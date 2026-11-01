// End-to-end encrypted calls (plans/Comms-Framework.md §6.4, D-W3, W8; docs/interfaces/WebRTC.md "Encrypted calls"): the
// policy of org.moqui.impl.CallServices and Calls.js. A call is encrypted when its starter asks for it, or when its room's
// or call link's policy says so, and stays so. Only a browser that can take part answers or joins one (it has
// RTCRtpScriptTransform and names a crypto device of its user's, ChatDevice); a leg binds its media only with a device,
// and the call's view publishes the roster of legs, devices and key indexes the browsers send their keys to. An encrypted
// call refuses recording, captions, a phone number, desk phones, forwarded numbers, guests and merging; each refusal is
// shown beside a plain call that the same parameters pass. Revoking a device takes its leg out of the call. No media runs
// here: the browsers' SFrame and key exchange are the client spec EncryptedCallFlow's.
// Run: ./moq test framework --spec CallEncryption
const A = "E2EE_SPEC_A"
const B = "E2EE_SPEC_B"
const C = "E2EE_SPEC_C"
const D = "E2EE_SPEC_D"
const NODE = "e2ee-spec-node"
const SESSION = "moqui.call.CallSession"
const LEG = "moqui.call.CallLeg"
const DEVICES = {[A]: "EESPECAAAA", [B]: "EESPECBBBB", [C]: "EESPECCCCC", [D]: "EESPECDDDD"}
const B_OLD = "EESPECBOLD"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function as(userId, name, parameters) {
    ec.user.internalLoginUser(userId.toLowerCase().replace(/_/g, "."))
    let out = {}
    let thrown = ""
    try {
        // as the screens' transitions call them: the services check the signed-in user's part in each call
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : "org.moqui.impl.CallServices." + name)
            .parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() > 0) return
        const account = ec.entity.makeValue("moqui.security.UserAccount")
        account.setFields({userId, username: userId.toLowerCase().replace(/_/g, "."), userFullName: "Encrypted " + userId.slice(-1),
            disabled: "N"}, true, null, null)
        account.create()
    })
}
/** A crypto device with its keys uploaded (opaque here, as the server treats them); revoked when asked. */
function makeDevice(userId, deviceId, revoked) {
    sys(() => {
        ec.entity.find("moqui.chat.ChatDevice").condition({userId, deviceId}).deleteAll()
        const device = ec.entity.makeValue("moqui.chat.ChatDevice")
        device.setFields({userId, deviceId, displayName: "Spec browser", curve25519Key: "curve-" + deviceId, ed25519Key: "ed-" + deviceId,
            listRevision: 1, lastToDevicePos: 0, ackedToDevicePos: 0, createdDate: ec.user.nowTimestamp,
            revokedDate: revoked ? ec.user.nowTimestamp : null}, true, null, null)
        device.create()
    })
}
/** What a browser that can take part sends: RTCRtpScriptTransform and its device. */
const browser = (userId, more) => Object.assign({cryptoDeviceId: DEVICES[userId], scriptTransform: true}, more || {})
const session = id => sys(() => ec.entity.find(SESSION).condition("callSessionId", id).one())
const leg = id => sys(() => ec.entity.find(LEG).condition("callLegId", id).one())
const legOf = (call, userId) => {
    const p = (call.participants || []).find(x => x.userId === userId)
    return p ? (p.legs || []).find(l => l.type === "web") || null : null
}
/** An internal operation of Calls.js, in a transaction of its own (as the services that call it run it). */
const internal = (op, parameters) => parallel(1, (n, args) => {
    ec.artifactExecution.disableAuthz()
    const out = ec.resource.script("component://framework/service/org/moqui/impl/Calls.js").execute(args.op, args.parameters)
    return {forwardLegList: Array.from(out.forwardLegList || []).map(l => ({callLegId: l.callLegId})), reasonCode: out.reasonCode || null}
}, {op, parameters})[0]
function endAll() {
    sys(() => {
        const mine = [A, B, C, D]
        for (const s of ec.entity.find(SESSION).condition("startedByUserId", "in", mine).condition("statusId", "!=", "CssEnded").list()) {
            s.set("statusId", "CssEnded")
            s.update()
        }
        for (const l of ec.entity.find(LEG).condition("statusId", "in", ["ClsOffered", "ClsAnswered"]).list()) {
            const p = ec.entity.find("moqui.call.CallParticipant").condition("callParticipantId", l.callParticipantId).one()
            if (mine.includes(p?.userId)) {
                l.set("statusId", "ClsLeft")
                l.update()
            }
        }
    })
}

