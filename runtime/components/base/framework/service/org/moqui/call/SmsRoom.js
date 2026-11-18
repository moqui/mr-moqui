// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Text members of a room (plans/Comms-Messaging.md §18.3 phase 18; docs/interfaces/SMS.md "Text members of a room"): the
// services of org.moqui.call.SmsRoomServices and the helpers Sms.js calls.
//
// A moderator of a private room adds an outside number, with the person's consent, on one of their granted numbers that an
// administrator marked for room texts (PhoneNumber.roomTextsEnabled). The member is an external actor of that number's
// bridge, and its mapping (ChatRoomBridge with memberChatActorId) has the external room id ours/theirs#chatRoomId: the
// room's messages go to it as texts, "Ann: ...", checked in room mode (check#SmsSend chatRoomId), and its replies come back
// through receive#Sms as the member.
//
// One room per outside number on each of our numbers (the user's decision, plans/Comms-Messaging.md §19.1): while a number
// is a text member on ours, all of its texts to ours go to that room, and a send from the pair room of the two is refused
// (inRoomByText). A further room on a number is refused (noFreeNumber), and the add dialog says why.
//
// The name the adder gives is the room's alone (ChatRoomBridge.memberName): the shared ChatExternalUser keeps the number.

const VOICE = 'component://framework/service/org/moqui/call/Voice.js'
const CHAT_BLOCKS = 'component://framework/service/org/moqui/chat/ChatBlocks.js'
const ROOM = 'moqui.chat.ChatRoom'
const MEMBER = 'moqui.chat.ChatRoomMember'
const MAPPING = 'moqui.chat.ChatRoomBridge'
const EXTERNAL = 'moqui.chat.ChatExternalUser'
const ACTOR = 'moqui.chat.ChatActor'
const BRIDGE = 'moqui.chat.ChatBridge'
const NUMBER = 'moqui.call.PhoneNumber'
const OPT_OUT = 'moqui.call.SmsOptOut'
const COUNT = 'moqui.call.SmsSendCount'
const POLICY = 'moqui.call.VoicePolicy'
const ACCOUNT = 'moqui.security.UserAccount'
const DEFAULTS = { smsRoomMaxMembers: 10, smsRoomAddsPerDay: 20 }
const DAY = 86400000

const ms = (value) => {
  if (value == null || value === '') return null
  const number = Number(value)
  return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}
const nowMs = () => ms(ec.user.nowTimestamp)
const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const blank = (value) => value == null || String(value).trim() === ''
const isTrue = (value) => value === true || value === 'true' || value === 'Y'
const message = (err) => (err && err.message ? err.message : String(err))
const today = () => new Date(nowMs()).toISOString().slice(0, 10)

function voice(op, parameters) {
  return ec.resource.script(VOICE).execute(op, parameters || {}) || {}
}

function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

const memberKey = (ours, theirs, chatRoomId) => ours + '/' + theirs + '#' + chatRoomId
const theirsOf = (mapping) => String(mapping.externalRoomId || '').split('#')[0].split('/')[1] || null
const oursOf = (mapping) => String(mapping.externalRoomId || '').split('/')[0] || null

function policy() {
  const row = one(POLICY, { voicePolicyId: 'DEFAULT' }) || {}
  const out = {}
  for (const [key, value] of Object.entries(DEFAULTS)) out[key] = blank(row[key]) ? value : Number(row[key])
  return out
}

/** The active text member mappings of theirs on ours (any room), with rooms that are not archived. */
function activeOn(ours, theirs) {
  const prefix = ours + '/' + theirs + '#'
  const list = Array.from(find(MAPPING).condition('externalRoomId', 'like', prefix + '%').condition('thruDate', null)
    .condition('memberChatActorId', 'not-equals', null).list()).filter((row) => String(row.externalRoomId).startsWith(prefix))
  if (!list.length) return []
  const live = new Set(Array.from(find(ROOM).condition('chatRoomId', 'in', list.map((row) => row.chatRoomId)).condition('archivedDate', null)
    .selectField('chatRoomId').list()).map((room) => room.chatRoomId))
  return list.filter((row) => live.has(row.chatRoomId))
}

