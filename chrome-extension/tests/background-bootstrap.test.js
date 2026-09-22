import test from 'node:test';
import assert from 'node:assert/strict';

test('real worker startup protects storage, auto-connects, and rejects content-script commands', async () => {
  const id = 'a'.repeat(32), origin = `chrome-extension://${id}/`, token = 'b'.repeat(64);
  let listener;
  const events = [], messages = [], noopEvent = { addListener() {} };
  globalThis.chrome = {
    runtime: { id, getURL: (path) => origin+path, onMessage: { addListener(fn) { listener = fn; } }, onInstalled: noopEvent, onStartup: noopEvent,
      async sendNativeMessage() { events.push('native'); return { ok: true, value: { version: 1, managedProtocol: 1, installationId: 'fixture', controlUrl: 'ws://127.0.0.1:5902/control/v1', token } }; },
      async sendMessage(message) { messages.push(message); return { ok: true, value: { connected: true, snapshot: { phase: 'idle', renderers: [] } } }; } },
    storage: {
      local: { async setAccessLevel(value) { events.push(['local-access', value]); }, async get() { events.push('read'); return { pairingToken: 'old', connectionMode: 'managed' }; }, async remove(key) { events.push(['remove', key]); } },
      session: { async setAccessLevel(value) { events.push(['session-access', value]); }, async get() { return {}; } },
    },
    offscreen: { async hasDocument() { return true; } },
    tabs: { onRemoved: noopEvent, onUpdated: noopEvent, onActivated: noopEvent, async query() { return []; } },
    tabCapture: { onStatusChanged: noopEvent }, commands: { onCommand: noopEvent, async getAll() { return []; } },
  };
  await import('../background.js?bootstrap-test');
  await new Promise(setImmediate);
  assert.deepEqual(events.slice(0, 3), [['local-access', { accessLevel: 'TRUSTED_CONTEXTS' }], ['session-access', { accessLevel: 'TRUSTED_CONTEXTS' }], 'read']);
  assert.equal(messages.find((m) => m.type === 'managed-connect').token, token);
  assert.ok(events.some((e) => Array.isArray(e) && e[0] === 'remove' && e[1] === 'pairingToken'));
  for (const message of [{ type: 'popup-command', command: 'pair' }, { type: 'state-request' }, { type: 'managed-offscreen-event', event: { type: 'hello-ok' } }]) {
    assert.equal(listener(message, { id, tab: { id: 42 }, frameId: 0, url: 'https://www.youtube.com/watch?v=test' }, () => assert.fail('untrusted response')), undefined);
  }
  const response = await new Promise((resolve) => assert.equal(listener({ type: 'state-request' }, { id, url: origin+'popup.html' }, resolve), true));
  assert.equal(response.ok, true);
  assert.equal(JSON.stringify(response).includes(token), false);
  assert.equal(messages.some((m) => m.type === 'managed-start'), false);
});