spec({
    stepwise: true,
    setupSpec() {
        for (const id of [A, B, C, D]) {
            makeUser(id)
            makeDevice(id, DEVICES[id], false)
        }
        makeDevice(B, B_OLD, true)
        endAll()
        sys(() => call("org.moqui.impl.CallServices.register#MediaNode", {instanceId: NODE, mediaAddress: "127.0.0.1:10000"}))
    },
    cleanupSpec() {
        endAll()
        sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", NODE).deleteAll())
    }
})

let encrypted = null
let plain = null

feature("an encrypted call starts only from a browser that can encrypt, and its leg names the device and a key index", {
    when() {
        const before = sys(() => ec.entity.find(SESSION).condition("startedByUserId", A).count())
        const noTransform = as(A, "start#Call", {calleeUserIdList: [B], requireE2ee: true, ...browser(A, {scriptTransform: false})})
        const othersDevice = as(A, "start#Call", {calleeUserIdList: [B], requireE2ee: true, ...browser(B)})
        const afterRefused = sys(() => ec.entity.find(SESSION).condition("startedByUserId", A).count())
        const started = as(A, "start#Call", {calleeUserIdList: [B], requireE2ee: true, ...browser(A)})
        encrypted = started.callSessionId
        const notAsked = as(A, "start#Call", {calleeUserIdList: [C], ...browser(A)})
        plain = notAsked.callSessionId
        return {before, noTransform, othersDevice, afterRefused, started, notAsked, row: session(encrypted)}
    },
    then({before, noTransform, othersDevice, afterRefused, started, notAsked, row}) {
        noTransform.reasonCode == "unsupportedClient"
        noTransform.errors.includes("RTCRtpScriptTransform")
        othersDevice.reasonCode == "noCryptoDevice"
        afterRefused == before
        started.errors == ""
        row.encryptionMode == "e2ee-sframe"
        started.call.e2ee == true
        started.call.encryptionMode == "e2ee-sframe"
        legOf(started.call, A).deviceId == DEVICES[A]
        legOf(started.call, A).keyIndex == 1
        notAsked.call.encryptionMode == "dtls-srtp"
        notAsked.call.e2ee == false
        legOf(notAsked.call, A).deviceId == null
    }
})

feature("a browser that cannot take part does not answer, and the call rings on; one that can answers with its device", {
    when() {
        const noTransform = as(B, "answer#Call", {callSessionId: encrypted, ...browser(B, {scriptTransform: false})})
        const stillRinging = as(B, "get#Call", {callSessionId: encrypted})
        const revoked = as(B, "answer#Call", {callSessionId: encrypted, ...browser(B, {cryptoDeviceId: B_OLD})})
        const othersDevice = as(B, "answer#Call", {callSessionId: encrypted, ...browser(A)})
        const answered = as(B, "answer#Call", {callSessionId: encrypted, ...browser(B)})
        const plainAnswered = as(C, "answer#Call", {callSessionId: plain})
        return {noTransform, stillRinging, revoked, othersDevice, answered, plainAnswered, seenByA: as(A, "get#Call", {callSessionId: encrypted})}
    },
    then({noTransform, stillRinging, revoked, othersDevice, answered, plainAnswered, seenByA}) {
        noTransform.reasonCode == "unsupportedClient"
        stillRinging.call.myLegStatus == "offered"
        revoked.reasonCode == "noCryptoDevice"
        othersDevice.reasonCode == "noCryptoDevice"
        answered.errors == ""
        answered.call.status == "active"
        plainAnswered.errors == ""
        legOf(seenByA.call, B).deviceId == DEVICES[B]
        legOf(seenByA.call, B).keyIndex == 2
        legOf(seenByA.call, A).keyIndex == 1
    }
})

