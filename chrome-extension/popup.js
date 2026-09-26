import { effectiveDelay } from './shared/session-state.js';
import { allowedBackgroundSender } from './shared/message-senders.js';

const elements = {
  reload: document.querySelector('#reload'),
  shortcut: document.querySelector('#shortcut-status'),
  mode: document.querySelector('#mode'),
  source: document.querySelector('#source'),
  volume: document.querySelector('#volume'),
  volumeValue: document.querySelector('#volume-value'),
  volumeControls: document.querySelector('#volume-controls'),
  status: document.querySelector('#status'),
  token: document.querySelector('#token'),
  tokenLabel: document.querySelector('#token-label'),
  setupHint: document.querySelector('#setup-hint'),
  pair: document.querySelector('#pair'),
  renderer: document.querySelector('#renderer'),
  cast: document.querySelector('#cast'),
  stop: document.querySelector('#stop'),
  packets: document.querySelector('#packets'),
  measured: document.querySelector('#measured'),
  videoDelay: document.querySelector('#video-delay'),
  calibrationHint: document.querySelector('#calibration-hint'),
  offset: document.querySelector('#offset'),
  applyOffset: document.querySelector('#apply-offset'),
  calibrate: document.querySelector('#calibrate'),
  plexOrigin: document.querySelector('#plex-origin'),
  registerPlex: document.querySelector('#register-plex'),
};

let state = null;
let busy = false;
let actionError = null;

elements.pair.addEventListener('click', () => runCommand('pair', { token: elements.token.value, mode: elements.mode.value }));
elements.mode.addEventListener('change', renderConnection);
elements.source.addEventListener('change', () => runCommand('select-source', { source: elements.source.value }));
elements.volume.addEventListener('change', () => runCommand('set-volume', { value: Number(elements.volume.value) }));
elements.renderer.addEventListener('change', () => {
  void runCommand('select-renderer', { rendererId: elements.renderer.value });
});
elements.cast.addEventListener('click', () => runCommand('cast'));
elements.stop.addEventListener('click', () => runCommand('stop'));
elements.reload.addEventListener('click', () => runCommand('reload-tab'));
elements.calibrate.addEventListener('click', () => runCommand('calibrate'));
elements.applyOffset.addEventListener('click', () => {
  const milliseconds = Number(elements.offset.value);
  if (!Number.isInteger(milliseconds) || milliseconds < -5000 || milliseconds > 5000) {
    showError('Manual adjustment must be a whole number from −5000 to 5000 ms');
    return;
  }
  void runCommand('set-offset', { milliseconds });
});
elements.registerPlex.addEventListener('click', () => {
  void runCommand('register-plex-origin', { origin: elements.plexOrigin.value });
});

chrome.runtime.onMessage.addListener((message, sender) => {
  if (allowedBackgroundSender(sender, chrome.runtime.getURL('')) && message?.type === 'state-update') render(message.state);
});

void loadState();

async function loadState() {
  try {
    const response = await chrome.runtime.sendMessage({ type: 'state-request' });
    if (!response?.ok) throw new Error(response?.error ?? 'Could not load extension state');
    render(response.value);
  } catch (error) {
    showError(publicError(error));
  }
}

async function runCommand(command, fields = {}) {
  if (busy) return;
  actionError = null;
  setBusy(true);
  try {
    const response = await chrome.runtime.sendMessage({ type: 'popup-command', command, ...fields });
    if (!response?.ok) throw new Error(response?.error ?? 'Command failed');
    if (response.value) render(response.value);
    if (command === 'pair') elements.token.value = '';
    if (command === 'register-plex-origin') elements.plexOrigin.value = '';
  } catch (error) {
    showError(publicError(error));
  } finally {
    setBusy(false);
  }
}

