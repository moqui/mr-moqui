// Blocked numbers and the spam filter of texts (plans/Comms-Messaging.md §18.3 phase 11; SmsSpam.js, SmsSpamServices,
// CallHandling.js blockNumber, ChatBlocks.js): the rules and the threshold; a spam text kept out of sight for each
// member whose filter is on (not listed, counting and notifying nothing) while its STOP still applies; a known number,
// a trusted number and a room we replied in never judged; Not spam back for everyone, Report spam blocking for the
// reporter and Spam for a teammate; a blocked number's text out of the blocker's sight only; a group text not filtered
// by one number's block; the lookup's gate, cache and limit a day; and the spam purge. Our number reaches two users: its
// owner and a teammate granted it. No provider is reached: receive#Sms is called as the webhook would after its check.
// Run: ./moq test framework --spec SmsSpam
const OWNER = "SMS_SPAM_OWNER"
const MATE = "SMS_SPAM_MATE"
const USERS = [OWNER, MATE]
const BRIDGE = "SMS_SPAM_BRIDGE"
const OURS = "+15550007100"
const N = (n) => "+1555017" + String(1000 + n).slice(1)
const NUMBERS = Array.from({length: 20}, (_, i) => N(i + 1))
const SPAM = "component://framework/service/org/moqui/call/SmsSpam.js"

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
function text(theirs, body, extra) {
    seq += 1
    ec.user.logoutUser()
    ec.context.sharedMap._chatBridgeWebhook = BRIDGE
    const out = sys(() => ec.service.sync().name("org.moqui.call.SmsServices.receive#Sms").parameters(Object.assign({chatBridgeId: BRIDGE,
        ourNumber: OURS, theirNumber: theirs, text: body, messageId: "sms-spam-" + seq, eventId: "sms-spam-event-" + seq}, extra || {})).call()) || {}
    ec.message.clearErrors()
    delete ec.context.sharedMap._chatBridgeWebhook
    return out
}
const roomOf = (key) => sys(() => (Array.from(ec.entity.find("moqui.chat.ChatRoomBridge").condition("chatBridgeId", BRIDGE)
    .condition("externalRoomId", OURS + "/" + key).condition("thruDate", null).list())[0] || {}).chatRoomId || null)