feature("a leg binds its media only once it names its device; a reconnect is a new key generation", {
    when() {
        // a call started from a form names no device: the page names it before its media binds
        const formCall = as(A, "start#Call", {calleeUserIdList: [B, C], requireE2ee: true})
        const myLeg = formCall.callLegId
        const unbound = as(A, "bind#Connection", {callSessionId: formCall.callSessionId, callLegId: myLeg, signalingNodeId: NODE})
        const othersDevice = as(A, "register#CallDevice", {callSessionId: formCall.callSessionId, ...browser(B)})
        const noTransform = as(A, "register#CallDevice", {callSessionId: formCall.callSessionId, ...browser(A, {scriptTransform: false})})
        const stranger = as(D, "register#CallDevice", {callSessionId: formCall.callSessionId, ...browser(D)})
        const revisionBefore = Number(session(formCall.callSessionId).revision)
        const registered = as(A, "register#CallDevice", {callSessionId: formCall.callSessionId, ...browser(A)})
        const first = as(A, "bind#Connection", {callSessionId: formCall.callSessionId, callLegId: myLeg, signalingNodeId: NODE})
        const second = as(A, "bind#Connection", {callSessionId: formCall.callSessionId, callLegId: myLeg, signalingNodeId: NODE})
        const seen = as(A, "get#Call", {callSessionId: formCall.callSessionId})
        const onPlain = as(A, "register#CallDevice", {callSessionId: plain, ...browser(A)})
        as(A, "end#Call", {callSessionId: formCall.callSessionId})
        return {formCall, unbound, othersDevice, noTransform, stranger, revisionBefore, registered, first, second, seen, onPlain,
            row: leg(myLeg), revisionAfter: Number(session(formCall.callSessionId).revision)}
    },
    then({formCall, unbound, othersDevice, noTransform, stranger, revisionBefore, registered, first, second, seen, onPlain, row, revisionAfter}) {
        formCall.call.e2ee == true
        legOf(formCall.call, A).deviceId == null
        unbound.reasonCode == "noCryptoDevice"
        othersDevice.reasonCode == "noCryptoDevice"
        noTransform.reasonCode == "unsupportedClient"
        stranger.reasonCode == "notFound"
        registered.errors == ""
        row.cryptoDeviceId == DEVICES[A]
        Number(row.keyIndex) == 1
        first.generation == 1
        second.generation == 2
        legOf(seen.call, A).keyGeneration == 2
        revisionAfter > revisionBefore + 2
        onPlain.reasonCode == "notEncrypted"
    }
})

