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
const settle = () => new Promise((resolve) => setTimeout(resolve, 80));
const text = () => document.body.textContent;
const clone = (value) => JSON.parse(JSON.stringify(value));
// the rows of the room's timeline that show a text
const rows = (needle) => Array.from(document.querySelectorAll(".m-chat-msg")).filter((row) => row.textContent.includes(needle)).length;
const button = (scope, label) => Array.from((scope || document).querySelectorAll("button")).find((el) => el.textContent.trim() === label);
const dialogButton = (label) => until(() => button(document.querySelector(".q-dialog"), label), "the dialog's " + label);

// ---- the server, as the host passes it on: online its answers, offline the store's (X-Moqui-Offline) and no POST ----
const plainHeaders = { get: () => null };
const offlineHeaders = { get: (name) => (name === "X-Moqui-Offline" ? "1" : null) };
const unreachableHeaders = { get: (name) => (name === "X-Moqui-Offline" ? "unreachable" : null) };
const message = (seq, body, extra) => Object.assign({ eventSeq: seq, type: "message", senderUserId: "bob", senderName: "Bob", sentDate: Date.now() - 600000 + seq * 1000,
    body: "<p>" + body + "</p>", bodyText: body, reactions: [], attachments: [] }, extra || {});
const room = { chatRoomId: "R1", roomTypeEnumId: "CrtPrivate", roomName: "Ops", displayName: "Ops", lastEventSeq: 6, isMember: true, unreadCount: 0 };
// what the account's store holds of the room (the changes feeds keep it)
const server = {
    online: true,
    events: [message(4, "an older one"), message(5, "kept here"), message(6, "my own words", { senderUserId: "ann", senderName: "Ann" })],
    compose: null, saveError: null
};
// an encrypted room, its messages as the store keeps them (decrypted in the page: moqui.chat.openEvents, stood in for)
const sealedRoom = Object.assign({}, room, { chatRoomId: "R3", roomName: "Sealed", displayName: "Sealed", encrypted: true, lastEventSeq: 3 });
const sealedEvents = [message(2, "sealed one", { encrypted: true }), message(3, "sealed two", { encrypted: true })];
const roomOf = (params) => (params.chatRoomId === "R3" ? sealedRoom : room);
const timelineOf = (params) => {
    let list = params.chatRoomId === "R3" ? sealedEvents : server.events;
    if (params.eventSeq != null && params.eventSeq !== "") list = list.filter((event) => Number(event.eventSeq) === Number(params.eventSeq));
    else if (params.afterSeq != null && params.afterSeq !== "") list = list.filter((event) => Number(event.eventSeq) > Number(params.afterSeq));
    return { eventList: clone(list), hasMoreBefore: false, hasMoreAfter: false };
};
const answers = {
    "/qapps/clientConfig": () => ({ userId: "ann", username: "ann", userFullName: "Ann", locale: "en", server: { name: "Test Server", clientApi: 4 },
        calls: { appRoot: "/qapps/BasicComms" }, notificationEndpoint: "/notws" }),
    "/shellData": () => ({ apps: [], counts: {} }),
    "/apps/l10nCatalog": () => ({}),
    "/apps/BasicComms/Chat/chatData": () => ({ me: { userId: "ann" }, prefs: {}, roomList: [room, sealedRoom] }),
    "/apps/BasicComms/Chat/roomView": (params) => ({ room: roomOf(params), member: { lastReadEventSeq: 6 }, access: { isMember: true, canPost: true },
        memberList: [{ userId: "ann", name: "Ann" }, { userId: "bob", name: "Bob" }], timeline: timelineOf({ chatRoomId: params.chatRoomId }) }),
    "/apps/BasicComms/Chat/timeline": timelineOf,
    "/apps/BasicComms/Chat/edit": (params) => ({ event: { type: "edit", eventSeq: 20, editsEventSeq: Number(params.eventSeq), body: params.body,
        bodyText: String(params.body).replace(/<[^>]*>/g, ""), senderUserId: "ann", sentDate: Date.now() } }),
    "/apps/BasicComms/Inbox/Mail/inboxData": () => ({ accountList: [], mailboxList: [], searchList: [], totals: {}, me: { userId: "ann" }, prefs: {} }),
    "/apps/BasicComms/Inbox/Mail/threads": () => ({ threadList: [], hasMore: false, nextCursor: null, storeList: ["email"], failedStores: [], termList: [] }),
    "/apps/BasicComms/Inbox/Compose/composeData": () => ({ compose: clone(server.compose || { identityList: [{ value: "acct1", label: "ann@example.com" }],
        fromValue: "acct1", to: [], kind: "new" }) }),
    "/apps/BasicComms/Inbox/Compose/saveDraft": () => ({ emailMessageId: "E9", draftStamp: 77 })
};
const requests = [];
const asked = (path, method) => requests.filter((request) => request.path === path && request.method === (method || "GET"));
moqui.request = (url, options) => {
    const path = String(url).split("?")[0];
    const method = (options && options.method) || "GET";
    const params = options && moqui.isPlainObject(options.params) ? options.params : {};
    const request = { path, method, params, events: 0 };
    requests.push(request);
    const answer = answers[path];
    if (!server.online && method !== "GET") {
        moqui.noteAnswer(502, unreachableHeaders);
        return Promise.reject({ ok: false, status: 502, headers: unreachableHeaders, text: "The server could not be reached", json: undefined });
    }
    if (path === "/apps/BasicComms/Inbox/Compose/saveDraft" && server.saveError) {
        return Promise.reject({ ok: false, status: 503, headers: plainHeaders, json: { errors: server.saveError } });
    }
    const headers = server.online ? plainHeaders : offlineHeaders;
    moqui.noteAnswer(200, headers);
    const json = answer ? answer(params) : {};
    // how many events the answer has, for a check of how often a page decrypts them
    request.events = ((json.eventList || (json.timeline && json.timeline.eventList)) || []).length;
    return Promise.resolve({ ok: true, status: 200, headers, json, text: "" });
};

