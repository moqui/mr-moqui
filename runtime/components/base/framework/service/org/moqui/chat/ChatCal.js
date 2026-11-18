// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Event messages with RSVP (plans/Comms-Messaging.md §18.3 phase 19): the services of org.moqui.chat.ChatCalServices.
// An event is a message (CetMessage) with content.calEvent {title}; its body is the event in words (fallbackText), so
// the room list, search, notifications, the terminal, XMPP, SMS and every bridge show it; ChatCalEvent keeps its state and
// ChatCalAnswer the members' answers, read into the timeline (get#CalEventStates) and pushed as updates, as a poll's are.
//
// A change or a cancellation is a message of its own, a reply to the event in its thread (content.calChange), so every
// member and bridge sees it. An answer may ask for a reminder (phase 15's ChatMessageReminder, moved with the start) and a
// calendar entry: an app's (the chat-calendar provider hook, given ids only: it reads these rows itself), else the
// event's .ics file (get#CalEventIcs). An encrypted room has no events, as it has no polls: the server keeps the answers
// and makes the text.

const EVENT = 'moqui.chat.ChatEvent'
const CAL = 'moqui.chat.ChatCalEvent'
const ANSWER = 'moqui.chat.ChatCalAnswer'
const REMINDER = 'moqui.chat.ChatMessageReminder'
const CHAT = 'org.moqui.chat.ChatServices.'
const SELF = 'org.moqui.chat.ChatCalServices.'
const MINUTE = 60 * 1000
const DAY = 86400 * 1000

const LIMITS = { titleMax: 200, placeMax: 300, descriptionMax: 2000, lengthDays: 31, aheadDays: 731, changesMax: 20, remindMinutesMax: 10080 }
const ANSWERS = { going: 'CcaGoing', maybe: 'CcaMaybe', no: 'CcaNo' }
const CODES = { CcaGoing: 'going', CcaMaybe: 'maybe', CcaNo: 'no' }
const COUNTS = { CcaGoing: 'goingCount', CcaMaybe: 'maybeCount', CcaNo: 'noCount' }
const DATE = 'EEE d MMM yyyy'
const TIME = 'HH:mm'

const find = (entity) => ec.entity.find(entity).disableAuthz()
const blank = (value) => value == null || String(value).trim() === ''
const millis = (value) => (value == null || value === '' ? null : Number(value))
const now = () => Number(ec.user.nowTimestamp)
const localize = (text) => ec.l10n.localize(text)
const expand = (text, values) => ec.resource.expand(localize(text), '', values)
const escape = (text) => String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
const words = (value) => String(value ?? '').replace(/\s+/g, ' ').trim()
const paragraph = (value) => String(value ?? '').replace(/\r\n?/g, '\n').split('\n').map((line) => line.replace(/[ \t]+/g, ' ').trim()).join('\n').replace(/\n{3,}/g, '\n\n').trim()
const parse = (text) => { if (!text) return {}; try { const value = JSON.parse(text); return value && typeof value === 'object' ? value : {} } catch (err) { return {} } }
const message = (err) => (err && err.message ? err.message : String(err))
const appRoot = () => (System.getProperty('comms_app_root') || '/qapps/BasicComms').replace(/\/+$/, '')
const roomLink = (chatRoomId, eventSeq) => `${appRoot()}/Chat?chatRoomId=${encodeURIComponent(chatRoomId)}&eventSeq=${eventSeq}`

/** A refusal: its message an error, its reason code in the answer. */
function refuse(reasonCode, text) {
  ec.message.addError(text)
  return { reasonCode }
}

/**
 * body() in a transaction of its own (a screen's transition has one in place), committed when it ends with no error, so
 * the calendar providers (each in a transaction of its own) read what it wrote; rolled back on an error message or a
 * throw. Returns what body returns, or null when it failed.
 */
function committed(body) {
  let out = null
  try {
    ec.transaction.runRequireNew(null, 'Error in a chat event', () => {
      out = body()
      if (ec.message.hasError()) throw new Error(ec.message.errorsString)
    })
  } catch (err) {
    if (!ec.message.hasError()) ec.message.addError(message(err))
    return null
  }
  return out
}

/** The .ics UID of an event: stable for the message, the same in every file and every calendar provider. */
function uidOf(chatRoomId, eventSeq) {
  return `chat-${chatRoomId}-${eventSeq}@${System.getProperty('chat_cal_uid_domain') || 'chat.moqui'}`
}

