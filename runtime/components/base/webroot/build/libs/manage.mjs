// Maintenance commands (audit-js, outdated-js, update-js) on the same lockfiles build-js installs. ./moq runs them for
// one component at a time: JS_BUILD_DIR is its build/, JS_PROJECT_NAMES its npm projects there, JS_OUTPUT its served
// directory and JS_SCRIPT its build script (build/js-build.json). With none set, the webroot's.
import { npmCommand } from '../tools.mjs';
import { execFileSync, spawnSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const source = dirname(fileURLToPath(import.meta.url));
const build = resolve(process.env.JS_BUILD_DIR || dirname(source));
const projects = (process.env.JS_PROJECT_NAMES || 'libs,whiteboard').split(',').filter(Boolean);
const output = resolve(process.env.JS_OUTPUT || join(source, '../../screen/webroot/libs'));
const script = resolve(process.env.JS_SCRIPT || join(source, 'refresh.mjs'));
const webroot = build === dirname(source);
const read = file => JSON.parse(readFileSync(file));
const [task, ...args] = process.argv.slice(2);
const cutoff = new Date(Date.now() - 14 * 86400000).toISOString();
// Dependencies inlined in a library's published dist file, which the lockfiles cannot see. pattern reads the inlined
// version from the file; null lists one that carries no version, for review by hand. Mermaid, Tiptap, CodeMirror,
// OpenUI and the whiteboard are bundled here from the locks, so npm audit covers what they contain.
const BUNDLED_COPIES = !webroot ? [] : [
  { lib: 'chartjs', file: 'chartjs/chart.umd.min.js', pkg: '@kurkle/color', pattern: /@kurkle\/color v(\d+\.\d+\.\d+)/ },
  { lib: 'drauu', file: 'drauu/drauu.js', pkg: 'perfect-freehand', pattern: /perfect-freehand@(\d+\.\d+\.\d+)\// },
  { lib: 'drauu', file: 'drauu/drauu.js', pkg: 'nanoevents', pattern: /nanoevents@(\d+\.\d+\.\d+)\// },
  ...['rrweb-snapshot', 'rrdom', 'mitt', 'base64-arraybuffer', '@xstate/fsm']
    .map(pkg => ({ lib: 'rrweb', file: 'rrweb/rrweb.umd.min.js', pkg, pattern: null })),
];

function npm(argv, cwd, capture = false) {
  return execFileSync(npmCommand[0], [...npmCommand.slice(1), ...argv], { cwd, encoding: 'utf8', stdio: capture ? ['ignore', 'pipe', 'inherit'] : 'inherit' });
}

if (task === 'audit') {
  if (args.length) throw new Error('audit-js takes no arguments');
  for (const project of projects) {
    console.log('Auditing', project);
    const result = spawnSync(npmCommand[0], [...npmCommand.slice(1), 'audit', '--package-lock-only'], { cwd: join(build, project), stdio: 'inherit' });
    if (result.status !== 0) process.exitCode = 1;
  }
  // The lockfiles name what the build bundles itself. A library copied as published may carry its own copy of a
  // dependency at a version the lock does not show, so read that version from the distributed file.
  const embedded = {};
  for (const { lib, file, pkg, pattern } of BUNDLED_COPIES) {
    const text = readFileSync(join(output, file), 'utf8');
    if (!pattern) { console.log(lib, 'bundles', pkg, 'with no version in', file + ': review by hand'); continue; }
    const version = text.match(pattern)?.[1];
    if (!version) throw new Error('Update BUNDLED_COPIES: no ' + pkg + ' version found in ' + file);
    console.log(lib, 'bundles', pkg, version);
    (embedded[pkg] ??= []).push(version);
  }
  if (Object.keys(embedded).length) {
    const response = await fetch('https://registry.npmjs.org/-/npm/v1/security/advisories/bulk',
      { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(embedded) });
    if (!response.ok) throw new Error('Bundled library audit returned ' + response.status);
    for (const [pkg, advisories] of Object.entries(await response.json())) {
      for (const item of advisories) console.error(pkg, item.severity, item.title, item.url);
      process.exitCode = 1;
    }
  }
} else if (task === 'outdated') {
  if (args.length) throw new Error('outdated-js takes no arguments');
  // npm outdated reads an installed tree, which the source directories do not have. Compare each exact pin with the
  // registry instead: the newest release ^pin admits and the newest overall, each also as of the cooldown cutoff.
  const stable = v => /^\d+\.\d+\.\d+$/.test(v);
  const parts = v => v.split('.').map(Number);
  const newer = (a, b) => { const [x, y] = [parts(a), parts(b)]; for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] > y[i]; return false; };
  const newest = list => list.reduce((best, v) => (!best || newer(v, best) ? v : best), null);
  const rows = [];
  for (const project of projects) {
    const pins = Object.entries(read(join(build, project, 'package.json')).dependencies);
    for (let i = 0; i < pins.length; i += 8) {
      await Promise.all(pins.slice(i, i + 8).map(async ([pkg, pin]) => {
        const response = await fetch('https://registry.npmjs.org/' + pkg.replace('/', '%2F'));
        if (!response.ok) throw new Error('Registry returned ' + response.status + ' for ' + pkg);
        const { time } = await response.json();
        const all = Object.keys(time).filter(stable);
        const cooled = all.filter(v => time[v] <= cutoff);
        // What ^pin admits: the same major, or for 0.x the same minor (0.0.x: the same patch).
        const p = parts(pin);
        const lead = p[0] ? 1 : p[1] ? 2 : 3;
        const caret = list => list.filter(v => parts(v).slice(0, lead).every((n, i) => n === p[i]));
        const row = { project, pkg, pin, inMajor: newest(caret(cooled)), latest: newest(cooled), newest: newest(all) };
        if ([row.inMajor, row.latest, row.newest].some(v => v && newer(v, pin))) rows.push(row);
      }));
    }
  }
  if (!rows.length) console.log('Every pin is the newest release.');
  else {
    const table = [['Project', 'Package', 'Pinned', 'Compatible, cooled', 'Latest, cooled', 'Newest'],
      ...rows.sort((a, b) => a.project.localeCompare(b.project) || a.pkg.localeCompare(b.pkg))
        .map(r => [r.project, r.pkg, r.pin, r.inMajor ?? '-', r.latest ?? '-', r.newest ?? '-'])];
    const widths = table[0].map((_, c) => Math.max(...table.map(row => row[c].length)));
    for (const row of table) console.log(row.map((cell, c) => cell.padEnd(widths[c])).join('  ').trimEnd());
    console.log('Cooled: published before ' + cutoff.slice(0, 10) + '. update-js --all takes "Compatible, cooled" (^pin).');
  }
} else if (task === 'update') {
  const security = args[0] === '--security';
  if (security) args.shift();
  const all = args.length === 1 && args[0] === '--all';
  if ((!all && (args.length !== 2 || !/^\d+\.\d+\.\d+(?:-[\w.-]+)?$/.test(args[1]))) || (all && security))
    throw new Error('Use update-js [--security] <package> <exact-version> or update-js --all');
  const manifest = read(join(output, 'MANIFEST.json'));
  const lib = manifest.libraries.find(lib => lib.id === args[0]);
  const name = lib?.npm?.replace(/@[^@]+$/, '') || args[0];
  const candidate = mkdtempSync(join(process.env.JS_CACHE, 'update-'));
  let found = false;
  try {
    for (const project of projects) {
      const cwd = join(candidate, project);
      mkdirSync(cwd);
      for (const file of ['package.json', 'package-lock.json']) cpSync(join(build, project, file), join(cwd, file));
      const pins = read(join(cwd, 'package.json')).dependencies;
      if (!all && !Object.hasOwn(pins, name)) continue;
      found = true;
      // --all takes what ^pin admits: the same major, or the same minor of a 0.x. Anything past that is its own reviewed
      // update-js <package> <version>.
      const updates = all ? Object.entries(pins).map(([pkg, pin]) => pkg + '@^' + pin) : [name + '@' + args[1]];
      npm(['install', '--package-lock-only', '--save-exact', '--ignore-scripts', '--no-audit', '--no-fund',
        ...(security ? [] : ['--before=' + cutoff]), ...updates], cwd);
    }
    if (!found) throw new Error('Unknown direct dependency ' + name);
    // A component with font packs (the webroot) records each pack's locked source in its fonts.json.
    const catalogFile = join(build, 'fonts/fonts.json');
    const newCatalog = join(candidate, 'fonts/fonts.json');
    if (existsSync(catalogFile)) {
      const catalog = read(catalogFile);
      const locks = projects.map(project => read(join(candidate, project, 'package-lock.json')).packages);
      for (const pack of Object.values(catalog)) for (const font of pack.sources) {
        const pkg = locks.map(lock => lock['node_modules/' + font.package]).find(Boolean);
        if (!pkg) throw new Error('Missing locked font source ' + font.package);
        font.version = pkg.version;
        font.integrity = pkg.integrity;
      }
      mkdirSync(join(candidate, 'fonts'));
      writeFileSync(newCatalog, JSON.stringify(catalog, null, 2) + '\n');
    }
    execFileSync(process.execPath, [script, '--write'], {
      stdio: 'inherit', env: { ...process.env, JS_PROJECTS: candidate, JS_OUTPUT: output }
    });
    if (existsSync(catalogFile)) {
      cpSync(newCatalog, catalogFile + '.new');
      renameSync(catalogFile + '.new', catalogFile);
    }
    for (const project of projects) for (const file of ['package.json', 'package-lock.json']) {
      const dest = join(build, project, file);
      cpSync(join(candidate, project, file), dest + '.new');
      renameSync(dest + '.new', dest);
    }
  } finally { rmSync(candidate, { recursive: true, force: true }); }
} else throw new Error('Unknown maintenance task');
