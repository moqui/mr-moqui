// Build and verify the browser assets from both npm lockfiles: the webroot component's script of
// build/js-build.json. The steps every component's build shares are in ../js-kit.mjs.
import { checkManifest, componentName, esbuildBinary, installProject, mode, outputDir, outputLabel, publish, run, sha256, status, summary, toLf, withWork } from '../js-kit.mjs';
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { assetPack, fontLibraries, packs } from '../fonts/build.mjs';
import { buildBoard } from '../whiteboard/build.mjs';

const source = dirname(fileURLToPath(import.meta.url));
// Multiline text in this file picks up CR from a Windows checkout. Package license bytes do not.
const lf = (value) => toLf(value).toString('utf8');
const here = outputDir(resolve(source, '../../screen/webroot/libs'));
const projects = process.env.JS_PROJECTS || dirname(source);
const pins = JSON.parse(readFileSync(join(projects, 'libs/package.json'))).dependencies;
const { write, build: publishing, work: workArg } = mode();
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
  'lib0', '@floating-ui/core', '@floating-ui/dom', '@floating-ui/utils', 'linkifyjs',
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
// The packages of the libraries beside CodeMirror's bundle, each loaded only when an editor needs it: Yjs with its
// awareness protocol (co-editing, and Tiptap's collaboration), y-codemirror.next (a co-edited code editor), the Vim
// keymap and the legacy language modes.
const YJS_PACKAGES = ['yjs', 'y-protocols', 'lib0'];
const Y_CODEMIRROR_PACKAGES = ['y-codemirror.next', 'lib0'];
const CODEMIRROR_VIM_PACKAGES = ['@replit/codemirror-vim', '@replit/codemirror-vim-core'];
// The second bundle of CodeMirror's languages (plans/design-dev-aide.md 5.1): the legacy modes of @codemirror/legacy-modes
// that lang-* has no package for, each a stream parser's spec that js/WebrootCode.js gives StreamLanguage.define. SQL and
// YAML are lang-sql's and lang-yaml's, in the main bundle.
const LEGACY_MODES = [['shell', 'shell'], ['groovy', 'groovy'], ['properties', 'properties'], ['toml', 'toml'],
  ['dockerfile', 'dockerFile'], ['nginx', 'nginx']];
const LEGACY_ENTRY = LEGACY_MODES.map(([file, name]) => `export { ${name} } from '@codemirror/legacy-modes/mode/${file}';`).join('\n') + '\n';
// The TypeScript compiler's declarations of the language the server's scripts are checked against (ES2023, no DOM), as
// one script that sets self.MoquiTsLibs for the checker's worker: lib.es5 to lib.es2023 and the decorators.
const TS_LIB = /^lib\.(es5|es20(1[5-9]|2[0-3])(\.[a-z.]+)?|decorators(\.legacy)?)\.d\.ts$/;
// What the CodeMirror bundle exports as the global CodeMirror: the state and view, the commands, search, completion, lint
// and merge view, the languages, the highlight tags for a theme, and the indentation markers
// (js/WebrootCode.js builds m-code-editor, m-diff-view and the dev-aide's editor from these).
const CODEMIRROR_ENTRY = `export { EditorState, EditorSelection, Compartment, StateEffect, StateField, Transaction, Annotation, ChangeSet,
  Text, Prec, Facet, RangeSetBuilder } from '@codemirror/state';
export { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, dropCursor,
  rectangularSelection, crosshairCursor, highlightSpecialChars, Decoration, ViewPlugin, WidgetType, gutter, GutterMarker,
  placeholder, showPanel, hoverTooltip, tooltips } from '@codemirror/view';
export { defaultKeymap, history, historyKeymap, indentWithTab, undo, redo, undoDepth, redoDepth, toggleComment,
  deleteLine, selectLine, insertBlankLine, moveLineUp, moveLineDown, copyLineUp, copyLineDown, indentMore, indentLess,
  toggleBlockComment, lineComment, lineUncomment, cursorMatchingBracket, undoSelection, selectParentSyntax }
  from '@codemirror/commands';
export { search, searchKeymap, highlightSelectionMatches, openSearchPanel, selectNextOccurrence, selectSelectionMatches,
  gotoLine, findNext, findPrevious } from '@codemirror/search';
export { autocompletion, completionKeymap, closeBrackets, closeBracketsKeymap, startCompletion, snippetCompletion }
  from '@codemirror/autocomplete';
export { linter, lintGutter, lintKeymap, setDiagnostics, forceLinting, nextDiagnostic, previousDiagnostic, openLintPanel }
  from '@codemirror/lint';
export { syntaxHighlighting, defaultHighlightStyle, HighlightStyle, indentOnInput, bracketMatching, foldGutter, foldKeymap,
  indentUnit, syntaxTree, ensureSyntaxTree, LanguageSupport, StreamLanguage, foldAll, unfoldAll, foldCode, unfoldCode }
  from '@codemirror/language';
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
// the whole modules, for the bundles built against this one (the Vim keymap, y-codemirror.next): each reads them here, so
// an editor has one copy of the state and the view (CM_SHIM)
import * as cmState from '@codemirror/state';
import * as cmView from '@codemirror/view';
import * as cmLanguage from '@codemirror/language';
import * as cmCommands from '@codemirror/commands';
import * as cmSearch from '@codemirror/search';
export const modules = { '@codemirror/state': cmState, '@codemirror/view': cmView, '@codemirror/language': cmLanguage,
  '@codemirror/commands': cmCommands, '@codemirror/search': cmSearch };
`;

