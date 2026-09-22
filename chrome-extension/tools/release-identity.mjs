import { createHash, createPublicKey } from 'node:crypto';

export function extensionIdentity(key) {
  if (typeof key !== 'string' || key.length > 8192) throw new Error('Invalid extension public key');
  const der = Buffer.from(key, 'base64');
  if (der.toString('base64') !== key) throw new Error('Invalid extension public key');
  const canonical = createPublicKey({ key: der, type: 'spki', format: 'der' }).export({ type: 'spki', format: 'der' });
  if (!canonical.equals(der)) throw new Error('Invalid extension public key');
  return createHash('sha256').update(der).digest('hex').slice(0, 32).replace(/[0-9a-f]/g, (nibble) => String.fromCharCode(97 + Number.parseInt(nibble, 16)));
}

export function releaseIdentity(manifest, channel, storeKey) {
  if (!['development', 'production'].includes(channel)) throw new Error('Unknown release channel');
  const developmentID = extensionIdentity(manifest.key);
  let key = manifest.key;
  if (channel === 'production') {
    if (!storeKey || extensionIdentity(storeKey) === developmentID) throw new Error('Production requires a distinct store public key');
    key = storeKey;
  } else if (storeKey) throw new Error('Store key is only valid for production');
  return {
    manifest: { ...manifest, key },
    policy: { version: 1, channel, allowedOrigins: [`chrome-extension://${extensionIdentity(key)}/`] },
  };
}
