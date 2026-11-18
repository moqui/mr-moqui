// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Scheduled sends and message reminders in chat (plans/Comms-Messaging.md §18.3 phase 15): the services of
// org.moqui.chat.ChatScheduleServices. The entities are framework/entity/ChatScheduleEntities.xml.
//
// - A scheduled message of a plain, phone or bridged room is held by the server (ChatScheduledMessage with its body) and
//   sent at its time by sendDueMessages as its sender: signed in as them (ec.user.internalLoginUser), so their rights are
//   those of the send time, through send#Message, so every bridge and its send checks (check#SmsSend) apply then. The
//   clientRef sched:<id> makes it go once.
// - One of an encrypted room is a placeholder with no text: the sender's device (heldByDeviceId) keeps the draft sealed and
//   sends it at its time through the normal encrypted send, after claimHeld gives it a lease; completeHeld records the
//   message it made. The job never sends it, and fails it only when its device was revoked.
// - A reminder (ChatMessageReminder) keeps the room and the message's number only. At its time (sendDueReminders) it is an
//   Activity row (CuaReminder), a ChatReminder notification and a push to each device (queue#ReminderPush, chat_push.rs).
//
// Each service acts on the signed-in user's own rows: another's is "not found". The jobs take each row on its own, in a
// transaction of its own: one that fails is logged and the rest go on.

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

