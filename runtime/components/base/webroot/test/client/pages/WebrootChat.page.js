// The chat store, in the page. Was js/WebrootChat.test.js.
// WebrootVue.qvt.js starts the Vue app only when the page has #moqui-config plus Vue and Quasar. This page has none of those.
const chat = moqui.chat;
const compiler = globalThis.VueCompilerDOM;

const freshState = () => ({ me: null, presence: {}, unread: {}, typing: {}, reads: {}, openRooms: {}, statusDialog: false, outbox: [], sending: {},
    online: true, connected: null });
const push = (topic, message) => ({ topic, message });
const event = (fields) => Object.assign({ chatRoomId: "R1", type: "message", senderUserId: "bob", sentDate: 1000, mentions: [], content: {} }, fields);

test("presence and status from the userStatus topic, the user's own with the status chosen", () => {
    const state = freshState();
    assert.deepEqual(chat.reduce(state, push("userStatus", { userId: "bob", presence: "away", statusText: "Lunch", emoji: "🍜" }), "ann", 5),
        { kind: "presence", userId: "bob", presence: "away" });
    assert.equal(state.presence.bob.presence, "away");
    assert.equal(state.presence.bob.statusText, "Lunch");
    chat.reduce(state, push("userStatus", JSON.stringify({ userId: "ann", presence: "offline", self: true, status: "invisible" })), "ann", 6);
    assert.equal(state.me.status, "invisible");
    assert.equal(state.presence.ann.presence, "offline");
    assert.equal(chat.reduce(state, push("userStatus", { presence: "online" }), "ann", 7), null, "no user, nothing");
    assert.equal(chat.reduce(state, { topic: "userStatus" }, "ann", 7), null);
    assert.equal(chat.reduce(state, null, "ann", 7), null);
});

test("typing comes and goes, a message ends it; read markers are kept by room and user", () => {
    const state = freshState();
    chat.reduce(state, push("chatRoom:R1", { kind: "typing", chatRoomId: "R1", userId: "bob", typing: true }), "ann", 1000);
    chat.reduce(state, push("chatRoom:R1", { kind: "typing", chatRoomId: "R1", userId: "cat", typing: true }), "ann", 1000);
    assert.deepEqual(chat.typingUsers(state, "R1", 2000), ["bob", "cat"]);
    assert.deepEqual(chat.typingUsers(state, "R1", 8000), [], "a typing signal lasts a few seconds");
    chat.reduce(state, push("chatRoom:R1", { kind: "typing", chatRoomId: "R1", userId: "cat", typing: false }), "ann", 1500);
    assert.deepEqual(chat.typingUsers(state, "R1", 2000), ["bob"]);
    chat.reduce(state, push("chatRoom:R1", { kind: "event", chatRoomId: "R1", eventSeq: 4, event: event({ eventSeq: 4 }) }), "ann", 1600);
    assert.deepEqual(chat.typingUsers(state, "R1", 2000), []);
    const read = chat.reduce(state, push("chatRoom:R1", { kind: "read", chatRoomId: "R1", userId: "bob", lastReadEventSeq: 4 }), "ann", 1700);
    assert.deepEqual(read, { kind: "read", chatRoomId: "R1", userId: "bob", lastReadEventSeq: 4 });
    assert.deepEqual(state.reads.R1, { bob: 4 });
});

test("unread and mention counts: others' messages in rooms not shown, cleared by a read marker of this user", () => {
    const state = freshState();
    const send = (room, fields) => chat.reduce(state, push("chatRoom:" + room, { kind: "event", chatRoomId: room, event: event(Object.assign({ chatRoomId: room }, fields)) }), "ann", 1);
    send("R1", { eventSeq: 2 });
    send("R1", { eventSeq: 3, mentions: ["ann"] });
    send("R1", { eventSeq: 4, senderUserId: "ann" });
    send("R1", { eventSeq: 5, type: "reaction" });
    send("R2", { eventSeq: 7 });
    assert.deepEqual(chat.unreadOf(state, "R1"), { unread: 2, mentions: 1 });
    assert.equal(chat.unreadTotal(state), 3);
    state.openRooms.R2 = 1;
    send("R2", { eventSeq: 8 });
    assert.deepEqual(chat.unreadOf(state, "R2"), { unread: 1, mentions: 0 }, "a room shown now does not count");
    assert.deepEqual(chat.reduce(state, push("chatUser:ann", { kind: "read", chatRoomId: "R1", lastReadEventSeq: 5 }), "ann", 2),
        { kind: "read", chatRoomId: "R1", userId: "ann", all: false, lastReadEventSeq: 5 });
    assert.deepEqual(chat.unreadOf(state, "R1"), { unread: 0, mentions: 0 });
    chat.reduce(state, push("chatUser:ann", { kind: "read", all: true }), "ann", 3);
    assert.equal(chat.unreadTotal(state), 0);
    send("R3", { eventSeq: 2 });
    assert.deepEqual(chat.reduce(state, push("chatUser:ann", { kind: "room", action: "left", chatRoomId: "R3" }), "ann", 4),
        { kind: "room", chatRoomId: "R3", action: "left" });
    assert.equal(state.unread.R3, undefined);
    assert.equal(chat.reduce(state, push("chatUser:ann", { kind: "other" }), "ann", 5), null);
});

test("Spam & blocked (Comms-Messaging §18.3 phase 11): a filtered room is level none and counts nothing; the server's counts reset a room they no longer name; hidden messages fold into one line", () => {
    const state = Object.assign(freshState(), { levels: { R1: "all" } });
    assert.equal(chat.levelOf("CnlAll", false, "spam"), "none");
    assert.equal(chat.levelOf("CnlAll", false, null), "all");
    state.unread.R1 = { unread: 3, mentions: 1 };
    chat.reduce(state, push("chatUser:ann", { kind: "room", action: "settings", chatRoomId: "R1", notificationLevelEnumId: "CnlAll", filtered: "spam" }), "ann", 1);
    assert.equal(state.levels.R1, "none");
    assert.deepEqual(chat.unreadOf(state, "R1"), { unread: 0, mentions: 0 });
    chat.reduce(state, push("chatUser:ann", { kind: "room", action: "settings", chatRoomId: "R1", notificationLevelEnumId: "CnlAll", filtered: null }), "ann", 2);
    assert.equal(state.levels.R1, "all");
    state.unread = { R1: { unread: 2, mentions: 0 }, R2: { unread: 4, mentions: 1 } };
    chat.applyCounts(state, { R2: { unread: 1, mentions: 0, counted: 1 }, R3: { unread: 5, mentions: 2 } });
    assert.deepEqual(state.unread, { R1: { unread: 0, mentions: 0 }, R2: { unread: 1, mentions: 0 }, R3: { unread: 5, mentions: 2 } });
    const out = [];
    const rows = [event({ eventSeq: 1 }), event({ eventSeq: 2, hidden: "blocked" }), event({ eventSeq: 3, hidden: "blocked" }), event({ eventSeq: 4 }), event({ eventSeq: 5, hidden: "blocked" })];
    const kinds = rows.map((row) => chat.hiddenLine(out, row) ? "h" : (out.push({ kind: "message", eventSeq: row.eventSeq }), "m"));
    assert.deepEqual(kinds, ["m", "h", "h", "m", "h"]);
    assert.deepEqual(out.map((row) => row.kind), ["message", "hidden", "message", "hidden"]);
    assert.equal(out[1].eventSeq, 2);
});

test("a pushed event merged into a timeline: messages in order, edits, reactions, redactions, pins and threads", () => {
    const list = [event({ eventSeq: 2, bodyText: "one", reactions: [], threadReplyCount: 0 }), event({ eventSeq: 5, bodyText: "three" })];
    chat.applyEvent(list, event({ eventSeq: 3, bodyText: "two" }), "ann");
    chat.applyEvent(list, event({ eventSeq: 3, bodyText: "two again" }), "ann");
    assert.deepEqual(list.map((item) => item.bodyText), ["one", "two", "three"], "in order, once");
    assert.equal(list[1].redacted, false);
    chat.applyEvent(list, event({ eventSeq: 6, type: "edit", editsEventSeq: 3, body: "<p>two, edited</p>", bodyText: "two, edited", sentDate: 2000 }), "ann");
    assert.equal(list[1].body, "<p>two, edited</p>");
    assert.equal(list[1].edited, true);
    assert.equal(list.length, 3, "an edit is not a row");
    chat.applyEvent(list, event({ eventSeq: 7, type: "reaction", relatesToEventSeq: 2, senderUserId: "ann", content: { key: "👍" } }), "ann");
    chat.applyEvent(list, event({ eventSeq: 8, type: "reaction", relatesToEventSeq: 2, senderUserId: "bob", content: { key: "👍" } }), "ann");
    chat.applyEvent(list, event({ eventSeq: 9, type: "reaction", relatesToEventSeq: 2, senderUserId: "bob", content: { key: "🎉" } }), "ann");
    assert.deepEqual(list[0].reactions, [{ key: "👍", count: 2, userIds: ["ann", "bob"], mine: true }, { key: "🎉", count: 1, userIds: ["bob"], mine: false }]);
    chat.applyEvent(list, event({ eventSeq: 10, type: "redaction", relatesToEventSeq: 7, senderUserId: "ann", content: { target: "reaction", messageSeq: 2, key: "👍" } }), "ann");
    chat.applyEvent(list, event({ eventSeq: 11, type: "redaction", relatesToEventSeq: 9, senderUserId: "bob", content: { target: "reaction", messageSeq: 2, key: "🎉" } }), "ann");
    assert.deepEqual(list[0].reactions, [{ key: "👍", count: 1, userIds: ["bob"], mine: false }]);
    // a thread reply counts on its root in the main timeline, and is a row of that thread
    chat.applyEvent(list, event({ eventSeq: 12, threadRootEventSeq: 2, bodyText: "reply", sentDate: 3000 }), "ann");
    assert.equal(list.length, 3);
    assert.equal(list[0].threadReplyCount, 1);
    assert.equal(list[0].threadLastDate, 3000);
    const thread = [event({ eventSeq: 2, bodyText: "one" })];
    chat.applyEvent(thread, event({ eventSeq: 12, threadRootEventSeq: 2, bodyText: "reply" }), "ann", 2);
    chat.applyEvent(thread, event({ eventSeq: 13, bodyText: "main timeline" }), "ann", 2);
    assert.deepEqual(thread.map((item) => item.bodyText), ["one", "reply"]);
    // a pin marks its message and is a row too; a redaction removes the content
    chat.applyEvent(list, event({ eventSeq: 14, type: "pin", relatesToEventSeq: 5, senderUserId: "ann", content: { pinned: true } }), "ann");
    assert.equal(list[2].pinned, true);
    assert.equal(list[3].type, "pin");
    chat.applyEvent(list, event({ eventSeq: 15, type: "redaction", relatesToEventSeq: 5, content: { target: "message" } }), "ann");
    assert.equal(list[2].redacted, true);
    assert.equal(list[2].body, null);
    assert.equal(list[2].pinned, false);
    assert.equal(list.length, 4, "a redaction is not a row");
    assert.equal(chat.applyEvent(null, event({ eventSeq: 1 })), null);
});

test("an external actor of a bridged room reacts and takes it back; a delivery update keeps the message's own fields", () => {
    const list = [event({ eventSeq: 2, bodyText: "one", reactions: [] })];
    chat.applyEvent(list, event({ eventSeq: 3, type: "reaction", relatesToEventSeq: 2, senderUserId: null, senderActorId: "XA100000", content: { key: "👍" } }), "ann");
    chat.applyEvent(list, event({ eventSeq: 4, type: "reaction", relatesToEventSeq: 2, senderUserId: null, senderActorId: "XA100001", content: { key: "👍" } }), "ann");
    assert.deepEqual(list[0].reactions, [{ key: "👍", count: 2, userIds: ["XA100000", "XA100001"], mine: false }]);
    chat.applyEvent(list, event({ eventSeq: 5, type: "redaction", relatesToEventSeq: 3, senderUserId: null, senderActorId: "XA100000",
        content: { target: "reaction", messageSeq: 2, key: "👍" } }), "ann");
    assert.deepEqual(list[0].reactions, [{ key: "👍", count: 1, userIds: ["XA100001"], mine: false }]);
    chat.applyUpdate(list, { eventSeq: 2, fields: { delivery: { state: "failed", retryable: true, destinations: [{ state: "failed", networkName: "SMS" }] } } }, "ann");
    assert.equal(list[0].delivery.state, "failed");
    assert.equal(list[0].bodyText, "one");
});

