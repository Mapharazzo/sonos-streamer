import { TimedFrameQueue } from './frame-queue.js';
import { chooseFramePlan } from './resource-budget.js';

const MAX_DELAY_MS = 5000;
const MAX_CAPTURE_FPS = 30;
const MAX_TEXTURE_BYTES = 512 * 1024 ** 2;

export class WebGlDelaySurface {
  #video;
  #placement;
  #options;
  #originalStyle;
  #canvas = null;
  #gpu = null;
  #queue = null;
  #displayed = null;
  #plan = null;
  #sourceWidth = 0;
  #sourceHeight = 0;
  #delayMs = 0;
  #lastCaptureAt = Number.NEGATIVE_INFINITY;
  #videoCallbackId = null;
  #animationFrameId = null;
  #placementCleanup = null;
  #started = false;
  #disposed = false;
  #sourceHidden = false;
  #contextLostHandler;
  #videoFrameHandler;
  #animationFrameHandler;

  constructor(video, placement, options = {}) {
    if (!video?.style) throw new TypeError('video must have an inline style object');
    if (!placement?.container) throw new TypeError('placement must provide a container');
    this.#video = video;
    this.#placement = placement;
    this.#options = {
      createCanvas: options.createCanvas ?? (() => document.createElement('canvas')),
      createGpu: options.createGpu ?? ((canvas, plan) => new WebGlGpu(canvas, plan)),
      requestAnimationFrame:
        options.requestAnimationFrame ?? ((callback) => globalThis.requestAnimationFrame(callback)),
      cancelAnimationFrame:
        options.cancelAnimationFrame ?? ((id) => globalThis.cancelAnimationFrame(id)),
      onFallback: options.onFallback ?? (() => {}),
      maxBytes: options.maxBytes ?? MAX_TEXTURE_BYTES,
    };
    this.#originalStyle = {
      opacity: video.style.opacity,
      visibility: video.style.visibility,
      position: video.style.position,
      zIndex: video.style.zIndex,
    };
    this.#contextLostHandler = (event) => this.#onContextLost(event);
    this.#videoFrameHandler = (timestamp) => this.#onVideoFrame(timestamp);
    this.#animationFrameHandler = (timestamp) => this.#onAnimationFrame(timestamp);
  }

  start(delayMs) {
    if (this.#disposed) throw new Error('A disposed delay surface cannot be restarted');
    if (this.#started) {
      this.setDelay(delayMs);
      return;
    }
    if (typeof this.#video.requestVideoFrameCallback !== 'function') {
      this.#fail('unsupported-video-callback', 'This browser cannot schedule decoded video frames');
      return;
    }

    this.#delayMs = clampDelay(delayMs);
    this.#started = true;
    try {
      this.#canvas = this.#options.createCanvas();
      Object.assign(this.#canvas.style, {
        position: 'absolute',
        inset: '0',
        width: '100%',
        height: '100%',
        pointerEvents: 'none',
        visibility: 'hidden',
        zIndex: String(this.#placement.zIndex ?? 1),
      });
      this.#canvas.setAttribute?.('aria-hidden', 'true');
      this.#canvas.addEventListener('webglcontextlost', this.#contextLostHandler);
      if (typeof this.#placement.mount === 'function') {
        this.#placementCleanup = this.#placement.mount(this.#canvas, this.#video) ?? null;
      } else {
        this.#placement.container.append(this.#canvas);
      }
      this.#scheduleVideoFrame();
      this.#animationFrameId = this.#options.requestAnimationFrame(this.#animationFrameHandler);
    } catch (error) {
      this.#fail('surface-start-failed', publicMessage(error));
    }
  }

  setDelay(delayMs) {
    this.#delayMs = clampDelay(delayMs);
    this.#queue?.setDelay(this.#delayMs);
    if (this.#canvas) this.#placement.update?.(this.#canvas, this.#video);
  }

  dispose() {
    if (this.#disposed) return;
    this.#disposed = true;
    if (this.#videoCallbackId !== null) {
      this.#video.cancelVideoFrameCallback?.(this.#videoCallbackId);
      this.#videoCallbackId = null;
    }
    if (this.#animationFrameId !== null) {
      this.#options.cancelAnimationFrame(this.#animationFrameId);
      this.#animationFrameId = null;
    }
    this.#queue?.clear();
    this.#queue = null;
    this.#displayed?.release();
    this.#displayed = null;
    this.#gpu?.dispose();
    this.#gpu = null;
    if (this.#canvas) {
      this.#canvas.removeEventListener('webglcontextlost', this.#contextLostHandler);
      if (typeof this.#placementCleanup === 'function') this.#placementCleanup();
      else this.#canvas.parentNode?.removeChild(this.#canvas);
    }
    this.#placementCleanup = null;
    this.#canvas = null;
    this.#restoreSourceStyle();
  }

  #onVideoFrame(timestamp) {
    this.#videoCallbackId = null;
    if (this.#disposed) return;
    try {
      if (timestamp - this.#lastCaptureAt >= 1000 / MAX_CAPTURE_FPS) {
        this.#captureFrame(timestamp);
        this.#lastCaptureAt = timestamp;
      }
    } catch (error) {
      this.#fail('capture-failed', publicMessage(error));
      return;
    }
    this.#scheduleVideoFrame();
  }

  #captureFrame(timestamp) {
    const sourceWidth = Math.floor(this.#video.videoWidth);
    const sourceHeight = Math.floor(this.#video.videoHeight);
    const plan = chooseFramePlan({
      sourceWidth,
      sourceHeight,
      fps: MAX_CAPTURE_FPS,
      delayMs: this.#delayMs,
      maxBytes: this.#options.maxBytes,
    });
    if (!plan) throw new Error('Video delay exceeds the safe texture budget');

    if (
      !this.#gpu ||
      sourceWidth !== this.#sourceWidth ||
      sourceHeight !== this.#sourceHeight ||
      plan.width !== this.#plan.width ||
      plan.height !== this.#plan.height ||
      plan.frameCount !== this.#plan.frameCount
    ) {
      this.#replacePipeline(plan, sourceWidth, sourceHeight, timestamp);
      return;
    }

    const resource = this.#gpu.capture(this.#video);
    if (!this.#queue.enqueue(resource, timestamp, this.#delayMs, resource.bytes)) {
      throw new Error('Video frame queue rejected a texture');
    }
    if (!this.#sourceHidden) {
      this.#gpu.render(resource);
      this.#showOverlay();
    }
  }

  #replacePipeline(plan, sourceWidth, sourceHeight, timestamp) {
    let candidate = null;
    try {
      this.#canvas.width = plan.width;
      this.#canvas.height = plan.height;
      candidate = this.#options.createGpu(this.#canvas, { ...plan, sourceWidth, sourceHeight });
      const firstResource = candidate.capture(this.#video);
      candidate.render(firstResource);

      const nextQueue = new TimedFrameQueue({
        release: (resource) => resource.release(),
        maxFrames: plan.frameCount,
        maxBytes: this.#options.maxBytes,
      });
      if (!nextQueue.enqueue(firstResource, timestamp, this.#delayMs, firstResource.bytes)) {
        throw new Error('Video frame queue rejected its first texture');
      }

      this.#queue?.clear();
      this.#displayed?.release();
      this.#displayed = null;
      this.#gpu?.dispose();
      this.#gpu = candidate;
      this.#queue = nextQueue;
      this.#plan = plan;
      this.#sourceWidth = sourceWidth;
      this.#sourceHeight = sourceHeight;
      this.#placement.update?.(this.#canvas, this.#video);
      candidate = null;
      this.#showOverlay();
    } finally {
      candidate?.dispose();
    }
  }

  #onAnimationFrame(now) {
    this.#animationFrameId = null;
    if (this.#disposed) return;
    try {
      const resource = this.#queue?.takeDue(now);
      if (resource) {
        this.#gpu.render(resource);
        this.#displayed?.release();
        this.#displayed = resource;
      }
    } catch (error) {
      this.#fail('render-failed', publicMessage(error));
      return;
    }
    this.#animationFrameId = this.#options.requestAnimationFrame(this.#animationFrameHandler);
  }

  #scheduleVideoFrame() {
    this.#videoCallbackId = this.#video.requestVideoFrameCallback(this.#videoFrameHandler);
  }

  #showOverlay() {
    this.#canvas.style.visibility = 'visible';
    this.#video.style.opacity = '0';
    this.#sourceHidden = true;
  }

  #onContextLost(event) {
    event.preventDefault();
    this.#fail('webgl-context-lost', 'WebGL context was lost; continuing with audio only');
  }

  #fail(code, message) {
    if (this.#disposed) return;
    this.dispose();
    this.#options.onFallback({ code, message });
  }

  #restoreSourceStyle() {
    for (const [property, value] of Object.entries(this.#originalStyle)) {
      this.#video.style[property] = value;
    }
    this.#sourceHidden = false;
  }
}

class WebGlGpu {
  #gl;
  #program;
  #framebuffer;
  #sourceTexture;
  #resources = new Set();
  #plan;

  constructor(canvas, plan) {
    const gl = canvas.getContext('webgl2', {
      alpha: false,
      antialias: false,
      depth: false,
      stencil: false,
      preserveDrawingBuffer: false,
    });
    if (!gl) throw new Error('WebGL2 is unavailable');
    this.#gl = gl;
    this.#plan = plan;
    let program = null;
    let framebuffer = null;
    let sourceTexture = null;
    try {
      program = createProgram(gl);
      framebuffer = gl.createFramebuffer();
      sourceTexture = createTexture(gl);
      if (!framebuffer || !sourceTexture) throw new Error('Could not allocate WebGL objects');

      drainErrors(gl);
      gl.bindTexture(gl.TEXTURE_2D, sourceTexture);
      gl.texImage2D(
        gl.TEXTURE_2D,
        0,
        gl.RGBA,
        plan.sourceWidth,
        plan.sourceHeight,
        0,
        gl.RGBA,
        gl.UNSIGNED_BYTE,
        null,
      );
      assertNoGlError(gl, 'source texture allocation failed');
    } catch (error) {
      if (sourceTexture) gl.deleteTexture(sourceTexture);
      if (framebuffer) gl.deleteFramebuffer(framebuffer);
      if (program) gl.deleteProgram(program);
      throw error;
    }
    this.#program = program;
    this.#framebuffer = framebuffer;
    this.#sourceTexture = sourceTexture;
  }

  capture(video) {
    const gl = this.#gl;
    drainErrors(gl);
    gl.bindTexture(gl.TEXTURE_2D, this.#sourceTexture);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, true);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, gl.RGBA, gl.UNSIGNED_BYTE, video);
    assertNoGlError(gl, 'video texture upload failed');

    const texture = createTexture(gl);
    if (!texture) throw new Error('Could not allocate a delayed video texture');
    try {
      gl.bindTexture(gl.TEXTURE_2D, texture);
      gl.texImage2D(
        gl.TEXTURE_2D,
        0,
        gl.RGBA,
        this.#plan.width,
        this.#plan.height,
        0,
        gl.RGBA,
        gl.UNSIGNED_BYTE,
        null,
      );
      assertNoGlError(gl, 'delayed texture allocation failed');
      gl.bindFramebuffer(gl.FRAMEBUFFER, this.#framebuffer);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, texture, 0);
      if (gl.checkFramebufferStatus(gl.FRAMEBUFFER) !== gl.FRAMEBUFFER_COMPLETE) {
        throw new Error('Delayed texture framebuffer is incomplete');
      }
      this.#draw(this.#sourceTexture, this.#plan.width, this.#plan.height);
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      assertNoGlError(gl, 'video downscale failed');
    } catch (error) {
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      gl.deleteTexture(texture);
      throw error;
    }

    let active = true;
    const resource = {
      texture,
      bytes: this.#plan.bytesPerFrame,
      release: () => {
        if (!active) return;
        active = false;
        this.#resources.delete(resource);
        gl.deleteTexture(texture);
      },
    };
    this.#resources.add(resource);
    return resource;
  }

  render(resource) {
    this.#gl.bindFramebuffer(this.#gl.FRAMEBUFFER, null);
    this.#draw(resource.texture, this.#plan.width, this.#plan.height);
    assertNoGlError(this.#gl, 'delayed frame render failed');
  }

  dispose() {
    for (const resource of [...this.#resources]) resource.release();
    this.#gl.deleteTexture(this.#sourceTexture);
    this.#gl.deleteFramebuffer(this.#framebuffer);
    this.#gl.deleteProgram(this.#program);
    this.#resources.clear();
  }

  #draw(texture, width, height) {
    const gl = this.#gl;
    gl.viewport(0, 0, width, height);
    gl.useProgram(this.#program);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }
}

function createTexture(gl) {
  const texture = gl.createTexture();
  if (!texture) return null;
  gl.bindTexture(gl.TEXTURE_2D, texture);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  return texture;
}

function createProgram(gl) {
  const vertex = compileShader(
    gl,
    gl.VERTEX_SHADER,
    `#version 300 es
    out vec2 texturePosition;
    const vec2 positions[3] = vec2[](vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    void main() {
      vec2 position = positions[gl_VertexID];
      texturePosition = position * 0.5 + 0.5;
      gl_Position = vec4(position, 0.0, 1.0);
    }`,
  );
  let fragment;
  try {
    fragment = compileShader(
      gl,
      gl.FRAGMENT_SHADER,
      `#version 300 es
      precision mediump float;
      uniform sampler2D frame;
      in vec2 texturePosition;
      out vec4 outputColor;
      void main() { outputColor = texture(frame, texturePosition); }`,
    );
  } catch (error) {
    gl.deleteShader(vertex);
    throw error;
  }
  const program = gl.createProgram();
  if (!program) throw new Error('Could not create WebGL program');
  gl.attachShader(program, vertex);
  gl.attachShader(program, fragment);
  gl.linkProgram(program);
  gl.deleteShader(vertex);
  gl.deleteShader(fragment);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    const message = gl.getProgramInfoLog(program) || 'unknown link error';
    gl.deleteProgram(program);
    throw new Error(`WebGL program link failed: ${message}`);
  }
  gl.useProgram(program);
  const sampler = gl.getUniformLocation(program, 'frame');
  if (sampler !== null) gl.uniform1i(sampler, 0);
  return program;
}

function compileShader(gl, type, source) {
  const shader = gl.createShader(type);
  if (!shader) throw new Error('Could not create WebGL shader');
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    const message = gl.getShaderInfoLog(shader) || 'unknown compile error';
    gl.deleteShader(shader);
    throw new Error(`WebGL shader compilation failed: ${message}`);
  }
  return shader;
}

function drainErrors(gl) {
  while (gl.getError() !== gl.NO_ERROR) {
    // Drain stale errors so allocation and upload failures are attributed correctly.
  }
}

function assertNoGlError(gl, message) {
  if (gl.getError() !== gl.NO_ERROR) throw new Error(message);
}

function clampDelay(value) {
  if (!Number.isFinite(value)) throw new TypeError('Video delay must be finite');
  return Math.max(0, Math.min(MAX_DELAY_MS, value));
}

function publicMessage(error) {
  return error instanceof Error ? error.message : 'Unexpected WebGL failure';
}
