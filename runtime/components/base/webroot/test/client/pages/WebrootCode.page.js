/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The code editor's helpers in the page. editor is moqui.codeEditor from /js/WebrootCode.js.
// CodeMirror is the vendored bundle, the same IIFE the Node file ran with vm.runInThisContext; the legacy modes, the Vim
// keymap, Yjs and y-codemirror.next are its side bundles, built against its global.
"use strict";

const editor = moqui.codeEditor;
const CM = CodeMirror;

test("the language of a file comes from its extension, else plain text", () => {
    assert.equal(editor.languageOf("service/org/moqui/impl/UserServices.xml"), "xml");
    assert.equal(editor.languageOf("screen/Gallery/QuickTask.qvue"), "vue");
    assert.equal(editor.languageOf("screen/Home.tui.json"), "json");
    assert.equal(editor.languageOf("script/Run.qjs"), "javascript");
    assert.equal(editor.languageOf("README.MD"), "markdown");
    assert.equal(editor.languageOf("src\\lib.rs"), "rust");
    assert.equal(editor.languageOf("notes"), "text");
    assert.equal(editor.languageOf(""), "text");
    // the second bundle's languages, by extension or by the whole name
    assert.equal(editor.languageOf("scripts/check-packages.sh"), "shell");
    assert.equal(editor.languageOf("build.gradle"), "groovy");
    assert.equal(editor.languageOf("conf/app.properties"), "properties");
    assert.equal(editor.languageOf("Cargo.toml"), "toml");
    assert.equal(editor.languageOf("podman/Dockerfile"), "dockerfile");
    assert.equal(editor.languageOf("podman/Containerfile.dev"), "dockerfile");
    assert.equal(editor.languageOf("conf/nginx.conf"), "nginx");
});

test("a failed load says why, from the answer's errors, its status or the error's message", () => {
    assert.equal(editor.failureText({ ok: false, status: 403, statusText: "Forbidden", json: { errors: ["Not allowed to read"] } }), "Not allowed to read");
    assert.equal(editor.failureText({ ok: false, status: 500, json: { errorMessage: "Root not found" } }), "Root not found");
    assert.equal(editor.failureText({ ok: false, status: 404, statusText: "Not Found", json: null }), "HTTP 404 Not Found");
    assert.equal(editor.failureText(new Error("network down")), "network down");
    assert.equal(editor.failureText({}), "The folder could not be read");
});

test("a SQL editor takes the database's dialect and completes the schema's tables", () => {
    assert.equal(editor.sqlDialectName("embedded-postgres"), "PostgreSQL");
    assert.equal(editor.sqlDialectName("mysql8"), "MySQL");
    assert.equal(editor.sqlDialectName("mariadb"), "MariaSQL");
    assert.equal(editor.sqlDialectName(""), "StandardSQL");
    const schema = { enumeration: ["enum_id", "description"] };
    const config = editor.sqlConfig(CM, "sql:postgres", schema);
    assert.equal(config.dialect, CM.PostgreSQL);
    assert.equal(config.schema, schema);
    assert.equal(editor.sqlConfig(CM, "sql").dialect, CM.StandardSQL);
    // and lang-sql takes it
    const extension = editor.languageExtension(CM, "sql:mariadb", schema);
    const state = CM.EditorState.create({ doc: "select enum_id from enumeration", extensions: [extension] });
    assert.equal(state.doc.lines, 1);
    assert.ok(CM.MySQL && CM.MariaSQL, "the bundle has the dialects");
});

test("a folder shows its most pressing entry status", () => {
    assert.equal(editor.rollupStatus(["", "?", "M"]), "M");
    assert.equal(editor.rollupStatus(["A", "U", "M"]), "U");
    assert.equal(editor.rollupStatus(["D", "?"]), "D");
    assert.equal(editor.rollupStatus(["", "!"]), "");
    assert.equal(editor.rollupStatus([]), "");
});

