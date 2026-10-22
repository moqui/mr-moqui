/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Tests of the provider of m-coedit (WebrootVue.qvt.js, moqui.coedit) on the vendored Tiptap bundle's Yjs and awareness, with a
// fake notification client and a fake server that keeps its own Y.Doc: base64, the join and its exchange of state vectors,
// batching, acks, a remote update not sent back, peers and awareness, read-only, errors and fallback, a join again after the
// socket reopens with what was edited offline, and two editors converging. The editor itself (Tiptap on the shared document,
// the carets) needs a browser.
// Run: node --test runtime/base-component/webroot/screen/webroot/js/WebrootVue.qvt.coedit.test.js
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

// lib0 (in the bundle) reads crypto.getRandomValues when it loads
if (!globalThis.crypto) globalThis.crypto = require("node:crypto").webcrypto;
vm.runInThisContext(fs.readFileSync(path.join(__dirname, "../libs/tiptap/tiptap.min.js"), "utf8"));
globalThis.moqui = require("./MoquiLib.js");
require("./WebrootVue.qvt.js");
const T = globalThis.Tiptap;
const { Y } = T;
const coedit = moqui.coedit;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const b64 = coedit.toBase64;
const unb64 = coedit.fromBase64;

// a notification client: what was sent (copies), the listeners by topic, the state listeners, and the open flag
function fakeClient() {
    return {
        sent: [], listeners: {}, stateFns: [], open: false,
        send(message) { this.sent.push(JSON.parse(JSON.stringify(message))); if (this.onSend) this.onSend(message); return this.open; },
        registerListener(topic, fn) {
            (this.listeners[topic] = this.listeners[topic] || []).push(fn);
            return () => { this.listeners[topic] = this.listeners[topic].filter((other) => other !== fn); };
        },
        onState(fn) { this.stateFns.push(fn); return () => { this.stateFns = this.stateFns.filter((other) => other !== fn); }; },
        isOpen() { return this.open; },
        setOpen(open) { this.open = open; this.stateFns.slice().forEach((fn) => fn(open ? "open" : "closed")); },
        deliver(message) { (this.listeners[message.topic] || []).slice().forEach((fn) => fn(message, this)); },
        ofKind(kind) { return this.sent.filter((message) => message.collab === kind); }
    };
}

// an editor's side: a doc, its awareness and a provider on a client
function editor(client, extra) {
    const doc = new Y.Doc();
    const awareness = new T.Awareness(doc);
    const seen = { statuses: [], access: [], saved: [], fallback: [] };
    const provider = new coedit.Provider(Object.assign({ client, docKey: "WikiPage:100023", doc, awareness, lib: T, batchMs: 5,
        onStatus: (status) => seen.statuses.push(status), onAccess: (canEdit) => seen.access.push(canEdit),
        onSaved: (message) => seen.saved.push(message), onFallback: (info) => seen.fallback.push(info) }, extra));
    const done = () => { provider.destroy(); awareness.destroy(); doc.destroy(); };
    return { doc, awareness, provider, seen, done, last: () => seen.statuses[seen.statuses.length - 1] };
}

