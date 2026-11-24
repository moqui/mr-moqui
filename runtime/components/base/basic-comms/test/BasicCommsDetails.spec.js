// The lists of a room's details tabs and the Activity feed (plans/Comms-Messaging.md §5.2): a message's links kept for the
// Links list (ChatEventLink: written on send, replaced on edit, removed by a redaction and the retention), find#RoomLinks,
// find#RoomFiles's types to keep and to leave out, find#RoomReactions and find#ChatActivity. Each by the one paging rule (a
// page cut only between messages, a message with more items than limit whole, the cursor and not the count saying there is
// more, equal dates kept in order), for a user who may read the room only, and with no text of an encrypted room. The
// terminal's Links and Activity lists. The files are rows only (no stored file): nothing here opens one.
// Run: ./moq test basic-comms --spec BasicCommsDetails
const root = "component://basic-comms/screen/BasicComms.xml"
const CHAT = "org.moqui.chat.ChatServices."
const A = "CHAT_DETAILS_SPEC_A"
const B = "CHAT_DETAILS_SPEC_B"
const C = "CHAT_DETAILS_SPEC_C"
const D = "CHAT_DETAILS_SPEC_D"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
// the clock of the next calls (equal dates), or the real one
let clock = null
// a service as the screens call it, as a user (the service's own checks decide)
function as(userId, name, parameters) {
    if (userId) ec.user.internalLoginUser(username(userId))
    if (clock != null) ec.user.setEffectiveTime(clock)
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
            account.setFields({userId, username: username(userId), userFullName: "Details " + userId.slice(-1), disabled: "N"}, true, null, null)
            account.create()
        }
        for (const group of ["BASIC_COMMS_USERS", "BOSS_USERS"]) {
            if (ec.entity.find("moqui.security.UserGroup").condition("userGroupId", group).count() == 0) continue
            if (ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", group).condition("userId", userId).count() > 0) continue
            const member = ec.entity.makeValue("moqui.security.UserGroupMember")
            member.setFields({userGroupId: group, userId, fromDate: ec.user.nowTimestamp}, true, null, null)
            member.create()
        }
    })
}
// a file the user uploaded to the room, not yet sent (no stored file: the lists read the row only)
let fileCount = 0
function file(userId, roomId, fileName, contentType) {
    fileCount++
    return sys(() => {
        const row = ec.entity.makeValue("moqui.chat.ChatAttachment")
        row.setFields({chatAttachmentId: "CHAT_DETAILS_F" + fileCount, chatRoomId: roomId, location: "dbresource://chat-details-spec/" + fileCount,
            fileName, contentType, fileSize: 1000 + fileCount, uploadedByUserId: userId, uploadedDate: ec.user.nowTimestamp}, true, null, null)
        row.create()
        return row.chatAttachmentId
    })
}
// a preview kept in the cache, so a message's links are not fetched (get#LinkPreview reads the cache first)
function cached(url, title, siteName) {
    sys(() => {
        const urlHash = ec.service.sync().name(CHAT + "extract#LinkUrls").parameters({body: url, listed: true}).call().urlList[0].urlHash
        const row = ec.entity.makeValue("moqui.chat.ChatLinkPreview")
        row.setFields({urlHash, url, title, siteName, failed: "N", fetchedDate: ec.user.nowTimestamp}, true, null, null)
        row.createOrUpdate()
    })
}
const linkRows = (roomId, seq) => sys(() => ec.entity.find("moqui.chat.ChatEventLink").condition({chatRoomId: roomId, eventSeq: seq}).orderBy("linkSeq").list())
    .map((row) => row.url)
