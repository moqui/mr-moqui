// Invite links and join requests (plans/Comms-Messaging.md §18.3 phase 17; ChatInvites.js, ChatInviteServices): who makes a
// link in which room; the token check and its states; a straight join and its event; a link with approval and the
// moderators' queue (approve through add#Member, deny and the wait after it); a request is a use; a person taken out comes
// back only by a request; the maker's block answers as not found and a blocking moderator does not see the request; an
// external user and a public room; a room listed in Browse; the links' limit; revoke.
// Run: ./moq test framework --spec ChatInvites
const A = "CHAT_INVITES_A" // owner of the private room
const B = "CHAT_INVITES_B" // member, then moderator
const C = "CHAT_INVITES_C"
const D = "CHAT_INVITES_D"
const E = "CHAT_INVITES_E"
const F = "CHAT_INVITES_F" // an external user
const USERS = [A, B, C, D, E, F]
const CHAT = "org.moqui.chat.ChatServices."
const INVITES = "org.moqui.chat.ChatInviteServices."
const BLOCKS = "org.moqui.chat.ChatBlockServices."
const GROUP = "CHAT_INVITES_SPEC"
const EXTERNAL_GROUP = "CHAT_INVITES_EXT"

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
const make = (entity, fields) => ec.entity.makeValue(entity).setFields(fields, true, null, null).createOrUpdate()
function setup() {
    sys(() => {
        for (const userId of USERS) {
            make("moqui.security.UserAccount", {userId, username: username(userId), userFullName: "Invites " + userId.slice(13), disabled: "N"})
        }
        make("moqui.security.UserGroup", {userGroupId: GROUP, description: "Chat invites spec"})
        make("moqui.security.ArtifactGroup", {artifactGroupId: GROUP, description: "Chat invites spec"})
        make("moqui.security.ArtifactGroupMember", {artifactGroupId: GROUP, artifactName: "org\\.moqui\\.chat\\..*", artifactTypeEnumId: "AT_SERVICE",
            nameIsPattern: "Y", inheritAuthz: "Y"})
        make("moqui.security.ArtifactAuthz", {artifactAuthzId: GROUP, userGroupId: GROUP, artifactGroupId: GROUP, authzTypeEnumId: "AUTHZT_ALWAYS",
            authzActionEnumId: "AUTHZA_ALL"})
        for (const userId of USERS) make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId, fromDate: new Date(Date.now() - 60000)})
        make("moqui.security.UserGroup", {userGroupId: EXTERNAL_GROUP, description: "Chat invites spec external"})
        make("moqui.security.UserGroupPermission", {userGroupId: EXTERNAL_GROUP, userPermissionId: "EXTERNAL_USER", fromDate: new Date(Date.now() - 60000)})
        make("moqui.security.UserGroupMember", {userGroupId: EXTERNAL_GROUP, userId: F, fromDate: new Date(Date.now() - 60000)})
        ec.entity.find("moqui.chat.ChatUserBlock").condition("userId", "in", USERS).deleteAll()
        ec.entity.find("moqui.chat.ChatJoinRequest").condition("userId", "in", USERS).deleteAll()
        ec.cache.getCache("moqui.chat.invite.joins").clear()
    })
}
const tokenOf = (path) => String(path || "").replace("/join/", "")
const check = (token) => sys(() => ec.service.sync().name(INVITES + "check#InviteToken").parameter("token", token).call() || {})
const lastMembership = (chatRoomId) => sys(() => JSON.parse(ec.entity.find("moqui.chat.ChatEvent").condition({chatRoomId, eventTypeEnumId: "CetMembership"})
    .orderBy("-eventSeq").one().contentJson))
const member = (chatRoomId, userId) => sys(() => ec.entity.find("moqui.chat.ChatRoomMember").condition({chatRoomId, userId}).one())
const shared = {}

spec({
    stepwise: true,
    setupSpec() { setup() },
    cleanupSpec() {
        sys(() => ec.entity.find("moqui.chat.ChatUserBlock").condition("userId", "in", USERS).deleteAll())
        ec.user.logoutUser()
    }
})

