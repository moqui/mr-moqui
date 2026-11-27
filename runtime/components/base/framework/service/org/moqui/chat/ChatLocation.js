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
// gets, say whose live location it is, for how long, and that live updates are in the app. The card reads the share
// through get#Share, which checks its current audience. Offered only in a plain room: in an encrypted room the server
// would see each position, and the phone's location service (Rust, no page) holds no room keys to seal them (the decision
// recorded in docs/tools/Comms.md, "Live location").
const LIVE = 'org.moqui.maps.LocationServices.'

const BLOCKS = 'component://framework/service/org/moqui/chat/ChatBlocks.js'

/**
 * The room's members a live share starts with: its local users in it now but the caller, no agent, none who put the room
 * out of sight, no account that cannot sign in, none the caller blocked, and none the chat directory keeps from the caller
 * or who blocked the caller (create#Share would refuse the whole share for one of those). Null when the check failed.
 */
function liveAudience(chatRoomId, me) {
  const ids = []
  for (const row of ec.entity.find('moqui.chat.ChatRoomMember').condition('chatRoomId', chatRoomId).condition('leftDate', null)
    .selectField('userId,roleEnumId,filterEnumId').disableAuthz().list()) {
    if (row.userId && row.userId !== me && row.roleEnumId !== 'CmrAgent' && !row.filterEnumId && !ids.includes(row.userId)) ids.push(row.userId)
  }
  if (!ids.length) return ids
  const now = ec.user.timestampMillis(ec.user.nowTimestamp)
  const open = new Set(Array.from(ec.entity.find('moqui.security.UserAccount').condition('userId', 'in', ids)
    .selectField('userId,disabled,terminateDate').disableAuthz().list())
    .filter((row) => row.disabled !== 'Y' && !(row.terminateDate != null && ec.user.timestampMillis(row.terminateDate) <= now)).map((row) => row.userId))
  const blocked = new Set(Array.from(ec.resource.script(BLOCKS).execute('blockedBy', { userId: me }).userIdList || []).map(String))
  const refused = ec.service.sync().name('org.moqui.chat.ChatServices.get#RefusedUsers')
    .parameters({ userIdList: ids, blocks: true, answerWithAuthzOff: true }).call() || {}
  if (ec.message.hasError()) return null
  const out = new Set(Array.from(refused.refusedUserIdList || []).map(String))
  return ids.filter((id) => open.has(id) && !blocked.has(id) && !out.has(id))
}

/** How long a share lasts from now, in words that need no time zone: "15 min", "1 h", "2 h 30 min". */
function durationText(thru) {
  const minutes = Math.max(1, Math.round((thru - ec.user.timestampMillis(ec.user.nowTimestamp)) / 60000))
  const hours = Math.floor(minutes / 60)
  const rest = minutes % 60
  if (!hours) return minutes + ' ' + localize('min')
  return hours + ' ' + localize('h') + (rest ? ' ' + rest + ' ' + localize('min') : '')
}

/**
 * The words of a live location message: whose, for how long (a time from now, so a bridge's reader or a member in another
 * zone reads it right), and that live updates are in the app (with its link). No position.
 */
