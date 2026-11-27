// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Blocks in chat (plans/Comms-Messaging.md §18.3 phase 11): the services of org.moqui.chat.ChatBlockServices and the
// helpers ChatServices, Calls.js and SmsSpam.js call through ec.resource.script.
//
// - A person blocked in chat (ChatUserBlock): their messages are hidden from the user in shared rooms and notify nothing
//   (ChatServices: get#Timeline, search, the details lists, the counts, the pushes, activity, mentions, typing), their
//   calls do not ring the user (Calls.js ring), and get#RefusedUsers refuses them the user, as a user that does not
//   exist: no direct room with the user, no adding the user to a room. The user's one-to-one direct rooms with them go
//   to Spam & blocked.
// - A room out of a member's sight (ChatRoomMember.filterEnumId: CmfSpam from the spam filter, CmfBlocked from a block):
//   find#Rooms lists it only under its filter spam, it counts and notifies nothing, and no event of it is pushed to the
//   member. setFilters changes a user's rooms, each on its own, with one feed change (settings, filtered) for them all.
// - The room menu's Block (blockRoom): a pair room's number goes on the user's Blocked list (CallHandling.js
//   blockNumber), a one-to-one direct room's person is blocked, a group text's room is blocked for the user alone.
//
// The blocked person is told nothing; they still see the blocker in shared rooms and can still read the blocker's
// messages there. A person block refuses only what the blocked person starts: a direct room with the blocker, and adding
// the blocker to a private room (get#RefusedUsers with blocks). A room's admin who is not blocked still adds the blocker,
// and record and public rooms are never refused, so a block cannot keep anyone out of a team's rooms. Unblocking brings
// the hidden messages back, but not the mentions and Activity made during the block.
//
// The blocks are a preference, not a security boundary: blockedBy and blockersOf never fail (an error is logged and they
// answer nobody, so the message is pushed and the call rings everyone), and setFilters changes each room on its own.

const MEMBER = 'moqui.chat.ChatRoomMember'
const ROOM = 'moqui.chat.ChatRoom'
const BLOCK = 'moqui.chat.ChatUserBlock'
const MAPPING = 'moqui.chat.ChatRoomBridge'
const ACCOUNT = 'moqui.security.UserAccount'
const CALL_HANDLING = 'component://framework/service/org/moqui/call/CallHandling.js'
const FILTERS = ['CmfSpam', 'CmfBlocked']
const CODES = { CmfSpam: 'spam', CmfBlocked: 'blocked' }
/** The most people a user blocks. */
const MAX_BLOCKS = 500

const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const blank = (value) => value == null || String(value).trim() === ''
const ids = (list) => Array.from(new Set(Array.from(list || []).map((id) => String(id || '').trim()).filter((id) => id)))

function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

function callHandling(op, parameters) {
  return ec.resource.script(CALL_HANDLING).execute(op, parameters) || {}
}

// ---------- who is blocked ----------

/** The users userId blocked in chat; none (logged) when they cannot be read. */
function blockedBy(p) {
  if (blank(p.userId)) return { userIdList: [] }
  try {
    return { userIdList: Array.from(find(BLOCK).condition('userId', p.userId).selectField('blockedUserId').list()).map((row) => row.blockedUserId) }
  } catch (err) {
    ec.logger.error(`The chat blocks of ${p.userId} were not read, none applied: ${message(err)}`)
    return { userIdList: [] }
  }
}

const message = (err) => (err && err.message ? err.message : String(err))

/**
 * Of userIdList, those who blocked the caller: the local user senderUserId (ChatUserBlock), or the outside number
 * fromNumber (their Blocked list, CallHandling.js blockedUsers).
 */
function blockersOf(p) {
  const users = ids(p.userIdList)
  if (!users.length) return { userIdList: [] }
  try {
    const out = new Set()
    if (!blank(p.senderUserId)) {
      for (const row of find(BLOCK).condition('blockedUserId', p.senderUserId).condition('userId', 'in', users).selectField('userId').list()) out.add(row.userId)
    }
    if (!blank(p.fromNumber)) for (const id of callHandling('blockedUsers', { phoneNumber: p.fromNumber, userIdList: users }).userIdList || []) out.add(id)
    return { userIdList: users.filter((id) => out.has(id)) }
  } catch (err) {
    // a preference, not a boundary: everyone gets it
    ec.logger.error(`Blocks not read for ${p.senderUserId || p.fromNumber}, none applied: ${message(err)}`)
    return { userIdList: [] }
  }
}

