// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Call handling (plans/Comms-Framework.md §5.3, §5.4, V3): the services of org.moqui.impl.CallHandlingServices. An inbound
// phone call to a user or a ring group gets a plan when it arrives (inbound, from VoiceServices receive#PhoneCall): a list
// of steps, each "ring these users and numbers for so long", a voicemail box, a refusal, or the end. The plan is the
// user's rules read in order (anonymous and selective rejection, selective acceptance and forwarding, do not disturb,
// always-forward, busy, then their ring with a simultaneous ring to their mobile, then no answer), or the ring group's
// strategy (all at once, in order, round robin) and its fallback, each under its schedule's hours and holidays in the
// schedule's time zone. A forward to another user or group splices their plan in, bounded by call_forward_max_hops; a
// forward to one of this deployment's own numbers routes inside, never through the phone network; a forward back to the
// caller is skipped. A forward to an outside number is a phone leg of the user's (VoiceServices forwardCheck and
// dialForward: the full fraud policy and the user's permission to forward to that class of destination). The SIP
// engine's ring timer, a decline, or a forward that fails moves the call on (advance, through
// VoicemailServices.begin#Voicemail). The plan is kept in CallRoute; the steps run through Calls.js (ringStep,
// closeOffers), whose first answer wins across every tab, phone and forwarded leg.
//
// The user's own settings (their rules, caller lists, call waiting, mobile, caller-ID blocking, do not disturb) and the
// admin's ring groups and schedules are here too; admin services need VoiceAdmin.

const CALLS = 'component://framework/service/org/moqui/impl/Calls.js'
const VOICE = 'component://framework/service/org/moqui/impl/Voice.js'
const SESSION = 'moqui.call.CallSession'
const NUMBER = 'moqui.call.PhoneNumber'
const SCHEDULE = 'moqui.call.CommsSchedule'
const SCHEDULE_RULE = 'moqui.call.CommsScheduleRule'
const GROUP = 'moqui.call.RingGroup'
const MEMBER = 'moqui.call.RingGroupMember'
const LIST = 'moqui.call.CallerList'
const ENTRY = 'moqui.call.CallerListEntry'
const RULE = 'moqui.call.CallHandlingRule'
const SETTING = 'moqui.call.CallHandlingSetting'
const ROUTE = 'moqui.call.CallRoute'
const BOX = 'moqui.call.VoicemailBox'
const STATUS = 'moqui.security.UserStatus'
const ACCOUNT = 'moqui.security.UserAccount'

const KINDS = ['ChrAnonymousReject', 'ChrSelectiveReject', 'ChrSelectiveAccept', 'ChrSelectiveForward', 'ChrAlways', 'ChrBusy', 'ChrNoAnswer']
const TARGETS = ['ChtNumber', 'ChtUser', 'ChtGroup', 'ChtVoicemail', 'ChtReject']
/** The kinds whose target is where a call goes, and the kinds that refuse or accept. */
const SELECTIVE = ['ChrSelectiveReject', 'ChrSelectiveAccept', 'ChrSelectiveForward']
const STRATEGIES = ['RgsSimultaneous', 'RgsInOrder', 'RgsRoundRobin']
const FALLBACKS = ['RgfVoicemail', 'RgfGroup', 'RgfNumber', 'RgfNone']
const SCHEDULE_TYPES = ['CsrWeekly', 'CsrDate', 'CsrHoliday']
const DAY_NAMES = { Mon: 1, Tue: 2, Wed: 3, Thu: 4, Fri: 5, Sat: 6, Sun: 7 }
/** Voicemail's rings (VoicemailServices): seconds each, and the most. */
const RING_SECONDS_EACH = 6
const MAX_RINGS = 10
/** How long past its last step's ring a ringing call is kept (the SIP engine moves it on before that). */
const RING_GRACE_SECONDS = 30
/** The most rules, caller lists and entries a user keeps. */
const MAX_RULES = 50
const MAX_LISTS = 50
const MAX_ENTRIES = 500

const ms = (value) => {
  if (value == null || value === '') return null
  const number = Number(value)
  return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}
const nowMs = () => ms(ec.user.nowTimestamp)
const find = (entity) => ec.entity.find(entity).disableAuthz()
const one = (entity, fields) => find(entity).condition(fields).one()
const rows = (entity, fields) => Array.from(find(entity).condition(fields || {}).list())
const yes = (value) => value === true || value === 'Y' || value === 'true'
const blank = (value) => value == null || String(value).trim() === ''
const clamp = (value, low, high) => Math.min(Math.max(value, low), high)
const property = (name, fallback) => {
  const value = System.getProperty(name)
  return value == null || String(value).trim() === '' ? fallback : String(value).trim()
}
const maxHops = () => clamp(Math.round(Number(property('call_forward_max_hops', '5'))) || 5, 1, 10)
const forwardSeconds = () => clamp(Math.round(Number(property('call_forward_ring_seconds', '30'))) || 30, 10, 120)
const callRingSeconds = () => Math.max(10, Number(property('call_ring_seconds', '45')) || 45)

function calls(op, parameters) {
  return ec.resource.script(CALLS).execute(op, parameters) || {}
}
function voice(op, parameters) {
  return ec.resource.script(VOICE).execute(op, parameters) || {}
}

/** A refusal: an error for the user and a reason code for the caller. */
function refuse(reasonCode, text) {
  ec.message.addError(ec.l10n.localize(text))
  return { reasonCode }
}

function isAdmin() {
  return !!ec.user.userId && ec.user.hasPermission('VoiceAdmin')
}
function notAdmin() {
  return isAdmin() ? null : refuse('notAllowed', 'Only a voice administrator may do this')
}

function userName(userId) {
  const account = userId ? one(ACCOUNT, { userId }) : null
  return account ? account.userFullName || account.username || userId : userId
}

function makeOrUpdate(entity, pk, fields) {
  const row = one(entity, pk)
  if (row) {
    row.setFields(fields, true, null, null)
    row.update()
    return row
  }
  const made = ec.entity.makeValue(entity)
  made.setFields(Object.assign({}, pk, fields), true, null, null)
  made.create()
  return made
}

// ---------- time and schedules ----------

function minutesOf(text) {
  const match = /^(\d{1,2}):(\d{2})$/.exec(String(text || '').trim())
  if (!match) return null
  const minutes = Number(match[1]) * 60 + Number(match[2])
  return minutes >= 0 && minutes <= 1440 ? minutes : null
}

/** The local date (yyyy-MM-dd), minute of the day and day of the week (1 Monday to 7 Sunday) of atMs in zone. */
function localTime(atMs, zone) {
  const machine = ec.user.machineTimeZone || 'UTC'
  const name = blank(zone) ? machine : String(zone).trim()
  let text
  try {
    text = ec.l10n.format(Number(atMs), 'yyyy-MM-dd HH:mm EEE', null, name)
  } catch (err) {
    // a zone that is not known (data loaded around storeSchedule's check) is logged and read as the machine's, so one
    // schedule does not stop every call that reads it
    ec.logger.error(`Call handling: time zone ${name} is not known, read as ${machine}: ${err && err.message ? err.message : err}`)
    text = ec.l10n.format(Number(atMs), 'yyyy-MM-dd HH:mm EEE', null, machine)
  }
  const [date, time, day] = text.split(' ')
  return { date, minutes: minutesOf(time), dow: DAY_NAMES[day] }
}

/** The day before a yyyy-MM-dd date's day of the week. */
const previousDow = (dow) => (dow === 1 ? 7 : dow - 1)

function inHours(rule, minutes) {
  const from = minutesOf(rule.fromTime) ?? 0
  const thru = minutesOf(rule.thruTime) ?? 1440
  return thru > from ? minutes >= from && minutes < thru : minutes >= from
}

