// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// SMS and MMS (plans/Comms-Framework.md §5.6, §5.4 "One conversation per outside number", C4-SMS): the services of
// org.moqui.impl.SmsServices and org.moqui.SmsServices.send#SmsMessage. A number's texts go through its messagingBridgeId
// (a ChatBridge whose adapter is the provider's: Telnyx in crates/moqui-channels) and land in the number pair's external room:
// one ChatRoom (CrtExternal) for our number and their number, mapped to the bridge by a ChatRoomBridge whose externalRoomId
// is "ours/theirs". Its members are the users our number reaches: the user it routes to, a ring group's members, and the
// users granted it as a caller ID (Voice.js grantsOf). Calls and voicemail of the pair link to the same room (Calls.js
// startPhone with chatRoomId, Voicemail.js store).
//
// Sending. A text a user writes in the room goes out through the chat delivery layer; the adapter asks check#SmsSend before
// the provider: the sender must be granted our number, international and premium destinations must be enabled for the
// deployment and the user (VoicePolicy, VoiceUserSetting, as for calls), the person must not have opted out (STOP), and the
// day's limits (VoicePolicy smsMaxPerUserPerDay and smsMaxPerDay, counted in SmsSendCount) must allow it. The system's own
// texts (a sign-in code, send#SmsMessage) go from VoicePolicy.smsSystemNumber straight to the provider (send#BridgeDirect)
// with the same checks, and are kept in no room. A promotional text needs a yes from an sms-consent provider (Mantle's
// consent records); with none it is refused.

const VOICE = 'component://framework/service/org/moqui/impl/Voice.js'
const NUMBER = 'moqui.call.PhoneNumber'
const GRANT = 'moqui.call.PhoneNumberUser'
const OPT_OUT = 'moqui.call.SmsOptOut'
const COUNT = 'moqui.call.SmsSendCount'
const POLICY = 'moqui.call.VoicePolicy'
const SETTING = 'moqui.call.VoiceUserSetting'
const ROOM = 'moqui.chat.ChatRoom'
const MEMBER = 'moqui.chat.ChatRoomMember'
const MAPPING = 'moqui.chat.ChatRoomBridge'
const BRIDGE = 'moqui.chat.ChatBridge'

/** The keywords a carrier's rules give a person: STOP and its synonyms opt out of the pair, START opts back in. */
const STOP_WORDS = ['STOP', 'STOPALL', 'UNSUBSCRIBE', 'CANCEL', 'END', 'QUIT', 'OPTOUT', 'REVOKE']
const START_WORDS = ['START', 'UNSTOP', 'YES', 'OPTIN']
const HELP_WORDS = ['HELP', 'INFO']
const DEFAULT_LIMITS = { smsMaxPerUserPerDay: 200, smsMaxPerDay: 2000 }

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

function voice(op, parameters) {
  return ec.resource.script(VOICE).execute(op, parameters || {}) || {}
}

function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

/** The external room id of a pair. */
function pairKey(ours, theirs) {
  return ours + '/' + theirs
}

/** The deployment's SMS limits and system number, the defaults filling what VoicePolicy leaves empty. */
function smsPolicy() {
  const row = one(POLICY, { voicePolicyId: 'DEFAULT' }) || {}
  const out = {}
  for (const [key, value] of Object.entries(DEFAULT_LIMITS)) out[key] = blank(row[key]) ? value : Number(row[key])
  out.smsSystemNumber = row.smsSystemNumber || null
  return out
}

