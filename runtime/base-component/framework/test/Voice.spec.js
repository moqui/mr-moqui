// Voice (org.moqui.impl.VoiceServices and Voice.js; framework/entity/VoiceEntities.xml; plans/Comms-Framework.md §5.2, V1):
// the admin services only for VoiceAdmin, insecure trunks only with a profile that allows them, caller ID from the user's
// grants, international and premium off until the deployment and the user allow them, the limits at once and per day,
// failed digest attempts blocked per source without locking the account, emergency calls refused without readiness and
// routed with it whatever the limits, calls coming in and the phone leg's events, a phone leg's recording and captions
// notices with its acknowledgement by pressing 1, and the capabilities. No SIP engine
// runs here: fake SipNode and CallMediaNode rows stand in for one, and send#SipControl to them is only logged. Every
// refusal has its positive case with the same parameters.
// Run: ./moq test framework --spec Voice
const ADMIN = "VOICE_SPEC_ADMIN"
const A = "VOICE_SPEC_A"
const B = "VOICE_SPEC_B"
const NODE = "voice-spec-node"
const MINE = "+15550100001"
const OTHER = "+15550100002"
const DEST = "+15550123456"
const ABROAD = "+442071234567"
const SESSION = "moqui.call.CallSession"
const LEG = "moqui.call.CallLeg"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function username(userId) {
    return userId.toLowerCase().replace(/_/g, ".")
}
function as(userId, name, parameters) {
    if (userId) ec.user.internalLoginUser(username(userId))
    let out = {}
    let thrown = ""
    try {
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : "org.moqui.impl.VoiceServices." + name).parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const admin = (name, parameters) => as(ADMIN, name, parameters)
function makeUser(userId, groupId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "Voice " + userId.slice(-1), disabled: "N"}, true, null, null)
            account.create()
        }
        if (groupId && ec.entity.find("moqui.security.UserGroupMember").condition({userId, userGroupId: groupId}).count() === 0) {
            const member = ec.entity.makeValue("moqui.security.UserGroupMember")
            member.setFields({userId, userGroupId: groupId, fromDate: new Date(Date.now() - 60000)}, true, null, null)
            member.create()
        }
    })
}
function nodesUp() {
    sys(() => {
        ec.service.sync().name("org.moqui.impl.CallServices.register#MediaNode").parameters({instanceId: NODE, mediaAddress: "127.0.0.1:10000"}).call()
        ec.service.sync().name("org.moqui.impl.VoiceServices.register#SipNode").parameters({instanceId: NODE, sipAddress: "127.0.0.1:5060"}).call()
    })
}
function endAll() {
    sys(() => {
        for (const s of ec.entity.find(SESSION).condition("kindEnumId", "CskPhone").condition("statusId", "!=", "CssEnded").list()) {
            s.set("statusId", "CssEnded")
            s.update()
        }
    })
}
function clean() {
    endAll()
    sys(() => {
        for (const entity of ["PhoneNumberUser", "PhoneNumber", "VoiceUserSetting", "EmergencyLocation", "VoiceTrunk", "VoicePolicy", "VoiceAuthFailure", "SipNode"]) {
            ec.entity.find("moqui.call." + entity).deleteAll()
        }
        ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", NODE).deleteAll()
        for (const r of ec.entity.find("moqui.call.CallRecording").condition("ownerUserId", "in", [A, B]).list()) {
            for (const entity of ["CallRecordingConsent", "CallRecordingTrack", "CallRecordingSegment"]) {
                ec.entity.find("moqui.call." + entity).condition("callRecordingId", r.callRecordingId).deleteAll()
            }
            r.delete()
        }
        // the calls these users placed today, so the daily limit counts this run's only
        for (const s of ec.entity.find(SESSION).condition("kindEnumId", "CskPhone").condition("startedByUserId", "in", [A, B]).list()) {
            s.set("startedDate", new Date(Date.now() - 3 * 86400000))
            s.update()
        }
    })
}
const policy = fields => admin("update#VoicePolicy", fields)
const session = id => sys(() => ec.entity.find(SESSION).condition("callSessionId", id).one())
const legs = id => sys(() => Array.from(ec.entity.find(LEG).condition("callSessionId", id).orderBy("offeredDate").list()))
const placeA = (parameters) => as(A, "place#PhoneCall", Object.assign({phoneNumber: DEST, fromNumber: MINE}, parameters || {}))

