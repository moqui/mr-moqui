// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Call recording (plans/Comms-Framework.md W5, §5.4 "Recording", §6.4, D-V5): org.moqui.call.CallRecordingServices. A
// CallRecording is started by a host of an active call; the call's media node records it (moqui-host src/rtc/recorder.rs),
// told by send#MediaControl ({action: 'recording', op: start | pause | resume | stop | notice}) after the commit, and hands
// back each closed segment file (store#RecordingSegment). Every current leg, and each leg bound later, gets the notice: the
// rtc 'recording' frame the media node sends to every browser leg of the room (and again to a late joiner), or for a leg
// that is not a browser notify#LegRecording, a prompt played to the phone (VoiceServices.notify#PhoneLeg); each notice is
// a CallRecordingConsent row. Under the acknowledgement policy the recording pauses while a current leg has not
// acknowledged, and after any withdraws; resuming starts a new period of segments. A phone acknowledges by pressing 1
// (ack#LegRecordingNotice, from its DTMF) and withdraws by hanging up. Who may do what is in CallRecordingServices.xml.

const SESSION = 'moqui.call.CallSession'
const PARTICIPANT = 'moqui.call.CallParticipant'
const LEG = 'moqui.call.CallLeg'
const LINK = 'moqui.call.CallLink'
const MEDIA_NODE = 'moqui.call.CallMediaNode'
const RECORDING = 'moqui.call.CallRecording'
const TRACK = 'moqui.call.CallRecordingTrack'
const SEGMENT = 'moqui.call.CallRecordingSegment'
const CONSENT = 'moqui.call.CallRecordingConsent'
const CONSENT_HISTORY = 'moqui.call.CallRecordingConsentHistory'
/** A recording's transcripts (W6's CallTranscript) go with it (plans/Comms-Framework.md §5.4). */
const TRANSCRIPT = 'moqui.call.CallTranscript'

/** Where the files go; store#RecordingFile refuses any other place. */
const ROOT = 'dbresource://moqui/call/recording'
const ACTIVE = ['CrsRecording', 'CrsPaused']
const STATE_CODES = { CrsRecording: 'recording', CrsPaused: 'paused', CrsStopped: 'stopped', CrsFailed: 'failed' }
const KIND_CODES = { CrkPerTrack: 'perTrack', CrkMixed: 'mixed' }
const KINDS = { perTrack: 'CrkPerTrack', mixed: 'CrkMixed' }
const TRACK_KINDS = { audio: 'CrtAudio', video: 'CrtVideo', mix: 'CrtMix' }
const TRACK_CODES = { CrtAudio: 'audio', CrtVideo: 'video', CrtMix: 'mix' }
const WEB_LEGS = ['CltWeb', 'CltGuest']
/** A media node with no heartbeat in this long is lost (as Calls.js). */
const MEDIA_NODE_STALE_MS = 60000
/** Files of a recording that ended this long ago and that no segment names are a partial upload. */
const ORPHAN_AGE_MS = 3600000

const ms = (value) => {
  if (value == null || value === '') return null
  const number = Number(value)
  return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}
const nowMs = () => ms(ec.user.nowTimestamp)
const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const rows = (entity, fields) => Array.from(find(entity).condition(fields).list())

function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

/** The deployment's policy: the default-properties in MoquiDefaultConf.xml. */
/** An end-to-end encrypted call (W8): any media encryption but dtls-srtp, which the server cannot hear. */
function encrypted(session) {
  return !!session.encryptionMode && session.encryptionMode !== 'dtls-srtp'
}