test("the schema digest becomes lang-xml elements", () => {
    const elements = editor.xmlElements({
        root: "services",
        elements: [
            { name: "services", top: true, children: ["service"], attributes: [] },
            { name: "service", children: ["actions"], attributes: [{ name: "verb", required: true },
                { name: "authenticate", values: ["true", "false", "anonymous-all"] }] },
            { name: "actions", openChildren: true, children: [], attributes: [] }
        ]
    });
    assert.deepEqual(elements[0], { name: "services", top: true, children: ["service"], attributes: [] });
    assert.deepEqual(elements[1].attributes, ["verb", { name: "authenticate", values: ["true", "false", "anonymous-all"] }]);
    assert.equal(elements[2].children, undefined, "an open element takes any child");
    assert.deepEqual(editor.xmlElements(null), []);
    // and lang-xml takes them
    const state = CM.EditorState.create({ doc: "<services><service verb=\"run\"/></services>", extensions: [CM.xml({ elements })] });
    assert.equal(state.doc.lines, 1);
});

test("problems become diagnostics on their word, or their line", () => {
    const text = "<services>\n  <entity-find entity-name=\"no.Such\"/>\n</services>";
    const [onWord, onLine, past] = editor.diagnostics([
        { line: 2, column: 16, severity: "error", rule: "reference.entity", message: "entity no.Such" },
        { line: 3, column: 0, severity: "warning", message: "whole line" },
        { line: 9, column: 99, severity: "info", message: "past the end" }
    ], text);
    const second = text.indexOf("\n") + 1;
    assert.equal(onWord.from, second + 15);
    assert.equal(text.slice(onWord.from, onWord.to), "entity-name");
    assert.equal(onWord.severity, "error");
    assert.equal(onWord.message, "entity no.Such [reference.entity]");
    assert.equal(text.slice(onLine.from, onLine.to), "</services>");
    assert.equal(onLine.severity, "warning");
    assert.ok(past.from <= text.length && past.to <= text.length);
});

test("xml-op steps are one transaction of sequential changes", () => {
    // what apply#XmlOps answers for a set-attribute then an insert (crates/moqui-lib/testdata/xml-edit-vectors.json)
    const start = "<a t=\"\u{1F600}\">\n    <b/>\n</a>\n";
    const steps = [
        [{ from: 19, to: 19, from16: 17, to16: 17, text: " x=\"1\"" }],
        [{ from: 27, to: 27, from16: 25, to16: 25, text: "\n    <c/>" }]
    ];
    const specs = editor.xmlOpSpecs(steps);
    assert.equal(specs.length, 2);
    assert.equal(specs[1].sequential, true);
    const state = CM.EditorState.create({ doc: start });
    const result = state.update(...specs).state.doc.toString();
    assert.equal(result, "<a t=\"\u{1F600}\">\n    <b x=\"1\"/>\n    <c/>\n</a>\n");
    assert.deepEqual(editor.xmlOpSpecs([[], null]), []);
});

test("new text replaces only the part that differs", () => {
    assert.deepEqual(editor.textChange("abcdef", "abXYef"), { from: 2, to: 4, insert: "XY" });
    assert.deepEqual(editor.textChange("same", "same"), { from: 4, to: 4, insert: "" });
    assert.deepEqual(editor.textChange("", "new"), { from: 0, to: 0, insert: "new" });
    assert.deepEqual(editor.textChange("aaa", "aa"), { from: 2, to: 3, insert: "" });
});

test("a file tree entry is a node keyed by its path", () => {
    assert.deepEqual(editor.treeNode("service/org", { name: "moqui", dir: true, status: "M" }),
        { path: "service/org/moqui", name: "moqui", dir: true, status: "M", ignored: false, lazy: true, children: undefined });
    assert.deepEqual(editor.treeNode("", { name: "component.xml" }),
        { path: "component.xml", name: "component.xml", dir: false, status: "", ignored: false, lazy: false, children: [] });
});

