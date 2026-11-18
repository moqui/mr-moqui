// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// The GroupMe bridge's services (org.moqui.chat.GroupMeServices; plans/Comms-Messaging.md §18.3 phase 14,
// docs/interfaces/GroupMe.md). The adapter is crates/moqui-channels/src/groupme: it holds the push connection to
// GroupMe and calls these internal operations with authorization off. A GroupMe bridge (ChatBridge typeEnumId
// CbtGroupMe, adapter groupme) is one person's GroupMe account: its configJson (encrypted) holds the access token and
// ownerUserId, the local user whose account it is.
//
// - ensureRoom: the room of one group, Topic or direct conversation (a private room the owner owns), with its mapping.
// - receiveLikes: the likes of one message as a state, turned into heart reactions and their removal through
//   ingest#Event.
// - getMine, connectMine, disconnectMine: the current user's own GroupMe bridge (Settings, Chat, GroupMe), unless the
//   property chat_groupme_self_service is false. A user has at most one GroupMe bridge; the token is write-only.

const BRIDGE = 'moqui.chat.ChatBridge'
const ROOM = 'moqui.chat.ChatRoom'
const MEMBER = 'moqui.chat.ChatRoomMember'
const MAPPING = 'moqui.chat.ChatRoomBridge'
const EXTERNAL = 'moqui.chat.ChatEventExternal'
const EVENT = 'moqui.chat.ChatEvent'
const ACTOR = 'moqui.chat.ChatActor'
const EXTERNAL_USER = 'moqui.chat.ChatExternalUser'
const HEART = '❤️'
const HEARTS = ['❤️', '❤', '♥️', '♥']
const DEFAULT_MAX_ROOMS = 200

const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const rows = (entity, fields) => Array.from(find(entity).condition(fields || {}).list())
const blank = (value) => value == null || String(value).trim() === ''
const L = (text) => ec.l10n.localize(text)

function configOf(bridge) {
  try {
    const config = JSON.parse(String((bridge && bridge.configJson) || '{}'))
    return config && typeof config === 'object' ? config : {}
  } catch (err) {
    return {}
  }
}

function refuse(reasonCode, text) {
  ec.message.addError(L(text))
  return { reasonCode }
}

/** Whether users may connect their own GroupMe (the property chat_groupme_self_service, on unless false). */
function selfService() {
  return String(System.getProperty('chat_groupme_self_service') || '').trim().toLowerCase() !== 'false'
}

/** The GroupMe bridges whose owner is userId (their configJson read), oldest first. */
function bridgesOf(userId) {
  return Array.from(find(BRIDGE).condition('typeEnumId', 'CbtGroupMe').orderBy('createdDate').list())
    .filter((bridge) => configOf(bridge).ownerUserId === userId)
}

/**
 * Internal (the adapter): the room of a group, Topic or direct conversation of a GroupMe bridge, found or made under the
 * bridge's row lock, with its mapping (externalRoomId g:<id> or dm:<user id>). The bridge's owner (its configJson's
 * ownerUserId) owns a new room and is its one local member; one who left stays out. A new name from GroupMe renames the
 * room while it still has GroupMe's last name. conversationId (a direct conversation's chat id) is the mapping's
 * externalThreadId. Refused past the bridge's maxRooms (tooManyRooms).
 */
