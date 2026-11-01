// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Chat devices, keys and to-device messages (plans/Comms-Framework.md §4.5, C7 core; docs/interfaces/Chat-Keys.md): the
// services of org.moqui.impl.ChatKeyServices. The browser runs matrix-sdk-crypto's OlmMachine (js/WebrootCrypto.js), which
// does no I/O: these are the endpoints it calls, with Matrix's payloads, so the Matrix client API (C6a) can serve the same
// services at /_matrix/client/v3/keys/upload, keys/query, keys/claim, sendToDevice and the to-device part of /sync.
//
// - **Identity.** A device belongs to a user (moqui.chat.ChatDevice, keyed by userId and deviceId). In the payloads a user
//   is a Matrix user ID, @<localpart>:<chat_server_name>, the localpart the userId with Matrix's escaping of other
//   character sets (an upper-case letter is _ and the lower-case letter, _ is __, any other byte =xx), so it parses as a
//   conforming ID and maps back.
// - **Rules.** Only the signed-in user's own devices are written: the device a request names is theirs and not revoked, or
//   it is refused as "not found" (another user's) or "revoked". keys/query, keys/claim and sendToDevice reach only the
//   devices of users who share a room (a member now) or a call that has not ended (not denied or removed) with the caller,
//   and the caller's own; the others are left out as if they had none. A revoked device is left out of queries, its keys
//   and queued messages are deleted, and nothing new is queued to it. A one-time key is claimed under the lock of its
//   device's row, so two claims at once never get the same key.
// - **To-device.** A message is queued per device at the device's next stream position and kept until the device
//   acknowledges it: a sync from a position deletes what is at or before it and answers what comes after. A queued message
//   wakes the recipient's tabs with a push on chatUser:<userId> ({kind: 'toDevice'}) that carries nothing of the message.
// - **The server** stores and answers public keys, ciphertext and routing only. It does not read message content.

const DEVICE = 'moqui.chat.ChatDevice'
const KEY = 'moqui.chat.ChatDeviceKey'
const TO_DEVICE = 'moqui.chat.ChatToDevice'
const MEMBER = 'moqui.chat.ChatRoomMember'
const PARTICIPANT = 'moqui.call.CallParticipant'
const SESSION = 'moqui.call.CallSession'
/** A participant denied or removed is not in the call. */
const OUT_OF_CALL = ['CasDenied', 'CasRevoked']

/** The devices a user may have at once (revoked ones do not count). */
const MAX_DEVICES = 50
/** One-time keys in one upload, and unclaimed ones a device may hold. */
const MAX_UPLOAD_KEYS = 200
const MAX_DEVICE_KEYS = 500
/** Users in one query, and devices in one claim or one send. */
const MAX_USERS = 1000
const MAX_TARGETS = 1000
/** The size of a device's signed keys, and of one message's content (JSON characters). */
const MAX_DEVICE_KEYS_JSON = 16384
const MAX_CONTENT = 65536
/** Messages a device may have waiting; one past it is not queued (logged). */
const MAX_QUEUED = 5000
/** A sync answers at most this many messages; has_more says there are more. */
const MAX_SYNC = 500
/** A device-list cursor looks back this far, so a change committed a little late is still seen (repeating one is harmless). */
const LIST_OVERLAP_MS = 120000

const ms = (value) => {
  if (value == null || value === '') return null
  const number = Number(value)
  return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}
const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const rows = (entity, fields) => Array.from(find(entity).condition(fields || {}).list())
const blank = (value) => value == null || String(value).trim() === ''
const property = (name) => String(System.getProperty(name) || '').trim()
const isObject = (value) => value != null && typeof value === 'object' && !Array.isArray(value)
const chunks = (list, size) => {
  const out = []
  for (let i = 0; i < list.length; i += size) out.push(list.slice(i, i + size))
  return out
}

function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

// ---- Matrix user IDs ----