test("the bundle has what the widgets use, and a state for every language", () => {
    for (const name of ["EditorState", "EditorView", "Compartment", "RangeSetBuilder", "Decoration", "Text", "keymap", "lineNumbers",
        "highlightActiveLineGutter", "highlightSpecialChars", "history", "foldGutter", "drawSelection", "dropCursor", "indentOnInput",
        "bracketMatching", "closeBrackets", "autocompletion", "rectangularSelection", "crosshairCursor", "highlightActiveLine",
        "highlightSelectionMatches", "indentationMarkers", "closeBracketsKeymap", "defaultKeymap", "searchKeymap", "historyKeymap",
        "foldKeymap", "completionKeymap", "lintKeymap", "indentWithTab", "linter", "lintGutter", "placeholder", "MergeView",
        "unifiedMergeView", "HighlightStyle", "syntaxHighlighting", "defaultHighlightStyle", "tags", "undo", "redo"]) {
        assert.ok(CM[name], name);
    }
    const languages = new Set(Object.values(editor.extensions));
    for (const language of languages) {
        // a legacy mode: a stream language of the second bundle, made once a page
        if (editor.LEGACY_LANGUAGES[language]) {
            const made = editor.legacyLanguage(CM, language);
            assert.ok(made && made === editor.legacyLanguage(CM, language), language);
            const state = CM.EditorState.create({ doc: "x = 1", extensions: [editor.languageExtension(CM, language)] });
            assert.equal(state.doc.toString(), "x = 1", language);
            continue;
        }
        const factory = { xml: "xml", html: "html", vue: "vue", javascript: "javascript", json: "json", css: "css", markdown: "markdown",
            sql: "sql", yaml: "yaml", rust: "rust" }[language];
        assert.ok(factory && typeof CM[factory] === "function", language);
        const state = CM.EditorState.create({ doc: "x", extensions: [CM[factory]()] });
        assert.equal(state.doc.toString(), "x", language);
    }
});

test("the editor settings: defaults filled in, sizes bounded, only valid keys of known commands", () => {
    assert.deepEqual(editor.normalizeSettings(null), { fontSize: 13, tabSize: null, wrap: false, whitespace: false, keymap: "default", keys: {} });
    const given = editor.normalizeSettings({ fontSize: "16", tabSize: 40, wrap: true, whitespace: "true", keymap: "vim",
        keys: { toggleComment: "Mod-Shift-7", save: "Alt-s", nothing: "Mod-q", find: "Hyper-f" } });
    assert.deepEqual(given, { fontSize: 16, tabSize: null, wrap: true, whitespace: true, keymap: "vim", keys: { toggleComment: "Mod-Shift-7", save: "Alt-s" } });
    assert.ok(editor.validKeys("Ctrl-Space") && editor.validKeys("F2") && editor.validKeys("Mod--"));
    assert.ok(!editor.validKeys("") && !editor.validKeys("Mod-") && !editor.validKeys("Super-x"));
    // the bundle has each command a key rebinds
    for (const [name, of] of Object.entries(editor.COMMANDS)) if (of) assert.equal(typeof of(CM), "function", name);
});

test("a rebound command runs on its keys, ahead of the rest, and not on its default ones", () => {
    const comment = () => true, other = () => true;
    const bindings = [{ key: "Mod-/", run: comment }, { key: "Mod-x", run: other }, { key: "Mod-k", run: comment }];
    const out = editor.rebind(bindings, { toggleComment: "Alt-c", find: "Alt-f" }, { toggleComment: comment });
    assert.deepEqual(out.map((binding) => binding.key), ["Alt-c", "Mod-x"]);
    assert.equal(out[0].run, comment);
    assert.deepEqual(editor.rebind(bindings, {}, { toggleComment: comment }).map((binding) => binding.key), ["Mod-/", "Mod-x", "Mod-k"]);
    // with the bundle's keymaps: toggleComment's Mod-/ goes, its new key comes first
    const base = [].concat(CM.defaultKeymap, CM.historyKeymap);
    const rebound = editor.rebind(base, { toggleComment: "Alt-c", undo: "Alt-u" }, { toggleComment: CM.toggleComment, undo: CM.undo });
    assert.deepEqual(rebound.slice(0, 2).map((binding) => binding.key), ["Alt-c", "Alt-u"]);
    assert.ok(!rebound.slice(2).some((binding) => binding.run === CM.toggleComment || binding.run === CM.undo));
    assert.ok(rebound.some((binding) => binding.run === CM.redo));
});