function onDates(rule, date) {
  const from = String(rule.onDate || '')
  const thru = String(rule.thruDate || rule.onDate || '')
  if (!/^\d{4}-\d{2}-\d{2}$/.test(from)) return false
  if (yes(rule.repeatYearly)) {
    const [d, f, t] = [date.slice(5), from.slice(5), thru.slice(5)]
    return f <= t ? d >= f && d <= t : d >= f || d <= t
  }
  return date >= from && date <= thru
}

/**
 * Whether a schedule is open at atMs, in its time zone: a holiday on the date closes it; hours on the date replace the
 * weekly hours; else its weekly hours, a span past midnight counted on the day after too. null for a schedule not found.
 */
function scheduleOpen(commsScheduleId, atMs) {
  const schedule = commsScheduleId ? one(SCHEDULE, { commsScheduleId }) : null
  if (!schedule) return null
  const t = localTime(atMs, schedule.timeZone)
  const rules = rows(SCHEDULE_RULE, { commsScheduleId })
  const dated = rules.filter((r) => r.ruleTypeEnumId !== 'CsrWeekly' && onDates(r, t.date))
  if (dated.length) {
    if (dated.some((r) => r.ruleTypeEnumId === 'CsrHoliday')) return false
    return dated.some((r) => inHours(r, t.minutes))
  }
  return rules.filter((r) => r.ruleTypeEnumId === 'CsrWeekly').some((r) => {
    const day = Number(r.dayOfWeek)
    if (day === t.dow && inHours(r, t.minutes)) return true
    const from = minutesOf(r.fromTime) ?? 0
    const thru = minutesOf(r.thruTime) ?? 1440
    return day === previousDow(t.dow) && thru <= from && t.minutes < thru
  })
}

// ---------- callers and users ----------

/** A caller who gives no number: none, or a name for none (anonymous, restricted, withheld). */
function isAnonymous(raw) {
  const text = String(raw || '').trim()
  if (!text) return true
  if (/anonymous|restricted|private|unavailable|unknown|withheld/i.test(text)) return true
  return !/\d{3,}/.test(text)
}

function onList(callerListId, number) {
  if (!callerListId || !number) return false
  return rows(ENTRY, { callerListId }).some((e) => {
    const entry = String(e.phoneNumber || '')
    return entry.endsWith('*') ? number.startsWith(entry.slice(0, -1)) : entry === number
  })
}

function settingOf(userId) {
  return (userId && one(SETTING, { userId })) || {}
}

/** Do not disturb: the user's status (UserStatus, from the account menu or Phone settings), until it clears. */
function dnd(userId) {
  const status = userId ? one(STATUS, { userId }) : null
  if (!status || status.statusEnumId !== 'UpsDnd') return false
  return !(status.clearAtDate && ms(status.clearAtDate) <= nowMs())
}

function inCall(userId) {
  const status = userId ? one(STATUS, { userId }) : null
  return !!status && status.inCall === 'Y'
}

/** Busy: in a call with call waiting off. */
function busy(userId) {
  return inCall(userId) && settingOf(userId).callWaiting !== 'Y'
}

function enabledUser(userId) {
  const account = userId ? one(ACCOUNT, { userId }) : null
  return !!account && account.disabled !== 'Y'
}

/** How long a user's ring step rings: their box's rings (6 seconds each), else call_ring_seconds, as voicemail does. */
function userRingSeconds(userId) {
  const box = one(BOX, { userId })
  const rings = box && box.rings ? clamp(Number(box.rings), 1, MAX_RINGS) : 0
  return rings ? rings * RING_SECONDS_EACH : callRingSeconds()
}

// ---------- the plan ----------

/** A step of the plan; a trace line says why, for the evaluate service and the logs. */
function trace(ctx, text) {
  ctx.trace.push(text)
}

/** A rule applies now: enabled, and its schedule (when it has one) open or closed as it asks. */
function ruleApplies(rule, ctx) {
  if (rule.disabled === 'Y') return false
  if (!rule.commsScheduleId) return true
  const open = scheduleOpen(rule.commsScheduleId, ctx.atMs)
  if (open === null) return false
  return rule.scheduleWhen === 'closed' ? !open : open
}

/**
 * Where a forward to an outside number goes: one of this deployment's own numbers routes inside (its user or group);
 * the caller's own number, or one the policy or the user's permission refuses, is skipped (null).
 */
function numberSteps(raw, ownerUserId, ctx, byAdmin) {
  const check = voice('forwardCheck', { phoneNumber: raw, userId: ownerUserId, byAdmin: !!byAdmin })
  if (!check.number) {
    trace(ctx, `forward to ${raw} skipped: ${check.reasonCode || 'not a number'}`)
    return null
  }
  if (ctx.from && check.number === ctx.from) {
    trace(ctx, `forward to ${check.number} skipped: it is the caller`)
    return null
  }
  const own = one(NUMBER, { phoneNumber: check.number })
  if (own && own.routeId && ctx.seen.has((own.routeTypeEnumId === 'PnrRingGroup' ? 'g:' : 'u:') + own.routeId)) {
    trace(ctx, `forward to ${check.number} skipped: a loop`)
    return null
  }
  if (own) {
    if (own.routeTypeEnumId === 'PnrUser' && own.routeId) return hop(ctx, `own number ${check.number}`, () => planUser(own.routeId, ctx))
    if (own.routeTypeEnumId === 'PnrRingGroup' && own.routeId) return hop(ctx, `own number ${check.number}`, () => planGroup(own.routeId, ctx))
    trace(ctx, `forward to ${check.number} skipped: one of our numbers that rings no one`)
    return null
  }
  if (!check.allowed) {
    trace(ctx, `forward to ${check.number} skipped: ${check.reasonCode}`)
    return null
  }
  return [{ type: 'ring', users: [], numbers: [{ number: check.number, userId: ownerUserId || null, byAdmin: !!byAdmin }],
    seconds: forwardSeconds(), role: 'forward' }]
}

/** One more forward: refused (null) past call_forward_max_hops. */
function hop(ctx, what, build) {
  if (ctx.hops + 1 > maxHops()) {
    trace(ctx, `${what} skipped: more than ${maxHops()} forwards`)
    return null
  }
  ctx.hops++
  try {
    return build()
  } finally {
    ctx.hops--
  }
}

/** The steps of a rule's target, for a call to ownerUserId; null when it does not apply (a loop, a refused number). */
function targetSteps(rule, ownerUserId, ctx) {
  const id = rule.targetId
  switch (rule.targetTypeEnumId) {
    case 'ChtVoicemail': return [{ type: 'voicemail', userId: ownerUserId }]
    case 'ChtReject': return [{ type: 'reject', reasonCode: 'declined' }]
    case 'ChtNumber': {
      const steps = numberSteps(id, ownerUserId, ctx, false)
      return steps ? steps.concat([{ type: 'voicemail', userId: ownerUserId }]) : null
    }
    case 'ChtUser':
      if (!id || id === ownerUserId || ctx.seen.has('u:' + id)) {
        trace(ctx, `forward to user ${id} skipped: a loop`)
        return null
      }
      return hop(ctx, `forward to user ${id}`, () => planUser(id, ctx))
    case 'ChtGroup':
      if (!id || ctx.seen.has('g:' + id)) {
        trace(ctx, `forward to group ${id} skipped: a loop`)
        return null
      }
      return hop(ctx, `forward to group ${id}`, () => planGroup(id, ctx))
    default: return null
  }
}

/** A selective rule's target when it has none of its own: reject for reject and accept, nothing for forward. */
function selectiveSteps(rule, userId, ctx, fallback) {
  if (!rule.targetTypeEnumId) return fallback
  return targetSteps(rule, userId, ctx) || fallback
}

