/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * Disappearing messages and view-once media in the page (plans/Comms-Messaging.md §18.3 phase 16): moqui.chat.expiry, beside
 * the chat store of WebrootChat.js (loaded after it, in qapps.xml and the desktop shell's index.html).
 *
 *   - The room's timer in words: options (the dialog's choices), label, short (the header's chip) and left (the time a
 *     message has left).
 *   - The server's clock: learn(serverNow) from get#Timeline's answer (seen) and each pushed event's sentDate, so a device
 *     whose clock is wrong still deletes at the server's time (now).
 *   - One page timer to the next due time of the events the page shows (track, seen): at that time it emits the 'expired'
 *     signal on moqui.chat, the shape the server pushes ({kind: 'expired', chatRoomId, eventSeqList, before}), so every list
 *     of a room drops them the same way, and their decrypted copies go (moqui.chat.forgetEvents).
 *   - secureScreen(reason, on): the app host's screen security (the Android app's FLAG_SECURE, held per reason), nothing in a
 *     browser; canSecure says whether it is there.
 *
 * Runs under Node for its tests: nothing here touches the page until track starts its timer.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui;
    const chat = moqui && moqui.chat;
    if (!chat) return;

    // the dialog's choices in seconds: off, 30 seconds to 4 weeks
    const CHOICES = [0, 30, 300, 3600, 28800, 86400, 604800, 2419200];
    const LABELS = { 0: "Off", 30: "30 seconds", 300: "5 minutes", 3600: "1 hour", 28800: "8 hours", 86400: "1 day", 604800: "1 week", 2419200: "4 weeks" };
    // a setTimeout's longest wait
    const MAX_WAIT = 2147483647;
    // a pushed event's sentDate counts as the server's clock only when it is this fresh (a push is not a page of old events)
    const FRESH = 10000;

    const expiry = {
        offset: 0,
        held: {},
        // each room's timer in seconds, as its answers gave it (WebrootCrypto.js rotates an encrypted room's key by it)
        timers: {},
        timer: null,

        // ---- the pure parts ----

        // the dialog's choices: {value (seconds, 0 off), label (to localize)}
        options() { return CHOICES.map((value) => ({ value, label: LABELS[value] })); },
        // a timer in words, to localize: a choice's label, else the largest whole unit
        label(seconds) {
            const value = Number(seconds) || 0;
            if (LABELS[value]) return LABELS[value];
            if (value % 604800 === 0) return value / 604800 + " weeks";
            if (value % 86400 === 0) return value / 86400 + " days";
            if (value % 3600 === 0) return value / 3600 + " hours";
            if (value % 60 === 0) return value / 60 + " minutes";
            return value + " seconds";
        },
        // a timer as the header's chip shows it: 30s, 5m, 1h, 8h, 1d, 1w, 4w
        short(seconds) {
            const value = Number(seconds) || 0;
            if (!value) return "";
            if (value % 604800 === 0) return value / 604800 + "w";
            if (value % 86400 === 0) return value / 86400 + "d";
            if (value % 3600 === 0) return value / 3600 + "h";
            if (value % 60 === 0) return value / 60 + "m";
            return value + "s";
        },
        // the time a message has left at now: "45s", "4m", "2h", "3d" (rounded up; "0s" once due)
        left(expireDate, now) {
            const ms = Math.max(0, Number(expireDate) - Number(now));
            const seconds = Math.ceil(ms / 1000);
            if (seconds < 60) return seconds + "s";
            if (seconds < 3600) return Math.ceil(seconds / 60) + "m";
            if (seconds < 86400) return Math.ceil(seconds / 3600) + "h";
            return Math.ceil(seconds / 86400) + "d";
        },
        // the server's clock from a time it gave at local time at (now by default)
        learn(serverNow, at) {
            const value = Number(serverNow);
            if (!Number.isFinite(value) || value <= 0) return;
            expiry.offset = value - (at === undefined ? Date.now() : Number(at));
        },
        // now by the server's clock
        now(at) { return (at === undefined ? Date.now() : Number(at)) + expiry.offset; },
        // the events of a list due at now: their eventSeqs
        due(events, now) {
            return (events || []).filter((event) => event && event.expireDate != null && Number(event.expireDate) <= now).map((event) => Number(event.eventSeq));
        },
        // the next due time after now of a list's events, null for none
        nextDue(events, now) {
            let next = null;
            for (const event of events || []) {
                if (!event || event.expireDate == null) continue;
                const at = Number(event.expireDate);
                if (at > now && (next === null || at < next)) next = at;
            }
            return next;
        },

        // ---- the page's timer ----

        // an answer of the room's (a timeline page, a room view): the server's clock, and its expiring events held
        seen(json, roomId) {
            if (!json) return;
            const timeline = json.timeline || json;
            if (timeline.serverNow != null) expiry.learn(timeline.serverNow);
            const room = json.room || (json.timeline && json.timeline.room);
            if (room && roomId) expiry.timers[roomId] = Number(room.messageTimerSeconds) || 0;
            const lists = [json.eventList, json.timeline && json.timeline.eventList].filter((list) => Array.isArray(list));
            for (const list of lists) expiry.track(roomId, list);
        },
        // a room's timer in seconds, 0 for none (or not known here)
        timerOf(roomId) { return Number(expiry.timers[roomId]) || 0; },
        // a pushed event: a fresh one sets the clock; an expiring one is held
        pushed(roomId, event) {
            if (!event) return;
            const sent = Number(event.sentDate);
            if (Number.isFinite(sent) && Math.abs(expiry.now() - sent) > FRESH && Math.abs(Date.now() - sent) < 24 * 3600000) expiry.learn(sent);
            expiry.track(roomId, [event]);
        },
        // the expiring events of a room the page shows, held for the timer
        track(roomId, events) {
            if (!roomId) return;
            let added = false;
            for (const event of events || []) {
                if (!event || event.expireDate == null) continue;
                const room = expiry.held[roomId] = expiry.held[roomId] || {};
                room[Number(event.eventSeq)] = Number(event.expireDate);
                added = true;
            }
            if (added) expiry.schedule();
        },
        // gone from the server or this page: no longer held
        untrack(roomId, seqs, before) {
            const room = expiry.held[roomId];
            if (!room) return;
            for (const seq of seqs || []) delete room[Number(seq)];
            if (before != null) for (const seq of Object.keys(room)) if (room[seq] <= Number(before)) delete room[seq];
            if (!Object.keys(room).length) delete expiry.held[roomId];
        },
        schedule() {
            if (expiry.timer) root.clearTimeout(expiry.timer);
            expiry.timer = null;
            let next = null;
            for (const roomId of Object.keys(expiry.held)) for (const at of Object.values(expiry.held[roomId])) if (next === null || at < next) next = at;
            if (next === null || typeof root.setTimeout !== "function") return;
            expiry.timer = root.setTimeout(expiry.fire, Math.min(MAX_WAIT, Math.max(0, next - expiry.now())));
        },
        // what is due now: out of the page, as the server's push takes it out
        fire() {
            expiry.timer = null;
            const now = expiry.now();
            for (const roomId of Object.keys(expiry.held)) {
                const room = expiry.held[roomId];
                const seqs = Object.keys(room).filter((seq) => room[seq] <= now).map(Number);
                if (!seqs.length) continue;
                chat.emit("expired", { kind: "expired", chatRoomId: roomId, eventSeqList: seqs, before: null });
            }
            expiry.schedule();
        },

        // ---- screen security (the app host's) ----

        canSecure() {
            const host = root.moquiDesktop;
            return !!host && typeof host.secureScreen === "function";
        },
        // screenshots off (on true) for a reason (viewOnce here; the app or a room later), back once no reason holds them
        secureScreen(reason, on) {
            if (!expiry.canSecure()) return Promise.resolve(false);
            return Promise.resolve(root.moquiDesktop.secureScreen(String(reason || ""), !!on)).then((done) => !!done, () => false);
        }
    };

    chat.expiry = expiry;
    // an expired signal, the page's or the server's: its events are no longer held, and their decrypted copies go
    chat.on("expired", (done) => {
        if (!done || !done.chatRoomId) return;
        expiry.untrack(done.chatRoomId, done.eventSeqList, done.before);
        if (chat.forgetEvents) chat.forgetEvents(done.chatRoomId, done.eventSeqList || []);
        expiry.schedule();
    });
})(globalThis);
