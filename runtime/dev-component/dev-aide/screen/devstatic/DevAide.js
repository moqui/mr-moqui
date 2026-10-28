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
    pure.closeTab = (tabs, path, active) => {
        const index = tabs.findIndex((tab) => tab.path === path);
        if (index < 0) return { tabs, active };
        const rest = tabs.slice(0, index).concat(tabs.slice(index + 1));
        const next = active !== path ? active : (rest[index] || rest[index - 1] || {}).path || null;
        return { tabs: rest, active: next };
    };
    pure.isDirty = (tab) => !!tab && tab.text !== tab.saved;

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

    // ---- the browser ----

    const store = {
        get(key) { try { const raw = root.localStorage && root.localStorage.getItem(key); return raw ? JSON.parse(raw) : null; } catch (err) { return null; } },
        set(key, value) { try { if (root.localStorage) root.localStorage.setItem(key, JSON.stringify(value)); } catch (err) { /* no storage */ } },
        drop(key) { try { if (root.localStorage) root.localStorage.removeItem(key); } catch (err) { /* no storage */ } }
    };

    const workspace = {
        name: "mDevWorkspace",
        props: { base: { type: String, required: true }, query: Object },
        data() {
            return {
                roots: [], rootId: null, tier: "view", gitAvailable: false, upstreamEdits: false, shared: false,
                status: null, tab: "code", rail: "files", treeKey: 0, selected: null,
                tabs: [], active: null, cursor: { line: 1, column: 1 }, lastReload: "", restartNeeded: false,
                search: { query: "", regex: false, caseSensitive: false, pathFilter: "", results: [], running: false, truncated: false },
                diff: null, commitForm: { summary: "", body: "", amend: false, signOff: false, skipHooks: false, task: "" },
                log: [], commitDetail: null, goto: { open: false, filter: "", files: [] }, busy: "", schemas: {},
                refs: { title: "", items: [], loading: false }, artifactGoto: { open: false, text: "", results: [] },
                narrow: !!(root.innerWidth && root.innerWidth < 1024), offReload: null, reloadTimer: null
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
            canWrite() { return this.tier !== "view" && !this.narrow; }
        },
        mounted() {
            const client = this.$root && this.$root.notificationClient;
            if (client && client.registerListener) this.offReload = client.registerListener("DevReload", (message) => this.onReload(message));
            this.onResize = () => { this.narrow = root.innerWidth < 1024; };
            root.addEventListener("resize", this.onResize);
            this.loadRoots();
        },
        beforeUnmount() {
            if (this.offReload) this.offReload();
            root.removeEventListener("resize", this.onResize);
        },
        watch: {
            tabs: { deep: true, handler() { this.remember(); } },
            active() { this.remember(); }
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
                    if (this.query && this.query.path) this.openFile(this.query.path, Number(this.query.line) || 0);
                }, this.fail);
            },
            selectRoot(id) {
                if (!id) return;
                this.rootId = id;
                this.tabs = []; this.active = null; this.status = null; this.diff = null; this.log = []; this.commitDetail = null;
                this.treeKey++;
                const kept = store.get(pure.storageKey(id, "tabs"));
                if (kept && Array.isArray(kept.paths)) {
                    kept.paths.forEach((path) => this.openFile(path, 0, true));
                    this.active = kept.active || null;
                }
                this.refreshStatus();
            },
            remember() {
                if (!this.rootId) return;
                store.set(pure.storageKey(this.rootId, "tabs"), { paths: this.tabs.map((tab) => tab.path), active: this.active });
                for (const tab of this.tabs) {
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
            openFile(path, line, quiet) {
                if (!path) return;
                const opened = pure.openTab(this.tabs, { path, text: "", saved: "", base: "", loading: true, problems: [], line: line || 0 });
                this.tabs = opened.tabs;
                if (!quiet) this.active = opened.active;
                this.tab = "code";
                const tab = this.tabs.find((item) => item.path === path);
                if (!tab.loading) { if (line) tab.line = line; return; }
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
                }, (error) => { this.tabs = pure.closeTab(this.tabs, path, this.active).tabs; this.fail(error); });
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
                const close = () => { const next = pure.closeTab(this.tabs, path, this.active); this.tabs = next.tabs; this.active = next.active; };
                if (pure.isDirty(tab)) {
                    this.$q.dialog({ title: "Close " + path + "?", message: "Its changes are not saved.", cancel: true, ok: { label: "Close", color: "negative" } }).onOk(close);
                } else close();
            },
            lintOf(tab) {
                if (!/\.xml$/i.test(tab.path)) return null;
                return (text) => this.post("check", { rootId: this.rootId, path: tab.path, text }).then((answer) => answer.problems || []);
            },
            save(tab) {
                tab = tab || this.current;
                if (!tab || tab.readOnly || tab.saving) return;
                tab.saving = true;
                this.post("save", { rootId: this.rootId, path: tab.path, text: tab.text, baseHash: tab.base }).then((answer) => {
                    tab.saving = false;
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
                }, (error) => { tab.saving = false; this.fail(error); });
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
                this.get("diff", { rootId: this.rootId, path: entry.path, staged: !!staged }).then((answer) => {
                    this.diff = { path: entry.path, staged: !!staged, original: answer.original || "", modified: answer.modified || "" };
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
            network(what) {
                this.busy = what;
                this.post(what, { rootId: this.rootId }).then((answer) => {
                    this.busy = "";
                    this.note((what.charAt(0).toUpperCase() + what.slice(1)) + " done" + (answer.output ? ": " + answer.output.trim().split("\n").pop() : ""));
                    // an upstream change that removed what the site extends: the upgrade checklist
                    if (answer.extensionsAffected) this.$q.notify({ type: "warning", timeout: 0, closeBtn: "Close",
                        message: "Extensions affected: " + answer.extensionsAffected, actions: [{ label: "Customizations", color: "white",
                            handler: () => { window.location.href = "/qapps/dev/Artifacts/Customizations"; } }] });
                    this.refreshStatus(); this.treeKey++;
                }, this.fail);
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
            newBranch() {
                this.ask("New branch from the current one", "").then((name) => {
                    if (name) this.post("createBranch", { rootId: this.rootId, branch: name }).then(() => { this.note("On " + name); this.refreshStatus(); }, this.fail);
                });
            },
            loadLog() {
                this.get("log", { rootId: this.rootId, pageSize: 100 }).then((answer) => { this.log = answer.commits || []; }, this.fail);
            },
            openCommit(commit) {
                this.get("commitInfo", { rootId: this.rootId, commitId: commit.id }).then((answer) => {
                    this.commitDetail = Object.assign({ id: commit.id }, answer); this.diff = null;
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
            onKey(event) {
                const mod = event.ctrlKey || event.metaKey;
                if (mod && event.shiftKey && event.key.toLowerCase() === "o") { event.preventDefault(); this.openArtifactGoto(); }
                else if (mod && event.key.toLowerCase() === "p") { event.preventDefault(); this.openGoto(); }
                else if (mod && event.shiftKey && event.key.toLowerCase() === "f") { event.preventDefault(); this.tab = "code"; this.rail = "search"; }
            }
        },
        template: `
<div class="m-dev-workspace column no-wrap" id="DevWorkspace" tabindex="-1" @keydown="onKey">
  <div class="m-dev-repo-bar row items-center no-wrap q-gutter-x-sm q-px-sm q-py-xs">
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
          <q-item clickable v-close-popup @click="newBranch"><q-item-section>New branch…</q-item-section></q-item>
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
  <q-tabs dense align="left" no-caps v-model="tab" class="m-dev-tabs">
    <q-tab name="code" label="Code"></q-tab>
    <q-tab name="changes" :label="'Changes' + (changeCount ? ' ' + changeCount : '')" :disable="!isGit"></q-tab>
    <q-tab name="commits" label="Commits" :disable="!isGit" @click="loadLog"></q-tab>
  </q-tabs>
  <q-separator></q-separator>
  <div class="col row no-wrap m-dev-body" v-show="tab === 'code'">
    <div class="m-dev-side column no-wrap">
      <div class="row no-wrap items-center q-px-xs">
        <q-btn-toggle dense flat no-caps v-model="rail" :options="[{ label: 'Files', value: 'files' }, { label: 'Search', value: 'search' }]"></q-btn-toggle>
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
    </div>
    <div class="col column no-wrap m-dev-main">
      <div class="row no-wrap items-center m-dev-editor-tabs">
        <div v-for="item in tabs" :key="item.path" class="m-dev-editor-tab row no-wrap items-center" :class="{ active: item.path === active }"
            @click="active = item.path" :title="item.path" :data-path="item.path">
          <span class="ellipsis">{{item.path.split('/').pop()}}</span><span v-if="item.text !== item.saved" class="m-dev-dirty">●</span>
          <q-btn dense flat round size="xs" icon="close" @click.stop="closeTab(item.path)"></q-btn>
        </div>
      </div>
      <template v-if="current">
        <div class="m-dev-file-status row no-wrap items-center q-px-sm text-caption" id="DevFileStatus">
          <span class="ellipsis">{{root ? root.label : ''}} › {{current.path}}</span>
          <span v-if="current.status" class="m-file-status q-ml-sm" :class="'m-file-status-' + current.status">{{current.status}}</span>
          <span v-else-if="!isGit" class="q-ml-sm m-muted">Not versioned</span>
          <q-chip v-if="current.readOnly" dense square icon="sym_o_lock" class="q-ml-sm" id="DevReadOnly">{{current.readOnlyReason || 'read only'}}</q-chip>
          <q-space></q-space>
          <q-btn dense flat no-caps size="sm" label="Save" icon="sym_o_save" :disable="current.readOnly || current.text === current.saved" :loading="current.saving" @click="save(current)" id="DevSave"></q-btn>
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
        <m-code-editor v-else-if="!current.loading" class="col" :key="rootId + ':' + current.path" v-model="current.text" :file-name="current.path"
            :read-only="current.readOnly" :schema="current.schema" :lint="lintOf(current)" height="100%"
            :highlight-lines="current.line ? [current.line] : []" @save="save(current)" @cursor="cursor = $event"
            :extensions="editorExtensions()"></m-code-editor>
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
      <q-input dense outlined v-model="commitForm.task" label="Task (trailer)" class="q-mt-xs"></q-input>
      <div class="row q-gutter-x-sm"><q-checkbox dense size="xs" v-model="commitForm.amend" label="Amend"></q-checkbox>
        <q-checkbox dense size="xs" v-model="commitForm.signOff" label="Sign off"></q-checkbox>
        <q-checkbox dense size="xs" v-model="commitForm.skipHooks" label="Skip hooks"></q-checkbox></div>
      <div class="text-caption m-muted q-mt-xs">Author {{status.authorName}} &lt;{{status.authorEmail}}&gt;; committer {{status.committerName || '(git user.name is not set)'}}</div>
      <div class="row q-gutter-x-sm q-mt-xs">
        <q-btn dense unelevated no-caps color="primary" label="Commit" :disable="!commitForm.summary || !groups.staged.length || !canWrite" :loading="busy === 'commit'" @click="commit(false)" id="DevCommit"></q-btn>
        <q-btn dense flat no-caps label="Commit and Push" :disable="!commitForm.summary || !groups.staged.length || !canWrite" @click="commit(true)"></q-btn>
      </div>
    </div>
    <div class="col column no-wrap">
      <template v-if="diff"><div class="text-caption q-px-sm q-py-xs">{{diff.path}}{{diff.staged ? ' (staged)' : ''}}</div>
        <m-diff-view class="col" :original="diff.original" :modified="diff.modified" :file-name="diff.path"></m-diff-view></template>
      <div v-else class="col flex flex-center m-muted">Choose a file to see its change.</div>
    </div>
  </div>
  <div class="col row no-wrap m-dev-body" v-if="tab === 'commits'">
    <q-list dense class="m-dev-side scroll" id="DevCommits">
      <q-item v-for="commit in log" :key="commit.id" clickable @click="openCommit(commit)" :active="commitDetail && commitDetail.id === commit.id">
        <q-item-section><q-item-label class="ellipsis">{{commit.subject}}</q-item-label>
          <q-item-label caption>{{commit.shortId}} · {{commit.authorName}} · {{commit.date.slice(0, 10)}}</q-item-label></q-item-section>
      </q-item>
    </q-list>
    <div class="col column no-wrap" v-if="commitDetail">
      <pre class="q-ma-sm m-dev-mono" style="white-space: pre-wrap">{{commitDetail.message}}</pre>
      <q-list dense><q-item v-for="file in commitDetail.files" :key="file.path" clickable @click="commitDiff(file)">
        <q-item-section side class="m-file-status" :class="'m-file-status-' + file.status">{{file.status}}</q-item-section><q-item-section>{{file.path}}</q-item-section>
      </q-item></q-list>
      <m-diff-view v-if="diff" class="col" :original="diff.original" :modified="diff.modified" :file-name="diff.path"></m-diff-view>
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
