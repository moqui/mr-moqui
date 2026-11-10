// Desk and soft phones (org.moqui.call.SipDeviceServices and SipDevices.js; the V4 block of VoiceEntities.xml;
// plans/Comms-Framework.md §5.2 "Registrar for phones", V4): the registrar off by default, a phone's credentials shown once
// with only verifiers kept, another user refused with the same parameters, rotation and revocation ending the old
// credentials and the registrations, a call ringing a user's registered phone beside their tabs with the first answer
// winning (a phone over a tab, a tab over a phone, a phone over another phone), a phone call to a user with phones and a
// voicemail box (tabs and phones ring together for the box's rings, then voicemail cancels every phone; a decline on one
// phone declines the ring everywhere and sends the call to voicemail), and a phone's own call placed as its user. No SIP engine runs here: fake SipNode and CallMediaNode rows stand in for one, SipRegistration rows for the
// bindings it would keep, and send#SipControl to them is only logged. The engine's side is moqui-sip's ua_phones tests
// and `it sip::`.
// Run: ./moq test framework --spec SipDevice
const ADMIN = "SIPDEV_SPEC_ADMIN"
const A = "SIPDEV_SPEC_A"
const B = "SIPDEV_SPEC_B"
const NODE = "sipdev-spec-node"
const DEVICE = "moqui.call.SipDevice"
const LEG = "moqui.call.CallLeg"
const NUMBER_A = "+15550180001"
const CALLER = "+15557770188"

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
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : "org.moqui.call.SipDeviceServices." + name).parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const internal = (name, parameters) => as(null, name, parameters)
const on = (run) => withProperty("sip_registrar_enabled", "true", run)
function makeUser(userId, groupId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "Phone " + userId.slice(-1), disabled: "N"}, true, null, null)
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
        ec.service.sync().name("org.moqui.call.CallServices.register#MediaNode").parameters({instanceId: NODE, mediaAddress: "127.0.0.1:10000"}).call()
        ec.service.sync().name("org.moqui.call.VoiceServices.register#SipNode").parameters({instanceId: NODE, sipAddress: "127.0.0.1:5060", tlsAddress: "127.0.0.1:5061"}).call()
    })
}
/** A binding the engine would keep for the device. */
function registered(device, id) {
    return internal("store#SipRegistration", {change: "added", sipRegistrationId: id, sipDeviceId: device.sipDeviceId, instanceId: NODE,
        contactUri: "sip:" + device.username + "@192.168.1.20:5060", transport: "udp", sourceAddress: "198.51.100.7", sourcePort: 61000,
        sipCallId: "reg-" + id, cseq: 2, credentialRevision: device.credentialRevision || 1, expiresDate: Date.now() + 300000,
        lastRefreshDate: Date.now(), userAgent: "Spec Phone/1.0"})
}
const registrations = (sipDeviceId) => sys(() => ec.entity.find("moqui.call.SipRegistration").condition("sipDeviceId", sipDeviceId).count())
const deviceRow = (sipDeviceId) => sys(() => ec.entity.find(DEVICE).condition("sipDeviceId", sipDeviceId).one())
const legsOf = (callSessionId) => sys(() => Array.from(ec.entity.find(LEG).condition("callSessionId", callSessionId).orderBy("offeredDate").list()))
const deviceLegs = (callSessionId) => legsOf(callSessionId).filter((l) => l.legTypeEnumId === "CltSipDevice")
const note = (callSessionId, callLegId, event) => internal("org.moqui.call.VoiceServices.note#PhoneLeg", {callSessionId, callLegId, event})
function clean() {
    sys(() => {
        for (const s of ec.entity.find("moqui.call.CallSession").condition("startedByUserId", "in", [A, B]).condition("statusId", "!=", "CssEnded").list()) {
            s.set("statusId", "CssEnded")
            s.update()
        }
        for (const d of ec.entity.find(DEVICE).condition("userId", "in", [A, B]).list()) {
            ec.entity.find("moqui.call.SipDeviceVerifier").condition("sipDeviceId", d.sipDeviceId).deleteAll()
            ec.entity.find("moqui.call.SipRegistration").condition("sipDeviceId", d.sipDeviceId).deleteAll()
            d.delete()
        }
        ec.entity.find("moqui.call.PhoneNumber").condition("phoneNumber", NUMBER_A).deleteAll()
        ec.entity.find("moqui.call.VoicemailBox").condition("userId", "in", [A, B]).deleteAll()
        ec.entity.find("moqui.call.SipNode").condition("instanceId", NODE).deleteAll()
        ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", NODE).deleteAll()
    })
}

