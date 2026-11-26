/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The Workspace client's pure parts in the page. pure is moqui.devAide.pure from DevAide.js.
"use strict";

const pure = moqui.devAide.pure;

test("tabs open once, and a close moves to the neighbour", () => {
    let state = pure.openTab([], { path: "a.xml" });
    state = pure.openTab(state.tabs, { path: "b.xml" });
    state = pure.openTab(state.tabs, { path: "a.xml" });
    assert.deepEqual(state.tabs.map((tab) => tab.path), ["a.xml", "b.xml"]);
    assert.equal(state.active, "a.xml");
    const closed = pure.closeTab(state.tabs, "a.xml", "a.xml");
    assert.deepEqual(closed, { tabs: [{ path: "b.xml" }], active: "b.xml" });
    assert.equal(pure.closeTab(closed.tabs, "b.xml", "b.xml").active, null);
    assert.equal(pure.closeTab(state.tabs, "b.xml", "a.xml").active, "a.xml");
    assert.equal(pure.isDirty({ text: "x", saved: "y" }), true);
    assert.equal(pure.isDirty({ text: "x", saved: "x" }), false);
});

test("changes group as the Changes tab shows them", () => {
    const groups = pure.groupChanges([
        { path: "a", staged: true, unstaged: false },
        { path: "b", staged: true, unstaged: true },
        { path: "c", staged: false, unstaged: true },
        { path: "d", untracked: true },
        { path: "e", conflicted: true }
    ]);
    assert.deepEqual(Object.fromEntries(Object.entries(groups).map(([key, list]) => [key, list.map((entry) => entry.path)])),
        { conflicts: ["e"], staged: ["a", "b"], changes: ["b", "c"], untracked: ["d"] });
});

test("the files of a reload report in a root", () => {
    const report = { files: [{ path: "/w/site/service/A.xml" }, { path: "/w/other/x.xml" }, { path: "C:\\w\\site\\b.xml" }] };
    assert.deepEqual(pure.reloadedPaths(report, "/w/site"), ["service/A.xml"]);
    assert.deepEqual(pure.reloadedPaths(report, "/w/site/"), ["service/A.xml"]);
    assert.deepEqual(pure.reloadedPaths(report, "C:\\w\\site"), ["b.xml"]);
    assert.deepEqual(pure.reloadedPaths(null, "/w"), []);
});

test("the root element of an XML text", () => {
    assert.equal(pure.rootElement("<?xml version=\"1.0\"?>\n<!-- <x/> -->\n<services xmlns:xsi=\"x\">"), "services");
    assert.equal(pure.rootElement("<screen-extend/>"), "screen-extend");
    assert.equal(pure.rootElement("no xml"), null);
});

test("error text from a transition's answer", () => {
    assert.equal(pure.errorText({ json: { errors: ["a", "b"] } }), "a; b");
    assert.equal(pure.errorText({ json: { errors: "one" } }), "one");
    assert.equal(pure.errorText({ status: 500, statusText: "Server Error" }), "HTTP 500 Server Error");
    assert.equal(pure.errorText(new Error("boom")), "boom");
    // a save's conflict, thrown in the client
    assert.equal(pure.errorText({ errors: "A.xml changed on disk" }), "A.xml changed on disk");
    assert.equal(pure.storageKey("site", "tabs"), "moqui.devAide.tabs.site");
});

test("go to definition takes the attribute value under the cursor", () => {
    const line = '        <entity-find entity-name="moqui.example.Example" list="examples">';
    assert.equal(pure.valueAt(line, line.indexOf("example.Example") + 3), "moqui.example.Example");
    assert.equal(pure.valueAt(line, line.indexOf("list") + 1), "list");
    assert.equal(pure.valueAt("ec.entity.find(x) place#Order y", 22), "place#Order");
});

test("a review counts the hunks of a proposal and those kept", () => {
    const disk = "a\nb\nc\nd\ne\n";
    const proposed = "a\nB\nc\nd\nE\n";
    assert.equal(pure.hunks(disk, proposed), 2);
    assert.equal(pure.hunks(proposed, proposed), 0);
    // one hunk rejected: the text has the disk's line there, one hunk short of the proposal
    assert.equal(pure.hunks("a\nb\nc\nd\nE\n", proposed), 1);
    assert.equal(pure.hunks("", "x\n"), 1);
});

