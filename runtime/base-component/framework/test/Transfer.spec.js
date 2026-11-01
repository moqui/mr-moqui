// Transfer, add and merge (org.moqui.impl.CallTransferServices and CallTransfers.js; plans/Comms-Framework.md §5.1, §5.3, V5):
// a call that still rings is not transferred; a stale revision and a second open transfer are refused, so two transfers
// cannot both win; an attended transfer holds the far end while the user consults the target, then cancels (the far end
// back) or completes (the user leaves); a blind transfer leaves at once and the far end waits for the target, whose
// answer races the far end hanging up, either way to one clean outcome; a target that does not answer misses it and the
// far end goes to the target's voicemail; a transfer to an international number is refused under the fraud policy (the
// same transfer to a domestic number goes); a number transferred to hears the recording notice and acknowledges it by
// pressing 1; a person is added, a number is not added to an end-to-end encrypted call; two calls on one media node
// merge into a meeting, and calls on different nodes are refused. No SIP engine runs here: fake SipNode and CallMediaNode
// rows stand in for one, and the phones' events come through note#PhoneLeg.
// Run: ./moq test framework --spec Transfer
const ADMIN = "XFER_SPEC_ADMIN"
const A = "XFER_SPEC_A"
const B = "XFER_SPEC_B"
const C = "XFER_SPEC_C"
const USERS = [ADMIN, A, B, C]
const NODE = "xfer-spec-node"
const NODE2 = "xfer-spec-node2"
const TRUNK = "XFER_SPEC_TRUNK"
const MINE = "+15550500001"
const CALLER = "+15557770500"
const DEST = "+15550123499"
const ABROAD = "+442071234567"
const SESSION = "moqui.call.CallSession"
const LEG = "moqui.call.CallLeg"
const TRANSFER = "moqui.call.CallTransfer"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
function as(userId, name, parameters) {
    if (userId) ec.user.internalLoginUser(username(userId))
    let out = {}
    let thrown = ""
    try {
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : "org.moqui.impl.CallTransferServices." + name)
            .parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const engine = (name, parameters) => as(null, name, parameters)
function makeUser(userId, groupId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "Xfer " + userId.slice(10), disabled: "N"}, true, null, null)
            account.create()
        }
        if (groupId && ec.entity.find("moqui.security.UserGroupMember").condition({userId, userGroupId: groupId}).count() === 0) {
            const member = ec.entity.makeValue("moqui.security.UserGroupMember")
            member.setFields({userId, userGroupId: groupId, fromDate: new Date(Date.now() - 60000)}, true, null, null)
            member.create()
        }
    })
}
function endAll() {
    sys(() => {
        for (const s of ec.entity.find(SESSION).condition("statusId", "!=", "CssEnded").list()) {
            const mine = ec.entity.find("moqui.call.CallParticipant").condition("callSessionId", s.callSessionId).condition("userId", "in", USERS).count()
            if (!mine && s.mediaNodeId !== NODE && s.mediaNodeId !== NODE2) continue
            s.set("statusId", "CssEnded")
            s.update()
        }
        for (const t of ec.entity.find(TRANSFER).condition("statusId", "in", ["CtsRinging", "CtsConsulting"]).list()) {
            t.set("statusId", "CtsCanceled")
            t.update()
        }
        ec.entity.find("moqui.security.UserStatus").condition("userId", "in", USERS).deleteAll()
    })
}
function clean() {
    endAll()
    sys(() => {
        for (const entity of ["moqui.call.PhoneNumberUser", "moqui.call.PhoneNumber"]) ec.entity.find(entity).condition("phoneNumber", MINE).deleteAll()
        ec.entity.find("moqui.call.VoiceUserSetting").condition("userId", "in", USERS).deleteAll()
        ec.entity.find("moqui.call.VoicemailBox").condition("userId", "in", USERS).deleteAll()
        ec.entity.find("moqui.call.VoiceTrunk").condition("voiceTrunkId", TRUNK).deleteAll()
        ec.entity.find("moqui.call.SipNode").condition("instanceId", "in", [NODE, NODE2]).deleteAll()
        ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", "in", [NODE, NODE2]).deleteAll()
    })
    setProperty("call_recording_enabled", "false")
    setProperty("call_recording_ack_required", "false")
}
function nodesUp() {
    sys(() => {
        ec.service.sync().name("org.moqui.impl.CallServices.register#MediaNode").parameters({instanceId: NODE, mediaAddress: "127.0.0.1:10000"}).call()
        ec.service.sync().name("org.moqui.impl.VoiceServices.register#SipNode").parameters({instanceId: NODE, sipAddress: "127.0.0.1:5060"}).call()
    })
}
const session = (callSessionId) => sys(() => ec.entity.find(SESSION).condition("callSessionId", callSessionId).one())
const leg = (callLegId) => sys(() => ec.entity.find(LEG).condition("callLegId", callLegId).one())
const transferRow = (callTransferId) => sys(() => ec.entity.find(TRANSFER).condition("callTransferId", callTransferId).one())
const legsOf = (callSessionId, userId) => sys(() => {
    const p = ec.entity.find("moqui.call.CallParticipant").condition({callSessionId, userId}).one()
    return p ? Array.from(ec.entity.find(LEG).condition("callParticipantId", p.callParticipantId).orderBy("offeredDate").list()) : []
})
const lastLeg = (callSessionId, userId) => legsOf(callSessionId, userId).slice(-1)[0] || null
const rev = (callSessionId) => Number(session(callSessionId).revision)
const note = (callSessionId, callLegId, event, extra) => engine("org.moqui.impl.VoiceServices.note#PhoneLeg", Object.assign({callSessionId, callLegId, event}, extra || {}))
/** An inbound call to A's number, answered by A. */
function inbound() {
    const call = engine("org.moqui.impl.VoiceServices.receive#PhoneCall", {toNumber: MINE, fromNumber: CALLER, fromName: "Xfer Caller", instanceId: NODE})
    as(A, "org.moqui.impl.CallServices.answer#Call", {callSessionId: call.callSessionId})
    return {callSessionId: call.callSessionId, phoneLegId: call.callLegId}
}
/** A call A places to DEST, answered there. */
function outbound(phoneNumber) {
    const placed = as(A, "org.moqui.impl.VoiceServices.place#PhoneCall", {phoneNumber: phoneNumber || DEST, fromNumber: MINE})
    note(placed.callSessionId, placed.phoneLegId, "answered")
    return {callSessionId: placed.callSessionId, phoneLegId: placed.phoneLegId}
}
const transfer = (call, targetType, targetId, mode, revision) => as(A, "transfer#Call", {callSessionId: call.callSessionId, targetType, targetId,
    mode: mode || "blind", revision: revision === undefined ? rev(call.callSessionId) : revision})

