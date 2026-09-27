export const CONTROL_URL = 'ws://127.0.0.1:5902/control/v1';
export const AUDIO_URL = 'ws://127.0.0.1:5902/audio/v1';
export const AUDIO_FORMAT = Object.freeze({ sampleRate: 48000, channels: 2, sampleFormat: 's16le', framesPerPacket: 960 });
const sources = ['chrome', 'windows'];
const statuses = ['ok', 'rejected', 'failed'];
const actions = {
  'select-source': { source: (v) => member(v, sources) },
  'select-renderer': { rendererId: id }, 'start-windows': {}, 'prepare-browser': {},
  'start-browser': { leaseId: id }, stop: {}, 'set-volume': { value: (v) => integer(v, 0, 100) },
  calibrate: {}, 'set-offset': { milliseconds: (v) => integer(v, -5000, 5000) }, 'get-state': {},
  'capture-released': { transitionId: id }, 'reload-tab': {},
  'stop-for-reload': { actionId: id },
  'browser-status': { shortcut: (v) => string(v, 64), supportedTab: boolean, hasRecoveryTab: boolean },
  'browser-action-result': { actionId: id, status: (v) => member(v, statuses), code: (v) => string(v, 128) },
};
export function createManagedCommand(state, requestId, action) {
  id(state?.instanceId); integer(state.revision, 1); id(requestId);
  if (!Object.hasOwn(actions, action?.type)) throw new TypeError('Unknown managed action');
  exact(action, ['type', ...Object.keys(actions[action.type])], action.type === 'browser-status' ? ['videoSyncSupported'] : []);
  if (action.type === 'browser-status' && Object.hasOwn(action, 'videoSyncSupported')) boolean(action.videoSyncSupported);
  for (const [key, validate] of Object.entries(actions[action.type])) validate(action[key]);
  return { type: 'command', requestId, instanceId: state.instanceId, expectedRevision: state.revision, action: structuredClone(action) };
}
export function parseManagedEvent(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value) || JSON.stringify(value).length > 65536) throw new TypeError('Invalid managed event');
  switch (value.type) {
    case 'hello-ok':
      exact(value, ['type', 'version', 'instanceId', 'connectionId']);
      if (value.version !== 1) throw new TypeError('Unsupported version'); id(value.instanceId); id(value.connectionId); break;
    case 'state':
      exact(value, ['type', 'instanceId', 'revision', 'generation', 'source', 'phase', 'rendererId', 'renderers', 'volume', 'measuredDelayMs', 'manualOffsetMs', 'errorCode', 'capabilities']);
      id(value.instanceId); integer(value.revision, 1); integer(value.generation, 1); member(value.source, sources); member(value.phase, ['idle', 'starting', 'streaming', 'switching', 'failed']);
      if (value.rendererId !== null) id(value.rendererId);
      if (value.volume !== null) integer(value.volume, 0, 100);
      integer(value.measuredDelayMs, 0, 5000); integer(value.manualOffsetMs, -5000, 5000);
      if (value.errorCode !== null) string(value.errorCode, 128);
      if (!Array.isArray(value.renderers) || value.renderers.length > 128) throw new TypeError('Invalid room list');
      for (const room of value.renderers) { exact(room, ['id', 'name', 'model', 'address']); id(room.id); for (const key of ['name', 'model', 'address']) string(room[key], 1024); }
      exact(value.capabilities, ['windowsAvailable', 'browserConnected', 'browserCanCast', 'browserCanReload', 'calibrationAvailable', 'shortcut']);
      for (const [key, v] of Object.entries(value.capabilities)) { if (key === 'shortcut') string(v, 64); else boolean(v); } break;
    case 'result':
      exact(value, ['type', 'requestId', 'status', 'code', 'revision'], ['leaseId']);
      id(value.requestId); member(value.status, statuses); string(value.code, 128); integer(value.revision, 1); if (value.leaseId !== undefined) id(value.leaseId); break;
    case 'release-capture': exact(value, ['type', 'transitionId']); id(value.transitionId); break;
    case 'browser-action': exact(value, ['type', 'actionId', 'action']); id(value.actionId); member(value.action, ['reload-tab']); break;
    case 'error': exact(value, ['type', 'code'], ['terminal']); string(value.code, 128); if (value.terminal !== undefined) boolean(value.terminal); break;
    default: throw new TypeError('Unknown managed event');
  }
  return structuredClone(value);
}
function exact(value, required, optional = []) {
  if (!value || typeof value !== 'object' || Array.isArray(value) || required.some((key) => !Object.hasOwn(value, key)) || Object.keys(value).some((key) => !required.includes(key) && !optional.includes(key))) throw new TypeError('Invalid managed fields');
}
function string(v, max) { if (typeof v !== 'string' || new TextEncoder().encode(v).length > max || /[\u0000-\u001f\u007f]/.test(v)) throw new TypeError('Invalid managed string'); }
function id(v) { string(v, 128); if (!v) throw new TypeError('Empty managed id'); }
function integer(v, min, max = Number.MAX_SAFE_INTEGER) { if (!Number.isSafeInteger(v) || v < min || v > max) throw new TypeError('Invalid managed number'); }
function member(v, allowed) { if (!allowed.includes(v)) throw new TypeError('Invalid managed value'); }
function boolean(v) { if (typeof v !== 'boolean') throw new TypeError('Invalid managed boolean'); }
