// The call store's rules of a call's flow (js/WebrootCall.js; plans/Comms-Messaging.md §8.3 items 1-7, phase 4), in the
// page with no media and no server: where a call shows, the prompt when the other side's video comes or goes, Move to
// this device's choice and its push, and the quick text of a declined call (an encrypted room's sealed, never plain).
const call = moqui.call;
const state = call.state;

// a stand-in for a MediaStream with tracks of these kinds
const stream = (...kinds) => {
    const tracks = kinds.map((kind) => ({ kind, readyState: "live", enabled: true, stop() {} }));
    return { getTracks: () => tracks, getVideoTracks: () => tracks.filter((t) => t.kind === "video"), getAudioTracks: () => tracks.filter((t) => t.kind === "audio") };
};
const reset = () => Object.assign(state, { current: null, legId: null, phase: "idle", remotes: {}, localStream: null, cameraOff: false,
    videoPrompt: null, panel: null, movedAway: null, held: null, calls: {}, ringing: [] });
const view = (id, extra) => Object.assign({ callSessionId: id, status: "active", kind: "direct", video: false, myLegStatus: "answered", myLegId: "L1",
    participants: [{ userId: "ann", displayName: "Ann", legs: [{ callLegId: "L1", status: "answered", live: true }] },
        { userId: "bob", displayName: "Bob", inCall: true, legs: [{ callLegId: "L2", status: "answered", live: true }] }] }, extra || {});
call.me = "ann";

test("the other person's video coming or going in a call of two is a prompt, not a new call; not on first sight, not their hold, not in a group", async () => {
    const tick = () => new Promise((resolve) => setTimeout(resolve, 20));
    const timing = call.timing;
    call.timing = { settle: 0, promptWait: 0 };
    try {
        reset();
        Object.assign(state, { current: "C1", legId: "L1", phase: "connected" });
        const bob = call.remote("L2");
        bob.name = "Bob";
        // a leg seen first with its video (a late joiner, the other's new leg after a move) is its start: no prompt
        bob.stream = stream("audio", "video");
        call.noteVideo("L2");
        await tick();
        assert.equal(state.videoPrompt, null, "the first sight of a leg");
        bob.cameraOff = true;
        call.noteVideo("L2");
        await tick();
        assert.equal(state.videoPrompt, null, "this page sends no video: nothing to ask when theirs goes");
        bob.cameraOff = false;
        call.noteVideo("L2");
        await tick();
        assert.deepEqual(state.videoPrompt, { legId: "L2", name: "Bob", on: true });
        // this page turns its video on too; Bob turns his off: asked whether to turn ours off
        state.videoPrompt = null;
        state.localStream = stream("audio", "video");
        bob.cameraOff = true;
        call.noteVideo("L2");
        await tick();
        assert.deepEqual(state.videoPrompt, { legId: "L2", name: "Bob", on: false });
        call.answerPrompt(false);
        assert.equal(state.videoPrompt, null, "Not now");
        // Bob's hold sends his camera off: not a choice to answer
        state.calls.C1 = view("C1");
        state.calls.C1.participants[1].onHold = true;
        bob.cameraOff = false;
        call.noteVideo("L2");
        bob.cameraOff = true;
        call.noteVideo("L2");
        await tick();
        assert.equal(state.videoPrompt, null, "the other side's hold");
        // a group call asks nothing
        state.calls.C1.participants[1].onHold = false;
        const cat = call.remote("L3");
        cat.stream = stream("audio");
        bob.cameraOff = false;
        call.noteVideo("L2");
        await tick();
        assert.equal(state.videoPrompt, null, "a group call");
    } finally {
        call.timing = timing;
    }
});

test("a call shows in the side panel on a desktop's width and on the call screen on a phone's", () => {
    reset();
    const opened = [];
    call.vm = { currentPathList: ["qapps", "BasicComms", "Chat"], currentParameters: {}, setUrl: (url) => opened.push(url) };
    const width = window.innerWidth;
    try {
        Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true });
        call.show("C1");
        assert.equal(state.panel, "C1");
        assert.deepEqual(opened, []);
        call.closePanel();
        assert.equal(state.panel, null);
        Object.defineProperty(window, "innerWidth", { value: 390, configurable: true });
        call.show("C2");
        assert.equal(state.panel, null);
        assert.ok(opened[0].endsWith("/Calls/Call?callSessionId=C2"), opened[0]);
    } finally {
        Object.defineProperty(window, "innerWidth", { value: width, configurable: true });
    }
});

test("Move to this device: offered for a call the user is in on another device; the push stops this page's moved leg", () => {
    reset();
    assert.equal(call.legElsewhere(view("C1")), true, "answered, its socket open elsewhere");
    const gone = view("C1");
    gone.participants[0].legs[0].live = false;
    assert.equal(call.legElsewhere(gone), false, "a reload's leg: joined again, not moved");
    state.current = "C1";
    assert.equal(call.legElsewhere(view("C1")), false, "the call this page is in");
    // the leg moved to the other device: this page's media stops, and it says where the call went
    Object.assign(state, { current: "C1", legId: "L1", phase: "connected" });
    call.movedElsewhere("C1", ["L9"]);
    assert.equal(state.current, "C1", "another leg's move is not this page's");
    call.movedElsewhere("C1", ["L1"]);
    assert.equal(state.current, null);
    assert.deepEqual(state.movedAway, { callSessionId: "C1" });
});

