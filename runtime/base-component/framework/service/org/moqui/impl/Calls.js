// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Calls (plans/Comms-Framework.md §5.4, §6.3): the state machine of org.moqui.impl.CallServices. A call is a
// CallSession; its people are CallParticipants; each way one is in it is a CallLeg; a leg's signaling socket is its
// CallConnection, whose generation fences an older socket of the same leg. Every write checks the current user's part
// in the call (or their room, for a group call) and the call's state, with the session row locked, so two answers,
// a repeated request or a stale tab settle on one outcome. Nothing here touches media: the host's rtc module does,
// told by send#MediaControl after the commit. History is never deleted.

const SESSION = 'moqui.call.CallSession'
const PARTICIPANT = 'moqui.call.CallParticipant'
const LEG = 'moqui.call.CallLeg'
const CONNECTION = 'moqui.call.CallConnection'
const USER_LOG = 'moqui.call.CallUserLog'
const MEDIA_NODE = 'moqui.call.CallMediaNode'

const KIND_CODES = { CskDirect: 'direct', CskGroup: 'group', CskMeeting: 'meeting', CskPhone: 'phone' }
const STATUS_CODES = { CssRinging: 'ringing', CssActive: 'active', CssEnded: 'ended' }
const LEG_CODES = {
  ClsOffered: 'offered', ClsAnswered: 'answered', ClsDeclined: 'declined', ClsCanceled: 'canceled',
  ClsTimedOut: 'timedOut', ClsFailed: 'failed', ClsLeft: 'left',
}
const ADMISSION_CODES = {
  CasInvited: 'invited', CasWaiting: 'waiting', CasAdmitted: 'admitted', CasDenied: 'denied', CasRevoked: 'revoked',
}
const END_CODES = {
  CerHangup: 'hangup', CerDeclined: 'declined', CerNoAnswer: 'noAnswer', CerCanceled: 'canceled',
  CerConnectionLost: 'connectionLost', CerMediaNodeLost: 'mediaNodeLost', CerEmpty: 'empty', CerFailed: 'failed',
  CerRemoved: 'removed',
}
const OPEN_LEG = ['ClsOffered', 'ClsAnswered']

/** Seconds a call rings before it is missed. */
const ringSeconds = () => Math.max(10, Number(System.getProperty('call_ring_seconds') || 45))
/** A leg whose socket closed and did not come back in this long has left. */
const RECONNECT_GRACE_MS = 30000
/** A connection with no heartbeat in this long is gone (the host refreshes every 20 seconds). */
const HEARTBEAT_STALE_MS = 75000
/** An answered leg that never connected its media in this long has left. */
const NEVER_CONNECTED_MS = 120000
/** A media node with no heartbeat in this long is lost. */
const MEDIA_NODE_STALE_MS = 60000
/** The most people a call rings. */
const MAX_INVITEES = 50

const ms = (value) => {
  if (value == null || value === '') return null
  const number = Number(value)
  return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}
const nowMs = () => ms(ec.user.nowTimestamp)
const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const rows = (entity, fields) => Array.from(find(entity).condition(fields).list())

/** A refusal: an error for the user and a reason code for the caller. */
function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

function currentUser() {
  return ec.user.userId || null
}

function userNames(userIds) {
  const names = {}
  const ids = Array.from(new Set(userIds.filter((id) => id)))
  if (!ids.length) return names
  for (const account of find('moqui.security.UserAccount').condition('userId', 'in', ids)
    .selectField('userId').selectField('userFullName').selectField('username').list()) {
    names[account.userId] = account.userFullName || account.username || account.userId
  }
  return names
}

function isExternal(userId) {
  const out = ec.service.sync().name('org.moqui.impl.ChatServices.get#ExternalUsers')
    .parameter('userIdList', [userId]).disableAuthz().call()
  return out?.anyExternal !== false
}

function appRoot() {
  return (System.getProperty('comms_app_root') || '/qapps/BasicComms').replace(/\/+$/, '')
}

function callLink(callSessionId) {
  return appRoot() + '/Calls/Call?callSessionId=' + encodeURIComponent(callSessionId)
}

/** The live media nodes, newest heartbeat first; one that is draining takes no new call. */
function liveMediaNodes() {
  const since = new Date(nowMs() - MEDIA_NODE_STALE_MS)
  return Array.from(find(MEDIA_NODE).condition('heartbeatDate', '>=', since).orderBy('-heartbeatDate').list())
}

function lockSession(callSessionId) {
  return find(SESSION).condition('callSessionId', callSessionId).forUpdate(true).one()
}

function participantsOf(callSessionId) {
  return rows(PARTICIPANT, { callSessionId })
}

function legsOf(callSessionId) {
  return Array.from(find(LEG).condition('callSessionId', callSessionId).orderBy('offeredDate').list())
}

function participantOf(callSessionId, userId) {
  return userId ? one(PARTICIPANT, { callSessionId, userId }) : null
}

/** Whether the user is a current member of the room (a group call's room members may join it). */
function roomMember(chatRoomId, userId) {
  if (!chatRoomId) return false
  const access = ec.service.sync().name('org.moqui.impl.ChatServices.get#RoomAccess')
    .parameters({ chatRoomId, userId }).disableAuthz().call()
  return !!access?.isMember && !access?.isArchived
}