// ---- the host: its outbox (the test moves an operation through its states, as the flusher does), drafts and /notws ----
const MD4_KINDS = ["chat-send", "chat-read", "mail-send", "mail-state"];
const MD5_KINDS = MD4_KINDS.concat(["chat-edit", "chat-redact", "chat-react", "poll-vote", "internal-send"]);
const ops = [];
const listeners = [];
const host = { kinds: { clientApi: 3, kinds: MD4_KINDS, internalSend: false }, added: [], retried: [], discarded: [], pendingOpen: null };
const tell = (event) => listeners.slice().forEach((fn) => fn(event || { kind: "changed" }));
const opOf = (id) => ops.find((op) => op.id === id);
// an operation moved on by the host (its state, error, refusal or conflict), and the pages told
const move = (id, fields) => { Object.assign(opOf(id), fields); tell(); };
const drop = (id) => { const at = ops.findIndex((op) => op.id === id); if (at >= 0) ops.splice(at, 1); };
// a message's change answered, told as the flusher tells it: changed first (the pages' list no longer has it), then sent,
// which names its room and message
const answered = async (op, json) => {
    tell();
    await settle();
    tell({ kind: "sent", op: op.kind, clientRef: op.id, chatRoomId: op.payload.chatRoomId, eventSeq: op.payload.eventSeq, threadRootEventSeq: null, json });
};
let added = 0;
const outbox = {
    add: (kind, payload, files) => {
        let taken = kind;
        if (kind === "mail-send" && payload.fields && payload.fields.fromValue === "internal") {
            if (!host.kinds.internalSend) return Promise.reject(new Error("An internal message needs a connection to send"));
            taken = "internal-send";
        }
        if (!host.kinds.kinds.includes(taken)) return Promise.reject(new Error(taken + " needs a connection to this server"));
        added += 1;
        const op = { id: payload.clientRef || "q" + added, kind: taken, state: "queued", attempts: 0, error: null, status: null, rejected: false, current: null,
            createdMs: Date.now() + added, payload: clone(payload), files: files || [] };
        ops.push(op);
        host.added.push(op);
        setTimeout(() => tell(), 0);
        return Promise.resolve({});
    },
    list: () => Promise.resolve(clone(ops)),
    kinds: () => Promise.resolve(clone(host.kinds)),
    retry: (id) => { host.retried.push(id); move(id, { state: "retry", error: null, status: null, rejected: false, attempts: 0 }); return Promise.resolve(true); },
    discard: (id) => { host.discarded.push(id); drop(id); tell(); return Promise.resolve(true); },
    on: (fn) => { listeners.push(fn); return () => {}; },
    unsent: () => Promise.resolve({ operations: clone(ops), drafts: [] })
};
const kept = new Map();
const draftOf = (key) => ({ key, value: kept.get(key), files: [], updatedMs: 1 });
const drafts = {
    get: (key) => Promise.resolve(kept.has(key) ? draftOf(key) : null),
    list: (prefix) => Promise.resolve(Array.from(kept.keys()).filter((key) => key.indexOf(prefix) === 0).map(draftOf)),
    put: (key, value) => { kept.set(key, String(value)); return Promise.resolve(); },
    remove: (key) => { kept.delete(key); return Promise.resolve(true); },
    saveFile: () => Promise.resolve(null)
};
// the account's /notws, held by the host: the test opens and closes it and brings its pushes
let socket = null;
const openSocket = () => { socket.readyState = 1; if (socket.onopen) socket.onopen(); };
const closeSocket = () => { socket.readyState = 3; if (socket.onclose) socket.onclose(); };
const push = (event) => socket.onmessage({ data: JSON.stringify({ topic: "chatRoom:R1", message: { kind: "event", chatRoomId: "R1", event } }) });
const shellState = window.Vue.reactive({ accounts: [{ id: "a1", username: "ann", server: "https://s.example.com", deviceMode: "personal" }], current: "a1",
    signedIn: true, keyStore: "os", clientApi: 4, deviceMode: "choose", loadToken: null });
window.moquiDesktop = {
    state: () => Promise.resolve(shellState),
    takeOpen: () => { const path = host.pendingOpen; host.pendingOpen = null; return Promise.resolve(path); },
    clearScopes: () => Promise.resolve(),
    socket: () => { socket = { readyState: 0, send() {}, close() {} }; return socket; },
    outbox, drafts,
    signOut: () => Promise.resolve(),
    openRemote: () => Promise.resolve(),
    setCounts() {}, setPresence() {}, storageScope: "a1"
};

// the shell, mounted once; open(path) shows a bundled screen as a link from the host does
let root = null;
const open = async (path, view) => {
    if (!root) {
        const shell = document.createElement("div");
        shell.id = "shell";
        document.body.appendChild(shell);
        host.pendingOpen = path;
        await moqui.loadScript("/files/crates/moqui-desktop/shell/js/shell.js");
        root = await until(() => moqui.webrootVue && moqui.webrootVue.commsItems && moqui.webrootVue, "the shell's root");
    } else {
        host.pendingOpen = path;
        window.moquiShell.poke();
    }
    await until(() => root.view === view && root.$refs.view, "the " + view + " screen");
    return root.$refs.view;
};
const openRoom = async () => {
    const chat = await open("/qapps/BasicComms/Chat?chatRoomId=R1", "chat");
    const vm = await until(() => chat.$refs.room && chat.$refs.room.roomId === "R1" && chat.$refs.room.state === "ready" && chat.$refs.room, "the room");
    await until(() => document.querySelector(".m-chat-msg"), "the room's messages");
    if (socket && socket.readyState !== 1) openSocket();
    return vm;
};
const eventOf = (vm, seq) => vm.events.find((event) => Number(event.eventSeq) === seq);
const change = (kind, pending) => document.querySelector(".m-chat-change[data-change='" + kind + "']" + (pending ? "[data-pending='" + pending + "']" : ""));