let zoneSet = null
function knownZone(zone) {
  if (!zoneSet) zoneSet = new Set(Array.from(ec.l10n.getAvailableTimeZones() || []).map(String))
  return zoneSet.has(String(zone))
}

const format = (value, pattern, zone) => ec.l10n.format(new Date(Number(value)), pattern, null, zone)

/** The time line of an event in its zone: "Sat 21 Nov 2026, 14:00–16:00 (America/Denver)", or "…, all day". */
function whenText(cal) {
  const zone = cal.timeZone
  const start = millis(cal.startDate)
  const end = millis(cal.endDate)
  if (cal.allDay === 'Y') {
    const first = format(start, DATE, zone)
    const last = format(end - 1, DATE, zone)
    return first === last ? expand('${day}, all day', { day: first }) : first + ' – ' + last
  }
  const startDay = format(start, DATE, zone)
  const endDay = format(end, DATE, zone)
  const span = startDay === endDay
    ? startDay + ', ' + format(start, TIME, zone) + '–' + format(end, TIME, zone)
    : startDay + ', ' + format(start, TIME, zone) + ' – ' + endDay + ', ' + format(end, TIME, zone)
  return span + ' (' + zone + ')'
}

/** The event in words: its lines, the first its title. */
function fallbackLines(cal) {
  const lines = [expand('Event: ${title}', { title: cal.title }), whenText(cal)]
  if (!blank(cal.place)) lines.push(expand('Where: ${place}', { place: cal.place }))
  if (!blank(cal.description)) lines.push(String(cal.description).length > 300 ? String(cal.description).slice(0, 299) + '…' : String(cal.description))
  return lines
}

/** Lines as one paragraph of safe HTML, so send#Message's bodyText (what a bridge sends) is the lines with newlines. */
function linesHtml(lines, bold) {
  return '<p>' + lines.map((line, index) => {
    const text = escape(line).replace(/\n/g, '<br>')
    return index === 0 && bold ? '<strong>' + text + '</strong>' : text
  }).join('<br>') + '</p>'
}

/** The fields of an event as given, checked against the limits; `before` the row a change starts from. Or a refusal. */
function checked(p, before) {
  const pick = (name) => (p[name] !== undefined && p[name] !== null ? p[name] : before ? before[name] : null)
  const title = words(pick('title'))
  if (!title) return refuse('badEvent', localize('An event needs a title'))
  if (title.length > LIMITS.titleMax) return refuse('tooLong', expand('The title is up to ${max} characters', { max: LIMITS.titleMax }))
  const place = words(pick('place'))
  if (place.length > LIMITS.placeMax) return refuse('tooLong', expand('The place is up to ${max} characters', { max: LIMITS.placeMax }))
  const description = paragraph(pick('description'))
  if (description.length > LIMITS.descriptionMax) return refuse('tooLong', expand('The details are up to ${max} characters', { max: LIMITS.descriptionMax }))
  const zone = String(pick('timeZone') || ec.user.timeZone || '')
  if (!knownZone(zone)) return refuse('badTime', expand('Unknown time zone ${zone}', { zone }))
  const allDayGiven = p.allDay !== undefined && p.allDay !== null ? p.allDay : before ? before.allDay === 'Y' : false
  const allDay = allDayGiven === true || allDayGiven === 'Y' || allDayGiven === 'true'
  const start = millis(pick('startDate'))
  const end = millis(pick('endDate'))
  if (start == null || end == null || Number.isNaN(start) || Number.isNaN(end)) return refuse('badTime', localize('An event needs its start and end'))
  if (end <= start) return refuse('badTime', localize('An event ends after it starts'))
  if (end - start > LIMITS.lengthDays * DAY) return refuse('badTime', expand('An event lasts at most ${days} days', { days: LIMITS.lengthDays }))
  if (end <= now()) return refuse('badTime', localize('An event ends later than now'))
  if (start > now() + LIMITS.aheadDays * DAY) return refuse('badTime', localize('An event starts within two years'))
  if (allDay && (format(start, TIME, zone) !== '00:00' || format(end, TIME, zone) !== '00:00')) {
    return refuse('badTime', localize('An all-day event starts and ends at midnight in its time zone'))
  }
  return { fields: { title, place: place || null, description: description || null, timeZone: zone, allDay: allDay ? 'Y' : 'N', startDate: start, endDate: end } }
}

