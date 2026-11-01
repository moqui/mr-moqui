// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Call queues (plans/Comms-Framework.md §5.3, §5.4, V5): the services of org.moqui.impl.CallQueueServices. A call enters a
// queue from a number routed to it (PnrQueue), an IVR option or a transfer (enter): a CallQueueEntry, waiting. The SIP
// engine plays the caller hold music and asks tick#QueueCall every few seconds (call_queue_tick_seconds), which offers the
// waiting calls in order to the queue's free agents by its strategy (the agent idle longest, round robin, or all at
// once), each offer ringing the agents' tabs and phones as any call does (Calls.js ringStep: a CallUserLog row as queue
// agent, missed when the offer rings out); the first answer wins (Calls.js), and the entry is answered. An agent logged
// out, paused, in a call or in wrap-up (the queue's wrapUpSeconds after their last queue call) is skipped. Every
// announceSeconds the caller hears their place (the engine's tone pattern, the queue's position prompt and the
// deployment's digit prompts when uploaded). A call that waited maxWaitSeconds, one that finds maxLength calls waiting,
// or one outside the queue's schedule goes to its fallback (Ivr.js routeTarget: voicemail, a user, a group, a menu, a
// number, another queue, or the end). Times are now, or atMs (the specs' simulated time).
//
// Queues and their agents are kept by voice admins (VoiceAdmin); an agent logs in, out and pauses themselves.

const CALLS = 'component://framework/service/org/moqui/impl/Calls.js'
const CALL_HANDLING = 'component://framework/service/org/moqui/impl/CallHandling.js'
const IVR = 'component://framework/service/org/moqui/impl/Ivr.js'
const SESSION = 'moqui.call.CallSession'
const QUEUE = 'moqui.call.CallQueue'
const AGENT = 'moqui.call.CallQueueAgent'
const ENTRY = 'moqui.call.CallQueueEntry'
const NUMBER = 'moqui.call.PhoneNumber'
const STATUS = 'moqui.security.UserStatus'
const ACCOUNT = 'moqui.security.UserAccount'
const SCHEDULE = 'moqui.call.CommsSchedule'

const STRATEGIES = ['CqsLongestIdle', 'CqsRoundRobin', 'CqsAllAtOnce']
/** How deep queues and menus send a call to one another before it ends (a queue whose fallback is a queue ...). */
const MAX_DEPTH = 10
/** Defaults and bounds: [default, low, high]. */
const LIMITS = {
  ringSeconds: [20, 5, 120], maxWaitSeconds: [300, 10, 7200], maxLength: [20, 1, 500], wrapUpSeconds: [30, 0, 600],
  announceSeconds: [60, 0, 600],
}

const ms = (value) => {
  if (value == null || value === '') return null
  const number = Number(value)
  return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}
const nowMs = () => ms(ec.user.nowTimestamp)
const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const rows = (entity, fields) => Array.from(find(entity).condition(fields || {}).list())
const blank = (value) => value == null || String(value).trim() === ''
const clamp = (value, low, high) => Math.min(Math.max(value, low), high)
const intOr = (value, fallback) => (blank(value) || Number.isNaN(Number(value)) ? fallback : Math.round(Number(value)))
const limit = (queue, key) => clamp(intOr(queue[key], LIMITS[key][0]), LIMITS[key][1], LIMITS[key][2])
const tickSeconds = () => clamp(intOr(System.getProperty('call_queue_tick_seconds'), 5), 1, 30)
const atOf = (p) => (blank(p.atMs) ? nowMs() : Number(p.atMs))

function calls(op, parameters) {
  return ec.resource.script(CALLS).execute(op, parameters) || {}
}
function ivr(op, parameters) {
  return ec.resource.script(IVR).execute(op, parameters) || {}
}

/** A refusal: an error for the user and a reason code for the caller. */
function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

function isAdmin() {
  return !!ec.user.userId && ec.user.hasPermission('VoiceAdmin')
}
function notAdmin() {
  return isAdmin() ? null : refuse('notAllowed', 'Only a voice administrator may do this')
}

