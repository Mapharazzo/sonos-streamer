import test from 'node:test';
import assert from 'node:assert/strict';

import { GenerationGate } from '../shared/generation-gate.js';

test('cancellation invalidates an in-flight asynchronous setup', () => {
  const gate = new GenerationGate();
  const first = gate.begin();

  gate.cancel();

  assert.equal(gate.isCurrent(first), false);
  const second = gate.begin();
  assert.equal(gate.isCurrent(second), true);
  assert.equal(gate.isCurrent(first), false);
});
