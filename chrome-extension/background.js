import { cleanupTransition, normalizePlexOrigin, validateCastRequest } from './shared/background-state.js';
import { initialSessionState, reduceSession } from './shared/session-state.js';

const STORED_FIELDS = ['pairingToken', 'selectedRendererId', 'measuredDelays', 'manualOffsets', 'plexOrigins'];
const ACTIVE_PHASES = new Set(['starting', 'streaming', 'calibrating']);

let session = { ...initialSessionState(), cleanupPending: false, cleanupReason: null };
let pairingToken = '';
let measuredDelays = {};
let manualOffsets = {};
let plexOrigins = [];
let offscreenCreation = null;
let requestSequence = 0;

const ready = restoreSettings();

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (!message || typeof message !== 'object') return undefined;
  if (message.type === 'offscreen-event') {
    void ready.then(() => handleOffscreenEvent(message.event));
    return undefined;
  }
  if (!['state-request', 'popup-command'].includes(message.type)) return undefined;

  ready.then(() => handlePopupMessage(message)).then(
    (value) => sendResponse({ ok: true, value }),
    (error) => sendResponse({ ok: false, error: publicError(error) }),
  );
  return true;
});

chrome.tabs.onRemoved.addListener((tabId) => {
  if (tabId === session.tabId) void stopCapture('tab-removed');
});

chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (tabId === session.tabId && changeInfo.status === 'loading') void stopCapture('tab-navigated');
});

chrome.tabCapture.onStatusChanged.addListener((info) => {
  if (info.tabId === session.tabId && ['stopped', 'error'].includes(info.status)) {
    void stopCapture(`capture-${info.status}`);
  }
});

async function restoreSettings() {
  const stored = await chrome.storage.local.get(STORED_FIELDS);
  pairingToken = typeof stored.pairingToken === 'string' ? stored.pairingToken : '';
  measuredDelays = isRecord(stored.measuredDelays) ? stored.measuredDelays : {};
  manualOffsets = isRecord(stored.manualOffsets) ? stored.manualOffsets : {};
  plexOrigins = Array.isArray(stored.plexOrigins) ? stored.plexOrigins : [];
  if (typeof stored.selectedRendererId === 'string') {
    session.selectedRendererId = stored.selectedRendererId;
    applyRendererSettings(stored.selectedRendererId);
  }
}

async function handlePopupMessage(message) {
  if (message.type === 'state-request') return publicState();
  const command = message.command;
  switch (command) {
    case 'pair':
      return pair(message.token);
    case 'select-renderer':
      return selectRenderer(message.rendererId);
    case 'cast':
      return castActiveTab();
    case 'stop':
      await stopCapture('user-stop');
      return publicState();
    case 'calibrate':
      await sendOffscreen({
        type: 'browser-command',
        commandType: 'calibrate',
        requestId: nextRequestId('calibrate'),
      });
      return publicState();
    case 'set-offset':
      await sendOffscreen({
        type: 'browser-command',
        commandType: 'set-manual-offset',
        requestId: nextRequestId('offset'),
        fields: { milliseconds: message.milliseconds },
      });
      return publicState();
    case 'register-plex-origin':
      await registerPlexOrigin(message.origin);
      return publicState();
    default:
      throw new Error('Unknown popup command');
  }
}

async function pair(candidateToken) {
  const token = typeof candidateToken === 'string' && candidateToken.trim()
    ? candidateToken.trim()
    : pairingToken;
  if (!token) throw new Error('Enter the pairing token shown by the local Sonos app');
  pairingToken = token;
  await chrome.storage.local.set({ pairingToken });
  await ensureOffscreenDocument();
  await sendOffscreen({ type: 'bridge-connect', token: pairingToken });
  return publicState();
}

async function selectRenderer(rendererId) {
  if (typeof rendererId !== 'string' || rendererId.length === 0) {
    throw new Error('Choose a Sonos renderer');
  }
  session = { ...session, selectedRendererId: rendererId };
  applyRendererSettings(rendererId);
  await chrome.storage.local.set({ selectedRendererId: rendererId });
  broadcastState();
  return publicState();
}

async function castActiveTab() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tab || !Number.isInteger(tab.id)) throw new Error('No active browser tab was found');
  const errors = validateCastRequest({
    token: pairingToken,
    rendererId: session.selectedRendererId ?? '',
    url: tab.url ?? '',
    registeredPlexOrigins: plexOrigins,
  });
  if (errors.length > 0) throw new Error(castValidationMessage(errors));
  if (!session.connected || !session.paired) throw new Error('Pair with the local Sonos app first');

  return startCapture(tab.id, session.selectedRendererId);
}

export async function ensureOffscreenDocument() {
  if (await chrome.offscreen.hasDocument()) return;
  if (!offscreenCreation) {
    offscreenCreation = chrome.offscreen.createDocument({
      url: 'offscreen.html',
      reasons: ['USER_MEDIA'],
      justification: 'Capture the selected tab audio and stream it to the local Sonos companion',
    }).finally(() => {
      offscreenCreation = null;
    });
  }
  await offscreenCreation;
}

