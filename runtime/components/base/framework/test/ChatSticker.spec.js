// Sticker packs (plans/Comms-Messaging.md §18.3 phase 20; ChatSticker.js, ChatStickerServices): a user's own packs and the
// server's (a sticker admin's); a picture checked by decoding it, so an SVG or a page in a picture's name is refused; who
// may see a pack's pictures; a sent sticker a copy of its picture in the room, left out of the room's files, its words for
// the lists, and kept when its pack is deleted.
// Run: ./moq test framework --spec ChatSticker
// The pictures, each character a byte (uploadedFile's binary): a 2 x 2 red PNG, a 3 x 2 blue one, a 1 x 1 GIF and WebP
const RED_PNG = "\x89\x50\x4e\x47\x0d\x0a\x1a\x0a\x00\x00\x00\x0d\x49\x48\x44\x52\x00\x00\x00\x02\x00\x00\x00\x02\x08\x02\x00\x00\x00\xfd\xd4\x9a\x73\x00\x00\x00\x10\x49\x44\x41\x54\x78\x9c\x63\xf8\xcf\xc0\x00\x44\x0c\x10\x0a\x00\x1f\xee\x03\xfd\x8b\x5f\x14\xd4\x00\x00\x00\x00\x49\x45\x4e\x44\xae\x42\x60\x82"
const BLUE_PNG = "\x89\x50\x4e\x47\x0d\x0a\x1a\x0a\x00\x00\x00\x0d\x49\x48\x44\x52\x00\x00\x00\x03\x00\x00\x00\x02\x08\x02\x00\x00\x00\x12\x16\xf1\x4d\x00\x00\x00\x0f\x49\x44\x41\x54\x78\x9c\x63\x60\x60\xf8\x0f\x43\x30\x16\x00\x35\xde\x05\xfb\xe2\x96\x60\x9e\x00\x00\x00\x00\x49\x45\x4e\x44\xae\x42\x60\x82"
const GIF = "\x47\x49\x46\x38\x39\x61\x01\x00\x01\x00\x80\x00\x00\xff\x00\x00\x00\x00\x00\x21\xf9\x04\x01\x00\x00\x00\x00\x2c\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00\x3b"
const WEBP = "\x52\x49\x46\x46\x1a\x00\x00\x00\x57\x45\x42\x50\x56\x50\x38\x4c\x0d\x00\x00\x00\x2f\x00\x00\x00\x10\x07\x10\x11\x11\x88\x88\xfe\x07\x00"
const A = "CHAT_STICK_A"
const B = "CHAT_STICK_B"
const C = "CHAT_STICK_C"
const ADMIN = "CHAT_STICK_ADMIN"
const USERS = [A, B, C, ADMIN]
const CHAT = "org.moqui.chat.ChatServices."
const ST = "org.moqui.chat.ChatStickerServices."
const GROUP = "CHAT_STICK_SPEC"
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
const make = (entity, fields) => sys(() => ec.entity.makeValue(entity).setFields(fields, true, null, null).createOrUpdate())
const row = (entity, fields) => sys(() => ec.entity.find(entity).condition(fields).one())
const room = (userId, fields) => as(userId, CHAT + "create#Room", Object.assign({roomTypeEnumId: "CrtPrivate"}, fields)).chatRoomId
const picture = (name, bytes, type) => uploadedFile(name, bytes, type, true)
const shared = {}

