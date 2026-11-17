/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The phone's chat in the bundled shell (PhoneChat.spec.js): Shell/Root with its home (the chat panel at full size) on a
// stand-in host (window.moquiDesktop) and a stand-in server that answers Chat's transitions as BasicComms does, its
// textRoom as SmsServices open#PairRoom does for a user with one number that texts. window.phone has a step for each
// feature of the spec, each a promise of what it saw.
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
const gone = (el, what) => until(() => !el.isConnected, what);
const rect = (el) => { const box = el.getBoundingClientRect(); return { left: box.left, top: box.top, right: box.right, bottom: box.bottom, width: box.width, height: box.height }; };
const shown = (el) => !!el && el.getClientRects().length > 0;
const clone = (value) => JSON.parse(JSON.stringify(value));
const button = (scope, label) => Array.from((scope || document).querySelectorAll("button")).find((el) => (el.getAttribute("aria-label") || el.textContent.trim()) === label);

// ---- the server ----
const OUR_NUMBER = "+15550004000";
const NOW = Date.now();
const message = (room, seq, sender, body) => ({ eventSeq: seq, type: "message", senderUserId: sender, senderName: sender === "ann" ? "Ann" : sender === "bob" ? "Bob" : "Cy",
    sentDate: NOW - 600000 + seq * 1000, body: "<p>" + body + "</p>", bodyText: body, reactions: [], attachments: [] });
const people = [{ value: "bob", label: "Bob Baker" }, { value: "cy", label: "Cy Young" }, { value: "dee", label: "Dee Dunn" }];
const rooms = {};
const events = {};
const addRoom = (room, list) => {
    rooms[room.chatRoomId] = Object.assign({ isMember: true, unreadCount: 0, lastEventSeq: list.length ? list[list.length - 1].eventSeq : 0, lastEventDate: NOW }, room);
    events[room.chatRoomId] = list;
    const last = list[list.length - 1];
    if (last) rooms[room.chatRoomId].lastMessage = { eventSeq: last.eventSeq, senderUserId: last.senderUserId, senderName: last.senderName, sentDate: last.sentDate, snippet: last.bodyText };
};
addRoom({ chatRoomId: "R1", roomTypeEnumId: "CrtDirect", displayName: "Bob Baker", otherUserId: "bob" },
    [message("R1", 1, "bob", "Lunch at noon?"), message("R1", 2, "ann", "Yes, see you there"), message("R1", 3, "bob", "Great")]);
