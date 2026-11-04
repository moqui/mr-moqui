// The changes feeds of chat (org.moqui.impl.ChatServices record#UserChange, get#UserChanges, record#RoomChange and
// get#RoomChanges; the Chat screen's chatChanges and roomChanges): what a client that keeps chat offline reads after its
// cursors. Each user's feed has only what is theirs: membership as state, the room, their settings and read marker, and
// retention, one row per room and kind. A room's message changes (edits, reactions, redactions, pins, link previews and
// polls) are the room's, one row per message, and its heads carry their number; new messages are in neither. A removed user
// hears of it; agents get no user changes; a purge and a cursor from another database answer reset; a user reads only their
// own changes and the rooms they may read. Run: ./moq test basic-comms --spec BasicCommsChatChanges
const root = "component://basic-comms/screen/BasicComms.xml"
const OWNER = "CHAT_CHANGES_OWNER"
const MEMBER = "CHAT_CHANGES_MEMBER"
const OTHER = "CHAT_CHANGES_OTHER"
const AGENT = "CHAT_CHANGES_AGENT"
const NAMES = {
    [OWNER]: "chat.changes.owner",
    [MEMBER]: "chat.changes.member",
    [OTHER]: "chat.changes.other",
    [AGENT]: "chat.changes.agent"
}
let roomId = null
let firstSeq = null
let secondSeq = null
const cursors = {}
let roomCursor = null

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function request(path, parameters, method) {
    const str = ec.screen
        .makeTest()
        .rootScreen(root)
        .render("Chat/" + path, parameters || {}, method || null)
    const output = String(str.output || "")
    const errors = str.errorMessages.join(" | ")
    ec.message.clearErrors()
    let json = null
    try {
        json = JSON.parse(output)
    } catch (err) {
        json = null
    }
    return {status: str.status, errors, json}
}
function as(userId) {
    ec.user.internalLoginUser(NAMES[userId])
}
function post(userId, path, parameters) {
    as(userId)
    return request(path, parameters, "post")
}
function changes(userId, parameters) {
    as(userId)
    return request("chatChanges", parameters || {}, null).json
}
function roomChanges(userId, parameters) {
    as(userId)
    return request("roomChanges", Object.assign({chatRoomId: roomId}, parameters || {}), null)
}
// the user's changes after their cursor, read to the end two at a time; the cursor moves on
function since(userId) {
    const list = []
    let page = null
    for (let round = 0; round < 100; round++) {
        page = changes(userId, {afterSeq: cursors[userId], limit: 2})
        if (page.reset) throw new Error(`The feed of ${userId} was reset at ${cursors[userId]}`)
        list.push(...page.changes)
        cursors[userId] = page.lastSeq
        if (!page.more) break
    }
    return {list, heads: page.heads, headsSince: page.headsSince}
}
// the room's changes after the room cursor, as the member reads them; the cursor moves on
function roomSince() {
    const page = roomChanges(MEMBER, {afterSeq: roomCursor, limit: 1000}).json
    if (page.reset) throw new Error(`The room's feed was reset at ${roomCursor}`)
    roomCursor = page.lastSeq
    return page.changes
}
const about = (list, kind) => list.filter(change => change.kind === kind && change.chatRoomId === roomId)
const headOf = heads => (heads || []).find(head => head.chatRoomId === roomId)
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() == 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields(
                {userId, username: NAMES[userId], userFullName: "Changes " + userId, disabled: "N"},
                true,
                null,
                null
            )
            account.create()
        }
    })
}

spec({
    stepwise: true,
    setupSpec() {
        for (const userId of [OWNER, MEMBER, OTHER, AGENT]) makeUser(userId)
        sys(() => {
            if (ec.entity.find("moqui.agent.AgentDefinition").condition("agentId", "CHAT_CHANGES_AGENT").count() == 0) {
                const agent = ec.entity.makeValue("moqui.agent.AgentDefinition")
                agent.setFields({agentId: "CHAT_CHANGES_AGENT", agentName: "Changes agent", chatUserId: AGENT}, true, null, null)
                agent.create()
            }
        })
    }
})

