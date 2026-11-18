// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Sticker packs (plans/Comms-Messaging.md §18.3 phase 20): the services of org.moqui.chat.ChatStickerServices. The server's
// packs (no owner) are kept by a holder of ChatStickerAdmin and offered to every chat user; a user's own are theirs alone.
// A sticker is a PNG, WebP or GIF picture (animated ones too) stored as content under chat/stickers@root, at most
// chat/stickers@max-bytes: its type is read from its bytes (whatever the name or the browser said), its sides (at most
// chat/stickers@max-side, 512 px) from its header before it is decoded, and it is decoded within a small memory limit
// (ec.resource.makeStickerThumbnail, which also makes its still 128 px thumbnail). A user adds at most
// chat/stickers@uploads-per-hour (60) stickers an hour.
// Sending one copies its picture into the room as an ordinary ChatAttachment (stickerOf), so every path files take works
// for it (access by the room, retention, redaction, disappearing messages, the bridges' files and MMS), removing a pack
// never breaks a message, and a user's pack stays theirs. The message's body is its words ("Sticker 👍") for search,
// notifications and the room list; a bridge that sends files sends the picture alone (chat_bridge.rs request_of). In an
// encrypted room the browser seals the picture and sends it as any encrypted file. No outside service.

const PACK = 'moqui.chat.ChatStickerPack'
const STICKER = 'moqui.chat.ChatSticker'
const ATTACHMENT = 'moqui.chat.ChatAttachment'
const EVENT = 'moqui.chat.ChatEvent'
const TYPES = { 'image/png': 'png', 'image/webp': 'webp', 'image/gif': 'gif' }

const find = (entity) => ec.entity.find(entity).disableAuthz()
const blank = (value) => value == null || String(value).trim() === ''
const localize = (text) => ec.l10n.localize(text)
const expand = (text, values) => ec.resource.expand(localize(text), '', values)
const escape = (text) => String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
const words = (value) => String(value ?? '').replace(/[\u0000-\u001f\u007f]+/g, ' ').replace(/\s+/g, ' ').trim()
const parse = (text) => { if (!text) return {}; try { const value = JSON.parse(text); return value && typeof value === 'object' ? value : {} } catch (err) { return {} } }
const message = (err) => (err && err.message ? err.message : String(err))
// a limit by its conf selector (chat/stickers@max-bytes), the fallback when it is not above 0
const property = (name, fallback) => { const value = Number(ec.factory.getConfValue(name)); return value > 0 ? value : fallback }
const limits = () => ({ maxBytes: property('chat/stickers@max-bytes', 512 * 1024), packStickers: property('chat/stickers@pack-max', 200),
  serverPacks: property('chat/stickers@server-packs-max', 50), userPacks: property('chat/stickers@user-packs-max', 10),
  userPackStickers: property('chat/stickers@user-pack-max', 100), maxSide: property('chat/stickers@max-side', 512),
  perHour: property('chat/stickers@uploads-per-hour', 60) })
const root = () => (ec.factory.getConfValue('chat/stickers@root') || 'dbresource://chat-stickers').replace(/\/+$/, '')
const isAdmin = () => ec.user.hasPermission('ChatStickerAdmin') === true

function refuse(reasonCode, text) {
  ec.message.addError(text)
  return { reasonCode }
}

/** May the current user change this pack: its owner, or a sticker admin for a server pack. */
function mayEdit(pack) {
  if (!pack) return false
  if (pack.ownerUserId) return pack.ownerUserId === ec.user.userId
  return isAdmin()
}
/** May the current user send from (or see) this pack: their own, or an offered server pack. */
function mayUse(pack) {
  if (!pack) return false
  if (pack.ownerUserId) return pack.ownerUserId === ec.user.userId
  return pack.statusEnumId === 'CspActive'
}

/** A pack the caller may change, or a refusal (not found: another user's is never told). */
function editablePack(chatStickerPackId) {
  const pack = find(PACK).condition('chatStickerPackId', chatStickerPackId).forUpdate(true).one()
  if (!mayEdit(pack)) return refuse('notFound', expand('Sticker pack ${id} not found', { id: chatStickerPackId }))
  return { pack }
}

