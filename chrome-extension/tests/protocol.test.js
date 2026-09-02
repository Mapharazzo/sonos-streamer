import test from 'node:test';
import assert from 'node:assert/strict';

import {
  BRIDGE_URL,
  createCommand,
  createHello,
  parseServerEvent,
} from '../shared/protocol.js';

test('hello exactly matches Rust v1 audio contract', () => {
  assert.equal(BRIDGE_URL, 'ws://127.0.0.1:5902/browser/v1');
  assert.deepEqual(createHello('abc'), {
    type: 'hello',
    version: 1,
    token: 'abc',
    audio: {
      sampleRate: 48000,
      channels: 2,
      sampleFormat: 's16le',
      framesPerPacket: 960,
    },
  });
});

test('commands preserve request IDs and camel-case payload fields', () => {
  assert.deepEqual(createCommand('cast', 'cast-1', { rendererId: 'living-room' }), {
    type: 'cast',
    requestId: 'cast-1',
    rendererId: 'living-room',
  });
});

test('server event parser validates renderer lists and rejects unknown events', () => {
  const event = parseServerEvent({
    type: 'renderer-list',
    requestId: 'list-1',
    renderers: [{ id: 'r1', name: 'Room', model: 'One', address: '192.0.2.1' }],
  });
  assert.equal(event.renderers[0].id, 'r1');
  assert.throws(() => parseServerEvent({ type: 'not-a-real-event' }), /unknown/i);
  assert.throws(
    () => parseServerEvent({ type: 'renderer-list', requestId: 'x', renderers: [{}] }),
    /renderer/i,
  );
});
