import test from 'node:test';
import assert from 'node:assert/strict';
import { allowedSender, allowedBackgroundSender } from '../shared/message-senders.js';
const id = 'a'.repeat(32), extensionOrigin = `chrome-extension://${id}/`;
const context = { extensionOrigin, capturedTabId: 42, plexOrigins: [] };
test('only own popup may request state or commands', () => {
  const message = { type: 'popup-command', command: 'pair', token: 'untrusted' };
  assert.equal(allowedSender(message, { id, url: extensionOrigin+'popup.html' }, context), true);
  for (const sender of [{}, { id, tab: { id: 42 }, url: 'https://www.youtube.com/watch?v=x' }, { id, tab: { id: 42 }, url: extensionOrigin+'popup.html' }, { id: 'b'.repeat(32), url: extensionOrigin+'popup.html' }, { id, url: extensionOrigin+'popup.html?fake' }]) assert.equal(allowedSender(message, sender, context), false);
});
test('offscreen events and background commands are restricted to their actual contexts', () => {
  assert.equal(allowedSender({ type: 'managed-offscreen-event' }, { id, url: extensionOrigin+'offscreen.html' }, context), true);
  assert.equal(allowedSender({ type: 'managed-offscreen-event' }, { id, tab: { id: 42 }, url: 'https://www.youtube.com/' }, context), false);
  assert.equal(allowedBackgroundSender({ id, url: extensionOrigin+'background.js' }, extensionOrigin), true);
  assert.equal(allowedBackgroundSender({ id }, extensionOrigin), true); // Worker has no document URL on some Chrome versions.
  assert.equal(allowedBackgroundSender({ id, url: extensionOrigin+'popup.html' }, extensionOrigin), false);
  assert.equal(allowedBackgroundSender({ id, tab: { id: 42 } }, extensionOrigin), false);
});
test('video status is accepted only from the captured top-level supported tab', () => {
  const message = { type: 'video-delay-status' }, sender = { id, tab: { id: 42 }, frameId: 0, url: 'https://www.youtube.com/watch?v=x' };
  assert.equal(allowedSender(message, sender, context), true);
  for (const changed of [{ tab: { id: 43 } }, { frameId: 1 }, { url: 'https://evil.test/' }]) assert.equal(allowedSender(message, { ...sender, ...changed }, context), false);
  assert.equal(allowedSender(message, sender, { ...context, capturedTabId: null }), false);
});
