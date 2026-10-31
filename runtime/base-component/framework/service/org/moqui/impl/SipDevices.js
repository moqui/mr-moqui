// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Desk and soft phones (plans/Comms-Framework.md §5.2 "Registrar for phones", V4): the services of
// org.moqui.impl.SipDeviceServices. A user adds a phone and gets its SIP credentials once (a generated username and a strong
// password, of which only digest verifiers are kept, encrypted: SHA-256, and MD5 too for a legacy profile); rotating them
// makes new ones and ends the old verifiers and the phone's registrations; revoking the phone ends it. The SIP engine of a
// node (moqui-host src/sip/phones.rs) is the registrar: it asks get#SipDeviceAuth for a device's verifiers when it answers a
// challenge, keeps each binding as a SipRegistration row (store#SipRegistration), rings a user's devices when the call
// services ring the user (ring#SipDevices, from Calls.js ring), and hands a phone's own call to receive#DeviceCall, which
// places it as the phone's user: to another user (by username), or to a number through place#PhoneCall's policy. The
// registrar is off unless sip_registrar_enabled is true; then nothing rings and no call is taken, though phones may be added.

const CALLS = 'component://framework/service/org/moqui/impl/Calls.js'
const VOICE = 'component://framework/service/org/moqui/impl/Voice.js'
const DEVICE = 'moqui.call.SipDevice'
const VERIFIER = 'moqui.call.SipDeviceVerifier'
const REGISTRATION = 'moqui.call.SipRegistration'
const SESSION = 'moqui.call.CallSession'
const PARTICIPANT = 'moqui.call.CallParticipant'
const LEG = 'moqui.call.CallLeg'
const SIP_NODE = 'moqui.call.SipNode'
/** The phones a user may have at once. */
const MAX_DEVICES = 10
/** A SIP node with no heartbeat in this long is not up (Voice.js NODE_STALE_MS). */
const NODE_STALE_MS = 60000
const STATUS_CODES = { SdsActive: 'active', SdsRevoked: 'revoked' }

const ms = (value) => {
  if (value == null || value === '') return null
  const number = Number(value)
  return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}
const nowMs = () => ms(ec.user.nowTimestamp)
const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const rows = (entity, fields) => Array.from(find(entity).condition(fields || {}).list())
const yes = (value) => value === true || value === 'Y' || value === 'true' || value === 'y' || value === '1'
const blank = (value) => value == null || String(value).trim() === ''
const property = (name) => String(System.getProperty(name) || '').trim()

function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

function isAdmin() {
  return !!ec.user.userId && ec.user.hasPermission('VoiceAdmin')
}

/** Whether the registrar is on: sip_registrar_enabled (off by default, even with the SIP listeners on). */
function registrarOn() {
  return yes(property('sip_registrar_enabled').toLowerCase())
}

/** The realm of the verifiers and the domain phones register to, as the SIP engine takes it (moqui-host sip settings). */
function domain() {
  return property('sip_domain') || property('sip_public_address') || 'moqui'
}

function liveSipNodes() {
  return rows(SIP_NODE).filter((n) => ms(n.heartbeatDate) >= nowMs() - NODE_STALE_MS)
}

/** What a phone is set up with: the domain, the server's ports, and the transport to choose. */
function serverSetup() {
  const port = (address, name) => {
    const fromNode = address ? Number(String(address).split(':').pop()) : null
    return fromNode || (property(name) ? Number(property(name)) : null)
  }
  const node = liveSipNodes()[0]
  const sipPort = port(node?.sipAddress, 'sip_port')
  const tlsPort = port(node?.tlsAddress, 'sip_tls_port')
  return { domain: domain(), sipPort, tlsPort, transport: tlsPort ? 'TLS' : sipPort ? 'UDP' : null, registrarEnabled: registrarOn() }
}

function userName(userId) {
  const account = one('moqui.security.UserAccount', { userId })
  return account ? account.userFullName || account.username || userId : userId
}