/** The plan of a call to a user, their rules read in order (the module comment). */
function planUser(userId, ctx) {
  if (!enabledUser(userId)) {
    trace(ctx, `user ${userId} not found`)
    return null
  }
  ctx.seen.add('u:' + userId)
  const rules = rows(RULE, { userId }).filter((r) => KINDS.includes(r.kindEnumId))
    .sort((a, b) => (Number(a.sequenceNum) || 0) - (Number(b.sequenceNum) || 0))
    .filter((r) => ruleApplies(r, ctx))
  const of = (kind) => rules.filter((r) => r.kindEnumId === kind)
  const reject = (reasonCode) => [{ type: 'reject', reasonCode }]
  if (ctx.anonymous && of('ChrAnonymousReject').length) {
    trace(ctx, `${userId}: anonymous caller rejected`)
    return selectiveSteps(of('ChrAnonymousReject')[0], userId, ctx, reject('anonymous'))
  }
  for (const rule of of('ChrSelectiveReject')) {
    if (onList(rule.callerListId, ctx.from)) {
      trace(ctx, `${userId}: caller on a reject list`)
      return selectiveSteps(rule, userId, ctx, reject('declined'))
    }
  }
  const accepts = of('ChrSelectiveAccept')
  if (accepts.length && !accepts.some((rule) => onList(rule.callerListId, ctx.from))) {
    trace(ctx, `${userId}: caller not on an accept list`)
    return selectiveSteps(accepts[0], userId, ctx, reject('declined'))
  }
  for (const rule of of('ChrSelectiveForward')) {
    if (!onList(rule.callerListId, ctx.from)) continue
    const steps = targetSteps(rule, userId, ctx)
    if (steps) {
      trace(ctx, `${userId}: caller on a forward list`)
      return steps
    }
  }
  if (dnd(userId)) {
    trace(ctx, `${userId}: do not disturb, to voicemail`)
    return [{ type: 'voicemail', userId }]
  }
  for (const rule of of('ChrAlways')) {
    const steps = targetSteps(rule, userId, ctx)
    if (steps) {
      trace(ctx, `${userId}: always forward`)
      return steps
    }
  }
  if (busy(userId)) {
    const rule = of('ChrBusy')[0]
    const steps = rule ? targetSteps(rule, userId, ctx) : null
    trace(ctx, `${userId}: busy`)
    return steps || [{ type: 'voicemail', userId }]
  }
  const ring = { type: 'ring', users: [userId], numbers: mobileOf(userId, ctx), seconds: userRingSeconds(userId), role: 'callee' }
  const noAnswer = of('ChrNoAnswer')[0]
  const after = noAnswer ? targetSteps(noAnswer, userId, ctx) : null
  return [ring].concat(after || [{ type: 'voicemail', userId }])
}

/** The user's mobile, to ring with them, when it is on, in its schedule, not the caller and allowed. */
function mobileOf(userId, ctx) {
  const setting = settingOf(userId)
  if (setting.ringMobile !== 'Y' || blank(setting.mobileNumber)) return []
  if (setting.mobileScheduleId && scheduleOpen(setting.mobileScheduleId, ctx.atMs) !== true) return []
  const check = voice('forwardCheck', { phoneNumber: setting.mobileNumber, userId, byAdmin: false })
  if (!check.number || !check.allowed || check.number === ctx.from || one(NUMBER, { phoneNumber: check.number })) {
    trace(ctx, `${userId}: mobile not rung: ${check.reasonCode || 'the caller, or one of our numbers'}`)
    return []
  }
  return [{ number: check.number, userId, byAdmin: false }]
}

/** Whether a ring group's member takes a call now: an enabled user, not in do not disturb, not busy. */
function memberFree(userId) {
  return enabledUser(userId) && !dnd(userId) && !busy(userId)
}

function membersOf(ringGroupId) {
  return rows(MEMBER, { ringGroupId }).sort((a, b) => (Number(a.sequenceNum) || 0) - (Number(b.sequenceNum) || 0) ||
    String(a.userId).localeCompare(String(b.userId)))
}

/** The plan of a call to a ring group: its strategy over its free members, then its fallback. */
function planGroup(ringGroupId, ctx) {
  const group = ringGroupId ? one(GROUP, { ringGroupId }) : null
  if (!group) {
    trace(ctx, `ring group ${ringGroupId} not found`)
    return null
  }
  ctx.seen.add('g:' + ringGroupId)
  const fallback = fallbackSteps(group, ctx)
  if (group.commsScheduleId && scheduleOpen(group.commsScheduleId, ctx.atMs) === false) {
    trace(ctx, `group ${ringGroupId}: closed, to its fallback`)
    return fallback
  }
  const all = membersOf(ringGroupId)
  const free = all.filter((m) => memberFree(m.userId)).map((m) => m.userId)
  if (!free.length) {
    trace(ctx, `group ${ringGroupId}: no member free, to its fallback`)
    return fallback
  }
  const seconds = clamp(Number(group.ringSeconds) || 20, 5, 300)
  const base = { type: 'ring', seconds, role: 'ringGroupMember', ringGroupId }
  // each member's mobile rings with them, as for a call to them
  const mobiles = (users) => users.flatMap((userId) => mobileOf(userId, ctx))
  const strategy = STRATEGIES.includes(group.strategyEnumId) ? group.strategyEnumId : 'RgsSimultaneous'
  if (strategy === 'RgsSimultaneous') return [Object.assign({}, base, { users: free, numbers: mobiles(free) })].concat(fallback)
  let order = free
  if (strategy === 'RgsRoundRobin') {
    // start after the member rung first last time, in the order of all members
    const ids = all.map((m) => m.userId)
    const last = ids.indexOf(group.lastMemberUserId)
    const rotated = last < 0 ? ids : ids.slice(last + 1).concat(ids.slice(0, last + 1))
    order = rotated.filter((id) => free.includes(id))
  }
  return order.map((userId, i) => Object.assign({}, base, { users: [userId], numbers: mobiles([userId]),
    roundRobin: strategy === 'RgsRoundRobin' && i === 0 }))
    .concat(fallback)
}

function fallbackSteps(group, ctx) {
  const id = group.fallbackId
  switch (group.fallbackTypeEnumId) {
    case 'RgfVoicemail': return [{ type: 'voicemail', ringGroupId: group.ringGroupId }]
    case 'RgfGroup': {
      if (!id || ctx.seen.has('g:' + id)) {
        trace(ctx, `group ${group.ringGroupId}: fallback group ${id} skipped: a loop`)
        return [{ type: 'end' }]
      }
      return hop(ctx, `fallback group ${id}`, () => planGroup(id, ctx)) || [{ type: 'end' }]
    }
    case 'RgfNumber': return (numberSteps(id, null, ctx, true) || []).concat([{ type: 'end' }])
    default: return [{ type: 'end' }]
  }
}

/** The plan of a call to a user or a ring group, with its trace. */
function buildPlan(p) {
  const atMs = blank(p.atMs) ? nowMs() : Number(p.atMs)
  const from = p.fromNumber && /^\+\d{7,15}$/.test(String(p.fromNumber)) ? String(p.fromNumber) : null
  const ctx = { atMs, from, anonymous: !from && isAnonymous(p.rawFrom != null ? p.rawFrom : p.fromNumber), hops: 0, seen: new Set(), trace: [] }
  const steps = p.ringGroupId ? planGroup(p.ringGroupId, ctx) : planUser(p.userId, ctx)
  return { steps: steps && steps.length ? steps : [{ type: 'reject', reasonCode: 'notFound' }], trace: ctx.trace }
}