/** Whether the user may see the call: a participant, or a member of the room of a group call. */
function canSee(session, userId) {
  if (!session || !userId) return false
  const participant = participantOf(session.callSessionId, userId)
  if (participant && participant.admissionStatusId !== 'CasRevoked') return true
  return session.kindEnumId !== 'CskDirect' && roomMember(session.chatRoomId, userId)
}

/** The call as a client sees it. */
function view(session, forUserId) {
  const participants = participantsOf(session.callSessionId)
  const legs = legsOf(session.callSessionId)
  const names = userNames(participants.map((p) => p.userId).concat([session.startedByUserId]))
  let myLeg = null
  const list = participants.map((p) => {
    const own = legs.filter((leg) => leg.callParticipantId === p.callParticipantId)
    if (forUserId && p.userId === forUserId) {
      myLeg = own.find((leg) => leg.statusId === 'ClsAnswered') || own.find((leg) => leg.statusId === 'ClsOffered') ||
        own[own.length - 1] || null
    }
    return {
      callParticipantId: p.callParticipantId,
      userId: p.userId || null,
      displayName: p.displayName || names[p.userId] || p.userId,
      role: p.roleEnumId === 'CprHost' ? 'host' : p.roleEnumId === 'CprGuest' ? 'guest' : 'member',
      admission: ADMISSION_CODES[p.admissionStatusId] || null,
      inCall: own.some((leg) => leg.statusId === 'ClsAnswered'),
      ringing: own.some((leg) => leg.statusId === 'ClsOffered'),
      legs: own.map((leg) => ({ callLegId: leg.callLegId, status: LEG_CODES[leg.statusId] || null })),
    }
  })
  return {
    callSessionId: session.callSessionId,
    kind: KIND_CODES[session.kindEnumId] || null,
    status: STATUS_CODES[session.statusId] || null,
    revision: Number(session.revision || 1),
    chatRoomId: session.chatRoomId || null,
    video: session.video === 'Y',
    startedByUserId: session.startedByUserId || null,
    startedByName: names[session.startedByUserId] || null,
    startedDate: ms(session.startedDate),
    ringExpireDate: ms(session.ringExpireDate),
    answeredDate: ms(session.answeredDate),
    endedDate: ms(session.endedDate),
    endReason: END_CODES[session.endReasonEnumId] || null,
    encryptionMode: session.encryptionMode || null,
    participants: list,
    myLegId: myLeg ? myLeg.callLegId : null,
    myLegStatus: myLeg ? LEG_CODES[myLeg.statusId] || null : null,
    url: callLink(session.callSessionId),
  }
}

/** Pushes a call message to each user's tabs on chatUser:<userId>, after the commit; its kind is always 'call'. */
function push(userIds, message) {
  for (const userId of new Set(userIds.filter((id) => id))) {
    ec.service.sync().name('org.moqui.impl.ChatServices.push#Signal').parameters({
      topic: 'chatUser:' + userId, userIdList: [userId], message: Object.assign({}, message, { kind: 'call' }),
    }).disableAuthz().call()
  }
}

function participantUserIds(callSessionId) {
  return participantsOf(callSessionId).map((p) => p.userId).filter((id) => id)
}

/** Pushes the call's new state to everyone in it, each with their own view. */
function pushState(session, action, extra) {
  for (const userId of new Set(participantUserIds(session.callSessionId))) {
    push([userId], Object.assign({ action, callSessionId: session.callSessionId, revision: Number(session.revision || 1),
      call: view(session, userId) }, extra || {}))
  }
}

function bump(session) {
  session.set('revision', Number(session.revision || 1) + 1)
}

/** A CetCall event in the call's room: its start and its end, so the room's timeline shows the call. */
function roomEvent(session, action, extra) {
  if (!session.chatRoomId) return
  ec.service.sync().name('org.moqui.impl.ChatServices.append#Event').parameters({
    chatRoomId: session.chatRoomId, eventTypeEnumId: 'CetCall', senderUserId: session.startedByUserId,
    content: Object.assign({ callSessionId: session.callSessionId, action, kind: KIND_CODES[session.kindEnumId] || null,
      video: session.video === 'Y' }, extra || {}),
  }).disableAuthz().call()
}

/** Tells the call's media node to close a leg or the whole room, after the commit. */
function mediaControl(session, message) {
  if (!session.mediaNodeId) return
  ec.service.sync().name('org.moqui.impl.CallServices.send#MediaControl').parameters({
    mediaNodeId: session.mediaNodeId, message: Object.assign({ callSessionId: session.callSessionId }, message),
  }).disableAuthz().call()
}

/** Sets UserStatus.inCall from the user's answered legs of calls that have not ended, publishing a change. */
function updatePresence(userIds) {
  for (const userId of new Set(userIds.filter((id) => id))) {
    const participantIds = Array.from(find(PARTICIPANT).condition('userId', userId).selectField('callParticipantId')
      .orderBy('-callParticipantId').limit(100).list()).map((p) => p.callParticipantId)
    let inCall = false
    if (participantIds.length) {
      const sessionIds = Array.from(find(LEG).condition('callParticipantId', 'in', participantIds)
        .condition('statusId', 'ClsAnswered').selectField('callSessionId').list()).map((leg) => leg.callSessionId)
      if (sessionIds.length) {
        inCall = find(SESSION).condition('callSessionId', 'in', sessionIds).condition('statusId', '!=', 'CssEnded')
          .count() > 0
      }
    }
    const row = find('moqui.security.UserStatus').condition('userId', userId).forUpdate(true).one()
    const flag = inCall ? 'Y' : 'N'
    if (row && (row.inCall || 'N') === flag) continue
    if (row) {
      row.set('inCall', flag)
      row.update()
    } else {
      const created = ec.entity.makeValue('moqui.security.UserStatus')
      created.setFields({ userId, inCall: flag }, true, null, null)
      created.create()
    }
    ec.service.sync().name('org.moqui.impl.ChatServices.publish#UserStatus').parameters({ userId, force: true })
      .disableAuthz().call()
  }
}

