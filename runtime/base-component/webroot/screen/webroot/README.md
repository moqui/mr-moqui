# Static files of the webroot component

Everything in this directory is served by the path walk as a file under the `webroot` screen (`/css/moqui-html.css`,
`/js/moqui-html.js`, `/images/...`, `/favicon.ico`, `/libs/...`). The rules are in `screen/static_files.rs`: no hidden
files, no `..`, no screen or template sources (`.xml`, `.ftl`, `.fancy`, `.groovy`, `.properties`, `.sql`), nothing that a link
leads out of this directory, a media type by extension, a weak `ETag` and 304, `Cache-Control` of a year for `*.min.*` and for
a request with a `v` parameter and an hour with revalidation for the rest. The host compresses text with gzip.

## Moqui files

| Path | What |
|---|---|
| `css/moqui-html.css` | style sheet of the `html` mode (light and dark, responsive, print) |
| `js/moqui-html.js` | client of the `html` mode. `js/README.md` lists the data attributes. `js/moqui-html.test.js` runs under Node |
| `js/MoquiLib.js`, `js/MoquiL10n.js`, `js/WebrootVue.qvt.js`, `js/MoquiQzComponent.js` | client of the `qvt` mode and the `/qapps` shell (Vue 3.5, Quasar 2.35). `js/README.md` lists the components and the settings block. Their `*.test.js` files run under Node |
| `css/WebrootVue.qvt.css` | style sheet of the `qvt` shell and widgets: only what Quasar 2.35 has no prop for (tonal buttons, the div-table rules, the container-box accents, print) |
| `images/`, `MoquiLogo100.png`, `favicon.ico` | logo and icon |

## Vendored browser libraries (`libs/`)

`libs/MANIFEST.json` lists every file with its size, sha256, source, npm tarball integrity and SPDX license. `libs/NOTICES.md`
has each license text. `libs/refresh.mjs` rebuilds the tree from the pinned versions (`node libs/refresh.mjs` verifies against
the manifest, `node libs/refresh.mjs --write` rewrites the tree, manifest and notices). It needs Node and npm, runs no package
script, and is a maintainer tool: the server and the Rust build never run it. The test
`crates/moqui-core/tests/browser_libs.rs` checks that every manifest file exists with the recorded sha256 and that nothing else
is in `libs/`. To upgrade, change the version in `refresh.mjs`, run with `--write`, and update `plans/archive/License-Review.md`.

