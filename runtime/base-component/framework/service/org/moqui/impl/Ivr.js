// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// The IVR and auto attendant (plans/Comms-Framework.md §5.3, §5.4, V5): the services of org.moqui.impl.IvrServices. A
// number routed to a menu (PhoneNumber PnrIvr) or to a queue (PnrQueue) is answered by the SIP engine, which runs the
// menu's keys itself (moqui-host src/sip/ivr.rs: the prompt, a key or keys ended by #, the timeout and its retries) and
// asks route#IvrCall where a choice goes. A target (CallTarget) is a user (their call handling rules), a ring group, a
// queue (CallQueues.js), another menu, a voicemail box, an outside number (set by a voice admin, so the deployment's fraud
// policy alone applies), this menu again or the end; the user, group, box and number targets enter V3's step plan
// (CallHandling.js routeTo), so the engine rings, moves on and goes to voicemail as for any inbound call. A menu outside
// its schedule goes to its closed target. A call goes through at most MAX_MENUS menus, so a loop of menus ends.
//
// Menus, options and their prompts are kept by voice admins (VoiceAdmin). A prompt is audio the admin uploads (WAV or
// Ogg Opus, made Ogg Opus by convert#GreetingAudio as a voicemail greeting is); without one the engine plays a tone
// pattern. Moqui ships no recorded speech and calls no text to speech.

const CALLS = 'component://framework/service/org/moqui/impl/Calls.js'
const CALL_HANDLING = 'component://framework/service/org/moqui/impl/CallHandling.js'
const QUEUES = 'component://framework/service/org/moqui/impl/CallQueues.js'
const VOICE = 'component://framework/service/org/moqui/impl/Voice.js'
const SESSION = 'moqui.call.CallSession'
const LEG = 'moqui.call.CallLeg'
const MENU = 'moqui.call.IvrMenu'
const OPTION = 'moqui.call.IvrOption'
const STATE = 'moqui.call.CallIvrState'
const NUMBER = 'moqui.call.PhoneNumber'
const QUEUE = 'moqui.call.CallQueue'
const GROUP = 'moqui.call.RingGroup'
const BOX = 'moqui.call.VoicemailBox'
const ACCOUNT = 'moqui.security.UserAccount'