function ensureRoom(p) {
  const chatBridgeId = p.chatBridgeId
  const externalRoomId = String(p.externalRoomId || '')
  if (!/^(g|dm):[A-Za-z0-9_+-]{1,64}$/.test(externalRoomId)) return refuse('badRoom', 'A GroupMe room is a group or a person')
  const bridge = find(BRIDGE).condition('chatBridgeId', chatBridgeId).forUpdate(true).one()
  if (!bridge || bridge.typeEnumId !== 'CbtGroupMe') return refuse('unknownBridge', 'Not a GroupMe bridge')
  const config = configOf(bridge)
  const owner = config.ownerUserId ? one('moqui.security.UserAccount', { userId: config.ownerUserId }) : null
  if (!owner) return refuse('noOwner', 'The GroupMe bridge has no owner')
  const kind = ['group', 'topic', 'direct'].includes(p.kind) ? p.kind : 'group'
  const name = String(p.name || '').trim().slice(0, 200) || (kind === 'direct' ? L('GroupMe person') : L('GroupMe group'))
  const parentName = String(p.parentName || '').trim().slice(0, 200)
  const roomName = (kind === 'topic' && parentName ? parentName + ' › ' + name : name).slice(0, 255)
  const now = ec.user.nowTimestamp
  let mapping = Array.from(find(MAPPING).condition('chatBridgeId', chatBridgeId).condition('externalRoomId', externalRoomId)
    .condition('thruDate', null).list())[0]
  let room = mapping ? one(ROOM, { chatRoomId: mapping.chatRoomId }) : null
  let created = false
  if (room) {
    // GroupMe's new name, unless the owner named the room
    if (roomName !== mapping.externalRoomName && room.roomName === mapping.externalRoomName) {
      room.set('roomName', roomName)
      room.update()
    }
    const fields = {}
    if (roomName !== mapping.externalRoomName) fields.externalRoomName = roomName
    if (!blank(p.conversationId) && p.conversationId !== mapping.externalThreadId) fields.externalThreadId = String(p.conversationId).slice(0, 128)
    if (Object.keys(fields).length) {
      mapping.setFields(fields, true, null, null)
      mapping.update()
    }
  } else {
    const maxRooms = Number(config.maxRooms) > 0 ? Math.floor(Number(config.maxRooms)) : DEFAULT_MAX_ROOMS
    const count = find(MAPPING).condition('chatBridgeId', chatBridgeId).condition('thruDate', null).count()
    if (count >= maxRooms) return refuse('tooManyRooms', 'The GroupMe bridge has as many rooms as it may')
    room = ec.entity.makeValue(ROOM)
    room.setSequencedIdPrimary()
    const topic = kind === 'direct' ? L('GroupMe direct messages')
      : kind === 'topic' ? ec.resource.expand('GroupMe Topic in ${group}', '', { group: parentName || name }) : L('GroupMe group')
    room.setFields({ roomTypeEnumId: 'CrtPrivate', roomName, topic, visibilityEnumId: 'CrvMembers', createdByUserId: owner.userId,
      createdDate: now, lastEventSeq: 0, lastEventDate: now }, true, null, null)
    room.create()
    // a mapping whose room is gone ends, and the group gets a new room
    if (mapping) {
      mapping.setFields({ thruDate: now }, true, null, null)
      mapping.update()
    }
    mapping = ec.entity.makeValue(MAPPING)
    mapping.setSequencedIdPrimary()
    mapping.setFields({ chatRoomId: room.chatRoomId, chatBridgeId, externalRoomId, externalRoomName: roomName,
      externalThreadId: blank(p.conversationId) ? null : String(p.conversationId).slice(0, 128), directionEnumId: 'CrbBoth', fromDate: now },
    true, null, null)
    mapping.create()
    created = true
  }
  const member = one(MEMBER, { chatRoomId: room.chatRoomId, userId: owner.userId })
  if (!member && created) {
    const actorOf = ec.service.sync().name('org.moqui.chat.ChatServices.get#LocalActors').parameter('userIdList', [owner.userId])
      .disableAuthz().call()?.actorMap || {}
    const row = ec.entity.makeValue(MEMBER)
    row.setFields({ chatRoomId: room.chatRoomId, chatActorId: actorOf[owner.userId], userId: owner.userId, roleEnumId: 'CmrOwner', joinedDate: now,
      lastReadEventSeq: 0, notificationLevelEnumId: 'CnlAll', isPinned: 'N', isMuted: 'N' }, true, null, null)
    row.create()
    ec.service.sync().name('org.moqui.chat.ChatServices.record#UserChange').parameter('changeList', [{ userIdList: [owner.userId],
      changeKind: 'membership', chatRoomId: room.chatRoomId, change: { member: true, roleEnumId: 'CmrOwner', action: 'joined' },
      pushMessage: { kind: 'room', action: 'joined', chatRoomId: room.chatRoomId } }]).disableAuthz().call()
  }
  return { chatRoomId: room.chatRoomId, chatRoomBridgeId: mapping.chatRoomBridgeId, created }
}

