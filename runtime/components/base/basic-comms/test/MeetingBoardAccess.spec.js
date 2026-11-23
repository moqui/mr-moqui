// A meeting's grant to its open board (MeetingBoardAccess.js), which the collab socket asks at a join and again when the
// call changes: refused for another page, a closed board, a leg that left, a member who left the room, a wrong guest
// credential or link token, and a call that ended.
const grantCall = (service, parameters) => {
    const result = ec.service.sync().name(service).parameters(parameters).disableAuthz().call() || {}
    const error = ec.message.hasError() ? ec.message.errorsString : ""
    ec.message.clearErrors()
    return {...result, error}
}
const find = (entity, keys) => ec.entity.find(entity).condition(keys).disableAuthz()

feature("a meeting board grant follows the board, the leg, the room membership and the call", {
    when() {
        ec.user.internalLoginUser("john.doe")
        const call = "org.moqui.call.CallServices."
        grantCall(call + "register#MediaNode", {instanceId:"board-grant-spec", mediaAddress:"127.0.0.1:20102"})
        const {chatRoomId} = grantCall("org.moqui.chat.ChatServices.create#Room", {roomName:"Grant review", roomTypeEnumId:"CrtPrivate"})
        const started = grantCall(call + "start#Call", {chatRoomId})
        if (started.error) throw Error(started.error)
        const {callSessionId, callLegId} = started
        const board = grantCall(call + "create#MeetingBoard", {callSessionId, pageName:"Grant board"})
        const other = grantCall(call + "create#MeetingBoard", {callSessionId, pageName:"Grant other"})
        if (board.error || other.error) throw Error(board.error || other.error)
        const wikiPageId = board.wikiPageId
        // the call's rows as the call engine writes them
        ec.artifactExecution.disableAuthz()
        const row = ec.entity.makeValue("moqui.call.CallBoard")
        row.setFields({callSessionId, wikiPageId, openedByUserId:ec.user.userId, openedDate:ec.user.nowTimestamp,
            memberCanEdit:"Y", guestCanEdit:"Y"}, true, null, null)
        row.create()
        const grant = (extra) => grantCall("org.moqui.call.MeetingBoardServices.get#MeetingBoardGrant",
            {wikiPageId, callSessionId, callLegId, userId:ec.user.userId, ...extra})
        const asked = {}
        asked.open = grant({})
        asked.otherPage = grant({wikiPageId:other.wikiPageId})
        asked.wrongCredential = grant({credential:"x".repeat(40)})
        asked.image = grantCall("org.moqui.call.MeetingBoardServices.access#MeetingBoardImage",
            {wikiPageId, filename:"photo.png", token:"not-a-link-token", credential:"y".repeat(40)})

        const board_ = find("moqui.call.CallBoard", {callSessionId, wikiPageId}).one()
        board_.set("closedDate", ec.user.nowTimestamp); board_.update()
        asked.closed = grant({})
        board_.set("closedDate", null); board_.update()

        const leg = find("moqui.call.CallLeg", {callLegId}).one()
        leg.set("statusId", "ClsLeft"); leg.update()
        asked.legLeft = grant({})
        leg.set("statusId", "ClsAnswered"); leg.update()

        const member = find("moqui.chat.ChatRoomMember", {chatRoomId, userId:ec.user.userId}).one()
        member.set("leftDate", ec.user.nowTimestamp); member.update()
        asked.leftRoom = grant({})
        member.set("leftDate", null); member.update()

        asked.again = grant({})
        grantCall(call + "end#Call", {callSessionId})
        asked.ended = grant({})
        ec.artifactExecution.enableAuthz()
        return asked
    },
    then(r) {
        r.open.error === "" && r.open.canView === true && r.open.canEdit === true
        r.open.writerUserId === "EX_JOHN_DOE"
        !r.otherPage.canView
        !r.wrongCredential.canView
        r.image.denied === true
        !r.closed.canView
        !r.legLeft.canView
        !r.leftRoom.canView
        r.again.canView === true
        !r.ended.canView
    }
})

