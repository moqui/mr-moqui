// Ring groups (org.moqui.call.CallHandlingServices and CallHandling.js, RingGroup and RingGroupMember; plans/Comms-Framework.md
// §5.3, §5.4, V3): a number routed to a ring group rings its members by strategy. All at once: every free member (one in do not
// disturb or busy is skipped), each with a CallUserLog row as a ring group member, and the first answer wins across their
// tabs, phones and mobiles, the others' offers canceled. In order: one member at a time, the next when the SIP engine's
// timer ends the step (begin#Voicemail, reason noAnswer), then the fallback: the group's own box, which its members share
// (a message one of them heard is heard for all; anyone else is refused with the same id). Round robin starts after the
// member rung first last time. Outside its schedule a group goes straight to its fallback; another group and an outside
// number are fallbacks too. Only a voice admin keeps groups (the same parameters refused for anyone else).
// Run: ./moq test framework --spec RingGroup
const ADMIN = "RG_SPEC_ADMIN"
const M1 = "RG_SPEC_M1"
const M2 = "RG_SPEC_M2"
const M3 = "RG_SPEC_M3"
const OUT = "RG_SPEC_OUT"
const USERS = [ADMIN, M1, M2, M3, OUT]
const NODE = "rg-spec-node"
const TRUNK = "RG_SPEC_TRUNK"
const GROUP_NUMBER = "+15550200001"
const CALLER = "+15557770200"
const MOBILE_M1 = "+15558880201"
const LEG = "moqui.call.CallLeg"

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
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : "org.moqui.call.CallHandlingServices." + name)
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
            account.setFields({userId, username: username(userId), userFullName: "Group " + userId.slice(8), disabled: "N"}, true, null, null)
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
function clean() {
    sys(() => {
        for (const s of ec.entity.find("moqui.call.CallSession").condition("kindEnumId", "CskPhone").condition("statusId", "!=", "CssEnded").list()) {
            s.set("statusId", "CssEnded")
            s.update()
        }
        for (const entity of ["moqui.call.CallHandlingRule", "moqui.call.CallHandlingSetting", "moqui.call.VoiceUserSetting", "moqui.security.UserStatus"]) {
            ec.entity.find(entity).condition("userId", "in", USERS).deleteAll()
        }
        for (const group of ec.entity.find("moqui.call.RingGroup").condition("name", "like", "RG spec%").list()) {
            ec.entity.find("moqui.call.RingGroupMember").condition("ringGroupId", group.ringGroupId).deleteAll()
            for (const box of ec.entity.find("moqui.call.VoicemailBox").condition("ringGroupId", group.ringGroupId).list()) {
                for (const v of ec.entity.find("moqui.call.Voicemail").condition("voicemailBoxId", box.voicemailBoxId).list()) {
                    if (v.audioLocation) {
                        const ref = ec.resource.getLocationReference(v.audioLocation)
                        if (ref.getExists()) ref.delete()
                    }
                    v.delete()
                }
                box.delete()
            }
            ec.entity.find("moqui.call.PhoneNumber").condition("routeId", group.ringGroupId).deleteAll()
            group.delete()
        }
        for (const s of ec.entity.find("moqui.call.CommsSchedule").condition("name", "like", "RG spec%").list()) {
            ec.entity.find("moqui.call.CommsScheduleRule").condition("commsScheduleId", s.commsScheduleId).deleteAll()
            s.delete()
        }
        ec.entity.find("moqui.call.VoiceTrunk").condition("voiceTrunkId", TRUNK).deleteAll()
        ec.entity.find("moqui.call.SipNode").condition("instanceId", NODE).deleteAll()
        ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", NODE).deleteAll()
    })
}
function nodesUp() {
    sys(() => {
        ec.service.sync().name("org.moqui.call.CallServices.register#MediaNode").parameters({instanceId: NODE, mediaAddress: "127.0.0.1:10000"}).call()
        ec.service.sync().name("org.moqui.call.VoiceServices.register#SipNode").parameters({instanceId: NODE, sipAddress: "127.0.0.1:5060"}).call()
    })
}
const receive = () => engine("org.moqui.call.VoiceServices.receive#PhoneCall", {toNumber: GROUP_NUMBER, fromNumber: CALLER, fromName: "Spec Caller", instanceId: NODE})
const begin = (call, reason, stepToken) => engine("org.moqui.call.VoicemailServices.begin#Voicemail", {callSessionId: call.callSessionId, callLegId: call.callLegId,
    reason: reason || "noAnswer", stepToken: stepToken || null})