/** The users a number of ours reaches: its user or ring group, and the users granted it now. */
function usersOf(number) {
  const users = new Set()
  if (number.routeTypeEnumId === 'PnrUser' && number.routeId) users.add(number.routeId)
  if (number.routeTypeEnumId === 'PnrRingGroup' && number.routeId) {
    for (const member of rows('moqui.call.RingGroupMember', { ringGroupId: number.routeId })) if (member.userId) users.add(member.userId)
  }
  const now = nowMs()
  for (const grant of rows(GRANT, { phoneNumber: number.phoneNumber })) {
    if (ms(grant.fromDate) <= now && (!grant.thruDate || ms(grant.thruDate) > now)) users.add(grant.userId)
  }
  const enabled = users.size ? new Set(Array.from(find('moqui.security.UserAccount').condition('userId', 'in', Array.from(users))
    .condition(ec.entity.conditionFactory.makeCondition('disabled', 'not-equals', 'Y', true)).selectField('userId').list())
    .map((row) => row.userId)) : new Set()
  return Array.from(users).filter((userId) => enabled.has(userId))
}

/** The numbers the user may text from: their caller-ID grants (with a ring group's numbers) that have a messaging bridge. */
function textNumbersOf(userId) {
  const granted = Array.from(voice('grants', { userId }).numberList || [])
  if (!granted.length) return []
  return Array.from(find(NUMBER).condition('phoneNumber', 'in', granted).orderBy('phoneNumber').list())
    .filter((n) => !blank(n.messagingBridgeId))
}

// ---------- the pair room ----------

/**
 * Internal: the room of the pair (ourNumber, theirNumber), with its mapping to our number's messaging bridge, found or made,
 * and every user our number reaches a member of it (one who left stays out). Run in a transaction of its own before a text
 * is taken in (ingest#Event refuses an unmapped room) and before a call or a send links to it.
 */
function ensurePairRoom(p) {
  const ours = p.ourNumber
  const theirs = p.theirNumber
  if (blank(ours) || blank(theirs)) return refuse('badNumber', 'A conversation is between two numbers')
  // the number's row lock: two first texts of a pair at once make one room
  const number = find(NUMBER).condition('phoneNumber', ours).forUpdate(true).one()
  if (!number || blank(number.messagingBridgeId)) return { reasonCode: 'noMessaging' }
  if (!one(BRIDGE, { chatBridgeId: number.messagingBridgeId })) return { reasonCode: 'noMessaging' }
  const chatBridgeId = number.messagingBridgeId
  const externalRoomId = pairKey(ours, theirs)
  const now = ec.user.nowTimestamp
  let mapping = Array.from(find(MAPPING).condition('chatBridgeId', chatBridgeId).condition('externalRoomId', externalRoomId)
    .condition('thruDate', null).list())[0]
  let room = mapping ? one(ROOM, { chatRoomId: mapping.chatRoomId }) : null
  let created = false
  if (!room) {
    room = ec.entity.makeValue(ROOM)
    room.setSequencedIdPrimary()
    room.setFields({ roomTypeEnumId: 'CrtExternal', roomName: theirs, topic: ec.resource.expand('Texts and calls with ${theirs} on ${ours}', '',
      { theirs, ours }), visibilityEnumId: 'CrvMembers', createdDate: now, lastEventSeq: 0, lastEventDate: now }, true, null, null)
    room.create()
    // a mapping whose room is gone ends, and the pair gets a new one
    if (mapping) {
      mapping.setFields({ thruDate: now }, true, null, null)
      mapping.update()
    }
    mapping = ec.entity.makeValue(MAPPING)
    mapping.setSequencedIdPrimary()
    mapping.setFields({ chatRoomId: room.chatRoomId, chatBridgeId, externalRoomId, externalRoomName: theirs,
      directionEnumId: 'CrbBoth', fromDate: now }, true, null, null)
    mapping.create()
    created = true
  }
  const users = usersOf(number)
  const present = new Set(rows(MEMBER, { chatRoomId: room.chatRoomId }).map((m) => m.userId).filter((u) => u))
  const missing = users.filter((userId) => !present.has(userId))
  if (missing.length) {
    const actorOf = ec.service.sync().name('org.moqui.impl.ChatServices.get#LocalActors').parameter('userIdList', missing)
      .disableAuthz().call()?.actorMap || {}
    for (const userId of missing) {
      const member = ec.entity.makeValue(MEMBER)
      member.setFields({ chatRoomId: room.chatRoomId, chatActorId: actorOf[userId], userId, roleEnumId: 'CmrMember', joinedDate: now,
        lastReadEventSeq: Number(room.lastEventSeq || 0), notificationLevelEnumId: 'CnlAll', isPinned: 'N', isMuted: 'N' }, true, null, null)
      member.create()
      ec.service.sync().name('org.moqui.impl.ChatServices.push#Signal').parameters({ topic: 'chatUser:' + userId, userIdList: [userId],
        message: { kind: 'room', action: 'joined', chatRoomId: room.chatRoomId } }).disableAuthz().call()
    }
  }
  return { chatRoomId: room.chatRoomId, chatBridgeId, chatRoomBridgeId: mapping.chatRoomBridgeId, externalRoomId, created }
}

