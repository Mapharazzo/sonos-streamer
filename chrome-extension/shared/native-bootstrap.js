const HOST = 'com.mapharazzo.elgato_remote';
const keysAre = (value, keys) => value && typeof value === 'object' && !Array.isArray(value) && Object.keys(value).sort().join(',') === keys;

export async function loadNativeBootstrap(chrome) {
  let reply;
  try { reply = await chrome.runtime.sendNativeMessage(HOST, { version: 1, type: 'get-bootstrap' }); }
  catch { throw new Error('app-unavailable'); }
  if (keysAre(reply, 'code,ok') && reply.ok === false) {
    throw new Error(['origin-denied', 'installation-unavailable', 'unsupported-version'].includes(reply.code) ? `native-${reply.code}` : 'native-invalid-response');
  }
  const b = reply?.value;
  if (!keysAre(reply, 'ok,value') || reply.ok !== true || !keysAre(b, 'controlUrl,installationId,managedProtocol,token,version') ||
      b.version !== 1 || b.managedProtocol !== 1 || b.controlUrl !== 'ws://127.0.0.1:5902/control/v1' ||
      typeof b.installationId !== 'string' || !/^[a-z0-9][a-z0-9._-]{0,95}$/.test(b.installationId) ||
      typeof b.token !== 'string' || !/^[a-fA-F0-9]{64}$/.test(b.token)) throw new Error('native-invalid-response');
  return b;
}