function setLog(callSessionId, userId, fields) {
  const row = one(USER_LOG, { callSessionId, userId })
  if (row) {
    row.setFields(fields, true, null, null)
    row.update()
  } else {
    const created = ec.entity.makeValue(USER_LOG)
    created.setFields(Object.assign({ callSessionId, userId }, fields), true, null, null)
    created.create()
  }
}

function makeParticipant(session, userId, role, admission) {
  const p = ec.entity.makeValue(PARTICIPANT)
  p.setSequencedIdPrimary()
  p.setFields({
    callSessionId: session.callSessionId, identityKindEnumId: 'CikLocal', userId, roleEnumId: role,
    admissionStatusId: admission, admittedDate: admission === 'CasAdmitted' ? ec.user.nowTimestamp : null,
  }, true, null, null)
  p.create()
  return p
}

function makeLeg(session, participant, status) {
  const now = ec.user.nowTimestamp
  const leg = ec.entity.makeValue(LEG)
  leg.setSequencedIdPrimary()
  leg.setFields({
    callSessionId: session.callSessionId, callParticipantId: participant.callParticipantId, legTypeEnumId: 'CltWeb',
    statusId: status, offeredDate: now, answeredDate: status === 'ClsAnswered' ? now : null,
  }, true, null, null)
  leg.create()
  return leg
}

function seconds(from, to) {
  const start = ms(from)
  const end = ms(to)
  return start && end && end > start ? Math.round((end - start) / 1000) : 0
}

/** Ends a leg; an answered leg's time goes in its user's log. */
function closeLeg(leg, status, reason) {
  const now = ec.user.nowTimestamp
  const wasAnswered = leg.statusId === 'ClsAnswered'
  leg.setFields({ statusId: status, endReasonEnumId: reason || leg.endReasonEnumId || null, leftDate: now },
    true, null, null)
  leg.update()
  for (const conn of rows(CONNECTION, { callLegId: leg.callLegId, statusId: 'CcsActive' })) {
    conn.setFields({ statusId: 'CcsClosed', closedDate: now }, true, null, null)
    conn.update()
  }
  if (wasAnswered) {
    const participant = one(PARTICIPANT, { callParticipantId: leg.callParticipantId })
    if (participant?.userId) {
      const log = one(USER_LOG, { callSessionId: leg.callSessionId, userId: participant.userId })
      const total = Number(log?.durationSeconds || 0) + seconds(leg.answeredDate, now)
      setLog(leg.callSessionId, participant.userId, { durationSeconds: total })
    }
  }
}

/** Ends the call: its open legs close, the callees it rang and nobody answered have missed it. */
function endSession(session, reason) {
  if (session.statusId === 'CssEnded') return
  const now = ec.user.nowTimestamp
  for (const leg of legsOf(session.callSessionId)) {
    if (leg.statusId === 'ClsAnswered') {
      closeLeg(leg, 'ClsLeft', reason)
    } else if (leg.statusId === 'ClsOffered') {
      closeLeg(leg, reason === 'CerNoAnswer' ? 'ClsTimedOut' : 'ClsCanceled', reason)
    }
  }
  for (const log of rows(USER_LOG, { callSessionId: session.callSessionId })) {
    if (!log.outcomeEnumId) {
      log.set('outcomeEnumId', 'CuoMissed')
      log.update()
    }
  }
  session.setFields({ statusId: 'CssEnded', endedDate: now, endReasonEnumId: reason }, true, null, null)
  bump(session)
  session.update()
  const duration = seconds(session.answeredDate, now)
  roomEvent(session, 'ended', { endReason: END_CODES[reason] || null, durationSeconds: duration })
  mediaControl(session, { action: 'closeRoom' })
  pushState(session, 'ended')
  // everyone who was in it, the leg that just left too (updatePresence publishes only a change)
  updatePresence(participantsOf(session.callSessionId).filter((p) => p.admissionStatusId === 'CasAdmitted')
    .map((p) => p.userId))
  const missed = rows(USER_LOG, { callSessionId: session.callSessionId, outcomeEnumId: 'CuoMissed' }).map((l) => l.userId)
  if (missed.length) {
    ec.service.sync().name('org.moqui.impl.CallServices.publish#MissedCounts').parameter('userIdList', missed)
      .disableAuthz().call()
  }
}

function legOfMine(session, userId, statuses) {
  const participant = participantOf(session.callSessionId, userId)
  if (!participant) return { participant: null, leg: null }
  const legs = rows(LEG, { callParticipantId: participant.callParticipantId }).filter((leg) => statuses.includes(leg.statusId))
  legs.sort((a, b) => (ms(b.offeredDate) || 0) - (ms(a.offeredDate) || 0))
  return { participant, leg: legs[0] || null }
}

function answeredLegCount(callSessionId) {
  return find(LEG).condition('callSessionId', callSessionId).condition('statusId', 'ClsAnswered').count()
}

// ---------- operations ----------

