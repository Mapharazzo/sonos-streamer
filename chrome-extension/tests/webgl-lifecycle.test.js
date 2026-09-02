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
    createGpu() {
      gpuCreations += 1;
      if (failAllocation) throw new Error('fake allocation failure');
      return {
        capture() {
          const id = ++textureId;
          return { id, bytes: 1280 * 720 * 4, release: () => released.push(id) };
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
    deliverFrame(now) {
      const [id, callback] = pendingFrames.entries().next().value;
      pendingFrames.delete(id);
      callback(now, { mediaTime: now / 1000 });
    },
    loseContext() {
      contextLossHandler?.({ preventDefault() {} });
    },
  };
}