/** Of userIdList, those who blocked the current user (get#RefusedUsers). */
function blockersOfMe(p) {
  const me = ec.user.userId
  return me ? blockersOf({ senderUserId: me, userIdList: p.userIdList }) : { userIdList: [] }
}

// ---------- the rooms out of sight ----------

/**
 * Internal: set the filter of userId's member rows in chatRoomIdList to filterEnumId (CmfSpam, CmfBlocked), or with none
 * take it off; onlyFrom takes off only that filter, and keep (the default for a set) leaves a row that has another.
 * One feed change a room (settings, with filtered) and the user's counts published. Out: changedRoomIdList.
 */
function setFilters(p) {
  const userId = p.userId
  const rooms = ids(p.chatRoomIdList)
  const filter = blank(p.filterEnumId) ? null : String(p.filterEnumId)
  if (blank(userId) || !rooms.length) return { changedRoomIdList: [] }
  if (filter && !FILTERS.includes(filter)) return refuse('badFilter', 'Not a room filter')
  const changed = []
  const now = ec.user.nowTimestamp
  for (const chatRoomId of rooms) {
    // each room on its own: one that fails is logged and the rest go on
    try {
      const member = one(MEMBER, { chatRoomId, userId })
      if (!member) continue
      const current = member.filterEnumId || null
      if (filter ? current === filter || (current && p.replace !== true) : !current || (p.onlyFrom && current !== p.onlyFrom)) continue
      member.setFields({ filterEnumId: filter, filterDate: filter ? now : null }, true, null, null)
      member.update()
      changed.push(member)
    } catch (err) {
      ec.logger.error(`Chat room ${chatRoomId}: its filter for ${userId} was not changed: ${message(err)}`)
    }
  }
  if (changed.length) {
    const settingsOf = (row) => ({ notificationLevelEnumId: row.notificationLevelEnumId ?? null, isMuted: row.isMuted === 'Y', isPinned: row.isPinned === 'Y',
      archivedEventSeq: row.archivedEventSeq != null ? Number(row.archivedEventSeq) : null, filtered: CODES[row.filterEnumId] || null })
    ec.service.sync().name('org.moqui.chat.ChatServices.record#UserChange').parameter('changeList', changed.map((row) => ({ userIdList: [userId],
      changeKind: 'settings', chatRoomId: row.chatRoomId, change: settingsOf(row),
      pushMessage: Object.assign({ kind: 'room', action: 'settings', chatRoomId: row.chatRoomId }, settingsOf(row)) }))).call()
    ec.service.sync().name('org.moqui.chat.ChatServices.publish#ChatCounts').parameter('userIdList', [userId]).call()
  }
  return { changedRoomIdList: changed.map((row) => row.chatRoomId) }
}

/** The one-to-one direct rooms (plain and encrypted) of userId and otherUserId. */
function directRoomsOf(userId, otherUserId) {
  const keys = [[userId, otherUserId].sort().join(',')]
  keys.push('e2ee:' + keys[0])
  return Array.from(find(ROOM).condition('roomTypeEnumId', 'CrtDirect').condition('directKey', 'in', keys).selectField('chatRoomId').list())
    .map((row) => row.chatRoomId)
}

/** A phone room's mapping: its outside numbers (one for a pair room, several for a group text). */
function phoneNumbersOf(chatRoomId) {
  const mapping = Array.from(find(MAPPING).condition('chatRoomId', chatRoomId).condition('thruDate', null).list())
    .find((row) => String(row.externalRoomId || '').includes('/'))
  if (!mapping) return []
  return String(mapping.externalRoomId).split('/')[1].split(',').filter((number) => number)
}

/**
 * Internal (CallHandling.js, after the user's Blocked list changed): each of the user's pair rooms is blocked for them
 * while its number is on the list, and in sight again (from CmfBlocked) when it is not. Group texts keep their own.
 * Each room on its own: one that fails is logged and the rest go on.
 */
