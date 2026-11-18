// The join page of a room's invite link (plans/Comms-Messaging.md §18.3 phase 17; screen/ChatJoin.xml, the token-route "join"
// of MoquiConf.xml): the page in qvue, rendered once; its join transition for a signed-in person, which joins them and
// answers the room's link; a bad token joins nobody; and signIn keeps the link as the return path after Login. The rules of
// links and requests are framework/test/ChatInvites.spec.js's.
// Run: ./moq test basic-comms --spec ChatJoin
const root = "component://basic-comms/screen/ChatJoin.xml"
const OWNER = "BASIC_COMMS_JOIN_OWNER"
const JOINER = "BASIC_COMMS_JOIN_JOINER"
// test/service/moqui/basiccommstest/TextNamesServices.xml get#ChatTarget keeps a user of this prefix from everyone
const REFUSED = "BASIC_COMMS_REFUSED_JOIN"
const NAMES_APP = "BASIC_COMMS_JOIN_NAMES"
const INVITES = "org.moqui.chat.ChatInviteServices."

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
function makeUser(userId) {
    sys(() => {
        ec.entity.makeValue("moqui.security.UserAccount").setFields({userId, username: username(userId), userFullName: "Join " + userId.slice(18),
            disabled: "N"}, true, null, null).createOrUpdate()
        // BasicComms is opened by BASIC_COMMS_USERS, not ALL_USERS; IronStone's Comms, when installed, by BOSS_USERS
        for (const group of ["BASIC_COMMS_USERS", "BOSS_USERS"]) {
            if (ec.entity.find("moqui.security.UserGroup").condition("userGroupId", group).count() == 0) continue
            ec.entity.makeValue("moqui.security.UserGroupMember").setFields({userGroupId: group, userId, fromDate: new Date(Date.now() - 60000)}, true, null, null).createOrUpdate()
        }
    })
}
function render(path, parameters, method, mode) {
    let test = ec.screen.makeTest().rootScreen(root)
    if (mode) test = test.renderMode(mode)
    const str = test.render(path, parameters || {}, method || null)
    ec.message.clearErrors()
    return {status: str.status, output: String(str.output || "")}
}
// a Chat transition, as the screen's user
const chat = (path, parameters, method) => {
    const str = ec.screen.makeTest().rootScreen("component://basic-comms/screen/BasicComms.xml").render(path, parameters || {}, method || null)
    const errors = ec.message.hasError() ? ec.message.errorsString : ""
    ec.message.clearErrors()
    let out = {}
    try { out = JSON.parse(String(str.output || "{}")) } catch (err) { out = {} }
    return Object.assign({errors}, out)
}
const json = (out) => { try { return JSON.parse(out.output) } catch (err) { return {} } }
const shared = {}

