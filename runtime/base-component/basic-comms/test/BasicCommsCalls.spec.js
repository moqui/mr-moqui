// BasicComms Calls (screen/BasicComms/Calls.xml, Calls/History, Calls/Call and its terminal twin; plans/Comms-Framework.md
// W1): a user with no party calls john.doe through the Calls transitions the page's call store posts to, john.doe answers
// and both see the call in History; a user who is not in the call is refused the same post that works for john.doe, through
// either mount (IronStone's /qapps/Comms when it is installed); the terminal twin shows the call and leaves it; a stored
// recording (W5) plays for the one who recorded it and is nothing for anyone else; so does a voicemail (V2), for its owner.
// An end-to-end encrypted call (W8) is answered only with the browser's device, refuses recording, and the terminal twin
// shows its encryption without Answer.
// Run: ./moq test basic-comms --spec BasicCommsCalls
const root = "component://basic-comms/screen/BasicComms.xml"
const bossRoot = "component://mr-boss/screen/Comms.xml"
const CALLER = "BASIC_COMMS_CALLER"
const STRANGER = "BASIC_COMMS_CALL_STRANGER"
const NODE = "basic-comms-calls-spec"
const bossInstalled = !!ec.resource.getLocationReference(bossRoot)?.getExists()
let callId = null

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function request(rootLocation, path, parameters, method) {
    const str = ec.screen.makeTest().rootScreen(rootLocation).render(path, parameters || {}, method || null)
    const output = String(str.output || "")
    ec.message.clearErrors()
    let json = null
    try {
        json = JSON.parse(output)
    } catch (err) {
        json = null
    }
    return {status: str.status, json: json || {}, output}
}
const post = (rootLocation, name, parameters) => request(rootLocation, "Calls/" + name, parameters, "post")
function makeUser(userId, username) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() == 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username, userFullName: "Calls " + username, disabled: "N"}, true, null, null)
            account.create()
        }
        // BasicComms is opened by BASIC_COMMS_USERS, not ALL_USERS; IronStone's Comms, when installed, by BOSS_USERS
        for (const group of ["BASIC_COMMS_USERS", "BOSS_USERS"]) {
            if (ec.entity.find("moqui.security.UserGroup").condition("userGroupId", group).count() == 0) continue
            const found = ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", group)
            if (found.condition("userId", userId).count() > 0) continue
            const member = ec.entity.makeValue("moqui.security.UserGroupMember")
            member.setFields({userGroupId: group, userId, fromDate: ec.user.nowTimestamp}, true, null, null)
            member.create()
        }
    })
}
const john = () => sys(() => ec.entity.find("moqui.security.UserAccount").condition("username", "john.doe").one().userId)

spec({
    stepwise: true,
    setupSpec() {
        makeUser(CALLER, "basic.comms.caller")
        makeUser(STRANGER, "basic.comms.call.stranger")
        sys(() => call("org.moqui.impl.CallServices.register#MediaNode", {instanceId: NODE, mediaAddress: "127.0.0.1:10000"}))
    },
    cleanupSpec() {
        sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", NODE).deleteAll())
    }
})