function userName(userId) {
  const account = userId ? one(ACCOUNT, { userId }) : null
  return account ? account.userFullName || account.username || userId : userId
}

// ---------- agents ----------

/** The users with an offer of a waiting queue call ringing now: an agent takes one offer at a time. */
function offeredUsers() {
  const out = new Set()
  for (const entry of Array.from(find(ENTRY).condition('statusId', 'CqeWaiting').condition('offeredDate', '!=', null)
    .selectField('offerUserIds').limit(1000).list())) {
    for (const id of String(entry.offerUserIds || '').split(',')) if (id) out.add(id)
  }
  return out
}

/** Why an agent takes no call now, or null when they are free: out, paused, wrapUp, inCall, offered, disabled. */
function agentBusy(agent, atMs, offered) {
  if (agent.loggedIn !== 'Y') return 'out'
  if (agent.paused === 'Y') return 'paused'
  if (agent.wrapUpUntilDate && ms(agent.wrapUpUntilDate) > atMs) return 'wrapUp'
  const account = one(ACCOUNT, { userId: agent.userId })
  if (!account || account.disabled === 'Y') return 'disabled'
  if (one(STATUS, { userId: agent.userId })?.inCall === 'Y') return 'inCall'
  if (offered.has(agent.userId)) return 'offered'
  return null
}

/**
 * The agents an offer rings, by the queue's strategy: one (longest idle, round robin) or every free one (all at once).
 * Agents who missed this call already (skip) are tried again only when nobody else is free.
 */
function choose(queue, atMs, offered, skip) {
  const agents = rows(AGENT, { callQueueId: queue.callQueueId })
    .sort((a, b) => (intOr(a.sequenceNum, 0) - intOr(b.sequenceNum, 0)) || String(a.userId).localeCompare(String(b.userId)))
  const anyFree = agents.filter((a) => !agentBusy(a, atMs, offered))
  const fresh = anyFree.filter((a) => !skip || !skip.has(a.userId))
  const free = fresh.length ? fresh : anyFree
  if (!free.length) return []
  switch (queue.strategyEnumId) {
    case 'CqsAllAtOnce': return free.map((a) => a.userId)
    case 'CqsRoundRobin': {
      const order = agents.map((a) => a.userId)
      const last = order.indexOf(queue.lastAgentUserId)
      const rotated = last < 0 ? order : order.slice(last + 1).concat(order.slice(0, last + 1))
      const next = rotated.find((id) => free.some((a) => a.userId === id))
      return next ? [next] : []
    }
    default: {
      // the agent idle longest: since their last queue call ended, else since they logged in
      const idleSince = (a) => ms(a.lastCallEndDate) || ms(a.loggedInDate) || 0
      return [free.slice().sort((a, b) => idleSince(a) - idleSince(b))[0].userId]
    }
  }
}

/** Offers one waiting entry to the queue's free agents; false when none is free. */
function offerEntry(queue, entry, atMs, offered) {
  // one at a time, an agent who missed this call is offered it again only when nobody else is free
  const missed = queue.strategyEnumId === 'CqsAllAtOnce' ? null
    : new Set(rows('moqui.call.CallUserLog', { callSessionId: entry.callSessionId, outcomeEnumId: 'CuoMissed' }).map((l) => l.userId))
  const userIds = choose(queue, atMs, offered, missed)
  if (!userIds.length) return false
  const session = one(SESSION, { callSessionId: entry.callSessionId })
  if (!session || session.statusId === 'CssEnded') return true
  calls('ringStep', { callSessionId: entry.callSessionId, userIdList: userIds, role: 'queueAgent',
    ringSeconds: limit(queue, 'ringSeconds') + 10, displayName: null })
  const at = new Date(atMs)
  entry.setFields({ offeredDate: at, offerUserIds: userIds.join(',') }, true, null, null)
  entry.update()
  for (const userId of userIds) {
    offered.add(userId)
    const agent = one(AGENT, { callQueueId: queue.callQueueId, userId })
    if (agent) {
      agent.set('lastOfferDate', at)
      agent.update()
    }
  }
  if (queue.strategyEnumId === 'CqsRoundRobin') {
    const row = find(QUEUE).condition('callQueueId', queue.callQueueId).forUpdate(true).one()
    row.set('lastAgentUserId', userIds[0])
    row.update()
    queue.lastAgentUserId = userIds[0]
  }
  return true
}

