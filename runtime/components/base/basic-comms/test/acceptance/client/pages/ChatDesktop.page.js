/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The desktop gaps of Chat in the bundled shell (ChatDesktop.spec.js; plans/Comms-Messaging.md §3): Shell/Root on a stand-in
// host and a stand-in server (as ChatDetails.page.js) whose shell data has the comms app with its palette sources and a
// screen whose title also starts with the letters typed, so the palette opens a room only when the rooms lead. Its
// shellSearch answers Chat rooms and people as search#PaletteRooms does; its activity answers a mention and a reaction
// from another user, and thread replies; its mailLines a mail line on a person's direct chat. The keys are the browser's
// own (the spec's keyboard), so the shell's keymap is what hears them. window.desk has a step for each feature of the
// spec, each a promise of what it saw.
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
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const clone = (value) => JSON.parse(JSON.stringify(value));
const textOf = (el) => (el ? el.textContent.replace(/\s+/g, " ").trim() : "");

// ---- the server ----
const NOW = Date.now();
const room = (id, type, name, extra) => Object.assign({ chatRoomId: id, roomTypeEnumId: type, roomName: type === "CrtDirect" ? null : name, displayName: name,
    isMember: true, unreadCount: 0, visibilityEnumId: "CrvMembers", lastEventSeq: 3, lastEventDate: NOW - 60000,
    lastMessage: { eventSeq: 3, senderUserId: "bob", senderName: "Bob Baker", sentDate: NOW - 60000, snippet: "Hello from " + name } }, extra || {});
const rooms = [room("R1", "CrtDirect", "Bob Baker", { otherUserId: "bob" }), room("R2", "CrtPrivate", "General"),
    room("X1", "CrtExternal", "+15550001111"), room("B1", "CrtPrivate", "Matrix lounge", { bridged: true, bridgeNetworkNames: ["Matrix"] })];
const event = (seq, body) => ({ eventSeq: seq, type: "message", senderUserId: "bob", senderName: "Bob Baker", sentDate: NOW - 600000 + seq * 1000,
    body: "<p>" + body + "</p>", bodyText: body, reactions: [], attachments: [] });
const item = (kind, roomId, roomName, seq, extra) => Object.assign({ kind, chatRoomId: roomId, roomName, roomTypeEnumId: "CrtPrivate", eventSeq: seq, messageEventSeq: seq,
    threadRootEventSeq: null, actorUserId: "bob", actorName: "Bob Baker", snippet: null, reactionKey: null, date: NOW - 1000 * seq, encrypted: false }, extra || {});
const activity = [item("mention", "R2", "General", 7, { snippet: "@Ann can you look at the plan?" }),
    item("reaction", "R1", "Bob Baker", 9, { messageEventSeq: 2, snippet: "Lunch at noon?", reactionKey: "👍" })];
const threadReplies = [item("threadReply", "R2", "General", 12, { threadRootEventSeq: 5, snippet: "Second reply" }),
    item("threadReply", "R2", "General", 11, { threadRootEventSeq: 5, actorUserId: "cy", actorName: "Cy Cole", snippet: "First reply" }),
    item("threadReply", "B1", "Matrix lounge", 10, { threadRootEventSeq: 4, snippet: "Another thread" })];