test("the user's own dot and do not disturb: no toasts, the notifications still come", () => {
    const saved = { state: chat.state, vm: chat.vm, me: chat.me };
    chat.state = freshState();
    let display = true;
    chat.vm = { notificationClient: { disableDisplay: () => { display = false; }, enableDisplay: () => { display = true; } } };
    chat.me = "ann";
    assert.equal(chat.myPresence(), "");
    chat.setMe({ status: "online", presence: "online", autoAwayMinutes: 15 });
    assert.equal(chat.myPresence(), "online");
    assert.equal(chat.autoAwayMinutes, 15);
    assert.equal(chat.presenceOf("ann"), "online");
    chat.setMe({ status: "dnd", presence: "dnd" });
    assert.equal(chat.myPresence(), "dnd");
    assert.equal(chat.quiet(), true);
    assert.equal(display, false, "toasts are off");
    chat.receive(push("userStatus", { userId: "ann", presence: "online", self: true, status: "online" }));
    assert.equal(chat.quiet(), false);
    assert.equal(display, true);
    chat.setMe({ status: "invisible", presence: "offline" });
    assert.equal(chat.myPresence(), "offline", "invisible is a grey dot");
    chat.setMe({ status: "online", presence: "away" });
    assert.equal(chat.myPresence(), "away", "idle: away");
    chat.state.presence.bob = { presence: "dnd" };
    assert.equal(chat.presenceOf("bob"), "dnd");
    assert.equal(chat.presenceOf("cat"), "", "not known yet");
    assert.equal(chat.presenceOf(""), "");
    Object.assign(chat, saved);
});

test("the clear-at choices and the idle time", () => {
    const now = new Date(2026, 9, 21, 10, 30, 0).getTime(); // a Wednesday
    assert.equal(chat.clearAt(0, now), null);
    assert.equal(chat.clearAt(30, now), now + 30 * 60000);
    assert.equal(chat.clearAt("240", now), now + 240 * 60000);
    const today = new Date(chat.clearAt("today", now));
    assert.deepEqual([today.getDate(), today.getHours(), today.getMinutes()], [21, 23, 59]);
    const week = new Date(chat.clearAt("week", now));
    assert.deepEqual([week.getDay(), week.getDate(), week.getHours()], [0, 25, 23]);
    assert.deepEqual(chat.clearOptions().map((option) => option.value), [0, 30, 60, 240, "today", "week"]);
    assert.equal(chat.clearOptions({ today: "Hoy" })[4].label, "Hoy");
    assert.equal(chat.idleSeconds(1000, 62500), 61);
    assert.equal(chat.idleSeconds(5000, 1000), 0);
    assert.equal(chat.backFromIdle(600, 10), true);
    assert.equal(chat.backFromIdle(599, 10), false);
    assert.equal(chat.backFromIdle(700, 30), false);
    assert.equal(chat.dot("invisible"), "offline");
    assert.equal(chat.dot("nope"), "");
});

test("the status dialog's presets: the user's CHAT_STATUS_PRESETS, else the defaults localized; a preset fills the dialog", () => {
    const defaults = chat.statusPresets({}, (text) => "~" + text);
    assert.equal(defaults.length, 5);
    assert.deepEqual(defaults[0], { emoji: "📅", text: "~In a meeting", clearAfter: 60 });
    assert.deepEqual(defaults.map((preset) => preset.clearAfter), [60, 30, "today", "today", 0]);
    const kept = JSON.stringify([{ emoji: "🚀", text: "Shipping", clearAfter: "240" }, { text: "Lunch", clearAfter: "45" }, { emoji: "" , text: "" },
        { emoji: "🌙", clearAfter: "week" }]);
    // the user's own are as they wrote them (not localized); a clear-after that is not a choice of the dialog is never
    assert.deepEqual(chat.statusPresets({ CHAT_STATUS_PRESETS: kept }, (text) => "~" + text), [
        { emoji: "🚀", text: "Shipping", clearAfter: 240 }, { emoji: "", text: "Lunch", clearAfter: 0 }, { emoji: "🌙", text: "", clearAfter: "week" }]);
    // an empty list is the user's choice; text that is not JSON is the defaults
    assert.deepEqual(chat.statusPresets({ CHAT_STATUS_PRESETS: "[]" }), []);
    assert.equal(chat.statusPresets({ CHAT_STATUS_PRESETS: "not json" }).length, 5);
    assert.equal(chat.statusPresets({ CHAT_STATUS_PRESETS: JSON.stringify(new Array(20).fill({ text: "x" })) }).length, 12);
    const dialog = moqui.qvtComponents["m-user-status-dialog"];
    const self = Object.assign(dialog.data(), { clearOptions: chat.clearOptions({}) });
    dialog.methods.usePreset.call(self, { emoji: "🚀", text: "Shipping", clearAfter: 240 });
    assert.deepEqual([self.emoji, self.text, self.clear], ["🚀", "Shipping", 240]);
    assert.equal(dialog.methods.clearLabel.call(self, 240), "4 hours");
    assert.equal(dialog.methods.clearLabel.call(self, 0), "");
});

test("the tray's Chat tab reloads for an event no screen listens to, never for typing or a read marker", () => {
    assert.equal(chat.trayReloads(push("chatRoom:R1", { kind: "typing" })), false);
    assert.equal(chat.trayReloads(push("chatRoom:R1", { kind: "read" })), false);
    assert.equal(chat.trayReloads(push("chatRoom:R1", { kind: "event" })), true);
    assert.equal(chat.trayReloads({ topic: "chatRoom:R1" }), true);
    const off = chat.on("event", () => {});
    assert.equal(chat.trayReloads(push("chatRoom:R1", { kind: "event" })), false, "a live room list or room");
    off();
    assert.equal(chat.trayReloads(push("chatRoom:R1", { kind: "event" })), true);
});

test("listeners hear the reduced messages; one that throws does not stop the others", () => {
    const saved = { state: chat.state, me: chat.me };
    chat.state = freshState();
    chat.me = "ann";
    const heard = [];
    const offs = [chat.on("event", () => { throw new Error("broken listener"); }), chat.on("event", (payload) => heard.push(payload.event.eventSeq)),
        chat.on("typing", (payload) => heard.push("typing " + payload.userId))];
    const errors = console.error;
    console.error = () => {};
    chat.receive(push("chatRoom:R1", { kind: "event", chatRoomId: "R1", event: event({ eventSeq: 9 }) }));
    chat.receive(push("chatRoom:R1", { kind: "typing", chatRoomId: "R1", userId: "bob", typing: true }));
    chat.receive(push("shellCounts", { chat: 1 }));
    console.error = errors;
    assert.deepEqual(heard, [9, "typing bob"]);
    offs.forEach((off) => off());
    Object.assign(chat, saved);
});

test("the store starts once: its topics, the user's status, the counts and a heartbeat; presence asks go as one request", async () => {
    const requests = [];
    const listeners = [];
    const original = moqui.request;
    moqui.request = (url, options) => {
        requests.push({ url, method: (options && options.method) || "GET", params: options && options.params });
        let json = {};
        if (url.endsWith("/userStatus") || url.endsWith("/userStatusHeartbeat")) json = { myStatus: { status: "online", presence: "online", autoAwayMinutes: 10 } };
        if (url.endsWith("/chatUnread")) json = { roomCounts: { R1: { unread: 2, mentions: 1 } } };
        if (url.endsWith("/userStatuses")) json = { statusMap: { bob: { presence: "away" }, cat: { presence: "online" } } };
        return Promise.resolve({ json });
    };
    const client = { registerListener: (topic, fn) => listeners.push(topic), disableDisplay() {}, enableDisplay() {} };
    chat.state = freshState();
    chat.start({ userId: "ann", appRootPath: "", notificationClient: client });
    chat.start({ userId: "ann", appRootPath: "", notificationClient: client });
    clearInterval(chat.beatTimer);
    clearInterval(chat.flushTimer);
    assert.deepEqual(listeners, ["ALL", "chatUser:ann", "userStatus"]);
    await new Promise((resolve) => setTimeout(resolve, 10));
    assert.deepEqual(requests.map((request) => request.method + " " + request.url), ["GET /userStatus", "GET /chatUnread", "POST /userStatusHeartbeat"]);
    assert.equal(typeof requests[2].params.idleSeconds, "number");
    assert.deepEqual(chat.unreadOf(chat.state, "R1"), { unread: 2, mentions: 1 });
    assert.equal(chat.myPresence(), "online");
    // the avatars of a page ask at once: one request, and not again soon
    chat.watchUsers(["bob", "ann"]);
    chat.watchUsers(["cat", "bob"]);
    await new Promise((resolve) => setTimeout(resolve, 80));
    const asked = requests.filter((request) => request.url.endsWith("/userStatuses"));
    assert.equal(asked.length, 1);
    assert.equal(asked[0].params.userIds, "bob,cat");
    assert.equal(chat.presenceOf("bob"), "away");
    chat.watchUsers(["bob"]);
    await new Promise((resolve) => setTimeout(resolve, 80));
    assert.equal(requests.filter((request) => request.url.endsWith("/userStatuses")).length, 1);
    // a room shown: its topic, and its count read
    chat.openRoom("R1");
    assert.deepEqual(chat.unreadOf(chat.state, "R1"), { unread: 0, mentions: 0 });
    assert.equal(listeners[listeners.length - 1], "chatRoom:R1");
    chat.closeRoom("R1");
    assert.equal(chat.state.openRooms.R1, undefined);
    // typing goes at most every 3 seconds, and once when it stops
    chat.typing("R1");
    chat.typing("R1");
    chat.typing("R1", false);
    chat.typing("R1", false);
    const typed = requests.filter((request) => request.url.endsWith("/chatTyping"));
    assert.deepEqual(typed.map((request) => request.params.typing), [true, false]);
    // the status of the account menu, with a clear-at time
    await chat.setStatus({ status: "dnd", statusText: "Focus", clearAt: 30 });
    const set = requests.filter((request) => request.url.endsWith("/setUserStatus"))[0];
    assert.equal(set.method, "POST");
    assert.equal(set.params.status, "dnd");
    assert.ok(set.params.clearAtDate > Date.now(), "clear-at as epoch millis");
    assert.equal(set.params.clearAt, undefined);
    moqui.request = original;
});

test("the account menu's components compile, and WebrootVue.qvt.js registers them before the shell", () => {
    ["m-user-status-menu", "m-user-status-dialog", "m-assist-card"].forEach((name) => {
        const comp = moqui.qvt.components[name];
        assert.ok(comp && comp.template, name);
        const errors = [];
        compiler.compile(comp.template, { mode: "function", onError: (err) => errors.push(err.message) });
        assert.deepEqual(errors, [], name);
    });
    const shell = moqui.qvt.components["m-qapps-root"].template;
    assert.match(shell, /<m-user-status-menu v-if="chatStatus"><\/m-user-status-menu>/);
    assert.match(shell, /<m-user-status-dialog v-if="chatStatus"><\/m-user-status-dialog>/);
    assert.match(shell, /<m-avatar :name="userFullName \|\| username" :user-id="userId"/);
    const avatar = moqui.qvt.components["m-avatar"];
    assert.ok(avatar.props.userId);
    // an avatar's dot: its presence, else the store's for its user
    const saved = { state: chat.state, me: chat.me };
    chat.state = freshState();
    chat.state.presence.bob = { presence: "away" };
    assert.equal(avatar.computed.shownPresence.call({ presence: "", userId: "bob" }), "away");
    assert.equal(avatar.computed.shownPresence.call({ presence: "dnd", userId: "bob" }), "dnd");
    assert.equal(avatar.computed.shownPresence.call({ presence: "" }), "");
    Object.assign(chat, saved);
    const menu = moqui.qvt.components["m-user-status-menu"];
    assert.deepEqual(menu.computed.options.call({ labels: chat.labels }).map((option) => option.value + ":" + option.dot),
        ["online:online", "away:away", "dnd:dnd", "invisible:offline"]);
});