/** The server name in Matrix user IDs: chat_server_name, else localhost. Set it before the first device: a change makes every device new. */
function serverName() {
  return (property('chat_server_name') || 'localhost').toLowerCase()
}

const PLAIN = /^[a-z0-9.\-]$/
const hex2 = (code) => (code < 16 ? '0' : '') + code.toString(16)

/** The Matrix localpart of a userId (the spec's mapping from other character sets). */
function localpart(userId) {
  let out = ''
  for (const ch of String(userId)) {
    if (PLAIN.test(ch)) out += ch
    else if (ch === '_') out += '__'
    else if (ch >= 'A' && ch <= 'Z') out += '_' + ch.toLowerCase()
    else {
      const escaped = encodeURIComponent(ch)
      if (escaped.length === 1) out += '=' + hex2(ch.charCodeAt(0))
      else out += escaped.split('%').filter((part) => part).map((part) => '=' + part.toLowerCase()).join('')
    }
  }
  return out
}

/** The userId of a localpart, or null when it is not one this mapping makes. */
function fromLocalpart(local) {
  let out = ''
  let bytes = ''
  const flush = () => {
    if (!bytes) return
    out += decodeURIComponent(bytes)
    bytes = ''
  }
  for (let i = 0; i < local.length; i++) {
    const ch = local[i]
    if (ch === '=') {
      const pair = local.slice(i + 1, i + 3)
      if (!/^[0-9a-f]{2}$/.test(pair)) return null
      bytes += '%' + pair
      i += 2
      continue
    }
    try { flush() } catch (err) { return null }
    if (ch === '_') {
      const next = local[i + 1]
      if (next === '_') out += '_'
      else if (next >= 'a' && next <= 'z') out += next.toUpperCase()
      else return null
      i++
    } else if (PLAIN.test(ch)) out += ch
    else return null
  }
  try { flush() } catch (err) { return null }
  return out
}

function matrixId(userId) {
  return '@' + localpart(userId) + ':' + serverName()
}

/** The userId of a Matrix user ID of this server, or null (another server's, or not a user ID). */
function userOf(id) {
  const text = String(id || '')
  const colon = text.indexOf(':')
  if (text[0] !== '@' || colon < 2 || text.slice(colon + 1) !== serverName()) return null
  return fromLocalpart(text.slice(1, colon))
}

// ---- who may reach whom ----

/**
 * The users of candidates who share a room with me (both members now). ChatRoomMember is keyed by actor since C1, with
 * the local userId kept beside it (a local actor's id is its userId); external actors have no userId and no devices.
 */
function roomPeers(me, candidates) {
  const found = new Set()
  if (!candidates.length) return found
  const myRooms = Array.from(find(MEMBER).condition('userId', me).condition('leftDate', null).selectField('chatRoomId')
    .limit(10000).list()).map((row) => row.chatRoomId)
  for (const rooms of chunks(myRooms, 500)) {
    for (const users of chunks(candidates.filter((u) => !found.has(u)), 500)) {
      if (!users.length) break
      for (const row of find(MEMBER).condition('chatRoomId', 'in', rooms).condition('userId', 'in', users)
        .condition('leftDate', null).selectField('userId').list()) found.add(row.userId)
    }
  }
  return found
}

/** The users of candidates in a call with me that has not ended, neither of us denied or removed. */
function callPeers(me, candidates) {
  const found = new Set()
  if (!candidates.length) return found
  const mine = Array.from(find(PARTICIPANT).condition('userId', me).selectField('callSessionId').selectField('admissionStatusId')
    .limit(10000).list()).filter((row) => !OUT_OF_CALL.includes(row.admissionStatusId)).map((row) => row.callSessionId)
  for (const sessions of chunks(mine, 500)) {
    const live = Array.from(find(SESSION).condition('callSessionId', 'in', sessions).condition('statusId', '!=', 'CssEnded')
      .selectField('callSessionId').list()).map((row) => row.callSessionId)
    if (!live.length) continue
    for (const users of chunks(candidates.filter((u) => !found.has(u)), 500)) {
      if (!users.length) break
      for (const row of find(PARTICIPANT).condition('callSessionId', 'in', live).condition('userId', 'in', users)
        .selectField('userId').selectField('admissionStatusId').list()) {
        if (!OUT_OF_CALL.includes(row.admissionStatusId)) found.add(row.userId)
      }
    }
  }
  return found
}