/** The most messages a user has waiting, and reminders pending. */
const MAX_SCHEDULED = 100
const MAX_REMINDERS = 200
/** The earliest and latest times: a minute ahead (a few seconds of slack for the client's clock), a year ahead. */
const MIN_AHEAD_MS = 55 * 1000
const MAX_AHEAD_MS = 366 * 86400 * 1000
/** A held message's lease, and how late a send is "sent late". */
const LEASE_MS = 2 * 60 * 1000
const LATE_MS = 5 * 60 * 1000
/** Tries of a message whose send failed for no reason of its own, before it fails. */
const MAX_ATTEMPTS = 5
const STATUS = { CsmScheduled: 'scheduled', CsmSent: 'sent', CsmFailed: 'failed', CsmCancelled: 'cancelled' }
/** The reasons a message was not sent, in the sender's words. */
const REASONS = {
  notMember: 'You are no longer in the room',
  archived: 'The room is archived',
  blocked: 'You blocked this conversation',
  accountDisabled: 'Your account cannot send now',
  deviceGone: "The draft was on a device that is gone",
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
const appRoot = () => (System.getProperty('comms_app_root') || '/qapps/BasicComms').replace(/\/+$/, '')
const roomLink = (chatRoomId, eventSeq) => `${appRoot()}/Chat?chatRoomId=${encodeURIComponent(chatRoomId)}${eventSeq != null ? '&eventSeq=' + eventSeq : ''}`

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

/**
 * Run fn signed in as the user (userId), back to whoever was signed in after: {ran, value}, ran false when they cannot
 * sign in (disabled, ended). The chat rules are the services' own and take the signed-in user (membership, the archive,
 * blocks, mentions); the artifact grants are not asked again, as a job has no screen to inherit them from: the user's
 * screen checked them when the row was made.
 */
function asUser(userId, fn) {
  const was = ec.user.username || null
  const wasId = ec.user.userId || null
  const unchecked = () => {
    const off = ec.artifactExecution.disableAuthz()
    try {
      return fn()
    } finally {
      if (!off) ec.artifactExecution.enableAuthz()
    }
  }
  if (wasId === userId) return { ran: true, value: unchecked() }
  const account = one(ACCOUNT, { userId })
  if (!account || account.disabled === 'Y' || (account.terminateDate && Number(account.terminateDate) <= now())) return { ran: false }
  let signedIn = false
  try {
    signedIn = ec.user.internalLoginUser(account.username) && ec.user.userId === userId
    ec.message.clearErrors()
    if (!signedIn) return { ran: false }
    return { ran: true, value: unchecked() }
  } finally {
    if (signedIn) ec.user.logoutLocal()
    if (was) ec.user.internalLoginUser(was)
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

/** The caller's row of `entity` by id, locked, or null ("not found"). */
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
    replyToEventSeq: p.replyToEventSeq ?? null, threadRootEventSeq: p.threadRootEventSeq ?? null, createdDate: ec.user.nowTimestamp }
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
    const records = Array.from(p.recordList || []).filter((record) => record && record.entity && record.id).slice(0, 20)
      .map((record) => ({ entity: String(record.entity), id: String(record.id) }))
    fields.recordsJson = records.length ? JSON.stringify(records) : null
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
  return { chatScheduledMessageId: row.chatScheduledMessageId, chatRoomId: row.chatRoomId, sendDate: millis(row.sendDate),
    status: STATUS[row.statusEnumId] || null, body: row.body ?? null, bodyText: row.bodyText ?? null,
    replyToEventSeq: row.replyToEventSeq != null ? Number(row.replyToEventSeq) : null,
    threadRootEventSeq: row.threadRootEventSeq != null ? Number(row.threadRootEventSeq) : null, files,
    held, heldByDeviceId: row.heldByDeviceId ?? null, heldByDeviceName: row.heldByDeviceName ?? null,
    deviceGone: held && row.statusEnumId === 'CsmScheduled' && !device,
    sentLate: row.sentLate === 'Y', sentDate: millis(row.sentDate), sentEventSeq: row.sentEventSeq != null ? Number(row.sentEventSeq) : null,
    reasonCode: row.reasonCode ?? null, failText: row.failText ?? null }
}

/** update#ScheduledMessage: a new body or time, while it waits. A held one: its time only, from its device. */
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
    row.set("sendDate", new Date(at))
  }
  if (!row.heldByDeviceId && p.body != null) {
    const html = StringUtilities.cleanChatHtml(String(p.body)).trim()
    if (html.length > 65536) return refuse('tooLong', 'The message is too long')
    const hasFiles = find(ATTACHMENT).condition('chatScheduledMessageId', row.chatScheduledMessageId).count() > 0
    if (!textOf(html) && !/<img\b[^>]*\bsrc=/i.test(html) && !hasFiles) return refuse('empty', 'A message needs text or a file')
    row.set('body', html || null)
    row.set('bodyText', textOf(html) || null)
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

/** cancel#ScheduledMessage: from any device of the sender; a held one's draft stays on its device until it looks. */
function cancel(p) {
  const row = mine(SCHEDULED, 'chatScheduledMessageId', p.chatScheduledMessageId, 'senderUserId')
  if (!row) return refuse('notFound', 'Scheduled message not found')
  if (row.statusEnumId !== 'CsmScheduled' && row.statusEnumId !== 'CsmFailed') return refuse('notScheduled', 'This message was sent or is no longer scheduled')
  if (row.leaseDeviceId && millis(row.leaseExpiresDate) > now()) return refuse('sending', 'This message is being sent now')
  row.setFields({ statusEnumId: 'CsmCancelled', leaseDeviceId: null, leaseExpiresDate: null }, true, null, null)
  row.update()
  removeFiles(row)
  told(row.senderUserId, row.chatRoomId, 'scheduled')
  return {}
}

/** dismiss#ScheduledMessage: the sender saw that it went late. */
function dismiss(p) {
  const row = mine(SCHEDULED, 'chatScheduledMessageId', p.chatScheduledMessageId, 'senderUserId')
  if (!row) return refuse('notFound', 'Scheduled message not found')
  if (row.sentLate === 'Y' && !row.lateSeenDate) {
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
  if (result.failed || result.reasonCode) return refuse(result.reasonCode || 'failed', result.failText || REASONS[result.reasonCode] || REASONS.failed)
  return { eventSeq: result.eventSeq ?? null }
}

/**
 * deliver#ScheduledMessage (its own transaction): the send of a message the server holds, as its sender. Out: eventSeq,
 * or reasonCode and failText with an error (the send rolled back; the caller fails the row).
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
  const sent = asUser(row.senderUserId, () => {
    const access = ec.service.sync().name(CHAT + 'get#RoomAccess').parameters({ chatRoomId: row.chatRoomId }).call() || {}
    if (ec.message.hasError()) return refused('sendRefused', ec.message.errorsString)
    if (!access.canRead || !access.isMember) return refused('notMember')
    if (access.isArchived) return refused('archived')
    // the sender blocked the room's number or person since: it does not go (D1)
    const member = one(MEMBER, { chatRoomId: row.chatRoomId, userId: row.senderUserId })
    if (member && member.filterEnumId === 'CmfBlocked') return refused('blocked')
    const files = Array.from(find(ATTACHMENT).condition('chatScheduledMessageId', row.chatScheduledMessageId).condition('eventSeq', null)
      .orderBy('chatAttachmentId').list()).map((file) => file.chatAttachmentId)
    let records = []
    try { records = row.recordsJson ? JSON.parse(row.recordsJson) : [] } catch (err) { records = [] }
    const out = ec.service.sync().name(CHAT + 'send#Message').parameters({ chatRoomId: row.chatRoomId, body: row.body ?? null,
      replyToEventSeq: row.replyToEventSeq != null ? Number(row.replyToEventSeq) : null,
      threadRootEventSeq: row.threadRootEventSeq != null ? Number(row.threadRootEventSeq) : null,
      chatAttachmentIdList: files, recordList: records, roomUrl: roomLink(row.chatRoomId), clientRef: 'sched:' + row.chatScheduledMessageId }).call() || {}
    if (ec.message.hasError()) return refused(out.reasonCode || 'sendRefused', ec.message.errorsString)
    return { eventSeq: out.eventSeq }
  })
  if (!sent.ran) return refused('accountDisabled')
  if (sent.value.reasonCode) return sent.value
  row.setFields({ statusEnumId: 'CsmSent', sentEventSeq: sent.value.eventSeq, sentDate: ec.user.nowTimestamp, attemptCount: Number(row.attemptCount || 0) + 1,
    reasonCode: null, failText: null }, true, null, null)
  row.update()
  told(row.senderUserId, row.chatRoomId, 'scheduled')
  return { eventSeq: sent.value.eventSeq }
}

/**
 * fail#ScheduledMessage (its own transaction): a send that did not go. With a reasonCode it is not sent (CsmFailed, and
 * the sender is told); without one (an error of no reason of its own) it is tried again at the next run, MAX_ATTEMPTS in
 * all. Out: failed.
 */
function fail(p) {
  const row = find(SCHEDULED).condition('chatScheduledMessageId', p.chatScheduledMessageId).forUpdate(true).one()
  if (!row || (row.statusEnumId !== 'CsmScheduled' && row.statusEnumId !== 'CsmFailed')) return { failed: false }
  const attempts = Number(row.attemptCount || 0) + 1
  const final = !blank(p.reasonCode) || attempts >= MAX_ATTEMPTS
  row.setFields({ attemptCount: attempts, failText: p.failText ? String(p.failText).slice(0, 255) : null }, true, null, null)
  if (final) row.setFields({ statusEnumId: 'CsmFailed', reasonCode: p.reasonCode || 'failed', leaseDeviceId: null, leaseExpiresDate: null }, true, null, null)
  row.update()
  if (!final) return { failed: false }
  told(row.senderUserId, row.chatRoomId, 'scheduled')
  try {
    const roomRow = one(ROOM, { chatRoomId: row.chatRoomId })
    const name = roomRow && roomRow.roomName ? roomRow.roomName : null
    const title = asUser(row.senderUserId, () => {
      const why = ec.l10n.localize(REASONS[row.reasonCode] || REASONS.failed)
      return name ? ec.l10n.localize('Your scheduled message in ${roomName} was not sent: ${reason}').replace('${roomName}', name).replace('${reason}', why)
        : ec.l10n.localize('Your scheduled message was not sent: ${reason}').replace('${reason}', why)
    })
    ec.makeNotificationMessage().topic('ChatScheduleFailed').userIds([row.senderUserId]).message({ title: title.ran ? title.value : 'Your scheduled message was not sent',
      link: roomLink(row.chatRoomId), chatRoomId: row.chatRoomId, chatScheduledMessageId: row.chatScheduledMessageId, reasonCode: row.reasonCode })
      .record(ROOM, row.chatRoomId).send()
  } catch (err) {
    ec.logger.error(`The sender of scheduled message ${row.chatScheduledMessageId} was not told it was not sent: ${message(err)}`)
  }
  return { failed: true }
}

/** One row's send in its own transaction, then its failure in another. Out: {sent, eventSeq} or {failed, reasonCode}. */
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
  if (!result.reasonCode && !thrown) return { sent: true, eventSeq: result.eventSeq }
  if (thrown) ec.logger.error(`Could not send scheduled chat message ${id}: ${thrown}`)
  ec.message.pushErrors()
  let failed = false
  try {
    failed = !!(ec.service.sync().name(SERVICES + 'fail#ScheduledMessage').parameters({ chatScheduledMessageId: id,
      reasonCode: result.reasonCode || null, failText: result.failText || thrown || null }).requireNewTransaction(true).disableAuthz().call() || {}).failed
  } catch (err) {
    ec.logger.error(`Could not record the failed send of scheduled chat message ${id}: ${message(err)}`)
  }
  ec.message.clearErrors()
  ec.message.popErrors()
  return { failed, reasonCode: result.reasonCode || null, failText: result.failText || null }
}

/** send#DueMessages (the job): the server's due messages, each on its own; held ones whose device is gone fail. */
function sendDueMessages(p) {
  const limit = p.limit > 0 ? Number(p.limit) : 200
  const budget = Math.max(1, Number(p.timeBudgetSeconds ?? 45)) * 1000
  const started = Date.now()
  let sentCount = 0, failedCount = 0, left = 0
  const cf = ec.entity.conditionFactory
  const due = Array.from(find(SCHEDULED).condition('statusEnumId', 'CsmScheduled').condition(cf.makeCondition('sendDate', 'less-equals', ec.user.nowTimestamp))
    .selectField('chatScheduledMessageId').selectField('heldByDeviceId').selectField('senderUserId').orderBy('sendDate').orderBy('chatScheduledMessageId')
    .limit(limit).list())
  for (const row of due) {
    if (row.heldByDeviceId) {
      // the device keeps it; only a device revoked since cannot send it
      if (!liveDevice(row.senderUserId, row.heldByDeviceId)) {
        ec.message.pushErrors()
        try {
          if ((ec.service.sync().name(SERVICES + 'fail#ScheduledMessage').parameters({ chatScheduledMessageId: row.chatScheduledMessageId, reasonCode: 'deviceGone' })
            .requireNewTransaction(true).disableAuthz().call() || {}).failed) failedCount++
        } catch (err) {
          ec.logger.error(`Could not fail scheduled chat message ${row.chatScheduledMessageId} of a gone device: ${message(err)}`)
        }
        ec.message.clearErrors()
        ec.message.popErrors()
      }
      continue
    }
    if (Date.now() - started > budget) { left++; continue }
    const result = deliverOne(row.chatScheduledMessageId, false)
    if (result.sent) sentCount++
    else if (result.failed) failedCount++
  }
  if (left) ec.logger.info(`${left} due chat messages wait for the next run of send#DueMessages`)
  return { sentCount, failedCount }
}

// ---------- an encrypted room's message, held by its device ----------

/** The room's message the sender's device sent for row (its clientRef sched:<id>), or null. */
function sentFor(row) {
  const list = find(EVENT).condition('chatRoomId', row.chatRoomId).condition('senderUserId', row.senderUserId)
    .condition('clientRef', 'sched:' + row.chatScheduledMessageId).condition('eventTypeEnumId', 'CetMessage').limit(1).list()
  return list.length ? list[0] : null
}

function markSent(row, eventSeq) {
  const at = now()
  row.setFields({ statusEnumId: 'CsmSent', sentEventSeq: Number(eventSeq), sentDate: new Date(at), leaseDeviceId: null, leaseExpiresDate: null,
    sentLate: at - millis(row.sendDate) > LATE_MS ? 'Y' : null }, true, null, null)
  row.update()
  told(row.senderUserId, row.chatRoomId, 'scheduled')
}

/** claim#HeldMessage: the holding device takes a held message that is due (or Send now) to send it. */
function claimHeld(p) {
  const row = mine(SCHEDULED, 'chatScheduledMessageId', p.chatScheduledMessageId, 'senderUserId')
  if (!row || !row.heldByDeviceId) return refuse('notFound', 'Scheduled message not found')
  if (row.statusEnumId !== 'CsmScheduled') return { claimed: false, sent: row.statusEnumId === 'CsmSent' }
  if (String(p.deviceId || '') !== row.heldByDeviceId || !liveDevice(row.senderUserId, row.heldByDeviceId)) {
    return refuse('notHolder', 'Only the device that keeps this message sends it')
  }
  const at = now()
  if (!p.atOnce && millis(row.sendDate) > at + 1000) return { claimed: false }
  // a report that was lost: the message is there
  const made = sentFor(row)
  if (made) { markSent(row, made.eventSeq); return { claimed: false, sent: true } }
  if (row.leaseDeviceId && millis(row.leaseExpiresDate) > at) return { claimed: false }
  const access = ec.service.sync().name(CHAT + 'get#RoomAccess').parameters({ chatRoomId: row.chatRoomId }).call() || {}
  if (ec.message.hasError()) return {}
  const member = one(MEMBER, { chatRoomId: row.chatRoomId, userId: row.senderUserId })
  const reason = !access.canRead || !access.isMember ? 'notMember' : access.isArchived ? 'archived' : member && member.filterEnumId === 'CmfBlocked' ? 'blocked' : null
  if (reason) {
    row.setFields({ statusEnumId: 'CsmFailed', reasonCode: reason, leaseDeviceId: null, leaseExpiresDate: null }, true, null, null)
    row.update()
    told(row.senderUserId, row.chatRoomId, 'scheduled')
    return { claimed: false, reasonCode: reason }
  }
  row.setFields({ leaseDeviceId: row.heldByDeviceId, leaseExpiresDate: new Date(at + LEASE_MS), attemptCount: Number(row.attemptCount || 0) + 1 }, true, null, null)
  row.update()
  return { claimed: true, clientRef: 'sched:' + row.chatScheduledMessageId }
}

/** complete#HeldMessage: the holding device's report of the message it sent (eventSeq), or that its draft is lost. */
function completeHeld(p) {
  const row = mine(SCHEDULED, 'chatScheduledMessageId', p.chatScheduledMessageId, 'senderUserId')
  if (!row || !row.heldByDeviceId) return refuse('notFound', 'Scheduled message not found')
  if (row.statusEnumId === 'CsmSent') return { sent: true }
  if (row.statusEnumId !== 'CsmScheduled') return refuse('notScheduled', 'This message was sent or is no longer scheduled')
  if (String(p.deviceId || '') !== row.heldByDeviceId) return refuse('notHolder', 'Only the device that keeps this message sends it')
  if (p.draftLost === true || p.draftLost === 'true') {
    row.setFields({ statusEnumId: 'CsmFailed', reasonCode: 'draftLost', leaseDeviceId: null, leaseExpiresDate: null }, true, null, null)
    row.update()
    told(row.senderUserId, row.chatRoomId, 'scheduled')
    return { sent: false }
  }
  const made = sentFor(row)
  if (!made || p.eventSeq == null || Number(made.eventSeq) !== Number(p.eventSeq)) return refuse('notFound', 'The message this one made was not found')
  markSent(row, made.eventSeq)
  return { sent: true, sentLate: row.sentLate === 'Y' }
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
    row.set("remindDate", new Date(at))
    row.update()
  } else {
    if (find(REMINDER).condition('userId', me).condition('statusEnumId', 'CrmPending').count() >= MAX_REMINDERS) {
      return refuse('tooMany', 'You have 200 reminders: cancel some first')
    }
    row = ec.entity.makeValue(REMINDER)
    row.setSequencedIdPrimary()
    row.setFields({ userId: me, chatRoomId: p.chatRoomId, eventSeq: Number(p.eventSeq), remindDate: new Date(at), statusEnumId: 'CrmPending',
      createdDate: ec.user.nowTimestamp }, true, null, null)
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

/** fire#Reminder (its own transaction): one reminder that is due. Out: fired, dropped. */
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
  const room = one(ROOM, { chatRoomId: row.chatRoomId })
  const name = senderName(event)
  // as its user: their access now, and the texts in their language
  const ran = asUser(row.userId, () => {
    const access = ec.service.sync().name(CHAT + 'get#RoomAccess').parameters({ chatRoomId: row.chatRoomId }).call() || {}
    ec.message.clearErrors()
    if (!access.canRead) return null
    const direct = room && room.roomTypeEnumId === 'CrtDirect'
    const title = direct || !room || !room.roomName ? ec.l10n.localize('Reminder: ${senderName}').replace('${senderName}', name)
      : ec.l10n.localize('Reminder: ${senderName} in ${roomName}').replace('${senderName}', name).replace('${roomName}', room.roomName)
    return { title, hiddenBody: ec.l10n.localize('A message you asked to be reminded of') }
  })
  if (!ran.ran || !ran.value) return drop('noAccess')
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
      chatRoomId: row.chatRoomId, eventSeq: Number(row.eventSeq), title: ran.value.title, hiddenBody: ran.value.hiddenBody,
      nowMillis: String(now()) }).disableAuthz().call()
    if (ec.message.hasError()) ec.logger.error(`The push of chat reminder ${row.chatMessageReminderId} was not queued: ${ec.message.errorsString}`)
  } catch (err) {
    ec.logger.error(`The push of chat reminder ${row.chatMessageReminderId} was not queued: ${message(err)}`)
  }
  ec.message.clearErrors()
  ec.message.popErrors()
  try {
    ec.makeNotificationMessage().topic('ChatReminder').userIds([row.userId]).message({ title: ran.value.title, link: roomLink(row.chatRoomId, Number(row.eventSeq)),
      chatRoomId: row.chatRoomId, eventSeq: Number(row.eventSeq) }).reason('reminder').record(ROOM, row.chatRoomId).send()
  } catch (err) {
    ec.logger.error(`The notification of chat reminder ${row.chatMessageReminderId} was not sent: ${message(err)}`)
  }
  told(row.userId, row.chatRoomId, 'reminder')
  return { fired: true }
}