test("a declined call's quick text goes to the caller's direct room, sealed in an encrypted room", async () => {
    reset();
    const posted = [];
    const sealed = [];
    const request = moqui.request;
    const chat = moqui.chat;
    const crypto = moqui.crypto;
    let encrypted = false;
    moqui.request = (url, options) => {
        posted.push([url.replace(/^.*\/(\w+)$/, "$1"), (options && options.params) || {}]);
        if (url.endsWith("/declineCall")) return Promise.resolve({ json: { call: null } });
        if (url.endsWith("/createDirect")) return Promise.resolve({ json: { chatRoomId: "D7" } });
        if (url.endsWith("/room")) return Promise.resolve({ json: { room: { encrypted }, memberList: [{ userId: "ann" }, { userId: "bob" }] } });
        return Promise.resolve({ json: { eventSeq: 1 } });
    };
    moqui.chat = { escapeHtml: (text) => String(text), sealMessage: (roomId, members, html) => { sealed.push([roomId, members, html]); return Promise.resolve({ encryptedJson: "{}", mentionUserIds: "" }); } };
    moqui.crypto = {};
    try {
        state.calls.C1 = view("C1", { myLegStatus: "offered", startedByUserId: "bob", chatRoomId: "R1" });
        const direct = await call.declineWithText("C1", "Can't talk now");
        assert.equal(direct.texted, true);
        assert.deepEqual(posted.map((p) => p[0]), ["declineCall", "room", "send"]);
        assert.equal(posted[2][1].chatRoomId, "R1");
        assert.equal(posted[2][1].body, "Can't talk now");
        // a group call: the caller's direct room with the user, found or made; an encrypted one gets ciphertext only
        posted.length = 0;
        encrypted = true;
        state.calls.G1 = view("G1", { kind: "group", myLegStatus: "offered", startedByUserId: "bob", chatRoomId: "R2" });
        const group = await call.declineWithText("G1", "Later");
        assert.equal(group.texted, true);
        assert.deepEqual(posted.map((p) => p[0]), ["declineCall", "createDirect", "room", "send"]);
        assert.equal(posted[1][1].userIds, "bob");
        assert.deepEqual(sealed, [["D7", ["ann", "bob"], "Later"]]);
        assert.equal(posted[3][1].body, undefined, "no plain text to an encrypted room");
        assert.equal(posted[3][1].encryptedJson, "{}");
        // a phone call's caller has no chat room: declined, nothing sent
        posted.length = 0;
        state.calls.P1 = view("P1", { kind: "phone", myLegStatus: "offered", startedByUserId: null, chatRoomId: null });
        const phone = await call.declineWithText("P1", "Later");
        assert.equal(phone.texted, false);
        assert.deepEqual(posted.map((p) => p[0]), ["declineCall"]);
    } finally {
        moqui.request = request;
        moqui.chat = chat;
        moqui.crypto = crypto;
    }
});

test("call waiting: a refused hold keeps the call and answers nothing; a held leg that moved or a reloaded page's held leg", async () => {
    reset();
    const request = moqui.request;
    const answer = call.answer;
    const answered = [];
    let refuse = true;
    moqui.request = (url) => {
        if (url.endsWith("/holdOwnLeg")) return Promise.resolve({ json: refuse ? { reasonCode: "notFound", errors: "Call not found" } : { callLegId: "L1" } });
        return Promise.resolve({ json: {} });
    };
    call.answer = (id) => { answered.push(id); return Promise.resolve({}); };
    try {
        Object.assign(state, { current: "C1", legId: "L1", phase: "connected" });
        const out = await call.holdAndAnswer("C2", false);
        assert.equal(out.reasonCode, "notHeld");
        assert.equal(state.current, "C1", "the call goes on");
        assert.deepEqual(answered, []);
        refuse = false;
        await call.holdAndAnswer("C2", false);
        assert.deepEqual(state.held && [state.held.callSessionId, state.held.legId], ["C1", "L1"]);
        assert.deepEqual(answered, ["C2"]);
        // the held leg taken to another device: held here no more
        call.movedElsewhere("C1", ["L1"]);
        assert.equal(state.held, null);
        assert.deepEqual(state.movedAway, { callSessionId: "C1" });
        // a reloaded page: the view marks the user's leg held, with no socket holding it
        reset();
        const reloaded = view("C3");
        reloaded.participants[0].legs[0].held = true;
        reloaded.participants[0].legs[0].live = false;
        call.remember(reloaded);
        assert.deepEqual(state.held && [state.held.callSessionId, state.held.legId, state.held.name], ["C3", "L1", "Bob"]);
        call.remember(view("C3", { status: "ended" }));
        assert.equal(state.held, null, "a held call that ended");
    } finally {
        moqui.request = request;
        call.answer = answer;
    }
});

