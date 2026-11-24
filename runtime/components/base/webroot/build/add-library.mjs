// Shared CLI and Dev AIDE operations. Only the validated registry name and exact release go to npm.
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { componentFiles, entryName, exactVersion, identifier, json, licenseAllowed, packageOf, pastCooldown, read } from './component-libs.mjs';
import { destination, publishFiles, recover } from './library-changes.mjs';
import { npmCommand, run, sha256 } from './js-kit.mjs';

const [action,component,...args] = process.argv.slice(2);
const sets = process.env.JS_COMPONENTS, cacheRoot = process.env.JS_CACHE;
if (!identifier.test(component || '')) throw new Error('Expected a component name');
const matches = readdirSync(sets).map(set => join(sets,set,component)).filter(root => existsSync(join(root,'component.xml')));
if (matches.length !== 1) throw new Error('Expected one checked-out component named ' + component);
const root = realpathSync(matches[0]), cache = join(cacheRoot,component);
if (action !== 'recover' && process.env.JS_EXPECT_ROOT && root !== realpathSync(process.env.JS_EXPECT_ROOT)) throw new Error('Component checkout changed');
const configPath = 'build/js-build.json', pkgPath = 'build/libraries/package.json', lockPath = 'build/libraries/package-lock.json';
const owner = process.env.JS_REVIEW_USER || '';
const libraryRoot = dirname(fileURLToPath(import.meta.url));
const cutoff = new Date(Date.now()-14*86400000).toISOString();