test("the views of a file follow its root element", () => {
    const specs = [{ name: "entity", label: "Entity", roots: ["entities"] }, { name: "screen", label: "Screen", roots: ["screen", "screen-extend"] }];
    assert.deepEqual(pure.viewsFor(specs, "entity/A.xml", "entities").map((view) => view.name), ["text", "entity"]);
    assert.deepEqual(pure.viewsFor(specs, "screen-extend/x/B.xml", "screen-extend").map((view) => view.name), ["text", "screen"]);
    assert.deepEqual(pure.viewsFor(specs, "service/A.xml", "services").map((view) => view.name), ["text"]);
    assert.deepEqual(pure.viewsFor(specs, "README.md", null).map((view) => view.name), ["text"]);
});

test("Save all writes entities, services, rules, screens, then the rest", () => {
    const buffers = [
        { path: "data/SeedData.xml", text: "a", saved: "" },
        { path: "screen-extend/x/A.xml", text: "a", saved: "" },
        { path: "service/Site.secas.xml", text: "a", saved: "" },
        { path: "entity/SiteEntities.xml", text: "a", saved: "" },
        { path: "service/x/Services.xml", text: "a", saved: "" },
        { path: "entity/Clean.xml", text: "a", saved: "a" }
    ];
    assert.deepEqual(pure.saveOrder(buffers).map((buffer) => buffer.path), ["entity/SiteEntities.xml", "service/x/Services.xml",
        "service/Site.secas.xml", "screen-extend/x/A.xml", "data/SeedData.xml"]);
    assert.equal(pure.isDirtyAll({ text: "a", saved: "a", companions: [{ text: "b", saved: "" }] }), true);
    assert.equal(pure.isDirtyAll({ text: "a", saved: "a", companions: [] }), false);
});

test("the restart banner follows a rebuild and offers Rebuild only where it can run", () => {
    const needed = { needed: true, available: true, state: "" };
    assert.equal(pure.rebuildNote(needed), "A Rust change needs a rebuild first.");
    assert.equal(pure.rebuildOffered(needed), true);
    assert.equal(pure.rebuildNote({ needed: true, available: false }), "A Rust change needs a rebuild first: ./moq build.");
    assert.equal(pure.rebuildOffered({ needed: true, available: false }), false);
    assert.equal(pure.rebuildOffered({ ...needed, state: "running" }), false);
    assert.equal(pure.rebuildNote({ ...needed, state: "running" }), "Building in the Terminal (./moq build)…");
    assert.equal(pure.rebuildNote({ ...needed, state: "succeeded" }), "The build succeeded: restart to run it.");
    assert.equal(pure.rebuildNote({ ...needed, state: "failed", exitStatus: 101 }),
        "The build failed (status 101): its output is in the Terminal.");
    assert.equal(pure.rebuildOffered({ ...needed, state: "failed" }), true);
    assert.equal(pure.rebuildNote({ needed: false }), "");
});

test("a selection follows to its element in the outline an edit answers", () => {
    const before = { name: "widgets", path: [], children: [{ name: "container-dialog", path: [0], attrs: [["button-text", "New item"]] }] };
    const after = { name: "widgets", path: [], children: [{ name: "container-dialog", path: [0], attrs: [["button-text", "Add item"]] }] };
    const moved = { name: "widgets", path: [], children: [{ name: "label", path: [0], attrs: [] }] };
    assert.equal(pure.attr(pure.sameNodeIn(after, before.children[0]), "button-text"), "Add item");
    assert.equal(pure.sameNodeIn(moved, before.children[0]), null);
    assert.equal(pure.sameNodeIn(after, null), null);
});