/** send#DueReminders (the job): each reminder due, on its own. */
function sendDueReminders(p) {
  const limit = p.limit > 0 ? Number(p.limit) : 500
  const cf = ec.entity.conditionFactory
  const due = Array.from(find(REMINDER).condition('statusEnumId', 'CrmPending').condition(cf.makeCondition('remindDate', 'less-equals', ec.user.nowTimestamp))
    .selectField('chatMessageReminderId').orderBy('remindDate').orderBy('chatMessageReminderId').limit(limit).list())
  let firedCount = 0, droppedCount = 0
  for (const row of due) {
    ec.message.pushErrors()
    try {
      const out = ec.service.sync().name(SERVICES + 'fire#Reminder').parameter('chatMessageReminderId', row.chatMessageReminderId)
        .requireNewTransaction(true).disableAuthz().call() || {}
      if (ec.message.hasError()) ec.logger.error(`Could not send chat reminder ${row.chatMessageReminderId}: ${ec.message.errorsString}`)
      else if (out.fired) firedCount++
      else if (out.dropped) droppedCount++
    } catch (err) {
      ec.logger.error(`Could not send chat reminder ${row.chatMessageReminderId}: ${message(err)}`)
    }
    ec.message.clearErrors()
    ec.message.popErrors()
  }
  return { firedCount, droppedCount }
}

