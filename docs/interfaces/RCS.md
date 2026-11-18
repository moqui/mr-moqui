# Business RCS

**Not yet tested with the provider.** The two adapters below are built from Twilio's and Sinch's documentation and
checked against fake servers only. Have each tested live before you rely on it. The facts this guide could not
confirm in the providers' documentation are listed at the end.

The server sends and receives RCS as a verified **business** sender, through the SMS provider it already uses. This is
Google's RCS Business Messaging (RBM), resold by Twilio and Sinch.
- RCS here comes from the brand's agent: a name, a logo and a check mark, not a phone number.
- A phone with no RCS gets the message as SMS or MMS from one of our numbers.
- Business RCS is never end-to-end encrypted. The provider, Google, the carriers and this server can read it.
- It has no group chats, no edits, no deletes and no reactions.
- RCS on the user's own personal number is not possible this way ([plan §16](../../plans/Comms-Messaging.md#16-rcs)).

The design is [plans/Comms-Messaging.md](../../plans/Comms-Messaging.md) §16 RCS phases 1 and 2, and §18.3 phases 10
and 13. Phase 2 (§18.3 phase 13) adds rich cards and suggestion buttons, read receipts, the typing indicator and the
RCS labels ([Rich messages](#rich-messages) and below).

- **Rust:** `crates/moqui-channels`. Each RCS adapter is a part of its provider's module, on the neutral SMS core
  (`sms.rs`, [SMS](SMS.md)):
  - `rcs-twilio`: `providers/twilio.rs` and `providers/twilio/rcs.rs`;
  - `rcs-sinch`: `providers/sinch.rs` and `providers/sinch/rcs.rs`.
- **Entities:**
  - the bridge type `CbtRcs` (RCS) in `ChatEntities.xml`;
  - a delivery's `deliveryPath` (`rcs`, `sms` or `mms`) and `providerContentId` (a Twilio Content made for its
    send) on `ChatBridgeDelivery` in `VoiceEntities.xml`;
  - a rich message's `content.rich` and an answer's `content.postback` on the message (`ChatEvent.contentJson`).
- **Services:**
  - `ChatBridgeServices.note#DeliveryPath`, `note#DeliveryContent`, `send#BridgeTyping`, `clean#ProviderContent`;
  - `ChatRichServices.send#Rich`, `reply#Suggestion`, `note#SuggestionReply` (`ChatRich.js`);
  - the SMS services, unchanged: `check#SmsSend`, `ensure#PairRoom`, `receive#Sms`.

## How it works

- **The bridge and its number.** An RCS bridge is a chat bridge of network RCS whose adapter is `rcs-twilio` or
  `rcs-sinch`. It has exactly one number, its home number (below), given it in System, Voice, Numbers (Texts through)
  as an SMS bridge is. A second number is refused (`rcsOneNumber`), and so is a number that is not the bridge's home
  number (`notHomeNumber`). Several numbers on one RCS bridge are later work: they need each person's replies and
  opt-out followed across the bridge's numbers, since the agent's thread is one for all of them.
- **The room.** One room for each outside number, `ours/theirs`, as for SMS. It is named after their number, and its
  topic says "RCS and texts".
  - The person's RCS replies come into it.
  - So do their SMS replies to the fallback number.
  - On their phone these are two threads: the agent's and the number's.
- **Who is in it, and who may send.**
  - Its members are the number's users.
  - Only users granted the number may send from it.
  - Every send is checked as a text is (`check#SmsSend`): the grants, the opt-out (STOP), and the day's limits. RCS is
    billed as texts are.
- **Giving a number to an RCS bridge** makes every text of that number's rooms RCS-first, sent from the brand
  agent's name and not from the number ([plan §19.4](../../plans/Comms-Messaging.md#194-open-rcs) Q1). A phone with
  no RCS still gets them as SMS from the number.
- **The system's number stays on SMS.** The system's own texts (sign-in codes) go from the system number
  (System, Voice, Fraud). Two refusals keep it on an SMS bridge, both with reason `systemNumber`:
  - Numbers refuses to give the system's number an RCS bridge.
  - Fraud refuses to make a number of an RCS bridge the system's number.
- **The home number.** An RCS message that comes in names the agent, not a number. It goes to the room of the
  bridge's home number:
  - Twilio's `fallbackFrom`;
  - Sinch's `smsSender`.

  Set it in the bridge's configuration first, then give that number the bridge in Numbers. Until the number has the
  bridge, the RCS messages are left (`unknownNumber`). Chat Bridges refuses a home number that is not the bridge's
  number, and refuses to make a bridge RCS while it has several numbers or the system's.
- **The fallback.** `smsFallback` is true by default: a phone with no RCS gets the message as SMS, or as MMS for a
  file, from the room's number. Set it false to send RCS only; a phone with no RCS then gets nothing, and the
  delivery fails. RCS-only is a setting of the whole bridge. A switch for one room or one message is later work
  (§16 RCS phase 1 step 5).
- **A reply to a fallback text** comes to the number the fallback went from:
  - When that number's bridge is this RCS bridge, the reply goes to its RCS room.
  - When the number's bridge is an enabled SMS bridge of the same provider (`sms-twilio` for `rcs-twilio`,
    `sms-sinch` for `rcs-sinch`) and the same provider account (Twilio: the webhook's `AccountSid` is both bridges'
    `accountSid`; Sinch: its `project_id` and `app_id` are both bridges'), it goes to that number's SMS room, kept as
    that bridge's. This applies only within the RCS bridge's verified webhook.
  - Otherwise (another provider's bridge, another account, a disabled bridge) the reply is left before anything is
    kept, and logged: `otherProvider`, `otherAccount`, `bridgeDisabled`.
- **The path.** Each sent message records the way it went: RCS, SMS or MMS.
  - A send that cannot fall back is RCS as soon as the provider takes it.
  - Any other send gets its path from the provider's reports.
  - Once a message went as SMS or MMS, it never shows RCS again.
  - The message's caption says so ("Delivered to RCS as SMS", one translatable string), and its Info has a "Sent as"
    line. The terminal's Info
    has the same line.
- **The labels** (plan §16.5). An RCS room's header says "RCS Business - not end-to-end encrypted", with the open lock
  and a verified badge (the brand's agent). Its banner: "Encrypted in transit only. Twilio, Google, carriers and this
  server can read it." (Sinch, or "The provider"), in place of the SMS banner. Each message's Info has an
  "Encryption" line by the way it went: that text for RCS, "The phone did not support RCS. This message went as SMS."
  (or MMS) for the fallback. The composer says "RCS message". The terminal shows the banner and the Info line.
- **Read receipts.** Twilio's `read` status and Sinch's `READ` report are the message read on the phone: past
  delivered, never taken back. The caption says "Read on RCS". A failed message stays failed (and can be sent again):
  a later read changes nothing.
- **Typing.** While a user types in an RCS room, the person sees the agent typing: Twilio's typing indicator (it needs
  the bridge's `rcsSenderId`) or Sinch's `composing_event`, at most once in 15 seconds for a room (started only for a
  room with an RCS bridge), only to a person a message of ours reached, never to a person
  who sent STOP. `typingIndicators` false turns it off for the bridge. Neither provider documents the person's typing
  coming in, so none is shown. Our own read receipts going back to the person (Sinch's `read_message_event`) are left
  out: they need a setting of their own, later.
- **What comes in.**
  - Words and files, as for a text.
  - A shared place: its label and address, and an OpenStreetMap link built from its coordinates. The coordinates are
    kept on the message.
  - A tapped suggestion: its words (Twilio) or its payload (Sinch), its payload kept on the message
    (`content.postback`) and handed to `ChatRichServices.note#SuggestionReply`, where an app's SECA picks it up.
  - Opt-out keywords: STOP and START work as for SMS ([SMS, Opt-out](SMS.md#opt-out)).

## Capabilities

| Operation | RCS |
| --- | --- |
| Text and one or more files | yes, within the bridge's limits; the fallback's MMS limits apply by default |
| Delivery receipts | yes, `read` past delivered |
| Rich messages: cards, carousels, suggestion buttons | yes ([Rich messages](#rich-messages)); every other bridge gets their text |
| Typing indicator | out only, from a user of ours to the person |
| The path a message went | yes (`deliveryPath`) |
| Edit, delete, reaction | no (`bridgeUnsupported`) |
| Group chats | no: a group room on an RCS bridge refuses to send (`groupTextsNotSupported`) |
| Idempotent sends | no: a lost answer is uncertain and never sent again on its own |
| Encryption | `Transport`: encrypted in transit only, never end to end |

## Rich messages

A rich message is a message with `content.rich`: a text with suggestion buttons (chips), a card, or a carousel of 2 to
10 cards, each with its buttons. A button is a reply (its words come back, with its postback), a link (`http` or
`https`) or a call (an E.164 number). The server makes its text from the rich content (`ChatRich.js`): the words, each
card's title and words, "label: URL" for each link, "label: number" for each call and "Reply: Yes, No". That text is the
message's body: the terminal, search, notifications and every other bridge show and send it, and an SMS room gets it
with the cards' pictures as MMS. The limits are the stricter of Twilio's and Sinch's, so one message goes on both:

| | Text with buttons | Card | Carousel |
| --- | --- | --- | --- |
| Words | 1 to 1600 characters | title up to 200, words up to 1600 | title and words 160 together, each card |
| Picture | none | none or one | one on every card |
| Buttons | 1 to 10 | up to 4 | 1 or 2 a card, the same kinds in the same order on every card |

A button's words are up to 20 characters, a reply's postback up to 200. The text form is at most 1600 characters. A
card has its title or its words and one thing more (the other, a picture or a button), as Twilio's card needs. Pictures
are PNG, JPEG or GIF (Twilio's RCS types), each within the provider's RCS limit (`maxRcsMediaBytes`: Twilio 16 MB,
Sinch 100 MB) and 64 MB together (`maxRcsTotalBytes`); the MMS total (`maxMediaBytes`) applies only to the fallback's one
picture, the first, which is left out (the fallback is then text alone) when it is over that.

- **Who sends one.** An app or a service: `ChatRichServices.send#Rich` (mr-boss, marketing, an agent). A member: the
  composer's Card (a title, words, a picture and up to 4 buttons; with no title and no picture, a text with buttons),
  which shows the text people without RCS get. A carousel and a provider's template come from apps only. None in an
  encrypted room: the server makes its text.
- **A tap here.** In a room with no outbound bridge, a member taps a reply on another's message: a message with its
  words (`reply#Suggestion`). In an RCS room the buttons are the person's, on their phone: here they show only.
- **An answer.** A message answering a button has `content.postback` and the caption "Chose a suggestion". An app that
  sent the buttons puts its own context in the postback (`order:7:confirm`) and runs on a SECA of
  `ChatRichServices.note#SuggestionReply`.
- **Postbacks are untrusted.** Only an app's own call (authorization disabled) names a reply's postback; a member's is
  always `user:` and the button's words, and an app's may not start with `user:`. An RCS reply's postback comes from the
  person's device: it is not signed, and a device can send any text. `note#SuggestionReply` says who wrote the tapped
  message (`authorUserId`), whether it was an app's (`fromApp`) and whether the reply came from outside (`outside`: an
  RCS reply names no message). An app checks the tapped message's author, and that it was its own, before it acts on a
  postback; an outside one is only the device's word.
- **To a bridge with no RCS** (SMS, `sms-sinch`, any other provider): the text form with the first picture only, so a
  carousel never fails for its count.
- **Twilio.** A Content resource is made for each rich send (`POST https://content.twilio.com/v1/Content`:
  `twilio/card` for a card or a text with buttons, `twilio/carousel` for cards, and with the fallback `twilio/text`, or
  `twilio/media` with the first picture), then the message is sent with its `ContentSid`. Its id is kept on the
  delivery (`providerContentId`), and the delivery's lease is renewed for the second step. The message's final status
  (delivered, read, failed, undelivered) deletes it (`DELETE /v1/Content/{sid}`). An app that sends one card to many
  people names a template it made once in Twilio (`rich.providerContent.twilio`: `contentSid`, `contentVariables`), sent
  as it is and never kept or deleted.
  - Each Content is named `moqui-{delivery}-{attempt}`. A retry sends the Content it kept again when the message has no
    picture (a picture's address expires); else it deletes it and makes a new one. With none kept (the create's answer
    was lost), it searches the delivery's earlier ones by name (`GET /v2/Content?ContentName=moqui-{delivery}`) and
    deletes them first.
  - A Content step that could not be done (429, 5xx, a lost answer) is tried again, and no message went. One Twilio
    refuses (another 4xx) sends the text form and its picture as a plain message instead, noted on the delivery
    (`deliveryNote` `richRefused`, its Info: "The provider refused the card; its text went instead.").
  - The delivery worker's daily sweep deletes the Contents still named on deliveries whose last attempt was more than 2
    days ago and that no worker holds (a status that never came, an uncertain send), the oldest first; one that cannot
    be deleted is counted (`providerContentTries`) and let go after 5 sweeps, logged. Then it deletes our `moqui-`
    Contents made more than 2 days ago that no delivery names (`GET /v2/Content?ContentName=moqui-&DateCreatedBefore=`,
    3 pages at most). All of it is best effort: a failed delete is logged and tried again by the next sweep.
  - A carousel's own `body` (RCS drops it) is cut to 1024 characters; the whole text form goes in `twilio/text` or
    `twilio/media`. Its cards' buttons name no `chip_list` (the carousel page documents none).
- **Sinch.** One `messages:send`: `choice_message` for a text with buttons, `card_message`, `carousel_message`. With
  the fallback, the message's (the AppMessage's) `explicit_channel_omni_message` gives SMS our text, or MMS a card of it
  with the first picture, in place of Sinch's own rendering.

## Locations

A chat location (plans/Comms-Messaging.md §18.3 phase 20) with coordinates goes on Sinch RCS as a `location_message`
(`coordinates`, `title` its label or "Location", `label` its address), with its words and map link as the SMS fallback
(`explicit_channel_omni_message`). Sinch's RCS page says RCS has no location message of its own and that the
Conversation API sends it as a text with a location choice (read 2026-11-17). An address-only location, and every
location on Twilio (whose `twilio/location` content is WhatsApp's only), goes as its words: "Location: label, address"
and an OpenStreetMap link. A location the person shares from their phone comes in as a location message with the same
words (Twilio's `Latitude`, `Longitude`, `Label`, `Address`; Sinch's `location_message`).

## Twilio

Twilio's RCS is generally available since 2025-08-26 ([send RCS](https://www.twilio.com/docs/rcs/send-an-rcs-message),
[onboarding](https://www.twilio.com/docs/rcs/onboarding)).

1. **The RCS sender.** In the Console, under Numbers & senders, RCS:
   - Make the sender: a display name, a description, a logo (224 × 224), a banner (1440 × 448), an accent color,
     contact details, and privacy and terms URLs.
   - **Test devices:** add phones in E.164; each accepts the tester invite before it gets test messages.
   - **Compliance:** pick the countries, and register with Google and, for US recipients, with the US carriers. The US
     registration needs business details (EIN), the opt-in and opt-out policy, a use-case description and a video.
   - **Approval:** Twilio says four to six weeks or more. Once one carrier approves in a country, the sender works there.
   - **Fees:** the brand vetting and carrier fees come from the companies that verify the brand, not from Twilio.
     Per-message prices are on Twilio's pricing page.
2. **A Messaging Service**:
   - its sender pool holds the RCS sender, plus an SMS number for the fallback (the fallback needs A2P 10DLC in the
     US, as any text does);
   - turn on Advanced Opt-Out;
   - leave the service's own status callback empty: each send names the bridge's webhook.
3. **The webhooks.** Use `https://<server>/channels/<chatBridgeId>/webhook` (as for [SMS](SMS.md#twilio)) in two
   places:
   - the RCS sender's Configuration tab: its "Webhook URL for incoming messages" (POST) and its fallback URL;
   - the home number's incoming webhook, so SMS replies to the fallback come to the same bridge.

   A number of an `sms-twilio` bridge that is also in the service's pool keeps its own webhook, to its own bridge; a
   reply to it that comes to the RCS bridge anyway is handed to its bridge when the account is the same (above).
   Twilio signs each request with the Auth Token, and the signature is checked on the server that holds it.
4. **The bridge.** System, Chat Bridges, network RCS, adapter `rcs-twilio`. Its configuration (kept encrypted) has the
   `sms-twilio` keys ([SMS, Twilio](SMS.md#twilio)) and these:

   | Key | What |
   | --- | --- |
   | `messagingServiceSid` | `MG...`, the service with the RCS sender in its pool, required |
   | `fallbackFrom` | The home number in E.164, required: our number of the RCS messages that come in |
   | `smsFallback` | true (the default) or false (RCS only) |
   | `rcsSenderId` | The agent's id (the `rcs:` address of its inbound `To`), for the typing indicator; none sends none |
   | `typingIndicators` | true (the default) or false (no typing indicator) |
   | `contentLanguage` | The language of the Content made for a rich message (`en`) |
   | `contentBaseUrl`, `messagingBaseUrl` | `https://content.twilio.com` and `https://messaging.twilio.com` (a test's fake) |

**Out.** `POST /2010-04-01/Accounts/{AccountSid}/Messages.json` with `MessagingServiceSid`, `To` the person's number,
`FallbackFrom` the room's number, `Body`, a `MediaUrl` for each file, and `StatusCallback`.
- Twilio falls back from `FallbackFrom`, so a reply comes to the room's number and not to another number in the pool.
- With `smsFallback` false, `To` is `rcs:+1…` and there is no `FallbackFrom`: Twilio sends RCS only.
- Twilio packs words and a picture into one rich card.
- Its error codes are read as for SMS.

**In.**
- An RCS message's `From` is `rcs:+1…` and its `To` is the agent's `rcs:` address. The prefix is taken off, and our
  number is `fallbackFrom`. A sender that is not in E.164 (a short code) is taken as `sms-twilio` takes it: a room of
  its own.
- An SMS reply to a number is read as for `sms-twilio`.
- A place: `Latitude`, `Longitude`, `Label`, `Address`.
- A tapped suggestion: `Body`, the button's words, and `ButtonPayload`.
- An `sms-twilio` bridge leaves an RCS message, but keeps an RCS status callback (a Messaging Service with an RCS
  sender sends its texts as RCS first).

**The path.** A status callback whose `From` is the agent's `rcs:` address went as RCS. One whose `From` is a number
went as the fallback: SMS, or MMS when the message has files.

## Sinch

Sinch's RCS is a channel of the Conversation API
([RCS](https://developers.sinch.com/docs/conversation/channel-support/rcs),
[set-up](https://developers.sinch.com/docs/conversation/channel-support/rcs/set-up),
[message support](https://developers.sinch.com/docs/conversation/channel-support/rcs/message-support)).

1. **The RCS agent.** Get a Sinch RCS sender agent: Sinch's own guides on the Sinch Community cover the brand, the
   Google verification, the test devices and the carrier launch. It must be in the same region as the app (US, EU or
   BR), or the RCS channel fails and its delivery reports go missing.
2. **One Conversation API app for the bridge.** Set up its channels:
   - **RCS:** the agent, in the dashboard's "Set up channel", or by the API with the sender's `authName` and
     `authToken`;
   - **SMS:** the home number;
   - **MMS:** the same number, for files.

   Its webhook is `https://<server>/channels/<chatBridgeId>/webhook` with a secret, on the triggers `MESSAGE_INBOUND`,
   `MESSAGE_DELIVERY`, `OPT_IN` and `OPT_OUT`, as for [SMS, Sinch](SMS.md#sinch). Keep this app apart from any
   `sms-sinch` bridge's app: an app's webhook goes to one bridge.
3. **The bridge.** Network RCS, adapter `rcs-sinch`. Its configuration has the `sms-sinch` keys and these:

   | Key | What |
   | --- | --- |
   | `smsSender` | The home number, required: our number of the RCS messages that come in, and the fallback's SMS sender |
   | `mmsSender` | The MMS channel's sender; a file whose fallback would be MMS from another number is refused (`mmsNotSetUp`) |
   | `smsFallback` | true (the default) or false (RCS only) |
   | `typingIndicators` | true (the default) or false (no typing indicator) |

**Out.** `messages:send` to the person's `RCS` identity and their `SMS` identity (`MMS` for a file), with
`channel_priority_order` `["RCS","SMS"]` (or `["RCS","MMS"]`) and `SMS_SENDER` (or `MMS_SENDER`) set to the room's
number.
- With `smsFallback` false: only the `RCS` identity and `["RCS"]`.
- The message is a text, a file, or a card for words and a file; Sinch renders it for each channel.
- Sinch's RCS takes 3072 characters and files up to 100 MiB, but the bridge's defaults are the fallback's: 1600
  characters and one file of 1 MiB.

**In.** The `RCS` channel's messages:
- `text_message`;
- `media_message`;
- `location_message`: a place;
- `choice_response_message`: a tapped suggestion, its `postback_data`.

An RCS message's `sender_id` is not read, since it is the agent's and not a number: our number is `smsSender`. An
`OPT_OUT` or `OPT_IN` on the `RCS` channel is a STOP or START, as an SMS one is. An `sms-sinch` bridge still leaves
every RCS message.

**The path.** A delivery report's `channel_identity.channel` gives it: `RCS`, `SMS` or `MMS`. `SWITCHING_CHANNEL`
means RCS did not reach the phone and the fallback is tried: SMS, or MMS when the message has files.

## Not confirmed

These are not in the providers' documentation, or it is unclear there. Check each in the live test.

- **Sinch, a location message on RCS.** How Sinch renders our `location_message` on the phone (its RCS page and its
  location page differ), and that its SMS fallback is our text.

- **Twilio, the inbound address format.** That an RCS message's `From` is `rcs:+1…` and its `To` the agent's `rcs:`
  address when the sender is in a Messaging Service. Twilio's Studio guide shows both prefixed only for a sender
  without a service. The adapter takes a plain number too.
- **Twilio, the fallback in a callback.** That a status callback's `From` is a number when the message fell back.
  Twilio documents only that an RCS delivery shows `rcs:<SenderId>`, and names no other fallback field.
- **Twilio, the limits.** RCS's text length and file sizes: Twilio's RCS page says files up to 100 MB, while the bridge
  keeps the fallback's MMS limits.
- **Twilio, RCS STOP.** Whether a STOP sent on RCS is handled by Advanced Opt-Out, as an SMS one is.
- **Sinch, reports after a switch.** Whether a delivery report after `SWITCHING_CHANNEL` names the new channel in
  `channel_identity.channel`.
- **Sinch, the identity format.** Whether the RCS channel's identity takes the `+` of E.164: the adapter sends it with
  one.
- **Sinch, what `sender_id` holds on RCS.**
- **Sinch, countries, timelines and fees.** Sinch's RCS pages give none; the onboarding is in Sinch's Community guides
  and its sales terms.
- **Twilio, content approval.** Whether a Content for RCS needs approval before it is sent: Twilio's pages name
  approval for WhatsApp only.
- **Twilio, the content's fallback.** That Twilio sends the content's `twilio/text` or `twilio/media` as the SMS or MMS
  fallback (its page says it sends "the most complex type the channel supports"), and that `FallbackFrom` holds with a
  `ContentSid`.
- **Twilio, a tapped quick reply.** That its `id` comes back as `ButtonPayload` on RCS.
- **Twilio, the Content search.** That `GET /v2/Content` with `ContentName` finds a Content by its `friendly_name` (the
  reference calls it a search pattern, not an exact match: the adapter checks the name's prefix itself) and answers
  `contents` and `meta.next_page_url` as the v1 list does.
- **Twilio, a carousel's `body`.** Its limit: the page gives none, so 1024 is kept, and RCS drops it.
- **Twilio, the typing indicator's auth.** Twilio's example signs it with an API key; the adapter uses the bridge's key,
  or its account and token.
- **Sinch, the fallback override.** The shape of `explicit_channel_omni_message`'s value (the reference names a map of
  channel to content, with no schema), and that MMS shows a `card_message` with its title as the subject.
- **Both, typing fees.** Whether a typing indicator is billed. The indicator goes only from a room with an RCS bridge,
  to a person a message of ours already reached, never after a STOP.
- **Both, typing and read in.** Neither documents the person's typing coming to the webhook; read receipts come as
  documented (`read`, `READ`), untested.

## Tests

`crates/moqui-channels/src/tests/rcs.rs`, on the SMS test bed with a fake Twilio and a fake Sinch, each test under a
second:
- the capabilities and the required settings;
- RCS, places and tapped suggestions in, to the home number's room;
- each send's fields with and without the fallback, and with a file;
- the path from the send and from the reports, never back from the fallback;
- a reply to a fallback text, to the RCS room and to a same-provider, same-account SMS bridge's room (Twilio and
  Sinch); left for another provider's, another account's and a disabled bridge's, and outside the bridge's webhook;
  the webhook mark put back after an error;
- one number per RCS bridge, its home number; the network and adapter agreeing in Chat Bridges; a short-code sender;
- the RCS opt-out;
- the system's number kept on SMS;
- rich messages: Twilio's content and send (a card, a text with buttons without the fallback, a carousel), its delete
  on the final status and by the sweep, an app's own template never made or deleted, a refused and an unavailable
  content step; Sinch's choice, card and carousel messages with the SMS and MMS overrides; an SMS bridge sending the
  text and the picture;
- `read` and `READ`, never back;
- the typing indicator of each, throttled, off, without the agent's id, and never after a STOP.

`crates/moqui-core/src/service/chat_bridge_tests.rs` checks that a rich message reaches the adapter with its text, and
the typing indicator's throttle past a failing bridge. `basic-comms/test/BasicCommsRich.spec.js` checks the limits, the
text form, the refusals and the answers to buttons. `webroot/test/client/WebrootChat.spec.js` checks the caption's
"as SMS", Info's "Sent as" and "Encryption" lines, the RCS label and the composer's text form.
