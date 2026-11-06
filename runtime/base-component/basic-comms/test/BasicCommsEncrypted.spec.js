// Encrypted chat rooms on the server (plans/Comms-Framework.md §4.5, C7; docs/interfaces/Chat-Keys.md "Encrypted rooms"):
// a room made encrypted takes only Megolm ciphertext and keeps no plaintext (no bodyText, snippet, search document, link
// preview or notification summary), with the relation targets and the mentioned users in the clear; and it refuses what
// needs the plaintext, each refusal with the very same valid parameters that pass in a plain room: a plaintext send, edit
// or reaction key, a forward in or out, records, polls, agents, search, a bridge, a public or record room, Create from and
// File on record, and the terminal's send. No crypto runs here: the ciphertext is an opaque string, as the server treats
// it (the browsers' encryption is the client spec EncryptedChat).
// Run: ./moq test basic-comms --spec BasicCommsEncrypted
const root = "component://basic-comms/screen/BasicComms.xml"
const CHAT = "org.moqui.impl.ChatServices."
const A = "CHAT_E2EE_SPEC_A"
const B = "CHAT_E2EE_SPEC_B"
const C = "CHAT_E2EE_SPEC_C"
const SECRET = "Plaintext that never reaches the server"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
// a service as the screens call it, as a user (the service's own checks decide)
function as(userId, name, parameters) {
    if (userId) ec.user.internalLoginUser(userId.includes(".") ? userId : username(userId))
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
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "E2EE " + userId.slice(-1), disabled: "N"}, true, null, null)
            account.create()
        }
        // BasicComms is opened by BASIC_COMMS_USERS, not ALL_USERS; IronStone's Comms, when installed, by BOSS_USERS
        for (const group of ["BASIC_COMMS_USERS", "BOSS_USERS"]) {
            if (ec.entity.find("moqui.security.UserGroup").condition("userGroupId", group).count() == 0) continue
            const found = ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", group)
            if (found.condition("userId", userId).count() > 0) continue
            const member = ec.entity.makeValue("moqui.security.UserGroupMember")
            member.setFields({userGroupId: group, userId, fromDate: ec.user.nowTimestamp}, true, null, null)
            member.create()
        }
    })
}
function json(path, parameters) {
    const str = ec.screen.makeTest().rootScreen(root).render("Chat/" + path, parameters || {}, null)
    ec.message.clearErrors()
    try { return JSON.parse(String(str.output || "")) } catch (err) { return null }
}
function tui(path, parameters) {
    const str = ec.screen.makeTest().rootScreen(root).renderMode("tui").render(path, Object.assign({lastStandalone: "-2"}, parameters || {}), null)
    ec.message.clearErrors()
    return String(str.output || "")
}
/** A Megolm ciphertext as a browser's OlmMachine makes one (opaque here). */
const sealed = (tag) => ({algorithm: "m.megolm.v1.aes-sha2", ciphertext: "AwgAEn" + tag, session_id: "session-" + tag,
    sender_key: "curve-" + tag, device_id: "DEVICEAAAA"})
const row = (roomId, seq) => sys(() => ec.entity.find("moqui.chat.ChatEvent").condition({chatRoomId: roomId, eventSeq: seq}).one())
const roomRow = (roomId) => sys(() => ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", roomId).one())

let plain = null
let secret = null

spec({
    setupSpec() {
        for (const u of [A, B, C]) makeUser(u)
        plain = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "E2EE spec plain", memberUserIdList: [B]}).chatRoomId
        secret = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "E2EE spec encrypted", memberUserIdList: [B], encrypted: true}).chatRoomId
    }
})

