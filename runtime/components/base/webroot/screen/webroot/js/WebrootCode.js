/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The code editor widgets (plans/archive/design-dev-aide.md 5.1, F9), on the vendored CodeMirror 6 bundle (libs/codemirror), which
 * loads when the first editor mounts:
 *   - m-code-editor: one file's text with its language (by `language` or `file-name`), line numbers, folding, search,
 *     completion (XML elements and attributes from a schema digest; for `sql:<kind>` the dialect, and tables and columns
 *     from a schema map), lint (a `lint` function answering problems), the
 *     shell's dark mode, Mod-S as `save`; `ready` gives the EditorView to a host such as the dev-aide. The user's editor
 *     settings (`moqui.codeEditor.setSettings`: font size, tab size, wrap, white space shown, the keymap (CodeMirror's,
 *     VS Code's or Vim, whose library loads when picked) and the keys a user rebound) apply to every editor on the page.
 *     A language of the second bundle (libs/codemirror/legacy-modes.min.js: shell, Groovy, properties, TOML,
 *     Dockerfile, nginx) loads the first time a file needs it. With `collab` the text is a shared Y.Text
 *     (y-codemirror.next, plans/archive/design-dev-aide.md F15), with the others' carets and selections by name and Yjs' undo.
 *     When the bundle cannot load it is a plain text area, so a form still submits. In a form it writes a hidden input
 *     of `name`.
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

    // The words of a failed load: a request's answer (moqui.request rejects with {status, statusText, json}) by its
    // errors, else its status, else an error's message; never "[object Object]" (plans/ui-review-dev-aide-workspace.md
    // DEVW-UI-003).
    editor.failureText = (error) => {
        const json = error && (error.json || (error.errors ? error : null));
        const errors = json && (json.errors || json.errorMessage || json.message);
        if (Array.isArray(errors) && errors.length) return errors.join("; ");
        if (typeof errors === "string" && errors) return errors;
        if (error && error.status) return "HTTP " + error.status + (error.statusText ? " " + error.statusText : "");
        if (error && typeof error.message === "string" && error.message) return error.message;
        return typeof error === "string" && error ? error : "The folder could not be read";
    };

    // File extensions to the editor's languages; anything else is plain text. shell, groovy, properties, toml, dockerfile
    // and nginx are the second bundle's (LEGACY_LANGUAGES).
    const EXTENSIONS = {
        xml: "xml", xsd: "xml", xsl: "xml", svg: "xml",
        html: "html", htm: "html", ftl: "html", qvt: "html",
        vue: "vue", qvue: "vue",
        js: "javascript", mjs: "javascript", cjs: "javascript", qjs: "javascript", ts: "javascript",
        json: "json", css: "css", md: "markdown", markdown: "markdown", sql: "sql", yaml: "yaml", yml: "yaml", rs: "rust",
        sh: "shell", bash: "shell", zsh: "shell", ksh: "shell", groovy: "groovy", gradle: "groovy", gvy: "groovy",
        properties: "properties", toml: "toml", dockerfile: "dockerfile", containerfile: "dockerfile", nginx: "nginx"
    };
    editor.extensions = EXTENSIONS;
    // Whole file names that say the language, where the extension does not.
    const FILE_NAMES = {
        dockerfile: "dockerfile", containerfile: "dockerfile", "nginx.conf": "nginx", ".bashrc": "shell", ".bash_profile": "shell",
        ".profile": "shell", ".zshrc": "shell", "gradlew": "shell", "jenkinsfile": "groovy"
    };

    // The language of a file name: `.tui.json` is JSON, `Screen.qvue` is Vue, `Dockerfile` and `Dockerfile.dev` are
    // Dockerfile, `notes` is plain text.
    editor.languageOf = (fileName) => {
        const name = String(fileName || "").split(/[\\/]/).pop().toLowerCase();
        if (FILE_NAMES[name]) return FILE_NAMES[name];
        if (/^(docker|container)file\./.test(name)) return "dockerfile";
        const dot = name.lastIndexOf(".");
        return dot < 0 ? "text" : (EXTENSIONS[name.slice(dot + 1)] || "text");
    };
    // The languages of the second bundle (libs/codemirror/legacy-modes.min.js, @codemirror/legacy-modes), each by the
    // name of the mode it exports; the bundle loads the first time a file of one is shown.
    editor.LEGACY_LANGUAGES = Object.freeze({ shell: "shell", groovy: "groovy", properties: "properties", toml: "toml",
        dockerfile: "dockerFile", nginx: "nginx" });

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

    // ---- editor settings (plans/archive/design-dev-aide.md 5.1) ----

    // What a user sets (the dev-aide's DEV_AIDE_EDITOR preference, given to setSettings by the page that holds it, which
    // takes them back as it goes): the font size in pixels, the tab size and the indent unit (that many spaces; null
    // keeps CodeMirror's, a tab of 4 and an indent of 2), wrap (for an editor whose `wrap` is not set), white space
    // shown, the keymap (a base of KEYMAPS by name; "default" is CodeMirror's) and keys (a command's keys by its name,
    // over the keymap). With none set an editor is as CodeMirror makes it.
    editor.DEFAULT_SETTINGS = Object.freeze({ fontSize: 13, tabSize: null, wrap: false, whitespace: false, keymap: "default", keys: Object.freeze({}) });
    // The keymaps a user picks from (the dev-aide's editor.rs KEYMAPS): each a function of the bundle and the commands
    // the user rebound (their functions, a Set) answering the extension it adds ahead of the editor's own keys. "vscode"
    // is VS Code's bindings made from CodeMirror's commands (@replit/codemirror-vscode-keymap carries no license text, so
    // it is not vendored); "vim" is @replit/codemirror-vim, its library (KEYMAP_LIBRARIES) loaded when it is picked.
    editor.KEYMAPS = editor.KEYMAPS || { default: () => [] };
    editor.KEYMAPS.vscode = (CM, rebound) => editor.vscodeKeymap(CM, rebound);
    editor.KEYMAPS.vim = () => { if (!root.CodeMirrorVim) return []; editor.vimHistory(); return root.CodeMirrorVim.vim(); };
    // A keymap's library, loaded the first time it is picked, after which the editors on the page take it.
    editor.KEYMAP_LIBRARIES = { vim: { url: "/libs/codemirror/vim.min.js", ready: () => !!root.CodeMirrorVim } };
    // The views whose text is a shared Y.Text, with their collab ({undoManager}), for Vim's undo and redo.
    const sharedViews = new WeakMap();
    editor.sharedOf = (view) => sharedViews.get(view) || null;
    // Vim's u, Ctrl-R, :undo and :redo run its CodeMirror.commands undo and redo, which use CodeMirror's history: in a
    // shared text's editor they are Yjs' (this user's own edits), elsewhere as Vim has them. Once a page.
    editor.vimHistory = () => {
        const vim = root.CodeMirrorVim;
        if (!vim || !vim.CodeMirror || vim.historyShared) return;
        vim.historyShared = true;
        const commands = vim.CodeMirror.commands;
        for (const name of ["undo", "redo"]) {
            const own = commands[name];
            commands[name] = (cm) => {
                const shared = cm && sharedViews.get(cm.cm6);
                if (shared && shared.undoManager) shared.undoManager[name]();
                else own(cm);
            };
        }
        // the ex commands took the functions when Vim started: defined again over the ones above
        vim.Vim.defineEx("undo", "u", (cm) => commands.undo(cm));
        vim.Vim.defineEx("redo", "red", (cm) => commands.redo(cm));
    };
    // The commands a key row rebinds (DEV_AIDE_EDITOR keys), each the bundle's function its default bindings run, which
    // finds them; save is the editor's own (Mod-S emits `save`).
    editor.COMMANDS = {
        save: null, toggleComment: (CM) => CM.toggleComment, find: (CM) => CM.openSearchPanel, completion: (CM) => CM.startCompletion,
        undo: (CM) => CM.undo, redo: (CM) => CM.redo, foldAll: (CM) => CM.foldAll, unfoldAll: (CM) => CM.unfoldAll
    };
    const KEY_PART = /^(mod|ctrl|control|alt|shift|meta|cmd)$/i;
    // A key in CodeMirror's form ("Mod-Shift-f", "Ctrl-Space", "F2"): modifiers, then one key.
    editor.validKeys = (keys) => {
        const parts = String(keys || "").split(/-(?!$)/);
        const key = parts[parts.length - 1];
        return key !== "" && !/\s/.test(key) && (key === "-" || !key.endsWith("-")) && parts.slice(0, -1).every((part) => KEY_PART.test(part));
    };
    // The settings with the defaults filled in, a font size of 8 to 32 and a tab size of 1 to 8, and only the keys that
    // are valid.
    editor.normalizeSettings = (value) => {
        const given = value && typeof value === "object" ? value : {};
        const number = (raw, low, high, fallback) => { const n = Math.round(Number(raw)); return raw !== null && raw !== "" && Number.isFinite(n) && n >= low && n <= high ? n : fallback; };
        const keys = {};
        for (const [name, keyText] of Object.entries(given.keys && typeof given.keys === "object" ? given.keys : {}))
            if (Object.prototype.hasOwnProperty.call(editor.COMMANDS, name) && typeof keyText === "string" && editor.validKeys(keyText)) keys[name] = keyText;
        const base = editor.DEFAULT_SETTINGS;
        return {
            fontSize: number(given.fontSize, 8, 32, base.fontSize), tabSize: number(given.tabSize, 1, 8, null),
            wrap: given.wrap === true || given.wrap === "true", whitespace: given.whitespace === true || given.whitespace === "true",
            keymap: typeof given.keymap === "string" && given.keymap ? given.keymap : base.keymap, keys
        };
    };
    // The bindings with `keys` over them (each a command's keys by name): a command rebound loses its default bindings,
    // found by the function they run (`commands`, name to function), and runs on its keys instead, ahead of the rest.
    editor.rebind = (bindings, keys, commands) => {
        const ahead = [];
        const dropped = new Set();
        for (const [name, keyText] of Object.entries(keys || {})) {
            const run = commands && commands[name];
            if (typeof run !== "function") continue;
            dropped.add(run);
            ahead.push({ key: keyText, run, preventDefault: true });
        }
        return ahead.concat((bindings || []).filter((binding) => !dropped.has(binding.run)));
    };
    // VS Code's bindings where they differ from CodeMirror's (which stay under them), from CodeMirror's commands: Mod-L
    // selects the line, Mod-Shift-Enter opens a line above, Mod-Alt-Up and Down add a cursor, the Mod-K chords fold,
    // unfold and comment, Mod-G goes to a line, Mod-H replaces, F3 finds again, Mod-I completes, Shift-Alt-Right widens
    // the selection, and Alt-click adds a cursor. A command the user rebound (rebound, its function) keeps only their keys.
    editor.vscodeKeymap = (CM, rebound) => {
        const lineAbove = (view) => {
            if (view.state.readOnly) return false;
            const changes = view.state.changeByRange((range) => {
                const line = view.state.doc.lineAt(range.head);
                const indent = /^\s*/.exec(line.text)[0];
                return { changes: { from: line.from, insert: indent + "\n" }, range: CM.EditorSelection.cursor(line.from + indent.length) };
            });
            view.dispatch(changes, { scrollIntoView: true, userEvent: "input" });
            return true;
        };
        // a cursor on the line above (-1) or below (1) each cursor, at its column
        const addCursor = (direction) => (view) => {
            const state = view.state;
            const ranges = state.selection.ranges.slice();
            for (const range of state.selection.ranges) {
                const line = state.doc.lineAt(range.head);
                const number = line.number + direction;
                if (number < 1 || number > state.doc.lines) continue;
                const target = state.doc.line(number);
                ranges.push(CM.EditorSelection.cursor(target.from + Math.min(range.head - line.from, target.length)));
            }
            if (ranges.length === state.selection.ranges.length) return false;
            view.dispatch({ selection: CM.EditorSelection.create(ranges, ranges.length - 1), scrollIntoView: true });
            return true;
        };
        const bindings = [
            { key: "Mod-l", run: CM.selectLine, preventDefault: true },
            { key: "Mod-Shift-Enter", run: lineAbove, preventDefault: true },
            { key: "Mod-Alt-ArrowUp", run: addCursor(-1), preventDefault: true },
            { key: "Mod-Alt-ArrowDown", run: addCursor(1), preventDefault: true },
            { key: "Mod-Shift-k", run: CM.deleteLine, preventDefault: true },
            { key: "Mod-d", run: CM.selectNextOccurrence, preventDefault: true },
            { key: "Mod-Shift-l", run: CM.selectSelectionMatches, preventDefault: true },
            { key: "Mod-k Mod-c", run: CM.lineComment, preventDefault: true },
            { key: "Mod-k Mod-u", run: CM.lineUncomment, preventDefault: true },
            { key: "Mod-k Mod-0", run: CM.foldAll, preventDefault: true },
            { key: "Mod-k Mod-j", run: CM.unfoldAll, preventDefault: true },
            { key: "Mod-Shift-[", run: CM.foldCode, preventDefault: true },
            { key: "Mod-Shift-]", run: CM.unfoldCode, preventDefault: true },
            // on a Mac Cmd-G is find next, as in VS Code: go to a line is Ctrl-G there
            { key: "Mod-g", mac: "Ctrl-g", run: CM.gotoLine, preventDefault: true },
            { key: "Mod-h", run: CM.openSearchPanel, preventDefault: true },
            { key: "F3", run: CM.findNext, shift: CM.findPrevious, preventDefault: true },
            { key: "F8", run: CM.nextDiagnostic, shift: CM.previousDiagnostic, preventDefault: true },
            { key: "Mod-i", run: CM.startCompletion, preventDefault: true },
            { key: "Shift-Alt-ArrowRight", run: CM.selectParentSyntax, preventDefault: true },
            { key: "Mod-Shift-\\", run: CM.cursorMatchingBracket, preventDefault: true }
        ].filter((binding) => typeof binding.run === "function" && !(rebound && rebound.has(binding.run)));
        return [CM.EditorView.clickAddsSelectionRange.of((event) => event.altKey), CM.keymap.of(bindings)];
    };
    // The library of keymap `name` (KEYMAP_LIBRARIES), loading it the first time: resolved when it is there, at once for
    // a keymap that needs none.
    editor.loadKeymap = (name) => {
        const library = editor.KEYMAP_LIBRARIES[name];
        if (!library || library.ready()) return Promise.resolve();
        if (!library.loading) {
            library.loading = moqui.loadScript(library.url, null, library.ready)
                .catch((error) => { library.loading = null; throw error; });
        }
        return library.loading;
    };
    // settings a page gave before this file loaded (the dev-aide's Workspace sets them as it starts) are kept
    editor.settings = editor.normalizeSettings(editor.settings || null);
    // The editors on the page, which a change of the settings reconfigures.
    const liveEditors = new Set();
    // the keymap's library, then its keys in every editor; one that does not load leaves CodeMirror's keys
    const takeKeymap = (name) => {
        const library = editor.KEYMAP_LIBRARIES[name];
        if (!library || library.ready()) return;
        editor.loadKeymap(name).then(() => { for (const vm of liveEditors) vm.reconfigure("keys"); },
            (error) => console.error("The " + name + " keymap could not load; the editor keeps CodeMirror's keys", error));
    };
    editor.setSettings = (value) => {
        editor.settings = editor.normalizeSettings(value);
        for (const vm of liveEditors) vm.applySettings();
        takeKeymap(editor.settings.keymap);
        return editor.settings;
    };

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

    // The second bundle of languages (LEGACY_LANGUAGES), loaded once; each language made once a page.
    let legacyLoading = null;
    editor.loadLegacy = () => {
        if (root.CodeMirrorLegacy) return Promise.resolve(root.CodeMirrorLegacy);
        if (!legacyLoading) {
            legacyLoading = moqui.loadScript("/libs/codemirror/legacy-modes.min.js", null, () => !!root.CodeMirrorLegacy)
                .then(() => root.CodeMirrorLegacy)
                .catch((error) => { legacyLoading = null; throw error; });
        }
        return legacyLoading;
    };
    const legacyMade = new Map();
    // The language of a legacy mode, or null while its bundle has not loaded.
    function legacyLanguage(CM, language) {
        const modes = root.CodeMirrorLegacy;
        const mode = modes && modes[editor.LEGACY_LANGUAGES[language]];
        if (!mode) return null;
        if (!legacyMade.has(language)) legacyMade.set(language, CM.StreamLanguage.define(mode));
        return legacyMade.get(language);
    }
    editor.legacyLanguage = legacyLanguage;

    // What a co-edited editor needs (plans/archive/design-dev-aide.md F15): the CodeMirror bundle, Yjs (libs/yjs, the global
    // Yjs) and y-codemirror.next built against both (the global YCodeMirror), in that order; resolved with
    // { CM, Yjs, YCodeMirror }.
    let collabLoading = null;
    editor.loadCollab = () => {
        if (!collabLoading) {
            // a library the page has already is not loaded again: a second Yjs breaks the first's constructor checks
            const script = (src, ready) => (ready() ? Promise.resolve() : moqui.loadScript(src, null, ready));
            collabLoading = editor.load()
                .then(() => script("/libs/yjs/yjs.min.js", () => !!root.Yjs))
                .then(() => script("/libs/codemirror/y-codemirror.min.js", () => !!root.YCodeMirror))
                .then(() => ({ CM: root.CodeMirror, Yjs: root.Yjs, YCodeMirror: root.YCodeMirror }))
                .catch((error) => { collabLoading = null; throw error; });
        }
        return collabLoading;
    };

    // The lang-sql dialect of a database's kind (`get#Capabilities` backend, or the database conf): Postgres, MySQL or
    // MariaDB, else standard SQL.
    const SQL_DIALECTS = {
        postgres: "PostgreSQL", postgresql: "PostgreSQL", "embedded-postgres": "PostgreSQL",
        mysql: "MySQL", mysql8: "MySQL", mariadb: "MariaSQL"
    };
    editor.sqlDialectName = (kind) => SQL_DIALECTS[String(kind || "").toLowerCase()] || "StandardSQL";
    // The lang-sql config of `sql` or `sql:<kind>`: the dialect, and `schema` ({table: [column, ...]}, or {schema:
    // {table: [...]}}) for completion of table and column names.
    editor.sqlConfig = (CM, language, schema) => {
        const kind = String(language || "").split(":")[1];
        const config = { dialect: CM[editor.sqlDialectName(kind)] };
        if (schema && typeof schema === "object") config.schema = schema;
        return config;
    };

    function languageExtension(CM, language, schema) {
        if (String(language || "").startsWith("sql:")) return CM.sql(editor.sqlConfig(CM, language, schema));
        if (editor.LEGACY_LANGUAGES[language]) return legacyLanguage(CM, language) || [];
        switch (language) {
            case "xml": return CM.xml(schema ? { elements: editor.xmlElements(schema) } : {});
            case "html": return CM.html();
            case "vue": return CM.vue();
            case "javascript": return CM.javascript();
            case "json": return CM.json();
            case "css": return CM.css();
            case "markdown": return CM.markdown();
            case "sql": return CM.sql(editor.sqlConfig(CM, language, schema));
            case "yaml": return CM.yaml();
            case "rust": return CM.rust();
            default: return [];
        }
    }
    editor.languageExtension = languageExtension;

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
                "&": { height: "100%" },
                ".cm-scroller": { fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace", lineHeight: "1.5" },
                ".cm-gutters": dark ? { backgroundColor: "#1d1d1d", color: "#8a8a8a", border: "none" }
                    : { backgroundColor: "#f7f7f9", color: "#999", border: "none" },
                ".cm-m-highlight": { backgroundColor: dark ? "rgba(255, 213, 79, 0.12)" : "rgba(255, 213, 79, 0.3)" },
                // dark: the current line a faint grey-violet tint, between the background and the selection's teal (#233), not
                // CodeMirror's bright cyan wash (#99eeff33), which hid that line's selection and dimmed its text; translucent,
                // as the selection layer is drawn under the lines
                ...(dark ? { ".cm-activeLine": { backgroundColor: "rgba(170, 170, 210, 0.07)" }, ".cm-activeLineGutter": { backgroundColor: "#28282f" } } : {})
            }, { dark: !!dark }),
            CM.syntaxHighlighting(dark ? darkStyle : CM.defaultHighlightStyle, { fallback: true })
        ];
    }

    // The editing extensions every editor has, the basic setup of CodeMirror without the parts each editor decides (the
    // history is CodeMirror's, or Yjs' in a co-edited editor).
    function basics(CM) {
        return [
            CM.highlightActiveLineGutter(), CM.highlightSpecialChars(), CM.foldGutter(), CM.drawSelection(),
            CM.dropCursor(), CM.EditorState.allowMultipleSelections.of(true), CM.indentOnInput(), CM.bracketMatching(),
            CM.closeBrackets(), CM.autocompletion(), CM.rectangularSelection(), CM.crosshairCursor(), CM.highlightActiveLine(),
            CM.highlightSelectionMatches(), CM.indentationMarkers()
        ];
    }

    // The editor's keys: the chosen keymap's extension first, then save and CodeMirror's keymaps with the keys the user
    // rebound over them; a co-edited editor's undo and redo are Yjs' (YCodeMirror's yUndoManagerKeymap).
    function keys(CM, save, settings, collab) {
        const commands = {};
        for (const [name, of] of Object.entries(editor.COMMANDS)) if (of) commands[name] = of(CM);
        const shared = collab && root.YCodeMirror;
        // a rebound undo or redo of a shared text runs Yjs' (yUndoManagerKeymap: undo, then redo)
        if (shared) { commands.undo = root.YCodeMirror.yUndoManagerKeymap[0].run; commands.redo = root.YCodeMirror.yUndoManagerKeymap[1].run; }
        const base = [].concat(CM.closeBracketsKeymap, CM.defaultKeymap, CM.searchKeymap, shared ? root.YCodeMirror.yUndoManagerKeymap : CM.historyKeymap,
            CM.foldKeymap, CM.completionKeymap, CM.lintKeymap, [CM.indentWithTab]);
        const keymap = editor.KEYMAPS[settings.keymap] || editor.KEYMAPS.default;
        const rebound = new Set(Object.keys(settings.keys).map((name) => commands[name]).filter(Boolean));
        return [keymap(CM, rebound), CM.keymap.of([{ key: settings.keys.save || "Mod-s", preventDefault: true, run: save }]
            .concat(editor.rebind(base, settings.keys, commands)))];
    }

    // White space shown: each space a centered dot, each tab a line, in the lines on screen.
    function whitespaceMarks(CM) {
        const space = CM.Decoration.mark({ class: "cm-m-space" });
        const tab = CM.Decoration.mark({ class: "cm-m-tab" });
        const build = (view) => {
            const builder = new CM.RangeSetBuilder();
            for (const { from, to } of view.visibleRanges) {
                const text = view.state.doc.sliceString(from, to);
                for (let index = 0; index < text.length; index++) {
                    const ch = text.charCodeAt(index);
                    if (ch === 32) builder.add(from + index, from + index + 1, space);
                    else if (ch === 9) builder.add(from + index, from + index + 1, tab);
                }
            }
            return builder.finish();
        };
        return CM.ViewPlugin.fromClass(class {
            constructor(view) { this.decorations = build(view); }
            update(update) { if (update.docChanged || update.viewportChanged) this.decorations = build(update.view); }
        }, { decorations: (plugin) => plugin.decorations });
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
            ".m-code-editor .cm-m-space{background-image:radial-gradient(circle at 50% 55%,rgba(127,127,127,.6) 12%,transparent 14%)}" +
            ".m-code-editor .cm-m-tab{background-image:linear-gradient(rgba(127,127,127,.5),rgba(127,127,127,.5));background-size:calc(100% - 4px) 1px;" +
            "background-position:center;background-repeat:no-repeat}" +
            ".m-code-editor-fallback{width:100%;font-family:ui-monospace,Menlo,Consolas,monospace;font-size:13px}" +
            ".m-diff-view{border:1px solid var(--m-border,rgba(0,0,0,.12));border-radius:4px;overflow:auto}" +
            ".m-file-tree .m-file-status{font-family:ui-monospace,Menlo,monospace;font-size:11px;font-weight:600;margin-left:auto;padding-left:8px}" +
            ".m-file-status-M{color:#e2a400}.m-file-status-A,.m-file-status-\\?{color:#2e9e4f}.m-file-status-D{color:#d32f2f}" +
            ".m-file-status-U{color:#d32f2f}.m-file-status-R{color:#1e88e5}.m-file-tree .m-file-ignored{opacity:.5}" +
            ".m-pan-zoom{position:relative;overflow:hidden;overflow:clip;touch-action:none;cursor:grab;outline:none}.m-pan-zoom:active{cursor:grabbing}" +
            ".m-pan-zoom-layer{position:absolute;left:0;top:0;transform-origin:0 0}" +
            ".m-pan-zoom-layer>svg{position:absolute;left:0;top:0;overflow:visible;pointer-events:none}" +
            ".m-pan-zoom-controls{position:absolute;right:8px;bottom:8px;cursor:default;border-radius:6px;padding:2px;" +
            "background:var(--m-panel-bg,#fff);border:1px solid var(--m-border,rgba(127,127,127,.3));box-shadow:0 1px 4px rgba(0,0,0,.15)}" +
            ".m-pan-zoom-controls .m-pan-zoom-actual{min-width:3.4em}";
        root.document.head.appendChild(style);
    }

    // ---- m-code-editor ----

    const codeEditor = {
        name: "mCodeEditor",
        props: {
            modelValue: { type: String, default: "" }, language: String, fileName: String, readOnly: Boolean,
            // wrap unset follows the user's settings
            lineNumbers: { type: Boolean, default: true }, wrap: { type: Boolean, default: undefined }, highlightLines: Array, schema: Object, lint: Function,
            name: String, form: String, minHeight: { type: String, default: "6rem" }, maxHeight: String, height: String,
            autofocus: Boolean, placeholder: String, ariaLabel: String, ariaLabelledby: String,
            // who last changed each run of lines (git blame), shown in a gutter while it is set
            blame: Array,
            // more extensions from the page (CM) => extension: a keymap, event handlers, completion sources
            extensions: Function,
            // a key of the buffer (a file tab): the editor's state, with its undo history, is kept under it when the editor
            // unmounts and taken back when one mounts again with the same key and text, so switching tabs keeps history
            stateKey: String,
            // the editor settings of this editor (as setSettings takes them), in place of the page's
            settings: Object,
            // a shared text (plans/archive/design-dev-aide.md F15): { ytext, awareness, undoManager } of a joined Y.Doc, with
            // editor.loadCollab() done; the text is the Y.Text's while it is set (the model follows it), the undo Yjs'
            collab: Object
        },
        emits: ["update:modelValue", "save", "cursor", "ready"],
        data() { return { failed: false, ready: false, text: this.modelValue || "" }; },
        computed: {
            resolvedLanguage() { return this.language || editor.languageOf(this.fileName); },
            accessibilityAttributes() {
                const attrs = {};
                if (this.ariaLabel) attrs["aria-label"] = this.ariaLabel;
                if (this.ariaLabelledby) attrs["aria-labelledby"] = this.ariaLabelledby;
                return attrs;
            }
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
            resolvedLanguage() { this.takeLanguage(); this.reconfigure("language"); },
            schema() { this.reconfigure("language"); },
            readOnly() { this.reconfigure("readOnly"); },
            wrap() { this.reconfigure("wrap"); },
            settings() { this.applySettings(); },
            ariaLabel() { this.reconfigure("accessibility"); },
            ariaLabelledby() { this.reconfigure("accessibility"); },
            highlightLines() { this.reconfigure("highlight"); this.reveal(); },
            blame() { this.reconfigure("blame"); },
            // a shared text bound: the editor takes its text first, so y-codemirror starts from the same text; unbound, the
            // editor keeps the text and has CodeMirror's history again
            collab(value) {
                if (!this.view) return;
                if (value && value.ytext) {
                    const text = value.ytext.toString(), now = this.view.state.doc.toString();
                    if (text !== now) this.view.dispatch({ changes: editor.textChange(now, text) });
                }
                ["collab", "history", "keys"].forEach((key) => this.reconfigure(key));
                this.markShared();
            }
        },
        mounted() {
            addStyles();
            liveEditors.add(this);
            this.$watch(() => isDark(this), () => this.reconfigure("theme"));
            this.takeLanguage();
            const settings = this.effectiveSettings();
            if (editor.KEYMAP_LIBRARIES[settings.keymap]) takeKeymap(settings.keymap);
            editor.load().then((CM) => { if (!this.unmounted) this.build(CM); }, (error) => {
                console.error("The code editor could not load; editing as plain text", error);
                this.failed = true;
            });
        },
        beforeUnmount() {
            this.unmounted = true;
            liveEditors.delete(this);
            if (this.view) {
                if (this.stateKey && this.owner) editor.keepState(this.stateKey, { state: this.view.state, compartments: this.compartments, owner: this.owner });
                this.owner.vm = null;
                this.view.destroy();
            }
        },
        methods: {
            // the settings this editor follows: its own, else the page's
            effectiveSettings() { return this.settings ? editor.normalizeSettings(this.settings) : editor.settings; },
            applySettings() { ["settings", "keys", "wrap"].forEach((key) => this.reconfigure(key)); },
            // the view's shared text, for Vim's undo and redo
            markShared() {
                if (!this.view) return;
                if (this.collab && this.collab.undoManager && root.YCodeMirror) sharedViews.set(this.view, this.collab);
                else sharedViews.delete(this.view);
            },
            // a language of the second bundle: the bundle, then the language (plain text until it loads, or if it fails)
            takeLanguage() {
                if (!editor.LEGACY_LANGUAGES[this.resolvedLanguage] || root.CodeMirrorLegacy) return;
                editor.loadLegacy().then(() => { if (!this.unmounted) this.reconfigure("language"); },
                    (error) => console.error("The " + this.resolvedLanguage + " language could not load; the file is plain text", error));
            },
            parts(CM) {
                const owner = this.owner;
                const save = (view) => { if (owner && owner.vm) owner.vm.$emit("save", view.state.doc.toString()); return true; };
                return {
                    language: () => languageExtension(CM, this.resolvedLanguage, this.schema),
                    theme: () => theme(CM, isDark(this)),
                    readOnly: () => [CM.EditorState.readOnly.of(!!this.readOnly), CM.EditorView.editable.of(!this.readOnly)],
                    wrap: () => ((this.wrap === undefined ? this.effectiveSettings().wrap : this.wrap) ? CM.EditorView.lineWrapping : []),
                    settings: () => {
                        const settings = this.effectiveSettings();
                        // the tab size and indent only when the user set one: CodeMirror's otherwise
                        const tab = settings.tabSize ? [CM.EditorState.tabSize.of(settings.tabSize), CM.indentUnit.of(" ".repeat(settings.tabSize))] : [];
                        return [CM.EditorView.theme({ "&": { fontSize: settings.fontSize + "px" } }), tab, settings.whitespace ? whitespaceMarks(CM) : []];
                    },
                    keys: () => keys(CM, save, this.effectiveSettings(), this.collab),
                    history: () => (this.collab && root.YCodeMirror ? [] : CM.history()),
                    collab: () => (this.collab && this.collab.ytext && root.YCodeMirror
                        ? root.YCodeMirror.yCollab(this.collab.ytext, this.collab.awareness || null,
                            this.collab.undoManager ? { undoManager: this.collab.undoManager } : {}) : []),
                    accessibility: () => CM.EditorView.contentAttributes.of(this.accessibilityAttributes),
                    highlight: () => highlighted(CM, this.highlightLines, this.view ? this.view.state.doc : CM.Text.of(this.text.split("\n"))),
                    blame: () => blameGutter(CM, this.blame)
                };
            },
            build(CM) {
                // a kept state of this buffer: its extensions call the editor through its owner, which is now this one
                const kept = this.stateKey ? editor.takeState(this.stateKey) : null;
                // a shared text's editor starts from the Y.Text (the model follows it while unmounted too)
                if (this.collab && this.collab.ytext) this.text = this.collab.ytext.toString();
                if (kept && kept.state.doc.toString() === this.text) {
                    this.cm = CM;
                    this.compartments = kept.compartments;
                    this.owner = kept.owner;
                    this.owner.vm = this;
                    this.view = new CM.EditorView({ state: kept.state, parent: this.$refs.host });
                    ["language", "theme", "readOnly", "wrap", "accessibility", "highlight", "blame", "settings", "keys", "history", "collab"].forEach((key) => this.reconfigure(key));
                    if (this.autofocus) this.view.focus();
                    this.markShared();
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
                this.markShared();
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
                return h("textarea", Object.assign({
                    class: "m-code-editor-fallback q-pa-sm", value: this.text, readonly: this.readOnly, name: this.name,
                    form: this.form, rows: 12, spellcheck: false, "data-state": "fallback",
                    onInput: (event) => { this.text = event.target.value; this.$emit("update:modelValue", this.text); }
                }, this.accessibilityAttributes));
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
            // unified and editable: Accept and Reject on each chunk (a reviewed proposal; plans/archive/design-dev-aide.md 13.3)
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
                const named = this.language || editor.languageOf(this.fileName);
                if (editor.LEGACY_LANGUAGES[named] && !root.CodeMirrorLegacy) {
                    editor.loadLegacy().then(() => { if (!this.unmounted) this.rebuild(); }, () => {});
                }
                const language = languageExtension(CM, named);
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
                }, (error) => { this.failed = editor.failureText(error); });
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
        center(view, at, size) { return { x: size.width / 2 - at.x * view.k, y: size.height / 2 - at.y * view.k, k: view.k }; },
        // the view after a wheel event ({ deltaX, deltaY, ctrlKey, metaKey, shiftKey }) at `at`: Ctrl or Cmd (a pinch on a
        // trackpad) zooms; with `wheelZoom` a plain wheel zooms too and Shift pans (sideways for a wheel with one axis);
        // without it the wheel pans
        wheel(view, event, at, wheelZoom, min, max) {
            const zoom = event.ctrlKey || event.metaKey || (wheelZoom && !event.shiftKey);
            if (zoom) return panZoom.zoomAt(view, at, Math.exp(-event.deltaY / 300), min, max);
            const dx = wheelZoom && !event.deltaX ? event.deltaY : event.deltaX, dy = wheelZoom && !event.deltaX ? 0 : event.deltaY;
            return { x: view.x - dx, y: view.y - dy, k: view.k };
        }
    };

    // A pan-and-zoom layer (plans/archive/design-dev-aide.md D9): one CSS transform on the layer; drag the background, the wheel
    // or a trackpad to pan, Ctrl+wheel or a pinch to zoom at the pointer, arrow keys and + and - for both, 0 to fit. With
    // `wheel-zoom` a plain wheel zooms at the pointer and Shift+wheel pans; `controls` adds zoom out, the zoom (click: 100%),
    // zoom in and Fit buttons in the bottom right corner. The default slot is the layer's content, absolutely placed; the
    // `under` slot is drawn beneath it (one SVG of connectors). A drag that starts on a button, a link, an input or
    // [data-no-pan] is left to it.
    const panZoomLayer = {
        name: "mPanZoom",
        props: { minZoom: { type: Number, default: 0.2 }, maxZoom: { type: Number, default: 2 }, height: { type: String, default: "100%" },
            wheelZoom: Boolean, controls: Boolean },
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
            middle() { const size = this.size(); return { x: size.width / 2, y: size.height / 2 }; },
            zoomBy(factor, at) { this.setView(panZoom.zoomAt(this.view, at || this.middle(), factor, this.minZoom, this.maxZoom)); },
            // to the scale `k`, keeping the layer point under `at` (the frame's middle by default) where it is
            zoomTo(k, at) { this.zoomBy(k / this.view.k, at); },
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
                if (event.target.closest && event.target.closest(".m-pan-zoom-controls")) return;
                this.setView(panZoom.wheel(this.view, event, this.local(event), this.wheelZoom, this.minZoom, this.maxZoom));
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
            },
            // zoom out, the zoom in percent (click: 100%), zoom in and Fit, outside the layer
            controlBar() {
                const button = root.Vue.resolveComponent("q-btn");
                const tooltip = root.Vue.resolveComponent("q-tooltip");
                const tip = (text) => ({ default: () => [h(tooltip, null, { default: () => text })] });
                const k = this.view.k;
                return h("div", { class: "m-pan-zoom-controls row items-center no-wrap", "data-no-pan": "" }, [
                    h(button, { dense: true, flat: true, round: true, size: "sm", icon: "remove", "aria-label": "Zoom out", disable: k <= this.minZoom,
                        class: "m-pan-zoom-out", onClick: () => this.zoomBy(1 / 1.25) }, tip("Zoom out")),
                    h(button, { dense: true, flat: true, noCaps: true, size: "sm", label: Math.round(k * 100) + "%", "aria-label": "Actual size",
                        class: "m-pan-zoom-actual", onClick: () => this.zoomTo(1) }, tip("Actual size")),
                    h(button, { dense: true, flat: true, round: true, size: "sm", icon: "add", "aria-label": "Zoom in", disable: k >= this.maxZoom,
                        class: "m-pan-zoom-in", onClick: () => this.zoomBy(1.25) }, tip("Zoom in")),
                    h(button, { dense: true, flat: true, round: true, size: "sm", icon: "fit_screen", "aria-label": "Fit",
                        class: "m-pan-zoom-fit", onClick: () => this.fit() }, tip("Fit"))
                ]);
            }
        },
        render() {
            return h("div", {
                class: "m-pan-zoom", tabindex: 0, style: { height: this.height }, onWheel: this.onWheel, onPointerdown: this.onDown,
                onPointermove: this.onMove, onPointerup: this.onUp, onPointercancel: this.onUp, onKeydown: this.onKey
            }, [h("div", { ref: "layer", class: "m-pan-zoom-layer", style: { transform: this.transform } },
                [this.$slots.under ? this.$slots.under() : null].concat(this.$slots.default ? this.$slots.default() : [])),
                this.controls ? this.controlBar() : null]);
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
