// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Voicemail (plans/Comms-Framework.md §5.3, §5.4, V2; docs/interfaces/Voicemail.md): the services of
// org.moqui.call.VoicemailServices. A call to a user's number goes to their box when nobody answers within the box's rings
// (or call_ring_seconds), when the user is in another call, or when they decline it. The SIP engine of the call's node
// (moqui-host src/sip/voicemail.rs) answers it with no one else in it, plays the greeting and a beep, records the caller and
// hands the file to store#Voicemail; it asks begin#Voicemail first, which moves the call there (Calls.js phoneToVoicemail:
// the user missed it). A caller who presses * during the greeting, or calls the access number (PnrVoicemail), gives a PIN
// (check#VoicemailPin, rate limited per box and per source) and hears the new messages (act#PhoneVoicemail: 7 delete,
// 9 save, # next). After a message is stored its owner hears of it (a chatUser push, Web Push, an email when they asked),
// and a job transcribes it when voicemail_transcribe is on (SpeechServices.transcribe#Audio, a CallTranscript). Only the
// box's owner lists, plays, marks, deletes and empties; a box of a ring group (V3) will be its members'. The audio is read
// only through get#VoicemailAudio; trash is emptied after voicemail_trash_days.

const BOX = 'moqui.call.VoicemailBox'
const VOICEMAIL = 'moqui.call.Voicemail'
const TRANSCRIPT = 'moqui.call.CallTranscript'
const PIN_FAILURE = 'moqui.call.VoicemailPinFailure'
const SESSION = 'moqui.call.CallSession'
const PARTICIPANT = 'moqui.call.CallParticipant'
const NUMBER = 'moqui.call.PhoneNumber'
const CALLS = 'component://framework/service/org/moqui/call/Calls.js'
const CALL_HANDLING = 'component://framework/service/org/moqui/call/CallHandling.js'
const VOICE = 'component://framework/service/org/moqui/call/Voice.js'

/** Where the audio goes; store#VoicemailFile refuses any other place. */
const ROOT = 'dbresource://moqui/call/voicemail'
/** A ring, for a box's rings. */
const RING_SECONDS = 6
const MAX_RINGS = 10
/** How long past its voicemail an inbound call may ring before expire#Calls ends it (the engine's timer comes first). */
const RING_GRACE_SECONDS = 30
/** A PIN that may be checked: 4 to 12 digits (a PIN set before voicemail_pin_min_digits was raised still works). */
const PIN_PATTERN = /^[0-9]{4,12}$/
/** The fewest digits a new PIN has (voicemail_pin_min_digits, 4 to 12): six by default, as four fall to the box's limit in weeks. */
const pinMinDigits = () => clamp(Math.round(Number(property('voicemail_pin_min_digits', '6'))) || 6, 4, 12)
/** Failed PINs over the phone: a box takes BOX_FAILURES in BOX_WINDOW_MS, then refuses every PIN for BOX_BLOCK_MS. */
const BOX_FAILURES = 5
const BOX_WINDOW_MS = 15 * 60000
const BOX_BLOCK_MS = 15 * 60000
/** A source (the caller's number) takes SOURCE_FAILURES in SOURCE_WINDOW_MS, then is refused for SOURCE_BLOCK_MS. */
const SOURCE_FAILURES = 10
const SOURCE_WINDOW_MS = 60 * 60000
const SOURCE_BLOCK_MS = 60 * 60000
/** The biggest greeting taken, before it is converted. */
const MAX_GREETING_BYTES = 10 * 1024 * 1024
const PAGE_MAX = 100
const STATUS_CODES = { VmsStored: 'stored', VmsAudioFailed: 'audioFailed' }
const TRANSCRIPT_CODES = { CtsPending: 'pending', CtsDone: 'done', CtsFailed: 'failed' }

const ms = (value) => {
  if (value == null || value === '') return null
  const number = Number(value)
  return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}
const nowMs = () => ms(ec.user.nowTimestamp)
const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const rows = (entity, fields) => Array.from(find(entity).condition(fields || {}).list())
const yes = (value) => value === true || value === 'Y' || value === 'true'
const blank = (value) => value == null || String(value).trim() === ''
const property = (name, fallback) => {
  const value = System.getProperty(name)
  return value == null || String(value).trim() === '' ? fallback : String(value).trim()
}
const clamp = (value, low, high) => Math.min(Math.max(value, low), high)
const maxSeconds = () => clamp(Number(property('voicemail_max_seconds', '120')) || 120, 10, 600)
const trashDays = () => Math.max(Number(property('voicemail_trash_days', '30')) || 0, 0)
const transcribeOn = () => property('voicemail_transcribe', 'false') === 'true'
const callRingSeconds = () => Math.max(10, Number(property('call_ring_seconds', '45')) || 45)

/** A refusal: an error for the user and a reason code for the caller. */
function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

/** The edge link's delegate user (plans/Edge-Node.md §5.4 "Voice (SIP)", E8). */
const isEdgeCaller = () => ec.user.username === '_edge_'

/** Anyone, or the edge node's delegate user for a call whose media is on the edge node (Calls.js edgeAllowed). */
function edgeAllowed(p) {
  return !isEdgeCaller() || !!(ec.resource.script(CALLS).execute('edgeAllowed', p) || {}).allowed
}

/** On a paired node, the numbers' fallbacks again, as a box's greeting changed (Voice.js); a failure is logged. */
function fallbacksChanged() {
  ec.resource.script(VOICE).execute('fallbackChanged', {})
}

function appRoot() {
  return property('comms_app_root', '/qapps/BasicComms').replace(/\/+$/, '')
}

/** The site's own address for a link in an email, from the webapp properties; empty gives a path alone. */
function siteUrl() {
  const host = property('webapp_http_host', '')
  if (!host) return ''
  const https = property('webapp_https_port', '')
  if (https) return 'https://' + host + (https === '443' ? '' : ':' + https)
  const http = property('webapp_http_port', '')
  return 'http://' + host + (!http || http === '80' ? '' : ':' + http)
}

function voicemailPath(voicemailId) {
  return appRoot() + '/Calls/Voicemail?voicemailId=' + encodeURIComponent(voicemailId)
}

function userName(userId) {
  const account = userId ? one('moqui.security.UserAccount', { userId }) : null
  return account ? account.userFullName || account.username || userId : userId
}

/** The user's box, made with the defaults when create is true and they have none. */
function boxOf(userId, create) {
  if (!userId) return null
  const box = find(BOX).condition('userId', userId).one()
  if (box || !create) return box
  const made = ec.entity.makeValue(BOX)
  made.setSequencedIdPrimary()
  made.setFields({ userId, attachAudio: 'N', attachTranscript: 'N' }, true, null, null)
  made.create()
  return made
}

/** The ring groups a user is a member of (V3): their boxes are the user's to read too. */
function groupsOf(userId) {
  return userId ? rows('moqui.call.RingGroupMember', { userId }).map((m) => m.ringGroupId) : []
}

