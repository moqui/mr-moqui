/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The E2EE labels in the bundled shell (ChatLabels.spec.js; plans/Comms-Messaging.md §6.2): Shell/Root on a stand-in host and
// a stand-in server (as PhoneChat.page.js) with a room of each state: encrypted with every device verified, encrypted with a
// device not verified, a plain private room, a plain direct chat, a public channel, a phone number's texts and a room
// bridged to WhatsApp. The crypto device stands in too: open, with each encrypted room's devices (moqui.crypto.roomTrust).
// window.labels.walk() opens each room, on the phone from the home and on a desktop in Chat, and says what it shows.
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
const clone = (value) => JSON.parse(JSON.stringify(value));
const textOf = (el) => (el ? el.textContent.replace(/\s+/g, " ").trim() : "");

// ---- the server ----
const NOW = Date.now();
const message = (seq, sender, body) => ({ eventSeq: seq, type: "message", senderUserId: sender, senderName: sender === "ann" ? "Ann" : "Bob",
    sentDate: NOW - 600000 + seq * 1000, body: "<p>" + body + "</p>", bodyText: body, reactions: [], attachments: [] });
const rooms = {};
const events = {};
const addRoom = (room, list) => {
    rooms[room.chatRoomId] = Object.assign({ isMember: true, unreadCount: 0, visibilityEnumId: "CrvMembers", lastEventSeq: list.length, lastEventDate: NOW, bridges: [] }, room);
    events[room.chatRoomId] = list;
    const last = list[list.length - 1];
    if (last) rooms[room.chatRoomId].lastMessage = { eventSeq: last.eventSeq, senderUserId: last.senderUserId, senderName: last.senderName, sentDate: last.sentDate, snippet: last.bodyText };
};
// the encrypted rooms have no messages: nothing to decrypt here
addRoom({ chatRoomId: "V1", roomTypeEnumId: "CrtPrivate", roomName: "Verified", displayName: "Verified", encrypted: true }, []);
addRoom({ chatRoomId: "E1", roomTypeEnumId: "CrtDirect", displayName: "Sealed Bob", otherUserId: "bob", encrypted: true }, []);
addRoom({ chatRoomId: "P1", roomTypeEnumId: "CrtPrivate", roomName: "Ops", displayName: "Ops" }, [message(1, "bob", "Deploy at five")]);
addRoom({ chatRoomId: "D1", roomTypeEnumId: "CrtDirect", displayName: "Bob Baker", otherUserId: "bob" }, [message(1, "bob", "Lunch?")]);
addRoom({ chatRoomId: "C1", roomTypeEnumId: "CrtPublic", roomName: "Lobby", displayName: "Lobby", visibilityEnumId: "CrvUsers" }, [message(1, "bob", "Hello all")]);
const sms = { chatRoomBridgeId: "RB1", chatBridgeId: "B-SMS", name: "Telnyx", network: "sms", networkName: "SMS", connected: true, directionEnumId: "CrbBoth", outbound: true, encryption: "None" };
addRoom({ chatRoomId: "S1", roomTypeEnumId: "CrtExternal", roomName: "+15552224000", displayName: "+15552224000", bridged: true, bridgeNetworkNames: ["SMS"],
    bridgeNetworks: [{ network: "sms", networkName: "SMS", encryption: "None", outbound: true }], bridges: [sms] }, [message(1, "bob", "On my way")]);
const whatsapp = Object.assign({}, sms, { chatRoomBridgeId: "RB2", chatBridgeId: "B-WA", name: "WhatsApp bridge", network: "whatsapp", networkName: "WhatsApp", encryption: "BridgeDecrypted" });
addRoom({ chatRoomId: "W1", roomTypeEnumId: "CrtPrivate", roomName: "Partners", displayName: "Partners", bridged: true, bridgeNetworkNames: ["WhatsApp"],
    bridgeNetworks: [{ network: "whatsapp", networkName: "WhatsApp", encryption: "BridgeDecrypted", outbound: true }], bridges: [whatsapp] }, [message(1, "bob", "Hi from WhatsApp")]);