function legsOfUser(callSessionId, userId) {
    return sys(() => {
        const p = ec.entity.find("moqui.call.CallParticipant").condition({callSessionId, userId}).one()
        return p ? Array.from(ec.entity.find(LEG).condition("callParticipantId", p.callParticipantId).orderBy("offeredDate").list()) : []
    })
}
const offered = (callSessionId) => USERS.filter((userId) => legsOfUser(callSessionId, userId).some((l) => l.statusId === "ClsOffered"))
const log = (callSessionId, userId) => sys(() => ec.entity.find("moqui.call.CallUserLog").condition({callSessionId, userId}).one())
const note = (callSessionId, callLegId, event) => engine("org.moqui.call.VoiceServices.note#PhoneLeg", {callSessionId, callLegId, event})
const status = (userId, fields) => upsert("moqui.security.UserStatus", {userId}, fields)
const endCall = (callSessionId) => sys(() => {
    const s = ec.entity.find("moqui.call.CallSession").condition("callSessionId", callSessionId).one()
    if (s && s.statusId !== "CssEnded") { s.set("statusId", "CssEnded"); s.update() }
})

let groupId = null
let call2 = null

spec({
    stepwise: true,
    setupSpec() {
        makeUser(ADMIN, "ADMIN")
        for (const userId of [M1, M2, M3, OUT]) makeUser(userId)
        clean()
        upsert("moqui.call.VoiceTrunk", {voiceTrunkId: TRUNK}, {name: "Spec trunk", host: "sip.example.invalid", transportEnumId: "VtrTls",
            mediaEncryptionEnumId: "VmeSrtp", providerProfileEnumId: "VppGeneric", statusId: "VtsOk", disabled: "N"})
        nodesUp()
    },
    cleanupSpec() {
        clean()
    }
})

feature("only a voice admin keeps ring groups and routes a number to one", {
    when() {
        const fields = {name: "RG spec sales", strategyEnumId: "RgsSimultaneous", ringSeconds: 15, fallbackTypeEnumId: "RgfVoicemail"}
        const refused = as(OUT, "store#RingGroup", fields)
        const made = as(ADMIN, "store#RingGroup", fields)
        groupId = made.ringGroupId
        const memberRefused = as(OUT, "store#RingGroupMember", {ringGroupId: groupId, userId: M1})
        for (const [userId, seq] of [[M1, 10], [M2, 20], [M3, 30]]) as(ADMIN, "store#RingGroupMember", {ringGroupId: groupId, userId, sequenceNum: seq})
        const routed = as(ADMIN, "org.moqui.call.VoiceServices.store#PhoneNumber", {phoneNumber: GROUP_NUMBER, voiceTrunkId: TRUNK,
            routeTypeEnumId: "PnrRingGroup", routeId: groupId})
        const badRoute = as(ADMIN, "org.moqui.call.VoiceServices.store#PhoneNumber", {phoneNumber: GROUP_NUMBER, voiceTrunkId: TRUNK,
            routeTypeEnumId: "PnrRingGroup", routeId: "NO_SUCH_GROUP"})
        const badSeconds = as(ADMIN, "store#RingGroup", Object.assign({}, fields, {ringGroupId: groupId, ringSeconds: 2}))
        const found = as(ADMIN, "find#RingGroups")
        const findRefused = as(OUT, "find#RingGroups")
        return {refused, made, memberRefused, routed, badRoute, badSeconds, found, findRefused}
    },
    then({refused, made, memberRefused, routed, badRoute, badSeconds, found, findRefused}) {
        refused.reasonCode == "notAllowed"
        made.errors == ""
        memberRefused.reasonCode == "notAllowed"
        routed.errors == ""
        badRoute.reasonCode == "notFound"
        badSeconds.reasonCode == "badGroup"
        found.groupList.find((g) => g.ringGroupId == groupId).memberList.map((m) => m.userId).join(",") == [M1, M2, M3].join(",")
        found.groupList.find((g) => g.ringGroupId == groupId).numberList[0] == GROUP_NUMBER
        findRefused.reasonCode == "notAllowed"
    }
})

