import test from 'node:test';
import assert from 'node:assert/strict';

import { VideoDelayController } from '../video/controller.js';

test('pause keeps the active delayed surface so queued frames can drain', async () => {
  const video = { ended: false, isConnected: true };
  let selectedVideo = video;
  let observerCallback;
  let disposeCount = 0;
  const adapter = {
    matches: () => true,
    findVideo: () => selectedVideo,
    placement: () => ({ container: {} }),
    observe(callback) { observerCallback = callback; },
    dispose() {},
  };
  class Surface {
    start() {}
    setDelay() {}
    dispose() { disposeCount += 1; }
  }
  const controller = new VideoDelayController({
    document: {},
    location: { href: 'https://example.test/watch' },
    adapters: [adapter],
    Surface,
  });
  controller.setDelay(400);
  selectedVideo = null;
  observerCallback();
  await Promise.resolve();

  assert.equal(disposeCount, 0);
  assert.equal(controller.getStatus().status, 'active');
});

test('a surface fallback is not retried for the same media element', async () => {
  const video = { ended: false, isConnected: true };
  let observerCallback;
  let surfaceCount = 0;
  const adapter = {
    matches: () => true,
    findVideo: () => video,
    placement: () => ({ container: {} }),
    observe(callback) { observerCallback = callback; },
    dispose() {},
  };
  class FailingSurface {
    constructor(_video, _placement, options) {
      this.options = options;
      surfaceCount += 1;
    }
    start() { this.options.onFallback({ code: 'drm', message: 'blocked' }); }
    dispose() {}
  }
  const controller = new VideoDelayController({
    document: {},
    location: { href: 'https://example.test/watch' },
    adapters: [adapter],
    Surface: FailingSurface,
  });
  controller.setDelay(400);
  observerCallback();
  await Promise.resolve();

  assert.equal(surfaceCount, 1);
  assert.equal(controller.getStatus().status, 'audio-only');
});