/** Internal: the plan for a call, without placing it (the spec and support read it; atMs is the time it is read at). */
function evaluate(p) {
  if (blank(p.userId) && blank(p.ringGroupId)) return refuse('notFound', 'Name a user or a ring group')
  const plan = buildPlan(p)
  return { stepList: plan.steps, traceList: plan.trace }
}

// ---------- running a plan ----------

function routeOf(callSessionId) {
  return find(ROUTE).condition('callSessionId', callSessionId).forUpdate(true).one()
}

function planOf(route) {
  try {
    const steps = JSON.parse(String(route.planJson || '[]'))
    return Array.isArray(steps) ? steps : []
  } catch (err) {
    ec.logger.error(`Call ${route.callSessionId}: its handling plan does not read: ${err && err.message ? err.message : err}`)
    return []
  }
}

/** A ring step: the users still free (a group's members are read again), the forwards allowed; null when none rings. */
function runRing(route, step, displayName) {
  const users = (step.users || []).filter((userId) => step.role === 'ringGroupMember' ? memberFree(userId) : enabledUser(userId))
  const numbers = (step.numbers || []).filter((n) => (step.role !== 'ringGroupMember' || users.includes(n.userId)) &&
    voice('forwardCheck', { phoneNumber: n.number, userId: n.userId, byAdmin: !!n.byAdmin }).allowed)
  if (!users.length && !numbers.length) return null
  const rung = calls('ringStep', { callSessionId: route.callSessionId, userIdList: users, numberList: numbers, role: step.role,
    ringSeconds: step.seconds + RING_GRACE_SECONDS, displayName })
  let dialed = 0
  for (const leg of rung.forwardLegList || []) {
    const out = voice('dialForward', { callSessionId: route.callSessionId, callLegId: leg.callLegId, phoneNumber: leg.number,
      userId: leg.userId, byAdmin: !!leg.byAdmin, fromNumber: route.toNumber })
    if (out.dialed) dialed++
  }
  // only numbers, each refused at its dial (a limit reached): nothing rings, so the plan goes on now, not after the timer
  if (!users.length && !dialed) return null
  if (step.roundRobin && step.ringGroupId && users.length) {
    const group = find(GROUP).condition('ringGroupId', step.ringGroupId).forUpdate(true).one()
    if (group) {
      group.setFields({ lastMemberUserId: users[0], lastRungDate: ec.user.nowTimestamp }, true, null, null)
      group.update()
    }
  }
  return { mode: 'ring', ringSeconds: step.seconds }
}

/** A voicemail step's box: the user's, or the ring group's, made when it has none. */
function boxFor(step) {
  if (step.ringGroupId) {
    let box = find(BOX).condition('ringGroupId', step.ringGroupId).one()
    if (!box && one(GROUP, { ringGroupId: step.ringGroupId })) {
      box = ec.entity.makeValue(BOX)
      box.setSequencedIdPrimary()
      box.setFields({ ringGroupId: step.ringGroupId, attachAudio: 'N', attachTranscript: 'N' }, true, null, null)
      box.create()
    }
    return box
  }
  if (!step.userId || !enabledUser(step.userId)) return null
  let box = one(BOX, { userId: step.userId })
  if (!box) {
    box = ec.entity.makeValue(BOX)
    box.setSequencedIdPrimary()
    box.setFields({ userId: step.userId, attachAudio: 'N', attachTranscript: 'N' }, true, null, null)
    box.create()
  }
  return box
}

/** Runs the plan from step index on: the first step that rings, or the box, a refusal or the end. */
function runFrom(route, index, displayName) {
  const steps = planOf(route)
  let i = index
  for (; i < steps.length; i++) {
    const step = steps[i] || {}
    if (step.type === 'ring') {
      const out = runRing(route, step, displayName)
      if (out) return Object.assign(save(route, i), out)
      continue
    }
    if (step.type === 'voicemail') {
      const box = boxFor(step)
      if (!box) continue
      return Object.assign(save(route, i), { mode: 'voicemail', voicemailBoxId: box.voicemailBoxId })
    }
    if (step.type === 'reject') return Object.assign(save(route, i), { mode: 'reject', reasonCode: step.reasonCode || 'declined' })
    return Object.assign(save(route, i), { mode: 'end' })
  }
  return Object.assign(save(route, steps.length), { mode: 'end' })
}

function save(route, index) {
  route.setFields({ stepIndex: index, stepSeq: (Number(route.stepSeq) || 0) + 1, stepDate: ec.user.nowTimestamp }, true, null, null)
  route.update()
  return {}
}

/** The token of the step a plan is on: the engine's timer and the voicemail control carry it back to advance. */
function stepToken(route) {
  return String(Number(route.stepSeq) || 0)
}

/**
 * Internal (VoiceServices receive#PhoneCall): an inbound call to a user or a ring group arrives. Its plan is made and
 * its first step run. Answers what receive answers the SIP engine: a refusal (reasonCode anonymous, declined, notFound),
 * or the call, which goes to voicemail at once (voicemail busy) or rings for voicemailSeconds before it moves on.
 */
function inbound(p) {
  const plan = buildPlan({ userId: p.userId, ringGroupId: p.ringGroupId, fromNumber: p.fromNumber, rawFrom: p.rawFrom, atMs: p.atMs })
  if (plan.trace.length) ec.logger.info(`Call to ${p.toNumber}: ${plan.trace.join('; ')}`)
  const first = plan.steps[0]
  if (first.type === 'reject') return { reasonCode: first.reasonCode || 'declined' }
  if (first.type === 'end') return { reasonCode: 'unavailable' }
  // a user straight to voicemail is still the call's callee (missed, in their history), as V2's busy user is
  const voicemailUser = first.type === 'voicemail' && first.userId ? first.userId : null
  const ringSeconds = first.type === 'ring' ? first.seconds : callRingSeconds()
  const started = calls('startPhone', { userId: voicemailUser, direction: 'inbound', phoneNumber: p.fromNumber || p.rawFrom,
    displayName: p.displayName, mediaNodeId: p.instanceId, silent: true, ringing: true,
    ringExpireSeconds: ringSeconds + RING_GRACE_SECONDS })
  const route = ec.entity.makeValue(ROUTE)
  route.setFields({ callSessionId: started.callSessionId, userId: p.userId || null, ringGroupId: p.ringGroupId || null,
    toNumber: p.toNumber || null, fromNumber: p.fromNumber || null, planJson: JSON.stringify(plan.steps), stepIndex: 0,
    hopCount: 0, createdDate: ec.user.nowTimestamp, stepDate: ec.user.nowTimestamp }, true, null, null)
  route.create()
  const out = runFrom(route, 0, p.displayName)
  const answer = { callSessionId: started.callSessionId, callLegId: started.phoneLegId }
  if (out.mode === 'ring') return Object.assign(answer, { voicemail: null, voicemailSeconds: out.ringSeconds, stepToken: stepToken(route) })
  if (out.mode === 'voicemail') return Object.assign(answer, { voicemail: 'busy', stepToken: stepToken(route) })
  // nothing to ring and no box: the call ends at once
  calls('phoneEvent', { callSessionId: started.callSessionId, callLegId: started.phoneLegId, event: 'failed' })
  return { reasonCode: out.mode === 'reject' ? out.reasonCode : 'unavailable' }
}

/**
 * Internal (VoicemailServices.begin#Voicemail, for the SIP engine): the call's current step is over (its ring timer,
 * reason noAnswer; everyone rung declined, declined; a forward failed, noAnswer) or it goes to voicemail now (busy).
 * handled false for a call with no plan (V2's path). Otherwise: mode ring (the next step rings, for ringSeconds),
 * voicemail (voicemailBoxId), or a reasonCode: answered (someone answered first), callEnded, noRoute (the plan ended).
 */