function makeUsers() {
    sys(() => {
        for (const userId of USERS) {
            if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
                ec.entity.makeValue("moqui.security.UserAccount").setFields({userId, username: username(userId), userFullName: "Stick " + userId.slice(11),
                    disabled: "N"}, true, null, null).create()
            }
        }
    })
    make("moqui.security.UserGroup", {userGroupId: GROUP, description: "Chat stickers spec"})
    make("moqui.security.ArtifactGroup", {artifactGroupId: GROUP, description: "Chat stickers spec"})
    make("moqui.security.ArtifactGroupMember", {artifactGroupId: GROUP, artifactName: "org\\.moqui\\.chat\\..*", artifactTypeEnumId: "AT_SERVICE",
        nameIsPattern: "Y", inheritAuthz: "Y"})
    make("moqui.security.ArtifactAuthz", {artifactAuthzId: GROUP, userGroupId: GROUP, artifactGroupId: GROUP, authzTypeEnumId: "AUTHZT_ALWAYS",
        authzActionEnumId: "AUTHZA_ALL"})
    for (const userId of USERS) make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId, fromDate: new Date(Date.now() - DAY)})
    // the sticker admin: a group of its own with the permission
    make("moqui.security.UserGroup", {userGroupId: GROUP + "_ADMIN", description: "Chat stickers spec admins"})
    make("moqui.security.UserGroupPermission", {userGroupId: GROUP + "_ADMIN", userPermissionId: "ChatStickerAdmin", fromDate: new Date(Date.now() - DAY)})
    make("moqui.security.UserGroupMember", {userGroupId: GROUP + "_ADMIN", userId: ADMIN, fromDate: new Date(Date.now() - DAY)})
}
function cleanUp() {
    sys(() => {
        const packs = Array.from(ec.entity.find("moqui.chat.ChatStickerPack").condition("createdByUserId", "in", USERS).list()).map((pack) => pack.chatStickerPackId)
        if (packs.length) ec.entity.find("moqui.chat.ChatSticker").condition("chatStickerPackId", "in", packs).deleteAll()
        ec.entity.find("moqui.chat.ChatStickerPack").condition("createdByUserId", "in", USERS).deleteAll()
    })
}

spec({
    stepwise: true,
    setupSpec() {
        makeUsers()
        cleanUp()
        shared.team = room(A, {roomName: "Stick team", memberUserIdList: [B]})
    },
    cleanupSpec() {
        cleanUp()
        ec.user.logoutUser()
    }
})

feature("packs: a user's own and the server's, which only a sticker admin makes; a picture is checked by decoding it", {
    when() {
        const mine = as(A, ST + "create#StickerPack", {packName: "Mine"})
        const notAdmin = as(A, ST + "create#StickerPack", {packName: "Everyone's", server: true})
        const server = as(ADMIN, ST + "create#StickerPack", {packName: "Everyone's", server: true})
        shared.mine = mine.chatStickerPackId
        shared.server = server.chatStickerPackId
        const red = as(A, ST + "add#Sticker", {chatStickerPackId: shared.mine, file: picture("red.png", RED_PNG, "image/png"), emoji: "👍", altText: "thumbs up"})
        const again = as(A, ST + "add#Sticker", {chatStickerPackId: shared.mine, file: picture("again.png", RED_PNG, "image/png")})
        const gif = as(A, ST + "add#Sticker", {chatStickerPackId: shared.mine, file: picture("dot.gif", GIF, "image/gif"), emoji: "🎉"})
        const webp = as(ADMIN, ST + "add#Sticker", {chatStickerPackId: shared.server, file: picture("dot.webp", WEBP, "image/webp"), emoji: "😀"})
        // an SVG named .png, an HTML page, and text in a PNG's clothes are refused
        const svg = as(A, ST + "add#Sticker", {chatStickerPackId: shared.mine, file: uploadedFile("x.png", "<svg xmlns='http://www.w3.org/2000/svg'/>", "image/png")})
        const html = as(A, ST + "add#Sticker", {chatStickerPackId: shared.mine, file: uploadedFile("x.html", "<script>alert(1)</script>", "text/html")})
        const othersPack = as(B, ST + "add#Sticker", {chatStickerPackId: shared.mine, file: picture("blue.png", BLUE_PNG, "image/png")})
        const big = withProperty("chat_sticker_max_bytes", "20", () => as(A, ST + "add#Sticker", {chatStickerPackId: shared.mine,
            file: picture("blue.png", BLUE_PNG, "image/png")}))
        shared.red = red.chatStickerId
        shared.webp = webp.chatStickerId
        return {mine, notAdmin, server, red, again, gif, webp, svg, html, othersPack, big}
    },
    then({mine, notAdmin, server, red, again, gif, webp, svg, html, othersPack, big}) {
        mine.errors == ""
        notAdmin.reasonCode == "notAllowed"
        server.errors == ""
        red.errors == ""
        red.sticker.width == 2
        red.sticker.emoji == "👍"
        again.reasonCode == "duplicate"
        gif.errors == ""
        webp.errors == ""
        svg.reasonCode == "badType"
        html.reasonCode == "badType"
        othersPack.reasonCode == "notFound"
        big.reasonCode == "tooLarge"
    }
})