const stickerOut = (row) => ({ chatStickerId: row.chatStickerId, emoji: row.emoji ?? null, altText: row.altText ?? null, width: row.width != null ? Number(row.width) : null,
  height: row.height != null ? Number(row.height) : null, contentType: row.contentType ?? null })

/**
 * get#StickerPacks: the caller's own packs, then the server's offered ones (manage: every server pack, for a sticker
 * admin, and each pack's status), each with its stickers; version is the newest change, so Chat reads them again only
 * when they changed.
 */
function packs(p) {
  const me = ec.user.userId
  const manage = p.manage === true || p.manage === 'true'
  const own = Array.from(find(PACK).condition('ownerUserId', me).orderBy('sequenceNum').orderBy('createdDate').list())
  let server = find(PACK).condition('ownerUserId', null)
  if (!(manage && isAdmin())) server = server.condition('statusEnumId', 'CspActive')
  const rows = own.filter((row) => manage || row.statusEnumId === 'CspActive').concat(Array.from(server.orderBy('sequenceNum').orderBy('createdDate').list()))
  const ids = rows.map((row) => row.chatStickerPackId)
  const byPack = {}
  let version = 0
  if (ids.length) {
    for (const row of find(STICKER).condition('chatStickerPackId', 'in', ids).orderBy('sequenceNum').orderBy('createdDate').list()) {
      (byPack[row.chatStickerPackId] = byPack[row.chatStickerPackId] || []).push(stickerOut(row))
      version = Math.max(version, Number(row.lastUpdatedStamp || row.createdDate || 0))
    }
  }
  for (const row of rows) version = Math.max(version, Number(row.lastUpdatedStamp || row.createdDate || 0))
  return {
    packList: rows.map((row) => ({ chatStickerPackId: row.chatStickerPackId, packName: row.packName ?? '', own: row.ownerUserId === me,
      hidden: row.statusEnumId === 'CspHidden', coverStickerId: row.coverStickerId || ((byPack[row.chatStickerPackId] || [])[0] || {}).chatStickerId || null,
      stickers: byPack[row.chatStickerPackId] || [] })),
    version, canManageServer: isAdmin()
  }
}

/** create#StickerPack: a user's own pack, or (server, a sticker admin) the server's. */
function createPack(p) {
  const name = words(p.packName).slice(0, 60)
  if (!name) return refuse('badName', localize('A pack needs a name'))
  const server = p.server === true || p.server === 'true'
  if (server && !isAdmin()) return refuse('notAllowed', localize('Only a sticker administrator keeps the server\'s packs'))
  const lim = limits()
  const count = server ? find(PACK).condition('ownerUserId', null).count() : find(PACK).condition('ownerUserId', ec.user.userId).count()
  if (count >= (server ? lim.serverPacks : lim.userPacks)) return refuse('tooMany', expand('At most ${max} packs', { max: server ? lim.serverPacks : lim.userPacks }))
  const row = ec.entity.makeValue(PACK)
  row.setSequencedIdPrimary()
  row.setFields({ packName: name, ownerUserId: server ? null : ec.user.userId, statusEnumId: 'CspActive', sequenceNum: count + 1,
    createdByUserId: ec.user.userId, createdDate: ec.user.nowTimestamp }, true, null, null)
  row.create()
  return { chatStickerPackId: row.chatStickerPackId }
}

/** update#StickerPack: its name, whether it is offered, its place, its cover. */
function updatePack(p) {
  const got = editablePack(p.chatStickerPackId)
  if (!got.pack) return got
  const pack = got.pack
  if (p.packName != null) {
    const name = words(p.packName).slice(0, 60)
    if (!name) return refuse('badName', localize('A pack needs a name'))
    pack.set('packName', name)
  }
  if (p.hidden != null) pack.set('statusEnumId', p.hidden === true || p.hidden === 'true' ? 'CspHidden' : 'CspActive')
  if (p.sequenceNum != null) pack.set('sequenceNum', Number(p.sequenceNum))
  if (p.coverStickerId != null) {
    if (!find(STICKER).condition({ chatStickerId: p.coverStickerId, chatStickerPackId: pack.chatStickerPackId }).count()) return refuse('notFound', localize('That sticker is not in the pack'))
    pack.set('coverStickerId', p.coverStickerId)
  }
  pack.update()
  return {}
}