feature("call device tests report pending and failed requests and release cancelled or unmounted streams", {
    expect() {
        // Actual component methods with controlled media answers; this is no browser permission or live-media proof.
        const source = ec.resource.getLocationText("component://basic-comms/screen/BasicComms/Settings/Calls.qvue", false)
        const requests = [], timers = new Map()
        let timerId = 0, stopped = 0
        const scope = {
            module: {exports: {}},
            moqui: {call: {getMedia: () => ({then: (resolve, reject) => requests.push({resolve, reject})})}},
            window: {navigator: {mediaDevices: {enumerateDevices: () => ({then: resolve => resolve([])})}}},
            setTimeout: callback => { timers.set(++timerId, callback); return timerId },
            clearTimeout: id => timers.delete(id)
        }
        runInContext(/<script>([\s\S]*?)<\/script>/.exec(source)[1], scope)
        const component = scope.module.exports
        const state = Object.assign(component.data(), component.methods)
        const stream = () => ({getTracks: () => [{stop: () => stopped++}], getAudioTracks: () => []})
        state.test()
        assert.equal(state.pending, true, "a pending permission request has visible state immediately")
        assert.equal(state.testing, false, "waiting does not claim that media is running")
        state.stop()
        assert.equal(state.pending, false)
        requests[0].resolve(stream())
        assert.equal(stopped, 1, "media arriving after Cancel is stopped")
        assert.equal(state.stream, null)
        state.test()
        requests[0].reject(new Error("late refusal"))
        assert.equal(state.pending, true, "a stale refusal cannot end a newer request")
        requests[1].resolve(null)
        assert.equal(state.pending, false)
        assert.equal(state.testing, false)
        assert.ok(state.testError.includes("permissions"), "a null result explains how to retry")
        state.test()
        assert.equal(state.testError, null, "retry clears the previous refusal")
        requests[2].reject(new Error("permission refused"))
        assert.ok(state.testError.includes("permissions"), "a rejected request is also visible")
        state.test()
        requests[3].resolve(stream())
        assert.equal(state.pending, false)
        assert.equal(state.testing, true)
        const oldStop = timers.get(state.stopTimer)
        assert.equal(typeof oldStop, "function", "a stream stops even without an AudioContext")
        state.test()
        assert.equal(stopped, 2, "retry releases the earlier stream")
        requests[4].resolve(stream())
        oldStop()
        assert.equal(state.testing, true, "an earlier test's timeout cannot stop its replacement")
        component.beforeUnmount.call(state)
        assert.equal(stopped, 3)
        assert.equal(timers.size, 0, "unmount clears the running test's timer")
        state.test()
        component.beforeUnmount.call(state)
        requests[5].resolve(stream())
        assert.equal(stopped, 4, "media arriving after unmount is stopped too")
        assert.equal(state.testing, false)
        assert.equal(state.stream, null)
    }
})

feature("a user with no party calls john.doe and he answers, through the Calls transitions", {
    when() {
        ec.user.internalLoginUser("basic.comms.caller")
        const caps = request(root, "Calls/capabilities", {}, null)
        const started = post(root, "startCall", {calleeUserIdList: john(), video: "true"})
        callId = started.json.callSessionId
        login("john.doe")
        const ringing = request(root, "Calls/activeCalls", {}, null)
        const answered = post(root, "answerCall", {callSessionId: callId})
        const history = request(root, "Calls/History", {}, null)
        return {caps, started, ringing, answered, history}
    },
    then({caps, started, ringing, answered, history}) {
        caps.json.active == true
        started.json.errors == null
        started.json.call.kind == "direct"
        started.json.callLegId != null
        ringing.json.callList.some(c => c.callSessionId == callId && c.myLegStatus == "offered")
        answered.json.call.status == "active"
        answered.json.callLegId != null
        history.status == 200
        history.output.includes("In the call")
    }
})

feature("a user who is not in the call is refused what works for john.doe, through either mount", {
    when() {
        ec.user.internalLoginUser("basic.comms.call.stranger")
        const roots = bossInstalled ? [root, bossRoot] : [root]
        const refused = roots.map(r => ({
            state: request(r, "Calls/callState", {callSessionId: callId}, null).json,
            join: post(r, "joinCall", {callSessionId: callId}).json,
            end: post(r, "endCall", {callSessionId: callId}).json
        }))
        login("john.doe")
        const allowed = roots.map(r => request(r, "Calls/callState", {callSessionId: callId}, null).json)
        return {refused, allowed}
    },
    then({refused, allowed}) {
        refused.every(r => r.state.reasonCode == "notFound" && r.state.call == null)
        refused.every(r => r.join.reasonCode == "notFound")
        refused.every(r => r.end.reasonCode == "notFound")
        allowed.every(a => a.call && a.call.status == "active")
    }
})

