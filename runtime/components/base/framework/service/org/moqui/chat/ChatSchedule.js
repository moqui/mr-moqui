// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Scheduled sends and message reminders in chat (plans/Comms-Messaging.md §18.3 phase 15): the services of
// org.moqui.chat.ChatScheduleServices. The entities are framework/entity/ChatScheduleEntities.xml.
//
// - A scheduled message of a plain, phone or bridged room is held by the server (ChatScheduledMessage with its body) and
//   sent at its time by sendDueMessages as its sender (run#AsSender, chat_schedule.rs: the account as it is then, the
//   app's screens pushed again as the user, then send#Message), so every bridge and its send checks (check#SmsSend)
//   apply then. The clientRef sched:<id> makes it go once. Its text is cleared once it is sent or cancelled.
// - One of an encrypted room is a placeholder with no text: the sender's device (heldByDeviceId) keeps the draft sealed and
//   sends it at its time through sendHeld (send#HeldMessage, with the row locked and its claim checked) after claimHeld
//   gives it a lease. The job never sends it: it fails it when its device is revoked, or when it is long overdue (the
//   device was not open since).
// - A reminder (ChatMessageReminder) keeps the room and the message's number only. At its time (sendDueReminders) it is an
//   Activity row (CuaReminder), a ChatReminder notification and a push to each device (queue#ReminderPush, chat_push.rs).
//
// Each service acts on the signed-in user's own rows: another's is "not found". The jobs take each row on its own, in a
// transaction of its own: one that fails is logged and the rest go on. A sign-in lock of the account is a wait, never a
// failure.

const SCHEDULED = 'moqui.chat.ChatScheduledMessage'
const REMINDER = 'moqui.chat.ChatMessageReminder'
const ROOM = 'moqui.chat.ChatRoom'
const EVENT = 'moqui.chat.ChatEvent'
const MEMBER = 'moqui.chat.ChatRoomMember'
const ATTACHMENT = 'moqui.chat.ChatAttachment'
const DEVICE = 'moqui.chat.ChatDevice'
const ACCOUNT = 'moqui.security.UserAccount'
const SERVICES = 'org.moqui.chat.ChatScheduleServices.'
const CHAT = 'org.moqui.chat.ChatServices.'
const DAY = 86400 * 1000

/** The most messages a user has waiting, and reminders pending. */
const MAX_SCHEDULED = 100
const MAX_REMINDERS = 200
/** The earliest and latest times: a minute ahead (a few seconds of slack for the client's clock), a year ahead. */
const MIN_AHEAD_MS = 55 * 1000
const MAX_AHEAD_MS = 366 * DAY
/** A held message's lease, and how late a send is "sent late". */
const LEASE_MS = 2 * 60 * 1000
const LATE_MS = 5 * 60 * 1000
/** Tries of a message whose send failed for no reason of its own, before it fails. */
const MAX_ATTEMPTS = 5
/** Done rows (sent, cancelled, failed) are kept this long, a failed one with its text so the sender can resend it. */
const KEEP_DAYS = 7
const STATUS = { CsmScheduled: 'scheduled', CsmSent: 'sent', CsmFailed: 'failed', CsmCancelled: 'cancelled' }
/** The reasons a message was not sent, in the sender's words. */
const REASONS = {
  notMember: 'You are no longer in the room',
  archived: 'The room is archived',
  blocked: 'You blocked this conversation',
  accountDisabled: 'Your account is closed',
  noAccess: 'You no longer have the app',
  deviceGone: 'The draft was on a device that is gone',
  deviceAway: 'The device that keeps it has not been open since it was due',
  draftLost: 'The draft was lost on the device that held it',
  sendRefused: 'The message could not be sent',
  failed: 'The message could not be sent',
}

const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const blank = (value) => value == null || String(value).trim() === ''
const message = (err) => (err && err.message ? err.message : String(err))
const millis = (value) => (value == null || value === '' ? null : Number(value))
const now = () => Number(ec.user.nowTimestamp)
const property = (name, fallback) => { const value = Number(System.getProperty(name)); return value > 0 ? value : fallback }
/** A held message due longer than this is failed (deviceAway); a device unseen as long shows so in the lists. */
const awayDays = () => property('chat_held_overdue_days', 7)
/** The most messages of one sender a run of the job sends (a minute's): the rest wait for the next run. */
const perUserPerRun = () => property('chat_schedule_user_per_run', 20)
const appRoot = () => (System.getProperty('comms_app_root') || '/qapps/BasicComms').replace(/\/+$/, '')
const roomLink = (chatRoomId, eventSeq) => `${appRoot()}/Chat?chatRoomId=${encodeURIComponent(chatRoomId)}${eventSeq != null ? '&eventSeq=' + eventSeq : ''}`
const parseList = (text) => { try { const value = text ? JSON.parse(text) : []; return Array.isArray(value) ? value : [] } catch (err) { return [] } }

function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

/** The text of the composer's HTML, as send#Message keeps it. */
function textOf(html) {
  return String(html || '')
    .replace(/<br\s*\/?>/gi, '\n')
    .replace(/<\/(p|li|h[1-6]|blockquote|pre|div)>/gi, '\n')
    .replace(/<[^>]*>/g, '')
    .replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&nbsp;/g, ' ').replace(/&amp;/g, '&')
    .replace(/[ \t]+/g, ' ').replace(/ *\n */g, '\n').replace(/\n{3,}/g, '\n\n').trim()
}

/** Whether `at` (millis) is a time a message or reminder may be set for. */
function badTime(at) {
  const ahead = at == null || Number.isNaN(at) ? NaN : at - now()
  return !(ahead >= MIN_AHEAD_MS && ahead <= MAX_AHEAD_MS)
}

/** The screens of this request, root first (the app the user scheduled through), as JSON; null outside a screen. */
function screensNow() {
  const frames = Array.from(ec.artifactExecution.getStack() || []).filter((frame) => frame && frame.typeEnumId === 'AT_XML_SCREEN')
    .map((frame) => String(frame.name)).reverse().slice(0, 20)
  return frames.length ? JSON.stringify(frames) : null
}

/**
 * A ChatServices service run as userId at the job's time (run#AsSender): {state: ok | closed | locked, reasonCode
 * (noAccess: the app's grant is gone), result, errors}. The account's sign-in checks do not apply (a password change);
 * a sign-in lock is `locked`, which waits.
 */