const people = [{ userId: "dee", name: "Dee Dunn" }];
const posted = [];
const asked = [];
const answers = {
    "/qapps/clientConfig": () => ({ userId: "ann", username: "ann", userFullName: "Ann", locale: "en", server: { name: "Test Server", clientApi: 4 },
        calls: { appRoot: "/qapps/BasicComms" }, notificationEndpoint: "/notws" }),
    // the comms app: a screen that starts as a room does (Getting started, "ge"), the Messages and Chat rooms sources
    "/shellData": () => ({ apps: [{ name: "BasicComms", title: "Comms", path: "/qapps/BasicComms", image: "sym_o_forum", imageType: "icon",
        menu: [{ title: "Getting started", path: "/qapps/BasicComms/GettingStarted", depth: 1 }, { title: "Inbox", path: "/qapps/BasicComms/Inbox", depth: 1 }],
        providers: { paletteSources: [{ group: "Messages", label: "Messages", icon: "sym_o_mail", prefix: null },
            { group: "Rooms", label: "Chat rooms", icon: "sym_o_chat", prefix: "#" }] } }], counts: {} }),
    // as search#PaletteRooms: the rooms by name, then the people with no direct chat (dm); Messages beside them
    "/shellSearch": (params) => {
        const text = String(params.text || "").toLowerCase();
        const rows = rooms.filter((one) => one.displayName.toLowerCase().indexOf(text) >= 0)
            .map((one) => ({ title: one.displayName, subtitle: "Room", icon: "sym_o_groups", url: "/qapps/BasicComms/Chat?chatRoomId=" + one.chatRoomId }))
            .concat(people.filter((one) => one.name.toLowerCase().indexOf(text) >= 0)
                .map((one) => ({ title: one.name, subtitle: "Person · Start a direct message", icon: "sym_o_person_add", url: "/qapps/BasicComms/Chat?dm=" + one.userId })));
        const groups = [{ group: "Messages", label: "Messages", icon: "sym_o_mail", rows: [{ title: "Re: " + text, url: "/qapps/BasicComms/Inbox?thread=email%3AM9" }] }];
        if (rows.length) groups.push({ group: "Rooms", label: "Chat rooms", icon: "sym_o_chat", prefix: "#", rows, moreUrl: "/qapps/BasicComms/Chat" });
        return { groups };
    },
    "/apps/l10nCatalog": () => ({}),
    "/apps/BasicComms/Chat/chatData": () => ({ me: { userId: "ann" }, prefs: {}, roomList: clone(rooms) }),
    "/apps/BasicComms/Chat/roomView": (params) => ({ room: clone(rooms.find((one) => one.chatRoomId === params.chatRoomId) || rooms[0]),
        member: { lastReadEventSeq: 3 }, access: { isMember: true, canPost: true }, memberList: [{ userId: "ann", name: "Ann" }, { userId: "bob", name: "Bob Baker" }],
        timeline: { eventList: [event(1, "One"), event(2, "Lunch at noon?"), event(3, "Three")], hasMoreBefore: false, hasMoreAfter: false } }),
    "/apps/BasicComms/Chat/timeline": () => ({ eventList: [event(1, "One"), event(2, "Lunch at noon?"), event(3, "Three")], hasMoreBefore: false, hasMoreAfter: false }),
    "/apps/BasicComms/Chat/activity": (params) => ({ items: clone(String(params.kinds || "") === "threadReply" ? threadReplies : activity), nextCursor: null }),
    "/apps/BasicComms/Chat/mailLines": () => ({ mailList: [{ userId: "bob", chatRoomId: "R1", subject: "Q3 plan", date: NOW - 3600000, threadKey: "email:M1",
        url: "/qapps/BasicComms/Inbox?view=all&thread=email%3AM1" }] }),
    "/apps/BasicComms/Chat/createDirect": (params) => ({ chatRoomId: "D_" + params.userIds, created: true })
};
moqui.request = (url, options) => {
    const path = String(url).split("?")[0];
    const method = (options && options.method) || "GET";
    const params = options && moqui.isPlainObject(options.params) ? options.params : {};
    if (method === "POST") posted.push({ path, params: clone(params) });
    asked.push({ path, params: clone(params) });
    moqui.noteAnswer(200, { get: () => null });
    const answer = answers[path];
    return Promise.resolve({ ok: true, status: 200, headers: { get: () => null }, json: answer ? answer(params) : {}, text: "" });
};

// ---- the host ----
const shellState = window.Vue.reactive({ accounts: [{ id: "a1", username: "ann", server: "https://s.example.com", deviceMode: "personal" }], current: "a1",
    signedIn: true, keyStore: "os", clientApi: 4, deviceMode: "personal", loadToken: null });
