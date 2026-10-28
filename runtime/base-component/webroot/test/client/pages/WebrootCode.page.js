/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The code editor's helpers in the page. editor is moqui.codeEditor from /js/WebrootCode.js.
// CodeMirror is the vendored bundle, the same IIFE the Node file ran with vm.runInThisContext.
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
        const factory = { xml: "xml", html: "html", vue: "vue", javascript: "javascript", json: "json", css: "css", markdown: "markdown",
            sql: "sql", yaml: "yaml", rust: "rust" }[language];
        assert.ok(factory && typeof CM[factory] === "function", language);
        const state = CM.EditorState.create({ doc: "x", extensions: [CM[factory]()] });
        assert.equal(state.doc.toString(), "x", language);
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
