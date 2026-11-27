/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
"use strict";
(function () {
const until = async (check, what, ms) => {
    const end = Date.now() + (ms || 5000);
    await Vue.nextTick();
    for (;;) {
        const value = check();
        if (value) return value;
        if (Date.now() > end) throw new Error("waited for " + what);
        await new Promise((resolve) => setTimeout(resolve, 10));
    }
};
const text = () => document.body.textContent;

// ---- the server, as the host passes it on ----
const room = { chatRoomId: "R1", roomTypeEnumId: "CrtPrivate", roomName: "Ops", displayName: "Ops", lastEventSeq: 1, isMember: true, unreadCount: 0 };
const timeline = { eventList: [{ eventSeq: 1, type: "message", senderUserId: "bob", senderName: "Bob", sentDate: Date.now() - 60000,
    body: "<p>hello</p>", bodyText: "hello", reactions: [], attachments: [] }], hasMoreBefore: false, hasMoreAfter: false };
const answers = {
    "/qapps/clientConfig": () => ({ userId: "ann", username: "ann", userFullName: "Ann", locale: "en", server: { name: "Test Server", clientApi: 4 },
        calls: { appRoot: "/qapps/BasicComms" }, notificationEndpoint: "/notws" }),
    "/shellData": () => ({ apps: [{ name: "Other", path: "/apps/other", label: "Other app" }], counts: {} }),
    "/apps/l10nCatalog": () => ({}),
    "/apps/BasicComms/Chat/chatData": () => ({ me: { userId: "ann" }, prefs: {}, roomList: [room] }),
    "/apps/BasicComms/Chat/roomView": () => ({ room, member: { lastReadEventSeq: 1 }, access: { isMember: true, canPost: true },
        memberList: [{ userId: "ann", name: "Ann" }, { userId: "bob", name: "Bob" }], timeline }),
    "/apps/BasicComms/Chat/timeline": () => timeline,
    "/apps/BasicComms/Inbox/Mail/inboxData": () => ({ accountList: [], mailboxList: [], searchList: [], totals: {}, me: { userId: "ann" }, prefs: {} }),
    "/apps/BasicComms/Inbox/Mail/threads": () => ({ threadList: [], hasMore: false, nextCursor: null, storeList: ["email"], failedStores: [], termList: [] })
};
const plainHeaders = { get: () => null };
moqui.request = (url) => {
    const answer = answers[String(url).split("?")[0]];
    moqui.noteAnswer(200, plainHeaders);
    return Promise.resolve({ ok: true, status: 200, headers: plainHeaders, json: answer ? answer() : {}, text: "" });
};
moqui.crypto.ensureDevice = () => Promise.reject({ reasonCode: "standIn", errors: "No crypto device in this test" });

// ---- the host: a build whose start screen is the bundled Chat; a link it kept for a cold start ----
const host = { pendingOpen: "/qapps/BasicComms/Inbox", remotes: [] };
const shellState = window.Vue.reactive({ accounts: [{ id: "a1", username: "ann", server: "https://s.example.com", deviceMode: "personal" }], current: "a1",
    signedIn: true, keyStore: "os", clientApi: 4, deviceMode: "choose", loadToken: null, startPath: "/qapps/BasicComms/Chat" });
const none = () => Promise.resolve([]);
window.moquiDesktop = {
    state: () => Promise.resolve(shellState),
    takeOpen: () => { const path = host.pendingOpen; host.pendingOpen = null; return Promise.resolve(path); },
    clearScopes: () => Promise.resolve(),
    socket: () => ({ readyState: 0, send() {}, close() {} }),
    outbox: { add: () => Promise.resolve({}), list: none, kinds: () => Promise.resolve({ clientApi: 3, kinds: [], internalSend: false }),
        retry: () => Promise.resolve(true), discard: () => Promise.resolve(true), on: () => () => {}, unsent: () => Promise.resolve({ operations: [], drafts: [] }) },
    drafts: { get: () => Promise.resolve(null), list: none, put: () => Promise.resolve(), remove: () => Promise.resolve(true), saveFile: () => Promise.resolve(null) },
    signOut: () => Promise.resolve(),
    openRemote: (path) => { host.remotes.push(path); return Promise.resolve(); },
    setCounts() {}, setPresence() {}, storageScope: "a1"
};

let root = null;
const mount = async () => {
    if (root) return root;
    const shell = document.createElement("div");
    shell.id = "shell";
    document.body.appendChild(shell);
    await moqui.loadScript("/files/crates/moqui-desktop/shell/js/shell.js");
    root = await until(() => moqui.webrootVue && moqui.webrootVue.commsItems && moqui.webrootVue, "the shell's root");
    return root;
};
const backButton = () => document.querySelector(".q-header button[aria-label='Back']");

test("a link at a cold start has the start screen under it: Back goes to the start screen, and there leaves the app", async () => {
    await mount();
    await until(() => root.view === "mail", "the link the host kept");
    assert.deepEqual(root.backStack, ["/qapps/BasicComms/Chat"], "the start screen under the link");
    assert.ok(!root.atStart);
    assert.ok(moqui.back.pop(), "Back goes to the start screen");
    await until(() => root.view === "chat", "the start screen");
    assert.ok(root.atStart);
    assert.equal(moqui.back.pop(), false, "at the start screen Back does nothing, so the app leaves");
    assert.equal(root.view, "chat", "and never shows the chat panel or a blank page");
    await Vue.nextTick();
    assert.ok(!backButton(), "no Back in the header at the start screen");
    assert.deepEqual(host.remotes, [], "no server page opened");
});

test("the start screen is the home: no list of apps and no chat panel; Back from a place it opened goes back to it", async () => {
    await mount();
    root.goHome();
    await until(() => root.view === "chat" && root.atStart, "the home is the start screen");
    root.drawer = true;
    await Vue.nextTick();
    assert.ok(!text().includes("Applications"), "no list of apps");
    assert.ok(!text().includes("Other app"));
    assert.ok(!document.querySelector(".m-shell-chat"), "no chat panel");
    root.drawer = false;
    root.setUrl("/qapps/BasicComms/Inbox");
    await until(() => root.view === "mail", "another place");
    assert.ok(backButton(), "Back in the header away from the start screen");
    root.setUrl("/qapps/BasicComms/Chat?chatRoomId=R1");
    await until(() => root.currentParameters.chatRoomId === "R1", "a room");
    assert.ok(!root.atStart, "a place in the history is not the start");
    assert.ok(moqui.back.pop());
    await until(() => root.view === "mail", "back one step");
    assert.ok(moqui.back.pop());
    await until(() => root.view === "chat" && root.atStart, "back at the start screen");
    assert.equal(moqui.back.pop(), false);
    assert.equal(moqui.back.pop(), false, "Back again does not loop");
});

test("a link the host opens while the shell shows the start screen goes on its history", async () => {
    await mount();
    root.goHome();
    await until(() => root.atStart, "the start screen");
    host.pendingOpen = "/qapps/BasicComms/Inbox";
    window.moquiShell.poke();
    await until(() => root.view === "mail", "the link");
    assert.ok(moqui.back.pop());
    await until(() => root.atStart, "back at the start screen");
    // a poke with no link does not move the shell
    window.moquiShell.poke();
    await new Promise((resolve) => setTimeout(resolve, 20));
    assert.ok(root.atStart);
});

test("a start screen the shell does not bundle opens as the server's page, as /qapps", async () => {
    await mount();
    shellState.startPath = "/apps/feed";
    try {
        await Vue.nextTick();
        root.goHome();
        assert.deepEqual(host.remotes, ["/qapps/feed"]);
        assert.equal(root.view, null);
        assert.ok(!root.atStart);
        assert.ok(!document.querySelector(".m-shell-chat"), "no chat panel under it");
        // the shell itself showing (offline, the server down, before the page loads): Back leaves the app, and opens nothing
        assert.equal(moqui.back.pop(), false, "Back with the shell showing and no view leaves the app");
        assert.deepEqual(host.remotes, ["/qapps/feed"], "no server page opened again");
        // offline: the server's start page waits on the shell, which says so, and opens once the server answers
        host.remotes.length = 0;
        moqui.connection.offline = true;
        await Vue.nextTick();
        root.goHome();
        await until(() => document.querySelector(".m-shell-start-waiting"), "the notice that the app waits for the server");
        assert.deepEqual(host.remotes, [], "no page that would fail to load");
        assert.equal(moqui.back.pop(), false, "Back on the waiting shell leaves the app");
        moqui.connection.offline = false;
        await until(() => host.remotes.length, "the start page once the server answers");
        assert.deepEqual(host.remotes, ["/qapps/feed"]);
        // an offline start (the shell's start answered from the store) with no socket yet: the shell's own request
        // answered by the server opens it, and the socket opening after that opens nothing more (and reloads nothing)
        host.remotes.length = 0;
        root.$.props.offline = true;
        moqui.connection.offline = true;
        await Vue.nextTick();
        root.goHome();
        await until(() => document.querySelector(".m-shell-start-waiting"), "the notice of an offline start");
        moqui.connection.offline = false;
        await until(() => host.remotes.length, "the start page once a request is answered");
        root.socketState = "open";
        await new Promise((resolve) => setTimeout(resolve, 20));
        assert.deepEqual(host.remotes, ["/qapps/feed"], "opened exactly once");
        // and the socket first: once too
        host.remotes.length = 0;
        root.socketState = "closed";
        moqui.connection.offline = true;
        await Vue.nextTick();
        root.goHome();
        await until(() => document.querySelector(".m-shell-start-waiting"), "the notice again");
        root.socketState = "open";
        await until(() => host.remotes.length, "the start page once the socket opens");
        moqui.connection.offline = false;
        await new Promise((resolve) => setTimeout(resolve, 20));
        assert.deepEqual(host.remotes, ["/qapps/feed"], "opened exactly once");
    } finally {
        root.$.props.offline = false;
        root.socketState = null;
        moqui.connection.offline = false;
        shellState.startPath = "/qapps/BasicComms/Chat";
        host.remotes.length = 0;
    }
});

test("a cold start's link to the start screen, as /apps or /qapps, has nothing under it", async () => {
    await mount();
    shellState.startPath = "/apps/BasicComms/Chat";
    try {
        await Vue.nextTick();
        root.goHome();
        await until(() => root.atStart, "the start screen");
        root.view = null;
        root.openPathFromHost("/qapps/BasicComms/Chat");
        await until(() => root.view === "chat", "the link");
        assert.deepEqual(root.backStack, [], "not the start screen under itself");
        assert.ok(root.atStart);
    } finally {
        shellState.startPath = "/qapps/BasicComms/Chat";
    }
});

test("a start screen with a query is the start only with that query, or what took its place", async () => {
    await mount();
    shellState.startPath = "/qapps/BasicComms/Chat?chatRoomId=R1";
    try {
        await Vue.nextTick();
        root.goHome();
        await until(() => root.atStart && root.currentParameters.chatRoomId === "R1", "the start room");
        // the same view with another room and nothing before it (a history cut short) is not the start: Back goes to it
        root.setUrl("/qapps/BasicComms/Chat?chatRoomId=R2", null, null, false);
        await until(() => root.currentParameters.chatRoomId === "R2", "another room");
        assert.ok(!root.atStart, "another room is not the start");
        assert.ok(moqui.back.pop(), "Back goes to the start room");
        await until(() => root.atStart && root.currentParameters.chatRoomId === "R1", "the start room again");
        // the room a dm link of the start resolves to takes its place (replaceUrl), so Back there leaves the app
        root.replaceUrl("/qapps/BasicComms/Chat?chatRoomId=R2");
        assert.ok(root.atStart, "what replaced the start screen is the start");
        assert.equal(moqui.back.pop(), false);
    } finally {
        shellState.startPath = "/qapps/BasicComms/Chat";
    }
});
})();
