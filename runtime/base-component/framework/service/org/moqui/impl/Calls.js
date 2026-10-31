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
const LINK = 'moqui.call.CallLink'
const GUEST = 'moqui.call.CallGuest'

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
const IDENTITY_CODES = { CikLocal: 'local', CikGuest: 'guest', CikPhone: 'phone' }
const LEG_TYPE_CODES = { CltWeb: 'web', CltGuest: 'guest', CltSipDevice: 'sipDevice', CltPstn: 'pstn' }

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
/** How long a guest's credential lasts. */
const GUEST_HOURS = 4
/** How long a call link lasts by default, and at most, in hours. */
const LINK_HOURS = 24 * 7
const LINK_MAX_HOURS = 24 * 90

const ms = (value) => {
  if (value == null || value === '') return null
  const number = Number(value)
  return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}
const nowMs = () => ms(ec.user.nowTimestamp)
const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const rows = (entity, fields) => Array.from(find(entity).condition(fields).list())
/** A desk or soft phone's leg (V4), which the SIP engine rings and answers; a user's own legs in the browser are the others. */
const isDeviceLeg = (leg) => leg.legTypeEnumId === 'CltSipDevice'
/** The media node a call from a phone must use: its SIP engine's (startFromDevice), for start to take. */
let preferredNode = null

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
  if (participant) return false
  if (linkOwner(session) === userId) return true
  return session.kindEnumId !== 'CskDirect' && roomMember(session.chatRoomId, userId)
}