/** The room's access for the current user, or a refusal (not found, cannot post). */
function postable(chatRoomId) {
  const access = ec.service.sync().name(CHAT + 'get#RoomAccess').parameters({ chatRoomId }).call()
  if (ec.message.hasError()) return { error: true, reasonCode: null }
  if (!access || !access.canRead) return Object.assign({ error: true }, refuse('notFound', expand('Chat room ${id} not found', { id: chatRoomId })))
  if (!access.canPost) {
    return Object.assign({ error: true }, refuse(access.isArchived ? 'archived' : 'notMember', access.isArchived ? localize('The room is archived') : localize('Join the room to take part')))
  }
  return { access }
}

const encryptedRoom = (chatRoomId) => find('moqui.chat.ChatRoom').condition('chatRoomId', chatRoomId).condition('encrypted', 'Y').count() > 0

/** The shared state of one event, for push#Update (no one's own answer). */
function sharedState(chatRoomId, eventSeq) {
  return (ec.service.sync().name(SELF + 'get#CalEventStates').parameters({ chatRoomId, eventSeqList: [eventSeq], shared: true }).call().calEventMap || {})[eventSeq] || null
}
function myState(chatRoomId, eventSeq) {
  return (ec.service.sync().name(SELF + 'get#CalEventStates').parameters({ chatRoomId, eventSeqList: [eventSeq] }).call().calEventMap || {})[eventSeq] || null
}
function pushState(chatRoomId, eventSeq) {
  ec.service.sync().name(CHAT + 'push#Update').parameters({ chatRoomId, eventSeq, fields: { calEvent: sharedState(chatRoomId, eventSeq) } }).disableAuthz().call()
}

/**
 * The chat-calendar providers for this event, each in a transaction of its own (call#Providers' read mode), given ids and
 * the action only: a provider reads ChatCalEvent and the caller's ChatCalAnswer itself. Out: {eventUrl, failed, none}.
 */
function calendarCall(chatRoomId, eventSeq, action) {
  let out = { eventUrl: null, failed: false, none: true }
  ec.message.pushErrors()
  try {
    const answer = ec.service.sync().name('org.moqui.ui.ShellServices.call#Providers').parameters({ hook: 'chat-calendar',
      parameters: { chatRoomId, eventSeq, action } }).call() || {}
    const results = Array.from(answer.results || [])
    const failed = Array.from(answer.failedParts || [])
    for (const part of failed) ec.logger.error(`The chat-calendar provider ${part.sourceId} failed on ${action} of event ${eventSeq} of room ${chatRoomId}: ${part.error}`)
    const url = results.map((result) => result && result.eventUrl).find((value) => !blank(value))
    out = { eventUrl: url || null, failed: failed.length > 0 || ec.message.hasError(), none: answer.unavailable === true }
    if (ec.message.hasError()) ec.logger.error(`The chat-calendar providers failed on ${action} of event ${eventSeq} of room ${chatRoomId}: ${ec.message.errorsString}`)
  } catch (err) {
    ec.logger.error(`The chat-calendar providers failed on ${action} of event ${eventSeq} of room ${chatRoomId}: ${message(err)}`)
    out = { eventUrl: null, failed: true, none: false }
  }
  ec.message.clearErrors()
  ec.message.popErrors()
  return out
}

/** Cancel the pending reminders these answers made; each on its own. */
function cancelReminders(answers) {
  for (const row of answers) {
    if (blank(row.chatMessageReminderId)) continue
    try {
      const reminder = find(REMINDER).condition('chatMessageReminderId', row.chatMessageReminderId).forUpdate(true).one()
      if (reminder && reminder.statusEnumId === 'CrmPending' && reminder.userId === row.userId) {
        reminder.set('statusEnumId', 'CrmCancelled')
        reminder.update()
      }
    } catch (err) {
      ec.logger.error(`Could not cancel the event reminder ${row.chatMessageReminderId}: ${message(err)}`)
    }
  }
}

/**
 * create#CalEvent: an event from the current user, a member of a room that is not archived or encrypted: send#Message
 * with the event in words, then content.calEvent, the row and the shared state pushed.
 */