const kept = new Map();
const remote = [];
window.moquiDesktop = {
    state: () => Promise.resolve(shellState),
    takeOpen: () => Promise.resolve(null),
    clearScopes: () => Promise.resolve(),
    socket: () => ({ readyState: 0, send() {}, close() {} }),
    drafts: {
        get: (key) => Promise.resolve(kept.has(key) ? { key, value: kept.get(key), files: [], updatedMs: 1 } : null),
        list: () => Promise.resolve([]),
        put: (key, value) => { kept.set(key, String(value)); return Promise.resolve(); },
        remove: (key) => { kept.delete(key); return Promise.resolve(true); },
        saveFile: () => Promise.resolve(null)
    },
    signOut: () => Promise.resolve(),
    // a screen this shell does not show: an app window
    openRemote: (url) => { remote.push(String(url)); return Promise.resolve(); },
    setCounts() {}, setPresence() {}, storageScope: "a1"
};

// ---- the steps ----
let root = null;
const phone = () => window.innerWidth < 640;
const chip = (name) => document.querySelector((phone() ? ".m-chat-home " : ".m-chat-root ") + "[data-filter='" + name + "']");
const roomRow = (id) => document.querySelector((phone() ? ".m-chat-home " : ".m-chat-root ") + "[data-room='" + id + "']");
const where = () => ({ view: root.view, parameters: clone(root.currentParameters || {}) });
// back to the Chat list with no room
const toChat = async () => {
    root.setUrl("/qapps/BasicComms/Chat");
    await until(() => root.view === "chat" && !root.currentParameters.chatRoomId && chip("all"), "the Chat list");
    chip("all").click();
    await until(() => document.querySelector(".m-chat-root .q-item[data-room='R2']:not(.m-chat-feed-row)"), "the rooms of All");
};