const sealed = (tag) => ({algorithm: "m.megolm.v1.aes-sha2", ciphertext: "AwgAEn" + tag, session_id: "session-" + tag, sender_key: "curve-" + tag, device_id: "DEVICEAAAA"})
const mention = (userId) => `<span data-mention-id="${userId}">@${userId}</span>`
function tui(parameters) {
    const out = ec.screen.makeTest().rootScreen(root).renderMode("tui").render("ChatTui", Object.assign({lastStandalone: "-2"}, parameters), null)
    ec.message.clearErrors()
    return String(out.output || "")
}
// every page of a list, by its cursor: the items in order, and the page sizes
function allPages(read, cursorOf, itemsOf) {
    const items = []
    const sizes = []
    let cursor = null
    for (let page = 0; page < 20; page++) {
        const out = read(cursor)
        items.push(...itemsOf(out))
        sizes.push(itemsOf(out).length)
        cursor = cursorOf(out)
        if (cursor == null) break
    }
    return {items, sizes}
}

let plain = null
let secret = null

spec({
    setupSpec() {
        for (const u of [A, B, C, D]) makeUser(u)
        plain = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Details spec", memberUserIdList: [B]}).chatRoomId
        secret = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Details spec encrypted", memberUserIdList: [B], encrypted: true}).chatRoomId
        cached("https://example.com/guide", "The guide", "Example")
    },
    cleanupSpec() {
        clock = null
        ec.user.setEffectiveTime(null)
    }
})

feature("a message's links are kept for its room's Links list: on send, replaced on an edit, gone with a redaction and the retention", {
    when() {
        const sent = as(A, "send#Message", {chatRoomId: plain,
            body: '<p>See <a href="https://example.com/guide">the guide</a> and http://Example.org/page, not https://user:pw@example.net/x</p>'})
        const onSend = linkRows(plain, sent.eventSeq)
        // every link is listed, past the preview's three, on any port and our own (extract#LinkUrls listed)
        const many = as(A, "send#Message", {chatRoomId: plain, body: "<p>https://one.example/ https://two.example/ https://three.example/ "
            + "https://four.example/ https://example.com:8443/odd https://erp.example.com/qapps/Mail/Chat?chatRoomId=1</p>"})
        const listed = linkRows(plain, many.eventSeq)
        const forward = as(B, "send#Message", {chatRoomId: plain, body: "<p>also https://example.edu/more</p>", forwardOfChatRoomId: plain,
            forwardOfEventSeq: sent.eventSeq})
        const forwarded = linkRows(plain, forward.eventSeq)
        as(A, "edit#Message", {chatRoomId: plain, eventSeq: sent.eventSeq, body: "<p>Now only https://example.com/guide</p>"})
        const onEdit = linkRows(plain, sent.eventSeq)
        const gone = as(A, "send#Message", {chatRoomId: plain, body: "<p>https://example.com/gone</p>"})
        as(A, "redact#Message", {chatRoomId: plain, eventSeq: gone.eventSeq})
        const onRedact = linkRows(plain, gone.eventSeq)
        const sealedSent = as(A, "send#Message", {chatRoomId: secret, encrypted: sealed("l1")})
        const onSealed = sys(() => ec.entity.find("moqui.chat.ChatEventLink").condition("chatRoomId", secret).count())
        // the retention: a message older than the room's days goes with its links
        const kept = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Details spec retention", memberUserIdList: [B], retentionDays: 1}).chatRoomId
        clock = Date.now() - 3 * 86400000
        const old = as(A, "send#Message", {chatRoomId: kept, body: "<p>https://example.com/old</p>"})
        clock = null
        ec.user.setEffectiveTime(null)
        const beforeRetention = linkRows(kept, old.eventSeq)
        as(null, "clean#RoomRetention", {chatRoomId: kept})
        const afterRetention = linkRows(kept, old.eventSeq)
        return {sent, onSend, listed, forwarded, onEdit, onRedact, sealedSent, onSealed, beforeRetention, afterRetention}
    },
    then({sent, onSend, listed, forwarded, onEdit, onRedact, sealedSent, onSealed, beforeRetention, afterRetention}) {
        sent.errors == ""
        onSend.join(" ") == "https://example.com/guide http://example.org/page"
        listed.join(" ") == "https://one.example/ https://two.example/ https://three.example/ https://four.example/ https://example.com:8443/odd https://erp.example.com/qapps/Mail/Chat?chatRoomId=1"
        forwarded.join(" ") == "https://example.edu/more https://example.com/guide http://example.org/page"
        onEdit.join(" ") == "https://example.com/guide"
        onRedact.length == 0
        sealedSent.errors == ""
        onSealed == 0
        beforeRetention.join(" ") == "https://example.com/old"
        afterRetention.length == 0
    }
})