/** Internal (receive#Sms): the text member mapping a pair text from theirNumber to ourNumber goes to, or none. */
function routeOf(p) {
  const mapping = activeOn(p.ourNumber, p.theirNumber).find((row) => row.chatBridgeId === p.chatBridgeId)
  return { mapping: mapping || null }
}

/** Internal (check#SmsSend inRoom): the room theirNumber is a text member of on ourNumber, by its name, or null. */
function memberRoomName(p) {
  const mapping = activeOn(p.ourNumber, p.theirNumber).find((row) => !p.chatBridgeId || row.chatBridgeId === p.chatBridgeId)
  if (!mapping) return { roomName: null }
  const room = one(ROOM, { chatRoomId: mapping.chatRoomId })
  return { roomName: (room && room.roomName) || ec.l10n.localize('a room') }
}

/** The user's numbers for room texts: granted to them, with messaging, marked roomTextsEnabled. */
function roomNumbersOf(userId) {
  const granted = Array.from(voice('grants', { userId }).numberList || [])
  if (!granted.length) return []
  return Array.from(find(NUMBER).condition('phoneNumber', 'in', granted).condition('roomTextsEnabled', 'Y').orderBy('phoneNumber').list())
    .filter((row) => !blank(row.messagingBridgeId))
}

/** Why the user may not add a text member to room now, as a refusal, or null (the room's rules, before any number). */
function roomRefusal(room, roomAccess) {
  if (!room || !roomAccess.isMember) return { reasonCode: 'notFound', text: 'Chat room not found' }
  if (room.roomTypeEnumId !== 'CrtPrivate' || room.recordEntityName || room.visibilityEnumId === 'CrvRecord') {
    return { reasonCode: 'roomType', text: 'Only a private room has text members' }
  }
  if (room.encrypted === 'Y') return { reasonCode: 'encrypted', text: 'An end-to-end encrypted room has no text members: a text is not encrypted' }
  if (!blank(room.messageTimerSeconds)) return { reasonCode: 'timerOn', text: 'A room with disappearing messages has no text members: a text cannot be deleted' }
  if (room.archivedDate) return { reasonCode: 'archived', text: 'The room is archived' }
  if (!roomAccess.canModerate) return { reasonCode: 'notAllowed', text: 'Only a moderator or owner may add a text member' }
  return null
}

function access(chatRoomId) {
  return ec.service.sync().name('org.moqui.chat.ChatServices.get#RoomAccess').parameter('chatRoomId', chatRoomId).call() || {}
}

/** The choices of our number for theirs: each of the user's room numbers, free or taken (by the room's name). */
function choicesFor(userId, theirs) {
  return roomNumbersOf(userId).map((number) => {
    const taken = theirs ? activeOn(number.phoneNumber, theirs)[0] : null
    const room = taken ? one(ROOM, { chatRoomId: taken.chatRoomId }) : null
    // the pair room of the two had a text in the last 30 days: its replies would come to the room instead
    const pair = theirs ? Array.from(find(MAPPING).condition('chatBridgeId', number.messagingBridgeId)
      .condition('externalRoomId', number.phoneNumber + '/' + theirs).condition('thruDate', null).list())[0] : null
    const pairRoom = pair ? one(ROOM, { chatRoomId: pair.chatRoomId }) : null
    return { phoneNumber: number.phoneNumber, description: number.description || null, free: !taken,
      takenChatRoomId: taken ? taken.chatRoomId : null, takenRoomName: room ? room.roomName || null : null,
      pairRoomActive: !!pairRoom && !!pairRoom.lastMessageDate && ms(pairRoom.lastMessageDate) >= nowMs() - 30 * DAY }
  })
}

/** The day's count of counterKey (SmsSendCount). */
function counted(counterKey) {
  const row = one(COUNT, { counterKey, dayDate: today() })
  return row ? Number(row.sentCount || 0) : 0
}

