// Call queues (org.moqui.impl.CallQueueServices and CallQueues.js; plans/Comms-Framework.md §5.3, §5.4, V5): only a voice admin
// keeps queues (the same parameters refused for anyone else); agents log in, pause and log out themselves; a call to a
// number routed to a queue waits there and is offered to the free agent idle longest, each offer ringing their tabs and
// phones with a CallUserLog row; an offer that rings out is missed and the next agent is offered; with all at once the
// first answer wins and the entry is answered; after a queue call its agent is in wrap-up and skipped; past its wait
// limit or its length the call goes to the fallback; the stats count the waiting calls, the longest wait and the free
// agents; a caller who hangs up abandons the entry. Time is simulated (tick#QueueCall and the agents' status at atMs).
// No SIP engine runs here: fake SipNode and CallMediaNode rows stand in for one.
// Run: ./moq test framework --spec CallQueue
const ADMIN = "CQ_SPEC_ADMIN"
const A1 = "CQ_SPEC_A1"
const A2 = "CQ_SPEC_A2"
const OUT = "CQ_SPEC_OUT"
const USERS = [ADMIN, A1, A2, OUT]
const NODE = "cq-spec-node"
const TRUNK = "CQ_SPEC_TRUNK"
const QUEUE_NUMBER = "+15550400001"
const CALLER = "+15557770400"
const LEG = "moqui.call.CallLeg"
const ENTRY = "moqui.call.CallQueueEntry"
const AGENT = "moqui.call.CallQueueAgent"

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
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : "org.moqui.impl.CallQueueServices." + name)
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
            account.setFields({userId, username: username(userId), userFullName: "Queue " + userId.slice(8), disabled: "N"}, true, null, null)
            account.create()
        }
        if (groupId && ec.entity.find("moqui.security.UserGroupMember").condition({userId, userGroupId: groupId}).count() === 0) {
            const member = ec.entity.makeValue("moqui.security.UserGroupMember")
            member.setFields({userId, userGroupId: groupId, fromDate: new Date(Date.now() - 60000)}, true, null, null)
            member.create()
        }
    })
}
function upsert(entity, pk, fields) {
    sys(() => {
        const row = ec.entity.find(entity).condition(pk).one()
        if (row) {
            row.setFields(fields, true, null, null)
            row.update()
        } else {
            const made = ec.entity.makeValue(entity)
            made.setFields(Object.assign({}, pk, fields), true, null, null)
            made.create()
        }
    })
}
function endAll() {
    sys(() => {
        for (const s of ec.entity.find("moqui.call.CallSession").condition("kindEnumId", "CskPhone").condition("statusId", "!=", "CssEnded").list()) {
            s.set("statusId", "CssEnded")
            s.update()
        }
        for (const e of ec.entity.find(ENTRY).condition("statusId", "CqeWaiting").list()) {
            e.set("statusId", "CqeAbandoned")
            e.update()
        }
        ec.entity.find("moqui.security.UserStatus").condition("userId", "in", USERS).deleteAll()
    })
}
function clean() {
    endAll()
    sys(() => {
        ec.entity.find("moqui.call.PhoneNumber").condition("phoneNumber", QUEUE_NUMBER).deleteAll()
        for (const queue of ec.entity.find("moqui.call.CallQueue").condition("name", "like", "CQ spec%").list()) {
            ec.entity.find(ENTRY).condition("callQueueId", queue.callQueueId).deleteAll()
            ec.entity.find(AGENT).condition("callQueueId", queue.callQueueId).deleteAll()
            queue.delete()
        }
        ec.entity.find("moqui.call.VoicemailBox").condition("userId", "in", USERS).deleteAll()
        ec.entity.find("moqui.call.VoiceTrunk").condition("voiceTrunkId", TRUNK).deleteAll()
        ec.entity.find("moqui.call.SipNode").condition("instanceId", NODE).deleteAll()
        ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", NODE).deleteAll()
    })
}
function nodesUp() {
    sys(() => {
        ec.service.sync().name("org.moqui.impl.CallServices.register#MediaNode").parameters({instanceId: NODE, mediaAddress: "127.0.0.1:10000"}).call()
        ec.service.sync().name("org.moqui.impl.VoiceServices.register#SipNode").parameters({instanceId: NODE, sipAddress: "127.0.0.1:5060"}).call()
    })
}
const receive = () => engine("org.moqui.impl.VoiceServices.receive#PhoneCall", {toNumber: QUEUE_NUMBER, fromNumber: CALLER,
    fromName: "Queue Caller", instanceId: NODE})
