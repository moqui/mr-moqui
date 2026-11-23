// Populate a portable npm cache from the given npm project directories (each with a package-lock.json), by default the
// webroot's two. npm ci installs optional native tools only for its host, so this adds every locked tarball.
import { npmCommand } from '../tools.mjs';
import { execFileSync } from 'node:child_process';
import { readFileSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const build = dirname(dirname(fileURLToPath(import.meta.url)));
const [cache, ...dirs] = process.argv.slice(2);
const urls = new Set();
for (const project of dirs.length ? dirs : ['libs', 'whiteboard'].map(name => join(build, name))) {
  const lock = JSON.parse(readFileSync(join(project, 'package-lock.json')));
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
