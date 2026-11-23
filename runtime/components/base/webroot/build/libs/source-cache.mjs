// Populate a portable npm cache. npm ci installs optional native tools only for its host.
import { npmCommand } from '../tools.mjs';
import { execFileSync } from 'node:child_process';
import { readFileSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const build = dirname(dirname(fileURLToPath(import.meta.url)));
const cache = process.argv[2];
const urls = new Set();
for (const project of ['libs', 'whiteboard']) {
  const lock = JSON.parse(readFileSync(join(build, project, 'package-lock.json')));
  for (const [name, pkg] of Object.entries(lock.packages)) {
    if (!name) continue;
    if (!pkg.integrity || !pkg.resolved?.startsWith('https://registry.npmjs.org/'))
      throw new Error('No reviewed registry tarball for ' + name);
    urls.add(pkg.resolved);
  }
}
const tarballs = [...urls].sort();
for (let i = 0; i < tarballs.length; i += 16) {
  execFileSync(npmCommand[0], [...npmCommand.slice(1), 'cache', 'add', '--ignore-scripts', '--cache', cache, ...tarballs.slice(i, i + 16)], { stdio: 'inherit' });
}
rmSync(join(cache, '_logs'), { recursive: true, force: true });
console.log('Cached', urls.size, 'locked tarballs including optional platform packages');
