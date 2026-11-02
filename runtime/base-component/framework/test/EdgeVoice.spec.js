// SIP on an edge node, the local node's side (plans/Edge-Node.md §5.4 "Voice (SIP)", decision 4, E8): the edge node's
// SipNode row kept by the link and never another's, the delegate user _edge_ limited to calls and the node of its edge
// node, each number's fallback for while this node is away, the registrar's digest checked here so the verifiers never
// leave, a message the edge node queued stored in the number's box with its notices, and a saved message played only for
// the call that gave its box's PIN. No edge link runs here: fake SipNode and CallMediaNode rows stand in for the edge node.
// Run: ./moq test framework --spec EdgeVoice
const ADMIN = "EDGE_VOICE_SPEC_ADMIN"
const A = "EDGE_VOICE_SPEC_A"
const EDGE_NODE = "edge-voice-spec-edge"
const OTHER_NODE = "edge-voice-spec-other"
const USER_NUMBER = "+15550160001"
const GROUP_NUMBER = "+15550160002"
const MENU_NUMBER = "+15550160003"
const MOBILE = "+15550169999"
const SESSION = "moqui.call.CallSession"
const BOX = "moqui.call.VoicemailBox"
const CONTENT = "dbresource://moqui/edge/content/in/edge-voice-spec/"
const EDGE_REALM = "edge-voice-spec.example.test"

/** A nonce as the edge node's registrar makes them (moqui-sip NonceBook): base64url of its time and 24 more bytes. */
function edgeNonce(secondsAgo, fill) {
    const bytes = []
    let time = Math.floor(Date.now() / 1000) - secondsAgo
    const high = []
    for (let i = 0; i < 8; i++) {
        high.unshift(time % 256)
        time = Math.floor(time / 256)
    }
    bytes.push(...high)
    for (let i = 0; i < 24; i++) bytes.push(fill)
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
    let out = ""
    for (let i = 0; i < bytes.length; i += 3) {
        const n = (bytes[i] << 16) | ((bytes[i + 1] || 0) << 8) | (bytes[i + 2] || 0)
        const chars = [n >> 18, (n >> 12) & 63, (n >> 6) & 63, n & 63].map((c) => alphabet[c])
        out += chars.slice(0, i + 2 < bytes.length ? 4 : i + 1 < bytes.length ? 3 : 2).join("")
    }
    return out
}

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
/** A service as userId (`_edge_` as the edge link's delegate user), with authorization off, its errors returned. */
function as(userId, name, parameters) {
    if (userId) ec.user.internalLoginUser(userId === "_edge_" ? "_edge_" : username(userId))
    let out = {}
    let thrown = ""
    try {
        out = sys(() => ec.service.sync().name(name).parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
function makeUser(userId, groupId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "Edge " + userId.slice(-1), disabled: "N"}, true, null, null)
            account.create()
        }
        if (groupId && ec.entity.find("moqui.security.UserGroupMember").condition({userId, userGroupId: groupId}).count() === 0) {
            const member = ec.entity.makeValue("moqui.security.UserGroupMember")
            member.setFields({userId, userGroupId: groupId, fromDate: new Date(Date.now() - 60000)}, true, null, null)
            member.create()
        }
    })
}
function make(entity, fields) {
    sys(() => {
        const value = ec.entity.makeValue(entity)
        value.setFields(fields, true, null, null)
        value.createOrUpdate()
    })
}
function nodesUp() {
    sys(() => {
        call("org.moqui.impl.CallServices.register#MediaNode", {instanceId: EDGE_NODE, mediaAddress: "203.0.113.5:10000", edgeLinked: true})
        call("org.moqui.impl.VoiceServices.register#SipNode", {instanceId: EDGE_NODE, sipAddress: "203.0.113.5:5060", edgeLinked: true,
            sipDomain: EDGE_REALM, registrarEnabled: true})
        call("org.moqui.impl.CallServices.register#MediaNode", {instanceId: OTHER_NODE, mediaAddress: "127.0.0.1:10000"})
        call("org.moqui.impl.VoiceServices.register#SipNode", {instanceId: OTHER_NODE, sipAddress: "127.0.0.1:5060"})
    })
}
function call(name, parameters) {
    return ec.service.sync().name(name).parameters(parameters).call()
}
function clean() {
    sys(() => {
        for (const s of ec.entity.find(SESSION).condition("kindEnumId", "CskPhone").condition("statusId", "!=", "CssEnded").list()) {
            s.set("statusId", "CssEnded")
            s.update()
        }
        for (const phoneNumber of [USER_NUMBER, GROUP_NUMBER, MENU_NUMBER]) {
            ec.entity.find("moqui.call.PhoneNumberFallback").condition("phoneNumber", phoneNumber).deleteAll()
            ec.entity.find("moqui.call.PhoneNumber").condition("phoneNumber", phoneNumber).deleteAll()
        }
        ec.entity.find("moqui.call.SipNode").condition("instanceId", "in", [EDGE_NODE, OTHER_NODE]).deleteAll()
        ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", "in", [EDGE_NODE, OTHER_NODE]).deleteAll()
        ec.entity.find("moqui.call.CallHandlingSetting").condition("userId", A).deleteAll()
        ec.entity.find("moqui.call.VoiceUserSetting").condition("userId", A).deleteAll()
        for (const box of ec.entity.find(BOX).condition("userId", A).list()) {
            ec.entity.find("moqui.call.Voicemail").condition("voicemailBoxId", box.voicemailBoxId).deleteAll()
        }
        for (const device of ec.entity.find("moqui.call.SipDevice").condition("userId", A).list()) {
            ec.entity.find("moqui.call.SipDeviceVerifier").condition("sipDeviceId", device.sipDeviceId).deleteAll()
            device.delete()
        }
        ec.entity.find("moqui.call.RingGroup").condition("ringGroupId", "EDGE_VOICE_SPEC_RG").deleteAll()
    })
}

