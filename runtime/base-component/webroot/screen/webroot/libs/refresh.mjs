#!/usr/bin/env node
// Rebuilds the vendored browser libraries from their pinned versions and checks them against MANIFEST.json.
//
//   node refresh.mjs                  verify: download, rebuild in a scratch directory, compare sha256 with
//                                     MANIFEST.json and with the files on disk. Exit 1 on any difference.
//   node refresh.mjs --write          rewrite libs/, MANIFEST.json and NOTICES.md from the pinned versions
//   node refresh.mjs --work DIR       use DIR (must be empty or absent) as the scratch directory
//
// A fix to a vendored package is a reviewed patch under patches/ (a unified diff with a header that says what and why),
// applied by the build of that library and recorded with its sha256 in MANIFEST.json; minified files are never edited.
//
// To upgrade a library, change its `version` below, run with --write, review the diff and
// plans/archive/License-Review.md, and commit. Nothing here runs a package script: every install is
// `--ignore-scripts`, tarballs come from `npm pack`, and the tarball sha512 (npm `integrity`) is recorded.
// Node 18 or later and npm are needed. This is a maintainer tool. The Rust build and the server never run it.

import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const write = args.includes('--write');
const workArg = args.includes('--work') ? args[args.indexOf('--work') + 1] : null;

// Pinned toolchain for the bundles built here (compiler-dom, highlight.js and Tiptap). Direct and transitive
// packages are pinned so the bundle is the same bytes next time. Each version was published at least two weeks
// before it was pinned (npm install --before), so a just-released, possibly compromised version is never taken.
const BUILD_DEPS = {
  esbuild: '0.28.2',
  '@vue/compiler-dom': '3.5.44',
  '@vue/compiler-core': '3.5.44',
  '@vue/shared': '3.5.44',
  '@babel/parser': '7.29.9',
  '@babel/types': '7.29.8',
  '@babel/helper-string-parser': '7.29.7',
  '@babel/helper-validator-identifier': '7.29.7',
  entities: '7.0.1',
  'estree-walker': '2.0.2',
  'source-map-js': '1.2.2',
  'highlight.js': '11.12.0',
  // OpenUI Lang (pinned Oct 22, 2026; 0.2.18 was published Sep 10, the version the Java prototype's guide was written
  // against) with the zod it imports and its one dependency; the bundle is built from the npm package with the reviewed
  // patches of patches/ applied (OPENUI_PATCHES).
  '@openuidev/lang-core': '0.2.18',
  zod: '4.6.5',
  'ci-info': '4.4.0',
  // Tiptap 3 (pinned Oct 19, 2026, published by Oct 5) and everything its bundle takes in.
  ...TIPTAP_DEPS(),
};
const BUNDLED_LICENSE_PKGS = [
  '@vue/compiler-dom', '@vue/compiler-core', '@vue/shared', '@babel/parser', '@babel/types',
  '@babel/helper-string-parser', '@babel/helper-validator-identifier', 'entities', 'estree-walker',
];

