// Standard component bundles. npm installs in a private temporary project and esbuild reports the
// inputs it actually shipped. Both targets share a lock, but use their own resolution conditions.
import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { esbuildBinary, installProject, run, sha256, withWork } from './js-kit.mjs';

export const json = value => JSON.stringify(value, null, 2) + '\n';
export const read = path => JSON.parse(readFileSync(path, 'utf8'));
export const identifier = /^[A-Za-z0-9][A-Za-z0-9_-]*$/;
export const entryName = /^(?:@[a-z0-9][a-z0-9._-]*\/)?[a-z0-9][a-z0-9._-]*(?:\/[a-zA-Z0-9][a-zA-Z0-9._-]*)*$/;
export const exactVersion = /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[A-Za-z0-9.-]+)?(?:\+[A-Za-z0-9.-]+)?$/;
export const pastCooldown = (date, cutoff) => Number.isFinite(Date.parse(date)) && Date.parse(date) <= Date.parse(cutoff);
export const packageOf = entry => entry.split('/').slice(0, entry.startsWith('@') ? 2 : 1).join('/');
const permitted = new Set(['MIT','Apache-2.0','BSD-2-Clause','BSD-3-Clause','ISC','0BSD','Unlicense','Zlib',
  'BSL-1.0','PostgreSQL','Unicode-3.0','CDLA-Permissive-2.0','CC0-1.0','MIT-0']);

// An AND requires every term, an OR one permitted choice. Unknown syntax requires manual review.
export function licenseAllowed(expression) {
  const tokens = String(expression).match(/\(|\)|[^\s()]+/g) || [];
  let index = 0;
  function term() {
    const token = tokens[index++];
    if (token !== '(') {
      if (!token || ['AND','OR',')'].includes(token)) throw new Error('missing license');
      return permitted.has(token);
    }
    const result = alternatives();
    if (tokens[index++] !== ')') throw new Error('missing parenthesis');
    return result;
  }
  function conjunction() {
    let accepted = term();
    while (tokens[index] === 'AND') { index++; accepted = term() && accepted; }
    return accepted;
  }
  function alternatives() {
    let accepted = conjunction();
    while (tokens[index] === 'OR') { index++; accepted = conjunction() || accepted; }
    return accepted;
  }
  try { return alternatives() && index === tokens.length; } catch { return false; }
}

function packageNotice(directory, locked) {
  const pkg = read(join(directory, 'package.json'));
  if (!licenseAllowed(pkg.license)) throw new Error(pkg.name + ': license needs review: ' + pkg.license);
  const texts = readdirSync(directory, {withFileTypes: true}).filter(file => file.isFile() &&
    /^(?:licen[cs]e|copying|notice)(?:$|[.-])/i.test(file.name)).map(file => file.name).sort();
  if (!texts.some(name => /^(licen[cs]e|copying)/i.test(name))) throw new Error(pkg.name + ': missing license text');
  if (!locked?.integrity) throw new Error(pkg.name + ': missing lock integrity');
  return { name: pkg.name, version: pkg.version, license: pkg.license, integrity: locked.integrity,
    notice: texts.map(file => readFileSync(join(directory, file), 'utf8').trim()).join('\n\n') };
}

// esbuild's output graph distinguishes shipped modules from packages consulted during resolution.
function bundle(entry, target, installed, work) {
  const meta = join(work, 'inputs.json');
  const outfile = join(work, 'bundle.js');
  const client = target === 'client';
  const flags = [
    '--bundle', '--format=iife', '--global-name=libraryExports', '--target=es2020', '--log-level=warning',
    client ? '--platform=browser' : '--platform=neutral',
    client ? '--main-fields=browser,module,main' : '--main-fields=module,main',
    '--metafile=' + meta, '--outfile=' + outfile,
  ];
  try {
    run(esbuildBinary(join(installed, 'node_modules')), flags, {
      cwd: installed, stdio: ['pipe', 'pipe', 'inherit'],
      input: 'import * as api from ' + JSON.stringify(entry) + ';module.exports=api;',
    });
  } catch (error) {
    throw new Error(entry + ' could not be bundled for ' + target, {cause: error});
  }
  const emitted = Object.values(read(meta).outputs);
  if (emitted.length !== 1 || emitted[0].imports.length) {
    throw new Error(entry + ' must produce one self-contained JS file for ' + target);
  }
  return {text: readFileSync(outfile, 'utf8'), inputs: emitted[0].inputs};
}

