// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Invite links and join requests (plans/Comms-Messaging.md §18.3 phase 17; docs/tools/Comms.md "Invite links"): the services
// of org.moqui.chat.ChatInviteServices.
//
// - A link (ChatInviteLink) is /join/<token>, the token route "join" of basic-comms. A private room's moderators and owners
//   make them, with approval or not; any member of a public room makes them, never with approval (the room is open).
//   Direct, record and phone rooms take none. A link takes people while it is not revoked, expired or used up, its room is
//   not archived, and its maker may still make one (a moderator of the private room, a member of the public room).
// - Joining by a link (joinByInvite) needs an account: there are no chat guests. With approval, or for a person another
//   member took out of the room, it is a join request (ChatJoinRequest); otherwise the person joins, with the membership
//   event {action: 'join', via: 'link'}. A join or a request is a use of the link.
// - A private room its owner lists in Browse (ChatRoom.directoryListed) takes requests from Browse too (requestJoin).
// - The moderators hear of a request by the ChatJoinRequest notification and a push on the room's topic
//   ({kind: 'joinRequests', pendingCount}); a moderator who blocked the person neither (D1). An approval goes through
//   add#Member as the approving moderator, with all of its checks.
//
// Blocks: a link whose maker blocked the person answers as not found (SEC-COM-01). External users (EXTERNAL_USER) make no
// links and join no public room by a link.

const ROOM = 'moqui.chat.ChatRoom'
const MEMBER = 'moqui.chat.ChatRoomMember'
const LINK = 'moqui.chat.ChatInviteLink'
const REQUEST = 'moqui.chat.ChatJoinRequest'
const ACCOUNT = 'moqui.security.UserAccount'
const CHAT_BLOCKS = 'component://framework/service/org/moqui/chat/ChatBlocks.js'

/** The links in force a room has at most. */
const MAX_LINKS = 20
/** Pending requests a room has at most, and a person at most. */
const MAX_ROOM_PENDING = 100
const MAX_USER_PENDING = 10
/** Joins and requests by a link or from Browse a person makes in an hour. */
const JOINS_PER_HOUR = 30
/** A pending request expires after this many days; a denied one keeps its person from asking again this many days. */
const REQUEST_DAYS = 30
const DENIED_DAYS = 7
const DAY = 86400000
/** The choices of a link's uses, and the most hours of its expiry (30 days). */
const USE_CHOICES = [1, 5, 10, 25, 100]
const MAX_HOURS = 720

const ms = (value) => {
  if (value == null || value === '') return null
  const number = Number(value)
  return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}
const nowMs = () => ms(ec.user.nowTimestamp)
const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const rows = (entity, fields) => Array.from(find(entity).condition(fields || {}).list())
const blank = (value) => value == null || String(value).trim() === ''
const isTrue = (value) => value === true || value === 'true' || value === 'Y'
const message = (err) => (err && err.message ? err.message : String(err))

function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

const roomUrl = (chatRoomId) => (ec.factory.getConfValue('comms@app-root') || '/qapps/BasicComms') + '/Chat?chatRoomId=' + encodeURIComponent(chatRoomId)

function access(chatRoomId) {
  return ec.service.sync().name('org.moqui.chat.ChatServices.get#RoomAccess').parameter('chatRoomId', chatRoomId).call() || {}
}

function isExternal(userId) {
  return ec.service.sync().name('org.moqui.chat.ChatServices.get#ExternalUsers').parameter('userIdList', [userId])
    .disableAuthz().call()?.anyExternal !== false
}

function userNames(userIds) {
  const ids = Array.from(new Set(userIds.filter((id) => id)))
  const out = {}
  if (!ids.length) return out
  for (const row of find(ACCOUNT).condition('userId', 'in', ids).selectField('userId').selectField('userFullName').selectField('username').list()) {
    out[row.userId] = row.userFullName || row.username || row.userId
  }
  return out
}

/** The kind of room a link may be made for: private or public; null for a direct, record or phone room. */
function linkKindOf(room) {
  if (!room || room.recordEntityName || room.visibilityEnumId === 'CrvRecord') return null
  if (room.roomTypeEnumId === 'CrtPrivate') return 'private'
  if (room.roomTypeEnumId === 'CrtPublic') return 'public'
  return null
}

