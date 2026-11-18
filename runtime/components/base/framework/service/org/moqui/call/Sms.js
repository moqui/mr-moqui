// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// SMS and MMS (plans/archive/Comms-Framework.md §5.6, §5.4 "One conversation per outside number", C4-SMS): the services of
// org.moqui.call.SmsServices and org.moqui.SmsServices.send#SmsMessage. A number's texts go through its messagingBridgeId
// (a ChatBridge whose adapter is its SMS provider's, crates/moqui-channels) and land in the number pair's external room:
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
// with the same checks, and are kept in no room; they have a daily budget of their own (smsSystemMaxPerDay, counted apart
// from the users' texts), so users cannot use up what sign-in codes need. A promotional text needs a yes from an
// sms-consent provider (an app's, or one a component registers in its conf, as Mantle does for its consent records,
// asked for the system too); with none it is refused. It counts against its number's day of promotional texts
// (smsMaxPromotionalPerNumber, promo:<number>), and a campaign's, sent with no user, not against the system's budget.

const VOICE = 'component://framework/service/org/moqui/call/Voice.js'
const CHAT_BLOCKS = 'component://framework/service/org/moqui/chat/ChatBlocks.js'
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
const DEFAULT_LIMITS = { smsMaxPerUserPerDay: 200, smsMaxPerDay: 2000, smsSystemMaxPerDay: 500, smsMaxPromotionalPerNumber: 1000 }

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
const isTrue = (value) => value === true || value === 'true' || value === 'Y'

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

/** The most numbers a group text is read with (typed, or a list); the bridge's provider's maximum is the bound (groupMaxOf). */
const NUMBERS_READ = 32

/** New group rooms a day that one outside number may start with one of our numbers, and that one of our numbers takes. */
const GROUP_ROOMS_PER_SENDER = 5
const GROUP_ROOMS_PER_NUMBER = 50

/**
 * The most outside numbers of a group text through bridge chatBridgeId: its adapter's (get#BridgeCapabilities groupMax; Telnyx's
 * group MMS 8), 0 when its provider carries no group texts or this build has no adapter for it.
 */
function groupMaxOf(chatBridgeId) {
  if (blank(chatBridgeId)) return 0
  const caps = ec.service.sync().name('org.moqui.chat.ChatServices.get#BridgeCapabilities').parameter('chatBridgeIdList', [chatBridgeId])
    .disableAuthz().call()?.capabilityMap || {}
  return Number((caps[chatBridgeId] || {}).groupMax || 0)
}

/** Why a group text of count outside numbers cannot go through bridge chatBridgeId ({reasonCode, text}), or null when it can. */
function groupRefusal(chatBridgeId, count) {
  const max = groupMaxOf(chatBridgeId)
  if (!max) return { reasonCode: 'groupTextsNotSupported', text: 'This number\'s texting provider does not carry group texts' }
  if (count > max) return { reasonCode: 'tooManyNumbers', text: `A group text has at most ${max} other numbers` }
  return null
}

/** A number in E.164 (a short code or an alphanumeric sender is not). */
const isE164 = (number) => /^\+[0-9]{5,15}$/.test(String(number || ''))

/** Whether a number is one of ours (any of the deployment's): never another party of a group text. */
const isOurs = (number) => !!one(NUMBER, { phoneNumber: number })

/** A set of outside numbers: in E.164, each once, in order (so a group has one key whoever writes first). */
function numberSet(list) {
  const out = []
  for (const value of Array.from(list || [])) {
    const number = voice('toE164', { phoneNumber: String(value || '').trim() }).number
    if (number && !out.includes(number)) out.push(number)
  }
  return out.sort()
}

