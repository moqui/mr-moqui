// Calls (org.moqui.call.CallServices and Calls.js; framework/entity/CallEntities.xml; plans/archive/Comms-Framework.md §5.4,
// §6.3, W1): starting, ringing, the first answer winning, repeated requests, binding a socket with a fenced generation,
// presence across two calls, leaving and ending, declining, ringing out, a lost connection and a lost media node, and a
// room's group call. With cascading media nodes (W7, media-server@cascade-enabled) a leg's media goes to its signaling node, an
// leg node, unless that is no live media node or its relay was refused; losing the leg node ends only its legs. Every refusal has
// its negative user with the same parameters as the positive one.
// Run: ./moq test framework --spec Calls
const A = "CALLS_SPEC_A"
const B = "CALLS_SPEC_B"
const C = "CALLS_SPEC_C"
const D = "CALLS_SPEC_D"
const NODE = "calls-spec-node"
const EDGE_NODE = "calls-spec-edge-node"
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
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : "org.moqui.call.CallServices." + name).parameters(parameters || {}).call()) || {}
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
const nodeUp = () => sys(() => call("org.moqui.call.CallServices.register#MediaNode", {instanceId: NODE, mediaAddress: "127.0.0.1:10000"}))
const session = id => sys(() => ec.entity.find(SESSION).condition("callSessionId", id).one())
const legs = id => sys(() => Array.from(ec.entity.find(LEG).condition("callSessionId", id).orderBy("offeredDate").list()))
const log = (id, userId) => sys(() => ec.entity.find("moqui.call.CallUserLog").condition({callSessionId: id, userId}).one())
const inCall = userId => sys(() => ec.entity.find("moqui.security.UserStatus").condition("userId", userId).one()?.inCall || "N")
const expire = () => sys(() => call("org.moqui.call.CallServices.expire#Calls", {}))
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
        sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", "in", [NODE, EDGE_NODE]).deleteAll())
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
        const stale = sys(() => call("org.moqui.call.CallServices.release#Connection", {callLegId: myLeg, generation: first.generation}))
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

