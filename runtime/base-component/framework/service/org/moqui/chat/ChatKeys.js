// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Chat devices, keys and to-device messages (plans/Comms-Framework.md §4.5, C7 core; docs/interfaces/Chat-Keys.md): the
// services of org.moqui.chat.ChatKeyServices. The browser runs matrix-sdk-crypto's OlmMachine (js/WebrootCrypto.js), which
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
// - **Trust and backup.** Each device's store key (the passphrase its browser store is encrypted with) is answered only to
//   its own user. Cross-signing keys and the signatures users upload are stored and merged into key queries; the server
//   checks who may sign what, not the signatures (the clients do). A key backup holds Megolm sessions the browser
//   encrypted to the user's backup key (Matrix room_keys), opaque here.
// - **The server** stores and answers public keys, ciphertext and routing only. It does not read message content.

const DEVICE = 'moqui.chat.ChatDevice'
const KEY = 'moqui.chat.ChatDeviceKey'
const TO_DEVICE = 'moqui.chat.ChatToDevice'
const CROSS = 'moqui.chat.ChatCrossSigningKey'
const SIGNATURE = 'moqui.chat.ChatKeySignature'
const BACKUP = 'moqui.chat.ChatKeyBackup'
const BACKUP_SESSION = 'moqui.chat.ChatKeyBackupSession'
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
/** Users a sync names in device_lists.left at most, and room memberships it reads to find them. */
const MAX_LEFT = 1000
const MAX_LEFT_ROWS = 5000
/** Signatures in one upload, and the size of one signature. */
const MAX_SIGNATURES = 1000
const MAX_SIGNATURE = 1024
/** Backup versions a user may make; sessions in one put, in one answer, and held by a user; one session's size. */
const MAX_BACKUP_VERSIONS = 1000
const MAX_BACKUP_PUT = 1000
const MAX_BACKUP_GET = 10000
const MAX_BACKUP_SESSIONS = 100000
const MAX_BACKUP_SESSION_JSON = 16384
/** A device-list cursor looks back this far, so a change committed a little late is still seen (repeating one is harmless). */
const LIST_OVERLAP_MS = 120000
/** One-time keys one user may claim of one device in a minute; past it the device's fallback key answers. */
const MAX_CLAIMS_PER_MINUTE = 10
/** The age of a sign-in an identity reset, a new or deleted key backup and a revoke of a device need. */
const RECENT_AUTH_SECONDS = 300
/** The user's preference: room keys only to verified or cross-signed devices, in every encrypted room. */
const VERIFIED_ONLY_PREFERENCE = 'CHAT_VERIFIED_DEVICES_ONLY'
const ROOM = 'moqui.chat.ChatRoom'

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

/**
 * Whether the session's user signed in (or confirmed who they are) in the last five minutes (check#RecentAuth): a password,
 * a second factor's code, an SSO login or a re-authentication; never a key or an internal login.
 */
function recentAuth() {
  const out = ec.service.sync().name('org.moqui.sso.RecentAuthServices.check#RecentAuth')
    .parameters({ maxAgeSeconds: RECENT_AUTH_SECONDS }).disableAuthz().call() || {}
  return out.recent === true
}
const refuseReauth = () => refuse('reauthRequired', 'Confirm it is you: sign in again to do this')

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

/**
 * Whether a revoke of device may go without a recent sign-in: a device with no identity keys (nothing is encrypted to it:
 * a browser dropping a device whose first keys were refused), or the one-time re-key of a store made before the store key
 * was derived (it still has a stored key, legacyStoreKey) to a newer device of mine that replaces it (replacedBy).
 */
function revokeWithoutReauth(me, device, replacedBy) {
  if (!device.ed25519Key) return true
  if (blank(device.storeKey) || blank(replacedBy) || String(replacedBy) === device.deviceId) return false
  const newer = one(DEVICE, { userId: me, deviceId: String(replacedBy) })
  return !!newer && !newer.revokedDate && blank(newer.storeKey)
}

/**
 * Revoke one of my devices: no new keys or messages reach it, its keys and queue go, and it drops out of queries. A device
 * with keys needs a sign-in in the last five minutes (reauthRequired), as the server cannot tell which browser asks.
 */