test("a message's edit, reaction and deletion wait in the app's outbox where the server takes them once; one refused says why, with Retry and Discard", async () => {
    const vm = await openRoom();
    // a server below clientApi 4: the edit is asked of the server now, as before
    await vm.saveEdit({ eventSeq: 6, event: eventOf(vm, 6) }, "<p>first fix</p>");
    assert.equal(asked("/apps/BasicComms/Chat/edit", "POST").length, 1);
    assert.equal(host.added.length, 0);
    await until(() => text().includes("first fix"), "the edit answered");

    // the server takes a message's changes once by a clientRef: they go through the outbox and show as waiting
    host.kinds = { clientApi: 4, kinds: MD5_KINDS, internalSend: true };
    await moqui.chat.readKinds(window.moquiDesktop.outbox);
    await vm.saveEdit({ eventSeq: 6, event: eventOf(vm, 6) }, "<p>second fix</p>");
    assert.equal(asked("/apps/BasicComms/Chat/edit", "POST").length, 1, "not asked of the server");
    const edit = host.added[host.added.length - 1];
    assert.equal(edit.kind, "chat-edit");
    assert.deepEqual([edit.payload.chatRoomId, edit.payload.eventSeq, edit.payload.body], ["R1", 6, "<p>second fix</p>"]);
    assert.ok(edit.payload.clientRef, "taken once by its clientRef");
    const waiting = await until(() => change("chat-edit", "waiting"), "the edit waiting");
    assert.ok(waiting.textContent.includes("Edit: waiting to send"));
    assert.ok(!text().includes("second fix"), "shown when the server has it");
    vm.react({ eventSeq: 5, event: eventOf(vm, 5) }, "👍");
    const react = await until(() => host.added.find((op) => op.kind === "chat-react"), "the reaction queued");
    assert.equal(react.payload.key, "👍");
    await until(() => change("chat-react", "waiting"), "the reaction waiting");

    // the edit answered, told as the host tells it: changed (the list without it) first, then sent, which names its message
    drop(edit.id);
    await answered(edit, { event: { type: "edit", eventSeq: 21, editsEventSeq: 6, body: "<p>second fix</p>",
        bodyText: "second fix", senderUserId: "ann", sentDate: Date.now() } });
    await until(() => text().includes("second fix") && !change("chat-edit"), "the edit made");

    // a reaction taken back while it still waits is dropped from the outbox, never sent
    vm.react({ eventSeq: 4, event: eventOf(vm, 4) }, "🎉");
    const party = await until(() => host.added.find((op) => op.kind === "chat-react" && op.payload.key === "🎉"), "the second reaction queued");
    await until(() => change("chat-react", "waiting") && opOf(party.id), "it waits");
    const reacts = host.added.filter((op) => op.kind === "chat-react").length;
    vm.react({ eventSeq: 4, event: eventOf(vm, 4) }, "🎉");
    await until(() => host.discarded.includes(party.id), "the waiting reaction dropped");
    assert.equal(host.added.filter((op) => op.kind === "chat-react").length, reacts, "no reaction queued to take it back");
    assert.ok(!(eventOf(vm, 4).reactions || []).some((reaction) => reaction.key === "🎉" && reaction.mine));

    // the reaction refused (a 4xx): not made, with the server's reason; Retry sends it again, Discard drops it
    move(react.id, { state: "failed", error: "Reactions are off in this room", status: 403, rejected: true });
    const refused = await until(() => change("chat-react", "failed"), "the reaction refused");
    assert.ok(refused.textContent.includes("Reaction not made: Reactions are off in this room"));
    button(refused, "Retry").click();
    await until(() => host.retried.includes(react.id) && change("chat-react", "waiting"), "the reaction tried again");
    move(react.id, { state: "failed", error: "Reactions are off in this room", status: 403, rejected: true });
    button(await until(() => change("chat-react", "failed"), "refused again"), "Discard").click();
    await until(() => host.discarded.includes(react.id) && !change("chat-react"), "the reaction discarded");
    // the reaction shown at once is read again as the store has it: none
    await until(() => asked("/apps/BasicComms/Chat/timeline").some((request) => Number(request.params.eventSeq) === 5), "the message read again");
    await until(() => !(eventOf(vm, 5).reactions || []).length, "no reaction left");

    // a deletion waits too, the message shown until the server has removed it
    vm.redact({ eventSeq: 6, event: eventOf(vm, 6) });
    (await dialogButton("Delete")).click();
    const redact = await until(() => host.added.find((op) => op.kind === "chat-redact"), "the deletion queued");
    await until(() => change("chat-redact", "waiting"), "the deletion waiting");
    assert.ok(text().includes("second fix"));
    drop(redact.id);
    await answered(redact, {});
    await until(() => !text().includes("second fix") && !change("chat-redact"), "the message removed");
});

test("a lost answer then a retry shows one message; a push that comes twice shows once; a reconnect after a feed reset reads the room as the store has it", async () => {
    const vm = await openRoom();
    // sent through the outbox; its answer is lost (sending, then waiting to try again)
    const sent = await moqui.chat.send({ chatRoomId: "R1", url: "/apps/BasicComms/Chat/send", params: { body: "<p>sent once</p>" }, text: "sent once", threadRootEventSeq: null });
    assert.equal(sent.queued, true);
    const op = opOf(sent.clientRef);
    move(op.id, { state: "sending" });
    await until(() => document.querySelector(".m-chat-pending[data-pending='sending']"), "sending");
    move(op.id, { state: "retry", attempts: 1 });
    await until(() => document.querySelector(".m-chat-pending[data-pending='waiting']"), "waiting to try again");
    // the server had taken it: its echo (the host drops the operation by its clientRef), then the retry's answer, a duplicate
    const echo = message(9, "sent once", { senderUserId: "ann", senderName: "Ann", clientRef: op.id });
    drop(op.id);
    push(echo);
    tell();
    tell({ kind: "sent", op: "chat-send", clientRef: op.id, chatRoomId: "R1", threadRootEventSeq: null, json: { event: echo, eventSeq: 9, duplicate: true } });
    await until(() => rows("sent once") === 1 && !document.querySelector(".m-chat-pending"), "the message, once");
    await settle();
    assert.equal(rows("sent once"), 1, "not twice");

    // the same push from two sockets
    const twice = message(10, "pushed twice");
    push(twice);
    push(twice);
    await until(() => rows("pushed twice") > 0, "the pushed message");
    await settle();
    assert.equal(rows("pushed twice"), 1);

    // offline for a while; the changes feeds reset and the store reads the room again: an edit and a deletion made meanwhile
    server.events = server.events.map((event) => (event.eventSeq === 5 ? Object.assign({}, event, { body: "<p>changed while away</p>", bodyText: "changed while away", edited: true })
        : event.eventSeq === 4 ? Object.assign({}, event, { redacted: true, body: null, bodyText: null }) : event));
    // a reconnect alone reads only what came after the last message shown
    const before = asked("/apps/BasicComms/Chat/timeline").length;
    closeSocket();
    openSocket();
    await until(() => asked("/apps/BasicComms/Chat/timeline").length > before, "the reconnect's read");
    await settle();
    assert.ok(asked("/apps/BasicComms/Chat/timeline").slice(before).every((request) => Number(request.params.afterSeq) >= 10), "only what came after");
    assert.ok(!text().includes("changed while away"));
    // the host says the feeds changed another room: nothing read here
    const again = asked("/apps/BasicComms/Chat/timeline").length;
    tell({ kind: "rooms", rooms: ["R7"], reset: false });
    await settle();
    assert.equal(asked("/apps/BasicComms/Chat/timeline").length, again);
    // the host says the chat feed was reset: the messages shown are read again as the store has them
    tell({ kind: "rooms", rooms: [], reset: true });
    await until(() => text().includes("changed while away"), "the room as the store has it");
    assert.ok(!text().includes("an older one"), "the deleted message is gone");
    const read = asked("/apps/BasicComms/Chat/timeline").slice(again);
    assert.ok(read.some((request) => Number(request.params.afterSeq) < 5), "the messages shown read again");
    assert.equal(vm.state, "ready");
});