function advance(p) {
  const route = p.callSessionId ? routeOf(p.callSessionId) : null
  if (!route) return { handled: false }
  const session = find(SESSION).condition('callSessionId', p.callSessionId).forUpdate(true).one()
  if (!session || session.statusId === 'CssEnded') return { handled: true, reasonCode: 'callEnded' }
  // a timer or control of a step the plan has already left (one that arrived late) changes nothing
  if (!blank(p.stepToken) && String(p.stepToken) !== stepToken(route)) return { handled: true, reasonCode: 'stale' }
  const closed = calls('closeOffers', { callSessionId: p.callSessionId })
  if (closed.reasonCode) return { handled: true, reasonCode: closed.reasonCode }
  const steps = planOf(route)
  const index = Number(route.stepIndex) || 0
  const current = steps[index] || {}
  const displayName = closed.displayName || null
  const out = p.reason === 'busy' || current.type === 'voicemail' ? runFrom(route, index, displayName)
    : runFrom(route, index + 1, displayName)
  if (out.mode === 'ring') return { handled: true, mode: 'ring', ringSeconds: out.ringSeconds, stepToken: stepToken(route) }
  if (out.mode === 'voicemail') return { handled: true, mode: 'voicemail', voicemailBoxId: out.voicemailBoxId }
  return { handled: true, reasonCode: 'noRoute' }
}

// ---------- the user's settings ----------

function ruleView(rule) {
  const target = rule.targetTypeEnumId
  let targetName = rule.targetId || null
  if (target === 'ChtUser') targetName = userName(rule.targetId)
  else if (target === 'ChtGroup') targetName = one(GROUP, { ringGroupId: rule.targetId })?.name || rule.targetId
  return {
    callHandlingRuleId: rule.callHandlingRuleId, kindEnumId: rule.kindEnumId, targetTypeEnumId: target || null,
    targetId: rule.targetId || null, targetName, callerListId: rule.callerListId || null,
    callerListName: rule.callerListId ? one(LIST, { callerListId: rule.callerListId })?.name || null : null,
    commsScheduleId: rule.commsScheduleId || null,
    scheduleName: rule.commsScheduleId ? one(SCHEDULE, { commsScheduleId: rule.commsScheduleId })?.name || null : null,
    scheduleWhen: rule.scheduleWhen === 'closed' ? 'closed' : 'open', sequenceNum: Number(rule.sequenceNum) || 0,
    disabled: rule.disabled === 'Y',
  }
}

function listsOf(userId) {
  return Array.from(find(LIST).condition('userId', userId).orderBy('name').list()).map((list) => ({
    callerListId: list.callerListId, name: list.name,
    entryList: Array.from(find(ENTRY).condition('callerListId', list.callerListId).orderBy('phoneNumber').list())
      .map((e) => ({ phoneNumber: e.phoneNumber, name: e.name || null })),
  }))
}

/** The schedules a user may use: their own and the deployment's. */
function schedulesFor(userId) {
  return Array.from(find(SCHEDULE).orderBy('name').list()).filter((s) => !s.ownerUserId || s.ownerUserId === userId)
    .map((s) => ({ commsScheduleId: s.commsScheduleId, name: s.name || s.commsScheduleId, timeZone: s.timeZone || null, shared: !s.ownerUserId }))
}

function getSettings() {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const setting = settingOf(me)
  const voiceSetting = one('moqui.call.VoiceUserSetting', { userId: me }) || {}
  const enumsOf = (enumTypeId) => Array.from(find('moqui.basic.Enumeration').condition('enumTypeId', enumTypeId).orderBy('sequenceNum').list())
    .map((e) => ({ enumId: e.enumId, description: ec.l10n.localize(e.description) }))
  return {
    ruleList: Array.from(find(RULE).condition('userId', me).orderBy('sequenceNum').list()).map(ruleView),
    callerListList: listsOf(me),
    scheduleList: schedulesFor(me),
    groupList: Array.from(find(GROUP).orderBy('name').list()).map((g) => ({ ringGroupId: g.ringGroupId, name: g.name || g.ringGroupId })),
    settings: {
      callWaiting: setting.callWaiting === 'Y', mobileNumber: setting.mobileNumber || null, ringMobile: setting.ringMobile === 'Y',
      mobileScheduleId: setting.mobileScheduleId || null, hideCallerId: setting.hideCallerId === 'Y', dnd: dnd(me),
      forwardEnabled: voiceSetting.forwardEnabled === 'Y' && voiceSetting.outboundEnabled === 'Y',
    },
    kindList: enumsOf('CallHandlingKind'), targetList: enumsOf('CallHandlingTarget'),
  }
}

function updateSettings(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const fields = {}
  for (const key of ['callWaiting', 'ringMobile', 'hideCallerId']) if (p[key] != null && p[key] !== '') fields[key] = yes(p[key]) ? 'Y' : 'N'
  if (p.mobileNumber != null) {
    if (blank(p.mobileNumber)) fields.mobileNumber = null
    else {
      const check = voice('forwardCheck', { phoneNumber: p.mobileNumber, userId: me, byAdmin: false })
      if (!check.number || check.destinationClass === 'emergency') return refuse('badNumber', 'The mobile number is not a phone number')
      fields.mobileNumber = check.number
    }
  }
  if (p.mobileScheduleId != null) {
    if (blank(p.mobileScheduleId)) fields.mobileScheduleId = null
    else if (!schedulesFor(me).some((s) => s.commsScheduleId === p.mobileScheduleId)) return refuse('notFound', 'Schedule not found')
    else fields.mobileScheduleId = p.mobileScheduleId
  }
  makeOrUpdate(SETTING, { userId: me }, fields)
  if (p.dnd != null && p.dnd !== '' && yes(p.dnd) !== dnd(me)) {
    ec.service.sync().name('org.moqui.impl.ChatServices.set#UserStatus').parameter('status', yes(p.dnd) ? 'dnd' : 'online').call()
  }
  const out = getSettings()
  const warning = out.settings.ringMobile && out.settings.mobileNumber && !out.settings.forwardEnabled
    ? ec.l10n.localize('Your mobile does not ring yet: an administrator has to allow calls to be forwarded to outside numbers for you.') : null
  return { settings: out.settings, warning }
}

/** A rule's target, checked: an outside number in E.164 (never an emergency number), a user, a group, voicemail, reject. */
function checkTarget(p, me) {
  const type = blank(p.targetTypeEnumId) ? null : p.targetTypeEnumId
  if (type && !TARGETS.includes(type)) return { refusal: refuse('badRule', 'Choose where the calls go') }
  if (type === 'ChtNumber') {
    const check = voice('forwardCheck', { phoneNumber: p.targetId, userId: me, byAdmin: false })
    if (!check.number || check.destinationClass === 'emergency') return { refusal: refuse('badNumber', 'Forward to a phone number with its country code') }
    return { type, targetId: check.number }
  }
  if (type === 'ChtUser') {
    if (!enabledUser(p.targetId) || p.targetId === me) return { refusal: refuse('notFound', 'Choose someone else to forward to') }
    return { type, targetId: p.targetId }
  }
  if (type === 'ChtGroup') {
    if (!p.targetId || !one(GROUP, { ringGroupId: p.targetId })) return { refusal: refuse('notFound', 'Ring group not found') }
    return { type, targetId: p.targetId }
  }
  return { type, targetId: null }
}