/** The boxes the user reads: their own, and their ring groups' (V3), whose messages and heard state they share. */
function readableBoxes(userId) {
  const out = []
  const own = boxOf(userId, false)
  if (own) out.push(own)
  const groups = groupsOf(userId)
  if (groups.length) for (const box of find(BOX).condition('ringGroupId', 'in', groups).list()) out.push(box)
  return out
}

/** The box the current user may read a voicemail of (their own, or a ring group's they are in), or null. */
function readableBox(voicemail) {
  const me = ec.user.userId
  if (!me || !voicemail) return null
  const box = one(BOX, { voicemailBoxId: voicemail.voicemailBoxId })
  if (!box) return null
  if (box.userId) return box.userId === me ? box : null
  return box.ringGroupId && groupsOf(me).includes(box.ringGroupId) ? box : null
}

/** A ring group's name, for its box's messages. */
function groupName(ringGroupId) {
  return ringGroupId ? one('moqui.call.RingGroup', { ringGroupId })?.name || ringGroupId : null
}

/** A voicemail the current user may act on; the same "not found" for one that is not there and one of someone else. */
function myVoicemail(voicemailId) {
  const voicemail = voicemailId ? one(VOICEMAIL, { voicemailId }) : null
  const box = readableBox(voicemail)
  return box ? { voicemail, box } : null
}

function latestTranscript(voicemailId) {
  return Array.from(find(TRANSCRIPT).condition('voicemailId', voicemailId).orderBy('-createdDate').limit(1).list())[0] || null
}

function view(voicemail, withText) {
  const transcript = latestTranscript(voicemail.voicemailId)
  const out = {
    voicemailId: voicemail.voicemailId,
    callSessionId: voicemail.callSessionId || null,
    fromNumber: voicemail.fromNumber || null,
    fromName: voicemail.fromName || voicemail.fromNumber || ec.l10n.localize('Unknown number'),
    receivedDate: ms(voicemail.receivedDate),
    durationMs: Number(voicemail.durationMs || 0),
    heard: !!voicemail.heardDate,
    folder: voicemail.folder || 'inbox',
    status: STATUS_CODES[voicemail.statusId] || 'stored',
    hasAudio: !!voicemail.audioLocation,
    lastErrorText: voicemail.lastErrorText || null,
    transcriptStatus: transcript ? TRANSCRIPT_CODES[transcript.statusId] || null : null,
    transcriptSnippet: transcript && transcript.text ? String(transcript.text).slice(0, 140) : null,
  }
  if (withText) {
    out.transcriptText = transcript ? transcript.text || null : null
    out.transcriptError = transcript ? transcript.lastErrorText || null : null
    out.canRetryTranscript = transcribeOn() && !!voicemail.audioLocation && (!transcript || transcript.statusId === 'CtsFailed')
    out.transcribeOn = transcribeOn()
  }
  return out
}

// ---------- routing (Voice.js receive#PhoneCall) and the SIP engine ----------

/** Whether a call to a user goes to voicemail at once (they are in a call), and after how long it does when unanswered. */
function routing(p) {
  const status = p.userId ? one('moqui.security.UserStatus', { userId: p.userId }) : null
  const box = boxOf(p.userId, false)
  const rings = box && box.rings ? clamp(Number(box.rings), 1, MAX_RINGS) : 0
  const voicemailSeconds = rings ? rings * RING_SECONDS : callRingSeconds()
  return { busy: !!status && status.inCall === 'Y', voicemailSeconds, ringExpireSeconds: voicemailSeconds + RING_GRACE_SECONDS }
}

/**
 * Internal (the SIP engine): a call goes to voicemail: reason noAnswer, busy, declined, or access (the access number).
 * The call moves there (the users rung missed it) and the box is the one of the user it rang, made when they have none.
 */
function begin(p) {
  const session = p.callSessionId ? one(SESSION, { callSessionId: p.callSessionId }) : null
  if (!session) return refuse('notFound', 'Call not found')
  if (!edgeAllowed({ callSessionId: session.callSessionId, callLegId: p.callLegId })) {
    return refuse('notAllowed', 'The edge node sends to voicemail only calls whose media is on it')
  }
  const local = rows(PARTICIPANT, { callSessionId: session.callSessionId }).filter((x) => x.userId)
  if (p.reason === 'access') {
    if (local.length || session.statusId === 'CssEnded') return refuse('notFound', 'Call not found')
    return { mode: 'access', maxSeconds: maxSeconds() }
  }
  // V3: the call's handling plan says where it goes next: another step that rings (the engine rings on), a box, or the end
  const plan = ec.resource.script(CALL_HANDLING).execute('advance', { callSessionId: p.callSessionId, callLegId: p.callLegId,
    reason: p.reason, stepToken: p.stepToken }) || {}
  if (plan.handled && plan.reasonCode) return refuse(plan.reasonCode, plan.reasonCode === 'noRoute' ? 'Nobody takes this call'
    : plan.reasonCode === 'stale' ? 'The call has moved on from that step' : 'This call cannot go to voicemail now')
  if (plan.handled && plan.mode === 'ring') return { mode: 'ring', ringSeconds: plan.ringSeconds || null, stepToken: plan.stepToken || null }
  const moved = ec.resource.script(CALLS).execute('phoneToVoicemail', { callSessionId: p.callSessionId, callLegId: p.callLegId }) || {}
  if (moved.reasonCode) return refuse(moved.reasonCode, 'This call cannot go to voicemail now')
  const userId = Array.from(moved.userIdList || [])[0]
  const box = plan.handled ? one(BOX, { voicemailBoxId: plan.voicemailBoxId }) : boxOf(userId, true)
  if (!box) return refuse('noBox', 'This call has no voicemail box')
  const number = moved.phoneNumber || null
  const name = moved.displayName && moved.displayName !== number ? String(moved.displayName).replace(/ \(\+?[0-9]+\)$/, '') : null
  // the box a message the edge node's engine records is stored in (receiveEdge, E8)
  const moving = find(SESSION).condition('callSessionId', session.callSessionId).forUpdate(true).one()
  if (moving && moving.voicemailBoxId !== box.voicemailBoxId) {
    moving.set('voicemailBoxId', box.voicemailBoxId)
    moving.update()
  }
  return { mode: 'deposit', voicemailBoxId: box.voicemailBoxId, greetingLocation: box.greetingLocation || null,
    maxSeconds: maxSeconds(), fromNumber: number, fromName: name, hasPin: !blank(box.pinHash) }
}

/**
 * Internal (the SIP engine): a message the caller left, spooled at tempPath (runtime/tmp/voicemail), stored at
 * dbresource://moqui/call/voicemail/(box)/(voicemail).ogg. One whose audio does not store is kept without it
 * (VmsAudioFailed) so its owner hears of the call; then the owner is told, and the transcription job queued.
 */