window.desk = {
    async start() {
        Quasar.Platform.is.mobile = phone();
        const shell = document.createElement("div");
        shell.id = "shell";
        document.body.appendChild(shell);
        try { localStorage.clear(); } catch (err) { /* no storage */ }
        await moqui.loadScript("/files/crates/moqui-desktop/shell/js/shell.js");
        root = await until(() => moqui.webrootVue && moqui.webrootVue.commsItems && moqui.webrootVue, "the shell's root");
        if (!phone()) await toChat();
        else await until(() => roomRow("R1"), "the home's rooms");
        return true;
    },
    // the palette's input, open and focused (the spec's keyboard pressed Mod+K)
    paletteReady() { return until(() => document.getElementById("m-palette-input") === document.activeElement && root.paletteOpen, "the palette, focused").then(() => true); },
    // after the spec typed two letters and Enter: where the shell went, what it asked and whether a window opened
    async afterEnter(wanted) {
        await until(() => root.view === "chat" && root.currentParameters.chatRoomId === wanted, "the room " + wanted, 4000).catch(() => null);
        const searched = asked.filter((one) => one.path === "/shellSearch").map((one) => one.params.text);
        const out = Object.assign(where(), { searched, remote: remote.slice(), paletteOpen: root.paletteOpen,
            search: !!document.getElementById("m-search"), lead: root.paletteLead.slice() });
        await toChat();
        return out;
    },
    // a person with no direct chat (dm): Chat makes it and shows it in place of the link
    async afterPerson() {
        await until(() => root.currentParameters.chatRoomId === "D_dee", "the new direct chat", 4000).catch(() => null);
        const out = Object.assign(where(), { made: posted.filter((one) => one.path === "/apps/BasicComms/Chat/createDirect").map((one) => one.params.userIds) });
        await toChat();
        return out;
    },
    // Activity: a mention and a reaction from Bob, newest first; the mention opens its message
    async activity() {
        chip("activity").click();
        await until(() => document.querySelectorAll(".m-chat-feed .m-chat-feed-row").length === 2, "the activity rows");
        const rows = Array.from(document.querySelectorAll(".m-chat-feed .m-chat-feed-row"));
        const out = { kinds: rows.map((row) => row.getAttribute("data-kind")), titles: rows.map((row) => textOf(row.querySelector(".q-item__label .ellipsis"))),
            captions: rows.map((row) => textOf(row.querySelector(".q-item__label--caption"))),
            asked: asked.filter((one) => one.path === "/apps/BasicComms/Chat/activity").map((one) => one.params.kinds || "").slice(-1)[0] };
        rows[0].click();
        await until(() => root.currentParameters.chatRoomId === "R2", "the mention's room");
        out.opened = where();
        await toChat();
        return out;
    },
    // Threads: one row per thread, its newest reply; a row opens the thread
    async threads() {
        chip("threads").click();
        await until(() => document.querySelectorAll(".m-chat-feed[data-feed='threads'] .m-chat-feed-row").length === 2, "the thread rows");
        const rows = Array.from(document.querySelectorAll(".m-chat-feed .m-chat-feed-row"));
        const out = { titles: rows.map((row) => textOf(row.querySelector(".q-item__label .ellipsis"))), rooms: rows.map((row) => row.getAttribute("data-room")),
            asked: asked.filter((one) => one.path === "/apps/BasicComms/Chat/activity").map((one) => one.params.kinds || "").slice(-1)[0] };
        rows[0].click();
        await until(() => root.currentParameters.thread, "the thread");
        out.opened = where();
        chip("all").click();
        await toChat();
        return out;
    },
    // the groups of All: Recent, Phone and Bridged; Phone folds and is kept so (per user, in this browser's storage)
    async groups() {
        const headers = () => Array.from(document.querySelectorAll(".m-chat-root .m-chat-group")).map((el) => el.getAttribute("data-group"));
        const out = { groups: headers(), phoneRow: !!roomRow("X1"), bridgedRow: !!roomRow("B1") };
        document.querySelector(".m-chat-root .m-chat-group[data-group='phone']").click();
        await until(() => !roomRow("X1"), "the Phone group folded");
        out.foldedRow = !!roomRow("X1");
        out.expanded = document.querySelector(".m-chat-root .m-chat-group[data-group='phone']").getAttribute("aria-expanded");
        out.kept = Object.keys(localStorage).filter((key) => key.indexOf("moqui.chat.folded.ann") >= 0).map((key) => localStorage.getItem(key));
        document.querySelector(".m-chat-root .m-chat-group[data-group='phone']").click();
        await until(() => roomRow("X1"), "the Phone group open again");
        return out;
    },
    // the mail line on Bob's direct chat: the subject, the plain-mail label, and the thread in the Inbox
    async mailLine() {
        const line = await until(() => document.querySelector((phone() ? ".m-chat-home " : ".m-chat-root ") + ".m-chat-mail-line[data-mail='R1']"), "the mail line");
        const out = { text: textOf(line.querySelector(".col")), label: line.getAttribute("aria-label"), others: document.querySelectorAll(".m-chat-mail-line").length,
            lock: !!line.querySelector(".m-chat-e2ee-lock") };
        line.click();
        await until(() => root.view === "mail", "the Inbox");
        out.opened = where();
        if (phone()) { root.goHome(); await until(() => roomRow("R1"), "the home again"); } else await toChat();
        return out;
    },
    // the phone home's Phone filter: the number's room alone
    async phoneFilter() {
        const all = Array.from(document.querySelectorAll(".m-chat-home [data-room]")).map((el) => el.getAttribute("data-room"));
        chip("phone").click();
        await until(() => !roomRow("R1"), "the phone rooms alone");
        const phoneOnly = Array.from(document.querySelectorAll(".m-chat-home [data-room]")).map((el) => el.getAttribute("data-room"));
        chip("all").click();
        await until(() => roomRow("R1"), "all the rooms again");
        return { all, phoneOnly };
    }
};
})();