/** A short hash of a text (FNV-1a, 32 bits, hex): a transaction id's part. */
function hashOf(text) {
  let hash = 0x811c9dc5
  for (let i = 0; i < text.length; i++) {
    hash ^= text.charCodeAt(i)
    hash = Math.imul(hash, 0x01000193) >>> 0
  }
  return hash.toString(16)
}

/**
 * Internal (the adapter, inside its transport mark): the likes of GroupMe message messageId in externalRoomId as a
 * state, likedByList its likers' GroupMe ids. The heart reactions of the room's GroupMe people on the message are made
 * to match: a reaction for each new liker, a redaction of the latest like of each liker who took it back, through
 * ingest#Event, one transaction per state (so a repeat, or another node's, is a duplicate). ownGroupMeId (the token's
 * user) counts as liked while the owner has a local heart on it, so the owner's own like does not come back as a second
 * heart. nameMap: GroupMe id to name. Out: outcome (unchanged, unknownMessage, sent), added, removed.
 */
function receiveLikes(p) {
  const chatBridgeId = p.chatBridgeId
  const externalRoomId = String(p.externalRoomId || '')
  const messageId = String(p.messageId || '')
  if (blank(externalRoomId) || blank(messageId)) return { outcome: 'unknownMessage', added: 0, removed: 0 }
  const target = one(EXTERNAL, { chatBridgeId, externalRoomId, externalEventId: messageId })
  if (!target || target.eventSeq == null) return { outcome: 'unknownMessage', added: 0, removed: 0 }
  const bridge = one(BRIDGE, { chatBridgeId })
  const owner = configOf(bridge).ownerUserId
  const wanted = new Set(Array.from(p.likedByList || []).map((id) => String(id)).filter((id) => /^[A-Za-z0-9_-]{1,64}$/.test(id)))
  const names = p.nameMap && typeof p.nameMap === 'object' ? p.nameMap : {}
  const own = blank(p.ownGroupMeId) ? null : String(p.ownGroupMeId)
  // the hearts on the message now: each GroupMe person's latest, and whether the owner has one of their own
  const hearts = Array.from(find(EVENT).condition('chatRoomId', target.chatRoomId).condition('eventTypeEnumId', 'CetReaction')
    .condition('relatesToEventSeq', Number(target.eventSeq)).condition('redactedDate', null).orderBy('eventSeq').list())
    .filter((row) => { try { return HEARTS.includes(JSON.parse(row.contentJson || '{}').key) } catch (err) { return false } })
  let ownerHeart = false
  const current = new Map()
  for (const row of hearts) {
    if (row.senderUserId) { if (row.senderUserId === owner) ownerHeart = true; continue }
    const actor = row.senderActorId ? one(ACTOR, { chatActorId: row.senderActorId }) : null
    const person = actor && actor.chatExternalUserId ? one(EXTERNAL_USER, { chatExternalUserId: actor.chatExternalUserId }) : null
    if (!person || person.chatBridgeId !== chatBridgeId) continue
    current.set(person.externalUserId, row)
  }
  const items = []
  const at = Number(ec.user.nowTimestamp)
  const stamp = String(Number.isNaN(at) ? new Date(String(ec.user.nowTimestamp)).getTime() : at)
  for (const id of wanted) {
    if (current.has(id) || (id === own && ownerHeart)) continue
    items.push({ itemId: 'like-' + id, type: 'reaction', externalRoomId, externalEventId: `like:${messageId}:${id}:${stamp}`, externalUserId: id,
      displayName: names[id] ? String(names[id]).slice(0, 200) : null, relatesToExternalEventId: messageId, key: HEART })
  }
  for (const [id, row] of current) {
    if (wanted.has(id)) continue
    const mapped = Array.from(find(EXTERNAL).condition('chatBridgeId', chatBridgeId).condition('chatRoomId', row.chatRoomId)
      .condition('eventSeq', Number(row.eventSeq)).list())[0]
    if (!mapped) continue
    items.push({ itemId: 'unlike-' + id, type: 'redaction', externalRoomId, externalEventId: 'unlike:' + mapped.externalEventId, externalUserId: id,
      relatesToExternalEventId: mapped.externalEventId })
  }
  if (!items.length) return { outcome: 'unchanged', added: 0, removed: 0 }
  const transactionId = ('gml:' + messageId + ':' + hashOf(Array.from(wanted).sort().join(','))).slice(0, 128)
  ec.service.sync().name('org.moqui.chat.ChatBridgeServices.ingest#Event').parameters({ chatBridgeId, transactionId, transportVerified: true,
    itemList: items }).disableAuthz().call()
  return { outcome: 'sent', added: items.filter((item) => item.type === 'reaction').length, removed: items.filter((item) => item.type === 'redaction').length }
}