/** The files of a sticker removed, each on its own; one that cannot be removed is logged. */
function removeFiles(row) {
  for (const location of [row.contentLocation, row.thumbLocation]) {
    if (blank(location)) continue
    try {
      ec.resource.getLocationReference(location).delete()
    } catch (err) {
      ec.logger.error(`Could not remove the sticker file ${location}: ${message(err)}`)
    }
  }
}

/** delete#StickerPack: the pack, its stickers and their files (each on its own). A sticker already sent stays. */
function deletePack(p) {
  const got = editablePack(p.chatStickerPackId)
  if (!got.pack) return got
  for (const row of Array.from(find(STICKER).condition('chatStickerPackId', p.chatStickerPackId).list())) {
    removeFiles(row)
    row.delete()
  }
  got.pack.delete()
  return {}
}

/**
 * add#Sticker: a picture into a pack the caller may change: PNG, WebP or GIF, at most chat/stickers@max-bytes, that
 * decodes (its still thumbnail made); the same picture once a pack; emoji (one, for search and its words) and altText.
 */
function addSticker(p) {
  const got = editablePack(p.chatStickerPackId)
  if (!got.pack) return got
  const pack = got.pack
  const file = p.file
  if (!file) return refuse('noFile', localize('No picture uploaded'))
  const lim = limits()
  const size = Number(file.getSize() || 0)
  if (size <= 0) return refuse('noFile', localize('No picture uploaded'))
  if (size > lim.maxBytes) return refuse('tooLarge', expand('A sticker is at most ${kb} KB', { kb: Math.floor(lim.maxBytes / 1024) }))
  const most = pack.ownerUserId ? lim.userPackStickers : lim.packStickers
  if (find(STICKER).condition('chatStickerPackId', pack.chatStickerPackId).count() >= most) return refuse('tooMany', expand('A pack has at most ${max} stickers', { max: most }))
  const hourAgo = new Date(Number(ec.user.nowTimestamp) - 3600 * 1000)
  if (find(STICKER).condition('createdByUserId', ec.user.userId).condition('createdDate', '>=', hourAgo).count() >= lim.perHour) {
    return refuse('tooMany', expand('At most ${max} stickers an hour; try again later', { max: lim.perHour }))
  }
  const emoji = words(p.emoji).slice(0, 16)
  const altText = words(p.altText).slice(0, 80)
  const row = ec.entity.makeValue(STICKER)
  row.setSequencedIdPrimary()
  const base = `${root()}/${pack.chatStickerPackId}/${row.chatStickerId}`
  const upload = `${base}/upload`
  const thumbLocation = `${base}/thumb.png`
  file.write(upload)
  // its type from its bytes and its sides from its header, before it is decoded (a decompression bomb is refused unread)
  const checked = ec.resource.makeStickerThumbnail(upload, thumbLocation, lim.maxSide, 128) || {}
  if (checked.reasonCode || !TYPES[checked.contentType]) {
    removeFiles({ contentLocation: upload, thumbLocation })
    if (checked.reasonCode === 'tooLarge') return refuse('tooLarge', expand('A sticker is at most ${side} px a side', { side: lim.maxSide }))
    return refuse('badType', localize('A sticker is a PNG, WebP or GIF picture'))
  }
  const type = checked.contentType
  const location = `${base}/sticker.${TYPES[type]}`
  ec.resource.getLocationReference(upload).move(location)
  const thumb = { width: checked.thumbWidth, height: checked.thumbHeight }
  const digest = ec.resource.digest(location, 'sha256')
  if (digest && find(STICKER).condition({ chatStickerPackId: pack.chatStickerPackId, digest }).count()) {
    removeFiles({ contentLocation: location, thumbLocation })
    return refuse('duplicate', localize('That picture is in the pack already'))
  }
  row.setFields({ chatStickerPackId: pack.chatStickerPackId, sequenceNum: find(STICKER).condition('chatStickerPackId', pack.chatStickerPackId).count() + 1,
    emoji: emoji || null, altText: altText || null, contentLocation: location, thumbLocation, contentType: type, fileSize: size,
    width: Number(thumb.width), height: Number(thumb.height), digest: digest || null, createdByUserId: ec.user.userId,
    createdDate: ec.user.nowTimestamp }, true, null, null)
  row.create()
  return { chatStickerId: row.chatStickerId, sticker: stickerOut(row) }
}