function policy() {
  const flag = (name) => String(System.getProperty(name) || '').trim().toLowerCase() === 'true'
  const days = Number(System.getProperty('call_recording_retention_days') || 0)
  return {
    enabled: flag('call_recording_enabled'),
    ackRequired: flag('call_recording_ack_required'),
    revision: String(System.getProperty('call_recording_policy_revision') || '1'),
    retentionDays: days > 0 ? days : 0,
  }
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

function linkOwner(session) {
  if (!session.callLinkId) return null
  return one(LINK, { callLinkId: session.callLinkId })?.ownerUserId || null
}

/** Whether the user may see the call (as Calls.js): a participant not removed, the link's owner, or a member of the room of a group call. */
function canSee(session, userId) {
  if (!session || !userId) return false
  const participant = one(PARTICIPANT, { callSessionId: session.callSessionId, userId })
  if (participant) return participant.admissionStatusId !== 'CasRevoked'
  if (linkOwner(session) === userId) return true
  if (session.kindEnumId === 'CskDirect' || !session.chatRoomId) return false
  const access = ec.service.sync().name('org.moqui.chat.ChatServices.get#RoomAccess')
    .parameters({ chatRoomId: session.chatRoomId, userId }).disableAuthz().call()
  return !!access?.isMember && !access?.isArchived
}

/** The rule of who records: a host of the call (its starter, admitted) or the owner of the call link it was opened by. */
function isHost(session, userId) {
  if (!session || !userId) return false
  const participant = one(PARTICIPANT, { callSessionId: session.callSessionId, userId })
  if (participant?.roleEnumId === 'CprHost' && participant.admissionStatusId === 'CasAdmitted') return true
  return linkOwner(session) === userId
}

/** The rule of who reads: the recording's owner, or a host of its call. */
function canRead(recording, userId) {
  if (!recording || !userId) return false
  if (recording.ownerUserId === userId) return true
  return isHost(one(SESSION, { callSessionId: recording.callSessionId }), userId)
}

function currentLegs(callSessionId) {
  return rows(LEG, { callSessionId, statusId: 'ClsAnswered' })
}

function activeRecording(callSessionId) {
  return Array.from(find(RECORDING).condition('callSessionId', callSessionId).condition('statusId', 'in', ACTIVE)
    .orderBy('-startedDate').limit(1).list())[0] || null
}

function recordingView(recording) {
  if (!recording) return null
  return {
    callRecordingId: recording.callRecordingId,
    callSessionId: recording.callSessionId,
    state: STATE_CODES[recording.statusId] || null,
    pauseReason: recording.statusId === 'CrsPaused' ? recording.pauseReason || null : null,
    kind: KIND_CODES[recording.kindEnumId] || null,
    ackRequired: recording.ackRequired === 'Y',
    period: Number(recording.segmentPeriod || 0),
    startedDate: ms(recording.startedDate),
    endedDate: ms(recording.endedDate),
    lastErrorText: recording.lastErrorText || null,
  }
}

// ---------- the drive quota ----------

/** The bytes the stored files of the recordings userId owns take. */
function recordingBytes(userId) {
  if (!userId) return 0
  const ids = Array.from(find(RECORDING).condition('ownerUserId', userId).selectField('callRecordingId').list()).map((r) => r.callRecordingId)
  let total = 0
  for (let i = 0; i < ids.length; i += 500) {
    for (const segment of find(SEGMENT).condition('callRecordingId', 'in', ids.slice(i, i + 500)).selectField('byteSize').list()) {
      total += Number(segment.byteSize || 0)
    }
  }
  return total
}

/**
 * What is left of the quota of userId's personal drive (My files), which their recordings count toward with its files;
 * null when it has no quota (or no drive yet). A drive that cannot be sized is logged and taken as having no room.
 */
function quotaLeft(userId) {
  const drive = userId ? Array.from(find('moqui.resource.ResourceDrive').condition('ownerUserId', userId)
    .condition('driveKindEnumId', 'RdkPersonal').orderBy('driveId').limit(1).list())[0] : null
  if (!drive || drive.quotaBytes == null) return null
  let files = 0
  try {
    files = Number(ec.resource.getTreeSize(drive.rootLocation) || 0)
  } catch (err) {
    ec.logger.warn(`Recording quota: drive ${drive.driveId} at ${drive.rootLocation} could not be sized: ${err && err.message ? err.message : err}`)
    return 0
  }
  return Number(drive.quotaBytes) - files - recordingBytes(userId)
}

/** The rtc frame every leg of the room gets (and a late joiner, from the media node). */
function noticeFrame(recording) {
  const view = recordingView(recording)
  return {
    rtc: 'recording', callRecordingId: view.callRecordingId, state: view.state, pauseReason: view.pauseReason,
    kind: view.kind, ackRequired: view.ackRequired, policyRevision: recording.policyRevision || null,
  }
}

/** Tells the recording's media node, after the commit: what to do, with the notice for the legs. */
function control(recording, op, extra) {
  const node = recording.mediaNodeId
  if (!node) return
  ec.service.sync().name('org.moqui.call.CallServices.send#MediaControl').parameters({
    mediaNodeId: node,
    message: Object.assign({
      callSessionId: recording.callSessionId, action: 'recording', op, callRecordingId: recording.callRecordingId,
      kind: KIND_CODES[recording.kindEnumId] || 'perTrack', period: Number(recording.segmentPeriod || 0),
      notice: noticeFrame(recording),
    }, extra || {}),
  }).disableAuthz().call()
}

/** The leg's consent row for a recording: its notice and its current answer. */
function currentConsent(callRecordingId, callLegId) {
  return one(CONSENT, { callRecordingId, callLegId })
}

/**
 * The leg acknowledges (method CamClick or CamDtmf). After a withdrawal, the acknowledgement and the withdrawal it replaces
 * are kept first as a CallRecordingConsentHistory row, so no earlier answer is lost.
 */
function acknowledge(consent, method) {
  const now = ec.user.nowTimestamp
  if (consent.withdrawnDate) {
    const earlier = ec.entity.makeValue(CONSENT_HISTORY)
    earlier.setSequencedIdPrimary()
    earlier.setFields({
      callRecordingConsentId: consent.callRecordingConsentId, callRecordingId: consent.callRecordingId, callLegId: consent.callLegId,
      ackDate: consent.ackDate || null, ackMethodEnumId: consent.ackMethodEnumId || null, withdrawnDate: consent.withdrawnDate,
    }, true, null, null)
    earlier.create()
  }
  consent.setFields({ ackDate: now, ackMethodEnumId: method, withdrawnDate: null }, true, null, null)
  consent.update()
  return consent
}

/** Gives a leg the notice once: its consent row, and for a leg that is not a browser the hook. */
function notice(recording, leg, acknowledged) {
  const existing = currentConsent(recording.callRecordingId, leg.callLegId)
  if (existing) return existing
  const now = ec.user.nowTimestamp
  const web = WEB_LEGS.includes(leg.legTypeEnumId || 'CltWeb')
  const consent = ec.entity.makeValue(CONSENT)
  consent.setSequencedIdPrimary()
  consent.setFields({
    callRecordingId: recording.callRecordingId, callSessionId: recording.callSessionId,
    callParticipantId: leg.callParticipantId, callLegId: leg.callLegId,
    recordingScope: KIND_CODES[recording.kindEnumId] || 'perTrack', policyRevision: recording.policyRevision,
    noticeDate: now, noticeMethodEnumId: web ? 'CnmWeb' : 'CnmVoice', ackRequired: recording.ackRequired === 'Y' ? 'Y' : 'N',
    ackDate: acknowledged ? now : null, ackMethodEnumId: acknowledged ? 'CamClick' : null,
  }, true, null, null)
  consent.create()
  if (!web) {
    ec.service.sync().name('org.moqui.call.CallRecordingServices.notify#LegRecording').parameters({
      callRecordingId: recording.callRecordingId, callLegId: leg.callLegId,
      state: STATE_CODES[recording.statusId] || null, ackRequired: recording.ackRequired === 'Y',
    }).disableAuthz().call()
  }
  return consent
}

/** The state the policy wants now: the host's pause holds; else a withdrawal, then a missing acknowledgement, pauses. */
function wanted(recording) {
  if (recording.statusId === 'CrsPaused' && recording.pauseReason === 'user') return { statusId: 'CrsPaused', pauseReason: 'user' }
  const legs = currentLegs(recording.callSessionId)
  const consents = new Map(rows(CONSENT, { callRecordingId: recording.callRecordingId }).map((c) => [c.callLegId, c]))
  if (legs.some((leg) => consents.get(leg.callLegId)?.withdrawnDate)) return { statusId: 'CrsPaused', pauseReason: 'withdrawn' }
  if (recording.ackRequired === 'Y' && legs.some((leg) => !consents.get(leg.callLegId)?.ackDate)) {
    return { statusId: 'CrsPaused', pauseReason: 'ack' }
  }
  return { statusId: 'CrsRecording', pauseReason: null }
}

/** Moves the recording to what the policy wants and tells the media node; resuming starts the next period. */
function settle(recording) {
  if (!ACTIVE.includes(recording.statusId)) return
  const want = wanted(recording)
  const was = recording.statusId
  if (want.statusId === was && (want.pauseReason || null) === (recording.pauseReason || null)) return
  recording.setFields({ statusId: want.statusId, pauseReason: want.pauseReason }, true, null, null)
  if (was === 'CrsPaused' && want.statusId === 'CrsRecording') {
    recording.set('segmentPeriod', Number(recording.segmentPeriod || 0) + 1)
  }
  recording.update()
  if (want.statusId !== was) control(recording, want.statusId === 'CrsRecording' ? 'resume' : 'pause')
  else control(recording, 'notice')
}

function lockSession(callSessionId) {
  return callSessionId ? find(SESSION).condition('callSessionId', callSessionId).forUpdate(true).one() : null
}

/**
 * Who is asking, and their leg: a guest by credential, or the signed-in user. A call they cannot see is "not found".
 * {session, userId, leg} or {refusal}.
 */
function caller(p, lock) {
  if (p.credential) {
    const guest = ec.service.sync().name('org.moqui.call.CallServices.get#GuestState').parameter('credential', p.credential)
      .disableAuthz().call() || {}
    if (guest.admission !== 'admitted' || !guest.callSessionId || (p.callSessionId && p.callSessionId !== guest.callSessionId)) {
      return { refusal: refuse('notFound', 'Call not found') }
    }
    const session = lock ? lockSession(guest.callSessionId) : one(SESSION, { callSessionId: guest.callSessionId })
    const leg = guest.callLegId ? one(LEG, { callLegId: guest.callLegId }) : null
    return { session, userId: null, leg: leg?.statusId === 'ClsAnswered' ? leg : null }
  }
  const me = ec.user.userId
  if (!me) return { refusal: refuse('notSignedIn', 'Sign in to use calls') }
  const session = lock ? lockSession(p.callSessionId) : (p.callSessionId ? one(SESSION, { callSessionId: p.callSessionId }) : null)
  if (!session || !canSee(session, me)) return { refusal: refuse('notFound', 'Call not found') }
  const participant = one(PARTICIPANT, { callSessionId: session.callSessionId, userId: me })
  const legs = participant ? rows(LEG, { callParticipantId: participant.callParticipantId, statusId: 'ClsAnswered' }) : []
  legs.sort((a, b) => (ms(b.answeredDate) || 0) - (ms(a.answeredDate) || 0))
  return { session, userId: me, leg: legs[0] || null }
}

// ---------- operations ----------

function state(p) {
  const pol = policy()
  const who = caller(p, false)
  if (who.refusal) return who.refusal
  const recording = activeRecording(who.session.callSessionId)
  const consent = recording && who.leg ? currentConsent(recording.callRecordingId, who.leg.callLegId) : null
  return {
    enabled: pol.enabled, ackRequired: pol.ackRequired,
    canRecord: pol.enabled && !!who.userId && who.session.statusId === 'CssActive' && isHost(who.session, who.userId) && !encrypted(who.session),
    encrypted: encrypted(who.session),
    recording: recordingView(recording),
    myConsent: consent ? { noticed: true, ackRequired: consent.ackRequired === 'Y', acknowledged: !!consent.ackDate,
      withdrawn: !!consent.withdrawnDate } : null,
  }
}

function start(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in to use calls')
  const session = lockSession(p.callSessionId)
  if (!session || !canSee(session, me)) return refuse('notFound', 'Call not found')
  // an end-to-end encrypted call (W8): the server cannot hear it, and recording never lowers its encryption; said first,
  // whatever the policy, so its people know why
  if (encrypted(session)) return refuse('encrypted', 'An end-to-end encrypted call cannot be recorded')
  const pol = policy()
  if (!pol.enabled) return refuse('recordingOff', 'Recording calls is off on this server')
  if (!isHost(session, me)) return refuse('notAllowed', 'Only the host records the call')
  if (session.statusId === 'CssEnded') return refuse('callEnded', 'The call has ended')
  if (session.statusId !== 'CssActive') return refuse('notInCall', 'The call has not started')
  if (!session.mediaNodeId) return refuse('mediaUnavailable', 'The call has no media server')
  const existing = activeRecording(session.callSessionId)
  if (existing) return { callRecordingId: existing.callRecordingId, recording: recordingView(existing), repeated: true }
  // the recording's files count toward the host's drive quota: a full drive records nothing, and the recorder stops where
  // the room left runs out (maxBytes)
  const left = quotaLeft(me)
  if (left != null && left <= 0) return refuse('quotaFull', 'Your drive is full; free space in My files to record the call')

  const now = ec.user.nowTimestamp
  const recording = ec.entity.makeValue(RECORDING)
  recording.setSequencedIdPrimary()
  recording.setFields({
    callSessionId: session.callSessionId, ownerUserId: me, kindEnumId: KINDS[p.kind] || 'CrkPerTrack',
    statusId: 'CrsRecording', segmentPeriod: 1, policyRevision: pol.revision, ackRequired: pol.ackRequired ? 'Y' : 'N',
    mediaNodeId: session.mediaNodeId, startedDate: now,
    retentionUntilDate: pol.retentionDays ? new Date(nowMs() + pol.retentionDays * 86400000) : null,
  }, true, null, null)
  recording.create()
  const mine = caller({ callSessionId: session.callSessionId }, false).leg
  for (const leg of currentLegs(session.callSessionId)) notice(recording, leg, mine?.callLegId === leg.callLegId)
  // a recording held at its start (an acknowledgement missing) has recorded nothing: its first resume is period 1
  const want = wanted(recording)
  if (want.statusId === 'CrsPaused') {
    recording.setFields({ statusId: 'CrsPaused', pauseReason: want.pauseReason, segmentPeriod: 0 }, true, null, null)
    recording.update()
  }
  control(recording, 'start', { paused: recording.statusId === 'CrsPaused', maxBytes: left != null ? Math.floor(left) : null })
  return { callRecordingId: recording.callRecordingId, recording: recordingView(recording) }
}

/** The recording a host acts on: by id, or the one going in the call; with the session locked. */
function hostRecording(p) {
  const me = ec.user.userId
  if (!me) return { refusal: refuse('notSignedIn', 'Sign in to use calls') }
  const byId = p.callRecordingId ? one(RECORDING, { callRecordingId: p.callRecordingId }) : null
  const callSessionId = byId ? byId.callSessionId : p.callSessionId
  const session = lockSession(callSessionId)
  if (!session || !canSee(session, me)) return { refusal: refuse('notFound', 'Recording not found') }
  if (!isHost(session, me) && byId?.ownerUserId !== me) return { refusal: refuse('notAllowed', 'Only the host records the call') }
  const recording = byId ? find(RECORDING).condition('callRecordingId', byId.callRecordingId).forUpdate(true).one()
    : activeRecording(session.callSessionId)
  if (!recording || !ACTIVE.includes(recording.statusId)) return { refusal: refuse('notRecording', 'The call is not being recorded') }
  return { session, recording }
}

function stop(p) {
  const got = hostRecording(p)
  if (got.refusal) return got.refusal
  stopRecording(got.recording, null)
  return { recording: recordingView(got.recording) }
}

function stopRecording(recording, errorText, failed) {
  recording.setFields({ statusId: failed ? 'CrsFailed' : 'CrsStopped', pauseReason: null, endedDate: ec.user.nowTimestamp,
    lastErrorText: errorText || recording.lastErrorText || null }, true, null, null)
  recording.update()
  const now = ec.user.nowTimestamp
  for (const track of find(TRACK).condition('callRecordingId', recording.callRecordingId).condition('endedDate', null).list()) {
    track.set('endedDate', now)
    track.update()
  }
  control(recording, failed ? 'notice' : 'stop')
}

function pause(p) {
  const got = hostRecording(p)
  if (got.refusal) return got.refusal
  const recording = got.recording
  if (recording.statusId === 'CrsPaused' && recording.pauseReason === 'user') return { recording: recordingView(recording) }
  const was = recording.statusId
  recording.setFields({ statusId: 'CrsPaused', pauseReason: 'user' }, true, null, null)
  recording.update()
  control(recording, was === 'CrsRecording' ? 'pause' : 'notice')
  return { recording: recordingView(recording) }
}

function resume(p) {
  const got = hostRecording(p)
  if (got.refusal) return got.refusal
  const recording = got.recording
  if (recording.pauseReason === 'user') {
    // no longer the host's pause; the policy may still hold it
    recording.set('pauseReason', 'resumed')
    settle(recording)
  }
  return { recording: recordingView(recording) }
}

/** The caller's leg, its consent row and the recording going, for ack and withdraw. */
function myNotice(p) {
  const who = caller(p, true)
  if (who.refusal) return who
  const recording = activeRecording(who.session.callSessionId)
  if (!recording) return { refusal: refuse('notRecording', 'The call is not being recorded') }
  if (!who.leg) return { refusal: refuse('notInCall', 'You are not in this call') }
  return { recording, consent: notice(recording, who.leg, false) }
}

function ack(p) {
  const got = myNotice(p)
  if (got.refusal) return got.refusal
  if (got.consent.ackDate && !got.consent.withdrawnDate) return { recording: recordingView(got.recording) }
  acknowledge(got.consent, 'CamClick')
  settle(got.recording)
  return { recording: recordingView(got.recording) }
}

function withdraw(p) {
  const got = myNotice(p)
  if (got.refusal) return got.refusal
  // the row keeps its notice and acknowledgement; a withdrawal already made stays as it was
  if (!got.consent.withdrawnDate) {
    got.consent.set('withdrawnDate', ec.user.nowTimestamp)
    got.consent.update()
  }
  settle(got.recording)
  return { recording: recordingView(got.recording) }
}

/**
 * The hook for legs that are not browsers: a phone leg hears the notice as a prompt, with the acknowledgement tone (press
 * 1) under that policy. Browser and guest legs have the rtc frame instead.
 */
function notifyLeg(p) {
  const leg = one(LEG, { callLegId: p.callLegId })
  if (!leg || WEB_LEGS.includes(leg.legTypeEnumId || 'CltWeb')) return { notified: false }
  const out = ec.service.sync().name('org.moqui.call.VoiceServices.notify#PhoneLeg').parameters({
    callLegId: leg.callLegId, notice: p.ackRequired === true || p.ackRequired === 'true' ? 'recordingAck' : 'recording',
  }).disableAuthz().call() || {}
  return { notified: !!out.notified }
}

/**
 * A phone leg pressed 1 (Voice.js, from its DTMF): it acknowledges the notice of the recording going in its call, logged
 * with the method dtmf, when that recording asks for acknowledgements and this leg has not given one. Any other time the
 * key is the call's own (a menu, a PIN), so nothing is recorded and nothing is refused.
 */
function legAck(p) {
  const session = lockSession(p.callSessionId)
  const leg = session && p.callLegId ? one(LEG, { callLegId: p.callLegId }) : null
  if (!leg || leg.callSessionId !== session.callSessionId || leg.statusId !== 'ClsAnswered'
    || WEB_LEGS.includes(leg.legTypeEnumId || 'CltWeb')) return { acknowledged: false }
  const recording = activeRecording(session.callSessionId)
  if (!recording || recording.ackRequired !== 'Y') return { acknowledged: false }
  const consent = notice(recording, leg, false)
  if (consent.ackDate && !consent.withdrawnDate) return { acknowledged: false, recording: recordingView(recording) }
  acknowledge(consent, 'CamDtmf')
  settle(recording)
  return { acknowledged: true, recording: recordingView(recording) }
}

/** A change of the call (Calls.js): tolerant, so the call's own change goes on whatever happens here. */
function callChange(p) {
  ec.message.pushErrors()
  try {
    const recording = activeRecording(p.callSessionId)
    if (!recording) return {}
    if (p.change === 'ended') {
      stopRecording(recording, null)
      return {}
    }
    const leg = p.callLegId ? one(LEG, { callLegId: p.callLegId }) : null
    if (p.change === 'legBound' && leg?.statusId === 'ClsAnswered') {
      notice(recording, leg, false)
    } else if (p.change === 'legLeft' && leg) {
      const now = ec.user.nowTimestamp
      for (const track of find(TRACK).condition({ callRecordingId: recording.callRecordingId, callLegId: leg.callLegId })
        .condition('endedDate', null).list()) {
        track.set('endedDate', now)
        track.update()
      }
    }
    settle(recording)
  } catch (err) {
    ec.logger.error(`Call recording of ${p.callSessionId} did not take the change ${p.change}: ${err && err.message ? err.message : err}`)
  } finally {
    if (ec.message.hasError()) ec.logger.error(`Call recording of ${p.callSessionId}, change ${p.change}: ${ec.message.errorsString}`)
    ec.message.clearErrors()
    ec.message.popErrors()
  }
  return {}
}

function extensionOf(contentType) {
  return contentType === 'video/webm' ? 'webm' : 'ogg'
}

function dropTemp(path) {
  try {
    ec.resource.getLocationReference('file:' + path).delete()
  } catch (err) {
    ec.logger.warn(`Call recording spool file ${path} not removed: ${err && err.message ? err.message : err}`)
  }
}

function storeSegment(p) {
  if (!p.tempPath && !p.fromLocation) return refuse('badRequest', 'A segment needs its spooled file or its received copy')
  const recording = one(RECORDING, { callRecordingId: p.callRecordingId })
  if (!recording || recording.statusId === 'CrsFailed') {
    if (p.tempPath) dropTemp(p.tempPath)
    return { stored: false }
  }
  const trackKindEnumId = TRACK_KINDS[p.trackKind] || 'CrtAudio'
  const leg = p.callLegId ? one(LEG, { callLegId: p.callLegId }) : null
  const fields = { callRecordingId: recording.callRecordingId, callLegId: leg ? leg.callLegId : null, trackKindEnumId,
    source: p.source || null }
  let track = Array.from(find(TRACK).condition(fields).limit(1).list())[0]
  if (!track) {
    track = ec.entity.makeValue(TRACK)
    track.setSequencedIdPrimary()
    track.setFields(Object.assign({}, fields, { callParticipantId: leg ? leg.callParticipantId : null, codec: p.codec || null,
      contentType: p.contentType || null, startedDate: p.fromDate || ec.user.nowTimestamp }), true, null, null)
    track.create()
  }
  const segment = ec.entity.makeValue(SEGMENT)
  segment.setSequencedIdPrimary()
  const contentLocation = `${ROOT}/${recording.callRecordingId}/${segment.callRecordingSegmentId}.${extensionOf(p.contentType)}`
  const moved = ec.service.sync().name('org.moqui.call.CallRecordingServices.store#RecordingFile')
    .parameters({ tempPath: p.tempPath || null, fromLocation: p.fromLocation || null, contentLocation }).disableAuthz().call()
  if (ec.message.hasError()) return { stored: false }
  segment.setFields({
    callRecordingId: recording.callRecordingId, callRecordingTrackId: track.callRecordingTrackId,
    callParticipantId: track.callParticipantId, callLegId: track.callLegId, segmentPeriod: p.segmentPeriod || 0,
    segmentSeq: p.segmentSeq || 0, fromDate: p.fromDate || null, thruDate: p.thruDate || null,
    startOffsetMs: p.startOffsetMs || 0, durationMs: p.durationMs || 0, contentLocation, contentType: p.contentType || null,
    byteSize: Number(moved?.byteSize || 0), processingStatusId: 'CrgStored',
  }, true, null, null)
  segment.create()
  if (p.trackEnded && !track.endedDate) {
    track.set('endedDate', p.thruDate || ec.user.nowTimestamp)
    track.update()
  }
  // the owner's drive quota, with this file: a recording that filled it stops (its recorder's maxBytes is the first stop;
  // this one holds when the drive's other files grew meanwhile). Else the recorder is told the room left now, so files
  // added to the drive since the start stop it within a segment
  if (ACTIVE.includes(recording.statusId)) {
    const left = quotaLeft(recording.ownerUserId)
    if (left != null && left <= 0) {
      const locked = find(RECORDING).condition('callRecordingId', recording.callRecordingId).forUpdate(true).one()
      if (locked && ACTIVE.includes(locked.statusId)) {
        stopRecording(locked, ec.l10n.localize('The recording stopped: the drive quota of its owner is full'), false)
        ec.logger.warn(`Call recording ${recording.callRecordingId} stopped: the drive quota of ${recording.ownerUserId} is full`)
      }
    } else if (left != null) {
      // the recorder counts this recording's bytes from its start: what it stored already, and the room left after it
      let stored = 0
      for (const row of find(SEGMENT).condition('callRecordingId', recording.callRecordingId).selectField('byteSize').list()) {
        stored += Number(row.byteSize || 0)
      }
      try {
        control(recording, 'limit', { maxBytes: stored + left })
      } catch (err) {
        ec.logger.warn(`Call recording ${recording.callRecordingId}: its recorder was not told the quota left: ${err && err.message ? err.message : err}`)
      }
    }
  }
  return { callRecordingSegmentId: segment.callRecordingSegmentId, contentLocation, stored: true }
}

function recorderEnd(p) {
  const recording = find(RECORDING).condition('callRecordingId', p.callRecordingId).forUpdate(true).one()
  if (!recording || !ACTIVE.includes(recording.statusId)) {
    // stopped already (the call ended): keep what it says, add the error
    if (recording && p.outcome === 'failed' && !recording.lastErrorText) {
      recording.setFields({ lastErrorText: String(p.errorText || 'The recorder failed').slice(0, 4000) }, true, null, null)
      recording.update()
    }
    return {}
  }
  const full = p.outcome === 'limit' && (quotaLeft(recording.ownerUserId) ?? 1) <= 0
  const text = full ? ec.l10n.localize('The recording stopped: the drive quota of its owner is full')
    : p.outcome === 'limit' ? ec.l10n.localize('The recording reached its size limit')
    : String(p.errorText || ec.l10n.localize('The recorder failed')).slice(0, 4000)
  stopRecording(recording, text, p.outcome !== 'limit')
  ec.logger.warn(`Call recording ${p.callRecordingId} ended by its recorder (${p.outcome}): ${text}`)
  return {}
}

/**
 * The edge link's callRecording items (plans/Edge-Node.md §5.4): a segment or the end of a recording whose media node is
 * this node's edge node, stored here as this node's own recorder's are. Only for the queue, which runs it as _edge_.
 */
function receiveEdge(p) {
  if (ec.user.username !== '_edge_') return refuse('notAllowed', 'Only the edge link queue stores an edge recording')
  let item
  try {
    item = JSON.parse(p.payload || '{}')
  } catch (err) {
    return refuse('badRequest', 'The edge recording item is not JSON')
  }
  // only a recording of a call whose media is on this node's edge node (plans/Edge-Node.md §5.4)
  const recordingId = item.op === 'segment' ? item.parameters?.callRecordingId : item.callRecordingId
  const recording = recordingId ? one(RECORDING, { callRecordingId: recordingId }) : null
  const session = recording ? one(SESSION, { callSessionId: recording.callSessionId }) : null
  const node = session?.mediaNodeId ? one('moqui.call.CallMediaNode', { instanceId: session.mediaNodeId }) : null
  if (recording && (!node || node.edgeLinked !== 'Y')) {
    return refuse('notAllowed', 'The edge node stores only recordings of calls whose media is on it')
  }
  if (item.op === 'segment') {
    if (!p.contentLocation) return refuse('badRequest', 'The edge recording segment came without its file')
    const parameters = Object.assign({}, item.parameters || {}, { tempPath: null, fromLocation: p.contentLocation })
    ec.service.sync().name('org.moqui.call.CallRecordingServices.store#RecordingSegment').parameters(parameters).disableAuthz().call()
    return {}
  }
  if (item.op === 'end') {
    ec.service.sync().name('org.moqui.call.CallRecordingServices.note#RecorderEnd').parameters({
      callRecordingId: item.callRecordingId, outcome: item.outcome, errorText: item.errorText || null,
    }).disableAuthz().call()
    return {}
  }
  return refuse('badRequest', 'Unknown edge recording item ' + item.op)
}

function segmentName(segment, track, names) {
  if (!track || track.trackKindEnumId === 'CrtMix') return ec.l10n.localize('Everyone (mixed)')
  const who = names[track.callParticipantId] || ec.l10n.localize('Someone')
  return track.source && track.source !== 'microphone' && track.source !== 'camera' ? `${who} · ${track.source}` : who
}

function find_(p) {
  const me = ec.user.userId
  if (!me) return { recordingList: [], recordingListCount: 0 }
  // the calls the user hosts, by the rule of isHost: an admitted host, or the owner of the call's link
  const hosted = Array.from(find(PARTICIPANT).condition({ userId: me, roleEnumId: 'CprHost', admissionStatusId: 'CasAdmitted' }).selectField('callSessionId')
    .orderBy('-callParticipantId').limit(500).list()).map((x) => x.callSessionId)
  const linked = Array.from(find(LINK).condition('ownerUserId', me).selectField('callLinkId').limit(200).list()).map((l) => l.callLinkId)
  if (linked.length) {
    for (const s of find(SESSION).condition('callLinkId', 'in', linked).selectField('callSessionId').limit(500).list()) hosted.push(s.callSessionId)
  }
  const byId = new Map()
  const add = (list) => { for (const r of list) byId.set(r.callRecordingId, r) }
  const scoped = (q) => (p.callSessionId ? q.condition('callSessionId', p.callSessionId) : q)
  add(scoped(find(RECORDING).condition('ownerUserId', me)).orderBy('-startedDate').limit(500).list())
  if (hosted.length) add(scoped(find(RECORDING).condition('callSessionId', 'in', Array.from(new Set(hosted)))).orderBy('-startedDate').limit(500).list())
  const all = Array.from(byId.values()).sort((a, b) => (ms(b.startedDate) || 0) - (ms(a.startedDate) || 0))
  const size = Math.min(Math.max(Number(p.pageSize) || 20, 1), 100)
  const page = all.slice(Math.max(Number(p.pageIndex) || 0, 0) * size).slice(0, size)
  const ids = page.map((r) => r.callRecordingId)
  const tracks = ids.length ? Array.from(find(TRACK).condition('callRecordingId', 'in', ids).list()) : []
  const segments = ids.length ? Array.from(find(SEGMENT).condition('callRecordingId', 'in', ids).condition('processingStatusId', 'CrgStored')
    .orderBy('segmentPeriod').orderBy('segmentSeq').list()) : []
  const sessionIds = Array.from(new Set(page.map((r) => r.callSessionId)))
  const participants = sessionIds.length ? Array.from(find(PARTICIPANT).condition('callSessionId', 'in', sessionIds).list()) : []
  const userNamesById = userNames(participants.map((x) => x.userId).concat(page.map((r) => r.ownerUserId)))
  const names = {}
  for (const x of participants) names[x.callParticipantId] = x.displayName || userNamesById[x.userId] || x.userId
  const trackById = new Map(tracks.map((tr) => [tr.callRecordingTrackId, tr]))
  const recordingList = page.map((r) => {
    const periods = new Map()
    let durationMs = 0
    for (const s of segments.filter((x) => x.callRecordingId === r.callRecordingId)) {
      const track = trackById.get(s.callRecordingTrackId)
      const period = Number(s.segmentPeriod || 0)
      if (!periods.has(period)) periods.set(period, [])
      periods.get(period).push({
        callRecordingSegmentId: s.callRecordingSegmentId, trackKind: TRACK_CODES[track?.trackKindEnumId] || 'audio',
        source: track?.source || null, callLegId: s.callLegId || null, name: segmentName(s, track, names),
        contentType: s.contentType, startOffsetMs: Number(s.startOffsetMs || 0), durationMs: Number(s.durationMs || 0),
        segmentSeq: Number(s.segmentSeq || 0), byteSize: Number(s.byteSize || 0),
      })
      if (track?.trackKindEnumId === 'CrtMix') durationMs += Number(s.durationMs || 0)
    }
    const people = participants.filter((x) => x.callSessionId === r.callSessionId).map((x) => names[x.callParticipantId])
    return Object.assign(recordingView(r), {
      ownerUserId: r.ownerUserId, ownerName: userNamesById[r.ownerUserId] || r.ownerUserId, people, durationMs,
      canDelete: r.ownerUserId === me && !ACTIVE.includes(r.statusId),
      periods: Array.from(periods.entries()).sort((a, b) => a[0] - b[0]).map(([period, list]) => ({ period, segments: list })),
    })
  })
  return { recordingList, recordingListCount: all.length }
}

function file(p) {
  const me = ec.user.userId
  const segment = p.callRecordingSegmentId ? one(SEGMENT, { callRecordingSegmentId: p.callRecordingSegmentId }) : null
  const recording = segment ? one(RECORDING, { callRecordingId: segment.callRecordingId }) : null
  if (!segment || segment.processingStatusId !== 'CrgStored' || !segment.contentLocation || !canRead(recording, me)) {
    return refuse('notFound', 'Recording not found')
  }
  const track = one(TRACK, { callRecordingTrackId: segment.callRecordingTrackId })
  const kind = TRACK_CODES[track?.trackKindEnumId] || 'audio'
  return {
    location: segment.contentLocation, contentType: segment.contentType || 'application/octet-stream',
    fileName: `call-recording-${recording.callRecordingId}-${kind}${track?.source ? '-' + track.source : ''}-` +
      `${segment.segmentPeriod || 0}-${segment.segmentSeq || 0}.${extensionOf(segment.contentType)}`,
    byteSize: Number(segment.byteSize || 0),
  }
}

/** Deletes a recording's files and rows, its transcripts too; a file that does not go is logged, and the rows go anyway. */
function deleteRecording(recording) {
  for (const segment of rows(SEGMENT, { callRecordingId: recording.callRecordingId })) {
    if (segment.contentLocation) {
      try {
        ec.resource.getLocationReference(segment.contentLocation).delete()
      } catch (err) {
        ec.logger.error(`Call recording file ${segment.contentLocation} not deleted: ${err && err.message ? err.message : err}`)
      }
    }
  }
  try {
    ec.resource.getLocationReference(`${ROOT}/${recording.callRecordingId}`).delete()
  } catch (err) {
    ec.logger.debug(`Call recording folder ${recording.callRecordingId} not removed: ${err && err.message ? err.message : err}`)
  }
  for (const entity of [SEGMENT, TRACK, CONSENT_HISTORY, CONSENT, TRANSCRIPT]) find(entity).condition('callRecordingId', recording.callRecordingId).deleteAll()
  recording.delete()
}

function remove(p) {
  const me = ec.user.userId
  const recording = p.callRecordingId ? one(RECORDING, { callRecordingId: p.callRecordingId }) : null
  if (!recording || !canRead(recording, me)) return refuse('notFound', 'Recording not found')
  if (recording.ownerUserId !== me) return refuse('notAllowed', 'Only the one who recorded it deletes a recording')
  if (ACTIVE.includes(recording.statusId)) return refuse('notAllowed', 'Stop the recording before deleting it')
  deleteRecording(recording)
  return { deleted: true }
}

function eachSafely(label, items) {
  let done = 0
  for (const item of items) {
    ec.message.pushErrors()
    try {
      const out = ec.service.sync().name('org.moqui.call.CallRecordingServices.expire#RecordingItem')
        .parameters(Object.assign({ itemKind: label }, item)).requireNewTransaction(true).disableAuthz().call()
      if (ec.message.hasError()) ec.logger.error(`Call recording cleanup ${label} ${JSON.stringify(item)} failed: ${ec.message.errorsString}`)
      else if (out?.done) done++
    } catch (err) {
      ec.logger.error(`Call recording cleanup ${label} ${JSON.stringify(item)} failed: ${err && err.message ? err.message : err}`)
    } finally {
      ec.message.clearErrors()
      ec.message.popErrors()
    }
  }
  return done
}

function expire() {
  const now = nowMs()
  const retention = Array.from(find(RECORDING).condition('retentionUntilDate', '<', new Date(now))
    .condition('statusId', 'not-in', ACTIVE).selectField('callRecordingId').limit(500).list())
    .map((r) => ({ callRecordingId: r.callRecordingId }))
  // a node that started after the recording did has no recorder for it (a restart clears the spool too)
  const live = new Map(Array.from(find(MEDIA_NODE).condition('heartbeatDate', '>=', new Date(now - MEDIA_NODE_STALE_MS)).list())
    .map((n) => [n.instanceId, ms(n.startedDate) || 0]))
  const stale = []
  for (const r of find(RECORDING).condition('statusId', 'in', ACTIVE).limit(1000).list()) {
    const session = one(SESSION, { callSessionId: r.callSessionId })
    const restarted = (live.get(r.mediaNodeId) || 0) > (ms(r.startedDate) || 0)
    if (!session || session.statusId === 'CssEnded' || !live.has(r.mediaNodeId) || restarted) stale.push({ callRecordingId: r.callRecordingId })
  }
  const failed = Array.from(find(SEGMENT).condition('processingStatusId', 'CrgFailed').selectField('callRecordingSegmentId')
    .limit(500).list()).map((s) => ({ callRecordingSegmentId: s.callRecordingSegmentId }))
  const orphans = []
  const root = ec.resource.getLocationReference(ROOT)
  if (root.getExists() && root.isDirectory()) {
    for (const dir of root.getDirectoryEntries().slice(0, 500)) orphans.push({ location: dir.getLocation() })
  }
  return {
    deletedCount: eachSafely('retention', retention),
    recoveredCount: eachSafely('stale', stale) + eachSafely('failedSegment', failed) + eachSafely('orphanFiles', orphans),
  }
}

function expireItem(p) {
  if (p.itemKind === 'retention') {
    const recording = one(RECORDING, { callRecordingId: p.callRecordingId })
    if (!recording || ACTIVE.includes(recording.statusId) || ms(recording.retentionUntilDate) >= nowMs()) return { done: false }
    deleteRecording(recording)
    return { done: true }
  }
  if (p.itemKind === 'stale') {
    const recording = find(RECORDING).condition('callRecordingId', p.callRecordingId).forUpdate(true).one()
    if (!recording || !ACTIVE.includes(recording.statusId)) return { done: false }
    stopRecording(recording, ec.l10n.localize('The recorder stopped: the call ended or its media server stopped or restarted'), true)
    return { done: true }
  }
  if (p.itemKind === 'failedSegment') {
    const segment = one(SEGMENT, { callRecordingSegmentId: p.callRecordingSegmentId })
    if (!segment || segment.processingStatusId !== 'CrgFailed') return { done: false }
    if (segment.contentLocation) ec.resource.getLocationReference(segment.contentLocation).delete()
    segment.delete()
    return { done: true }
  }
  if (p.itemKind === 'orphanFiles') {
    // a folder per recording: one with no recording, or files no segment names in one that ended a while ago
    const location = String(p.location || '')
    if (!location.startsWith(ROOT + '/')) return { done: false }
    const id = location.slice(ROOT.length + 1).split('/')[0]
    const recording = one(RECORDING, { callRecordingId: id })
    if (recording && (ACTIVE.includes(recording.statusId) || (ms(recording.endedDate) || nowMs()) > nowMs() - ORPHAN_AGE_MS)) return { done: false }
    const named = new Set(recording ? rows(SEGMENT, { callRecordingId: id }).map((s) => s.contentLocation) : [])
    let removed = 0
    for (const entry of ec.resource.getLocationReference(location).getDirectoryEntries()) {
      if (!named.has(entry.getLocation())) {
        entry.delete()
        removed++
      }
    }
    if (!recording) ec.resource.getLocationReference(location).delete()
    return { done: removed > 0 || !recording }
  }
  throw new Error('Unknown call recording cleanup item ' + p.itemKind)
}

const operations = {
  state, start, stop, pause, resume, ack, withdraw, notifyLeg, legAck, callChange, storeSegment, recorderEnd, receiveEdge, find: find_, file, remove,
  expire, expireItem, recordingBytes: (p) => ({ recordingBytes: recordingBytes(p.userId) }),
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown call recording operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