function start(p) {
  const me = currentUser()
  if (!me) return refuse('notSignedIn', 'Sign in to make a call')
  const key = p.idempotencyKey ? String(p.idempotencyKey).slice(0, 200) : null
  if (key) {
    const prior = Array.from(find(SESSION).condition('startedByUserId', me).condition('idempotencyKey', key)
      .orderBy('-startedDate').limit(1).list())[0]
    if (prior) {
      const legs = legOfMine(prior, me, ['ClsAnswered', 'ClsLeft'])
      return { callSessionId: prior.callSessionId, callLegId: legs.leg?.callLegId || null, call: view(prior, me), repeated: true }
    }
  }

  let chatRoomId = p.chatRoomId || null
  let invitees = []
  let kind
  if (chatRoomId) {
    const access = ec.service.sync().name('org.moqui.impl.ChatServices.get#RoomAccess').parameter('chatRoomId', chatRoomId).call()
    if (!access?.canPost) return refuse('notFound', 'Chat room not found')
    const current = Array.from(find(SESSION).condition('chatRoomId', chatRoomId).condition('statusId', '!=', 'CssEnded')
      .orderBy('-startedDate').limit(1).list())[0]
    if (current) return { callSessionId: current.callSessionId, call: view(current, me), reasonCode: 'callActive' }
    const room = one('moqui.chat.ChatRoom', { chatRoomId })
    const isDirect = room.roomTypeEnumId === 'CrtDirect'
    invitees = Array.from(find('moqui.chat.ChatRoomMember').condition('chatRoomId', chatRoomId).selectField('userId')
      .selectField('leftDate').selectField('roleEnumId').list())
      .filter((m) => (isDirect || !m.leftDate) && m.userId !== me && m.roleEnumId !== 'CmrAgent').map((m) => m.userId)
    kind = isDirect && invitees.length === 1 ? 'CskDirect' : 'CskGroup'
  } else {
    if (isExternal(me)) return refuse('notAllowed', 'Start a call from a chat room you are in')
    const asked = Array.isArray(p.calleeUserIdList) ? p.calleeUserIdList : String(p.calleeUserIdList || '').split(',')
    invitees = Array.from(new Set(asked.map((id) => String(id || '').trim()).filter((id) => id && id !== me)))
    if (!invitees.length) return refuse('noCallee', 'Choose someone to call')
    const known = Array.from(find('moqui.security.UserAccount').condition('userId', 'in', invitees)
      .selectField('userId').selectField('disabled').list()).filter((a) => a.disabled !== 'Y').map((a) => a.userId)
    if (known.length !== invitees.length) return refuse('noCallee', 'Choose someone to call')
    const room = ec.service.sync().name('org.moqui.impl.ChatServices.create#Room')
      .parameters({ roomTypeEnumId: 'CrtDirect', memberUserIdList: invitees }).call()
    if (ec.message.hasError() || !room?.chatRoomId) return { reasonCode: 'failed' }
    chatRoomId = room.chatRoomId
    kind = invitees.length === 1 ? 'CskDirect' : 'CskGroup'
  }
  if (invitees.length > MAX_INVITEES) invitees = invitees.slice(0, MAX_INVITEES)

  const nodes = liveMediaNodes().filter((n) => n.draining !== 'Y')
  if (!nodes.length) return refuse('mediaUnavailable', 'Calls are not available: no media server is running')

  const now = ec.user.nowTimestamp
  const session = ec.entity.makeValue(SESSION)
  session.setSequencedIdPrimary()
  session.setFields({
    chatRoomId, kindEnumId: kind, statusId: kind === 'CskDirect' ? 'CssRinging' : 'CssActive', revision: 1,
    directionEnumId: 'CdrInternal', video: p.video === false || p.video === 'false' || p.video === 'N' ? 'N' : 'Y',
    startedByUserId: me, idempotencyKey: key, startedDate: now,
    ringExpireDate: new Date(nowMs() + ringSeconds() * 1000), mediaNodeId: nodes[0].instanceId, mediaGeneration: 1,
    encryptionMode: 'dtls-srtp',
  }, true, null, null)
  session.create()
  const host = makeParticipant(session, me, 'CprHost', 'CasAdmitted')
  const myLeg = makeLeg(session, host, 'ClsAnswered')
  setLog(session.callSessionId, me, { roleEnumId: 'CurCaller', outcomeEnumId: 'CuoPlaced', logDate: now })
  for (const userId of invitees) {
    const participant = makeParticipant(session, userId, 'CprMember', 'CasInvited')
    makeLeg(session, participant, 'ClsOffered')
    setLog(session.callSessionId, userId, { roleEnumId: 'CurCallee', logDate: now })
  }
  roomEvent(session, 'started')
  ring(session, invitees, me)
  updatePresence([me])
  return { callSessionId: session.callSessionId, callLegId: myLeg.callLegId, call: view(session, me) }
}

/** Rings the invitees' tabs, and their devices by Web Push (expiring with the ring). */
function ring(session, userIds, fromUserId) {
  if (!userIds.length) return
  const from = { userId: fromUserId, name: userNames([fromUserId])[fromUserId] || fromUserId }
  for (const userId of userIds) {
    push([userId], { action: 'ring', callSessionId: session.callSessionId, revision: Number(session.revision || 1),
      from, video: session.video === 'Y', callKind: KIND_CODES[session.kindEnumId], chatRoomId: session.chatRoomId,
      expiresAt: ms(session.ringExpireDate), call: view(session, userId) })
  }
  ec.service.async().name('org.moqui.impl.NotificationServices.send#WebPush').parameters({
    userIdList: userIds, title: ec.l10n.localize('Call from') + ' ' + from.name, link: callLink(session.callSessionId),
    topic: 'IncomingCall', tag: 'call-' + session.callSessionId, ttlSeconds: ringSeconds(), urgency: 'high',
  }).call()
}