feature("all at once: every free member rings (do not disturb skipped), each logged, and the first answer cancels the rest", {
    when() {
        status(M3, {statusEnumId: "UpsDnd"})
        const call = receive()
        const rung = offered(call.callSessionId)
        const logs = [M1, M2, M3].map((userId) => log(call.callSessionId, userId))
        const answered = as(M2, "org.moqui.call.CallServices.answer#Call", {callSessionId: call.callSessionId})
        const late = as(M1, "org.moqui.call.CallServices.answer#Call", {callSessionId: call.callSessionId})
        const m1Leg = legsOfUser(call.callSessionId, M1)[0]
        const m1Log = log(call.callSessionId, M1)
        const m2Log = log(call.callSessionId, M2)
        // a timer that fires after the answer changes nothing
        const lateTimer = begin(call)
        status(M3, {statusEnumId: "UpsOnline"})
        // M2 hangs up: the call ends and M2 is in no call
        as(M2, "org.moqui.call.CallServices.leave#Call", {callSessionId: call.callSessionId})
        return {call, rung, logs, answered, late, m1Leg, m1Log, m2Log, lateTimer, m2InCall: sys(() => ec.entity.find("moqui.security.UserStatus").condition("userId", M2).one()?.inCall)}
    },
    then({call, rung, logs, answered, late, m1Leg, m1Log, m2Log, lateTimer, m2InCall}) {
        m2InCall == "N"
        call.voicemailSeconds == 15
        call.voicemail == null
        rung.join(",") == [M1, M2].join(",")
        logs[0].roleEnumId == "CurRingGroupMember"
        logs[1].roleEnumId == "CurRingGroupMember"
        logs[2] == null
        answered.errors == ""
        late.reasonCode == "notRinging"
        m1Leg.statusId == "ClsCanceled"
        m1Log.outcomeEnumId == "CuoMissed"
        m1Log.seenDate != null
        m2Log.outcomeEnumId == "CuoAnswered"
        lateTimer.reasonCode == "answered"
    }
})

feature("a member's mobile rings with them, and its answer wins over every member's tab", {
    when() {
        upsert("moqui.call.VoiceUserSetting", {userId: M1}, {outboundEnabled: "Y", forwardEnabled: "Y"})
        as(M1, "update#CallHandlingSetting", {mobileNumber: MOBILE_M1, ringMobile: "Y"})
        const call = receive()
        const mobile = legsOfUser(call.callSessionId, M1).find((l) => l.legTypeEnumId === "CltPstn")
        const answered = note(call.callSessionId, mobile.callLegId, "answered")
        const tab = as(M2, "org.moqui.call.CallServices.answer#Call", {callSessionId: call.callSessionId})
        const stillOffered = offered(call.callSessionId)
        as(M1, "update#CallHandlingSetting", {ringMobile: "N"})
        // the mobile hangs up: the call ends
        note(call.callSessionId, mobile.callLegId, "ended")
        return {mobile, answered, tab, stillOffered, m1Log: log(call.callSessionId, M1)}
    },
    then({mobile, answered, tab, stillOffered, m1Log}) {
        mobile.phoneNumber == MOBILE_M1
        mobile.statusId == "ClsOffered"
        answered.errors == ""
        tab.reasonCode == "notRinging"
        stillOffered.length == 0
        m1Log.outcomeEnumId == "CuoAnswered"
    }
})