let trunkId = null

spec({
    stepwise: true,
    setupSpec() {
        makeUser(ADMIN, "ADMIN")
        makeUser(A)
        makeUser(B)
        clean()
    },
    cleanupSpec() {
        clean()
    }
})

feature("the admin services are a voice admin's: the same trunk is refused for another user", {
    when() {
        const trunk = {name: "Spec trunk", host: "sip.example.test", username: "spec", password: "secret-1", realm: "sip.example.test", algorithms: "SHA-256,MD5"}
        const refused = as(A, "store#VoiceTrunk", trunk)
        const made = admin("store#VoiceTrunk", trunk)
        trunkId = made.voiceTrunkId
        const listRefused = as(A, "find#VoiceTrunks")
        const listed = admin("find#VoiceTrunks")
        const row = sys(() => ec.entity.find("moqui.call.VoiceTrunk").condition("voiceTrunkId", trunkId).one())
        return {refused, made, listRefused, listed, row}
    },
    then({refused, made, listRefused, listed, row}) {
        refused.reasonCode == "notAllowed"
        made.errors == ""
        made.voiceTrunkId != null
        listRefused.reasonCode == "notAllowed"
        listed.trunkList.length == 1
        listed.trunkList[0].hasPassword == true
        listed.trunkList[0].password === undefined
        listed.trunkList[0].transportEnumId == "VtrTls"
        listed.trunkList[0].mediaEncryptionEnumId == "VmeSrtp"
        JSON.parse(row.authJson).password == "secret-1"
    }
})

feature("UDP or plain RTP needs a profile that allows it, never a silent downgrade", {
    when() {
        const plain = {name: "Office PBX", host: "10.0.0.5", transportEnumId: "VtrUdp", mediaEncryptionEnumId: "VmePlain"}
        const generic = admin("store#VoiceTrunk", Object.assign({providerProfileEnumId: "VppGeneric"}, plain))
        const lan = admin("store#VoiceTrunk", Object.assign({providerProfileEnumId: "VppPlainLan"}, plain))
        if (lan.voiceTrunkId) admin("delete#VoiceTrunk", {voiceTrunkId: lan.voiceTrunkId})
        return {generic, lan}
    },
    then({generic, lan}) {
        generic.reasonCode == "insecureNotAllowed"
        lan.errors == ""
        lan.voiceTrunkId != null
    }
})

feature("only a PBX trunk's credentials sign a call in; inbound addresses wider than a /8 or a /32 are refused", {
    when() {
        const pbx = admin("store#VoiceTrunk", {name: "Office PBX", host: "10.0.0.6", transportEnumId: "VtrUdp", mediaEncryptionEnumId: "VmePlain",
            providerProfileEnumId: "VppPlainLan", username: "pbx", password: "secret-2"})
        const config = sys(() => ec.service.sync().name("org.moqui.impl.VoiceServices.get#EngineConfig").call())
        const digestOf = id => config.trunkList.find(t => t.voiceTrunkId == id).inboundDigest
        const wide = ["0.0.0.0/0", "::/0", "10.0.0.0/7", "2001:db8::/31", "sip.example.test", "203.0.113.0/33"]
            .map(inboundAddresses => admin("store#VoiceTrunk", {voiceTrunkId: trunkId, inboundAddresses}))
        const narrow = admin("store#VoiceTrunk", {voiceTrunkId: trunkId, inboundAddresses: "203.0.113.0/24, 10.0.0.0/8, 2001:db8::/32, 198.51.100.7"})
        const row = sys(() => ec.entity.find("moqui.call.VoiceTrunk").condition("voiceTrunkId", trunkId).one())
        if (pbx.voiceTrunkId) admin("delete#VoiceTrunk", {voiceTrunkId: pbx.voiceTrunkId})
        admin("store#VoiceTrunk", {voiceTrunkId: trunkId, inboundAddresses: ""})
        return {pbxDigest: digestOf(pbx.voiceTrunkId), providerDigest: digestOf(trunkId), wide, narrow, row}
    },
    then({pbxDigest, providerDigest, wide, narrow, row}) {
        pbxDigest == true
        providerDigest == false
        wide.every(r => r.reasonCode == "badInboundAddress")
        narrow.errors == ""
        row.inboundAddresses == "203.0.113.0/24,10.0.0.0/8,2001:db8::/32,198.51.100.7"
    }
})

