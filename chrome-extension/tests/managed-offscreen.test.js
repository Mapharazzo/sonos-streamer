import test from 'node:test';
import assert from 'node:assert/strict';
const state = { type: 'state', instanceId: 'instance', revision: 3, generation: 2, source: 'chrome', phase: 'starting', rendererId: 'room', renderers: [], volume: null, measuredDelayMs: 0, manualOffsetMs: 0, errorCode: null, capabilities: { windowsAvailable: true, browserConnected: true, browserCanCast: true, browserCanReload: false, calibrationAvailable: false, shortcut: '' } };
class Socket {
  static OPEN = 1; static CLOSING = 2; static CLOSED = 3;
  readyState = 0; listeners = new Map();
  constructor(url) { this.url = url; queueMicrotask(() => { this.readyState = 1; this.emit('open', {}); }); }
  addEventListener(type, callback) { this.listeners.set(type, [...(this.listeners.get(type) ?? []), callback]); }
  emit(type, event) { for (const callback of this.listeners.get(type) ?? []) callback(event); }
  send(json) {
    const value = JSON.parse(json);
    if (value.type === 'hello') queueMicrotask(() => {
      if (this.url.endsWith('/control/v1')) {
        this.emit('message', { data: JSON.stringify({ type: 'hello-ok', version: 1, instanceId: 'instance', connectionId: 'browser' }) });
        this.emit('message', { data: JSON.stringify(state) });
      } else this.emit('message', { data: JSON.stringify({ type: 'hello-ok', version: 1, instanceId: 'instance', generation: 2 }) });
    });
  }
  close() { if (this.readyState === 3) return; this.readyState = 3; this.emit('close', {}); }
}
test('release waits for in-flight getUserMedia and ends its tracks before reporting cleanup', async () => {
  let listener, completeMedia, mediaRequested;
  const requested = new Promise((resolve) => { mediaRequested = resolve; });
  globalThis.WebSocket = Socket;
  globalThis.chrome = { runtime: { onMessage: { addListener(callback) { listener = callback; } }, sendMessage: async () => {} } };
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { mediaDevices: { getUserMedia() { mediaRequested(); return new Promise((resolve) => { completeMedia = resolve; }); } } } });
  await import('../offscreen.js?managed-offscreen-test');
  const send = (message) => new Promise((resolve) => assert.equal(listener(message, {}, resolve), true));
  await send({ type: 'managed-connect', token: 'a'.repeat(64) }); await new Promise(setImmediate);
  const starting = send({ type: 'managed-start', leaseId: 'lease', instanceId: 'instance', generation: 2, streamId: 'grant', tabId: 42 });
  await requested;
  let released = false; const stopping = send({ type: 'managed-stop' }).then((result) => { released = true; return result; });
  await new Promise(setImmediate); assert.equal(released, false);
  const track = { readyState: 'live', stop() { this.readyState = 'ended'; } };
  completeMedia({ getTracks: () => [track] });
  assert.equal((await starting).ok, false); assert.equal((await stopping).ok, true); assert.equal(track.readyState, 'ended');
  const remaining = await send({ type: 'managed-state' }); assert.equal(remaining.value.connected, true); assert.equal(remaining.value.capturing, false);
  await send({ type: 'managed-close' });
});
