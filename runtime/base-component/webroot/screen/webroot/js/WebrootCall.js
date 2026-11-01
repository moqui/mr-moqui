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
 * An end-to-end encrypted call (W8, view.e2ee): before its media this page takes its crypto device (moqui.callKeys,
 * WebrootCallKeys.js), names it on its leg (register#CallDevice, or with the answer or join), and SFrame-encrypts what it
 * sends and decrypts what it receives in an RTCRtpScriptTransform worker; a browser that cannot is refused, with a notice,
 * and never joins without encryption.
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
        // a guest waiting in the lobby of a call link's meeting: {callSessionId, guestName}, for the dock
        lobbyNotice: null,
        settings: readSettings(),
        capabilities: null,
        // live captions of the current call (W6): whether the CC button shows (allowed), whether they are on, who
        // transcribes (the notice), why they are unavailable, and the last lines {key, leg, name, text, gap, at}
        captions: { allowed: false, on: false, provider: "", local: false, unavailable: null, lines: [] },
        // recording (W5), by callSessionId: {enabled, ackRequired, canRecord, recording: {callRecordingId, state, pauseReason,
        // kind, ackRequired} or null, myConsent: {acknowledged, withdrawn, ackRequired} or null, failed}
        recordingOf: {},
        // V5: the call-context providers' answer for a phone call, by callSessionId (a screen pop: who it is, open records)
        contextOf: {},
        // W8: why this browser cannot take part in an end-to-end encrypted call: {callSessionId, reasonCode, message}
        e2eeNotice: null
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
        request(name, params, method, quiet) {
            return moqui.request(call.base() + "/" + name, { method: method || "POST", params: params || {} }).then((result) => {
                const json = result.json || {};
                if (json.errors && !quiet) {
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
            // W8: an encrypted call's roster, for its keys
            if (moqui.callKeys && moqui.callKeys.state.active && moqui.callKeys.state.callSessionId === view.callSessionId) moqui.callKeys.onView(view);
            const ringingForMe = view.status !== "ended" && view.myLegStatus === "offered";
            const at = state.ringing.indexOf(view.callSessionId);
            if (ringingForMe && at < 0) state.ringing.push(view.callSessionId);
            // V5: who a phone caller is, once a call, when it rings for me or I am in it
            if (view.kind === "phone" && view.status !== "ended" && !(view.callSessionId in state.contextOf)) call.loadContext(view.callSessionId);
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
            // V5: a merge moved this page's leg into another call: its media joins that call's room with the same leg
            if (data.action === "moved" && data.toCallSessionId && state.current === data.callSessionId) call.moveTo(data.toCallSessionId);
            if (data.action === "ended" && state.current === data.callSessionId) call.hangUp("ended");
            if (data.action === "captions" && data.callSessionId === state.current) call.loadCaptions(data.callSessionId);
            if (data.action === "guestWaiting") state.lobbyNotice = { callSessionId: data.callSessionId, guestName: data.guestName || "" };
            if (data.action === "admitted" || data.action === "denied" || data.action === "ended") {
                const view = state.calls[data.callSessionId];
                const still = view && (view.participants || []).some((p) => p.admission === "waiting");
                if (!still && state.lobbyNotice && state.lobbyNotice.callSessionId === data.callSessionId) state.lobbyNotice = null;
            }
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
        // ---- W8: a browser in an end-to-end encrypted call: its device, or the notice why it cannot take part ----
        e2eeRefused(callSessionId, err) {
            const notice = { callSessionId: callSessionId || null, reasonCode: (err && err.reasonCode) || "unsupportedClient",
                message: (err && (err.message || err.errors)) || t("This browser cannot take part in an end-to-end encrypted call") };
            state.e2eeNotice = notice;
            if (moqui.notifyMessages) moqui.notifyMessages([], notice.message);
            return { reasonCode: notice.reasonCode, errors: notice.message };
        },
        // a request that an encrypted call takes only with the browser's device: with it when the call is known to be one,
        // and again with it when the server says so (a call this page did not know yet)
        requestWithDevice(name, params, encrypted, callSessionId) {
            const withDevice = () => moqui.callKeys ? moqui.callKeys.prepare().then((device) => call.request(name, Object.assign({}, params, device)))
                : Promise.reject({ reasonCode: "unsupportedClient" });
            if (encrypted) return withDevice().catch((err) => call.e2eeRefused(callSessionId, err));
            return call.request(name, params, "POST", true).then((json) => {
                if (json.reasonCode === "unsupportedClient" || json.reasonCode === "noCryptoDevice") {
                    return withDevice().catch((err) => call.e2eeRefused(callSessionId, err));
                }
                if (json.errors && moqui.notifyMessages) moqui.notifyMessages([], json.errors);
                return json;
            });
        },
        startCall(params) {
            const encrypted = !!(params && params.requireE2ee);
            return call.requestWithDevice("startCall", Object.assign({ idempotencyKey: "c" + Date.now() + Math.random().toString(36).slice(2) }, params,
                encrypted ? { requireE2ee: "true" } : {}), encrypted, null).then((json) => {
                if (json.callSessionId && json.callLegId) {
                    call.open(json.callSessionId);
                    return call.join(json.callSessionId, json.callLegId, { video: params.video !== false }).then(() => json);
                }
                if (json.reasonCode === "callActive" && json.callSessionId) call.open(json.callSessionId);
                return json;
            });
        },
        answer(callSessionId, video) {
            const view = state.calls[callSessionId];
            return call.requestWithDevice("answerCall", { callSessionId }, !!(view && view.e2ee), callSessionId).then((json) => {
                if (!json.callLegId) return json;
                call.open(callSessionId);
                return call.join(callSessionId, json.callLegId, { video: video !== false }).then(() => json);
            });
        },
        decline(callSessionId) {
            return call.request("declineCall", { callSessionId });
        },
        joinCall(callSessionId, options) {
            const view = state.calls[callSessionId];
            return call.requestWithDevice("joinCall", { callSessionId }, !!(view && view.e2ee), callSessionId).then((json) => {
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

        // ---- V5: call-context, a merge's move, and the call actions of m-call-actions ----
        loadContext(callSessionId) {
            if (call.guest || !callSessionId) return Promise.resolve(null);
            state.contextOf[callSessionId] = null;
            return call.request("callContext", { callSessionId }, "GET").then((json) => {
                state.contextOf[callSessionId] = json && !json.reasonCode ? json : null;
                return json;
            }, () => null);
        },
        moveTo(toCallSessionId) {
            if (!state.current) return Promise.resolve();
            // the call screen of the old call shows the new one
            const vm = call.vm || {};
            const list = vm.currentPathList || [];
            const onScreen = list[list.length - 1] === "Call" && (vm.currentParameters || {}).callSessionId === state.current;
            // W8: the keys of the old call are not the new one's (connectWith takes no media of an encrypted call without them)
            if (moqui.callKeys && moqui.callKeys.state.active && moqui.callKeys.state.callSessionId !== toCallSessionId) moqui.callKeys.end();
            state.current = toCallSessionId;
            state.remotes = {};
            call.trackSources = {};
            call.tracks = {};
            state.phase = "joining";
            if (onScreen) call.open(toCallSessionId);
            return call.connect(state.localStream).catch((err) => (state.error = String((err && err.message) || err)));
        },
        callAction(name, params) {
            return call.request(name, params).then((json) => {
                if (json.call) call.remember(json.call);
                return json;
            });
        },

        // ---- recording (W5): the policy, the call's recording and this leg's notice; a guest asks with its credential ----
        recordingRequest(name, params, method) {
            if (call.guest && call.guest.recording) return Promise.resolve(call.guest.recording(name));
            return call.request(name, params, method);
        },
        noteRecording(callSessionId, json) {
            if (!callSessionId || !json) return;
            const was = state.recordingOf[callSessionId] || {};
            state.recordingOf[callSessionId] = Object.assign({}, was, {
                enabled: json.enabled != null ? !!json.enabled : !!was.enabled,
                ackRequired: json.ackRequired != null ? !!json.ackRequired : !!was.ackRequired,
                canRecord: json.canRecord != null ? !!json.canRecord : !!was.canRecord,
                recording: json.recording !== undefined ? json.recording : was.recording || null,
                myConsent: json.myConsent !== undefined ? json.myConsent : was.myConsent || null
            });
        },
        loadRecording(callSessionId) {
            if (!callSessionId) return Promise.resolve({});
            return call.recordingRequest("recordingState", { callSessionId }, "GET").then((json) => {
                if (json && !json.reasonCode) call.noteRecording(callSessionId, json);
                return json || {};
            }, () => ({}));
        },
        // the host's startRecording, stopRecording, pauseRecording, resumeRecording; anyone's ackRecording or
        // withdrawRecording; then the state again (this leg's notice changed)
        recordingAction(callSessionId, name) {
            const rec = state.recordingOf[callSessionId] || {};
            const params = { callSessionId, callRecordingId: rec.recording ? rec.recording.callRecordingId : null };
            return call.recordingRequest(name, params).then(() => call.loadRecording(callSessionId));
        },
        // the media node's recording frame, to every leg and again to one that joins late
        recordingFrame(frame) {
            const id = frame.callSessionId;
            const going = frame.state === "recording" || frame.state === "paused";
            const was = state.recordingOf[id] || {};
            const known = was.recording && was.recording.callRecordingId === frame.callRecordingId;
            call.noteRecording(id, { recording: going ? { callRecordingId: frame.callRecordingId, state: frame.state, pauseReason: frame.pauseReason || null,
                kind: frame.kind || null, ackRequired: !!frame.ackRequired } : null });
            state.recordingOf[id].failed = frame.state === "failed";
            // a new recording: this leg's notice is new too
            if (going && !known) call.loadRecording(id);
        },

        // ---- the media ----
        send(frame) {
            if (call.guest) {
                call.guest.send(frame);
                return;
            }
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
            return call.prepareKeys(callSessionId, legId).then(() => {
                const media = keep ? Promise.resolve(keep) : call.getMedia(video);
                return media.then((stream) => {
                    if (state.current !== callSessionId) return;
                    if (!stream) state.error = t("No microphone or camera: you can hear and see the others, they cannot hear or see you.");
                    state.localStream = stream;
                    if (stream && (state.settings.joinMuted || state.muted)) stream.getAudioTracks().forEach((track) => (track.enabled = false));
                    state.muted = !!(stream && stream.getAudioTracks().some((track) => !track.enabled));
                    return call.connect(stream);
                });
            }, (err) => {
                // W8: this browser cannot take part in the encrypted call: no media at all, never plain media
                if (keep) keep.getTracks().forEach((track) => track.stop());
                call.hangUp("refused");
                call.e2eeRefused(callSessionId, err);
            }).catch((err) => {
                state.error = String((err && err.message) || err);
                state.phase = "idle";
            });
        },
        // W8: an encrypted call's keys before its media: this browser's device, named on the leg when it is not yet, and the
        // key protocol started with the call's roster; nothing for another call
        prepareKeys(callSessionId, legId) {
            // a guest's call is never encrypted (the server takes no guest in one); a call whose state cannot be read is not
            // joined, as it might be an encrypted one this page would otherwise send plain media to
            if (call.guest) return Promise.resolve(null);
            const known = state.calls[callSessionId];
            const viewOf = known ? Promise.resolve(known) : call.request("callState", { callSessionId }, "GET", true)
                .then((json) => json.call || null, () => { throw { reasonCode: "failed", message: t("The call could not be read: try again") }; });
            return viewOf.then((view) => {
                if (!view || !view.e2ee) {
                    if (moqui.callKeys && moqui.callKeys.state.active) moqui.callKeys.end();
                    return null;
                }
                if (!moqui.callKeys) throw { reasonCode: "unsupportedClient" };
                return moqui.callKeys.prepare().then((device) => {
                    const mine = (view.participants || []).flatMap((p) => p.legs || []).find((leg) => leg.callLegId === legId);
                    const named = mine && mine.deviceId === device.cryptoDeviceId ? Promise.resolve({ call: view })
                        : call.request("registerCallDevice", Object.assign({ callSessionId, callLegId: legId }, device), "POST", true);
                    return named.then((json) => {
                        if (json.reasonCode) throw { reasonCode: json.reasonCode, message: json.errors };
                        if (state.current !== callSessionId) return null;
                        state.e2eeNotice = null;
                        moqui.callKeys.begin(callSessionId, legId);
                        moqui.callKeys.onView(state.calls[callSessionId] || json.call);
                        return null;
                    });
                });
            });
        },

        // the ICE servers of this page's leg (the embedded TURN, or coturn), with a short-lived credential; none is no TURN
        fetchIceServers(callSessionId) {
            const asked = call.guest && call.guest.iceServers ? call.guest.iceServers()
                : call.request("iceServers", { callSessionId }, "GET");
            return Promise.resolve(asked).then((json) => (json && Array.isArray(json.iceServers) ? json.iceServers : []), () => []);
        },
        connect(stream) {
            return call.fetchIceServers(state.current).then((iceServers) => call.connectWith(stream, iceServers));
        },
        connectWith(stream, iceServers) {
            const id = state.current;
            const legId = state.legId;
            if (!id) return Promise.resolve();
            // W8: an end-to-end encrypted call's media only through its keys: without them (a call this page moved to, keys
            // ended) no media at all, never plain
            const known = state.calls[id];
            if (known && known.e2ee && !call.keysOn(id)) {
                if (call.pc) {
                    try { call.pc.close(); } catch (e) { /* closed */ }
                    call.pc = null;
                }
                state.phase = "idle";
                return Promise.reject(new Error(t("This call is end-to-end encrypted, and this page has no keys for it: join it again")));
            }
            if (call.unsubscribe) call.unsubscribe();
            const client = call.vm && call.vm.notificationClient;
            if (call.guest) call.unsubscribe = null;
            else if (client) call.unsubscribe = client.registerListener("rtc:" + id, call.onFrame);
            if (call.pc) {
                try { call.pc.close(); } catch (e) { /* closed */ }
            }
            const pc = new root.RTCPeerConnection({ bundlePolicy: "max-bundle", iceServers: iceServers || [] });
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
            // W8: what an encrypted call's page sends goes through its SFrame worker
            if (call.keysOn(id)) {
                if (audioTx) moqui.callKeys.attachSender(audioTx.sender, "audio");
                if (videoTx) moqui.callKeys.attachSender(videoTx.sender, "video");
            }
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

        keysOn(callSessionId) {
            return !!(moqui.callKeys && moqui.callKeys.state.active && moqui.callKeys.state.callSessionId === callSessionId);
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
                case "caption":
                case "captions":
                    call.onCaption(frame);
                    break;
                case "recording":
                    call.recordingFrame(frame);
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
            // W8: what an encrypted call's page receives is decrypted with the keys of the leg it comes from
            if (call.keysOn(state.current) && event.receiver) moqui.callKeys.attachReceiver(event.receiver, event.track.kind, legId);
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
                    if (call.keysOn(state.current)) moqui.callKeys.attachSender(call.screenTx.sender, "video");
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
            if (state.current) delete state.recordingOf[state.current];
            if (moqui.callKeys && moqui.callKeys.state.active) moqui.callKeys.end();
            if (call.guest) {
                call.guest.close();
                call.guest = null;
            }
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
            state.captions = { allowed: false, on: false, provider: "", local: false, unavailable: null, lines: [] };
            call.trackSources = {};
            call.tracks = {};
            call.sentLayers = {};
            call.screenStream = null;
            call.screenTx = null;
        },

        // ---- a guest of a call link (the /call/<token> page): a socket of its own, the credential in its first frame ----
        joinAsGuest(options) {
            if (call.guest) call.guest.close();
            const guest = call.openGuestSocket(options.credential);
            guest.iceServers = options.iceServers || null;
            guest.recording = options.recording || null;
            call.guest = guest;
            return guest.ready.then(() => call.join(options.callSessionId, options.callLegId, { video: options.video, stream: options.stream }));
        },
        openGuestSocket(credential) {
            const scheme = root.location.protocol === "https:" ? "wss://" : "ws://";
            const guest = { socket: null, queue: [], open: false, closed: false, tries: 0 };
            let readyResolve = null;
            let readyReject = null;
            guest.ready = new Promise((resolve, reject) => { readyResolve = resolve; readyReject = reject; });
            const connect = () => {
                const socket = new root.WebSocket(scheme + root.location.host + "/callws");
                guest.socket = socket;
                socket.onopen = () => socket.send(JSON.stringify({ rtc: "guestAuth", credential }));
                socket.onmessage = (event) => {
                    let frame = null;
                    try { frame = JSON.parse(event.data); } catch (err) { return; }
                    if (!frame || !frame.rtc) return;
                    if (frame.rtc === "guestAuthed") {
                        const reconnect = guest.open === false && guest.tries > 0;
                        guest.open = true;
                        guest.queue.splice(0).forEach((text) => socket.send(text));
                        readyResolve();
                        if (reconnect && state.current) call.rejoin();
                        return;
                    }
                    if (frame.rtc === "error" && !frame.callSessionId) {
                        state.error = t("This link no longer works.");
                        readyReject(new Error(frame.code || "refused"));
                        guest.closed = true;
                        return;
                    }
                    call.onFrame(frame);
                };
                socket.onclose = () => {
                    guest.open = false;
                    // a dropped socket comes back while the page is in the call, a few times
                    if (!guest.closed && state.current && guest.tries < 5) {
                        guest.tries++;
                        state.phase = "reconnecting";
                        setTimeout(connect, 1000 * guest.tries);
                    }
                };
            };
            guest.send = (frame) => {
                const text = JSON.stringify(frame);
                if (guest.open && guest.socket.readyState === 1) guest.socket.send(text);
                else guest.queue.push(text);
            };
            guest.close = () => {
                guest.closed = true;
                try { if (guest.socket) guest.socket.close(); } catch (err) { /* closed */ }
            };
            connect();
            return guest;
        },

        // ---- live captions (W6): the media node transcribes; anyone in the call turns them on or off ----
        loadCaptions(callSessionId) {
            if (call.guest || !callSessionId) return Promise.resolve(null);
            return call.request("captionState", { callSessionId }, "GET").then((json) => {
                if (state.current !== callSessionId) return json;
                const c = json.captions || {};
                Object.assign(state.captions, { allowed: !!c.allowed, on: !!c.on, provider: c.providerLabel || state.captions.provider, local: !!c.local });
                if (!c.on) Object.assign(state.captions, { unavailable: null, lines: [] });
                return json;
            }, () => null);
        },
        toggleCaptions() {
            const id = state.current;
            if (!id) return Promise.resolve({});
            return call.request(state.captions.on ? "stopCaptions" : "startCaptions", { callSessionId: id }).then((json) => {
                if (json.captions && state.current === id) {
                    state.captions.on = !!json.captions.on;
                    if (json.captions.providerLabel) state.captions.provider = json.captions.providerLabel;
                    if (!json.captions.on) Object.assign(state.captions, { unavailable: null, lines: [] });
                }
                return json;
            });
        },
        // A caption, a gap where one was dropped, or the captions' state: on (the notice), off, unavailable
        onCaption(frame) {
            const c = state.captions;
            if (frame.rtc === "captions") {
                if (frame.state === "on") Object.assign(c, { on: true, unavailable: null, provider: frame.provider || c.provider, local: frame.local != null ? !!frame.local : c.local });
                else if (frame.state === "off") Object.assign(c, { on: false, unavailable: null, lines: [] });
                else if (frame.state === "unavailable") c.unavailable = frame.message || frame.reason || t("Captions are unavailable");
                return;
            }
            c.on = true;
            if (!frame.gap) c.unavailable = null;
            const name = frame.leg === state.legId ? t("You") : frame.name || call.nameOf(frame.leg);
            c.lines.push({ key: frame.leg + ":" + frame.seq, leg: frame.leg, name, text: frame.gap ? "…" : String(frame.text || ""), gap: !!frame.gap, at: Date.now() });
            if (c.lines.length > 4) c.lines.splice(0, c.lines.length - 4);
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

    // The recording indicator's text (W5): recording, or why it is paused.
    function recordingText(recording) {
        if (!recording) return "";
        if (recording.state === "recording") return t("Recording");
        if (recording.pauseReason === "ack") return t("Recording paused: waiting for everyone to acknowledge");
        if (recording.pauseReason === "withdrawn") return t("Recording paused: someone withdrew consent");
        return t("Recording paused");
    }
    moqui.callRecordingText = recordingText;

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
            openLobby() { const id = this.call.lobbyNotice && this.call.lobbyNotice.callSessionId; this.call.lobbyNotice = null; if (id) moqui.call.open(id); },
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
            '<div class="col"><div class="text-subtitle2">{{fromName(view)}}</div><div class="text-caption m-muted">{{view.video ? t(\'Video call\') : t(\'Audio call\')}}<span v-if="view.e2ee"> · <q-icon name="sym_o_lock" size="14px"></q-icon> {{t(\'End-to-end encrypted\')}}</span></div>' +
            '<m-call-context :call-session-id="view.callSessionId" dense></m-call-context></div>' +
            '<q-btn round unelevated color="positive" icon="sym_o_call" class="q-ml-sm" :aria-label="t(\'Answer\')" @click="answer(view, view.video)"><q-tooltip>{{t(\'Answer\')}}</q-tooltip></q-btn>' +
            '<q-btn v-if="view.video" round flat color="positive" icon="sym_o_mic" class="q-ml-xs" :aria-label="t(\'Answer with audio only\')" @click="answer(view, false)"><q-tooltip>{{t(\'Answer with audio only\')}}</q-tooltip></q-btn>' +
            '<q-btn round unelevated color="negative" icon="sym_o_call_end" class="q-ml-xs" :aria-label="t(\'Decline\')" @click="decline(view)"><q-tooltip>{{t(\'Decline\')}}</q-tooltip></q-btn>' +
            "</q-card-section></q-card></q-page-sticky>" +
            '<q-page-sticky v-if="elsewhere.length && !ringingCalls.length && !onCallScreen" position="top" :offset="[0, 8]" style="z-index:2050">' +
            '<q-banner v-for="view in elsewhere" :key="\'e\' + view.callSessionId" dense rounded class="m-tonal q-mb-sm">' +
            '<template v-slot:avatar><q-icon name="sym_o_call"></q-icon></template>{{t(\'You are in a call\')}}: {{fromName(view)}}' +
            '<template v-slot:action><q-btn flat no-caps :label="t(\'Return to the call\')" @click="returnTo(view)"></q-btn>' +
            '<q-btn flat no-caps color="negative" :label="t(\'Leave\')" @click="leaveElsewhere(view)"></q-btn></template></q-banner></q-page-sticky>' +
            '<q-page-sticky v-if="call.lobbyNotice && !onCallScreen && !ringingCalls.length" position="top" :offset="[0, 8]" style="z-index:2060">' +
            '<q-banner dense rounded class="m-tonal"><template v-slot:avatar><q-icon name="sym_o_person_add"></q-icon></template>' +
            '{{call.lobbyNotice.guestName}} {{t(\'is waiting to join your call\')}}' +
            '<template v-slot:action><q-btn flat no-caps :label="t(\'Open the call\')" @click="openLobby"></q-btn>' +
            '<q-btn flat round dense icon="sym_o_close" :aria-label="t(\'Close\')" @click="call.lobbyNotice = null"></q-btn></template></q-banner></q-page-sticky>' +
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
                        muted: r.muted, hand: r.hand, reaction: r.reaction, audioOnly: r.audioOnly && !this.phoneLegs.has(r.legId),
                        phone: this.phoneLegs.has(r.legId), speaking: this.call.speaker === r.legId });
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
            // guests in the lobby of a call link's meeting, for those in the call to let in
            waiting() { const v = this.known; return v ? (v.participants || []).filter((p) => p.admission === "waiting") : []; },
            reactions() { return ["👍", "👏", "😂", "❤️", "🎉", "🙋"]; },
            // live captions (W6): the last lines of the last ten seconds, and the notice of who hears the audio
            captionLines() { return this.call.captions.lines.filter((line) => this.now - line.at < 10000); },
            captionNotice() { return captionNotice(this.call.captions); },
            elapsed() { return this.now && elapsedText(this.call.startedAt); },
            // recording (W5): the policy and the call's recording, this leg's notice
            rec() { return this.call.recordingOf[this.callSessionId] || null; },
            recording() { return this.rec ? this.rec.recording : null; },
            recordingText() { return recordingText(this.recording); },
            canRecord() { return !!(this.rec && this.rec.enabled && this.rec.canRecord && this.inThis); },
            needsAck() { const r = this.rec; return !!(r && r.recording && r.recording.ackRequired && !(r.myConsent && r.myConsent.acknowledged)); },
            withdrawn() { const r = this.rec; return !!(r && r.recording && r.myConsent && r.myConsent.withdrawn); },
            // a phone call (V1): its phone legs are audio only, and the keypad sends tones to them
            phoneLegs() {
                const v = this.known;
                return new Set(v ? (v.participants || []).filter((p) => p.identity === "phone").flatMap((p) => (p.legs || []).map((leg) => leg.callLegId)) : []);
            },
            hasPhone() { return this.phoneLegs.size > 0; },
            keys() { return ["1", "2", "3", "4", "5", "6", "7", "8", "9", "*", "0", "#"]; },
            // W8: the call's media encryption, apart from the room's message encryption
            e2ee() { return !!(this.known && this.known.e2ee); },
            encryptionText() { return this.e2ee ? t("End-to-end encrypted") : t("Encrypted to the server"); },
            encryptionTip() {
                return this.e2ee ? t("Only the people in the call can hear and see it: each one's media is encrypted in their browser with keys the server never has. No recording, captions, phones or guests.")
                    : t("Encrypted between each browser and the server, which forwards it (and can record or caption it). The encryption of the room's messages is separate.");
            },
            // a browser that cannot encrypt call media cannot join an encrypted call
            e2eeUnsupported() { return this.e2ee && !(moqui.callKeys && moqui.callKeys.supported()); },
            e2eeNotice() { const n = this.call.e2eeNotice; return n && (!n.callSessionId || n.callSessionId === this.callSessionId) ? n.message : ""; },
            callKeys() { return moqui.callKeys ? moqui.callKeys.state : null; }
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
                    if (this.view && this.view.status !== "ended") moqui.call.loadRecording(this.callSessionId);
                    if (this.view && this.view.kind === "phone") this.video = false;
                    if (this.view && !this.inThis && this.view.status !== "ended") this.startPreview();
                    if (this.view && this.inThis) moqui.call.loadCaptions(this.callSessionId);
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
            recAction(name) { moqui.call.recordingAction(this.callSessionId, name); },
            dtmf(key) { moqui.call.request("sendDtmf", { callSessionId: this.callSessionId, digits: key }); },
            endForAll() { moqui.call.endForAll().then(() => this.load()); },
            tileStyle(tile) {
                const ring = tile.speaking ? ";box-shadow:0 0 0 3px var(--q-positive)" : "";
                return "position:relative;background:#000;border-radius:8px;overflow:hidden;" + (this.mainTile === tile ? "height:100%" : "aspect-ratio:16/9") + ring;
            },
            admit(p, yes) {
                moqui.call.request(yes ? "admitGuest" : "denyGuest", { callSessionId: this.callSessionId, callParticipantId: p.callParticipantId })
                    .then(() => this.load());
            },
            remove(p) {
                moqui.call.request("removeParticipant", { callSessionId: this.callSessionId, callParticipantId: p.callParticipantId })
                    .then(() => this.load());
            },
            // W8: a participant's key state on the roster: this page's own key, or whether their keys came and decrypt
            keyText(p) {
                if (!this.e2ee || !p.inCall || !this.inThis || !moqui.callKeys) return "";
                if (p.userId === moqui.call.me) return this.callKeys.kid != null ? t("your key") + " " + (this.callKeys.epoch || 0) : t("making your key");
                const leg = (p.legs || []).find((l) => l.status === "answered" && l.type === "web");
                if (!leg) return "";
                if (!leg.deviceId) return t("no encryption device");
                const st = moqui.callKeys.legState(leg.callLegId);
                const k = this.callKeys.keys[leg.callLegId];
                const verified = k && k.verified ? t("verified device") : t("unverified device");
                return st === "keys" ? t("keys received") + " · " + verified : st === "failing" ? t("frames do not decrypt") : t("waiting for keys");
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
            '<q-chip v-if="recording" dense square :color="recording.state === \'recording\' ? \'negative\' : \'warning\'" text-color="white" icon="sym_o_fiber_manual_record" class="m-call-recording">{{recordingText}}</q-chip>' +
            '<q-chip v-if="known && known.encryptionMode" dense square :class="e2ee ? \'m-call-encryption m-call-e2ee m-tonal text-positive\' : \'m-call-encryption m-tonal\'" :icon="e2ee ? \'sym_o_lock\' : \'sym_o_shield\'">{{encryptionText}}<q-tooltip max-width="320px">{{encryptionTip}}</q-tooltip></q-chip>' +
            '<span v-if="inThis" class="text-caption q-ml-sm">{{elapsed}}</span><q-space></q-space>' +
            '<span v-if="inThis && call.quality" class="text-caption m-muted">{{call.quality}} kbps</span></div>' +
            '<q-banner v-if="call.error && inThis" dense rounded class="m-tonal text-warning">{{call.error}}</q-banner>' +
            '<q-banner v-if="e2eeNotice || (e2eeUnsupported && !inThis && canJoin)" dense rounded class="m-tonal text-negative m-call-e2ee-notice"><template v-slot:avatar><q-icon name="sym_o_lock"></q-icon></template>' +
            '{{e2eeNotice || t(\'This call is end-to-end encrypted, and this browser cannot encrypt call media (it has no RTCRtpScriptTransform). Join it from a current Chrome, Edge, Firefox or Safari.\')}}</q-banner>' +
            '<q-banner v-if="inThis && e2ee && callKeys && callKeys.error" dense rounded class="m-tonal text-warning">{{callKeys.error}}</q-banner>' +
            // C7 hardening: the roster has a device of a participant this browser has not verified (WebrootCallKeys.js unverified)
            '<q-banner v-if="inThis && e2ee && callKeys && callKeys.unverified && callKeys.unverified.length" dense rounded class="m-tonal text-warning m-call-unverified"><template v-slot:avatar><q-icon name="sym_o_gpp_maybe"></q-icon></template>' +
            '{{t(\'Not verified: a device in this call gets its keys though you have not verified it. Verify it in Settings, My devices, if you do not know it:\')}} {{callKeys.unverified.map((u) => ((known && known.participants || []).find((p) => p.userId === u.userId) || {}).displayName || u.userId).join(\', \')}}</q-banner>' +
            '<m-call-context v-if="known && known.kind === \'phone\'" :call-session-id="callSessionId"></m-call-context>' +
            '<q-banner v-if="recording && inThis" dense rounded class="m-tonal m-call-recording-notice"><template v-slot:avatar><q-icon name="sym_o_fiber_manual_record" color="negative"></q-icon></template>' +
            '{{withdrawn ? t(\'You withdrew your consent: the recording is paused while you are in the call.\') : needsAck ? t(\'This call is being recorded. The recording waits until everyone in the call acknowledges.\') : t(\'This call is being recorded. Staying in the call is consent.\')}}' +
            '<template v-slot:action><q-btn v-if="needsAck || withdrawn" flat no-caps color="primary" :label="withdrawn ? t(\'Consent again\') : t(\'Acknowledge\')" @click="recAction(\'ackRecording\')"></q-btn>' +
            '<q-btn v-if="!withdrawn" flat no-caps color="negative" :label="t(\'Withdraw consent\')" @click="recAction(\'withdrawRecording\')"></q-btn></template></q-banner>' +
            '<q-banner v-if="rec && rec.failed && inThis" dense rounded class="m-tonal text-warning">{{t(\'The recording stopped: the recorder failed. The call goes on.\')}}</q-banner>' +
            '<q-banner v-for="p in (inThis || isHost) ? waiting : []" :key="\'w\' + p.callParticipantId" dense rounded class="m-tonal">' +
            '<template v-slot:avatar><q-icon name="sym_o_person_add"></q-icon></template>{{p.displayName}} {{t(\'is waiting to join\')}}' +
            '<template v-slot:action><q-btn flat no-caps color="positive" :label="t(\'Let in\')" @click="admit(p, true)"></q-btn>' +
            '<q-btn flat no-caps color="negative" :label="t(\'Turn away\')" @click="admit(p, false)"></q-btn></template></q-banner>' +
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
            '<q-btn unelevated color="primary" icon="sym_o_call" :disable="e2eeUnsupported" :label="known.myLegStatus === \'offered\' ? t(\'Answer\') : t(\'Join\')" @click="join"></q-btn></div></div>' +
            // the call view: the grid, or the speaker (or a screen share) large over a strip of the rest
            '<div v-if="inThis && mainTile" style="height:60vh" class="q-mb-sm">' +
            '<div v-for="tile in [mainTile]" :key="\'main\' + tile.key" class="fit">' + '<div class="m-call-tile" :style="tileStyle(tile)">' +
            '<m-call-video v-if="tile.stream" :stream="tile.stream" muted :mirror="!!tile.mirror" :style="tile.screen ? \'width:100%;height:100%;object-fit:contain\' : \'width:100%;height:100%;object-fit:cover\'"></m-call-video>' +
            '<div v-else class="fit flex flex-center text-white text-subtitle1"><q-icon v-if="tile.phone" name="sym_o_call" class="q-mr-xs"></q-icon>{{tile.name}}</div>' +
            '<div v-if="tile.reaction" class="absolute-center text-h3">{{tile.reaction}}</div>' +
            '<q-chip v-if="tile.audioOnly" dense square class="absolute-top-right q-ma-xs" color="grey-9" text-color="white" icon="sym_o_signal_cellular_alt_1_bar">{{t(\'Audio only: low bandwidth\')}}</q-chip>' +
            '<div class="absolute-bottom-left q-pa-xs text-white text-caption">{{tile.name}}<q-icon v-if="tile.muted" name="sym_o_mic_off" class="q-ml-xs"></q-icon>' +
            '<q-icon v-if="tile.hand" name="sym_o_back_hand" color="warning" class="q-ml-xs"></q-icon></div></div>' + '</div></div>' +
            '<div v-if="inThis" :style="gridStyle">' +
            '<div v-for="tile in stripTiles" :key="tile.key">' + '<div class="m-call-tile" :style="tileStyle(tile)">' +
            '<m-call-video v-if="tile.stream" :stream="tile.stream" muted :mirror="!!tile.mirror" :style="tile.screen ? \'width:100%;height:100%;object-fit:contain\' : \'width:100%;height:100%;object-fit:cover\'"></m-call-video>' +
            '<div v-else class="fit flex flex-center text-white text-subtitle1"><q-icon v-if="tile.phone" name="sym_o_call" class="q-mr-xs"></q-icon>{{tile.name}}</div>' +
            '<div v-if="tile.reaction" class="absolute-center text-h3">{{tile.reaction}}</div>' +
            '<q-chip v-if="tile.audioOnly" dense square class="absolute-top-right q-ma-xs" color="grey-9" text-color="white" icon="sym_o_signal_cellular_alt_1_bar">{{t(\'Audio only: low bandwidth\')}}</q-chip>' +
            '<div class="absolute-bottom-left q-pa-xs text-white text-caption">{{tile.name}}<q-icon v-if="tile.muted" name="sym_o_mic_off" class="q-ml-xs"></q-icon>' +
            '<q-icon v-if="tile.hand" name="sym_o_back_hand" color="warning" class="q-ml-xs"></q-icon></div></div>' + '</div></div>' +
            '<div v-if="inThis && !remotes.length" class="m-muted">{{call.phase === \'connected\' ? t(\'Waiting for the others\') : t(\'Connecting\')}}</div>' +
            // the captions: the notice while they are on, why they are unavailable, and each speaker's last lines
            captionsBox("inThis", "captionLines") +
            // the bottom bar
            '<div v-if="inThis" class="row justify-center q-gutter-sm q-mt-sm">' +
            '<q-btn round unelevated :color="call.muted ? \'negative\' : \'grey-8\'" :icon="call.muted ? \'sym_o_mic_off\' : \'sym_o_mic\'" :aria-label="t(\'Mute\')" @click="moquiCall().toggleMute()"><q-tooltip>{{call.muted ? t(\'Unmute\') : t(\'Mute\')}}</q-tooltip></q-btn>' +
            '<q-btn round unelevated :color="call.cameraOff ? \'negative\' : \'grey-8\'" :icon="call.cameraOff ? \'sym_o_videocam_off\' : \'sym_o_videocam\'" :aria-label="t(\'Camera\')" :disable="!call.localStream || !call.localStream.getVideoTracks().length" @click="moquiCall().toggleCamera()"><q-tooltip>{{t(\'Camera\')}}</q-tooltip></q-btn>' +
            '<q-btn v-if="canShare" round unelevated :color="call.sharing ? \'primary\' : \'grey-8\'" :icon="call.sharing ? \'sym_o_stop_screen_share\' : \'sym_o_present_to_all\'" :aria-label="call.sharing ? t(\'Stop sharing\') : t(\'Share the screen\')" @click="call.sharing ? moquiCall().stopShare() : moquiCall().shareScreen()"><q-tooltip>{{call.sharing ? t(\'Stop sharing\') : t(\'Share the screen\')}}</q-tooltip></q-btn>' +
            '<q-btn round unelevated :color="call.hand ? \'warning\' : \'grey-8\'" icon="sym_o_back_hand" :aria-label="t(\'Raise your hand\')" @click="moquiCall().toggleHand()"><q-tooltip>{{call.hand ? t(\'Lower your hand\') : t(\'Raise your hand\')}}</q-tooltip></q-btn>' +
            '<q-btn v-if="call.captions.allowed || call.captions.on" round unelevated :color="call.captions.on ? \'primary\' : \'grey-8\'" icon="sym_o_closed_caption" :aria-label="call.captions.on ? t(\'Turn off captions\') : t(\'Turn on captions\')" @click="moquiCall().toggleCaptions()"><q-tooltip>{{call.captions.on ? t(\'Turn off captions\') : t(\'Turn on captions\')}}</q-tooltip></q-btn>' +
            '<q-btn round unelevated color="grey-8" icon="sym_o_add_reaction" :aria-label="t(\'React\')"><q-menu><div class="row q-pa-xs">' +
            '<q-btn v-for="emoji in reactions" :key="emoji" flat dense size="lg" v-close-popup @click="moquiCall().sendReaction(emoji)">{{emoji}}</q-btn></div></q-menu></q-btn>' +
            '<q-btn round unelevated color="grey-8" :icon="call.layout === \'speaker\' ? \'sym_o_grid_view\' : \'sym_o_person\'" :aria-label="call.layout === \'speaker\' ? t(\'Grid view\') : t(\'Speaker view\')" @click="moquiCall().setLayout(call.layout === \'speaker\' ? \'grid\' : \'speaker\')"><q-tooltip>{{call.layout === \'speaker\' ? t(\'Grid view\') : t(\'Speaker view\')}}</q-tooltip></q-btn>' +
            '<q-btn round unelevated color="grey-8" icon="sym_o_group" :aria-label="t(\'Participants\')" @click="call.participantsOpen = !call.participantsOpen"><q-badge v-if="raisedCount" color="warning" floating>{{raisedCount}}</q-badge><q-tooltip>{{t(\'Participants\')}}</q-tooltip></q-btn>' +
            '<q-btn v-if="canRecord && !recording" round unelevated color="grey-8" icon="sym_o_radio_button_checked" :aria-label="t(\'Record\')" @click="recAction(\'startRecording\')"><q-tooltip>{{t(\'Record the call\')}}</q-tooltip></q-btn>' +
            '<q-btn v-if="canRecord && recording" round unelevated color="negative" icon="sym_o_radio_button_checked" :aria-label="t(\'Recording\')"><q-tooltip>{{recordingText}}</q-tooltip><q-menu><div class="column q-pa-xs">' +
            '<q-btn v-if="recording.state === \'recording\'" flat no-caps align="left" icon="sym_o_pause" :label="t(\'Pause recording\')" v-close-popup @click="recAction(\'pauseRecording\')"></q-btn>' +
            '<q-btn v-if="recording.state === \'paused\' && recording.pauseReason === \'user\'" flat no-caps align="left" icon="sym_o_play_arrow" :label="t(\'Resume recording\')" v-close-popup @click="recAction(\'resumeRecording\')"></q-btn>' +
            '<q-btn flat no-caps align="left" color="negative" icon="sym_o_stop" :label="t(\'Stop recording\')" v-close-popup @click="recAction(\'stopRecording\')"></q-btn></div></q-menu></q-btn>' +
            '<q-btn v-if="hasPhone" round unelevated color="grey-8" icon="sym_o_dialpad" :aria-label="t(\'Keypad\')"><q-tooltip>{{t(\'Keypad\')}}</q-tooltip><q-menu>' +
            '<div class="q-pa-sm" style="display:grid;grid-template-columns:repeat(3,48px);gap:4px"><q-btn v-for="key in keys" :key="key" outline dense size="lg" @click="dtmf(key)">{{key}}</q-btn></div></q-menu></q-btn>' +
            '<m-call-actions v-if="inThis && known && known.status !== \'ended\'" :view="known"></m-call-actions>' +
            '<q-btn round unelevated color="negative" icon="sym_o_call_end" :aria-label="t(\'Leave\')" @click="leave"><q-tooltip>{{t(\'Leave\')}}</q-tooltip></q-btn>' +
            '<q-btn v-if="isHost && known && known.kind !== \'direct\'" flat no-caps color="negative" :label="t(\'End for everyone\')" @click="endForAll"></q-btn>' +
            '<m-ext-actions v-if="known" slot="call-actions" :item="{callSessionId: callSessionId}"></m-ext-actions></div>' +
            // the participants: who is in it, who is ringing, raised hands
            '<q-list v-if="inThis && call.participantsOpen && known" bordered separator dense class="rounded-borders q-mt-sm">' +
            '<q-item v-for="p in known.participants || []" :key="p.callParticipantId"><q-item-section avatar><m-avatar :name="p.displayName" :user-id="p.userId || undefined" size="28px"></m-avatar></q-item-section>' +
            '<q-item-section><q-item-label>{{p.displayName}}<span v-if="p.role === \'host\'" class="text-caption m-muted"> · {{t(\'host\')}}</span></q-item-label>' +
            '<q-item-label caption>{{p.inCall ? t(\'In the call\') : p.ringing ? t(\'Ringing\') : t(\'Not in the call\')}}<span v-if="keyText(p)" class="m-call-key-state"> · <q-icon name="sym_o_key" size="14px"></q-icon> {{keyText(p)}}</span></q-item-label></q-item-section>' +
            '<q-item-section side class="row no-wrap items-center"><q-icon v-if="handOf(p)" name="sym_o_back_hand" color="warning"></q-icon>' +
            '<q-btn v-if="isHost && p.userId !== moquiCallMe() && p.admission === \'admitted\'" flat dense round size="sm" icon="sym_o_person_remove" :aria-label="t(\'Remove from the call\')" @click="remove(p)"><q-tooltip>{{t(\'Remove from the call\')}}</q-tooltip></q-btn>' +
            '</q-item-section></q-item></q-list>' +
            "</div>"
    };
    // V5: who a phone caller is (the call-context providers, as the user rung): names with their record links, open records
    components["m-call-context"] = {
        name: "mCallContext",
        props: { callSessionId: String, dense: Boolean },
        data() { return { state }; },
        computed: {
            context() { return this.state.contextOf[this.callSessionId] || null; },
            contacts() { return this.context ? (this.context.contactList || []).slice(0, this.dense ? 1 : 5) : []; },
            records() { return this.context && !this.dense ? (this.context.recordList || []).slice(0, 5) : []; }
        },
        methods: { t },
        template:
            '<div v-if="contacts.length || records.length" class="m-call-context">' +
            '<div v-for="c in contacts" :key="c.sourceId + c.url + c.name" class="text-caption">' +
            '<q-icon :name="c.kind === \'organization\' ? \'sym_o_domain\' : \'sym_o_person\'" size="16px" class="q-mr-xs"></q-icon>' +
            '<a v-if="c.url" :href="c.url">{{c.name}}</a><span v-else>{{c.name}}</span><span v-if="c.detail" class="m-muted"> · {{c.detail}}</span></div>' +
            '<div v-for="r in records" :key="r.sourceId + r.url + r.title" class="text-caption">' +
            '<q-icon name="sym_o_assignment" size="16px" class="q-mr-xs"></q-icon><a v-if="r.url" :href="r.url">{{r.title}}</a><span v-else>{{r.title}}</span>' +
            '<span v-if="r.status" class="m-muted"> · {{r.status}}</span></div></div>'
    };

    // V5: hold, transfer (blind or attended, then complete or cancel), add a person or a number, merge another call, make it a
    // meeting; each with the call's revision, so a stale view is refused rather than acting twice
    components["m-call-actions"] = {
        name: "mCallActions",
        props: { view: Object },
        data() {
            return { state, dialog: null, targetType: "user", userId: null, userOptions: [], number: "", queueId: null, queueOptions: [],
                mode: "blind", otherId: null, busy: false };
        },
        computed: {
            phoneParticipants() { return (this.view.participants || []).filter((p) => p.identity === "phone" && p.inCall); },
            hasPhone() { return this.phoneParticipants.length > 0; },
            held() { return this.phoneParticipants.some((p) => p.onHold); },
            myConsult() { const x = this.view.transfer; return x && x.mine && x.status === "consulting" ? x : null; },
            otherCalls() {
                return Object.values(this.state.calls).filter((v) => v.callSessionId !== this.view.callSessionId && v.status === "active" &&
                    v.myLegStatus === "answered").map((v) => ({ value: v.callSessionId,
                    label: (v.participants || []).filter((p) => p.userId !== moqui.call.me).map((p) => p.displayName).join(", ") || t("Call") }));
            },
            canSend() {
                if (this.dialog === "merge") return !!this.otherId;
                if (this.targetType === "user") return !!this.userId;
                if (this.targetType === "number") return !!this.number.trim();
                return !!this.queueId;
            }
        },
        methods: {
            t,
            act(name, params) {
                this.busy = true;
                return moqui.call.callAction(name, Object.assign({ callSessionId: this.view.callSessionId, revision: this.view.revision }, params || {}))
                    .then((json) => { this.busy = false; if (!json.reasonCode) this.dialog = null; return json; }, () => (this.busy = false));
            },
            open(kind) {
                this.dialog = kind;
                this.targetType = "user";
                this.userId = null;
                this.number = "";
                this.queueId = null;
                this.otherId = null;
                this.mode = "blind";
                if (kind === "transfer") moqui.call.request("transferTargets", {}, "GET").then((json) => (this.queueOptions = (json.queueList || [])
                    .map((q) => ({ value: q.callQueueId, label: q.name + " (" + q.waitingCount + " " + t("waiting") + ", " + q.agentsAvailable + " " + t("free") + ")" }))));
            },
            searchUsers(term, update) {
                moqui.request(moqui.call.base() + "/searchUsers", { method: "GET", params: { term: term || "" } })
                    .then((result) => update(() => (this.userOptions = Array.isArray(result.json) ? result.json : [])), () => update(() => null));
            },
            send() {
                if (this.dialog === "merge") return this.act("mergeCalls", { otherCallSessionId: this.otherId });
                const target = this.targetType === "user" ? this.userId : this.targetType === "number" ? this.number.trim() : this.queueId;
                if (this.dialog === "add") {
                    return this.act("addParticipant", this.targetType === "user" ? { userId: target } : { phoneNumber: target });
                }
                return this.act("transferCall", { targetType: this.targetType, targetId: target, mode: this.targetType === "queue" ? "blind" : this.mode });
            },
            hold() { this.act(this.held ? "resumeCall" : "holdCall"); },
            meeting() { this.act("toMeeting"); },
            finish(complete) { moqui.call.callAction(complete ? "completeTransfer" : "cancelTransfer", { callTransferId: this.myConsult.callTransferId }); }
        },
        template:
            '<span class="m-call-actions">' +
            '<q-btn v-if="hasPhone" round unelevated :color="held ? \'warning\' : \'grey-8\'" :icon="held ? \'sym_o_play_arrow\' : \'sym_o_pause\'" :disable="busy"' +
            ' :aria-label="held ? t(\'Resume\') : t(\'Hold\')" @click="hold" class="q-ml-xs"><q-tooltip>{{held ? t(\'Take off hold\') : t(\'Put on hold\')}}</q-tooltip></q-btn>' +
            '<q-btn round unelevated color="grey-8" icon="sym_o_more_horiz" :aria-label="t(\'More call actions\')" class="q-ml-xs"><q-menu><q-list dense style="min-width:220px">' +
            '<q-item v-if="hasPhone && !view.transfer" clickable v-close-popup @click="open(\'transfer\')"><q-item-section avatar><q-icon name="sym_o_phone_forwarded"></q-icon></q-item-section><q-item-section>{{t(\'Transfer\')}}</q-item-section></q-item>' +
            '<q-item clickable v-close-popup @click="open(\'add\')"><q-item-section avatar><q-icon name="sym_o_person_add"></q-icon></q-item-section><q-item-section>{{t(\'Add a participant\')}}</q-item-section></q-item>' +
            '<q-item v-if="otherCalls.length && !view.e2ee" clickable v-close-popup @click="open(\'merge\')"><q-item-section avatar><q-icon name="sym_o_call_merge"></q-icon></q-item-section><q-item-section>{{t(\'Merge another call\')}}</q-item-section></q-item>' +
            '<q-item v-if="view.kind !== \'meeting\'" clickable v-close-popup @click="meeting"><q-item-section avatar><q-icon name="sym_o_groups"></q-icon></q-item-section><q-item-section>{{t(\'Make it a meeting\')}}</q-item-section></q-item>' +
            '</q-list></q-menu></q-btn>' +
            '<q-banner v-if="myConsult" dense rounded class="m-tonal q-mt-sm"><template v-slot:avatar><q-icon name="sym_o_phone_forwarded"></q-icon></template>' +
            '{{t(\'Transferring to\')}} {{myConsult.targetName}}: {{t(\'the caller is on hold\')}}' +
            '<template v-slot:action><q-btn flat no-caps color="positive" :label="t(\'Complete transfer\')" @click="finish(true)"></q-btn>' +
            '<q-btn flat no-caps color="negative" :label="t(\'Cancel transfer\')" @click="finish(false)"></q-btn></template></q-banner>' +
            '<q-dialog :model-value="!!dialog" @update:model-value="v => { if (!v) dialog = null }"><q-card style="min-width:360px">' +
            '<q-card-section class="text-h6">{{dialog === \'transfer\' ? t(\'Transfer the call\') : dialog === \'add\' ? t(\'Add a participant\') : t(\'Merge another call into this one\')}}</q-card-section>' +
            '<q-card-section v-if="dialog === \'merge\'"><q-select dense outlined emit-value map-options v-model="otherId" :options="otherCalls" :label="t(\'Call\')"></q-select>' +
            '<div class="text-caption m-muted q-mt-sm">{{t(\'The calls become one meeting.\')}}</div></q-card-section>' +
            '<q-card-section v-else class="q-gutter-sm">' +
            '<q-btn-toggle v-model="targetType" no-caps dense unelevated toggle-color="primary" :options="dialog === \'transfer\' ? [{label: t(\'Person\'), value: \'user\'}, {label: t(\'Number\'), value: \'number\'}, {label: t(\'Queue\'), value: \'queue\'}] : view.e2ee ? [{label: t(\'Person\'), value: \'user\'}] : [{label: t(\'Person\'), value: \'user\'}, {label: t(\'Number\'), value: \'number\'}]"></q-btn-toggle>' +
            '<q-select v-if="targetType === \'user\'" dense outlined use-input emit-value map-options v-model="userId" :options="userOptions" @filter="searchUsers" :label="t(\'Who\')"></q-select>' +
            '<q-input v-if="targetType === \'number\'" dense outlined v-model="number" :label="t(\'Number\')" type="tel"></q-input>' +
            '<q-select v-if="targetType === \'queue\'" dense outlined emit-value map-options v-model="queueId" :options="queueOptions" :label="t(\'Queue\')"></q-select>' +
            '<q-option-group v-if="dialog === \'transfer\' && targetType !== \'queue\'" v-model="mode" inline dense :options="[{label: t(\'Now (blind)\'), value: \'blind\'}, {label: t(\'Talk to them first\'), value: \'attended\'}]"></q-option-group>' +
            '</q-card-section>' +
            '<q-card-actions align="right"><q-btn flat no-caps :label="t(\'Cancel\')" @click="dialog = null"></q-btn>' +
            '<q-btn unelevated no-caps color="primary" :disable="!canSend || busy" :label="dialog === \'transfer\' ? t(\'Transfer\') : dialog === \'add\' ? t(\'Add\') : t(\'Merge\')" @click="send"></q-btn></q-card-actions>' +
            '</q-card></q-dialog></span>'
    };

    components["m-call"].methods.moquiCall = () => moqui.call;
    components["m-call"].methods.moquiCallMe = () => moqui.call.me;
    // The captions notice, as m-call and m-call-guest show it: who hears what people say
    function captionNotice(c) {
        const who = c.provider || t("the configured provider");
        return c.local ? t("Captions are on: speech is transcribed on this organization's own server") + " (" + who + ")"
            : t("Captions are on: what people say is sent to") + " " + who + " " + t("to be transcribed");
    }
    // The captions box: the notice while they are on, why they are unavailable, and each speaker's last lines
    function captionsBox(shown, lines) {
        return '<div v-if="' + shown + ' && (call.captions.on || call.captions.unavailable)" class="m-call-captions q-pa-sm rounded-borders" style="background:rgba(0,0,0,.78);color:#fff" aria-live="polite">' +
            '<div class="text-caption" style="opacity:.85"><q-icon name="sym_o_closed_caption" class="q-mr-xs"></q-icon>{{captionNotice}}</div>' +
            '<div v-if="call.captions.unavailable" class="text-caption text-warning"><q-icon name="sym_o_warning" class="q-mr-xs"></q-icon>{{t(\'Captions are unavailable\')}}: {{call.captions.unavailable}}</div>' +
            '<div v-for="line in ' + lines + '" :key="line.key" class="text-body2"><span class="text-weight-bold">{{line.name}}:</span> <span :style="line.gap ? \'opacity:.6\' : \'\'">{{line.text}}</span></div></div>';
    }
    // The page of a call link for a person with no account (basic-comms CallGuest): the link's name, the guest's name, the
    // camera preview, Join or Ask to join, the lobby until someone lets them in, then the call. base is the route's path
    // (/call/<token>): its transitions check the token and take the credential in the body.
    components["m-call-guest"] = {
        name: "mCallGuest",
        props: { base: String, link: { type: Object, default: () => ({}) } },
        data() {
            return { call: state, name: "", phase: "lobby", credential: null, callSessionId: null, callLegId: null, preview: null,
                people: [], poll: null, error: "", busy: false, video: this.link.video !== false };
        },
        computed: {
            inCall() { return this.call.current && this.call.current === this.callSessionId; },
            remotes() { return Object.values(this.call.remotes); },
            // live captions (W6): a guest gets the notice and the lines as every leg does, by its socket
            captionNotice() { return captionNotice(this.call.captions); },
            rec() { return this.callSessionId ? this.call.recordingOf[this.callSessionId] || null : null; },
            recording() { return this.rec ? this.rec.recording : null; },
            needsAck() { const r = this.rec; return !!(r && r.recording && r.recording.ackRequired && !(r.myConsent && r.myConsent.acknowledged)); },
            withdrawn() { const r = this.rec; return !!(r && r.recording && r.myConsent && r.myConsent.withdrawn); },
            gridStyle() {
                const n = this.remotes.length + 1;
                const cols = n <= 1 ? 1 : n <= 4 ? 2 : n <= 9 ? 3 : 4;
                return "display:grid;gap:8px;grid-template-columns:repeat(" + cols + ",1fr)";
            }
        },
        mounted() {
            try { this.name = (root.localStorage && root.localStorage.getItem("moqui.call.guestName")) || ""; } catch (err) { this.name = ""; }
            this.startPreview();
        },
        beforeUnmount() {
            clearInterval(this.poll);
            if (this.preview && !this.inCall) this.preview.getTracks().forEach((track) => track.stop());
        },
        methods: {
            t,
            post(name, params) {
                return moqui.request(this.base + "/" + name, { method: "POST", params }).then((result) => result.json || {});
            },
            startPreview() {
                call.getMedia(this.video).then((stream) => { this.preview = stream; });
            },
            join() {
                const name = String(this.name || "").trim();
                if (!name || this.busy) return;
                try { if (root.localStorage) root.localStorage.setItem("moqui.call.guestName", name); } catch (err) { /* private window */ }
                this.busy = true;
                this.error = "";
                this.post("joinGuest", { displayName: name }).then((json) => {
                    this.busy = false;
                    if (!json.credential) { this.error = json.errors || t("This link no longer works."); return; }
                    this.credential = json.credential;
                    this.callSessionId = json.callSessionId;
                    this.callLegId = json.callLegId;
                    if (json.admission === "admitted") this.enter();
                    else this.waitInLobby();
                }, () => { this.busy = false; this.error = t("This link no longer works."); });
            },
            waitInLobby() {
                this.phase = "waiting";
                this.poll = setInterval(() => this.checkState(), 2000);
            },
            checkState() {
                this.post("recordingState", { credential: this.credential }).then((json) => call.noteRecording(this.callSessionId, json), () => null);
                this.post("guestState", { credential: this.credential }).then((json) => {
                    this.people = json.people || [];
                    if (json.admission === "admitted" && this.phase === "waiting") {
                        clearInterval(this.poll);
                        this.enter();
                    } else if (json.admission !== "waiting" && json.admission !== "admitted" || json.callStatus === "ended") {
                        clearInterval(this.poll);
                        this.phase = json.admission === "denied" ? "denied" : "ended";
                        call.hangUp();
                    }
                }, () => null);
            },
            enter() {
                this.phase = "in";
                const stream = this.preview;
                call.joinAsGuest({ callSessionId: this.callSessionId, callLegId: this.callLegId, credential: this.credential, video: this.video, stream,
                    iceServers: () => this.post("iceServers", { credential: this.credential }),
                    recording: (name) => this.post(name, { credential: this.credential }) })
                    .catch(() => { this.phase = "ended"; });
                // the names in the call, now and then (a guest hears of no one by push)
                this.poll = setInterval(() => this.checkState(), 5000);
            },
            recAction(name) {
                this.post(name, { credential: this.credential }).then(() => this.post("recordingState", { credential: this.credential }))
                    .then((json) => call.noteRecording(this.callSessionId, json), () => null);
            },
            leave() {
                const credential = this.credential;
                call.send({ rtc: "leave", callSessionId: this.callSessionId, callLegId: this.callLegId });
                call.hangUp();
                clearInterval(this.poll);
                this.phase = "left";
                this.preview = null;
                if (credential) this.post("leaveGuest", { credential }).catch(() => null);
            }
        },
        watch: {
            "call.current"(now) {
                if (!now && this.phase === "in") { this.phase = "ended"; clearInterval(this.poll); }
            }
        },
        template:
            '<div class="m-call-guest column q-gutter-md q-pa-md" style="max-width:1100px;margin:0 auto">' +
            '<div><div class="text-h5">{{link.name || t(\'Call\')}}</div><div v-if="link.ownerName" class="text-caption m-muted">{{t(\'With\')}} {{link.ownerName}}</div></div>' +
            '<q-banner v-if="error" dense rounded class="m-tonal text-negative">{{error}}</q-banner>' +
            '<q-banner v-if="recording" dense rounded class="m-tonal m-call-recording-notice"><template v-slot:avatar><q-icon name="sym_o_fiber_manual_record" :color="recording.state === \'recording\' ? \'negative\' : \'warning\'"></q-icon></template>' +
            '<div class="text-weight-medium">{{moquiRecordingText(recording)}}</div>' +
            '{{withdrawn ? t(\'You withdrew your consent: the recording is paused while you are in the call.\') : needsAck ? t(\'This call is being recorded. The recording waits until everyone in the call acknowledges.\') : t(\'This call is being recorded. Staying in the call is consent.\')}}' +
            '<template v-if="phase === \'in\'" v-slot:action><q-btn v-if="needsAck || withdrawn" flat no-caps color="primary" :label="withdrawn ? t(\'Consent again\') : t(\'Acknowledge\')" @click="recAction(\'ackRecording\')"></q-btn>' +
            '<q-btn v-if="!withdrawn" flat no-caps color="negative" :label="t(\'Withdraw consent\')" @click="recAction(\'withdrawRecording\')"></q-btn></template></q-banner>' +
            // the lobby: name, preview, join
            '<div v-if="phase === \'lobby\' || phase === \'waiting\'" class="row q-col-gutter-md">' +
            '<div class="col-12 col-md-7"><div style="background:#000;aspect-ratio:16/9;border-radius:8px;overflow:hidden">' +
            '<m-call-video v-if="preview && preview.getVideoTracks().length" :stream="preview" muted mirror style="width:100%;height:100%;object-fit:cover"></m-call-video>' +
            '<div v-else class="fit flex flex-center text-white">{{t(\'No camera or no permission for it\')}}</div></div></div>' +
            '<div class="col-12 col-md-5 column q-gutter-sm">' +
            '<q-input v-model="name" outlined dense maxlength="60" :label="t(\'Your name\')" :disable="phase === \'waiting\'" @keyup.enter="join"></q-input>' +
            '<q-banner v-if="link.encrypted" dense rounded class="m-tonal text-negative"><template v-slot:avatar><q-icon name="sym_o_lock"></q-icon></template>{{t(\'This meeting is end-to-end encrypted: only people with an account can join it. Ask its host for another way in.\')}}</q-banner>' +
            '<q-btn v-if="phase === \'lobby\'" unelevated color="primary" icon="sym_o_call" :loading="busy" :disable="!name.trim() || !!link.encrypted" :label="link.policy === \'link\' ? t(\'Join\') : t(\'Ask to join\')" @click="join"></q-btn>' +
            '<div v-if="phase === \'waiting\'" class="row items-center q-gutter-sm"><q-spinner size="20px"></q-spinner><span>{{t(\'Waiting for someone in the call to let you in\')}}</span></div>' +
            '</div></div>' +
            // the call
            '<div v-if="phase === \'in\'">' +
            '<div :style="gridStyle">' +
            '<div style="position:relative;background:#000;aspect-ratio:16/9;border-radius:8px;overflow:hidden">' +
            '<m-call-video v-if="call.localStream && call.localStream.getVideoTracks().length && !call.cameraOff" :stream="call.localStream" muted mirror style="width:100%;height:100%;object-fit:cover"></m-call-video>' +
            '<div v-else class="fit flex flex-center text-white">{{name}}</div>' +
            '<div class="absolute-bottom-left q-pa-xs text-white text-caption">{{name}}</div></div>' +
            '<div v-for="r in remotes" :key="r.legId" style="position:relative;background:#000;aspect-ratio:16/9;border-radius:8px;overflow:hidden">' +
            '<m-call-video v-if="(r.screen || r.stream) && (r.screen || r.stream).getVideoTracks().length && !r.cameraOff" :stream="r.screen || r.stream" muted style="width:100%;height:100%;object-fit:cover"></m-call-video>' +
            '<div v-else class="fit flex flex-center text-white">{{r.name}}</div>' +
            '<m-call-audio :stream="r.stream" :sink-id="call.settings.audioOut"></m-call-audio>' +
            '<div class="absolute-bottom-left q-pa-xs text-white text-caption">{{r.name}}</div></div></div>' +
            '<div class="text-caption m-muted q-mt-xs">{{people.map(p => p.displayName).join(\', \')}}</div>' +
            captionsBox("true", "call.captions.lines") +
            '<div class="row justify-center q-gutter-sm q-mt-sm">' +
            '<q-btn round unelevated :color="call.muted ? \'negative\' : \'grey-8\'" :icon="call.muted ? \'sym_o_mic_off\' : \'sym_o_mic\'" :aria-label="t(\'Mute\')" @click="moquiCall().toggleMute()"></q-btn>' +
            '<q-btn round unelevated :color="call.cameraOff ? \'negative\' : \'grey-8\'" :icon="call.cameraOff ? \'sym_o_videocam_off\' : \'sym_o_videocam\'" :aria-label="t(\'Camera\')" @click="moquiCall().toggleCamera()"></q-btn>' +
            '<q-btn round unelevated :color="call.hand ? \'warning\' : \'grey-8\'" icon="sym_o_back_hand" :aria-label="t(\'Raise your hand\')" @click="moquiCall().toggleHand()"></q-btn>' +
            '<q-btn round unelevated color="negative" icon="sym_o_call_end" :aria-label="t(\'Leave\')" @click="leave"></q-btn></div></div>' +
            '<div v-if="phase === \'denied\'" class="text-subtitle1">{{t(\'You were not let in to this call.\')}}</div>' +
            '<div v-if="phase === \'ended\'" class="text-subtitle1">{{t(\'The call has ended for you.\')}}</div>' +
            '<div v-if="phase === \'left\'" class="text-subtitle1">{{t(\'You left the call.\')}}</div>' +
            "</div>"
    };
    components["m-call-guest"].methods.moquiCall = () => moqui.call;
    components["m-call-guest"].methods.moquiRecordingText = recordingText;
})(globalThis);