test("an outline is walked and read by path", () => {
    const outline = { name: "entities", path: [], attrs: [], children: [
        { name: "entity", path: [0], attrs: [["entity-name", "A"]], children: [{ name: "field", path: [0, 0], attrs: [["name", "x"]] }] }] };
    assert.equal(pure.outlineAt(outline, [0, 0]).name, "field");
    assert.equal(pure.outlineAt(outline, [3]), null);
    assert.equal(pure.attr(pure.outlineAt(outline, [0]), "entity-name"), "A");
    const seen = [];
    pure.walkOutline(outline, (node, depth) => seen.push(node.name + depth));
    assert.deepEqual(seen, ["entities0", "entity1", "field2"]);
    assert.equal(pure.workspaceBase("/qapps/dev/Entities/Edit"), "/qapps/dev/Workspace");
    assert.equal(pure.workspaceBase("/qapps/dev/Workspace"), "/qapps/dev/Workspace");
});

test("a shared server's editors of a file, and a notification's payload", () => {
    const editors = [{ userId: "U1", username: "jane", paths: ["a.xml", "b.xml"] }, { userId: "U2", username: "raj", paths: ["b.xml"] }];
    assert.deepEqual(pure.editorsOf(editors, "b.xml"), ["jane", "raj"]);
    assert.deepEqual(pure.editorsOf(editors, "c.xml"), []);
    assert.deepEqual(pure.editorsOf(null, "a.xml"), []);
    assert.deepEqual(pure.messageJson({ message: '{"rootId":"r"}' }), { rootId: "r" });
    assert.deepEqual(pure.messageJson({ rootId: "r" }), { rootId: "r" });
    assert.equal(pure.messageJson({ message: "not json" }), null);
});

test("a view offered by a file's extension, as a .qvue's Design", () => {
    const specs = [{ name: "qvue", label: "Design", extensions: ["qvue"] }, { name: "screen", label: "Screen", roots: ["screen"] }];
    assert.deepEqual(pure.viewsFor(specs, "screen/A/Board.qvue", null).map((view) => view.name), ["text", "qvue"]);
    assert.deepEqual(pure.viewsFor(specs, "screen/A/Board.xml", "screen").map((view) => view.name), ["text", "screen"]);
});

test("a doc's link to another doc is its path after the Docs screen's", () => {
    assert.equal(pure.docPathOfLink("/qapps/dev/Artifacts/Doc/entity/mantle/order/OrderItem.md"), "entity/mantle/order/OrderItem.md");
    assert.equal(pure.docPathOfLink("/qapps/dev/Artifacts/Doc/service/a/B/place-Order.md#md-uses"), "service/a/B/place-Order.md");
    assert.equal(pure.docPathOfLink("/qapps/dev/Artifacts/Doc/screen/My%20Dir/A.md?x=1"), "screen/My Dir/A.md");
    assert.equal(pure.docPathOfLink("/qapps/dev/Artifacts/Doc/entity/a/"), null, "a directory is not a doc");
    assert.equal(pure.docPathOfLink("/qapps/dev/Workspace?path=a.md"), null);
    assert.equal(pure.docPathOfLink(""), null);
});

test("the artifact doc component is registered with the workspace", () => {
    const comp = moqui.devAide.components["m-dev-artifact-doc"];
    assert.ok(comp && comp.template.indexOf("copyable") > 0, "its markdown has the copy button");
    assert.deepEqual(Object.keys(comp.props).sort(), ["artifactId", "base", "docPath", "stamp"]);
});

test("Go to dialogs keep empty or loading searches open and open a selected result", () => {
    const methods = moqui.devAide.components["m-dev-workspace"].methods;
    const opened = [];
    const ws = { goto: { open: true, loading: false }, artifactGoto: { open: true, loading: false },
        openFile: path => opened.push(path), openArtifact: artifact => opened.push(artifact.id) };
    methods.gotoFile.call(ws, undefined); methods.gotoArtifact.call(ws, undefined);
    assert.equal(ws.goto.open, true); assert.equal(ws.artifactGoto.open, true);
    ws.goto.loading = true; ws.artifactGoto.loading = true;
    methods.gotoFile.call(ws, "README.md"); methods.gotoArtifact.call(ws, { id: "entity:Example" });
    assert.deepEqual(opened, []);
    ws.goto.loading = false; ws.artifactGoto.loading = false;
    methods.gotoFile.call(ws, "README.md"); methods.gotoArtifact.call(ws, { id: "entity:Example" });
    assert.deepEqual(opened, ["README.md", "entity:Example"]);
    assert.equal(ws.goto.open, false); assert.equal(ws.artifactGoto.open, false);
});

