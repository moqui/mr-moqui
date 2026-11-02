// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Transfer, add, merge (plans/Comms-Framework.md §5.1, §5.3, V5): the services of org.moqui.impl.CallTransferServices.
// Every call is one CallSession and these change its legs, not its media path:
//
// - **Blind transfer** (transfer#Call mode blind): the call's outside party (its answered phone leg) goes to a user, an
//   outside number or a queue, and the transferring leg leaves at once. The far end hears ringing (the engine's
//   ringback) until the target answers; a target that does not answer in call_transfer_ring_seconds, or declines, sends
//   the call to the target user's voicemail (a number's: the call ends).
// - **Attended transfer** (mode attended): the far end is put on hold (hold music, its audio kept from the others) and
//   the target rings in the same call; the user talks to them, then completes (complete#CallTransfer: the far end comes
//   off hold and the user leaves) or cancels (cancel#CallTransfer: the target leaves, the far end comes back).
// - **Add a participant** (add#CallParticipant): a user rings in the call, or a number joins as one more phone leg.
// - **Merge** (merge#Calls): another call the user is in moves into this one, which becomes a meeting; both must be on
//   one media node (cascading SFUs are W7). **To a meeting** (convert#CallToMeeting).
//
// The rules: a call that still rings cannot be transferred; a request names the call's revision and a stale one is
// refused, as is a second transfer while one is open, so two transfers cannot both win. A number (a transfer's or an
// added one) is a call the user places: their outbound permission, the deployment's and their international and premium
// permissions, emergency numbers refused, their limits at once and per day (VoiceServices forwardCheck, dialForward).
// An encrypted call adds no phone legs (§5.1). A desk phone's REFER is a blind transfer by its user (refer, from the SIP
// engine). The engine is told with send#SipControl (route, ringback, connect, hold, resume, transferTimer, move).

const CALLS = 'component://framework/service/org/moqui/impl/Calls.js'
const CALL_HANDLING = 'component://framework/service/org/moqui/impl/CallHandling.js'
const QUEUES = 'component://framework/service/org/moqui/impl/CallQueues.js'
const VOICE = 'component://framework/service/org/moqui/impl/Voice.js'
const SESSION = 'moqui.call.CallSession'
const PARTICIPANT = 'moqui.call.CallParticipant'
const LEG = 'moqui.call.CallLeg'
const TRANSFER = 'moqui.call.CallTransfer'
const NUMBER = 'moqui.call.PhoneNumber'
const ACCOUNT = 'moqui.security.UserAccount'

const OPEN = ['CtsRinging', 'CtsConsulting']
/** The media encryption of every call but an end-to-end encrypted one (W8, e2ee-sframe), which takes no phone legs. */
const PLAIN_MEDIA = 'dtls-srtp'

const ms = (value) => {
  if (value == null || value === '') return null
  const number = Number(value)
  return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}
const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const rows = (entity, fields) => Array.from(find(entity).condition(fields || {}).list())
const blank = (value) => value == null || String(value).trim() === ''
const clamp = (value, low, high) => Math.min(Math.max(value, low), high)
const ringSeconds = () => clamp(Math.round(Number(System.getProperty('call_transfer_ring_seconds') || 30)) || 30, 10, 120)

function calls(op, parameters) {
  return ec.resource.script(CALLS).execute(op, parameters) || {}
}
function voice(op, parameters) {
  return ec.resource.script(VOICE).execute(op, parameters) || {}
}

/** A refusal: an error for the user and a reason code for the caller. */
/**
 * Anyone, or the edge link's delegate user for a call whose media is on this node's edge node (Calls.js edgeAllowed;
 * plans/Edge-Node.md §5.4 "Voice (SIP)", E8): its SIP engine routes only its own calls.
 */
function edgeAllowed(p) {
  return ec.user.username !== '_edge_' || !!(ec.resource.script(CALLS).execute('edgeAllowed', p) || {}).allowed
}

function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

function userName(userId) {
  const account = userId ? one(ACCOUNT, { userId }) : null
  return account ? account.userFullName || account.username || userId : userId
}

/** Cascading media nodes (W7, Calls.js cascadeOn). */
const cascadeOn = () => ['true', 'y', 'yes', '1'].includes(String(System.getProperty('rtc_cascade_enabled') || '').trim().toLowerCase())

