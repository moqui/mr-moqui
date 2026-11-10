// The IVR and auto attendant (org.moqui.call.IvrServices and Ivr.js; plans/Comms-Framework.md §5.3, §5.4, V5): only a voice
// admin keeps menus (the same parameters refused for anyone else); a number routed to a menu is answered with the menu
// (receive#PhoneCall's ivr); the caller's keys route through route#IvrCall: to a user (V3's plan: their tabs ring), a
// nested menu, a queue, a voicemail box; no key goes to the timeout target, a key no option has to the invalid target
// or, without one, the menu again; outside its schedule a menu goes to its closed target; a loop of menus ends; a number
// target is held to the deployment's fraud policy. The keys themselves (the prompt, retries, timeouts) are the SIP
// engine's (moqui-host src/sip/ivr.rs, its unit tests on simulated time). No SIP engine runs here: fake SipNode and
// CallMediaNode rows stand in for one.
// Run: ./moq test framework --spec Ivr
const ADMIN = "IVR_SPEC_ADMIN"
const SALES = "IVR_SPEC_SALES"
const OUT = "IVR_SPEC_OUT"
const USERS = [ADMIN, SALES, OUT]
const NODE = "ivr-spec-node"
const TRUNK = "IVR_SPEC_TRUNK"
const MAIN_NUMBER = "+15550300001"
const CALLER = "+15557770300"

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
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : "org.moqui.call.IvrServices." + name)
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
            account.setFields({userId, username: username(userId), userFullName: "Ivr " + userId.slice(9), disabled: "N"}, true, null, null)
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
        ec.entity.find("moqui.call.PhoneNumber").condition("phoneNumber", MAIN_NUMBER).deleteAll()
        for (const menu of ec.entity.find("moqui.call.IvrMenu").condition("name", "like", "Ivr spec%").list()) {
            ec.entity.find("moqui.call.IvrOption").condition("ivrMenuId", menu.ivrMenuId).deleteAll()
            menu.delete()
        }
        for (const queue of ec.entity.find("moqui.call.CallQueue").condition("name", "like", "Ivr spec%").list()) {
            ec.entity.find("moqui.call.CallQueueEntry").condition("callQueueId", queue.callQueueId).deleteAll()
            ec.entity.find("moqui.call.CallQueueAgent").condition("callQueueId", queue.callQueueId).deleteAll()
            queue.delete()
        }
        for (const s of ec.entity.find("moqui.call.CommsSchedule").condition("name", "like", "Ivr spec%").list()) {
            ec.entity.find("moqui.call.CommsScheduleRule").condition("commsScheduleId", s.commsScheduleId).deleteAll()
            s.delete()
        }
        ec.entity.find("moqui.call.VoicemailBox").condition("userId", "in", USERS).deleteAll()
        ec.entity.find("moqui.call.CallHandlingRule").condition("userId", "in", USERS).deleteAll()
        ec.entity.find("moqui.security.UserStatus").condition("userId", "in", USERS).deleteAll()
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
const receive = (atMs) => engine("org.moqui.call.VoiceServices.receive#PhoneCall", {toNumber: MAIN_NUMBER, fromNumber: CALLER,
    fromName: "Ivr Caller", instanceId: NODE, atMs: atMs || null})
const route = (call, ivrMenuId, kind, digits) => engine("org.moqui.call.IvrServices.route#IvrCall", {callSessionId: call.callSessionId,
    callLegId: call.callLegId, ivrMenuId, kind, digits: digits || null}).answer || {}
const offered = (callSessionId, userId) => sys(() => {
    const p = ec.entity.find("moqui.call.CallParticipant").condition({callSessionId, userId}).one()
    return !!p && ec.entity.find("moqui.call.CallLeg").condition({callParticipantId: p.callParticipantId, statusId: "ClsOffered"}).count() > 0
})
const endCall = (callSessionId) => sys(() => {
    const s = ec.entity.find("moqui.call.CallSession").condition("callSessionId", callSessionId).one()
    if (s && s.statusId !== "CssEnded") { s.set("statusId", "CssEnded"); s.update() }
})

let main = null
let sub = null
let queueId = null
let boxId = null

spec({
    stepwise: true,
    setupSpec() {
        makeUser(ADMIN, "ADMIN")
        makeUser(SALES)
        makeUser(OUT)
        clean()
        upsert("moqui.call.VoiceTrunk", {voiceTrunkId: TRUNK}, {name: "Ivr spec trunk", host: "sip.example.invalid", transportEnumId: "VtrTls",
            mediaEncryptionEnumId: "VmeSrtp", providerProfileEnumId: "VppGeneric", statusId: "VtsOk", disabled: "N"})
        nodesUp()
        upsert("moqui.call.VoicemailBox", {voicemailBoxId: "IVR_SPEC_BOX"}, {userId: SALES, attachAudio: "N", attachTranscript: "N"})
        boxId = "IVR_SPEC_BOX"
    },
    cleanupSpec() {
        clean()
    }
})