feature("a first read is a reset with the cursor to start from; then a new room is a membership state and a head", {
    when() {
        const first = changes(MEMBER)
        cursors[MEMBER] = first.lastSeq
        cursors[OWNER] = changes(OWNER).lastSeq
        const made = post(OWNER, "createRoom", {roomTypeEnumId: "private", roomName: "Changes", memberUserIds: MEMBER})
        roomId = made.json && made.json.chatRoomId
        const sent = post(OWNER, "send", {chatRoomId: roomId, body: "<p>First</p>", clientRef: "changes-1"})
        firstSeq = sent.json && sent.json.eventSeq
        const read = since(MEMBER)
        const roomFirst = roomChanges(MEMBER).json
        roomCursor = roomFirst.lastSeq
        return {first, made, sent, read, roomFirst}
    },
    then({first, made, sent, read, roomFirst}) {
        first.reset == true
        first.changes.length == 0
        typeof first.lastSeq == "number"
        made.errors == ""
        sent.errors == ""
        about(read.list, "membership").length == 1
        about(read.list, "membership")[0].data.member == true
        about(read.list, "membership")[0].data.roleEnumId == "CmrMember"
        about(read.list, "membership")[0].data.action == "joined"
        read.list.every(change => ["membership", "room", "settings", "read", "retention"].includes(change.kind))
        headOf(read.heads).lastEventSeq >= firstSeq
        headOf(read.heads).lastMessageSeq == firstSeq
        headOf(read.heads).lastChangeSeq == 0
        typeof read.headsSince == "number"
        roomFirst.reset == true
        roomFirst.lastSeq == 0
    }
})

feature("an edit, then a redaction, is one change of the room's message, the latest; a reaction and its removal another", {
    when() {
        since(MEMBER)
        const capture = ec.factory.getTool("NotificationCapture")
        capture.enable()
        capture.clear()
        let edited, pushed
        try {
            edited = post(OWNER, "edit", {chatRoomId: roomId, eventSeq: firstSeq, body: "<p>First, edited</p>"})
            pushed = capture.last("chatRoom:" + roomId)
        } finally {
            capture.disable()
            capture.clear()
        }
        const afterEdit = roomSince()
        const redacted = post(OWNER, "redact", {chatRoomId: roomId, eventSeq: firstSeq})
        const sent = post(OWNER, "send", {chatRoomId: roomId, body: "<p>Second</p>", clientRef: "changes-2"})
        secondSeq = sent.json && sent.json.eventSeq
        const reacted = post(MEMBER, "react", {chatRoomId: roomId, eventSeq: secondSeq, key: "+1"})
        const takenBack = post(MEMBER, "react", {chatRoomId: roomId, eventSeq: secondSeq, key: "+1"})
        const room = roomChanges(OWNER, {afterSeq: 0, limit: 1000}).json
        const mine = since(MEMBER)
        return {edited, pushed, afterEdit, redacted, reacted, takenBack, room, mine}
    },
    then({edited, pushed, afterEdit, redacted, reacted, takenBack, room, mine}) {
        edited.errors == ""
        afterEdit.length == 1
        afterEdit[0].eventSeq == firstSeq
        afterEdit[0].data.cause == "edit"
        pushed.message.roomChangeSeq == afterEdit[0].changeSeq
        redacted.errors == ""
        reacted.json.added == true
        takenBack.json.added == false
        room.changes.filter(change => change.eventSeq == firstSeq).length == 1
        room.changes.find(change => change.eventSeq == firstSeq).data.cause == "redaction"
        room.changes.find(change => change.eventSeq == firstSeq).data.redacted == true
        room.changes.filter(change => change.eventSeq == secondSeq).length == 1
        room.changes.find(change => change.eventSeq == secondSeq).data.cause == "reaction"
        mine.list.length == 0
        headOf(mine.heads).lastChangeSeq == room.lastSeq
    }
})