function asSender(userId, service, parameters, screenPathJson) {
  const out = ec.service.sync().name(SERVICES + 'run#AsSender').parameters({ userId, serviceName: CHAT + service, parameters,
    screenList: parseList(screenPathJson), nowMillis: String(now()) }).disableAuthz().call() || {}
  return { state: out.accountState || 'closed', reasonCode: out.reasonCode || null, result: out.result || {}, errors: out.errors || '' }
}

/** fn with the language and zone of userId when the job has no user (the texts of a notice), back to none after. */
function inLanguageOf(userId, fn) {
  if (ec.user.userId) return fn()
  const account = one(ACCOUNT, { userId })
  try {
    if (account && account.locale) ec.user.setLocale(account.locale)
    if (account && account.timeZone) ec.user.setTimeZone(account.timeZone)
    return fn()
  } finally {
    ec.user.setLocale(null)
    ec.user.setTimeZone(ec.user.machineTimeZone)
  }
}

/** Tell the user's open Chats that their scheduled messages or reminders changed (after the commit). */
function told(userId, chatRoomId, what) {
  try {
    ec.service.sync().name(CHAT + 'push#Signal').parameters({ topic: 'chatUser:' + userId, userIdList: [userId],
      message: { kind: 'schedule', chatRoomId: chatRoomId || null, what } }).call()
  } catch (err) {
    ec.logger.warn(`The schedule change of ${userId} in room ${chatRoomId} was not pushed: ${message(err)}`)
  }
}

/** The caller's row of `entity` by id, locked unless `unlocked`, or null ("not found"). */
function mine(entity, idField, id, userField, unlocked) {
  const me = ec.user.userId
  if (!me || blank(id)) return null
  const row = find(entity).condition(idField, String(id)).forUpdate(!unlocked).one()
  return row && row[userField] === me ? row : null
}

/** The device of `userId` named `deviceId` that is not revoked, or null. */
function liveDevice(userId, deviceId) {
  if (blank(deviceId)) return null
  const device = one(DEVICE, { userId, deviceId: String(deviceId) })
  return device && !device.revokedDate ? device : null
}

/** Whether a live device was not seen for awayDays (a cleared browser's device is never seen again). */
function deviceAway(device) {
  const seen = millis(device && (device.lastSeenDate || device.createdDate))
  return !!device && seen != null && now() - seen > awayDays() * DAY
}

/** The records [{entity, id}] of a message, as JSON, at most 20. */
function recordsJsonOf(list) {
  const records = Array.from(list || []).filter((record) => record && record.entity && record.id).slice(0, 20)
    .map((record) => ({ entity: String(record.entity), id: String(record.id) }))
  return records.length ? JSON.stringify(records) : null
}

/** A row done: sent or cancelled keeps no text. */
function done(row, fields) {
  row.setFields(Object.assign({ body: null, bodyText: null, recordsJson: null, leaseDeviceId: null, leaseExpiresDate: null, reviewNeeded: null }, fields),
    true, null, null)
  row.update()
  told(row.senderUserId, row.chatRoomId, 'scheduled')
}

// ---------- scheduled messages ----------

/** schedule#Message: a message the server holds, or an encrypted room's placeholder its device holds. */
function schedule(p) {
  const me = ec.user.userId
  if (!me) return refuse('notFound', 'Sign in to schedule a message')
  const access = ec.service.sync().name(CHAT + 'get#RoomAccess').parameters({ chatRoomId: p.chatRoomId }).call() || {}
  if (ec.message.hasError()) return {}
  if (!access.canRead) return refuse('notFound', 'Chat room not found')
  if (!access.isMember) return refuse('notMember', 'Join the room to post in it')
  if (access.isArchived) return refuse('archived', 'The room is archived')
  const sendAt = millis(p.sendDate)
  if (badTime(sendAt)) return refuse('badTime', 'Pick a time at least a minute from now and within a year')
  const waiting = find(SCHEDULED).condition('senderUserId', me).condition('statusEnumId', 'CsmScheduled').count()
  if (waiting >= MAX_SCHEDULED) return refuse('tooMany', 'You have 100 scheduled messages: send or cancel some first')
  const encrypted = !!(access.room && (access.room.encrypted === true || access.room.encrypted === 'Y'))
  const row = ec.entity.makeValue(SCHEDULED)
  row.setSequencedIdPrimary()
  const fields = { chatRoomId: p.chatRoomId, senderUserId: me, sendDate: new Date(sendAt), statusEnumId: 'CsmScheduled', attemptCount: 0,
    replyToEventSeq: p.replyToEventSeq ?? null, threadRootEventSeq: p.threadRootEventSeq ?? null, createdDate: ec.user.nowTimestamp,
    screenPathJson: screensNow() }
  const ids = Array.from(new Set(Array.from(p.chatAttachmentIdList || []).map((id) => String(id || '').trim()).filter((id) => id)))
  if (encrypted) {
    // the sender's own device keeps the draft and sends it: the server keeps no text
    if (!blank(p.body) || ids.length || (p.recordList && p.recordList.length)) {
      return refuse('encryptedRoom', 'In an encrypted room the device that schedules a message keeps it: only its time is sent here')
    }
    const device = liveDevice(me, p.heldByDeviceId)
    if (!device) return refuse('noDevice', 'This browser has no encryption device to keep the message')
    fields.heldByDeviceId = device.deviceId
    fields.heldByDeviceName = device.displayName || device.userAgent || device.deviceId
  } else {
    if (!blank(p.heldByDeviceId)) return refuse('notEncrypted', 'This room is not encrypted')
    const html = p.body ? StringUtilities.cleanChatHtml(String(p.body)).trim() : ''
    if (html.length > 65536) return refuse('tooLong', 'The message is too long')
    if (!textOf(html) && !/<img\b[^>]*\bsrc=/i.test(html) && !ids.length) return refuse('empty', 'A message needs text or a file')
    fields.body = html || null
    fields.bodyText = textOf(html) || null
    fields.recordsJson = recordsJsonOf(p.recordList)
  }
  for (const seq of [p.replyToEventSeq, p.threadRootEventSeq]) {
    if (seq == null) continue
    const target = one(EVENT, { chatRoomId: p.chatRoomId, eventSeq: Number(seq) })
    if (!target || target.eventTypeEnumId !== 'CetMessage') return refuse('notFound', 'That message is not in the room')
  }
  row.setFields(fields, true, null, null)
  row.create()
  // its files: the sender's uploads of the room not yet sent with a message, nor on another scheduled one
  for (const id of ids) {
    const file = find(ATTACHMENT).condition('chatAttachmentId', id).forUpdate(true).one()
    if (!file || file.chatRoomId !== p.chatRoomId || file.uploadedByUserId !== me || file.eventSeq != null || file.chatScheduledMessageId) {
      return refuse('notFound', 'A file of the message was not found')
    }
    file.set('chatScheduledMessageId', row.chatScheduledMessageId)
    file.update()
  }
  told(me, p.chatRoomId, 'scheduled')
  return { chatScheduledMessageId: row.chatScheduledMessageId, scheduled: listed(row, {}) }
}

