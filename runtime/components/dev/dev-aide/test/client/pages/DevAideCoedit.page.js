/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Co-editing a file (DevAideCoedit.js, plans/design-dev-aide.md F15), in the page: the pure parts, and two Workspace
// tabs on one room of a fake server (the protocol of moqui.coedit.Provider, the room seeded from the file as
// moqui-collab's text rooms are), each bound to an m-code-editor's extensions (y-codemirror.next): an edit in one shows
// in the other and in its tab's text, a tab's unsaved edits go into a room still at the file, the other's caret shows
// with its name, and a closed socket ends the sessions, which start again when it opens.
"use strict";

const coedit = moqui.devAide.coedit;
const CM = CodeMirror;
const { Y } = Yjs;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const b64 = moqui.coedit.toBase64;
const unb64 = moqui.coedit.fromBase64;
const SEED = "<services>\n</services>\n";
const KEY = coedit.keyOf("site", "service/Things.xml");

function fakeClient() {
    return {
        open: true, listeners: {}, stateFns: [],
        send(message) { if (this.onSend) this.onSend(JSON.parse(JSON.stringify(message))); return this.open; },
        registerListener(topic, fn) {
            (this.listeners[topic] = this.listeners[topic] || []).push(fn);
            return () => { this.listeners[topic] = this.listeners[topic].filter((other) => other !== fn); };
        },
        onState(fn) { this.stateFns.push(fn); return () => { this.stateFns = this.stateFns.filter((other) => other !== fn); }; },
        isOpen() { return this.open; },
        // a socket that closes is gone from the rooms, as the server's close takes its clients out
        setOpen(open) { this.open = open; if (!open && this.onClose) this.onClose(); this.stateFns.slice().forEach((fn) => fn(open ? "open" : "closed")); },
        deliver(message) { (this.listeners[message.topic] || []).slice().forEach((fn) => fn(message, this)); }
    };
}

// The room: seeded from the file at its first join, an update applied and passed on, awareness passed on, peers told.
function fakeRoom() {
    const topic = "collab:" + KEY;
    const members = new Map();
    let doc = null;
    let seeds = 0, seedId = null;
    const toAll = (message, except) => members.forEach((member, id) => { if (id !== except) member.fake.deliver(Object.assign({ topic }, message)); });
    return {
        members,
        text: () => (doc ? doc.getText("codemirror").toString() : null),
        // the room closes on the server (its members gone), and its next join seeds it again from the file
        close() { members.clear(); doc = null; },
        // the server's error to one client
        refuse(fake, client, code) { fake.deliver({ topic, type: "error", client, code, message: "join the document first" }); },
        attach(fake, userId, name) {
            fake.onClose = () => {
                for (const [client, member] of Array.from(members.entries())) if (member.fake === fake) members.delete(client);
                if (!members.size) doc = null;
            };
            fake.onSend = (message) => {
                const from = message.client;
                if (message.collab === "join") {
                    if (!doc) { doc = new Y.Doc(); doc.getText("codemirror").insert(0, SEED); seedId = "seed-" + (++seeds); }
                    const peers = Array.from(members.entries()).map(([client, member]) => ({ client, userId: member.userId, name: member.name, awareness: member.awareness }));
                    members.set(from, { fake, userId, name, awareness: message.awareness });
                    fake.deliver({ topic, type: "joined", client: from, canEdit: true, seed: seedId, hash: "file-hash",
                        update: b64(Y.encodeStateAsUpdate(doc, unb64(message.sv))), sv: b64(Y.encodeStateVector(doc)), peers });
                    toAll({ type: "peer", client: from, userId, name, awareness: message.awareness, joined: true }, from);
                } else if (message.collab === "update") {
                    Y.applyUpdate(doc, unb64(message.update));
                    fake.deliver({ topic, type: "ack", client: from, id: message.id });
                    toAll({ type: "update", client: from, update: message.update }, from);
                } else if (message.collab === "awareness") {
                    toAll({ type: "awareness", client: from, update: message.update }, from);
                } else if (message.collab === "leave") {
                    const member = members.get(from);
                    members.delete(from);
                    if (member) toAll({ type: "peer", client: from, userId: member.userId, name: member.name, awareness: member.awareness, joined: false });
                    // the room is dropped with its last member, and seeded from the file again
                    if (!members.size) doc = null;
                }
            };
        }
    };
}

// An editor of a tab's shared text, with m-code-editor's own extensions for it (its collab, history and keys parts).
function editorOf(tab) {
    const methods = moqui.codeEditor.components["m-code-editor"].methods;
    const vm = { collab: tab.collab, owner: { vm: null } };
    vm.effectiveSettings = () => moqui.codeEditor.normalizeSettings(null);
    const parts = methods.parts.call(vm, CM);
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new CM.EditorView({ parent, state: CM.EditorState.create({ doc: tab.collab.ytext.toString(),
        extensions: [parts.collab(), parts.history(), parts.keys()] }) });
    return { view, parent, done: () => { view.destroy(); parent.remove(); } };
}

