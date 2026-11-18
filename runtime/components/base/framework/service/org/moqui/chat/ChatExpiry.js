// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Disappearing messages and view-once media (plans/Comms-Messaging.md §18.3 phase 16; docs/tools/Comms.md "Disappearing
// messages"): the services of org.moqui.chat.ChatExpiryServices and the helpers ChatServices calls through
// ec.resource.script.
//
// - A room's timer (ChatRoom.messageTimerSeconds, setRoomTimer): a new message goes that long after it is sent, for
//   everyone. append#Event asks expiryOf for an event's expireDate: a message's sentDate plus the timer; an edit, reaction,
//   redaction or pin of a message the message's own, so they are due together. A view-once message's is at the latest
//   chat_view_once_days after it is sent (content.viewOnceUntil keeps that time).
// - The delete (expireDue, expireRoom): the due rows by the index CHAT_EVENT_EXPIRE, each room in its own transaction.
//   A message goes with what clear#MessageContent clears, its preview cache rows no other link has, its thread root's
//   count, the room's last message, its room changes (one marker change stands for them: eventSeq empty, {cause:
//   'expired', before}) and, in an encrypted room, the key backup of a Megolm session no event of the room uses any more.
//   The members get an 'expired' push and their counts.
// - View-once (openViewOnce, viewOnceFile): each recipient opens it once, with a window of WINDOW_MILLIS and at most
//   MAX_DOWNLOADS downloads; its sender never gets the file. Once every recipient opened it, the message is due at the
//   end of that window: its file goes then and the message stands, as Opened, to its viewOnceUntil.
// - Bridges: a bridged, phone or federated room gets no timer and no view-once (a bridge cannot delete there), and a
//   room with a timer is not bridged.

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

const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const blank = (value) => value == null || String(value).trim() === ''
const property = (name) => String(System.getProperty(name) || '').trim()
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
  return Math.max(1, Number(property('chat_view_once_days')) || 14)
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
  return '!' + chatRoomId + ':' + (property('chat_server_name') || 'localhost').toLowerCase()
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
  const room = one(ROOM, { chatRoomId: p.chatRoomId })
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

function viewOnceMessage(chatRoomId, eventSeq) {
  const row = one(EVENT, { chatRoomId, eventSeq })
  return row && row.viewOnce === 'Y' && MESSAGES.includes(row.eventTypeEnumId) && !row.redactedDate ? row : null
}

function openViewOnce(p) {
  const me = ec.user.userId
  const access = ec.service.sync().name(CHAT + '.get#RoomAccess').parameter('chatRoomId', p.chatRoomId).call()
  if (ec.message.hasError()) return {}
  const row = access && access.isMember ? viewOnceMessage(p.chatRoomId, p.eventSeq) : null
  if (!row) { ec.message.addError(`Message ${p.eventSeq} not found`); return {} }
  if (row.senderUserId === me) return refuse('sender', 'Your view-once photo is for the others in the room')
  const file = find(ATTACHMENT).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).orderBy('attachmentSeq').list()[0]
  if (!file) return refuse('gone', 'This view-once photo is no longer here')
  const now = nowMillis(p)
  let open = find(OPEN).condition({ chatRoomId: p.chatRoomId, eventSeq: p.eventSeq, userId: me }).forUpdate(true).one()
  if (open) {
    const opened = millis(open.openedDate)
    if (now > opened + WINDOW_MILLIS || Number(open.downloadCount || 0) >= MAX_DOWNLOADS) return refuse('opened', 'You opened this view-once photo already')
  } else {
    open = ec.entity.makeValue(OPEN)
    open.setFields({ chatRoomId: p.chatRoomId, eventSeq: p.eventSeq, userId: me, openedDate: now, downloadCount: 0 }, true, null, null)
    open.create()
    // the sender's Opened, and the opener's other devices drop it: one change of the message
    ec.service.sync().name(CHAT + '.push#Update').parameters({ chatRoomId: p.chatRoomId, eventSeq: Number(p.eventSeq), fields: { viewOnceChanged: true } }).call()
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
  if (!file || file.eventSeq == null || !viewOnceMessage(file.chatRoomId, file.eventSeq)) return missing()
  const open = find(OPEN).condition({ chatRoomId: file.chatRoomId, eventSeq: file.eventSeq, userId: me }).forUpdate(true).one()
  const now = nowMillis(p)
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
  return { viewOnce: !!row && row.viewOnce === 'Y', expiring: !!row && row.expireDate != null && String(row.expireDate) !== '' }
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
      if (off == null) off = new Set(recipients.length ? ec.service.sync().name(CHAT + '.get#ReceiptsOffUsers').parameter('userIdList', recipients).call()?.offUserIdList || [] : [])
      state.recipientCount = recipients.length
      state.openedCount = mine.filter((open) => recipients.includes(open.userId) && !off.has(open.userId)).length
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
  const limit = Math.max(1, Number(p.limit) || Number(property('chat_expiry_round_limit')) || 2000)
  const due = Array.from(find(EVENT).condition('expireDate', 'less-equals', new Date(now)).selectField('chatRoomId').selectField('eventSeq')
    .orderBy('expireDate').limit(limit + 1).list())
  const more = due.length > limit
  const rooms = Array.from(new Set(due.slice(0, limit).map((row) => row.chatRoomId)))
  let roomCount = 0, deletedCount = 0, failedCount = 0, roomMore = false
  for (const chatRoomId of rooms) {
    ec.message.pushErrors()
    try {
      const out = ec.service.sync().name('org.moqui.chat.ChatExpiryServices.expire#RoomMessages').parameters({ chatRoomId, nowMillis: now, limit })
        .requireNewTransaction(true).disableAuthz().call()
      if (ec.message.hasError()) {
        failedCount++
        ec.logger.error(`Could not delete the disappearing messages of chat room ${chatRoomId}: ${ec.message.errorsString}`)
      } else {
        roomCount++
        deletedCount += Number((out && out.deletedCount) || 0)
        if (out && out.more) roomMore = true
      }
    } catch (err) {
      failedCount++
      ec.logger.error(`Could not delete the disappearing messages of chat room ${chatRoomId}: ${message(err)}`)
    } finally {
      ec.message.clearErrors()
      ec.message.popErrors()
    }
  }
  if (deletedCount || failedCount) ec.logger.info(`Chat expiry: ${deletedCount} events deleted in ${roomCount} rooms, ${failedCount} rooms failed`)
  return { roomCount, deletedCount, failedCount, more: (more || roomMore) && roomCount > 0 }
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
  ec.service.sync().name(CHAT + '.push#Update').parameters({ chatRoomId: row.chatRoomId, eventSeq: Number(row.eventSeq), fields: { viewOnceChanged: true } }).call()
}

function expireRoom(p) {
  const chatRoomId = p.chatRoomId
  const now = nowMillis(p)
  const limit = Math.max(1, Number(p.limit) || 2000)
  const room = find(ROOM).condition('chatRoomId', chatRoomId).forUpdate(true).one()
  if (!room) return { deletedCount: 0 }
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
  const sessions = new Set()
  const roots = new Set()
  for (const row of doomed.values()) {
    if (row.megolmSessionId) sessions.add(row.megolmSessionId)
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
  // an encrypted room's key backup: a session no event of the room uses any more goes from every user's backup
  if (sessions.size) {
    const roomId = matrixRoomId(chatRoomId)
    for (const sessionId of sessions)
      if (!find(EVENT).condition('chatRoomId', chatRoomId).condition('megolmSessionId', sessionId).count())
        find(BACKUP_SESSION).condition('roomId', roomId).condition('sessionId', sessionId).deleteAll()
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