feature("an added user unseen then made moderator reads both as one state; a removed user hears of the removal", {
    when() {
        cursors[OTHER] = changes(OTHER).lastSeq
        since(MEMBER)
        const added = post(OWNER, "addMembers", {chatRoomId: roomId, userIds: OTHER})
        const promoted = post(OWNER, "setRole", {chatRoomId: roomId, userId: OTHER, roleEnumId: "moderator"})
        const joined = since(OTHER)
        const member = since(MEMBER).list
        const removed = post(OWNER, "removeMember", {chatRoomId: roomId, userId: OTHER})
        const left = since(OTHER)
        as(OTHER)
        const timeline = request("timeline", {chatRoomId: roomId}, null)
        const outsider = roomChanges(OTHER, {afterSeq: 0})
        return {added, promoted, joined, member, removed, left, timeline, outsider}
    },
    then({added, promoted, joined, member, removed, left, timeline, outsider}) {
        added.errors == ""
        promoted.errors == ""
        about(joined.list, "membership").length == 1
        about(joined.list, "membership")[0].data.member == true
        about(joined.list, "membership")[0].data.roleEnumId == "CmrModerator"
        headOf(joined.heads) != null
        about(member, "room").length == 1
        about(member, "room")[0].data.reason == "members"
        about(member, "membership").length == 0
        removed.errors == ""
        about(left.list, "membership").length == 1
        about(left.list, "membership")[0].data.member == false
        about(left.list, "membership")[0].data.byUserId == OWNER
        headOf(left.heads) == null
        !JSON.stringify(timeline.json || {}).includes("Second")
        outsider.errors.includes("not found")
        !JSON.stringify(outsider.json || {}).includes("changeSeq")
    }
})

feature("an agent member gets no changes of its own, while the others hear of it", {
    when() {
        since(MEMBER)
        as(OWNER)
        const added = sys(() =>
            ec.service
                .sync()
                .name("org.moqui.impl.ChatServices.add#Member")
                .parameters({chatRoomId: roomId, userId: AGENT, roleEnumId: "agent"})
                .call()
        )
        const errors = ec.message.errorsString
        ec.message.clearErrors()
        const agentRows = sys(() => ec.entity.find("moqui.chat.ChatUserChange").condition("userId", AGENT).count())
        const member = since(MEMBER).list
        return {added, errors, agentRows, member}
    },
    then({added, errors, agentRows, member}) {
        errors == ""
        Array.from(added.addedUserIdList).includes(AGENT)
        agentRows == 0
        about(member, "room").length == 1
    }
})

feature("the user's settings and read marker are their own changes, pushed with their changeSeq; others never see them", {
    when() {
        since(MEMBER)
        since(OWNER)
        const capture = ec.factory.getTool("NotificationCapture")
        capture.enable()
        capture.clear()
        let muted, pushed
        try {
            muted = post(MEMBER, "setNotification", {chatRoomId: roomId, isMuted: "true"})
            pushed = capture.last("chatUser:" + MEMBER)
        } finally {
            capture.disable()
            capture.clear()
        }
        const read = post(MEMBER, "markRead", {chatRoomId: roomId, eventSeq: firstSeq})
        const mine = since(MEMBER).list
        const owner = since(OWNER).list
        as(OWNER)
        const asked = request("chatChanges", {afterSeq: 0, userId: MEMBER}, null).json
        return {muted, pushed, read, mine, owner, asked}
    },
    then({muted, pushed, read, mine, owner, asked}) {
        muted.errors == ""
        about(mine, "settings").length == 1
        about(mine, "settings")[0].data.isMuted == true
        pushed != null
        pushed.message.changeSeq == about(mine, "settings")[0].changeSeq
        read.errors == ""
        about(mine, "read").length == 1
        about(mine, "read")[0].data.lastReadEventSeq == read.json.lastReadEventSeq
        about(owner, "settings").length == 0
        about(owner, "read").length == 0
        !JSON.stringify(asked).includes('"isMuted"')
    }
})

