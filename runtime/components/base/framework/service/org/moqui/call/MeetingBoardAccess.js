// A meeting grants access to the board opened in it, on behalf of the person who opened it.
// The signed-in wiki page, its history and other files in the wiki space have their own grants.
const find = (name, map) => ec.entity.find(name).disableAuthz().condition(map)
const call = (name, params) => ec.service.sync().name(name).parameters(params).disableAuthz().call() || {}
const denied = () => ({canView:false, canEdit:false})
function grant(p) {
    const row = find('moqui.call.CallBoard', {callSessionId:p.callSessionId, wikiPageId:p.wikiPageId}).one()
    if (!row || row.closedDate) return denied()
    const session = find('moqui.call.CallSession', {callSessionId:p.callSessionId}).one()
    const leg = find('moqui.call.CallLeg', {callSessionId:p.callSessionId, callLegId:p.callLegId}).one()
    if (!session || session.statusId === 'CssEnded' || !leg || leg.statusId !== 'ClsAnswered') return denied()
    const actor = find('moqui.call.CallParticipant', {callParticipantId:leg.callParticipantId}).one()
    if (!actor || actor.admissionStatusId !== 'CasAdmitted') return denied()
    let guest = false
    if (p.credential) {
        const identity = call('org.moqui.call.CallServices.get#GuestBoardIdentity', {credential:p.credential})
        if (!identity.callGuestId || identity.callGuestId !== actor.callGuestId || identity.callSessionId !== p.callSessionId)
            return denied()
        guest = true
    } else if (!p.userId || actor.userId !== p.userId) return denied()
    // A room member leaving loses its boards, including an old connected call socket.
    if (!guest && session.chatRoomId) {
        const member = find('moqui.chat.ChatRoomMember', {chatRoomId:session.chatRoomId, userId:p.userId}).one()
        if (!member || member.leftDate) return denied()
    }
    const wiki = 'org.moqui.resource.WikiServices.get#WikiPageAccess'
    const opener = call(wiki, {wikiPageId:p.wikiPageId,userId:row.openedByUserId})
    if (!opener.canView) return denied()
    const host = session.startedByUserId
    const hostGrant = host ? call(wiki, {wikiPageId:p.wikiPageId,userId:host}) : {}
    const edit = guest ? row.guestCanEdit !== 'N' : actor.roleEnumId === 'CprHost' || row.memberCanEdit !== 'N'
    return {canView:true, canEdit:!!(edit && opener.canUpdate && hostGrant.canUpdate && !opener.readOnly),
        storage:opener.storage, writerUserId:host, displayName:guest ? (actor.displayName || 'Guest') + ' (guest)' :
            (actor.displayName || find('moqui.security.UserAccount', {userId:p.userId}).one()?.userFullName || p.userId), guest}
}

function file(p) {
    if (p.credential) {
        const link = call('org.moqui.call.CallServices.check#CallLinkToken', {token:p.token})
        const identity = call('org.moqui.call.CallServices.get#GuestBoardIdentity', {credential:p.credential})
        const actual = identity.callLinkId && find('moqui.call.CallLink', {callLinkId:identity.callLinkId}).one()
        if (!link.valid || !actual || actual.tokenHash !== ec.user.singleUseCodeHash(String(p.token))) {
            return {denied:true}
        }
        const state = call('org.moqui.call.CallServices.get#GuestState', {credential:p.credential})
        p = {...p, callSessionId:state.callSessionId, callLegId:state.callLegId}
    } else {
        p = {...p, userId:ec.user.userId}
    }
    const access = grant(p), upload = p.attachmentFile
    if (!access.canView || (upload && !access.canEdit)) return {denied:true}
    const filename = String(upload ? upload.getName() : p.filename || '')
    if (!/^[\w.-]+\.(png|jpe?g|gif|webp|svg)$/i.test(filename) || filename.startsWith('_board')) {
        ec.message.addError('Invalid board image name')
        return {}
    }
    return ec.artifactExecution.runAsSystem(() => {
        const prefix = 'org.moqui.resource.WikiServices.'
        const wikiPageId = p.wikiPageId
        if (upload) {
            // the system uploads it, so its version says for whom
            const versionNote = ec.l10n.localize('Uploaded in a meeting by ${name}').replace('${name}', access.displayName || '')
            call(prefix + 'upload#WikiPageAttachment', {wikiPageId, attachmentFile:upload, versionNote})
            return ec.message.hasError() ? {} : {filename}
        }
        const ref = call(prefix + 'get#WikiPageAttachment', {wikiPageId, filename}).attachmentReference
        if (!ref?.exists || ref.getSize() > Number(ec.factory.getConfValue('board@max-image-bytes'))) {
            return {denied:true}
        }
        if (p.locationOnly) return {imageLocation:ref.location}
        const stream = ref.openStream()
        try { return {mimeType:ref.getContentType(), base64:StringUtilities.base64Encode(stream.readAllBytes())} }
        finally { stream.close() }
    })
}
;({grant,file})
