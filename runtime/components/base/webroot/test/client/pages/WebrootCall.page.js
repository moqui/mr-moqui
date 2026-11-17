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

test("the other side's video coming or going is a prompt, not a new call: on when this page sends none, off when it still sends", () => {
    reset();
    Object.assign(state, { current: "C1", legId: "L1", phase: "connected" });
    const bob = call.remote("L2");
    bob.name = "Bob";
    bob.stream = stream("audio");
    call.noteVideo("L2");
    assert.equal(state.videoPrompt, null, "audio only: nothing to ask");
    bob.stream = stream("audio", "video");
    call.noteVideo("L2");
    assert.deepEqual(state.videoPrompt, { legId: "L2", name: "Bob", on: true });
    // this page turns its video on too: nothing to ask when Bob turns his off... until he does
    state.videoPrompt = null;
    state.localStream = stream("audio", "video");
    bob.cameraOff = true;
    call.noteVideo("L2");
    assert.deepEqual(state.videoPrompt, { legId: "L2", name: "Bob", on: false });
    // Not now: no prompt, and nothing changes
    call.answerPrompt(false);
    assert.equal(state.videoPrompt, null);
    // while joining, a track coming is the call's start, not a change
    reset();
    Object.assign(state, { current: "C1", legId: "L1", phase: "joining" });
    call.remote("L2").stream = stream("audio", "video");
    call.noteVideo("L2");
    assert.equal(state.videoPrompt, null);
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
    } finally {
        moqui.request = request;
        moqui.chat = chat;
        moqui.crypto = crypto;
    }
});
