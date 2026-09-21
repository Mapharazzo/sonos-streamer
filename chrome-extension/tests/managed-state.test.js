import test from 'node:test';
import assert from 'node:assert/strict';
import { CaptureLifecycle } from '../shared/managed-state.js';
test('external release acknowledges only after audio and video are disposed', async () => {
  const calls = []; const life = new CaptureLifecycle({ disposeAudio: async () => calls.push('audio'), disposeVideo: async () => calls.push('video'), acknowledge: async (id) => calls.push(id) });
  await life.release('transition-2'); assert.deepEqual(calls, ['audio', 'video', 'transition-2']);
});
test('failed cleanup cannot acknowledge release', async () => {
  let acknowledged = false; const life = new CaptureLifecycle({ disposeAudio: async () => { throw new Error('track not stopped'); }, disposeVideo: async () => {}, acknowledge: async () => { acknowledged = true; } });
  await assert.rejects(life.release('t')); assert.equal(acknowledged, false);
});
test('competing popup actions are rejected and external Stop cancels delayed capture setup', async () => {
  let finish; const pending = new Promise((resolve) => { finish = resolve; });
  const life = new CaptureLifecycle({ disposeAudio: async () => {}, disposeVideo: async () => {}, acknowledge: async () => {} });
  const first = life.start(async (check) => { await pending; check(); throw new Error('must not start audio'); });
  await assert.rejects(life.start(async () => {}), /capture-busy/);
  await life.release('t'); finish(); await assert.rejects(first, /capture-cancelled/);
});
