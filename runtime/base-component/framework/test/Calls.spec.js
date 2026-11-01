// Calls (org.moqui.impl.CallServices and Calls.js; framework/entity/CallEntities.xml; plans/Comms-Framework.md §5.4,
// §6.3, W1): starting, ringing, the first answer winning, repeated requests, binding a socket with a fenced generation,
// presence across two calls, leaving and ending, declining, ringing out, a lost connection and a lost media node, and a
// room's group call. With cascading media nodes (W7, rtc_cascade_enabled) a leg's media goes to its signaling node, an
// edge, unless that is no live media node or its relay was refused; losing the edge ends only its legs. Every refusal has
// its negative user with the same parameters as the positive one.
// Run: ./moq test framework --spec Calls
const A = "CALLS_SPEC_A"
const B = "CALLS_SPEC_B"
const C = "CALLS_SPEC_C"
const D = "CALLS_SPEC_D"
const NODE = "calls-spec-node"
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
function as(userId, name, parameters) {
    ec.user.internalLoginUser(userId.toLowerCase().replace(/_/g, "."))
    let out = {}
    let thrown = ""
    try {
        // as the Calls screens' transitions call them: the services check the signed-in user's part in each call
        out = sys(() => ec.service.sync().name("org.moqui.impl.CallServices." + name).parameters(parameters || {}).call()) || {}
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
        const username = userId.toLowerCase().replace(/_/g, ".")
        account.setFields({userId, username, userFullName: "Calls " + userId.slice(-1), disabled: "N"}, true, null, null)
        account.create()
    })
}
const nodeUp = () => sys(() => call("org.moqui.impl.CallServices.register#MediaNode", {instanceId: NODE, mediaAddress: "127.0.0.1:10000"}))
const session = id => sys(() => ec.entity.find(SESSION).condition("callSessionId", id).one())
const legs = id => sys(() => Array.from(ec.entity.find(LEG).condition("callSessionId", id).orderBy("offeredDate").list()))
const log = (id, userId) => sys(() => ec.entity.find("moqui.call.CallUserLog").condition({callSessionId: id, userId}).one())
const inCall = userId => sys(() => ec.entity.find("moqui.security.UserStatus").condition("userId", userId).one()?.inCall || "N")
const expire = () => sys(() => call("org.moqui.impl.CallServices.expire#Calls", {}))
function age(entity, fields, field, millis) {
    sys(() => {
        for (const row of ec.entity.find(entity).condition(fields).list()) {
            row.set(field, new Date(Date.now() - millis))
            row.update()
        }
    })
}
function endAll() {
    sys(() => {
        for (const s of ec.entity.find(SESSION).condition("startedByUserId", "in", [A, B, C, D]).condition("statusId", "!=", "CssEnded").list()) {
            s.set("statusId", "CssEnded")
            s.update()
        }
        for (const l of ec.entity.find(LEG).condition("statusId", "in", ["ClsOffered", "ClsAnswered"]).list()) {
            const p = ec.entity.find("moqui.call.CallParticipant").condition("callParticipantId", l.callParticipantId).one()
            if ([A, B, C, D].includes(p?.userId)) {
                l.set("statusId", "ClsLeft")
                l.update()
            }
        }
    })
}

spec({
    stepwise: true,
    setupSpec() {
        for (const id of [A, B, C, D]) makeUser(id)
        sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", NODE).deleteAll())
        endAll()
    },
    cleanupSpec() {
        endAll()
        sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", NODE).deleteAll())
    }
})

let direct = null
let room = null

feature("with no media server a call is refused, and the capabilities say why", {
    when() {
        const live = sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("heartbeatDate", ">=", new Date(Date.now() - 60000)).count())
        return {live, caps: as(A, "get#Capabilities"), started: as(A, "start#Call", {calleeUserIdList: [B]})}
    },
    then({live, caps, started}) {
        live > 0 || caps.active == false
        live > 0 || started.reasonCode == "mediaUnavailable"
        live > 0 || started.errors.includes("no media server")
    }
})

