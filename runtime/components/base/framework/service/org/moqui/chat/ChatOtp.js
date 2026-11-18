// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// One-time codes and screen security (plans/Comms-Messaging.md §18.3 phase 21): the services of
// org.moqui.chat.ChatPrivacyServices. A code is found only in an incoming text from outside (SMS, MMS, RCS: Sms.js
// receive), never in chat between users: a 4 to 8 digit candidate near a keyword (chat@otp-keywords), with phone numbers,
// prices, dates, times, years and links left out, and two candidates as near taken as none. The message keeps it as
// content.otp {code}; its push carries it where the push carries the snippet (chat_push.rs), for a Copy button. When every
// local member of the room asks for it (CHAT_OTP_DELETE) and the code may delete itself (the text's only candidate, within
// 20 characters of a core word: code, OTP, verification, passcode, security code, one-time; "confirm", "login" and "pin"
// find a code for Copy but never delete one), the message gets phase 16's expireDate a day on, and goes from the server
// and every device then; Keep takes that back.

const EVENT = 'moqui.chat.ChatEvent'
const MEMBER = 'moqui.chat.ChatRoomMember'
const DAY = 86400 * 1000
// the words a code may delete itself by: only these, near it (CORE_NEAR characters), with one candidate in the text
const CORE = ['code', 'codes', 'otp', 'verification', 'passcode', 'security code', 'one-time', 'one time']
const CORE_NEAR = 20
const KEYWORDS = ['code', 'codes', 'verification', 'verify', 'otp', 'passcode', 'one-time', 'one time', 'pin', 'security code', 'login', 'log in',
  'sign in', 'sign-in', '2fa', 'two-factor', 'authentication', 'authenticate', 'confirm', 'confirmation']

const find = (entity) => ec.entity.find(entity).disableAuthz()
const localize = (text) => ec.l10n.localize(text)
const parse = (text) => { if (!text) return {}; try { const value = JSON.parse(text); return value && typeof value === 'object' ? value : {} } catch (err) { return {} } }

/** The keywords: chat@otp-keywords (comma separated) when set, else the English ones. */
function keywords() {
  const own = String(ec.factory.getConfValue('chat@otp-keywords') || '').split(',').map((word) => word.trim().toLowerCase()).filter((word) => word)
  return own.length ? own : KEYWORDS
}

/**
 * The code of a text, or null: {code, strong}. A candidate is 4 to 8 digits on word boundaries (one space or dash inside
 * allowed: "123 456", "123-456"), or the digits after a "G-" style prefix; it is not part of a phone number, a price or
 * an amount, a date or time, a year (unless right after "code" or "is"), or a link; the one nearest a keyword, within 40
 * characters. Two as near are ambiguous: none. strong: it was the only candidate in the text.
 */
function detect(text) {
  const input = String(text == null ? '' : text)
  if (!input.trim() || input.length > 600) return null
  const lower = input.toLowerCase()
  const marks = []
  for (const word of keywords()) {
    const pattern = new RegExp('(^|[^a-z0-9])' + word.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '(?=$|[^a-z0-9])', 'g')
    let match
    while ((match = pattern.exec(lower))) marks.push({ word, start: match.index + match[1].length, end: match.index + match[1].length + word.length })
  }
  if (!marks.length) return null
  // the parts no code is in: links, phone numbers, prices and amounts, dates and times
  const blocked = []
  const block = (pattern) => { let match; while ((match = pattern.exec(input))) blocked.push([match.index, match.index + match[0].length]) }
  block(/\b(?:https?:\/\/|www\.)\S+/gi)
  block(/\+\d[\d\s().-]{7,}\d/g)
  block(/\(\d{3}\)\s*\d{3}[\s.-]?\d{4}/g)
  block(/\b\d{3}[.-]\d{3}[.-]\d{4}\b/g)
  block(/(?:[$€£¥₹]|\b(?:usd|eur|gbp|cad|aud)\s?)\s?\d[\d,]*(?:\.\d+)?/gi)
  block(/\b\d[\d,]*\.\d+\b/g)
  block(/\b\d{1,4}[/-]\d{1,2}(?:[/-]\d{2,4})?\b/g)
  block(/\b\d{1,2}:\d{2}(?::\d{2})?\b/g)
  const inside = (start, end) => blocked.some(([from, to]) => start < to && end > from)
  const candidates = []
  const pattern = /(?:\b[A-Z]{1,3}-)?\b(\d{2,4}[ -]\d{2,4}|\d{4,8})\b/g
  let match
  while ((match = pattern.exec(input))) {
    const digits = match[1].replace(/[ -]/g, '')
    if (digits.length < 4 || digits.length > 8) continue
    const start = match.index
    const end = match.index + match[0].length
    if (inside(start, end)) continue
    // a longer run of digits around it (a phone number, an account number): no code
    if (/\d/.test(input.charAt(start - 1)) || /\d/.test(input.charAt(end))) continue
    if (/^(19|20)\d\d$/.test(digits) && !/(code|is)[\s:]*$/i.test(input.slice(Math.max(0, start - 8), start))) continue
    candidates.push({ code: digits, start, end })
  }
  if (!candidates.length) return null
  const gap = (one, mark) => (one.end <= mark.start ? mark.start - one.end : one.start >= mark.end ? one.start - mark.end : 0)
  const distance = (one, list) => Math.min(...list.map((mark) => gap(one, mark)))
  const near = candidates.map((one) => Object.assign(one, { distance: distance(one, marks) })).filter((one) => one.distance <= 40).sort((a, b) => a.distance - b.distance)
  if (!near.length) return null
  if (near.length > 1 && near[1].distance === near[0].distance && near[1].code !== near[0].code) return null
  // a code may delete itself only by a core word near it (not "confirm", "login" or "pin"), the text's only candidate
  const core = marks.filter((mark) => CORE.includes(mark.word))
  const deletable = candidates.length === 1 && core.length > 0 && distance(near[0], core) <= CORE_NEAR
  return { code: near[0].code, strong: candidates.length === 1, deletable }
}