feature("in order: one member at a time, the next when the step's timer ends, then the group's voicemail", {
    when() {
        as(ADMIN, "store#RingGroup", {ringGroupId: groupId, strategyEnumId: "RgsInOrder"})
        const call = receive()
        const first = offered(call.callSessionId)
        const next = begin(call)
        const second = offered(call.callSessionId)
        const m1Log = log(call.callSessionId, M1)
        const third = begin(call)
        const thirdRung = offered(call.callSessionId)
        // M3 declines: nothing else rings, so the plan goes on to the fallback
        const declined = as(M3, "org.moqui.call.CallServices.decline#Call", {callSessionId: call.callSessionId})
        const fallback = begin(call, "declined")
        call2 = call
        return {call, first, next, second, m1Log, third, thirdRung, declined, fallback,
            groupBox: sys(() => ec.entity.find("moqui.call.VoicemailBox").condition("ringGroupId", groupId).one())}
    },
    then({call, first, next, second, m1Log, third, thirdRung, declined, fallback, groupBox}) {
        call.voicemailSeconds == 15
        first.join(",") == M1
        next.errors == ""
        next.mode == "ring"
        next.ringSeconds == 15
        second.join(",") == M2
        m1Log.outcomeEnumId == "CuoMissed"
        third.mode == "ring"
        thirdRung.join(",") == M3
        declined.errors == ""
        fallback.errors == ""
        fallback.mode == "deposit"
        fallback.voicemailBoxId == groupBox.voicemailBoxId
    }
})

feature("the group's box is its members': a message one of them heard is heard for all; anyone else is refused the same id", {
    when() {
        const path = ec.factory.getRuntimePath() + "/tmp/voicemail/rg-spec-1.ogg"
        ec.resource.getLocationReference("file:" + path).putText("OggS a ring group's message")
        const box = sys(() => ec.entity.find("moqui.call.VoicemailBox").condition("ringGroupId", groupId).one())
        const stored = engine("org.moqui.call.VoicemailServices.store#Voicemail", {voicemailBoxId: box.voicemailBoxId, callSessionId: call2.callSessionId,
            tempPath: path, durationMs: 3000, fromNumber: CALLER, fromName: "Spec Caller"})
        const m1Before = as(M1, "org.moqui.call.VoicemailServices.count#UnheardVoicemails")
        const m1List = as(M1, "org.moqui.call.VoicemailServices.find#Voicemails", {})
        const heard = as(M2, "org.moqui.call.VoicemailServices.mark#VoicemailHeard", {voicemailId: stored.voicemailId, heard: true})
        const m1After = as(M1, "org.moqui.call.VoicemailServices.get#Voicemail", {voicemailId: stored.voicemailId})
        const outsider = as(OUT, "org.moqui.call.VoicemailServices.get#Voicemail", {voicemailId: stored.voicemailId})
        const outsiderAudio = as(OUT, "org.moqui.call.VoicemailServices.get#VoicemailAudio", {voicemailId: stored.voicemailId})
        // a member who leaves the group loses it; back in, they have it again
        as(ADMIN, "delete#RingGroupMember", {ringGroupId: groupId, userId: M3})
        const former = as(M3, "org.moqui.call.VoicemailServices.get#Voicemail", {voicemailId: stored.voicemailId})
        as(ADMIN, "store#RingGroupMember", {ringGroupId: groupId, userId: M3, sequenceNum: 30})
        // one member deletes it and another empties the trash they share
        const trashed = as(M2, "org.moqui.call.VoicemailServices.delete#Voicemail", {voicemailId: stored.voicemailId})
        const emptied = as(M1, "org.moqui.call.VoicemailServices.empty#VoicemailTrash", {})
        const gone = as(M2, "org.moqui.call.VoicemailServices.get#Voicemail", {voicemailId: stored.voicemailId})
        endCall(call2.callSessionId)
        return {stored, m1Before, m1List, heard, m1After, outsider, outsiderAudio, former, trashed, emptied, gone}
    },
    then({stored, m1Before, m1List, heard, m1After, outsider, outsiderAudio, former, trashed, emptied, gone}) {
        former.reasonCode == "notFound"
        trashed.errors == ""
        emptied.deletedCount == 1
        gone.reasonCode == "notFound"
        stored.errors == ""
        m1Before.unheardCount == 1
        m1Before.hasBox == true
        m1List.voicemailList[0].voicemailId == stored.voicemailId
        m1List.voicemailList[0].groupName == "RG spec sales"
        heard.errors == ""
        m1After.voicemail.heard == true
        outsider.reasonCode == "notFound"
        outsiderAudio.reasonCode == "notFound"
    }
})