const TARGETS = ['CtgUser', 'CtgGroup', 'CtgQueue', 'CtgMenu', 'CtgVoicemail', 'CtgNumber', 'CtgRepeat', 'CtgEnd']
/** The menus one call goes through before it ends: a loop of menus cannot hold a call. */
const MAX_MENUS = 20
/** Where uploaded prompts and hold music are kept (moqui-host src/sip/audio.rs PROMPT_ROOT). */
const PROMPT_ROOT = 'dbresource://moqui/call/prompt'
const MAX_PROMPT_BYTES = 10 * 1024 * 1024
const DIGITS = /^[0-9*#]{1,10}$/

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
const clamp = (value, low, high) => Math.min(Math.max(value, low), high)
const intOr = (value, fallback) => (blank(value) || Number.isNaN(Number(value)) ? fallback : Math.round(Number(value)))

function calls(op, parameters) {
  return ec.resource.script(CALLS).execute(op, parameters) || {}
}
function handling(op, parameters) {
  return ec.resource.script(CALL_HANDLING).execute(op, parameters) || {}
}
function queues(op, parameters) {
  return ec.resource.script(QUEUES).execute(op, parameters) || {}
}
function voice(op, parameters) {
  return ec.resource.script(VOICE).execute(op, parameters) || {}
}

/** A refusal: an error for the user and a reason code for the caller. */
/**
 * Anyone, or the edge link's delegate user for a call whose media is on this node's edge node (Calls.js edgeAllowed;
 * plans/Edge-Node.md §5.4 "Voice (SIP)", E8): its SIP engine routes only its own calls.
 */
function edgeAllowed(p) {
  return ec.user.username !== '_edge_' || !!(ec.resource.script(CALLS).execute('edgeAllowed', p) || {}).allowed
}

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

// ---------- targets ----------

/** What a target is called, for the screens. */
function targetName(type, id) {
  switch (type) {
    case 'CtgUser': return userName(id)
    case 'CtgGroup': return one(GROUP, { ringGroupId: id })?.name || id
    case 'CtgQueue': return one(QUEUE, { callQueueId: id })?.name || id
    case 'CtgMenu': return one(MENU, { ivrMenuId: id })?.name || id
    case 'CtgVoicemail': {
      const box = id ? one(BOX, { voicemailBoxId: id }) : null
      if (!box) return id
      return box.ringGroupId ? (one(GROUP, { ringGroupId: box.ringGroupId })?.name || box.ringGroupId) : userName(box.userId)
    }
    case 'CtgRepeat': return ec.l10n.localize('This menu again')
    case 'CtgEnd': return ec.l10n.localize('End the call')
    default: return id || null
  }
}

/** A target an admin sets: null when it is good, else the refusal. A number is checked against the deployment's policy. */
function checkTarget(type, id) {
  if (blank(type)) return null
  if (!TARGETS.includes(type)) return refuse('badTarget', 'Not a target: ' + type)
  if (['CtgRepeat', 'CtgEnd'].includes(type)) return null
  if (blank(id)) return refuse('badTarget', 'Choose where the call goes')
  switch (type) {
    case 'CtgUser': {
      const account = one(ACCOUNT, { userId: id })
      return account && account.disabled !== 'Y' ? null : refuse('notFound', 'User not found')
    }
    case 'CtgGroup': return one(GROUP, { ringGroupId: id }) ? null : refuse('notFound', 'Ring group not found')
    case 'CtgQueue': return one(QUEUE, { callQueueId: id }) ? null : refuse('notFound', 'Queue not found')
    case 'CtgMenu': return one(MENU, { ivrMenuId: id }) ? null : refuse('notFound', 'Menu not found')
    case 'CtgVoicemail': return one(BOX, { voicemailBoxId: id }) ? null : refuse('notFound', 'Voicemail box not found')
    case 'CtgNumber': {
      const check = voice('forwardCheck', { phoneNumber: id, byAdmin: true })
      if (!check.number) return refuse('badNumber', 'That is not a phone number')
      if (!check.allowed) return refuse(check.reasonCode || 'notAllowed', 'The deployment does not allow calls to that number (' + (check.reasonCode || 'notAllowed') + ')')
      return null
    }
    default: return null
  }
}

/**
 * Where a call goes now (an IVR choice, a queue's fallback, a menu's closed or timeout target, a transfer's), as the SIP
 * engine takes it: mode menu (the menu's keys, from menuAnswer), queue (CallQueues enter), ring (ringSeconds, stepToken),
 * voicemail (stepToken: the engine starts it), or end. ctx: {callSessionId, callLegId, fromNumber, displayName, atMs,
 * currentMenuId}.
 */
function routeTarget(type, id, ctx) {
  const end = (why) => {
    if (why) ec.logger.info(`Call ${ctx.callSessionId}: ${why}; it ends`)
    return { mode: 'end' }
  }
  switch (type) {
    case 'CtgMenu': return menuAnswer(id, ctx)
    case 'CtgRepeat': return ctx.currentMenuId ? menuAnswer(ctx.currentMenuId, ctx) : end('no menu to repeat')
    case 'CtgQueue': return queues('enter', { callSessionId: ctx.callSessionId, callLegId: ctx.callLegId, callQueueId: id,
      fromNumber: ctx.fromNumber, displayName: ctx.displayName, atMs: ctx.atMs, depth: (ctx.depth || 0) + 1 })
    case 'CtgUser':
    case 'CtgGroup':
    case 'CtgVoicemail':
    case 'CtgNumber': {
      const out = handling('routeTo', { callSessionId: ctx.callSessionId, userId: type === 'CtgUser' ? id : null,
        ringGroupId: type === 'CtgGroup' ? id : null, voicemailBoxId: type === 'CtgVoicemail' ? id : null,
        phoneNumber: type === 'CtgNumber' ? id : null, fromNumber: ctx.fromNumber, displayName: ctx.displayName, atMs: ctx.atMs })
      if (out.mode === 'ring') return { mode: 'ring', ringSeconds: out.ringSeconds, stepToken: out.stepToken }
      if (out.mode === 'voicemail') return { mode: 'voicemail', stepToken: out.stepToken }
      return end(`${type} ${id} takes no call now (${out.reasonCode || 'noRoute'})`)
    }
    default: return end(type ? `target ${type} not known` : 'no target')
  }
}

/** The call's IVR state, made when it has none. */
function stateOf(callSessionId) {
  let state = find(STATE).condition('callSessionId', callSessionId).forUpdate(true).one()
  if (!state) {
    state = ec.entity.makeValue(STATE)
    state.setFields({ callSessionId, menuCount: 0, enteredDate: ec.user.nowTimestamp, lastDate: ec.user.nowTimestamp }, true, null, null)
    state.create()
  }
  return state
}

/**
 * A menu for the engine: its prompt, timeout, retries, how many keys, and the keys its options take (hasInvalid: a key
 * no option has goes to the invalid target, else it counts as a retry). Outside the menu's schedule its closed target
 * runs instead; past MAX_MENUS the call ends.
 */
function menuAnswer(ivrMenuId, ctx) {
  const menu = ivrMenuId ? one(MENU, { ivrMenuId }) : null
  if (!menu) {
    ec.logger.warn(`Call ${ctx.callSessionId}: IVR menu ${ivrMenuId} not found; the call ends`)
    return { mode: 'end' }
  }
  const state = stateOf(ctx.callSessionId)
  const count = (Number(state.menuCount) || 0) + 1
  if (count > MAX_MENUS) {
    ec.logger.warn(`Call ${ctx.callSessionId}: more than ${MAX_MENUS} menus (a loop of menus?); the call ends`)
    return { mode: 'end' }
  }
  state.setFields({ ivrMenuId: menu.ivrMenuId, menuCount: count, lastDate: ec.user.nowTimestamp }, true, null, null)
  state.update()
  if (menu.commsScheduleId) {
    const open = handling('scheduleOpen', { commsScheduleId: menu.commsScheduleId, atMs: ctx.atMs }).open
    if (open === false) {
      note(state, `${menu.name || menu.ivrMenuId}: closed`)
      if (blank(menu.closedTypeEnumId)) return { mode: 'end' }
      return routeTarget(menu.closedTypeEnumId, menu.closedId, Object.assign({}, ctx, { currentMenuId: null }))
    }
  }
  const options = rows(OPTION, { ivrMenuId: menu.ivrMenuId }).filter((o) => o.targetTypeEnumId)
  return {
    mode: 'menu', ivrMenuId: menu.ivrMenuId, promptLocation: menu.promptLocation || null,
    timeoutSeconds: clamp(intOr(menu.timeoutSeconds, 5), 1, 60), maxRetries: clamp(intOr(menu.maxRetries, 2), 0, 5),
    maxDigits: clamp(intOr(menu.maxDigits, 1), 1, 10), optionList: options.map((o) => ({ digits: o.digits })),
    hasInvalid: !blank(menu.invalidTypeEnumId),
  }
}

/** Adds a step to the call's path (history and support read it). */
function note(state, text) {
  const path = String(state.pathText || '')
  state.set('pathText', (path ? path + ' > ' : '') + text)
  if (String(state.pathText).length > 1000) state.set('pathText', String(state.pathText).slice(-1000))
  state.update()
}

/**
 * Internal (Voice.js receive#PhoneCall): a call to a number routed to an IVR menu (kind menu) or a queue (kind queue). The
 * call starts with nobody rung (Calls.js startPhone), the engine answers it, and the answer is the first thing it does:
 * the menu, the queue, or where a closed menu sends it.
 */
function inbound(p) {
  const started = calls('startPhone', { userId: null, direction: 'inbound', phoneNumber: p.fromNumber || p.rawFrom,
    displayName: p.displayName, mediaNodeId: p.instanceId })
  if (!started.callSessionId) return { reasonCode: 'unavailable' }
  const ctx = { callSessionId: started.callSessionId, callLegId: started.phoneLegId, fromNumber: p.fromNumber || null,
    displayName: p.displayName || null, atMs: blank(p.atMs) ? null : Number(p.atMs) }
  const answer = p.kind === 'queue' ? routeTarget('CtgQueue', p.routeId, ctx) : routeTarget('CtgMenu', p.routeId, ctx)
  return { callSessionId: started.callSessionId, callLegId: started.phoneLegId, ivr: answer }
}

/**
 * Internal (the SIP engine): the caller chose in a menu: kind option (digits, one of its options), invalid (a key no
 * option has) or timeout (no key after the retries). Answers where the call goes now, as routeTarget.
 */
function route(p) {
  const session = p.callSessionId ? one(SESSION, { callSessionId: p.callSessionId }) : null
  if (!session || session.statusId === 'CssEnded') return { mode: 'end', reasonCode: 'callEnded' }
  if (!edgeAllowed({ callSessionId: session.callSessionId, callLegId: p.callLegId })) return { mode: 'end', reasonCode: 'notAllowed' }
  const menu = p.ivrMenuId ? one(MENU, { ivrMenuId: p.ivrMenuId }) : null
  if (!menu) return { mode: 'end', reasonCode: 'notFound' }
  const leg = p.callLegId ? one(LEG, { callLegId: p.callLegId }) : null
  const ctx = { callSessionId: session.callSessionId, callLegId: p.callLegId, fromNumber: leg?.phoneNumber || null,
    displayName: null, atMs: blank(p.atMs) ? null : Number(p.atMs), currentMenuId: menu.ivrMenuId }
  const state = stateOf(session.callSessionId)
  const label = menu.name || menu.ivrMenuId
  if (p.kind === 'option') {
    const option = one(OPTION, { ivrMenuId: menu.ivrMenuId, digits: String(p.digits || '') })
    if (option && option.targetTypeEnumId) {
      note(state, `${label}: ${option.digits}`)
      const out = routeTarget(option.targetTypeEnumId, option.targetId, ctx)
      if (option.promptLocation && out.mode !== 'end') out.sayLocation = option.promptLocation
      return out
    }
  }
  if (p.kind === 'timeout') {
    note(state, `${label}: no key`)
    return blank(menu.timeoutTypeEnumId) ? { mode: 'end' } : routeTarget(menu.timeoutTypeEnumId, menu.timeoutId, ctx)
  }
  note(state, `${label}: ${p.digits || '?'} (no such option)`)
  return blank(menu.invalidTypeEnumId) ? menuAnswer(menu.ivrMenuId, ctx) : routeTarget(menu.invalidTypeEnumId, menu.invalidId, ctx)
}

// ---------- administration (VoiceAdmin) ----------

function menuView(menu) {
  const options = rows(OPTION, { ivrMenuId: menu.ivrMenuId }).sort((a, b) => String(a.digits).localeCompare(String(b.digits)))
  return {
    ivrMenuId: menu.ivrMenuId, name: menu.name || menu.ivrMenuId, description: menu.description || null,
    promptLocation: menu.promptLocation || null, hasPrompt: !blank(menu.promptLocation),
    timeoutSeconds: intOr(menu.timeoutSeconds, 5), maxRetries: intOr(menu.maxRetries, 2), maxDigits: intOr(menu.maxDigits, 1),
    invalidTypeEnumId: menu.invalidTypeEnumId || null, invalidId: menu.invalidId || null,
    invalidName: menu.invalidTypeEnumId ? targetName(menu.invalidTypeEnumId, menu.invalidId) : null,
    timeoutTypeEnumId: menu.timeoutTypeEnumId || null, timeoutId: menu.timeoutId || null,
    timeoutName: menu.timeoutTypeEnumId ? targetName(menu.timeoutTypeEnumId, menu.timeoutId) : null,
    commsScheduleId: menu.commsScheduleId || null,
    scheduleName: menu.commsScheduleId ? one('moqui.call.CommsSchedule', { commsScheduleId: menu.commsScheduleId })?.name || null : null,
    closedTypeEnumId: menu.closedTypeEnumId || null, closedId: menu.closedId || null,
    closedName: menu.closedTypeEnumId ? targetName(menu.closedTypeEnumId, menu.closedId) : null,
    optionList: options.map((o) => ({ ivrMenuId: o.ivrMenuId, digits: o.digits, targetTypeEnumId: o.targetTypeEnumId,
      targetId: o.targetId || null, targetName: targetName(o.targetTypeEnumId, o.targetId), description: o.description || null,
      hasPrompt: !blank(o.promptLocation) })),
    numberList: rows(NUMBER, { routeTypeEnumId: 'PnrIvr', routeId: menu.ivrMenuId }).map((n) => n.phoneNumber),
  }
}

function findMenus() {
  const denied = notAdmin()
  if (denied) return denied
  return { menuList: Array.from(find(MENU).orderBy('name').list()).map(menuView) }
}

function storeMenu(p) {
  const denied = notAdmin()
  if (denied) return denied
  let menu = blank(p.ivrMenuId) ? null : one(MENU, { ivrMenuId: p.ivrMenuId })
  if (!blank(p.ivrMenuId) && !menu) return refuse('notFound', 'Menu not found')
  if (!menu && blank(p.name)) return refuse('badMenu', 'Name the menu')
  for (const [type, id] of [[p.invalidTypeEnumId, p.invalidId], [p.timeoutTypeEnumId, p.timeoutId], [p.closedTypeEnumId, p.closedId]]) {
    const bad = checkTarget(type, id)
    if (bad) return bad
  }
  if (!blank(p.commsScheduleId) && !one('moqui.call.CommsSchedule', { commsScheduleId: p.commsScheduleId })) return refuse('notFound', 'Schedule not found')
  const timeout = intOr(p.timeoutSeconds, menu ? intOr(menu.timeoutSeconds, 5) : 5)
  const retries = intOr(p.maxRetries, menu ? intOr(menu.maxRetries, 2) : 2)
  const digits = intOr(p.maxDigits, menu ? intOr(menu.maxDigits, 1) : 1)
  if (timeout < 1 || timeout > 60 || retries < 0 || retries > 5 || digits < 1 || digits > 10) {
    return refuse('badMenu', 'A menu waits 1 to 60 seconds, plays again 0 to 5 times, and takes 1 to 10 keys')
  }
  const fields = { timeoutSeconds: timeout, maxRetries: retries, maxDigits: digits }
  for (const key of ['name', 'description', 'invalidTypeEnumId', 'invalidId', 'timeoutTypeEnumId', 'timeoutId', 'commsScheduleId',
    'closedTypeEnumId', 'closedId']) {
    if (p[key] !== undefined && p[key] !== null) fields[key] = blank(p[key]) ? null : String(p[key]).trim()
  }
  for (const [type, id] of [['invalidTypeEnumId', 'invalidId'], ['timeoutTypeEnumId', 'timeoutId'], ['closedTypeEnumId', 'closedId']]) {
    if (fields[type] && ['CtgRepeat', 'CtgEnd'].includes(fields[type])) fields[id] = null
  }
  if (!menu) {
    menu = ec.entity.makeValue(MENU)
    menu.setSequencedIdPrimary()
    menu.setFields(fields, true, null, null)
    menu.create()
  } else {
    menu.setFields(fields, true, null, null)
    menu.update()
  }
  return { ivrMenuId: menu.ivrMenuId }
}

function deleteMenu(p) {
  const denied = notAdmin()
  if (denied) return denied
  const menu = one(MENU, { ivrMenuId: p.ivrMenuId })
  if (!menu) return refuse('notFound', 'Menu not found')
  const used = rows(NUMBER, { routeTypeEnumId: 'PnrIvr', routeId: menu.ivrMenuId }).length +
    find(OPTION).condition('targetTypeEnumId', 'CtgMenu').condition('targetId', menu.ivrMenuId).count() +
    find(QUEUE).condition('fallbackTypeEnumId', 'CtgMenu').condition('fallbackId', menu.ivrMenuId).count()
  if (used) return refuse('inUse', 'A number, another menu or a queue sends calls to this menu: change that first')
  for (const option of rows(OPTION, { ivrMenuId: menu.ivrMenuId })) {
    deleteLocation(option.promptLocation)
    option.delete()
  }
  deleteLocation(menu.promptLocation)
  menu.delete()
  return {}
}

function storeOption(p) {
  const denied = notAdmin()
  if (denied) return denied
  const menu = one(MENU, { ivrMenuId: p.ivrMenuId })
  if (!menu) return refuse('notFound', 'Menu not found')
  const digits = String(p.digits || '').trim()
  if (!DIGITS.test(digits)) return refuse('badDigits', 'An option is a key, 0 to 9, * or #, or up to 10 of them')
  if (digits.length > intOr(menu.maxDigits, 1)) return refuse('badDigits', 'This menu takes ' + intOr(menu.maxDigits, 1) + ' key(s): raise its keys first')
  if (blank(p.targetTypeEnumId)) return refuse('badTarget', 'Choose where the option goes')
  if (p.targetTypeEnumId === 'CtgMenu' && p.targetId === menu.ivrMenuId) return refuse('badTarget', 'Use "This menu again" for the same menu')
  const bad = checkTarget(p.targetTypeEnumId, p.targetId)
  if (bad) return bad
  const fields = { targetTypeEnumId: p.targetTypeEnumId, targetId: ['CtgRepeat', 'CtgEnd'].includes(p.targetTypeEnumId) ? null : String(p.targetId).trim(),
    description: blank(p.description) ? null : String(p.description).trim() }
  const row = one(OPTION, { ivrMenuId: menu.ivrMenuId, digits })
  if (row) {
    row.setFields(fields, true, null, null)
    row.update()
  } else {
    const made = ec.entity.makeValue(OPTION)
    made.setFields(Object.assign({ ivrMenuId: menu.ivrMenuId, digits }, fields), true, null, null)
    made.create()
  }
  return {}
}

function deleteOption(p) {
  const denied = notAdmin()
  if (denied) return denied
  const option = one(OPTION, { ivrMenuId: p.ivrMenuId, digits: String(p.digits || '') })
  if (!option) return refuse('notFound', 'Option not found')
  deleteLocation(option.promptLocation)
  option.delete()
  return {}
}

// ---------- prompts (uploaded audio) ----------

function audioKind(head) {
  if (head.startsWith('RIFF') && head.slice(8, 12) === 'WAVE') return 'wav'
  if (head.startsWith('OggS') && head.indexOf('OpusHead') > 0 && head.indexOf('OpusHead') < 80) return 'ogg'
  return null
}

function deleteLocation(location) {
  if (blank(location) || !String(location).startsWith(PROMPT_ROOT + '/')) return
  try {
    const ref = ec.resource.getLocationReference(location)
    if (ref.getExists()) ref.delete()
  } catch (err) {
    ec.logger.warn(`Prompt ${location} not removed: ${err && err.message ? err.message : err}`)
  }
}

/**
 * Internal (the admin's prompt and hold music uploads, here and in CallQueues.js): an uploaded WAV (16-bit PCM) or Ogg
 * Opus file stored under PROMPT_ROOT/folder as Ogg Opus (convert#GreetingAudio, in a build with phone calls; else kept as
 * uploaded). Answers location, or a refusal (badAudio).
 */
function storePromptFile(p) {
  const file = p.file
  if (!file) return refuse('noFile', 'No file uploaded')
  const size = Number(file.getSize() || 0)
  if (!size || size > MAX_PROMPT_BYTES) return refuse('badAudio', 'A prompt is a WAV or Ogg Opus file of at most 10 MB')
  const kind = audioKind(String(file.getString('ISO-8859-1') || '').slice(0, 96))
  if (!kind) return refuse('badAudio', 'A prompt is a WAV or Ogg Opus file of at most 10 MB')
  const folder = String(p.folder || 'misc').replace(/[^A-Za-z0-9_/-]/g, '_')
  const stamp = String(nowMs())
  const upload = `${PROMPT_ROOT}/${folder}/upload-${stamp}.${kind}`
  file.write(upload)
  const target = `${PROMPT_ROOT}/${folder}/prompt-${stamp}.ogg`
  let converted = null
  ec.message.pushErrors()
  try {
    const out = ec.service.sync().name('org.moqui.impl.VoicemailServices.convert#GreetingAudio')
      .parameters({ sourceLocation: upload, targetLocation: target }).requireNewTransaction(true).disableAuthz().call() || {}
    if (!ec.message.hasError()) converted = out
  } catch (err) {
    ec.logger.info(`Prompt ${upload} kept as uploaded: ${err && err.message ? err.message : err}`)
  } finally {
    ec.message.clearErrors()
    ec.message.popErrors()
  }
  if (converted && converted.reasonCode) {
    deleteLocation(upload)
    return refuse('badAudio', 'That audio could not be read: ' + (converted.errorText || converted.reasonCode))
  }
  if (converted) deleteLocation(upload)
  return { location: converted ? target : upload, durationMs: converted ? converted.durationMs || null : null }
}

/** A menu's prompt (digits empty) or an option's: uploaded, or removed (file empty and remove true). */
function storePrompt(p) {
  const denied = notAdmin()
  if (denied) return denied
  const menu = one(MENU, { ivrMenuId: p.ivrMenuId })
  if (!menu) return refuse('notFound', 'Menu not found')
  const row = blank(p.digits) ? menu : one(OPTION, { ivrMenuId: menu.ivrMenuId, digits: String(p.digits) })
  if (!row) return refuse('notFound', 'Option not found')
  if (p.remove === true || p.remove === 'true') {
    deleteLocation(row.promptLocation)
    row.set('promptLocation', null)
    row.update()
    return {}
  }
  const stored = storePromptFile({ file: p.file, folder: `menu/${menu.ivrMenuId}` })
  if (stored.reasonCode) return stored
  deleteLocation(row.promptLocation)
  row.set('promptLocation', stored.location)
  row.update()
  return { promptLocation: stored.location }
}

/** A prompt's audio for a voice admin to play back: location and contentType. */
function promptAudio(p) {
  const denied = notAdmin()
  if (denied) return denied
  const location = String(p.location || '')
  if (!location.startsWith(PROMPT_ROOT + '/') || location.includes('..')) return refuse('notFound', 'Prompt not found')
  return { location, contentType: location.endsWith('.wav') ? 'audio/wav' : 'audio/ogg' }
}

/** The targets the screens offer, by type: users are searched; the rest are listed. */
function targetOptions() {
  const denied = notAdmin()
  if (denied) return denied
  const L = (text) => ec.l10n.localize(text)
  const boxes = Array.from(find(BOX).orderBy('voicemailBoxId').limit(500).list())
  return {
    typeList: Array.from(find('moqui.basic.Enumeration').condition('enumTypeId', 'CallTarget').orderBy('sequenceNum').list())
      .map((e) => ({ key: e.enumId, text: L(e.description) })),
    groupList: Array.from(find(GROUP).orderBy('name').list()).map((g) => ({ key: g.ringGroupId, text: g.name })),
    queueList: Array.from(find(QUEUE).orderBy('name').list()).map((q) => ({ key: q.callQueueId, text: q.name })),
    menuList: Array.from(find(MENU).orderBy('name').list()).map((m) => ({ key: m.ivrMenuId, text: m.name || m.ivrMenuId })),
    boxList: boxes.map((b) => ({ key: b.voicemailBoxId, text: targetName('CtgVoicemail', b.voicemailBoxId) })),
  }
}

const operations = {
  inbound, route, findMenus, storeMenu, deleteMenu, storeOption, deleteOption, storePrompt, promptAudio, targetOptions,
  // for CallQueues.js and CallTransfers.js
  routeTarget: (p) => routeTarget(p.targetTypeEnumId, p.targetId, p), storePromptFile, deleteLocation: (p) => { deleteLocation(p.location); return {} },
  checkTarget: (p) => checkTarget(p.targetTypeEnumId, p.targetId) || {}, targetName: (p) => ({ name: targetName(p.targetTypeEnumId, p.targetId) }),
}

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown IVR operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