/** Tells the SIP engine of the leg's node (its own, W7, else the call's) about one of its legs, after the commit; logged when it cannot. */
function engine(session, message) {
  if (!session?.mediaNodeId) return
  const leg = message.callLegId ? one(LEG, { callLegId: message.callLegId }) : null
  ec.message.pushErrors()
  try {
    ec.service.sync().name('org.moqui.impl.VoiceServices.send#SipControl').parameters({ instanceId: (leg && leg.mediaNodeId) || session.mediaNodeId,
      message: Object.assign({ callSessionId: session.callSessionId }, message) }).disableAuthz().call()
    if (ec.message.hasError()) ec.logger.error(`SIP engine not told to ${message.action}: ${ec.message.errorsString}`)
  } catch (err) {
    ec.logger.error(`SIP engine not told to ${message.action}: ${err && err.message ? err.message : err}`)
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
}

const REFUSALS = {
  badNumber: 'That is not a phone number',
  emergency: 'An emergency number is called directly, not transferred to or added',
  outboundNotAllowed: 'You may not call outside numbers',
  internationalNotEnabled: 'Calls to international numbers are not enabled',
  premiumNotEnabled: 'Calls to premium numbers are not enabled',
}

/**
 * The call and the current user's answered leg in it (a tab, or a desk phone), the session row locked; or the refusal:
 * notFound (not theirs), callEnded, ringing (it still rings: nothing moves until it is answered), notInCall, stale (the
 * revision given is not the call's: it changed since the user saw it).
 */
function myCall(callSessionId, revision) {
  const me = ec.user.userId
  if (!me) return { refusal: refuse('notSignedIn', 'Sign in first') }
  const session = callSessionId ? find(SESSION).condition('callSessionId', callSessionId).forUpdate(true).one() : null
  const participant = session ? one(PARTICIPANT, { callSessionId: session.callSessionId, userId: me }) : null
  if (!session || !participant || participant.admissionStatusId === 'CasRevoked') return { refusal: refuse('notFound', 'Call not found') }
  if (session.statusId === 'CssEnded') return { refusal: refuse('callEnded', 'The call has ended') }
  if (session.statusId === 'CssRinging') return { refusal: refuse('ringing', 'A call that is still ringing cannot be transferred, added to or merged') }
  const myLeg = rows(LEG, { callParticipantId: participant.callParticipantId, statusId: 'ClsAnswered' })
    .find((leg) => leg.legTypeEnumId === 'CltWeb' || leg.legTypeEnumId === 'CltSipDevice')
  if (!myLeg) return { refusal: refuse('notInCall', 'You are not in this call') }
  if (!blank(revision) && Number(revision) !== Number(session.revision || 1)) {
    return { refusal: refuse('stale', 'The call changed since you saw it: look again and try again') }
  }
  return { session, me, myLeg }
}

/**
 * An external user (the framework permission EXTERNAL_USER; a failed check counts as external) chooses nobody to ring:
 * as Calls.js start refuses them a call to people of their choosing, they neither transfer a call nor add to it.
 */
function externalRefusal(userId) {
  const out = ec.service.sync().name('org.moqui.impl.ChatServices.get#ExternalUsers').parameter('userIdList', [userId])
    .disableAuthz().call()
  return out?.anyExternal !== false ? refuse('notAllowed', 'You may not transfer this call or add to it') : null
}

function openTransfer(callSessionId) {
  return Array.from(find(TRANSFER).condition('callSessionId', callSessionId).condition('statusId', 'in', OPEN).forUpdate(true).limit(1).list())[0] || null
}

/** The call's outside party: its first answered phone leg (not a number dialed for someone here). */
function farLeg(session) {
  return rows(LEG, { callSessionId: session.callSessionId, legTypeEnumId: 'CltPstn', statusId: 'ClsAnswered' })
    .filter((leg) => {
      const participant = one(PARTICIPANT, { callParticipantId: leg.callParticipantId })
      return participant && participant.identityKindEnumId === 'CikPhone' && participant.roleEnumId !== 'CprForward'
    }).sort((a, b) => (ms(a.answeredDate) || 0) - (ms(b.answeredDate) || 0))[0] || null
}

/** The number a call to an outside number presents: the one the call came in on, else the user's first caller ID. */
function callerIdFor(session, userId) {
  const route = one('moqui.call.CallRoute', { callSessionId: session.callSessionId })
  if (route?.toNumber) return route.toNumber
  return Array.from(voice('grants', { userId }).numberList || [])[0] || null
}

/**
 * Where a transfer or an add goes: a user (enabled, not the user themselves), a number (the user's checks for a call they
 * place; one of this deployment's numbers goes to what it routes to, a user or a queue) or a queue. {type, id, name} or
 * the refusal.
 */
function resolveTarget(type, id, me) {
  if (type === 'user') {
    const account = blank(id) ? null : one(ACCOUNT, { userId: id })
    if (!account || account.disabled === 'Y') return { refusal: refuse('notFound', 'User not found') }
    if (account.userId === me) return { refusal: refuse('badTarget', 'Choose someone else') }
    // an app's call-target providers may keep transfer and add targets to the user's own (Calls.js refusedTargets): the
    // same answer as for no such user
    if (calls('refusedTargets', { userIdList: [account.userId] }).refusedUserIdList?.length) {
      return { refusal: refuse('notFound', 'User not found') }
    }
    return { type: 'user', id: account.userId, name: userName(account.userId) }
  }
  if (type === 'queue') {
    const queue = blank(id) ? null : one('moqui.call.CallQueue', { callQueueId: id })
    if (!queue) return { refusal: refuse('notFound', 'Queue not found') }
    return { type: 'queue', id: queue.callQueueId, name: queue.name || queue.callQueueId }
  }
  if (type === 'number') {
    const check = voice('forwardCheck', { phoneNumber: id, userId: me, transfer: true })
    if (!check.number) return { refusal: refuse('badNumber', REFUSALS.badNumber) }
    const own = one(NUMBER, { phoneNumber: check.number })
    if (own && own.routeTypeEnumId === 'PnrUser' && own.routeId) return resolveTarget('user', own.routeId, me)
    if (own && own.routeTypeEnumId === 'PnrQueue' && own.routeId) return resolveTarget('queue', own.routeId, me)
    if (own) return { refusal: refuse('badTarget', 'That is one of our numbers: choose the person or queue it rings') }
    if (!check.allowed) return { refusal: refuse(check.reasonCode || 'notAllowed', REFUSALS[check.reasonCode] || 'You may not call that number') }
    return { type: 'number', id: check.number, name: check.number }
  }
  return { refusal: refuse('badTarget', 'Transfer to a person, a number or a queue') }
}

/** A number joins the call as a phone leg, dialed through the trunk of its caller ID; the refusal when it may not. */
function dialNumber(session, number, me) {
  if (session.encryptionMode && session.encryptionMode !== PLAIN_MEDIA) {
    return { refusal: refuse('encrypted', 'An end-to-end encrypted call cannot add a phone number') }
  }
  const node = one('moqui.call.SipNode', { instanceId: session.mediaNodeId })
  if (!node) return { refusal: refuse('voiceUnavailable', 'Phone calls are not available on this call\'s media server') }
  const added = calls('addPhoneLeg', { callSessionId: session.callSessionId, phoneNumber: number, displayName: number, dialedByUserId: me })
  if (!added.callLegId) return { refusal: refuse(added.reasonCode || 'callEnded', 'The call has ended') }
  const dialed = voice('dialForward', { callSessionId: session.callSessionId, callLegId: added.callLegId, phoneNumber: number,
    userId: me, transfer: true, fromNumber: callerIdFor(session, me) })
  if (dialed.reasonCode) {
    return { refusal: refuse(dialed.reasonCode, REFUSALS[dialed.reasonCode] || 'The number was not called (' + dialed.reasonCode + ')') }
  }
  return { callLegId: added.callLegId }
}

function view(callSessionId) {
  return calls('get', { callSessionId }).call || null
}

// ---------- transfer ----------

/**
 * transfer#Call: the call's outside party goes to targetType user, number or queue (targetId), blind or attended (mode),
 * for the call at revision.
 */
function transfer(p) {
  const mine = myCall(p.callSessionId, p.revision)
  if (mine.refusal) return mine.refusal
  const { session, me, myLeg } = mine
  const external = externalRefusal(me)
  if (external) return external
  if (openTransfer(session.callSessionId)) return refuse('transferring', 'This call is being transferred already')
  const far = farLeg(session)
  if (!far) return refuse('noPhone', 'Only a phone call\'s outside party is transferred: add the person to the call instead')
  const attended = p.mode === 'attended'
  const target = resolveTarget(p.targetType, p.targetId, me)
  if (target.refusal) return target.refusal
  if (attended && target.type === 'queue') return refuse('notSupported', 'A call goes to a queue by a blind transfer')
  const now = ec.user.nowTimestamp
  const row = ec.entity.makeValue(TRANSFER)
  row.setSequencedIdPrimary()
  row.setFields({ callSessionId: session.callSessionId, kindEnumId: attended ? 'CtkAttended' : 'CtkBlind', fromUserId: me,
    fromLegId: myLeg.callLegId, heldLegId: far.callLegId, targetTypeEnumId: target.type === 'user' ? 'CtgUser' : target.type === 'queue' ? 'CtgQueue' : 'CtgNumber',
    targetId: target.id, reasonText: target.name, statusId: attended ? 'CtsConsulting' : 'CtsRinging', createdDate: now }, true, null, null)
  row.create()
  // the revision moves on: a second request made from the same view is stale
  session.set('revision', Number(session.revision || 1) + 1)
  session.update()
  if (target.type === 'queue') {
    row.setFields({ statusId: 'CtsCompleted', endedDate: now }, true, null, null)
    row.update()
    const answer = ec.resource.script(QUEUES).execute('enter', { callSessionId: session.callSessionId, callLegId: far.callLegId,
      callQueueId: target.id, fromNumber: far.phoneNumber || null, depth: 1 }) || {}
    engine(session, { action: 'route', callLegId: far.callLegId, answer })
    calls('transferLeave', { callLegId: myLeg.callLegId })
    return { callTransferId: row.callTransferId, call: view(session.callSessionId) }
  }
  if (target.type === 'user') {
    calls('offerUsers', { callSessionId: session.callSessionId, userIdList: [target.id], role: 'callee', ringSeconds: ringSeconds() + 10,
      displayName: one(PARTICIPANT, { callParticipantId: far.callParticipantId })?.displayName || far.phoneNumber || null })
  } else {
    const dialed = dialNumber(session, target.id, me)
    if (dialed.refusal) {
      row.setFields({ statusId: 'CtsFailed', endedDate: now }, true, null, null)
      row.update()
      return dialed.refusal
    }
    row.set('targetLegId', dialed.callLegId)
    row.update()
  }
  if (attended) {
    calls('setHeld', { callLegId: far.callLegId, held: true })
    engine(session, { action: 'hold', callLegId: far.callLegId })
  } else {
    engine(session, { action: 'ringback', callLegId: far.callLegId })
    engine(session, { action: 'transferTimer', callLegId: far.callLegId, seconds: ringSeconds(), callTransferId: row.callTransferId })
    calls('transferLeave', { callLegId: myLeg.callLegId })
  }
  return { callTransferId: row.callTransferId, call: view(session.callSessionId) }
}

/** The transfer's target is in the call: the user's answered leg, or the number's. */
function targetAnswered(row) {
  if (row.targetTypeEnumId === 'CtgNumber') return one(LEG, { callLegId: row.targetLegId })?.statusId === 'ClsAnswered'
  const participant = one(PARTICIPANT, { callSessionId: row.callSessionId, userId: row.targetId })
  return !!participant && rows(LEG, { callParticipantId: participant.callParticipantId, statusId: 'ClsAnswered' }).length > 0
}

/** The target's legs end: a user's offers and leg, or the number's. */
function dropTarget(row) {
  if (row.targetTypeEnumId === 'CtgNumber') calls('closePhoneLeg', { callLegId: row.targetLegId })
  else if (row.targetTypeEnumId === 'CtgUser') calls('closeUserLegs', { callSessionId: row.callSessionId, userId: row.targetId })
}

function resumeHeld(row, session) {
  const held = one(LEG, { callLegId: row.heldLegId })
  if (!held || held.statusId !== 'ClsAnswered' || !held.heldDate) return
  calls('setHeld', { callLegId: held.callLegId, held: false })
  engine(session, { action: 'resume', callLegId: held.callLegId })
}

/**
 * A transfer and its call, locked as every other path locks them, the call first (Calls.js answer, phoneEvent and
 * declineRing lock the call, then its open transfer), so two of them never wait on each other.
 */
function lockTransfer(callTransferId) {
  const seen = callTransferId ? one(TRANSFER, { callTransferId }) : null
  if (!seen) return { row: null, session: null }
  const session = find(SESSION).condition('callSessionId', seen.callSessionId).forUpdate(true).one()
  const row = find(TRANSFER).condition('callTransferId', callTransferId).forUpdate(true).one()
  return { row, session }
}

function myTransfer(callTransferId) {
  const me = ec.user.userId
  const { row, session } = me ? lockTransfer(callTransferId) : { row: null, session: null }
  if (!me || !row || row.fromUserId !== me) return { refusal: refuse('notFound', 'Transfer not found') }
  if (!session || session.statusId === 'CssEnded') return { refusal: refuse('callEnded', 'The call has ended') }
  return { row, session }
}

/** complete#CallTransfer: the consulted target answered; the far end comes off hold with them and the user leaves. */
function complete(p) {
  const mine = myTransfer(p.callTransferId)
  if (mine.refusal) return mine.refusal
  const { row, session } = mine
  if (row.statusId !== 'CtsConsulting') return refuse('notConsulting', 'This transfer is not waiting for you')
  if (!targetAnswered(row)) return refuse('notAnswered', 'The person you are transferring to has not answered yet')
  row.setFields({ statusId: 'CtsCompleted', endedDate: ec.user.nowTimestamp }, true, null, null)
  row.update()
  resumeHeld(row, session)
  calls('transferLeave', { callLegId: row.fromLegId })
  return { call: view(session.callSessionId) }
}

/** cancel#CallTransfer: the consulted target leaves (or stops ringing) and the far end comes back to the user. */
function cancel(p) {
  const mine = myTransfer(p.callTransferId)
  if (mine.refusal) return mine.refusal
  const { row, session } = mine
  if (row.statusId !== 'CtsConsulting') return refuse('notConsulting', 'This transfer is not waiting for you')
  row.setFields({ statusId: 'CtsCanceled', endedDate: ec.user.nowTimestamp }, true, null, null)
  row.update()
  dropTarget(row)
  resumeHeld(row, session)
  return { call: view(session.callSessionId) }
}

/** Internal (Calls.js afterAnswer): someone answered; a blind transfer's target that answers completes it. */
function answered(p) {
  const row = openTransfer(p.callSessionId)
  if (!row) return {}
  const isTarget = (row.targetTypeEnumId === 'CtgUser' && p.userId === row.targetId) ||
    (row.targetTypeEnumId === 'CtgNumber' && p.callLegId === row.targetLegId)
  if (!isTarget || row.statusId !== 'CtsRinging') return {}
  row.setFields({ statusId: 'CtsCompleted', endedDate: ec.user.nowTimestamp }, true, null, null)
  row.update()
  // the far end stops hearing ringing and hears the target
  engine(one(SESSION, { callSessionId: row.callSessionId }), { action: 'connect', callLegId: row.heldLegId })
  return {}
}

/** The far end goes where a blind transfer nobody took sends it: the target user's voicemail; for a number, the end. */
function blindFallback(row) {
  if (row.targetTypeEnumId !== 'CtgUser') return { mode: 'end' }
  const out = ec.resource.script(CALL_HANDLING).execute('routeTo', { callSessionId: row.callSessionId, voicemailUserId: row.targetId }) || {}
  return out.mode === 'voicemail' ? { mode: 'voicemail', stepToken: out.stepToken } : { mode: 'end' }
}

/**
 * Internal (the SIP engine, call_transfer_ring_seconds after a blind transfer): a target that did not answer missed it;
 * answers where the far end goes (the target user's voicemail, or the end), or mode done when the transfer is over.
 */
function expire(p) {
  const { row, session } = lockTransfer(p.callTransferId)
  if (!row || row.statusId !== 'CtsRinging') return { mode: 'done' }
  if (!session || session.statusId === 'CssEnded') return { mode: 'done' }
  if (!edgeAllowed({ callSessionId: session.callSessionId })) return { mode: 'done', reasonCode: 'notAllowed' }
  if (row.targetTypeEnumId === 'CtgUser') calls('closeOffers', { callSessionId: row.callSessionId })
  else dropTarget(row)
  row.setFields({ statusId: 'CtsFailed', endedDate: ec.user.nowTimestamp }, true, null, null)
  row.update()
  return blindFallback(row)
}

/** Internal (Calls.js declineRing): a transfer's target declined. handled true when it was the target. */
function declined(p) {
  const row = openTransfer(p.callSessionId)
  if (!row || row.targetTypeEnumId !== 'CtgUser' || row.targetId !== p.userId) return {}
  row.setFields({ statusId: 'CtsFailed', endedDate: ec.user.nowTimestamp }, true, null, null)
  row.update()
  const session = one(SESSION, { callSessionId: row.callSessionId })
  if (row.kindEnumId === 'CtkAttended') resumeHeld(row, session)
  else engine(session, { action: 'route', callLegId: row.heldLegId, answer: blindFallback(row) })
  return { handled: true }
}

/**
 * Internal (Calls.js phoneLegGone): a phone leg of a call with an open transfer closed. The party being transferred
 * hung up: the transfer is over and its target stops ringing (the call then ends). The number transferred to failed or
 * hung up: an attended transfer goes back to the user with the far end off hold; a blind one ends the call.
 */
function legGone(p) {
  const row = openTransfer(p.callSessionId)
  if (!row) return {}
  const session = one(SESSION, { callSessionId: row.callSessionId })
  if (p.callLegId === row.heldLegId) {
    row.setFields({ statusId: 'CtsCanceled', endedDate: ec.user.nowTimestamp }, true, null, null)
    row.update()
    if (!targetAnswered(row) || row.targetTypeEnumId === 'CtgNumber') dropTarget(row)
    return {}
  }
  if (p.callLegId === row.targetLegId) {
    row.setFields({ statusId: 'CtsFailed', endedDate: ec.user.nowTimestamp }, true, null, null)
    row.update()
    if (row.kindEnumId === 'CtkAttended') resumeHeld(row, session)
    else engine(session, { action: 'hangup', callLegId: row.heldLegId })
  }
  return {}
}

/** Internal (Calls.js endSession): the call ended; an open transfer is over. */
function sessionEnded(p) {
  for (const row of rows(TRANSFER, { callSessionId: p.callSessionId }).filter((r) => OPEN.includes(r.statusId))) {
    row.setFields({ statusId: 'CtsCanceled', endedDate: ec.user.nowTimestamp }, true, null, null)
    row.update()
  }
  return {}
}

/**
 * Internal (the SIP engine): a desk phone in a call sent REFER (blind transfer) to target: a username or a number. Its
 * user transfers the call as they would in the browser.
 */
function refer(p) {
  const leg = p.callLegId ? one(LEG, { callLegId: p.callLegId }) : null
  if (!leg || leg.legTypeEnumId !== 'CltSipDevice' || leg.statusId !== 'ClsAnswered') return refuse('notFound', 'Call not found')
  if (!edgeAllowed({ callLegId: leg.callLegId })) return refuse('notFound', 'Call not found')
  const participant = one(PARTICIPANT, { callParticipantId: leg.callParticipantId })
  const account = participant?.userId ? one(ACCOUNT, { userId: participant.userId }) : null
  if (!account || account.disabled === 'Y') return refuse('notFound', 'Call not found')
  try {
    ec.user.internalLoginUser(account.username)
  } catch (err) {
    return refuse('notAllowed', 'This phone cannot transfer now')
  }
  const to = String(p.target || '').trim()
  const user = /^[+0-9*#]+$/.test(to) ? null : one(ACCOUNT, { username: to })
  return transfer({ callSessionId: leg.callSessionId, targetType: user ? 'user' : 'number', targetId: user ? user.userId : to, mode: 'blind' })
}

// ---------- add, merge, meeting ----------

/** add#CallParticipant: a user rings in the call (userId), or a number joins it (phoneNumber), under the fraud policy. */
function addParticipant(p) {
  const mine = myCall(p.callSessionId, p.revision)
  if (mine.refusal) return mine.refusal
  const { session, me } = mine
  const external = externalRefusal(me)
  if (external) return external
  if (!blank(p.userId)) {
    const target = resolveTarget('user', p.userId, me)
    if (target.refusal) return target.refusal
    const participant = one(PARTICIPANT, { callSessionId: session.callSessionId, userId: target.id })
    if (participant && rows(LEG, { callParticipantId: participant.callParticipantId }).some((l) => ['ClsOffered', 'ClsAnswered'].includes(l.statusId))) {
      return refuse('alreadyIn', 'They are in the call or it rings for them already')
    }
    calls('offerUsers', { callSessionId: session.callSessionId, userIdList: [target.id], role: 'callee', ringSeconds: ringSeconds() + 10,
      displayName: userName(me) })
    return { call: view(session.callSessionId) }
  }
  if (blank(p.phoneNumber)) return refuse('badTarget', 'Choose a person or a number')
  // an end-to-end encrypted call (W8) takes no phone leg, whatever the number: said before the number is looked at
  if (session.encryptionMode && session.encryptionMode !== PLAIN_MEDIA) {
    return refuse('encrypted', 'An end-to-end encrypted call cannot add a phone number')
  }
  const target = resolveTarget('number', p.phoneNumber, me)
  if (target.refusal) return target.refusal
  if (target.type === 'user') return addParticipant(Object.assign({}, p, { phoneNumber: null, userId: target.id }))
  if (target.type !== 'number') return refuse('badTarget', 'A queue is not added to a call: transfer the caller to it')
  const dialed = dialNumber(session, target.id, me)
  if (dialed.refusal) return dialed.refusal
  return { callLegId: dialed.callLegId, call: view(session.callSessionId) }
}

/**
 * merge#Calls: the other call (otherCallSessionId), which the user is in too, moves into this one, which becomes a
 * meeting; the legs on hold in it come back. Calls on two media nodes merge with cascading media nodes on
 * (rtc_cascade_enabled, W7): the other call's phones stay on their node, which relays to this call's node, and its
 * browsers bind again (on their own node). Without it, both must be on one media node.
 */
function merge(p) {
  if (blank(p.otherCallSessionId) || p.otherCallSessionId === p.callSessionId) return refuse('badTarget', 'Choose another call to merge')
  const mine = myCall(p.callSessionId, p.revision)
  if (mine.refusal) return mine.refusal
  const other = myCall(p.otherCallSessionId, null)
  if (other.refusal) return other.refusal
  const { session } = mine
  for (const s of [session, other.session]) {
    if (s.encryptionMode && s.encryptionMode !== PLAIN_MEDIA) return refuse('encrypted', 'An end-to-end encrypted call is not merged')
    if (openTransfer(s.callSessionId)) return refuse('transferring', 'A call being transferred is not merged')
  }
  if (session.mediaNodeId !== other.session.mediaNodeId && !cascadeOn()) {
    return refuse('differentNodes', 'These calls are on different media servers, so they cannot be merged (turn on rtc_cascade_enabled)')
  }
  const out = calls('mergeInto', { callSessionId: session.callSessionId, fromCallSessionId: other.session.callSessionId })
  if (out.reasonCode) return refuse(out.reasonCode, 'The call has ended')
  for (const leg of rows(LEG, { callSessionId: session.callSessionId, statusId: 'ClsAnswered' }).filter((l) => l.heldDate)) {
    calls('setHeld', { callLegId: leg.callLegId, held: false })
    if (leg.legTypeEnumId === 'CltPstn') engine(session, { action: 'resume', callLegId: leg.callLegId })
  }
  return { call: view(session.callSessionId), movedCount: out.movedCount || 0 }
}

/** convert#CallToMeeting: the call goes on as a meeting, as long as anyone is in it. */
function toMeeting(p) {
  const mine = myCall(p.callSessionId, p.revision)
  if (mine.refusal) return mine.refusal
  calls('setKind', { callSessionId: mine.session.callSessionId, kindEnumId: 'CskMeeting' })
  return { call: view(mine.session.callSessionId) }
}

const operations = {
  transfer, complete, cancel, addParticipant, merge, toMeeting, refer,
  // internal: Calls.js and the SIP engine
  answered, declined, legGone, sessionEnded, expire,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown call transfer operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
