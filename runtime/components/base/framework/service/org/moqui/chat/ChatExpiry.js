// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Disappearing messages and view-once media (plans/Comms-Messaging.md §18.3 phase 16; docs/tools/Comms.md "Disappearing
// messages"): the services of org.moqui.chat.ChatExpiryServices and the helpers ChatServices calls through
// ec.resource.script.
//
// - A room's timer (ChatRoom.messageTimerSeconds, setRoomTimer): a new message goes that long after it is sent, for
//   everyone. append#Event asks expiryOf for an event's expireDate: a message's sentDate plus the timer; an edit, reaction,
//   redaction or pin of a message the message's own, so they are due together. A view-once message's is at the latest
//   chat@view-once-days after it is sent (content.viewOnceUntil keeps that time).
// - The delete (expireDue, expireRoom): the due rows by the index CHAT_EVENT_EXPIRE, each room in its own transaction.
//   A message goes with what clear#MessageContent clears, its preview cache rows no other link has, its thread root's
//   count, the room's last message, its room changes (one marker change stands for them: eventSeq empty, {cause:
//   'expired', before}) and, in an encrypted room, the key backup of a Megolm session no event of the room uses any more.
//   The members get an 'expired' push and their counts.
// - View-once (openViewOnce, viewOnceFile): each recipient opens it once, with a window of WINDOW_MILLIS and at most
//   MAX_DOWNLOADS downloads; its sender never gets the file. Once every recipient opened it, the message is due at the
//   end of that window: its file goes then and the message stands, as Opened, to its viewOnceUntil.
// - Bridges: a bridged, phone or federated room gets no timer and no view-once (a bridge cannot delete there), and a
//   room with a timer is not bridged (checkBridge, with the room row locked as setRoomTimer locks it). No service sets
//   ChatRoom.federationEnabled (it is data only): one that does must refuse a room with a timer, as checkBridge does.
// - A failing room is skipped for FAILED_SKIP_MILLIS on this node and the worker waits before it tries again, so one
//   room never spins the worker or holds the others back.
// - Scheduled send (phase 15): a scheduled message gets its time when it is sent (append#Event), by the room's timer
//   then; a pending reminder of a message that went is dropped (messageGone).

const ROOM = 'moqui.chat.ChatRoom'
const EVENT = 'moqui.chat.ChatEvent'
const MEMBER = 'moqui.chat.ChatRoomMember'
const MAPPING = 'moqui.chat.ChatRoomBridge'
const ATTACHMENT = 'moqui.chat.ChatAttachment'
const OPEN = 'moqui.chat.ChatViewOnceOpen'
const LINK = 'moqui.chat.ChatEventLink'
const PREVIEW = 'moqui.chat.ChatLinkPreview'
const BACKUP_SESSION = 'moqui.chat.ChatKeyBackupSession'
const CHANGE = 'moqui.chat.ChatRoomChange'
const ACTIVITY = 'moqui.chat.ChatUserActivity'
const CHAT = 'org.moqui.chat.ChatServices'

/** The timer's range in seconds: 30 s to 4 weeks. */
const MIN_SECONDS = 30
const MAX_SECONDS = 28 * 86400
/** A view-once open serves the file this long, this many times. */
const WINDOW_MILLIS = 120000
const MAX_DOWNLOADS = 3
/** The view-once media: one photo or one video. */
const VIEW_ONCE_TYPE = /^(image\/(png|jpe?g|gif|webp)|video\/(mp4|webm|quicktime))(\s*;.*)?$/i
const MESSAGES = ['CetMessage', 'CetPoll']
const RELATIONS = { CetEdit: 'editsEventSeq', CetReaction: 'relatesToEventSeq', CetRedaction: 'relatesToEventSeq', CetPin: 'relatesToEventSeq' }
/** The most seqs an 'expired' push names; `before` covers the rest. */
const PUSH_SEQS = 200
/** The most rooms a round takes. */
const ROUND_ROOMS = 200
/** A room whose delete failed is skipped this long on this node (the cache below), so one room cannot hold a round. */
const FAILED_SKIP_MILLIS = 5 * 60000
const FAILED_CACHE = 'chat.expiry.failed'
/** A Megolm session's backup goes this long after its newest use past the room's rotation period: it may be in use till then. */
const SESSION_MARGIN_MILLIS = 10 * 60000
const WEEK_MILLIS = 7 * 86400000
const PENDING_SESSION = 'moqui.chat.ChatExpirySession'
const REMINDER = 'moqui.chat.ChatMessageReminder'