let trunkId = null

spec({
    stepwise: true,
    setupSpec() {
        makeUser(ADMIN, "ADMIN")
        makeUser(A)
        clean()
        trunkId = as(ADMIN, "org.moqui.impl.VoiceServices.store#VoiceTrunk", {name: "Edge spec trunk", host: "sip.example.com"}).voiceTrunkId
        nodesUp()
    },
    cleanupSpec() {
        clean()
    }
})

feature("the edge link keeps its edge node's SipNode row, never one an engine registered, nor the reverse", {
    when() {
        const plainOnEdge = as(ADMIN, "org.moqui.impl.VoiceServices.register#SipNode", {instanceId: EDGE_NODE, sipAddress: "127.0.0.1:5099"})
        const linkOnOther = as(ADMIN, "org.moqui.impl.VoiceServices.register#SipNode", {instanceId: OTHER_NODE, sipAddress: "203.0.113.9:5060", edgeLinked: true})
        const rows = sys(() => Object.fromEntries(Array.from(ec.entity.find("moqui.call.SipNode")
            .condition("instanceId", "in", [EDGE_NODE, OTHER_NODE]).list()).map((n) => [n.instanceId, n])))
        return {plainOnEdge, linkOnOther, rows}
    },
    then({plainOnEdge, linkOnOther, rows}) {
        plainOnEdge.reasonCode == "notAllowed"
        linkOnOther.reasonCode == "notAllowed"
        rows[EDGE_NODE].edgeLinked == "Y"
        rows[EDGE_NODE].sipAddress == "203.0.113.5:5060"
        rows[OTHER_NODE].edgeLinked == "N"
        rows[OTHER_NODE].sipAddress == "127.0.0.1:5060"
    }
})