addRoom({ chatRoomId: "R2", roomTypeEnumId: "CrtPrivate", roomName: "Ops", displayName: "Ops" }, [message("R2", 1, "cy", "Deploy at five")]);
for (let n = 3; n <= 8; n++) addRoom({ chatRoomId: "R" + n, roomTypeEnumId: "CrtPublic", roomName: "Room " + n, displayName: "Room " + n }, [message("R" + n, 1, "dee", "Note " + n)]);
const posted = [];
const requested = [];
let made = 0;
const answers = {
    "/qapps/clientConfig": () => ({ userId: "ann", username: "ann", userFullName: "Ann", locale: "en", server: { name: "Test Server", clientApi: 4 },
        calls: { appRoot: "/qapps/BasicComms" }, notificationEndpoint: "/notws" }),
    "/shellData": () => ({ apps: [], counts: {} }),
    "/apps/l10nCatalog": () => ({}),
    "/apps/BasicComms/Chat/chatData": () => ({ me: { userId: "ann" }, prefs: {}, roomList: Object.values(rooms).map(clone) }),
    "/apps/BasicComms/Chat/roomView": (params) => {
        const room = rooms[params.chatRoomId];
        const members = [{ userId: "ann", name: "Ann" }].concat(room.memberIds ? room.memberIds.map((id) => ({ userId: id, name: id })) : [{ userId: "bob", name: "Bob Baker" }]);
        return { room: clone(room), member: { lastReadEventSeq: room.lastEventSeq }, access: { isMember: true, canPost: true }, memberList: members,
            timeline: { eventList: clone(events[params.chatRoomId] || []), hasMoreBefore: false, hasMoreAfter: false } };
    },
    // a thread: its root and a reply
    "/apps/BasicComms/Chat/timeline": (params) => {
        const list = clone(events[params.chatRoomId] || []);
        if (params.threadRootEventSeq) {
            const rootSeq = Number(params.threadRootEventSeq);
            const reply = Object.assign(message(params.chatRoomId, 10, "bob", "In the thread"), { threadRootEventSeq: rootSeq });
            return { eventList: list.filter((event) => event.eventSeq === rootSeq).concat([reply]), hasMoreBefore: false, hasMoreAfter: false };
        }
        return { eventList: params.pinnedOnly ? [] : list, hasMoreBefore: false, hasMoreAfter: false };
    },
    // the people pickers: people by the term, and with numbers=true a term that is a phone number first (Chat.xml searchUsers)
    "/apps/BasicComms/Chat/searchUsers": (params) => {
        const term = String(params.term || "").trim();
        const found = people.filter((person) => term && person.label.toLowerCase().indexOf(term.toLowerCase()) >= 0);
        if (params.numbers === "true" && /^\+?[0-9 ().-]+$/.test(term) && term.replace(/[^0-9]/g, "").length >= 7) found.unshift({ value: "tel:" + term, label: "Text " + term });
        return found;
    },
    "/apps/BasicComms/Chat/textNumbers": () => ({ numberList: [{ phoneNumber: OUR_NUMBER, description: "Office" }] }),
    "/apps/BasicComms/Chat/createDirect": (params) => {
        if (params.userIds === "bob") return { chatRoomId: "R1", created: false };
        made += 1;
        const id = "D" + made;
        addRoom({ chatRoomId: id, roomTypeEnumId: "CrtDirect", displayName: params.userIds, memberIds: params.userIds.split(",") }, []);
        return { chatRoomId: id, created: true };
    },
    "/apps/BasicComms/Chat/createRoom": (params) => {
        made += 1;
        const id = "N" + made;
        addRoom({ chatRoomId: id, roomTypeEnumId: "CrtPrivate", roomName: params.roomName, displayName: params.roomName, memberIds: params.memberUserIds.split(",") }, []);
        return { chatRoomId: id, created: true };
    },
    // open#PairRoom: the pair's external room, found or made
    "/apps/BasicComms/Chat/textRoom": (params) => {
        const theirs = "+1" + String(params.textTo).replace(/[^0-9]/g, "").slice(-10);
        const found = Object.values(rooms).find((room) => room.roomTypeEnumId === "CrtExternal" && room.roomName === theirs);
        if (found) return { chatRoomId: found.chatRoomId, theirNumber: theirs };
        made += 1;
        addRoom({ chatRoomId: "X" + made, roomTypeEnumId: "CrtExternal", roomName: theirs, displayName: theirs, memberIds: [] }, []);
        return { chatRoomId: "X" + made, theirNumber: theirs };
    }
};
moqui.request = (url, options) => {
    const path = String(url).split("?")[0];
    const method = (options && options.method) || "GET";
    const params = options && moqui.isPlainObject(options.params) ? options.params : {};
    if (method === "POST") posted.push({ path, params: clone(params) });
    requested.push(path);
    moqui.noteAnswer(200, { get: () => null });
    const answer = answers[path];
    return Promise.resolve({ ok: true, status: 200, headers: { get: () => null }, json: answer ? answer(params) : {}, text: "" });
};

// ---- the host ----
const shellState = window.Vue.reactive({ accounts: [{ id: "a1", username: "ann", server: "https://s.example.com", deviceMode: "personal" }], current: "a1",
    signedIn: true, keyStore: "os", clientApi: 4, deviceMode: "personal", loadToken: null });
const kept = new Map();
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
    openRemote: () => Promise.resolve(),
    setCounts() {}, setPresence() {}, storageScope: "a1"
};

