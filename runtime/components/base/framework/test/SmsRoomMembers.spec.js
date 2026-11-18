// Text members of a room (plans/Comms-Messaging.md §18.3 phase 18; SmsRoom.js, SmsRoomServices, the hooks in Sms.js and
// ChatServices): who adds one and where; one room per outside number on each of our numbers (noFreeNumber, and the add
// dialog's choices); the welcome checked in room mode and its count let go when the provider is not reached; a message
// makes one delivery a text member, an edit or a reaction none, and reactions stay in the room; a member's reply posts as
// the member and goes to the other members, not back; check#SmsSend's room mode and inRoomByText; the room's own name for
// a member; HELP once a day; STOP takes them out; taking one out ends the mapping; an archived room's texts go to the pair
// room. No provider is reached: deliveries are rows, and receive#Sms is called as the webhook would after its check.
// Run: ./moq test framework --spec SmsRoomMembers
const MOD = "SMS_ROOM_MOD"
const MATE = "SMS_ROOM_MATE"
const BARE = "SMS_ROOM_BARE" // a moderator of their own room with no number for room texts
const USERS = [MOD, MATE, BARE]
const BRIDGE = "SMS_ROOM_BRIDGE"
const OURS1 = "+15550008101"
const OURS2 = "+15550008102"
const THEIRS = "+15552228101"
const OTHER = "+15552228102"
const ROOMS = "org.moqui.call.SmsRoomServices."
const CHAT = "org.moqui.chat.ChatServices."

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
    ec.user.internalLoginUser(username(userId))
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
let seq = 0
function text(ours, theirs, body) {
    seq += 1
    ec.user.logoutUser()
    ec.context.sharedMap._chatBridgeWebhook = BRIDGE
    const out = sys(() => ec.service.sync().name("org.moqui.call.SmsServices.receive#Sms").parameters({chatBridgeId: BRIDGE, ourNumber: ours,
        theirNumber: theirs, text: body, messageId: "sms-room-" + seq, eventId: "sms-room-event-" + seq}).call()) || {}
    ec.message.clearErrors()
    delete ec.context.sharedMap._chatBridgeWebhook
    return out
}
const make = (entity, fields) => sys(() => ec.entity.makeValue(entity).setFields(fields, true, null, null).createOrUpdate())
const mappingOf = (ours, theirs, chatRoomId) => sys(() => ec.entity.find("moqui.chat.ChatRoomBridge").condition("chatBridgeId", BRIDGE)
    .condition("externalRoomId", ours + "/" + theirs + "#" + chatRoomId).orderBy("-fromDate").list()[0] || null)
const pairRoomOf = (ours, theirs) => sys(() => (ec.entity.find("moqui.chat.ChatRoomBridge").condition("chatBridgeId", BRIDGE)
    .condition("externalRoomId", ours + "/" + theirs).condition("thruDate", null).list()[0] || {}).chatRoomId || null)
const deliveries = (chatRoomId, chatRoomBridgeId) => sys(() => ec.entity.find("moqui.chat.ChatBridgeDelivery").condition({chatRoomId, chatRoomBridgeId}).count())
const lastEvent = (chatRoomId) => sys(() => ec.entity.find("moqui.chat.ChatEvent").condition("chatRoomId", chatRoomId).orderBy("-eventSeq").one())
const counted = (counterKey) => sys(() => Number((ec.entity.find("moqui.call.SmsSendCount").condition("counterKey", counterKey)
    .condition("dayDate", new Date(nowMs()).toISOString().slice(0, 10)).one() || {}).sentCount || 0))
function nowMs() {
    const number = Number(ec.user.nowTimestamp)
    return Number.isNaN(number) ? new Date(String(ec.user.nowTimestamp)).getTime() : number
}
const shared = {}