/** Whether userId may make a link in room now: a moderator of a private room, a member of a public room; never an external user. */
function mayMake(room, userId) {
  const kind = linkKindOf(room)
  if (!kind || room.archivedDate || !userId) return false
  // a disabled or terminated account's links take no one
  const account = one(ACCOUNT, { userId })
  if (!account || account.disabled === 'Y' || (account.terminateDate && ms(account.terminateDate) <= nowMs())) return false
  const member = one(MEMBER, { chatRoomId: room.chatRoomId, userId })
  if (!member || member.leftDate) return false
  if (kind === 'private' && !['CmrOwner', 'CmrModerator'].includes(member.roleEnumId)) return false
  return !isExternal(userId)
}

/** Whether userId moderates room (an owner or moderator, now). */
function moderates(room, userId) {
  const member = userId ? one(MEMBER, { chatRoomId: room.chatRoomId, userId }) : null
  return !!member && !member.leftDate && ['CmrOwner', 'CmrModerator'].includes(member.roleEnumId)
}

/** Why a link takes no one now (revoked, expired, usedUp, archived, maker), or null when it works. */
function linkProblem(link, room) {
  if (link.revokedDate) return 'revoked'
  if (link.expiryDate && ms(link.expiryDate) <= nowMs()) return 'expired'
  if (link.maxUses != null && Number(link.useCount || 0) >= Number(link.maxUses)) return 'usedUp'
  if (!room || room.archivedDate) return 'archived'
  if (!mayMake(room, link.createdByUserId)) return 'maker'
  return null
}

/** The link a token names (any state), found by its hash; locked when forUpdate. */
function linkByToken(token, forUpdate) {
  if (!/^[A-Za-z0-9_-]{16,64}$/.test(String(token || ''))) return null
  return find(LINK).condition('tokenHash', ec.user.singleUseCodeHash(String(token))).forUpdate(!!forUpdate).one()
}

const pathOf = (link) => (link.tokenText ? '/join/' + link.tokenText : null)

function linkView(link, names) {
  return {
    chatInviteLinkId: link.chatInviteLinkId, chatRoomId: link.chatRoomId, path: pathOf(link), label: link.label || null,
    createdByUserId: link.createdByUserId || null, createdByName: names[link.createdByUserId] || null, createdDate: ms(link.createdDate),
    expiryDate: ms(link.expiryDate), maxUses: link.maxUses != null ? Number(link.maxUses) : null, useCount: Number(link.useCount || 0),
    approvalRequired: link.approvalRequired === 'Y', lastUsedDate: ms(link.lastUsedDate),
  }
}

// ---------- links ----------

function createLink(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in to make an invite link')
  const room = one(ROOM, { chatRoomId: p.chatRoomId })
  if (!room || !access(p.chatRoomId).canRead) return refuse('notFound', 'Chat room not found')
  const kind = linkKindOf(room)
  if (!kind) return refuse('roomType', 'This room takes no invite links')
  if (room.archivedDate) return refuse('archived', 'The room is archived')
  if (!mayMake(room, me)) {
    return refuse('notAllowed', kind === 'private' ? 'Only a moderator or owner may make an invite link' : 'Join the room to make an invite link')
  }
  const inForce = rows(LINK, { chatRoomId: room.chatRoomId, revokedDate: null }).filter((link) => !linkProblem(link, room))
  if (inForce.length >= MAX_LINKS) return refuse('tooManyLinks', `A room has at most ${MAX_LINKS} invite links: revoke one first`)
  const hours = Number(p.expiryHours)
  if (p.expiryHours != null && p.expiryHours !== '' && (!Number.isFinite(hours) || hours < 0 || hours > MAX_HOURS)) {
    return refuse('badExpiry', 'An invite link lasts from an hour to 30 days, or has no end')
  }
  const uses = blank(p.maxUses) ? null : Number(p.maxUses)
  if (uses != null && !USE_CHOICES.includes(uses)) return refuse('badUses', 'Choose how many people the link takes')
  const token = ec.user.randomString(32)
  const link = ec.entity.makeValue(LINK)
  link.setSequencedIdPrimary()
  link.setFields({
    chatRoomId: room.chatRoomId, tokenHash: ec.user.singleUseCodeHash(token), tokenText: token,
    label: blank(p.label) ? null : String(p.label).trim().slice(0, 100), createdByUserId: me, createdDate: ec.user.nowTimestamp,
    // the default is a week; 0 is no end
    expiryDate: p.expiryHours == null || p.expiryHours === '' ? new Date(nowMs() + 7 * DAY) : hours === 0 ? null : new Date(nowMs() + hours * 3600000),
    maxUses: uses, useCount: 0, approvalRequired: kind === 'private' && isTrue(p.approvalRequired) ? 'Y' : 'N',
  }, true, null, null)
  link.create()
  return { chatInviteLinkId: link.chatInviteLinkId, token, path: '/join/' + token }
}

