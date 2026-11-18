// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// One person across networks (plans/Comms-Messaging.md §11.1, §18.3 phase 22): the services of
// org.moqui.chat.ChatPeopleServices, and mailPeople for find#DirectChatMail.
//
// A person is one user's grouping of the addresses they reach someone by: user:userId (a user of this server), tel:+E.164
// (an outside number: its SMS and RCS rooms), email:address and ext:chatExternalUserId (a person on a bridge's network,
// GroupMe). The people of a user are made, each time, from:
// 1. the user's rooms: a direct room of two (user:), a pair room of one outside number (tel:), a GroupMe direct
//    conversation (ext:);
// 2. the user's own rows (ChatPerson, ChatPersonAddress), which win: an address with a row is in that row's person;
// 3. the contacts hook (find#ContactPeople, the app's contacts the user can see), asked only about the addresses of the
//    user's rooms that have no row, which joins those it knows into one person and brings their other addresses. Its
//    answer is kept a few minutes for the user (a cache shared by their tabs, devices and the mail line), so the room
//    list spends one of the hour's asks;
// 4. each address left alone is a person of its own.
//
// Privacy:
// - a person who is a user of this server shows only the numbers and addresses of the viewer's own rooms and rows, never
//   what the hook or the user account gave (§14.4), and chat never links a number to a user from its own or voice's data;
// - a user that get#RefusedUsers refuses (an app's chat-target, or one who blocked the viewer), a disabled user and an
//   external user are as users that do not exist: a hook contact that names one is left out whole (no name, no other
//   address), so the person shows only as the addresses of the viewer's own rooms; a room the viewer already has with
//   them still shows, with no new direct chat and no nudge;
// - an address typed into a merge is matched only against what the viewer is shown, and joins a user of this server only
//   when it is already shown to the viewer, so typing cannot probe what the hook knows;
// - no other user's rows are read; nothing here reaches the person.
//
// Failure tolerance: the hook failing or skipped (the hour's asks used) leaves the people of the rooms and rows; a change
// that needs the hook's grouping (merge, rename) is then refused, so no partial rows are written. A room whose mapping
// cannot be read is logged and left out.

const MEMBER = 'moqui.chat.ChatRoomMember'
const PERSON_ROOM = 'moqui.chat.ChatPersonRoom'
const INBOUND = 'moqui.chat.ChatRoomInboundLast'
const MAPPING = 'moqui.chat.ChatRoomBridge'
const BRIDGE = 'moqui.chat.ChatBridge'
const EXT_USER = 'moqui.chat.ChatExternalUser'
const BLOCK = 'moqui.chat.ChatUserBlock'
const PERSON = 'moqui.chat.ChatPerson'
const ADDRESS = 'moqui.chat.ChatPersonAddress'
const ACCOUNT = 'moqui.security.UserAccount'
const NUMBER = 'moqui.call.PhoneNumber'
const OPT_OUT = 'moqui.call.SmsOptOut'
const CALLER_LIST = 'moqui.call.CallerList'
const CALLER_ENTRY = 'moqui.call.CallerListEntry'
const VOICE = 'component://framework/service/org/moqui/call/Voice.js'
const SMS = 'component://framework/service/org/moqui/call/Sms.js'

/** The most addresses a person holds, rows a user keeps, rooms read, and hook asks an hour a user. */
const MAX_ADDRESSES = 20
const MAX_ROWS = 2000
const MAX_ROOMS = 200
const HOOK_PER_HOUR = 60
/** The most new texts a person's number offers (one a number of the user's). */
const MAX_TEXT_WAYS = 3
const NETWORKS = { CbtSms: 'sms', CbtRcs: 'rcs', CbtGroupMe: 'groupme' }

const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const blank = (value) => value == null || String(value).trim() === ''
const message = (err) => (err && err.message ? err.message : String(err))
const toMillis = (value) => {
  if (value == null || value === '') return null
  if (typeof value === 'number') return value
  if (value instanceof Date) return value.getTime()
  const number = Number(value)
  return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}
const chunks = (list, size) => { const out = []; for (let i = 0; i < list.length; i += size) out.push(list.slice(i, i + size)); return out }
const kindOf = (key) => String(key).split(':')[0]
const valueOf = (key) => String(key).slice(String(key).indexOf(':') + 1)
const lowest = (keys) => keys.slice().sort()[0]

function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

/**
 * A call whose errors are its own: the caller's messages are kept aside (pushErrors) and back after (popErrors), so
 * nothing the caller had is cleared. Answers {out, failed}.
 */
