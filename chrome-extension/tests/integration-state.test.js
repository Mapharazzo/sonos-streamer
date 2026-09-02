import test from 'node:test';
import assert from 'node:assert/strict';

import {
  applyCalibration,
  effectiveDelay,
  reconcileCapturedTab,
  restoreOffscreenSession,
  terminalCleanup,
} from '../shared/session-state.js';

test('calibration result persists raw measurement and sends clamped effective delay', () => {
  const result = applyCalibration(streamingState({ manualOffsetMs: -100 }), { latencyMs: 1800 });

  assert.equal(result.state.measuredDelayMs, 1800);
  assert.deepEqual(result.effects, [{ type: 'send-video-delay', delayMs: 1700 }]);
});

test('effective delay clamps both ends without changing raw values', () => {
  assert.equal(effectiveDelay(100, -500), 0);
  assert.equal(effectiveDelay(4900, 900), 5000);
  assert.equal(effectiveDelay(250, -25), 225);
});

test('socket loss stops renderer intent, capture, and overlay exactly once', () => {
  const result = terminalCleanup(streamingState(), 'socket-lost');

  assert.deepEqual(
    result.effects.map((effect) => effect.type),
    ['best-effort-stop-renderer', 'stop-media-tracks', 'dispose-audio', 'dispose-video'],
  );
  assert.equal(result.state.phase, 'stopping');
  assert.equal(terminalCleanup(result.state, 'socket-lost').effects.length, 0);
});

test('cleanup from an inactive session has no effects', () => {
  const result = terminalCleanup({ ...streamingState(), phase: 'disconnected' }, 'socket-lost');
  assert.deepEqual(result.effects, []);
});

test('service-worker restart restores an active offscreen capture without exposing a token', () => {
  const restored = restoreOffscreenSession(
    { ...streamingState(), phase: 'disconnected', connected: false, paired: false, tabId: null },
    { connected: true, authenticated: true, capturing: true, tabId: 73 },
  );

  assert.equal(restored.phase, 'streaming');
  assert.equal(restored.connected, true);
  assert.equal(restored.paired, true);
  assert.equal(restored.tabId, 73);
  assert.equal('token' in restored, false);
});

test('worker restart trusts only a tab Chrome still reports as captured', () => {
  const snapshot = { connected: true, authenticated: true, capturing: true, tabId: 42 };

  assert.equal(reconcileCapturedTab(snapshot, [{ tabId: 42, status: 'active' }]), snapshot);
  assert.deepEqual(reconcileCapturedTab(snapshot, [{ tabId: 42, status: 'stopped' }]), {
    connected: true,
    authenticated: true,
    capturing: false,
    tabId: null,
  });
});

function streamingState(fields = {}) {
  return {
    phase: 'streaming',
    connected: true,
    paired: true,
    tabId: 42,
    selectedRendererId: 'room',
    measuredDelayMs: 0,
    manualOffsetMs: 0,
    cleanupPending: false,
    ...fields,
  };
}