function create(p) {
  const out = { eventSeq: null, event: null, duplicate: false, reasonCode: null }
  if (encryptedRoom(p.chatRoomId)) return Object.assign(out, refuse('encryptedRoom', localize('An encrypted room has no events: the server keeps the answers and makes the event\'s text')))
  const room = postable(p.chatRoomId)
  if (room.error) return Object.assign(out, { reasonCode: room.reasonCode })
  const check = checked(p, null)
  if (!check.fields) return Object.assign(out, check)
  const fields = check.fields
  const sent = ec.service.sync().name(CHAT + 'send#Message').parameters({ chatRoomId: p.chatRoomId, body: linesHtml(fallbackLines(fields), true),
    threadRootEventSeq: p.threadRootEventSeq ?? null, clientRef: p.clientRef || null, roomUrl: p.roomUrl || null }).call()
  if (ec.message.hasError()) return Object.assign(out, { reasonCode: (sent && sent.reasonCode) || null })
  out.eventSeq = sent.eventSeq
  out.event = sent.event
  if (sent.duplicate) return Object.assign(out, { duplicate: true })
  const row = ec.entity.find(EVENT).condition('chatRoomId', p.chatRoomId).condition('eventSeq', sent.eventSeq).one()
  const content = parse(row.contentJson)
  content.calEvent = { title: fields.title }
  row.set('contentJson', JSON.stringify(content))
  row.update()
  const cal = ec.entity.makeValue(CAL)
  cal.setFields(Object.assign({ chatRoomId: p.chatRoomId, eventSeq: sent.eventSeq, createdByUserId: ec.user.userId, revision: 0,
    goingCount: 0, maybeCount: 0, noCount: 0 }, fields, { startDate: new Date(fields.startDate), endDate: new Date(fields.endDate) }), true, null, null)
  cal.create()
  out.event = Object.assign({}, sent.event, { calEvent: myState(p.chatRoomId, sent.eventSeq) })
  pushState(p.chatRoomId, sent.eventSeq)
  return out
}

/** get#CalEventStates: the events of these messages, with the current user's own answer unless shared. */
function states(p) {
  const calEventMap = {}
  const seqs = Array.from(new Set(Array.from(p.eventSeqList || []).map(Number).filter((seq) => !Number.isNaN(seq))))
  if (!seqs.length) return { calEventMap }
  const rows = Array.from(find(CAL).condition('chatRoomId', p.chatRoomId).condition('eventSeq', 'in', seqs).list())
  if (!rows.length) return { calEventMap }
  const answers = {}
  for (const row of find(ANSWER).condition('chatRoomId', p.chatRoomId).condition('eventSeq', 'in', seqs).orderBy('answeredDate').list()) {
    (answers[Number(row.eventSeq)] = answers[Number(row.eventSeq)] || []).push(row)
  }
  const me = ec.user.userId
  const at = now()
  for (const row of rows) {
    const seq = Number(row.eventSeq)
    const given = answers[seq] || []
    const byAnswer = { going: [], maybe: [], no: [] }
    for (const answer of given) if (CODES[answer.answerEnumId] && answer.userId) byAnswer[CODES[answer.answerEnumId]].push(answer.userId)
    const end = millis(row.endDate)
    const state = { title: row.title ?? null, start: millis(row.startDate), end, timeZone: row.timeZone ?? null, allDay: row.allDay === 'Y',
      place: row.place ?? null, description: row.description ?? null, revision: Number(row.revision || 0), cancelled: !!row.cancelledDate,
      cancelledDate: millis(row.cancelledDate), ended: end != null && end <= at, createdByUserId: row.createdByUserId ?? null,
      lastChangeEventSeq: row.lastChangeEventSeq != null ? Number(row.lastChangeEventSeq) : null,
      going: Number(row.goingCount || 0), maybe: Number(row.maybeCount || 0), no: Number(row.noCount || 0), answers: byAnswer,
      uid: uidOf(p.chatRoomId, seq) }
    if (!p.shared) {
      const mine = given.find((answer) => answer.userId === me)
      // one object, so a pushed update (the same for everyone, with none) keeps it (moqui.chat.applyUpdate)
      state.mine = mine ? { answer: CODES[mine.answerEnumId] || null, remindMinutes: mine.remindMinutes != null ? Number(mine.remindMinutes) : null,
        calendarDate: millis(mine.calendarDate) } : null
    }
    calEventMap[seq] = state
  }
  return { calEventMap }
}

