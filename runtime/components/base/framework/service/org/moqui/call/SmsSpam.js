// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// The spam filter of texts from outside numbers, and the blocks that keep a text out of sight (plans/Comms-Messaging.md
// §18.3 phase 11; docs/interfaces/SMS.md "Blocked numbers and spam"): the services of org.moqui.call.SmsSpamServices.
//
// A text Sms.js receive takes in is judged first (judge, in a transaction of its own), and what comes out is a filter for
// each user our number reaches (memberFilterMap), which ensure#PairRoom puts on their member rows: CmfBlocked for a user
// with the sender on their Blocked list (a pair room; a group text is never filtered by one number), CmfSpam for the
// others whose filter is on (CallHandlingSetting.spamFilter) when the text is spam. A filtered room is out of the user's
// sight: kept, listed only under Spam & blocked, counting and notifying nothing (ChatBlocks.js). Nothing is dropped: a
// number of ours may reach several users, and Not spam brings a text back.
//
// The score of a text is the sum of the weights of the SmsSpamRule rows it matches; spam at VoicePolicy.smsSpamThreshold
// (100). A text is judged only while the filter is on, the room has no message from a local sender (once we replied it is
// a conversation), and the number is neither trusted (more not-spam votes than spam votes, SmsSpamVote) nor known (on a
// member's own caller list, a user's mobile, or named by an app's call-context provider: for a new conversation only, as
// the room's first member). The provider's lookup (lookup#BridgeNumber: the line type) is asked only with
// VoicePolicy.smsSpamLookup on, for a first contact in E.164, when its answer can change the verdict, with no answer kept
// younger than smsSpamLookupDays, under smsSpamLookupMaxPerDay.
//
// Not spam (markNotSpam) clears the room's Spam for every member, votes not-spam and counts the rules; Report spam
// (reportSpam) blocks the pair room's number for the user (CallHandling.js blockNumber: its calls too) or a group text's
// room, votes spam, counts the rules and puts the room in Spam for the other members whose filter is on. Failure tolerant:
// a judge that fails lets the text in unfiltered, a provider or a lookup that fails adds nothing.

const NUMBER = 'moqui.call.PhoneNumber'
const POLICY = 'moqui.call.VoicePolicy'
const RULE = 'moqui.call.SmsSpamRule'
const KNOWN = 'moqui.call.SmsSpamNumber'
const VOTE = 'moqui.call.SmsSpamVote'
const COUNT = 'moqui.call.SmsSendCount'
const LIST = 'moqui.call.CallerList'
const ENTRY = 'moqui.call.CallerListEntry'
const SETTING = 'moqui.call.CallHandlingSetting'
const MEMBER = 'moqui.chat.ChatRoomMember'
const ROOM = 'moqui.chat.ChatRoom'
const EVENT = 'moqui.chat.ChatEvent'
const MAPPING = 'moqui.chat.ChatRoomBridge'
const SMS = 'component://framework/service/org/moqui/call/Sms.js'
const CALL_HANDLING = 'component://framework/service/org/moqui/call/CallHandling.js'
const CHAT_BLOCKS = 'component://framework/service/org/moqui/chat/ChatBlocks.js'
const TYPES = ['SsrFirstContact', 'SsrLink', 'SsrLinkHost', 'SsrKeyword', 'SsrNumber', 'SsrReported', 'SsrLineType']
/** The most of a text the rules read. */
const TEXT_READ = 2000
const DEFAULTS = { smsSpamThreshold: 100, smsSpamLookupDays: 30, smsSpamLookupMaxPerDay: 100 }

const ms = (value) => {
  if (value == null || value === '') return null
  const number = Number(value)
  return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}
const nowMs = () => ms(ec.user.nowTimestamp)
const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const blank = (value) => value == null || String(value).trim() === ''
const yes = (value) => value === true || value === 'Y' || value === 'true'
const isE164 = (number) => /^\+[0-9]{5,15}$/.test(String(number || ''))
const message = (err) => (err && err.message ? err.message : String(err))

function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}
function script(location, op, parameters) {
  return ec.resource.script(location).execute(op, parameters || {}) || {}
}

