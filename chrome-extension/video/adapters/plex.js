import { createPlacement, findDominantVideo, observePlayerChanges } from './common.js';

let stopObserving = null;

export const plexAdapter = {
  matches(location) {
    if (!['http:', 'https:'].includes(location.protocol)) return false;
    return location.hostname === 'app.plex.tv' || location.pathname === '/web' || location.pathname.startsWith('/web/');
  },

  findVideo(document) {
    return findDominantVideo(document);
  },

  placement(video) {
    return createPlacement(
      video,
      '[data-testid*="player" i], [class*="Player"], [class*="player"]',
      '[data-testid*="control" i], [class*="PlayerControls"], [class*="player-controls"]',
    );
  },

  observe(callback, document = globalThis.document) {
    this.dispose();
    stopObserving = observePlayerChanges(document, callback);
    return stopObserving;
  },

  dispose() {
    stopObserving?.();
    stopObserving = null;
  },
};
