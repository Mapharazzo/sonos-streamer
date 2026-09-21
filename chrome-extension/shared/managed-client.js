import { CONTROL_URL, parseManagedEvent } from './managed-protocol.js';
export class ManagedClient {
  constructor({ connect = (url) => new WebSocket(url), schedule = setTimeout, cancel = clearTimeout, onEvent = () => {} } = {}) {
    Object.assign(this, { openSocket: connect, schedule, cancel, onEvent });
    this.socket = null; this.token = ''; this.pending = new Map(); this.timers = new Set(); this.backoff = 500; this.closed = true; this.authenticated = false;
  }
  connect(token) { this.close(); this.token = token; this.closed = false; this.backoff = 500; this.open(); }
  later(fn, ms) { const timer = this.schedule(() => { this.timers.delete(timer); fn(); }, ms); this.timers.add(timer); return timer; }
  clear(timer) { this.cancel(timer); this.timers.delete(timer); }
  open() {
    if (this.closed) return;
    let socket;
    try { socket = this.openSocket(CONTROL_URL); } catch { this.retry(); return; }
    this.socket = socket; this.authenticated = false;
    const timeout = this.later(() => { if (this.socket === socket) socket.close(); }, 5000);
    socket.addEventListener('open', () => { if (this.socket === socket) socket.send(JSON.stringify({ type: 'hello', version: 1, token: this.token, role: 'browser' })); });
    socket.addEventListener('message', ({ data }) => {
      if (this.socket !== socket) return;
      let event; try { if (typeof data !== 'string' || data.length > 65536) throw new Error(); event = parseManagedEvent(JSON.parse(data)); } catch { this.close(); this.onEvent({ type: 'error', code: 'invalid-managed-event' }); return; }
      if (event.type === 'hello-ok') { this.clear(timeout); this.authenticated = true; }
      if (event.type === 'state') this.backoff = 500;
      if (event.type === 'result') {
        const pending = this.pending.get(event.requestId);
        if (pending) { this.clear(pending.timer); this.pending.delete(event.requestId); pending.resolve(event); }
      }
      if (event.type === 'error') { this.close(); }
      this.onEvent(event);
    });
    socket.addEventListener('close', () => {
      if (this.socket !== socket) return;
      this.clear(timeout); this.socket = null; this.authenticated = false; this.rejectPending('control-disconnected'); this.onEvent({ type: 'disconnected' }); this.retry();
    });
    socket.addEventListener('error', () => { if (this.socket === socket) socket.close(); });
  }
  retry() { if (!this.closed) { this.later(() => this.open(), this.backoff); this.backoff = Math.min(30000, this.backoff * 2); } }
  send(command) {
    if (!this.authenticated || this.socket?.readyState !== 1) return Promise.reject(new Error('control-disconnected'));
    if (this.pending.has(command.requestId) || this.pending.size >= 16) return Promise.reject(new Error('command-busy'));
    return new Promise((resolve, reject) => {
      const timer = this.later(() => { this.pending.delete(command.requestId); reject(new Error('command-outcome-unknown')); }, 16000);
      this.pending.set(command.requestId, { resolve, reject, timer });
      try { this.socket.send(JSON.stringify(command)); } catch { this.clear(timer); this.pending.delete(command.requestId); reject(new Error('control-disconnected')); }
    });
  }
  rejectPending(message) { for (const pending of this.pending.values()) { this.clear(pending.timer); pending.reject(new Error(message)); } this.pending.clear(); }
  close() {
    this.closed = true; this.authenticated = false;
    for (const timer of this.timers) this.cancel(timer); this.timers.clear();
    this.rejectPending('control-disconnected'); const socket = this.socket; this.socket = null; socket?.close();
  }
}
