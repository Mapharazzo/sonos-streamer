import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { shortcutStatus } from '../shared/recovery.js';
test('shortcut collision is explicitly unavailable', () => {
  assert.deepEqual(shortcutStatus([{ name: 'cast-current-tab', shortcut: '' }]), { available: false, shortcut: '' });
});
test('user remapping is respected', () => {
  assert.deepEqual(shortcutStatus([{ name: 'cast-current-tab', shortcut: 'Alt+Shift+9' }]), { available: true, shortcut: 'Alt+Shift+9' });
});
test('default cast command is scoped to Chrome', async () => {
  const manifest = JSON.parse(await readFile(new URL('../manifest.json', import.meta.url)));
  assert.equal(manifest.commands['cast-current-tab'].suggested_key.default, 'Ctrl+Shift+8');
  assert.notEqual(manifest.commands['cast-current-tab'].global, true);
});
