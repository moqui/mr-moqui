// Event messages with RSVP (plans/Comms-Messaging.md §18.3 phase 19; ChatCal.js, ChatCalServices): an event is a
// message whose body is the event in words (what bridges and texts get), with its rows and the members' answers; a
// reminder an answer asks for (phase 15's), moved with a change and cancelled with No or a cancellation; changes and a
// cancellation posted as messages; the .ics file. The clock is pinned with ec.user.setEffectiveTime.
// Run: ./moq test framework --spec ChatCal
const A = "CHAT_CAL_A"
const B = "CHAT_CAL_B"
const C = "CHAT_CAL_C"
const USERS = [A, B, C]
const CHAT = "org.moqui.chat.ChatServices."
const CAL = "org.moqui.chat.ChatCalServices."
const GROUP = "CHAT_CAL_SPEC"
const MINUTE = 60 * 1000
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR
// the spec's clock, and an event on Saturday 23 November 2030 at 14:00 to 16:00 in Denver (UTC-7 then)
const T0 = Date.UTC(2030, 10, 1, 12, 0)
const START = Date.UTC(2030, 10, 23, 21, 0)
const END = START + 2 * HOUR
const ZONE = "America/Denver"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
let clock = T0
function as(userId, name, parameters) {
    if (userId) ec.user.internalLoginUser(username(userId))
    ec.user.setEffectiveTime(clock)
    let out = {}
    let thrown = ""
    try {
        out = ec.service.sync().name(name).parameters(parameters || {}).call() || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const make = (entity, fields) => sys(() => ec.entity.makeValue(entity).setFields(fields, true, null, null).createOrUpdate())
const row = (entity, fields) => sys(() => ec.entity.find(entity).condition(fields).one())
const rows = (entity, fields) => sys(() => Array.from(ec.entity.find(entity).condition(fields).list()))
const room = (userId, fields) => as(userId, CHAT + "create#Room", Object.assign({roomTypeEnumId: "CrtPrivate"}, fields)).chatRoomId
const timeline = (userId, chatRoomId) => Array.from(as(userId, CHAT + "get#Timeline", {chatRoomId}).eventList || [])
const reminder = (userId, eventSeq) => rows("moqui.chat.ChatMessageReminder", {userId, eventSeq}).find((one) => one.statusEnumId === "CrmPending") || null
const event = (fields) => Object.assign({title: "Picnic", startDate: START, endDate: END, timeZone: ZONE, place: "Liberty Park", description: "Bring a chair"}, fields)
const shared = {}

function makeUsers() {
    sys(() => {
        for (const userId of USERS) {
            if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
                ec.entity.makeValue("moqui.security.UserAccount").setFields({userId, username: username(userId), userFullName: "Cal " + userId.slice(9),
                    disabled: "N"}, true, null, null).create()
            }
        }
    })
    make("moqui.security.UserGroup", {userGroupId: GROUP, description: "Chat events spec"})
    make("moqui.security.ArtifactGroup", {artifactGroupId: GROUP, description: "Chat events spec"})
    make("moqui.security.ArtifactGroupMember", {artifactGroupId: GROUP, artifactName: "org\\.moqui\\.chat\\..*", artifactTypeEnumId: "AT_SERVICE",
        nameIsPattern: "Y", inheritAuthz: "Y"})
    make("moqui.security.ArtifactAuthz", {artifactAuthzId: GROUP, userGroupId: GROUP, artifactGroupId: GROUP, authzTypeEnumId: "AUTHZT_ALWAYS",
        authzActionEnumId: "AUTHZA_ALL"})
    // from the real clock: a grant is read at the time of the check, not the spec's pinned one
    for (const userId of USERS) make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId, fromDate: new Date(Date.now() - DAY)})
}
function cleanUp() {
    sys(() => {
        ec.entity.find("moqui.chat.ChatMessageReminder").condition("userId", "in", USERS).deleteAll()
        ec.entity.find("moqui.chat.ChatCalAnswer").condition("userId", "in", USERS).deleteAll()
        ec.entity.find("moqui.chat.ChatCalEvent").condition("createdByUserId", "in", USERS).deleteAll()
    })
}