function refilterPhoneRooms(p) {
  const userId = p.userId
  if (blank(userId)) return {}
  const memberRooms = Array.from(find(MEMBER).condition('userId', userId).condition('leftDate', null).selectField('chatRoomId').selectField('filterEnumId').list())
  if (!memberRooms.length) return {}
  const phoneRooms = new Set(Array.from(find(ROOM).condition('chatRoomId', 'in', memberRooms.map((row) => row.chatRoomId))
    .condition('roomTypeEnumId', 'CrtExternal').selectField('chatRoomId').list()).map((row) => row.chatRoomId))
  const block = []
  const clear = []
  for (const member of memberRooms) {
    if (!phoneRooms.has(member.chatRoomId)) continue
    try {
      const numbers = phoneNumbersOf(member.chatRoomId)
      if (numbers.length !== 1) continue
      const blocked = (callHandling('blockedUsers', { phoneNumber: numbers[0], userIdList: [userId] }).userIdList || []).length > 0
      if (blocked && member.filterEnumId !== 'CmfBlocked') block.push(member.chatRoomId)
      else if (!blocked && member.filterEnumId === 'CmfBlocked') clear.push(member.chatRoomId)
    } catch (err) {
      ec.logger.error(`Chat room ${member.chatRoomId}: its block was not checked for ${userId}: ${err && err.message ? err.message : err}`)
    }
  }
  // setFilters changes each room on its own
  if (block.length) setFilters({ userId, chatRoomIdList: block, filterEnumId: 'CmfBlocked', replace: true })
  if (clear.length) setFilters({ userId, chatRoomIdList: clear, onlyFrom: 'CmfBlocked' })
  return { blockedCount: block.length, clearedCount: clear.length }
}

// ---------- block a person ----------

function userName(userId) {
  const account = one(ACCOUNT, { userId })
  return account ? account.userFullName || account.username || userId : userId
}

/** Block blockedUserId (a local user, not oneself) for the current user; their direct rooms with them go out of sight. */
function blockUser(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const other = String(p.blockedUserId || '').trim()
  if (!other || other === me || !one(ACCOUNT, { userId: other })) return refuse('notFound', 'Person not found')
  if (!one(BLOCK, { userId: me, blockedUserId: other })) {
    if (find(BLOCK).condition('userId', me).count() >= MAX_BLOCKS) return refuse('tooMany', 'You have blocked too many people')
    const row = ec.entity.makeValue(BLOCK)
    row.setFields({ userId: me, blockedUserId: other, blockedDate: ec.user.nowTimestamp }, true, null, null)
    row.create()
  }
  setFilters({ userId: me, chatRoomIdList: directRoomsOf(me, other), filterEnumId: 'CmfBlocked', replace: true })
  // out of the live locations the user shares in chat rooms, in a transaction of its own, never failing the block
  // (plans/Location-Sharing.md §3.5)
  ec.resource.script('component://framework/service/org/moqui/chat/ChatLocation.js').execute('revokeLive', { ownerUserId: me, userId: other })
  return { blockedUserId: other }
}

function unblockUser(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const other = String(p.blockedUserId || '').trim()
  const row = other ? one(BLOCK, { userId: me, blockedUserId: other }) : null
  if (row) row.delete()
  if (other) setFilters({ userId: me, chatRoomIdList: directRoomsOf(me, other), onlyFrom: 'CmfBlocked' })
  return {}
}

/** The people the current user blocked: blockedList [{userId, name, blockedDate}], by name. */
function findBlocked() {
  const me = ec.user.userId
  if (!me) return { blockedList: [] }
  return { blockedList: Array.from(find(BLOCK).condition('userId', me).list()).map((row) => ({ userId: row.blockedUserId,
    name: userName(row.blockedUserId), blockedDate: row.blockedDate })).sort((a, b) => String(a.name).localeCompare(String(b.name))) }
}

// ---------- the room menu ----------

/** The current user's live member row of a room, and the room; a refusal when they are not in it. */
function myRoom(chatRoomId) {
  const me = ec.user.userId
  if (!me) return { refusal: refuse('notSignedIn', 'Sign in first') }
  const member = chatRoomId ? one(MEMBER, { chatRoomId, userId: me }) : null
  const room = member ? one(ROOM, { chatRoomId }) : null
  if (!member || !room || (member.leftDate && room.roomTypeEnumId !== 'CrtDirect')) return { refusal: refuse('notFound', 'Chat room not found') }
  return { me, member, room }
}