/** The deployment's filter settings, the defaults filling what VoicePolicy leaves empty. */
function policy() {
  const row = one(POLICY, { voicePolicyId: 'DEFAULT' }) || {}
  const out = { smsSpamFilter: row.smsSpamFilter !== 'N', smsSpamLookup: row.smsSpamLookup === 'Y' }
  for (const [key, value] of Object.entries(DEFAULTS)) out[key] = blank(row[key]) || !(Number(row[key]) > 0) ? value : Number(row[key])
  return out
}

function rules() {
  return Array.from(find(RULE).list()).filter((rule) => rule.disabled !== 'Y' && TYPES.includes(rule.ruleTypeEnumId))
}

// ---------- what a text has ----------

/** The text's words in lower case, single spaces, padded with one at each end. */
function wordsOf(text) {
  return ' ' + String(text || '').toLowerCase().replace(/[^\p{L}\p{N}]+/gu, ' ').trim() + ' '
}

/** The hosts of the text's links: http(s) addresses and bare domains (example.com/x). */
function hostsOf(text) {
  const hosts = []
  for (const match of String(text || '').matchAll(/(?:https?:\/\/)?((?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,24})(?=[\/\s:?#.,;!)\]'"]|$)/gi)) {
    if (!/^https?:\/\//i.test(match[0]) && /^\d/.test(match[1])) continue
    hosts.push(match[1].toLowerCase())
  }
  return hosts
}

function hostMatches(host, pattern) {
  const want = String(pattern || '').toLowerCase().trim().replace(/^\*\./, '')
  return !!want && (host === want || host.endsWith('.' + want))
}

function entryHolds(entry, number) {
  const text = String(entry || '')
  return text.endsWith('*') ? number.startsWith(text.slice(0, -1)) : text === number
}

/** The votes of a number: {spam, notSpam}. */
function votesOf(number) {
  const out = { spam: 0, notSpam: 0 }
  for (const vote of find(VOTE).condition('phoneNumber', number).list()) {
    if (vote.voteEnumId === 'SsvSpam') out.spam++
    else if (vote.voteEnumId === 'SsvNotSpam') out.notSpam++
  }
  return out
}

/** Run fn as the user (username), back to whoever was signed in after. */
function asUser(username, fn) {
  const was = ec.user.username || null
  if (was === username) return fn()
  try {
    ec.user.internalLoginUser(username)
    return fn()
  } finally {
    ec.user.logoutUser()
    if (was) ec.user.internalLoginUser(was)
  }
}

/**
 * Whether an outside number is known to the users of our number: on one of their own caller lists (not the Blocked list),
 * a user's mobile, or (askApps, a new conversation) named by an app's call-context provider, asked as the first of them.
 * A provider that fails counts as knowing it, so a failure adds no points.
 */
function known(number, users, askApps) {
  if (!users.length) return false
  const lists = Array.from(find(LIST).condition('userId', 'in', users).list()).filter((list) => list.listTypeEnumId !== 'CltBlocked')
  if (lists.length && Array.from(find(ENTRY).condition('callerListId', 'in', lists.map((l) => l.callerListId)).list())
    .some((entry) => entryHolds(entry.phoneNumber, number))) return true
  if (find(SETTING).condition('mobileNumber', number).count() > 0) return true
  if (!askApps) return false
  const account = users.map((userId) => one('moqui.security.UserAccount', { userId })).find((row) => row && row.disabled !== 'Y')
  if (!account) return false
  ec.message.pushErrors()
  try {
    const providers = ec.service.sync().name('org.moqui.ui.ShellServices.get#Providers').parameter('hook', 'call-context').disableAuthz().call() || {}
    if (!Array.from(providers.providerList || []).length) return false
    const out = asUser(account.username, () => ec.service.sync().name('org.moqui.ui.ShellServices.call#Providers')
      .parameters({ hook: 'call-context', parameters: { number } }).call() || {})
    if (ec.message.hasError() || Array.from(out.failedParts || []).length) return true
    return Array.from(out.results || []).some((answer) => Array.from((answer && answer.contactList) || []).length > 0)
  } catch (err) {
    ec.logger.warn(`Spam filter: the call-context providers failed for ${number}, taken as known: ${message(err)}`)
    return true
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
}

// ---------- the provider's lookup ----------

function today() {
  return new Date(nowMs()).toISOString().slice(0, 10)
}

/** The day's lookups, counted (SmsSendCount lookup); false when the day's limit is reached. */
function countLookup(max) {
  const day = today()
  const row = find(COUNT).condition({ counterKey: 'lookup', dayDate: day }).forUpdate(true).one()
  if (row && Number(row.sentCount || 0) >= max) return false
  if (row) {
    row.setFields({ sentCount: Number(row.sentCount || 0) + 1, lastSentDate: ec.user.nowTimestamp }, true, null, null)
    row.update()
  } else {
    const made = ec.entity.makeValue(COUNT)
    made.setFields({ counterKey: 'lookup', dayDate: day, sentCount: 1, lastSentDate: ec.user.nowTimestamp }, true, null, null)
    made.create()
  }
  return true
}

/** The provider's answer for number: kept when younger than the policy's days, else asked (and kept); null for none. */
function lookupOf(number, chatBridgeId, pol) {
  const kept = one(KNOWN, { phoneNumber: number })
  if (kept && kept.lookupJson && ms(kept.lookupDate) > nowMs() - pol.smsSpamLookupDays * 86400000) {
    try { return JSON.parse(kept.lookupJson) } catch (err) { /* asked again */ }
  }
  if (!countLookup(pol.smsSpamLookupMaxPerDay)) {
    ec.logger.warn(`Spam filter: the day's ${pol.smsSpamLookupMaxPerDay} number lookups are used; ${number} not looked up`)
    return null
  }
  ec.message.pushErrors()
  let out = {}
  try {
    out = ec.service.sync().name('org.moqui.chat.ChatBridgeServices.lookup#BridgeNumber').parameters({ chatBridgeId, phoneNumber: number }).disableAuthz().call() || {}
    if (ec.message.hasError()) throw new Error(ec.message.errorsString)
  } catch (err) {
    ec.logger.warn(`Spam filter: the lookup of ${number} failed: ${message(err)}`)
    return null
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
  if (out.reasonCode) {
    if (out.reasonCode === 'failed') ec.logger.warn(`Spam filter: the lookup of ${number} failed: ${out.error || ''}`)
    return null
  }
  const found = { lineType: out.lineType || null, carrierName: out.carrierName || null, callerName: out.callerName || null }
  keepNumber(number, { lookupJson: JSON.stringify(found), lookupDate: ec.user.nowTimestamp, lookupBridgeId: chatBridgeId })
  return found
}

function keepNumber(number, fields) {
  const row = one(KNOWN, { phoneNumber: number })
  if (row) {
    row.setFields(fields, true, null, null)
    row.update()
  } else {
    const made = ec.entity.makeValue(KNOWN)
    made.setFields(Object.assign({ phoneNumber: number }, fields), true, null, null)
    made.create()
  }
}

// ---------- the score ----------

/**
 * The score of a text from number (the rules it matches): {score, reasons: [smsSpamRuleId]}. firstContact: a new
 * conversation from a number nobody knows; lookup: a function answering the provider's lookup, or null.
 */
function scoreOf(text, number, firstContact, threshold, lookup) {
  const read = String(text || '').slice(0, TEXT_READ)
  const words = wordsOf(read)
  const hosts = hostsOf(read)
  const all = rules()
  let votes = null
  let score = 0
  const reasons = []
  const hit = (rule) => { score += Number(rule.weight || 0); reasons.push(rule.smsSpamRuleId) }
  for (const rule of all) {
    const pattern = String(rule.pattern || '').trim()
    switch (rule.ruleTypeEnumId) {
      case 'SsrFirstContact': if (firstContact) hit(rule); break
      case 'SsrLink': if (hosts.length) hit(rule); break
      case 'SsrLinkHost': if (pattern && hosts.some((host) => hostMatches(host, pattern))) hit(rule); break
      case 'SsrKeyword': { const want = wordsOf(pattern); if (want.trim() && words.includes(want)) hit(rule); break }
      case 'SsrNumber': if (pattern && entryHolds(pattern, number)) hit(rule); break
      case 'SsrReported': votes = votes || votesOf(number); if (votes.spam > votes.notSpam) hit(rule); break
      default: break
    }
  }
  // the line type last: only when its answer can change the verdict
  const lineRules = all.filter((rule) => rule.ruleTypeEnumId === 'SsrLineType' && !blank(rule.pattern))
  const most = lineRules.reduce((sum, rule) => sum + Math.max(0, Number(rule.weight || 0)), 0)
  if (lookup && lineRules.length && score < threshold && score + most >= threshold) {
    const found = lookup()
    const lineType = String((found && found.lineType) || '').toLowerCase()
    if (lineType) for (const rule of lineRules) if (String(rule.pattern).trim().toLowerCase() === lineType) hit(rule)
  }
  return { score, reasons }
}

/**
 * Internal (Sms.js receive, in a transaction of its own, before ensure#PairRoom): the filters a text from theirNumber to
 * ourNumber on chatBridgeId puts on the users our number reaches. groupNumbers: a group text's numbers (no filter by a
 * block then). Out: memberFilterMap (userId to CmfBlocked or CmfSpam), spam, score, reasons, judged.
 */
function judge(p) {
  const ours = p.ourNumber
  const theirs = p.theirNumber
  const out = { memberFilterMap: {}, spam: false, score: 0, reasons: [], judged: false }
  if (blank(ours) || blank(theirs)) return out
  const users = Array.from(script(SMS, 'usersOf', { phoneNumber: ours }).userIdList || [])
  if (!users.length) return out
  const group = Array.from(p.groupNumbers || []).length > 1
  if (!group) {
    for (const userId of script(CALL_HANDLING, 'blockedUsers', { phoneNumber: theirs, userIdList: users }).userIdList || []) out.memberFilterMap[userId] = 'CmfBlocked'
  }
  const pol = policy()
  if (!pol.smsSpamFilter) return out
  // the room: none yet is a new conversation; one with a message of ours is a conversation, never judged
  const key = ours + '/' + (group ? Array.from(p.groupNumbers).join(',') : p.pairNumber || theirs)
  const mapping = Array.from(find(MAPPING).condition('chatBridgeId', p.chatBridgeId).condition('externalRoomId', key).condition('thruDate', null).list())[0]
  const room = mapping ? one(ROOM, { chatRoomId: mapping.chatRoomId }) : null
  if (room && find(EVENT).condition('chatRoomId', room.chatRoomId).condition('eventTypeEnumId', 'CetMessage')
    .condition('senderUserId', 'is-not-null', null).count() > 0) return out
  const votes = votesOf(theirs)
  if (votes.notSpam > votes.spam) return out
  const firstContact = !room
  if (known(theirs, users, firstContact && isE164(theirs))) return out
  const lookup = pol.smsSpamLookup && firstContact && isE164(theirs) ? () => lookupOf(theirs, p.chatBridgeId, pol) : null
  const scored = scoreOf(p.text, theirs, firstContact, pol.smsSpamThreshold, lookup)
  out.judged = true
  out.score = scored.score
  out.reasons = scored.reasons
  out.spam = scored.score >= pol.smsSpamThreshold
  if (out.spam) {
    const off = new Set(Array.from(find(SETTING).condition('userId', 'in', users).condition('spamFilter', 'N').selectField('userId').list()).map((row) => row.userId))
    for (const userId of users) if (!out.memberFilterMap[userId] && !off.has(userId)) out.memberFilterMap[userId] = 'CmfSpam'
  }
  return out
}

/**
 * Internal (Sms.js receive, after the text was taken and not a duplicate, in a transaction of its own): the rules a judged
 * text matched count it, and its number keeps the verdict (the banner's reasons).
 */
function noteJudged(p) {
  const reasons = Array.from(p.reasons || []).map(String)
  if (!blank(p.theirNumber) && p.judged !== false) {
    keepNumber(p.theirNumber, { lastScore: Number(p.score || 0), lastReasonsJson: JSON.stringify(reasons), lastJudgedDate: ec.user.nowTimestamp })
  }
  countRules(reasons, 'hitCount', true)
  return {}
}

function countRules(ruleIds, field, hitDate) {
  for (const smsSpamRuleId of Array.from(new Set(ruleIds))) {
    const rule = find(RULE).condition('smsSpamRuleId', smsSpamRuleId).forUpdate(true).one()
    if (!rule) continue
    rule.set(field, Number(rule[field] || 0) + 1)
    if (hitDate) rule.set('lastHitDate', ec.user.nowTimestamp)
    rule.update()
  }
}

// ---------- Not spam and Report spam ----------

/** The current user's phone room: {me, room, numbers}, or a refusal. */
function myPhoneRoom(chatRoomId) {
  const me = ec.user.userId
  if (!me) return { refusal: refuse('notSignedIn', 'Sign in first') }
  const member = chatRoomId ? one(MEMBER, { chatRoomId, userId: me }) : null
  const room = member && !member.leftDate ? one(ROOM, { chatRoomId }) : null
  if (!room || room.roomTypeEnumId !== 'CrtExternal') return { refusal: refuse('notFound', 'Chat room not found') }
  const numbers = Array.from(script(CHAT_BLOCKS, 'phoneNumbers', { chatRoomId }).numberList || [])
  if (!numbers.length) return { refusal: refuse('notFound', 'Chat room not found') }
  return { me, room, numbers, member }
}

function vote(number, userId, voteEnumId) {
  const row = one(VOTE, { phoneNumber: number, userId })
  if (row) {
    if (row.voteEnumId === voteEnumId) return false
    row.setFields({ voteEnumId, voteDate: ec.user.nowTimestamp }, true, null, null)
    row.update()
  } else {
    const made = ec.entity.makeValue(VOTE)
    made.setFields({ phoneNumber: number, userId, voteEnumId, voteDate: ec.user.nowTimestamp }, true, null, null)
    made.create()
  }
  return true
}

function lastReasons(number) {
  const kept = one(KNOWN, { phoneNumber: number })
  try { return kept && kept.lastReasonsJson ? Array.from(JSON.parse(kept.lastReasonsJson)) : [] } catch (err) { return [] }
}

/** The room's current local members: [{userId, filterEnumId}]. */
function membersOf(chatRoomId) {
  return Array.from(find(MEMBER).condition('chatRoomId', chatRoomId).condition('leftDate', null).condition('userId', 'is-not-null', null).list())
}

/** Not spam: the room in sight again for every member it was Spam for, the numbers voted not spam, the rules counted. */
function markNotSpam(p) {
  const found = myPhoneRoom(p.chatRoomId)
  if (found.refusal) return found.refusal
  for (const member of membersOf(found.room.chatRoomId)) {
    if (member.filterEnumId !== 'CmfSpam') continue
    try {
      script(CHAT_BLOCKS, 'setFilters', { userId: member.userId, chatRoomIdList: [found.room.chatRoomId], onlyFrom: 'CmfSpam' })
    } catch (err) {
      ec.logger.error(`Chat room ${found.room.chatRoomId}: Not spam did not reach ${member.userId}: ${message(err)}`)
    }
  }
  for (const number of found.numbers) if (vote(number, found.me, 'SsvNotSpam')) countRules(lastReasons(number), 'notSpamCount', false)
  return {}
}

/**
 * Report spam: a pair room's number on the user's Blocked list (its calls too), a group text's room blocked for the user;
 * the numbers voted spam, the rules counted, and the room in Spam for the other members whose filter is on.
 */
function reportSpam(p) {
  const found = myPhoneRoom(p.chatRoomId)
  if (found.refusal) return found.refusal
  const chatRoomId = found.room.chatRoomId
  if (found.numbers.length === 1) {
    const out = script(CALL_HANDLING, 'blockNumber', { phoneNumber: found.numbers[0] })
    if (ec.message.hasError()) return out
  } else {
    script(CHAT_BLOCKS, 'setFilters', { userId: found.me, chatRoomIdList: [chatRoomId], filterEnumId: 'CmfBlocked', replace: true })
  }
  for (const number of found.numbers) if (vote(number, found.me, 'SsvSpam')) countRules(lastReasons(number), 'spamCount', false)
  if (policy().smsSpamFilter) {
    const others = membersOf(chatRoomId).filter((member) => member.userId !== found.me && !member.filterEnumId)
    const off = new Set(others.length ? Array.from(find(SETTING).condition('userId', 'in', others.map((m) => m.userId)).condition('spamFilter', 'N')
      .selectField('userId').list()).map((row) => row.userId) : [])
    for (const member of others) {
      if (off.has(member.userId)) continue
      try {
        script(CHAT_BLOCKS, 'setFilters', { userId: member.userId, chatRoomIdList: [chatRoomId], filterEnumId: 'CmfSpam' })
      } catch (err) {
        ec.logger.error(`Chat room ${chatRoomId}: the report did not reach ${member.userId}: ${message(err)}`)
      }
    }
  }
  return {}
}

/** The banner of a phone room: filtered (spam, blocked or none) for the current user, and why it is spam (reasons). */
function spamInfo(p) {
  const found = myPhoneRoom(p.chatRoomId)
  if (found.refusal) return found.refusal
  const ids = Array.from(new Set(found.numbers.flatMap((number) => lastReasons(number))))
  const reasons = ids.length ? Array.from(find(RULE).condition('smsSpamRuleId', 'in', ids).list())
    .map((rule) => ec.l10n.localize(rule.description || rule.smsSpamRuleId)) : []
  return { filtered: { CmfSpam: 'spam', CmfBlocked: 'blocked' }[found.member.filterEnumId] || null, reasons: Array.from(new Set(reasons)) }
}

// ---------- the purge (chat_spam_purge_days) ----------

/**
 * Internal (ChatServices clean#ChatRetention, daily): with the property chat_spam_purge_days above 0, the texts older than
 * that in each phone room that every current member has in Spam are deleted (clean#RoomRetention). Each room on its own,
 * in its own transaction: one that fails is logged and the rest go on. Out: roomCount, deletedCount, failedCount.
 */
function purgeSpam(p) {
  const days = Math.floor(Number(blank(p.purgeDays) ? System.getProperty('chat_spam_purge_days') || 0 : p.purgeDays))
  const out = { roomCount: 0, deletedCount: 0, failedCount: 0 }
  if (!(days > 0)) return out
  const spamRooms = Array.from(new Set(Array.from(find(MEMBER).condition('filterEnumId', 'CmfSpam').condition('leftDate', null)
    .selectField('chatRoomId').list()).map((row) => row.chatRoomId)))
  for (const chatRoomId of spamRooms) {
    ec.message.pushErrors()
    try {
      const members = membersOf(chatRoomId)
      if (!members.length || members.some((member) => member.filterEnumId !== 'CmfSpam')) continue
      const room = one(ROOM, { chatRoomId })
      if (!room || room.roomTypeEnumId !== 'CrtExternal') continue
      const kept = Number(room.retentionDays) > 0 ? Math.min(Number(room.retentionDays), days) : days
      const done = ec.service.sync().name('org.moqui.chat.ChatServices.clean#RoomRetention').parameters({ chatRoomId, retentionDays: kept })
        .requireNewTransaction(true).disableAuthz().call() || {}
      if (ec.message.hasError()) throw new Error(ec.message.errorsString)
      out.roomCount++
      out.deletedCount += Number(done.deletedCount || 0)
    } catch (err) {
      out.failedCount++
      ec.logger.error(`Could not purge the spam of chat room ${chatRoomId}: ${message(err)}`)
    } finally {
      ec.message.clearErrors()
      ec.message.popErrors()
    }
  }
  if (out.deletedCount || out.failedCount) ec.logger.info(`Spam purge: ${out.deletedCount} events deleted in ${out.roomCount} rooms, ${out.failedCount} rooms failed`)
  return out
}

// ---------- the admin's settings (VoiceAdmin) ----------

function notAdmin() {
  return ec.user.userId && ec.user.hasPermission('VoiceAdmin') ? null : refuse('notAllowed', 'Only a voice administrator may do this')
}

function getFilter() {
  const refusal = notAdmin()
  if (refusal) return refusal
  return { settings: policy(), ruleList: Array.from(find(RULE).orderBy('ruleTypeEnumId').orderBy('pattern').list()).map((rule) => ({
    smsSpamRuleId: rule.smsSpamRuleId, ruleTypeEnumId: rule.ruleTypeEnumId, pattern: rule.pattern || null, weight: Number(rule.weight || 0),
    description: rule.description || null, disabled: rule.disabled === 'Y', hitCount: Number(rule.hitCount || 0),
    notSpamCount: Number(rule.notSpamCount || 0), spamCount: Number(rule.spamCount || 0), lastHitDate: rule.lastHitDate || null })) }
}

function updateFilter(p) {
  const refusal = notAdmin()
  if (refusal) return refusal
  let row = one(POLICY, { voicePolicyId: 'DEFAULT' })
  if (!row) {
    row = ec.entity.makeValue(POLICY)
    row.set('voicePolicyId', 'DEFAULT')
    row.create()
  }
  for (const key of ['smsSpamFilter', 'smsSpamLookup']) if (p[key] != null && p[key] !== '') row.set(key, yes(p[key]) ? 'Y' : 'N')
  for (const key of Object.keys(DEFAULTS)) {
    if (p[key] == null) continue
    if (p[key] === '') { row.set(key, null); continue }
    const value = Math.floor(Number(p[key]))
    if (!(value > 0) || value > 100000) return refuse('badNumber', 'A number above 0')
    row.set(key, value)
  }
  row.update()
  return { settings: policy() }
}

function storeRule(p) {
  const refusal = notAdmin()
  if (refusal) return refusal
  let rule = blank(p.smsSpamRuleId) ? null : one(RULE, { smsSpamRuleId: p.smsSpamRuleId })
  if (!blank(p.smsSpamRuleId) && !rule) return refuse('notFound', 'Rule not found')
  const type = p.ruleTypeEnumId || (rule && rule.ruleTypeEnumId)
  if (!TYPES.includes(type)) return refuse('badRule', 'Choose what the rule looks for')
  const pattern = p.pattern == null ? (rule && rule.pattern) || '' : String(p.pattern).trim().slice(0, 200)
  if (['SsrLinkHost', 'SsrKeyword', 'SsrNumber', 'SsrLineType'].includes(type) && !pattern) return refuse('badRule', 'This rule needs a pattern')
  const weight = p.weight == null || p.weight === '' ? Number((rule && rule.weight) || 0) : Math.round(Number(p.weight))
  if (!Number.isFinite(weight) || weight < -1000 || weight > 1000) return refuse('badRule', 'A weight from -1000 to 1000')
  const fields = { ruleTypeEnumId: type, pattern: pattern || null, weight,
    description: p.description == null ? (rule && rule.description) || null : String(p.description).trim().slice(0, 200) || null,
    disabled: p.disabled == null || p.disabled === '' ? (rule && rule.disabled) || 'N' : yes(p.disabled) ? 'Y' : 'N' }
  if (!rule) {
    rule = ec.entity.makeValue(RULE)
    rule.setSequencedIdPrimary()
    rule.setFields(fields, true, null, null)
    rule.create()
  } else {
    rule.setFields(fields, true, null, null)
    rule.update()
  }
  return { smsSpamRuleId: rule.smsSpamRuleId }
}

function deleteRule(p) {
  const refusal = notAdmin()
  if (refusal) return refusal
  const rule = blank(p.smsSpamRuleId) ? null : one(RULE, { smsSpamRuleId: p.smsSpamRuleId })
  if (rule) rule.delete()
  return {}
}

const operations = {
  judge, noteJudged, markNotSpam, reportSpam, spamInfo, purgeSpam, getFilter, updateFilter, storeRule, deleteRule,
  score: (p) => scoreOf(p.text, p.theirNumber, yes(p.firstContact), policy().smsSpamThreshold, null),
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown SMS spam operation ' + op)
  return operations[op](parameters || {}) || {}
}

;({ execute })