// The Tiptap packages and their dependencies, all MIT. A function so BUILD_DEPS above can spread it.
function TIPTAP_DEPS() {
  return {
    '@tiptap/core': '3.31.4',
    '@tiptap/extension-blockquote': '3.31.4',
    '@tiptap/extension-bold': '3.31.4',
    '@tiptap/extension-bullet-list': '3.31.4',
    '@tiptap/extension-code': '3.31.4',
    '@tiptap/extension-code-block': '3.31.4',
    '@tiptap/extension-collaboration': '3.31.4',
    '@tiptap/extension-collaboration-caret': '3.31.4',
    '@tiptap/extension-document': '3.31.4',
    '@tiptap/extension-dropcursor': '3.31.4',
    '@tiptap/extension-gapcursor': '3.31.4',
    '@tiptap/extension-hard-break': '3.31.4',
    '@tiptap/extension-heading': '3.31.4',
    '@tiptap/extension-horizontal-rule': '3.31.4',
    '@tiptap/extension-image': '3.31.4',
    '@tiptap/extension-italic': '3.31.4',
    '@tiptap/extension-link': '3.31.4',
    '@tiptap/extension-list': '3.31.4',
    '@tiptap/extension-list-item': '3.31.4',
    '@tiptap/extension-list-keymap': '3.31.4',
    '@tiptap/extension-mention': '3.31.4',
    '@tiptap/extension-ordered-list': '3.31.4',
    '@tiptap/extension-paragraph': '3.31.4',
    '@tiptap/extension-placeholder': '3.31.4',
    '@tiptap/extension-strike': '3.31.4',
    '@tiptap/extension-table': '3.31.4',
    '@tiptap/extension-table-cell': '3.31.4',
    '@tiptap/extension-table-header': '3.31.4',
    '@tiptap/extension-table-row': '3.31.4',
    '@tiptap/extension-text': '3.31.4',
    '@tiptap/extension-underline': '3.31.4',
    '@tiptap/extensions': '3.31.4',
    '@tiptap/pm': '3.31.4',
    '@tiptap/starter-kit': '3.31.4',
    '@tiptap/suggestion': '3.31.4',
    // The collaboration of m-coedit (pinned Oct 21, 2026, published by Sep 30): Yjs, Tiptap's y-prosemirror (@tiptap/y-tiptap,
    // which the two collaboration extensions import) and the awareness protocol, in the same bundle so ProseMirror and Yjs are
    // one instance each.
    '@tiptap/y-tiptap': '3.0.9',
    yjs: '13.6.33',
    'y-protocols': '1.0.7',
    lib0: '0.2.119',
    'isomorphic.js': '0.2.5',
    '@floating-ui/core': '1.8.0',
    '@floating-ui/dom': '1.8.0',
    '@floating-ui/utils': '0.2.12',
    linkifyjs: '4.3.3',
    orderedmap: '2.1.1',
    'prosemirror-changeset': '2.4.4',
    'prosemirror-commands': '1.7.2',
    'prosemirror-dropcursor': '1.8.4',
    'prosemirror-gapcursor': '1.4.1',
    'prosemirror-history': '1.5.1',
    'prosemirror-inputrules': '1.5.1',
    'prosemirror-keymap': '1.2.3',
    'prosemirror-model': '1.25.12',
    'prosemirror-schema-list': '1.5.1',
    'prosemirror-state': '1.4.4',
    'prosemirror-tables': '1.8.5',
    'prosemirror-transform': '1.12.2',
    'prosemirror-view': '1.42.6',
    'rope-sequence': '1.3.4',
    'w3c-keyname': '2.2.8',
  };
}
// What the Tiptap bundle exports as the global Tiptap: the editor and the extensions m-rich-text uses, and for m-coedit the
// Yjs namespace (Y), the collaboration extensions and the awareness protocol.
const TIPTAP_ENTRY = `export { Editor, Extension, Node, Mark, mergeAttributes } from '@tiptap/core';
export { default as StarterKit } from '@tiptap/starter-kit';
export { default as Link } from '@tiptap/extension-link';
export { default as Image } from '@tiptap/extension-image';
export { default as Mention } from '@tiptap/extension-mention';
export { default as Placeholder } from '@tiptap/extension-placeholder';
export { Table, TableRow, TableCell, TableHeader } from '@tiptap/extension-table';
export { PluginKey } from '@tiptap/pm/state';
export { default as Collaboration } from '@tiptap/extension-collaboration';
export { default as CollaborationCaret } from '@tiptap/extension-collaboration-caret';
export * as Y from 'yjs';
export { Awareness, encodeAwarenessUpdate, applyAwarenessUpdate, removeAwarenessStates } from 'y-protocols/awareness';
`;

// What the OpenUI Lang bundle exports as the global OpenUILang: the parser (whole and streaming), the statement merge of
// a patch, the store, the query manager and the evaluator the canvas renderer (js/WebrootAssistCanvas.js) uses; the
// server runs the same bundle in QuickJS to parse, check and merge write_ui programs (crates/moqui-core/src/llm/ui/openui.rs),
// which also reads each statement's syntax tree (tokenize, split, parseExpression).
const OPENUI_ENTRY = `export { createParser, createStreamingParser, mergeStatements, createStore, createQueryManager, evaluate,
  evaluateElementProps, BuiltinActionType, ACTION_STEPS, walkAST, tokenize, split, parseExpression, BUILTIN_NAMES,
  ACTION_NAMES } from '@openuidev/lang-core';
`;
// The reviewed patches applied, in order, to the installed package before it is bundled (patch -p1 in its directory).
// Each file says what it changes and why; a patch that no longer applies stops the build.
const OPENUI_PATCHES = [
  'patches/openui-lang-core-0.2.18-telemetry-off.patch',
  'patches/openui-lang-core-0.2.18-failed-query.patch',
];

const HLJS_LANGS = ['bash', 'css', 'diff', 'groovy', 'java', 'javascript', 'json', 'kotlin', 'markdown', 'plaintext',
  'python', 'rust', 'sql', 'typescript', 'xml', 'yaml'];
const QUASAR_LANGS = ['en-US', 'de', 'es', 'fr', 'it', 'pt-BR', 'ja', 'zh-CN', 'ru', 'nl'];