/** A scheduled message as the lists show it. */
function listed(row, devices) {
  const files = row.heldByDeviceId ? [] : Array.from(find(ATTACHMENT).condition('chatScheduledMessageId', row.chatScheduledMessageId)
    .selectField('chatAttachmentId').selectField('fileName').selectField('contentType').selectField('fileSize').orderBy('chatAttachmentId').list())
    .map((file) => ({ chatAttachmentId: file.chatAttachmentId, fileName: file.fileName ?? null, contentType: file.contentType ?? null,
      fileSize: file.fileSize != null ? Number(file.fileSize) : null }))
  const held = !!row.heldByDeviceId
  const device = held ? (row.heldByDeviceId in devices ? devices[row.heldByDeviceId] : liveDevice(row.senderUserId, row.heldByDeviceId)) : null
  const waiting = row.statusEnumId === 'CsmScheduled'
  return { chatScheduledMessageId: row.chatScheduledMessageId, chatRoomId: row.chatRoomId, sendDate: millis(row.sendDate),
    status: STATUS[row.statusEnumId] || null, body: row.body ?? null, bodyText: row.bodyText ?? null,
    replyToEventSeq: row.replyToEventSeq != null ? Number(row.replyToEventSeq) : null,
    threadRootEventSeq: row.threadRootEventSeq != null ? Number(row.threadRootEventSeq) : null, files,
    held, heldByDeviceId: row.heldByDeviceId ?? null, heldByDeviceName: row.heldByDeviceName ?? null,
    deviceGone: held && waiting && !device, deviceAway: held && waiting && deviceAway(device), reviewNeeded: row.reviewNeeded === 'Y',
    sentLate: row.sentLate === 'Y', sentAfterCancel: row.sentAfterCancel === 'Y', sentDate: millis(row.sentDate),
    sentEventSeq: row.sentEventSeq != null ? Number(row.sentEventSeq) : null, attemptCount: Number(row.attemptCount || 0),
    reasonCode: row.reasonCode ?? null, failText: row.failText ?? null }
}

/** update#ScheduledMessage: a new body (with its record cards) or time, while it waits. A held one: its time only, from its device. */
function update(p) {
  const row = mine(SCHEDULED, 'chatScheduledMessageId', p.chatScheduledMessageId, 'senderUserId')
  if (!row) return refuse('notFound', 'Scheduled message not found')
  if (row.statusEnumId !== 'CsmScheduled') return refuse('notScheduled', 'This message was sent or is no longer scheduled')
  if (row.heldByDeviceId) {
    if (String(p.deviceId || '') !== row.heldByDeviceId) return refuse('notHolder', 'Only the device that keeps this message can change it')
    if (!blank(p.body)) return refuse('encryptedRoom', 'In an encrypted room the device that schedules a message keeps it: only its time is sent here')
  }
  if (p.sendDate != null && p.sendDate !== '') {
    const at = millis(p.sendDate)
    if (badTime(at)) return refuse('badTime', 'Pick a time at least a minute from now and within a year')
    row.set('sendDate', new Date(at))
  }
  if (!row.heldByDeviceId && p.body != null) {
    const html = StringUtilities.cleanChatHtml(String(p.body)).trim()
    if (html.length > 65536) return refuse('tooLong', 'The message is too long')
    const hasFiles = find(ATTACHMENT).condition('chatScheduledMessageId', row.chatScheduledMessageId).count() > 0
    if (!textOf(html) && !/<img\b[^>]*\bsrc=/i.test(html) && !hasFiles) return refuse('empty', 'A message needs text or a file')
    row.set('body', html || null)
    row.set('bodyText', textOf(html) || null)
    // the cards follow the text
    row.set('recordsJson', recordsJsonOf(p.recordList))
  }
  row.update()
  told(row.senderUserId, row.chatRoomId, 'scheduled')
  return { scheduled: listed(row, {}) }
}

/** Remove the files of a scheduled message, each on its own. */
function removeFiles(row) {
  for (const file of find(ATTACHMENT).condition('chatScheduledMessageId', row.chatScheduledMessageId).condition('eventSeq', null).list()) {
    try {
      ec.service.sync().name(CHAT + 'remove#AttachmentFile').parameter('chatAttachmentId', file.chatAttachmentId).disableAuthz().call()
      file.delete()
    } catch (err) {
      ec.logger.error(`Could not remove chat file ${file.chatAttachmentId} of scheduled message ${row.chatScheduledMessageId}: ${message(err)}`)
    }
  }
}

/** cancel#ScheduledMessage: from any device of the sender; a held one's draft goes from its device when it next looks. */
function cancel(p) {
  const row = mine(SCHEDULED, 'chatScheduledMessageId', p.chatScheduledMessageId, 'senderUserId')
  if (!row) return refuse('notFound', 'Scheduled message not found')
  if (row.statusEnumId !== 'CsmScheduled' && row.statusEnumId !== 'CsmFailed') return refuse('notScheduled', 'This message was sent or is no longer scheduled')
  if (row.leaseDeviceId && millis(row.leaseExpiresDate) > now()) return refuse('sending', 'This message is being sent now')
  done(row, { statusEnumId: 'CsmCancelled' })
  removeFiles(row)
  return {}
}

/** dismiss#ScheduledMessage: the sender saw that it went late, or after a cancel. */
function dismiss(p) {
  const row = mine(SCHEDULED, 'chatScheduledMessageId', p.chatScheduledMessageId, 'senderUserId')
  if (!row) return refuse('notFound', 'Scheduled message not found')
  if ((row.sentLate === 'Y' || row.sentAfterCancel === 'Y') && !row.lateSeenDate) {
    row.set('lateSeenDate', ec.user.nowTimestamp)
    row.update()
    told(row.senderUserId, row.chatRoomId, 'scheduled')
  }
  return {}
}