const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const blank = (value) => value == null || String(value).trim() === ''
// a setting by its conf selector (chat@view-once-days), empty when it is not set
const property = (name) => String(ec.factory.getConfValue(name) || '').trim()
const millis = (value) => (value == null || value === '' ? null : ec.user.timestampMillis(value))
const nowMillis = (p) => (p && p.nowMillis != null && String(p.nowMillis) !== '' ? Number(p.nowMillis) : millis(ec.user.nowTimestamp))
const parse = (text) => { if (!text) return {}; try { const value = JSON.parse(text); return value && typeof value === 'object' ? value : {} } catch (err) { return {} } }
const message = (err) => (err && err.message ? err.message : String(err))
const chunks = (list, size) => { const out = []; for (let i = 0; i < list.length; i += size) out.push(list.slice(i, i + size)); return out }

function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

function viewOnceDays() {
  return Math.max(1, Number(property('chat@view-once-days')) || 14)
}

/** The mappings in force of a room (its bridges), and whether it is federated: then no timer or view-once. */
function bridgedReason(room) {
  if (!room) return null
  if (room.roomTypeEnumId === 'CrtExternal') return 'bridged'
  if (find(MAPPING).condition('chatRoomId', room.chatRoomId).condition('thruDate', null).count() > 0) return 'bridged'
  if (room.federationEnabled === 'Y') return 'federated'
  return null
}

/** The room's server name in the Matrix room ID the browsers name its key backup by. */
function matrixRoomId(chatRoomId) {
  return '!' + chatRoomId + ':' + (property('chat@server-name') || 'localhost').toLowerCase()
}

// ---------- append#Event: an event's time ----------

/**
 * An event's expireDate (millis or null), viewOnce ('Y' or null) and megolmSessionId, for append#Event; content is the
 * content to store (a view-once message's gets viewOnceUntil).
 */
function expiryOf(p) {
  const type = p.eventTypeEnumId
  const content = p.content && typeof p.content === 'object' ? p.content : null
  const sessionId = p.encrypted && content && content.encrypted && typeof content.encrypted.session_id === 'string'
    ? String(content.encrypted.session_id).slice(0, 255) : null
  const now = nowMillis(p)
  let expireDate = null, viewOnce = null
  if (MESSAGES.includes(type)) {
    const seconds = Number(p.messageTimerSeconds) || 0
    if (seconds > 0) expireDate = now + seconds * 1000
    if (p.viewOnce) {
      viewOnce = 'Y'
      const until = now + viewOnceDays() * 86400000
      expireDate = expireDate == null ? until : Math.min(expireDate, until)
      if (content) content.viewOnceUntil = expireDate
    }
  } else if (RELATIONS[type] && p[RELATIONS[type]] != null) {
    const target = find(EVENT).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p[RELATIONS[type]])
      .selectField('expireDate').one()
    expireDate = target ? millis(target.expireDate) : null
  }
  return { expireDate, viewOnce, megolmSessionId: sessionId, content }
}

// ---------- send#Message's checks ----------

/** A forward of a message that expires, or of a view-once one, is refused. */
function checkForward(p) {
  const source = p.source
  if (source && (source.expireDate != null && String(source.expireDate) !== '' || source.viewOnce === 'Y'))
    return refuse('expiring', 'A disappearing or view-once message cannot be forwarded')
  return {}
}

/** A view-once message: one photo or video, no text, no records, not in a bridged room. */
function checkViewOnce(p) {
  if (!p.viewOnce) return {}
  const room = one(ROOM, { chatRoomId: p.chatRoomId })
  if (bridgedReason(room)) return refuse('bridged', 'A view-once photo cannot go to another network')
  const files = Array.from(p.attachmentRows || [])
  if (!p.emptyBody || (p.recordCount || 0) > 0 || p.forward) return refuse('viewOnceFiles', 'A view-once photo or video has no text')
  if (files.length !== 1) return refuse('viewOnceFiles', 'A view-once message is one photo or one video')
  if (!p.encrypted && !VIEW_ONCE_TYPE.test(String(files[0].contentType || ''))) return refuse('viewOnceFiles', 'A view-once message is one photo or one video')
  return {}
}

// ---------- the room's timer ----------