/** The links in force of a room: all of them for a moderator, a public room's member their own. */
function findLinks(p) {
  const me = ec.user.userId
  const room = me ? one(ROOM, { chatRoomId: p.chatRoomId }) : null
  const roomAccess = room ? access(p.chatRoomId) : {}
  if (!room || !roomAccess.isMember) return refuse('notFound', 'Chat room not found')
  const kind = linkKindOf(room)
  const all = moderates(room, me)
  const list = rows(LINK, { chatRoomId: room.chatRoomId, revokedDate: null })
    .filter((link) => !linkProblem(link, room) && (all || link.createdByUserId === me))
    .sort((a, b) => ms(b.createdDate) - ms(a.createdDate))
  const names = userNames(list.map((link) => link.createdByUserId))
  return {
    linkList: list.map((link) => linkView(link, names)), canMake: !!kind && mayMake(room, me), roomKind: kind,
    canList: kind === 'private' && roomAccess.isOwner === true && !room.archivedDate, listed: room.directoryListed === 'Y',
  }
}

/** A link a user may manage: its maker, or a moderator of its room. */
function manageable(chatInviteLinkId, forUpdate) {
  const me = ec.user.userId
  const link = me && !blank(chatInviteLinkId) ? find(LINK).condition('chatInviteLinkId', chatInviteLinkId).forUpdate(!!forUpdate).one() : null
  if (!link) return null
  const room = one(ROOM, { chatRoomId: link.chatRoomId })
  if (!room || !access(link.chatRoomId).isMember) return null
  return link.createdByUserId === me || moderates(room, me) ? { link, room } : null
}

/** The QR code of a link's URL (its path after baseUrl, the request's own scheme and host). */
function linkQr(p) {
  const found = manageable(p.chatInviteLinkId, false)
  if (!found || found.link.revokedDate || !pathOf(found.link)) return refuse('notFound', 'Invite link not found')
  const base = String(p.baseUrl || '').replace(/\/+$/, '')
  if (!/^https?:\/\/[A-Za-z0-9.\-]+(:[0-9]{1,5})?$/.test(base)) return refuse('badUrl', 'The server address is not known')
  const url = base + pathOf(found.link)
  try {
    return { url, qrSvg: ec.resource.barcodeSvg('QR', url, {}) }
  } catch (err) {
    // the link stands; the dialog shows it without a code
    ec.logger.error(`The QR code of invite link ${found.link.chatInviteLinkId} was not drawn: ${message(err)}`)
    return { url, qrSvg: null }
  }
}

function revokeLink(p) {
  const found = manageable(p.chatInviteLinkId, true)
  if (!found) return refuse('notFound', 'Invite link not found')
  if (found.link.revokedDate) return { revoked: true, repeated: true }
  found.link.setFields({ revokedDate: ec.user.nowTimestamp, revokedByUserId: ec.user.userId }, true, null, null)
  found.link.update()
  return { revoked: true }
}

function setListed(p) {
  const me = ec.user.userId
  const room = me ? one(ROOM, { chatRoomId: p.chatRoomId }) : null
  const roomAccess = room ? access(p.chatRoomId) : {}
  if (!room || !roomAccess.isMember) return refuse('notFound', 'Chat room not found')
  if (linkKindOf(room) !== 'private') return refuse('roomType', 'Only a private room is listed in Browse')
  if (!roomAccess.isOwner) return refuse('notAllowed', 'Only an owner may list the room in Browse')
  if (room.archivedDate) return refuse('archived', 'The room is archived')
  room.set('directoryListed', isTrue(p.listed) ? 'Y' : 'N')
  room.update()
  return { listed: room.directoryListed === 'Y' }
}

// ---------- joining ----------

/** The person's joins and requests this hour; past JOINS_PER_HOUR, refused (counted on the distributed cache). */
function rateLimited(userId) {
  const cache = ec.cache.getCache('moqui.chat.invite.joins')
  const key = userId + ':' + Math.floor(nowMs() / 3600000)
  const count = Number(cache.get(key) || 0)
  if (count >= JOINS_PER_HOUR) return true
  cache.put(key, count + 1)
  return false
}

/** The room's current moderators and owners (userIds). */
function moderatorsOf(chatRoomId) {
  return Array.from(find(MEMBER).condition('chatRoomId', chatRoomId).condition('leftDate', null)
    .condition('roleEnumId', 'in', ['CmrOwner', 'CmrModerator']).selectField('userId').list()).map((row) => row.userId).filter((id) => id)
}