/** send#ScheduledMessageNow: the server's send of one of the sender's waiting or failed messages, at once. */
function sendNow(p) {
  // not locked here: the send locks it in a transaction of its own
  const row = mine(SCHEDULED, 'chatScheduledMessageId', p.chatScheduledMessageId, 'senderUserId', true)
  if (!row) return refuse('notFound', 'Scheduled message not found')
  if (row.heldByDeviceId) return refuse('notHolder', 'The device that keeps this message sends it')
  if (row.statusEnumId !== 'CsmScheduled' && row.statusEnumId !== 'CsmFailed') return refuse('notScheduled', 'This message was sent or is no longer scheduled')
  const result = deliverOne(row.chatScheduledMessageId, true)
  if (result.skipped) return refuse('notScheduled', 'This message was sent or is no longer scheduled')
  if (result.waiting) return refuse('locked', 'Your account is locked for a while: the message waits and goes after')
  if (result.failed || result.reasonCode) return refuse(result.reasonCode || 'failed', result.failText || REASONS[result.reasonCode] || REASONS.failed)
  return { eventSeq: result.eventSeq ?? null }
}

/**
 * deliver#ScheduledMessage (its own transaction): the send of a message the server holds, as its sender. Out: eventSeq;
 * waiting (a sign-in lock: nothing done, nothing counted); or reasonCode and failText with an error (the send rolled back;
 * the caller fails the row).
 */
function deliver(p) {
  const row = find(SCHEDULED).condition('chatScheduledMessageId', p.chatScheduledMessageId).forUpdate(true).one()
  // Send now takes a failed one again
  const waiting = row && (row.statusEnumId === 'CsmScheduled' || (p.atOnce && row.statusEnumId === 'CsmFailed'))
  if (!waiting || row.heldByDeviceId) return { skipped: true }
  if (!p.atOnce && millis(row.sendDate) > now() + 1000) return { skipped: true }
  const refused = (code, text) => {
    ec.message.addError(text || ec.l10n.localize(REASONS[code] || REASONS.failed))
    return { reasonCode: code, failText: text || null }
  }
  const access = asSender(row.senderUserId, 'get#RoomAccess', { chatRoomId: row.chatRoomId }, row.screenPathJson)
  if (access.state === 'locked') return { waiting: true }
  if (access.state !== 'ok') return refused('accountDisabled')
  if (access.reasonCode === 'noAccess') return refused('noAccess')
  if (access.errors) return refused('sendRefused', access.errors)
  if (!access.result.canRead || !access.result.isMember) return refused('notMember')
  if (access.result.isArchived) return refused('archived')
  // the sender blocked the room's number or person since: it does not go (D1)
  const member = one(MEMBER, { chatRoomId: row.chatRoomId, userId: row.senderUserId })
  if (member && member.filterEnumId === 'CmfBlocked') return refused('blocked')
  const files = Array.from(find(ATTACHMENT).condition('chatScheduledMessageId', row.chatScheduledMessageId).condition('eventSeq', null)
    .orderBy('chatAttachmentId').list()).map((file) => file.chatAttachmentId)
  const sent = asSender(row.senderUserId, 'send#Message', { chatRoomId: row.chatRoomId, body: row.body ?? null,
    replyToEventSeq: row.replyToEventSeq != null ? Number(row.replyToEventSeq) : null,
    threadRootEventSeq: row.threadRootEventSeq != null ? Number(row.threadRootEventSeq) : null,
    chatAttachmentIdList: files, recordList: parseList(row.recordsJson), roomUrl: roomLink(row.chatRoomId), clientRef: 'sched:' + row.chatScheduledMessageId },
  row.screenPathJson)
  if (sent.state === 'locked') return { waiting: true }
  if (sent.errors || sent.result.eventSeq == null) return refused(sent.result.reasonCode || 'sendRefused', sent.errors || null)
  done(row, { statusEnumId: 'CsmSent', sentEventSeq: sent.result.eventSeq, sentDate: ec.user.nowTimestamp, attemptCount: Number(row.attemptCount || 0) + 1,
    reasonCode: null, failText: null })
  return { eventSeq: sent.result.eventSeq }
}

/** The sender's notice of a message that was not sent; one that fails is logged. */
function notifyFailed(row) {
  try {
    const roomRow = one(ROOM, { chatRoomId: row.chatRoomId })
    const name = roomRow && roomRow.roomName ? roomRow.roomName : null
    const title = inLanguageOf(row.senderUserId, () => {
      const why = ec.l10n.localize(REASONS[row.reasonCode] || REASONS.failed)
      return name ? ec.l10n.localize('Your scheduled message in ${roomName} was not sent: ${reason}').replace('${roomName}', name).replace('${reason}', why)
        : ec.l10n.localize('Your scheduled message was not sent: ${reason}').replace('${reason}', why)
    })
    ec.makeNotificationMessage().topic('ChatScheduleFailed').userIds([row.senderUserId]).message({ title, link: roomLink(row.chatRoomId),
      chatRoomId: row.chatRoomId, chatScheduledMessageId: row.chatScheduledMessageId, reasonCode: row.reasonCode }).record(ROOM, row.chatRoomId).send()
  } catch (err) {
    ec.logger.error(`The sender of scheduled message ${row.chatScheduledMessageId} was not told it was not sent: ${message(err)}`)
  }
}

/** Fail a row: CsmFailed with the reason, no lease, the sender told and notified. */
function failRow(row, reasonCode, failText) {
  row.setFields({ statusEnumId: 'CsmFailed', reasonCode: reasonCode || 'failed', failText: failText ? String(failText).slice(0, 255) : null,
    leaseDeviceId: null, leaseExpiresDate: null, reviewNeeded: null }, true, null, null)
  row.update()
  told(row.senderUserId, row.chatRoomId, 'scheduled')
  notifyFailed(row)
}

/**
 * fail#ScheduledMessage (its own transaction): a send that did not go. With a reasonCode it is CsmFailed and the sender is
 * told; without one (an error of no reason of its own) it is tried again at the next run, MAX_ATTEMPTS in all. Out: failed.
 */
function fail(p) {
  const row = find(SCHEDULED).condition('chatScheduledMessageId', p.chatScheduledMessageId).forUpdate(true).one()
  if (!row || (row.statusEnumId !== 'CsmScheduled' && row.statusEnumId !== 'CsmFailed')) return { failed: false }
  const attempts = Number(row.attemptCount || 0) + 1
  if (blank(p.reasonCode) && attempts < MAX_ATTEMPTS) {
    row.setFields({ attemptCount: attempts, failText: p.failText ? String(p.failText).slice(0, 255) : null }, true, null, null)
    row.update()
    return { failed: false }
  }
  row.set('attemptCount', attempts)
  failRow(row, p.reasonCode || 'failed', p.failText)
  return { failed: true }
}