feature("the edge node's delegate user takes calls only on its own node, and notes the legs of its own calls only", {
    when() {
        as(ADMIN, "org.moqui.impl.VoiceServices.store#PhoneNumber", {phoneNumber: USER_NUMBER, voiceTrunkId: trunkId,
            routeTypeEnumId: "PnrUser", routeId: A})
        const parameters = {voiceTrunkId: trunkId, toNumber: USER_NUMBER, fromNumber: "+15557770000"}
        const elsewhere = as("_edge_", "org.moqui.impl.VoiceServices.receive#PhoneCall", Object.assign({instanceId: OTHER_NODE}, parameters))
        const taken = as("_edge_", "org.moqui.impl.VoiceServices.receive#PhoneCall", Object.assign({instanceId: EDGE_NODE}, parameters))
        const other = as(ADMIN, "org.moqui.impl.VoiceServices.receive#PhoneCall", Object.assign({instanceId: OTHER_NODE}, parameters))
        const noteOther = as("_edge_", "org.moqui.impl.VoiceServices.note#PhoneLeg", {callSessionId: other.callSessionId,
            callLegId: other.callLegId, event: "ringing"})
        const noteOwn = as("_edge_", "org.moqui.impl.VoiceServices.note#PhoneLeg", {callSessionId: taken.callSessionId,
            callLegId: taken.callLegId, event: "ringing"})
        const voicemailOther = as("_edge_", "org.moqui.impl.VoicemailServices.begin#Voicemail", {callSessionId: other.callSessionId,
            callLegId: other.callLegId, reason: "noAnswer"})
        const voicemailOwn = as("_edge_", "org.moqui.impl.VoicemailServices.begin#Voicemail", {callSessionId: taken.callSessionId,
            callLegId: taken.callLegId, reason: "noAnswer"})
        const row = sys(() => ec.entity.find(SESSION).condition("callSessionId", taken.callSessionId).one())
        return {elsewhere, taken, other, noteOther, noteOwn, voicemailOther, voicemailOwn, row}
    },
    then({elsewhere, taken, other, noteOther, noteOwn, voicemailOther, voicemailOwn, row}) {
        elsewhere.reasonCode == "notAllowed"
        taken.errors == ""
        taken.callSessionId != null
        other.errors == ""
        noteOther.reasonCode == "notAllowed"
        noteOwn.errors == ""
        voicemailOther.reasonCode == "notAllowed"
        voicemailOwn.mode == "deposit"
        row.mediaNodeId == EDGE_NODE
        // the box a message the edge node records for this call is stored in
        row.voicemailBoxId == voicemailOwn.voicemailBoxId
    }
})

feature("each number's fallback: a user's mobile then a message, a ring group's outside number, nothing for a menu", {
    when() {
        // A rings their mobile with their calls and may forward to it
        make("moqui.call.CallHandlingSetting", {userId: A, mobileNumber: MOBILE, ringMobile: "Y"})
        make("moqui.call.VoiceUserSetting", {userId: A, outboundEnabled: "Y", forwardEnabled: "Y"})
        make("moqui.call.RingGroup", {ringGroupId: "EDGE_VOICE_SPEC_RG", name: "Edge spec group", strategyEnumId: "RgsSimultaneous",
            fallbackTypeEnumId: "RgfNumber", fallbackId: "+15550168888"})
        make("moqui.call.PhoneNumber", {phoneNumber: GROUP_NUMBER, voiceTrunkId: trunkId, routeTypeEnumId: "PnrRingGroup", routeId: "EDGE_VOICE_SPEC_RG"})
        make("moqui.call.PhoneNumber", {phoneNumber: MENU_NUMBER, voiceTrunkId: trunkId, routeTypeEnumId: "PnrIvr", routeId: "NO_MENU"})
        const first = as(ADMIN, "org.moqui.impl.VoiceServices.refresh#PhoneFallbacks")
        const again = as(ADMIN, "org.moqui.impl.VoiceServices.refresh#PhoneFallbacks")
        const rows = sys(() => Object.fromEntries(Array.from(ec.entity.find("moqui.call.PhoneNumberFallback")
            .condition("phoneNumber", "in", [USER_NUMBER, GROUP_NUMBER, MENU_NUMBER]).list()).map((r) => [r.phoneNumber, r])))
        // the user may no longer forward: their number takes a message only
        make("moqui.call.VoiceUserSetting", {userId: A, outboundEnabled: "Y", forwardEnabled: "N"})
        const changed = as(ADMIN, "org.moqui.impl.VoiceServices.refresh#PhoneFallbacks", {phoneNumber: USER_NUMBER})
        const after = sys(() => ec.entity.find("moqui.call.PhoneNumberFallback").condition("phoneNumber", USER_NUMBER).one())
        return {first, again, rows, changed, after}
    },
    then({first, again, rows, changed, after}) {
        first.errors == ""
        Number(again.writtenCount) == 0
        rows[USER_NUMBER].voicemailFlag == "Y"
        rows[USER_NUMBER].forwardNumber == MOBILE
        rows[USER_NUMBER].voiceTrunkId == trunkId
        rows[GROUP_NUMBER].voicemailFlag == "N"
        rows[GROUP_NUMBER].forwardNumber == "+15550168888"
        // the limits the edge node holds the forwards to: the user's at once and a day (the policy's defaults), and
        // the deployment's for a ring group
        Number(rows[USER_NUMBER].forwardMaxConcurrent) == 2
        Number(rows[USER_NUMBER].forwardMaxPerDay) == 100
        Number(rows[GROUP_NUMBER].forwardMaxConcurrent) == 2
        Number(rows[GROUP_NUMBER].forwardMaxPerDay) == 1000
        rows[MENU_NUMBER] == null
        Number(changed.writtenCount) == 1
        after.forwardNumber == null
        after.voicemailFlag == "Y"
    }
})

