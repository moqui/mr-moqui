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

The design is [plans/Comms-Messaging.md](../../plans/Comms-Messaging.md) §16 RCS phase 1 and §18.3 phase 10. Rich
cards, suggestion chips, read receipts, typing and the RCS labels in the thread header are phase 2.

- **Rust:** `crates/moqui-channels`. Each RCS adapter is a part of its provider's module, on the neutral SMS core
  (`sms.rs`, [SMS](SMS.md)):
  - `rcs-twilio`: `providers/twilio.rs` and `providers/twilio/rcs.rs`;
  - `rcs-sinch`: `providers/sinch.rs` and `providers/sinch/rcs.rs`.
- **Entities:**
  - the bridge type `CbtRcs` (RCS) in `ChatEntities.xml`;
  - a delivery's `deliveryPath` (`rcs`, `sms` or `mms`) on `ChatBridgeDelivery` in `VoiceEntities.xml`.
- **Services:**
  - `ChatBridgeServices.note#DeliveryPath`;
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
  - The thread header keeps the SMS banner until phase 2's RCS labels.
- **What comes in.**
  - Words and files, as for a text.
  - A shared place: its label and address, and an OpenStreetMap link built from its coordinates. The coordinates are
    kept on the message.
  - A tapped suggestion: its words (Twilio) or its payload (Sinch), with the payload kept on the message for phase 2.
  - Opt-out keywords: STOP and START work as for SMS ([SMS, Opt-out](SMS.md#opt-out)).

## Capabilities

| Operation | RCS |
| --- | --- |
| Text and one or more files | yes, within the bridge's limits; the fallback's MMS limits apply by default |
| Delivery receipts | yes; `read` counts as delivered until phase 2 |
| The path a message went | yes (`deliveryPath`) |
| Edit, delete, reaction | no (`bridgeUnsupported`) |
| Group chats | no: a group room on an RCS bridge refuses to send (`groupTextsNotSupported`) |
| Idempotent sends | no: a lost answer is uncertain and never sent again on its own |
| Encryption | `Transport`: encrypted in transit only, never end to end |

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
- **Both, typing and read.** Typing events (Twilio's `Indicators/Typing` API and Sinch's `composing_event`) and read
  receipts as their own state are phase 2, untested.

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
- the system's number kept on SMS.

`webroot/test/client/WebrootChat.spec.js` checks the caption's "as SMS" and Info's "Sent as" line.