function setRoomTimer(p) {
  const access = ec.service.sync().name(CHAT + '.get#RoomAccess').parameter('chatRoomId', p.chatRoomId).call()
  if (ec.message.hasError()) return {}
  if (!access || !access.isMember) { ec.message.addError(`Chat room ${p.chatRoomId} not found`); return {} }
  const room = find(ROOM).condition('chatRoomId', p.chatRoomId).forUpdate(true).one()
  const seconds = blank(p.seconds) ? 0 : Math.floor(Number(p.seconds))
  if (Number.isNaN(seconds) || (seconds !== 0 && (seconds < MIN_SECONDS || seconds > MAX_SECONDS)))
    return refuse('range', 'Disappearing messages go after 30 seconds to 4 weeks')
  const direct = room.roomTypeEnumId === 'CrtDirect'
  if (!direct && !access.canModerate && !access.isOwner) return refuse('notAllowed', 'Only a moderator or owner may change disappearing messages')
  if (room.archivedDate) return refuse('archived', 'The room is archived')
  if (room.roomTypeEnumId === 'CrtRecord') return refuse('recordRoom', 'A record’s room keeps its messages')
  const bridged = seconds > 0 ? bridgedReason(room) : null
  if (bridged === 'bridged') return refuse('bridged', 'This room is bridged: a message that went to another network cannot be deleted there')
  if (bridged === 'federated') return refuse('federated', 'This room is open to other servers, which cannot be made to delete a message')
  const before = Number(room.messageTimerSeconds) || 0
  if (before === seconds) return { seconds: seconds || null }
  room.set('messageTimerSeconds', seconds || null)
  room.update()
  ec.service.sync().name(CHAT + '.append#Event').parameters({ chatRoomId: p.chatRoomId, eventTypeEnumId: 'CetSystem', senderUserId: ec.user.userId,
    content: { action: 'timer', seconds: seconds || null, byUserId: ec.user.userId } }).call()
  if (ec.message.hasError()) return {}
  const memberIds = Array.from(find(MEMBER).condition('chatRoomId', p.chatRoomId).condition('leftDate', null).selectField('userId').list())
    .map((row) => row.userId).filter((id) => id)
  ec.service.sync().name(CHAT + '.record#UserChange').parameters({ userIdList: memberIds, changeKind: 'room', chatRoomId: p.chatRoomId,
    change: { reason: 'timer' }, pushMessage: { kind: 'room', action: 'changed', chatRoomId: p.chatRoomId } }).call()
  return { seconds: seconds || null }
}

/** A bridge mapping of a room with a timer is refused (store#RoomBridge). */
function checkBridge(p) {
  const room = find(ROOM).condition('chatRoomId', p.chatRoomId).forUpdate(true).one()
  if (room && Number(room.messageTimerSeconds) > 0)
    return refuse('timerOn', 'This room has disappearing messages: turn them off before it is bridged')
  return {}
}

// ---------- view-once ----------

/** The local members a view-once message is for: the current ones but its sender and agents. */
function recipientsOf(chatRoomId, senderUserId) {
  return Array.from(find(MEMBER).condition('chatRoomId', chatRoomId).condition('leftDate', null).selectField('userId').selectField('roleEnumId').list())
    .filter((row) => row.userId && row.userId !== senderUserId && row.roleEnumId !== 'CmrAgent').map((row) => row.userId)
}

function viewOnceMessage(chatRoomId, eventSeq, now, lock) {
  const row = find(EVENT).condition({ chatRoomId, eventSeq }).forUpdate(!!lock).one()
  const standing = row && (row.expireDate == null || String(row.expireDate) === '' || millis(row.expireDate) > now)
  return standing && row.viewOnce === 'Y' && MESSAGES.includes(row.eventTypeEnumId) && !row.redactedDate ? row : null
}

/** Whose read receipts are off of userIdList (CHAT_READ_RECEIPTS N): an open by them, or seen by them, is not told. */
function receiptsOff(userIdList) {
  const ids = Array.from(new Set(Array.from(userIdList || []).filter((id) => id)))
  if (!ids.length) return new Set()
  return new Set(ec.service.sync().name(CHAT + '.get#ReceiptsOffUsers').parameter('userIdList', ids).call()?.offUserIdList || [])
}

/**
 * A view-once message changed (opened, its file gone): a change of it in the room's feed and an update push, so each
 * client reads it again. Not pushed to its sender when the opener's read receipts, or the sender's, are off (the sender
 * reads it again only from the feed, and sees no Opened then).
 */