function answer(p) {
  const me = currentUser()
  const session = me && p.callSessionId ? lockSession(p.callSessionId) : null
  if (!session || !canSee(session, me)) return refuse('notFound', 'Call not found')
  if (session.statusId === 'CssEnded') return refuse('callEnded', 'The call has ended')
  const answered = legOfMine(session, me, ['ClsAnswered'])
  if (answered.leg) return { callLegId: answered.leg.callLegId, call: view(session, me), repeated: true }
  const offered = legOfMine(session, me, ['ClsOffered'])
  if (!offered.leg) return refuse('notRinging', 'This call is not ringing for you')
  const now = ec.user.nowTimestamp
  offered.leg.setFields({ statusId: 'ClsAnswered', answeredDate: now }, true, null, null)
  offered.leg.update()
  offered.participant.setFields({ admissionStatusId: 'CasAdmitted', admittedDate: now }, true, null, null)
  offered.participant.update()
  setLog(session.callSessionId, me, { outcomeEnumId: 'CuoAnswered' })
  if (session.statusId === 'CssRinging') session.setFields({ statusId: 'CssActive', answeredDate: now }, true, null, null)
  else if (!session.answeredDate) session.set('answeredDate', now)
  bump(session)
  session.update()
  pushState(session, 'answered', { answeredBy: me, callLegId: offered.leg.callLegId })
  updatePresence([me])
  return { callLegId: offered.leg.callLegId, call: view(session, me) }
}

function decline(p) {
  const me = currentUser()
  const session = me && p.callSessionId ? lockSession(p.callSessionId) : null
  if (!session || !canSee(session, me)) return refuse('notFound', 'Call not found')
  if (session.statusId === 'CssEnded') return { call: view(session, me), repeated: true }
  const offered = legOfMine(session, me, ['ClsOffered'])
  if (!offered.leg) return refuse('notRinging', 'This call is not ringing for you')
  closeLeg(offered.leg, 'ClsDeclined', 'CerDeclined')
  setLog(session.callSessionId, me, { outcomeEnumId: 'CuoDeclined' })
  const stillRinging = find(LEG).condition('callSessionId', session.callSessionId).condition('statusId', 'ClsOffered').count()
  if (session.kindEnumId === 'CskDirect' && !stillRinging) {
    endSession(session, 'CerDeclined')
  } else {
    bump(session)
    session.update()
    pushState(session, 'declined', { declinedBy: me })
  }
  return { call: view(session, me) }
}

function join(p) {
  const me = currentUser()
  const session = me && p.callSessionId ? lockSession(p.callSessionId) : null
  if (!session || !canSee(session, me)) return refuse('notFound', 'Call not found')
  if (session.statusId === 'CssEnded') return refuse('callEnded', 'The call has ended')
  const answered = legOfMine(session, me, ['ClsAnswered'])
  if (answered.leg) return { callLegId: answered.leg.callLegId, call: view(session, me), repeated: true }
  const offered = legOfMine(session, me, ['ClsOffered'])
  if (offered.leg) return answer(p)
  let participant = participantOf(session.callSessionId, me)
  if (participant && ['CasRevoked', 'CasDenied'].includes(participant.admissionStatusId)) {
    return refuse('notAllowed', 'You were removed from this call')
  }
  if (session.kindEnumId === 'CskDirect' && !participant) return refuse('notFound', 'Call not found')
  if (answeredLegCount(session.callSessionId) >= MAX_INVITEES) return refuse('callFull', 'The call is full')
  const now = ec.user.nowTimestamp
  if (participant) {
    participant.setFields({ admissionStatusId: 'CasAdmitted', admittedDate: participant.admittedDate || now }, true, null, null)
    participant.update()
  } else {
    participant = makeParticipant(session, me, 'CprMember', 'CasAdmitted')
  }
  const leg = makeLeg(session, participant, 'ClsAnswered')
  setLog(session.callSessionId, me, Object.assign({ outcomeEnumId: 'CuoAnswered' },
    one(USER_LOG, { callSessionId: session.callSessionId, userId: me }) ? {} : { roleEnumId: 'CurCallee', logDate: now }))
  if (session.statusId === 'CssRinging') session.setFields({ statusId: 'CssActive', answeredDate: now }, true, null, null)
  bump(session)
  session.update()
  pushState(session, 'joined', { joinedBy: me })
  updatePresence([me])
  return { callLegId: leg.callLegId, call: view(session, me) }
}

function leave(p) {
  const me = currentUser()
  const session = me && p.callSessionId ? lockSession(p.callSessionId) : null
  if (!session || !canSee(session, me)) return refuse('notFound', 'Call not found')
  if (session.statusId === 'CssEnded') return { call: view(session, me), repeated: true }
  const mine = legOfMine(session, me, ['ClsAnswered'])
  const leg = p.callLegId ? (mine.leg?.callLegId === p.callLegId ? mine.leg : null) : mine.leg
  if (!leg) {
    if (p.callLegId) return refuse('notFound', 'Call not found')
    return { call: view(session, me), repeated: true }
  }
  return leaveLeg(session, leg, 'CerHangup', me)
}

