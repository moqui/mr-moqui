// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Location messages (plans/Comms-Messaging.md §18.3 phase 20): the services of org.moqui.chat.ChatLocationServices. A
// location is a message (CetMessage) with content.location {latitude, longitude, label, address, accuracy, source}: the
// coordinates or an address or both. There is no map, no geocoding and no outside request. Its body is its words with an
// OpenStreetMap link (fallbackText), so every client, bridge and text shows it; Sinch RCS sends Sinch's location message
// (crates/moqui-channels, providers/sinch/rcs.rs). An inbound RCS location (Sms.js) is made with the same words. In an
// encrypted room the browser makes the same words (moqui.chat.locationFallback in WebrootChat.js; the specs keep the two
// in step) and the place is only in the ciphertext.

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

const operations = { send, words: wordsOf, mark }

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown location operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute, fallbackText, clean })