/** A pending request older than REQUEST_DAYS is expired: written so. */
function expireOld(condition) {
  const cutoff = new Date(nowMs() - REQUEST_DAYS * DAY)
  find(REQUEST).condition(condition).condition('statusEnumId', 'CjrPending').condition('requestedDate', '<', cutoff)
    .updateAll({ statusEnumId: 'CjrExpired', decidedDate: ec.user.nowTimestamp })
}

function pendingCount(chatRoomId) {
  return find(REQUEST).condition({ chatRoomId, statusEnumId: 'CjrPending' }).count()
}

/** The moderators who see userId's requests: those who did not block them (D1; a failed lookup blocks no one). */
function seers(chatRoomId, userId) {
  const moderators = moderatorsOf(chatRoomId)
  const blockers = new Set(ec.resource.script(CHAT_BLOCKS).execute('blockersOf', { senderUserId: userId, userIdList: moderators }).userIdList || [])
  return moderators.filter((id) => !blockers.has(id))
}

/** The room's moderators hear the count of pending requests on the room's topic (the room's banner). */
function pushCount(chatRoomId) {
  // each moderator their own count: the requests of people they blocked are not theirs to see (D1)
  for (const userId of moderatorsOf(chatRoomId)) {
    try {
      ec.service.sync().name('org.moqui.chat.ChatServices.push#Signal').parameters({ topic: 'chatRoom:' + chatRoomId, userIdList: [userId],
        message: { kind: 'joinRequests', chatRoomId, pendingCount: visibleCount(chatRoomId, userId) } }).disableAuthz().call()
    } catch (err) {
      ec.logger.error(`The join requests count of room ${chatRoomId} was not pushed to ${userId}: ${message(err)}`)
    }
  }
}

/** The pending requests of a room that moderator userId sees (not those of people they blocked). */
function visibleCount(chatRoomId, userId) {
  const blocked = ec.resource.script(CHAT_BLOCKS).execute('blockedBy', { userId }).userIdList || []
  let finder = find(REQUEST).condition({ chatRoomId, statusEnumId: 'CjrPending' })
    .condition('requestedDate', '>=', new Date(nowMs() - REQUEST_DAYS * DAY))
  if (blocked.length) finder = finder.condition('userId', 'not-in', blocked)
  return finder.count()
}

/**
 * Whether the link's maker could not have added userId to room: they blocked them (D1), or get#RefusedUsers asked as the
 * maker refuses them (an app's chat-target provider, the maker's reach; for a private room, userId's block of the maker),
 * the check add#Member makes. Such a person is answered as for a link that does not exist (SEC-COM-01), on the join page
 * too. And an external user (EXTERNAL_USER) joins no public room by a link.
 */
function refusedBy(link, room, userId) {
  const kind = linkKindOf(room)
  if (kind === 'public' && isExternal(userId)) return true
  if ((ec.resource.script(CHAT_BLOCKS).execute('blockersOf', { senderUserId: userId, userIdList: [link.createdByUserId] }).userIdList || []).length) return true
  const refused = ec.service.sync().name('org.moqui.chat.ChatInviteServices.get#RefusedUsersAs').parameters({ userId: link.createdByUserId,
    userIdList: [userId], blocks: kind === 'private' }).disableAuthz().call()?.refusedUserIdList || [userId]
  return Array.from(refused).includes(userId)
}

/** The ChatJoinRequest notification to each moderator who sees the request, each on its own. */
function notifyModerators(room, request) {
  const name = userNames([request.userId])[request.userId] || request.userId
  for (const userId of seers(room.chatRoomId, request.userId)) {
    try {
      ec.makeNotificationMessage().topic('ChatJoinRequest').userIds([userId]).reason('approval')
        .message({ roomName: room.roomName || room.chatRoomId, chatRoomId: room.chatRoomId, chatJoinRequestId: request.chatJoinRequestId,
          requesterName: name, note: request.note || null })
        .record('moqui.chat.ChatRoom', room.chatRoomId).actorUserId(request.userId).link(roomUrl(room.chatRoomId)).send()
    } catch (err) {
      ec.logger.error(`Moderator ${userId} was not told of join request ${request.chatJoinRequestId}: ${message(err)}`)
    }
  }
  pushCount(room.chatRoomId)
}

/**
 * A request of userId to join room (from link, or from Browse with none): the one pending answers again (repeated), else
 * a new one, refused deniedRecently, tooManyRequests or queueFull. Out: {outcome: 'requested', chatJoinRequestId, repeated}.
 */