test("a tab's room key, which tabs join, and what a joining tab does with the room's text", () => {
    assert.equal(coedit.keyOf("site", "service/a/Things.xml"), "DevFile:site:service/a/Things.xml");
    const tab = { path: "service/Things.xml", text: "a", saved: "a" };
    assert.ok(coedit.eligible(tab, "site", false));
    assert.ok(!coedit.eligible(Object.assign({}, tab, { readOnly: true }), "site", false));
    assert.ok(!coedit.eligible(Object.assign({}, tab, { loading: true }), "site", false));
    assert.ok(!coedit.eligible(tab, "site", true), "a hosted root's files are not co-edited");
    assert.ok(!coedit.eligible(Object.assign({}, tab, { path: "x/".repeat(100) + "y.xml" }), "site", false), "a key over 200");
    // a clean tab takes the room's text; a dirty one pushes into a room still at the file, else gives way
    assert.deepEqual(coedit.joinPlan({ text: "a\r\nb", saved: "a\r\nb" }, "a\nb"), { take: true, push: false, dropped: false });
    assert.deepEqual(coedit.joinPlan({ text: "a\nb", saved: "a\nb" }, "a\nb!"), { take: true, push: false, dropped: false });
    assert.deepEqual(coedit.joinPlan({ text: "mine", saved: "file" }, "file"), { take: false, push: true, dropped: false });
    assert.deepEqual(coedit.joinPlan({ text: "mine", saved: "file" }, "theirs"), { take: true, push: false, dropped: true });
    assert.deepEqual(coedit.peerNames({ peers: [{ userId: "u1", name: "Ann" }, { userId: "u1", name: "Ann" }, { userId: "me", name: "Me" }] }, "me"), ["Ann"]);
    const doc = new Y.Doc(), ytext = doc.getText("codemirror");
    ytext.insert(0, "hello world");
    assert.ok(coedit.writeText(ytext, "hello there world", null));
    assert.equal(ytext.toString(), "hello there world");
    assert.ok(!coedit.writeText(ytext, "hello there world", null));
    doc.destroy();
});

