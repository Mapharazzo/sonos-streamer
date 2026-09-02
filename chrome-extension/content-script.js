(() => {
  let latestStatus = { status: 'audio-only', code: 'loading' };
  const controllerPromise = import(chrome.runtime.getURL('video/controller.js')).then(
    ({ VideoDelayController }) =>
      new VideoDelayController({
        onStatus(status) {
          latestStatus = status;
          void chrome.runtime.sendMessage({ type: 'video-delay-status', status }).catch(() => {});
        },
      }),
  );

  chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
    if (!message || typeof message !== 'object') return undefined;
    if (!['video-delay:set', 'video-delay:stop', 'video-delay:get-status'].includes(message.type)) {
      return undefined;
    }
    controllerPromise.then((controller) => {
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
