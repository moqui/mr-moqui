// A person blocked in chat (plans/Comms-Messaging.md §18.3 phase 11; ChatBlocks.js, ChatBlockServices, the filters in
// ChatServices): refused, as a user that does not exist, a direct room with the blocker and adding them to a room; their
// messages in a shared room hidden from the blocker (the timeline, the links) and counting nothing (unread, mentions,
// activity); the blocker's direct room with them in Spam & blocked, where it shows everything; Block from the direct
// room's menu; and unblock.
// Run: ./moq test framework --spec ChatBlocks
const A = "CHAT_BLOCKS_A"
const B = "CHAT_BLOCKS_B"
const C = "CHAT_BLOCKS_C"
const USERS = [A, B, C]
const CHAT = "org.moqui.chat.ChatServices."
const BLOCKS = "org.moqui.chat.ChatBlockServices."

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
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "Blocks " + userId.slice(12), disabled: "N"}, true, null, null)
            account.create()
        }
    })
}
const mention = (userId) => `<span data-mention-id="${userId}">@${userId}</span>`
const timeline = (userId, chatRoomId) => Array.from(as(userId, CHAT + "get#Timeline", {chatRoomId}).eventList || []).filter((event) => event.type === "message")
const ids = (out) => Array.from(out.roomList || []).map((room) => room.chatRoomId)
const shared = {}
const GROUP = "CHAT_BLOCKS_SPEC"
/** The spec's users may run the chat services as themselves (with authorization on, so get#RefusedUsers is asked). */
function grantChat() {
    sys(() => {
        const make = (entity, fields) => ec.entity.makeValue(entity).setFields(fields, true, null, null).createOrUpdate()
        make("moqui.security.UserGroup", {userGroupId: GROUP, description: "Chat blocks spec"})
        make("moqui.security.ArtifactGroup", {artifactGroupId: GROUP, description: "Chat blocks spec"})
        make("moqui.security.ArtifactGroupMember", {artifactGroupId: GROUP, artifactName: "org\\.moqui\\.chat\\..*", artifactTypeEnumId: "AT_SERVICE",
            nameIsPattern: "Y", inheritAuthz: "Y"})
        make("moqui.security.ArtifactAuthz", {artifactAuthzId: GROUP, userGroupId: GROUP, artifactGroupId: GROUP, authzTypeEnumId: "AUTHZT_ALWAYS",
            authzActionEnumId: "AUTHZA_ALL"})
        for (const userId of USERS) make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId, fromDate: new Date(Date.now() - 60000)})
    })
}

spec({
    stepwise: true,
    setupSpec() {
        USERS.forEach(makeUser)
        grantChat()
        sys(() => ec.entity.find("moqui.chat.ChatUserBlock").condition("userId", "in", USERS).deleteAll())
    },
    cleanupSpec() {
        sys(() => ec.entity.find("moqui.chat.ChatUserBlock").condition("userId", "in", USERS).deleteAll())
        ec.user.logoutUser()
    }
})

feature("a blocked person cannot start a direct room with me or add me to a room, and is told only that I am not found", {
    when() {
        shared.direct = as(A, CHAT + "create#Room", {roomTypeEnumId: "CrtDirect", memberUserIdList: [B]}).chatRoomId
        as(B, CHAT + "send#Message", {chatRoomId: shared.direct, body: "<p>before</p>"})
        const self = as(A, BLOCKS + "block#ChatUser", {blockedUserId: A})
        const nobody = as(A, BLOCKS + "block#ChatUser", {blockedUserId: "CHAT_BLOCKS_NOBODY"})
        const blocked = as(A, BLOCKS + "block#ChatUser", {blockedUserId: B})
        const direct = as(B, CHAT + "create#Room", {roomTypeEnumId: "CrtDirect", memberUserIdList: [A]})
        const group = as(B, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "B's room", memberUserIdList: [A, C]})
        const own = as(B, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "B alone"})
        const add = as(B, CHAT + "add#Member", {chatRoomId: own.chatRoomId, userIdList: [A]})
        // a block refuses only a direct room and a private room's invite: a public room takes the blocker
        const pub = as(B, CHAT + "create#Room", {roomTypeEnumId: "CrtPublic", roomName: "B's public room"})
        const addPublic = as(B, CHAT + "add#Member", {chatRoomId: pub.chatRoomId, userIdList: [A]})
        // a third person still puts us in one room
        const third = as(C, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Team", memberUserIdList: [A, B]})
        shared.team = third.chatRoomId
        // the one who blocked may still start a direct room: the one with them, in Spam & blocked
        const mine = as(A, CHAT + "create#Room", {roomTypeEnumId: "CrtDirect", memberUserIdList: [B]})
        return {self, nobody, blocked, direct, group, add, addPublic, third, mine}
    },
    then({self, nobody, blocked, direct, group, add, addPublic, third, mine}) {
        addPublic.errors == ""
        self.reasonCode == "notFound"
        nobody.reasonCode == "notFound"
        blocked.errors == ""
        direct.errors.includes("not found")
        direct.chatRoomId == null
        group.errors.includes("not found")
        add.errors.includes("not found")
        third.errors == ""
        mine.chatRoomId == shared.direct
    }
})