feature("an encrypted room is direct or private, set at creation and members only; its own direct room beside the plain one", {
    when() {
        const room = as(A, "get#Room", {chatRoomId: secret}).room
        const plainRoom = as(A, "get#Room", {chatRoomId: plain}).room
        const publicRoom = as(A, "create#Room", {roomTypeEnumId: "public", roomName: "E2EE spec public", encrypted: true})
        const publicPlain = as(A, "create#Room", {roomTypeEnumId: "public", roomName: "E2EE spec public", encrypted: false})
        const recordRoom = as(A, "create#Room", {roomTypeEnumId: "record", recordEntityName: "moqui.security.UserAccount", recordId: A, encrypted: true})
        const directPlain = as(A, "create#Room", {roomTypeEnumId: "direct", memberUserIdList: [B]})
        const directSealed = as(A, "create#Room", {roomTypeEnumId: "direct", memberUserIdList: [B], encrypted: true})
        const directAgain = as(A, "create#Room", {roomTypeEnumId: "direct", memberUserIdList: [B], encrypted: true})
        const toPublic = as(A, "update#Room", {chatRoomId: secret, visibilityEnumId: "CrvUsers"})
        const plainToPublic = as(A, "update#Room", {chatRoomId: plain, visibilityEnumId: "CrvUsers"})
        as(A, "update#Room", {chatRoomId: plain, visibilityEnumId: "CrvMembers"})
        if (publicPlain.chatRoomId) as(A, "update#Room", {chatRoomId: publicPlain.chatRoomId, archived: true})
        return {room, plainRoom, publicRoom, publicPlain, recordRoom, directPlain, directSealed, directAgain, toPublic, plainToPublic}
    },
    then({room, plainRoom, publicRoom, publicPlain, recordRoom, directPlain, directSealed, directAgain, toPublic, plainToPublic}) {
        room.encrypted == true
        plainRoom.encrypted == false
        publicRoom.reasonCode == "encryptedRoom"
        publicPlain.errors == ""
        recordRoom.reasonCode == "encryptedRoom"
        directSealed.errors == ""
        directSealed.chatRoomId != directPlain.chatRoomId
        directAgain.chatRoomId == directSealed.chatRoomId
        toPublic.errors == "An encrypted room is for its members only"
        plainToPublic.errors == ""
    }
})

feature("an encrypted message is kept as ciphertext only, with its mentions in the clear: no bodyText, snippet, search or summary", {
    when() {
        const capture = ec.factory.getTool("NotificationCapture")
        capture.enable()
        try {
            capture.clear()
            const sent = as(A, "send#Message", {chatRoomId: secret, encrypted: sealed("m1"), mentionUserIdList: [B, "NOBODY_HERE"]})
            const stored = row(secret, sent.eventSeq)
            const room = roomRow(secret)
            const list = as(B, "find#Rooms", {}).roomList.find((item) => item.chatRoomId === secret)
            const page = as(B, "get#Timeline", {chatRoomId: secret, eventSeq: sent.eventSeq}).eventList[0]
            const searched = as(B, "search#ChatEvents", {searchText: "AwgAEnm1"})
            const searchedRoom = as(B, "search#ChatEvents", {searchText: "AwgAEnm1", chatRoomId: secret})
            const mentions = sys(() => ec.entity.find("moqui.chat.ChatEventMention").condition({chatRoomId: secret, eventSeq: sent.eventSeq}).list())
                    .map((m) => m.userId)
            const notices = capture.getMessages("ChatMention").map((m) => typeof m.message === "string" ? m.message : JSON.stringify(m.message))
            return {sent, stored, room, list, page, searched, searchedRoom, mentions, notices}
        } finally {
            capture.disable()
            capture.clear()
        }
    },
    then({sent, stored, room, list, page, searched, searchedRoom, mentions, notices}) {
        sent.errors == ""
        stored.encrypted == "Y"
        stored.bodyText == null
        JSON.parse(stored.contentJson).body == null
        JSON.parse(stored.contentJson).encrypted.session_id == "session-m1"
        room.lastMessageSnippet == null
        Number(room.lastMessageSeq) == sent.eventSeq
        list.encrypted == true
        list.lastMessage.encrypted == true
        list.lastMessage.redacted == false
        list.mentionCount == 1
        page.body == null
        page.bodyText == null
        page.encrypted.ciphertext == "AwgAEnm1"
        page.encryptedEventSeq == sent.eventSeq
        mentions.join(",") == B
        searched.eventList.length == 0
        searchedRoom.reasonCode == "encryptedRoom"
        notices.every((text) => text.indexOf("summary") < 0 && text.indexOf("AwgAEn") < 0)
    }
})