feature("a call goes out from a granted number; the same call from a number not granted is refused", {
    when() {
        nodesUp()
        admin("store#PhoneNumber", {phoneNumber: MINE, voiceTrunkId: trunkId, routeTypeEnumId: "PnrUser", routeId: A})
        admin("store#PhoneNumber", {phoneNumber: OTHER, voiceTrunkId: trunkId, routeTypeEnumId: "PnrUser", routeId: B})
        const notEnabled = placeA()
        admin("store#VoiceUserSetting", {userId: A, outboundEnabled: "Y"})
        const notGranted = placeA()
        admin("grant#CallerId", {phoneNumber: MINE, userId: A})
        const placed = placeA({idempotencyKey: "voice-spec-1"})
        const again = placeA({idempotencyKey: "voice-spec-1"})
        endAll()
        const other = placeA({fromNumber: OTHER})
        const mine = as(A, "find#MyCallerIds")
        return {notEnabled, notGranted, placed, again, other, mine, row: session(placed.callSessionId), legs: legs(placed.callSessionId)}
    },
    then({notEnabled, notGranted, placed, again, other, mine, row, legs}) {
        notEnabled.reasonCode == "notAllowed"
        notGranted.reasonCode == "callerIdNotAllowed"
        placed.errors == ""
        placed.fromNumber == MINE
        placed.voiceTrunkId == trunkId
        placed.destinationClass == "domestic"
        row.kindEnumId == "CskPhone"
        row.directionEnumId == "CdrOutbound"
        row.mediaNodeId == NODE
        legs.length == 2
        legs.find(l => l.legTypeEnumId == "CltPstn").phoneNumber == DEST
        legs.find(l => l.legTypeEnumId == "CltPstn").statusId == "ClsOffered"
        placed.call.participants.some(p => p.identity == "phone")
        again.repeated == true
        again.callSessionId == placed.callSessionId
        other.reasonCode == "callerIdNotAllowed"
        mine.numberList.map(n => n.phoneNumber).join() == MINE
    }
})

feature("international calls are refused until the deployment and the user both allow them", {
    when() {
        endAll()
        const off = placeA({phoneNumber: ABROAD})
        policy({internationalEnabled: "Y"})
        const deploymentOnly = placeA({phoneNumber: ABROAD})
        admin("store#VoiceUserSetting", {userId: A, internationalEnabled: "Y"})
        const both = placeA({phoneNumber: ABROAD})
        endAll()
        policy({internationalEnabled: "N"})
        const offAgain = placeA({phoneNumber: ABROAD})
        const premium = placeA({phoneNumber: "+19005550100"})
        // Jamaica dials like a domestic number from country code 1, but it is another country
        const caribbean = placeA({phoneNumber: "1 876 555 0100"})
        return {off, deploymentOnly, both, offAgain, premium, caribbean}
    },
    then({off, deploymentOnly, both, offAgain, premium, caribbean}) {
        off.reasonCode == "internationalNotEnabled"
        deploymentOnly.reasonCode == "internationalNotEnabled"
        both.errors == ""
        both.destinationClass == "international"
        offAgain.reasonCode == "internationalNotEnabled"
        premium.reasonCode == "premiumNotEnabled"
        caribbean.reasonCode == "internationalNotEnabled"
    }
})

