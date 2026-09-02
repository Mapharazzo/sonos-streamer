const elements = {
  status: document.querySelector('#status'),
  token: document.querySelector('#token'),
  pair: document.querySelector('#pair'),
  renderer: document.querySelector('#renderer'),
  cast: document.querySelector('#cast'),
  stop: document.querySelector('#stop'),
  measured: document.querySelector('#measured'),
  offset: document.querySelector('#offset'),
  applyOffset: document.querySelector('#apply-offset'),
  calibrate: document.querySelector('#calibrate'),
  plexOrigin: document.querySelector('#plex-origin'),
  registerPlex: document.querySelector('#register-plex'),
};

let state = null;

elements.pair.addEventListener('click', () => runCommand('pair', { token: elements.token.value }));
elements.renderer.addEventListener('change', () => {
  void runCommand('select-renderer', { rendererId: elements.renderer.value });
});
elements.cast.addEventListener('click', () => runCommand('cast'));
elements.stop.addEventListener('click', () => runCommand('stop'));
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

chrome.runtime.onMessage.addListener((message) => {
  if (message?.type === 'state-update') render(message.state);
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
  elements.status.classList.toggle('error', Boolean(state.warning));
  elements.status.textContent = state.warning || phaseLabel(state.phase);
  elements.pair.textContent = state.hasStoredToken && !elements.token.value ? 'Connect' : 'Pair';
  renderRenderers();
  elements.measured.value = `${state.measuredDelayMs ?? 0} ms`;
  if (document.activeElement !== elements.offset) elements.offset.value = state.manualOffsetMs ?? 0;

  const pairedWithRenderer = state.connected && state.paired && Boolean(state.selectedRendererId);
  elements.cast.disabled = !pairedWithRenderer || state.phase !== 'idle';
  elements.stop.disabled = !['starting', 'streaming', 'calibrating'].includes(state.phase);
  elements.calibrate.disabled = state.phase !== 'streaming';
  elements.applyOffset.disabled = !state.connected || !state.paired || !state.selectedRendererId;
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

function setBusy(busy) {
  for (const button of document.querySelectorAll('button')) button.setAttribute('aria-busy', String(busy));
  if (!busy && state) render(state);
}

function showError(message) {
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
    failed: 'Casting stopped with an error',
  };
  return labels[phase] ?? phase;
}

function publicError(error) {
  return error instanceof Error ? error.message : 'Unexpected extension error';
}