test("artifact search ignores stale answers and keeps failures available for retry", async () => {
    const methods = moqui.devAide.components["m-dev-workspace"].methods, pending = [];
    const ws = { artifactGoto: { open: true, text: "User", results: [], loading: false, error: "" },
        get: () => new Promise((resolve, reject) => pending.push({ resolve, reject })) };
    const first = methods.searchArtifacts.call(ws);
    ws.artifactGoto.text = "Service";
    const second = methods.searchArtifacts.call(ws);
    pending[0].resolve({ artifacts: [{ id: "entity:User" }] }); await first;
    assert.deepEqual(ws.artifactGoto.results, []); assert.equal(ws.artifactGoto.loading, true);
    pending[1].reject(new Error("Search is unavailable")); await second;
    assert.equal(ws.artifactGoto.open, true); assert.equal(ws.artifactGoto.loading, false);
    assert.equal(ws.artifactGoto.error, "Search is unavailable");
    const retry = methods.searchArtifacts.call(ws);
    pending[2].resolve({ artifacts: [{ id: "service:Example" }] }); await retry;
    assert.equal(ws.artifactGoto.error, "");
    assert.equal(ws.artifactGoto.results[0].id, "service:Example");
});

test("the Libraries panel reviews the chosen release and refreshes the owning component after apply", async () => {
    const panel = moqui.devAide.components["m-dev-libraries"];
    const calls = [];
    const added = {id:"sum", entry:"math/add", targets:["server","client"]};
    let libraries = [];
    const vm = Object.assign(panel.data(), panel.methods, {rootId:"site", path:"screen-extend/Page.xml", attachable:true});
    vm.ctx = {
        get: async (_, params) => {
            assert.equal(params.path, "screen-extend/Page.xml");
            return {result:{component:"site", libraries:libraries.slice(), canManage:true}};
        },
        post: async (_, args) => {
            calls.push(args);
            // newest first: a cooled prerelease and a deprecated release are passed over, as is one within the cooldown
            if (args.operation === "versions") return {result:[
                {version:"2.1.0-beta.1", cooled:true, allowed:true, prerelease:true},
                {version:"2.0.0", cooled:false, allowed:true}, {version:"1.1.0", cooled:true, allowed:true, deprecated:true},
                {version:"1.0.0", cooled:true, allowed:true}]};
            if (args.operation === "prepare") return {result:{token:"review", changes:[], packages:[]}};
            if (args.operation === "apply") libraries.push(added);
            return {result:{applied:true}};
        },
        note() {}
    };
    await vm.show();
    assert.deepEqual(vm.catalog.libraries, []);
    await vm.pick("math");
    vm.id = "sum"; vm.entry = "math/add"; vm.target = "both";
    await vm.prepare();
    assert.deepEqual(calls[1], {rootId:"site", path:"screen-extend/Page.xml", operation:"prepare",
        entry:"math/add", libraryId:"sum", version:"1.0.0", target:"both"});
    assert.equal(vm.review.token, "review");
    assert.deepEqual(vm.catalog.libraries, []);
    await vm.apply();
    assert.equal(calls[2].token, "review");
    assert.deepEqual(vm.catalog.libraries, [added]);
    assert.equal(vm.moduleCall("sum"), "ec.resource.script('component://site/script/libs/sum.js')");
});

// a fresh IndexedDB of a test, dropped after
const dropDatabase = (name) => new Promise((resolve) => { const request = indexedDB.deleteDatabase(name); request.onsuccess = request.onerror = request.onblocked = resolve; });