spec({
    stepwise: true,
    setupSpec() {
        makeUser(OWNER)
        makeUser(JOINER)
        makeUser(REFUSED)
        // the test app root of the chat-target provider (test/screen/TextNamesApp.xml), opened by BasicComms' users
        sys(() => {
            ec.entity.makeValue("moqui.security.ArtifactGroup").setFields({artifactGroupId: NAMES_APP, description: "ChatJoin names"}, true, null, null).createOrUpdate()
            ec.entity.makeValue("moqui.security.ArtifactGroupMember").setFields({artifactGroupId: NAMES_APP,
                artifactName: "component://basic-comms/test/screen/TextNamesApp.xml", artifactTypeEnumId: "AT_XML_SCREEN", inheritAuthz: "Y"}, true, null, null).createOrUpdate()
            ec.entity.makeValue("moqui.security.ArtifactAuthz").setFields({artifactAuthzId: NAMES_APP, artifactGroupId: NAMES_APP, userGroupId: "BASIC_COMMS_USERS",
                authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL"}, true, null, null).createOrUpdate()
        })
        sys(() => {
            ec.user.internalLoginUser(username(OWNER))
            shared.room = ec.service.sync().name("org.moqui.chat.ChatServices.create#Room").parameters({roomTypeEnumId: "CrtPrivate", roomName: "Join spec room"}).call().chatRoomId
            shared.token = ec.service.sync().name(INVITES + "create#InviteLink").parameters({chatRoomId: shared.room}).call().token
            ec.message.clearErrors()
        })
    },
    cleanupSpec() {
        sys(() => {
            ec.entity.find("moqui.security.ArtifactAuthz").condition("artifactAuthzId", NAMES_APP).deleteAll()
            ec.entity.find("moqui.security.ArtifactGroupMember").condition("artifactGroupId", NAMES_APP).deleteAll()
            ec.entity.find("moqui.security.ArtifactGroup").condition("artifactGroupId", NAMES_APP).deleteAll()
        })
        ec.user.logoutUser()
    }
})

feature("the join page renders, a signed-in person joins by its transition, and a bad token joins nobody", {
    when() {
        ec.user.internalLoginUser(username(JOINER))
        const page = render("", {token: shared.token}, null, "qvue")
        const bad = json(render("join", {token: "NOT_A_TOKEN_0123456789abcdef"}, "post"))
        const joined = json(render("join", {token: shared.token}, "post"))
        const member = sys(() => ec.entity.find("moqui.chat.ChatRoomMember").condition({chatRoomId: shared.room, userId: JOINER}).one())
        return {page, bad, joined, member}
    },
    then({page, bad, joined, member}) {
        page.output.includes("m-chat-join-screen")
        page.output.includes("Sign in to join")
        bad.reasonCode == "notFound"
        bad.outcome == null
        joined.outcome == "joined"
        joined.roomUrl.includes("/Chat?chatRoomId=" + shared.room)
        member.leftDate == null
    }
})

feature("Chat's invite transitions: links with their QR code, the requests' queue and its count, Browse's listed rooms", {
    when() {
        ec.user.internalLoginUser(username(OWNER))
        const made = chat("Chat/createInviteLink", {chatRoomId: shared.room, approvalRequired: "true", maxUses: "5", expiryHours: "24"}, "post")
        const links = chat("Chat/inviteLinks", {chatRoomId: shared.room})
        const qr = chat("Chat/inviteQr", {chatInviteLinkId: made.chatInviteLinkId})
        const listed = chat("Chat/setRoomListed", {chatRoomId: shared.room, listed: "true"}, "post")
        ec.user.internalLoginUser(username(JOINER))
        chat("Chat/leave", {chatRoomId: shared.room}, "post")
        const browse = chat("Chat/listedRooms", {searchText: "join spec"})
        const asked = chat("Chat/requestJoin", {chatRoomId: shared.room, note: "back again"}, "post")
        ec.user.internalLoginUser(username(OWNER))
        const count = chat("Chat/joinRequestCount", {chatRoomId: shared.room})
        const queue = chat("Chat/joinRequests", {chatRoomId: shared.room})
        const decided = chat("Chat/decideJoinRequest", {chatJoinRequestId: asked.chatJoinRequestId, decision: "approve"}, "post")
        const revoked = chat("Chat/revokeInviteLink", {chatInviteLinkId: made.chatInviteLinkId}, "post")
        return {made, links, qr, listed, browse, asked, count, queue, decided, revoked}
    },
    then({made, links, qr, listed, browse, asked, count, queue, decided, revoked}) {
        made.path.startsWith("/join/")
        links.linkList.length == 2
        links.canList == true
        qr.url.endsWith(made.path)
        qr.qrSvg.startsWith("<svg")
        listed.listed == true
        browse.roomList.some((room) => room.chatRoomId === shared.room)
        asked.outcome == "requested"
        count.pendingCount == 1
        queue.requestList[0].note == "back again"
        decided.decided == true
        revoked.revoked == true
    }
})

feature("a person an app keeps from the link's maker gets a link that does not exist, and an approval checks the approving moderator", {
    when() {
        ec.user.internalLoginUser(username(REFUSED))
        const page = sys(() => ec.service.sync().name(INVITES + "check#InviteToken").parameter("token", shared.token).call())
        const joined = json(render("join", {token: shared.token}, "post"))
        const asked = sys(() => ec.service.sync().name(INVITES + "request#Join").parameters({chatRoomId: shared.room}).call() || {})
        ec.message.clearErrors()
        ec.user.internalLoginUser(username(OWNER))
        const decided = chat("Chat/decideJoinRequest", {chatJoinRequestId: asked.chatJoinRequestId, decision: "approve"}, "post")
        const errors = String(decided.errors || "") + JSON.stringify(decided)
        const member = sys(() => ec.entity.find("moqui.chat.ChatRoomMember").condition({chatRoomId: shared.room, userId: REFUSED}).one())
        const request = sys(() => ec.entity.find("moqui.chat.ChatJoinRequest").condition("chatJoinRequestId", asked.chatJoinRequestId).one())
        return {page, joined, asked, decided, errors, member, request}
    },
    then({page, joined, asked, decided, errors, member, request}) {
        page.valid == false
        joined.reasonCode == "notFound"
        asked.outcome == "requested"
        decided.decided != true
        errors.includes("not found")
        member == null
        request.statusEnumId == "CjrPending"
    }
})

feature("signIn keeps the link as the return path after Login, and only for a link that works", {
    when() {
        ec.user.logoutUser()
        const good = json(render("signIn", {token: shared.token}, "post"))
        const bad = json(render("signIn", {token: "NOT_A_TOKEN_0123456789abcdef"}, "post"))
        return {good, bad}
    },
    then({good, bad}) {
        good.loginUrl == "/Login"
        bad.loginUrl == null
    }
})