/** The event row, locked, of a message of the room that is an event; or a refusal. */
function lockedEvent(chatRoomId, eventSeq) {
  const cal = ec.entity.find(CAL).condition('chatRoomId', chatRoomId).condition('eventSeq', eventSeq).forUpdate(true).one()
  if (!cal) { refuse('notFound', expand('Event ${seq} not found in the room', { seq: eventSeq })); return null }
  return cal
}

/**
 * answer#CalEvent: the current user's answer (going, maybe, no, or empty to take it back), the counts kept; then its
 * reminder and its calendar entry, each on its own (a failure logged and answered; the answer stands).
 */
function answer(p) {
  const out = { calEvent: null, eventUrl: null, reminderSkipped: false, reminderFailed: false, calendarFailed: false, reasonCode: null }
  const done = committed(() => recordAnswer(p, out))
  if (!done) return out
  // the calendar: the current user's entry put, changed or removed by the providers, once the answer is committed
  if (done.wantsEntry || (done.hadEntry && (!done.after || done.after === 'CcaNo'))) {
    const called = calendarCall(p.chatRoomId, Number(p.eventSeq), 'answer')
    out.calendarFailed = called.failed
    out.eventUrl = called.eventUrl
    if (done.after && !called.failed && !called.none) {
      committed(() => {
        const row = ec.entity.find(ANSWER).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).condition('chatActorId', done.actor).forUpdate(true).one()
        if (!row) return true
        row.set('calendarDate', done.wantsEntry ? ec.user.nowTimestamp : null)
        row.set('calendarRevision', done.wantsEntry ? done.revision : null)
        row.update()
        return true
      })
    }
  }
  if (done.changed) pushState(p.chatRoomId, Number(p.eventSeq))
  out.calEvent = myState(p.chatRoomId, Number(p.eventSeq))
  return out
}

/** answer#CalEvent's writes: the answer, the counts and the reminder. Out: what the calendar step needs, or null. */
function recordAnswer(p, out) {
  const room = postable(p.chatRoomId)
  const fail = (fields) => { Object.assign(out, fields); return null }
  if (room.error) return fail({ reasonCode: room.reasonCode })
  const cal = lockedEvent(p.chatRoomId, p.eventSeq)
  if (!cal) return fail({ reasonCode: 'notFound' })
  if (cal.cancelledDate) return fail(refuse('cancelled', localize('The event is cancelled')))
  if (millis(cal.endDate) <= now()) return fail(refuse('ended', localize('The event is over')))
  const code = blank(p.answer) ? null : String(p.answer).toLowerCase()
  if (code != null && !ANSWERS[code]) return fail(refuse('badAnswer', localize('The answer is going, maybe or no')))
  let remind = p.remindMinutes == null || p.remindMinutes === '' ? null : Number(p.remindMinutes)
  if (remind != null && (!Number.isInteger(remind) || remind < 0 || remind > LIMITS.remindMinutesMax)) {
    return fail(refuse('badTime', expand('A reminder is 0 to ${max} minutes before the start', { max: LIMITS.remindMinutesMax })))
  }
  if (code == null || code === 'no') remind = null
  const me = ec.user.userId
  const actor = (ec.service.sync().name(CHAT + 'get#LocalActors').parameter('userIdList', [me]).disableAuthz().call()?.actorMap || {})[me]
  if (!actor) return fail(refuse('notFound', localize('You have no chat actor')))
  let row = ec.entity.find(ANSWER).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).condition('chatActorId', actor).forUpdate(true).one()
  const before = row ? row.answerEnumId : null
  const hadEntry = !!(row && row.calendarDate)
  const after = code ? ANSWERS[code] : null
  if (before !== after) {
    if (before && COUNTS[before]) cal.set(COUNTS[before], Math.max(0, Number(cal[COUNTS[before]] || 0) - 1))
    if (after) cal.set(COUNTS[after], Number(cal[COUNTS[after]] || 0) + 1)
    cal.update()
  }
  if (!after) {
    if (row) {
      cancelReminders([row])
      row.delete()
      row = null
    }
  } else {
    if (!row) {
      row = ec.entity.makeValue(ANSWER)
      row.setFields({ chatRoomId: p.chatRoomId, eventSeq: p.eventSeq, chatActorId: actor, userId: me, answerEnumId: after,
        answeredDate: ec.user.nowTimestamp, remindMinutes: remind }, true, null, null)
      row.create()
    } else {
      row.set('answerEnumId', after)
      if (before !== after) row.set('answeredDate', ec.user.nowTimestamp)
      row.set('remindMinutes', remind)
    }
    // the reminder: set (or moved) for going and maybe with remindMinutes, else cancelled
    if (remind == null) {
      cancelReminders([row])
      row.set('chatMessageReminderId', null)
    } else {
      const at = millis(cal.startDate) - remind * MINUTE
      if (at < now() + MINUTE) {
        cancelReminders([row])
        row.set('chatMessageReminderId', null)
        out.reminderSkipped = true
      } else {
        const set = reminderInOwn(p.chatRoomId, Number(p.eventSeq), at)
        if (set.id) row.set('chatMessageReminderId', set.id)
        else out.reminderFailed = true
      }
    }
    row.update()
  }
  const wantsEntry = !!((p.addToCalendar === true || p.addToCalendar === 'true') && after && after !== 'CcaNo')
  return { actor, before, after, hadEntry, wantsEntry, changed: before !== after, revision: Number(cal.revision || 0) }
}

