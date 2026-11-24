// Run with node --test scripts/test-component-libs.mjs. Reuse the framework's reviewed tarballs.
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';
import { fileURLToPath } from 'node:url';
import { buildLibraries, licenseAllowed, pastCooldown, read } from '../runtime/components/base/webroot/build/component-libs.mjs';
import { recover } from '../runtime/components/base/webroot/build/library-changes.mjs';
import { sha256 } from '../runtime/components/base/webroot/build/js-kit.mjs';

const build = fileURLToPath(new URL('../runtime/components/base/webroot/build/', import.meta.url));
function scratch(t) {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), 'component-libs-')));
  t.after(() => rmSync(dir, {recursive: true, force: true}));
  return dir;
}

test('policy evaluates the whole SPDX expression and the release cutoff', () => {
  assert.ok(licenseAllowed('(MIT OR GPL-3.0-only) AND BSD-2-Clause'));
  assert.equal(licenseAllowed('MIT AND GPL-3.0-only'), false);
  assert.equal(licenseAllowed('MIT OR'), false);
  const cutoff = '2026-11-01T12:00:00Z';
  assert.ok(pastCooldown(cutoff, cutoff));
  assert.equal(pastCooldown('2026-11-01T12:00:01Z', cutoff), false);
  assert.equal(pastCooldown(undefined, cutoff), false);
});

test('one lock reproduces the shipped packages and only the selected targets', t => {
  const dir = scratch(t);
  const framework = read(join(build, 'libs/package-lock.json')).packages;
  const packages = Object.fromEntries(Object.entries(framework).filter(([key]) =>
    key === 'node_modules/esbuild' || key === 'node_modules/lodash-es' || key.startsWith('node_modules/@esbuild/')));
  const dependencies = Object.fromEntries(['esbuild','lodash-es'].map(name => [name, packages['node_modules/' + name].version]));
  packages[''] = {name: 'fixture', dependencies};
  writeFileSync(join(dir, 'package.json'), JSON.stringify({...packages[''], private:true}));
  writeFileSync(join(dir, 'package-lock.json'), JSON.stringify({name:'fixture', lockfileVersion:3, packages}));
  const config = {libraries:[
    {id:'add', entry:'lodash-es/add.js', targets:['client']},
    {id:'subtract', entry:'lodash-es/subtract.js', targets:['server']},
    {id:'multiply', entry:'lodash-es/multiply.js', targets:['server','client']},
  ]};
  function make(name) {
    return buildLibraries({component:'fixture', config, project:dir, work:join(dir, name)});
  }
  const files = make('first');
  assert.deepEqual(files, make('second'));
  assert.equal(files.has('server/add.js'), false);
  assert.equal(files.has('client/subtract.js'), false);
  assert.equal(runInNewContext(files.get('server/subtract.js').toString()).default(9, 6), 3);
  const globals = {moqui:{}};
  runInNewContext(files.get('client/add.js').toString(), globals);
  assert.equal(globals.moqui.libraries['fixture/add'].default(2, 3), 5);
  assert.deepEqual(JSON.parse(files.get('client/MANIFEST.json')).libraries[2].packages.map(p => p.name), ['lodash-es']);
  assert.match(files.get('client/NOTICES.md').toString(), /lodash-es@/);
});

test('apply binds the review to its author and refuses a stale file before writing', t => {
  const dir = scratch(t);
  const root = join(dir, 'components', 'site', 'fixture'), cache = join(dir, 'cache', 'fixture');
  mkdirSync(root, {recursive:true});
  mkdirSync(cache, {recursive:true});
  writeFileSync(join(root, 'component.xml'), '<component/>');
  const token = 'e3721fa4-0e96-492a-886c-c6206f19703a';
  const saved = ['config.json','manifest.json'].map(path => ({path, base:null, bytes:Buffer.from('reviewed').toString('base64')}));
  const candidate = join(cache, 'review-' + token + '.json');
  writeFileSync(candidate, JSON.stringify({owner:'author', root, saved}));
  const apply = owner => spawnSync(process.execPath, [join(build, 'add-library.mjs'), 'apply', 'fixture', token], {
    encoding:'utf8', env:{...process.env, JS_CACHE:join(dir, 'cache'), JS_COMPONENTS:join(dir, 'components'), JS_REVIEW_USER:owner}
  });
  assert.match(apply('other').stderr, /another component or author/);
  writeFileSync(join(root, 'manifest.json'), 'intervening edit');
  assert.match(apply('author').stderr, /Changed since review: manifest.json/);
  assert.equal(existsSync(join(root, 'config.json')), false);
  rmSync(join(root, 'manifest.json'));
  assert.equal(apply('author').status, 0);
  assert.equal(readFileSync(join(root, 'manifest.json'), 'utf8'), 'reviewed');
  assert.equal(existsSync(candidate), false);
});

test('an interrupted apply restores renamed files and removes its staged output', t => {
  const root = scratch(t), cache = join(root, 'cache'), token = 'fixture';
  mkdirSync(cache);
  const files = ['existing', 'created', 'staged'].map(name => ({
    name, previous:name === 'existing' ? Buffer.from('original').toString('base64') : null, intended:sha256('new')
  }));
  writeFileSync(join(cache, 'rollback.json'), JSON.stringify({root, files, token}));
  writeFileSync(join(root, 'existing'), 'new');
  writeFileSync(join(root, 'created'), 'new');
  writeFileSync(join(root, 'staged.new-fixture'), 'partial');
  recover(root, cache);
  recover(root, cache);
  assert.equal(readFileSync(join(root, 'existing'), 'utf8'), 'original');
  for (const name of ['created', 'staged', 'staged.new-fixture', 'cache/rollback.json']) assert.equal(existsSync(join(root, name)), false);
});