/** What the current user's own GroupMe bridge shows (Settings, Chat): the bridge's status, never its token. */
function getMine() {
  const me = ec.user.userId
  const enabled = selfService()
  if (!me) return { enabled, connected: false, hasBridge: false }
  const bridge = bridgesOf(me)[0]
  if (!bridge) return { enabled, connected: false, hasBridge: false }
  const config = configOf(bridge)
  const roomCount = find(MAPPING).condition('chatBridgeId', bridge.chatBridgeId).condition('thruDate', null).count()
  const statusText = bridge.statusId === 'CbsConnected' ? L('Connected')
    : bridge.statusId === 'CbsDisabled' ? L('Disconnected') : L('Not connected')
  return { enabled, hasBridge: true, chatBridgeId: bridge.chatBridgeId, statusId: bridge.statusId, statusText,
    connected: bridge.statusId === 'CbsConnected', hasToken: !blank(config.accessToken), roomCount,
    lastErrorText: bridge.lastErrorText || null, lastStatusDate: bridge.lastStatusDate || null }
}

/**
 * Connect the current user's own GroupMe (Settings, Chat, GroupMe): their access token makes their GroupMe bridge, or
 * replaces its token and connects it again. An empty token keeps the one kept (to connect again after a disconnect).
 * Refused when the property chat_groupme_self_service is false (selfServiceOff).
 */
function connectMine(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  if (!selfService()) return refuse('selfServiceOff', 'Connecting GroupMe is turned off on this server: ask an administrator')
  const token = String(p.accessToken || '').trim()
  if (token && !/^[A-Za-z0-9]{10,200}$/.test(token)) return refuse('badToken', 'A GroupMe access token is letters and digits, as dev.groupme.com shows it')
  const now = ec.user.nowTimestamp
  // one GroupMe bridge per user: two first connects at once make one (the user's row lock)
  find('moqui.security.UserAccount').condition('userId', me).selectField('userId').forUpdate(true).one()
  let bridge = bridgesOf(me)[0]
  if (!bridge) {
    if (!token) return refuse('tokenRequired', 'Paste your GroupMe access token')
    const account = one('moqui.security.UserAccount', { userId: me })
    bridge = ec.entity.makeValue(BRIDGE)
    bridge.setSequencedIdPrimary()
    bridge.setFields({ typeEnumId: 'CbtGroupMe', adapterName: 'groupme', name: ec.resource.expand('GroupMe of ${name}', '',
      { name: (account && (account.userFullName || account.username)) || me }), configJson: JSON.stringify({ accessToken: token, ownerUserId: me }),
    statusId: 'CbsNotConnected', lastStatusDate: now, createdDate: now }, true, null, null)
    bridge.create()
  } else {
    const config = configOf(bridge)
    if (!token && blank(config.accessToken)) return refuse('tokenRequired', 'Paste your GroupMe access token')
    if (token) config.accessToken = token
    config.ownerUserId = me
    bridge.setFields({ configJson: JSON.stringify(config), statusId: 'CbsNotConnected', lastStatusDate: now, lastErrorText: null }, true, null, null)
    bridge.update()
  }
  ec.service.sync().name('org.moqui.chat.ChatServices.wake#BridgeDelivery').disableAuthz().call()
  return { chatBridgeId: bridge.chatBridgeId }
}