/** The other person of a one-to-one direct room, or null. */
function otherOf(room, me) {
  if (room.roomTypeEnumId !== 'CrtDirect') return null
  const others = Array.from(find(MEMBER).condition('chatRoomId', room.chatRoomId).selectField('userId').list())
    .map((row) => row.userId).filter((id) => id && id !== me)
  return others.length === 1 ? others[0] : null
}

/** The kind of block a room's Block makes: number, person or room (a group text), with what it blocks; null for none. */
function blockKindOf(room, me) {
  if (room.roomTypeEnumId === 'CrtExternal') {
    const numbers = phoneNumbersOf(room.chatRoomId)
    if (numbers.length === 1) return { kind: 'number', phoneNumber: numbers[0] }
    return numbers.length ? { kind: 'room' } : null
  }
  const other = otherOf(room, me)
  return other ? { kind: 'person', userId: other } : null
}

/** The room menu's Block: the pair room's number, the direct room's person, or the group text's room for the user. */
function blockRoom(p) {
  const found = myRoom(p.chatRoomId)
  if (found.refusal) return found.refusal
  const kind = blockKindOf(found.room, found.me)
  if (!kind) return refuse('notBlockable', 'Block a phone conversation or a direct chat with one person')
  if (kind.kind === 'number') {
    const out = callHandling('blockNumber', { phoneNumber: kind.phoneNumber })
    if (ec.message.hasError()) return out
  } else if (kind.kind === 'person') blockUser({ blockedUserId: kind.userId })
  else setFilters({ userId: found.me, chatRoomIdList: [found.room.chatRoomId], filterEnumId: 'CmfBlocked', replace: true })
  return { blocked: kind.kind, phoneNumber: kind.phoneNumber || null, blockedUserId: kind.userId || null }
}

/** The room's Unblock: as blockRoom made it; a room in Spam also comes back (Not spam is SmsSpam.js's). */
function unblockRoom(p) {
  const found = myRoom(p.chatRoomId)
  if (found.refusal) return found.refusal
  const kind = blockKindOf(found.room, found.me)
  if (kind && kind.kind === 'number') callHandling('unblockNumber', { phoneNumber: kind.phoneNumber, exactOnly: false })
  else if (kind && kind.kind === 'person') unblockUser({ blockedUserId: kind.userId })
  else {
    setFilters({ userId: found.me, chatRoomIdList: [found.room.chatRoomId], onlyFrom: 'CmfBlocked' })
    // a group text's report is withdrawn with its block (SmsSpam.js)
    ec.resource.script('component://framework/service/org/moqui/call/SmsSpam.js').execute('withdrawReport', { userId: found.me, chatRoomId: found.room.chatRoomId })
  }
  // a prefix on the Blocked list may still hold the pair room: it says so
  const after = one(MEMBER, { chatRoomId: found.room.chatRoomId, userId: found.me })
  if (after && after.filterEnumId === 'CmfBlocked') return refuse('stillBlocked', 'A prefix on your Blocked list still blocks this number; remove it in Call handling')
  return {}
}

/** What the room's banner and menu say: filtered (spam, blocked), and what Block would block (number, person, room). */
function roomBlock(p) {
  const found = myRoom(p.chatRoomId)
  if (found.refusal) return found.refusal
  const kind = blockKindOf(found.room, found.me)
  return { filtered: CODES[found.member.filterEnumId] || null, blockKind: kind ? kind.kind : null, phoneNumber: kind && kind.phoneNumber || null,
    phoneNumberList: kind && kind.kind === 'room' ? phoneNumbersOf(found.room.chatRoomId) : [],
    blockedUserId: kind && kind.userId || null, blockedName: kind && kind.userId ? userName(kind.userId) : null,
    personBlocked: !!(kind && kind.userId && one(BLOCK, { userId: found.me, blockedUserId: kind.userId })) }
}

const operations = {
  blockedBy, blockersOf, blockersOfMe, setFilters, refilterPhoneRooms, blockUser, unblockUser, findBlocked, blockRoom, unblockRoom, roomBlock,
  phoneNumbers: (p) => ({ numberList: phoneNumbersOf(p.chatRoomId) }),
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown chat block operation ' + op)
  return operations[op](parameters || {}) || {}
}

;({ execute })