const filterOf = (chatRoomId, userId) => sys(() => (ec.entity.find("moqui.chat.ChatRoomMember").condition({chatRoomId, userId}).one() || {}).filterEnumId || null)
const listed = (userId, filter) => as(userId, "org.moqui.chat.ChatServices.find#Rooms", filter ? {filter} : {})
const ids = (out) => Array.from(out.roomList || []).map((room) => room.chatRoomId)
const rule = (smsSpamRuleId) => sys(() => ec.entity.find("moqui.call.SmsSpamRule").condition("smsSpamRuleId", smsSpamRuleId).one()) || {}
const score = (body, theirs, firstContact) => ec.resource.script(SPAM).execute("score", {text: body, theirNumber: theirs || N(20), firstContact: !!firstContact})
function policy(fields) {
    sys(() => {
        let row = ec.entity.find("moqui.call.VoicePolicy").condition("voicePolicyId", "DEFAULT").one()
        if (!row) {
            row = ec.entity.makeValue("moqui.call.VoicePolicy")
            row.set("voicePolicyId", "DEFAULT")
            row.create()
        }
        row.setFields(fields, true, null, null)
        row.update()
    })
}
const POLICY_FIELDS = {smsSpamFilter: null, smsSpamThreshold: null, smsSpamLookup: null, smsSpamLookupDays: null, smsSpamLookupMaxPerDay: null}
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "Spam " + userId.slice(9), disabled: "N"}, true, null, null)
            account.create()
        }
    })
}
function clean() {
    sys(() => {
        for (const mapping of ec.entity.find("moqui.chat.ChatRoomBridge").condition("chatBridgeId", BRIDGE).list()) {
            for (const entity of ["moqui.chat.ChatEventLink", "moqui.chat.ChatEventExternal", "moqui.chat.ChatEventMention", "moqui.chat.ChatUserActivity",
                "moqui.chat.ChatRoomChange", "moqui.chat.ChatBridgeDelivery", "moqui.chat.ChatPushJob", "moqui.chat.ChatAttachment", "moqui.chat.ChatEvent",
                "moqui.chat.ChatRoomMember"])
                ec.entity.find(entity).condition("chatRoomId", mapping.chatRoomId).deleteAll()
            mapping.delete()
            ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", mapping.chatRoomId).deleteAll()
        }
        ec.entity.find("moqui.chat.ChatBridgeInbound").condition("chatBridgeId", BRIDGE).deleteAll()
        for (const list of ec.entity.find("moqui.call.CallerList").condition("userId", "in", USERS).list()) {
            ec.entity.find("moqui.call.CallerListEntry").condition("callerListId", list.callerListId).deleteAll()
            list.delete()
        }
        ec.entity.find("moqui.call.CallHandlingSetting").condition("userId", "in", USERS).deleteAll()
        for (const entity of ["moqui.call.SmsSpamVote", "moqui.call.SmsSpamNumber"]) ec.entity.find(entity).condition("phoneNumber", "in", NUMBERS).deleteAll()
        ec.entity.find("moqui.call.SmsOptOut").condition("phoneNumber", OURS).deleteAll()
        ec.entity.find("moqui.call.SmsSendCount").condition("counterKey", "lookup").deleteAll()
        ec.entity.find("moqui.call.PhoneNumberUser").condition("phoneNumber", OURS).deleteAll()
        ec.entity.find("moqui.call.PhoneNumber").condition("phoneNumber", OURS).deleteAll()
    })
}

spec({
    stepwise: true,
    setupSpec() {
        USERS.forEach(makeUser)
        clean()
        policy(POLICY_FIELDS)
        sys(() => {
            const bridge = ec.entity.makeValue("moqui.chat.ChatBridge")
            bridge.setFields({chatBridgeId: BRIDGE, typeEnumId: "CbtSms", adapterName: "sms", name: "Spam spec texts",
                configJson: JSON.stringify({apiKey: "spec", apiBaseUrl: "http://127.0.0.1:9"})}, true, null, null)
            bridge.createOrUpdate()
            const number = ec.entity.makeValue("moqui.call.PhoneNumber")
            number.setFields({phoneNumber: OURS, messagingBridgeId: BRIDGE, routeTypeEnumId: "PnrUser", routeId: OWNER}, true, null, null)
            number.createOrUpdate()
            const grant = ec.entity.makeValue("moqui.call.PhoneNumberUser")
            grant.setFields({phoneNumber: OURS, userId: MATE, fromDate: new Date(Date.now() - 60000)}, true, null, null)
            grant.create()
        })
    },
    cleanupSpec() {
        clean()
        policy(POLICY_FIELDS)
        ec.user.logoutUser()
    }
})

feature("a text's score is the weights of the rules it matches", {
    when() {
        return {
            shortened: score("Claim it now: https://bit.ly/abc", null, true),
            linkAlone: score("See example.com.", null, true),
            words: score("You have WON a gift card", null, true),
            later: score("Claim it now: https://bit.ly/abc", null, false),
            plain: score("Running late, see you at 6", null, true),
            dotted: score("Mr.Smith said the 3.5 version", null, false)
        }
    },
    then({shortened, linkAlone, words, later, plain, dotted}) {
        shortened.score == 110
        shortened.reasons.includes("SSR_HOST_BIT_LY")
        shortened.reasons.includes("SSR_FIRST_CONTACT")
        linkAlone.score == 70
        words.score == 120
        later.score == 70
        plain.score == 40
        !dotted.reasons.includes("SSR_HOST_BIT_LY")
    }
})