/** The users of userIds whose devices I may reach: myself and those I share a room or a live call with. */
function reachable(me, userIds) {
  const wanted = Array.from(new Set(userIds.filter((u) => u)))
  const out = new Set(wanted.filter((u) => u === me))
  const others = wanted.filter((u) => u !== me)
  for (const u of roomPeers(me, others)) out.add(u)
  const rest = others.filter((u) => !out.has(u))
  for (const u of callPeers(me, rest)) out.add(u)
  return out
}

// ---- devices ----

/** My device deviceId (its row locked with lock), or the refusal: notSignedIn, notFound (not mine), revoked. */
function ownDevice(deviceId, lock) {
  const me = ec.user.userId
  if (!me) return { refusal: refuse('notSignedIn', 'Sign in first') }
  const device = blank(deviceId) ? null : find(DEVICE).condition({ userId: me, deviceId: String(deviceId) }).forUpdate(!!lock).one()
  if (!device) return { refusal: refuse('notFound', 'Device not found') }
  if (device.revokedDate) return { refusal: refuse('revoked', 'This device was removed') }
  return { me, device }
}

/** A change others should see of a device: its list revision goes up. */
function listChanged(device) {
  device.setFields({ listRevision: Number(device.listRevision || 1) + 1, listChangedDate: ec.user.nowTimestamp }, true, null, null)
}

function newDeviceId(me) {
  const letters = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'
  for (let attempt = 0; attempt < 10; attempt++) {
    const random = String(ec.user.randomString(20))
    let id = ''
    for (let i = 0; i < random.length && id.length < 10; i++) id += letters[random.charCodeAt(i) % 26]
    if (id.length === 10 && !one(DEVICE, { userId: me, deviceId: id })) return id
  }
  return null
}

function deviceView(device) {
  return {
    deviceId: device.deviceId, displayName: device.displayName || null, ed25519Key: device.ed25519Key || null,
    hasKeys: !!device.ed25519Key, userAgent: device.userAgent || null, createdDate: ms(device.createdDate),
    lastSeenDate: ms(device.lastSeenDate), verifiedDate: ms(device.verifiedDate), revokedDate: ms(device.revokedDate),
    revoked: !!device.revokedDate,
  }
}

/** A new device of the signed-in user: its deviceId and Matrix user ID, which the browser's crypto store is made for. */
function createDevice(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  if (find(DEVICE).condition('userId', me).condition('revokedDate', null).count() >= MAX_DEVICES) {
    return refuse('tooManyDevices', 'Remove a device in Settings, My devices, before adding another')
  }
  const deviceId = newDeviceId(me)
  if (!deviceId) return refuse('failed', 'No device could be made; try again')
  const device = ec.entity.makeValue(DEVICE)
  device.setFields({ userId: me, deviceId, displayName: String(p.displayName || '').trim().slice(0, 100) || null,
    userAgent: String(p.userAgent || '').slice(0, 255) || null, listRevision: 1, listChangedDate: ec.user.nowTimestamp,
    lastToDevicePos: 0, ackedToDevicePos: 0, createdDate: ec.user.nowTimestamp, lastSeenDate: ec.user.nowTimestamp }, true, null, null)
  device.create()
  return { deviceId, user_id: matrixId(me), serverName: serverName() }
}

function findDevices(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  let query = find(DEVICE).condition('userId', me)
  if (p.includeRevoked !== true && p.includeRevoked !== 'true' && p.includeRevoked !== 'Y') query = query.condition('revokedDate', null)
  const list = Array.from(query.orderBy('-lastSeenDate').limit(200).list()).map(deviceView)
  return { deviceList: list, user_id: matrixId(me) }
}

