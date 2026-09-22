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
  assert.equal(h.calls.includes('grant'), false);
  assert.deepEqual(result, { type: 'browser-action-result', actionId: 'action', status: 'ok', code: 'invoke-extension' });
});