feature("a plaintext body, a forward in or out and records are refused in an encrypted room, as they pass in a plain one", {
    when() {
        const body = "<p>" + SECRET + "</p>"
        const plainSend = as(A, "send#Message", {chatRoomId: plain, body})
        const sealedSend = as(A, "send#Message", {chatRoomId: secret, body})
        const cipherInPlain = as(A, "send#Message", {chatRoomId: plain, encrypted: sealed("x")})
        const noCipher = as(A, "send#Message", {chatRoomId: secret, encrypted: {algorithm: "m.olm.v1.curve25519-aes-sha2", ciphertext: "x"}})
        const forwardIn = as(A, "send#Message", {chatRoomId: secret, encrypted: sealed("f"), forwardOfChatRoomId: plain, forwardOfEventSeq: plainSend.eventSeq})
        const forwardPlain = as(A, "send#Message", {chatRoomId: plain, forwardOfChatRoomId: plain, forwardOfEventSeq: plainSend.eventSeq})
        const lastSealed = Number(roomRow(secret).lastMessageSeq)
        const forwardOut = as(A, "send#Message", {chatRoomId: plain, forwardOfChatRoomId: secret, forwardOfEventSeq: lastSealed})
        const recordsPlain = as(A, "send#Message", {chatRoomId: plain, body, recordList: [{entity: "moqui.security.UserAccount", id: A}]})
        const records = as(A, "send#Message", {chatRoomId: secret, encrypted: sealed("r"), recordList: [{entity: "moqui.security.UserAccount", id: A}]})
        const leaked = sys(() => ec.entity.find("moqui.chat.ChatEvent").condition("chatRoomId", secret).list())
                .some((event) => String(event.contentJson || "").indexOf(SECRET) >= 0 || String(event.bodyText || "").indexOf(SECRET) >= 0)
        return {plainSend, sealedSend, cipherInPlain, noCipher, forwardIn, forwardPlain, forwardOut, recordsPlain, records, leaked}
    },
    then({plainSend, sealedSend, cipherInPlain, noCipher, forwardIn, forwardPlain, forwardOut, recordsPlain, records, leaked}) {
        plainSend.errors == ""
        sealedSend.reasonCode == "encryptedRoom"
        cipherInPlain.reasonCode == "notEncrypted"
        noCipher.reasonCode == "encryptedRoom"
        forwardIn.reasonCode == "encryptedRoom"
        forwardPlain.errors == ""
        forwardOut.reasonCode == "encryptedRoom"
        recordsPlain.errors == ""
        records.reasonCode == "encryptedRoom"
        leaked == false
    }
})

