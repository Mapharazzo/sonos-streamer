import test from 'node:test';
import assert from 'node:assert/strict';
import { ManagedBackground } from '../managed-background.js';
const state = { type: 'state', instanceId: 'i', revision: 1, generation: 1, source: 'chrome', phase: 'idle', rendererId: 'room', renderers: [], measuredDelayMs: 0, manualOffsetMs: 0, capabilities: {} };

test('completed calibration immediately updates the captured tab without another Apply click', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []);
  h.background.tabId = 42;
  const delays = [];
  h.chrome.tabs.sendMessage = async (tabId, message) => delays.push({ tabId, ...message });
  await h.background.event({ ...state, phase: 'streaming', revision: 2, manualOffsetMs: 5000 });
  await h.background.event({ ...state, phase: 'streaming', revision: 3, measuredDelayMs: 513, manualOffsetMs: 0 });
  assert.deepEqual(delays, [
    { tabId: 42, type: 'video-delay:set', milliseconds: 5000 },
    { tabId: 42, type: 'video-delay:set', milliseconds: 513 },
  ]);
});
function harness() {
  const calls = [];
  const chrome = { storage: { local: { set: async () => {} } }, tabs: { query: async () => [{ id: 42, url: 'https://www.youtube.com/watch?v=test' }], sendMessage: async (tab, message) => calls.push(message.type), get: async () => ({ id: 42, url: 'https://www.youtube.com/watch?v=test' }) }, tabCapture: { getMediaStreamId: async () => { calls.push('grant'); return 'grant'; } } };
  const engine = async (message) => {
    calls.push(message.type === 'managed-command' ? message.action.type : message.type);
    if (message.type === 'managed-connect') return { connected: true, snapshot: state, capturing: false };
    if (message.action?.type === 'prepare-browser') return { leaseId: 'lease', state: { ...state, generation: 2 } };
    return {};
  };
  const background = new ManagedBackground({ chrome, ensureOffscreen: async () => {}, sendOffscreen: (message) => engine(message), onState: () => {} });
  return { background, chrome, calls };
}

test('explicit Cast takes over another browser only after its cleanup reaches idle', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []);
  h.background.snapshot = { ...state, phase: 'streaming' };
  const send = h.background.sendOffscreen;
  h.background.sendOffscreen = async message => {
    const result = await send(message);
    if (message.action?.type === 'stop') {
      await h.background.event({ ...state, generation: 2, revision: 2, phase: 'switching' });
      setTimeout(() => { void h.background.event({ ...state, generation: 2, revision: 3 }); }, 10);
    }
    if (message.action?.type === 'prepare-browser') assert.equal(h.background.snapshot.phase, 'idle');
    return result;
  };
  await h.background.cast();
  assert.deepEqual(h.calls.filter(c => ['grant', 'stop', 'prepare-browser', 'start-browser'].includes(c)), ['grant', 'stop', 'prepare-browser', 'start-browser']);
});

test('a competing source change cancels takeover without starting another producer', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []);
  h.background.snapshot = { ...state, phase: 'streaming' };
  const send = h.background.sendOffscreen;
  h.background.sendOffscreen = async message => {
    const result = await send(message);
    if (message.action?.type === 'stop') await h.background.event({ ...state, generation: 3, revision: 3, source: 'windows' });
    return result;
  };
  await assert.rejects(h.background.cast(), /capture-cancelled/);
  assert.equal(h.calls.includes('prepare-browser'), false);
  assert.equal(h.calls.includes('managed-start'), false);
});

test('reload from an observing browser is routed to the active browser by the server', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []);
  await h.background.popup({command: 'reload-tab'});
  assert.equal(h.calls.includes('reload-tab'), true);
  assert.equal(h.calls.includes('grant'), false);
});

test('explicit browser shortcut can switch Windows playback to tab capture', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []);
  h.background.snapshot = { ...state, source: 'windows', phase: 'streaming' };
  const send = h.background.sendOffscreen;
  h.background.sendOffscreen = async message => {
    const result = await send(message);
    if (message.action?.type === 'select-source') await h.background.event({ ...state, generation: 2, revision: 2 });
    return result;
  };
  await h.background.cast();
  assert.deepEqual(h.calls.filter(c => ['grant', 'select-source', 'prepare-browser', 'start-browser'].includes(c)), ['grant', 'select-source', 'prepare-browser', 'start-browser']);
});

test('observing another browser after reconnect never captures or stops its stream', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []);
  await h.background.event({ ...state, phase: 'streaming', revision: 2 });
  assert.equal(h.calls.includes('grant'), false);
  assert.equal(h.calls.includes('stop'), false);
  assert.equal(h.calls.includes('managed-start'), false);
});

test('observer video controls follow the active cast capability, not its foreground tab', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []);
  h.background.videoDelaySupported = false;
  h.background.snapshot = { ...state, phase: 'streaming', capabilities: {calibrationAvailable: true} };
  assert.equal(h.background.publicState().videoDelaySupported, true);
  await h.background.popup({command: 'calibrate'});
  assert.equal(h.calls.includes('calibrate'), true);
  h.background.videoDelaySupported = true;
  h.background.snapshot.capabilities.calibrationAvailable = false;
  assert.equal(h.background.publicState().videoDelaySupported, false);
  await assert.rejects(h.background.popup({command: 'calibrate'}), /audio-only/i);
});