function renameDevice(p) {
  const found = ownDevice(p.deviceId, true)
  if (found.refusal) return found.refusal
  const name = String(p.displayName || '').trim().slice(0, 100)
  if (!name) return refuse('nameRequired', 'Give the device a name')
  found.device.set('displayName', name)
  listChanged(found.device)
  found.device.update()
  return { device: deviceView(found.device) }
}

/** Revoke one of my devices: no new keys or messages reach it, its keys and queue go, and it drops out of queries. */
function revokeDevice(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const device = blank(p.deviceId) ? null : find(DEVICE).condition({ userId: me, deviceId: String(p.deviceId) }).forUpdate(true).one()
  if (!device) return refuse('notFound', 'Device not found')
  if (device.revokedDate) return { deviceId: device.deviceId, repeated: true }
  device.set('revokedDate', ec.user.nowTimestamp)
  listChanged(device)
  device.update()
  find(KEY).condition({ userId: me, deviceId: device.deviceId }).deleteAll()
  find(TO_DEVICE).condition({ userId: me, deviceId: device.deviceId }).deleteAll()
  wake(me, { kind: 'devices', action: 'revoked', deviceId: device.deviceId })
  return { deviceId: device.deviceId }
}

// ---- keys/upload, keys/query, keys/claim ----

/**
 * The device's unclaimed one-time keys by algorithm, signed_curve25519 always there (0 when none is left): the OlmMachine
 * updates its count only from an algorithm the answer names, so without it a device whose keys were all claimed would never
 * upload more (Synapse answers it the same way).
 */
function unclaimedCounts(me, deviceId) {
  const counts = { signed_curve25519: 0 }
  for (const row of find(KEY).condition({ userId: me, deviceId, isFallback: 'N', claimedDate: null }).selectField('algorithm').list()) {
    counts[row.algorithm] = (counts[row.algorithm] || 0) + 1
  }
  return counts
}

function unusedFallbackTypes(me, deviceId) {
  return Array.from(new Set(Array.from(find(KEY).condition({ userId: me, deviceId, isFallback: 'Y', claimedDate: null })
    .selectField('algorithm').list()).map((row) => row.algorithm)))
}

/** The algorithm of a key ID (signed_curve25519:AAAAHg), or null when it is not one. */
function algorithmOf(keyId) {
  const text = String(keyId || '')
  const colon = text.indexOf(':')
  if (colon < 1 || colon === text.length - 1 || text.length > 255) return null
  return text.slice(0, colon)
}

