/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// Reactions and Info by touch in the bundled shell (PhoneReactions.spec.js): Shell/Root with its home on a stand-in host
// (window.moquiDesktop) and a stand-in server that answers Chat's transitions as BasicComms does: a direct room with Bob, whose
// read marker is at Ann's message. window.phone has a step for each feature of the spec, each a promise of what it saw.
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
const gone = (el, what) => until(() => !el.isConnected, what);
const clone = (value) => JSON.parse(JSON.stringify(value));

// ---- made touches: a tap is touchstart, touchend and the click a phone sends after it; a long press holds 600 ms ----
let touchId = 0;
const touch = (el, type) => {
    const box = el.getBoundingClientRect();
    const point = new Touch({ identifier: ++touchId, target: el, clientX: box.left + box.width / 2, clientY: box.top + box.height / 2 });
    const list = type === "touchend" ? [] : [point];
    return el.dispatchEvent(new TouchEvent(type, { touches: list, targetTouches: list, changedTouches: [point], bubbles: true, cancelable: true }));
};
const tap = async (el) => {
    touch(el, "touchstart");
    await wait(40);
    // a phone sends no click after a touchend whose default was prevented
    if (touch(el, "touchend")) el.click();
    await wait(40);
};
const press = async (el) => {
    touch(el, "touchstart");
    await wait(600);
    const clicks = touch(el, "touchend");
    await wait(40);
    return clicks;
};

// ---- the server ----
const NOW = Date.now();
const message = (seq, sender, body) => ({ eventSeq: seq, type: "message", senderUserId: sender, senderName: sender === "ann" ? "Ann" : "Bob Baker",
    sentDate: NOW - 600000 + seq * 1000, body: "<p>" + body + "</p>", bodyText: body, reactions: [], attachments: [] });
const room = { chatRoomId: "R1", roomTypeEnumId: "CrtDirect", displayName: "Bob Baker", otherUserId: "bob", isMember: true, unreadCount: 0, lastEventSeq: 3, lastEventDate: NOW };
// Bob reacted 👍 to his own first message already
const events = [Object.assign(message(1, "bob", "Lunch at noon?"), { reactions: [{ key: "👍", count: 1, userIds: ["bob"], mine: false }] }),
    message(2, "ann", "Yes, see you there"), message(3, "bob", "Great")];
const members = [{ userId: "ann", name: "Ann", lastReadEventSeq: 3 }, { userId: "bob", name: "Bob Baker", lastReadEventSeq: 2 }];
const posted = [];
const requested = [];
const answers = {
    "/qapps/clientConfig": () => ({ userId: "ann", username: "ann", userFullName: "Ann", locale: "en", server: { name: "Test Server", clientApi: 4 },
        calls: { appRoot: "/qapps/BasicComms" }, notificationEndpoint: "/notws" }),
    "/shellData": () => ({ apps: [], counts: {} }),
    "/apps/l10nCatalog": () => ({}),
    "/apps/BasicComms/Chat/chatData": () => ({ me: { userId: "ann" }, prefs: {}, roomList: [clone(room)] }),
    "/apps/BasicComms/Chat/roomView": () => ({ room: clone(room), member: { lastReadEventSeq: 3 }, access: { isMember: true, canPost: true }, memberList: clone(members),
        timeline: { eventList: clone(events), hasMoreBefore: false, hasMoreAfter: false } }),
    // one event (Info): it, with the members' read markers as get#Timeline gives them
    "/apps/BasicComms/Chat/timeline": (params) => {
        const list = params.eventSeq != null ? events.filter((event) => event.eventSeq === Number(params.eventSeq)) : events;
        return { eventList: clone(list), hasMoreBefore: false, hasMoreAfter: false, memberList: clone(members) };
    },
    "/apps/BasicComms/Chat/react": () => ({})
};
moqui.request = (url, options) => {
    const path = String(url).split("?")[0];
    const method = (options && options.method) || "GET";
    const params = options && moqui.isPlainObject(options.params) ? options.params : {};
    if (method === "POST") posted.push({ path, params: clone(params) });
    requested.push({ path, params: clone(params) });
    moqui.noteAnswer(200, { get: () => null });
    const answer = answers[path];
    return Promise.resolve({ ok: true, status: 200, headers: { get: () => null }, json: answer ? answer(params) : {}, text: "" });
};
const reacts = () => posted.filter((item) => item.path === "/apps/BasicComms/Chat/react");

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
const msg = (seq) => document.querySelector(".m-chat-phone .m-chat-msg[data-seq='" + seq + "']");
const chip = (seq, key) => { const row = msg(seq); return row && row.querySelector(".m-chat-reaction[data-reaction='" + key + "']"); };
const openSheet = async (seq) => {
    await press(msg(seq).querySelector(".m-chat-body"));
    return until(() => document.querySelector(".m-chat-sheet"), "the sheet of message " + seq);
};
const sheetItem = (sheet, key) => sheet.querySelector("[data-action='" + key + "']");