feature("the limits at once and per day", {
    when() {
        endAll()
        policy({maxConcurrentPerUser: 1})
        const first = placeA()
        const second = placeA()
        endAll()
        const afterEnd = placeA()
        endAll()
        const today = sys(() => ec.entity.find(SESSION).condition({kindEnumId: "CskPhone", startedByUserId: A})
            .condition("startedDate", ">=", new Date(Date.now() - 86400000)).count())
        policy({maxCallsPerUserPerDay: today})
        const daily = placeA()
        policy({maxConcurrentPerUser: 2, maxCallsPerUserPerDay: 100})
        return {first, second, afterEnd, daily}
    },
    then({first, second, afterEnd, daily}) {
        first.errors == ""
        second.reasonCode == "tooManyCalls"
        afterEnd.errors == ""
        daily.reasonCode == "dailyLimit"
    }
})

// six places at once, each in its own transaction on its own thread: under the one limits lock (Voice.js lockLimits)
// only as many as the limit go out, the rest are refused, and a refused one does not count
function placeAtOnce(n) {
    return Array.from(parallel(n, (threadNum, args) => {
        ec.user.internalLoginUser(args.user)
        const wasDisabled = ec.artifactExecution.disableAuthz()
        try {
            const out = ec.service.sync().name("org.moqui.impl.VoiceServices.place#PhoneCall").requireNewTransaction(true)
                .parameters({phoneNumber: args.dest, fromNumber: args.from}).call() || {}
            const errors = ec.message.hasError() ? ec.message.errorsString : ""
            ec.message.clearErrors()
            return {placed: !!out.callSessionId, reasonCode: out.reasonCode || null, errors}
        } catch (err) {
            return {placed: false, reasonCode: null, errors: String(err.message || err)}
        } finally {
            if (!wasDisabled) ec.artifactExecution.enableAuthz()
        }
    }, {user: username(A), dest: DEST, from: MINE}, {transaction: false}))
}

feature("calls placed at once cannot pass the limits at once and per day together", {
    when() {
        endAll()
        policy({maxConcurrentPerUser: 2, maxCallsPerUserPerDay: 100})
        const atOnce = placeAtOnce(6)
        endAll()
        const today = sys(() => ec.entity.find(SESSION).condition({kindEnumId: "CskPhone", startedByUserId: A})
            .condition("startedDate", ">=", new Date(Date.now() - 86400000)).count())
        policy({maxConcurrentPerUser: 10, maxCallsPerUserPerDay: today + 2})
        const perDay = placeAtOnce(6)
        endAll()
        const after = sys(() => ec.entity.find(SESSION).condition({kindEnumId: "CskPhone", startedByUserId: A})
            .condition("startedDate", ">=", new Date(Date.now() - 86400000)).count())
        policy({maxConcurrentPerUser: 2, maxCallsPerUserPerDay: 100})
        return {atOnce, perDay, today, after}
    },
    then({atOnce, perDay, today, after}) {
        atOnce.filter(r => r.placed).length == 2
        atOnce.filter(r => !r.placed).every(r => r.reasonCode == "tooManyCalls")
        perDay.filter(r => r.placed).length == 2
        perDay.filter(r => !r.placed).every(r => r.reasonCode == "dailyLimit")
        after == today + 2
    }
})