let first = null

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

feature("the registrar is off by default: phones may be added, but none rings and none places a call", {
    when() {
        nodesUp()
        const listed = as(A, "find#SipDevices")
        const made = as(A, "create#SipDevice", {name: "Desk"})
        const device = deviceRow(made.sipDeviceId)
        registered(device, "off-1")
        const call = as(B, "org.moqui.call.CallServices.start#Call", {calleeUserIdList: [A]})
        const placed = internal("receive#DeviceCall", {sipDeviceId: made.sipDeviceId, toUser: username(B), instanceId: NODE})
        const listedOn = on(() => as(A, "find#SipDevices"))
        const rung = deviceLegs(call.callSessionId)
        as(B, "org.moqui.call.CallServices.end#Call", {callSessionId: call.callSessionId})
        as(A, "revoke#SipDevice", {sipDeviceId: made.sipDeviceId})
        return {listed, made, call, rung, placed, listedOn}
    },
    then({listed, made, call, rung, placed, listedOn}) {
        listed.registrarEnabled == false
        made.errors == ""
        made.registrarEnabled == false
        call.callSessionId != null
        rung.length == 0
        placed.reasonCode == "notAllowed"
        listedOn.registrarEnabled == true
    }
})

feature("a phone's credentials are shown once: only its verifiers are kept", {
    when() {
        first = as(A, "create#SipDevice", {name: "Desk phone"})
        const auth = internal("get#SipDeviceAuth", {username: first.username, realm: first.domain})
        const listed = as(A, "find#SipDevices")
        const legacy = as(A, "create#SipDevice", {name: "Old phone", deviceProfile: "legacy"})
        const legacyAuth = internal("get#SipDeviceAuth", {username: legacy.username, realm: legacy.domain})
        as(A, "revoke#SipDevice", {sipDeviceId: legacy.sipDeviceId})
        return {auth, listed, legacyAuth, row: deviceRow(first.sipDeviceId)}
    },
    then({auth, listed, legacyAuth, row}) {
        first.errors == ""
        /^[1-9][0-9]{9}$/.test(first.username)
        first.password.length == 22
        first.domain == "moqui"
        first.tlsPort == 5061
        first.transport == "TLS"
        auth.sipDeviceId == first.sipDeviceId
        auth.verifierList.length == 1
        auth.verifierList[0].algorithm == "SHA-256"
        /^[0-9a-f]{64}$/.test(auth.verifierList[0].ha1)
        auth.verifierList[0].ha1 != first.password
        legacyAuth.verifierList.map((v) => v.algorithm).sort().join(",") == "MD5,SHA-256"
        listed.deviceList.length == 1
        !JSON.stringify(listed).includes(first.password)
        row.deviceProfile == "standard"
        row.statusId == "SdsActive"
    }
})

feature("another user is refused with the same parameters; a voice admin is not", {
    when() {
        const params = {sipDeviceId: first.sipDeviceId}
        return {
            rotate: as(B, "rotate#SipDeviceCredential", params),
            revoke: as(B, "revoke#SipDevice", params),
            list: as(B, "find#SipDevices", {userId: A}),
            create: as(B, "create#SipDevice", {name: "Not mine", userId: A}),
            adminList: as(ADMIN, "find#SipDevices", {userId: A}),
            all: as(B, "find#SipDevices", {allUsers: true}),
            row: deviceRow(first.sipDeviceId),
        }
    },
    then({rotate, revoke, list, create, adminList, all, row}) {
        rotate.reasonCode == "notFound"
        rotate.password == null
        revoke.reasonCode == "notFound"
        list.reasonCode == "notAllowed"
        create.reasonCode == "notAllowed"
        all.reasonCode == "notAllowed"
        adminList.deviceList.length == 1
        row.statusId == "SdsActive"
        Number(row.credentialRevision) == 1
    }
})