test("the Inbox: a thread change that conflicted says it changed on the server and reads the list again; a send that conflicted keeps a copy, opened in Compose", async () => {
    // The real message preview is deliberately sandboxed; Chromium reports the blocked injected script as a page problem.
    // Keep the mounted message-body assertions below. This allowance is confined to this case, not the suite.
    harness.allowProblems();
    // a thread's archive waiting, then the server met the thread changed
    await outbox.add("mail-state", { targets: [{ store: "email", threadId: "T1", rowKey: "email:T1" }], action: "archive" });
    const state = host.added[host.added.length - 1];
    await open("/qapps/BasicComms/Inbox", "mail");
    await until(() => document.querySelector(".m-inbox-outbox") && document.querySelector(".m-inbox-outbox").textContent.includes("1 change waits to be made"), "the change waiting");
    const lists = asked("/apps/BasicComms/Inbox/Mail/threads").length;
    move(state.id, { state: "conflict", current: { conflicts: [{ key: "email:E1", current: { archived: false }, message: "A new message came" }] } });
    const conflict = await until(() => document.querySelector(".m-inbox-outbox-change[data-outbox='conflict']"), "the conflict");
    assert.ok(conflict.textContent.includes("Archive"));
    assert.ok(conflict.textContent.includes("Changed on the server: the thread shows its state there (A new message came)"));
    await until(() => asked("/apps/BasicComms/Inbox/Mail/threads").length > lists, "the list read again");
    assert.ok(!document.querySelector(".m-inbox-outbox").textContent.includes("change waits"), "not counted as waiting");
    // a later change of the same thread waits: Retry would land after it and undo it, so it asks first
    await outbox.add("mail-state", { targets: [{ store: "email", threadId: "T1", rowKey: "email:T1" }], action: "unarchive" });
    const later = host.added[host.added.length - 1];
    await until(() => document.querySelector(".m-inbox-outbox").textContent.includes("1 change waits to be made"), "the later change waiting");
    button(document.querySelector(".m-inbox-outbox-change[data-outbox='conflict']"), "Retry").click();
    (await dialogButton("Keep the later change")).click();
    await settle();
    assert.ok(!host.retried.includes(state.id), "not retried over the later change");
    drop(later.id);
    tell();
    await until(() => !document.querySelector(".m-inbox-outbox").textContent.includes("change waits"), "the later change made");
    // and one made on the server from here since (online, not queued) asks too
    const inbox = root.$refs.view;
    await inbox.post("state", { targetsJson: JSON.stringify([{ store: "email", threadId: "T1", rowKey: "email:T1" }]), action: "flag" });
    button(document.querySelector(".m-inbox-outbox-change[data-outbox='conflict']"), "Retry").click();
    (await dialogButton("Retry anyway")).click();
    await until(() => host.retried.includes(state.id), "the change sent anyway");
    // refused: not made, with the reason; Discard leaves the thread as the server has it
    move(state.id, { state: "failed", error: "This thread is not yours", status: 403, rejected: true });
    const refused = await until(() => document.querySelector(".m-inbox-outbox-change[data-outbox='failed']"), "the refusal");
    assert.ok(refused.textContent.includes("Not made: This thread is not yours"));
    button(refused, "Discard").click();
    (await dialogButton("Discard")).click();
    await until(() => host.discarded.includes(state.id) && !document.querySelector(".m-inbox-outbox-change"), "the change discarded");

    // The existing mounted Inbox also checks its message disclosures, using two local bodies and no send action.
    const threadPath = "/apps/BasicComms/Inbox/Mail/thread", listPath = "/apps/BasicComms/Inbox/Mail/threads";
    const oldThreadAnswer = answers[threadPath], oldListAnswer = answers[listPath];
    const pat = { name: "Pat Sender", address: "pat@example.invalid" }, alex = { name: "Alex Reviewer", address: "alex@example.invalid" };
    const to = [{ name: "Ann", address: "ann@example.com", isMe: true }];
    const mail = (id, from, body, date) => ({ key: "email:" + id, emailMessageId: id, store: "email", from, toList: to,
        ccList: [], bccList: [], attachmentList: [], linkList: [], subject: "Keyboard review", text: body, date });
    const earlier = mail("KEY1", pat, "Older body available by keyboard", Date.now() - 60000);
    const laterMessage = mail("KEY2", alex, "Latest body", Date.now());
    const row = { store: "email", threadId: "TKEY", rowKey: "email:TKEY", subject: "Keyboard review", messageCount: 2,
        participantList: [pat, alex], fromName: alex.name, fromAddress: alex.address, sortDate: laterMessage.date,
        isUnread: true, unreadCount: 0, labelList: [], linkList: [], folderKindEnumId: "EfkInbox" };
    answers[threadPath] = () => ({ thread: Object.assign(clone(row), { messageList: [clone(earlier), clone(laterMessage)], canSend: false }) });
    answers[listPath] = () => ({ threadList: [clone(row)], hasMore: false, storeList: ["email"] });
    // Quasar's real button handlers turn keyup into the activation; the shell's Enter shortcut must not take it.
    const keys = event => moqui.keymap.handle(event);
    const press = (control, key, code) => {
        control.focus();
        control.dispatchEvent(new KeyboardEvent("keydown", { key, keyCode: code, bubbles: true, cancelable: true }));
        control.dispatchEvent(new KeyboardEvent("keyup", { key, keyCode: code, bubbles: true, cancelable: true }));
    };
    document.addEventListener("keydown", keys);
    try {
        await inbox.loadList(true);
        const threadRow = await until(() => document.getElementById("inbox-row-email_TKEY"), "the keyboard thread row");
        assert.equal(threadRow.getAttribute("aria-label"), "Keyboard review. Pat Sender, Alex Reviewer. 2 messages. Unread");
        assert.ok(threadRow.querySelector("button"), "the concise row name preserves its individual actions");
        threadRow.click();
        let disclosure = await until(() => document.getElementById("inbox-toggle-email_KEY1"), "the collapsed older message");
        assert.equal(disclosure.tagName, "BUTTON");
        assert.equal(disclosure.tabIndex, 0);
        assert.equal(disclosure.getAttribute("aria-expanded"), "false");
        assert.ok(disclosure.getAttribute("aria-label").startsWith("Read message from Pat Sender"));
        const reads = asked(threadPath).length;
        press(disclosure, "Enter", 13);
        disclosure = await until(() => {
            const control = document.getElementById("inbox-toggle-email_KEY1");
            return control && control.getAttribute("aria-expanded") === "true" && document.activeElement === control && control;
        }, "the expanded message and retained disclosure focus");
        assert.ok(disclosure.getAttribute("aria-label").startsWith("Collapse message from Pat Sender"));
        await until(() => {
            const frame = document.querySelector("#inbox-body-email_KEY1 iframe");
            return frame && frame.contentDocument && frame.contentDocument.body.textContent.includes("Older body available by keyboard");
        }, "the older message body");
        const recipients = document.getElementById("inbox-recipients-email_KEY1");
        assert.equal(recipients.tagName, "BUTTON");
        assert.equal(recipients.tabIndex, 0);
        assert.ok(recipients.getAttribute("aria-label").startsWith("Message details from Pat Sender"));
        press(recipients, "Enter", 13);
        await until(() => recipients.getAttribute("aria-expanded") === "true", "recipient details expanded");
        assert.ok(document.getElementById(recipients.getAttribute("aria-controls")).textContent.includes("pat@example.invalid"));
        assert.equal(document.activeElement, recipients);
        press(recipients, " ", 32);
        await until(() => recipients.getAttribute("aria-expanded") === "false", "recipient details collapsed");
        assert.equal(document.activeElement, recipients);
        press(disclosure, "Enter", 13);
        disclosure = await until(() => {
            const control = document.getElementById("inbox-toggle-email_KEY1");
            return control && control.getAttribute("aria-expanded") === "false" && document.activeElement === control && control;
        }, "collapsed message focus");
        press(disclosure, " ", 32);
        await until(() => document.getElementById("inbox-toggle-email_KEY1").getAttribute("aria-expanded") === "true", "Space expands the message");
        assert.equal(asked(threadPath).length, reads, "disclosure Enter did not reopen the whole thread");

        // A message's flag names the next action after each successful change, just like its conversation row.
        const statePath = "/apps/BasicComms/Inbox/Mail/state", changesBefore = asked(statePath, "POST").length;
        const flag = document.querySelector('#inbox-msg-email_KEY1 button[aria-label="Flag"]');
        assert.ok(flag, "the unflagged message offers Flag");
        flag.click();
        await until(() => flag.getAttribute("aria-label") === "Remove flag", "the flagged message offers Remove flag");
        assert.equal(inbox.thread.messageList.find(message => message.key === "email:KEY1").isFlagged, true);
        assert.ok(threadRow.querySelector('button[aria-label="Remove flag"]'), "the conversation reflects its flagged message");
        flag.click();
        await until(() => flag.getAttribute("aria-label") === "Flag", "the unflagged message offers Flag again");
        assert.equal(inbox.thread.messageList.find(message => message.key === "email:KEY1").isFlagged, false);
        assert.ok(threadRow.querySelector('button[aria-label="Flag"]'), "the conversation flag is restored");
        const changes = asked(statePath, "POST").slice(changesBefore);
        assert.deepEqual(changes.map(request => request.params.action), ["flag", "unflag"], "one request per toggle");
        assert.ok(changes.every(request => JSON.stringify(JSON.parse(request.params.targetsJson)) ===
            JSON.stringify([{ store: "email", emailMessageId: "KEY1" }])), "only the selected message is changed");
    } finally {
        document.removeEventListener("keydown", keys);
        answers[listPath] = oldListAnswer;
        if (oldThreadAnswer) answers[threadPath] = oldThreadAnswer; else delete answers[threadPath];
        inbox.closeThread();
        await until(() => !inbox.thread, "the keyboard thread closed");
        await inbox.loadList(true);
    }

    // a mail whose draft was changed elsewhere: the server's draft stays, the user's version is kept as a copy
    const copy = { at: 1, email: true, clientRef: "mCOPY", queued: [{ file: "f1", name: "plan.pdf", size: 3, contentType: "application/pdf" }], conflictOf: { clientRef: "mc1", emailMessageId: "E5" },
        draft: { to: [{ value: "cy@example.com", label: "Cy" }], cc: [], bcc: [], subject: "Quarterly report (mine)", body: "<p>my words</p>", quoteHtml: null,
            fromValue: "acct1", parentEmailMessageId: null, isForward: null, attachments: [], uploads: [] } };
    kept.set("ironstone.compose.email.mCOPY", JSON.stringify(copy));
    ops.push({ id: "mc1", kind: "mail-send", state: "conflict", attempts: 1, error: null, status: null, rejected: false, createdMs: Date.now(),
        payload: { clientRef: "mc1", fields: { subject: "Quarterly report", toJson: JSON.stringify([{ value: "cy@example.com", label: "Cy" }]), fromValue: "acct1" } },
        current: { compose: { subject: "Quarterly report (theirs)" }, copyKey: "ironstone.compose.email.mCOPY", copyRef: "mCOPY" } });
    tell();
    const send = await until(() => document.querySelector(".m-inbox-outbox-item[data-outbox='conflict']"), "the send's conflict");
    assert.ok(send.textContent.includes("Changed elsewhere: your version is kept as a copy"));
    assert.ok(send.querySelector(".m-inbox-open-copy"), "Open the copy");
    assert.ok(button(send, "Retry") && button(send, "Discard"));
    // Discard: not sent, and the copy stays, listed to open
    button(send, "Discard").click();
    (await dialogButton("Discard")).click();
    const listed = await until(() => document.querySelector(".m-inbox-outbox-copy"), "the copy listed");
    assert.ok(listed.textContent.includes("Quarterly report (mine)"));
    assert.ok(host.discarded.includes("mc1"));
    // opened: a new draft of the user's version, saved on the server by its own clientRef; then no longer kept here
    button(listed, "Open").click();
    await until(() => document.querySelector(".m-compose-copy"), "the copy in Compose");
    await until(() => Array.from(document.querySelectorAll(".m-compose input")).some((input) => input.value === "Quarterly report (mine)"), "its subject");
    const saved = await until(() => asked("/apps/BasicComms/Inbox/Compose/saveDraft", "POST").find((request) => request.params.clientRef === "mCOPY"), "the copy saved");
    assert.equal(saved.params.subject, "Quarterly report (mine)");
    assert.ok(saved.params.body.includes("my words"));
    await until(() => !kept.has("ironstone.compose.email.mCOPY"), "the copy on the server now");
    // with its file still to send it stays kept here, by the server's draft, but as a copy no longer
    const now = await until(() => kept.get("ironstone.compose.email.E9"), "kept by its draft");
    assert.equal(JSON.parse(now).conflictOf, undefined);
    await until(() => !document.querySelector(".m-inbox-outbox-copy"), "no longer listed as a copy");

    // a copyRef that is not a kept copy (a server draft's id, edited offline) opens nothing and touches nothing
    const edited = JSON.stringify({ at: 2, email: true, clientRef: "mE123", queued: [], draft: { emailMessageId: "E123", to: [], cc: [], bcc: [],
        subject: "Edited offline", body: "<p>mine</p>", fromValue: "acct1", attachments: [], uploads: [] } });
    kept.set("ironstone.compose.email.E123", edited);
    for (const ref of ["E123", "../x"]) {
        await open("/qapps/BasicComms/Inbox/Compose?copyRef=" + encodeURIComponent(ref), "compose");
        await until(() => text().includes("This copy is no longer kept on this computer."), "not a copy: " + ref);
        await settle();
        assert.ok(!document.querySelector(".m-compose-copy"));
    }
    await settle();
    assert.equal(kept.get("ironstone.compose.email.E123"), edited, "the offline edits kept as they were");
});