test("an editor takes the settings: font size, tab size and indent, wrap, and the keys rebound over its keymap", () => {
    const methods = editor.components["m-code-editor"].methods;
    const editorOf = (settings, wrap) => {
        const vm = { settings, wrap, owner: { vm: null } };
        vm.effectiveSettings = methods.effectiveSettings.bind(vm);
        return methods.parts.call(vm, CM);
    };
    const parts = editorOf({ fontSize: 18, tabSize: 2, wrap: true, keys: { toggleComment: "Alt-c" } }, undefined);
    const state = CM.EditorState.create({ doc: "x", extensions: [parts.settings(), parts.keys()] });
    assert.equal(state.tabSize, 2);
    assert.equal(state.facet(CM.indentUnit), "  ");
    const bindings = state.facet(CM.keymap).flat();
    assert.ok(bindings.some((binding) => binding.key === "Alt-c" && binding.run === CM.toggleComment));
    assert.ok(!bindings.some((binding) => binding.key === "Mod-/"));
    assert.ok(bindings.some((binding) => binding.key === "Mod-s"), "save keeps its default");
    // wrap: the user's when the editor does not set it, else the editor's
    assert.equal(parts.wrap(), CM.EditorView.lineWrapping);
    assert.deepEqual(editorOf({ wrap: true }, false).wrap(), []);
    // the page's settings when an editor has none of its own; with no tab size set, CodeMirror's tab and indent
    const page = editor.settings;
    try {
        editor.setSettings({ tabSize: 8 });
        const defaults = editorOf(undefined, undefined);
        assert.equal(CM.EditorState.create({ doc: "", extensions: [defaults.settings()] }).tabSize, 8);
        editor.setSettings(null);
        const plain = CM.EditorState.create({ doc: "", extensions: [editorOf(undefined, undefined).settings()] });
        assert.equal(plain.tabSize, 4);
        assert.equal(plain.facet(CM.indentUnit), "  ");
    } finally {
        editor.setSettings(page);
    }
});