function makeRequest(room, userId, link, note) {
  expireOld({ chatRoomId: room.chatRoomId, userId })
  const open = rows(REQUEST, { chatRoomId: room.chatRoomId, userId, statusEnumId: 'CjrPending' })[0]
  if (open) return { outcome: 'requested', chatJoinRequestId: open.chatJoinRequestId, repeated: true }
  const denied = find(REQUEST).condition({ chatRoomId: room.chatRoomId, userId, statusEnumId: 'CjrDenied' })
    .condition('decidedDate', '>=', new Date(nowMs() - DENIED_DAYS * DAY)).count()
  if (denied) return refuse('deniedRecently', 'You asked to join this room lately; try again later')
  expireOld({ userId })
  if (find(REQUEST).condition({ userId, statusEnumId: 'CjrPending' }).count() >= MAX_USER_PENDING) {
    return refuse('tooManyRequests', `You have ${MAX_USER_PENDING} requests waiting: cancel one first`)
  }
  expireOld({ chatRoomId: room.chatRoomId })
  if (pendingCount(room.chatRoomId) >= MAX_ROOM_PENDING) return refuse('queueFull', 'This room has too many requests waiting; try again later')
  const request = ec.entity.makeValue(REQUEST)
  request.setSequencedIdPrimary()
  request.setFields({ chatRoomId: room.chatRoomId, userId, chatInviteLinkId: link ? link.chatInviteLinkId : null, statusEnumId: 'CjrPending',
    note: blank(note) ? null : String(note).replace(/[\u0000-\u001f\u007f]/g, ' ').trim().slice(0, 200), requestedDate: ec.user.nowTimestamp }, true, null, null)
  request.create()
  notifyModerators(room, request)
  return { outcome: 'requested', chatJoinRequestId: request.chatJoinRequestId, repeated: false }
}

/**
 * userId joins room by link: a member row as add#Member makes one (back in when they left), and the membership event
 * {action: 'join', via: 'link'}. add#Member is not called: it lets a person join only a room they can read, and an internal
 * parameter on a remote service would be a way round its checks.
 */
function admit(room, userId, link) {
  find(ROOM).condition('chatRoomId', room.chatRoomId).selectField('chatRoomId').forUpdate(true).one()
  const now = ec.user.nowTimestamp
  const fresh = one(ROOM, { chatRoomId: room.chatRoomId })
  const lastSeq = Number(fresh.lastEventSeq || 0)
  let row = find(MEMBER).condition({ chatRoomId: room.chatRoomId, userId }).forUpdate(true).one()
  if (row && !row.leftDate) return
  if (row) {
    row.setFields({ leftDate: null, joinedDate: now, roleEnumId: 'CmrMember', agentRef: null, removedByUserId: null, addedByUserId: null,
      lastReadEventSeq: Math.max(Number(row.lastReadEventSeq || 0), lastSeq) }, true, null, null)
    row.update()
  } else {
    const level = (ec.service.sync().name('org.moqui.chat.ChatServices.get#DefaultNotifyLevels').parameter('userIdList', [userId])
      .disableAuthz().call()?.levelMap || {})[userId] || (room.roomTypeEnumId === 'CrtPublic' ? 'CnlMentions' : 'CnlAll')
    const actor = (ec.service.sync().name('org.moqui.chat.ChatServices.get#LocalActors').parameter('userIdList', [userId])
      .disableAuthz().call()?.actorMap || {})[userId]
    row = ec.entity.makeValue(MEMBER)
    row.setFields({ chatRoomId: room.chatRoomId, chatActorId: actor, userId, roleEnumId: 'CmrMember', joinedDate: now, lastReadEventSeq: lastSeq,
      notificationLevelEnumId: level, isPinned: 'N', isMuted: 'N' }, true, null, null)
    row.create()
  }
  const appended = ec.service.sync().name('org.moqui.chat.ChatServices.append#Event').parameters({ chatRoomId: room.chatRoomId,
    eventTypeEnumId: 'CetMembership', senderUserId: userId, content: { action: 'join', userId, roleEnumId: 'CmrMember', byUserId: null,
      via: 'link', chatInviteLinkId: link.chatInviteLinkId }, noUserChanges: true }).disableAuthz().call()
  if (ec.message.hasError()) return
  find(MEMBER).condition({ chatRoomId: room.chatRoomId, userId }).updateAll({ lastReadEventSeq: appended.eventSeq })
  ec.service.sync().name('org.moqui.chat.ChatServices.record#UserChange').parameter('changeList', [
    { userIdList: appended.memberUserIdList, changeKind: 'room', chatRoomId: room.chatRoomId, eventSeq: appended.eventSeq, change: { reason: 'members' } },
    { userIdList: [userId], changeKind: 'membership', chatRoomId: room.chatRoomId, eventSeq: appended.eventSeq,
      change: { member: true, roleEnumId: 'CmrMember', action: 'joined', byUserId: null }, pushMessage: { kind: 'room', action: 'joined', chatRoomId: room.chatRoomId } },
  ]).disableAuthz().call()
}

