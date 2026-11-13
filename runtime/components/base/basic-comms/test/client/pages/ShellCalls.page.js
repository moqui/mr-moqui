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
const text = () => document.body.textContent;
const plainHeaders = { get: () => null };

// the calls as the server shows them (CallServices get#Call)
const person = (userId, displayName, role, inCall) => ({ userId, displayName, role, inCall, legs: [] });
const callView = (callSessionId, extra) => Object.assign({ callSessionId, status: "ringing", video: true, kind: "chat", myLegStatus: "offered",
    encryptionMode: "srtp", e2ee: false, participants: [person("bob", "Bob", "host", true), person("ann", "Ann", "participant", false)] }, extra || {});
const calls = {
    C1: callView("C1"),
    C2: callView("C2", { status: "ringing", myLegStatus: "answered", myLegId: "L2" }),
    E1: callView("E1", { e2ee: true, encryptionMode: "e2ee-sframe" }),
    P1: callView("P1")
};
const room = { chatRoomId: "R1", roomTypeEnumId: "CrtPrivate", roomName: "Ops", displayName: "Ops", lastEventSeq: 5, isMember: true, unreadCount: 0 };
const answers = {
    "/qapps/clientConfig": { userId: "ann", username: "ann", userFullName: "Ann", locale: "en", server: { name: "Test Server", clientApi: 4 },
        calls: { appRoot: "/qapps/BasicComms" }, notificationEndpoint: "/notws" },
    "/shellData": { apps: [], counts: {} },
    "/apps/l10nCatalog": {},
    "/apps/BasicComms/Chat/chatData": { me: { userId: "ann" }, prefs: {}, roomList: [room] },
    "/apps/BasicComms/Chat/roomView": { room, member: { lastReadEventSeq: 5 }, access: { isMember: true, canPost: true },
        memberList: [{ userId: "ann", name: "Ann" }, { userId: "bob", name: "Bob" }], timeline: { eventList: [], hasMoreBefore: false, hasMoreAfter: false } },
    "/apps/BasicComms/Calls/activeCalls": { callList: [calls.C1] },
    "/apps/BasicComms/Calls/startCall": { callSessionId: "C2", callLegId: "L2", call: calls.C2 },
    "/apps/BasicComms/Calls/Dialer/dialerData": { phoneActive: true, callerIdList: [{ phoneNumber: "+15550100", description: "Desk" }], recentList: [] }
};
const requests = [];
let offline = false;
const serve = (url, options) => {
    const path = String(url).split("?")[0];
    const params = (options && options.params) || {};
    requests.push(path + " " + ((options && options.method) || "GET"));
    // the host's own 502 while the server is out of reach
    if (offline) {
        const headers = { get: (name) => (name === "X-Moqui-Offline" ? "unreachable" : null) };
        moqui.noteAnswer(502, headers);
        return Promise.reject({ ok: false, status: 502, headers, text: "The server could not be reached" });
    }
    let json = answers[path];
    if (path === "/apps/BasicComms/Calls/callState") json = { call: calls[params.callSessionId] || null };
    if (json === undefined) {
        moqui.noteAnswer(404, plainHeaders);
        return Promise.reject({ ok: false, status: 404, headers: plainHeaders, text: "" });
    }
    moqui.noteAnswer(200, plainHeaders);
    return Promise.resolve({ ok: true, status: 200, headers: plainHeaders, json: JSON.parse(JSON.stringify(json)), text: "" });
};

// no media here: the call's join and the lobby's preview are stood in for, and no ringtone
const joined = [];
moqui.call.join = (callSessionId, legId, options) => { joined.push([callSessionId, legId]); return Promise.resolve(); };
moqui.call.getMedia = () => Promise.resolve(null);
moqui.call.state.settings.ringtone = false;
// leaving the call this page is in (an account switch asks first)
const left = [];
moqui.call.leave = () => { left.push(moqui.call.state.current); moqui.call.state.current = null; return Promise.resolve({}); };

