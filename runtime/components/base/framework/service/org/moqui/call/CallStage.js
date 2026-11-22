// The one presentation stage of a live call. Changes serialize on the same session row as
// admission and departure. Each media node receives the committed revision and fences older ones.
const boards = ec.resource.script('component://framework/service/org/moqui/call/CallBoards.js')
const empty = () => ({kind:null, callLegId:null, requests:[]})
function read(session) {
    return session.stageState ? JSON.parse(session.stageState) : empty()
}
function store(session, stage) {
    if (stage.requested) for (const key of Object.keys(stage.requested)) if (!stage.requests.includes(key)) delete stage.requested[key]
    const value = stage.callLegId || stage.requests.length ? JSON.stringify(stage) : null
    if ((session.stageState || null) === value) return false
    const old = read(session)
    if (old.wikiPageId && old.wikiPageId !== stage.wikiPageId) boards.close(session,old.wikiPageId)
    session.set('stageState', value)
    return true
}
function leave(session, legId) {
    const stage = read(session)
    stage.requests = stage.requests.filter(id => id !== legId)
    if (stage.callLegId === legId) return store(session, Object.assign(empty(), {requests:stage.requests}))
    return store(session, stage)
}
function present(session,stage,leg,kind,pageId) {
    let data={}
    if (kind==='board') {
        data=boards.open(session,pageId,leg)
        if (!data) return false
    }
    for (const key of ['wikiPageId','memberCanEdit','guestCanEdit']) delete stage[key]
    Object.assign(stage,data,{kind,callLegId:leg})
    return true
}
function recurring(session,leg,pageId) {
    const stage=read(session)
    return present(session,stage,leg,'board',pageId) && store(session,stage)
}
function end(session) { return store(session, empty()) }

const one = (entity, fields) => ec.entity.find('moqui.call.' + entity).condition(fields).disableAuthz().one()
function personOf(legId) {
    const leg = one('CallLeg', {callLegId:legId})
    return leg?.statusId === 'ClsAnswered' ? one('CallParticipant', {callParticipantId:leg.callParticipantId}) : null
}
function setMode(session, mode) {
    const stage = read(session)
    stage.requests = []
    if (mode !== 'CsmAnyone' && stage.callLegId && personOf(stage.callLegId)?.roleEnumId !== 'CprHost') return end(session)
    return store(session, stage)
}

function change(p, engine) {
    const session = engine.lockSession(p.callSessionId)
    const actor = personOf(p.callLegId)
    if (!session || session.statusId === 'CssEnded' || !actor || actor.callSessionId !== p.callSessionId ||
            actor.admissionStatusId !== 'CasAdmitted') return engine.refuse('notJoined', 'Join the call before presenting')
    if (p.credential) {
        if (engine.guestOf(p.credential)?.participant?.callParticipantId !== actor.callParticipantId)
            return engine.refuse('notAllowed', 'This guest cannot present for that leg')
    } else if (!ec.user.userId || actor.userId !== ec.user.userId) {
        return engine.refuse('notAllowed', 'This is another participant’s leg')
    }
    if (!one('CallConnection', {callLegId:p.callLegId, generation:p.generation, statusId:'CcsActive'}))
        return engine.refuse('staleGeneration', 'Join the call on this connection first')
    const mode = session.shareModeEnumId || 'CsmAnyone', stage = read(session)
    const host = actor.roleEnumId === 'CprHost'
    const leg = p.callLegId, target = p.targetLegId || leg
    if (p.action === 'ask' || p.action === 'take') {
        if (p.kind === 'board' && (!actor.userId || !boards.readable(p.wikiPageId,actor.userId)))
            return engine.refuse('notFound','Board not found')
        if (!['board','screen'].includes(p.kind || 'screen')) return engine.refuse('badRequest', 'Unknown presentation type')
        if (mode === 'CsmHosts' && !host) return engine.refuse('notAllowed', 'Only hosts may present')
        if (mode === 'CsmAsk' && !host && stage.callLegId !== leg) {
            if (!stage.requests.includes(leg)) stage.requests.push(leg)
            stage.requested = Object.assign(stage.requested || {},{[leg]:{kind:p.kind || 'screen',wikiPageId:p.wikiPageId || null}})
        } else {
            if (stage.callLegId && stage.callLegId !== leg && !host) {
                if (p.action !== 'take') return engine.refuse('takeoverRequired', 'Confirm who you want to take over from')
                if (p.previousLegId !== stage.callLegId) return engine.refuse('stageChanged', 'The presenter changed')
            }
            if (!present(session,stage,leg,p.kind || 'screen',p.wikiPageId)) return engine.refuse('notFound','Board not found')
            stage.requests = stage.requests.filter(id => id !== leg)
        }
    } else if (p.action === 'approve') {
        if (!host) return engine.refuse('notAllowed', 'Only hosts may approve a presentation')
        const presenter = personOf(target)
        if (!stage.requests.includes(target) || !presenter || presenter.callSessionId !== session.callSessionId ||
                presenter.admissionStatusId !== 'CasAdmitted') return engine.refuse('stageChanged', 'The request has ended')
        const req=stage.requested?.[target] || {kind:'screen'}
        if (!present(session,stage,target,req.kind,req.wikiPageId)) return engine.refuse('notFound','Board not found')
        stage.requests = stage.requests.filter(id => id !== target)
    } else if (p.action === 'lower') {
        if (target !== leg && !host) return engine.refuse('notAllowed', 'Only a host may dismiss another request')
        stage.requests = stage.requests.filter(id => id !== target)
    } else if (p.action === 'stop') {
        if (!host && stage.callLegId !== leg) return engine.refuse('notAllowed', 'Only the presenter or host may stop a presentation')
        if (p.previousLegId && p.previousLegId !== stage.callLegId) return engine.refuse('stageChanged', 'The presenter changed')
        present(session,stage,null,null,null)
    } else {
        return engine.refuse('badRequest', 'Unknown stage action')
    }
    if (store(session, stage)) {
        engine.bump(session)
        session.update()
        engine.mediaControl(session, {action:'stage', stage, revision:Number(session.revision), shareModeEnumId:mode})
    }
    return {stage, stageRevision:Number(session.revision), shareModeEnumId:mode}
}
;({read, end, leave, setMode, change, recurring})