spec({
    stepwise: true,
    setupSpec() {
        for (const userId of USERS) make("moqui.security.UserAccount", {userId, username: username(userId), userFullName: "Rooms " + userId.slice(9), disabled: "N"})
        make("moqui.chat.ChatBridge", {chatBridgeId: BRIDGE, typeEnumId: "CbtSms", adapterName: "sms", name: "Room texts spec",
            configJson: JSON.stringify({apiKey: "spec", apiBaseUrl: "http://127.0.0.1:9", allowInternalAddresses: true})})
        for (const number of [OURS1, OURS2]) {
            make("moqui.call.PhoneNumber", {phoneNumber: number, messagingBridgeId: BRIDGE, routeTypeEnumId: "PnrUser", routeId: MOD, roomTextsEnabled: "Y"})
            make("moqui.call.PhoneNumberUser", {phoneNumber: number, userId: MOD, fromDate: new Date(Date.now() - 60000)})
        }
        ec.user.internalLoginUser(username(MOD))
        shared.room = as(MOD, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Soccer parents", memberUserIdList: [MATE]}).chatRoomId
        shared.second = as(MOD, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Choir"}).chatRoomId
        shared.third = as(MOD, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Book club"}).chatRoomId
        shared.fourth = as(MOD, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Garden"}).chatRoomId
        shared.pub = as(MOD, CHAT + "create#Room", {roomTypeEnumId: "CrtPublic", roomName: "Everyone"}).chatRoomId
        shared.sealed = as(MOD, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Sealed"}).chatRoomId
        sys(() => ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", shared.sealed).updateAll({encrypted: "Y"}))
        shared.bare = as(BARE, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Bare"}).chatRoomId
    },
    cleanupSpec() { ec.user.logoutUser() }
})

feature("a moderator of a private room that is not encrypted adds a number, with consent, on a number for room texts", {
    when() {
        const add = (userId, chatRoomId, extra) => as(userId, ROOMS + "add#TextMember", Object.assign({chatRoomId, phoneNumber: THEIRS, consent: "Y"}, extra || {}))
        return {
            notModerator: add(MATE, shared.room), publicRoom: add(MOD, shared.pub), encrypted: add(MOD, shared.sealed),
            noConsent: add(MOD, shared.room, {consent: "N"}), ours: add(MOD, shared.room, {phoneNumber: OURS2}),
            emergency: add(MOD, shared.room, {phoneNumber: "911"}), noNumber: add(BARE, shared.bare),
            added: add(MOD, shared.room, {memberName: "Bob"}), again: add(MOD, shared.room),
        }
    },
    then({notModerator, publicRoom, encrypted, noConsent, ours, emergency, noNumber, added, again}) {
        notModerator.reasonCode == "notAllowed"
        publicRoom.reasonCode == "roomType"
        encrypted.reasonCode == "encrypted"
        noConsent.reasonCode == "consentRequired"
        ours.reasonCode == "badNumber"
        emergency.reasonCode != null
        noNumber.reasonCode == "noRoomNumber"
        added.errors == ""
        added.ourNumber == OURS1
        added.welcomeSent == false
        again.reasonCode == "alreadyMember"
        // the welcome was checked in room mode and, not reaching the provider, let go of its count
        mappingOf(OURS1, THEIRS, shared.room).welcomeReasonCode != null
        counted("room:" + shared.room) == 0
        counted("roomAdds:" + MOD) == 1
        lastEvent(shared.room).contentJson.includes("\"via\":\"text\"")
    }
})

feature("disappearing messages and text members exclude each other, both ways (phase 16)", {
    when() {
        const timed = as(MOD, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Timed"}).chatRoomId
        const timer = as(MOD, "org.moqui.chat.ChatExpiryServices.set#RoomTimer", {chatRoomId: timed, seconds: 3600})
        const add = as(MOD, ROOMS + "add#TextMember", {chatRoomId: timed, phoneNumber: OTHER, consent: "Y"})
        const onTexted = as(MOD, "org.moqui.chat.ChatExpiryServices.set#RoomTimer", {chatRoomId: shared.room, seconds: 3600})
        return {timer, add, onTexted}
    },
    then({timer, add, onTexted}) {
        timer.errors == ""
        add.reasonCode == "timerOn"
        onTexted.reasonCode == "bridged"
    }
})

feature("one room per outside number on each of our numbers: the second room takes another number, a third none", {
    when() {
        const second = as(MOD, ROOMS + "add#TextMember", {chatRoomId: shared.second, phoneNumber: THEIRS, consent: "Y"})
        const choices = as(MOD, ROOMS + "get#TextMemberChoices", {chatRoomId: shared.third, phoneNumber: THEIRS})
        const third = as(MOD, ROOMS + "add#TextMember", {chatRoomId: shared.third, phoneNumber: THEIRS, consent: "Y"})
        return {second, choices, third}
    },
    then({second, choices, third}) {
        second.ourNumber == OURS2
        choices.numberList.length == 2
        choices.numberList.every((choice) => !choice.free)
        choices.numberList.find((choice) => choice.phoneNumber === OURS1).takenRoomName == "Soccer parents"
        third.reasonCode == "noFreeNumber"
        third.errors.includes("already a text member of Soccer parents on this number")
    }
})

feature("a message goes to each text member, an edit or a reaction to none, and reactions stay in the room", {
    when() {
        as(MOD, ROOMS + "add#TextMember", {chatRoomId: shared.room, phoneNumber: OTHER, consent: "Y", memberName: "Carol"})
        const sent = as(MOD, CHAT + "send#Message", {chatRoomId: shared.room, body: "<p>Practice at 6</p>"})
        const bob = mappingOf(OURS1, THEIRS, shared.room)
        const carol = mappingOf(OURS1, OTHER, shared.room)
        const toBob = deliveries(shared.room, bob.chatRoomBridgeId)
        as(MATE, CHAT + "react#Message", {chatRoomId: shared.room, eventSeq: sent.eventSeq, reactionKey: "👍"})
        const afterReaction = deliveries(shared.room, bob.chatRoomBridgeId)
        const bridges = sys(() => ec.service.sync().name(CHAT + "get#RoomBridges").parameter("chatRoomIdList", [shared.room]).call())
        const support = sys(() => ec.service.sync().name(CHAT + "check#BridgeSupport").parameters({chatRoomId: shared.room, eventSeq: sent.eventSeq, operation: "reaction"}).call())
        const room = as(MATE, CHAT + "get#Room", {chatRoomId: shared.room})
        return {toBob, carol, afterReaction, bridges, support, room}
    },
    then({toBob, carol, afterReaction, bridges, support, room}) {
        toBob == 1
        deliveries(shared.room, carol.chatRoomBridgeId) == 1
        afterReaction == 1
        bridges.textMemberMap[shared.room] == 2
        bridges.bridgeMap[shared.room] == null
        support.supported == true
        // the room's own name for them; the shared external user keeps the number
        room.memberList.find((person) => person.textMember && person.phoneNumber === THEIRS).name == "Bob"
        sys(() => ec.entity.find("moqui.chat.ChatExternalUser").condition({chatBridgeId: BRIDGE, externalUserId: THEIRS}).one().displayName) == THEIRS
    }
})

feature("a member's reply posts to the room as them and goes to the other members, not back", {
    when() {
        const reply = text(OURS1, THEIRS, "Running late")
        const bob = mappingOf(OURS1, THEIRS, shared.room)
        const carol = mappingOf(OURS1, OTHER, shared.room)
        const event = lastEvent(shared.room)
        return {reply, event, bob, carol}
    },
    then({reply, event, bob, carol}) {
        reply.chatRoomId == shared.room
        event.senderActorId == bob.memberChatActorId
        sys(() => ec.entity.find("moqui.chat.ChatBridgeDelivery").condition({chatRoomId: shared.room, eventSeq: event.eventSeq, chatRoomBridgeId: carol.chatRoomBridgeId}).count()) == 1
        sys(() => ec.entity.find("moqui.chat.ChatBridgeDelivery").condition({chatRoomId: shared.room, eventSeq: event.eventSeq, chatRoomBridgeId: bob.chatRoomBridgeId}).count()) == 0
        pairRoomOf(OURS1, THEIRS) == null
    }
})

feature("check#SmsSend in room mode needs no grant and a number for room texts, and the pair room refuses while they are a member", {
    when() {
        const check = (parameters) => sys(() => ec.service.sync().name("org.moqui.call.SmsServices.check#SmsSend").parameters(parameters).call())
        const room = check({fromNumber: OURS1, toNumber: THEIRS, chatRoomId: shared.room})
        const pair = check({fromNumber: OURS1, toNumber: THEIRS, userId: MOD, inRoom: true})
        const elsewhere = check({fromNumber: OURS1, toNumber: "+15552228199", userId: MOD, inRoom: true})
        sys(() => ec.entity.find("moqui.call.PhoneNumber").condition("phoneNumber", OURS1).updateAll({roomTextsEnabled: "N"}))
        const off = check({fromNumber: OURS1, toNumber: THEIRS, chatRoomId: shared.room})
        sys(() => ec.entity.find("moqui.call.PhoneNumber").condition("phoneNumber", OURS1).updateAll({roomTextsEnabled: "Y"}))
        return {room, pair, elsewhere, off}
    },
    then({room, pair, elsewhere, off}) {
        room.allowed == true
        pair.allowed == false
        pair.reasonCode == "inRoomByText"
        pair.message.includes("Soccer parents")
        elsewhere.allowed == true
        off.reasonCode == "roomTextsOff"
    }
})

feature("HELP is answered once a day; STOP takes the member out, and their next text goes to the pair room", {
    when() {
        text(OURS1, THEIRS, "HELP")
        const helped = mappingOf(OURS1, THEIRS, shared.room).helpReplyDate
        text(OURS1, THEIRS, "HELP")
        const helpedAgain = mappingOf(OURS1, THEIRS, shared.room).helpReplyDate
        const stop = text(OURS1, THEIRS, "STOP")
        const ended = mappingOf(OURS1, THEIRS, shared.room)
        const member = sys(() => ec.entity.find("moqui.chat.ChatRoomMember").condition({chatRoomId: shared.room, chatActorId: ended.memberChatActorId}).one())
        const event = lastEvent(shared.room)
        const optOut = sys(() => ec.entity.find("moqui.call.SmsOptOut").condition({phoneNumber: OURS1, otherNumber: THEIRS}).one())
        text(OURS1, THEIRS, "START")
        const after = text(OURS1, THEIRS, "Am I still in?")
        return {helped, helpedAgain, stop, ended, member, event, optOut, after}
    },
    then({helped, helpedAgain, stop, ended, member, event, optOut, after}) {
        helped != null
        String(helpedAgain) == String(helped)
        stop.outcome == "noted"
        ended.thruDate != null
        member.leftDate != null
        event.contentJson.includes("\"via\":\"stop\"")
        optOut.optedOut == "Y"
        after.chatRoomId != shared.room
        after.chatRoomId == pairRoomOf(OURS1, THEIRS)
    }
})

feature("taking a member out ends its mapping, and an archived room's texts go to the pair room", {
    when() {
        const carol = mappingOf(OURS1, OTHER, shared.room)
        const removed = as(MOD, CHAT + "remove#Member", {chatRoomId: shared.room, chatActorId: carol.memberChatActorId})
        const ended = mappingOf(OURS1, OTHER, shared.room).thruDate
        sys(() => ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", shared.second).updateAll({archivedDate: ec.user.nowTimestamp}))
        const archived = text(OURS2, THEIRS, "Anyone there?")
        return {removed, ended, archived}
    },
    then({removed, ended, archived}) {
        removed.errors == ""
        ended != null
        archived.chatRoomId != shared.second
        archived.chatRoomId == pairRoomOf(OURS2, THEIRS)
    }
})

const policy = (fields) => make("moqui.call.VoicePolicy", Object.assign({voicePolicyId: "DEFAULT"}, fields))
const NEXT = "+15552228103"
const check = (parameters) => sys(() => ec.service.sync().name("org.moqui.call.SmsServices.check#SmsSend").parameters(parameters).call())

feature("an archived room's member still takes the number, and two mappings of one person route to the pair room", {
    when() {
        const choices = as(MOD, ROOMS + "get#TextMemberChoices", {chatRoomId: shared.third, phoneNumber: THEIRS})
        const third = as(MOD, ROOMS + "add#TextMember", {chatRoomId: shared.third, phoneNumber: THEIRS, consent: "Y", ourNumber: OURS2})
        // a second mapping of THEIRS on OURS2, as no add makes one: the text goes to neither room
        const second = mappingOf(OURS2, THEIRS, shared.second)
        sys(() => ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", shared.second).updateAll({archivedDate: null}))
        make("moqui.chat.ChatRoomBridge", {chatRoomBridgeId: "SMS_ROOM_DUP", chatRoomId: shared.third, chatBridgeId: BRIDGE,
            externalRoomId: OURS2 + "/" + THEIRS + "#" + shared.third, directionEnumId: "CrbBoth", fromDate: ec.user.nowTimestamp,
            memberChatActorId: second.memberChatActorId})
        const routed = text(OURS2, THEIRS, "Which room?")
        sys(() => ec.entity.find("moqui.chat.ChatRoomBridge").condition("chatRoomBridgeId", "SMS_ROOM_DUP").deleteAll())
        return {choices, third, routed}
    },
    then({choices, third, routed}) {
        choices.numberList.find((choice) => choice.phoneNumber === OURS2).takenArchived == true
        third.reasonCode == "noFreeNumber"
        third.errors.includes("which is archived")
        routed.chatRoomId == pairRoomOf(OURS2, THEIRS)
    }
})

feature("the adder's own permissions and day: an international number refused, room texts counted on them, stopped when they lose the number", {
    when() {
        policy({internationalEnabled: "Y"})
        const international = as(MOD, ROOMS + "add#TextMember", {chatRoomId: shared.third, phoneNumber: "+447700900123", consent: "Y"})
        policy({internationalEnabled: null})
        as(MOD, ROOMS + "add#TextMember", {chatRoomId: shared.third, phoneNumber: NEXT, consent: "Y"})
        const before = counted("user:" + MOD)
        const sent = check({fromNumber: OURS1, toNumber: NEXT, chatRoomId: shared.third, reserve: true})
        const after = counted("user:" + MOD)
        policy({smsMaxPerUserPerDay: after})
        const overDay = check({fromNumber: OURS1, toNumber: NEXT, chatRoomId: shared.third})
        policy({smsMaxPerUserPerDay: null})
        sys(() => ec.entity.find("moqui.call.PhoneNumberUser").condition({phoneNumber: OURS1, userId: MOD}).deleteAll())
        const noGrant = check({fromNumber: OURS1, toNumber: NEXT, chatRoomId: shared.third})
        const told = sys(() => ec.entity.find("moqui.security.user.NotificationMessage").condition("topic", "ChatTextMember")
            .condition("recordId", shared.third).count())
        make("moqui.call.PhoneNumberUser", {phoneNumber: OURS1, userId: MOD, fromDate: new Date(Date.now() - 60000)})
        return {international, sent, before, after, overDay, noGrant, told}
    },
    then({international, sent, before, after, overDay, noGrant, told}) {
        international.reasonCode == "internationalNotEnabled"
        sent.allowed == true
        sent.counterUserId == MOD
        after == before + 1
        overDay.reasonCode == "dailyLimit"
        noGrant.reasonCode == "adderNotAllowed"
        told == 1
    }
})

feature("an add's other refusals: a room open to every user, the room's members, the day's adds, a blocked number, one that replied STOP", {
    when() {
        const add = (chatRoomId, phoneNumber) => as(MOD, ROOMS + "add#TextMember", {chatRoomId, phoneNumber, consent: "Y"})
        const open = as(MOD, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Open"}).chatRoomId
        sys(() => ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", open).updateAll({visibilityEnumId: "CrvUsers"}))
        const openRoom = add(open, "+15552228111")
        policy({smsRoomMaxMembers: 1})
        const tooMany = add(shared.third, "+15552228112")
        policy({smsRoomMaxMembers: null, smsRoomAddsPerDay: counted("roomAdds:" + MOD)})
        const adds = add(shared.fourth, "+15552228113")
        policy({smsRoomAddsPerDay: null})
        as(MOD, "org.moqui.call.CallHandlingServices.block#PhoneNumber", {phoneNumber: "+15552228114"})
        const blocked = add(shared.fourth, "+15552228114")
        make("moqui.call.SmsOptOut", {phoneNumber: OURS1, otherNumber: "+15552228115", optedOut: "Y", changedDate: ec.user.nowTimestamp})
        make("moqui.call.SmsOptOut", {phoneNumber: OURS2, otherNumber: "+15552228115", optedOut: "Y", changedDate: ec.user.nowTimestamp})
        const optedOut = add(shared.fourth, "+15552228115")
        return {openRoom, tooMany, adds, blocked, optedOut}
    },
    then({openRoom, tooMany, adds, blocked, optedOut}) {
        openRoom.reasonCode == "openRoom"
        tooMany.reasonCode == "tooManyMembers"
        adds.reasonCode == "dailyLimit"
        blocked.reasonCode == "blocked"
        optedOut.reasonCode == "optedOut"
    }
})
