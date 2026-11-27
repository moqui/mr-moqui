// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Location messages (plans/Comms-Messaging.md §18.3 phase 20): the services of org.moqui.chat.ChatLocationServices. A
// location is a message (CetMessage) with content.location {latitude, longitude, label, address, accuracy, source}: the
// coordinates or an address or both. There is no map, no geocoding and no outside request. Its body is its words with an
// OpenStreetMap link (fallbackText), so every client, bridge and text shows it; Sinch RCS sends Sinch's location message
// (crates/moqui-channels, providers/sinch/rcs.rs). An inbound RCS location (Sms.js) is made with the same words. In an
// encrypted room the browser makes the same words (moqui.chat.locationFallback in WebrootChat.js; the specs keep the two
// in step) and the place is only in the ciphertext. A live location (plans/Location-Sharing.md §3.5) is below: a
// LocationServices share with the room's members and a message carrying its id, never a position.

const EVENT = 'moqui.chat.ChatEvent'
const LIMITS = { labelMax: 100, addressMax: 300, accuracyMax: 100000 }
const SOURCES = ['here', 'typed', 'network']

const blank = (value) => value == null || String(value).trim() === ''
const words = (value) => String(value ?? '').replace(/[\u0000-\u001f\u007f]+/g, ' ').replace(/\s+/g, ' ').trim()
const localize = (text) => ec.l10n.localize(text)
const expand = (text, values) => ec.resource.expand(localize(text), '', values)
const escape = (text) => String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
const parse = (text) => { if (!text) return {}; try { const value = JSON.parse(text); return value && typeof value === 'object' ? value : {} } catch (err) { return {} } }
// a number as a JSON number or text comes in, or a decimal (a service call's JSON map gives decimals)
const num = (value) => (value != null && typeof value === 'object' && typeof value.toNumber === 'function' ? value.toNumber() : Number(value))
const fixed = (number) => num(number).toFixed(6)

function refuse(reasonCode, text) {
  ec.message.addError(text)
  return { reasonCode }
}

/**
 * A place as given, checked: coordinates (both, finite, in range, rounded to 6 places) or an address or both, the words
 * cut to their limits. Out: {location} or {error}.
 */
function clean(given) {
  const it = given && typeof given === 'object' ? given : {}
  const out = {}
  const hasLat = !blank(it.latitude)
  const hasLon = !blank(it.longitude)
  if (hasLat || hasLon) {
    const latitude = num(it.latitude)
    const longitude = num(it.longitude)
    if (!hasLat || !hasLon || !Number.isFinite(latitude) || !Number.isFinite(longitude) || Math.abs(latitude) > 90 || Math.abs(longitude) > 180) {
      return { error: localize('A place is a latitude from -90 to 90 and a longitude from -180 to 180') }
    }
    out.latitude = Number(fixed(latitude))
    out.longitude = Number(fixed(longitude))
  }
  const label = words(it.label).slice(0, LIMITS.labelMax)
  const address = words(it.address).slice(0, LIMITS.addressMax)
  if (label) out.label = label
  if (address) out.address = address
  if (out.latitude == null && !address) return { error: localize('A place needs its coordinates or an address') }
  if (!blank(it.accuracy)) {
    const accuracy = num(it.accuracy)
    if (Number.isFinite(accuracy) && accuracy >= 0) out.accuracy = Math.min(Math.round(accuracy), LIMITS.accuracyMax)
  }
  out.source = SOURCES.includes(it.source) ? it.source : 'typed'
  return { location: out }
}

/** The map link: OpenStreetMap at the coordinates, or OpenStreetMap's search for the address (the person runs it). */
function linkOf(place) {
  if (place.latitude != null && place.longitude != null) {
    const lat = fixed(place.latitude)
    const lon = fixed(place.longitude)
    return `https://www.openstreetmap.org/?mlat=${lat}&mlon=${lon}#map=17/${lat}/${lon}`
  }
  return place.address ? 'https://www.openstreetmap.org/search?query=' + encodeURIComponent(place.address) : ''
}

/** The words of a place: "Location: Label, Address" and the map link, a line each. */
function fallbackText(place) {
  const name = [place.label, place.address].filter((word) => !blank(word)).join(', ')
  const coordinates = place.latitude != null ? fixed(place.latitude) + ', ' + fixed(place.longitude) : ''
  return [expand('Location: ${name}', { name: name || coordinates || localize('a place') }), linkOf(place)].filter((line) => line).join('\n')
}

function fallbackHtml(place) {
  return '<p>' + fallbackText(place).split('\n').map(escape).join('<br>') + '</p>'
}

