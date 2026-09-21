import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createManagedCommand, parseManagedEvent } from '../shared/managed-protocol.js';
import { reduceManagedState } from '../shared/managed-state.js';

export function snapshot(fields = {}) {
  return { type: 'state', instanceId: 'instance-1', revision: 1, generation: 1, source: 'chrome', phase: 'idle', rendererId: null, renderers: [], volume: null, measuredDelayMs: 0, manualOffsetMs: 0, errorCode: null, capabilities: { windowsAvailable: true, browserConnected: true, browserCanCast: false, browserCanReload: false, calibrationAvailable: false, shortcut: '' }, ...fields };
}
test('managed command matches the executable Rust fixture', async () => {
  const fixture = JSON.parse(await readFile(new URL('../../tests/fixtures/managed-select-source.json', import.meta.url)));
  assert.deepEqual(createManagedCommand(snapshot(), 'request-1', { type: 'select-source', source: 'windows' }), fixture);
  assert.throws(() => createManagedCommand(snapshot(), 'x', { type: 'set-volume', value: 101 }));
  assert.throws(() => createManagedCommand(snapshot(), 'x', { type: 'stop', unexpected: true }));
});
test('managed events reject unknown fields and unsafe revisions', () => {
  assert.deepEqual(parseManagedEvent(snapshot()), snapshot());
  for (const fields of [{ revision: Number.MAX_SAFE_INTEGER + 1 }, { source: 'fallback' }, { volume: 101 }, { token: 'leak' }]) assert.throws(() => parseManagedEvent(snapshot(fields)));
});
test('state reducer drops old revisions and accepts new instances only after hello', () => {
  let state = reduceManagedState(null, { type: 'hello-ok', version: 1, instanceId: 'instance-1', connectionId: 'connection' });
  state = reduceManagedState(state, snapshot({ revision: 8 }));
  assert.equal(reduceManagedState(state, snapshot({ revision: 7 })), state);
  assert.equal(reduceManagedState(state, snapshot({ instanceId: 'old-instance', revision: 9 })), state);
  state = reduceManagedState(state, { type: 'hello-ok', version: 1, instanceId: 'new-instance', connectionId: 'next' });
  assert.equal(state.snapshot, null);
  assert.equal(reduceManagedState(state, snapshot({ instanceId: 'new-instance' })).snapshot.revision, 1);
});