const tick = (call, atMs) => engine("tick#QueueCall", {callQueueEntryId: call.ivr.callQueueEntryId, atMs}).answer || {}
const entryOf = (call) => sys(() => ec.entity.find(ENTRY).condition("callQueueEntryId", call.ivr.callQueueEntryId).one())
const offeredTo = (callSessionId) => USERS.filter((userId) => sys(() => {
    const p = ec.entity.find("moqui.call.CallParticipant").condition({callSessionId, userId}).one()
    return !!p && ec.entity.find(LEG).condition({callParticipantId: p.callParticipantId, statusId: "ClsOffered"}).count() > 0
}))
const log = (callSessionId, userId) => sys(() => ec.entity.find("moqui.call.CallUserLog").condition({callSessionId, userId}).one())
const agent = (userId) => sys(() => ec.entity.find(AGENT).condition({callQueueId: queueId, userId}).one())
const hangUp = (call) => engine("org.moqui.impl.VoiceServices.note#PhoneLeg", {callSessionId: call.callSessionId, callLegId: call.callLegId, event: "ended"})
const ms = (value) => {
    if (value == null || value === "") return null
    const number = Number(value)
    return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}

let queueId = null

spec({
    stepwise: true,
    setupSpec() {
        makeUser(ADMIN, "ADMIN")
        for (const userId of [A1, A2, OUT]) makeUser(userId)
        clean()
        upsert("moqui.call.VoiceTrunk", {voiceTrunkId: TRUNK}, {name: "CQ spec trunk", host: "sip.example.invalid", transportEnumId: "VtrTls",
            mediaEncryptionEnumId: "VmeSrtp", providerProfileEnumId: "VppGeneric", statusId: "VtsOk", disabled: "N"})
        nodesUp()
    },
    cleanupSpec() {
        clean()
    }
})

feature("only a voice admin keeps queues and their agents; an agent logs in, pauses and logs out only themselves", {
    when() {
        const fields = {name: "CQ spec support", strategyEnumId: "CqsLongestIdle", ringSeconds: 15, maxWaitSeconds: 120, maxLength: 2,
            wrapUpSeconds: 30, announceSeconds: 30, fallbackTypeEnumId: "CtgVoicemail"}
        const refused = as(OUT, "store#CallQueue", fields)
        const badFallback = as(ADMIN, "store#CallQueue", fields)
        upsert("moqui.call.VoicemailBox", {voicemailBoxId: "CQ_SPEC_BOX"}, {userId: A1, attachAudio: "N", attachTranscript: "N"})
        const made = as(ADMIN, "store#CallQueue", Object.assign({}, fields, {fallbackId: "CQ_SPEC_BOX"}))
        queueId = made.callQueueId
        const badLimit = as(ADMIN, "store#CallQueue", {callQueueId: queueId, ringSeconds: 1})
        const agentRefused = as(OUT, "store#CallQueueAgent", {callQueueId: queueId, userId: A1})
        as(ADMIN, "store#CallQueueAgent", {callQueueId: queueId, userId: A1, sequenceNum: 10})
        as(ADMIN, "store#CallQueueAgent", {callQueueId: queueId, userId: A2, sequenceNum: 20})
        const routed = as(ADMIN, "org.moqui.impl.VoiceServices.store#PhoneNumber", {phoneNumber: QUEUE_NUMBER, voiceTrunkId: TRUNK,
            routeTypeEnumId: "PnrQueue", routeId: queueId})
        const notAgent = as(OUT, "set#QueueAgentStatus", {callQueueId: queueId, status: "in"})
        const loggedIn = as(A1, "set#QueueAgentStatus", {callQueueId: queueId, status: "in"})
        const paused = as(A1, "set#QueueAgentStatus", {callQueueId: queueId, status: "pause"})
        const resumed = as(A1, "set#QueueAgentStatus", {status: "resume"})
        as(A2, "set#QueueAgentStatus", {status: "in"})
        const found = as(ADMIN, "find#CallQueues")
        const findRefused = as(OUT, "find#CallQueues")
        return {refused, badFallback, made, badLimit, agentRefused, routed, notAgent, loggedIn, paused, resumed, found, findRefused}
    },
    then({refused, badFallback, made, badLimit, agentRefused, routed, notAgent, loggedIn, paused, resumed, found, findRefused}) {
        refused.reasonCode == "notAllowed"
        badFallback.reasonCode == "badTarget"
        made.errors == ""
        badLimit.reasonCode == "badQueue"
        agentRefused.reasonCode == "notAllowed"
        routed.errors == ""
        notAgent.reasonCode == "notFound"
        loggedIn.queueList[0].state == "available"
        paused.queueList[0].state == "paused"
        resumed.queueList[0].state == "available"
        found.queueList.find((q) => q.callQueueId == queueId).agentList.map((a) => a.userId).join(",") == [A1, A2].join(",")
        found.queueList.find((q) => q.callQueueId == queueId).agentsAvailable == 2
        found.queueList.find((q) => q.callQueueId == queueId).numberList[0] == QUEUE_NUMBER
        findRefused.reasonCode == "notAllowed"
    }
})