function countOne(counterKey) {
  const row = find(COUNT).condition({ counterKey, dayDate: today() }).forUpdate(true).one()
  if (row) {
    row.setFields({ sentCount: Number(row.sentCount || 0) + 1, lastSentDate: ec.user.nowTimestamp }, true, null, null)
    row.update()
  } else {
    ec.entity.makeValue(COUNT).setFields({ counterKey, dayDate: today(), sentCount: 1, lastSentDate: ec.user.nowTimestamp }, true, null, null).create()
  }
}

/** The adder's choices of our number for phoneNumber in a room (the add dialog). */
function textMemberChoices(p) {
  const me = ec.user.userId
  const room = me ? one(ROOM, { chatRoomId: p.chatRoomId }) : null
  const refusal = roomRefusal(room, room ? access(p.chatRoomId) : {})
  if (refusal) return refuse(refusal.reasonCode, refusal.text)
  const theirs = blank(p.phoneNumber) ? null : voice('toE164', { phoneNumber: String(p.phoneNumber).trim() }).number || null
  return { numberList: choicesFor(me, theirs), phoneNumber: theirs }
}

/**
 * Add a text member: the checks, then the rows in a transaction of their own (store#TextMember), the day's add counted, and
 * the welcome text sent after that commit. Out: chatActorId, chatRoomBridgeId, ourNumber, warning (pairRoomActive),
 * welcomeSent.
 */
function addTextMember(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in to add a text member')
  const room = one(ROOM, { chatRoomId: p.chatRoomId })
  const refusal = roomRefusal(room, room ? access(p.chatRoomId) : {})
  if (refusal) return refuse(refusal.reasonCode, refusal.text)
  const dest = voice('classify', { phoneNumber: String(p.phoneNumber || '').trim() })
  if (!dest.number || !/^\+[0-9]{5,15}$/.test(dest.number)) return refuse('badNumber', 'That is not a phone number')
  if (dest.destinationClass === 'emergency') return refuse('emergency', 'An emergency number cannot be a text member')
  const pol = voice('policy')
  if (dest.destinationClass === 'international' && pol.internationalEnabled !== 'Y') return refuse('internationalNotEnabled', 'Texts to international numbers are not enabled')
  if (dest.destinationClass === 'premium' && pol.premiumEnabled !== 'Y') return refuse('premiumNotEnabled', 'Texts to premium numbers are not enabled')
  if (one(NUMBER, { phoneNumber: dest.number })) return refuse('badNumber', 'That is one of our numbers')
  if (!isTrue(p.consent)) return refuse('consentRequired', 'Confirm that this person agreed to get the room\'s texts')
  if ((ec.resource.script(CHAT_BLOCKS).execute('blockersOf', { fromNumber: dest.number, userIdList: [me] }).userIdList || []).length) {
    return refuse('blocked', 'You blocked this number')
  }
  const limits = policy()
  const current = find(MAPPING).condition('chatRoomId', room.chatRoomId).condition('thruDate', null).condition('memberChatActorId', 'not-equals', null).list()
  if (Array.from(current).some((row) => theirsOf(row) === dest.number)) return refuse('alreadyMember', 'This number is a text member of the room already')
  if (Array.from(current).length >= limits.smsRoomMaxMembers) return refuse('tooManyMembers', `A room has at most ${limits.smsRoomMaxMembers} text members`)
  if (counted('roomAdds:' + me) >= limits.smsRoomAddsPerDay) return refuse('dailyLimit', 'The limit of text members added in a day is reached')
  const choices = choicesFor(me, dest.number)
  if (!choices.length) return refuse('noRoomNumber', 'You have no number for room texts; an administrator can mark one')
  let pick
  if (!blank(p.ourNumber)) {
    const ours = voice('toE164', { phoneNumber: p.ourNumber }).number
    pick = choices.find((choice) => choice.phoneNumber === ours)
    if (!pick) return refuse('numberNotGranted', 'Choose one of your numbers for room texts')
  } else {
    pick = choices.find((choice) => choice.free) || choices[0]
  }
  if (!pick.free) {
    // one room per outside number on each of our numbers (plans/Comms-Messaging.md §19.1)
    ec.message.addError(ec.resource.expand('This person is already a text member of ${roomName} on this number; another number is needed.', '',
      { roomName: pick.takenRoomName || ec.l10n.localize('a room') }))
    return { reasonCode: 'noFreeNumber' }
  }
  const optOut = one(OPT_OUT, { phoneNumber: pick.phoneNumber, otherNumber: dest.number })
  if (optOut && optOut.optedOut === 'Y') return refuse('optedOut', 'This person replied STOP to this number; they are not texted until they reply START')
  const stored = ec.service.sync().name('org.moqui.call.SmsRoomServices.store#TextMember').parameters({ chatRoomId: room.chatRoomId,
    ourNumber: pick.phoneNumber, theirNumber: dest.number, memberName: blank(p.memberName) ? null : String(p.memberName).trim().slice(0, 60) })
    .requireNewTransaction(true).call() || {}
  if (ec.message.hasError() || !stored.chatRoomBridgeId) return {}
  try {
    ec.transaction.runRequireNew(null, 'The text member add was not counted', () => countOne('roomAdds:' + me))
  } catch (err) {
    ec.logger.error(`The add of text member ${dest.number} by ${me} was not counted: ${message(err)}`)
  }
  const adder = one(ACCOUNT, { userId: me }) || {}
  const welcome = ec.resource.expand('${adder} added you to ${roomName}. You get its messages here as texts, and your replies go to the group. Reply STOP to leave.',
    '', { adder: adder.userFullName || adder.username || me, roomName: room.roomName || ec.l10n.localize('a group') })
  // the mapping as stored: a row its own transaction committed may not be seen from this one
  const welcomed = textOne({ chatRoomBridgeId: stored.chatRoomBridgeId, chatRoomId: room.chatRoomId, chatBridgeId: stored.chatBridgeId,
    externalRoomId: stored.externalRoomId }, welcome, 'welcome')
  return { chatActorId: stored.chatActorId, chatRoomBridgeId: stored.chatRoomBridgeId, ourNumber: pick.phoneNumber,
    warning: pick.pairRoomActive ? 'pairRoomActive' : null, welcomeSent: welcomed.sent, welcomeReasonCode: welcomed.reasonCode }
}

