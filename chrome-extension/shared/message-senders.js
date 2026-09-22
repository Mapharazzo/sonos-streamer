import { isSupportedMediaUrl } from './background-state.js';

function own(sender, origin) {
  return /^chrome-extension:\/\/[a-p]{32}\/$/.test(origin) && sender?.id === origin.slice(19, -1);
}
export function allowedBackgroundSender(sender, extensionOrigin) {
  return own(sender, extensionOrigin) && !sender.tab && !sender.documentId &&
    (sender.url === undefined || sender.url === extensionOrigin + 'background.js');
}
export function allowedSender(message, sender, { extensionOrigin, capturedTabId, plexOrigins = [] }) {
  if (!own(sender, extensionOrigin) || !message || typeof message !== 'object') return false;
  if (['state-request', 'popup-command'].includes(message.type)) return !sender.tab && sender.url === extensionOrigin + 'popup.html';
  if (['offscreen-event', 'managed-offscreen-event'].includes(message.type)) return !sender.tab && sender.url === extensionOrigin + 'offscreen.html';
  return message.type === 'video-delay-status' && Number.isInteger(capturedTabId) && sender.tab?.id === capturedTabId &&
    sender.frameId === 0 && isSupportedMediaUrl(sender.url, plexOrigins);
}