/** Tells a node's SIP engine (after the commit); one that cannot be told is logged. */
function sipControl(instanceId, message) {
  ec.message.pushErrors()
  try {
    ec.service.sync().name('org.moqui.impl.VoiceServices.send#SipControl').parameters({ instanceId, message }).disableAuthz().call()
    if (ec.message.hasError()) ec.logger.error(`SIP engine ${instanceId} not told ${message.action}: ${ec.message.errorsString}`)
  } catch (err) {
    ec.logger.error(`SIP engine ${instanceId} not told ${message.action}: ${err && err.message ? err.message : err}`)
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
}

/** A new password and its verifiers for this realm (make#SipCredential, Rust); the username is generated when none. */
function makeCredential(realm, username, legacy) {
  return ec.service.sync().name('org.moqui.impl.SipDeviceServices.make#SipCredential')
    .parameters({ realm, username: username || null, legacy: !!legacy }).disableAuthz().call() || {}
}

function storeVerifiers(device, verifierList, revision) {
  for (const old of rows(VERIFIER, { sipDeviceId: device.sipDeviceId })) old.delete()
  for (const v of Array.from(verifierList || [])) {
    const row = ec.entity.makeValue(VERIFIER)
    row.setFields({ sipDeviceId: device.sipDeviceId, algorithm: v.algorithm, realm: device.realm, ha1: v.ha1,
      credentialRevision: revision }, true, null, null)
    row.create()
  }
}

/** Its registrations end: the rows go, and each live SIP engine drops its bindings and hangs up its legs. */
function endRegistrations(device) {
  for (const row of rows(REGISTRATION, { sipDeviceId: device.sipDeviceId })) row.delete()
  for (const node of liveSipNodes()) {
    sipControl(node.instanceId, { action: 'dropDevice', sipDeviceId: device.sipDeviceId, username: device.username })
  }
}

/** The device when the current user may manage it (its owner or a voice admin), else a refusal: "not found" for both. */
function manageable(sipDeviceId) {
  const device = sipDeviceId ? one(DEVICE, { sipDeviceId }) : null
  if (!device || !ec.user.userId || (device.userId !== ec.user.userId && !isAdmin())) {
    return { refusal: refuse('notFound', 'Phone not found') }
  }
  return { device }
}

function view(device, now) {
  const registrations = rows(REGISTRATION, { sipDeviceId: device.sipDeviceId }).filter((r) => ms(r.expiresDate) > now)
  return {
    sipDeviceId: device.sipDeviceId, name: device.name || null, username: device.username, userId: device.userId,
    userName: userName(device.userId), deviceProfile: device.deviceProfile || 'standard',
    status: STATUS_CODES[device.statusId] || null, createdDate: ms(device.createdDate), lastSeenDate: ms(device.lastSeenDate),
    registrationList: registrations.map((r) => ({
      contactUri: r.contactUri, transport: r.transport, sourceAddress: r.sourceAddress, sourcePort: r.sourcePort == null ? null : Number(r.sourcePort),
      userAgent: r.userAgent || null, expiresDate: ms(r.expiresDate), outbound: r.outboundFlag === 'Y', instanceId: r.instanceId,
    })),
  }
}

// ---------- the user's phones ----------

function create(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in to add a phone')
  const userId = blank(p.userId) ? me : String(p.userId)
  if (userId !== me && !isAdmin()) return refuse('notAllowed', 'You may add phones only for yourself')
  const account = one('moqui.security.UserAccount', { userId })
  if (!account || account.disabled === 'Y') return refuse('notFound', 'User not found')
  if (find(DEVICE).condition('userId', userId).condition('statusId', 'SdsActive').count() >= MAX_DEVICES) {
    return refuse('tooManyDevices', 'Remove a phone before adding another')
  }
  const profile = p.deviceProfile === 'legacy' ? 'legacy' : 'standard'
  const realm = domain()
  let credential = null
  for (let i = 0; i < 5 && !credential; i++) {
    const made = makeCredential(realm, null, profile === 'legacy')
    if (made.username && !one(DEVICE, { username: made.username })) credential = made
  }
  if (!credential) return refuse('failed', 'No phone credentials could be made; try again')
  const device = ec.entity.makeValue(DEVICE)
  device.setSequencedIdPrimary()
  device.setFields({ userId, username: credential.username, realm, name: String(p.name || '').trim().slice(0, 60) || ec.l10n.localize('Phone'),
    credentialRevision: 1, deviceProfile: profile, statusId: 'SdsActive', createdDate: ec.user.nowTimestamp }, true, null, null)
  device.create()
  storeVerifiers(device, credential.verifierList, 1)
  return Object.assign({ sipDeviceId: device.sipDeviceId, username: device.username, password: credential.password }, serverSetup())
}

function rotate(p) {
  const found = manageable(p.sipDeviceId)
  if (found.refusal) return found.refusal
  const device = found.device
  if (device.statusId !== 'SdsActive') return refuse('revoked', 'This phone was removed')
  const revision = Number(device.credentialRevision || 1) + 1
  const realm = domain()
  const credential = makeCredential(realm, device.username, device.deviceProfile === 'legacy')
  if (!credential.password) return refuse('failed', 'No phone credentials could be made; try again')
  device.setFields({ credentialRevision: revision, realm }, true, null, null)
  device.update()
  storeVerifiers(device, credential.verifierList, revision)
  endRegistrations(device)
  return Object.assign({ sipDeviceId: device.sipDeviceId, username: device.username, password: credential.password }, serverSetup())
}

function revoke(p) {
  const found = manageable(p.sipDeviceId)
  if (found.refusal) return found.refusal
  const device = found.device
  if (device.statusId === 'SdsRevoked') return { sipDeviceId: device.sipDeviceId, repeated: true }
  device.setFields({ statusId: 'SdsRevoked', revokedDate: ec.user.nowTimestamp }, true, null, null)
  device.update()
  for (const v of rows(VERIFIER, { sipDeviceId: device.sipDeviceId })) v.delete()
  endRegistrations(device)
  return { sipDeviceId: device.sipDeviceId }
}

function findDevices(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in to see your phones')
  let f = find(DEVICE)
  if (yes(p.allUsers)) {
    if (!isAdmin()) return refuse('notAllowed', 'Only a voice administrator may do this')
  } else {
    const userId = blank(p.userId) ? me : String(p.userId)
    if (userId !== me && !isAdmin()) return refuse('notAllowed', 'Only a voice administrator may do this')
    f = f.condition('userId', userId)
  }
  if (!yes(p.includeRevoked)) f = f.condition('statusId', 'SdsActive')
  const now = nowMs()
  return Object.assign({ deviceList: Array.from(f.orderBy('-createdDate').limit(500).list()).map((d) => view(d, now)) }, serverSetup())
}

// ---------- the SIP engine (internal) ----------

/** The active device with this username and its verifiers for the realm; nothing for another (the engine checks the same). */
function deviceAuth(p) {
  const device = blank(p.username) ? null : one(DEVICE, { username: String(p.username) })
  if (!device || device.statusId !== 'SdsActive') return {}
  const account = one('moqui.security.UserAccount', { userId: device.userId })
  if (!account || account.disabled === 'Y') return {}
  const verifierList = []
  for (const v of rows(VERIFIER, { sipDeviceId: device.sipDeviceId, realm: String(p.realm || device.realm || '') })) {
    try {
      if (Number(v.credentialRevision) === Number(device.credentialRevision)) verifierList.push({ algorithm: v.algorithm, ha1: v.ha1 })
    } catch (err) {
      ec.logger.error(`SIP device ${device.sipDeviceId}: its ${v.algorithm} verifier does not read: ${err && err.message ? err.message : err}`)
    }
  }
  return { sipDeviceId: device.sipDeviceId, username: device.username, credentialRevision: Number(device.credentialRevision || 1), verifierList }
}

/** A binding the engine added, refreshed or removed; one of an older revision or of a removed device is not kept. */
function storeRegistration(p) {
  const row = one(REGISTRATION, { sipRegistrationId: p.sipRegistrationId })
  if (p.change === 'removed') {
    if (row) row.delete()
    return {}
  }
  const device = one(DEVICE, { sipDeviceId: p.sipDeviceId })
  if (!device || device.statusId !== 'SdsActive' || Number(p.credentialRevision || 0) !== Number(device.credentialRevision)) {
    if (row) row.delete()
    return {}
  }
  const now = ec.user.nowTimestamp
  const fields = {
    sipDeviceId: p.sipDeviceId, instanceId: p.instanceId, contactUri: String(p.contactUri || '').slice(0, 250), transport: p.transport,
    sourceAddress: p.sourceAddress, sourcePort: p.sourcePort, sipCallId: String(p.sipCallId || '').slice(0, 250), cseq: p.cseq,
    instanceKey: p.instanceKey || null, regId: p.regId ?? null, outboundFlag: yes(p.outbound) ? 'Y' : 'N',
    credentialRevision: p.credentialRevision, expiresDate: new Date(Number(p.expiresDate)),
    lastRefreshDate: new Date(Number(p.lastRefreshDate || nowMs())), userAgent: p.userAgent ? String(p.userAgent).slice(0, 250) : null,
  }
  if (row) {
    row.setFields(fields, true, null, null)
    row.update()
  } else {
    const created = ec.entity.makeValue(REGISTRATION)
    created.setFields(Object.assign({ sipRegistrationId: p.sipRegistrationId, createdDate: now }, fields), true, null, null)
    created.create()
  }
  device.set('lastSeenDate', now)
  device.update()
  return {}
}

/** The job: rows past their expiry, and those of an engine that is no longer up. Each on its own. */
function expireRegistrations() {
  const live = new Set(liveSipNodes().map((n) => n.instanceId))
  const now = new Date(nowMs())
  let count = 0
  for (const row of rows(REGISTRATION)) {
    if (ms(row.expiresDate) > now.getTime() && live.has(row.instanceId)) continue
    try {
      row.delete()
      count++
    } catch (err) {
      ec.logger.error(`SIP registration ${row.sipRegistrationId} not removed: ${err && err.message ? err.message : err}`)
    }
  }
  return { removedCount: count }
}

/**
 * Rings the devices of these users that are registered with the call's node (Calls.js ring, beside their tabs): a CltSipDevice
 * leg offered for each, which the engine forks to the device's live bindings. The first answer wins (Calls.js deviceEvent).
 * Nothing while the registrar is off, or the call's node has no SIP engine.
 */
function ringDevices(p) {
  if (!registrarOn()) return { ringCount: 0 }
  const session = p.callSessionId ? one(SESSION, { callSessionId: p.callSessionId }) : null
  if (!session || session.statusId === 'CssEnded' || !session.mediaNodeId) return { ringCount: 0 }
  if (!liveSipNodes().some((n) => n.instanceId === session.mediaNodeId)) return { ringCount: 0 }
  const now = nowMs()
  let fromUser = null
  if (p.fromUserId) fromUser = one('moqui.security.UserAccount', { userId: p.fromUserId })?.username || null
  if (!fromUser) fromUser = rows(LEG, { callSessionId: session.callSessionId, legTypeEnumId: 'CltPstn' })[0]?.phoneNumber || 'moqui'
  let ringCount = 0
  for (const userId of Array.from(p.userIdList || [])) {
    const participant = one(PARTICIPANT, { callSessionId: session.callSessionId, userId })
    if (!participant) continue
    for (const device of rows(DEVICE, { userId, statusId: 'SdsActive' })) {
      const live = find(REGISTRATION).condition('sipDeviceId', device.sipDeviceId).condition('instanceId', session.mediaNodeId)
        .condition('expiresDate', '>', new Date(now)).count() > 0
      if (!live) continue
      const leg = ec.entity.makeValue(LEG)
      leg.setSequencedIdPrimary()
      leg.setFields({ callSessionId: session.callSessionId, callParticipantId: participant.callParticipantId, legTypeEnumId: 'CltSipDevice',
        sipDeviceId: device.sipDeviceId, statusId: 'ClsOffered', offeredDate: ec.user.nowTimestamp }, true, null, null)
      leg.create()
      sipControl(session.mediaNodeId, { action: 'ringDevice', callSessionId: session.callSessionId, callLegId: leg.callLegId,
        sipDeviceId: device.sipDeviceId, username: device.username, fromUser, fromName: p.fromName || fromUser })
      ringCount++
    }
  }
  return { ringCount }
}

/**
 * A registered phone's own call (the engine, after its digest answer): placed as the phone's user, the phone being their leg.
 * toUser is another user's username (an internal call that rings their tabs and phones), else a number, which goes through
 * place#PhoneCall's checks: the user's outbound permission and caller ID, the fraud limits, and the emergency path first.
 */
function receiveDeviceCall(p) {
  if (!registrarOn()) return refuse('notAllowed', 'The registrar is off')
  const device = p.sipDeviceId ? one(DEVICE, { sipDeviceId: p.sipDeviceId }) : null
  if (!device || device.statusId !== 'SdsActive') return refuse('notAllowed', 'This phone was removed')
  const account = one('moqui.security.UserAccount', { userId: device.userId })
  if (!account || account.disabled === 'Y') return refuse('notAllowed', 'This phone was removed')
  try {
    ec.user.internalLoginUser(account.username)
  } catch (err) {
    ec.logger.warn(`SIP device ${device.sipDeviceId}: its user cannot place calls now: ${err && err.message ? err.message : err}`)
    return refuse('notAllowed', 'This phone cannot place calls now')
  }
  if (ec.user.userId !== device.userId) return refuse('notAllowed', 'This phone cannot place calls now')
  const to = String(p.toUser || '').trim()
  if (!to) return refuse('notFound', 'Nothing was dialed')
  const callee = /^[+0-9*#]+$/.test(to) ? null : find('moqui.security.UserAccount').condition('username', to).one()
  if (callee && callee.disabled !== 'Y' && callee.userId !== device.userId) {
    const out = ec.resource.script(CALLS).execute('startFromDevice', { calleeUserIdList: [callee.userId], sipDeviceId: device.sipDeviceId,
      mediaNodeId: p.instanceId }) || {}
    return out.callSessionId ? { callSessionId: out.callSessionId, callLegId: out.callLegId } : { reasonCode: out.reasonCode || 'notFound' }
  }
  const out = ec.resource.script(VOICE).execute('placeFromDevice', { phoneNumber: to, instanceId: p.instanceId }) || {}
  if (!out.callSessionId) return { reasonCode: out.reasonCode || 'notFound' }
  ec.resource.script(CALLS).execute('deviceCaller', { callLegId: out.callLegId, sipDeviceId: device.sipDeviceId })
  return { callSessionId: out.callSessionId, callLegId: out.callLegId }
}

const operations = {
  create, rotate, revoke, findDevices, deviceAuth, storeRegistration, expireRegistrations, ringDevices, receiveDeviceCall,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown SIP device operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