feature("a spam text is kept out of every member's sight, counts nothing, and its STOP still applies", {
    when() {
        const hostHits = Number(rule("SSR_HOST_BIT_LY").hitCount || 0)
        const first = text(N(1), "Your package could not be delivered: https://bit.ly/pkg")
        const room = roomOf(N(1))
        const again = sys(() => ec.service.sync().name("org.moqui.call.SmsServices.receive#Sms").parameters({chatBridgeId: BRIDGE, ourNumber: OURS,
            theirNumber: N(1), text: "x", messageId: "sms-spam-" + seq, eventId: "sms-spam-event-" + seq}).call()) || {}
        ec.message.clearErrors()
        const stop = text(N(1), "STOP")
        const optOut = sys(() => ec.entity.find("moqui.call.SmsOptOut").condition({phoneNumber: OURS, otherNumber: N(1)}).one())
        const rooms = listed(OWNER)
        const spam = listed(OWNER, "spam")
        const counts = as(OWNER, "org.moqui.chat.ChatServices.get#UnreadCounts")
        const events = sys(() => ec.entity.find("moqui.chat.ChatEvent").condition("chatRoomId", room).condition("eventTypeEnumId", "CetMessage").count())
        const kept = sys(() => ec.entity.find("moqui.call.SmsSpamNumber").condition("phoneNumber", N(1)).one())
        const info = as(MATE, "org.moqui.call.SmsSpamServices.get#SpamInfo", {chatRoomId: room})
        return {hostHits, first, room, again, stop, optOut, rooms, spam, counts, events, kept, info, hostAfter: Number(rule("SSR_HOST_BIT_LY").hitCount || 0)}
    },
    then({hostHits, first, room, stop, optOut, rooms, spam, counts, events, kept, info, hostAfter}) {
        first.outcome == "accepted"
        room != null
        filterOf(room, OWNER) == "CmfSpam"
        filterOf(room, MATE) == "CmfSpam"
        stop.outcome == "accepted"
        optOut.optedOut == "Y"
        !ids(rooms).includes(room)
        rooms.filteredCount >= 1
        ids(spam).includes(room)
        spam.roomList.find((item) => item.chatRoomId == room).filtered == "spam"
        counts.levelMap[room] == "none"
        counts.roomCounts[room] == null
        events == 2
        kept.lastScore == 150
        info.filtered == "spam"
        info.reasons.length >= 3
        hostAfter == hostHits + 1
    }
})

feature("a known number, a trusted number and a room we replied in are not judged", {
    when() {
        const list = as(OWNER, "org.moqui.call.CallHandlingServices.store#CallerList", {name: "Friends"})
        as(OWNER, "org.moqui.call.CallHandlingServices.add#CallerListEntry", {callerListId: list.callerListId, phoneNumber: N(2)})
        text(N(2), "Look: https://bit.ly/fun")
        sys(() => {
            const vote = ec.entity.makeValue("moqui.call.SmsSpamVote")
            vote.setFields({phoneNumber: N(3), userId: MATE, voteEnumId: "SsvNotSpam", voteDate: ec.user.nowTimestamp}, true, null, null)
            vote.create()
        })
        text(N(3), "Look: https://bit.ly/fun")
        // a plain first text, a reply of ours, then a spammy one
        text(N(4), "Hi, is this the shop?")
        const replied = roomOf(N(4))
        as(OWNER, "org.moqui.chat.ChatServices.send#Message", {chatRoomId: replied, body: "<p>Yes</p>"})
        text(N(4), "Pay here: https://bit.ly/pay gift card")
        return {known: roomOf(N(2)), trusted: roomOf(N(3)), replied}
    },
    then({known, trusted, replied}) {
        filterOf(known, OWNER) == null
        filterOf(trusted, MATE) == null
        filterOf(replied, OWNER) == null
        filterOf(replied, MATE) == null
    }
})