feature("an edit and a reaction are ciphertext too: the timeline gives the edit's ciphertext and the reactions to decrypt", {
    when() {
        const sent = as(A, "send#Message", {chatRoomId: secret, encrypted: sealed("e0")})
        const plainEdit = as(A, "edit#Message", {chatRoomId: secret, eventSeq: sent.eventSeq, body: "<p>" + SECRET + "</p>"})
        const edit = as(A, "edit#Message", {chatRoomId: secret, eventSeq: sent.eventSeq, encrypted: sealed("e1"), mentionUserIdList: [B]})
        const plainSent = as(A, "send#Message", {chatRoomId: plain, body: "<p>edit me</p>"})
        const cipherEditPlain = as(A, "edit#Message", {chatRoomId: plain, eventSeq: plainSent.eventSeq, encrypted: sealed("e1")})
        const plainKey = as(B, "react#Message", {chatRoomId: secret, eventSeq: sent.eventSeq, key: "👍"})
        const plainKeyPlain = as(B, "react#Message", {chatRoomId: plain, eventSeq: plainSent.eventSeq, key: "👍"})
        const reaction = as(B, "react#Message", {chatRoomId: secret, eventSeq: sent.eventSeq, encrypted: sealed("k1")})
        const second = as(B, "react#Message", {chatRoomId: secret, eventSeq: sent.eventSeq, encrypted: sealed("k2")})
        const othersBack = as(A, "react#Message", {chatRoomId: secret, eventSeq: sent.eventSeq, reactionEventSeq: second.reactionEventSeq})
        const takeBack = as(B, "react#Message", {chatRoomId: secret, eventSeq: sent.eventSeq, reactionEventSeq: second.reactionEventSeq})
        const page = as(A, "get#Timeline", {chatRoomId: secret, eventSeq: sent.eventSeq}).eventList[0]
        const original = row(secret, sent.eventSeq)
        return {plainEdit, edit, cipherEditPlain, plainKey, plainKeyPlain, reaction, othersBack, takeBack, page, original}
    },
    then({plainEdit, edit, cipherEditPlain, plainKey, plainKeyPlain, reaction, othersBack, takeBack, page, original}) {
        plainEdit.reasonCode == "encryptedRoom"
        edit.errors == ""
        cipherEditPlain.reasonCode == "notEncrypted"
        plainKey.reasonCode == "encryptedRoom"
        plainKeyPlain.errors == ""
        reaction.added == true
        othersBack.errors.indexOf("not found") >= 0
        takeBack.errors == ""
        original.bodyText == null
        page.edited == true
        page.encrypted.ciphertext == "AwgAEne1"
        page.encryptedEventSeq == edit.editEventSeq
        page.reactions.length == 0
        page.encryptedReactions.length == 1
        page.encryptedReactions[0].eventSeq == reaction.reactionEventSeq
        page.encryptedReactions[0].senderUserId == B
    }
})

feature("polls, agents, link previews and bridges are refused in an encrypted room, as they pass in a plain one", {
    when() {
        const poll = {question: "Lunch?", optionList: ["Soup", "Salad"]}
        const pollPlain = as(A, "create#Poll", Object.assign({chatRoomId: plain}, poll))
        const pollSealed = as(A, "create#Poll", Object.assign({chatRoomId: secret}, poll))
        const agentSealed = as(A, "add#Member", {chatRoomId: secret, userIdList: [B], roleEnumId: "CmrAgent"})
        const agentRole = as(A, "set#MemberRole", {chatRoomId: secret, userId: B, roleEnumId: "CmrAgent"})
        const files = as(B, "find#RoomFiles", {chatRoomId: secret})
        // a bridge: an administrator maps a plain room, and is refused the encrypted one
        ec.user.internalLoginUser("john.doe")
        const bridge = as(null, "org.moqui.impl.ChatBridgeServices.store#Bridge", {name: "E2EE spec line", typeEnumId: "CbtSms",
            adapterName: "spec-missing", inboundToken: "e2ee-spec-token"}).chatBridgeId
        const johnPlain = as(null, "create#Room", {roomTypeEnumId: "private", roomName: "E2EE spec bridged"}).chatRoomId
        const johnSealed = as(null, "create#Room", {roomTypeEnumId: "private", roomName: "E2EE spec bridged", encrypted: true}).chatRoomId
        const mapPlain = as(null, "org.moqui.impl.ChatBridgeServices.store#RoomBridge", {chatRoomId: johnPlain, chatBridgeId: bridge, externalRoomId: "+15550199"})
        const mapSealed = as(null, "org.moqui.impl.ChatBridgeServices.store#RoomBridge", {chatRoomId: johnSealed, chatBridgeId: bridge, externalRoomId: "+15550198"})
        return {pollPlain, pollSealed, agentSealed, agentRole, files, mapPlain, mapSealed}
    },
    then({pollPlain, pollSealed, agentSealed, agentRole, files, mapPlain, mapSealed}) {
        pollPlain.errors == ""
        pollSealed.reasonCode == "encryptedRoom"
        agentSealed.errors == "An agent cannot read an encrypted room"
        agentRole.errors == "An agent cannot read an encrypted room"
        files.errors == ""
        mapPlain.errors == ""
        mapSealed.reasonCode == "encryptedRoom"
    }
})

