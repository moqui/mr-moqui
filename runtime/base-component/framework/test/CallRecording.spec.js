// Call recording (org.moqui.impl.CallRecordingServices and CallRecordings.js; plans/Comms-Framework.md W5, §5.4
// "Recording", D-V5): the policy off refuses; the host records and each current leg gets the notice, logged, a late
// joiner too; under the acknowledgement policy the recording pauses until every current leg acknowledged, and a resume
// starts the next period of segments; a withdrawal pauses; only the owner and the hosts read a recording (another user
// with the same segment id gets "not found"); a recorder failure ends the recording and not the call; retention deletes
// the files and rows. The media node is simulated: its segments come in through store#RecordingSegment, as the recorder
// hands them, and its failure through note#RecorderEnd. The recorder itself is moqui-host's (it rtc::).
// Run: ./moq test framework --spec CallRecording
const A = "REC_SPEC_A"
const B = "REC_SPEC_B"
const C = "REC_SPEC_C"
const D = "REC_SPEC_D"
const NODE = "rec-spec-node"
const RECORDING = "moqui.call.CallRecording"
const CONSENT = "moqui.call.CallRecordingConsent"
const SEGMENT = "moqui.call.CallRecordingSegment"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
// as the Calls screens' transitions call them: the services check the signed-in user's part in the call themselves
function as(userId, name, parameters) {
    ec.user.internalLoginUser(userId.toLowerCase().replace(/_/g, "."))
    let out = {}
    let thrown = ""
    try {
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : "org.moqui.impl.CallRecordingServices." + name)
            .parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const calls = name => "org.moqui.impl.CallServices." + name
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() > 0) return
        const account = ec.entity.makeValue("moqui.security.UserAccount")
        const username = userId.toLowerCase().replace(/_/g, ".")
        account.setFields({userId, username, userFullName: "Recording " + userId.slice(-1), disabled: "N"}, true, null, null)
        account.create()
    })
}
const recording = id => sys(() => ec.entity.find(RECORDING).condition("callRecordingId", id).one())
const consents = id => sys(() => Array.from(ec.entity.find(CONSENT).condition("callRecordingId", id).list()))
const legOf = (callSessionId, userId) => sys(() => {
    const p = ec.entity.find("moqui.call.CallParticipant").condition({callSessionId, userId}).one()
    return Array.from(ec.entity.find("moqui.call.CallLeg").condition({callParticipantId: p.callParticipantId, statusId: "ClsAnswered"}).list())[0]
})
// a segment as the media node's recorder hands it over: a spooled file, then store#RecordingSegment
let spooled = 0
function storeSegment(callRecordingId, fields) {
    const dir = ec.factory.getRuntimePath() + "/tmp/call-recording/" + callRecordingId
    const path = dir + "/spec-" + (++spooled) + ".ogg"
    ec.resource.getLocationReference("file:" + path).putText("OggS a recording spec's segment")
    return sys(() => call("org.moqui.impl.CallRecordingServices.store#RecordingSegment", Object.assign({callRecordingId,
        trackKind: "audio", source: "microphone", codec: "opus", contentType: "audio/ogg", segmentPeriod: 1, segmentSeq: 1,
        startOffsetMs: 0, durationMs: 1000, tempPath: path}, fields || {})))
}
function endAll() {
    sys(() => {
        for (const s of ec.entity.find("moqui.call.CallSession").condition("startedByUserId", "in", [A, B, C, D])
            .condition("statusId", "!=", "CssEnded").list()) {
            s.set("statusId", "CssEnded")
            s.update()
        }
        for (const r of ec.entity.find(RECORDING).condition("ownerUserId", "in", [A, B, C, D]).list()) {
            for (const seg of ec.entity.find(SEGMENT).condition("callRecordingId", r.callRecordingId).list()) {
                if (seg.contentLocation) ec.resource.getLocationReference(seg.contentLocation).delete()
            }
            for (const entity of [SEGMENT, "moqui.call.CallRecordingTrack", CONSENT]) {
                ec.entity.find(entity).condition("callRecordingId", r.callRecordingId).deleteAll()
            }
            r.delete()
        }
    })
}

spec({
    stepwise: true,
    setupSpec() {
        for (const id of [A, B, C, D]) makeUser(id)
        endAll()
        sys(() => call(calls("register#MediaNode"), {instanceId: NODE, mediaAddress: "127.0.0.1:10000"}))
        setProperty("call_recording_enabled", "false")
        setProperty("call_recording_ack_required", "false")
        setProperty("call_recording_retention_days", "30")
    },
    cleanupSpec() {
        endAll()
        sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", NODE).deleteAll())
    }
})

let callId = null
let recId = null