/**
 * words#Location: the words of a place (for Sms.js and the specs). Out: text, location (clean). A place that is not one
 * (coordinates out of range, a label alone) keeps its words with no link, as phase 10's did: "Location: label", and
 * location null; no error.
 */
function wordsOf(p) {
  const checked = clean(p.location)
  if (!checked.error) return { text: fallbackText(checked.location), location: checked.location }
  const it = p.location && typeof p.location === 'object' ? p.location : {}
  const name = [words(it.label).slice(0, LIMITS.labelMax), words(it.address).slice(0, LIMITS.addressMax)].filter((word) => word).join(', ')
  return { text: expand('Location: ${name}', { name: name || localize('a place') }), location: null }
}

/**
 * send#Location: a location from the current user in a plain room: send#Message with its words, then content.location
 * and the update pushed. Refused in an encrypted room (the browser sends it there in the ciphertext).
 */
function send(p) {
  const out = { eventSeq: null, event: null, duplicate: false, reasonCode: null }
  if (ec.entity.find('moqui.chat.ChatRoom').condition('chatRoomId', p.chatRoomId).condition('encrypted', 'Y').disableAuthz().count()) {
    return Object.assign(out, refuse('encryptedRoom', localize('In an encrypted room your device sends the place, encrypted')))
  }
  const checked = clean(p.location)
  if (checked.error) return Object.assign(out, refuse('badLocation', checked.error))
  const sent = ec.service.sync().name('org.moqui.chat.ChatServices.send#Message').parameters({ chatRoomId: p.chatRoomId, body: fallbackHtml(checked.location),
    replyToEventSeq: p.replyToEventSeq ?? null, threadRootEventSeq: p.threadRootEventSeq ?? null, clientRef: p.clientRef || null,
    roomUrl: p.roomUrl || null }).call()
  if (ec.message.hasError()) return Object.assign(out, { reasonCode: (sent && sent.reasonCode) || null })
  out.eventSeq = sent.eventSeq
  out.event = sent.event
  if (sent.duplicate) return Object.assign(out, { duplicate: true })
  setOn(p.chatRoomId, sent.eventSeq, checked.location)
  out.event = Object.assign({}, sent.event, { location: checked.location })
  return out
}

/** content.location set on a message and pushed as an update (also an inbound one, Sms.js, through ingest#Event). */
function setOn(chatRoomId, eventSeq, location) {
  const row = ec.entity.find(EVENT).condition('chatRoomId', chatRoomId).condition('eventSeq', eventSeq).disableAuthz().one()
  if (!row) return
  const content = parse(row.contentJson)
  content.location = location
  row.set('contentJson', JSON.stringify(content))
  row.update()
  ec.service.sync().name('org.moqui.chat.ChatServices.push#Update').parameters({ chatRoomId, eventSeq: Number(eventSeq), fields: { location } }).disableAuthz().call()
}

/** mark#Location (internal): a received message is a location (an RCS one): content.location, checked. */
function mark(p) {
  const checked = clean(Object.assign({}, p.location, { source: 'network' }))
  if (checked.error) { ec.logger.warn(`An inbound location of message ${p.eventSeq} of room ${p.chatRoomId} was not kept: ${checked.error}`); return { marked: false } }
  setOn(p.chatRoomId, p.eventSeq, checked.location)
  return { marked: true }
}

// ---- live location in chat (plans/Location-Sharing.md §3.5, LL4) ----
// A live share whose audience is the room's members as users when it starts (LocationServices.create#Share, which applies
// the chat directory's rule and blocks again), and a message with content.location {liveShareId, label, thruDate,
// approximate, source: 'live'} that carries the share's id and never a position: its words, which every bridge and text
// gets, say whose live location it is, until when, and that live updates are in the app. The card reads the share
// through get#Share, which checks its current audience. Offered only in a plain room: in an encrypted room the server
// would see each position, and the phone's location service (Rust, no page) holds no room keys to seal them (the decision
// recorded in docs/tools/Comms.md, "Live location").
const LIVE = 'org.moqui.maps.LocationServices.'