/** The external room id of a group text: our number and the outside numbers, comma-separated (`ours/a,b,c`). */
function groupKey(ours, theirsList) {
  return ours + '/' + theirsList.join(',')
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
  // a group text (theirNumberList of two or more outside numbers): one room for that set of numbers; our own numbers
  // are never in it (a group MMS never goes to one of ours)
  const listed = Array.from(p.theirNumberList || [])
  if (listed.length > NUMBERS_READ) return refuse('tooManyNumbers', `A group text has at most ${NUMBERS_READ} numbers`)
  const group = numberSet(listed).filter((number) => number !== ours && !isOurs(number))
  if (group.length === 1 && blank(p.theirNumber)) p.theirNumber = group[0]
  const isGroup = group.length > 1
  const theirs = isGroup ? group.join(', ') : p.theirNumber
  if (blank(ours) || blank(theirs)) return refuse('badNumber', 'A conversation is between two numbers')
  // the number's row lock: two first texts of a pair at once make one room
  const number = find(NUMBER).condition('phoneNumber', ours).forUpdate(true).one()
  if (!number || blank(number.messagingBridgeId)) return { reasonCode: 'noMessaging' }
  if (!one(BRIDGE, { chatBridgeId: number.messagingBridgeId })) return { reasonCode: 'noMessaging' }
  const chatBridgeId = number.messagingBridgeId
  // as many others as the bridge's provider takes in a group text; none for one that carries no group texts
  if (isGroup) {
    const refusal = groupRefusal(chatBridgeId, group.length)
    if (refusal) return refuse(refusal.reasonCode, refusal.text)
  }
  const externalRoomId = isGroup ? groupKey(ours, group) : pairKey(ours, theirs)
  const now = ec.user.nowTimestamp
  let mapping = Array.from(find(MAPPING).condition('chatBridgeId', chatBridgeId).condition('externalRoomId', externalRoomId)
    .condition('thruDate', null).list())[0]
  let room = mapping ? one(ROOM, { chatRoomId: mapping.chatRoomId }) : null
  let created = false
  if (!room) {
    room = ec.entity.makeValue(ROOM)
    room.setSequencedIdPrimary()
    const topic = isGroup ? ec.resource.expand('Group texts with ${theirs} on ${ours}', '', { theirs, ours })
      : ec.resource.expand('Texts and calls with ${theirs} on ${ours}', '', { theirs, ours })
    room.setFields({ roomTypeEnumId: 'CrtExternal', roomName: theirs, topic, visibilityEnumId: 'CrvMembers', createdDate: now, lastEventSeq: 0,
      lastEventDate: now }, true, null, null)
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
  // the filters of a text that comes in (SmsSpam.js judge: CmfBlocked, CmfSpam): a new member is out of sight from the
  // start, with no joined push (plans/Comms-Messaging.md §18.3 phase 11)
  const filters = p.memberFilterMap && typeof p.memberFilterMap === 'object' ? p.memberFilterMap : {}
  if (missing.length) {
    const actorOf = ec.service.sync().name('org.moqui.chat.ChatServices.get#LocalActors').parameter('userIdList', missing)
      .disableAuthz().call()?.actorMap || {}
    for (const userId of missing) {
      const member = ec.entity.makeValue(MEMBER)
      member.setFields({ chatRoomId: room.chatRoomId, chatActorId: actorOf[userId], userId, roleEnumId: 'CmrMember', joinedDate: now,
        lastReadEventSeq: Number(room.lastEventSeq || 0), notificationLevelEnumId: 'CnlAll', isPinned: 'N', isMuted: 'N',
        filterEnumId: filters[userId] || null, filterDate: filters[userId] ? now : null }, true, null, null)
      member.create()
    }
    // the users' changes feeds in one call: the new members are in, the others' room has new members
    const changes = missing.map((userId) => ({ userIdList: [userId], changeKind: 'membership', chatRoomId: room.chatRoomId,
      change: { member: true, roleEnumId: 'CmrMember', action: 'joined' },
      pushMessage: filters[userId] ? null : { kind: 'room', action: 'joined', chatRoomId: room.chatRoomId } }))
    if (present.size) changes.push({ userIdList: Array.from(present), changeKind: 'room', chatRoomId: room.chatRoomId, change: { reason: 'members' } })
    ec.service.sync().name('org.moqui.chat.ChatServices.record#UserChange').parameter('changeList', changes).disableAuthz().call()
  }
  // a current member a new filter applies to goes out of sight now (never one already filtered)
  for (const userId of present) {
    if (filters[userId]) ec.resource.script(CHAT_BLOCKS).execute('setFilters', { userId, chatRoomIdList: [room.chatRoomId], filterEnumId: filters[userId] })
  }
  return { chatRoomId: room.chatRoomId, chatBridgeId, chatRoomBridgeId: mapping.chatRoomBridgeId, externalRoomId, created, isGroup }
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
  // the users' changes feeds, in one call for every room: those taken out are out, the others' room lost members
  const changes = []
  for (const chatRoomId of roomIds) {
    const room = one(ROOM, { chatRoomId })
    if (!room || room.roomTypeEnumId !== 'CrtExternal') continue
    const staying = []
    for (const member of rows(MEMBER, { chatRoomId })) {
      if (!member.userId) continue
      if (users.has(member.userId)) { staying.push(member.userId); continue }
      member.delete()
      removedCount++
      removedUsers.add(member.userId)
      changes.push({ userIdList: [member.userId], changeKind: 'membership', chatRoomId, change: { member: false, roleEnumId: null, action: 'left' },
        pushMessage: { kind: 'room', action: 'left', chatRoomId } })
    }
    if (staying.length && changes.some((change) => change.chatRoomId === chatRoomId))
      changes.push({ userIdList: staying, changeKind: 'room', chatRoomId, change: { reason: 'members' } })
  }
  if (changes.length) ec.service.sync().name('org.moqui.chat.ChatServices.record#UserChange').parameter('changeList', changes).disableAuthz().call()
  if (removedUsers.size) {
    ec.service.sync().name('org.moqui.chat.ChatServices.publish#ChatCounts').parameter('userIdList', Array.from(removedUsers)).disableAuthz().call()
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
    const out = ec.service.sync().name('org.moqui.call.SmsServices.ensure#PairRoom').parameters({ ourNumber: p.ourNumber,
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
  // several numbers (comma-separated): a group text, each number checked as a send to it would be
  const typed = String(p.toNumber || '').split(',').map((value) => value.trim()).filter((value) => value)
  if (typed.length > NUMBERS_READ) return refuse('tooManyNumbers', `A group text has at most ${NUMBERS_READ} numbers`)
  const dests = typed.map((value) => voice('classify', { phoneNumber: value }))
  if (!dests.length || dests.some((dest) => !dest.number)) return refuse('badNumber', 'That is not a phone number')
  if (dests.some((dest) => dest.destinationClass === 'emergency')) return refuse('emergency', 'Texts to emergency numbers are not sent from here; call them')
  if (dests.length > 1) return openGroupRoom(p, me, dests)
  const dest = dests[0]
  const numbers = textNumbersOf(me).map((n) => n.phoneNumber)
  const fromNumber = blank(p.fromNumber) ? (numbers.length === 1 ? numbers[0] : null) : voice('toE164', { phoneNumber: p.fromNumber }).number
  if (!fromNumber || !numbers.includes(fromNumber)) {
    return refuse(numbers.length ? 'numberNotGranted' : 'noMessaging', numbers.length ? 'Choose one of your numbers to text from'
      : 'You have no number to text from; an administrator can give you one')
  }
  const classRefusal = destinationRefusal(dest, me)
  if (classRefusal) return refuse(classRefusal.reasonCode, classRefusal.text)
  const out = ec.service.sync().name('org.moqui.call.SmsServices.ensure#PairRoom').parameters({ ourNumber: fromNumber,
    theirNumber: dest.number }).requireNewTransaction(true).disableAuthz().call() || {}
  if (ec.message.hasError()) return {}
  if (!out.chatRoomId) return refuse(out.reasonCode || 'noMessaging', 'That number has no messaging')
  // back in it when they left
  const mine = one(MEMBER, { chatRoomId: out.chatRoomId, userId: me })
  if (mine && mine.leftDate) {
    mine.set('leftDate', null)
    mine.update()
    ec.service.sync().name('org.moqui.chat.ChatServices.record#UserChange').parameters({ userIdList: [me], changeKind: 'membership',
      chatRoomId: out.chatRoomId, change: { member: true, roleEnumId: mine.roleEnumId ?? null, action: 'joined' } }).disableAuthz().call()
  }
  return { chatRoomId: out.chatRoomId, ourNumber: fromNumber, theirNumber: dest.number }
}

/** The number the user texts from (fromNumber, else their only one), or a refusal. */
function fromNumberOf(p, me) {
  const numbers = textNumbersOf(me).map((n) => n.phoneNumber)
  const fromNumber = blank(p.fromNumber) ? (numbers.length === 1 ? numbers[0] : null) : voice('toE164', { phoneNumber: p.fromNumber }).number
  if (!fromNumber || !numbers.includes(fromNumber)) {
    return { refusal: refuse(numbers.length ? 'numberNotGranted' : 'noMessaging', numbers.length ? 'Choose one of your numbers to text from'
      : 'You have no number to text from; an administrator can give you one') }
  }
  return { fromNumber }
}

/** A group text (open#PairRoom with several numbers): the room of fromNumber and that set of outside numbers. */
function openGroupRoom(p, me, dests) {
  const from = fromNumberOf(p, me)
  if (from.refusal) return from.refusal
  for (const dest of dests) {
    const classRefusal = destinationRefusal(dest, me)
    if (classRefusal) return refuse(classRefusal.reasonCode, classRefusal.text)
  }
  const theirNumberList = numberSet(dests.map((dest) => dest.number)).filter((number) => number !== from.fromNumber)
  if (theirNumberList.length < 2) return openPairRoom(Object.assign({}, p, { toNumber: theirNumberList[0] || '' }))
  // refused here, with its reason (ensure#PairRoom's own refusal would come back as an error alone)
  const refusal = groupRefusal((one(NUMBER, { phoneNumber: from.fromNumber }) || {}).messagingBridgeId, theirNumberList.length)
  if (refusal) return refuse(refusal.reasonCode, refusal.text)
  const out = ec.service.sync().name('org.moqui.call.SmsServices.ensure#PairRoom').parameters({ ourNumber: from.fromNumber,
    theirNumberList }).requireNewTransaction(true).disableAuthz().call() || {}
  if (ec.message.hasError()) return {}
  if (!out.chatRoomId) return refuse(out.reasonCode || 'noMessaging', 'That number has no messaging')
  const mine = one(MEMBER, { chatRoomId: out.chatRoomId, userId: me })
  if (mine && mine.leftDate) {
    mine.set('leftDate', null)
    mine.update()
    ec.service.sync().name('org.moqui.chat.ChatServices.record#UserChange').parameters({ userIdList: [me], changeKind: 'membership',
      chatRoomId: out.chatRoomId, change: { member: true, roleEnumId: mine.roleEnumId ?? null, action: 'joined' } }).disableAuthz().call()
  }
  return { chatRoomId: out.chatRoomId, ourNumber: from.fromNumber, theirNumber: theirNumberList.join(','), theirNumberList }
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

/** Whether a promotional text may go: an sms-consent provider says yes, none says no and none failed. */
function consented(fromNumber, toNumber) {
  ec.message.pushErrors()
  try {
    const out = ec.service.sync().name('org.moqui.ui.ShellServices.call#Providers').parameters({ hook: 'sms-consent',
      parameters: { fromNumber, toNumber } }).disableAuthz().call() || {}
    const answers = Array.from(out.results || [])
    // a provider that failed may have said no: the text is refused, as for a no
    return out.unavailable !== true && Array.from(out.failedParts || []).length === 0
      && answers.some((answer) => answer && answer.consent === true) && !answers.some((answer) => answer && answer.consent === false)
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
  const promotional = isTrue(p.isPromotional)
  if (promotional) {
    if (!consented(from, dest.number)) return no('consentRequired', 'A promotional text needs the person\'s consent, and none is recorded')
  }
  const limits = smsPolicy()
  const reserve = p.reserve === true || p.reserve === 'true' || p.reserve === 'Y'
  const counters = countersOf(userId, from, promotional)
  const day = today()
  // reserve: the check and the count are one step, under the lock of the day's count of all users' texts, of the
  // system's, or of the number's promotional texts (made at 0 when there is none yet), so sends at once cannot pass a
  // limit together; the caller lets the count go (release#SmsSend) when the text does not reach the provider
  if (reserve) {
    for (const lockKey of counters.locks) {
      if (find(COUNT).condition({ counterKey: lockKey, dayDate: day }).forUpdate(true).one()) continue
      const first = ec.entity.makeValue(COUNT)
      first.setFields({ counterKey: lockKey, dayDate: day, sentCount: 0 }, true, null, null)
      first.create()
    }
  }
  const limitOf = { all: limits.smsMaxPerDay, system: limits.smsSystemMaxPerDay }
  const over = counters.keys.some((counterKey) => sentToday(counterKey, day) >= (counterKey.startsWith('user:')
    ? limits.smsMaxPerUserPerDay : counterKey.startsWith('promo:') ? limits.smsMaxPromotionalPerNumber : limitOf[counterKey]))
  if (over) return no('dailyLimit', 'The limit of texts for a day is reached')
  if (reserve) countSent(counters.keys, day, 1)
  return { allowed: true, ourNumber: from, theirNumber: dest.number, chatBridgeId: number.messagingBridgeId, destinationClass: dest.destinationClass,
    counterDay: reserve ? day : null }
}

/**
 * The day's counts a text counts toward (keys) and the ones whose row lock a reserve takes (locks): a user's text, the
 * user's and all users'; the system's, its own budget (sign-in codes); a promotional text also its number's
 * (promo:<number>), and a promotional text with no user (a campaign's) that alone, so it never spends the system's budget.
 */
function countersOf(userId, fromNumber, promotional) {
  const promo = promotional && fromNumber ? 'promo:' + fromNumber : null
  if (userId) return { keys: promo ? ['user:' + userId, 'all', promo] : ['user:' + userId, 'all'], locks: promo ? ['all', promo] : ['all'] }
  if (promo) return { keys: [promo], locks: [promo] }
  return { keys: ['system'], locks: ['system'] }
}

/** Add delta to the day's counts of each of counterKeys, never below 0. */
function countSent(counterKeys, day, delta) {
  for (const counterKey of counterKeys) {
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

/** The number in E.164 of a counted text's fromNumber, for its promotional count; null when it is not promotional. */
function promoNumber(p) {
  return isTrue(p.isPromotional) && !blank(p.fromNumber) ? voice('toE164', { phoneNumber: p.fromNumber }).number || null : null
}

/** Internal: a text went to the provider (taken, or not known): it counts for the sender's day (a user's, and all users'; or the system's), and a promotional one for its number's. */
function noteSent(p) {
  countSent(countersOf(blank(p.userId) ? null : p.userId, promoNumber(p), isTrue(p.isPromotional)).keys, today(), 1)
  return {}
}

/** Internal: a text check#SmsSend reserved (counted) on counterDay did not reach the provider: it no longer counts. */
function releaseSend(p) {
  countSent(countersOf(blank(p.userId) ? null : p.userId, promoNumber(p), isTrue(p.isPromotional)).keys,
    blank(p.counterDay) ? today() : String(p.counterDay), -1)
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
 * Internal (the adapter's webhook, after it checked the provider's signature): an opt-out (STOP) or opt-in (START) the
 * provider tells of as its event eventId, applied once: the event is kept as a done ChatBridgeInbound (transactionId
 * signal:<eventId>, removed with the batches after 30 days), so a copy changes nothing. Only for a number of ours on this
 * bridge. outcome noted, duplicate or ignored.
 */
function noteOptSignal(p) {
  const keyword = String(p.keyword || '').toUpperCase()
  if (!['STOP', 'START'].includes(keyword)) return { outcome: 'ignored', reasonCode: 'badKeyword' }
  const ours = voice('toE164', { phoneNumber: p.ourNumber }).number
  const theirs = voice('toE164', { phoneNumber: p.theirNumber }).number
  const number = ours ? one(NUMBER, { phoneNumber: ours }) : null
  if (!number || number.messagingBridgeId !== p.chatBridgeId) return { outcome: 'ignored', reasonCode: 'unknownNumber' }
  if (!theirs) return { outcome: 'ignored', reasonCode: 'badNumber' }
  const transactionId = 'signal:' + String(p.eventId)
  // the bridge's row lock: two copies at once apply it once
  find(BRIDGE).condition('chatBridgeId', p.chatBridgeId).forUpdate(true).one()
  if (one('moqui.chat.ChatBridgeInbound', { chatBridgeId: p.chatBridgeId, transactionId })) return { outcome: 'duplicate' }
  const seen = ec.entity.makeValue('moqui.chat.ChatBridgeInbound')
  seen.setFields({ chatBridgeId: p.chatBridgeId, transactionId, receivedDate: ec.user.nowTimestamp, processedDate: ec.user.nowTimestamp,
    statusEnumId: 'CbiDone', itemCount: 0, waitingCount: 0 }, true, null, null)
  seen.create()
  noteKeyword({ ourNumber: ours, theirNumber: theirs, keyword })
  return { outcome: 'noted' }
}

/**
 * Internal (the adapter's webhook, after it checked the provider's signature): a text from theirNumber to ourNumber.
 * ourNumber must have messaging through chatBridgeId (else outcome ignored, reasonCode unknownNumber). A STOP, START or HELP
 * keeps the pair's opt-out (STOP and START once for the message). The pair room is found or made, then the text goes through ingest#Event as the provider's
 * transaction eventId (a repeated webhook does nothing again): outcome, chatRoomId, eventSeq.
 */
function receive(p) {
  let ours = voice('toE164', { phoneNumber: p.ourNumber }).number
  const theirs = voice('toE164', { phoneNumber: p.theirNumber }).number || String(p.theirNumber || '').replace(/[^0-9+]/g, '').slice(0, 20)
  let number = ours ? one(NUMBER, { phoneNumber: ours }) : null
  // a group text (recipientList: every number it went to): our number is the one of them on this bridge
  const recipients = numberSet(p.recipientList)
  if ((!number || number.messagingBridgeId !== p.chatBridgeId) && recipients.length) {
    for (const candidate of recipients) {
      const row = one(NUMBER, { phoneNumber: candidate })
      if (row && row.messagingBridgeId === p.chatBridgeId) { ours = candidate; number = row; break }
    }
  }
  // a provider's conversation (externalThreadId: a Twilio Conversation of a group text): the room of the mapping that
  // keeps it, of our number and its numbers, whoever writes
  const thread = blank(p.externalThreadId) ? null : Array.from(find(MAPPING).condition('chatBridgeId', p.chatBridgeId)
    .condition('externalThreadId', String(p.externalThreadId)).condition('thruDate', null).list())[0]
  const threadNumbers = thread ? String(thread.externalRoomId || '').split('/') : null
  if (threadNumbers && threadNumbers.length === 2) {
    const row = one(NUMBER, { phoneNumber: threadNumbers[0] })
    if (row && row.messagingBridgeId === p.chatBridgeId) { ours = threadNumbers[0]; number = row }
  }
  if (!number || number.messagingBridgeId !== p.chatBridgeId) return { outcome: 'ignored', reasonCode: 'unknownNumber' }
  if (!theirs) return { outcome: 'ignored', reasonCode: 'badNumber' }
  const threadList = threadNumbers && threadNumbers.length === 2 && threadNumbers[0] === ours ? threadNumbers[1].split(',').filter((n) => n) : null
  // the others it went to: a group's room, the sender and them. Our numbers are left out (two of ours in one group are
  // both ours), the sender is kept whatever is cut (then at most the bridge's group maximum less one others), and a
  // sender that is not a number in E.164 (a short code) keeps a room of its own: never another person's pair room; on a
  // bridge whose provider carries no group texts it is the sender's pair room, as a reply could not go to the group
  const groupMax = recipients.length > 1 ? groupMaxOf(p.chatBridgeId) : 0
  const others = isE164(theirs) && groupMax ? recipients.filter((value) => value !== theirs && value !== ours && !isOurs(value)) : []
  let groupNumbers = threadList ? (threadList.length > 1 ? threadList : null)
    : others.length ? numberSet([theirs].concat(others.slice(0, groupMax - 1))) : null
  // a conversation's pair: its one number
  const pairTheirs = threadList && threadList.length === 1 ? threadList[0] : theirs
  let note = null
  if (groupNumbers && !threadList) {
    // a new group room: at most GROUP_ROOMS_PER_SENDER a day from one outside number, GROUP_ROOMS_PER_NUMBER for one of
    // ours; past that the text goes to the sender's pair room, said so, and no new room reaches the number's users
    const key = groupKey(ours, groupNumbers)
    const known = find(MAPPING).condition('chatBridgeId', p.chatBridgeId).condition('externalRoomId', key).condition('thruDate', null).count() > 0
    if (!known) {
      const day = today()
      const senderKey = 'group:' + ours + ':' + theirs
      const numberKey = 'groups:' + ours
      if (sentToday(senderKey, day) >= GROUP_ROOMS_PER_SENDER || sentToday(numberKey, day) >= GROUP_ROOMS_PER_NUMBER) {
        ec.logger.warn(`SMS ${p.messageId}: a new group text from ${theirs} to ${ours} past the day's new groups; kept in their pair room`)
        note = ec.resource.expand('[A group text with ${count} numbers, kept here: more new groups came from this number today than are opened]',
          '', { count: groupNumbers.length + 1 })
        groupNumbers = null
      } else {
        ec.transaction.runRequireNew(null, 'The new group text was not counted', () => countSent([senderKey, numberKey], day, 1))
      }
    }
  }
  const keyword = keywordOf(p.text)
  const sub = (name, parameters) => {
    const out = ec.service.sync().name(name).parameters(parameters).requireNewTransaction(true).disableAuthz().call() || {}
    if (ec.message.hasError()) throw new Error(ec.message.errorsString)
    return out
  }
  // a STOP or START is applied once for its message (note#SmsOptSignal, signal:text:<messageId>), so a copy of the
  // webhook the replay cache no longer holds (a provider whose signature has no time: Twilio) does not change the
  // opt-out again after a later keyword; HELP changes no opt-out
  if (keyword === 'HELP') sub('org.moqui.call.SmsServices.note#SmsKeyword', { ourNumber: ours, theirNumber: theirs, keyword })
  else if (keyword) sub('org.moqui.call.SmsServices.note#SmsOptSignal', { chatBridgeId: p.chatBridgeId,
    eventId: 'text:' + String(p.messageId), ourNumber: ours, theirNumber: theirs, keyword })
  // the blocks and the spam filter (SmsSpam.js judge, in a transaction of its own): what each user's room is (in sight,
  // blocked, spam); one that fails lets the text in unfiltered
  let verdict = { memberFilterMap: {}, spam: false, judged: false }
  try {
    verdict = sub('org.moqui.call.SmsSpamServices.judge#SmsText', { chatBridgeId: p.chatBridgeId, ourNumber: ours, theirNumber: theirs,
      pairNumber: pairTheirs, groupNumbers, text: p.text })
  } catch (err) {
    ec.logger.error(`SMS ${p.messageId}: not judged for spam or blocks, taken in: ${err && err.message ? err.message : err}`)
    ec.message.clearErrors()
  }
  const pair = sub('org.moqui.call.SmsServices.ensure#PairRoom', groupNumbers ? { ourNumber: ours, theirNumberList: groupNumbers,
    memberFilterMap: verdict.memberFilterMap } : { ourNumber: ours, theirNumber: pairTheirs, memberFilterMap: verdict.memberFilterMap })
  if (!pair.chatRoomId) return { outcome: 'ignored', reasonCode: pair.reasonCode || 'noMessaging' }
  const item = { itemId: String(p.messageId), type: 'message', externalRoomId: pair.externalRoomId, externalEventId: String(p.messageId),
    externalUserId: theirs, displayName: theirs, text: note ? note + (p.text ? '\n' + p.text : '') : p.text || null, originDate: p.originMillis != null ? Number(p.originMillis) : null,
    media: Array.from(p.mediaList || []).map((file) => ({ url: file.url, contentType: file.contentType || null, fileName: file.fileName || null,
      maxBytes: file.maxBytes != null ? Number(file.maxBytes) : null })) }
  if (keyword) item.metadata = { keyword }
  const out = ec.service.sync().name('org.moqui.chat.ChatBridgeServices.ingest#Event').parameters({ chatBridgeId: p.chatBridgeId,
    transactionId: String(p.eventId), transportVerified: true, itemList: [item] }).disableAuthz().call() || {}
  if (ec.message.hasError()) return { outcome: 'failed', reasonCode: out.reasonCode || 'failed' }
  const first = Array.from(out.outcomeList || [])[0] || {}
  if (first.outcome === 'rejected') ec.logger.warn(`SMS ${p.messageId} on ${p.chatBridgeId} was rejected: ${first.reasonCode} ${first.message || ''}`)
  // the rules a spam text matched count it, and its number keeps the verdict; a failure is logged
  if (verdict.spam && first.outcome === 'accepted' && out.duplicate !== true) {
    try {
      sub('org.moqui.call.SmsSpamServices.note#SmsJudged', { theirNumber: theirs, score: verdict.score, reasons: verdict.reasons })
    } catch (err) {
      ec.logger.error(`SMS ${p.messageId}: its spam verdict was not kept: ${err && err.message ? err.message : err}`)
      ec.message.clearErrors()
    }
  }
  // the conversation of a new outside party filed on a record by the apps (external-conversation), until one did it; not
  // while it is spam (Not spam and the next text file it)
  if (first.outcome === 'accepted' && first.eventSeq != null && out.duplicate !== true && pair.chatRoomBridgeId && !verdict.spam) {
    const mapping = one(MAPPING, { chatRoomBridgeId: pair.chatRoomBridgeId })
    if (mapping && !mapping.filedDate) {
      try {
        ec.service.sync().name('org.moqui.chat.ChatBridgeServices.file#ExternalConversation').parameters({ chatRoomBridgeId: pair.chatRoomBridgeId,
          eventSeq: first.eventSeq }).disableAuthz().call()
      } catch (err) {
        ec.logger.error(`SMS ${p.messageId}: its conversation was not filed: ${err && err.message ? err.message : err}`)
      }
      ec.message.clearErrors()
    }
  }
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
 * error. deliveryOutcome, as send#EmailTemplate's, for a caller that keeps its own ledger: retryable (the day's limit, or
 * the provider asked to try later), permanent (refused before the provider, or by it), uncertain (the provider's answer
 * was lost: it may have been sent, so it is never sent again on its own).
 */
function sendMessage(p) {
  const fail = (reasonCode, message, deliveryOutcome) => {
    ec.message.addError(message)
    return { successful: false, messageId: null, errorMessage: message, reasonCode, deliveryOutcome: deliveryOutcome || 'permanent' }
  }
  const fromNumber = blank(p.fromNumber) ? smsPolicy().smsSystemNumber : p.fromNumber
  if (blank(fromNumber)) return fail('noMessaging', ec.l10n.localize('No number to send texts from: set the system number in Voice, Fraud'))
  // checked and counted in a transaction of its own (its lock is not held while the provider answers)
  const check = ec.service.sync().name('org.moqui.call.SmsServices.check#SmsSend').parameters({ fromNumber, toNumber: contactOf(p),
    userId: p.userId || null, isPromotional: p.isPromotional, reserve: true }).requireNewTransaction(true).disableAuthz().call() || {}
  if (ec.message.hasError()) return fail('failed', ec.message.errorsString, 'retryable')
  if (!check.allowed) return fail(check.reasonCode, check.message, check.reasonCode === 'dailyLimit' ? 'retryable' : 'permanent')
  const out = ec.service.sync().name('org.moqui.chat.ChatBridgeServices.send#BridgeDirect').parameters({ chatBridgeId: check.chatBridgeId,
    externalRoomId: pairKey(check.ourNumber, check.theirNumber), text: String(p.message || ''), senderUserId: p.userId || null }).disableAuthz().call() || {}
  if (out.outcome !== 'accepted' && out.outcome !== 'uncertain') {
    ec.service.sync().name('org.moqui.call.SmsServices.release#SmsSend').parameters({ userId: p.userId || null, counterDay: check.counterDay,
      fromNumber: check.ourNumber, isPromotional: p.isPromotional }).requireNewTransaction(true).disableAuthz().call()
  }
  if (out.outcome === 'accepted') return { successful: true, messageId: out.remoteEventId || null, errorMessage: null, reasonCode: null, deliveryOutcome: null }
  ec.logger.warn(`A text to ${check.theirNumber} was not sent: ${out.reasonCode || out.outcome} ${out.error || ''}`)
  return fail(out.reasonCode || out.outcome || 'failed', out.outcome === 'uncertain'
    ? ec.l10n.localize('The text may not have been sent; the provider did not answer') : String(out.error || ec.l10n.localize('The text was not sent')),
  out.outcome === 'uncertain' ? 'uncertain' : out.outcome === 'retry' ? 'retryable' : 'permanent')
}

// ---------- the user's numbers ----------

/** The numbers the current user may text from, with their descriptions. */
function myTextNumbers() {
  const me = ec.user.userId
  if (!me) return { numberList: [] }
  return { numberList: textNumbersOf(me).map((n) => ({ phoneNumber: n.phoneNumber, description: n.description || null })) }
}

/** At most this many numbers are named in one find#PhoneNames, and this many an hour for one user. */
const NAMES_MAX = 50
const NAMES_PER_HOUR = 600

/** Whether the number is one of a phone room the user is in (a pair's or a group's mapping), so it may be named. */
function inMyPhoneRoom(me, number) {
  const mappings = Array.from(find(MAPPING).condition('externalRoomId', 'like', '%' + number + '%').condition('thruDate', null).list())
    .filter((m) => String(m.externalRoomId).split('/')[1]?.split(',').includes(number))
  return mappings.some((m) => find(MEMBER).condition({ chatRoomId: m.chatRoomId, userId: me }).condition('leftDate', null).count() > 0)
}

/**
 * The names of outside numbers for the current user (plans/Comms-Messaging.md §9, Phone book): what the call-context
 * providers of the apps the user may open say of each (ShellServices.call#Providers, as this user, the first contact's
 * name), for the phone rooms' names. Only a number of a phone room the user is in is asked; a number no app knows is
 * left out; past NAMES_PER_HOUR numbers an hour, none (reasonCode rateLimited). Out: nameMap (number to name).
 */
function phoneNames(p) {
  const me = ec.user.userId
  if (!me) return { nameMap: {} }
  // only the numbers of the user's own phone rooms, so the apps' contacts cannot be read by number
  const numbers = numberSet(String(p.numbers || '').split(',').concat(Array.from(p.numberList || []))).slice(0, NAMES_MAX)
    .filter((number) => inMyPhoneRoom(me, number))
  const nameMap = {}
  if (!numbers.length) return { nameMap }
  // at most NAMES_PER_HOUR numbers an hour a user (counted on the distributed cache by user and hour)
  const asked = ec.cache.getCache('moqui.sms.names.asked')
  const hourKey = me + ':' + Math.floor(nowMs() / 3600000)
  const count = Number(asked.get(hourKey) || 0)
  if (count + numbers.length > NAMES_PER_HOUR) return { nameMap, reasonCode: 'rateLimited' }
  asked.put(hourKey, count + numbers.length)
  for (const number of numbers) {
    const answers = ec.service.sync().name('org.moqui.ui.ShellServices.call#Providers')
      .parameters({ hook: 'call-context', parameters: { number } }).call() || {}
    ec.message.clearErrors()
    for (const answer of Array.from(answers.results || [])) {
      const contact = Array.from(answer.contactList || []).find((c) => c && !blank(c.name))
      if (contact) { nameMap[number] = String(contact.name).slice(0, 200); break }
    }
  }
  return { nameMap }
}

const operations = {
  // the users a number of ours reaches (SmsSpam.js)
  usersOf: (p) => {
    const number = blank(p.phoneNumber) ? null : one(NUMBER, { phoneNumber: p.phoneNumber })
    return { userIdList: number ? usersOf(number) : [] }
  },
  ensurePairRoom, pairRoomFor, openPairRoom, checkSend, noteSent, releaseSend, noteKeyword, noteOptSignal, receive, sendMessage, myTextNumbers,
  syncNumberRooms, phoneNames,
  keyword: (p) => ({ keyword: keywordOf(p.text) }),
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown SMS operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