// A bundle built against CodeMirror's or Yjs' takes those packages from its global (esbuild --alias to these shims), so a
// page has one copy of each: two of @codemirror/state break its instanceof checks, two of yjs its constructor checks.
const CM_SHIM = (name) => `if (!globalThis.CodeMirror || !globalThis.CodeMirror.modules) throw new Error('load /libs/codemirror/codemirror.min.js first');
module.exports = globalThis.CodeMirror.modules['${name}'];
`;
const YJS_SHIM = (member) => `if (!globalThis.Yjs) throw new Error('load /libs/yjs/yjs.min.js first');
module.exports = ${member};
`;
// What the Yjs library exports as the global Yjs: the yjs namespace (Y) and the awareness protocol, for m-coedit,
// Tiptap's collaboration (its bundle reads them here), the whiteboard and a co-edited code editor.
const YJS_ENTRY = `export * as Y from 'yjs';
export { Awareness, encodeAwarenessUpdate, applyAwarenessUpdate, removeAwarenessStates } from 'y-protocols/awareness';
`;
// What the y-codemirror.next bundle exports as the global YCodeMirror (against the globals CodeMirror and Yjs).
const Y_CODEMIRROR_ENTRY = `export { yCollab, ySync, ySyncFacet, ySyncAnnotation, yRemoteSelections, yRemoteSelectionsTheme, yUndoManagerKeymap,
  YRange } from 'y-codemirror.next';
`;
// What the Vim bundle exports as the global CodeMirrorVim (against the global CodeMirror): the extension, and Vim for
// its ex commands and mappings.
const CODEMIRROR_VIM_ENTRY = `export { vim, Vim, getCM } from '@replit/codemirror-vim';
`;

// What the Tiptap bundle exports as the global Tiptap: the editor and the extensions m-rich-text uses, and for m-coedit the
// Yjs namespace (Y), the collaboration extensions and the awareness protocol. Y and the awareness are the Yjs library's
// (the global Yjs, loaded first), not a copy in this bundle.
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