const host = { opened: [], notified: [], pendingOpen: null, seen: true, switched: [] };
const desktopHost = {
    state: () => Promise.resolve({ accounts: [{ id: "a1", username: "ann", server: "https://s.example.com", deviceMode: "personal" },
        { id: "a2", username: "ann", server: "https://t.example.com", deviceMode: "personal" }], current: "a1",
        signedIn: true, keyStore: "os", clientApi: 4, deviceMode: "personal", loadToken: null }),
    takeOpen: () => { const path = host.pendingOpen; host.pendingOpen = null; return Promise.resolve(path); },
    clearScopes: () => Promise.resolve(),
    socket: () => ({ readyState: 0, send() {}, close() {} }),
    outbox: { add: () => Promise.resolve({}), list: () => Promise.resolve([]), retry: () => Promise.resolve(true), discard: () => Promise.resolve(true),
        on: () => () => {}, unsent: () => Promise.resolve({ operations: [], drafts: [] }) },
    drafts: { get: () => Promise.resolve(null), list: () => Promise.resolve([]), put: () => Promise.resolve(), remove: () => Promise.resolve(true), saveFile: () => Promise.resolve(null) },
    notify: (note) => { host.notified.push(note); return Promise.resolve(); },
    // whether the user sees the main window, as the host tells it (src/bridge.rs tell_focus)
    windowSeen: () => host.seen,
    switchAccount: (id) => { host.switched.push(id); return Promise.resolve(); },
    openRemote: (path) => { host.opened.push(path); return Promise.resolve(); },
    setCounts() {}, setPresence() {}, storageScope: "a1", deviceName: "MR Moqui desktop on test"
};