function uploadKeys(p) {
  const found = ownDevice(p.deviceId, true)
  if (found.refusal) return found.refusal
  const { me, device } = found
  const deviceId = device.deviceId
  const deviceKeys = p.device_keys
  if (deviceKeys != null) {
    if (!isObject(deviceKeys)) return refuse('badKeys', 'The device keys are not an object')
    const text = JSON.stringify(deviceKeys)
    if (text.length > MAX_DEVICE_KEYS_JSON) return refuse('tooLarge', 'The device keys are too large')
    const keys = isObject(deviceKeys.keys) ? deviceKeys.keys : {}
    const ed = keys['ed25519:' + deviceId]
    const curve = keys['curve25519:' + deviceId]
    if (deviceKeys.user_id !== matrixId(me) || deviceKeys.device_id !== deviceId || typeof ed !== 'string' || typeof curve !== 'string' ||
      !isObject(deviceKeys.signatures)) {
      return refuse('badKeys', 'The device keys are not this device\'s')
    }
    if (device.ed25519Key && (device.ed25519Key !== ed || device.curve25519Key !== curve)) {
      return refuse('keysChanged', 'A device\'s identity keys cannot change: remove it and use a new device')
    }
    const first = !device.ed25519Key
    device.setFields({ ed25519Key: ed, curve25519Key: curve, deviceKeysJson: text }, true, null, null)
    if (first) listChanged(device)
  }
  const oneTime = p.one_time_keys
  if (oneTime != null) {
    if (!isObject(oneTime)) return refuse('badKeys', 'The one-time keys are not an object')
    const ids = Object.keys(oneTime)
    if (ids.length > MAX_UPLOAD_KEYS) return refuse('tooManyKeys', 'Too many one-time keys in one upload')
    const held = find(KEY).condition({ userId: me, deviceId, isFallback: 'N', claimedDate: null }).count()
    if (held + ids.length > MAX_DEVICE_KEYS) return refuse('tooManyKeys', 'This device holds as many one-time keys as it may')
    for (const keyId of ids) {
      const algorithm = algorithmOf(keyId)
      const value = oneTime[keyId]
      if (!algorithm || (typeof value !== 'string' && !isObject(value))) return refuse('badKeys', 'A one-time key is not a key')
      const keyJson = JSON.stringify(value)
      if (keyJson.length > 4096) return refuse('tooLarge', 'A one-time key is too large')
      const existing = one(KEY, { userId: me, deviceId, keyId })
      if (existing) {
        if (existing.keyJson !== keyJson) return refuse('keyExists', 'A one-time key with that ID was uploaded before with another value')
        continue
      }
      const row = ec.entity.makeValue(KEY)
      row.setFields({ userId: me, deviceId, keyId, algorithm, keyJson, isFallback: 'N', uploadedDate: ec.user.nowTimestamp }, true, null, null)
      row.create()
    }
  }
  const fallback = p.fallback_keys
  if (fallback != null) {
    if (!isObject(fallback)) return refuse('badKeys', 'The fallback keys are not an object')
    for (const keyId of Object.keys(fallback).slice(0, 10)) {
      const algorithm = algorithmOf(keyId)
      const value = fallback[keyId]
      if (!algorithm || !isObject(value)) return refuse('badKeys', 'A fallback key is not a key')
      const keyJson = JSON.stringify(value)
      if (keyJson.length > 4096) return refuse('tooLarge', 'A fallback key is too large')
      const same = one(KEY, { userId: me, deviceId, keyId })
      if (same && same.keyJson === keyJson && same.isFallback === 'Y') continue
      find(KEY).condition({ userId: me, deviceId, algorithm, isFallback: 'Y' }).deleteAll()
      if (same) same.delete()
      const row = ec.entity.makeValue(KEY)
      row.setFields({ userId: me, deviceId, keyId, algorithm, keyJson, isFallback: 'Y', uploadedDate: ec.user.nowTimestamp }, true, null, null)
      row.create()
    }
  }
  device.set('lastSeenDate', ec.user.nowTimestamp)
  device.update()
  return { one_time_key_counts: unclaimedCounts(me, deviceId) }
}

/** The signed device keys of a device as others see them: as uploaded, with its display name unsigned. */
function publicKeys(device) {
  let keys = null
  try { keys = JSON.parse(device.deviceKeysJson) } catch (err) { return null }
  if (!isObject(keys)) return null
  keys.unsigned = Object.assign({}, isObject(keys.unsigned) ? keys.unsigned : {}, device.displayName ? { device_display_name: device.displayName } : {})
  return keys
}

function queryKeys(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const asked = isObject(p.device_keys) ? p.device_keys : {}
  const ids = Object.keys(asked)
  if (ids.length > MAX_USERS) return refuse('tooMany', 'Too many users in one query')
  const byUser = {}
  for (const id of ids) {
    const userId = userOf(id)
    if (userId) byUser[userId] = id
  }
  const allowed = reachable(me, Object.keys(byUser))
  const out = {}
  for (const userId of allowed) {
    const id = byUser[userId]
    const wanted = Array.isArray(asked[id]) ? asked[id].map(String) : []
    let query = find(DEVICE).condition('userId', userId).condition('revokedDate', null).condition('deviceKeysJson', '!=', null)
    if (wanted.length) query = query.condition('deviceId', 'in', wanted.slice(0, MAX_TARGETS))
    const devices = {}
    for (const device of query.limit(MAX_DEVICES + 10).list()) {
      const keys = publicKeys(device)
      if (keys) devices[device.deviceId] = keys
    }
    out[id] = devices
  }
  return { device_keys: out, failures: {}, master_keys: {}, self_signing_keys: {}, user_signing_keys: {} }
}

