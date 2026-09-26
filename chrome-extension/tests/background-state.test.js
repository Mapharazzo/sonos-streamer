import test from 'node:test';
import assert from 'node:assert/strict';

import {
  canChangeRenderer,
  canStartCast,
  cleanupTransition,
  isSupportedMediaUrl,
  normalizePlexOrigin,
  terminalEventReason,
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

test('ordinary web tabs can cast audio without becoming video-delay sites', () => {
  for (const url of ['https://example.org/music', 'http://127.0.0.1:58503/']) {
    assert.deepEqual(validateCastRequest({ token: 'secret', rendererId: 'room', url }), []);
    assert.equal(isSupportedMediaUrl(url), false);
  }
  for (const url of ['chrome://settings', 'chrome-extension://id/page.html', 'file:///audio.html', 'https://chromewebstore.google.com/detail/test', 'https://chrome.google.com/webstore/detail/test']) {
    assert.deepEqual(validateCastRequest({ token: 'secret', rendererId: 'room', url }), ['unsupported-tab']);
  }
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

test('cast and renderer changes are guarded by session phase', () => {
  assert.equal(canStartCast({ phase: 'idle', cleanupPending: false }), true);
  assert.equal(canStartCast({ phase: 'starting', cleanupPending: false }), false);
  assert.equal(canStartCast({ phase: 'idle', cleanupPending: true }), false);
  assert.equal(canChangeRenderer({ phase: 'streaming' }), false);
  assert.equal(canChangeRenderer({ phase: 'idle' }), true);
});

test('renderer lookup and play failures terminate only the active cast attempt', () => {
  assert.equal(terminalEventReason({ type: 'error', code: 'renderer-not-found', terminal: false }), 'cast-failed');
  assert.equal(terminalEventReason({ type: 'error', code: 'cast-failed', terminal: false }), 'cast-failed');
  assert.equal(terminalEventReason({ type: 'warning', code: 'audio-backpressure' }), null);
});

test('stream startup timeout terminates the active cast attempt', () => {
  assert.equal(
    terminalEventReason({ type: 'error', code: 'stream-start-timeout', terminal: false }),
    'cast-failed',
  );
});