/**
 * Disconnect the current user's own GroupMe: the bridge is disabled (its push connection stops), its token is
 * forgotten, and its rooms keep their history and stop syncing. Connecting again needs a token.
 */
function disconnectMine() {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const bridge = bridgesOf(me)[0]
  if (!bridge) return { chatBridgeId: null }
  const config = configOf(bridge)
  delete config.accessToken
  bridge.setFields({ configJson: JSON.stringify(config), statusId: 'CbsDisabled', lastStatusDate: ec.user.nowTimestamp,
    lastErrorText: L('Disconnected by its owner: the rooms keep their history and no longer sync with GroupMe') }, true, null, null)
  bridge.update()
  ec.service.sync().name('org.moqui.chat.ChatServices.wake#BridgeDelivery').disableAuthz().call()
  return { chatBridgeId: bridge.chatBridgeId }
}

/**
 * Internal (store#Bridge, an administrator's change of a GroupMe bridge): refusal text for a configuration that is not
 * a GroupMe bridge's, else null: the adapter groupme, an access token and an owner who is a user, the owner unchanged
 * while the bridge has rooms (ownerFixed), and no other GroupMe bridge of that owner (oneBridge).
 */
function checkBridge(p) {
  if (p.adapterName && p.adapterName !== 'groupme') return { reasonCode: 'badAdapter', message: L('A GroupMe bridge\'s adapter is groupme') }
  let config = {}
  try { config = JSON.parse(String(p.configJson || '{}')) || {} } catch (err) { config = {} }
  if (blank(config.accessToken)) return { reasonCode: 'tokenRequired', message: L('A GroupMe bridge needs its accessToken') }
  if (blank(config.ownerUserId) || !one('moqui.security.UserAccount', { userId: config.ownerUserId }))
    return { reasonCode: 'noOwner', message: L('A GroupMe bridge needs ownerUserId, the user whose account it is') }
  if (!blank(p.chatBridgeId)) {
    const before = configOf(one(BRIDGE, { chatBridgeId: p.chatBridgeId }))
    const rooms = find(MAPPING).condition('chatBridgeId', p.chatBridgeId).count()
    if (rooms && before.ownerUserId && before.ownerUserId !== config.ownerUserId)
      return { reasonCode: 'ownerFixed', message: L('A GroupMe bridge with rooms keeps its owner') }
  }
  if (bridgesOf(config.ownerUserId).some((bridge) => bridge.chatBridgeId !== p.chatBridgeId))
    return { reasonCode: 'oneBridge', message: L('That user has a GroupMe bridge already') }
  return { reasonCode: null, message: null }
}

const operations = { ensureRoom, receiveLikes, getMine, connectMine, disconnectMine, checkBridge }

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown GroupMe operation ' + op)
  // each operation checks who may do it itself (the current user's own bridge, or an internal caller); its rows are
  // written with authorization off, and it is back on after, however the operation ends
  const authzWasDisabled = ec.artifactExecution.disableAuthz()
  try {
    return operations[op](parameters) || {}
  } finally {
    if (!authzWasDisabled) ec.artifactExecution.enableAuthz()
  }
}

;({ execute })