/** set#Reminder as the current user, in a transaction of its own; one that fails is logged. Out: {id}. */
function reminderInOwn(chatRoomId, eventSeq, at) {
  let id = null
  ec.message.pushErrors()
  try {
    const set = ec.service.sync().name('org.moqui.chat.ChatScheduleServices.set#Reminder').parameters({ chatRoomId, eventSeq, remindDate: new Date(at) })
      .requireNewTransaction(true).call() || {}
    if (ec.message.hasError()) ec.logger.error(`Could not set the reminder of event ${eventSeq} of room ${chatRoomId}: ${ec.message.errorsString}`)
    else id = set.chatMessageReminderId || null
  } catch (err) {
    ec.logger.error(`Could not set the reminder of event ${eventSeq} of room ${chatRoomId}: ${message(err)}`)
  }
  ec.message.clearErrors()
  ec.message.popErrors()
  return { id }
}

/** May the current user change or cancel this event: its creator while a member who may post, or a moderator. */
function mayManage(cal, access) {
  return (cal.createdByUserId === ec.user.userId && access.canPost) || access.canModerate === true
}

/** A change or cancellation message: a reply to the event in its thread, sent as the current user. */
function postChange(chatRoomId, eventSeq, lines, change) {
  const event = find(EVENT).condition('chatRoomId', chatRoomId).condition('eventSeq', eventSeq).one()
  const sent = ec.service.sync().name(CHAT + 'send#Message').parameters({ chatRoomId, body: linesHtml(lines, false), replyToEventSeq: eventSeq,
    threadRootEventSeq: event && event.threadRootEventSeq != null ? Number(event.threadRootEventSeq) : null }).call()
  if (ec.message.hasError() || !sent || sent.eventSeq == null) return null
  const row = ec.entity.find(EVENT).condition('chatRoomId', chatRoomId).condition('eventSeq', sent.eventSeq).one()
  const content = parse(row.contentJson)
  content.calChange = change
  row.set('contentJson', JSON.stringify(content))
  row.update()
  ec.service.sync().name(CHAT + 'push#Update').parameters({ chatRoomId, eventSeq: Number(sent.eventSeq), fields: { calChange: change } }).disableAuthz().call()
  return Number(sent.eventSeq)
}

/**
 * update#CalEvent: its creator or a moderator changes an event that is not cancelled or over; revision + 1, a change
 * message, the answers' reminders moved (each on its own), then the providers' change.
 */
function update(p) {
  const out = { calEvent: null, changeEventSeq: null, reasonCode: null }
  const done = committed(() => recordUpdate(p, out))
  if (!done) return out
  if (done.changed) {
    calendarCall(p.chatRoomId, Number(p.eventSeq), 'change')
    pushState(p.chatRoomId, Number(p.eventSeq))
  }
  out.calEvent = myState(p.chatRoomId, Number(p.eventSeq))
  return out
}