feature("a private room's moderators make links, a public room's members do, and a direct room takes none", {
    when() {
        shared.room = as(A, CHAT + "create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Invites private", memberUserIdList: [B]}).chatRoomId
        shared.pub = as(B, CHAT + "create#Room", {roomTypeEnumId: "CrtPublic", roomName: "Invites public"}).chatRoomId
        const direct = as(A, CHAT + "create#Room", {roomTypeEnumId: "CrtDirect", memberUserIdList: [C]}).chatRoomId
        const owner = as(A, INVITES + "create#InviteLink", {chatRoomId: shared.room, label: "Open"})
        const plainMember = as(B, INVITES + "create#InviteLink", {chatRoomId: shared.room})
        const stranger = as(C, INVITES + "create#InviteLink", {chatRoomId: shared.room})
        const publicMember = as(B, INVITES + "create#InviteLink", {chatRoomId: shared.pub, approvalRequired: "Y"})
        const directLink = as(A, INVITES + "create#InviteLink", {chatRoomId: direct})
        const badUses = as(A, INVITES + "create#InviteLink", {chatRoomId: shared.room, maxUses: 7})
        shared.open = tokenOf(owner.path)
        shared.publicLink = tokenOf(publicMember.path)
        const listed = as(A, INVITES + "find#InviteLinks", {chatRoomId: shared.room})
        const theirs = as(B, INVITES + "find#InviteLinks", {chatRoomId: shared.room})
        return {owner, plainMember, stranger, publicMember, directLink, badUses, listed, theirs}
    },
    then({owner, plainMember, stranger, publicMember, directLink, badUses, listed, theirs}) {
        owner.errors == ""
        owner.path.startsWith("/join/")
        owner.token.length == 32
        plainMember.reasonCode == "notAllowed"
        stranger.reasonCode == "notFound"
        publicMember.errors == ""
        directLink.reasonCode == "roomType"
        badUses.reasonCode == "badUses"
        // a public room's link never needs approval; a moderator sees every link, and copies it again
        sys(() => ec.entity.find("moqui.chat.ChatInviteLink").condition("chatInviteLinkId", publicMember.chatInviteLinkId).one().approvalRequired) == "N"
        listed.linkList.length == 1
        listed.linkList[0].path == owner.path
        listed.canMake == true
        listed.canList == true
        theirs.linkList.length == 0
        theirs.canMake == false
    }
})

feature("the token check shows the room to a link that works and nothing to another token", {
    when() {
        return {good: check(shared.open), bad: check("NOT_A_TOKEN_0123456789abcdef"), mangled: check("short")}
    },
    then({good, bad, mangled}) {
        good.valid == true
        good.routeContext.roomName == "Invites private"
        good.routeContext.memberCount == 2
        good.routeContext.approval == false
        good.routeContext.roomKind == "private"
        bad.valid == false
        bad.routeContext == null
        mangled.valid == false
    }
})

feature("a person joins by the link, the room says so, and the link counts the use", {
    when() {
        const joined = as(C, INVITES + "join#ByInvite", {token: shared.open})
        const again = as(C, INVITES + "join#ByInvite", {token: shared.open})
        const signedIn = sys(() => { ec.user.internalLoginUser(username(C)); return ec.service.sync().name(INVITES + "check#InviteToken").parameter("token", shared.open).call() })
        return {joined, again, event: lastMembership(shared.room), signedIn}
    },
    then({joined, again, event, signedIn}) {
        joined.outcome == "joined"
        joined.chatRoomId == shared.room
        joined.roomUrl.includes("/Chat?chatRoomId=")
        again.outcome == "member"
        event.action == "join"
        event.via == "link"
        event.userId == C
        member(shared.room, C).leftDate == null
        sys(() => ec.entity.find("moqui.chat.ChatInviteLink").condition("tokenHash", ec.user.singleUseCodeHash(shared.open)).one().useCount) == 1
        signedIn.routeContext.member == true
        signedIn.routeContext.makerName == "Invites A"
    }
})

