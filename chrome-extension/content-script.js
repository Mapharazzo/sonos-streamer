(() => {
  let latestStatus = { status: 'audio-only', code: 'loading' };
  let contextInvalidated = false;
  function statusDeliveryFailed(error) {
    if (contextInvalidated || !String(error?.message ?? error).includes('Extension context invalidated')) return;
    contextInvalidated = true;
    // Dispose after the current callback unwinds; stop() reports status too.
    void controllerPromise.then((controller) => controller.stop()).catch(() => {});
  }
  const controllerPromise = import(chrome.runtime.getURL('video/controller.js')).then(
    ({ VideoDelayController }) =>
      new VideoDelayController({
        onStatus(status) {
          latestStatus = status;
          if (contextInvalidated) return;
          try {
            void chrome.runtime.sendMessage({ type: 'video-delay-status', status }).catch(statusDeliveryFailed);
          } catch (error) {
            statusDeliveryFailed(error);
          }
        },
      }),
  );

  chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
    if (contextInvalidated) return undefined;
    if (!message || typeof message !== 'object') return undefined;
    if (message.type === 'recovery:ready') { sendResponse({ ready: true }); return undefined; }
    if (message.type === 'recovery:snapshot') {
      const video = document.querySelector('video');
      sendResponse(video ? { position: video.currentTime, paused: video.paused, seekable: Number.isFinite(video.duration) && !video.mediaKeys && video.seekable.length > 0 } : null);
      return undefined;
    }
    if (message.type === 'recovery:restore') {
      const video = document.querySelector('video'); const playback = message.playback;
      if (!video || !playback?.seekable || !Number.isFinite(playback.position) || !Number.isFinite(video.duration) || video.mediaKeys) { sendResponse({ restored: false }); return undefined; }
      let seekable = false;
      for (let index = 0; index < video.seekable.length; index += 1) if (playback.position >= video.seekable.start(index) && playback.position <= video.seekable.end(index)) seekable = true;
      if (!seekable) { sendResponse({ restored: false }); return undefined; }
      try { video.currentTime = playback.position; if (playback.paused) video.pause(); else void video.play().catch(() => {}); sendResponse({ restored: true }); }
      catch { sendResponse({ restored: false }); }
      return undefined;
    }
    if (!['video-delay:set', 'video-delay:stop', 'video-delay:get-status'].includes(message.type)) {
      return undefined;
    }
    controllerPromise.then((controller) => {
      if (contextInvalidated) { sendResponse(latestStatus); return; }
      if (message.type === 'video-delay:set') latestStatus = controller.setDelay(message.milliseconds);
      else if (message.type === 'video-delay:stop') latestStatus = controller.stop();
      else latestStatus = controller.getStatus();
      sendResponse(latestStatus);
    }).catch((error) => {
      latestStatus = {
        status: 'error',
        code: 'controller-load-failed',
        message: error instanceof Error ? error.message : 'Could not load the video delay controller',
      };
      sendResponse(latestStatus);
    });
    return true;
  });
})();
