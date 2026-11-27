// All callers hold moq's filesystem lock. Publish the complete rollback record before changing
// the component. A subsequent invocation can recover even if the publisher was killed mid-rename.
import { existsSync, lstatSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { sha256 } from './js-kit.mjs';

export function destination(root, relative) {
  let path = root;
  for (const part of relative.split('/')) {
    if (!/^[\w.-]+$/.test(part) || part === '.' || part === '..') throw new Error('Invalid component path: ' + relative);
    path = join(path, part);
    if (lstatSync(path, {throwIfNoEntry: false})?.isSymbolicLink()) throw new Error('Symlink in component path: ' + relative);
  }
  return path;
}

function bytesAt(path) {
  return existsSync(path) ? readFileSync(path) : null;
}
const digest = bytes => bytes === null ? null : sha256(bytes);

function replace(path, bytes, temp, mode = 0o666) {
  if (bytes === null) {
    rmSync(path, {force: true});
    return;
  }
  mkdirSync(dirname(path), {recursive: true});
  writeFileSync(temp, bytes, {flag: 'wx', mode});
  renameSync(temp, path);
}

export function recover(root, cache) {
  const journal = join(cache, 'rollback.json');
  if (!existsSync(journal)) return;
  const backup = JSON.parse(readFileSync(journal));
  if (backup.root !== resolve(root)) throw new Error('Interrupted library change belongs to ' + backup.root);
  for (const {name, previous, intended, mode} of backup.files) {
    const path = destination(root, name);
    const old = previous === null ? null : Buffer.from(previous, 'base64');
    const current = digest(bytesAt(path));
    const temp = path + '.new-' + backup.token;
    if (current !== intended && current !== digest(old)) throw new Error('Edited during library recovery: ' + path);
    rmSync(temp, {force: true});
    if (current !== digest(old)) replace(path, old, temp, mode);
  }
  rmSync(journal);
}

export function publishFiles(root, files, cache) {
  mkdirSync(cache, {recursive: true, mode: 0o700});
  recover(root, cache);
  const backup = [];
  for (const [name, bytes] of files) {
    const path = destination(root, name);
    const old = bytesAt(path);
    if (digest(old) === digest(bytes)) continue;
    const mode = lstatSync(path, {throwIfNoEntry: false})?.mode;
    backup.push({name, previous: old?.toString('base64') ?? null, intended: digest(bytes), mode});
  }
  if (!backup.length) return;
  const journal = join(cache, 'rollback.json');
  const token = randomUUID();
  replace(journal, Buffer.from(JSON.stringify({root: resolve(root), token, files: backup})), journal + '.new-' + token, 0o600);
  try {
    for (const {name, mode} of backup) {
      const path = destination(root, name);
      replace(path, files.get(name), path + '.new-' + token, mode);
    }
    rmSync(journal);
  } catch (error) {
    recover(root, cache);
    throw error;
  }
}