function viewOnceChanged(row, openerId) {
  const recorded = ec.service.sync().name(CHAT + '.record#RoomChange').parameters({ chatRoomId: row.chatRoomId, eventSeq: Number(row.eventSeq),
    change: { cause: 'update', threadRootEventSeq: row.threadRootEventSeq != null ? Number(row.threadRootEventSeq) : null } }).call()
  if (ec.message.hasError()) return
  const off = openerId ? receiptsOff([openerId, row.senderUserId]) : new Set()
  const quietToSender = openerId && (off.has(openerId) || off.has(row.senderUserId))
  const room = one(ROOM, { chatRoomId: row.chatRoomId })
  const isDirect = room && room.roomTypeEnumId === 'CrtDirect'
  const users = Array.from(find(MEMBER).condition('chatRoomId', row.chatRoomId).selectField('userId').selectField('leftDate').selectField('filterEnumId').list())
    .filter((member) => member.userId && (isDirect || !member.leftDate) && !member.filterEnumId && !(quietToSender && member.userId === row.senderUserId))
    .map((member) => member.userId)
  if (users.length) ec.service.sync().name(CHAT + '.push#Signal').parameters({ topic: 'chatRoom:' + row.chatRoomId, userIdList: users,
    message: { kind: 'update', chatRoomId: row.chatRoomId, eventSeq: Number(row.eventSeq),
      threadRootEventSeq: row.threadRootEventSeq != null ? Number(row.threadRootEventSeq) : null, fields: { viewOnceChanged: true },
      roomChangeSeq: recorded && recorded.roomChangeSeq != null ? recorded.roomChangeSeq : null } }).call()
}

function openViewOnce(p) {
  const me = ec.user.userId
  const access = ec.service.sync().name(CHAT + '.get#RoomAccess').parameter('chatRoomId', p.chatRoomId).call()
  if (ec.message.hasError()) return {}
  const now = nowMillis(p)
  // the message's row locked first: two last opens at once cannot both miss that everyone opened it
  const row = access && access.isMember ? viewOnceMessage(p.chatRoomId, p.eventSeq, now, true) : null
  if (!row) { ec.message.addError(`Message ${p.eventSeq} not found`); return {} }
  if (row.senderUserId === me) return refuse('sender', 'Your view-once photo is for the others in the room')
  const file = find(ATTACHMENT).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).orderBy('attachmentSeq').list()[0]
  if (!file) return refuse('gone', 'This view-once photo is no longer here')
  let open = find(OPEN).condition({ chatRoomId: p.chatRoomId, eventSeq: p.eventSeq, userId: me }).forUpdate(true).one()
  if (open) {
    const opened = millis(open.openedDate)
    if (now > opened + WINDOW_MILLIS || Number(open.downloadCount || 0) >= MAX_DOWNLOADS) return refuse('opened', 'You opened this view-once photo already')
  } else {
    open = ec.entity.makeValue(OPEN)
    open.setFields({ chatRoomId: p.chatRoomId, eventSeq: p.eventSeq, userId: me, openedDate: now, downloadCount: 0 }, true, null, null)
    open.create()
    // the sender's Opened, and the opener's other devices drop it: one change of the message
    viewOnceChanged(row, me)
    if (ec.message.hasError()) return {}
    // everyone opened it: its file goes at the end of this window (expireRoom), the message stands as Opened
    const recipients = recipientsOf(p.chatRoomId, row.senderUserId)
    const opened = new Set(Array.from(find(OPEN).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).selectField('userId').list()).map((one) => one.userId))
    if (recipients.every((id) => opened.has(id))) {
      const due = now + WINDOW_MILLIS
      if (!(millis(row.expireDate) <= due)) {
        row.set('expireDate', due)
        row.update()
        ec.service.sync().name('org.moqui.chat.ChatExpiryServices.wake#ChatExpiry').parameter('dueMillis', String(due)).disableAuthz().call()
      }
    }
  }
  return { chatAttachmentId: file.chatAttachmentId, fileName: file.fileName, contentType: file.contentType,
    fileSize: file.fileSize != null ? Number(file.fileSize) : null, until: millis(open.openedDate) + WINDOW_MILLIS }
}

