/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The room's details tabs in the bundled shell (ChatDetails.spec.js; plans/Comms-Messaging.md §5): Shell/Root on a stand-in
// host and a stand-in server (as PhoneReactions.page.js) with a room of photos, a video, a PDF, a voice note, two links and
// three reactions. The server's lists answer by the parameters the tabs send (the types to keep and to leave out, the
// reactions to me), as find#RoomFiles, find#RoomLinks and find#RoomReactions do, so a tab that asks wrongly shows the wrong
// items. The microphone is a made sound (an oscillator), so a voice note is recorded with no device. window.details has a
// step for each feature of the spec, each a promise of what it saw.
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

// ---- made touches: a tap is touchstart, touchend and the click a phone sends after it ----
let touchId = 0;
const touch = (el, type, dx) => {
    const box = el.getBoundingClientRect();
    const point = new Touch({ identifier: ++touchId, target: el, clientX: box.left + box.width / 2 + (dx || 0), clientY: box.top + box.height / 2 });
    const list = type === "touchend" ? [] : [point];
    return el.dispatchEvent(new TouchEvent(type, { touches: list, targetTouches: list, changedTouches: [point], bubbles: true, cancelable: true }));
};
const tap = async (el) => {
    if (window.innerWidth >= 640) { el.click(); await wait(40); return; }
    touch(el, "touchstart");
    await wait(40);
    if (touch(el, "touchend")) el.click();
    await wait(40);
};
// a swipe to the left across el: the next item
const swipeLeft = async (el) => {
    touch(el, "touchstart", 120);
    await wait(60);
    touch(el, "touchend", -120);
    await wait(60);
};

// ---- the server ----
const NOW = Date.now();
const sent = (seq) => NOW - 600000 + seq * 1000;
const file = (id, seq, sender, fileName, contentType) => ({ chatAttachmentId: id, eventSeq: seq, attachmentSeq: 1, fileName, contentType, fileSize: 2048,
    senderUserId: sender, senderName: sender === "ann" ? "Ann" : "Bob Baker", sentDate: sent(seq), threadRootEventSeq: null, encrypted: false });
const files = [file("F1", 1, "bob", "beach.png", "image/png"), Object.assign(file("F2", 1, "bob", "dunes.jpg", "image/jpeg"), { attachmentSeq: 2 }),
    file("F3", 2, "ann", "waves.mp4", "video/mp4"), file("F4", 3, "bob", "plan.pdf", "application/pdf"), file("F5", 4, "ann", "Voice note.webm", "audio/webm")];
const message = (seq, sender, body, extra) => Object.assign({ eventSeq: seq, type: "message", senderUserId: sender, senderName: sender === "ann" ? "Ann" : "Bob Baker",
    sentDate: sent(seq), body: body ? "<p>" + body + "</p>" : null, bodyText: body || "", reactions: [], attachments: files.filter((one) => one.eventSeq === seq) }, extra || {});
const events = [
    message(1, "bob", "Photos from the trip", { reactions: [{ key: "👍", count: 1, userIds: ["ann"], mine: true }, { key: "🎉", count: 1, userIds: ["cy"], mine: false }] }),
    message(2, "ann", ""), message(3, "bob", "The plan", { reactions: [] }),
    message(4, "ann", "", { reactions: [{ key: "❤️", count: 1, userIds: ["bob"], mine: false }] }),
    message(5, "bob", "Read https://example.com/guide and https://example.org/page")
];
const links = [{ eventSeq: 5, linkSeq: 1, url: "https://example.com/guide", title: "The guide", siteName: "Example", senderUserId: "bob", senderName: "Bob Baker", sentDate: sent(5) },
    { eventSeq: 5, linkSeq: 2, url: "https://example.org/page", title: null, siteName: null, senderUserId: "bob", senderName: "Bob Baker", sentDate: sent(5) }];