feature("an encrypted call refuses recording, captions, a number, desk phones, forwarded numbers and a merge, which a plain call passes with the same parameters", {
    when() {
        setProperty("call_recording_enabled", "true")
        setProperty("call_recording_ack_required", "false")
        const recordEncrypted = as(A, "org.moqui.impl.CallRecordingServices.start#Recording", {callSessionId: encrypted})
        const stateEncrypted = as(A, "org.moqui.impl.CallRecordingServices.get#RecordingState", {callSessionId: encrypted})
        const recordPlain = as(A, "org.moqui.impl.CallRecordingServices.start#Recording", {callSessionId: plain})
        as(A, "org.moqui.impl.CallRecordingServices.stop#Recording", {callSessionId: plain, callRecordingId: recordPlain.callRecordingId})
        setProperty("call_recording_enabled", "false")
        const captions = withProperty("call_captions_enabled", "true", () => ({
            encrypted: as(A, "org.moqui.impl.CallCaptionServices.start#Captions", {callSessionId: encrypted}),
            plain: as(A, "org.moqui.impl.CallCaptionServices.start#Captions", {callSessionId: plain}),
        }))
        as(A, "org.moqui.impl.CallCaptionServices.stop#Captions", {callSessionId: plain})
        const numberEncrypted = as(A, "org.moqui.impl.CallTransferServices.add#CallParticipant", {callSessionId: encrypted, phoneNumber: "+15550199977"})
        const numberPlain = as(A, "org.moqui.impl.CallTransferServices.add#CallParticipant", {callSessionId: plain, phoneNumber: "+15550199977"})
        const phonesEncrypted = as(A, "org.moqui.impl.SipDeviceServices.ring#SipDevices", {callSessionId: encrypted, userIdList: [B]})
        const phonesPlain = as(A, "org.moqui.impl.SipDeviceServices.ring#SipDevices", {callSessionId: plain, userIdList: [C]})
        const forwardEncrypted = internal("ringStep", {callSessionId: encrypted, userIdList: [], numberList: [{number: "+15550199988"}]})
        const forwardPlain = internal("ringStep", {callSessionId: plain, userIdList: [], numberList: [{number: "+15550199988"}]})
        const mergeEncrypted = as(A, "org.moqui.impl.CallTransferServices.merge#Calls", {callSessionId: plain, otherCallSessionId: encrypted})
        const mergeIntoEncrypted = as(A, "org.moqui.impl.CallTransferServices.merge#Calls", {callSessionId: encrypted, otherCallSessionId: plain})
        // a second plain call, which merges with the first
        const other = as(A, "start#Call", {calleeUserIdList: [D]})
        as(D, "answer#Call", {callSessionId: other.callSessionId})
        const mergePlain = as(A, "org.moqui.impl.CallTransferServices.merge#Calls", {callSessionId: plain, otherCallSessionId: other.callSessionId})
        const pstnLegs = sys(() => ec.entity.find(LEG).condition({callSessionId: encrypted, legTypeEnumId: "CltPstn"}).count())
        return {recordEncrypted, stateEncrypted, recordPlain, captions, numberEncrypted, numberPlain, phonesEncrypted, phonesPlain,
            forwardEncrypted, forwardPlain, mergeEncrypted, mergeIntoEncrypted, mergePlain, pstnLegs}
    },
    then({recordEncrypted, stateEncrypted, recordPlain, captions, numberEncrypted, numberPlain, phonesEncrypted, phonesPlain,
          forwardEncrypted, forwardPlain, mergeEncrypted, mergeIntoEncrypted, mergePlain, pstnLegs}) {
        recordEncrypted.reasonCode == "encrypted"
        recordEncrypted.errors.includes("cannot be recorded")
        stateEncrypted.canRecord == false
        stateEncrypted.encrypted == true
        recordPlain.errors == ""
        recordPlain.callRecordingId != null
        captions.encrypted.reasonCode == "encrypted"
        captions.plain.reasonCode != "encrypted"
        numberEncrypted.reasonCode == "encrypted"
        numberPlain.reasonCode != "encrypted"
        phonesEncrypted.reasonCode == "encrypted"
        phonesEncrypted.ringCount == 0
        phonesPlain.reasonCode == null
        forwardEncrypted.forwardLegList.length == 0
        forwardPlain.forwardLegList.length == 1
        mergeEncrypted.reasonCode == "encrypted"
        mergeIntoEncrypted.reasonCode == "encrypted"
        mergePlain.reasonCode != "encrypted"
        pstnLegs == 0
    }
})