function storeRule(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  let rule = blank(p.callHandlingRuleId) ? null : one(RULE, { callHandlingRuleId: p.callHandlingRuleId })
  if (!blank(p.callHandlingRuleId) && (!rule || rule.userId !== me)) return refuse('notFound', 'Rule not found')
  const kind = p.kindEnumId || rule?.kindEnumId
  if (!KINDS.includes(kind)) return refuse('badRule', 'Choose what the rule does')
  const target = checkTarget({ targetTypeEnumId: p.targetTypeEnumId ?? rule?.targetTypeEnumId, targetId: p.targetId ?? rule?.targetId }, me)
  if (target.refusal) return target.refusal
  const needsTarget = ['ChrSelectiveForward', 'ChrAlways', 'ChrBusy', 'ChrNoAnswer'].includes(kind)
  if (needsTarget && !target.type) return refuse('badRule', 'Choose where the calls go')
  const callerListId = p.callerListId !== undefined && p.callerListId !== null ? p.callerListId || null : rule?.callerListId || null
  if (SELECTIVE.includes(kind)) {
    const list = callerListId ? one(LIST, { callerListId }) : null
    if (!list || list.userId !== me) return refuse('badRule', 'Choose a list of callers')
  }
  const commsScheduleId = p.commsScheduleId === undefined || p.commsScheduleId === null ? rule?.commsScheduleId || null : p.commsScheduleId || null
  if (commsScheduleId && !schedulesFor(me).some((s) => s.commsScheduleId === commsScheduleId)) return refuse('notFound', 'Schedule not found')
  if (!rule && find(RULE).condition('userId', me).count() >= MAX_RULES) return refuse('tooMany', 'You have too many rules')
  const fields = {
    userId: me, kindEnumId: kind, targetTypeEnumId: target.type, targetId: target.targetId,
    callerListId: SELECTIVE.includes(kind) ? callerListId : null, commsScheduleId,
    scheduleWhen: (p.scheduleWhen ?? rule?.scheduleWhen) === 'closed' ? 'closed' : 'open',
    sequenceNum: blank(p.sequenceNum) ? (rule ? rule.sequenceNum : (find(RULE).condition('userId', me).count() + 1) * 10) : Number(p.sequenceNum),
    disabled: p.disabled == null || p.disabled === '' ? rule?.disabled || 'N' : yes(p.disabled) ? 'Y' : 'N',
  }
  if (!rule) {
    rule = ec.entity.makeValue(RULE)
    rule.setSequencedIdPrimary()
    rule.setFields(fields, true, null, null)
    rule.create()
  } else {
    rule.setFields(fields, true, null, null)
    rule.update()
  }
  return { callHandlingRuleId: rule.callHandlingRuleId, rule: ruleView(rule) }
}

function deleteRule(p) {
  const me = ec.user.userId
  const rule = me && p.callHandlingRuleId ? one(RULE, { callHandlingRuleId: p.callHandlingRuleId }) : null
  if (!rule || rule.userId !== me) return refuse('notFound', 'Rule not found')
  rule.delete()
  return {}
}

function myList(callerListId) {
  const me = ec.user.userId
  const list = me && callerListId ? one(LIST, { callerListId }) : null
  return list && list.userId === me ? list : null
}

function storeList(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const name = String(p.name || '').trim().slice(0, 100)
  if (!name) return refuse('noName', 'Give the list a name')
  if (!blank(p.callerListId)) {
    const list = myList(p.callerListId)
    if (!list) return refuse('notFound', 'List not found')
    list.set('name', name)
    list.update()
    return { callerListId: list.callerListId }
  }
  if (find(LIST).condition('userId', me).count() >= MAX_LISTS) return refuse('tooMany', 'You have too many lists')
  const list = ec.entity.makeValue(LIST)
  list.setSequencedIdPrimary()
  list.setFields({ userId: me, name }, true, null, null)
  list.create()
  return { callerListId: list.callerListId }
}

function deleteList(p) {
  const list = myList(p.callerListId)
  if (!list) return refuse('notFound', 'List not found')
  if (find(RULE).condition('callerListId', list.callerListId).count() > 0) return refuse('inUse', 'A rule uses this list; change the rule first')
  find(ENTRY).condition('callerListId', list.callerListId).deleteAll()
  list.delete()
  return {}
}

function addEntry(p) {
  const list = myList(p.callerListId)
  if (!list) return refuse('notFound', 'List not found')
  const raw = String(p.phoneNumber || '').trim()
  let number
  if (raw.endsWith('*')) {
    number = '+' + raw.replace(/[^0-9]/g, '') + '*'
    if (!/^\+\d{1,14}\*$/.test(number) || !raw.startsWith('+')) return refuse('badNumber', 'A prefix starts with + and its country code, then ends with *')
  } else {
    number = voice('forwardCheck', { phoneNumber: raw, userId: ec.user.userId, byAdmin: false }).number
    if (!number) return refuse('badNumber', 'That is not a phone number')
  }
  if (find(ENTRY).condition('callerListId', list.callerListId).count() >= MAX_ENTRIES) return refuse('tooMany', 'The list is full')
  makeOrUpdate(ENTRY, { callerListId: list.callerListId, phoneNumber: number }, { name: blank(p.name) ? null : String(p.name).trim().slice(0, 100) })
  return { phoneNumber: number }
}

function removeEntry(p) {
  const list = myList(p.callerListId)
  if (!list) return refuse('notFound', 'List not found')
  const entry = one(ENTRY, { callerListId: list.callerListId, phoneNumber: p.phoneNumber })
  if (entry) entry.delete()
  return {}
}

// ---------- ring groups and schedules (voice admins) ----------

function groupView(group) {
  const fallback = group.fallbackTypeEnumId
  return {
    ringGroupId: group.ringGroupId, name: group.name || group.ringGroupId, strategyEnumId: group.strategyEnumId || 'RgsSimultaneous',
    ringSeconds: Number(group.ringSeconds) || 20, fallbackTypeEnumId: fallback || 'RgfVoicemail', fallbackId: group.fallbackId || null,
    fallbackName: fallback === 'RgfGroup' ? one(GROUP, { ringGroupId: group.fallbackId })?.name || group.fallbackId : group.fallbackId || null,
    commsScheduleId: group.commsScheduleId || null,
    scheduleName: group.commsScheduleId ? one(SCHEDULE, { commsScheduleId: group.commsScheduleId })?.name || null : null,
    lastMemberUserId: group.lastMemberUserId || null,
    memberList: membersOf(group.ringGroupId).map((m) => ({ userId: m.userId, userName: userName(m.userId), sequenceNum: Number(m.sequenceNum) || 0 })),
    numberList: rows(NUMBER, { routeTypeEnumId: 'PnrRingGroup', routeId: group.ringGroupId }).map((n) => n.phoneNumber),
  }
}

function findGroups() {
  const denied = notAdmin()
  if (denied) return denied
  return { groupList: Array.from(find(GROUP).orderBy('name').list()).map(groupView) }
}

