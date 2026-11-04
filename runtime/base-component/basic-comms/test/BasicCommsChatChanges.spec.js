// The changes feed of each user (org.moqui.impl.ChatServices record#UserChange and get#UserChanges; the Chat screen's
// chatChanges): what a client that keeps chat offline reads after its cursor, across all its rooms. Edits, reactions,
// redactions, link previews and polls are message changes, one per message; membership, the room, the user's settings and
// read marker, and retention each one per room; new messages are not changes but heads. A removed user hears of it; a
// purge of the feed and a cursor from another database answer reset; a user reads only their own changes.
// Run: ./moq test basic-comms --spec BasicCommsChatChanges
const root = "component://basic-comms/screen/BasicComms.xml"
const OWNER = "CHAT_CHANGES_OWNER"
const MEMBER = "CHAT_CHANGES_MEMBER"
const OTHER = "CHAT_CHANGES_OTHER"
const NAMES = {[OWNER]: "chat.changes.owner", [MEMBER]: "chat.changes.member", [OTHER]: "chat.changes.other"}
let roomId = null
let firstSeq = null
let secondSeq = null
const cursors = {}

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
    return {list, heads: page.heads}
}
const about = (list, kind) => list.filter(change => change.kind === kind && change.chatRoomId === roomId)
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
        for (const userId of [OWNER, MEMBER, OTHER]) makeUser(userId)
    }
})

feature("a first read is a reset with the cursor to start from; then a new room is a membership change and a head", {
    when() {
        const first = changes(MEMBER)
        cursors[MEMBER] = first.lastSeq
        const ownerFirst = changes(OWNER)
        cursors[OWNER] = ownerFirst.lastSeq
        const made = post(OWNER, "createRoom", {roomTypeEnumId: "private", roomName: "Changes", memberUserIds: MEMBER})
        roomId = made.json && made.json.chatRoomId
        const sent = post(OWNER, "send", {chatRoomId: roomId, body: "<p>First</p>", clientRef: "changes-1"})
        firstSeq = sent.json && sent.json.eventSeq
        const read = since(MEMBER)
        return {first, made, sent, read}
    },
    then({first, made, sent, read}) {
        first.reset == true
        first.changes.length == 0
        typeof first.lastSeq == "number"
        made.errors == ""
        sent.errors == ""
        about(read.list, "membership").length == 1
        about(read.list, "membership")[0].data.action == "joined"
        about(read.list, "message").length == 0
        read.heads.some(head => head.chatRoomId === roomId && head.lastEventSeq >= firstSeq && head.lastMessageSeq == firstSeq)
    }
})

feature("an edit and then a redaction are one message change, the latest; a reaction and its removal another", {
    when() {
        since(MEMBER)
        since(OWNER)
        const edited = post(OWNER, "edit", {chatRoomId: roomId, eventSeq: firstSeq, body: "<p>First, edited</p>"})
        const afterEdit = since(MEMBER).list
        const redacted = post(OWNER, "redact", {chatRoomId: roomId, eventSeq: firstSeq})
        const sent = post(OWNER, "send", {chatRoomId: roomId, body: "<p>Second</p>", clientRef: "changes-2"})
        secondSeq = sent.json && sent.json.eventSeq
        const reacted = post(MEMBER, "react", {chatRoomId: roomId, eventSeq: secondSeq, key: "+1"})
        const takenBack = post(MEMBER, "react", {chatRoomId: roomId, eventSeq: secondSeq, key: "+1"})
        const owner = since(OWNER).list
        return {edited, afterEdit, redacted, reacted, takenBack, owner}
    },
    then({edited, afterEdit, redacted, reacted, takenBack, owner}) {
        edited.errors == ""
        about(afterEdit, "message").length == 1
        about(afterEdit, "message")[0].eventSeq == firstSeq
        about(afterEdit, "message")[0].data.cause == "edit"
        redacted.errors == ""
        reacted.json.added == true
        takenBack.json.added == false
        about(owner, "message").filter(change => change.eventSeq == firstSeq).length == 1
        about(owner, "message").find(change => change.eventSeq == firstSeq).data.cause == "redaction"
        about(owner, "message").find(change => change.eventSeq == firstSeq).data.redacted == true
        about(owner, "message").filter(change => change.eventSeq == secondSeq).length == 1
        about(owner, "message").find(change => change.eventSeq == secondSeq).data.cause == "reaction"
    }
})

feature("an added user hears of the room and the members of its members; a removed user hears of the removal", {
    when() {
        cursors[OTHER] = changes(OTHER).lastSeq
        since(MEMBER)
        const added = post(OWNER, "addMembers", {chatRoomId: roomId, userIds: OTHER})
        const joined = since(OTHER)
        const member = since(MEMBER).list
        const removed = post(OWNER, "removeMember", {chatRoomId: roomId, userId: OTHER})
        const left = since(OTHER)
        as(OTHER)
        const timeline = request("timeline", {chatRoomId: roomId}, null)
        return {added, joined, member, removed, left, timeline}
    },
    then({added, joined, member, removed, left, timeline}) {
        added.errors == ""
        about(joined.list, "membership").length == 1
        about(joined.list, "membership")[0].data.action == "joined"
        joined.heads.some(head => head.chatRoomId === roomId)
        about(member, "room").length == 1
        about(member, "room")[0].data.reason == "members"
        about(member, "membership").length == 0
        removed.errors == ""
        about(left.list, "membership").length == 1
        about(left.list, "membership")[0].data.action == "left"
        about(left.list, "membership")[0].data.byUserId == OWNER
        !left.heads.some(head => head.chatRoomId === roomId)
        !JSON.stringify(timeline.json || {}).includes("Second")
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

feature("a change pushed to a message (its link previews, its poll) is a message change; retention is a range tombstone", {
    when() {
        since(MEMBER)
        sys(() =>
            ec.service
                .sync()
                .name("org.moqui.impl.ChatServices.push#Update")
                .parameters({chatRoomId: roomId, eventSeq: secondSeq, fields: {linkPreviews: []}})
                .call()
        )
        const updated = since(MEMBER).list
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
        return {updated, cleaned, retained}
    },
    then({updated, cleaned, retained}) {
        about(updated, "message").length == 1
        about(updated, "message")[0].eventSeq == secondSeq
        about(updated, "message")[0].data.cause == "update"
        cleaned.deletedCount > 0
        about(retained, "retention").length == 1
        about(retained, "retention")[0].eventSeq == firstSeq
    }
})

feature("pages follow afterSeq and limit to the end, each change once, in order", {
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
        return {pages, whole}
    },
    then({pages, whole}) {
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
    }
})

feature("after the feed is purged, an older cursor is a reset; a cursor past the last change too", {
    when() {
        const last = changes(MEMBER, {afterSeq: 0, limit: 1000}).lastSeq
        const cleaned = sys(() =>
            ec.service
                .sync()
                .name("org.moqui.impl.ChatServices.clean#UserChanges")
                .parameters({beforeDate: Number(ec.user.nowTimestamp) + 60000, userIdList: [MEMBER]})
                .call()
        )
        const old = changes(MEMBER, {afterSeq: 0})
        const current = changes(MEMBER, {afterSeq: last})
        const ahead = changes(MEMBER, {afterSeq: last + 100})
        const owner = changes(OWNER, {afterSeq: 0, limit: 1000})
        return {last, cleaned, old, current, ahead, owner}
    },
    then({last, cleaned, old, current, ahead, owner}) {
        cleaned.userCount == 1
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
    }
})