test("two tabs on one room: edits go both ways, unsaved edits go into a fresh room, and the other's caret has its name", async () => {
    const room = fakeRoom();
    const lib = { Yjs, YCodeMirror };
    const ann = { client: fakeClient(), tab: { path: "service/Things.xml", text: SEED, saved: SEED, collab: null, coedit: null } };
    // bob has unsaved edits of the file as he read it: they go into the room, which is still the file
    const bob = { client: fakeClient(), tab: { path: "service/Things.xml", text: SEED + "<!-- bob -->\n", saved: SEED, collab: null, coedit: null } };
    room.attach(ann.client, "ann", "Ann Example");
    room.attach(bob.client, "bob", "Bob Example");
    ann.session = new coedit.Session({ client: ann.client, rootId: "site", tab: ann.tab, user: { id: "ann", name: "Ann Example", color: "#e53935" }, lib, batchMs: 1 }).start();
    await sleep(5);
    bob.session = new coedit.Session({ client: bob.client, rootId: "site", tab: bob.tab, user: { id: "bob", name: "Bob Example", color: "#43a047" }, lib, batchMs: 1 }).start();
    await sleep(30);
    try {
        assert.ok(ann.tab.collab && bob.tab.collab, "both bound");
        assert.equal(room.text(), SEED + "<!-- bob -->\n");
        assert.equal(ann.tab.text, SEED + "<!-- bob -->\n");
        assert.deepEqual(ann.tab.coedit.peers, ["Bob Example"]);
        ann.editor = editorOf(ann.tab);
        bob.editor = editorOf(bob.tab);
        // an edit in ann's editor: in the room, bob's editor and bob's tab
        ann.editor.view.dispatch({ changes: { from: SEED.indexOf("</services>"), insert: "    <service verb=\"get\" noun=\"Thing\"/>\n" } });
        await sleep(30);
        const both = SEED.replace("</services>", "    <service verb=\"get\" noun=\"Thing\"/>\n</services>") + "<!-- bob -->\n";
        assert.equal(room.text(), both);
        assert.equal(bob.editor.view.state.doc.toString(), both);
        assert.equal(bob.tab.text, both);
        // the tab's text set by the Workspace (Take the disk's) goes into the room, and to ann
        bob.session.fromTab("<services/>\r\n");
        await sleep(30);
        assert.equal(ann.editor.view.state.doc.toString(), "<services/>\n");
        // ann's caret, from her awareness, in bob's editor with her name: the server's, not the one her awareness claims,
        // and only a color that is one
        const ytext = ann.tab.collab.ytext;
        const at = Y.createRelativePositionFromTypeIndex(ytext, 2);
        ann.session.awareness.setLocalStateField("user", { name: "Mallory", color: "red;background:url(/x)", colorLight: "#e5393533" });
        ann.session.awareness.setLocalStateField("cursor", { anchor: Y.relativePositionToJSON(at), head: Y.relativePositionToJSON(at) });
        await sleep(30);
        const seen = bob.session.awareness.getStates().get(ann.session.awareness.clientID).user;
        assert.equal(seen.name, "Ann Example");
        assert.ok(/^#[0-9a-f]{6}$/i.test(seen.color), seen.color);
        assert.equal(seen.colorLight, "#e5393533");
        const caret = bob.editor.parent.querySelector(".cm-ySelectionCaret");
        assert.ok(caret, "ann's caret");
        assert.ok(caret.textContent.includes("Ann Example") && !caret.textContent.includes("Mallory"), caret.textContent);
        assert.ok(!caret.getAttribute("style").includes("url("), caret.getAttribute("style"));
        // undo is Yjs' and only this user's own: bob's takes back his Take the disk's, not ann's edit
        bob.tab.collab.undoManager.undo();
        await sleep(30);
        assert.equal(ann.editor.view.state.doc.toString(), both);
    } finally {
        for (const side of [ann, bob]) { if (side.editor) side.editor.done(); side.session.end(); }
    }
    assert.equal(ann.tab.collab, null);
    assert.equal(room.members.size, 0);
});

test("the Workspace's sessions end with the socket and start again when it opens; a refused file waits", async () => {
    const room = fakeRoom();
    const client = fakeClient();
    room.attach(client, "ann", "Ann");
    const tab = { path: "service/Things.xml", text: SEED, saved: SEED, collab: null, coedit: null };
    const readOnly = { path: "component.xml", text: "<component/>", saved: "<component/>", readOnly: true, collab: null };
    const sessions = new coedit.Sessions({ client, user: { id: "ann", name: "Ann" }, batchMs: 1 });
    try {
        await sessions.sync("site", [tab, readOnly], false);
        await sleep(20);
        assert.ok(tab.collab, "joined");
        assert.equal(readOnly.collab, null);
        assert.equal(sessions.sessions.size, 1);
        client.setOpen(false);
        assert.equal(tab.collab, null);
        assert.equal(sessions.sessions.size, 0);
        assert.equal(tab.text, SEED, "the tab keeps its text");
        client.setOpen(true);
        await sleep(20);
        assert.ok(tab.collab, "joined again on the open");
        // a tab closed is a session ended
        await sessions.sync("site", [], false);
        assert.equal(tab.collab, null);
        assert.equal(room.members.size, 0);
    } finally {
        sessions.destroy();
    }
});

test("a session the server ends past its join starts again with a new document, so a room seeded again is not doubled", async () => {
    const room = fakeRoom();
    const client = fakeClient();
    room.attach(client, "ann", "Ann");
    const tab = { path: "service/Things.xml", text: SEED, saved: SEED, collab: null, coedit: null };
    const ended = [];
    const sessions = new coedit.Sessions({ client, user: { id: "ann", name: "Ann" }, batchMs: 1, onEnded: (endedTab, info) => ended.push(info) });
    try {
        await sessions.sync("site", [tab], false);
        await sleep(20);
        const first = tab.collab;
        assert.ok(first, "joined");
        // the room closes under the session (its socket's updates over the rate, say), and the server says so
        room.close();
        const session = sessions.sessions.get(coedit.keyOf("site", tab.path)).session;
        room.refuse(client, session.provider.clientId, 409);
        await sleep(30);
        assert.ok(tab.collab && tab.collab !== first, "a new session");
        assert.ok(tab.collab.ytext.doc !== first.ytext.doc, "with a new document");
        assert.equal(room.text(), SEED, "the room seeded again holds the file once");
        assert.equal(tab.text, SEED);
        assert.deepEqual(ended, []);
        // a right taken away past the join ends it for good, said once
        const again = sessions.sessions.get(coedit.keyOf("site", tab.path)).session;
        room.refuse(client, again.provider.clientId, 403);
        await sleep(20);
        assert.equal(tab.collab, null);
        assert.equal(ended.length, 1);
        assert.equal(ended[0].code, 403);
    } finally {
        sessions.destroy();
    }
});

test("a tab that read another version of the file than the room was seeded from is told", async () => {
    const told = [];
    for (const base of ["an-older-hash", "file-hash"]) {
        const room = fakeRoom();
        const client = fakeClient();
        room.attach(client, "ann", "Ann");
        const tab = { path: "service/Things.xml", text: SEED, saved: SEED, base, collab: null, coedit: null };
        const sessions = new coedit.Sessions({ client, user: { id: "ann", name: "Ann" }, batchMs: 1, onOtherBase: () => told.push(base) });
        try {
            await sessions.sync("site", [tab], false);
            await sleep(20);
            assert.ok(tab.collab, base);
        } finally {
            sessions.destroy();
        }
    }
    assert.deepEqual(told, ["an-older-hash"]);
});