/** The room's members a live share starts with: its local users in it now but the caller, no agent, none who put it out of sight. */
function liveAudience(chatRoomId, me) {
  const ids = []
  for (const row of ec.entity.find('moqui.chat.ChatRoomMember').condition('chatRoomId', chatRoomId).condition('leftDate', null)
    .selectField('userId,roleEnumId,filterEnumId').disableAuthz().list()) {
    if (row.userId && row.userId !== me && row.roleEnumId !== 'CmrAgent' && !row.filterEnumId && !ids.includes(row.userId)) ids.push(row.userId)
  }
  if (!ids.length) return ids
  // an account that cannot sign in is left out, as are the people the chat directory keeps from the caller and those who
  // blocked them (create#Share would refuse the whole share for one of them)
  const now = ec.user.timestampMillis(ec.user.nowTimestamp)
  const open = new Set(Array.from(ec.entity.find('moqui.security.UserAccount').condition('userId', 'in', ids)
    .selectField('userId,disabled,terminateDate').disableAuthz().list())
    .filter((row) => row.disabled !== 'Y' && !(row.terminateDate != null && ec.user.timestampMillis(row.terminateDate) <= now)).map((row) => row.userId))
  const refused = ec.service.sync().name('org.moqui.chat.ChatServices.get#RefusedUsers')
    .parameters({ userIdList: ids, blocks: true, answerWithAuthzOff: true }).call() || {}
  if (ec.message.hasError()) return null
  const out = new Set(Array.from(refused.refusedUserIdList || []).map(String))
  return ids.filter((id) => open.has(id) && !out.has(id))
}

/** "15:30", or "Fri 09:00" when the end is not today, in the sender's zone. */
function untilText(thru) {
  const day = (value) => ec.l10n.format(new Date(value), 'yyyy-MM-dd')
  const now = ec.user.timestampMillis(ec.user.nowTimestamp)
  return ec.l10n.format(new Date(thru), day(thru) === day(now) ? 'HH:mm' : 'EEE HH:mm')
}

