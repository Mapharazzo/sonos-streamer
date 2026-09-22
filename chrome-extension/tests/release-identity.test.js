import test from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync } from 'node:crypto';
import { extensionIdentity, releaseIdentity } from '../tools/release-identity.mjs';

const key = 'MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEApPgL47WyXxb6yTMhgoDbcn1viTM+VwYI7QRBW/jlU0ekkYfuBXuH9UmX+giNhgcEuOtg2Pyck1GQIz7wZKGnFo6FeyXAn2mXk8LAaHm0Sd2dGlEk4op5pXK/Oow9mkHj1KwLGninZjAzKlB4MYZoqv0RodKhYUwg23Wcik5oFuMcbJuf1vx1W+4phXV8FrQdHaaVpteOjDP6KGy9XHjBt0p8HPLuWAV14T2iKHeMuT1ChF6EckFb6d4hM0/ySqnbnIw9wcZi4tLRsGrLOG3TrRZXA8OVayjw9+3qtcVUGAnlAWR55RrZOjeS9NUhgsObHaSeI9z849QaSjOMew4ZmwIDAQAB';
test('development identity is stable and agrees with the Windows helper', () => {
  assert.equal(extensionIdentity(key), 'bhlhfokccmkkadhhhhkamndjfaekehjg');
  assert.deepEqual(releaseIdentity({ key }, 'development').policy, { version: 1, channel: 'development', allowedOrigins: ['chrome-extension://bhlhfokccmkkadhhhhkamndjfaekehjg/'] });
  assert.throws(() => extensionIdentity('not-a-key'));
  assert.throws(() => extensionIdentity(Buffer.concat([Buffer.from(key, 'base64'), Buffer.from([0])]).toString('base64')));
});
test('production requires a distinct explicitly provided store identity', () => {
  assert.throws(() => releaseIdentity({ key }, 'production'));
  assert.throws(() => releaseIdentity({ key }, 'production', key));
  assert.throws(() => releaseIdentity({ key }, 'unknown'));
  const storeKey = generateKeyPairSync('rsa', { modulusLength: 2048 }).publicKey.export({ type: 'spki', format: 'der' }).toString('base64');
  const result = releaseIdentity({ key }, 'production', storeKey);
  assert.equal(result.manifest.key, storeKey);
  assert.deepEqual(result.policy.allowedOrigins, [`chrome-extension://${extensionIdentity(storeKey)}/`]);
});