spec({
    stepwise: true,
    setupSpec() {
        makeUser(ADMIN, "ADMIN")
        for (const userId of [A, B, C]) makeUser(userId)
        clean()
        nodesUp()
        const admin = (name, parameters) => as(ADMIN, "org.moqui.impl.VoiceServices." + name, parameters)
        sys(() => {
            const t = ec.entity.find("moqui.call.VoiceTrunk").condition("voiceTrunkId", TRUNK).one()
            if (!t) {
                const made = ec.entity.makeValue("moqui.call.VoiceTrunk")
                made.setFields({voiceTrunkId: TRUNK, name: "Xfer spec trunk", host: "sip.example.invalid", transportEnumId: "VtrTls",
                    mediaEncryptionEnumId: "VmeSrtp", providerProfileEnumId: "VppGeneric", statusId: "VtsOk", disabled: "N"}, true, null, null)
                made.create()
            }
        })
        admin("store#PhoneNumber", {phoneNumber: MINE, voiceTrunkId: TRUNK, routeTypeEnumId: "PnrUser", routeId: A})
        admin("grant#CallerId", {phoneNumber: MINE, userId: A})
        admin("store#VoiceUserSetting", {userId: A, outboundEnabled: "Y"})
    },
    cleanupSpec() {
        clean()
    }
})

