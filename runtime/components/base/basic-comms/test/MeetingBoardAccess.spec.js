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
