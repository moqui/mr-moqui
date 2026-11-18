// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Rich messages (plans/Comms-Messaging.md §18.3 phase 13, §16 RCS phase 2): the services of
// org.moqui.chat.ChatRichServices. A rich message is a message (CetMessage) with content.rich: a text with suggestion
// chips, a card, or a carousel of cards, each with its suggestions (a reply, a link or a call). Its body and bodyText are
// the fallback, made here from the rich content (the words, each card's title and description, "label: URL" for a link,
// "label: number" for a call and "Reply: Yes, No" for the replies), so every client and bridge that knows nothing of
// cards shows and sends that: the terminal, search, notifications, SMS (its images go as MMS) and the other networks.
// Business RCS sends the cards themselves (crates/moqui-channels, providers/twilio/rcs.rs and providers/sinch/rcs.rs).
//
// The limits are the stricter of Twilio's and Sinch's (Twilio twilio/card and twilio/carousel, Sinch RCS message
// support), so one rich message goes on both: see LIMITS. A card's image is one of the message's files (attachmentSeq,
// 1 to n); the message has no other files. A message answering a suggestion has content.postback {data, toEventSeq};
// note#SuggestionReply is the hook an app's SECA runs on.
//
// The client's moqui.chat.richFallback (WebrootChat.js) makes the same fallback for the composer's preview; the specs
// keep the two in step.

