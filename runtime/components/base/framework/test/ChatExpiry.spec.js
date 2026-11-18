// Disappearing messages (plans/Comms-Messaging.md §18.3 phase 16; ChatExpiry.js, ChatExpiryServices): a room's timer, who
// sets it and where it is refused (a bridged and a record room, a bridge onto a room with one); a message's expireDate and
// its edit's and reaction's; and expire#DueMessages, called directly with a clock past the time: the message goes with its
// mentions, activity, links, the preview cache row no other message links, its thread root's count, the room's last
// message and its room changes, for one marker change; an encrypted room's key backup of a session no event uses any
// more; and a forward of an expiring message refused.
// Run: ./moq test framework --spec ChatExpiry
const A = "CHAT_EXPIRY_A"
const B = "CHAT_EXPIRY_B"
const C = "CHAT_EXPIRY_C"
const USERS = [A, B, C]
const CHAT = "org.moqui.chat.ChatServices."
const EXPIRY = "org.moqui.chat.ChatExpiryServices."
const GROUP = "CHAT_EXPIRY_SPEC"

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
        out = ec.service.sync().name(name).parameters(parameters || {}).call() || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
function setup() {
    sys(() => {
        const make = (entity, fields) => ec.entity.makeValue(entity).setFields(fields, true, null, null).createOrUpdate()
        for (const userId of USERS) make("moqui.security.UserAccount", {userId, username: username(userId), userFullName: "Expiry " + userId.slice(12), disabled: "N"})
        make("moqui.security.UserGroup", {userGroupId: GROUP, description: "Chat expiry spec"})
        make("moqui.security.ArtifactGroup", {artifactGroupId: GROUP, description: "Chat expiry spec"})
        make("moqui.security.ArtifactGroupMember", {artifactGroupId: GROUP, artifactName: "org\\.moqui\\.chat\\..*", artifactTypeEnumId: "AT_SERVICE",
            nameIsPattern: "Y", inheritAuthz: "Y"})
        make("moqui.security.ArtifactAuthz", {artifactAuthzId: GROUP, userGroupId: GROUP, artifactGroupId: GROUP, authzTypeEnumId: "AUTHZT_ALWAYS",
            authzActionEnumId: "AUTHZA_ALL"})
        for (const userId of USERS) make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId, fromDate: new Date(Date.now() - 60000)})
    })
}
const event = (roomId, seq) => sys(() => ec.entity.find("moqui.chat.ChatEvent").condition({chatRoomId: roomId, eventSeq: seq}).one())
const count = (entity, fields) => sys(() => ec.entity.find(entity).condition(fields).count())
const mention = (userId) => `<span data-mention-id="${userId}">@${userId}</span>`
// a preview kept in the cache, so a message's links are not fetched
function cached(url) {
    return sys(() => {
        const urlHash = ec.service.sync().name(CHAT + "extract#LinkUrls").parameters({body: url, listed: true}).call().urlList[0].urlHash
        ec.entity.makeValue("moqui.chat.ChatLinkPreview").setFields({urlHash, url, title: url, failed: "N", fetchedDate: ec.user.nowTimestamp}, true, null, null).createOrUpdate()
        return urlHash
    })
}
// the due deletes, as the worker runs them, at a clock past every time set here
const expireAt = (millis) => sys(() => ec.service.sync().name(EXPIRY + "expire#DueMessages").parameters({nowMillis: millis}).call() || {})
const shared = {}

spec({
    stepwise: true,
    setupSpec() { setup() },
    cleanupSpec() { ec.user.logoutUser() }
})