// ---- the steps ----
let root = null;
const home = () => document.querySelector(".m-chat-home");
const roomRow = (id) => document.querySelector(".m-chat-home [data-room='" + id + "']");
const openRoom = async (id) => {
    roomRow(id).click();
    return until(() => document.querySelector(".m-chat-home .m-chat-phone .m-chat-msg, .m-chat-home .m-chat-phone .m-chat-composer") && document.querySelector(".m-chat-home .m-chat-phone"), "room " + id);
};
const headerTitle = () => { const el = document.querySelector(".m-chat-phone .text-subtitle1"); return el ? el.textContent.trim() : ""; };
// type into the picker's people search, and take the option with this label
const pick = async (typed, label) => {
    const input = await until(() => document.querySelector(".m-chat-new input[type=search], .m-chat-new .q-select input"), "the picker's search");
    input.focus();
    input.value = typed;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    const option = await until(() => Array.from(document.querySelectorAll(".q-menu .q-item")).find((item) => item.textContent.trim() === label), "the option " + label);
    option.click();
    await until(() => document.querySelector(".m-chat-new").textContent.includes(label), "the chip " + label);
};
const startPicked = async () => {
    const start = await until(() => { const el = document.querySelector(".m-chat-new-start"); return el && !el.disabled && el; }, "Start enabled");
    const picker = document.querySelector(".m-chat-new");
    start.click();
    await gone(picker, "the picker to close");
};
const openPicker = async () => {
    document.querySelector(".m-chat-start").click();
    return until(() => document.querySelector(".m-chat-new"), "the picker");
};
const backHome = async () => {
    while (moqui.back.pop()) await new Promise((resolve) => setTimeout(resolve, 150));
    return until(() => roomRow("R1"), "the room list");
};
const lastPost = (name) => posted.filter((item) => item.path === "/apps/BasicComms/Chat/" + name).slice(-1)[0] || null;