spec({
    stepwise: true,
    setupSpec() {
        makeUsers()
        cleanUp()
        shared.team = room(A, {roomName: "Cal team", memberUserIdList: [B]})
    },
    cleanupSpec() {
        cleanUp()
        ec.user.setEffectiveTime(null)
        ec.user.logoutUser()
    }
})

feature("create#CalEvent refuses an encrypted room, a stranger, bad times and zones and long text; its body is the event in its zone", {
    when() {
        const sealed = room(A, {roomName: "Cal sealed", memberUserIdList: [B], encrypted: true})
        const refused = {
            encrypted: as(A, CAL + "create#CalEvent", Object.assign({chatRoomId: sealed}, event())).reasonCode,
            stranger: as(C, CAL + "create#CalEvent", Object.assign({chatRoomId: shared.team}, event())).reasonCode,
            backwards: as(A, CAL + "create#CalEvent", Object.assign({chatRoomId: shared.team}, event({endDate: START - HOUR}))).reasonCode,
            over: as(A, CAL + "create#CalEvent", Object.assign({chatRoomId: shared.team}, event({startDate: T0 - 3 * HOUR, endDate: T0 - HOUR}))).reasonCode,
            tooLongEvent: as(A, CAL + "create#CalEvent", Object.assign({chatRoomId: shared.team}, event({endDate: START + 40 * DAY}))).reasonCode,
            farOff: as(A, CAL + "create#CalEvent", Object.assign({chatRoomId: shared.team}, event({startDate: T0 + 800 * DAY, endDate: T0 + 801 * DAY}))).reasonCode,
            zone: as(A, CAL + "create#CalEvent", Object.assign({chatRoomId: shared.team}, event({timeZone: "Mars/Olympus"}))).reasonCode,
            title: as(A, CAL + "create#CalEvent", Object.assign({chatRoomId: shared.team}, event({title: "x".repeat(201)}))).reasonCode,
            allDayAt2pm: as(A, CAL + "create#CalEvent", Object.assign({chatRoomId: shared.team}, event({allDay: true}))).reasonCode,
        }
        const made = as(A, CAL + "create#CalEvent", Object.assign({chatRoomId: shared.team}, event()))
        shared.seq = made.eventSeq
        const message = row("moqui.chat.ChatEvent", {chatRoomId: shared.team, eventSeq: made.eventSeq})
        // a whole day: midnight to midnight in Denver (07:00 UTC)
        const day = as(A, CAL + "create#CalEvent", Object.assign({chatRoomId: shared.team}, event({title: "Fair", allDay: true,
            startDate: Date.UTC(2030, 10, 24, 7), endDate: Date.UTC(2030, 10, 25, 7), place: null, description: null})))
        const dayText = row("moqui.chat.ChatEvent", {chatRoomId: shared.team, eventSeq: day.eventSeq}).bodyText
        return {refused, made, bodyText: message.bodyText, content: JSON.parse(message.contentJson), dayText}
    },
    then({refused, made, bodyText, content, dayText}) {
        refused.encrypted == "encryptedRoom"
        refused.stranger == "notFound"
        refused.backwards == "badTime"
        refused.over == "badTime"
        refused.tooLongEvent == "badTime"
        refused.farOff == "badTime"
        refused.zone == "badTime"
        refused.title == "tooLong"
        refused.allDayAt2pm == "badTime"
        made.errors == ""
        made.event.calEvent.title == "Picnic"
        made.event.calEvent.going == 0
        content.calEvent.title == "Picnic"
        bodyText == "Event: Picnic\nSat 23 Nov 2030, 14:00–16:00 (America/Denver)\nWhere: Liberty Park\nBring a chair"
        dayText == "Event: Fair\nSun 24 Nov 2030, all day"
    }
})

