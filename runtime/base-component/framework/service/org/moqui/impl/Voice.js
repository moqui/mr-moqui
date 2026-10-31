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
    'authFailureLimit', 'authFailureWindowSeconds', 'authBlockSeconds']) out[key] = Number(out[key])
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

/** The numbers the user may present now. */
function grantsOf(userId) {
  if (!userId) return []
  const now = ec.user.nowTimestamp
  return rows(GRANT, { userId }).filter((g) => ms(g.fromDate) <= ms(now) && (!g.thruDate || ms(g.thruDate) > ms(now)))
    .map((g) => g.phoneNumber)
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

function countPhoneCalls(fields, sinceMs) {
  let f = find(SESSION).condition('kindEnumId', 'CskPhone')
  for (const [k, v] of Object.entries(fields)) f = f.condition(k, v)
  if (sinceMs != null) f = f.condition('startedDate', '>=', new Date(sinceMs))
  else f = f.condition('statusId', '!=', 'CssEnded')
  return f.count()
}

function dial(started, trunk, dest, fromNumber, fromName, userId, emergency, location) {
  sipControl(started.mediaNodeId, {
    action: 'dial', callSessionId: started.callSessionId, callLegId: started.phoneLegId, voiceTrunkId: trunk.voiceTrunkId,
    toNumber: dest.number, fromNumber, fromName, destinationClass: dest.destinationClass, emergency: !!emergency,
    callbackNumber: location ? location.callbackNumber : null, locationReference: location ? location.providerReference || null : null,
    userId,
  })
}

function userName(userId) {
  const account = one('moqui.security.UserAccount', { userId })
  return account ? account.userFullName || account.username || userId : userId
}

function place(p) {
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
    const nodes = voiceNodes()
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
  // a user's own setting can lower the policy's limits, never raise them
  const perUser = blank(setting.maxConcurrent) ? pol.maxConcurrentPerUser : Math.min(Number(setting.maxConcurrent), pol.maxConcurrentPerUser)
  if (countPhoneCalls({ startedByUserId: me }) >= perUser || countPhoneCalls({}) >= pol.maxConcurrentCalls) {
    return refuse('tooManyCalls', 'Too many phone calls at once; try again when one ends')
  }
  const dayStart = nowMs() - 86400000
  const perDay = blank(setting.maxPerDay) ? pol.maxCallsPerUserPerDay : Math.min(Number(setting.maxPerDay), pol.maxCallsPerUserPerDay)
  if (countPhoneCalls({ startedByUserId: me, directionEnumId: 'CdrOutbound' }, dayStart) >= perDay ||
    countPhoneCalls({ directionEnumId: 'CdrOutbound' }, dayStart) >= pol.maxCallsPerDay) {
    return refuse('dailyLimit', 'The limit of phone calls for a day is reached')
  }
  const number = one(NUMBER, { phoneNumber: fromNumber })
  const trunk = number?.voiceTrunkId ? one(TRUNK, { voiceTrunkId: number.voiceTrunkId }) : null
  if (!trunk || trunk.disabled === 'Y') return refuse('noTrunk', 'Your number has no trunk to call out through')
  const nodes = voiceNodes()
  if (!nodes.length) return refuse('voiceUnavailable', 'Phone calls are not available: no SIP server is running')
  const started = calls('startPhone', { userId: me, direction: 'outbound', phoneNumber: dest.number, displayName: dest.number,
    mediaNodeId: nodes[0].instanceId, idempotencyKey: key })
  started.mediaNodeId = nodes[0].instanceId
  dial(started, trunk, dest, fromNumber, userName(me), me, false, null)
  return { callSessionId: started.callSessionId, callLegId: started.callLegId, phoneLegId: started.phoneLegId, call: started.call,
    voiceTrunkId: trunk.voiceTrunkId, fromNumber, destinationClass: dest.destinationClass }
}

function receive(p) {
  const pol = policy()
  const to = toE164(p.toNumber, pol.homeCountryCode)
  const number = to ? one(NUMBER, { phoneNumber: to }) : null
  if (!number || (p.voiceTrunkId && number.voiceTrunkId && number.voiceTrunkId !== p.voiceTrunkId)) return { reasonCode: 'notFound' }
  if (number.routeTypeEnumId !== 'PnrUser' || blank(number.routeId)) return { reasonCode: 'notFound' }
  const account = one('moqui.security.UserAccount', { userId: number.routeId })
  if (!account || account.disabled === 'Y') return { reasonCode: 'notFound' }
  if (!voiceNodes().some((n) => n.instanceId === p.instanceId)) return { reasonCode: 'voiceUnavailable' }
  if (countPhoneCalls({}) >= pol.maxConcurrentCalls) return { reasonCode: 'busy' }
  const from = toE164(p.fromNumber, pol.homeCountryCode) || cleanNumber(p.fromNumber) || ec.l10n.localize('Unknown number')
  const name = blank(p.fromName) || p.fromName === from ? from : String(p.fromName).slice(0, 60) + ' (' + from + ')'
  const started = calls('startPhone', { userId: number.routeId, direction: 'inbound', phoneNumber: from, displayName: name,
    mediaNodeId: p.instanceId })
  return { callSessionId: started.callSessionId, callLegId: started.phoneLegId }
}

function noteLeg(p) {
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
  sipControl(session.mediaNodeId, { action: 'prompt', notice: p.notice, callSessionId: session.callSessionId, callLegId: leg.callLegId })
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
  for (const leg of mine.legs) sipControl(mine.session.mediaNodeId, { action: 'dtmf', callSessionId: p.callSessionId, callLegId: leg.callLegId, digits })
  return { sent: true }
}

function holdOrResume(p, action) {
  const mine = myPhoneLegs(p.callSessionId)
  if (mine.refusal) return mine.refusal
  for (const leg of mine.legs) sipControl(mine.session.mediaNodeId, { action, callSessionId: p.callSessionId, callLegId: leg.callLegId })
  return {}
}

/** The numbers the current user may call from, for the caller-ID choice. */
function myCallerIds() {
  return { numberList: grantsOf(ec.user.userId).map((phoneNumber) => ({ phoneNumber, description: one(NUMBER, { phoneNumber })?.description || null })) }
}

// ---------- the SIP engine ----------

function registerNode(p) {
  const row = one(SIP_NODE, { instanceId: p.instanceId })
  const now = ec.user.nowTimestamp
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
    created.setFields(Object.assign({ instanceId: p.instanceId, startedDate: now }, fields), true, null, null)
    created.create()
  }
  return {}
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
  return {
    numberList: numbers.map((n) => ({
      phoneNumber: n.phoneNumber, voiceTrunkId: n.voiceTrunkId, trunkName: trunks.get(n.voiceTrunkId) || null,
      routeTypeEnumId: n.routeTypeEnumId, routeId: n.routeId, routeName: n.routeTypeEnumId === 'PnrUser' && n.routeId ? userName(n.routeId) : n.routeId,
      emergencyLocationId: n.emergencyLocationId, locationName: locations.get(n.emergencyLocationId) || null, description: n.description,
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
  if (routeTypeEnumId !== 'PnrUser') return refuse('notSupported', 'A number rings a user for now')
  if (!blank(p.routeId) && !one('moqui.security.UserAccount', { userId: p.routeId })) return refuse('notFound', 'User not found')
  if (!blank(p.emergencyLocationId) && !one(LOCATION, { emergencyLocationId: p.emergencyLocationId })) return refuse('notFound', 'Emergency location not found')
  const fields = { voiceTrunkId: p.voiceTrunkId || null, routeTypeEnumId, routeId: p.routeId || null,
    emergencyLocationId: p.emergencyLocationId || null, description: p.description || null }
  const row = one(NUMBER, { phoneNumber })
  if (row) {
    row.setFields(fields, true, null, null)
    row.update()
  } else {
    const created = ec.entity.makeValue(NUMBER)
    created.setFields(Object.assign({ phoneNumber }, fields), true, null, null)
    created.create()
  }
  return { phoneNumber }
}

function deleteNumber(p) {
  const denied = notAdmin()
  if (denied) return denied
  const row = one(NUMBER, { phoneNumber: p.phoneNumber })
  if (!row) return refuse('notFound', 'Number not found')
  find(GRANT).condition('phoneNumber', row.phoneNumber).deleteAll()
  row.delete()
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
  'premiumPrefixes', 'alertUserGroupId']

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
    else if (/^(max|auth)/.test(key)) {
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
      internationalEnabled: s.internationalEnabled || 'N', premiumEnabled: s.premiumEnabled || 'N',
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
  for (const key of ['outboundEnabled', 'internationalEnabled', 'premiumEnabled']) if (p[key] !== undefined && p[key] !== null) fields[key] = flag(p[key])
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
    created.setFields(Object.assign({ userId: p.userId, outboundEnabled: 'N', internationalEnabled: 'N', premiumEnabled: 'N' }, fields), true, null, null)
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
  place, receive, noteLeg, notifyPhoneLeg, sendDtmf, hold: (p) => holdOrResume(p, 'hold'), resume: (p) => holdOrResume(p, 'resume'),
  myCallerIds, registerNode, engineConfig, trunkStatus, authFailure, capabilities, status, findTrunks, storeTrunk,
  deleteTrunk, findNumbers, storeNumber, deleteNumber, findGrants, grantCallerId, revokeCallerId, findLocations,
  storeLocation, deleteLocation, getPolicy, updatePolicy, findUserSettings, storeUserSetting, findAuthBlocks,
  clearAuthBlock,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown voice operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
