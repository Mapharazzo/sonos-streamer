import { isCapturableTabUrl, isSupportedMediaUrl } from './shared/background-state.js';
import { CaptureLifecycle } from './shared/managed-state.js';
import { effectiveDelay } from './shared/session-state.js';
import { reloadSelectedTab, shortcutStatus } from './shared/recovery.js';
import { loadNativeBootstrap } from './shared/native-bootstrap.js';

export class ManagedBackground {
  constructor({ chrome, ensureOffscreen, sendOffscreen, onState }) {
    Object.assign(this, { chrome, ensureOffscreen, sendOffscreen, onState });
    this.token = ''; this.plexOrigins = []; this.snapshot = null; this.connected = false; this.tabId = null; this.warning = null; this.videoTabId = null; this.counters = {};
    this.recoveryTab = null; this.recoveryRunning = false; this.lastBrowserStatus = ''; this.statusPending = false;
    this.pendingTab = null;
    this.videoDelaySupported = false;
    this.active = true; this.bootstrapEpoch = 0; this.bootstrapPending = null; this.authRefreshed = false; this.setupStatus = 'connecting';
    this.lifecycle = new CaptureLifecycle({
      disposeAudio: () => this.sendOffscreen({ type: 'managed-stop' }),
      disposeVideo: () => this.stopVideo(),
      acknowledge: (transitionId) => this.command({ type: 'capture-released', transitionId }),
    });
  }
  deactivate() {
    this.active = false; this.bootstrapEpoch++; this.bootstrapPending = null;
    this.token = ''; this.connected = false; this.snapshot = null;
  }
  bootstrap({ refresh = false } = {}) {
    if (!this.active) return Promise.resolve();
    if (this.bootstrapPending) return this.bootstrapPending;
    if (this.token && !refresh) return Promise.resolve();
    const epoch = ++this.bootstrapEpoch;
    this.setupStatus = 'connecting';
    const current = () => this.active && epoch === this.bootstrapEpoch;
    const attempt = (async () => {
      try {
        const bootstrap = await loadNativeBootstrap(this.chrome);
        if (!current()) return;
        this.installationId = bootstrap.installationId;
        await this.restore(bootstrap.token, this.plexOrigins, epoch);
        if (!current()) return;
        await this.chrome.storage.local.remove?.('pairingToken');
        if (current()) this.setupStatus = this.connected ? 'connected' : 'connecting';
      } catch (error) {
        if (current()) { this.token = ''; this.setupStatus = error.message.startsWith('native-') ? error.message : 'app-unavailable'; }
      } finally { if (current()) this.publish(); }
    })();
    this.bootstrapPending = attempt;
    void attempt.finally(() => { if (this.bootstrapPending === attempt) this.bootstrapPending = null; }).catch(() => {});
    return attempt;
  }
  async restore(token, plexOrigins, epoch = this.bootstrapEpoch) {
    this.token = token; this.plexOrigins = plexOrigins;
    const recovery = await this.chrome.storage.session?.get('recoveryTab');
    if (!this.active || epoch !== this.bootstrapEpoch) return;
    this.recoveryTab = recovery?.recoveryTab ?? null;
    if (!token) return;
    await this.ensureOffscreen();
    if (!this.active || epoch !== this.bootstrapEpoch) return;
    const state = await this.sendOffscreen({ type: 'managed-connect', token });
    if (!this.active || epoch !== this.bootstrapEpoch) return;
    this.connected = Boolean(state?.connected); this.snapshot = state?.snapshot ?? null; this.tabId = state?.capturing ? state.tabId : null;
    if (Number.isInteger(this.tabId)) {
      const tab = await this.chrome.tabs.get(this.tabId).catch(() => null);
      if (!this.active || epoch !== this.bootstrapEpoch) return;
      this.videoDelaySupported = isSupportedMediaUrl(tab?.url, this.plexOrigins);
    }
    this.videoTabId = this.videoDelaySupported ? this.tabId : null;
    this.publish();
  }
  publicState() {
    const state = this.snapshot;
    return { mode: 'managed', videoDelaySupported: this.videoDelaySupported, setupStatus: this.setupStatus, connected: this.connected, paired: this.connected && Boolean(state), plexOrigins: this.plexOrigins, phase: state?.phase ?? 'disconnected', source: state?.source ?? 'chrome', selectedRendererId: state?.rendererId ?? null, renderers: state?.renderers ?? [], volume: state?.volume ?? null, measuredDelayMs: state?.measuredDelayMs ?? 0, manualOffsetMs: state?.manualOffsetMs ?? 0, capabilities: state?.capabilities ?? {}, warning: this.warning ?? state?.errorCode, tabId: this.tabId, counters: this.counters, cleanupPending: this.lifecycle.busy || this.recoveryRunning };
  }
  publish() { this.onState(this.publicState()); }
  async command(action) { return this.sendOffscreen({ type: 'managed-command', action }); }
  async popup(message) {
    if (message.type === 'state-request') { await this.refreshBrowserStatus(); return this.publicState(); }
    this.warning = null;
    switch (message.command) {
      case 'pair': {
        if (this.lifecycle.busy || ['starting', 'streaming', 'switching'].includes(this.snapshot?.phase)) throw new Error('Stop playback before pairing');
        this.authRefreshed = false;
        await this.bootstrap({ refresh: true }); break;
      }
      case 'select-source': await this.command({ type: 'select-source', source: message.source }); break;
      case 'select-renderer': await this.command({ type: 'select-renderer', rendererId: message.rendererId }); break;
      case 'cast': if (this.snapshot?.source === 'windows') await this.command({ type: 'start-windows' }); else await this.cast(); break;
      case 'stop': await this.command({ type: 'stop' }); break;
      case 'set-volume': await this.command({ type: 'set-volume', value: message.value }); break;
      case 'calibrate':
        if (!this.videoDelaySupported) throw new Error('This tab uses audio-only capture; video calibration is unavailable');
        await this.command({ type: 'calibrate' }); break;
      case 'set-offset':
        if (!this.videoDelaySupported) throw new Error('This tab uses audio-only capture; video delay is unavailable');
        await this.command({ type: 'set-offset', milliseconds: message.milliseconds }); break;
      case 'reload-tab': await this.reload(); break;
      default: throw new Error('Unknown managed command');
    }
    return this.publicState();
  }
  async cast(invokedTab) {
    if (this.recoveryRunning) throw new Error('reload-in-progress');
    return this.lifecycle.start(async (check) => {
      if (!this.connected || !this.snapshot?.rendererId || this.snapshot.source !== 'chrome' || !['idle', 'failed'].includes(this.snapshot.phase)) throw new Error('Select Chrome and a Sonos room before casting');
      const tab = invokedTab ?? (await this.chrome.tabs.query({ active: true, currentWindow: true }))[0]; check();
      if (!Number.isInteger(tab?.id) || !isCapturableTabUrl(tab.url)) throw new Error('Open an ordinary HTTP or HTTPS web page to cast audio');
      this.videoDelaySupported = isSupportedMediaUrl(tab.url, this.plexOrigins);
      const pending = { id: tab.id, url: tab.url };
      this.pendingTab = pending;
      try {
      this.recoveryTab = { tabId: tab.id, origin: new URL(tab.url).origin };
      await this.chrome.storage.session?.set({ recoveryTab: this.recoveryTab }); check();
      await this.ensureOffscreen(); check();
      // Only this explicit popup/shortcut invocation can request the capture grant.
      const streamId = await this.chrome.tabCapture.getMediaStreamId({ targetTabId: tab.id }); check();
      const prepared = await this.command({ type: 'prepare-browser' }); check();
      const state = prepared.state;
      this.tabId = tab.id;
      try {
        const current = await this.chrome.tabs.get(tab.id).catch(() => null); check();
        if (current?.id !== tab.id || current?.url !== pending.url) throw new Error('tab-changed');
        await this.sendOffscreen({ type: 'managed-start', streamId, tabId: tab.id, leaseId: prepared.leaseId, instanceId: state.instanceId, generation: state.generation }); check();
        await this.command({ type: 'start-browser', leaseId: prepared.leaseId }); check();
      } catch (error) {
        // An external transition owns cleanup. Never send an old Stop into its destination.
        check(); await this.command({ type: 'stop' }).catch(() => {}); throw error;
      }
      return this.publicState();
      } finally { if (this.pendingTab === pending) this.pendingTab = null; }
    });
  }
  async event(event) {
    if (!this.active) return;
    if (event.type === 'state') {
      if (this.snapshot?.instanceId === event.instanceId && this.snapshot.revision > event.revision) return;
      this.snapshot = event; this.connected = true;
      if (event.source === 'chrome' && event.phase === 'streaming' && Number.isInteger(this.tabId)) await this.applyVideoDelay();
      if (event.source !== 'chrome' || event.phase === 'idle' || event.phase === 'failed') {
        if (Number.isInteger(this.tabId)) await this.lifecycle.release();
      }
      void this.refreshBrowserStatus().catch(() => {});
    } else if (event.type === 'hello-ok') { this.snapshot = null; this.connected = true; this.lastBrowserStatus = ''; this.authRefreshed = false; this.setupStatus = 'connected'; this.warning = null; }
    else if (event.type === 'browser-action') {
      let status = 'ok', code = 'invoke-extension';
      try { await this.reload(); } catch (error) {
        status = 'failed';
        const allowed = ['reload-in-progress', 'no-recovery-tab', 'select-chrome-source', 'capture-cleanup-failed', 'tab-changed', 'reload-timeout', 'control-not-ready', 'control-disconnected'];
        code = allowed.includes(error.message) ? error.message : 'reload-failed';
      }
      await this.command({ type: 'browser-action-result', actionId: event.actionId, status, code });
    }
    else if (event.type === 'release-capture') { await this.lifecycle.release(event.transitionId); }
    else if (['disconnected', 'error', 'producer-disconnected', 'capture-ended'].includes(event.type)) {
      if (event.type === 'disconnected' || event.type === 'error') { this.connected = false; this.snapshot = null; }
      if (event.code) this.warning = event.code;
      await this.lifecycle.release();
      if (event.type === 'error' && event.code === 'authentication-failed') {
        this.token = '';
        if (!this.authRefreshed) {
          this.authRefreshed = true;
          await this.bootstrapPending;
          await this.bootstrap({ refresh: true });
        } else this.setupStatus = 'app-authentication-failed';
      }
    } else if (event.type === 'audio-stats') this.counters = event;
    this.publish();
  }
  async tabClosedOrNavigated(tabId, { captureEnded = false } = {}) {
    if (this.recoveryRunning) return;
    if (tabId !== this.tabId && tabId !== this.pendingTab?.id) return;
    if (captureEnded) {
      // Track shutdown can precede the server's source-switch state. Release
      // local resources without replacing that transition with another Stop.
      // An unexpected end closes the producer, which the server handles itself.
      await this.lifecycle.release(); this.publish(); return;
    }
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
    const tab = await this.chrome.tabs.get(tabId).catch(() => null);
    if (this.tabId !== tabId || this.snapshot?.generation !== generation) return;
    this.videoDelaySupported = isSupportedMediaUrl(tab?.url, this.plexOrigins);
    if (!this.videoDelaySupported) return;
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

  async refreshBrowserStatus() {
    if (!this.connected || !this.snapshot || this.statusPending) return;
    this.statusPending = true;
    try {
      const binding = shortcutStatus(await this.chrome.commands?.getAll() ?? []);
      const [tab] = await this.chrome.tabs.query({ active: true, currentWindow: true });
      if (!Number.isInteger(this.tabId) && !this.pendingTab) this.videoDelaySupported = isSupportedMediaUrl(tab?.url, this.plexOrigins);
      if (this.recoveryTab) {
        const recovery = await this.chrome.tabs.get(this.recoveryTab.tabId).catch(() => null);
        if (!recovery?.url || new URL(recovery.url).origin !== this.recoveryTab.origin) this.recoveryTab = null;
      }
      const action = { type: 'browser-status', shortcut: binding.shortcut, supportedTab: isCapturableTabUrl(tab?.url), hasRecoveryTab: Boolean(this.recoveryTab) };
      const signature = JSON.stringify(action);
      if (signature !== this.lastBrowserStatus) { this.lastBrowserStatus = signature; await this.command(action); }
    } catch { this.lastBrowserStatus = ''; }
    finally { this.statusPending = false; }
  }
  async reload() {
    if (this.recoveryRunning) throw new Error('reload-in-progress');
    if (!this.recoveryTab) throw new Error('no-recovery-tab');
    if (this.snapshot?.source !== 'chrome') throw new Error('select-chrome-source');
    this.recoveryRunning = true; this.publish();
    const { tabId, origin } = this.recoveryTab; let playback = null;
    try {
      const result = await reloadSelectedTab({ tabId, expectedOrigin: origin, tabs: this.chrome.tabs,
        stopCapture: async () => {
          playback = await this.chrome.tabs.sendMessage(tabId, { type: 'recovery:snapshot' }).catch(() => null);
          await this.command({ type: 'stop' });
          const deadline = Date.now() + 5000;
          while (this.snapshot?.phase !== 'idle') {
            if (!this.connected || Date.now() >= deadline || this.snapshot?.phase === 'failed') throw new Error('capture-cleanup-failed');
            await new Promise((resolve) => setTimeout(resolve, 25));
          }
          await this.lifecycle.release();
        },
        waitReady: async (id, signal) => {
          while (!signal.aborted) {
            const tab = await this.chrome.tabs.get(id);
            if (!tab.url || new URL(tab.url).origin !== origin) throw new Error('tab-changed');
            if (tab.status === 'complete') {
              if (!isSupportedMediaUrl(tab.url, this.plexOrigins)) return;
              const ready = await this.chrome.tabs.sendMessage(id, { type: 'recovery:ready' }).catch(() => null);
              if (ready?.ready) return;
            }
            await new Promise((resolve) => setTimeout(resolve, 100));
          }
          throw new Error('reload-timeout');
        },
        restorePlayback: (id, signal) => { if (signal.aborted) throw new Error('reload-timeout'); return this.chrome.tabs.sendMessage(id, { type: 'recovery:restore', playback }); },
      });
      this.warning = 'Tab reloaded. Press Cast or the Chrome shortcut to resume.';
      return result;
    } finally { this.recoveryRunning = false; this.publish(); }
  }
}