const LGPL_URL = 'https://www.gnu.org/licenses/old-licenses/lgpl-2.1.txt';
const OFL_URL = 'https://openfontlicense.org/documents/OFL.txt';

// Each entry: npm package and exact version, SPDX license, files copied from the package
// ([path in package, path under libs/]), the license files to print in NOTICES.md, and an optional build.
const LIBS = [
  {
    id: 'fontawesome', name: 'Font Awesome', npm: 'font-awesome', version: '4.7.0',
    license: 'OFL-1.1 AND MIT', licenseNote: 'Fonts: SIL OFL 1.1. CSS: MIT. The documentation (CC-BY-3.0) is not shipped.',
    copy: [
      ['css/font-awesome.min.css', 'fontawesome/css/font-awesome.min.css'],
      ['fonts/fontawesome-webfont.woff2', 'fontawesome/fonts/fontawesome-webfont.woff2'],
      ['fonts/fontawesome-webfont.woff', 'fontawesome/fonts/fontawesome-webfont.woff'],
      ['fonts/fontawesome-webfont.ttf', 'fontawesome/fonts/fontawesome-webfont.ttf'],
      ['fonts/fontawesome-webfont.eot', 'fontawesome/fonts/fontawesome-webfont.eot'],
      ['fonts/fontawesome-webfont.svg', 'fontawesome/fonts/fontawesome-webfont.svg'],
    ],
    // The package carries no license file. Its README states the terms, and the css banner names them.
    noticeText: () => `Copyright (c) Dave Gandy, http://fontawesome.io (Font Awesome 4.7.0).

The package README and the css banner state: "The Font Awesome font is licensed under the SIL OFL 1.1. Font Awesome
CSS, LESS, and Sass files are licensed under the MIT License. The Font Awesome documentation is licensed under the
CC BY 3.0 License. Attribution is no longer required as of Font Awesome 3.0, but much appreciated: Font Awesome by
Dave Gandy - http://fontawesome.io". Full details: http://fontawesome.io/license/

MIT License (the css)

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the Software without restriction, including without limitation the
rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit
persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the
Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

SIL Open Font License 1.1 (the fonts): the full text is in licenses/OFL-1.1.txt, from ${OFL_URL}.`,
  },
  {
    id: 'tom-select', name: 'Tom Select', npm: 'tom-select', version: '2.6.2', license: 'Apache-2.0',
    copy: [
      ['dist/js/tom-select.complete.min.js', 'tom-select/tom-select.complete.min.js'],
      ['dist/css/tom-select.css', 'tom-select/tom-select.css'],
    ],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'sortablejs', name: 'SortableJS', npm: 'sortablejs', version: '1.15.7', license: 'MIT',
    copy: [['Sortable.min.js', 'sortablejs/Sortable.min.js']],
    licenseFiles: ['LICENSE'],
  },
  {
    // The browser terminal (/qapps/Terminal): the terminal client over /termws. Pinned Oct 21, 2026; 6.0.0 and 0.11.0
    // are the latest stable releases, both published in 2025.
    id: 'xterm', name: 'xterm.js', npm: '@xterm/xterm', version: '6.0.0', license: 'MIT',
    copy: [
      ['lib/xterm.js', 'xterm/xterm.js'],
      ['css/xterm.css', 'xterm/xterm.css'],
    ],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'xterm-addon-fit', name: '@xterm/addon-fit', npm: '@xterm/addon-fit', version: '0.11.0', license: 'MIT',
    copy: [['lib/addon-fit.js', 'xterm/addon-fit.js']],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'vue', name: 'Vue', npm: 'vue', version: '3.5.44', license: 'MIT',
    copy: [
      ['dist/vue.global.prod.js', 'vue/vue.global.prod.js'],
      ['dist/vue.runtime.global.prod.js', 'vue/vue.runtime.global.prod.js'],
    ],
    licenseFiles: ['LICENSE'],
    extra: (ctx) => [['vue/VERSION', Buffer.from(
      `vue ${ctx.version('vue')}\n@vue/compiler-dom ${BUILD_DEPS['@vue/compiler-dom']} (compiler-dom.cjs.js, a bundle)\n`)]],
  },
  {
    id: 'vue-compiler-dom', name: '@vue/compiler-dom (Node build, bundled for QuickJS)', npm: '@vue/compiler-dom', version: '3.5.44',
    license: 'MIT AND BSD-2-Clause', licenseNote: 'Bundles @babel/parser (MIT), entities (BSD-2-Clause), estree-walker (MIT), @vue/* (MIT).',
    copy: [],
    noticePackages: BUNDLED_LICENSE_PKGS,
    build: 'esbuild --bundle --minify --format=iife --global-name=VueCompilerDOM --platform=node --target=es2020 ' +
      'with NODE_ENV=production, source-map-js aliased to a stub that throws (only used for sourceMap:true), and an atob ' +
      'shim injected (QuickJS has no atob; entities decodes its tables with it at load). Exposes the global VueCompilerDOM. ' +
      'No Node builtins are required.',
    produce: (ctx) => [['vue/compiler-dom.cjs.js', ctx.esbuildCompilerDom()]],
  },
  {
    id: 'quasar', name: 'Quasar', npm: 'quasar', version: '2.35.1', license: 'MIT',
    copy: [
      ['dist/quasar.umd.prod.js', 'quasar/quasar.umd.prod.js'],
      ['dist/quasar.prod.css', 'quasar/quasar.prod.css'],
      ...QUASAR_LANGS.map((l) => [`dist/lang/${l}.umd.prod.js`, `quasar/lang/${l}.umd.prod.js`]),
      ['dist/icon-set/material-symbols-outlined.umd.prod.js', 'quasar/icon-set/material-symbols-outlined.umd.prod.js'],
    ],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'material-icons', name: 'Material Icons (Google)', npm: 'material-icons', version: '1.13.14', license: 'Apache-2.0',
    copy: [
      ['iconfont/material-icons.css', 'material-icons/material-icons.css'],
      ['iconfont/material-icons.woff2', 'material-icons/material-icons.woff2'],
      ['iconfont/material-icons.woff', 'material-icons/material-icons.woff'],
    ],
    licenseFiles: ['LICENSE'],
  },
  {
    // The outlined style at weight 400 (grade 0, optical size 48) of the variable Material Symbols font, for Quasar's sym_o_* names
    // (style-guide §5.6): 0.5 MB. The outlined font of the material-symbols package has every weight, fill, grade and optical size in 4 MB.
    id: 'material-symbols', name: 'Material Symbols Outlined (Google)', npm: '@material-symbols/font-400', version: '0.47.6',
    license: 'Apache-2.0',
    copy: [
      ['outlined.css', 'material-symbols/outlined.css'],
      ['material-symbols-outlined.woff2', 'material-symbols/material-symbols-outlined.woff2'],
    ],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'chartjs', name: 'Chart.js', npm: 'chart.js', version: '4.5.1', license: 'MIT',
    copy: [['dist/chart.umd.min.js', 'chartjs/chart.umd.min.js']],
    licenseFiles: ['LICENSE.md'],
  },
  {
    id: 'mermaid', name: 'Mermaid', npm: 'mermaid', version: '11.17.2', license: 'MIT',
    copy: [['dist/mermaid.min.js', 'mermaid/mermaid.min.js']],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'marked', name: 'Marked', npm: 'marked', version: '18.1.1', license: 'MIT',
    copy: [],
    build: 'esbuild --minify --legal-comments=none on lib/marked.umd.js (a transform, no bundling), banner kept as one line.',
    produce: (ctx) => [['marked/marked.umd.min.js', ctx.esbuildMarked()]],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'dompurify', name: 'DOMPurify', npm: 'dompurify', version: '3.4.17', license: 'MPL-2.0 OR Apache-2.0',
    licenseNote: 'Taken under Apache-2.0.',
    copy: [['dist/purify.min.js', 'dompurify/purify.min.js']],
    licenseFiles: ['LICENSE', 'LICENSE-MPL'],
  },
  {
    id: 'highlightjs', name: 'highlight.js', npm: 'highlight.js', version: '11.12.0', license: 'BSD-3-Clause',
    copy: [
      ['styles/github-dark.min.css', 'highlightjs/styles/github-dark.min.css'],
      ['styles/github.min.css', 'highlightjs/styles/github.min.css'],
    ],
    build: `esbuild --bundle --minify --format=iife --global-name=hljs of lib/core plus the languages ${HLJS_LANGS.join(', ')}. ` +
      'Exposes the global hljs.',
    produce: (ctx) => [['highlightjs/highlight.min.js', ctx.esbuildHighlight()]],
    licenseFiles: ['LICENSE'],
  },
  {
    // The rich-text editor of m-rich-text and m-coedit, loaded only when an editor mounts. Tiptap has no browser build of its
    // own (it ships ES modules), so this is a bundle of the packages in TIPTAP_DEPS with ProseMirror and Yjs in it.
    id: 'tiptap', name: 'Tiptap (with ProseMirror, bundled)', npm: '@tiptap/core', version: '3.31.4', license: 'MIT',
    licenseNote: 'Bundles @tiptap/core, starter-kit (and the extensions it includes), extension-link, extension-image, ' +
      'extension-mention, extension-placeholder, extension-table, extension-table-row, extension-table-cell, ' +
      'extension-table-header, extension-collaboration, extension-collaboration-caret, extensions, suggestion and pm 3.31.4, ' +
      'the prosemirror-* packages, @tiptap/y-tiptap, yjs, y-protocols, lib0, isomorphic.js, @floating-ui/*, linkifyjs, ' +
      'orderedmap, rope-sequence and w3c-keyname: all MIT. Each license text is below once, ' +
      'with the packages it is the license of.',
    copy: [],
    noticePackages: Object.keys(TIPTAP_DEPS()),
    noticeDedupe: true,
    build: 'esbuild --bundle --minify --format=iife --global-name=Tiptap --platform=browser --target=es2020 ' +
      '--legal-comments=none of an entry that exports Editor, Extension, Node, Mark, mergeAttributes, StarterKit, Link, ' +
      'Image, Mention, Placeholder, Table, TableRow, TableCell, TableHeader, PluginKey, Collaboration, CollaborationCaret, Y ' +
      '(the yjs namespace), Awareness, encodeAwarenessUpdate, applyAwarenessUpdate and removeAwarenessStates. Exposes the global Tiptap.',
    produce: (ctx) => [['tiptap/tiptap.min.js', ctx.esbuildTiptap()]],
  },
  {
    // The OpenUI Lang canvas of Assist (plans/LLM-A2A-Assist.md 4.10): parsed in the browser by the renderer and on the
    // server in QuickJS. The license texts are lang-core's and zod's (noticePackages).
    id: 'openui-lang-core', name: 'OpenUI Lang core (bundled with zod)', npm: '@openuidev/lang-core', version: '0.2.18',
    license: 'MIT', licenseNote: 'Bundles @openuidev/lang-core and the parts of zod 4.6.5 it uses (MIT). Patched: ' +
      OPENUI_PATCHES.join(', ') + ' (telemetry off; a failed Query is not fetched again until its arguments change).',
    copy: [],
    noticePackages: ['@openuidev/lang-core', 'zod'],
    noticeDedupe: true,
    patches: OPENUI_PATCHES,
    build: 'the patches applied with patch -p1 to the installed package, then esbuild --bundle --minify --format=iife ' +
      '--global-name=OpenUILang --platform=browser --target=es2020 --legal-comments=none of an entry that exports ' +
      'createParser, createStreamingParser, mergeStatements, createStore, createQueryManager, evaluate, ' +
      'evaluateElementProps, BuiltinActionType, ACTION_STEPS, walkAST, tokenize, split, parseExpression, BUILTIN_NAMES and ' +
      'ACTION_NAMES. Exposes the global OpenUILang; the build fails ' +
      'when the bundle names a telemetry host, a process call or a network call of its own.',
    produce: (ctx) => [['openui/lang-core.min.js', ctx.esbuildLangCore()]],
  },
  {
    // PDF previews in Files: the PDF engine's Chromium page imports the two modules (crates/moqui-host/src/pdf.rs).
    // Pinned Oct 21, 2026; 6.4.299 was published Oct 3 and has the CVE-2024-4367 fix (4.2.67 and later).
    id: 'pdfjs', name: 'PDF.js', npm: 'pdfjs-dist', version: '6.4.299', license: 'Apache-2.0',
    licenseNote: 'Only the display and worker modules. The standard_fonts (Foxit fonts under a BSD license, Liberation ' +
      'fonts under GPL-2.0 with a font exception), cmaps, iccs and wasm directories are not shipped.',
    copy: [
      ['build/pdf.min.mjs', 'pdfjs/pdf.min.mjs'],
      ['build/pdf.worker.min.mjs', 'pdfjs/pdf.worker.min.mjs'],
    ],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'qz-tray', name: 'QZ Tray connector (qz-tray.js)', npm: 'qz-tray', version: '2.3.0', license: 'LGPL-2.1-only',
    licenseNote: 'The file is served unmodified as its own script, and only when printing is enabled. Never concatenated or minified into another file.',
    copy: [['qz-tray.js', 'qz-tray/qz-tray.js']],
    noticeText: () => `Copyright (c) QZ Industries, LLC. The file header states "@license LGPL-2.1-only". The package carries no license file.\n\nThe full text is in licenses/LGPL-2.1.txt, from ${LGPL_URL}.\nSource: https://github.com/qzind/tray (tag v2.3.0), and the npm package qz-tray@2.3.0 contains the same file unmodified.`,
  },
];

