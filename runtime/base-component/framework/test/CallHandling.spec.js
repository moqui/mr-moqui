// Call handling (org.moqui.impl.CallHandlingServices and CallHandling.js; the V3 block of framework/entity/VoiceEntities.xml;
// plans/Comms-Framework.md §5.3, §5.4, V3): a user's rules for an inbound phone call, read in order. Anonymous and selective
// rejection and acceptance by caller list, selective forward, business hours and holidays across a time zone (the plan read
// at a given time, atMs, not by waiting), do not disturb to voicemail, busy with and without call waiting, forward loops
// and the hop bound, a forward back to the caller skipped, a forward to an outside number only with the forward permission,
// simultaneous ring to a mobile and the first answer winning over it, and presence from legs: one ended leg leaves the
// other call's. No SIP engine runs: fake SipNode and CallMediaNode rows stand in, and the engine's reports are
// note#PhoneLeg calls. Every refusal has its positive case with the same parameters.
// Run: ./moq test framework --spec CallHandling
const A = "CH_SPEC_A"
const B = "CH_SPEC_B"
const CHAIN = ["CH_SPEC_C1", "CH_SPEC_C2", "CH_SPEC_C3", "CH_SPEC_C4", "CH_SPEC_C5", "CH_SPEC_C6", "CH_SPEC_C7"]
const USERS = [A, B].concat(CHAIN)
const NODE = "ch-spec-node"
const TRUNK = "CH_SPEC_TRUNK"
const NUMBER_A = "+15550190001"
const NUMBER_B = "+15550190002"
const CALLER = "+15557770190"
const OTHER_CALLER = "+15557770191"
const MOBILE = "+15558880190"
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
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : "org.moqui.impl.CallHandlingServices." + name)
            .parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const engine = (name, parameters) => as(null, name, parameters)
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "Handling " + userId.slice(8), disabled: "N"}, true, null, null)
            account.create()
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
const status = (userId, fields) => upsert("moqui.security.UserStatus", {userId}, fields)
const voiceSetting = (userId, fields) => upsert("moqui.call.VoiceUserSetting", {userId}, fields)
function clean() {
    sys(() => {
        for (const s of ec.entity.find("moqui.call.CallSession").condition("kindEnumId", "CskPhone").condition("statusId", "!=", "CssEnded").list()) {
            s.set("statusId", "CssEnded")
            s.update()
        }
        for (const entity of ["moqui.call.CallHandlingRule", "moqui.call.CallHandlingSetting", "moqui.call.VoiceUserSetting", "moqui.security.UserStatus"]) {
            ec.entity.find(entity).condition("userId", "in", USERS).deleteAll()
        }
        for (const list of ec.entity.find("moqui.call.CallerList").condition("userId", "in", USERS).list()) {
            ec.entity.find("moqui.call.CallerListEntry").condition("callerListId", list.callerListId).deleteAll()
            list.delete()
        }
        for (const s of ec.entity.find("moqui.call.CommsSchedule").condition("name", "like", "CH spec%").list()) {
            ec.entity.find("moqui.call.CommsScheduleRule").condition("commsScheduleId", s.commsScheduleId).deleteAll()
            s.delete()
        }
        ec.entity.find("moqui.call.PhoneNumber").condition("phoneNumber", "in", [NUMBER_A, NUMBER_B]).deleteAll()
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
const receive = (toNumber, fromNumber) => engine("org.moqui.impl.VoiceServices.receive#PhoneCall", {toNumber, fromNumber: fromNumber === undefined ? CALLER : fromNumber,
    fromName: "Spec Caller", instanceId: NODE})
const evaluate = (userId, fromNumber, atMs) => engine("evaluate#CallHandling", {userId, fromNumber: fromNumber === undefined ? CALLER : fromNumber, atMs: atMs || null})
const legsOf = (callSessionId) => sys(() => Array.from(ec.entity.find(LEG).condition("callSessionId", callSessionId).orderBy("offeredDate").list()))
function userLegs(callSessionId, userId) {
    return sys(() => {
        const p = ec.entity.find("moqui.call.CallParticipant").condition({callSessionId, userId}).one()
        return p ? Array.from(ec.entity.find(LEG).condition("callParticipantId", p.callParticipantId).list()) : []
    })
}
const note = (callSessionId, callLegId, event) => engine("org.moqui.impl.VoiceServices.note#PhoneLeg", {callSessionId, callLegId, event})
const inCall = (userId) => sys(() => ec.entity.find("moqui.security.UserStatus").condition("userId", userId).one()?.inCall || "N")
const log = (callSessionId, userId) => sys(() => ec.entity.find("moqui.call.CallUserLog").condition({callSessionId, userId}).one())
const rule = (userId, fields) => as(userId, "store#CallHandlingRule", fields)
const clearRules = (userId) => sys(() => ec.entity.find("moqui.call.CallHandlingRule").condition("userId", userId).deleteAll())
const types = (out) => Array.from(out.stepList || []).map((s) => s.type).join(",")
let forwardedCall = null
const ringUsers = (out, i) => Array.from((out.stepList || [])[i || 0]?.users || [])

spec({
    stepwise: true,
    setupSpec() {
        for (const userId of USERS) makeUser(userId)
        clean()
        upsert("moqui.call.VoiceTrunk", {voiceTrunkId: TRUNK}, {name: "Spec trunk", host: "sip.example.invalid", transportEnumId: "VtrTls",
            mediaEncryptionEnumId: "VmeSrtp", providerProfileEnumId: "VppGeneric", statusId: "VtsOk", disabled: "N"})
        upsert("moqui.call.PhoneNumber", {phoneNumber: NUMBER_A}, {voiceTrunkId: TRUNK, routeTypeEnumId: "PnrUser", routeId: A})
        upsert("moqui.call.PhoneNumber", {phoneNumber: NUMBER_B}, {voiceTrunkId: TRUNK, routeTypeEnumId: "PnrUser", routeId: B})
        nodesUp()
    },
    cleanupSpec() {
        clean()
    }
})

feature("an anonymous caller is refused by an anonymous rejection rule, a caller with a number rings", {
    when() {
        const before = receive(NUMBER_A, "anonymous")
        const made = rule(A, {kindEnumId: "ChrAnonymousReject"})
        const refused = receive(NUMBER_A, "anonymous")
        const blank = receive(NUMBER_A, "")
        const numbered = receive(NUMBER_A, CALLER)
        // another user's rule is not B's
        const other = receive(NUMBER_B, "anonymous")
        clearRules(A)
        return {before, made, refused, blank, numbered, other, legs: userLegs(numbered.callSessionId, A)}
    },
    then({before, made, refused, blank, numbered, other, legs}) {
        before.errors == ""
        before.callSessionId != null
        made.errors == ""
        refused.reasonCode == "anonymous"
        refused.callSessionId == null
        blank.reasonCode == "anonymous"
        numbered.reasonCode == null
        numbered.voicemailSeconds == 45
        legs.length == 1
        legs[0].statusId == "ClsOffered"
        other.callSessionId != null
    }
})

feature("selective reject, accept and forward read the caller's number against the user's own lists", {
    when() {
        const list = as(A, "store#CallerList", {name: "Blocked"})
        const added = as(A, "add#CallerListEntry", {callerListId: list.callerListId, phoneNumber: "(555) 777-0190"})
        const prefix = as(A, "add#CallerListEntry", {callerListId: list.callerListId, phoneNumber: "+1666*"})
        const badPrefix = as(A, "add#CallerListEntry", {callerListId: list.callerListId, phoneNumber: "1666*"})
        // B may not use A's list, with the same parameters
        const notHis = rule(B, {kindEnumId: "ChrSelectiveReject", callerListId: list.callerListId})
        const reject = rule(A, {kindEnumId: "ChrSelectiveReject", callerListId: list.callerListId})
        const rejected = receive(NUMBER_A, CALLER)
        const byPrefix = evaluate(A, "+16665550000")
        const passes = evaluate(A, OTHER_CALLER)
        clearRules(A)
        // accept only the list: others refused, the listed rings
        rule(A, {kindEnumId: "ChrSelectiveAccept", callerListId: list.callerListId})
        const acceptedListed = evaluate(A, CALLER)
        const notListed = evaluate(A, OTHER_CALLER)
        clearRules(A)
        // the listed callers forward to B
        rule(A, {kindEnumId: "ChrSelectiveForward", callerListId: list.callerListId, targetTypeEnumId: "ChtUser", targetId: B})
        const forwarded = evaluate(A, CALLER)
        const notForwarded = evaluate(A, OTHER_CALLER)
        clearRules(A)
        return {list, added, prefix, badPrefix, notHis, reject, rejected, byPrefix, passes, acceptedListed, notListed, forwarded, notForwarded}
    },
    then({list, added, prefix, badPrefix, notHis, reject, rejected, byPrefix, passes, acceptedListed, notListed, forwarded, notForwarded}) {
        list.errors == ""
        added.phoneNumber == CALLER
        prefix.phoneNumber == "+1666*"
        badPrefix.reasonCode == "badNumber"
        notHis.reasonCode == "badRule"
        reject.errors == ""
        rejected.reasonCode == "declined"
        types(byPrefix).split(",")[0] == "reject"
        types(passes).split(",")[0] == "ring"
        types(acceptedListed).split(",")[0] == "ring"
        types(notListed).split(",")[0] == "reject"
        ringUsers(forwarded)[0] == B
        ringUsers(notForwarded)[0] == A
    }
})

// business hours in America/Chicago (UTC-6 in November): Monday to Friday 09:00 to 17:00, closed on December 25 every year
const UTC = (text) => new Date(text).getTime()
feature("business hours and holidays are read in the schedule's time zone at the call's time", {
    when() {
        const schedule = as(A, "store#CommsSchedule", {name: "CH spec office", timeZone: "America/Chicago"})
        const badZone = as(A, "store#CommsSchedule", {name: "CH spec bad", timeZone: "Mars/Olympus"})
        for (let day = 1; day <= 5; day++) as(A, "store#CommsScheduleRule", {commsScheduleId: schedule.commsScheduleId, ruleTypeEnumId: "CsrWeekly",
            dayOfWeek: day, fromTime: "09:00", thruTime: "17:00"})
        const holiday = as(A, "store#CommsScheduleRule", {commsScheduleId: schedule.commsScheduleId, ruleTypeEnumId: "CsrHoliday",
            onDate: "2026-12-25", repeatYearly: "Y"})
        // B may not change A's schedule, with the same parameters
        const notHis = as(B, "store#CommsScheduleRule", {commsScheduleId: schedule.commsScheduleId, ruleTypeEnumId: "CsrHoliday", onDate: "2026-12-26"})
        // outside the hours, calls go to voicemail
        rule(A, {kindEnumId: "ChrAlways", targetTypeEnumId: "ChtVoicemail", commsScheduleId: schedule.commsScheduleId, scheduleWhen: "closed"})
        const check = (when) => as(A, "check#CommsSchedule", {commsScheduleId: schedule.commsScheduleId, atMs: UTC(when)})
        const out = {
            schedule, badZone, holiday, notHis,
            // Monday 2026-11-02: 08:30 in Chicago is 14:30 UTC (closed), 09:30 is 15:30 UTC (open)
            early: evaluate(A, CALLER, UTC("2026-11-02T14:30:00Z")), open: evaluate(A, CALLER, UTC("2026-11-02T15:30:00Z")),
            // 23:30 UTC Monday is 17:30 Monday in Chicago: closed, though it is still Monday in UTC
            evening: evaluate(A, CALLER, UTC("2026-11-02T23:30:00Z")),
            // Saturday, and Christmas on a Friday at 10:00 Chicago (in 2026 and again in 2027 by the yearly rule)
            weekend: evaluate(A, CALLER, UTC("2026-11-07T16:00:00Z")), christmas: evaluate(A, CALLER, UTC("2026-12-25T16:00:00Z")),
            nextChristmas: evaluate(A, CALLER, UTC("2028-12-25T16:00:00Z")), dayAfter: evaluate(A, CALLER, UTC("2026-12-28T16:00:00Z")),
            checked: check("2026-11-02T15:30:00Z"), checkedHoliday: check("2026-12-25T16:00:00Z"),
        }
        clearRules(A)
        return out
    },
    then(out) {
        out.schedule.errors == ""
        out.badZone.reasonCode == "badZone"
        out.holiday.errors == ""
        out.notHis.reasonCode == "notFound"
        types(out.early).split(",")[0] == "voicemail"
        types(out.open).split(",")[0] == "ring"
        types(out.evening).split(",")[0] == "voicemail"
        types(out.weekend).split(",")[0] == "voicemail"
        types(out.christmas).split(",")[0] == "voicemail"
        types(out.nextChristmas).split(",")[0] == "voicemail"
        types(out.dayAfter).split(",")[0] == "ring"
        out.checked.open == true
        out.checked.localDate == "2026-11-02"
        out.checked.localMinutes == 570
        out.checked.dayOfWeek == 1
        out.checkedHoliday.open == false
    }
})

feature("forward loops are bounded: no forward back to a user already in the plan, at most call_forward_max_hops forwards, none to the caller", {
    when() {
        // A forwards to B and B back to A: B rings
        rule(A, {kindEnumId: "ChrAlways", targetTypeEnumId: "ChtUser", targetId: B})
        rule(B, {kindEnumId: "ChrAlways", targetTypeEnumId: "ChtUser", targetId: A})
        const loop = evaluate(A, CALLER)
        const self = rule(A, {kindEnumId: "ChrAlways", targetTypeEnumId: "ChtUser", targetId: A})
        clearRules(A)
        clearRules(B)
        // a chain of seven: the sixth forward is past the bound of five, so the sixth user rings
        for (let i = 0; i < CHAIN.length - 1; i++) rule(CHAIN[i], {kindEnumId: "ChrAlways", targetTypeEnumId: "ChtUser", targetId: CHAIN[i + 1]})
        const chain = evaluate(CHAIN[0], CALLER)
        for (const userId of CHAIN) clearRules(userId)
        // a forward to the caller's own number is skipped, even with the permission
        voiceSetting(A, {outboundEnabled: "Y", forwardEnabled: "Y"})
        rule(A, {kindEnumId: "ChrAlways", targetTypeEnumId: "ChtNumber", targetId: CALLER})
        const toCaller = evaluate(A, CALLER)
        const toOther = evaluate(A, OTHER_CALLER)
        clearRules(A)
        // a forward to one of our own numbers routes inside: to B, never through the trunk
        rule(A, {kindEnumId: "ChrAlways", targetTypeEnumId: "ChtNumber", targetId: NUMBER_B})
        const own = evaluate(A, CALLER)
        clearRules(A)
        sys(() => ec.entity.find("moqui.call.VoiceUserSetting").condition("userId", A).deleteAll())
        return {loop, self, chain, toCaller, toOther, own}
    },
    then({loop, self, chain, toCaller, toOther, own}) {
        types(loop) == "ring,voicemail"
        ringUsers(loop)[0] == B
        loop.traceList.some((t) => t.includes("a loop"))
        self.reasonCode == "notFound"
        ringUsers(chain)[0] == CHAIN[5]
        chain.traceList.some((t) => t.includes("more than 5 forwards"))
        ringUsers(toCaller)[0] == A
        toCaller.traceList.some((t) => t.includes("it is the caller"))
        toOther.stepList[0].numbers[0].number == CALLER
        ringUsers(own)[0] == B
        own.stepList[0].numbers.length == 0
    }
})

feature("do not disturb sends calls straight to voicemail; busy goes there too unless call waiting is on", {
    when() {
        status(A, {statusEnumId: "UpsDnd"})
        const dnd = receive(NUMBER_A)
        const begun = engine("org.moqui.impl.VoicemailServices.begin#Voicemail", {callSessionId: dnd.callSessionId, callLegId: dnd.callLegId, reason: "busy"})
        // a status that cleared is not do not disturb
        status(A, {statusEnumId: "UpsDnd", clearAtDate: new Date(Date.now() - 60000)})
        const cleared = evaluate(A)
        status(A, {statusEnumId: "UpsOnline", clearAtDate: null, inCall: "Y"})
        const busy = evaluate(A)
        const waiting = as(A, "update#CallHandlingSetting", {callWaiting: "Y"})
        const rings = evaluate(A)
        // the busy rule's target: B
        as(A, "update#CallHandlingSetting", {callWaiting: "N"})
        rule(A, {kindEnumId: "ChrBusy", targetTypeEnumId: "ChtUser", targetId: B})
        const busyForward = evaluate(A)
        clearRules(A)
        status(A, {inCall: "N"})
        // the user's own switch, from Settings: do not disturb is their status
        const viaSettings = as(A, "update#CallHandlingSetting", {dnd: "Y"})
        const dndSetting = evaluate(A)
        as(A, "update#CallHandlingSetting", {dnd: "N"})
        return {dnd, begun, cleared, busy, waiting, rings, busyForward, viaSettings, dndSetting, aLeg: userLegs(dnd.callSessionId, A)}
    },
    then({dnd, begun, cleared, busy, waiting, rings, busyForward, viaSettings, dndSetting, aLeg}) {
        dnd.voicemail == "busy"
        begun.errors == ""
        begun.mode == "deposit"
        aLeg.length == 1
        types(cleared).split(",")[0] == "ring"
        types(busy).split(",")[0] == "voicemail"
        waiting.settings.callWaiting == true
        types(rings).split(",")[0] == "ring"
        ringUsers(busyForward)[0] == B
        viaSettings.settings.dnd == true
        types(dndSetting).split(",")[0] == "voicemail"
    }
})

feature("a forward to an outside number needs the forward permission, and the first answer wins over the user's tabs", {
    when() {
        rule(A, {kindEnumId: "ChrNoAnswer", targetTypeEnumId: "ChtNumber", targetId: MOBILE})
        // without the permission the forward is left out: voicemail after the ring
        const without = evaluate(A)
        voiceSetting(A, {outboundEnabled: "Y", forwardEnabled: "Y"})
        const allowed = evaluate(A)
        // international is off until the deployment and the user allow it
        rule(A, {kindEnumId: "ChrAlways", targetTypeEnumId: "ChtNumber", targetId: "+442071234567"})
        const international = evaluate(A)
        const emergency = rule(A, {kindEnumId: "ChrAlways", targetTypeEnumId: "ChtNumber", targetId: "911"})
        clearRules(A)
        // simultaneous ring to the mobile: the tab and the mobile ring together
        const mobile = as(A, "update#CallHandlingSetting", {mobileNumber: "555 888 0190", ringMobile: "Y"})
        const call = receive(NUMBER_A)
        forwardedCall = call.callSessionId
        const rung = userLegs(call.callSessionId, A)
        const mobileLeg = rung.find((l) => l.legTypeEnumId === "CltPstn")
        const answered = note(call.callSessionId, mobileLeg.callLegId, "answered")
        const after = userLegs(call.callSessionId, A)
        const tabAnswer = as(A, "org.moqui.impl.CallServices.answer#Call", {callSessionId: call.callSessionId})
        const presence = inCall(A)
        return {without, allowed, international, emergency, mobile, call, rung, answered, after, tabAnswer, presence, mobileLeg}
    },
    then({without, allowed, international, emergency, mobile, call, rung, answered, after, tabAnswer, presence, mobileLeg}) {
        types(without) == "ring,voicemail"
        types(allowed) == "ring,ring,voicemail"
        allowed.stepList[1].numbers[0].number == MOBILE
        types(international).split(",")[0] == "ring"
        ringUsers(international)[0] == A
        international.traceList.some((t) => t.includes("internationalNotEnabled"))
        emergency.reasonCode == "badNumber"
        mobile.settings.mobileNumber == MOBILE
        mobile.warning == null
        call.voicemailSeconds == 45
        rung.length == 2
        mobileLeg.phoneNumber == MOBILE
        answered.errors == ""
        after.find((l) => l.legTypeEnumId === "CltWeb").statusId == "ClsCanceled"
        after.find((l) => l.legTypeEnumId === "CltPstn").statusId == "ClsAnswered"
        tabAnswer.reasonCode == "notRinging"
        presence == "Y"
    }
})

feature("presence comes from the user's legs: leaving one of two calls keeps them in a call, a phone leg and a tab leg", {
    when() {
        // A is in a call on their mobile (the forwarded leg of the last feature) and starts a call from a tab to B
        const phoneCall = {callSessionId: forwardedCall}
        const phoneLeg = userLegs(phoneCall.callSessionId, A).find((l) => l.legTypeEnumId === "CltPstn" && l.statusId === "ClsAnswered")
        const tabCall = as(A, "org.moqui.impl.CallServices.start#Call", {calleeUserIdList: [B], video: false})
        as(B, "org.moqui.impl.CallServices.answer#Call", {callSessionId: tabCall.callSessionId})
        const both = inCall(A)
        // the tab leg ends: the phone call still holds A
        as(A, "org.moqui.impl.CallServices.leave#Call", {callSessionId: tabCall.callSessionId})
        const afterTab = inCall(A)
        // then the mobile hangs up: A is in no call
        note(phoneCall.callSessionId, phoneLeg.callLegId, "ended")
        const afterPhone = inCall(A)
        return {both, afterTab, afterPhone, b: inCall(B)}
    },
    then({both, afterTab, afterPhone, b}) {
        both == "Y"
        afterTab == "Y"
        afterPhone == "N"
        b == "N"
    }
})

feature("the user's settings and rules are their own: another user is refused with the same parameters", {
    when() {
        const made = rule(A, {kindEnumId: "ChrAlways", targetTypeEnumId: "ChtVoicemail"})
        const theirs = as(B, "delete#CallHandlingRule", {callHandlingRuleId: made.callHandlingRuleId})
        const mine = as(A, "get#CallHandling")
        const removed = as(A, "delete#CallHandlingRule", {callHandlingRuleId: made.callHandlingRuleId})
        const noTarget = rule(A, {kindEnumId: "ChrAlways"})
        const badKind = rule(A, {kindEnumId: "ChrNothing"})
        return {made, theirs, mine, removed, noTarget, badKind}
    },
    then({made, theirs, mine, removed, noTarget, badKind}) {
        made.errors == ""
        theirs.reasonCode == "notFound"
        mine.ruleList.some((r) => r.callHandlingRuleId == made.callHandlingRuleId)
        mine.kindList.length == 7
        removed.errors == ""
        noTarget.reasonCode == "badRule"
        badKind.reasonCode == "badRule"
    }
})