feature("a direct call rings its callee, through their direct room", {
    when() {
        nodeUp()
        const started = as(A, "start#Call", {calleeUserIdList: [B], idempotencyKey: "spec-direct-1"})
        direct = started.callSessionId
        const again = as(A, "start#Call", {calleeUserIdList: [B], idempotencyKey: "spec-direct-1"})
        const seenByB = as(B, "get#Call", {callSessionId: direct})
        const seenByC = as(C, "get#Call", {callSessionId: direct})
        const ringing = as(B, "find#ActiveCalls")
        const events = sys(() => ec.entity.find("moqui.chat.ChatEvent").condition("chatRoomId", started.call.chatRoomId)
            .condition("eventTypeEnumId", "CetCall").count())
        return {started, again, seenByB, seenByC, ringing, events, row: session(direct), legs: legs(direct)}
    },
    then({started, again, seenByB, seenByC, ringing, events, row, legs}) {
        started.errors == ""
        started.call.status == "ringing"
        started.call.kind == "direct"
        started.call.myLegStatus == "answered"
        started.callLegId == started.call.myLegId
        row.mediaNodeId != null
        again.repeated == true
        again.callSessionId == direct
        legs.length == 2
        seenByB.call.myLegStatus == "offered"
        seenByC.reasonCode == "notFound"
        ringing.callList.some(c => c.callSessionId == direct)
        events == 1
        inCall(A) == "Y"
    }
})

feature("the first answer wins and a second is the same answer; someone else cannot answer", {
    when() {
        const other = as(C, "answer#Call", {callSessionId: direct})
        const first = as(B, "answer#Call", {callSessionId: direct})
        const second = as(B, "answer#Call", {callSessionId: direct})
        return {other, first, second, row: session(direct)}
    },
    then({other, first, second, row}) {
        other.reasonCode == "notFound"
        first.errors == ""
        first.call.status == "active"
        second.repeated == true
        second.callLegId == first.callLegId
        row.statusId == "CssActive"
        Number(row.revision) == 2
        inCall(B) == "Y"
    }
})

feature("binding a socket claims the next generation and fences the older one", {
    when() {
        const myLeg = as(A, "get#Call", {callSessionId: direct}).call.myLegId
        const first = as(A, "bind#Connection", {callSessionId: direct, callLegId: myLeg})
        const second = as(A, "bind#Connection", {callSessionId: direct, callLegId: myLeg})
        const stranger = as(C, "bind#Connection", {callSessionId: direct, callLegId: myLeg})
        const otherSide = as(B, "bind#Connection", {callSessionId: direct, callLegId: myLeg})
        const stale = sys(() => call("org.moqui.impl.CallServices.release#Connection", {callLegId: myLeg, generation: first.generation}))
        const conns = sys(() => Array.from(ec.entity.find("moqui.call.CallConnection").condition("callLegId", myLeg).orderBy("generation").list()))
        return {first, second, stranger, otherSide, stale, conns}
    },
    then({first, second, stranger, otherSide, stale, conns}) {
        first.generation == 1
        second.generation == 2
        first.mediaNodeId != null
        stranger.reasonCode == "notFound"
        otherSide.reasonCode == "notFound"
        stale.released == false
        conns[0].statusId == "CcsReplaced"
        conns[1].statusId == "CcsActive"
    }
})

feature("presence stays in a call while one of two calls ends", {
    when() {
        const second = as(A, "start#Call", {calleeUserIdList: [C]})
        const endSecond = as(A, "leave#Call", {callSessionId: second.callSessionId})
        return {second, endSecond, a: inCall(A), c: log(second.callSessionId, C)}
    },
    then({second, endSecond, a, c}) {
        second.errors == ""
        endSecond.call.status == "ended"
        endSecond.call.endReason == "canceled"
        a == "Y"
        c.outcomeEnumId == "CuoMissed"
    }
})

feature("leaving a direct call ends it, and each side has its history", {
    when() {
        const left = as(B, "leave#Call", {callSessionId: direct})
        const again = as(B, "leave#Call", {callSessionId: direct})
        const join = as(A, "join#Call", {callSessionId: direct})
        return {left, again, join, row: session(direct), legs: legs(direct), a: log(direct, A), b: log(direct, B)}
    },
    then({left, again, join, row, legs, a, b}) {
        left.call.status == "ended"
        again.repeated == true
        join.reasonCode == "callEnded"
        row.endReasonEnumId == "CerHangup"
        legs.every(l => l.statusId == "ClsLeft")
        a.outcomeEnumId == "CuoPlaced"
        b.outcomeEnumId == "CuoAnswered"
        inCall(A) == "N"
        inCall(B) == "N"
    }
})