feature("rotation makes new credentials and ends the old ones and the registrations", {
    when() {
        const before = internal("get#SipDeviceAuth", {username: first.username, realm: "moqui"})
        registered(deviceRow(first.sipDeviceId), "rot-1")
        const counted = registrations(first.sipDeviceId)
        const listed = as(A, "find#SipDevices")
        const rotated = as(A, "rotate#SipDeviceCredential", {sipDeviceId: first.sipDeviceId})
        const after = internal("get#SipDeviceAuth", {username: first.username, realm: "moqui"})
        // a binding the engine reports for the old revision is not kept
        const stale = registered(Object.assign({}, deviceRow(first.sipDeviceId), {credentialRevision: 1}), "rot-2")
        return {before, counted, listed, rotated, after, stale, left: registrations(first.sipDeviceId)}
    },
    then({before, counted, listed, rotated, after, left}) {
        counted == 1
        listed.deviceList[0].registrationList.length == 1
        listed.deviceList[0].registrationList[0].userAgent == "Spec Phone/1.0"
        rotated.errors == ""
        rotated.username == first.username
        rotated.password != first.password
        after.credentialRevision == 2
        after.verifierList.length == 1
        after.verifierList[0].ha1 != before.verifierList[0].ha1
        left == 0
    }
})

feature("a call rings the user's registered phone beside the tabs, and the phone's answer wins", {
    when() {
        return on(() => {
            nodesUp()
            registered(deviceRow(first.sipDeviceId), "ring-1")
            const call = as(B, "org.moqui.call.CallServices.start#Call", {calleeUserIdList: [A]})
            const offered = deviceLegs(call.callSessionId)
            const answered = note(call.callSessionId, offered[0].callLegId, "answered")
            const tabAnswer = as(A, "org.moqui.call.CallServices.answer#Call", {callSessionId: call.callSessionId})
            const view = as(A, "org.moqui.call.CallServices.get#Call", {callSessionId: call.callSessionId})
            const legs = legsOf(call.callSessionId)
            as(B, "org.moqui.call.CallServices.end#Call", {callSessionId: call.callSessionId})
            return {call, offered, answered, tabAnswer, view, legs}
        })
    },
    then({offered, answered, tabAnswer, view, legs}) {
        offered.length == 1
        offered[0].statusId == "ClsOffered"
        offered[0].sipDeviceId == first.sipDeviceId
        answered.errors == ""
        legs.find((l) => l.callLegId == offered[0].callLegId).statusId == "ClsAnswered"
        // A's tab leg was canceled: the tab answering after the phone is refused
        tabAnswer.reasonCode == "notRinging"
        view.call.status == "active"
        view.call.myLegStatus == "canceled"
    }
})

