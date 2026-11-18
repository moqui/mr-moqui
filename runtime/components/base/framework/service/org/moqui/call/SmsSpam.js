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
// (100). A text is judged only while the filter is on, the room was never marked Not spam (ChatRoomBridge.notSpamDate),
// the room has no message from a local sender (once we replied it is a conversation), and the number is neither trusted
// for everyone (an admin's not-spam vote, or TRUST_VOTES distinct users' that outnumber the spam votes) nor known (on a
// member's own caller list that no reject rule uses, a user's mobile, or named by an app's call-context provider: for a
// new conversation only, as the room's first member). A trusted or known number is still judged by the admin's number
// rules (SsrNumber) alone, and a user who voted it not spam never gets it in Spam. Each distinct reporter adds the
// reported rule's weight (50: two reach the threshold), less one for each not-spam vote; an admin's report reaches the
// threshold alone. The provider's lookup (lookup#BridgeNumber: the line type) is asked only with VoicePolicy.smsSpamLookup
// on, for a first contact in E.164, when its answer can change the verdict, with no answer kept younger than
// smsSpamLookupDays, under smsSpamLookupMaxPerDay (the day's count reserved in a transaction of its own, so no lock is held
// through the provider's answer, and given back when the lookup fails).
//
// Not spam (markNotSpam) clears the room's Spam for every member, marks the room never to be judged again and votes not
// spam (the number trusted for the voter's own rooms). Report spam (reportSpam) acts on the reporter: a pair room's number
// on their Blocked list (CallHandling.js blockNumber: its calls too), a group text's room blocked for them alone with votes
// only against the numbers they pick; and it puts the room in Spam for the teammates whose filter is on only when nobody
// here ever replied in it, keeping those rooms on the vote (filteredJson), so the reporter's Unblock withdraws the vote
// and brings them back (withdrawReport). Failure tolerant: a judge that fails lets the text in unfiltered, a provider or a
// lookup that fails adds nothing.

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
/** The distinct users whose Not spam trusts a number for everyone (an admin's alone does). */
const TRUST_VOTES = 3
/** The rule kinds that make a caller list a reject list, never a list of known numbers. */
const REJECT_KINDS = ['ChrSelectiveReject', 'ChrAnonymousReject']

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

/** The votes of a number: {spam, notSpam, spamAdmin, notSpamAdmin, notSpamUsers}. */
function votesOf(number) {
  const out = { spam: 0, notSpam: 0, spamAdmin: false, notSpamAdmin: false, notSpamUsers: new Set() }
  for (const vote of find(VOTE).condition('phoneNumber', number).list()) {
    if (vote.voteEnumId === 'SsvSpam') {
      out.spam++
      if (vote.byAdmin === 'Y') out.spamAdmin = true
    } else if (vote.voteEnumId === 'SsvNotSpam') {
      out.notSpam++
      out.notSpamUsers.add(vote.userId)
      if (vote.byAdmin === 'Y') out.notSpamAdmin = true
    }
  }
  return out
}

