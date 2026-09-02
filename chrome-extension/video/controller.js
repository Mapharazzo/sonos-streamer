import { plexAdapter } from './adapters/plex.js';
import { youtubeAdapter } from './adapters/youtube.js';
import { WebGlDelaySurface } from './webgl-delay.js';

const MAX_DELAY_MS = 5000;

export class VideoDelayController {
  #document;
  #location;
  #adapters;
  #Surface;
  #onStatus;
  #adapter = null;
  #surface = null;
  #video = null;
  #delayMs = 0;
  #enabled = false;
  #evaluationQueued = false;
  #failedVideos = new WeakSet();
  #status = { status: 'audio-only', code: 'not-started' };

  constructor(options = {}) {
    this.#document = options.document ?? globalThis.document;
    this.#location = options.location ?? globalThis.location;
    this.#adapters = options.adapters ?? [youtubeAdapter, plexAdapter];
    this.#Surface = options.Surface ?? WebGlDelaySurface;
    this.#onStatus = options.onStatus ?? (() => {});
  }

  setDelay(delayMs) {
    if (!Number.isFinite(delayMs)) throw new TypeError('Video delay must be finite');
    this.#delayMs = Math.max(0, Math.min(MAX_DELAY_MS, delayMs));
    this.#enabled = true;
    this.#evaluate();
    return this.#status;
  }

  stop() {
    this.#enabled = false;
    this.#surface?.dispose();
    this.#surface = null;
    this.#video = null;
    this.#adapter?.dispose();
    this.#adapter = null;
    return this.#report({ status: 'audio-only', code: 'stopped' });
  }

  getStatus() {
    return { ...this.#status };
  }

  #queueEvaluation() {
    if (this.#evaluationQueued) return;
    this.#evaluationQueued = true;
    queueMicrotask(() => {
      this.#evaluationQueued = false;
      if (this.#enabled) this.#evaluate();
    });
  }

  #evaluate() {
    if (!this.#enabled) return;
    const location = new URL(this.#location.href);
    const adapter = this.#adapters.find((candidate) => candidate.matches(location)) ?? null;
    if (!adapter) {
      this.#replaceAdapter(null);
      this.#report({ status: 'error', code: 'unsupported-site', message: 'This site is not supported' });
      return;
    }
    if (adapter !== this.#adapter) this.#replaceAdapter(adapter);

    const video = adapter.findVideo(this.#document);
    if (!video) {
      if (this.#video && this.#surface && this.#video.isConnected !== false && !this.#video.ended) {
        this.#surface.setDelay(this.#delayMs);
        this.#report({ status: 'active', delayMs: this.#delayMs });
        return;
      }
      this.#replaceSurface(null);
      this.#report({ status: 'audio-only', code: 'waiting-for-video' });
      return;
    }
    if (this.#failedVideos.has(video)) {
      this.#report({ status: 'audio-only', code: 'video-fallback' });
      return;
    }
    if (video === this.#video && this.#surface) {
      this.#surface.setDelay(this.#delayMs);
      this.#report({ status: 'active', delayMs: this.#delayMs });
      return;
    }

    this.#replaceSurface(null);
    const surface = new this.#Surface(video, adapter.placement(video), {
      onFallback: (event) => {
        this.#failedVideos.add(video);
        if (this.#surface === surface) this.#surface = null;
        this.#report({ status: 'audio-only', ...event });
      },
    });
    this.#surface = surface;
    this.#video = video;
    surface.start(this.#delayMs);
    if (this.#surface === surface) this.#report({ status: 'active', delayMs: this.#delayMs });
  }

  #replaceAdapter(adapter) {
    this.#replaceSurface(null);
    this.#adapter?.dispose();
    this.#adapter = adapter;
    if (adapter) adapter.observe(() => this.#queueEvaluation(), this.#document);
  }

  #replaceSurface(surface) {
    if (this.#surface !== surface) this.#surface?.dispose();
    this.#surface = surface;
    if (!surface) this.#video = null;
  }

  #report(status) {
    this.#status = { ...status };
    this.#onStatus(this.getStatus());
    return this.getStatus();
  }
}