feature("the terminal twin shows the call and leaves it", {
    when() {
        login("john.doe")
        const shown = sys(() => ec.screen.makeTest().rootScreen(root).renderMode("tui").render("Calls/CallTui", {callSessionId: callId}, null))
        const text = String(shown.output || "")
        ec.message.clearErrors()
        request(root, "Calls/CallTui/leave", {callSessionId: callId}, "post")
        const after = request(root, "Calls/callState", {callSessionId: callId}, null).json
        return {text, after}
    },
    then({text, after}) {
        text.includes("not live in the terminal")
        text.includes("Calls basic.comms.caller")
        after.call.status == "ended"
    }
})

feature("an end-to-end encrypted call through the transitions: a browser without its device does not answer; the terminal twin shows the encryption and no Answer; Call back from History is encrypted again", {
    when() {
        // a device of john.doe's with its keys uploaded (opaque here); the browser names it with RTCRtpScriptTransform
        const johnId = john()
        sys(() => {
            if (ec.entity.find("moqui.chat.ChatDevice").condition({userId: johnId, deviceId: "BCCSPECDEV"}).count() === 0) {
                const device = ec.entity.makeValue("moqui.chat.ChatDevice")
                device.setFields({userId: johnId, deviceId: "BCCSPECDEV", ed25519Key: "ed-spec", curve25519Key: "curve-spec", listRevision: 1,
                    lastToDevicePos: 0, ackedToDevicePos: 0, createdDate: ec.user.nowTimestamp}, true, null, null)
                device.create()
            }
        })
        ec.user.internalLoginUser("basic.comms.caller")
        const started = post(root, "startCall", {calleeUserIdList: johnId, video: "true", requireE2ee: "true"})
        const encryptedId = started.json.callSessionId
        login("john.doe")
        const shown = sys(() => ec.screen.makeTest().rootScreen(root).renderMode("tui").render("Calls/CallTui", {callSessionId: encryptedId}, null))
        const text = String(shown.output || "")
        ec.message.clearErrors()
        const plainly = post(root, "answerCall", {callSessionId: encryptedId})
        const answered = post(root, "answerCall", {callSessionId: encryptedId, cryptoDeviceId: "BCCSPECDEV", scriptTransform: "true"})
        const recording = post(root, "startRecording", {callSessionId: encryptedId})
        post(root, "leaveCall", {callSessionId: encryptedId})
        sys(() => {
            const s = ec.entity.find("moqui.call.CallSession").condition("callSessionId", encryptedId).one()
            if (s.statusId !== "CssEnded") { s.set("statusId", "CssEnded"); s.update() }
        })
        // History's Call back of the encrypted call starts an encrypted call again, with the row's own parameters
        const row = (sys(() => call("org.moqui.impl.CallServices.find#Calls", {pageSize: 25})).callList || []).find((c) => c.callSessionId == encryptedId)
        const before = Date.now() - 1000
        request(root, "Calls/History/callBack", {otherUserIds: row.otherUserIdList.join(","), chatRoomId: row.chatRoomId || "",
            isRoomCall: "false", video: row.video ? "true" : "false", e2ee: row.e2ee ? "true" : "false"}, "post")
        const calledBack = sys(() => Array.from(ec.entity.find("moqui.call.CallSession").condition("startedByUserId", johnId)
            .condition("startedDate", ">=", new Date(before)).orderBy("-startedDate").limit(1).list())[0])
        if (calledBack) post(root, "endCall", {callSessionId: calledBack.callSessionId})
        return {started, text, plainly, answered, recording, row, calledBack}
    },
    then({started, text, plainly, answered, recording, row, calledBack}) {
        row.e2ee == true
        calledBack.callSessionId != encryptedId
        calledBack.encryptionMode == "e2ee-sframe"
        started.json.call.e2ee == true
        started.json.call.encryptionMode == "e2ee-sframe"
        text.includes("End-to-end encrypted")
        text.includes("answer it in a browser")
        plainly.json.reasonCode == "unsupportedClient"
        answered.json.errors == null
        answered.json.call.e2ee == true
        recording.json.reasonCode == "encrypted"
    }
})