window.phone = {
    async start() {
        const shell = document.createElement("div");
        shell.id = "shell";
        document.body.appendChild(shell);
        await moqui.loadScript("/files/crates/moqui-desktop/shell/js/shell.js");
        root = await until(() => moqui.webrootVue && moqui.webrootVue.commsItems && moqui.webrootVue, "the shell's root");
        await until(() => roomRow("R1"), "the home's rooms");
        return true;
    },
    // the home at 390 x 844: the list from edge to edge and down to the bottom, Start chat over it, then the picker over all
    async home() {
        const list = rect(home());
        const fab = rect(document.querySelector(".m-chat-start"));
        const row = rect(roomRow("R1"));
        const search = !!document.querySelector(".m-chat-home-search");
        const picker = rect(await openPicker());
        const pickerBack = button(document.querySelector(".m-chat-new"), "Back to chats");
        const numbersAsked = await until(() => requested.includes("/apps/BasicComms/Chat/textNumbers"), "the numbers asked");
        const closed = moqui.back.pop();
        await gone(document.querySelector(".m-chat-new"), "the picker to close on Back");
        return { viewport: { width: window.innerWidth, height: window.innerHeight, layoutWidth: document.documentElement.clientWidth,
            pageHeight: document.documentElement.scrollHeight }, list, fab, row, search, picker, pickerBack: !!pickerBack, numbersAsked, closed };
    },
    // one person: their direct room (the one there is)
    async direct() {
        await openPicker();
        await pick("bob", "Bob Baker");
        const nameField = !!document.querySelector(".m-chat-new input[name=panelRoomName]");
        await startPicked();
        await until(() => headerTitle() === "Bob Baker", "Bob's room");
        const out = { post: lastPost("createDirect"), title: headerTitle(), nameField, shellHeader: shown(document.querySelector(".q-header")) };
        await backHome();
        return out;
    },
    // two people and a name: a private room of that name
    async group() {
        await openPicker();
        await pick("bob", "Bob Baker");
        await pick("cy", "Cy Young");
        const name = await until(() => document.querySelector(".m-chat-new input[name=panelRoomName]"), "the name field");
        name.value = "Lunch crew";
        name.dispatchEvent(new Event("input", { bubbles: true }));
        await startPicked();
        await until(() => headerTitle() === "Lunch crew", "the new room");
        const out = { post: lastPost("createRoom"), title: headerTitle() };
        await backHome();
        return out;
    },
    // a phone number: the number's room, from the user's number
    async number() {
        await openPicker();
        const label = document.querySelector(".m-chat-new .q-field__label").textContent.trim();
        await pick("(555) 222-4000", "Text (555) 222-4000");
        const hint = document.querySelector(".m-chat-new-hint").textContent.trim();
        await startPicked();
        await until(() => headerTitle() === "+15552224000", "the number's room");
        const icon = document.querySelector(".m-chat-phone .q-avatar .q-icon");
        const out = { post: lastPost("textRoom"), title: headerTitle(), label, hint, icon: icon ? icon.textContent.trim() : "" };
        const id = Object.values(rooms).find((one) => one.roomTypeEnumId === "CrtExternal").chatRoomId;
        await backHome();
        out.rowIcon = (await until(() => roomRow(id), "the number's row")).querySelector(".q-avatar .q-icon").textContent.trim();
        return out;
    },
    // bubbles: mine on the right, the others' on the left
    async bubbles() {
        await openRoom("R1");
        const body = (seq) => rect(document.querySelector(".m-chat-msg[data-seq='" + seq + "'] .m-chat-body"));
        const out = { mine: body(2), theirs: body(1), mineClass: document.querySelector(".m-chat-msg[data-seq='2']").classList.contains("m-chat-mine"),
            width: window.innerWidth };
        // attach: the camera, the gallery or a file
        document.querySelector(".m-chat-attach").click();
        const menu = await until(() => document.querySelector(".m-chat-attach-menu"), "the attach menu");
        out.attach = Array.from(menu.querySelectorAll("[data-attach]")).map((item) => item.getAttribute("data-attach"));
        const camera = document.querySelector(".m-chat-composer input[name^=chatCamera]");
        const gallery = document.querySelector(".m-chat-composer input[name^=chatGallery]");
        out.camera = camera ? camera.getAttribute("accept") + " " + camera.getAttribute("capture") : "";
        out.gallery = gallery ? gallery.getAttribute("accept") + (gallery.multiple ? " multiple" : "") : "";
        moqui.back.pop();
        await gone(menu, "the attach menu to close");
        await backHome();
        return out;
    },
    // Back (moqui.back.pop, as Android's back gesture asks): the menu, then a sheet, then the thread pane, then the room;
    // at the home it closes nothing, so the app leaves
    async back() {
        const out = {};
        const room = () => document.querySelector(".m-chat-phone");
        await openRoom("R1");
        out.room = { depth: moqui.back.depth(), back: !!button(room(), "Back to chats"), shellHeader: shown(document.querySelector(".q-header")) };
        button(room(), "Room").click();
        const menu = await until(() => document.querySelector(".q-menu"), "the room's menu");
        out.menu = { popup: moqui.back.popupOpen(), closed: moqui.back.pop() };
        await gone(menu, "the menu to close on Back");
        out.menu.roomStays = !!room();
        // a long press (here its right click) on a message: its sheet, and Reply in thread opens the thread over the timeline
        document.querySelector(".m-chat-msg[data-seq='1']").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
        let sheet = await until(() => document.querySelector(".m-chat-sheet"), "the message sheet");
        Array.from(sheet.querySelectorAll(".q-item")).find((item) => item.textContent.includes("Reply in thread")).click();
        await gone(sheet, "the sheet to close");
        await until(() => room().querySelector(".m-chat-msg[data-seq='10']"), "the thread");
        out.thread = { depth: moqui.back.depth(), back: !!button(room(), "Back to the room") };
        room().querySelector(".m-chat-msg[data-seq='10']").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
        sheet = await until(() => document.querySelector(".m-chat-sheet"), "the thread message's sheet");
        out.sheet = { popup: moqui.back.popupOpen(), closed: moqui.back.pop() };
        await gone(sheet, "the sheet to close on Back");
        out.sheet.threadStays = !!room().querySelector(".m-chat-msg[data-seq='10']");
        out.pane = { closed: moqui.back.pop() };
        await until(() => room().querySelector(".m-chat-msg[data-seq='3']"), "the timeline again");
        out.pane.roomStays = !!room() && !room().querySelector(".m-chat-msg[data-seq='10']");
        out.leave = { closed: moqui.back.pop() };
        await until(() => roomRow("R1"), "the list after Back");
        out.leave.list = !!roomRow("R1");
        out.leave.view = root.view;
        out.home = { depth: moqui.back.depth(), closed: moqui.back.pop() };
        return out;
    }
};
})();
