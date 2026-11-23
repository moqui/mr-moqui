// Maintenance commands. Use the same two lockfiles that build-js installs.
import { execFileSync, spawnSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const source = dirname(fileURLToPath(import.meta.url));
const projects = ['libs', 'whiteboard'];
const output = resolve(source, '../../screen/webroot/libs');
const read = file => JSON.parse(readFileSync(file));
const [task, ...args] = process.argv.slice(2);
const cutoff = new Date(Date.now() - 14 * 86400000).toISOString();
const manifest = read(join(output, 'MANIFEST.json'));

function npm(argv, cwd, capture = false) {
  return execFileSync('npm', argv, { cwd, encoding: 'utf8', stdio: capture ? ['ignore', 'pipe', 'inherit'] : 'inherit' });
}

if (task === 'audit') {
  if (args.length) throw new Error('audit-js takes no arguments');
  for (const project of projects) {
    console.log('Auditing', project);
    const result = spawnSync('npm', ['audit', '--package-lock-only'], { cwd: join(source, '..', project), stdio: 'inherit' });
    if (result.status !== 0) process.exitCode = 1;
  }
  const diagram = readFileSync(join(output, 'mermaid/mermaid.min.js'), 'utf8');
  const embedded = {};
  for (const [pkg, pattern] of [['dompurify', /DOMPurify ([\d.]+)/], ['katex', /"([\d.]+)",\w+=\{Span:/]]) {
    const version = diagram.match(pattern)?.[1];
    if (!version) throw new Error('Review embedded ' + pkg + ' version in Mermaid');
    console.log('Mermaid bundles', pkg, version);
    embedded[pkg] = [version];
  }
  const response = await fetch('https://registry.npmjs.org/-/npm/v1/security/advisories/bulk',
    { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(embedded) });
  if (!response.ok) throw new Error('Bundled library audit returned ' + response.status);
  for (const [pkg, advisories] of Object.entries(await response.json())) {
    for (const item of advisories) console.error(pkg, item.severity, item.title, item.url);
    process.exitCode = 1;
  }
} else if (task === 'outdated') {
  if (args.length) throw new Error('outdated-js takes no arguments');
  for (const project of projects) {
    const cwd = join(source, '..', project);
    for (const before of [[], ['--before=' + cutoff]]) {
      console.log(project, ...before);
      const result = spawnSync('npm', ['outdated', '--package-lock-only', ...before], { cwd, stdio: 'inherit' });
      if (result.error || ![0, 1].includes(result.status)) throw result.error || new Error('npm outdated failed');
    }
  }
} else if (task === 'update') {
  const security = args[0] === '--security';
  if (security) args.shift();
  const all = args.length === 1 && args[0] === '--all';
  if ((!all && (args.length !== 2 || !/^\d+\.\d+\.\d+(?:-[\w.-]+)?$/.test(args[1]))) || (all && security))
    throw new Error('Use update-js [--security] <package> <exact-version> or update-js --all');
  const lib = manifest.libraries.find(lib => lib.id === args[0]);
  const name = lib?.npm?.replace(/@[^@]+$/, '') || args[0];
  const candidate = mkdtempSync(join(process.env.JS_CACHE, 'update-'));
  let found = false;
  try {
    for (const project of projects) {
      const cwd = join(candidate, project);
      mkdirSync(cwd);
      for (const file of ['package.json', 'package-lock.json']) cpSync(join(source, '..', project, file), join(cwd, file));
      const pins = read(join(cwd, 'package.json')).dependencies;
      if (!all && !Object.hasOwn(pins, name)) continue;
      found = true;
      const updates = all ? Object.keys(pins).map(pkg => pkg + '@*') : [name + '@' + args[1]];
      npm(['install', '--package-lock-only', '--save-exact', '--ignore-scripts', '--no-audit', '--no-fund',
        ...(security ? [] : ['--before=' + cutoff]), ...updates], cwd);
    }
    if (!found) throw new Error('Unknown direct dependency ' + name);
    execFileSync(process.execPath, [join(source, 'refresh.mjs'), '--write'], {
      stdio: 'inherit', env: { ...process.env, JS_PROJECTS: candidate }
    });
    for (const project of projects) for (const file of ['package.json', 'package-lock.json']) {
      const dest = join(source, '..', project, file);
      cpSync(join(candidate, project, file), dest + '.new');
      renameSync(dest + '.new', dest);
    }
  } finally { rmSync(candidate, { recursive: true, force: true }); }
} else throw new Error('Unknown maintenance task');