test("the keymaps: VS Code's bindings from CodeMirror's commands, Vim on the page's CodeMirror", () => {
    assert.deepEqual(Object.keys(editor.KEYMAPS).sort(), ["default", "vim", "vscode"]);
    const vscode = (rebound) => CM.EditorState.create({ doc: "", extensions: editor.KEYMAPS.vscode(CM, rebound) }).facet(CM.keymap).flat();
    const keys = vscode(new Set());
    for (const [key, run] of [["Mod-l", CM.selectLine], ["Mod-g", CM.gotoLine], ["Mod-h", CM.openSearchPanel], ["Mod-d", CM.selectNextOccurrence],
        ["Mod-k Mod-0", CM.foldAll], ["Mod-k Mod-j", CM.unfoldAll], ["Mod-k Mod-c", CM.lineComment], ["Shift-Alt-ArrowRight", CM.selectParentSyntax]]) {
        assert.ok(keys.some((binding) => binding.key === key && binding.run === run), key);
    }
    // a command the user rebound keeps only their keys
    assert.ok(!vscode(new Set([CM.foldAll])).some((binding) => binding.run === CM.foldAll));
    // Mod-Shift-Enter opens a line above, indented as the line; Mod-Alt-ArrowDown adds a cursor below at the column
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new CM.EditorView({ parent, state: CM.EditorState.create({ doc: "  one\n  two\nthree",
        selection: { anchor: 9 }, extensions: [CM.EditorState.allowMultipleSelections.of(true)] }) });
    try {
        assert.ok(keys.find((binding) => binding.key === "Mod-Shift-Enter").run(view));
        assert.equal(view.state.doc.toString(), "  one\n  \n  two\nthree");
        assert.equal(view.state.selection.main.head, 8);
        assert.ok(keys.find((binding) => binding.key === "Mod-Alt-ArrowDown").run(view));
        assert.deepEqual(view.state.selection.ranges.map((range) => range.head), [8, 11]);
    } finally {
        view.destroy();
        parent.remove();
    }
    // Vim's bundle reads the page's CodeMirror: its extension goes into a state with ours (two copies of
    // @codemirror/state would refuse it)
    assert.ok(globalThis.CodeMirrorVim && globalThis.CodeMirrorVim.Vim);
    const vim = editor.KEYMAPS.vim(CM);
    const state = CM.EditorState.create({ doc: "x", extensions: [vim, CM.history()] });
    assert.equal(state.doc.toString(), "x");
    // a keymap with its library loads it, then the editors take it; one with none is there at once
    return editor.loadKeymap("vim").then(() => editor.loadKeymap("vscode"));
});

test("a co-edited editor takes the shared text, its undo is Yjs', and two editors on synced documents converge", () => {
    const methods = editor.components["m-code-editor"].methods;
    const { Y } = globalThis.Yjs;
    const sides = [new Y.Doc(), new Y.Doc()];
    sides[0].getText("codemirror").insert(0, "shared\n");
    Y.applyUpdate(sides[1], Y.encodeStateAsUpdate(sides[0]));
    // each document's updates go to the other, as the room passes them on
    sides.forEach((doc, index) => doc.on("update", (update, origin) => { if (origin !== "peer") Y.applyUpdate(sides[1 - index], update, "peer"); }));
    const views = sides.map((doc) => {
        const ytext = doc.getText("codemirror");
        const vm = { collab: { ytext, awareness: new globalThis.Yjs.Awareness(doc), undoManager: new Y.UndoManager(ytext) }, owner: { vm: null } };
        vm.effectiveSettings = () => editor.normalizeSettings(null);
        const parts = methods.parts.call(vm, CM);
        // no CodeMirror history beside Yjs', and Yjs' undo on Mod-z
        assert.deepEqual(parts.history(), []);
        const bindings = CM.EditorState.create({ doc: "", extensions: [parts.keys()] }).facet(CM.keymap).flat();
        assert.ok(bindings.some((binding) => binding.key === "Mod-z" && globalThis.YCodeMirror.yUndoManagerKeymap.some((y) => y.run === binding.run)));
        const parent = document.createElement("div");
        document.body.appendChild(parent);
        return new CM.EditorView({ parent, state: CM.EditorState.create({ doc: ytext.toString(), extensions: [parts.collab(), parts.history(), parts.keys()] }) });
    });
    try {
        views[0].dispatch({ changes: { from: 0, insert: "one " } });
        views[1].dispatch({ changes: { from: views[1].state.doc.length, insert: "two" } });
        assert.equal(views[0].state.doc.toString(), "one shared\ntwo");
        assert.equal(views[1].state.doc.toString(), "one shared\ntwo");
    } finally {
        views.forEach((view) => { view.dom.parentNode.remove(); view.destroy(); });
        sides.forEach((doc) => doc.destroy());
    }
});

test("kept editor states come back by key, the most recent 50", () => {
    const state = CM.EditorState.create({ doc: "x", extensions: [CM.history()] });
    editor.keepState("a", { state, compartments: {}, owner: { vm: null } });
    assert.equal(editor.takeState("a").state, state);
    assert.equal(editor.takeState("a"), null);
    for (let i = 0; i < 55; i++) editor.keepState("k" + i, { state });
    assert.equal(editor.takeState("k0"), null);
    assert.ok(editor.takeState("k54"));
    editor.keepState("gone", { state });
    editor.dropState("gone");
    assert.equal(editor.takeState("gone"), null);
});