feature("History has Call links, and the Meetings of an app's meetings-source through either mount", {
    when() {
        login("john.doe")
        const own = request(root, "Calls/History", {}, null)
        const app = bossInstalled ? request(bossRoot, "Calls/History", {}, null) : null
        const meetings = sys(() => call("org.moqui.impl.ShellServices.call#Providers", {hook: "meetings-source",
            parameters: {fromDate: new Date(), thruDate: new Date(Date.now() + 7 * 86400000)}})) || {}
        return {own, app, meetings}
    },
    then({own, app, meetings}) {
        own.status == 200
        own.output.includes("Call links")
        !bossInstalled || app.output.includes("Meetings")
        !bossInstalled || meetings.results.map(r => r.sourceId).includes("boss.calendar")
        !bossInstalled || meetings.failedParts.length == 0
        bossInstalled || meetings.unavailable == true
    }
})

// W5: a stored recording of the call (rows and a file made here, as the media node's recorder would store them) plays only
// for the one who recorded it; john.doe, in the call but not its host, and a stranger get nothing for the same file
feature("Recordings lists a recording for its owner, and its file is nothing for anyone else", {
    when() {
        const location = "dbresource://moqui/call/recording/BCC_SPEC_REC/BCC_SPEC_SEG.ogg"
        sys(() => {
            ec.resource.getLocationReference(location).putText("OggS-spec")
            const rows = [
                ["moqui.call.CallRecording", {callRecordingId: "BCC_SPEC_REC", callSessionId: callId, ownerUserId: CALLER, kindEnumId: "CrkPerTrack",
                    statusId: "CrsStopped", segmentPeriod: 1, startedDate: new Date(), endedDate: new Date()}],
                ["moqui.call.CallRecordingTrack", {callRecordingTrackId: "BCC_SPEC_TRK", callRecordingId: "BCC_SPEC_REC", trackKindEnumId: "CrtMix",
                    codec: "opus", contentType: "audio/ogg"}],
                ["moqui.call.CallRecordingSegment", {callRecordingSegmentId: "BCC_SPEC_SEG", callRecordingId: "BCC_SPEC_REC",
                    callRecordingTrackId: "BCC_SPEC_TRK", segmentPeriod: 1, segmentSeq: 1, startOffsetMs: 0, durationMs: 1000,
                    contentLocation: location, contentType: "audio/ogg", byteSize: 9, processingStatusId: "CrgStored"}]
            ]
            for (const [entity, fields] of rows) {
                const value = ec.entity.makeValue(entity)
                value.setFields(fields, true, null, null)
                value.create()
            }
        })
        const file = {callRecordingSegmentId: "BCC_SPEC_SEG"}
        ec.user.internalLoginUser("basic.comms.caller")
        const list = request(root, "Calls/Recordings/recordings", {}, null).json
        const tui = sys(() => ec.screen.makeTest().rootScreen(root).renderMode("tui").render("Calls/RecordingsTui", {}, null))
        const tuiText = String(tui.output || "")
        ec.message.clearErrors()
        const owner = request(root, "Calls/Recordings/recordingFile", file, null)
        ec.user.internalLoginUser("basic.comms.call.stranger")
        const stranger = request(root, "Calls/Recordings/recordingFile", file, null)
        const strangerList = request(root, "Calls/Recordings/recordings", {}, null).json
        login("john.doe")
        const member = request(root, "Calls/Recordings/recordingFile", file, null)
        return {list, tuiText, owner, stranger, strangerList, member}
    },
    then({list, tuiText, owner, stranger, strangerList, member}) {
        list.recordingList.some(r => r.callRecordingId == "BCC_SPEC_REC" && r.canDelete && r.periods.length == 1)
        tuiText.includes("They play in the browser")
        owner.output.includes("OggS-spec")
        Object.keys(stranger.json || {nothing: true}).length == 0
        !stranger.output.includes("OggS")
        Object.keys(member.json || {nothing: true}).length == 0
        !member.output.includes("OggS")
        !strangerList.recordingList.some(r => r.callRecordingId == "BCC_SPEC_REC")
    },
    cleanup() {
        sys(() => {
            for (const entity of ["moqui.call.CallRecordingSegment", "moqui.call.CallRecordingTrack", "moqui.call.CallRecording"])
                ec.entity.find(entity).condition("callRecordingId", "BCC_SPEC_REC").deleteAll()
            ec.resource.getLocationReference("dbresource://moqui/call/recording/BCC_SPEC_REC/BCC_SPEC_SEG.ogg").delete()
            ec.resource.getLocationReference("dbresource://moqui/call/recording/BCC_SPEC_REC").delete()
        })
    }
})