feature("the edge node's SIP engine takes calls while its media row drains (calls off for browsers)", {
    when() {
        sys(() => call("org.moqui.impl.CallServices.register#MediaNode", {instanceId: EDGE_NODE, mediaAddress: "203.0.113.5:10000",
            edgeLinked: true, draining: true}))
        const taken = as("_edge_", "org.moqui.impl.VoiceServices.receive#PhoneCall", {voiceTrunkId: trunkId, toNumber: USER_NUMBER,
            fromNumber: "+15557770002", instanceId: EDGE_NODE})
        sys(() => call("org.moqui.impl.CallServices.register#MediaNode", {instanceId: EDGE_NODE, mediaAddress: "203.0.113.5:10000",
            edgeLinked: true}))
        return {taken}
    },
    then({taken}) {
        taken.errors == ""
        taken.callSessionId != null
    }
})

feature("a message the edge node queued is stored in the number's box, only from the queue", {
    when() {
        const location = CONTENT + "vm-1"
        sys(() => ec.resource.getLocationReference(location).putText("OggS an edge node's message"))
        const payload = JSON.stringify({toNumber: USER_NUMBER, fromNumber: "+15557771111", durationMs: 1500, receivedMs: Date.now() - 60000})
        const item = {queueItemId: "edge-voice-spec-1", pairedNodeId: "local", kind: "voicemail", payload, contentLocation: location}
        const refused = as(A, "org.moqui.impl.VoicemailServices.receive#EdgeVoicemail", item)
        const stored = as("_edge_", "org.moqui.impl.VoicemailServices.receive#EdgeVoicemail", item)
        const outside = as("_edge_", "org.moqui.impl.VoicemailServices.receive#EdgeVoicemail",
            Object.assign({}, item, {contentLocation: "dbresource://moqui/call/voicemail/elsewhere.ogg"}))
        const row = stored.voicemailId ? sys(() => ec.entity.find("moqui.call.Voicemail").condition("voicemailId", stored.voicemailId).one()) : null
        const box = row ? sys(() => ec.entity.find(BOX).condition("voicemailBoxId", row.voicemailBoxId).one()) : null
        const audio = row ? sys(() => ec.resource.getLocationReference(row.audioLocation).getText()) : null
        return {refused, stored, outside, row, box, audio}
    },
    then({refused, stored, outside, row, box, audio}) {
        refused.reasonCode == "notAllowed"
        stored.errors == ""
        row.fromNumber == "+15557771111"
        row.statusId == "VmsStored"
        row.callSessionId == null
        box.userId == A
        audio == "OggS an edge node's message"
        // a copy that is not the queue's own is not stored: the message is kept without its audio
        outside.voicemailId != null
    }
})

