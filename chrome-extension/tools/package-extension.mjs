import { createHash } from 'node:crypto';
import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

import { verifyExtension } from './verify-extension.mjs';

const extensionRoot = path.resolve(import.meta.dirname, '..');
const outputPath = path.join(extensionRoot, 'dist', 'sonos-tab-caster.zip');
const EXCLUDED = new Set(['dist', 'node_modules', 'tests', 'tools']);
const RUNTIME_EXTENSIONS = new Set(['.css', '.html', '.ico', '.js', '.json', '.png', '.svg']);

const verificationErrors = await verifyExtension(extensionRoot);
if (verificationErrors.length > 0) {
  throw new Error(`Extension verification failed:\n${verificationErrors.join('\n')}`);
}

const entries = [];
for (const absolute of await runtimeFiles(extensionRoot)) {
  const name = path.relative(extensionRoot, absolute).split(path.sep).join('/');
  entries.push({ name, data: await readFile(absolute) });
}
entries.sort((left, right) => left.name.localeCompare(right.name));

const archive = createZip(entries);
await mkdir(path.dirname(outputPath), { recursive: true });
await writeFile(outputPath, archive);
const digest = createHash('sha256').update(archive).digest('hex');
process.stdout.write(`Created dist/sonos-tab-caster.zip (${archive.length} bytes, sha256 ${digest})\n`);

async function runtimeFiles(root) {
  const files = [];
  async function visit(directory) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      if (entry.isDirectory() && EXCLUDED.has(entry.name)) continue;
      const absolute = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        await visit(absolute);
      } else if (
        RUNTIME_EXTENSIONS.has(path.extname(entry.name)) &&
        entry.name !== 'package.json'
      ) {
        files.push(absolute);
      }
    }
  }
  await visit(root);
  return files;
}

function createZip(entries) {
  const localParts = [];
  const centralParts = [];
  let offset = 0;

  for (const entry of entries) {
    const name = Buffer.from(entry.name, 'utf8');
    const crc = crc32(entry.data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0x0800, 6);
    local.writeUInt16LE(0, 8);
    local.writeUInt16LE(0, 10);
    local.writeUInt16LE(0x0021, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(entry.data.length, 18);
    local.writeUInt32LE(entry.data.length, 22);
    local.writeUInt16LE(name.length, 26);
    local.writeUInt16LE(0, 28);
    localParts.push(local, name, entry.data);

    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x0800, 8);
    central.writeUInt16LE(0, 10);
    central.writeUInt16LE(0, 12);
    central.writeUInt16LE(0x0021, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(entry.data.length, 20);
    central.writeUInt32LE(entry.data.length, 24);
    central.writeUInt16LE(name.length, 28);
    central.writeUInt16LE(0, 30);
    central.writeUInt16LE(0, 32);
    central.writeUInt16LE(0, 34);
    central.writeUInt16LE(0, 36);
    central.writeUInt32LE(0, 38);
    central.writeUInt32LE(offset, 42);
    centralParts.push(central, name);
    offset += local.length + name.length + entry.data.length;
  }

  const centralDirectory = Buffer.concat(centralParts);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(0, 4);
  end.writeUInt16LE(0, 6);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(centralDirectory.length, 12);
  end.writeUInt32LE(offset, 16);
  end.writeUInt16LE(0, 20);
  return Buffer.concat([...localParts, centralDirectory, end]);
}

function crc32(data) {
  let crc = 0xffffffff;
  for (const byte of data) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}