/** update#CalEvent's writes: the row, the change message and the reminders moved. Out: {changed}, or null. */
function recordUpdate(p, out) {
  const room = postable(p.chatRoomId)
  const fail = (fields) => { Object.assign(out, fields); return null }
  if (room.error) return fail({ reasonCode: room.reasonCode })
  const cal = lockedEvent(p.chatRoomId, p.eventSeq)
  if (!cal) return fail({ reasonCode: 'notFound' })
  if (!mayManage(cal, room.access)) return fail(refuse('notAllowed', localize('Only the event\'s creator or a moderator changes it')))
  if (cal.cancelledDate) return fail(refuse('cancelled', localize('The event is cancelled')))
  if (millis(cal.endDate) <= now()) return fail(refuse('ended', localize('The event is over')))
  if (Number(cal.revision || 0) >= LIMITS.changesMax) return fail(refuse('tooManyChanges', expand('An event is changed at most ${max} times', { max: LIMITS.changesMax })))
  const before = { title: cal.title, place: cal.place, description: cal.description, timeZone: cal.timeZone, allDay: cal.allDay,
    startDate: millis(cal.startDate), endDate: millis(cal.endDate) }
  const check = checked(p, before)
  if (!check.fields) return fail(check)
  const fields = check.fields
  const what = []
  if (fields.title !== before.title) what.push('title')
  if (fields.startDate !== before.startDate || fields.endDate !== before.endDate || fields.timeZone !== before.timeZone || fields.allDay !== (before.allDay || 'N')) what.push('when')
  if ((fields.place || null) !== (before.place || null)) what.push('place')
  if ((fields.description || null) !== (before.description || null)) what.push('description')
  if (!what.length) return { changed: false }
  const revision = Number(cal.revision || 0) + 1
  cal.setFields(Object.assign({}, fields, { startDate: new Date(fields.startDate), endDate: new Date(fields.endDate), revision }), true, null, null)
  cal.update()
  const lines = [expand('Changed: ${title}', { title: fields.title })]
  if (what.includes('when') || what.includes('title')) lines.push(expand('Now ${when}', { when: whenText(fields) }))
  if (what.includes('place')) lines.push(fields.place ? expand('Where: ${place}', { place: fields.place }) : localize('No place now'))
  if (what.includes('description') && lines.length === 1) lines.push(localize('The details changed'))
  const changeSeq = postChange(p.chatRoomId, Number(p.eventSeq), lines, { eventSeq: Number(p.eventSeq), revision, what })
  if (ec.message.hasError()) return null
  if (changeSeq != null) { cal.set('lastChangeEventSeq', changeSeq); cal.update() }
  out.changeEventSeq = changeSeq
  if (fields.startDate !== before.startDate) moveReminders(p.chatRoomId, Number(p.eventSeq), fields.startDate)
  return { changed: true }
}

/** The answers' pending reminders moved to the new start, each on its own; one now too near is cancelled. */
function moveReminders(chatRoomId, eventSeq, start) {
  const answers = Array.from(find(ANSWER).condition('chatRoomId', chatRoomId).condition('eventSeq', eventSeq).condition('chatMessageReminderId', 'is-not-null', null).list())
  for (const row of answers) {
    try {
      const reminder = find(REMINDER).condition('chatMessageReminderId', row.chatMessageReminderId).forUpdate(true).one()
      if (!reminder || reminder.statusEnumId !== 'CrmPending' || reminder.userId !== row.userId) continue
      const at = start - Number(row.remindMinutes || 0) * MINUTE
      if (at < now() + MINUTE) reminder.set('statusEnumId', 'CrmCancelled')
      else reminder.set('remindDate', new Date(at))
      reminder.update()
    } catch (err) {
      ec.logger.error(`Could not move the event reminder ${row.chatMessageReminderId}: ${message(err)}`)
    }
  }
}

/** cancel#CalEvent: its creator or a moderator; a cancellation message, the answers' reminders cancelled, the providers'. */
function cancel(p) {
  const out = { calEvent: null, changeEventSeq: null, reasonCode: null }
  const done = committed(() => recordCancel(p, out))
  if (!done) return out
  if (done.changed) {
    calendarCall(p.chatRoomId, Number(p.eventSeq), 'cancel')
    pushState(p.chatRoomId, Number(p.eventSeq))
  }
  out.calEvent = myState(p.chatRoomId, Number(p.eventSeq))
  return out
}

