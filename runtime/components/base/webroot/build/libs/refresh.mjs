// Build and verify the browser assets from both npm lockfiles.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { cpSync, constants, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { buildBoard } from '../whiteboard/build.mjs';

const source = dirname(fileURLToPath(import.meta.url));
const here = resolve(source, '../../screen/webroot/libs');
const pins = JSON.parse(readFileSync(join(source, 'package.json'))).dependencies;
const args = process.argv.slice(2);
const write = args.includes('--write');
const workArg = args.includes('--work') ? args[args.indexOf('--work') + 1] : null;
const BUNDLED_LICENSE_PKGS = [
  '@vue/compiler-dom', '@vue/compiler-core', '@vue/shared', '@babel/parser', '@babel/types',
  '@babel/helper-string-parser', '@babel/helper-validator-identifier', 'entities', 'estree-walker',
];

const TIPTAP_PACKAGES = [
  '@tiptap/core', '@tiptap/extension-blockquote', '@tiptap/extension-bold', '@tiptap/extension-bullet-list',
  '@tiptap/extension-code', '@tiptap/extension-code-block', '@tiptap/extension-collaboration', '@tiptap/extension-collaboration-caret',
  '@tiptap/extension-document', '@tiptap/extension-dropcursor', '@tiptap/extension-gapcursor', '@tiptap/extension-hard-break',
  '@tiptap/extension-heading', '@tiptap/extension-horizontal-rule', '@tiptap/extension-image', '@tiptap/extension-italic',
  '@tiptap/extension-link', '@tiptap/extension-list', '@tiptap/extension-list-item', '@tiptap/extension-list-keymap',
  '@tiptap/extension-mention', '@tiptap/extension-ordered-list', '@tiptap/extension-paragraph', '@tiptap/extension-placeholder',
  '@tiptap/extension-strike', '@tiptap/extension-table', '@tiptap/extension-table-cell', '@tiptap/extension-table-header',
  '@tiptap/extension-table-row', '@tiptap/extension-text', '@tiptap/extension-underline', '@tiptap/extensions',
  '@tiptap/pm', '@tiptap/starter-kit', '@tiptap/suggestion', '@tiptap/y-tiptap',
  'yjs', 'y-protocols', 'lib0', 'isomorphic.js',
  '@floating-ui/core', '@floating-ui/dom', '@floating-ui/utils', 'linkifyjs',
  'orderedmap', 'prosemirror-changeset', 'prosemirror-commands', 'prosemirror-dropcursor',
  'prosemirror-gapcursor', 'prosemirror-history', 'prosemirror-inputrules', 'prosemirror-keymap',
  'prosemirror-model', 'prosemirror-schema-list', 'prosemirror-state', 'prosemirror-tables',
  'prosemirror-transform', 'prosemirror-view', 'rope-sequence', 'w3c-keyname',
];
const CODEMIRROR_PACKAGES = [
  '@codemirror/state', '@codemirror/view', '@codemirror/language', '@codemirror/commands',
  '@codemirror/search', '@codemirror/autocomplete', '@codemirror/lint', '@codemirror/merge',
  '@codemirror/lang-xml', '@codemirror/lang-javascript', '@codemirror/lang-markdown', '@codemirror/lang-json',
  '@codemirror/lang-css', '@codemirror/lang-html', '@codemirror/lang-vue', '@codemirror/lang-sql',
  '@codemirror/lang-yaml', '@codemirror/lang-rust', '@codemirror/streamparser', '@lezer/common',
  '@lezer/css', '@lezer/highlight', '@lezer/html', '@lezer/javascript',
  '@lezer/json', '@lezer/lr', '@lezer/markdown', '@lezer/rust',
  '@lezer/xml', '@lezer/yaml', '@marijn/find-cluster-break', '@replit/codemirror-indentation-markers',
  'crelt', 'style-mod', 'w3c-keyname',
];
// What the CodeMirror bundle exports as the global CodeMirror: the state and view, the commands, search, completion, lint
// and merge view, the languages, the highlight tags for a theme, and the indentation markers
// (js/WebrootCode.js builds m-code-editor, m-diff-view and the dev-aide's editor from these).
const CODEMIRROR_ENTRY = `export { EditorState, EditorSelection, Compartment, StateEffect, StateField, Transaction, Annotation, ChangeSet,
  Text, Prec, Facet, RangeSetBuilder } from '@codemirror/state';
export { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, dropCursor,
  rectangularSelection, crosshairCursor, highlightSpecialChars, Decoration, ViewPlugin, WidgetType, gutter, GutterMarker,
  placeholder, showPanel, hoverTooltip, tooltips } from '@codemirror/view';
export { defaultKeymap, history, historyKeymap, indentWithTab, undo, redo, undoDepth, redoDepth, toggleComment }
  from '@codemirror/commands';
export { search, searchKeymap, highlightSelectionMatches, openSearchPanel } from '@codemirror/search';
export { autocompletion, completionKeymap, closeBrackets, closeBracketsKeymap, startCompletion, snippetCompletion }
  from '@codemirror/autocomplete';
export { linter, lintGutter, lintKeymap, setDiagnostics, forceLinting } from '@codemirror/lint';
export { syntaxHighlighting, defaultHighlightStyle, HighlightStyle, indentOnInput, bracketMatching, foldGutter, foldKeymap,
  indentUnit, syntaxTree, ensureSyntaxTree, LanguageSupport, StreamLanguage, foldAll, unfoldAll } from '@codemirror/language';
export { MergeView, unifiedMergeView, getChunks, goToNextChunk, goToPreviousChunk, acceptChunk, rejectChunk }
  from '@codemirror/merge';
export { xml, xmlLanguage } from '@codemirror/lang-xml';
export { javascript, javascriptLanguage } from '@codemirror/lang-javascript';
export { markdown, markdownLanguage } from '@codemirror/lang-markdown';
export { json } from '@codemirror/lang-json';
export { css } from '@codemirror/lang-css';
export { html } from '@codemirror/lang-html';
export { vue } from '@codemirror/lang-vue';
export { sql, PostgreSQL, MySQL, MariaSQL, StandardSQL } from '@codemirror/lang-sql';
export { yaml } from '@codemirror/lang-yaml';
export { rust } from '@codemirror/lang-rust';
export { tags } from '@lezer/highlight';
export { indentationMarkers } from '@replit/codemirror-indentation-markers';
`;

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
// The crates compiled into matrix-sdk-crypto-wasm's WebAssembly module, with their versions, licenses and license texts.
// Made for each pin from the Cargo.lock of the package's git tag: `cargo metadata --locked --filter-platform
// wasm32-unknown-unknown` on that Cargo.toml and lock (which fetches the crates), then every normal dependency reachable
// from the root that is not a procedural macro, with the LICENSE, COPYING and NOTICE files of its package, each text once.
// Kept as committed: the npm package does not carry it.
const CRYPTO_CRATES_NOTICE = 'licenses/matrix-sdk-crypto-wasm-18.9.0-crates.txt';
const OFL_URL = 'https://openfontlicense.org/documents/OFL.txt';
const UNICODE_URL = 'https://www.unicode.org/license.txt';