test("an earlier version's drafts in localStorage move to IndexedDB once, and Assist's hand-off stays", async () => {
    assert.deepEqual(pure.draftKeysToMove(["moqui.devAide.draft:service/A.xml.site", "moqui.devAide.draft:component://site/A.xml",
        "moqui.devAide.tabs.site", null]), ["moqui.devAide.draft:service/A.xml.site"]);
    const name = "moqui.devAide.test." + Date.now();
    const draftKey = pure.storageKey("site", "draft:service/A.xml");
    const handOff = "moqui.devAide.draft:component://site/service/A.xml";
    localStorage.setItem(draftKey, JSON.stringify({ base: "h1", text: "<services/>\n" }));
    localStorage.setItem(handOff, JSON.stringify({ text: "proposed" }));
    localStorage.setItem("moqui.devAide.tabs.site", JSON.stringify({ paths: ["service/A.xml"] }));
    try {
        const drafts = moqui.devAide.makeDrafts(name);
        assert.equal(await drafts.ready(), true);
        assert.deepEqual(await drafts.get(draftKey), { base: "h1", text: "<services/>\n" });
        assert.equal(localStorage.getItem(draftKey), null, "moved, not copied");
        assert.notEqual(localStorage.getItem(handOff), null, "Assist's hand-off is the builder's to take");
        assert.notEqual(localStorage.getItem("moqui.devAide.tabs.site"), null, "the open tabs stay in localStorage");
        // the same API: set and drop answer whether they were written, get answers null for none
        assert.equal(await drafts.set("moqui.devAide.draft:B.xml.site", { base: "h2", text: "b" }), true);
        const written = await drafts.get("moqui.devAide.draft:B.xml.site");
        assert.equal(written.text, "b");
        assert.ok(written.at > 0, "each draft has its time");
        assert.equal(await drafts.drop("moqui.devAide.draft:B.xml.site"), true);
        assert.equal(await drafts.get("moqui.devAide.draft:B.xml.site"), null);
        // once: a second opening finds nothing to move and keeps what IndexedDB has
        await drafts.close();
        const again = moqui.devAide.makeDrafts(name);
        assert.equal((await again.get(draftKey)).text, "<services/>\n");
        await again.close();
    } finally {
        [draftKey, handOff, "moqui.devAide.tabs.site"].forEach((key) => localStorage.removeItem(key));
        await dropDatabase(name);
    }
});

test("pagehide's copy in localStorage is taken in only over an older draft", async () => {
    assert.equal(pure.draftNewer({ at: 2 }, null), true);
    assert.equal(pure.draftNewer({ at: 2 }, { at: 1 }), true);
    assert.equal(pure.draftNewer({ at: 1 }, { at: 2 }), false);
    assert.equal(pure.draftNewer({}, { at: 1 }), false, "a draft without a time is the oldest");
    const name = "moqui.devAide.test.newer." + Date.now();
    const older = pure.storageKey("site", "draft:Older.xml"), newer = pure.storageKey("site", "draft:Newer.xml");
    try {
        const first = moqui.devAide.makeDrafts(name);
        await first.set(older, { base: "b", text: "kept", at: 2000 });
        await first.set(newer, { base: "b", text: "replaced", at: 1000 });
        await first.close();
        // pagehide's synchronous copies: one older than IndexedDB's, one newer
        assert.equal(first.setNow(older, { base: "b", text: "stale", at: 1500 }), true);
        first.setNow(newer, { base: "b", text: "newest", at: 3000 });
        const second = moqui.devAide.makeDrafts(name);
        assert.equal((await second.get(older)).text, "kept");
        assert.equal((await second.get(newer)).text, "newest");
        assert.equal(localStorage.getItem(older), null);
        assert.equal(localStorage.getItem(newer), null);
        await second.close();
    } finally {
        [older, newer].forEach((key) => localStorage.removeItem(key));
        await dropDatabase(name);
    }
});

