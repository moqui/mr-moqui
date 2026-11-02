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

/**
 * The live SIP node of this node's edge node (plans/Edge-Node.md §5.4 "Voice (SIP)", E8), whose row the link keeps from
 * its Hello with its realm and registrar switch; null when there is none. Its registrar is the phones'.
 */
function edgeSipNode() {
  return rows(SIP_NODE).find((n) => n.edgeLinked === 'Y' && ms(n.heartbeatDate) >= nowMs() - NODE_STALE_MS) || null
}

/**
 * Whether the registrar is on: sip_registrar_enabled (off by default, even with the SIP listeners on); behind an edge
 * node, the edge node's (its row), whose registrar the phones use.
 */
function registrarOn() {
  const edge = edgeSipNode()
  if (edge) return edge.registrarEnabled === 'Y'
  return yes(property('sip_registrar_enabled').toLowerCase())
}

/**
 * The realm of the verifiers and the domain phones register to, as the SIP engine takes it (moqui-host sip settings);
 * behind an edge node, the realm its registrar challenges with (its row's sipDomain).
 */
function domain() {
  const edge = edgeSipNode()
  if (edge && !blank(edge.sipDomain)) return edge.sipDomain
  return property('sip_domain') || property('sip_public_address') || 'moqui'
}

function liveSipNodes() {
  return rows(SIP_NODE).filter((n) => ms(n.heartbeatDate) >= nowMs() - NODE_STALE_MS)
}

/** Cascading media nodes (W7, Calls.js cascadeOn): a phone may ring on another node than the call's. */
const cascade = () => ['true', 'y', 'yes', '1'].includes(property('rtc_cascade_enabled').toLowerCase())

