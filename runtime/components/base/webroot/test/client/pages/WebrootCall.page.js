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
        assert.equal(posted[1][1].encrypted, undefined, "the default where the page can encrypt");
        assert.deepEqual(sealed, [["D7", ["ann", "bob"], "Later"]]);
        assert.equal(posted[3][1].body, undefined, "no plain text to an encrypted room");
        assert.equal(posted[3][1].encryptedJson, "{}");
        // a phone call's caller has no chat room: declined, nothing sent
        posted.length = 0;
        state.calls.P1 = view("P1", { kind: "phone", myLegStatus: "offered", startedByUserId: null, chatRoomId: null });
        const phone = await call.declineWithText("P1", "Later");
        assert.equal(phone.texted, false);
        assert.deepEqual(posted.map((p) => p[0]), ["declineCall"]);
        // a page that can encrypt takes the default; one that cannot (a desktop app window) asks for the plain direct room,
        // so no encrypted room is made that it cannot write in (Comms-Messaging §6.5)
        assert.equal(posted.length, 1);
        posted.length = 0;
        moqui.crypto = { elsewhere: () => true };
        encrypted = false;
        state.calls.C3 = view("C3", { kind: "group", myLegStatus: "offered", startedByUserId: "bob", chatRoomId: "R3" });
        await call.declineWithText("C3", "Not now");
        assert.equal(posted[1][0], "createDirect");
        assert.equal(posted[1][1].encrypted, "false");
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
        // joined goes with the call's name (the other people, else the room's), then its kind once joined is told
        assert.equal(call.joinedName({ participants: [{ userId: call.me, displayName: "Me" }, { userId: "bob", displayName: "Bob" },
            { userId: "cy", displayName: "Cy" }], roomName: "Ops" }), "Bob, Cy");
        assert.equal(call.joinedName({ participants: [{ userId: call.me, displayName: "Me" }], roomName: "Ops" }), "Ops");
        assert.equal(call.joinedName(null), "");
        state.calls.C2 = { callSessionId: "C2", participants: [{ userId: "bob", displayName: "Bob" }], roomName: "Ops" };
        let joinedDone = null;
        const order = [];
        window.moquiDesktop = { callState: (id, what, name) => {
            order.push(what + (name ? ":" + name : ""));
            return what === "joined" ? new Promise((resolve) => { joinedDone = resolve; }) : Promise.resolve();
        } };
        const told2 = call.tellJoined("C2", "video");
        await Promise.resolve();
        assert.deepEqual(order, ["joined:Bob"], "the kind waits for joined");
        joinedDone();
        await told2;
        assert.deepEqual(order, ["joined:Bob", "video"]);
        delete state.calls.C2;
        // the call screen's token: each mount a newer one
        assert.ok(Number(call.screenToken()) < Number(call.screenToken()));
        // a hold that never settles gives up, and the next ask is not stuck on it
        const wait = call.HOLD_WAIT_MS;
        call.HOLD_WAIT_MS = 20;
        moqui.request = () => new Promise(() => {});
        Object.assign(state, { current: "C5", legId: "L5", phase: "connected", held: null });
        assert.equal(await call.hold(), null);
        assert.equal(call.holding, null);
        call.HOLD_WAIT_MS = wait;
    } finally {
        moqui.request = request;
        call.toggleMute = toggleMute;
        window.moquiDesktop = host;
        reset();
        Object.assign(state, { muted: false, hostAudio: null, hostPip: false });
        call.holding = null;
    }
});