feature("a link with approval makes a request; approve adds the person, deny makes them wait", {
    when() {
        const approval = as(A, INVITES + "create#InviteLink", {chatRoomId: shared.room, approvalRequired: "Y", maxUses: 5})
        const token = tokenOf(approval.path)
        const asked = as(D, INVITES + "join#ByInvite", {token, note: "From the newsletter"})
        const askedAgain = as(D, INVITES + "join#ByInvite", {token})
        const askedE = as(E, INVITES + "join#ByInvite", {token})
        const queue = as(A, INVITES + "find#JoinRequests", {chatRoomId: shared.room})
        const notModerator = as(C, INVITES + "find#JoinRequests", {chatRoomId: shared.room})
        const count = as(A, INVITES + "get#JoinRequestCount", {chatRoomId: shared.room})
        const notified = sys(() => ec.entity.find("moqui.security.user.NotificationMessage").condition("topic", "ChatJoinRequest")
            .condition("recordId", shared.room).count())
        const deny = as(A, INVITES + "decide#JoinRequest", {chatJoinRequestId: asked.chatJoinRequestId, decision: "deny"})
        const approve = as(A, INVITES + "decide#JoinRequest", {chatJoinRequestIdList: [askedE.chatJoinRequestId], decision: "approve"})
        const denied = as(D, INVITES + "join#ByInvite", {token})
        const uses = sys(() => ec.entity.find("moqui.chat.ChatInviteLink").condition("chatInviteLinkId", approval.chatInviteLinkId).one().useCount)
        return {asked, askedAgain, askedE, queue, notModerator, count, notified, deny, approve, denied, uses}
    },
    then({asked, askedAgain, askedE, queue, notModerator, count, notified, deny, approve, denied, uses}) {
        asked.outcome == "requested"
        askedAgain.outcome == "requested"
        askedAgain.chatJoinRequestId == asked.chatJoinRequestId
        askedE.outcome == "requested"
        queue.requestList.length == 2
        queue.requestList[0].note == "From the newsletter"
        queue.requestList[0].fromBrowse == false
        notModerator.reasonCode == "notFound"
        count.pendingCount == 2
        notified == 2
        deny.decided == true
        approve.decidedCount == 1
        approve.failedList.length == 0
        member(shared.room, E).leftDate == null
        member(shared.room, D) == null
        denied.reasonCode == "deniedRecently"
        // a request is a use, and the same person asking again is not
        uses == 2
    }
})

feature("a person taken out of the room comes back only by a request", {
    when() {
        as(A, CHAT + "remove#Member", {chatRoomId: shared.room, userId: C})
        const removedBy = member(shared.room, C).removedByUserId
        const back = as(C, INVITES + "join#ByInvite", {token: shared.open})
        const queue = as(A, INVITES + "find#JoinRequests", {chatRoomId: shared.room})
        return {removedBy, back, queue}
    },
    then({removedBy, back, queue}) {
        removedBy == A
        back.outcome == "requested"
        member(shared.room, C).leftDate != null
        queue.requestList.find((request) => request.userId === C).wasRemoved == true
    }
})

feature("the maker's block answers as not found, and a moderator who blocked someone does not see their request", {
    when() {
        as(B, BLOCKS + "block#ChatUser", {blockedUserId: D})
        const byBlocker = as(D, INVITES + "join#ByInvite", {token: shared.publicLink})
        as(A, CHAT + "set#MemberRole", {chatRoomId: shared.room, userId: B, roleEnumId: "CmrModerator"})
        sys(() => ec.entity.find("moqui.chat.ChatJoinRequest").condition({chatRoomId: shared.room, userId: D}).deleteAll())
        const asked = as(D, INVITES + "join#ByInvite", {token: tokenOf(as(A, INVITES + "find#InviteLinks", {chatRoomId: shared.room})
            .linkList.find((link) => link.approvalRequired).path)})
        const seenByA = as(A, INVITES + "find#JoinRequests", {chatRoomId: shared.room}).requestList.some((request) => request.userId === D)
        const seenByB = as(B, INVITES + "find#JoinRequests", {chatRoomId: shared.room}).requestList.some((request) => request.userId === D)
        as(B, BLOCKS + "unblock#ChatUser", {blockedUserId: D})
        return {byBlocker, asked, seenByA, seenByB}
    },
    then({byBlocker, asked, seenByA, seenByB}) {
        byBlocker.reasonCode == "notFound"
        asked.outcome == "requested"
        seenByA == true
        seenByB == false
    }
})

