/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The Dev AIDE Workspace client (plans/design-dev-aide.md 4 and 5): m-dev-workspace, the IDE of a root (a repository or a
 * plain directory): the repository bar (root, branch, ahead and behind, Fetch, Pull, Push, Commit, Publish), the Code tab
 * (the file tree with git status, Search, the editor tabs in CodeMirror with the file status strip, Problems, the status
 * bar), Changes (staged, changes, untracked; stage, unstage, discard, diff; the commit form with the task trailer) and
 * Commits. It talks to the transitions of DevAide/Workspace.xml. A save that finds the file changed on disk opens
 * Compare; a reload report (the DevReload notification) refreshes the tree and the status and the clean tabs, and marks
 * a tab with edits "Changed on disk". Unsaved text is kept in the browser by root and path until saved.
 *
 * The pure parts are moqui.devAide.pure, which test/client/pages/DevAide.page.js runs in the page.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui || (root.moqui = {});
    const devAide = moqui.devAide = moqui.devAide || {};

    // ---- pure parts ----

    const pure = devAide.pure = {};

    // The open tabs of a root: open (or focus) a path, mark it dirty, close it; the active tab after a close is its
    // neighbour.
    pure.openTab = (tabs, tab) => {
        const found = tabs.find((have) => have.path === tab.path);
        return found ? { tabs, active: found.path } : { tabs: tabs.concat([tab]), active: tab.path };
    };
    /** A notification's JSON payload, whether it came as the message or inside it. */
    pure.messageJson = (message) => {
        let body = message && (message.message !== undefined ? message.message : message);
        if (typeof body === "string") { try { body = JSON.parse(body); } catch (err) { return null; } }
        return body && typeof body === "object" ? body : null;
    };
    /** The user names of the other editors with `path` open. */
    pure.editorsOf = (editors, path) => (editors || []).filter((editor) => (editor.paths || []).indexOf(path) >= 0).map((editor) => editor.username);
    pure.closeTab = (tabs, path, active) => {
        const index = tabs.findIndex((tab) => tab.path === path);
        if (index < 0) return { tabs, active };
        const rest = tabs.slice(0, index).concat(tabs.slice(index + 1));
        const next = active !== path ? active : (rest[index] || rest[index - 1] || {}).path || null;
        return { tabs: rest, active: next };
    };
    pure.isDirty = (tab) => !!tab && tab.text !== tab.saved;
    // A tab or any of its view's companion buffers has unsaved text.
    pure.isDirtyAll = (tab) => pure.isDirty(tab) || !!(tab && (tab.companions || []).some((buffer) => buffer.text !== buffer.saved));

    // The changed files of a status as the Changes tab groups them.
    pure.groupChanges = (entries) => {
        const groups = { conflicts: [], staged: [], changes: [], untracked: [] };
        for (const entry of entries || []) {
            if (entry.conflicted) groups.conflicts.push(entry);
            else if (entry.untracked) groups.untracked.push(entry);
            else {
                if (entry.staged) groups.staged.push(entry);
                if (entry.unstaged) groups.changes.push(entry);
            }
        }
        return groups;
    };

    // The files of a reload report (absolute paths) that are in a root, relative to it.
    pure.reloadedPaths = (report, rootPath) => {
        const prefix = String(rootPath || "").replace(/[\\/]+$/, "") + "/";
        return ((report && report.files) || [])
            .map((file) => String(file.path || "").replace(/\\/g, "/"))
            .filter((path) => path.startsWith(prefix.replace(/\\/g, "/")))
            .map((path) => path.slice(prefix.length));
    };

    // The root element of an XML text (after the prolog and comments), for its schema digest.
    pure.rootElement = (text) => {
        const body = String(text || "").replace(/<\?[\s\S]*?\?>/g, "").replace(/<!--[\s\S]*?-->/g, "").replace(/<!DOCTYPE[^>]*>/i, "");
        const match = /<([A-Za-z][\w.-]*)/.exec(body);
        return match ? match[1] : null;
    };

    // The words of a service or transition error answer.
    pure.errorText = (error) => {
        const json = error && error.json;
        const errors = json && (json.errors || json.errorMessage || json.message);
        if (Array.isArray(errors) && errors.length) return errors.join("; ");
        if (typeof errors === "string" && errors) return errors;
        if (error && error.status) return "HTTP " + error.status + (error.statusText ? " " + error.statusText : "");
        return String((error && error.message) || error || "failed");
    };

    // A storage key of a root's state in the browser.
    pure.storageKey = (rootId, what) => "moqui.devAide." + what + "." + rootId;
    // The name under the cursor for go to definition: the attribute value around `column` (0-based) of a line, else
    // the word of name characters there (letters, digits, . _ # : / -).
    pure.valueAt = (text, column) => {
        const quoted = /"([^"]*)"|'([^']*)'/g;
        let match;
        while ((match = quoted.exec(text))) {
            if (column > match.index && column < match.index + match[0].length) return match[1] !== undefined ? match[1] : match[2];
        }
        const name = /[A-Za-z0-9_.#:/-]/;
        let from = column, to = column;
        while (from > 0 && name.test(text[from - 1])) from--;
        while (to < text.length && name.test(text[to])) to++;
        return text.slice(from, to);
    };

    // The changed regions between two texts, by lines: a common prefix and suffix taken off, the rest by the longest common
    // subsequence of lines (a review's hunks: plans/design-dev-aide.md 13.3).
    pure.hunks = (a, b) => {
        const x = String(a || "").split("\n");
        const y = String(b || "").split("\n");
        let start = 0;
        while (start < x.length && start < y.length && x[start] === y[start]) start++;
        let endX = x.length, endY = y.length;
        while (endX > start && endY > start && x[endX - 1] === y[endY - 1]) { endX--; endY--; }
        const m = x.slice(start, endX), n = y.slice(start, endY);
        if (!m.length && !n.length) return 0;
        if (m.length * n.length > 4000000) return 1;
        const table = Array.from({ length: m.length + 1 }, () => new Array(n.length + 1).fill(0));
        for (let i = m.length - 1; i >= 0; i--) for (let j = n.length - 1; j >= 0; j--)
            table[i][j] = m[i] === n[j] ? table[i + 1][j + 1] + 1 : Math.max(table[i + 1][j], table[i][j + 1]);
        let i = 0, j = 0, count = 0, inHunk = false;
        while (i < m.length || j < n.length) {
            if (i < m.length && j < n.length && m[i] === n[j]) { i++; j++; inHunk = false; continue; }
            if (!inHunk) { count++; inHunk = true; }
            if (j >= n.length || (i < m.length && table[i + 1][j] >= table[i][j + 1])) i++; else j++;
        }
        return count;
    };

    // The visual views of a file tab (plans/design-dev-aide.md 5.2): each matches the file's root element, and its script
    // loads on first use and registers its component. A view edits the tab's buffer by the operations of the lossless
    // XML layer, so Text and the view share one undo history and one Save.
    devAide.viewSpecs = devAide.viewSpecs || [];
    devAide.registerView = (spec) => {
        devAide.viewSpecs = devAide.viewSpecs.filter((have) => have.name !== spec.name).concat([spec]);
    };
    // The views of the dev-aide, whose scripts load on first use and register their components.
    devAide.registerView({ name: "entity", label: "Entity", roots: ["entities", "entity-facade-xml"], component: "m-dev-entity-view",
        script: "/devstatic/DevAideEntity.js", ready: () => !!devAide.entityViewRegistered });
    devAide.registerView({ name: "screen", label: "Screen", roots: ["screen", "screen-extend"], component: "m-dev-screen-view",
        script: "/devstatic/DevAideScreen.js", ready: () => !!devAide.screenViewRegistered });
    devAide.registerView({ name: "service", label: "Service", roots: ["services"], component: "m-dev-service-view",
        script: "/devstatic/DevAideService.js", ready: () => !!devAide.serviceViewRegistered });
    devAide.registerView({ name: "eca", label: "Rules", roots: ["secas", "eecas", "emecas", "gitecas"], component: "m-dev-eca-view",
        script: "/devstatic/DevAideService.js", ready: () => !!devAide.serviceViewRegistered });
    // A screen in the screen builder (11.2): the entity view's "Show it on a form" and the like, by the reference's
    // other side (an index row with its root and path).
    devAide.openInBuilder = (ref) => {
        const other = (ref && ref.other) || ref || {};
        if (!other.rootId || !other.relativePath) return;
        root.location.href = "/qapps/dev/Screens/Design?rootId=" + encodeURIComponent(other.rootId) + "&path=" + encodeURIComponent(other.relativePath);
    };
    // The views offered for a file: Text first, then each spec whose roots take the file's root element.
    pure.viewsFor = (specs, path, element) => {
        const offered = [{ name: "text", label: "Text" }];
        if (!/\.xml$/i.test(String(path || "")) || !element) return offered;
        for (const spec of specs || []) if ((spec.roots || []).indexOf(element) >= 0) offered.push({ name: spec.name, label: spec.label });
        return offered;
    };
    // The order Save all writes files in (5.7, 6.2): entities, then services, then ECA rules, then screens, then data and
    // the rest, so a screen never loads before its field.
    pure.kindRank = (path) => {
        const text = String(path || "");
        if (/(^|\/)entity\//.test(text) && !/\.eecas\.xml$/.test(text)) return 0;
        if (/\.secas\.xml$|\.eecas\.xml$|\.emecas\.xml$|\.gitecas\.xml$/.test(text)) return 2;
        if (/(^|\/)service\//.test(text)) return 1;
        if (/(^|\/)screen(-extend)?\//.test(text)) return 3;
        return 4;
    };
    // The buffers to save, dirty ones only, in kind order.
    pure.saveOrder = (buffers) => (buffers || []).filter((buffer) => buffer && buffer.text !== buffer.saved)
        .map((buffer, index) => ({ buffer, index })).sort((a, b) => pure.kindRank(a.buffer.path) - pure.kindRank(b.buffer.path) || a.index - b.index)
        .map((entry) => entry.buffer);
    // The element of an outline at a path, or null.
    pure.outlineAt = (outline, path) => {
        let node = outline || null;
        for (const index of path || []) node = node && node.children ? node.children[index] || null : null;
        return node;
    };
    // The elements of an outline, depth first, each with its depth: what a view lists.
    pure.walkOutline = (outline, visit, depth) => {
        if (!outline) return;
        visit(outline, depth || 0);
        for (const child of outline.children || []) pure.walkOutline(child, visit, (depth || 0) + 1);
    };
    // The attribute of an outline element.
    pure.attr = (node, name) => {
        const found = node && (node.attrs || []).find((pair) => pair[0] === name);
        return found ? found[1] : undefined;
    };
    // The Workspace's transitions from the path of a Dev AIDE page (/qapps/dev/Entities/Edit -> /qapps/dev/Workspace).
    pure.workspaceBase = (path) => {
        const text = String(path || "");
        const at = text.lastIndexOf("/dev/");
        return (at >= 0 ? text.slice(0, at + 4) : text.replace(/\/[^/]*$/, "")) + "/Workspace";
    };

    // ---- the browser ----

    const store = {
        get(key) { try { const raw = root.localStorage && root.localStorage.getItem(key); return raw ? JSON.parse(raw) : null; } catch (err) { return null; } },
        set(key, value) { try { if (root.localStorage) root.localStorage.setItem(key, JSON.stringify(value)); } catch (err) { /* no storage */ } },
        drop(key) { try { if (root.localStorage) root.localStorage.removeItem(key); } catch (err) { /* no storage */ } }
    };

    const workspace = {
        name: "mDevWorkspace",
        // mode "guided" (D18): one artifact in its view, with no tree, tabs or repository bar
        props: { base: { type: String, required: true }, query: Object, mode: { type: String, default: "workspace" } },
        data() {
            return {
                roots: [], rootId: null, tier: "view", gitAvailable: false, upstreamEdits: false, shared: false,
                status: null, tab: "code", rail: "files", treeKey: 0, selected: null,
                tabs: [], active: null, cursor: { line: 1, column: 1 }, lastReload: "", restartNeeded: false,
                search: { query: "", regex: false, caseSensitive: false, pathFilter: "", results: [], running: false, truncated: false },
                diff: null, commitForm: { summary: "", body: "", amend: false, signOff: false, skipHooks: false, task: "" },
                log: [], commitDetail: null, goto: { open: false, filter: "", files: [] }, busy: "", schemas: {},
                refs: { title: "", items: [], loading: false }, artifactGoto: { open: false, text: "", results: [] },
                // an agent's change set under review (?review=<approvalId>): its files with their texts on disk and as proposed
                review: null, editorView: null, viewsLoaded: {},
                narrow: !!(root.innerWidth && root.innerWidth < 1024), offReload: null, reloadTimer: null,
                // shared mode (D19): the other users with files of this root open
                editors: [],
                // the task panel (14): the user's open tasks from their source
                taskList: { tasks: [], loading: false, error: "", sourceLabel: "" },
                // git depth (P8): Branches, Tags and Stashes under Advanced; a running network command's last line; the
                // diff's hunks; the conflicted file open in the conflict view; the Commits tab's paging and path
                advanced: !!store.get("advanced"), progress: null, hunks: null, conflictPath: null, gitLoaded: false,
                logPage: 0, logMore: false, logPath: "", tree: null
            };
        },
        computed: {
            root() { return this.roots.find((item) => item.id === this.rootId) || null; },
            isGit() { return !!(this.root && this.root.kind === "git" && this.gitAvailable); },
            current() { return this.tabs.find((tab) => tab.path === this.active) || null; },
            groups() { return pure.groupChanges(this.status && this.status.entries); },
            changeCount() { return ((this.status && this.status.entries) || []).length; },
            siteChanges() { return ((this.status && this.status.entries) || []).filter((entry) => entry.site).length; },
            problems() { return (this.current && this.current.problems) || []; },
            gotoMatches() {
                const filter = this.goto.filter.toLowerCase();
                return this.goto.files.filter((path) => !filter || path.toLowerCase().indexOf(filter) >= 0).slice(0, 50);
            },
            canWrite() { return this.tier !== "view" && !this.narrow; },
            guided() { return this.mode === "guided"; },
            siteComponent() { const root = this.roots.find((item) => !item.upstream); return root ? ((root.components || []).find((c) => c.site) || {}).name : null; },
            currentViews() { return this.current ? pure.viewsFor(devAide.viewSpecs, this.current.path, pure.rootElement(this.current.saved || this.current.text)) : []; },
            currentSpec() { return this.current && this.current.view !== "text" ? devAide.viewSpecs.find((spec) => spec.name === this.current.view) || null : null; },
            currentContext() { return this.current ? this.viewContext(this.current) : null; },
            workspaceLink() {
                const tab = this.current;
                if (!tab) return this.base;
                return this.base + "?rootId=" + encodeURIComponent(this.rootId) + "&path=" + encodeURIComponent(tab.path) +
                    (tab.view !== "text" ? "&view=" + encodeURIComponent(tab.view) : "") + (tab.artifact ? "&artifact=" + encodeURIComponent(tab.artifact) : "");
            }
        },
        mounted() {
            const client = this.$root && this.$root.notificationClient;
            if (client && client.registerListener) {
                this.offReload = client.registerListener("DevReload", (message) => this.onReload(message));
                this.offPresence = client.registerListener("DevPresence", (message) => this.onPresence(message));
                this.offSaved = client.registerListener("DevSaved", (message) => this.onSaved(message));
                this.offProgress = client.registerListener("DevGitProgress", (message) => {
                    const line = pure.messageJson(message);
                    if (line && this.progress && line.operationId === this.progress.id) this.progress.line = line.line;
                });
            }
            this.presenceTimer = setInterval(() => this.tellPresence(true), 60000);
            this.onResize = () => { this.narrow = root.innerWidth < 1024; };
            root.addEventListener("resize", this.onResize);
            this.loadRoots();
            const assist = moqui.assist;
            if (assist) {
                // the editor in the Assist drawer's page state, and the actions on the code blocks of its answers
                assist.editorState = () => this.editorState();
                if (assist.codeActions) {
                    assist.codeActions.register({ key: "dev.insert", label: "Insert at cursor", icon: "sym_o_input", run: (code) => this.insertCode(code, false) });
                    assist.codeActions.register({ key: "dev.replace", label: "Replace selection", icon: "sym_o_find_replace", run: (code) => this.insertCode(code, true) });
                    assist.codeActions.register({ key: "dev.propose", label: "Propose as change", icon: "sym_o_approval", run: (code, language) => this.proposeCode(code, language) });
                }
            }
            if (this.query && this.query.review) this.openReview(this.query.review);
            this.outlineTimers = {};
            this.saveHooks = {};
        },
        beforeUnmount() {
            if (this.offReload) this.offReload();
            if (this.offPresence) this.offPresence();
            if (this.offSaved) this.offSaved();
            if (this.offProgress) this.offProgress();
            clearInterval(this.presenceTimer);
            if (this.shared && this.rootId) this.post("presence", { rootId: this.rootId, paths: [] }).catch(() => {});
            root.removeEventListener("resize", this.onResize);
            const assist = moqui.assist;
            if (assist) {
                if (assist.editorState) assist.editorState = null;
                if (assist.codeActions) ["dev.insert", "dev.replace", "dev.propose"].forEach((key) => assist.codeActions.remove(key));
            }
        },
        watch: {
            tabs: { deep: true, handler() { this.remember(); this.tellPresence(false); } },
            tab(name) { if (["branches", "tags", "stashes", "settings"].indexOf(name) >= 0) this.loadGit(); },
            advanced(on) { store.set("advanced", on); if (!on && ["branches", "tags", "stashes"].indexOf(this.tab) >= 0) this.tab = "code"; },
            active() { this.remember(); },
            // a visual view reads the outline of the text as it changes, in Text or by its own operations
            "current.text"() { const tab = this.current; if (tab && tab.view !== "text") this.scheduleOutline(tab); }
        },
        methods: {
            get(name, params) { return moqui.request(this.base + "/" + name, { params: params || {} }).then((result) => result.json || {}); },
            post(name, params) { return moqui.request(this.base + "/" + name, { method: "POST", params: params || {} }).then((result) => result.json || {}); },
            fail(error) { this.busy = ""; this.$q.notify({ type: "negative", message: pure.errorText(error), timeout: 8000 }); },
            note(message, type) { this.$q.notify({ type: type || "positive", message, timeout: 4000 }); },
            loadRoots() {
                return this.get("roots").then((answer) => {
                    this.roots = answer.roots || [];
                    this.tier = answer.tier || "view";
                    this.gitAvailable = !!answer.gitAvailable;
                    this.upstreamEdits = !!answer.upstreamEdits;
                    this.shared = !!answer.shared;
                    const asked = this.query && this.query.rootId;
                    const site = this.roots.find((item) => !item.upstream);
                    this.selectRoot((asked && this.roots.find((item) => item.id === asked) ? asked : null) || (site || this.roots[0] || {}).id);
                    const view = this.query && this.query.view, artifact = this.query && this.query.artifact;
                    if (this.query && this.query.path) this.openFile(this.query.path, Number(this.query.line) || 0, false, { view, artifact });
                    else if (artifact) this.openArtifactId(artifact, view);
                    else if (this.query && this.query.page) this.openPage(this.query.page);
                }, this.fail);
            },
            selectRoot(id) {
                if (!id) return;
                this.rootId = id;
                this.tabs = []; this.active = null; this.status = null; this.diff = null; this.log = []; this.commitDetail = null;
                this.treeKey++;
                const kept = store.get(pure.storageKey(id, "tabs"));
                if (kept && Array.isArray(kept.paths)) {
                    if (!this.guided) kept.paths.forEach((path) => this.openFile(path, 0, true));
                    this.active = kept.active || null;
                }
                this.refreshStatus();
            },
            remember() {
                if (!this.rootId || this.guided) return;
                store.set(pure.storageKey(this.rootId, "tabs"), { paths: this.tabs.map((tab) => tab.path), active: this.active });
                for (const tab of this.tabs) {
                    if (tab.loading) continue;
                    const key = pure.storageKey(this.rootId, "draft:" + tab.path);
                    if (pure.isDirty(tab)) store.set(key, { base: tab.base, text: tab.text });
                    else store.drop(key);
                }
            },
            refreshStatus() {
                if (!this.isGit) { this.status = null; return Promise.resolve(); }
                return this.get("status", { rootId: this.rootId }).then((status) => {
                    this.status = status;
                    if (!this.commitForm.task && status.currentTask) this.commitForm.task = status.currentTask;
                }, this.fail);
            },
            loadDir(path) {
                return this.get("list", { rootId: this.rootId, path }).then((answer) => answer.entries || []);
            },
            // ---- files and tabs ----
            openFile(path, line, quiet, how) {
                if (!path) return;
                how = how || {};
                const opened = pure.openTab(this.tabs, { path, text: "", saved: "", base: "", loading: true, problems: [], line: line || 0,
                    view: "text", artifact: how.artifact || "", page: how.page || "", outline: null, outlineError: "", companions: [] });
                this.tabs = opened.tabs;
                if (!quiet) this.active = opened.active;
                this.tab = "code";
                const tab = this.tabs.find((item) => item.path === path);
                if (!tab.loading) {
                    if (line) tab.line = line;
                    if (how.artifact) tab.artifact = how.artifact;
                    if (how.view) this.setView(tab, how.view);
                    return;
                }
                this.get("file", { rootId: this.rootId, path }).then((file) => {
                    Object.assign(tab, {
                        loading: false, text: file.text || "", saved: file.text || "", base: file.hash, status: file.status || "",
                        readOnly: !!file.readOnly || !this.canWrite, readOnlyReason: file.readOnlyReason || (this.narrow ? "Read only below 1024 px" : ""),
                        binary: !!file.binary, lineEnding: file.lineEnding || "LF", component: file.component
                    });
                    const draft = store.get(pure.storageKey(this.rootId, "draft:" + path));
                    if (draft && draft.text !== tab.text && !tab.readOnly) {
                        this.$q.dialog({
                            title: "Unsaved changes", message: path + " has changes from an earlier visit that were not saved." +
                                (draft.base !== tab.base ? " The file changed since." : "") + " Restore them?", cancel: { label: "Discard them" },
                            ok: { label: "Restore" }
                        }).onOk(() => { tab.text = draft.text; }).onCancel(() => store.drop(pure.storageKey(this.rootId, "draft:" + path)));
                    }
                    this.loadSchema(tab);
                    this.takeDraft(tab, this.rootId);
                    if (how.view && how.view !== "text") this.setView(tab, how.view);
                }, (error) => { this.tabs = pure.closeTab(this.tabs, path, this.active).tabs; this.fail(error); });
            },
            // An artifact by its id, opened in its file at its line (in a view when one is asked for).
            openArtifactId(artifactId, view) {
                this.get("artifactDetail", { artifactId }).then((answer) => {
                    const artifact = answer.artifact;
                    if (!artifact || !artifact.rootId) { this.note("No artifact " + artifactId + " in the Workspace's roots", "warning"); return; }
                    if (artifact.rootId !== this.rootId) this.selectRoot(artifact.rootId);
                    this.openFile(artifact.relativePath, artifact.line || 0, false, { view, artifact: artifact.id });
                }, this.fail);
            },
            // An app page in the screen builder (Open in screen builder, 11.2): its screen's file in the screen view, the
            // page's path and query kept on the tab for the canvas.
            openPage(page) {
                this.get("screenView", { page }).then((answer) => {
                    const base = answer.view && answer.view.base;
                    if (!base) { this.note("The screen of " + page + " is not in the Workspace's roots", "warning"); return; }
                    if (base.rootId !== this.rootId) this.selectRoot(base.rootId);
                    this.openFile(base.path, 0, false, { view: "screen", page });
                }, this.fail);
            },
            // The component:// location of a file of a root, as the screen-xml dialect names one.
            locationOf(rootId, path) {
                const item = this.roots.find((root) => root.id === rootId);
                if (!item) return "";
                const component = (item.components || []).filter((c) => !c.path || path === c.path || path.indexOf(c.path + "/") === 0)
                    .sort((a, b) => b.path.length - a.path.length)[0];
                return component ? "component://" + component.name + "/" + (component.path ? path.slice(component.path.length + 1) : path) : "";
            },
            // An agent's proposal opened in the builder (13.5): the draft of the file's location becomes its unsaved text.
            takeDraft(buffer, rootId) {
                const key = "moqui.devAide.draft:" + this.locationOf(rootId, buffer.path);
                const draft = store.get(key);
                if (!draft || typeof draft.text !== "string") return;
                store.drop(key);
                buffer.text = draft.text;
                this.note("Opened with Assist's proposal, not saved: Save keeps it", "info");
            },
            // ---- visual views (5.2) ----
            setView(tab, name) {
                if (!tab) return;
                if (name === "text" || !name) { tab.view = "text"; return; }
                const spec = devAide.viewSpecs.find((item) => item.name === name);
                if (!spec) { this.note("No " + name + " view", "warning"); return; }
                const ready = spec.ready ? spec.ready() : true;
                const loaded = ready ? Promise.resolve() : moqui.loadScript(spec.script, null, () => !!(spec.ready && spec.ready()));
                loaded.then(() => {
                    this.viewsLoaded[name] = true;
                    tab.view = name;
                    this.refreshOutline(tab);
                }, this.fail);
            },
            scheduleOutline(tab) {
                clearTimeout(this.outlineTimers[tab.path]);
                this.outlineTimers[tab.path] = setTimeout(() => this.refreshOutline(tab), 300);
            },
            refreshOutline(tab) {
                const text = tab.text;
                return this.post("outline", { text }).then((answer) => {
                    if (tab.text !== text) return;
                    tab.outline = answer.outline || null; tab.outlineError = "";
                }, (error) => { tab.outlineError = pure.errorText(error); });
            },
            // What a view of `tab` works with: its outline and digest come from the tab; apply runs XML operations on the
            // tab's buffer (one undo step of its editor) or on a companion buffer the view opened (the site's extension
            // file, a seed data file), and save is the tab's Save, which saves the companions too.
            viewContext(tab) {
                const vm = this;
                return {
                    tab, base: this.base, tier: this.tier, rootId: this.rootId, root: this.root, roots: this.roots,
                    site: this.siteComponent, canWrite: this.canWrite && !tab.readOnly, mayWrite: this.canWrite, upstreamEdits: this.upstreamEdits,
                    get: (name, params) => vm.get(name, params), post: (name, params) => vm.post(name, params),
                    note: (message, type) => vm.note(message, type), fail: (error) => vm.fail(error),
                    apply: (ops, buffer) => vm.applyOps(tab, ops, buffer),
                    undo: () => vm.undo(tab), redo: () => vm.redo(tab),
                    outlineOf: (text) => vm.post("outline", { text }).then((answer) => answer.outline || null),
                    companion: (rootId, path, initialText) => vm.companion(tab, rootId, path, initialText),
                    save: () => vm.save(tab), onSaved: (hook) => { vm.saveHooks[tab.path] = hook; },
                    openFile: (path, line) => vm.openFile(path, line), openArtifact: (artifact) => vm.openArtifact(artifact),
                    openArtifactId: (id, view) => vm.openArtifactId(id, view), showText: () => { tab.view = "text"; },
                    showReferences: (artifact) => vm.showReferences(artifact)
                };
            },
            postOps(text, ops) { return this.post("xmlOp", { text, ops: JSON.stringify(ops) }); },
            applyOps(tab, ops, buffer) {
                if (!ops || !ops.length) return Promise.resolve();
                if (!buffer || buffer === tab) {
                    if (tab.readOnly) return Promise.reject(new Error(tab.readOnlyReason || "read only"));
                    const view = this.current === tab ? this.editorView : null;
                    let answered = null;
                    const post = (text, list) => this.postOps(text, list).then((answer) => { answered = answer; return answer; });
                    if (view && moqui.codeEditor) {
                        return moqui.codeEditor.applyXmlOps(view, ops, post).then((text) => {
                            tab.text = text;
                            if (answered && answered.outline) tab.outline = answered.outline;
                            return text;
                        });
                    }
                    return post(tab.text, ops).then((answer) => {
                        if (!answer || answer.text === undefined) throw new Error("no edits");
                        tab.text = answer.text;
                        if (answer.outline) tab.outline = answer.outline;
                        return answer.text;
                    });
                }
                return this.postOps(buffer.text, ops).then((answer) => {
                    if (!answer || answer.text === undefined) throw new Error("no edits");
                    buffer.undo.push(buffer.text); buffer.redo = [];
                    buffer.text = answer.text;
                    buffer.outline = answer.outline || null;
                    return answer.text;
                });
            },
            undo(tab) {
                const CM = root.CodeMirror;
                if (this.current === tab && this.editorView && CM && CM.undo) CM.undo(this.editorView);
            },
            redo(tab) {
                const CM = root.CodeMirror;
                if (this.current === tab && this.editorView && CM && CM.redo) CM.redo(this.editorView);
            },
            // A second buffer of a tab's view: a file in any root, made with `initialText` on its first save when missing.
            companion(tab, rootId, path, initialText) {
                const found = tab.companions.find((item) => item.rootId === rootId && item.path === path);
                if (found) return Promise.resolve(found);
                return this.get("file", { rootId, path, missingOk: true }).then((file) => {
                    const missing = !!file.missing;
                    const text = missing ? (initialText || "") : (file.text || "");
                    tab.companions.push({ rootId, path, text, saved: missing ? "" : text, base: missing ? "" : file.hash, missing,
                        readOnly: !!file.readOnly, readOnlyReason: file.readOnlyReason || "", outline: null, undo: [], redo: [] });
                    const buffer = tab.companions[tab.companions.length - 1];
                    this.takeDraft(buffer, rootId);
                    return this.post("outline", { text: buffer.text }).then((answer) => { buffer.outline = answer.outline || null; return buffer; },
                        () => buffer);
                });
            },
            loadSchema(tab) {
                if (!/\.xml$/i.test(tab.path)) return;
                const element = pure.rootElement(tab.text);
                if (!element) return;
                if (this.schemas[element] !== undefined) { tab.schema = this.schemas[element]; return; }
                this.get("schema", { rootElement: element }).then((answer) => {
                    this.schemas[element] = answer.digest || null;
                    tab.schema = this.schemas[element];
                }, () => { this.schemas[element] = null; });
            },
            closeTab(path) {
                const tab = this.tabs.find((item) => item.path === path);
                const close = () => {
                    const next = pure.closeTab(this.tabs, path, this.active); this.tabs = next.tabs; this.active = next.active;
                    if (moqui.codeEditor && moqui.codeEditor.dropState) moqui.codeEditor.dropState(this.rootId + ":" + path);
                };
                if (pure.isDirtyAll(tab)) {
                    this.$q.dialog({ title: "Close " + path + "?", message: "Its changes are not saved.", cancel: true, ok: { label: "Close", color: "negative" } }).onOk(close);
                } else close();
            },
            lintOf(tab) {
                if (!/\.xml$/i.test(tab.path)) return null;
                return (text) => this.post("check", { rootId: this.rootId, path: tab.path, text }).then((answer) => {
                    if (tab.text === text) tab.problems = answer.problems || [];
                    return answer.problems || [];
                });
            },
            save(tab) {
                tab = tab || this.current;
                if (!tab || tab.saving) return;
                const companions = (tab.companions || []).filter((buffer) => buffer.text !== buffer.saved);
                if (!companions.length) return this.saveTab(tab);
                // Save all of a view: the buffers by kind, the tab among them, then the view's hook
                const ordered = pure.saveOrder([tab].concat(companions));
                tab.saving = true;
                const reports = [];
                return ordered.reduce((chain, buffer) => chain.then(() => buffer === tab ? this.saveTab(tab, true) : this.saveCompanion(buffer).then((answer) => {
                    if (answer) reports.push(answer);
                })), Promise.resolve()).then(() => {
                    tab.saving = false;
                    const files = reports.map((answer) => ((answer.reload && answer.reload.files) || []).map((file) => file.summary).join("; ")).filter((text) => text);
                    if (files.length) this.lastReload = (this.lastReload ? this.lastReload + " · " : "Saved · ") + files.join(" · ");
                    const hook = this.saveHooks[tab.path];
                    if (hook) return hook(reports);
                }, (error) => { tab.saving = false; this.fail(error); });
            },
            saveCompanion(buffer) {
                return this.post("save", { rootId: buffer.rootId, path: buffer.path, text: buffer.text, baseHash: buffer.base }).then((answer) => {
                    if (answer.conflict) throw { errors: buffer.path + " changed on disk since the view read it; reopen the view" };
                    buffer.saved = buffer.text; buffer.base = answer.hash; buffer.missing = false;
                    if (answer.restart) this.restartNeeded = true;
                    return answer;
                });
            },
            saveTab(tab, inAll) {
                if (!tab || tab.readOnly || (tab.saving && !inAll)) return Promise.resolve();
                if (tab.text === tab.saved) return Promise.resolve();
                tab.saving = true;
                return this.post("save", { rootId: this.rootId, path: tab.path, text: tab.text, baseHash: tab.base }).then((answer) => {
                    tab.saving = inAll;
                    if (answer.conflict) {
                        tab.conflict = { diskText: answer.diskText || "", diskHash: answer.diskHash };
                        this.note(tab.path + " changed on disk since it was opened: compare them", "warning");
                        return;
                    }
                    tab.saved = tab.text; tab.base = answer.hash; tab.status = answer.status || ""; tab.conflict = null; tab.changedOnDisk = false;
                    tab.problems = answer.problems || [];
                    const files = (answer.reload && answer.reload.files) || [];
                    const summary = files.map((file) => file.summary).join("; ");
                    this.lastReload = "Saved" + (summary ? " · " + summary : "") + (answer.reload ? " · " + answer.reload.millis + " ms" : "");
                    if (answer.restart) this.restartNeeded = true;
                    this.note(this.lastReload, tab.problems.some((problem) => problem.severity === "error") ? "warning" : "positive");
                    this.treeKey++;
                    this.refreshStatus();
                    if (!inAll) { const hook = this.saveHooks[tab.path]; if (hook) return hook([answer]); }
                }, (error) => { tab.saving = false; if (inAll) throw error; this.fail(error); });
            },
            keepMine(tab) { tab.base = tab.conflict.diskHash; tab.conflict = null; this.save(tab); },
            takeDisk(tab) { tab.text = tab.conflict.diskText; tab.saved = tab.conflict.diskText; tab.base = tab.conflict.diskHash; tab.conflict = null; tab.changedOnDisk = false; },
            reloadTab(tab) {
                return this.get("file", { rootId: this.rootId, path: tab.path }).then((file) => {
                    if (file.hash === tab.base) { tab.changedOnDisk = false; return; }
                    if (pure.isDirty(tab)) { tab.changedOnDisk = true; tab.disk = { text: file.text || "", hash: file.hash }; return; }
                    tab.text = file.text || ""; tab.saved = tab.text; tab.base = file.hash; tab.status = file.status || ""; tab.changedOnDisk = false;
                }, () => { tab.changedOnDisk = true; });
            },
            useDisk(tab) {
                const disk = tab.disk || {};
                tab.text = disk.text || ""; tab.saved = tab.text; tab.base = disk.hash; tab.changedOnDisk = false;
            },
            compareDisk(tab) { tab.conflict = { diskText: (tab.disk && tab.disk.text) || "", diskHash: tab.disk && tab.disk.hash }; },
            // ---- shared mode (D19): who else has which files open, and their saves ----
            tellPresence(always) {
                if (!this.shared || !this.rootId) return;
                const paths = this.tabs.map((tab) => tab.path);
                const key = this.rootId + "|" + paths.join("|");
                if (!always && key === this.presenceKey) return;
                this.presenceKey = key;
                this.post("presence", { rootId: this.rootId, paths }).then((answer) => { this.editors = answer.editors || []; }, () => {});
            },
            onPresence(message) {
                const answer = pure.messageJson(message);
                if (!answer || answer.rootId !== this.rootId) return;
                const me = this.$root && this.$root.userId;
                this.editors = (answer.editors || []).filter((editor) => editor.userId !== me);
            },
            onSaved(message) {
                const saved = pure.messageJson(message);
                if (!saved || saved.rootId !== this.rootId || !this.tabs.some((tab) => tab.path === saved.path)) return;
                this.note(saved.path.split("/").pop() + " was saved by " + (saved.username || "another user"), "info");
            },
            editorsOf(path) { return pure.editorsOf(this.editors, path); },
            onReload(message) {
                let report = message && (message.message !== undefined ? message.message : message);
                if (typeof report === "string") { try { report = JSON.parse(report); } catch (err) { return; } }
                if (!this.root || !report) return;
                const paths = pure.reloadedPaths(report, this.root.path);
                if (!paths.length) return;
                this.lastReload = (report.files || []).map((file) => file.path.split(/[\\/]/).pop() + ": " + file.summary).join("; ");
                if (report.restart) this.restartNeeded = true;
                for (const tab of this.tabs) if (paths.indexOf(tab.path) >= 0 && !tab.saving) this.reloadTab(tab);
                clearTimeout(this.reloadTimer);
                this.reloadTimer = setTimeout(() => { this.treeKey++; this.refreshStatus(); }, 300);
            },
            // ---- tree actions ----
            treeMenu(event) { this.selected = event.path; this.menuTarget = event; this.$refs.treeMenu && this.$refs.treeMenu.show(event.event); },
            ask(title, value) {
                return new Promise((resolve) => {
                    this.$q.dialog({ title, prompt: { model: value || "", type: "text" }, cancel: true }).onOk((answer) => resolve(answer && answer.trim()));
                });
            },
            folderOf(target) { return !target ? "" : (target.dir ? target.path : target.path.split("/").slice(0, -1).join("/")); },
            newFile(target) {
                const folder = this.folderOf(target);
                this.ask("New file", folder ? folder + "/" : "").then((path) => {
                    if (!path) return;
                    this.post("createFile", { rootId: this.rootId, path, text: "" }).then(() => { this.treeKey++; this.openFile(path); this.refreshStatus(); }, this.fail);
                });
            },
            newFolder(target) {
                const folder = this.folderOf(target);
                this.ask("New folder", folder ? folder + "/" : "").then((path) => {
                    if (path) this.post("createDirectory", { rootId: this.rootId, path }).then(() => { this.treeKey++; }, this.fail);
                });
            },
            renamePath(target) {
                if (!target) return;
                this.ask("Rename or move " + target.path, target.path).then((to) => {
                    if (!to || to === target.path) return;
                    this.post("rename", { rootId: this.rootId, path: target.path, to }).then(() => {
                        const tab = this.tabs.find((item) => item.path === target.path);
                        if (tab) tab.path = to;
                        if (this.active === target.path) this.active = to;
                        this.treeKey++; this.refreshStatus();
                    }, this.fail);
                });
            },
            deletePath(target) {
                if (!target) return;
                const plain = !this.isGit ? " This directory has no git history, so it cannot be brought back." : "";
                this.$q.dialog({ title: "Delete " + target.path + "?", message: (target.dir ? "The folder and what is in it." : "The file.") + plain, cancel: true,
                    ok: { label: "Delete", color: "negative" } }).onOk(() => {
                    const confirm = plain ? new Promise((resolve) => this.$q.dialog({ title: "Really delete it?", message: target.path, cancel: true,
                        ok: { label: "Delete", color: "negative" } }).onOk(resolve)) : Promise.resolve();
                    confirm.then(() => this.post("delete", { rootId: this.rootId, path: target.path }).then(() => {
                        this.tabs = this.tabs.filter((tab) => tab.path !== target.path && tab.path.indexOf(target.path + "/") !== 0);
                        this.treeKey++; this.refreshStatus();
                    }, this.fail));
                });
            },
            copyPath(target) { if (target && root.navigator && root.navigator.clipboard) root.navigator.clipboard.writeText(target.path); },
            // ---- search and go to file ----
            runSearch() {
                if (!this.search.query) return;
                this.search.running = true;
                this.get("search", { rootId: this.rootId, query: this.search.query, regex: this.search.regex, caseSensitive: this.search.caseSensitive,
                    pathFilter: this.search.pathFilter }).then((answer) => {
                    this.search.running = false; this.search.results = answer.results || []; this.search.truncated = !!answer.truncated;
                }, (error) => { this.search.running = false; this.fail(error); });
            },
            openGoto() {
                this.goto.open = true; this.goto.filter = "";
                this.get("files", { rootId: this.rootId }).then((answer) => { this.goto.files = answer.files || []; }, this.fail);
            },
            gotoFile(path) { this.goto.open = false; if (path) this.openFile(path); },
            // ---- Assist on this page (plans/design-dev-aide.md 13.4) ----
            editorState() {
                const tab = this.current;
                if (!tab) return null;
                const view = this.editorView;
                let element = "", selection = "";
                if (view && view.state) {
                    const range = view.state.selection.main;
                    selection = view.state.sliceDoc(range.from, range.to).slice(0, 2000);
                    const line = view.state.doc.lineAt(range.head).text.trim();
                    const tag = /^<[A-Za-z][^>]*>?/.exec(line);
                    element = tag ? tag[0].slice(0, 200) : "";
                }
                return { root: this.rootId, path: tab.path, language: (tab.path.split(".").pop() || ""), view: tab.view || "text", artifact: tab.artifact || "",
                    unsaved: pure.isDirty(tab), element, line: this.cursor.line, selection, problems: (tab.problems || []).length };
            },
            insertCode(code, replace) {
                const view = this.editorView;
                if (!view || !this.current || this.current.readOnly) { this.note("Open a file you may edit first", "warning"); return; }
                const range = view.state.selection.main;
                view.dispatch({ changes: { from: replace ? range.from : range.head, to: replace ? range.to : range.head, insert: code } });
            },
            proposeCode(code, language) {
                const tab = this.current;
                const where = tab ? " in " + this.rootId + ":" + tab.path : "";
                const fence = "```" + (language || "") + "\n" + code + "\n```";
                if (moqui.assist && moqui.assist.ask) moqui.assist.ask("Propose this as a change" + where + " with propose_edit:\n" + fence);
            },
            // ---- Review: an agent's change set, each file a merge of the disk's text and the proposal ----
            openReview(approvalId) {
                this.get("changeSet", { approvalId }).then((answer) => {
                    const files = (answer.files || []).map((file) => Object.assign({}, file, { text: file.proposedText, open: true }));
                    this.review = { approvalId, summary: answer.summary || "", status: answer.status, files, saving: false };
                    this.tab = "review";
                }, this.fail);
            },
            leaveReview() { this.review = null; this.tab = "code"; },
            saveReview() {
                const review = this.review;
                if (!review) return;
                review.saving = true;
                let total = 0, accepted = 0;
                const edited = [], saved = [];
                const writable = review.files.filter((file) => !file.conflict);
                const saveOne = (file) => {
                    const proposed = pure.hunks(file.diskText || "", file.proposedText);
                    const kept = pure.hunks(file.text, file.proposedText);
                    total += proposed;
                    accepted += Math.max(0, proposed - kept);
                    if (file.text !== file.proposedText) edited.push(file.path);
                    if (file.text === (file.diskText || "")) return Promise.resolve();
                    return this.post("save", { rootId: file.rootId, path: file.path, text: file.text, baseHash: file.diskHash || "" }).then((answer) => {
                        if (answer.conflict) throw { errors: file.path + " changed on disk while you reviewed it" };
                        saved.push(file.path);
                    });
                };
                writable.reduce((chain, file) => chain.then(() => saveOne(file)), Promise.resolve())
                    .then(() => this.post("completeReview", { approvalId: review.approvalId, accepted, total, edited: edited.join(","), saved: saved.join(",") }))
                    .then((answer) => {
                        this.note("Saved: " + (answer.review || saved.length + " files"));
                        this.review = null; this.tab = "code"; this.treeKey++; this.refreshStatus();
                    }, (error) => { review.saving = false; this.fail(error); });
            },
            // ---- the artifact index: go to definition (F12, Mod+click), references (Shift+F12), go to artifact ----
            editorExtensions() {
                return (CM) => [
                    CM.keymap.of([
                        { key: "F12", run: (view) => { this.goToDefinition(view, view.state.selection.main.head); return true; } },
                        { key: "Shift-F12", run: (view) => { this.findReferences(view, view.state.selection.main.head); return true; } }
                    ]),
                    CM.EditorView.domEventHandlers({
                        mousedown: (event, view) => {
                            if (!(event.ctrlKey || event.metaKey)) return false;
                            const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
                            if (pos == null) return false;
                            event.preventDefault();
                            this.goToDefinition(view, pos);
                            return true;
                        }
                    })
                ];
            },
            atPosition(view, pos) {
                const line = view.state.doc.lineAt(pos);
                return { line: line.number, word: pure.valueAt(line.text, pos - line.from) };
            },
            goToDefinition(view, pos) {
                const tab = this.current;
                if (!tab) return;
                const at = this.atPosition(view, pos);
                this.get("definition", { rootId: this.rootId, path: tab.path, line: at.line, word: at.word }).then((answer) => {
                    const artifact = answer.artifact;
                    if (!artifact) { this.note("No definition found for " + (at.word || "this line"), "warning"); return; }
                    this.openArtifact(artifact);
                }, this.fail);
            },
            openArtifact(artifact) {
                if (!artifact.rootId || !artifact.relativePath) { this.note(artifact.name + " is not in a root of the Workspace", "warning"); return; }
                if (artifact.rootId !== this.rootId) this.selectRoot(artifact.rootId);
                this.openFile(artifact.relativePath, artifact.line || 0);
            },
            findReferences(view, pos) {
                const tab = this.current;
                if (!tab) return;
                const at = this.atPosition(view, pos);
                this.refs = { title: "References", items: [], loading: true };
                this.get("definition", { rootId: this.rootId, path: tab.path, line: at.line, word: at.word }).then((answer) => {
                    const artifact = answer.artifact;
                    if (!artifact) { this.refs = { title: "No artifact at line " + at.line, items: [], loading: false }; return; }
                    this.showReferences(artifact);
                }, (error) => { this.refs.loading = false; this.fail(error); });
            },
            showReferences(artifact) {
                this.refs = { title: "Used by " + artifact.kind + " " + artifact.shortName, items: [], loading: true, artifactId: artifact.id };
                this.get("refs", { artifactId: artifact.id, direction: "in" }).then((answer) => {
                    this.refs.items = (answer.refs || []).map((entry) => ({
                        label: (entry.other ? entry.other.kind + " " + entry.other.shortName : entry.from),
                        caption: entry.refType + " · " + (entry.relativePath || entry.path || "").split("/").slice(-2).join("/") + ":" + entry.line +
                            (entry.calls ? " · " + entry.calls + " calls" : ""),
                        rootId: entry.rootId, relativePath: entry.relativePath, line: entry.line
                    }));
                    this.refs.loading = false;
                }, (error) => { this.refs.loading = false; this.fail(error); });
            },
            openArtifactGoto() { this.artifactGoto = { open: true, text: "", results: [] }; },
            searchArtifacts() {
                const text = this.artifactGoto.text;
                if (!text || text.length < 2) { this.artifactGoto.results = []; return; }
                this.get("artifacts", { text, limit: 30 }).then((answer) => {
                    if (this.artifactGoto.text === text) this.artifactGoto.results = answer.artifacts || [];
                }, this.fail);
            },
            gotoArtifact(artifact) { this.artifactGoto.open = false; if (artifact) this.openArtifact(artifact); },
            // ---- changes and commits ----
            paths(entries) { return entries.map((entry) => entry.path).join(","); },
            stage(entries) { this.post("stage", { rootId: this.rootId, paths: this.paths(entries) }).then(() => this.refreshStatus(), this.fail); },
            unstage(entries) { this.post("unstage", { rootId: this.rootId, paths: this.paths(entries) }).then(() => this.refreshStatus(), this.fail); },
            discard(entries) {
                this.$q.dialog({ title: "Discard the changes?", message: entries.map((entry) => entry.path).join("\n"), cancel: true,
                    ok: { label: "Discard", color: "negative" } }).onOk(() => this.post("discard", { rootId: this.rootId, paths: this.paths(entries) })
                    .then(() => { this.refreshStatus(); this.treeKey++; this.tabs.forEach((tab) => this.reloadTab(tab)); }, this.fail));
            },
            showDiff(entry, staged) {
                if (entry.conflicted || entry.letter === "U") { this.openConflict(entry.path); return; }
                this.conflictPath = null;
                this.get("diff", { rootId: this.rootId, path: entry.path, staged: !!staged }).then((answer) => {
                    this.diff = { path: entry.path, staged: !!staged, original: answer.original || "", modified: answer.modified || "" };
                }, this.fail);
                this.loadHunks(entry.path, !!staged);
            },
            loadHunks(path, staged) {
                this.hunks = null;
                if (!this.isGit) return;
                this.get("hunks", { rootId: this.rootId, path, staged }).then((answer) => {
                    this.hunks = { path, staged, list: answer.hunks || [], diffHash: answer.diffHash };
                }, () => { this.hunks = null; });
            },
            hunkLabel(hunk) { return pure.hunkLabel ? pure.hunkLabel(hunk, this.hunks && this.hunks.staged) : ""; },
            // one hunk staged, unstaged or discarded (4.5): git apply of its patch, checked against the diff read
            hunkWrite(kind, index) {
                const hunks = this.hunks;
                if (!hunks) return;
                const run = () => this.post(kind + "Hunks", { rootId: this.rootId, path: hunks.path, hunks: [index], diffHash: hunks.diffHash }).then(() => {
                    this.refreshStatus();
                    if (kind === "discard") this.tabs.filter((tab) => tab.path === hunks.path).forEach((tab) => this.reloadTab(tab));
                    this.showDiff({ path: hunks.path }, hunks.staged);
                }, (error) => { this.fail(error); this.showDiff({ path: hunks.path }, hunks.staged); });
                if (kind !== "discard") { run(); return; }
                this.$q.dialog({ title: "Discard this change?", message: this.hunkLabel(hunks.list[index]), cancel: true,
                    ok: { label: "Discard", color: "negative" } }).onOk(run);
            },
            loadGit() {
                if (this.gitLoaded) return Promise.resolve();
                return moqui.loadScript("/devstatic/DevAideGit.js", null, () => !!devAide.gitRegistered).then(() => { this.gitLoaded = true; }, this.fail);
            },
            openConflict(path) { this.diff = null; this.hunks = null; this.loadGit().then(() => { this.conflictPath = path; }); },
            afterConflict() {
                this.tab = "changes";
                this.refreshStatus().then(() => {
                    const first = ((this.status && this.status.entries) || []).find((entry) => entry.conflicted);
                    if (first) this.openConflict(first.path);
                });
                this.treeKey++;
            },
            reloadTabs() { this.tabs.forEach((tab) => { if (!pure.isDirty(tab)) this.reloadTab(tab); }); this.treeKey++; },
            // ---- the task panel (14) ----
            loadTasks() {
                this.taskList.loading = true;
                return this.get("tasks", {}).then((answer) => {
                    this.taskList = { tasks: answer.tasks || [], loading: false, error: answer.error || "", sourceLabel: answer.sourceLabel || "" };
                }, (error) => { this.taskList.loading = false; this.taskList.error = pure.errorText(error); });
            },
            copyText(text) { if (root.navigator && root.navigator.clipboard) root.navigator.clipboard.writeText(text).then(() => this.note("Copied " + text)); },
            useTask(task, current) {
                this.commitForm.task = task.key;
                if (current) this.setCurrentTask(); else { this.tab = "changes"; this.note("The next commit gets " + task.key + "'s trailer"); }
            },
            taskBranch(task) {
                this.post("taskBranch", { rootId: this.rootId, taskId: task.key, title: task.title }).then((answer) => {
                    this.commitForm.task = task.key;
                    this.note("On " + answer.branch); this.refreshStatus(); this.reloadTabs();
                }, this.fail);
            },
            // the current task's trailer kept for this root (14)
            setCurrentTask() {
                this.post("setCurrentTask", { rootId: this.rootId, task: this.commitForm.task || "" }).then(() => {
                    this.note(this.commitForm.task ? "Current task: " + this.commitForm.task : "No current task");
                    this.refreshStatus();
                }, this.fail);
            },
            // Amend starts from the last commit's message
            amendChanged(on) {
                if (!on || this.commitForm.summary) return;
                this.get("log", { rootId: this.rootId, pageSize: 1 }).then((answer) => {
                    const last = (answer.commits || [])[0];
                    if (last) return this.get("commitInfo", { rootId: this.rootId, commitId: last.id });
                    return null;
                }).then((info) => {
                    if (!info || this.commitForm.summary) return;
                    const lines = String(info.message || "").split("\n");
                    this.commitForm.summary = lines[0] || "";
                    this.commitForm.body = lines.slice(1).join("\n").replace(/^\n+/, "").replace(/\n*(Task|Fixes): .*$/, "");
                }, this.fail);
            },
            commit(andPush) {
                this.busy = "commit";
                this.post("commit", Object.assign({ rootId: this.rootId }, this.commitForm)).then((answer) => {
                    this.busy = "";
                    this.note("Committed " + String(answer.commitId || "").slice(0, 9));
                    this.commitForm.summary = ""; this.commitForm.body = ""; this.commitForm.amend = false;
                    this.refreshStatus();
                    if (andPush) this.network("push");
                }, this.fail);
            },
            // a network command (fetch, pull, push, a tag's push, a prune) with its progress and Cancel (4.2)
            network(what, params) {
                this.busy = what;
                const id = "git-" + Date.now().toString(36) + "-" + Math.random().toString(36).slice(2, 7);
                this.progress = { id, what, line: "" };
                return this.post(what, Object.assign({ rootId: this.rootId, operationId: id }, params || {})).then((answer) => {
                    this.busy = ""; this.progress = null;
                    this.note((what.charAt(0).toUpperCase() + what.slice(1)) + " done" + (answer.output ? ": " + answer.output.trim().split("\n").pop() : ""));
                    // an upstream change that removed what the site extends: the upgrade checklist
                    if (answer.extensionsAffected) this.$q.notify({ type: "warning", timeout: 0, closeBtn: "Close",
                        message: "Extensions affected: " + answer.extensionsAffected, actions: [{ label: "Customizations", color: "white",
                            handler: () => { window.location.href = "/qapps/dev/Artifacts/Customizations"; } }] });
                    this.refreshStatus(); this.treeKey++;
                    return answer;
                }, (error) => {
                    this.progress = null;
                    // a pull is fast-forward only: when the branch and its upstream both moved, offer to merge the upstream
                    const text = pure.errorText(error);
                    if (what === "pull" && /fast-forward|diverge/i.test(text) && this.status && this.status.upstream) {
                        this.busy = "";
                        this.$q.dialog({ title: "Your branch and " + this.status.upstream + " have both moved", message: "Merge " + this.status.upstream +
                            " into " + (this.status.branch || "this branch") + " with a merge commit? Conflicts, if any, open in Changes.", cancel: true,
                            ok: { label: "Merge" } }).onOk(() => this.mergeUpstream());
                        return null;
                    }
                    this.fail(error); throw error;
                });
            },
            mergeUpstream() {
                const upstream = this.status && this.status.upstream;
                if (!upstream) return;
                this.post("mergeBranch", { rootId: this.rootId, branch: upstream, mergeCommit: true }).then((answer) => {
                    if (answer.conflict) { this.note("The merge stopped at conflicts", "warning"); this.afterConflict(); return; }
                    this.note("Merged " + upstream); this.refreshStatus(); this.reloadTabs();
                }, this.fail);
            },
            cancelNetwork() { if (this.progress) this.post("cancelGit", { operationId: this.progress.id }).catch(() => {}); },
            // a push that overwrites the remote branch, for the full tier, after the remote branch is typed back (question 8)
            forcePush() {
                const status = this.status || {};
                const target = status.upstream || ("origin/" + (status.branch || ""));
                this.$q.dialog({ title: "Force push", message: "This overwrites " + target + " with your branch, as long as it still has what you last fetched. Type " +
                    target + " to confirm.", prompt: { model: "", type: "text" }, cancel: true, ok: { label: "Force push", color: "negative" } })
                    .onOk((typed) => this.network("push", { forceWithLease: true, confirmRef: typed }).catch(() => {}));
            },
            publish() {
                this.$q.dialog({ title: "Publish the site's changes", message: "Commit the site component's " + this.siteChanges +
                    " changed files and push them. A summary (or leave it for one that names the files):",
                    prompt: { model: "", type: "text" }, cancel: true, ok: { label: "Publish" } }).onOk((summary) => {
                    this.busy = "publish";
                    this.post("publish", { rootId: this.rootId, summary: summary || "", task: this.commitForm.task }).then((answer) => {
                        this.busy = "";
                        this.note(answer.message || "Published", answer.published ? "positive" : "warning");
                        this.refreshStatus();
                    }, this.fail);
                });
            },
            switchBranch(name) {
                this.post("switchBranch", { rootId: this.rootId, branch: name }).then(() => {
                    this.note("On " + name); this.refreshStatus(); this.treeKey++; this.tabs.forEach((tab) => this.reloadTab(tab));
                }, this.fail);
            },
            newBranch(from) {
                return this.ask(from ? "New branch at " + String(from).slice(0, 9) : "New branch from the current one", "").then((name) => {
                    if (!name) return null;
                    const params = { rootId: this.rootId, branch: name };
                    if (from) params.startPoint = from;
                    return this.post("createBranch", params).then(() => { this.note("On " + name); this.refreshStatus(); this.reloadTabs(); }, this.fail);
                });
            },
            // the Commits tab, a page at a time, for the branch or one path (History)
            loadLog(more) {
                const page = more ? this.logPage + 1 : 0;
                const params = { rootId: this.rootId, pageSize: 100, pageIndex: page };
                if (this.logPath) params.path = this.logPath;
                return this.get("log", params).then((answer) => {
                    const commits = answer.commits || [];
                    this.log = more ? this.log.concat(commits) : commits;
                    this.logPage = page; this.logMore = commits.length === 100;
                }, this.fail);
            },
            history(path) { this.logPath = path || ""; this.commitDetail = null; this.diff = null; this.tab = "commits"; this.loadLog(); },
            replay(kind) {
                const commit = this.commitDetail;
                if (!commit) return;
                const verb = kind === "revert" ? "Revert" : "Cherry-pick";
                this.$q.dialog({ title: verb + " " + String(commit.id).slice(0, 9) + "?", message: (commit.message || "").split("\n")[0], cancel: true })
                    .onOk(() => this.post(kind === "revert" ? "revertCommit" : "cherryPick", { rootId: this.rootId, commitId: commit.id }).then((answer) => {
                        if (answer.conflict) { this.note(verb + " stopped at conflicts", "warning"); this.afterConflict(); return; }
                        this.note(verb + " done"); this.refreshStatus(); this.loadLog(); this.reloadTabs();
                    }, this.fail));
            },
            tagHere() {
                const commit = this.commitDetail;
                if (commit) this.ask("Tag " + String(commit.id).slice(0, 9), "").then((tag) => {
                    if (tag) this.post("createTag", { rootId: this.rootId, tag, commitId: commit.id }).then(() => this.note("Tagged " + tag), this.fail);
                });
            },
            copyId() { const commit = this.commitDetail; if (commit && root.navigator && root.navigator.clipboard) root.navigator.clipboard.writeText(commit.id).then(() => this.note("Copied")); },
            // Browse files at this commit: its tree a directory at a time, a file read only
            browseTree(path) {
                const commit = this.commitDetail;
                if (!commit) return;
                this.get("tree", { rootId: this.rootId, rev: commit.id, path: path || "" }).then((answer) => {
                    this.tree = { path: path || "", entries: answer.entries || [], file: null };
                }, this.fail);
            },
            browseFile(entry) {
                const commit = this.commitDetail;
                if (entry.dir) { this.browseTree(entry.path); return; }
                this.get("fileAt", { rootId: this.rootId, rev: commit.id, path: entry.path }).then((answer) => {
                    this.tree.file = { path: entry.path, text: answer.binary ? "(a binary file)" : answer.text || "" };
                }, this.fail);
            },
            // Blame on the current tab: who last changed each run of its lines, in the editor's gutter
            toggleBlame() {
                const tab = this.current;
                if (!tab) return;
                if (tab.blame) { tab.blame = null; return; }
                this.get("blame", { rootId: this.rootId, path: tab.path }).then((answer) => {
                    tab.blame = answer.ranges || [];
                    if (!tab.blame.length) this.note("Not committed yet: no blame", "info");
                }, this.fail);
            },
            stageCurrent() { const tab = this.current; if (tab) this.stage([{ path: tab.path }]); },
            discardCurrent() { const tab = this.current; if (tab) this.discard([{ path: tab.path }]); },
            diffCurrent() { const tab = this.current; if (tab) { this.tab = "changes"; this.showDiff({ path: tab.path }, false); } },
            openCommit(commit) {
                this.get("commitInfo", { rootId: this.rootId, commitId: commit.id }).then((answer) => {
                    this.commitDetail = Object.assign({ id: commit.id }, answer); this.diff = null; this.tree = null;
                }, this.fail);
            },
            commitDiff(file) {
                this.get("diff", { rootId: this.rootId, path: file.path, commitId: this.commitDetail.id }).then((answer) => {
                    this.diff = { path: file.path, original: answer.original || "", modified: answer.modified || "" };
                }, this.fail);
            },
            restart() {
                this.$q.dialog({ title: "Restart the server?", message: "Sessions and the sim overlay are lost; the open tabs come back.", cancel: true })
                    .onOk(() => this.post("restart").then(() => { this.note("Restarting…", "warning"); this.restartNeeded = false; }, this.fail));
            },
            isDirtyAll(tab) { return pure.isDirtyAll(tab); },
            // Mod+Z, Mod+Shift+Z and Mod+S in a visual view: the buffer's undo and redo, and Save
            onViewKey(event, tab) {
                const mod = event.ctrlKey || event.metaKey;
                if (!mod) return;
                const target = event.target;
                // a field of the view keeps its own undo
                if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)) {
                    if (event.key.toLowerCase() === "s") { event.preventDefault(); this.save(tab); }
                    return;
                }
                const key = event.key.toLowerCase();
                if (key === "z") { event.preventDefault(); if (event.shiftKey) this.redo(tab); else this.undo(tab); }
                else if (key === "y") { event.preventDefault(); this.redo(tab); }
                else if (key === "s") { event.preventDefault(); this.save(tab); }
            },
            onKey(event) {
                const mod = event.ctrlKey || event.metaKey;
                if (mod && event.shiftKey && event.key.toLowerCase() === "o") { event.preventDefault(); this.openArtifactGoto(); }
                else if (mod && event.key.toLowerCase() === "p") { event.preventDefault(); this.openGoto(); }
                else if (mod && event.shiftKey && event.key.toLowerCase() === "f") { event.preventDefault(); this.tab = "code"; this.rail = "search"; }
            }
        },
        template: `
<div class="m-dev-workspace column no-wrap" :class="{ 'm-dev-guided': guided }" id="DevWorkspace" tabindex="-1" @keydown="onKey">
  <div v-if="guided" class="row items-center no-wrap q-gutter-x-sm q-px-sm q-py-xs" id="DevGuidedBar">
    <q-icon :name="root && root.upstream ? 'sym_o_lock' : 'sym_o_folder'" size="18px"></q-icon>
    <span class="text-subtitle2 ellipsis">{{current ? current.path.split('/').pop() : ''}}</span>
    <span class="text-caption m-muted ellipsis">{{root ? root.label : ''}}</span>
    <q-space></q-space>
    <q-btn dense flat no-caps icon="sym_o_code" label="Open in Workspace" :href="workspaceLink" type="a" id="DevOpenInWorkspace"></q-btn>
  </div>
  <div v-else class="m-dev-repo-bar row items-center no-wrap q-gutter-x-sm q-px-sm q-py-xs">
    <q-select dense outlined emit-value map-options options-dense style="min-width: 14rem" :model-value="rootId" @update:model-value="selectRoot"
        :options="roots.map(r => ({ label: r.label + (r.upstream ? '' : ' (site)'), value: r.id, upstream: r.upstream }))" id="DevRootSelect">
      <template v-slot:prepend><q-icon :name="root && root.upstream ? 'sym_o_lock' : 'sym_o_folder'" size="18px"></q-icon></template>
    </q-select>
    <q-chip v-if="root && root.upstream" dense square icon="sym_o_lock" class="m-dev-upstream-chip">upstream{{upstreamEdits ? ' · editing on' : ''}}</q-chip>
    <template v-if="isGit && status">
      <q-btn-dropdown dense flat no-caps icon="sym_o_account_tree" :label="status.branch || 'detached'" id="DevBranchMenu">
        <q-list dense style="min-width: 12rem">
          <q-item v-for="branch in status.branches" :key="branch.name" clickable v-close-popup @click="switchBranch(branch.name)" :active="branch.current">
            <q-item-section>{{branch.name}}</q-item-section><q-item-section side v-if="branch.upstream" class="text-caption">{{branch.upstream}}</q-item-section>
          </q-item>
          <q-separator></q-separator>
          <q-item clickable v-close-popup @click="newBranch()"><q-item-section>New branch…</q-item-section></q-item>
          <q-item v-if="advanced && tier === 'full' && status.upstream" clickable v-close-popup @click="forcePush"><q-item-section class="text-negative">Force push…</q-item-section></q-item>
        </q-list>
      </q-btn-dropdown>
      <span class="text-caption m-muted" v-if="status.upstream">↑{{status.ahead}} ↓{{status.behind}}</span>
      <span class="text-caption" id="DevChangeCount">{{changeCount}} changed</span>
    </template>
    <q-chip v-else-if="root" dense square icon="sym_o_folder_off" id="DevNotGit">{{root.label}} · Not a git repository</q-chip>
    <q-space></q-space>
    <q-btn dense flat no-caps label="Fetch" :disable="!isGit || !canWrite" :loading="busy === 'fetch'" @click="network('fetch')"><q-tooltip v-if="!isGit">Not a git repository</q-tooltip></q-btn>
    <q-btn dense flat no-caps label="Pull" :disable="!isGit || !canWrite" :loading="busy === 'pull'" @click="network('pull')"></q-btn>
    <q-btn dense flat no-caps :label="'Push' + (status && status.ahead ? ' ' + status.ahead : '')" :disable="!isGit || !canWrite || (root && root.publishBranchRequired)" :loading="busy === 'push'" @click="network('push')"></q-btn>
    <q-btn dense flat no-caps label="Commit…" :disable="!isGit || !canWrite" @click="tab = 'changes'"></q-btn>
    <q-btn dense unelevated no-caps color="primary" :label="'Publish' + (siteChanges ? ' ' + siteChanges : '')" :disable="!isGit || !canWrite || !siteChanges" :loading="busy === 'publish'" @click="publish" id="DevPublish"></q-btn>
  </div>
  <q-banner v-if="restartNeeded" dense class="bg-amber-2 text-black" id="DevRestartBanner">A change needs a restart to apply.
    <template v-slot:action><q-btn dense flat no-caps label="Restart" @click="restart"></q-btn><q-btn dense flat no-caps label="Later" @click="restartNeeded = false"></q-btn></template>
  </q-banner>
  <q-tabs v-if="!guided" dense align="left" no-caps v-model="tab" class="m-dev-tabs">
    <q-tab name="code" label="Code"></q-tab>
    <q-tab name="changes" :label="'Changes' + (changeCount ? ' ' + changeCount : '')" :disable="!isGit"></q-tab>
    <q-tab name="commits" label="Commits" :disable="!isGit" @click="loadLog()"></q-tab>
    <q-tab v-if="advanced" name="branches" label="Branches" :disable="!isGit" id="DevBranchesTab"></q-tab>
    <q-tab v-if="advanced" name="tags" label="Tags" :disable="!isGit"></q-tab>
    <q-tab v-if="advanced" name="stashes" label="Stashes" :disable="!isGit"></q-tab>
    <q-tab name="settings" label="Settings" :disable="!isGit"></q-tab>
    <q-tab v-if="review" name="review" :label="'Review ' + review.files.length" id="DevReviewTab"></q-tab>
    <q-space></q-space>
    <q-toggle dense size="xs" v-model="advanced" label="Advanced" class="q-mr-sm text-caption" id="DevAdvanced"></q-toggle>
  </q-tabs>
  <div v-if="progress" class="row items-center no-wrap q-px-sm q-gutter-x-sm text-caption" id="DevGitProgress">
    <q-spinner size="1em"></q-spinner><span class="ellipsis">{{progress.what}}: {{progress.line || 'starting…'}}</span><q-space></q-space>
    <q-btn dense flat no-caps size="sm" label="Cancel" @click="cancelNetwork"></q-btn>
  </div>
  <div v-if="status && status.operation" class="row items-center no-wrap q-px-sm text-caption bg-orange-2 text-black" id="DevOperationBanner">
    A {{status.operation}} stopped at conflicts: resolve each file in Changes, then commit.
    <q-space></q-space><q-btn dense flat no-caps size="sm" label="Open the conflicts" @click="afterConflict"></q-btn>
  </div>
  <template v-if="gitLoaded && isGit && status">
    <m-dev-git-branches v-if="tab === 'branches'" :ws="this" :key="'b' + rootId"></m-dev-git-branches>
    <m-dev-git-tags v-if="tab === 'tags'" :ws="this" :key="'t' + rootId"></m-dev-git-tags>
    <m-dev-git-stashes v-if="tab === 'stashes'" :ws="this" :key="'s' + rootId"></m-dev-git-stashes>
    <m-dev-git-settings v-if="tab === 'settings'" :ws="this" :key="'g' + rootId"></m-dev-git-settings>
  </template>
  <q-separator></q-separator>
  <div class="col row no-wrap m-dev-body" v-show="tab === 'code'">
    <div class="m-dev-side column no-wrap" v-if="!guided">
      <div class="row no-wrap items-center q-px-xs">
        <q-btn-toggle dense flat no-caps v-model="rail" :options="[{ label: 'Files', value: 'files' }, { label: 'Search', value: 'search' }, { label: 'Tasks', value: 'tasks' }]"
            @update:model-value="$event === 'tasks' && loadTasks()" id="DevRail"></q-btn-toggle>
        <q-space></q-space>
        <q-btn dense flat round size="sm" icon="sym_o_note_add" :disable="!canWrite" @click="newFile(null)"><q-tooltip>New file</q-tooltip></q-btn>
        <q-btn dense flat round size="sm" icon="sym_o_create_new_folder" :disable="!canWrite" @click="newFolder(null)"><q-tooltip>New folder</q-tooltip></q-btn>
        <q-btn dense flat round size="sm" icon="sym_o_search" @click="openGoto"><q-tooltip>Go to file (Ctrl+P)</q-tooltip></q-btn>
        <q-btn dense flat round size="sm" icon="sym_o_data_object" @click="openArtifactGoto" id="DevGotoArtifact"><q-tooltip>Go to artifact (Ctrl+Shift+O)</q-tooltip></q-btn>
      </div>
      <div v-show="rail === 'files'" class="col scroll">
        <m-file-tree v-if="rootId" :key="rootId" :load="loadDir" :reload-key="treeKey" v-model:selected="selected" @open="openFile" @menu="treeMenu"></m-file-tree>
        <q-menu ref="treeMenu" context-menu touch-position>
          <q-list dense style="min-width: 10rem">
            <q-item clickable v-close-popup :disable="!canWrite" @click="newFile(menuTarget)"><q-item-section>New file</q-item-section></q-item>
            <q-item clickable v-close-popup :disable="!canWrite" @click="newFolder(menuTarget)"><q-item-section>New folder</q-item-section></q-item>
            <q-item clickable v-close-popup :disable="!canWrite" @click="renamePath(menuTarget)"><q-item-section>Rename or move</q-item-section></q-item>
            <q-item clickable v-close-popup :disable="!canWrite" @click="deletePath(menuTarget)"><q-item-section>Delete</q-item-section></q-item>
            <q-item clickable v-close-popup @click="copyPath(menuTarget)"><q-item-section>Copy path</q-item-section></q-item>
            <q-item v-if="isGit" clickable v-close-popup @click="history(menuTarget && menuTarget.path)"><q-item-section>History</q-item-section></q-item>
          </q-list>
        </q-menu>
      </div>
      <div v-show="rail === 'search'" class="col column no-wrap q-pa-xs">
        <q-input dense outlined v-model="search.query" placeholder="Search in files" @keyup.enter="runSearch" id="DevSearchQuery"></q-input>
        <div class="row items-center no-wrap">
          <q-checkbox dense size="xs" v-model="search.regex" label=".*"></q-checkbox>
          <q-checkbox dense size="xs" v-model="search.caseSensitive" label="Aa" class="q-ml-sm"></q-checkbox>
          <q-input dense borderless v-model="search.pathFilter" placeholder="in path" class="q-ml-sm col"></q-input>
        </div>
        <q-linear-progress v-if="search.running" indeterminate></q-linear-progress>
        <q-list dense class="col scroll" id="DevSearchResults">
          <q-item v-for="(hit, index) in search.results" :key="index" clickable @click="openFile(hit.path, hit.line)">
            <q-item-section><q-item-label caption>{{hit.path}}:{{hit.line}}</q-item-label><q-item-label class="ellipsis m-dev-mono">{{hit.text}}</q-item-label></q-item-section>
          </q-item>
          <q-item v-if="search.truncated"><q-item-section class="text-caption">The first 500 lines only</q-item-section></q-item>
        </q-list>
      </div>
      <div v-show="rail === 'tasks'" class="col column no-wrap q-pa-xs" id="DevTasks">
        <div class="row items-center no-wrap text-caption"><span class="ellipsis">{{taskList.error ? taskList.sourceLabel + ': ' + taskList.error : 'My tasks · ' + (taskList.sourceLabel || '')}}</span>
          <q-space></q-space><q-btn dense flat round size="xs" icon="refresh" @click="loadTasks"></q-btn>
          <q-btn dense flat round size="xs" icon="sym_o_settings" href="/qapps/dev/Tasks" type="a"><q-tooltip>Task source and app</q-tooltip></q-btn></div>
        <q-linear-progress v-if="taskList.loading" indeterminate></q-linear-progress>
        <q-list dense class="col scroll">
          <q-item v-for="task in taskList.tasks" :key="task.taskId" :data-task="task.taskId">
            <q-item-section><q-item-label class="ellipsis">{{task.title}}</q-item-label>
              <q-item-label caption>{{task.key}} · {{task.statusId}}{{task.projectName ? ' · ' + task.projectName : ''}}
                <q-icon v-if="status && status.currentTask === task.key" name="sym_o_star" size="xs"></q-icon></q-item-label></q-item-section>
            <q-item-section side><q-btn dense flat round size="xs" icon="more_vert"><q-menu><q-list dense style="min-width: 10rem">
              <q-item v-if="task.url" clickable v-close-popup :href="task.url" target="_blank" tag="a"><q-item-section>Open</q-item-section></q-item>
              <q-item clickable v-close-popup @click="copyText(task.key)"><q-item-section>Copy key</q-item-section></q-item>
              <q-item clickable v-close-popup :disable="!isGit" @click="useTask(task)"><q-item-section>Use in commit</q-item-section></q-item>
              <q-item clickable v-close-popup :disable="!isGit" @click="useTask(task, true)"><q-item-section>Set current</q-item-section></q-item>
              <q-item clickable v-close-popup :disable="!isGit || !canWrite" @click="taskBranch(task)"><q-item-section>Branch</q-item-section></q-item>
            </q-list></q-menu></q-btn></q-item-section>
          </q-item>
          <q-item v-if="!taskList.loading && !taskList.tasks.length && !taskList.error"><q-item-section class="text-caption m-muted">No open tasks.</q-item-section></q-item>
        </q-list>
      </div>
    </div>
    <div class="col column no-wrap m-dev-main">
      <div class="row no-wrap items-center m-dev-editor-tabs" v-if="!guided">
        <div v-for="item in tabs" :key="item.path" class="m-dev-editor-tab row no-wrap items-center" :class="{ active: item.path === active }"
            @click="active = item.path" :title="item.path" :data-path="item.path">
          <span class="ellipsis">{{item.path.split('/').pop()}}</span><span v-if="isDirtyAll(item)" class="m-dev-dirty">●</span>
          <q-icon v-if="editorsOf(item.path).length" name="sym_o_group" size="xs" class="q-ml-xs text-amber-8"><q-tooltip>being edited by {{editorsOf(item.path).join(', ')}}</q-tooltip></q-icon>
          <q-btn dense flat round size="xs" icon="close" @click.stop="closeTab(item.path)"></q-btn>
        </div>
      </div>
      <template v-if="current">
        <div class="m-dev-file-status row no-wrap items-center q-px-sm text-caption" id="DevFileStatus">
          <span class="ellipsis">{{root ? root.label : ''}} › {{current.path}}</span>
          <span v-if="current.status" class="m-file-status q-ml-sm" :class="'m-file-status-' + current.status">{{current.status}}</span>
          <span v-else-if="!isGit" class="q-ml-sm m-muted">Not versioned</span>
          <q-chip v-if="current.readOnly" dense square icon="sym_o_lock" class="q-ml-sm" id="DevReadOnly">{{current.readOnlyReason || 'read only'}}</q-chip>
          <q-chip v-if="editorsOf(current.path).length" dense square icon="sym_o_group" class="q-ml-sm" color="amber-3" text-color="black" id="DevEditedBy">being edited by {{editorsOf(current.path).join(', ')}}</q-chip>
          <q-space></q-space>
          <q-btn-toggle v-if="currentViews.length > 1" dense flat no-caps size="sm" :model-value="current.view" @update:model-value="setView(current, $event)"
              :options="currentViews.map(v => ({ label: v.label, value: v.name }))" id="DevViewToggle"></q-btn-toggle>
          <q-btn v-if="isGit && !guided" dense flat round size="sm" icon="more_vert" id="DevFileMenu"><q-menu><q-list dense style="min-width: 10rem">
            <q-item clickable v-close-popup @click="diffCurrent"><q-item-section>Diff</q-item-section></q-item>
            <q-item clickable v-close-popup @click="history(current.path)"><q-item-section>History</q-item-section></q-item>
            <q-item clickable v-close-popup @click="toggleBlame"><q-item-section>{{current.blame ? 'Hide blame' : 'Blame'}}</q-item-section></q-item>
            <q-item clickable v-close-popup :disable="!canWrite" @click="stageCurrent"><q-item-section>Stage</q-item-section></q-item>
            <q-item clickable v-close-popup :disable="!canWrite" @click="discardCurrent"><q-item-section>Discard</q-item-section></q-item>
            <q-item clickable v-close-popup :disable="!canWrite" @click="tab = 'changes'"><q-item-section>Commit…</q-item-section></q-item>
          </q-list></q-menu></q-btn>
          <q-btn dense flat no-caps size="sm" label="Save" icon="sym_o_save" :disable="!isDirtyAll(current) || (current.readOnly && !(current.companions || []).length)" :loading="current.saving" @click="save(current)" id="DevSave"></q-btn>
        </div>
        <q-banner v-if="current.changedOnDisk && !current.conflict" dense class="bg-orange-2 text-black" id="DevChangedOnDisk">Changed on disk
          <template v-slot:action>
            <q-btn dense flat no-caps label="Compare" @click="compareDisk(current)"></q-btn>
            <q-btn dense flat no-caps label="Reload" @click="useDisk(current)"></q-btn>
            <q-btn dense flat no-caps label="Keep mine" @click="current.changedOnDisk = false"></q-btn>
          </template>
        </q-banner>
        <div v-if="current.conflict" class="col column no-wrap" id="DevCompare">
          <q-banner dense class="bg-orange-2 text-black">The file on disk (left) changed since it was opened; your text is on the right.
            <template v-slot:action>
              <q-btn dense flat no-caps label="Save mine over it" @click="keepMine(current)"></q-btn>
              <q-btn dense flat no-caps label="Take the disk's" @click="takeDisk(current)"></q-btn>
            </template>
          </q-banner>
          <m-diff-view class="col" :original="current.conflict.diskText" :modified="current.text" :file-name="current.path" editable
              @update:modified="current.text = $event"></m-diff-view>
        </div>
        <div v-else-if="current.binary" class="q-pa-md m-muted">A binary file: it opens in the browser, not in the editor.</div>
        <template v-else-if="!current.loading">
          <!-- a visual view of the buffer; Text stays mounted beneath it, so the two share one undo history (5.2) -->
          <div v-if="currentSpec && viewsLoaded[current.view]" class="col column no-wrap m-dev-view" :data-view="current.view"
              @keydown.capture="onViewKey($event, current)">
            <component :is="currentSpec.component" class="col" :ctx="currentContext" :tab="current" :outline="current.outline"
                :key="rootId + ':' + current.path + ':' + current.view"></component>
            <div v-if="current.outlineError" class="text-caption text-negative q-px-sm" id="DevOutlineError">The text does not parse, so the view shows it as it last parsed: {{current.outlineError}}</div>
          </div>
          <m-code-editor v-show="!currentSpec" class="col" :key="rootId + ':' + current.path" v-model="current.text" :file-name="current.path"
              :state-key="rootId + ':' + current.path" :read-only="current.readOnly" :blame="current.blame || null" :schema="current.schema" :lint="lintOf(current)" height="100%"
              :highlight-lines="current.line ? [current.line] : []" @save="save(current)" @cursor="cursor = $event"
              :extensions="editorExtensions()" @ready="editorView = $event"></m-code-editor>
        </template>
        <div class="m-dev-problems" v-if="refs.title" id="DevReferences">
          <div class="row items-center no-wrap q-px-sm q-pt-xs"><span class="text-caption text-weight-medium">{{refs.title}} {{refs.loading ? '…' : refs.items.length}}</span>
            <q-space></q-space><q-btn dense flat round size="sm" icon="sym_o_close" @click="refs.title = ''"></q-btn></div>
          <q-list dense>
            <q-item v-for="(item, index) in refs.items" :key="index" clickable @click="openArtifact(item)">
              <q-item-section><q-item-label>{{item.label}}</q-item-label><q-item-label caption>{{item.caption}}</q-item-label></q-item-section>
            </q-item>
          </q-list>
        </div>
        <div class="m-dev-problems" v-if="problems.length" id="DevProblems">
          <div class="text-caption q-px-sm q-pt-xs text-weight-medium">Problems {{problems.length}}</div>
          <q-list dense>
            <q-item v-for="(problem, index) in problems" :key="index" class="m-dev-problem">
              <q-item-section avatar><q-icon size="16px" :name="problem.severity === 'error' ? 'sym_o_error' : 'sym_o_warning'" :color="problem.severity === 'error' ? 'negative' : 'warning'"></q-icon></q-item-section>
              <q-item-section><q-item-label>{{problem.line ? problem.line + ':' + problem.column + ' ' : ''}}{{problem.message}}</q-item-label>
                <q-item-label caption>{{problem.rule}}{{problem.skill ? ' · ' + problem.skill : ''}}</q-item-label></q-item-section>
            </q-item>
          </q-list>
        </div>
      </template>
      <div v-else class="col flex flex-center m-muted q-pa-md">Open a file from the tree, or Ctrl+P to go to one.</div>
      <div class="m-dev-status-bar row no-wrap items-center q-px-sm text-caption" id="DevStatusBar">
        <span v-if="current">Ln {{cursor.line}}, Col {{cursor.column}} · {{current.lineEnding || 'LF'}} · UTF-8</span>
        <span v-if="status && status.branch" class="q-ml-md">⎇ {{status.branch}}</span>
        <span v-if="problems.length" class="q-ml-md">{{problems.length}} problems</span>
        <q-space></q-space><span class="ellipsis" id="DevLastReload">{{lastReload}}</span>
      </div>
    </div>
  </div>
  <div class="col column no-wrap m-dev-body scroll q-pa-sm" v-if="tab === 'review' && review" id="DevReview">
    <div class="row items-center no-wrap q-gutter-x-sm q-mb-sm">
      <div class="text-subtitle2 ellipsis">Review: {{review.summary}}</div><q-space></q-space>
      <q-btn unelevated no-caps dense color="primary" label="Save accepted" :loading="review.saving" :disable="!canWrite" @click="saveReview" id="DevReviewSave"></q-btn>
      <q-btn flat no-caps dense label="Leave review" @click="leaveReview"></q-btn>
    </div>
    <div class="text-caption m-muted q-mb-sm">Accept or reject each change, edit the text as you like, then Save accepted: the files save as any save does, and Assist reads what you did.</div>
    <div v-for="file in review.files" :key="file.file" class="q-mb-md" :data-review-file="file.path">
      <div class="row items-center no-wrap text-caption text-weight-medium q-mb-xs">
        <span class="ellipsis">{{file.rootId}}:{{file.path}}</span><span v-if="file.new" class="q-ml-xs m-muted">(new)</span>
        <span v-if="file.changedOnDisk && !file.conflict" class="q-ml-xs text-warning">changed on disk since the proposal: merged onto it</span>
      </div>
      <div v-if="file.conflict" class="text-negative text-caption">{{file.conflict}}</div>
      <m-diff-view v-else :original="file.diskText || ''" :modified="file.proposedText" :file-name="file.path" unified editable merge-controls
          @update:modified="file.text = $event"></m-diff-view>
    </div>
  </div>
  <div class="col row no-wrap m-dev-body" v-if="tab === 'changes' && status">
    <div class="m-dev-side column no-wrap scroll q-pa-xs" id="DevChanges">
      <template v-for="(entries, group) in groups" :key="group">
        <div v-if="entries.length" class="row items-center no-wrap q-mt-sm">
          <span class="text-caption text-weight-medium text-capitalize">{{group}} {{entries.length}}</span><q-space></q-space>
          <q-btn v-if="group === 'staged'" dense flat no-caps size="sm" label="Unstage all" @click="unstage(entries)"></q-btn>
          <q-btn v-if="group === 'changes' || group === 'untracked'" dense flat no-caps size="sm" label="Stage all" @click="stage(entries)"></q-btn>
        </div>
        <div v-for="entry in entries" :key="group + entry.path" class="row items-center no-wrap m-dev-change" :data-path="entry.path">
          <span class="m-file-status" :class="'m-file-status-' + entry.letter">{{entry.letter}}</span>
          <a class="ellipsis q-ml-xs cursor-pointer" @click="showDiff(entry, group === 'staged')">{{entry.path}}</a><q-space></q-space>
          <q-btn v-if="group === 'staged'" dense flat round size="xs" icon="remove" @click="unstage([entry])"><q-tooltip>Unstage</q-tooltip></q-btn>
          <q-btn v-else-if="group !== 'conflicts'" dense flat round size="xs" icon="add" @click="stage([entry])"><q-tooltip>Stage</q-tooltip></q-btn>
          <q-btn v-if="group === 'changes' || group === 'untracked'" dense flat round size="xs" icon="undo" @click="discard([entry])"><q-tooltip>Discard</q-tooltip></q-btn>
          <q-btn dense flat round size="xs" icon="sym_o_open_in_new" @click="openFile(entry.path)"><q-tooltip>Open</q-tooltip></q-btn>
        </div>
      </template>
      <q-separator class="q-my-sm"></q-separator>
      <q-input dense outlined v-model="commitForm.summary" label="Summary" id="DevCommitSummary"></q-input>
      <q-input dense outlined type="textarea" autogrow v-model="commitForm.body" label="Body" class="q-mt-xs"></q-input>
      <div class="row items-center no-wrap q-mt-xs">
        <q-input dense outlined v-model="commitForm.task" label="Task (trailer)" class="col" id="DevCommitTask">
          <template v-slot:prepend><q-icon name="sym_o_task_alt" size="xs"></q-icon></template></q-input>
        <q-btn dense flat no-caps size="sm" :label="status.currentTask && status.currentTask === commitForm.task ? 'Current' : 'Set current'"
            :disable="status.currentTask === (commitForm.task || null)" @click="setCurrentTask" id="DevSetCurrentTask"></q-btn>
      </div>
      <div class="row q-gutter-x-sm"><q-checkbox dense size="xs" v-model="commitForm.amend" label="Amend" @update:model-value="amendChanged"></q-checkbox>
        <q-checkbox dense size="xs" v-model="commitForm.signOff" label="Sign off"></q-checkbox>
        <q-checkbox dense size="xs" v-model="commitForm.skipHooks" label="Skip hooks"></q-checkbox></div>
      <div class="text-caption m-muted q-mt-xs">Author {{status.authorName}} &lt;{{status.authorEmail}}&gt;; committer {{status.committerName || '(git user.name is not set)'}}</div>
      <div class="row q-gutter-x-sm q-mt-xs">
        <q-btn dense unelevated no-caps color="primary" label="Commit" :disable="!commitForm.summary || !groups.staged.length || !canWrite" :loading="busy === 'commit'" @click="commit(false)" id="DevCommit"></q-btn>
        <q-btn dense flat no-caps label="Commit and Push" :disable="!commitForm.summary || !groups.staged.length || !canWrite" @click="commit(true)"></q-btn>
      </div>
    </div>
    <m-dev-git-conflict v-if="conflictPath && gitLoaded" class="col" :ws="this" :path="conflictPath"></m-dev-git-conflict>
    <div v-else class="col column no-wrap">
      <template v-if="diff"><div class="text-caption q-px-sm q-py-xs">{{diff.path}}{{diff.staged ? ' (staged)' : ''}}</div>
        <div v-if="hunks && hunks.path === diff.path && hunks.list.length" class="row q-gutter-xs q-px-sm q-pb-xs" id="DevHunks">
          <q-chip v-for="(hunk, index) in hunks.list" :key="index" dense square :data-hunk="index">{{hunkLabel(hunk)}}
            <q-btn v-if="!hunks.staged" dense flat no-caps size="sm" label="Stage" :disable="!canWrite" @click="hunkWrite('stage', index)"></q-btn>
            <q-btn v-if="hunks.staged" dense flat no-caps size="sm" label="Unstage" :disable="!canWrite" @click="hunkWrite('unstage', index)"></q-btn>
            <q-btn v-if="!hunks.staged" dense flat no-caps size="sm" label="Discard" :disable="!canWrite" @click="hunkWrite('discard', index)"></q-btn>
          </q-chip>
        </div>
        <m-diff-view class="col" :original="diff.original" :modified="diff.modified" :file-name="diff.path"></m-diff-view></template>
      <div v-else class="col flex flex-center m-muted">Choose a file to see its change.</div>
    </div>
  </div>
  <div class="col row no-wrap m-dev-body" v-if="tab === 'commits'">
    <q-list dense class="m-dev-side scroll" id="DevCommits">
      <q-item v-if="logPath" dense><q-item-section class="text-caption">History of {{logPath}}</q-item-section>
        <q-item-section side><q-btn dense flat no-caps size="sm" label="All" @click="history('')"></q-btn></q-item-section></q-item>
      <q-item v-for="commit in log" :key="commit.id" clickable @click="openCommit(commit)" :active="commitDetail && commitDetail.id === commit.id">
        <q-item-section><q-item-label class="ellipsis">{{commit.subject}}</q-item-label>
          <q-item-label caption>{{commit.shortId}} · {{commit.authorName}} · {{commit.date.slice(0, 10)}}</q-item-label></q-item-section>
      </q-item>
      <q-item v-if="logMore" clickable @click="loadLog(true)" id="DevLogMore"><q-item-section class="text-caption">More commits…</q-item-section></q-item>
    </q-list>
    <div class="col column no-wrap" v-if="commitDetail">
      <div class="row items-center no-wrap q-gutter-x-xs q-px-sm q-pt-xs" id="DevCommitActions">
        <span class="text-caption m-dev-mono">{{String(commitDetail.id).slice(0, 12)}}</span><q-space></q-space>
        <q-btn dense flat no-caps size="sm" label="Copy id" @click="copyId"></q-btn>
        <q-btn dense flat no-caps size="sm" label="Browse files" @click="browseTree('')"></q-btn>
        <q-btn dense flat no-caps size="sm" label="Branch here" :disable="!canWrite" @click="newBranch(commitDetail.id)"></q-btn>
        <q-btn v-if="advanced" dense flat no-caps size="sm" label="Tag here" :disable="tier !== 'full'" @click="tagHere"></q-btn>
        <q-btn dense flat no-caps size="sm" label="Cherry-pick" :disable="tier !== 'full'" @click="replay('cherry-pick')"></q-btn>
        <q-btn dense flat no-caps size="sm" label="Revert" :disable="tier !== 'full'" @click="replay('revert')"></q-btn>
      </div>
      <div v-if="tree" class="col row no-wrap" id="DevCommitTree">
        <q-list dense class="m-dev-side scroll">
          <q-item v-if="tree.path" clickable @click="browseTree(tree.path.split('/').slice(0, -1).join('/'))"><q-item-section>..</q-item-section></q-item>
          <q-item v-for="entry in tree.entries" :key="entry.path" clickable @click="browseFile(entry)">
            <q-item-section avatar style="min-width: 0"><q-icon :name="entry.dir ? 'sym_o_folder' : 'sym_o_description'" size="xs"></q-icon></q-item-section>
            <q-item-section>{{entry.name}}</q-item-section></q-item>
        </q-list>
        <m-code-editor v-if="tree.file" class="col" :model-value="tree.file.text" :file-name="tree.file.path" read-only height="100%"></m-code-editor>
        <div v-else class="col flex flex-center m-muted">Files at {{String(commitDetail.id).slice(0, 9)}}<q-btn dense flat no-caps label="Close" @click="tree = null"></q-btn></div>
      </div>
      <pre v-if="!tree" class="q-ma-sm m-dev-mono" style="white-space: pre-wrap">{{commitDetail.message}}</pre>
      <q-list dense v-if="!tree"><q-item v-for="file in commitDetail.files" :key="file.path" clickable @click="commitDiff(file)">
        <q-item-section side class="m-file-status" :class="'m-file-status-' + file.status">{{file.status}}</q-item-section><q-item-section>{{file.path}}</q-item-section>
      </q-item></q-list>
      <m-diff-view v-if="diff && !tree" class="col" :original="diff.original" :modified="diff.modified" :file-name="diff.path"></m-diff-view>
    </div>
  </div>
  <q-dialog v-model="artifactGoto.open">
    <q-card style="width: 44rem; max-width: 95vw">
      <q-card-section class="q-pb-none"><q-input dense outlined autofocus v-model="artifactGoto.text" placeholder="Go to artifact: OrdHdr, place#Order"
          debounce="200" @update:model-value="searchArtifacts" @keyup.enter="gotoArtifact(artifactGoto.results[0])" id="DevGotoArtifactInput"></q-input></q-card-section>
      <q-list dense class="scroll" style="max-height: 50vh"><q-item v-for="artifact in artifactGoto.results" :key="artifact.id" clickable @click="gotoArtifact(artifact)">
        <q-item-section><q-item-label>{{artifact.shortName}} <span class="m-muted">{{artifact.kind}}</span></q-item-label>
          <q-item-label caption>{{artifact.component}}{{artifact.upstream ? '' : ' · site'}} · {{artifact.name}}</q-item-label></q-item-section></q-item></q-list>
    </q-card>
  </q-dialog>
  <q-dialog v-model="goto.open">
    <q-card style="width: 40rem; max-width: 95vw">
      <q-card-section class="q-pb-none"><q-input dense outlined autofocus v-model="goto.filter" placeholder="Go to file"
          @keyup.enter="gotoFile(gotoMatches[0])" id="DevGotoInput"></q-input></q-card-section>
      <q-list dense class="scroll" style="max-height: 50vh"><q-item v-for="path in gotoMatches" :key="path" clickable @click="gotoFile(path)">
        <q-item-section>{{path}}</q-item-section></q-item></q-list>
    </q-card>
  </q-dialog>
</div>`
    };

    devAide.components = { "m-dev-workspace": workspace };

    // Load the code editor widgets, then register the workspace on the running application.
    devAide.start = () => moqui.loadScript("/js/WebrootCode.js", null, () => !!(moqui.codeEditor && moqui.codeEditor.components))
        .then(() => {
            const qvt = moqui.qvt;
            if (qvt && qvt.registerLate) Object.keys(devAide.components).forEach((name) => qvt.registerLate(name, devAide.components[name]));
            if (root.document && !root.document.getElementById("m-dev-aide-style")) {
                const style = root.document.createElement("style");
                style.id = "m-dev-aide-style";
                style.textContent = ".m-dev-workspace{height:calc(100vh - 120px);min-height:28rem}" +
                    ".m-dev-side{width:280px;min-width:200px;border-right:1px solid var(--m-border,rgba(0,0,0,.12))}" +
                    ".m-dev-editor-tabs{overflow-x:auto;border-bottom:1px solid var(--m-border,rgba(0,0,0,.12))}" +
                    ".m-dev-editor-tab{padding:2px 4px 2px 10px;cursor:pointer;max-width:14rem;border-right:1px solid var(--m-border,rgba(0,0,0,.08))}" +
                    ".m-dev-editor-tab.active{background:var(--m-surface-2,rgba(127,127,127,.12))}.m-dev-dirty{margin-left:4px;color:#e2a400}" +
                    ".m-dev-status-bar,.m-dev-file-status{min-height:24px;border-top:1px solid var(--m-border,rgba(0,0,0,.12))}" +
                    ".m-dev-problems{max-height:30%;overflow:auto;border-top:1px solid var(--m-border,rgba(0,0,0,.12))}" +
                    ".m-dev-mono{font-family:ui-monospace,Menlo,Consolas,monospace;font-size:12px}" +
                    ".m-dev-change .m-file-status{width:1.2em;text-align:center}" +
                    ".m-dev-view{overflow:auto;min-height:0}.m-dev-guided .m-dev-main{width:100%}" +
                    ".m-dev-side .q-tree{padding-right:8px;overflow:hidden}.m-dev-status-bar>span{white-space:nowrap}" +
                    // below 1024 px it reads only: the tree above the file, the bar wrapping
                    "@media (max-width:1023px){.m-dev-body.row{flex-direction:column}.m-dev-workspace{height:auto}" +
                    ".m-dev-side{width:auto;max-height:40vh;border-right:none;border-bottom:1px solid var(--m-border,rgba(0,0,0,.12))}" +
                    ".m-dev-main{min-height:60vh}.m-dev-repo-bar{flex-wrap:wrap;row-gap:4px}.m-dev-repo-bar .q-select{min-width:0!important;flex:1 1 12rem}}";
                root.document.head.appendChild(style);
            }
        });

    if (typeof module !== "undefined" && module.exports) module.exports = devAide;
})(typeof window !== "undefined" ? window : globalThis);