/** The sticker and its pack, the caller allowed to change it; or a refusal. */
function editableSticker(chatStickerId) {
  const row = find(STICKER).condition('chatStickerId', chatStickerId).forUpdate(true).one()
  const pack = row ? find(PACK).condition('chatStickerPackId', row.chatStickerPackId).one() : null
  if (!row || !mayEdit(pack)) return refuse('notFound', expand('Sticker ${id} not found', { id: chatStickerId }))
  return { row, pack }
}

/** update#Sticker: its emoji, description and place in the pack. */
function updateSticker(p) {
  const got = editableSticker(p.chatStickerId)
  if (!got.row) return got
  if (p.emoji != null) got.row.set('emoji', words(p.emoji).slice(0, 16) || null)
  if (p.altText != null) got.row.set('altText', words(p.altText).slice(0, 80) || null)
  if (p.sequenceNum != null) got.row.set('sequenceNum', Number(p.sequenceNum))
  got.row.update()
  return {}
}

/** remove#Sticker: the sticker and its files. A copy already sent stays. */
function removeSticker(p) {
  const got = editableSticker(p.chatStickerId)
  if (!got.row) return got
  removeFiles(got.row)
  if (got.pack.coverStickerId === got.row.chatStickerId) {
    const pack = find(PACK).condition('chatStickerPackId', got.pack.chatStickerPackId).forUpdate(true).one()
    pack.set('coverStickerId', null)
    pack.update()
  }
  got.row.delete()
  return {}
}

/** get#StickerImage: where a sticker's picture (or its thumbnail) is, for one who may use its pack. */
function image(p) {
  const row = find(STICKER).condition('chatStickerId', p.chatStickerId).one()
  const pack = row ? find(PACK).condition('chatStickerPackId', row.chatStickerPackId).one() : null
  // a hidden server pack stays readable to its admins (the manage screen)
  if (!row || !(mayUse(pack) || mayEdit(pack))) return Object.assign(refuse('notFound', expand('Sticker ${id} not found', { id: p.chatStickerId })), { location: null })
  const thumb = p.thumb === true || p.thumb === 'true'
  return { location: thumb ? row.thumbLocation : row.contentLocation, contentType: thumb ? 'image/png' : row.contentType }
}

/**
 * send#Sticker: a sticker the caller may use, to a plain room they may post in: its picture copied as the room's file
 * (stickerOf, counted in the day's file bytes), then send#Message with its words and that file, then content.sticker
 * and the update pushed.
 */