/** The token check of /join/<token>: what the join page may show of a link that works; nothing is read for a bad one. */
function checkToken(p) {
  const link = linkByToken(p.token, false)
  const room = link ? one(ROOM, { chatRoomId: link.chatRoomId }) : null
  if (!link || linkProblem(link, room)) return { valid: false, routeContext: null }
  const me = ec.user.userId
  // a person the maker could not add, or blocked, sees the page a bad link gets: nothing tells them why
  if (me && refusedBy(link, room, me)) return { valid: false, routeContext: null }
  const memberCount = find(MEMBER).condition('chatRoomId', room.chatRoomId).condition('leftDate', null).condition('userId', 'not-equals', null).count()
  const context = { roomName: room.roomName || ec.l10n.localize('A room'), memberCount, roomKind: linkKindOf(room),
    approval: link.approvalRequired === 'Y', encrypted: room.encrypted === 'Y', verifiedOnly: room.verifiedDevicesOnly === 'Y', signedIn: !!me }
  if (me) {
    const member = one(MEMBER, { chatRoomId: room.chatRoomId, userId: me })
    context.member = !!member && !member.leftDate
    context.pending = !context.member && find(REQUEST).condition({ chatRoomId: room.chatRoomId, userId: me, statusEnumId: 'CjrPending' })
      .condition('requestedDate', '>=', new Date(nowMs() - REQUEST_DAYS * DAY)).count() > 0
    context.makerName = userNames([link.createdByUserId])[link.createdByUserId] || null
    if (context.member) context.roomUrl = roomUrl(room.chatRoomId)
  }
  return { valid: true, routeContext: context }
}

/**
 * The signed-in person joins by a link's token, or asks to (approval, or one another member took out). Out: outcome
 * (joined, requested, member), chatRoomId, roomUrl when in. Refused: notSignedIn, notFound, rateLimited, and makeRequest's.
 */
function joinByInvite(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in to join')
  // the link's row lock: its last use is taken once
  const link = linkByToken(p.token, true)
  const room = link ? one(ROOM, { chatRoomId: link.chatRoomId }) : null
  const notFound = () => refuse('notFound', 'This link has expired or was not found.')
  if (!link || linkProblem(link, room)) return notFound()
  const kind = linkKindOf(room)
  // a person the maker could not add (their block, the apps' chat-target providers), and an external user on a public
  // room's link: answered as not found (SEC-COM-01)
  if (refusedBy(link, room, me)) return notFound()
  const member = one(MEMBER, { chatRoomId: room.chatRoomId, userId: me })
  if (member && !member.leftDate) return { outcome: 'member', chatRoomId: room.chatRoomId, roomUrl: roomUrl(room.chatRoomId) }
  if (rateLimited(me)) return refuse('rateLimited', 'Too many joins in an hour; try again later')
  const removed = kind === 'private' && !!member && !blank(member.removedByUserId) && member.removedByUserId !== me
  const now = ec.user.nowTimestamp
  if ((kind === 'private' && link.approvalRequired === 'Y') || removed) {
    const out = makeRequest(room, me, link, p.note)
    if (ec.message.hasError()) return out
    if (!out.repeated) {
      link.setFields({ useCount: Number(link.useCount || 0) + 1, lastUsedDate: now }, true, null, null)
      link.update()
    }
    return { outcome: 'requested', chatRoomId: null, chatJoinRequestId: out.chatJoinRequestId }
  }
  admit(room, me, link)
  if (ec.message.hasError()) return {}
  link.setFields({ useCount: Number(link.useCount || 0) + 1, lastUsedDate: now }, true, null, null)
  link.update()
  return { outcome: 'joined', chatRoomId: room.chatRoomId, roomUrl: roomUrl(room.chatRoomId) }
}

/** Ask to join a private room its owner listed in Browse. */
function requestJoin(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in to ask to join')
  const room = one(ROOM, { chatRoomId: p.chatRoomId })
  if (!room || room.directoryListed !== 'Y' || linkKindOf(room) !== 'private' || room.archivedDate || isExternal(me)) return refuse('notFound', 'Chat room not found')
  const member = one(MEMBER, { chatRoomId: room.chatRoomId, userId: me })
  if (member && !member.leftDate) return { outcome: 'member', chatRoomId: room.chatRoomId }
  if (rateLimited(me)) return refuse('rateLimited', 'Too many joins in an hour; try again later')
  // the room's row lock: two requests at once make one
  find(ROOM).condition('chatRoomId', room.chatRoomId).selectField('chatRoomId').forUpdate(true).one()
  return makeRequest(room, me, null, p.note)
}