export async function startCapture(tabId, rendererId) {
  if (!Number.isInteger(tabId) || typeof rendererId !== 'string' || rendererId.length === 0) {
    throw new TypeError('A tab and renderer are required');
  }
  session = {
    ...session,
    phase: 'starting',
    tabId,
    cleanupPending: false,
    cleanupReason: null,
    warning: null,
  };
  broadcastState();

  try {
    await ensureOffscreenDocument();
    const streamId = await chrome.tabCapture.getMediaStreamId({ targetTabId: tabId });
    await sendOffscreen({ type: 'start-capture', streamId });
    session = reduceSession(session, { type: 'capture-started', tabId });
    broadcastState();
    await sendOffscreen({
      type: 'browser-command',
      commandType: 'cast',
      requestId: nextRequestId('cast'),
      fields: { rendererId },
    });
    return publicState();
  } catch (error) {
    await stopCapture('cast-failed');
    throw error;
  }
}

export async function stopCapture(reason = 'stopped') {
  const transition = cleanupTransition(session, reason);
  session = transition.state;
  if (!transition.shouldCleanup) return publicState();
  broadcastState();

  try {
    if (session.connected && session.paired) {
      await sendOffscreen({
        type: 'browser-command',
        commandType: 'stop',
        requestId: nextRequestId('stop'),
      });
    }
  } catch {
    // Teardown must continue even if the bridge is already gone.
  }
  try {
    await sendOffscreen({ type: 'stop-session' });
  } catch {
    // The offscreen document may have already been destroyed or disconnected.
  }

  session = {
    ...session,
    phase: 'disconnected',
    connected: false,
    paired: false,
    tabId: null,
    cleanupPending: false,
    cleanupReason: reason,
  };
  broadcastState();
  return publicState();
}

export async function registerPlexOrigin(value) {
  const origin = normalizePlexOrigin(value);
  const matchPattern = `${origin}/*`;
  const granted = await chrome.permissions.request({ origins: [matchPattern] });
  if (!granted) throw new Error('Permission for that Plex origin was not granted');

  const id = `plex_${stableHash(origin)}`;
  const existing = await chrome.scripting.getRegisteredContentScripts({ ids: [id] });
  const definition = { id, matches: [matchPattern], js: ['content-script.js'], runAt: 'document_idle' };
  if (existing.length === 0) await chrome.scripting.registerContentScripts([definition]);
  else await chrome.scripting.updateContentScripts([definition]);

  if (!plexOrigins.includes(origin)) {
    plexOrigins = [...plexOrigins, origin];
    await chrome.storage.local.set({ plexOrigins });
  }
  broadcastState();
  return origin;
}

async function handleOffscreenEvent(event) {
  if (!event || typeof event !== 'object') return;
  if (event.type === 'capture-ended') {
    await stopCapture('track-ended');
    return;
  }

  const wasActive = ACTIVE_PHASES.has(session.phase);
  session = { ...reduceSession(session, event), cleanupPending: session.cleanupPending };

  if (event.type === 'hello-ok') {
    void sendOffscreen({
      type: 'browser-command',
      commandType: 'list-renderers',
      requestId: nextRequestId('renderers'),
    });
    void sendOffscreen({
      type: 'browser-command',
      commandType: 'get-state',
      requestId: nextRequestId('state'),
    });
  } else if (event.type === 'calibration-result' && event.rendererId) {
    measuredDelays = { ...measuredDelays, [event.rendererId]: event.milliseconds };
    await chrome.storage.local.set({ measuredDelays });
  } else if (event.type === 'manual-offset-set' && session.selectedRendererId) {
    manualOffsets = { ...manualOffsets, [session.selectedRendererId]: event.milliseconds };
    await chrome.storage.local.set({ manualOffsets });
  }

  if ((event.type === 'bridge-disconnected' || (event.type === 'error' && event.terminal)) && wasActive) {
    session = {
      ...session,
      phase: 'disconnected',
      connected: false,
      tabId: null,
      cleanupPending: false,
      cleanupReason: 'bridge-ended',
    };
  }
  broadcastState();
}

function applyRendererSettings(rendererId) {
  session = {
    ...session,
    measuredDelayMs: integerOrZero(measuredDelays[rendererId]),
    manualOffsetMs: integerOrZero(manualOffsets[rendererId]),
  };
}

async function sendOffscreen(message) {
  const response = await chrome.runtime.sendMessage(message);
  if (!response?.ok) throw new Error(response?.error ?? 'The audio engine did not respond');
  return response.value;
}

function publicState() {
  return {
    ...session,
    hasStoredToken: pairingToken.length > 0,
    plexOrigins: [...plexOrigins],
  };
}

function broadcastState() {
  void chrome.runtime.sendMessage({ type: 'state-update', state: publicState() }).catch(() => {});
}

function nextRequestId(prefix) {
  requestSequence = (requestSequence + 1) % Number.MAX_SAFE_INTEGER;
  return `${prefix}-${Date.now()}-${requestSequence}`;
}

function stableHash(value) {
  let hash = 2166136261;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return (hash >>> 0).toString(36);
}

function castValidationMessage(errors) {
  const messages = {
    'pairing-token': 'Enter the local app pairing token',
    renderer: 'Choose a Sonos renderer',
    'unsupported-tab': 'Open YouTube or Plex in the active tab',
  };
  return errors.map((error) => messages[error]).join('; ');
}

function integerOrZero(value) {
  return Number.isInteger(value) ? value : 0;
}

function isRecord(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function publicError(error) {
  return error instanceof Error ? error.message : 'Unexpected extension error';
}