const LIMITS = {
  textMax: 1600, titleMax: 200, descriptionMax: 1600, carouselWordsMax: 160, chipsMax: 10, cardSuggestionsMax: 4,
  carouselSuggestionsMin: 1, carouselSuggestionsMax: 2, cardsMin: 2, cardsMax: 10, suggestionTextMax: 20,
  postbackMax: 200, urlMax: 2048, fallbackMax: 1600, inboundPostbackMax: 1024,
}
const IMAGE_TYPES = /^image\/(png|jpe?g|gif|webp)$/i
const PHONE = /^\+[1-9]\d{3,14}$/
const URL_FORM = /^https?:\/\/[^\s<>"]+$/i
const EVENT = 'moqui.chat.ChatEvent'

const escape = (text) => String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
const words = (value) => String(value ?? '').replace(/\s+/g, ' ').trim()
const paragraph = (value) => String(value ?? '').replace(/\r\n?/g, '\n').split('\n').map((line) => line.replace(/[ \t]+/g, ' ').trim()).join('\n').replace(/\n{3,}/g, '\n\n').trim()
const localize = (text) => ec.l10n.localize(text)
const isObjectJson = (text) => { try { const value = JSON.parse(text); return !!value && typeof value === 'object' && !Array.isArray(value) } catch (err) { return false } }
const parse = (text) => { if (!text) return {}; try { const value = JSON.parse(text); return value && typeof value === 'object' ? value : {} } catch (err) { return {} } }

/** A refusal: its message an error, its reason code in the answer. */
function refuse(reasonCode, message) {
  ec.message.addError(message)
  return { reasonCode }
}

/** A suggestion, checked and cleaned; or {error}. */
function suggestionOf(value) {
  const it = value && typeof value === 'object' ? value : {}
  const type = String(it.type || '')
  const text = words(it.text)
  if (!text) return { error: localize('A button needs its words') }
  if (text.length > LIMITS.suggestionTextMax) return { error: ec.resource.expand(localize('A button is up to ${max} characters: ${text}'), '', { max: LIMITS.suggestionTextMax, text }) }
  if (type === 'reply') {
    const postback = it.postback == null || it.postback === '' ? text : String(it.postback).trim()
    if (postback.length > LIMITS.postbackMax || /[\u0000-\u001f\u007f]/.test(postback)) return { error: localize('A reply button\'s postback is up to 200 characters, with no control characters') }
    return { value: { type, text, postback: postback || text } }
  }
  if (type === 'url') {
    const url = String(it.url || '').trim()
    if (!URL_FORM.test(url) || url.length > LIMITS.urlMax) return { error: ec.resource.expand(localize('A link button opens an http or https address: ${text}'), '', { text }) }
    return { value: { type, text, url } }
  }
  if (type === 'dial') {
    const phone = String(it.phone || '').replace(/[\s().-]/g, '')
    if (!PHONE.test(phone)) return { error: ec.resource.expand(localize('A call button calls a number in international form (+1...): ${text}'), '', { text }) }
    return { value: { type, text, phone } }
  }
  return { error: localize('A button is a reply, a link or a call') }
}

/** A list of suggestions, checked; or {error}. */
function suggestionsOf(list) {
  const out = []
  for (const one of Array.from(list || [])) {
    const checked = suggestionOf(one)
    if (checked.error) return { error: checked.error }
    out.push(checked.value)
  }
  return { value: out }
}

/** The fallback lines of suggestions: links and calls, then the replies on one line. */
function suggestionLines(suggestions) {
  const lines = []
  for (const one of suggestions) {
    if (one.type === 'url') lines.push(one.text + ': ' + one.url)
    else if (one.type === 'dial') lines.push(one.text + ': ' + one.phone)
  }
  const replies = suggestions.filter((one) => one.type === 'reply').map((one) => one.text)
  if (replies.length) lines.push(ec.resource.expand(localize('Reply: ${options}'), '', { options: replies.join(', ') }))
  return lines
}

/** The fallback of a clean rich message: its blocks (each a list of lines, the first a title when bold). */
function fallbackBlocks(rich) {
  const blocks = []
  if (rich.kind === 'text') {
    blocks.push({ lines: [rich.text] })
    const more = suggestionLines(rich.suggestions)
    if (more.length) blocks.push({ lines: more })
    return blocks
  }
  rich.cards.forEach((card, index) => {
    const head = rich.kind === 'carousel' ? (index + 1) + '. ' + (card.title || card.description) : card.title
    const lines = []
    if (head) lines.push({ title: true, text: head })
    if (card.description && !(rich.kind === 'carousel' && !card.title)) lines.push(card.description)
    blocks.push({ lines: lines.concat(suggestionLines(card.suggestions)) })
  })
  return blocks
}

function fallbackText(rich) {
  return fallbackBlocks(rich).map((block) => block.lines.map((line) => (typeof line === 'string' ? line : line.text)).join('\n')).join('\n\n')
}

// one paragraph, its blocks apart by a blank line, so the message's text (send#Message's bodyText, which a bridge sends)
// is fallbackText
function fallbackHtml(rich) {
  return '<p>' + fallbackBlocks(rich).map((block) => block.lines.map((line) => {
    if (typeof line !== 'string') return '<strong>' + escape(line.text) + '</strong>'
    return escape(line).replace(/\n/g, '<br>')
  }).join('<br>')).join('<br><br>') + '</p>'
}

/**
 * normalize#Rich: rich (the message's content.rich as given), the content types of the message's files in order
 * (attachmentTypeList), allowProviderContent. Out: rich (clean: unknown keys dropped), body (safe HTML), fallbackText,
 * or reasonCode with an error.
 */
function normalize(p) {
  const given = p.rich && typeof p.rich === 'object' ? p.rich : null
  if (!given) return refuse('badRich', localize('A rich message needs its cards or buttons'))
  const kind = String(given.kind || '')
  const types = Array.from(p.attachmentTypeList || []).map((type) => String(type || ''))
  const rich = { kind }
  if (kind === 'text') {
    const text = paragraph(given.text)
    if (!text) return refuse('badRich', localize('A message with buttons needs its words'))
    if (text.length > LIMITS.textMax) return refuse('tooLong', ec.resource.expand(localize('The words are up to ${max} characters'), '', { max: LIMITS.textMax }))
    const chips = suggestionsOf(given.suggestions)
    if (chips.error) return refuse('badSuggestion', chips.error)
    if (!chips.value.length || chips.value.length > LIMITS.chipsMax) return refuse('badSuggestion', ec.resource.expand(localize('A message has 1 to ${max} buttons'), '', { max: LIMITS.chipsMax }))
    if (types.length) return refuse('badRich', localize('A message with buttons has no files: make it a card for a picture'))
    Object.assign(rich, { text, suggestions: chips.value })
  } else if (kind === 'card' || kind === 'carousel') {
    const cards = Array.from(given.cards || [])
    const carousel = kind === 'carousel'
    if (!carousel && cards.length !== 1) return refuse('badRich', localize('A card is one card'))
    if (carousel && (cards.length < LIMITS.cardsMin || cards.length > LIMITS.cardsMax)) return refuse('badRich', ec.resource.expand(localize('A carousel has ${min} to ${max} cards'), '', { min: LIMITS.cardsMin, max: LIMITS.cardsMax }))
    const used = new Set()
    let shape = null
    rich.cards = []
    for (const given of cards) {
      const card = given && typeof given === 'object' ? given : {}
      const title = words(card.title)
      const description = paragraph(card.description)
      if (!title && !description) return refuse('badRich', localize('A card needs a title or words'))
      if (title.length > LIMITS.titleMax) return refuse('tooLong', ec.resource.expand(localize('A card\'s title is up to ${max} characters'), '', { max: LIMITS.titleMax }))
      if (description.length > LIMITS.descriptionMax) return refuse('tooLong', ec.resource.expand(localize('A card\'s words are up to ${max} characters'), '', { max: LIMITS.descriptionMax }))
      if (carousel && title.length + description.length > LIMITS.carouselWordsMax) return refuse('tooLong', ec.resource.expand(localize('A carousel card\'s title and words are up to ${max} characters together'), '', { max: LIMITS.carouselWordsMax }))
      const buttons = suggestionsOf(card.suggestions)
      if (buttons.error) return refuse('badSuggestion', buttons.error)
      const count = buttons.value.length
      if (!carousel && count > LIMITS.cardSuggestionsMax) return refuse('badSuggestion', ec.resource.expand(localize('A card has up to ${max} buttons'), '', { max: LIMITS.cardSuggestionsMax }))
      if (carousel) {
        if (count < LIMITS.carouselSuggestionsMin || count > LIMITS.carouselSuggestionsMax) return refuse('badSuggestion', localize('A carousel card has 1 or 2 buttons'))
        // Twilio: the same kinds of buttons in the same order on every card
        const kinds = buttons.value.map((one) => one.type).join(',')
        if (shape != null && kinds !== shape) return refuse('badSuggestion', localize('Every card of a carousel has the same kinds of buttons, in the same order'))
        shape = kinds
      }
      let seq = card.attachmentSeq == null || card.attachmentSeq === '' ? null : Number(card.attachmentSeq)
      if (seq != null) {
        if (!Number.isInteger(seq) || seq < 1 || seq > types.length || used.has(seq)) return refuse('badImage', localize('A card\'s picture is one of the message\'s files, each card its own'))
        if (!IMAGE_TYPES.test(types[seq - 1])) return refuse('badImage', localize('A card\'s picture is a PNG, JPEG, GIF or WebP image'))
        used.add(seq)
      } else if (carousel) return refuse('badImage', localize('Every card of a carousel has a picture'))
      const clean = { suggestions: buttons.value }
      if (title) clean.title = title
      if (description) clean.description = description
      if (seq != null) clean.attachmentSeq = seq
      rich.cards.push(clean)
    }
    if (used.size !== types.length) return refuse('badImage', localize('A card\'s message has only its cards\' pictures as files'))
  } else return refuse('badRich', localize('A rich message is a text with buttons, a card or a carousel'))
  const own = given.providerContent && given.providerContent.twilio
  if (own) {
    if (!p.allowProviderContent) return refuse('notAllowed', localize('A provider template is named by an app only'))
    const sid = String(own.contentSid || '')
    if (!/^HX[0-9a-f]{32}$/i.test(sid)) return refuse('badRich', localize('A Twilio template is named by its HX... id'))
    let variables = own.contentVariables == null ? null : own.contentVariables
    if (variables != null && typeof variables === 'object') variables = JSON.stringify(variables)
    if (variables != null && (String(variables).length > 4000 || !isObjectJson(String(variables)))) return refuse('badRich', localize('A Twilio template\'s variables are a JSON object'))
    rich.providerContent = { twilio: variables != null ? { contentSid: sid, contentVariables: String(variables) } : { contentSid: sid } }
  }
  const text = fallbackText(rich)
  if (text.length > LIMITS.fallbackMax) return refuse('tooLong', ec.resource.expand(localize('A card is too long to go as a text: its text form is over ${max} characters'), '', { max: LIMITS.fallbackMax }))
  return { rich, body: fallbackHtml(rich), fallbackText: text }
}

/** The room's active mappings that send out (a bridge's). */
function outboundBridged(chatRoomId) {
  return Array.from(ec.entity.find('moqui.chat.ChatRoomBridge').condition('chatRoomId', chatRoomId).condition('thruDate', null).disableAuthz().list())
    .some((row) => row.directionEnumId !== 'CrbIn')
}

/**
 * send#Rich: a rich message from the current user, or from an app (authorization disabled, which alone may name a
 * provider's template): normalize#Rich, then send#Message with the fallback as its body and the cards' pictures as its
 * files, then content.rich set and pushed (push#Update {rich}), in one transaction. Refused in an encrypted room.
 */
function send(p) {
  const out = { eventSeq: null, event: null, duplicate: false, reasonCode: null }
  if (ec.entity.find('moqui.chat.ChatRoom').condition('chatRoomId', p.chatRoomId).condition('encrypted', 'Y').count()) {
    return Object.assign(out, refuse('encryptedRoom', localize('An encrypted room has no cards: the server cannot read them to make their text')))
  }
  const ids = Array.from(p.chatAttachmentIdList || []).map((id) => String(id || '')).filter((id) => id)
  const types = ids.map((id) => {
    const row = ec.entity.find('moqui.chat.ChatAttachment').condition('chatAttachmentId', id).condition('chatRoomId', p.chatRoomId).disableAuthz().one()
    return row ? String(row.contentType || '') : ''
  })
  const checked = normalize({ rich: p.rich, attachmentTypeList: types, allowProviderContent: ec.artifactExecution.authzDisabled() })
  if (ec.message.hasError()) return Object.assign(out, { reasonCode: checked.reasonCode || 'badRich' })
  const sent = ec.service.sync().name('org.moqui.chat.ChatServices.send#Message').parameters({ chatRoomId: p.chatRoomId, body: checked.body,
    chatAttachmentIdList: ids, threadRootEventSeq: p.threadRootEventSeq ?? null, clientRef: p.clientRef || null, roomUrl: p.roomUrl || null,
    senderUserId: p.senderUserId || null }).call()
  if (ec.message.hasError()) return Object.assign(out, { reasonCode: (sent && sent.reasonCode) || null })
  out.eventSeq = sent.eventSeq
  out.event = sent.event
  if (sent.duplicate) return Object.assign(out, { duplicate: true })
  const row = ec.entity.find(EVENT).condition('chatRoomId', p.chatRoomId).condition('eventSeq', sent.eventSeq).one()
  const content = parse(row.contentJson)
  content.rich = checked.rich
  row.set('contentJson', JSON.stringify(content))
  row.update()
  out.event = Object.assign({}, sent.event, { rich: checked.rich })
  ec.service.sync().name('org.moqui.chat.ChatServices.push#Update').parameters({ chatRoomId: p.chatRoomId, eventSeq: sent.eventSeq,
    fields: { rich: checked.rich } }).call()
  return out
}

/**
 * reply#Suggestion: the current user taps reply index (of card cardIndex, else of the message's chips) of message
 * eventSeq: a message with its words, then note#SuggestionReply. Refused on one's own message, in a room with an outbound
 * bridge (its buttons are the other person's to tap, on their phone), and for a link or a call (the client opens those).
 */
function reply(p) {
  const out = { eventSeq: null, event: null, reasonCode: null }
  const me = ec.user.userId
  const access = ec.service.sync().name('org.moqui.chat.ChatServices.get#RoomAccess').parameters({ chatRoomId: p.chatRoomId }).call()
  if (ec.message.hasError()) return out
  if (!access || !access.canPost) return Object.assign(out, refuse('notMember', ec.resource.expand(localize('Chat room ${id} not found'), '', { id: p.chatRoomId })))
  const row = ec.entity.find(EVENT).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).one()
  const rich = row && row.eventTypeEnumId === 'CetMessage' && !row.redactedDate ? parse(row.contentJson).rich : null
  if (!rich) return Object.assign(out, refuse('notFound', ec.resource.expand(localize('Message ${seq} has no buttons'), '', { seq: p.eventSeq })))
  if (row.senderUserId === me) return Object.assign(out, refuse('ownMessage', localize('Your own message\'s buttons are for the others to tap')))
  if (outboundBridged(p.chatRoomId)) return Object.assign(out, refuse('bridgedRoom', localize('This room\'s buttons are for the other person to tap, on their phone')))
  const list = p.cardIndex == null || p.cardIndex === '' ? rich.suggestions : ((rich.cards || [])[Number(p.cardIndex)] || {}).suggestions
  const suggestion = Array.from(list || [])[Number(p.index)]
  if (!suggestion || suggestion.type !== 'reply') return Object.assign(out, refuse('notReply', localize('That button is not a reply')))
  const sent = ec.service.sync().name('org.moqui.chat.ChatServices.send#Message').parameters({ chatRoomId: p.chatRoomId,
    body: '<p>' + escape(suggestion.text) + '</p>', threadRootEventSeq: row.threadRootEventSeq ?? null, clientRef: p.clientRef || null,
    roomUrl: p.roomUrl || null }).call()
  if (ec.message.hasError()) return Object.assign(out, { reasonCode: (sent && sent.reasonCode) || null })
  out.eventSeq = sent.eventSeq
  out.event = sent.event
  if (sent.duplicate) return out
  const noted = ec.service.sync().name('org.moqui.chat.ChatRichServices.note#SuggestionReply').parameters({ chatRoomId: p.chatRoomId,
    eventSeq: sent.eventSeq, postback: suggestion.postback || suggestion.text, toEventSeq: Number(p.eventSeq) }).call()
  if (noted && noted.postback) out.event = Object.assign({}, sent.event, { postback: noted.postback })
  return out
}