test("a thread reply names its root's last reply; a pushed update changes a message and keeps what is mine", () => {
    const list = [event({ eventSeq: 2, bodyText: "root", threadReplyCount: 0 }),
        event({ eventSeq: 3, bodyText: "poll", poll: { question: "Lunch?", mine: [1], options: [{ id: 1, text: "Pizza", voteCount: 1, mine: true },
            { id: 2, text: "Sushi", voteCount: 0, mine: false }] } })];
    chat.applyEvent(list, event({ eventSeq: 4, threadRootEventSeq: 2, senderUserId: "bob", sentDate: 4000 }), "ann");
    assert.equal(list[0].threadLastEventSeq, 4);
    assert.equal(list[0].threadLastUserId, "bob");
    const state = freshState();
    const update = chat.reduce(state, push("chatRoom:R1", { kind: "update", chatRoomId: "R1", eventSeq: 3, threadRootEventSeq: null,
        fields: { poll: { question: "Lunch?", options: [{ id: 2, text: "Sushi", voteCount: 1 }, { id: 1, text: "Pizza", voteCount: 1 }] } } }), "ann", 1);
    assert.equal(update.kind, "update");
    assert.equal(update.eventSeq, 3);
    assert.equal(chat.reduce(state, push("chatRoom:R1", { kind: "update", eventSeq: 3 }), "ann", 1), null, "no fields, nothing");
    assert.equal(chat.trayReloads(push("chatRoom:R1", { kind: "update" })), false);
    chat.applyUpdate(list, update, "ann");
    const poll = list[1].poll;
    assert.deepEqual(poll.mine, [1], "my votes stay: the push has none");
    assert.deepEqual(poll.options.map((option) => [option.id, option.voteCount, option.mine]), [[2, 1, false], [1, 1, true]]);
    chat.applyUpdate(list, { eventSeq: 2, fields: { linkPreviews: [{ url: "https://example.com/", title: "Example" }] } }, "ann");
    assert.equal(list[0].linkPreviews[0].title, "Example");
    chat.applyUpdate(list, { eventSeq: 9, fields: { linkPreviews: [] } }, "ann");
    assert.equal(list.length, 2, "a message not in the list is left");
});

test("the outbox: at most 50 items of 64 KB, by room and thread, files that wait too long, and what a failed send does next", () => {
    let list = [];
    for (let n = 0; n < 50; n++) list = chat.outboxAdd(list, { clientRef: "c" + n, chatRoomId: "R1" });
    assert.equal(list.length, 50);
    assert.equal(chat.outboxAdd(list, { clientRef: "c50", chatRoomId: "R1" }), null, "full");
    assert.equal(chat.outboxAdd([], { clientRef: "big", chatRoomId: "R1", params: { body: "x".repeat(70 * 1024) } }), null, "too large");
    assert.equal(chat.outboxAdd([], { chatRoomId: "R1" }), null, "no clientRef");
    assert.equal(chat.outboxRemove(list, "c3").length, 49);
    const now = 100 * 3600 * 1000;
    const items = [{ clientRef: "a", chatRoomId: "R1", createdDate: now }, { clientRef: "b", chatRoomId: "R1", threadRootEventSeq: 7, createdDate: now },
        { clientRef: "c", chatRoomId: "R2", createdDate: now }, { clientRef: "d", chatRoomId: "R1", hasFiles: true, createdDate: now - 21 * 3600 * 1000, status: "waiting" }];
    assert.deepEqual(chat.outboxFor(items, "R1", null, now).map((item) => item.clientRef), ["a", "d"]);
    assert.deepEqual(chat.outboxFor(items, "R1", 7, now).map((item) => item.clientRef), ["b"]);
    assert.equal(chat.outboxFor(items, "R1", null, now)[1].status, "failed", "its files are about to go");
    assert.equal(chat.outboxFor(items, "R1", null, now)[1].expired, true);
    assert.deepEqual([0, 408, 429, 500, 503].map(chat.classify), ["retry", "retry", "retry", "retry", "retry"]);
    assert.deepEqual([401, 403].map(chat.classify), ["hold", "hold"]);
    assert.deepEqual([400, 404, 200].map(chat.classify), ["fail", "fail", "fail"]);
    assert.deepEqual([1, 2, 3, 4, 9].map(chat.backoff), [2000, 5000, 15000, 60000, 60000]);
});

test("a send goes through the outbox: answered now, waiting to try again, held, failed, offline, and dropped by its push", async () => {
    const stored = new Map();
    // Chrome will not replace localStorage. These three methods are the ones the outbox calls, and they are restored afterwards.
    const realGet = localStorage.getItem.bind(localStorage);
    const realSet = localStorage.setItem.bind(localStorage);
    const realRemove = localStorage.removeItem.bind(localStorage);
    localStorage.getItem = (key) => (stored.has(key) ? stored.get(key) : null);
    localStorage.setItem = (key, value) => stored.set(key, String(value));
    localStorage.removeItem = (key) => stored.delete(key);
    const original = moqui.request;
    const answers = [];
    const requests = [];
    moqui.request = (url, options) => {
        requests.push({ url, params: options.params });
        const answer = answers.shift() || { json: { eventSeq: 9, event: { eventSeq: 9 } } };
        return answer.status != null ? Promise.reject(answer) : Promise.resolve(answer);
    };
    const sent = [];
    const off = chat.on("sent", (payload) => sent.push(payload.clientRef));
    chat.state = freshState();
    chat.me = "ann";
    chat.started = true;
    const item = (clientRef, more) => Object.assign({ clientRef, chatRoomId: "R1", url: "/apps/chat/send", params: { body: "<p>hi</p>" }, text: "hi" }, more);
    try {
        // answered now: gone from the outbox, and told
        const now = await chat.send(item("c1"));
        assert.equal(now.sent, true);
        assert.equal(requests[0].params.clientRef, "c1");
        assert.equal(requests[0].params.chatRoomId, "R1");
        assert.deepEqual(sent, ["c1"]);
        assert.equal(stored.has("moqui.chat.outbox.ann"), false);
        // no answer: it waits, kept in the page's storage, and is not due at once
        answers.push({ status: 0 });
        const waiting = await chat.send(item("c2"));
        assert.equal(waiting.queued, true);
        const kept = JSON.parse(stored.get("moqui.chat.outbox.ann"));
        assert.equal(kept[0].clientRef, "c2");
        assert.equal(kept[0].attempts, 1);
        assert.ok(kept[0].nextDate > Date.now() + 1000, "the next try in 2 seconds");
        assert.equal(chat.pendingIn("R1", null)[0].status, "waiting");
        await chat.flush(false);
        assert.equal(requests.length, 2, "not due yet");
        await chat.flush(true);
        assert.equal(requests.length, 3);
        assert.deepEqual(chat.state.outbox, []);
        // signed out: held until the next flush that sends all
        answers.push({ status: 403 });
        await chat.send(item("c3"));
        assert.equal(chat.state.outbox[0].status, "held");
        await chat.flush(false);
        assert.equal(requests.length, 4);
        answers.push({ status: 400, json: { errors: ["Join the room to post in it"] } });
        await chat.flush(true);
        assert.equal(chat.state.outbox[0].status, "failed");
        assert.equal(chat.state.outbox[0].error, "Join the room to post in it");
        await chat.flush(true);
        assert.equal(requests.length, 5, "a failed one waits for Retry");
        await chat.retry("c3");
        assert.deepEqual(chat.state.outbox, []);
        // offline: kept, not asked; back online it goes; a thread's item is the thread's
        chat.state.online = false;
        const offline = await chat.send(item("c4", { threadRootEventSeq: 7 }));
        assert.equal(offline.queued, true);
        assert.equal(requests.length, 6);
        assert.equal(chat.pendingIn("R1", 7).length, 1);
        assert.equal(chat.pendingIn("R1", null).length, 0);
        await chat.flush(true);
        assert.equal(requests.length, 6, "not while offline");
        // its push came first (another tab sent it): it waits no more
        chat.receive(push("chatRoom:R1", { kind: "event", chatRoomId: "R1", event: event({ eventSeq: 11, senderUserId: "ann", clientRef: "c4" }) }));
        assert.deepEqual(chat.state.outbox, []);
        chat.state.online = true;
        // too large for the outbox: refused
        await assert.rejects(chat.send(item("c5", { params: { body: "x".repeat(70 * 1024) } })), (err) => err.outbox === true);
    } finally {
        off();
        moqui.request = original;
        chat.started = false;
        localStorage.getItem = realGet;
        localStorage.setItem = realSet;
        localStorage.removeItem = realRemove;
    }
});

test("an agent's room card: any member decides an approval, only the requester answers or opens the run", () => {
    const approval = event({ eventSeq: 9, senderUserId: "_AGENT_ASSIST_", assist: { kind: "approval", runId: "R9", conversationId: "C9", itemId: "A1",
        requesterUserId: "ann", requesterName: "Ann", title: "Approval needed", summary: "Status: Approved → Held",
        rows: [{ field: "statusId", label: "Status", current: "Approved", proposed: "Held" }], state: "pending" } });
    const forBob = chat.assistCard(approval, "bob");
    assert.equal(forBob.kind, "approval");
    assert.ok(forBob.canDecide, "another member may decide (answered question 12)");
    assert.ok(!forBob.canViewRun, "the run is the requester's");
    assert.equal(forBob.askedBy, "Asked by Ann");
    assert.deepEqual(forBob.rows, [{ label: "Status", current: "Approved", proposed: "Held" }]);
    assert.ok(chat.assistCard(approval, "ann").canViewRun);
    const decided = event({ assist: Object.assign({}, approval.assist, { state: "executed", decidedBy: { userId: "bob", name: "Bob" } }) });
    const shown = chat.assistCard(decided, "ann");
    assert.ok(!shown.canDecide && !shown.pending);
    assert.equal(shown.decided, "Done by Bob");
    const question = event({ assist: { kind: "question", itemId: "Q1", requesterUserId: "ann", requesterName: "Ann", summary: "Which one?",
        options: [{ label: "First", recommended: true }, { label: "Second" }], state: "pending" } });
    assert.deepEqual(chat.assistCard(question, "ann").options, ["First", "Second"]);
    assert.ok(chat.assistCard(question, "ann").canAnswer);
    assert.ok(!chat.assistCard(question, "bob").canAnswer);
    assert.equal(chat.assistCard(question, "bob").waiting, "Waiting for Ann");
    assert.equal(chat.assistCard(event({}), "ann"), null, "a message with no card");
    const reply = chat.assistCard(event({ assist: { kind: "reply", runId: "R9", conversationId: "C9", requesterUserId: "ann" } }), "ann");
    assert.equal(reply.state, "done");
    assert.ok(reply.canViewRun);
});

test("a card's button posts to chatAssistCard; an agent's typing carries its status text", async () => {
    const original = moqui.request;
    const sent = [];
    moqui.request = (url, options) => { sent.push({ url, options }); return Promise.resolve({ json: { status: "approved" } }); };
    const saved = chat.vm;
    chat.vm = { appRootPath: "/qapps" };
    const answer = await chat.decideCard("R1", 9, { decision: "approve" });
    assert.deepEqual(answer, { status: "approved" });
    assert.equal(sent[0].url, "/qapps/chatAssistCard");
    assert.deepEqual(sent[0].options, { method: "POST", params: { chatRoomId: "R1", eventSeq: 9, decision: "approve" } });
    moqui.request = () => Promise.reject({ status: 409, json: { errors: "this approval was decided already" } });
    assert.deepEqual(await chat.decideCard("R1", 9, { decision: "reject" }), { error: "this approval was decided already" });
    moqui.request = original;
    chat.vm = saved;
    const state = freshState();
    const typed = chat.reduce(state, push("chatRoom:R1", { kind: "typing", chatRoomId: "R1", userId: "_AGENT_ASSIST_", typing: true,
        statusText: "Searching…" }), "ann", 1000);
    assert.equal(typed.statusText, "Searching…");
    assert.equal(state.typingText.R1._AGENT_ASSIST_, "Searching…");
    chat.reduce(state, push("chatRoom:R1", { kind: "typing", chatRoomId: "R1", userId: "_AGENT_ASSIST_", typing: false }), "ann", 1100);
    assert.equal(state.typingText.R1._AGENT_ASSIST_, undefined);
});

