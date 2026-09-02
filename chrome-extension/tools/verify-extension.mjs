import { lstat, readFile, readdir, realpath } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const REQUIRED_PERMISSIONS = ['activeTab', 'offscreen', 'scripting', 'storage', 'tabCapture'];
const RUNTIME_EXTENSIONS = new Set(['.js', '.mjs', '.html', '.css']);
const EXCLUDED_DIRECTORIES = new Set(['dist', 'node_modules', 'tests', 'tools']);

export async function verifyExtension(extensionRoot) {
  const errors = [];
  const root = path.resolve(extensionRoot);
  let rootReal;
  try {
    rootReal = await realpath(root);
  } catch {
    return [`extension root does not exist: ${root}`];
  }

  let manifest;
  try {
    manifest = JSON.parse(await readFile(path.join(root, 'manifest.json'), 'utf8'));
  } catch (error) {
    return [`manifest.json could not be parsed: ${publicMessage(error)}`];
  }

  validateManifestShape(manifest, errors);
  const files = await listFiles(root);
  const relativeFiles = files.map((file) => toPosix(path.relative(root, file)));
  const referenced = collectManifestReferences(manifest, relativeFiles, errors);

  for (const reference of referenced) {
    await validateLocalPath(root, rootReal, reference, errors);
  }

  const runtimeFiles = files.filter((file) => RUNTIME_EXTENSIONS.has(path.extname(file)));
  for (const file of runtimeFiles) {
    const relative = toPosix(path.relative(root, file));
    await validateLocalPath(root, rootReal, relative, errors);
    const source = await readFile(file, 'utf8');
    validateExecutableSource(source, relative, errors);
    for (const reference of collectSourceReferences(source, file, root)) {
      await validateLocalPath(root, rootReal, reference, errors);
    }
  }

  return [...new Set(errors)].sort();
}

function validateManifestShape(manifest, errors) {
  if (manifest.manifest_version !== 3) errors.push('manifest_version must be 3');
  if (Number.parseInt(manifest.minimum_chrome_version, 10) < 116) {
    errors.push('minimum_chrome_version must be at least 116');
  }
  const actualPermissions = Array.isArray(manifest.permissions) ? [...manifest.permissions].sort() : [];
  if (JSON.stringify(actualPermissions) !== JSON.stringify([...REQUIRED_PERMISSIONS].sort())) {
    errors.push(`permissions must be exactly: ${REQUIRED_PERMISSIONS.join(', ')}`);
  }
  if (manifest.background?.type !== 'module') errors.push('background service worker must use module type');
}

function collectManifestReferences(manifest, files, errors) {
  const references = new Set();
  addReference(references, manifest.background?.service_worker);
  addReference(references, manifest.action?.default_popup);
  addReference(references, manifest.options_page);
  addReference(references, manifest.options_ui?.page);
  addReference(references, manifest.devtools_page);
  addReference(references, manifest.side_panel?.default_path);
  for (const value of Object.values(manifest.chrome_url_overrides ?? {})) addReference(references, value);
  for (const script of manifest.content_scripts ?? []) {
    for (const file of script.js ?? []) addReference(references, file);
    for (const file of script.css ?? []) addReference(references, file);
  }
  for (const page of manifest.sandbox?.pages ?? []) addReference(references, page);
  collectIconReferences(manifest.icons, references);
  collectIconReferences(manifest.action?.default_icon, references);

  for (const group of manifest.web_accessible_resources ?? []) {
    for (const pattern of group.resources ?? []) {
      const matches = files.filter((file) => globMatches(pattern, file));
      if (matches.length === 0) errors.push(`web-accessible resource pattern matches no files: ${pattern}`);
      matches.forEach((file) => references.add(file));
    }
  }
  return references;
}

function collectIconReferences(value, references) {
  if (typeof value === 'string') addReference(references, value);
  else if (value && typeof value === 'object') {
    for (const file of Object.values(value)) addReference(references, file);
  }
}

function addReference(references, value) {
  if (typeof value === 'string' && value.length > 0) references.add(value);
}