/** Internal (add#TextMember, in a transaction of its own): the external user and actor, the member, the mapping and the event. */
function storeTextMember(p) {
  const me = ec.user.userId
  const number = one(NUMBER, { phoneNumber: p.ourNumber })
  if (!number || blank(number.messagingBridgeId)) return refuse('noMessaging', 'That number has no messaging')
  const chatBridgeId = number.messagingBridgeId
  const now = ec.user.nowTimestamp
  // the room's row lock: two adds of one number at once make one member
  const room = find(ROOM).condition('chatRoomId', p.chatRoomId).forUpdate(true).one()
  if (!room) return refuse('notFound', 'Chat room not found')
  if (activeOn(p.ourNumber, p.theirNumber).length) return refuse('noFreeNumber', 'This person is already a text member of a room on this number; another number is needed.')
  const findExternal = () => Array.from(find(EXTERNAL).condition({ chatBridgeId, externalUserId: p.theirNumber }).list())[0]
  let external = findExternal()
  if (!external) {
    find(BRIDGE).condition('chatBridgeId', chatBridgeId).selectField('chatBridgeId').forUpdate(true).one()
    external = findExternal()
  }
  if (!external) {
    external = ec.entity.makeValue(EXTERNAL)
    external.setSequencedIdPrimary()
    // the number, never the adder's name for them: that is the room's (memberName)
    external.setFields({ chatBridgeId, externalUserId: p.theirNumber, displayName: p.theirNumber, createdDate: now }, true, null, null)
    external.create()
  }
  let actor = Array.from(find(ACTOR).condition('chatExternalUserId', external.chatExternalUserId).list())[0]
  if (!actor) {
    actor = ec.entity.makeValue(ACTOR)
    actor.setSequencedIdPrimary()
    actor.setFields({ kindEnumId: 'CakExternal', chatExternalUserId: external.chatExternalUserId, createdDate: now }, true, null, null)
    actor.create()
  }
  let member = one(MEMBER, { chatRoomId: room.chatRoomId, chatActorId: actor.chatActorId })
  if (member && !member.leftDate) return refuse('alreadyMember', 'This number is a text member of the room already')
  if (member) {
    member.setFields({ leftDate: null, joinedDate: now, roleEnumId: 'CmrMember', removedByUserId: null, addedByUserId: me,
      lastReadEventSeq: Number(room.lastEventSeq || 0) }, true, null, null)
    member.update()
  } else {
    member = ec.entity.makeValue(MEMBER)
    member.setFields({ chatRoomId: room.chatRoomId, chatActorId: actor.chatActorId, userId: null, roleEnumId: 'CmrMember', joinedDate: now,
      lastReadEventSeq: Number(room.lastEventSeq || 0), notificationLevelEnumId: 'CnlNone', isPinned: 'N', isMuted: 'N', addedByUserId: me }, true, null, null)
    member.create()
  }
  const mapping = ec.entity.makeValue(MAPPING)
  mapping.setSequencedIdPrimary()
  mapping.setFields({ chatRoomId: room.chatRoomId, chatBridgeId, externalRoomId: memberKey(p.ourNumber, p.theirNumber, room.chatRoomId),
    externalRoomName: p.memberName || p.theirNumber, directionEnumId: 'CrbBoth', fromDate: now, memberChatActorId: actor.chatActorId,
    memberName: p.memberName || null, addedByUserId: me, consentDate: now }, true, null, null)
  mapping.create()
  ec.service.sync().name('org.moqui.chat.ChatServices.append#Event').parameters({ chatRoomId: room.chatRoomId, eventTypeEnumId: 'CetMembership',
    senderUserId: me, content: { action: 'add', chatActorId: actor.chatActorId, byUserId: me, via: 'text' } }).disableAuthz().call()
  return { chatRoomBridgeId: mapping.chatRoomBridgeId, chatActorId: actor.chatActorId, chatBridgeId, externalRoomId: mapping.externalRoomId }
}

