import test from 'node:test';
import assert from 'node:assert/strict';
import { ManagedClient } from '../shared/managed-client.js';
class Socket {
  readyState = 0; sent = []; listeners = {};
  addEventListener(type, listener) { this.listeners[type] = listener; }
  emit(type, data) { if (type === 'open') this.readyState = 1; if (type === 'close') this.readyState = 3; this.listeners[type]?.(data); }
  send(data) { this.sent.push(JSON.parse(data)); }
  close() { this.emit('close'); }
}
function harness() {
  const sockets = [], timers = new Map(), events = []; let sequence = 0;
  const client = new ManagedClient({ connect: () => { const socket = new Socket(); sockets.push(socket); return socket; }, schedule: (fn, ms) => { const id = ++sequence; timers.set(id, { fn, ms }); return id; }, cancel: (id) => timers.delete(id), onEvent: (event) => events.push(event) });
  return { client, sockets, timers, events, fire(ms) { const entry = [...timers].find(([, value]) => value.ms === ms); assert.ok(entry, `timer ${ms}`); timers.delete(entry[0]); entry[1].fn(); } };
}
test('control reconnects with bounded backoff and never replays mutations', async () => {
  const h = harness(); h.client.connect('a'.repeat(64)); h.sockets[0].emit('open');
  assert.equal(h.sockets[0].sent[0].role, 'browser');
  h.sockets[0].emit('message', { data: JSON.stringify({ type: 'hello-ok', version: 1, instanceId: 'i', connectionId: 'c' }) });
  const pending = h.client.send({ type: 'command', requestId: 'x', instanceId: 'i', expectedRevision: 1, action: { type: 'stop' } });
  h.sockets[0].emit('close'); await assert.rejects(pending, /disconnected/);
  h.fire(500); h.sockets[1].emit('open');
  assert.equal(h.sockets[1].sent.length, 1); assert.equal(h.sockets[1].sent[0].type, 'hello');
  h.sockets[1].emit('close'); h.fire(1000);
  h.client.close(); assert.equal(h.timers.size, 0);
});
test('terminal authentication failure cancels retries and pending requests', () => {
  const h = harness(); h.client.connect('a'.repeat(64)); h.sockets[0].emit('open');
  h.sockets[0].emit('message', { data: JSON.stringify({ type: 'error', code: 'authentication-failed' }) });
  assert.equal(h.timers.size, 0);
  assert.equal(h.events.at(-1).code, 'authentication-failed');
});

test('default timers keep the browser global receiver during pairing and cleanup', (t) => {
  const timers = new Map(); let sequence = 0;
  t.mock.method(globalThis, 'setTimeout', function (callback, milliseconds) {
    if (this !== globalThis) throw new TypeError('Illegal invocation');
    const id = ++sequence; timers.set(id, { callback, milliseconds }); return id;
  });
  t.mock.method(globalThis, 'clearTimeout', function (id) {
    if (this !== globalThis) throw new TypeError('Illegal invocation');
    timers.delete(id);
  });
  const socket = new Socket();
  const client = new ManagedClient({ connect: () => socket });
  assert.doesNotThrow(() => client.connect('a'.repeat(64)));
  socket.emit('open');
  socket.emit('message', { data: JSON.stringify({ type: 'hello-ok', version: 1, instanceId: 'i', connectionId: 'c' }) });
  assert.equal(client.authenticated, true);
  assert.equal(timers.size, 0);
  client.later(() => {}, 500);
  assert.doesNotThrow(() => client.close());
  assert.equal(timers.size, 0);
});
