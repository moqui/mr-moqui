/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
"use strict";
(function () {
const until = async (check, what, ms) => {
    const end = Date.now() + (ms || 5000);
    for (;;) {
        const value = check();
        if (value) return value;
        if (Date.now() > end) throw new Error("waited for " + what);
        await new Promise((resolve) => setTimeout(resolve, 25));
    }
};
const text = () => document.body.textContent;
const offlineHeaders = { get: (name) => (name === "X-Moqui-Offline" ? "1" : null) };

// the server's answers as the host gives them offline: from the account's store, marked X-Moqui-Offline; the rest 502
const room = { chatRoomId: "R1", roomTypeEnumId: "CrtPrivate", roomName: "Ops", displayName: "Ops", lastEventSeq: 5, isMember: true, unreadCount: 0 };
const message = { eventSeq: 5, type: "message", senderUserId: "bob", senderName: "Bob", sentDate: Date.now() - 60000, body: "<p>kept here</p>", bodyText: "kept here",
    reactions: [], attachments: [] };
const answers = {
    "/qapps/clientConfig": { userId: "ann", username: "ann", userFullName: "Ann", locale: "en", server: { name: "Test Server", clientApi: 3 },
        calls: { appRoot: "/qapps/BasicComms" }, notificationEndpoint: "/notws" },
    "/shellData": { apps: [], counts: {} },
    "/apps/l10nCatalog": {},
    "/apps/BasicComms/Chat/chatData": { me: { userId: "ann" }, prefs: {}, roomList: [room], offline: true },
    "/apps/BasicComms/Chat/roomView": { room, member: { lastReadEventSeq: 5 }, access: { isMember: true, canPost: true }, memberList: [{ userId: "ann", name: "Ann" }, { userId: "bob", name: "Bob" }],
        timeline: { eventList: [message], hasMoreBefore: false, hasMoreAfter: false, offline: { gapBefore: true, gapAfter: false } }, offline: true },
    "/apps/BasicComms/Inbox/Mail/inboxData": { accountList: [], mailboxList: [], searchList: [], totals: {}, me: { userId: "ann" }, prefs: {} },
    "/apps/BasicComms/Inbox/Mail/threads": { threadList: [], hasMore: false, nextCursor: null, storeList: ["email"], failedStores: [], termList: [], offline: true },
    "/apps/BasicComms/Inbox/Compose/composeData": { compose: { identityList: [{ value: "acct1", label: "ann@example.com" }], fromValue: "acct1", to: [], kind: "new" } }
};
// what the host's store does not hold: another room, a search of the mail
const notKept = (url) => /chatRoomId=R2/.test(url) || (/\/threads\b/.test(url) && /[?&]q=/.test(url))
    || (host.composeNotKept && /\/composeData\b/.test(url));
const unreachableHeaders = { get: (name) => (name === "X-Moqui-Offline" ? "unreachable" : null) };
const requests = [];
const serve = (desktop) => (url, options) => {
    const path = String(url).split("?")[0];
    const query = moqui.objToSearch((options && options.params && moqui.isPlainObject(options.params)) ? options.params : {});
    requests.push(path + " " + ((options && options.method) || "GET"));
    if (desktop && answers[path] && !notKept(url + "?" + query)) {
        moqui.noteAnswer(200, offlineHeaders);
        const json = JSON.parse(JSON.stringify(answers[path]));
        if (host.composeOffline && /\/composeData$/.test(path)) json.offline = host.composeOffline;
        return Promise.resolve({ ok: true, status: 200, headers: offlineHeaders, json, text: "" });
    }
    // the desktop client's host marks its own 502 (the server out of reach); a 502 of a server's (the web) is not
    const headers = desktop ? unreachableHeaders : { get: () => null };
    moqui.noteAnswer(502, headers);
    return Promise.reject({ ok: false, status: 502, headers, text: "The server could not be reached", json: undefined });
};

// the host: a shared device's account signed in, with a chat message and a mail that did not go yet and a draft
const ops = [
    { id: "c1", kind: "chat-send", target: "R1", state: "failed", attempts: 3, error: "Join the room to post in it", createdMs: 1, payload: { chatRoomId: "R1", text: "not yet sent", threadRootEventSeq: null } },
    { id: "m1", kind: "mail-send", target: "m1", state: "queued", attempts: 0, error: null, createdMs: 2,
        payload: { clientRef: "m1", fields: { subject: "Quarterly report", toJson: JSON.stringify([{ value: "cy@example.com", label: "Cy" }]) }, files: [] } }
];
const host = { opened: [], signedOut: false, retried: [], discarded: [], listeners: [], pendingOpen: null, unsentFails: false };
const outbox = {
    add: (kind, payload) => { ops.push({ id: "q" + ops.length, kind, state: "queued", payload, createdMs: Date.now() }); host.listeners.forEach((fn) => fn({ kind: "changed" })); return Promise.resolve({}); },
    list: () => Promise.resolve(ops.map((op) => Object.assign({}, op))),
    retry: (id) => { host.retried.push(id); return Promise.resolve(true); },
    discard: (id) => { host.discarded.push(id); return Promise.resolve(true); },
    on: (fn) => { host.listeners.push(fn); return () => {}; },
    unsent: () => (host.unsentFails ? Promise.reject(new Error("The store is not open"))
        : Promise.resolve({ operations: ops.map((op) => Object.assign({}, op)), drafts: [{ key: "boss.chat.draft.R2", updatedMs: 1 }] }))
};
// the state the host gives (reactive here, so a test can change the account's device mode)
const shellState = window.Vue.reactive({ accounts: [{ id: "a1", username: "ann", server: "https://s.example.com", deviceMode: "shared" }], current: "a1",
    signedIn: true, keyStore: "os", clientApi: 3, deviceMode: "choose", loadToken: null });
const desktopHost = {
    state: () => Promise.resolve(shellState),
    // the link the host keeps for the shell, taken once (src/client/windows.rs open_page)
    takeOpen: () => { const path = host.pendingOpen; host.pendingOpen = null; return Promise.resolve(path); },
    clearScopes: () => Promise.resolve(),
    socket: () => ({ readyState: 0, send() {}, close() {} }),
    outbox,
    drafts: { get: () => Promise.resolve(null), list: () => Promise.resolve([]), put: () => Promise.resolve(), remove: () => Promise.resolve(true), saveFile: () => Promise.resolve(null) },
    signOut: () => { host.signedOut = true; return Promise.resolve(); },
    openRemote: (path) => { host.opened.push(path); return Promise.resolve(); },
    setCounts() {}, setPresence() {}, storageScope: "a1"
};

test("the shell shows Chat, the Inbox and Compose itself, offline, with what waits and what needs a connection", async () => {
    const shell = document.createElement("div");
    shell.id = "shell";
    document.body.appendChild(shell);
    window.moquiDesktop = desktopHost;
    moqui.request = serve(true);
    // a room opened (a notification) while the shell was loading: kept by the host, taken as the shell mounts
    host.pendingOpen = "/qapps/BasicComms/Chat?chatRoomId=R1";
    await moqui.loadScript("/files/crates/moqui-desktop/shell/js/shell.js");
    const root = await until(() => moqui.webrootVue && moqui.webrootVue.commsItems && moqui.webrootVue, "the shell's root");
    // the start came from the store: offline, and a shared device says what it keeps
    await until(() => document.querySelector(".m-shell-offline"), "the offline chip");
    assert.equal(moqui.connection.offline, true);
    assert.ok(document.querySelector(".m-shell-shared-hint"), "the shared device's hint");
    assert.deepEqual(root.commsItems.map((item) => item.view), ["chat", "mail", "compose"], "the bundled comms screens, in the drawer");

    // that room: Chat here, its requests to the comms app's transitions
    await until(() => document.querySelector(".m-chat-gap[data-gap='before']"), "the gap line before the kept messages");
    assert.equal(root.view, "chat");
    assert.deepEqual(root.currentPathList, ["BasicComms", "Chat"]);
    assert.ok(text().includes("kept here"));
    assert.ok(text().includes("Older messages load when the server can be reached"));
    assert.ok(requests.includes("/apps/BasicComms/Chat/roomView GET"));
    // the outbox's message of the room, refused, with Retry and Delete
    const pending = await until(() => document.querySelector(".m-chat-pending[data-pending='failed']"), "the message not sent");
    assert.ok(pending.textContent.includes("not yet sent"));
    assert.ok(pending.textContent.includes("Join the room to post in it"));
    assert.ok(pending.textContent.includes("Retry"));
    // an action that needs the server says so, and asks nothing
    const before = requests.length;
    document.querySelector("button[aria-label='Search in the room']").click();
    await until(() => text().includes("This needs a connection to the server"), "needs a connection");
    assert.equal(requests.length, before);

    assert.equal(host.pendingOpen, null, "taken once");

    // a room this device did not keep, from the host's poke: it says so, not that it does not exist
    host.pendingOpen = "/qapps/BasicComms/Chat?chatRoomId=R2";
    window.moquiShell.poke();
    await until(() => document.querySelector(".m-chat-unreachable"), "the room not kept");
    assert.equal(root.currentParameters.chatRoomId, "R2");
    assert.ok(!text().includes("This conversation is not available"));

    // the Inbox: its outbox of mail waiting, offline
    host.pendingOpen = "/qapps/BasicComms/Inbox";
    window.moquiShell.poke();
    const card = await until(() => document.querySelector(".m-inbox-outbox"), "the Inbox's outbox");
    assert.equal(root.view, "mail");
    assert.ok(card.textContent.includes("Quarterly report"));
    assert.ok(card.textContent.includes("To Cy"));
    assert.ok(document.querySelector(".m-inbox-offline"), "the offline banner");
    card.querySelector(".m-inbox-outbox-item button").click();
    await until(() => host.retried.length, "a retry through the host");
    assert.deepEqual(host.retried, ["m1"]);

    // a thread change offline waits in the host's outbox as absolute values, and asks the server nothing
    const inbox = root.$refs.view;
    const asked = requests.length;
    const changed = await inbox.post("state", { targetsJson: JSON.stringify([{ store: "email", threadId: "T1", rowKey: "email:T1" }]), action: "archive" });
    assert.equal(changed.queued, true);
    const queued = ops[ops.length - 1];
    assert.equal(queued.kind, "mail-state");
    assert.deepEqual(queued.payload, { targets: [{ store: "email", threadId: "T1", rowKey: "email:T1" }], action: "archive" });
    assert.equal(requests.slice(asked).filter((line) => line.includes("/state")).length, 0);
    // another change needs the server
    await assert.rejects(inbox.post("state", { targetsJson: "[]", action: "snooze" }), (err) => err.offline === true);
    await until(() => document.querySelector(".m-inbox-outbox").textContent.includes("1 change waits to be made"), "the change waiting");
    // back online with that change still waiting: the next one waits behind it, in order
    moqui.setOffline(false);
    const before2 = requests.length;
    const behind = await inbox.post("state", { targetsJson: JSON.stringify([{ store: "email", threadId: "T2", rowKey: "email:T2" }]), action: "read" });
    assert.equal(behind.queued, true);
    assert.equal(requests.slice(before2).filter((line) => line.includes("/state")).length, 0);
    assert.equal(ops[ops.length - 1].payload.action, "read");
    moqui.setOffline(true);
    // a search the device did not keep: said so, not an error
    root.setUrl("/qapps/BasicComms/Inbox/Mail?q=report");
    await until(() => text().includes("This list is not kept on this device"), "the search not kept");
    // the screen that is not bundled opens in an app window, under /qapps
    root.setUrl("/apps/BasicComms/Calls/History?direction=in");
    assert.deepEqual(host.opened, ["/qapps/BasicComms/Calls/History?direction=in"]);
    assert.equal(root.view, "mail", "the shell stays where it was");
    // Back, through the search and the Inbox to the rooms, then home (the panel)
    root.goBack();
    assert.equal(root.view, "mail");
    root.goBack();
    assert.equal(root.view, "chat");
    root.goBack();
    root.goBack();
    assert.equal(root.view, null);

    // Compose for a mailto: link, filled in, with its offline banner
    host.pendingOpen = "/qapps/BasicComms/Inbox/Compose?to=dee%40example.com&subject=Hello";
    window.moquiShell.poke();
    const banner = await until(() => document.querySelector(".m-compose-offline"), "Compose's offline banner");
    assert.equal(root.view, "compose");
    assert.equal(root.currentParameters.subject, "Hello");
    assert.ok(banner.textContent.includes("this email waits in the outbox"));
    // a forward built offline says the files it leaves out (only the server copies them)
    host.composeOffline = { forwardFilesMissing: 2 };
    host.pendingOpen = "/qapps/BasicComms/Inbox/Compose?forwardEmailMessageId=F1";
    window.moquiShell.poke();
    await until(() => text().includes("Its 2 files are not attached"), "the forward's files left out");
    host.composeOffline = null;
    // nothing kept to start a reply from (no composeData in the store): a new email answering it, not an error
    host.composeNotKept = true;
    host.pendingOpen = "/qapps/BasicComms/Inbox/Compose?replyToEmailMessageId=GONE&replyAll=false";
    window.moquiShell.poke();
    await until(() => text().includes("The message this answers is not kept on this computer"), "Compose's blank reply offline");
    assert.ok(document.querySelector(".m-compose-offline"), "with its offline banner");
    assert.ok(!text().includes("This message cannot be written here"));
    assert.ok(document.querySelector(".m-compose").textContent.includes("Subject"), "a message to write");
    // and no mail account known to send from: said, and Send is off
    assert.ok(text().includes("No mail account is known on this computer yet"));
    const sendButton = Array.from(document.querySelectorAll(".m-compose button")).find((button) => button.textContent.includes("Send"));
    assert.ok(sendButton && (sendButton.disabled || sendButton.classList.contains("disabled")), "Send is off");
    // a draft needs the server: said so, not that it cannot be written
    host.pendingOpen = "/qapps/BasicComms/Inbox/Compose?emailMessageId=D1";
    window.moquiShell.poke();
    await until(() => text().includes("This needs a connection to the server; open it again"), "the draft needing a connection");
    assert.ok(!text().includes("This message cannot be written here"));
    host.composeNotKept = false;

    // signing out a shared account lists what waits unsent first; Sign out anyway signs out
    root.signOut();
    const unsent = await until(() => document.querySelector(".m-shell-unsent"), "the unsent items");
    assert.ok(unsent.textContent.includes("not yet sent"));
    assert.ok(unsent.textContent.includes("Quarterly report"));
    assert.ok(unsent.textContent.includes("1 draft"));
    assert.ok(text().includes("Not sent yet: signing out deletes these."));
    const anyway = Array.from(document.querySelectorAll("button")).find((button) => button.textContent.includes("Sign out anyway"));
    anyway.click();
    await until(() => host.signedOut, "the sign-out");
    host.signedOut = false;
    // unsent items that could not be read: said, and for a shared device that any are deleted
    host.unsentFails = true;
    root.signOut();
    await until(() => document.querySelector(".m-shell-unsent-unknown"), "could not check");
    assert.ok(text().includes("any of it is deleted with the sign-out"));
    Array.from(document.querySelectorAll("button")).find((button) => button.textContent.includes("Stay signed in")).click();
    await until(() => !document.querySelector(".m-shell-unsent-unknown"), "the dialog closed");
    assert.equal(host.signedOut, false, "stayed signed in");
    // a personal device's account keeps them for the next sign-in
    host.unsentFails = false;
    shellState.accounts[0].deviceMode = "personal";
    root.signOut();
    await until(() => document.querySelector(".m-shell-unsent"), "the unsent items, personal");
    assert.ok(text().includes("Not sent yet: these stay on this computer and go after you sign in again."));
    assert.ok(!text().includes("signing out deletes these"));
    Array.from(document.querySelectorAll("button")).find((button) => button.textContent.includes("Stay signed in")).click();
});

test("with no host, as on the web, the same screens show no offline or outbox presentation", async () => {
    delete window.moquiDesktop;
    moqui.setOffline(true);
    assert.equal(moqui.connection.offline, false, "never offline without the desktop client's host");
    if (moqui.chat) moqui.chat.state.outbox = [];
    moqui.request = serve(false);
    // the server is not there: its requests fail as the web tells them (handleAjaxError), kept out of the page's problems
    const warn = console.warn;
    const warned = [];
    console.warn = (...args) => { if (/request failed, status 502/.test(String(args[0]))) warned.push(args[0]); else warn.apply(console, args); };
    try {
    const Vue = window.Vue;
    const mountScreen = (name, pathList, parameters) => {
        const el = document.createElement("div");
        document.body.appendChild(el);
        const screen = moqui.bundledComponent(name);
        const app = Vue.createApp({
            data() { return { basePath: "/apps", linkBasePath: "/qapps", appRootPath: "", currentPathList: pathList, currentParameters: parameters, shellCounts: {},
                quickCreateShow: false, userId: "ann" }; },
            computed: { currentLinkPath() { return "/qapps/" + this.currentPathList.join("/"); } },
            methods: { setUrl() {}, setParameters() {}, addContainer() {}, removeContainer() {} },
            render() { return Vue.h(screen); }
        });
        app.use(window.Quasar, { plugins: moqui.qvt.quasarPlugins(window.Quasar) });
        Object.keys(moqui.qvt.components).forEach((component) => app.component(component, moqui.qvt.components[component]));
        app.config.globalProperties.moqui = moqui;
        app.mount(el);
        return { app, el };
    };
    const chatScreen = mountScreen("BasicComms/Chat", ["BasicComms", "Chat"], { chatRoomId: "R1" });
    await until(() => chatScreen.el.textContent.includes("This conversation is not available"), "the room, not reached");
    assert.equal(chatScreen.el.querySelector(".m-chat-unreachable"), null);
    assert.equal(chatScreen.el.querySelector(".m-chat-gap"), null);
    const mail = mountScreen("BasicComms/Inbox/Mail", ["BasicComms", "Inbox", "Mail"], {});
    await until(() => requests.includes("/apps/BasicComms/Inbox/Mail/threads GET") && !mail.el.querySelector(".q-skeleton"), "the list asked for");
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(mail.el.querySelector(".m-inbox-outbox"), null);
    assert.equal(mail.el.querySelector(".m-inbox-offline"), null);
    assert.ok(!mail.el.textContent.includes("not kept on this device"), "a list that did not load is not an offline one");
    // Compose with no answer: on the web it cannot be written, as before (no blank message offline)
    const compose = mountScreen("BasicComms/Inbox/Compose", ["BasicComms", "Inbox", "Compose"], {});
    await until(() => compose.el.textContent.includes("This message cannot be written here"), "Compose not reached, on the web");
    assert.equal(compose.el.querySelector(".m-compose-offline"), null);
    assert.ok(warned.length > 0, "the web told of the failed requests");
    chatScreen.app.unmount();
    mail.app.unmount();
    compose.app.unmount();
    } finally { console.warn = warn; }
});

test("an app window leaves the crypto device to the shell: no device, and an encrypted room opens in Chat; the web is as it was", async () => {
    const Vue = window.Vue;
    const sealedRoom = Object.assign({}, room, { chatRoomId: "R3", displayName: "Sealed", roomName: "Sealed", encrypted: true });
    moqui.request = (url) => {
        const path = String(url).split("?")[0];
        if (path === "/apps/BasicComms/Chat/chatData") return Promise.resolve({ ok: true, status: 200, json: { me: { userId: "ann" }, prefs: {}, roomList: [sealedRoom] } });
        if (path === "/apps/BasicComms/Chat/roomView") return Promise.resolve({ ok: true, status: 200, json: { room: sealedRoom, member: { lastReadEventSeq: 5 },
            access: { isMember: true, canPost: true }, memberList: [{ userId: "ann", name: "Ann" }], timeline: { eventList: [], hasMoreBefore: false, hasMoreAfter: false } } });
        return Promise.reject({ ok: false, status: 404, headers: { get: () => null } });
    };
    const mountRoom = () => {
        const el = document.createElement("div");
        document.body.appendChild(el);
        const screen = moqui.bundledComponent("BasicComms/Chat");
        const app = Vue.createApp({
            data() { return { basePath: "/apps", linkBasePath: "/qapps", appRootPath: "", currentPathList: ["BasicComms", "Chat"], currentParameters: { chatRoomId: "R3" },
                shellCounts: {}, quickCreateShow: false, userId: "ann" }; },
            methods: { setUrl() {}, setParameters() {}, addContainer() {}, removeContainer() {} },
            render() { return Vue.h(screen); }
        });
        app.use(window.Quasar, { plugins: moqui.qvt.quasarPlugins(window.Quasar) });
        Object.keys(moqui.qvt.components).forEach((component) => app.component(component, moqui.qvt.components[component]));
        app.config.globalProperties.moqui = moqui;
        app.mount(el);
        return { app, el };
    };
    try {
        // the web: no flag, so the page may have a device (here no user is signed in, so none starts), and the room is shown
        delete window.__moquiCryptoInShell;
        delete window.moquiDesktop;
        assert.equal(moqui.crypto.elsewhere(), false);
        await assert.rejects(moqui.crypto.ensureDevice(), (err) => err.reasonCode !== "elsewhere");
        const web = mountRoom();
        await until(() => web.el.querySelector(".m-chat-composer"), "the encrypted room, as the web shows it");
        assert.equal(web.el.querySelector(".m-chat-crypto-elsewhere"), null);
        web.app.unmount();

        // an app window of the desktop client: the host's flag; the device is refused, the room says it opens in Chat
        const asked = [];
        window.__moquiCryptoInShell = true;
        window.moquiDesktop = { storageScope: "a1", openInShell: (path) => { asked.push(path); return Promise.resolve(); } };
        assert.equal(moqui.crypto.elsewhere(), true);
        await assert.rejects(moqui.crypto.ensureDevice(), (err) => err.reasonCode === "elsewhere");
        await assert.rejects(moqui.crypto.encryptRoom("R3", ["ann"], "m.room.message", { body: "x" }), (err) => err.reasonCode === "elsewhere");
        assert.equal(moqui.crypto.state.deviceId, null, "no device made");
        const appWindow = mountRoom();
        const block = await until(() => appWindow.el.querySelector(".m-chat-crypto-elsewhere"), "Open in Chat");
        assert.ok(block.textContent.includes("Encrypted messages open in Chat"));
        assert.equal(appWindow.el.querySelector(".m-chat-composer"), null, "no composer that could not encrypt");
        block.querySelector("button").click();
        await until(() => asked.length, "the host asked");
        assert.deepEqual(asked, ["/qapps/BasicComms/Chat?chatRoomId=R3"]);
        appWindow.app.unmount();
    } finally {
        delete window.__moquiCryptoInShell;
        delete window.moquiDesktop;
    }
});
})();