function quietly(run) {
  ec.message.pushErrors()
  try {
    const out = run()
    return { out, failed: ec.message.hasError() }
  } catch (err) {
    ec.logger.warn(`Chat people: ${message(err)}`)
    return { out: null, failed: true }
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
}

/** A key as chat writes it, or null: tel: in E.164 (through the deployment's numbering), email: lower case, user:, ext:. */
function normalKey(text) {
  const raw = String(text == null ? '' : text).trim()
  const at = raw.indexOf(':')
  if (at < 1) return null
  const kind = raw.slice(0, at).toLowerCase()
  const rest = raw.slice(at + 1).trim()
  if (!rest) return null
  if (kind === 'tel') {
    const number = ec.resource.script(VOICE).execute('toE164', { phoneNumber: rest })?.number
    return number && /^\+[0-9]{5,15}$/.test(number) ? 'tel:' + number : null
  }
  if (kind === 'email') {
    const address = rest.toLowerCase()
    return /^[^\s@,;<>"]+@[^\s@,;<>"]+\.[^\s@,;<>"]+$/.test(address) && address.length <= 254 ? 'email:' + address : null
  }
  if (kind === 'user') return /^[A-Za-z0-9_.@-]{1,64}$/.test(rest) ? 'user:' + rest : null
  if (kind === 'ext') return /^[A-Za-z0-9_.:-]{1,64}$/.test(rest) ? 'ext:' + rest : null
  return null
}

// ---------- the user's rooms ----------

/**
 * The rooms of me that are one person's, the MAX_ROOMS newest in sight (one query): each {chatRoomId, addressKey, network
 * (moqui, sms, rcs, groupme), encrypted, fromNumber, lastMessageDate, lastInboundDate, notConnected, chatBridgeId}. A
 * fixed number of queries whatever the count of rooms: the rooms, the direct rooms' members, the mappings, their
 * bridges, the GroupMe people, the latest message from the other side.
 */
function roomsOf(me) {
  const newest = Array.from(find(PERSON_ROOM).condition('userId', me).condition('leftDate', null).condition('filterEnumId', null)
    .condition('roomTypeEnumId', 'in', ['CrtDirect', 'CrtExternal', 'CrtPrivate'])
    .selectField('chatRoomId,roomTypeEnumId,encrypted,lastEventDate,lastMessageDate').orderBy('-lastEventDate').limit(MAX_ROOMS).list())
  const out = []
  // direct rooms of two: the other member
  const direct = newest.filter((room) => room.roomTypeEnumId === 'CrtDirect').map((room) => room.chatRoomId)
  const membersOf = {}
  if (direct.length) {
    for (const row of find(MEMBER).condition('chatRoomId', 'in', direct).selectField('chatRoomId,userId').list()) {
      if (row.userId) (membersOf[row.chatRoomId] = membersOf[row.chatRoomId] || new Set()).add(row.userId)
    }
  }
  // bridged rooms: their mapping, bridge and GroupMe person
  const bridged = newest.filter((room) => room.roomTypeEnumId !== 'CrtDirect').map((room) => room.chatRoomId)
  const mappingOf = {}
  if (bridged.length) {
    for (const row of find(MAPPING).condition('chatRoomId', 'in', bridged).condition('thruDate', null)
      .selectField('chatRoomId,chatBridgeId,externalRoomId').list()) if (!mappingOf[row.chatRoomId]) mappingOf[row.chatRoomId] = row
  }
  const bridgeIds = Array.from(new Set(Object.values(mappingOf).map((row) => row.chatBridgeId)))
  const bridgeOf = {}
  if (bridgeIds.length) for (const row of find(BRIDGE).condition('chatBridgeId', 'in', bridgeIds).selectField('chatBridgeId,typeEnumId,statusId').list()) bridgeOf[row.chatBridgeId] = row
  const groupmeIds = Object.values(mappingOf).filter((row) => bridgeOf[row.chatBridgeId]?.typeEnumId === 'CbtGroupMe' && String(row.externalRoomId || '').startsWith('dm:'))
    .map((row) => String(row.externalRoomId).slice(3))
  const externalOf = {}
  if (groupmeIds.length) {
    for (const row of find(EXT_USER).condition('externalUserId', 'in', Array.from(new Set(groupmeIds))).selectField('chatExternalUserId,chatBridgeId,externalUserId').list()) {
      externalOf[row.chatBridgeId + '/' + row.externalUserId] = row.chatExternalUserId
    }
  }
  const inboundOf = {}
  if (bridged.length) {
    for (const row of find(INBOUND).condition('chatRoomId', 'in', bridged).condition('senderUserId', null)
      .selectField('chatRoomId,lastInboundDate').list()) inboundOf[row.chatRoomId] = toMillis(row.lastInboundDate)
  }
  for (const room of newest) {
    const base = { chatRoomId: room.chatRoomId, encrypted: room.encrypted === 'Y', lastMessageDate: toMillis(room.lastMessageDate) ?? toMillis(room.lastEventDate),
      fromNumber: null, lastInboundDate: null, notConnected: false, chatBridgeId: null }
    if (room.roomTypeEnumId === 'CrtDirect') {
      const others = Array.from(membersOf[room.chatRoomId] || []).filter((id) => id !== me)
      if (others.length === 1) out.push(Object.assign(base, { addressKey: 'user:' + others[0], network: 'moqui' }))
      continue
    }
    const mapping = mappingOf[room.chatRoomId]
    if (!mapping) continue
    try {
      const bridge = bridgeOf[mapping.chatBridgeId]
      const network = bridge ? NETWORKS[bridge.typeEnumId] : null
      if (!network) continue
      const external = String(mapping.externalRoomId || '')
      let addressKey = null
      let fromNumber = null
      if (network === 'groupme') {
        const id = external.startsWith('dm:') ? externalOf[mapping.chatBridgeId + '/' + external.slice(3)] : null
        if (id) addressKey = 'ext:' + id
      } else if (room.roomTypeEnumId === 'CrtExternal') {
        const [ours, theirs] = external.split('/')
        if (ours && theirs && !theirs.includes(',') && /^\+[0-9]{5,15}$/.test(theirs)) { addressKey = 'tel:' + theirs; fromNumber = ours }
      }
      if (!addressKey) continue
      out.push(Object.assign(base, { addressKey, network, fromNumber, chatBridgeId: mapping.chatBridgeId,
        notConnected: !!bridge.statusId && bridge.statusId !== 'CbsConnected', lastInboundDate: inboundOf[room.chatRoomId] ?? null }))
    } catch (err) {
      ec.logger.error(`The person of chat room ${room.chatRoomId} was not read, it is left out: ${message(err)}`)
    }
  }
  return out
}

// ---------- the contacts hook ----------

/** Whether me may ask the hook now: at most HOOK_PER_HOUR times an hour, a slot each claimed with putIfAbsent. */
function hookSlot(me) {
  const asked = ec.cache.getCache('moqui.chat.people.hook.asked')
  const hourKey = me + ':' + Math.floor(toMillis(ec.user.nowTimestamp) / 3600000)
  let slot = Math.max(0, Number(asked.get(hourKey) || 0))
  while (slot < HOOK_PER_HOUR) {
    if (asked.putIfAbsent(hourKey + ':' + slot, 1)) { asked.put(hourKey, slot + 1); return true }
    slot++
  }
  return false
}

/**
 * The hook's contacts for these addresses: {contactList (null: no hook), failed, skipped}. The last answer for the user is
 * kept a few minutes (moqui.chat.people.contacts) and answers again while it was asked about every one of these.
 */
function contactsFor(me, addressKeys) {
  if (!addressKeys.length) return { contactList: [] }
  const cache = ec.cache.getCache('moqui.chat.people.contacts')
  let kept = null
  try { kept = cache.get(me) ? JSON.parse(String(cache.get(me))) : null } catch (err) { kept = null }
  if (kept && Array.isArray(kept.asked)) {
    const known = new Set(kept.asked)
    if (addressKeys.every((key) => known.has(key))) return { contactList: kept.contactList == null ? null : kept.contactList }
  }
  if (!hookSlot(me)) return { contactList: [], skipped: true }
  const asked = addressKeys.slice(0, 300)
  const call = quietly(() => ec.service.sync().name('org.moqui.chat.ChatPeopleServices.find#ContactPeople')
    .parameters({ userId: me, addressList: asked }).disableAuthz().call() || {})
  const out = call.out || {}
  if (call.failed || out.hookFailed) return { contactList: [], failed: true }
  const contactList = out.contactList == null ? null : Array.from(out.contactList)
  try { cache.put(me, JSON.stringify({ asked, contactList })) } catch (err) { ec.logger.warn(`The contacts of ${me} were not kept: ${message(err)}`) }
  return { contactList }
}

/** Of these users, those me may reach: an enabled account, not an external user, not refused (with the blocks). */
function reachableOf(me, userIds, accountOf) {
  const asked = userIds.filter((id) => id !== me && accountOf[id] && accountOf[id].disabled !== 'Y')
  if (!asked.length) return new Set()
  const external = quietly(() => ec.service.sync().name('org.moqui.chat.ChatServices.get#ExternalUsers').parameter('userIdList', asked)
    .disableAuthz().call())
  // a failed check leaves everyone out
  const outside = new Set(external.failed ? asked : Array.from(external.out?.externalUserIdList || []))
  const inside = asked.filter((id) => !outside.has(id))
  if (!inside.length) return new Set()
  const refused = quietly(() => ec.service.sync().name('org.moqui.chat.ChatServices.get#RefusedUsers')
    .parameters({ userIdList: inside, blocks: true }).call())
  if (refused.failed) return new Set()
  const out = new Set(Array.from(refused.out?.refusedUserIdList || []))
  return new Set(inside.filter((id) => !out.has(id)))
}

// ---------- the people ----------

/**
 * The people of me, grouped as the header says. Each person (internal form): {personKey, chatPersonId, displayName,
 * userId, reachable, account, contact {name, url}, keys, roomAddresses (Set), rowAddresses (Set), rooms}. Out: {people,
 * roomRows, hookFailed, contactsSkipped, accountOf}.
 */
function groupPeople(me) {
  const rooms = roomsOf(me)
  const rows = Array.from(find(ADDRESS).condition('ownerUserId', me).selectField('addressKey,chatPersonId').limit(MAX_ROWS).list())
  const persons = {}
  for (const row of find(PERSON).condition('ownerUserId', me).selectField('chatPersonId,displayName').list()) persons[row.chatPersonId] = row
  const ownerOf = new Map(rows.filter((row) => persons[row.chatPersonId]).map((row) => [row.addressKey, row.chatPersonId]))
  const roomAddresses = Array.from(new Set(rooms.map((room) => room.addressKey)))
  const unowned = roomAddresses.filter((key) => !ownerOf.has(key) && kindOf(key) !== 'ext')
  const hook = contactsFor(me, unowned)
  const contacts = Array.from(hook.contactList || []).map((contact) => ({ name: contact.name || null, url: contact.url || null,
    keys: Array.from(contact.addressList || []).map(String) }))
  // the users named anywhere, read and checked once
  const roomUsers = new Set(roomAddresses.filter((key) => kindOf(key) === 'user').map(valueOf))
  const allUsers = Array.from(new Set(roomAddresses.concat(Array.from(ownerOf.keys()), contacts.flatMap((contact) => contact.keys))
    .filter((key) => kindOf(key) === 'user').map(valueOf)))
  const accountOf = {}
  for (const chunk of chunks(allUsers, 500)) {
    for (const row of find(ACCOUNT).condition('userId', 'in', chunk).selectField('userId,username,userFullName,disabled,emailAddress').list()) accountOf[row.userId] = row
  }
  const reachable = reachableOf(me, allUsers, accountOf)
  // a room's user stays (the room is the viewer's); another only when reachable
  const userOk = (id) => roomUsers.has(id) || reachable.has(id)
  // union-find over the addresses
  const parent = new Map()
  const root = (key) => { if (!parent.has(key)) parent.set(key, key); let at = key; while (parent.get(at) !== at) at = parent.get(at); parent.set(key, at); return at }
  const join = (a, b) => { const ra = root(a), rb = root(b); if (ra !== rb) parent.set(rb, ra) }
  for (const key of roomAddresses) root(key)
  const byPerson = {}
  for (const [key, personId] of ownerOf) {
    if (kindOf(key) === 'user' && !userOk(valueOf(key))) continue
    root(key)
    ;(byPerson[personId] = byPerson[personId] || []).push(key)
  }
  for (const keys of Object.values(byPerson)) for (const key of keys.slice(1)) join(keys[0], key)
  // the hook: a contact that names a user the viewer may not reach is left out whole; the rest join only addresses with
  // no row, among themselves, and name the person
  const contactOf = new Map()
  for (const contact of contacts) {
    if (contact.keys.some((key) => kindOf(key) === 'user' && !reachable.has(valueOf(key)))) continue
    const keys = contact.keys.filter((key) => !ownerOf.has(key))
    if (!keys.some((key) => unowned.includes(key))) continue
    for (const key of keys) root(key)
    for (const key of keys.slice(1)) join(keys[0], key)
    for (const key of keys) if (!contactOf.has(key)) contactOf.set(key, { name: contact.name, url: contact.url })
  }
  const groups = new Map()
  for (const key of parent.keys()) {
    const top = root(key)
    if (!groups.has(top)) groups.set(top, [])
    groups.get(top).push(key)
  }
  const people = []
  const roomRows = {}
  for (const keys of groups.values()) {
    const owned = keys.map((key) => ownerOf.get(key)).find((id) => id)
    const theRooms = rooms.filter((room) => keys.includes(room.addressKey))
    if (!theRooms.length && !owned) continue
    const userId = keys.filter((key) => kindOf(key) === 'user').map(valueOf)[0] || null
    const contact = owned ? null : keys.map((key) => contactOf.get(key)).find((found) => found && (found.name || found.url)) || null
    const person = {
      // stable: a person the user changed by its row, else its lowest address, whatever room was last
      personKey: owned ? 'p:' + owned : 'a:' + lowest(keys),
      chatPersonId: owned || null, displayName: owned ? persons[owned].displayName || null : null, userId,
      reachable: !userId || reachable.has(userId), account: userId ? accountOf[userId] || null : null,
      contact, keys, roomAddresses: new Set(theRooms.map((room) => room.addressKey)), rowAddresses: new Set(keys.filter((key) => ownerOf.has(key))), rooms: theRooms,
    }
    people.push(person)
    for (const room of theRooms) roomRows[room.chatRoomId] = person.personKey
  }
  people.sort((a, b) => Math.max(0, ...b.rooms.map((room) => room.lastMessageDate || 0)) - Math.max(0, ...a.rooms.map((room) => room.lastMessageDate || 0)))
  return { people, roomRows, hookFailed: !!hook.failed, contactsSkipped: !!hook.skipped, accountOf }
}

/** The addresses a person shows: all of an outside person's; of a user's, only the viewer's own rooms and rows (§14.4). */
function shownKeys(person) {
  if (!person.userId) return person.keys.slice()
  return person.keys.filter((key) => kindOf(key) === 'user' || person.roomAddresses.has(key) || person.rowAddresses.has(key))
}

/** The name of a person and where it came from: mine, user, contact, or none. */
function nameOf(person) {
  if (!blank(person.displayName)) return { name: person.displayName, nameFrom: 'mine' }
  if (person.account) return { name: person.account.userFullName || person.account.username || null, nameFrom: 'user' }
  if (person.contact && person.contact.name) return { name: person.contact.name, nameFrom: 'contact' }
  return { name: null, nameFrom: null }
}

/** A shown address: {addressKey, kind, label}. */
function addressOf(person, key, externalOf) {
  const kind = kindOf(key)
  const value = valueOf(key)
  if (kind === 'user') return { addressKey: key, kind, label: person.account && person.account.username ? '@' + person.account.username : null }
  if (kind === 'ext') { const ext = externalOf[value]; return { addressKey: key, kind, label: ext ? ext.displayName || null : null, network: ext ? ext.network : null } }
  return { addressKey: key, kind, label: value }
}

/** The person's ways, in order, the best first and marked (plans/Comms-Messaging.md §18.3 phase 22). */
function waysOf(person, context) {
  if (person.userId && context.blockedUsers.has(person.userId)) return { ways: [], best: null, blocked: true }
  const usable = (way) => !way.blocked && !way.reasonCode
  const ways = []
  const roomWay = (room) => {
    const way = { kind: 'room', network: room.network, chatRoomId: room.chatRoomId, encrypted: room.encrypted, fromNumber: room.fromNumber,
      number: kindOf(room.addressKey) === 'tel' ? valueOf(room.addressKey) : null, lastMessageDate: room.lastMessageDate, lastInboundDate: room.lastInboundDate }
    if (way.number && context.blockedNumber(way.number)) way.blocked = true
    else if (way.number && room.fromNumber && context.optedOut(room.fromNumber, way.number)) way.reasonCode = 'optedOut'
    else if (room.notConnected) way.reasonCode = 'notConnected'
    return way
  }
  const moqui = person.rooms.filter((room) => room.network === 'moqui').sort((a, b) => (b.encrypted ? 1 : 0) - (a.encrypted ? 1 : 0))
  for (const room of moqui) ways.push(roomWay(room))
  // a new direct chat only with a user the viewer may reach
  if (person.userId && person.reachable && !moqui.length) ways.push({ kind: 'direct', network: 'moqui', userId: person.userId })
  const bridged = person.rooms.filter((room) => room.network !== 'moqui').map(roomWay)
    .sort((a, b) => (b.lastMessageDate || 0) - (a.lastMessageDate || 0))
  ways.push(...bridged)
  // new texts: a number of the person's, from each of my numbers that has no room with it yet
  const shown = shownKeys(person)
  for (const key of shown.filter((found) => kindOf(found) === 'tel')) {
    const number = valueOf(key)
    let offered = 0
    for (const mine of context.textNumbers) {
      if (offered >= MAX_TEXT_WAYS) break
      if (person.rooms.some((room) => room.addressKey === key && room.fromNumber === mine.phoneNumber)) continue
      const way = { kind: 'text', network: mine.network, number, fromNumber: mine.phoneNumber }
      if (context.blockedNumber(number)) way.blocked = true
      else if (context.optedOut(mine.phoneNumber, number)) way.reasonCode = 'optedOut'
      else if (mine.notConnected) way.reasonCode = 'notConnected'
      ways.push(way)
      offered++
    }
  }
  // mail: an outside person's addresses, in the Inbox's Compose (a user's address is never shown)
  if (!person.userId) {
    const compose = (System.getProperty('comms_app_root') || '/qapps/BasicComms') + '/Inbox/Compose?to='
    for (const key of shown.filter((found) => kindOf(found) === 'email')) ways.push({ kind: 'mail', network: 'mail', address: valueOf(key), url: compose + encodeURIComponent(valueOf(key)) })
  }
  // the best: Moqui chat; else the network they last wrote on; else the SMS room with the latest message from either side,
  // else a new text; else mail
  let best = ways.find((way) => way.network === 'moqui' && usable(way)) || null
  if (!best) {
    const wrote = ways.filter((way) => way.kind === 'room' && way.network !== 'moqui' && usable(way) && way.lastInboundDate)
      .sort((a, b) => b.lastInboundDate - a.lastInboundDate)[0]
    best = wrote || ways.find((way) => way.kind === 'room' && way.network === 'sms' && usable(way))
      || ways.find((way) => way.kind === 'text' && way.network === 'sms' && usable(way))
      || ways.find((way) => way.kind === 'mail') || null
  }
  if (best) { best.best = true; ways.splice(ways.indexOf(best), 1); ways.unshift(best) }
  return { ways, best: best ? ways[0] : null, blocked: false }
}

/**
 * What waysOf needs, read once for all of me's people (IN lists, not a query a room): the people I blocked, my Blocked
 * list (its numbers and prefixes, matched as CallHandling.js entryHolds does), the opt-outs of the pairs, the numbers I
 * text from with their bridges' state.
 */
function wayContext(me, people) {
  const blockedUsers = new Set(Array.from(find(BLOCK).condition('userId', me).selectField('blockedUserId').list()).map((row) => row.blockedUserId))
  const tels = new Set()
  for (const person of people) for (const key of shownKeys(person)) if (kindOf(key) === 'tel') tels.add(valueOf(key))
  let entries = []
  let textNumbers = []
  const optOuts = new Set()
  if (tels.size) {
    try {
      const lists = Array.from(find(CALLER_LIST).condition('userId', me).condition('listTypeEnumId', 'CltBlocked').selectField('callerListId').list()).map((row) => row.callerListId)
      if (lists.length) entries = Array.from(find(CALLER_ENTRY).condition('callerListId', 'in', lists).selectField('phoneNumber').list()).map((row) => String(row.phoneNumber || ''))
    } catch (err) {
      ec.logger.error(`The Blocked list of ${me} was not read: ${message(err)}`)
    }
    try {
      const mine = Array.from(ec.resource.script(SMS).execute('myTextNumbers', {}).numberList || []).map((row) => row.phoneNumber)
      const numberOf = {}
      if (mine.length) for (const row of find(NUMBER).condition('phoneNumber', 'in', mine).selectField('phoneNumber,messagingBridgeId').list()) numberOf[row.phoneNumber] = row
      const bridgeIds = Array.from(new Set(Object.values(numberOf).map((row) => row.messagingBridgeId).filter((id) => id)))
      const bridgeOf = {}
      if (bridgeIds.length) for (const row of find(BRIDGE).condition('chatBridgeId', 'in', bridgeIds).selectField('chatBridgeId,typeEnumId,statusId').list()) bridgeOf[row.chatBridgeId] = row
      textNumbers = mine.map((phoneNumber) => {
        const bridge = bridgeOf[numberOf[phoneNumber]?.messagingBridgeId]
        return { phoneNumber, network: bridge && bridge.typeEnumId === 'CbtRcs' ? 'rcs' : 'sms', notConnected: !bridge || (!!bridge.statusId && bridge.statusId !== 'CbsConnected') }
      })
    } catch (err) {
      ec.logger.error(`The text numbers of ${me} were not read: ${message(err)}`)
    }
    const ours = Array.from(new Set(people.flatMap((person) => person.rooms.map((room) => room.fromNumber)).concat(textNumbers.map((row) => row.phoneNumber)).filter((n) => n)))
    if (ours.length) {
      for (const row of find(OPT_OUT).condition('phoneNumber', 'in', ours).condition('otherNumber', 'in', Array.from(tels)).condition('optedOut', 'Y')
        .selectField('phoneNumber,otherNumber').list()) optOuts.add(row.phoneNumber + '/' + row.otherNumber)
    }
  }
  const blockedNumber = (number) => entries.some((entry) => (entry.endsWith('*') ? number.startsWith(entry.slice(0, -1)) : entry === number))
  const optedOut = (ours, theirs) => optOuts.has(ours + '/' + theirs)
  return { blockedUsers, blockedNumber, optedOut, textNumbers }
}

/** The bridge people (ext: keys) of these people: chatExternalUserId to {displayName, network}. */
function externalsOf(people) {
  const ids = Array.from(new Set(people.flatMap((person) => person.keys.filter((key) => kindOf(key) === 'ext').map(valueOf))))
  const out = {}
  if (!ids.length) return out
  const rows = Array.from(find(EXT_USER).condition('chatExternalUserId', 'in', ids).selectField('chatExternalUserId,chatBridgeId,displayName').list())
  const bridges = {}
  const bridgeIds = Array.from(new Set(rows.map((row) => row.chatBridgeId)))
  if (bridgeIds.length) for (const bridge of find(BRIDGE).condition('chatBridgeId', 'in', bridgeIds).selectField('chatBridgeId,typeEnumId').list()) bridges[bridge.chatBridgeId] = bridge
  for (const row of rows) out[row.chatExternalUserId] = { displayName: row.displayName || null, network: NETWORKS[bridges[row.chatBridgeId]?.typeEnumId] || null }
  return out
}

/** find#ChatPeople: the current user's people, each with its shown addresses, rooms, ways and best way. */
function findPeople() {
  const me = ec.user.userId
  if (!me) return { personList: [], roomPersonMap: {} }
  const grouped = groupPeople(me)
  const context = wayContext(me, grouped.people)
  const externalOf = externalsOf(grouped.people)
  const personList = grouped.people.map((person) => {
    const named = nameOf(person)
    const routes = waysOf(person, context)
    return {
      personKey: person.personKey, name: named.name, nameFrom: named.nameFrom, edited: !!person.chatPersonId, userId: person.userId,
      reachable: person.reachable, contactUrl: person.contact ? person.contact.url : null,
      addresses: shownKeys(person).map((key) => addressOf(person, key, externalOf)),
      rooms: person.rooms.map((room) => ({ chatRoomId: room.chatRoomId, network: room.network, encrypted: room.encrypted, fromNumber: room.fromNumber,
        lastMessageDate: room.lastMessageDate, lastInboundDate: room.lastInboundDate, notConnected: room.notConnected })),
      ways: routes.ways, best: routes.best, blocked: routes.blocked,
    }
  })
  return { personList, roomPersonMap: grouped.roomRows, hookFailed: grouped.hookFailed, contactsSkipped: grouped.contactsSkipped }
}

/**
 * Internal, for find#DirectChatMail: each person of the current user with rooms, as {personKey, userId, chatRoomIdList,
 * mailAddressList}: a user's account address and the addresses the viewer linked; an outside person's every address
 * (the hook's and the viewer's rows). The accounts are those the grouping read; the hook's answer is the one kept.
 */
function mailPeople() {
  const me = ec.user.userId
  if (!me) return { personList: [] }
  const grouped = groupPeople(me)
  const personList = []
  for (const person of grouped.people) {
    if (!person.rooms.length) continue
    const keys = person.userId ? person.keys.filter((key) => person.rowAddresses.has(key)) : person.keys
    const addresses = keys.filter((key) => kindOf(key) === 'email').map(valueOf)
    if (person.userId) {
      const own = String(grouped.accountOf[person.userId]?.emailAddress || '').trim().toLowerCase()
      if (own.includes('@') && !addresses.includes(own)) addresses.unshift(own)
    }
    if (addresses.length) personList.push({ personKey: person.personKey, userId: person.userId, chatRoomIdList: person.rooms.map((room) => room.chatRoomId), mailAddressList: addresses })
  }
  return { personList }
}

// ---------- merge, separate, rename, reset ----------

/** The current user's row lock: one change to their people at a time. */
function lockMe(me) {
  find(ACCOUNT).condition('userId', me).forUpdate(true).one()
}

/** The person of key: a changed one by its row, an automatic one by its address (which stays when a message comes). */
function personByKey(grouped, key) {
  const text = String(key || '')
  if (text.startsWith('a:')) {
    const address = text.slice(2)
    return grouped.people.find((person) => person.personKey === text) || grouped.people.find((person) => shownKeys(person).includes(address)) || null
  }
  return grouped.people.find((person) => person.personKey === text) || null
}

/** A change needs the hook's grouping as the user sees it: refused while the hook failed or was skipped. */
function contactsRefusal(grouped) {
  return grouped.hookFailed || grouped.contactsSkipped ? refuse('contactsUnavailable', 'Contacts are not available right now; try again shortly') : null
}

/** Write keys into the person chatPersonId (made when null): rows moved or made; persons left empty deleted. */
function writePerson(me, chatPersonId, keys, displayName) {
  let id = chatPersonId
  if (!id) {
    const person = ec.entity.makeValue(PERSON)
    person.setSequencedIdPrimary()
    person.setFields({ ownerUserId: me, displayName: blank(displayName) ? null : String(displayName).trim().slice(0, 100), createdDate: ec.user.nowTimestamp }, true, null, null)
    person.create()
    id = person.chatPersonId
  }
  const left = new Set()
  for (const key of keys) {
    const row = one(ADDRESS, { ownerUserId: me, addressKey: key })
    if (row) {
      if (row.chatPersonId !== id) { left.add(row.chatPersonId); row.set('chatPersonId', id); row.update() }
      continue
    }
    const made = ec.entity.makeValue(ADDRESS)
    made.setFields({ ownerUserId: me, addressKey: key, chatPersonId: id, createdDate: ec.user.nowTimestamp }, true, null, null)
    made.create()
  }
  for (const other of left) {
    if (find(ADDRESS).condition({ ownerUserId: me, chatPersonId: other }).count() === 0) {
      const person = one(PERSON, { chatPersonId: other })
      if (person && person.ownerUserId === me) person.delete()
    }
  }
  return id
}

/** Whether me may name this key in a person: a reachable user, a number, an address, or a bridge person of my own room. */
function keyRefusal(me, key, grouped) {
  const kind = kindOf(key)
  if (kind === 'user') {
    const userId = valueOf(key)
    if (userId === me) return refuse('notAPerson', 'You cannot add yourself to a person')
    // a user of one of my rooms, unless they are one I may not reach (refused, or they blocked me)
    const ofRoom = grouped.people.find((person) => person.roomAddresses.has(key))
    if (ofRoom) return ofRoom.reachable ? null : refuse('userNotFound', 'No such user')
    const account = one(ACCOUNT, { userId })
    const accountOf = account ? { [userId]: account } : {}
    if (!reachableOf(me, [userId], accountOf).has(userId)) return refuse('userNotFound', 'No such user')
    return null
  }
  if (kind === 'ext' && !grouped.people.some((person) => person.roomAddresses.has(key))) return refuse('notYourRoom', 'That person is not in one of your conversations')
  return null
}

/**
 * merge#ChatPeople: one person of the current user's with every address the persons named (personKeyList) show and the
 * addresses given (addressKeyList). A typed address is matched only against what the user is shown, and joins a user of
 * this server only when it is already shown to the user (one of their rooms or rows). Refused: badAddress, userNotFound,
 * notYourRoom, personNotFound, nothingToMerge, twoUsers, notShown, tooManyAddresses, tooManyPeople, contactsUnavailable.
 * Out: personKey.
 */
function mergePeople(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in to change your people')
  lockMe(me)
  const grouped = groupPeople(me)
  const unavailable = contactsRefusal(grouped)
  if (unavailable) return unavailable
  const shownAll = new Set(grouped.people.flatMap((person) => shownKeys(person)))
  const keys = []
  const typed = []
  const add = (key) => { if (!keys.includes(key)) keys.push(key) }
  let chatPersonId = null
  let displayName = null
  for (const personKey of Array.from(p.personKeyList || []).map(String).filter((key) => key)) {
    const person = personByKey(grouped, personKey)
    if (!person) return refuse('personNotFound', 'That person is no longer in your list')
    if (person.chatPersonId && !chatPersonId) chatPersonId = person.chatPersonId
    const name = nameOf(person)
    if (!displayName && name.nameFrom === 'contact') displayName = name.name
    shownKeys(person).forEach(add)
  }
  for (const value of Array.from(p.addressKeyList || []).map(String).filter((key) => key)) {
    const key = normalKey(value)
    if (!key) return refuse('badAddress', 'That is not a number, a mail address or a person you can add')
    // an address the user is shown brings its person; one only the contacts know brings nothing
    const person = grouped.people.find((found) => shownKeys(found).includes(key))
    if (person) {
      if (person.chatPersonId && !chatPersonId) chatPersonId = person.chatPersonId
      shownKeys(person).forEach(add)
    }
    if (!shownAll.has(key)) typed.push(key)
    add(key)
  }
  for (const key of keys) { const refusal = keyRefusal(me, key, grouped); if (refusal) return refusal }
  if (keys.length < 2) return refuse('nothingToMerge', 'Choose two or more to put together')
  if (keys.filter((key) => kindOf(key) === 'user').length > 1) return refuse('twoUsers', 'Two users of this server are two people')
  if (keys.some((key) => kindOf(key) === 'user') && typed.some((key) => kindOf(key) !== 'user')) {
    return refuse('notShown', 'A user of this server takes only a number or address of one of your conversations')
  }
  if (keys.length > MAX_ADDRESSES) return refuse('tooManyAddresses', `A person has at most ${MAX_ADDRESSES} addresses`)
  const already = find(ADDRESS).condition('ownerUserId', me).count()
  const fresh = keys.filter((key) => !one(ADDRESS, { ownerUserId: me, addressKey: key })).length
  if (already + fresh > MAX_ROWS) return refuse('tooManyPeople', 'Your list of people is full')
  if (keys.some((key) => kindOf(key) === 'user')) displayName = null
  const id = writePerson(me, chatPersonId, keys, displayName)
  return { personKey: 'p:' + id }
}

/** separate#ChatPersonAddress: the address alone in a person of its own, which the contacts do not join back. */
function separateAddress(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in to change your people')
  const key = normalKey(p.addressKey)
  if (!key) return refuse('badAddress', 'That is not a number, a mail address or a person you can add')
  lockMe(me)
  const grouped = groupPeople(me)
  const person = grouped.people.find((found) => shownKeys(found).includes(key))
  if (!person) return refuse('personNotFound', 'That person is no longer in your list')
  if (find(ADDRESS).condition('ownerUserId', me).count() >= MAX_ROWS && !one(ADDRESS, { ownerUserId: me, addressKey: key })) {
    return refuse('tooManyPeople', 'Your list of people is full')
  }
  const id = writePerson(me, null, [key], null)
  // an automatic person left with others keeps them as they were: they stay automatic
  return { personKey: 'p:' + id }
}

/** rename#ChatPerson: the user's own name for the person (empty clears it); a person with no rows gets them. */
function renamePerson(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in to change your people')
  const name = blank(p.displayName) ? null : String(p.displayName).trim().slice(0, 100)
  lockMe(me)
  const grouped = groupPeople(me)
  const person = personByKey(grouped, String(p.personKey || ''))
  if (!person) return refuse('personNotFound', 'That person is no longer in your list')
  if (person.chatPersonId) {
    const row = one(PERSON, { chatPersonId: person.chatPersonId })
    row.set('displayName', name)
    row.update()
    return { personKey: person.personKey }
  }
  // an automatic person's rows are its grouping as the user sees it now: not while the contacts are away
  const unavailable = contactsRefusal(grouped)
  if (unavailable) return unavailable
  const keys = shownKeys(person).slice(0, MAX_ADDRESSES)
  if (find(ADDRESS).condition('ownerUserId', me).count() + keys.length > MAX_ROWS) return refuse('tooManyPeople', 'Your list of people is full')
  return { personKey: 'p:' + writePerson(me, null, keys, name) }
}

/** reset#ChatPerson: the user's rows of the person deleted, so it is grouped automatically again. */
function resetPerson(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in to change your people')
  const key = String(p.personKey || '')
  if (!key.startsWith('p:')) return {}
  lockMe(me)
  const person = one(PERSON, { chatPersonId: key.slice(2) })
  if (!person || person.ownerUserId !== me) return refuse('personNotFound', 'That person is no longer in your list')
  for (const row of find(ADDRESS).condition({ ownerUserId: me, chatPersonId: person.chatPersonId }).list()) row.delete()
  person.delete()
  return {}
}

const operations = { findPeople, mailPeople, mergePeople, separateAddress, renamePerson, resetPerson }

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown chat people operation ' + op)
  return operations[op](parameters || {}) || {}
}

;({ execute })