test("an encrypted message's edit keeps its files: their names, types and keys go in the edit's ciphertext, and read back from it", async () => {
    await chat.loadPurify();
    const encryptRoom = moqui.crypto.encryptRoom;
    let sealedContent = null;
    moqui.crypto.encryptRoom = (roomId, memberIds, type, content) => { sealedContent = content; return Promise.resolve({ algorithm: "m.megolm.v1.aes-sha2" }); };
    try {
        // the message as the timeline has it once decrypted: a voice note with its key (sealed), and a file never decrypted
        const attachments = [{ chatAttachmentId: "A1", attachmentSeq: 1, fileName: "Voice note.webm", contentType: "audio/webm", fileSize: 900, sealed: { key: "k1" } },
            { chatAttachmentId: "A2", attachmentSeq: 2, fileName: null, contentType: null, fileSize: 5, sealed: null }];
        assert.deepEqual(chat.keptFiles(attachments).map((one) => one.chatAttachmentId), ["A1"], "only what this device can read is kept");
        await chat.sealMessage("R1", ["ann"], "<p>Now with text</p>", chat.keptFiles(attachments));
        assert.deepEqual(sealedContent["io.moqui.files"], [{ chatAttachmentId: "A1", name: "Voice note.webm", type: "audio/webm", size: 900, file: { key: "k1" } }]);
        // read back as the timeline reads an edited message: the server's attachments (no names) and the edit's content
        const item = { attachments: [{ chatAttachmentId: "A1", attachmentSeq: 1, fileName: null, contentType: null, fileSize: 900 }] };
        await chat.applyOpened(item, { content: sealedContent });
        assert.deepEqual([item.attachments[0].fileName, item.attachments[0].contentType, item.attachments[0].sealed.key], ["Voice note.webm", "audio/webm", "k1"]);
    } finally {
        moqui.crypto.encryptRoom = encryptRoom;
    }
});

test("an encrypted room's decrypted HTML keeps what a plain room's message may have, and a decrypted file opens as an image or bytes", async () => {
    await chat.loadPurify();
    const kept = '<p>Hi <span data-mention-id="bob">@Bob</span> <a href="https://example.com/x" hreflang="en">link</a> <b>bold</b></p>'
        + '<table><tbody><tr><td colspan="2">cell</td></tr></tbody></table><img src="/richTextImage/100123" alt="a" width="10">';
    assert.equal(chat.clean(kept), kept, "the markup a plain room keeps stays");
    const cleaned = chat.clean('<p class="fixed-full z-max" id="moqui" style="color:red" onclick="alert(1)">x</p>'
        + '<a href="javascript:alert(1)" target="_blank">y</a><svg><circle onload="alert(1)"></circle></svg><script>alert(1)</script>'
        + '<img src="x" onerror="alert(1)"><iframe src="https://example.com"></iframe><form><input name="a"></form>'
        + '<video src="https://example.com/v.mp4"></video><span data-other="1" aria-label="z">z</span>');
    for (const bad of ["class", "id=", "style", "onclick", "javascript:", "target", "<svg", "<circle", "<script", "onerror", "<iframe", "<form",
        "<input", "<video", "data-other", "aria-label"]) {
        assert.ok(cleaned.indexOf(bad) < 0, "no " + bad + " in " + cleaned);
    }
    assert.ok(cleaned.indexOf("<p>x</p>") >= 0, cleaned);
    // an image only of this server (SEC-COM-02): another host's, or another path of this one, loses its src, in an encrypted
    // room's HTML and in a plain room's stored one
    for (const src of ["https://host.example/x.png?u=1", "//host.example/x.png", "/apps/report/export", "/richTextImage/1?x=2"]) {
        assert.ok(chat.clean('<p>a<img src="' + src + '" alt="b"></p>').indexOf("src=") < 0, src);
        assert.ok(chat.ownImages('<p>a<img src="' + src + '" alt="b"></p>').indexOf("src=") < 0, src);
    }
    assert.equal(chat.ownImages('<p><img src="/richTextImage/9"></p>'), '<p><img src="/richTextImage/9"></p>');
    assert.equal(chat.blobType("image/PNG"), "image/png");
    assert.equal(chat.blobType("image/jpeg"), "image/jpeg");
    // a voice note plays from its blob: a sound or video keeps its type, without its codecs
    assert.equal(chat.blobType("audio/webm;codecs=opus"), "audio/webm");
    assert.equal(chat.blobType("video/mp4"), "video/mp4");
    for (const type of ["text/html", "image/svg+xml", "application/xhtml+xml", "application/pdf", "audio/x-unknown", "", null]) {
        assert.equal(chat.blobType(type), "application/octet-stream", String(type));
    }
});

test("an encrypted room's decrypted files are let go when the room closes: their blob URLs revoked, decrypted again when opened", async () => {
    const realFetch = window.fetch;
    const realDecrypt = window.moqui.crypto;
    const revoked = [];
    const realRevoke = URL.revokeObjectURL;
    let decrypted = 0;
    window.fetch = () => Promise.resolve(new Response(new Uint8Array([1, 2, 3])));
    window.moqui.crypto = { decryptFile: () => { decrypted++; return Promise.resolve(new Uint8Array([4, 5])); } };
    URL.revokeObjectURL = (url) => { revoked.push(url); realRevoke.call(URL, url); };
    try {
        chat.openRoom("R1");
        chat.openRoom("R2");
        const first = await chat.openFile("/a", { chatAttachmentId: "F1", contentType: "image/png", sealed: {} }, "R1");
        const again = await chat.openFile("/a", { chatAttachmentId: "F1", contentType: "image/png", sealed: {} }, "R1");
        const other = await chat.openFile("/b", { chatAttachmentId: "F2", contentType: "image/png", sealed: {} }, "R2");
        assert.equal(again, first, "opened once while the room is open");
        assert.equal(decrypted, 2);
        chat.closeRoom("R1");
        await new Promise((resolve) => setTimeout(resolve, 0));
        assert.deepEqual(revoked, [first], "the closed room's file only");
        assert.ok(!chat.fileUrls.F1 && !!chat.fileUrls.F2);
        const reopened = await chat.openFile("/a", { chatAttachmentId: "F1", contentType: "image/png", sealed: {} }, "R1");
        assert.ok(reopened !== first && decrypted === 3, "decrypted again");
        chat.releaseFiles("R2", ["F2"]);
        await new Promise((resolve) => setTimeout(resolve, 0));
        assert.ok(revoked.includes(other), "a message that left the page lets its file go");
        chat.closeRoom("R2");
        chat.releaseFiles("R1");
    } finally {
        window.fetch = realFetch;
        window.moqui.crypto = realDecrypt;
        URL.revokeObjectURL = realRevoke;
    }
});

test("the outbox is kept under the storage scope of a desktop client's account", () => {
    const savedMe = chat.me;
    const savedScope = moqui.storageScope;
    try {
        chat.me = "U1";
        moqui.storageScope = "";
        assert.equal(chat.outboxKey(), "moqui.chat.outbox.U1");
        moqui.storageScope = "erp.example.com-0123abcd";
        assert.equal(chat.outboxKey(), "erp.example.com-0123abcd:moqui.chat.outbox.U1");
    } finally {
        chat.me = savedMe;
        moqui.storageScope = savedScope;
    }
});

// A host outbox as the desktop client's bridge gives it (moquiDesktop.outbox), kept in memory.
const fakeHostOutbox = () => {
    const host = { ops: [], listeners: [], calls: [], failAdd: false };
    Object.assign(host, {
        add(kind, payload, files) {
            host.calls.push(["add", kind]);
            if (host.failAdd) return Promise.reject(new Error(host.failAdd === true ? "The outbox is full" : host.failAdd));
            const id = payload.clientRef || "h" + host.ops.length;
            const kept = host.ops.find((op) => op.id === id);
            if (kept) return Promise.resolve(kept);
            const clean = Object.assign({}, payload);
            ["status", "attempts", "nextDate", "error"].forEach((name) => delete clean[name]);
            const op = { id, kind, target: payload.chatRoomId || "", state: "queued", attempts: 0, nextMs: 0, createdMs: payload.createdDate || Date.now(),
                error: null, payload: clean, files: files || [] };
            host.ops.push(op);
            return Promise.resolve(op);
        },
        list() { return Promise.resolve(host.ops.map((op) => Object.assign({}, op))); },
        retry(id) { host.calls.push(["retry", id]); host.ops.filter((op) => op.id === id).forEach((op) => { op.state = "retry"; op.error = null; }); return Promise.resolve(true); },
        discard(id) { host.calls.push(["discard", id]); host.ops = host.ops.filter((op) => op.id !== id); return Promise.resolve(true); },
        on(fn) { host.listeners.push(fn); return () => {}; },
        tell(event) { host.listeners.forEach((fn) => fn(event)); }
    });
    return host;
};