/**
 * Internal (Voice.js and CallHandling.js, in the caller's transaction, when a grant ends or a number's route, bridge or ring
 * group changes): the users no longer reached by phoneNumber are taken out of its pair rooms, so the pair's texts are not
 * seen by them from then on. Their rows go (not a leftDate), so one reached again is put back by ensure#PairRoom. Users
 * newly reached join at the pair's next text or call. Out: removedCount.
 */
function syncNumberRooms(p) {
  const phoneNumber = p.phoneNumber
  if (blank(phoneNumber)) return { removedCount: 0 }
  const number = one(NUMBER, { phoneNumber })
  const users = new Set(number && !blank(number.messagingBridgeId) && !p.deleted ? usersOf(number) : [])
  const mappings = Array.from(find(MAPPING).condition('externalRoomId', 'like', phoneNumber + '/%').list())
  const roomIds = Array.from(new Set(mappings.filter((m) => m.externalRoomId.startsWith(phoneNumber + '/')).map((m) => m.chatRoomId)))
  let removedCount = 0
  const removedUsers = new Set()
  for (const chatRoomId of roomIds) {
    const room = one(ROOM, { chatRoomId })
    if (!room || room.roomTypeEnumId !== 'CrtExternal') continue
    for (const member of rows(MEMBER, { chatRoomId })) {
      if (!member.userId || users.has(member.userId)) continue
      member.delete()
      removedCount++
      removedUsers.add(member.userId)
      ec.service.sync().name('org.moqui.impl.ChatServices.push#Signal').parameters({ topic: 'chatUser:' + member.userId, userIdList: [member.userId],
        message: { kind: 'room', action: 'left', chatRoomId } }).disableAuthz().call()
    }
  }
  if (removedUsers.size) {
    ec.service.sync().name('org.moqui.impl.ChatServices.publish#ChatCounts').parameter('userIdList', Array.from(removedUsers)).disableAuthz().call()
  }
  return { removedCount }
}