feature("find#RoomLinks: newest first with the cached title, whole messages a page, by search; a stranger gets nothing, an encrypted room nothing", {
    when() {
        const room = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Details spec links", memberUserIdList: [B]}).chatRoomId
        const first = as(A, "send#Message", {chatRoomId: room, body: "<p>https://example.com/guide</p>"})
        const three = as(B, "send#Message", {chatRoomId: room, body: "<p>https://one.example/ https://two.example/ https://three.example/</p>"})
        const last = as(A, "send#Message", {chatRoomId: room, body: "<p>https://four.example/</p>"})
        const pages = allPages((cursor) => as(B, "find#RoomLinks", {chatRoomId: room, limit: 2, beforeEventSeq: cursor}),
            (out) => out.nextBeforeEventSeq ?? null, (out) => out.links || [])
        const searched = as(B, "find#RoomLinks", {chatRoomId: room, searchText: "guide"}).links
        const titled = as(B, "find#RoomLinks", {chatRoomId: room, searchText: "The gui"}).links
        const stranger = as(C, "find#RoomLinks", {chatRoomId: room})
        const sealedLinks = as(B, "find#RoomLinks", {chatRoomId: secret})
        return {first, three, last, pages, searched, titled, stranger, sealedLinks}
    },
    then({first, three, last, pages, searched, titled, stranger, sealedLinks}) {
        // a page of limit 2 takes the last message's link, then the message of three whole, then the first
        pages.sizes.join(",") == "1,3,1"
        pages.items.map((link) => link.eventSeq).join(",") == [last.eventSeq, three.eventSeq, three.eventSeq, three.eventSeq, first.eventSeq].join(",")
        pages.items.map((link) => link.url).slice(1, 4).join(" ") == "https://one.example/ https://two.example/ https://three.example/"
        pages.items[4].title == "The guide"
        pages.items[4].siteName == "Example"
        pages.items[4].senderUserId == A
        pages.items[1].title == null
        searched.length == 1
        titled.length == 1
        titled[0].url == "https://example.com/guide"
        stranger.errors.includes("not found")
        stranger.links.length == 0
        sealedLinks.errors == ""
        sealedLinks.links.length == 0
    }
})