function send(p) {
  const out = { eventSeq: null, event: null, duplicate: false, reasonCode: null }
  const me = ec.user.userId
  const access = ec.service.sync().name('org.moqui.chat.ChatServices.get#RoomAccess').parameter('chatRoomId', p.chatRoomId).call()
  if (ec.message.hasError()) return out
  if (!access || !access.canRead) return Object.assign(out, refuse('notFound', expand('Chat room ${id} not found', { id: p.chatRoomId })))
  if (!access.canPost) return Object.assign(out, refuse(access.isArchived ? 'archived' : 'notMember', access.isArchived ? localize('The room is archived') : localize('Join the room to post in it')))
  if (access.room && access.room.encrypted) return Object.assign(out, refuse('encryptedRoom', localize('In an encrypted room your device sends the sticker, encrypted')))
  const sticker = find(STICKER).condition('chatStickerId', p.chatStickerId).one()
  const pack = sticker ? find(PACK).condition('chatStickerPackId', sticker.chatStickerPackId).one() : null
  if (!sticker || !mayUse(pack)) return Object.assign(out, refuse('notFound', expand('Sticker ${id} not found', { id: p.chatStickerId })))
  // a retry with the same clientRef answers the message it made, with no second copy
  if (p.clientRef) {
    const done = find(EVENT).condition({ chatRoomId: p.chatRoomId, senderUserId: me, clientRef: String(p.clientRef) }).one()
    if (done) {
      const again = ec.service.sync().name('org.moqui.chat.ChatServices.send#Message').parameters({ chatRoomId: p.chatRoomId, body: '<p>x</p>',
        clientRef: String(p.clientRef) }).call()
      return Object.assign(out, { eventSeq: again.eventSeq, event: again.event, duplicate: true })
    }
  }
  const size = Number(sticker.fileSize || 0)
  const daily = Number(ec.factory.getConfValue('chat@attachment-daily-bytes') || 1073741824)
  const dayAgo = new Date(Number(ec.user.nowTimestamp) - 86400000)
  const today = Array.from(find(ATTACHMENT).condition('uploadedByUserId', me).condition('uploadedDate', '>=', dayAgo).selectField('fileSize').list())
    .reduce((sum, one) => sum + Number(one.fileSize || 0), 0)
  if (today + size > daily) return Object.assign(out, refuse('tooMany', localize('Too many files today; try again later')))
  const extension = TYPES[sticker.contentType] || 'png'
  const file = ec.entity.makeValue(ATTACHMENT)
  file.setSequencedIdPrimary()
  const chatRoot = (ec.factory.getConfValue('chat@attachment-root') || 'dbresource://chat').replace(/\/+$/, '')
  const location = `${chatRoot}/${p.chatRoomId}/${file.chatAttachmentId}/sticker.${extension}`
  ec.resource.getLocationReference(location).copyFrom(sticker.contentLocation)
  file.setFields({ chatRoomId: p.chatRoomId, location, fileName: 'sticker.' + extension, contentType: sticker.contentType, fileSize: size,
    uploadedByUserId: me, uploadedDate: ec.user.nowTimestamp, stickerOf: sticker.chatStickerId }, true, null, null)
  file.create()
  const label = sticker.emoji ? localize('Sticker') + ' ' + sticker.emoji : localize('Sticker')
  const sent = ec.service.sync().name('org.moqui.chat.ChatServices.send#Message').parameters({ chatRoomId: p.chatRoomId, body: '<p>' + escape(label) + '</p>',
    chatAttachmentIdList: [file.chatAttachmentId], replyToEventSeq: p.replyToEventSeq ?? null, threadRootEventSeq: p.threadRootEventSeq ?? null,
    clientRef: p.clientRef || null, roomUrl: p.roomUrl || null }).call()
  if (ec.message.hasError()) return Object.assign(out, { reasonCode: (sent && sent.reasonCode) || null })
  out.eventSeq = sent.eventSeq
  out.event = sent.event
  if (sent.duplicate) return Object.assign(out, { duplicate: true })
  const content = { chatStickerId: sticker.chatStickerId, packName: pack.packName ?? null, emoji: sticker.emoji ?? null, altText: sticker.altText ?? null,
    width: sticker.width != null ? Number(sticker.width) : null, height: sticker.height != null ? Number(sticker.height) : null }
  const row = ec.entity.find(EVENT).condition('chatRoomId', p.chatRoomId).condition('eventSeq', sent.eventSeq).disableAuthz().one()
  const fields = parse(row.contentJson)
  fields.sticker = content
  row.set('contentJson', JSON.stringify(fields))
  row.update()
  out.event = Object.assign({}, sent.event, { sticker: content })
  ec.service.sync().name('org.moqui.chat.ChatServices.push#Update').parameters({ chatRoomId: p.chatRoomId, eventSeq: Number(sent.eventSeq),
    fields: { sticker: content } }).disableAuthz().call()
  return out
}

const operations = { packs, createPack, updatePack, deletePack, addSticker, updateSticker, removeSticker, image, send }

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown sticker operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