/** One row's send in its own transaction, then its failure in another. Out: {sent, eventSeq}, {waiting}, {skipped} or {failed, reasonCode}. */
function deliverOne(id, sendAtOnce) {
  let result = {}
  let thrown = null
  ec.message.pushErrors()
  try {
    result = ec.service.sync().name(SERVICES + 'deliver#ScheduledMessage').parameters({ chatScheduledMessageId: id, atOnce: !!sendAtOnce })
      .requireNewTransaction(true).disableAuthz().call() || {}
    if (ec.message.hasError() && !result.reasonCode) thrown = ec.message.errorsString
  } catch (err) {
    thrown = message(err)
  }
  ec.message.clearErrors()
  ec.message.popErrors()
  if (result.skipped) return { skipped: true }
  if (result.waiting && !thrown) return { waiting: true }
  if (!result.reasonCode && !thrown) return { sent: true, eventSeq: result.eventSeq }
  if (thrown) ec.logger.error(`Could not send scheduled chat message ${id}: ${thrown}`)
  return Object.assign(failInOwn(id, result.reasonCode || null, result.failText || thrown || null), { reasonCode: result.reasonCode || null,
    failText: result.failText || null })
}

/** fail#ScheduledMessage in a transaction of its own; one that fails is logged. */
function failInOwn(id, reasonCode, failText) {
  let failed = false
  ec.message.pushErrors()
  try {
    failed = !!(ec.service.sync().name(SERVICES + 'fail#ScheduledMessage').parameters({ chatScheduledMessageId: id, reasonCode, failText })
      .requireNewTransaction(true).disableAuthz().call() || {}).failed
  } catch (err) {
    ec.logger.error(`Could not record the failed send of scheduled chat message ${id}: ${message(err)}`)
  }
  ec.message.clearErrors()
  ec.message.popErrors()
  return { failed }
}

/**
 * send#DueMessages (the job): the server's due messages, oldest first, each on its own, at most perUserPerRun of a sender
 * a run; then the held ones the job ends: those of a device revoked since, and those overdue past awayDays.
 */
function sendDueMessages(p) {
  const limit = p.limit > 0 ? Number(p.limit) : 200
  const budget = Math.max(1, Number(p.timeBudgetSeconds ?? 45)) * 1000
  const cap = perUserPerRun()
  const started = Date.now()
  const cf = ec.entity.conditionFactory
  let sentCount = 0, failedCount = 0, left = 0
  // the messages the server holds: held ones never stand in their way
  const due = Array.from(find(SCHEDULED).condition('statusEnumId', 'CsmScheduled').condition('heldByDeviceId', null)
    .condition(cf.makeCondition('sendDate', 'less-equals', ec.user.nowTimestamp))
    .selectField('chatScheduledMessageId').selectField('senderUserId').orderBy('sendDate').orderBy('chatScheduledMessageId').limit(limit).list())
  const bySender = {}
  for (const row of due) {
    if (Date.now() - started > budget) { left++; continue }
    if ((bySender[row.senderUserId] || 0) >= cap) { left++; continue }
    bySender[row.senderUserId] = (bySender[row.senderUserId] || 0) + 1
    const result = deliverOne(row.chatScheduledMessageId, false)
    if (result.sent) sentCount++
    else if (result.failed) failedCount++
  }
  if (left) ec.logger.info(`${left} due chat messages wait for the next run of send#DueMessages`)
  // the held ones: a revoked device's (any time), and those due longer than awayDays (their device was not open since)
  const ends = []
  for (const row of find(SCHEDULED).condition('statusEnumId', 'CsmScheduled').condition('heldByDeviceId', 'not-equals', null)
    .selectField('senderUserId').selectField('heldByDeviceId').distinct(true).limit(1000).list()) {
    if (liveDevice(row.senderUserId, row.heldByDeviceId)) continue
    for (const gone of find(SCHEDULED).condition({ statusEnumId: 'CsmScheduled', senderUserId: row.senderUserId, heldByDeviceId: row.heldByDeviceId })
      .selectField('chatScheduledMessageId').limit(200).list()) ends.push([gone.chatScheduledMessageId, 'deviceGone'])
  }
  for (const row of find(SCHEDULED).condition('statusEnumId', 'CsmScheduled').condition('heldByDeviceId', 'not-equals', null)
    .condition(cf.makeCondition('sendDate', 'less-equals', new Date(now() - awayDays() * DAY))).selectField('chatScheduledMessageId')
    .orderBy('sendDate').limit(200).list()) ends.push([row.chatScheduledMessageId, 'deviceAway'])
  const ended = new Set()
  for (const [id, reason] of ends) {
    if (ended.has(id)) continue
    ended.add(id)
    if (failInOwn(id, reason, null).failed) failedCount++
  }
  return { sentCount, failedCount }
}

// ---------- an encrypted room's message, held by its device ----------

/** The room's message the sender's device sent for row (its clientRef sched:<id>), or null. */
function sentFor(row) {
  const list = find(EVENT).condition('chatRoomId', row.chatRoomId).condition('senderUserId', row.senderUserId)
    .condition('clientRef', 'sched:' + row.chatScheduledMessageId).condition('eventTypeEnumId', 'CetMessage').limit(1).list()
  return list.length ? list[0] : null
}

/** A held row whose message went: sent (late, or after a cancel, as it was). */
function markSent(row, eventSeq) {
  const at = now()
  done(row, { statusEnumId: 'CsmSent', sentEventSeq: Number(eventSeq), sentDate: new Date(at), reasonCode: null, failText: null,
    sentLate: at - millis(row.sendDate) > LATE_MS ? 'Y' : null, sentAfterCancel: row.statusEnumId === 'CsmCancelled' ? 'Y' : null })
}

/** The caller's held row by id, locked, with its message if it went; null when not theirs or not held. */
function heldRow(id) {
  const row = mine(SCHEDULED, 'chatScheduledMessageId', id, 'senderUserId')
  return row && row.heldByDeviceId ? row : null
}

/**
 * claim#HeldMessage: the holding device takes a held message that is due (or Send now) to send it. A message that went
 * already (a lost report, before a revoke or a cancel) is marked sent, whatever the row's status. A failed one is not
 * taken again: its draft goes from the device, so it cannot be sent.
 */