// V2: a voicemail (its rows and file made here, as the SIP engine would store them) is in its owner's Voicemail tab and
// plays only for them through the audio transition; a stranger gets nothing for the same id. Settings Phone renders.
feature("Voicemail lists a message for its owner, and its audio is nothing for anyone else", {
    when() {
        const location = "dbresource://moqui/call/voicemail/BCC_SPEC_BOX/BCC_SPEC_VM.ogg"
        sys(() => {
            ec.resource.getLocationReference(location).putText("OggS-voicemail-spec")
            const rows = [
                ["moqui.call.VoicemailBox", {voicemailBoxId: "BCC_SPEC_BOX", userId: CALLER, attachAudio: "N", attachTranscript: "N"}],
                ["moqui.call.Voicemail", {voicemailId: "BCC_SPEC_VM", voicemailBoxId: "BCC_SPEC_BOX", fromNumber: "+15557770999",
                    receivedDate: new Date(), durationMs: 3000, audioLocation: location, folder: "inbox", statusId: "VmsStored"}],
                ["moqui.call.CallTranscript", {callTranscriptId: "BCC_SPEC_VMT", voicemailId: "BCC_SPEC_VM", segment: 1,
                    statusId: "CtsDone", text: "Call me back about the spec", createdDate: new Date()}]
            ]
            for (const [entity, fields] of rows) {
                const value = ec.entity.makeValue(entity)
                value.setFields(fields, true, null, null)
                value.create()
            }
        })
        const file = {voicemailId: "BCC_SPEC_VM"}
        ec.user.internalLoginUser("basic.comms.caller")
        const tab = request(root, "Calls/History", {outcome: "voicemail"}, null)
        const detail = request(root, "Calls/Voicemail/voicemail", file, null).json
        const tui = sys(() => ec.screen.makeTest().rootScreen(root).renderMode("tui").render("Calls/VoicemailTui", file, null))
        const tuiText = String(tui.output || "")
        const owner = request(root, "Calls/Voicemail/audio", file, null)
        const phone = request(root, "Settings/Phone/settings", {}, null).json
        ec.user.internalLoginUser("basic.comms.call.stranger")
        const stranger = request(root, "Calls/Voicemail/audio", file, null)
        const strangerDetail = request(root, "Calls/Voicemail/voicemail", file, null).json
        const strangerTab = request(root, "Calls/History", {outcome: "voicemail"}, null)
        return {tab, detail, tuiText, owner, phone, stranger, strangerDetail, strangerTab}
    },
    then({tab, detail, tuiText, owner, phone, stranger, strangerDetail, strangerTab}) {
        tab.status == 200
        tab.output.includes("+15557770999")
        tab.output.includes("Call me back about the spec")
        detail.voicemail.transcriptText == "Call me back about the spec"
        tuiText.includes("Call me back about the spec")
        owner.output.includes("OggS-voicemail-spec")
        phone.box.voicemailBoxId == "BCC_SPEC_BOX"
        phone.box.attachAudio == false
        Object.keys(stranger.json || {nothing: true}).length == 0
        !stranger.output.includes("OggS")
        strangerDetail.voicemail == null
        strangerDetail.reasonCode == "notFound"
        !strangerTab.output.includes("+15557770999")
    },
    cleanup() {
        sys(() => {
            ec.entity.find("moqui.call.CallTranscript").condition("voicemailId", "BCC_SPEC_VM").deleteAll()
            ec.entity.find("moqui.call.Voicemail").condition("voicemailId", "BCC_SPEC_VM").deleteAll()
            ec.entity.find("moqui.call.VoicemailBox").condition("voicemailBoxId", "BCC_SPEC_BOX").deleteAll()
            ec.resource.getLocationReference("dbresource://moqui/call/voicemail/BCC_SPEC_BOX/BCC_SPEC_VM.ogg").delete()
            ec.resource.getLocationReference("dbresource://moqui/call/voicemail/BCC_SPEC_BOX").delete()
        })
    }
})