test("a pan-and-zoom view zooms at a point, fits a box and centers on a point", () => {
    const pz = moqui.codeEditor.panZoom;
    // the layer point under (100, 50) stays there
    const zoomed = pz.zoomAt({ x: 0, y: 0, k: 1 }, { x: 100, y: 50 }, 2, 0.2, 4);
    assert.deepEqual(zoomed, { x: -100, y: -50, k: 2 });
    assert.equal((100 - zoomed.x) / zoomed.k, 100);
    // clamped to the limits
    assert.equal(pz.zoomAt({ x: 0, y: 0, k: 1 }, { x: 0, y: 0 }, 10, 0.2, 2).k, 2);
    // a 1000 x 500 box in an 524 x 524 frame with 12 around it: scale 0.5, centered
    const fit = pz.fit({ x: 0, y: 0, width: 1000, height: 500 }, { width: 524, height: 524 }, 12, 0.1, 2);
    assert.deepEqual(fit, { x: 12, y: 137, k: 0.5 });
    // a small box is not zoomed in past 1
    assert.equal(pz.fit({ x: 10, y: 10, width: 10, height: 10 }, { width: 500, height: 500 }, 0, 0.1, 4).k, 1);
    assert.deepEqual(pz.center({ x: 0, y: 0, k: 2 }, { x: 50, y: 25 }, { width: 400, height: 200 }), { x: 100, y: 50, k: 2 });
});

test("the wheel pans, Ctrl zooms; with wheel zoom a plain wheel zooms at the pointer and Shift pans", () => {
    const pz = moqui.codeEditor.panZoom;
    const view = { x: 10, y: 20, k: 1 }, at = { x: 100, y: 50 };
    assert.deepEqual(pz.wheel(view, { deltaX: 5, deltaY: 30 }, at, false, 0.2, 4), { x: 5, y: -10, k: 1 });
    const pinched = pz.wheel(view, { deltaX: 0, deltaY: -300, ctrlKey: true }, at, false, 0.2, 4);
    assert.equal(pinched.k, Math.E);
    // the point under the pointer stays there
    assert.ok(Math.abs((at.x - pinched.x) / pinched.k - (at.x - view.x)) < 1e-9);
    const zoomed = pz.wheel(view, { deltaX: 0, deltaY: 300 }, at, true, 0.2, 4);
    assert.equal(zoomed.k, Math.exp(-1));
    assert.ok(Math.abs((at.y - zoomed.y) / zoomed.k - (at.y - view.y)) < 1e-9);
    assert.equal(pz.wheel(view, { deltaX: 0, deltaY: -3000 }, at, true, 0.2, 4).k, 4, "clamped");
    // Shift: a one-axis wheel pans sideways, a trackpad's two axes pan as they are
    assert.deepEqual(pz.wheel(view, { deltaX: 0, deltaY: 40, shiftKey: true }, at, true, 0.2, 4), { x: -30, y: 20, k: 1 });
    assert.deepEqual(pz.wheel(view, { deltaX: 8, deltaY: 4, shiftKey: true }, at, true, 0.2, 4), { x: 2, y: 16, k: 1 });
});

test("the pan-and-zoom layer takes wheel-zoom and controls", () => {
    const comp = moqui.codeEditor.components["m-pan-zoom"];
    ["minZoom", "maxZoom", "height", "wheelZoom", "controls"].forEach((name) => assert.ok(Object.keys(comp.props).includes(name), name));
    ["zoomBy", "zoomTo", "fit", "centerOn", "controlBar"].forEach((name) => assert.equal(typeof comp.methods[name], "function", name));
});