const LEG_NODE = "sipdev-spec-leg-node"
feature("a phone registered with another node rings there only with cascading media nodes, its leg's media on that node", {
    when() {
        return on(() => {
            nodesUp()
            sys(() => {
                ec.service.sync().name("org.moqui.call.CallServices.register#MediaNode").parameters({instanceId: LEG_NODE, mediaAddress: "127.0.0.1:10002"}).call()
                ec.service.sync().name("org.moqui.call.VoiceServices.register#SipNode").parameters({instanceId: LEG_NODE, sipAddress: "127.0.0.1:5070"}).call()
                // the call's node is the one that registered last
                for (const n of ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", LEG_NODE).list()) {
                    n.set("heartbeatDate", new Date(Date.now() - 5000))
                    n.update()
                }
                ec.entity.find("moqui.call.SipRegistration").condition("sipDeviceId", first.sipDeviceId).deleteAll()
            })
            const device = deviceRow(first.sipDeviceId)
            internal("store#SipRegistration", {change: "added", sipRegistrationId: "ring-leg-node", sipDeviceId: device.sipDeviceId, instanceId: LEG_NODE,
                contactUri: "sip:" + device.username + "@192.168.1.21:5060", transport: "udp", sourceAddress: "198.51.100.8", sourcePort: 61001,
                sipCallId: "reg-leg-node", cseq: 2, credentialRevision: device.credentialRevision || 1, expiresDate: Date.now() + 300000,
                lastRefreshDate: Date.now(), userAgent: "Spec Phone/1.0"})
            setProperty("rtc_cascade_enabled", "false")
            const off = as(B, "org.moqui.call.CallServices.start#Call", {calleeUserIdList: [A]})
            const offLegs = deviceLegs(off.callSessionId)
            as(B, "org.moqui.call.CallServices.end#Call", {callSessionId: off.callSessionId})
            setProperty("rtc_cascade_enabled", "true")
            const onCall = as(B, "org.moqui.call.CallServices.start#Call", {calleeUserIdList: [A]})
            const onLegs = deviceLegs(onCall.callSessionId)
            const session = sys(() => ec.entity.find("moqui.call.CallSession").condition("callSessionId", onCall.callSessionId).one())
            as(B, "org.moqui.call.CallServices.end#Call", {callSessionId: onCall.callSessionId})
            setProperty("rtc_cascade_enabled", "false")
            sys(() => {
                ec.entity.find("moqui.call.SipRegistration").condition("sipRegistrationId", "ring-leg-node").deleteAll()
                ec.entity.find("moqui.call.SipNode").condition("instanceId", LEG_NODE).deleteAll()
                ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", LEG_NODE).deleteAll()
            })
            return {offLegs, onLegs, session}
        })
    },
    then({offLegs, onLegs, session}) {
        session.mediaNodeId == NODE
        offLegs.length == 0
        onLegs.length == 1
        onLegs[0].mediaNodeId == LEG_NODE
        onLegs[0].statusId == "ClsOffered"
    }
})

feature("a tab's answer wins over the phone, and a phone that answers after another loses", {
    when() {
        return on(() => {
            const second = as(A, "create#SipDevice", {name: "Soft phone"})
            registered(deviceRow(first.sipDeviceId), "race-1")
            registered(deviceRow(second.sipDeviceId), "race-2")
            const byTab = as(B, "org.moqui.call.CallServices.start#Call", {calleeUserIdList: [A]})
            const tabAnswer = as(A, "org.moqui.call.CallServices.answer#Call", {callSessionId: byTab.callSessionId})
            const afterTab = deviceLegs(byTab.callSessionId)
            const late = note(byTab.callSessionId, afterTab[0].callLegId, "answered")
            as(B, "org.moqui.call.CallServices.end#Call", {callSessionId: byTab.callSessionId})
            const byPhones = as(B, "org.moqui.call.CallServices.start#Call", {calleeUserIdList: [A]})
            const two = deviceLegs(byPhones.callSessionId)
            const won = note(byPhones.callSessionId, two[0].callLegId, "answered")
            const lost = note(byPhones.callSessionId, two[1].callLegId, "answered")
            const phonesAfter = deviceLegs(byPhones.callSessionId)
            as(B, "org.moqui.call.CallServices.end#Call", {callSessionId: byPhones.callSessionId})
            as(A, "revoke#SipDevice", {sipDeviceId: second.sipDeviceId})
            return {tabAnswer, afterTab, late, two, won, lost, phonesAfter}
        })
    },
    then({tabAnswer, afterTab, late, two, phonesAfter}) {
        tabAnswer.errors == ""
        afterTab.length == 2
        afterTab.every((l) => l.statusId == "ClsCanceled")
        late.repeated == true
        two.length == 2
        phonesAfter.filter((l) => l.statusId == "ClsAnswered").length == 1
        phonesAfter.find((l) => l.callLegId == two[0].callLegId).statusId == "ClsAnswered"
        phonesAfter.find((l) => l.callLegId == two[1].callLegId).statusId == "ClsCanceled"
    }
})