test("the shell shows a call and the Dialer itself, rings, and a room's call opens there; offline a call needs a connection", async () => {
    const shell = document.createElement("div");
    shell.id = "shell";
    document.body.appendChild(shell);
    window.moquiDesktop = desktopHost;
    moqui.request = serve;
    await moqui.loadScript("/files/crates/moqui-desktop/shell/js/shell.js");
    const root = await until(() => moqui.webrootVue && moqui.webrootVue.commsItems && moqui.webrootVue, "the shell's root");
    assert.ok(root.callsOn, "the server holds calls");
    assert.deepEqual(root.commsItems.map((item) => item.view), ["chat", "mail", "compose", "dialer"], "the Dialer in the drawer");

    // the call store started on the shell: the call ringing for the user shows in the ring bar
    const ring = await until(() => document.querySelector(".m-call-ring"), "the ring bar");
    assert.ok(ring.textContent.includes("Bob"));
    assert.ok(requests.includes("/apps/BasicComms/Calls/activeCalls GET"));

    // a ringing call while the user sees the window: the ring bar only; unfocused or hidden to the tray (the host says so,
    // whatever document.hidden says): a notification whose link is the call's screen, which the host shows here
    const ringPush = { message: JSON.stringify({ kind: "call", action: "ring", callSessionId: "C1", from: { name: "Bob" }, call: calls.C1 }) };
    moqui.call.receive(ringPush);
    assert.equal(host.notified.length, 0);
    host.seen = false;
    moqui.call.receive(ringPush);
    host.seen = true;
    assert.equal(host.notified.length, 1);
    assert.equal(host.notified[0].link, "/qapps/BasicComms/Calls/Call?callSessionId=C1");
    assert.equal(host.notified[0].tag, "call-C1");

    // that link, kept by the host (src/client/windows.rs open_page): the call's lobby here, with Answer
    host.pendingOpen = "/qapps/BasicComms/Calls/Call?callSessionId=C1";
    window.moquiShell.poke();
    const callEl = await until(() => document.querySelector(".m-shell-view .m-call"), "the call's screen");
    assert.equal(root.view, "call");
    assert.deepEqual(root.currentPathList, ["BasicComms", "Calls", "Call"]);
    assert.equal(root.currentParameters.callSessionId, "C1");
    await until(() => callEl.textContent.includes("Ringing for you"), "the call's state");
    assert.ok(Array.from(callEl.querySelectorAll("button")).some((button) => button.textContent.includes("Answer")), "Answer in the lobby");
    assert.equal(callEl.querySelector(".m-call-in-shell"), null, "the shell holds the device: no offer to go elsewhere");
    assert.ok(requests.includes("/apps/BasicComms/Calls/callState GET"));

    // a tel: link's Dialer, filled in, here
    host.pendingOpen = "/qapps/BasicComms/Calls/Dialer?number=5551234";
    window.moquiShell.poke();
    const dialer = await until(() => document.querySelector(".m-shell-view .m-dialer"), "the Dialer");
    assert.equal(root.view, "dialer");
    await until(() => requests.includes("/apps/BasicComms/Calls/Dialer/dialerData GET"), "the Dialer's data, from its own transitions");
    assert.equal(dialer.querySelector("input").value, "5551234");
    // a second tel: link while the Dialer is shown: its number fills it
    host.pendingOpen = "/qapps/BasicComms/Calls/Dialer?number=5559876";
    window.moquiShell.poke();
    await until(() => dialer.querySelector("input").value === "5559876", "the second number");
    assert.equal(document.querySelector(".m-shell-view .m-dialer"), dialer, "the same Dialer");
    // Calls' other screens are the server's pages, in an app window
    root.setUrl("/apps/BasicComms/Calls/History");
    assert.deepEqual(host.opened, ["/qapps/BasicComms/Calls/History"]);
    assert.equal(root.view, "dialer");

    // a room's call button: the call starts and shows here
    root.setUrl("/qapps/BasicComms/Chat?chatRoomId=R1");
    const button = await until(() => document.querySelector(".m-shell-view button[aria-label='Call']"), "the room's call button");
    button.click();
    await until(() => root.view === "call" && root.currentParameters.callSessionId === "C2", "the room's call, here");
    assert.ok(requests.includes("/apps/BasicComms/Calls/startCall POST"));
    assert.deepEqual(joined, [["C2", "L2"]]);

    // an encrypted call's SFrame worker: the staged script the shell's CSP names (src/client/assets.rs SHELL_CSP worker-src)
    const Worker = window.Worker;
    const workers = [];
    window.Worker = function (url) { workers.push(String(url)); this.postMessage = () => {}; this.terminate = () => {}; };
    try {
        moqui.callKeys.begin("C2", "L2");
        moqui.callKeys.end();
    } finally {
        window.Worker = Worker;
    }
    assert.deepEqual(workers, ["/js/WebrootSFrameWorker.js"]);
    assert.equal(moqui.callKeys.elsewhere(), false, "the shell holds the crypto device");

    // an account switch in a call: asked first, left, then switched
    moqui.call.state.current = "C2";
    root.switchAccount("a2");
    const leaveButton = await until(() => Array.from(document.querySelectorAll(".q-dialog button")).find((el) => el.textContent.includes("Leave the call")), "asked first");
    assert.deepEqual(host.switched, [], "not before the answer");
    leaveButton.click();
    await until(() => host.switched.length, "the switch");
    assert.deepEqual(left, ["C2"]);
    assert.deepEqual(host.switched, ["a2"]);
    // the sign-out box says so too
    moqui.call.state.current = "C2";
    root.signOut();
    await until(() => document.querySelector(".m-shell-sign-out-call"), "the sign-out says it leaves the call");
    root.signOutBox.open = false;
    moqui.call.state.current = null;

    // offline: a call, its lobby and the Dialer say they need a connection, and ask the server nothing
    moqui.setOffline(true);
    offline = true;
    assert.equal(moqui.connection.offline, true);
    root.setUrl("/qapps/BasicComms/Calls/Call?callSessionId=C1");
    await until(() => document.querySelector(".m-shell-view .m-call-offline"), "the call needs a connection");
    assert.ok(!document.querySelector(".m-shell-view .m-call").textContent.includes("Call not found"));
    const asked = requests.length;
    const started = await moqui.call.startCall({ chatRoomId: "R1", video: true });
    assert.equal(started.reasonCode, "offline");
    assert.equal((await moqui.call.answer("C1", true)).reasonCode, "offline");
    assert.equal((await moqui.call.decline("C1")).reasonCode, "offline");
    assert.equal(requests.length, asked, "nothing asked of the server");
    await until(() => text().includes("Calls need a connection to the server"), "said so");
    root.setUrl("/qapps/BasicComms/Calls/Dialer?number=1");
    const offlineDialer = await until(() => document.querySelector(".m-shell-view .m-dialer-offline") && document.querySelector(".m-shell-view .m-dialer"), "the Dialer offline");
    const callButton = Array.from(offlineDialer.querySelectorAll("button")).find((el) => /Call$/.test(el.textContent.trim()));
    const off = (button) => button.disabled || button.classList.contains("disabled");
    assert.ok(callButton && off(callButton), "Call is off");
    // the connection back: the Dialer reads again and Call is on, the calls going on are asked for, a call's Join returns
    const dialerReads = requests.filter((line) => line.startsWith("/apps/BasicComms/Calls/Dialer/dialerData")).length;
    const activeReads = requests.filter((line) => line.startsWith("/apps/BasicComms/Calls/activeCalls")).length;
    offline = false;
    moqui.setOffline(false);
    await until(() => !document.querySelector(".m-shell-view .m-dialer-offline"), "the Dialer online");
    await until(() => requests.filter((line) => line.startsWith("/apps/BasicComms/Calls/Dialer/dialerData")).length > dialerReads, "the Dialer read again");
    await until(() => requests.filter((line) => line.startsWith("/apps/BasicComms/Calls/activeCalls")).length > activeReads, "the calls asked for again");
    const backButton = Array.from(document.querySelectorAll(".m-shell-view .m-dialer button")).find((el) => /Call$/.test(el.textContent.trim()));
    await until(() => !off(backButton), "Call on again");
    root.setUrl("/qapps/BasicComms/Calls/Call?callSessionId=C1");
    const back = await until(() => document.querySelector(".m-shell-view .m-call"), "the call");
    await until(() => !back.querySelector(".m-call-offline"), "no offline banner");
    const answerButton = await until(() => Array.from(back.querySelectorAll("button")).find((button) => button.textContent.includes("Answer")), "Answer back");
    await until(() => !off(answerButton), "Answer on again");
    root.goHome();
});