| Library | Version | Paths under `/libs/` | Loaded by |
|---|---|---|---|
| Font Awesome | 4.7.0 | `fontawesome/css/font-awesome.min.css`, `fontawesome/fonts/fontawesome-webfont.{woff2,woff,ttf,eot,svg}` | `html` mode theme (every page); `qvt` shell |
| Tom Select | 2.6.2 | `tom-select/tom-select.complete.min.js`, `tom-select/tom-select.css` | `html` mode, lazy, `footer_scripts` |
| SortableJS | 1.15.7 | `sortablejs/Sortable.min.js` | `html` mode column picker, lazy |
| Vue (full, with the template compiler) | 3.5.44 | `vue/vue.global.prod.js`, global `Vue` | `qvt` pages with server-merged output or a merged fragment (needs `unsafe-eval`) |
| Vue (runtime only) | 3.5.44 | `vue/vue.runtime.global.prod.js`, global `Vue` | `qvt` pages whose output is entirely server-static (render functions compiled on the server, no `unsafe-eval`) |
| `@vue/compiler-dom` Node build, one bundle for QuickJS | 3.5.44 | `vue/compiler-dom.cjs.js`, global `VueCompilerDOM`, `VERSION` beside it | Server only (read by the host, never linked from a page) |
| Quasar | 2.35.1 | `quasar/quasar.umd.prod.js`, `quasar/quasar.prod.css`, `quasar/lang/{en-US,de,es,fr,it,pt-BR,ja,zh-CN,ru,nl}.umd.prod.js`, `quasar/icon-set/material-symbols-outlined.umd.prod.js`, global `Quasar` | `qvt` pages. The language pack is chosen from the user locale; the icon set is Quasar's own icons as Material Symbols. |
| Material Icons | 1.13.14 | `material-icons/material-icons.css` and `.woff2`, `.woff` beside it | `qvt` pages (icon names without a prefix: `menu`, `add`) |
| Material Symbols Outlined | 0.47.6 (npm `@material-symbols/font-400`) | `material-symbols/outlined.css`, `material-symbols/material-symbols-outlined.woff2` (weight 400, 0.5 MB) | `qvt` pages (the `sym_o_*` names and Quasar's icon set) |
| Chart.js | 4.5.1 | `chartjs/chart.umd.min.js`, global `Chart` | Lazy, Assist and chart widgets |
| Mermaid | 11.17.2 | `mermaid/mermaid.min.js`, global `mermaid` | Lazy, Assist. `mermaid.init` no longer exists in 11, call `mermaid.run`. |
| Marked | 18.1.1 | `marked/marked.umd.min.js`, global `marked` | Assist panel, markdown textarea |
| DOMPurify | 3.4.17 | `dompurify/purify.min.js`, global `DOMPurify` | Assist panel |
| highlight.js | 11.12.0 | `highlightjs/highlight.min.js` (core and 15 languages), `highlightjs/styles/github-dark.min.css`, `highlightjs/styles/github.min.css`, global `hljs` | Assist panel |
| Tiptap, with ProseMirror and Yjs (one esbuild bundle of `@tiptap/core`, `starter-kit`, `extension-link`, `-image`, `-mention`, `-placeholder`, `-table`, `-collaboration`, `-collaboration-caret`, `yjs`, `y-protocols` and their dependencies) | 3.31.4 | `tiptap/tiptap.min.js` (587 KB, 187 KB gzipped), global `Tiptap` (`Editor`, `Extension`, `Node`, `Mark`, `mergeAttributes`, `StarterKit`, `Link`, `Image`, `Mention`, `Placeholder`, `Table`, `TableRow`, `TableCell`, `TableHeader`, `PluginKey`, `Collaboration`, `CollaborationCaret`, `Y` (the yjs namespace), `Awareness`, `encodeAwarenessUpdate`, `applyAwarenessUpdate`, `removeAwarenessStates`) | Lazy, once a page's first `m-rich-text` or `m-coedit` mounts |
| xterm.js, with `@xterm/addon-fit` | 6.0.0, 0.11.0 | `xterm/xterm.js`, `xterm/xterm.css`, `xterm/addon-fit.js`, globals `Terminal` and `FitAddon` | Lazy, the Terminal screen (`apps/Terminal.qvue`, the browser terminal on `/termws`) |
| QZ Tray | 2.3.0 | `qz-tray/qz-tray.js` (LGPL-2.1, served unmodified) | Only when the user preference `qz.print.enabled` is `true`. The page needs the `qzSign` transition of `apps.xml`. |

The `html` mode theme (`WebrootThemeData.xml`) links Font Awesome, the style sheet and `moqui-html.js`. A widget that needs Tom Select or
SortableJS adds its paths to `footer_scripts` and `footer_stylesheets`, which `Footer.html.fancy` writes. A fragment that
a page loads later has no footer, so the client loads the library itself the first time it needs it.

The `/qapps` shell loads `vue/vue.global.prod.js` (the compiler build, because server-merged `qvt` is compiled in the browser), `quasar/quasar.umd.prod.js`, the
language pack that matches the user locale, the Material Symbols icon set, `MoquiLib.js`, `MoquiL10n.js` and `WebrootVue.qvt.js`, in that order, with the Font
Awesome, Material Symbols, Material Icons, Quasar and `WebrootVue.qvt.css` style sheets of the `DEFAULT_QUASAR` theme (`data/WebrootQuasarThemeData.xml`; the
icon fonts come before Quasar, whose icon rules set their size). Chart.js, Mermaid, Marked, DOMPurify, Tiptap and
QZ Tray are loaded by the component that needs them, from the paths in the table. The Assist panel is not ported yet.

A screen marked `server-static` for `qvt` is sent as a script (`X-Moqui-Qvt: static-template`) that registers a compiled render function in
`moqui.qvtStatic[path]`, and the client loads it with a script element, so the page needs no `unsafe-eval` for it. The runtime build of Vue
is for a page whose every qvt response is of that kind, but the `m-*` components and the shell root still have string templates that only the
compiler build compiles, so `webapp_qvt_static_only=true` (which loads `vue.runtime.global.prod.js` and says `build: "runtime"` in the settings) waits
for those templates to be precompiled. `/qapps` loads the compiler build by default.

The Assist client of the Java tree (`js/assist/AssistOpenUiLibrary.js`, not copied here) names the old paths `/libs/Chart.js/Chart.min.js`,
`/libs/marked/marked.umd.min.js`, `/libs/dompurify/purify.min.js`, `/libs/highlight.js/highlight.min.js` and
`/libs/highlight.js/styles/github-dark.min.css`. Here they are `chartjs/chart.umd.min.js`, `marked/marked.umd.min.js` (same),
`dompurify/purify.min.js` (same), `highlightjs/highlight.min.js` and `highlightjs/styles/github-dark.min.css`.
Not vendored: jQuery, Bootstrap, Moment, CKEditor, SimpleMDE, Select2, Selectivity, jsTree, http-vue-loader, RSVP, Vue 2, Quasar 1.