feature("answers: counts, a changed answer, a take-back, the timeline's own answer; a reminder an hour before; a stranger refused", {
    when() {
        const going = as(B, CAL + "answer#CalEvent", {chatRoomId: shared.team, eventSeq: shared.seq, answer: "going", remindMinutes: 60})
        const remind = reminder(B, shared.seq)
        const maybe = as(A, CAL + "answer#CalEvent", {chatRoomId: shared.team, eventSeq: shared.seq, answer: "maybe"})
        const stranger = as(C, CAL + "answer#CalEvent", {chatRoomId: shared.team, eventSeq: shared.seq, answer: "going"})
        const bad = as(B, CAL + "answer#CalEvent", {chatRoomId: shared.team, eventSeq: shared.seq, answer: "perhaps"})
        const mine = timeline(B, shared.team).find((one) => one.eventSeq === shared.seq).calEvent
        // A changes to going, then takes it back
        as(A, CAL + "answer#CalEvent", {chatRoomId: shared.team, eventSeq: shared.seq, answer: "going"})
        const both = row("moqui.chat.ChatCalEvent", {chatRoomId: shared.team, eventSeq: shared.seq})
        const back = as(A, CAL + "answer#CalEvent", {chatRoomId: shared.team, eventSeq: shared.seq, answer: ""})
        return {going, remindAt: remind ? Number(remind.remindDate) : null, maybe, stranger, bad, mine, bothGoing: Number(both.goingCount),
            bothMaybe: Number(both.maybeCount), back: back.calEvent}
    },
    then({going, remindAt, maybe, stranger, bad, mine, bothGoing, bothMaybe, back}) {
        going.errors == ""
        going.calEvent.mine.answer == "going"
        going.calEvent.going == 1
        going.calendarFailed == false
        remindAt == START - HOUR
        maybe.calEvent.maybe == 1
        stranger.reasonCode == "notFound"
        bad.reasonCode == "badAnswer"
        mine.mine.answer == "going"
        mine.mine.remindMinutes == 60
        mine.answers.going[0] == B
        mine.answers.maybe[0] == A
        bothGoing == 2
        bothMaybe == 0
        back.mine == null
        back.going == 1
    }
})

feature("a change: only by its creator or a moderator, a reply message, revision + 1, the reminder moved; a message edit refused", {
    when() {
        const notCreator = as(B, CAL + "update#CalEvent", {chatRoomId: shared.team, eventSeq: shared.seq, title: "Not mine"})
        const changed = as(A, CAL + "update#CalEvent", {chatRoomId: shared.team, eventSeq: shared.seq, startDate: START + DAY, endDate: END + DAY})
        const posted = row("moqui.chat.ChatEvent", {chatRoomId: shared.team, eventSeq: changed.changeEventSeq})
        const remind = reminder(B, shared.seq)
        const edit = as(A, CHAT + "edit#Message", {chatRoomId: shared.team, eventSeq: shared.seq, body: "<p>edited</p>"})
        const same = as(A, CAL + "update#CalEvent", {chatRoomId: shared.team, eventSeq: shared.seq, title: "Picnic"})
        return {notCreator, changed, posted, content: JSON.parse(posted.contentJson), remindAt: remind ? Number(remind.remindDate) : null, edit, same}
    },
    then({notCreator, changed, posted, content, remindAt, edit, same}) {
        notCreator.reasonCode == "notAllowed"
        changed.errors == ""
        changed.calEvent.revision == 1
        changed.calEvent.start == START + DAY
        Number(posted.replyToEventSeq) == shared.seq
        posted.bodyText == "Changed: Picnic\nNow Sun 24 Nov 2030, 14:00–16:00 (America/Denver)"
        content.calChange.what[0] == "when"
        remindAt == START + DAY - HOUR
        edit.errors.includes("changed with Change")
        same.changeEventSeq == null
    }
})