feature("Not spam brings the room back for everyone and the number is trusted; Report spam blocks it for the reporter", {
    when() {
        const room = roomOf(N(1))
        const stranger = as(OWNER, "org.moqui.call.SmsSpamServices.mark#NotSpam", {chatRoomId: roomOf(N(2)) + "-none"})
        const notSpam = as(MATE, "org.moqui.call.SmsSpamServices.mark#NotSpam", {chatRoomId: room})
        const after = {owner: filterOf(room, OWNER), mate: filterOf(room, MATE)}
        const misfires = Number(rule("SSR_HOST_BIT_LY").notSpamCount || 0)
        text(N(1), "Another https://bit.ly/x")
        const stillIn = filterOf(room, OWNER)
        // a plain first text, then reported by the owner
        text(N(5), "hello")
        const reportedRoom = roomOf(N(5))
        const plain = filterOf(reportedRoom, OWNER)
        const report = as(OWNER, "org.moqui.call.SmsSpamServices.report#Spam", {chatRoomId: reportedRoom})
        const handling = as(OWNER, "org.moqui.call.CallHandlingServices.get#CallHandling")
        const call = as(OWNER, "org.moqui.call.CallHandlingServices.evaluate#CallHandling", {userId: OWNER, fromNumber: N(5)})
        // the teammate turns the filter off: their Spam comes back
        const off = as(MATE, "org.moqui.call.CallHandlingServices.update#CallHandlingSetting", {spamFilter: "false"})
        return {stranger, notSpam, after, misfires, stillIn, plain, report, reportedRoom, handling, call, off}
    },
    then({stranger, notSpam, after, misfires, stillIn, plain, report, reportedRoom, handling, call, off}) {
        stranger.reasonCode == "notFound"
        notSpam.errors == ""
        after.owner == null
        after.mate == null
        misfires >= 1
        stillIn == null
        plain == null
        report.errors == ""
        filterOf(reportedRoom, OWNER) == "CmfBlocked"
        handling.blockedList.some((entry) => entry.phoneNumber == N(5))
        call.stepList[0].type == "reject"
        off.settings.spamFilter == false
        filterOf(reportedRoom, MATE) == null
    }
})

feature("a blocked number's text is out of the blocker's sight only, and an unblock brings it back", {
    when() {
        const blocked = as(OWNER, "org.moqui.call.CallHandlingServices.block#PhoneNumber", {phoneNumber: N(6)})
        const first = text(N(6), "it's me")
        const room = roomOf(N(6))
        const ownerRooms = ids(listed(OWNER))
        const mateRooms = ids(listed(MATE))
        const roomBlock = as(OWNER, "org.moqui.chat.ChatBlockServices.get#RoomBlock", {chatRoomId: room})
        const unblock = as(OWNER, "org.moqui.chat.ChatBlockServices.unblock#Room", {chatRoomId: room})
        const back = filterOf(room, OWNER)
        // Block from the room's menu puts the number on the Blocked list
        const menu = as(OWNER, "org.moqui.chat.ChatBlockServices.block#Room", {chatRoomId: room})
        return {blocked, first, room, ownerRooms, mateRooms, roomBlock, unblock, back, menu}
    },
    then({blocked, first, room, ownerRooms, mateRooms, roomBlock, unblock, back, menu}) {
        blocked.phoneNumber == N(6)
        first.outcome == "accepted"
        filterOf(room, MATE) == null
        !ownerRooms.includes(room)
        mateRooms.includes(room)
        roomBlock.filtered == "blocked"
        roomBlock.blockKind == "number"
        unblock.errors == ""
        back == null
        menu.blocked == "number"
        filterOf(room, OWNER) == "CmfBlocked"
    }
})

