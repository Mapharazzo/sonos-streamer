import test from 'node:test';
import assert from 'node:assert/strict';

import { webSocketCloseReason } from '../shared/diagnostics.js';

test('cleanup reason becomes a bounded websocket close reason', () => {
  assert.equal(webSocketCloseReason('capture-stopped'), 'session: capture-stopped');
  assert.equal(webSocketCloseReason('bad\nreason'), 'session: bad reason');
  assert.ok(new TextEncoder().encode(webSocketCloseReason('x'.repeat(500))).byteLength <= 123);
});