/** The queue's waiting calls, oldest first. */
function waiting(callQueueId) {
  return Array.from(find(ENTRY).condition('callQueueId', callQueueId).condition('statusId', 'CqeWaiting').orderBy('enteredDate')
    .orderBy('callQueueEntryId').list())
}

/** Offers the waiting calls that have no offer, oldest first, while agents are free. */
function offerWaiting(queue, atMs) {
  const offered = offeredUsers()
  for (const entry of waiting(queue.callQueueId)) {
    if (entry.offeredDate) continue
    if (!offerEntry(queue, entry, atMs, offered)) break
  }
}

/** An offer that rang ringSeconds with no answer ends: the agents rung missed it (Calls.js closeOffers). */
function closeOffer(entry) {
  calls('closeOffers', { callSessionId: entry.callSessionId })
  entry.setFields({ offeredDate: null, offerUserIds: null }, true, null, null)
  entry.update()
}

function stats(queue, atMs) {
  const entries = waiting(queue.callQueueId)
  const agents = rows(AGENT, { callQueueId: queue.callQueueId })
  const offered = offeredUsers()
  return {
    waitingCount: entries.length,
    longestWaitSeconds: entries.length ? Math.max(0, Math.round((atMs - ms(entries[0].enteredDate)) / 1000)) : 0,
    agentsLoggedIn: agents.filter((a) => a.loggedIn === 'Y').length,
    agentsAvailable: agents.filter((a) => !agentBusy(a, atMs, offered)).length,
  }
}

// ---------- calls in a queue (internal: Ivr.js, the SIP engine, Calls.js) ----------

/** The engine's answer for a waiting call: hold music, the next tick, and the caller's place when it is due. */
function queueAnswer(queue, entry, atMs, announce) {
  const place = waiting(queue.callQueueId).findIndex((e) => e.callQueueEntryId === entry.callQueueEntryId) + 1
  if (announce) {
    entry.set('lastAnnounceDate', new Date(atMs))
    entry.update()
  }
  return {
    mode: 'queue', callQueueEntryId: entry.callQueueEntryId, holdMusicLocation: queue.holdMusicLocation || null,
    positionPromptLocation: queue.positionPromptLocation || null, tickSeconds: tickSeconds(), position: place,
    announce: !!announce && place > 0,
  }
}

/** The queue's fallback for a call: Ivr.js routeTarget, or the end; depth bounds queues and menus that send to each other. */
function fallback(queue, entry, ctx, why) {
  ec.logger.info(`Call ${ctx.callSessionId}: queue ${queue.name || queue.callQueueId} ${why}; to its fallback`)
  if (entry) {
    if (entry.offeredDate) closeOffer(entry)
    entry.setFields({ statusId: 'CqeOverflow', endedDate: new Date(ctx.atMs) }, true, null, null)
    entry.update()
  }
  if (blank(queue.fallbackTypeEnumId) || (ctx.depth || 0) >= MAX_DEPTH) return { mode: 'end' }
  return ivr('routeTarget', Object.assign({}, ctx, { targetTypeEnumId: queue.fallbackTypeEnumId, targetId: queue.fallbackId,
    currentMenuId: null, depth: (ctx.depth || 0) + 1 }))
}

/**
 * Internal (Ivr.js routeTarget, CallTransfers.js): the call (its phone leg callLegId) waits in the queue. Outside the
 * queue's schedule, or with maxLength calls waiting, it goes to the fallback instead. Offers at once when an agent is free.
 */