/** The words of a live location message: whose, until when, and that live updates are in the app (with its link). No position. */
function liveText(label, thru, roomUrl) {
  let link = ''
  if (roomUrl) {
    try { link = ec.web && typeof ec.web.getWebappRootUrl === 'function' ? String(ec.web.getWebappRootUrl(true, null) || '') + roomUrl : '' } catch (err) { link = '' }
    if (!/^https?:\/\//.test(link)) link = ''
  }
  return [expand('Live location: ${name}, until ${until}', { name: label, until: untilText(thru) }),
    link ? expand('Live updates are in the app: ${link}', { link }) : localize('Live updates are in the app')].join('\n')
}

/** A whole-number setting of the location conf element, as LocationShares.js reads it. */
function locationSetting(name, fallback) {
  const value = String(ec.factory.getConfValue('location@' + name) ?? '').trim()
  const number = Number(value)
  return value !== '' && Number.isInteger(number) && number >= 0 ? number : fallback
}

/**
 * send#LiveLocation: a live share with the room's members, and its message. The message first (so a send again by its
 * clientRef starts no second share), then the share, then the message's content.location; a refusal of either rolls both
 * back.
 */
function sendLive(p) {
  const out = { eventSeq: null, event: null, shareId: null, thruDate: null, intervalSeconds: null, duplicate: false, reasonCode: null }
  const room = ec.entity.find('moqui.chat.ChatRoom').condition('chatRoomId', p.chatRoomId).selectField('chatRoomId,encrypted,roomName').disableAuthz().one()
  if (room && room.encrypted === 'Y') {
    return Object.assign(out, refuse('encryptedRoom', localize('Live location is not offered in an encrypted room: the server would see where you are. Send a location instead')))
  }
  const me = ec.user.userId
  const member = !!room && !!me && ec.entity.find('moqui.chat.ChatRoomMember').condition('chatRoomId', p.chatRoomId).condition('userId', me)
    .condition('leftDate', null).disableAuthz().count() > 0
  if (!member) return Object.assign(out, refuse('notMember', localize('Join the room to share your live location there')))
  const audience = liveAudience(p.chatRoomId, me)
  if (audience == null) return Object.assign(out, { reasonCode: 'audienceCheckFailed' })
  if (!audience.length) return Object.assign(out, refuse('noAudience', localize('Nobody else in this room can see your live location')))
  const thru = p.thruDate == null ? null : ec.user.timestampMillis(p.thruDate)
  if (thru == null) return Object.assign(out, refuse('badUntil', localize('A live location ends after now')))
  const owner = ec.entity.find('moqui.security.UserAccount').condition('userId', me).selectField('userFullName,username').disableAuthz().one()
  const label = words(p.label).slice(0, LIMITS.labelMax) || (owner && (owner.userFullName || owner.username)) || me
  const sent = ec.service.sync().name('org.moqui.chat.ChatServices.send#Message').parameters({ chatRoomId: p.chatRoomId,
    body: '<p>' + liveText(label, thru, p.roomUrl).split('\n').map(escape).join('<br>') + '</p>',
    replyToEventSeq: p.replyToEventSeq ?? null, threadRootEventSeq: p.threadRootEventSeq ?? null, clientRef: p.clientRef || null,
    roomUrl: p.roomUrl || null }).call()
  if (ec.message.hasError()) return Object.assign(out, { reasonCode: (sent && sent.reasonCode) || null })
  out.eventSeq = sent.eventSeq
  out.event = sent.event
  if (sent.duplicate) return Object.assign(out, { duplicate: true })
  const approximate = p.precisionEnumId === 'LspApproximate'
  const share = ec.service.sync().name(LIVE + 'create#Share').parameters({
    audienceList: audience.map((userId) => ({ audienceTypeEnumId: 'LsaUser', userId })), thruDate: p.thruDate,
    precisionEnumId: approximate ? 'LspApproximate' : 'LspExact', keepTrail: false, label,
    purposeText: room.roomName ? expand('In chat: ${name}', { name: words(room.roomName).slice(0, 100) }) : localize('In a chat')
  }).call() || {}
  if (ec.message.hasError()) return Object.assign(out, { eventSeq: null, event: null, reasonCode: share.reasonCode || null })
  const location = { liveShareId: share.shareId, label, thruDate: ec.user.timestampMillis(share.thruDate), approximate, source: 'live' }
  setOn(p.chatRoomId, sent.eventSeq, location)
  // the interval a page sends at (LocationShares.js report's), so its reports are not refused as too soon
  const minimum = locationSetting('min-interval-seconds', 5)
  return Object.assign(out, { event: Object.assign({}, sent.event, { location }), shareId: share.shareId, thruDate: location.thruDate,
    intervalSeconds: approximate ? Math.max(minimum, locationSetting('approximate-interval-seconds', 60)) : minimum })
}

/**
 * get#LiveLocation: a live location message's share as the caller may see it. The message is read by get#Timeline, so
 * only one who reads the room gets anything; the share through get#Share, which checks its current audience (a member
 * who joined later, or one the owner took out, is refused there) and is the only read of a position. state: live (with
 * latest, or none yet), ended, or notShared (the share is on and the caller is not in its audience).
 */
function liveView(p) {
  const out = { state: null, owner: false, share: null, latest: null, reasonCode: null }
  const timeline = ec.service.sync().name('org.moqui.chat.ChatServices.get#Timeline').parameters({ chatRoomId: p.chatRoomId, eventSeq: p.eventSeq }).call() || {}
  if (ec.message.hasError()) return Object.assign(out, { reasonCode: 'notFound' })
  const event = Array.from(timeline.eventList || []).find((one) => Number(one.eventSeq) === Number(p.eventSeq))
  const place = event && !event.redacted ? event.location : null
  const shareId = place && place.liveShareId ? String(place.liveShareId) : ''
  if (!shareId || !event.senderUserId) return Object.assign(out, refuse('notFound', localize('That is not a live location')))
  const me = ec.user.userId
  let got = null
  // in a transaction of its own: its refusal (not in the audience) would mark this one rollback-only
  ec.message.pushErrors()
  try {
    got = ec.service.sync().name(LIVE + 'get#Share').parameters({ shareId, trailPageSize: 1 }).requireNewTransaction(true).call() || {}
    if (ec.message.hasError()) got = null
  } finally { ec.message.clearErrors(); ec.message.popErrors() }
  const share = got && got.share
  // the share of the message's author only: a share id in another person's message reads as none
  if (share && share.ownerUserId === event.senderUserId) {
    const owner = share.ownerUserId === me
    const active = owner ? !!share.active : true
    const view = { shareId, label: share.label || place.label || null, ownerName: share.ownerName || null, thruDate: share.thruDate, approximate: !!share.approximate }
    if (owner) Object.assign(view, { audienceVersion: share.audienceVersion, sentFromDevice: !!share.sentFromDevice, deviceName: share.deviceName || null,
      fromThisDevice: !!share.fromThisDevice, audienceCount: Array.from(share.audienceList || []).filter((row) => !row.revoked).length })
    return Object.assign(out, { state: active ? 'live' : 'ended', owner, share: view, latest: active ? got.latest || null : null })
  }
  const status = ec.service.sync().name(LIVE + 'get#ShareStatus').parameters({ shareId, ownerUserId: event.senderUserId }).call() || {}
  if (ec.message.hasError()) return Object.assign(out, { reasonCode: 'notFound' })
  return Object.assign(out, { state: status.active ? 'notShared' : 'ended', share: { shareId, label: place.label || null, thruDate: place.thruDate || null } })
}

const operations = { send, words: wordsOf, mark, live: sendLive, liveView }

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown location operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute, fallbackText, clean })