test("with the desktop client's host the outbox is the host's: imported once, sent by it, shown here", async () => {
    const stored = new Map();
    const realGet = localStorage.getItem.bind(localStorage);
    const realSet = localStorage.setItem.bind(localStorage);
    const realRemove = localStorage.removeItem.bind(localStorage);
    localStorage.getItem = (key) => (stored.has(key) ? stored.get(key) : null);
    localStorage.setItem = (key, value) => stored.set(key, String(value));
    localStorage.removeItem = (key) => stored.delete(key);
    const original = moqui.request;
    const requests = [];
    moqui.request = (url, options) => { requests.push(url); return Promise.resolve({ json: { eventSeq: 20, event: { eventSeq: 20 } } }); };
    const savedScope = moqui.storageScope;
    const host = fakeHostOutbox();
    const sent = [];
    const off = chat.on("sent", (payload) => sent.push(payload));
    const older = [
        { clientRef: "c1", chatRoomId: "R1", url: "/apps/chat/send", params: { body: "<p>one</p>" }, text: "one", createdDate: 1000, attempts: 3, status: "failed" },
        { clientRef: "c2", chatRoomId: "R1", url: "/apps/chat/send", params: { encryptedJson: "{\"ct\":\"x\"}" }, text: "", sealed: true, createdDate: 2000,
            status: "waiting" }
    ];
    try {
        moqui.storageScope = null;
        globalThis.moquiDesktop = { storageScope: "acct1", outbox: host };
        chat.state = freshState();
        chat.me = "ann";
        chat.started = true;
        stored.set("acct1:moqui.chat.outbox.ann", JSON.stringify(older));
        await chat.startHost(host);
        // the browser's outbox moved into the host, with its refs, dates and ciphertext, and removed here
        assert.deepEqual(host.ops.map((op) => op.id), ["c1", "c2"]);
        assert.equal(stored.has("acct1:moqui.chat.outbox.ann"), false);
        assert.deepEqual(chat.state.outbox.map((item) => [item.clientRef, item.status, item.createdDate]), [["c1", "waiting", 1000], ["c2", "waiting", 2000]]);
        assert.equal(chat.state.outbox[1].sealed, true);
        assert.equal(chat.state.outbox[1].params.encryptedJson, "{\"ct\":\"x\"}");
        // again (a crash before the list was removed): taken once
        stored.set("acct1:moqui.chat.outbox.ann", JSON.stringify(older));
        assert.equal(await chat.importOutbox(host), 2);
        assert.equal(host.ops.length, 2);
        assert.equal(stored.has("acct1:moqui.chat.outbox.ann"), false);
        // one the host cannot take now (its outbox full): the list stays to try again
        stored.set("acct1:moqui.chat.outbox.ann", JSON.stringify(older));
        host.failAdd = true;
        await chat.importOutbox(host);
        assert.equal(JSON.parse(stored.get("acct1:moqui.chat.outbox.ann")).length, 2);
        // one refused for good is dropped, and each taken comes off the list on its own
        const refused = { clientRef: "bad", chatRoomId: "R1", url: "https://elsewhere.example.com/", params: {} };
        stored.set("acct1:moqui.chat.outbox.ann", JSON.stringify([refused].concat(older)));
        host.failAdd = false;
        const realAdd = host.add;
        host.add = (kind, payload, files) => (payload.clientRef === "bad" ? Promise.reject(new Error("Not the comms app's send")) : realAdd(kind, payload, files));
        const warn = console.warn;
        const warned = [];
        console.warn = (...args) => warned.push(String(args[0]));
        try { await chat.importOutbox(host); } finally { console.warn = warn; host.add = realAdd; }
        assert.equal(warned.length, 1, "the one dropped is told");
        assert.equal(stored.has("acct1:moqui.chat.outbox.ann"), false);
        assert.equal(host.ops.some((op) => op.id === "bad"), false);

        // a send is kept by the host, which sends it: the page asks nothing itself
        const outcome = await chat.send({ clientRef: "c3", chatRoomId: "R1", url: "/apps/chat/send", params: { body: "<p>three</p>" }, text: "three" });
        assert.deepEqual(outcome, { queued: true, clientRef: "c3" });
        assert.equal(host.ops[2].kind, "chat-send");
        assert.equal(chat.pendingIn("R1", null).length, 3);
        await chat.flush(true);
        assert.deepEqual(requests, [], "the page's own flush is off");
        // the host's answer comes as the page's sent; its echo takes it off here too
        host.tell({ kind: "sent", op: "chat-send", clientRef: "c3", chatRoomId: "R1", threadRootEventSeq: null, json: { eventSeq: 9, event: { eventSeq: 9 } } });
        assert.equal(sent[0].clientRef, "c3");
        assert.equal(sent[0].json.eventSeq, 9);
        chat.receive(push("chatRoom:R1", { kind: "event", chatRoomId: "R1", event: event({ eventSeq: 9, senderUserId: "ann", clientRef: "c3" }) }));
        assert.equal(chat.state.outbox.some((item) => item.clientRef === "c3"), false);
        // a refusal shows with its message; retry and discard are the host's
        host.ops[0].state = "failed";
        host.ops[0].error = "Join the room to post in it";
        await chat.refreshHost();
        assert.equal(chat.pendingIn("R1", null)[0].status, "failed");
        assert.equal(chat.pendingIn("R1", null)[0].error, "Join the room to post in it");
        await chat.retry("c1");
        assert.equal(chat.pendingIn("R1", null)[0].status, "waiting");
        await chat.discard("c2");
        assert.equal(host.ops.some((op) => op.id === "c2"), false);
        assert.equal(chat.state.outbox.some((item) => item.clientRef === "c2"), false);
        // a mark-read waits in the host's outbox with its absolute seq
        assert.deepEqual(await chat.queueRead("R1", 12), { lastReadEventSeq: 12 });
        const read = host.ops.find((op) => op.kind === "chat-read");
        assert.deepEqual(read.payload, { chatRoomId: "R1", lastReadEventSeq: 12 });
        assert.equal(chat.queueRead("R1", null), null, "no seq: the screen asks the server");
        // one the host will not keep for another reason goes now while online, as on the web
        host.failAdd = "Not the comms app's send";
        const direct = await chat.send({ clientRef: "c5", chatRoomId: "R1", url: "/qapps/x/Chat/send", params: { body: "five" }, text: "five" });
        assert.equal(direct.sent, true);
        assert.deepEqual(requests, ["/qapps/x/Chat/send"]);
        requests.length = 0;
        // its outbox full: refused, and not shown
        host.failAdd = true;
        await assert.rejects(chat.send({ clientRef: "c4", chatRoomId: "R1", url: "/apps/chat/send", params: {}, text: "four" }), (err) => err.outbox === true);
        assert.equal(chat.state.outbox.some((item) => item.clientRef === "c4"), false);
        assert.deepEqual(requests, []);
        // on the web: no host, the screen asks the server itself
        delete globalThis.moquiDesktop;
        assert.equal(chat.queueRead("R1", 12), null);
        assert.equal(chat.host(), null);
    } finally {
        off();
        delete globalThis.moquiDesktop;
        moqui.storageScope = savedScope;
        moqui.request = original;
        chat.started = false;
        chat.state = freshState();
        localStorage.getItem = realGet;
        localStorage.setItem = realSet;
        localStorage.removeItem = realRemove;
    }
});

test("an operation of the host's outbox as an item of the page's", () => {
    const item = chat.itemOfOp({ id: "c1", kind: "chat-send", state: "retry", attempts: 2, nextMs: 50, createdMs: 10, error: null,
        payload: { chatRoomId: "R1", text: "hi", threadRootEventSeq: 4 } });
    assert.deepEqual(item, { chatRoomId: "R1", text: "hi", threadRootEventSeq: 4, clientRef: "c1", createdDate: 10, attempts: 2, nextDate: 50,
        status: "waiting", error: null });
    assert.deepEqual(["queued", "sending", "held", "failed", "acked"].map((state) => chat.itemOfOp({ id: "x", state, payload: {} }).status),
        ["waiting", "sending", "held", "failed", "waiting"]);
});

test("the shared list helpers: times in the user's zone by calendar day, the room's icon, counts, archive and snippet", () => {
    const l10n = moqui.l10n, priorZone = l10n.timeZone, priorLocale = l10n.locale, priorNow = Date.now;
    const plain = (text) => String(text).replace(/[  ]/g, " ");
    try {
        l10n.setLocale("en_US");
        l10n.setTimeZone("America/Los_Angeles");
        Date.now = () => Date.parse("2026-11-09T08:05:00Z");
        const late = Date.parse("2026-11-09T07:55:00Z"), early = Date.parse("2026-11-09T08:01:00Z");
        // the user's day, not the browser's: 11:55 PM is yesterday in Los Angeles
        assert.equal(chat.dayLabel(late), "Yesterday");
        assert.equal(chat.dayLabel(early), "Today");
        assert.equal(chat.shortWhen(late), "Sun");
        assert.equal(plain(chat.shortWhen(early)), "12:01 AM");
        assert.equal(chat.shortWhen(Date.parse("2026-10-01T18:00:00Z")), "Oct 1");
        assert.equal(chat.shortWhen(Date.parse("2025-10-01T18:00:00Z")), "Oct 1, 2025", "another year says so");
        assert.ok(plain(chat.dateTime(early)).includes("Nov 9, 2026") && plain(chat.dateTime(early)).includes("12:01 AM"), chat.dateTime(early));
        assert.equal(chat.shortWhen(null), "");
        l10n.setTimeZone("Europe/Berlin");
        assert.equal(plain(chat.shortWhen(early)), "9:01 AM", "the same instant in the zone set");
    } finally {
        Date.now = priorNow;
        l10n.setLocale(priorLocale);
        l10n.setTimeZone(priorZone);
    }
    assert.equal(chat.roomIcon({ roomTypeEnumId: "CrtExternal" }), "sym_o_sms", "a phone number's room has its icon");
    assert.equal(chat.roomIcon({ roomTypeEnumId: "CrtRecord", record: { icon: "sym_o_work" } }), "sym_o_work");
    assert.equal(chat.roomIcon(null), "sym_o_chat");
    const last = { eventSeq: 7, senderUserId: "bob", senderName: "Bob Smith", snippet: "hello" };
    assert.ok(chat.isArchived({ isMember: true, archivedEventSeq: 7, lastMessage: last }));
    assert.ok(!chat.isArchived({ isMember: true, archivedEventSeq: 6, lastMessage: last }), "a newer message brings it back");
    assert.equal(chat.roomSnippet({ isMember: true, roomTypeEnumId: "CrtPrivate", lastMessage: last }, "ann"), "Bob: hello");
    assert.equal(chat.roomSnippet({ isMember: true, roomTypeEnumId: "CrtDirect", otherUserId: "bob", lastMessage: last }, "ann"), "hello");
    assert.equal(chat.roomSnippet({ isMember: true, roomTypeEnumId: "CrtDirect", otherUserId: "bob", lastMessage: Object.assign({}, last, { senderUserId: "ann" }) }, "ann"), "You: hello");
    assert.equal(chat.roomSnippet({ isMember: false }, "ann"), "Public room: not joined");
    assert.deepEqual(chat.countsOf({ isMember: false, unreadCount: 3 }), { unread: 0, mentions: 0 });
    assert.deepEqual(chat.countsOf({ isMember: true, chatRoomId: "NONE", unreadCount: 3, mentionCount: 1 }), { unread: 3, mentions: 1 });
});

test("the new-chat pickers: one person is a direct chat, several a group (with a name, a private room), a phone number its room, and a number with others is refused", async () => {
    assert.equal(chat.pickKind(null), "none");
    assert.equal(chat.pickKind(["bob"]), "direct");
    assert.equal(chat.pickKind(["bob", "cy"]), "group");
    assert.equal(chat.pickKind(["tel:+15552224000"]), "number");
    assert.equal(chat.pickKind(["tel:+15552224000", "bob"]), "mixed");
    // several numbers: a group text (plans/Comms-Messaging.md §9)
    assert.equal(chat.pickKind(["tel:+15552224000", "tel:+15552225000"]), "numbers");
    const posts = [];
    const post = (name, params) => { posts.push({ name, params }); return Promise.resolve({ chatRoomId: "R" + posts.length }); };
    assert.equal((await chat.startChat(post, { values: ["bob"] })).chatRoomId, "R1");
    await chat.startChat(post, { values: ["bob", "cy"], name: "  " });
    await chat.startChat(post, { values: ["bob", "cy"], name: " Ops " });
    await chat.startChat(post, { values: ["tel:(555) 222-4000"], fromNumber: "+15550004000" });
    await chat.startChat(post, { values: ["tel:5552224000"] });
    assert.deepEqual(posts, [
        { name: "createDirect", params: { userIds: "bob" } },
        { name: "createDirect", params: { userIds: "bob,cy" } },
        // no encrypted: the server decides, encrypted when everyone in it has a crypto device (Comms-Messaging §6.5)
        { name: "createRoom", params: { roomTypeEnumId: "private", roomName: "Ops", memberUserIds: "bob,cy" } },
        { name: "textRoom", params: { textTo: "(555) 222-4000", textFrom: "+15550004000" } },
        { name: "textRoom", params: { textTo: "5552224000", textFrom: "" } }]);
    // a number with someone else, or nothing picked, posts nothing
    await assert.rejects(chat.startChat(post, { values: ["tel:5552224000", "bob"] }), (err) => err.refused === "mixed");
    await assert.rejects(chat.startChat(post, { values: [] }), (err) => err.refused === "none");
    assert.equal(posts.length, 5);
    // several numbers: one group text's room
    await chat.startChat(post, { values: ["tel:5552224000", "tel:+15552225000"], fromNumber: "+15550004000" });
    assert.deepEqual(posts[5], { name: "textRoom", params: { textTo: "5552224000,+15552225000", textFrom: "+15550004000" } });
    assert.deepEqual(chat.numberOptions([{ phoneNumber: "+15550004000", description: "Office" }, { phoneNumber: "+15550004001" }]),
        [{ value: "+15550004000", label: "+15550004000 · Office" }, { value: "+15550004001", label: "+15550004001" }]);
});