function store(p) {
  const box = p.voicemailBoxId ? one(BOX, { voicemailBoxId: p.voicemailBoxId }) : null
  if (!box) {
    discardSpool(p.tempPath)
    return refuse('notFound', 'Voicemail box not found')
  }
  if (blank(p.tempPath) && blank(p.fromLocation)) return refuse('noAudio', 'A voicemail needs its spooled file or its received copy')
  const voicemail = ec.entity.makeValue(VOICEMAIL)
  voicemail.setSequencedIdPrimary()
  const location = `${ROOT}/${box.voicemailBoxId}/${voicemail.voicemailId}.ogg`
  voicemail.setFields({ voicemailBoxId: box.voicemailBoxId, callSessionId: p.callSessionId || null,
    fromNumber: p.fromNumber || null, fromName: p.fromName || null, receivedDate: p.receivedDate || ec.user.nowTimestamp,
    durationMs: Math.max(Number(p.durationMs) || 0, 0), folder: 'inbox', statusId: 'VmsStored' }, true, null, null)
  ec.message.pushErrors()
  try {
    const out = ec.service.sync().name('org.moqui.call.VoicemailServices.store#VoicemailFile')
      .parameters({ tempPath: blank(p.fromLocation) ? p.tempPath : null, fromLocation: p.fromLocation || null, contentLocation: location })
      .requireNewTransaction(true).disableAuthz().call() || {}
    if (ec.message.hasError()) throw new Error(ec.message.errorsString)
    voicemail.setFields({ audioLocation: location, byteSize: out.byteSize || null }, true, null, null)
  } catch (err) {
    const text = err && err.message ? err.message : String(err)
    ec.logger.error(`Voicemail ${voicemail.voicemailId}: its audio was not stored, the message is kept without it: ${text}`)
    voicemail.setFields({ statusId: 'VmsAudioFailed', lastErrorText: text.slice(0, 2000) }, true, null, null)
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
  voicemail.create()
  roomNote(voicemail)
  received(box, voicemail)
  return { voicemailId: voicemail.voicemailId }
}

// ---------- messages the edge node recorded (plans/Edge-Node.md §5.4 "Voice (SIP)", decision 4, E8) ----------

/** The most ring groups a number's fallback is followed through (Voice.js MAX_GROUP_HOPS). */
const MAX_GROUP_HOPS = 5

/** A ring group's box, made with the defaults when it has none (CallHandling.js boxFor). */
function groupBox(ringGroupId) {
  let box = find(BOX).condition('ringGroupId', ringGroupId).one()
  if (!box && one('moqui.call.RingGroup', { ringGroupId })) {
    box = ec.entity.makeValue(BOX)
    box.setSequencedIdPrimary()
    box.setFields({ ringGroupId, attachAudio: 'N', attachTranscript: 'N' }, true, null, null)
    box.create()
  }
  return box
}

/**
 * The box a settings operation is about: a ring group's (ringGroupId: for voice admins only; another user gets notFound
 * for the same group), else the signed-in user's; made when create is true and there is none. {box} or
 * {refusal}; box is null for a user with none yet.
 */
function settingsBox(p, create) {
  const me = ec.user.userId
  if (!me) return { refusal: refuse('notSignedIn', 'Sign in first') }
  if (blank(p.ringGroupId)) return { box: boxOf(me, create) }
  const ringGroupId = String(p.ringGroupId)
  if (!ec.user.hasPermission('VoiceAdmin') || !one('moqui.call.RingGroup', { ringGroupId })) {
    return { refusal: refuse('notFound', 'Ring group not found') }
  }
  return { box: create ? groupBox(ringGroupId) : find(BOX).condition('ringGroupId', ringGroupId).one() }
}

/**
 * The box a number's calls leave messages in when nothing here decided where the call went: the user's it rings (made
 * when they have none; not for a disabled user), or its ring group's fallback box, followed through other groups; null
 * for any other route (Voice.js fallbackOf gives the same).
 */
function boxOfRoute(phoneNumber) {
  const number = blank(phoneNumber) ? null : one(NUMBER, { phoneNumber })
  if (!number || blank(number.routeId)) return null
  if (number.routeTypeEnumId === 'PnrUser') {
    const account = one('moqui.security.UserAccount', { userId: number.routeId })
    return account && account.disabled !== 'Y' ? boxOf(number.routeId, true) : null
  }
  if (number.routeTypeEnumId !== 'PnrRingGroup') return null
  let groupId = number.routeId
  for (let hop = 0; hop < MAX_GROUP_HOPS && groupId; hop++) {
    const group = one('moqui.call.RingGroup', { ringGroupId: groupId })
    if (!group) return null
    if (group.fallbackTypeEnumId === 'RgfVoicemail') return groupBox(group.ringGroupId)
    if (group.fallbackTypeEnumId !== 'RgfGroup') return null
    groupId = group.fallbackId
  }
  return null
}

/**
 * The queue kind voicemail (receive#EdgeVoicemail), as _edge_ only: a message the edge node's SIP engine recorded. Its
 * box is the one begin#Voicemail chose for the call when this node routed it (the session's, its media on the edge
 * node), else the called number's own again; never one the edge node names.
 */
function receiveEdge(p) {
  if (!isEdgeCaller()) return refuse('notAllowed', 'Only the edge link queue stores a message the edge node recorded')
  let item
  try {
    item = JSON.parse(p.payload || '{}')
  } catch (err) {
    return refuse('badRequest', 'The edge voicemail item is not JSON')
  }
  if (blank(p.contentLocation)) return refuse('badRequest', 'The edge voicemail item came without its audio')
  const session = blank(item.callSessionId) ? null : one(SESSION, { callSessionId: item.callSessionId })
  const routed = session && edgeAllowed({ callSessionId: session.callSessionId }) && !blank(session.voicemailBoxId)
  const box = routed ? one(BOX, { voicemailBoxId: session.voicemailBoxId }) : boxOfRoute(item.toNumber)
  if (!box) {
    ec.logger.error(`Edge voicemail item ${p.queueItemId}: the number ${item.toNumber || '(none)'} has no voicemail box here, the message is dropped`)
    return {}
  }
  const received = Number(item.receivedMs)
  const out = store({ voicemailBoxId: box.voicemailBoxId, fromLocation: p.contentLocation,
    durationMs: Math.max(Number(item.durationMs) || 0, 0), callSessionId: routed ? session.callSessionId : null,
    fromNumber: blank(item.fromNumber) ? null : String(item.fromNumber).slice(0, 60),
    fromName: blank(item.fromName) ? null : String(item.fromName).slice(0, 120),
    receivedDate: received > 0 && received <= nowMs() + 60000 ? new Date(received) : null })
  return { voicemailId: out.voicemailId || null }
}

/**
 * C4-SMS: a message left on a call in a number pair's room is shown there too, as an event of the call (CetCall, action
 * voicemail) that names it; its audio stays the box's, for the box's readers only. A failure is logged.
 */
function roomNote(voicemail) {
  const session = voicemail.callSessionId ? one(SESSION, { callSessionId: voicemail.callSessionId }) : null
  if (!session || !session.chatRoomId) return
  ec.message.pushErrors()
  try {
    ec.service.sync().name('org.moqui.chat.ChatServices.append#Event').parameters({ chatRoomId: session.chatRoomId, eventTypeEnumId: 'CetCall',
      content: { callSessionId: session.callSessionId, action: 'voicemail', kind: 'phone', voicemailId: voicemail.voicemailId,
        durationSeconds: Math.round(Number(voicemail.durationMs || 0) / 1000), phoneNumber: voicemail.fromNumber || null } }).disableAuthz().call()
    if (ec.message.hasError()) throw new Error(ec.message.errorsString)
  } catch (err) {
    ec.logger.warn(`Voicemail ${voicemail.voicemailId}: not shown in room ${session.chatRoomId}: ${err && err.message ? err.message : err}`)
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
}

function discardSpool(tempPath) {
  if (blank(tempPath)) return
  ec.message.pushErrors()
  try {
    ec.service.sync().name('org.moqui.call.VoicemailServices.store#VoicemailFile').parameters({ tempPath, contentLocation: '' })
      .requireNewTransaction(true).disableAuthz().call()
  } catch (err) {
    ec.logger.warn(`Voicemail spool ${tempPath} not removed: ${err && err.message ? err.message : err}`)
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
}

/** The owner hears of a new message, and its transcription and email notice are queued; each part on its own. */
function received(box, voicemail) {
  const from = voicemail.fromName || voicemail.fromNumber || ec.l10n.localize('Unknown number')
  const title = ec.resource.expand('New voicemail from ${from}', '', { from })
  // a ring group's box tells each of its members (V3)
  const userIds = box.userId ? [box.userId] : box.ringGroupId ? rows('moqui.call.RingGroupMember', { ringGroupId: box.ringGroupId }).map((m) => m.userId) : []
  const safely = (label, fn) => {
    ec.message.pushErrors()
    try {
      fn()
    } catch (err) {
      ec.logger.error(`Voicemail ${voicemail.voicemailId}: ${label} failed: ${err && err.message ? err.message : err}`)
    } finally {
      ec.message.clearErrors()
      ec.message.popErrors()
    }
  }
  for (const userId of userIds) {
    safely('the push to ' + userId, () => {
      ec.service.sync().name('org.moqui.chat.ChatServices.push#Signal').parameters({
        topic: 'chatUser:' + userId, userIdList: [userId], message: { kind: 'voicemail', action: 'new', voicemailId: voicemail.voicemailId,
          from, unheardCount: unheardOfUser(userId) },
      }).disableAuthz().call()
    })
  }
  if (userIds.length) {
    safely('the Web Push notice', () => {
      ec.service.special().name('org.moqui.ui.NotificationServices.send#WebPush').parameters({ userIdList: userIds, title,
        link: voicemailPath(voicemail.voicemailId), topic: 'Voicemail', tag: 'voicemail-' + voicemail.voicemailId }).registerOnCommit()
    })
  }
  const transcribing = transcribeOn() && !!voicemail.audioLocation
  if (transcribing) {
    safely('queueing its transcription', () => {
      pendingTranscript(voicemail.voicemailId)
      ec.service.special().name('org.moqui.call.VoicemailServices.transcribe#Voicemail')
        .parameter('voicemailId', voicemail.voicemailId).registerOnCommit()
    })
  }
  // with the transcript in it, the email waits for the transcription
  if (!blank(box.notifyEmail) && !(transcribing && yes(box.attachTranscript))) {
    safely('queueing its email notice', () => {
      ec.service.special().name('org.moqui.call.VoicemailServices.send#VoicemailNotice')
        .parameter('voicemailId', voicemail.voicemailId).registerOnCommit()
    })
  }
}

function unheardOf(box) {
  return find(VOICEMAIL).condition('voicemailBoxId', box.voicemailBoxId).condition('folder', 'inbox').condition('heardDate', null).count()
}

/** The unheard messages of every box the user reads (their own and their ring groups'). */
function unheardOfUser(userId) {
  const ids = readableBoxes(userId).map((b) => b.voicemailBoxId)
  return ids.length ? find(VOICEMAIL).condition('voicemailBoxId', 'in', ids).condition('folder', 'inbox').condition('heardDate', null).count() : 0
}

function pendingTranscript(voicemailId) {
  const row = ec.entity.makeValue(TRANSCRIPT)
  row.setSequencedIdPrimary()
  row.setFields({ voicemailId, segment: 1, statusId: 'CtsPending', createdDate: ec.user.nowTimestamp }, true, null, null)
  row.create()
  return row
}

/**
 * The transcription job: the voicemail's audio to text (SpeechServices.transcribe#Audio, as its owner, purpose
 * voicemail:<id>). Only while voicemail_transcribe is on (the deployment's recording and transcription policy) and a
 * provider is set up; a failure is the transcript's state (CtsFailed with why), never the audio's.
 */
function transcribe(p) {
  const voicemail = p.voicemailId ? one(VOICEMAIL, { voicemailId: p.voicemailId }) : null
  if (!voicemail) return { reasonCode: 'notFound' }
  const box = one(BOX, { voicemailBoxId: voicemail.voicemailBoxId })
  if (!transcribeOn()) return { reasonCode: 'transcriptionOff' }
  let row = latestTranscript(voicemail.voicemailId)
  if (!row || row.statusId === 'CtsDone') row = pendingTranscript(voicemail.voicemailId)
  else if (row.statusId === 'CtsFailed') {
    row.setFields({ statusId: 'CtsPending', lastErrorText: null }, true, null, null)
    row.update()
  }
  let fields
  if (!voicemail.audioLocation) {
    fields = { statusId: 'CtsFailed', lastErrorText: ec.l10n.localize('The audio was not stored') }
  } else {
    let out = {}
    ec.message.pushErrors()
    try {
      out = ec.service.sync().name('org.moqui.call.SpeechServices.transcribe#Audio').parameters({ audioLocation: voicemail.audioLocation,
        mediaType: 'audio/ogg', userId: box ? box.userId : null, purpose: 'voicemail:' + voicemail.voicemailId,
        language: property('voicemail_transcribe_language', '') || null }).requireNewTransaction(true).disableAuthz().call() || {}
      if (ec.message.hasError()) out = { reasonCode: 'failed', errorText: ec.message.errorsString }
    } catch (err) {
      out = { reasonCode: 'failed', errorText: err && err.message ? err.message : String(err) }
    } finally {
      ec.message.clearErrors()
      ec.message.popErrors()
    }
    fields = out.reasonCode
      ? { statusId: 'CtsFailed', lastErrorText: (out.reasonCode + (out.errorText ? ': ' + out.errorText : '')).slice(0, 2000) }
      : { statusId: 'CtsDone', text: out.text || '', lastErrorText: null }
  }
  row.setFields(fields, true, null, null)
  row.update()
  if (fields.statusId === 'CtsFailed') ec.logger.warn(`Voicemail ${voicemail.voicemailId}: not transcribed, the audio is kept: ${fields.lastErrorText}`)
  // the email that waited for the transcript goes now, with it or with its state
  if (box && !blank(box.notifyEmail) && yes(box.attachTranscript) && !voicemail.noticeSentDate) {
    notice({ voicemailId: voicemail.voicemailId })
  }
  return { status: TRANSCRIPT_CODES[fields.statusId] }
}

/**
 * The email notice of a new message to its box's notifyEmail, through the framework's mail (an EmailMessage on the SYSTEM
 * server): who called, when, how long, and a link to it. The audio is attached only when the box's attachAudio is on, and
 * the transcript is in the message only when attachTranscript is: mail cannot be recalled. Sent once.
 */
function notice(p) {
  const voicemail = p.voicemailId ? one(VOICEMAIL, { voicemailId: p.voicemailId }) : null
  const box = voicemail ? one(BOX, { voicemailBoxId: voicemail.voicemailBoxId }) : null
  if (!voicemail || !box || blank(box.notifyEmail) || voicemail.noticeSentDate) return { sent: false }
  const L = (text) => ec.l10n.localize(text)
  const from = voicemail.fromName || voicemail.fromNumber || L('Unknown number')
  const subject = ec.resource.expand('New voicemail from ${from}', '', { from })
  const seconds = Math.round(Number(voicemail.durationMs || 0) / 1000)
  const lines = [subject, L('Received') + ': ' + ec.l10n.format(new Date(ms(voicemail.receivedDate)), 'yyyy-MM-dd HH:mm'),
    L('Length') + ': ' + seconds + ' s', L('Listen') + ': ' + siteUrl() + voicemailPath(voicemail.voicemailId)]
  if (yes(box.attachTranscript)) {
    const transcript = latestTranscript(voicemail.voicemailId)
    if (transcript && transcript.statusId === 'CtsDone') lines.push('', L('Transcript') + ':', transcript.text || '')
    else if (transcript && transcript.statusId === 'CtsFailed') lines.push('', L('The transcript could not be made.'))
  }
  const email = ec.entity.makeValue('moqui.basic.email.EmailMessage')
  email.setSequencedIdPrimary()
  email.setFields({ statusId: 'ES_READY', subject, toAddresses: box.notifyEmail, emailServerId: 'SYSTEM', contentType: 'text/plain',
    body: lines.join('\n') }, true, null, null)
  email.create()
  if (yes(box.attachAudio) && voicemail.audioLocation) {
    const attachment = ec.entity.makeValue('moqui.basic.email.EmailMessageAttachment')
    attachment.setSequencedIdPrimary()
    attachment.setFields({ emailMessageId: email.emailMessageId, attachmentLocation: voicemail.audioLocation,
      fileName: 'voicemail-' + voicemail.voicemailId + '.ogg', fileSize: voicemail.byteSize || null, contentType: 'audio/ogg' }, true, null, null)
    attachment.create()
  }
  voicemail.set('noticeSentDate', ec.user.nowTimestamp)
  voicemail.update()
  ec.service.special().name('org.moqui.mail.EmailServices.send#EmailMessage').parameter('emailMessageId', email.emailMessageId).registerOnCommit()
  return { sent: true, emailMessageId: email.emailMessageId }
}

// ---------- phone access (the SIP engine) ----------

function failureRow(key) {
  return find(PIN_FAILURE).condition('failureKey', key).forUpdate(true).one()
}

function blockedUntil(row) {
  return row ? ms(row.blockedUntilDate) || 0 : 0
}

/** Counts a failure under key, blocking it at limit in window; true when it is now blocked. */
function countFailure(key, limit, window, block) {
  const now = nowMs()
  let row = failureRow(key)
  if (!row) {
    row = ec.entity.makeValue(PIN_FAILURE)
    row.setFields({ failureKey: key, failureCount: 0, firstFailureDate: new Date(now) }, true, null, null)
    row.create()
  }
  if ((ms(row.firstFailureDate) || 0) < now - window) row.setFields({ failureCount: 0, firstFailureDate: new Date(now) }, true, null, null)
  const count = Number(row.failureCount || 0) + 1
  row.setFields({ failureCount: count, lastFailureDate: new Date(now),
    blockedUntilDate: count >= limit ? new Date(now + block) : row.blockedUntilDate }, true, null, null)
  row.update()
  return count >= limit
}

function countBoxFailure(box) {
  const now = nowMs()
  const fresh = (ms(box.pinFailureDate) || 0) >= now - BOX_WINDOW_MS
  const count = (fresh ? Number(box.pinFailureCount || 0) : 0) + 1
  // pinFailureDate is the first failure of the window
  box.setFields({ pinFailureCount: count, pinFailureDate: fresh ? box.pinFailureDate : new Date(now),
    pinBlockedUntil: count >= BOX_FAILURES ? new Date(now + BOX_BLOCK_MS) : box.pinBlockedUntil }, true, null, null)
  box.update()
  return count >= BOX_FAILURES
}

/** The box a number entered on the access number names: the one user its PhoneNumber rings; null for any other. */
function boxOfNumber(digits) {
  if (digits.length < 4) return null
  const matches = Array.from(find(NUMBER).condition('phoneNumber', 'like', '%' + digits).limit(3).list())
    .filter((n) => String(n.phoneNumber).replace(/\D/g, '').endsWith(digits))
  if (matches.length !== 1 || matches[0].routeTypeEnumId !== 'PnrUser' || blank(matches[0].routeId)) return null
  return boxOf(matches[0].routeId, false)
}

/**
 * The burn of a PIN check that has no box or no PIN, so it takes as long as one that does: one argon2 verify against a
 * stand-in with the parameters hashPassword uses (argon2id m=19456, t=2, p=1). A constant, as module state does not last
 * between calls: making it each time would double the work and tell the cases apart.
 */
const BURN_HASH = '$argon2id$v=19$m=19456,t=2,p=1$KBg++4RAP5/1otppCxt6MA$tgUTiDXK/eH6KeQTwgaX45bqP84tS4nqhe4ecUa2nYE'
function burnCheck(pin) {
  ec.user.passwordsMatch(pin || '0000', BURN_HASH, 'argon2')
}

/**
 * Internal (the SIP engine): a PIN entered over the phone, for the box of the call (voicemailBoxId, after * in its
 * greeting) or the box a number names (boxNumber, on the access number). An answer, never an error, so the failure counts
 * commit: accepted with the box's new messages (oldest first), or not (blocked when the box or the source is now blocked).
 * A box that does not exist, has no PIN, or is blocked is refused the same way, after the same work, so a caller learns
 * nothing of which boxes exist. Failures are counted per box (VoicemailBox.pin*) and per source (the caller's number).
 */
function checkPin(p) {
  // the edge node's engine (E8): only for a call whose media is on it, whose PIN then opens the box for that call
  if (isEdgeCaller() && (blank(p.callSessionId) || !edgeAllowed({ callSessionId: p.callSessionId }))) {
    return { accepted: false, blocked: false }
  }
  const pin = String(p.pin || '')
  const sourceKey = 'source:' + (String(p.fromNumber || '').replace(/[^0-9+]/g, '') || 'unknown')
  let box = null
  let boxKey
  if (!blank(p.voicemailBoxId)) {
    box = find(BOX).condition('voicemailBoxId', p.voicemailBoxId).forUpdate(true).one()
    boxKey = 'box:' + p.voicemailBoxId
  } else {
    const digits = String(p.boxNumber || '').replace(/\D/g, '').slice(0, 20)
    box = boxOfNumber(digits)
    if (box) box = find(BOX).condition('voicemailBoxId', box.voicemailBoxId).forUpdate(true).one()
    boxKey = box ? 'box:' + box.voicemailBoxId : 'number:' + digits
  }
  const now = nowMs()
  const blocked = blockedUntil(failureRow(sourceKey)) > now || (box ? (ms(box.pinBlockedUntil) || 0) > now : blockedUntil(failureRow(boxKey)) > now)
  let accepted = false
  if (!blocked && box && !blank(box.pinHash) && PIN_PATTERN.test(pin)) accepted = ec.user.passwordsMatch(pin, box.pinHash, 'argon2')
  else burnCheck(pin)
  if (!accepted) {
    let nowBlocked = blocked
    if (!blocked) {
      const sourceBlocked = countFailure(sourceKey, SOURCE_FAILURES, SOURCE_WINDOW_MS, SOURCE_BLOCK_MS)
      const boxBlocked = box ? countBoxFailure(box) : countFailure(boxKey, BOX_FAILURES, BOX_WINDOW_MS, BOX_BLOCK_MS)
      nowBlocked = sourceBlocked || boxBlocked
    }
    ec.logger.info(`Voicemail PIN refused on call ${p.callSessionId} from ${sourceKey}${nowBlocked ? ' (blocked)' : ''}`)
    return { accepted: false, blocked: nowBlocked }
  }
  box.setFields({ pinFailureCount: 0, pinBlockedUntil: null }, true, null, null)
  box.update()
  // the box this call opened: the only one whose messages it may play and act on from the edge node (E8)
  const opened = blank(p.callSessionId) ? null : find(SESSION).condition('callSessionId', p.callSessionId).forUpdate(true).one()
  if (opened && opened.voicemailAccessBoxId !== box.voicemailBoxId) {
    opened.set('voicemailAccessBoxId', box.voicemailBoxId)
    opened.update()
  }
  const messageList = Array.from(find(VOICEMAIL).condition('voicemailBoxId', box.voicemailBoxId).condition('folder', 'inbox')
    .condition('heardDate', null).condition('statusId', 'VmsStored').orderBy('receivedDate').limit(50).list())
    .map((v) => ({ voicemailId: v.voicemailId, audioLocation: v.audioLocation, fromNumber: v.fromNumber || null,
      receivedDate: ms(v.receivedDate), durationMs: Number(v.durationMs || 0) }))
  return { accepted: true, blocked: false, voicemailBoxId: box.voicemailBoxId, messageList }
}

/** Internal (the SIP engine, after an accepted PIN): a key on a message: delete (7), save (9: heard) or heard (played). */
function phoneAct(p) {
  const voicemail = p.voicemailId ? one(VOICEMAIL, { voicemailId: p.voicemailId }) : null
  if (!voicemail || voicemail.voicemailBoxId !== p.voicemailBoxId) return refuse('notFound', 'Voicemail not found')
  // the edge node's engine (E8): only on a call, its media on the edge node, whose caller gave this box's PIN
  if (isEdgeCaller()) {
    const session = blank(p.callSessionId) ? null : one(SESSION, { callSessionId: p.callSessionId })
    if (!session || session.statusId === 'CssEnded' || session.voicemailAccessBoxId !== p.voicemailBoxId ||
      !edgeAllowed({ callSessionId: session.callSessionId })) return refuse('notFound', 'Voicemail not found')
  }
  if (p.op === 'delete') toTrash(voicemail)
  else if (p.op === 'save' || p.op === 'heard') markHeard(voicemail, true)
  else return refuse('badOp', 'Not a voicemail action: ' + p.op)
  return {}
}

// ---------- the owner's (Calls Voicemail, Settings Phone) ----------

function markHeard(voicemail, heard) {
  if (!!voicemail.heardDate === heard) return
  voicemail.set('heardDate', heard ? ec.user.nowTimestamp : null)
  voicemail.update()
}

function toTrash(voicemail) {
  if (voicemail.folder === 'trash') return
  voicemail.setFields({ folder: 'trash', trashedDate: ec.user.nowTimestamp }, true, null, null)
  voicemail.update()
}

/** Deletes a message for good: its audio, its transcripts and its row. */
function purge(voicemail) {
  if (voicemail.audioLocation) {
    const ref = ec.resource.getLocationReference(voicemail.audioLocation)
    if (ref.getExists()) ref.delete()
  }
  find(TRANSCRIPT).condition('voicemailId', voicemail.voicemailId).deleteAll()
  voicemail.delete()
}

function findVoicemails(p) {
  const boxes = readableBoxes(ec.user.userId)
  const empty = { voicemailList: [], voicemailListCount: 0, unheardCount: 0, trashCount: 0, hasBox: boxes.length > 0 }
  if (!boxes.length) return empty
  const ids = boxes.map((b) => b.voicemailBoxId)
  const groupOfBox = new Map(boxes.filter((b) => b.ringGroupId).map((b) => [b.voicemailBoxId, groupName(b.ringGroupId)]))
  const folder = p.folder === 'trash' ? 'trash' : 'inbox'
  const pageSize = clamp(Number(p.pageSize) || 25, 1, PAGE_MAX)
  const pageIndex = Math.max(Number(p.pageIndex) || 0, 0)
  const base = () => {
    let f = find(VOICEMAIL).condition('voicemailBoxId', 'in', ids).condition('folder', folder)
    if (p.heard === 'unheard') f = f.condition('heardDate', null)
    else if (p.heard === 'heard') f = f.condition('heardDate', 'is-not-null', null)
    return f
  }
  const list = Array.from(base().orderBy('-receivedDate').offset(pageIndex * pageSize).limit(pageSize).list())
  return {
    voicemailList: list.map((v) => Object.assign(view(v, false), { groupName: groupOfBox.get(v.voicemailBoxId) || null })),
    voicemailListCount: base().count(),
    unheardCount: unheardOfUser(ec.user.userId),
    trashCount: find(VOICEMAIL).condition('voicemailBoxId', 'in', ids).condition('folder', 'trash').count(),
    hasBox: true,
  }
}

function countUnheard() {
  const boxes = readableBoxes(ec.user.userId)
  return { unheardCount: boxes.length ? unheardOfUser(ec.user.userId) : 0, hasBox: boxes.length > 0 }
}

function getVoicemail(p) {
  const mine = myVoicemail(p.voicemailId)
  if (!mine) return refuse('notFound', 'Voicemail not found')
  return { voicemail: view(mine.voicemail, true) }
}

function audio(p) {
  const mine = myVoicemail(p.voicemailId)
  if (!mine || !mine.voicemail.audioLocation) return refuse('notFound', 'Voicemail not found')
  return { location: mine.voicemail.audioLocation, contentType: 'audio/ogg',
    fileName: 'voicemail-' + mine.voicemail.voicemailId + '.ogg' }
}

function mark(p) {
  const mine = myVoicemail(p.voicemailId)
  if (!mine) return refuse('notFound', 'Voicemail not found')
  markHeard(mine.voicemail, p.heard !== false && p.heard !== 'false' && p.heard !== 'N')
  return { voicemail: view(mine.voicemail, false) }
}

function remove(p) {
  const mine = myVoicemail(p.voicemailId)
  if (!mine) return refuse('notFound', 'Voicemail not found')
  if (mine.voicemail.folder === 'trash') {
    purge(mine.voicemail)
    return { deleted: true }
  }
  toTrash(mine.voicemail)
  return { trashed: true }
}

function restore(p) {
  const mine = myVoicemail(p.voicemailId)
  if (!mine) return refuse('notFound', 'Voicemail not found')
  mine.voicemail.setFields({ folder: 'inbox', trashedDate: null }, true, null, null)
  mine.voicemail.update()
  return { voicemail: view(mine.voicemail, false) }
}

function emptyTrash() {
  // the trash the user sees: their own box's and their ring groups' (V3), whose trash the members share as they share the inbox
  const ids = readableBoxes(ec.user.userId).map((b) => b.voicemailBoxId)
  if (!ids.length) return { deletedCount: 0 }
  let deletedCount = 0
  for (const voicemail of find(VOICEMAIL).condition('voicemailBoxId', 'in', ids).condition('folder', 'trash').list()) {
    purge(voicemail)
    deletedCount++
  }
  return { deletedCount }
}

function retryTranscript(p) {
  const mine = myVoicemail(p.voicemailId)
  if (!mine) return refuse('notFound', 'Voicemail not found')
  if (!transcribeOn()) return refuse('transcriptionOff', 'Transcription is off here')
  if (!mine.voicemail.audioLocation) return refuse('noAudio', 'This voicemail has no audio to transcribe')
  const row = latestTranscript(mine.voicemail.voicemailId)
  if (row && row.statusId !== 'CtsFailed') return refuse('notFailed', 'This voicemail is transcribed or being transcribed')
  if (row) {
    row.setFields({ statusId: 'CtsPending', lastErrorText: null }, true, null, null)
    row.update()
  } else {
    pendingTranscript(mine.voicemail.voicemailId)
  }
  ec.service.special().name('org.moqui.call.VoicemailServices.transcribe#Voicemail').parameter('voicemailId', mine.voicemail.voicemailId)
    .registerOnCommit()
  return { voicemail: view(mine.voicemail, true) }
}

/** Failed-PIN counts kept this long after their last failure (past every window and block). */
const PIN_FAILURE_KEEP_MS = 24 * 3600000

/**
 * The trash job: messages in the trash longer than voicemail_trash_days (0 keeps them), each on its own; and the
 * per-source and no-box PIN failure counts a day old, which a caller faking numbers would otherwise pile up.
 */
function expireTrash() {
  ec.message.pushErrors()
  try {
    ec.service.sync().name('org.moqui.call.VoicemailServices.expire#VoicemailPinFailures').requireNewTransaction(true).disableAuthz().call()
    if (ec.message.hasError()) ec.logger.error(`Voicemail PIN failures not cleaned up: ${ec.message.errorsString}`)
  } catch (err) {
    ec.logger.error(`Voicemail PIN failures not cleaned up: ${err && err.message ? err.message : err}`)
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
  const days = trashDays()
  if (!days) return { deletedCount: 0 }
  const before = new Date(nowMs() - days * 86400000)
  const ids = Array.from(find(VOICEMAIL).condition('folder', 'trash').condition('trashedDate', '<', before)
    .selectField('voicemailId').limit(1000).list()).map((v) => v.voicemailId)
  let deletedCount = 0
  for (const voicemailId of ids) {
    ec.message.pushErrors()
    try {
      const out = ec.service.sync().name('org.moqui.call.VoicemailServices.expire#VoicemailItem').parameter('voicemailId', voicemailId)
        .requireNewTransaction(true).disableAuthz().call() || {}
      if (ec.message.hasError()) ec.logger.error(`Voicemail trash: ${voicemailId} not deleted: ${ec.message.errorsString}`)
      else if (out.deleted) deletedCount++
    } catch (err) {
      ec.logger.error(`Voicemail trash: ${voicemailId} not deleted: ${err && err.message ? err.message : err}`)
    } finally {
      ec.message.clearErrors()
      ec.message.popErrors()
    }
  }
  return { deletedCount }
}

function expirePinFailures() {
  const before = new Date(nowMs() - PIN_FAILURE_KEEP_MS)
  return { deletedCount: find(PIN_FAILURE).condition('lastFailureDate', '<', before).deleteAll() }
}

function expireItem(p) {
  const voicemail = one(VOICEMAIL, { voicemailId: p.voicemailId })
  const days = trashDays()
  if (!voicemail || voicemail.folder !== 'trash' || !days || (ms(voicemail.trashedDate) || nowMs()) >= nowMs() - days * 86400000) {
    return { deleted: false }
  }
  purge(voicemail)
  return { deleted: true }
}

// ---------- settings ----------

function settingsView(box) {
  return {
    voicemailBoxId: box ? box.voicemailBoxId : null,
    hasPin: !!box && !blank(box.pinHash),
    hasGreeting: !!box && !blank(box.greetingLocation),
    greetingContentType: box ? box.greetingContentType || null : null,
    greetingDurationMs: box && box.greetingDurationMs ? Number(box.greetingDurationMs) : null,
    notifyEmail: box ? box.notifyEmail || '' : '',
    attachAudio: !!box && yes(box.attachAudio),
    attachTranscript: !!box && yes(box.attachTranscript),
    rings: box && box.rings ? Number(box.rings) : null,
  }
}

function settings(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  if (!blank(p.ringGroupId)) {
    const target = settingsBox(p, false)
    if (target.refusal) return target.refusal
    return { box: settingsView(target.box), maxSeconds: maxSeconds(), transcribeOn: transcribeOn() }
  }
  const box = boxOf(me, false)
  const numbers = rows(NUMBER, { routeTypeEnumId: 'PnrUser', routeId: me }).map((n) => n.phoneNumber)
  const access = rows(NUMBER, { routeTypeEnumId: 'PnrVoicemail' }).map((n) => n.phoneNumber)
  return { box: settingsView(box), myNumberList: numbers, accessNumberList: access, ringSeconds: callRingSeconds(),
    ringSecondsEach: RING_SECONDS, maxRings: MAX_RINGS, maxSeconds: maxSeconds(), transcribeOn: transcribeOn(), pinMinDigits: pinMinDigits() }
}

function updateSettings(p) {
  const target = settingsBox(p, true)
  if (target.refusal) return target.refusal
  const email = String(p.notifyEmail || '').trim()
  if (email && !/^[^\s@,;<>]+@[^\s@,;<>]+\.[^\s@,;<>]+$/.test(email)) return refuse('badEmail', 'That is not an email address')
  let rings = blank(p.rings) ? null : Math.round(Number(p.rings))
  if (rings != null && (Number.isNaN(rings) || rings < 1 || rings > MAX_RINGS)) return refuse('badRings', 'Rings are 1 to ' + MAX_RINGS)
  const box = target.box
  const before = { audio: yes(box.attachAudio), transcript: yes(box.attachTranscript) }
  const attachAudio = yes(p.attachAudio) && !!email
  const attachTranscript = yes(p.attachTranscript) && !!email
  // a ring group's box rings by the group's own seconds: it has no rings of its own
  box.setFields({ notifyEmail: email || null, attachAudio: attachAudio ? 'Y' : 'N', attachTranscript: attachTranscript ? 'Y' : 'N',
    rings: box.ringGroupId ? null : rings }, true, null, null)
  box.update()
  const turnedOn = (attachAudio && !before.audio) || (attachTranscript && !before.transcript)
  return { box: settingsView(box),
    warning: turnedOn ? ec.l10n.localize('Email cannot be recalled: the audio or the transcript of each new voicemail goes to that address and stays there.') : null }
}

/** A PIN no one should use: one digit repeated, or a run up or down (123456, 987654). */
function weakPin(pin) {
  if (/^(\d)\1+$/.test(pin)) return true
  const digits = pin.split('').map(Number)
  const step = digits[1] - digits[0]
  return (step === 1 || step === -1) && digits.every((d, i) => i === 0 || d - digits[i - 1] === step)
}

function setPin(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const box = boxOf(me, true)
  if (yes(p.clear)) {
    box.setFields({ pinHash: null, pinChangedDate: ec.user.nowTimestamp }, true, null, null)
    box.update()
    return { box: settingsView(box) }
  }
  const pin = String(p.pin || '').trim()
  const least = pinMinDigits()
  if (!/^[0-9]+$/.test(pin) || pin.length < least || pin.length > 12) {
    return refuse('badPin', ec.resource.expand('A PIN is ${least} to 12 digits', '', { least }))
  }
  if (weakPin(pin)) return refuse('weakPin', 'Choose a PIN that is not one digit repeated or a run like 123456')
  box.setFields({ pinHash: ec.user.hashPassword(pin), pinChangedDate: ec.user.nowTimestamp, pinFailureCount: 0, pinBlockedUntil: null },
    true, null, null)
  box.update()
  return { box: settingsView(box) }
}

/** What a greeting upload is, from its first bytes: wav, ogg (Ogg Opus) or null. */
function audioKind(head) {
  if (head.startsWith('RIFF') && head.slice(8, 12) === 'WAVE') return 'wav'
  if (head.startsWith('OggS') && head.indexOf('OpusHead') > 0 && head.indexOf('OpusHead') < 80) return 'ogg'
  return null
}

function deleteLocation(location) {
  if (blank(location)) return
  const ref = ec.resource.getLocationReference(location)
  if (ref.getExists()) ref.delete()
}

/**
 * The user's greeting, recorded in the browser or uploaded: WAV (16-bit PCM) or Ogg Opus, at most a minute. It is
 * converted to Ogg Opus by the SIP engine's audio (convert#GreetingAudio, in a build with phone calls), which plays to a
 * caller and in the browser; in a build without it the upload is kept as it is.
 */
function storeGreeting(p) {
  const chosen = settingsBox(p, true)
  if (chosen.refusal) return chosen.refusal
  const file = p.file
  if (!file) return refuse('noFile', 'No file uploaded')
  const size = Number(file.getSize() || 0)
  if (!size || size > MAX_GREETING_BYTES) return refuse('badAudio', 'A greeting is a WAV or Ogg Opus file of at most 10 MB')
  const kind = audioKind(String(file.getString('ISO-8859-1') || '').slice(0, 96))
  if (!kind) return refuse('badAudio', 'A greeting is a WAV or Ogg Opus file of at most 10 MB')
  const box = chosen.box
  const stamp = String(nowMs())
  const upload = `${ROOT}/greeting/${box.voicemailBoxId}/upload-${stamp}.${kind}`
  file.write(upload)
  const target = `${ROOT}/greeting/${box.voicemailBoxId}/greeting-${stamp}.ogg`
  let converted = null
  ec.message.pushErrors()
  try {
    const out = ec.service.sync().name('org.moqui.call.VoicemailServices.convert#GreetingAudio')
      .parameters({ sourceLocation: upload, targetLocation: target }).requireNewTransaction(true).disableAuthz().call() || {}
    if (!ec.message.hasError()) converted = out
  } catch (err) {
    ec.logger.info(`Voicemail greeting of box ${box.voicemailBoxId} kept as uploaded: ${err && err.message ? err.message : err}`)
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
  if (converted && converted.reasonCode) {
    deleteLocation(upload)
    return refuse('badAudio', 'That audio could not be read: ' + (converted.errorText || converted.reasonCode))
  }
  const old = box.greetingLocation
  const kept = converted ? target : upload
  if (converted) deleteLocation(upload)
  if (old && old !== kept) deleteLocation(old)
  box.setFields({ greetingLocation: kept, greetingContentType: converted || kind === 'ogg' ? 'audio/ogg' : 'audio/wav',
    greetingDurationMs: converted ? converted.durationMs || null : null }, true, null, null)
  box.update()
  fallbacksChanged()
  return { box: settingsView(box), converted: !!converted }
}

function deleteGreeting(p) {
  const target = settingsBox(p, false)
  if (target.refusal) return target.refusal
  const box = target.box
  if (!box) return {}
  deleteLocation(box.greetingLocation)
  box.setFields({ greetingLocation: null, greetingContentType: null, greetingDurationMs: null }, true, null, null)
  box.update()
  fallbacksChanged()
  return { box: settingsView(box) }
}

function greeting(p) {
  const target = settingsBox(p, false)
  if (target.refusal) return target.refusal
  const box = target.box
  if (!box || blank(box.greetingLocation)) return refuse('notFound', 'No greeting')
  return { location: box.greetingLocation, contentType: box.greetingContentType || 'audio/ogg' }
}

const operations = {
  routing, begin, store, receiveEdge, transcribe, notice, checkPin, phoneAct, findVoicemails, countUnheard, getVoicemail, audio, mark, remove,
  restore, emptyTrash, retryTranscript, expireTrash, expireItem, expirePinFailures, settings, updateSettings, setPin, storeGreeting, deleteGreeting,
  greeting,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown voicemail operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