feature("with phones and a voicemail box, tabs and phones ring together until voicemail, which cancels every phone; a decline (603) on a phone declines the ring and goes to voicemail, a busy phone (486) stops only its own ring", {
    when() {
        return on(() => {
            nodesUp()
            sys(() => {
                const number = ec.entity.makeValue("moqui.call.PhoneNumber")
                number.setFields({phoneNumber: NUMBER_A, routeTypeEnumId: "PnrUser", routeId: A}, true, null, null)
                number.create()
                const status = ec.entity.find("moqui.security.UserStatus").condition("userId", A).one()
                if (status) { status.set("inCall", "N"); status.update() }
            })
            const second = as(A, "create#SipDevice", {name: "Soft phone 2"})
            registered(deviceRow(first.sipDeviceId), "vm-1")
            registered(deviceRow(second.sipDeviceId), "vm-2")
            const rings = as(A, "org.moqui.call.VoicemailServices.update#VoicemailSettings", {rings: 10})
            const receive = () => internal("org.moqui.call.VoiceServices.receive#PhoneCall", {toNumber: NUMBER_A, fromNumber: CALLER,
                fromName: "Spec Caller", instanceId: NODE})
            const legsOfA = (callSessionId) => legsOf(callSessionId).filter((l) => l.legTypeEnumId !== "CltPstn")
            const logOf = (callSessionId) => sys(() => ec.entity.find("moqui.call.CallUserLog").condition({callSessionId, userId: A}).one())
            const sessionOf = (callSessionId) => sys(() => ec.entity.find("moqui.call.CallSession").condition("callSessionId", callSessionId).one())
            const ended = []

            // unanswered: rung 50 s, past call_ring_seconds (45) and before the box's 10 rings (60 s), every leg still rings
            const call = receive()
            ended.push(call.callSessionId)
            const rung = legsOfA(call.callSessionId)
            sys(() => {
                for (const leg of rung) {
                    leg.set("offeredDate", new Date(Date.now() - 50000))
                    leg.update()
                }
            })
            internal("org.moqui.call.CallServices.expire#Calls")
            const stillRinging = legsOfA(call.callSessionId)
            const begun = internal("org.moqui.call.VoicemailServices.begin#Voicemail", {callSessionId: call.callSessionId,
                callLegId: call.callLegId, reason: "noAnswer"})
            const afterVoicemail = legsOfA(call.callSessionId)
            const lateAnswer = note(call.callSessionId, rung.find((l) => l.legTypeEnumId === "CltSipDevice").callLegId, "answered")
            const missedLog = logOf(call.callSessionId)

            // V3: a phone busy on another call (486) ends only its own ring; the tab and the other phone ring on
            const busyCall = receive()
            ended.push(busyCall.callSessionId)
            const busyPhones = deviceLegs(busyCall.callSessionId)
            const busyNoted = note(busyCall.callSessionId, busyPhones[0].callLegId, "busy")
            const afterBusy = legsOfA(busyCall.callSessionId)
            const busyLog = logOf(busyCall.callSessionId)

            // declined on one phone (603): the tab and the other phone stop, and nobody else rings, so voicemail
            const declinedCall = receive()
            ended.push(declinedCall.callSessionId)
            const phones = deviceLegs(declinedCall.callSessionId)
            const refused = note(declinedCall.callSessionId, phones[0].callLegId, "declined")
            const afterDecline = legsOfA(declinedCall.callSessionId)
            const declinedSession = sessionOf(declinedCall.callSessionId)
            const declinedBegun = internal("org.moqui.call.VoicemailServices.begin#Voicemail", {callSessionId: declinedCall.callSessionId,
                callLegId: declinedCall.callLegId, reason: "declined"})
            const declinedLog = logOf(declinedCall.callSessionId)

            // a colleague's call declined on a phone ends, as one declined in a tab does
            const direct = as(B, "org.moqui.call.CallServices.start#Call", {calleeUserIdList: [A]})
            const directRefused = note(direct.callSessionId, deviceLegs(direct.callSessionId)[0].callLegId, "declined")
            const directSession = sessionOf(direct.callSessionId)

            sys(() => {
                for (const id of ended) {
                    const s = ec.entity.find("moqui.call.CallSession").condition("callSessionId", id).one()
                    if (s && s.statusId !== "CssEnded") { s.set("statusId", "CssEnded"); s.update() }
                }
            })
            as(A, "revoke#SipDevice", {sipDeviceId: second.sipDeviceId})
            return {rings, call, rung, stillRinging, begun, afterVoicemail, lateAnswer, missedLog, phones, refused, afterDecline,
                declinedSession, declinedBegun, declinedLog, directRefused, directSession, busyPhones, busyNoted, afterBusy, busyLog}
        })
    },
    then({rings, call, rung, stillRinging, begun, afterVoicemail, lateAnswer, missedLog, phones, refused, afterDecline,
          declinedSession, declinedBegun, declinedLog, directRefused, directSession, busyPhones, busyNoted, afterBusy, busyLog}) {
        busyNoted.errors == ""
        afterBusy.find((l) => l.callLegId == busyPhones[0].callLegId).statusId == "ClsDeclined"
        afterBusy.filter((l) => l.callLegId != busyPhones[0].callLegId).every((l) => l.statusId == "ClsOffered")
        busyLog.outcomeEnumId == null
        rings.box.rings == 10
        call.voicemailSeconds == 60
        // the tab's leg and both phones
        rung.length == 3
        rung.filter((l) => l.legTypeEnumId == "CltSipDevice").length == 2
        stillRinging.every((l) => l.statusId == "ClsOffered")
        begun.errors == ""
        begun.mode == "deposit"
        afterVoicemail.every((l) => l.statusId == "ClsTimedOut")
        lateAnswer.repeated == true
        missedLog.outcomeEnumId == "CuoMissed"
        phones.length == 2
        refused.errors == ""
        afterDecline.find((l) => l.callLegId == phones[0].callLegId).statusId == "ClsDeclined"
        afterDecline.filter((l) => l.callLegId != phones[0].callLegId).every((l) => l.statusId == "ClsCanceled")
        afterDecline.length == 3
        declinedSession.statusId != "CssEnded"
        declinedBegun.errors == ""
        declinedBegun.mode == "deposit"
        declinedLog.outcomeEnumId == "CuoDeclined"
        directRefused.errors == ""
        directSession.statusId == "CssEnded"
        directSession.endReasonEnumId == "CerDeclined"
    }
})

