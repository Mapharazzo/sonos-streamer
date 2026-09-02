import test from 'node:test';
import assert from 'node:assert/strict';

import { TimedFrameQueue } from '../video/frame-queue.js';

test('delay reduction retires stale frames and releases superseded resources', () => {
  const released = [];
  const queue = new TimedFrameQueue({
    release: (resource) => released.push(resource.id),
    maxFrames: 10,
    maxBytes: 1000,
  });
  queue.enqueue({ id: 1 }, 100, 300, 10);
  queue.enqueue({ id: 2 }, 200, 300, 10);

  queue.setDelay(100);

  assert.equal(queue.takeDue(350).id, 2);
  assert.deepEqual(released, [1]);
  assert.equal(queue.length, 0);
  assert.equal(queue.bytes, 0);
});

test('zero and negative delay are immediately due', () => {
  const queue = new TimedFrameQueue({ release() {}, maxFrames: 3, maxBytes: 30 });
  queue.enqueue({ id: 1 }, 100, -20, 10);

  assert.equal(queue.takeDue(99), null);
  assert.equal(queue.takeDue(100).id, 1);
});

test('frame and byte caps evict oldest resources', () => {
  const released = [];
  const queue = new TimedFrameQueue({
    release: (resource) => released.push(resource.id),
    maxFrames: 2,
    maxBytes: 10,
  });
  queue.enqueue({ id: 1 }, 0, 100, 5);
  queue.enqueue({ id: 2 }, 1, 100, 5);
  queue.enqueue({ id: 3 }, 2, 100, 8);

  assert.deepEqual(released, [1, 2]);
  assert.equal(queue.length, 1);
  assert.equal(queue.bytes, 8);
  assert.equal(queue.takeDue(102).id, 3);
});

test('clear is idempotent and does not release transferred frames', () => {
  const releases = new Map();
  const queue = new TimedFrameQueue({
    release: (resource) => releases.set(resource.id, (releases.get(resource.id) ?? 0) + 1),
    maxFrames: 4,
    maxBytes: 40,
  });
  queue.enqueue({ id: 1 }, 0, 0, 10);
  assert.equal(queue.takeDue(0).id, 1);
  queue.enqueue({ id: 2 }, 1, 100, 10);
  queue.clear();
  queue.clear();

  assert.deepEqual([...releases], [[2, 1]]);
});