// plans/Comms-Messaging.md §9, Phone book: a phone room's numbers by their names, the phone's contacts first, then the
// apps' call-context; the pickers' extra options from the phone's contacts
test("a phone room is named by its numbers' names, the phone's contacts first; the picker offers the phone's contacts", async () => {
    const host = window.moquiDesktop;
    const request = moqui.request;
    const asked = [];
    window.moquiDesktop = {
        contactNames: (numbers) => { asked.push(["phone", numbers]); return Promise.resolve({ "+15552224000": "Ann Phone" }); },
        contactsSearch: (term) => Promise.resolve(term === "ann" ? [{ name: "Ann Phone", number: "+15552224000" }, { name: "No number" }] : [])
    };
    moqui.request = (url, opts) => {
        asked.push(["server", url, opts.params.numbers]);
        return Promise.resolve({ json: { nameMap: { "+15552225000": "Bo Server", "+15552224000": "Ann Server" } } });
    };
    try {
        chat.phoneNames = {};
        chat.contactNamesOf = {};
        chat.serverNamesOf = {};
        const rooms = [
            { chatRoomId: "P1", roomTypeEnumId: "CrtExternal", roomName: "+15552224000", displayName: "+15552224000" },
            { chatRoomId: "P2", roomTypeEnumId: "CrtExternal", roomName: "+15552224000, +15552225000, +15552226000", displayName: "x" },
            { chatRoomId: "D1", roomTypeEnumId: "CrtDirect", roomName: "+15559990000", displayName: "Bob" },
            { chatRoomId: "B1", roomTypeEnumId: "CrtExternal", roomName: "Team on Slack", displayName: "Team on Slack" }
        ];
        await chat.nameRooms(rooms);
        assert.equal(rooms[0].displayName, "Ann Phone", "the phone's contact before the server's name");
        assert.equal(rooms[1].displayName, "Ann Phone, Bo Server, +15552226000", "a group text; a number no one knows stays");
        assert.equal(rooms[2].displayName, "Bob", "not a phone room");
        assert.equal(rooms[3].displayName, "Team on Slack", "a bridged room named otherwise");
        assert.deepEqual(asked[0], ["phone", ["+15552224000", "+15552225000", "+15552226000"]]);
        assert.deepEqual(asked[1].slice(2), ["+15552224000,+15552225000,+15552226000"],
            "the server is asked for every number, so it cannot tell which the phone's contacts have");
        const count = asked.length;
        await chat.nameRooms(rooms);
        assert.equal(asked.length, count, "never asked twice for a number");
        // the contacts allowed later: the phone is asked again for the rooms last named, no reload
        window.moquiDesktop.contactNames = (numbers) => { asked.push(["phone", numbers]); return Promise.resolve({ "+15552226000": "Cy Phone" }); };
        await chat.contactsChanged();
        assert.deepEqual(asked.slice(count).map((one) => one[0]), ["phone"], "only the phone asked again");
        assert.equal(rooms[1].displayName, "Ann Server, Bo Server, Cy Phone");
        assert.deepEqual(await chat.contactOptions("ann"), [{ value: "tel:+15552224000", label: "Ann Phone · +15552224000" }]);
        assert.deepEqual(await chat.contactOptions("a"), [], "two letters at least");
        delete window.moquiDesktop.contactsSearch;
        assert.deepEqual(await chat.contactOptions("ann"), [], "no phone: nothing");
    } finally {
        window.moquiDesktop = host;
        moqui.request = request;
        chat.phoneNames = {};
        chat.contactNamesOf = {};
        chat.serverNamesOf = {};
        chat.namedRooms = [];
    }
});

// plans/Comms-Messaging.md §7.5: the app's notification of a socket event follows the rules of the server's pushes
test("the app notifies a message by the room's level, not its own, not on do not disturb or while shown, and no text sealed", () => {
    const message = event({ eventSeq: 9, senderName: "Bob", bodyText: "Lunch\n at noon?", mentions: [] });
    const base = { me: "ann", levels: { R1: "all" }, appRoot: "/qapps/Comms" };
    assert.deepEqual(chat.notification(message, "R1", base),
        { title: "Bob", body: "Lunch at noon?", tag: "chat-R1", link: "/qapps/Comms/Chat?chatRoomId=R1&eventSeq=9" });
    assert.equal(chat.notification(event({ senderUserId: "ann" }), "R1", base), null, "my own message");
    assert.equal(chat.notification(message, "R1", Object.assign({}, base, { quiet: true })), null, "do not disturb");
    assert.equal(chat.notification(message, "R1", Object.assign({}, base, { seen: true })), null, "the room shows");
    assert.equal(chat.notification(message, "R1", Object.assign({}, base, { levels: { R1: "none" } })), null, "nothing");
    const mentionsOnly = Object.assign({}, base, { levels: { R1: "mentions" } });
    assert.equal(chat.notification(message, "R1", mentionsOnly), null, "mentions only, not mentioned");
    assert.ok(chat.notification(event({ mentions: ["ann"] }), "R1", mentionsOnly));
    // review gaps: a reply to me and a reply in my thread count as mentions, as the server's activity does
    assert.ok(chat.notification(event({ replyTo: { eventSeq: 3, senderUserId: "ann" } }), "R1", mentionsOnly), "a reply to me");
    assert.ok(chat.notification(event({ threadRootEventSeq: 3, threadRootSenderUserId: "ann" }), "R1", mentionsOnly), "in my thread");
    assert.equal(chat.notification(message, "R1", Object.assign({}, base, { preview: false })).body, null, "previews off");
    assert.equal(chat.notification(event({ encrypted: true, bodyText: "x" }), "R1", base).body, "New message", "encrypted: no text");
    assert.equal(chat.notification(event({ type: "reaction" }), "R1", base), null, "only messages");
    // a room joined since the levels were read: the user's default level
    assert.equal(chat.notification(message, "R9", Object.assign({}, base, { defaultLevel: "none" })), null);
    assert.ok(chat.notification(message, "R9", base), "no default: all");
    // a group room's name is the title, the sender before the text
    assert.deepEqual(chat.notification(message, "R1", Object.assign({}, base, { roomNames: { R1: "Team" } })),
        { title: "Team", body: "Bob: Lunch at noon?", tag: "chat-R1", link: "/qapps/Comms/Chat?chatRoomId=R1&eventSeq=9" });
    // a missed call to me and a voicemail notify unless the level is none (rule 6); a call I made, or missed by others, not
    const call = (content, fields) => event(Object.assign({ type: "call", content }, fields || {}));
    assert.equal(chat.notification(call({ action: "ended", missedUserIdList: ["ann"] }), "R1", mentionsOnly).body, "Missed call");
    assert.equal(chat.notification(call({ action: "voicemail" }), "R1", base).body, "Voicemail");
    assert.equal(chat.notification(call({ action: "ended", missedUserIdList: ["cy"] }), "R1", base), null);
    assert.equal(chat.notification(call({ action: "ended", missedUserIdList: ["ann"] }), "R1", Object.assign({}, base, { levels: { R1: "none" } })), null);
    // the room's settings: a muted room is mentions, none stays none
    assert.equal(chat.levelOf("CnlAll", true), "mentions");
    assert.equal(chat.levelOf("CnlNone", true), "none");
    assert.equal(chat.levelOf("mentions", false), "mentions");
    assert.equal(chat.levelOf(null, false), "all");
    // what was missed while closed: each room whose count went up since it was shown, the most first
    assert.deepEqual(chat.missed({ R1: { counted: 2 }, R2: { counted: 0 }, R3: { counted: 1 }, R4: { counted: 5 } }, { R3: 1, R4: 7 }),
        [{ chatRoomId: "R1", count: 2 }]);
    assert.deepEqual(chat.missed({ R1: { counted: 2 }, R3: { counted: 4 } }, { R3: 1 }),
        [{ chatRoomId: "R3", count: 4 }, { chatRoomId: "R1", count: 2 }]);
    // the user's settings change on another device: the room's level follows
    const state = Object.assign(freshState(), { levels: { R1: "all" } });
    chat.reduce(state, push("chatUser:ann", { kind: "room", action: "settings", chatRoomId: "R1", notificationLevelEnumId: "CnlAll", isMuted: true }), "ann", 1);
    assert.equal(state.levels.R1, "mentions");
    // the app's host: a socket message notifies with the room's tag, a read of the room clears it; a phone's host
    // (pushDelivers) leaves its notifications to its pushes; the catch-up then counts what was shown
    const calls = [];
    const before = { me: chat.me, host: window.moquiDesktop };
    chat.me = "ann";
    window.moquiDesktop = { storageScope: "acct1", notify: (shown) => { calls.push(["notify", shown.tag]); return Promise.resolve(); },
        notifyClear: (tag) => { calls.push(["clear", tag]); return Promise.resolve(); } };
    try {
        try { localStorage.removeItem(chat.shownKey(window.moquiDesktop)); } catch (err) { /* no storage */ }
        chat.notifyApp({ kind: "event", chatRoomId: "R7", event: event({ chatRoomId: "R7", senderName: "Bob", bodyText: "hi" }) });
        assert.equal(chat.readShown(window.moquiDesktop).R7, 1, "a live notification counts as shown");
        chat.notifyMissed({ R7: { counted: 1 } });
        assert.equal(calls.length, 1, "not shown again by the catch-up");
        chat.notifyApp({ kind: "read", chatRoomId: null, userId: "ann", all: true });
        assert.deepEqual(chat.readShown(window.moquiDesktop), {});
        // more than five rooms missed: one summary
        chat.notifyMissed({ A: { counted: 1 }, B: { counted: 2 }, C: { counted: 1 }, D: { counted: 1 }, E: { counted: 1 }, F: { counted: 3 } });
        window.moquiDesktop.pushDelivers = true;
        chat.notifyApp({ kind: "event", chatRoomId: "R8", event: event({ chatRoomId: "R8", bodyText: "hi" }) });
        // Mark all read: every room's notification of the account (chat-*), then the rooms this page showed
        assert.deepEqual(calls, [["notify", "chat-R7"], ["clear", "chat-*"], ["clear", "chat-R7"], ["notify", "chat-missed"]]);
    } finally {
        try { localStorage.removeItem(chat.shownKey(window.moquiDesktop)); } catch (err) { /* no storage */ }
        chat.me = before.me;
        window.moquiDesktop = before.host;
    }
});

test("the E2EE label (Comms-Messaging §6.2): a lock only for end to end, the open lock and its banner for the rest, the composer's hint for a text and a bridge", () => {
    const label = (room, trust) => chat.e2eeLabel(room, trust);
    // a private room's type icon is people, not a lock (COM-UI-001)
    assert.equal(chat.roomIcon({ roomTypeEnumId: "CrtPrivate" }), "sym_o_groups");
    const verified = label({ roomTypeEnumId: "CrtPrivate", encrypted: true }, { allVerified: true });
    assert.deepEqual([verified.state, verified.lock, verified.verified, verified.icon, verified.verify], ["verified", true, true, "sym_o_lock", false]);
    assert.ok(verified.banner.startsWith("Messages and calls are end-to-end encrypted"));
    const sealed = label({ roomTypeEnumId: "CrtDirect", encrypted: true }, { allVerified: false });
    assert.deepEqual([sealed.state, sealed.lock, sealed.verified, sealed.verify], ["encrypted", true, false, true], "not all verified: Verify");
    assert.equal(label({ encrypted: true }).state, "encrypted", "trust not known is not verified");
    for (const room of [{ roomTypeEnumId: "CrtPrivate", visibilityEnumId: "CrvMembers" }, { roomTypeEnumId: "CrtDirect" }]) {
        const plain = label(room, { allVerified: true });
        assert.deepEqual([plain.state, plain.lock, plain.icon, plain.hint], ["plain", false, "sym_o_lock_open", ""]);
        assert.equal(plain.banner, "Not end-to-end encrypted. The server can read these messages.");
    }
    const channel = label({ roomTypeEnumId: "CrtPublic", visibilityEnumId: "CrvUsers" });
    assert.deepEqual([channel.state, channel.lock, channel.banner], ["public", false, ""], "a public channel: the open lock, no banner");
    assert.ok(channel.text, "but a line of text");
    assert.equal(label({ roomTypeEnumId: "CrtRecord", visibilityEnumId: "CrvRecord" }).state, "public");
    // a text: its pair room, or a room bridged to SMS (the adapter's encryption None)
    for (const room of [{ roomTypeEnumId: "CrtExternal" }, { roomTypeEnumId: "CrtPrivate", bridges: [{ network: "sms", networkName: "SMS", encryption: "None" }] }]) {
        const text = label(room);
        assert.deepEqual([text.state, text.lock, text.hint], ["sms", false, "Text message"]);
        assert.equal(text.banner, "Not end-to-end encrypted. Texts go through the phone network and the SMS provider.");
    }
    // a bridge, as get#Room gives it and as find#Rooms does: the network's name in the composer
    const bridged = label({ roomTypeEnumId: "CrtPrivate", bridges: [{ network: "whatsapp", networkName: "WhatsApp", encryption: "BridgeDecrypted" }] });
    assert.deepEqual([bridged.state, bridged.lock, bridged.hint], ["bridged", false, "WhatsApp message"]);
    assert.equal(bridged.banner, "Not end-to-end encrypted on this server. The bridge reads messages to pass them on.");
    assert.equal(label({ roomTypeEnumId: "CrtPublic", bridgeNetworks: [{ network: "xmpp", networkName: "XMPP", encryption: "Transport" }] }).hint, "XMPP message");
    assert.equal(label({ roomTypeEnumId: "CrtPublic", bridged: true, bridgeNetworkNames: ["Matrix"] }).state, "bridged", "a cached row with names only");
    // calls and mail
    assert.deepEqual([label({ call: { e2ee: true } }).state, label({ call: { e2ee: true } }).lock], ["call-encrypted", true]);
    assert.deepEqual([label({ call: { phone: true, e2ee: true } }).state, label({ call: { phone: true } }).lock], ["call-phone", false]);
    assert.equal(label({ call: { phone: true } }).banner, "Phone calls are not end-to-end encrypted.");
    assert.equal(label({ call: {} }).lock, false);
    const mail = label({ mail: true });
    assert.deepEqual([mail.state, mail.lock, mail.icon, mail.text], ["mail", false, null, "Email is not end-to-end encrypted"], "mail never has a lock");
    assert.equal(label(null).state, "plain");
});

