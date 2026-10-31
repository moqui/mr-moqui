/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * Calls in the browser (plans/Comms-Framework.md §6.3, §6.6, W1): the page's call store (moqui.call), the media of the call
 * this page is in, and the shell's parts of a call: the ring bar in every tab of the user and the LiveCall window that keeps
 * a call while the user goes to another screen (m-call-dock, which the shell puts in its canvas), and m-call, the lobby
 * and call view a Call screen shows (any app may embed one by callSessionId; the services check the user's part).
 *
 * The state of a call comes from the call services (the Calls screen's JSON transitions under the Comms app, config.calls)
 * and their pushes on chatUser:<me> ({kind: 'call', action, call}). The media goes through the server's SFU: one
 * RTCPeerConnection, signaled with rtc frames on the notification socket (join with the offer, the SFU's offers as other
 * people's tracks come and go, answers, candidates; replies on rtc:<callSessionId>). The SFU is the polite side: when both
 * offer at once this page keeps its own and ignores the SFU's, which offers again. Camera video goes in three simulcast
 * layers (rids l, m, h); the SFU sends each subscriber the one that fits. After the socket comes back, or the SFU says the
 * media was lost, the page joins again, which takes the leg's next generation and fences the old connection.
 *
 * Loaded after MoquiLib.js and before WebrootVue.qvt.js (qapps.xml): its components are in moqui.qvtComponents, and the shell
 * root calls moqui.call.start(root) once mounted. Device choices are kept in this browser only (Settings, Calls).
 */
(function (root) {
    "use strict";
    const moqui = (root.moqui = root.moqui || {});
    const reactive = (obj) => (root.Vue && root.Vue.reactive ? root.Vue.reactive(obj) : obj);
    const t = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);
    const SETTINGS_KEY = "moqui.call.settings";
    const LAYERS = [
        { rid: "l", scaleResolutionDownBy: 4, maxBitrate: 150000 },
        { rid: "m", scaleResolutionDownBy: 2, maxBitrate: 500000 },
        { rid: "h", maxBitrate: 1500000 }
    ];

    function readSettings() {
        try {
            const text = root.localStorage && root.localStorage.getItem(SETTINGS_KEY);
            return Object.assign({ audioIn: "", videoIn: "", audioOut: "", joinMuted: false, cameraOff: false, ringtone: true }, text ? JSON.parse(text) : {});
        } catch (e) {
            return { audioIn: "", videoIn: "", audioOut: "", joinMuted: false, cameraOff: false, ringtone: true };
        }
    }
    function writeSettings(settings) {
        try {
            if (root.localStorage) root.localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
        } catch (e) {
            /* a private window: the choices last for this page only */
        }
    }

    const state = reactive({
        // the calls this page knows, by callSessionId (each as get#Call shows it), and the ones ringing for me
        calls: {},
        ringing: [],
        // the call this page is in: its id, leg and media
        current: null,
        legId: null,
        phase: "idle", // idle, joining, connected, reconnecting
        error: "",
        muted: false,
        cameraOff: false,
        audioOnly: false,
        localStream: null,
        // the other people's media: {legId: {legId, name, stream, connected, muted, cameraOff, hand}}
        remotes: {},
        quality: null,
        startedAt: 0,
        // the leg speaking now (the SFU's activeSpeaker), the layout, my screen share and raised hand
        speaker: null,
        layout: "grid", // grid, speaker
        sharing: false,
        hand: false,
        participantsOpen: false,
        settings: readSettings(),
        capabilities: null
    });

    const call = {
        state,
        started: false,
        vm: null,
        me: null,
        pc: null,
        generation: 0,
        pendingOffer: false,
        unsubscribe: null,
        trackSources: {},
        tracks: {},
        sentLayers: {},
        offScreen: false,
        ringtone: null,
        nextId: 1,

        // ---- the Comms app's call transitions ----
        appRoot() {
            const config = moqui.config || {};
            return (config.calls && config.calls.appRoot) || "/qapps/BasicComms";
        },
        base() {
            return "/apps" + call.appRoot().replace(/^\/qapps/, "") + "/Calls";
        },
        callUrl(callSessionId) {
            return call.appRoot() + "/Calls/Call?callSessionId=" + encodeURIComponent(callSessionId);
        },
        request(name, params, method) {
            return moqui.request(call.base() + "/" + name, { method: method || "POST", params: params || {} }).then((result) => {
                const json = result.json || {};
                if (json.errors) {
                    if (moqui.notifyMessages) moqui.notifyMessages([], json.errors);
                    else console.warn(json.errors);
                }
                if (json.call) call.remember(json.call);
                return json;
            });
        },
        remember(view) {
            if (!view || !view.callSessionId) return;
            state.calls[view.callSessionId] = view;
            const ringingForMe = view.status !== "ended" && view.myLegStatus === "offered";
            const at = state.ringing.indexOf(view.callSessionId);
            if (ringingForMe && at < 0) state.ringing.push(view.callSessionId);
            if (!ringingForMe && at >= 0) state.ringing.splice(at, 1);
            call.ring(state.ringing.length > 0);
        },

        // ---- the store: started once the shell is mounted ----
        start(vm) {
            if (call.started || !vm || !vm.userId || !(moqui.config || {}).calls) return;
            call.started = true;
            call.vm = vm;
            call.me = vm.userId;
            const client = vm.notificationClient;
            if (client) {
                client.registerListener("chatUser:" + call.me, call.receive);
                if (client.onState) client.onState((open) => { if (open && state.current) call.rejoin(); });
            }
            call.request("activeCalls", {}, "GET").then((json) => (json.callList || []).forEach(call.remember), () => null);
        },

        // A push on chatUser:<me>: its message (JSON text, or already an object) when it is a call's
        receive(jsonObj) {
            let data = jsonObj ? jsonObj.message : null;
            if (typeof data === "string") {
                try { data = JSON.parse(data); } catch (err) { data = null; }
            }
            if (!data || data.kind !== "call") return;
            if (data.call) call.remember(data.call);
            if (data.action === "ended" && state.current === data.callSessionId) call.hangUp("ended");
            if (data.action === "ring" && root.document && root.document.hidden && root.Notification && root.Notification.permission === "granted") {
                // a tab in the background shows the ring as a notification too (a closed one gets the Web Push)
                try {
                    new root.Notification(t("Call from") + " " + ((data.from && data.from.name) || ""), { tag: "call-" + data.callSessionId });
                } catch (e) {
                    /* not allowed here */
                }
            }
        },

        // ---- ringing ----
        ring(on) {
            if (!on || !state.settings.ringtone) {
                if (call.ringtone) {
                    clearInterval(call.ringtone.timer);
                    try { call.ringtone.ctx.close(); } catch (e) { /* closed */ }
                    call.ringtone = null;
                }
                return;
            }
            if (call.ringtone || !(root.AudioContext || root.webkitAudioContext)) return;
            const Ctx = root.AudioContext || root.webkitAudioContext;
            const ctx = new Ctx();
            const beep = () => {
                [0, 0.4].forEach((at) => {
                    const osc = ctx.createOscillator();
                    const gain = ctx.createGain();
                    osc.frequency.value = 440;
                    gain.gain.value = 0.08;
                    osc.connect(gain).connect(ctx.destination);
                    osc.start(ctx.currentTime + at);
                    osc.stop(ctx.currentTime + at + 0.3);
                });
            };
            beep();
            call.ringtone = { ctx, timer: setInterval(beep, 3000) };
        },

        // ---- the call actions ----
        startCall(params) {
            return call.request("startCall", Object.assign({ idempotencyKey: "c" + Date.now() + Math.random().toString(36).slice(2) }, params)).then((json) => {
                if (json.callSessionId && json.callLegId) {
                    call.open(json.callSessionId);
                    return call.join(json.callSessionId, json.callLegId, { video: params.video !== false }).then(() => json);
                }
                if (json.reasonCode === "callActive" && json.callSessionId) call.open(json.callSessionId);
                return json;
            });
        },
        answer(callSessionId, video) {
            return call.request("answerCall", { callSessionId }).then((json) => {
                if (!json.callLegId) return json;
                call.open(callSessionId);
                return call.join(callSessionId, json.callLegId, { video: video !== false }).then(() => json);
            });
        },
        decline(callSessionId) {
            return call.request("declineCall", { callSessionId });
        },
        joinCall(callSessionId, options) {
            return call.request("joinCall", { callSessionId }).then((json) => {
                if (json.callLegId) return call.join(callSessionId, json.callLegId, options || {}).then(() => json);
                return json;
            });
        },
        leave() {
            const id = state.current;
            const leg = state.legId;
            if (!id) return Promise.resolve({});
            call.send({ rtc: "leave", callSessionId: id, callLegId: leg, id: call.nextId++ });
            call.hangUp("left");
            return call.request("leaveCall", { callSessionId: id, callLegId: leg });
        },
        endForAll() {
            const id = state.current;
            if (!id) return Promise.resolve({});
            call.hangUp("ended");
            return call.request("endCall", { callSessionId: id });
        },
        open(callSessionId) {
            if (call.vm && call.vm.setUrl) call.vm.setUrl(call.callUrl(callSessionId));
        },

        // ---- the media ----
        send(frame) {
            const client = call.vm && call.vm.notificationClient;
            if (client) client.send(frame);
        },
        constraints(video) {
            const s = state.settings;
            return {
                audio: s.audioIn ? { deviceId: { exact: s.audioIn } } : true,
                video: video ? (s.videoIn ? { deviceId: { exact: s.videoIn } } : { width: 1280, height: 720 }) : false
            };
        },
        getMedia(video) {
            const media = root.navigator && root.navigator.mediaDevices;
            if (!media || !media.getUserMedia) return Promise.resolve(null);
            return media.getUserMedia(call.constraints(video)).catch(() =>
                // no camera, or no permission for it: audio only
                (video ? media.getUserMedia(call.constraints(false)) : Promise.reject(new Error("no media"))).then(
                    (stream) => { state.audioOnly = true; return stream; },
                    () => null
                )
            );
        },

        // Joins the media of a call with the user's leg in it: the camera and microphone, the offer, and the socket's frames.
        join(callSessionId, legId, options) {
            if (state.current && state.current !== callSessionId) call.hangUp("switched");
            const video = options.video !== false && !state.settings.cameraOff;
            state.current = callSessionId;
            state.legId = legId;
            state.phase = "joining";
            state.error = "";
            state.audioOnly = !video;
            state.startedAt = state.startedAt || Date.now();
            const keep = options.stream || state.localStream;
            const media = keep ? Promise.resolve(keep) : call.getMedia(video);
            return media.then((stream) => {
                if (state.current !== callSessionId) return;
                if (!stream) state.error = t("No microphone or camera: you can hear and see the others, they cannot hear or see you.");
                state.localStream = stream;
                if (stream && (state.settings.joinMuted || state.muted)) stream.getAudioTracks().forEach((track) => (track.enabled = false));
                state.muted = !!(stream && stream.getAudioTracks().some((track) => !track.enabled));
                return call.connect(stream);
            }).catch((err) => {
                state.error = String((err && err.message) || err);
                state.phase = "idle";
            });
        },

        connect(stream) {
            const id = state.current;
            const legId = state.legId;
            if (call.unsubscribe) call.unsubscribe();
            const client = call.vm && call.vm.notificationClient;
            if (client) call.unsubscribe = client.registerListener("rtc:" + id, call.onFrame);
            if (call.pc) {
                try { call.pc.close(); } catch (e) { /* closed */ }
            }
            const pc = new root.RTCPeerConnection({ bundlePolicy: "max-bundle", iceServers: [] });
            call.pc = pc;
            call.chain = Promise.resolve();
            call.sentLayers = {};
            call.screenTx = null;
            state.sharing = false;
            call.pendingOffer = true;
            const audio = stream && stream.getAudioTracks()[0];
            const camera = stream && stream.getVideoTracks()[0];
            const audioTx = audio ? pc.addTransceiver(audio, { direction: "sendonly", streams: [stream] }) : null;
            const videoTx = camera ? pc.addTransceiver(camera, { direction: "sendonly", streams: [stream], sendEncodings: LAYERS }) : null;
            pc.ontrack = (event) => call.onTrack(event);
            // a candidate before the join is bound waits for it (the socket refuses a frame of a leg it has not joined)
            call.early = [];
            pc.onicecandidate = (event) => {
                if (!event.candidate || !event.candidate.candidate) return;
                const frame = { rtc: "candidate", callSessionId: id, callLegId: legId, candidate: event.candidate.candidate };
                if (call.early) call.early.push(frame);
                else call.send(frame);
            };
            pc.onconnectionstatechange = () => {
                if (pc !== call.pc) return;
                if (pc.connectionState === "connected") state.phase = "connected";
                if (pc.connectionState === "failed") call.rejoin();
            };
            return pc.createOffer().then((offer) => pc.setLocalDescription(offer)).then(() => {
                call.publish = [audioTx, videoTx].filter((tx) => tx && tx.mid).map((tx) => ({ mid: tx.mid, source: tx === videoTx ? "camera" : "microphone" }));
                call.send({ rtc: "join", callSessionId: id, callLegId: legId, sdp: pc.localDescription.sdp, id: call.nextId++ });
            });
        },

        rejoin() {
            if (!state.current || state.phase === "reconnecting") return;
            state.phase = "reconnecting";
            setTimeout(() => {
                if (!state.current) return;
                call.connect(state.localStream).catch((err) => (state.error = String((err && err.message) || err)));
            }, 500);
        },

        // The socket's frames for the call, one at a time in their order: a description is applied before the next
        // frame is looked at, so the SFU's offer that follows "joined" meets the page's own offer answered.
        onFrame(message) {
            const frame = message && message.rtc ? message : null;
            if (!frame || frame.callSessionId !== state.current) return;
            call.chain = (call.chain || Promise.resolve())
                .then(() => call.handleFrame(frame))
                .catch((err) => console.warn("call frame " + frame.rtc + " failed", err));
        },
        handleFrame(frame) {
            const pc = call.pc;
            if (!pc || frame.callSessionId !== state.current) return null;
            switch (frame.rtc) {
                case "joined":
                    call.generation = frame.generation;
                    return pc.setRemoteDescription({ type: "answer", sdp: frame.sdp }).then(() => {
                        call.pendingOffer = false;
                        (call.early || []).forEach((early) => call.send(early));
                        call.early = null;
                        (call.publish || []).forEach((p) =>
                            call.send({ rtc: "publish", callSessionId: state.current, callLegId: state.legId, mid: p.mid, source: p.source }));
                    }, (err) => (state.error = String(err)));
                case "answer":
                    return pc.setRemoteDescription({ type: "answer", sdp: frame.sdp }).then(() => (call.pendingOffer = false), () => null);
                case "offer":
                    // impolite: while this page's own offer is out, the SFU's waits; the SFU gives way and offers again
                    if (call.pendingOffer || pc.signalingState !== "stable") return null;
                    return pc.setRemoteDescription({ type: "offer", sdp: frame.sdp })
                        .then(() => pc.createAnswer())
                        .then((answer) => pc.setLocalDescription(answer))
                        .then(() => call.send({ rtc: "answer", callSessionId: state.current, callLegId: state.legId, offerId: frame.offerId, sdp: pc.localDescription.sdp }));
                case "connected":
                    state.phase = "connected";
                    break;
                case "mediaLost":
                    call.rejoin();
                    break;
                case "track":
                    if (frame.callLegId === state.legId) break;
                    call.trackSources[frame.callLegId + ":" + frame.mid] = frame.source;
                    if (!frame.open) call.dropTrack(frame.callLegId, frame.mid);
                    else call.sortTracks(frame.callLegId);
                    break;
                case "activeSpeaker":
                    state.speaker = frame.speaker || null;
                    call.chooseLayers();
                    break;
                case "layer":
                    if (frame.publisher) call.remote(frame.publisher).audioOnly = frame.layer == null;
                    break;
                case "reaction":
                    if (frame.from) call.react(frame.from, frame.emoji);
                    break;
                case "participant":
                    if (frame.from || frame.callLegId === state.legId) break;
                    call.remote(frame.callLegId).connected = !!frame.connected;
                    if (!frame.connected) delete state.remotes[frame.callLegId];
                    break;
                case "mute":
                    if (frame.from) Object.assign(call.remote(frame.from), { muted: !!frame.audioMuted, cameraOff: !!frame.cameraOff });
                    break;
                case "hand":
                    if (frame.from) call.remote(frame.from).hand = !!frame.raised;
                    break;
                case "quality":
                    state.quality = frame.kbps;
                    break;
                case "ended":
                    call.hangUp(frame.reason || "ended");
                    break;
                case "error":
                    if (frame.code === "staleGeneration") return null;
                    state.error = frame.message || frame.code;
                    if (frame.code === "notFound" || frame.code === "callEnded" || frame.code === "legClosed") call.hangUp("ended");
                    break;
                default:
                    break;
            }
            return null;
        },

        remote(legId) {
            if (!state.remotes[legId]) {
                state.remotes[legId] = { legId, name: call.nameOf(legId), stream: null, screen: null, connected: true, muted: false,
                    cameraOff: false, hand: false, reaction: null, audioOnly: false };
                call.tracks[legId] = [];
            }
            return state.remotes[legId];
        },
        // A leg's tracks as two streams: its camera and microphone, and its screen share (by the SFU's track frames)
        sortTracks(legId) {
            const remote = state.remotes[legId];
            const tracks = (call.tracks[legId] || []).filter((tr) => tr.readyState !== "ended");
            if (!remote) return;
            const isScreen = (tr) => call.trackSources[tr.id] === "screen" || call.trackSources[tr.id] === "screenAudio";
            const main = tracks.filter((tr) => !isScreen(tr));
            const screen = tracks.filter(isScreen);
            remote.stream = main.length ? new root.MediaStream(main) : null;
            remote.screen = screen.some((tr) => tr.kind === "video") ? new root.MediaStream(screen) : null;
            call.chooseLayers();
        },
        react(legId, emoji) {
            const remote = call.remote(legId);
            remote.reaction = String(emoji || "").slice(0, 8);
            setTimeout(() => { if (remote.reaction === emoji) remote.reaction = null; }, 3000);
        },
        nameOf(legId) {
            const view = state.calls[state.current];
            const person = view && (view.participants || []).find((p) => (p.legs || []).some((leg) => leg.callLegId === legId));
            return person ? person.displayName : t("Someone");
        },
        onTrack(event) {
            const stream = event.streams && event.streams[0];
            const legId = stream ? stream.id : "";
            if (!legId || legId === state.legId) return;
            call.remote(legId);
            const list = call.tracks[legId];
            if (!list.includes(event.track)) list.push(event.track);
            event.track.onended = () => call.dropTrack(legId, null, event.track);
            call.sortTracks(legId);
        },
        dropTrack(legId, mid, track) {
            const list = call.tracks[legId];
            if (!list) return;
            call.tracks[legId] = list.filter((tr) => (track ? tr !== track : tr.id !== legId + ":" + mid));
            call.sortTracks(legId);
        },

        // ---- what to receive: each publisher's layer by how big it is shown ----
        // In the speaker view the speaker (or a screen share) is large and the rest small; in the grid the tiles shrink with
        // their number; off the call's screen (the LiveCall window) only the speaker is shown.
        chooseLayers() {
            const legs = Object.keys(state.remotes);
            if (!state.current || !legs.length) return;
            const sharing = legs.find((leg) => state.remotes[leg].screen);
            const main = sharing || state.speaker || legs[0];
            const layers = {};
            for (const leg of legs) {
                if (state.layout === "speaker" || call.offScreen) layers[leg] = leg === main ? (call.offScreen ? "mid" : "high") : call.offScreen ? "none" : "low";
                else layers[leg] = legs.length <= 2 ? "high" : legs.length <= 4 ? "mid" : "low";
            }
            call.sentLayers = call.sentLayers || {};
            for (const leg of legs) {
                if (call.sentLayers[leg] === layers[leg]) continue;
                call.sentLayers[leg] = layers[leg];
                call.send({ rtc: "subscribe", callSessionId: state.current, callLegId: state.legId, publisher: leg, layer: layers[leg] });
            }
        },
        setOffScreen(off) {
            if (call.offScreen === off) return;
            call.offScreen = off;
            call.chooseLayers();
        },
        setLayout(layout) {
            state.layout = layout === "speaker" ? "speaker" : "grid";
            call.chooseLayers();
        },

        // ---- screen share: a track of its own, offered by the page; the SFU answers ----
        shareScreen() {
            const media = root.navigator && root.navigator.mediaDevices;
            if (!call.pc || !media || !media.getDisplayMedia || state.sharing) return Promise.resolve(false);
            return media.getDisplayMedia({ video: { frameRate: 15 }, audio: false }).then((stream) => {
                const track = stream.getVideoTracks()[0];
                if (!track || !call.pc) return false;
                call.screenStream = stream;
                const reuse = call.screenTx && call.screenTx.currentDirection !== "stopped" ? call.screenTx : null;
                if (reuse) {
                    reuse.direction = "sendonly";
                    reuse.sender.replaceTrack(track);
                } else {
                    call.screenTx = call.pc.addTransceiver(track, { direction: "sendonly", streams: [stream] });
                }
                track.onended = () => call.stopShare();
                state.sharing = true;
                return call.renegotiate("screen");
            }, () => false);
        },
        stopShare() {
            if (!state.sharing) return Promise.resolve(false);
            state.sharing = false;
            if (call.screenStream) call.screenStream.getTracks().forEach((track) => track.stop());
            call.screenStream = null;
            if (call.screenTx) {
                call.screenTx.sender.replaceTrack(null);
                call.screenTx.direction = "inactive";
            }
            return call.renegotiate(null);
        },
        // the page's own offer (a track added or stopped); its answer comes as an "answer" frame
        renegotiate(source) {
            const pc = call.pc;
            if (!pc) return Promise.resolve(false);
            call.chain = (call.chain || Promise.resolve()).then(() => {
                call.pendingOffer = true;
                return pc.createOffer().then((offer) => pc.setLocalDescription(offer)).then(() => {
                    if (source && call.screenTx && call.screenTx.mid)
                        call.send({ rtc: "publish", callSessionId: state.current, callLegId: state.legId, mid: call.screenTx.mid, source });
                    call.send({ rtc: "offer", callSessionId: state.current, callLegId: state.legId, sdp: pc.localDescription.sdp, id: call.nextId++ });
                });
            }).catch((err) => { call.pendingOffer = false; console.warn("call renegotiation failed", err); });
            return call.chain.then(() => true);
        },

        toggleHand() {
            state.hand = !state.hand;
            call.send({ rtc: "hand", callSessionId: state.current, callLegId: state.legId, raised: state.hand });
        },
        sendReaction(emoji) {
            call.send({ rtc: "reaction", callSessionId: state.current, callLegId: state.legId, emoji });
            state.myReaction = emoji;
            setTimeout(() => { if (state.myReaction === emoji) state.myReaction = null; }, 3000);
        },

        toggleMute() {
            const stream = state.localStream;
            if (!stream) return;
            state.muted = !state.muted;
            stream.getAudioTracks().forEach((track) => (track.enabled = !state.muted));
            call.sendMute();
        },
        toggleCamera() {
            const stream = state.localStream;
            if (!stream || !stream.getVideoTracks().length) return;
            state.cameraOff = !state.cameraOff;
            stream.getVideoTracks().forEach((track) => (track.enabled = !state.cameraOff));
            call.sendMute();
        },
        sendMute() {
            call.send({ rtc: "mute", callSessionId: state.current, callLegId: state.legId, audioMuted: state.muted, cameraOff: state.cameraOff });
        },

        // Leaves the media (the services hear of it from leave#Call or the socket); the call's state stays in the store.
        hangUp() {
            if (call.unsubscribe) {
                call.unsubscribe();
                call.unsubscribe = null;
            }
            if (call.pc) {
                try { call.pc.close(); } catch (e) { /* closed */ }
                call.pc = null;
            }
            if (state.localStream) state.localStream.getTracks().forEach((track) => track.stop());
            if (call.screenStream) call.screenStream.getTracks().forEach((track) => track.stop());
            Object.assign(state, { current: null, legId: null, phase: "idle", localStream: null, remotes: {}, muted: false, cameraOff: false,
                quality: null, startedAt: 0, speaker: null, sharing: false, hand: false, myReaction: null });
            call.trackSources = {};
            call.tracks = {};
            call.sentLayers = {};
            call.screenStream = null;
            call.screenTx = null;
        },

        saveSettings(changes) {
            Object.assign(state.settings, changes || {});
            writeSettings(state.settings);
        }
    };
    moqui.call = call;

    // ---- components (registered by WebrootVue.qvt.js) ----
    const components = (moqui.qvtComponents = moqui.qvtComponents || {});

    // A video element bound to a stream; muted for the user's own.
    components["m-call-video"] = {
        name: "mCallVideo",
        props: { stream: Object, muted: Boolean, mirror: Boolean },
        watch: { stream() { this.bind(); } },
        mounted() { this.bind(); },
        methods: {
            bind() {
                const el = this.$refs.video;
                if (el && el.srcObject !== this.stream) el.srcObject = this.stream || null;
            }
        },
        template: '<video ref="video" autoplay playsinline :muted="muted" class="m-call-video" :style="mirror ? \'transform:scaleX(-1)\' : \'\'"></video>'
    };

    // An audio element for one remote: it lives in the dock, so the call is heard on every screen.
    components["m-call-audio"] = {
        name: "mCallAudio",
        props: { stream: Object, sinkId: String },
        watch: { stream() { this.bind(); }, sinkId() { this.bind(); } },
        mounted() { this.bind(); },
        methods: {
            bind() {
                const el = this.$refs.audio;
                if (!el) return;
                if (el.srcObject !== this.stream) el.srcObject = this.stream || null;
                if (this.sinkId && el.setSinkId) el.setSinkId(this.sinkId).catch(() => null);
            }
        },
        template: '<audio ref="audio" autoplay></audio>'
    };

    function elapsedText(since) {
        if (!since) return "";
        const seconds = Math.max(0, Math.floor((Date.now() - since) / 1000));
        const m = Math.floor(seconds / 60);
        const s = String(seconds % 60).padStart(2, "0");
        return m >= 60 ? Math.floor(m / 60) + ":" + String(m % 60).padStart(2, "0") + ":" + s : m + ":" + s;
    }

    // The shell's part of calls: the ring bar, the LiveCall window off the call's own screen, and the call's audio.
    components["m-call-dock"] = {
        name: "mCallDock",
        data() {
            return { call: state, now: Date.now(), timer: null };
        },
        computed: {
            ringingCalls() {
                return this.call.ringing.map((id) => this.call.calls[id]).filter((view) => view);
            },
            // calls the user is in that this page has no media for (after a reload, or from another tab)
            elsewhere() {
                return Object.values(this.call.calls).filter((view) => view.status !== "ended" && view.myLegStatus === "answered" &&
                    view.callSessionId !== this.call.current);
            },
            remotes() {
                return Object.values(this.call.remotes);
            },
            onCallScreen() {
                const params = this.$root.currentParameters || {};
                const list = this.$root.currentPathList || [];
                return list[list.length - 1] === "Call" && params.callSessionId === this.call.current;
            },
            current() {
                return this.call.current ? this.call.calls[this.call.current] : null;
            },
            // the LiveCall window shows a screen share, else the speaker, else anyone with video
            speaker() {
                const shared = this.remotes.find((r) => r.screen);
                if (shared) return { stream: shared.screen };
                const speaking = this.remotes.find((r) => r.legId === this.call.speaker && r.stream && r.stream.getVideoTracks().length);
                return speaking || this.remotes.find((r) => r.stream && r.stream.getVideoTracks().length) || null;
            },
            elapsed() {
                return this.now && elapsedText(this.call.startedAt);
            }
        },
        watch: {
            // off the call's screen only the LiveCall window's speaker is shown: the others' video is not received
            onCallScreen: { immediate: true, handler(on) { moqui.call.setOffScreen(!on); } }
        },
        mounted() {
            this.timer = setInterval(() => (this.now = Date.now()), 1000);
        },
        beforeUnmount() {
            clearInterval(this.timer);
        },
        methods: {
            t,
            fromName(view) {
                const host = (view.participants || []).find((p) => p.role === "host");
                return (host && host.displayName) || view.startedByName || t("Someone");
            },
            answer(view, video) { moqui.call.answer(view.callSessionId, video); },
            decline(view) { moqui.call.decline(view.callSessionId); },
            back() { if (this.call.current) moqui.call.open(this.call.current); },
            returnTo(view) { moqui.call.open(view.callSessionId); },
            leaveElsewhere(view) { moqui.call.request("leaveCall", { callSessionId: view.callSessionId }); },
            leave() { moqui.call.leave(); }
        },
        template:
            '<div class="m-call-dock">' +
            '<m-call-audio v-for="r in remotes" :key="\'a\' + r.legId" :stream="r.stream" :sink-id="call.settings.audioOut"></m-call-audio>' +
            '<q-page-sticky v-if="ringingCalls.length" position="top" :offset="[0, 8]" style="z-index:2100">' +
            '<q-card v-for="view in ringingCalls" :key="view.callSessionId" class="m-call-ring q-mb-sm" style="min-width:300px">' +
            '<q-card-section class="row items-center no-wrap q-py-sm">' +
            '<q-icon :name="view.video ? \'sym_o_videocam\' : \'sym_o_call\'" size="28px" class="q-mr-sm"></q-icon>' +
            '<div class="col"><div class="text-subtitle2">{{fromName(view)}}</div><div class="text-caption m-muted">{{view.video ? t(\'Video call\') : t(\'Audio call\')}}</div></div>' +
            '<q-btn round unelevated color="positive" icon="sym_o_call" class="q-ml-sm" :aria-label="t(\'Answer\')" @click="answer(view, view.video)"><q-tooltip>{{t(\'Answer\')}}</q-tooltip></q-btn>' +
            '<q-btn v-if="view.video" round flat color="positive" icon="sym_o_mic" class="q-ml-xs" :aria-label="t(\'Answer with audio only\')" @click="answer(view, false)"><q-tooltip>{{t(\'Answer with audio only\')}}</q-tooltip></q-btn>' +
            '<q-btn round unelevated color="negative" icon="sym_o_call_end" class="q-ml-xs" :aria-label="t(\'Decline\')" @click="decline(view)"><q-tooltip>{{t(\'Decline\')}}</q-tooltip></q-btn>' +
            "</q-card-section></q-card></q-page-sticky>" +
            '<q-page-sticky v-if="elsewhere.length && !ringingCalls.length && !onCallScreen" position="top" :offset="[0, 8]" style="z-index:2050">' +
            '<q-banner v-for="view in elsewhere" :key="\'e\' + view.callSessionId" dense rounded class="m-tonal q-mb-sm">' +
            '<template v-slot:avatar><q-icon name="sym_o_call"></q-icon></template>{{t(\'You are in a call\')}}: {{fromName(view)}}' +
            '<template v-slot:action><q-btn flat no-caps :label="t(\'Return to the call\')" @click="returnTo(view)"></q-btn>' +
            '<q-btn flat no-caps color="negative" :label="t(\'Leave\')" @click="leaveElsewhere(view)"></q-btn></template></q-banner></q-page-sticky>' +
            '<q-page-sticky v-if="call.current && !onCallScreen" position="bottom-right" :offset="[16, 16]" style="z-index:2000">' +
            '<q-card class="m-live-call" style="width:320px">' +
            '<div class="m-live-call-media cursor-pointer" style="height:180px;background:#000" @click="back">' +
            '<m-call-video v-if="speaker" :stream="speaker.stream" muted style="width:100%;height:100%;object-fit:cover"></m-call-video>' +
            '<div v-else class="fit flex flex-center text-white text-subtitle1">{{current ? (current.participants || []).filter(p => p.inCall).map(p => p.displayName).join(\', \') : \'\'}}</div>' +
            "</div>" +
            '<q-card-actions class="q-py-xs"><span class="text-caption q-ml-sm">{{elapsed}}</span><q-space></q-space>' +
            '<q-btn flat round dense :icon="call.muted ? \'sym_o_mic_off\' : \'sym_o_mic\'" :aria-label="t(\'Mute\')" @click="$event.stopPropagation(); moquiCall().toggleMute()"></q-btn>' +
            '<q-btn flat round dense :icon="call.cameraOff ? \'sym_o_videocam_off\' : \'sym_o_videocam\'" :aria-label="t(\'Camera\')" @click="moquiCall().toggleCamera()"></q-btn>' +
            '<q-btn flat round dense color="negative" icon="sym_o_call_end" :aria-label="t(\'Leave\')" @click="leave"></q-btn>' +
            "</q-card-actions></q-card></q-page-sticky>" +
            "</div>"
    };
    components["m-call-dock"].methods.moquiCall = () => moqui.call;

    // The lobby and the call view of one call: what a Call screen shows.
    components["m-call"] = {
        name: "mCall",
        props: { callSessionId: String },
        data() {
            return { call: state, view: null, loading: false, preview: null, devices: [], level: 0, meter: null, video: true, now: Date.now(), timer: null };
        },
        computed: {
            inThis() { return this.call.current === this.callSessionId; },
            remotes() { return Object.values(this.call.remotes); },
            known() { return this.call.calls[this.callSessionId] || this.view; },
            isHost() {
                const me = moqui.call.me;
                const v = this.known;
                return !!v && (v.participants || []).some((p) => p.userId === me && p.role === "host");
            },
            canJoin() { const v = this.known; return !!v && v.status !== "ended"; },
            stateText() {
                const v = this.known;
                if (!v) return "";
                if (v.status === "ended") return t("The call has ended");
                if (v.myLegStatus === "offered") return t("Ringing for you");
                return v.status === "ringing" ? t("Ringing") : t("In progress");
            },
            audioInputs() { return this.devices.filter((d) => d.kind === "audioinput"); },
            videoInputs() { return this.devices.filter((d) => d.kind === "videoinput"); },
            // every tile: me, each other leg's camera, and each screen share
            tiles() {
                const me = this.call.localStream;
                const list = [{ key: "me", name: t("You"), stream: me && me.getVideoTracks().length && !this.call.cameraOff ? me : null, mirror: true,
                    muted: this.call.muted, hand: this.call.hand, reaction: this.call.myReaction, isMe: true }];
                for (const r of this.remotes) {
                    list.push({ key: r.legId, legId: r.legId, name: r.name, stream: r.stream && r.stream.getVideoTracks().length && !r.cameraOff ? r.stream : null,
                        muted: r.muted, hand: r.hand, reaction: r.reaction, audioOnly: r.audioOnly, speaking: this.call.speaker === r.legId });
                    if (r.screen) list.push({ key: r.legId + ":screen", legId: r.legId, name: r.name + " · " + t("screen"), stream: r.screen, screen: true });
                }
                return list;
            },
            mainTile() {
                if (this.call.layout !== "speaker") return null;
                return this.tiles.find((tile) => tile.screen) || this.tiles.find((tile) => tile.speaking) || this.tiles.find((tile) => !tile.isMe) || this.tiles[0];
            },
            stripTiles() { return this.mainTile ? this.tiles.filter((tile) => tile !== this.mainTile) : this.tiles; },
            gridStyle() {
                const n = this.stripTiles.length;
                if (this.mainTile) return "display:grid;gap:8px;grid-template-columns:repeat(" + Math.max(1, Math.min(n, 6)) + ",1fr)";
                const cols = n <= 1 ? 1 : n <= 4 ? 2 : n <= 9 ? 3 : 4;
                return "display:grid;gap:8px;grid-template-columns:repeat(" + cols + ",1fr)";
            },
            canShare() { return !!(root.navigator && root.navigator.mediaDevices && root.navigator.mediaDevices.getDisplayMedia); },
            raisedCount() { return this.remotes.filter((r) => r.hand).length + (this.call.hand ? 1 : 0); },
            reactions() { return ["👍", "👏", "😂", "❤️", "🎉", "🙋"]; },
            elapsed() { return this.now && elapsedText(this.call.startedAt); }
        },
        watch: {
            callSessionId() { this.load(); }
        },
        mounted() {
            this.load();
            this.timer = setInterval(() => (this.now = Date.now()), 1000);
        },
        beforeUnmount() {
            clearInterval(this.timer);
            this.stopPreview();
        },
        methods: {
            t,
            load() {
                if (!this.callSessionId) return;
                this.loading = true;
                moqui.call.request("callState", { callSessionId: this.callSessionId }, "GET").then((json) => {
                    this.view = json.call || null;
                    this.loading = false;
                    if (this.view && !this.inThis && this.view.status !== "ended") this.startPreview();
                }, () => (this.loading = false));
            },
            listDevices() {
                const media = root.navigator && root.navigator.mediaDevices;
                if (media && media.enumerateDevices) media.enumerateDevices().then((list) => (this.devices = list), () => null);
            },
            startPreview() {
                if (this.preview || this.inThis) return;
                moqui.call.getMedia(this.video && !this.call.settings.cameraOff).then((stream) => {
                    if (!stream || this.inThis) { if (stream) stream.getTracks().forEach((tr) => tr.stop()); return; }
                    this.preview = stream;
                    this.listDevices();
                    this.startMeter(stream);
                });
            },
            stopPreview(keep) {
                if (this.meter) { cancelAnimationFrame(this.meter.frame); try { this.meter.ctx.close(); } catch (e) { /* closed */ } this.meter = null; }
                if (this.preview && !keep) this.preview.getTracks().forEach((tr) => tr.stop());
                this.preview = null;
            },
            startMeter(stream) {
                const Ctx = root.AudioContext || root.webkitAudioContext;
                if (!Ctx || !stream.getAudioTracks().length) return;
                const ctx = new Ctx();
                const analyser = ctx.createAnalyser();
                ctx.createMediaStreamSource(stream).connect(analyser);
                const data = new Uint8Array(analyser.fftSize);
                const tick = () => {
                    analyser.getByteTimeDomainData(data);
                    let peak = 0;
                    for (const v of data) peak = Math.max(peak, Math.abs(v - 128));
                    this.level = Math.min(1, peak / 64);
                    this.meter.frame = requestAnimationFrame(tick);
                };
                this.meter = { ctx, frame: requestAnimationFrame(tick) };
            },
            pick(changes) {
                moqui.call.saveSettings(changes);
                this.stopPreview();
                this.startPreview();
            },
            join() {
                const stream = this.preview;
                this.stopPreview(true);
                const v = this.known;
                const start = v && v.myLegStatus === "offered" ? moqui.call.answer(this.callSessionId, this.video)
                    : v && v.myLegStatus === "answered" && v.myLegId ? moqui.call.join(this.callSessionId, v.myLegId, { video: this.video, stream })
                    : moqui.call.joinCall(this.callSessionId, { video: this.video, stream });
                start.then(() => this.load());
            },
            leave() { moqui.call.leave().then(() => this.load()); },
            endForAll() { moqui.call.endForAll().then(() => this.load()); },
            tileStyle(tile) {
                const ring = tile.speaking ? ";box-shadow:0 0 0 3px var(--q-positive)" : "";
                return "position:relative;background:#000;border-radius:8px;overflow:hidden;" + (this.mainTile === tile ? "height:100%" : "aspect-ratio:16/9") + ring;
            },
            handOf(p) {
                if (p.userId === moqui.call.me) return this.call.hand;
                return (p.legs || []).some((leg) => this.call.remotes[leg.callLegId] && this.call.remotes[leg.callLegId].hand);
            }
        },
        template:
            '<div class="m-call column q-gutter-sm">' +
            '<div class="row items-center"><div class="text-h6 q-mr-md">{{known ? (known.participants || []).filter(p => p.userId !== moquiCallMe()).map(p => p.displayName).join(\', \') || t(\'Call\') : t(\'Call\')}}</div>' +
            '<q-chip dense square :color="known && known.status === \'ended\' ? \'grey-6\' : \'positive\'" text-color="white">{{stateText}}</q-chip>' +
            '<span v-if="inThis" class="text-caption q-ml-sm">{{elapsed}}</span><q-space></q-space>' +
            '<span v-if="inThis && call.quality" class="text-caption m-muted">{{call.quality}} kbps</span></div>' +
            '<q-banner v-if="call.error && inThis" dense rounded class="m-tonal text-warning">{{call.error}}</q-banner>' +
            '<div v-if="!known && !loading" class="m-muted">{{t(\'Call not found\')}}</div>' +
            // the lobby
            '<div v-if="known && !inThis && canJoin" class="row q-col-gutter-md">' +
            '<div class="col-12 col-md-7"><div style="background:#000;aspect-ratio:16/9;border-radius:8px;overflow:hidden">' +
            '<m-call-video v-if="preview && preview.getVideoTracks().length" :stream="preview" muted mirror style="width:100%;height:100%;object-fit:cover"></m-call-video>' +
            '<div v-else class="fit flex flex-center text-white">{{preview ? t(\'Camera off\') : t(\'No camera or no permission for it\')}}</div></div>' +
            '<q-linear-progress :value="level" class="q-mt-xs" color="positive" :aria-label="t(\'Microphone level\')"></q-linear-progress></div>' +
            '<div class="col-12 col-md-5 column q-gutter-sm">' +
            '<q-select dense outlined emit-value map-options :label="t(\'Microphone\')" :options="audioInputs.map(d => ({value: d.deviceId, label: d.label || t(\'Microphone\')}))" :model-value="call.settings.audioIn" @update:model-value="v => pick({audioIn: v})"></q-select>' +
            '<q-select dense outlined emit-value map-options :label="t(\'Camera\')" :options="videoInputs.map(d => ({value: d.deviceId, label: d.label || t(\'Camera\')}))" :model-value="call.settings.videoIn" @update:model-value="v => pick({videoIn: v})"></q-select>' +
            '<q-toggle v-model="video" :label="t(\'Join with video\')" @update:model-value="pick({})"></q-toggle>' +
            '<q-btn unelevated color="primary" icon="sym_o_call" :label="known.myLegStatus === \'offered\' ? t(\'Answer\') : t(\'Join\')" @click="join"></q-btn></div></div>' +
            // the call view: the grid, or the speaker (or a screen share) large over a strip of the rest
            '<div v-if="inThis && mainTile" style="height:60vh" class="q-mb-sm">' +
            '<div v-for="tile in [mainTile]" :key="\'main\' + tile.key" class="fit">' + '<div class="m-call-tile" :style="tileStyle(tile)">' +
            '<m-call-video v-if="tile.stream" :stream="tile.stream" muted :mirror="!!tile.mirror" :style="tile.screen ? \'width:100%;height:100%;object-fit:contain\' : \'width:100%;height:100%;object-fit:cover\'"></m-call-video>' +
            '<div v-else class="fit flex flex-center text-white text-subtitle1">{{tile.name}}</div>' +
            '<div v-if="tile.reaction" class="absolute-center text-h3">{{tile.reaction}}</div>' +
            '<q-chip v-if="tile.audioOnly" dense square class="absolute-top-right q-ma-xs" color="grey-9" text-color="white" icon="sym_o_signal_cellular_alt_1_bar">{{t(\'Audio only: low bandwidth\')}}</q-chip>' +
            '<div class="absolute-bottom-left q-pa-xs text-white text-caption">{{tile.name}}<q-icon v-if="tile.muted" name="sym_o_mic_off" class="q-ml-xs"></q-icon>' +
            '<q-icon v-if="tile.hand" name="sym_o_back_hand" color="warning" class="q-ml-xs"></q-icon></div></div>' + '</div></div>' +
            '<div v-if="inThis" :style="gridStyle">' +
            '<div v-for="tile in stripTiles" :key="tile.key">' + '<div class="m-call-tile" :style="tileStyle(tile)">' +
            '<m-call-video v-if="tile.stream" :stream="tile.stream" muted :mirror="!!tile.mirror" :style="tile.screen ? \'width:100%;height:100%;object-fit:contain\' : \'width:100%;height:100%;object-fit:cover\'"></m-call-video>' +
            '<div v-else class="fit flex flex-center text-white text-subtitle1">{{tile.name}}</div>' +
            '<div v-if="tile.reaction" class="absolute-center text-h3">{{tile.reaction}}</div>' +
            '<q-chip v-if="tile.audioOnly" dense square class="absolute-top-right q-ma-xs" color="grey-9" text-color="white" icon="sym_o_signal_cellular_alt_1_bar">{{t(\'Audio only: low bandwidth\')}}</q-chip>' +
            '<div class="absolute-bottom-left q-pa-xs text-white text-caption">{{tile.name}}<q-icon v-if="tile.muted" name="sym_o_mic_off" class="q-ml-xs"></q-icon>' +
            '<q-icon v-if="tile.hand" name="sym_o_back_hand" color="warning" class="q-ml-xs"></q-icon></div></div>' + '</div></div>' +
            '<div v-if="inThis && !remotes.length" class="m-muted">{{call.phase === \'connected\' ? t(\'Waiting for the others\') : t(\'Connecting\')}}</div>' +
            // the bottom bar
            '<div v-if="inThis" class="row justify-center q-gutter-sm q-mt-sm">' +
            '<q-btn round unelevated :color="call.muted ? \'negative\' : \'grey-8\'" :icon="call.muted ? \'sym_o_mic_off\' : \'sym_o_mic\'" :aria-label="t(\'Mute\')" @click="moquiCall().toggleMute()"><q-tooltip>{{call.muted ? t(\'Unmute\') : t(\'Mute\')}}</q-tooltip></q-btn>' +
            '<q-btn round unelevated :color="call.cameraOff ? \'negative\' : \'grey-8\'" :icon="call.cameraOff ? \'sym_o_videocam_off\' : \'sym_o_videocam\'" :aria-label="t(\'Camera\')" :disable="!call.localStream || !call.localStream.getVideoTracks().length" @click="moquiCall().toggleCamera()"><q-tooltip>{{t(\'Camera\')}}</q-tooltip></q-btn>' +
            '<q-btn v-if="canShare" round unelevated :color="call.sharing ? \'primary\' : \'grey-8\'" :icon="call.sharing ? \'sym_o_stop_screen_share\' : \'sym_o_present_to_all\'" :aria-label="call.sharing ? t(\'Stop sharing\') : t(\'Share the screen\')" @click="call.sharing ? moquiCall().stopShare() : moquiCall().shareScreen()"><q-tooltip>{{call.sharing ? t(\'Stop sharing\') : t(\'Share the screen\')}}</q-tooltip></q-btn>' +
            '<q-btn round unelevated :color="call.hand ? \'warning\' : \'grey-8\'" icon="sym_o_back_hand" :aria-label="t(\'Raise your hand\')" @click="moquiCall().toggleHand()"><q-tooltip>{{call.hand ? t(\'Lower your hand\') : t(\'Raise your hand\')}}</q-tooltip></q-btn>' +
            '<q-btn round unelevated color="grey-8" icon="sym_o_add_reaction" :aria-label="t(\'React\')"><q-menu><div class="row q-pa-xs">' +
            '<q-btn v-for="emoji in reactions" :key="emoji" flat dense size="lg" v-close-popup @click="moquiCall().sendReaction(emoji)">{{emoji}}</q-btn></div></q-menu></q-btn>' +
            '<q-btn round unelevated color="grey-8" :icon="call.layout === \'speaker\' ? \'sym_o_grid_view\' : \'sym_o_person\'" :aria-label="call.layout === \'speaker\' ? t(\'Grid view\') : t(\'Speaker view\')" @click="moquiCall().setLayout(call.layout === \'speaker\' ? \'grid\' : \'speaker\')"><q-tooltip>{{call.layout === \'speaker\' ? t(\'Grid view\') : t(\'Speaker view\')}}</q-tooltip></q-btn>' +
            '<q-btn round unelevated color="grey-8" icon="sym_o_group" :aria-label="t(\'Participants\')" @click="call.participantsOpen = !call.participantsOpen"><q-badge v-if="raisedCount" color="warning" floating>{{raisedCount}}</q-badge><q-tooltip>{{t(\'Participants\')}}</q-tooltip></q-btn>' +
            '<q-btn round unelevated color="negative" icon="sym_o_call_end" :aria-label="t(\'Leave\')" @click="leave"><q-tooltip>{{t(\'Leave\')}}</q-tooltip></q-btn>' +
            '<q-btn v-if="isHost && known && known.kind !== \'direct\'" flat no-caps color="negative" :label="t(\'End for everyone\')" @click="endForAll"></q-btn>' +
            '<m-ext-actions v-if="known" slot="call-actions" :item="{callSessionId: callSessionId}"></m-ext-actions></div>' +
            // the participants: who is in it, who is ringing, raised hands
            '<q-list v-if="inThis && call.participantsOpen && known" bordered separator dense class="rounded-borders q-mt-sm">' +
            '<q-item v-for="p in known.participants || []" :key="p.callParticipantId"><q-item-section avatar><m-avatar :name="p.displayName" :user-id="p.userId || undefined" size="28px"></m-avatar></q-item-section>' +
            '<q-item-section><q-item-label>{{p.displayName}}<span v-if="p.role === \'host\'" class="text-caption m-muted"> · {{t(\'host\')}}</span></q-item-label>' +
            '<q-item-label caption>{{p.inCall ? t(\'In the call\') : p.ringing ? t(\'Ringing\') : t(\'Not in the call\')}}</q-item-label></q-item-section>' +
            '<q-item-section side><q-icon v-if="handOf(p)" name="sym_o_back_hand" color="warning"></q-icon></q-item-section></q-item></q-list>' +
            "</div>"
    };
    components["m-call"].methods.moquiCall = () => moqui.call;
    components["m-call"].methods.moquiCallMe = () => moqui.call.me;
})(globalThis);
