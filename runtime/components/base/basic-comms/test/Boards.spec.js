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

feature("markup saves a reply attachment beside the original image", {
    when() {
        ec.user.internalLoginUser("john.doe")
        const chat = "org.moqui.chat.ChatServices."
        const {chatRoomId} = boardCall(chat+"create#Room",{roomTypeEnumId:"CrtPrivate",roomName:"Image markup"})
        const image = uploadedFile("map.png","original pixels","image/png")
        const original = boardCall(chat+"upload#Attachment",{chatRoomId,file:image})
        const sent = boardCall(chat+"send#Message",{chatRoomId,chatAttachmentIdList:[original.chatAttachmentId]})
        const copy = boardCall("org.moqui.comms.AnnotationServices.markup#ChatImage",{chatRoomId,chatAttachmentId:original.chatAttachmentId,
            file:uploadedFile("map-marked.png","annotated pixels","image/png")})
        const get = id => boardCall(chat+"get#Attachment",{chatAttachmentId:id})
        const first = get(original.chatAttachmentId), second = get(copy.chatAttachmentId)
        const reply = ec.entity.find("moqui.chat.ChatEvent").condition({chatRoomId,eventSeq:copy.eventSeq}).disableAuthz().one()
        return {copy,original,sent,first,second,reply}
    },
    then(r) {
        r.copy.error === "" && r.copy.chatAttachmentId !== r.original.chatAttachmentId
        r.first.fileName === "map.png" && r.second.fileName === "map-marked.png"
        r.first.location !== r.second.location
        Number(r.reply.replyToEventSeq) === Number(r.sent.eventSeq)
    }
})

feature("a snapshot stores its locked frame as an attachment and its annotations as editable shapes", {
    when() {
        ec.user.internalLoginUser("john.doe")
        const call = "org.moqui.call.CallServices."
        boardCall(call+"register#MediaNode",{instanceId:"board-snapshot-spec",mediaAddress:"127.0.0.1:20101"})
        const {chatRoomId} = boardCall("org.moqui.chat.ChatServices.create#Room",{roomName:"Snapshot review",roomTypeEnumId:"CrtPrivate"})
        const {callSessionId} = boardCall(call+"start#Call",{chatRoomId})
        const scene = {type:"excalidraw",version:2,files:{},elements:[
            {id:"frame",type:"image",fileId:"capture",locked:true,x:0,y:0,width:10,height:10},
            {id:"stroke",type:"line",x:0,y:0,width:5,height:5,points:[[0,0],[5,5]]}]}
        const snapshot = () => boardCall("org.moqui.comms.AnnotationServices.snapshot#Board",{callSessionId,pageName:"Snapshot specimen",
            frameFile:uploadedFile("capture.png","pixels","image/png"),scene:JSON.stringify(scene)})
        const result = snapshot(), second = snapshot()
        if (result.error || second.error) throw Error(result.error || second.error)
        const read = boardCall("org.moqui.comms.BoardServices.get#Board",{wikiPageId:result.wikiPageId})
        const info = boardCall("org.moqui.resource.WikiServices.get#WikiPageInfo",{wikiPageId:result.wikiPageId})
        boardCall(call+"end#Call",{callSessionId})
        return {scene:JSON.parse(read.scene), info,second:boardCall("org.moqui.comms.BoardServices.get#Board",{wikiPageId:second.wikiPageId})}
    },
    then(r) {
        r.second.title === "Snapshot specimen (2)"
        r.scene.elements[0].locked === true && !r.scene.elements[1].locked
        r.scene.files.capture.attachmentName === "capture.png"
        r.info.attachmentList.some(file=>file.filename === "capture.png")
    }
})

feature("a saved board version takes its preview, links from markdown and leaves out files its scene dropped", {
    when() {
        ec.user.internalLoginUser("john.doe")
        const wiki = "org.moqui.resource.WikiServices."
        const created = boardCall("org.moqui.comms.BoardServices.create#Board", {pageName:"Pruned sketch"})
        const wikiPageId = created.wikiPageId
        const info = boardCall(wiki + "get#WikiPageInfo", {wikiPageId})
        const upload = name => boardCall(wiki + "upload#WikiPageAttachment",
            {wikiPageId, attachmentFile:uploadedFile(name, "pixels", "image/png")})
        const uploads = [upload("used.png"), upload("stale.png")].map(u => u.error)
        const scene = {type:"excalidraw", version:2, elements:[{id:"photo", type:"image", fileId:"f1", x:0, y:0, width:4, height:4,
            version:1, versionNonce:1, index:"a0"}], appState:{}, files:{f1:{id:"f1", attachmentName:"used.png", mimeType:"image/png"}}}
        const page = boardCall(wiki + "update#WikiPage", {wikiPageId, wikiSpaceId:info.wikiSpaceId, pagePath:info.pagePath,
            pageName:info.pageName, pageText:JSON.stringify(scene)})
        const preview = '<svg xmlns="http://www.w3.org/2000/svg"><text>Client preview</text></svg>'
        const saved = boardCall("org.moqui.comms.BoardServices.save#Board", {wikiPageId, note:"Checked", preview, scene:JSON.stringify(scene)})
        // the files themselves, as the board's services read them
        ec.artifactExecution.disableAuthz()
        const shown = boardCall(wiki + "get#WikiPageAttachment", {wikiPageId, filename:"_board.svg"}).attachmentReference.getText()
        const removed = ec.resource.script("component://framework/service/org/moqui/resource/WikiBoard.js")
            .pruneFiles(wikiPageId, saved.versionName || page.versionName, 0)
        const after = boardCall(wiki + "get#WikiPageInfo", {wikiPageId}).attachmentList.map(file => file.filename)
        const notes = boardCall(wiki + "update#WikiPage", {wikiSpaceId:info.wikiSpaceId, pageName:"Sketch notes",
            pageText:"See [[" + info.pagePath + "]]"})
        const html = withConf("comms@app-root", "/apps/Comms", () =>
            boardCall(wiki + "render#WikiPage", {wikiPageId:notes.wikiPageId}).pageHtml)
        ec.artifactExecution.enableAuthz()
        return {uploads, page, saved, shown, removed:Array.from(removed), after, html, wikiPageId}
    },
    then(r) {
        JSON.stringify(r.uploads) === "[\"\",\"\"]" && r.page.error === "" && r.saved.error === ""
        r.shown.includes("Client preview")
        JSON.stringify(r.removed) === '["stale.png"]'
        r.after.includes("used.png") && !r.after.includes("stale.png") && r.after.includes("_board.svg")
        r.html.includes('href="/apps/Comms/Boards/Board?wikiPageId=' + r.wikiPageId)
    }
})
