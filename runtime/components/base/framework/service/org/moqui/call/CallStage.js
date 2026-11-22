// The one presentation stage of a live call. Changes serialize on the same session row as
// admission and departure. Each media node receives the committed revision and fences older ones.
const boards = ec.resource.script('component://framework/service/org/moqui/call/CallBoards.js')
const empty = () => ({kind:null, callLegId:null, requests:[]})
function read(session) {
    if (!session.stageState) return empty()
    try {
        const stage = JSON.parse(session.stageState)
        if (stage && typeof stage === 'object' && Array.isArray(stage.requests)) return stage
    } catch (_) { /* logged below */ }
    // one unreadable stage must not stop the call: it is logged and treated as nobody presenting
    ec.logger.error(`Call ${session.callSessionId} has an unreadable stage, treated as empty: ${String(session.stageState).slice(0, 200)}`)
    return empty()
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
    if (stage.kind !== kind || stage.callLegId !== leg) {
        if (kind === 'follow') stage.followEpoch = Number(session.revision || 0) + 1
        else delete stage.followEpoch
        delete stage.inkMode
        delete stage.inkEmbedded
        if (kind === 'screen') stage.inkEpoch = Number(session.revision || 0) + 1
        else delete stage.inkEpoch
    }
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
/** A leg's page connected again (a reload, a new socket): its screen share is gone with the old page, so its screen stage ends. */
function rebound(session, legId) {
    const stage = read(session)
    if (stage.kind !== 'screen' || stage.callLegId !== legId) return false
    return store(session, Object.assign(empty(), {requests:stage.requests, requested:stage.requested}))
}

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
        // the same members whose pages may follow (Calls.js bindLeg memberScreen)
        if (p.kind === 'follow' && (p.credential || !actor.userId || actor.identityKindEnumId !== 'CikLocal' ||
                one('CallLeg',{callLegId:leg})?.legTypeEnumId !== 'CltWeb')) return engine.refuse('notAllowed','Only a member can present an application screen')
        if (!['board','screen','follow'].includes(p.kind || 'screen')) return engine.refuse('badRequest', 'Unknown presentation type')
        if (mode === 'CsmHosts' && !host) return engine.refuse('notAllowed', 'Only hosts may present')
        if (mode === 'CsmAsk' && !host && stage.callLegId !== leg) {
            if (!stage.requests.includes(leg)) stage.requests.push(leg)
            stage.requested = Object.assign(stage.requested || {},{[leg]:{kind:p.kind || 'screen',wikiPageId:p.kind === 'board' ? p.wikiPageId : null}})
        } else {
            if (stage.callLegId && stage.callLegId !== leg && !host) {
                if (personOf(stage.callLegId)?.roleEnumId === 'CprHost') return engine.refuse('notAllowed', 'Only a host may take over from a host')
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
    } else if (p.action === 'inkMode' || p.action === 'inkSurface') {
        if (stage.kind !== 'screen' || (stage.callLegId !== leg && (!host || p.action === 'inkSurface')))
            return engine.refuse('notAllowed','Only the presenter or a host may manage share annotation')
        if (p.action === 'inkMode') {
            if (!['off','hosts','everyone'].includes(p.mode)) return engine.refuse('badRequest','Unknown drawing permission')
            stage.inkMode = p.mode
        } else {
            stage.inkEmbedded = p.embedded === true
        }
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
    let current = stage
    if (store(session, stage)) {
        // bump drops the legs that left, and publishes the stage itself when that changed it
        const published = engine.bump(session)
        session.update()
        current = read(session)
        if (!published) engine.mediaControl(session, {action:'stage', stage:current, revision:Number(session.revision), shareModeEnumId:mode})
    }
    return {stage:current, stageRevision:Number(session.revision), shareModeEnumId:mode}
}
;({read, end, leave, rebound, setMode, change, recurring})