const answers = {
    "/qapps/clientConfig": () => ({ userId: "ann", username: "ann", userFullName: "Ann", locale: "en", server: { name: "Test Server", clientApi: 4 },
        calls: { appRoot: "/qapps/BasicComms" }, notificationEndpoint: "/notws" }),
    "/shellData": () => ({ apps: [], counts: {} }),
    "/apps/l10nCatalog": () => ({}),
    "/apps/BasicComms/Chat/chatData": () => ({ me: { userId: "ann" }, prefs: {}, roomList: Object.values(rooms).map(clone) }),
    "/apps/BasicComms/Chat/roomView": (params) => {
        const room = rooms[params.chatRoomId];
        return { room: clone(room), member: { lastReadEventSeq: room.lastEventSeq }, access: { isMember: true, canPost: true },
            memberList: [{ userId: "ann", name: "Ann" }, { userId: "bob", name: "Bob Baker" }],
            timeline: { eventList: clone(events[params.chatRoomId] || []), hasMoreBefore: false, hasMoreAfter: false } };
    },
    "/apps/BasicComms/Chat/timeline": (params) => ({ eventList: params.pinnedOnly ? [] : clone(events[params.chatRoomId] || []), hasMoreBefore: false, hasMoreAfter: false })
};
moqui.request = (url, options) => {
    const path = String(url).split("?")[0];
    const params = options && moqui.isPlainObject(options.params) ? options.params : {};
    moqui.noteAnswer(200, { get: () => null });
    const answer = answers[path];
    return Promise.resolve({ ok: true, status: 200, headers: { get: () => null }, json: answer ? answer(params) : {}, text: "" });
};

// ---- the crypto device: open, with the devices of each encrypted room (this device left out) ----
const trusts = {
    V1: { verifiedOnly: false, users: [{ userId: "ann", devices: [] }, { userId: "bob", devices: [{ deviceId: "BOB1", displayName: "Bob's phone", verified: true }] }] },
    E1: { verifiedOnly: false, users: [{ userId: "ann", devices: [] }, { userId: "bob", devices: [{ deviceId: "BOB2", displayName: "Bob's laptop", verified: false }] }] }
};
moqui.crypto.roomTrust = (roomId) => Promise.resolve(clone(trusts[roomId]));
moqui.crypto.state.status = "ready";

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
const phone = () => window.innerWidth < 640;
const rowOf = (id) => document.querySelector((phone() ? ".m-chat-home " : "") + "[data-room='" + id + "']");
// what the open room shows: its header's lock (state, check), its banner (state, text, Verify), its start's line and the
// composer's hint (the editor's placeholder)
const seen = async (id) => {
    const header = await until(() => document.querySelector(".m-chat-encrypted, .m-chat-not-e2ee"), "the label of " + id);
    // an encrypted room's trust comes after its view
    if (id === "V1") await until(() => document.querySelector(".m-chat-verified"), "the verified check");
    const banner = document.querySelector(".m-chat-e2ee-banner");
    const placeholder = await until(() => { const el = document.querySelector(".m-chat-composer [data-placeholder]"); return el && el.getAttribute("data-placeholder"); },
        "the composer's placeholder in " + id);
    return { header: header.getAttribute("data-e2ee"), lock: header.classList.contains("m-chat-encrypted"), check: !!document.querySelector(".m-chat-verified"),
        banner: banner ? banner.getAttribute("data-e2ee") : null, bannerText: banner ? textOf(banner.querySelector(".m-chat-e2ee-banner-text")) : "",
        verify: !!document.querySelector(".m-chat-e2ee-banner .m-chat-verify"), intro: textOf(document.querySelector(".m-chat-e2ee-intro-text")), placeholder,
        listLock: !!document.querySelector(".m-chat-e2ee-lock[data-e2ee='" + id + "']") };
};
const ids = () => Object.keys(rooms);

window.labels = {
    async start() {
        const shell = document.createElement("div");
        shell.id = "shell";
        document.body.appendChild(shell);
        await moqui.loadScript("/files/crates/moqui-desktop/shell/js/shell.js");
        root = await until(() => moqui.webrootVue && moqui.webrootVue.commsItems && moqui.webrootVue, "the shell's root");
        if (!phone()) root.setUrl("/qapps/BasicComms/Chat");
        await until(() => rowOf("V1"), "the rooms");
        return true;
    },
    // each room in turn: on the phone from the home (then Back), on a desktop from Chat's rail
    async walk() {
        const out = { width: window.innerWidth, rooms: {} };
        for (const id of ids()) {
            const list = !!document.querySelector(".m-chat-e2ee-lock[data-e2ee='" + id + "']");
            rowOf(id).click();
            await until(() => document.querySelector(".m-chat-composer")
                && document.querySelector((phone() ? ".m-chat-phone " : "") + ".text-subtitle1[title='" + rooms[id].displayName + "']"), "room " + id);
            out.rooms[id] = Object.assign(await seen(id), { listLock: list });
            if (phone()) {
                while (moqui.back.pop()) await new Promise((resolve) => setTimeout(resolve, 150));
                await until(() => rowOf("V1"), "the home again");
            }
        }
        return out;
    }
};
})();
