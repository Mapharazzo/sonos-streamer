import { isSupportedMediaUrl } from './shared/background-state.js';
import { CaptureLifecycle } from './shared/managed-state.js';
import { effectiveDelay } from './shared/session-state.js';

export class ManagedBackground {
  constructor({ chrome, ensureOffscreen, sendOffscreen, onState }) {
    Object.assign(this, { chrome, ensureOffscreen, sendOffscreen, onState });
    this.token = ''; this.plexOrigins = []; this.snapshot = null; this.connected = false; this.tabId = null; this.warning = null; this.videoTabId = null; this.counters = {};
    this.lifecycle = new CaptureLifecycle({
      disposeAudio: () => this.sendOffscreen({ type: 'managed-stop' }),
      disposeVideo: () => this.stopVideo(),
      acknowledge: (transitionId) => this.command({ type: 'capture-released', transitionId }),
    });
  }
  async restore(token, plexOrigins) {
    this.token = token; this.plexOrigins = plexOrigins;
    if (!token) return;
    await this.ensureOffscreen();
    const state = await this.sendOffscreen({ type: 'managed-connect', token });
    this.connected = Boolean(state?.connected); this.snapshot = state?.snapshot ?? null; this.tabId = state?.capturing ? state.tabId : null; this.videoTabId = this.tabId;
    this.publish();
  }
  publicState() {
    const state = this.snapshot;
    return { mode: 'managed', connected: this.connected, paired: this.connected && Boolean(state), hasStoredToken: Boolean(this.token), plexOrigins: this.plexOrigins, phase: state?.phase ?? 'disconnected', source: state?.source ?? 'chrome', selectedRendererId: state?.rendererId ?? null, renderers: state?.renderers ?? [], volume: state?.volume ?? null, measuredDelayMs: state?.measuredDelayMs ?? 0, manualOffsetMs: state?.manualOffsetMs ?? 0, capabilities: state?.capabilities ?? {}, warning: this.warning ?? state?.errorCode, tabId: this.tabId, counters: this.counters, cleanupPending: this.lifecycle.busy };
  }
  publish() { this.onState(this.publicState()); }
  async command(action) { return this.sendOffscreen({ type: 'managed-command', action }); }
  async popup(message) {
    if (message.type === 'state-request') return this.publicState();
    this.warning = null;
    switch (message.command) {
      case 'pair': {
        const token = message.token?.trim() || this.token;
        if (!/^[a-fA-F0-9]{64}$/.test(token)) throw new Error('Enter the Windows app pairing token');
        if (this.lifecycle.busy || ['starting', 'streaming', 'switching'].includes(this.snapshot?.phase)) throw new Error('Stop playback before pairing');
        await this.chrome.storage.local.set({ pairingToken: token }); await this.restore(token, this.plexOrigins); break;
      }
      case 'select-source': await this.command({ type: 'select-source', source: message.source }); break;
      case 'select-renderer': await this.command({ type: 'select-renderer', rendererId: message.rendererId }); break;
      case 'cast': if (this.snapshot?.source === 'windows') await this.command({ type: 'start-windows' }); else await this.cast(); break;
      case 'stop': await this.command({ type: 'stop' }); break;
      case 'set-volume': await this.command({ type: 'set-volume', value: message.value }); break;
      case 'calibrate': await this.command({ type: 'calibrate' }); break;
      case 'set-offset': await this.command({ type: 'set-offset', milliseconds: message.milliseconds }); break;
      default: throw new Error('Unknown managed command');
    }
    return this.publicState();
  }
  async cast(invokedTab) {
    return this.lifecycle.start(async (check) => {
      if (!this.connected || !this.snapshot?.rendererId || this.snapshot.source !== 'chrome' || !['idle', 'failed'].includes(this.snapshot.phase)) throw new Error('Select Chrome and a Sonos room before casting');
      const tab = invokedTab ?? (await this.chrome.tabs.query({ active: true, currentWindow: true }))[0]; check();
      if (!Number.isInteger(tab?.id) || !isSupportedMediaUrl(tab.url, this.plexOrigins)) throw new Error('Open YouTube or Plex in the active tab');
      await this.ensureOffscreen(); check();
      // Only this explicit popup/shortcut invocation can request the capture grant.
      const streamId = await this.chrome.tabCapture.getMediaStreamId({ targetTabId: tab.id }); check();
      const prepared = await this.command({ type: 'prepare-browser' }); check();
      const state = prepared.state;
      this.tabId = tab.id;
      try {
        await this.sendOffscreen({ type: 'managed-start', streamId, tabId: tab.id, leaseId: prepared.leaseId, instanceId: state.instanceId, generation: state.generation }); check();
        await this.command({ type: 'start-browser', leaseId: prepared.leaseId }); check();
      } catch (error) {
        // An external transition owns cleanup. Never send an old Stop into its destination.
        check(); await this.command({ type: 'stop' }).catch(() => {}); throw error;
      }
      return this.publicState();
    });
  }
  async event(event) {
    if (event.type === 'state') {
      if (this.snapshot?.instanceId === event.instanceId && this.snapshot.revision > event.revision) return;
      this.snapshot = event; this.connected = true;
      if (event.source === 'chrome' && event.phase === 'streaming' && Number.isInteger(this.tabId)) await this.applyVideoDelay();
      if (event.source !== 'chrome' || event.phase === 'idle' || event.phase === 'failed') {
        if (Number.isInteger(this.tabId)) await this.lifecycle.release();
      }
    } else if (event.type === 'hello-ok') { this.snapshot = null; this.connected = true; }
    else if (event.type === 'release-capture') { await this.lifecycle.release(event.transitionId); }
    else if (['disconnected', 'error', 'producer-disconnected', 'capture-ended'].includes(event.type)) {
      if (event.type === 'disconnected' || event.type === 'error') { this.connected = false; this.snapshot = null; }
      if (event.code) this.warning = event.code;
      await this.lifecycle.release();
    } else if (event.type === 'audio-stats') this.counters = event;
    this.publish();
  }
  async tabClosedOrNavigated(tabId) {
    if (tabId !== this.tabId) return;
    // The old document and video overlay are gone. Server Stop requests audio cleanup.
    this.videoTabId = null;
    if (!this.lifecycle.busy && this.snapshot?.source === 'chrome' && ['starting', 'streaming'].includes(this.snapshot.phase)) await this.command({ type: 'stop' }).catch(() => {});
    await this.lifecycle.release(); this.publish();
  }
  async stopVideo() {
    if (Number.isInteger(this.videoTabId)) {
      try { await this.chrome.tabs.sendMessage(this.videoTabId, { type: 'video-delay:stop' }); }
      catch (error) {
        const exists = await this.chrome.tabs.get(this.videoTabId).then(() => true, () => false);
        if (exists) throw error;
      }
    }
    this.videoTabId = null; this.tabId = null;
  }
  async applyVideoDelay() {
    const tabId = this.tabId; const generation = this.snapshot.generation;
    const milliseconds = effectiveDelay(this.snapshot.measuredDelayMs, this.snapshot.manualOffsetMs);
    try {
      await this.chrome.tabs.sendMessage(tabId, { type: 'video-delay:set', milliseconds });
      if (this.tabId !== tabId || this.snapshot?.generation !== generation || this.snapshot?.phase !== 'streaming') {
        await this.chrome.tabs.sendMessage(tabId, { type: 'video-delay:stop' }); return;
      }
      this.videoTabId = tabId;
    }
    catch { this.warning = 'Video delay unavailable; audio continues'; }
  }
}