test("Compose: offline, an internal message waits in the outbox where the server takes its send once, and a server draft's send carries its stamp", async () => {
    server.online = false;
    host.kinds = { clientApi: 4, kinds: MD5_KINDS, internalSend: true };
    server.compose = { identityList: [{ value: "internal", label: "Internal", isInternal: true }], fromValue: "internal", to: [{ value: "party:bob", label: "Bob", kind: "party" }],
        subject: "Lunch", kind: "new" };
    let compose = await open("/qapps/BasicComms/Inbox/Compose?subject=Lunch", "compose");
    await until(() => document.querySelector(".m-compose-offline") && text().includes("this message waits in the outbox"), "the internal message's banner");
    compose = root.$refs.view;
    const before = host.added.length;
    compose.send();
    const internal = await until(() => host.added.length > before && host.added[host.added.length - 1], "the internal send queued");
    assert.equal(internal.kind, "internal-send");
    assert.equal(internal.payload.fields.fromValue, "internal");
    assert.equal(internal.payload.fields.subject, "Lunch");

    // a server that does not take it once: it needs a connection, kept as a draft
    host.kinds = { clientApi: 4, kinds: MD5_KINDS.filter((kind) => kind !== "internal-send"), internalSend: false };
    compose = await open("/qapps/BasicComms/Inbox/Compose?subject=Lunch2", "compose");
    await until(() => text().includes("an internal message needs a connection to send"), "needs a connection");
    compose = root.$refs.view;
    const count = host.added.length;
    compose.send();
    await until(() => text().includes("An internal message needs a connection to send; it is kept as a draft"), "kept as a draft");
    assert.equal(host.added.length, count);

    // the server's draft edited here: its send waits with the stamp it was edited from, so one saved elsewhere is a conflict
    server.compose = { identityList: [{ value: "acct1", label: "ann@example.com" }], fromValue: "acct1", to: [{ value: "cy@example.com", label: "Cy" }], subject: "Plan",
        kind: "draft", emailMessageId: "E5", clientRef: "mE5", draftStamp: 1234 };
    compose = await open("/qapps/BasicComms/Inbox/Compose?emailMessageId=E5", "compose");
    compose = await until(() => root.$refs.view && root.$refs.view.state === "ready" && root.$refs.view.draft.emailMessageId === "E5" && root.$refs.view, "the draft");
    compose.send();
    const mail = await until(() => host.added.find((op) => op.kind === "mail-send" && op.payload.fields.emailMessageId === "E5"), "the send queued");
    assert.equal(mail.payload.fields.baseStamp, "1234");
    assert.equal(mail.id, "mE5", "by the draft's clientRef");
    server.online = true;
    server.compose = null;
});