/** The file of a view-once message for a download: the recipient's open in its window, counted. */
function viewOnceFile(p) {
  const me = ec.user.userId
  const file = one(ATTACHMENT, { chatAttachmentId: p.chatAttachmentId })
  const missing = () => { ec.message.addError(`Chat file ${p.chatAttachmentId} not found`); return {} }
  const now = nowMillis(p)
  if (!file || file.eventSeq == null || !viewOnceMessage(file.chatRoomId, file.eventSeq, now, false)) return missing()
  const open = find(OPEN).condition({ chatRoomId: file.chatRoomId, eventSeq: file.eventSeq, userId: me }).forUpdate(true).one()
  if (!open || now > millis(open.openedDate) + WINDOW_MILLIS || Number(open.downloadCount || 0) >= MAX_DOWNLOADS) return missing()
  if (!ec.service.sync().name(CHAT + '.get#RoomAccess').parameter('chatRoomId', file.chatRoomId).call()?.isMember) return missing()
  open.set('downloadCount', Number(open.downloadCount || 0) + 1)
  open.update()
  return { location: file.location, fileName: file.fileName, contentType: file.contentType }
}

/** Whether a sent file belongs to a view-once message (get#Attachment serves none of those) or to one that expires. */
function fileKind(p) {
  if (p.chatRoomId == null || p.eventSeq == null || p.eventSeq === '') return {}
  const row = find(EVENT).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).selectField('viewOnce').selectField('expireDate').one()
  const expiring = !!row && row.expireDate != null && String(row.expireDate) !== ''
  return { viewOnce: !!row && row.viewOnce === 'Y', expiring, due: expiring && millis(row.expireDate) <= nowMillis(p) }
}

/**
 * get#Timeline's items of view-once messages: no attachments; viewOnce {kind, openedByMe, gone}, and for the sender
 * openedCount and recipientCount (a member whose read receipts are off is not counted as opened).
 */
function timelineView(p) {
  const me = ec.user.userId
  const items = Array.from(p.eventList || []).filter((item) => item && item.viewOnce)
  if (!items.length) return {}
  const seqs = items.map((item) => Number(item.eventSeq))
  const opens = Array.from(find(OPEN).condition('chatRoomId', p.chatRoomId).condition('eventSeq', 'in', seqs).list())
  const files = new Set(Array.from(find(ATTACHMENT).condition('chatRoomId', p.chatRoomId).condition('eventSeq', 'in', seqs).selectField('eventSeq').list())
    .map((row) => Number(row.eventSeq)))
  let off = null
  for (const item of items) {
    const seq = Number(item.eventSeq)
    const type = String(((item.attachments || [])[0] || {}).contentType || '')
    const mine = opens.filter((open) => Number(open.eventSeq) === seq)
    const state = { kind: /^video\//i.test(type) ? 'video' : /^image\//i.test(type) ? 'photo' : null,
      openedByMe: mine.some((open) => open.userId === me), gone: !files.has(seq) }
    if (item.senderUserId === me) {
      const recipients = recipientsOf(p.chatRoomId, me)
      // read receipts off on either side: the sender's own setting too, as for read markers
      if (off == null) off = receiptsOff(recipients.concat([me]))
      state.recipientCount = recipients.length
      state.openedCount = off.has(me) ? 0 : mine.filter((open) => recipients.includes(open.userId) && !off.has(open.userId)).length
    }
    item.viewOnce = state
    item.attachments = []
  }
  return {}
}

// ---------- the delete ----------

