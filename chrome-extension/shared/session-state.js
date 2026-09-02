export function initialSessionState() {
  return {
    phase: 'disconnected',
    connected: false,
    paired: false,
    renderers: [],
    selectedRendererId: null,
    tabId: null,
    measuredDelayMs: 0,
    manualOffsetMs: 0,
    warning: null,
    videoStatus: 'inactive',
    videoWarning: null,
    counters: { receivedPackets: 0, droppedPackets: 0 },
  };
}

export function reduceSession(state, event) {
  switch (event.type) {
    case 'bridge-connecting':
      return { ...state, phase: 'connecting', warning: null };
    case 'bridge-connected':
      return { ...state, connected: true, phase: 'authenticating', warning: null };
    case 'hello-ok':
      return { ...state, connected: true, paired: true, phase: 'idle', warning: null };
    case 'renderer-list':
      return { ...state, renderers: [...event.renderers] };
    case 'cast-state':
      return {
        ...state,
        phase: event.state,
        selectedRendererId: event.rendererId ?? state.selectedRendererId,
      };
    case 'stream-state':
      return { ...state, phase: event.state === 'started' ? 'streaming' : 'idle' };
    case 'calibration-started':
      return { ...state, phase: 'calibrating', warning: null };
    case 'calibration-result':
      return { ...state, phase: 'streaming', measuredDelayMs: event.milliseconds, warning: null };
    case 'calibration-failed':
      return { ...state, phase: 'streaming', warning: event.message ?? event.code };
    case 'manual-offset-set':
      return { ...state, manualOffsetMs: event.milliseconds };
    case 'audio-stats':
      return {
        ...state,
        counters: {
          receivedPackets: event.receivedPackets,
          droppedPackets: event.droppedPackets,
        },
      };
    case 'warning':
      return { ...state, warning: event.message ?? event.code };
    case 'error':
      return { ...state, phase: event.terminal ? 'failed' : state.phase, warning: event.message };
    case 'capture-started':
      return { ...state, tabId: event.tabId };
    case 'video-delay-status':
      {
        const isFallback =
          event.status === 'error' ||
          (event.status === 'audio-only' &&
            !['waiting-for-video', 'stopped', 'not-started'].includes(event.code));
        const videoWarning = isFallback
          ? event.message ?? 'Video delay unavailable; audio is still casting'
          : null;
        const warning = isFallback
          ? videoWarning
          : state.warning === state.videoWarning
            ? null
            : state.warning;
        return { ...state, videoStatus: event.status, videoWarning, warning };
      }
    case 'bridge-disconnected':
      return {
        ...state,
        connected: false,
        phase: 'disconnected',
        tabId: null,
        warning: event.message ?? state.warning,
      };
    default:
      return state;
  }
}

export function effectiveDelay(measuredMs, manualOffsetMs) {
  if (!Number.isFinite(measuredMs) || !Number.isFinite(manualOffsetMs)) {
    throw new TypeError('Delay values must be finite');
  }
  return Math.max(0, Math.min(5000, Math.round(measuredMs) + Math.round(manualOffsetMs)));
}

export function applyCalibration(state, { latencyMs }) {
  if (!Number.isInteger(latencyMs) || latencyMs < 0) {
    throw new TypeError('Calibration latency must be a non-negative integer');
  }
  const nextState = {
    ...state,
    phase: 'streaming',
    measuredDelayMs: latencyMs,
    warning: null,
  };
  return {
    state: nextState,
    effects: [
      {
        type: 'send-video-delay',
        delayMs: effectiveDelay(nextState.measuredDelayMs, nextState.manualOffsetMs),
      },
    ],
  };
}

export function terminalCleanup(state, reason) {
  const active = ['starting', 'streaming', 'calibrating'].includes(state.phase);
  if (!active || state.cleanupPending) return { state, effects: [] };
  return {
    state: { ...state, phase: 'stopping', cleanupPending: true, cleanupReason: reason },
    effects: [
      { type: 'best-effort-stop-renderer' },
      { type: 'stop-media-tracks' },
      { type: 'dispose-audio' },
      { type: 'dispose-video' },
    ],
  };
}