feature("a voice admin sets a group box's greeting and notices; a member gets not found for the same group; callers to the group hear its greeting", {
    when() {
        const vm = (userId, name, parameters) => as(userId, "org.moqui.call.VoicemailServices." + name, parameters)
        const header = "RIFF$\u0000\u0000\u0000WAVEfmt \u0010\u0000\u0000\u0000\u0001\u0000\u0001\u0000\u0080»\u0000\u0000\u0000w\u0001\u0000\u0002\u0000\u0010\u0000data\u0000\u0000\u0000\u0000"
        const notices = vm(ADMIN, "update#VoicemailSettings", {ringGroupId: groupId, notifyEmail: "sales@example.com", attachAudio: "Y", rings: "4"})
        const greeting = vm(ADMIN, "store#VoicemailGreeting", {ringGroupId: groupId, file: uploadedFile("sales.wav", header, "audio/wav")})
        const read = vm(ADMIN, "get#VoicemailSettings", {ringGroupId: groupId})
        const played = vm(ADMIN, "get#VoicemailGreeting", {ringGroupId: groupId})
        // a member, with the same parameters
        const memberNotices = vm(M1, "update#VoicemailSettings", {ringGroupId: groupId, notifyEmail: "me@example.com"})
        const memberGreeting = vm(M1, "delete#VoicemailGreeting", {ringGroupId: groupId})
        const memberRead = vm(M1, "get#VoicemailSettings", {ringGroupId: groupId})
        const own = vm(M1, "get#VoicemailSettings", {})
        // the group's call that falls to its box hears the group's greeting
        as(ADMIN, "store#RingGroup", {ringGroupId: groupId, strategyEnumId: "RgsInOrder"})
        const box = sys(() => ec.entity.find("moqui.call.VoicemailBox").condition("ringGroupId", groupId).one())
        const removed = vm(ADMIN, "delete#VoicemailGreeting", {ringGroupId: groupId})
        return {notices, greeting, read, played, memberNotices, memberGreeting, memberRead, own, box, removed}
    },
    then({notices, greeting, read, played, memberNotices, memberGreeting, memberRead, own, box, removed}) {
        notices.errors == ""
        notices.box.notifyEmail == "sales@example.com"
        notices.box.attachAudio == true
        notices.box.rings == null
        notices.warning != null
        greeting.errors == ""
        read.box.hasGreeting == true
        read.box.notifyEmail == "sales@example.com"
        played.location != null
        memberNotices.reasonCode == "notFound"
        memberGreeting.reasonCode == "notFound"
        memberRead.reasonCode == "notFound"
        own.box.notifyEmail != "sales@example.com"
        box.greetingLocation == played.location
        removed.box.hasGreeting == false
    }
})

feature("a stale step's timer or voicemail control does nothing: only the step the plan is on moves it on", {
    when() {
        const call = receive()
        const first = offered(call.callSessionId)
        const next = begin(call, "noAnswer", call.stepToken)
        // the first step's decline (or its timer) arrives after the second step began: it is stale, and M2 rings on
        const stale = begin(call, "declined", call.stepToken)
        const stillRinging = offered(call.callSessionId)
        const m2Log = log(call.callSessionId, M2)
        const third = begin(call, "noAnswer", next.stepToken)
        const thirdRung = offered(call.callSessionId)
        endCall(call.callSessionId)
        return {call, first, next, stale, stillRinging, m2Log, third, thirdRung}
    },
    then({call, first, next, stale, stillRinging, m2Log, third, thirdRung}) {
        call.stepToken != null
        first.join(",") == M1
        next.mode == "ring"
        next.stepToken != call.stepToken
        stale.reasonCode == "stale"
        stillRinging.join(",") == M2
        m2Log.outcomeEnumId == null
        third.mode == "ring"
        thirdRung.join(",") == M3
    }
})