/** Whether the votes trust a number for everyone: an admin's Not spam, or TRUST_VOTES users' that outnumber the reports. */
function trustedForAll(votes) {
  return votes.notSpamAdmin || (votes.notSpam >= TRUST_VOTES && votes.notSpam > votes.spam)
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
  // a list a reject rule uses is a list of callers kept out, never of numbers known
  const rejecting = new Set(Array.from(find('moqui.call.CallHandlingRule').condition('userId', 'in', users).list())
    .filter((rule) => REJECT_KINDS.includes(rule.kindEnumId) || rule.targetTypeEnumId === 'ChtReject').map((rule) => rule.callerListId).filter((id) => id))
  const lists = Array.from(find(LIST).condition('userId', 'in', users).list())
    .filter((list) => list.listTypeEnumId !== 'CltBlocked' && !rejecting.has(list.callerListId))
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

/**
 * A lookup of the day reserved (SmsSendCount lookup, by delta 1) or given back (delta -1), in a transaction of its own so
 * its row lock is not held through the provider's answer. A reserve answers false when the day's limit is reached.
 */
function countLookup(max, delta) {
  let reserved = false
  ec.transaction.runRequireNew(null, 'The day\'s number lookups were not counted', () => {
    const day = today()
    const row = find(COUNT).condition({ counterKey: 'lookup', dayDate: day }).forUpdate(true).one()
    if (delta > 0 && row && Number(row.sentCount || 0) >= max) return
    if (row) {
      row.setFields({ sentCount: Math.max(0, Number(row.sentCount || 0) + delta), lastSentDate: ec.user.nowTimestamp }, true, null, null)
      row.update()
    } else if (delta > 0) {
      const made = ec.entity.makeValue(COUNT)
      made.setFields({ counterKey: 'lookup', dayDate: day, sentCount: 1, lastSentDate: ec.user.nowTimestamp }, true, null, null)
      made.create()
    }
    reserved = true
  })
  return reserved
}

/** The provider's answer for number: kept when younger than the policy's days, else asked (and kept); null for none. */
function lookupOf(number, chatBridgeId, pol) {
  const kept = one(KNOWN, { phoneNumber: number })
  if (kept && kept.lookupJson && ms(kept.lookupDate) > nowMs() - pol.smsSpamLookupDays * 86400000) {
    try { return JSON.parse(kept.lookupJson) } catch (err) { /* asked again */ }
  }
  if (!countLookup(pol.smsSpamLookupMaxPerDay, 1)) {
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
    out = { reasonCode: 'failed' }
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
  if (out.reasonCode) {
    if (out.reasonCode === 'failed' && out.error) ec.logger.warn(`Spam filter: the lookup of ${number} failed: ${out.error}`)
    // a lookup that failed is not counted against the day
    if (out.reasonCode === 'failed') countLookup(pol.smsSpamLookupMaxPerDay, -1)
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
function scoreOf(text, number, firstContact, threshold, lookup, numbersOnly) {
  const read = String(text || '').slice(0, TEXT_READ)
  const words = wordsOf(read)
  const hosts = hostsOf(read)
  // a trusted or known number: the admin's number rules alone
  const all = rules().filter((rule) => !numbersOnly || rule.ruleTypeEnumId === 'SsrNumber')
  let votes = null
  let score = 0
  const reasons = []
  const hit = (rule, points) => { score += points != null ? points : Number(rule.weight || 0); reasons.push(rule.smsSpamRuleId) }
  for (const rule of all) {
    const pattern = String(rule.pattern || '').trim()
    switch (rule.ruleTypeEnumId) {
      case 'SsrFirstContact': if (firstContact) hit(rule); break
      case 'SsrLink': if (hosts.length) hit(rule); break
      case 'SsrLinkHost': if (pattern && hosts.some((host) => hostMatches(host, pattern))) hit(rule); break
      case 'SsrKeyword': { const want = wordsOf(pattern); if (want.trim() && words.includes(want)) hit(rule); break }
      case 'SsrNumber': if (pattern && entryHolds(pattern, number)) hit(rule); break
      case 'SsrReported': {
        // its weight for each distinct reporter, less the not-spam votes; an admin's report reaches the threshold alone
        votes = votes || votesOf(number)
        const reporters = Math.max(0, votes.spam - votes.notSpam)
        const weight = Number(rule.weight || 0)
        if (votes.spamAdmin) hit(rule, Math.max(weight * reporters, threshold))
        else if (reporters > 0) hit(rule, weight * reporters)
        break
      }
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
  // the room: none yet is a new conversation; one a member marked Not spam, or with a message of ours, is never judged
  const key = ours + '/' + (group ? Array.from(p.groupNumbers).join(',') : p.pairNumber || theirs)
  const mapping = Array.from(find(MAPPING).condition('chatBridgeId', p.chatBridgeId).condition('externalRoomId', key).condition('thruDate', null).list())[0]
  if (mapping && mapping.notSpamDate) return out
  const room = mapping ? one(ROOM, { chatRoomId: mapping.chatRoomId }) : null
  if (room && find(EVENT).condition('chatRoomId', room.chatRoomId).condition('eventTypeEnumId', 'CetMessage')
    .condition('senderUserId', 'is-not-null', null).count() > 0) return out
  const votes = votesOf(theirs)
  const firstContact = !room
  // trusted for everyone, or known: only the admin's number rules apply
  const numbersOnly = trustedForAll(votes) || known(theirs, users, firstContact && isE164(theirs))
  const lookup = !numbersOnly && pol.smsSpamLookup && firstContact && isE164(theirs) ? () => lookupOf(theirs, p.chatBridgeId, pol) : null
  const scored = scoreOf(p.text, theirs, firstContact && !numbersOnly, pol.smsSpamThreshold, lookup, numbersOnly)
  out.judged = true
  out.score = scored.score
  out.reasons = scored.reasons
  out.spam = scored.score >= pol.smsSpamThreshold
  if (out.spam) {
    const off = new Set(Array.from(find(SETTING).condition('userId', 'in', users).condition('spamFilter', 'N').selectField('userId').list()).map((row) => row.userId))
    // a user who said Not spam to the number keeps it in their sight (but not against the admin's number rules)
    const voters = numbersOnly ? new Set() : votes.notSpamUsers
    for (const userId of users) if (!out.memberFilterMap[userId] && !off.has(userId) && !voters.has(userId)) out.memberFilterMap[userId] = 'CmfSpam'
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

/** The user's vote of a number (one a user; an admin's is marked), with what a report keeps; true when it changed. */
function vote(number, userId, voteEnumId, extra) {
  const fields = Object.assign({ voteEnumId, voteDate: ec.user.nowTimestamp,
    byAdmin: ec.user.userId === userId && ec.user.hasPermission('VoiceAdmin') ? 'Y' : 'N', chatRoomId: null, filteredJson: null }, extra || {})
  const row = one(VOTE, { phoneNumber: number, userId })
  const changed = !row || row.voteEnumId !== voteEnumId
  if (row) {
    row.setFields(fields, true, null, null)
    row.update()
  } else {
    const made = ec.entity.makeValue(VOTE)
    made.setFields(Object.assign({ phoneNumber: number, userId }, fields), true, null, null)
    made.create()
  }
  return changed
}

/** Whether anyone here ever wrote in the room (a message of a local sender). */
function repliedIn(chatRoomId) {
  return find(EVENT).condition('chatRoomId', chatRoomId).condition('eventTypeEnumId', 'CetMessage').condition('senderUserId', 'is-not-null', null).count() > 0
}

function lastReasons(number) {
  const kept = one(KNOWN, { phoneNumber: number })
  try { return kept && kept.lastReasonsJson ? Array.from(JSON.parse(kept.lastReasonsJson)) : [] } catch (err) { return [] }
}

/** The room's current local members: [{userId, filterEnumId}]. */
function membersOf(chatRoomId) {
  return Array.from(find(MEMBER).condition('chatRoomId', chatRoomId).condition('leftDate', null).condition('userId', 'is-not-null', null).list())
}

/**
 * Not spam: the room in sight again for every member it was Spam for and never judged again (its mapping's notSpamDate),
 * the numbers voted not spam by the user (trusted for the user's own rooms), the rules counted.
 */
function markNotSpam(p) {
  const found = myPhoneRoom(p.chatRoomId)
  if (found.refusal) return found.refusal
  for (const mapping of find(MAPPING).condition('chatRoomId', found.room.chatRoomId).condition('thruDate', null).list()) {
    if (mapping.notSpamDate) continue
    mapping.set('notSpamDate', ec.user.nowTimestamp)
    mapping.update()
  }
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
 * Report spam, for the reporter: a pair room's number on their Blocked list (its calls refused too) and a spam vote of it;
 * a group text's room blocked for them alone, with spam votes only of the numbers of phoneNumberList they picked (none by
 * default). In a pair room the teammates whose filter is on get the room in Spam only when nobody here ever replied in it; those rooms
 * are kept on the vote, so the reporter's Unblock brings them back (withdrawReport). Out: blocked, votedNumberList,
 * filteredUserIdList.
 */
function reportSpam(p) {
  const found = myPhoneRoom(p.chatRoomId)
  if (found.refusal) return found.refusal
  const chatRoomId = found.room.chatRoomId
  const pair = found.numbers.length === 1
  if (pair) {
    const out = script(CALL_HANDLING, 'blockNumber', { phoneNumber: found.numbers[0] })
    if (ec.message.hasError()) return out
  } else {
    script(CHAT_BLOCKS, 'setFilters', { userId: found.me, chatRoomIdList: [chatRoomId], filterEnumId: 'CmfBlocked', replace: true })
  }
  const picked = new Set(Array.from(p.phoneNumberList || []).map(String))
  const voted = pair ? found.numbers : found.numbers.filter((number) => picked.has(number))
  // the teammates of a pair room, only when nobody here ever replied in it (a group text is hidden for the reporter alone)
  const filtered = []
  if (pair && policy().smsSpamFilter && !repliedIn(chatRoomId)) {
    const others = membersOf(chatRoomId).filter((member) => member.userId !== found.me && !member.filterEnumId)
    const off = new Set(others.length ? Array.from(find(SETTING).condition('userId', 'in', others.map((m) => m.userId)).condition('spamFilter', 'N')
      .selectField('userId').list()).map((row) => row.userId) : [])
    for (const member of others) {
      if (off.has(member.userId)) continue
      const changed = script(CHAT_BLOCKS, 'setFilters', { userId: member.userId, chatRoomIdList: [chatRoomId], filterEnumId: 'CmfSpam' }).changedRoomIdList || []
      if (changed.length) filtered.push({ userId: member.userId, chatRoomId })
    }
  }
  for (const number of voted) {
    if (vote(number, found.me, 'SsvSpam', { chatRoomId, filteredJson: filtered.length ? JSON.stringify(filtered) : null })) {
      countRules(lastReasons(number), 'spamCount', false)
    }
  }
  return { blocked: pair ? 'number' : 'room', votedNumberList: voted, filteredUserIdList: filtered.map((item) => item.userId) }
}

/**
 * Internal (the reporter's Unblock: CallHandling.js unblockNumber, ChatBlocks.js unblockRoom): the user's spam votes of
 * phoneNumber, or those made from chatRoomId, are withdrawn, and the teammates' rooms those reports put in Spam come back
 * (when they are still in Spam). Each on its own: one that fails is logged.
 */
function withdrawReport(p) {
  if (blank(p.userId) || (blank(p.phoneNumber) && blank(p.chatRoomId))) return {}
  let find0 = find(VOTE).condition('userId', p.userId).condition('voteEnumId', 'SsvSpam')
  find0 = blank(p.phoneNumber) ? find0.condition('chatRoomId', p.chatRoomId) : find0.condition('phoneNumber', p.phoneNumber)
  let withdrawn = 0
  for (const row of Array.from(find0.list())) {
    try {
      let filtered = []
      try { filtered = row.filteredJson ? Array.from(JSON.parse(row.filteredJson)) : [] } catch (err) { filtered = [] }
      for (const item of filtered) {
        if (item && item.userId && item.chatRoomId) script(CHAT_BLOCKS, 'setFilters', { userId: item.userId, chatRoomIdList: [item.chatRoomId], onlyFrom: 'CmfSpam' })
      }
      row.delete()
      withdrawn++
    } catch (err) {
      ec.logger.error(`The report of ${row.phoneNumber} by ${p.userId} was not withdrawn: ${message(err)}`)
    }
  }
  return { withdrawnCount: withdrawn }
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
    notSpamCount: Number(rule.notSpamCount || 0), spamCount: Number(rule.spamCount || 0), lastHitDate: rule.lastHitDate || null })),
    voteList: votesList(), trustVotes: TRUST_VOTES }
}

/** The numbers with votes, the latest first (at most 200): spam and not-spam counts, an admin's, and whether trusted for everyone. */
function votesList() {
  const byNumber = new Map()
  for (const row of find(VOTE).orderBy('-voteDate').limit(1000).list()) {
    if (!byNumber.has(row.phoneNumber)) byNumber.set(row.phoneNumber, { phoneNumber: row.phoneNumber, lastVoteDate: row.voteDate || null })
  }
  return Array.from(byNumber.values()).slice(0, 200).map((item) => {
    const votes = votesOf(item.phoneNumber)
    return Object.assign(item, { spamCount: votes.spam, notSpamCount: votes.notSpam, spamAdmin: votes.spamAdmin, notSpamAdmin: votes.notSpamAdmin,
      trusted: trustedForAll(votes) })
  })
}

/** An admin's vote of a number (VoiceAdmin): notSpam trusts it for everyone, spam reports it for everyone, clear removes every vote of it. */
function storeVote(p) {
  const refusal = notAdmin()
  if (refusal) return refusal
  const number = String(p.phoneNumber || '').trim()
  if (!isE164(number)) return refuse('badNumber', 'A number with its country code, in E.164')
  if (p.vote === 'clear') {
    find(VOTE).condition('phoneNumber', number).deleteAll()
    return {}
  }
  if (p.vote !== 'spam' && p.vote !== 'notSpam') return refuse('badVote', 'Choose spam, not spam or clear')
  vote(number, ec.user.userId, p.vote === 'spam' ? 'SsvSpam' : 'SsvNotSpam')
  return {}
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
  judge, noteJudged, markNotSpam, reportSpam, withdrawReport, spamInfo, purgeSpam, getFilter, updateFilter, storeRule, deleteRule, storeVote,
  score: (p) => scoreOf(p.text, p.theirNumber, yes(p.firstContact), policy().smsSpamThreshold, null),
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown SMS spam operation ' + op)
  return operations[op](parameters || {}) || {}
}

;({ execute })
