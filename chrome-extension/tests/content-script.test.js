import test from 'node:test';
import assert from 'node:assert/strict';

let sequence = 0;
async function harness(t, sendMessage) {
  const previous = globalThis.chrome;
  let ready;
  const constructed = new Promise((resolve) => { ready = resolve; });
  const controller = { stops: 0, stop() { this.stops++; this.onStatus({ status: 'audio-only', code: 'stopped' }); } };
  globalThis.contentScriptTest = { controller, ready };
  globalThis.chrome = { runtime: {
    id: 'extension-id', sendMessage,
    getURL: () => `data:text/javascript,${encodeURIComponent('export class VideoDelayController { constructor({onStatus}) { const {controller, ready} = globalThis.contentScriptTest; controller.onStatus = onStatus; ready(); return controller; } }')}`,
    onMessage: { addListener() {} },
  } };
  t.after(() => { globalThis.chrome = previous; delete globalThis.contentScriptTest; });
  await import(`../content-script.js?content-script-test=${++sequence}`);
  await constructed;
  return controller;
}

test('invalidated extension context stops the old controller without throwing or recursive reports', async (t) => {
  let sends = 0;
  const controller = await harness(t, () => { sends++; throw new Error('Extension context invalidated.'); });
  assert.doesNotThrow(() => controller.onStatus({ status: 'active', delayMs: 513 }));
  await new Promise(setImmediate);
  assert.equal(controller.stops, 1);
  controller.onStatus({ status: 'active', delayMs: 513 });
  assert.equal(sends, 1);
  assert.equal(controller.stops, 1);
});

test('asynchronous invalidation also disposes the old controller', async (t) => {
  const controller = await harness(t, () => Promise.reject(new Error('Extension context invalidated.')));
  controller.onStatus({ status: 'active', delayMs: 513 });
  await new Promise(setImmediate);
  assert.equal(controller.stops, 1);
});

test('a transient missing receiver does not stop video delay', async (t) => {
  const controller = await harness(t, () => Promise.reject(new Error('Could not establish connection. Receiving end does not exist.')));
  controller.onStatus({ status: 'active', delayMs: 513 });
  await new Promise(setImmediate);
  assert.equal(controller.stops, 0);
});