test("the stage owner alone can capture, and takeover ends both share tracks", async () => {
    reset();
    const frames = [];
    const oldSend = call.send, oldPc = call.pc, oldConfirm = moqui.confirmAction;
    const devices = navigator.mediaDevices;
    const oldCapture = devices.getDisplayMedia;
    const live = stream("video","audio");
    let stopped = 0, options;
    live.getTracks().forEach(track => {track.stop=() => {stopped++;};});
    const senders = [];
    call.pc = {
        createOffer: async () => ({type:"offer",sdp:"test"}),
        setLocalDescription: async function(offer) {this.localDescription=offer;},
        addTransceiver(track) {
            const sender={track,replaceTrack(next) {this.track=next;return Promise.resolve();}};
            senders.push(sender);
            return {sender,mid:String(senders.length),direction:"sendonly"};
        }
    };
    const stage = (leg, revision) => call.handleFrame({rtc:"stage",callSessionId:"ST",revision,shareModeEnumId:"CsmAnyone",
        stage:{kind:leg?"screen":null,callLegId:leg,requests:[]}});
    try {
        Object.assign(state,{current:"ST",legId:"ME",stageRevision:-1,shareMode:"CsmAnyone",sharing:false});
        call.chain = Promise.resolve();
        call.send = frame => frames.push(frame);
        devices.getDisplayMedia = async opts => { options=opts;return live; };
        stage("OTHER",1);
        assert.equal(await call.shareScreen("window",false),false);
        assert.equal(options,undefined);
        moqui.confirmAction=async () => false;
        await call.requestPresentation();
        assert.equal(frames.length,0);
        moqui.confirmAction=async () => true;
        await call.requestPresentation();
        assert.equal(frames[0].action,"take");
        assert.equal(frames[0].previousLegId,"OTHER");
        stage("ME",2);
        assert.equal(await call.shareScreen("window",true),true);
        assert.equal(options.audio,true);
        assert.equal(options.systemAudio,"exclude");
        assert.equal(options.video.frameRate.ideal,30);
        assert.equal(live.getVideoTracks()[0].contentHint,"motion");
        assert.deepEqual(frames.filter(f=>f.rtc==="publish").map(f=>f.source),["screen","screenAudio"]);
        stage("OTHER",1);
        assert.equal(state.sharing,true,"stale stage ignored");
        stage(null,3);
        await new Promise(resolve=>setTimeout(resolve,0));
        assert.equal(state.sharing,false);
        assert.equal(stopped,2);
        assert.ok(senders.every(sender=>sender.track===null));
        assert.equal(frames.some(f=>f.action==="stop"),false,"server already ended the stage");
    } finally {
        call.send=oldSend; call.pc=oldPc; moqui.confirmAction=oldConfirm; devices.getDisplayMedia=oldCapture;
        call.screenTx=call.screenAudioTx=call.screenStream=null;
        Object.assign(state,{stage:{kind:null,callLegId:null,requests:[]},stageRevision:-1});
        reset();
    }
});

