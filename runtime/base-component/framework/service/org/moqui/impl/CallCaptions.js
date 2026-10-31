// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Live captions of a call (plans/Comms-Framework.md §5.4, §6.4, W6): the policy of org.moqui.impl.CallCaptionServices.
// Someone in the call turns captions on or off; the call's media node transcribes (moqui-host rtc/captions.rs), told by
// send#MediaControl after the commit, and tells every leg that captions are on and which provider hears the audio (the
// notice). The checks, in order: the deployment allows captions (call_captions_enabled), the user is in the call (anyone
// else gets "not found"), the call has not ended, its media is not end-to-end encrypted (the server cannot hear it, and
// captions never downgrade it), and a speech provider is set up. CallCaptions keeps the state; nothing of the audio is
// kept, and the text only when call_captions_store is true. A phone in the call hears the notice as a prompt instead.

const SESSION = 'moqui.call.CallSession'
const PARTICIPANT = 'moqui.call.CallParticipant'
const LEG = 'moqui.call.CallLeg'
const CAPTIONS = 'moqui.call.CallCaptions'

/** The media protection captions work with: hop-by-hop DTLS-SRTP through the SFU. Anything else is end to end. */
const SERVER_READABLE = ['', 'dtls-srtp']

const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()

function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

/** Whether this deployment allows captions at all. */
function enabled() {
  return String(System.getProperty('call_captions_enabled') || '').trim() === 'true'
}

/** Whether the call's media can be heard by the server: not end-to-end encrypted. */
function serverReadable(session) {
  return SERVER_READABLE.includes(String(session.encryptionMode || '').trim())
}

/** The user's participation in the call when they are in it now (admitted, with an answered leg). */
function inCall(session, userId) {
  if (!userId) return null
  const participant = one(PARTICIPANT, { callSessionId: session.callSessionId, userId })
  if (!participant || ['CasRevoked', 'CasDenied', 'CasWaiting'].includes(participant.admissionStatusId)) return null
  const answered = find(LEG).condition({ callParticipantId: participant.callParticipantId, statusId: 'ClsAnswered' }).count()
  return answered > 0 ? participant : null
}

function state(session) {
  const row = session ? one(CAPTIONS, { callSessionId: session.callSessionId }) : null
  const on = !!row && row.captionsOn === 'Y' && session.statusId !== 'CssEnded'
  return {
    on,
    providerLabel: on ? row.providerLabel || null : null,
    local: on ? row.providerLocal === 'Y' : false,
    startedByUserId: on ? row.startedByUserId || null : null,
  }
}

/** The checks shared by start and stop; a refusal, or the locked session. */
function check(callSessionId, forStart) {
  if (!enabled()) return { refusal: refuse('captionsOff', 'Captions are turned off on this server') }
  const userId = ec.user.userId
  if (!userId) return { refusal: refuse('notSignedIn', 'Sign in to use captions') }
  const session = callSessionId ? find(SESSION).condition('callSessionId', callSessionId).forUpdate(true).one() : null
  if (!session || !inCall(session, userId)) return { refusal: refuse('notFound', 'Call not found') }
  if (session.statusId === 'CssEnded') return { refusal: refuse('callEnded', 'The call has ended') }
  if (forStart && !serverReadable(session)) {
    return { refusal: refuse('encrypted', 'This call is end-to-end encrypted, so it cannot have captions') }
  }
  if (forStart && !session.mediaNodeId) return { refusal: refuse('mediaUnavailable', 'The call has no media server') }
  return { session, userId }
}

function provider() {
  return ec.service.sync().name('org.moqui.impl.SpeechServices.get#SpeechProvider').disableAuthz().call() || {}
}

function control(session, message) {
  ec.service.sync().name('org.moqui.impl.CallServices.send#MediaControl').parameters({
    mediaNodeId: session.mediaNodeId, message: Object.assign({ callSessionId: session.callSessionId }, message),
  }).disableAuthz().call()
}