feature("the agent idle longest is offered first; an offer that rings out is missed and the next agent is offered", {
    when() {
        // A1 ended a call a minute ago, A2 ten minutes ago: A2 has been idle longer
        const now = Date.now()
        upsert(AGENT, {callQueueId: queueId, userId: A1}, {lastCallEndDate: new Date(now - 60000)})
        upsert(AGENT, {callQueueId: queueId, userId: A2}, {lastCallEndDate: new Date(now - 600000)})
        const call = receive()
        const first = offeredTo(call.callSessionId)
        const stats = as(A1, "find#MyCallQueues").queueList[0]
        // 10 s on: still ringing A2; 16 s on (ringSeconds 15): A2 missed it, A1 rings
        const early = tick(call, now + 10000)
        const stillFirst = offeredTo(call.callSessionId)
        const later = tick(call, now + 16000)
        const second = offeredTo(call.callSessionId)
        const a2Log = log(call.callSessionId, A2)
        const a1Log = log(call.callSessionId, A1)
        // A1 answers: the entry is answered and the caller's ticks are done
        const answered = as(A1, "org.moqui.impl.CallServices.answer#Call", {callSessionId: call.callSessionId})
        const done = tick(call, now + 17000)
        const entry = entryOf(call)
        // A1 hangs up: the call ends, and A1 is in wrap-up for 30 seconds
        as(A1, "org.moqui.impl.CallServices.leave#Call", {callSessionId: call.callSessionId})
        return {call, first, stats, early, stillFirst, later, second, a2Log, a1Log, answered, done, entry, a1: agent(A1), now}
    },
    then({call, first, stats, early, stillFirst, later, second, a2Log, a1Log, answered, done, entry, a1, now}) {
        call.ivr.mode == "queue"
        call.ivr.position == 1
        call.ivr.announce == true
        first.join(",") == A2
        stats.waitingCount == 1
        stats.agentsAvailable == 1
        early.mode == "queue"
        early.announce == false
        stillFirst.join(",") == A2
        later.mode == "queue"
        later.announce == false
        second.join(",") == A1
        a2Log.roleEnumId == "CurQueueAgent"
        a2Log.outcomeEnumId == "CuoMissed"
        a1Log.roleEnumId == "CurQueueAgent"
        answered.errors == ""
        done.mode == "done"
        entry.statusId == "CqeAnswered"
        entry.answeredUserId == A1
        ms(a1.wrapUpUntilDate) > Date.now() + 20000
        ms(a1.lastCallEndDate) > now - 1000
    }
})