feature("a ringing call is not transferred; a stale revision and a second open transfer are refused; an attended transfer cancels", {
    when() {
        const call = engine("org.moqui.impl.VoiceServices.receive#PhoneCall", {toNumber: MINE, fromNumber: CALLER, instanceId: NODE})
        const ringing = as(A, "transfer#Call", {callSessionId: call.callSessionId, targetType: "user", targetId: B, mode: "blind"})
        as(A, "org.moqui.impl.CallServices.answer#Call", {callSessionId: call.callSessionId})
        const seen = rev(call.callSessionId)
        const stale = transfer({callSessionId: call.callSessionId}, "user", B, "attended", seen - 1)
        const consult = transfer({callSessionId: call.callSessionId}, "user", B, "attended", seen)
        // the second from the same view is stale; from a fresh one, the first is still open
        const again = transfer({callSessionId: call.callSessionId}, "user", C, "attended", seen)
        const fresh = transfer({callSessionId: call.callSessionId}, "user", C, "attended")
        const held = leg(call.callLegId)
        const bOffered = lastLeg(call.callSessionId, B)
        const early = as(A, "complete#CallTransfer", {callTransferId: consult.callTransferId})
        const byB = as(B, "cancel#CallTransfer", {callTransferId: consult.callTransferId})
        as(B, "org.moqui.impl.CallServices.answer#Call", {callSessionId: call.callSessionId})
        const bIn = lastLeg(call.callSessionId, B)
        const canceled = as(A, "cancel#CallTransfer", {callTransferId: consult.callTransferId})
        return {call, ringing, stale, consult, again, fresh, held, bOffered, early, byB, bIn, canceled, after: leg(call.callLegId),
            bAfter: lastLeg(call.callSessionId, B), aAfter: lastLeg(call.callSessionId, A), row: transferRow(consult.callTransferId),
            status: session(call.callSessionId).statusId}
    },
    then({ringing, stale, consult, again, fresh, held, bOffered, early, byB, bIn, canceled, after, bAfter, aAfter, row, status}) {
        ringing.reasonCode == "ringing"
        stale.reasonCode == "stale"
        consult.errors == ""
        consult.call.transfer.kind == "attended"
        consult.call.transfer.status == "consulting"
        again.reasonCode == "stale"
        fresh.reasonCode == "transferring"
        held.heldDate != null
        bOffered.statusId == "ClsOffered"
        early.reasonCode == "notAnswered"
        byB.reasonCode == "notFound"
        bIn.statusId == "ClsAnswered"
        canceled.errors == ""
        after.heldDate == null
        bAfter.statusId == "ClsLeft"
        aAfter.statusId == "ClsAnswered"
        row.statusId == "CtsCanceled"
        status == "CssActive"
    }
})

feature("an attended transfer completes once the target answered: the far end comes off hold with them and the user leaves", {
    when() {
        endAll()
        const call = inbound()
        const consult = transfer(call, "user", B, "attended")
        as(B, "org.moqui.impl.CallServices.answer#Call", {callSessionId: call.callSessionId})
        const done = as(A, "complete#CallTransfer", {callTransferId: consult.callTransferId})
        return {done, far: leg(call.phoneLegId), a: lastLeg(call.callSessionId, A), b: lastLeg(call.callSessionId, B),
            row: transferRow(consult.callTransferId), status: session(call.callSessionId).statusId}
    },
    then({done, far, a, b, row, status}) {
        done.errors == ""
        far.heldDate == null
        far.statusId == "ClsAnswered"
        a.statusId == "ClsLeft"
        a.endReasonEnumId == "CerTransferred"
        b.statusId == "ClsAnswered"
        row.statusId == "CtsCompleted"
        status == "CssActive"
    }
})