feature("a phone's digest answer is checked here: matched, noted for the edge node, and a wrong one not", {
    when() {
        // the phone is made for the edge node's realm, and its registrar is the edge node's
        const made = as(A, "org.moqui.impl.SipDeviceServices.create#SipDevice", {name: "Edge spec phone"})
        const auth = as(null, "org.moqui.impl.SipDeviceServices.get#SipDeviceAuth", {username: made.username, realm: made.domain})
        const verifier = (auth.verifierList || []).find((v) => v.algorithm == "SHA-256")
        const ha2 = "0".repeat(64)
        const signed = (nonce, nc) => Object.assign({username: made.username, realm: made.domain, algorithm: "SHA-256", nonce, nc,
            cnonce: "c1", qop: "auth", ha2}, {response: as(null, "org.moqui.impl.SipDeviceServices.compute#SipDigestResponse",
            {algorithm: "SHA-256", ha1: verifier.ha1, nonce, nc, cnonce: "c1", qop: "auth", ha2}).response})
        const nonce = edgeNonce(5, 7)
        const answer = signed(nonce, "00000001")
        const check = (fields) => as("_edge_", "org.moqui.impl.SipDeviceServices.check#SipDeviceDigest", fields)
        const wrong = check(Object.assign({}, answer, {response: "f".repeat(64)}))
        const otherRealm = check(Object.assign({}, answer, {realm: "moqui"}))
        const before = sys(() => ec.entity.find("moqui.call.SipDevice").condition("sipDeviceId", auth.sipDeviceId).one()).edgeDigestDate
        const right = check(answer)
        const after = sys(() => ec.entity.find("moqui.call.SipDevice").condition("sipDeviceId", auth.sipDeviceId).one()).edgeDigestDate
        // the same answer again, and a right one on a nonce made long ago: an answer the edge node kept signs nobody in
        const replayed = check(answer)
        const nextCount = check(signed(nonce, "00000002"))
        const old = check(signed(edgeNonce(3600, 8), "00000001"))
        const elsewhere = as("_edge_", "org.moqui.impl.SipDeviceServices.store#SipRegistration", {change: "added",
            sipRegistrationId: "edge-voice-spec-reg", sipDeviceId: auth.sipDeviceId, instanceId: OTHER_NODE, credentialRevision: 1,
            expiresDate: Date.now() + 60000})
        const own = as("_edge_", "org.moqui.impl.SipDeviceServices.store#SipRegistration", {change: "added",
            sipRegistrationId: "edge-voice-spec-reg", sipDeviceId: auth.sipDeviceId, instanceId: EDGE_NODE, credentialRevision: 1,
            expiresDate: Date.now() + 60000})
        const kept = sys(() => ec.entity.find("moqui.call.SipRegistration").condition("sipRegistrationId", "edge-voice-spec-reg").one())
        sys(() => ec.entity.find("moqui.call.SipRegistration").condition("sipRegistrationId", "edge-voice-spec-reg").deleteAll())
        return {made, wrong, otherRealm, before, right, after, replayed, nextCount, old, auth, elsewhere, own, kept}
    },
    then({made, wrong, otherRealm, before, right, after, replayed, nextCount, old, auth, elsewhere, own, kept}) {
        made.errors == ""
        made.domain == EDGE_REALM
        made.registrarEnabled == true
        wrong.matched == false
        otherRealm.matched == false
        replayed.matched == false
        nextCount.matched == true
        old.matched == false
        wrong.sipDeviceId == null
        before == null
        right.matched == true
        right.sipDeviceId == auth.sipDeviceId
        after != null
        elsewhere.reasonCode == "notAllowed"
        own.errors == ""
        kept.instanceId == EDGE_NODE
    }
})