test("an encrypted room's trust: every other member's devices verified, else Verify; under only verified devices who cannot read, and why the reader cannot (COM-UI-013)", () => {
    const device = (deviceId, verified) => ({ deviceId, displayName: deviceId + " phone", verified });
    const all = chat.trustOf({ verifiedOnly: false, users: [{ userId: "ann", devices: [] }, { userId: "bob", devices: [device("B1", true)] }] }, "ann");
    assert.deepEqual([all.known, all.allVerified, all.leftOut.length], [true, true, 0], "my only device is this one");
    const some = chat.trustOf({ verifiedOnly: true, users: [{ userId: "ann", devices: [device("A2", false)] }, { userId: "bob", devices: [device("B1", true), device("B2", false)] }] }, "ann");
    assert.equal(some.allVerified, false);
    assert.deepEqual(some.leftOut.map((one) => one.userId + "/" + one.deviceId), ["ann/A2", "bob/B2"]);
    assert.equal(chat.trustOf({ verifiedOnly: false, users: [{ userId: "bob", devices: [device("B2", false)] }] }, "ann").leftOut.length, 0, "shared with all: no one left out");
    assert.equal(chat.trustOf({ users: [{ userId: "ann", devices: [] }, { userId: "bob", devices: [] }] }, "ann").allVerified, false, "a member with no known device is not verified");
    assert.equal(chat.trustOf(null, "ann").known, false);
    assert.equal(chat.leftOutText(some.leftOut, "ann", (id) => (id === "bob" ? "Bob Baker" : id)), "Not readable by Bob Baker, your other devices: their devices are not verified");
    assert.equal(chat.leftOutText([], "ann"), "");
    assert.ok(chat.undecryptableText("m.unverified", "Bob").startsWith("Bob has not verified this device"), "the sender withheld it from this device");
    assert.ok(chat.undecryptableText(null, "Bob").includes("Restore your key backup"), "else the key backup");
    // a new device not set up yet (Comms-Messaging §6.5): sent before it, Set up; the withheld text stays
    assert.equal(chat.undecryptableText(null, "Bob", true), "Sent before this device was set up. Set up this device to read it.");
    assert.ok(chat.undecryptableText("m.unverified", "Bob", true).startsWith("Bob has not verified this device"));
    // a withheld key's reason reaches the message
    const item = {};
    chat.applyOpened(item, { error: "unreadable", withheld: "m.unverified" });
    assert.deepEqual([item.undecryptable, item.withheld], [true, "m.unverified"]);
});

test("the E2EE label's edges: a members-only record room is plain, a text with another bridge says both, an inbound-only bridge names nothing in the composer", () => {
    const label = (room) => chat.e2eeLabel(room);
    assert.equal(label({ roomTypeEnumId: "CrtRecord", visibilityEnumId: "CrvMembers" }).state, "plain", "a record room kept to its members");
    assert.equal(label({ roomTypeEnumId: "CrtRecord" }).state, "public", "a record's room for those who see the record");
    const both = label({ roomTypeEnumId: "CrtPrivate", bridges: [{ network: "sms", networkName: "SMS", outbound: true }, { network: "whatsapp", networkName: "WhatsApp", outbound: true }] });
    assert.equal(both.state, "sms");
    assert.ok(both.banner.includes("SMS provider") && both.banner.includes("the bridge reads messages"), both.banner);
    assert.equal(both.hint, "Text message · WhatsApp message");
    const inbound = label({ roomTypeEnumId: "CrtPrivate", bridges: [{ network: "xmpp", networkName: "XMPP", outbound: false }] });
    assert.deepEqual([inbound.state, inbound.hint], ["bridged", ""], "nothing goes out through it");
    assert.equal(label({ roomTypeEnumId: "CrtPrivate", bridges: [{ network: "sms", networkName: "SMS", outbound: false }] }).hint, "", "a text room that sends nothing out");
    // a network its adapter says has no E2EE (GroupMe, Comms-Messaging phase 14), as get#RoomBridges gives it, says so
    const groupme = label({ roomTypeEnumId: "CrtPrivate", bridges: [{ chatBridgeId: "GM1", name: "GroupMe of Ann", network: "groupme",
        networkName: "GroupMe", statusId: "CbsConnected", connected: true, directionEnumId: "CrbBoth", outbound: true, encryption: "NoneByNetwork" }] });
    assert.deepEqual([groupme.state, groupme.lock, groupme.hint], ["bridged", false, "GroupMe message"]);
    assert.equal(groupme.banner, "Not end-to-end encrypted. GroupMe has no end-to-end encryption, and the bridge reads messages to pass them on.");
    // None alone (an adapter that said nothing, or none in this build) is the plain bridged label
    const unknown = label({ roomTypeEnumId: "CrtPrivate", bridges: [{ network: "telegram", networkName: "Telegram", outbound: true, encryption: "None" }] });
    assert.equal(unknown.banner, "Not end-to-end encrypted on this server. The bridge reads messages to pass them on.");
});

test("people's checks (§6.1 rule 3): a verified identity or every device verified; a new identity is not verified; Verify asks another person first", () => {
    const trust = chat.trustOf({ users: [{ userId: "ann", devices: [{ deviceId: "A2", verified: false }] },
        { userId: "bob", identity: { verified: true }, devices: [{ deviceId: "B1", verified: true }] },
        { userId: "cy", identity: { verified: false }, devices: [{ deviceId: "C1", verified: true }] },
        { userId: "dee", identity: { verified: true, changed: true }, devices: [{ deviceId: "D1", verified: false }] },
        { userId: "eve", devices: [] }] }, "ann");
    assert.deepEqual(["bob", "cy", "dee", "eve"].map((id) => trust.people[id].verified), [true, true, false, false]);
    assert.equal(trust.people.dee.changed, true);
    assert.equal(chat.verifyTarget(trust, "ann"), "dee", "the first other person with a device not verified");
    assert.equal(chat.verifyTarget(chat.trustOf({ users: [{ userId: "ann", devices: [{ deviceId: "A2", verified: false }] }] }, "ann"), "ann"), null, "only my own: My devices");
    assert.deepEqual(chat.problemsText([{ userId: "bob", deviceId: "B9", displayName: "Bob's tablet", kind: "newDevice" }, { userId: "dee", kind: "changedIdentity" }],
        (id) => ({ bob: "Bob", dee: "Dee" })[id]), ["Bob has a new device that is not verified: Bob's tablet", "Dee has a new identity: verify them again"]);
    assert.ok(chat.undecryptableText("m.unverified", "Bob").includes("Members, Verify"), "the route there is");
});

test("who could not read my message is kept per account in this browser: from its clientRef to its eventSeq, for a reload or another tab", () => {
    const priorMe = chat.me, priorScope = moqui.storageScope;
    try {
        chat.me = "ann";
        moqui.storageScope = "acct-1";
        localStorage.removeItem(chat.leftOutKey());
        chat.noteLeftOut("c-77", "R9", "Not readable by Bob: their devices are not verified");
        assert.deepEqual(chat.leftOutOf("R9"), {}, "not before it is sent");
        chat.emit("sent", { clientRef: "c-77", chatRoomId: "R9", json: { event: { eventSeq: 41 } } });
        assert.equal(chat.leftOutOf("R9")[41], "Not readable by Bob: their devices are not verified");
        assert.equal(chat.readLeftOut().refs["c-77"], undefined);
        // another account in the same browser keeps its own, and so do the banners
        moqui.storageScope = "acct-2";
        assert.deepEqual(chat.leftOutOf("R9"), {});
        chat.closeBanner("R9", "plain");
        assert.equal(chat.bannerClosed("R9", "plain"), true);
        moqui.storageScope = "acct-1";
        assert.equal(chat.bannerClosed("R9", "plain"), false, "a room id of another account is not this one's");
        assert.equal(chat.leftOutOf("R9")[41], "Not readable by Bob: their devices are not verified");
    } finally {
        localStorage.removeItem(chat.leftOutKey());
        chat.me = priorMe;
        moqui.storageScope = priorScope;
    }
});