/**
 * One text to a text member's mapping, outside the room's timeline (the welcome, a HELP's reply): check#SmsSend in room mode
 * (reserved, in a transaction of its own), send#BridgeDirect, the count let go when it did not reach the provider. The
 * mapping keeps a welcome's outcome. Out: sent (the provider took it) and reasonCode (why not, or uncertain).
 */
function textOne(mapping, text, purpose) {
  const chatRoomBridgeId = mapping.chatRoomBridgeId
  const ours = oursOf(mapping)
  const theirs = theirsOf(mapping)
  let reasonCode = null
  let sent = false
  ec.message.pushErrors()
  try {
    const check = ec.service.sync().name('org.moqui.call.SmsServices.check#SmsSend').parameters({ fromNumber: ours, toNumber: theirs,
      chatRoomId: mapping.chatRoomId, reserve: true }).requireNewTransaction(true).disableAuthz().call() || {}
    if (ec.message.hasError()) reasonCode = 'failed'
    else if (!check.allowed) reasonCode = check.reasonCode || 'failed'
    else {
      const out = ec.service.sync().name('org.moqui.chat.ChatBridgeServices.send#BridgeDirect').parameters({ chatBridgeId: mapping.chatBridgeId,
        externalRoomId: mapping.externalRoomId, text }).disableAuthz().call() || {}
      sent = out.outcome === 'accepted'
      if (out.outcome !== 'accepted' && out.outcome !== 'uncertain') {
        reasonCode = out.reasonCode || out.outcome || 'failed'
        ec.service.sync().name('org.moqui.call.SmsServices.release#SmsSend').parameters({ chatRoomId: mapping.chatRoomId, counterDay: check.counterDay,
          fromNumber: ours }).requireNewTransaction(true).disableAuthz().call()
      } else if (out.outcome === 'uncertain') reasonCode = 'uncertain'
    }
  } catch (err) {
    ec.logger.error(`The ${purpose} text to text member ${theirs} of room ${mapping.chatRoomId} was not sent: ${message(err)}`)
    reasonCode = 'failed'
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
  if (reasonCode && reasonCode !== 'uncertain') ec.logger.warn(`The ${purpose} text to text member ${theirs} of room ${mapping.chatRoomId} was not sent: ${reasonCode}`)
  try {
    ec.transaction.runRequireNew(null, 'The text member\'s text was not noted', () => {
      const row = find(MAPPING).condition('chatRoomBridgeId', chatRoomBridgeId).forUpdate(true).one()
      if (!row) return
      if (purpose === 'welcome') row.setFields({ welcomeDate: sent || reasonCode === 'uncertain' ? ec.user.nowTimestamp : null, welcomeReasonCode: sent ? null : reasonCode }, true, null, null)
      else row.set('helpReplyDate', ec.user.nowTimestamp)
      row.update()
    })
  } catch (err) {
    ec.logger.error(`The ${purpose} text's outcome of mapping ${chatRoomBridgeId} was not kept: ${message(err)}`)
  }
  return { sent, reasonCode }
}

/** Internal (receive#Sms): a text member's STOP takes them out of the room; HELP is answered once a day. */
function noteMemberKeyword(p) {
  // HELP reads the row unlocked: its reply keeps its date in a transaction of its own, on this row
  const mapping = find(MAPPING).condition('chatRoomBridgeId', p.chatRoomBridgeId).forUpdate(p.keyword === 'STOP').one()
  if (!mapping || mapping.thruDate || !mapping.memberChatActorId) return { done: false }
  if (p.keyword === 'HELP') {
    if (mapping.helpReplyDate && ms(mapping.helpReplyDate) > nowMs() - DAY) return { done: false }
    const room = one(ROOM, { chatRoomId: mapping.chatRoomId }) || {}
    const text = ec.resource.expand('Texts of ${roomName} from MR Moqui. Reply STOP to leave.', '', { roomName: room.roomName || ec.l10n.localize('a group') })
    return { done: textOne(mapping, text, 'help').sent }
  }
  if (p.keyword !== 'STOP') return { done: false }
  endMapping(mapping)
  const member = one(MEMBER, { chatRoomId: mapping.chatRoomId, chatActorId: mapping.memberChatActorId })
  if (member && !member.leftDate) {
    member.set('leftDate', ec.user.nowTimestamp)
    member.update()
    ec.service.sync().name('org.moqui.chat.ChatServices.append#Event').parameters({ chatRoomId: mapping.chatRoomId, eventTypeEnumId: 'CetMembership',
      senderActorId: mapping.memberChatActorId, content: { action: 'leave', chatActorId: mapping.memberChatActorId, via: 'stop' } }).disableAuthz().call()
  }
  return { done: true }
}

function endMapping(mapping) {
  mapping.set('thruDate', ec.user.nowTimestamp)
  mapping.update()
}

/** Internal (the SECA after remove#Member of an external actor): a text member taken out has its mapping ended. */
function endTextMember(p) {
  if (blank(p.chatRoomId) || blank(p.chatActorId)) return { ended: 0 }
  const member = one(MEMBER, { chatRoomId: p.chatRoomId, chatActorId: p.chatActorId })
  if (!member || !member.leftDate) return { ended: 0 }
  let ended = 0
  for (const mapping of Array.from(find(MAPPING).condition({ chatRoomId: p.chatRoomId, memberChatActorId: p.chatActorId }).condition('thruDate', null).list())) {
    endMapping(mapping)
    ended++
  }
  return { ended }
}

/** The room's text members, for its members: names, numbers, our number, who added them, and their state. */
function findTextMembers(p) {
  const me = ec.user.userId
  const room = me ? one(ROOM, { chatRoomId: p.chatRoomId }) : null
  const roomAccess = room ? access(p.chatRoomId) : {}
  if (!room || !roomAccess.isMember) return refuse('notFound', 'Chat room not found')
  const mappings = Array.from(find(MAPPING).condition('chatRoomId', room.chatRoomId).condition('thruDate', null)
    .condition('memberChatActorId', 'not-equals', null).orderBy('fromDate').list())
  const adders = {}
  const adderIds = Array.from(new Set(mappings.map((row) => row.addedByUserId).filter((id) => id)))
  if (adderIds.length) for (const row of find(ACCOUNT).condition('userId', 'in', adderIds).list()) adders[row.userId] = row.userFullName || row.username
  const numbers = {}
  const ourIds = Array.from(new Set(mappings.map(oursOf).filter((id) => id)))
  if (ourIds.length) for (const row of find(NUMBER).condition('phoneNumber', 'in', ourIds).list()) numbers[row.phoneNumber] = row
  const memberList = mappings.map((row) => {
    const ours = oursOf(row)
    const theirs = theirsOf(row)
    const optOut = one(OPT_OUT, { phoneNumber: ours, otherNumber: theirs })
    const failure = Array.from(find('moqui.chat.ChatBridgeDelivery').condition('chatRoomBridgeId', row.chatRoomBridgeId)
      .orderBy('-createdDate').limit(1).list())[0]
    return { chatActorId: row.memberChatActorId, chatRoomBridgeId: row.chatRoomBridgeId, memberName: row.memberName || null, phoneNumber: theirs,
      ourNumber: ours, addedByName: adders[row.addedByUserId] || null, addedDate: ms(row.fromDate), welcomeDate: ms(row.welcomeDate),
      welcomeReasonCode: row.welcomeReasonCode || null, optedOut: !!optOut && optOut.optedOut === 'Y',
      roomTextsOff: (numbers[ours] || {}).roomTextsEnabled !== 'Y',
      lastFailure: failure && failure.statusEnumId === 'CdsFailed' ? failure.reasonCode || 'failed' : null }
  })
  const refusal = roomRefusal(room, roomAccess)
  return { memberList, canAdd: !refusal && roomNumbersOf(me).length > 0, addRefusal: refusal ? refusal.reasonCode : roomNumbersOf(me).length ? null : 'noRoomNumber',
    maxMembers: policy().smsRoomMaxMembers }
}

/** A moderator renames a text member in this room only. */
function renameTextMember(p) {
  const me = ec.user.userId
  const room = me ? one(ROOM, { chatRoomId: p.chatRoomId }) : null
  const roomAccess = room ? access(p.chatRoomId) : {}
  if (!room || !roomAccess.isMember) return refuse('notFound', 'Chat room not found')
  if (!roomAccess.canModerate) return refuse('notAllowed', 'Only a moderator or owner may rename a text member')
  const mapping = Array.from(find(MAPPING).condition({ chatRoomId: room.chatRoomId, memberChatActorId: p.chatActorId }).condition('thruDate', null).list())[0]
  if (!mapping) return refuse('notFound', 'Text member not found')
  const name = blank(p.memberName) ? null : String(p.memberName).trim().slice(0, 60)
  mapping.setFields({ memberName: name, externalRoomName: name || theirsOf(mapping) }, true, null, null)
  mapping.update()
  ec.service.sync().name('org.moqui.chat.ChatServices.record#UserChange').parameters({ userIdList: Array.from(find(MEMBER).condition('chatRoomId', room.chatRoomId)
    .condition('leftDate', null).condition('userId', 'not-equals', null).selectField('userId').list()).map((row) => row.userId),
  changeKind: 'room', chatRoomId: room.chatRoomId, change: { reason: 'members' } }).disableAuthz().call()
  return { memberName: name }
}

const operations = {
  routeOf, memberRoomName, textMemberChoices, addTextMember, storeTextMember, noteMemberKeyword, endTextMember, findTextMembers, renameTextMember,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown room texts operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