feature("find#RoomFiles keeps a list of types and leaves out another: Media, Files and Voice; whole messages a page; an encrypted room sorts in the browser", {
    when() {
        const room = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Details spec files", memberUserIdList: [B]}).chatRoomId
        const photos = as(A, "send#Message", {chatRoomId: room, body: "<p>photos</p>", chatAttachmentIdList: [file(A, room, "a.png", "image/png"),
            file(A, room, "b.jpg", "image/jpeg"), file(A, room, "c.png", "image/png")]})
        const video = as(B, "send#Message", {chatRoomId: room, chatAttachmentIdList: [file(B, room, "clip.mp4", "video/mp4")]})
        const pdf = as(A, "send#Message", {chatRoomId: room, chatAttachmentIdList: [file(A, room, "plan.pdf", "application/pdf"), file(A, room, "blob", null)]})
        const voice = as(B, "send#Message", {chatRoomId: room, chatAttachmentIdList: [file(B, room, "Voice note.webm", "audio/webm")]})
        const names = (out) => (out.fileList || []).map((one) => one.fileName).join(" ")
        const media = as(B, "find#RoomFiles", {chatRoomId: room, contentTypePrefixList: ["image/", "video/"]})
        const files = as(B, "find#RoomFiles", {chatRoomId: room, excludeContentTypePrefixList: ["image/", "video/", "audio/"]})
        const voices = as(B, "find#RoomFiles", {chatRoomId: room, contentTypePrefixList: ["audio/"]})
        const one = as(B, "find#RoomFiles", {chatRoomId: room, contentTypePrefix: "image/"})
        const pages = allPages((cursor) => as(B, "find#RoomFiles", {chatRoomId: room, contentTypePrefixList: ["image/", "video/"], limit: 2, beforeEventSeq: cursor}),
            (out) => out.nextBeforeEventSeq ?? null, (out) => out.fileList || [])
        const stranger = as(C, "find#RoomFiles", {chatRoomId: room})
        // an encrypted room: no filter, and each file with its message's ciphertext
        const sealedFile = file(A, secret, "encrypted", "application/octet-stream")
        const sealedSent = as(A, "send#Message", {chatRoomId: secret, encrypted: sealed("f1"), chatAttachmentIdList: [sealedFile]})
        const sealedAll = as(B, "find#RoomFiles", {chatRoomId: secret})
        const sealedMedia = as(B, "find#RoomFiles", {chatRoomId: secret, contentTypePrefixList: ["image/"]})
        return {photos, video, pdf, voice, media: names(media), files: names(files), voices: names(voices), one: names(one), pages, stranger,
            sealedSent, sealedAll, sealedMedia}
    },
    then({photos, video, pdf, voice, media, files, voices, one, pages, stranger, sealedSent, sealedAll, sealedMedia}) {
        voice.errors == ""
        media == "clip.mp4 a.png b.jpg c.png"
        files == "plan.pdf blob"
        voices == "Voice note.webm"
        one == "a.png b.jpg c.png"
        pages.sizes.join(",") == "1,3"
        pages.items.map((one) => one.eventSeq).join(",") == [video.eventSeq, photos.eventSeq, photos.eventSeq, photos.eventSeq].join(",")
        stranger.errors.includes("not found")
        sealedSent.errors == ""
        sealedAll.fileList.length == 1
        sealedAll.fileList[0].encrypted == true
        sealedAll.fileList[0].fileName == null
        sealedAll.fileList[0].encryptedContent.ciphertext == "AwgAEnf1"
        sealedAll.fileList[0].encryptedEventSeq == sealedSent.eventSeq
        sealedMedia.fileList.length == 0
    }
})

feature("the attachment transition serves a voice note as a sound to play, by its name's type, never sniffed", {
    when() {
        const types = {}
        for (const name of ["Voice note.webm", "Voice note.m4a"]) {
            const location = "dbresource://chat-details-spec/" + name.slice(-4) + "/" + name
            sys(() => ec.resource.getLocationReference(location).putText("not really sound"))
            const id = file(A, plain, name, name.endsWith(".webm") ? "audio/webm" : "audio/mp4")
            sys(() => { const row = ec.entity.find("moqui.chat.ChatAttachment").condition("chatAttachmentId", id).one(); row.set("location", location); row.update() })
            as(A, "send#Message", {chatRoomId: plain, chatAttachmentIdList: [id]})
            ec.user.internalLoginUser(username(B))
            const out = ec.screen.makeTest().rootScreen(root).render("Chat/attachment", {chatAttachmentId: id, inline: "true"}, null)
            ec.message.clearErrors()
            types[name] = {type: out.header("Content-Type"), sniff: out.header("X-Content-Type-Options"), disposition: out.header("Content-Disposition") || ""}
        }
        return {types}
    },
    then({types}) {
        types["Voice note.webm"].type.startsWith("video/webm")
        types["Voice note.m4a"].type.startsWith("audio/mp4")
        types["Voice note.webm"].sniff == "nosniff"
        !types["Voice note.m4a"].disposition.startsWith("attachment")
    }
})