/** The owner of the call link a meeting was opened by. */
function linkOwner(session) {
  if (!session.callLinkId) return null
  return one(LINK, { callLinkId: session.callLinkId })?.ownerUserId || null
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
      // the user's leg in a browser: a phone's leg is the SIP engine's, never one a tab answers or joins
      const tabs = own.filter((leg) => !isDeviceLeg(leg))
      myLeg = tabs.find((leg) => leg.statusId === 'ClsAnswered') || tabs.find((leg) => leg.statusId === 'ClsOffered') ||
        tabs[tabs.length - 1] || null
    }
    return {
      callParticipantId: p.callParticipantId,
      userId: p.userId || null,
      displayName: p.displayName || names[p.userId] || p.userId,
      role: p.roleEnumId === 'CprHost' ? 'host' : p.roleEnumId === 'CprGuest' ? 'guest' : 'member',
      identity: IDENTITY_CODES[p.identityKindEnumId] || 'local',
      admission: ADMISSION_CODES[p.admissionStatusId] || null,
      inCall: own.some((leg) => leg.statusId === 'ClsAnswered'),
      ringing: own.some((leg) => leg.statusId === 'ClsOffered'),
      legs: own.map((leg) => ({ callLegId: leg.callLegId, status: LEG_CODES[leg.statusId] || null,
        type: LEG_TYPE_CODES[leg.legTypeEnumId] || 'web' })),
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

/**
 * The call's recording (W5, CallRecordings.js) hears of a leg bound (a late joiner's notice), a leg gone, or the end;
 * only while one is going. It never fails the call's own change.
 */
function recordingChange(callSessionId, change, callLegId) {
  if (find('moqui.call.CallRecording').condition('callSessionId', callSessionId)
    .condition('statusId', 'in', ['CrsRecording', 'CrsPaused']).count() === 0) return
  ec.service.sync().name('org.moqui.impl.CallRecordingServices.note#CallChange')
    .parameters({ callSessionId, change, callLegId: callLegId || null }).disableAuthz().call()
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

function makeLeg(session, participant, status, legType) {
  const now = ec.user.nowTimestamp
  const leg = ec.entity.makeValue(LEG)
  leg.setSequencedIdPrimary()
  leg.setFields({
    callSessionId: session.callSessionId, callParticipantId: participant.callParticipantId, legTypeEnumId: legType || 'CltWeb',
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
    recordingChange(leg.callSessionId, 'legLeft', leg.callLegId)
  }
}

/** Ends the call: its open legs close, the callees it rang and nobody answered have missed it. */
function endSession(session, reason) {
  if (session.statusId === 'CssEnded') return
  const now = ec.user.nowTimestamp
  // a recording stops first, so the legs closing below do not resume it
  recordingChange(session.callSessionId, 'ended')
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
  const legs = rows(LEG, { callParticipantId: participant.callParticipantId })
    .filter((leg) => statuses.includes(leg.statusId) && !isDeviceLeg(leg))
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

  const nodes = liveMediaNodes().filter((n) => n.draining !== 'Y' && (!preferredNode || n.instanceId === preferredNode))
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
function ring(session, userIds, fromUserId, fromName) {
  if (!userIds.length) return
  const from = { userId: fromUserId || null, name: fromName || userNames([fromUserId])[fromUserId] || fromUserId }
  for (const userId of userIds) {
    push([userId], { action: 'ring', callSessionId: session.callSessionId, revision: Number(session.revision || 1),
      from, video: session.video === 'Y', callKind: KIND_CODES[session.kindEnumId], chatRoomId: session.chatRoomId,
      expiresAt: ms(session.ringExpireDate), call: view(session, userId) })
  }
  ec.service.async().name('org.moqui.impl.NotificationServices.send#WebPush').parameters({
    userIdList: userIds, title: ec.l10n.localize('Call from') + ' ' + from.name, link: callLink(session.callSessionId),
    topic: 'IncomingCall', tag: 'call-' + session.callSessionId, ttlSeconds: ringSeconds(), urgency: 'high',
  }).call()
  // and their desk and soft phones (V4, SipDeviceServices); a failure leaves the tabs ringing
  ec.message.pushErrors()
  try {
    ec.service.sync().name('org.moqui.impl.SipDeviceServices.ring#SipDevices').parameters({ callSessionId: session.callSessionId,
      userIdList: userIds, fromUserId: fromUserId || null, fromName: from.name }).disableAuthz().call()
    if (ec.message.hasError()) ec.logger.error(`Call ${session.callSessionId}: phones not rung: ${ec.message.errorsString}`)
  } catch (err) {
    ec.logger.error(`Call ${session.callSessionId}: phones not rung: ${err && err.message ? err.message : err}`)
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
}

/** Tells the SIP engine of the call's node about one leg ({action: 'answer' | 'hangup'}), after the commit; logged if not. */
function sipLegControl(session, callLegId, action) {
  if (!session.mediaNodeId) return
  ec.message.pushErrors()
  try {
    ec.service.sync().name('org.moqui.impl.VoiceServices.send#SipControl').parameters({
      instanceId: session.mediaNodeId, message: { action, callSessionId: session.callSessionId, callLegId },
    }).disableAuthz().call()
    if (ec.message.hasError()) ec.logger.error(`SIP engine not told to ${action} leg ${callLegId}: ${ec.message.errorsString}`)
  } catch (err) {
    ec.logger.error(`SIP engine not told to ${action} leg ${callLegId}: ${err && err.message ? err.message : err}`)
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
}

/**
 * The first answer won for this participant: their other offers end (V4): the tabs' legs close, and each phone that rings
 * for them is canceled by the SIP engine.
 */
function cancelSiblingOffers(session, participant, winningLegId) {
  for (const other of rows(LEG, { callParticipantId: participant.callParticipantId, statusId: 'ClsOffered' })) {
    if (other.callLegId === winningLegId) continue
    closeLeg(other, 'ClsCanceled', 'CerCanceled')
    if (isDeviceLeg(other)) sipLegControl(session, other.callLegId, 'hangup')
  }
}

/** A phone that placed this call (a CltSipDevice leg the caller is in) hears the answer: its INVITE gets its 200. */
function answerCallerDevices(session) {
  for (const leg of rows(LEG, { callSessionId: session.callSessionId, legTypeEnumId: 'CltSipDevice', statusId: 'ClsAnswered' })) {
    sipLegControl(session, leg.callLegId, 'answer')
  }
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
  // the user's phones that ring for this call stop (V4)
  cancelSiblingOffers(session, offered.participant, offered.leg.callLegId)
  if (session.statusId === 'CssRinging') session.setFields({ statusId: 'CssActive', answeredDate: now }, true, null, null)
  else if (!session.answeredDate) session.set('answeredDate', now)
  bump(session)
  session.update()
  pushState(session, 'answered', { answeredBy: me, callLegId: offered.leg.callLegId })
  updatePresence([me])
  if (session.kindEnumId === 'CskPhone' && session.directionEnumId === 'CdrInbound') phoneControl(session, 'answer')
  answerCallerDevices(session)
  return { callLegId: offered.leg.callLegId, call: view(session, me) }
}

function decline(p) {
  const me = currentUser()
  const session = me && p.callSessionId ? lockSession(p.callSessionId) : null
  if (!session || !canSee(session, me)) return refuse('notFound', 'Call not found')
  if (session.statusId === 'CssEnded') return { call: view(session, me), repeated: true }
  const offered = legOfMine(session, me, ['ClsOffered'])
  if (!offered.leg) return refuse('notRinging', 'This call is not ringing for you')
  declineRing(session, offered.participant, offered.leg, me)
  return { call: view(session, me) }
}

/**
 * A user declined the call on one of their legs, a tab (decline) or a phone (deviceEvent): their ring ends on every leg
 * (the tabs' close, the SIP engine cancels each phone, V4) and stays declined. A direct call nobody else is rung for ends;
 * an inbound phone call nobody else is rung for goes to voicemail (V2: the SIP engine starts it, begin#Voicemail).
 */
function declineRing(session, participant, leg, userId) {
  closeLeg(leg, 'ClsDeclined', 'CerDeclined')
  cancelSiblingOffers(session, participant, leg.callLegId)
  setLog(session.callSessionId, userId, { outcomeEnumId: 'CuoDeclined' })
  const stillRinging = find(LEG).condition('callSessionId', session.callSessionId).condition('statusId', 'ClsOffered').count()
  if (session.kindEnumId === 'CskDirect' && !stillRinging) {
    endSession(session, 'CerDeclined')
    return
  }
  bump(session)
  session.update()
  pushState(session, 'declined', { declinedBy: userId })
  if (session.kindEnumId === 'CskPhone' && session.directionEnumId === 'CdrInbound' && !stillRinging) phoneControl(session, 'voicemail')
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
  const owner = !participant && linkOwner(session) === me
  if (answeredLegCount(session.callSessionId) >= MAX_INVITEES) return refuse('callFull', 'The call is full')
  const now = ec.user.nowTimestamp
  if (participant) {
    participant.setFields({ admissionStatusId: 'CasAdmitted', admittedDate: participant.admittedDate || now }, true, null, null)
    participant.update()
  } else {
    participant = makeParticipant(session, me, owner ? 'CprHost' : 'CprMember', 'CasAdmitted')
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
  } else if (session.kindEnumId === 'CskDirect' || remaining === 0 ||
    (session.kindEnumId === 'CskPhone' && !localAnsweredCount(session.callSessionId))) {
    endSession(session, remaining === 0 && !['CskDirect', 'CskPhone'].includes(session.kindEnumId) ? 'CerEmpty' : reason)
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
  return bindLeg(session, leg, participant, p.signalingNodeId, me)
}

/** Binds a socket to an admitted leg that is in the call: the leg's next generation, the older connection fenced. */
function bindLeg(session, leg, participant, signalingNodeId, forUserId) {
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
    callLegId: leg.callLegId, generation, signalingNodeId: signalingNodeId || null, connectedDate: now,
    lastHeartbeatDate: now, statusId: 'CcsActive',
  }, true, null, null)
  conn.create()
  recordingChange(session.callSessionId, 'legBound', leg.callLegId)
  return {
    callConnectionId: conn.callConnectionId, generation, mediaNodeId: session.mediaNodeId,
    callLegId: leg.callLegId, call: forUserId ? view(session, forUserId) : null,
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
    .selectField('answeredDate').selectField('legTypeEnumId').limit(2000).list())
    // a phone's leg has no socket here: the SIP engine reports its end
    .filter((leg) => leg.legTypeEnumId !== 'CltPstn' && leg.legTypeEnumId !== 'CltSipDevice')
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
  // a meeting nobody is in or waiting for, a while after it started (a link opened and left)
  const startedBefore = new Date(now - NEVER_CONNECTED_MS)
  const empty = Array.from(find(SESSION).condition('statusId', 'CssActive').condition('kindEnumId', 'CskMeeting')
    .condition('startedDate', '<', startedBefore).selectField('callSessionId').limit(500).list())
    .filter((s) => find(LEG).condition('callSessionId', s.callSessionId).condition('statusId', 'in', OPEN_LEG).count() === 0)
    .map((s) => ({ callSessionId: s.callSessionId }))
  const expiredGuests = Array.from(find(GUEST).condition('revokedDate', null).condition('credentialExpiry', '<', new Date(now))
    .selectField('callGuestId').limit(500).list()).map((g) => ({ callGuestId: g.callGuestId }))
  return {
    endedCount: eachSafely('ringing', ringing) + eachSafely('mediaNodeLost', orphaned) + eachSafely('empty', empty),
    legCount: eachSafely('offered', offered) + eachSafely('connectionLost', lost) + eachSafely('guestExpired', expiredGuests),
  }
}

/** One cleanup item, in its own transaction. */
function expireItem(p) {
  if (p.itemKind === 'guestExpired') {
    const guest = one(GUEST, { callGuestId: p.callGuestId })
    if (!guest || guest.revokedDate) return { done: false }
    revokeGuest(guest, 'CerHangup')
    return { done: true }
  }
  if (p.itemKind === 'empty') {
    const session = lockSession(p.callSessionId)
    if (!session || session.statusId === 'CssEnded') return { done: false }
    if (find(LEG).condition('callSessionId', session.callSessionId).condition('statusId', 'in', OPEN_LEG).count() > 0) return { done: false }
    endSession(session, 'CerEmpty')
    return { done: true }
  }
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
    // a phone call that rings longer than call_ring_seconds before voicemail (its box's rings, V2) keeps ringing until then
    if (session.statusId === 'CssRinging' && (ms(session.ringExpireDate) || 0) > nowMs()) return { done: false }
    closeLeg(current, 'ClsTimedOut', 'CerNoAnswer')
    if (isDeviceLeg(current)) sipLegControl(session, current.callLegId, 'hangup')
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

// ---------- call links and guests (W3) ----------

/** The link a token names, when it is not revoked or expired. */
function linkByToken(token) {
  if (!/^[A-Za-z0-9_-]{16,64}$/.test(String(token || ''))) return null
  const link = find(LINK).condition('tokenHash', ec.user.singleUseCodeHash(String(token))).one()
  if (!link || link.revokedDate) return null
  if (link.expiryDate && ms(link.expiryDate) <= nowMs()) return null
  return link
}

/** A link to a meeting for people with no account; the token is in the answer only. */
function createLink(p) {
  const me = currentUser()
  if (!me) return refuse('notSignedIn', 'Sign in to make a call link')
  const name = String(p.name || '').trim().slice(0, 100)
  if (!name) return refuse('noName', 'Give the link a name')
  const chatRoomId = p.chatRoomId || null
  if (chatRoomId) {
    const access = ec.service.sync().name('org.moqui.impl.ChatServices.get#RoomAccess').parameter('chatRoomId', chatRoomId).call()
    if (!access?.canPost) return refuse('notFound', 'Chat room not found')
  }
  const hours = Math.min(Math.max(Number(p.expiryHours) || LINK_HOURS, 1), LINK_MAX_HOURS)
  const token = ec.user.randomString(32)
  const link = ec.entity.makeValue(LINK)
  link.setSequencedIdPrimary()
  link.setFields({
    tokenHash: ec.user.singleUseCodeHash(token), name, ownerUserId: me, chatRoomId,
    policyEnumId: p.policy === 'link' ? 'ClpLink' : 'ClpLobby', video: p.video === false || p.video === 'false' ? 'N' : 'Y',
    createdDate: ec.user.nowTimestamp, expiryDate: new Date(nowMs() + hours * 3600000),
  }, true, null, null)
  link.create()
  return { callLinkId: link.callLinkId, token, path: '/call/' + token }
}

function linkView(link) {
  const active = Array.from(find(SESSION).condition('callLinkId', link.callLinkId).condition('statusId', '!=', 'CssEnded')
    .limit(1).list())[0]
  const waiting = active ? find(PARTICIPANT).condition('callSessionId', active.callSessionId)
    .condition('admissionStatusId', 'CasWaiting').count() : 0
  return {
    callLinkId: link.callLinkId, name: link.name, policy: link.policyEnumId === 'ClpLink' ? 'link' : 'lobby',
    video: link.video !== 'N', chatRoomId: link.chatRoomId || null, createdDate: ms(link.createdDate),
    expiryDate: ms(link.expiryDate), lastUsedDate: ms(link.lastUsedDate), callSessionId: active ? active.callSessionId : null,
    waitingCount: waiting,
  }
}

/** The user's links that still work, newest first. */
function findLinks() {
  const me = currentUser()
  if (!me) return { linkList: [] }
  const linkList = Array.from(find(LINK).condition('ownerUserId', me).condition('revokedDate', null)
    .condition('expiryDate', '>', new Date(nowMs())).orderBy('-createdDate').limit(100).list()).map(linkView)
  return { linkList }
}

/** A guest leaves the call: their credential ends and their legs close. Removed (CerRemoved), they cannot come back. */
function revokeGuest(guest, reason) {
  const now = ec.user.nowTimestamp
  if (!guest.revokedDate) {
    guest.set('revokedDate', now)
    guest.update()
  }
  const participant = one(PARTICIPANT, { callGuestId: guest.callGuestId })
  if (!participant) return
  if (reason === 'CerRemoved' && !['CasRevoked', 'CasDenied'].includes(participant.admissionStatusId)) {
    participant.setFields({ admissionStatusId: 'CasRevoked', revokedDate: now }, true, null, null)
    participant.update()
  }
  const session = lockSession(participant.callSessionId)
  if (!session || session.statusId === 'CssEnded') return
  for (const leg of rows(LEG, { callParticipantId: participant.callParticipantId })) {
    if (leg.statusId === 'ClsAnswered') {
      leaveLeg(session, leg, reason, null)
    } else if (leg.statusId === 'ClsOffered') {
      closeLeg(leg, 'ClsCanceled', reason)
      bump(session)
      session.update()
      pushState(session, 'left')
    }
  }
}

/** The owner revokes a link: it takes no one more, and its guests are removed. */
function revokeLink(p) {
  const me = currentUser()
  const link = me && p.callLinkId ? find(LINK).condition('callLinkId', p.callLinkId).forUpdate(true).one() : null
  if (!link || link.ownerUserId !== me) return refuse('notFound', 'Call link not found')
  if (link.revokedDate) return { revoked: true, repeated: true }
  link.set('revokedDate', ec.user.nowTimestamp)
  link.update()
  for (const guest of rows(GUEST, { callLinkId: link.callLinkId })) {
    if (!guest.revokedDate) revokeGuest(guest, 'CerRemoved')
  }
  return { revoked: true }
}

/** The token check of the /call/<token> route: what the public page may show of a link that works. */
function checkToken(p) {
  const link = linkByToken(p.token)
  if (!link) return { valid: false, routeContext: null }
  const owner = userNames([link.ownerUserId])[link.ownerUserId] || null
  return {
    valid: true,
    routeContext: { name: link.name, ownerName: owner, policy: link.policyEnumId === 'ClpLink' ? 'link' : 'lobby', video: link.video !== 'N' },
  }
}

/** The meeting of a link: the one going, else a new one (its owner is not in it until they open it). */
function linkSession(link) {
  const going = Array.from(find(SESSION).condition('callLinkId', link.callLinkId).condition('statusId', '!=', 'CssEnded')
    .orderBy('-startedDate').limit(1).list())[0]
  if (going) return lockSession(going.callSessionId)
  const nodes = liveMediaNodes().filter((n) => n.draining !== 'Y')
  if (!nodes.length) return null
  const now = ec.user.nowTimestamp
  const session = ec.entity.makeValue(SESSION)
  session.setSequencedIdPrimary()
  session.setFields({
    chatRoomId: link.chatRoomId || null, callLinkId: link.callLinkId, kindEnumId: 'CskMeeting', statusId: 'CssActive', revision: 1,
    directionEnumId: 'CdrInternal', video: link.video !== 'N' ? 'Y' : 'N', startedByUserId: link.ownerUserId, startedDate: now,
    answeredDate: now, mediaNodeId: nodes[0].instanceId, mediaGeneration: 1, encryptionMode: 'dtls-srtp',
  }, true, null, null)
  session.create()
  return session
}

/** The owner opens a link's meeting from Calls: it starts when none is going, and they are its host. */
function openLink(p) {
  const me = currentUser()
  const link = me && p.callLinkId ? one(LINK, { callLinkId: p.callLinkId }) : null
  if (!link || link.ownerUserId !== me || link.revokedDate || (link.expiryDate && ms(link.expiryDate) <= nowMs())) {
    return refuse('notFound', 'Call link not found')
  }
  const session = linkSession(link)
  if (!session) return refuse('mediaUnavailable', 'Calls are not available: no media server is running')
  const joined = join({ callSessionId: session.callSessionId })
  return Object.assign({ callSessionId: session.callSessionId }, joined)
}

/** A guest opens a link with their name: they are in the meeting, or wait in its lobby. The credential is for their page. */
function guestJoin(p) {
  const link = linkByToken(p.token)
  if (!link) return refuse('notFound', 'This link has expired or was not found')
  const name = String(p.displayName || '').replace(/[\u0000-\u001f\u007f]/g, '').trim().slice(0, 60)
  if (!name) return refuse('noName', 'Give your name')
  const session = linkSession(link)
  if (!session) return refuse('mediaUnavailable', 'Calls are not available: no media server is running')
  const guests = find(PARTICIPANT).condition('callSessionId', session.callSessionId).condition('identityKindEnumId', 'CikGuest')
    .condition('admissionStatusId', 'in', ['CasWaiting', 'CasAdmitted']).count()
  if (guests >= MAX_INVITEES) return refuse('callFull', 'The call is full')
  const now = ec.user.nowTimestamp
  const credential = ec.user.randomString(40)
  const straightIn = link.policyEnumId === 'ClpLink'
  const guest = ec.entity.makeValue(GUEST)
  guest.setSequencedIdPrimary()
  guest.setFields({
    callLinkId: link.callLinkId, callSessionId: session.callSessionId, displayName: name,
    credentialHash: ec.user.singleUseCodeHash(credential), credentialExpiry: new Date(nowMs() + GUEST_HOURS * 3600000),
    createdDate: now, admittedDate: straightIn ? now : null,
  }, true, null, null)
  guest.create()
  const participant = ec.entity.makeValue(PARTICIPANT)
  participant.setSequencedIdPrimary()
  participant.setFields({
    callSessionId: session.callSessionId, identityKindEnumId: 'CikGuest', callGuestId: guest.callGuestId, displayName: name,
    roleEnumId: 'CprGuest', admissionStatusId: straightIn ? 'CasAdmitted' : 'CasWaiting', admittedDate: straightIn ? now : null,
  }, true, null, null)
  participant.create()
  const leg = makeLeg(session, participant, straightIn ? 'ClsAnswered' : 'ClsOffered', 'CltGuest')
  link.set('lastUsedDate', now)
  link.update()
  bump(session)
  session.update()
  pushState(session, straightIn ? 'joined' : 'guestWaiting', { guestName: name })
  // the link's owner hears of a guest even when not in the meeting
  if (!participantOf(session.callSessionId, link.ownerUserId)) {
    push([link.ownerUserId], {
      action: straightIn ? 'joined' : 'guestWaiting', callSessionId: session.callSessionId,
      revision: Number(session.revision || 1), guestName: name, call: view(session, link.ownerUserId),
    })
  }
  return { credential, callSessionId: session.callSessionId, callLegId: leg.callLegId, admission: straightIn ? 'admitted' : 'waiting' }
}

/** The guest a credential names, with their participant and link, when it still works. */
function guestOf(credential) {
  if (!/^[A-Za-z0-9_-]{20,80}$/.test(String(credential || ''))) return null
  const guest = find(GUEST).condition('credentialHash', ec.user.singleUseCodeHash(String(credential))).one()
  if (!guest || guest.revokedDate || ms(guest.credentialExpiry) <= nowMs()) return null
  const link = one(LINK, { callLinkId: guest.callLinkId })
  if (!link || link.revokedDate) return null
  const participant = one(PARTICIPANT, { callGuestId: guest.callGuestId })
  return participant ? { guest, participant, link } : null
}

/** What a guest's page may know: whether they are let in, whether the call goes on, and the names in it. */
function guestState(p) {
  const g = guestOf(p.credential)
  if (!g) return { admission: 'gone', callStatus: null, people: [] }
  const session = one(SESSION, { callSessionId: g.participant.callSessionId })
  const legs = rows(LEG, { callParticipantId: g.participant.callParticipantId })
  const admitted = session ? participantsOf(session.callSessionId).filter((x) => x.admissionStatusId === 'CasAdmitted') : []
  const names = userNames(admitted.map((x) => x.userId))
  return {
    admission: ADMISSION_CODES[g.participant.admissionStatusId] || null,
    callStatus: session ? STATUS_CODES[session.statusId] : null,
    callLegId: (legs.find((leg) => OPEN_LEG.includes(leg.statusId)) || {}).callLegId || null,
    callSessionId: g.participant.callSessionId,
    people: g.participant.admissionStatusId === 'CasAdmitted' ? admitted.map((x) => ({ displayName: x.displayName || names[x.userId] || '' })) : [],
  }
}

/** A guest's socket binds their leg (the host's /callws, after the credential frame). */
function guestBind(p) {
  const g = guestOf(p.credential)
  if (!g || g.participant.callSessionId !== p.callSessionId) return refuse('notFound', 'Call not found')
  const session = lockSession(p.callSessionId)
  if (!session || session.statusId === 'CssEnded') return refuse('callEnded', 'The call has ended')
  const leg = p.callLegId ? one(LEG, { callLegId: p.callLegId }) : null
  if (!leg || leg.callParticipantId !== g.participant.callParticipantId) return refuse('notFound', 'Call not found')
  const participant = one(PARTICIPANT, { callParticipantId: g.participant.callParticipantId })
  return bindLeg(session, leg, participant, p.signalingNodeId, null)
}

/** A guest leaves: their leg closes and their credential ends. */
function guestLeave(p) {
  const g = guestOf(p.credential)
  if (!g) return { left: false }
  revokeGuest(g.guest, 'CerHangup')
  return { left: true }
}

/** Someone in the call (or the link's owner) lets a waiting guest in, or not. */
function admitOrDeny(p, admit) {
  const me = currentUser()
  const session = me && p.callSessionId ? lockSession(p.callSessionId) : null
  if (!session || !canSee(session, me)) return refuse('notFound', 'Call not found')
  if (session.statusId === 'CssEnded') return refuse('callEnded', 'The call has ended')
  const mine = participantOf(session.callSessionId, me)
  const inIt = !!mine && mine.admissionStatusId === 'CasAdmitted' &&
    rows(LEG, { callParticipantId: mine.callParticipantId }).some((leg) => leg.statusId === 'ClsAnswered')
  if (!inIt && linkOwner(session) !== me) return refuse('notAllowed', 'Join the call to let people in')
  const target = p.callParticipantId ? one(PARTICIPANT, { callParticipantId: p.callParticipantId }) : null
  if (!target || target.callSessionId !== session.callSessionId) return refuse('notFound', 'Call not found')
  if (target.admissionStatusId !== 'CasWaiting') return { call: view(session, me), repeated: true }
  const now = ec.user.nowTimestamp
  target.setFields(admit ? { admissionStatusId: 'CasAdmitted', admittedDate: now } : { admissionStatusId: 'CasDenied', revokedDate: now },
    true, null, null)
  target.update()
  for (const leg of rows(LEG, { callParticipantId: target.callParticipantId, statusId: 'ClsOffered' })) {
    if (admit) {
      leg.setFields({ statusId: 'ClsAnswered', answeredDate: now }, true, null, null)
      leg.update()
    } else {
      closeLeg(leg, 'ClsDeclined', 'CerDeclined')
    }
  }
  if (target.callGuestId) {
    const guest = one(GUEST, { callGuestId: target.callGuestId })
    if (guest) {
      guest.set(admit ? 'admittedDate' : 'revokedDate', now)
      guest.update()
    }
  }
  bump(session)
  session.update()
  pushState(session, admit ? 'admitted' : 'denied', { participantName: target.displayName || null })
  return { call: view(session, me) }
}

/** The host (or the link's owner) removes someone from the call: their legs close and they cannot come back. */
function removeParticipant(p) {
  const me = currentUser()
  const session = me && p.callSessionId ? lockSession(p.callSessionId) : null
  if (!session || !canSee(session, me)) return refuse('notFound', 'Call not found')
  const mine = participantOf(session.callSessionId, me)
  if (mine?.roleEnumId !== 'CprHost' && linkOwner(session) !== me) return refuse('notAllowed', 'Only the host removes people')
  const target = p.callParticipantId ? one(PARTICIPANT, { callParticipantId: p.callParticipantId }) : null
  if (!target || target.callSessionId !== session.callSessionId || target.userId === me) return refuse('notFound', 'Call not found')
  if (target.callGuestId) {
    const guest = one(GUEST, { callGuestId: target.callGuestId })
    if (guest) revokeGuest(guest, 'CerRemoved')
  } else {
    target.setFields({ admissionStatusId: 'CasRevoked', revokedDate: ec.user.nowTimestamp }, true, null, null)
    target.update()
    for (const leg of rows(LEG, { callParticipantId: target.callParticipantId })) {
      if (leg.statusId === 'ClsAnswered') leaveLeg(session, leg, 'CerRemoved', target.userId)
      else if (leg.statusId === 'ClsOffered') closeLeg(leg, 'ClsCanceled', 'CerRemoved')
    }
  }
  return { call: view(lockSession(session.callSessionId), me) }
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

/** The ICE servers (TURN, with a credential for the leg) of an admitted leg in the call: its media node's and external. */
function iceServersOf(session, leg) {
  const node = session.mediaNodeId ? one(MEDIA_NODE, { instanceId: session.mediaNodeId }) : null
  const out = ec.service.sync().name('org.moqui.impl.CallServices.make#IceServers').parameters({
    callLegId: leg.callLegId, turnUrls: node?.turnUrls || '',
  }).disableAuthz().call() || {}
  return { iceServers: out.iceServers || [] }
}

function iceServers(p) {
  const me = currentUser()
  const session = me && p.callSessionId ? one(SESSION, { callSessionId: p.callSessionId }) : null
  if (!session || !canSee(session, me) || session.statusId === 'CssEnded') return refuse('notFound', 'Call not found')
  const mine = legOfMine(session, me, ['ClsAnswered'])
  if (!mine.leg || mine.participant.admissionStatusId !== 'CasAdmitted') return { iceServers: [], reasonCode: 'notAdmitted' }
  return iceServersOf(session, mine.leg)
}

function guestIceServers(p) {
  const g = guestOf(p.credential)
  if (!g || g.participant.admissionStatusId !== 'CasAdmitted') return { iceServers: [] }
  const session = one(SESSION, { callSessionId: g.participant.callSessionId })
  const leg = rows(LEG, { callParticipantId: g.participant.callParticipantId }).find((l) => l.statusId === 'ClsAnswered')
  if (!session || session.statusId === 'CssEnded' || !leg) return { iceServers: [] }
  return iceServersOf(session, leg)
}

function registerNode(p) {
  const row = find(MEDIA_NODE).condition('instanceId', p.instanceId).one()
  const now = ec.user.nowTimestamp
  if (p.stopping) {
    if (row) row.delete()
    return {}
  }
  if (row) {
    row.setFields({ mediaAddress: p.mediaAddress || row.mediaAddress, turnUrls: p.turnUrls || null, heartbeatDate: now,
      draining: p.draining ? 'Y' : 'N' }, true, null, null)
    row.update()
  } else {
    const created = ec.entity.makeValue(MEDIA_NODE)
    created.setFields({ instanceId: p.instanceId, mediaAddress: p.mediaAddress || null, turnUrls: p.turnUrls || null,
      startedDate: now, heartbeatDate: now, draining: p.draining ? 'Y' : 'N' }, true, null, null)
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

// ---------- phone legs (V1; org.moqui.impl.VoiceServices and Voice.js) ----------

/** The answered legs of people here (browsers), not of phones. */
function localAnsweredCount(callSessionId) {
  return find(LEG).condition('callSessionId', callSessionId).condition('statusId', 'ClsAnswered')
    .condition('legTypeEnumId', 'in', ['CltWeb', 'CltGuest', 'CltSipDevice']).count()
}

function phoneLegsOf(callSessionId) {
  return rows(LEG, { callSessionId, legTypeEnumId: 'CltPstn' }).filter((leg) => OPEN_LEG.includes(leg.statusId))
}

/** Tells the SIP engine on the call's node about its phone legs ({action: 'answer'}), after the commit. */
function phoneControl(session, action) {
  if (!session.mediaNodeId) return
  for (const leg of phoneLegsOf(session.callSessionId)) {
    // one that cannot be told is logged, as a node that is gone is; the answer stands
    ec.message.pushErrors()
    try {
      ec.service.sync().name('org.moqui.impl.VoiceServices.send#SipControl').parameters({
        instanceId: session.mediaNodeId, message: { action, callSessionId: session.callSessionId, callLegId: leg.callLegId },
      }).disableAuthz().call()
      if (ec.message.hasError()) ec.logger.error(`SIP engine not told to ${action} leg ${leg.callLegId}: ${ec.message.errorsString}`)
    } catch (err) {
      ec.logger.error(`SIP engine not told to ${action} leg ${leg.callLegId}: ${err && err.message ? err.message : err}`)
    } finally {
      ec.message.clearErrors()
      ec.message.popErrors()
    }
  }
}

/**
 * A phone leg that answered hears the call's notices, as a browser leg that binds gets their rtc frames: the recording's
 * (note#CallChange, logged per leg) and, while captions are on, the captions' (VoiceServices.notify#PhoneLeg).
 */
function phoneJoined(session, leg) {
  recordingChange(session.callSessionId, 'legBound', leg.callLegId)
  const captions = one('moqui.call.CallCaptions', { callSessionId: session.callSessionId })
  if (captions && captions.captionsOn === 'Y') {
    ec.service.sync().name('org.moqui.impl.VoiceServices.notify#PhoneLeg')
      .parameters({ callLegId: leg.callLegId, notice: 'captions' }).disableAuthz().call()
  }
}

function makePhoneParticipant(session, displayName, admission) {
  const p = ec.entity.makeValue(PARTICIPANT)
  p.setSequencedIdPrimary()
  p.setFields({
    callSessionId: session.callSessionId, identityKindEnumId: 'CikPhone', displayName, roleEnumId: 'CprMember',
    admissionStatusId: admission, admittedDate: admission === 'CasAdmitted' ? ec.user.nowTimestamp : null,
  }, true, null, null)
  p.create()
  return p
}

/**
 * Internal (Voice.js place#PhoneCall and receive#PhoneCall, which checked the user, the number and the policy): a phone
 * call. Outbound: the user is in it at once and the number rings (a CltPstn leg offered until the SIP engine says it
 * answered). Inbound: the phone is in it (its leg answered) and the user is rung like any callee; silent (V2: the user is
 * busy, the call goes straight to voicemail) offers the user's leg without ringing it, and with no userId (V2: the
 * voicemail access number) nobody is rung and the call is active at once. ringExpireSeconds (V2) keeps an inbound call
 * ringing past call_ring_seconds while its voicemail waits for more rings.
 */
function startPhone(p) {
  const userId = p.userId
  const outbound = p.direction !== 'inbound'
  const key = outbound && p.idempotencyKey ? String(p.idempotencyKey).slice(0, 200) : null
  if (key) {
    const prior = Array.from(find(SESSION).condition('startedByUserId', userId).condition('idempotencyKey', key)
      .orderBy('-startedDate').limit(1).list())[0]
    if (prior) {
      const mine = legOfMine(prior, userId, ['ClsAnswered', 'ClsLeft'])
      const phone = rows(LEG, { callSessionId: prior.callSessionId, legTypeEnumId: 'CltPstn' })[0]
      return { callSessionId: prior.callSessionId, callLegId: mine.leg?.callLegId || null, phoneLegId: phone?.callLegId || null,
        call: view(prior, userId), repeated: true }
    }
  }
  const now = ec.user.nowTimestamp
  const session = ec.entity.makeValue(SESSION)
  session.setSequencedIdPrimary()
  const expireSeconds = Math.max(ringSeconds(), Number(p.ringExpireSeconds) || 0)
  session.setFields({
    kindEnumId: 'CskPhone', statusId: outbound || userId ? 'CssRinging' : 'CssActive', revision: 1,
    directionEnumId: outbound ? 'CdrOutbound' : 'CdrInbound',
    video: 'N', startedByUserId: outbound ? userId : null, idempotencyKey: key, startedDate: now,
    ringExpireDate: new Date(nowMs() + expireSeconds * 1000), mediaNodeId: p.mediaNodeId, mediaGeneration: 1,
    encryptionMode: 'dtls-srtp',
  }, true, null, null)
  session.create()
  const name = p.displayName || p.phoneNumber
  let userLeg
  let phoneLeg
  if (outbound) {
    const host = makeParticipant(session, userId, 'CprHost', 'CasAdmitted')
    userLeg = makeLeg(session, host, 'ClsAnswered')
    const phone = makePhoneParticipant(session, name, 'CasInvited')
    phoneLeg = makeLeg(session, phone, 'ClsOffered', 'CltPstn')
    setLog(session.callSessionId, userId, { roleEnumId: 'CurCaller', outcomeEnumId: 'CuoPlaced', logDate: now })
  } else {
    const phone = makePhoneParticipant(session, name, 'CasAdmitted')
    phoneLeg = makeLeg(session, phone, 'ClsAnswered', 'CltPstn')
    if (userId) {
      const callee = makeParticipant(session, userId, 'CprMember', 'CasInvited')
      userLeg = makeLeg(session, callee, 'ClsOffered')
      setLog(session.callSessionId, userId, { roleEnumId: 'CurCallee', logDate: now })
    }
  }
  phoneLeg.set('phoneNumber', p.phoneNumber)
  phoneLeg.update()
  if (outbound) updatePresence([userId])
  else if (userId && !p.silent) ring(session, [userId], null, name)
  return { callSessionId: session.callSessionId, callLegId: userLeg ? userLeg.callLegId : null, phoneLegId: phoneLeg.callLegId,
    call: view(session, userId) }
}

/**
 * Internal (the SIP engine, through Voice.js note#PhoneLeg): what a phone leg did. answered: the number picked up;
 * failed or busy: it never did, and the call ends; ended or canceled: the far end hung up, and the call ends with its last
 * phone leg. A repeat, or an event for a leg that is closed, answers as it is.
 */
function phoneEvent(p) {
  const session = p.callSessionId ? lockSession(p.callSessionId) : null
  const leg = session && p.callLegId ? one(LEG, { callLegId: p.callLegId }) : null
  if (session && leg && leg.callSessionId === session.callSessionId && isDeviceLeg(leg)) return deviceEvent(session, leg, p)
  if (!session || !leg || leg.callSessionId !== session.callSessionId || leg.legTypeEnumId !== 'CltPstn') {
    return { reasonCode: 'notFound' }
  }
  if (session.statusId === 'CssEnded' || !OPEN_LEG.includes(leg.statusId)) return { repeated: true }
  const now = ec.user.nowTimestamp
  const participant = one(PARTICIPANT, { callParticipantId: leg.callParticipantId })
  if (p.event === 'answered') {
    if (leg.statusId === 'ClsAnswered') return { repeated: true }
    leg.setFields({ statusId: 'ClsAnswered', answeredDate: now, joinedDate: now }, true, null, null)
    leg.update()
    participant.setFields({ admissionStatusId: 'CasAdmitted', admittedDate: now }, true, null, null)
    participant.update()
    if (session.statusId === 'CssRinging') session.setFields({ statusId: 'CssActive', answeredDate: now }, true, null, null)
    bump(session)
    session.update()
    pushState(session, 'answered', { callLegId: leg.callLegId })
    phoneJoined(session, leg)
    answerCallerDevices(session)
    return {}
  }
  if (p.event === 'failed' || p.event === 'busy') {
    if (leg.statusId === 'ClsAnswered') return phoneEvent(Object.assign({}, p, { event: 'ended' }))
    const busy = p.event === 'busy'
    closeLeg(leg, busy ? 'ClsDeclined' : 'ClsFailed', busy ? 'CerDeclined' : 'CerFailed')
    if (!phoneLegsOf(session.callSessionId).length) endSession(session, busy ? 'CerDeclined' : 'CerFailed')
    return {}
  }
  if (p.event === 'ended' || p.event === 'canceled') {
    const wasAnswered = leg.statusId === 'ClsAnswered'
    closeLeg(leg, wasAnswered ? 'ClsLeft' : 'ClsCanceled', wasAnswered ? 'CerHangup' : 'CerCanceled')
    if (!phoneLegsOf(session.callSessionId).length) {
      endSession(session, session.statusId === 'CssRinging' ? 'CerCanceled' : 'CerHangup')
    } else {
      bump(session)
      session.update()
      pushState(session, 'left')
    }
    return {}
  }
  if (p.event === 'held' || p.event === 'resumed') {
    pushState(session, p.event, { callLegId: leg.callLegId })
  }
  return {}
}

/**
 * Internal (VoicemailServices.begin#Voicemail, V2): an inbound phone call goes to voicemail, its phone leg answered by the
 * SIP engine with no one else in it. The users still rung stop ringing, in their tabs and on their phones (V4), and
 * missed it (CallUserLog, as a call that rang out; one who declined keeps that); the call is active with the phone alone until it hangs up. Refused (reasonCode) when the
 * call is not an inbound phone call (notFound), has ended (callEnded) or someone answered it (answered).
 */
function phoneToVoicemail(p) {
  const session = p.callSessionId ? lockSession(p.callSessionId) : null
  const leg = session && p.callLegId ? one(LEG, { callLegId: p.callLegId }) : null
  if (!session || !leg || leg.callSessionId !== session.callSessionId || leg.legTypeEnumId !== 'CltPstn' ||
    session.kindEnumId !== 'CskPhone' || session.directionEnumId !== 'CdrInbound') return { reasonCode: 'notFound' }
  if (session.statusId === 'CssEnded' || leg.statusId !== 'ClsAnswered') return { reasonCode: 'callEnded' }
  if (localAnsweredCount(session.callSessionId)) return { reasonCode: 'answered' }
  for (const offered of rows(LEG, { callSessionId: session.callSessionId, statusId: 'ClsOffered' })) {
    closeLeg(offered, 'ClsTimedOut', 'CerNoAnswer')
    // a desk or soft phone still ringing for it (V4) is canceled: the call stays up, so its room does not close them
    if (isDeviceLeg(offered)) sipLegControl(session, offered.callLegId, 'hangup')
  }
  const callees = participantsOf(session.callSessionId).filter((x) => x.userId).map((x) => x.userId)
  const missed = []
  for (const userId of callees) {
    const log = one(USER_LOG, { callSessionId: session.callSessionId, userId })
    if (log && !log.outcomeEnumId) {
      setLog(session.callSessionId, userId, { outcomeEnumId: 'CuoMissed' })
      missed.push(userId)
    }
  }
  if (session.statusId === 'CssRinging') session.set('statusId', 'CssActive')
  bump(session)
  session.update()
  pushState(session, 'timedOut')
  if (missed.length) {
    ec.service.sync().name('org.moqui.impl.CallServices.publish#MissedCounts').parameter('userIdList', missed)
      .disableAuthz().call()
  }
  return { userIdList: callees, phoneNumber: leg.phoneNumber || null,
    displayName: one(PARTICIPANT, { callParticipantId: leg.callParticipantId })?.displayName || null }
}

// ---------- desk and soft phones (V4; org.moqui.impl.SipDeviceServices and SipDevices.js) ----------

/**
 * Internal (the SIP engine, through note#PhoneLeg): what a phone's leg did. A ringing phone that answers wins only if
 * none of the user's legs answered first (a tab, or another phone): then the user is in the call on it, their other
 * offers end, and an inbound phone call's trunk leg and a calling phone are answered; else it is hung up. A phone that
 * refuses its ring (busy) declines it, as a tab does (declineRing); one that fails or is canceled while it rings just
 * stops; one in the call that hangs up leaves it, as a tab that leaves.
 */
function deviceEvent(session, leg, p) {
  if (session.statusId === 'CssEnded' || !OPEN_LEG.includes(leg.statusId)) {
    // the call or the leg ended here first: a late answer is hung up
    if (p.event === 'answered') sipLegControl(session, leg.callLegId, 'hangup')
    return { repeated: true }
  }
  const participant = one(PARTICIPANT, { callParticipantId: leg.callParticipantId })
  const userId = participant?.userId || null
  const now = ec.user.nowTimestamp
  if (p.event === 'answered') {
    if (leg.statusId === 'ClsAnswered') return { repeated: true }
    const answeredFirst = rows(LEG, { callParticipantId: leg.callParticipantId, statusId: 'ClsAnswered' }).length > 0
    if (answeredFirst || !participant) {
      closeLeg(leg, 'ClsCanceled', 'CerCanceled')
      sipLegControl(session, leg.callLegId, 'hangup')
      return { won: false }
    }
    leg.setFields({ statusId: 'ClsAnswered', answeredDate: now, joinedDate: now }, true, null, null)
    leg.update()
    participant.setFields({ admissionStatusId: 'CasAdmitted', admittedDate: now }, true, null, null)
    participant.update()
    setLog(session.callSessionId, userId, { outcomeEnumId: 'CuoAnswered' })
    cancelSiblingOffers(session, participant, leg.callLegId)
    if (session.statusId === 'CssRinging') session.setFields({ statusId: 'CssActive', answeredDate: now }, true, null, null)
    else if (!session.answeredDate) session.set('answeredDate', now)
    bump(session)
    session.update()
    pushState(session, 'answered', { answeredBy: userId, callLegId: leg.callLegId, onPhone: true })
    updatePresence([userId])
    if (session.kindEnumId === 'CskPhone' && session.directionEnumId === 'CdrInbound') phoneControl(session, 'answer')
    answerCallerDevices(session)
    phoneJoined(session, leg)
    return { won: true }
  }
  if (p.event === 'ended' || p.event === 'canceled' || p.event === 'failed' || p.event === 'busy') {
    if (leg.statusId === 'ClsAnswered') return leaveLeg(session, leg, 'CerHangup', userId)
    // a phone that refuses its ring (busy or decline: phones send either for the reject key) declines it, as a tab does
    if (p.event === 'busy' && participant && userId) {
      declineRing(session, participant, leg, userId)
      return {}
    }
    closeLeg(leg, p.event === 'failed' ? 'ClsFailed' : 'ClsCanceled', p.event === 'failed' ? 'CerFailed' : 'CerCanceled')
    return {}
  }
  if (p.event === 'held' || p.event === 'resumed') pushState(session, p.event, { callLegId: leg.callLegId })
  return {}
}

/**
 * Internal (SipDevices.js receiveDeviceCall, as the phone's user): a call from a phone to other users, as start makes it from
 * a tab, on the media node of the phone's SIP engine, the caller's leg being the phone's.
 */
function startFromDevice(p) {
  preferredNode = p.mediaNodeId || null
  let out
  try {
    out = start({ calleeUserIdList: p.calleeUserIdList, video: false })
  } finally {
    preferredNode = null
  }
  if (!out.callSessionId || !out.callLegId) return out
  deviceCaller({ callLegId: out.callLegId, sipDeviceId: p.sipDeviceId })
  return { callSessionId: out.callSessionId, callLegId: out.callLegId }
}

/** Internal: the caller's leg of a call placed from a phone is that phone's (a CltSipDevice leg). */
function deviceCaller(p) {
  const leg = p.callLegId ? one(LEG, { callLegId: p.callLegId }) : null
  if (!leg) return { reasonCode: 'notFound' }
  leg.setFields({ legTypeEnumId: 'CltSipDevice', sipDeviceId: p.sipDeviceId, joinedDate: ec.user.nowTimestamp }, true, null, null)
  leg.update()
  return {}
}

const operations = {
  startFromDevice, deviceCaller,
  start, startPhone, phoneEvent, phoneToVoicemail, answer, decline, join, leave, end, bind, heartbeat, release, mediaEvent, expire, expireItem, get, active,
  registerNode, capabilities, history, countMissed, markMissedSeen, createLink, findLinks, revokeLink, checkToken, openLink,
  guestJoin, guestState, guestBind, guestLeave, admit: (p) => admitOrDeny(p, true), deny: (p) => admitOrDeny(p, false),
  removeParticipant, iceServers, guestIceServers,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown call operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