// The server of the protocol for a doc: a Y.Doc, the clients joined, joined with its diff and state vector, an update applied,
// acknowledged and sent to the others, awareness passed on, peer frames on join and leave.
function fakeServer(options) {
    const opts = options || {};
    const doc = new Y.Doc();
    const members = new Map(); // client id -> {fake, userId, name, awareness}
    const topic = "collab:WikiPage:100023";
    const toAll = (message, except) => members.forEach((member, id) => { if (id !== except) member.fake.deliver(Object.assign({ topic }, message)); });
    return {
        doc, members,
        attach(fake, userId, name) {
            fake.onSend = (message) => {
                const from = message.client;
                if (message.collab === "join") {
                    const peers = Array.from(members.entries()).map(([client, member]) => ({ client, userId: member.userId, name: member.name, awareness: member.awareness }));
                    members.set(from, { fake, userId, name, awareness: message.awareness });
                    fake.deliver({ topic, type: "joined", client: from, canEdit: opts.canEdit !== false,
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
                }
            };
        }
    };
}

const paragraphs = (doc) => doc.getXmlFragment("default").toString();
const typeParagraph = (doc, text) => {
    const fragment = doc.getXmlFragment("default");
    const paragraph = new Y.XmlElement("paragraph");
    paragraph.insert(0, [new Y.XmlText(text)]);
    fragment.push([paragraph]);
};

test("base64 of binary updates round trips, with btoa and without it", () => {
    const sizes = [0, 1, 2, 3, 4, 255, 0x8000 + 7, 70000];
    const withoutBtoa = (fn) => {
        const btoa = globalThis.btoa, atob = globalThis.atob;
        delete globalThis.btoa;
        delete globalThis.atob;
        try { return fn(); } finally { globalThis.btoa = btoa; globalThis.atob = atob; }
    };
    for (const size of sizes) {
        const bytes = new Uint8Array(size);
        for (let i = 0; i < size; i++) bytes[i] = (i * 151 + 7) & 255;
        const text = b64(bytes);
        assert.equal(text, Buffer.from(bytes).toString("base64"), "size " + size);
        assert.deepEqual(Array.from(unb64(text)), Array.from(bytes));
        assert.equal(withoutBtoa(() => b64(bytes)), text);
        assert.deepEqual(Array.from(withoutBtoa(() => unb64(text))), Array.from(bytes));
    }
    assert.equal(unb64("").length, 0);
});

test("a user's caret color is a #rrggbb of the id, the same each time", () => {
    assert.match(coedit.userColor("EX_JOHN_DOE"), /^#[0-9a-f]{6}$/);
    assert.equal(coedit.userColor("EX_JOHN_DOE"), coedit.userColor("EX_JOHN_DOE"));
    assert.match(coedit.userColor(""), /^#[0-9a-f]{6}$/);
});

test("the provider listens on the topic, joins when the socket opens, and applies the server's diff then sends its own", async () => {
    const client = fakeClient();
    const server = fakeServer();
    typeParagraph(server.doc, "From the server");
    const ed = editor(client);
    try {
        // written before the join (offline): it goes in the exchange
        typeParagraph(ed.doc, "Mine");
        ed.provider.start();
        assert.equal(client.listeners["collab:WikiPage:100023"].length, 1);
        assert.equal(client.sent.length, 0, "nothing goes while the socket is not open");
        assert.equal(ed.last().state, "connecting");
        server.attach(client, "EX_JOHN_DOE", "John Doe");
        client.setOpen(true);
        const join = client.ofKind("join")[0];
        assert.equal(join.doc, "WikiPage:100023");
        assert.equal(join.client, ed.provider.clientId);
        assert.equal(join.awareness, ed.doc.clientID);
        assert.ok(join.sv);
        // joined: our diff went as update 1, the server applied it and acknowledged it
        const updates = client.ofKind("update");
        assert.equal(updates.length, 1);
        assert.equal(updates[0].id, 1);
        assert.equal(paragraphs(ed.doc), paragraphs(server.doc));
        assert.match(paragraphs(ed.doc), /From the server/);
        assert.match(paragraphs(ed.doc), /Mine/);
        assert.deepEqual(ed.seen.access, [true]);
        assert.deepEqual(ed.last(), { state: "synced", peers: [], pending: 0 });
        // the local awareness went once joined
        assert.equal(client.ofKind("awareness").length, 1);
    } finally {
        ed.done();
    }
});

test("local edits within the batch time go as one update, pending until acknowledged; another client's ack is not ours", async () => {
    const client = fakeClient();
    const ed = editor(client);
    try {
        client.open = true;
        ed.provider.start();
        const topic = "collab:WikiPage:100023";
        const join = client.ofKind("join")[0];
        // a joined of another editor of the same user is not ours
        client.deliver({ topic, type: "joined", client: "other", canEdit: false, update: b64(Y.encodeStateAsUpdate(new Y.Doc())), sv: b64(new Uint8Array([0])), peers: [] });
        assert.equal(ed.last().state, "connecting");
        client.deliver({ topic, type: "joined", client: join.client, canEdit: true, update: b64(Y.encodeStateAsUpdate(new Y.Doc())), sv: b64(new Uint8Array([0])), peers: [] });
        assert.equal(client.ofKind("update").length, 0, "an empty document has no diff to send");
        const text = ed.doc.getText("t");
        text.insert(0, "a");
        text.insert(1, "b");
        text.insert(2, "c");
        assert.equal(ed.last().state, "saving");
        await sleep(30);
        const updates = client.ofKind("update");
        assert.equal(updates.length, 1, "three transactions, one message");
        assert.equal(updates[0].id, 1);
        const copy = new Y.Doc();
        Y.applyUpdate(copy, unb64(updates[0].update));
        assert.equal(copy.getText("t").toString(), "abc");
        assert.deepEqual(ed.last(), { state: "saving", peers: [], pending: 1 });
        client.deliver({ topic, type: "ack", client: "other", id: 1 });
        assert.equal(ed.last().pending, 1);
        client.deliver({ topic, type: "ack", client: join.client, id: 1 });
        assert.deepEqual(ed.last(), { state: "synced", peers: [], pending: 0 });
        // a second batch is message 2
        text.insert(3, "d");
        await sleep(30);
        assert.deepEqual(client.ofKind("update").map((message) => message.id), [1, 2]);
        // a remote update applies and is not sent back
        const remote = new Y.Doc();
        Y.applyUpdate(remote, Y.encodeStateAsUpdate(ed.doc));
        remote.getText("t").insert(0, ">");
        client.deliver({ topic, type: "update", client: "someone", update: b64(Y.encodeStateAsUpdate(remote, Y.encodeStateVector(ed.doc))) });
        assert.equal(text.toString(), ">abcd");
        await sleep(30);
        assert.equal(client.ofKind("update").length, 2);
        // our own update coming back on the topic is not applied twice (and would be harmless)
        client.deliver({ topic, type: "update", client: join.client, update: client.ofKind("update")[1].update });
        assert.equal(text.toString(), ">abcd");
        remote.destroy();
        copy.destroy();
    } finally {
        ed.done();
    }
});

test("peers come and go, the others' awareness applies and their carets go with them", async () => {
    const client = fakeClient();
    const ed = editor(client);
    const other = new Y.Doc();
    const otherAwareness = new T.Awareness(other);
    try {
        client.open = true;
        ed.provider.start();
        const topic = "collab:WikiPage:100023";
        const me = client.ofKind("join")[0].client;
        client.deliver({ topic, type: "joined", client: me, canEdit: true, update: b64(Y.encodeStateAsUpdate(new Y.Doc())), sv: b64(new Uint8Array([0])),
            peers: [{ client: "c-ann", userId: "EX_ANN", name: "Ann", awareness: other.clientID }, { client: me, userId: "EX_ME", name: "Me", awareness: ed.doc.clientID }] });
        assert.deepEqual(ed.last().peers, [{ client: "c-ann", userId: "EX_ANN", name: "Ann" }]);
        otherAwareness.setLocalStateField("user", { name: "Ann", color: "#e53935" });
        client.deliver({ topic, type: "awareness", client: "c-ann", update: b64(T.encodeAwarenessUpdate(otherAwareness, [other.clientID])) });
        assert.deepEqual(ed.awareness.getStates().get(other.clientID).user, { name: "Ann", color: "#e53935" });
        client.deliver({ topic, type: "peer", client: "c-bob", userId: "EX_BOB", name: "Bob", awareness: 77, joined: true });
        assert.deepEqual(ed.last().peers.map((peer) => peer.name), ["Ann", "Bob"]);
        client.deliver({ topic, type: "peer", client: "c-ann", userId: "EX_ANN", name: "Ann", awareness: other.clientID, joined: false });
        assert.deepEqual(ed.last().peers.map((peer) => peer.name), ["Bob"]);
        assert.equal(ed.awareness.getStates().has(other.clientID), false, "Ann's caret is gone");
        // a local caret change goes as an awareness frame of our state only
        const before = client.ofKind("awareness").length;
        ed.awareness.setLocalStateField("cursor", { anchor: 1 });
        await sleep(30);
        const frames = client.ofKind("awareness");
        assert.equal(frames.length, before + 1);
        const check = new T.Awareness(new Y.Doc());
        T.applyAwarenessUpdate(check, unb64(frames[frames.length - 1].update), "test");
        assert.deepEqual(check.getStates().get(ed.doc.clientID), { cursor: { anchor: 1 } });
        check.destroy();
        // saved is told to the host
        client.deliver({ topic, type: "saved", versionName: "3", userId: "EX_BOB", name: "Bob" });
        assert.deepEqual(ed.seen.saved.map((message) => message.versionName), ["3"]);
    } finally {
        otherAwareness.destroy();
        other.destroy();
        ed.done();
    }
});

test("read-only: canEdit false sends no diff and no edits, and a 403 after the join makes it read-only", async () => {
    const client = fakeClient();
    const server = fakeServer({ canEdit: false });
    const ed = editor(client);
    try {
        server.attach(client, "EX_GUEST", "Guest");
        typeParagraph(ed.doc, "not sent");
        client.open = true;
        ed.provider.start();
        assert.equal(client.ofKind("update").length, 0);
        assert.equal(ed.last().state, "readonly");
        assert.deepEqual(ed.seen.access, [false]);
        typeParagraph(ed.doc, "nor this");
        await sleep(30);
        assert.equal(client.ofKind("update").length, 0);
    } finally {
        ed.done();
    }
    const client2 = fakeClient();
    const ed2 = editor(client2);
    try {
        fakeServer().attach(client2, "EX_ANN", "Ann");
        client2.open = true;
        ed2.provider.start();
        assert.equal(ed2.last().state, "synced");
        const me = ed2.provider.clientId;
        client2.deliver({ topic: "collab:WikiPage:100023", type: "error", client: "other", code: 403, message: "not yours" });
        assert.equal(ed2.last().state, "synced", "another client's error is not ours");
        client2.deliver({ topic: "collab:WikiPage:100023", type: "error", client: me, code: 403, message: "The page is locked" });
        assert.equal(ed2.last().state, "readonly");
        assert.deepEqual(ed2.seen.access, [true, false]);
    } finally {
        ed2.done();
    }
});

test("an error answering the join, or no answer, is a fallback; a 409 after it joins again, three times at most", async () => {
    for (const code of [404, 500]) {
        const client = fakeClient();
        const ed = editor(client);
        try {
            client.open = true;
            ed.provider.start();
            client.deliver({ topic: "collab:WikiPage:100023", type: "error", client: ed.provider.clientId, code, message: "no such page" });
            assert.deepEqual(ed.seen.fallback, [{ reason: "server", code, message: "no such page" }]);
            assert.equal(ed.last().state, "failed");
            assert.equal(client.ofKind("leave").length, 1);
            // nothing more is taken
            client.setOpen(false);
            client.setOpen(true);
            assert.equal(client.ofKind("join").length, 1);
        } finally {
            ed.done();
        }
    }
    const quiet = fakeClient();
    const timed = editor(quiet, { joinTimeoutMs: 10 });
    try {
        quiet.open = true;
        timed.provider.start();
        await sleep(40);
        assert.deepEqual(timed.seen.fallback.map((info) => info.reason), ["timeout"]);
    } finally {
        timed.done();
    }
    const client = fakeClient();
    const ed = editor(client);
    try {
        fakeServer().attach(client, "EX_ANN", "Ann");
        client.open = true;
        ed.provider.start();
        const me = ed.provider.clientId;
        for (let i = 0; i < 3; i++) client.deliver({ topic: "collab:WikiPage:100023", type: "error", client: me, code: 409, message: "conflict" });
        assert.equal(client.ofKind("join").length, 4);
        assert.equal(ed.seen.fallback.length, 0);
        client.deliver({ topic: "collab:WikiPage:100023", type: "error", client: me, code: 409, message: "conflict" });
        assert.deepEqual(ed.seen.fallback.map((info) => info.code), [409]);
    } finally {
        ed.done();
    }
});

test("offline edits merge when the socket reopens: the join sends the state vector and the diff goes", async () => {
    const client = fakeClient();
    const server = fakeServer();
    const ed = editor(client);
    try {
        server.attach(client, "EX_ANN", "Ann");
        client.open = true;
        ed.provider.start();
        typeParagraph(ed.doc, "online");
        await sleep(30);
        assert.equal(paragraphs(server.doc), paragraphs(ed.doc));
        client.setOpen(false);
        assert.equal(ed.last().state, "offline");
        typeParagraph(ed.doc, "offline");
        // someone else wrote meanwhile
        typeParagraph(server.doc, "elsewhere");
        await sleep(30);
        const sentBefore = client.ofKind("update").length;
        client.setOpen(true);
        const joins = client.ofKind("join");
        assert.equal(joins.length, 2);
        const sv = Y.decodeStateVector(unb64(joins[1].sv));
        assert.ok(sv.get(ed.doc.clientID) > 0, "the state vector has our edits");
        assert.equal(client.ofKind("update").length, sentBefore + 1, "the offline edit went in the exchange");
        assert.equal(paragraphs(server.doc), paragraphs(ed.doc));
        assert.match(paragraphs(ed.doc), /elsewhere/);
        assert.match(paragraphs(ed.doc), /offline/);
        assert.equal(ed.last().state, "synced");
    } finally {
        ed.done();
    }
});

test("two editors through the server converge on concurrent edits, and leave tells the other", async () => {
    const server = fakeServer();
    typeParagraph(server.doc, "Seed");
    const clientA = fakeClient();
    const clientB = fakeClient();
    server.attach(clientA, "EX_ANN", "Ann");
    server.attach(clientB, "EX_BOB", "Bob");
    const a = editor(clientA);
    const b = editor(clientB);
    try {
        clientA.open = true;
        clientB.open = true;
        a.provider.start();
        b.provider.start();
        assert.deepEqual(a.last().peers.map((peer) => peer.name), ["Bob"]);
        assert.deepEqual(b.last().peers.map((peer) => peer.name), ["Ann"]);
        const textA = a.doc.getXmlFragment("default").get(0).get(0);
        const textB = b.doc.getXmlFragment("default").get(0).get(0);
        textA.insert(0, "Ann: ");
        textB.insert(4, " from Bob");
        textA.insert(textA.length, "!");
        await sleep(30);
        assert.equal(paragraphs(a.doc), paragraphs(b.doc));
        assert.equal(paragraphs(a.doc), paragraphs(server.doc));
        // both edits are there; A's "!" and Bob's " from Bob" went at one place, and Yjs orders such inserts by client id
        assert.ok(["Ann: Seed! from Bob", "Ann: Seed from Bob!"].includes(textA.toString()), textA.toString());
        const converged = textA.toString();
        assert.equal(a.last().pending, 0);
        // the last batch goes before leave
        textB.insert(0, "x");
        b.provider.destroy();
        const kinds = clientB.sent.map((message) => message.collab);
        assert.deepEqual(kinds.slice(-2), ["update", "leave"]);
        assert.equal(textA.toString(), "x" + converged);
        assert.deepEqual(a.last().peers, []);
        assert.equal(clientB.listeners["collab:WikiPage:100023"].length, 0, "the listener is gone");
    } finally {
        a.done();
        b.done();
    }
});
