// The join page of a room's invite link (plans/Comms-Messaging.md §18.3 phase 17; screen/ChatJoin.xml, the token-route "join"
// of MoquiConf.xml): the page in qvue, rendered once; its join transition for a signed-in person, which joins them and
// answers the room's link; a bad token joins nobody; and signIn keeps the link as the return path after Login. The rules of
// links and requests are framework/test/ChatInvites.spec.js's.
// Run: ./moq test basic-comms --spec ChatJoin
const root = "component://basic-comms/screen/ChatJoin.xml"
const OWNER = "BASIC_COMMS_JOIN_OWNER"
const JOINER = "BASIC_COMMS_JOIN_JOINER"
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
const json = (out) => { try { return JSON.parse(out.output) } catch (err) { return {} } }
const shared = {}

spec({
    stepwise: true,
    setupSpec() {
        makeUser(OWNER)
        makeUser(JOINER)
        sys(() => {
            ec.user.internalLoginUser(username(OWNER))
            shared.room = ec.service.sync().name("org.moqui.chat.ChatServices.create#Room").parameters({roomTypeEnumId: "CrtPrivate", roomName: "Join spec room"}).call().chatRoomId
            shared.token = ec.service.sync().name(INVITES + "create#InviteLink").parameters({chatRoomId: shared.room}).call().token
            ec.message.clearErrors()
        })
    },
    cleanupSpec() { ec.user.logoutUser() }
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
        const chat = (path, parameters, method) => {
            const str = ec.screen.makeTest().rootScreen("component://basic-comms/screen/BasicComms.xml").render(path, parameters || {}, method || null)
            ec.message.clearErrors()
            try { return JSON.parse(String(str.output || "{}")) } catch (err) { return {} }
        }
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