/**
 * note#SuggestionReply: message eventSeq answers a suggestion: content.postback {data (cut to 1024), toEventSeq (the
 * suggesting message, when known)} kept and pushed. An app that sent the buttons runs on a SECA of this service.
 */
function noteReply(p) {
  const row = ec.entity.find(EVENT).condition('chatRoomId', p.chatRoomId).condition('eventSeq', p.eventSeq).forUpdate(true).disableAuthz().one()
  if (!row || row.eventTypeEnumId !== 'CetMessage' || row.redactedDate) return { postback: null }
  const data = String(p.postback ?? '').trim().slice(0, LIMITS.inboundPostbackMax)
  if (!data) return { postback: null }
  const content = parse(row.contentJson)
  content.postback = p.toEventSeq != null && p.toEventSeq !== '' ? { data, toEventSeq: Number(p.toEventSeq) } : { data }
  row.set('contentJson', JSON.stringify(content))
  row.update()
  ec.service.sync().name('org.moqui.chat.ChatServices.push#Update').parameters({ chatRoomId: p.chatRoomId, eventSeq: Number(p.eventSeq),
    fields: { postback: content.postback } }).disableAuthz().call()
  return { postback: content.postback }
}

const operations = { normalize, send, reply, noteReply }

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown rich message operation ' + op)
  return operations[op](parameters) || {}
}

;({ execute })