feature("blind: the user leaves at once; the target's answer and the far end hanging up race to one outcome; no answer goes to voicemail", {
    when() {
        endAll()
        // the far end hangs up first: the call ends, the transfer is over, the target's late answer finds it ended
        const first = inbound()
        const blind1 = transfer(first, "user", B)
        const aLeft = lastLeg(first.callSessionId, A)
        const waiting = session(first.callSessionId).statusId
        note(first.callSessionId, first.phoneLegId, "ended")
        const late = as(B, "org.moqui.impl.CallServices.answer#Call", {callSessionId: first.callSessionId})
        const firstOut = {session: session(first.callSessionId), row: transferRow(blind1.callTransferId), b: lastLeg(first.callSessionId, B)}
        // the target answers first: the transfer completes; then the far end hangs up and the call ends as any call does
        const second = inbound()
        const blind2 = transfer(second, "user", B)
        const won = as(B, "org.moqui.impl.CallServices.answer#Call", {callSessionId: second.callSessionId})
        const completed = transferRow(blind2.callTransferId)
        const tooLate = engine("expire#CallTransfer", {callTransferId: blind2.callTransferId})
        note(second.callSessionId, second.phoneLegId, "ended")
        const secondOut = session(second.callSessionId)
        // nobody answers: the timer's expire, and the far end goes to the target's voicemail
        const third = inbound()
        const blind3 = transfer(third, "user", B)
        const expired = engine("expire#CallTransfer", {callTransferId: blind3.callTransferId})
        const bLog = sys(() => ec.entity.find("moqui.call.CallUserLog").condition({callSessionId: third.callSessionId, userId: B}).one())
        const deposit = engine("org.moqui.impl.VoicemailServices.begin#Voicemail", {callSessionId: third.callSessionId, callLegId: third.phoneLegId,
            reason: "busy", stepToken: expired.answer.stepToken})
        return {blind1, aLeft, waiting, late, firstOut, won, completed, tooLate, secondOut, expired, bLog, deposit, third: transferRow(blind3.callTransferId)}
    },
    then({blind1, aLeft, waiting, late, firstOut, won, completed, tooLate, secondOut, expired, bLog, deposit, third}) {
        blind1.errors == ""
        aLeft.statusId == "ClsLeft"
        aLeft.endReasonEnumId == "CerTransferred"
        waiting == "CssActive"
        late.reasonCode == "callEnded"
        firstOut.session.statusId == "CssEnded"
        firstOut.row.statusId == "CtsCanceled"
        firstOut.b.statusId != "ClsAnswered"
        won.errors == ""
        completed.statusId == "CtsCompleted"
        tooLate.answer.mode == "done"
        secondOut.statusId == "CssEnded"
        expired.answer.mode == "voicemail"
        third.statusId == "CtsFailed"
        bLog.outcomeEnumId == "CuoMissed"
        deposit.mode == "deposit"
    }
})

feature("a transfer to an international number is refused under the fraud policy; the same to a domestic number is dialed", {
    when() {
        endAll()
        const call = outbound()
        const abroad = transfer(call, "number", ABROAD, "attended")
        const premium = transfer(call, "number", "+19005550100", "attended")
        const emergency = transfer(call, "number", "911", "attended")
        const home = transfer(call, "number", "+15550199988", "attended")
        const target = home.callTransferId ? leg(transferRow(home.callTransferId).targetLegId) : null
        const canceled = as(A, "cancel#CallTransfer", {callTransferId: home.callTransferId})
        return {abroad, premium, emergency, home, target, canceled, targetAfter: target ? leg(target.callLegId) : null}
    },
    then({abroad, premium, emergency, home, target, canceled, targetAfter}) {
        abroad.reasonCode == "internationalNotEnabled"
        premium.reasonCode == "premiumNotEnabled"
        emergency.reasonCode == "emergency"
        home.errors == ""
        target.legTypeEnumId == "CltPstn"
        target.phoneNumber == "+15550199988"
        canceled.errors == ""
        targetAfter.statusId == "ClsCanceled"
    }
})