feature("find#RoomReactions: the newest reaction first, a message whole with all its reactions, by key and to me; the cursor goes past a removed message", {
    when() {
        const room = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Details spec reactions", memberUserIdList: [B, C]}).chatRoomId
        // equal dates: every event at the same moment
        clock = Date.now() - 60000
        const mine = as(A, "send#Message", {chatRoomId: room, body: "<p>Three reactions to this one</p>"})
        const theirs = as(B, "send#Message", {chatRoomId: room, body: "<p>B's message</p>"})
        const removed = as(B, "send#Message", {chatRoomId: room, body: "<p>removed later</p>"})
        as(A, "react#Message", {chatRoomId: room, eventSeq: theirs.eventSeq, key: "👍"})
        as(A, "react#Message", {chatRoomId: room, eventSeq: mine.eventSeq, key: "👍"})
        as(B, "react#Message", {chatRoomId: room, eventSeq: mine.eventSeq, key: "🎉"})
        as(C, "react#Message", {chatRoomId: room, eventSeq: mine.eventSeq, key: "👍"})
        as(C, "react#Message", {chatRoomId: room, eventSeq: removed.eventSeq, key: "😂"})
        as(B, "redact#Message", {chatRoomId: room, eventSeq: removed.eventSeq})
        // a reaction taken back no longer counts
        as(C, "react#Message", {chatRoomId: room, eventSeq: theirs.eventSeq, key: "❤️"})
        as(C, "react#Message", {chatRoomId: room, eventSeq: theirs.eventSeq, key: "❤️"})
        clock = null
        ec.user.setEffectiveTime(null)
        const pages = allPages((cursor) => as(B, "find#RoomReactions", {chatRoomId: room, limit: 1, beforeReactionSeq: cursor}),
            (out) => out.nextBeforeReactionSeq ?? null, (out) => out.messages || [])
        const toMe = as(A, "find#RoomReactions", {chatRoomId: room, onlyToMe: true}).messages
        // a full page with only others' messages after it: no cursor
        const toMeOne = as(A, "find#RoomReactions", {chatRoomId: room, onlyToMe: true, limit: 1})
        const party = as(B, "find#RoomReactions", {chatRoomId: room, reactionKey: "🎉"}).messages
        const outsider = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Details spec not C"}).chatRoomId
        const stranger = as(C, "find#RoomReactions", {chatRoomId: outsider})
        // an encrypted room: no key filter, no snippet, the reactions for the browser
        const sealedSent = as(A, "send#Message", {chatRoomId: secret, encrypted: sealed("r1")})
        const sealedReaction = as(B, "react#Message", {chatRoomId: secret, eventSeq: sealedSent.eventSeq, encrypted: sealed("r2")})
        const sealedList = as(A, "find#RoomReactions", {chatRoomId: secret}).messages
        const sealedKey = as(A, "find#RoomReactions", {chatRoomId: secret, reactionKey: "👍"}).messages
        return {mine, theirs, pages, toMe, toMeOne, party, stranger, sealedSent, sealedReaction, sealedList, sealedKey}
    },
    then({mine, theirs, pages, toMe, toMeOne, party, stranger, sealedSent, sealedReaction, sealedList, sealedKey}) {
        pages.items.map((one) => one.eventSeq).join(",") == [mine.eventSeq, theirs.eventSeq].join(",")
        pages.items[0].snippet == "Three reactions to this one"
        pages.items[0].reactions.map((one) => one.key + one.count).join(" ") == "👍2 🎉1"
        pages.items[0].reactions.find((one) => one.key === "🎉").mine == true
        pages.items[1].reactions.map((one) => one.key + one.count).join(" ") == "👍1"
        pages.items[0].latestReactionSeq > pages.items[1].latestReactionSeq
        toMe.map((one) => one.eventSeq).join(",") == String(mine.eventSeq)
        toMeOne.messages.length == 1
        toMeOne.nextBeforeReactionSeq == null
        party.map((one) => one.eventSeq).join(",") == String(mine.eventSeq)
        stranger.errors.includes("not found")
        sealedReaction.errors == ""
        sealedList.length == 1
        sealedList[0].snippet == null
        sealedList[0].encrypted.ciphertext == "AwgAEnr1"
        sealedList[0].encryptedReactions.length == 1
        sealedList[0].encryptedReactions[0].eventSeq == sealedReaction.reactionEventSeq
        sealedKey.length == 0
    }
})

