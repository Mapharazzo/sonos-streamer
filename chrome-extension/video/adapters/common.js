import { selectDominantVideo } from '../dominant-video.js';

export function findDominantVideo(document) {
  const view = document.defaultView;
  return selectDominantVideo(document.querySelectorAll('video'), {
    left: 0,
    top: 0,
    width: view?.innerWidth ?? 0,
    height: view?.innerHeight ?? 0,
  });
}

export function createPlacement(video, containerSelectors, controlSelectors) {
  const matchedContainer = video.closest?.(containerSelectors);
  const container = matchedContainer && matchedContainer !== video ? matchedContainer : video.parentElement;
  if (!container) throw new Error('Could not locate a stable video player container');
  const originalContainerPosition = container.style.position;
  const raisedControls = new Map();
  const overlayZIndex = 1;

  function raiseControls() {
    if (!controlSelectors || typeof container.querySelectorAll !== 'function') return;
    const view = container.ownerDocument?.defaultView;
    for (const control of container.querySelectorAll(controlSelectors)) {
      if (raisedControls.has(control)) continue;
      const computed = view?.getComputedStyle?.(control);
      const computedZIndex = Number.parseInt(computed?.zIndex, 10);
      if (Number.isFinite(computedZIndex) && computedZIndex > overlayZIndex) continue;
      raisedControls.set(control, {
        position: control.style.position,
        zIndex: control.style.zIndex,
      });
      if (computed?.position === 'static') control.style.position = 'relative';
      control.style.zIndex = String(overlayZIndex + 1);
    }
  }

  function update(canvas) {
    const videoRect = video.getBoundingClientRect();
    const containerRect = container.getBoundingClientRect();
    canvas.style.inset = 'auto';
    canvas.style.left = `${videoRect.left - containerRect.left}px`;
    canvas.style.top = `${videoRect.top - containerRect.top}px`;
    canvas.style.width = `${videoRect.width}px`;
    canvas.style.height = `${videoRect.height}px`;
    raiseControls();
  }

  return {
    container,
    zIndex: overlayZIndex,
    mount(canvas) {
      const view = container.ownerDocument?.defaultView;
      if (view?.getComputedStyle?.(container).position === 'static') container.style.position = 'relative';
      container.append(canvas);
      update(canvas);
      return () => {
        canvas.parentNode?.removeChild(canvas);
        container.style.position = originalContainerPosition;
        for (const [control, style] of raisedControls) {
          control.style.position = style.position;
          control.style.zIndex = style.zIndex;
        }
        raisedControls.clear();
      };
    },
    update,
  };
}

export function observePlayerChanges(document, callback, extraEvents = []) {
  const view = document.defaultView;
  const cleanups = [];
  const observedVideos = new WeakSet();
  let resizeObserver = null;
  let queued = false;
  const notify = () => {
    if (queued) return;
    queued = true;
    queueMicrotask(() => {
      queued = false;
      callback();
    });
  };
  const refreshResizeTargets = () => {
    if (!resizeObserver) return;
    for (const video of document.querySelectorAll('video')) {
      if (observedVideos.has(video)) continue;
      observedVideos.add(video);
      resizeObserver.observe(video);
    }
  };

  const MutationObserverClass = view?.MutationObserver ?? globalThis.MutationObserver;
  if (MutationObserverClass && document.documentElement) {
    const observer = new MutationObserverClass(() => {
      refreshResizeTargets();
      notify();
    });
    observer.observe(document.documentElement, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ['class', 'src'],
    });
    cleanups.push(() => observer.disconnect());
  }

  const ResizeObserverClass = view?.ResizeObserver ?? globalThis.ResizeObserver;
  if (ResizeObserverClass) {
    resizeObserver = new ResizeObserverClass(notify);
    refreshResizeTargets();
    cleanups.push(() => resizeObserver.disconnect());
  }

  for (const eventName of ['fullscreenchange', ...extraEvents]) {
    document.addEventListener?.(eventName, notify);
    cleanups.push(() => document.removeEventListener?.(eventName, notify));
  }
  for (const eventName of ['play', 'pause', 'emptied', 'loadedmetadata']) {
    document.addEventListener?.(eventName, notify, true);
    cleanups.push(() => document.removeEventListener?.(eventName, notify, true));
  }

  return () => cleanups.splice(0).forEach((cleanup) => cleanup());
}