function cancelRequest(p) {
  const me = ec.user.userId
  const request = me && !blank(p.chatJoinRequestId) ? find(REQUEST).condition('chatJoinRequestId', p.chatJoinRequestId).forUpdate(true).one() : null
  if (!request || request.userId !== me) return refuse('notFound', 'Join request not found')
  if (request.statusEnumId !== 'CjrPending') return { cancelled: false }
  request.setFields({ statusEnumId: 'CjrCancelled', decidedDate: ec.user.nowTimestamp }, true, null, null)
  request.update()
  pushCount(request.chatRoomId)
  return { cancelled: true }
}

/** The pending requests of a room, for its moderators (not the requests of people the moderator blocked). */
function findRequests(p) {
  const me = ec.user.userId
  const room = me ? one(ROOM, { chatRoomId: p.chatRoomId }) : null
  if (!room || !access(p.chatRoomId).isMember || !moderates(room, me)) return refuse('notFound', 'Chat room not found')
  expireOld({ chatRoomId: room.chatRoomId })
  const blocked = new Set(ec.resource.script(CHAT_BLOCKS).execute('blockedBy', { userId: me }).userIdList || [])
  const list = Array.from(find(REQUEST).condition({ chatRoomId: room.chatRoomId, statusEnumId: 'CjrPending' }).orderBy('requestedDate').limit(MAX_ROOM_PENDING).list())
    .filter((request) => !blocked.has(request.userId))
  const names = userNames(list.map((request) => request.userId))
  const linkIds = Array.from(new Set(list.map((request) => request.chatInviteLinkId).filter((id) => id)))
  const labels = {}
  if (linkIds.length) for (const link of find(LINK).condition('chatInviteLinkId', 'in', linkIds).list()) labels[link.chatInviteLinkId] = { label: link.label || null, revoked: !!link.revokedDate }
  const userIds = list.map((request) => request.userId)
  const removed = new Set(userIds.length ? Array.from(find(MEMBER).condition('chatRoomId', room.chatRoomId).condition('userId', 'in', userIds)
    .condition('removedByUserId', 'not-equals', null).selectField('userId').list()).map((row) => row.userId) : [])
  return {
    requestList: list.map((request) => ({
      chatJoinRequestId: request.chatJoinRequestId, userId: request.userId, name: names[request.userId] || request.userId, note: request.note || null,
      requestedDate: ms(request.requestedDate), fromBrowse: !request.chatInviteLinkId,
      linkLabel: request.chatInviteLinkId ? (labels[request.chatInviteLinkId] || {}).label || null : null,
      linkRevoked: request.chatInviteLinkId ? !!(labels[request.chatInviteLinkId] || {}).revoked : false, wasRemoved: removed.has(request.userId),
    })),
  }
}

/** Approve (add#Member as this moderator) or deny one request; the person is told. */
function decideOne(chatJoinRequestId, decision) {
  const me = ec.user.userId
  const request = me && !blank(chatJoinRequestId) ? find(REQUEST).condition('chatJoinRequestId', chatJoinRequestId).forUpdate(true).one() : null
  const room = request ? one(ROOM, { chatRoomId: request.chatRoomId }) : null
  if (!request || !room || !access(request.chatRoomId).isMember || !moderates(room, me)) return refuse('notFound', 'Join request not found')
  if (request.statusEnumId !== 'CjrPending') return { decided: false, statusEnumId: request.statusEnumId }
  if (!['approve', 'deny'].includes(decision)) return refuse('badDecision', 'Approve or deny the request')
  if (decision === 'approve') {
    ec.service.sync().name('org.moqui.chat.ChatServices.add#Member').parameters({ chatRoomId: request.chatRoomId, userId: request.userId }).call()
    if (ec.message.hasError()) return {}
  }
  request.setFields({ statusEnumId: decision === 'approve' ? 'CjrApproved' : 'CjrDenied', decidedDate: ec.user.nowTimestamp, decidedByUserId: me }, true, null, null)
  request.update()
  try {
    const roomName = room.roomName || room.chatRoomId
    const nm = ec.makeNotificationMessage().topic('ChatJoinDecision').userIds([request.userId]).reason('system')
      .message({ roomName, chatRoomId: room.chatRoomId, approved: decision === 'approve' }).record('moqui.chat.ChatRoom', room.chatRoomId)
      .title(decision === 'approve' ? ec.resource.expand('You are in ${roomName}', '', { roomName })
        : ec.resource.expand('Your request to join ${roomName} was declined', '', { roomName }))
    if (decision === 'approve') nm.link(roomUrl(room.chatRoomId))
    nm.send()
  } catch (err) {
    ec.logger.error(`The person of join request ${request.chatJoinRequestId} was not told: ${message(err)}`)
  }
  pushCount(request.chatRoomId)
  return { decided: true, statusEnumId: request.statusEnumId }
}