// the messages with reactions, the newest reaction first (on the voice note, then the photos)
const reacted = [{ eventSeq: 4, senderUserId: "ann", senderName: "Ann", sentDate: sent(4), snippet: "📎 Voice note.webm", reactions: clone(events[3].reactions), latestReactionSeq: 9 },
    { eventSeq: 1, senderUserId: "bob", senderName: "Bob Baker", sentDate: sent(1), snippet: "Photos from the trip", reactions: clone(events[0].reactions), latestReactionSeq: 8 }];
const room = { chatRoomId: "R1", roomTypeEnumId: "CrtPrivate", roomName: "Trip", displayName: "Trip", isMember: true, unreadCount: 0, visibilityEnumId: "CrvMembers",
    lastEventSeq: 5, lastEventDate: NOW, bridges: [] };
const members = [{ userId: "ann", name: "Ann", lastReadEventSeq: 5, roleEnumId: "CmrOwner" }, { userId: "bob", name: "Bob Baker", lastReadEventSeq: 5 },
    { userId: "cy", name: "Cy Cole", lastReadEventSeq: 5 }];
const prefixes = (text) => String(text || "").split(",").map((part) => part.trim()).filter((part) => part);
const posted = [];
const requested = [];
const answers = {
    "/qapps/clientConfig": () => ({ userId: "ann", username: "ann", userFullName: "Ann", locale: "en", server: { name: "Test Server", clientApi: 4 },
        calls: { appRoot: "/qapps/BasicComms" }, notificationEndpoint: "/notws" }),
    "/shellData": () => ({ apps: [], counts: {} }),
    "/apps/l10nCatalog": () => ({}),
    "/apps/BasicComms/Chat/chatData": () => ({ me: { userId: "ann" }, prefs: {}, roomList: [clone(room)] }),
    "/apps/BasicComms/Chat/roomView": () => ({ room: clone(room), member: { lastReadEventSeq: 5, roleEnumId: "CmrOwner" },
        access: { isMember: true, canPost: true, canModerate: true, isOwner: true }, memberList: clone(members),
        timeline: { eventList: clone(events), hasMoreBefore: false, hasMoreAfter: false } }),
    "/apps/BasicComms/Chat/timeline": (params) => ({ eventList: params.pinnedOnly ? [] : clone(events), hasMoreBefore: false, hasMoreAfter: false, memberList: clone(members) }),
    // as find#RoomFiles: the types to keep, and to leave out (a file with no type stays), newest first
    "/apps/BasicComms/Chat/files": (params) => {
        const keep = prefixes(params.contentTypePrefixes);
        const leave = prefixes(params.excludeContentTypePrefixes);
        const list = files.filter((one) => (!keep.length || keep.some((prefix) => one.contentType.startsWith(prefix)))
            && !leave.some((prefix) => one.contentType.startsWith(prefix)));
        return { fileList: clone(list.sort((a, b) => b.eventSeq - a.eventSeq || a.attachmentSeq - b.attachmentSeq)), nextBeforeEventSeq: null };
    },
    "/apps/BasicComms/Chat/links": () => ({ links: clone(links), nextBeforeEventSeq: null }),
    "/apps/BasicComms/Chat/reactions": (params) => ({ messages: clone(String(params.onlyToMe) === "true" ? reacted.filter((one) => one.senderUserId === "ann") : reacted),
        nextBeforeReactionSeq: null }),
    "/apps/BasicComms/Chat/upload": () => ({ uploadList: [{ chatAttachmentId: "UP1", fileName: "Voice note.webm", contentType: "audio/webm", fileSize: 900 }] }),
    "/apps/BasicComms/Chat/send": () => ({ eventSeq: 6, event: Object.assign(message(6, "ann", ""), { attachments: [file("UP1", 6, "ann", "Voice note.webm", "audio/webm")] }) })
};
moqui.request = (url, options) => {
    const path = String(url).split("?")[0];
    const method = (options && options.method) || "GET";
    const params = options && moqui.isPlainObject(options.params) ? options.params : {};
    if (method === "POST") posted.push({ path, params: clone(params), form: !!(options && options.params instanceof FormData) });
    requested.push({ path, params: clone(params) });
    moqui.noteAnswer(200, { get: () => null });
    const answer = answers[path];
    return Promise.resolve({ ok: true, status: 200, headers: { get: () => null }, json: answer ? answer(params) : {}, text: "" });
};