feature("the timer: a direct room's member sets it, a group's moderator, and a bridged or record room has none", {
    when() {
        shared.direct = as(A, CHAT + "create#Room", {roomTypeEnumId: "CrtDirect", memberUserIdList: [B]}).chatRoomId
        shared.group = as(A, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Expiry group", memberUserIdList: [B, C]}).chatRoomId
        const byMember = as(B, EXPIRY + "set#RoomTimer", {chatRoomId: shared.direct, seconds: 3600})
        const groupMember = as(B, EXPIRY + "set#RoomTimer", {chatRoomId: shared.group, seconds: 3600})
        const tooShort = as(A, EXPIRY + "set#RoomTimer", {chatRoomId: shared.group, seconds: 10})
        const tooLong = as(A, EXPIRY + "set#RoomTimer", {chatRoomId: shared.group, seconds: 29 * 86400})
        const owner = as(A, EXPIRY + "set#RoomTimer", {chatRoomId: shared.group, seconds: 300})
        const room = as(A, CHAT + "get#Room", {chatRoomId: shared.direct}).room
        const system = Array.from(as(A, CHAT + "get#Timeline", {chatRoomId: shared.direct}).eventList || []).find((item) => item.type === "system" && item.content.action === "timer")
        // a bridged room: a bridge cannot delete there; and a room with a timer is not bridged
        const bridged = as(A, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Expiry bridged", memberUserIdList: [B]}).chatRoomId
        sys(() => {
            ec.entity.makeValue("moqui.chat.ChatBridge").setFields({chatBridgeId: "CHAT_EXPIRY_BRIDGE", name: "Expiry bridge"}, true, null, null).createOrUpdate()
            ec.entity.makeValue("moqui.chat.ChatRoomBridge").setFields({chatRoomBridgeId: "CHAT_EXPIRY_MAP", chatRoomId: bridged, chatBridgeId: "CHAT_EXPIRY_BRIDGE",
                externalRoomId: "elsewhere", directionEnumId: "CrbBoth", fromDate: ec.user.nowTimestamp}, true, null, null).createOrUpdate()
        })
        const onBridged = as(A, EXPIRY + "set#RoomTimer", {chatRoomId: bridged, seconds: 3600})
        const checked = sys(() => ec.resource.script("component://framework/service/org/moqui/chat/ChatExpiry.js").execute("checkBridge", {chatRoomId: shared.group}))
        ec.message.clearErrors()
        const record = as(A, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Expiry record", memberUserIdList: [B]}).chatRoomId
        sys(() => { const row = ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", record).one(); row.set("roomTypeEnumId", "CrtRecord"); row.update() })
        const onRecord = as(A, EXPIRY + "set#RoomTimer", {chatRoomId: record, seconds: 3600})
        return {byMember, groupMember, tooShort, tooLong, owner, room, system, onBridged, checked, onRecord}
    },
    then({byMember, groupMember, tooShort, tooLong, owner, room, system, onBridged, checked, onRecord}) {
        byMember.errors == ""
        byMember.seconds == 3600
        room.messageTimerSeconds == 3600
        system.content.seconds == 3600
        system.content.byUserId == B
        groupMember.reasonCode == "notAllowed"
        tooShort.reasonCode == "range"
        tooLong.reasonCode == "range"
        owner.seconds == 300
        onBridged.reasonCode == "bridged"
        checked.reasonCode == "timerOn"
        onRecord.reasonCode == "recordRoom"
    }
})

feature("an expiring message: its time, and its edit's and reaction's; a forward of it is refused", {
    when() {
        const before = Date.now()
        const sent = as(A, CHAT + "send#Message", {chatRoomId: shared.direct, body: "<p>soon gone</p>"})
        shared.message = sent.eventSeq
        const edit = as(A, CHAT + "edit#Message", {chatRoomId: shared.direct, eventSeq: sent.eventSeq, body: "<p>soon gone, edited</p>"})
        const reaction = as(B, CHAT + "react#Message", {chatRoomId: shared.direct, eventSeq: sent.eventSeq, key: "👍"})
        const row = event(shared.direct, sent.eventSeq)
        const editRow = sys(() => ec.entity.find("moqui.chat.ChatEvent").condition({chatRoomId: shared.direct, editsEventSeq: sent.eventSeq}).one())
        const reactionRow = event(shared.direct, reaction.reactionEventSeq)
        const forward = as(A, CHAT + "send#Message", {chatRoomId: shared.group, forwardOfChatRoomId: shared.direct, forwardOfEventSeq: sent.eventSeq})
        const membership = sys(() => ec.entity.find("moqui.chat.ChatEvent").condition({chatRoomId: shared.direct, eventTypeEnumId: "CetMembership"}).list())
        return {before, sent, edit, row, editRow, reactionRow, forward, membership}
    },
    then({before, sent, edit, row, editRow, reactionRow, forward, membership}) {
        sent.errors == ""
        sent.event.expireDate >= before + 3600000
        ec.user.timestampMillis(row.expireDate) == sent.event.expireDate
        ec.user.timestampMillis(editRow.expireDate) == sent.event.expireDate
        ec.user.timestampMillis(reactionRow.expireDate) == sent.event.expireDate
        forward.reasonCode == "expiring"
        membership.every((item) => item.expireDate == null)
    }
})

feature("at its time a message goes with its mentions, activity, links, preview, thread count, the room's last message and changes", {
    when() {
        // in the group (5 minutes): a root sent before the timer stands, its expiring reply goes
        sys(() => { const row = ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", shared.group).one(); row.set("messageTimerSeconds", null); row.update() })
        const root = as(A, CHAT + "send#Message", {chatRoomId: shared.group, body: "<p>the root, kept</p>"}).eventSeq
        as(A, EXPIRY + "set#RoomTimer", {chatRoomId: shared.group, seconds: 300})
        const only = "https://expiry.example.com/only"
        const both = "https://expiry.example.com/both"
        const onlyHash = cached(only)
        const bothHash = cached(both)
        const plain = as(C, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Expiry plain", memberUserIdList: [A]}).chatRoomId
        as(C, CHAT + "send#Message", {chatRoomId: plain, body: `<p>${both}</p>`})
        const reply = as(A, CHAT + "send#Message", {chatRoomId: shared.group, threadRootEventSeq: root,
            body: `<p>${mention(B)} see ${only} and ${both}</p>`}).eventSeq
        const kept = {mentions: count("moqui.chat.ChatEventMention", {chatRoomId: shared.group, eventSeq: reply}),
            activity: count("moqui.chat.ChatUserActivity", {chatRoomId: shared.group, eventSeq: reply}),
            links: count("moqui.chat.ChatEventLink", {chatRoomId: shared.group, eventSeq: reply}),
            rootCount: Number(event(shared.group, root).threadReplyCount || 0)}
        // the direct room's last message is the expiring one, with an edit and a reaction
        const lastBefore = sys(() => ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", shared.direct).one()).lastMessageSeq
        const round = expireAt(Date.now() + 3600000 + 60000)
        const directRoom = sys(() => ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", shared.direct).one())
        const marker = sys(() => ec.entity.find("moqui.chat.ChatRoomChange").condition({chatRoomId: shared.direct, eventSeq: null}).one())
        const changes = sys(() => ec.entity.find("moqui.chat.ChatRoomChange").condition({chatRoomId: shared.direct, eventSeq: shared.message}).count())
        const feed = as(B, CHAT + "get#RoomChanges", {chatRoomId: shared.direct, afterSeq: 0})
        const gone = {message: event(shared.group, reply), direct: event(shared.direct, shared.message),
            edits: count("moqui.chat.ChatEvent", {chatRoomId: shared.direct, editsEventSeq: shared.message}),
            reactions: count("moqui.chat.ChatEvent", {chatRoomId: shared.direct, relatesToEventSeq: shared.message}),
            mentions: count("moqui.chat.ChatEventMention", {chatRoomId: shared.group, eventSeq: reply}),
            activity: count("moqui.chat.ChatUserActivity", {chatRoomId: shared.group, eventSeq: reply}),
            links: count("moqui.chat.ChatEventLink", {chatRoomId: shared.group, eventSeq: reply}),
            onlyPreview: count("moqui.chat.ChatLinkPreview", {urlHash: onlyHash}), bothPreview: count("moqui.chat.ChatLinkPreview", {urlHash: bothHash}),
            root: event(shared.group, root)}
        return {kept, lastBefore, round, directRoom, marker, changes, feed, gone}
    },
    then({kept, lastBefore, round, directRoom, marker, changes, feed, gone}) {
        kept.mentions == 1
        kept.activity >= 1
        kept.links == 2
        kept.rootCount == 1
        lastBefore == shared.message
        round.deletedCount >= 4
        round.failedCount == 0
        gone.message == null
        gone.direct == null
        gone.edits == 0
        gone.reactions == 0
        gone.mentions == 0
        gone.activity == 0
        gone.links == 0
        gone.onlyPreview == 0
        gone.bothPreview == 1
        gone.root != null
        Number(gone.root.threadReplyCount || 0) == 0
        gone.root.threadLastEventSeq == null
        directRoom.lastMessageSeq == null
        directRoom.lastMessageSnippet == null
        JSON.parse(marker.changeJson).cause == "expired"
        changes == 0
        JSON.stringify(feed.changeList).includes('"eventSeq":null')
        JSON.stringify(feed.changeList).includes('"cause":"expired"')
    }
})

feature("an encrypted room: a session's key backup goes with its last event, and stays while one is left", {
    when() {
        const sealed = (session) => ({algorithm: "m.megolm.v1.aes-sha2", ciphertext: "AwgAEn" + session, session_id: session, sender_key: "curve", device_id: "DEVICEAAAA"})
        const room = as(A, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Expiry encrypted", memberUserIdList: [B], encrypted: true}).chatRoomId
        // the first message before the timer, in the session that goes on after it
        as(A, CHAT + "send#Message", {chatRoomId: room, encrypted: sealed("expiry-s1")})
        as(A, EXPIRY + "set#RoomTimer", {chatRoomId: room, seconds: 60})
        as(A, CHAT + "send#Message", {chatRoomId: room, encrypted: sealed("expiry-s1")})
        as(A, CHAT + "send#Message", {chatRoomId: room, encrypted: sealed("expiry-s2")})
        const roomId = "!" + room + ":" + String(System.getProperty("chat_server_name") || "localhost").toLowerCase()
        sys(() => {
            const make = (entity, fields) => ec.entity.makeValue(entity).setFields(fields, true, null, null).createOrUpdate()
            make("moqui.chat.ChatKeyBackup", {userId: A, version: "1", algorithm: "m.megolm_backup.v1.curve25519-aes-sha2", createdDate: ec.user.nowTimestamp})
            for (const sessionId of ["expiry-s1", "expiry-s2"])
                make("moqui.chat.ChatKeyBackupSession", {userId: A, version: "1", roomId, sessionId, sessionDataJson: "{}", uploadedDate: ec.user.nowTimestamp})
        })
        const sessions = sys(() => ec.entity.find("moqui.chat.ChatEvent").condition({chatRoomId: room}).condition("megolmSessionId", "not-equals", null).count())
        expireAt(Date.now() + 120000)
        const left = sys(() => Array.from(ec.entity.find("moqui.chat.ChatKeyBackupSession").condition({userId: A, roomId}).list()).map((row) => row.sessionId))
        const events = count("moqui.chat.ChatEvent", {chatRoomId: room, eventTypeEnumId: "CetMessage"})
        return {sessions, left, events}
    },
    then({sessions, left, events}) {
        sessions == 3
        events == 1
        left.length == 1
        left[0] == "expiry-s1"
    }
})
