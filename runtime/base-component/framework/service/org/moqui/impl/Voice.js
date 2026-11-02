// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Voice (plans/Comms-Framework.md §5.2, V1): the services of org.moqui.impl.VoiceServices. Trunks, numbers, caller-ID
// grants, emergency locations and the fraud policy are set up by voice admins (the VoiceAdmin permission). A phone call
// is placed (place#PhoneCall) or received (receive#PhoneCall, from the SIP engine) as a CallSession through the
// CallServices state machine (Calls.js startPhone and phoneEvent); the SIP engine on the call's node is told what to do
// by send#SipControl and reports back through note#PhoneLeg. The fraud checks run here and again in the engine: caller
// IDs only from a user's grants, international and premium destinations off until the deployment and the user allow them,
// limits on calls at once and per day, and failed digest attempts blocked per source address, never by account.
// Emergency calls are checked first and go only through an emergency-capable trunk with the user's validated location, a
// callback number and a notification destination; otherwise they are refused with what is missing.

const CALLS = 'component://framework/service/org/moqui/impl/Calls.js'
const SESSION = 'moqui.call.CallSession'
const LEG = 'moqui.call.CallLeg'
const MEDIA_NODE = 'moqui.call.CallMediaNode'
const TRUNK = 'moqui.call.VoiceTrunk'
const NUMBER = 'moqui.call.PhoneNumber'
const GRANT = 'moqui.call.PhoneNumberUser'
const LOCATION = 'moqui.call.EmergencyLocation'
const SETTING = 'moqui.call.VoiceUserSetting'
const POLICY = 'moqui.call.VoicePolicy'
const AUTH_FAILURE = 'moqui.call.VoiceAuthFailure'
const SIP_NODE = 'moqui.call.SipNode'
const CALL_HANDLING = 'component://framework/service/org/moqui/impl/CallHandling.js'
const IVR = 'component://framework/service/org/moqui/impl/Ivr.js'
const PARTICIPANT = 'moqui.call.CallParticipant'
const SMS = 'component://framework/service/org/moqui/impl/Sms.js'
const LIMIT_LOCK = 'moqui.call.CallLimitLock'

/** The key a phone presses to acknowledge the recording notice (docs/interfaces/Call-Recording.md). */
const PHONE_ACK_KEY = '1'
/** The notices a phone leg hears as a prompt (moqui-host src/sip/prompt.rs). */
const PHONE_NOTICES = ['recording', 'recordingAck', 'captions']
/** The legs a SIP engine carries. */
const PHONE_LEGS = ['CltPstn', 'CltSipDevice']
/** A node with no heartbeat in this long is not up. */
const NODE_STALE_MS = 60000
/**
 * The profiles of a PBX: only their trunks' digest credentials sign a call in from any address. A provider trunk's are
 * its account's at the provider, which anyone could guess online, so its calls come only from its addresses.
 */
const PBX_PROFILES = ['VppPlainLan']
/** The widest inbound ranges a trunk takes: an IPv4 /8, an IPv6 /32 (moqui-sip ua.rs inbound_range). */
const MIN_V4_PREFIX = 8
const MIN_V6_PREFIX = 32
/** Provider profiles proven by an authorized live check (docs/interfaces/SIP.md); none yet. */
const VERIFIED_PROFILES = []
const DEFAULT_POLICY = {
  internationalEnabled: 'N', premiumEnabled: 'N', maxConcurrentCalls: 20, maxConcurrentPerUser: 2,
  maxCallsPerUserPerDay: 100, maxCallsPerDay: 1000, authFailureLimit: 5, authFailureWindowSeconds: 600,
  authBlockSeconds: 3600, homeCountryCode: '1', emergencyNumbers: '911,112,999', premiumPrefixes: '+1900,+1976',
  alertUserGroupId: 'ADMIN',
  // C4-SMS: texts a user and the users in all send a day, the system's own texts (sign-in codes) a day, and the number
  // they go from
  smsMaxPerUserPerDay: 200, smsMaxPerDay: 2000, smsSystemMaxPerDay: 500, smsSystemNumber: null,
}
const TRANSPORT_CODES = { VtrTls: 'tls', VtrTcp: 'tcp', VtrUdp: 'udp' }
const MEDIA_CODES = { VmeSrtp: 'srtp', VmePlain: 'plain' }
const STATUS_BY_REPORT = { registered: 'VtsRegistered', ok: 'VtsOk', failed: 'VtsFailed', unregistered: 'VtsUnknown' }
const ALGORITHMS = ['SHA-256', 'SHA-512-256', 'MD5']
/**
 * The area codes of the North American Numbering Plan's countries other than the United States (and its territories)
 * and Canada: international from a home country of 1, though they dial like domestic numbers. moqui-sip dial.rs
 * NANP_OTHER_COUNTRIES has the same list, for the engine's check.
 */
const NANP_OTHER_COUNTRIES = ['242', '246', '264', '268', '284', '345', '441', '473', '649', '658', '664', '721', '758', '767',
  '784', '809', '829', '849', '868', '869', '876']

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
const flag = (value) => (yes(value) ? 'Y' : 'N')
const blank = (value) => value == null || String(value).trim() === ''
const list = (text) => String(text || '').split(',').map((x) => x.trim()).filter((x) => x)

function calls(op, parameters) {
  return ec.resource.script(CALLS).execute(op, parameters) || {}
}

/** A refusal: an error for the user and a reason code for the caller. */
function refuse(reasonCode, text, extra) {
  ec.message.addError(ec.l10n.localize(text))
  return Object.assign({ reasonCode }, extra || {})
}

function isAdmin() {
  return !!ec.user.userId && ec.user.hasPermission('VoiceAdmin')
}

/** The admin services' guard: the same refusal whatever was asked. */
function notAdmin() {
  return isAdmin() ? null : refuse('notAllowed', 'Only a voice administrator may do this')
}

// ---------- the policy ----------

function policyRow() {
  return one(POLICY, { voicePolicyId: 'DEFAULT' })
}

/** The deployment's policy, the defaults filling what its row leaves empty. */
function policy() {
  const row = policyRow()
  const out = Object.assign({}, DEFAULT_POLICY)
  if (row) for (const key of Object.keys(DEFAULT_POLICY)) if (!blank(row[key])) out[key] = row[key]
  for (const key of ['maxConcurrentCalls', 'maxConcurrentPerUser', 'maxCallsPerUserPerDay', 'maxCallsPerDay',
    'authFailureLimit', 'authFailureWindowSeconds', 'authBlockSeconds', 'smsMaxPerUserPerDay', 'smsMaxPerDay', 'smsSystemMaxPerDay']) out[key] = Number(out[key])
  out.homeCountryCode = String(out.homeCountryCode).replace(/\D/g, '') || '1'
  return out
}

function userSetting(userId) {
  return (userId && one(SETTING, { userId })) || {}
}

// ---------- numbers ----------

