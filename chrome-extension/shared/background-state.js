const OFFICIAL_MEDIA_HOSTS = new Set(['youtube.com', 'www.youtube.com', 'app.plex.tv']);
const ACTIVE_CAPTURE_PHASES = new Set(['starting', 'streaming', 'calibrating']);

export function canStartCast(state) {
  return state.phase === 'idle' && !state.cleanupPending;
}

export function canChangeRenderer(state) {
  return !ACTIVE_CAPTURE_PHASES.has(state.phase) && state.phase !== 'stopping';
}

export function terminalEventReason(event) {
  if (event?.type === 'bridge-disconnected') return 'socket-lost';
  if (event?.type === 'stream-state' && ['ended', 'stopped'].includes(event.state)) {
    return 'renderer-ended';
  }
  if (event?.type === 'error') {
    if (event.terminal) return 'fatal-protocol-error';
    if (['cast-failed', 'renderer-not-found', 'stream-start-timeout'].includes(event.code)) {
      return 'cast-failed';
    }
  }
  return null;
}

export function isSupportedMediaUrl(value, registeredPlexOrigins = []) {
  let url;
  try {
    url = new URL(value);
  } catch {
    return false;
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return false;
  if (OFFICIAL_MEDIA_HOSTS.has(url.hostname.toLowerCase())) return true;
  return registeredPlexOrigins.includes(url.origin);
}

export function validateCastRequest({ token, rendererId, url, registeredPlexOrigins = [] }) {
  const errors = [];
  if (typeof token !== 'string' || token.trim().length === 0) errors.push('pairing-token');
  if (typeof rendererId !== 'string' || rendererId.length === 0) errors.push('renderer');
  if (!isSupportedMediaUrl(url, registeredPlexOrigins)) errors.push('unsupported-tab');
  return errors;
}

export function normalizePlexOrigin(value) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new TypeError('Enter a valid URL for the Plex server');
  }
  if (!['http:', 'https:'].includes(url.protocol)) {
    throw new TypeError('Plex server URLs must use http or https');
  }
  if (url.username || url.password) throw new TypeError('Plex server URLs cannot contain credentials');
  return url.origin;
}

export function cleanupTransition(state, reason) {
  if (state.cleanupPending || !ACTIVE_CAPTURE_PHASES.has(state.phase)) {
    return { state, shouldCleanup: false };
  }
  return {
    state: { ...state, phase: 'stopping', cleanupPending: true, cleanupReason: reason },
    shouldCleanup: true,
  };
}
