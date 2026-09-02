import test from 'node:test';
import assert from 'node:assert/strict';

import { MAX_AUDIO_BACKLOG_BYTES, shouldDropAudio } from '../shared/pcm.js';

test('high websocket backlog drops new live input', () => {
  assert.equal(shouldDropAudio(256 * 1024, 3840), true);
  assert.equal(shouldDropAudio(0, 3840), false);
});

test('packet crossing the backlog limit is dropped', () => {
  assert.equal(shouldDropAudio(MAX_AUDIO_BACKLOG_BYTES - 3839, 3840), true);
  assert.equal(shouldDropAudio(MAX_AUDIO_BACKLOG_BYTES - 3840, 3840), false);
});

test('invalid backlog values fail closed', () => {
  assert.equal(shouldDropAudio(Number.NaN, 3840), true);
  assert.equal(shouldDropAudio(0, -1), true);
});