/** Takes one key of the device (its row locked first): the oldest unclaimed one-time key, else its fallback key. */
function claimOne(me, userId, deviceId, algorithm) {
  const device = find(DEVICE).condition({ userId, deviceId }).forUpdate(true).one()
  if (!device || device.revokedDate) return null
  const first = (query) => Array.from(query.limit(1).list())[0] || null
  const key = first(find(KEY).condition({ userId, deviceId, algorithm, isFallback: 'N', claimedDate: null }).orderBy('uploadedDate').orderBy('keyId'))
  const chosen = key || first(find(KEY).condition({ userId, deviceId, algorithm, isFallback: 'Y' }).orderBy('-uploadedDate'))
  if (!chosen) return null
  if (!chosen.claimedDate) {
    chosen.setFields({ claimedDate: ec.user.nowTimestamp, claimedByUserId: me }, true, null, null)
    chosen.update()
  }
  let value = null
  try { value = JSON.parse(chosen.keyJson) } catch (err) { return null }
  return { keyId: chosen.keyId, value }
}

function claimKeys(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const asked = isObject(p.one_time_keys) ? p.one_time_keys : {}
  const targets = []
  for (const id of Object.keys(asked)) {
    const userId = userOf(id)
    const devices = asked[id]
    if (!userId || !isObject(devices)) continue
    for (const deviceId of Object.keys(devices)) targets.push({ id, userId, deviceId, algorithm: String(devices[deviceId] || '') })
  }
  if (targets.length > MAX_TARGETS) return refuse('tooMany', 'Too many devices in one claim')
  const allowed = reachable(me, targets.map((t) => t.userId))
  // locked in one order (user, then device), so two claims over the same devices cannot wait on each other
  targets.sort((a, b) => (a.userId === b.userId ? (a.deviceId < b.deviceId ? -1 : 1) : (a.userId < b.userId ? -1 : 1)))
  const out = {}
  for (const t of targets) {
    if (!allowed.has(t.userId) || !t.algorithm) continue
    const claimed = claimOne(me, t.userId, t.deviceId, t.algorithm)
    if (!claimed) continue
    out[t.id] = out[t.id] || {}
    out[t.id][t.deviceId] = { [claimed.keyId]: claimed.value }
  }
  return { one_time_keys: out, failures: {} }
}

// ---- to-device ----

/** Tells a user's tabs, after the commit, that something waits for one of their devices; the push carries no content. */
function wake(userId, message) {
  ec.service.sync().name('org.moqui.impl.ChatServices.push#Signal').parameters({
    topic: 'chatUser:' + userId, userIdList: [userId], message,
  }).disableAuthz().call()
}

