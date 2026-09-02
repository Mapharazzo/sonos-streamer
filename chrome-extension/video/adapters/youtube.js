import { createPlacement, findDominantVideo, observePlayerChanges } from './common.js';

let stopObserving = null;

export const youtubeAdapter = {
  matches(location) {
    return location.protocol === 'https:' && ['youtube.com', 'www.youtube.com'].includes(location.hostname);
  },

  findVideo(document) {
    return findDominantVideo(document);
  },

  placement(video) {
    return createPlacement(
      video,
      '#movie_player, .html5-video-player',
      '.ytp-chrome-bottom, .ytp-chrome-top, .ytp-popup, .ytp-gradient-bottom',
    );
  },

  observe(callback, document = globalThis.document) {
    this.dispose();
    stopObserving = observePlayerChanges(document, callback, ['yt-navigate-finish']);
    return stopObserving;
  },

  dispose() {
    stopObserving?.();
    stopObserving = null;
  },
};