test("screen followers confirm their own access before navigation and can pause and catch up", async()=>{
    const prior={vm:call.vm,send:call.send,fetch:window.fetch,guest:call.guest};
    const sent=[],navigations=[],reads=[];
    try {
        call.followReset();
        Object.assign(state,{current:"FOLLOW",legId:"ME",stage:{kind:"follow",callLegId:"OWNER",followEpoch:"F"}});
        call.guest=null;call.vm={setUrl(url){navigations.push(url);call.followLocalNav();},activeContainers:{}};
        call.send=f=>sent.push(f);
        window.fetch=async(url,opts)=>{
            reads.push([url,opts.credentials]);
            return new Response("{}",{
                status:url.includes("wikiPageId=SECRET") && url.includes("/board?")?404:200});
        };
        const next=(path,params)=>call.followFrame({rtc:"follow",action:"navigate",from:"OWNER",path,params,followEpoch:"F"});
        call.followStart(state.stage);
        assert.equal(sent.at(-1).action,"catchUp");
        await next("/qapps/BasicComms/Home",{formValue:"hidden",sessionId:"discard"});
        assert.deepEqual(navigations,["/qapps/BasicComms/Home"]);
        await next("/qapps/BasicComms/Boards/Board",{wikiPageId:"SECRET"});
        assert.equal(state.followDenied,true);assert.equal(navigations.length,1);
        assert.deepEqual(reads.at(-1),["/apps/BasicComms/Boards/Board/board?wikiPageId=SECRET","same-origin"]);
        await next("/qapps/BasicComms/Chat",{chatRoomId:"private"});
        assert.equal(state.followDenied,true);assert.equal(navigations.length,1);
        assert.deepEqual(reads.at(-1),["/apps/BasicComms/Chat/room?chatRoomId=private","same-origin"]);
        call.followLocalNav();assert.equal(state.following,false);
        await next("/qapps/BasicComms/Calls",{});assert.equal(navigations.length,1);
        call.setFollowing(true);assert.equal(sent.at(-1).action,"catchUp");
        await next("/qapps/BasicComms/Calls",{});assert.equal(navigations.at(-1),"/qapps/BasicComms/Calls");
        await next("/apps/BasicComms/Home",{});
        assert.equal(navigations.at(-1),"/apps/BasicComms/Home");
        assert.deepEqual(reads.at(-1),["/apps/BasicComms/Home.qvt","same-origin"]);
        await next("/apps/BasicComms/Calls/Call",{callSessionId:"secret-record",sessionId:"drop"});
        assert.equal(state.followDenied,true);assert.equal(navigations.length,3);
        assert.deepEqual(reads.at(-1),["/apps/BasicComms/Calls/callState?callSessionId=secret-record","same-origin"]);
        const mount=document.createElement("div");mount.id="m-canvas";document.body.append(mount);
        try {
            mount.innerHTML='<main class="q-page"><div class="m-container-box" id="HomeRooms"></div><div class="m-container-box" id="HomeShortcuts"></div></main>';
            const section=mount.querySelector("#HomeShortcuts");
            for(const [el,y] of [[mount.firstChild,0],[mount.querySelector("#HomeRooms"),-60],[section,0]]){
                el.getBoundingClientRect=()=>({top:y,bottom:y+20});el.getClientRects=()=>[el.getBoundingClientRect()];
            }
            assert.equal(call.visibleSection(mount.firstChild),"HomeShortcuts");
            state.followDenied=false;let applied=0;
            section.scrollIntoView=()=>++applied;
            call.followLatest={section:"HomeShortcuts"};call.followLocate();call.followLocate();
            assert.equal(applied,1);
            const replacement=section.cloneNode(true);replacement.scrollIntoView=()=>++applied;section.replaceWith(replacement);
            await new Promise(r=>setTimeout(r,30));assert.equal(applied,2);
        }finally{mount.remove();}
        call.guest={};await next("/qapps/BasicComms/Chat",{chatRoomId:"room"});assert.equal(navigations.length,3);
        // On late admission the SFU can replay a MID either before or after Chrome fires ontrack.
        const canvas=document.createElement("canvas"), video=canvas.captureStream().getVideoTracks()[0],pc=call.pc;
        try {
            call.pc={};call.tracks={};call.trackSources={};state.remotes={};
            Object.defineProperty(video,"id",{value:"OWNER2"});
            const replay=()=>call.handleFrame({rtc:"track",callSessionId:"FOLLOW",callLegId:"OWNER",mid:"2",kind:"video",source:"screen",open:true});
            const track=()=>call.onTrack({track:video,streams:[{id:"OWNER"}]});
            replay();track();assert.equal(state.remotes.OWNER.screen.getVideoTracks()[0],video);
            call.tracks={};call.trackSources={};state.remotes={};
            track();assert.equal(state.remotes.OWNER.screen,null);replay();
            assert.equal(state.remotes.OWNER.screen.getVideoTracks()[0],video);
        }finally{video.stop();call.pc=pc;}

    } finally {
        call.followReset();reset();state.stage={kind:null,callLegId:null,requests:[]};
        call.vm=prior.vm;call.send=prior.send;call.guest=prior.guest;window.fetch=prior.fetch;
    }
});