function revokeDevice(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const device = blank(p.deviceId) ? null : find(DEVICE).condition({ userId: me, deviceId: String(p.deviceId) }).forUpdate(true).one()
  if (!device) return refuse('notFound', 'Device not found')
  if (device.revokedDate) return { deviceId: device.deviceId, repeated: true }
  if (!revokeWithoutReauth(me, device, p.replacedBy) && !recentAuth()) return refuseReauth()
  device.set('revokedDate', ec.user.nowTimestamp)
  // its store can no longer be opened with a key from here: a stored key (from before the derived one) is dropped, and the
  // revision the key is derived from goes up
  device.set('storeKey', null)
  device.set('storeRevision', Number(device.storeRevision || 1) + 1)
  listChanged(device)
  device.update()
  find(KEY).condition({ userId: me, deviceId: device.deviceId }).deleteAll()
  find(SIGNATURE).condition({ targetUserId: me, targetKeyId: device.deviceId }).deleteAll()
  find(SIGNATURE).condition({ signerUserId: me, signerKeyId: 'ed25519:' + device.deviceId }).deleteAll()
  find(TO_DEVICE).condition({ userId: me, deviceId: device.deviceId }).deleteAll()
  wake(me, { kind: 'devices', action: 'revoked', deviceId: device.deviceId })
  // its legs in end-to-end encrypted calls leave them (W8): the others rotate their call keys without it
  ec.resource.script('component://framework/service/org/moqui/call/Calls.js').execute('deviceRevoked', { userId: me, deviceId: device.deviceId })
  return { deviceId: device.deviceId }
}

/**
 * The passphrase my device's browser store is encrypted with (OlmMachine.initialize store_passphrase): derived, not stored
 * (derive#ChatStoreKey: HMAC-SHA-256 under the platform secret chat_store_key_secret of userId|deviceId|revision, 43
 * characters of URL-safe base64), so no table or export holds it. A device made before the derived key still has the
 * random one it was given (ChatDevice.storeKey): it is answered as legacyStoreKey, so the browser opens its store once with
 * it and moves the keys to a new device under the derived key (the old one is then revoked, which drops it). Only for my
 * own device that is not revoked. Never logged.
 */
function storeKey(p) {
  const found = ownDevice(p.deviceId, false)
  if (found.refusal) return found.refusal
  const { me, device } = found
  let derived = {}
  try {
    derived = ec.service.sync().name('org.moqui.chat.ChatKeyServices.derive#ChatStoreKey')
      .parameters({ userId: me, deviceId: device.deviceId, revision: Number(device.storeRevision || 1) }).disableAuthz().call() || {}
  } catch (err) {
    ec.logger.error(`The store key of device ${device.deviceId} of ${me} was not made: ${err && err.message ? err.message : err}`)
  }
  if (ec.message.hasError() || blank(derived.storeKey)) return refuse('failed', 'This device\'s store key could not be made')
  return { storeKey: String(derived.storeKey), legacyStoreKey: blank(device.storeKey) ? null : String(device.storeKey) }
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
  const cross = { master_keys: {}, self_signing_keys: {}, user_signing_keys: {} }
  for (const userId of allowed) {
    const id = byUser[userId]
    const wanted = Array.isArray(asked[id]) ? asked[id].map(String) : []
    let query = find(DEVICE).condition('userId', userId).condition('revokedDate', null).condition('deviceKeysJson', '!=', null)
    if (wanted.length) query = query.condition('deviceId', 'in', wanted.slice(0, MAX_TARGETS))
    const signatures = signaturesOver(me, userId)
    const devices = {}
    for (const device of query.limit(MAX_DEVICES + 10).list()) {
      const keys = publicKeys(device)
      if (keys) devices[device.deviceId] = withSignatures(keys, signatures.get(device.deviceId))
    }
    out[id] = devices
    for (const row of rows(CROSS, { userId })) {
      // the user-signing key only to its own user (Matrix answers it to no one else)
      if (row.keyUsage === 'user_signing' && userId !== me) continue
      let key = null
      try { key = JSON.parse(row.keyJson) } catch (err) {
        ec.logger.error(`Cross-signing key ${row.keyUsage} of ${userId} is not JSON; left out`)
        continue
      }
      cross[row.keyUsage + '_keys'][id] = withSignatures(key, signatures.get(row.publicKey))
    }
  }
  return { device_keys: out, failures: {}, master_keys: cross.master_keys, self_signing_keys: cross.self_signing_keys,
    user_signing_keys: cross.user_signing_keys }
}

/**
 * Takes one key of the device (its row locked first): the oldest unclaimed one-time key, else its fallback key. A claimer
 * who took MAX_CLAIMS_PER_MINUTE of the device's one-time keys in the last minute gets the fallback key, so one user
 * cannot drain a device's one-time keys (Olm sessions on a fallback key still work).
 */
