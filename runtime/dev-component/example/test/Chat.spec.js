// The framework's chat and presence (org.moqui.impl.ChatServices) on the real database: the SQL of the timeline pages
// and the counts (eventSeq ranges, IN and NOT IN lists, IS NOT NULL), the search of the MoquiChatEvent document by
// membership, files as DbResource files, and the clear-at time of a status (a time stamp condition). The access rules,
// the pushes and the rest are the Rust tests in crates/moqui-core/src/service/chat_tests.rs.

const SERVICES = "org.moqui.impl.ChatServices"
const room = {}
const seq = {}

const chat = (verb, params) => call(`${SERVICES}.${verb}`, params || {})
const texts = (out) => out.eventList.filter((event) => event.type == "message" && !event.redacted).map((event) => event.bodyText)
const user = (userId, name) => {
    if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() > 0) return
    const row = ec.entity.makeValue("moqui.security.UserAccount")
    row.setFields({ userId: userId, username: userId, userFullName: name, emailAddress: `${userId}@example.com` }, true, null, null)
    row.create()
}
const as = (userId) => ec.user.internalLoginUser(userId)

spec({
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
})

feature("a room's timeline, its pages and the counts", {
    when() {
        user("chatSpecAnn", "Ann Spec")
        user("chatSpecBen", "Ben Spec")
        user("chatSpecCal", "Cal Spec")
        as("chatSpecAnn")
        room.team = chat("create#Room", { roomTypeEnumId: "private", roomName: "Spec team", memberUserIdList: ["chatSpecBen"] }).chatRoomId
        room.lobby = chat("create#Room", { roomTypeEnumId: "public", roomName: "Spec lobby" }).chatRoomId
        for (const n of [1, 2, 3, 4, 5]) seq["m" + n] = chat("send#Message", { chatRoomId: room.team, body: `<p>spec message ${n}</p>` }).eventSeq
        seq.mention = chat("send#Message", { chatRoomId: room.team,
            body: '<p>the quarterly forecast for <span data-mention-id="chatSpecBen">@Ben Spec</span></p>' }).eventSeq
        chat("send#Message", { chatRoomId: room.lobby, body: "<p>a forecast party in the lobby</p>" })
        chat("edit#Message", { chatRoomId: room.team, eventSeq: seq.m5, body: "<p>spec message 5, edited</p>" })
        chat("pin#Message", { chatRoomId: room.team, eventSeq: seq.m2 })
        chat("redact#Message", { chatRoomId: room.team, eventSeq: seq.m3 })
        as("chatSpecBen")
        const counts = chat("get#UnreadCounts")
        chat("react#Message", { chatRoomId: room.team, eventSeq: seq.m4, key: "👍" })
        // a message of one's own reads the room up to it
        chat("send#Message", { chatRoomId: room.team, body: "<p>a thread reply</p>", threadRootEventSeq: seq.m1 })
        const latest = chat("get#Timeline", { chatRoomId: room.team, limit: 4 })
        const before = chat("get#Timeline", { chatRoomId: room.team, limit: 3, beforeSeq: latest.eventList[0].eventSeq })
        const after = chat("get#Timeline", { chatRoomId: room.team, limit: 2, afterSeq: seq.m1 })
        const around = chat("get#Timeline", { chatRoomId: room.team, limit: 4, aroundSeq: seq.m4 })
        const thread = chat("get#Timeline", { chatRoomId: room.team, threadRootEventSeq: seq.m1 })
        const pinned = chat("get#Timeline", { chatRoomId: room.team, pinnedOnly: true })
        const rooms = chat("find#Rooms", { includeJoinable: true })
        const afterReply = chat("get#UnreadCounts")
        as("chatSpecAnn")
        const annBefore = chat("get#UnreadCounts")
        chat("mark#Read", { chatRoomId: room.team })
        return { counts, latest, before, after, around, thread, pinned, rooms, afterReply, annBefore, read: chat("get#UnreadCounts") }
    },
    then({ counts, latest, before, after, around, thread, pinned, rooms, afterReply, annBefore, read }) {
        // m1, m2, m4, m5 and the mention: the redacted one does not count
        counts.unread == 5
        counts.mentions == 1
        counts.total == 5
        texts(latest) == ["spec message 4", "spec message 5, edited", "the quarterly forecast for @Ben Spec"]
        latest.eventList[3].type == "pin"
        latest.hasMoreBefore == true
        latest.eventList[0].reactions.length == 1
        latest.eventList[0].reactions[0].mine == true
        latest.eventList[1].edited == true
        before.eventList.some((event) => event.redacted && event.removedLabel == "Message removed")
        texts(after) == ["spec message 2"]
        after.hasMoreAfter == true
        texts(around) == ["spec message 2", "spec message 4", "spec message 5, edited"]
        afterReply.total == 0
        // Ann: Ben's thread reply
        annBefore.unread == 1
        texts(thread) == ["spec message 1", "a thread reply"]
        texts(pinned) == ["spec message 2"]
        rooms.roomList.map((item) => item.displayName).includes("Spec team")
        rooms.roomList.find((item) => item.chatRoomId == room.lobby).isMember == false
        read.total == 0
    },
})

