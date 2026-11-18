// View-once media (plans/Comms-Messaging.md §18.3 phase 16; ChatExpiry.js, ChatExpiryServices): what send#Message takes as
// view-once (one photo or video, no text, not in a bridged room); get#Timeline gives no file; open#ViewOnce serves the file in
// a window to a recipient, never to its sender, and once only; get#Attachment never serves it; the sender's Opened; the file
// gone once everyone opened it, the message standing; and one nobody opened gone after chat@view-once-days.
// Run: ./moq test framework --spec ChatViewOnce
const A = "CHAT_ONCE_A"
const B = "CHAT_ONCE_B"
const USERS = [A, B]
const CHAT = "org.moqui.chat.ChatServices."
const EXPIRY = "org.moqui.chat.ChatExpiryServices."
const GROUP = "CHAT_ONCE_SPEC"
const MINUTE = 60000
const DAY = 86400000

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
        for (const userId of USERS) make("moqui.security.UserAccount", {userId, username: username(userId), userFullName: "Once " + userId.slice(10), disabled: "N"})
        make("moqui.security.UserGroup", {userGroupId: GROUP, description: "Chat view-once spec"})
        make("moqui.security.ArtifactGroup", {artifactGroupId: GROUP, description: "Chat view-once spec"})
        make("moqui.security.ArtifactGroupMember", {artifactGroupId: GROUP, artifactName: "org\\.moqui\\.chat\\..*", artifactTypeEnumId: "AT_SERVICE",
            nameIsPattern: "Y", inheritAuthz: "Y"})
        make("moqui.security.ArtifactAuthz", {artifactAuthzId: GROUP, userGroupId: GROUP, artifactGroupId: GROUP, authzTypeEnumId: "AUTHZT_ALWAYS",
            authzActionEnumId: "AUTHZA_ALL"})
        for (const userId of USERS) make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId, fromDate: new Date(Date.now() - 60000)})
    })
}
// a file the user uploaded to the room, not yet sent, with stored bytes
let fileCount = 0
function file(userId, roomId, fileName, contentType) {
    fileCount++
    return sys(() => {
        const location = "dbresource://chat-once-spec/" + fileCount + "/" + fileName
        ec.resource.getLocationReference(location).putText("not really a picture")
        const row = ec.entity.makeValue("moqui.chat.ChatAttachment")
        row.setSequencedIdPrimary()
        row.setFields({chatRoomId: roomId, location, fileName, contentType, fileSize: 20, uploadedByUserId: userId, uploadedDate: ec.user.nowTimestamp}, true, null, null)
        row.create()
        return row.chatAttachmentId
    })
}
const message = (userId, roomId, seq) => Array.from(as(userId, CHAT + "get#Timeline", {chatRoomId: roomId, eventSeq: seq}).eventList || [])[0] || null
const expireAt = (millis) => sys(() => ec.service.sync().name(EXPIRY + "expire#DueMessages").parameters({nowMillis: millis}).call() || {})
const shared = {}

spec({
    stepwise: true,
    setupSpec() { setup() },
    cleanupSpec() { ec.user.logoutUser() }
})