function render(nextState) {
  state = nextState;
  const managed = state.mode === 'managed';
  const audioOnly = state.videoDelaySupported === false;
  elements.reload.hidden = !managed;
  elements.reload.disabled = busy || !state.paired || state.cleanupPending || state.source !== 'chrome' || !state.capabilities?.browserCanReload;
  elements.shortcut.textContent = !managed ? '' : state.capabilities?.shortcut ? `Chrome shortcut: ${state.capabilities.shortcut}` : 'Cast shortcut unassigned or unavailable. Check chrome://extensions/shortcuts.';
  elements.status.classList.toggle('error', Boolean(actionError || state.warning));
  elements.status.textContent = actionError || state.warning || (state.phase === 'streaming' && state.source === 'windows' ? 'Streaming Windows audio' : phaseLabel(state.phase));
  if (document.activeElement !== elements.mode) elements.mode.value = state.mode ?? 'legacy';
  elements.source.value = state.source ?? 'chrome';
  elements.source.disabled = busy || !managed || !state.paired || state.phase === 'switching';
  elements.volumeControls.hidden = !managed;
  elements.volumeValue.value = state.volume == null ? '—' : `${state.volume}%`;
  if (state.volume != null && document.activeElement !== elements.volume) elements.volume.value = state.volume;
  elements.volume.disabled = busy || !state.paired || !state.selectedRendererId;
  elements.cast.textContent = managed && state.source === 'windows' ? 'Start Windows audio' : 'Cast current tab';
  renderConnection();
  renderRenderers();
  elements.measured.value = `${state.measuredDelayMs ?? 0} ms`;
  elements.videoDelay.value = managed && state.source !== 'chrome' ? 'Not used for Windows audio' : audioOnly ? 'Audio only — no video delay' : `${effectiveDelay(state.measuredDelayMs ?? 0, state.manualOffsetMs ?? 0)} ms`;
  elements.calibrate.textContent = managed ? 'Calibrate & apply' : 'Run audible calibration';
  elements.calibrationHint.textContent = audioOnly ? 'This tab streams audio only. Video sync is available on YouTube and Plex; Sonos playback still has its normal buffering delay.' : managed
    ? 'Plays a timing signal, then automatically applies the measured video delay and resets manual adjustment to 0. You can fine-tune it afterwards.'
    : 'Calibration briefly plays a timing signal through the selected speaker.';
  elements.packets.value = `${state.counters?.receivedPackets ?? 0} sent, ${state.counters?.droppedPackets ?? 0} dropped`;
  if (document.activeElement !== elements.offset) elements.offset.value = state.manualOffsetMs ?? 0;

  const pairedWithRenderer = state.connected && state.paired && Boolean(state.selectedRendererId);
  const castActive = ['starting', 'streaming', 'calibrating', 'stopping', 'switching'].includes(state.phase);
  elements.pair.disabled = busy || castActive;
  elements.mode.disabled = busy || castActive;
  elements.cast.disabled = busy || state.cleanupPending || !pairedWithRenderer || !['idle', 'failed'].includes(state.phase);
  elements.stop.disabled = busy || !state.paired || !['starting', 'streaming', 'calibrating', 'failed', 'switching'].includes(state.phase);
  elements.calibrate.disabled = busy || audioOnly || state.phase !== 'streaming' || (managed && !state.capabilities?.calibrationAvailable);
  elements.offset.disabled = audioOnly || (managed && state.source !== 'chrome');
  elements.applyOffset.disabled = busy || audioOnly || !state.connected || !state.paired || !state.selectedRendererId || (managed && state.source !== 'chrome');
  elements.renderer.disabled ||= castActive;
}

function renderConnection() {
  const managed = elements.mode.value === 'managed';
  elements.token.hidden = managed; elements.tokenLabel.hidden = managed;
  elements.pair.textContent = managed ? (state?.paired ? 'Reconnect' : 'Connect') : 'Pair';
  const setup = {
    'app-unavailable': 'Open or install Elgato Remote on this PC, then click Connect.',
    'native-installation-unavailable': 'Repair Elgato Remote, then click Connect.',
    'native-origin-denied': 'This extension version does not match the installed app. Repair or update Elgato Remote.',
    'native-unsupported-version': 'Update Elgato Remote to connect this extension.',
    'native-invalid-response': 'Update or repair Elgato Remote, then click Connect.',
    'app-authentication-failed': 'Restart Elgato Remote, then click Connect.',
    connecting: 'Connecting to Elgato Remote…',
    connected: 'Connected automatically to Elgato Remote on this PC.',
  };
  elements.setupHint.textContent = managed ? (setup[state?.setupStatus] ?? 'Connects automatically to Elgato Remote on this PC.') : 'Use the token from the standalone legacy app.';
}

function renderRenderers() {
  const selected = state.selectedRendererId ?? '';
  const fragment = document.createDocumentFragment();
  const placeholder = document.createElement('option');
  placeholder.value = '';
  placeholder.textContent = state.renderers.length ? 'Choose a room' : 'No rooms loaded';
  fragment.append(placeholder);
  for (const renderer of state.renderers) {
    const option = document.createElement('option');
    option.value = renderer.id;
    option.textContent = renderer.model ? `${renderer.name} — ${renderer.model}` : renderer.name;
    fragment.append(option);
  }
  elements.renderer.replaceChildren(fragment);
  elements.renderer.value = selected;
  elements.renderer.disabled = !state.connected || !state.paired || state.renderers.length === 0;
}

function setBusy(value) {
  busy = value;
  for (const button of document.querySelectorAll('button')) button.setAttribute('aria-busy', String(busy));
  if (state) render(state);
}

function showError(message) {
  actionError = message;
  elements.status.classList.add('error');
  elements.status.textContent = message;
}

function phaseLabel(phase) {
  const labels = {
    disconnected: 'Local app is disconnected',
    connecting: 'Connecting to local app…',
    authenticating: 'Checking pairing token…',
    idle: 'Ready to cast',
    starting: 'Starting Sonos playback…',
    streaming: 'Casting tab audio',
    calibrating: 'Running audible calibration…',
    stopping: 'Stopping…',
    switching: 'Releasing Chrome capture…',
    failed: 'Casting stopped with an error',
  };
  return labels[phase] ?? phase;
}

function publicError(error) {
  return error instanceof Error ? error.message : 'Unexpected extension error';
}
