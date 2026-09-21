import test from 'node:test';
import assert from 'node:assert/strict';
import { reloadSelectedTab } from '../shared/recovery.js';
function options(fields = {}) {
  const calls = [];
  return { calls, tabId: 42, expectedOrigin: 'https://www.youtube.com', tabs: { get: async () => ({ id: 42, url: 'https://www.youtube.com/watch?v=x' }), reload: async (id) => calls.push(`reload-${id}`) }, stopCapture: async () => calls.push('stop'), waitReady: async () => calls.push('ready'), restorePlayback: async () => calls.push('restore'), ...fields };
}
test('recovery stops capture before exactly one reload and requires fresh invocation', async () => {
  const o = options(); const result = await reloadSelectedTab(o);
  assert.deepEqual(o.calls, ['stop', 'reload-42', 'ready', 'restore']); assert.equal(result.status, 'invoke-extension');
});
test('navigation to another origin cancels restoration', async () => {
  const o = options(); let gets = 0; o.tabs.get = async () => ({ id: 42, url: ++gets === 1 ? 'https://www.youtube.com/watch' : 'https://elsewhere.test/' });
  await assert.rejects(reloadSelectedTab(o), /tab-changed/); assert.equal(o.calls.includes('restore'), false);
});
test('timeout stays stopped and never retries reload', async () => {
  const o = options({ timeoutMs: 20, waitReady: () => new Promise(() => {}) });
  await assert.rejects(reloadSelectedTab(o), /reload-timeout/); assert.deepEqual(o.calls, ['stop', 'reload-42']);
});
test('position restore failure does not create a reload loop', async () => {
  const o = options({ restorePlayback: async () => { throw new Error('live video'); } });
  assert.equal((await reloadSelectedTab(o)).status, 'invoke-extension'); assert.equal(o.calls.filter((call) => call.startsWith('reload')).length, 1);
});
test('closed tab is rejected before any reload', async () => {
  const o = options(); o.tabs.get = async () => { throw new Error('tab-closed'); };
  await assert.rejects(reloadSelectedTab(o), /tab-closed/); assert.equal(o.calls.some((call) => call.startsWith('reload')), false);
});