function claimHeld(p) {
  const row = heldRow(p.chatScheduledMessageId)
  if (!row) return refuse('notFound', 'Scheduled message not found')
  if (row.statusEnumId === 'CsmSent') return { claimed: false, sent: true }
  const made = sentFor(row)
  if (made) { markSent(row, made.eventSeq); return { claimed: false, sent: true } }
  if (row.statusEnumId !== 'CsmScheduled') return { claimed: false, reasonCode: row.reasonCode || STATUS[row.statusEnumId] || null }
  if (String(p.deviceId || '') !== row.heldByDeviceId || !liveDevice(row.senderUserId, row.heldByDeviceId)) {
    return refuse('notHolder', 'Only the device that keeps this message sends it')
  }
  const at = now()
  if (!p.atOnce && millis(row.sendDate) > at + 1000) return { claimed: false }
  // waiting for the sender's decision: only their Review and send takes it
  if (row.reviewNeeded === 'Y' && !p.atOnce) return { claimed: false, reasonCode: 'reviewNeeded' }
  if (row.leaseDeviceId && millis(row.leaseExpiresDate) > at) return { claimed: false }
  const access = ec.service.sync().name(CHAT + 'get#RoomAccess').parameters({ chatRoomId: row.chatRoomId }).call() || {}
  if (ec.message.hasError()) return {}
  const member = one(MEMBER, { chatRoomId: row.chatRoomId, userId: row.senderUserId })
  const reason = !access.canRead || !access.isMember ? 'notMember' : access.isArchived ? 'archived' : member && member.filterEnumId === 'CmfBlocked' ? 'blocked' : null
  if (reason) {
    failRow(row, reason, null)
    return { claimed: false, reasonCode: reason }
  }
  row.setFields({ leaseDeviceId: row.heldByDeviceId, leaseExpiresDate: new Date(at + LEASE_MS), reviewNeeded: null }, true, null, null)
  row.update()
  return { claimed: true, clientRef: 'sched:' + row.chatScheduledMessageId }
}

/**
 * send#HeldMessage: the holding device's encrypted send of a held message it claimed, with the row locked: refused
 * (notClaimed) unless it still waits, claimed by this device with its lease live, so a message cancelled (or taken by
 * another try) meanwhile does not go; then sent through send#Message with clientRef sched:<id> and marked sent in the
 * same transaction.
 */
function sendHeld(p) {
  const row = heldRow(p.chatScheduledMessageId)
  if (!row) return refuse('notFound', 'Scheduled message not found')
  if (row.statusEnumId !== 'CsmScheduled' || row.leaseDeviceId !== String(p.deviceId || '') || !(millis(row.leaseExpiresDate) > now())) {
    return refuse('notClaimed', 'This message is no longer waiting to be sent from this device')
  }
  const out = ec.service.sync().name(CHAT + 'send#Message').parameters({ chatRoomId: row.chatRoomId, encrypted: p.encrypted || null,
    mentionUserIdList: p.mentionUserIdList || null, replyToEventSeq: row.replyToEventSeq != null ? Number(row.replyToEventSeq) : null,
    threadRootEventSeq: row.threadRootEventSeq != null ? Number(row.threadRootEventSeq) : null, roomUrl: roomLink(row.chatRoomId),
    clientRef: 'sched:' + row.chatScheduledMessageId }).call() || {}
  if (ec.message.hasError()) return { reasonCode: out.reasonCode || 'sendRefused' }
  markSent(row, out.eventSeq)
  return { eventSeq: out.eventSeq, event: out.event || null, sentLate: row.sentLate === 'Y' }
}

/**
 * complete#HeldMessage: the holding device's report of a message it sent (eventSeq), or that its draft is lost. A message
 * that went is marked sent whatever the row's status (after a cancel, it says so).
 */
function completeHeld(p) {
  const row = heldRow(p.chatScheduledMessageId)
  if (!row) return refuse('notFound', 'Scheduled message not found')
  if (row.statusEnumId === 'CsmSent') return { sent: true }
  const made = sentFor(row)
  if (made && (p.eventSeq == null || Number(made.eventSeq) === Number(p.eventSeq))) {
    markSent(row, made.eventSeq)
    return { sent: true, sentLate: row.sentLate === 'Y' }
  }
  if (row.statusEnumId !== 'CsmScheduled') return refuse('notScheduled', 'This message was sent or is no longer scheduled')
  if (String(p.deviceId || '') !== row.heldByDeviceId) return refuse('notHolder', 'Only the device that keeps this message sends it')
  if (p.draftLost === true || p.draftLost === 'true') {
    failRow(row, 'draftLost', null)
    return { sent: false }
  }
  return refuse('notFound', 'The message this one made was not found')
}

/**
 * report#HeldFailure: the holding device could not send a held message. review: it waits for the sender's decision (a
 * verified person's new device or identity), not sent by itself meanwhile and not counted. Otherwise it is one try more,
 * and after MAX_ATTEMPTS it fails with the reason and the sender is told.
 */
function reportHeld(p) {
  const row = heldRow(p.chatScheduledMessageId)
  if (!row) return refuse('notFound', 'Scheduled message not found')
  if (row.statusEnumId !== 'CsmScheduled') return { failed: false }
  if (String(p.deviceId || '') !== row.heldByDeviceId) return refuse('notHolder', 'Only the device that keeps this message sends it')
  if (p.review === true || p.review === 'true') {
    row.setFields({ reviewNeeded: 'Y', leaseDeviceId: null, leaseExpiresDate: null }, true, null, null)
    row.update()
    told(row.senderUserId, row.chatRoomId, 'scheduled')
    return { failed: false, reviewNeeded: true }
  }
  const attempts = Number(row.attemptCount || 0) + 1
  const text = p.failText ? String(p.failText).slice(0, 255) : null
  if (attempts < MAX_ATTEMPTS) {
    row.setFields({ attemptCount: attempts, failText: text, leaseDeviceId: null, leaseExpiresDate: null }, true, null, null)
    row.update()
    return { failed: false }
  }
  row.set('attemptCount', attempts)
  failRow(row, blank(p.reasonCode) ? 'sendRefused' : String(p.reasonCode).slice(0, 60), text)
  return { failed: true }
}

// ---------- reminders ----------