// ---------- the lists ----------

/** get#ChatSchedule: the caller's scheduled messages and pending reminders. */
function getSchedule() {
  const me = ec.user.userId
  if (!me) return { scheduledList: [], reminderList: [] }
  const cf = ec.entity.conditionFactory
  const rows = Array.from(find(SCHEDULED).condition('senderUserId', me).condition(cf.makeCondition([
    cf.makeCondition('statusEnumId', 'in', ['CsmScheduled', 'CsmFailed']),
    cf.makeCondition([cf.makeCondition('sentLate', 'equals', 'Y'), cf.makeCondition('lateSeenDate', 'equals', null),
      cf.makeCondition('sentDate', 'greater-equals', new Date(now() - 7 * 86400 * 1000))], 'and')], 'or'))
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

/** clean#ScheduledMessages (daily): rows done after 7 days, failed ones after 30 with their files; each on its own. */
function clean(p) {
  const at = now()
  const cf = ec.entity.conditionFactory
  let removedCount = 0
  const old = (status, field, days) => Array.from(find(SCHEDULED).condition('statusEnumId', status)
    .condition(cf.makeCondition(field, 'less', new Date(at - days * 86400 * 1000))).limit(1000).list())
  for (const row of old('CsmSent', 'sentDate', 7).concat(old('CsmCancelled', 'lastUpdatedStamp', 7), old('CsmFailed', 'lastUpdatedStamp', 30))) {
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
      .condition(cf.makeCondition('lastUpdatedStamp', 'less', new Date(at - 7 * 86400 * 1000))).deleteAll()
  } catch (err) {
    ec.logger.error(`Could not remove old chat reminders: ${message(err)}`)
  }
  return { removedCount }
}

const operations = {
  schedule, update, cancel, dismiss, sendNow, deliver, fail, sendDueMessages, claimHeld, completeHeld, setReminder, cancelReminder, fire,
  sendDueReminders, getSchedule, clean,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown chat schedule operation ' + op)
  return operations[op](parameters || {}) || {}
}

;({ execute })