/** The rounds' work: the due events, each room on its own in its own transaction. */
function expireDue(p) {
  const now = nowMillis(p)
  const limit = Math.max(1, Number(p.limit) || Number(property('chat@expiry-round-limit')) || 2000)
  // the rooms with events due (grouped by room: one room's many rows do not fill a round; each room is emptied in its
  // turn, so the rounds come to every room), and the rooms whose backed-up sessions are due to go
  const due = Array.from(find('moqui.chat.ChatEventDueRoom').condition('expireDate', 'less-equals', new Date(now)).selectField('chatRoomId')
    .selectField('firstDue').limit(ROUND_ROOMS + 1).list())
  const more = due.length > ROUND_ROOMS
  const rooms = Array.from(new Set(due.slice(0, ROUND_ROOMS).map((row) => row.chatRoomId)))
  for (const row of find(PENDING_SESSION).condition('dueDate', 'less-equals', new Date(now)).selectField('chatRoomId').limit(ROUND_ROOMS).list())
    if (!rooms.includes(row.chatRoomId)) rooms.push(row.chatRoomId)
  // a room that failed lately on this node waits (the cache is this node's), so it cannot hold the others back
  const failed = ec.cache.getCache(FAILED_CACHE)
  let roomCount = 0, deletedCount = 0, failedCount = 0, skippedCount = 0, roomMore = false
  for (const chatRoomId of rooms) {
    const failedAt = Number(failed.get(chatRoomId) || 0)
    if (failedAt && now - failedAt < FAILED_SKIP_MILLIS) { skippedCount++; continue }
    ec.message.pushErrors()
    try {
      const out = ec.service.sync().name('org.moqui.chat.ChatExpiryServices.expire#RoomMessages').parameters({ chatRoomId, nowMillis: now, limit })
        .requireNewTransaction(true).disableAuthz().call()
      if (ec.message.hasError()) {
        failedCount++
        failed.put(chatRoomId, String(now))
        ec.logger.error(`Could not delete the disappearing messages of chat room ${chatRoomId}: ${ec.message.errorsString}`)
      } else {
        roomCount++
        if (failedAt) failed.remove(chatRoomId)
        deletedCount += Number((out && out.deletedCount) || 0)
        if (out && out.more) roomMore = true
      }
    } catch (err) {
      failedCount++
      failed.put(chatRoomId, String(now))
      ec.logger.error(`Could not delete the disappearing messages of chat room ${chatRoomId}: ${message(err)}`)
    } finally {
      ec.message.clearErrors()
      ec.message.popErrors()
    }
  }
  if (deletedCount || failedCount) ec.logger.info(`Chat expiry: ${deletedCount} events deleted in ${roomCount} rooms, ${failedCount} rooms failed, ${skippedCount} skipped`)
  return { roomCount, deletedCount, failedCount, skippedCount, more: (more || roomMore) && roomCount > 0 && failedCount === 0 }
}

/** A view-once message everyone opened: its file goes, and it stands as Opened until its own time. */
function retireViewOnce(row, now) {
  const content = parse(row.contentJson)
  for (const file of Array.from(find(ATTACHMENT).condition('chatRoomId', row.chatRoomId).condition('eventSeq', row.eventSeq).list())) {
    try { ec.service.sync().name(CHAT + '.remove#AttachmentFile').parameter('chatAttachmentId', file.chatAttachmentId).call() }
    catch (err) { ec.logger.warn(`Could not remove view-once file ${file.location}: ${message(err)}`) }
    file.delete()
  }
  const until = Number(content.viewOnceUntil) || null
  content.attachments = []
  const standsTo = until != null && until > now ? until : now
  row.setFields({ contentJson: JSON.stringify(content), expireDate: standsTo }, true, null, null)
  row.update()
  for (const field of ['editsEventSeq', 'relatesToEventSeq'])
    find(EVENT).condition('chatRoomId', row.chatRoomId).condition(field, row.eventSeq).updateAll({ expireDate: standsTo })
  viewOnceChanged(row, null)
}

/** The room's sessions whose wait is over: their backup goes, unless an event of the room uses them again. */
function pendingSessions(chatRoomId, now) {
  for (const row of Array.from(find(PENDING_SESSION).condition('chatRoomId', chatRoomId).condition('dueDate', 'less-equals', new Date(now)).list())) {
    if (!find(EVENT).condition('chatRoomId', chatRoomId).condition('megolmSessionId', row.sessionId).count())
      find(BACKUP_SESSION).condition('roomId', matrixRoomId(chatRoomId)).condition('sessionId', row.sessionId).deleteAll()
    find(PENDING_SESSION).condition({ chatRoomId, sessionId: row.sessionId }).deleteAll()
  }
}