feature("failed digest attempts block their source, not the account", {
    when() {
        policy({authFailureLimit: 3})
        const before = sys(() => ec.entity.find("moqui.security.UserAccount").condition("userId", A).one())
        const tries = [1, 2, 3].map(() => as(null, "record#AuthFailure", {sourceAddress: "203.0.113.5", username: username(A)}))
        const elsewhere = as(null, "record#AuthFailure", {sourceAddress: "203.0.113.6", username: username(A)})
        // the engine reports a source only when it blocked it: blocked at once
        const engineBlocked = as(null, "record#AuthFailure", {sourceAddress: "203.0.113.7", username: username(A), blocked: true})
        const after = sys(() => ec.entity.find("moqui.security.UserAccount").condition("userId", A).one())
        const config = as(null, "get#EngineConfig")
        const blocks = admin("find#AuthBlocks")
        const cleared = admin("clear#AuthBlock", {sourceAddress: "203.0.113.5"})
        const alerts = sys(() => ec.entity.find("moqui.security.user.NotificationMessage").condition("topic", "VoiceAlert").count())
        return {tries, elsewhere, engineBlocked, before, after, config, blocks, cleared, alerts}
    },
    then({tries, elsewhere, engineBlocked, before, after, config, blocks, cleared, alerts}) {
        tries[0].blocked == false
        tries[1].blocked == false
        tries[2].blocked == true
        tries[2].blockedUntil > Date.now()
        elsewhere.blocked == false
        engineBlocked.blocked == true
        after.disabled == before.disabled
        after.disabled != "Y"
        String(after.successiveFailedLogins || 0) == String(before.successiveFailedLogins || 0)
        config.blockedSourceList.map(b => b.sourceAddress).sort().join() == "203.0.113.5,203.0.113.7"
        config.trunkList.length == 1
        config.trunkList[0].password == "secret-1"
        config.trunkList[0].algorithms.join() == "SHA-256,MD5"
        config.policy.internationalEnabled == false
        blocks.blockList.find(b => b.sourceAddress == "203.0.113.5").blocked == true
        cleared.errors == ""
        alerts > 0
    }
})

feature("an emergency call is refused with what is missing, then routed through the emergency trunk whatever the limits", {
    when() {
        endAll()
        const missing = []
        missing.push(placeA({phoneNumber: "911", fromNumber: null}))
        const emergencyTrunk = admin("store#VoiceTrunk", {name: "Emergency trunk", host: "e911.example.test", emergencyCapable: "Y"}).voiceTrunkId
        missing.push(placeA({phoneNumber: "911", fromNumber: null}))
        const location = admin("store#EmergencyLocation", {name: "Head office", address1: "1 Main St", city: "Springfield", postalCode: "12345"}).emergencyLocationId
        admin("store#VoiceUserSetting", {userId: A, emergencyLocationId: location})
        missing.push(placeA({phoneNumber: "911", fromNumber: null}))
        const capsBefore = as(A, "get#VoiceCapabilities")
        admin("store#EmergencyLocation", {emergencyLocationId: location, validationStatusId: "ElvValidated", callbackNumber: "555 010 0009",
            notifyEmailAddress: "front.desk@example.test", providerReference: "spec-addr-1"})
        // limits that refuse an ordinary call: one at once, and one going
        policy({maxConcurrentPerUser: 1})
        const ordinary = placeA()
        const blocked = placeA()
        const routed = placeA({phoneNumber: "911", fromNumber: null})
        const capsAfter = as(A, "get#VoiceCapabilities")
        const email = sys(() => ec.entity.find("moqui.basic.email.EmailMessage").condition("toAddresses", "front.desk@example.test").count())
        policy({maxConcurrentPerUser: 2})
        return {missing, emergencyTrunk, ordinary, blocked, routed, capsBefore, capsAfter, email, legs: legs(routed.callSessionId)}
    },
    then({missing, emergencyTrunk, ordinary, blocked, routed, capsBefore, capsAfter, email, legs}) {
        missing.every(m => m.reasonCode == "emergencyNotReady")
        missing[0].missing.includes("noEmergencyTrunk")
        missing[0].errors.includes("no trunk is set up for emergency calls")
        missing[1].missing.join() == "noLocation"
        missing[2].missing.join() == "locationNotValidated,noCallbackNumber,noNotification"
        capsBefore.emergencyReady == false
        capsBefore.emergencyReasonCode == "locationNotValidated"
        ordinary.errors == ""
        blocked.reasonCode == "tooManyCalls"
        routed.errors == ""
        routed.destinationClass == "emergency"
        routed.voiceTrunkId == emergencyTrunk
        routed.fromNumber == "+15550100009"
        legs.find(l => l.legTypeEnumId == "CltPstn").phoneNumber == "911"
        capsAfter.emergencyReady == true
        email == 1
    }
})