feature("an encrypted call link's meeting takes no guest; a plain link takes one with the same parameters", {
    when() {
        const encryptedLink = as(A, "create#CallLink", {name: "Encrypted spec meeting", policy: "link", requireE2ee: true})
        const plainLink = as(A, "create#CallLink", {name: "Plain spec meeting", policy: "link"})
        const tokenEncrypted = as(A, "check#CallLinkToken", {token: encryptedLink.token})
        const tokenPlain = as(A, "check#CallLinkToken", {token: plainLink.token})
        const guestEncrypted = as(D, "join#AsGuest", {token: encryptedLink.token, displayName: "Spec Guest"})
        const guestPlain = as(D, "join#AsGuest", {token: plainLink.token, displayName: "Spec Guest"})
        // the owner opens the encrypted meeting from a form: its leg names its device before the media binds
        const opened = as(A, "open#CallLink", {callLinkId: encryptedLink.callLinkId})
        const guestNow = as(D, "join#AsGuest", {token: encryptedLink.token, displayName: "Spec Guest"})
        const links = as(A, "find#CallLinks")
        for (const id of [opened.callSessionId, guestPlain.callSessionId]) if (id) as(A, "end#Call", {callSessionId: id})
        return {tokenEncrypted, tokenPlain, guestEncrypted, guestPlain, opened, guestNow, links, encryptedLink}
    },
    then({tokenEncrypted, tokenPlain, guestEncrypted, guestPlain, opened, guestNow, links, encryptedLink}) {
        tokenEncrypted.routeContext.encrypted == true
        tokenPlain.routeContext.encrypted == false
        guestEncrypted.reasonCode == "encryptedNoGuests"
        guestEncrypted.credential == null
        guestPlain.admission == "admitted"
        opened.call.e2ee == true
        opened.callLegId != null
        guestNow.reasonCode == "encryptedNoGuests"
        links.linkList.find(l => l.callLinkId == encryptedLink.callLinkId).e2ee == true
    }
})

let roomCall = null

feature("a room's owner or moderator makes its calls encrypted; a plain call going on is never offered for an encrypted one", {
    when() {
        ec.user.internalLoginUser("e2ee.spec.a")
        const room = sys(() => call("org.moqui.impl.ChatServices.create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Encrypted calls spec",
            memberUserIdList: [B, C]}))
        const chatRoomId = room.chatRoomId
        const plainRoomCall = as(A, "start#Call", {chatRoomId})
        const downgrade = as(B, "start#Call", {chatRoomId, requireE2ee: true, ...browser(B)})
        const byMember = as(C, "set#RoomCallEncryption", {chatRoomId, required: true})
        const byOwner = as(A, "set#RoomCallEncryption", {chatRoomId, required: true})
        const byStranger = as(D, "set#RoomCallEncryption", {chatRoomId, required: true})
        // the room's policy asks for encryption while its plain call goes on: that call is not offered either
        const policyDowngrade = as(B, "start#Call", {chatRoomId, ...browser(B)})
        as(A, "end#Call", {callSessionId: plainRoomCall.callSessionId})
        const started = as(B, "start#Call", {chatRoomId, ...browser(B)})
        roomCall = started.callSessionId
        const joinedPlainly = as(A, "join#Call", {callSessionId: roomCall})
        const joined = as(A, "join#Call", {callSessionId: roomCall, ...browser(A)})
        const roomView = sys(() => call("org.moqui.impl.ChatServices.get#RoomAccess", {chatRoomId, userId: A}))
        return {plainRoomCall, downgrade, policyDowngrade, byMember, byOwner, byStranger, started, joinedPlainly, joined, roomView}
    },
    then({plainRoomCall, downgrade, policyDowngrade, byMember, byOwner, byStranger, started, joinedPlainly, joined, roomView}) {
        plainRoomCall.call.e2ee == false
        downgrade.reasonCode == "callNotEncrypted"
        downgrade.callSessionId == plainRoomCall.callSessionId
        policyDowngrade.reasonCode == "callNotEncrypted"
        policyDowngrade.call == null
        byMember.reasonCode == "notAllowed"
        byOwner.required == true
        byStranger.reasonCode == "notFound"
        roomView.room.callE2eeRequired == true
        started.call.e2ee == true
        legOf(started.call, B).deviceId == DEVICES[B]
        joinedPlainly.reasonCode == "unsupportedClient"
        joined.errors == ""
        legOf(joined.call, A).keyIndex == 2
    }
})

feature("revoking a device takes its leg out of the encrypted call, and the others' roster no longer has it", {
    when() {
        const revoked = as(B, "org.moqui.impl.ChatKeyServices.revoke#ChatDevice", {deviceId: DEVICES[B]})
        const seen = as(A, "get#Call", {callSessionId: roomCall})
        return {revoked, seen}
    },
    then({revoked, seen}) {
        revoked.errors == ""
        seen.call.status == "active"
        legOf(seen.call, B).status == "left"
        legOf(seen.call, A).status == "answered"
    }
})
