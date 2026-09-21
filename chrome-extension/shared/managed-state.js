export function reduceManagedState(state, event) {
  if (event.type === 'hello-ok') return { instanceId: event.instanceId, connectionId: event.connectionId, snapshot: null, connected: true };
  if (event.type === 'disconnected') return { ...state, connected: false, snapshot: null };
  if (event.type !== 'state' || !state?.connected || state.instanceId !== event.instanceId || (state.snapshot && event.revision <= state.snapshot.revision)) return state;
  return { ...state, snapshot: event };
}

/** Single admission gate for popup and shortcut capture; cleanup cancels async setup. */
export class CaptureLifecycle {
  generation = 0;
  busy = false;
  constructor({ disposeAudio, disposeVideo, acknowledge }) { Object.assign(this, { disposeAudio, disposeVideo, acknowledge }); this.releaseQueue = Promise.resolve(); this.releases = 0; }
  async start(operation) {
    if (this.busy) throw new Error('capture-busy');
    this.busy = true; const generation = ++this.generation;
    const check = () => { if (generation !== this.generation) throw new Error('capture-cancelled'); };
    try { const value = await operation(check); check(); return value; }
    finally { if (generation === this.generation) this.busy = false; }
  }
  release(transitionId) {
    ++this.generation; this.busy = true; this.releases += 1;
    // A failed cleanup must never acknowledge release to the source switch.
    const operation = this.releaseQueue.then(async () => { await this.disposeAudio(); await this.disposeVideo(); if (transitionId) await this.acknowledge(transitionId); }).finally(() => { this.releases -= 1; this.busy = this.releases > 0; });
    this.releaseQueue = operation.catch(() => {});
    return operation;
  }
}
