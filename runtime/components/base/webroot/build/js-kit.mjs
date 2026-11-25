// What a component's browser library build shares with the framework's (build/js-build.json names the script that
// uses it). ./moq build-js runs a build script as `node <script> --build|--verify|--write` with JS_CACHE (its
// temporary trees), JS_OUTPUT (the served directory that holds the committed MANIFEST.json), JS_PROJECTS (where the
// npm projects are, when update-js hands it candidate locks) and JS_PACKS. A script installs only with
// installProject, so lifecycle scripts never run, and ends with finish(), which checks every output against the
// reviewed MANIFEST.json and publishes the tree in one rename.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { npmCommand, esbuildBinary } from './tools.mjs';

export { npmCommand, esbuildBinary };

export const sha256 = (buf) => createHash('sha256').update(buf).digest('hex');

/// Reviewed text is LF. A Windows checkout may have added CR; do not pass a binary.
export function toLf(data) {
  const buf = Buffer.isBuffer(data) ? data : Buffer.from(String(data));
  if (!buf.includes(0x0d)) return buf;
  return Buffer.from(buf.toString('utf8').replace(/\r\n/g, '\n').replace(/\r/g, '\n'));
}
export const run = (cmd, argv, opts = {}) =>
  execFileSync(cmd, argv, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'], ...opts });

/// The run's mode from its arguments: --write regenerates the manifest, --build checks and publishes, and neither
/// only verifies. --work <dir> keeps the work tree, for debugging a build.
export function mode(args = process.argv.slice(2)) {
  const write = args.includes('--write');
  return {
    write, build: write || args.includes('--build'),
    work: args.includes('--work') ? args[args.indexOf('--work') + 1] : null,
  };
}

/// The served directory: JS_OUTPUT when moq names it, else the script's own default.
export function outputDir(fallback) {
  return resolve(process.env.JS_OUTPUT || fallback);
}

// Node skips finally blocks when a signal ends it, and the build is synchronous, so a handler runs only once it
// returns. Remove the temporary trees in both cases, or each interrupted run would leave its installed trees in
// target/js and a staged tree beside the served directory. Then end by that signal, as its default action would: a
// calling shell stops its loop over the components only for a child the signal ended (bash's SIGINT rule).
const temporary = new Set();
const signals = [['SIGINT', 130], ['SIGTERM', 143], ['SIGHUP', 129]];
function endBy(signal, code) {
  for (const dir of temporary) rmSync(dir, { recursive: true, force: true });
  temporary.clear();
  for (const [name] of signals) process.removeAllListeners(name);
  process.kill(process.pid, signal);
  process.exit(code);
}
for (const [signal, code] of signals) process.on(signal, () => endBy(signal, code));

// The signal that ended the child process an error (or one of its causes) came from: npm or esbuild got the Ctrl-C
// too, and execFileSync turned its end into an exception.
function childSignal(error) {
  for (let item = error, depth = 0; item && depth < 8; item = item.cause, depth++) {
    const found = signals.find(([name, code]) => item.signal === name || item.status === code);
    if (found) return found;
  }
  return null;
}

/// Run build(work) in a fresh work directory under JS_CACHE, removed afterwards (also on a signal) unless --work
/// named it. build may return a promise; the tree is removed when it settles.
export function withWork(workArg, build) {
  const work = workArg ? resolve(workArg) : mkdtempSync(join(process.env.JS_CACHE || tmpdir(), 'build-'));
  if (!workArg) temporary.add(work);
  const clean = () => {
    if (workArg) return;
    rmSync(work, { recursive: true, force: true });
    temporary.delete(work);
  };
  const failed = (error) => {
    clean();
    const signal = childSignal(error);
    if (signal) endBy(...signal);
    throw error;
  };
  let result;
  try {
    if (workArg && existsSync(work) && readdirSync(work).length) throw new Error(`work directory ${work} is not empty`);
    mkdirSync(work, { recursive: true });
    result = build(work);
  } catch (error) {
    failed(error);
  }
  if (result && typeof result.then === 'function') {
    return result.then((value) => { clean(); return value; }, failed);
  }
  clean();
  return result;
}

/// Install one npm project into its own directory of the run's work tree, from its committed lock with lifecycle
/// scripts disabled. The run owns that tree and deletes it, so patches and bundles work in it directly.
export function installProject(project, directory) {
  mkdirSync(directory, { recursive: true });
  for (const name of ['package.json', 'package-lock.json']) cpSync(join(project, name), join(directory, name));
  run(npmCommand[0], [...npmCommand.slice(1), 'ci', '--ignore-scripts', '--prefer-offline', '--no-audit', '--no-fund'], { cwd: directory });
  return directory;
}

/// Compare the rebuilt manifest with the committed one, naming each rebuilt file that differs. With write, the new
/// manifest is the one to review instead.
export function checkManifest(here, manifestText, outputs, write) {
  if (write) return;
  const committed = toLf(readFileSync(join(here, 'MANIFEST.json'))).toString('utf8');
  if (manifestText === committed) return;
  const previous = JSON.parse(readFileSync(join(here, 'MANIFEST.json')));
  const before = new Map(previous.libraries.flatMap(lib => lib.files.map(file => [file.path, file.sha256])));
  for (const [path, buf] of outputs) {
    if (before.has(path) && before.get(path) !== sha256(buf)) console.error(`Rebuilt ${path} differs`);
  }
  throw new Error('Rebuilt assets or metadata differ from the reviewed MANIFEST.json');
}

/// Prepare the complete tree beside the served directory, on the same filesystem, and swap it in. The old tree stays
/// until the new one is in place; ./moq puts a stranded .<name>-previous back.
export function publish(here, outputs) {
  const name = basename(here);
  const staged = mkdtempSync(join(dirname(here), `.${name}-stage-`));
  temporary.add(staged);
  const backup = join(dirname(here), `.${name}-previous`);
  try {
    for (const [path, bytes] of outputs) {
      const file = join(staged, path);
      mkdirSync(dirname(file), { recursive: true });
      writeFileSync(file, bytes);
      if (!readFileSync(file).equals(bytes)) throw new Error('Staged asset differs: ' + path);
    }
    renameSync(here, backup);
    try { renameSync(staged, here); }
    catch (error) { renameSync(backup, here); throw error; }
    rmSync(backup, { recursive: true });
  } finally {
    rmSync(staged, { recursive: true, force: true });
    temporary.delete(staged);
  }
}

/// The usual end of a build: check the manifest, then in build mode publish the outputs with the manifest.
export function finish({ here, outputs, manifest, write, build }) {
  const manifestText = JSON.stringify(manifest, null, 2) + '\n';
  checkManifest(here, manifestText, outputs, write);
  if (build) {
    outputs.set('MANIFEST.json', Buffer.from(manifestText));
    publish(here, outputs);
  }
  console.log(`${build ? 'Built' : 'Verified'} ${outputs.size} files`);
}
