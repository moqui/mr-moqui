// The desktop gaps of Chat (plans/Comms-Messaging.md §3, phase 8), their rules on the server: the palette's Chat rooms
// (search#PaletteRooms: rooms by name, a phone-number room by its digits, and no room the user may not read; the people
// with no direct chat of their own are search#PalettePeople, each opening Chat with dm), the Threads view
// (find#ChatThreads: the threads the user started or replied in, across rooms, by cursor), and the mail line on a person's
// direct chat (find#DirectChatMail: their latest thread by their user account's address only, in mail the user may read,
// Spam, Trash and drafts left out, no line for a person with no direct chat, nothing for a stranger).
// Run: ./moq test basic-comms --spec BasicCommsDesktop
const CHAT = "org.moqui.chat.ChatServices."
const SCREEN = "org.moqui.comms.ChatScreenServices."
const A = "CHAT_DESKTOP_SPEC_A"
const B = "CHAT_DESKTOP_SPEC_B"
const C = "CHAT_DESKTOP_SPEC_C"
const D = "CHAT_DESKTOP_SPEC_D"
const P = "CHAT_DESKTOP_SPEC_P"
const S = "CHAT_DESKTOP_SPEC_S"
const X = "CHAT_DESKTOP_SPEC_X"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
const address = (userId) => userId.slice(-1).toLowerCase() + "@desktop-spec.example"
// a service as the screens call it, as a user (the service's own checks decide)
// the clock of the next calls, or the real one
let clock = null
function as(userId, name, parameters) {
    ec.user.internalLoginUser(username(userId))
    ec.user.setEffectiveTime(clock)
    let out = {}
    let thrown = ""
    try {
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : CHAT + name).parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
function store(entityName, fields) {
    sys(() => {
        const value = ec.entity.makeValue(entityName)
        value.setFields(fields, true, null, null)
        value.createOrUpdate()
    })
}
function makeUser(userId) {
    store("moqui.security.UserAccount", {userId, username: username(userId), userFullName: "Desktop " + userId.slice(-1),
        emailAddress: address(userId), disabled: "N"})
    sys(() => {
        if (ec.entity.find("moqui.security.UserGroup").condition("userGroupId", "BASIC_COMMS_USERS").count() == 0) return
        if (ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", "BASIC_COMMS_USERS").condition("userId", userId).count() > 0) return
        const member = ec.entity.makeValue("moqui.security.UserGroupMember")
        member.setFields({userGroupId: "BASIC_COMMS_USERS", userId, fromDate: ec.user.nowTimestamp}, true, null, null)
        member.create()
    })
}
// a mail account of its owner (no server) with an Inbox and a Trash
function account(id, ownerUserId) {
    store("moqui.basic.email.EmailAccount", {emailAccountId: id, ownerUserId, accountKindEnumId: "EakHosted", emailAddress: id.toLowerCase() + "@desktop-spec.example",
        accountName: id, statusId: "EasActive"})
    store("moqui.basic.email.EmailFolder", {emailFolderId: id + "_IN", emailAccountId: id, folderName: "Inbox", folderPath: "INBOX", folderKindEnumId: "EfkInbox", subscribed: "Y"})
    store("moqui.basic.email.EmailFolder", {emailFolderId: id + "_TR", emailAccountId: id, folderName: "Trash", folderPath: "Trash", folderKindEnumId: "EfkTrash", subscribed: "Y"})
}
function mail(id, accountId, folder, fields, hoursAgo) {
    const at = new Date(Date.now() - hoursAgo * 3600000)
    store("moqui.basic.email.EmailMessage", Object.assign({emailMessageId: id, rootEmailMessageId: id, emailAccountId: accountId,
        emailFolderId: folder ? accountId + "_" + folder : null, statusId: "ES_RECEIVED", messageDate: at, receivedDate: at, isRead: "N",
        subject: id}, fields))
}

spec({
    setupSpec() {
        for (const u of [A, B, C, D, P, S, X]) makeUser(u)
    }
})

feature("the palette's Chat rooms: a room by name, a phone room by its digits, no people; People to message: one with no direct chat (dm), not one with; a stranger finds no private room", {
    when() {
        const room = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Desktop spec room", memberUserIdList: [B]}).chatRoomId
        const direct = as(A, "create#Room", {roomTypeEnumId: "direct", memberUserIdList: [B]}).chatRoomId
        // a phone number's room (the SMS bridge makes them; here a room of A's made one)
        const phone = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Desktop spec number"}).chatRoomId
        sys(() => {
            const row = ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", phone).one()
            row.setFields({roomTypeEnumId: "CrtExternal", roomName: "+1 555 010 4477"}, true, null, null)
            row.update()
        })
        const byName = as(A, SCREEN + "search#PaletteRooms", {text: "Desktop", limit: 20}).rows || []
        const byNumber = as(A, SCREEN + "search#PaletteRooms", {text: "555 0104", limit: 20}).rows || []
        const stranger = as(S, SCREEN + "search#PaletteRooms", {text: "Desktop spec room", limit: 20}).rows || []
        const people = as(A, SCREEN + "search#PalettePeople", {text: "Desktop", limit: 20}).rows || []
        return {room, direct, phone, byName, byNumber, stranger, people}
    },
    then({room, direct, phone, byName, byNumber, stranger, people}) {
        byName.some((row) => row.title === "Desktop spec room" && row.url.endsWith("/Chat?chatRoomId=" + room))
        // B has a direct chat with A: the room, not a person row
        byName.some((row) => row.url.endsWith("/Chat?chatRoomId=" + direct) && row.title === "Desktop B")
        // the rooms group has no people: they are a group of their own, which never leads the palette
        !byName.some((row) => row.url.includes("dm="))
        !people.some((row) => row.url.includes("dm=" + B + "&"))
        // P and D have none: choosing one opens a message to them (dm); A is never offered to A
        people.some((row) => row.title === "Desktop P" && row.url.endsWith("/Chat?dm=" + P + "&dmName=Desktop%20P") && row.icon === "sym_o_person_add")
        people.some((row) => row.url.includes("dm=" + D + "&"))
        !people.some((row) => row.url.includes("dm=" + A + "&"))
        byNumber.some((row) => row.url.endsWith("/Chat?chatRoomId=" + phone) && row.icon === "sym_o_sms")
        !stranger.some((row) => row.url.includes("chatRoomId=" + room))
    }
})

feature("find#ChatThreads: the threads I started or replied in across rooms, mine last or alone too, the latest first by cursor; no other thread, nothing for a stranger", {
    when() {
        const room = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Desktop spec threads", memberUserIdList: [B, C]}).chatRoomId
        const other = as(C, "create#Room", {roomTypeEnumId: "private", roomName: "Desktop spec threads elsewhere", memberUserIdList: [B, A]}).chatRoomId
        const seq = (userId, roomId, text, root) => as(userId, "send#Message", {chatRoomId: roomId, body: `<p>${text}</p>`, threadRootEventSeq: root ?? null}).eventSeq
        // B's thread: C, then A, then B replies; A wrote in it, and B wrote last
        const bThread = seq(B, room, "B starts a thread")
        seq(C, room, "C replies", bThread)
        seq(A, room, "A replies", bThread)
        seq(B, room, "B replies last", bThread)
        // A's own thread, which only A replied in, a minute later
        clock = Date.now() + 60000
        const aThread = seq(A, other, "A starts")
        seq(A, other, "A replies to itself", aThread)
        // C's thread A is not in
        const cThread = seq(C, other, "C alone")
        seq(B, other, "B replies to C", cThread)
        clock = null
        const list = (userId, cursor, limit) => as(userId, SCREEN + "find#ChatThreads", {cursor, limit})
        const all = list(A).items || []
        const first = list(A, null, 1)
        const second = list(A, first.nextCursor, 1)
        return {room, other, bThread, aThread, cThread, all, first, second, stranger: list(S).items || []}
    },
    then({room, other, bThread, aThread, all, first, second, stranger}) {
        all.map((item) => item.chatRoomId + ":" + item.threadRootEventSeq).join(" ") == [other + ":" + aThread, room + ":" + bThread].join(" ")
        all[0].startedByMe && all[0].replierCount == 0 && all[0].lastSnippet == "A replies to itself"
        !all[1].startedByMe
        all[1].replyCount == 3
        all[1].lastUserName == "Desktop B"
        all[1].lastSnippet == "B replies last"
        all[1].rootSnippet == "B starts a thread"
        // the other people who replied: C and B
        all[1].replierCount == 2
        first.items.length == 1 && second.items.length == 1
        first.items[0].threadRootEventSeq == aThread
        second.items[0].threadRootEventSeq == bThread
        second.nextCursor == null
        stranger.length == 0
    }
})

feature("the mail line: a person's latest thread by their account's address in mail I may read, a shared account's too; an alias they own, Trash, drafts, a look-alike and an account not mine left out", {
    when() {
        const withB = as(A, "create#Room", {roomTypeEnumId: "direct", memberUserIdList: [B]}).chatRoomId
        const withC = as(A, "create#Room", {roomTypeEnumId: "direct", memberUserIdList: [C]}).chatRoomId
        account("DESKTOP_SPEC_MINE", A)
        account("DESKTOP_SPEC_SHARED", X)
        account("DESKTOP_SPEC_OTHER", X)
        store("moqui.basic.email.EmailAccountUser", {emailAccountId: "DESKTOP_SPEC_SHARED", userId: A, permissionEnumId: "EapRead",
            fromDate: new Date(Date.now() - 86400000)})
        mail("DESKTOP_SPEC_M_OLD", "DESKTOP_SPEC_MINE", "IN", {fromAddress: address(B), subject: "Older from B"}, 5)
        mail("DESKTOP_SPEC_M_SHARED", "DESKTOP_SPEC_SHARED", "IN", {fromAddress: "pat@elsewhere.example",
            toAddresses: `"Desktop B" <${address(B).toUpperCase()}>, other@elsewhere.example`, subject: "To B, in the shared account"}, 3)
        // the reply in that thread is the newest of it
        mail("DESKTOP_SPEC_M_REPLY", "DESKTOP_SPEC_SHARED", "IN", {rootEmailMessageId: "DESKTOP_SPEC_M_SHARED", fromAddress: "pat@elsewhere.example",
            ccAddresses: address(B), subject: "Re: To B, in the shared account"}, 2)
        mail("DESKTOP_SPEC_M_TRASH", "DESKTOP_SPEC_MINE", "TR", {fromAddress: address(B), subject: "In the Trash"}, 1)
        mail("DESKTOP_SPEC_M_DRAFT", "DESKTOP_SPEC_MINE", null, {statusId: "ES_DRAFT", toAddresses: address(B), subject: "A draft to B"}, 0.5)
        // an address that only has B's in it
        mail("DESKTOP_SPEC_M_LOOKALIKE", "DESKTOP_SPEC_MINE", "IN", {fromAddress: "x" + address(B), toAddresses: "x" + address(B), subject: "Not B"}, 0.2)
        mail("DESKTOP_SPEC_M_NOT_MINE", "DESKTOP_SPEC_OTHER", "IN", {fromAddress: address(C), subject: "C, in an account not shared with A"}, 1)
        mail("DESKTOP_SPEC_M_D", "DESKTOP_SPEC_MINE", "IN", {fromAddress: address(D), subject: "D has no direct chat with A"}, 1)
        // a mail account B owns (a shared box, an alias): its address is not B's to be credited with
        account("DESKTOP_SPEC_B_ALIAS", B)
        mail("DESKTOP_SPEC_M_ALIAS", "DESKTOP_SPEC_MINE", "IN", {fromAddress: "desktop_spec_b_alias@desktop-spec.example", subject: "From B's alias"}, 0.1)
        return {withB, withC, mine: as(A, SCREEN + "find#DirectChatMail").mailList || [], stranger: as(S, SCREEN + "find#DirectChatMail"),
            partner: as(B, SCREEN + "find#DirectChatMail").mailList || []}
    },
    then({withB, withC, mine, stranger, partner}) {
        mine.length == 1
        mine[0].chatRoomId == withB
        mine[0].userId == B
        mine[0].subject == "Re: To B, in the shared account"
        mine[0].threadKey == "email:DESKTOP_SPEC_M_SHARED"
        mine[0].url.endsWith("/Inbox?view=all&thread=email%3ADESKTOP_SPEC_M_SHARED")
        !mine.some((line) => line.chatRoomId === withC)
        stranger.errors == ""
        stranger.mailList.length == 0
        // B reads none of A's mail
        partner.length == 0
    }
})