feature("a change pushed to a message is the room's; retention is a range tombstone and takes the room's changes it covers", {
    when() {
        since(MEMBER)
        roomSince()
        sys(() =>
            ec.service
                .sync()
                .name("org.moqui.impl.ChatServices.push#Update")
                .parameters({chatRoomId: roomId, eventSeq: secondSeq, fields: {linkPreviews: []}})
                .call()
        )
        const updated = roomSince()
        // the events up to the first message are past a day's retention
        const twoDaysAgo = String(Number(ec.user.nowTimestamp) - 2 * 86400000)
        const cleaned = sys(() => {
            ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", roomId).updateAll({retentionDays: 1})
            ec.entity
                .find("moqui.chat.ChatEvent")
                .condition("chatRoomId", roomId)
                .condition("eventSeq", "less-equals", firstSeq)
                .updateAll({sentDate: twoDaysAgo})
            return ec.service
                .sync()
                .name("org.moqui.impl.ChatServices.clean#RoomRetention")
                .parameter("chatRoomId", roomId)
                .call()
        })
        const retained = since(MEMBER).list
        const room = roomChanges(MEMBER, {afterSeq: 0, limit: 1000}).json
        return {updated, cleaned, retained, room}
    },
    then({updated, cleaned, retained, room}) {
        updated.length == 1
        updated[0].eventSeq == secondSeq
        updated[0].data.cause == "update"
        cleaned.deletedCount > 0
        about(retained, "retention").length == 1
        about(retained, "retention")[0].eventSeq == firstSeq
        room.changes.every(change => change.eventSeq > firstSeq)
        room.changes.some(change => change.eventSeq == secondSeq)
    }
})

feature("pages follow afterSeq and limit to the end, each change once, in order; heads only since headsSince", {
    when() {
        const pages = []
        let cursor = 0
        for (let round = 0; round < 100; round++) {
            const page = changes(MEMBER, {afterSeq: cursor, limit: 1})
            pages.push(page)
            cursor = page.lastSeq
            if (!page.more) break
        }
        const whole = changes(MEMBER, {afterSeq: 0, limit: 1000})
        const quiet = changes(MEMBER, {afterSeq: whole.lastSeq, headsSince: Number(ec.user.nowTimestamp) + 3600000})
        const all = changes(MEMBER, {afterSeq: whole.lastSeq, headsSince: 0})
        return {pages, whole, quiet, all}
    },
    then({pages, whole, quiet, all}) {
        pages.length > 3
        pages.slice(0, -1).every(page => page.more && page.changes.length == 1 && page.heads == null)
        pages[pages.length - 1].heads != null
        pages
            .flatMap(page => page.changes)
            .map(change => change.changeSeq)
            .join() == whole.changes.map(change => change.changeSeq).join()
        whole.changes.every((change, index) => index == 0 || change.changeSeq > whole.changes[index - 1].changeSeq)
        pages[pages.length - 1].lastSeq == whole.lastSeq
        whole.reset == false
        headOf(quiet.heads) == null
        headOf(all.heads) != null
    }
})

feature("after the feeds are purged, an older cursor is a reset; a cursor past the last change too", {
    when() {
        const last = changes(MEMBER, {afterSeq: 0, limit: 1000}).lastSeq
        const roomLast = roomChanges(MEMBER, {afterSeq: 0, limit: 1000}).json.lastSeq
        const cleaned = sys(() =>
            ec.service
                .sync()
                .name("org.moqui.impl.ChatServices.clean#ChatChanges")
                .parameters({beforeDate: Number(ec.user.nowTimestamp) + 60000, userIdList: [MEMBER], chatRoomIdList: [roomId]})
                .call()
        )
        const old = changes(MEMBER, {afterSeq: 0})
        const current = changes(MEMBER, {afterSeq: last})
        const ahead = changes(MEMBER, {afterSeq: last + 100})
        const owner = changes(OWNER, {afterSeq: 0, limit: 1000})
        const roomOld = roomChanges(MEMBER, {afterSeq: 0}).json
        const roomCurrent = roomChanges(MEMBER, {afterSeq: roomLast}).json
        return {last, roomLast, cleaned, old, current, ahead, owner, roomOld, roomCurrent}
    },
    then({last, roomLast, cleaned, old, current, ahead, owner, roomOld, roomCurrent}) {
        cleaned.userCount == 1
        cleaned.roomCount == 1
        cleaned.deletedCount > 0
        old.reset == true
        old.changes.length == 0
        old.lastSeq == last
        current.reset == false
        current.changes.length == 0
        current.lastSeq == last
        ahead.reset == true
        owner.reset == false
        owner.changes.length > 0
        roomOld.reset == true
        roomOld.lastSeq == roomLast
        roomCurrent.reset == false
    }
})