feature("a view-once message is one photo or video with no text, in a room that is not bridged, and its file is not in the timeline", {
    when() {
        shared.room = as(A, CHAT + "create#Room", {roomTypeEnumId: "CrtDirect", memberUserIdList: [B]}).chatRoomId
        const withText = as(A, CHAT + "send#Message", {chatRoomId: shared.room, body: "<p>look</p>", viewOnce: true,
            chatAttachmentIdList: [file(A, shared.room, "a.png", "image/png")]})
        const pdf = as(A, CHAT + "send#Message", {chatRoomId: shared.room, viewOnce: true, chatAttachmentIdList: [file(A, shared.room, "a.pdf", "application/pdf")]})
        const two = as(A, CHAT + "send#Message", {chatRoomId: shared.room, viewOnce: true,
            chatAttachmentIdList: [file(A, shared.room, "b.png", "image/png"), file(A, shared.room, "c.png", "image/png")]})
        const bridged = as(A, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Once bridged", memberUserIdList: [B]}).chatRoomId
        sys(() => {
            ec.entity.makeValue("moqui.chat.ChatBridge").setFields({chatBridgeId: "CHAT_ONCE_BRIDGE", name: "Once bridge"}, true, null, null).createOrUpdate()
            ec.entity.makeValue("moqui.chat.ChatRoomBridge").setFields({chatRoomBridgeId: "CHAT_ONCE_MAP", chatRoomId: bridged, chatBridgeId: "CHAT_ONCE_BRIDGE",
                externalRoomId: "elsewhere", directionEnumId: "CrbBoth", fromDate: ec.user.nowTimestamp}, true, null, null).createOrUpdate()
        })
        const onBridge = as(A, CHAT + "send#Message", {chatRoomId: bridged, viewOnce: true, chatAttachmentIdList: [file(A, bridged, "d.png", "image/png")]})
        shared.photo = file(A, shared.room, "photo.jpg", "image/jpeg")
        const before = Date.now()
        const sent = as(A, CHAT + "send#Message", {chatRoomId: shared.room, viewOnce: true, chatAttachmentIdList: [shared.photo]})
        shared.seq = sent.eventSeq
        const seen = message(B, shared.room, sent.eventSeq)
        const edit = as(A, CHAT + "edit#Message", {chatRoomId: shared.room, eventSeq: sent.eventSeq, body: "<p>now with text</p>"})
        return {withText, pdf, two, onBridge, sent, seen, before, edit}
    },
    then({withText, pdf, two, onBridge, sent, seen, before, edit}) {
        withText.reasonCode == "viewOnceFiles"
        pdf.reasonCode == "viewOnceFiles"
        two.reasonCode == "viewOnceFiles"
        onBridge.reasonCode == "bridged"
        sent.errors == ""
        sent.event.attachments.length == 0
        sent.event.viewOnce.kind == "photo"
        seen.attachments.length == 0
        seen.viewOnce.kind == "photo"
        seen.viewOnce.openedByMe == false
        seen.expireDate >= before + 14 * DAY - MINUTE
        edit.errors != ""
    }
})

feature("a recipient opens it once, in a window, and the sender never gets the file", {
    when() {
        const bySender = as(A, EXPIRY + "open#ViewOnce", {chatRoomId: shared.room, eventSeq: shared.seq})
        const plainFile = as(B, CHAT + "get#Attachment", {chatAttachmentId: shared.photo})
        const opened = as(B, EXPIRY + "open#ViewOnce", {chatRoomId: shared.room, eventSeq: shared.seq})
        const file = as(B, EXPIRY + "get#ViewOnceFile", {chatAttachmentId: shared.photo})
        const senderFile = as(A, EXPIRY + "get#ViewOnceFile", {chatAttachmentId: shared.photo})
        const late = as(B, EXPIRY + "get#ViewOnceFile", {chatAttachmentId: shared.photo, nowMillis: Date.now() + 3 * MINUTE})
        const again = as(B, EXPIRY + "open#ViewOnce", {chatRoomId: shared.room, eventSeq: shared.seq, nowMillis: Date.now() + 3 * MINUTE})
        const mine = message(B, shared.room, shared.seq)
        const sender = message(A, shared.room, shared.seq)
        return {bySender, plainFile, opened, file, senderFile, late, again, mine, sender}
    },
    then({bySender, plainFile, opened, file, senderFile, late, again, mine, sender}) {
        bySender.reasonCode == "sender"
        plainFile.errors.includes("not found")
        opened.errors == ""
        opened.chatAttachmentId == shared.photo
        opened.until > Date.now()
        file.location.endsWith("photo.jpg")
        senderFile.errors.includes("not found")
        late.errors.includes("not found")
        // after the window it is refused (everyone opened it, so the message is due then too)
        again.errors != ""
        mine.viewOnce.openedByMe == true
        sender.viewOnce.openedCount == 1
        sender.viewOnce.recipientCount == 1
    }
})

feature("once everyone opened it its file goes and it stands as Opened; one nobody opened goes after 14 days", {
    when() {
        expireAt(Date.now() + 3 * MINUTE)
        const files = sys(() => ec.entity.find("moqui.chat.ChatAttachment").condition("chatAttachmentId", shared.photo).count())
        const stands = message(A, shared.room, shared.seq)
        const unopened = as(A, CHAT + "send#Message", {chatRoomId: shared.room, viewOnce: true,
            chatAttachmentIdList: [file(A, shared.room, "clip.mp4", "video/mp4")]})
        const video = message(B, shared.room, unopened.eventSeq)
        expireAt(Date.now() + 15 * DAY)
        const goneVideo = message(B, shared.room, unopened.eventSeq)
        const goneFirst = message(A, shared.room, shared.seq)
        return {files, stands, video, goneVideo, goneFirst}
    },
    then({files, stands, video, goneVideo, goneFirst}) {
        files == 0
        stands != null
        stands.viewOnce.gone == true
        stands.viewOnce.openedCount == 1
        video.viewOnce.kind == "video"
        goneVideo == null
        goneFirst == null
    }
})

feature("Opened shows only when read receipts are on on both sides", {
    when() {
        const pref = (userId, value) => sys(() => {
            const find = ec.entity.find("moqui.security.UserPreference").condition({userId, preferenceKey: "CHAT_READ_RECEIPTS"})
            if (value == null) find.deleteAll()
            else ec.entity.makeValue("moqui.security.UserPreference").setFields({userId, preferenceKey: "CHAT_READ_RECEIPTS", preferenceValue: value}, true, null, null).createOrUpdate()
        })
        const seq = as(A, CHAT + "send#Message", {chatRoomId: shared.room, viewOnce: true, chatAttachmentIdList: [file(A, shared.room, "q.png", "image/png")]}).eventSeq
        pref(B, "N")
        as(B, EXPIRY + "open#ViewOnce", {chatRoomId: shared.room, eventSeq: seq})
        const theirsOff = message(A, shared.room, seq).viewOnce.openedCount
        pref(B, null)
        pref(A, "N")
        const mineOff = message(A, shared.room, seq).viewOnce.openedCount
        pref(A, null)
        const bothOn = message(A, shared.room, seq).viewOnce.openedCount
        return {theirsOff, mineOff, bothOn}
    },
    then({theirsOff, mineOff, bothOn}) {
        theirsOff == 0
        mineOff == 0
        bothOn == 1
    }
})
