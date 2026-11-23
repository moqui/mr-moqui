// Rebuild the board in an isolated dependency tree. Its pinned React and Mermaid transitive versions
// are independent of the shell. The converter takes window.mermaid, and the editor takes no Yjs.
import { readFileSync, writeFileSync, mkdirSync, readdirSync, cpSync } from 'node:fs';
import { join, dirname, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const source = dirname(fileURLToPath(import.meta.url));
const sha = bytes => createHash('sha256').update(bytes).digest('hex');

export function buildBoard(work, installed = false) {
  mkdirSync(work, { recursive: true });
  if (installed) {
    for (const filename of ['package.json', 'package-lock.json']) cpSync(join(work, '..', filename), join(work, filename));
  } else {
    for (const filename of ['package.json', 'package-lock.json']) cpSync(join(source, filename), join(work, filename));
  }
  for (const filename of ['editor-entry.js', 'converter-external.js', 'mermaid-external.js']) {
    cpSync(join(source, filename), join(work, filename));
  }
  if (!installed) execFileSync('npm', ['ci', '--ignore-scripts', '--no-audit', '--no-fund'], { cwd: work });
  const modules = join(work, 'node_modules');
  const prod = join(modules, '@excalidraw/excalidraw/dist/prod');
  const outputs = new Map();
  const packages = new Set();
  // Upstream unconditionally adds a public CDN fallback even when given a local font base.
  // It also always offers to browse/publish libraries. Remove these paths in the pinned distribution.
  function replaceOnce(file, from, to) {
    const input = readFileSync(file, 'utf8');
    if (input.includes(to) && !input.includes(from)) return;
    if (input.split(from).length !== 2) throw new Error('Unexpected Excalidraw source for ' + from);
    writeFileSync(file, input.replace(from, to));
  }
  replaceOnce(join(prod, 'chunk-FX7ZIABN.js'), 'return r.push(new URL(n,jn.ASSETS_FALLBACK_URL)),r', 'return r');
  replaceOnce(join(prod, 'index.js'), 'return eS("a",{className:"library-menu-browse-button"', 'return null;return eS("a",{className:"library-menu-browse-button"');
  replaceOnce(join(prod, 'index.js'), 'h&&Mt(Ce.Item,{icon:Cb,onSelect:()=>T(!0)', 'false&&Mt(Ce.Item,{icon:Cb,onSelect:()=>T(!0)');
  // Arimo is metrically compatible with the legacy sans face. Keep every script subset of the
  // fontsource release and its Unicode range so existing Helvetica scenes measure and export.
  const arimo = join(modules, '@fontsource/arimo');
  const faces = [];
  const css = readFileSync(join(arimo, '400.css'), 'utf8');
  for (const match of css.matchAll(/url\(\.\/files\/(arimo-[\w-]+\.woff2)\)[^;]+;\s*unicode-range:\s*([^;]+);/g)) {
    const [, file, unicodeRange] = match;
    faces.push({ uri: './fonts/Arimo/' + file, descriptors: { unicodeRange } });
    outputs.set('excalidraw/fonts/Arimo/' + file, readFileSync(join(arimo, 'files', file)));
  }
  const metadata = JSON.parse(readFileSync(join(arimo, 'metadata.json')));
  if (faces.length !== metadata.subsets.length) throw new Error('Incomplete Arimo font coverage');
  const references = faces.map((face, i) => i ? JSON.stringify(face) : '{uri:cc,descriptors:' + JSON.stringify(face.descriptors) + '}');
  replaceOnce(join(prod, 'chunk-FX7ZIABN.js'), 'var cc="./fonts/Liberation/LiberationSans-Regular.woff2";var lc=[{uri:cc}];',
    'var cc=' + JSON.stringify(faces[0].uri) + ';var lc=[' + references.join(',') + '];');
  // Modern browsers disallow unload by default. pagehide also fires when a board enters the back/forward cache.
  replaceOnce(join(prod, 'index.js'), 'Ae(window,"unload",this.onUnload,!1)', 'Ae(window,"pagehide",this.onUnload,!1)');
  replaceOnce(join(prod, 'chunk-FX7ZIABN.js'), 'import("./subset-worker.chunk.js")',
    'Promise.resolve({WorkerUrl:new URL("subset-worker.js",window.EXCALIDRAW_ASSET_PATH)})');
  // The worker is separately bundled below. Its URL is a local asset even though the editor is an IIFE.
  replaceOnce(join(prod, 'chunk-FX7ZIABN.js'), '!import.meta.url||t.toString()===import.meta.url', 't.toString()===window.location.href');
  execFileSync(join(modules, '.bin/esbuild'), [join(prod, 'subset-worker.chunk.js'), '--bundle', '--minify', '--format=esm',
    '--outfile=subset-worker.js', '--log-level=error'], { cwd: work });
  outputs.set('excalidraw/subset-worker.js', readFileSync(join(work, 'subset-worker.js')));
  for (const [name, entry, global, alias] of [
    ['editor', 'editor-entry.js', 'MoquiBoardEditor', '@excalidraw/mermaid-to-excalidraw=./converter-external.js'],
    ['converter', 'node_modules/@excalidraw/mermaid-to-excalidraw/dist/index.js', 'MoquiBoardMermaid', 'mermaid=./mermaid-external.js']
  ]) {
    execFileSync(join(modules, '.bin/esbuild'), [entry, '--bundle', '--minify', '--conditions=production', '--format=iife',
      '--global-name=' + global, '--define:process.env.NODE_ENV="production"', '--alias:' + alias,
      '--outfile=' + name + '.js', '--metafile=' + name + '-meta.json', '--log-level=error'], { cwd: work });
    outputs.set('excalidraw/' + name + '.js', readFileSync(join(work, name + '.js')));
    const inputs = Object.keys(JSON.parse(readFileSync(join(work, name + '-meta.json'))).inputs);
    for (const input of inputs.filter(i => i.startsWith('node_modules/'))) {
      if (/(?:^|\/)node_modules\/(?:mermaid|yjs)\//.test(input)) throw new Error('Bundled a shared dependency: ' + input);
      const pkg = input.match(/^(.*node_modules\/(?:@[^/]+\/)?[^/]+)/)[1];
      packages.add(pkg);
    }
  }
  function fonts(dir) {
    for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a,b) => a.name.localeCompare(b.name, 'en'))) {
      const file = join(dir, entry.name);
      if (entry.isDirectory()) { if (entry.name !== 'Liberation') fonts(file); }
      else outputs.set('excalidraw/' + relative(prod, file), readFileSync(file));
    }
  }
  fonts(join(prod, 'fonts'));
  outputs.set('excalidraw/editor.css', readFileSync(join(prod, 'index.css')));
  for (const file of ['flowchart.excalidrawlib', 'wireframe.excalidrawlib']) {
    outputs.set('excalidraw/library/' + file, readFileSync(join(source, 'library', file)));
  }
  packages.add('node_modules/@fontsource/arimo');
  const notices = new Map();
  for (const pkg of [...packages].sort()) {
    const dir = join(work, pkg);
    const info = JSON.parse(readFileSync(join(dir, 'package.json')));
    const file = readdirSync(dir).sort().find(f => /^(?:licen[cs]e|copying)(?:[.\-].*)?$/i.test(f));
    const fallback = info.name === '@excalidraw/excalidraw' ? 'Excalidraw-LICENSE'
      : info.name.startsWith('@radix-ui/') ? 'Radix-LICENSE' : info.name === 'react-remove-scroll-bar' ? 'ScrollBar-LICENSE' : null;
    if (!file && !fallback) throw new Error('Missing license text for ' + info.name);
    const text = readFileSync(file ? join(dir, file) : join(source, fallback), 'utf8').trim();
    if (!notices.has(text)) notices.set(text, new Set());
    notices.get(text).add(info.name + '@' + info.version);
  }
  // Upstream prebundles these two WASM payloads. Esbuild's package metafile cannot
  // enumerate their native libraries; retain the release's wrapper notices and native copyrights.
  const subset = readFileSync(join(prod, 'chunk-EIO257PC.js'), 'utf8');
  const wrapperNotes = [...subset.matchAll(/\/\*\*[\s\S]*?\*\//g)].map(m => m[0].replace(/[ \t]+$/gm, '')).filter(s => s.includes('wasm module'));
  if (wrapperNotes.length !== 2 || !['fonteditor-core', 'harfbuzzjs'].every(pkg => wrapperNotes.some(n => n.includes('"' + pkg + '"'))))
    throw new Error('Review the upstream WASM module notices');
  const nativeNotes = ['HarfBuzz', 'WOFF2', 'Brotli'].map(name => name + '\n' + readFileSync(join(source, name + '-LICENSE'), 'utf8').trim());
  const notice = [...[...notices].map(([text, names]) => [...names].sort().join(', ') + '\n' + text),
    ...wrapperNotes, ...nativeNotes, readFileSync(join(source, 'Font-NOTICES.txt'), 'utf8').trim()].join('\n\n') + '\n';
  outputs.set('excalidraw/NOTICES.txt', Buffer.from(notice));
  const lock = JSON.parse(readFileSync(join(work, 'package-lock.json')));
  const editor = lock.packages['node_modules/@excalidraw/excalidraw'];
  const lib = { id: 'excalidraw', name: 'Excalidraw editor and Mermaid converter', npm: '@excalidraw/excalidraw',
    version: editor.version, license: 'MIT AND MIT-Modern-Variant AND Apache-2.0 AND 0BSD AND CC0-1.0 AND Zlib AND OFL-1.1',
    source: editor.resolved,
    tarballIntegrity: editor.integrity,
    build: 'webroot/build/whiteboard/build.mjs, isolated npm ci from its package-lock.json',
    files: [...outputs].map(([path, data]) => ({ path, from: 'build', size: data.length, sha256: sha(data) })) };
  return { outputs, lib, text: 'Excalidraw, React, the converter and their dependencies are credited in excalidraw/NOTICES.txt. Fonts carry their own notices.' };
}