function claimOne(me, userId, deviceId, algorithm) {
  const device = find(DEVICE).condition({ userId, deviceId }).forUpdate(true).one()
  if (!device || device.revokedDate) return null
  const first = (query) => Array.from(query.limit(1).list())[0] || null
  const recent = find(KEY).condition({ userId, deviceId, isFallback: 'N', claimedByUserId: me })
    .condition('claimedDate', '>', new Date(ms(ec.user.nowTimestamp) - 60000)).count()
  const key = recent >= MAX_CLAIMS_PER_MINUTE ? null
    : first(find(KEY).condition({ userId, deviceId, algorithm, isFallback: 'N', claimedDate: null }).orderBy('uploadedDate').orderBy('keyId'))
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
  ec.service.sync().name('org.moqui.chat.ChatServices.push#Signal').parameters({
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
  // The rows are locked in one order (user, then device), the sender's own among them, before the transaction is looked
  // up: two sends of one transaction at once (a retry) wait on the sender's row, and the second finds the first's copies.
  // One order everywhere (claims too) means two sends cannot wait on each other.
  const senderKey = me + '\u0000' + sender.deviceId
  const order = (a, b) => (a.userId === b.userId ? (a.deviceId < b.deviceId ? -1 : 1) : (a.userId < b.userId ? -1 : 1))
  const ordered = Array.from(targets.values()).filter((t) => t.userId + '\u0000' + t.deviceId !== senderKey)
  const locks = ordered.concat([{ userId: me, deviceId: sender.deviceId, sender: true }])
  locks.sort(order)
  const rowsOf = new Map()
  for (const t of locks) rowsOf.set(t.userId + '\u0000' + t.deviceId, find(DEVICE).condition({ userId: t.userId, deviceId: t.deviceId }).forUpdate(true).one())
  if (find(TO_DEVICE).condition({ senderUserId: me, senderDeviceId: sender.deviceId, txnId }).count() > 0) {
    return { queued: 0, repeated: true }
  }
  ordered.sort(order)
  const woken = new Set()
  let queued = 0
  for (const t of ordered) {
    const device = rowsOf.get(t.userId + '\u0000' + t.deviceId)
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
  const left = listSince == null ? [] : leftUsers(me, listSince)
  return {
    next_batch: String(next), has_more: hasMore, to_device: { events },
    device_lists: { changed, left }, device_lists_next: now,
    device_one_time_keys_count: unclaimedCounts(me, deviceId),
    device_unused_fallback_key_types: unusedFallbackTypes(me, deviceId),
  }
}

// ---- cross-signing (keys/device_signing/upload, keys/signatures/upload) ----

const USAGES = ['master', 'self_signing', 'user_signing']
const PARAMETER_OF = { master: 'master_key', self_signing: 'self_signing_key', user_signing: 'user_signing_key' }
/** An ed25519 public key as Matrix writes it (unpadded base64), or a device ID. */
const KEY_TEXT = /^[A-Za-z0-9+/=_-]{1,64}$/

/** The signatures stored over userId's keys that I may see (theirs, and mine), by target key ID. */
function signaturesOver(me, userId) {
  const byTarget = new Map()
  const signers = Array.from(new Set([userId, me]))
  for (const row of find(SIGNATURE).condition('targetUserId', userId).condition('signerUserId', 'in', signers).limit(5000).list()) {
    if (!byTarget.has(row.targetKeyId)) byTarget.set(row.targetKeyId, [])
    byTarget.get(row.targetKeyId).push(row)
  }
  return byTarget
}

/** A signed object with the stored signatures merged into its signatures (signer's Matrix user ID, then key ID). */
function withSignatures(object, list) {
  if (!list || !list.length) return object
  const signatures = isObject(object.signatures) ? object.signatures : {}
  for (const row of list) {
    const signer = matrixId(row.signerUserId)
    signatures[signer] = Object.assign({}, isObject(signatures[signer]) ? signatures[signer] : {}, { [row.signerKeyId]: row.signature })
  }
  object.signatures = signatures
  return object
}

/** Users' device lists changed (cross-signing keys or signatures): their devices' list revisions go up, so syncs name them. */
function usersChanged(userIds) {
  for (const userId of userIds) {
    for (const device of find(DEVICE).condition('userId', userId).condition('revokedDate', null).limit(MAX_DEVICES + 10).list()) {
      listChanged(device)
      device.update()
    }
  }
}

/** An uploaded cross-signing key of usage: {publicKey, keyJson, value}, or {reasonCode, text}. */
function crossSigningKey(me, usage, value) {
  if (!isObject(value)) return { reasonCode: 'badKeys', text: 'A cross-signing key is not an object' }
  const keyJson = JSON.stringify(value)
  if (keyJson.length > MAX_DEVICE_KEYS_JSON) return { reasonCode: 'tooLarge', text: 'A cross-signing key is too large' }
  if (value.user_id !== matrixId(me)) return { reasonCode: 'badKeys', text: 'A cross-signing key is not yours' }
  if (!Array.isArray(value.usage) || !value.usage.includes(usage)) {
    return { reasonCode: 'badKeys', text: 'A cross-signing key does not have the usage it is uploaded as' }
  }
  const ids = isObject(value.keys) ? Object.keys(value.keys) : []
  const publicKey = ids.length === 1 && ids[0].startsWith('ed25519:') ? ids[0].slice(8) : ''
  if (!KEY_TEXT.test(publicKey) || value.keys[ids[0]] !== publicKey) {
    return { reasonCode: 'badKeys', text: 'A cross-signing key must hold one ed25519 key' }
  }
  return { publicKey, keyJson, value }
}

/** Deletes cross-signing keys of mine and every signature by them or over them. */
function deleteCrossSigning(me, keys) {
  for (const key of keys) {
    find(SIGNATURE).condition({ signerUserId: me, signerKeyId: 'ed25519:' + key.publicKey }).deleteAll()
    find(SIGNATURE).condition({ targetUserId: me, targetKeyId: key.publicKey }).deleteAll()
    key.delete()
  }
}

/**
 * Matrix keys/device_signing/upload for me. The session stands for Matrix's user-interactive auth: the first upload is
 * allowed, the same keys again change nothing, and a different master key replaces my identity only with reset (else
 * keysChanged); a reset deletes the old keys and every signature by or over them. Without a master key in the upload I
 * must have one; a self-signing or user-signing key must carry a signature by the master key (present; not checked).
 */
function uploadCrossSigningKeys(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const uploaded = {}
  for (const usage of USAGES) {
    const value = p[PARAMETER_OF[usage]]
    if (value == null) continue
    const key = crossSigningKey(me, usage, value)
    if (key.reasonCode) return refuse(key.reasonCode, key.text)
    uploaded[usage] = key
  }
  if (!Object.keys(uploaded).length) return refuse('badKeys', 'No cross-signing key was given')
  const current = {}
  for (const row of find(CROSS).condition('userId', me).forUpdate(true).list()) current[row.keyUsage] = row
  const reset = p.reset === true || p.reset === 'true' || p.reset === 'Y'
  const master = uploaded.master
  const replacesMaster = !!(master && current.master && current.master.publicKey !== master.publicKey)
  if (replacesMaster && !reset) {
    return refuse('keysChanged', 'Your identity (master key) cannot change unless you choose to reset it')
  }
  // a reset replaces who the user is to everyone they talk to: it needs a sign-in in the last five minutes
  if (replacesMaster && !recentAuth()) return refuseReauth()
  if (!master && !current.master) return refuse('noMasterKey', 'Upload the master key first')
  const masterKey = master ? master.publicKey : current.master.publicKey
  for (const usage of ['self_signing', 'user_signing']) {
    const key = uploaded[usage]
    if (!key) continue
    const mine = isObject(key.value.signatures) ? key.value.signatures[matrixId(me)] : null
    const signature = isObject(mine) ? mine['ed25519:' + masterKey] : null
    if (typeof signature !== 'string' || !signature) return refuse('badKeys', 'A cross-signing key is not signed by the master key')
  }
  let changed = false
  if (replacesMaster) {
    deleteCrossSigning(me, Object.values(current))
    for (const usage of USAGES) delete current[usage]
    changed = true
  }
  for (const usage of USAGES) {
    const key = uploaded[usage]
    if (!key) continue
    const old = current[usage]
    if (old && old.publicKey === key.publicKey) {
      if (old.keyJson !== key.keyJson) {
        old.setFields({ keyJson: key.keyJson, uploadedDate: ec.user.nowTimestamp }, true, null, null)
        old.update()
        changed = true
      }
      continue
    }
    if (old) deleteCrossSigning(me, [old])
    const row = ec.entity.makeValue(CROSS)
    row.setFields({ userId: me, keyUsage: usage, publicKey: key.publicKey, keyJson: key.keyJson, uploadedDate: ec.user.nowTimestamp },
      true, null, null)
    row.create()
    changed = true
  }
  if (changed) usersChanged([me])
  return { changed }
}

/**
 * Which of my keys may sign targetKeyId of userId (Matrix user ID id), as signer key IDs, or {errcode, error}: my own
 * device by my self-signing key, my master key by my devices, my self-signing and user-signing keys by my master key, and
 * the master key of another user in reach by my user-signing key. object must be the key it names.
 */
function signers(me, mine, userId, id, keyId, object, allowed) {
  const forbidden = { errcode: 'M_FORBIDDEN', error: 'Not a key you may sign' }
  if (!userId || !KEY_TEXT.test(keyId)) return forbidden
  if (!isObject(object) || object.user_id !== matrixId(userId) || !isObject(object.keys)) {
    return { errcode: 'M_INVALID_PARAM', error: 'The signed object is not the key it names' }
  }
  const crossKey = (publicKey) => object.keys['ed25519:' + publicKey] === publicKey
  const by = (usage) => (mine[usage] ? ['ed25519:' + mine[usage]] : [])
  if (userId === me) {
    const usage = USAGES.find((u) => mine[u] === keyId)
    if (usage) {
      if (!crossKey(keyId)) return { errcode: 'M_INVALID_PARAM', error: 'The signed object is not the key it names' }
      if (usage !== 'master') return by('master')
      return Array.from(find(DEVICE).condition('userId', me).condition('revokedDate', null).condition('ed25519Key', '!=', null)
        .selectField('deviceId').limit(MAX_DEVICES + 10).list()).map((d) => 'ed25519:' + d.deviceId)
    }
    const device = one(DEVICE, { userId: me, deviceId: keyId })
    if (!device || device.revokedDate || !device.ed25519Key) return { errcode: 'M_NOT_FOUND', error: 'Device not found' }
    if (object.device_id !== keyId || object.keys['ed25519:' + keyId] !== device.ed25519Key) {
      return { errcode: 'M_INVALID_PARAM', error: 'The signed object is not the key it names' }
    }
    return by('self_signing')
  }
  if (!allowed.has(userId)) return forbidden
  const master = one(CROSS, { userId, keyUsage: 'master' })
  if (!master || master.publicKey !== keyId) return forbidden
  if (!crossKey(keyId)) return { errcode: 'M_INVALID_PARAM', error: 'The signed object is not the key it names' }
  return by('user_signing')
}

/**
 * Matrix keys/signatures/upload: signed is the request body, Matrix user ID to key ID (a device ID or a cross-signing
 * public key) to the signed object. Stores the signatures in it by my keys that may sign that key; the others are
 * ignored. A key refused is in failures (Matrix user ID to key ID to {errcode, error}).
 */
function uploadKeySignatures(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  if (!isObject(p.signed)) return refuse('badRequest', 'The signatures are not an object')
  const signed = p.signed
  let total = 0
  for (const id of Object.keys(signed)) total += isObject(signed[id]) ? Object.keys(signed[id]).length : 1
  if (total > MAX_SIGNATURES) return refuse('tooMany', 'Too many signatures in one upload')
  const myId = matrixId(me)
  const mine = {}
  for (const row of rows(CROSS, { userId: me })) mine[row.keyUsage] = row.publicKey
  const targets = Object.keys(signed).map((id) => ({ id, userId: userOf(id) }))
  const allowed = reachable(me, targets.map((t) => t.userId).filter((u) => u && u !== me))
  const failures = {}
  const fail = (id, keyId, failure) => {
    failures[id] = failures[id] || {}
    failures[id][keyId] = failure
  }
  const changedUsers = new Set()
  for (const { id, userId } of targets) {
    const objects = signed[id]
    if (!isObject(objects)) {
      fail(id, '', { errcode: 'M_BAD_JSON', error: 'Not an object of keys' })
      continue
    }
    for (const keyId of Object.keys(objects)) {
      const object = objects[keyId]
      const may = signers(me, mine, userId, id, keyId, object, allowed)
      if (!Array.isArray(may)) {
        fail(id, keyId, may)
        continue
      }
      const theirs = isObject(object.signatures) ? object.signatures[myId] : null
      const accepted = isObject(theirs) ? Object.keys(theirs).filter((signerKeyId) => may.includes(signerKeyId) &&
        typeof theirs[signerKeyId] === 'string' && theirs[signerKeyId] && theirs[signerKeyId].length <= MAX_SIGNATURE) : []
      if (!accepted.length) {
        fail(id, keyId, { errcode: 'M_INVALID_SIGNATURE', error: 'No signature by a key of yours that may sign this key' })
        continue
      }
      for (const signerKeyId of accepted) {
        const fields = { targetUserId: userId, targetKeyId: keyId, signerUserId: me, signerKeyId }
        const existing = one(SIGNATURE, fields)
        if (existing && existing.signature === theirs[signerKeyId]) continue
        if (existing) {
          existing.setFields({ signature: theirs[signerKeyId], uploadedDate: ec.user.nowTimestamp }, true, null, null)
          existing.update()
        } else {
          const row = ec.entity.makeValue(SIGNATURE)
          row.setFields(Object.assign({ signature: theirs[signerKeyId], uploadedDate: ec.user.nowTimestamp }, fields), true, null, null)
          row.create()
        }
        changedUsers.add(userId)
      }
    }
  }
  usersChanged(Array.from(changedUsers).sort())
  return { failures }
}

// ---- device_lists.left ----

/**
 * The Matrix user IDs I was in reach of through a room and no longer am (no shared room, no live call), because a
 * membership ended since listSince: a co-member who left a room I am or was in, or the members of a room I left.
 */
function leftUsers(me, listSince) {
  const since = new Date(listSince - LIST_OVERLAP_MS)
  const myRooms = Array.from(new Set(Array.from(find(MEMBER).condition('userId', me).selectField('chatRoomId').limit(10000).list())
    .map((row) => row.chatRoomId)))
  const candidates = new Set()
  let read = 0
  for (const rooms of chunks(myRooms, 500)) {
    if (read >= MAX_LEFT_ROWS) break
    const ended = Array.from(find(MEMBER).condition('chatRoomId', 'in', rooms).condition('leftDate', '>', since)
      .selectField('chatRoomId').selectField('userId').limit(MAX_LEFT_ROWS - read).list())
    read += ended.length
    const roomsLeft = []
    for (const row of ended) {
      if (row.userId === me) roomsLeft.push(row.chatRoomId)
      else if (row.userId) candidates.add(row.userId)
    }
    // of a room I left: its members then (still there, or gone since)
    for (const left of chunks(roomsLeft, 500)) {
      for (const row of find(MEMBER).condition('chatRoomId', 'in', left).selectField('userId').selectField('leftDate')
        .limit(MAX_LEFT_ROWS).list()) {
        if (row.userId && (!row.leftDate || ms(row.leftDate) > since.getTime())) candidates.add(row.userId)
      }
    }
  }
  candidates.delete(me)
  const users = Array.from(candidates).sort().slice(0, MAX_LEFT)
  const still = reachable(me, users)
  return users.filter((u) => !still.has(u)).map(matrixId)
}

// ---- key backup (room_keys) ----

/** My backup versions not deleted; the current one is the latest. */
function currentBackup(me, lock) {
  let latest = null
  for (const row of find(BACKUP).condition('userId', me).condition('deletedDate', null).selectField('version')
    .limit(MAX_BACKUP_VERSIONS + 10).list()) {
    if (!latest || Number(row.version) > Number(latest)) latest = row.version
  }
  return latest ? liveBackup(me, latest, lock) : null
}

/** My backup version, not deleted, or null. */
function liveBackup(me, version, lock) {
  if (blank(version)) return null
  const row = find(BACKUP).condition({ userId: me, version: String(version).trim() }).forUpdate(!!lock).one()
  return row && !row.deletedDate ? row : null
}

const backupCount = (me, version) => find(BACKUP_SESSION).condition({ userId: me, version }).count()

function parseJson(text, what) {
  try { return JSON.parse(text) } catch (err) {
    ec.logger.error(`${what} is not JSON; answered as null`)
    return null
  }
}

/** The backup's auth_data from the request, as stored JSON, or a refusal. */
function authData(p) {
  if (!isObject(p.auth_data)) return { refusal: refuse('badRequest', 'The backup auth_data is not an object') }
  const json = JSON.stringify(p.auth_data)
  if (json.length > MAX_DEVICE_KEYS_JSON) return { refusal: refuse('tooLarge', 'The backup auth_data is too large') }
  return { json }
}

/** A new backup version of mine (versions "1", "2", ..., never reused); it is the current one. */
function createBackupVersion(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const algorithm = String(p.algorithm || '').trim()
  if (!algorithm || algorithm.length > 255) return refuse('badRequest', 'The backup algorithm is required')
  const auth = authData(p)
  if (auth.refusal) return auth.refusal
  // a new backup takes the room keys of the user's devices from now on, encrypted to its key: it needs a recent sign-in
  if (!recentAuth()) return refuseReauth()
  const made = find(BACKUP).condition('userId', me).count()
  if (made >= MAX_BACKUP_VERSIONS) return refuse('tooMany', 'Too many key backup versions')
  let number = made + 1
  while (one(BACKUP, { userId: me, version: String(number) })) number++
  const row = ec.entity.makeValue(BACKUP)
  row.setFields({ userId: me, version: String(number), algorithm, authDataJson: auth.json, etag: 0, createdDate: ec.user.nowTimestamp },
    true, null, null)
  row.create()
  return { version: String(number) }
}

function getBackupVersion(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const row = blank(p.version) ? currentBackup(me, false) : liveBackup(me, p.version, false)
  if (!row) return refuse('notFound', 'No key backup')
  return {
    algorithm: row.algorithm, auth_data: parseJson(row.authDataJson, `Key backup ${row.version} auth_data of ${me}`),
    count: backupCount(me, row.version), etag: String(row.etag || 0), version: row.version,
  }
}

function updateBackupVersion(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const row = liveBackup(me, p.version, true)
  if (!row) return refuse('notFound', 'No key backup')
  if (String(p.algorithm || '').trim() !== row.algorithm) return refuse('algorithmChanged', 'A key backup\'s algorithm cannot change')
  const auth = authData(p)
  if (auth.refusal) return auth.refusal
  // new signatures need nothing more; a new public key sends the backup's keys from now on to another key, as a new
  // version does: it needs a recent sign-in
  const before = parseJson(row.authDataJson, `Key backup ${row.version} auth_data of ${me}`)
  const keyOf = (data) => (isObject(data) && data.public_key != null ? String(data.public_key) : '')
  if (keyOf(before) !== keyOf(p.auth_data) && !recentAuth()) return refuseReauth()
  row.set('authDataJson', auth.json)
  row.update()
  return { version: row.version }
}

function deleteBackupVersion(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const row = liveBackup(me, p.version, true)
  if (!row) return refuse('notFound', 'No key backup')
  if (!recentAuth()) return refuseReauth()
  find(BACKUP_SESSION).condition({ userId: me, version: row.version }).deleteAll()
  row.set('deletedDate', ec.user.nowTimestamp)
  row.update()
  return { version: row.version }
}

const isCount = (value) => Number.isInteger(value) && value >= 0

/** Matrix's rule: a verified copy wins, then the lower first message index, then the lower forwarded count. */
function better(session, old) {
  const oldVerified = old.isVerified === 'Y'
  if (session.is_verified !== oldVerified) return session.is_verified
  if (session.first_message_index !== Number(old.firstMessageIndex)) return session.first_message_index < Number(old.firstMessageIndex)
  return session.forwarded_count < Number(old.forwardedCount)
}

/**
 * Matrix PUT room_keys/keys into my current backup version (another version: wrongVersion with current_version). Each
 * session is stored, or replaces the one there only when better. etag goes up when anything changed.
 */
function storeBackupKeys(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const current = currentBackup(me, true)
  if (!current) return refuse('notFound', 'No key backup')
  if (String(p.version == null ? '' : p.version).trim() !== current.version) {
    return Object.assign(refuse('wrongVersion', 'That is not the current key backup version'), { current_version: current.version })
  }
  if (!isObject(p.rooms)) return refuse('badRequest', 'The rooms are not an object')
  const sessions = []
  for (const roomId of Object.keys(p.rooms)) {
    const room = p.rooms[roomId]
    if (!roomId || roomId.length > 255 || !isObject(room) || !isObject(room.sessions)) return refuse('badRequest', 'A room is not a room of sessions')
    for (const sessionId of Object.keys(room.sessions)) {
      if (sessions.length >= MAX_BACKUP_PUT) return refuse('tooMany', 'Too many sessions in one backup upload')
      const data = room.sessions[sessionId]
      if (!sessionId || sessionId.length > 255 || !isObject(data) || !isCount(data.first_message_index) || !isCount(data.forwarded_count) ||
        typeof data.is_verified !== 'boolean' || !isObject(data.session_data)) {
        return refuse('badRequest', 'A backed-up session is not KeyBackupData')
      }
      const sessionDataJson = JSON.stringify(data.session_data)
      if (sessionDataJson.length > MAX_BACKUP_SESSION_JSON) return refuse('tooLarge', 'A backed-up session is too large')
      sessions.push({ roomId, sessionId, first_message_index: data.first_message_index, forwarded_count: data.forwarded_count,
        is_verified: data.is_verified, sessionDataJson, existing: one(BACKUP_SESSION, { userId: me, version: current.version, roomId, sessionId }) })
    }
  }
  const added = sessions.filter((s) => !s.existing).length
  if (added && find(BACKUP_SESSION).condition('userId', me).count() + added > MAX_BACKUP_SESSIONS) {
    return refuse('tooMany', 'The key backup holds as many sessions as it may')
  }
  let changed = 0
  for (const s of sessions) {
    if (s.existing && !better(s, s.existing)) continue
    const row = s.existing || ec.entity.makeValue(BACKUP_SESSION)
    row.setFields({ userId: me, version: current.version, roomId: s.roomId, sessionId: s.sessionId, firstMessageIndex: s.first_message_index,
      forwardedCount: s.forwarded_count, isVerified: s.is_verified ? 'Y' : 'N', sessionDataJson: s.sessionDataJson,
      uploadedDate: ec.user.nowTimestamp }, true, null, null)
    if (s.existing) row.update()
    else row.create()
    changed++
  }
  if (changed) {
    current.set('etag', Number(current.etag || 0) + 1)
    current.update()
  }
  return { count: backupCount(me, current.version), etag: String(current.etag || 0) }
}

/** The sessions of my backup version (all, a room's, or one), at most MAX_BACKUP_GET from offset; has_more when more follow. */
function getBackupKeys(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const row = liveBackup(me, p.version, false)
  if (!row) return refuse('notFound', 'No key backup')
  if (!blank(p.sessionId) && blank(p.roomId)) return refuse('badRequest', 'A session is named with its room')
  let query = find(BACKUP_SESSION).condition({ userId: me, version: row.version })
  if (!blank(p.roomId)) query = query.condition('roomId', String(p.roomId))
  if (!blank(p.sessionId)) query = query.condition('sessionId', String(p.sessionId))
  const offset = Math.max(0, Math.floor(Number(p.offset) || 0))
  const list = Array.from(query.orderBy('roomId').orderBy('sessionId').offset(offset).limit(MAX_BACKUP_GET + 1).list())
  const rooms = {}
  for (const s of list.slice(0, MAX_BACKUP_GET)) {
    const room = rooms[s.roomId] || (rooms[s.roomId] = { sessions: {} })
    room.sessions[s.sessionId] = {
      first_message_index: Number(s.firstMessageIndex || 0), forwarded_count: Number(s.forwardedCount || 0),
      is_verified: s.isVerified === 'Y', session_data: parseJson(s.sessionDataJson, `Backed-up session ${s.sessionId} of ${me}`),
    }
  }
  return { rooms, has_more: list.length > MAX_BACKUP_GET }
}

/** Deletes sessions of my backup version (all, a room's, or one); etag goes up when any was deleted. */
function deleteBackupKeys(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const row = liveBackup(me, p.version, true)
  if (!row) return refuse('notFound', 'No key backup')
  if (!blank(p.sessionId) && blank(p.roomId)) return refuse('badRequest', 'A session is named with its room')
  let query = find(BACKUP_SESSION).condition({ userId: me, version: row.version })
  if (!blank(p.roomId)) query = query.condition('roomId', String(p.roomId))
  if (!blank(p.sessionId)) query = query.condition('sessionId', String(p.sessionId))
  // deleting backed-up keys loses them as deleting the backup does: it needs a recent sign-in
  if (!recentAuth()) return refuseReauth()
  if (query.deleteAll() > 0) {
    row.set('etag', Number(row.etag || 0) + 1)
    row.update()
  }
  return { count: backupCount(me, row.version), etag: String(row.etag || 0) }
}

/**
 * My backup versions not deleted, newest first: version, algorithm, the backup's public key (auth_data.public_key, so a
 * browser finds the version its recovery key opens), count, createdDate, and current (the latest). An older one still
 * holds the keys backed up to it until it is deleted.
 */
function findBackupVersions() {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const list = Array.from(find(BACKUP).condition('userId', me).condition('deletedDate', null).limit(MAX_BACKUP_VERSIONS + 10).list())
  list.sort((a, b) => Number(b.version) - Number(a.version))
  const versionList = list.map((row, index) => {
    const auth = parseJson(row.authDataJson, `Key backup ${row.version} auth_data of ${me}`)
    return { version: row.version, algorithm: row.algorithm, publicKey: isObject(auth) ? auth.public_key || null : null,
      count: backupCount(me, row.version), createdDate: ms(row.createdDate), current: index === 0 }
  })
  return { versionList }
}

// ---- only verified devices (room keys to verified or cross-signed devices only) ----

const isTrue = (value) => value === true || value === 'true' || value === 'Y'

/**
 * My encryption settings: verifiedDevicesOnly (mine, for every encrypted room), and with chatRoomId the room's
 * (roomVerifiedDevicesOnly, set by its moderators; false for a room I am not a member of). The browser shares a room's keys
 * only with verified or cross-signed devices when either is on.
 */
function getKeySettings(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  let roomVerifiedDevicesOnly = false
  if (!blank(p.chatRoomId)) {
    const member = find(MEMBER).condition({ chatRoomId: String(p.chatRoomId), userId: me }).condition('leftDate', null).count() > 0
    const room = member ? one(ROOM, { chatRoomId: String(p.chatRoomId) }) : null
    roomVerifiedDevicesOnly = !!room && room.verifiedDevicesOnly === 'Y'
  }
  return { verifiedDevicesOnly: ec.user.getPreference(VERIFIED_ONLY_PREFERENCE) === 'Y', roomVerifiedDevicesOnly }
}

/** My setting: room keys only to verified or cross-signed devices, in every encrypted room. */
function storeKeySettings(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const on = isTrue(p.verifiedDevicesOnly)
  ec.user.setPreference(VERIFIED_ONLY_PREFERENCE, on ? 'Y' : 'N')
  return { verifiedDevicesOnly: on }
}

/**
 * An encrypted room's setting, by a moderator or owner: its members' browsers share its keys only with verified or
 * cross-signed devices. reasonCode notFound (not a member), notModerator, notEncrypted.
 */
function setRoomVerifiedOnly(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const access = ec.service.sync().name('org.moqui.chat.ChatServices.get#RoomAccess').parameter('chatRoomId', String(p.chatRoomId || ''))
    .call() || {}
  if (ec.message.hasError()) return { reasonCode: 'failed' }
  if (!access.isMember) return refuse('notFound', 'Chat room not found')
  if (!access.canModerate) return refuse('notModerator', 'Only a moderator or owner may change the room')
  const room = find(ROOM).condition('chatRoomId', String(p.chatRoomId)).forUpdate(true).one()
  if (!room || room.encrypted !== 'Y') return refuse('notEncrypted', 'Only an encrypted room has this setting')
  const on = isTrue(p.verifiedDevicesOnly)
  room.set('verifiedDevicesOnly', on ? 'Y' : 'N')
  room.update()
  return { verifiedDevicesOnly: on }
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
  const out = ec.service.sync().name('org.moqui.chat.ChatKeyServices.' + p.verb).parameters(input)
    .requireNewTransaction(true).disableAuthz().call() || {}
  if (ec.message.hasError()) {
    const errors = ec.message.errorsString
    ec.message.clearErrors()
    // a refusal's own fields stay (current_version of wrongVersion)
    ec.web.sendJsonResponse(Object.assign({}, out, { errors, reasonCode: out.reasonCode || 'failed' }))
  } else {
    ec.web.sendJsonResponse(out)
  }
  return {}
}

const operations = {
  createDevice, findDevices, renameDevice, revokeDevice, storeKey, uploadKeys, queryKeys, claimKeys, sendToDevice, syncToDevice,
  uploadCrossSigningKeys, uploadKeySignatures, createBackupVersion, getBackupVersion, updateBackupVersion,
  deleteBackupVersion, storeBackupKeys, getBackupKeys, deleteBackupKeys, clean, whoami, mapIds, web,
  findBackupVersions, getKeySettings, storeKeySettings, setRoomVerifiedOnly,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown chat key operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
