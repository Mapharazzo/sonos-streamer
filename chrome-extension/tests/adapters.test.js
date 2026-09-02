import test from 'node:test';
import assert from 'node:assert/strict';

import { selectDominantVideo } from '../video/dominant-video.js';
import { plexAdapter } from '../video/adapters/plex.js';
import { youtubeAdapter } from '../video/adapters/youtube.js';

const viewport = { left: 0, top: 0, width: 1280, height: 720 };

test('dominant selection ignores paused thumbnails and chooses largest visible player', () => {
  const paused = fakeVideo({ width: 1200, height: 675, paused: true });
  const small = fakeVideo({ width: 200, height: 100, paused: false });
  const large = fakeVideo({ width: 1000, height: 600, paused: false });

  assert.equal(selectDominantVideo([paused, small, large], viewport), large);
});

test('dominant selection uses visible intersection and rejects offscreen video', () => {
  const offscreen = fakeVideo({ left: 1400, width: 1200, height: 700, paused: false });
  const visible = fakeVideo({ left: 50, top: 50, width: 640, height: 360, paused: false });

  assert.equal(selectDominantVideo([offscreen, visible], viewport), visible);
});

test('dominant selection keeps an active source hidden by its delay overlay', () => {
  const delayedSource = fakeVideo({ width: 960, height: 540, paused: false });
  delayedSource.style.opacity = '0';

  assert.equal(selectDominantVideo([delayedSource], viewport), delayedSource);
});

test('adapters recognize YouTube and hosted or self-hosted Plex', () => {
  assert.equal(youtubeAdapter.matches(new URL('https://www.youtube.com/watch?v=x')), true);
  assert.equal(youtubeAdapter.matches(new URL('https://example.com/watch?v=x')), false);
  assert.equal(plexAdapter.matches(new URL('https://app.plex.tv/desktop/')), true);
  assert.equal(plexAdapter.matches(new URL('http://plex.lan:32400/web/')), true);
  assert.equal(plexAdapter.matches(new URL('https://example.com/not-plex')), false);
});

test('site adapters delegate video choice to the dominant playing element', () => {
  const small = fakeVideo({ width: 320, height: 180, paused: false });
  const large = fakeVideo({ width: 960, height: 540, paused: false });
  const document = {
    defaultView: { innerWidth: 1280, innerHeight: 720 },
    querySelectorAll(selector) {
      assert.equal(selector, 'video');
      return [small, large];
    },
  };

  assert.equal(youtubeAdapter.findVideo(document), large);
  assert.equal(plexAdapter.findVideo(document), large);
});

function fakeVideo({ left = 0, top = 0, width, height, paused }) {
  return {
    paused,
    ended: false,
    hidden: false,
    readyState: 4,
    videoWidth: width,
    videoHeight: height,
    style: { display: '', visibility: '', opacity: '' },
    getBoundingClientRect() {
      return { left, top, right: left + width, bottom: top + height, width, height };
    },
  };
}