/** A leg leaves; the call ends with its last leg, or a ringing direct call when its caller hangs up. */
function leaveLeg(session, leg, reason, userId) {
  closeLeg(leg, 'ClsLeft', reason)
  mediaControl(session, { action: 'closeLeg', callLegId: leg.callLegId })
  const remaining = answeredLegCount(session.callSessionId)
  if (session.statusId === 'CssRinging') {
    endSession(session, 'CerCanceled')
  } else if (session.kindEnumId === 'CskDirect' || remaining === 0) {
    endSession(session, remaining === 0 && session.kindEnumId !== 'CskDirect' ? 'CerEmpty' : reason)
  } else {
    bump(session)
    session.update()
    pushState(session, 'left', { leftBy: userId || null })
    updatePresence([userId])
  }
  return { call: view(session, userId) }
}

function end(p) {
  const me = currentUser()
  const session = me && p.callSessionId ? lockSession(p.callSessionId) : null
  if (!session || !canSee(session, me)) return refuse('notFound', 'Call not found')
  if (session.statusId === 'CssEnded') return { call: view(session, me), repeated: true }
  const participant = participantOf(session.callSessionId, me)
  const host = participant?.roleEnumId === 'CprHost'
  if (!host && session.kindEnumId !== 'CskDirect') return refuse('notAllowed', 'Only the host ends the call for everyone')
  if (!participant) return refuse('notFound', 'Call not found')
  endSession(session, session.statusId === 'CssRinging' ? 'CerCanceled' : 'CerHangup')
  return { call: view(session, me) }
}

function bind(p) {
  const me = currentUser()
  const session = me && p.callSessionId ? lockSession(p.callSessionId) : null
  if (!session || !canSee(session, me)) return refuse('notFound', 'Call not found')
  if (session.statusId === 'CssEnded') return refuse('callEnded', 'The call has ended')
  const leg = p.callLegId ? one(LEG, { callLegId: p.callLegId }) : null
  const participant = leg ? one(PARTICIPANT, { callParticipantId: leg.callParticipantId }) : null
  if (!leg || leg.callSessionId !== session.callSessionId || participant?.userId !== me) {
    return refuse('notFound', 'Call not found')
  }
  if (participant.admissionStatusId !== 'CasAdmitted') return refuse('notAdmitted', 'You are not in this call yet')
  if (leg.statusId !== 'ClsAnswered') return refuse('legClosed', 'You are not in this call')
  const now = ec.user.nowTimestamp
  let generation = 0
  for (const conn of rows(CONNECTION, { callLegId: leg.callLegId })) {
    generation = Math.max(generation, Number(conn.generation || 0))
    if (conn.statusId === 'CcsActive') {
      conn.setFields({ statusId: 'CcsReplaced', closedDate: now }, true, null, null)
      conn.update()
    }
  }
  generation += 1
  const conn = ec.entity.makeValue(CONNECTION)
  conn.setSequencedIdPrimary()
  conn.setFields({
    callLegId: leg.callLegId, generation, signalingNodeId: p.signalingNodeId || null, connectedDate: now,
    lastHeartbeatDate: now, statusId: 'CcsActive',
  }, true, null, null)
  conn.create()
  return {
    callConnectionId: conn.callConnectionId, generation, mediaNodeId: session.mediaNodeId,
    callLegId: leg.callLegId, call: view(session, me),
  }
}

function heartbeat(p) {
  const list = Array.isArray(p.connectionList) ? p.connectionList : []
  if (!list.length) return { updatedCount: 0 }
  const wanted = new Map(list.map((c) => [String(c.callLegId) + ':' + Number(c.generation), true]))
  const legIds = Array.from(new Set(list.map((c) => String(c.callLegId))))
  const now = ec.user.nowTimestamp
  let updatedCount = 0
  for (const conn of find(CONNECTION).condition('callLegId', 'in', legIds).condition('statusId', 'CcsActive').list()) {
    if (!wanted.has(conn.callLegId + ':' + Number(conn.generation))) continue
    conn.set('lastHeartbeatDate', now)
    conn.update()
    updatedCount++
  }
  return { updatedCount }
}

function release(p) {
  const conn = find(CONNECTION).condition('callLegId', p.callLegId).condition('generation', Number(p.generation || 0)).one()
  if (!conn || conn.statusId !== 'CcsActive') return { released: false }
  conn.setFields({ statusId: 'CcsClosed', closedDate: ec.user.nowTimestamp }, true, null, null)
  conn.update()
  return { released: true }
}

function mediaEvent(p) {
  const leg = one(LEG, { callLegId: p.callLegId })
  if (!leg) return {}
  const conn = find(CONNECTION).condition('callLegId', p.callLegId).condition('generation', Number(p.generation || 0)).one()
  if (!conn || conn.statusId !== 'CcsActive') return {}
  if (p.event === 'connected' && !leg.joinedDate) {
    leg.set('joinedDate', ec.user.nowTimestamp)
    leg.update()
  } else if (p.event === 'quality' && p.quality) {
    leg.set('qualityJson', JSON.stringify(p.quality).slice(0, 4000))
    leg.update()
  }
  return {}
}