feature("the session arbitrates presentation grants, takeover, host stop and approval", {
    when() {
        const binding = user => {
            const callLegId = as(user, "get#Call", {callSessionId:direct}).call.myLegId
            return {callLegId, generation:as(user, "bind#Connection", {callSessionId:direct, callLegId}).generation}
        }
        const host = binding(A), member = binding(B)
        const act = (who, leg, action, extra = {}) => as(who, "org.moqui.call.StageServices.change#CallStage",
                {callSessionId:direct, ...leg, action, ...extra})
        const setMode = shareModeEnumId => as(A, "set#CallShareMode", {callSessionId:direct, shareModeEnumId})
        const first = act(A, host, "ask")
        const untrustedInk = act(B, member, "inkMode", {mode:"everyone"})
        const inkMode = act(A, host, "inkMode", {mode:"hosts"})
        const embedded = act(A, host, "inkSurface", {embedded:true})
        const confirm = act(B, member, "ask")
        const fromHost = act(B, member, "take", {previousLegId:host.callLegId})
        const impostor = act(A, member, "stop")
        const stop = act(A, host, "stop")
        // a host takes over from a member, and the member's share annotation goes with their share
        act(B, member, "ask")
        act(B, member, "inkMode", {mode:"everyone"})
        act(B, member, "inkSurface", {embedded:true})
        const takeover = act(A, host, "take")
        act(A, host, "stop")
        const hosts = setMode("CsmHosts")
        const barred = act(B, member, "ask")
        setMode("CsmAsk")
        const queued = act(B, member, "ask")
        const approved = act(A, host, "approve", {targetLegId:member.callLegId})
        const view = as(A, "get#Call", {callSessionId:direct}).call
        const follow = act(A, host, "take", {kind:"follow"})
        act(A, host, "stop")
        setMode("CsmAnyone")
        return {first, untrustedInk, inkMode, embedded, confirm, fromHost, takeover, impostor, stop, hosts, barred, queued, approved, view, follow, host, member}
    },
    then(r) {
        r.first.stage.callLegId === r.host.callLegId
        r.confirm.reasonCode === "notAllowed" && r.fromHost.reasonCode === "notAllowed"
        r.follow.stage.kind === "follow" && r.follow.stage.followEpoch > 0
        r.untrustedInk.reasonCode === "notAllowed" && r.inkMode.stage.inkMode === "hosts" && r.embedded.stage.inkEmbedded === true
        !r.takeover.stage.inkEmbedded && !r.takeover.stage.inkMode
        r.takeover.stage.callLegId === r.host.callLegId && r.impostor.reasonCode === "notAllowed"
        r.stop.stage.kind == null && r.hosts.call.shareModeEnumId === "CsmHosts" && r.barred.reasonCode === "notAllowed"
        r.queued.stage.kind == null && r.queued.stage.requests.includes(r.member.callLegId)
        r.approved.stage.callLegId === r.member.callLegId && r.view.stage.callLegId === r.member.callLegId
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

// The direct path (plans/Comms-Messaging.md §8.3 item 15): a one-to-one call of two users goes peer to peer; either
// one's Always relay my calls, or a third person rung in, keeps it on the media server, and a socket that asks for the
// direct path then is refused.
feature("a one-to-one call goes direct unless either person always relays or a third person is rung, and a media-server join moves it for good", {
    when() {
        endAll()
        const started = as(A, "start#Call", {calleeUserIdList: [B]})
        const id = started.callSessionId
        const answered = as(B, "answer#Call", {callSessionId: id})
        const before = as(A, "get#Call", {callSessionId: id}).call.direct
        const bound = as(B, "bind#Connection", {callSessionId: id, callLegId: answered.callLegId, direct: true})
        const relay = as(B, "set#CallPreferences", {alwaysRelay: true})
        const relayed = as(A, "get#Call", {callSessionId: id}).call.direct
        const refused = as(B, "bind#Connection", {callSessionId: id, callLegId: answered.callLegId, direct: true})
        const plain = as(B, "bind#Connection", {callSessionId: id, callLegId: answered.callLegId})
        const back = as(B, "set#CallPreferences", {alwaysRelay: false})
        const prefs = as(A, "get#CallPreferences", {})
        const again = as(A, "get#Call", {callSessionId: id}).call.direct
        sys(() => call("org.moqui.call.CallTransferServices.add#CallParticipant", {callSessionId: id, userId: C}))
        ec.user.internalLoginUser(A.toLowerCase().replace(/_/g, "."))
        const third = sys(() => call("org.moqui.call.CallServices.get#Call", {callSessionId: id})).call
        endAll()
        // a page that joins a direct call through the media server moves it there for good, for both pages
        const next = as(A, "start#Call", {calleeUserIdList: [B]})
        const nextAnswer = as(B, "answer#Call", {callSessionId: next.callSessionId})
        as(B, "bind#Connection", {callSessionId: next.callSessionId, callLegId: nextAnswer.callLegId, direct: true})
        const moved = as(A, "bind#Connection", {callSessionId: next.callSessionId, callLegId: next.callLegId})
        const movedView = as(B, "get#Call", {callSessionId: next.callSessionId}).call
        const backDirect = as(B, "bind#Connection", {callSessionId: next.callSessionId, callLegId: nextAnswer.callLegId, direct: true})
        const relayedDate = session(next.callSessionId).relayedDate
        endAll()
        return {before, bound, relay, relayed, refused, plain, back, prefs, again, third, moved, movedView, backDirect, relayedDate}
    },
    then({before, bound, relay, relayed, refused, plain, back, prefs, again, third, moved, movedView, backDirect, relayedDate}) {
        moved.generation == 1 && moved.call.direct == false
        movedView.direct == false
        backDirect.reasonCode == "notDirect"
        relayedDate != null
        before == true
        bound.direct == true && bound.generation == 1
        relay.alwaysRelay == true
        relayed == false
        refused.reasonCode == "notDirect"
        plain.generation == 2 && !plain.direct
        back.alwaysRelay == false && prefs.alwaysRelay == false && prefs.directPathEnabled == true
        again == true
        third.direct == false
    }
})

feature("a leg whose socket did not come back in a minute leaves, and the call ends", {
    when() {
        const started = as(A, "start#Call", {calleeUserIdList: [B]})
        const answered = as(B, "answer#Call", {callSessionId: started.callSessionId})
        const bound = as(B, "bind#Connection", {callSessionId: started.callSessionId, callLegId: answered.callLegId})
        sys(() => call("org.moqui.call.CallServices.release#Connection", {callLegId: answered.callLegId, generation: bound.generation}))
        // gone 40 s (a phone's network back within the minute, Comms-Messaging §8.3 item 11): still in the call
        age("moqui.call.CallConnection", {callLegId: answered.callLegId}, "closedDate", 40000)
        expire()
        const kept = session(started.callSessionId).statusId
        age("moqui.call.CallConnection", {callLegId: answered.callLegId}, "closedDate", 61000)
        const expired = expire()
        return {kept, expired, row: session(started.callSessionId)}
    },
    then({kept, expired, row}) {
        kept != "CssEnded"
        expired.legCount >= 1
        row.statusId == "CssEnded"
        row.endReasonEnumId == "CerConnectionLost"
    }
})

// Move to this device (plans/Comms-Messaging.md §8.3 item 6): a second leg of the same person, the first leaving only once
// the second's media connected; a move that never connects is left and the call goes on. Call waiting (item 7): a user's
// own leg on hold, and back when it binds again.
let movedCall = null
feature("moving a call to another device adds a leg, and the first leaves once it connects; a move left before it connects keeps the call", {
    when() {
        endAll()
        const started = as(A, "start#Call", {calleeUserIdList: [B]})
        const id = (movedCall = started.callSessionId)
        const answered = as(B, "answer#Call", {callSessionId: id})
        as(B, "bind#Connection", {callSessionId: id, callLegId: answered.callLegId})
        const live = as(A, "get#Call", {callSessionId: id}).call
        const stranger = as(C, "move#Call", {callSessionId: id})
        const first = as(B, "move#Call", {callSessionId: id})
        // one move at a time: a new one replaces a move that has not connected
        const replaced = as(B, "move#Call", {callSessionId: id})
        const firstAfter = legs(id).find(l => l.callLegId == first.callLegId)
        const left = as(B, "leave#Call", {callSessionId: id, callLegId: replaced.callLegId})
        const afterLeft = session(id).statusId
        const second = as(B, "move#Call", {callSessionId: id})
        const bound = as(B, "bind#Connection", {callSessionId: id, callLegId: second.callLegId})
        const before = legs(id).find(l => l.callLegId == answered.callLegId).statusId
        sys(() => call("org.moqui.call.CallServices.note#MediaEvent", {callLegId: second.callLegId, generation: bound.generation, event: "connected"}))
        return {answered, live, stranger, first, replaced, firstAfter, left, afterLeft, second, before, legs: legs(id), view: as(B, "get#Call", {callSessionId: id}).call, row: session(id)}
    },
    then({answered, live, stranger, first, replaced, firstAfter, left, afterLeft, second, before, legs, view, row}) {
        replaced.movedFromLegId == answered.callLegId
        firstAfter.statusId == "ClsLeft" && firstAfter.endReasonEnumId == "CerCanceled"
        live.participants.find(p => p.userId == B).legs.find(l => l.callLegId == answered.callLegId).live == true
        stranger.reasonCode == "notFound"
        first.movedFromLegId == answered.callLegId && first.callLegId != answered.callLegId
        left.call.status == "active" && afterLeft == "CssActive"
        second.movedFromLegId == answered.callLegId
        before == "ClsAnswered"
        legs.find(l => l.callLegId == answered.callLegId).statusId == "ClsLeft"
        legs.find(l => l.callLegId == answered.callLegId).endReasonEnumId == "CerMoved"
        legs.find(l => l.callLegId == second.callLegId).statusId == "ClsAnswered"
        row.statusId == "CssActive"
        view.myLegId == second.callLegId && view.participants.find(p => p.userId == B).inCall == true
    }
})

feature("a user puts their own leg on hold for another call, everyone sees it, and binding it again takes it off", {
    when() {
        const id = movedCall
        const held = as(B, "hold#OwnLeg", {callSessionId: id})
        const again = as(B, "hold#OwnLeg", {callSessionId: id})
        const stranger = as(C, "hold#OwnLeg", {callSessionId: id})
        const seen = as(A, "get#Call", {callSessionId: id}).call.participants.find(p => p.userId == B).onHold
        as(B, "bind#Connection", {callSessionId: id, callLegId: held.callLegId})
        const back = as(A, "get#Call", {callSessionId: id}).call.participants.find(p => p.userId == B).onHold
        as(B, "hold#OwnLeg", {callSessionId: id})
        const off = as(B, "hold#OwnLeg", {callSessionId: id, held: false})
        // on hold, the leg outlives its socket (a reconnect, a reload) for half an hour, then leaves
        const bound = as(B, "bind#Connection", {callSessionId: id, callLegId: held.callLegId})
        as(B, "hold#OwnLeg", {callSessionId: id})
        sys(() => call("org.moqui.call.CallServices.release#Connection", {callLegId: held.callLegId, generation: bound.generation}))
        age("moqui.call.CallConnection", {callLegId: held.callLegId}, "closedDate", 61000)
        expire()
        const kept = legs(id).find(l => l.callLegId == held.callLegId).statusId
        age(LEG, {callLegId: held.callLegId}, "heldDate", 31 * 60000)
        expire()
        const gone = legs(id).find(l => l.callLegId == held.callLegId).statusId
        endAll()
        return {held, again, stranger, seen, back, off, kept, gone}
    },
    then({held, again, stranger, seen, back, off, kept, gone}) {
        kept == "ClsAnswered"
        gone == "ClsLeft"
        held.errors == "" && held.callLegId != null
        again.repeated == true
        stranger.reasonCode == "notFound"
        seen == true
        back == false
        off.call.participants.find(p => p.userId == B).onHold == false
    }
})

feature("a move that never bound does not keep a direct call going once the person's real leg leaves", {
    when() {
        endAll()
        const started = as(A, "start#Call", {calleeUserIdList: [B]})
        const id = started.callSessionId
        const answered = as(B, "answer#Call", {callSessionId: id})
        as(B, "bind#Connection", {callSessionId: id, callLegId: answered.callLegId})
        const moved = as(B, "move#Call", {callSessionId: id})
        const left = as(B, "leave#Call", {callSessionId: id, callLegId: answered.callLegId})
        return {moved, left, row: session(id), pending: legs(id).find(l => l.callLegId == moved.callLegId)}
    },
    then({moved, left, row, pending}) {
        moved.callLegId != null
        left.call.status == "ended"
        row.statusId == "CssEnded"
        pending.statusId == "ClsLeft"
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
        const agent = "CALLS_SPEC_AGENT", blocker = "CALLS_SPEC_BLOCKER"
        makeUser(agent)
        makeUser(blocker)
        ec.user.internalLoginUser("calls.spec.a")
        room = sys(() => ec.service.sync().name("org.moqui.chat.ChatServices.create#Room")
            .parameters({roomTypeEnumId: "CrtPrivate", roomName: "Calls spec", memberUserIdList: [B, C, D, agent, blocker]}).call()).chatRoomId
        ec.message.clearErrors()
        sys(() => {
            ec.entity.find("moqui.chat.ChatRoomMember").condition({chatRoomId: room, userId: D}).updateAll({leftDate: ec.user.nowTimestamp})
            ec.entity.find("moqui.chat.ChatRoomMember").condition({chatRoomId: room, userId: agent}).updateAll({roleEnumId: "CmrAgent"})
            const actor = ec.entity.makeValue("moqui.chat.ChatActor")
            actor.setSequencedIdPrimary()
            actor.set("kindEnumId", "CakExternal").create()
            ec.entity.makeValue("moqui.chat.ChatRoomMember").setAll({chatRoomId: room, chatActorId: actor.chatActorId, roleEnumId: "CmrMember"}).create()
        })
        const block = as(blocker, "org.moqui.chat.ChatBlockServices.block#ChatUser", {blockedUserId: A})
        const started = as(A, "start#Call", {chatRoomId: room})
        const id = started.callSessionId
        const offered = started.call.participants.filter(p => p.ringing).map(p => p.userId).sort()
        as(blocker, "org.moqui.chat.ChatBlockServices.unblock#ChatUser", {blockedUserId: A})
        const again = as(B, "start#Call", {chatRoomId: room})
        const outsider = as(D, "join#Call", {callSessionId: id})
        const outsiderStart = as(D, "start#Call", {chatRoomId: room})
        const b = as(B, "join#Call", {callSessionId: id})
        const c = as(C, "join#Call", {callSessionId: id})
        const notHost = as(B, "end#Call", {callSessionId: id})
        const hostLeft = as(A, "leave#Call", {callSessionId: id})
        const bLeft = as(B, "leave#Call", {callSessionId: id})
        const cLeft = as(C, "leave#Call", {callSessionId: id})
        return {started, offered, block, again, outsider, outsiderStart, b, c, notHost, hostLeft, bLeft, cLeft, row: session(id)}
    },
    then({started, offered, block, again, outsider, outsiderStart, b, c, notHost, hostLeft, bLeft, cLeft, row}) {
        block.errors == ""
        offered == [B, C]
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

feature("an active room board grants drawing until the host restricts it and becomes call history at hangup", {
    when() {
        const {callSessionId} = as(A,"start#Call",{chatRoomId:room})
        const hostLeg = as(A,"get#Call",{callSessionId}).call.myLegId
        const {generation} = as(A,"bind#Connection",{callSessionId,callLegId:hostLeg})
        as(B,"join#Call",{callSessionId})
        const {wikiPageId,errors} = as(A,"create#MeetingBoard",{callSessionId,pageName:"Call history sketch"})
        if (errors) throw Error(errors)
        const shown = as(A,"org.moqui.call.StageServices.change#CallStage",{callSessionId,callLegId:hostLeg,generation,action:"ask",kind:"board",wikiPageId})
        const restrict = who => as(who,"set#BoardDrawing",{callSessionId,wikiPageId,memberCanEdit:false,guestCanEdit:false})
        const refused = restrict(B), changed = restrict(A)
        as(A,"end#Call",{callSessionId})
        const history = as(A,"find#MeetingBoards",{callSessionId})
        return {shown,refused,changed,history,wikiPageId}
    },
    then(r) {
        r.shown.stage.wikiPageId === r.wikiPageId && r.shown.stage.guestCanEdit === true
        r.refused.errors.includes("host")
        !r.changed.errors && r.changed.guestCanEdit === false
        r.history.shown.some(b=>b.wikiPageId===r.wikiPageId && b.closedDate)
    }
})

feature("a non-host cannot take the stage from a host; a leg that rebinds, releases or is removed gives up its share", {
    when() {
        const {callSessionId} = as(A, "start#Call", {chatRoomId: room})
        as(B, "join#Call", {callSessionId})
        as(C, "join#Call", {callSessionId})
        const binding = user => {
            const callLegId = as(user, "get#Call", {callSessionId}).call.myLegId
            return {callLegId, generation: as(user, "bind#Connection", {callSessionId, callLegId}).generation}
        }
        const host = binding(A), b = binding(B), c = binding(C)
        const act = (who, leg, action, extra = {}) => as(who, "org.moqui.call.StageServices.change#CallStage",
                {callSessionId, ...leg, action, ...extra})
        const setStage = value => sys(() => { const row = session(callSessionId); row.set("stageState", value); row.update() })
        const stageOf = () => as(A, "get#Call", {callSessionId}).call.stage
        act(A, host, "ask")
        const fromHost = act(B, b, "take", {previousLegId: host.callLegId})
        act(A, host, "stop")
        const memberShows = act(B, b, "ask")
        const confirm = act(C, c, "ask")
        const fromMember = act(C, c, "take", {previousLegId: b.callLegId})
        const hostTakes = act(A, host, "take")
        act(A, host, "stop")
        // a request of a leg that is gone: the answer is the stage as stored, without it
        setStage(JSON.stringify({kind: null, callLegId: null, requests: ["GONE_LEG"]}))
        const reread = act(B, b, "ask")
        // an unreadable stage is nobody presenting, and the next change replaces it
        setStage("{not json")
        const badView = stageOf()
        const afterBad = act(C, c, "ask")
        // C's page reloads: its new connection ends its screen share
        c.generation = as(C, "bind#Connection", {callSessionId, callLegId: c.callLegId}).generation
        const rebound = stageOf()
        act(B, b, "ask")
        sys(() => call("org.moqui.call.CallServices.release#Connection", {callLegId: b.callLegId, generation: b.generation}))
        const released = stageOf()
        act(C, c, "ask")
        const cParticipant = as(A, "get#Call", {callSessionId}).call.participants.find(p => p.userId === C).callParticipantId
        as(A, "remove#Participant", {callSessionId, callParticipantId: cParticipant})
        const removed = stageOf()
        as(A, "end#Call", {callSessionId})
        return {host, b, c, fromHost, memberShows, confirm, fromMember, hostTakes, reread, badView, afterBad, rebound, released, removed}
    },
    then(r) {
        r.fromHost.reasonCode === "notAllowed" && r.fromHost.errors.includes("host")
        r.memberShows.stage.callLegId === r.b.callLegId && r.confirm.reasonCode === "takeoverRequired"
        r.fromMember.stage.callLegId === r.c.callLegId
        r.hostTakes.stage.callLegId === r.host.callLegId
        r.reread.stage.callLegId === r.b.callLegId && !r.reread.stage.requests.includes("GONE_LEG")
        r.badView.kind == null && r.badView.callLegId == null
        r.afterBad.stage.callLegId === r.c.callLegId
        r.c.generation === 2 && r.rebound.callLegId == null
        r.released.callLegId == null
        r.removed.callLegId == null
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
        out = ec.service.sync().name(name.includes(".") ? name : "org.moqui.call.CallServices." + name).parameters(parameters || {}).call() || {}
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
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : "org.moqui.call.CallServices." + name).parameters(parameters || {}).call()) || {}
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
        const guestFollow = internal("org.moqui.call.StageServices.change#CallStage", {credential:guest.credential,
            callSessionId:guest.callSessionId,callLegId:guest.callLegId,generation:bound.generation,action:"take",kind:"follow"})
        const wrongCall = internal("bind#GuestConnection", {credential: guest.credential, callSessionId: direct, callLegId: guest.callLegId})
        ec.message.clearErrors()
        const links = as(A, "find#CallLinks")
        return {bad, good, noName, waiting, earlyBind, stranger, opened, admitted, inState, bound, guestFollow, wrongCall, links}
    },
    then({bad, good, noName, waiting, earlyBind, stranger, opened, admitted, inState, bound, guestFollow, wrongCall, links}) {
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
        guestFollow.reasonCode == "notAllowed" && bound.memberScreen === false
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

// SEC-COM-05: 50 guests who joined and left kept a link's meeting full; now a place is an open guest leg, and a link
// takes calls@guest-joins-per-minute (20) joins a minute whatever the client
feature("a link's places are held by guests with an open leg, and a link takes only so many joins a minute", {
    when() {
        const open = as(A, "create#CallLink", {name: "Busy door", policy: "link"})
        const first = anon("join#AsGuest", {token: open.token, displayName: "First"})
        const linkId = open.callLinkId
        // 50 guests who joined two minutes ago and left: admitted, their legs closed
        const seeded = sys(() => {
            const guestIds = []
            const legIds = []
            for (let i = 0; i < 50; i++) {
                const g = ec.entity.makeValue("moqui.call.CallGuest")
                g.setSequencedIdPrimary()
                g.setFields({callLinkId: linkId, callSessionId: first.callSessionId, displayName: "Gone " + i,
                    createdDate: new Date(Date.now() - 120000)}, true, null, null)
                g.create()
                const p = ec.entity.makeValue("moqui.call.CallParticipant")
                p.setSequencedIdPrimary()
                p.setFields({callSessionId: first.callSessionId, identityKindEnumId: "CikGuest", callGuestId: g.callGuestId,
                    displayName: "Gone " + i, roleEnumId: "CprGuest", admissionStatusId: "CasAdmitted"}, true, null, null)
                p.create()
                const l = ec.entity.makeValue(LEG)
                l.setSequencedIdPrimary()
                l.setFields({callSessionId: first.callSessionId, callParticipantId: p.callParticipantId, legTypeEnumId: "CltGuest",
                    statusId: "ClsLeft", offeredDate: new Date(Date.now() - 120000)}, true, null, null)
                l.create()
                guestIds.push(g.callGuestId)
                legIds.push(l.callLegId)
            }
            return {guestIds, legIds}
        })
        const freed = anon("join#AsGuest", {token: open.token, displayName: "Next"})
        const setLegs = status => sys(() => {
            for (const l of ec.entity.find(LEG).condition("callLegId", "in", seeded.legIds).list()) { l.set("statusId", status); l.update() }
        })
        setLegs("ClsAnswered")
        const full = anon("join#AsGuest", {token: open.token, displayName: "Too many"})
        setLegs("ClsLeft")
        sys(() => {
            for (const g of ec.entity.find("moqui.call.CallGuest").condition("callGuestId", "in", seeded.guestIds).list()) {
                g.set("createdDate", new Date())
                g.update()
            }
        })
        const rushed = anon("join#AsGuest", {token: open.token, displayName: "Rushed"})
        for (const g of [first, freed]) anon("leave#AsGuest", {credential: g.credential})
        return {first, freed, full, rushed}
    },
    then({first, freed, full, rushed}) {
        first.admission == "admitted"
        freed.admission == "admitted"
        full.reasonCode == "callFull"
        rushed.reasonCode == "tooManyJoins"
    }
})

const LEG_NODE = "calls-spec-leg-node"
feature("with cascading media nodes a leg's media is on its signaling node, and losing that node ends only its legs", {
    when() {
        nodeUp()
        sys(() => call("org.moqui.call.CallServices.register#MediaNode", {instanceId: LEG_NODE, mediaAddress: "127.0.0.1:10002"}))
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
        setConf("media-server@cascade-enabled", "false")
        const off = as(B, "bind#Connection", {callSessionId: id, callLegId: bLeg, signalingNodeId: LEG_NODE})
        setConf("media-server@cascade-enabled", "true")
        const onLegNode = as(B, "bind#Connection", {callSessionId: id, callLegId: bLeg, signalingNodeId: LEG_NODE})
        const stranger = as(D, "bind#Connection", {callSessionId: id, callLegId: bLeg, signalingNodeId: LEG_NODE})
        const noMedia = as(A, "bind#Connection", {callSessionId: id, callLegId: started.callLegId, signalingNodeId: "calls-spec-nowhere"})
        // the call's node refused B's leg node's relay: B's media goes back to the call's node, and stays there
        sys(() => call("org.moqui.call.CallServices.note#MediaEvent", {callLegId: bLeg, generation: onLegNode.generation, event: "relayRefused"}))
        const pinned = as(B, "bind#Connection", {callSessionId: id, callLegId: bLeg, signalingNodeId: LEG_NODE})
        const cOnLegNode = as(C, "bind#Connection", {callSessionId: id, callLegId: cLeg, signalingNodeId: LEG_NODE})
        // the leg node is lost: C's leg leaves with that reason; the call and the others go on
        age("moqui.call.CallMediaNode", {instanceId: LEG_NODE}, "heartbeatDate", 120000)
        const expired = expire()
        setConf("media-server@cascade-enabled", "false")
        const leg = (legId) => sys(() => ec.entity.find(LEG).condition("callLegId", legId).one())
        const out = {off, onLegNode, stranger, noMedia, pinned, cOnLegNode, expired, row: session(id), b: leg(bLeg), c: leg(cLeg)}
        as(A, "end#Call", {callSessionId: id})
        sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", LEG_NODE).deleteAll())
        return out
    },
    then({off, onLegNode, stranger, noMedia, pinned, cOnLegNode, expired, row, b, c}) {
        off.mediaNodeId == NODE
        off.originNodeId == NODE
        onLegNode.mediaNodeId == LEG_NODE
        onLegNode.originNodeId == NODE
        stranger.reasonCode == "notFound"
        noMedia.mediaNodeId == NODE
        pinned.mediaNodeId == NODE
        b.mediaNodePinned == "Y"
        cOnLegNode.mediaNodeId == LEG_NODE
        expired.legCount >= 1
        row.statusId == "CssActive"
        c.statusId == "ClsLeft"
        c.endReasonEnumId == "CerMediaNodeLost"
        b.statusId == "ClsAnswered"
    }
})

feature("a leg in the call gets its TURN servers with a credential of its own; nobody else does", {
    when() {
        sys(() => call("org.moqui.call.CallServices.register#MediaNode", {instanceId: NODE, mediaAddress: "127.0.0.1:10000",
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

feature("a new call's media goes to the edge node, whose row the edge link keeps, before a newer media node", {
    when() {
        // the edge node's row (plans/archive/Edge-Node.md §5.4), its heartbeat older than the other media node's
        sys(() => call("org.moqui.call.CallServices.register#MediaNode", {instanceId: EDGE_NODE, mediaAddress: "203.0.113.5:10000",
            edgeLinked: true}))
        age("moqui.call.CallMediaNode", {instanceId: EDGE_NODE}, "heartbeatDate", 5000)
        nodeUp()
        const started = as(A, "start#Call", {calleeUserIdList: [B]})
        const row = session(started.callSessionId)
        const edgeRow = sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", EDGE_NODE).one())
        const other = sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", NODE).one())
        // a media node's own heartbeat never takes the edge row, and the link never takes another node's row
        const plainOnEdge = as(A, "register#MediaNode", {instanceId: EDGE_NODE, mediaAddress: "127.0.0.1:10009"})
        const linkOnOther = as(A, "register#MediaNode", {instanceId: NODE, mediaAddress: "203.0.113.9:10000", edgeLinked: true})
        const edgeAfter = sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", EDGE_NODE).one())
        const otherAfter = sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", NODE).one())
        as(A, "end#Call", {callSessionId: started.callSessionId})
        // unpairing removes the edge node's row
        sys(() => call("org.moqui.call.CallServices.register#MediaNode", {instanceId: EDGE_NODE, edgeLinked: true, stopping: true}))
        const gone = sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", EDGE_NODE).count())
        return {started, row, edgeRow, other, gone, plainOnEdge, linkOnOther, edgeAfter, otherAfter}
    },
    then({started, row, edgeRow, other, gone, plainOnEdge, linkOnOther, edgeAfter, otherAfter}) {
        plainOnEdge.errors.includes("is not a node of this cluster")
        linkOnOther.errors.includes("is not this node's edge node")
        edgeAfter.edgeLinked == "Y"
        edgeAfter.mediaAddress == "203.0.113.5:10000"
        otherAfter.edgeLinked == "N"
        otherAfter.mediaAddress == "127.0.0.1:10000"
        started.errors == ""
        edgeRow.edgeLinked == "Y"
        other.edgeLinked == "N"
        row.mediaNodeId == EDGE_NODE
        gone == 0
    }
})