function expireRoom(p) {
  const chatRoomId = p.chatRoomId
  const now = nowMillis(p)
  const limit = Math.max(1, Number(p.limit) || 2000)
  const room = find(ROOM).condition('chatRoomId', chatRoomId).forUpdate(true).one()
  // events of a room that is gone: deleted, with nothing of the room to change
  if (!room) {
    const deletedCount = find(EVENT).condition('chatRoomId', chatRoomId).condition('expireDate', 'less-equals', new Date(now)).deleteAll()
    // a room that is gone takes nobody out of its live locations any more: their links go (plans/Location-Sharing.md §3.5)
    find('moqui.chat.ChatLiveShare').condition('chatRoomId', chatRoomId).deleteAll()
    return { deletedCount }
  }
  const rotationMillis = Math.min(WEEK_MILLIS, (Number(room.messageTimerSeconds) || 0) * 1000 || WEEK_MILLIS)
  pendingSessions(chatRoomId, now)
  let due = Array.from(find(EVENT).condition('chatRoomId', chatRoomId).condition('expireDate', 'less-equals', new Date(now))
    .orderBy('expireDate').orderBy('eventSeq').limit(limit + 1).list())
  if (!due.length) return { deletedCount: 0 }
  const more = due.length > limit
  due = due.slice(0, limit)
  // a view-once message everyone opened before its own time: only its file goes
  const kept = []
  due = due.filter((row) => {
    if (row.viewOnce === 'Y' && Number(parse(row.contentJson).viewOnceUntil) > now
        && find(ATTACHMENT).condition('chatRoomId', chatRoomId).condition('eventSeq', row.eventSeq).count() > 0) { kept.push(row); return false }
    return true
  })
  for (const row of kept) retireViewOnce(row, now)
  if (ec.message.hasError()) return {}
  // what relates to a message that stands keeps going with it
  const standing = new Set(kept.map((row) => Number(row.eventSeq)))
  if (standing.size) due = due.filter((row) => !standing.has(Number(row.editsEventSeq)) && !standing.has(Number(row.relatesToEventSeq)))
  const seqs = due.map((row) => Number(row.eventSeq))
  const doomed = new Map(due.map((row) => [Number(row.eventSeq), row]))
  // the edits, reactions and redactions of what goes, even when theirs is later
  for (const chunk of chunks(seqs, 500))
    for (const field of ['editsEventSeq', 'relatesToEventSeq'])
      for (const row of find(EVENT).condition('chatRoomId', chatRoomId).condition(field, 'in', chunk).list())
        if (!doomed.has(Number(row.eventSeq))) doomed.set(Number(row.eventSeq), row)
  const all = Array.from(doomed.keys()).sort((a, b) => a - b)
  const counted = new Set()
  // each Megolm session of what goes, with its newest use
  const sessions = new Map()
  const roots = new Set()
  for (const row of doomed.values()) {
    if (row.megolmSessionId) sessions.set(row.megolmSessionId, Math.max(sessions.get(row.megolmSessionId) || 0, millis(row.sentDate) || 0))
    if (!MESSAGES.includes(row.eventTypeEnumId)) continue
    if (row.threadRootEventSeq != null) roots.add(Number(row.threadRootEventSeq))
    // its links' preview cache rows that no other message links
    const hashes = Array.from(find(LINK).condition('chatRoomId', chatRoomId).condition('eventSeq', row.eventSeq).selectField('urlHash').list())
      .map((link) => link.urlHash).filter((hash) => hash)
    const cleared = ec.service.sync().name(CHAT + '.clear#MessageContent').parameters({ chatRoomId, eventSeq: row.eventSeq }).call()
    if (ec.message.hasError()) return {}
    for (const id of (cleared && cleared.mentionedUserIdList) || []) counted.add(id)
    for (const hash of new Set(hashes))
      if (!find(LINK).condition('urlHash', hash).count()) find(PREVIEW).condition('urlHash', hash).deleteAll()
  }
  let deletedCount = 0
  for (const chunk of chunks(all, 500)) {
    find(ACTIVITY).condition('chatRoomId', chatRoomId).condition('eventSeq', 'in', chunk).deleteAll()
    find(OPEN).condition('chatRoomId', chatRoomId).condition('eventSeq', 'in', chunk).deleteAll()
    find(CHANGE).condition('chatRoomId', chatRoomId).condition('eventSeq', 'in', chunk).deleteAll()
    // a reminder of what went is not sent (phase 15's ChatMessageReminder: dropped, as for a message deleted)
    find(REMINDER).condition('chatRoomId', chatRoomId).condition('eventSeq', 'in', chunk).condition('statusEnumId', 'CrmPending')
      .updateAll({ statusEnumId: 'CrmDropped', reasonCode: 'messageGone' })
    deletedCount += find(EVENT).condition('chatRoomId', chatRoomId).condition('eventSeq', 'in', chunk).deleteAll()
  }
  // the thread roots that stand: their count and last reply
  for (const rootSeq of roots) {
    if (doomed.has(rootSeq)) continue
    const root = find(EVENT).condition('chatRoomId', chatRoomId).condition('eventSeq', rootSeq).one()
    if (!root) continue
    const replies = find(EVENT).condition('chatRoomId', chatRoomId).condition('threadRootEventSeq', rootSeq).condition('eventTypeEnumId', 'in', MESSAGES)
      .condition('redactedDate', null)
    const last = replies.orderBy('-eventSeq').limit(1).list()[0] || null
    root.setFields({ threadReplyCount: find(EVENT).condition('chatRoomId', chatRoomId).condition('threadRootEventSeq', rootSeq)
      .condition('eventTypeEnumId', 'in', MESSAGES).condition('redactedDate', null).count(),
    threadLastEventSeq: last ? last.eventSeq : null, threadLastUserId: last ? last.senderUserId ?? null : null, threadLastDate: last ? last.sentDate : null }, true, null, null)
    root.update()
  }
  // the room's last message is the last one that stands
  if (room.lastMessageSeq != null && doomed.has(Number(room.lastMessageSeq))) {
    const row = find(EVENT).condition('chatRoomId', chatRoomId).condition('eventTypeEnumId', 'CetMessage').condition('threadRootEventSeq', null)
      .condition('redactedDate', null).orderBy('-eventSeq').limit(1).list()[0] || null
    const text = row && row.encrypted !== 'Y' ? String(row.bodyText || '').replace(/\s+/g, ' ').trim() : ''
    room.setFields({ lastMessageSeq: row ? row.eventSeq : null, lastMessageUserId: row ? row.senderUserId ?? null : null,
      lastMessageActorId: row ? row.senderActorId ?? null : null, lastMessageDate: row ? row.sentDate : null,
      lastMessageSnippet: row && text ? (text.length > 140 ? text.slice(0, 139) + '…' : text) : null }, true, null, null)
    room.update()
  }
  // an encrypted room's key backup: a session no event of the room uses any more goes from every user's backup, once it
  // can be in use no more (its newest use past the room's rotation period and a margin); till then it waits
  // (ChatExpirySession, pendingSessions)
  for (const [sessionId, lastUsed] of sessions) {
    if (find(EVENT).condition('chatRoomId', chatRoomId).condition('megolmSessionId', sessionId).count()) continue
    const dueAt = lastUsed + rotationMillis + SESSION_MARGIN_MILLIS
    if (dueAt <= now) find(BACKUP_SESSION).condition('roomId', matrixRoomId(chatRoomId)).condition('sessionId', sessionId).deleteAll()
    else ec.entity.makeValue(PENDING_SESSION).setFields({ chatRoomId, sessionId, lastUsedDate: lastUsed, dueDate: dueAt }, true, null, null).createOrUpdate()
  }
  // the marker in the room's feed: every event of the room due at or before `before` is gone
  const before = more ? millis(due[due.length - 1] ? due[due.length - 1].expireDate : now) - 1 : now
  ec.service.sync().name(CHAT + '.record#RoomChange').parameters({ chatRoomId, eventSeq: null, change: { cause: 'expired', before } }).call()
  if (ec.message.hasError()) return {}
  // the counts, and the members' open pages
  if (all.length) {
    for (const row of find(MEMBER).condition('chatRoomId', chatRoomId).condition('leftDate', null).condition('lastReadEventSeq', 'less', all[all.length - 1])
      .selectField('userId').list()) if (row.userId) counted.add(row.userId)
    if (counted.size) ec.service.sync().name(CHAT + '.publish#ChatCounts').parameter('userIdList', Array.from(counted)).call()
    const isDirect = room.roomTypeEnumId === 'CrtDirect'
    const users = Array.from(find(MEMBER).condition('chatRoomId', chatRoomId).selectField('userId').selectField('leftDate').selectField('filterEnumId').list())
      .filter((member) => member.userId && (isDirect || !member.leftDate) && !member.filterEnumId).map((member) => member.userId)
    if (users.length) ec.service.sync().name(CHAT + '.push#Signal').parameters({ topic: 'chatRoom:' + chatRoomId, userIdList: users,
      message: { kind: 'expired', chatRoomId, before, eventSeqList: all.slice(-PUSH_SEQS) } }).call()
  }
  return { deletedCount, before, more }
}

const operations = {
  expiryOf, checkForward, checkViewOnce, checkBridge, setRoomTimer, openViewOnce, viewOnceFile, fileKind, timelineView, expireDue, expireRoom,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown chat expiry operation ' + op)
  return operations[op](parameters || {}) || {}
}

;({ execute })
