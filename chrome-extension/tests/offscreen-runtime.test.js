import test from 'node:test';
import assert from 'node:assert/strict';

class FakeWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;

  static instance = null;

  constructor(url) {
    this.url = url;
    this.readyState = FakeWebSocket.CONNECTING;
    this.listeners = new Map();
    this.sent = [];
    FakeWebSocket.instance = this;
    queueMicrotask(() => {
      this.readyState = FakeWebSocket.OPEN;
      this.emit('open', {});
    });
  }

  addEventListener(type, listener, options = {}) {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push({ listener, once: options.once === true });
    this.listeners.set(type, listeners);
  }

  emit(type, event) {
    const listeners = this.listeners.get(type) ?? [];
    this.listeners.set(type, listeners.filter(({ once }) => !once));
    for (const { listener } of listeners) listener(event);
  }

  send(payload) {
    this.sent.push(JSON.parse(payload));
  }

  close() {
    this.readyState = FakeWebSocket.CLOSED;
  }
}

function sendRuntimeMessage(listener, message) {
  return new Promise((resolve, reject) => {
    try {
      assert.equal(listener(message, {}, resolve), true);
    } catch (error) {
      reject(error);
    }
  });
}

test('browser commands complete their runtime response after being sent', async () => {
  let runtimeListener;
  globalThis.WebSocket = FakeWebSocket;
  globalThis.chrome = {
    runtime: {
      onMessage: {
        addListener(listener) {
          runtimeListener = listener;
        },
      },
      sendMessage() {
        return Promise.resolve();
      },
    },
  };

  await import('../offscreen.js?offscreen-runtime-test');
  assert.equal(typeof runtimeListener, 'function');

  assert.deepEqual(
    await sendRuntimeMessage(runtimeListener, { type: 'bridge-connect', token: 'pairing-token' }),
    { ok: true, value: undefined },
  );
  FakeWebSocket.instance.emit('message', {
    data: JSON.stringify({ type: 'hello-ok', version: 1 }),
  });

  assert.deepEqual(
    await sendRuntimeMessage(runtimeListener, {
      type: 'browser-command',
      commandType: 'cast',
      requestId: 'cast-1',
      fields: { rendererId: 'living-room' },
    }),
    { ok: true, value: undefined },
  );
  assert.deepEqual(FakeWebSocket.instance.sent.at(-1), {
    type: 'cast',
    requestId: 'cast-1',
    rendererId: 'living-room',
  });
});
