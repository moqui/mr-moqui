/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The code editor widgets (plans/design-dev-aide.md 5.1, F9), on the vendored CodeMirror 6 bundle (libs/codemirror), which
 * loads when the first editor mounts:
 *   - m-code-editor: one file's text with its language (by `language` or `file-name`), line numbers, folding, search,
 *     completion (XML elements and attributes from a schema digest), lint (a `lint` function answering problems), the
 *     shell's dark mode, Mod-S as `save`; `ready` gives the EditorView to a host such as the dev-aide. When the bundle cannot
 *     load it is a plain text area, so a form still submits. In a form it writes a hidden input of `name`.
 *   - m-diff-view: two texts side by side (MergeView) or inline (unifiedMergeView), read only unless `editable`.
 *   - m-file-tree: a lazy tree of a directory over a `load(path)` function, each entry with its git status, the folder's
 *     rolled up; `open` for a file, `menu` for a right click.
 * The components are render functions, not templates, so they need no template compiler and run under the strict content
 * policy of a static page. Registered late (qvt.registerLate) when loaded after WebrootVue.qvt.js, which is the usual case:
 * m-text-area with editor-type="code" loads this file. The pure helpers are moqui.codeEditor, which
 * test/client/pages/WebrootCode.page.js runs in the page.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui || (root.moqui = {});
    const editor = moqui.codeEditor = moqui.codeEditor || {};

    // ---- pure helpers ----

    // File extensions to the editor's languages; anything else is plain text.
    const EXTENSIONS = {
        xml: "xml", xsd: "xml", xsl: "xml", svg: "xml",
        html: "html", htm: "html", ftl: "html", qvt: "html",
        vue: "vue", qvue: "vue",
        js: "javascript", mjs: "javascript", cjs: "javascript", qjs: "javascript", ts: "javascript",
        json: "json", css: "css", md: "markdown", markdown: "markdown", sql: "sql", yaml: "yaml", yml: "yaml", rs: "rust"
    };
    editor.extensions = EXTENSIONS;

    // The language of a file name: `.tui.json` is JSON, `Screen.qvue` is Vue, `notes` is plain text.
    editor.languageOf = (fileName) => {
        const name = String(fileName || "").split(/[\\/]/).pop().toLowerCase();
        const dot = name.lastIndexOf(".");
        return dot < 0 ? "text" : (EXTENSIONS[name.slice(dot + 1)] || "text");
    };

    // The status a folder shows from its entries' (git's letters): a conflict first, then changes, additions, deletions,
    // untracked files; "" when every entry is clean or ignored.
    const STATUS_ORDER = ["U", "M", "A", "R", "D", "?"];
    editor.rollupStatus = (statuses) => {
        let best = "";
        for (const status of statuses || []) {
            const at = STATUS_ORDER.indexOf(status);
            if (at >= 0 && (best === "" || at < STATUS_ORDER.indexOf(best))) best = status;
        }
        return best;
    };

    // The schema digest (moqui_core::devtools::schema) as lang-xml's `elements`: each element with its children and its
    // attributes, an enumerated one with its values.
    editor.xmlElements = (digest) => (digest && digest.elements ? digest.elements : []).map((element) => ({
        name: element.name,
        top: !!element.top,
        children: element.openChildren ? undefined : element.children,
        attributes: (element.attributes || []).map((attribute) => (attribute.values && attribute.values.length
            ? { name: attribute.name, values: attribute.values } : attribute.name))
    }));

    // Problems (moqui_core::devtools::Problem: 1-based line and column) as CodeMirror diagnostics over `text`: from the
    // problem's column to the end of its word, or the whole line when it has no column, clamped to the text.
    editor.diagnostics = (problems, text) => {
        const lines = String(text || "").split("\n");
        const starts = [];
        let offset = 0;
        for (const line of lines) {
            starts.push(offset);
            offset += line.length + 1;
        }
        return (problems || []).map((problem) => {
            const index = Math.min(Math.max((problem.line || 1) - 1, 0), lines.length - 1);
            const line = lines[index] || "";
            const column = problem.column ? Math.min(problem.column - 1, line.length) : 0;
            let end = problem.column ? column : line.length;
            if (problem.column) while (end < line.length && /[\w:.#-]/.test(line[end])) end++;
            if (end === column) end = Math.min(column + 1, line.length);
            return {
                from: starts[index] + column,
                to: starts[index] + Math.max(end, column),
                severity: problem.severity === "warning" || problem.severity === "info" ? problem.severity : "error",
                message: problem.message + (problem.rule ? " [" + problem.rule + "]" : ""),
                source: problem.rule || undefined
            };
        });
    };

    // The steps of an xml-op answer (moqui_lib::xml_edit: each operation's edits, in UTF-16 offsets of the text before it) as
    // the specs of one CodeMirror transaction: the first in the start document's offsets, each later one sequential, so the
    // operations are one undo step.
    editor.xmlOpSpecs = (steps) => (steps || []).filter((edits) => edits && edits.length).map((edits, index) => ({
        changes: edits.map((edit) => ({ from: edit.from16, to: edit.to16, insert: edit.text })),
        sequential: index > 0
    }));

    // Apply XML operations to an editor's buffer: `post(text, ops)` answers { text, steps } (the dev-aide's xml-op
    // transition); the view's text becomes the answer's, as one transaction, when it has not changed meanwhile.
    editor.applyXmlOps = (view, ops, post) => {
        const before = view.state.doc.toString();
        return Promise.resolve(post(before, ops)).then((answer) => {
            if (!answer || !answer.steps) throw new Error((answer && answer.error) || "no edits");
            if (view.state.doc.toString() !== before) throw new Error("the text changed while the operations ran");
            const specs = editor.xmlOpSpecs(answer.steps);
            if (specs.length) view.dispatch(...specs);
            return view.state.doc.toString();
        });
    };

    // The one change that turns `before` into `after`: the part between their common start and end.
    editor.textChange = (before, after) => {
        let start = 0;
        const shortest = Math.min(before.length, after.length);
        while (start < shortest && before.charCodeAt(start) === after.charCodeAt(start)) start++;
        let end = 0;
        while (end < shortest - start && before.charCodeAt(before.length - 1 - end) === after.charCodeAt(after.length - 1 - end)) end++;
        return { from: start, to: before.length - end, insert: after.slice(start, after.length - end) };
    };

    // The editor states kept by buffer key (m-code-editor stateKey), the most recent 50: outside Vue's reactivity, as a
    // CodeMirror state is not to be proxied.
    const keptStates = new Map();
    editor.keepState = (key, kept) => {
        keptStates.delete(key);
        keptStates.set(key, kept);
        while (keptStates.size > 50) keptStates.delete(keptStates.keys().next().value);
    };
    editor.takeState = (key) => {
        const kept = keptStates.get(key);
        keptStates.delete(key);
        return kept || null;
    };
    editor.dropState = (key) => { keptStates.delete(key); };

    // ---- the CodeMirror bundle ----

    let loading = null;
    editor.load = () => {
        if (root.CodeMirror) return Promise.resolve(root.CodeMirror);
        if (!loading) {
            loading = moqui.loadScript("/libs/codemirror/codemirror.min.js", null, () => !!root.CodeMirror)
                .then(() => root.CodeMirror)
                .catch((error) => { loading = null; throw error; });
        }
        return loading;
    };

    function languageExtension(CM, language, schema) {
        switch (language) {
            case "xml": return CM.xml(schema ? { elements: editor.xmlElements(schema) } : {});
            case "html": return CM.html();
            case "vue": return CM.vue();
            case "javascript": return CM.javascript();
            case "json": return CM.json();
            case "css": return CM.css();
            case "markdown": return CM.markdown();
            case "sql": return CM.sql();
            case "yaml": return CM.yaml();
            case "rust": return CM.rust();
            default: return [];
        }
    }

    let darkStyle = null;
    function theme(CM, dark) {
        if (dark && !darkStyle) {
            const t = CM.tags;
            darkStyle = CM.HighlightStyle.define([
                { tag: [t.keyword, t.operatorKeyword, t.modifier], color: "#c792ea" },
                { tag: [t.tagName, t.typeName, t.className], color: "#82aaff" },
                { tag: [t.attributeName, t.propertyName], color: "#ffcb6b" },
                { tag: [t.string, t.attributeValue, t.special(t.string)], color: "#c3e88d" },
                { tag: [t.number, t.bool, t.null, t.atom], color: "#f78c6c" },
                { tag: [t.comment, t.lineComment, t.blockComment], color: "#7f8c98", fontStyle: "italic" },
                { tag: [t.function(t.variableName), t.function(t.propertyName)], color: "#89ddff" },
                { tag: [t.heading], color: "#82aaff", fontWeight: "bold" },
                { tag: [t.link, t.url], color: "#89ddff", textDecoration: "underline" },
                { tag: t.invalid, color: "#ff5370" }
            ]);
        }
        return [
            CM.EditorView.theme({
                "&": { height: "100%", fontSize: "13px" },
                ".cm-scroller": { fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace", lineHeight: "1.5" },
                ".cm-gutters": dark ? { backgroundColor: "#1d1d1d", color: "#8a8a8a", border: "none" }
                    : { backgroundColor: "#f7f7f9", color: "#999", border: "none" },
                ".cm-m-highlight": { backgroundColor: dark ? "rgba(255, 213, 79, 0.12)" : "rgba(255, 213, 79, 0.3)" }
            }, { dark: !!dark }),
            CM.syntaxHighlighting(dark ? darkStyle : CM.defaultHighlightStyle, { fallback: true })
        ];
    }

    // The editing extensions every editor has, the basic setup of CodeMirror without the parts each editor decides.
    function basics(CM) {
        return [
            CM.highlightActiveLineGutter(), CM.highlightSpecialChars(), CM.history(), CM.foldGutter(), CM.drawSelection(),
            CM.dropCursor(), CM.EditorState.allowMultipleSelections.of(true), CM.indentOnInput(), CM.bracketMatching(),
            CM.closeBrackets(), CM.autocompletion(), CM.rectangularSelection(), CM.crosshairCursor(), CM.highlightActiveLine(),
            CM.highlightSelectionMatches(), CM.indentationMarkers()
        ];
    }

    function keys(CM, extra) {
        return CM.keymap.of([].concat(extra || [], CM.closeBracketsKeymap, CM.defaultKeymap, CM.searchKeymap,
            CM.historyKeymap, CM.foldKeymap, CM.completionKeymap, CM.lintKeymap, [CM.indentWithTab]));
    }

    function highlighted(CM, lines, doc) {
        const builder = new CM.RangeSetBuilder();
        const wanted = (lines || []).map(Number).filter((line) => line >= 1 && line <= doc.lines).sort((a, b) => a - b);
        for (const number of new Set(wanted)) {
            const line = doc.line(number);
            builder.add(line.from, line.from, CM.Decoration.line({ class: "cm-m-highlight" }));
        }
        return CM.EditorView.decorations.of(builder.finish());
    }

    // A blame gutter: each run of lines one commit last changed gets the commit's short id and author at its first line,
    // with its summary as the title. ranges are { from, to, commit, author, time, summary }, lines from 1.
    function blameGutter(CM, ranges) {
        if (!ranges || !ranges.length) return [];
        const starts = new Map();
        for (const range of ranges) starts.set(range.from, range);
        class BlameMarker extends CM.GutterMarker {
            constructor(range) { super(); this.range = range; }
            eq(other) { return other.range === this.range; }
            toDOM() {
                const span = root.document.createElement("span");
                const date = this.range.time ? new Date(this.range.time * 1000).toISOString().slice(0, 10) : "";
                span.textContent = String(this.range.commit || "").slice(0, 7) + " " + (this.range.author || "").split(" ")[0] + " " + date;
                span.title = this.range.summary || "";
                span.setAttribute("data-commit", this.range.commit || "");
                return span;
            }
        }
        return CM.gutter({
            class: "cm-m-blame",
            lineMarker(view, line) {
                const range = starts.get(view.state.doc.lineAt(line.from).number);
                return range ? new BlameMarker(range) : null;
            },
            initialSpacer: () => new BlameMarker({ commit: "0000000", author: "Somebody", time: 0 })
        });
    }

    const isDark = (vm) => !!(vm.$q && vm.$q.dark && vm.$q.dark.isActive);
    const h = (...args) => root.Vue.h(...args);

    let styled = false;
    function addStyles() {
        if (styled || !root.document) return;
        styled = true;
        const style = root.document.createElement("style");
        style.textContent = ".m-code-editor{position:relative;border:1px solid var(--m-border,rgba(0,0,0,.12));border-radius:4px;overflow:hidden}" +
            ".m-code-editor .cm-editor{height:100%}.m-code-editor .cm-editor.cm-focused{outline:none}" +
            ".m-code-editor .cm-m-blame{font-size:11px;opacity:.75;padding:0 6px;white-space:nowrap}" +
            ".m-code-editor-fallback{width:100%;font-family:ui-monospace,Menlo,Consolas,monospace;font-size:13px}" +
            ".m-diff-view{border:1px solid var(--m-border,rgba(0,0,0,.12));border-radius:4px;overflow:auto}" +
            ".m-file-tree .m-file-status{font-family:ui-monospace,Menlo,monospace;font-size:11px;font-weight:600;margin-left:auto;padding-left:8px}" +
            ".m-file-status-M{color:#e2a400}.m-file-status-A,.m-file-status-\\?{color:#2e9e4f}.m-file-status-D{color:#d32f2f}" +
            ".m-file-status-U{color:#d32f2f}.m-file-status-R{color:#1e88e5}.m-file-tree .m-file-ignored{opacity:.5}" +
            ".m-pan-zoom{position:relative;overflow:hidden;touch-action:none;cursor:grab;outline:none}.m-pan-zoom:active{cursor:grabbing}" +
            ".m-pan-zoom-layer{position:absolute;left:0;top:0;transform-origin:0 0}" +
            ".m-pan-zoom-layer>svg{position:absolute;left:0;top:0;overflow:visible;pointer-events:none}";
        root.document.head.appendChild(style);
    }

    // ---- m-code-editor ----

    const codeEditor = {
        name: "mCodeEditor",
        props: {
            modelValue: { type: String, default: "" }, language: String, fileName: String, readOnly: Boolean,
            lineNumbers: { type: Boolean, default: true }, wrap: Boolean, highlightLines: Array, schema: Object, lint: Function,
            name: String, form: String, minHeight: { type: String, default: "6rem" }, maxHeight: String, height: String,
            autofocus: Boolean, placeholder: String,
            // who last changed each run of lines (git blame), shown in a gutter while it is set
            blame: Array,
            // more extensions from the page (CM) => extension: a keymap, event handlers, completion sources
            extensions: Function,
            // a key of the buffer (a file tab): the editor's state, with its undo history, is kept under it when the editor
            // unmounts and taken back when one mounts again with the same key and text, so switching tabs keeps history
            stateKey: String
        },
        emits: ["update:modelValue", "save", "cursor", "ready"],
        data() { return { failed: false, ready: false, text: this.modelValue || "" }; },
        computed: {
            resolvedLanguage() { return this.language || editor.languageOf(this.fileName); }
        },
        watch: {
            modelValue(value) {
                const text = value || "";
                this.text = text;
                if (this.view && text !== this.view.state.doc.toString()) {
                    // only the part that differs, so the cursor and scroll outside it stay where they were
                    this.view.dispatch({ changes: editor.textChange(this.view.state.doc.toString(), text) });
                    // a file's text arrives after its editor: the highlighted lines are in it now
                    if (this.highlightLines && this.highlightLines.length) { this.reconfigure("highlight"); this.reveal(); }
                }
            },
            resolvedLanguage() { this.reconfigure("language"); },
            schema() { this.reconfigure("language"); },
            readOnly() { this.reconfigure("readOnly"); },
            wrap() { this.reconfigure("wrap"); },
            highlightLines() { this.reconfigure("highlight"); this.reveal(); },
            blame() { this.reconfigure("blame"); }
        },
        mounted() {
            addStyles();
            this.$watch(() => isDark(this), () => this.reconfigure("theme"));
            editor.load().then((CM) => { if (!this.unmounted) this.build(CM); }, (error) => {
                console.error("The code editor could not load; editing as plain text", error);
                this.failed = true;
            });
        },
        beforeUnmount() {
            this.unmounted = true;
            if (this.view) {
                if (this.stateKey && this.owner) editor.keepState(this.stateKey, { state: this.view.state, compartments: this.compartments, owner: this.owner });
                this.owner.vm = null;
                this.view.destroy();
            }
        },
        methods: {
            parts(CM) {
                return {
                    language: () => languageExtension(CM, this.resolvedLanguage, this.schema),
                    theme: () => theme(CM, isDark(this)),
                    readOnly: () => [CM.EditorState.readOnly.of(!!this.readOnly), CM.EditorView.editable.of(!this.readOnly)],
                    wrap: () => (this.wrap ? CM.EditorView.lineWrapping : []),
                    highlight: () => highlighted(CM, this.highlightLines, this.view ? this.view.state.doc : CM.Text.of(this.text.split("\n"))),
                    blame: () => blameGutter(CM, this.blame)
                };
            },
            build(CM) {
                // a kept state of this buffer: its extensions call the editor through its owner, which is now this one
                const kept = this.stateKey ? editor.takeState(this.stateKey) : null;
                if (kept && kept.state.doc.toString() === this.text) {
                    this.cm = CM;
                    this.compartments = kept.compartments;
                    this.owner = kept.owner;
                    this.owner.vm = this;
                    this.view = new CM.EditorView({ state: kept.state, parent: this.$refs.host });
                    ["language", "theme", "readOnly", "wrap", "highlight", "blame"].forEach((key) => this.reconfigure(key));
                    if (this.autofocus) this.view.focus();
                    this.ready = true;
                    this.reveal();
                    this.$emit("ready", this.view);
                    return;
                }
                const owner = this.owner = { vm: this };
                const parts = this.parts(CM);
                this.compartments = {};
                const configured = Object.keys(parts).map((key) => {
                    this.compartments[key] = new CM.Compartment();
                    return this.compartments[key].of(parts[key]());
                });
                const extensions = [
                    this.lineNumbers ? CM.lineNumbers() : [],
                    basics(CM),
                    keys(CM, [{ key: "Mod-s", preventDefault: true, run: (view) => { if (owner.vm) owner.vm.$emit("save", view.state.doc.toString()); return true; } }]),
                    configured,
                    this.placeholder ? CM.placeholder(this.placeholder) : [],
                    this.extensions ? this.extensions(CM) : [],
                    this.lint ? [CM.linter((view) => Promise.resolve(owner.vm && owner.vm.lint ? owner.vm.lint(view.state.doc.toString()) : [])
                        .then((problems) => editor.diagnostics(problems, view.state.doc.toString()))
                        .catch(() => []), { delay: 500 }), CM.lintGutter()] : [],
                    CM.EditorView.updateListener.of((update) => {
                        const vm = owner.vm;
                        if (!vm) return;
                        if (update.docChanged) {
                            vm.text = update.state.doc.toString();
                            vm.$emit("update:modelValue", vm.text);
                        }
                        if (update.docChanged || update.selectionSet) {
                            const pos = update.state.selection.main.head;
                            const line = update.state.doc.lineAt(pos);
                            vm.$emit("cursor", { pos, line: line.number, column: pos - line.from + 1 });
                        }
                    })
                ];
                this.cm = CM;
                this.view = new CM.EditorView({
                    state: CM.EditorState.create({ doc: this.text, extensions }),
                    parent: this.$refs.host
                });
                if (this.autofocus) this.view.focus();
                this.ready = true;
                this.reveal();
                this.$emit("ready", this.view);
            },
            // The cursor to the first highlighted line, scrolled to the middle, once the line is in the text; once for
            // each line asked for, so editing after it does not scroll back.
            reveal() {
                const view = this.view, CM = this.cm;
                const first = (this.highlightLines || []).map(Number).filter((line) => line >= 1).sort((a, b) => a - b)[0];
                if (!view || !CM || !first || first > view.state.doc.lines || this.revealed === first) return;
                this.revealed = first;
                const line = view.state.doc.line(first);
                view.dispatch({ selection: { anchor: line.from }, effects: CM.EditorView.scrollIntoView(line.from, { y: "center" }) });
            },
            reconfigure(key) {
                if (!this.view || !this.compartments || !this.compartments[key]) return;
                this.view.dispatch({ effects: this.compartments[key].reconfigure(this.parts(this.cm)[key]()) });
            },
            focus() { if (this.view) this.view.focus(); }
        },
        render() {
            if (this.failed) {
                return h("textarea", {
                    class: "m-code-editor-fallback q-pa-sm", value: this.text, readonly: this.readOnly, name: this.name,
                    form: this.form, rows: 12, spellcheck: false, "data-state": "fallback",
                    onInput: (event) => { this.text = event.target.value; this.$emit("update:modelValue", this.text); }
                });
            }
            return h("div", { class: "m-code-editor", "data-state": this.ready ? "ready" : "loading" }, [
                h("div", { ref: "host", class: "m-code-editor-host", style: {
                    minHeight: this.height ? undefined : this.minHeight, maxHeight: this.maxHeight, height: this.height,
                    overflow: "auto" } }),
                this.name ? h("input", { type: "hidden", name: this.name, form: this.form, value: this.text }) : null
            ]);
        }
    };

    // ---- m-diff-view ----

    const diffView = {
        name: "mDiffView",
        props: {
            original: { type: String, default: "" }, modified: { type: String, default: "" }, language: String,
            fileName: String, unified: Boolean, editable: Boolean, collapseUnchanged: { type: Boolean, default: true },
            height: String,
            // unified and editable: Accept and Reject on each chunk (a reviewed proposal; plans/design-dev-aide.md 13.3)
            mergeControls: Boolean
        },
        emits: ["update:modified", "ready"],
        data() { return { failed: false }; },
        watch: {
            original() { this.rebuild(); },
            modified(value) {
                const view = this.editorOfModified();
                if (view && value !== view.state.doc.toString()) this.rebuild();
            },
            unified() { this.rebuild(); },
            language() { this.rebuild(); },
            fileName() { this.rebuild(); }
        },
        mounted() {
            addStyles();
            this.$watch(() => isDark(this), () => this.rebuild());
            editor.load().then((CM) => { this.cm = CM; if (!this.unmounted) this.rebuild(); }, (error) => {
                console.error("The diff view could not load", error);
                this.failed = true;
            });
        },
        beforeUnmount() { this.unmounted = true; this.destroy(); },
        methods: {
            destroy() {
                if (this.merge) this.merge.destroy();
                if (this.view) this.view.destroy();
                this.merge = null;
                this.view = null;
            },
            editorOfModified() { return this.view || (this.merge && this.merge.b) || null; },
            rebuild() {
                const CM = this.cm;
                if (!CM || !this.$refs.host) return;
                this.destroy();
                const language = languageExtension(CM, this.language || editor.languageOf(this.fileName));
                const look = [CM.lineNumbers(), CM.highlightSpecialChars(), CM.drawSelection(), language, theme(CM, isDark(this))];
                const readOnly = [CM.EditorState.readOnly.of(true), CM.EditorView.editable.of(false)];
                const changes = CM.EditorView.updateListener.of((update) => {
                    if (update.docChanged) this.$emit("update:modified", update.state.doc.toString());
                });
                const modifiedExtensions = [look, this.editable ? [CM.history(), keys(CM), changes] : readOnly];
                if (this.unified) {
                    this.view = new CM.EditorView({
                        parent: this.$refs.host,
                        state: CM.EditorState.create({ doc: this.modified, extensions: [modifiedExtensions,
                            CM.unifiedMergeView({ original: this.original, mergeControls: !!(this.mergeControls && this.editable), gutter: true,
                                collapseUnchanged: this.collapseUnchanged ? {} : undefined })] })
                    });
                } else {
                    this.merge = new CM.MergeView({
                        parent: this.$refs.host,
                        a: { doc: this.original, extensions: [look, readOnly] },
                        b: { doc: this.modified, extensions: modifiedExtensions },
                        collapseUnchanged: this.collapseUnchanged ? {} : undefined,
                        gutter: true
                    });
                }
                this.$emit("ready", this.merge || this.view);
            }
        },
        render() {
            if (this.failed) {
                return h("div", { class: "m-diff-view row no-wrap", "data-state": "fallback" }, [
                    h("pre", { class: "col q-ma-none q-pa-sm" }, this.original),
                    h("pre", { class: "col q-ma-none q-pa-sm" }, this.modified)
                ]);
            }
            return h("div", { class: "m-diff-view", style: { height: this.height } }, [h("div", { ref: "host" })]);
        }
    };

    // ---- m-file-tree ----

    // An entry from `load` ({ name, dir, status, ignored }) as a node of the tree, keyed by its path.
    editor.treeNode = (parent, entry) => {
        const path = parent ? parent + "/" + entry.name : entry.name;
        return {
            path, name: entry.name, dir: !!entry.dir, status: entry.status || "", ignored: !!entry.ignored,
            lazy: !!entry.dir, children: entry.dir ? undefined : []
        };
    };

    const fileTree = {
        name: "mFileTree",
        props: {
            load: { type: Function, required: true }, filter: String, selected: String, showIgnored: { type: Boolean, default: true },
            reloadKey: [String, Number]
        },
        emits: ["open", "menu", "update:selected"],
        data() { return { nodes: [], expanded: [], failed: "" }; },
        watch: { reloadKey() { this.refresh(); } },
        mounted() { addStyles(); this.refresh(); },
        methods: {
            refresh() {
                Promise.resolve(this.load("")).then((entries) => {
                    this.failed = "";
                    this.nodes = (entries || []).map((entry) => editor.treeNode("", entry));
                    this.expanded = [];
                }, (error) => { this.failed = String((error && error.message) || error); });
            },
            onLazyLoad({ node, done, fail }) {
                Promise.resolve(this.load(node.path)).then((entries) =>
                    done((entries || []).filter((entry) => this.showIgnored || !entry.ignored)
                        .map((entry) => editor.treeNode(node.path, entry))), fail);
            },
            onSelected(key) {
                if (!key) return;
                this.$emit("update:selected", key);
                const node = this.$refs.tree && this.$refs.tree.getNodeByKey(key);
                if (node && !node.dir) this.$emit("open", node.path);
                else if (node) this.$refs.tree.setExpanded(key, !this.$refs.tree.isExpanded(key));
            },
            header(scope) {
                const node = scope.node;
                const Q = root.Quasar;
                return h("div", {
                    class: ["row items-center no-wrap full-width", node.ignored ? "m-file-ignored" : ""],
                    "data-path": node.path,
                    onContextmenu: (event) => { event.preventDefault(); this.$emit("menu", { path: node.path, dir: node.dir, event }); }
                }, [
                    h(Q.QIcon, { name: node.dir ? "folder" : "description", size: "18px", class: "q-mr-xs text-grey-7" }),
                    h("span", { class: "ellipsis" }, node.name),
                    node.status ? h("span", { class: "m-file-status m-file-status-" + node.status }, node.status) : null
                ]);
            }
        },
        render() {
            const Q = root.Quasar;
            if (this.failed) return h("div", { class: "m-file-tree q-pa-sm text-negative" }, this.failed);
            return h(Q.QTree, {
                ref: "tree", class: "m-file-tree", nodes: this.nodes, nodeKey: "path", labelKey: "name", dense: true,
                noTransition: true, filter: this.filter,
                filterMethod: (node, filter) => node.name.toLowerCase().includes(String(filter).toLowerCase()),
                selected: this.selected, "onUpdate:selected": this.onSelected,
                expanded: this.expanded, "onUpdate:expanded": (keys) => { this.expanded = keys; },
                onLazyLoad: this.onLazyLoad
            }, { "default-header": (scope) => this.header(scope) });
        }
    };

    // ---- m-pan-zoom ----

    // The view of a pan-and-zoom layer, { x, y, k }: the layer's offset in the frame and its scale. Pure, so the arithmetic
    // is tested without a page.
    const panZoom = editor.panZoom = {
        clamp: (value, min, max) => Math.min(max, Math.max(min, value)),
        // a zoom by `factor` that keeps the layer point under `at` (frame coordinates) where it is
        zoomAt(view, at, factor, min, max) {
            const k = panZoom.clamp(view.k * factor, min, max);
            const ratio = k / view.k;
            return { x: at.x - (at.x - view.x) * ratio, y: at.y - (at.y - view.y) * ratio, k };
        },
        // the view that shows `box` ({ x, y, width, height } in layer coordinates) whole in a frame of `size`, centered,
        // with `pad` around it, never zoomed in past 1
        fit(box, size, pad, min, max) {
            const width = Math.max(1, box.width), height = Math.max(1, box.height);
            const k = panZoom.clamp(Math.min((size.width - 2 * pad) / width, (size.height - 2 * pad) / height, 1), min, max);
            return { x: (size.width - width * k) / 2 - box.x * k, y: (size.height - height * k) / 2 - box.y * k, k };
        },
        // the view that puts the layer point `at` in the middle of a frame of `size`, keeping the scale
        center(view, at, size) { return { x: size.width / 2 - at.x * view.k, y: size.height / 2 - at.y * view.k, k: view.k }; }
    };

    // A pan-and-zoom layer (plans/design-dev-aide.md D9): one CSS transform on the layer; drag the background, the wheel
    // or a trackpad to pan, Ctrl+wheel or a pinch to zoom at the pointer, arrow keys and + and - for both, 0 to fit. The
    // default slot is the layer's content, absolutely placed; the `under` slot is drawn beneath it (one SVG of
    // connectors). A drag that starts on a button, a link, an input or [data-no-pan] is left to it.
    const panZoomLayer = {
        name: "mPanZoom",
        props: { minZoom: { type: Number, default: 0.2 }, maxZoom: { type: Number, default: 2 }, height: { type: String, default: "100%" } },
        emits: ["view"],
        data() { return { view: { x: 0, y: 0, k: 1 }, pointers: {}, drag: null }; },
        mounted() { addStyles(); },
        computed: {
            transform() { return "translate(" + this.view.x + "px," + this.view.y + "px) scale(" + this.view.k + ")"; }
        },
        methods: {
            size() { const el = this.$el; return { width: el ? el.clientWidth : 0, height: el ? el.clientHeight : 0 }; },
            setView(view) { this.view = view; this.$emit("view", view); },
            local(event) { const rect = this.$el.getBoundingClientRect(); return { x: event.clientX - rect.left, y: event.clientY - rect.top }; },
            zoomBy(factor, at) { this.setView(panZoom.zoomAt(this.view, at || { x: this.size().width / 2, y: this.size().height / 2 }, factor, this.minZoom, this.maxZoom)); },
            panBy(dx, dy) { this.setView({ x: this.view.x + dx, y: this.view.y + dy, k: this.view.k }); },
            // the layer's content whole in the frame; `box` defaults to the content's own extent
            fit(box) {
                const content = box || this.extent();
                if (content) this.setView(panZoom.fit(content, this.size(), 24, this.minZoom, this.maxZoom));
            },
            centerOn(point) { this.setView(panZoom.center(this.view, point, this.size())); },
            extent() {
                const layer = this.$refs.layer;
                if (!layer) return null;
                let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
                for (const child of layer.children) {
                    if (child.tagName === "svg") continue;
                    minX = Math.min(minX, child.offsetLeft); minY = Math.min(minY, child.offsetTop);
                    maxX = Math.max(maxX, child.offsetLeft + child.offsetWidth); maxY = Math.max(maxY, child.offsetTop + child.offsetHeight);
                }
                return minX === Infinity ? null : { x: minX, y: minY, width: maxX - minX, height: maxY - minY };
            },
            onWheel(event) {
                event.preventDefault();
                if (event.ctrlKey || event.metaKey) this.zoomBy(Math.exp(-event.deltaY / 300), this.local(event));
                else this.panBy(-event.deltaX, -event.deltaY);
            },
            onDown(event) {
                if (event.target.closest && event.target.closest("button, a, input, textarea, select, [data-no-pan]")) return;
                this.pointers[event.pointerId] = this.local(event);
                const ids = Object.keys(this.pointers);
                if (ids.length === 1) this.drag = { start: this.local(event), view: this.view };
                else if (ids.length === 2) {
                    const [a, b] = ids.map((id) => this.pointers[id]);
                    this.drag = { pinch: Math.hypot(a.x - b.x, a.y - b.y), view: this.view, at: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 } };
                }
                if (this.$el.setPointerCapture) this.$el.setPointerCapture(event.pointerId);
            },
            onMove(event) {
                if (!this.drag || !this.pointers[event.pointerId]) return;
                this.pointers[event.pointerId] = this.local(event);
                const ids = Object.keys(this.pointers);
                if (this.drag.pinch && ids.length === 2) {
                    const [a, b] = ids.map((id) => this.pointers[id]);
                    const distance = Math.hypot(a.x - b.x, a.y - b.y);
                    this.setView(panZoom.zoomAt(this.drag.view, this.drag.at, distance / this.drag.pinch, this.minZoom, this.maxZoom));
                } else if (this.drag.start) {
                    const at = this.local(event);
                    this.setView({ x: this.drag.view.x + at.x - this.drag.start.x, y: this.drag.view.y + at.y - this.drag.start.y, k: this.drag.view.k });
                }
            },
            onUp(event) {
                delete this.pointers[event.pointerId];
                if (!Object.keys(this.pointers).length) this.drag = null;
            },
            onKey(event) {
                if (event.target !== this.$el) return;
                const step = 60;
                const keys = { ArrowLeft: () => this.panBy(step, 0), ArrowRight: () => this.panBy(-step, 0), ArrowUp: () => this.panBy(0, step),
                    ArrowDown: () => this.panBy(0, -step), "+": () => this.zoomBy(1.2), "=": () => this.zoomBy(1.2), "-": () => this.zoomBy(1 / 1.2),
                    "0": () => this.fit() };
                if (keys[event.key]) { event.preventDefault(); keys[event.key](); }
            }
        },
        render() {
            return h("div", {
                class: "m-pan-zoom", tabindex: 0, style: { height: this.height }, onWheel: this.onWheel, onPointerdown: this.onDown,
                onPointermove: this.onMove, onPointerup: this.onUp, onPointercancel: this.onUp, onKeydown: this.onKey
            }, [h("div", { ref: "layer", class: "m-pan-zoom-layer", style: { transform: this.transform } },
                [this.$slots.under ? this.$slots.under() : null].concat(this.$slots.default ? this.$slots.default() : []))]);
        }
    };

    const components = { "m-code-editor": codeEditor, "m-diff-view": diffView, "m-file-tree": fileTree, "m-pan-zoom": panZoomLayer };
    editor.components = components;

    // Before WebrootVue.qvt.js its registry takes them; after it they are registered late on the running application.
    const qvt = moqui.qvt;
    if (qvt && qvt.registerLate) Object.keys(components).forEach((name) => qvt.registerLate(name, components[name]));
    else {
        const registry = moqui.qvtComponents = moqui.qvtComponents || {};
        Object.keys(components).forEach((name) => { registry[name] = components[name]; });
    }

    if (typeof module !== "undefined" && module.exports) module.exports = editor;
})(typeof window !== "undefined" ? window : globalThis);