/** The digits a user dialed, with a leading + kept. */
function cleanNumber(raw) {
  const text = String(raw || '').trim()
  const plus = text.startsWith('+')
  const digits = text.replace(/[^0-9*#]/g, '')
  return (plus ? '+' : '') + digits
}

/** A number in E.164 (+15551234567), from what was dialed with the home country's conventions; null when it is not one. */
function toE164(raw, homeCountryCode) {
  let text = cleanNumber(raw)
  if (/[*#]/.test(text)) return null
  if (!text.startsWith('+')) {
    if (text.startsWith('00')) text = '+' + text.slice(2)
    else if (homeCountryCode === '1' && text.startsWith('011')) text = '+' + text.slice(3)
    else if (homeCountryCode === '1' && text.length === 10) text = '+1' + text
    else if (homeCountryCode === '1' && text.length === 11 && text.startsWith('1')) text = '+' + text
    else if (homeCountryCode === '1') return null
    else if (text.startsWith('0')) text = '+' + homeCountryCode + text.slice(1)
    else text = '+' + homeCountryCode + text
  }
  return /^\+[1-9]\d{6,14}$/.test(text) ? text : null
}

/**
 * What a dialed number is: emergency (one of the policy's emergency numbers, exactly as dialed), premium (a premium
 * prefix), international (outside the home country code, or another country of the North American plan from a home
 * country of 1) or domestic; null when it is not a number. Emergency comes first. The engine classes a number again
 * with the same rules (moqui-sip dial.rs).
 */
function classify(raw, pol) {
  const dialed = cleanNumber(raw)
  if (!dialed) return null
  if (list(pol.emergencyNumbers).includes(dialed)) return { destinationClass: 'emergency', number: dialed }
  const number = toE164(raw, pol.homeCountryCode)
  if (!number) return null
  if (list(pol.premiumPrefixes).some((prefix) => number.startsWith(prefix))) return { destinationClass: 'premium', number }
  if (!number.startsWith('+' + pol.homeCountryCode)) return { destinationClass: 'international', number }
  if (pol.homeCountryCode === '1' && NANP_OTHER_COUNTRIES.includes(number.slice(2, 5))) return { destinationClass: 'international', number }
  return { destinationClass: 'domestic', number }
}

/** The numbers the user may present now: their grants, and the numbers of the ring groups they are in (V3). */
function grantsOf(userId) {
  if (!userId) return []
  const now = ec.user.nowTimestamp
  const granted = rows(GRANT, { userId }).filter((g) => ms(g.fromDate) <= ms(now) && (!g.thruDate || ms(g.thruDate) > ms(now)))
    .map((g) => g.phoneNumber)
  for (const number of groupNumbersOf(userId)) if (!granted.includes(number)) granted.push(number)
  return granted
}

/** The numbers routed to the ring groups the user is a member of. */
function groupNumbersOf(userId) {
  const groups = rows('moqui.call.RingGroupMember', { userId }).map((m) => m.ringGroupId)
  if (!groups.length) return []
  return Array.from(find(NUMBER).condition('routeTypeEnumId', 'PnrRingGroup').condition('routeId', 'in', groups).list())
    .map((n) => n.phoneNumber)
}

// ---------- nodes and trunks ----------

function liveSipNodes() {
  return rows(SIP_NODE).filter((n) => ms(n.heartbeatDate) >= nowMs() - NODE_STALE_MS)
}

/** The nodes a phone call can go to: a live SIP engine with a live media node that takes calls. */
function voiceNodes() {
  const sip = new Set(liveSipNodes().map((n) => n.instanceId))
  return Array.from(find(MEDIA_NODE).condition('heartbeatDate', '>=', new Date(nowMs() - NODE_STALE_MS))
    .orderBy('-heartbeatDate').list()).filter((n) => n.draining !== 'Y' && sip.has(n.instanceId))
}

function enabledTrunks() {
  return rows(TRUNK).filter((t) => t.disabled !== 'Y')
}

/** Tells a node's SIP engine (after the commit); one that cannot be told is logged, as a node that is gone is. */
function sipControl(instanceId, message) {
  ec.message.pushErrors()
  try {
    ec.service.sync().name('org.moqui.impl.VoiceServices.send#SipControl').parameters({ instanceId, message })
      .disableAuthz().call()
    if (ec.message.hasError()) ec.logger.error(`SIP engine ${instanceId} not told ${message.action}: ${ec.message.errorsString}`)
  } catch (err) {
    ec.logger.error(`SIP engine ${instanceId} not told ${message.action}: ${err && err.message ? err.message : err}`)
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
}

/** Each live SIP engine reloads its trunks and policy. */
function reloadEngines() {
  for (const node of liveSipNodes()) sipControl(node.instanceId, { action: 'reload' })
}

function authOf(trunk) {
  if (!trunk || blank(trunk.authJson)) return {}
  const parsed = JSON.parse(String(trunk.authJson))
  return parsed && typeof parsed === 'object' ? parsed : {}
}

// ---------- emergency readiness ----------

/** The user's registered location: their own, else the location of a number they may present. */
function locationOf(userId) {
  const setting = userSetting(userId)
  if (setting.emergencyLocationId) return one(LOCATION, { emergencyLocationId: setting.emergencyLocationId })
  for (const phoneNumber of grantsOf(userId)) {
    const number = one(NUMBER, { phoneNumber })
    if (number?.emergencyLocationId) return one(LOCATION, { emergencyLocationId: number.emergencyLocationId })
  }
  return null
}

const MISSING_TEXT = {
  noEmergencyTrunk: 'no trunk is set up for emergency calls',
  noLocation: 'no emergency location is registered for you',
  locationNotValidated: 'your emergency location is not validated with the provider',
  noCallbackNumber: 'your emergency location has no callback number',
  noNotification: 'your emergency location has no notification destination',
}

/** What an emergency call of this user needs, and what is missing (§5.2, D-V2). */
function readiness(userId) {
  const trunks = enabledTrunks().filter((t) => t.emergencyCapable === 'Y' && t.statusId !== 'VtsFailed')
  const location = userId ? locationOf(userId) : null
  const missing = []
  if (!trunks.length) missing.push('noEmergencyTrunk')
  if (!location) missing.push('noLocation')
  else {
    if (location.validationStatusId !== 'ElvValidated') missing.push('locationNotValidated')
    if (blank(location.callbackNumber)) missing.push('noCallbackNumber')
    if (blank(location.notifyEmailAddress)) missing.push('noNotification')
  }
  // the trunk of the callback number when it can take emergency calls, else the first that can
  const callbackTrunk = location && trunks.find((t) => t.voiceTrunkId === one(NUMBER, { phoneNumber: location.callbackNumber })?.voiceTrunkId)
  return { ready: !missing.length, missing, location, trunk: callbackTrunk || trunks[0] || null }
}

function missingText(missing) {
  return missing.map((code) => ec.l10n.localize(MISSING_TEXT[code])).join('; ')
}

/** Tells the location's notification destination and the voice admins of an emergency call, failure tolerant. */
function notifyEmergency(userId, location, callSessionId) {
  const pol = policy()
  const who = userId
  const title = ec.resource.expand('Emergency call by ${who} from ${place}', '', { who, place: location.name || location.emergencyLocationId })
  try {
    ec.makeNotificationMessage().topic('VoiceAlert').userGroupId(pol.alertUserGroupId)
      .message({ kind: 'emergencyCall', userId, emergencyLocationId: location.emergencyLocationId, callSessionId })
      .title(title).send()
  } catch (err) {
    ec.logger.error(`Emergency call ${callSessionId}: the admins were not notified: ${err && err.message ? err.message : err}`)
  }
  try {
    const email = ec.entity.makeValue('moqui.basic.email.EmailMessage')
    email.setSequencedIdPrimary()
    email.setFields({ statusId: 'ES_READY', subject: title, toAddresses: location.notifyEmailAddress, emailServerId: 'SYSTEM',
      sentDate: null, contentType: 'text/plain',
      body: title + '\n' + [location.address1, location.address2, location.city, location.postalCode].filter((x) => x).join(', ') +
        '\n' + ec.l10n.localize('Callback number') + ': ' + location.callbackNumber }, true, null, null)
    email.create()
    ec.service.async().name('org.moqui.impl.EmailServices.send#EmailMessage').parameter('emailMessageId', email.emailMessageId).call()
  } catch (err) {
    ec.logger.error(`Emergency call ${callSessionId}: ${location.notifyEmailAddress} was not emailed: ${err && err.message ? err.message : err}`)
  }
}

// ---------- placing and receiving calls ----------

/**
 * The one reservation for the call limits: the CallLimitLock row, locked until the caller's transaction ends, before the
 * calls at once and in a day are counted. The counts are the call rows the caller then makes in that transaction, so two
 * calls placed at once are counted one after the other and cannot pass a limit together; a call refused or never placed
 * rolls back, or closes as CerRefused, and so no longer counts.
 */
function lockLimits() {
  const row = find(LIMIT_LOCK).condition('lockKey', 'all').forUpdate(true).one()
  if (row) return
  // the seed row is missing (data not loaded): made now, and locked as made
  const made = ec.entity.makeValue(LIMIT_LOCK)
  made.setFields({ lockKey: 'all', lastLockedDate: ec.user.nowTimestamp }, true, null, null)
  made.create()
}

/** A condition leaving out the phone legs refused by the limits or the policy (CerRefused): they were never placed. */
function notRefused() {
  return ec.entity.conditionFactory.makeCondition('endReasonEnumId', 'not-equals', 'CerRefused', true)
}

function countPhoneCalls(fields, sinceMs) {
  let f = find(SESSION).condition('kindEnumId', 'CskPhone')
  for (const [k, v] of Object.entries(fields)) f = f.condition(k, v)
  if (sinceMs != null) f = f.condition('startedDate', '>=', new Date(sinceMs))
  else f = f.condition('statusId', '!=', 'CssEnded')
  return f.count()
}

function dial(started, trunk, dest, fromNumber, fromName, userId, emergency, location, privacy) {
  sipControl(started.mediaNodeId, {
    action: 'dial', callSessionId: started.callSessionId, callLegId: started.phoneLegId, voiceTrunkId: trunk.voiceTrunkId,
    toNumber: dest.number, fromNumber, fromName, destinationClass: dest.destinationClass, emergency: !!emergency,
    callbackNumber: location ? location.callbackNumber : null, locationReference: location ? location.providerReference || null : null,
    userId, privacy: !!privacy && !emergency,
  })
}

/** Caller-ID blocking (V3): asked for this call, else the user's setting (CallHandlingSetting.hideCallerId). */
function hideCallerId(p, userId) {
  if (p.hideCallerId != null && p.hideCallerId !== '') return yes(p.hideCallerId)
  return one('moqui.call.CallHandlingSetting', { userId })?.hideCallerId === 'Y'
}

function userName(userId) {
  const account = one('moqui.security.UserAccount', { userId })
  return account ? account.userFullName || account.username || userId : userId
}

function place(p, viaNode) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in to make a call')
  const pol = policy()
  const dest = classify(p.phoneNumber, pol)
  if (!dest) return refuse('badNumber', 'That is not a phone number')
  const key = p.idempotencyKey ? String(p.idempotencyKey).slice(0, 200) : null
  if (key && find(SESSION).condition('startedByUserId', me).condition('idempotencyKey', key).count() > 0) {
    return calls('startPhone', { userId: me, idempotencyKey: key })
  }

  // emergency first: never refused for caller ID, destination or limits, only for what it needs
  if (dest.destinationClass === 'emergency') {
    const ready = readiness(me)
    if (!ready.ready) {
      return refuse('emergencyNotReady', ec.resource.expand('Emergency calls cannot be placed from here: ${what}. Call emergency services from another phone.', '',
        { what: missingText(ready.missing) }), { missing: ready.missing })
    }
    const nodes = voiceNodes().filter((n) => !viaNode || n.instanceId === viaNode)
    if (!nodes.length) return refuse('voiceUnavailable', 'Phone calls are not available: no SIP server is running. Call emergency services from another phone.')
    const started = calls('startPhone', { userId: me, direction: 'outbound', phoneNumber: dest.number, displayName: dest.number,
      mediaNodeId: nodes[0].instanceId, idempotencyKey: key })
    started.mediaNodeId = nodes[0].instanceId
    dial(started, ready.trunk, dest, ready.location.callbackNumber, userName(me), me, true, ready.location)
    notifyEmergency(me, ready.location, started.callSessionId)
    return { callSessionId: started.callSessionId, callLegId: started.callLegId, phoneLegId: started.phoneLegId, call: started.call,
      voiceTrunkId: ready.trunk.voiceTrunkId, fromNumber: ready.location.callbackNumber, destinationClass: 'emergency' }
  }

  const setting = userSetting(me)
  if (setting.outboundEnabled !== 'Y') return refuse('notAllowed', 'You may not place phone calls; an administrator can allow it')
  const granted = grantsOf(me)
  const fromNumber = blank(p.fromNumber) ? (granted.length === 1 ? granted[0] : null) : toE164(p.fromNumber, pol.homeCountryCode)
  if (!fromNumber || !granted.includes(fromNumber)) return refuse('callerIdNotAllowed', 'Choose one of your numbers to call from')
  if (dest.destinationClass === 'international' && !(pol.internationalEnabled === 'Y' && setting.internationalEnabled === 'Y')) {
    return refuse('internationalNotEnabled', 'International calls are not enabled for you')
  }
  if (dest.destinationClass === 'premium' && !(pol.premiumEnabled === 'Y' && setting.premiumEnabled === 'Y')) {
    return refuse('premiumNotEnabled', 'Calls to premium numbers are not enabled for you')
  }
  const number = one(NUMBER, { phoneNumber: fromNumber })
  const trunk = number?.voiceTrunkId ? one(TRUNK, { voiceTrunkId: number.voiceTrunkId }) : null
  if (!trunk || trunk.disabled === 'Y') return refuse('noTrunk', 'Your number has no trunk to call out through')
  const nodes = voiceNodes().filter((n) => !viaNode || n.instanceId === viaNode)
  if (!nodes.length) return refuse('voiceUnavailable', 'Phone calls are not available: no SIP server is running')
  // C4-SMS: the call is in the pair's room, with its texts, when our number has messaging
  const pair = pairRoom(fromNumber, dest.number)
  const started = calls('startPhone', { userId: me, direction: 'outbound', phoneNumber: dest.number, displayName: dest.number,
    mediaNodeId: nodes[0].instanceId, idempotencyKey: key, chatRoomId: pair })
  // the limits, counted under the one lock with this call already made (lockLimits): taken after the call's rows, as a
  // call coming in takes it after its own (its room, its number), so the locks come in one order. A refusal rolls the
  // call back, so it never counts.
  lockLimits()
  // a user's own setting can lower the policy's limits, never raise them
  const perUser = blank(setting.maxConcurrent) ? pol.maxConcurrentPerUser : Math.min(Number(setting.maxConcurrent), pol.maxConcurrentPerUser)
  // the user's forwards (V3) and the numbers they added or transferred to (V5) count with the calls they place
  if (countPhoneCalls({ startedByUserId: me }) + forwardLegCount(me, null) + dialedLegCount(me, null) > perUser ||
    countPhoneCalls({}) > pol.maxConcurrentCalls) {
    return refuse('tooManyCalls', 'Too many phone calls at once; try again when one ends')
  }
  const dayStart = nowMs() - 86400000
  const perDay = blank(setting.maxPerDay) ? pol.maxCallsPerUserPerDay : Math.min(Number(setting.maxPerDay), pol.maxCallsPerUserPerDay)
  if (countPhoneCalls({ startedByUserId: me, directionEnumId: 'CdrOutbound' }, dayStart) + forwardLegCount(me, dayStart) +
    dialedLegCount(me, dayStart) > perDay ||
    countPhoneCalls({ directionEnumId: 'CdrOutbound' }, dayStart) > pol.maxCallsPerDay) {
    return refuse('dailyLimit', 'The limit of phone calls for a day is reached')
  }
  started.mediaNodeId = nodes[0].instanceId
  const privacy = hideCallerId(p, me)
  dial(started, trunk, dest, fromNumber, privacy ? null : userName(me), me, false, null, privacy)
  return { callSessionId: started.callSessionId, callLegId: started.callLegId, phoneLegId: started.phoneLegId, call: started.call,
    voiceTrunkId: trunk.voiceTrunkId, fromNumber, destinationClass: dest.destinationClass }
}

/** The pair room of our number and theirs (Sms.js), null when our number has no messaging or the room cannot be made. */
function pairRoom(ours, theirs) {
  if (!ours || !theirs) return null
  return ec.resource.script(SMS).execute('pairRoomFor', { ourNumber: ours, theirNumber: theirs })?.chatRoomId || null
}

/** C4-SMS: the users phoneNumber no longer reaches are taken out of its pair rooms (Sms.js syncNumberRooms). */
function syncPairRooms(phoneNumber, deleted) {
  if (!phoneNumber) return
  ec.resource.script(SMS).execute('syncNumberRooms', { phoneNumber, deleted: deleted === true })
}

function receive(p) {
  const pol = policy()
  const to = toE164(p.toNumber, pol.homeCountryCode)
  const number = to ? one(NUMBER, { phoneNumber: to }) : null
  if (!number || (p.voiceTrunkId && number.voiceTrunkId && number.voiceTrunkId !== p.voiceTrunkId)) return { reasonCode: 'notFound' }
  // the voicemail access number (V2): the caller gives a box number and its PIN
  const access = number.routeTypeEnumId === 'PnrVoicemail'
  const group = number.routeTypeEnumId === 'PnrRingGroup'
  // V5: an IVR menu or a queue, which the engine answers (Ivr.js)
  const menu = number.routeTypeEnumId === 'PnrIvr'
  const queue = number.routeTypeEnumId === 'PnrQueue'
  if (!access && !['PnrUser', 'PnrRingGroup', 'PnrIvr', 'PnrQueue'].includes(number.routeTypeEnumId)) return { reasonCode: 'notFound' }
  if (!access && blank(number.routeId)) return { reasonCode: 'notFound' }
  if (group && !one('moqui.call.RingGroup', { ringGroupId: number.routeId })) return { reasonCode: 'notFound' }
  if (menu && !one('moqui.call.IvrMenu', { ivrMenuId: number.routeId })) return { reasonCode: 'notFound' }
  if (queue && !one('moqui.call.CallQueue', { callQueueId: number.routeId })) return { reasonCode: 'notFound' }
  const account = access || group || menu || queue ? null : one('moqui.security.UserAccount', { userId: number.routeId })
  if (!access && !group && !menu && !queue && (!account || account.disabled === 'Y')) return { reasonCode: 'notFound' }
  if (!voiceNodes().some((n) => n.instanceId === p.instanceId)) return { reasonCode: 'voiceUnavailable' }
  // the edge node's engine (E8) takes calls only as itself
  if (!edgeAllowed({ instanceId: p.instanceId })) return refuse('notAllowed', 'The edge node takes calls only on its own SIP engine')
  if (countPhoneCalls({}) >= pol.maxConcurrentCalls) return { reasonCode: 'busy' }
  const e164 = toE164(p.fromNumber, pol.homeCountryCode)
  const from = e164 || cleanNumber(p.fromNumber) || ec.l10n.localize('Unknown number')
  const name = blank(p.fromName) || p.fromName === from ? from : String(p.fromName).slice(0, 60) + ' (' + from + ')'
  if (access) {
    const started = calls('startPhone', { userId: null, direction: 'inbound', phoneNumber: from, displayName: name, mediaNodeId: p.instanceId })
    return { callSessionId: started.callSessionId, callLegId: started.phoneLegId, voicemail: 'access', voicemailSeconds: null }
  }
  if (menu || queue) {
    return ec.resource.script(IVR).execute('inbound', { kind: menu ? 'menu' : 'queue', routeId: number.routeId, toNumber: to,
      fromNumber: e164, rawFrom: from, displayName: name, instanceId: p.instanceId, atMs: p.atMs }) || {}
  }
  // V3: the user's call handling rules, or the ring group's strategy (CallHandling.js); V2's busy and no-answer
  // voicemail are its last steps
  return ec.resource.script(CALL_HANDLING).execute('inbound', { userId: group ? null : number.routeId, ringGroupId: group ? number.routeId : null,
    toNumber: to, fromNumber: e164, rawFrom: p.fromNumber || null, displayName: name, instanceId: p.instanceId, atMs: p.atMs,
    chatRoomId: e164 ? pairRoom(to, e164) : null }) || {}
}

// ---------- forwards (V3: call handling rules, simultaneous ring, a ring group's fallback) ----------

/**
 * Whether a call may be forwarded to an outside number for a user (CallHandling.js): number (E.164) and its
 * destinationClass, and allowed with reasonCode when not. Emergency numbers are never forwarded to; the user needs
 * outbound calls and forwardEnabled (VoiceUserSetting), and international and premium destinations need the
 * deployment's and the user's permission, as for a call they place. A ring group's fallback (byAdmin, set by a voice
 * admin) needs only the deployment's.
 */
function forwardCheck(p) {
  const pol = policy()
  const dest = classify(p.phoneNumber, pol)
  if (!dest) return { number: null, allowed: false, reasonCode: 'badNumber' }
  const out = { number: dest.number, destinationClass: dest.destinationClass, allowed: false }
  if (dest.destinationClass === 'emergency') return Object.assign(out, { reasonCode: 'emergency' })
  const setting = p.byAdmin ? {} : userSetting(p.userId)
  // V5: a transfer to a number, or a number added to a call, is a call the user places: outbound calls, not forwards
  if (!p.byAdmin && p.transfer && setting.outboundEnabled !== 'Y') return Object.assign(out, { reasonCode: 'outboundNotAllowed' })
  if (!p.byAdmin && !p.transfer && !(setting.outboundEnabled === 'Y' && setting.forwardEnabled === 'Y')) return Object.assign(out, { reasonCode: 'forwardNotAllowed' })
  if (dest.destinationClass === 'international' && !(pol.internationalEnabled === 'Y' && (p.byAdmin || setting.internationalEnabled === 'Y'))) {
    return Object.assign(out, { reasonCode: 'internationalNotEnabled' })
  }
  if (dest.destinationClass === 'premium' && !(pol.premiumEnabled === 'Y' && (p.byAdmin || setting.premiumEnabled === 'Y'))) {
    return Object.assign(out, { reasonCode: 'premiumNotEnabled' })
  }
  return Object.assign(out, { allowed: true, reasonCode: null })
}

/** The user's forwarded legs since sinceMs (open ones only when sinceMs is null). */
function forwardLegCount(userId, sinceMs) {
  const participants = Array.from(find(PARTICIPANT).condition('userId', userId).selectField('callParticipantId')
    .orderBy('-callParticipantId').limit(500).list()).map((x) => x.callParticipantId)
  if (!participants.length) return 0
  let f = find(LEG).condition('callParticipantId', 'in', participants).condition('legTypeEnumId', 'CltPstn')
  f = sinceMs != null ? f.condition('offeredDate', '>=', new Date(sinceMs)).condition(notRefused())
    : f.condition('statusId', 'in', ['ClsOffered', 'ClsAnswered'])
  return f.count()
}

/**
 * V5: the numbers the user added to calls or transferred calls to (CallLeg dialedByUserId), offered since sinceMs, or (with
 * sinceMs null) still open in a call that has not ended.
 */
function dialedLegCount(userId, sinceMs) {
  const f = find(LEG).condition('dialedByUserId', userId).condition('legTypeEnumId', 'CltPstn')
  if (sinceMs != null) return f.condition('offeredDate', '>=', new Date(sinceMs)).condition(notRefused()).count()
  const open = Array.from(f.condition('statusId', 'in', ['ClsOffered', 'ClsAnswered']).selectField('callSessionId').limit(200).list())
  if (!open.length) return 0
  const live = new Set(Array.from(find(SESSION).condition('callSessionId', 'in', Array.from(new Set(open.map((l) => l.callSessionId))))
    .condition('statusId', '!=', 'CssEnded').selectField('callSessionId').list()).map((s) => s.callSessionId))
  return open.filter((l) => live.has(l.callSessionId)).length
}

/**
 * Internal (CallHandling.js): dials a forwarded leg (Calls.js made it, offered) through the trunk of the number the call
 * came in on, which is its caller ID, after forwardCheck and the user's limits at once and per day; the engine checks the
 * destination again. One refused is closed as failed here (reasonCode), and the call goes on with its other legs.
 */
function dialForward(p) {
  const check = forwardCheck(p)
  const refused = (reasonCode) => {
    ec.logger.warn(`Call ${p.callSessionId}: forward to ${check.number || p.phoneNumber} refused: ${reasonCode}`)
    calls('closeForward', { callLegId: p.callLegId, reasonCode })
    return { reasonCode }
  }
  if (!check.allowed) return refused(check.reasonCode || 'notAllowed')
  const pol = policy()
  const dayStart = nowMs() - 86400000
  // counted under the one lock (lockLimits): the leg being dialed is in this transaction, those of other forwards,
  // transfers and adds at once are counted once theirs commit
  lockLimits()
  if (!p.byAdmin && p.userId) {
    const setting = userSetting(p.userId)
    const perUser = blank(setting.maxConcurrent) ? pol.maxConcurrentPerUser : Math.min(Number(setting.maxConcurrent), pol.maxConcurrentPerUser)
    const perDay = blank(setting.maxPerDay) ? pol.maxCallsPerUserPerDay : Math.min(Number(setting.maxPerDay), pol.maxCallsPerUserPerDay)
    // the user's limits count their forwards and the numbers they added or transferred to (V5) with the calls they place;
    // the leg being dialed is open already
    if (forwardLegCount(p.userId, null) + dialedLegCount(p.userId, null) + countPhoneCalls({ startedByUserId: p.userId }) > perUser) {
      return refused('tooManyCalls')
    }
    if (forwardLegCount(p.userId, dayStart) + dialedLegCount(p.userId, dayStart) +
      countPhoneCalls({ startedByUserId: p.userId, directionEnumId: 'CdrOutbound' }, dayStart) > perDay) {
      return refused('dailyLimit')
    }
  }
  // the deployment's calls out in a day, for every forward (a ring group's fallback too): the phone legs offered since,
  // less the far ends of the calls that came in, are the calls placed and the forwards (with the one being dialed)
  const outToday = find(LEG).condition('legTypeEnumId', 'CltPstn').condition('offeredDate', '>=', new Date(dayStart))
    .condition(notRefused()).count() -
    countPhoneCalls({ directionEnumId: 'CdrInbound' }, dayStart)
  if (outToday > pol.maxCallsPerDay) return refused('dailyLimit')
  const fromNumber = toE164(p.fromNumber, pol.homeCountryCode)
  const number = fromNumber ? one(NUMBER, { phoneNumber: fromNumber }) : null
  const trunk = number?.voiceTrunkId ? one(TRUNK, { voiceTrunkId: number.voiceTrunkId }) : enabledTrunks()[0]
  if (!trunk || trunk.disabled === 'Y') return refused('noTrunk')
  const session = one(SESSION, { callSessionId: p.callSessionId })
  if (!session?.mediaNodeId) return refused('voiceUnavailable')
  dial({ mediaNodeId: session.mediaNodeId, callSessionId: p.callSessionId, phoneLegId: p.callLegId }, trunk,
    { number: check.number, destinationClass: check.destinationClass }, number ? number.phoneNumber : null, null, p.userId || null, false, null, false)
  return { dialed: true }
}

function noteLeg(p) {
  if (!edgeAllowed({ callSessionId: p.callSessionId, callLegId: p.callLegId })) {
    return refuse('notAllowed', 'The edge node notes only the legs of calls whose media is on it')
  }
  if (p.event === 'dtmf') return phoneKeys(p)
  if (p.event === 'ringing') return {}
  return calls('phoneEvent', p)
}

/**
 * Keys a phone pressed. PHONE_ACK_KEY acknowledges the recording notice when the call's recording asks for that
 * (CallRecordingServices.ack#LegRecordingNotice); otherwise keys are the call's own and nothing is kept.
 */
function phoneKeys(p) {
  if (!String(p.digits || '').includes(PHONE_ACK_KEY)) return {}
  ec.service.sync().name('org.moqui.impl.CallRecordingServices.ack#LegRecordingNotice')
    .parameters({ callSessionId: p.callSessionId, callLegId: p.callLegId }).disableAuthz().call()
  return {}
}

/**
 * A call's notice to a phone leg, as the SIP engine of its node plays it (docs/interfaces/SIP.md, "Notices"): recording
 * (the recording tone), recordingAck (the tone that asks for PHONE_ACK_KEY) or captions. One path for every notice a
 * browser sees as an rtc frame; a leg that is not an answered phone leg here is skipped (notified false).
 */
function notifyPhoneLeg(p) {
  if (!PHONE_NOTICES.includes(p.notice)) return refuse('badNotice', 'Not a phone notice: ' + p.notice)
  const leg = p.callLegId ? one(LEG, { callLegId: p.callLegId }) : null
  if (!leg || !PHONE_LEGS.includes(leg.legTypeEnumId) || leg.statusId !== 'ClsAnswered') return { notified: false }
  const session = one(SESSION, { callSessionId: leg.callSessionId })
  if (!session || !session.mediaNodeId || session.statusId === 'CssEnded') return { notified: false }
  // the leg's own node when it is on a leg node of the call (W7: a merged or cross-node phone)
  sipControl(leg.mediaNodeId || session.mediaNodeId, { action: 'prompt', notice: p.notice, callSessionId: session.callSessionId, callLegId: leg.callLegId })
  return { notified: true }
}

/** The phone legs of a call the current user is in (an answered leg of theirs). */
function myPhoneLegs(callSessionId) {
  const out = calls('get', { callSessionId })
  if (!out.call) return { refusal: { reasonCode: out.reasonCode || 'notFound' } }
  ec.message.clearErrors()
  if (out.call.myLegStatus !== 'answered') return { refusal: refuse('notInCall', 'You are not in this call') }
  const legs = rows(LEG, { callSessionId, legTypeEnumId: 'CltPstn', statusId: 'ClsAnswered' })
  if (!legs.length) return { refusal: refuse('noPhone', 'This call has no phone in it') }
  const session = one(SESSION, { callSessionId })
  return { legs, session }
}

function sendDtmf(p) {
  const digits = String(p.digits || '').toUpperCase()
  if (!/^[0-9*#A-D]{1,32}$/.test(digits)) return refuse('badDigits', 'Keys are 0 to 9, * and #')
  const mine = myPhoneLegs(p.callSessionId)
  if (mine.refusal) return mine.refusal
  for (const leg of mine.legs) {
    sipControl(leg.mediaNodeId || mine.session.mediaNodeId, { action: 'dtmf', callSessionId: p.callSessionId, callLegId: leg.callLegId, digits })
  }
  return { sent: true }
}

/**
 * Puts the call's phones on hold, or takes them off: the engine re-INVITEs each (sendonly), plays it hold music (the
 * deployment's call_hold_music_location, else a tone pattern) and keeps its audio from the others (V5: CallLeg heldDate).
 */
function holdOrResume(p, action) {
  const mine = myPhoneLegs(p.callSessionId)
  if (mine.refusal) return mine.refusal
  for (const leg of mine.legs) {
    calls('setHeld', { callLegId: leg.callLegId, held: action === 'hold' })
    sipControl(leg.mediaNodeId || mine.session.mediaNodeId, { action, callSessionId: p.callSessionId, callLegId: leg.callLegId })
  }
  return {}
}

/** The numbers the current user may call from, for the caller-ID choice. */
function myCallerIds() {
  return {
    numberList: grantsOf(ec.user.userId).map((phoneNumber) => {
      const number = one(NUMBER, { phoneNumber })
      const group = number?.routeTypeEnumId === 'PnrRingGroup' ? one('moqui.call.RingGroup', { ringGroupId: number.routeId }) : null
      return { phoneNumber, description: number?.description || (group ? group.name : null), ringGroupId: group ? group.ringGroupId : null }
    }),
  }
}

// ---------- the SIP engine ----------

function registerNode(p) {
  const row = one(SIP_NODE, { instanceId: p.instanceId })
  const now = ec.user.nowTimestamp
  // the edge link keeps only its edge node's row (plans/Edge-Node.md §5.4, E8), as CallServices register#MediaNode: never
  // a row an engine registered, and an engine's own heartbeat never takes a link-kept row over or ends it
  const linked = !!row && row.edgeLinked === 'Y'
  if (row && yes(p.edgeLinked) !== linked) {
    ec.logger.warn(`SIP node ${p.instanceId}: a ${yes(p.edgeLinked) ? 'link-kept' : 'registered'} heartbeat for a row the ${linked ? 'edge link keeps' : 'engine registers'} is refused`)
    return refuse('notAllowed', `SIP node ${p.instanceId} is not ${yes(p.edgeLinked) ? "this node's edge node" : 'a node of this cluster'}`)
  }
  if (yes(p.stopping)) {
    if (row) row.delete()
    return {}
  }
  const fields = { sipAddress: p.sipAddress || null, tlsAddress: p.tlsAddress || null, rtpPorts: p.rtpPorts || null, heartbeatDate: now }
  if (row) {
    row.setFields(fields, true, null, null)
    row.update()
  } else {
    const created = ec.entity.makeValue(SIP_NODE)
    created.setFields(Object.assign({ instanceId: p.instanceId, startedDate: now, edgeLinked: flag(p.edgeLinked) }, fields), true, null, null)
    created.create()
  }
  return {}
}

/**
 * Whether the caller may act for a call (callSessionId, callLegId) or as a node (instanceId): anyone but the edge link's
 * delegate user, which may only for a call whose media is on this node's edge node, or as that node (Calls.js
 * edgeAllowed; plans/Edge-Node.md §5.4 "Voice (SIP)", E8).
 */
function edgeAllowed(p) {
  return ec.user.username !== '_edge_' || !!calls('edgeAllowed', p).allowed
}

// ---------- SIP on the edge node (plans/Edge-Node.md §5.4 "Voice (SIP)", decision 4, E8) ----------

const FALLBACK = 'moqui.call.PhoneNumberFallback'
const BOX = 'moqui.call.VoicemailBox'
const FALLBACK_FIELDS = ['voiceTrunkId', 'voicemailFlag', 'greetingLocation', 'forwardNumber', 'forwardSeconds']
/** Where greetings are kept: only these are queued to the edge node. */
const GREETING_ROOT = 'dbresource://moqui/call/voicemail/greeting/'
/** Ring groups whose fallback is another group are followed this many times at most. */
const MAX_GROUP_HOPS = 5

/** A location under root with no empty, . or .. part (call_media.rs under). */
function under(location, root) {
  const text = String(location || '')
  if (!text.startsWith(root)) return false
  const rest = text.slice(root.length)
  return rest !== '' && !rest.split('/').some((part) => part === '' || part === '.' || part === '..')
}

/** How long the edge node rings a number it forwards to (call_forward_ring_seconds, 10 to 120). */
function forwardRingSeconds() {
  const seconds = Number(String(System.getProperty('call_forward_ring_seconds') || '').trim() || 30)
  return Math.min(Math.max(Number.isNaN(seconds) ? 30 : Math.round(seconds), 10), 120)
}

/**
 * What the edge node does with a call to number while this node is away, or null for nothing (480, so the trunk's own
 * failover acts): a user's number rings their mobile first when they ring it with their calls and may forward to it,
 * then takes a message; a ring group's fallback (followed through other groups) takes a message in the group's box, or
 * forwards to its outside number. The rules and schedules need this node, so they are not followed.
 */
function fallbackOf(number) {
  const out = { voiceTrunkId: number.voiceTrunkId || null, voicemailFlag: 'N', greetingLocation: null, forwardNumber: null, forwardSeconds: null }
  if (blank(number.routeId)) return null
  if (number.routeTypeEnumId === 'PnrUser') {
    const userId = number.routeId
    const account = one('moqui.security.UserAccount', { userId })
    if (!account || account.disabled === 'Y') return null
    const handling = one('moqui.call.CallHandlingSetting', { userId })
    if (handling && handling.ringMobile === 'Y' && !blank(handling.mobileNumber)) {
      const check = forwardCheck({ phoneNumber: handling.mobileNumber, userId })
      if (check.allowed) Object.assign(out, { forwardNumber: check.number, forwardSeconds: forwardRingSeconds() })
    }
    const box = find(BOX).condition('userId', userId).one()
    return Object.assign(out, { voicemailFlag: 'Y', greetingLocation: box?.greetingLocation || null })
  }
  if (number.routeTypeEnumId !== 'PnrRingGroup') return null
  let groupId = number.routeId
  for (let hop = 0; hop < MAX_GROUP_HOPS && groupId; hop++) {
    const group = one('moqui.call.RingGroup', { ringGroupId: groupId })
    if (!group) return null
    if (group.fallbackTypeEnumId === 'RgfVoicemail') {
      const box = find(BOX).condition('ringGroupId', group.ringGroupId).one()
      return Object.assign(out, { voicemailFlag: 'Y', greetingLocation: box?.greetingLocation || null })
    }
    if (group.fallbackTypeEnumId === 'RgfNumber') {
      const check = forwardCheck({ phoneNumber: group.fallbackId, byAdmin: true })
      return check.allowed ? Object.assign(out, { forwardNumber: check.number, forwardSeconds: forwardRingSeconds() }) : null
    }
    if (group.fallbackTypeEnumId !== 'RgfGroup') return null
    groupId = group.fallbackId
  }
  return null
}

/** Queues a greeting to the edge node (callAudio, keyed by its location: each goes once); one that fails is logged. */
function queueCallAudio(location) {
  ec.message.pushErrors()
  try {
    ec.service.sync().name('org.moqui.impl.EdgeServices.enqueue#Item').parameters({ kind: 'callAudio',
      conversationKey: 'callAudio:' + location, idempotencyKey: 'callAudio:' + location, payload: JSON.stringify({ location }),
      contentLocation: location }).disableAuthz().call()
    if (ec.message.hasError()) ec.logger.error(`Greeting ${location} not queued for the edge node: ${ec.message.errorsString}`)
  } catch (err) {
    ec.logger.error(`Greeting ${location} not queued for the edge node: ${err && err.message ? err.message : err}`)
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
}

/**
 * A number's route, box or greeting changed: on a paired node, its fallback is written again now (the edge link also
 * writes them all every minute); a failure is logged and the change goes on. phoneNumber null: every number.
 */
function fallbackChanged(phoneNumber) {
  if (String(System.getProperty('edge_node_url') || '').trim() === '') return
  ec.message.pushErrors()
  try {
    refreshFallbacks({ phoneNumber })
  } catch (err) {
    ec.logger.error(`The fallback of ${phoneNumber || 'the numbers'} was not written: ${err && err.message ? err.message : err}`)
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
}

/** The numbers' fallbacks written where they changed (all, or p.phoneNumber's), and their greetings queued when paired. */
function refreshFallbacks(p) {
  const numbers = !blank(p.phoneNumber) ? [one(NUMBER, { phoneNumber: p.phoneNumber })].filter((n) => n) : rows(NUMBER)
  const wanted = new Map()
  for (const number of numbers) {
    try {
      const fallback = fallbackOf(number)
      if (fallback) wanted.set(number.phoneNumber, fallback)
    } catch (err) {
      // one number that does not resolve keeps its row as it was, and the others go on
      wanted.set(number.phoneNumber, null)
      ec.logger.error(`The fallback of number ${number.phoneNumber} was not worked out: ${err && err.message ? err.message : err}`)
    }
  }
  const existing = !blank(p.phoneNumber) ? rows(FALLBACK, { phoneNumber: p.phoneNumber }) : rows(FALLBACK)
  const same = (row, want) => FALLBACK_FIELDS.every((field) => String(row[field] ?? '') === String(want[field] ?? ''))
  let writtenCount = 0
  let removedCount = 0
  const greetings = new Set()
  for (const row of existing) {
    if (wanted.has(row.phoneNumber) && wanted.get(row.phoneNumber) === null) {
      if (row.greetingLocation) greetings.add(row.greetingLocation)
      wanted.delete(row.phoneNumber)
      continue
    }
    const want = wanted.get(row.phoneNumber)
    wanted.delete(row.phoneNumber)
    if (!want) {
      row.delete()
      removedCount++
      continue
    }
    if (want.greetingLocation) greetings.add(want.greetingLocation)
    if (same(row, want)) continue
    row.setFields(want, true, null, null)
    row.update()
    writtenCount++
  }
  for (const [phoneNumber, want] of wanted) {
    if (!want) continue
    if (want.greetingLocation) greetings.add(want.greetingLocation)
    const created = ec.entity.makeValue(FALLBACK)
    created.setFields(Object.assign({ phoneNumber }, want), true, null, null)
    created.create()
    writtenCount++
  }
  // the edge node plays them while this node is away: on a paired node, each one there (a name is stamped, so once)
  if (String(System.getProperty('edge_node_url') || '').trim() !== '') {
    for (const location of greetings) if (under(location, GREETING_ROOT)) queueCallAudio(location)
  }
  return { writtenCount, removedCount }
}

function engineConfig() {
  const trunkList = []
  for (const trunk of enabledTrunks()) {
    try {
      const auth = authOf(trunk)
      const algorithms = (Array.isArray(auth.algorithms) ? auth.algorithms : list(auth.algorithms)).filter((a) => ALGORITHMS.includes(a))
      trunkList.push({
        voiceTrunkId: trunk.voiceTrunkId, name: trunk.name || trunk.voiceTrunkId, host: trunk.host,
        port: blank(trunk.port) ? null : Number(trunk.port), transport: TRANSPORT_CODES[trunk.transportEnumId] || 'tls',
        mediaEncryption: MEDIA_CODES[trunk.mediaEncryptionEnumId] || 'srtp', username: auth.username || null,
        password: auth.password || null, authUsername: auth.authUsername || null, realm: auth.realm || null,
        algorithms: algorithms.length ? algorithms : ['SHA-256', 'MD5'], register: trunk.registerFlag === 'Y',
        inboundAddresses: list(trunk.inboundAddresses), emergencyCapable: trunk.emergencyCapable === 'Y',
        fromDomain: trunk.fromDomain || null, profile: one('moqui.basic.Enumeration', { enumId: trunk.providerProfileEnumId })?.enumCode || 'generic',
        inboundDigest: PBX_PROFILES.includes(trunk.providerProfileEnumId),
      })
    } catch (err) {
      ec.logger.error(`Voice trunk ${trunk.voiceTrunkId} left out of the SIP engine, its credentials do not read: ${err && err.message ? err.message : err}`)
    }
  }
  const pol = policy()
  const now = nowMs()
  const blockedSourceList = Array.from(find(AUTH_FAILURE).condition('blockedUntilDate', '>', new Date(now)).list())
    .map((row) => ({ sourceAddress: row.sourceAddress, blockedUntil: ms(row.blockedUntilDate) }))
  return {
    trunkList,
    policy: {
      authFailureLimit: pol.authFailureLimit, authFailureWindowSeconds: pol.authFailureWindowSeconds,
      authBlockSeconds: pol.authBlockSeconds, maxConcurrentCalls: pol.maxConcurrentCalls,
      internationalEnabled: pol.internationalEnabled === 'Y', premiumEnabled: pol.premiumEnabled === 'Y',
      homeCountryCode: pol.homeCountryCode, emergencyNumbers: list(pol.emergencyNumbers), premiumPrefixes: list(pol.premiumPrefixes),
    },
    blockedSourceList,
  }
}

function alertAdmins(title, message) {
  try {
    ec.makeNotificationMessage().topic('VoiceAlert').userGroupId(policy().alertUserGroupId).message(message).title(title).send()
  } catch (err) {
    ec.logger.error(`Voice alert not sent (${title}): ${err && err.message ? err.message : err}`)
  }
}

function trunkStatus(p) {
  const trunk = find(TRUNK).condition('voiceTrunkId', p.voiceTrunkId).forUpdate(true).one()
  if (!trunk) return {}
  const statusId = STATUS_BY_REPORT[p.status] || 'VtsUnknown'
  const was = trunk.statusId
  trunk.setFields({ statusId, lastErrorText: statusId === 'VtsFailed' ? String(p.errorText || '').slice(0, 4000) : null,
    lastStatusDate: ec.user.nowTimestamp }, true, null, null)
  trunk.update()
  if (statusId === 'VtsFailed' && was !== 'VtsFailed') {
    alertAdmins(ec.resource.expand('Phone trunk ${name} failed', '', { name: trunk.name || trunk.voiceTrunkId }),
      { kind: 'trunkFailed', voiceTrunkId: trunk.voiceTrunkId, errorText: trunk.lastErrorText })
  }
  return {}
}

/**
 * A failed digest attempt from a source: counted in the window, blocked at the limit. The SIP engine counts them itself
 * and reports a source only when it blocks it (blocked), so a flood of failures from many addresses is not a write each;
 * that block is kept here as it is. Never touches a UserAccount.
 */
function authFailure(p) {
  const pol = policy()
  const source = String(p.sourceAddress || '').trim().slice(0, 200)
  if (!source) return { blocked: false }
  const now = nowMs()
  let row = find(AUTH_FAILURE).condition('sourceAddress', source).forUpdate(true).one()
  if (!row) {
    row = ec.entity.makeValue(AUTH_FAILURE)
    row.setFields({ sourceAddress: source, failureCount: 0, firstFailureDate: new Date(now) }, true, null, null)
    row.create()
  }
  const blockedUntil = ms(row.blockedUntilDate)
  let count = Number(row.failureCount || 0)
  let first = ms(row.firstFailureDate) || now
  if (now - first > pol.authFailureWindowSeconds * 1000 && !(blockedUntil > now)) {
    count = 0
    first = now
  }
  count = yes(p.blocked) ? Math.max(count + 1, pol.authFailureLimit) : count + 1
  const fields = { failureCount: count, firstFailureDate: new Date(first), lastFailureDate: new Date(now),
    lastUsername: p.username ? String(p.username).slice(0, 200) : null }
  let until = blockedUntil > now ? blockedUntil : null
  if (!until && count >= pol.authFailureLimit) {
    until = now + pol.authBlockSeconds * 1000
    fields.blockedUntilDate = new Date(until)
    fields.alertedDate = new Date(now)
    alertAdmins(ec.resource.expand('SIP source ${source} blocked after ${count} failed sign-ins', '', { source, count }),
      { kind: 'sourceBlocked', sourceAddress: source, failureCount: count, lastUsername: fields.lastUsername, blockedUntil: until })
  }
  row.setFields(fields, true, null, null)
  row.update()
  return { blocked: !!until, blockedUntil: until }
}

// ---------- capabilities ----------

function capabilities() {
  const supported = !!ec.factory.hasFeature('sip')
  const property = (name) => String(System.getProperty(name) || '').trim() !== ''
  const trunks = enabledTrunks()
  const configured = supported && (property('sip_port') || property('sip_tls_port')) && trunks.length > 0
  const nodes = voiceNodes()
  const working = trunks.filter((t) => t.statusId !== 'VtsFailed')
  const active = nodes.length > 0 && working.length > 0
  const reasonCode = active ? null : !supported && !nodes.length ? 'notBuilt' : !trunks.length ? 'noTrunk'
    : !nodes.length ? (supported && !property('sip_port') && !property('sip_tls_port') ? 'notConfigured' : 'noSipServer') : 'trunkFailed'
  const ready = readiness(ec.user.userId)
  return { supported, configured, active, reasonCode, emergencyReady: active && ready.ready,
    emergencyReasonCode: ready.missing[0] || (active ? null : 'voiceUnavailable'), emergencyMissing: ready.missing }
}

// ---------- administration ----------

/** An inbound address entry's problem, or null: an IPv4 or IPv6 address, or a range no wider than a /8 or a /32. */
function inboundEntryProblem(entry) {
  const [net, bits, extra] = String(entry).split('/')
  const v4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(net)
  const isV4 = !!v4 && v4.slice(1).every((part) => Number(part) <= 255)
  const isV6 = !isV4 && /^[0-9a-fA-F:.]+$/.test(net) && net.includes(':')
  if (extra !== undefined || (!isV4 && !isV6)) return 'not an address or range'
  if (bits === undefined) return null
  if (!/^\d{1,3}$/.test(bits)) return 'not an address or range'
  const prefix = Number(bits)
  if (prefix > (isV4 ? 32 : 128)) return 'not an address or range'
  if (prefix < (isV4 ? MIN_V4_PREFIX : MIN_V6_PREFIX)) return isV4 ? 'wider than a /8' : 'wider than a /32'
  return null
}

function profileAllowsInsecure(enumId) {
  return one('moqui.basic.Enumeration', { enumId, enumTypeId: 'VoiceProviderProfile' })?.optionValue === 'Y'
}

function trunkView(trunk) {
  let auth = {}
  try {
    auth = authOf(trunk)
  } catch (err) {
    auth = { unreadable: true }
  }
  const profile = one('moqui.basic.Enumeration', { enumId: trunk.providerProfileEnumId })
  const status = one('moqui.basic.StatusItem', { statusId: trunk.statusId })
  return {
    voiceTrunkId: trunk.voiceTrunkId, name: trunk.name, host: trunk.host, port: trunk.port, transportEnumId: trunk.transportEnumId,
    mediaEncryptionEnumId: trunk.mediaEncryptionEnumId, providerProfileEnumId: trunk.providerProfileEnumId,
    profileText: profile?.description || null, profileVerified: VERIFIED_PROFILES.includes(profile?.enumCode),
    username: auth.username || null, authUsername: auth.authUsername || null, realm: auth.realm || null,
    algorithms: (Array.isArray(auth.algorithms) ? auth.algorithms : list(auth.algorithms)).join(','), hasPassword: !blank(auth.password),
    credentialsUnreadable: !!auth.unreadable, registerFlag: trunk.registerFlag || 'N', emergencyCapable: trunk.emergencyCapable || 'N',
    inboundAddresses: trunk.inboundAddresses || null, fromDomain: trunk.fromDomain || null, disabled: trunk.disabled || 'N',
    statusId: trunk.statusId || 'VtsUnknown', statusText: status?.description || null, lastErrorText: trunk.lastErrorText || null,
    lastStatusDate: ms(trunk.lastStatusDate),
  }
}

function findTrunks() {
  const denied = notAdmin()
  if (denied) return denied
  return { trunkList: Array.from(find(TRUNK).orderBy('name').list()).map(trunkView) }
}

function storeTrunk(p) {
  const denied = notAdmin()
  if (denied) return denied
  let trunk = blank(p.voiceTrunkId) ? null : find(TRUNK).condition('voiceTrunkId', p.voiceTrunkId).forUpdate(true).one()
  if (!blank(p.voiceTrunkId) && !trunk) return refuse('notFound', 'Trunk not found')
  const pick = (key, fallback) => (p[key] !== undefined && p[key] !== null ? p[key] : trunk ? trunk[key] : fallback)
  const transportEnumId = pick('transportEnumId', 'VtrTls') || 'VtrTls'
  const mediaEncryptionEnumId = pick('mediaEncryptionEnumId', 'VmeSrtp') || 'VmeSrtp'
  const providerProfileEnumId = pick('providerProfileEnumId', 'VppGeneric') || 'VppGeneric'
  if (!TRANSPORT_CODES[transportEnumId] || !MEDIA_CODES[mediaEncryptionEnumId]) return refuse('badTrunk', 'Choose a transport and a media encryption')
  if ((transportEnumId !== 'VtrTls' || mediaEncryptionEnumId !== 'VmeSrtp') && !profileAllowsInsecure(providerProfileEnumId)) {
    return refuse('insecureNotAllowed', 'UDP or TCP signaling and plain RTP need a provider profile that allows them; TLS with SRTP is the default')
  }
  const host = String(pick('host', '') || '').trim()
  if (!host || /[\s/@]/.test(host)) return refuse('badTrunk', 'A trunk needs a host name or address')
  const inbound = blank(pick('inboundAddresses', null)) ? [] : list(pick('inboundAddresses', ''))
  for (const entry of inbound) {
    const problem = inboundEntryProblem(entry)
    if (problem) return refuse('badInboundAddress', `Inbound address ${entry}: ${problem}; list the provider's own addresses`)
  }
  const port = blank(pick('port', null)) ? null : Number(pick('port', null))
  if (port != null && !(port > 0 && port < 65536)) return refuse('badTrunk', 'The port is 1 to 65535')
  let auth = {}
  if (trunk) {
    try {
      auth = authOf(trunk)
    } catch (err) {
      auth = {}
    }
  }
  for (const key of ['username', 'authUsername', 'realm']) if (p[key] !== undefined && p[key] !== null) auth[key] = String(p[key]).trim() || null
  if (!blank(p.password)) auth.password = String(p.password)
  if (p.algorithms !== undefined && p.algorithms !== null) {
    const asked = Array.isArray(p.algorithms) ? p.algorithms : list(p.algorithms)
    const bad = asked.filter((a) => !ALGORITHMS.includes(a))
    if (bad.length) return refuse('badTrunk', 'Digest algorithms are SHA-256, SHA-512-256 and MD5')
    auth.algorithms = asked
  }
  const fields = {
    name: String(pick('name', host) || host).trim(), host, port, transportEnumId, mediaEncryptionEnumId, providerProfileEnumId,
    authJson: JSON.stringify(auth), registerFlag: flag(pick('registerFlag', 'N')), emergencyCapable: flag(pick('emergencyCapable', 'N')),
    inboundAddresses: inbound.length ? inbound.join(',') : null,
    fromDomain: blank(pick('fromDomain', null)) ? null : String(pick('fromDomain', '')).trim(), disabled: flag(pick('disabled', 'N')),
  }
  if (!trunk) {
    trunk = ec.entity.makeValue(TRUNK)
    trunk.setSequencedIdPrimary()
    trunk.setFields(Object.assign({ statusId: 'VtsUnknown' }, fields), true, null, null)
    trunk.create()
  } else {
    if (fields.disabled === 'Y') fields.statusId = 'VtsDisabled'
    else if (trunk.statusId === 'VtsDisabled') fields.statusId = 'VtsUnknown'
    trunk.setFields(fields, true, null, null)
    trunk.update()
  }
  reloadEngines()
  return { voiceTrunkId: trunk.voiceTrunkId }
}

function deleteTrunk(p) {
  const denied = notAdmin()
  if (denied) return denied
  const trunk = one(TRUNK, { voiceTrunkId: p.voiceTrunkId })
  if (!trunk) return refuse('notFound', 'Trunk not found')
  if (find(NUMBER).condition('voiceTrunkId', trunk.voiceTrunkId).count() > 0) return refuse('inUse', 'Numbers use this trunk; move or remove them first')
  trunk.delete()
  reloadEngines()
  return {}
}

function findNumbers() {
  const denied = notAdmin()
  if (denied) return denied
  const numbers = Array.from(find(NUMBER).orderBy('phoneNumber').list())
  const trunks = new Map(rows(TRUNK).map((t) => [t.voiceTrunkId, t.name || t.voiceTrunkId]))
  const locations = new Map(rows(LOCATION).map((l) => [l.emergencyLocationId, l.name || l.emergencyLocationId]))
  const bridges = new Map(rows('moqui.chat.ChatBridge').map((b) => [b.chatBridgeId, b.name || b.chatBridgeId]))
  return {
    numberList: numbers.map((n) => ({
      phoneNumber: n.phoneNumber, voiceTrunkId: n.voiceTrunkId, trunkName: trunks.get(n.voiceTrunkId) || null,
      routeTypeEnumId: n.routeTypeEnumId, routeId: n.routeId,
      routeName: n.routeTypeEnumId === 'PnrUser' && n.routeId ? userName(n.routeId)
        : n.routeTypeEnumId === 'PnrRingGroup' && n.routeId ? (one('moqui.call.RingGroup', { ringGroupId: n.routeId })?.name || n.routeId)
          : n.routeTypeEnumId === 'PnrIvr' && n.routeId ? (one('moqui.call.IvrMenu', { ivrMenuId: n.routeId })?.name || n.routeId)
            : n.routeTypeEnumId === 'PnrQueue' && n.routeId ? (one('moqui.call.CallQueue', { callQueueId: n.routeId })?.name || n.routeId)
          : n.routeTypeEnumId === 'PnrVoicemail' ? ec.l10n.localize('Voicemail access') : n.routeId,
      emergencyLocationId: n.emergencyLocationId, locationName: locations.get(n.emergencyLocationId) || null, description: n.description,
      messagingBridgeId: n.messagingBridgeId || null, messagingName: bridges.get(n.messagingBridgeId) || null,
    })),
  }
}

function storeNumber(p) {
  const denied = notAdmin()
  if (denied) return denied
  const pol = policy()
  const phoneNumber = toE164(p.phoneNumber, pol.homeCountryCode)
  if (!phoneNumber) return refuse('badNumber', 'That is not a phone number')
  if (!blank(p.voiceTrunkId) && !one(TRUNK, { voiceTrunkId: p.voiceTrunkId })) return refuse('notFound', 'Trunk not found')
  const routeTypeEnumId = blank(p.routeTypeEnumId) ? 'PnrUser' : p.routeTypeEnumId
  if (!['PnrUser', 'PnrRingGroup', 'PnrIvr', 'PnrQueue', 'PnrVoicemail'].includes(routeTypeEnumId)) {
    return refuse('notSupported', 'A number rings a user, a ring group, an IVR menu or a queue, or is the voicemail access number')
  }
  if (routeTypeEnumId === 'PnrIvr' && (blank(p.routeId) || !one('moqui.call.IvrMenu', { ivrMenuId: p.routeId }))) {
    return refuse('notFound', 'IVR menu not found')
  }
  if (routeTypeEnumId === 'PnrQueue' && (blank(p.routeId) || !one('moqui.call.CallQueue', { callQueueId: p.routeId }))) {
    return refuse('notFound', 'Queue not found')
  }
  if (routeTypeEnumId === 'PnrUser' && !blank(p.routeId) && !one('moqui.security.UserAccount', { userId: p.routeId })) {
    return refuse('notFound', 'User not found')
  }
  if (routeTypeEnumId === 'PnrRingGroup' && (blank(p.routeId) || !one('moqui.call.RingGroup', { ringGroupId: p.routeId }))) {
    return refuse('notFound', 'Ring group not found')
  }
  if (!blank(p.emergencyLocationId) && !one(LOCATION, { emergencyLocationId: p.emergencyLocationId })) return refuse('notFound', 'Emergency location not found')
  // C4-SMS: its texts' bridge, an SMS one
  const bridge = blank(p.messagingBridgeId) ? null : one('moqui.chat.ChatBridge', { chatBridgeId: p.messagingBridgeId })
  if (!blank(p.messagingBridgeId) && (!bridge || bridge.typeEnumId !== 'CbtSms')) return refuse('notFound', 'SMS bridge not found')
  const fields = { voiceTrunkId: p.voiceTrunkId || null, routeTypeEnumId, routeId: p.routeId || null,
    emergencyLocationId: p.emergencyLocationId || null, description: p.description || null }
  // left as it is when not given (a caller from before C4-SMS)
  if (p.messagingBridgeId !== undefined) fields.messagingBridgeId = blank(p.messagingBridgeId) ? null : p.messagingBridgeId
  const row = one(NUMBER, { phoneNumber })
  if (row) {
    row.setFields(fields, true, null, null)
    row.update()
  } else {
    const created = ec.entity.makeValue(NUMBER)
    created.setFields(Object.assign({ phoneNumber }, fields), true, null, null)
    created.create()
  }
  // its route or its bridge may have changed: who its texts reach
  if (row) syncPairRooms(phoneNumber)
  fallbackChanged(phoneNumber)
  return { phoneNumber }
}

function deleteNumber(p) {
  const denied = notAdmin()
  if (denied) return denied
  const row = one(NUMBER, { phoneNumber: p.phoneNumber })
  if (!row) return refuse('notFound', 'Number not found')
  find(GRANT).condition('phoneNumber', row.phoneNumber).deleteAll()
  syncPairRooms(row.phoneNumber, true)
  row.delete()
  fallbackChanged(row.phoneNumber)
  return {}
}

function findGrants() {
  const denied = notAdmin()
  if (denied) return denied
  const now = nowMs()
  return {
    grantList: Array.from(find(GRANT).orderBy('phoneNumber').orderBy('userId').list())
      .filter((g) => !g.thruDate || ms(g.thruDate) > now)
      .map((g) => ({ phoneNumber: g.phoneNumber, userId: g.userId, userName: userName(g.userId), fromDate: ms(g.fromDate) })),
  }
}

function grantCallerId(p) {
  const denied = notAdmin()
  if (denied) return denied
  const pol = policy()
  const phoneNumber = toE164(p.phoneNumber, pol.homeCountryCode)
  if (!phoneNumber || !one(NUMBER, { phoneNumber })) return refuse('notFound', 'Number not found')
  if (!one('moqui.security.UserAccount', { userId: p.userId })) return refuse('notFound', 'User not found')
  if (grantsOf(p.userId).includes(phoneNumber)) return { phoneNumber, repeated: true }
  const row = ec.entity.makeValue(GRANT)
  row.setFields({ phoneNumber, userId: p.userId, fromDate: ec.user.nowTimestamp }, true, null, null)
  row.create()
  return { phoneNumber }
}

function revokeCallerId(p) {
  const denied = notAdmin()
  if (denied) return denied
  const now = ec.user.nowTimestamp
  let revoked = 0
  for (const g of rows(GRANT, { phoneNumber: p.phoneNumber, userId: p.userId })) {
    if (g.thruDate && ms(g.thruDate) <= ms(now)) continue
    g.set('thruDate', now)
    g.update()
    revoked++
  }
  if (revoked) syncPairRooms(p.phoneNumber)
  return { revokedCount: revoked }
}

function findLocations() {
  const denied = notAdmin()
  if (denied) return denied
  return {
    locationList: Array.from(find(LOCATION).orderBy('name').list()).map((l) => {
      const missing = []
      if (l.validationStatusId !== 'ElvValidated') missing.push('locationNotValidated')
      if (blank(l.callbackNumber)) missing.push('noCallbackNumber')
      if (blank(l.notifyEmailAddress)) missing.push('noNotification')
      return Object.assign({}, Object.fromEntries(['emergencyLocationId', 'name', 'address1', 'address2', 'city', 'stateProvinceGeoId',
        'postalCode', 'countryGeoId', 'callbackNumber', 'validationStatusId', 'providerReference', 'notifyEmailAddress']
        .map((k) => [k, l[k] ?? null])), { validatedDate: ms(l.validatedDate), ready: !missing.length, missingText: missingText(missing) })
    }),
  }
}

function storeLocation(p) {
  const denied = notAdmin()
  if (denied) return denied
  const pol = policy()
  let row = blank(p.emergencyLocationId) ? null : one(LOCATION, { emergencyLocationId: p.emergencyLocationId })
  if (!blank(p.emergencyLocationId) && !row) return refuse('notFound', 'Emergency location not found')
  const fields = {}
  for (const key of ['name', 'address1', 'address2', 'city', 'stateProvinceGeoId', 'postalCode', 'countryGeoId', 'providerReference', 'notifyEmailAddress']) {
    if (p[key] !== undefined && p[key] !== null) fields[key] = String(p[key]).trim() || null
  }
  if (p.callbackNumber !== undefined && p.callbackNumber !== null) {
    if (blank(p.callbackNumber)) fields.callbackNumber = null
    else {
      fields.callbackNumber = toE164(p.callbackNumber, pol.homeCountryCode)
      if (!fields.callbackNumber) return refuse('badNumber', 'The callback number is not a phone number')
    }
  }
  if (fields.notifyEmailAddress && !/^[^\s@]+@[^\s@]+$/.test(fields.notifyEmailAddress)) return refuse('badEmail', 'The notification destination is an email address')
  if (!blank(p.validationStatusId)) {
    if (!['ElvUnvalidated', 'ElvValidated', 'ElvFailed'].includes(p.validationStatusId)) return refuse('badStatus', 'Choose a validation state')
    fields.validationStatusId = p.validationStatusId
    if (p.validationStatusId === 'ElvValidated' && (!row || row.validationStatusId !== 'ElvValidated')) fields.validatedDate = ec.user.nowTimestamp
  }
  // a changed address is no longer the one the provider validated
  if (row && !fields.validationStatusId && ['address1', 'address2', 'city', 'postalCode', 'stateProvinceGeoId', 'countryGeoId']
    .some((k) => k in fields && (fields[k] || null) !== (row[k] || null))) fields.validationStatusId = 'ElvUnvalidated'
  if (!row) {
    row = ec.entity.makeValue(LOCATION)
    row.setSequencedIdPrimary()
    row.setFields(Object.assign({ validationStatusId: 'ElvUnvalidated' }, fields), true, null, null)
    row.create()
  } else {
    row.setFields(fields, true, null, null)
    row.update()
  }
  return { emergencyLocationId: row.emergencyLocationId }
}

function deleteLocation(p) {
  const denied = notAdmin()
  if (denied) return denied
  const row = one(LOCATION, { emergencyLocationId: p.emergencyLocationId })
  if (!row) return refuse('notFound', 'Emergency location not found')
  if (find(NUMBER).condition('emergencyLocationId', row.emergencyLocationId).count() > 0 ||
    find(SETTING).condition('emergencyLocationId', row.emergencyLocationId).count() > 0) {
    return refuse('inUse', 'Numbers or users have this location; change them first')
  }
  row.delete()
  return {}
}

const POLICY_FIELDS = ['internationalEnabled', 'premiumEnabled', 'maxConcurrentCalls', 'maxConcurrentPerUser', 'maxCallsPerUserPerDay',
  'maxCallsPerDay', 'authFailureLimit', 'authFailureWindowSeconds', 'authBlockSeconds', 'homeCountryCode', 'emergencyNumbers',
  'premiumPrefixes', 'alertUserGroupId', 'smsMaxPerUserPerDay', 'smsMaxPerDay', 'smsSystemMaxPerDay', 'smsSystemNumber']

function getPolicy() {
  const denied = notAdmin()
  if (denied) return denied
  return { policy: policy() }
}

function updatePolicy(p) {
  const denied = notAdmin()
  if (denied) return denied
  const fields = {}
  for (const key of POLICY_FIELDS) {
    if (p[key] === undefined || p[key] === null || p[key] === '') continue
    if (key === 'internationalEnabled' || key === 'premiumEnabled') fields[key] = flag(p[key])
    else if (key === 'smsSystemNumber') {
      // C4-SMS: one of our numbers, with messaging
      const number = toE164(p[key], policy().homeCountryCode)
      const row = number ? one(NUMBER, { phoneNumber: number }) : null
      if (!row || blank(row.messagingBridgeId)) return refuse('badPolicy', 'The system texts from one of our numbers that has messaging')
      fields[key] = number
    } else if (/^(max|auth|smsMax)/.test(key)) {
      const number = Number(p[key])
      if (!(Number.isInteger(number) && number >= 1)) return refuse('badPolicy', 'Limits are whole numbers of 1 or more')
      fields[key] = number
    } else fields[key] = String(p[key]).trim()
  }
  if (fields.homeCountryCode && !/^[1-9]\d{0,2}$/.test(fields.homeCountryCode)) return refuse('badPolicy', 'The country code is 1 to 3 digits')
  const row = find(POLICY).condition('voicePolicyId', 'DEFAULT').forUpdate(true).one()
  if (row) {
    row.setFields(fields, true, null, null)
    row.update()
  } else {
    const created = ec.entity.makeValue(POLICY)
    created.setFields(Object.assign({ voicePolicyId: 'DEFAULT' }, DEFAULT_POLICY, fields), true, null, null)
    created.create()
  }
  reloadEngines()
  return { policy: policy() }
}

function findUserSettings() {
  const denied = notAdmin()
  if (denied) return denied
  return {
    settingList: Array.from(find(SETTING).orderBy('userId').list()).map((s) => ({
      userId: s.userId, userName: userName(s.userId), outboundEnabled: s.outboundEnabled || 'N',
      internationalEnabled: s.internationalEnabled || 'N', premiumEnabled: s.premiumEnabled || 'N', forwardEnabled: s.forwardEnabled || 'N',
      emergencyLocationId: s.emergencyLocationId || null, maxConcurrent: s.maxConcurrent ?? null, maxPerDay: s.maxPerDay ?? null,
    })),
  }
}

function storeUserSetting(p) {
  const denied = notAdmin()
  if (denied) return denied
  if (!one('moqui.security.UserAccount', { userId: p.userId })) return refuse('notFound', 'User not found')
  if (!blank(p.emergencyLocationId) && !one(LOCATION, { emergencyLocationId: p.emergencyLocationId })) return refuse('notFound', 'Emergency location not found')
  const fields = {}
  for (const key of ['outboundEnabled', 'internationalEnabled', 'premiumEnabled', 'forwardEnabled']) if (p[key] !== undefined && p[key] !== null) fields[key] = flag(p[key])
  for (const key of ['maxConcurrent', 'maxPerDay']) {
    if (p[key] === undefined || p[key] === null) continue
    if (p[key] === '') fields[key] = null
    else {
      const number = Number(p[key])
      if (!(Number.isInteger(number) && number >= 0)) return refuse('badSetting', 'Limits are whole numbers')
      fields[key] = number
    }
  }
  if (p.emergencyLocationId !== undefined && p.emergencyLocationId !== null) fields.emergencyLocationId = p.emergencyLocationId || null
  const row = one(SETTING, { userId: p.userId })
  if (row) {
    row.setFields(fields, true, null, null)
    row.update()
  } else {
    const created = ec.entity.makeValue(SETTING)
    created.setFields(Object.assign({ userId: p.userId, outboundEnabled: 'N', internationalEnabled: 'N', premiumEnabled: 'N', forwardEnabled: 'N' }, fields), true, null, null)
    created.create()
  }
  return { userId: p.userId }
}

function findAuthBlocks() {
  const denied = notAdmin()
  if (denied) return denied
  const now = nowMs()
  return {
    blockList: Array.from(find(AUTH_FAILURE).orderBy('-lastFailureDate').limit(200).list()).map((row) => ({
      sourceAddress: row.sourceAddress, failureCount: Number(row.failureCount || 0), lastUsername: row.lastUsername || null,
      lastFailureDate: ms(row.lastFailureDate), blockedUntil: ms(row.blockedUntilDate), blocked: ms(row.blockedUntilDate) > now,
    })),
  }
}

function clearAuthBlock(p) {
  const denied = notAdmin()
  if (denied) return denied
  const row = one(AUTH_FAILURE, { sourceAddress: p.sourceAddress })
  if (!row) return refuse('notFound', 'That source has no failures')
  row.delete()
  reloadEngines()
  return {}
}

/** The voice status for the admin screen: the capabilities, the SIP and media nodes. */
function status() {
  const denied = notAdmin()
  if (denied) return denied
  const now = nowMs()
  const live = (row) => ms(row.heartbeatDate) >= now - NODE_STALE_MS
  return {
    capabilities: capabilities(),
    sipNodeList: rows(SIP_NODE).map((n) => ({ instanceId: n.instanceId, sipAddress: n.sipAddress, tlsAddress: n.tlsAddress,
      rtpPorts: n.rtpPorts, heartbeatDate: ms(n.heartbeatDate), live: live(n) })),
    mediaNodeList: rows(MEDIA_NODE).map((n) => ({ instanceId: n.instanceId, mediaAddress: n.mediaAddress,
      heartbeatDate: ms(n.heartbeatDate), live: live(n), draining: n.draining === 'Y' })),
    activePhoneCalls: countPhoneCalls({}),
  }
}

const operations = {
  place: (p) => place(p), receive, noteLeg, notifyPhoneLeg, sendDtmf, hold: (p) => holdOrResume(p, 'hold'), resume: (p) => holdOrResume(p, 'resume'),
  // a registered phone's call (SipDevices.js receiveDeviceCall), on its own SIP engine's node; not a service
  placeFromDevice: (p) => place(p, p.instanceId || null),
  myCallerIds, registerNode, refreshFallbacks, fallbackChanged: (p) => { fallbackChanged(p.phoneNumber || null); return {} }, syncPairRooms: (p) => { syncPairRooms(p.phoneNumber, p.deleted === true); return {} },
  // V3 (CallHandling.js): forwards, and the dialer's country code
  forwardCheck, dialForward, grants: (p) => ({ numberList: grantsOf(p.userId) }), homeCountry: () => ({ homeCountryCode: policy().homeCountryCode }), engineConfig,
  // C4-SMS (Sms.js): the policy, and a number's class and E.164 form by the same rules as a call's
  policy: () => policy(), classify: (p) => classify(p.phoneNumber, policy()) || {}, toE164: (p) => ({ number: toE164(p.phoneNumber, policy().homeCountryCode) }), trunkStatus, authFailure, capabilities, status, findTrunks, storeTrunk,
  deleteTrunk, findNumbers, storeNumber, deleteNumber, findGrants, grantCallerId, revokeCallerId, findLocations,
  storeLocation, deleteLocation, getPolicy, updatePolicy, findUserSettings, storeUserSetting, findAuthBlocks,
  clearAuthBlock,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown voice operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
