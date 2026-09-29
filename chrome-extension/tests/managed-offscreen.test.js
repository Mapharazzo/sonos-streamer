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
    if (value.type === 'command') Socket.onCommand?.(this, value);
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
  globalThis.chrome = { runtime: { id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", getURL: (path) => "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/"+path, onMessage: { addListener(callback) { listener = callback; } }, sendMessage: async () => {} } };
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { mediaDevices: { getUserMedia() { mediaRequested(); return new Promise((resolve) => { completeMedia = resolve; }); } } } });
  await import('../offscreen.js?managed-offscreen-test');
  const send = (message) => new Promise((resolve) => assert.equal(listener(message, { id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", url: "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/background.js" }, resolve), true));
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

test('a pending reload result does not block the commands needed to finish reload', async () => {
  let listener, reloadRequest;
  const calls = [];
  globalThis.WebSocket = Socket;
  globalThis.chrome = { runtime: { id: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', getURL: path => 'chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/' + path, onMessage: {addListener(callback) {listener = callback;}}, sendMessage: async () => {} } };
  Socket.onCommand = (socket, command) => {
    calls.push(command.action.type);
    const reply = request => socket.emit('message', {data: JSON.stringify({type: 'result', requestId: request.requestId, status: 'ok', code: '', revision: 3})});
    if (command.action.type === 'reload-tab') reloadRequest = command;
    else { reply(command); if (command.action.type === 'browser-action-result') reply(reloadRequest); }
  };
  await import('../offscreen.js?reload-queue-test');
  const send = message => new Promise(resolve => listener(message, {id: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', url: 'chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/background.js'}, resolve));
  await send({type: 'managed-connect', token: 'a'.repeat(64)}); await new Promise(setImmediate);
  const reload = send({type: 'managed-command', action: {type: 'reload-tab'}});
  const stop = send({type: 'managed-command', action: {type: 'stop'}});
  try {
    await new Promise(setImmediate);
    assert.deepEqual(calls, ['reload-tab', 'stop']);
    assert.equal((await stop).ok, true);
    assert.equal((await send({type: 'managed-command', action: {type: 'browser-action-result', actionId: 'reload', status: 'ok', code: ''}})).ok, true);
    assert.equal((await reload).ok, true);
  } finally { await send({type: 'managed-close'}); await Promise.allSettled([reload, stop]); Socket.onCommand = null; }
});

test('queued capture cleanup cannot stop a newer source before dispatch', async () => {
  let listener, heldSocket, heldCommand;
  const calls = [];
  globalThis.WebSocket = Socket;
  globalThis.chrome = { runtime: { id: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', getURL: path => 'chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/' + path, onMessage: {addListener(callback) {listener = callback;}}, sendMessage: async () => {} } };
  Socket.onCommand = (socket, command) => { calls.push(command.action.type); heldSocket = socket; heldCommand = command; };
  await import('../offscreen.js?stale-capture-queue-test');
  const send = message => new Promise(resolve => listener(message, {id: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', url: 'chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/background.js'}, resolve));
  await send({type: 'managed-connect', token: 'a'.repeat(64)}); await new Promise(setImmediate);
  const held = send({type: 'managed-command', action: {type: 'get-state'}});
  const stale = send({type: 'managed-command', action: {type: 'stop'}, expectedState: {instanceId: state.instanceId, generation: state.generation, source: state.source}});
  try {
    await new Promise(setImmediate);
    heldSocket.emit('message', {data: JSON.stringify({...state, generation: 3, revision: 4, source: 'windows'})});
    heldSocket.emit('message', {data: JSON.stringify({type: 'result', requestId: heldCommand.requestId, status: 'ok', code: '', revision: 4})});
    assert.equal((await held).ok, true);
    assert.equal((await stale).error, 'capture-cancelled');
    assert.deepEqual(calls, ['get-state']);
  } finally { await send({type: 'managed-close'}); await Promise.allSettled([held, stale]); Socket.onCommand = null; }
});
