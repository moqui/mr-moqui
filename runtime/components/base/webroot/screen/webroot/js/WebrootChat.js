/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * Chat and presence in the /qapps shell (IronStone design-drawers-commons §1.8, §13; impl-drawers-commons M10): moqui.chat, a
 * small store over the notification socket, and the account menu's status. It keeps
 *   - presence: the presence and status of the users the page shows (m-avatar with a user-id draws the dot from it),
 *   - me: the current user's own status, set from the account menu, with the heartbeat that keeps it (auto-away from idle),
 *   - unread: each room's unread and mention counts, typing: who is typing in each room, reads: the members' read markers,
 *   - outbox: the messages sent and not yet answered, kept in localStorage per user (moqui.chat.outbox.<userId>) so a
 *     send made offline, or one that failed for a while, goes when the page is back (chat.send, chat.flush); in the
 *     desktop client the host keeps and sends it (moqui.hostOutbox(), crates/moqui-desktop/src/client/outbox.rs), also
 *     with no window open, and the page shows its items and hears of each change; with a server that takes them once by
 *     a clientRef (clientApi 4), a message's edit, deletion, reaction and poll vote wait there too (chat.queueChange,
 *     chat.state.changes),
 * and hands the chat pushes to the screens that listen (moqui.chat.on). The server side is org.moqui.chat.ChatServices; the
 * topics and their messages are described there and in docs/ui/Qvt-Client.md, "Chat store and presence".
 *
 * Loaded after MoquiLib.js and before WebrootVue.qvt.js (qapps.xml): its components are in moqui.qvtComponents, which
 * WebrootVue.qvt.js registers, and the shell root calls moqui.chat.start(root) once mounted. Runs under Node for its tests
 * (WebrootChat.test.js): nothing here touches the page until start.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui;
    const L = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);
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
    // the host outbox's states as the page's items show them
    const HOST_STATUS = { queued: "waiting", retry: "waiting", sending: "sending", held: "held", failed: "failed", conflict: "failed" };
    // a message's changes the host's outbox keeps with a server at clientApi 4 (moqui_client::sync::outbox MESSAGE_KINDS)
    const CHANGE_KINDS = ["chat-edit", "chat-redact", "chat-react", "poll-vote"];

    const chat = {
        labels: { statusOnline: "Online", statusAway: "Away", statusDnd: "Do not disturb", statusInvisible: "Invisible", statusOffline: "Offline",
            setStatus: "Set a status message", clearStatus: "Clear status message", statusText: "What's your status?", emoji: "Emoji",
            clearAfter: "Clear after", dontClear: "Don't clear", minutes30: "30 minutes", hour1: "1 hour", hours4: "4 hours", today: "Today",
            thisWeek: "This week", save: "Save", cancel: "Cancel", status: "Status", encryptedMessage: "Encrypted message",
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
                // a reactor is a local user, or an external actor of a bridged room (no userId)
                const who = event.senderUserId || event.senderActorId;
                if (entry.userIds.indexOf(who) < 0) {
                    entry.userIds.push(who);
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
                        const index = entry.userIds.indexOf(event.senderUserId || event.senderActorId);
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
        // An operation of the host's outbox (outbox_list) as an item of the page's outbox, with its status.
        itemOfOp(op) {
            return Object.assign({}, op.payload, { clientRef: op.id, createdDate: op.createdMs, attempts: op.attempts || 0, nextDate: op.nextMs || 0,
                status: HOST_STATUS[op.state] || "waiting", error: op.error || null });
        },

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
                // events gone at their time (disappearing messages, plans/Comms-Messaging.md §18.3 phase 16; WebrootChatExpiry.js)
                if (message.kind === "expired") {
                    return { kind: "expired", chatRoomId: roomId, eventSeqList: Array.isArray(message.eventSeqList) ? message.eventSeqList.map(Number) : [],
                        before: message.before != null ? Number(message.before) : null };
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
                // the join requests waiting, for the room's moderators (Comms-Messaging §18.3 phase 17)
                if (message.kind === "joinRequests") return { kind: "joinRequests", chatRoomId: roomId, pendingCount: Number(message.pendingCount || 0) };
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
                    // the user's own settings of the room changed (here or on another device): its notifications follow
                    if (message.action === "settings" && message.chatRoomId && state.levels) {
                        state.levels[message.chatRoomId] = chat.levelOf(message.notificationLevelEnumId, !!message.isMuted, message.filtered);
                    }
                    // a room gone to Spam & blocked counts nothing
                    if (message.action === "settings" && message.chatRoomId && message.filtered) state.unread[message.chatRoomId] = { unread: 0, mentions: 0 };
                    return { kind: "room", chatRoomId: message.chatRoomId, action: message.action };
                }
                // the user's scheduled messages or reminders changed (ChatSchedule.js told): read them again
                if (message.kind === "schedule") return { kind: "schedule", chatRoomId: message.chatRoomId || null, what: message.what || null };
            }
            return null;
        },
        // the users still typing in a room
        typingUsers(state, roomId, now) {
            const typing = state.typing[roomId] || {};
            return Object.keys(typing).filter((userId) => typing[userId] > now);
        },
        // The server's counts (get#UnreadCounts roomCounts) in the store: each room it names, and a room it no longer names
        // (read elsewhere, or out of sight in Spam & blocked) at nothing.
        applyCounts(state, roomCounts) {
            const counts = moqui.isPlainObject(roomCounts) ? roomCounts : {};
            Object.keys(state.unread).forEach((roomId) => { if (!counts[roomId]) state.unread[roomId] = { unread: 0, mentions: 0 }; });
            Object.keys(counts).forEach((roomId) => { state.unread[roomId] = { unread: counts[roomId].unread || 0, mentions: counts[roomId].mentions || 0 }; });
        },
        // A timeline row of a message of someone the user blocked (event.hidden): one quiet line for a run of them, added to
        // out unless the last row is that line. False for any other event.
        hiddenLine(out, event) {
            if (!event || !event.hidden) return false;
            const last = out[out.length - 1];
            if (!last || last.kind !== "hidden") out.push({ kind: "hidden", key: "h" + event.eventSeq, eventSeq: event.eventSeq });
            return true;
        },
        // the room's unread counts and the total of every room
        unreadOf(state, roomId) { return state.unread[roomId] || { unread: 0, mentions: 0 }; },
        unreadTotal(state) { return Object.keys(state.unread).reduce((sum, roomId) => sum + (state.unread[roomId].unread || 0), 0); },

        // ---- the store of the page ----

        state: reactive({ me: null, presence: {}, unread: {}, levels: {}, roomNames: {}, typing: {}, reads: {}, openRooms: {}, statusDialog: false, outbox: [], sending: {},
            online: true, connected: null, changes: [], hostKinds: [] }),
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
            // the user's own status first: the catch-up below decides on do not disturb by it
            const meLoaded = chat.request("/userStatus").then((json) => { if (json && json.myStatus) chat.setMe(json.myStatus); });
            const readCounts = () => Promise.all([chat.request("/chatUnread"), meLoaded]).then(([json]) => {
                const counts = json && moqui.isPlainObject(json.roomCounts) ? json.roomCounts : {};
                if (json) chat.applyCounts(chat.state, counts);
                if (json && moqui.isPlainObject(json.levelMap)) chat.state.levels = Object.assign({}, json.levelMap);
                if (json && moqui.isPlainObject(json.roomNameMap)) chat.state.roomNames = Object.assign({}, json.roomNameMap);
                // in an app: what was missed while it was closed or offline (§7.5)
                if (json) chat.notifyMissed(counts);
            });
            chat.readCounts = readCounts;
            readCounts();
            if (client && client.onState) {
                let closed = false;
                client.onState((socketState) => { if (socketState === "closed") closed = true; else if (socketState === "open" && closed) { closed = false; readCounts(); } });
            }
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
            // (the desktop client's server out of reach is offline too: moqui.connection, from its answers)
            const browserOnline = () => !(root.navigator && root.navigator.onLine === false);
            const reachable = () => !(moqui.connection && moqui.connection.offline);
            chat.state.online = browserOnline() && reachable();
            // the desktop client's host keeps and sends the outbox: the page shows it and stops its own sending
            const host = chat.host();
            if (host) chat.startHost(host);
            else chat.readOutbox();
            if (host && moqui.onConnection) moqui.onConnection((offline) => { chat.state.online = !offline && browserOnline(); chat.touchOutbox(); if (!offline) chat.readKinds(host); });
            if (root.addEventListener) {
                root.addEventListener("online", () => { chat.state.online = reachable(); chat.flush(true); });
                root.addEventListener("offline", () => { chat.state.online = false; chat.touchOutbox(); });
                if (!host) root.addEventListener("storage", (event) => { if (event && event.key === chat.outboxKey()) { chat.readOutbox(); chat.emit("outbox", {}); } });
            }
            if (client && client.onState) client.onState((socketState) => {
                const reconnected = socketState === "open" && chat.state.connected === false;
                chat.state.connected = socketState === "open";
                // the host's socket to the server is open: the server can be reached
                if (socketState === "open" && host && moqui.setOffline) moqui.setOffline(false);
                if (socketState === "open") chat.flush(true);
                chat.emit("connection", { state: socketState, reconnected });
            });
            if (host) return;
            chat.flushTimer = setInterval(() => chat.flush(false), FLUSH_EVERY);
            chat.flush(true);
        },
        // the host's outbox (the desktop client), null on the web
        host() { return moqui.hostOutbox ? moqui.hostOutbox() : null; },
        // Show the host's outbox and hear its changes: a chat send answered is told as the page's own (sent). An outbox this
        // browser kept is dropped first, never sent (dropBrowserOutbox).
        startHost(host) {
            chat.hostOff = host.on((event) => {
                if (event && event.kind === "sent" && event.op === "chat-send") {
                    chat.emit("sent", { clientRef: event.clientRef, chatRoomId: event.chatRoomId, threadRootEventSeq: event.threadRootEventSeq ?? null,
                        json: event.json || {} });
                }
                // a message's change answered: its room and message (the host's event names them), what this page noted of it
                // (a reaction's key), and the answer
                if (event && event.kind === "sent" && CHANGE_KINDS.includes(event.op)) {
                    chat.emit("changed", { kind: event.op, clientRef: event.clientRef, chatRoomId: event.chatRoomId || null,
                        eventSeq: event.eventSeq != null ? Number(event.eventSeq) : null, note: chat.changeNotes[event.clientRef] || null, json: event.json || {} });
                    delete chat.changeNotes[event.clientRef];
                }
                // the changes feeds changed the held messages of rooms (or were reset): a room shown reads them again
                if (event && event.kind === "rooms") {
                    chat.emit("rooms", { rooms: Array.isArray(event.rooms) ? event.rooms.map(String) : [], reset: !!event.reset });
                    return;
                }
                chat.refreshHost();
            });
            chat.readKinds(host);
            chat.dropBrowserOutbox();
            return chat.refreshHost();
        },
        // the kinds the host's outbox takes for the account's server (outbox.kinds(): {clientApi, kinds, internalSend}); with
        // none known, a message's changes are asked of the server as on the web
        readKinds(host) {
            if (!host || typeof host.kinds !== "function") return Promise.resolve();
            return Promise.resolve(host.kinds()).then((answer) => {
                chat.state.hostKinds = answer && Array.isArray(answer.kinds) ? answer.kinds.map(String) : [];
            }, (err) => console.info("chat: the app's outbox kinds were not read", err));
        },
        refreshHost() {
            const host = chat.host();
            if (!host) return Promise.resolve();
            return host.list().then((ops) => {
                const list = Array.isArray(ops) ? ops : [];
                chat.state.outbox = list.filter((op) => op.kind === "chat-send").map((op) => chat.itemOfOp(op));
                const changes = list.filter((op) => CHANGE_KINDS.includes(op.kind)).map((op) => chat.changeOfOp(op));
                // one this page is adding, which a list read before the host had it does not have yet
                const listed = new Set(changes.map((change) => change.clientRef));
                chat.state.changes = changes.concat(Object.values(chat.addingChanges).filter((change) => !listed.has(change.clientRef)));
                chat.emit("outbox", {});
            }, (err) => console.info("chat outbox not read from the app", err));
        },
        // A message's change waiting in the host's outbox (chat-edit, chat-redact, chat-react, poll-vote) as the screens show
        // it under its message: {clientRef, kind, chatRoomId, eventSeq, status, error, rejected, createdDate}.
        // Also a vote's optionIds (numbers), and a reaction's key: the payload's, or for an encrypted one (its key only as
        // ciphertext in the outbox) what this page noted when it queued it.
        changeOfOp(op) {
            const payload = op.payload || {};
            const note = chat.changeNotes[op.id] || null;
            const options = payload.optionIds == null ? null : String(payload.optionIds).split(",").map((id) => id.trim()).filter((id) => id).map(Number);
            return { clientRef: op.id, kind: op.kind, chatRoomId: payload.chatRoomId || null, eventSeq: Number(payload.eventSeq) || null,
                status: HOST_STATUS[op.state] || "waiting", error: op.error || null, rejected: !!op.rejected, createdDate: op.createdMs || 0,
                key: payload.key || (note && note.key) || null, optionIds: options, note };
        },
        // what this page noted of the changes it queued, by clientRef (in this page only)
        changeNotes: {},
        // the changes this page is adding to the host's outbox, until it has them
        addingChanges: {},
        // the changes of a room's message still waiting, oldest first
        changesOf(roomId, eventSeq) {
            return chat.state.changes.filter((change) => change.chatRoomId === roomId && change.eventSeq === Number(eventSeq))
                .sort((left, right) => left.createdDate - right.createdDate);
        },
        // A message's change through the host's outbox (the desktop client, with a server that takes it once by its clientRef):
        // kind is chat-edit, chat-redact, chat-react or poll-vote, payload its fields with chatRoomId and eventSeq. Resolves to
        // {queued: true, clientRef} once the host keeps it (shown under its message until it is answered); null when the host
        // does not take the kind (the web, a server below clientApi 4), where the screen asks the server itself.
        queueChange(kind, payload, note) {
            const host = chat.host();
            if (!host || !CHANGE_KINDS.includes(kind) || !chat.state.hostKinds.includes(kind)) return null;
            const clientRef = chat.newClientRef();
            if (note) chat.changeNotes[clientRef] = note;
            const change = chat.changeOfOp({ id: clientRef, kind, state: "queued", payload, createdMs: Date.now() });
            // shown at once; the host's list follows
            chat.state.changes = chat.state.changes.concat([change]);
            chat.addingChanges[clientRef] = change;
            return host.add(kind, Object.assign({ clientRef }, payload)).then(() => {
                delete chat.addingChanges[clientRef];
                chat.refreshHost();
                return { queued: true, clientRef };
            }, (err) => {
                delete chat.addingChanges[clientRef];
                chat.state.changes = chat.state.changes.filter((item) => item.clientRef !== clientRef);
                delete chat.changeNotes[clientRef];
                throw err;
            });
        },
        // An outbox this browser kept (localStorage, under the scope) is dropped, never moved into the host: in the apps every
        // server's pages share one origin, so another server's page may have written it under this account's key, and the
        // host would send it as this user (plans/Security-Review-Medium.md SEC-CLI-81). The host's outbox is the one the
        // apps send from. Answers how many items there were.
        dropBrowserOutbox() {
            const key = chat.outboxKey();
            let count = 0;
            try {
                const text = root.localStorage ? root.localStorage.getItem(key) : null;
                const parsed = text ? JSON.parse(text) : [];
                count = Array.isArray(parsed) ? parsed.length : 0;
                if (text !== null) root.localStorage.removeItem(key);
            } catch (err) { /* no storage, or not a list: nothing to drop */ }
            if (count) console.warn("chat outbox of this browser dropped, not sent: " + count + " items");
            return count;
        },
        // A room read up to seq, through the host's outbox (sent when it can, the room shown read at once in the stored
        // copy): resolves as markRead answers; null on the web or with no seq, where the screen asks the server itself.
        queueRead(roomId, seq) {
            const host = chat.host();
            if (!host || !roomId || seq == null || !(Number(seq) >= 0)) return null;
            return host.add("chat-read", { chatRoomId: roomId, lastReadEventSeq: Number(seq) }).then(() => ({ lastReadEventSeq: Number(seq) }));
        },
        noop() {},
        // a GET or POST to a webroot transition of the store, its JSON (null when it fails, which is logged)
        request(path, params, method) {
            const base = (chat.vm && chat.vm.appRootPath) || "";
            return moqui.request(base + path, { method: method || "GET", params }).then((result) => result.json || null,
                (err) => { console.info("chat " + path + " failed", err && err.status); return null; });
        },
        receive(jsonObj) {
            // a message over the host's socket: the server is there
            if (jsonObj && moqui.connection && moqui.connection.offline && chat.host() && moqui.setOffline) moqui.setOffline(false);
            const done = chat.reduce(chat.state, jsonObj, chat.me, Date.now());
            if (!done) return;
            // my message arrived: it waits no more (it was sent from another tab, or its answer was lost)
            if (done.kind === "event" && done.event.clientRef && done.event.senderUserId === chat.me) chat.dropOutbox(done.event.clientRef);
            // a disappearing message is held for the page's timer (WebrootChatExpiry.js)
            if (done.kind === "event" && chat.expiry) chat.expiry.pushed(done.chatRoomId, done.event);
            if (done.kind === "presence" && done.userId === chat.me) {
                // do not disturb ended: a summary of what came meanwhile, as the server's pushes have (rule 4)
                const quiet = chat.quiet();
                if (chat.wasQuiet && !quiet && chat.readCounts) chat.readCounts();
                chat.wasQuiet = quiet;
                chat.applyQuiet();
            }
            // a room joined: its level and name (a room of the session takes the user's default until then)
            if (done.kind === "room" && (done.action === "joined" || done.action === "added") && chat.readCounts) chat.readCounts();
            chat.notifyApp(done);
            chat.emit(done.kind, done);
        },

        // ---- the app's own notifications (plans/Comms-Messaging.md §7.5) ----

        // A room's notification level as the server keeps it (CnlAll, CnlMentions, CnlNone, or their codes), muted: all,
        // mentions (a muted room) or none; none for a room out of the user's sight (filtered: spam or blocked,
        // plans/Comms-Messaging.md §18.3 phase 11).
        levelOf(level, muted, filtered) {
            if (filtered) return "none";
            const code = { CnlAll: "all", CnlMentions: "mentions", CnlNone: "none" }[level] || level || "all";
            if (code === "none") return "none";
            return muted ? "mentions" : (code === "mentions" ? "mentions" : "all");
        },
        // Room pushes carry the viewer's own reply qualification. Timeline-shaped events may carry the replied-to sender.
        mentionsOrRepliesTo(event, me) {
            return !!me && (event.replyForViewer === true || (event.mentions || []).includes(me)
                || (event.replyTo && event.replyTo.senderUserId === me)
                || (event.threadRootEventSeq != null && event.threadRootSenderUserId === me));
        },
        // What the desktop app (window.moquiDesktop.notify) shows of a socket event, by the rules of the server's pushes
        // (§7.2): a message from someone else, by the room's level (levels: chatRoomId to all, mentions or none, a muted
        // room is mentions; /chatUnread levelMap; a room not in it, joined since, at the user's default, defaultLevel),
        // where a mention, a reply to the user and a reply in the user's thread count at the mentions level; a missed call
        // to the user and a voicemail as messages unless the level is none (rule 6); not on do not disturb (quiet), not
        // while its room shows in a window the user sees (seen); no text for an encrypted room or with previews off; a
        // group room's name (roomNames) is its title, with the sender before the text. {title, body, tag, link}, or null.
        notification(event, roomId, options) {
            const given = options || {};
            if (!event || given.quiet || given.seen) return null;
            const level = (given.levels || {})[roomId] || given.defaultLevel || "all";
            if (level === "none") return null;
            const tr = given.localize || ((text) => text);
            const appRoot = given.appRoot || "/qapps/BasicComms";
            const link = appRoot + "/Chat?chatRoomId=" + encodeURIComponent(roomId) + (event.eventSeq ? "&eventSeq=" + event.eventSeq : "");
            const roomName = (given.roomNames || {})[roomId] || null;
            if (event.type === "call") {
                const content = moqui.isPlainObject(event.content) ? event.content : {};
                const missed = content.action === "ended" && (content.missedUserIdList || []).indexOf(given.me) >= 0;
                const voicemail = content.action === "voicemail" && event.senderUserId !== given.me;
                if (!missed && !voicemail) return null;
                return { title: String(roomName || event.senderName || tr("Missed call")), body: missed ? tr("Missed call") : tr("Voicemail"),
                    tag: "chat-" + roomId, link };
            }
            if (event.type !== "message" || !event.senderUserId && !event.senderActorId) return null;
            if (event.senderUserId && event.senderUserId === given.me) return null;
            if (level === "mentions" && !chat.mentionsOrRepliesTo(event, given.me)) return null;
            const text = event.encrypted ? null : String(event.bodyText || "").replace(/\s+/g, " ").trim();
            const body = event.encrypted ? tr("New message") : given.preview === false ? null : (text ? text.slice(0, 140) : null);
            const sender = String(event.senderName || tr("New message"));
            if (roomName) return { title: roomName, body: body ? sender + ": " + body : sender, tag: "chat-" + roomId, link };
            return { title: sender, body, tag: "chat-" + roomId, link };
        },
        // The rooms whose unread messages the app missed while it was closed, offline or on do not disturb (counted, the
        // server's count by level) beyond what it showed (shown: chatRoomId to the count shown or seen): [{chatRoomId,
        // count}], the most first. Only a count that went up shows again.
        missed(roomCounts, shown) {
            const out = [];
            Object.keys(roomCounts || {}).forEach((roomId) => {
                const counted = Number((roomCounts[roomId] || {}).counted) || 0;
                if (counted > 0 && counted > Number((shown || {})[roomId] || 0)) out.push({ chatRoomId: roomId, count: counted });
            });
            return out.sort((a, b) => b.count - a.count);
        },
        // At most this many missed rooms are shown one by one; more are one summary.
        MISSED_MAX: 5,
        // the app's notify and clear, when the page runs in one
        notifyHost() {
            const host = root.moquiDesktop;
            return host && host.notify ? host : null;
        },
        // What this user's app has shown of each room (chatRoomId to the unread count then), per account (its storage
        // scope) and user, in local storage: the catch-up shows only what went up since.
        shownKey(host) { return "moqui.chat.notified." + String((host && host.storageScope) || "") + "." + chat.me; },
        readShown(host) {
            try { return JSON.parse(root.localStorage.getItem(chat.shownKey(host)) || "{}") || {}; } catch (err) { return {}; }
        },
        writeShown(host, shown) {
            try { root.localStorage.setItem(chat.shownKey(host), JSON.stringify(shown)); } catch (err) { /* no storage: shown again next time */ }
        },
        notifyApp(done) {
            const host = chat.notifyHost();
            if (!host) return;
            if (done.kind === "read" && done.userId === chat.me) {
                // read here or on another device: the room's notification goes (rule 3)
                const rooms = done.all ? Object.keys(chat.notifiedRooms) : done.chatRoomId ? [done.chatRoomId] : [];
                // every room read (Mark all read): the app's notifications of the account all go, a phone's from its
                // pushes too, which this page never showed
                if (done.all && host.notifyClear) Promise.resolve(host.notifyClear("chat-*")).catch(() => null);
                const shown = chat.readShown(host);
                rooms.forEach((roomId) => {
                    delete chat.notifiedRooms[roomId];
                    delete shown[roomId];
                    if (host.notifyClear) Promise.resolve(host.notifyClear("chat-" + roomId)).catch(() => null);
                });
                chat.writeShown(host, shown);
                return;
            }
            // a phone's chat notifications come by its pushes (pushDelivers, §7.2 rule 7), the desktop's from the socket
            if (done.kind !== "event" || host.pushDelivers) return;
            const prefs = (moqui.config && moqui.config.preferences) || {};
            const seen = !!chat.state.openRooms[done.chatRoomId] && (host.windowSeen ? host.windowSeen() : !(root.document && root.document.hidden));
            const shown = chat.notification(done.event, done.chatRoomId, { me: chat.me, levels: chat.state.levels, quiet: chat.quiet(), seen,
                defaultLevel: chat.levelOf(prefs.CHAT_NOTIFY_DEFAULT, false), roomNames: chat.state.roomNames,
                preview: prefs.CHAT_NOTIFY_PREVIEW !== "N" && done.event.expireDate == null, appRoot: moqui.call && moqui.call.appRoot ? moqui.call.appRoot() : null,
                localize: (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text) });
            if (!shown) return;
            chat.notifiedRooms[done.chatRoomId] = true;
            // the catch-up after a restart counts this one as shown
            const kept = chat.readShown(host);
            kept[done.chatRoomId] = Math.max(Number(kept[done.chatRoomId]) || 0, (chat.state.unread[done.chatRoomId] || {}).unread || 1);
            chat.writeShown(host, kept);
            Promise.resolve(host.notify(shown)).catch((err) => console.info("The app's notification was not shown", err));
        },
        // the rooms this page notified, which a read of all rooms clears
        notifiedRooms: {},
        // On start, when the socket comes back and when do not disturb ends: the rooms with unread messages the app
        // missed (their counts went up since it last showed them), one notification each, at most MISSED_MAX, else one
        // summary. Not on a phone (its pushes do it) and not while do not disturb.
        notifyMissed(roomCounts) {
            const host = chat.notifyHost();
            if (!host || host.pushDelivers || chat.quiet()) return;
            const shown = chat.readShown(host);
            const tr = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);
            const appRoot = moqui.call && moqui.call.appRoot ? moqui.call.appRoot() : "/qapps/BasicComms";
            const missed = chat.missed(roomCounts, shown);
            const names = chat.state.roomNames || {};
            const one = (room) => {
                const count = room.count === 1 ? tr("1 new message") : tr("${count} new messages").replace("${count}", String(room.count));
                const title = names[room.chatRoomId] ? names[room.chatRoomId] : count;
                Promise.resolve(host.notify({ title, body: names[room.chatRoomId] ? count : null, tag: "chat-" + room.chatRoomId,
                    link: appRoot + "/Chat?chatRoomId=" + encodeURIComponent(room.chatRoomId) })).catch(() => null);
                chat.notifiedRooms[room.chatRoomId] = true;
            };
            if (missed.length > chat.MISSED_MAX) {
                const total = missed.reduce((sum, room) => sum + room.count, 0);
                Promise.resolve(host.notify({ title: tr("${count} new messages").replace("${count}", String(total)),
                    body: tr("in ${rooms} rooms").replace("${rooms}", String(missed.length)), tag: "chat-missed", link: appRoot + "/Chat" }))
                    .catch(() => null);
            } else {
                missed.forEach(one);
            }
            missed.forEach((room) => { shown[room.chatRoomId] = room.count; });
            Object.keys(shown).forEach((roomId) => { if (!roomCounts[roomId]) delete shown[roomId]; });
            chat.writeShown(host, shown);
        },
        // listeners of the screens: kind is event, update, typing, read, room or presence; outbox (the outbox changed), sent
        // ({clientRef, chatRoomId, threadRootEventSeq, json}: a message of the outbox was answered), changed ({kind, clientRef,
        // chatRoomId, eventSeq, note, json}: a message's change in the host's outbox was answered), rooms ({rooms, reset}: the
        // host's changes feeds changed those rooms' held messages, or were reset) and connection ({state, reconnected});
        // returns the function that removes it
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
            // the phone's pushes of the room show nothing while it is in front (§7.5)
            const host = root.moquiDesktop;
            if (host && host.roomShown) host.roomShown(roomId);
        },
        closeRoom(roomId) {
            if (!chat.state.openRooms[roomId]) return;
            if (--chat.state.openRooms[roomId] <= 0) {
                delete chat.state.openRooms[roomId];
                chat.releaseFiles(roomId);
                const host = root.moquiDesktop;
                if (host && host.roomShown) host.roomShown(Object.keys(chat.state.openRooms)[0] || null);
            }
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
                .then((result) => (result && result.json) || {}, (err) => ({ error: (err && err.json && err.json.errors) || L(chat.labels.assistNoDecide) }));
        },

        // ---- encrypted rooms (C7; moqui.crypto) ----
        // A room whose encrypted is true takes only ciphertext: sealMessage and sealReaction encrypt before anything is sent or
        // kept (the outbox keeps the ciphertext, and the text it shows while it waits only in this page), and openEvents and
        // openEvent decrypt what the timeline and the pushes give, before applyEvent, into the fields a plain event has (body,
        // bodyText, attachments, reactions, a reply's snippet). Decrypted HTML is cleaned by DOMPurify. Files are encrypted
        // before upload (sealFile) and decrypted into blob URLs (openFile).

        sealedTexts: {},
        opened: {},
        reactionKeys: {},
        fileUrls: {},
        fileRooms: {},
        purifyLoad: null,
        mentionsIn(html) {
            return Array.from(new Set(Array.from(String(html || "").matchAll(/data-mention-id="([^"]*)"/g)).map((match) => match[1]).filter((id) => id)));
        },
        plainText(html) {
            const doc = root.document;
            if (!doc) return String(html || "").replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim();
            const div = doc.createElement("div");
            div.innerHTML = String(html || "").replace(/<br\s*\/?>/gi, "\n").replace(/<\/(p|li|div|h[1-6]|blockquote|pre)>/gi, "$&\n");
            return String(div.textContent || "").replace(/[ \t]+/g, " ").replace(/\n{3,}/g, "\n\n").trim();
        },
        loadPurify() {
            return chat.purifyLoad || (chat.purifyLoad = moqui.loadScript("/libs/dompurify/purify.min.js", null, () => !!root.DOMPurify)
                .catch((err) => { chat.purifyLoad = null; throw err; }));
        },
        // Decrypted HTML as the room shows it: what the server lets into a plain room's message (allow-html="safe", ammonia's
        // defaults and the mention and record spans) and no more. No script, style, class, id, form, frame, media, SVG or
        // MathML, and no event attribute, so a member cannot put markup in an encrypted room that a plain room would refuse
        // (a class of the page's own CSS could lay a link over the whole page).
        purifyConfig: {
            ALLOWED_TAGS: ["a", "abbr", "acronym", "article", "aside", "b", "bdi", "bdo", "blockquote", "br", "caption", "center", "cite", "code", "col",
                "colgroup", "data", "dd", "del", "details", "dfn", "div", "dl", "dt", "em", "figcaption", "figure", "footer", "h1", "h2", "h3", "h4",
                "h5", "h6", "header", "hgroup", "hr", "i", "img", "ins", "kbd", "li", "mark", "nav", "ol", "p", "pre", "q", "rp", "rt", "rtc", "ruby",
                "s", "samp", "small", "span", "strike", "strong", "sub", "summary", "sup", "table", "tbody", "td", "tfoot", "th", "thead", "time",
                "tr", "tt", "u", "ul", "var", "wbr"],
            ALLOWED_ATTR: ["lang", "title", "href", "hreflang", "dir", "cite", "datetime", "align", "char", "charoff", "span", "size", "width", "height",
                "alt", "src", "start", "summary", "colspan", "rowspan", "headers", "data-mention-id", "data-record-entity", "data-record-id"],
            ALLOW_DATA_ATTR: false, ALLOW_ARIA_ATTR: false,
        },
        // An image a message may show: only this server's rich-text image route, as the server's chat cleaner keeps
        // (StringUtilities.cleanChatHtml, SEC-COM-02). Another host's image would tell its sender each reader's address
        // and when they read.
        ownImage(src) {
            return /^\/richTextImage\/[A-Za-z0-9_-]+$/.test(String(src || ""));
        },
        clean(html) {
            const purify = root.DOMPurify;
            const hook = (node, data) => {
                if (node.nodeName === "IMG" && data.attrName === "src" && !chat.ownImage(data.attrValue)) data.keepAttr = false;
            };
            purify.addHook("uponSanitizeAttribute", hook);
            try {
                return purify.sanitize(String(html || ""), chat.purifyConfig);
            } finally {
                purify.removeHook("uponSanitizeAttribute", hook);
            }
        },
        // A plain room's message as the server stored it, with the src of any image that is not this server's taken
        // out (a message stored before the rule). Parsed in a template, whose content loads nothing.
        ownImages(html) {
            const text = String(html || "");
            const doc = root.document;
            if (!doc || !/<img\b/i.test(text)) return text;
            const box = doc.createElement("template");
            box.innerHTML = text;
            let changed = false;
            box.content.querySelectorAll("img").forEach((img) => {
                if (!chat.ownImage(img.getAttribute("src"))) { img.removeAttribute("src"); changed = true; }
            });
            return changed ? box.innerHTML : text;
        },
        // the type a decrypted file's blob is made with: an image the room shows inline, or a sound or video its player plays
        // (a voice note), keeps its type, anything else is bytes to save (a blob URL is of this origin, so a decrypted page or
        // SVG must never open as one; media types run no script)
        blobType(contentType) {
            const type = String(contentType || "").split(";")[0].trim().toLowerCase();
            return /^(image\/(png|jpe?g|gif|webp)|audio\/(webm|ogg|mpeg|mp4|aac|wav|x-m4a)|video\/(mp4|webm|ogg|quicktime))$/.test(type) ? type : "application/octet-stream";
        },
        escapeHtml(text) {
            return String(text || "").replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/\n/g, "<br>");
        },
        // The send parameters of an encrypted room's message (or edit): {encryptedJson, mentionUserIds}. files are the
        // composer's uploads [{chatAttachmentId, fileName, contentType, fileSize, info}], their keys going in the ciphertext.
        sealMessage(roomId, memberIds, html, files, extra) {
            const text = chat.plainText(html);
            const list = (files || []).filter((file) => file && file.chatAttachmentId).map((file) => ({ chatAttachmentId: file.chatAttachmentId,
                name: file.fileName || null, type: file.contentType || null, size: file.fileSize || null, file: file.info || null }));
            const content = { msgtype: !text && list.length ? "m.file" : "m.text", body: text, format: "org.matrix.custom.html", formatted_body: html || "" };
            if (list.length) content["io.moqui.files"] = list;
            // a sticker or a place (plans/Comms-Messaging.md phase 20), only in the ciphertext
            if (extra && extra.sticker) content["io.moqui.sticker"] = extra.sticker;
            if (extra && extra.location) content["io.moqui.location"] = extra.location;
            return moqui.crypto.encryptRoom(roomId, memberIds, "m.room.message", content).then((encrypted) => ({
                encryptedJson: JSON.stringify(encrypted), mentionUserIds: chat.mentionsIn(html).join(",") }));
        },
        // the files of an encrypted message (decrypted, each with its key: sealed) as sealMessage takes them, so an edit
        // carries them in its ciphertext too: the timeline and the details' tabs read an edited message's files there
        keptFiles(attachments) {
            return (attachments || []).filter((file) => file && file.chatAttachmentId && file.sealed).map((file) => ({ chatAttachmentId: file.chatAttachmentId,
                fileName: file.fileName || null, contentType: file.contentType || null, fileSize: file.fileSize || null, info: file.sealed }));
        },
        sealReaction(roomId, memberIds, key) {
            return moqui.crypto.encryptRoom(roomId, memberIds, "m.reaction", { key }).then((encrypted) => JSON.stringify(encrypted));
        },
        // a file for an encrypted room: {blob (to upload, named encrypted), info (its key and hash, for the message)}
        sealFile(file) { return moqui.crypto.encryptFile(file); },
        // the blob URL of an encrypted room's file, fetched from url and decrypted (once while its room is open: the
        // decrypted bytes go when the room closes, releaseFiles)
        openFile(url, file, roomId) {
            const id = file.chatAttachmentId;
            if (roomId) chat.fileRooms[id] = roomId;
            if (chat.fileUrls[id]) return chat.fileUrls[id];
            chat.fileUrls[id] = root.fetch(url, { credentials: "same-origin" }).then((response) => {
                if (!response.ok) throw new Error("status " + response.status);
                return response.arrayBuffer();
            }).then((data) => moqui.crypto.decryptFile(data, file.sealed)).then((bytes) => root.URL.createObjectURL(new root.Blob([bytes],
                { type: chat.blobType(file.contentType) }))).catch((err) => { delete chat.fileUrls[id]; throw err; });
            return chat.fileUrls[id];
        },
        // The decrypted files of a room let go (its blob URLs revoked, the bytes freed): all of them when the room closes
        // (closeRoom, when no view of it is left), or the attachments ids (a message that left the page). Opened again later,
        // a file is fetched and decrypted again.
        releaseFiles(roomId, ids) {
            const wanted = ids ? new Set(Array.from(ids).map(String)) : null;
            for (const id of Object.keys(chat.fileUrls)) {
                if (wanted ? !wanted.has(String(id)) : chat.fileRooms[id] !== roomId) continue;
                const made = chat.fileUrls[id];
                delete chat.fileUrls[id];
                delete chat.fileRooms[id];
                Promise.resolve(made).then((url) => root.URL.revokeObjectURL(url), () => null);
            }
        },
        // A room's events that went (disappearing messages, phase 16): their decrypted copies and reaction keys leave this page.
        forgetEvents(roomId, seqs) {
            const gone = new Set(Array.from(seqs || []).map(Number));
            for (const seq of gone) delete chat.opened[roomId + ":" + seq];
            const prefix = roomId + ":";
            for (const name of Object.keys(chat.reactionKeys)) {
                if (name.indexOf(prefix) !== 0) continue;
                if (gone.has(Number(name.slice(prefix.length))) || gone.has(Number(chat.reactionKeys[name].messageSeq))) delete chat.reactionKeys[name];
            }
        },
        // my reaction with key to a message of a room (its eventSeq, to take it back), null when none
        myReaction(roomId, messageSeq, key, me) {
            const prefix = roomId + ":";
            for (const name of Object.keys(chat.reactionKeys)) {
                const known = chat.reactionKeys[name];
                if (name.indexOf(prefix) === 0 && known.messageSeq === Number(messageSeq) && known.key === key && known.userId === me) return Number(name.slice(prefix.length));
            }
            return null;
        },
        // the decrypted content of an encrypted event (cached per event), or {error}
        openSealed(roomId, eventSeq, sentDate, senderUserId, encrypted) {
            const name = roomId + ":" + eventSeq;
            const known = chat.opened[name];
            if (known && !known.error) return Promise.resolve(known);
            return moqui.crypto.decryptRoom(roomId, { eventSeq, sentDate, senderUserId, encrypted }).then((out) => {
                chat.opened[name] = out || { error: "unreadable" };
                return chat.opened[name];
            }, (err) => ({ error: (err && (err.errors || err.message)) || "unreadable" }));
        },
        // a decrypted message's content on an event or timeline item: body (cleaned), bodyText, files and mentions
        applyOpened(item, opened, attachmentUrl, roomId) {
            if (!opened || opened.error) {
                Object.assign(item, { body: null, bodyText: null, undecryptable: true, withheld: (opened && opened.withheld) || null });
                return Promise.resolve(item);
            }
            const content = opened.content || {};
            const html = content.formatted_body ? chat.clean(content.formatted_body) : chat.escapeHtml(content.body);
            const files = Array.isArray(content["io.moqui.files"]) ? content["io.moqui.files"] : [];
            const stored = Array.isArray(item.attachments) ? item.attachments : [];
            Object.assign(item, { body: html || null, bodyText: content.body || "", undecryptable: false, withheld: null, verifiedSender: !!opened.verified });
            // a sticker or a place sent encrypted (phase 20), checked as the server checks a plain one's
            if (content["io.moqui.sticker"]) item.sticker = chat.stickers.clean(content["io.moqui.sticker"]);
            if (content["io.moqui.location"]) item.location = chat.place.clean(content["io.moqui.location"]);
            if (files.length || stored.length) {
                item.attachments = stored.map((file) => {
                    const sealed = files.find((one) => one.chatAttachmentId === file.chatAttachmentId) || {};
                    return Object.assign({}, file, { fileName: sealed.name || file.fileName || null, contentType: sealed.type || null,
                        fileSize: sealed.size || file.fileSize || null, sealed: sealed.file || null });
                });
                if (!stored.length) item.attachments = files.map((one, index) => ({ chatAttachmentId: one.chatAttachmentId, attachmentSeq: index + 1,
                    fileName: one.name || null, contentType: one.type || null, fileSize: one.size || null, sealed: one.file || null }));
            }
            // an image is shown from its decrypted blob
            const images = attachmentUrl ? item.attachments.filter((file) => file.sealed && /^image\//i.test(file.contentType || "")) : [];
            return Promise.all(images.map((file) => chat.openFile(attachmentUrl(file), file, roomId).then((url) => { file.blobUrl = url; }, () => null))).then(() => item);
        },
        // Decrypts the encrypted events of a page of a room's timeline in place (get#Timeline's items). Nothing for plain ones.
        openEvents(roomId, list, attachmentUrl) {
            const items = (list || []).filter((item) => item && (item.encrypted || (item.encryptedReactions || []).length || (item.replyTo && item.replyTo.encrypted)));
            if (!items.length) return Promise.resolve(list);
            return chat.loadPurify().then(() => Promise.all(items.map((item) => {
                const work = [];
                if (item.encrypted) work.push(chat.openSealed(roomId, item.encryptedEventSeq || item.eventSeq, item.encryptedSentDate || item.sentDate,
                    item.senderUserId, item.encrypted).then((opened) => chat.applyOpened(item, opened, attachmentUrl, roomId)));
                if ((item.encryptedReactions || []).length) work.push(Promise.all(item.encryptedReactions.map((reaction) =>
                    chat.openSealed(roomId, reaction.eventSeq, reaction.sentDate, reaction.senderUserId, reaction.encrypted).then((opened) => ({ reaction, opened }))))
                    .then((all) => {
                        const byKey = {};
                        for (const { reaction, opened } of all) {
                            const key = opened && !opened.error && opened.content ? opened.content.key : null;
                            if (!key) continue;
                            chat.reactionKeys[roomId + ":" + reaction.eventSeq] = { key, userId: reaction.senderUserId, messageSeq: Number(item.eventSeq) };
                            const entry = byKey[key] = byKey[key] || { key, count: 0, userIds: [], mine: false };
                            if (entry.userIds.indexOf(reaction.senderUserId) >= 0) continue;
                            entry.userIds.push(reaction.senderUserId);
                            entry.count++;
                            if (reaction.senderUserId === chat.me) entry.mine = true;
                        }
                        item.reactions = Object.values(byKey);
                    }));
                if (item.replyTo && item.replyTo.encrypted) work.push(chat.openSealed(roomId, item.replyTo.eventSeq, item.replyTo.sentDate,
                    item.replyTo.senderUserId, item.replyTo.encrypted).then((opened) => {
                    const text = opened && !opened.error && opened.content ? String(opened.content.body || "").replace(/\s+/g, " ").trim() : "";
                    item.replyTo.snippet = text.length > 100 ? text.slice(0, 99) + "…" : text;
                }));
                return Promise.all(work);
            }))).then(() => list, (err) => { console.error("Encrypted events not opened", err); return list; });
        },
        // Decrypts a pushed event of an encrypted room in place, before applyEvent: a message or an edit gets its body, a
        // reaction its key, and the take-back of an encrypted reaction the key it had.
        openEvent(roomId, event, attachmentUrl) {
            if (!event) return Promise.resolve(event);
            if (event.type === "redaction" && event.content && event.content.target === "reaction" && !event.content.key) {
                const known = chat.reactionKeys[roomId + ":" + event.relatesToEventSeq];
                if (known) { event.content = Object.assign({}, event.content, { key: known.key }); delete chat.reactionKeys[roomId + ":" + event.relatesToEventSeq]; }
                return Promise.resolve(event);
            }
            if (!event.encrypted) return Promise.resolve(event);
            return chat.loadPurify().then(() => chat.openSealed(roomId, event.eventSeq, event.sentDate, event.senderUserId, event.encrypted)).then((opened) => {
                if (event.type === "reaction") {
                    const key = opened && !opened.error && opened.content ? opened.content.key : null;
                    if (key) {
                        event.content = Object.assign({}, event.content, { key });
                        chat.reactionKeys[roomId + ":" + event.eventSeq] = { key, userId: event.senderUserId, messageSeq: Number(event.relatesToEventSeq) };
                    }
                    return event;
                }
                return chat.applyOpened(event, opened, attachmentUrl, roomId);
            }).then(() => event, (err) => { console.error("An encrypted event was not opened", err); return event; });
        },

        // ---- the outbox ----

        // under the storage scope: in the desktop client every server's pages share one origin (MoquiLib scopedKey)
        outboxKey() { return moqui.scopedKey("moqui.chat.outbox." + (chat.me || "")); },
        // the outbox as stored (another tab may have changed it); the state changes only when it did, so the screens
        // do not draw their pending rows again for nothing
        readOutbox() {
            if (chat.host()) return chat.state.outbox.map((item) => Object.assign({}, item));
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
            // the host takes its own off (an echo, an answer); the page shows it gone now
            if (chat.host()) { chat.state.outbox = chat.outboxRemove(chat.state.outbox, clientRef); chat.emit("outbox", {}); return; }
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
            // an encrypted room's item keeps no text: it shows the text this page sent, else that it is encrypted
            const shown = (item) => (item.sealed ? Object.assign({}, item, { text: chat.sealedTexts[item.clientRef] || chat.labels.encryptedMessage }) : item);
            return chat.outboxFor(chat.state.outbox, roomId, threadRoot, Date.now()).map((item) => shown(chat.state.sending[item.clientRef]
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
            const host = chat.host();
            if (host) {
                // shown at once; the host sends it, and its answer comes as sent
                chat.state.outbox = list;
                chat.emit("outbox", {});
                return host.add("chat-send", kept).then(() => { chat.refreshHost(); return { queued: true, clientRef: kept.clientRef }; }, (err) => {
                    chat.state.outbox = chat.outboxRemove(chat.state.outbox, kept.clientRef);
                    chat.emit("outbox", {});
                    // one the host will not keep (not its outbox being full) goes now while online, as the web sends it
                    if (chat.state.online && !/full/i.test(String((err && err.message) || ""))) return chat.sendNow(kept);
                    return Promise.reject({ outbox: true, status: 0, error: err && err.message });
                });
            }
            chat.saveOutbox(list);
            if (!chat.state.online) return Promise.resolve({ queued: true, clientRef: kept.clientRef });
            return chat.sendItem(kept);
        },
        // A message sent at once, not kept (the host would not): resolves as send does, {sent, json} or {failed, error}.
        sendNow(item) {
            const params = Object.assign({}, item.params, { chatRoomId: item.chatRoomId, clientRef: item.clientRef });
            return moqui.request(item.url, { method: "POST", params }).then((result) => {
                const json = (result && result.json) || {};
                if (json.event || json.eventSeq != null) {
                    chat.emit("sent", { clientRef: item.clientRef, chatRoomId: item.chatRoomId, threadRootEventSeq: item.threadRootEventSeq ?? null, json });
                    return { sent: true, json };
                }
                return { failed: true, clientRef: item.clientRef, error: json.errors ? [].concat(json.errors).join(" ") : null };
            }, (err) => {
                const json = err && err.json;
                return { failed: true, clientRef: item.clientRef, error: json && json.errors ? [].concat(json.errors).join(" ") : null };
            });
        },
        newClientRef() { return "c" + Date.now().toString(36) + Math.random().toString(36).slice(2, 8); },
        retry(clientRef) {
            const host = chat.host();
            if (host) return host.retry(clientRef).then(() => { chat.refreshHost(); return { queued: true, clientRef }; });
            const item = chat.changeOutbox(clientRef, { status: "waiting", nextDate: 0, attempts: 0, error: null, createdDate: Date.now() });
            return item ? chat.sendItem(item) : Promise.resolve(null);
        },
        discard(clientRef) {
            const host = chat.host();
            if (host) {
                chat.dropOutbox(clientRef);
                chat.state.changes = chat.state.changes.filter((change) => change.clientRef !== clientRef);
                return host.discard(clientRef).then(() => chat.refreshHost());
            }
            chat.dropOutbox(clientRef);
            return Promise.resolve();
        },
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
            if (!chat.started || !chat.state.online || chat.flushing || chat.host()) return Promise.resolve();
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
            // a page in a preview frame is a picture of a page: it is not the user being there
            if (!chat.vm || moqui.previewToken) return;
            const idle = chat.idleSeconds(chat.lastInput, Date.now());
            chat.lastBeatIdle = idle;
            chat.request("/userStatusHeartbeat", { idleSeconds: idle }, "POST").then((json) => { if (json && json.myStatus) chat.setMe(json.myStatus); });
        }
    };
    // ---- what the chat screens show the same way: Chat.qvue, Panel/ChatPanel.qvue and the shell's Root.qvue ----
    // Dates are in the user's zone and language (moqui.l10n, read when called: a test runs this file without it), and
    // compare calendar days, so the boundaries hold through daylight saving changes (COM-UI-007; Comms-Messaging §2.2).
    const l10n = () => moqui.l10n;
    const tr = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);
    const fillIn = (text, values) => String(text).replace(/\{(\w+)\}/g, (match, key) => (values[key] == null ? "" : String(values[key])));
    Object.assign(chat, {
        // a room's type icon (people for a private room: a lock means end-to-end encrypted only, COM-UI-001); a record room
        // shows its record's own
        ROOM_ICONS: { CrtDirect: "sym_o_group", CrtPrivate: "sym_o_groups", CrtPublic: "sym_o_tag", CrtRecord: "sym_o_description", CrtExternal: "sym_o_sms" },
        // the layout thresholds (Comms-Messaging §1): a room under NARROW_PX wide shows bubbles, under SIDE_OVERLAY_PX a
        // details pane over the timeline; a screen under Quasar's md (1024 px) is the stacked layout
        NARROW_PX: 640,
        SIDE_OVERLAY_PX: 900,
        timeText(millis) { return millis ? l10n().formatZoned(Number(millis), { hour: "numeric", minute: "2-digit" }) : ""; },
        fullTime(millis) { return millis ? l10n().formatZoned(Number(millis), { dateStyle: "full", timeStyle: "short" }) : ""; },
        dateTime(millis) { return millis ? l10n().formatZoned(Number(millis), { dateStyle: "medium", timeStyle: "short" }) : ""; },
        // a day's label in the timeline: Today, Yesterday, else its date, with the year when it is not this one
        dayLabel(millis) {
            const z = l10n(), now = Date.now(), day = z.dayNumber(Number(millis)), today = z.dayNumber(now);
            if (day === today) return tr("Today");
            if (day === today - 1) return tr("Yesterday");
            const opts = { weekday: "long", month: "long", day: "numeric" };
            if (z.dayKey(Number(millis)).slice(0, 4) !== z.dayKey(now).slice(0, 4)) opts.year = "numeric";
            return z.formatZoned(Number(millis), opts);
        },
        // a list's time: the time today, the weekday in the last six days, the date, with the year when it is not this one
        shortWhen(millis) {
            if (!millis) return "";
            const z = l10n(), at = Number(millis), now = Date.now(), days = z.dayNumber(now) - z.dayNumber(at);
            if (days === 0) return chat.timeText(at);
            if (days < 6) return z.formatZoned(at, { weekday: "short" });
            if (z.dayKey(at).slice(0, 4) === z.dayKey(now).slice(0, 4)) return z.formatZoned(at, { month: "short", day: "numeric" });
            return z.formatZoned(at, { year: "numeric", month: "short", day: "numeric" });
        },
        roomIcon(room) { return (room && room.record && room.record.icon) || (room && chat.ROOM_ICONS[room.roomTypeEnumId]) || "sym_o_chat"; },
        // The direct room of exactly these people with me (the rooms of find#Rooms, otherUserIdList), the encrypted one first
        // when there are both (Comms-Messaging §6.5), or null: Chat's dm link opens it, else an unsaved message whose first
        // send makes the room (§3), so a link followed by mistake makes nothing the other person sees. plain: the plain one
        // only (the link back from an encrypted direct room to the earlier chat).
        directRoomFor(rooms, userIds, me, plain) {
            const wanted = Array.from(new Set((userIds || []).map(String).filter((id) => id && id !== me))).sort().join(",");
            if (!wanted) return null;
            const of = (room) => room && room.roomTypeEnumId === "CrtDirect" && room.isMember
                && Array.from(new Set((room.otherUserIdList || (room.otherUserId ? [room.otherUserId] : [])).map(String))).sort().join(",") === wanted;
            // the oldest of each kind, as the server answers it (two made at once: create#ChatRoom answers the oldest)
            const order = (a, b) => (Number(a.createdDate) || 0) - (Number(b.createdDate) || 0)
                || (/^\d+$/.test(a.chatRoomId) && /^\d+$/.test(b.chatRoomId) ? Number(a.chatRoomId) - Number(b.chatRoomId) : String(a.chatRoomId).localeCompare(String(b.chatRoomId)));
            const list = (rooms || []).filter(of).sort(order);
            const found = (plain ? null : list.find((room) => room.encrypted)) || list.find((room) => !room.encrypted);
            return found ? found.chatRoomId : null;
        },
        // The mail lines of the people's direct chats (the Chat screen's mailLines, find#DirectChatMail) by room, asked once for
        // the page and shared by Chat and the tray's or the home's panel: a request within maxAge (five minutes) answers
        // what the last one did. request(url) is moqui.request's; a failure keeps nothing, so the next asks again.
        mailLines(url, request, maxAge) {
            const now = Date.now();
            const kept = chat.mailCache;
            if (kept && kept.url === url && now - kept.at < (maxAge || 300000)) return kept.promise;
            const promise = (request || moqui.request)(url).then((result) => {
                const out = {};
                ((result && result.json && result.json.mailList) || []).forEach((mail) => { if (mail && mail.chatRoomId && mail.url) out[mail.chatRoomId] = mail; });
                return out;
            }, () => { if (chat.mailCache && chat.mailCache.promise === promise) chat.mailCache = null; return {}; });
            chat.mailCache = { url, at: now, promise };
            return promise;
        },
        // archived by me, until a message after the archive (ChatRoomMember.archivedEventSeq)
        isArchived(room) {
            return !!room.isMember && room.archivedEventSeq != null && Number(room.lastMessage ? room.lastMessage.eventSeq : 0) <= Number(room.archivedEventSeq);
        },
        countsOf(room) {
            if (!room.isMember) return { unread: 0, mentions: 0 };
            if (chat.state && chat.state.unread[room.chatRoomId]) return chat.unreadOf(chat.state, room.chatRoomId);
            return { unread: room.unreadCount || 0, mentions: room.mentionCount || 0 };
        },
        // a room's last message in a list, with who sent it, but not in a direct room the other person wrote in
        roomSnippet(room, meUserId) {
            const last = room.lastMessage;
            if (!room.isMember) return tr("Public room: not joined");
            if (!last) return room.topic || tr("No messages yet");
            const who = last.senderUserId === meUserId ? tr("You") : String(last.senderName || "").split(" ")[0];
            const text = last.encrypted ? tr("Encrypted message") : last.redacted || !last.snippet ? tr("Message removed") : last.snippet;
            return room.roomTypeEnumId === "CrtDirect" && last.senderUserId !== meUserId && room.otherUserId ? text : (who ? who + ": " : "") + text;
        },
        // ---- the new-chat pickers (Chat's New direct message, the chat panel's New message, the phone home's Start chat) ----
        // A picked value is a userId, or a phone number as Chat's searchUsers offers it with numbers=true (tel:<number>).
        isNumberPick(value) { return String(value || "").indexOf("tel:") === 0; },
        // what was picked: none, direct (one person), group (several), number (one phone number), numbers (several: a
        // group text, plans/Comms-Messaging.md §9), or mixed (numbers with people, which Start refuses: a text goes to
        // numbers only)
        pickKind(values) {
            const list = (values || []).map((value) => String(value).trim()).filter((value) => value);
            if (!list.length) return "none";
            const numbers = list.filter((value) => chat.isNumberPick(value)).length;
            if (numbers) return numbers < list.length ? "mixed" : numbers > 1 ? "numbers" : "number";
            return list.length > 1 ? "group" : "direct";
        },
        // Start: the number's room (textRoom: open#PairRoom, from fromNumber or the user's only number), a private room when
        // a group has a name (createRoom), else the direct or group direct chat (createDirect); the server decides whether a
        // new one is encrypted (everyone in it has a crypto device, Comms-Messaging §6.5). post(name, params) is the screen's,
        // a promise of the transition's JSON (chatRoomId, encrypted).
        startChat(post, pick) {
            const values = ((pick && pick.values) || []).map((value) => String(value).trim()).filter((value) => value);
            const kind = chat.pickKind(values);
            if (kind === "none") return Promise.reject({ refused: "none" });
            if (kind === "mixed") {
                if (moqui.notifyMessages) moqui.notifyMessages([{ type: "info", message: tr("A text goes to phone numbers only: start it without the people") }]);
                return Promise.reject({ refused: "mixed" });
            }
            if (kind === "number" || kind === "numbers") {
                return post("textRoom", { textTo: values.map((value) => value.slice(4)).join(","), textFrom: (pick && pick.fromNumber) || "" });
            }
            const name = String((pick && pick.name) || "").trim();
            if (kind === "group" && name) return post("createRoom", { roomTypeEnumId: "private", roomName: name, memberUserIds: values.join(",") });
            return post("createDirect", { userIds: values.join(",") });
        },
        // ---- names of outside numbers (plans/Comms-Messaging.md §9, Phone book) ----
        // The numbers of a phone room (CrtExternal, named by its number, or a group text's numbers comma-separated), or
        // none for another room or one whose name is not numbers.
        roomNumbers(room) {
            if (!room || room.roomTypeEnumId !== "CrtExternal") return [];
            const parts = String(room.roomName || "").split(",").map((part) => part.trim()).filter((part) => part);
            return parts.length && parts.every((part) => /^\+[0-9]{5,15}$/.test(part)) ? parts : [];
        },
        // a phone room's name with its numbers named (names: number to name), each number left as it is when unknown
        phoneTitle(numbers, names) {
            return numbers.map((number) => (names && names[number]) || number).join(", ");
        },
        // Phone rooms named by the names of their numbers: the phone's own contacts first (the mobile app's
        // moquiDesktop.contactNames, read on the phone), then the apps' call-context (Chat's phoneNames). The server is asked
        // for every number of the rooms, not only those the phone does not know, so it cannot tell which numbers are in the
        // phone's contacts. rooms is a screen's reactive list, its displayName set now from what is known and again when the
        // rest comes. Never asks twice for a number; contactsChanged (the contacts allowed later) asks the phone again.
        phoneNames: {},
        contactNamesOf: {},
        serverNamesOf: {},
        namedRooms: [],
        nameRooms(rooms) {
            const list = Array.isArray(rooms) ? rooms : [];
            chat.namedRooms = list;
            const apply = () => {
                list.forEach((room) => {
                    const numbers = chat.roomNumbers(room);
                    if (numbers.length) room.displayName = chat.phoneTitle(numbers, chat.phoneNames);
                });
                // a person's own name over the phone names (§11.1)
                chat.applyPeople(list);
            };
            const merge = () => {
                const names = {};
                Object.keys(chat.serverNamesOf).forEach((number) => { if (chat.serverNamesOf[number]) names[number] = chat.serverNamesOf[number]; });
                Object.keys(chat.contactNamesOf).forEach((number) => { if (chat.contactNamesOf[number]) names[number] = chat.contactNamesOf[number]; });
                chat.phoneNames = names;
                apply();
            };
            apply();
            const all = Array.from(new Set(list.flatMap((room) => chat.roomNumbers(room)))).slice(0, 50);
            const host = root.moquiDesktop;
            const fromHost = all.filter((number) => !(number in chat.contactNamesOf));
            const fromServer = all.filter((number) => !(number in chat.serverNamesOf));
            fromHost.forEach((number) => { chat.contactNamesOf[number] = null; });
            fromServer.forEach((number) => { chat.serverNamesOf[number] = null; });
            const phone = fromHost.length && host && typeof host.contactNames === "function"
                ? Promise.resolve(host.contactNames(fromHost)).then((names) => {
                    Object.keys(names || {}).forEach((number) => { if (names[number]) chat.contactNamesOf[number] = String(names[number]); });
                }, () => null) : Promise.resolve();
            const base = moqui.call && moqui.call.chatBase ? moqui.call.chatBase() : "/apps/BasicComms/Chat";
            const server = fromServer.length ? moqui.request(base + "/phoneNames", { params: { numbers: fromServer.join(",") } }).then((result) => {
                const map = (result.json && result.json.nameMap) || {};
                Object.keys(map).forEach((number) => { if (map[number]) chat.serverNamesOf[number] = String(map[number]); });
            }, () => null) : Promise.resolve();
            return Promise.all([phone, server]).then(merge);
        },
        // The phone's contacts became readable (the permission granted after the rooms were named): ask the phone again for
        // the rooms last named, with no reload.
        contactsChanged() {
            chat.contactNamesOf = {};
            return chat.nameRooms(chat.namedRooms);
        },
        // The phone's own contacts as a new-chat picker's extra options (the mobile app's moquiDesktop.contactsSearch, which
        // asks for the contacts permission on first use): each {value: tel:<number>, label: name · number}.
        contactOptions(term) {
            const host = root.moquiDesktop;
            const typed = String(term || "").trim();
            if (!host || typeof host.contactsSearch !== "function" || typed.length < 2) return Promise.resolve([]);
            return Promise.resolve(host.contactsSearch(typed)).then((list) => (Array.isArray(list) ? list : []).slice(0, 10)
                .filter((contact) => contact && contact.number)
                .map((contact) => ({ value: "tel:" + contact.number, label: (contact.name ? contact.name + " · " : "") + contact.number })), () => []);
        },
        // the user's numbers that text (Chat's textNumbers), as a picker's From options
        numberOptions(list) {
            return (list || []).map((number) => ({ value: number.phoneNumber, label: number.phoneNumber + (number.description ? " · " + number.description : "") }));
        },
        // ---- one person across networks (plans/Comms-Messaging.md §11.1, phase 22) ----
        // The people of my rooms (Chat's people transition, ChatPeopleServices.find#ChatPeople: {personList, roomPersonMap}),
        // asked once for the page and shared by Chat and the panel, as mailLines: a request within maxAge (five minutes)
        // answers what the last one did; peopleChanged() (a merge, a split, a name) asks again. A failure keeps nothing.
        peopleState: null,
        people(url, request, maxAge) {
            const now = Date.now();
            const kept = chat.peopleCache;
            if (kept && kept.url === url && now - kept.at < (kept.maxAge || maxAge || 300000)) return kept.promise;
            const promise = (request || moqui.request)(url).then((result) => {
                const json = (result && result.json) || {};
                const state = { personList: Array.isArray(json.personList) ? json.personList : [], roomPersonMap: json.roomPersonMap || {},
                    contactsAway: !!(json.hookFailed || json.contactsSkipped) };
                // the contacts were away (the hook failed or its hour was used): asked again soon, not in five minutes
                if (state.contactsAway && chat.peopleCache && chat.peopleCache.promise === promise) chat.peopleCache.maxAge = 30000;
                chat.peopleState = state;
                chat.applyPeople(chat.namedRooms);
                return state;
            }, () => { if (chat.peopleCache && chat.peopleCache.promise === promise) chat.peopleCache = null; return { personList: [], roomPersonMap: {} }; });
            chat.peopleCache = { url, at: now, promise };
            return promise;
        },
        peopleChanged() { chat.peopleCache = null; chat.mailCache = null; },
        // the person of a room, or null
        personOf(chatRoomId, state) {
            const people = state || chat.peopleState;
            if (!people || !chatRoomId) return null;
            const key = (people.roomPersonMap || {})[chatRoomId];
            return key ? (people.personList || []).find((person) => person.personKey === key) || null : null;
        },
        // A person's room in a list shows the person's name: the user's own name for them, a user's name, the phone's
        // contacts, the app's contact, call-context, the number. A phone room's phone names are nameRooms'; this puts the
        // user's own and a user's name over them, and the app's contact where the phone has none.
        applyPeople(rooms, state) {
            const people = state || chat.peopleState;
            if (!people) return;
            (Array.isArray(rooms) ? rooms : []).forEach((room) => {
                const person = chat.personOf(room.chatRoomId, people);
                if (!person || !person.name) return;
                const numbers = chat.roomNumbers(room);
                const phoneKnows = numbers.some((number) => chat.contactNamesOf[number]);
                if (person.nameFrom === "mine" || (person.nameFrom === "user" && room.roomTypeEnumId !== "CrtDirect")
                    || (person.nameFrom === "contact" && room.roomTypeEnumId !== "CrtDirect" && !phoneKnows)) room.displayName = person.name;
            });
        },
        NETWORK_ICONS: { moqui: "sym_o_chat", sms: "sym_o_sms", rcs: "sym_o_verified", groupme: "sym_o_groups", mail: "sym_o_mail" },
        networkName(network) {
            return { moqui: tr("MR Moqui chat"), sms: tr("SMS"), rcs: tr("RCS"), groupme: "GroupMe", mail: tr("Email") }[network] || network || "";
        },
        // A way's E2EE label (§6.2), as the room it opens shows it: a new direct chat's from directEncrypted (the server's
        // encryptionDefault for the person), unknown until asked.
        wayLabel(way, directEncrypted) {
            const it = way || {};
            if (it.network === "mail") return chat.e2eeLabel({ mail: true });
            if (it.network === "moqui") {
                const encrypted = it.kind === "direct" ? directEncrypted === true : !!it.encrypted;
                return chat.e2eeLabel({ roomTypeEnumId: "CrtDirect", visibilityEnumId: "CrvMembers", encrypted });
            }
            if (it.network === "rcs") return chat.e2eeLabel({ roomTypeEnumId: "CrtExternal", bridges: [{ network: "rcs", outbound: true }] });
            if (it.network === "groupme") return chat.e2eeLabel({ bridges: [{ network: "groupme", networkName: "GroupMe", encryption: "NoneByNetwork" }] });
            return chat.e2eeLabel({ roomTypeEnumId: "CrtExternal" });
        },
        // The ways of a person to show (the network button, the person card), in the server's order, the best first: each the
        // way with key, icon, name, detail (the number, where a text goes from, the address), label (wayLabel), current (the
        // room open now), reason (why it cannot be used now) and usable.
        waysOf(person, currentRoomId, directEncrypted) {
            const reasons = { optedOut: tr("They replied STOP"), notConnected: tr("Not connected") };
            return ((person && person.ways) || []).map((way, index) => {
                const parts = [];
                if (way.number) parts.push(way.number);
                if (way.fromNumber) parts.push(fillIn(tr("from {number}"), { number: way.fromNumber }));
                if (way.address) parts.push(way.address);
                if (way.kind === "direct") parts.push(tr("New chat"));
                if (way.kind === "text") parts.push(tr("New text"));
                const reason = way.blocked ? tr("Blocked") : way.reasonCode ? reasons[way.reasonCode] || way.reasonCode : null;
                return Object.assign({}, way, { key: index + ":" + (way.chatRoomId || way.kind + ":" + (way.number || way.address || way.userId || "") + ":" + (way.fromNumber || "")),
                    icon: chat.NETWORK_ICONS[way.network] || "sym_o_chat", name: chat.networkName(way.network), detail: parts.join(" · "),
                    label: chat.wayLabel(way, directEncrypted), current: !!way.chatRoomId && way.chatRoomId === currentRoomId, reason, usable: !reason });
            });
        },
        // The room a way opens: an existing room, the direct chat made (createDirect, no encrypted: the server decides), the
        // number's room (textRoom), or none for mail (the Inbox opens). post(name, params) is the screen's. Resolves
        // {chatRoomId, encrypted} (a made direct chat's as the server made it, never a guess) or {url} (mail); a refused way
        // rejects, and its reason is the server's message.
        openWay(way, post) {
            const it = way || {};
            if (it.kind === "room" && it.chatRoomId) return Promise.resolve({ chatRoomId: it.chatRoomId, encrypted: !!it.encrypted });
            if (it.kind === "mail") return Promise.resolve({ url: it.url || null });
            const made = (json) => (json && json.chatRoomId ? { chatRoomId: json.chatRoomId, encrypted: json.encrypted === true } : Promise.reject({ refused: "notMade" }));
            if (it.kind === "direct" && it.userId) return post("createDirect", { userIds: it.userId }).then(made);
            if (it.kind === "text" && it.number) return post("textRoom", { textTo: it.number, textFrom: it.fromNumber || "" }).then((json) => made(Object.assign({}, json, { encrypted: false })));
            return Promise.reject({ refused: "none" });
        },
        // Moving a draft from the room open (fromSealed) to the room opened (toSealed, as openWay answered it): a note when the
        // move leaves end-to-end encryption, which the user is told once, else null.
        moveNote(fromSealed, toSealed, network) {
            if (!fromSealed || toSealed) return null;
            return fillIn(tr("Moved to {network}: not end-to-end encrypted"), { network: chat.networkName(network) });
        },
        // A draft moved into a room that has one: both kept, the room's first, a blank line between (never overwritten).
        joinDrafts(existing, moved) {
            const empty = (html) => !html || !String(html).replace(/<[^>]*>/g, "").replace(/&nbsp;/g, " ").trim();
            if (empty(moved)) return existing || "";
            if (empty(existing)) return moved;
            return String(existing) + "<p><br></p>" + String(moved);
        },
        // The nudge in a person's text or bridge room when they are a user of this server: the Moqui chat way to offer, or
        // null (it is the room's own network, the person has none, or it was closed for this room).
        nudgeWay(person, room, closed) {
            if (!person || !person.userId || person.reachable === false || !room || closed || person.blocked) return null;
            const own = (person.rooms || []).find((one) => one.chatRoomId === room.chatRoomId);
            if (!own || own.network === "moqui") return null;
            return (person.ways || []).find((way) => way.network === "moqui" && !way.blocked && !way.reasonCode) || null;
        },
        nudgeKey(chatRoomId) { return "moqui.chat.nudge." + chatRoomId; },
        nudgeClosed(chatRoomId) { try { return !!(root.localStorage && root.localStorage.getItem(chat.nudgeKey(chatRoomId))); } catch (err) { return false; } },
        closeNudge(chatRoomId) { try { if (root.localStorage) root.localStorage.setItem(chat.nudgeKey(chatRoomId), "1"); } catch (err) { /* only a convenience */ } },
        // A change to my people (merge, separate, rename, reset; Chat's transitions): post(name, params) is the screen's.
        // Resolves the transition's JSON, and the next people() asks the server again.
        changePeople(post, name, params) {
            return post(name, params).then((json) => { chat.peopleChanged(); return json || {}; });
        },
        // ---- a message's Info (Comms-Messaging §4, COM-UI-008): only what the server knows ----
        // event: as get#Timeline gives it (eventSeq, sentDate, senderUserId, senderExternal, delivery, threadRootEventSeq);
        // members: the room's memberList, whose lastReadEventSeq is a position ("has read up to here or later") and is blank
        // for another member with read receipts off (one who joined after the message, joinedDate, is not a reader of it); opts: {me, receipts (false: the viewer's read receipts are off), inThread,
        // limit (the names shown, 8)}. Answers {sent (millis), from (the network a bridged message came from), delivered (per
        // network, only for a message that went out on a bridge: [{name, state, receipt, error, path}]), readBy (null in a thread,
        // which has no read markers; {off: true} when the viewer's receipts are off; else {people, more, count}), thread}.
        // Delivery to a member's device is never in it: the server does not know it, and a push acknowledgement is not it.
        messageInfo(event, members, opts) {
            const it = event || {};
            const options = opts || {};
            const seq = Number(it.eventSeq);
            const thread = !!options.inThread || (it.threadRootEventSeq != null && it.threadRootEventSeq !== "");
            const destinations = (it.delivery && it.delivery.destinations) || [];
            // two bridges of one network are told apart by their own names
            const network = (one) => one.networkName || one.bridgeName || "";
            const repeats = (name) => destinations.filter((one) => network(one) === name).length > 1;
            const delivered = destinations.length ? destinations.map((one) => ({
                name: repeats(network(one)) && one.bridgeName && one.bridgeName !== network(one) ? network(one) + " · " + one.bridgeName : network(one),
                state: one.state || "pending", receipt: one.receipt || null, error: one.error || null, path: one.path || null,
                adapterName: one.adapterName || null, note: one.note || null,
                // a group text's receipts, one a recipient (number to its receipt)
                recipients: one.recipients && typeof one.recipients === "object" ? one.recipients : null })) : null;
            let readBy = null;
            if (!thread && options.receipts === false) readBy = { off: true };
            else if (!thread) {
                // a member who joined after the message has a marker past it (a join reads to the latest) but never read it
                const sent = it.sentDate != null && it.sentDate !== "" ? Number(it.sentDate) : null;
                const joinedBefore = (member) => member.joinedDate == null || member.joinedDate === "" || sent == null || Number(member.joinedDate) <= sent;
                const people = (members || []).filter((member) => member && member.userId && member.userId !== options.me && member.userId !== it.senderUserId
                    && !member.isAgent && member.lastReadEventSeq != null && member.lastReadEventSeq !== "" && Number(member.lastReadEventSeq) >= seq
                    && joinedBefore(member))
                    .map((member) => ({ userId: member.userId, name: member.name || member.userId }));
                const limit = options.limit || 8;
                readBy = { people: people.slice(0, limit), more: Math.max(0, people.length - limit), count: people.length };
            }
            return { sent: it.sentDate != null && it.sentDate !== "" ? Number(it.sentDate) : null,
                from: it.senderExternal ? (it.senderExternal.networkName || it.senderExternal.network || tr("another network")) : null, delivered, readBy, thread };
        },
        // A destination's fallback, for the bubble's caption ("Delivered to {network} as {path}", Comms-Messaging §16): SMS or
        // MMS when an RCS bridge's message went as the fallback; '' for RCS, an unknown way, or any other bridge (which
        // records none).
        pathName(destination) {
            const path = destination && destination.path;
            return path === "sms" ? tr("SMS") : path === "mms" ? tr("MMS") : "";
        },
        // The provider an SMS or RCS bridge's adapter is of (rcs-twilio: Twilio), for the RCS label's banner; '' for another
        providerName(adapterName) {
            const name = String(adapterName || "");
            if (/(^|-)twilio$/.test(name)) return "Twilio";
            if (/(^|-)sinch$/.test(name)) return "Sinch";
            if (name === "sms" || /(^|-)telnyx$/.test(name)) return "Telnyx";
            return "";
        },
        // An RCS message's encryption by the way it went (Comms-Messaging §16.5): RCS (or not known yet) in transit only, the
        // provider named; the fallback's SMS or MMS not encrypted
        rcsDetail(path, adapterName) {
            if (path === "sms") return tr("The phone did not support RCS. This message went as SMS.");
            if (path === "mms") return tr("The phone did not support RCS. This message went as MMS.");
            return fillIn(tr("Encrypted in transit only. {provider}, Google, carriers and this server can read it."),
                { provider: chat.providerName(adapterName) || tr("The provider") });
        },
        // ---- rich messages (Comms-Messaging §18.3 phase 13; the server's ChatRich.js, kept in step by the specs) ----
        // The text a rich message ({kind, text, suggestions, cards}) goes as where cards are not shown: its words, each card's
        // title and words (a carousel's numbered), "label: URL" for a link, "label: number" for a call, "Reply: Yes, No"
        richFallback(rich) {
            const it = rich || {};
            const lines = (suggestions) => {
                const out = [];
                for (const one of suggestions || []) {
                    if (one.type === "url") out.push(one.text + ": " + one.url);
                    else if (one.type === "dial") out.push(one.text + ": " + one.phone);
                }
                const replies = (suggestions || []).filter((one) => one.type === "reply").map((one) => one.text);
                if (replies.length) out.push(tr("Reply: ${options}").replace("${options}", replies.join(", ")));
                return out;
            };
            const blocks = [];
            if (it.kind === "text") {
                blocks.push([it.text || ""]);
                const more = lines(it.suggestions);
                if (more.length) blocks.push(more);
            } else {
                (it.cards || []).forEach((card, index) => {
                    const head = it.kind === "carousel" ? (index + 1) + ". " + (card.title || card.description) : card.title;
                    const block = [];
                    if (head) block.push(head);
                    if (card.description && !(it.kind === "carousel" && !card.title)) block.push(card.description);
                    blocks.push(block.concat(lines(card.suggestions)));
                });
            }
            return blocks.map((block) => block.join("\n")).join("\n\n");
        },
        // Info as lines [{key, label, text}] (the dialog's, and what a test reads): when is fullTime(millis). A read receipt is
        // never said to be missing: an empty Read by says who is not listed, and receipts off say so.
        messageInfoLines(info, when) {
            const lines = [];
            if (info.sent != null) lines.push({ key: "sent", label: tr("Sent"), text: when ? when(info.sent) : String(info.sent) });
            if (info.from) lines.push({ key: "from", label: tr("Came from"), text: info.from });
            const STATES = { pending: tr("Sending"), failed: tr("Not delivered"), uncertain: tr("May not have arrived") };
            const RECEIPTS = { delivered: tr("Delivered: the network's receipt says so"), read: tr("Read: the network's receipt says so"), sent: tr("Sent on by the network"),
                unconfirmed: tr("Taken by the network; it has not confirmed delivery") };
            const PATHS = { rcs: tr("RCS"), sms: tr("SMS: the phone did not take RCS"), mms: tr("MMS: the phone did not take RCS") };
            for (const one of info.delivered || []) {
                const text = one.state === "accepted" ? (RECEIPTS[one.receipt] || tr("Taken by the network")) : (STATES[one.state] || one.state);
                lines.push({ key: "delivered", label: one.name || tr("Bridge"), text: text + (one.error && one.state !== "accepted" ? ": " + one.error : ""), state: one.state });
                // an RCS bridge's: the way it went (Comms-Messaging §16), RCS or the fallback's SMS or MMS
                if (PATHS[one.path]) lines.push({ key: "path", label: tr("Sent as"), text: PATHS[one.path], path: one.path });
                // the provider refused the cards and their text went (§18.3 phase 13)
                if (one.note === "richRefused") lines.push({ key: "deliveryNote", label: tr("Sent as text"), text: tr("The provider refused the card; its text went instead.") });
                // an RCS bridge's: what can read it, by the way it went (§16.5)
                if (one.path || /^rcs-/.test(one.adapterName || "")) lines.push({ key: "encryption", label: tr("Encryption"), text: chat.rcsDetail(one.path, one.adapterName) });
                // a group text: each recipient's own receipt, a failed one said as such even when the others got it
                for (const number of Object.keys(one.recipients || {}).sort()) {
                    const receipt = one.recipients[number];
                    lines.push({ key: "recipient", label: number, text: receipt === "failed" ? tr("Not delivered") : (RECEIPTS[receipt] || tr("Taken by the network")),
                        state: receipt === "failed" ? "failed" : "accepted" });
                }
            }
            if (info.readBy && info.readBy.off) lines.push({ key: "read", label: tr("Read by"), text: tr("Your read receipts are off, so you do not see who has read it") });
            else if (info.readBy) {
                const names = info.readBy.people.map((person) => person.name);
                const text = !names.length ? tr("No one yet, of those with read receipts on")
                    : names.join(", ") + (info.readBy.more ? " " + tr("and {count} more").replace("{count}", info.readBy.more) : "");
                lines.push({ key: "read", label: tr("Read by"), text, count: info.readBy.count });
            }
            const notes = [];
            if (info.thread) notes.push(tr("A thread has no read receipts."));
            else if (info.readBy && !info.readBy.off) notes.push(tr("People with read receipts off are not listed. Read by means read up to here or later."));
            notes.push(tr("Delivery to each person's devices is not known to the server."));
            lines.push({ key: "note", label: "", text: notes.join(" ") });
            return lines;
        },
        // ---- the E2EE label (Comms-Messaging §6.2, its one table; COM-UI-001) ----
        // A lock means end-to-end encrypted and nothing else (§6.1 rule 6): an encrypted room has it, with a check when every
        // device in it is verified; any other room has the open lock, and a public or record room no banner. A bridge says
        // what its network does (encryption, from its adapter: None, Transport, BridgeDecrypted, EndToEnd), but a bridged
        // room is never end to end on this server: the bridge reads its messages to pass them on. Mail never has a lock
        // (rule 7). subject: a room as get#Room or find#Rooms gives it ({encrypted, roomTypeEnumId, visibilityEnumId, bridges
        // or bridgeNetworks, bridged, bridgeNetworkNames}), a call ({call: {e2ee, phone}}) or mail ({mail: true}); trust: the
        // room's devices when known (chat.trustOf). Answers {state, lock, verified, icon, badge, banner, hint, verify, text}: state
        // verified, encrypted, plain, public, rcs, sms, bridged, call-encrypted, call-phone, call or mail; badge an icon beside
        // the open lock (rcs: the brand's agent is verified, §16.5); lock true only for end
        // to end; icon a Material symbol (none for mail); banner the text at the top of the room ('' for none); hint the
        // composer's ('' for its own); verify whether the banner offers Verify; text the label as one line.
        e2eeLabel(subject, trust) {
            const it = subject || {};
            const label = (state, lock, banner, more) => Object.assign({ state, lock, verified: false, icon: lock ? "sym_o_lock" : "sym_o_lock_open",
                banner, hint: "", verify: false, text: banner }, more || {});
            const sealedText = tr("Messages and calls are end-to-end encrypted. Only people in this chat can read them.");
            if (it.mail) return label("mail", false, tr("Email is not end-to-end encrypted"), { icon: null });
            if (it.call) {
                if (it.call.phone) return label("call-phone", false, tr("Phone calls are not end-to-end encrypted."));
                if (it.call.e2ee) return label("call-encrypted", true, tr("This call is end-to-end encrypted."));
                return label("call", false, tr("This call is encrypted to the server, not end to end."));
            }
            if (it.encrypted) {
                if (trust && trust.allVerified) return label("verified", true, sealedText, { verified: true, text: tr("End-to-end encrypted, every device verified") });
                return label("encrypted", true, sealedText, { verify: true, text: tr("End-to-end encrypted") });
            }
            const bridges = (Array.isArray(it.bridges) ? it.bridges : Array.isArray(it.bridgeNetworks) ? it.bridgeNetworks : []).filter((bridge) => bridge);
            // the composer names what a message goes to: only the bridges that send out (an inbound-only one takes none)
            const outbound = bridges.filter((bridge) => bridge.outbound !== false);
            const smsBridge = bridges.some((bridge) => bridge.network === "sms");
            const others = bridges.filter((bridge) => bridge.network !== "sms");
            const namesOf = (list) => Array.from(new Set(list.map((bridge) => bridge.networkName || bridge.name).filter((name) => name))).join(", ");
            const otherNames = namesOf(outbound.filter((bridge) => bridge.network !== "sms")) || (!bridges.length ? (it.bridgeNetworkNames || []).join(", ") : "");
            // business RCS (§16.5): encrypted in transit only, from the brand's verified agent; its texts' fallback is SMS
            // only an RCS bridge that sends out; another bridge of the room reads its messages too, and is said
            const rcsBridge = outbound.find((bridge) => bridge.network === "rcs");
            if (rcsBridge) {
                const elsewhere = bridges.some((bridge) => bridge.network !== "rcs");
                const detail = chat.rcsDetail(null, rcsBridge.adapterName);
                return label("rcs", false, elsewhere ? detail + " " + tr("The bridge reads messages to pass them on.") : detail,
                    { text: tr("RCS Business - not end-to-end encrypted"), badge: "sym_o_verified", hint: tr("RCS message") });
            }
            const hints = [];
            if (it.roomTypeEnumId === "CrtExternal" || outbound.some((bridge) => bridge.network === "sms")) hints.push(tr("Text message"));
            if (otherNames) hints.push(fillIn(tr("{network} message"), { network: otherNames }));
            if (it.roomTypeEnumId === "CrtExternal" || smsBridge) {
                // a text, and another network's bridge too: both are said
                const banner = others.length ? tr("Not end-to-end encrypted. Texts go through the phone network and the SMS provider, and the bridge reads messages to pass them on.")
                    : tr("Not end-to-end encrypted. Texts go through the phone network and the SMS provider.");
                return label("sms", false, banner, { hint: hints.join(" · ") });
            }
            // a network its adapter says has no E2EE at all (GroupMe: NoneByNetwork; None alone may be an adapter that said
            // nothing): it says so
            if (bridges.length && bridges.every((bridge) => bridge.encryption === "NoneByNetwork") && namesOf(bridges)) {
                return label("bridged", false, fillIn(tr("Not end-to-end encrypted. {network} has no end-to-end encryption, and the bridge reads messages to pass them on."),
                    { network: namesOf(bridges) }), { hint: hints.join(" · ") });
            }
            if (bridges.length || it.bridged) {
                return label("bridged", false, tr("Not end-to-end encrypted on this server. The bridge reads messages to pass them on."), { hint: hints.join(" · ") });
            }
            // public: a public channel, a room any user may join, a record's room for those who see the record; a record room
            // kept to its members is a plain private room
            const visibility = it.visibilityEnumId;
            if (it.roomTypeEnumId === "CrtPublic" || visibility === "CrvUsers" || visibility === "CrvRecord" || (it.roomTypeEnumId === "CrtRecord" && visibility !== "CrvMembers")) {
                return label("public", false, "", { text: tr("Not end-to-end encrypted: the server can read this room") });
            }
            return label("plain", false, tr("Not end-to-end encrypted. The server can read these messages."));
        },
        // The trust of an encrypted room's devices, from moqui.crypto.roomTrust ({verifiedOnly, users: [{userId, identity:
        // {known, verified, changed}, devices: [{deviceId, displayName, verified}]}]}, this device left out): allVerified when
        // every other member has a device and every device is verified; unverified, the devices that are not; leftOut, those
        // the next message does not reach (only verified devices is on); people, by userId {verified (their identity, or every
        // device of theirs), changed (a new identity of someone verified)}: the check by a name (§6.1 rule 3). Unknown (null)
        // is not verified.
        trustOf(answer, me) {
            if (!answer || !Array.isArray(answer.users)) return { known: false, allVerified: false, verifiedOnly: false, unverified: [], leftOut: [], people: {} };
            const unverified = [];
            const people = {};
            let missing = false;
            for (const user of answer.users) {
                const devices = Array.isArray(user.devices) ? user.devices : [];
                const identity = user.identity || {};
                if (!devices.length && user.userId !== me) missing = true;
                for (const device of devices) if (!device.verified) unverified.push({ userId: user.userId, deviceId: device.deviceId, displayName: device.displayName || null });
                people[user.userId] = { verified: !identity.changed && (!!identity.verified || (devices.length > 0 && devices.every((device) => device.verified))),
                    changed: !!identity.changed, devices };
            }
            const verifiedOnly = !!answer.verifiedOnly;
            return { known: true, allVerified: !missing && !unverified.length && answer.users.length > 0, verifiedOnly, unverified,
                leftOut: verifiedOnly ? unverified : [], people };
        },
        // whom a Verify asks first: the first other person with a device not verified, else null (only the user's own:
        // My devices)
        verifyTarget(trust, me) {
            const device = ((trust && trust.unverified) || []).find((one) => one.userId !== me);
            return device ? device.userId : null;
        },
        // Who cannot read what this user sends now (COM-UI-013): the people of devices (chat.trustOf's leftOut, or
        // moqui.crypto.leftOut's), by name (nameOf(userId)), and "your other devices" for the user's own; '' for none.
        leftOutText(devices, me, nameOf) {
            const list = Array.isArray(devices) ? devices : [];
            if (!list.length) return "";
            const people = Array.from(new Set(list.filter((device) => device.userId !== me).map((device) => (nameOf ? nameOf(device.userId) : null) || device.userId)));
            if (list.some((device) => device.userId === me)) people.push(tr("your other devices"));
            return fillIn(tr("Not readable by {who}: their devices are not verified"), { who: people.join(", ") });
        },
        // An encrypted message this device cannot read: why, when the sender withheld its key from an unverified device
        // (m.room_key.withheld m.unverified, "only verified devices"); sent before this device was set up (setup: setupNeeded
        // says so, Comms-Messaging §6.5); else the key backup or another device's share
        undecryptableText(withheld, senderName, setup) {
            if (withheld === "m.unverified") {
                return fillIn(tr("{name} has not verified this device, so it cannot read this message. Verify each other in this chat (Members, Verify) and compare emoji; then {name}'s next messages can be read here."),
                    { name: senderName || tr("The sender") });
            }
            if (setup) return tr("Sent before this device was set up. Set up this device to read it.");
            return tr("Encrypted message: this device has no key for it yet. Restore your key backup in Settings, My devices, or keep this page open while another of your devices shares it.");
        },
        // ---- a new device (Comms-Messaging §6.5, first sign-in) ----
        // Whether this device must be set up to read what was sent before it, from moqui.crypto.setupStatus's answer: until it
        // can read that, which is when it holds the current backup's key, or (with no backup on the server to receive) it was
        // verified with another of the user's devices, which then forwards keys; and only when the user has a key backup or
        // another device to set it up from (a first device needs none). Verified with a backup there is not enough: the
        // backup key must come too (setupWaiting).
        setupNeeded(status) {
            if (!status || status.restored) return false;
            if (status.verified && !status.backup) return false;
            return !!status.backup || (Array.isArray(status.otherDevices) && status.otherDevices.length > 0);
        },
        // verified with another device, the backup key not here yet: the card says it is getting the keys
        setupWaiting(status) { return !!status && !!status.verified && !!status.backup && !status.restored; },
        // a user's first device with encrypted rooms and no key backup: offer to set one up (once, closed per account)
        backupHinted(status) { return !!status && !status.backup && !chat.setupHintClosed(); },
        setupHintKey() { return moqui.scopedKey("moqui.chat.backupHint." + (chat.me || "")); },
        setupHintClosed() {
            try { return !!root.localStorage && root.localStorage.getItem(chat.setupHintKey()) === "closed"; } catch (err) { return false; }
        },
        closeSetupHint() {
            try { if (root.localStorage) root.localStorage.setItem(chat.setupHintKey(), "closed"); } catch (err) { /* this page only */ }
        },
        // "Not now" on Set up this device: per account and device, in this browser
        setupLaterKey(deviceId) { return moqui.scopedKey("moqui.chat.setupLater." + (chat.me || "") + "." + (deviceId || "")); },
        setupLater(deviceId) {
            try { return !!root.localStorage && root.localStorage.getItem(chat.setupLaterKey(deviceId)) === "later"; } catch (err) { return false; }
        },
        putSetupLater(deviceId) {
            try { if (root.localStorage) root.localStorage.setItem(chat.setupLaterKey(deviceId), "later"); } catch (err) { /* this page only */ }
        },
        // The page makes this browser's crypto device when Chat opens (Comms-Messaging §6.5): in idle time, never in a
        // desktop app window (the shell owns the device) or a browser without WebAssembly or IndexedDB. A failure is logged;
        // Chat goes on, and this browser has no device.
        makeDeviceSoon() {
            const crypto = moqui.crypto;
            if (!crypto || !crypto.ensureDevice || (crypto.elsewhere && crypto.elsewhere())) return false;
            if (typeof root.WebAssembly !== "object" || !root.indexedDB) return false;
            const run = () => crypto.ensureDevice({ automatic: true }).catch((err) => console.info("This browser has no chat crypto device", err));
            if (typeof root.requestIdleCallback === "function") root.requestIdleCallback(run, { timeout: 5000 });
            else setTimeout(run, 1500);
            return true;
        },
        // Before a send (§6.1 rule 3): a verified person's new device or changed identity, as encryptRoom refuses with them
        // (verifiedUserProblem), one line each, by name
        problemsText(problems, nameOf) {
            return (problems || []).map((problem) => {
                const name = (nameOf ? nameOf(problem.userId) : null) || problem.userId;
                if (problem.kind === "changedIdentity") return fillIn(tr("{name} has a new identity: verify them again"), { name });
                return fillIn(tr("{name} has a new device that is not verified: {device}"), { name, device: problem.displayName || problem.deviceId });
            });
        },
        // a room's banner closed by this user (until its state changes), in this browser only, per account
        bannerKey(roomId) { return moqui.scopedKey("moqui.chat.banner." + (chat.me || "") + "." + roomId); },
        bannerClosed(roomId, state) {
            try { return !!root.localStorage && root.localStorage.getItem(chat.bannerKey(roomId)) === state; } catch (err) { return false; }
        },
        closeBanner(roomId, state) {
            try { if (root.localStorage) root.localStorage.setItem(chat.bannerKey(roomId), state); } catch (err) { /* this page only */ }
        },
        // ---- who could not read my message (COM-UI-013), kept in this browser per account so a reload, another room or tab
        // shows it: {rooms: {roomId: {eventSeq: text}}, refs: {clientRef: {roomId, text}}} until the send is answered ----
        LEFT_OUT_MAX: 300,
        leftOutKey() { return moqui.scopedKey("moqui.chat.leftOut." + (chat.me || "")); },
        readLeftOut() {
            let kept = null;
            try { kept = root.localStorage ? JSON.parse(root.localStorage.getItem(chat.leftOutKey()) || "null") : null; } catch (err) { kept = null; }
            return kept && typeof kept === "object" ? { rooms: kept.rooms || {}, refs: kept.refs || {} } : { rooms: {}, refs: {} };
        },
        writeLeftOut(kept) {
            // the newest LEFT_OUT_MAX messages are kept
            const all = [];
            for (const roomId of Object.keys(kept.rooms)) for (const seq of Object.keys(kept.rooms[roomId])) all.push([roomId, seq]);
            for (const [roomId, seq] of all.sort((a, b) => Number(b[1]) - Number(a[1])).slice(chat.LEFT_OUT_MAX)) delete kept.rooms[roomId][seq];
            try { if (root.localStorage) root.localStorage.setItem(chat.leftOutKey(), JSON.stringify(kept)); } catch (err) { /* this page only */ }
            chat.leftOutTick = (chat.leftOutTick || 0) + 1;
            if (chat.emit) chat.emit("leftOut", {});
        },
        // a sealed message's clientRef, before it is sent: who it leaves out
        noteLeftOut(clientRef, roomId, text) {
            if (!clientRef || !text) return;
            const kept = chat.readLeftOut();
            kept.refs[clientRef] = { roomId, text };
            chat.writeLeftOut(kept);
        },
        // a send answered: its note moves to its eventSeq
        leftOutSent(payload) {
            if (!payload || !payload.clientRef) return;
            const kept = chat.readLeftOut();
            const ref = kept.refs[payload.clientRef];
            if (!ref) return;
            const json = payload.json || {};
            const seq = json.event && json.event.eventSeq != null ? json.event.eventSeq : json.eventSeq;
            delete kept.refs[payload.clientRef];
            if (seq != null) (kept.rooms[ref.roomId] = kept.rooms[ref.roomId] || {})[seq] = ref.text;
            chat.writeLeftOut(kept);
        },
        // the notes of a room's messages: {eventSeq: text}
        leftOutOf(roomId) { void chat.leftOutTick; return Object.assign({}, chat.readLeftOut().rooms[roomId] || {}); }
    });
    moqui.chat = chat;

    // ---- scheduled sends and reminders (plans/Comms-Messaging.md phase 15; org.moqui.chat.ChatScheduleServices) ----
    const MINUTE = 60 * 1000;
    Object.assign(chat, {
        // The times offered, at now (millis), in the user's zone (the one the labels use, moqui.l10n): kind "send" (Send
        // later) or "remind" (Remind me), each {key, at}. Send later: in 1 hour, tomorrow morning (8:00) and afternoon
        // (13:00), and Monday morning (8:00) on a Friday or Saturday there; Remind me: in 20 minutes, in 1 hour, tomorrow
        // (9:00). A time in a minute is a whole minute.
        whenChoices(kind, now) {
            const z = l10n();
            const wall = z.zonedWall(now);
            const dayAt = (days, hour) => z.wallMillis(new Date(Date.parse(wall.dayKey + "T00:00:00Z") + days * 86400000).toISOString().slice(0, 10), hour, 0);
            const ahead = (minutes) => Math.ceil((now + minutes * MINUTE) / MINUTE) * MINUTE;
            if (kind === "remind") return [{ key: "minutes20", at: ahead(20) }, { key: "hour", at: ahead(60) }, { key: "tomorrow", at: dayAt(1, 9) }];
            const list = [{ key: "hour", at: ahead(60) }, { key: "tomorrowMorning", at: dayAt(1, 8) }, { key: "tomorrowAfternoon", at: dayAt(1, 13) }];
            if (wall.weekday === 5 || wall.weekday === 6) list.push({ key: "monday", at: dayAt(8 - wall.weekday, 8) });
            return list;
        },
        // A date and time picked (a datetime-local input's yyyy-MM-ddTHH:mm) as a time stamp, read in the user's zone; NaN
        // for anything else.
        wallTime(text) {
            const match = /^(\d{4}-\d\d-\d\d)T(\d\d):(\d\d)/.exec(String(text || ""));
            return match ? l10n().wallMillis(match[1], Number(match[2]), Number(match[3])) : NaN;
        },
        // A time to come as a choice says it, in the user's language and zone: the time today, the weekday and time this week,
        // else the date and time.
        whenText(at, now) {
            const z = l10n(), days = z.dayNumber(Number(at)) - z.dayNumber(Number(now));
            const options = days === 0 ? { hour: "numeric", minute: "2-digit" } : days > 0 && days < 7 ? { weekday: "long", hour: "numeric", minute: "2-digit" }
                : { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" };
            return z.formatZoned(Number(at), options);
        },
        // Whether a time picked may be used: a minute to a year ahead (the server says the same: badTime).
        timeAllowed(at, now) {
            const ahead = Number(at) - now;
            return ahead >= MINUTE && ahead <= 366 * 86400 * 1000;
        },
        // Whether a waiting held row cannot go: its device is gone, or has not been open for a while (it fails when overdue).
        heldStuck(row) { return !!(row && row.status === "scheduled" && (row.deviceGone || row.deviceAway)); },
        // A room's scheduled messages as its bar says them: waiting (to go), failed (not sent, waiting for a review, or held on
        // a device that is gone or away), late (sent late or after a cancel, not yet seen) and next (the soonest time waiting).
        scheduledLine(list, roomId) {
            const rows = (list || []).filter((row) => row && row.chatRoomId === roomId);
            const waiting = rows.filter((row) => row.status === "scheduled" && !chat.heldStuck(row) && !row.reviewNeeded);
            return { waiting: waiting.length, failed: rows.filter((row) => row.status === "failed" || chat.heldStuck(row) || (row.status === "scheduled" && row.reviewNeeded)).length,
                late: rows.filter((row) => row.status === "sent" && (row.sentLate || row.sentAfterCancel)).length,
                next: waiting.reduce((soonest, row) => (soonest == null || row.sendDate < soonest ? row.sendDate : soonest), null), total: rows.length };
        },
        // The pending reminder of a message, or null.
        reminderOf(reminders, roomId, eventSeq) {
            return (reminders || []).find((row) => row && row.chatRoomId === roomId && Number(row.eventSeq) === Number(eventSeq)) || null;
        },
        // This device's held messages (an encrypted room's) that are due and not waiting for a review: it sends them.
        dueHeld(list, deviceId, now) {
            if (!deviceId) return [];
            return (list || []).filter((row) => row && row.held && row.status === "scheduled" && !row.deviceGone && !row.reviewNeeded
                && row.heldByDeviceId === deviceId && Number(row.sendDate) <= now);
        },
        // Drop the drafts this device keeps whose rows it no longer holds waiting (sent, cancelled, failed, gone): list is a
        // read of chatSchedule. Out: the ids dropped.
        pruneHeld(list, deviceId) {
            const all = chat.readHeld();
            const kept = new Set((list || []).filter((row) => row && row.held && row.status === "scheduled" && row.heldByDeviceId === deviceId)
                .map((row) => row.chatScheduledMessageId));
            const dropped = Object.keys(all).filter((id) => !kept.has(id));
            if (dropped.length) {
                dropped.forEach((id) => delete all[id]);
                chat.writeHeld(all);
            }
            return dropped;
        },
        // The drafts this device keeps for its held messages, in local storage under the user and the storage scope, each
        // sealed ({v, iv, data}, moqui.crypto.sealLocal) by its chatScheduledMessageId: never the text itself.
        heldKey() { return moqui.scopedKey("moqui.chat.held." + (chat.me || "")); },
        readHeld() {
            try {
                const value = JSON.parse((root.localStorage && root.localStorage.getItem(chat.heldKey())) || "{}");
                return moqui.isPlainObject(value) ? value : {};
            } catch (err) { return {}; }
        },
        writeHeld(all) {
            try {
                if (Object.keys(all).length) root.localStorage.setItem(chat.heldKey(), JSON.stringify(all));
                else root.localStorage.removeItem(chat.heldKey());
                return true;
            } catch (err) { return false; }
        },
        // Keep draft {body, replyToEventSeq, threadRootEventSeq} as id's, sealed by seal (text to a promise of the sealed);
        // rejects when it cannot be kept.
        keepHeld(id, draft, seal) {
            return seal(JSON.stringify(draft)).then((sealed) => {
                const all = chat.readHeld();
                all[id] = sealed;
                if (!chat.writeHeld(all)) throw { reasonCode: "noStorage", errors: L("This browser cannot keep the message") };
                return sealed;
            });
        },
        // The draft id's, opened by open (the sealed to a promise of its text); null when this device has none.
        openHeld(id, open) {
            const sealed = chat.readHeld()[id];
            return sealed ? open(sealed).then((text) => JSON.parse(text)) : Promise.resolve(null);
        },
        // The user's scheduled messages and reminders (chatSchedule), into state.schedule; a failure is logged and Chat
        // goes on without them.
        loadSchedule(url) {
            return moqui.request(url("chatSchedule"), { params: {} }).then((result) => {
                const json = (result && result.json) || {};
                chat.state.schedule = { scheduledList: json.scheduledList || [], reminderList: json.reminderList || [], loaded: true };
                return chat.state.schedule;
            }, (err) => {
                console.error("The scheduled messages and reminders were not read", err);
                return chat.state.schedule;
            });
        },
        hasHeld(id) { return !!chat.readHeld()[id]; },
        dropHeld(id) {
            const all = chat.readHeld();
            if (all[id]) { delete all[id]; chat.writeHeld(all); }
        },
        // Send one held message of this device's (row as chatSchedule lists it): claimed on the server (claimHeld), its draft
        // opened, encrypted for the room's members, and sent with the row locked and its claim checked (sendHeld: a message
        // cancelled meanwhile does not go). url(name) is a Chat transition's URL; atOnce is Send now or Review and send.
        // Resolves to:
        //   {sent};
        //   {lost}: no draft here, reported, so every device shows it cannot be sent;
        //   {waiting}: not claimed (not due, another try holds it, waiting for a review, or refused), or the claim was lost;
        //   {review, error}: a verified person's new device or identity: the row waits for the sender's decision (error
        //     is the refusal for the problems box: sealRefused);
        //   {failed} or {retry}: a refusal of the encryption or the send, reported as one try (failed after five).
        // Rejects when the server could not be reached: the draft and the row stay for the next look.
        sendHeld(row, url, atOnce) {
            const crypto = moqui.crypto;
            const deviceId = crypto && crypto.thisDeviceId ? crypto.thisDeviceId() : null;
            const id = row && row.chatScheduledMessageId;
            if (!id || !deviceId || deviceId !== row.heldByDeviceId) return Promise.resolve({ waiting: true });
            // a refusal's answer is still an answer (its reasonCode); no answer rejects
            const post = (name, params) => moqui.request(url(name), { method: "POST", params }).then((result) => (result && result.json) || {},
                (err) => (err && err.json ? Object.assign({ refused: true }, err.json) : Promise.reject(err)));
            const lost = () => post("completeHeld", { chatScheduledMessageId: id, deviceId, draftLost: true }).then(() => { chat.dropHeld(id); return { lost: true }; });
            const report = (err) => {
                const text = String((err && (err.errors || err.message)) || "").slice(0, 255);
                if (err && err.reasonCode === "verifiedUserProblem") {
                    return post("reportHeld", { chatScheduledMessageId: id, deviceId, review: "true" }).then(() => ({ review: true, error: err }));
                }
                return post("reportHeld", { chatScheduledMessageId: id, deviceId, reasonCode: (err && err.reasonCode) || "", failText: text })
                    .then((json) => (json.failed ? { failed: true } : { retry: true }));
            };
            if (!chat.hasHeld(id)) return lost();
            return post("claimHeld", { chatScheduledMessageId: id, deviceId, atOnce: atOnce ? "true" : "" }).then((claim) => {
                if (claim.sent) { chat.dropHeld(id); return { sent: true }; }
                if (!claim.claimed) return { waiting: true, reasonCode: claim.reasonCode || null };
                // a draft that does not open with this device's key (its store key changed) is lost
                return chat.openHeld(id, crypto.openLocal).catch((err) => (err && err.name === "OperationError" ? null : Promise.reject(err))).then((draft) => {
                    if (!draft) return lost();
                    return moqui.request(url("room"), { params: { chatRoomId: row.chatRoomId } }).then((result) => {
                        const members = ((result && result.json && result.json.memberList) || []).filter((person) => person && person.userId).map((person) => person.userId);
                        return chat.sealMessage(row.chatRoomId, members, draft.body || "", []);
                    }).then((sealed) => post("sendHeld", { chatScheduledMessageId: id, deviceId, encryptedJson: sealed.encryptedJson,
                        mentionUserIds: sealed.mentionUserIds }).then((json) => {
                        if (json.eventSeq != null) { chat.dropHeld(id); return { sent: true, sentLate: !!json.sentLate }; }
                        // cancelled, or taken by another try, meanwhile: nothing went from here
                        if (json.reasonCode === "notClaimed") return { waiting: true, reasonCode: "notClaimed" };
                        return report(json);
                    }), (err) => (err && err.status === undefined && (err.reasonCode || err.errors || err.message) ? report(err) : Promise.reject(err)));
                });
            });
        }
    });
    chat.state.schedule = { scheduledList: [], reminderList: [], loaded: false };
    // a sealed message's note follows its send, whichever page made it; another tab's notes show here too
    chat.on("sent", (payload) => chat.leftOutSent(payload));
    if (root.addEventListener) root.addEventListener("storage", (event) => {
        if (event && event.key === chat.leftOutKey()) { chat.leftOutTick = (chat.leftOutTick || 0) + 1; chat.emit("leftOut", {}); }
    });

    // ---- events with RSVP (plans/Comms-Messaging.md phase 19; org.moqui.chat.ChatCalServices) ----
    // An event's state is the server's get#CalEventStates: {title, start, end (millis), timeZone, allDay, place, description,
    // revision, cancelled, ended, going, maybe, no, answers {going, maybe, no: [userId]}, mine {answer, remindMinutes} or null}.
    const calWall = (millis, zone) => String(l10n().formatTimestamp(millis, "yyyy-MM-dd HH:mm", zone));
    chat.cal = {
        // The time stamp of a wall clock time (yyyy-MM-dd and HH:mm) in an IANA zone, across a daylight saving change.
        wallIn(dayKey, time, zone) {
            const day = /^(\d{4})-(\d\d)-(\d\d)$/.exec(String(dayKey || ""));
            const clock = /^(\d{1,2}):(\d\d)$/.exec(String(time || "00:00"));
            if (!day || !clock) return NaN;
            const target = Date.UTC(Number(day[1]), Number(day[2]) - 1, Number(day[3]), Number(clock[1]), Number(clock[2]));
            let guess = target;
            for (let pass = 0; pass < 3; pass++) {
                const wall = calWall(guess, zone);
                const seen = Date.UTC(Number(wall.slice(0, 4)), Number(wall.slice(5, 7)) - 1, Number(wall.slice(8, 10)), Number(wall.slice(11, 13)), Number(wall.slice(14, 16)));
                if (seen === target) break;
                guess += target - seen;
            }
            return guess;
        },
        // An event's start and end as the box edits them, in its own zone: {startDay, startTime, endDay, endTime}; an
        // all-day event's end day is its last day.
        boxFields(event) {
            const start = calWall(event.start, event.timeZone);
            const end = calWall(event.allDay ? event.end - 1 : event.end, event.timeZone);
            return { startDay: start.slice(0, 10), startTime: start.slice(11, 16), endDay: end.slice(0, 10), endTime: event.allDay ? "00:00" : end.slice(11, 16) };
        },
        // The start and end millis of what the box holds, in zone; an all-day event is midnight to the midnight after its
        // last day. Null when the box is not filled in or ends before it starts.
        boxTimes(box) {
            const zone = box.timeZone;
            const start = chat.cal.wallIn(box.startDay, box.allDay ? "00:00" : box.startTime, zone);
            let end;
            if (box.allDay) {
                const last = /^(\d{4})-(\d\d)-(\d\d)$/.exec(String(box.endDay || box.startDay || ""));
                if (!last) return null;
                const next = new Date(Date.UTC(Number(last[1]), Number(last[2]) - 1, Number(last[3])) + 86400000).toISOString().slice(0, 10);
                end = chat.cal.wallIn(next, "00:00", zone);
            } else end = chat.cal.wallIn(box.endDay || box.startDay, box.endTime, zone);
            if (!(start > 0) || !(end > start)) return null;
            return { start, end };
        },
        // The time line in the viewer's zone and language: "Sat, Nov 23, 2:00 – 4:00 PM", or the days of an all-day event
        // (the event's own, which a whole day is in).
        when(event) {
            if (!event || event.start == null) return "";
            const z = l10n();
            const day = { weekday: "short", month: "short", day: "numeric" };
            const time = { hour: "numeric", minute: "2-digit" };
            if (event.allDay) {
                const first = String(z.formatTimestamp(event.start, "yyyy-MM-dd", event.timeZone));
                const last = String(z.formatTimestamp(event.end - 1, "yyyy-MM-dd", event.timeZone));
                // a day by its noon there, which is the same day in any zone within 11 hours of it
                const words = (key) => z.formatZoned(chat.cal.wallIn(key, "12:00", event.timeZone), day);
                return first === last ? fillIn(tr("{day}, all day"), { day: words(first) }) : words(first) + " – " + words(last);
            }
            const sameDay = z.dayKey(event.start) === z.dayKey(event.end);
            return z.formatZoned(event.start, Object.assign({}, day, time)) + " – " + (sameDay ? z.formatZoned(event.end, time) : z.formatZoned(event.end, Object.assign({}, day, time)));
        },
        // The event's own time when the viewer's zone shows it otherwise: "14:00 in America/Denver"; else "".
        zoneNote(event) {
            if (!event || event.allDay || !event.timeZone || event.start == null) return "";
            const z = l10n();
            const there = calWall(event.start, event.timeZone);
            const wall = z.zonedWall(event.start);
            const here = wall.dayKey + " " + String(wall.hour).padStart(2, "0") + ":" + String(wall.minute).padStart(2, "0");
            return there === here ? "" : fillIn(tr("{time} in {zone}"), { time: there.slice(11, 16), zone: event.timeZone });
        },
        // "5 going · 2 maybe · 1 can't go", or "No answers yet".
        countLine(event) {
            if (!event) return "";
            const parts = [];
            if (event.going) parts.push(fillIn(tr("{count} going"), { count: event.going }));
            if (event.maybe) parts.push(fillIn(tr("{count} maybe"), { count: event.maybe }));
            if (event.no) parts.push(fillIn(tr("{count} can't go"), { count: event.no }));
            return parts.length ? parts.join(" · ") : tr("No answers yet");
        },
        canAnswer(event, now) { return !!event && !event.cancelled && !(event.end != null && event.end <= now); },
        // The reminders offered (minutes before the start), only those still more than a minute ahead; 0 is none.
        remindChoices(event, now) {
            const choices = [{ minutes: 15, label: tr("15 minutes before") }, { minutes: 60, label: tr("1 hour before") }, { minutes: 1440, label: tr("1 day before") }];
            return [{ minutes: 0, label: tr("No reminder") }].concat(choices.filter((one) => event && event.start - one.minutes * MINUTE > now + MINUTE));
        },
        myAnswer(event) { return event && event.mine ? event.mine.answer || null : null; }
    };

    // ---- stickers and locations (plans/Comms-Messaging.md phase 20; org.moqui.chat.ChatStickerServices, ChatLocationServices) ----
    const fixed6 = (number) => Number(number).toFixed(6);
    chat.place = {
        // A place from what a person typed or pasted: coordinates ("40.7452, -111.875", or degrees, minutes and seconds
        // with N/S/E/W), a geo: URI, or an OpenStreetMap, Google Maps or Apple Maps link (its coordinates read from the
        // link's text, nothing fetched); any other text is an address. {latitude, longitude} or {address}, or null.
        parse(text) {
            const input = String(text == null ? "" : text).trim();
            if (!input) return null;
            const inRange = (lat, lon) => Number.isFinite(lat) && Number.isFinite(lon) && Math.abs(lat) <= 90 && Math.abs(lon) <= 180;
            const pair = (lat, lon) => (inRange(lat, lon) ? { latitude: Number(fixed6(lat)), longitude: Number(fixed6(lon)) } : null);
            let match = /^geo:\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)/i.exec(input);
            if (match) {
                const at = pair(Number(match[1]), Number(match[2]));
                const query = /[?&]q=([^&]*)/.exec(input);
                if (at && (at.latitude !== 0 || at.longitude !== 0)) return at;
                if (query) return chat.place.parse(decodeURIComponent(query[1].replace(/\+/g, " ")).replace(/\(.*\)$/, "").trim());
                return at;
            }
            if (/^https?:\/\//i.test(input)) {
                const patterns = [/[?&#]mlat=(-?\d+(?:\.\d+)?)&mlon=(-?\d+(?:\.\d+)?)/, /@(-?\d+(?:\.\d+)?),(-?\d+(?:\.\d+)?)/,
                    /[?&](?:q|ll|query|sll)=(-?\d+(?:\.\d+)?)(?:,|%2C)\s*(-?\d+(?:\.\d+)?)/i, /#map=\d+\/(-?\d+(?:\.\d+)?)\/(-?\d+(?:\.\d+)?)/];
                for (const pattern of patterns) {
                    match = pattern.exec(input);
                    if (match) return pair(Number(match[1]), Number(match[2]));
                }
                return null;
            }
            match = /^(-?\d{1,2}(?:\.\d+)?)\s*[,;\s]\s*(-?\d{1,3}(?:\.\d+)?)$/.exec(input);
            if (match) return pair(Number(match[1]), Number(match[2]));
            // degrees, minutes and seconds: 40°44'42.8"N 111°52'30.0"W
            const dms = /^(\d{1,2})°\s*(\d{1,2})?['′]?\s*(\d{1,2}(?:\.\d+)?)?["″]?\s*([NS])[,\s]+(\d{1,3})°\s*(\d{1,2})?['′]?\s*(\d{1,2}(?:\.\d+)?)?["″]?\s*([EW])$/i.exec(input);
            if (dms) {
                const part = (deg, min, sec, hemi) => (Number(deg) + Number(min || 0) / 60 + Number(sec || 0) / 3600) * (/[SW]/i.test(hemi) ? -1 : 1);
                return pair(part(dms[1], dms[2], dms[3], dms[4]), part(dms[5], dms[6], dms[7], dms[8]));
            }
            return { address: input.replace(/\s+/g, " ").slice(0, 300) };
        },
        // the map link: OpenStreetMap at the coordinates, or its search for the address (as ChatLocation.js linkOf)
        osmUrl(place) {
            if (!place) return "";
            if (place.latitude != null && place.longitude != null) {
                const lat = fixed6(place.latitude), lon = fixed6(place.longitude);
                return "https://www.openstreetmap.org/?mlat=" + lat + "&mlon=" + lon + "#map=17/" + lat + "/" + lon;
            }
            return place.address ? "https://www.openstreetmap.org/search?query=" + encodeURIComponent(place.address) : "";
        },
        // the geo: URI the phone's maps app opens (RFC 5870, with Android's q and its label)
        geoUri(place) {
            if (!place) return "";
            const label = place.label ? "(" + encodeURIComponent(place.label) + ")" : "";
            if (place.latitude != null && place.longitude != null) {
                const at = fixed6(place.latitude) + "," + fixed6(place.longitude);
                return "geo:" + at + "?q=" + at + label;
            }
            return place.address ? "geo:0,0?q=" + encodeURIComponent(place.address) : "";
        },
        // the words of a place, the same as the server's (ChatLocation.js fallbackText): an encrypted room's message
        fallback(place) {
            if (!place) return "";
            const name = [place.label, place.address].filter((word) => word && String(word).trim()).join(", ");
            const coordinates = place.latitude != null ? fixed6(place.latitude) + ", " + fixed6(place.longitude) : "";
            return [fillIn(tr("Location: {name}"), { name: name || coordinates || tr("a place") }), chat.place.osmUrl(place)].filter((line) => line).join("\n");
        },
        coordinatesText(place) {
            if (!place || place.latitude == null) return "";
            const accuracy = place.accuracy != null ? " ± " + Math.round(Number(place.accuracy)) + " m" : "";
            return fixed6(place.latitude).replace(/0+$/, "").replace(/\.$/, "") + ", " + fixed6(place.longitude).replace(/0+$/, "").replace(/\.$/, "") + accuracy;
        },
        // a decrypted message's place, checked as the server checks a sent one (or null)
        clean(given) {
            if (!given || typeof given !== "object") return null;
            const out = {};
            if (given.latitude != null || given.longitude != null) {
                const lat = Number(given.latitude), lon = Number(given.longitude);
                if (!(Number.isFinite(lat) && Number.isFinite(lon) && Math.abs(lat) <= 90 && Math.abs(lon) <= 180)) return null;
                out.latitude = Number(fixed6(lat));
                out.longitude = Number(fixed6(lon));
            }
            if (given.label) out.label = String(given.label).slice(0, 100);
            if (given.address) out.address = String(given.address).slice(0, 300);
            if (given.accuracy != null && Number.isFinite(Number(given.accuracy))) out.accuracy = Math.round(Number(given.accuracy));
            if (out.latitude == null && !out.address) return null;
            out.source = ["here", "typed", "network"].includes(given.source) ? given.source : "typed";
            return out;
        }
    };
    chat.stickers = {
        // the stickers whose emoji or description has the words, across the packs
        search(packs, text) {
            const term = String(text || "").trim().toLowerCase();
            const all = [];
            (packs || []).forEach((pack) => (pack.stickers || []).forEach((sticker) => all.push(Object.assign({ chatStickerPackId: pack.chatStickerPackId }, sticker))));
            if (!term) return all;
            return all.filter((sticker) => String(sticker.emoji || "").includes(term) || String(sticker.altText || "").toLowerCase().includes(term));
        },
        recentKey(userId) { return moqui.scopedKey ? moqui.scopedKey("moqui.chat.recentStickers." + (userId || "")) : "moqui.chat.recentStickers." + (userId || ""); },
        // the ids of the stickers sent last, newest first, at most 24 (the page's storage; none when it has none)
        recent(userId) {
            try {
                const list = JSON.parse((root.localStorage && root.localStorage.getItem(chat.stickers.recentKey(userId))) || "[]");
                return Array.isArray(list) ? list.filter((id) => typeof id === "string").slice(0, 24) : [];
            } catch (err) { return []; }
        },
        remember(userId, chatStickerId) {
            const list = [chatStickerId].concat(chat.stickers.recent(userId).filter((id) => id !== chatStickerId)).slice(0, 24);
            try { if (root.localStorage) root.localStorage.setItem(chat.stickers.recentKey(userId), JSON.stringify(list)); } catch (err) { /* no storage: none kept */ }
            return list;
        },
        // a decrypted message's sticker, checked (or null)
        clean(given) {
            if (!given || typeof given !== "object") return null;
            const number = (value) => (value == null || !Number.isFinite(Number(value)) ? null : Math.max(1, Math.min(512, Math.round(Number(value)))));
            return { chatStickerId: given.chatStickerId ? String(given.chatStickerId).slice(0, 40) : null, emoji: given.emoji ? String(given.emoji).slice(0, 16) : null,
                altText: given.altText ? String(given.altText).slice(0, 80) : null, packName: given.packName ? String(given.packName).slice(0, 60) : null,
                width: number(given.width), height: number(given.height) };
        }
    };

    // ---- screen security, one-time codes and photos (plans/Comms-Messaging.md phase 21; org.moqui.chat.ChatPrivacyServices) ----
    chat.screen = {
        // whether a room asks for Android's FLAG_SECURE: the whole app's setting, an encrypted room with "in encrypted
        // rooms", or the member's own switch for the room
        wanted(setting, room, member) {
            if (setting === "app") return true;
            if (!room || !room.encrypted) return !!(member && member.screenSecure === "Y");
            return setting === "encrypted" || !!(member && member.screenSecure === "Y");
        },
        // a reason on or off through the app host (phase 16's ScreenSecurity); whether the flag holds (false on the web)
        set(reason, on) {
            const host = root.moquiDesktop;
            if (!host || typeof host.secureScreen !== "function") return Promise.resolve(false);
            return Promise.resolve(host.secureScreen(reason, on === true)).then((held) => held === true, () => false);
        },
        available() { return !!(root.moquiDesktop && typeof root.moquiDesktop.secureScreen === "function"); },
        // the whole app's reason of this account (its storage scope): each account on the phone holds its own, so any one
        // with "everywhere" keeps the flag on, and turning it off there takes only that account's away
        appReason() {
            const host = root.moquiDesktop;
            const scope = typeof moqui.storageScope === "string" ? moqui.storageScope : (host && host.storageScope) || "";
            return ("app:" + String(scope).replace(/[^A-Za-z0-9_-]/g, "")).slice(0, 64);
        },
        // the whole app's setting applied at once, on or off (CHAT_SCREEN_SECURITY: its preference applier, and Chat)
        applyApp(setting) { return chat.screen.set(chat.screen.appReason(), setting === "app"); }
    };
    chat.otp = {
        // a message's code chip: "Code 482913", and when it is to disappear "Deletes tomorrow at 15:40"
        line(event) { return event && event.otp && event.otp.code ? fillIn(tr("Code {code}"), { code: event.otp.code }) : ""; },
        copy(code) {
            if (!code || !root.navigator || !root.navigator.clipboard) return Promise.resolve(false);
            return root.navigator.clipboard.writeText(String(code)).then(() => true, () => false);
        }
    };
    chat.photos = {
        // a still photo the composer may make smaller: JPEG, PNG or WebP (a GIF may move; a video is no photo)
        compressible(file) { return !!file && /^image\/(jpeg|png|webp)$/i.test(file.type || "") && Number(file.size || 0) > 0; },
        // the sizes and qualities tried, the long side first: 2048 px at 0.85, then smaller and lower until it fits maxBytes
        steps(maxBytes) {
            const all = [[2048, 0.85], [2048, 0.7], [1600, 0.7], [1600, 0.55], [1280, 0.55], [1024, 0.5]];
            return maxBytes > 0 ? all : all.slice(0, 1);
        },
        // The photo drawn again in the browser (createImageBitmap with its orientation, a canvas, toBlob), so its metadata (its
        // GPS among it: EXIF, and a PNG's eXIf, tEXt, iTXt and zTXt chunks) never goes: a JPEG or WebP becomes a JPEG of at
        // most 2048 px a side, also when that is no smaller; a PNG stays a PNG (a screenshot's text stays sharp), made
        // smaller only past 2048 px. With maxBytes it steps down until it fits. The file as it was only when it is no still
        // photo or the browser cannot read it (HEIC on most): the composer then says it goes as it is.
        compress(file, options) {
            const opts = options || {};
            if (!chat.photos.compressible(file) || typeof root.createImageBitmap !== "function") return Promise.resolve(file);
            const maxBytes = Number(opts.maxBytes || 0);
            const png = /png$/i.test(file.type);
            return root.createImageBitmap(file, { imageOrientation: "from-image" }).then((bitmap) => {
                const encode = (side, quality, asPng) => {
                    const scale = Math.min(1, side / Math.max(bitmap.width, bitmap.height));
                    const width = Math.max(1, Math.round(bitmap.width * scale)), height = Math.max(1, Math.round(bitmap.height * scale));
                    const canvas = typeof root.OffscreenCanvas === "function" ? new root.OffscreenCanvas(width, height)
                        : Object.assign(root.document.createElement("canvas"), { width, height });
                    const context = canvas.getContext("2d");
                    if (!asPng) { context.fillStyle = "#fff"; context.fillRect(0, 0, width, height); }
                    context.drawImage(bitmap, 0, 0, width, height);
                    const type = asPng ? "image/png" : "image/jpeg";
                    if (canvas.convertToBlob) return canvas.convertToBlob({ type, quality });
                    return new Promise((resolve) => canvas.toBlob(resolve, type, quality));
                };
                // a PNG stays a PNG while it fits; past the limit it goes as a JPEG, which steps down
                const steps = chat.photos.steps(maxBytes);
                const tryStep = (index, asPng) => encode(steps[index][0], steps[index][1], asPng).then((blob) => {
                    if (blob && maxBytes > 0 && blob.size > maxBytes) {
                        if (asPng) return tryStep(0, false);
                        if (index + 1 < steps.length) return tryStep(index + 1, false);
                    }
                    return blob;
                });
                return tryStep(0, png).then((blob) => {
                    if (bitmap.close) bitmap.close();
                    if (!blob) return file;
                    const asPng = /png$/i.test(blob.type || "");
                    const name = String(file.name || "photo").replace(/\.[^.]*$/, "") + (asPng ? ".png" : ".jpg");
                    return new root.File([blob], name, { type: blob.type || (asPng ? "image/png" : "image/jpeg") });
                });
            }, () => file);
        },
        // the most bytes one more photo may have so the message's files stay within a text's total limit: the limit shared
        // by the files the message will have (0 for no limit)
        budget(maxBytes, fileCount) { return maxBytes > 0 ? Math.floor(maxBytes / Math.max(1, fileCount)) : 0; }
    };
    // the account's whole-app screen security, once the shell has its preferences (phase 21)
    try {
        const prefs = (moqui.config && moqui.config.preferences) || {};
        if (prefs.CHAT_SCREEN_SECURITY) chat.screen.applyApp(prefs.CHAT_SCREEN_SECURITY);
    } catch (err) { /* no config yet: Chat applies it when it opens */ }

    // ---- the components the shell writes (registered by WebrootVue.qvt.js) ----

    const localizeTree = (value) => {
        if (typeof value === "string") return L(value);
        if (!value || typeof value !== "object" || Array.isArray(value)) return value;
        const out = {};
        Object.keys(value).forEach((key) => { out[key] = localizeTree(value[key]); });
        return out;
    };
    const labelsOf = (vm) => Object.assign({}, localizeTree(chat.labels), vm.$root && vm.$root.labels);
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

    // A verification going on (an item of moqui.crypto.state.verifications): with one of this user's own devices (My devices)
    // or with another person (Chat's Verify, COM-UI-013): who, then waiting, the emoji to compare or a QR code, the steps
    // (Accept, Compare emoji, Show a QR code, Scan their code, They match, They differ, Cancel), and how it ended, with
    // Try again (the retry event, when the parent can ask again) and Dismiss. A completed emoji comparison with another
    // person verifies their device, and with cross-signing their identity, so their signed devices too.
    components["m-chat-verification"] = {
        name: "mChatVerification",
        props: { flow: { type: Object, required: true }, who: String, canRetry: Boolean },
        emits: ["retry", "step"],
        computed: {
            v() { return this.flow; },
            title() { return (this.v.weStarted ? tr("Verifying") : tr("Asked to verify")) + " " + (this.who || this.v.otherDeviceId || this.v.otherUserId); },
            canScan() { return typeof root.BarcodeDetector === "function" && !!(root.navigator && root.navigator.mediaDevices && root.navigator.mediaDevices.getUserMedia); },
            endedText() { return this.v.timedOut ? tr("The verification timed out: the other device did not answer in time.") : tr("Cancelled"); }
        },
        methods: {
            t: tr,
            timeLeft(ms) {
                const seconds = Math.max(0, Math.round(Number(ms) / 1000));
                return "(" + (seconds >= 60 ? Math.ceil(seconds / 60) + " " + tr("min left") : seconds + " " + tr("s left")) + ")";
            },
            step(action, arg) {
                moqui.crypto.verification(this.v.flowId, action, arg).then(() => this.$emit("step", action),
                    (err) => moqui.notifyMessages([], (err && err.errors) || tr("The verification step failed")));
            },
            dismiss() { if (moqui.crypto && moqui.crypto.dismissVerification) moqui.crypto.dismissVerification(this.v.flowId); },
            retry() { this.dismiss(); this.$emit("retry", this.v); },
            // the other device's QR code read with the camera (BarcodeDetector), its bytes handed to the verification
            scan() {
                const detector = new root.BarcodeDetector({ formats: ["qr_code"] });
                root.navigator.mediaDevices.getUserMedia({ video: { facingMode: "environment" } }).then((stream) => {
                    const video = root.document.createElement("video");
                    video.srcObject = stream;
                    video.play();
                    const stop = () => stream.getTracks().forEach((track) => track.stop());
                    const started = Date.now();
                    const look = () => detector.detect(video).then((codes) => {
                        if (codes.length) { stop(); this.step("scanned", Array.from(codes[0].rawValue, (ch) => ch.charCodeAt(0) & 0xff)); }
                        else if (Date.now() - started < 30000) setTimeout(look, 300);
                        else stop();
                    }, () => setTimeout(look, 300));
                    look();
                }, () => moqui.notifyMessages([], tr("The camera is not available")));
            }
        },
        template:
            '<q-card flat bordered class="q-pa-md m-chat-verification" :data-flow="v.flowId" :data-user="v.otherUserId">' +
                '<div class="text-subtitle1">{{title}}</div>' +
                '<div v-if="v.done" class="text-positive m-chat-verification-done">{{t(\'Verified\')}}</div>' +
                '<template v-else-if="v.cancelled">' +
                    '<div class="m-muted m-chat-verification-ended">{{endedText}} <span v-if="v.cancelReason && !v.timedOut">({{v.cancelReason}})</span></div>' +
                    '<div class="row q-gutter-xs q-mt-xs">' +
                        '<q-btn v-if="canRetry" unelevated no-caps color="primary" :label="t(\'Try again\')" class="m-chat-verification-retry" @click="retry"></q-btn>' +
                        '<q-btn flat no-caps :label="t(\'Dismiss\')" @click="dismiss"></q-btn>' +
                    '</div>' +
                '</template>' +
                '<template v-else>' +
                    '<div v-if="v.started && !v.emojis && !v.qrImage && !v.scanned" class="m-muted q-my-sm m-chat-verification-waiting" role="status">' +
                        '{{t(\'Waiting for the other device…\')}}<span v-if="v.remainingMs"> {{timeLeft(v.remainingMs)}}</span></div>' +
                    '<div v-else-if="v.confirmed" class="m-muted q-my-sm m-chat-verification-waiting" role="status">{{t(\'Waiting for the other device to confirm\')}}</div>' +
                    '<div v-if="v.emojis" class="row q-gutter-md q-my-sm m-chat-verification-emoji">' +
                        '<div v-for="e in v.emojis" :key="e.description" class="column items-center"><span style="font-size:32px">{{e.symbol}}</span><span class="text-caption">{{t(e.description)}}</span></div>' +
                    '</div>' +
                    '<div v-if="v.emojis" class="text-caption m-muted">{{t(\'Do the same emoji show on the other device, in the same order?\')}}</div>' +
                    '<img v-if="v.qrImage && !v.emojis" :src="v.qrImage" alt="QR" style="width:220px;height:220px" class="q-my-sm">' +
                    '<div class="row q-gutter-xs q-mt-xs">' +
                        '<q-btn v-if="!v.weStarted && v.phase === \'requested\'" unelevated no-caps color="primary" :label="t(\'Accept\')" class="m-chat-verification-accept" @click="step(\'accept\')"></q-btn>' +
                        '<q-btn v-if="v.ready && !v.emojis" flat no-caps icon="sym_o_mood" :label="t(\'Compare emoji\')" class="m-chat-verification-sas" @click="step(\'sas\')"></q-btn>' +
                        '<q-btn v-if="v.ready && !v.emojis && !v.qrImage" flat no-caps icon="sym_o_qr_code_2" :label="t(\'Show a QR code\')" @click="step(\'showQr\')"></q-btn>' +
                        '<q-btn v-if="v.ready && !v.emojis && canScan" flat no-caps icon="sym_o_qr_code_scanner" :label="t(\'Scan their code\')" @click="scan"></q-btn>' +
                        '<q-btn v-if="v.scanned" unelevated no-caps color="primary" :label="t(\'They scanned it\')" @click="step(\'qrConfirm\')"></q-btn>' +
                        '<q-btn v-if="v.emojis && !v.confirmed" unelevated no-caps color="primary" :label="t(\'They match\')" class="m-chat-verification-match" @click="step(\'confirm\')"></q-btn>' +
                        '<q-btn v-if="v.emojis && !v.confirmed" flat no-caps color="negative" :label="t(\'They differ\')" @click="step(\'mismatch\')"></q-btn>' +
                        '<q-btn flat no-caps :label="t(\'Cancel\')" @click="step(\'cancel\')"></q-btn>' +
                    '</div>' +
                '</template>' +
            '</q-card>'
    };

    // Set up this device (Comms-Messaging §6.5, first sign-in on a new device): what was sent before it was added is read
    // here after a restore of the key backup with the recovery key, or a verification with another of the user's devices,
    // which then sends this one the backup key (moqui.crypto's secrets) or the room keys it holds. status is
    // moqui.crypto.setupStatus's answer; the parent reads it again on refresh (after a restore, room keys or a trust change)
    // and closes the card once chat.setupNeeded says no. A full-screen dialog on a phone, 480 px on a desktop; Not now emits
    // later (the parent keeps it per account and device).
    components["m-chat-device-setup"] = {
        name: "mChatDeviceSetup",
        props: { modelValue: Boolean, status: Object },
        emits: ["update:modelValue", "refresh", "later"],
        data() { return { recoveryKey: "", busy: false, verifiedAt: 0, now: Date.now(), timer: null }; },
        computed: {
            phone() { return !!(this.$q && this.$q.screen && this.$q.screen.lt.sm); },
            hasBackup() { return !!(this.status && this.status.backup); },
            // the user's other devices in use (seen lately: the server's active), each by its name and when it was last seen
            devices() { return ((this.status && this.status.otherDevices) || []).filter((d) => d.active !== false); },
            me() { return (root.moqui && root.moqui.webrootVue && root.moqui.webrootVue.userId) || chat.me || null; },
            // the verifications with the user's own devices (another person's are Chat's)
            flows() { return ((moqui.crypto && moqui.crypto.state.verifications) || []).filter((v) => v.otherUserId === this.me); },
            done() { return !chat.setupNeeded(this.status); },
            // verified, and the keys not here yet: they come from the other device while it is open (two minutes, then said)
            waiting() { return !this.done && (!!this.verifiedAt || chat.setupWaiting(this.status)); },
            waitedLong() { return this.waiting && this.now - (this.verifiedAt || this.now) > 120000; }
        },
        watch: {
            flows(list) {
                if (!this.verifiedAt && (list || []).some((v) => v.done)) {
                    this.verifiedAt = Date.now();
                    this.$emit("refresh");
                }
            },
            waiting(on) {
                clearInterval(this.timer);
                if (on && !this.verifiedAt) this.verifiedAt = Date.now();
                this.timer = on ? setInterval(() => { this.now = Date.now(); this.$emit("refresh"); }, 10000) : null;
            }
        },
        beforeUnmount() { clearInterval(this.timer); },
        methods: {
            t: tr,
            nameOf(device) { return device.displayName || device.deviceId; },
            whenText(millis) { return millis ? chat.dateTime(Number(millis)) : ""; },
            whoOf(flow) { const device = this.devices.find((d) => d.deviceId === flow.otherDeviceId); return device ? this.nameOf(device) : flow.otherDeviceId; },
            canRetry(flow) { return this.devices.some((d) => d.deviceId === flow.otherDeviceId); },
            setOpen(value) { this.$emit("update:modelValue", value); },
            close() { this.setOpen(false); },
            later() { this.$emit("later"); this.close(); },
            restore() {
                if (!String(this.recoveryKey || "").trim() || this.busy) return;
                this.busy = true;
                moqui.crypto.restoreBackup(this.recoveryKey).then((out) => {
                    this.busy = false;
                    this.recoveryKey = "";
                    if (moqui.notifyMessages) moqui.notifyMessages([{ type: "success", message: fillIn(tr("This device is set up: {count} keys restored"), { count: out.imported }) }]);
                    this.$emit("refresh");
                }, (err) => {
                    this.busy = false;
                    const code = err && err.reasonCode;
                    const text = code === "badRecoveryKey" ? tr("This is not a recovery key. Check it and try again.")
                        : code === "wrongRecoveryKey" ? tr("This recovery key does not open your key backup.")
                        : (err && err.errors) || tr("The keys were not restored");
                    if (moqui.notifyMessages) moqui.notifyMessages([], text);
                });
            },
            // the keys asked of the other devices again, and two more minutes to wait
            askAgain() {
                this.verifiedAt = Date.now();
                this.now = this.verifiedAt;
                if (moqui.crypto && moqui.crypto.requestSecrets) moqui.crypto.requestSecrets().catch(() => null);
            },
            useRecoveryKey() {
                const input = this.$el && this.$el.querySelector ? this.$el.querySelector("input[name='setupRecoveryKey']") : null;
                const field = input || (document.querySelector && document.querySelector(".m-chat-device-setup input[name='setupRecoveryKey']"));
                if (field && field.focus) field.focus();
            },
            verify(device) {
                this.verifiedAt = 0;
                moqui.crypto.verifyDevice(this.me, device.deviceId).catch((err) => moqui.notifyMessages && moqui.notifyMessages([], (err && err.errors) || tr("The verification did not start")));
            },
            retry(flow) {
                const device = this.devices.find((d) => d.deviceId === flow.otherDeviceId);
                if (device) this.verify(device);
            }
        },
        template:
            '<q-dialog :model-value="modelValue" :maximized="phone" @update:model-value="setOpen">' +
            '<q-card class="m-chat-device-setup column no-wrap" :style="phone ? \'\' : \'width:480px;max-width:95vw\'">' +
                '<q-card-section class="row items-center no-wrap q-pb-none">' +
                    '<q-icon name="sym_o_phonelink_lock" size="24px" class="text-primary q-mr-sm"></q-icon>' +
                    '<div class="text-h6 col">{{t(\'Set up this device\')}}</div>' +
                    '<q-btn flat round dense icon="sym_o_close" :aria-label="t(\'Close\')" @click="close"></q-btn>' +
                '</q-card-section>' +
                '<q-card-section class="col scroll" style="min-height:0">' +
                    '<div v-if="done" class="text-positive m-chat-device-setup-done" role="status">{{t(\'This device is set up: it reads your encrypted messages.\')}}</div>' +
                    '<template v-else>' +
                    '<div class="text-body2 q-mb-md">{{t(\'Messages sent before this device was added were encrypted for your other devices. To read them here, use one of these.\')}}</div>' +
                    '<div v-if="hasBackup" class="q-mb-md m-chat-device-setup-key">' +
                        '<div class="text-subtitle2">{{t(\'Your recovery key\')}}</div>' +
                        '<div class="text-caption m-muted q-mb-xs">{{t(\'The key you kept when you set up key backup.\')}}</div>' +
                        '<div class="row no-wrap items-center q-gutter-sm">' +
                            '<div class="col" style="min-width:0"><q-input v-model="recoveryKey" dense outlined autocomplete="off" spellcheck="false" name="setupRecoveryKey" :label="t(\'Recovery key\')" @keydown.enter.prevent="restore"></q-input></div>' +
                            '<q-btn unelevated no-caps color="primary" :label="t(\'Restore\')" :loading="busy" :disable="!recoveryKey.trim()" class="m-chat-device-setup-restore" @click="restore"></q-btn>' +
                        '</div>' +
                    '</div>' +
                    '<div v-if="devices.length" class="m-chat-device-setup-devices">' +
                        '<div class="text-subtitle2">{{t(\'Another of your devices\')}}</div>' +
                        '<div class="text-caption m-muted q-mb-xs">{{t(\'Verify this device from one you use, with Chat or My devices open there: you compare emoji on both.\')}}</div>' +
                        '<q-list dense separator bordered class="rounded-borders">' +
                            '<q-item v-for="d in devices" :key="d.deviceId" class="m-chat-device-setup-device" :data-device="d.deviceId">' +
                                '<q-item-section style="min-width:0"><q-item-label class="ellipsis">{{nameOf(d)}}</q-item-label>' +
                                    '<q-item-label caption v-if="d.lastSeenDate">{{t(\'last seen\')}} {{whenText(d.lastSeenDate)}}</q-item-label></q-item-section>' +
                                '<q-item-section side><q-btn flat dense no-caps color="primary" icon="sym_o_verified_user" :label="t(\'Verify\')" class="m-chat-device-setup-verify" @click="verify(d)"></q-btn></q-item-section>' +
                            '</q-item>' +
                        '</q-list>' +
                    '</div>' +
                    '<m-chat-verification v-for="v in flows" :key="v.flowId" :flow="v" :who="whoOf(v)" :can-retry="canRetry(v)" class="q-mt-md" @retry="retry(v)"></m-chat-verification>' +
                    '<div v-if="waiting && !waitedLong" class="m-muted q-mt-sm m-chat-device-setup-waiting" role="status">{{t(\'Verified. Getting your keys from the other device…\')}}</div>' +
                    '<div v-if="waitedLong" class="q-mt-sm m-chat-device-setup-slow" role="status"><div class="text-negative">{{t(\'Your other device did not send the keys. Open Chat there and keep it open, then try again, or use your recovery key.\')}}</div>' +
                        '<div class="row q-gutter-xs q-mt-xs"><q-btn unelevated no-caps color="primary" :label="t(\'Try again\')" class="m-chat-device-setup-again" @click="askAgain"></q-btn>' +
                        '<q-btn v-if="hasBackup" flat no-caps color="primary" :label="t(\'Use your recovery key\')" class="m-chat-device-setup-use-key" @click="useRecoveryKey"></q-btn></div></div>' +
                    '</template>' +
                '</q-card-section>' +
                '<q-card-actions align="right">' +
                    '<q-btn v-if="!done" flat no-caps :label="t(\'Not now\')" class="m-chat-device-setup-later" @click="later"></q-btn>' +
                    '<q-btn v-else unelevated no-caps color="primary" :label="t(\'Done\')" @click="close"></q-btn>' +
                '</q-card-actions>' +
            '</q-card>' +
            '</q-dialog>'
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

    // ---- one person across networks (plans/Comms-Messaging.md §11.1, phase 22) ----
    // A list of a person's ways (moqui.chat.waysOf): each with its network, where it goes and its E2EE label, the best marked.
    const waysListTemplate =
        '<q-list class="m-chat-ways" style="min-width:260px">' +
            '<q-item v-for="way in ways" :key="way.key" :clickable="way.usable && !way.current" :disable="!way.usable" :active="way.current" ' +
                    ':data-way="way.network + \':\' + way.kind" v-close-popup="closes" @click="choose(way)">' +
                '<q-item-section avatar><q-icon :name="way.icon"></q-icon></q-item-section>' +
                '<q-item-section>' +
                    '<q-item-label>{{way.name}}<q-badge v-if="way.best" color="primary" class="q-ml-xs m-chat-way-best">{{t(\'Best\')}}</q-badge>' +
                        '<span v-if="way.current" class="text-caption m-muted q-ml-xs">{{t(\'Here\')}}</span></q-item-label>' +
                    '<q-item-label v-if="way.detail" caption class="ellipsis">{{way.detail}}</q-item-label>' +
                    '<q-item-label caption class="row no-wrap items-center m-chat-way-label">' +
                        '<q-icon v-if="way.label.icon" :name="way.label.icon" size="14px" class="q-mr-xs"></q-icon>' +
                        '<span class="ellipsis">{{way.label.text || way.label.banner}}</span></q-item-label>' +
                '</q-item-section>' +
                '<q-item-section v-if="way.reason" side class="text-caption">{{way.reason}}</q-item-section>' +
            '</q-item>' +
        '</q-list>';

    // The network button of a person's room (at the start of its composer): the room's network, and a choice of the
    // person's ways, best first; choose emits the way, and the room opens or makes it and moves the draft there. A sheet
    // from the bottom on a phone, a menu above the composer on a desktop.
    components["m-chat-networks"] = {
        name: "mChatNetworks",
        props: { person: Object, roomId: String, directEncrypted: { type: Boolean, default: null } },
        emits: ["choose"],
        data() { return { sheet: false, closes: true }; },
        computed: {
            phone() { return !!(this.$q && this.$q.screen && this.$q.screen.lt.sm); },
            ways() { return chat.waysOf(this.person, this.roomId, this.directEncrypted); },
            current() { return this.ways.find((way) => way.current) || null; },
            icon() { return this.current ? this.current.icon : "sym_o_chat"; },
            title() { return this.current ? fillIn(tr("Sending with {network}: choose another"), { network: this.current.name }) : tr("Choose how to send"); }
        },
        methods: {
            choose(way) {
                this.sheet = false;
                if (way && way.usable && !way.current) this.$emit("choose", way);
            }
        },
        template:
            '<q-btn v-if="ways.length > 1" flat round dense size="sm" :icon="icon" class="m-chat-networks" :aria-label="title" @click="phone ? (sheet = true) : null">' +
                '<q-tooltip>{{title}}</q-tooltip>' +
                '<q-menu v-if="!phone" anchor="top left" self="bottom left">' + waysListTemplate + '</q-menu>' +
                '<q-dialog v-if="phone" v-model="sheet" position="bottom">' +
                    '<q-card class="full-width"><q-card-section class="text-subtitle2 q-pb-none">{{t(\'Send with\')}}</q-card-section>' + waysListTemplate + '</q-card>' +
                '</q-dialog>' +
            '</q-btn>'
    };

    // A person's card, at the top of the People tab of their room: their name (Rename), their ways with labels (open or
    // start one), their mail line, their addresses (Not this person), Add (another conversation of theirs, a number or a
    // mail address) and Undo my changes. It emits what the room does: open (a way), merge ({personKeys, addressKeys}),
    // separate (an addressKey), rename (a name), reset. others: the other people of the user's list, to pick one to add.
    components["m-chat-person"] = {
        name: "mChatPerson",
        props: { person: Object, roomId: String, mail: Object, directEncrypted: { type: Boolean, default: null }, others: { type: Array, default: () => [] } },
        emits: ["open", "merge", "separate", "rename", "reset", "mail"],
        data() { return { renaming: false, nameText: "", adding: false, addText: "", closes: true }; },
        computed: {
            phone() { return !!(this.$q && this.$q.screen && this.$q.screen.lt.sm); },
            ways() { return chat.waysOf(this.person, this.roomId, this.directEncrypted); },
            title() {
                const person = this.person || {};
                const first = (person.addresses || []).find((one) => one.label);
                return person.name || (first ? first.label : tr("This person"));
            },
            username() { const user = ((this.person && this.person.addresses) || []).find((one) => one.kind === "user"); return user ? user.label : null; },
            // the addresses that can be taken out: all of them when there are two or more
            addresses() {
                const list = ((this.person && this.person.addresses) || []).filter((one) => one.label);
                return list.length > 1 ? list : [];
            },
            mailLabel() { return chat.e2eeLabel({ mail: true }); },
            addTitle() { return fillIn(tr("Add to {name}"), { name: this.title }); },
            typedLabel() { return fillIn(tr("Add {address}"), { address: String(this.addText || "").trim() }); },
            picks() {
                const text = String(this.addText || "").trim().toLowerCase();
                const mine = this.person ? this.person.personKey : null;
                return (this.others || []).filter((other) => other && other.personKey !== mine && (!text
                    || [other.name].concat((other.addresses || []).map((one) => one.label)).join(" ").toLowerCase().indexOf(text) >= 0)).slice(0, 8);
            },
            typed() {
                const text = String(this.addText || "").trim();
                if (/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(text)) return "email:" + text;
                if (/^\+?[0-9 ()./-]{5,20}$/.test(text) && text.replace(/\D/g, "").length >= 5) return "tel:" + text;
                return null;
            }
        },
        methods: {
            choose(way) { if (way && way.usable && !way.current) this.$emit("open", way); },
            startRename() { this.nameText = (this.person && this.person.nameFrom === "mine" && this.person.name) || ""; this.renaming = true; },
            saveName() { this.renaming = false; this.$emit("rename", String(this.nameText || "").trim()); },
            addPerson(other) { this.adding = false; this.addText = ""; this.$emit("merge", { personKeys: [this.person.personKey, other.personKey], addressKeys: [] }); },
            addTyped() { if (!this.typed) return; const key = this.typed; this.adding = false; this.addText = ""; this.$emit("merge", { personKeys: [this.person.personKey], addressKeys: [key] }); },
            otherLabel(other) {
                const first = (other.addresses || []).find((one) => one.label);
                return [other.name, first && first.label !== other.name ? first.label : null].filter((part) => part).join(" · ");
            },
            notThis(address) { return fillIn(tr("Not {name}"), { name: this.title }) + ": " + address.label; }
        },
        template:
            '<q-card v-if="person" flat bordered class="m-chat-person q-ma-sm">' +
                '<q-card-section class="row items-center no-wrap q-py-sm">' +
                    '<div class="col" style="min-width:0">' +
                        '<div class="text-subtitle2 ellipsis m-chat-person-name">{{title}}</div>' +
                        '<div v-if="username" class="text-caption m-muted ellipsis">{{username}}</div>' +
                    '</div>' +
                    '<q-btn flat round dense icon="sym_o_more_vert" class="m-chat-person-menu" :aria-label="t(\'Person\')">' +
                        '<q-menu><q-list style="min-width:200px">' +
                            '<q-item clickable v-close-popup data-person="rename" @click="startRename"><q-item-section>{{t(\'Rename\')}}</q-item-section></q-item>' +
                            '<q-item clickable v-close-popup data-person="add" @click="adding = true"><q-item-section>{{t(\'Add a conversation, number or address\')}}</q-item-section></q-item>' +
                            '<q-item v-if="person.contactUrl" clickable v-close-popup tag="a" :href="person.contactUrl" data-person="contact"><q-item-section>{{t(\'Open the contact\')}}</q-item-section></q-item>' +
                            '<q-item v-if="person.edited" clickable v-close-popup data-person="reset" @click="$emit(\'reset\')"><q-item-section>{{t(\'Undo my changes\')}}</q-item-section></q-item>' +
                        '</q-list></q-menu></q-btn>' +
                '</q-card-section>' +
                '<q-card-section v-if="renaming" class="row items-center no-wrap q-gutter-x-sm q-pt-none">' +
                    '<q-input v-model="nameText" dense outlined autofocus class="col" maxlength="100" name="personName" :label="t(\'Your name for them\')" @keyup.enter="saveName"></q-input>' +
                    '<q-btn unelevated no-caps dense color="primary" :label="t(\'Save\')" class="m-chat-person-save" @click="saveName"></q-btn>' +
                    '<q-btn flat no-caps dense :label="t(\'Cancel\')" @click="renaming = false"></q-btn>' +
                '</q-card-section>' +
                '<div v-if="person.blocked" class="q-px-md q-pb-sm text-caption">{{t(\'You blocked this person\')}}</div>' +
                waysListTemplate +
                '<q-item v-if="mail" clickable class="m-chat-person-mail" @click="$emit(\'mail\', mail)">' +
                    '<q-item-section avatar><q-icon name="sym_o_mail"></q-icon></q-item-section>' +
                    '<q-item-section><q-item-label class="ellipsis">{{t(\'Email\')}}: {{mail.subject || t(\'(no subject)\')}}</q-item-label>' +
                        '<q-item-label caption>{{mailLabel.text}}</q-item-label></q-item-section>' +
                '</q-item>' +
                '<template v-if="addresses.length"><q-separator></q-separator>' +
                    '<q-item v-for="address in addresses" :key="address.addressKey" dense>' +
                        '<q-item-section><q-item-label caption class="ellipsis">{{address.label}}</q-item-label></q-item-section>' +
                        '<q-item-section side><q-btn flat dense no-caps size="sm" :label="t(\'Not this person\')" :aria-label="notThis(address)" ' +
                            ':data-separate="address.addressKey" @click="$emit(\'separate\', address.addressKey)"></q-btn></q-item-section>' +
                    '</q-item></template>' +
                '<q-dialog v-model="adding" :maximized="phone">' +
                    '<q-card :style="phone ? \'\' : \'width:420px;max-width:95vw\'" class="m-chat-person-add">' +
                        '<q-card-section class="text-subtitle1">{{addTitle}}</q-card-section>' +
                        '<q-card-section class="q-pt-none">' +
                            '<q-input v-model="addText" dense outlined autofocus name="personAdd" :label="t(\'A name, a number or a mail address\')" @keyup.enter="addTyped"></q-input>' +
                            '<q-list dense class="q-mt-sm">' +
                                '<q-item v-if="typed" clickable data-add="typed" @click="addTyped"><q-item-section avatar><q-icon :name="typed.indexOf(\'email:\') === 0 ? \'sym_o_mail\' : \'sym_o_call\'"></q-icon></q-item-section>' +
                                    '<q-item-section>{{typedLabel}}</q-item-section></q-item>' +
                                '<q-item v-for="other in picks" :key="other.personKey" clickable :data-add="other.personKey" @click="addPerson(other)">' +
                                    '<q-item-section>{{otherLabel(other)}}</q-item-section></q-item>' +
                            '</q-list>' +
                        '</q-card-section>' +
                        '<q-card-actions align="right"><q-btn flat no-caps :label="t(\'Cancel\')" v-close-popup></q-btn></q-card-actions>' +
                    '</q-card>' +
                '</q-dialog>' +
            '</q-card>'
    };

    if (typeof module !== "undefined" && module.exports) module.exports = chat;
})(globalThis);
