import test from 'node:test';
import assert from 'node:assert/strict';

import { chooseFramePlan } from '../video/resource-budget.js';

test('resource plan downscales but never below 640x360', () => {
  const plan = chooseFramePlan({
    sourceWidth: 1920,
    sourceHeight: 1080,
    fps: 30,
    delayMs: 3000,
    maxBytes: 512 * 1024 ** 2,
  });

  assert.ok(plan.width <= 1920 && plan.height <= 1080);
  assert.ok(plan.width >= 640 && plan.height >= 360);
  assert.ok(plan.estimatedBytes <= 512 * 1024 ** 2);
  assert.equal(plan.frameCount, 92);
});

test('resource plan caps oversized sources at 1920x1080 proportionally', () => {
  const plan = chooseFramePlan({
    sourceWidth: 3840,
    sourceHeight: 2160,
    fps: 30,
    delayMs: 0,
    maxBytes: 64 * 1024 ** 2,
  });

  assert.deepEqual(
    { width: plan.width, height: plan.height, frameCount: plan.frameCount },
    { width: 1920, height: 1080, frameCount: 2 },
  );
});

test('negative delay clamps to zero and impossible budgets return null', () => {
  const zeroDelay = chooseFramePlan({
    sourceWidth: 1280,
    sourceHeight: 720,
    fps: 60,
    delayMs: -500,
    maxBytes: 16 * 1024 ** 2,
  });
  const impossible = chooseFramePlan({
    sourceWidth: 1920,
    sourceHeight: 1080,
    fps: 60,
    delayMs: 5000,
    maxBytes: 8 * 1024 ** 2,
  });

  assert.equal(zeroDelay.frameCount, 2);
  assert.equal(impossible, null);
});

test('invalid dimensions and frame rates return null', () => {
  assert.equal(
    chooseFramePlan({ sourceWidth: 0, sourceHeight: 1080, fps: 30, delayMs: 1, maxBytes: 1e9 }),
    null,
  );
  assert.equal(
    chooseFramePlan({ sourceWidth: 1920, sourceHeight: 1080, fps: 0, delayMs: 1, maxBytes: 1e9 }),
    null,
  );
});