feature("their messages in a shared room are hidden from me and count, mention and notify nothing", {
    when() {
        as(B, CHAT + "send#Message", {chatRoomId: shared.team, body: `<p>${mention(A)} see https://example.com/blocked</p>`})
        as(C, CHAT + "send#Message", {chatRoomId: shared.team, body: "<p>hi all</p>"})
        const mine = timeline(A, shared.team)
        const theirs = timeline(C, shared.team)
        const counts = as(A, CHAT + "get#UnreadCounts")
        const rooms = as(A, CHAT + "find#Rooms")
        const mentions = sys(() => ec.entity.find("moqui.chat.ChatEventMention").condition({chatRoomId: shared.team, userId: A}).count())
        const activity = sys(() => ec.entity.find("moqui.chat.ChatUserActivity").condition({chatRoomId: shared.team, userId: A}).count())
        // a reply to their message shows nothing of it; a thread they started is not listed
        const bSeq = timeline(C, shared.team)[0].eventSeq
        as(C, CHAT + "send#Message", {chatRoomId: shared.team, body: "<p>answering</p>", replyToEventSeq: bSeq})
        const reply = timeline(A, shared.team).find((event) => event.replyToEventSeq == bSeq)
        const linksA = Array.from(as(A, CHAT + "find#RoomLinks", {chatRoomId: shared.team}).links || [])
        const linksC = Array.from(as(C, CHAT + "find#RoomLinks", {chatRoomId: shared.team}).links || [])
        return {mine, theirs, counts, rooms, mentions, activity, linksA, linksC, reply}
    },
    then({mine, theirs, counts, rooms, mentions, activity, linksA, linksC, reply}) {
        reply.replyTo.hidden == "blocked"
        reply.replyTo.snippet == null
        mine[0].senderUserId == null
        mine.length == 2
        mine[0].hidden == "blocked"
        mine[0].body == null
        mine[0].bodyText == null
        mine[1].hidden == null
        theirs[0].body.includes("see")
        counts.roomCounts[shared.team].unread == 1
        rooms.roomList.find((room) => room.chatRoomId == shared.team).unreadCount == 1
        mentions == 0
        activity == 0
        linksA.length == 0
        linksC.length == 1
    }
})

feature("my direct room with them is in Spam & blocked and shows everything there; unblock brings it all back", {
    when() {
        const rooms = ids(as(A, CHAT + "find#Rooms"))
        const folder = as(A, CHAT + "find#Rooms", {filter: "spam"})
        const direct = timeline(A, shared.direct)
        const list = as(A, BLOCKS + "find#BlockedUsers")
        const other = as(C, BLOCKS + "get#RoomBlock", {chatRoomId: shared.direct})
        const unblock = as(A, BLOCKS + "unblock#ChatUser", {blockedUserId: B})
        const back = ids(as(A, CHAT + "find#Rooms"))
        const team = timeline(A, shared.team)
        // Block from the direct room's menu blocks the person again
        const menu = as(A, BLOCKS + "block#Room", {chatRoomId: shared.direct})
        const info = as(A, BLOCKS + "get#RoomBlock", {chatRoomId: shared.direct})
        return {rooms, folder, direct, list, other, unblock, back, team, menu, info}
    },
    then({rooms, folder, direct, list, other, unblock, back, team, menu, info}) {
        !rooms.includes(shared.direct)
        ids(folder).includes(shared.direct)
        folder.roomList.find((room) => room.chatRoomId == shared.direct).filtered == "blocked"
        direct[0].body.includes("before")
        direct[0].hidden == null
        list.blockedList.length == 1
        list.blockedList[0].userId == B
        other.reasonCode == "notFound"
        unblock.errors == ""
        back.includes(shared.direct)
        team[0].hidden == null
        menu.blocked == "person"
        info.filtered == "blocked"
        info.personBlocked == true
    }
})