feature("find#ChatActivity: my mentions, replies and reactions across rooms, rows of one date kept in order across pages, by kind; a room I left gives none", {
    when() {
        const room = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Details spec activity", memberUserIdList: [D]}).chatRoomId
        const leftRoom = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Details spec left", memberUserIdList: [D]}).chatRoomId
        clock = Date.now() - 30000
        const mine = as(D, "send#Message", {chatRoomId: room, body: "<p>D writes</p>"})
        const mentioned = as(A, "send#Message", {chatRoomId: room, body: `<p>${mention(D)} look at this</p>`})
        const reply = as(A, "send#Message", {chatRoomId: room, body: "<p>a reply</p>", replyToEventSeq: mine.eventSeq})
        const reacted = as(A, "react#Message", {chatRoomId: room, eventSeq: mine.eventSeq, key: "👍"})
        const elsewhere = as(A, "send#Message", {chatRoomId: leftRoom, body: `<p>${mention(D)} in the room D leaves</p>`})
        const sealedRoom = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Details spec activity encrypted", memberUserIdList: [D], encrypted: true}).chatRoomId
        const sealedMention = as(A, "send#Message", {chatRoomId: sealedRoom, encrypted: sealed("a1"), mentionUserIdList: [D]})
        clock = null
        ec.user.setEffectiveTime(null)
        as(A, "remove#Member", {chatRoomId: leftRoom, userId: D})
        const pages = allPages((cursor) => as(D, "find#ChatActivity", {limit: 1, cursor}), (out) => out.nextCursor ?? null, (out) => out.items || [])
        const whole = as(D, "find#ChatActivity", {}).items
        const mentions = as(D, "find#ChatActivity", {kinds: ["mention"]}).items
        const reactions = as(D, "find#ChatActivity", {kinds: "reaction"}).items
        const rows = sys(() => ec.entity.find("moqui.chat.ChatUserActivity").condition("userId", D).count())
        return {room, leftRoom, sealedRoom, mentioned, reply, reacted, elsewhere, sealedMention, pages, whole, mentions, reactions, rows}
    },
    then({room, leftRoom, sealedRoom, mentioned, reply, reacted, elsewhere, sealedMention, pages, whole, mentions, reactions, rows}) {
        elsewhere.errors == ""
        // the row of the room D left is kept, and not listed
        rows == 5
        pages.items.length == 4
        pages.items.map((item) => item.chatRoomId + ":" + item.eventSeq + ":" + item.kind).join(" ") == whole.map((item) => item.chatRoomId + ":" + item.eventSeq + ":" + item.kind).join(" ")
        new Set(pages.items.map((item) => item.chatRoomId + ":" + item.eventSeq + ":" + item.kind)).size == 4
        !pages.items.some((item) => item.chatRoomId === leftRoom)
        pages.items.filter((item) => item.chatRoomId === room).map((item) => item.kind + item.eventSeq).sort().join(" ") == ["mention" + mentioned.eventSeq, "reaction" + reacted.reactionEventSeq, "reply" + reply.eventSeq].sort().join(" ")
        pages.items.find((item) => item.kind === "mention" && item.chatRoomId === room).snippet == "@" + D + " look at this"
        pages.items.find((item) => item.kind === "reaction").reactionKey == "👍"
        pages.items.find((item) => item.kind === "reaction").actorUserId == A
        pages.items.find((item) => item.chatRoomId === sealedRoom).snippet == null
        pages.items.find((item) => item.chatRoomId === sealedRoom).encrypted == true
        pages.items.find((item) => item.chatRoomId === sealedRoom).encryptedContent.ciphertext == "AwgAEna1"
        mentions.length == 2
        reactions.length == 1
    }
})