feature("search: a member finds the room's messages, a stranger only the public room's", {
    when() {
        const finds = (userId) => {
            as(userId)
            return chat("search#ChatEvents", { searchText: "forecast", limit: 50 }).eventList
                .filter((hit) => hit.chatRoomId == room.team || hit.chatRoomId == room.lobby).map((hit) => hit.roomName).sort()
        }
        return { searchAvailable: ec.search.isAvailable(), ben: finds("chatSpecBen"), cal: finds("chatSpecCal") }
    },
    then({ searchAvailable, ben, cal }) {
        searchAvailable
        ben == ["Spec lobby", "Spec team"]
        cal == ["Spec lobby"]
    },
})

feature("a file is a DbResource file that goes with its message", {
    when() {
        as("chatSpecAnn")
        const stored = chat("upload#Attachment", { chatRoomId: room.team, file: uploadedFile("spec-plan.txt", "the spec plan") })
        const text = ec.resource.getLocationReference(stored.location).getText()
        const sent = chat("send#Message", { chatRoomId: room.team, chatAttachmentIdList: [stored.chatAttachmentId] })
        const shown = chat("get#Timeline", { chatRoomId: room.team, limit: 1 }).eventList[0]
        chat("redact#Message", { chatRoomId: room.team, eventSeq: sent.eventSeq })
        return { stored, text, shown, gone: !ec.resource.getLocationReference(stored.location).getExists(),
            rows: ec.entity.find("moqui.chat.ChatAttachment").condition("chatAttachmentId", stored.chatAttachmentId).count() }
    },
    then({ stored, text, shown, gone, rows }) {
        stored.location == `dbresource://chat/${room.team}/${stored.chatAttachmentId}/spec-plan.txt`
        text == "the spec plan"
        shown.attachments.map((file) => file.fileName) == ["spec-plan.txt"]
        gone
        rows == 0
    },
})

feature("a status clears at its time, and presence follows the heartbeat", {
    when() {
        const start = Date.now()
        ec.user.setEffectiveTime(start)
        as("chatSpecAnn")
        chat("heartbeat#UserStatus", { idleSeconds: 0 })
        const set = chat("set#UserStatus", { status: "dnd", statusText: "Spec focus", clearAfterMinutes: 30 }).myStatus
        const early = chat("expire#UserStatuses")
        ec.user.setEffectiveTime(start + 31 * 60000)
        const later = chat("expire#UserStatuses")
        const row = ec.entity.find("moqui.security.UserStatus").condition("userId", "chatSpecAnn").one()
        as("chatSpecBen")
        const seen = chat("get#UserStatuses", { userIdList: ["chatSpecAnn"] }).statusMap.chatSpecAnn
        return { set, early, later, row, seen }
    },
    then({ set, early, later, row, seen }) {
        set.presence == "dnd"
        set.statusText == "Spec focus"
        early.clearedCount == 0
        later.clearedCount >= 1
        row.statusText == null
        row.statusEnumId == "UpsOnline"
        // no heartbeat for 31 minutes
        seen.presence == "offline"
    },
})
