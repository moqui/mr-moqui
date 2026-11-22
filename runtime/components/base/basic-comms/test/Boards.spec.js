// Board creation must preserve an existing wiki page at the requested name, including another page type.
const boardCall = (service, parameters) => {
    const result = ec.service.sync().name(service).parameters(parameters).disableAuthz().call() || {}
    const error = ec.message.hasError() ? ec.message.errorsString : ""
    ec.message.clearErrors()
    return {...result, error}
}

feature("a new personal board opens as editable and creating the same name preserves the scene", {
    when() {
        ec.user.internalLoginUser("john.doe")
        const service = "org.moqui.comms.BoardServices."
        const title = "Planning sketch"
        const created = boardCall(service + "create#Board", {pageName: title})
        const duplicate = boardCall(service + "create#Board", {pageName: title})
        const board = boardCall(service + "get#Board", {wikiPageId: created.wikiPageId})
        return {created, duplicate, board}
    },
    then({created, duplicate, board}) {
        created.error === ""
        duplicate.error.includes("already exists")
        board.error === ""
        board.canEdit === true
        board.title === "Planning sketch"
        JSON.parse(board.scene).type === "excalidraw"
    }
})

feature("room boards follow current membership and survive room archival", {
    when() {
        ec.user.internalLoginUser("john.doe")
        const service = "org.moqui.chat.RoomBoardServices."
        const created = boardCall("org.moqui.chat.ChatServices.create#Room", {roomTypeEnumId:"CrtPrivate",roomName:"Sketch room"})
        const chatRoomId = created.chatRoomId
        const board = boardCall(service+"create#RoomBoard", {chatRoomId,pageName:"Room sketch"})
        if (board.error) throw Error(board.error)
        const room = ec.entity.find("moqui.chat.ChatRoom").condition({chatRoomId}).disableAuthz().one()
        const wikiSpaceId = room.wikiSpaceId
        const list = boardCall(service+"find#RoomBoards",{chatRoomId})
        const grants = ec.entity.find("moqui.resource.wiki.WikiSpaceUser").condition({wikiSpaceId}).disableAuthz().count()
        const timeline = ec.entity.find("moqui.chat.ChatEvent").condition({chatRoomId,eventTypeEnumId:"CetBoard"}).disableAuthz().count()
        boardCall("update#moqui.chat.ChatRoom",{chatRoomId,archivedDate:ec.user.nowTimestamp})
        const wiki="org.moqui.resource.WikiServices."
        const access=()=>boardCall(wiki+"get#WikiPageAccess",{wikiPageId:board.wikiPageId})
        const archived=access()
        const member=ec.entity.find("moqui.chat.ChatRoomMember").condition({chatRoomId,userId:ec.user.userId}).disableAuthz().one()
        boardCall("update#moqui.chat.ChatRoomMember",{chatRoomId,chatActorId:member.chatActorId,leftDate:ec.user.nowTimestamp})
        const departed=access()
        const scope=boardCall(wiki+"get#WikiViewScope",{})
        return {list, archived, departed, scope, wikiSpaceId, grants, timeline}
    },
    then(r) {
        r.list.boards[0].title === "Room sketch"
        r.grants === 0 && r.timeline === 1
        r.archived.canView && !r.archived.canUpdate
        !r.departed.canView && !r.scope.viewSpaceIdList.includes(r.wikiSpaceId)
    }
})