feature("with the policy off, even the host's start is refused", {
    when() {
        const started = as(A, calls("start#Call"), {calleeUserIdList: [B, C]})
        callId = started.callSessionId
        const joined = as(B, calls("join#Call"), {callSessionId: callId})
        return {started, joined, refused: as(A, "start#Recording", {callSessionId: callId}),
            stateOff: as(A, "get#RecordingState", {callSessionId: callId}),
            rows: sys(() => ec.entity.find(RECORDING).condition("callSessionId", callId).count())}
    },
    then({started, joined, refused, stateOff, rows}) {
        started.call.kind == "group"
        joined.call.myLegStatus == "answered"
        refused.reasonCode == "recordingOff"
        refused.errors.includes("off")
        stateOff.enabled == false
        stateOff.canRecord == false
        rows == 0
    }
})

feature("the host records: each current leg gets the notice, logged; no one else may start it with the same call", {
    when() {
        setProperty("call_recording_enabled", "true")
        const byMember = as(B, "start#Recording", {callSessionId: callId})
        const byStranger = as(D, "start#Recording", {callSessionId: callId})
        const started = as(A, "start#Recording", {callSessionId: callId})
        recId = started.callRecordingId
        const again = as(A, "start#Recording", {callSessionId: callId})
        return {byMember, byStranger, started, again, rows: consents(recId), seenByB: as(B, "get#RecordingState", {callSessionId: callId}),
            seenByA: as(A, "get#RecordingState", {callSessionId: callId}), aLeg: legOf(callId, A), bLeg: legOf(callId, B)}
    },
    then({byMember, byStranger, started, again, rows, seenByB, seenByA, aLeg, bLeg}) {
        byMember.reasonCode == "notAllowed"
        byStranger.reasonCode == "notFound"
        started.errors == ""
        started.recording.state == "recording"
        started.recording.kind == "perTrack"
        started.recording.period == 1
        again.repeated == true
        again.callRecordingId == recId
        rows.length == 2
        rows.every(r => r.noticeMethodEnumId == "CnmWeb" && r.noticeDate && r.policyRevision == "1")
        rows.find(r => r.callLegId == aLeg.callLegId).ackDate != null
        rows.find(r => r.callLegId == bLeg.callLegId).ackDate == null
        seenByB.recording.state == "recording"
        seenByB.myConsent.noticed == true
        seenByB.canRecord == false
        seenByA.canRecord == true
        recording(recId).retentionUntilDate != null
    }
})

feature("a late joiner gets the notice when its leg is bound, logged for its leg", {
    when() {
        const joined = as(C, calls("join#Call"), {callSessionId: callId})
        const before = consents(recId).length
        const bound = as(C, calls("bind#Connection"), {callSessionId: callId, callLegId: joined.callLegId})
        return {joined, bound, before, rows: consents(recId), seenByC: as(C, "get#RecordingState", {callSessionId: callId})}
    },
    then({joined, bound, before, rows, seenByC}) {
        bound.generation == 1
        before == 2
        rows.length == 3
        rows.some(r => r.callLegId == joined.callLegId && r.noticeMethodEnumId == "CnmWeb")
        seenByC.myConsent.noticed == true
        seenByC.recording.state == "recording"
    }
})

feature("under the acknowledgement policy the recording waits for every current leg, and each resume is a new period", {
    when() {
        const stopped = as(A, "stop#Recording", {callSessionId: callId})
        setProperty("call_recording_ack_required", "true")
        const started = as(A, "start#Recording", {callSessionId: callId})
        recId = started.callRecordingId
        const bAck = as(B, "ack#RecordingNotice", {callSessionId: callId})
        const cAck = as(C, "ack#RecordingNotice", {callSessionId: callId})
        const first = storeSegment(recId, {callLegId: legOf(callId, B).callLegId, segmentPeriod: 1})
        // C leaves and comes back: its new leg has not acknowledged, so the recording waits again
        as(C, calls("leave#Call"), {callSessionId: callId})
        const rejoined = as(C, calls("join#Call"), {callSessionId: callId})
        as(C, calls("bind#Connection"), {callSessionId: callId, callLegId: rejoined.callLegId})
        const held = recording(recId)
        const cAgain = as(C, "ack#RecordingNotice", {callSessionId: callId})
        const second = storeSegment(recId, {callLegId: rejoined.callLegId, segmentPeriod: 2})
        const stranger = as(D, "ack#RecordingNotice", {callSessionId: callId})
        return {stopped, started, bAck, cAck, first, held, cAgain, second, stranger, segments: sys(() =>
            Array.from(ec.entity.find(SEGMENT).condition("callRecordingId", recId).orderBy("segmentPeriod").list()))}
    },
    then({stopped, started, bAck, cAck, first, held, cAgain, second, stranger, segments}) {
        stopped.recording.state == "stopped"
        started.recording.state == "paused"
        started.recording.pauseReason == "ack"
        started.recording.period == 0
        bAck.recording.state == "paused"
        cAck.recording.state == "recording"
        cAck.recording.period == 1
        first.stored == true
        held.statusId == "CrsPaused"
        held.pauseReason == "ack"
        cAgain.recording.state == "recording"
        cAgain.recording.period == 2
        second.stored == true
        segments.map(s => s.segmentPeriod) == [1, 2]
        segments.every(s => s.processingStatusId == "CrgStored" && s.contentLocation.startsWith("dbresource://moqui/call/recording/"))
        stranger.reasonCode == "notFound"
    }
})