feature("Create from, File on record and the terminal's send refuse an encrypted room's message; the terminal shows Encrypted message", {
    when() {
        ec.user.internalLoginUser(username(A))
        const last = Number(roomRow(secret).lastMessageSeq)
        const made = json("createFrom", {chatRoomId: secret, eventSeq: last, kind: "task"})
        const filed = json("fileOnRecord", {chatRoomId: secret, eventSeq: last, recordEntityName: "moqui.security.UserAccount", recordId: A})
        const before = Number(roomRow(secret).lastEventSeq)
        ec.screen.makeTest().rootScreen(root).renderMode("tui").render("ChatTui/send", {chatRoomId: secret, bodyText: SECRET}, null)
        ec.message.clearErrors()
        const after = Number(roomRow(secret).lastEventSeq)
        const page = tui("ChatTui", {chatRoomId: secret})
        const list = tui("ChatTui", {})
        return {made, filed, before, after, page, list}
    },
    then({made, filed, before, after, page, list}) {
        made.reasonCode == "encryptedRoom"
        filed.reasonCode == "encryptedRoom"
        after == before
        page.indexOf("Encrypted message") >= 0
        page.indexOf("open it in a browser") >= 0
        page.indexOf(SECRET) < 0
        list.indexOf("Encrypted message") >= 0
    }
})

// One who is not in the room learns nothing of it: the same calls a member is refused with encryptedRoom answer the outsider
// as for any room it cannot see (not found), never that the room is encrypted.
feature("an outsider sending the same reaction, Create from or File on record to an encrypted room hears only that it is not found", {
    when() {
        const sent = as(A, "send#Message", {chatRoomId: secret, encrypted: sealed("o1")})
        const memberKey = as(B, "react#Message", {chatRoomId: secret, eventSeq: sent.eventSeq, key: "👍"})
        const outsiderKey = as(C, "react#Message", {chatRoomId: secret, eventSeq: sent.eventSeq, key: "👍"})
        const outsiderSealed = as(C, "react#Message", {chatRoomId: secret, eventSeq: sent.eventSeq, encrypted: sealed("o2")})
        ec.user.internalLoginUser(username(C))
        const made = json("createFrom", {chatRoomId: secret, eventSeq: sent.eventSeq, kind: "task"})
        const filed = json("fileOnRecord", {chatRoomId: secret, eventSeq: sent.eventSeq, recordEntityName: "moqui.security.UserAccount", recordId: C})
        const before = Number(roomRow(secret).lastEventSeq)
        ec.screen.makeTest().rootScreen(root).renderMode("tui").render("ChatTui/send", {chatRoomId: secret, bodyText: SECRET}, null)
        const tuiErrors = ec.message.hasError() ? ec.message.errorsString : ""
        ec.message.clearErrors()
        return {memberKey, outsiderKey, outsiderSealed, made, filed, tuiErrors, before, after: Number(roomRow(secret).lastEventSeq)}
    },
    then({memberKey, outsiderKey, outsiderSealed, made, filed, tuiErrors, before, after}) {
        memberKey.reasonCode == "encryptedRoom"
        outsiderKey.reasonCode == null
        outsiderKey.errors.indexOf("not found") >= 0
        outsiderSealed.reasonCode == null
        outsiderSealed.errors.indexOf("not found") >= 0
        made == null || made.reasonCode != "encryptedRoom"
        filed == null || filed.reasonCode != "encryptedRoom"
        tuiErrors.indexOf("encrypted") < 0
        after == before
    }
})