feature("round robin starts after the member rung first last time, and keeps that across calls", {
    when() {
        as(ADMIN, "store#RingGroup", {ringGroupId: groupId, strategyEnumId: "RgsRoundRobin"})
        sys(() => {
            const g = ec.entity.find("moqui.call.RingGroup").condition("ringGroupId", groupId).one()
            g.set("lastMemberUserId", null)
            g.update()
        })
        const firstCall = receive()
        const first = offered(firstCall.callSessionId)
        endCall(firstCall.callSessionId)
        const secondCall = receive()
        const second = offered(secondCall.callSessionId)
        endCall(secondCall.callSessionId)
        // a member in a call (call waiting off) is skipped
        status(M3, {inCall: "Y"})
        const thirdCall = receive()
        const third = offered(thirdCall.callSessionId)
        endCall(thirdCall.callSessionId)
        status(M3, {inCall: "N"})
        return {first, second, third, last: sys(() => ec.entity.find("moqui.call.RingGroup").condition("ringGroupId", groupId).one().lastMemberUserId)}
    },
    then({first, second, third, last}) {
        first.join(",") == M1
        second.join(",") == M2
        third.join(",") == M1
        last == M1
    }
})

feature("outside its schedule a group goes straight to its fallback: its box, another group, or an outside number", {
    when() {
        const schedule = as(ADMIN, "store#CommsSchedule", {name: "RG spec hours", timeZone: "Europe/Berlin", shared: true})
        const notShared = as(OUT, "store#CommsSchedule", {name: "RG spec hours", timeZone: "Europe/Berlin", shared: true})
        as(ADMIN, "store#CommsScheduleRule", {commsScheduleId: schedule.commsScheduleId, ruleTypeEnumId: "CsrWeekly", dayOfWeek: 1, fromTime: "08:00", thruTime: "12:00"})
        as(ADMIN, "store#RingGroup", {ringGroupId: groupId, commsScheduleId: schedule.commsScheduleId, strategyEnumId: "RgsSimultaneous"})
        const evaluate = (atMs) => engine("evaluate#CallHandling", {ringGroupId: groupId, fromNumber: CALLER, atMs})
        // Monday 2026-11-02 in Berlin (UTC+1): 10:00 is 09:00 UTC (open), 13:00 is 12:00 UTC (closed)
        const open = evaluate(new Date("2026-11-02T09:00:00Z").getTime())
        const closed = evaluate(new Date("2026-11-02T12:00:00Z").getTime())
        const other = as(ADMIN, "store#RingGroup", {name: "RG spec support", strategyEnumId: "RgsSimultaneous", fallbackTypeEnumId: "RgfNone"})
        as(ADMIN, "store#RingGroupMember", {ringGroupId: other.ringGroupId, userId: OUT})
        as(ADMIN, "store#RingGroup", {ringGroupId: groupId, fallbackTypeEnumId: "RgfGroup", fallbackId: other.ringGroupId})
        const toGroup = evaluate(new Date("2026-11-02T12:00:00Z").getTime())
        const toItself = as(ADMIN, "store#RingGroup", {ringGroupId: groupId, fallbackTypeEnumId: "RgfGroup", fallbackId: groupId})
        as(ADMIN, "store#RingGroup", {ringGroupId: groupId, fallbackTypeEnumId: "RgfNumber", fallbackId: "+15559990200"})
        const toNumber = evaluate(new Date("2026-11-02T12:00:00Z").getTime())
        const toEmergency = as(ADMIN, "store#RingGroup", {ringGroupId: groupId, fallbackTypeEnumId: "RgfNumber", fallbackId: "911"})
        const inUse = as(ADMIN, "delete#CommsSchedule", {commsScheduleId: schedule.commsScheduleId})
        as(ADMIN, "store#RingGroup", {ringGroupId: groupId, fallbackTypeEnumId: "RgfVoicemail", commsScheduleId: ""})
        return {schedule, notShared, open, closed, toGroup, toItself, toNumber, toEmergency, inUse}
    },
    then({schedule, notShared, open, closed, toGroup, toItself, toNumber, toEmergency, inUse}) {
        schedule.errors == ""
        notShared.reasonCode == "notAllowed"
        open.stepList[0].type == "ring"
        open.stepList[0].users.length == 3
        closed.stepList.map((s) => s.type).join(",") == "voicemail"
        toGroup.stepList[0].users.join(",") == OUT
        toGroup.stepList[1].type == "end"
        toItself.reasonCode == "notFound"
        toNumber.stepList[0].numbers[0].number == "+15559990200"
        toNumber.stepList[0].numbers[0].byAdmin == true
        toEmergency.reasonCode == "badNumber"
        inUse.reasonCode == "inUse"
    }
})