export function buildLibraries({component, config, project, work, validator}) {
  work = resolve(work);
  const lock = read(join(project, 'package-lock.json')).packages;
  for (const [name, item] of Object.entries(lock)) {
    if (name && (!item.integrity || !item.resolved?.startsWith('https://registry.npmjs.org/'))) {
      throw new Error('No reviewed registry tarball for ' + name);
    }
  }
  const pins = read(join(project, 'package.json')).dependencies;
  for (const [name, version] of Object.entries(pins)) {
    if (!exactVersion.test(version) || lock[''].dependencies[name] !== version ||
        lock['node_modules/' + name]?.version !== version) throw new Error('Expected an exactly locked version of ' + name);
  }
  const packageInfo = new Map();
  const installed = installProject(project, join(work, 'npm'));
  const outputs = new Map();
  const libraries = [];
  const notices = new Map();
  const seen = new Set();
  for (const {id, entry, targets} of config.libraries) {
    if (!identifier.test(id || '') || seen.has(id) || !entryName.test(entry || '') || !Object.hasOwn(pins, packageOf(entry)) ||
        !Array.isArray(targets) || !targets.length || targets.some(t => !['server', 'client'].includes(t)) ||
        new Set(targets).size !== targets.length) throw new Error('Invalid library declaration: ' + id);
    seen.add(id);
    const files = [], packages = new Map();
    for (const target of targets) {
      const {text, inputs} = bundle(entry, target, installed, work);
      for (const [input, usage] of Object.entries(inputs)) {
        if (input === '<stdin>' || !usage.bytesInOutput) continue;
        if (!input.startsWith('node_modules/') || input.split('/').includes('..')) throw new Error('Unexpected bundled file: ' + input);
        let dir = dirname(input);
        while (dir !== '.' && !lock[dir]) dir = dirname(dir);
        if (!lock[dir]) throw new Error('Unidentified bundled package: ' + input);
        if (!packageInfo.has(dir)) packageInfo.set(dir, packageNotice(join(installed, dir), lock[dir]));
        const item = packageInfo.get(dir);
        const key = item.name + '@' + item.version;
        packages.set(key, {name: item.name, version: item.version, license: item.license, integrity: item.integrity});
        notices.set(key, item.notice);
      }
      const expression = '(() => {\n' + text + '\nreturn libraryExports;\n})()';
      const path = id + '.js';
      // The server module's first line marks it for the script runtime, which keeps its value for the run
      // (COMPONENT_LIBRARY_MARKER in moqui-core's script/runtime/facades.rs).
      const marker = '/* moqui component library ' + component + '/' + id + ' */\n';
      const bytes = Buffer.from(target === 'server' ? marker + expression + '\n' :
        '(moqui.libraries ||= Object.create(null))[' + JSON.stringify(component + '/' + id) + '] = ' + expression + ';\n');
      if (target === 'server' && validator) {
        const check = join(work, path);
        writeFileSync(check, bytes);
        try { run(validator, ['check-js-module', check], {timeout: 10000}); }
        catch (error) { throw new Error(id + ' is incompatible with the QuickJS server target', {cause: error}); }
      }
      outputs.set(target + '/' + path, bytes);
      files.push({target, path, sha256: sha256(bytes), size: bytes.length});
    }
    const name = packageOf(entry);
    libraries.push({id, npm: name + '@' + pins[name], files, packages: [...packages.keys()].sort().map(key => packages.get(key))});
  }
  const notice = Buffer.from([...notices.keys()].sort().map(name => '## ' + name + '\n\n' + notices.get(name)).join('\n\n') + '\n');
  outputs.set('client/NOTICES.md', notice);
  outputs.set('client/MANIFEST.json', Buffer.from(json({libraries, notices: {path: 'NOTICES.md', sha256: sha256(notice)}})));
  return outputs;
}

export function outputPaths(config) {
  if (!/^screen\/(?:[\w-]+\/)*[\w-]+$/.test(config.output)) throw new Error('Expected an output directory under a screen');
  return {client: config.output, server: 'script/libs'};
}

export function componentFiles(component, config, project, validator) {
  const bases = outputPaths(config);
  const files = withWork(null, work => buildLibraries({component, config, project, work, validator}));
  return new Map([...files].map(([name, bytes]) => {
    const [target, file] = name.split('/');
    return [bases[target] + '/' + file, bytes];
  }));
}

if (resolve(process.argv[1] || '') === fileURLToPath(import.meta.url)) {
  const build = process.env.JS_BUILD_DIR;
  const root = dirname(build);
  const config = read(join(build, 'js-build.json'));
  const project = join(process.env.JS_PROJECTS || build, 'libraries');
  const writing = process.argv.includes('--write');
  if (writing && config.libraries.some(lib => lib.targets.includes('server')) && !process.env.JS_VALIDATOR)
    throw new Error('A server target is checked by a built moqui binary (moqui check-js-module): run ./moq build, or set JS_VALIDATOR');
  const files = componentFiles(basename(root), config, project, writing ? process.env.JS_VALIDATOR : null);
  if (writing && process.env.JS_PROJECTS) {
    for (const file of ['package.json','package-lock.json']) {
      files.set('build/libraries/' + file, readFileSync(join(project, file)));
    }
  }
  const manifest = config.output + '/MANIFEST.json';
  if (!writing && !files.get(manifest).equals(readFileSync(join(root, manifest)))) throw new Error('Rebuilt libraries differ from the reviewed manifest');
  if (process.argv.includes('--verify')) {
    for (const [name, bytes] of files) {
      const path = join(root, name);
      if ((name.endsWith('.md') || existsSync(path)) && !bytes.equals(readFileSync(path))) {
        throw new Error('Installed library output differs from its reviewed build: ' + name);
      }
    }
  } else {
    const {publishFiles} = await import('./library-changes.mjs');
    publishFiles(root, files, process.env.JS_CACHE);
  }
  console.log((process.argv.includes('--verify') ? 'Verified ' : 'Built ') + config.libraries.length + ' component libraries');
}