feature("a group text is judged by its sender and not filtered by one number's block", {
    when() {
        const out = text(N(6), "Group hello", {recipientList: [OURS, N(7)]})
        const caps = sys(() => ec.service.sync().name("org.moqui.chat.ChatServices.get#BridgeCapabilities").parameter("chatBridgeIdList", [BRIDGE]).call()) || {}
        const groupMax = Number(((caps.capabilityMap || {})[BRIDGE] || {}).groupMax || 0)
        const key = [N(6), N(7)].sort().join(",")
        return {out, groupMax, room: roomOf(key)}
    },
    then({out, groupMax, room}) {
        out.outcome == "accepted"
        groupMax == 0 || room != null
        groupMax == 0 || filterOf(room, OWNER) == null
    }
})

feature("the provider's lookup is asked only when it can change the verdict, kept, and within its day's limit", {
    when() {
        const cached = (number, lineType) => sys(() => {
            const row = ec.entity.makeValue("moqui.call.SmsSpamNumber")
            row.setFields({phoneNumber: number, lookupJson: JSON.stringify({lineType}), lookupDate: ec.user.nowTimestamp}, true, null, null)
            row.create()
        })
        const lookups = () => Number(sys(() => ec.entity.find("moqui.call.SmsSendCount").condition("counterKey", "lookup").one())?.sentCount || 0)
        cached(N(8), "nonFixedVoip")
        cached(N(9), "nonFixedVoip")
        // off by default: the kept answer is not read
        text(N(8), "Hi there example.com")
        const offRoom = roomOf(N(8))
        policy({smsSpamLookup: "Y", smsSpamLookupMaxPerDay: 1})
        text(N(9), "Hi there example.com")
        const onRoom = roomOf(N(9))
        // the gate: a first contact with no link (40) cannot reach 100 with the line type: not asked
        text(N(10), "hello there")
        const closed = lookups()
        // asked (this build's sms adapter has no lookup: notSupported), counted; then the day's limit stops the next
        text(N(11), "Hi there example.com")
        const asked = lookups()
        text(N(12), "Hi there example.com")
        const limited = lookups()
        policy({smsSpamLookup: null, smsSpamLookupMaxPerDay: null})
        return {offRoom, onRoom, closed, asked, limited}
    },
    then({offRoom, onRoom, closed, asked, limited}) {
        filterOf(offRoom, OWNER) == null
        filterOf(onRoom, OWNER) == "CmfSpam"
        closed == 0
        asked == 1
        limited == 1
    }
})

feature("the spam purge deletes old texts only in rooms every member has in Spam, and only with chat_spam_purge_days", {
    when() {
        text(N(13), "Final notice: https://bit.ly/toll unpaid toll")
        const spamRoom = roomOf(N(13))
        const mixed = roomOf(N(9))
        // the teammate turned the filter off above: N(13) is Spam for the owner only, so put it in Spam for both
        sys(() => {
            const member = ec.entity.find("moqui.chat.ChatRoomMember").condition({chatRoomId: spamRoom, userId: MATE}).one()
            member.set("filterEnumId", "CmfSpam")
            member.update()
            for (const chatRoomId of [spamRoom, mixed]) for (const event of ec.entity.find("moqui.chat.ChatEvent").condition("chatRoomId", chatRoomId).list()) {
                event.set("sentDate", new Date(Date.now() - 40 * 86400000))
                event.update()
            }
        })
        const count = (chatRoomId) => sys(() => ec.entity.find("moqui.chat.ChatEvent").condition("chatRoomId", chatRoomId).count())
        const before = {spam: count(spamRoom), mixed: count(mixed)}
        const off = sys(() => ec.service.sync().name("org.moqui.call.SmsSpamServices.purge#SpamRooms").call()) || {}
        const on = sys(() => ec.service.sync().name("org.moqui.call.SmsSpamServices.purge#SpamRooms").parameter("purgeDays", 30).call()) || {}
        return {before, off, on, after: {spam: count(spamRoom), mixed: count(mixed)}}
    },
    then({before, off, on, after}) {
        before.spam > 0
        off.roomCount == 0
        on.roomCount >= 1
        after.spam == 0
        after.mixed == before.mixed
    }
})