const LICENSE_TEXTS = [
  { path: 'licenses/LGPL-2.1.txt', url: LGPL_URL },
  { path: 'licenses/OFL-1.1.txt', url: OFL_URL },
];

const sha256 = (buf) => createHash('sha256').update(buf).digest('hex');
const run = (cmd, argv, opts = {}) => execFileSync(cmd, argv, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'], ...opts });

function main() {
  const work = workArg ? resolve(workArg) : mkdtempSync(join(tmpdir(), 'moqui-libs-'));
  if (existsSync(work) && readdirSync(work).length) throw new Error(`work directory ${work} is not empty`);
  mkdirSync(work, { recursive: true });
  const outputs = new Map(); // path under libs/ -> Buffer
  const libMeta = [];
  const notices = [];
  const packages = {};

  // Download and extract each package into its own fresh directory.
  for (const lib of LIBS) {
    const dir = join(work, 'pkg', lib.id);
    mkdirSync(join(dir, 'tgz'), { recursive: true });
    mkdirSync(join(dir, 'x'), { recursive: true });
    const spec = `${lib.npm}@${lib.version}`;
    const info = JSON.parse(run('npm', ['pack', spec, '--json', '--ignore-scripts', '--pack-destination', join(dir, 'tgz')]))[0];
    run('tar', ['-xzf', join(dir, 'tgz', info.filename), '-C', join(dir, 'x'), '--no-same-owner']);
    packages[lib.id] = { root: join(dir, 'x', 'package'), integrity: info.integrity, shasum: info.shasum };
    const pj = JSON.parse(readFileSync(join(packages[lib.id].root, 'package.json'), 'utf8'));
    if (pj.version !== lib.version) throw new Error(`${spec}: tarball says ${pj.version}`);
    if (pj.license !== undefined && typeof pj.license === 'string' && !lib.license.includes(pj.license.replace(/[()]/g, '').split(' ')[0])) {
      console.warn(`warning: ${spec} package.json license "${pj.license}" vs recorded "${lib.license}"`);
    }
  }

  // Build environment for the bundles.
  const build = join(work, 'build');
  mkdirSync(build, { recursive: true });
  writeFileSync(join(build, 'package.json'), '{"private":true}\n');
  run('npm', ['install', '--ignore-scripts', '--no-audit', '--no-fund', '--save-exact',
    ...Object.entries(BUILD_DEPS).map(([n, v]) => `${n}@${v}`)], { cwd: build });
  const esbuild = (argv) => {
    const out = join(build, 'out', `o${Math.random().toString(36).slice(2)}.js`);
    run(join(build, 'node_modules', '.bin', 'esbuild'), [...argv, `--outfile=${out}`, '--log-level=error'], { cwd: build });
    return readFileSync(out);
  };
  const ctx = {
    version: (id) => LIBS.find((l) => l.id === id).version,
    esbuildCompilerDom() {
      writeFileSync(join(build, 'cd-entry.js'), "module.exports = require('@vue/compiler-dom');\n");
      writeFileSync(join(build, 'smj-stub.js'), "module.exports = { SourceMapGenerator: function SourceMapGenerator() { throw new Error('source maps are not available in this build'); } };\n");
      writeFileSync(join(build, 'atob-shim.js'), `// QuickJS has no atob. entities (a dependency of the Vue compiler) decodes its tables with it at load time.
const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
function polyfill(input) {
  const s = String(input).replace(/[\\t\\n\\f\\r =]/g, '');
  let out = '', bits = 0, acc = 0;
  for (let i = 0; i < s.length; i++) {
    const v = chars.indexOf(s[i]);
    if (v < 0) throw new Error('invalid base64');
    acc = (acc << 6) | v; bits += 6;
    if (bits >= 8) { bits -= 8; out += String.fromCharCode((acc >> bits) & 255); }
  }
  return out;
}
export const atob = typeof globalThis.atob === 'function' ? globalThis.atob.bind(globalThis) : polyfill;
`);
      return esbuild(['cd-entry.js', '--bundle', '--minify', '--format=iife', '--global-name=VueCompilerDOM', '--platform=node',
        '--target=es2020', '--define:process.env.NODE_ENV="production"', '--alias:source-map-js=./smj-stub.js',
        '--inject:./atob-shim.js', '--legal-comments=none']);
    },
    esbuildHighlight() {
      const lines = ["const hljs = require('highlight.js/lib/core');"];
      for (const l of HLJS_LANGS) lines.push(`hljs.registerLanguage('${l}', require('highlight.js/lib/languages/${l}'));`);
      lines.push('module.exports = hljs;');
      writeFileSync(join(build, 'hl-entry.js'), lines.join('\n') + '\n');
      return esbuild(['hl-entry.js', '--bundle', '--minify', '--format=iife', '--global-name=hljs', '--platform=browser',
        '--target=es2018', '--legal-comments=none']);
    },
    esbuildTiptap() {
      writeFileSync(join(build, 'tt-entry.js'), TIPTAP_ENTRY);
      return esbuild(['tt-entry.js', '--bundle', '--minify', '--format=iife', '--global-name=Tiptap', '--platform=browser',
        '--target=es2020', '--legal-comments=none']);
    },
    esbuildLangCore() {
      const pkg = join(build, 'node_modules', '@openuidev', 'lang-core');
      for (const patch of OPENUI_PATCHES) {
        run('patch', ['-p1', '--forward', '--batch', '-i', join(here, patch)], { cwd: pkg });
      }
      writeFileSync(join(build, 'openui-entry.js'), OPENUI_ENTRY);
      const out = esbuild(['openui-entry.js', '--bundle', '--minify', '--format=iife', '--global-name=OpenUILang',
        '--platform=browser', '--target=es2020', '--legal-comments=none']);
      // the bundle parses, evaluates and merges; it reaches nothing: no telemetry host, no process, no network of its own
      // (a Query's tool is the page's read proxy, given to the query manager as its provider)
      const text = out.toString('utf8');
      for (const banned of ['posthog', 'cloudfront', 'ci-info', 'child_process', 'execfile', 'getbuiltinmodule', 'sendbeacon',
        'xmlhttprequest', 'fetch(', 'websocket', 'importscripts']) {
        if (text.toLowerCase().includes(banned)) throw new Error(`the OpenUI Lang bundle still names ${banned}`);
      }
      return out;
    },
    esbuildMarked() {
      const src = join(packages.marked.root, 'lib', 'marked.umd.js');
      const v = LIBS.find((l) => l.id === 'marked').version;
      return esbuild([src, '--minify', '--legal-comments=none', `--banner:js=/*! marked v${v} | MIT | https://github.com/markedjs/marked */`]);
    },
  };

  for (const lib of LIBS) {
    const root = packages[lib.id].root;
    const files = [];
    for (const [from, to] of lib.copy) {
      outputs.set(to, readFileSync(join(root, from)));
      files.push({ path: to, from });
    }
    for (const [to, buf] of [...(lib.produce ? lib.produce(ctx) : []), ...(lib.extra ? lib.extra(ctx) : [])]) {
      outputs.set(to, buf);
      files.push({ path: to, from: lib.produce ? 'build' : 'generated' });
    }
    let text = '';
    if (lib.noticeText) text += lib.noticeText() + '\n';
    for (const f of lib.licenseFiles ?? []) text += `--- ${f} ---\n${readFileSync(join(root, f), 'utf8').trimEnd()}\n\n`;
    // noticeDedupe: a license text that several packages share is printed once, after the list of them
    const shared = new Map();
    for (const p of lib.noticePackages ?? []) {
      const pdir = join(build, 'node_modules', p);
      const lf = readdirSync(pdir).sort().find((n) => /^licen[cs]e(\.md|\.txt)?$/i.test(n));
      if (!lf) throw new Error(`no license file in ${p}`);
      const named = `${p} ${JSON.parse(readFileSync(join(pdir, 'package.json'), 'utf8')).version}: ${lf}`;
      const body = readFileSync(join(pdir, lf), 'utf8').trimEnd();
      if (!lib.noticeDedupe) text += `--- ${named} ---\n${body}\n\n`;
      else if (shared.has(body)) shared.get(body).push(named);
      else shared.set(body, [named]);
    }
    for (const [body, names] of shared) text += `--- ${names.join(', ')} ---\n${body}\n\n`;
    notices.push({ lib, text });
    libMeta.push({
      id: lib.id, name: lib.name, version: lib.version, license: lib.license,
      ...(lib.licenseNote ? { note: lib.licenseNote } : {}),
      source: `https://registry.npmjs.org/${lib.npm}/-/${lib.npm.split('/').pop()}-${lib.version}.tgz`,
      npm: `${lib.npm}@${lib.version}`, tarballIntegrity: packages[lib.id].integrity,
      ...(lib.build ? { build: lib.build } : {}),
      ...(lib.patches ? { patches: lib.patches.map((path) => ({ path, sha256: sha256(readFileSync(join(here, path))) })) } : {}),
      files: files.map((f) => ({ path: f.path, from: f.from, size: outputs.get(f.path).length, sha256: sha256(outputs.get(f.path)) })),
    });
  }

  // Verbatim license texts that the packages do not carry. The hosts sometimes do not answer; these texts
  // do not change with a library pin, so a failed download keeps the copy already in the tree.
  const lt = [];
  for (const { path, url } of LICENSE_TEXTS) {
    let buf;
    try {
      buf = Buffer.from(run('curl', ['-fsSL', '--max-time', '20', url]), 'utf8');
    } catch (err) {
      const kept = join(here, path);
      if (!existsSync(kept)) throw err;
      console.error(`could not download ${url}; keeping the copy at ${path}`);
      buf = readFileSync(kept);
    }
    outputs.set(path, buf);
    lt.push({ path, from: url, size: buf.length, sha256: sha256(buf) });
  }
  libMeta.push({ id: 'license-texts', name: 'License texts', version: '-', license: 'n/a',
    source: 'see each file', files: lt });

  const noticesMd = renderNotices(notices);
  outputs.set('NOTICES.md', Buffer.from(noticesMd, 'utf8'));
  const manifest = {
    generatedBy: 'libs/refresh.mjs',
    buildTools: BUILD_DEPS,
    notices: { path: 'NOTICES.md', sha256: sha256(outputs.get('NOTICES.md')) },
    libraries: libMeta,
  };
  const manifestText = JSON.stringify(manifest, null, 2) + '\n';

  if (write) {
    for (const [path, buf] of outputs) {
      const dest = join(here, path);
      mkdirSync(dirname(dest), { recursive: true });
      writeFileSync(dest, buf);
    }
    writeFileSync(join(here, 'MANIFEST.json'), manifestText);
    console.log(`wrote ${outputs.size} files and MANIFEST.json under ${here}`);
    cleanup(work);
    return;
  }

  // Verify mode: compare against the committed manifest, and the rebuilt bytes against the tree.
  const committed = JSON.parse(readFileSync(join(here, 'MANIFEST.json'), 'utf8'));
  const want = new Map();
  for (const lib of committed.libraries) for (const f of lib.files) want.set(f.path, f.sha256);
  want.set(committed.notices.path, committed.notices.sha256);
  const problems = [];
  // the reviewed patches a bundle was built with are the ones on disk
  for (const lib of committed.libraries) {
    for (const patch of lib.patches ?? []) {
      const disk = join(here, patch.path);
      if (!existsSync(disk)) problems.push(`missing patch: ${patch.path}`);
      else if (sha256(readFileSync(disk)) !== patch.sha256) problems.push(`patch ${patch.path} differs from MANIFEST.json`);
    }
  }
  for (const [path, buf] of outputs) {
    const h = sha256(buf);
    if (!want.has(path)) problems.push(`not in MANIFEST.json: ${path}`);
    else if (want.get(path) !== h) problems.push(`rebuilt ${path} differs from MANIFEST.json (rebuilt ${h}, manifest ${want.get(path)})`);
  }
  for (const [path, h] of want) {
    if (!outputs.has(path)) problems.push(`in MANIFEST.json but not rebuilt: ${path}`);
    const disk = join(here, path);
    if (!existsSync(disk)) problems.push(`missing on disk: ${path}`);
    else if (statSync(disk).isFile() && sha256(readFileSync(disk)) !== h) problems.push(`on-disk ${path} differs from MANIFEST.json`);
  }
  cleanup(work);
  if (problems.length) {
    console.error(problems.join('\n'));
    process.exit(1);
  }
  console.log(`verified ${want.size} files against MANIFEST.json`);
}

function cleanup(work) {
  if (!workArg) rmSync(work, { recursive: true, force: true });
}

function renderNotices(notices) {
  let md = `# Third-party notices

The files under \`libs/\` are third-party browser libraries served as their own files. They are not compiled into the
\`moqui\` binary. \`MANIFEST.json\` lists each file with its sha256, and \`refresh.mjs\` rebuilds them from the pinned
versions below. The license of each library applies to its files only. The framework's own CC0 license does not
relicense them, and theirs does not apply to Moqui code.

`;
  for (const { lib, text } of notices) {
    md += `## ${lib.name} ${lib.version}\n\nLicense: ${lib.license}. Source: npm \`${lib.npm}@${lib.version}\`.\n`;
    if (lib.licenseNote) md += `${lib.licenseNote}\n`;
    md += `\n\`\`\`text\n${text.trimEnd()}\n\`\`\`\n\n`;
  }
  return md;
}

main();