// plans/Comms-Messaging.md §8.3 items 4, 5 and 7, the mobile app's side: the speaker button chooses the native call's
// route, the app's picture-in-picture shows the video only, and Telecom's hold and resume reach the call
test("the mobile app's native call: its audio routes, picture-in-picture, Telecom's hold, and the held call left", async () => {
    reset();
    const host = window.moquiDesktop;
    const told = [];
    const routes = [];
    window.moquiDesktop = {
        callState: (id, what) => { told.push(id + ":" + what); return Promise.resolve(); },
        callAudioRoute: (route) => { routes.push(route); return Promise.resolve(true); }
    };
    const request = moqui.request;
    moqui.request = (url) => Promise.resolve({ json: url.endsWith("/holdOwnLeg") ? { callLegId: "L1" } : {} });
    try {
        assert.deepEqual(call.hostRoutes(), [], "no routes before the host tells them");
        call.setHostAudio({ route: "earpiece", routes: ["earpiece", "speaker", "bluetooth"] });
        assert.deepEqual(call.hostRoutes(), ["earpiece", "speaker", "bluetooth"]);
        assert.equal(await call.chooseHostRoute("speaker"), true);
        assert.deepEqual(routes, ["speaker"]);
        assert.equal(state.hostAudio.route, "speaker");
        call.setHostAudio({ route: "earpiece", routes: ["earpiece"] });
        assert.deepEqual(call.hostRoutes(), [], "one route: nothing to choose");
        call.setHostPip(true);
        assert.equal(state.hostPip, true);
        call.setHostPip(false);
        // Telecom holds the call this page is in: the page holds it, and says so back
        Object.assign(state, { current: "C1", legId: "L1", phase: "connected" });
        await call.hostHold("C1", true);
        assert.deepEqual(state.held && state.held.callSessionId, "C1");
        assert.ok(told.includes("C1:held"));
        // another call's hold: nothing
        assert.equal(await call.hostHold("C9", true), null);
        // the held call left from its banner: the native call on hold ends too
        await call.leaveHeld();
        assert.ok(told.includes("C1:left"));
    } finally {
        moqui.request = request;
        window.moquiDesktop = host;
        reset();
        Object.assign(state, { hostAudio: null, hostPip: false });
    }
});

// Review findings of phase 4's Android part: a held call that ends or moves tells the app; Telecom's hold of the call
// this page is in, with another on hold, is refused; a hold on its way answers once; Telecom's mute; joined with its name
test("the app's native calls: a held call gone tells the app, one hold at a time, Telecom's mute, joined with its name", async () => {
    reset();
    const host = window.moquiDesktop;
    const told = [];
    let holds = 0;
    let release = null;
    window.moquiDesktop = { callState: (id, what, name) => { told.push(id + ":" + what + (name ? ":" + name : "")); return Promise.resolve(); } };
    const request = moqui.request;
    moqui.request = (url) => {
        if (url.endsWith("/holdOwnLeg")) {
            holds++;
            return new Promise((resolve) => { release = () => resolve({ json: { callLegId: "L1" } }); });
        }
        return Promise.resolve({ json: {} });
    };
    const toggles = [];
    const toggleMute = call.toggleMute;
    call.toggleMute = () => { toggles.push(true); state.muted = !state.muted; };
    try {
        // a hold on its way: Telecom's and the page's at once are one
        Object.assign(state, { current: "C1", legId: "L1", phase: "connected" });
        const first = call.hold();
        const second = call.hostHold("C1", true);
        assert.equal(holds, 1, "one hold asked");
        release();
        assert.deepEqual([(await first).callSessionId, (await second).callSessionId], ["C1", "C1"]);
        // another call in, with C1 on hold: Telecom's hold of it is refused (C1 stays the held one)
        Object.assign(state, { current: "C2", legId: "L2" });
        assert.equal(await call.hostHold("C2", true), null);
        assert.equal(state.held.callSessionId, "C1");
        // the held call ends on the server: the app hears it left
        call.receive({ topic: "chatUser:ann", message: { kind: "call", action: "ended", callSessionId: "C1" } });
        assert.equal(state.held, null);
        assert.ok(told.includes("C1:left"), told.join(" "));
        // a held leg moved to another device: left too
        state.held = { callSessionId: "C3", legId: "L3", video: false, name: "" };
        call.movedElsewhere("C3", ["L3"]);
        assert.ok(told.includes("C3:left"));
        // Telecom's mute: the microphone follows, once
        state.muted = false;
        call.setHostMute(true);
        call.setHostMute(true);
        assert.equal(toggles.length, 1);
        // joined goes with the call's name, then its kind after it
        await call.hostCallState("C2", "joined", "Bob");
        assert.ok(told.includes("C2:joined:Bob"));
    } finally {
        moqui.request = request;
        call.toggleMute = toggleMute;
        window.moquiDesktop = host;
        reset();
        Object.assign(state, { muted: false, hostAudio: null, hostPip: false });
        call.holding = null;
    }
});