feature("a call coming in rings the number's user; the phone's events move the call", {
    when() {
        endAll()
        const unknown = as(null, "receive#PhoneCall", {voiceTrunkId: trunkId, toNumber: "+15550199999", fromNumber: "+15557770000", instanceId: NODE})
        const wrongNode = as(null, "receive#PhoneCall", {voiceTrunkId: trunkId, toNumber: MINE, fromNumber: "+15557770000", instanceId: "elsewhere"})
        const received = as(null, "receive#PhoneCall", {voiceTrunkId: trunkId, toNumber: MINE, fromNumber: "+15557770000", fromName: "Pat Caller", instanceId: NODE})
        const ringing = as(A, "org.moqui.impl.CallServices.find#ActiveCalls")
        const answered = as(A, "org.moqui.impl.CallServices.answer#Call", {callSessionId: received.callSessionId})
        const hungUp = as(null, "note#PhoneLeg", {callSessionId: received.callSessionId, callLegId: received.callLegId, event: "ended"})
        const repeat = as(null, "note#PhoneLeg", {callSessionId: received.callSessionId, callLegId: received.callLegId, event: "ended"})
        return {unknown, wrongNode, received, ringing, answered, hungUp, repeat, row: session(received.callSessionId)}
    },
    then({unknown, wrongNode, received, ringing, answered, hungUp, repeat, row}) {
        unknown.reasonCode == "notFound"
        wrongNode.reasonCode == "voiceUnavailable"
        received.errors == ""
        received.callLegId != null
        ringing.callList.some(c => c.callSessionId == received.callSessionId && c.myLegStatus == "offered")
        ringing.callList.find(c => c.callSessionId == received.callSessionId).participants.some(p => p.displayName == "Pat Caller (+15557770000)")
        answered.errors == ""
        answered.call.status == "active"
        hungUp.errors == ""
        repeat.repeated == true
        row.statusId == "CssEnded"
        row.directionEnumId == "CdrInbound"
    }
})

feature("an outbound call is active when the number answers, and ends when the user leaves", {
    when() {
        endAll()
        const placed = placeA()
        const answered = as(null, "note#PhoneLeg", {callSessionId: placed.callSessionId, callLegId: placed.phoneLegId, event: "answered"})
        const active = session(placed.callSessionId).statusId
        const dtmf = as(A, "send#PhoneDtmf", {callSessionId: placed.callSessionId, digits: "12#"})
        const badDtmf = as(A, "send#PhoneDtmf", {callSessionId: placed.callSessionId, digits: "1x"})
        const dtmfByB = as(B, "send#PhoneDtmf", {callSessionId: placed.callSessionId, digits: "12#"})
        const left = as(A, "org.moqui.impl.CallServices.leave#Call", {callSessionId: placed.callSessionId})
        const busy = placeA()
        const failed = as(null, "note#PhoneLeg", {callSessionId: busy.callSessionId, callLegId: busy.phoneLegId, event: "busy", sipCode: 486})
        return {answered, active, dtmf, badDtmf, dtmfByB, left, row: session(placed.callSessionId), busyRow: session(busy.callSessionId), failed}
    },
    then({answered, active, dtmf, badDtmf, dtmfByB, left, row, busyRow, failed}) {
        answered.errors == ""
        active == "CssActive"
        dtmf.sent == true
        badDtmf.reasonCode == "badDigits"
        dtmfByB.reasonCode == "notFound"
        left.errors == ""
        row.statusId == "CssEnded"
        failed.errors == ""
        busyRow.statusId == "CssEnded"
        busyRow.endReasonEnumId == "CerDeclined"
    }
})