// Each entry: npm package and exact version, SPDX license, files copied from the package
// ([path in package, path under libs/]), the license files to print in NOTICES.md, and an optional build.
const LIBS = [
  {
    id: 'drauu', name: 'drauu', npm: '@drauu/core', license: 'MIT',
    copy: [['dist/index.mjs', 'drauu/drauu.js']],
    licenseFiles: ['LICENSE'], noticePackages: ['perfect-freehand', 'nanoevents'],
  },
  {
    id: 'fontawesome', name: 'Font Awesome', npm: 'font-awesome', license: 'OFL-1.1 AND MIT', licenseNote: 'Fonts: SIL OFL 1.1. CSS: MIT. The documentation (CC-BY-3.0) is not shipped.',
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
    id: 'tom-select', name: 'Tom Select', npm: 'tom-select', license: 'Apache-2.0',
    copy: [
      ['dist/js/tom-select.complete.min.js', 'tom-select/tom-select.complete.min.js'],
      ['dist/css/tom-select.css', 'tom-select/tom-select.css'],
    ],
    licenseFiles: ['LICENSE'],
  },
  {
    // The data grid of Tools › Data (plans/archive/Tools-Database.md §9, §11): a plain global, Tabulator. Its optional
    // Luxon, SheetJS and jsPDF are not vendored: dates are formatted by MoquiL10n.js and files are written by the
    // server. 6.7.0 (Oct 21, 2026) is the newest at least two weeks old when pinned on Nov 5; 6.7.1 is from Oct 22.
    id: 'tabulator', name: 'Tabulator', npm: 'tabulator-tables', license: 'MIT',
    copy: [
      ['dist/js/tabulator.min.js', 'tabulator/tabulator.min.js'],
      ['dist/css/tabulator.min.css', 'tabulator/tabulator.min.css'],
    ],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'sortablejs', name: 'SortableJS', npm: 'sortablejs', license: 'MIT',
    copy: [['Sortable.min.js', 'sortablejs/Sortable.min.js']],
    licenseFiles: ['LICENSE'],
  },
  {
    // The browser terminal (/qapps/Terminal): the terminal client over /termws. Pinned Oct 21, 2026; 6.0.0 and 0.11.0
    // are the latest stable releases, both published in 2025.
    id: 'xterm', name: 'xterm.js', npm: '@xterm/xterm', license: 'MIT',
    copy: [
      ['lib/xterm.js', 'xterm/xterm.js'],
      ['css/xterm.css', 'xterm/xterm.css'],
    ],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'xterm-addon-fit', name: '@xterm/addon-fit', npm: '@xterm/addon-fit', license: 'MIT',
    copy: [['lib/addon-fit.js', 'xterm/addon-fit.js']],
    licenseFiles: ['LICENSE'],
  },
  {
    // The terminal's search, plain URLs as links, and wide characters and emoji at the right width
    // (plans/Terminal-Fancy.md §9.4). Pinned Nov 11, 2026, the releases that go with xterm.js 6.0.0.
    id: 'xterm-addon-search', name: '@xterm/addon-search', npm: '@xterm/addon-search', license: 'MIT',
    copy: [['lib/addon-search.js', 'xterm/addon-search.js']],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'xterm-addon-web-links', name: '@xterm/addon-web-links', npm: '@xterm/addon-web-links', license: 'MIT',
    copy: [['lib/addon-web-links.js', 'xterm/addon-web-links.js']],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'xterm-addon-unicode11', name: '@xterm/addon-unicode11', npm: '@xterm/addon-unicode11', license: 'MIT',
    copy: [['lib/addon-unicode11.js', 'xterm/addon-unicode11.js']],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'vue', name: 'Vue', npm: 'vue', license: 'MIT',
    copy: [
      ['dist/vue.global.prod.js', 'vue/vue.global.prod.js'],
      ['dist/vue.runtime.global.prod.js', 'vue/vue.runtime.global.prod.js'],
    ],
    licenseFiles: ['LICENSE'],
    extra: (ctx) => [['vue/VERSION', Buffer.from(
      `vue ${ctx.version('vue')}\n@vue/compiler-dom ${pins['@vue/compiler-dom']} (compiler-dom.cjs.js, a bundle)\n`)]],
  },
  {
    id: 'vue-compiler-dom', name: '@vue/compiler-dom (Node build, bundled for QuickJS)', npm: '@vue/compiler-dom', license: 'MIT AND BSD-2-Clause', licenseNote: 'Bundles @babel/parser (MIT), entities (BSD-2-Clause), estree-walker (MIT), @vue/* (MIT).',
    copy: [],
    noticePackages: BUNDLED_LICENSE_PKGS,
    build: 'esbuild --bundle --minify --format=iife --global-name=VueCompilerDOM --platform=node --target=es2020 ' +
      'with NODE_ENV=production, source-map-js aliased to a stub that throws (only used for sourceMap:true), and an atob ' +
      'shim injected (QuickJS has no atob; entities decodes its tables with it at load). Exposes the global VueCompilerDOM. ' +
      'No Node builtins are required.',
    produce: (ctx) => [['vue/compiler-dom.cjs.js', ctx.esbuildCompilerDom()]],
  },
  {
    id: 'quasar', name: 'Quasar', npm: 'quasar', license: 'MIT',
    copy: [
      ['dist/quasar.umd.prod.js', 'quasar/quasar.umd.prod.js'],
      ['dist/quasar.prod.css', 'quasar/quasar.prod.css'],
      ...QUASAR_LANGS.map((l) => [`dist/lang/${l}.umd.prod.js`, `quasar/lang/${l}.umd.prod.js`]),
      ['dist/icon-set/material-symbols-outlined.umd.prod.js', 'quasar/icon-set/material-symbols-outlined.umd.prod.js'],
    ],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'material-icons', name: 'Material Icons (Google)', npm: 'material-icons', license: 'Apache-2.0',
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
    id: 'material-symbols', name: 'Material Symbols Outlined (Google)', npm: '@material-symbols/font-400', license: 'Apache-2.0',
    copy: [
      ['outlined.css', 'material-symbols/outlined.css'],
      ['material-symbols-outlined.woff2', 'material-symbols/material-symbols-outlined.woff2'],
    ],
    licenseFiles: ['LICENSE'],
  },
  // The default fonts of the qvt shell, served from here so a page makes no request to a font host: Atkinson Hyperlegible Next
  // for text and Atkinson Hyperlegible Mono for code (Braille Institute, OFL). The variable woff2 files only (weights 200 to 800,
  // normal and italic); the @font-face rules are in css/WebrootVue.qvt.css. Pinned Nov 7, 2026; 5.3.0 was published Jul 19, 2026.
  {
    id: 'atkinson-next', name: 'Atkinson Hyperlegible Next (variable, latin and latin-ext)', npm: '@fontsource-variable/atkinson-hyperlegible-next',
    license: 'OFL-1.1',
    copy: [
      ['files/atkinson-hyperlegible-next-latin-wght-normal.woff2', 'atkinson-next/atkinson-hyperlegible-next-latin-wght-normal.woff2'],
      ['files/atkinson-hyperlegible-next-latin-wght-italic.woff2', 'atkinson-next/atkinson-hyperlegible-next-latin-wght-italic.woff2'],
      ['files/atkinson-hyperlegible-next-latin-ext-wght-normal.woff2', 'atkinson-next/atkinson-hyperlegible-next-latin-ext-wght-normal.woff2'],
      ['files/atkinson-hyperlegible-next-latin-ext-wght-italic.woff2', 'atkinson-next/atkinson-hyperlegible-next-latin-ext-wght-italic.woff2'],
    ],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'atkinson-mono', name: 'Atkinson Hyperlegible Mono (variable, latin and latin-ext)', npm: '@fontsource-variable/atkinson-hyperlegible-mono',
    license: 'OFL-1.1',
    copy: [
      ['files/atkinson-hyperlegible-mono-latin-wght-normal.woff2', 'atkinson-mono/atkinson-hyperlegible-mono-latin-wght-normal.woff2'],
      ['files/atkinson-hyperlegible-mono-latin-wght-italic.woff2', 'atkinson-mono/atkinson-hyperlegible-mono-latin-wght-italic.woff2'],
      ['files/atkinson-hyperlegible-mono-latin-ext-wght-normal.woff2', 'atkinson-mono/atkinson-hyperlegible-mono-latin-ext-wght-normal.woff2'],
      ['files/atkinson-hyperlegible-mono-latin-ext-wght-italic.woff2', 'atkinson-mono/atkinson-hyperlegible-mono-latin-ext-wght-italic.woff2'],
    ],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'chartjs', name: 'Chart.js', npm: 'chart.js', license: 'MIT',
    copy: [['dist/chart.umd.min.js', 'chartjs/chart.umd.min.js']],
    licenseFiles: ['LICENSE.md'],
  },
  {
    // The country outline of geomap panels: Natural Earth's 1:110m Admin 0 boundaries (public domain) as TopoJSON.
    id: 'naturalearth', name: 'world-atlas (Natural Earth 110m countries)', npm: 'world-atlas', license: 'ISC',
    licenseNote: 'The map data is Natural Earth 4.1.0, in the public domain (https://www.naturalearthdata.com/about/terms-of-use/).',
    copy: [['countries-110m.json', 'naturalearth/countries-110m.json']],
    licenseFiles: ['LICENSE'],
  },
  {
    // The emoji and their names of m-emoji-picker (js/WebrootEmoji.js, plans/Comms-Messaging.md §4): Unicode's emoji by group,
    // made smaller (the emoji and its name only) and left at Emoji 15.1, which current systems draw. Pinned Nov 17, 2026;
    // 0.9.0 was published Apr 18, 2026.
    id: 'emoji', name: 'unicode-emoji-json', npm: 'unicode-emoji-json', license: 'MIT AND Unicode-3.0',
    licenseNote: 'The package is MIT; the emoji data in it is from unicode.org, under the Unicode License v3.',
    noticeText: () => `The emoji and their names are Unicode's data (emoji-test.txt), under the Unicode License v3: the full text is in
licenses/Unicode-3.0.txt, from ${UNICODE_URL}.
`,
    copy: [],
    build: 'data-by-group.json made into {version, groups: [{slug, name, emoji: [[emoji, name]]}]}, the emoji of ' +
      'emoji_version 15.1 or earlier, in the order of the file.',
    produce: (ctx) => [['emoji/emoji.json', ctx.emojiData()]],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'mermaid', name: 'Mermaid', npm: 'mermaid', license: 'MIT',
    copy: [['dist/mermaid.min.js', 'mermaid/mermaid.min.js']],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'marked', name: 'Marked', npm: 'marked', license: 'MIT',
    copy: [],
    build: 'esbuild --minify --legal-comments=none on lib/marked.umd.js (a transform, no bundling), banner kept as one line.',
    produce: (ctx) => [['marked/marked.umd.min.js', ctx.esbuildMarked()]],
    licenseFiles: ['LICENSE'],
  },
  {
    id: 'dompurify', name: 'DOMPurify', npm: 'dompurify', license: 'MPL-2.0 OR Apache-2.0',
    licenseNote: 'Taken under Apache-2.0.',
    copy: [['dist/purify.min.js', 'dompurify/purify.min.js']],
    licenseFiles: ['LICENSE', 'LICENSE-MPL'],
  },
  {
    id: 'highlightjs', name: 'highlight.js', npm: 'highlight.js', license: 'BSD-3-Clause',
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
    id: 'tiptap', name: 'Tiptap (with ProseMirror, bundled)', npm: '@tiptap/core', license: 'MIT',
    licenseNote: 'Bundles @tiptap/core, starter-kit (and the extensions it includes), extension-link, extension-image, ' +
      'extension-mention, extension-placeholder, extension-table, extension-table-row, extension-table-cell, ' +
      'extension-table-header, extension-collaboration, extension-collaboration-caret, extensions, suggestion and pm 3.31.4, ' +
      'the prosemirror-* packages, @tiptap/y-tiptap, yjs, y-protocols, lib0, isomorphic.js, @floating-ui/*, linkifyjs, ' +
      'orderedmap, rope-sequence and w3c-keyname: all MIT. Each license text is below once, ' +
      'with the packages it is the license of.',
    copy: [],
    noticePackages: TIPTAP_PACKAGES,
    noticeDedupe: true,
    build: 'esbuild --bundle --minify --format=iife --global-name=Tiptap --platform=browser --target=es2020 ' +
      '--legal-comments=none of an entry that exports Editor, Extension, Node, Mark, mergeAttributes, StarterKit, Link, ' +
      'Image, Mention, Placeholder, Table, TableRow, TableCell, TableHeader, PluginKey, Collaboration, CollaborationCaret, Y ' +
      '(the yjs namespace), Awareness, encodeAwarenessUpdate, applyAwarenessUpdate and removeAwarenessStates. Exposes the global Tiptap.',
    produce: (ctx) => [['tiptap/tiptap.min.js', ctx.esbuildTiptap()]],
  },
  {
    // The code editor of m-code-editor, m-diff-view and the dev-aide (plans/design-dev-aide.md 5.1), loaded only when an
    // editor mounts. CodeMirror ships ES modules, so this is a bundle of the packages in CODEMIRROR_DEPS.
    id: 'codemirror', name: 'CodeMirror 6 (bundled)', npm: '@codemirror/view', license: 'MIT',
    licenseNote: 'Bundles @codemirror/state, view, language, commands, search, autocomplete, lint, merge, streamparser, ' +
      'lang-xml, lang-javascript, lang-markdown, lang-json, lang-css, lang-html, lang-vue, lang-sql, lang-yaml and lang-rust, ' +
      'the @lezer/* parsers they use, @marijn/find-cluster-break, crelt, style-mod, w3c-keyname ' +
      'and @replit/codemirror-indentation-markers: all MIT. Each license text is below ' +
      'once, with the packages it is the license of.',
    copy: [],
    noticePackages: CODEMIRROR_PACKAGES,
    noticeDedupe: true,
    build: 'esbuild --bundle --minify --format=iife --global-name=CodeMirror --platform=browser --target=es2020 ' +
      '--legal-comments=none of an entry (CODEMIRROR_ENTRY) that exports the state, view, commands, search, autocomplete, ' +
      'lint, language and merge APIs, the language packages, the highlight tags and indentationMarkers. ' +
      'Exposes the global CodeMirror.',
    produce: (ctx) => [['codemirror/codemirror.min.js', ctx.esbuildCodeMirror()]],
  },
  {
    // The OpenUI Lang canvas of Assist (plans/LLM-A2A-Assist.md 4.10): parsed in the browser by the renderer and on the
    // server in QuickJS. The license texts are lang-core's and zod's (noticePackages).
    id: 'openui-lang-core', name: 'OpenUI Lang core (bundled with zod)', npm: '@openuidev/lang-core', license: 'MIT', licenseNote: 'Bundles @openuidev/lang-core and the parts of zod 4.6.5 it uses (MIT). Patched: ' +
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
    id: 'pdfjs', name: 'PDF.js', npm: 'pdfjs-dist', license: 'Apache-2.0',
    licenseNote: 'Only the display and worker modules. The standard_fonts (Foxit fonts under a BSD license, Liberation ' +
      'fonts under GPL-2.0 with a font exception), cmaps, iccs and wasm directories are not shipped.',
    copy: [
      ['build/pdf.min.mjs', 'pdfjs/pdf.min.mjs'],
      ['build/pdf.worker.min.mjs', 'pdfjs/pdf.worker.min.mjs'],
    ],
    licenseFiles: ['LICENSE'],
  },
  {
    // Browser session replay (js/WebrootRum.js records, Monitor › Replays plays back with its Replayer): loaded only for
    // a sampled, opt-in replay and on the replay screen. Pinned Oct 30, 2026: 2.1.7 (Oct 2) is the latest stable release
    // at least two weeks old; 2.2.0 is from Oct 24.
    id: 'rrweb', name: 'rrweb', npm: 'rrweb', license: 'MIT',
    licenseNote: 'The UMD bundle includes rrweb-snapshot, rrdom, @rrweb/types, @rrweb/utils, mitt, base64-arraybuffer and @xstate/fsm, all MIT.',
    copy: [
      ['dist/rrweb.umd.min.cjs', 'rrweb/rrweb.umd.min.js'],
      ['dist/style.min.css', 'rrweb/style.min.css'],
    ],
    // The package carries no license file; this is the LICENSE of github.com/rrweb-io/rrweb.
    noticeText: () => `MIT License

Copyright (c) 2018 Contributors (https://github.com/rrweb-io/rrweb/graphs/contributors) and SmartX Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the Software without restriction, including without limitation the
rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit
persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the
Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.`,
  },
  {
    id: 'qz-tray', name: 'QZ Tray connector (qz-tray.js)', npm: 'qz-tray', license: 'LGPL-2.1-only',
    licenseNote: 'The file is served unmodified as its own script, and only when printing is enabled. Never concatenated or minified into another file.',
    copy: [['qz-tray.js', 'qz-tray/qz-tray.js']],
    noticeText: () => `Copyright (c) QZ Industries, LLC. The file header states "@license LGPL-2.1-only". The package carries no license file.\n\nThe full text is in licenses/LGPL-2.1.txt, from ${LGPL_URL}.\nSource: https://github.com/qzind/tray (tag v2.3.0), and the npm package qz-tray@2.3.0 contains the same file unmodified.`,
  },
  {
    // End-to-end encryption in the browser (plans/archive/Comms-Framework.md §4.5, C7 core): matrix-sdk-crypto's OlmMachine (on
    // vodozemac) built to WebAssembly by its maintainers. js/WebrootCrypto.js imports index.mjs only when a device is
    // needed, never on a page load: the module is 7.8 MB. Pinned Oct 31, 2026; 18.9.0 was published Sep 21, 2026.
    id: 'matrix-sdk-crypto', name: 'matrix-sdk-crypto-wasm', npm: '@matrix-org/matrix-sdk-crypto-wasm', license: 'Apache-2.0 AND MIT AND BSD-3-Clause AND ISC AND Zlib AND Unicode-3.0 AND BSL-1.0 AND MPL-2.0',
    licenseNote: 'The package is Apache-2.0. Its WebAssembly module has 240 Rust crates inside (a dual license taken on its ' +
      'permissive side); each, with its version, license and license text, is in ' + CRYPTO_CRATES_NOTICE + '. Seven are ' +
      'MPL-2.0 (as_variant, eyeball, eyeball-im, readlock, readlock-tokio, imbl, imbl-sized-chunks), unmodified: their ' +
      'source is on crates.io at the versions listed there.',
    copy: [
      ['index.mjs', 'matrix-sdk-crypto/index.mjs'],
      ['pkg/matrix_sdk_crypto_wasm_bg.js', 'matrix-sdk-crypto/pkg/matrix_sdk_crypto_wasm_bg.js'],
      ['pkg/matrix_sdk_crypto_wasm_bg.wasm', 'matrix-sdk-crypto/pkg/matrix_sdk_crypto_wasm_bg.wasm'],
    ],
    licenseFiles: ['LICENSE'],
  },
];