test("Compose keeps unsaved changes open without an identity or after storage failures, and recovery preserves the message", async () => {
    server.online = true;
    server.compose = { identityList: [], fromValue: null, to: [],
        subject: "Retain this draft", kind: "new" };
    await open("/qapps/BasicComms/Inbox/Compose?subject=RetainThisDraft", "compose");
    const compose = await until(() => root.$refs.view && root.$refs.view.state === "ready"
        && root.$refs.view.draft.subject === "Retain this draft" && root.$refs.view, "the composer");
    const setUrl = root.setUrl;
    const desktop = window.moquiDesktop;
    const put = drafts.put;
    const local = moqui.drafts.local;
    const request = moqui.request;
    const closed = [];
    root.setUrl = (url) => closed.push(url);
    try {
        compose.draft.body = "<p>Keep these words until they are saved.</p>";
        delete window.moquiDesktop;
        await window.Vue.nextTick();
        const identityBanner = document.querySelector(".m-compose-no-from");
        assert.ok(identityBanner && identityBanner.textContent.includes("No sending identity is available"));
        const accounts = identityBanner.querySelector("a");
        assert.equal(accounts.getAttribute("target"), "_blank", "account setup leaves the unsaved message open");
        assert.ok(accounts.getAttribute("href").endsWith("/BasicComms/Settings/MailAccounts"));
        const beforeSave = asked("/apps/BasicComms/Inbox/Compose/saveDraft", "POST").length;
        await compose.close();
        await window.Vue.nextTick();
        assert.equal(closed.length, 0);
        assert.equal(asked("/apps/BasicComms/Inbox/Compose/saveDraft", "POST").length, beforeSave, "missing From is explained before calling the service");
        assert.ok(compose.saveError.includes("Add a mail account"));
        assert.equal(compose.draft.subject, "Retain this draft");
        assert.equal(compose.draft.body, "<p>Keep these words until they are saved.</p>");
        compose.draft.fromValue = "identity:removed";
        server.compose = { identityList: [{ value: "acct1", label: "ann@example.com", signatureHtml: "<p>Ann</p>" }], fromValue: "acct1", to: [], subject: "", body: "" };
        button(identityBanner, "Refresh sending identities").click();
        await until(() => !compose.refreshingIdentities && !compose.noFrom, "the sending identity is available");
        assert.equal(compose.draft.fromValue, "acct1");
        assert.equal(compose.draft.subject, "Retain this draft");
        assert.equal(compose.draft.body, "<p>Keep these words until they are saved.</p>");
        assert.deepEqual(asked("/apps/BasicComms/Inbox/Compose/composeData").slice(-1)[0].params, {}, "identity refresh cannot repeat draft/reply initialization");
        window.moquiDesktop = desktop;
        compose.savedAt = 1;
        await window.Vue.nextTick();
        server.saveError = "Draft storage is temporarily unavailable";
        await compose.close();
        await window.Vue.nextTick();
        assert.equal(closed.length, 0, "closing cannot discard unsaved changes");
        assert.equal(compose.dirty, true);
        assert.equal(compose.draft.body, "<p>Keep these words until they are saved.</p>");
        assert.equal(compose.savedText, "Not saved", "the previous Saved time is no longer presented as current");
        let banner = document.querySelector(".m-compose-save-error");
        assert.ok(banner && banner.textContent.includes(server.saveError), "the save failure is visible beside the draft");
        assert.equal(banner.getAttribute("role"), "alert");
        server.saveError = null;
        const beforeRetry = asked("/apps/BasicComms/Inbox/Compose/saveDraft", "POST").length;
        button(banner, "Retry save").click();
        await until(() => !compose.saving && !compose.saveError && !compose.dirty, "the retry saved");
        assert.equal(asked("/apps/BasicComms/Inbox/Compose/saveDraft", "POST").length, beforeRetry + 1,
            "the first server-assigned draft id does not trigger another save");
        assert.equal(compose.saveTimer, null, "the assigned id does not leave a redundant autosave timer");
        assert.ok(!document.querySelector(".m-compose-save-error"));
        assert.equal(asked("/apps/BasicComms/Inbox/Compose/saveDraft", "POST").slice(-1)[0].params.body,
            "<p>Keep these words until they are saved.</p>");
        await compose.close();
        assert.equal(closed.length, 1, "closing now keeps the saved draft");
        assert.ok(closed[0].endsWith("/Inbox"));

        // A slow save acknowledges its own content, not edits made while it was in flight. Close waits for those too.
        const pending = [];
        moqui.request = (url, options) => {
            const answer = request(url, options);
            if (!String(url).endsWith("/Compose/saveDraft")) return answer;
            return answer.then((result) => new Promise((resolve, reject) => {
                pending.push({ params: clone(options.params), resolve: () => resolve(result), reject });
            }));
        };
        compose.draft.body = "<p>The first server snapshot.</p>";
        await window.Vue.nextTick();
        const firstSave = compose.save();
        await until(() => pending.length === 1, "the first server save waiting");
        compose.draft.body = "<p>New words while the server saves.</p>";
        await window.Vue.nextTick();
        pending[0].resolve();
        await until(() => pending.length === 2 || !compose.saving, "the old server answer handled");
        assert.equal(compose.dirty, true, "an old server answer cannot mark newer text saved");
        assert.equal(pending.length, 2, "newer text is saved without waiting for the debounce timer");
        assert.equal(pending[1].params.body, "<p>New words while the server saves.</p>");
        const closingServer = compose.close();
        assert.equal(closed.length, 1, "Close waits for the current content");
        pending[1].reject({ ok: false, status: 503, headers: plainHeaders, json: { errors: "The follow-up save failed" } });
        await Promise.all([firstSave, closingServer]);
        assert.equal(closed.length, 1, "a failed follow-up retains the composer");
        assert.equal(compose.dirty, true);
        assert.equal(compose.draft.body, "<p>New words while the server saves.</p>");
        assert.ok(compose.saveError.includes("follow-up save failed"));
        moqui.request = request;
        await compose.save();
        assert.equal(compose.dirty, false);
        assert.equal(compose.saveTimer, null, "acknowledged content leaves no redundant autosave");

        // An offline email cannot close just because the server failed: the host must persist the fallback.
        server.online = false;
        drafts.put = () => Promise.reject(new Error("Device storage is full"));
        compose.draft.body = "<p>These offline changes must stay open.</p>";
        await window.Vue.nextTick();
        await compose.close();
        await window.Vue.nextTick();
        assert.equal(closed.length, 1);
        assert.equal(compose.dirty, true);
        assert.ok(compose.saveError.includes("This device could not keep the draft"));
        drafts.put = put;
        button(document.querySelector(".m-compose-save-error"), "Retry save").click();
        await until(() => !compose.saving && !compose.saveError && !compose.dirty, "the offline draft persisted");
        assert.equal(JSON.parse(kept.get(compose.emailLocalKey)).draft.body, "<p>These offline changes must stay open.</p>");
        await compose.close();
        assert.equal(closed.length, 2);

        // The host's asynchronous local write has the same race; the local fallback survives an older server answer.
        const localPending = [];
        drafts.put = (key, value) => new Promise((resolve, reject) => {
            localPending.push({ key, value, resolve: () => { kept.set(key, String(value)); resolve(); }, reject });
        });
        compose.draft.body = "<p>The first device snapshot.</p>";
        await window.Vue.nextTick();
        const localSave = compose.save();
        await until(() => localPending.length === 1, "the first device write waiting");
        compose.draft.body = "<p>New words while the device saves.</p>";
        await window.Vue.nextTick();
        localPending[0].resolve();
        await until(() => localPending.length === 2 || !compose.saving, "the old device answer handled");
        assert.equal(compose.dirty, true, "an old device answer cannot mark newer text saved");
        assert.equal(localPending.length, 2);
        assert.equal(JSON.parse(localPending[1].value).draft.body, "<p>New words while the device saves.</p>");
        const closingLocal = compose.close();
        assert.equal(closed.length, 2);
        localPending[1].resolve();
        await Promise.all([localSave, closingLocal]);
        assert.equal(closed.length, 3, "Close follows persistence of the newest device content");
        assert.equal(localPending.length, 2, "Close does not create an unchanged third write");
        drafts.put = put;

        server.online = true;
        moqui.request = (url, options) => {
            const answer = request(url, options);
            if (!String(url).endsWith("/Compose/saveDraft")) return answer;
            return answer.then((result) => new Promise((resolve, reject) => {
                pending.push({ params: clone(options.params), resolve: () => resolve(result), reject });
            }));
        };
        const fallbackKey = compose.localEmailKey;
        const reconnectSave = compose.save();
        await until(() => pending.length === 3, "the reconnect save waiting");
        compose.draft.subject = "New subject during reconnection";
        await window.Vue.nextTick();
        pending[2].resolve();
        await until(() => pending.length === 4, "the newer reconnect content waiting");
        assert.ok(kept.has(fallbackKey), "an older server snapshot does not delete the local fallback");
        pending[3].reject({ ok: false, status: 503, headers: plainHeaders, json: { errors: "Reconnection save failed" } });
        await reconnectSave;
        assert.ok(kept.has(fallbackKey), "a failed newer save still has its previous local recovery copy");
        assert.equal(compose.draft.subject, "New subject during reconnection");
        assert.equal(compose.dirty, true);
        moqui.request = request;
        await compose.save();
        await until(() => !kept.has(fallbackKey), "the current server content replaces its local fallback");

        // Clearing a new offline email during its first write must replace that snapshot even without a server id.
        server.online = false;
        compose.draft.emailMessageId = null;
        compose.draft.subject = "";
        compose.draft.body = "<p>Clear this before its first offline save finishes.</p>";
        localPending.length = 0;
        drafts.put = (key, value) => {
            if (key === compose.emailRefKey) return put(key, value);
            return new Promise((resolve) => {
                localPending.push({ key, value, resolve: () => { kept.set(key, String(value)); resolve(); } });
            });
        };
        await window.Vue.nextTick();
        const clearingSave = compose.save();
        await until(() => localPending.length === 1, "the new offline draft's first write waiting");
        compose.draft.body = "";
        await window.Vue.nextTick();
        assert.equal(compose.isEmpty, true);
        const closingEmpty = compose.close();
        await window.Vue.nextTick();
        assert.equal(closed.length, 3, "an empty draft still waits for its in-flight save before closing");
        localPending[0].resolve();
        await until(() => localPending.length === 2 || !compose.saving, "the cleared offline draft acknowledged");
        assert.equal(localPending.length, 2, "an empty draft replaces already-persisted offline content");
        assert.equal(JSON.parse(localPending[1].value).draft.body, "");
        assert.equal(closed.length, 3, "the empty shortcut cannot close while clearing is still being saved");
        localPending[1].resolve();
        await Promise.all([clearingSave, closingEmpty]);
        assert.equal(closed.length, 4);
        assert.equal(JSON.parse(kept.get(compose.emailLocalKey)).draft.body, "", "reopening cannot restore the cleared text");
        drafts.put = put;

        // Internal messages on the web use localStorage; a failed write must have the same recovery.
        server.online = true;
        delete window.moquiDesktop;
        moqui.drafts.local = () => ({setItem() { throw new Error("Storage quota exceeded"); }});
        compose.draft.fromValue = "internal";
        compose.draft.body = "<p>These browser changes must stay open.</p>";
        await window.Vue.nextTick();
        await compose.close();
        await window.Vue.nextTick();
        assert.equal(closed.length, 4);
        assert.equal(compose.dirty, true);
        assert.ok(document.querySelector(".m-compose-save-error"));
        moqui.drafts.local = local;
        button(document.querySelector(".m-compose-save-error"), "Retry save").click();
        await until(() => !compose.saving && !compose.saveError && !compose.dirty, "the browser draft persisted");
        assert.equal(JSON.parse(moqui.drafts.get(compose.draftStorageKey)).draft.body, "<p>These browser changes must stay open.</p>");
        await compose.close();
        assert.equal(closed.length, 5);
        await moqui.drafts.remove(compose.draftStorageKey);
    } finally {
        root.setUrl = setUrl;
        window.moquiDesktop = desktop;
        drafts.put = put;
        moqui.drafts.local = local;
        moqui.request = request;
        server.online = true;
        server.saveError = null;
        server.compose = null;
    }
});