function storeGroup(p) {
  const denied = notAdmin()
  if (denied) return denied
  let group = blank(p.ringGroupId) ? null : find(GROUP).condition('ringGroupId', p.ringGroupId).forUpdate(true).one()
  if (!blank(p.ringGroupId) && !group) return refuse('notFound', 'Ring group not found')
  const pick = (key, fallback) => (p[key] !== undefined && p[key] !== null ? p[key] : group ? group[key] : fallback)
  const name = String(pick('name', '') || '').trim().slice(0, 100)
  if (!name) return refuse('noName', 'Give the group a name')
  const strategyEnumId = pick('strategyEnumId', 'RgsSimultaneous') || 'RgsSimultaneous'
  if (!STRATEGIES.includes(strategyEnumId)) return refuse('badGroup', 'Choose how the group rings')
  const ringSeconds = Number(pick('ringSeconds', 20)) || 20
  if (!(ringSeconds >= 5 && ringSeconds <= 300)) return refuse('badGroup', 'Ring each member 5 to 300 seconds')
  const fallbackTypeEnumId = pick('fallbackTypeEnumId', 'RgfVoicemail') || 'RgfVoicemail'
  if (!FALLBACKS.includes(fallbackTypeEnumId)) return refuse('badGroup', 'Choose what happens when nobody answers')
  let fallbackId = blank(pick('fallbackId', null)) ? null : String(pick('fallbackId', '')).trim()
  if (fallbackTypeEnumId === 'RgfGroup') {
    if (!fallbackId || !one(GROUP, { ringGroupId: fallbackId }) || (group && fallbackId === group.ringGroupId)) return refuse('notFound', 'Choose another ring group')
  } else if (fallbackTypeEnumId === 'RgfNumber') {
    const check = voice('forwardCheck', { phoneNumber: fallbackId, userId: null, byAdmin: true })
    if (!check.number || check.destinationClass === 'emergency') return refuse('badNumber', 'The fallback is a phone number with its country code')
    fallbackId = check.number
  } else {
    fallbackId = null
  }
  const commsScheduleId = blank(pick('commsScheduleId', null)) ? null : pick('commsScheduleId', null)
  if (commsScheduleId && !one(SCHEDULE, { commsScheduleId, ownerUserId: null })) return refuse('notFound', 'Schedule not found')
  const fields = { name, strategyEnumId, ringSeconds, fallbackTypeEnumId, fallbackId, commsScheduleId }
  if (!group) {
    group = ec.entity.makeValue(GROUP)
    group.setSequencedIdPrimary()
    group.setFields(fields, true, null, null)
    group.create()
  } else {
    group.setFields(fields, true, null, null)
    group.update()
  }
  return { ringGroupId: group.ringGroupId }
}

function deleteGroup(p) {
  const denied = notAdmin()
  if (denied) return denied
  const group = one(GROUP, { ringGroupId: p.ringGroupId })
  if (!group) return refuse('notFound', 'Ring group not found')
  if (find(NUMBER).condition('routeTypeEnumId', 'PnrRingGroup').condition('routeId', group.ringGroupId).count() > 0 ||
    find(GROUP).condition('fallbackTypeEnumId', 'RgfGroup').condition('fallbackId', group.ringGroupId).count() > 0 ||
    find(RULE).condition('targetTypeEnumId', 'ChtGroup').condition('targetId', group.ringGroupId).count() > 0) {
    return refuse('inUse', 'Numbers, other groups or rules send calls to this group; change them first')
  }
  find(MEMBER).condition('ringGroupId', group.ringGroupId).deleteAll()
  // its box stays with its messages, unrouted; the admin can see it was the group's
  group.delete()
  return {}
}

function storeMember(p) {
  const denied = notAdmin()
  if (denied) return denied
  if (!one(GROUP, { ringGroupId: p.ringGroupId })) return refuse('notFound', 'Ring group not found')
  if (!enabledUser(p.userId)) return refuse('notFound', 'User not found')
  const sequenceNum = blank(p.sequenceNum) ? (find(MEMBER).condition('ringGroupId', p.ringGroupId).count() + 1) * 10 : Number(p.sequenceNum)
  makeOrUpdate(MEMBER, { ringGroupId: p.ringGroupId, userId: p.userId }, { sequenceNum })
  return {}
}

function deleteMember(p) {
  const denied = notAdmin()
  if (denied) return denied
  const member = one(MEMBER, { ringGroupId: p.ringGroupId, userId: p.userId })
  if (!member) return refuse('notFound', 'Member not found')
  member.delete()
  return {}
}

/** A schedule the current user may change: their own, or as a voice admin one of the deployment's. */
function changeableSchedule(commsScheduleId) {
  const schedule = commsScheduleId ? one(SCHEDULE, { commsScheduleId }) : null
  if (!schedule) return null
  if (schedule.ownerUserId) return schedule.ownerUserId === ec.user.userId ? schedule : null
  return isAdmin() ? schedule : null
}

function scheduleView(s) {
  return {
    commsScheduleId: s.commsScheduleId, name: s.name || s.commsScheduleId, timeZone: s.timeZone || null, description: s.description || null,
    ownerUserId: s.ownerUserId || null,
    ruleList: Array.from(find(SCHEDULE_RULE).condition('commsScheduleId', s.commsScheduleId).orderBy('ruleTypeEnumId').orderBy('dayOfWeek')
      .orderBy('onDate').orderBy('fromTime').list()).map((r) => ({
      ruleSeqId: r.ruleSeqId, ruleTypeEnumId: r.ruleTypeEnumId, dayOfWeek: r.dayOfWeek == null ? null : Number(r.dayOfWeek),
      fromTime: r.fromTime || null, thruTime: r.thruTime || null, onDate: r.onDate || null, thruDate: r.thruDate || null,
      repeatYearly: r.repeatYearly === 'Y', description: r.description || null,
    })),
  }
}

/** The schedules: the deployment's for a voice admin (shared), else the user's own and the deployment's. */
function findSchedules(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  if (yes(p.shared)) {
    const denied = notAdmin()
    if (denied) return denied
    return { scheduleList: Array.from(find(SCHEDULE).condition('ownerUserId', null).orderBy('name').list()).map(scheduleView) }
  }
  return { scheduleList: Array.from(find(SCHEDULE).orderBy('name').list()).filter((s) => !s.ownerUserId || s.ownerUserId === me).map(scheduleView) }
}

function validZone(zone) {
  if (blank(zone)) return true
  try {
    ec.l10n.format(nowMs(), 'HH:mm', null, String(zone).trim())
    return true
  } catch (err) {
    return false
  }
}

function storeSchedule(p) {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  let schedule = null
  if (!blank(p.commsScheduleId)) {
    schedule = changeableSchedule(p.commsScheduleId)
    if (!schedule) return refuse('notFound', 'Schedule not found')
  } else if (yes(p.shared)) {
    const denied = notAdmin()
    if (denied) return denied
  }
  const name = String(p.name ?? schedule?.name ?? '').trim().slice(0, 100)
  if (!name) return refuse('noName', 'Give the schedule a name')
  const timeZone = p.timeZone === undefined || p.timeZone === null ? schedule?.timeZone || null : String(p.timeZone).trim() || null
  if (!validZone(timeZone)) return refuse('badZone', 'That time zone is not known; use a name such as America/Chicago')
  const fields = { name, timeZone, description: p.description === undefined ? schedule?.description || null : p.description || null }
  if (!schedule) {
    schedule = ec.entity.makeValue(SCHEDULE)
    schedule.setSequencedIdPrimary()
    schedule.setFields(Object.assign({ ownerUserId: yes(p.shared) ? null : me }, fields), true, null, null)
    schedule.create()
  } else {
    schedule.setFields(fields, true, null, null)
    schedule.update()
  }
  return { commsScheduleId: schedule.commsScheduleId }
}

function deleteSchedule(p) {
  const schedule = changeableSchedule(p.commsScheduleId)
  if (!schedule) return refuse('notFound', 'Schedule not found')
  const id = schedule.commsScheduleId
  if (find(RULE).condition('commsScheduleId', id).count() > 0 || find(GROUP).condition('commsScheduleId', id).count() > 0 ||
    find(SETTING).condition('mobileScheduleId', id).count() > 0) {
    return refuse('inUse', 'Rules or ring groups use this schedule; change them first')
  }
  find(SCHEDULE_RULE).condition('commsScheduleId', id).deleteAll()
  schedule.delete()
  return {}
}

function dateText(value) {
  return /^\d{4}-\d{2}-\d{2}$/.test(String(value || '').trim()) ? String(value).trim() : null
}