feature("only a voice admin keeps menus and routes a number to one; a bad target is refused", {
    when() {
        const fields = {name: "Ivr spec main", timeoutSeconds: 4, maxRetries: 1, timeoutTypeEnumId: "CtgVoicemail", timeoutId: "IVR_SPEC_BOX"}
        const refused = as(OUT, "store#IvrMenu", fields)
        const made = as(ADMIN, "store#IvrMenu", fields)
        main = made.ivrMenuId
        sub = as(ADMIN, "store#IvrMenu", {name: "Ivr spec sales"}).ivrMenuId
        queueId = as(ADMIN, "org.moqui.call.CallQueueServices.store#CallQueue", {name: "Ivr spec queue"}).callQueueId
        const optionRefused = as(OUT, "store#IvrOption", {ivrMenuId: main, digits: "1", targetTypeEnumId: "CtgUser", targetId: SALES})
        as(ADMIN, "store#IvrOption", {ivrMenuId: main, digits: "1", targetTypeEnumId: "CtgUser", targetId: SALES})
        as(ADMIN, "store#IvrOption", {ivrMenuId: main, digits: "2", targetTypeEnumId: "CtgMenu", targetId: sub})
        as(ADMIN, "store#IvrOption", {ivrMenuId: main, digits: "3", targetTypeEnumId: "CtgQueue", targetId: queueId})
        as(ADMIN, "store#IvrOption", {ivrMenuId: sub, digits: "9", targetTypeEnumId: "CtgVoicemail", targetId: boxId})
        as(ADMIN, "store#IvrOption", {ivrMenuId: sub, digits: "0", targetTypeEnumId: "CtgRepeat"})
        const badDigits = as(ADMIN, "store#IvrOption", {ivrMenuId: main, digits: "12", targetTypeEnumId: "CtgEnd"})
        const badUser = as(ADMIN, "store#IvrOption", {ivrMenuId: main, digits: "4", targetTypeEnumId: "CtgUser", targetId: "NO_SUCH_USER"})
        // the deployment's policy holds an admin's number target too: international is off
        const abroad = as(ADMIN, "store#IvrOption", {ivrMenuId: main, digits: "5", targetTypeEnumId: "CtgNumber", targetId: "+442071234567"})
        const routed = as(ADMIN, "org.moqui.call.VoiceServices.store#PhoneNumber", {phoneNumber: MAIN_NUMBER, voiceTrunkId: TRUNK,
            routeTypeEnumId: "PnrIvr", routeId: main})
        const badRoute = as(ADMIN, "org.moqui.call.VoiceServices.store#PhoneNumber", {phoneNumber: MAIN_NUMBER, voiceTrunkId: TRUNK,
            routeTypeEnumId: "PnrIvr", routeId: "NO_SUCH_MENU"})
        const found = as(ADMIN, "find#IvrMenus")
        const findRefused = as(OUT, "find#IvrMenus")
        const inUse = as(ADMIN, "delete#IvrMenu", {ivrMenuId: sub})
        return {refused, made, optionRefused, badDigits, badUser, abroad, routed, badRoute, found, findRefused, inUse}
    },
    then({refused, made, optionRefused, badDigits, badUser, abroad, routed, badRoute, found, findRefused, inUse}) {
        refused.reasonCode == "notAllowed"
        made.errors == ""
        optionRefused.reasonCode == "notAllowed"
        badDigits.reasonCode == "badDigits"
        badUser.reasonCode == "notFound"
        abroad.reasonCode == "internationalNotEnabled"
        routed.errors == ""
        badRoute.reasonCode == "notFound"
        found.menuList.find((m) => m.ivrMenuId == main).optionList.map((o) => o.digits).join(",") == "1,2,3"
        found.menuList.find((m) => m.ivrMenuId == main).numberList[0] == MAIN_NUMBER
        found.menuList.find((m) => m.ivrMenuId == main).timeoutName == "Ivr SALES"
        findRefused.reasonCode == "notAllowed"
        inUse.reasonCode == "inUse"
    }
})

