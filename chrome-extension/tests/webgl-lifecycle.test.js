import test from 'node:test';
import assert from 'node:assert/strict';

import { WebGlDelaySurface } from '../video/webgl-delay.js';

test('source video is hidden only after first successful captured texture', () => {
  const fake = makeSurfaceHarness();
  fake.surface.start(800);

  assert.equal(fake.video.style.opacity, '1');
  fake.deliverFrame(0);

  assert.equal(fake.video.style.opacity, '0');
  assert.equal(fake.canvases.length, 1);
  assert.equal(fake.rendered.length, 1);
});

test('allocation failure restores exact original inline styles', () => {
  const fake = makeSurfaceHarness({ failAllocation: true, originalOpacity: '0.7' });
  fake.surface.start(800);
  fake.deliverFrame(0);

  assert.equal(fake.video.style.opacity, '0.7');
  assert.equal(fake.video.style.visibility, 'collapse');
  assert.equal(fake.canvases.length, 0);
  assert.equal(fake.fallbacks[0].code, 'capture-failed');
});

test('dispose is idempotent and releases queued textures once', () => {
  const fake = makeSurfaceHarness();
  fake.surface.start(500);
  fake.deliverFrame(0);

  fake.surface.dispose();
  fake.surface.dispose();

  assert.deepEqual(fake.released, [1]);
  assert.equal(fake.video.style.opacity, '1');
  assert.equal(fake.canvases.length, 0);
});

test('context loss falls back to the source video', () => {
  const fake = makeSurfaceHarness();
  fake.surface.start(500);
  fake.deliverFrame(0);

  fake.loseContext();

  assert.equal(fake.video.style.opacity, '1');
  assert.equal(fake.canvases.length, 0);
  assert.equal(fake.fallbacks.at(-1).code, 'webgl-context-lost');
});

test('changing delay preserves the compatible GPU pipeline and queued textures', () => {
  const fake = makeSurfaceHarness();
  fake.surface.start(500);
  fake.deliverFrame(0);
  fake.deliverFrame(40);

  fake.surface.setDelay(800);
  fake.deliverFrame(80);

  assert.equal(fake.gpuCreations(), 1);
  assert.deepEqual(fake.released, []);
});

for (const fps of [24, 25, 29.97, 30, 50, 59.94, 60]) {
  test(`delay preserves ${fps} fps despite callback timing jitter`, () => {
    const fake = makeSurfaceHarness();
    fake.surface.start(800);
    const frames = Math.round(fps * 3);
    for (let frame = 0; frame < frames; frame++) {
      const displayAt = frame * 1000 / fps;
      fake.deliverFrame(displayAt + (frame % 2 ? -1 : 1), displayAt);
      fake.deliverAnimation(displayAt + 2);
    }
    assert.equal(fake.captured(), frames);
    assert.deepEqual(fake.fallbacks, []);
    fake.surface.dispose();
    assert.equal(new Set(fake.released).size, fake.captured());
    assert.equal(fake.released.length, fake.captured());
  });
}

test('high refresh sources stay within the 60 fps frame budget', () => {
  const fake = makeSurfaceHarness();
  fake.surface.start(800);
  for (let frame = 0; frame < 120 * 3; frame++) {
    fake.deliverFrame(frame * 1000 / 120);
    fake.deliverAnimation(frame * 1000 / 120);
  }
  assert.equal(fake.captured(), 180);
  assert.deepEqual(fake.fallbacks, []);
  fake.surface.dispose();
});

test('timestamp fallback preserves 60 fps with callback jitter', () => {
  const fake = makeSurfaceHarness();
  fake.surface.start(800);
  for (let frame = 0; frame < 180; frame++) {
    const now = frame * 1000 / 60 + (frame % 2 ? -1 : 1);
    fake.deliverFrame(now, null);
    fake.deliverAnimation(now + 2);
  }
  assert.equal(fake.captured(), 180);
  assert.deepEqual(fake.fallbacks, []);
  fake.surface.dispose();
});

test('five-second delay retains and presents 60 fps within its texture budget', () => {
  const fake = makeSurfaceHarness();
  fake.surface.start(5000);
  for (let frame = 0; frame < 420; frame++) {
    const now = frame * 1000 / 60;
    fake.deliverFrame(now);
    fake.deliverAnimation(now + 1);
  }
  assert.equal(fake.captured(), 420);
  assert.deepEqual(fake.fallbacks, []);
  assert.equal(fake.gpuCreations(), 1);
  assert.deepEqual(fake.rendered.slice(-60), Array.from({length:60},(_,i)=>i+61));
  fake.surface.dispose();
  assert.equal(fake.released.length, 420);
  assert.equal(new Set(fake.released).size, 420);
});

function makeSurfaceHarness({ failAllocation = false, originalOpacity = '1' } = {}) {
  const pendingFrames = new Map();
  const pendingAnimationFrames = new Map();
  const canvases = [];
  const released = [];
  const rendered = [];
  const fallbacks = [];
  let callbackId = 0;
  let textureId = 0;
  let gpuCreations = 0;

  const video = {
    videoWidth: 1280,
    videoHeight: 720,
    style: { opacity: originalOpacity, visibility: 'collapse', position: '', zIndex: '' },
    requestVideoFrameCallback(callback) {
      const id = ++callbackId;
      pendingFrames.set(id, callback);
      return id;
    },
    cancelVideoFrameCallback(id) {
      pendingFrames.delete(id);
    },
  };
  const container = {
    append(canvas) {
      canvases.push(canvas);
      canvas.parentNode = container;
    },
    removeChild(canvas) {
      const index = canvases.indexOf(canvas);
      if (index >= 0) canvases.splice(index, 1);
      canvas.parentNode = null;
    },
  };
  let contextLossHandler = null;
  const createCanvas = () => ({
    style: {},
    parentNode: null,
    width: 0,
    height: 0,
    addEventListener(type, handler) {
      if (type === 'webglcontextlost') contextLossHandler = handler;
    },
    removeEventListener(type, handler) {
      if (type === 'webglcontextlost' && contextLossHandler === handler) contextLossHandler = null;
    },
  });

  const surface = new WebGlDelaySurface(video, { container }, {
    createCanvas,
    createGpu(_canvas, plan) {
      gpuCreations += 1;
      if (failAllocation) throw new Error('fake allocation failure');
      return {
        capture() {
          const id = ++textureId;
          return { id, bytes: plan.bytesPerFrame, release: () => released.push(id) };
        },
        render(texture) {
          rendered.push(texture.id);
        },
        dispose() {},
      };
    },
    requestAnimationFrame(callback) {
      const id = ++callbackId;
      pendingAnimationFrames.set(id, callback);
      return id;
    },
    cancelAnimationFrame(id) {
      pendingAnimationFrames.delete(id);
    },
    onFallback(event) {
      fallbacks.push(event);
    },
  });

  return {
    surface,
    video,
    canvases,
    released,
    rendered,
    fallbacks,
    gpuCreations: () => gpuCreations,
    captured: () => textureId,
    deliverFrame(now, expectedDisplayTime = now) {
      const [id, callback] = pendingFrames.entries().next().value;
      pendingFrames.delete(id);
      callback(now, { mediaTime: expectedDisplayTime / 1000, expectedDisplayTime });
    },
    deliverAnimation(now) {
      const [id, callback] = pendingAnimationFrames.entries().next().value;
      pendingAnimationFrames.delete(id);
      callback(now);
    },
    loseContext() {
      contextLossHandler?.({ preventDefault() {} });
    },
  };
}