/** cancel#CalEvent's writes: the row, the cancellation message and the reminders cancelled. Out: {changed}, or null. */
function recordCancel(p, out) {
  const room = postable(p.chatRoomId)
  const fail = (fields) => { Object.assign(out, fields); return null }
  if (room.error) return fail({ reasonCode: room.reasonCode })
  const cal = lockedEvent(p.chatRoomId, p.eventSeq)
  if (!cal) return fail({ reasonCode: 'notFound' })
  if (!mayManage(cal, room.access)) return fail(refuse('notAllowed', localize('Only the event\'s creator or a moderator cancels it')))
  if (cal.cancelledDate) return { changed: false }
  cal.set('cancelledDate', ec.user.nowTimestamp)
  cal.set('cancelledByUserId', ec.user.userId)
  cal.update()
  const day = format(millis(cal.startDate), DATE, cal.timeZone)
  const changeSeq = postChange(p.chatRoomId, Number(p.eventSeq), [expand('Cancelled: ${title}, ${day}', { title: cal.title, day })],
    { eventSeq: Number(p.eventSeq), revision: Number(cal.revision || 0), cancelled: true })
  if (ec.message.hasError()) return null
  if (changeSeq != null) { cal.set('lastChangeEventSeq', changeSeq); cal.update() }
  out.changeEventSeq = changeSeq
  cancelReminders(Array.from(find(ANSWER).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).list()))
  return { changed: true }
}

/** get#CalEventIcs: the event as an .ics file for one who can read the room: PUBLISH, or CANCEL once cancelled. */
function ics(p) {
  const out = { fileText: null, fileName: null, contentType: 'text/calendar', reasonCode: null }
  const access = ec.service.sync().name(CHAT + 'get#RoomAccess').parameters({ chatRoomId: p.chatRoomId }).call()
  if (ec.message.hasError()) return out
  const cal = access && access.canRead ? find(CAL).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).one() : null
  if (!cal) return Object.assign(out, refuse('notFound', expand('Event ${seq} not found in the room', { seq: p.eventSeq })))
  if (ec.factory.getToolFactory('ICalendar') == null) return Object.assign(out, refuse('noIcs', localize('This server makes no calendar files')))
  const cancelled = !!cal.cancelledDate
  const event = { uid: uidOf(p.chatRoomId, Number(p.eventSeq)), sequence: Number(cal.revision || 0), summary: cal.title,
    description: cal.description || null, location: cal.place || null, status: cancelled ? 'CANCELLED' : 'CONFIRMED',
    timeZone: cal.timeZone, url: p.baseUrl ? String(p.baseUrl).replace(/\/+$/, '') + roomLink(p.chatRoomId, Number(p.eventSeq)) : null }
  if (cal.allDay === 'Y') {
    Object.assign(event, { allDay: true, start: null, end: null, startDate: format(millis(cal.startDate), 'yyyy-MM-dd', cal.timeZone),
      endDate: format(millis(cal.endDate), 'yyyy-MM-dd', cal.timeZone) })
  } else Object.assign(event, { allDay: false, start: millis(cal.startDate), end: millis(cal.endDate) })
  out.fileText = ec.factory.getTool('ICalendar').write({ method: cancelled ? 'CANCEL' : 'PUBLISH', timeZone: cal.timeZone, events: [event] })
  out.fileName = (words(cal.title).replace(/[\\/:*?"<>|\u0000-\u001f]+/g, ' ').trim().slice(0, 80) || 'event') + '.ics'
  return out
}

/**
 * forget#CalEvent: the message is gone (a redaction, retention, an expiry): the answers' pending reminders cancelled, the
 * rows removed, the providers told; each part on its own. The caller has checked access.
 */
function forget(p) {
  const cal = find(CAL).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).one()
  if (!cal) return { forgotten: false }
  const answers = Array.from(find(ANSWER).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).list())
  cancelReminders(answers)
  if (!cal.cancelledDate && answers.some((row) => row.calendarDate)) calendarCall(p.chatRoomId, Number(p.eventSeq), 'cancel')
  try {
    find(ANSWER).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).deleteAll()
    find(CAL).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).deleteAll()
  } catch (err) {
    ec.logger.error(`Could not remove event ${p.eventSeq} of room ${p.chatRoomId}: ${message(err)}`)
  }
  return { forgotten: true }
}

const operations = { create, states, answer, update, cancel, ics, forget, whenText: (p) => ({ text: whenText(p) }) }

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown chat event operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