test('generic tab capture needs no content script, video delay, or calibration', async () => {
  const h = harness();
  const tab = { id: 42, url: 'http://127.0.0.1:58503/', status: 'complete' };
  h.chrome.tabs.query = async () => [tab]; h.chrome.tabs.get = async () => tab;
  h.chrome.tabs.sendMessage = async () => { throw new Error('No content script'); };
  await h.background.restore('a'.repeat(64), []);
  await h.background.cast();
  await h.background.event({ ...state, phase: 'streaming', revision: 2, measuredDelayMs: 5000 });
  assert.equal(h.background.publicState().videoDelaySupported, false);
  assert.equal(h.background.warning, null);
  assert.equal(h.background.videoTabId, null);
  await assert.rejects(h.background.popup({ command: 'calibrate' }), /audio-only/i);
  await h.background.event({ type: 'release-capture', transitionId: 'generic-stop' });
  assert.equal(h.calls.includes('capture-released'), true);
});

test('generic reload completes without a video adapter and never starts capture', async () => {
  const h = harness();
  const tab = { id: 42, url: 'https://example.org/music', status: 'complete' };
  h.chrome.tabs.query = async () => [tab]; h.chrome.tabs.get = async () => tab;
  h.chrome.tabs.sendMessage = async () => { throw new Error('No content script'); };
  h.chrome.tabs.reload = async () => h.calls.push('reload');
  await h.background.restore('a'.repeat(64), []);
  h.background.recoveryTab = { tabId: 42, origin: 'https://example.org' };
  await h.background.reload('generic-reload');
  assert.equal(h.calls.includes('reload'), true);
  assert.equal(h.calls.includes('grant'), false);
});

test('protected pages are rejected before requesting a capture grant', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []);
  await assert.rejects(h.background.cast({ id: 42, url: 'chrome://settings' }), /web page/i);
  assert.equal(h.calls.includes('grant'), false);
});
test('pair/reconnect opens only control and Cast explicitly gets capture grant', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []);
  assert.deepEqual(h.calls, ['managed-connect']);
  await h.background.cast();
  assert.deepEqual(h.calls, ['managed-connect', 'grant', 'prepare-browser', 'managed-start', 'start-browser']);
});
test('external source switch disposes capture and delayed video before ack, without sending Stop', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []); h.background.tabId = 42; h.background.videoTabId = 42; h.calls.length = 0;
  await h.background.event({ type: 'release-capture', transitionId: 'transition-3' });
  assert.deepEqual(h.calls, ['managed-stop', 'video-delay:stop', 'capture-released']);
  assert.equal(h.background.tabId, null);
});

test('capture stopped notification arriving before source-switch state cannot replace its destination', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []);
  h.background.snapshot = { ...state, phase: 'streaming' };
  h.background.tabId = 42; h.background.videoTabId = 42; h.calls.length = 0;
  // Chrome can report tracks stopped before the worker receives the server's
  // Switching snapshot/release request. Its cached Streaming state is stale.
  await h.background.tabClosedOrNavigated(42, { captureEnded: true });
  await h.background.event({ type: 'release-capture', transitionId: 'switch-to-windows' });
  assert.equal(h.calls.includes('stop'), false);
  assert.equal(h.calls.includes('video-delay:stop'), true);
  assert.equal(h.calls.includes('capture-released'), true);
  assert.equal(h.background.tabId, null);
});
test('failed audio disposal never acknowledges capture release', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []);
  h.background.sendOffscreen = async (message) => { h.calls.push(message.type); if (message.type === 'managed-stop') throw new Error('cleanup-failed'); };
  await assert.rejects(h.background.event({ type: 'release-capture', transitionId: 't' }));
  assert.equal(h.calls.includes('capture-released'), false);
});
test('competing Cast calls do not mint two capture grants', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []);
  const results = await Promise.allSettled([h.background.cast(), h.background.cast()]);
  assert.equal(results.filter((result) => result.status === 'fulfilled').length, 1);
  assert.equal(h.calls.filter((call) => call === 'grant').length, 1);
});

test('navigation while prepare-browser is pending cancels setup before audio starts', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []);
  let finishPrepare, preparing;
  const started = new Promise((resolve) => { preparing = resolve; });
  const original = h.background.sendOffscreen;
  h.background.sendOffscreen = async (message) => {
    if (message.action?.type === 'prepare-browser') {
      preparing();
      await new Promise((resolve) => { finishPrepare = resolve; });
    }
    return original(message);
  };
  const cast = h.background.cast();
  const cancelled = assert.rejects(cast, /capture-cancelled/);
  await started;
  await h.background.tabClosedOrNavigated(42);
  finishPrepare();
  await cancelled;
  assert.equal(h.calls.includes('managed-start'), false);
  assert.equal(h.calls.includes('start-browser'), false);
});

test('reload from native controller completes once and never requests capture itself', async () => {
  const h = harness(); await h.background.restore('a'.repeat(64), []);
  h.background.recoveryTab = { tabId: 42, origin: 'https://www.youtube.com' };
  h.chrome.tabs.get = async () => ({ id: 42, url: 'https://www.youtube.com/watch?v=test', status: 'complete' });
  h.chrome.tabs.reload = async () => h.calls.push('reload');
  h.chrome.tabs.sendMessage = async (_tab, message) => { h.calls.push(message.type); return message.type === 'recovery:ready' ? { ready: true } : {}; };
  let result;
  const send = h.background.sendOffscreen;
  h.background.sendOffscreen = async (message) => { if (message.action?.type === 'browser-action-result') result = message.action; return send(message); };
  await h.background.event({ type: 'browser-action', actionId: 'action' });
  assert.equal(h.calls.filter((call) => call === 'reload').length, 1);
  assert.equal(h.calls.includes('stop-for-reload'), true);
  assert.equal(h.calls.includes('stop'), false);
  assert.equal(h.calls.includes('grant'), false);
  assert.deepEqual(result, { type: 'browser-action-result', actionId: 'action', status: 'ok', code: 'invoke-extension' });
});