/** A live media node that takes calls (CallMediaNode), for a phone's room there to relay to the call's node. */
function liveMediaNode(instanceId) {
  const node = one('moqui.call.CallMediaNode', { instanceId })
  return !!node && node.draining !== 'Y' && ms(node.heartbeatDate) >= nowMs() - NODE_STALE_MS
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

// ---------- the registrar of this node's edge node (plans/Edge-Node.md §5.4 "Voice (SIP)", decision 4, E8) ----------

/** How long after a digest answer checked here the edge node may store the phone's binding or place its call. */
const EDGE_DIGEST_MS = 60000

const isEdgeCaller = () => ec.user.username === '_edge_'

/** Whether the caller may act as SIP node instanceId: anyone, or the edge node's delegate user as its own node. */
function edgeNodeAllowed(instanceId) {
  if (!isEdgeCaller()) return true
  const node = blank(instanceId) ? null : one(SIP_NODE, { instanceId })
  return !!node && node.edgeLinked === 'Y'
}

/** Whether the edge node's delegate user had a digest answer of this device checked here just now (or any other caller). */
function edgeDigestFresh(device) {
  return !isEdgeCaller() || (ms(device.edgeDigestDate) || 0) >= nowMs() - EDGE_DIGEST_MS
}

/**
 * A phone's digest answer to the edge node's registrar, checked against its verifier here, so the verifiers never leave
 * this node: matched, with the device, when the response is right; matched false and nothing else when not.
 */
function checkDigest(p) {
  // the edge node's (E8): only for the realm its registrar challenges with, which its phones' verifiers are for
  if (isEdgeCaller() && String(p.realm || '') !== domain()) return { matched: false }
  const auth = deviceAuth({ username: p.username, realm: p.realm })
  const verifier = (auth.verifierList || []).find((v) => String(v.algorithm).toUpperCase() === String(p.algorithm || '').toUpperCase())
  let expected = ''
  if (verifier) {
    expected = ec.service.sync().name('org.moqui.impl.SipDeviceServices.compute#SipDigestResponse').parameters({
      algorithm: verifier.algorithm, ha1: verifier.ha1, nonce: p.nonce, nc: p.nc || null, cnonce: p.cnonce || null,
      qop: p.qop || null, ha2: p.ha2 }).disableAuthz().call()?.response || ''
  }
  if (!expected || expected !== String(p.response || '').toLowerCase()) return { matched: false }
  if (isEdgeCaller()) {
    // an answer the edge node kept, sent again (an old nonce, or a count taken before), signs nobody in
    const fresh = ec.service.sync().name('org.moqui.impl.SipDeviceServices.claim#SipDigestNonce')
      .parameters({ sipDeviceId: auth.sipDeviceId, nonce: p.nonce, nc: p.nc || null }).disableAuthz().call()?.fresh
    if (!fresh) {
      ec.logger.warn(`SIP device ${auth.sipDeviceId}: the edge node sent an answer with an old or used nonce, refused`)
      return { matched: false }
    }
    const device = find(DEVICE).condition('sipDeviceId', auth.sipDeviceId).forUpdate(true).one()
    if (device) {
      device.set('edgeDigestDate', ec.user.nowTimestamp)
      device.update()
    }
  }
  return { matched: true, sipDeviceId: auth.sipDeviceId, username: auth.username, credentialRevision: auth.credentialRevision }
}

/** A binding the engine added, refreshed or removed; one of an older revision or of a removed device is not kept. */
function storeRegistration(p) {
  if (!edgeNodeAllowed(p.instanceId)) return refuse('notAllowed', 'The edge node keeps the bindings of its own SIP node only')
  const row = one(REGISTRATION, { sipRegistrationId: p.sipRegistrationId })
  if (row && isEdgeCaller() && row.instanceId !== p.instanceId) return refuse('notAllowed', 'Not a binding of the edge node')
  if (p.change === 'removed') {
    if (row) row.delete()
    return {}
  }
  const device = one(DEVICE, { sipDeviceId: p.sipDeviceId })
  if (device && !edgeDigestFresh(device)) return refuse('notAllowed', 'The edge node had no digest answer of this phone checked')
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
 * Nothing while the registrar is off, or the call's node has no SIP engine. With rtc_cascade_enabled (W7) a phone
 * registered with another node rings there too: its leg's media is on that node (CallLeg.mediaNodeId), whose room is
 * linked to the call's node first.
 */
function ringDevices(p) {
  const session = p.callSessionId ? one(SESSION, { callSessionId: p.callSessionId }) : null
  // an end-to-end encrypted call (W8) rings no phone: it could not decrypt the call's media
  if (session && session.encryptionMode && session.encryptionMode !== 'dtls-srtp') return { ringCount: 0, reasonCode: 'encrypted' }
  if (!registrarOn()) return { ringCount: 0 }
  if (!session || session.statusId === 'CssEnded' || !session.mediaNodeId) return { ringCount: 0 }
  // the nodes a phone may ring on: the call's, and with cascading media nodes (W7) any live one with a SIP engine and
  // media, whose room relays to the call's node
  const sipNodes = new Set(liveSipNodes().map((n) => n.instanceId))
  const ringNodes = cascade() ? new Set(Array.from(sipNodes).filter((id) => id === session.mediaNodeId || liveMediaNode(id)))
    : new Set(sipNodes.has(session.mediaNodeId) ? [session.mediaNodeId] : [])
  if (!ringNodes.size) return { ringCount: 0 }
  const now = nowMs()
  let fromUser = null
  if (p.fromUserId) fromUser = one('moqui.security.UserAccount', { userId: p.fromUserId })?.username || null
  if (!fromUser) fromUser = rows(LEG, { callSessionId: session.callSessionId, legTypeEnumId: 'CltPstn' })[0]?.phoneNumber || 'moqui'
  // a phone rings as long as the call may ring: until the session's ring expiry (a voicemail box's rings, V2, can make it
  // longer than call_ring_seconds); voicemail or an answer cancels it sooner
  const ringSeconds = Math.max(1, Math.ceil(((ms(session.ringExpireDate) || now) - now) / 1000))
  let ringCount = 0
  for (const userId of Array.from(p.userIdList || [])) {
    const participant = one(PARTICIPANT, { callSessionId: session.callSessionId, userId })
    if (!participant) continue
    for (const device of rows(DEVICE, { userId, statusId: 'SdsActive' })) {
      // the node the phone is registered with: the call's when it is there too
      const nodes = Array.from(find(REGISTRATION).condition('sipDeviceId', device.sipDeviceId)
        .condition('expiresDate', '>', new Date(now)).list()).map((r) => r.instanceId).filter((id) => ringNodes.has(id))
      if (!nodes.length) continue
      const node = nodes.includes(session.mediaNodeId) ? session.mediaNodeId : nodes[0]
      const leg = ec.entity.makeValue(LEG)
      leg.setSequencedIdPrimary()
      leg.setFields({ callSessionId: session.callSessionId, callParticipantId: participant.callParticipantId, legTypeEnumId: 'CltSipDevice',
        sipDeviceId: device.sipDeviceId, statusId: 'ClsOffered', offeredDate: ec.user.nowTimestamp,
        mediaNodeId: node === session.mediaNodeId ? null : node }, true, null, null)
      leg.create()
      if (node !== session.mediaNodeId) {
        ec.service.sync().name('org.moqui.impl.CallServices.send#MediaControl').parameters({ mediaNodeId: node,
          message: { action: 'link', callSessionId: session.callSessionId, originNodeId: session.mediaNodeId } }).disableAuthz().call()
      }
      sipControl(node, { action: 'ringDevice', callSessionId: session.callSessionId, callLegId: leg.callLegId,
        sipDeviceId: device.sipDeviceId, username: device.username, fromUser, fromName: p.fromName || fromUser, ringSeconds })
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
  // the edge node's engine (E8): only on its own SIP node, for a phone whose answer to its challenge was checked here
  if (!edgeNodeAllowed(p.instanceId) || !edgeDigestFresh(device)) {
    return refuse('notAllowed', 'The edge node had no digest answer of this phone checked')
  }
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
  create, rotate, revoke, findDevices, deviceAuth, checkDigest, storeRegistration, expireRegistrations, ringDevices, receiveDeviceCall,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown SIP device operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