function storeScheduleRule(p) {
  const schedule = changeableSchedule(p.commsScheduleId)
  if (!schedule) return refuse('notFound', 'Schedule not found')
  const type = p.ruleTypeEnumId
  if (!SCHEDULE_TYPES.includes(type)) return refuse('badRule', 'Choose weekly hours, hours on dates or a holiday')
  const fields = { ruleTypeEnumId: type, repeatYearly: yes(p.repeatYearly) ? 'Y' : 'N', description: blank(p.description) ? null : String(p.description).slice(0, 200) }
  for (const key of ['fromTime', 'thruTime']) {
    if (blank(p[key])) fields[key] = null
    else if (minutesOf(p[key]) === null) return refuse('badTime', 'Times are HH:mm, such as 08:30')
    else fields[key] = String(p[key]).trim().padStart(5, '0')
  }
  if (type === 'CsrWeekly') {
    const day = Number(p.dayOfWeek)
    if (!(Number.isInteger(day) && day >= 1 && day <= 7)) return refuse('badRule', 'Choose a day of the week')
    if (!fields.fromTime || !fields.thruTime) return refuse('badTime', 'Weekly hours need a start and an end')
    Object.assign(fields, { dayOfWeek: day, onDate: null, thruDate: null })
  } else {
    const on = dateText(p.onDate)
    if (!on) return refuse('badDate', 'Dates are yyyy-MM-dd')
    const thru = blank(p.thruDate) ? null : dateText(p.thruDate)
    if (!blank(p.thruDate) && !thru) return refuse('badDate', 'Dates are yyyy-MM-dd')
    Object.assign(fields, { dayOfWeek: null, onDate: on, thruDate: thru })
    if (type === 'CsrHoliday') Object.assign(fields, { fromTime: null, thruTime: null })
  }
  let rule = blank(p.ruleSeqId) ? null : one(SCHEDULE_RULE, { commsScheduleId: schedule.commsScheduleId, ruleSeqId: p.ruleSeqId })
  if (!blank(p.ruleSeqId) && !rule) return refuse('notFound', 'Rule not found')
  if (!rule) {
    rule = ec.entity.makeValue(SCHEDULE_RULE)
    rule.set('commsScheduleId', schedule.commsScheduleId)
    rule.setSequencedIdSecondary()
    rule.setFields(fields, true, null, null)
    rule.create()
  } else {
    rule.setFields(fields, true, null, null)
    rule.update()
  }
  return { ruleSeqId: rule.ruleSeqId }
}

function deleteScheduleRule(p) {
  const schedule = changeableSchedule(p.commsScheduleId)
  const rule = schedule ? one(SCHEDULE_RULE, { commsScheduleId: schedule.commsScheduleId, ruleSeqId: p.ruleSeqId }) : null
  if (!rule) return refuse('notFound', 'Rule not found')
  rule.delete()
  return {}
}

/** Whether a schedule is open now (or at atMs): the admin screen's check. */
function checkSchedule(p) {
  const schedule = p.commsScheduleId ? one(SCHEDULE, { commsScheduleId: p.commsScheduleId }) : null
  if (!schedule || (schedule.ownerUserId && schedule.ownerUserId !== ec.user.userId && !isAdmin())) return refuse('notFound', 'Schedule not found')
  const atMs = blank(p.atMs) ? nowMs() : Number(p.atMs)
  const t = localTime(atMs, schedule.timeZone)
  return { open: !!scheduleOpen(schedule.commsScheduleId, atMs), localDate: t.date, localMinutes: t.minutes, dayOfWeek: t.dow }
}

// ---------- the dialer ----------

/**
 * Numbers and people to call for a text: the recipient-source providers' people with numbers (each source its own page),
 * else the framework's users with a number that rings them; and what was typed, when it is a number.
 */
function searchDialable(p) {
  const me = ec.user.userId
  if (!me) return { resultList: [], number: null }
  const text = String(p.text || '').trim().slice(0, 100)
  const limit = clamp(Number(p.limit) || 10, 1, 25)
  const number = text ? voice('forwardCheck', { phoneNumber: text, userId: me, byAdmin: false }).number || null : null
  const resultList = []
  const failedParts = []
  let unavailable = true
  if (text.length >= 2) {
    const out = ec.service.sync().name('org.moqui.impl.ShellServices.call#Providers')
      .parameters({ hook: 'recipient-source', parameters: { text, kinds: 'phone', limit } }).call() || {}
    ec.message.clearErrors()
    unavailable = out.unavailable !== false
    for (const part of out.failedParts || []) failedParts.push(part)
    for (const result of out.results || []) {
      for (const r of result.recipientList || []) {
        const phones = Array.from(r.phoneList || []).filter((ph) => ph && ph.number)
        if (!phones.length) continue
        resultList.push({ sourceId: result.sourceId, itemId: r.itemId || null, name: r.name || phones[0].number,
          kind: r.kind || 'person', phoneList: phones.map((ph) => ({ number: ph.number, label: ph.label || null })) })
      }
    }
    if (unavailable) {
      // no app answers: the framework's users that a number of ours rings
      const like = '%' + text.toLowerCase() + '%'
      const cf = ec.entity.conditionFactory
      const accounts = Array.from(find(ACCOUNT).condition(cf.makeCondition([cf.makeCondition('username', 'like', like, false, true),
        cf.makeCondition('userFullName', 'like', like, false, true)], 'or')).condition('disabled', '!=', 'Y').limit(50).list())
      for (const account of accounts) {
        if (resultList.length >= limit) break
        const numbers = rows(NUMBER, { routeTypeEnumId: 'PnrUser', routeId: account.userId }).map((n) => n.phoneNumber)
        if (!numbers.length || account.userId === me) continue
        resultList.push({ sourceId: 'framework.users', itemId: account.userId, userId: account.userId,
          name: account.userFullName || account.username, kind: 'person', phoneList: numbers.map((n) => ({ number: n, label: null })) })
      }
    }
  }
  return { resultList: resultList.slice(0, limit * 3), number, failedParts, providersAvailable: !unavailable }
}

/** The dialer's data: the caller IDs (mine and my ring groups'), whether my number is hidden, my recent numbers. */
function dialerData() {
  const me = ec.user.userId
  if (!me) return refuse('notSignedIn', 'Sign in first')
  const ids = voice('myCallerIds', {})
  const recent = []
  const participants = Array.from(find('moqui.call.CallParticipant').condition('userId', me).selectField('callSessionId')
    .orderBy('-callParticipantId').limit(60).list()).map((x) => x.callSessionId)
  if (participants.length) {
    for (const leg of find('moqui.call.CallLeg').condition('callSessionId', 'in', participants).condition('legTypeEnumId', 'CltPstn')
      .orderBy('-offeredDate').limit(60).list()) {
      if (!leg.phoneNumber || recent.some((r) => r.number === leg.phoneNumber)) continue
      recent.push({ number: leg.phoneNumber, whenMs: ms(leg.offeredDate) })
      if (recent.length >= 10) break
    }
  }
  const caps = voice('capabilities', {})
  return { callerIdList: ids.numberList || [], hideCallerId: settingOf(me).hideCallerId === 'Y', recentList: recent,
    phoneActive: !!caps.active, homeCountryCode: voice('homeCountry', {}).homeCountryCode || '1' }
}

function normalize(p) {
  const out = voice('forwardCheck', { phoneNumber: p.phoneNumber, userId: ec.user.userId, byAdmin: false })
  return { number: out.number || null, destinationClass: out.destinationClass || null }
}

const operations = {
  inbound, advance, evaluate, getSettings, updateSettings, storeRule, deleteRule, storeList, deleteList, addEntry, removeEntry,
  findGroups, storeGroup, deleteGroup, storeMember, deleteMember, findSchedules, storeSchedule, deleteSchedule,
  storeScheduleRule, deleteScheduleRule, checkSchedule, searchDialable, dialerData, normalize,
  // for Voice.js and Voicemail.js
  scheduleOpen: (p) => ({ open: scheduleOpen(p.commsScheduleId, blank(p.atMs) ? nowMs() : Number(p.atMs)) }),
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown call handling operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