function enter(p) {
  const atMs = atOf(p)
  const queue = p.callQueueId ? one(QUEUE, { callQueueId: p.callQueueId }) : null
  const ctx = { callSessionId: p.callSessionId, callLegId: p.callLegId, fromNumber: p.fromNumber || null,
    displayName: p.displayName || null, atMs, depth: intOr(p.depth, 0) }
  if (!queue) {
    ec.logger.warn(`Call ${p.callSessionId}: queue ${p.callQueueId} not found; the call ends`)
    return { mode: 'end' }
  }
  if (queue.commsScheduleId) {
    const open = ec.resource.script(CALL_HANDLING).execute('scheduleOpen', { commsScheduleId: queue.commsScheduleId, atMs })?.open
    if (open === false) return fallback(queue, null, ctx, 'is closed')
  }
  if (waiting(queue.callQueueId).length >= limit(queue, 'maxLength')) return fallback(queue, null, ctx, 'is full')
  const entry = ec.entity.makeValue(ENTRY)
  entry.setSequencedIdPrimary()
  entry.setFields({ callQueueId: queue.callQueueId, callSessionId: p.callSessionId, callLegId: p.callLegId || null,
    fromNumber: p.fromNumber || null, statusId: 'CqeWaiting', enteredDate: new Date(atMs) }, true, null, null)
  entry.create()
  offerWaiting(queue, atMs)
  const current = one(ENTRY, { callQueueEntryId: entry.callQueueEntryId })
  return queueAnswer(queue, current, atMs, limit(queue, 'announceSeconds') > 0)
}

/**
 * Internal (the SIP engine, every few seconds while a call waits): an offer that rang out ends (missed) and the next is
 * made; past maxWaitSeconds the call goes to the fallback; every announceSeconds the caller hears their place. Answers
 * as enter, mode done when the call no longer waits (answered, ended), or where the fallback sends it.
 */
function tick(p) {
  const atMs = atOf(p)
  const entry = p.callQueueEntryId ? find(ENTRY).condition('callQueueEntryId', p.callQueueEntryId).forUpdate(true).one() : null
  if (!entry || entry.statusId !== 'CqeWaiting') return { mode: 'done' }
  const session = one(SESSION, { callSessionId: entry.callSessionId })
  if (!session || session.statusId === 'CssEnded') {
    entry.setFields({ statusId: 'CqeAbandoned', endedDate: new Date(atMs) }, true, null, null)
    entry.update()
    return { mode: 'done' }
  }
  const queue = one(QUEUE, { callQueueId: entry.callQueueId })
  if (!queue) return { mode: 'end' }
  const ctx = { callSessionId: entry.callSessionId, callLegId: entry.callLegId, fromNumber: entry.fromNumber || null, atMs, depth: 1 }
  if (atMs - ms(entry.enteredDate) >= limit(queue, 'maxWaitSeconds') * 1000) return fallback(queue, entry, ctx, 'waited too long')
  if (entry.offeredDate && atMs - ms(entry.offeredDate) >= limit(queue, 'ringSeconds') * 1000) closeOffer(entry)
  offerWaiting(queue, atMs)
  const current = one(ENTRY, { callQueueEntryId: entry.callQueueEntryId })
  if (current.statusId !== 'CqeWaiting') return { mode: 'done' }
  const every = limit(queue, 'announceSeconds')
  const last = ms(current.lastAnnounceDate) || ms(current.enteredDate)
  return queueAnswer(queue, current, atMs, every > 0 && atMs - last >= every * 1000)
}

/** Internal (Calls.js, when someone answers a call): a waiting entry of the call is answered by userId. */
function answered(p) {
  for (const entry of rows(ENTRY, { callSessionId: p.callSessionId, statusId: 'CqeWaiting' })) {
    entry.setFields({ statusId: 'CqeAnswered', answeredUserId: p.userId || null, answeredDate: ec.user.nowTimestamp,
      offeredDate: null, offerUserIds: null }, true, null, null)
    entry.update()
  }
  return {}
}