/** Each item on its own: one that fails is logged by name and the rest go on. */
function eachSafely(label, items) {
  let done = 0
  for (const item of items) {
    ec.message.pushErrors()
    try {
      const out = ec.service.sync().name('org.moqui.impl.CallServices.expire#Item')
        .parameters(Object.assign({ itemKind: label }, item)).requireNewTransaction(true).disableAuthz().call()
      if (ec.message.hasError()) {
        ec.logger.error(`Call cleanup ${label} ${JSON.stringify(item)} failed: ${ec.message.errorsString}`)
      } else if (out?.done) {
        done++
      }
    } catch (err) {
      ec.logger.error(`Call cleanup ${label} ${JSON.stringify(item)} failed: ${err && err.message ? err.message : err}`)
    } finally {
      ec.message.clearErrors()
      ec.message.popErrors()
    }
  }
  return done
}

function expire() {
  const now = nowMs()
  const ringing = Array.from(find(SESSION).condition('statusId', 'CssRinging')
    .condition('ringExpireDate', '<', new Date(now)).selectField('callSessionId').limit(500).list())
    .map((s) => ({ callSessionId: s.callSessionId }))
  const offered = Array.from(find(LEG).condition('statusId', 'ClsOffered')
    .condition('offeredDate', '<', new Date(now - ringSeconds() * 1000)).selectField('callLegId').limit(500).list())
    .map((l) => ({ callLegId: l.callLegId }))
  const staleSince = new Date(now - HEARTBEAT_STALE_MS)
  const closedSince = new Date(now - RECONNECT_GRACE_MS)
  const answered = Array.from(find(LEG).condition('statusId', 'ClsAnswered').selectField('callLegId')
    .selectField('answeredDate').limit(2000).list())
  const lost = []
  for (const leg of answered) {
    const conns = rows(CONNECTION, { callLegId: leg.callLegId })
    const active = conns.find((c) => c.statusId === 'CcsActive')
    if (active) {
      if (ms(active.lastHeartbeatDate) < staleSince.getTime()) lost.push({ callLegId: leg.callLegId })
    } else if (conns.length) {
      const last = Math.max(...conns.map((c) => ms(c.closedDate) || 0))
      if (last < closedSince.getTime()) lost.push({ callLegId: leg.callLegId })
    } else if ((ms(leg.answeredDate) || 0) < now - NEVER_CONNECTED_MS) {
      lost.push({ callLegId: leg.callLegId })
    }
  }
  const liveNodes = new Set(liveMediaNodes().map((n) => n.instanceId))
  const orphaned = Array.from(find(SESSION).condition('statusId', '!=', 'CssEnded').selectField('callSessionId')
    .selectField('mediaNodeId').limit(2000).list()).filter((s) => s.mediaNodeId && !liveNodes.has(s.mediaNodeId))
    .map((s) => ({ callSessionId: s.callSessionId }))
  return {
    endedCount: eachSafely('ringing', ringing) + eachSafely('mediaNodeLost', orphaned),
    legCount: eachSafely('offered', offered) + eachSafely('connectionLost', lost),
  }
}

/** One cleanup item, in its own transaction. */
function expireItem(p) {
  if (p.itemKind === 'ringing' || p.itemKind === 'mediaNodeLost') {
    const session = lockSession(p.callSessionId)
    if (!session || session.statusId === 'CssEnded') return { done: false }
    if (p.itemKind === 'ringing' && session.statusId !== 'CssRinging') return { done: false }
    endSession(session, p.itemKind === 'ringing' ? 'CerNoAnswer' : 'CerMediaNodeLost')
    return { done: true }
  }
  const leg = one(LEG, { callLegId: p.callLegId })
  if (!leg) return { done: false }
  const session = lockSession(leg.callSessionId)
  const current = one(LEG, { callLegId: p.callLegId })
  if (!session || session.statusId === 'CssEnded') return { done: false }
  const participant = one(PARTICIPANT, { callParticipantId: current.callParticipantId })
  if (p.itemKind === 'offered' && current.statusId === 'ClsOffered') {
    closeLeg(current, 'ClsTimedOut', 'CerNoAnswer')
    if (participant?.userId) {
      const log = one(USER_LOG, { callSessionId: session.callSessionId, userId: participant.userId })
      if (log && !log.outcomeEnumId) setLog(session.callSessionId, participant.userId, { outcomeEnumId: 'CuoMissed' })
      ec.service.sync().name('org.moqui.impl.CallServices.publish#MissedCounts')
        .parameter('userIdList', [participant.userId]).disableAuthz().call()
    }
    bump(session)
    session.update()
    pushState(session, 'timedOut')
    return { done: true }
  }
  if (p.itemKind === 'connectionLost' && current.statusId === 'ClsAnswered') {
    leaveLeg(session, current, 'CerConnectionLost', participant?.userId || null)
    return { done: true }
  }
  return { done: false }
}

function get(p) {
  const me = currentUser()
  const session = me && p.callSessionId ? one(SESSION, { callSessionId: p.callSessionId }) : null
  if (!session || !canSee(session, me)) return refuse('notFound', 'Call not found')
  return { call: view(session, me) }
}

/** The calls ringing for the user or that the user is in. */
function active() {
  const me = currentUser()
  if (!me) return { callList: [] }
  const participants = Array.from(find(PARTICIPANT).condition('userId', me).selectField('callParticipantId')
    .selectField('callSessionId').orderBy('-callParticipantId').limit(100).list())
  if (!participants.length) return { callList: [] }
  const sessions = Array.from(find(SESSION).condition('callSessionId', 'in', participants.map((x) => x.callSessionId))
    .condition('statusId', '!=', 'CssEnded').orderBy('-startedDate').limit(10).list())
  const callList = sessions.map((s) => view(s, me)).filter((c) => ['offered', 'answered'].includes(c.myLegStatus))
  return { callList }
}