test("with no IndexedDB the drafts stay in localStorage, and a move that fails leaves them there", async () => {
    const key = pure.storageKey("site", "draft:Local.xml");
    try {
        const none = moqui.devAide.makeDrafts("moqui.devAide.test.none", null);
        assert.equal(await none.ready(), false);
        assert.equal(await none.set(key, { base: "b", text: "here" }), true);
        assert.equal(JSON.parse(localStorage.getItem(key)).text, "here");
        assert.equal((await none.get(key)).text, "here");
        // an IndexedDB whose writes fail: the move leaves localStorage's drafts for the next load (and warns), and a write
        // says it failed
        harness.allowProblems();
        const failing = { open() {
            const request = {};
            setTimeout(() => { request.result = { transaction() { throw new Error("quota"); }, close() {} }; request.onsuccess(); }, 0);
            return request;
        } };
        const broken = moqui.devAide.makeDrafts("moqui.devAide.test.broken", failing);
        assert.equal(await broken.ready(), true);
        assert.equal(JSON.parse(localStorage.getItem(key)).text, "here", "not removed when not moved");
        assert.equal(await broken.set(key, { base: "b", text: "lost" }), false);
        assert.equal(await broken.get(key), null);
    } finally {
        localStorage.removeItem(key);
    }
});

test("the Workspace closes the app drawer for its stay, and puts it back unless the user changed it", () => {
    const shell = { leftOpen: true };
    const restore = moqui.devAide.closeAppDrawer(shell);
    assert.equal(shell.leftOpen, false);
    restore();
    assert.equal(shell.leftOpen, true);
    // the user opened it and closed it again on the Workspace: left as the user has it
    let watcher = null, stopped = false;
    const watched = { leftOpen: true, $watch(name, callback) { assert.equal(name, "leftOpen"); watcher = callback; return () => { stopped = true; }; } };
    const put = moqui.devAide.closeAppDrawer(watched);
    watched.leftOpen = true; watcher(); watched.leftOpen = false; watcher();
    put();
    assert.equal(watched.leftOpen, false);
    assert.equal(stopped, true, "the watch ends as the Workspace goes");
    // closed already: left closed
    const closed = { leftOpen: false };
    moqui.devAide.closeAppDrawer(closed)();
    assert.equal(closed.leftOpen, false);
    // no shell (a page test, the terminal): nothing
    moqui.devAide.closeAppDrawer(null)();
});

test("a commit names an issue by its number, else its key; Amend drops only that trailer", () => {
    assert.equal(pure.trailerOf("#12"), "Fixes #12");
    assert.equal(pure.trailerOf("PROJ-12"), "Task: PROJ-12");
    assert.equal(pure.trailerOf("Closes PROJ-3"), "Closes PROJ-3");
    assert.equal(pure.trailerOf("#12a"), "Task: #12a");
    assert.equal(pure.withoutTrailer("Why\n\nTask: PROJ-12\n", "PROJ-12"), "Why");
    assert.equal(pure.withoutTrailer("Why\n\nFixes #12", "#12"), "Why");
    // another issue's Fixes stays
    assert.equal(pure.withoutTrailer("Why\n\nFixes #3", "#12"), "Why\n\nFixes #3");
    assert.equal(pure.withoutTrailer("Why\n\nFixes #12", ""), "Why\n\nFixes #12");
    // the user's own last line stays
    assert.equal(pure.withoutTrailer("Fixes typo", "PROJ-12"), "Fixes typo");
    assert.equal(pure.withoutTrailer("Why\n\nTask: OTHER-1", "PROJ-12"), "Why\n\nTask: OTHER-1");
});