feature("an agent in wrap-up is skipped; all at once rings every free agent and the first answer wins", {
    when() {
        // A1 is in wrap-up from the last call; A2 is in another call: nobody is free
        upsert("moqui.security.UserStatus", {userId: A2}, {inCall: "Y"})
        const now = Date.now()
        const call = receive()
        const nobody = offeredTo(call.callSessionId)
        const stats = as(ADMIN, "find#CallQueues").queueList.find((q) => q.callQueueId == queueId)
        // A2 is free again: offered on the next tick, though A1 has been idle longer since... A1 is in wrap-up still
        upsert("moqui.security.UserStatus", {userId: A2}, {inCall: "N"})
        tick(call, now + 5000)
        const whileWrapping = offeredTo(call.callSessionId)
        // all at once, past A1's wrap-up and A2's offer: both ring, and A2 answers first
        as(ADMIN, "store#CallQueue", {callQueueId: queueId, strategyEnumId: "CqsAllAtOnce"})
        tick(call, now + 16000)
        const pastWrapUp = tick(call, now + 40000)
        const both = offeredTo(call.callSessionId)
        const won = as(A2, "org.moqui.impl.CallServices.answer#Call", {callSessionId: call.callSessionId})
        const lost = as(A1, "org.moqui.impl.CallServices.answer#Call", {callSessionId: call.callSessionId})
        const a1Log = log(call.callSessionId, A1)
        as(A2, "org.moqui.impl.CallServices.leave#Call", {callSessionId: call.callSessionId})
        as(ADMIN, "store#CallQueue", {callQueueId: queueId, strategyEnumId: "CqsLongestIdle"})
        return {nobody, stats, whileWrapping, pastWrapUp, both, won, lost, a1Log, entry: entryOf(call)}
    },
    then({nobody, stats, whileWrapping, pastWrapUp, both, won, lost, a1Log, entry}) {
        nobody.length == 0
        stats.waitingCount == 1
        stats.agentsAvailable == 0
        stats.agentsLoggedIn == 2
        whileWrapping.join(",") == A2
        pastWrapUp.mode == "queue"
        both.join(",") == [A1, A2].join(",")
        won.errors == ""
        lost.reasonCode == "notRinging"
        a1Log.outcomeEnumId == "CuoMissed"
        entry.answeredUserId == A2
    }
})

feature("past its wait limit a call goes to the fallback; a full queue sends a new call there at once; a caller who hangs up abandons", {
    when() {
        endAll()
        sys(() => ec.entity.find(AGENT).condition("callQueueId", queueId).updateAll({wrapUpUntilDate: null}))
        as(A1, "set#QueueAgentStatus", {status: "out"})
        as(A2, "set#QueueAgentStatus", {status: "out"})
        const now = Date.now()
        const call = receive()
        const waiting = tick(call, now + 60000)
        const overflow = tick(call, now + 121000)
        const overflowEntry = entryOf(call)
        // maxLength 2: two wait, the third goes to the fallback
        const second = receive()
        const third = receive()
        const fourth = receive()
        const stats = as(ADMIN, "find#CallQueues").queueList.find((q) => q.callQueueId == queueId)
        hangUp(third)
        const abandoned = entryOf(third)
        const thirdTick = tick(third, now + 1000)
        const after = as(ADMIN, "find#CallQueues").queueList.find((q) => q.callQueueId == queueId)
        return {waiting, overflow, overflowEntry, second, third, fourth, stats, abandoned, thirdTick, after}
    },
    then({waiting, overflow, overflowEntry, second, third, fourth, stats, abandoned, thirdTick, after}) {
        waiting.mode == "queue"
        waiting.announce == true
        overflow.mode == "voicemail"
        overflow.stepToken != null
        overflowEntry.statusId == "CqeOverflow"
        second.ivr.mode == "queue"
        second.ivr.position == 1
        third.ivr.position == 2
        fourth.ivr.mode == "voicemail"
        stats.waitingCount == 2
        stats.agentsAvailable == 0
        abandoned.statusId == "CqeAbandoned"
        thirdTick.mode == "done"
        after.waitingCount == 1
    }
})