feature("DTMF routing: a call to the number gets the menu; 1 rings the user, 2 the nested menu, 9 there the box, 3 the queue", {
    when() {
        const call = receive()
        const toUser = route(call, main, "option", "1")
        const ringing = offered(call.callSessionId, SALES)
        const plan = sys(() => ec.entity.find("moqui.call.CallRoute").condition("callSessionId", call.callSessionId).one())
        endCall(call.callSessionId)
        const call2 = receive()
        const nested = route(call2, main, "option", "2")
        const repeat = route(call2, sub, "option", "0")
        const toBox = route(call2, sub, "option", "9")
        const path = sys(() => ec.entity.find("moqui.call.CallIvrState").condition("callSessionId", call2.callSessionId).one())
        // the engine starts voicemail with the plan's token
        const deposit = engine("org.moqui.call.VoicemailServices.begin#Voicemail", {callSessionId: call2.callSessionId,
            callLegId: call2.callLegId, reason: "busy", stepToken: toBox.stepToken})
        endCall(call2.callSessionId)
        const call3 = receive()
        const toQueue = route(call3, main, "option", "3")
        const entry = sys(() => ec.entity.find("moqui.call.CallQueueEntry").condition("callSessionId", call3.callSessionId).one())
        endCall(call3.callSessionId)
        return {call, toUser, ringing, plan, nested, repeat, toBox, path, deposit, toQueue, entry}
    },
    then({call, toUser, ringing, plan, nested, repeat, toBox, path, deposit, toQueue, entry}) {
        call.errors == ""
        call.ivr.mode == "menu"
        call.ivr.ivrMenuId == main
        call.ivr.timeoutSeconds == 4
        call.ivr.maxRetries == 1
        call.ivr.optionList.map((o) => o.digits).join(",") == "1,2,3"
        call.ivr.hasInvalid == false
        toUser.mode == "ring"
        toUser.ringSeconds > 0
        toUser.stepToken != null
        ringing == true
        plan.userId == SALES
        nested.mode == "menu"
        nested.ivrMenuId == sub
        repeat.mode == "menu"
        repeat.ivrMenuId == sub
        toBox.mode == "voicemail"
        path.pathText == "Ivr spec main: 2 > Ivr spec sales: 0 > Ivr spec sales: 9"
        deposit.mode == "deposit"
        deposit.voicemailBoxId == boxId
        toQueue.mode == "queue"
        toQueue.callQueueEntryId == entry.callQueueEntryId
        entry.statusId == "CqeWaiting"
    }
})

feature("no key goes to the timeout target, a key no option has to the invalid target, or without one the menu again", {
    when() {
        const call = receive()
        const timeout = route(call, main, "timeout")
        endCall(call.callSessionId)
        const call2 = receive()
        const again = route(call2, main, "invalid", "7")
        as(ADMIN, "store#IvrMenu", {ivrMenuId: main, invalidTypeEnumId: "CtgMenu", invalidId: sub})
        const invalid = route(call2, main, "invalid", "7")
        // an unknown option is invalid too, whatever the engine says
        const unknown = route(call2, sub, "option", "5")
        as(ADMIN, "store#IvrMenu", {ivrMenuId: main, invalidTypeEnumId: "", invalidId: ""})
        endCall(call2.callSessionId)
        return {timeout, again, invalid, unknown}
    },
    then({timeout, again, invalid, unknown}) {
        timeout.mode == "voicemail"
        again.mode == "menu"
        again.ivrMenuId == main
        invalid.mode == "menu"
        invalid.ivrMenuId == sub
        unknown.mode == "menu"
        unknown.ivrMenuId == sub
    }
})

feature("outside its schedule a menu goes to its closed target; with none the call ends; a loop of menus ends", {
    when() {
        const hours = as(ADMIN, "org.moqui.call.CallHandlingServices.store#CommsSchedule", {name: "Ivr spec hours", timeZone: "UTC", shared: true})
        for (const dayOfWeek of [1, 2, 3, 4, 5]) {
            as(ADMIN, "org.moqui.call.CallHandlingServices.store#CommsScheduleRule", {commsScheduleId: hours.commsScheduleId, ruleTypeEnumId: "CsrWeekly",
                dayOfWeek, fromTime: "09:00", thruTime: "17:00"})
        }
        as(ADMIN, "store#IvrMenu", {ivrMenuId: main, commsScheduleId: hours.commsScheduleId, closedTypeEnumId: "CtgVoicemail", closedId: boxId})
        // Saturday 2026-10-31 10:00 UTC, then Monday 2026-11-02 10:00 UTC
        const saturday = Date.UTC(2026, 9, 31, 10, 0)
        const monday = Date.UTC(2026, 10, 2, 10, 0)
        const closed = receive(saturday)
        const open = receive(monday)
        as(ADMIN, "store#IvrMenu", {ivrMenuId: main, closedTypeEnumId: "", closedId: ""})
        const closedNoTarget = receive(saturday)
        as(ADMIN, "store#IvrMenu", {ivrMenuId: main, commsScheduleId: ""})
        for (const c of [closed, open, closedNoTarget]) if (c.callSessionId) endCall(c.callSessionId)
        // a menu that sends a key to itself again and again: the call ends after 20 menus
        const looping = receive()
        let last = null
        for (let i = 0; i < 25 && (!last || last.mode == "menu"); i++) last = route(looping, sub, "option", "0")
        endCall(looping.callSessionId)
        return {closed, open, closedNoTarget, last}
    },
    then({closed, open, closedNoTarget, last}) {
        closed.ivr.mode == "voicemail"
        open.ivr.mode == "menu"
        closedNoTarget.ivr.mode == "end"
        last.mode == "end"
    }
})
