/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * Chat and presence in the /qapps shell (IronStone design-drawers-commons §1.8, §13; impl-drawers-commons M10): moqui.chat, a
 * small store over the notification socket, and the account menu's status. It keeps
 *   - presence: the presence and status of the users the page shows (m-avatar with a user-id draws the dot from it),
 *   - me: the current user's own status, set from the account menu, with the heartbeat that keeps it (auto-away from idle),
 *   - unread: each room's unread and mention counts, typing: who is typing in each room, reads: the members' read markers,
 *   - outbox: the messages sent and not yet answered, kept in localStorage per user (moqui.chat.outbox.<userId>) so a
 *     send made offline, or one that failed for a while, goes when the page is back (chat.send, chat.flush),
 * and hands the chat pushes to the screens that listen (moqui.chat.on). The server side is org.moqui.impl.ChatServices; the
 * topics and their messages are described there and in js/README.md, "Chat store and presence".
 *
 * Loaded after MoquiLib.js and before WebrootVue.qvt.js (qapps.xml): its components are in moqui.qvtComponents, which
 * WebrootVue.qvt.js registers, and the shell root calls moqui.chat.start(root) once mounted. Runs under Node for its tests
 * (WebrootChat.test.js): nothing here touches the page until start.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui;
    const reactive = (obj) => (root.Vue && root.Vue.reactive ? root.Vue.reactive(obj) : obj);

    // A presence as the dot draws it: online, away, dnd or offline; invisible is offline (to others and on one's own dot).
    const STATUSES = ["online", "away", "dnd", "invisible"];
    const DOT = { online: "online", away: "away", dnd: "dnd", invisible: "offline", offline: "offline" };
    const TYPING_FOR = 6000;
    const BEAT_EVERY = 60 * 1000;
    const STATUS_TTL = 5 * 60 * 1000;
    // the outbox: its size, how long an item with files waits (clean#PendingAttachments removes unsent files after 24
    // hours), the waits between tries, and how often it looks for an item that is due
    const OUTBOX_MAX = 50;
    const OUTBOX_ITEM_BYTES = 64 * 1024;
    const OUTBOX_FILES_FOR = 20 * 3600 * 1000;
    const BACKOFF = [2000, 5000, 15000, 60000];
    const FLUSH_EVERY = 2000;

    const chat = {
        labels: { statusOnline: "Online", statusAway: "Away", statusDnd: "Do not disturb", statusInvisible: "Invisible", statusOffline: "Offline",
            setStatus: "Set a status message", clearStatus: "Clear status message", statusText: "What's your status?", emoji: "Emoji",
            clearAfter: "Clear after", dontClear: "Don't clear", minutes30: "30 minutes", hour1: "1 hour", hours4: "4 hours", today: "Today",
            thisWeek: "This week", save: "Save", cancel: "Cancel", status: "Status",
            assistApprove: "Approve", assistReject: "Reject", assistFeedback: "What should the agent do instead?", assistViewRun: "View run",
            assistWaitingFor: "Waiting for {0}", assistAskedBy: "Asked by {0}", assistDecidedBy: "{0} by {1}", assistNoDecide: "This could not be decided",
            assistStates: { pending: "Waiting", approved: "Approved", rejected: "Rejected", executed: "Done", executing: "Running", failed: "Failed",
                expired: "Expired", conflict: "Changed meanwhile", handedOff: "Handed to the screen", answered: "Answered", done: "Done" } },
        // The status presets until the user keeps their own (the preference CHAT_STATUS_PRESETS), as IronStone's chat settings
        // start them: {emoji, text, clearAfter}, clearAfter minutes, today, week or 0 for never.
        defaultPresets: [{ emoji: "📅", text: "In a meeting", clearAfter: "60" }, { emoji: "🚌", text: "Commuting", clearAfter: "30" },
            { emoji: "🤒", text: "Out sick", clearAfter: "today" }, { emoji: "🏠", text: "Working remotely", clearAfter: "today" },
            { emoji: "🌴", text: "On vacation", clearAfter: "0" }],

        // ---- the pure parts (WebrootChat.test.js) ----

        statuses: STATUSES,
        // the dot of a presence or status, "" for none
        dot(presence) { return DOT[presence] || ""; },
        // the clear-at choices of the status dialog: minutes from now, or the end of today or of this week
        clearOptions(labels) {
            const text = Object.assign({}, chat.labels, labels);
            return [{ value: 0, label: text.dontClear }, { value: 30, label: text.minutes30 }, { value: 60, label: text.hour1 },
                { value: 240, label: text.hours4 }, { value: "today", label: text.today }, { value: "week", label: text.thisWeek }];
        },
        // The status presets of the status dialog: the user's CHAT_STATUS_PRESETS preference (a JSON list of {emoji, text,
        // clearAfter}, which a chat settings screen keeps), else the defaults, their text localized; at most 12, each with a
        // text or an emoji, clearAfter as a value of clearOptions (0 never, minutes, today or week).
        statusPresets(preferences, localize) {
            let list = null;
            const kept = preferences ? preferences.CHAT_STATUS_PRESETS : null;
            if (kept !== null && kept !== undefined && String(kept).trim() !== "") {
                try { list = JSON.parse(String(kept)); } catch (err) { list = null; }
            }
            const own = Array.isArray(list);
            if (!own) list = chat.defaultPresets;
            const clearOf = (value) => {
                const text = String(value === null || value === undefined ? "0" : value).trim();
                if (text === "today" || text === "week") return text;
                const minutes = Number(text);
                return [30, 60, 240].indexOf(minutes) >= 0 ? minutes : 0;
            };
            return list.filter((item) => item && (item.text || item.emoji)).slice(0, 12).map((item) => ({
                emoji: String(item.emoji || ""),
                text: own || !localize ? String(item.text || "") : localize(String(item.text || "")),
                clearAfter: clearOf(item.clearAfter)
            }));
        },
        // the time a clear-at choice comes to (epoch millis, in the browser's zone for today and the week), null for none
        clearAt(option, now) {
            const at = new Date(now);
            if (option === "today") { at.setHours(23, 59, 59, 999); return at.getTime(); }
            if (option === "week") {
                // the end of Sunday, the week of ISO 8601
                const toSunday = (7 - at.getDay()) % 7;
                at.setDate(at.getDate() + toSunday);
                at.setHours(23, 59, 59, 999);
                return at.getTime();
            }
            const minutes = Number(option);
            return minutes > 0 ? now + minutes * 60000 : null;
        },
        // whole seconds since the last input
        idleSeconds(lastInput, now) { return Math.max(0, Math.floor((now - lastInput) / 1000)); },
        // whether input now, after the last beat reported this idle, should beat at once (the user is back from away)
        backFromIdle(lastBeatIdle, autoAwayMinutes) { return lastBeatIdle >= (Number(autoAwayMinutes) || 10) * 60; },

        // A pushed event merged into a timeline (a list of events as get#Timeline answers them, oldest first): a message of
        // the main timeline is added once (a thread reply only counts on its root, unless the list is that thread), an
        // edit replaces its message's body, a reaction is added to its message, a redaction removes a message's content or
        // takes a reaction back, a pin marks its message. Returns the list (the same array, changed in place).
        applyEvent(list, event, me, threadRoot) {
            if (!Array.isArray(list) || !event) return list;
            const seq = Number(event.eventSeq);
            const find = (target) => list.find((item) => Number(item.eventSeq) === Number(target));
            const type = event.type || "";
            if (type === "edit") {
                const target = find(event.editsEventSeq);
                if (target && !target.redacted) {
                    target.body = event.body; target.bodyText = event.bodyText; target.mentions = event.mentions || [];
                    target.edited = true; target.editedDate = event.sentDate;
                }
                return list;
            }
            if (type === "reaction") {
                const target = find(event.relatesToEventSeq);
                const key = event.content && event.content.key;
                if (!target || !key) return list;
                target.reactions = target.reactions || [];
                let entry = target.reactions.find((reaction) => reaction.key === key);
                if (!entry) { entry = { key, count: 0, userIds: [], mine: false }; target.reactions.push(entry); }
                if (entry.userIds.indexOf(event.senderUserId) < 0) {
                    entry.userIds.push(event.senderUserId);
                    entry.count++;
                    if (event.senderUserId === me) entry.mine = true;
                }
                return list;
            }
            if (type === "redaction") {
                const content = event.content || {};
                if (content.target === "reaction") {
                    const target = find(content.messageSeq);
                    const entry = target && (target.reactions || []).find((reaction) => reaction.key === content.key);
                    if (entry) {
                        const index = entry.userIds.indexOf(event.senderUserId);
                        if (index >= 0) { entry.userIds.splice(index, 1); entry.count--; }
                        if (event.senderUserId === me) entry.mine = false;
                        if (entry.count <= 0) target.reactions.splice(target.reactions.indexOf(entry), 1);
                    }
                } else {
                    const target = find(event.relatesToEventSeq);
                    if (target) Object.assign(target, { redacted: true, body: null, bodyText: null, attachments: [], records: [], reactions: [],
                        mentions: [], pinned: false, edited: false, redactedDate: event.sentDate });
                    if (target && target.threadRootEventSeq != null && threadRoot == null) {
                        const rootItem = find(target.threadRootEventSeq);
                        if (rootItem) rootItem.threadReplyCount = Math.max(0, (rootItem.threadReplyCount || 0) - 1);
                    }
                }
                return list;
            }
            if (type === "pin") {
                const target = find(event.relatesToEventSeq);
                if (target) { target.pinned = !!(event.content && event.content.pinned); target.pinnedByUserId = target.pinned ? event.senderUserId : null; }
                // the pin is also a row of the timeline
            }
            if (find(seq)) return list;
            const inThread = event.threadRootEventSeq != null;
            if (inThread && Number(threadRoot) !== Number(event.threadRootEventSeq)) {
                const rootItem = find(event.threadRootEventSeq);
                if (rootItem) {
                    rootItem.threadReplyCount = (rootItem.threadReplyCount || 0) + 1;
                    rootItem.threadLastDate = event.sentDate;
                    rootItem.threadLastEventSeq = seq;
                    rootItem.threadLastUserId = event.senderUserId || null;
                }
                return list;
            }
            if (threadRoot != null && !inThread && seq !== Number(threadRoot)) return list;
            const item = Object.assign({ edited: false, redacted: false, pinned: false, reactions: [], threadReplyCount: 0, replyTo: null }, event);
            let at = list.length;
            while (at > 0 && Number(list[at - 1].eventSeq) > seq) at--;
            list.splice(at, 0, item);
            return list;
        },

        // An agent's card (content.assist of a message: an approval, a question or a reply; ChatServices.send#AssistCard) as a
        // room shows it to the user me: its kind and state, whether it waits, who may act, and the lines to show. Any human who
        // may post in the room may decide an approval (the server checks, and refuses agents); only the requester answers a
        // question or opens the run. null for a message with no card.
        assistCard(event, me, labels) {
            const card = event && moqui.isPlainObject(event.assist) ? event.assist : null;
            if (!card || !card.kind) return null;
            const text = Object.assign({}, chat.labels, labels);
            const fill = (template, ...values) => String(template).replace(/\{(\d)\}/g, (match, index) => values[Number(index)] ?? "");
            const state = String(card.state || (card.kind === "reply" ? "done" : "pending"));
            const pending = state === "pending";
            const requester = card.requesterName || card.requesterUserId || "";
            const mine = !!me && card.requesterUserId === me;
            const stateLabel = (text.assistStates || {})[state] || state;
            const decidedBy = card.decidedBy && (card.decidedBy.name || card.decidedBy.userId);
            return {
                kind: card.kind, state, pending, stateLabel, mine,
                itemId: card.itemId || null, runId: card.runId || null, conversationId: card.conversationId || null,
                title: card.title || "", summary: card.summary || "",
                rows: Array.isArray(card.rows) ? card.rows.map((row) => ({ label: row.label || row.field || "", current: row.current ?? "", proposed: row.proposed ?? "" })) : [],
                options: Array.isArray(card.options) ? card.options.map((option) => (moqui.isPlainObject(option) ? option.label : String(option))).filter((label) => label) : [],
                canDecide: card.kind === "approval" && pending,
                canAnswer: card.kind === "question" && pending && mine && !card.hasFields,
                canViewRun: mine && !!card.conversationId,
                askedBy: requester ? fill(text.assistAskedBy, requester) : "",
                waiting: card.kind === "question" && pending && !mine && requester ? fill(text.assistWaitingFor, requester) : "",
                decided: !pending && card.kind !== "reply" ? (decidedBy ? fill(text.assistDecidedBy, stateLabel, decidedBy) : stateLabel) : ""
            };
        },

        // A pushed update of a message (push#Update: its link previews, its poll) merged into a timeline: the fields replace
        // the message's, but what is the user's own stays as the list has it (mine: the push is the same for everyone, so it
        // has none). Nothing for a message not in the list. Returns the list (changed in place).
        applyUpdate(list, payload, me) {
            if (!Array.isArray(list) || !payload || !moqui.isPlainObject(payload.fields)) return list;
            const target = list.find((item) => Number(item.eventSeq) === Number(payload.eventSeq));
            if (!target || target.redacted) return list;
            const keepMine = (before, after) => {
                if (Array.isArray(after)) {
                    if (!Array.isArray(before)) return after;
                    return after.map((value, index) => {
                        const match = moqui.isPlainObject(value) && value.id !== undefined
                            ? before.find((old) => moqui.isPlainObject(old) && old.id === value.id) : before[index];
                        return keepMine(match, value);
                    });
                }
                if (!moqui.isPlainObject(after)) return after;
                const out = {};
                Object.keys(after).forEach((key) => { out[key] = keepMine(moqui.isPlainObject(before) ? before[key] : undefined, after[key]); });
                if (moqui.isPlainObject(before) && before.mine !== undefined && after.mine === undefined) out.mine = before.mine;
                return out;
            };
            Object.keys(payload.fields).forEach((key) => { target[key] = keepMine(target[key], payload.fields[key]); });
            return list;
        },

        // ---- the outbox (pure) ----

        // The outbox with item added at its end: a new list, or null when it is full (50 items) or the item is over 64 KB.
        outboxAdd(list, item) {
            const items = Array.isArray(list) ? list : [];
            if (!item || !item.clientRef || items.length >= OUTBOX_MAX) return null;
            if (JSON.stringify(item).length > OUTBOX_ITEM_BYTES) return null;
            return items.filter((other) => other.clientRef !== item.clientRef).concat([item]);
        },
        outboxRemove(list, clientRef) { return (Array.isArray(list) ? list : []).filter((item) => item.clientRef !== clientRef); },
        // whether an item with files waited too long: its files are about to go (clean#PendingAttachments)
        outboxExpired(item, now) { return !!item && !!item.hasFiles && now - Number(item.createdDate || 0) > OUTBOX_FILES_FOR; },
        // The items of a room (threadRoot null) or of one of its threads, oldest first; one that expired shows as not sent.
        outboxFor(list, roomId, threadRoot, now) {
            return (Array.isArray(list) ? list : []).filter((item) => item.chatRoomId === roomId
                && (threadRoot == null ? item.threadRootEventSeq == null : Number(item.threadRootEventSeq) === Number(threadRoot)))
                .map((item) => (chat.outboxExpired(item, now) ? Object.assign({}, item, { status: "failed", expired: true }) : item));
        },
        // What a send that did not go does next, by its HTTP status: retry (0 no answer, 408, 429, 5xx), hold (401 and 403,
        // until the user is signed in again) or fail (another 4xx, or an answer with no message).
        classify(status) {
            const code = Number(status) || 0;
            if (code === 0 || code === 408 || code === 429 || code >= 500) return "retry";
            if (code === 401 || code === 403) return "hold";
            return "fail";
        },
        // the wait before try number attempts + 1: 2, 5, 15, then 60 seconds
        backoff(attempts) { return BACKOFF[Math.min(Math.max(Number(attempts) || 1, 1), BACKOFF.length) - 1]; },

        // A message of the socket on the state: presence (userStatus), the room events and signals (chatRoom:<id>) and the
        // user's room list (chatUser:<me>). Returns what it was ({kind, chatRoomId, ...}) for the listeners, or null.
        reduce(state, jsonObj, me, now) {
            const topic = jsonObj && jsonObj.topic ? String(jsonObj.topic) : "";
            let message = jsonObj ? jsonObj.message : null;
            if (moqui.isString(message)) { try { message = JSON.parse(message); } catch (err) { message = null; } }
            if (!topic || !moqui.isPlainObject(message)) return null;
            if (topic === "userStatus") {
                if (!message.userId) return null;
                const entry = { presence: message.presence || "offline", statusText: message.statusText || null, emoji: message.emoji || null,
                    clearAtDate: message.clearAtDate || null, inCall: !!message.inCall, at: now };
                if (message.userId === me) {
                    state.me = Object.assign({}, state.me || {}, entry, message.self && message.status ? { status: message.status } : {});
                    if (message.self && message.status === "invisible") entry.presence = "offline";
                }
                state.presence[message.userId] = entry;
                return { kind: "presence", userId: message.userId, presence: entry.presence };
            }
            if (topic.indexOf("chatRoom:") === 0) {
                const roomId = message.chatRoomId || topic.slice("chatRoom:".length);
                if (message.kind === "typing") {
                    const typing = state.typing[roomId] = Object.assign({}, state.typing[roomId]);
                    if (message.typing) typing[message.userId] = now + TYPING_FOR;
                    else delete typing[message.userId];
                    // what an agent member says it does ("Looking up open orders…")
                    const texts = state.typingText = state.typingText || {};
                    const roomTexts = texts[roomId] = Object.assign({}, texts[roomId]);
                    if (message.typing && message.statusText) roomTexts[message.userId] = String(message.statusText);
                    else delete roomTexts[message.userId];
                    return { kind: "typing", chatRoomId: roomId, userId: message.userId, typing: !!message.typing, statusText: message.statusText || null };
                }
                if (message.kind === "read") {
                    state.reads[roomId] = Object.assign({}, state.reads[roomId], { [message.userId]: message.lastReadEventSeq });
                    return { kind: "read", chatRoomId: roomId, userId: message.userId, lastReadEventSeq: message.lastReadEventSeq };
                }
                if (message.kind === "update" && moqui.isPlainObject(message.fields)) {
                    return { kind: "update", chatRoomId: roomId, eventSeq: message.eventSeq, threadRootEventSeq: message.threadRootEventSeq ?? null,
                        fields: message.fields };
                }
                if (message.kind === "event" && message.event) {
                    const event = message.event;
                    // who sent a message has stopped typing
                    if (event.senderUserId && state.typing[roomId] && state.typing[roomId][event.senderUserId]) {
                        const typing = state.typing[roomId] = Object.assign({}, state.typing[roomId]);
                        delete typing[event.senderUserId];
                    }
                    if (event.type === "message" && event.senderUserId !== me && !state.openRooms[roomId]) {
                        const counts = state.unread[roomId] = Object.assign({ unread: 0, mentions: 0 }, state.unread[roomId]);
                        counts.unread++;
                        if ((event.mentions || []).indexOf(me) >= 0) counts.mentions++;
                    }
                    return { kind: "event", chatRoomId: roomId, event };
                }
                return null;
            }
            if (topic.indexOf("chatUser:") === 0) {
                if (message.kind === "read") {
                    if (message.all) Object.keys(state.unread).forEach((roomId) => { state.unread[roomId] = { unread: 0, mentions: 0 }; });
                    else if (message.chatRoomId) state.unread[message.chatRoomId] = { unread: 0, mentions: 0 };
                    return { kind: "read", chatRoomId: message.chatRoomId || null, userId: me, all: !!message.all, lastReadEventSeq: message.lastReadEventSeq };
                }
                if (message.kind === "room") {
                    if (message.action === "left") delete state.unread[message.chatRoomId];
                    return { kind: "room", chatRoomId: message.chatRoomId, action: message.action };
                }
            }
            return null;
        },
        // the users still typing in a room
        typingUsers(state, roomId, now) {
            const typing = state.typing[roomId] || {};
            return Object.keys(typing).filter((userId) => typing[userId] > now);
        },
        // the room's unread counts and the total of every room
        unreadOf(state, roomId) { return state.unread[roomId] || { unread: 0, mentions: 0 }; },
        unreadTotal(state) { return Object.keys(state.unread).reduce((sum, roomId) => sum + (state.unread[roomId].unread || 0), 0); },

        // ---- the store of the page ----

        state: reactive({ me: null, presence: {}, unread: {}, typing: {}, reads: {}, openRooms: {}, statusDialog: false, outbox: [], sending: {},
            online: true, connected: null }),
        listeners: {},
        vm: null,
        started: false,
        watched: {},
        pendingUsers: [],
        lastInput: Date.now(),
        lastBeatIdle: 0,

        // The page's store starts once the shell is mounted: its socket listener, the user's status, the counts, the idle
        // tracking and the heartbeat. Without a signed in user it does nothing.
        start(vm) {
            if (chat.started || !vm || !vm.userId) return;
            chat.started = true;
            chat.vm = vm;
            chat.me = vm.userId;
            const client = vm.notificationClient;
            if (client) {
                client.registerListener("ALL", chat.receive);
                // the topics of the store, named so a page that subscribes topic by topic gets them too
                ["chatUser:" + chat.me, "userStatus"].forEach((topic) => client.registerListener(topic, chat.noop));
            }
            const prefs = (moqui.config && moqui.config.preferences) || {};
            chat.autoAwayMinutes = Number(prefs.CHAT_AUTO_AWAY_MINUTES) || 10;
            chat.request("/userStatus").then((json) => { if (json && json.myStatus) chat.setMe(json.myStatus); });
            chat.request("/chatUnread").then((json) => {
                const counts = json && moqui.isPlainObject(json.roomCounts) ? json.roomCounts : {};
                Object.keys(counts).forEach((roomId) => { chat.state.unread[roomId] = { unread: counts[roomId].unread || 0, mentions: counts[roomId].mentions || 0 }; });
            });
            const doc = root.document;
            if (doc && doc.addEventListener) {
                const input = () => chat.onInput();
                ["keydown", "pointerdown", "wheel", "touchstart", "focusin"].forEach((name) => doc.addEventListener(name, input, { passive: true, capture: true }));
                let moved = 0;
                doc.addEventListener("mousemove", () => { const now = Date.now(); if (now - moved > 5000) { moved = now; chat.onInput(); } }, { passive: true });
                doc.addEventListener("visibilitychange", () => { if (doc.visibilityState === "visible") chat.onInput(); });
            }
            chat.beat();
            chat.beatTimer = setInterval(() => chat.beat(), BEAT_EVERY);
            // the avatars mounted before the store started
            chat.watchUsers([]);
            // the outbox: what waited goes now, when the browser and the socket are back, and when an item is due; other
            // tabs' changes are read (the server answers a message sent twice with the first)
            chat.state.online = !(root.navigator && root.navigator.onLine === false);
            chat.readOutbox();
            if (root.addEventListener) {
                root.addEventListener("online", () => { chat.state.online = true; chat.flush(true); });
                root.addEventListener("offline", () => { chat.state.online = false; chat.touchOutbox(); });
                root.addEventListener("storage", (event) => { if (event && event.key === chat.outboxKey()) { chat.readOutbox(); chat.emit("outbox", {}); } });
            }
            if (client && client.onState) client.onState((socketState) => {
                const reconnected = socketState === "open" && chat.state.connected === false;
                chat.state.connected = socketState === "open";
                if (socketState === "open") chat.flush(true);
                chat.emit("connection", { state: socketState, reconnected });
            });
            chat.flushTimer = setInterval(() => chat.flush(false), FLUSH_EVERY);
            chat.flush(true);
        },
        noop() {},
        // a GET or POST to a webroot transition of the store, its JSON (null when it fails, which is logged)
        request(path, params, method) {
            const base = (chat.vm && chat.vm.appRootPath) || "";
            return moqui.request(base + path, { method: method || "GET", params }).then((result) => result.json || null,
                (err) => { console.info("chat " + path + " failed", err && err.status); return null; });
        },
        receive(jsonObj) {
            const done = chat.reduce(chat.state, jsonObj, chat.me, Date.now());
            if (!done) return;
            // my message arrived: it waits no more (it was sent from another tab, or its answer was lost)
            if (done.kind === "event" && done.event.clientRef && done.event.senderUserId === chat.me) chat.dropOutbox(done.event.clientRef);
            if (done.kind === "presence" && done.userId === chat.me) chat.applyQuiet();
            chat.emit(done.kind, done);
        },
        // listeners of the screens: kind is event, update, typing, read, room or presence; outbox (the outbox changed), sent
        // ({clientRef, chatRoomId, threadRootEventSeq, json}: a message of the outbox was answered) and connection ({state,
        // reconnected}); returns the function that removes it
        on(kind, fn) {
            (chat.listeners[kind] = chat.listeners[kind] || []).push(fn);
            return () => { const list = chat.listeners[kind] || []; const index = list.indexOf(fn); if (index >= 0) list.splice(index, 1); };
        },
        emit(kind, payload) {
            (chat.listeners[kind] || []).slice().forEach((fn) => {
                try { fn(payload); } catch (err) { console.error("chat listener failed", err); }
            });
        },
        // Whether a chat push should reload the tray's Chat tab: not for a typing or read signal, and not when a screen
        // listens to the events itself (a live room list or room).
        trayReloads(jsonObj) {
            const message = jsonObj && moqui.isPlainObject(jsonObj.message) ? jsonObj.message : {};
            if (message.kind === "typing" || message.kind === "read" || message.kind === "update") return false;
            return !((chat.listeners.event || []).length || (chat.listeners.room || []).length);
        },
        // a room shown now: its events count as read there; the socket is told of its topic
        openRoom(roomId) {
            if (!roomId) return;
            chat.state.openRooms[roomId] = (chat.state.openRooms[roomId] || 0) + 1;
            chat.state.unread[roomId] = { unread: 0, mentions: 0 };
            if (chat.vm && chat.vm.notificationClient) chat.vm.notificationClient.registerListener("chatRoom:" + roomId, chat.noop);
        },
        closeRoom(roomId) {
            if (!chat.state.openRooms[roomId]) return;
            if (--chat.state.openRooms[roomId] <= 0) delete chat.state.openRooms[roomId];
        },
        // typing in a room's composer: told at most every 3 seconds, and once when it stops
        typing(roomId, typing) {
            const now = Date.now();
            chat.typingSent = chat.typingSent || {};
            const stop = typing === false;
            if (!stop && chat.typingSent[roomId] && now - chat.typingSent[roomId] < 3000) return;
            if (stop && !chat.typingSent[roomId]) return;
            chat.typingSent[roomId] = stop ? 0 : now;
            chat.request("/chatTyping", { chatRoomId: roomId, typing: !stop }, "POST");
        },
        typingIn(roomId) { return chat.typingUsers(chat.state, roomId, Date.now()); },
        // the status text of a user typing in a room (an agent's), "" for none
        typingText(roomId, userId) { return ((chat.state.typingText || {})[roomId] || {})[userId] || ""; },
        // A button of an agent's card: decision approve or reject (with feedback), or choice for a question. Resolves to the
        // server's answer ({status}) or {error}; the card itself changes through the room's update push.
        decideCard(roomId, eventSeq, params) {
            const base = (chat.vm && chat.vm.appRootPath) || "";
            return moqui.request(base + "/chatAssistCard", { method: "POST", params: Object.assign({ chatRoomId: roomId, eventSeq }, params) })
                .then((result) => (result && result.json) || {}, (err) => ({ error: (err && err.json && err.json.errors) || chat.labels.assistNoDecide }));
        },

        // ---- the outbox ----

        outboxKey() { return "moqui.chat.outbox." + (chat.me || ""); },
        // the outbox as stored (another tab may have changed it); the state changes only when it did, so the screens
        // do not draw their pending rows again for nothing
        readOutbox() {
            let text = null;
            try { text = root.localStorage ? root.localStorage.getItem(chat.outboxKey()) : null; } catch (err) { text = null; }
            if (text !== null && text === chat.outboxText) return chat.state.outbox.map((item) => Object.assign({}, item));
            let list = [];
            try {
                const parsed = text ? JSON.parse(text) : [];
                if (Array.isArray(parsed)) list = parsed.filter((item) => item && item.clientRef && item.chatRoomId);
            } catch (err) { list = []; }
            // without storage, or with storage that refused the last write, the state is the outbox
            if (chat.storageFailed || (text === null && !chat.outboxText && chat.state.outbox.length)) list = chat.state.outbox;
            chat.outboxText = text;
            if (list !== chat.state.outbox) chat.state.outbox = list;
            return list.map((item) => Object.assign({}, item));
        },
        outboxText: null,
        storageFailed: false,
        saveOutbox(list) {
            chat.state.outbox = list;
            try {
                if (root.localStorage) {
                    if (list.length) root.localStorage.setItem(chat.outboxKey(), JSON.stringify(list));
                    else root.localStorage.removeItem(chat.outboxKey());
                    chat.outboxText = list.length ? JSON.stringify(list) : null;
                }
                chat.storageFailed = false;
            } catch (err) { chat.storageFailed = true; console.info("chat outbox not stored", err); }
            chat.emit("outbox", {});
        },
        touchOutbox() { chat.state.outbox = chat.state.outbox.slice(); chat.emit("outbox", {}); },
        dropOutbox(clientRef) {
            if (!chat.state.outbox.some((item) => item.clientRef === clientRef)) return;
            chat.saveOutbox(chat.outboxRemove(chat.readOutbox(), clientRef));
        },
        changeOutbox(clientRef, fields) {
            const list = chat.readOutbox();
            const item = list.find((other) => other.clientRef === clientRef);
            if (!item) return null;
            Object.assign(item, fields);
            chat.saveOutbox(list);
            return item;
        },
        // the outbox items of a room or a thread, each with its status: sending (asked now), waiting (to try again),
        // held (until the user is signed in again) or failed (not sent: Retry or Delete)
        pendingIn(roomId, threadRoot) {
            return chat.outboxFor(chat.state.outbox, roomId, threadRoot, Date.now()).map((item) => (chat.state.sending[item.clientRef]
                ? Object.assign({}, item, { status: "sending" }) : item));
        },
        // Send a message through the outbox: item {chatRoomId, url (the send transition), params (its other parameters),
        // text (shown while it waits), threadRootEventSeq, hasFiles}, given a clientRef. It is kept first, so a page
        // closed or offline sends it later. Resolves to {sent: true, json} when answered now, {queued: true} when it
        // waits, {failed: true, error} when refused; rejects when the outbox cannot take it (full, or too large).
        send(item) {
            const kept = Object.assign({ clientRef: chat.newClientRef(), createdDate: Date.now(), attempts: 0, nextDate: 0, status: "waiting" }, item);
            const list = chat.outboxAdd(chat.readOutbox(), kept);
            if (!list) return Promise.reject({ outbox: true, status: 0 });
            chat.saveOutbox(list);
            if (!chat.state.online) return Promise.resolve({ queued: true, clientRef: kept.clientRef });
            return chat.sendItem(kept);
        },
        newClientRef() { return "c" + Date.now().toString(36) + Math.random().toString(36).slice(2, 8); },
        retry(clientRef) {
            const item = chat.changeOutbox(clientRef, { status: "waiting", nextDate: 0, attempts: 0, error: null, createdDate: Date.now() });
            return item ? chat.sendItem(item) : Promise.resolve(null);
        },
        discard(clientRef) { chat.dropOutbox(clientRef); },
        sendItem(item) {
            if (chat.inFlight[item.clientRef]) return chat.inFlight[item.clientRef];
            const params = Object.assign({}, item.params, { chatRoomId: item.chatRoomId, clientRef: item.clientRef });
            chat.state.sending = Object.assign({}, chat.state.sending, { [item.clientRef]: true });
            const done = moqui.request(item.url, { method: "POST", params }).then((result) => {
                const json = (result && result.json) || {};
                if (json.event || json.eventSeq != null) {
                    chat.dropOutbox(item.clientRef);
                    chat.emit("sent", { clientRef: item.clientRef, chatRoomId: item.chatRoomId, threadRootEventSeq: item.threadRootEventSeq ?? null, json });
                    return { sent: true, json };
                }
                return chat.notSent(item, 200, json);
            }, (err) => chat.notSent(item, err && err.status, err && err.json));
            chat.inFlight[item.clientRef] = done;
            const clear = () => {
                delete chat.inFlight[item.clientRef];
                const sending = Object.assign({}, chat.state.sending);
                delete sending[item.clientRef];
                chat.state.sending = sending;
            };
            done.then(clear, clear);
            return done;
        },
        inFlight: {},
        // a send that did not go: tried again later, held, or failed with the server's message
        notSent(item, status, json) {
            const next = chat.classify(status);
            const attempts = Number(item.attempts || 0) + 1;
            const errors = json && json.errors ? (Array.isArray(json.errors) ? json.errors.join(" ") : String(json.errors)) : null;
            if (next === "retry") {
                chat.changeOutbox(item.clientRef, { attempts, nextDate: Date.now() + chat.backoff(attempts), status: "waiting" });
                return { queued: true, clientRef: item.clientRef };
            }
            if (next === "hold") {
                chat.changeOutbox(item.clientRef, { attempts, status: "held" });
                return { queued: true, clientRef: item.clientRef };
            }
            chat.changeOutbox(item.clientRef, { attempts, status: "failed", error: errors });
            return { failed: true, clientRef: item.clientRef, error: errors };
        },
        // Send what is due, oldest first and one at a time (force: what waits too, as when the browser or the socket is
        // back). Not while offline; not an item that failed or whose files expired.
        flush(force) {
            if (!chat.started || !chat.state.online || chat.flushing) return Promise.resolve();
            const now = Date.now();
            const due = chat.readOutbox().filter((item) => !chat.inFlight[item.clientRef] && item.status !== "failed" && !chat.outboxExpired(item, now)
                && (force || (item.status !== "held" && Number(item.nextDate || 0) <= now)));
            if (!due.length) return Promise.resolve();
            chat.flushing = true;
            const done = due.reduce((previous, item) => previous.then(() => (chat.state.online ? chat.sendItem(item) : null)), Promise.resolve());
            return done.then(() => { chat.flushing = false; }, () => { chat.flushing = false; });
        },

        // ---- presence ----

        // the dot of a user: the presence the store has, "" while it is not known (asked for by watchUsers)
        presenceOf(userId) {
            if (!userId) return "";
            if (userId === chat.me && chat.state.me) return chat.myPresence();
            const entry = chat.state.presence[userId];
            return entry ? DOT[entry.presence] || "offline" : "";
        },
        statusOf(userId) { return chat.state.presence[userId] || null; },
        // my own dot: what I chose (invisible is a grey dot), else what the server says
        myPresence() {
            const me = chat.state.me;
            if (!me) return "";
            if (me.status === "invisible") return "offline";
            if (me.status === "dnd" || me.status === "away") return me.status;
            return DOT[me.presence] === "offline" ? "online" : DOT[me.presence] || "online";
        },
        // do not disturb: no toasts and no chat sounds; the notifications are still recorded and counted
        quiet() { return !!chat.state.me && chat.state.me.status === "dnd"; },
        applyQuiet() {
            const client = chat.vm && chat.vm.notificationClient;
            if (!client) return;
            if (chat.quiet()) client.disableDisplay(); else client.enableDisplay();
        },
        setMe(myStatus) {
            chat.state.me = Object.assign({}, myStatus);
            if (myStatus.autoAwayMinutes) chat.autoAwayMinutes = myStatus.autoAwayMinutes;
            chat.state.presence[chat.me] = { presence: myStatus.presence, statusText: myStatus.statusText || null, emoji: myStatus.emoji || null,
                clearAtDate: myStatus.clearAtDate || null, inCall: !!myStatus.inCall, at: Date.now() };
            chat.applyQuiet();
        },
        // ask for these users' presence, once each in five minutes; the asks of the same moment go as one request
        watchUsers(userIds) {
            const now = Date.now();
            (userIds || []).forEach((userId) => {
                if (!userId || userId === chat.me) return;
                if (chat.watched[userId] && now - chat.watched[userId] < STATUS_TTL) return;
                chat.watched[userId] = now;
                chat.pendingUsers.push(userId);
            });
            if (!chat.pendingUsers.length || chat.watchTimer || !chat.vm) return;
            chat.watchTimer = setTimeout(() => {
                chat.watchTimer = null;
                const ids = chat.pendingUsers.splice(0, chat.pendingUsers.length).filter((userId) => userId !== chat.me);
                if (!ids.length) return;
                chat.request("/userStatuses", { userIds: ids.join(",") }).then((json) => {
                    const map = json && moqui.isPlainObject(json.statusMap) ? json.statusMap : {};
                    Object.keys(map).forEach((userId) => { chat.state.presence[userId] = Object.assign({ at: Date.now() }, map[userId]); });
                });
            }, 50);
        },
        // the account menu: a status, a status text with an emoji and a clear-at choice, or clearing the text
        setStatus(params) {
            const send = Object.assign({}, params);
            if (send.clearAt !== undefined) {
                const at = chat.clearAt(send.clearAt, Date.now());
                delete send.clearAt;
                if (at) send.clearAtDate = at;
            }
            return chat.request("/setUserStatus", send, "POST").then((json) => {
                if (json && json.myStatus) chat.setMe(json.myStatus);
                return json;
            });
        },
        openStatusDialog() { chat.state.statusDialog = true; },
        onInput() {
            const now = Date.now();
            const wasIdle = chat.idleSeconds(chat.lastInput, now);
            chat.lastInput = now;
            // back from away: beat now so the others see it at once
            if (chat.backFromIdle(Math.max(chat.lastBeatIdle, wasIdle), chat.autoAwayMinutes)) chat.beat();
        },
        beat() {
            if (!chat.vm) return;
            const idle = chat.idleSeconds(chat.lastInput, Date.now());
            chat.lastBeatIdle = idle;
            chat.request("/userStatusHeartbeat", { idleSeconds: idle }, "POST").then((json) => { if (json && json.myStatus) chat.setMe(json.myStatus); });
        }
    };
    moqui.chat = chat;

    // ---- the components the shell writes (registered by WebrootVue.qvt.js) ----

    const labelsOf = (vm) => Object.assign({}, chat.labels, vm.$root && vm.$root.labels);
    const components = moqui.qvtComponents = moqui.qvtComponents || {};

    // The status part of the account menu: the current status with its text, the four statuses, and the status message.
    components["m-user-status-menu"] = {
        name: "mUserStatusMenu",
        computed: {
            labels() { return labelsOf(this); },
            me() { return chat.state.me || {}; },
            current() { return this.me.status || "online"; },
            currentDot() { return chat.myPresence() || "offline"; },
            currentLabel() { return this.labels["status" + this.current.charAt(0).toUpperCase() + this.current.slice(1)] || this.current; },
            options() {
                return STATUSES.map((status) => ({ value: status, dot: DOT[status], label: this.labels["status" + status.charAt(0).toUpperCase() + status.slice(1)] }));
            }
        },
        methods: {
            choose(status) { chat.setStatus({ status }); },
            openDialog() { chat.openStatusDialog(); },
            clearText() { chat.setStatus({ clearStatus: true }); }
        },
        template:
            '<div class="m-user-status">' +
                '<q-item id="m-status-current" dense>' +
                    '<q-item-section avatar class="m-status-avatar"><span class="m-status-dot" :class="\'m-presence-\' + currentDot"></span></q-item-section>' +
                    '<q-item-section><q-item-label>{{currentLabel}}</q-item-label>' +
                        '<q-item-label v-if="me.statusText || me.emoji" caption class="m-status-text">{{me.emoji ? me.emoji + \' \' : \'\'}}{{me.statusText}}</q-item-label></q-item-section>' +
                '</q-item>' +
                '<q-item v-for="option in options" :key="option.value" dense clickable v-close-popup :active="current === option.value" :data-status="option.value" @click="choose(option.value)">' +
                    '<q-item-section avatar class="m-status-avatar"><span class="m-status-dot" :class="\'m-presence-\' + option.dot"></span></q-item-section>' +
                    '<q-item-section>{{option.label}}</q-item-section>' +
                '</q-item>' +
                '<q-item dense clickable v-close-popup data-status="message" @click="openDialog">' +
                    '<q-item-section avatar class="m-status-avatar"><q-icon name="sym_o_edit" size="xs"></q-icon></q-item-section>' +
                    '<q-item-section>{{labels.setStatus}}</q-item-section>' +
                '</q-item>' +
                '<q-item v-if="me.statusText || me.emoji" dense clickable v-close-popup data-status="clear" @click="clearText">' +
                    '<q-item-section avatar class="m-status-avatar"><q-icon name="sym_o_close" size="xs"></q-icon></q-item-section>' +
                    '<q-item-section>{{labels.clearStatus}}</q-item-section>' +
                '</q-item>' +
            '</div>'
    };

    // An agent's card in a room (content.assist): an approval with its changes (current → proposed) and Approve / Reject for
    // any member, a question with its options for the person who asked, a reply's View run for them; decided cards show who
    // decided. A chat screen writes <m-assist-card :event="event" :chat-room-id="roomId"> for a message with event.assist.
    components["m-assist-card"] = {
        name: "mAssistCard",
        props: { event: { type: Object, required: true }, chatRoomId: { type: String, required: true } },
        data() { return { busy: false, error: "", rejecting: false, feedback: "" }; },
        computed: {
            labels() { return labelsOf(this); },
            card() { return chat.assistCard(this.event, chat.me, this.labels); }
        },
        methods: {
            send(params) {
                this.busy = true;
                this.error = "";
                return chat.decideCard(this.chatRoomId, this.event.eventSeq, params).then((json) => {
                    this.busy = false;
                    if (json && json.error) this.error = String(json.error);
                    else { this.rejecting = false; this.feedback = ""; }
                });
            },
            approve() { return this.send({ decision: "approve" }); },
            // the requester's feedback is the agent's next task; another member's rejection carries none
            reject() {
                if (!this.card.mine) return this.send({ decision: "reject" });
                if (!this.rejecting) { this.rejecting = true; return null; }
                return this.send({ decision: "reject", feedback: this.feedback });
            },
            answer(option) { return this.send({ choice: option }); },
            viewRun() {
                const assist = root.moqui && root.moqui.assist;
                if (!assist || !this.card || !this.card.conversationId) return;
                assist.openDrawer();
                assist.load(this.card.conversationId);
            }
        },
        template:
            '<div v-if="card && card.kind !== \'reply\'" class="m-assist-card" :class="\'m-assist-\' + card.kind + \' m-assist-state-\' + card.state">' +
                '<div class="m-assist-card-title text-weight-medium">{{card.title}} <span class="m-assist-card-state text-caption">{{card.stateLabel}}</span></div>' +
                '<div v-if="card.summary" class="m-assist-card-summary">{{card.summary}}</div>' +
                '<table v-if="card.rows.length" class="m-assist-card-rows"><tr v-for="row in card.rows" :key="row.label">' +
                    '<th>{{row.label}}</th><td class="m-assist-current">{{row.current}}</td><td>→</td><td class="m-assist-proposed">{{row.proposed}}</td></tr></table>' +
                '<div v-if="card.askedBy" class="m-assist-card-asked text-caption">{{card.askedBy}}</div>' +
                '<div v-if="card.canDecide" class="m-assist-card-actions">' +
                    '<q-input v-if="rejecting" v-model="feedback" dense outlined autogrow :label="labels.assistFeedback" class="m-assist-feedback"></q-input>' +
                    '<q-btn unelevated no-caps dense color="primary" :label="labels.assistApprove" :loading="busy" class="m-assist-approve" @click="approve"></q-btn>' +
                    '<q-btn flat no-caps dense :label="labels.assistReject" :disable="busy" class="m-assist-reject" @click="reject"></q-btn>' +
                '</div>' +
                '<div v-if="card.canAnswer" class="m-assist-card-actions">' +
                    '<q-btn v-for="option in card.options" :key="option" outline no-caps dense :label="option" :disable="busy" class="m-assist-option" @click="answer(option)"></q-btn>' +
                '</div>' +
                '<div v-if="card.waiting" class="m-assist-card-waiting text-caption">{{card.waiting}}</div>' +
                '<div v-if="card.decided" class="m-assist-card-decided text-caption">{{card.decided}}</div>' +
                '<div v-if="error" class="m-assist-card-error text-negative text-caption">{{error}}</div>' +
            '</div>' +
            '<div v-else-if="card && card.canViewRun" class="m-assist-card-run"><q-btn flat no-caps dense size="sm" :label="labels.assistViewRun" class="m-assist-view-run" @click="viewRun"></q-btn></div>'
    };

    // The status message dialog of the account menu, outside the menu so it outlives it.
    components["m-user-status-dialog"] = {
        name: "mUserStatusDialog",
        data() { return { text: "", emoji: "", clear: 0, saving: false }; },
        computed: {
            labels() { return labelsOf(this); },
            open: { get() { return chat.state.statusDialog; }, set(value) { chat.state.statusDialog = !!value; } },
            clearOptions() { return chat.clearOptions(this.labels); },
            // read when the dialog opens: the preference may have changed since (X-Moqui-Preferences)
            presets() {
                if (!this.open) return [];
                const config = (root.moqui && root.moqui.config) || {};
                const l10n = root.moqui && root.moqui.l10n;
                return chat.statusPresets(config.preferences, l10n && l10n.localize ? (text) => l10n.localize(text) : null);
            }
        },
        watch: {
            open(value) {
                if (!value) return;
                const me = chat.state.me || {};
                this.text = me.statusText || "";
                this.emoji = me.emoji || "";
                this.clear = 0;
            }
        },
        methods: {
            // a preset fills the dialog; Save sets it
            usePreset(preset) {
                this.emoji = preset.emoji;
                this.text = preset.text;
                this.clear = preset.clearAfter;
            },
            clearLabel(value) {
                const found = this.clearOptions.find((option) => option.value === value);
                return found && value ? found.label : "";
            },
            save() {
                this.saving = true;
                const params = { statusText: this.text, emoji: this.emoji };
                if (this.clear) params.clearAt = this.clear;
                else params.clearAfterMinutes = 0;
                chat.setStatus(params).then(() => { this.saving = false; this.open = false; });
            }
        },
        template:
            '<q-dialog v-model="open">' +
                '<q-card style="min-width:320px;max-width:96vw;">' +
                    '<q-card-section class="text-subtitle1">{{labels.setStatus}}</q-card-section>' +
                    '<q-card-section class="q-pt-none q-gutter-y-sm">' +
                        '<q-list v-if="presets.length" dense class="m-status-presets">' +
                            '<q-item v-for="(preset, index) in presets" :key="index" clickable dense :data-preset="index" @click="usePreset(preset)">' +
                                '<q-item-section avatar style="min-width:28px;">{{preset.emoji}}</q-item-section>' +
                                '<q-item-section>{{preset.text}}</q-item-section>' +
                                '<q-item-section side class="text-caption">{{clearLabel(preset.clearAfter)}}</q-item-section>' +
                            '</q-item>' +
                        '</q-list>' +
                        '<div class="row no-wrap q-gutter-x-sm">' +
                            '<q-input v-model="emoji" dense outlined stack-label :label="labels.emoji" maxlength="16" style="width:84px;" name="emoji"></q-input>' +
                            '<q-input v-model="text" dense outlined stack-label autofocus class="col" :label="labels.statusText" maxlength="100" name="statusText" @keyup.enter="save"></q-input>' +
                        '</div>' +
                        '<q-select v-model="clear" :options="clearOptions" emit-value map-options dense outlined stack-label :label="labels.clearAfter" name="clearAt"></q-select>' +
                    '</q-card-section>' +
                    '<q-card-actions align="right">' +
                        '<q-btn flat no-caps :label="labels.cancel" v-close-popup></q-btn>' +
                        '<q-btn unelevated no-caps color="primary" :label="labels.save" :loading="saving" id="m-status-save" @click="save"></q-btn>' +
                    '</q-card-actions>' +
                '</q-card>' +
            '</q-dialog>'
    };

    if (typeof module !== "undefined" && module.exports) module.exports = chat;
})(globalThis);
