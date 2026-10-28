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