feature("the .ics file: PUBLISH with its UID, SEQUENCE and zone; refused to one who cannot read the room", {
    when() {
        const file = as(B, CAL + "get#CalEventIcs", {chatRoomId: shared.team, eventSeq: shared.seq, baseUrl: "https://chat.example.com"})
        const stranger = as(C, CAL + "get#CalEventIcs", {chatRoomId: shared.team, eventSeq: shared.seq})
        return {file, stranger}
    },
    then({file, stranger}) {
        file.contentType == "text/calendar"
        file.fileName == "Picnic.ics"
        file.fileText.includes("METHOD:PUBLISH")
        file.fileText.includes("UID:chat-" + shared.team + "-" + shared.seq + "@")
        file.fileText.includes("SEQUENCE:1")
        file.fileText.includes("TZID=America/Denver")
        file.fileText.includes("LOCATION:Liberty Park")
        stranger.reasonCode == "notFound"
    }
})

feature("a cancellation: a message, the reminders cancelled, no more answers, a CANCEL file", {
    when() {
        const notCreator = as(B, CAL + "cancel#CalEvent", {chatRoomId: shared.team, eventSeq: shared.seq})
        const cancelled = as(A, CAL + "cancel#CalEvent", {chatRoomId: shared.team, eventSeq: shared.seq})
        const posted = row("moqui.chat.ChatEvent", {chatRoomId: shared.team, eventSeq: cancelled.changeEventSeq})
        const late = as(B, CAL + "answer#CalEvent", {chatRoomId: shared.team, eventSeq: shared.seq, answer: "no"})
        const file = as(B, CAL + "get#CalEventIcs", {chatRoomId: shared.team, eventSeq: shared.seq})
        return {notCreator, cancelled, bodyText: posted.bodyText, remind: reminder(B, shared.seq), late, file}
    },
    then({notCreator, cancelled, bodyText, remind, late, file}) {
        notCreator.reasonCode == "notAllowed"
        cancelled.calEvent.cancelled == true
        bodyText == "Cancelled: Picnic, Sun 24 Nov 2030"
        remind == null
        late.reasonCode == "cancelled"
        file.fileText.includes("METHOD:CANCEL")
    }
})

feature("deleting an event's message removes its rows and the reminders its answers made; a bridged room sends its text", {
    when() {
        const made = as(A, CAL + "create#CalEvent", Object.assign({chatRoomId: shared.team}, event({title: "Movie"})))
        as(B, CAL + "answer#CalEvent", {chatRoomId: shared.team, eventSeq: made.eventSeq, answer: "going", remindMinutes: 15})
        const before = reminder(B, made.eventSeq) != null
        as(A, CHAT + "redact#Message", {chatRoomId: shared.team, eventSeq: made.eventSeq})
        const left = rows("moqui.chat.ChatCalAnswer", {chatRoomId: shared.team, eventSeq: made.eventSeq}).length
            + rows("moqui.chat.ChatCalEvent", {chatRoomId: shared.team, eventSeq: made.eventSeq}).length
        const bridged = room(A, {roomName: "Cal bridged"})
        make("moqui.chat.ChatBridge", {chatBridgeId: "CHAT_CAL_BRIDGE", name: "Spec bridge", adapterName: "none"})
        make("moqui.chat.ChatRoomBridge", {chatRoomBridgeId: "CHAT_CAL_MAP", chatRoomId: bridged, chatBridgeId: "CHAT_CAL_BRIDGE",
            externalRoomId: "+15550100", directionEnumId: "CrbOut", fromDate: new Date(T0)})
        const out = as(A, CAL + "create#CalEvent", Object.assign({chatRoomId: bridged}, event({title: "Call"})))
        const delivered = sys(() => ec.entity.find("moqui.chat.ChatBridgeDelivery").condition({chatRoomId: bridged, eventSeq: out.eventSeq}).count())
        sys(() => {
            ec.entity.find("moqui.chat.ChatBridgeDelivery").condition("chatRoomId", bridged).deleteAll()
            ec.entity.find("moqui.chat.ChatRoomBridge").condition("chatRoomBridgeId", "CHAT_CAL_MAP").deleteAll()
        })
        return {before, after: reminder(B, made.eventSeq), left, delivered}
    },
    then({before, after, left, delivered}) {
        before == true
        after == null
        left == 0
        delivered == 1
    }
})