const OUTCOME_CODES = { CuoPlaced: 'placed', CuoAnswered: 'answered', CuoMissed: 'missed', CuoDeclined: 'declined' }
const OUTCOMES = { placed: 'CuoPlaced', answered: 'CuoAnswered', missed: 'CuoMissed', declined: 'CuoDeclined' }

/**
 * The user's call history, newest first, a page at a time: each call with the others in it, its kind, its outcome for
 * the user (placed, answered, missed, declined), when and how long, and whether a missed one is unseen. outcome filters.
 */
function history(p) {
  const me = currentUser()
  if (!me) return { callList: [], callListCount: 0 }
  const pageSize = Math.min(Math.max(Number(p.pageSize) || 20, 1), 100)
  const pageIndex = Math.max(Number(p.pageIndex) || 0, 0)
  const outcome = OUTCOMES[p.outcome] || null
  let find_ = find(USER_LOG).condition('userId', me)
  if (outcome) find_ = find_.condition('outcomeEnumId', outcome)
  const callListCount = find_.count()
  let logs = find(USER_LOG).condition('userId', me)
  if (outcome) logs = logs.condition('outcomeEnumId', outcome)
  const rowsOf = Array.from(logs.orderBy('-logDate').offset(pageIndex * pageSize).limit(pageSize).list())
  if (!rowsOf.length) return { callList: [], callListCount }
  const ids = rowsOf.map((r) => r.callSessionId)
  const sessions = new Map(Array.from(find(SESSION).condition('callSessionId', 'in', ids).list()).map((s) => [s.callSessionId, s]))
  const people = Array.from(find(PARTICIPANT).condition('callSessionId', 'in', ids).selectField('callSessionId')
    .selectField('userId').selectField('displayName').list())
  const names = userNames(people.map((x) => x.userId))
  const callList = rowsOf.map((log) => {
    const session = sessions.get(log.callSessionId) || {}
    const others = people.filter((x) => x.callSessionId === log.callSessionId && x.userId !== me)
    return {
      callSessionId: log.callSessionId,
      outcome: OUTCOME_CODES[log.outcomeEnumId] || (session.statusId === 'CssEnded' ? 'missed' : 'ringing'),
      role: log.roleEnumId === 'CurCaller' ? 'caller' : 'callee',
      kind: KIND_CODES[session.kindEnumId] || null,
      video: session.video === 'Y',
      status: STATUS_CODES[session.statusId] || null,
      chatRoomId: session.chatRoomId || null,
      startedDate: ms(log.logDate || session.startedDate),
      durationSeconds: Number(log.durationSeconds || 0),
      unseen: log.outcomeEnumId === 'CuoMissed' && !log.seenDate,
      otherUserIdList: others.map((x) => x.userId).filter((id) => id),
      otherNames: others.map((x) => x.displayName || names[x.userId] || x.userId),
      url: callLink(log.callSessionId),
    }
  })
  return { callList, callListCount }
}

/** The user's missed calls not seen yet. */
function countMissed(p) {
  const me = currentUser()
  const userId = p.userId || me
  if (!userId) return { missedCount: 0 }
  if (userId !== me && !ec.artifactExecution.authzDisabled()) return refuse('notAllowed', 'Another user\'s calls are read only internally')
  return { missedCount: find(USER_LOG).condition('userId', userId).condition('outcomeEnumId', 'CuoMissed')
    .condition('seenDate', null).count() }
}

/** The user saw their missed calls: they no longer count. */
function markMissedSeen() {
  const me = currentUser()
  if (!me) return { markedCount: 0 }
  let markedCount = 0
  const now = ec.user.nowTimestamp
  for (const log of find(USER_LOG).condition('userId', me).condition('outcomeEnumId', 'CuoMissed').condition('seenDate', null)
    .limit(1000).list()) {
    log.set('seenDate', now)
    log.update()
    markedCount++
  }
  if (markedCount) {
    ec.service.sync().name('org.moqui.impl.CallServices.publish#MissedCounts').parameter('userIdList', [me]).disableAuthz().call()
  }
  return { markedCount }
}

function registerNode(p) {
  const row = find(MEDIA_NODE).condition('instanceId', p.instanceId).one()
  const now = ec.user.nowTimestamp
  if (p.stopping) {
    if (row) row.delete()
    return {}
  }
  if (row) {
    row.setFields({ mediaAddress: p.mediaAddress || row.mediaAddress, heartbeatDate: now,
      draining: p.draining ? 'Y' : 'N' }, true, null, null)
    row.update()
  } else {
    const created = ec.entity.makeValue(MEDIA_NODE)
    created.setFields({ instanceId: p.instanceId, mediaAddress: p.mediaAddress || null, startedDate: now,
      heartbeatDate: now, draining: p.draining ? 'Y' : 'N' }, true, null, null)
    created.create()
  }
  return {}
}

function capabilities() {
  const supported = !!ec.factory.hasFeature('rtc')
  const live = liveMediaNodes().filter((n) => n.draining !== 'Y')
  const configured = supported && String(System.getProperty('rtc_media_port') || '').trim() !== ''
  const reasonCode = live.length ? null : !supported && !live.length ? 'notBuilt' : 'noMediaServer'
  return { supported, configured, active: live.length > 0, reasonCode }
}

const operations = {
  start, answer, decline, join, leave, end, bind, heartbeat, release, mediaEvent, expire, expireItem, get, active,
  registerNode, capabilities, history, countMissed, markMissedSeen,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown call operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
