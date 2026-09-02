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