const LICENSE_TEXTS = [
  { path: 'licenses/LGPL-2.1.txt', url: LGPL_URL },
  { path: 'licenses/OFL-1.1.txt', url: OFL_URL },
  { path: 'licenses/Unicode-3.0.txt', url: UNICODE_URL },
  // Kept as committed (no url): made once per pin from the crates' own license files (see CRYPTO_CRATES_NOTICE).
  { path: CRYPTO_CRATES_NOTICE, url: null },
];

const sha256 = (buf) => createHash('sha256').update(buf).digest('hex');
const run = (cmd, argv, opts = {}) => execFileSync(cmd, argv, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'], ...opts });

// Keep the installed tree pristine. Patch and bundle a private copy, preserving relative bin links.
function installProject(project, directory) {
  mkdirSync(directory, { recursive: true });
  for (const name of ['package.json', 'package-lock.json']) cpSync(join(project, name), join(directory, name));
  run('npm', ['ci', '--ignore-scripts', '--prefer-offline', '--no-audit', '--no-fund'], { cwd: directory });
  const work = join(directory, 'work');
  mkdirSync(work);
  cpSync(join(directory, 'node_modules'), join(work, 'node_modules'),
    { recursive: true, verbatimSymlinks: true, mode: constants.COPYFILE_FICLONE });
  return work;
}

function main() {
  const work = workArg ? resolve(workArg) : mkdtempSync(join(tmpdir(), 'moqui-libs-'));
  if (existsSync(work) && readdirSync(work).length) throw new Error(`work directory ${work} is not empty`);
  mkdirSync(work, { recursive: true });
  const outputs = new Map(); // path under libs/ -> Buffer
  const libMeta = [];
  const notices = [];
  const packages = {};

  const build = installProject(source, join(work, 'libs'));
  const lock = JSON.parse(readFileSync(join(source, 'package-lock.json')));
  for (const lib of LIBS) {
    lib.version = pins[lib.npm];
    const key = 'node_modules/' + lib.npm;
    const root = join(build, key);
    const info = JSON.parse(readFileSync(join(root, 'package.json')));
    if (info.version !== lib.version) throw new Error(`Unexpected ${lib.npm} version ${info.version}`);
    packages[lib.id] = { root, integrity: lock.packages[key].integrity };
  }
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
    esbuildCodeMirror() {
      writeFileSync(join(build, 'cm-entry.js'), CODEMIRROR_ENTRY);
      const out = esbuild(['cm-entry.js', '--bundle', '--minify', '--format=iife', '--global-name=CodeMirror',
        '--platform=browser', '--target=es2020', '--legal-comments=none']);
      // an editor reaches nothing of its own: no network, no workers, no eval
      const text = out.toString('utf8').toLowerCase();
      for (const banned of ['xmlhttprequest', 'fetch(', 'websocket', 'importscripts', 'new worker(', 'eval(', 'new function(']) {
        if (text.includes(banned)) throw new Error(`the CodeMirror bundle names ${banned}`);
      }
      return out;
    },
    esbuildLangCore() {
      const pkg = join(build, 'node_modules', '@openuidev', 'lang-core');
      for (const patch of OPENUI_PATCHES) {
        run('patch', ['-p1', '--forward', '--batch', '-i', join(source, patch)], { cwd: pkg });
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
    emojiData() {
      const lib = LIBS.find((l) => l.id === 'emoji');
      const groups = JSON.parse(readFileSync(join(packages.emoji.root, 'data-by-group.json'), 'utf8'));
      const drawn = (version) => { const [major, minor] = String(version).split('.').map(Number); return major < 15 || (major === 15 && (minor || 0) <= 1); };
      const out = { version: lib.version, groups: groups.map((group) => ({ slug: group.slug, name: group.name,
        emoji: group.emojis.filter((one) => drawn(one.emoji_version)).map((one) => [one.emoji, one.name]) })) };
      return Buffer.from(JSON.stringify(out) + '\n', 'utf8');
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
      ...(lib.patches ? { patches: lib.patches.map((path) => ({ path, sha256: sha256(readFileSync(join(source, path))) })) } : {}),
      files: files.map((f) => ({ path: f.path, from: f.from, size: outputs.get(f.path).length, sha256: sha256(outputs.get(f.path)) })),
    });
  }

  // Verbatim license texts that the packages do not carry. The hosts sometimes do not answer; these texts
  // do not change with a library pin, so a failed download keeps the copy already in the tree.
  const lt = [];
  for (const { path, url } of LICENSE_TEXTS) {
    let buf;
    try {
      if (!write || !url) throw new Error(`${path} is kept as committed`);
      buf = Buffer.from(run('curl', ['-fsSL', '--max-time', '20', url]), 'utf8');
    } catch (err) {
      const kept = join(here, path);
      if (!existsSync(kept)) throw err;
      if (write && url) console.error(`could not download ${url}; keeping the copy at ${path}`);
      buf = readFileSync(kept);
    }
    outputs.set(path, buf);
    lt.push({ path, from: url || 'committed', size: buf.length, sha256: sha256(buf) });
  }
  libMeta.push({ id: 'license-texts', name: 'License texts', version: '-', license: 'n/a',
    source: 'see each file', files: lt });

  const board = buildBoard(installProject(join(source, '../whiteboard'), join(work, 'whiteboard')), true);
  for (const [path, data] of board.outputs) outputs.set(path, data);
  libMeta.push(board.lib);
  notices.push(board);
  const noticesMd = renderNotices(notices);
  outputs.set('NOTICES.md', Buffer.from(noticesMd, 'utf8'));
  const manifest = {
    generatedBy: 'build/libs/refresh.mjs',
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
  if (manifestText !== readFileSync(join(here, 'MANIFEST.json'), 'utf8')) problems.push('Rebuilt manifest differs');
  // the reviewed patches a bundle was built with are the ones on disk
  for (const lib of committed.libraries) {
    for (const patch of lib.patches ?? []) {
      const disk = join(source, patch.path);
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