feature("a number transferred to hears the recording notice and acknowledges it by pressing 1", {
    when() {
        endAll()
        setProperty("call_recording_enabled", "true")
        setProperty("call_recording_ack_required", "true")
        const call = outbound()
        const started = as(A, "org.moqui.impl.CallRecordingServices.start#Recording", {callSessionId: call.callSessionId})
        const consult = transfer(call, "number", "+15550199977", "attended")
        const targetLegId = transferRow(consult.callTransferId).targetLegId
        note(call.callSessionId, targetLegId, "answered")
        const consent = () => sys(() => ec.entity.find("moqui.call.CallRecordingConsent").condition({callRecordingId: started.callRecordingId,
            callLegId: targetLegId}).one())
        const noticed = consent()
        note(call.callSessionId, targetLegId, "dtmf", {digits: "1"})
        const acked = consent()
        const done = as(A, "complete#CallTransfer", {callTransferId: consult.callTransferId})
        setProperty("call_recording_ack_required", "false")
        setProperty("call_recording_enabled", "false")
        return {started, noticed, acked, done, a: lastLeg(call.callSessionId, A)}
    },
    then({started, noticed, acked, done, a}) {
        started.errors == ""
        noticed.noticeMethodEnumId == "CnmVoice"
        noticed.ackRequired == "Y"
        noticed.ackDate == null
        acked.ackDate != null
        acked.ackMethodEnumId == "CamDtmf"
        done.errors == ""
        a.statusId == "ClsLeft"
    }
})

feature("a person is added; a number is not added to an encrypted call; calls on one media node merge into a meeting, on two they do not", {
    when() {
        endAll()
        const first = inbound()
        const added = as(A, "add#CallParticipant", {callSessionId: first.callSessionId, userId: C, revision: rev(first.callSessionId)})
        const cRinging = lastLeg(first.callSessionId, C)
        const addedAgain = as(A, "add#CallParticipant", {callSessionId: first.callSessionId, userId: C})
        const abroad = as(A, "add#CallParticipant", {callSessionId: first.callSessionId, phoneNumber: ABROAD})
        sys(() => {
            const s = ec.entity.find(SESSION).condition("callSessionId", first.callSessionId).one()
            s.set("encryptionMode", "e2ee")
            s.update()
        })
        const encrypted = as(A, "add#CallParticipant", {callSessionId: first.callSessionId, phoneNumber: "+15550199966"})
        sys(() => {
            const s = ec.entity.find(SESSION).condition("callSessionId", first.callSessionId).one()
            s.set("encryptionMode", "dtls-srtp")
            s.update()
        })
        // A puts the first call on hold and places a second; then merges them
        as(A, "org.moqui.impl.VoiceServices.hold#PhoneCall", {callSessionId: first.callSessionId})
        const heldFirst = leg(first.phoneLegId)
        const second = outbound()
        const merged = as(A, "merge#Calls", {callSessionId: first.callSessionId, otherCallSessionId: second.callSessionId,
            revision: rev(first.callSessionId)})
        const moved = leg(second.phoneLegId)
        // a third call on another media node is not merged
        const third = outbound("+15550123488")
        sys(() => {
            const s = ec.entity.find(SESSION).condition("callSessionId", third.callSessionId).one()
            s.set("mediaNodeId", NODE2)
            s.update()
        })
        const otherNode = as(A, "merge#Calls", {callSessionId: first.callSessionId, otherCallSessionId: third.callSessionId})
        const meeting = as(A, "convert#CallToMeeting", {callSessionId: third.callSessionId})
        return {added, cRinging, addedAgain, abroad, encrypted, heldFirst, merged, moved, otherNode, meeting,
            firstAfter: session(first.callSessionId), secondAfter: session(second.callSessionId), farFirst: leg(first.phoneLegId),
            aSecond: lastLeg(second.callSessionId, A)}
    },
    then({added, cRinging, addedAgain, abroad, encrypted, heldFirst, merged, moved, otherNode, meeting, firstAfter, secondAfter, farFirst, aSecond}) {
        added.errors == ""
        cRinging.statusId == "ClsOffered"
        addedAgain.reasonCode == "alreadyIn"
        abroad.reasonCode == "internationalNotEnabled"
        encrypted.reasonCode == "encrypted"
        heldFirst.heldDate != null
        merged.errors == ""
        merged.movedCount >= 1
        merged.call.kind == "meeting"
        moved.callSessionId == firstAfter.callSessionId
        moved.statusId == "ClsAnswered"
        firstAfter.kindEnumId == "CskMeeting"
        secondAfter.statusId == "CssEnded"
        secondAfter.endReasonEnumId == "CerMerged"
        farFirst.heldDate == null
        aSecond.statusId == "ClsLeft"
        otherNode.reasonCode == "differentNodes"
        meeting.call.kind == "meeting"
    }
})