feature("a registered phone calls another user by username, as its user", {
    when() {
        return on(() => {
            nodesUp()
            const placed = internal("receive#DeviceCall", {sipDeviceId: first.sipDeviceId, toUser: username(B), instanceId: NODE})
            const legs = placed.callSessionId ? legsOf(placed.callSessionId) : []
            const session = placed.callSessionId ? sys(() => ec.entity.find("moqui.call.CallSession").condition("callSessionId", placed.callSessionId).one()) : null
            if (placed.callSessionId) as(A, "org.moqui.call.CallServices.end#Call", {callSessionId: placed.callSessionId})
            return {placed, legs, session}
        })
    },
    then({placed, legs, session}) {
        placed.errors == ""
        placed.callLegId != null
        session.startedByUserId == A
        session.mediaNodeId == NODE
        legs.find((l) => l.callLegId == placed.callLegId).legTypeEnumId == "CltSipDevice"
        legs.find((l) => l.callLegId == placed.callLegId).statusId == "ClsAnswered"
        legs.some((l) => l.legTypeEnumId == "CltWeb" && l.statusId == "ClsOffered")
    }
})

feature("revocation ends the phone: no verifiers, no registrations, no ringing, no calls", {
    when() {
        return on(() => {
            registered(deviceRow(first.sipDeviceId), "rev-1")
            const revoked = as(A, "revoke#SipDevice", {sipDeviceId: first.sipDeviceId})
            const again = as(A, "revoke#SipDevice", {sipDeviceId: first.sipDeviceId})
            const auth = internal("get#SipDeviceAuth", {username: first.username, realm: "moqui"})
            const kept = registered(deviceRow(first.sipDeviceId), "rev-2")
            const call = as(B, "org.moqui.call.CallServices.start#Call", {calleeUserIdList: [A]})
            const placed = internal("receive#DeviceCall", {sipDeviceId: first.sipDeviceId, toUser: username(B), instanceId: NODE})
            as(B, "org.moqui.call.CallServices.end#Call", {callSessionId: call.callSessionId})
            return {revoked, again, auth, rung: deviceLegs(call.callSessionId), placed, listed: as(A, "find#SipDevices"),
                left: registrations(first.sipDeviceId), kept}
        })
    },
    then({revoked, again, auth, rung, placed, listed, left}) {
        revoked.errors == ""
        again.repeated == true
        auth.sipDeviceId == null
        left == 0
        rung.length == 0
        placed.reasonCode == "notAllowed"
        listed.deviceList.length == 0
    }
})
