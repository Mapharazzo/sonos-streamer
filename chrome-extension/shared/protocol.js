export const BRIDGE_URL = 'ws://127.0.0.1:5902/browser/v1';
export const PROTOCOL_VERSION = 1;

const COMMAND_TYPES = new Set([
  'list-renderers',
  'cast',
  'stop',
  'calibrate',
  'set-manual-offset',
  'get-state',
]);

const SERVER_EVENT_TYPES = new Set([
  'hello-ok',
  'renderer-list',
  'cast-state',
  'stream-state',
  'calibration-started',
  'calibration-result',
  'calibration-failed',
  'audio-stats',
  'manual-offset-set',
  'warning',
  'error',
]);

export function createHello(token) {
  requireString(token, 'token', 4096, true);
  return {
    type: 'hello',
    version: PROTOCOL_VERSION,
    token,
    audio: {
      sampleRate: 48000,
      channels: 2,
      sampleFormat: 's16le',
      framesPerPacket: 960,
    },
  };
}

export function createCommand(type, requestId, fields = {}) {
  if (!COMMAND_TYPES.has(type)) {
    throw new TypeError(`unknown command type: ${type}`);
  }
  requireString(requestId, 'requestId', 128);
  if (!isRecord(fields)) {
    throw new TypeError('command fields must be an object');
  }
  if (type === 'set-manual-offset') {
    const value = fields.milliseconds;
    if (!Number.isInteger(value) || value < -5000 || value > 5000) {
      throw new RangeError('manual offset must be an integer between -5000 and 5000');
    }
  }
  return { type, requestId, ...fields };
}

export function parseServerEvent(value) {
  if (!isRecord(value)) {
    throw new TypeError('server event must be an object');
  }
  const type = requireString(value.type, 'event type', 64);
  if (!SERVER_EVENT_TYPES.has(type)) {
    throw new TypeError(`unknown server event: ${type}`);
  }
  if (value.version !== undefined && value.version !== PROTOCOL_VERSION) {
    throw new TypeError(`unsupported protocol version: ${value.version}`);
  }

  if (value.requestId !== undefined && value.requestId !== null) {
    requireString(value.requestId, 'requestId', 128);
  }
  if (type === 'renderer-list') {
    if (!Array.isArray(value.renderers) || value.renderers.length > 256) {
      throw new TypeError('renderer list must be a bounded array');
    }
    value.renderers.forEach(validateRenderer);
  }
  if (type === 'calibration-result') {
    requireInteger(value.milliseconds, 'calibration milliseconds', 0, 0xffffffff);
  }
  if (type === 'manual-offset-set') {
    requireInteger(value.milliseconds, 'manual offset', -5000, 5000);
  }
  validateBoundedStrings(value);
  return structuredClone(value);
}

function validateRenderer(renderer) {
  if (!isRecord(renderer)) {
    throw new TypeError('renderer must be an object');
  }
  for (const field of ['id', 'name', 'model', 'address']) {
    requireString(renderer[field], `renderer ${field}`, 1024);
  }
}

function validateBoundedStrings(value, depth = 0) {
  if (depth > 4) {
    throw new TypeError('server event is nested too deeply');
  }
  if (typeof value === 'string') {
    if (value.length > 4096) throw new TypeError('server event string is too long');
    return;
  }
  if (Array.isArray(value)) {
    if (value.length > 256) throw new TypeError('server event array is too long');
    value.forEach((entry) => validateBoundedStrings(entry, depth + 1));
    return;
  }
  if (isRecord(value)) {
    for (const entry of Object.values(value)) validateBoundedStrings(entry, depth + 1);
  }
}

function requireString(value, name, maxLength, allowEmpty = false) {
  if (typeof value !== 'string' || (!allowEmpty && value.length === 0) || value.length > maxLength) {
    throw new TypeError(`${name} must be a bounded string`);
  }
  return value;
}

function requireInteger(value, name, minimum, maximum) {
  if (!Number.isInteger(value) || value < minimum || value > maximum) {
    throw new TypeError(`${name} is out of range`);
  }
}

function isRecord(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