feature("the owner opens a recurring board after early arrivals and revocation ends guest image access", {
    when() {
        ec.user.internalLoginUser("john.doe")
        const calls = "org.moqui.call.CallServices."
        const wiki = "org.moqui.resource.WikiServices."
        const service = (name, params) => {
            const result = grantCall(name, params)
            if (result.error) throw Error(name + ": " + result.error)
            return result
        }
        const reader = "BOARD_EARLY_READER"
        service("create#moqui.security.UserAccount", {userId: reader, username: "board.early.reader", userFullName: "Early reader"})
        service(calls + "register#MediaNode", {instanceId:"board-recurring-spec", mediaAddress:"127.0.0.1:20103"})
        const {chatRoomId} = service("org.moqui.chat.ChatServices.create#Room", {roomName:"Recurring meeting", memberUserIdList:[reader]})
        const {wikiSpaceId} = service(wiki + "create#WikiSpace", {wikiSpaceId:"EARLY", restrictUpdate:"Y", description:"Recurring boards",
            memberList:[{userId:reader, allowView:"Y", allowUpdate:"N"}]})
        const {wikiPageId} = service(wiki + "create#WikiBoard", {wikiSpaceId, pageName:"Recurring drawing"})
        const {callLinkId, token} = service(calls + "create#CallLink", {name:"Weekly sketch", wikiPageId, chatRoomId, policy:"link"})
        const guest = service(calls + "join#AsGuest", {token, displayName:"Early visitor"})
        const callSessionId = guest.callSessionId
        const boardRows = () => find("moqui.call.CallBoard", {callSessionId, wikiPageId}).count()
        const afterGuest = boardRows()
        ec.user.internalLoginUser("board.early.reader")
        const rights = service(wiki + "get#WikiPageAccess", {wikiPageId})
        const member = service(calls + "join#Call", {callSessionId})
        const afterMember = boardRows()
        ec.user.internalLoginUser("john.doe")
        const opened = service(calls + "open#CallLink", {callLinkId})
        const keys = {callSessionId, wikiPageId, callLegId:guest.callLegId, credential:guest.credential}
        const image = params => service("org.moqui.call.MeetingBoardServices.access#MeetingBoardImage",
            {...keys, token, ...params})
        const file = () => uploadedFile("sketch.png", "pixels", "image/png")
        const uploaded = image({attachmentFile:file()})
        const read = image({filename:"sketch.png"})
        const grant = () => service("org.moqui.call.MeetingBoardServices.get#MeetingBoardGrant", keys)
        const before = grant()
        const bound = service(calls + "bind#Connection", {callSessionId, callLegId:opened.callLegId})
        service("org.moqui.call.StageServices.change#CallStage", {callSessionId, callLegId:opened.callLegId,
            generation:bound.generation, action:"take", kind:"screen"})
        const reopened = service(calls + "open#CallLink", {callLinkId})
        service("org.moqui.call.StageServices.change#CallStage", {callSessionId, callLegId:opened.callLegId,
            generation:bound.generation, action:"take", kind:"board", wikiPageId})
        const restored = grant()
        service(calls + "revoke#CallLink", {callLinkId})
        const refusedRead = image({filename:"sketch.png"}), refusedWrite = image({attachmentFile:file()}), revoked = grant()
        const replacement = service(calls + "create#CallLink", {name:"Weekly sketch", wikiPageId, chatRoomId, policy:"link"})
        const checks = token => service(calls + "check#CallLinkToken", {token}).valid
        const reissued = replacement.token !== token && checks(replacement.token) && !checks(token)
        // A clean board already shown in the call has no new revision when the host ends it.
        const history = () => find("moqui.resource.wiki.WikiPageHistory", {wikiPageId}).count()
        const revisions = history()
        service(calls + "end#Call", {callSessionId})
        return {afterGuest, afterMember, member, rights, opened, reopened, wikiPageId, uploaded, read, before, restored, revoked,
            refusedRead, refusedWrite, reissued, rows:boardRows(), clean:history() === revisions}
    },
    then(r) {
        r.afterGuest === 0 && r.afterMember === 0 && !!r.member.callLegId
        r.rights.canView && !r.rights.canUpdate
        r.opened.call.stage.wikiPageId === r.wikiPageId && r.rows === 1
        r.reopened.call.stage.kind === "screen" && r.reopened.call.stage.callLegId === r.opened.callLegId
        r.uploaded.filename === "sketch.png" && r.read.base64 === "cGl4ZWxz"
        r.before.canEdit && r.restored.canEdit && r.before.canView && !r.revoked.canEdit && !r.revoked.canView
        r.refusedRead.denied && r.refusedWrite.denied
        r.reissued && r.clean
    }
})