feature("find#ChatActivity's cursor: two rows alike but for their id are both listed a page at a time; a cursor that is not one is refused", {
    when() {
        const room = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Details spec cursor", memberUserIdList: [C]}).chatRoomId
        const sent = as(A, "send#Message", {chatRoomId: room, body: "<p>twice</p>"})
        // two rows of one user, date, room, event and kind (made here: record#UserActivity makes one a kind)
        const at = ec.user.nowTimestamp
        for (const id of ["CHAT_DETAILS_ACT_1", "CHAT_DETAILS_ACT_2"]) sys(() => {
            const row = ec.entity.makeValue("moqui.chat.ChatUserActivity")
            row.setFields({chatUserActivityId: id, userId: C, activityKindEnumId: "CuaMention", chatRoomId: room, eventSeq: sent.eventSeq,
                messageEventSeq: sent.eventSeq, actorUserId: A, activityDate: at}, true, null, null)
            row.create()
        })
        const pages = allPages((cursor) => as(C, "find#ChatActivity", {limit: 1, cursor}), (out) => out.nextCursor ?? null, (out) => out.items || [])
        const garbage = as(C, "find#ChatActivity", {cursor: "not a cursor"})
        const absurd = as(C, "find#ChatActivity", {cursor: "999999999999999~1~CuaMention~X~" + room})
        return {pages, garbage, absurd}
    },
    then({pages, garbage, absurd}) {
        pages.items.map((item) => item.chatUserActivityId).join(",") == "CHAT_DETAILS_ACT_2,CHAT_DETAILS_ACT_1"
        pages.sizes.slice(0, 2).join(",") == "1,1"
        garbage.errors.includes("not valid")
        absurd.errors.includes("not valid")
    }
})

feature("the last owner's Leave preview and the transfer agree when two moderators joined together", {
    when() {
        clock = Date.now() - 30000
        try {
            const room = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Details succession", memberUserIdList: [D, C, B]}).chatRoomId
            for (const userId of [D, C]) as(A, "set#MemberRole", {chatRoomId: room, userId, roleEnumId: "moderator"})
            const before = as(A, "get#Room", {chatRoomId: room}).memberList.filter((person) => person.roleEnumId === "CmrModerator")
            const leave = as(A, "remove#Member", {chatRoomId: room})
            const owner = as(C, "get#Room", {chatRoomId: room}).memberList.find((person) => person.roleEnumId === "CmrOwner")
            return {before, leave, owner}
        } finally {
            clock = null
            ec.user.setEffectiveTime(null)
        }
    },
    then({before, leave, owner}) {
        before.length == 2
        before[0].joinedDate == before[1].joinedDate
        before[0].userId == C
        !leave.errors
        owner.userId == before[0].userId
    }
})

feature("the terminal lists a room's links and my activity", {
    when() {
        const room = as(A, "create#Room", {roomTypeEnumId: "private", roomName: "Details spec terminal", memberUserIdList: [B]}).chatRoomId
        as(A, "send#Message", {chatRoomId: room, body: `<p>${mention(B)} read https://example.com/guide</p>`})
        ec.user.internalLoginUser(username(B))
        const links = tui({chatRoomId: room, chatView: "links"})
        const activity = tui({chatView: "activity"})
        return {links, activity}
    },
    then({links, activity}) {
        links.includes("Links")
        links.includes("https://example.com/guide")
        links.includes("The guide")
        activity.includes("Activity")
        activity.includes("Mentioned you")
        activity.includes("Details spec terminal")
    }
})