window.phone = {
    async start() {
        Quasar.Platform.is.mobile = true;
        const shell = document.createElement("div");
        shell.id = "shell";
        document.body.appendChild(shell);
        try { localStorage.clear(); } catch (err) { /* no storage */ }
        await moqui.loadScript("/files/crates/moqui-desktop/shell/js/shell.js");
        const row = await until(() => document.querySelector(".m-chat-home [data-room='R1']"), "the home's rooms");
        await tap(row);
        await until(() => msg(3), "the room");
        return true;
    },
    async react() {
        const sheet = await openSheet(1);
        const actions = Array.from(sheet.querySelectorAll("[data-action]")).map((el) => el.getAttribute("data-action"));
        const thumbs = Array.from(sheet.querySelectorAll(".m-chat-sheet-emoji")).find((el) => el.textContent.trim() === "👍");
        await tap(thumbs);
        await gone(sheet, "the sheet to close");
        const shown = await until(() => { const el = chip(1, "👍"); return el && el.textContent.includes("👍 2") && el; }, "my reaction on the chip");
        return { sheet: true, actions, posted: reacts().slice(-1)[0].params, chip: { text: shown.textContent.trim().replace(/\s+/g, " "),
            mine: shown.classList.contains("bg-primary") } };
    },
    async whoReacted() {
        const count = reacts().length;
        await press(chip(1, "👍"));
        const box = await until(() => document.querySelector(".m-chat-people"), "who reacted");
        const people = Array.from(box.querySelectorAll("[data-person]")).map((el) => el.querySelector(".q-item__section--main").textContent.trim());
        const title = box.querySelector(".text-subtitle1").textContent.trim();
        await wait(100);
        const reactsAfterHold = reacts().length - count;
        await tap(box.querySelector("button[aria-label='Close']"));
        await gone(box, "the list to close");
        await tap(chip(1, "👍"));
        await until(() => reacts().length === count + 1, "the reaction taken back");
        await until(() => { const el = chip(1, "👍"); return el && el.textContent.includes("👍 1"); }, "the chip with Bob's only");
        // Bob's stays; mine is gone from it
        return { people, title, reactsAfterHold, closed: true, removed: reacts().slice(-1)[0].params, notMine: !chip(1, "👍").classList.contains("bg-primary") };
    },
    async picker() {
        const sheet = await openSheet(3);
        await tap(sheet.querySelector(".m-chat-more-reactions"));
        const picker = await until(() => sheet.querySelector(".m-emoji-picker .m-emoji-cell") && sheet.querySelector(".m-emoji-picker"), "the picker");
        const tabs = picker.querySelectorAll(".m-emoji-tabs [data-group]").length;
        const input = picker.querySelector("input");
        input.value = "party popper";
        input.dispatchEvent(new Event("input", { bubbles: true }));
        const cell = await until(() => picker.querySelector(".m-emoji-cell[data-emoji='🎉']"), "the search's emoji");
        const found = Array.from(picker.querySelectorAll(".m-emoji-cell")).map((el) => el.getAttribute("data-emoji"));
        await tap(cell);
        await gone(sheet, "the sheet to close");
        const shown = await until(() => chip(3, "🎉"), "the picked reaction");
        return { picker: true, tabs, found, posted: reacts().slice(-1)[0].params, chip: !!shown };
    },
    async info() {
        const sheet = await openSheet(2);
        await tap(sheetItem(sheet, "info"));
        const box = await until(() => document.querySelector(".m-chat-info [data-info='read']") && document.querySelector(".m-chat-info"), "Info");
        await until(() => !box.querySelector(".q-inner-loading"), "Info read again");
        const line = (key) => box.querySelector("[data-info='" + key + "']").textContent.trim();
        const asked = requested.filter((item) => item.path === "/apps/BasicComms/Chat/timeline").slice(-1)[0].params;
        const out = { asked, sent: line("sent"), readBy: line("read"), note: line("note") };
        await tap(box.querySelector("button[aria-label='Close']"));
        await gone(box, "Info to close");
        out.closed = true;
        return out;
    }
};
})();