test("a revoked key holds what waits, each saying to sign in again; a held item stops no other", async () => {
    server.online = true;
    ops.push({ id: "h1", kind: "chat-send", state: "held", attempts: 1, createdMs: Date.now(), payload: { chatRoomId: "R1", text: "held for the key", threadRootEventSeq: null } },
        { id: "h2", kind: "chat-edit", state: "held", attempts: 1, createdMs: Date.now(), payload: { chatRoomId: "R1", eventSeq: 5, body: "<p>x</p>" } },
        { id: "w1", kind: "chat-send", state: "queued", attempts: 0, createdMs: Date.now() + 1, payload: { chatRoomId: "R1", text: "after it", threadRootEventSeq: null } },
        { id: "h3", kind: "mail-send", state: "held", attempts: 1, createdMs: Date.now(), payload: { clientRef: "h3", fields: { subject: "Held mail", fromValue: "acct1" } } });
    tell();
    await openRoom();
    const held = await until(() => document.querySelector(".m-chat-pending[data-client-ref='h1'][data-pending='held']"), "the held message");
    assert.ok(held.textContent.includes("Waiting: sign in again to send it"));
    assert.ok((await until(() => change("chat-edit", "held"), "the held edit")).textContent.includes("Edit: waiting, sign in again to send it"));
    const after = await until(() => document.querySelector(".m-chat-pending[data-client-ref='w1']"), "the next message");
    assert.equal(after.getAttribute("data-pending"), "waiting");
    await open("/qapps/BasicComms/Inbox", "mail");
    const mail = await until(() => Array.from(document.querySelectorAll(".m-inbox-outbox-item")).find((item) => item.textContent.includes("Held mail")), "the held mail");
    assert.ok(mail.textContent.includes("Waiting: sign in again to send it"));
});