/** set#Reminder: on a message of a room the caller can read now; it replaces their pending one of the message. */
function setReminder(p) {
  const me = ec.user.userId
  if (!me) return refuse('notFound', 'Sign in to set a reminder')
  const access = ec.service.sync().name(CHAT + 'get#RoomAccess').parameters({ chatRoomId: p.chatRoomId }).call() || {}
  if (ec.message.hasError()) return {}
  const event = access.canRead && p.eventSeq != null ? one(EVENT, { chatRoomId: p.chatRoomId, eventSeq: Number(p.eventSeq) }) : null
  if (!event || event.eventTypeEnumId !== 'CetMessage' || event.redactedDate) return refuse('notFound', 'Message not found')
  const at = millis(p.remindDate)
  if (badTime(at)) return refuse('badTime', 'Pick a time at least a minute from now and within a year')
  const pending = find(REMINDER).condition('userId', me).condition('chatRoomId', p.chatRoomId).condition('eventSeq', Number(p.eventSeq))
    .condition('statusEnumId', 'CrmPending').forUpdate(true).list()
  let row = pending.length ? pending[0] : null
  if (row) {
    row.setFields({ remindDate: new Date(at), screenPathJson: screensNow() }, true, null, null)
    row.update()
  } else {
    if (find(REMINDER).condition('userId', me).condition('statusEnumId', 'CrmPending').count() >= MAX_REMINDERS) {
      return refuse('tooMany', 'You have 200 reminders: cancel some first')
    }
    row = ec.entity.makeValue(REMINDER)
    row.setSequencedIdPrimary()
    row.setFields({ userId: me, chatRoomId: p.chatRoomId, eventSeq: Number(p.eventSeq), remindDate: new Date(at), statusEnumId: 'CrmPending',
      screenPathJson: screensNow(), createdDate: ec.user.nowTimestamp }, true, null, null)
    row.create()
  }
  told(me, p.chatRoomId, 'reminder')
  return { chatMessageReminderId: row.chatMessageReminderId, remindDate: at }
}

/** cancel#Reminder */
function cancelReminder(p) {
  const row = mine(REMINDER, 'chatMessageReminderId', p.chatMessageReminderId, 'userId')
  if (!row) return refuse('notFound', 'Reminder not found')
  if (row.statusEnumId === 'CrmPending') {
    row.set('statusEnumId', 'CrmCancelled')
    row.update()
    told(row.userId, row.chatRoomId, 'reminder')
  }
  return {}
}

/** The name of the sender of `event`: a local user's, else its actor's. */
function senderName(event) {
  if (event.senderUserId) {
    const account = find(ACCOUNT).condition('userId', event.senderUserId).selectField('userFullName').selectField('username').one()
    return account ? account.userFullName || account.username || event.senderUserId : event.senderUserId
  }
  if (event.senderActorId) {
    const actor = (ec.service.sync().name(CHAT + 'get#ActorNames').parameter('chatActorIdList', [event.senderActorId]).disableAuthz().call()?.actorMap || {})[event.senderActorId]
    if (actor) return actor.name
  }
  return ''
}

/** fire#Reminder (its own transaction): one reminder that is due. Out: fired, dropped, waiting (a sign-in lock). */
function fire(p) {
  const row = find(REMINDER).condition('chatMessageReminderId', p.chatMessageReminderId).forUpdate(true).one()
  if (!row || row.statusEnumId !== 'CrmPending' || millis(row.remindDate) > now() + 1000) return {}
  const drop = (reasonCode) => {
    row.setFields({ statusEnumId: 'CrmDropped', reasonCode }, true, null, null)
    row.update()
    told(row.userId, row.chatRoomId, 'reminder')
    return { dropped: true }
  }
  const event = one(EVENT, { chatRoomId: row.chatRoomId, eventSeq: Number(row.eventSeq) })
  if (!event || event.eventTypeEnumId !== 'CetMessage' || event.redactedDate) return drop('messageGone')
  // its user's access now: the account, the app's screens, the room
  const access = asSender(row.userId, 'get#RoomAccess', { chatRoomId: row.chatRoomId }, row.screenPathJson)
  if (access.state === 'locked') return { waiting: true }
  if (access.state !== 'ok') return drop('accountDisabled')
  if (access.reasonCode === 'noAccess' || !access.result.canRead) return drop('noAccess')
  const room = one(ROOM, { chatRoomId: row.chatRoomId })
  const name = senderName(event)
  const texts = inLanguageOf(row.userId, () => {
    const direct = room && room.roomTypeEnumId === 'CrtDirect'
    const title = direct || !room || !room.roomName ? ec.l10n.localize('Reminder: ${senderName}').replace('${senderName}', name)
      : ec.l10n.localize('Reminder: ${senderName} in ${roomName}').replace('${senderName}', name).replace('${roomName}', room.roomName)
    return { title, hiddenBody: ec.l10n.localize('A message you asked to be reminded of') }
  })
  const activity = ec.entity.makeValue('moqui.chat.ChatUserActivity')
  activity.setSequencedIdPrimary()
  activity.setFields({ userId: row.userId, activityKindEnumId: 'CuaReminder', chatRoomId: row.chatRoomId, eventSeq: Number(row.eventSeq),
    messageEventSeq: Number(row.eventSeq), actorUserId: event.senderUserId ?? null, chatActorId: event.senderActorId ?? null,
    activityDate: ec.user.nowTimestamp }, true, null, null)
  activity.create()
  row.setFields({ statusEnumId: 'CrmFired', firedDate: ec.user.nowTimestamp }, true, null, null)
  row.update()
  // the push and the notification: one that fails is logged, and the reminder still fired (its Activity row holds it)
  ec.message.pushErrors()
  try {
    ec.service.sync().name(SERVICES + 'queue#ReminderPush').parameters({ userId: row.userId, chatMessageReminderId: row.chatMessageReminderId,
      chatRoomId: row.chatRoomId, eventSeq: Number(row.eventSeq), title: texts.title, hiddenBody: texts.hiddenBody,
      nowMillis: String(now()) }).disableAuthz().call()
    if (ec.message.hasError()) ec.logger.error(`The push of chat reminder ${row.chatMessageReminderId} was not queued: ${ec.message.errorsString}`)
  } catch (err) {
    ec.logger.error(`The push of chat reminder ${row.chatMessageReminderId} was not queued: ${message(err)}`)
  }
  ec.message.clearErrors()
  ec.message.popErrors()
  try {
    ec.makeNotificationMessage().topic('ChatReminder').userIds([row.userId]).message({ title: texts.title, link: roomLink(row.chatRoomId, Number(row.eventSeq)),
      chatRoomId: row.chatRoomId, eventSeq: Number(row.eventSeq) }).reason('reminder').record(ROOM, row.chatRoomId).send()
  } catch (err) {
    ec.logger.error(`The notification of chat reminder ${row.chatMessageReminderId} was not sent: ${message(err)}`)
  }
  told(row.userId, row.chatRoomId, 'reminder')
  return { fired: true }
}