function liveText(name, thru, roomUrl) {
  let link = ''
  if (roomUrl) {
    try { link = ec.web && typeof ec.web.getWebappRootUrl === 'function' ? String(ec.web.getWebappRootUrl(true, null) || '') + roomUrl : '' } catch (err) { link = '' }
    if (!/^https?:\/\//.test(link)) link = ''
  }
  return [expand('Live location: ${name}, for ${duration}', { name, duration: durationText(thru) }),
    link ? expand('Live updates are in the app: ${link}', { link }) : localize('Live updates are in the app')].join('\n')
}

/** The report interval of a share (LocationShares.js report's), so a page's reports are not refused as too soon. */
function intervalOf(approximate) {
  const minimum = locationSetting('min-interval-seconds', 5)
  return approximate ? Math.max(minimum, locationSetting('approximate-interval-seconds', 60)) : minimum
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
  const me = ec.user.userId
  // a member first: one who is not learns nothing of the room (not even that it is encrypted)
  const member = !!me && ec.entity.find('moqui.chat.ChatRoomMember').condition('chatRoomId', p.chatRoomId).condition('userId', me)
    .condition('leftDate', null).disableAuthz().count() > 0
  const room = member ? ec.entity.find('moqui.chat.ChatRoom').condition('chatRoomId', p.chatRoomId).selectField('chatRoomId,encrypted,roomName').disableAuthz().one() : null
  if (!room) return Object.assign(out, refuse('notMember', localize('Join the room to share your live location there')))
  if (room.encrypted === 'Y') {
    return Object.assign(out, refuse('encryptedRoom', localize('Live location is not offered in an encrypted room: the server would see where you are. Send a location instead')))
  }
  const audience = liveAudience(p.chatRoomId, me)
  if (audience == null) return Object.assign(out, { reasonCode: 'audienceCheckFailed' })
  if (!audience.length) return Object.assign(out, refuse('noAudience', localize('Nobody else in this room can see your live location')))
  const thru = p.thruDate == null ? null : ec.user.timestampMillis(p.thruDate)
  if (thru == null) return Object.assign(out, refuse('badUntil', localize('A live location ends after now')))
  // the person's name as people read it, never the username (which may be an email address the bridges would carry)
  const owner = ec.entity.find('moqui.security.UserAccount').condition('userId', me).selectField('userFullName').disableAuthz().one()
  const given = words(p.label).slice(0, LIMITS.labelMax)
  const name = given || words(owner && owner.userFullName).slice(0, LIMITS.labelMax) || localize('A member of the room')
  const sent = ec.service.sync().name('org.moqui.chat.ChatServices.send#Message').parameters({ chatRoomId: p.chatRoomId,
    body: '<p>' + liveText(name, thru, p.roomUrl).split('\n').map(escape).join('<br>') + '</p>',
    replyToEventSeq: p.replyToEventSeq ?? null, threadRootEventSeq: p.threadRootEventSeq ?? null, clientRef: p.clientRef || null,
    roomUrl: p.roomUrl || null }).call()
  if (ec.message.hasError()) return Object.assign(out, { reasonCode: (sent && sent.reasonCode) || null })
  out.eventSeq = sent.eventSeq
  out.event = sent.event
  if (sent.duplicate) {
    // the first send's share, so a client whose answer was lost starts sending it
    const first = ec.entity.find(EVENT).condition('chatRoomId', p.chatRoomId).condition('eventSeq', sent.eventSeq).disableAuthz().one()
    const place = first ? parse(first.contentJson).location : null
    if (place && place.liveShareId) Object.assign(out, { event: Object.assign({}, sent.event, { location: place }), shareId: place.liveShareId,
      thruDate: place.thruDate ?? null, intervalSeconds: intervalOf(place.approximate === true) })
    return Object.assign(out, { duplicate: true })
  }
  const approximate = p.precisionEnumId === 'LspApproximate'
  const share = ec.service.sync().name(LIVE + 'create#Share').parameters({
    audienceList: audience.map((userId) => ({ audienceTypeEnumId: 'LsaUser', userId })), thruDate: p.thruDate,
    precisionEnumId: approximate ? 'LspApproximate' : 'LspExact', keepTrail: false, label: name,
    purposeText: room.roomName ? expand('In chat: ${name}', { name: words(room.roomName).slice(0, 100) }) : localize('In a chat')
  }).call() || {}
  if (ec.message.hasError()) return Object.assign(out, { eventSeq: null, event: null, reasonCode: share.reasonCode || null })
  const location = { liveShareId: share.shareId, label: name, thruDate: ec.user.timestampMillis(share.thruDate), approximate, source: 'live' }
  setOn(p.chatRoomId, sent.eventSeq, location)
  // the room and message of the share, for a member taken out or blocked later (revokeLive)
  ec.entity.makeValue('moqui.chat.ChatLiveShare').setAll({ chatRoomId: p.chatRoomId, eventSeq: sent.eventSeq, shareId: share.shareId, ownerUserId: me,
    createdDate: ec.user.nowTimestamp }).create()
  return Object.assign(out, { event: Object.assign({}, sent.event, { location }), shareId: share.shareId, thruDate: location.thruDate,
    intervalSeconds: intervalOf(approximate) })
}

/**
 * revokeLive (internal; plans/Location-Sharing.md §3.5, the LL4 decision): userId taken out of the live shares of a room
 * (chatRoomId: a member taken out of it by another, remove#Member) or of an owner's live shares in every room
 * (ownerUserId: the owner blocked them, ChatBlocks.js blockUser), through LocationServices.revoke#ChatAudience in a
 * transaction of its own. It only narrows who sees a share, so it never fails its caller: a failure is logged and cleared.
 * A member who leaves by themselves keeps the share until its owner takes them out.
 */
function revokeLive(p) {
  const userId = words(p.userId)
  if (!userId || (blank(p.chatRoomId) && blank(p.ownerUserId))) return { revokedCount: 0 }
  ec.message.pushErrors()
  try {
    let rows = ec.entity.find('moqui.chat.ChatLiveShare').disableAuthz()
    if (!blank(p.chatRoomId)) rows = rows.condition('chatRoomId', p.chatRoomId)
    if (!blank(p.ownerUserId)) rows = rows.condition('ownerUserId', p.ownerUserId)
    const linked = Array.from(rows.selectField('shareId,ownerUserId').list()).filter((row) => row.shareId && row.ownerUserId)
    if (!linked.length) return { revokedCount: 0 }
    // only the shares still on (an ended one's row is deleted with it; one past its end waits for the hourly clean)
    const now = ec.user.nowTimestamp
    const active = new Set(Array.from(ec.entity.find('moqui.maps.LocationShare').condition('shareId', 'in', linked.map((row) => row.shareId))
      .condition('endedDate', null).condition('revokedDate', null).condition('thruDate', 'greater', now).selectField('shareId').disableAuthz().list())
      .map((row) => row.shareId))
    const shareList = linked.filter((row) => active.has(row.shareId)).map((row) => ({ shareId: row.shareId, ownerUserId: row.ownerUserId }))
    if (!shareList.length) return { revokedCount: 0 }
    const done = ec.service.sync().name(LIVE + 'revoke#ChatAudience').parameters({ shareList, userId }).disableAuthz().call() || {}
    if (ec.message.hasError()) ec.logger.error(`A person was not taken out of the live locations of ${p.chatRoomId || 'the rooms of ' + p.ownerUserId}: ${ec.message.getErrorsString()}`)
    return { revokedCount: Number(done.revokedCount) || 0 }
  } catch (err) {
    ec.logger.error(`A person was not taken out of the live locations of ${p.chatRoomId || 'the rooms of ' + p.ownerUserId}: ${err && err.message ? err.message : err}`)
    return { revokedCount: 0 }
  } finally { ec.message.clearErrors(); ec.message.popErrors() }
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
    if (owner) {
      const audience = Array.from(share.audienceList || []).filter((row) => !row.revoked)
      // the people it still reaches who left the room by themselves: the owner takes them out in Who sees my location
      const inRoom = new Set(Array.from(ec.entity.find('moqui.chat.ChatRoomMember').condition('chatRoomId', p.chatRoomId).condition('leftDate', null)
        .selectField('userId').disableAuthz().list()).map((row) => row.userId))
      Object.assign(view, { audienceVersion: share.audienceVersion, sentFromDevice: !!share.sentFromDevice, deviceName: share.deviceName || null,
        fromThisDevice: !!share.fromThisDevice, audienceCount: audience.length, intervalSeconds: intervalOf(!!share.approximate),
        leftRoomNames: audience.filter((row) => row.audienceTypeEnumId === 'LsaUser' && row.userId && !inRoom.has(row.userId)).map((row) => row.userName || row.userId) })
    }
    return Object.assign(out, { state: active ? 'live' : 'ended', owner, share: view, latest: active ? got.latest || null : null })
  }
  const status = ec.service.sync().name(LIVE + 'get#ShareStatus').parameters({ shareId, ownerUserId: event.senderUserId }).call() || {}
  if (ec.message.hasError()) return Object.assign(out, { reasonCode: 'notFound' })
  return Object.assign(out, { state: status.active ? 'notShared' : 'ended', share: { shareId, label: place.label || null, thruDate: place.thruDate || null } })
}

const operations = { send, words: wordsOf, mark, live: sendLive, liveView, revokeLive }

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown location operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute, fallbackText, clean })