feature("the picker: one's own packs then the server's; a user pack's picture for its owner only; a hidden server pack offered to no one", {
    when() {
        const forA = as(A, ST + "get#StickerPacks", {})
        const forB = as(B, ST + "get#StickerPacks", {})
        const ownImage = as(A, ST + "get#StickerImage", {chatStickerId: shared.red, thumb: true})
        const othersImage = as(B, ST + "get#StickerImage", {chatStickerId: shared.red})
        const serverImage = as(B, ST + "get#StickerImage", {chatStickerId: shared.webp})
        as(ADMIN, ST + "update#StickerPack", {chatStickerPackId: shared.server, hidden: true})
        const hidden = as(B, ST + "get#StickerPacks", {})
        const hiddenImage = as(B, ST + "get#StickerImage", {chatStickerId: shared.webp})
        as(ADMIN, ST + "update#StickerPack", {chatStickerPackId: shared.server, hidden: false})
        return {forA, forB, ownImage, othersImage, serverImage, hidden, hiddenImage}
    },
    then({forA, forB, ownImage, othersImage, serverImage, hidden, hiddenImage}) {
        forA.packList[0].chatStickerPackId == shared.mine
        forA.packList[0].stickers.length == 2
        forA.packList.some((pack) => pack.chatStickerPackId == shared.server)
        !forB.packList.some((pack) => pack.chatStickerPackId == shared.mine)
        forB.packList.some((pack) => pack.chatStickerPackId == shared.server)
        ownImage.location.endsWith("/thumb.png")
        ownImage.contentType == "image/png"
        othersImage.reasonCode == "notFound"
        serverImage.contentType == "image/webp"
        !hidden.packList.some((pack) => pack.chatStickerPackId == shared.server)
        hiddenImage.reasonCode == "notFound"
    }
})

feature("send#Sticker: a copy of its picture in the room, its words for the lists, left out of the room's files, refused in an encrypted room; it stays when its pack goes", {
    when() {
        const sent = as(A, ST + "send#Sticker", {chatRoomId: shared.team, chatStickerId: shared.red})
        const message = row("moqui.chat.ChatEvent", {chatRoomId: shared.team, eventSeq: sent.eventSeq})
        const copy = sys(() => Array.from(ec.entity.find("moqui.chat.ChatAttachment").condition({chatRoomId: shared.team, eventSeq: sent.eventSeq}).list())[0])
        const files = Array.from(as(B, CHAT + "find#RoomFiles", {chatRoomId: shared.team}).fileList || [])
        const timeline = Array.from(as(B, CHAT + "get#Timeline", {chatRoomId: shared.team}).eventList || []).find((one) => one.eventSeq === sent.eventSeq)
        const sealed = room(A, {roomName: "Stick sealed", memberUserIdList: [B], encrypted: true})
        const inSealed = as(A, ST + "send#Sticker", {chatRoomId: sealed, chatStickerId: shared.red})
        const stranger = as(C, ST + "send#Sticker", {chatRoomId: shared.team, chatStickerId: shared.red})
        const notMine = as(B, ST + "send#Sticker", {chatRoomId: shared.team, chatStickerId: shared.red})
        const roomRow = row("moqui.chat.ChatRoom", {chatRoomId: shared.team})
        as(A, ST + "delete#StickerPack", {chatStickerPackId: shared.mine})
        const kept = sys(() => ec.resource.getLocationReference(copy.location).getExists())
        return {sent, bodyText: message.bodyText, content: JSON.parse(message.contentJson), copy, files, timeline, inSealed, stranger, notMine,
            snippet: roomRow.lastMessageSnippet, kept, packGone: row("moqui.chat.ChatStickerPack", {chatStickerPackId: shared.mine}) == null}
    },
    then({sent, bodyText, content, copy, files, timeline, inSealed, stranger, notMine, snippet, kept, packGone}) {
        sent.errors == ""
        bodyText == "Sticker 👍"
        content.sticker.chatStickerId == shared.red
        content.sticker.altText == "thumbs up"
        copy.stickerOf == shared.red
        copy.contentType == "image/png"
        !files.some((file) => file.chatAttachmentId == copy.chatAttachmentId)
        timeline.sticker.emoji == "👍"
        timeline.attachments.length == 1
        inSealed.reasonCode == "encryptedRoom"
        stranger.reasonCode == "notFound"
        notMine.reasonCode == "notFound"
        snippet == "Sticker 👍"
        packGone == true
        kept == true
    }
})
