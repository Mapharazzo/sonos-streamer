import test from 'node:test';
import assert from 'node:assert/strict';
import { loadNativeBootstrap } from '../shared/native-bootstrap.js';
import { ManagedBackground } from '../managed-background.js';
const value = { version: 1, managedProtocol: 1, installationId: 'fixture-host', controlUrl: 'ws://127.0.0.1:5902/control/v1', token: 'a'.repeat(64) };
function harness(load = async () => ({ ok: true, value })) {
  const sent = [], stored = [];
  const chrome = { runtime: { sendNativeMessage: load }, storage: { local: { remove: async (key) => stored.push(key) }, session: { get: async () => ({}) } } };
  const background = new ManagedBackground({ chrome, ensureOffscreen: async () => {}, sendOffscreen: async (message) => { sent.push(message); return { connected: true, snapshot: { phase: 'idle' } }; }, onState: () => {} });
  return { chrome, background, sent, stored };
}
test('native bootstrap validates fixed host, request and bounded response', async () => {
  const chrome = { runtime: { async sendNativeMessage(host, request) { assert.equal(host, 'com.mapharazzo.elgato_remote'); assert.deepEqual(request, { version: 1, type: 'get-bootstrap' }); return { ok: true, value }; } } };
  assert.deepEqual(await loadNativeBootstrap(chrome), value);
  for (const reply of [null, {}, { ok: false, code: 'origin-denied' }, { ok: true, value: { ...value, token: 'bad' } }, { ok: true, value: { ...value, managedProtocol: 2 } }, { ok: true, value: { ...value, controlUrl: 'ws://remote/control' } }, { ok: true, value, extra: true }, { ok: true, value: { ...value, extra: true } }]) {
    await assert.rejects(loadNativeBootstrap({ runtime: { sendNativeMessage: async () => reply } }), /app-|native-/);
  }
  await assert.rejects(loadNativeBootstrap({ runtime: { sendNativeMessage: async () => { throw new Error(value.token); } } }), (error) => !error.message.includes(value.token));
});
test('bootstrap coalesces and passes credential only to offscreen, removes old local token', async () => {
  let count = 0;
  const h = harness(async () => { count++; return { ok: true, value }; });
  await Promise.all([h.background.bootstrap(), h.background.bootstrap()]);
  assert.equal(count, 1); assert.equal(h.sent[0].token, value.token);
  assert.deepEqual(h.stored, ['pairingToken']);
  assert.equal(JSON.stringify(h.background.publicState()).includes(value.token), false);
  assert.equal(h.sent.some((m) => m.type === 'managed-start' || m.action?.type?.startsWith('start-')), false);
});
test('late native reply after leaving managed mode cannot reconnect', async () => {
  let finish;
  const h = harness(() => new Promise((resolve) => { finish = resolve; }));
  const pending = h.background.bootstrap();
  h.background.deactivate();
  finish({ ok: true, value }); await pending;
  assert.equal(h.sent.length, 0); assert.equal(h.background.token, '');
});
test('authentication rejection refreshes once without replaying capture', async () => {
  let count = 0;
  const h = harness(async () => ({ ok: true, value: { ...value, token: (++count === 1 ? 'a' : 'b').repeat(64) } }));
  await h.background.bootstrap();
  await h.background.event({ type: 'error', code: 'authentication-failed' });
  await h.background.event({ type: 'error', code: 'authentication-failed' });
  assert.equal(count, 2); assert.equal(h.background.setupStatus, 'app-authentication-failed');
  assert.equal(h.sent.filter((m) => m.type === 'managed-connect').length, 2);
  assert.equal(h.sent.some((m) => m.type === 'managed-start'), false);
});
test('missing host can recover on explicit Connect', async () => {
  const h = harness(async () => { throw new Error('missing host'); });
  await h.background.bootstrap(); assert.equal(h.background.setupStatus, 'app-unavailable');
  h.chrome.runtime.sendNativeMessage = async () => ({ ok: true, value });
  await h.background.popup({ type: 'popup-command', command: 'pair' });
  assert.equal(h.background.connected, true);
});
test('worker bootstrap restores surviving offscreen capture without acquiring a grant', async () => {
  const h = harness();
  h.background.sendOffscreen = async (message) => { h.sent.push(message); return { connected: true, snapshot: { phase: 'streaming' }, capturing: true, tabId: 42 }; };
  await h.background.bootstrap();
  assert.equal(h.background.tabId, 42); assert.deepEqual(h.sent.map((m) => m.type), ['managed-connect']);
});