// ---- the microphone: a made tone, so MediaRecorder records with no device ----
const tone = () => {
    const context = new AudioContext();
    const oscillator = context.createOscillator();
    const out = context.createMediaStreamDestination();
    oscillator.connect(out);
    oscillator.start();
    return out.stream;
};
if (navigator.mediaDevices) navigator.mediaDevices.getUserMedia = () => Promise.resolve(tone());

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
const phone = () => window.innerWidth < 640;
const scope = () => (phone() ? ".m-chat-phone " : "");
const msg = (seq) => document.querySelector(scope() + ".m-chat-msg[data-seq='" + seq + "']");
const pane = (name) => document.querySelector("[data-pane='" + name + "']");
// a tab, and what it lists once its answer is shown
const openTab = async (name) => {
    await tap(await until(() => document.querySelector(".m-chat-details-tabs [data-tab='" + name + "']"), "the tab " + name));
    return until(() => pane(name), "the pane " + name);
};
const ids = (el, selector, attr) => Array.from(el.querySelectorAll(selector)).map((one) => one.getAttribute(attr));

window.details = {
    async start() {
        Quasar.Platform.is.mobile = phone();
        const shell = document.createElement("div");
        shell.id = "shell";
        document.body.appendChild(shell);
        try { localStorage.clear(); } catch (err) { /* no storage */ }
        await moqui.loadScript("/files/crates/moqui-desktop/shell/js/shell.js");
        const root = await until(() => moqui.webrootVue && moqui.webrootVue.commsItems && moqui.webrootVue, "the shell's root");
        if (!phone()) root.setUrl("/qapps/BasicComms/Chat");
        const row = () => document.querySelector((phone() ? ".m-chat-home " : ".m-chat-root ") + "[data-room='R1']");
        await until(row, "the rooms");
        // the rail's rows are drawn again once Chat has read its data: tapped until the room opens
        for (let tries = 0; tries < 10 && !msg(5); tries++) {
            if (row()) await tap(row());
            await until(() => msg(5), "the room", 1000).catch(() => null);
        }
        await until(() => msg(5), "the room");
        return true;
    },
    // the details from the room's header (a desktop) or its menu (a phone), then each tab in turn
    async tabs() {
        if (phone()) {
            await tap(Array.from(document.querySelectorAll(".m-chat-phone button")).find((el) => el.getAttribute("aria-label") === "Room"));
            await tap(await until(() => document.querySelector(".q-menu .m-chat-details-item"), "Media, files and links in the menu"));
        } else {
            await tap(await until(() => document.querySelector(".m-chat-details-btn"), "the details button"));
        }
        const out = { width: window.innerWidth };
        const media = await openTab("media");
        await until(() => media.querySelectorAll(".m-chat-media").length === 3, "the media");
        out.media = ids(media, ".m-chat-media", "data-file");
        out.video = !!media.querySelector(".m-chat-media[data-file='F3'] .m-chat-media-video");
        // the viewer: a tap on the first picture, a swipe to the next, and Close
        await tap(media.querySelector(".m-chat-media[data-file='F3'] .m-chat-media-video"));
        const viewer = await until(() => document.querySelector(".m-chat-viewer"), "the viewer");
        out.viewerFirst = viewer.getAttribute("data-file");
        out.viewerCount = textOf(viewer.querySelector(".m-chat-viewer-where"));
        await swipeLeft(viewer);
        out.viewerNext = await until(() => { const el = document.querySelector(".m-chat-viewer"); return el && el.getAttribute("data-file") !== out.viewerFirst && el.getAttribute("data-file"); }, "the next one");
        out.viewerImage = !!document.querySelector(".m-chat-viewer img");
        moqui.back.pop();
        await until(() => !document.querySelector(".m-chat-viewer"), "the viewer to close");
        out.paneAfterViewer = !!pane("media");
        const fileTab = await openTab("files");
        await until(() => fileTab.querySelectorAll(".q-item[data-file]").length, "the files");
        out.files = ids(fileTab, ".q-item[data-file]", "data-file");
        const voice = await openTab("voice");
        await until(() => voice.querySelectorAll(".q-item[data-file]").length, "the voice notes");
        out.voice = ids(voice, ".q-item[data-file]", "data-file");
        out.voicePlayer = !!voice.querySelector(".m-chat-audio audio");
        const linkTab = await openTab("links");
        await until(() => linkTab.querySelectorAll(".m-chat-link").length === 2, "the links");
        out.links = Array.from(linkTab.querySelectorAll(".m-chat-link a")).map((a) => a.getAttribute("href") + " " + textOf(a));
        const reactionTab = await openTab("reactions");
        await until(() => reactionTab.querySelectorAll(".m-chat-reacted").length === 2, "the reacted messages");
        out.reacted = ids(reactionTab, ".m-chat-reacted", "data-seq");
        out.keys = ids(reactionTab, ".m-chat-reacted [data-key]", "data-key");
        await tap(reactionTab.querySelector("[data-reactions='mine']"));
        await until(() => reactionTab.querySelectorAll(".m-chat-reacted").length === 1, "the reactions to me");
        out.toMe = ids(reactionTab, ".m-chat-reacted", "data-seq");
        out.askedToMe = requested.filter((one) => one.path === "/apps/BasicComms/Chat/reactions").slice(-1)[0].params.onlyToMe;
        const people = await openTab("members");
        out.people = Array.from(people.querySelectorAll(".q-list .q-item .q-item__label:first-child")).map(textOf).filter((name) => name);
        const pinned = await openTab("pinned");
        out.pinned = textOf(await until(() => pinned.querySelector(".q-item"), "the pinned list"));
        // the tabs asked the server for their types (as the server filters by them)
        const asked = requested.filter((one) => one.path === "/apps/BasicComms/Chat/files").map((one) => one.params.contentTypePrefixes || "-" + (one.params.excludeContentTypePrefixes || ""));
        out.fileAsks = Array.from(new Set(asked));
        moqui.back.pop();
        await until(() => !document.querySelector(".m-chat-details-tabs"), "the details to close");
        out.closed = true;
        return out;
    },
    // a voice note: the microphone in place of Send, its time while it records, then sent as an audio file of the message
    async voiceNote() {
        const composer = await until(() => document.querySelector(scope() + ".m-chat-composer"), "the composer");
        const mic = await until(() => composer.querySelector(".m-chat-voice"), "the microphone");
        const out = { mic: mic.getAttribute("aria-label"), sendShown: !!composer.querySelector(".m-chat-send") };
        await tap(mic);
        await until(() => composer.querySelector(".m-chat-recording"), "the recording");
        await wait(1300);
        out.time = textOf(composer.querySelector(".m-chat-recording-time"));
        const before = posted.length;
        await tap(composer.querySelector(".m-chat-voice-send"));
        const send = await until(() => posted.slice(before).find((one) => one.path === "/apps/BasicComms/Chat/send"), "the message sent");
        out.uploaded = posted.slice(before).filter((one) => one.path === "/apps/BasicComms/Chat/upload").length;
        out.attachments = send.params.chatAttachmentIds;
        // the sent message plays in the room
        out.inline = !!(await until(() => { const row = msg(6); return row && row.querySelector(".m-chat-audio audio"); }, "the voice note in the room"));
        out.micAgain = !!(await until(() => composer.querySelector(".m-chat-voice"), "the microphone again"));
        return out;
    }
};
})();