feature("a withdrawal pauses the recording; acknowledging again resumes it in a new period", {
    when() {
        const withdrawn = as(B, "withdraw#RecordingConsent", {callSessionId: callId})
        const row = consents(recId).find(r => r.callLegId == legOf(callId, B).callLegId)
        const again = as(B, "ack#RecordingNotice", {callSessionId: callId})
        return {withdrawn, row, again}
    },
    then({withdrawn, row, again}) {
        withdrawn.recording.state == "paused"
        withdrawn.recording.pauseReason == "withdrawn"
        row.withdrawnDate != null
        again.recording.state == "recording"
        again.recording.period == 3
    }
})

feature("only the owner and the call's hosts read a recording: another user with the same segment gets not found", {
    when() {
        const segmentId = sys(() => Array.from(ec.entity.find(SEGMENT).condition("callRecordingId", recId).list())[0].callRecordingSegmentId)
        const owner = as(A, "get#RecordingFile", {callRecordingSegmentId: segmentId})
        const member = as(B, "get#RecordingFile", {callRecordingSegmentId: segmentId})
        const stranger = as(D, "get#RecordingFile", {callRecordingSegmentId: segmentId})
        const ownerList = as(A, "find#Recordings", {callSessionId: callId})
        const memberList = as(B, "find#Recordings", {callSessionId: callId})
        const memberDelete = as(B, "delete#Recording", {callRecordingId: recId})
        const text = owner.location ? sys(() => ec.resource.getLocationReference(owner.location).getText()) : null
        return {owner, member, stranger, ownerList, memberList, memberDelete, text}
    },
    then({owner, member, stranger, ownerList, memberList, memberDelete, text}) {
        owner.errors == ""
        owner.contentType == "audio/ogg"
        owner.fileName.endsWith(".ogg")
        text == "OggS a recording spec's segment"
        member.reasonCode == "notFound"
        member.location == null
        stranger.reasonCode == "notFound"
        stranger.location == null
        ownerList.recordingList.some(r => r.callRecordingId == recId && r.periods.length == 2)
        memberList.recordingList.length == 0
        memberDelete.reasonCode == "notFound"
    }
})

feature("a recorder failure ends the recording and not the call; a segment after it is dropped", {
    when() {
        sys(() => call("org.moqui.impl.CallRecordingServices.note#RecorderEnd", {callRecordingId: recId, outcome: "failed",
            errorText: "the recorder fell behind: its queue of 4096 frames was full"}))
        const late = storeSegment(recId, {segmentPeriod: 3})
        return {row: recording(recId), late, call: as(A, calls("get#Call"), {callSessionId: callId}),
            state: as(B, "get#RecordingState", {callSessionId: callId})}
    },
    then({row, late, call, state}) {
        row.statusId == "CrsFailed"
        row.lastErrorText.includes("fell behind")
        row.endedDate != null
        late.stored == false
        call.call.status == "active"
        call.call.participants.filter(p => p.inCall).length == 3
        state.recording == null
    }
})

feature("the call's end stops its recording, and retention deletes the files and rows", {
    when() {
        setProperty("call_recording_ack_required", "false")
        const started = as(A, "start#Recording", {callSessionId: callId})
        const kept = storeSegment(started.callRecordingId, {callLegId: legOf(callId, A).callLegId})
        as(A, calls("end#Call"), {callSessionId: callId})
        const ended = recording(started.callRecordingId)
        const stillThere = sys(() => ec.resource.getLocationReference(kept.contentLocation).getExists())
        sys(() => {
            for (const r of ec.entity.find(RECORDING).condition("callSessionId", callId).list()) {
                r.set("retentionUntilDate", new Date(Date.now() - 60000))
                r.update()
            }
        })
        const expired = sys(() => call("org.moqui.impl.CallRecordingServices.expire#Recordings", {}))
        return {started, ended, stillThere, expired, gone: !sys(() => ec.resource.getLocationReference(kept.contentLocation).getExists()),
            rows: sys(() => ec.entity.find(RECORDING).condition("callSessionId", callId).count()),
            segments: sys(() => ec.entity.find(SEGMENT).condition("callRecordingId", started.callRecordingId).count()),
            consentRows: sys(() => ec.entity.find(CONSENT).condition("callSessionId", callId).count())}
    },
    then({started, ended, stillThere, expired, gone, rows, segments, consentRows}) {
        started.recording.state == "recording"
        ended.statusId == "CrsStopped"
        stillThere == true
        expired.deletedCount >= 3
        gone == true
        rows == 0
        segments == 0
        consentRows == 0
    }
})