feature("a declined direct call ends; only its callee may decline it", {
    when() {
        const started = as(A, "start#Call", {calleeUserIdList: [B]})
        const stranger = as(C, "decline#Call", {callSessionId: started.callSessionId})
        const declined = as(B, "decline#Call", {callSessionId: started.callSessionId})
        return {stranger, declined, row: session(started.callSessionId), b: log(started.callSessionId, B)}
    },
    then({stranger, declined, row, b}) {
        stranger.reasonCode == "notFound"
        declined.call.status == "ended"
        row.endReasonEnumId == "CerDeclined"
        b.outcomeEnumId == "CuoDeclined"
    }
})

feature("a call that rings out is missed", {
    when() {
        const started = as(A, "start#Call", {calleeUserIdList: [C]})
        age(SESSION, {callSessionId: started.callSessionId}, "ringExpireDate", 1000)
        const expired = expire()
        return {expired, row: session(started.callSessionId), c: log(started.callSessionId, C), legs: legs(started.callSessionId)}
    },
    then({expired, row, c, legs}) {
        expired.endedCount >= 1
        row.endReasonEnumId == "CerNoAnswer"
        c.outcomeEnumId == "CuoMissed"
        legs.some(l => l.statusId == "ClsTimedOut")
    }
})

feature("a leg whose socket did not come back leaves, and the call ends", {
    when() {
        const started = as(A, "start#Call", {calleeUserIdList: [B]})
        const answered = as(B, "answer#Call", {callSessionId: started.callSessionId})
        const bound = as(B, "bind#Connection", {callSessionId: started.callSessionId, callLegId: answered.callLegId})
        sys(() => call("org.moqui.impl.CallServices.release#Connection", {callLegId: answered.callLegId, generation: bound.generation}))
        age("moqui.call.CallConnection", {callLegId: answered.callLegId}, "closedDate", 60000)
        const expired = expire()
        return {expired, row: session(started.callSessionId)}
    },
    then({expired, row}) {
        expired.legCount >= 1
        row.statusId == "CssEnded"
        row.endReasonEnumId == "CerConnectionLost"
    }
})

feature("a call whose media server stopped ends with that reason", {
    when() {
        const started = as(A, "start#Call", {calleeUserIdList: [B]})
        age("moqui.call.CallMediaNode", {instanceId: NODE}, "heartbeatDate", 120000)
        sys(() => {
            const s = ec.entity.find(SESSION).condition("callSessionId", started.callSessionId).one()
            s.set("mediaNodeId", NODE)
            s.update()
        })
        const expired = expire()
        nodeUp()
        return {expired, row: session(started.callSessionId)}
    },
    then({expired, row}) {
        expired.endedCount >= 1
        row.endReasonEnumId == "CerMediaNodeLost"
    }
})

feature("a room's group call: members join, a non-member cannot, the host's leaving keeps it, the last leg ends it", {
    when() {
        ec.user.internalLoginUser("calls.spec.a")
        room = sys(() => ec.service.sync().name("org.moqui.impl.ChatServices.create#Room")
            .parameters({roomTypeEnumId: "CrtPrivate", roomName: "Calls spec", memberUserIdList: [B, C]}).call()).chatRoomId
        ec.message.clearErrors()
        const started = as(A, "start#Call", {chatRoomId: room})
        const id = started.callSessionId
        const again = as(B, "start#Call", {chatRoomId: room})
        const outsider = as(D, "join#Call", {callSessionId: id})
        const outsiderStart = as(D, "start#Call", {chatRoomId: room})
        const b = as(B, "join#Call", {callSessionId: id})
        const c = as(C, "join#Call", {callSessionId: id})
        const notHost = as(B, "end#Call", {callSessionId: id})
        const hostLeft = as(A, "leave#Call", {callSessionId: id})
        const bLeft = as(B, "leave#Call", {callSessionId: id})
        const cLeft = as(C, "leave#Call", {callSessionId: id})
        return {started, again, outsider, outsiderStart, b, c, notHost, hostLeft, bLeft, cLeft, row: session(id)}
    },
    then({started, again, outsider, outsiderStart, b, c, notHost, hostLeft, bLeft, cLeft, row}) {
        started.call.kind == "group"
        started.call.status == "active"
        again.reasonCode == "callActive"
        again.callSessionId == started.callSessionId
        outsider.reasonCode == "notFound"
        outsiderStart.reasonCode == "notFound"
        b.call.myLegStatus == "answered"
        c.call.myLegStatus == "answered"
        notHost.reasonCode == "notAllowed"
        hostLeft.call.status == "active"
        bLeft.call.status == "active"
        cLeft.call.status == "ended"
        row.endReasonEnumId == "CerEmpty"
    }
})