async function validateLocalPath(root, rootReal, reference, errors) {
  if (typeof reference !== 'string' || reference.length === 0) return;
  if (/^https?:\/\//i.test(reference)) {
    errors.push(`remote executable code is forbidden: ${reference}`);
    return;
  }
  const normalized = reference.replace(/^\//, '');
  const absolute = path.resolve(root, normalized);
  if (!isInside(root, absolute)) {
    errors.push(`runtime path escapes extension root: ${reference}`);
    return;
  }
  try {
    const metadata = await lstat(absolute);
    if (metadata.isSymbolicLink()) {
      errors.push(`runtime path must not be a symlink: ${reference}`);
      return;
    }
    if (!metadata.isFile()) errors.push(`runtime path is not a file: ${reference}`);
    const resolved = await realpath(absolute);
    if (!isInside(rootReal, resolved)) errors.push(`runtime path resolves outside extension root: ${reference}`);
  } catch {
    errors.push(`referenced runtime file is missing: ${reference}`);
  }
}

function validateExecutableSource(source, relative, errors) {
  const remotePatterns = [
    /\bimport\s*\(\s*['"]https?:\/\//i,
    /\b(?:import|export)\s+[^;]*?\bfrom\s*['"]https?:\/\//i,
    /\b(?:importScripts|Worker|SharedWorker)\s*\(\s*['"]https?:\/\//i,
    /<(?:script|link)\b[^>]*(?:src|href)\s*=\s*['"]https?:\/\//i,
    /\b(?:cdn\.jsdelivr\.net|unpkg\.com)\b/i,
  ];
  if (remotePatterns.some((pattern) => pattern.test(source))) {
    errors.push(`remote executable code found in ${relative}`);
  }
  if (/\b(?:node_modules|third[_-]party|vendor)\//i.test(source)) {
    errors.push(`third-party executable source reference found in ${relative}`);
  }

  for (const specifier of extractImportSpecifiers(source)) {
    if (/^https?:\/\//i.test(specifier)) continue;
    if (!specifier.startsWith('.') && !specifier.startsWith('/')) {
      errors.push(`bare package import is forbidden in ${relative}: ${specifier}`);
    }
  }
}

function collectSourceReferences(source, file, root) {
  const references = new Set();
  for (const specifier of extractImportSpecifiers(source)) {
    if (/^[a-z][a-z+.-]*:/i.test(specifier)) continue;
    const resolved = specifier.startsWith('/')
      ? specifier.slice(1)
      : path.relative(root, path.resolve(path.dirname(file), specifier));
    references.add(toPosix(resolved));
  }

  for (const match of source.matchAll(/\b(?:getURL|addModule)\s*\(\s*['"]([^'"]+)['"]/g)) {
    const base = match[0].startsWith('getURL') ? root : path.dirname(file);
    references.add(toPosix(path.relative(root, path.resolve(base, match[1]))));
  }
  for (const match of source.matchAll(/\bcreateDocument\s*\(\s*\{[\s\S]*?\burl\s*:\s*['"]([^'"]+)['"]/g)) {
    references.add(toPosix(path.relative(root, path.resolve(root, match[1]))));
  }
  if (path.extname(file) === '.html') {
    for (const match of source.matchAll(/<(?:script|link)\b[^>]*(?:src|href)\s*=\s*['"]([^'"]+)['"]/gi)) {
      if (/^(?:https?:|data:|#)/i.test(match[1])) continue;
      references.add(toPosix(path.relative(root, path.resolve(path.dirname(file), match[1]))));
    }
  }
  return references;
}

function extractImportSpecifiers(source) {
  const specifiers = [];
  const patterns = [
    /\bimport\s*\(\s*['"]([^'"]+)['"]\s*\)/g,
    /\b(?:import|export)\s+(?:[^'";]*?\s+from\s+)?['"]([^'"]+)['"]/g,
  ];
  for (const pattern of patterns) {
    for (const match of source.matchAll(pattern)) specifiers.push(match[1]);
  }
  return specifiers;
}

async function listFiles(root) {
  const files = [];
  async function visit(directory) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      if (entry.isDirectory() && EXCLUDED_DIRECTORIES.has(entry.name)) continue;
      const absolute = path.join(directory, entry.name);
      if (entry.isDirectory()) await visit(absolute);
      else files.push(absolute);
    }
  }
  await visit(root);
  return files;
}

function globMatches(pattern, value) {
  const escaped = pattern.replace(/[.+^${}()|[\]\\]/g, '\\$&').replaceAll('*', '.*');
  return new RegExp(`^${escaped}$`).test(value);
}

function isInside(root, candidate) {
  const relative = path.relative(root, candidate);
  return relative === '' || (!relative.startsWith('..') && !path.isAbsolute(relative));
}

function toPosix(value) {
  return value.split(path.sep).join('/');
}

function publicMessage(error) {
  return error instanceof Error ? error.message : String(error);
}

if (process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href === import.meta.url) {
  const root = path.resolve(import.meta.dirname, '..');
  const errors = await verifyExtension(root);
  if (errors.length > 0) {
    for (const error of errors) process.stderr.write(`- ${error}\n`);
    process.exitCode = 1;
  } else {
    process.stdout.write('Extension verification passed with zero errors.\n');
  }
}
