export class TimedFrameQueue {
  #entries = [];
  #release;
  #maxFrames;
  #maxBytes;
  #bytes = 0;
  #delayMs = null;

  constructor({ release, maxFrames, maxBytes }) {
    if (typeof release !== 'function') throw new TypeError('release must be a function');
    if (!Number.isInteger(maxFrames) || maxFrames <= 0) {
      throw new RangeError('maxFrames must be a positive integer');
    }
    if (!Number.isFinite(maxBytes) || maxBytes <= 0) {
      throw new RangeError('maxBytes must be positive');
    }
    this.#release = release;
    this.#maxFrames = maxFrames;
    this.#maxBytes = maxBytes;
  }

  get length() {
    return this.#entries.length;
  }

  get bytes() {
    return this.#bytes;
  }

  enqueue(resource, capturedAt, delayMs, bytes) {
    requireFinite(capturedAt, 'capturedAt');
    requireFinite(delayMs, 'delayMs');
    if (!Number.isFinite(bytes) || bytes < 0) throw new RangeError('bytes must not be negative');

    const clampedDelay = Math.max(0, delayMs);
    if (this.#delayMs === null) this.#delayMs = clampedDelay;
    else if (clampedDelay !== this.#delayMs) this.setDelay(clampedDelay);

    const entry = {
      resource,
      capturedAt,
      dueAt: capturedAt + this.#delayMs,
      bytes,
    };
    const insertionIndex = this.#entries.findIndex((candidate) => candidate.capturedAt > capturedAt);
    if (insertionIndex === -1) this.#entries.push(entry);
    else this.#entries.splice(insertionIndex, 0, entry);
    this.#bytes += bytes;

    while (this.#entries.length > this.#maxFrames || this.#bytes > this.#maxBytes) {
      this.#releaseEntry(this.#entries.shift());
    }
    return this.#entries.includes(entry);
  }

  takeDue(now) {
    requireFinite(now, 'now');
    let newestDue = null;
    for (const entry of this.#entries) {
      if (entry.dueAt <= now) newestDue = entry;
    }
    if (!newestDue) return null;

    const previousEntries = this.#entries;
    const future = previousEntries.filter((entry) => entry.dueAt > now);
    this.#entries = future;
    this.#bytes = future.reduce((total, entry) => total + entry.bytes, 0);
    for (const entry of previousEntries) {
      if (entry.dueAt <= now && entry !== newestDue) this.#release(entry.resource);
    }
    return newestDue.resource;
  }

  setDelay(delayMs) {
    requireFinite(delayMs, 'delayMs');
    const nextDelay = Math.max(0, delayMs);
    const previousDelay = this.#delayMs ?? nextDelay;
    const delta = nextDelay - previousDelay;
    for (const entry of this.#entries) entry.dueAt += delta;
    this.#delayMs = nextDelay;
  }

  clear() {
    const entries = this.#entries;
    this.#entries = [];
    this.#bytes = 0;
    for (const entry of entries) this.#release(entry.resource);
  }

  #releaseEntry(entry) {
    if (!entry) return;
    this.#bytes -= entry.bytes;
    this.#release(entry.resource);
  }
}

function requireFinite(value, name) {
  if (!Number.isFinite(value)) throw new TypeError(`${name} must be finite`);
}