feature("a phone leg hears the recording notice and acknowledges it by pressing 1; another key is the call's own", {
    when() {
        endAll()
        setProperty("call_recording_enabled", "true")
        setProperty("call_recording_ack_required", "true")
        const placed = placeA()
        const callSessionId = placed.callSessionId
        const phone = {callSessionId, callLegId: placed.phoneLegId}
        as(null, "note#PhoneLeg", Object.assign({event: "answered"}, phone))
        const started = as(A, "org.moqui.impl.CallRecordingServices.start#Recording", {callSessionId})
        const recId = started.callRecordingId
        const consent = () => sys(() => ec.entity.find("moqui.call.CallRecordingConsent").condition({callRecordingId: recId, callLegId: placed.phoneLegId}).one())
        const recording = () => sys(() => ec.entity.find("moqui.call.CallRecording").condition("callRecordingId", recId).one())
        const noticed = consent()
        as(null, "note#PhoneLeg", Object.assign({event: "dtmf", digits: "2"}, phone))
        const otherKey = {consent: consent(), recording: recording()}
        const pressed = as(null, "note#PhoneLeg", Object.assign({event: "dtmf", digits: "1"}, phone))
        const acked = {consent: consent(), recording: recording()}
        const again = as(null, "org.moqui.impl.CallRecordingServices.ack#LegRecordingNotice", phone)
        const prompted = as(null, "org.moqui.impl.CallRecordingServices.notify#LegRecording", {callRecordingId: recId, callLegId: placed.phoneLegId, ackRequired: true})
        const webPrompt = as(null, "org.moqui.impl.CallRecordingServices.notify#LegRecording", {callRecordingId: recId, callLegId: placed.callLegId})
        const captions = as(null, "notify#PhoneLeg", {callLegId: placed.phoneLegId, notice: "captions"})
        const captionsToWeb = as(null, "notify#PhoneLeg", {callLegId: placed.callLegId, notice: "captions"})
        const badNotice = as(null, "notify#PhoneLeg", {callLegId: placed.phoneLegId, notice: "beep"})
        // withdrawing by phone is hanging up: the leg is gone, and with it the call and its recording
        as(null, "note#PhoneLeg", Object.assign({event: "ended"}, phone))
        return {started, noticed, otherKey, pressed, acked, again, prompted, webPrompt, captions, captionsToWeb, badNotice, ended: recording()}
    },
    then({started, noticed, otherKey, pressed, acked, again, prompted, webPrompt, captions, captionsToWeb, badNotice, ended}) {
        started.errors == ""
        started.recording.state == "paused"
        started.recording.pauseReason == "ack"
        noticed.noticeMethodEnumId == "CnmVoice"
        noticed.ackRequired == "Y"
        noticed.ackDate == null
        otherKey.consent.ackDate == null
        otherKey.recording.statusId == "CrsPaused"
        pressed.errors == ""
        acked.consent.ackDate != null
        acked.consent.ackMethodEnumId == "CamDtmf"
        acked.recording.statusId == "CrsRecording"
        again.acknowledged == false
        prompted.notified == true
        webPrompt.notified == false
        captions.notified == true
        captionsToWeb.notified == false
        badNotice.reasonCode == "badNotice"
        ended.statusId == "CrsStopped"
    }
})

feature("the capabilities say supported, configured and active apart", {
    when() {
        const caps = as(A, "get#VoiceCapabilities")
        sys(() => ec.entity.find("moqui.call.SipNode").deleteAll())
        const noNode = as(A, "get#VoiceCapabilities")
        nodesUp()
        return {caps, noNode, built: ec.factory.hasFeature("sip")}
    },
    then({caps, noNode, built}) {
        caps.supported == built
        caps.active == true
        caps.reasonCode == null
        noNode.active == false
        noNode.reasonCode == (built ? (String(System.getProperty("sip_port") || System.getProperty("sip_tls_port") || "").trim() ? "noSipServer" : "notConfigured") : "notBuilt")
    }
})