/** Internal (Calls.js, when an answered leg closes): the agent who answered a queue call goes into wrap-up. */
function legClosed(p) {
  if (!p.userId) return {}
  const now = nowMs()
  for (const entry of rows(ENTRY, { callSessionId: p.callSessionId, statusId: 'CqeAnswered', answeredUserId: p.userId })) {
    if (entry.endedDate) continue
    entry.set('endedDate', new Date(now))
    entry.update()
    const queue = one(QUEUE, { callQueueId: entry.callQueueId })
    const agent = find(AGENT).condition({ callQueueId: entry.callQueueId, userId: p.userId }).forUpdate(true).one()
    if (!queue || !agent) continue
    const wrap = limit(queue, 'wrapUpSeconds')
    agent.setFields({ lastCallEndDate: new Date(now), wrapUpUntilDate: wrap ? new Date(now + wrap * 1000) : null }, true, null, null)
    agent.update()
  }
  return {}
}

/** Internal (Calls.js, when a call ends): its waiting entries were abandoned. */
function sessionEnded(p) {
  for (const entry of rows(ENTRY, { callSessionId: p.callSessionId, statusId: 'CqeWaiting' })) {
    entry.setFields({ statusId: 'CqeAbandoned', endedDate: ec.user.nowTimestamp, offeredDate: null, offerUserIds: null }, true, null, null)
    entry.update()
  }
  return {}
}

// ---------- agents' own status ----------

function agentView(agent, queue, atMs, offered) {
  const why = agentBusy(agent, atMs, offered)
  return {
    callQueueId: agent.callQueueId, queueName: queue ? queue.name : agent.callQueueId, userId: agent.userId,
    userName: userName(agent.userId), sequenceNum: intOr(agent.sequenceNum, 0), loggedIn: agent.loggedIn === 'Y',
    paused: agent.paused === 'Y', wrapUpUntil: ms(agent.wrapUpUntilDate) > atMs ? ms(agent.wrapUpUntilDate) : null,
    state: why || 'available', lastCallEnd: ms(agent.lastCallEndDate),
  }
}

/** The queues the current user takes calls for, with their status and each queue's stats. */
function myQueues(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const atMs = atOf(p || {})
  const offered = offeredUsers()
  return {
    queueList: rows(AGENT, { userId: me }).map((agent) => {
      const queue = one(QUEUE, { callQueueId: agent.callQueueId })
      return Object.assign(agentView(agent, queue, atMs, offered), queue ? stats(queue, atMs) : {})
    }),
  }
}

/** The current user logs in to (status in), out of, pauses or resumes taking a queue's calls; all their queues without callQueueId. */
function setAgentStatus(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  if (!['in', 'out', 'pause', 'resume'].includes(p.status)) return refuse('badStatus', 'Log in, log out, pause or resume')
  const mine = rows(AGENT, { userId: me }).filter((a) => blank(p.callQueueId) || a.callQueueId === p.callQueueId)
  if (!mine.length) return refuse('notFound', 'You take no calls for that queue')
  const atMs = atOf(p)
  for (const row of mine) {
    const agent = find(AGENT).condition({ callQueueId: row.callQueueId, userId: me }).forUpdate(true).one()
    if (p.status === 'in') agent.setFields({ loggedIn: 'Y', paused: 'N', loggedInDate: agent.loggedIn === 'Y' ? agent.loggedInDate : new Date(atMs) }, true, null, null)
    else if (p.status === 'out') agent.setFields({ loggedIn: 'N', paused: 'N' }, true, null, null)
    else agent.set('paused', p.status === 'pause' ? 'Y' : 'N')
    agent.update()
    // a waiting call rings an agent who just came free
    if (p.status === 'in' || p.status === 'resume') {
      const queue = one(QUEUE, { callQueueId: row.callQueueId })
      if (queue) offerWaiting(queue, atMs)
    }
  }
  return myQueues({ atMs: p.atMs })
}

// ---------- administration (VoiceAdmin) ----------