async function registry(path) {
  const url = new URL(path, 'https://registry.npmjs.org');
  const response = await fetch(url,{signal:AbortSignal.timeout(15000)});
  if (!response.ok) throw new Error('npm registry: HTTP ' + response.status);
  return response.json();
}
const configFile = destination(root,configPath);
function configOf() {
  if (existsSync(configFile)) return read(configFile);
  return {output:'screen/' + component + '/libs', projects:['libraries'], libraries:[]};
}
function contents(name) {
  const path = destination(root,name);
  return existsSync(path) ? readFileSync(path) : null;
}
function digest(name) {
  const bytes = contents(name);
  return bytes ? sha256(bytes) : null;
}
function reviewDiff(name, before, after, scratch) {
  const a = join(scratch,'before'), b = join(scratch,'after');
  writeFileSync(a,before || ''); writeFileSync(b,after);
  const diff = spawnSync('git',['diff','--no-index','--no-ext-diff','--no-color','--',a,b],{encoding:'utf8',maxBuffer:16*1024*1024});
  if (![0,1].includes(diff.status)) throw new Error('Could not diff ' + name);
  return diff.stdout.slice(diff.stdout.indexOf('@@')).trimEnd();
}
async function prepare() {
  const [id,entry,version,target] = args;
  if (!identifier.test(id || '') || !entryName.test(entry || '') || !exactVersion.test(version) ||
      !['client','server','both'].includes(target)) throw new Error('Expected <id> <npm-entry> <exact-version> <client|server|both>');
  const pkg = packageOf(entry);
  const info = await registry('/' + encodeURIComponent(pkg));
  if (!pastCooldown(info.time?.[version], cutoff)) throw new Error('Release is missing or within the 14-day cooldown');
  if (!licenseAllowed(info.versions[version].license)) throw new Error('Package license needs review: ' + info.versions[version].license);
  const baseline = new Map([configPath,pkgPath,lockPath,'.gitignore'].map(name => [name,digest(name)]));
  const config = configOf();
  if (!baseline.get(configPath)) {
    if (baseline.get(pkgPath) || baseline.get(lockPath)) throw new Error('First-library setup would overwrite an existing npm project');
    const clientDir = destination(root,config.output);
    if (existsSync(clientDir) && readdirSync(clientDir).length) throw new Error('Choose an empty library output directory');
  }
  if (config.script || config.projects?.length !== 1 || config.projects[0] !== 'libraries' || !Array.isArray(config.libraries)) throw new Error('This component uses a custom build script. Add its library through that build.');
  if (config.libraries.some(lib => lib.id === id)) throw new Error('Library ID already exists: ' + id);
  const targets = target === 'both' ? ['client','server'] : [target];
  for (const library of [...config.libraries, {id, targets}]) {
    for (const target of library.targets) {
      const path = (target === 'server' ? 'script/libs' : config.output) + '/' + library.id + '.js';
      baseline.set(path, digest(path));
    }
  }
  for (const file of ['MANIFEST.json', 'NOTICES.md']) {
    const path = config.output + '/' + file;
    baseline.set(path, digest(path));
  }
  config.libraries.push({id,entry,targets});
  mkdirSync(cache,{mode:0o700,recursive:true});
  if (targets.includes('server') && !process.env.JS_VALIDATOR) throw new Error('A server target needs the moqui QuickJS validator');
  const scratch = mkdtempSync(join(cache,'prepare-'));
  try {
    const project = join(scratch,'libraries');
    mkdirSync(project);
    const pkgFile = contents(pkgPath);
    const lock = contents(lockPath);
    const pkgJson = pkgFile ? JSON.parse(pkgFile) : {name:component + '-libraries',private:true,dependencies:{
      esbuild:read(join(libraryRoot,'libs/package.json')).dependencies.esbuild}};
    if (pkgJson.dependencies[pkg] && pkgJson.dependencies[pkg] !== version) throw new Error('Change the existing pin with update-js');
    pkgJson.dependencies[pkg] = version;
    writeFileSync(join(project,'package.json'),json(pkgJson));
    if (lock) writeFileSync(join(project,'package-lock.json'),lock);
    run(npmCommand[0],[...npmCommand.slice(1),'install','--package-lock-only','--ignore-scripts','--save-exact',
      '--no-audit','--no-fund','--registry=https://registry.npmjs.org','--before='+cutoff],{cwd:project});
    const files = componentFiles(component,config,project,process.env.JS_VALIDATOR);
    files.set(configPath,Buffer.from(json(config)));
    files.set(pkgPath,Buffer.from(json(pkgJson)));
    files.set(lockPath,readFileSync(join(project,'package-lock.json')));
    for (const target of targets) {
      const path = (target === 'server' ? 'script/libs' : config.output) + '/' + id + '.js';
      if (contents(path)) throw new Error('Library would overwrite ' + path);
    }
    if (!baseline.get(configPath)) {
      for (const path of files.keys()) {
        if (contents(path)) throw new Error('First-library setup would overwrite ' + path);
      }
    }
    const ignore = contents('.gitignore')?.toString() || '';
    const entries = targets.map(target => '/' + (target === 'server' ? 'script/libs' : config.output) + '/' + id + '.js');
    const added = entries.filter(entry => !ignore.split(/\r?\n/).includes(entry));
    if (added.length) files.set('.gitignore', Buffer.from(ignore + (ignore && !ignore.endsWith('\n') ? '\n' : '') + added.join('\n') + '\n'));
    for (const [file,original] of baseline) if (original !== digest(file)) throw new Error('Changed during prepare: ' + file);
    const changes = [], saved = [];
    for (const [path,bytes] of files) {
      const before = contents(path);
      saved.push({path,base:before ? sha256(before) : null,bytes:bytes.toString('base64')});
      if (before?.equals(bytes)) continue;
      changes.push({path,size:bytes.length,...(path.endsWith('.js') ? {sha256:sha256(bytes)} : {diff:reviewDiff(path,before,bytes,scratch)})});
    }
    const token = randomUUID();
    writeFileSync(join(cache,'review-'+token+'.json'),json({root,owner,saved}),{flag:'wx',mode:0o600});
    const libraries = JSON.parse(files.get(config.output + '/MANIFEST.json')).libraries;
    return {token, changes, packages: libraries.find(lib => lib.id === id).packages, libraries};
  } finally { rmSync(scratch,{recursive:true,force:true}); }
}
function apply() {
  const [token] = args;
  if (!/^[0-9a-f-]{36}$/.test(token)) throw new Error('Invalid review ID');
  const path = join(cache,'review-'+token+'.json');
  const review = read(path);
  if (review.root !== root || review.owner !== owner) throw new Error('Review belongs to another component or author');
  for (const item of review.saved) if (digest(item.path) !== item.base) throw new Error('Changed since review: '+item.path);
  publishFiles(root,new Map(review.saved.map(item=>[item.path,Buffer.from(item.bytes,'base64')])),cache);
  rmSync(path);
  return {applied:true};
}
let result;
if (action === 'recover') { recover(root,cache); process.exit(0); }
if (action === 'list') result = configOf().libraries || [];
else if (action === 'search') {
  const response = await registry('/-/v1/search?size=16&text='+encodeURIComponent(args[0]));
  result = response.objects.map(({package:pkg})=>({name:pkg.name,description:pkg.description}));
} else if (action === 'versions') {
  if (!entryName.test(args[0] || '') || packageOf(args[0]) !== args[0]) throw new Error('Invalid npm package');
  const info = await registry('/'+encodeURIComponent(args[0]));
  result = Object.entries(info.versions).map(([version,data])=>({version,license:data.license,date:info.time[version],
    allowed:licenseAllowed(data.license),cooled:pastCooldown(info.time[version], cutoff)})).reverse();
} else if (action === 'prepare') result = await prepare();
else if (action === 'apply') result = apply();
else throw new Error('Unknown library operation: '+action);
console.log(JSON.stringify(result));