// a screen of the comms app mounted as an app window or the web mounts it, with no shell around it
const mountScreen = (name, pathList, parameters) => {
    const Vue = window.Vue;
    const el = document.createElement("div");
    document.body.appendChild(el);
    const screen = moqui.bundledComponent(name);
    const app = Vue.createApp({
        data() { return { basePath: "/apps", linkBasePath: "/qapps", appRootPath: "", currentPathList: pathList, currentParameters: parameters, shellCounts: {},
            quickCreateShow: false, userId: "ann" }; },
        methods: { setUrl() {}, setParameters() {}, addContainer() {}, removeContainer() {} },
        render() { return Vue.h(screen); }
    });
    app.use(window.Quasar, { plugins: moqui.qvt.quasarPlugins(window.Quasar) });
    Object.keys(moqui.qvt.components).forEach((component) => app.component(component, moqui.qvt.components[component]));
    app.config.globalProperties.moqui = moqui;
    app.mount(el);
    return { app, el };
};

test("an app window leaves every call to the shell: it rings silently and opens a call there; the web is as it was", async () => {
    const shellHost = window.moquiDesktop;
    const asked = [];
    const callUrl = (id, extra) => "/qapps/BasicComms/Calls/Call?callSessionId=" + id + (extra || "");
    try {
        // an app window of the desktop client: the host's flag and its openInShell only
        window.__moquiCryptoInShell = true;
        window.moquiDesktop = { storageScope: "a1", socket: shellHost.socket, outbox: shellHost.outbox, drafts: shellHost.drafts,
            openInShell: (path) => { asked.push(path); return Promise.resolve(); } };
        assert.equal(moqui.callKeys.elsewhere(), true);
        assert.equal(moqui.call.inAppWindow(), true);
        await assert.rejects(moqui.callKeys.prepare(), (err) => err.reasonCode === "elsewhere");

        // the encrypted call: no lobby (no camera taken), Open in the app's Calls
        const encrypted = mountScreen("BasicComms/Calls/Call", ["BasicComms", "Calls", "Call"], { callSessionId: "E1" });
        const offer = await until(() => encrypted.el.querySelector(".m-call-in-shell"), "the offer to open it in the app's Calls");
        assert.ok(offer.textContent.includes("keeps its keys in the main window"));
        assert.ok(!Array.from(encrypted.el.querySelectorAll("button")).some((button) => button.textContent.includes("Answer")), "no Answer here");
        offer.querySelector("button").click();
        await until(() => asked.length, "the host asked");
        assert.deepEqual(asked, [callUrl("E1")]);
        encrypted.app.unmount();

        // a plain call: also the shell's, one owner of its media
        const plain = mountScreen("BasicComms/Calls/Call", ["BasicComms", "Calls", "Call"], { callSessionId: "P1" });
        const plainOffer = await until(() => plain.el.querySelector(".m-call-in-shell"), "the plain call's offer");
        assert.ok(plainOffer.textContent.includes("This app takes calls in its main window"));
        plain.app.unmount();

        // the ring bar's Answer and Answer with audio only, and Join: there, asking the server nothing, never answering here
        const before = requests.length;
        assert.equal((await moqui.call.answer("P1", true)).reasonCode, "elsewhere");
        assert.equal((await moqui.call.answer("E1", false)).reasonCode, "elsewhere");
        assert.equal((await moqui.call.joinCall("P1", { video: true })).reasonCode, "elsewhere");
        assert.deepEqual(asked.slice(1), [callUrl("P1"), callUrl("E1", "&video=false"), callUrl("P1")]);
        assert.equal(requests.length, before);
        const dock = moqui.qvt.components["m-call-dock"].methods;
        assert.equal(dock.answerLabel(calls.P1), "Answer in the app's Calls");
        assert.equal(dock.answerLabel(calls.P1, true), "Answer with audio only in the app's Calls");
        assert.equal(dock.canLeaveElsewhere(), false, "an app window does not end the shell's call");
        // Return to a call: there
        moqui.call.open("C2");
        assert.equal(asked[asked.length - 1], callUrl("C2"));
        // no ringtone here, nor a notification: the shell rings
        moqui.call.state.settings.ringtone = true;
        moqui.call.remember(callView("R5"));
        assert.equal(moqui.call.ringtone, null);
        moqui.call.state.settings.ringtone = false;

        // a request refused for want of the device here (a call not known to be encrypted): its screen in the shell
        const count = asked.length;
        answers["/apps/BasicComms/Calls/answerCall"] = { reasonCode: "noCryptoDevice" };
        const viaDevice = await moqui.call.requestWithDevice("answerCall", { callSessionId: "X1" }, false, "X1");
        assert.equal(viaDevice.reasonCode, "elsewhere");
        assert.deepEqual(asked.slice(count), [callUrl("X1")]);

        // a plain call started here: made by the server, joined in the shell with its leg, no media here
        joined.length = 0;
        const started = await moqui.call.startCall({ chatRoomId: "R1", video: true });
        assert.equal(started.callSessionId, "C2");
        assert.equal(asked[asked.length - 1], callUrl("C2"));
        assert.deepEqual(joined, []);

        // a room whose calls are encrypted, called from the app window: the server wants a device, so the room opens in Chat
        answers["/apps/BasicComms/Calls/startCall"] = { reasonCode: "noCryptoDevice", errors: "This room's calls are end-to-end encrypted" };
        const refused = await moqui.call.startCall({ chatRoomId: "R9", video: true });
        assert.equal(refused.reasonCode, "elsewhere");
        await until(() => asked[asked.length - 1] === "/qapps/BasicComms/Chat?chatRoomId=R9", "the room in the shell");
        const handedOff = asked.length;

        // the web: no flag and no host, so the encrypted call is this page's to take
        delete window.__moquiCryptoInShell;
        delete window.moquiDesktop;
        assert.equal(moqui.callKeys.elsewhere(), false);
        assert.equal(moqui.call.inAppWindow(), false);
        const web = mountScreen("BasicComms/Calls/Call", ["BasicComms", "Calls", "Call"], { callSessionId: "E1" });
        await until(() => web.el.textContent.includes("Ringing for you"), "the encrypted call on the web");
        assert.equal(web.el.querySelector(".m-call-in-shell"), null);
        assert.equal(web.el.querySelector(".m-call-offline"), null);
        assert.ok(Array.from(web.el.querySelectorAll("button")).some((button) => button.textContent.includes("Answer")), "its lobby");
        assert.equal(dock.answerLabel(calls.P1), "Answer");
        assert.equal(dock.canLeaveElsewhere(), true);
        web.app.unmount();
        assert.equal(asked.length, handedOff);
    } finally {
        delete window.__moquiCryptoInShell;
        window.moquiDesktop = shellHost;
    }
});
})();