function queueView(queue, atMs) {
  const offered = offeredUsers()
  const name = (type, id) => (type ? ivr('targetName', { targetTypeEnumId: type, targetId: id }).name : null)
  return Object.assign({
    callQueueId: queue.callQueueId, name: queue.name || queue.callQueueId, description: queue.description || null,
    strategyEnumId: queue.strategyEnumId || 'CqsLongestIdle', ringSeconds: limit(queue, 'ringSeconds'),
    maxWaitSeconds: limit(queue, 'maxWaitSeconds'), maxLength: limit(queue, 'maxLength'), wrapUpSeconds: limit(queue, 'wrapUpSeconds'),
    announceSeconds: limit(queue, 'announceSeconds'), hasHoldMusic: !blank(queue.holdMusicLocation),
    hasPositionPrompt: !blank(queue.positionPromptLocation), fallbackTypeEnumId: queue.fallbackTypeEnumId || null,
    fallbackId: queue.fallbackId || null, fallbackName: name(queue.fallbackTypeEnumId, queue.fallbackId),
    commsScheduleId: queue.commsScheduleId || null,
    scheduleName: queue.commsScheduleId ? one(SCHEDULE, { commsScheduleId: queue.commsScheduleId })?.name || null : null,
    agentList: rows(AGENT, { callQueueId: queue.callQueueId }).map((a) => agentView(a, queue, atMs, offered))
      .sort((a, b) => (a.sequenceNum - b.sequenceNum) || String(a.userName).localeCompare(String(b.userName))),
    numberList: rows(NUMBER, { routeTypeEnumId: 'PnrQueue', routeId: queue.callQueueId }).map((n) => n.phoneNumber),
  }, stats(queue, atMs))
}

function findQueues(p) {
  const denied = notAdmin()
  if (denied) return denied
  const atMs = atOf(p || {})
  return { queueList: Array.from(find(QUEUE).orderBy('name').list()).map((q) => queueView(q, atMs)) }
}

function storeQueue(p) {
  const denied = notAdmin()
  if (denied) return denied
  let queue = blank(p.callQueueId) ? null : one(QUEUE, { callQueueId: p.callQueueId })
  if (!blank(p.callQueueId) && !queue) return refuse('notFound', 'Queue not found')
  if (!queue && blank(p.name)) return refuse('badQueue', 'Name the queue')
  if (!blank(p.strategyEnumId) && !STRATEGIES.includes(p.strategyEnumId)) return refuse('badQueue', 'Not a queue strategy')
  if (!blank(p.fallbackTypeEnumId)) {
    if (p.fallbackTypeEnumId === 'CtgRepeat') return refuse('badTarget', 'A queue has no menu to repeat')
    if (p.fallbackTypeEnumId === 'CtgQueue' && queue && p.fallbackId === queue.callQueueId) return refuse('badTarget', 'A queue cannot fall back to itself')
    const bad = ivr('checkTarget', { targetTypeEnumId: p.fallbackTypeEnumId, targetId: p.fallbackId })
    if (bad.reasonCode) return bad
  }
  if (!blank(p.commsScheduleId) && !one(SCHEDULE, { commsScheduleId: p.commsScheduleId })) return refuse('notFound', 'Schedule not found')
  const fields = {}
  for (const key of Object.keys(LIMITS)) {
    if (blank(p[key])) continue
    const value = intOr(p[key], null)
    const [, low, high] = LIMITS[key]
    if (value == null || value < low || value > high) return refuse('badQueue', `${key} is ${low} to ${high}`)
    fields[key] = value
  }
  for (const key of ['name', 'description', 'strategyEnumId', 'fallbackTypeEnumId', 'fallbackId', 'commsScheduleId']) {
    if (p[key] !== undefined && p[key] !== null) fields[key] = blank(p[key]) ? null : String(p[key]).trim()
  }
  if (fields.fallbackTypeEnumId === 'CtgEnd') fields.fallbackId = null
  if (!queue) {
    queue = ec.entity.makeValue(QUEUE)
    queue.setSequencedIdPrimary()
    queue.setFields(Object.assign({ strategyEnumId: 'CqsLongestIdle' }, fields), true, null, null)
    if (!queue.strategyEnumId) queue.set('strategyEnumId', 'CqsLongestIdle')
    queue.create()
  } else {
    queue.setFields(fields, true, null, null)
    queue.update()
  }
  return { callQueueId: queue.callQueueId }
}