feature("each user's history has their outcome of each call, and missed calls count until seen", {
    when() {
        const missedBefore = as(C, "count#MissedCalls")
        const cMissed = as(C, "find#Calls", {outcome: "missed"})
        const aPlaced = as(A, "find#Calls", {outcome: "placed", pageSize: 3})
        const bAll = as(B, "find#Calls", {})
        const marked = as(C, "mark#MissedSeen")
        const missedAfter = as(C, "count#MissedCalls")
        return {missedBefore, cMissed, aPlaced, bAll, marked, missedAfter}
    },
    then({missedBefore, cMissed, aPlaced, bAll, marked, missedAfter}) {
        missedBefore.missedCount >= 2
        cMissed.callList.every(c => c.outcome == "missed" && c.unseen && c.otherNames.includes("Calls A"))
        cMissed.callListCount == missedBefore.missedCount
        aPlaced.callList.length == 3
        aPlaced.callListCount > 3
        aPlaced.callList.every(c => c.outcome == "placed" && c.role == "caller")
        bAll.callList.some(c => c.outcome == "answered" && c.callSessionId == direct)
        bAll.callList.some(c => c.outcome == "declined")
        marked.markedCount == missedBefore.missedCount
        missedAfter.missedCount == 0
    }
})

// as a guest's page and the /call/<token> route call them: anonymous, by token and credential
function anon(name, parameters) {
    ec.user.logoutUser()
    let out = {}
    let thrown = ""
    try {
        out = ec.service.sync().name("org.moqui.impl.CallServices." + name).parameters(parameters || {}).call() || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
// as the host's /callws calls them: internal work, the refusals answered
function internal(name, parameters) {
    let out = {}
    let thrown = ""
    try {
        out = sys(() => ec.service.sync().name("org.moqui.impl.CallServices." + name).parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
let link = null
let guest = null

feature("a call link takes a guest by name into its meeting's lobby, and only someone in the call lets them in", {
    when() {
        link = as(A, "create#CallLink", {name: "Spec meeting", policy: "lobby"})
        const bad = anon("check#CallLinkToken", {token: "not-a-real-token-at-all"})
        const good = anon("check#CallLinkToken", {token: link.token})
        guest = anon("join#AsGuest", {token: link.token, displayName: "Visitor"})
        const noName = anon("join#AsGuest", {token: link.token, displayName: "  "})
        const waiting = anon("get#GuestState", {credential: guest.credential})
        const earlyBind = internal("bind#GuestConnection", {credential: guest.credential, callSessionId: guest.callSessionId, callLegId: guest.callLegId})
        ec.message.clearErrors()
        const view = as(A, "get#Call", {callSessionId: guest.callSessionId})
        const waitingParticipant = view.call.participants.find(p => p.admission == "waiting")
        const stranger = as(D, "admit#Guest", {callSessionId: guest.callSessionId, callParticipantId: waitingParticipant.callParticipantId})
        const opened = as(A, "open#CallLink", {callLinkId: link.callLinkId})
        const admitted = as(A, "admit#Guest", {callSessionId: guest.callSessionId, callParticipantId: waitingParticipant.callParticipantId})
        const inState = anon("get#GuestState", {credential: guest.credential})
        const bound = internal("bind#GuestConnection", {credential: guest.credential, callSessionId: guest.callSessionId, callLegId: guest.callLegId})
        const wrongCall = internal("bind#GuestConnection", {credential: guest.credential, callSessionId: direct, callLegId: guest.callLegId})
        ec.message.clearErrors()
        const links = as(A, "find#CallLinks")
        return {bad, good, noName, waiting, earlyBind, stranger, opened, admitted, inState, bound, wrongCall, links}
    },
    then({bad, good, noName, waiting, earlyBind, stranger, opened, admitted, inState, bound, wrongCall, links}) {
        link.path == "/call/" + link.token
        bad.valid == false
        good.valid == true
        good.routeContext.name == "Spec meeting"
        good.routeContext.policy == "lobby"
        guest.admission == "waiting"
        guest.credential.length >= 20
        noName.reasonCode == "noName"
        waiting.admission == "waiting"
        waiting.people.length == 0
        earlyBind.reasonCode == "notAdmitted"
        stranger.reasonCode == "notFound"
        opened.callSessionId == guest.callSessionId
        opened.call.myLegStatus == "answered"
        opened.call.participants.some(p => p.userId == A && p.role == "host")
        admitted.call.participants.some(p => p.displayName == "Visitor" && p.admission == "admitted")
        inState.admission == "admitted"
        inState.people.some(p => p.displayName == "Visitor")
        bound.generation == 1
        wrongCall.reasonCode == "notFound"
        links.linkList.some(l => l.callLinkId == link.callLinkId && l.callSessionId == guest.callSessionId)
    }
})

feature("revoking the link removes its guest, whose credential then works nowhere", {
    when() {
        const notOwner = as(B, "revoke#CallLink", {callLinkId: link.callLinkId})
        const revoked = as(A, "revoke#CallLink", {callLinkId: link.callLinkId})
        const state = anon("get#GuestState", {credential: guest.credential})
        const bind = internal("bind#GuestConnection", {credential: guest.credential, callSessionId: guest.callSessionId, callLegId: guest.callLegId})
        ec.message.clearErrors()
        const again = anon("join#AsGuest", {token: link.token, displayName: "Visitor"})
        const view = as(A, "get#Call", {callSessionId: guest.callSessionId})
        return {notOwner, revoked, state, bind, again, view}
    },
    then({notOwner, revoked, state, bind, again, view}) {
        notOwner.reasonCode == "notFound"
        revoked.revoked == true
        state.admission == "gone"
        bind.reasonCode == "notFound"
        again.reasonCode == "notFound"
        view.call.participants.some(p => p.displayName == "Visitor" && p.admission == "revoked" && !p.inCall)
        view.call.status == "active"
    }
})

feature("a link that lets guests straight in, guests who leave, and a meeting nobody is in ends", {
    when() {
        const open = as(A, "create#CallLink", {name: "Open door", policy: "link"})
        const g = anon("join#AsGuest", {token: open.token, displayName: "Walk-in"})
        const left = anon("leave#AsGuest", {credential: g.credential})
        const after = anon("get#GuestState", {credential: g.credential})
        // a guest who gives up in the lobby leaves a meeting with nobody in it: the cleanup ends it
        const lobby = as(A, "create#CallLink", {name: "Waiting room"})
        const w = anon("join#AsGuest", {token: lobby.token, displayName: "Patient"})
        anon("leave#AsGuest", {credential: w.credential})
        const before = session(w.callSessionId)
        age(SESSION, {callSessionId: w.callSessionId}, "startedDate", 300000)
        const expired = expire()
        return {g, left, after, row: session(g.callSessionId), before, expired, emptied: session(w.callSessionId)}
    },
    then({g, left, after, row, before, expired, emptied}) {
        g.admission == "admitted"
        left.left == true
        after.admission == "gone"
        row.statusId == "CssEnded"
        row.endReasonEnumId == "CerEmpty"
        before.statusId == "CssActive"
        expired.endedCount >= 1
        emptied.endReasonEnumId == "CerEmpty"
    }
})

const EDGE = "calls-spec-edge"
feature("with cascading media nodes a leg's media is on its signaling node, and losing that node ends only its legs", {
    when() {
        nodeUp()
        sys(() => call("org.moqui.impl.CallServices.register#MediaNode", {instanceId: EDGE, mediaAddress: "127.0.0.1:10002"}))
        const started = as(A, "start#Call", {calleeUserIdList: [B, C]})
        const id = started.callSessionId
        sys(() => {
            const s = ec.entity.find(SESSION).condition("callSessionId", id).one()
            s.set("mediaNodeId", NODE)
            s.update()
        })
        const bLeg = as(B, "answer#Call", {callSessionId: id}).callLegId
        const cLeg = as(C, "answer#Call", {callSessionId: id}).callLegId
        // off: every leg's media is on the call's node, whatever its socket's
        setProperty("rtc_cascade_enabled", "false")
        const off = as(B, "bind#Connection", {callSessionId: id, callLegId: bLeg, signalingNodeId: EDGE})
        setProperty("rtc_cascade_enabled", "true")
        const onEdge = as(B, "bind#Connection", {callSessionId: id, callLegId: bLeg, signalingNodeId: EDGE})
        const stranger = as(D, "bind#Connection", {callSessionId: id, callLegId: bLeg, signalingNodeId: EDGE})
        const noMedia = as(A, "bind#Connection", {callSessionId: id, callLegId: started.callLegId, signalingNodeId: "calls-spec-nowhere"})
        // the call's node refused B's edge's relay: B's media goes back to the call's node, and stays there
        sys(() => call("org.moqui.impl.CallServices.note#MediaEvent", {callLegId: bLeg, generation: onEdge.generation, event: "relayRefused"}))
        const pinned = as(B, "bind#Connection", {callSessionId: id, callLegId: bLeg, signalingNodeId: EDGE})
        const cOnEdge = as(C, "bind#Connection", {callSessionId: id, callLegId: cLeg, signalingNodeId: EDGE})
        // the edge is lost: C's leg leaves with that reason; the call and the others go on
        age("moqui.call.CallMediaNode", {instanceId: EDGE}, "heartbeatDate", 120000)
        const expired = expire()
        setProperty("rtc_cascade_enabled", "false")
        const leg = (legId) => sys(() => ec.entity.find(LEG).condition("callLegId", legId).one())
        const out = {off, onEdge, stranger, noMedia, pinned, cOnEdge, expired, row: session(id), b: leg(bLeg), c: leg(cLeg)}
        as(A, "end#Call", {callSessionId: id})
        sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", EDGE).deleteAll())
        return out
    },
    then({off, onEdge, stranger, noMedia, pinned, cOnEdge, expired, row, b, c}) {
        off.mediaNodeId == NODE
        off.originNodeId == NODE
        onEdge.mediaNodeId == EDGE
        onEdge.originNodeId == NODE
        stranger.reasonCode == "notFound"
        noMedia.mediaNodeId == NODE
        pinned.mediaNodeId == NODE
        b.mediaNodePinned == "Y"
        cOnEdge.mediaNodeId == EDGE
        expired.legCount >= 1
        row.statusId == "CssActive"
        c.statusId == "ClsLeft"
        c.endReasonEnumId == "CerMediaNodeLost"
        b.statusId == "ClsAnswered"
    }
})

feature("a leg in the call gets its TURN servers with a credential of its own; nobody else does", {
    when() {
        sys(() => call("org.moqui.impl.CallServices.register#MediaNode", {instanceId: NODE, mediaAddress: "127.0.0.1:10000",
            turnUrls: "turn:127.0.0.1:3478?transport=udp,turns:127.0.0.1:5349?transport=tcp"}))
        const started = as(A, "start#Call", {calleeUserIdList: [B]})
        sys(() => {
            const s = ec.entity.find(SESSION).condition("callSessionId", started.callSessionId).one()
            s.set("mediaNodeId", NODE)
            s.update()
        })
        const mine = as(A, "get#IceServers", {callSessionId: started.callSessionId})
        const ringing = as(B, "get#IceServers", {callSessionId: started.callSessionId})
        const stranger = as(D, "get#IceServers", {callSessionId: started.callSessionId})
        as(A, "end#Call", {callSessionId: started.callSessionId})
        return {started, mine, ringing, stranger}
    },
    then({started, mine, ringing, stranger}) {
        mine.iceServers.length == 1
        mine.iceServers[0].urls.length == 2
        mine.iceServers[0].username.endsWith(":" + started.callLegId)
        mine.iceServers[0].credential.length > 20
        ringing.iceServers.length == 0
        ringing.reasonCode == "notAdmitted"
        stranger.reasonCode == "notFound"
    }
})
