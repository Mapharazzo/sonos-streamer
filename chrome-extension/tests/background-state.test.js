import test from 'node:test';
import assert from 'node:assert/strict';

import {
  cleanupTransition,
  isSupportedMediaUrl,
  normalizePlexOrigin,
  validateCastRequest,
} from '../shared/background-state.js';

test('cast requires token, renderer, and supported active tab', () => {
  assert.deepEqual(
    validateCastRequest({ token: '', rendererId: '', url: 'https://youtube.com/watch?v=x' }),
    ['pairing-token', 'renderer'],
  );
  assert.deepEqual(
    validateCastRequest({ token: 'secret', rendererId: 'room', url: 'chrome://settings' }),
    ['unsupported-tab'],
  );
});

test('registered self-hosted Plex origins are supported exactly', () => {
  const registeredPlexOrigins = ['https://plex.example.test:32400'];
  assert.equal(
    isSupportedMediaUrl('https://plex.example.test:32400/web/index.html', registeredPlexOrigins),
    true,
  );
  assert.equal(isSupportedMediaUrl('https://evil.example.test/', registeredPlexOrigins), false);
});

test('Plex origin registration accepts only http or https origins', () => {
  assert.equal(normalizePlexOrigin('HTTPS://Media.Example.test:32400/web'), 'https://media.example.test:32400');
  assert.throws(() => normalizePlexOrigin('file:///media/index.html'), /http/i);
  assert.throws(() => normalizePlexOrigin('not a url'), /valid URL/i);
});

test('terminal capture event requests one idempotent cleanup', () => {
  const streamingState = { phase: 'streaming', cleanupPending: false };
  const first = cleanupTransition(streamingState, 'track-ended');
  const second = cleanupTransition(first.state, 'track-ended');

  assert.equal(first.shouldCleanup, true);
  assert.equal(first.state.phase, 'stopping');
  assert.equal(first.state.cleanupReason, 'track-ended');
  assert.equal(second.shouldCleanup, false);
});
