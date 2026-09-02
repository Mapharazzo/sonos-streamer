import test from 'node:test';
import assert from 'node:assert/strict';

import { initialSessionState, reduceSession } from '../shared/session-state.js';

test('disconnect resets active cast but keeps pairing state', () => {
  const active = {
    ...initialSessionState(),
    paired: true,
    connected: true,
    phase: 'streaming',
    tabId: 42,
    selectedRendererId: 'r1',
  };

  const next = reduceSession(active, { type: 'bridge-disconnected' });

  assert.equal(next.phase, 'disconnected');
  assert.equal(next.tabId, null);
  assert.equal(next.connected, false);
  assert.equal(next.paired, true);
  assert.equal(next.selectedRendererId, 'r1');
});

test('calibration result records measured delay without losing manual offset', () => {
  const state = { ...initialSessionState(), manualOffsetMs: -75 };

  const next = reduceSession(state, {
    type: 'calibration-result',
    rendererId: 'r1',
    milliseconds: 320,
  });

  assert.equal(next.measuredDelayMs, 320);
  assert.equal(next.manualOffsetMs, -75);
});