function sendToDevice(p) {
  const found = ownDevice(p.deviceId, false)
  if (found.refusal) return found.refusal
  const { me, device: sender } = found
  const eventType = String(p.event_type || '').trim()
  const txnId = String(p.txn_id || '').trim()
  if (!eventType || eventType.length > 255) return refuse('badRequest', 'The event type is required')
  if (!txnId || txnId.length > 255) return refuse('badRequest', 'The transaction ID is required')
  const messages = isObject(p.messages) ? p.messages : {}
  if (find(TO_DEVICE).condition({ senderUserId: me, senderDeviceId: sender.deviceId, txnId }).count() > 0) {
    return { queued: 0, repeated: true }
  }
  const asked = []
  for (const id of Object.keys(messages)) {
    const userId = userOf(id)
    const byDevice = messages[id]
    if (!userId || !isObject(byDevice)) continue
    for (const deviceId of Object.keys(byDevice)) {
      const content = byDevice[deviceId]
      if (!isObject(content)) return refuse('badRequest', 'A message is not an object')
      const contentJson = JSON.stringify(content)
      if (contentJson.length > MAX_CONTENT) return refuse('tooLarge', 'A message is too large')
      asked.push({ userId, deviceId, contentJson })
    }
  }
  if (asked.length > MAX_TARGETS) return refuse('tooMany', 'Too many devices in one send')
  const allowed = reachable(me, asked.map((a) => a.userId))
  const targets = new Map()
  for (const a of asked) {
    if (!allowed.has(a.userId)) continue
    if (a.deviceId === '*') {
      for (const d of find(DEVICE).condition('userId', a.userId).condition('revokedDate', null).selectField('deviceId').limit(MAX_DEVICES + 10).list()) {
        const k = a.userId + '\u0000' + d.deviceId
        if (!targets.has(k)) targets.set(k, { userId: a.userId, deviceId: d.deviceId, contentJson: a.contentJson })
      }
    } else targets.set(a.userId + '\u0000' + a.deviceId, a)
  }
  const ordered = Array.from(targets.values())
  ordered.sort((a, b) => (a.userId === b.userId ? (a.deviceId < b.deviceId ? -1 : 1) : (a.userId < b.userId ? -1 : 1)))
  const woken = new Set()
  let queued = 0
  for (const t of ordered) {
    if (t.userId === me && t.deviceId === sender.deviceId) continue
    const device = find(DEVICE).condition({ userId: t.userId, deviceId: t.deviceId }).forUpdate(true).one()
    if (!device || device.revokedDate) continue
    const pos = Number(device.lastToDevicePos || 0) + 1
    if (pos - Number(device.ackedToDevicePos || 0) > MAX_QUEUED) {
      ec.logger.warn(`To-device message not queued to device ${t.deviceId} of ${t.userId}: ${MAX_QUEUED} are waiting`)
      continue
    }
    device.set('lastToDevicePos', pos)
    device.update()
    const row = ec.entity.makeValue(TO_DEVICE)
    row.setFields({ userId: t.userId, deviceId: t.deviceId, streamPos: pos, eventType, senderUserId: me, senderDeviceId: sender.deviceId,
      txnId, contentJson: t.contentJson, queuedDate: ec.user.nowTimestamp }, true, null, null)
    row.create()
    queued++
    woken.add(t.userId)
  }
  for (const userId of woken) wake(userId, { kind: 'toDevice' })
  return { queued }
}

/**
 * The to-device part of a sync for my device: acknowledges what is at or before since (deleted), answers what comes after
 * (at most limit), the users whose devices changed since deviceListSince, and the device's key counts.
 */