function deleteQueue(p) {
  const denied = notAdmin()
  if (denied) return denied
  const queue = one(QUEUE, { callQueueId: p.callQueueId })
  if (!queue) return refuse('notFound', 'Queue not found')
  const used = rows(NUMBER, { routeTypeEnumId: 'PnrQueue', routeId: queue.callQueueId }).length +
    find('moqui.call.IvrOption').condition('targetTypeEnumId', 'CtgQueue').condition('targetId', queue.callQueueId).count() +
    find(QUEUE).condition('fallbackTypeEnumId', 'CtgQueue').condition('fallbackId', queue.callQueueId).count()
  if (used) return refuse('inUse', 'A number, a menu or another queue sends calls to this queue: change that first')
  if (waiting(queue.callQueueId).length) return refuse('inUse', 'Calls are waiting in this queue')
  find(AGENT).condition('callQueueId', queue.callQueueId).deleteAll()
  ivr('deleteLocation', { location: queue.holdMusicLocation })
  ivr('deleteLocation', { location: queue.positionPromptLocation })
  queue.delete()
  return {}
}

function storeAgent(p) {
  const denied = notAdmin()
  if (denied) return denied
  if (!one(QUEUE, { callQueueId: p.callQueueId })) return refuse('notFound', 'Queue not found')
  const account = p.userId ? one(ACCOUNT, { userId: p.userId }) : null
  if (!account || account.disabled === 'Y') return refuse('notFound', 'User not found')
  const row = one(AGENT, { callQueueId: p.callQueueId, userId: p.userId })
  const sequenceNum = intOr(p.sequenceNum, row ? intOr(row.sequenceNum, 0) : (rows(AGENT, { callQueueId: p.callQueueId }).length + 1) * 10)
  if (row) {
    row.set('sequenceNum', sequenceNum)
    row.update()
  } else {
    const made = ec.entity.makeValue(AGENT)
    made.setFields({ callQueueId: p.callQueueId, userId: p.userId, sequenceNum, loggedIn: 'N', paused: 'N' }, true, null, null)
    made.create()
  }
  return {}
}

function deleteAgent(p) {
  const denied = notAdmin()
  if (denied) return denied
  const row = one(AGENT, { callQueueId: p.callQueueId, userId: p.userId })
  if (!row) return refuse('notFound', 'Agent not found')
  row.delete()
  return {}
}

/** A queue's hold music (kind music) or position prompt (kind position): uploaded, or removed (remove true). */
function storeQueueAudio(p) {
  const denied = notAdmin()
  if (denied) return denied
  const queue = one(QUEUE, { callQueueId: p.callQueueId })
  if (!queue) return refuse('notFound', 'Queue not found')
  const field = p.kind === 'position' ? 'positionPromptLocation' : 'holdMusicLocation'
  if (p.remove === true || p.remove === 'true') {
    ivr('deleteLocation', { location: queue[field] })
    queue.set(field, null)
    queue.update()
    return {}
  }
  const stored = ivr('storePromptFile', { file: p.file, folder: `queue/${queue.callQueueId}` })
  if (stored.reasonCode) return stored
  ivr('deleteLocation', { location: queue[field] })
  queue.set(field, stored.location)
  queue.update()
  return { location: stored.location }
}

/** The queues a signed-in user may transfer a call to: their names, with how many calls wait and agents are free. */
function transferQueues(p) {
  if (!ec.user.userId) return refuse('notSignedIn', 'Sign in first')
  const atMs = nowMs()
  const term = String(p.term || '').trim().toLowerCase()
  return {
    queueList: Array.from(find(QUEUE).orderBy('name').limit(100).list())
      .filter((q) => !term || String(q.name || '').toLowerCase().includes(term))
      .map((q) => Object.assign({ callQueueId: q.callQueueId, name: q.name || q.callQueueId }, stats(q, atMs))),
  }
}

const operations = {
  transferQueues,
  enter, tick, answered, legClosed, sessionEnded, myQueues, setAgentStatus, findQueues, storeQueue, deleteQueue, storeAgent,
  deleteAgent, storeQueueAudio,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown call queue operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