// The map of m-map (js/WebrootMap.js, plans/Maps.md §3.2): OpenLayers with ol-mapbox-style, ol-pmtiles and pmtiles, bundled
// as the global MoquiOl. OPENLAYERS_PACKAGES are the packages the bundle holds whose license file is in the package;
// pmtiles and ol-pmtiles carry none, so their license is PMTILES_LICENSE. The build fails when the bundle holds a package
// that is in neither list, or a second copy of one under another's node_modules.
// @maplibre/maplibre-gl-style-spec's dist holds its own copies of STYLE_SPEC_INLINED (under dist/node_modules); their license
// texts are those of the same packages installed beside it. Their pins in package.json (@mapbox/unitbezier, tinyqueue)
// exist only for that license text and move only with the style spec's; quickselect is also bundled through rbush.
const STYLE_SPEC_INLINED = ['@mapbox/unitbezier', 'tinyqueue', 'quickselect'];
const OPENLAYERS_PACKAGES = [
  'ol', 'ol-mapbox-style', 'mapbox-to-css-font', '@maplibre/maplibre-gl-style-spec', '@mapbox/unitbezier', 'tinyqueue',
  'pbf', 'rbush', 'quickselect', 'fflate',
];
const PMTILES_PACKAGES = ['pmtiles', 'ol-pmtiles'];
// What m-map uses: the map and view, the projection and extent helpers, the layers and sources, the formats of overlays
// and of the Natural Earth outline (TopoJSON), the controls, the default interactions with Select, Modify and Draw to
// pick a point, the style classes, an Overlay for a popup, Geolocation, ol-mapbox-style's apply, applyStyle and
// applyBackground for a MapLibre style, and the PMTiles sources, with pmtiles' FetchSource and FileSource for a source
// m-map makes itself.
const OPENLAYERS_ENTRY = `export { default as Map } from 'ol/Map.js';
export { default as View } from 'ol/View.js';
export { default as Overlay } from 'ol/Overlay.js';
export { default as Geolocation } from 'ol/Geolocation.js';
export { default as Feature } from 'ol/Feature.js';
export { default as Collection } from 'ol/Collection.js';
export { unByKey } from 'ol/Observable.js';
export { fromLonLat, toLonLat, transformExtent, get as getProjection } from 'ol/proj.js';
export { boundingExtent, buffer as bufferExtent, containsCoordinate, getCenter, isEmpty as isEmptyExtent } from 'ol/extent.js';
export { default as TileLayer } from 'ol/layer/Tile.js';
export { default as VectorLayer } from 'ol/layer/Vector.js';
export { default as VectorTileLayer } from 'ol/layer/VectorTile.js';
export { default as LayerGroup } from 'ol/layer/Group.js';
export { default as XYZ } from 'ol/source/XYZ.js';
export { default as VectorSource } from 'ol/source/Vector.js';
export { default as VectorTileSource } from 'ol/source/VectorTile.js';
export { default as GeoJSON } from 'ol/format/GeoJSON.js';
export { default as GPX } from 'ol/format/GPX.js';
export { default as KML } from 'ol/format/KML.js';
export { default as TopoJSON } from 'ol/format/TopoJSON.js';
export { default as MVT } from 'ol/format/MVT.js';
export { default as Point } from 'ol/geom/Point.js';
export { fromExtent as polygonFromExtent } from 'ol/geom/Polygon.js';
export { defaults as defaultControls } from 'ol/control/defaults.js';
export { default as Attribution } from 'ol/control/Attribution.js';
export { default as ScaleLine } from 'ol/control/ScaleLine.js';
export { default as Zoom } from 'ol/control/Zoom.js';
export { default as FullScreen } from 'ol/control/FullScreen.js';
export { defaults as defaultInteractions } from 'ol/interaction/defaults.js';
export { default as Select } from 'ol/interaction/Select.js';
export { default as Modify } from 'ol/interaction/Modify.js';
export { default as Draw } from 'ol/interaction/Draw.js';
export { default as Style } from 'ol/style/Style.js';
export { default as Fill } from 'ol/style/Fill.js';
export { default as Stroke } from 'ol/style/Stroke.js';
export { default as CircleStyle } from 'ol/style/Circle.js';
export { default as Icon } from 'ol/style/Icon.js';
export { default as Text } from 'ol/style/Text.js';
export { apply, applyStyle, applyBackground } from 'ol-mapbox-style';
export { PMTilesVectorSource, PMTilesRasterSource } from 'ol-pmtiles';
export { PMTiles, FetchSource, FileSource } from 'pmtiles';
`;
// The map loads what its layers name, so it fetches (styles, TileJSON, sprites, tiles: fetch and XMLHttpRequest), and the
// raster source behind a hillshade layer runs its operation in a Worker made from a Blob. It must not evaluate code or
// keep state of its own, and every URL it names (absolute, ws, or a protocol-relative //host in a string) is one of these,
// reviewed. A host built at run time (a template literal's ${...} in it) is listed with ${…} for each placeholder, and
// only an exact listing passes; a new host stops the build for review.
// - http://www.opengis.net, www.topografix.com, www.w3.org, earth.google.com, www.google.com, developers.google.com:
//   the XML namespaces and schema locations of KML, GPX and GML, never fetched.
// - https://api.mapbox.com and https://${…}.tiles.mapbox.com: ol-mapbox-style's normalizeSourceUrl and
//   normalizeStyleUrl rewrite a mapbox:// URL to these (the tiles hosts with or without an access token). m-map passes
//   only styles and sources its profiles name, so a mapbox:// URL is refused before it reaches the library.
// - https://cdn.jsdelivr.net: ol-mapbox-style's default webfonts template (Fontsource CSS). A style's own
//   metadata['ol:webfonts'] wins over the webfonts option, so m-map loads the style JSON itself, replaces any
//   metadata['ol:webfonts'] with the local template (libs/mapfonts) and passes the same template as webfonts. The template
//   is absolute (the page's origin first): ol-mapbox-style resolves a root-relative one against the style's URL, so for a
//   hosted style it would ask the provider's host.
// - https://maps.google.com: KML's default pushpin icon. A KML IconStyle without an Icon gets it, and an Icon href is
//   loaded as the file is parsed; neither extractStyles: false nor defaultStyle stops either, since the IconStyle is still
//   read and its image loaded. m-map reads KML with an iconUrlFunction that answers a blank data: icon for every href (the
//   pushpin's too), with extractStyles: false, and styles the layer itself.
// - https://github.com: a link in an error message.
const OPENLAYERS_HOSTS = [
  'http://earth.google.com', 'http://www.google.com', 'http://www.opengis.net', 'http://www.topografix.com',
  'http://www.w3.org', 'https://api.mapbox.com', 'https://${…}.tiles.mapbox.com', 'https://cdn.jsdelivr.net',
  'https://developers.google.com', 'https://github.com', 'https://maps.google.com',
];
// The license of pmtiles and ol-pmtiles, whose npm packages carry no license file: the LICENSE of their repository at the
// commit each release names (gitHead 3b10e67e for pmtiles 4.5.0, 3f8279aa for ol-pmtiles 2.0.2; the same text at both).
const PMTILES_LICENSE = `From https://github.com/protomaps/PMTiles/blob/3b10e67edb65c6b04549f74c0279cef8328d859c/LICENSE (pmtiles and
ol-pmtiles are both published from that repository; their npm packages carry no license file):

The below license (BSD-3) applies to the reference implementations in this repository.

The PMTiles specification itself is public domain, or CC0 where applicable.

Sample tilesets available in this repository are subject to their own license terms.

---

Copyright 2021 Protomaps LLC

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following
disclaimer in the documentation and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote products
derived from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.`;
// The faces of Noto Sans the packaged map styles name: OpenFreeMap's liberty (Noto Sans Regular, Italic and Bold) and the
// Protomaps basemap (Regular, Italic and Medium). ol-mapbox-style asks for a face the page lacks at the style's
// metadata['ol:webfonts'], else its webfonts option; m-map sets both to the page's origin and
// /libs/mapfonts/{font-family}/{fontweight}{-fontstyle}.css
// (OPENLAYERS_HOSTS). That is the Fontsource layout, so each face is that package's own CSS (every subset, each by
// unicode-range), with only the woff2 sources.
const MAP_FONT_FACES = ['400', '400-italic', '500', '700'];
// The local Protomaps profile (plans/Maps.md §4.1, §5): a regional Protomaps PMTiles archive registered as a layer draws
// with these. The styles are the output of @protomaps/basemaps (layers() of a namedFlavor, lang "en"), generated here
// with Node from the package's CommonJS build into protomaps/light.json and protomaps/dark.json, each with one vector
// source, "protomaps", that m-map binds to the archive (applyStyle, updateSource false), its sprite at
// /libs/protomaps/sprites/v4/<flavor> and no glyphs (ol-mapbox-style draws text with the web fonts of MAP_FONT_FACES:
// lang "en" names Noto Sans Regular, Medium and Italic only; another language would name script faces such as
// "Noto Sans Devanagari Regular v1", which are not packaged). The build fails when a generated style names a URL or a font
// that is not packaged. The sprites are those of github.com/protomaps/basemaps-assets, sprites/v4 at commit
// 028c18f713baecad011301ff7a69acc39bcc2ae7 (Oct 31, 2025), light and dark at 1x and 2x, kept as committed under
// build/libs/assets/protomaps-sprites-v4 since no npm package carries them; their license is that directory's
// (derived from tangrams/icons, MIT). The package carries no license file: PROTOMAPS_LICENSE is LICENSE.md of its
// repository at the commit 5.7.2 names (gitHead 3ea8293a).
const PROTOMAPS_FLAVORS = ['light', 'dark'];
const PROTOMAPS_FONTS = ['Noto Sans Regular', 'Noto Sans Medium', 'Noto Sans Italic'];
const PROTOMAPS_SPRITES = 'assets/protomaps-sprites-v4';
const PROTOMAPS_LICENSE = `From https://github.com/protomaps/basemaps/blob/3ea8293a28131c3dc63f1bb20827bdb8a76df06f/LICENSE.md
(@protomaps/basemaps 5.7.2 is published from that repository; its npm package carries no license file):

# Code Licenses and Attribution

# The BSD 3-Clause License (BSD-3-Clause)

Copyright 2019-2023 Protomaps LLC, Kelso Cartography

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote products derived from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

# Visual Design

The visual design of the Protomaps basemap styles was created by Geraldine Sarmiento for Protomaps LLC and released under a Creative Commons 0 (CC0) license.

https://creativecommons.org/publicdomain/zero/1.0/

Certain map styles may also include derivatives of icons designed for Mapzen and released under an MIT license.

https://github.com/tangrams/icons/blob/master/LICENSE.md

# Tile Schema

The organization of layers and tags is adapted from the Tilezen project.

https://github.com/tilezen/vector-datasource
https://github.com/tilezen/vector-datasource/blob/master/docs/LICENSE-SCHEMA.md

# The MIT License (MIT)

Copyright (c) 2015-2018 Mapzen
Copyright (c) 2019 Linux Foundation

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.`;
const PROTOMAPS_SPRITES_LICENSE = `The sprites (protomaps/sprites/v4) are from https://github.com/protomaps/basemaps-assets/tree/028c18f713baecad011301ff7a69acc39bcc2ae7/sprites/v4,
whose README says they are "derived from MIT-licensed tangrams/icons"
(https://github.com/tangrams/icons/blob/92510779634f4a006c61ea70e50cb8c52c765a81/LICENSE.md):

The MIT License (MIT)

Copyright (c) 2017 Mapzen

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.`;

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
    id: 'mermaid', name: 'Mermaid', npm: 'mermaid', license: 'MIT AND ISC AND BSD-3-Clause AND Apache-2.0',
    copy: [],
    licenseNote: 'Mermaid core and its locked dependencies, bundled locally. DOMPurify is used under Apache-2.0.',
    build: 'esbuild of mermaid.core.mjs with the locked DOMPurify and KaTeX packages',
    produce: ctx => ctx.esbuildMermaid(),
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
      'the prosemirror-* packages, @tiptap/y-tiptap, lib0, @floating-ui/*, linkifyjs, ' +
      'orderedmap, rope-sequence and w3c-keyname: all MIT. yjs and y-protocols are the Yjs library\'s (libs/yjs), which ' +
      'the bundle reads from the global Yjs. Each license text is below once, with the packages it is the license of.',
    copy: [],
    noticePackages: TIPTAP_PACKAGES,
    noticeDedupe: true,
    build: 'esbuild --bundle --minify --format=iife --global-name=Tiptap --platform=browser --target=es2020 ' +
      '--legal-comments=none of an entry that exports Editor, Extension, Node, Mark, mergeAttributes, StarterKit, Link, ' +
      'Image, Mention, Placeholder, Table, TableRow, TableCell, TableHeader, PluginKey, Collaboration, CollaborationCaret, Y ' +
      '(the yjs namespace), Awareness, encodeAwarenessUpdate, applyAwarenessUpdate and removeAwarenessStates, with yjs and ' +
      'y-protocols aliased to shims that read the global Yjs (libs/yjs/yjs.min.js, loaded first). Exposes the global Tiptap.',
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
    // Yjs and its awareness protocol, the shared types of co-editing (m-coedit, Tiptap's collaboration, the whiteboard, a
    // co-edited code editor: plans/design-dev-aide.md F15), loaded before any of them. One library, so a page has one
    // copy of Yjs: two break its constructor checks.
    id: 'yjs', name: 'Yjs with y-protocols (bundled)', npm: 'yjs', license: 'MIT',
    licenseNote: 'Bundles yjs, y-protocols (its awareness protocol) and the lib0 modules they use: all MIT. Each license ' +
      'text is below once, with the packages it is the license of.',
    copy: [],
    noticePackages: YJS_PACKAGES,
    noticeDedupe: true,
    build: 'esbuild --bundle --minify --format=iife --global-name=Yjs --platform=browser --target=es2020 ' +
      '--legal-comments=none of an entry (YJS_ENTRY) that exports Y (the yjs namespace), Awareness, encodeAwarenessUpdate, ' +
      'applyAwarenessUpdate and removeAwarenessStates. Exposes the global Yjs.',
    produce: (ctx) => [['yjs/yjs.min.js', ctx.esbuildYjs()]],
  },
  {
    // The binding of a CodeMirror editor to a Y.Text (a co-edited file of the dev-aide, F15): its sync, the others' carets
    // and selections with their names, and Yjs' undo. Built against the CodeMirror and Yjs globals, loaded after them.
    id: 'y-codemirror', name: 'y-codemirror.next (bundled)', npm: 'y-codemirror.next', license: 'MIT',
    licenseNote: 'Bundles y-codemirror.next and the lib0 modules it uses: both MIT. @codemirror/state and view are read ' +
      'from the global CodeMirror, yjs from the global Yjs.',
    copy: [],
    noticePackages: Y_CODEMIRROR_PACKAGES,
    noticeDedupe: true,
    build: 'esbuild --bundle --minify --format=iife --global-name=YCodeMirror --platform=browser --target=es2020 ' +
      '--legal-comments=none of an entry (Y_CODEMIRROR_ENTRY) that exports yCollab, ySync, ySyncFacet, ySyncAnnotation, ' +
      'yRemoteSelections, yRemoteSelectionsTheme, yUndoManagerKeymap and YRange, with @codemirror/* and yjs aliased to shims ' +
      'that read the globals CodeMirror and Yjs. Exposes the global YCodeMirror.',
    produce: (ctx) => [['codemirror/y-codemirror.min.js', ctx.esbuildYCodeMirror()]],
  },
  {
    // The Vim keymap of the code editor (the editor settings' keymap "vim", plans/design-dev-aide.md 5.1), loaded only for
    // a user who picks it. Built against the CodeMirror global.
    id: 'codemirror-vim', name: 'Vim keybindings for CodeMirror 6 (bundled)', npm: '@replit/codemirror-vim', license: 'MIT',
    licenseNote: 'Bundles @replit/codemirror-vim and @replit/codemirror-vim-core: both MIT. @codemirror/* are read from ' +
      'the global CodeMirror.',
    copy: [],
    noticePackages: CODEMIRROR_VIM_PACKAGES,
    noticeDedupe: true,
    build: 'esbuild --bundle --minify --format=iife --global-name=CodeMirrorVim --platform=browser --target=es2020 ' +
      '--legal-comments=none of an entry (CODEMIRROR_VIM_ENTRY) that exports vim, Vim and getCM, with @codemirror/* aliased ' +
      'to shims that read the global CodeMirror. Exposes the global CodeMirrorVim.',
    produce: (ctx) => [['codemirror/vim.min.js', ctx.esbuildCodeMirrorVim()]],
  },
  {
    // The second bundle of the code editor's languages (plans/design-dev-aide.md 5.1): legacy stream modes, loaded only
    // when a file needs one (js/WebrootCode.js LEGACY_EXTENSIONS).
    id: 'codemirror-legacy', name: 'CodeMirror legacy modes (bundled)', npm: '@codemirror/legacy-modes', license: 'MIT',
    licenseNote: 'The shell, groovy, properties, toml, dockerfile and nginx modes of @codemirror/legacy-modes (MIT), with ' +
      'its simple-mode helper.',
    copy: [],
    licenseFiles: ['LICENSE'],
    build: 'esbuild --bundle --minify --format=iife --global-name=CodeMirrorLegacy --platform=browser --target=es2020 ' +
      '--legal-comments=none of an entry (LEGACY_ENTRY) that exports the modes LEGACY_MODES names. ' +
      'Exposes the global CodeMirrorLegacy.',
    produce: (ctx) => [['codemirror/legacy-modes.min.js', ctx.esbuildLegacyModes()]],
  },
  {
    // The TypeScript compiler of the dev-aide's type checker (plans/design-dev-aide.md 5.4, question 10): a browser worker
    // runs it over the project's `moqui types` declarations, loaded only on the first JS tab with checking turned on. It
    // is large (lib/typescript.js is 8.8 MB, about 3.4 MB minified) and never loads with a page.
    id: 'typescript', name: 'TypeScript', npm: 'typescript', license: 'Apache-2.0',
    licenseNote: 'lib/typescript.js minified (its license header kept), and the ES5 to ES2023 and decorators ' +
      'declarations of lib/ as one script. ThirdPartyNoticeText.txt is the package\'s notice of what its declarations ' +
      'derive from.',
    copy: [],
    licenseFiles: ['LICENSE.txt', 'ThirdPartyNoticeText.txt'],
    build: 'esbuild --minify --legal-comments=inline on lib/typescript.js (a transform, no bundling), and lib-es2023.js: ' +
      'self.MoquiTsLibs set to the text of each lib/ file TS_LIB matches, by name.',
    produce: (ctx) => ctx.typescriptFiles(),
  },
  {
    // The map of m-map (js/WebrootMap.js, plans/Maps.md §3.2), loaded with moqui.loadScript when the first map mounts.
    // OpenLayers, ol-mapbox-style, ol-pmtiles and pmtiles ship ES modules, so this is a bundle of what OPENLAYERS_ENTRY
    // exports. Pinned Nov 25, 2026: each is the newest release of its line at least two weeks old (ol 10.11.0 of Oct 4,
    // ol-mapbox-style 13.9.0 of Nov 1, ol-pmtiles 2.0.2, which needs ol 9 or later, and pmtiles 4.5.0 of Aug 10).
    id: 'openlayers', name: 'OpenLayers with ol-mapbox-style and PMTiles (bundled)', npm: 'ol',
    license: 'BSD-2-Clause AND BSD-3-Clause AND ISC AND MIT',
    licenseNote: 'Bundles ol, ol-mapbox-style, mapbox-to-css-font and @mapbox/unitbezier (BSD-2-Clause), ol-pmtiles, ' +
      'pmtiles, pbf and @maplibre/maplibre-gl-style-spec (BSD-3-Clause: the style spec\'s package.json says ISC, its ' +
      'LICENSE.txt is the BSD-3-Clause text of MapLibre and of the Mapbox code it contains), quickselect and tinyqueue ' +
      '(ISC), rbush and fflate (MIT). ' +
      'ol.css is OpenLayers\'. Each license text is below once, with the packages it is the license of.',
    copy: [['ol.css', 'openlayers/ol.css']],
    noticeText: () => `--- ${PMTILES_PACKAGES.map((p) => p + ' ' + pins[p]).join(', ')}: LICENSE of the repository ---\n${PMTILES_LICENSE}\n`,
    noticePackages: OPENLAYERS_PACKAGES,
    noticeDedupe: true,
    build: 'esbuild --bundle --minify --format=iife --global-name=MoquiOl --platform=browser --target=es2020 ' +
      '--legal-comments=none of an entry (OPENLAYERS_ENTRY) that exports Map, View, Overlay, Geolocation, Feature, ' +
      'Collection, unByKey, the projection and extent helpers, the Tile, Vector, VectorTile and Group layers, the XYZ, ' +
      'Vector and VectorTile sources, the GeoJSON, GPX, KML, TopoJSON and MVT formats, Point and polygonFromExtent, the ' +
      'controls, the interactions, the style classes, ol-mapbox-style\'s apply, applyStyle and applyBackground, ' +
      'PMTilesVectorSource and PMTilesRasterSource, and PMTiles, FetchSource and FileSource. Exposes the global MoquiOl. ' +
      'The build fails when the bundle holds an unreviewed package, evaluates code, or names an unreviewed host.',
    produce: (ctx) => [['openlayers/ol-map.min.js', ctx.esbuildOpenLayers()]],
    licenseFiles: [],
  },
  {
    // The label fonts of the map styles m-map packages (MAP_FONT_FACES), so a map does not fetch them from jsDelivr (m-map
    // sets a style's metadata['ol:webfonts'] and the webfonts option to them; OPENLAYERS_HOSTS).
    id: 'mapfonts', name: 'Noto Sans (map label fonts)', npm: '@fontsource/noto-sans', license: 'OFL-1.1',
    licenseNote: `The faces ${MAP_FONT_FACES.join(', ')}, every subset, woff2 only.`,
    copy: [],
    build: `For each face of ${MAP_FONT_FACES.join(', ')}: its Fontsource CSS with only the woff2 source of each ` +
      '@font-face, at mapfonts/noto-sans/<face>.css, and those woff2 files under mapfonts/noto-sans/files/.',
    produce: (ctx) => ctx.mapFonts(),
    licenseFiles: ['LICENSE'],
  },
  {
    // The local Protomaps profile's styles and sprites (PROTOMAPS_FLAVORS; plans/Maps.md §4.1), for a regional archive
    // registered as the LOCAL_PROTOMAPS layers (framework data/MapsSetupData.xml). Pinned Nov 25, 2026: 5.7.2 of Mar 10.
    id: 'protomaps', name: 'Protomaps basemap styles (generated) and sprites', npm: '@protomaps/basemaps',
    license: 'BSD-3-Clause AND CC0-1.0 AND MIT',
    licenseNote: 'The styles are generated by the package\'s code (BSD-3-Clause); their visual design is CC0, with ' +
      'derivatives of Mapzen icons (MIT), and the layer schema is adapted from Tilezen. The sprites are those of ' +
      'protomaps/basemaps-assets (derived from tangrams/icons, MIT).',
    copy: [],
    build: `For each flavor of ${PROTOMAPS_FLAVORS.join(', ')}: the package's layers("protomaps", namedFlavor(flavor), ` +
      '{lang: "en"}), run by Node from dist/cjs/index.cjs, as a style (version 8, the source protomaps of type vector ' +
      'with the Protomaps and OpenStreetMap attribution, sprite /libs/protomaps/sprites/v4/<flavor>, no glyphs) at ' +
      'protomaps/<flavor>.json; and the committed sprites of basemaps-assets sprites/v4 (build/libs/' + PROTOMAPS_SPRITES +
      ') at protomaps/sprites/v4/. Fails when a style names a URL or a font that is not packaged.',
    produce: (ctx) => ctx.protomapsStyles(),
    noticeText: () => `${PROTOMAPS_LICENSE}\n\n${PROTOMAPS_SPRITES_LICENSE}\n`,
    licenseFiles: [],
  },
  {
    // The OpenUI Lang canvas of Assist (plans/LLM-A2A-Assist.md 4.10): parsed in the browser by the renderer and on the
    // server in QuickJS. The license texts are lang-core's and zod's (noticePackages).
    id: 'openui-lang-core', name: 'OpenUI Lang core (bundled with zod)', npm: '@openuidev/lang-core', license: 'MIT', licenseNote: 'Bundles @openuidev/lang-core and the parts of zod ' + pins.zod + ' it uses (MIT). Patched: ' +
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

LIBS.push(...fontLibraries());

const LICENSE_TEXTS = [
  { path: 'licenses/LGPL-2.1.txt', url: LGPL_URL },
  { path: 'licenses/OFL-1.1.txt', url: OFL_URL },
  { path: 'licenses/Unicode-3.0.txt', url: UNICODE_URL },
  // Kept as committed (no url): made once per pin from the crates' own license files (see CRYPTO_CRATES_NOTICE).
  { path: CRYPTO_CRATES_NOTICE, url: null },
];

function main() {
  withWork(workArg, buildAssets);
}

function buildAssets(work) {
  const outputs = new Map(); // path under libs/ -> Buffer
  const libMeta = [];
  const notices = [];
  const packages = {};

  const build = installProject(join(projects, 'libs'), join(work, 'libs'));
  const lock = JSON.parse(readFileSync(join(projects, 'libs/package-lock.json')));
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
    run(esbuildBinary(join(build, 'node_modules')), [...argv, `--outfile=${out}`, '--log-level=error'], { cwd: build });
    return readFileSync(out);
  };
  const ctx = {
    package: id => packages[id],
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
    esbuildMermaid() {
      writeFileSync(join(build, 'mermaid-entry.mjs'), 'import mermaid from "mermaid"; globalThis.mermaid = mermaid;\n');
      const bytes = esbuild(['mermaid-entry.mjs', '--bundle', '--format=iife', '--platform=browser', '--minify',
        '--target=es2020', '--legal-comments=inline', '--metafile=mermaid-inputs.json']);
      const inputs = JSON.parse(readFileSync(join(build, 'mermaid-inputs.json'))).inputs;
      const packageDirs = new Set(Object.keys(inputs).filter(path => path.startsWith('node_modules/'))
        .map(path => path.match(/^(.*node_modules\/(?:@[^/]+\/)?[^/]+)/)[1]));
      const notes = [];
      for (const path of [...packageDirs].sort()) {
        const dir = join(build, path);
        const info = JSON.parse(readFileSync(join(dir, 'package.json')));
        const licenses = readdirSync(dir).sort().filter(name => /^(licen[sc]e|copying)([.-]|$)/i.test(name));
        let text = licenses.map(name => readFileSync(join(dir, name), 'utf8').trim()).join('\n');
        if (!text && info.name === 'fastdom') text = readFileSync(join(dir, 'README.md'), 'utf8').split('## License')[1]?.trim();
        if (!text) throw new Error('Missing Mermaid dependency notice for ' + info.name);
        notes.push(info.name + '@' + info.version + '\n' + text);
      }
      for (const pkg of ['dompurify', 'katex']) {
        if (!packageDirs.has('node_modules/' + pkg)) throw new Error('Missing locked Mermaid dependency ' + pkg);
      }
      return [['mermaid/mermaid.min.js', bytes], ['mermaid/NOTICES.txt', Buffer.from(notes.join('\n\n') + '\n')]];
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
        '--target=es2020', '--legal-comments=none', ...ctx.yjsShims()]);
    },
    // esbuild --alias arguments that take yjs and y-protocols' awareness from the global Yjs
    yjsShims() {
      mkdirSync(join(build, 'shims/y-protocols'), { recursive: true });
      writeFileSync(join(build, 'shims/yjs.js'), YJS_SHIM('globalThis.Yjs.Y'));
      writeFileSync(join(build, 'shims/y-protocols/awareness.js'), YJS_SHIM('globalThis.Yjs'));
      return ['--alias:yjs=./shims/yjs.js', '--alias:y-protocols=./shims/y-protocols'];
    },
    // esbuild --alias arguments that take @codemirror/* from the global CodeMirror (its modules)
    codemirrorShims() {
      const args = [];
      mkdirSync(join(build, 'shims/codemirror'), { recursive: true });
      for (const name of ['state', 'view', 'language', 'commands', 'search']) {
        writeFileSync(join(build, `shims/codemirror/${name}.js`), CM_SHIM('@codemirror/' + name));
        args.push(`--alias:@codemirror/${name}=./shims/codemirror/${name}.js`);
      }
      return args;
    },
    esbuildYjs() {
      writeFileSync(join(build, 'yjs-entry.js'), YJS_ENTRY);
      return esbuild(['yjs-entry.js', '--bundle', '--minify', '--format=iife', '--global-name=Yjs', '--platform=browser',
        '--target=es2020', '--legal-comments=none']);
    },
    esbuildYCodeMirror() {
      writeFileSync(join(build, 'ycm-entry.js'), Y_CODEMIRROR_ENTRY);
      return esbuild(['ycm-entry.js', '--bundle', '--minify', '--format=iife', '--global-name=YCodeMirror',
        '--platform=browser', '--target=es2020', '--legal-comments=none', ...ctx.codemirrorShims(), '--alias:yjs=./shims/yjs.js']);
    },
    esbuildCodeMirrorVim() {
      writeFileSync(join(build, 'cmvim-entry.js'), CODEMIRROR_VIM_ENTRY);
      return esbuild(['cmvim-entry.js', '--bundle', '--minify', '--format=iife', '--global-name=CodeMirrorVim',
        '--platform=browser', '--target=es2020', '--legal-comments=none', ...ctx.codemirrorShims()]);
    },
    esbuildLegacyModes() {
      writeFileSync(join(build, 'cmlegacy-entry.js'), LEGACY_ENTRY);
      return esbuild(['cmlegacy-entry.js', '--bundle', '--minify', '--format=iife', '--global-name=CodeMirrorLegacy',
        '--platform=browser', '--target=es2020', '--legal-comments=none']);
    },
    typescriptFiles() {
      const root = packages.typescript.root;
      const compiler = esbuild([join(root, 'lib', 'typescript.js'), '--minify', '--legal-comments=inline']);
      const libs = {};
      for (const name of readdirSync(join(root, 'lib')).sort()) {
        if (TS_LIB.test(name)) libs[name] = toLf(readFileSync(join(root, 'lib', name))).toString('utf8');
      }
      if (!libs['lib.es2023.d.ts'] || !libs['lib.es5.d.ts']) throw new Error('typescript has no lib.es5.d.ts or lib.es2023.d.ts');
      const script = '/* The ES5 to ES2023 declarations of TypeScript ' + pins.typescript + ' (Apache-2.0), by file name. */\n' +
        'self.MoquiTsLibs = ' + JSON.stringify(libs) + ';\n';
      return [['typescript/typescript.min.js', compiler], ['typescript/lib-es2023.js', Buffer.from(script, 'utf8')]];
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
    esbuildOpenLayers() {
      writeFileSync(join(build, 'ol-entry.js'), OPENLAYERS_ENTRY);
      const out = esbuild(['ol-entry.js', '--bundle', '--minify', '--format=iife', '--global-name=MoquiOl',
        '--platform=browser', '--target=es2020', '--legal-comments=none', '--metafile=ol-inputs.json']);
      // the packages in the bundle are the reviewed ones, each the single top-level copy whose license is printed
      const output = Object.values(JSON.parse(readFileSync(join(build, 'ol-inputs.json'))).outputs)[0];
      const reviewed = [...OPENLAYERS_PACKAGES, ...PMTILES_PACKAGES];
      for (const [path, input] of Object.entries(output.inputs)) {
        if (!input.bytesInOutput || path === 'ol-entry.js') continue;
        // the style spec's dist carries its own copies of a few small dependencies (STYLE_SPEC_INLINED)
        const inlined = path.match(/^node_modules\/@maplibre\/maplibre-gl-style-spec\/dist\/node_modules\/((?:@[^/]+\/)?[^/]+)\//);
        if (inlined && STYLE_SPEC_INLINED.includes(inlined[1])) continue;
        const pkg = path.match(/^node_modules\/((?:@[^/]+\/)?[^/]+)\/(.*)$/);
        if (!pkg || pkg[2].includes('node_modules/')) throw new Error(`the OpenLayers bundle holds ${path}, not a top-level package`);
        if (!reviewed.includes(pkg[1])) throw new Error(`the OpenLayers bundle holds ${pkg[1]}: review its license and add it`);
      }
      const text = out.toString('utf8');
      for (const banned of ['eval(', 'new function(', 'importscripts', 'websocket', 'sendbeacon', 'document.cookie',
        'localstorage', 'sessionstorage', 'indexeddb']) {
        if (text.toLowerCase().includes(banned)) throw new Error(`the OpenLayers bundle names ${banned}`);
      }
      // every scheme://host and string-literal //host, a template placeholder in it written ${…}
      const host = (name) => name.replace(/\$\{[^}]*\}/g, '${…}').toLowerCase();
      const hosts = new Set([
        ...[...text.matchAll(/\b((?:https?|wss?):)\/\/([^\s"'`/\\)<>,;?#]+)/gi)].map((m) => m[1].toLowerCase() + '//' + host(m[2])),
        ...[...text.matchAll(/["'`]\/\/([^\s"'`/\\)<>,;?#]+\.[^\s"'`/\\)<>,;?#]+)/g)].map((m) => '//' + host(m[1])),
      ]);
      for (const name of hosts) {
        if (OPENLAYERS_HOSTS.includes(name)) continue;
        throw new Error(name.includes('${') ? `the OpenLayers bundle builds a URL host at run time, ${name}: review it and add it`
          : `the OpenLayers bundle names ${name}: review it and add it`);
      }
      return out;
    },
    mapFonts() {
      const root = packages.mapfonts.root;
      const files = [];
      for (const face of MAP_FONT_FACES) {
        const css = readFileSync(join(root, `${face}.css`), 'utf8');
        const rules = [...css.matchAll(/(?:\/\*[^*]*\*\/\s*)?@font-face\s*\{[^}]+\}/g)].map(([rule]) => {
          const src = rule.match(/src: url\(\.\/files\/([\w-]+\.woff2)\) format\('woff2'\), url\(\.\/files\/[\w-]+\.woff\) format\('woff'\);/);
          if (!src) throw new Error(`Unknown Fontsource CSS in ${face}.css of @fontsource/noto-sans`);
          files.push([`mapfonts/noto-sans/files/${src[1]}`, readFileSync(join(root, 'files', src[1]))]);
          return rule.replace(src[0], `src: url(./files/${src[1]}) format('woff2');`);
        });
        if (!rules.length) throw new Error(`No @font-face in ${face}.css of @fontsource/noto-sans`);
        files.push([`mapfonts/noto-sans/${face}.css`, Buffer.from(rules.join('\n\n') + '\n')]);
      }
      return files;
    },
    protomapsStyles() {
      const root = packages.protomaps.root;
      const files = [];
      const generator = `const pm = require(${JSON.stringify(join(root, 'dist/cjs/index.cjs'))});
const out = {};
for (const flavor of ${JSON.stringify(PROTOMAPS_FLAVORS)}) out[flavor] = pm.layers('protomaps', pm.namedFlavor(flavor), {lang: 'en'});
process.stdout.write(JSON.stringify(out));`;
      const generated = JSON.parse(run(process.execPath, ['-e', generator], { cwd: build }));
      const version = LIBS.find((l) => l.id === 'protomaps').version;
      for (const flavor of PROTOMAPS_FLAVORS) {
        const layers = generated[flavor];
        if (!Array.isArray(layers) || !layers.length) throw new Error(`@protomaps/basemaps made no layers for ${flavor}`);
        const style = {
          version: 8,
          name: `Protomaps ${flavor}`,
          metadata: { 'generated-by': `@protomaps/basemaps ${version} layers("protomaps", namedFlavor("${flavor}"), {lang: "en"})` },
          sources: { protomaps: { type: 'vector', attribution: '<a href="https://protomaps.com">Protomaps</a> © <a href="https://openstreetmap.org/copyright">OpenStreetMap</a>' } },
          sprite: `/libs/protomaps/sprites/v4/${flavor}`,
          layers,
        };
        const text = JSON.stringify(layers);
        const urls = text.match(/[a-z][a-z0-9+.-]*:\/\/[^"\\]+/gi);
        if (urls) throw new Error(`the Protomaps ${flavor} style names ${urls[0]}: review it`);
        // every font a text-font names: a plain list of names, each name of a ["literal", [...]] of an expression, and any
        // other text of the expression with a space in it (an operator or a property name has none)
        for (const layer of layers) {
          const value = layer.layout?.['text-font'];
          if (value === undefined) continue;
          const fonts = [];
          const walk = (node, top) => {
            if (!Array.isArray(node)) return;
            if (node[0] === 'literal' && Array.isArray(node[1])) { fonts.push(...node[1]); return; }
            if (top && node.every((item) => typeof item === 'string')) { fonts.push(...node); return; }
            for (const item of node) {
              if (typeof item === 'string' && item.includes(' ')) fonts.push(item);
              else walk(item, false);
            }
          };
          if (typeof value === 'string') fonts.push(value); else walk(value, true);
          if (!fonts.length) throw new Error(`the Protomaps ${flavor} style's layer ${layer.id} has a text-font naming no font`);
          for (const font of fonts) if (!PROTOMAPS_FONTS.includes(font)) throw new Error(`the Protomaps ${flavor} style names the font ${JSON.stringify(font)}, which is not packaged`);
        }
        files.push([`protomaps/${flavor}.json`, Buffer.from(JSON.stringify(style) + '\n')]);
        for (const name of [`${flavor}.json`, `${flavor}.png`, `${flavor}@2x.json`, `${flavor}@2x.png`])
          files.push([`protomaps/sprites/v4/${name}`, readFileSync(join(source, PROTOMAPS_SPRITES, name))]);
      }
      return files;
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
    status(publishing ? 'Bundling' : 'Checking', `${lib.id} v${lib.version} (${lib.npm})`);
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
    if (lib.noticeText) text += lf(lib.noticeText()) + '\n';
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
      ...(lib.licenseNote ? { note: lf(lib.licenseNote) } : {}),
      source: `https://registry.npmjs.org/${lib.npm}/-/${lib.npm.split('/').pop()}-${lib.version}.tgz`,
      npm: `${lib.npm}@${lib.version}`, tarballIntegrity: packages[lib.id].integrity,
      ...(lib.build ? { build: lf(lib.build) } : {}),
      ...(lib.patches ? { patches: lib.patches.map((path) => ({ path, sha256: sha256(toLf(readFileSync(join(source, path)))) })) } : {}),
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
    buf = toLf(buf);
    outputs.set(path, buf);
    lt.push({ path, from: url || 'committed', size: buf.length, sha256: sha256(buf) });
  }
  libMeta.push({ id: 'license-texts', name: 'License texts', version: '-', license: 'n/a',
    source: 'see each file', files: lt });

  const boardProject = installProject(join(projects, 'whiteboard'), join(work, 'whiteboard'));
  const editor = JSON.parse(readFileSync(join(boardProject, 'package-lock.json'))).packages['node_modules/@excalidraw/excalidraw'];
  status(publishing ? 'Bundling' : 'Checking', `excalidraw v${editor.version} (@excalidraw/excalidraw)`);
  const board = buildBoard(boardProject, true);
  for (const [path, data] of board.outputs) outputs.set(path, data);
  const cjk = packs['excalidraw-cjk'].sources[0];
  if (cjk.integrity !== board.lib.tarballIntegrity || cjk.version !== board.lib.version) throw new Error('Review Xiaolai source in fonts.json');
  libMeta.push(board.lib);
  for (const lib of libMeta) for (const file of lib.files) {
    const pack = assetPack(file.path);
    if (pack) file.pack = pack;
  }
  notices.push(board);
  const noticesMd = renderNotices(notices);
  outputs.set('NOTICES.md', Buffer.from(noticesMd, 'utf8'));
  const manifest = {
    generatedBy: 'build/libs/refresh.mjs',
    notices: { path: 'NOTICES.md', sha256: sha256(outputs.get('NOTICES.md')) },
    libraries: libMeta,
  };
  const manifestText = JSON.stringify(manifest, null, 2) + '\n';

  checkManifest(here, manifestText, outputs, write);
  if (publishing) {
    const selected = (process.env.JS_PACKS || '').split(' ').filter(Boolean);
    for (const path of outputs.keys()) {
      const pack = assetPack(path);
      if (pack && !selected.includes(pack)) outputs.delete(path);
    }
    outputs.set('fonts-extra/installed.json', Buffer.from(JSON.stringify(selected) + '\n'));
    outputs.set('MANIFEST.json', Buffer.from(manifestText));
    publish(here, outputs);
  }
  summary({ build: publishing, component: componentName() || 'webroot', files: outputs.size, output: outputLabel(here),
    libraries: libMeta.filter((lib) => lib.id !== 'license-texts').length });
}

function renderNotices(notices) {
  let md = lf(`# Third-party notices

The files under \`libs/\` are third-party browser libraries served as their own files. They are not compiled into the
\`moqui\` binary. \`MANIFEST.json\` lists each file with its sha256, and \`refresh.mjs\` rebuilds them from the pinned
versions below. The license of each library applies to its files only. The framework's own CC0 license does not
relicense them, and theirs does not apply to Moqui code.

`);
  for (const { lib, text } of notices) {
    md += `## ${lib.name} ${lib.version}\n\nLicense: ${lib.license}. Source: npm \`${lib.npm}@${lib.version}\`.\n`;
    if (lib.licenseNote) md += `${lf(lib.licenseNote)}\n`;
    md += `\n\`\`\`text\n${text.trimEnd()}\n\`\`\`\n\n`;
  }
  return md;
}

main();