feature("an external user joins no public room by a link and makes no link", {
    when() {
        const pub = as(F, INVITES + "join#ByInvite", {token: shared.publicLink})
        const priv = as(F, INVITES + "join#ByInvite", {token: shared.open})
        const own = as(F, INVITES + "create#InviteLink", {chatRoomId: shared.room})
        return {pub, priv, own}
    },
    then({pub, priv, own}) {
        pub.reasonCode == "notFound"
        priv.outcome == "joined"
        own.reasonCode == "notAllowed"
    }
})

feature("a private room its owner lists in Browse takes requests, and others cannot list it", {
    when() {
        const notOwner = as(B, INVITES + "set#RoomListed", {chatRoomId: shared.room, listed: "Y"})
        const listed = as(A, INVITES + "set#RoomListed", {chatRoomId: shared.room, listed: "Y"})
        const browse = as(D, INVITES + "find#ListedRooms", {searchText: "invites priv"})
        sys(() => ec.entity.find("moqui.chat.ChatJoinRequest").condition({chatRoomId: shared.room, userId: D}).deleteAll())
        const asked = as(D, INVITES + "request#Join", {chatRoomId: shared.room, note: "hello"})
        const external = as(F, INVITES + "find#ListedRooms", {})
        const browseAfter = as(D, INVITES + "find#ListedRooms", {searchText: "invites priv"})
        const cancelled = as(D, INVITES + "cancel#JoinRequest", {chatJoinRequestId: asked.chatJoinRequestId})
        as(A, INVITES + "set#RoomListed", {chatRoomId: shared.room, listed: "N"})
        const unlisted = as(D, INVITES + "request#Join", {chatRoomId: shared.room})
        return {notOwner, listed, browse, asked, external, browseAfter, cancelled, unlisted}
    },
    then({notOwner, listed, browse, asked, external, browseAfter, cancelled, unlisted}) {
        notOwner.reasonCode == "notAllowed"
        listed.listed == true
        browse.roomList.some((room) => room.chatRoomId === shared.room)
        asked.outcome == "requested"
        external.roomList.length == 0
        browseAfter.roomList.find((room) => room.chatRoomId === shared.room).pending == true
        cancelled.cancelled == true
        unlisted.reasonCode == "notFound"
    }
})

feature("a link stops when revoked, used up or its maker is no longer a moderator, and a room has at most 20", {
    when() {
        const once = as(B, INVITES + "create#InviteLink", {chatRoomId: shared.room, maxUses: 1, expiryHours: 0})
        const fromB = as(B, INVITES + "create#InviteLink", {chatRoomId: shared.room})
        const usedOnce = as(D, INVITES + "join#ByInvite", {token: tokenOf(once.path)})
        const usedUp = check(tokenOf(once.path)).valid
        as(A, CHAT + "set#MemberRole", {chatRoomId: shared.room, userId: B, roleEnumId: "CmrMember"})
        const makerDemoted = check(tokenOf(fromB.path)).valid
        const revoked = as(C, INVITES + "revoke#InviteLink", {chatInviteLinkId: once.chatInviteLinkId})
        const revoke = as(A, INVITES + "revoke#InviteLink", {chatInviteLinkId: sys(() => ec.entity.find("moqui.chat.ChatInviteLink")
            .condition("tokenHash", ec.user.singleUseCodeHash(shared.open)).one().chatInviteLinkId)})
        const afterRevoke = check(shared.open).valid
        const made = []
        for (let i = 0; i < 21; i++) made.push(as(A, INVITES + "create#InviteLink", {chatRoomId: shared.room}))
        return {once, usedOnce, usedUp, makerDemoted, revoked, revoke, afterRevoke, made}
    },
    then({once, usedOnce, usedUp, makerDemoted, revoked, revoke, afterRevoke, made}) {
        once.errors == ""
        usedOnce.outcome == "joined"
        usedUp == false
        makerDemoted == false
        revoked.reasonCode == "notFound"
        revoke.revoked == true
        afterRevoke == false
        made.filter((out) => out.chatInviteLinkId).length <= 20
        made[made.length - 1].reasonCode == "tooManyLinks"
    }
})