test("poll votes: one waiting counts for the next (several choices add up); a vote taken back waits for one still queued", async () => {
    const vm = await openRoom();
    const poll = (multiple) => ({ pollId: "P" + (multiple ? 1 : 2), question: "Lunch?", multiple, anonymous: false, closed: false, createdByUserId: "bob", voterCount: 0,
        options: [{ id: 1, text: "Soup", mine: false, voteCount: 0 }, { id: 2, text: "Salad", mine: false, voteCount: 0 }] });
    const votes = () => host.added.filter((op) => op.kind === "poll-vote");
    const many = { eventSeq: 11, poll: poll(true) };
    vm.vote(many, many.poll.options[0]);
    await until(() => votes().length === 1, "the first vote queued");
    assert.equal(votes()[0].payload.optionIds, "1");
    assert.equal(vm.optionIcon(many, many.poll.options[0]), "sym_o_check_box", "shown as chosen while it waits");
    vm.vote(many, many.poll.options[1]);
    await until(() => votes().length === 2, "the second vote queued");
    assert.equal(votes()[1].payload.optionIds, "1,2", "Soup is kept");
    // one choice: chosen, then taken back while the vote waits: asked of nobody, and said
    const one = { eventSeq: 12, poll: poll(false) };
    vm.vote(one, one.poll.options[0]);
    await until(() => votes().length === 3, "the single vote queued");
    const posts = asked("/apps/BasicComms/Chat/votePoll", "POST").length;
    const said = [];
    const notify = moqui.notifyMessages;
    moqui.notifyMessages = (...args) => { said.push(JSON.stringify(args)); return notify(...args); };
    // (the vote just queued counts, though the host's list may not have it yet)
    try { vm.vote(one, one.poll.options[0]); } finally { moqui.notifyMessages = notify; }
    assert.equal(said.length, 1);
    assert.ok(said[0].includes("Your vote is waiting to be counted"), said[0]);
    assert.equal(votes().length, 3, "not queued again");
    assert.equal(asked("/apps/BasicComms/Chat/votePoll", "POST").length, posts, "not asked ahead of the queued vote");
    // answered: the poll as the server counted it
    for (const op of votes()) drop(op.id);
    tell();
    await settle();
    tell({ kind: "sent", op: "poll-vote", clientRef: votes()[1].id, chatRoomId: "R1", eventSeq: 11, json: { poll: Object.assign(poll(true), { voterCount: 1 }) } });
    await settle();
    assert.equal(vm.myOptions(many).length, 0, "no vote waits; the poll's own answer counts");
});

test("an encrypted room: a reaction taken back while it waits is dropped, and a reconnect decrypts each page once", async () => {
    const chat = moqui.chat;
    const kept = { openEvents: chat.openEvents, openEvent: chat.openEvent, sealReaction: chat.sealReaction, myReaction: chat.myReaction };
    let opened = 0;
    chat.openEvents = (roomId, list) => { opened += 1; return Promise.resolve(list); };
    chat.openEvent = () => Promise.resolve();
    chat.sealReaction = () => Promise.resolve(JSON.stringify({ ciphertext: "x" }));
    chat.myReaction = () => null;
    try {
        const screen = await open("/qapps/BasicComms/Chat?chatRoomId=R3", "chat");
        const vm = await until(() => screen.$refs.room && screen.$refs.room.roomId === "R3" && screen.$refs.room.state === "ready" && screen.$refs.room, "the encrypted room");
        assert.ok(vm.sealed);
        vm.react({ eventSeq: 3, event: eventOf(vm, 3) }, "👍");
        const sealed = await until(() => host.added.find((op) => op.kind === "chat-react" && op.payload.chatRoomId === "R3"), "the encrypted reaction queued");
        assert.ok(sealed.payload.encryptedJson && !sealed.payload.key, "its key only as ciphertext");
        await until(() => change("chat-react", "waiting"), "it waits");
        // taken back before the server has it (no reaction event of its own to take back): dropped from the outbox
        vm.react({ eventSeq: 3, event: eventOf(vm, 3) }, "👍");
        await until(() => host.discarded.includes(sealed.id), "the waiting reaction dropped");
        assert.ok(!host.added.some((op) => op.kind === "chat-react" && op.payload.chatRoomId === "R3" && op.payload.reactionEventSeq != null));
        // a new message while away: the reconnect decrypts what each read brings once
        sealedEvents.push(message(4, "sealed three", { encrypted: true }));
        const from = requests.length;
        const before = opened;
        closeSocket();
        openSocket();
        await until(() => text().includes("sealed three"), "the new message");
        await settle();
        const reads = requests.slice(from).filter((request) => request.method === "GET" && request.params.chatRoomId === "R3" && request.events > 0).length;
        assert.ok(reads > 0);
        assert.equal(opened - before, reads, "once each");
    } finally {
        Object.assign(chat, kept);
    }
});

test("with no host, as on the web, a message's changes are asked of the server as before", async () => {
    const vm = await openRoom();
    const desktop = window.moquiDesktop;
    delete window.moquiDesktop;
    try {
        const count = host.added.length;
        const edits = asked("/apps/BasicComms/Chat/edit", "POST").length;
        await vm.saveEdit({ eventSeq: 5, event: eventOf(vm, 5) }, "<p>from the web</p>");
        assert.equal(asked("/apps/BasicComms/Chat/edit", "POST").length, edits + 1);
        vm.react({ eventSeq: 5, event: eventOf(vm, 5) }, "👀");
        await until(() => asked("/apps/BasicComms/Chat/react", "POST").some((request) => request.params.key === "👀"), "the reaction asked");
        assert.equal(moqui.chat.queueChange("chat-edit", { chatRoomId: "R1", eventSeq: 5, body: "x" }), null);
        assert.equal(host.added.length, count, "nothing queued");
        assert.ok(!vm.queues("chat-react"));
    } finally {
        window.moquiDesktop = desktop;
    }
});
})();