/** Tells everyone in the call, on chatUser:<userId> after the commit: their tabs show the notice or take it down. */
function pushState(session, captions) {
  const userIds = Array.from(find(PARTICIPANT).condition('callSessionId', session.callSessionId).list())
    .map((p) => p.userId).filter((id) => id)
  for (const userId of new Set(userIds)) {
    ec.service.sync().name('org.moqui.impl.ChatServices.push#Signal').parameters({
      topic: 'chatUser:' + userId, userIdList: [userId],
      message: { kind: 'call', action: 'captions', callSessionId: session.callSessionId, captions },
    }).disableAuthz().call()
  }
}

/**
 * The phones in the call hear the captions notice as a prompt (VoiceServices.notify#PhoneLeg), as browsers get the rtc
 * frame; a phone that answers later hears it from Calls.js.
 */
function noticePhones(session) {
  const legs = find(LEG).condition({ callSessionId: session.callSessionId, statusId: 'ClsAnswered' })
    .condition('legTypeEnumId', 'in', ['CltPstn', 'CltSipDevice']).list()
  for (const leg of legs) {
    ec.service.sync().name('org.moqui.impl.VoiceServices.notify#PhoneLeg')
      .parameters({ callLegId: leg.callLegId, notice: 'captions' }).disableAuthz().call()
  }
}

function start({ callSessionId }) {
  const checked = check(callSessionId, true)
  if (checked.refusal) return checked.refusal
  const { session, userId } = checked
  const speech = provider()
  if (!speech.available) {
    return refuse('noProvider', 'Captions need a speech to text provider: ' + (speech.errorText || 'none is set up'))
  }
  let row = one(CAPTIONS, { callSessionId })
  const repeated = !!row && row.captionsOn === 'Y'
  const isNew = !row
  if (!row) {
    row = ec.entity.makeValue(CAPTIONS)
    row.set('callSessionId', callSessionId)
  }
  if (!repeated) {
    row.setFields({ captionsOn: 'Y', startedByUserId: userId, startedDate: ec.user.nowTimestamp,
      stoppedByUserId: null, stoppedDate: null }, true, null, null)
  }
  row.setFields({ providerLabel: speech.providerLabel || null, providerLocal: speech.local ? 'Y' : 'N' }, true, null, null)
  if (!isNew) row.update()
  else row.create()
  // the media node starts (or, repeated, sends the notice again); its quota is the user's who turned them on
  control(session, { action: 'startCaptions', userId: row.startedByUserId || userId, provider: speech.providerLabel || '',
    local: !!speech.local })
  const captions = state(session)
  if (!repeated) {
    pushState(session, captions)
    noticePhones(session)
  }
  return { captions, repeated }
}

function stop({ callSessionId }) {
  const checked = check(callSessionId, false)
  if (checked.refusal) return checked.refusal
  const { session, userId } = checked
  const row = one(CAPTIONS, { callSessionId })
  if (!row || row.captionsOn !== 'Y') return { captions: state(session), repeated: true }
  row.setFields({ captionsOn: 'N', stoppedByUserId: userId, stoppedDate: ec.user.nowTimestamp }, true, null, null)
  row.update()
  if (session.mediaNodeId) control(session, { action: 'stopCaptions' })
  const captions = state(session)
  pushState(session, captions)
  return { captions, repeated: false }
}

/** What the user's call view shows: whether the CC button is there (allowed), and whether captions are on. */
function getState({ callSessionId }) {
  const userId = ec.user.userId
  const session = callSessionId ? one(SESSION, { callSessionId }) : null
  if (!session || !inCall(session, userId)) return { captions: { allowed: false, on: false }, reasonCode: 'notFound' }
  const current = state(session)
  const reasonCode = !enabled() ? 'captionsOff' : !serverReadable(session) ? 'encrypted'
    : session.statusId === 'CssEnded' ? 'callEnded' : null
  return { captions: Object.assign({ allowed: !reasonCode }, current), reasonCode }
}

const operations = { start, stop, getState }

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown caption operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