/** send#DueReminders (the job): each reminder due, on its own; a sign-in lock's wait for the next run. */
function sendDueReminders(p) {
  const limit = p.limit > 0 ? Number(p.limit) : 500
  const cf = ec.entity.conditionFactory
  const due = Array.from(find(REMINDER).condition('statusEnumId', 'CrmPending').condition(cf.makeCondition('remindDate', 'less-equals', ec.user.nowTimestamp))
    .selectField('chatMessageReminderId').orderBy('remindDate').orderBy('chatMessageReminderId').limit(limit).list())
  let firedCount = 0, droppedCount = 0, waitingCount = 0
  for (const row of due) {
    ec.message.pushErrors()
    try {
      const out = ec.service.sync().name(SERVICES + 'fire#Reminder').parameter('chatMessageReminderId', row.chatMessageReminderId)
        .requireNewTransaction(true).disableAuthz().call() || {}
      if (ec.message.hasError()) ec.logger.error(`Could not send chat reminder ${row.chatMessageReminderId}: ${ec.message.errorsString}`)
      else if (out.fired) firedCount++
      else if (out.dropped) droppedCount++
      else if (out.waiting) waitingCount++
    } catch (err) {
      ec.logger.error(`Could not send chat reminder ${row.chatMessageReminderId}: ${message(err)}`)
    }
    ec.message.clearErrors()
    ec.message.popErrors()
  }
  return { firedCount, droppedCount, waitingCount }
}

// ---------- the lists ----------

/** get#ChatSchedule: the caller's scheduled messages and pending reminders. */
function getSchedule() {
  const me = ec.user.userId
  if (!me) return { scheduledList: [], reminderList: [] }
  const cf = ec.entity.conditionFactory
  const rows = Array.from(find(SCHEDULED).condition('senderUserId', me).condition(cf.makeCondition([
    cf.makeCondition('statusEnumId', 'in', ['CsmScheduled', 'CsmFailed']),
    cf.makeCondition([cf.makeCondition('statusEnumId', 'equals', 'CsmSent'), cf.makeCondition('lateSeenDate', 'equals', null),
      cf.makeCondition([cf.makeCondition('sentLate', 'equals', 'Y'), cf.makeCondition('sentAfterCancel', 'equals', 'Y')], 'or'),
      cf.makeCondition('sentDate', 'greater-equals', new Date(now() - KEEP_DAYS * DAY))], 'and')], 'or'))
    .orderBy('sendDate').orderBy('chatScheduledMessageId').limit(MAX_SCHEDULED * 2).list())
  const devices = {}
  for (const row of rows) if (row.heldByDeviceId && !(row.heldByDeviceId in devices)) devices[row.heldByDeviceId] = liveDevice(me, row.heldByDeviceId)
  const scheduledList = rows.map((row) => listed(row, devices))
  const reminders = Array.from(find(REMINDER).condition('userId', me).condition('statusEnumId', 'CrmPending').orderBy('remindDate')
    .orderBy('chatMessageReminderId').limit(MAX_REMINDERS).list())
  const reminderList = []
  const readable = {}
  for (const row of reminders) {
    if (!(row.chatRoomId in readable)) {
      const access = ec.service.sync().name(CHAT + 'get#RoomAccess').parameters({ chatRoomId: row.chatRoomId }).call() || {}
      ec.message.clearErrors()
      readable[row.chatRoomId] = access.canRead ? access.room || {} : null
    }
    const room = readable[row.chatRoomId]
    if (!room) continue
    const event = one(EVENT, { chatRoomId: row.chatRoomId, eventSeq: Number(row.eventSeq) })
    const sealed = event && event.encrypted === 'Y'
    let content = {}
    try { content = sealed && event.contentJson ? JSON.parse(event.contentJson) : {} } catch (err) { content = {} }
    const text = !event || event.redactedDate || sealed ? null : String(event.bodyText || '').replace(/\s+/g, ' ').trim()
    reminderList.push({ chatMessageReminderId: row.chatMessageReminderId, chatRoomId: row.chatRoomId, eventSeq: Number(row.eventSeq),
      remindDate: millis(row.remindDate), roomName: room.roomName ?? null, senderUserId: event ? event.senderUserId ?? null : null,
      senderName: event ? senderName(event) : null, snippet: text ? (text.length > 140 ? text.slice(0, 139) + '…' : text) : null,
      encrypted: !!sealed, encryptedContent: sealed ? content.encrypted || null : null, encryptedSentDate: sealed ? millis(event.sentDate) : null,
      gone: !event || !!event.redactedDate })
  }
  return { scheduledList, reminderList }
}

/** clean#ScheduledMessages (daily): every done row after KEEP_DAYS, a failed one with its text and files; each on its own. */
function clean(p) {
  const at = now()
  const cf = ec.entity.conditionFactory
  let removedCount = 0
  const old = (status, field) => Array.from(find(SCHEDULED).condition('statusEnumId', status)
    .condition(cf.makeCondition(field, 'less', new Date(at - KEEP_DAYS * DAY))).limit(1000).list())
  for (const row of old('CsmSent', 'sentDate').concat(old('CsmCancelled', 'lastUpdatedStamp'), old('CsmFailed', 'lastUpdatedStamp'))) {
    try {
      removeFiles(row)
      row.delete()
      removedCount++
    } catch (err) {
      ec.logger.error(`Could not remove scheduled chat message ${row.chatScheduledMessageId}: ${message(err)}`)
    }
  }
  try {
    removedCount += find(REMINDER).condition('statusEnumId', 'in', ['CrmFired', 'CrmCancelled', 'CrmDropped'])
      .condition(cf.makeCondition('lastUpdatedStamp', 'less', new Date(at - KEEP_DAYS * DAY))).deleteAll()
  } catch (err) {
    ec.logger.error(`Could not remove old chat reminders: ${message(err)}`)
  }
  return { removedCount }
}

const operations = {
  schedule, update, cancel, dismiss, sendNow, deliver, fail, sendDueMessages, claimHeld, sendHeld, completeHeld, reportHeld, setReminder,
  cancelReminder, fire, sendDueReminders, getSchedule, clean,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown chat schedule operation ' + op)
  return operations[op](parameters || {}) || {}
}

;({ execute })