test("the editor settings: the code editor's rebound keys, and the Workspace's own commands as key map actions", () => {
    const answer = { settings: { fontSize: 16, tabSize: null, wrap: true, whitespace: false, keymap: "default", keys: { find: "Alt-f" } },
        commands: [
            { command: "toggleComment", defaultKeys: "Mod-/", keys: "Alt-c" },
            { command: "find", defaultKeys: "Mod-f", keys: "Alt-f" },
            { command: "undo", defaultKeys: "Mod-z", keys: "Mod-z" }] };
    assert.deepEqual(pure.editorSettingsOf(answer), { fontSize: 16, tabSize: null, wrap: true, whitespace: false, keymap: "default",
        keys: { toggleComment: "Alt-c", find: "Alt-f" } });
    assert.deepEqual(pure.editorSettingsOf(null), { keys: {} });
    // Go to file, Go to artifact, Search in files and Save: shell actions, each run by a Workspace method
    assert.deepEqual(pure.WORKSPACE_KEYS.map((command) => command.action + " " + command.keys),
        ["devGotoFile Mod+P", "devGotoArtifact Mod+Shift+O", "devSearchFiles Mod+Shift+F", "devSave Mod+S"]);
    const methods = moqui.devAide.components["m-dev-workspace"].methods;
    for (const command of pure.WORKSPACE_KEYS) assert.equal(typeof methods[command.run], "function", command.run);
    // Save's keys in the text editor are devSave's, as CodeMirror writes them
    assert.equal(pure.codeMirrorKeys("Mod+S"), "Mod-s");
    assert.equal(pure.codeMirrorKeys("Mod+Shift+S"), "Mod-Shift-s");
    assert.equal(pure.codeMirrorKeys("Ctrl++"), "Ctrl-+");
    assert.equal(pure.codeMirrorKeys("g s"), "g s");
    assert.equal(pure.codeMirrorKeys("Esc"), "Escape");
    assert.equal(pure.codeMirrorKeys(""), "");
    const previous = moqui.keymap;
    try {
        moqui.keymap = { keysOf: (scope, binding) => scope === "dev-aide" && binding.action === "devSave" ? "Mod+Alt+S" : binding.keys };
        assert.equal(methods.saveKeys.call({ keyBindings: [{ action: "devSave", keys: "Mod+S" }] }), "Mod-Alt-s");
        moqui.keymap = undefined;
        assert.equal(methods.saveKeys.call({ keyBindings: [] }), null);
    } finally {
        moqui.keymap = previous;
    }
});

test("Replace sends the chosen matches with each file's hash, and refuses a file with unsaved changes in any open buffer", () => {
    const hit = (index, chosen) => ({ index, line: index + 1, chosen });
    const preview = { files: [
        { path: "a.xml", hash: "ha", matches: [hit(0, true), hit(1, false), hit(2, true)] },
        { path: "b.xml", hash: "hb", matches: [hit(0, true)] },
        { path: "c.xml", hash: "hc", matches: [hit(0, false)] },
        { path: "d.xml", hash: "hd", matches: [hit(0, true)] },
        { path: "up.xml", hash: "hu", refused: "upstream", matches: [hit(0, true)] }] };
    // b.xml edited in its tab; d.xml edited as a companion of a.xml's view; another root's d.xml is not this one's
    const tabs = [{ path: "a.xml", text: "x", saved: "x", companions: [{ rootId: "r", path: "d.xml", text: "edited", saved: "x" },
        { rootId: "other", path: "a.xml", text: "edited", saved: "x" }] }, { path: "b.xml", text: "edited", saved: "x" }];
    const request = pure.replaceRequest(preview, tabs, "r");
    assert.deepEqual(request.files, [{ path: "a.xml", hash: "ha", matches: [0, 2] }]);
    assert.equal(request.count, 2);
    assert.deepEqual(request.refused.map((item) => item.path), ["b.xml", "d.xml"]);
    assert.ok(request.refused[0].reason.includes("unsaved"));
    assert.deepEqual(pure.replaceRequest(null, tabs, "r"), { files: [], refused: [], count: 0 });
    assert.deepEqual(pure.openBuffers(tabs, "r").map((open) => open.rootId + ":" + open.path), ["r:a.xml", "r:d.xml", "other:a.xml", "r:b.xml"]);
});

test("a .qvue saves after its XML screen, and a companion's draft is not its file's tab's", () => {
    const xml = { path: "screen/A/Board.xml", text: "x", saved: "" }, qvue = { path: "screen/A/Board.qvue", text: "y", saved: "" };
    assert.deepEqual(pure.saveOrder([qvue, xml]).map((buffer) => buffer.path), ["screen/A/Board.xml", "screen/A/Board.qvue"]);
    assert.notEqual(pure.draftKey("r", { path: "screen/A/Board.xml" }), pure.draftKey("r", { path: "screen/A/Board.xml", owner: "screen/A/Board.qvue" }));
    assert.ok(pure.isDraftKey(pure.draftKey("r", { path: "a.xml", owner: "b.qvue" })));
});