// V5: the call-context providers' screen pop for a phone caller, as the user rung (Calls/callContext): two providers of the
// example gallery, one answering and one failing on purpose for this number, left out and named in failedParts; a user
// not in the call gets not found with the same post. Skipped when the example component is not loaded.
const exampleInstalled = ec.resource.getLocationReference("component://example/screen/Gallery.xml").getExists()
feature("call-context: a phone caller's providers answer for the user rung, one failing is left out; another user is refused", {
    when() {
        if (!exampleInstalled) return {skip: true}
        const NUMBER = "+15550109101"
        const TRUNK = "BASIC_COMMS_CTX_TRUNK"
        sys(() => {
            call("org.moqui.impl.VoiceServices.register#SipNode", {instanceId: NODE, sipAddress: "127.0.0.1:5060"})
            ec.entity.find("moqui.call.PhoneNumber").condition("phoneNumber", NUMBER).deleteAll()
            ec.entity.find("moqui.call.VoiceTrunk").condition("voiceTrunkId", TRUNK).deleteAll()
            const trunk = ec.entity.makeValue("moqui.call.VoiceTrunk")
            trunk.setFields({voiceTrunkId: TRUNK, name: "Context trunk", host: "sip.example.invalid", transportEnumId: "VtrTls",
                mediaEncryptionEnumId: "VmeSrtp", providerProfileEnumId: "VppGeneric", statusId: "VtsOk", disabled: "N"}, true, null, null)
            trunk.create()
            const number = ec.entity.makeValue("moqui.call.PhoneNumber")
            number.setFields({phoneNumber: NUMBER, voiceTrunkId: TRUNK, routeTypeEnumId: "PnrUser", routeId: john()}, true, null, null)
            number.create()
        })
        const ring = sys(() => call("org.moqui.impl.VoiceServices.receive#PhoneCall", {toNumber: NUMBER, fromNumber: "+15550100666", instanceId: NODE})) || {}
        ec.user.internalLoginUser("john.doe")
        const mine = request(root, "Calls/callContext", {callSessionId: ring.callSessionId}, "get")
        ec.user.internalLoginUser("basic.comms.call.stranger")
        const stranger = request(root, "Calls/callContext", {callSessionId: ring.callSessionId}, "get")
        ec.user.internalLoginUser("john.doe")
        sys(() => {
            const s = ec.entity.find("moqui.call.CallSession").condition("callSessionId", ring.callSessionId).one()
            if (s) { s.set("statusId", "CssEnded"); s.update() }
            ec.entity.find("moqui.call.PhoneNumber").condition("phoneNumber", NUMBER).deleteAll()
            ec.entity.find("moqui.call.VoiceTrunk").condition("voiceTrunkId", TRUNK).deleteAll()
            ec.entity.find("moqui.call.SipNode").condition("instanceId", NODE).deleteAll()
        })
        return {ring, mine: mine.json, stranger: stranger.json}
    },
    then({skip, ring, mine, stranger}) {
        skip || ring.callSessionId != null
        skip || mine.number == "+15550100666"
        skip || mine.contactList.some((c) => c.sourceId == "gallery.callContext" && c.name == "Gallery caller 0666")
        skip || mine.recordList.some((r) => r.sourceId == "gallery.callContext")
        skip || mine.failedParts.map((f) => f.sourceId).includes("gallery.callContextFailing")
        skip || !mine.contactList.some((c) => c.sourceId == "gallery.callContextFailing")
        skip || stranger.reasonCode == "notFound"
        skip || JSON.stringify(stranger.contactList) == "[]"
    }
})