test("Info (Comms-Messaging §4, COM-UI-008): Sent, a bridged message's delivery per network, Read by from read markers; receipts off on either side; a thread", () => {
    const members = [{ userId: "ann", name: "Ann", lastReadEventSeq: 3 }, { userId: "bob", name: "Bob", lastReadEventSeq: 9 },
        { userId: "cy", name: "Cy", lastReadEventSeq: 5 }, { userId: "dee", name: "Dee", lastReadEventSeq: 4 },
        // receipts off: the server blanks the marker for others
        { userId: "eve", name: "Eve", lastReadEventSeq: null }, { userId: "agent", name: "Helper", isAgent: true, lastReadEventSeq: 9 }];
    const message = event({ eventSeq: 5, sentDate: 1000, senderUserId: "ann" });
    const when = (millis) => "at " + millis;
    const texts = (lines) => lines.map((line) => line.text).join(" | ");
    // read up to here or later; not me, not the sender, not an agent, not one whose receipts are off
    const info = chat.messageInfo(message, members, { me: "ann" });
    assert.deepEqual(info.readBy.people.map((person) => person.userId), ["bob", "cy"]);
    assert.equal(info.delivered, null, "a message that went out on no bridge has no delivery line");
    assert.equal(info.sent, 1000);
    let lines = chat.messageInfoLines(info, when);
    assert.deepEqual(lines.map((line) => line.key), ["sent", "read", "note"]);
    assert.equal(lines[0].text, "at 1000");
    assert.equal(lines[1].text, "Bob, Cy");
    assert.ok(lines[2].text.includes("read receipts off are not listed") && lines[2].text.includes("devices is not known"));
    // one who joined after the message has a marker past it (a join reads to the latest) but did not read it
    const joined = members.concat([{ userId: "fay", name: "Fay", lastReadEventSeq: 9, joinedDate: 1500 }, { userId: "gus", name: "Gus", lastReadEventSeq: 9, joinedDate: 1000 }]);
    assert.deepEqual(chat.messageInfo(message, joined, { me: "ann" }).readBy.people.map((person) => person.userId), ["bob", "cy", "gus"]);
    // and N more
    const many = Array.from({ length: 11 }, (unused, n) => ({ userId: "u" + n, name: "User " + n, lastReadEventSeq: 6 }));
    const more = chat.messageInfo(message, many, { me: "ann", limit: 8 });
    assert.equal(more.readBy.people.length, 8);
    assert.equal(more.readBy.more, 3);
    assert.ok(chat.messageInfoLines(more, when)[1].text.endsWith("User 7 and 3 more"));
    // no one has read up to it: said so, never "not read"
    const none = chat.messageInfoLines(chat.messageInfo(event({ eventSeq: 99, senderUserId: "ann" }), members, { me: "ann" }), when);
    assert.equal(none[1].text, "No one yet, of those with read receipts on");
    // the viewer's receipts off: no names, and it says why
    const off = chat.messageInfo(message, members, { me: "ann", receipts: false });
    assert.deepEqual(off.readBy, { off: true });
    lines = chat.messageInfoLines(off, when);
    assert.equal(lines[1].text, "Your read receipts are off, so you do not see who has read it");
    assert.ok(!/not read\b/i.test(texts(lines)) && !texts(lines).includes("Bob"));
    // a bridged message: each network's delivery, from its delivery row
    const bridged = event({ eventSeq: 5, sentDate: 1000, senderUserId: "ann", delivery: { state: "failed", destinations: [
        { networkName: "SMS", state: "accepted", receipt: "delivered" }, { bridgeName: "Ops Slack", state: "failed", error: "Channel archived" },
        { networkName: "Telegram", state: "uncertain" }, { networkName: "Matrix", state: "accepted" }, { networkName: "XMPP", state: "pending" }] } });
    lines = chat.messageInfoLines(chat.messageInfo(bridged, members, { me: "ann" }), when).filter((line) => line.key === "delivered");
    assert.deepEqual(lines.map((line) => line.label + ": " + line.text), ["SMS: Delivered: the network's receipt says so", "Ops Slack: Not delivered: Channel archived",
        "Telegram: May not have arrived", "Matrix: Taken by the network", "XMPP: Sending"]);
    // two bridges of one network: each with its own name
    const twice = event({ eventSeq: 5, sentDate: 1000, senderUserId: "ann", delivery: { state: "accepted", destinations: [
        { networkName: "SMS", bridgeName: "Office line", state: "accepted" }, { networkName: "SMS", bridgeName: "Shop line", state: "pending" },
        { networkName: "Matrix", bridgeName: "Matrix bridge", state: "accepted" }] } });
    assert.deepEqual(chat.messageInfo(twice, members, { me: "ann" }).delivered.map((one) => one.name), ["SMS · Office line", "SMS · Shop line", "Matrix"]);
    // a group text (plans/Comms-Messaging.md §9): each recipient's receipt, one failed while the other got it
    const group = event({ eventSeq: 5, sentDate: 1000, senderUserId: "ann", delivery: { state: "accepted", destinations: [
        { networkName: "SMS", state: "accepted", receipt: "delivered", recipients: { "+15552220002": "failed", "+15552220001": "delivered" } }] } });
    lines = chat.messageInfoLines(chat.messageInfo(group, members, { me: "ann" }), when).filter((line) => line.key === "delivered" || line.key === "recipient");
    assert.deepEqual(lines.map((line) => line.label + ": " + line.text), ["SMS: Delivered: the network's receipt says so",
        "+15552220001: Delivered: the network's receipt says so", "+15552220002: Not delivered"]);
    // an RCS bridge's message (plans/Comms-Messaging.md §16): the way it went; "as SMS" on the bubble only for the fallback
    const rcs = (path) => event({ eventSeq: 5, sentDate: 1000, senderUserId: "ann", delivery: { state: "accepted", destinations: [
        { networkName: "RCS", state: "accepted", receipt: "delivered", path }] } });
    const sentAs = (path) => chat.messageInfoLines(chat.messageInfo(rcs(path), members, { me: "ann" }), when).filter((line) => line.key === "path").map((line) => line.text);
    assert.deepEqual(sentAs("rcs"), ["RCS"]);
    assert.deepEqual(sentAs("sms"), ["SMS: the phone did not take RCS"]);
    assert.deepEqual(sentAs("mms"), ["MMS: the phone did not take RCS"]);
    assert.deepEqual(sentAs(null), [], "not known yet, or an SMS bridge's");
    assert.deepEqual(["rcs", "sms", "mms", null].map((path) => chat.pathName({ path })), ["", "SMS", "MMS", ""]);
    // a thread: Sent and Delivered only, it has no read markers
    const reply = event({ eventSeq: 7, sentDate: 2000, senderUserId: "ann", threadRootEventSeq: 5, delivery: bridged.delivery });
    const thread = chat.messageInfo(reply, members, { me: "ann" });
    assert.equal(thread.readBy, null);
    assert.equal(thread.thread, true);
    lines = chat.messageInfoLines(thread, when);
    assert.deepEqual(Array.from(new Set(lines.map((line) => line.key))), ["sent", "delivered", "note"]);
    assert.ok(lines[lines.length - 1].text.includes("A thread has no read receipts."));
    assert.equal(chat.messageInfo(message, members, { me: "ann", inThread: true }).readBy, null, "the root shown in its thread too");
    // a message that came in from a network says where from
    const inbound = chat.messageInfo(event({ eventSeq: 5, senderUserId: null, senderExternal: { network: "sms", networkName: "SMS" } }), members, { me: "ann" });
    assert.equal(chat.messageInfoLines(inbound, when).find((line) => line.key === "from").text, "SMS");
});

test("before a send (§6.1 rule 3): a verified person's device not verified is a problem unless let through for the room by its key; one verified by emoji passes; a changed identity until let through by its master key", async () => {
    const problemsOf = moqui.crypto.problemsOf;
    const device = (deviceId, more) => Object.assign({ deviceId, displayName: deviceId + " phone", key: "key-" + deviceId, verified: false, locallyTrusted: false }, more || {});
    // bob's identity is verified: his new device B2 (signed by nobody I trust) is a problem; B1, signed by him, is not
    const bob = { userId: "bob", identity: { verified: true }, devices: [device("B1", { verified: true }), device("B2")] };
    assert.deepEqual(problemsOf([bob], []).map((p) => p.kind + ":" + p.deviceId), ["newDevice:B2"]);
    // verified here by emoji (local trust, not signed by bob): it passes, the same rule the send then shares by
    assert.deepEqual(problemsOf([{ userId: "bob", identity: { verified: true }, devices: [device("B1", { verified: true }), device("B2", { locallyTrusted: true })] }], []), []);
    // let through for this room with its key; the same device id with another key is asked again
    assert.deepEqual(problemsOf([bob], [{ userId: "bob", deviceId: "B2", key: "key-B2" }]), []);
    assert.equal(problemsOf([bob], [{ userId: "bob", deviceId: "B2", key: "another" }]).length, 1);
    // a person verified by emoji only (no verified identity) is a verified person too: their new device is asked
    const cy = { userId: "cy", identity: { verified: false }, devices: [device("C1", { verified: true, locallyTrusted: true }), device("C2")] };
    assert.deepEqual(problemsOf([cy], []).map((p) => p.deviceId), ["C2"]);
    // nobody verified dee: no question
    assert.deepEqual(problemsOf([{ userId: "dee", identity: null, devices: [device("D1")] }], []), []);
    // a changed identity, until let through by this master key
    const eve = { userId: "eve", identity: { verified: false, changed: true, masterKey: "mk-2" }, devices: [device("E1")] };
    assert.deepEqual(problemsOf([eve], []).map((p) => p.kind + ":" + p.masterKey), ["changedIdentity:mk-2"]);
    assert.deepEqual(problemsOf([eve], [{ userId: "eve", masterKey: "mk-2" }]), []);
    assert.equal(problemsOf([eve], [{ userId: "eve", masterKey: "mk-1" }]).length, 1, "an identity let through before is not this one");
});

test("a dm link opens the direct room of exactly those people, the encrypted one first, else none (an unsaved message: no room until the first send)", () => {
    const rooms = [{ chatRoomId: "D1", roomTypeEnumId: "CrtDirect", isMember: true, otherUserIdList: ["bob"] },
        { chatRoomId: "D2", roomTypeEnumId: "CrtDirect", isMember: true, otherUserIdList: ["cy", "bob"] },
        { chatRoomId: "E1", roomTypeEnumId: "CrtDirect", isMember: true, encrypted: true, otherUserIdList: ["dee"] },
        { chatRoomId: "P1", roomTypeEnumId: "CrtPrivate", isMember: true, otherUserIdList: ["eve"] },
        { chatRoomId: "L1", roomTypeEnumId: "CrtDirect", isMember: false, otherUserIdList: ["fay"] }];
    assert.equal(chat.directRoomFor(rooms, ["bob"], "ann"), "D1");
    assert.equal(chat.directRoomFor(rooms, ["bob", "cy", "ann"], "ann"), "D2", "a group's, in any order, me left out");
    assert.equal(chat.directRoomFor(rooms, ["dee"], "ann"), "E1", "an encrypted direct room with no plain one");
    // both: the encrypted one first (Comms-Messaging §6.5), the plain one when asked for it (the link back to the earlier chat)
    const both = rooms.concat([{ chatRoomId: "E2", roomTypeEnumId: "CrtDirect", isMember: true, encrypted: true, otherUserIdList: ["bob"] }]);
    assert.equal(chat.directRoomFor(both, ["bob"], "ann"), "E2");
    assert.equal(chat.directRoomFor(both, ["bob"], "ann", true), "D1");
    assert.equal(chat.directRoomFor(rooms, ["dee"], "ann", true), null, "no plain one");
    // two of a kind (made at once): the oldest, as the server answers it
    const twins = [{ chatRoomId: "100012", roomTypeEnumId: "CrtDirect", isMember: true, encrypted: true, otherUserIdList: ["hal"] },
        { chatRoomId: "100009", roomTypeEnumId: "CrtDirect", isMember: true, encrypted: true, otherUserIdList: ["hal"] }];
    assert.equal(chat.directRoomFor(twins, ["hal"], "ann"), "100009");
    assert.equal(chat.directRoomFor(rooms, ["eve"], "ann"), null, "a private room is no direct chat");
    assert.equal(chat.directRoomFor(rooms, ["fay"], "ann"), null, "one I am not in");
    assert.equal(chat.directRoomFor(rooms, ["gus"], "ann"), null);
    assert.equal(chat.directRoomFor(rooms, ["ann"], "ann"), null, "myself alone is nobody");
});

test("a new device needs setting up only while it holds neither the backup's key nor a verification, and has a backup or another device to set it up from", () => {
    const status = (fields) => Object.assign({ deviceId: "N1", backup: null, restored: false, verified: false, otherDevices: [] }, fields);
    assert.equal(chat.setupNeeded(null), false);
    assert.equal(chat.setupNeeded(status({})), false, "a first device: nothing to set it up from");
    assert.equal(chat.setupNeeded(status({ backup: "2" })), true, "a key backup to restore");
    assert.equal(chat.setupNeeded(status({ otherDevices: [{ deviceId: "O1" }] })), true, "another device to verify with");
    assert.equal(chat.setupNeeded(status({ backup: "2", restored: true })), false, "restored");
    assert.equal(chat.setupNeeded(status({ otherDevices: [{ deviceId: "O1" }], verified: true })), false, "verified, with no backup to receive");
    // verified with a key backup on the server: not set up until its key came and the backup was restored
    assert.equal(chat.setupNeeded(status({ backup: "2", otherDevices: [{ deviceId: "O1" }], verified: true })), true, "verified, the backup key not here yet");
    assert.equal(chat.setupWaiting(status({ backup: "2", verified: true })), true, "getting the keys");
    assert.equal(chat.setupWaiting(status({ backup: "2", verified: true, restored: true })), false);
    assert.equal(chat.setupWaiting(status({ backup: "2" })), false, "not verified: nothing to wait for");
    // the key backup hint of a first device: once, closed per account
    chat.me = "ann";
    try { localStorage.removeItem(chat.setupHintKey()); } catch (err) { /* none */ }
    assert.equal(chat.backupHinted(status({})), true);
    chat.closeSetupHint();
    assert.equal(chat.backupHinted(status({})), false);
    assert.equal(chat.backupHinted(status({ backup: "1" })), false);
    // Not now, per account and device
    assert.equal(chat.setupLater("N9"), false);
    chat.putSetupLater("N9");
    assert.equal(chat.setupLater("N9"), true);
    assert.equal(chat.setupLater("N8"), false);
});

test("the mail lines are asked once for the page within five minutes, by room; a failure keeps nothing", async () => {
    chat.mailCache = null;
    let asked = 0;
    let fail = false;
    const request = () => {
        asked++;
        return fail ? Promise.reject({ status: 0 })
            : Promise.resolve({ json: { mailList: [{ chatRoomId: "D1", url: "/qapps/BasicComms/Inbox?thread=email%3AM1", subject: "Q3" }, { chatRoomId: "D2" }] } });
    };
    const url = "/apps/BasicComms/Chat/mailLines";
    const first = await chat.mailLines(url, request);
    const again = await chat.mailLines(url, request);
    assert.deepEqual(Object.keys(first), ["D1"], "a line with no link is left out");
    assert.equal(again.D1.subject, "Q3");
    assert.equal(asked, 1, "the Chat screen and the panel share one request");
    chat.mailCache.at -= 300001;
    fail = true;
    assert.deepEqual(await chat.mailLines(url, request), {});
    assert.equal(asked, 2);
    assert.equal(chat.mailCache, null, "the failure is not kept");
    fail = false;
    await chat.mailLines(url, request);
    assert.equal(asked, 3);
    chat.mailCache = null;
});
