// A call opens a scoped view of a wiki page. The row records who supplied the
// board, so changing the presenter does not transfer their wiki permissions.
const CB = 'moqui.call.CallBoard'
const wiki = 'org.moqui.resource.WikiServices.'
const rooms = 'org.moqui.chat.RoomBoardServices.'
const invoke = (name, params) => ec.service.sync().name(name).parameters(params).disableAuthz().call() || {}
const find = (name, fields) => ec.entity.find(name).condition(fields).disableAuthz()
function readable(id, userId) {
    if (!id) return false
    const access = invoke(wiki+'get#WikiPageAccess',{wikiPageId:id,userId:userId || ec.user.userId})
    if (!access.canView) return false
    const page = find('moqui.resource.wiki.WikiPage',{wikiPageId:id}).one()
    if (!page) return false
    const file = invoke(wiki+'get#WikiPageFile',{wikiSpaceId:page.wikiSpaceId,pagePath:page.pagePath})
    return file.exists && String(file.pageLocation).endsWith('.excalidraw')
}
function asMember(id) {
    const session = find('moqui.call.CallSession',{callSessionId:id}).forUpdate(true).one()
    const member = session && ec.user.userId && find('moqui.call.CallParticipant',{callSessionId:id,userId:ec.user.userId,admissionStatusId:'CasAdmitted'}).one()
    if (!member) { ec.message.addError('Call not found'); return null }
    if (session.chatRoomId) {
        const rights = invoke('org.moqui.chat.ChatServices.get#RoomAccess',{chatRoomId:session.chatRoomId})
        if (!rights.isMember || rights.member?.leftDate) { ec.message.addError('Room not found'); return null }
    }
    return {session,member}
}
function open(session,pageId,legId) {
    const participant = find('moqui.call.CallLeg',{callLegId:legId}).one()
    const user = participant && find('moqui.call.CallParticipant',{callParticipantId:participant.callParticipantId}).one()?.userId
    if (!user || !readable(pageId,user)) return null
    const keys = {callSessionId:session.callSessionId,wikiPageId:pageId}
    let record = find(CB,keys).one()
    const write = invoke(wiki+'get#WikiPageAccess',{wikiPageId:pageId,userId:user}).canUpdate
    const alreadyOpen = record && !record.closedDate
    if (!alreadyOpen) {
        const fresh = !record
        if (fresh) record = ec.entity.makeValue(CB)
        record.setFields({...keys,openedDate:ec.user.nowTimestamp,openedByUserId:user,closedDate:null,
            memberCanEdit:write ? 'Y' : 'N',guestCanEdit:write ? 'Y' : 'N'},true,null,null)
        if (fresh) record.create(); else record.update()
        if (session.chatRoomId) {
            const access=invoke('org.moqui.chat.ChatServices.get#RoomAccess',{chatRoomId:session.chatRoomId,userId:user})
            if (access.canPost && user === ec.user.userId) invoke(rooms+'post#RoomBoard',{chatRoomId:session.chatRoomId,wikiPageId:pageId})
        }
    }
    return {wikiPageId:pageId,memberCanEdit:record.memberCanEdit==='Y',guestCanEdit:record.guestCanEdit==='Y'}
}
function close(session,id) {
    if (!id) return
    const row = find(CB,{callSessionId:session.callSessionId,wikiPageId:id}).one()
    if (row && !row.closedDate) { row.set('closedDate',ec.user.nowTimestamp); row.update() }
}
function finish(session) {
    for (const row of find(CB,{callSessionId:session.callSessionId}).list()) {
        ec.message.pushErrors()
        try {
            invoke('org.moqui.resource.CollabServices.save#MeetingBoard',{callSessionId:session.callSessionId,wikiPageId:row.wikiPageId})
            if (ec.message.hasError()) ec.logger.error('Could not save call board '+row.wikiPageId+': '+ec.message.errorsString)
        } catch(e) { ec.logger.error('Could not save call board '+row.wikiPageId+': '+e) }
        finally { ec.message.clearErrors(); ec.message.popErrors() }
        close(session,row.wikiPageId)
    }
}
function list(p) {
    const actor=asMember(p.callSessionId)
    if (!actor) return {}
    const {session}=actor
    const result=session.chatRoomId ? invoke(rooms+'find#RoomBoards',{chatRoomId:session.chatRoomId}) : {boards:[],canCreate:session.startedByUserId===ec.user.userId}
    const shown=[]
    for (const row of find(CB,{callSessionId:p.callSessionId}).orderBy('-openedDate').list()) {
        const visible=readable(row.wikiPageId)
        if (!visible && (row.closedDate || session.statusId === 'CssEnded')) continue
        const page=find('moqui.resource.wiki.WikiPage',{wikiPageId:row.wikiPageId}).one()
        if (page) shown.push({wikiPageId:row.wikiPageId,title:page.pagePath,openedDate:Number(row.openedDate),closedDate:row.closedDate ? Number(row.closedDate) : null})
    }
    return {boards:result.boards,shown,canCreate:!!result.canCreate && session.statusId !== 'CssEnded'}
}
function create(p) {
    const actor=asMember(p.callSessionId)
    if (!actor || actor.session.statusId==='CssEnded') { ec.message.addError('Meeting has ended'); return {} }
    const session=actor.session
    if (session.chatRoomId) return invoke(rooms+'create#RoomBoard',{chatRoomId:session.chatRoomId,pageName:p.pageName})
    // The starter owns the notes space. Only someone with existing update access can create a file there.
    const space=invoke(wiki+'ensure#UserNotesSpace',{userId:session.startedByUserId})
    return invoke(wiki+'create#WikiBoard',{wikiSpaceId:space.wikiSpaceId,pageName:p.pageName})
}
function drawing(p, engine) {
    const actor=asMember(p.callSessionId)
    if (!actor) return {}
    const {session,member}=actor
    const stage=JSON.parse(session.stageState || '{}')
    if (session.statusId==='CssEnded' || member.roleEnumId!=='CprHost' || stage.kind!=='board' || stage.wikiPageId!==p.wikiPageId) {
        ec.message.addError('Only the host can change drawing on the staged board')
        return {}
    }
    const row=find(CB,{callSessionId:p.callSessionId,wikiPageId:p.wikiPageId,closedDate:null}).one()
    if (!row) { ec.message.addError('Board has closed'); return {} }
    const editable=invoke(wiki+'get#WikiPageAccess',{wikiPageId:p.wikiPageId,userId:row.openedByUserId}).canUpdate
    stage.memberCanEdit=!!editable && p.memberCanEdit === true
    stage.guestCanEdit=!!editable && p.guestCanEdit === true
    row.setFields({memberCanEdit:stage.memberCanEdit?'Y':'N',guestCanEdit:stage.guestCanEdit?'Y':'N'},true,null,null)
    row.update()
    session.set('stageState',JSON.stringify(stage))
    engine.bump(session)
    session.update()
    engine.publishStage(session)
    return {memberCanEdit:stage.memberCanEdit,guestCanEdit:stage.guestCanEdit}
}
;({readable,open,close,finish,list,create,drawing})