/** One request, or each of a list on its own (in a transaction of its own): failedList names the ones that failed. */
function decide(p) {
  const list = Array.from(p.chatJoinRequestIdList || []).map((id) => String(id || '').trim()).filter((id) => id)
  if (!list.length) return decideOne(p.chatJoinRequestId, String(p.decision || ''))
  const failedList = []
  let decidedCount = 0
  for (const id of list.slice(0, MAX_ROOM_PENDING)) {
    ec.message.pushErrors()
    try {
      const out = ec.service.sync().name('org.moqui.chat.ChatInviteServices.decide#JoinRequest').parameters({ chatJoinRequestId: id, decision: p.decision })
        .requireNewTransaction(true).call() || {}
      if (ec.message.hasError()) failedList.push({ chatJoinRequestId: id, message: ec.message.errorsString })
      else if (out.decided) decidedCount++
    } catch (err) {
      ec.logger.error(`Join request ${id} was not decided: ${message(err)}`)
      failedList.push({ chatJoinRequestId: id, message: message(err) })
    } finally {
      ec.message.clearErrors()
      ec.message.popErrors()
    }
  }
  return { decided: decidedCount > 0, decidedCount, failedList }
}

/** The listed private rooms the user is not in, for Browse (none for an external user). */
function findListed(p) {
  const me = ec.user.userId
  if (!me || isExternal(me)) return { roomList: [] }
  let finder = find(ROOM).condition('directoryListed', 'Y').condition('roomTypeEnumId', 'CrtPrivate').condition('archivedDate', null)
  const term = String(p.searchText || '').trim()
  if (term) {
    const cf = ec.entity.conditionFactory
    finder = finder.condition(cf.makeCondition(cf.makeCondition('roomName', 'like', '%' + term + '%').ignoreCase(), 'or',
      cf.makeCondition('topic', 'like', '%' + term + '%').ignoreCase()))
  }
  const rooms = Array.from(finder.orderBy('roomName').limit(200).list())
  if (!rooms.length) return { roomList: [] }
  const roomIds = rooms.map((room) => room.chatRoomId)
  const mine = new Set(Array.from(find(MEMBER).condition('userId', me).condition('leftDate', null).condition('chatRoomId', 'in', roomIds)
    .selectField('chatRoomId').list()).map((row) => row.chatRoomId))
  const shown = rooms.filter((room) => !mine.has(room.chatRoomId)).slice(0, 50)
  const shownIds = shown.map((room) => room.chatRoomId)
  const pending = new Set(shown.length ? Array.from(find(REQUEST).condition('userId', me).condition('statusEnumId', 'CjrPending')
    .condition('chatRoomId', 'in', shownIds).selectField('chatRoomId').list()).map((row) => row.chatRoomId) : [])
  // the members of every room shown, in one query, counted by room
  const counts = {}
  if (shownIds.length) {
    for (const row of find(MEMBER).condition('chatRoomId', 'in', shownIds).condition('leftDate', null).condition('userId', 'not-equals', null)
      .selectField('chatRoomId').list()) counts[row.chatRoomId] = (counts[row.chatRoomId] || 0) + 1
  }
  return {
    roomList: shown.map((room) => ({
      chatRoomId: room.chatRoomId, roomName: room.roomName || null, topic: room.topic || null, pending: pending.has(room.chatRoomId),
      memberCount: counts[room.chatRoomId] || 0,
    })),
  }
}

/** The count of pending requests a moderator sees in a room (the banner), 0 for anyone else. */
function requestCount(p) {
  const me = ec.user.userId
  const room = me ? one(ROOM, { chatRoomId: p.chatRoomId }) : null
  if (!room || !moderates(room, me)) return { pendingCount: 0 }
  return { pendingCount: visibleCount(room.chatRoomId, me) }
}

const operations = {
  createLink, findLinks, linkQr, revokeLink, setListed, checkToken, joinByInvite, requestJoin, cancelRequest, findRequests, decide,
  findListed, requestCount,
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown invite operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