/** detect#OneTimeCode: the code of a text (for Sms.js and the spec). */
function detectOp(p) {
  const found = detect(p.text)
  return { code: found ? found.code : null, strong: found ? found.strong : false, deletable: found ? found.deletable : false }
}

/**
 * mark#OneTimeCode (internal, Sms.js after an inbound text was taken in): the message keeps its code (content.otp), and
 * when it is strong and every local member of the room has CHAT_OTP_DELETE Y, it disappears a day after it was sent
 * (phase 16's expireDate). The update is pushed. Out: code, expireDate.
 */
function mark(p) {
  const row = find(EVENT).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).forUpdate(true).one()
  if (!row || row.eventTypeEnumId !== 'CetMessage' || row.senderUserId || row.redactedDate) return { code: null }
  const found = detect(row.bodyText || p.text)
  if (!found) return { code: null }
  const content = parse(row.contentJson)
  content.otp = { code: found.code }
  row.set('contentJson', JSON.stringify(content))
  let expireDate = null
  if (found.deletable && !row.expireDate) {
    const members = Array.from(find(MEMBER).condition('chatRoomId', p.chatRoomId).condition('leftDate', null).selectField('userId').list())
      .map((member) => member.userId).filter((userId) => userId)
    const wants = members.length > 0 && members.every((userId) => {
      const pref = find('moqui.security.UserPreference').condition({ userId, preferenceKey: 'CHAT_OTP_DELETE' }).one()
      return pref && pref.preferenceValue === 'Y'
    })
    if (wants) {
      expireDate = Number(row.sentDate || ec.user.nowTimestamp) + DAY
      row.set('expireDate', new Date(expireDate))
    }
  }
  row.update()
  ec.service.sync().name('org.moqui.chat.ChatServices.push#Update').parameters({ chatRoomId: p.chatRoomId, eventSeq: Number(p.eventSeq),
    fields: expireDate ? { otp: content.otp, expireDate } : { otp: content.otp } }).disableAuthz().call()
  return { code: found.code, expireDate }
}

/** keep#CodeMessage: a member keeps a code's message that was to disappear after a day (its expireDate cleared). */
function keep(p) {
  const access = ec.service.sync().name('org.moqui.chat.ChatServices.get#RoomAccess').parameter('chatRoomId', p.chatRoomId).call()
  if (ec.message.hasError()) return {}
  if (!access || !access.isMember) { ec.message.addError(ec.resource.expand(localize('Chat room ${id} not found'), '', { id: p.chatRoomId })); return { reasonCode: 'notFound' } }
  const row = find(EVENT).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).forUpdate(true).one()
  if (!row || !parse(row.contentJson).otp) { ec.message.addError(localize('That message has no code')); return { reasonCode: 'notFound' } }
  // only a code's own day: a room's disappearing timer is the room's, not one member's to take back
  const room = find('moqui.chat.ChatRoom').condition('chatRoomId', p.chatRoomId).one()
  if (room && room.messageTimerSeconds) { ec.message.addError(localize('This room\'s messages disappear by its timer')); return { reasonCode: 'roomTimer' } }
  if (row.expireDate) {
    row.set('expireDate', null)
    row.update()
    ec.service.sync().name('org.moqui.chat.ChatServices.push#Update').parameters({ chatRoomId: p.chatRoomId, eventSeq: Number(p.eventSeq),
      fields: { expireDate: null } }).disableAuthz().call()
  }
  return { kept: true }
}

/** set#RoomScreenSecure: the caller's own choice to block screenshots in an encrypted room they are in. */
function roomScreen(p) {
  const member = find(MEMBER).condition({ chatRoomId: p.chatRoomId, userId: ec.user.userId }).condition('leftDate', null).forUpdate(true).one()
  if (!member) { ec.message.addError(ec.resource.expand(localize('Chat room ${id} not found'), '', { id: p.chatRoomId })); return { reasonCode: 'notFound' } }
  member.set('screenSecure', p.on === true || p.on === 'true' ? 'Y' : null)
  member.update()
  return { screenSecure: member.screenSecure === 'Y' }
}

const operations = { detect: detectOp, mark, keep, roomScreen }

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown chat privacy operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