function syncToDevice(p) {
  const found = ownDevice(p.deviceId, true)
  if (found.refusal) return found.refusal
  const { me, device } = found
  const deviceId = device.deviceId
  // never past the last position queued: a cursor ahead of it would acknowledge messages queued after this sync, unread
  const since = Math.min(Math.max(0, Math.floor(Number(p.since) || 0)), Number(device.lastToDevicePos || 0))
  const limit = Math.min(MAX_SYNC, Math.max(1, Math.floor(Number(p.limit) || 100)))
  if (since > 0) {
    find(TO_DEVICE).condition({ userId: me, deviceId }).condition('streamPos', '<=', since).deleteAll()
    if (since > Number(device.ackedToDevicePos || 0)) device.set('ackedToDevicePos', since)
  }
  device.set('lastSeenDate', ec.user.nowTimestamp)
  device.update()
  const queued = Array.from(find(TO_DEVICE).condition({ userId: me, deviceId }).condition('streamPos', '>', since)
    .orderBy('streamPos').limit(limit + 1).list())
  const hasMore = queued.length > limit
  const events = []
  let next = Math.max(since, Number(device.ackedToDevicePos || 0))
  for (const row of queued.slice(0, limit)) {
    next = Number(row.streamPos)
    let content = null
    try { content = JSON.parse(row.contentJson) } catch (err) {
      ec.logger.error(`To-device message ${row.streamPos} of device ${deviceId} of ${me} is not JSON; skipped`)
      continue
    }
    events.push({ type: row.eventType, sender: matrixId(row.senderUserId), content })
  }
  const now = ms(ec.user.nowTimestamp)
  const changed = []
  const listSince = ms(p.deviceListSince)
  if (listSince != null) {
    const users = Array.from(new Set(Array.from(find(DEVICE).condition('listChangedDate', '>', new Date(listSince - LIST_OVERLAP_MS))
      .selectField('userId').limit(5000).list()).map((row) => row.userId)))
    for (const userId of reachable(me, users)) changed.push(matrixId(userId))
  }
  return {
    next_batch: String(next), has_more: hasMore, to_device: { events },
    device_lists: { changed, left: [] }, device_lists_next: now,
    device_one_time_keys_count: unclaimedCounts(me, deviceId),
    device_unused_fallback_key_types: unusedFallbackTypes(me, deviceId),
  }
}

/** The cleanup job: messages waiting longer than chat_to_device_retention_days (default 30), and claimed one-time keys. */
function clean(p) {
  const days = Math.max(1, Number(property('chat_to_device_retention_days')) || 30)
  const now = ms(ec.user.nowTimestamp)
  let messages = 0
  let keys = 0
  try {
    messages = find(TO_DEVICE).condition('queuedDate', '<', new Date(now - days * 86400000)).deleteAll()
  } catch (err) {
    ec.logger.error(`Old to-device messages not cleaned: ${err && err.message ? err.message : err}`)
  }
  try {
    keys = find(KEY).condition('isFallback', 'N').condition('claimedDate', '<', new Date(now - 86400000)).deleteAll()
  } catch (err) {
    ec.logger.error(`Claimed one-time keys not cleaned: ${err && err.message ? err.message : err}`)
  }
  return { messagesDeleted: messages, keysDeleted: keys }
}

function whoami() {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  return { user_id: matrixId(me), serverName: serverName() }
}

/** Internal: Matrix user IDs of userIds, and back (for screens and other services). */
function mapIds(p) {
  const out = {}
  for (const userId of Array.from(p.userIdList || [])) out[userId] = matrixId(userId)
  const back = {}
  for (const id of Array.from(p.matrixIdList || [])) back[id] = userOf(id)
  return { matrixIds: out, userIds: back }
}

/**
 * A /ChatKeys transition (webroot ChatKeys.xml): the service verb with the request parameters names, in its own
 * transaction, answered as JSON (its out-parameters, or {errors, reasonCode}); 401 without a user.
 */
function web(p) {
  if (!ec.user.userId) {
    ec.web.sendJsonError(401, 'Authentication required')
    return {}
  }
  const parameters = ec.web.parameters
  const input = {}
  for (const name of Array.from(p.names || [])) {
    const value = parameters[name]
    if (value !== undefined && value !== null) input[name] = value
  }
  if (p.verb === 'create#ChatDevice') input.userAgent = ec.web.request?.getHeader?.('User-Agent') || null
  const out = ec.service.sync().name('org.moqui.impl.ChatKeyServices.' + p.verb).parameters(input)
    .requireNewTransaction(true).disableAuthz().call() || {}
  if (ec.message.hasError()) {
    const errors = ec.message.errorsString
    ec.message.clearErrors()
    ec.web.sendJsonResponse({ errors, reasonCode: out.reasonCode || 'failed' })
  } else {
    ec.web.sendJsonResponse(out)
  }
  return {}
}

const operations = {
  createDevice, findDevices, renameDevice, revokeDevice, uploadKeys, queryKeys, claimKeys, sendToDevice, syncToDevice,
  clean, whoami, mapIds, web,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown chat key operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