/** The pair room of a call or a voicemail (Voice.js, CallHandling.js), in a transaction of its own; null when our number has no messaging. */
function pairRoomFor(p) {
  if (blank(p.ourNumber) || blank(p.theirNumber)) return {}
  const number = one(NUMBER, { phoneNumber: p.ourNumber })
  if (!number || blank(number.messagingBridgeId)) return {}
  ec.message.pushErrors()
  try {
    const out = ec.service.sync().name('org.moqui.impl.SmsServices.ensure#PairRoom').parameters({ ourNumber: p.ourNumber,
      theirNumber: p.theirNumber }).requireNewTransaction(true).disableAuthz().call() || {}
    if (ec.message.hasError()) throw new Error(ec.message.errorsString)
    return { chatRoomId: out.chatRoomId || null }
  } catch (err) {
    // the call goes on without its room
    ec.logger.warn(`The room of ${p.ourNumber} and ${p.theirNumber} was not made: ${err && err.message ? err.message : err}`)
    return {}
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
}

/**
 * Text a number (the user's): the pair room of fromNumber (one of the user's numbers with messaging; their only one when
 * empty) and toNumber, which the user is then in. Refused as a send would be: badNumber, emergency, numberNotGranted,
 * noMessaging, internationalNotEnabled, premiumNotEnabled.
 */
function openPairRoom(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in to send a text')
  const dest = voice('classify', { phoneNumber: p.toNumber })
  if (!dest.number) return refuse('badNumber', 'That is not a phone number')
  if (dest.destinationClass === 'emergency') return refuse('emergency', 'Texts to emergency numbers are not sent from here; call them')
  const numbers = textNumbersOf(me).map((n) => n.phoneNumber)
  const fromNumber = blank(p.fromNumber) ? (numbers.length === 1 ? numbers[0] : null) : voice('toE164', { phoneNumber: p.fromNumber }).number
  if (!fromNumber || !numbers.includes(fromNumber)) {
    return refuse(numbers.length ? 'numberNotGranted' : 'noMessaging', numbers.length ? 'Choose one of your numbers to text from'
      : 'You have no number to text from; an administrator can give you one')
  }
  const classRefusal = destinationRefusal(dest, me)
  if (classRefusal) return refuse(classRefusal.reasonCode, classRefusal.text)
  const out = ec.service.sync().name('org.moqui.impl.SmsServices.ensure#PairRoom').parameters({ ourNumber: fromNumber,
    theirNumber: dest.number }).requireNewTransaction(true).disableAuthz().call() || {}
  if (ec.message.hasError()) return {}
  if (!out.chatRoomId) return refuse(out.reasonCode || 'noMessaging', 'That number has no messaging')
  // back in it when they left
  const mine = one(MEMBER, { chatRoomId: out.chatRoomId, userId: me })
  if (mine && mine.leftDate) {
    mine.set('leftDate', null)
    mine.update()
  }
  return { chatRoomId: out.chatRoomId, ourNumber: fromNumber, theirNumber: dest.number }
}

// ---------- the checks before the provider ----------

/** The refusal of a destination class: international and premium need the deployment's and the user's (not the system's) yes. */
function destinationRefusal(dest, userId) {
  const pol = voice('policy')
  const setting = userId ? one(SETTING, { userId }) || {} : null
  const allowed = (key) => pol[key] === 'Y' && (!setting || setting[key] === 'Y')
  if (dest.destinationClass === 'international' && !allowed('internationalEnabled')) {
    return { reasonCode: 'internationalNotEnabled', text: 'Texts to international numbers are not enabled' }
  }
  if (dest.destinationClass === 'premium' && !allowed('premiumEnabled')) {
    return { reasonCode: 'premiumNotEnabled', text: 'Texts to premium numbers are not enabled' }
  }
  return null
}

function today() {
  return new Date(nowMs()).toISOString().slice(0, 10)
}

function sentToday(counterKey, day) {
  const row = one(COUNT, { counterKey, dayDate: day || today() })
  return row ? Number(row.sentCount || 0) : 0
}

/** Whether a promotional text may go: an sms-consent provider says yes, and none says no. */
function consented(fromNumber, toNumber) {
  ec.message.pushErrors()
  try {
    const out = ec.service.sync().name('org.moqui.impl.ShellServices.call#Providers').parameters({ hook: 'sms-consent',
      parameters: { fromNumber, toNumber } }).call() || {}
    const answers = Array.from(out.results || [])
    return out.unavailable !== true && answers.some((answer) => answer && answer.consent === true)
      && !answers.some((answer) => answer && answer.consent === false)
  } catch (err) {
    ec.logger.warn(`The sms-consent providers failed for ${toNumber}: ${err && err.message ? err.message : err}`)
    return false
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
}

/**
 * Internal (the adapter, before the provider; send#SmsMessage): whether a text from fromNumber to toNumber may go now, sent
 * by userId (none: the system). allowed, or reasonCode and message: badNumber, emergency, numberNotGranted, noMessaging,
 * internationalNotEnabled, premiumNotEnabled, optedOut, consentRequired, dailyLimit.
 */
function checkSend(p) {
  const no = (reasonCode, text) => ({ allowed: false, reasonCode, message: ec.l10n.localize(text) })
  const from = voice('toE164', { phoneNumber: p.fromNumber }).number
  const number = from ? one(NUMBER, { phoneNumber: from }) : null
  if (!number || blank(number.messagingBridgeId)) return no('noMessaging', 'That number has no messaging')
  const dest = voice('classify', { phoneNumber: p.toNumber })
  if (!dest.number) return no('badNumber', 'That is not a phone number')
  if (dest.destinationClass === 'emergency') return no('emergency', 'Texts to emergency numbers are not sent from here; call them')
  const userId = blank(p.userId) ? null : p.userId
  if (userId && !Array.from(voice('grants', { userId }).numberList || []).includes(from)) {
    return no('numberNotGranted', 'You may not text from that number')
  }
  const classRefusal = destinationRefusal(dest, userId)
  if (classRefusal) return no(classRefusal.reasonCode, classRefusal.text)
  const optOut = one(OPT_OUT, { phoneNumber: from, otherNumber: dest.number })
  if (optOut && optOut.optedOut === 'Y') return no('optedOut', 'This person replied STOP; texts to them are not sent until they reply START')
  if (p.isPromotional === true || p.isPromotional === 'true' || p.isPromotional === 'Y') {
    if (!consented(from, dest.number)) return no('consentRequired', 'A promotional text needs the person\'s consent, and none is recorded')
  }
  const limits = smsPolicy()
  const reserve = p.reserve === true || p.reserve === 'true' || p.reserve === 'Y'
  const userKey = userId ? 'user:' + userId : 'system'
  const day = today()
  // reserve: the check and the count are one step, under the lock of the day's deployment count (made at 0 when there is
  // none yet), so sends at once cannot pass a limit together; the caller lets the count go (release#SmsSend) when the text
  // does not reach the provider
  if (reserve && !find(COUNT).condition({ counterKey: 'all', dayDate: day }).forUpdate(true).one()) {
    const first = ec.entity.makeValue(COUNT)
    first.setFields({ counterKey: 'all', dayDate: day, sentCount: 0 }, true, null, null)
    first.create()
  }
  if (sentToday(userKey, day) >= (userId ? limits.smsMaxPerUserPerDay : limits.smsMaxPerDay) || sentToday('all', day) >= limits.smsMaxPerDay) {
    return no('dailyLimit', 'The limit of texts for a day is reached')
  }
  if (reserve) countSent(userKey, day, 1)
  return { allowed: true, ourNumber: from, theirNumber: dest.number, chatBridgeId: number.messagingBridgeId, destinationClass: dest.destinationClass,
    counterDay: reserve ? day : null }
}

/** Add delta to the day's counts of counterKey and of the deployment (all), never below 0. */
function countSent(userKey, day, delta) {
  for (const counterKey of [userKey, 'all']) {
    const row = find(COUNT).condition({ counterKey, dayDate: day }).forUpdate(true).one()
    if (row) {
      row.setFields({ sentCount: Math.max(0, Number(row.sentCount || 0) + delta), lastSentDate: ec.user.nowTimestamp }, true, null, null)
      row.update()
    } else if (delta > 0) {
      const created = ec.entity.makeValue(COUNT)
      created.setFields({ counterKey, dayDate: day, sentCount: delta, lastSentDate: ec.user.nowTimestamp }, true, null, null)
      created.create()
    }
  }
}

/** Internal: a text went to the provider (taken, or not known): it counts for the sender's and the deployment's day. */
function noteSent(p) {
  countSent(blank(p.userId) ? 'system' : 'user:' + p.userId, today(), 1)
  return {}
}

/** Internal: a text check#SmsSend reserved (counted) on counterDay did not reach the provider: it no longer counts. */
function releaseSend(p) {
  countSent(blank(p.userId) ? 'system' : 'user:' + p.userId, blank(p.counterDay) ? today() : String(p.counterDay), -1)
  return {}
}

// ---------- texts that come in ----------

/** The keyword of a text: STOP, START or HELP (and their synonyms), as the whole text. */
function keywordOf(text) {
  const word = String(text || '').trim().toUpperCase().replace(/[.!]+$/, '')
  if (STOP_WORDS.includes(word)) return 'STOP'
  if (START_WORDS.includes(word)) return 'START'
  if (HELP_WORDS.includes(word)) return 'HELP'
  return null
}

/** Internal: keep a pair's opt-out after a keyword (in a transaction of its own). */
function noteKeyword(p) {
  const keyword = p.keyword
  if (!['STOP', 'START', 'HELP'].includes(keyword)) return {}
  const row = find(OPT_OUT).condition({ phoneNumber: p.ourNumber, otherNumber: p.theirNumber }).forUpdate(true).one()
  const fields = { lastKeyword: keyword, changedDate: ec.user.nowTimestamp }
  if (keyword !== 'HELP') fields.optedOut = keyword === 'STOP' ? 'Y' : 'N'
  if (row) {
    row.setFields(fields, true, null, null)
    row.update()
  } else {
    const created = ec.entity.makeValue(OPT_OUT)
    created.setFields(Object.assign({ phoneNumber: p.ourNumber, otherNumber: p.theirNumber, optedOut: 'N' }, fields), true, null, null)
    created.create()
  }
  return {}
}

/**
 * Internal (the adapter's webhook, after it checked the provider's signature): a text from theirNumber to ourNumber.
 * ourNumber must have messaging through chatBridgeId (else outcome ignored, reasonCode unknownNumber). A STOP, START or HELP
 * keeps the pair's opt-out. The pair room is found or made, then the text goes through ingest#Event as the provider's
 * transaction eventId (a repeated webhook does nothing again): outcome, chatRoomId, eventSeq.
 */
function receive(p) {
  const ours = voice('toE164', { phoneNumber: p.ourNumber }).number
  const theirs = voice('toE164', { phoneNumber: p.theirNumber }).number || String(p.theirNumber || '').replace(/[^0-9+]/g, '').slice(0, 20)
  const number = ours ? one(NUMBER, { phoneNumber: ours }) : null
  if (!number || number.messagingBridgeId !== p.chatBridgeId) return { outcome: 'ignored', reasonCode: 'unknownNumber' }
  if (!theirs) return { outcome: 'ignored', reasonCode: 'badNumber' }
  const keyword = keywordOf(p.text)
  const sub = (name, parameters) => {
    const out = ec.service.sync().name(name).parameters(parameters).requireNewTransaction(true).disableAuthz().call() || {}
    if (ec.message.hasError()) throw new Error(ec.message.errorsString)
    return out
  }
  if (keyword) sub('org.moqui.impl.SmsServices.note#SmsKeyword', { ourNumber: ours, theirNumber: theirs, keyword })
  const pair = sub('org.moqui.impl.SmsServices.ensure#PairRoom', { ourNumber: ours, theirNumber: theirs })
  if (!pair.chatRoomId) return { outcome: 'ignored', reasonCode: pair.reasonCode || 'noMessaging' }
  const item = { itemId: String(p.messageId), type: 'message', externalRoomId: pair.externalRoomId, externalEventId: String(p.messageId),
    externalUserId: theirs, displayName: theirs, text: p.text || null, originDate: p.originMillis != null ? Number(p.originMillis) : null,
    media: Array.from(p.mediaList || []).map((file) => ({ url: file.url, contentType: file.contentType || null, fileName: file.fileName || null,
      maxBytes: file.maxBytes != null ? Number(file.maxBytes) : null })) }
  if (keyword) item.metadata = { keyword }
  const out = ec.service.sync().name('org.moqui.impl.ChatBridgeServices.ingest#Event').parameters({ chatBridgeId: p.chatBridgeId,
    transactionId: String(p.eventId), transportVerified: true, itemList: [item] }).disableAuthz().call() || {}
  if (ec.message.hasError()) return { outcome: 'failed', reasonCode: out.reasonCode || 'failed' }
  const first = Array.from(out.outcomeList || [])[0] || {}
  if (first.outcome === 'rejected') ec.logger.warn(`SMS ${p.messageId} on ${p.chatBridgeId} was rejected: ${first.reasonCode} ${first.message || ''}`)
  // a webhook sent again answers as the first did, as a duplicate
  return { outcome: out.duplicate === true ? 'duplicate' : first.outcome || 'waiting', reasonCode: first.reasonCode || null, message: first.message || null,
    duplicate: out.duplicate === true, chatRoomId: pair.chatRoomId, eventSeq: first.eventSeq ?? null }
}

// ---------- the system's own texts ----------

/** A number from send#SmsMessage's parts: contactNumber as it is when it starts with +, else the country and area codes before it. */
function contactOf(p) {
  const contact = String(p.contactNumber || '').trim()
  if (contact.startsWith('+')) return contact
  const digits = (value) => String(value || '').replace(/\D/g, '')
  return '+' + digits(p.countryCode || '1') + digits(p.areaCode) + digits(contact)
}

/**
 * org.moqui.SmsServices.send#SmsMessage: a text from fromNumber (the system's number, VoicePolicy.smsSystemNumber, when
 * empty) to the contact number, sent by userId when given (their grants and limits; else the system's), straight to the
 * provider and kept in no room. successful, messageId (the provider's), errorMessage and reasonCode; a refusal is also an
 * error.
 */
function sendMessage(p) {
  const fail = (reasonCode, message) => {
    ec.message.addError(message)
    return { successful: false, messageId: null, errorMessage: message, reasonCode }
  }
  const fromNumber = blank(p.fromNumber) ? smsPolicy().smsSystemNumber : p.fromNumber
  if (blank(fromNumber)) return fail('noMessaging', ec.l10n.localize('No number to send texts from: set the system number in Voice, Fraud'))
  // checked and counted in a transaction of its own (its lock is not held while the provider answers)
  const check = ec.service.sync().name('org.moqui.impl.SmsServices.check#SmsSend').parameters({ fromNumber, toNumber: contactOf(p),
    userId: p.userId || null, isPromotional: p.isPromotional, reserve: true }).requireNewTransaction(true).disableAuthz().call() || {}
  if (ec.message.hasError()) return fail('failed', ec.message.errorsString)
  if (!check.allowed) return fail(check.reasonCode, check.message)
  const out = ec.service.sync().name('org.moqui.impl.ChatBridgeServices.send#BridgeDirect').parameters({ chatBridgeId: check.chatBridgeId,
    externalRoomId: pairKey(check.ourNumber, check.theirNumber), text: String(p.message || ''), senderUserId: p.userId || null }).disableAuthz().call() || {}
  if (out.outcome !== 'accepted' && out.outcome !== 'uncertain') {
    ec.service.sync().name('org.moqui.impl.SmsServices.release#SmsSend').parameters({ userId: p.userId || null, counterDay: check.counterDay })
      .requireNewTransaction(true).disableAuthz().call()
  }
  if (out.outcome === 'accepted') return { successful: true, messageId: out.remoteEventId || null, errorMessage: null, reasonCode: null }
  ec.logger.warn(`A text to ${check.theirNumber} was not sent: ${out.reasonCode || out.outcome} ${out.error || ''}`)
  return fail(out.reasonCode || out.outcome || 'failed', out.outcome === 'uncertain'
    ? ec.l10n.localize('The text may not have been sent; the provider did not answer') : String(out.error || ec.l10n.localize('The text was not sent')))
}

// ---------- the user's numbers ----------

/** The numbers the current user may text from, with their descriptions. */
function myTextNumbers() {
  const me = ec.user.userId
  if (!me) return { numberList: [] }
  return { numberList: textNumbersOf(me).map((n) => ({ phoneNumber: n.phoneNumber, description: n.description || null })) }
}

const operations = {
  ensurePairRoom, pairRoomFor, openPairRoom, checkSend, noteSent, releaseSend, noteKeyword, receive, sendMessage, myTextNumbers,
  syncNumberRooms,
  keyword: (p) => ({ keyword: keywordOf(p.text) }),
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown SMS operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
