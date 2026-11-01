# SMS and MMS

Texts to and from outside numbers, through the provider's messaging API and webhooks, on the chat bridge layer
([Chat bridges](Chat-Bridges.md)). Telnyx is the first provider (D-V4). A number's texts, calls and voicemail land in one
room per outside number. The design is [plans/Comms-Framework.md](../../plans/Comms-Framework.md) §5.6, §5.4 ("One
conversation per outside number"), §4.3 ("Business channels"), milestone C4-SMS. Texting needs no SIP engine: a number
with a messaging bridge and no trunk texts.

- **Rust:** `crates/moqui-channels` (the `channels` feature of the host, in Maximal and Enterprise): the `sms` adapter
  (`telnyx.rs`) and its webhook (`webhook.rs`). `crates/moqui-core/src/service/chat_bridge_web.rs`: the webhook and media
  routes' core (`chat_bridge::webhook`, `media`, `media_url`), `store#BridgeMedia` and `send#BridgeDirect`. The host's
  routes are `crates/moqui-host/src/channels.rs`.
- **Services:** `org.moqui.impl.SmsServices` and `Sms.js` (the pair room, the checks before a send, texts that come in),
  `org.moqui.SmsServices.send#SmsMessage` (the system's sender), `ChatBridgeServices` (`ingest#Event` with media,
  `note#DeliveryReceipt`), the SMS parts of `VoiceServices`/`Voice.js` (a number's `messagingBridgeId`, the SMS limits).
- **Entities:** the C4-SMS block of `framework/entity/VoiceEntities.xml`: `SmsOptOut`, `SmsSendCount`, the SMS fields of
  `VoicePolicy`, a delivery's receipt on `ChatBridgeDelivery`; `ChatRoom` type `CrtExternal`.
- **Screens:** System, Chat Bridges (the bridge), System, Voice: Numbers (Texts through) and Fraud (the SMS limits, the
  system's number); Calls History's Text a number; the pair's room in Chat (its SMS icon and bridge chip).

## Setup

1. **The provider.** In the Telnyx portal: a messaging profile with the numbers on it, an API key, and the account's
   public key for webhooks. Set the profile's inbound webhook URL to `https://<server>/channels/<chatBridgeId>/webhook`.
2. **10DLC (United States).** Texts from a local US number to US phones need the brand and campaign registered with The
   Campaign Registry through Telnyx (an operator's step in the portal, which takes days); the numbers are then assigned
   to the campaign. Keep its id in the bridge's `campaignId` for the record. Moqui does not register or check it: an
   unregistered number's texts are refused or filtered by the carriers (Telnyx answers its own error, kept as the
   delivery's reason, `notRegistered` for the codes known).
3. **The bridge.** System, Chat Bridges, a bridge of network SMS whose configuration (kept encrypted, never shown again)
   is JSON:

   | Key | What |
   | --- | --- |
   | `apiKey` | The API key, required |
   | `publicKey` | The webhook public key (base64 Ed25519), required for texts that come in |
   | `messagingProfileId` | Sent with each text when set |
   | `campaignId` | The 10DLC campaign, for the record |
   | `apiBaseUrl` | `https://api.telnyx.com` |
   | `maxTextLength` | 1600 characters |
   | `maxMediaCount`, `maxMediaBytes` | 10 files and 1 MiB in all for an MMS that goes out |
   | `maxInboundMediaBytes` | 5 MiB, the largest file taken from a text that comes in |
   | `webhookToleranceSeconds` | 300 |
   | `sendsPerMinute` | 60 (the bridge's own field wins); Telnyx's long-code rate is lower per number, so set it to what the account allows |
   | `allowInternalAddresses` | false; only a test's fake provider needs it |

4. **The numbers.** System, Voice, Numbers: Texts through the bridge. A number's users (the user it rings, a ring group's
   members, its caller-ID grants) are the members of its pair rooms; only users granted the number (Caller IDs, or a ring
   group's number) send from it.
5. **The server.** `channels_webhooks_enabled=true`; `channels_public_url` the https base the provider reaches this server
   at (an MMS that goes out is refused, `mediaUnreachable`, without it); System, Voice, Fraud: the SMS limits and the
   system's number (sign-in codes).

## The number pair's room

One room for each pair (our number, their number): `ChatRoom` type `CrtExternal`, named their number, visible to its
members only, mapped to our number's messaging bridge by a `ChatRoomBridge` whose `externalRoomId` is `ours/theirs`.
`SmsServices.ensure#PairRoom` finds or makes it under the number's row lock, in a transaction of its own, before a text
is taken in (`ingest#Event` refuses an unmapped room) and before a call links to it, and adds as members the users the
number reaches; a user who left stays out until they open it again (Text a number). Its members are the number's
users and nobody else: no member adds another (`add#Member` refuses in a `CrtExternal` room), and when a grant ends or
the number's route, bridge or ring group changes (`revoke#CallerId`, `store#PhoneNumber`, `delete#PhoneNumber`,
`delete#RingGroupMember`), the users it no longer reaches are taken out of its pair rooms at once (`Sms.js`
`syncNumberRooms`), so they see none of the pair's later texts; one reached again is back at the pair's next text or
call. The outside person is an external actor of the bridge (`ChatExternalUser` keyed by their number).

- **Calls.** A phone call to or from the pair (`receive#PhoneCall` for a user or a ring group, `place#PhoneCall`) is in
  the room: `CallSession.chatRoomId` is set and its start and end are `CetCall` events there. Nothing else about the call
  changes: the room's members are not in the call and cannot see or join it by being in the room (a phone call is left
  out of the room members' view in `Calls.js` `canSee`). A number without messaging makes no room for its calls. Calls
  through an IVR menu or a queue, and the voicemail access number, are not linked.
- **Voicemail.** A message left on such a call is a `CetCall` event (action `voicemail`, its `voicemailId`) in the room;
  its audio stays the box's, for the box's readers only.
- **Text a number.** Calls History has it beside Call a number for a user with a number that texts:
  `open#PairRoom` (the same checks as a send) opens the room in Chat.

## Texts that come in

`POST /channels/{chatBridgeId}/webhook` (off unless `channels_webhooks_enabled`):

1. The host reads the raw body first, at most `channels_webhook_max_bytes` (256 KiB; a larger declared or sent body is
   413), and hands it with the headers to the bridge's adapter on a request worker. An unknown bridge, or one this build
   has no adapter for, is 404; a disabled one 503, so the provider tries again later.
2. The adapter checks `telnyx-signature-ed25519`, the Ed25519 signature of `{telnyx-timestamp}|{raw body}` under the
   bridge's `publicKey` (aws-lc-rs, the crypto provider rustls already links), before it parses anything. A missing or
   wrong signature, or one whose timestamp is more than `webhookToleranceSeconds` from now, is 401 and counts for web
   protection as a refused login. A signature already worked within the window is answered 200 `duplicate` and not
   worked again, one still being worked on another worker 409; neither counts for web protection (the signature is
   the provider's, and counting it could block the provider's address). A signature is held from when its request is
   taken and let go when it was not worked (not 2xx), so the provider's retry of one that failed goes through. The
   cache is the node's; another node's copy is deduped by the ids below.
3. `message.received`: `SmsServices.receive#Sms` checks that our number's messaging bridge is this one (else 200,
   `ignored`), keeps a STOP, START or HELP (below), finds or makes the pair room, and hands the text to `ingest#Event`
   (`transportVerified`: no inbound token, honored only inside that bridge's webhook, which `chat_bridge::webhook` marks
   on the context; a caller passing the parameter alone gets `badToken`) as transaction the webhook's event id, item the message id. A webhook sent
   again is a `duplicate` and does nothing; so is the same message under a new event id (`ChatEventExternal`). Its files
   (`media`) are fetched by the adapter, each on its own, within `maxInboundMediaBytes`, and kept as `ChatAttachment`
   rows of the room with no uploader (`store#BridgeMedia`): a type outside images, audio, video, vCard, plain text and
   PDF is kept as `application/octet-stream`, so nothing from a network is served as a page. A file that cannot be
   fetched is named in the message (`[A file not received: image/png]`) and the rest go on.
4. `message.sent` and `message.finalized` (the receipts of texts that went out): `note#DeliveryReceipt` keeps the
   receipt on the message's delivery (`sent`, `delivered`, `unconfirmed`); a `sending_failed` or `delivery_failed` fails
   the delivery (`deliveryFailed`, with the carrier's reason), which its sender may send again. A receipt never takes a
   delivered text back to sent. A receipt of a text Moqui did not send is answered and left.
5. A failure of the server's own is 500, so the provider tries again; the batch was stored before (§4.1).

## Texts that go out

A user writes in the pair's room; the delivery layer sends it. Before the provider, the adapter asks
`SmsServices.check#SmsSend`, and a refusal fails the delivery with its reason (shown on the message, not retried):

| Reason | When |
| --- | --- |
| `numberNotGranted` | The sender is not granted our number (a member through its route alone reads but does not send), or the message has no sending user (a text in a room never goes as the system) |
| `internationalNotEnabled`, `premiumNotEnabled` | The destination's class (Voice.js `classify`, as for a call) is not enabled for the deployment and the user (VoicePolicy, VoiceUserSetting) |
| `emergency` | An emergency number: texts to them are not sent from here |
| `optedOut` | The person sent STOP to our number and not START since |
| `consentRequired` | A promotional text (`send#SmsMessage` `isPromotional`) with no `sms-consent` provider saying yes |
| `dailyLimit` | The sender's (`smsMaxPerUserPerDay`, 200) or the deployment's (`smsMaxPerDay`, 2000) texts of the day, counted in `SmsSendCount`. The check and the count are one step (`reserve`) under the lock of the day's deployment count, so sends at once cannot pass a limit together; a text that does not reach the provider (refused, retried, unreachable) is taken off again (`release#SmsSend`), and one whose answer was lost still counts |
| `textTooLong`, `tooManyFiles`, `mediaTooLarge`, `mediaUnreachable` | The bridge's limits, or no `channels_public_url` for a file |

The text goes to `POST /v2/messages` with the delivery's idempotency key in `Idempotency-Key`; Telnyx does not dedupe
on it, so the adapter is not idempotent. Its outcome:

- 2xx with the message id: accepted (the id is the delivery's remote id, for the receipts).
- 429 or 503: tried again after the backoff. 401 or 403 (the key): tried again, the bridge not connected.
- Another 4xx: failed, with Telnyx's reason (`optedOut` for its STOP block, 40300; `badNumber`; `notRegistered`;
  else `providerRefused`).
- A connection that could not be made: tried again, the bridge not connected.
- An answer lost after the request went (a timeout, a reset, another 5xx): uncertain. It is never sent again on its
  own; its sender, a moderator or a bridge admin may send it again (`retry#Delivery`), and then it may arrive twice.

Its files go as addresses the provider fetches: `GET /channels/{chatBridgeId}/media/{attachment}.{expiry}.{signature}`
under `channels_public_url`, signed with HMAC-SHA-256 under `channels_media_key` (generated when unset), good for
`channels_media_minutes` (60), for a file of a message with a delivery to that bridge only, sent as an attachment with
`nosniff`.

Each text that reached the provider (accepted or uncertain) counts for the day of its sender and of the deployment.

## Opt-out

A text whose whole body is STOP (or STOPALL, UNSUBSCRIBE, CANCEL, END, QUIT, OPTOUT, REVOKE) opts the person out of
texts from that number of ours (`SmsOptOut` per pair); START (UNSTOP, YES, OPTIN) opts back in; HELP (INFO) is kept as
the pair's last keyword. The keyword is still a message in the room. Moqui sends no reply to a keyword: Telnyx's
messaging profile answers STOP and HELP itself (its opt-out management), and blocks a send to a number that sent STOP
(its error 40300, kept as `optedOut`). The opt-out also gates the system's texts.

## The system's texts

`org.moqui.SmsServices.send#SmsMessage` keeps the interface of the Java stub (`countryCode`, `areaCode`, `contactNumber`,
`message`, `isPromotional`; `successful`, `messageId`, `errorMessage`) with `fromNumber` and `userId` added, and is
internal (`allow-remote="false"`). From `fromNumber`, else `VoicePolicy.smsSystemNumber`, it goes straight to the
provider (`send#BridgeDirect`: no room, no delivery row, no retry), so a sign-in code (`UserServices.send#AuthcCodeSms`)
is seen by nobody else. The checks are `check#SmsSend`'s, with the system's day limit when `userId` is empty. A refusal
or a failure is an error with its `reasonCode`. Marketing's texts (Mantle) go through it with `isPromotional`, and Mantle
answers the `sms-consent` hook from its consent records.

## Capabilities

| Operation | SMS |
| --- | --- |
| Text, MMS files | yes, within the limits above |
| Delivery receipts | yes (`receiptStatus`; a failed one fails the delivery) |
| Edit, delete, reaction | no (refused before the change: `bridgeUnsupported`) |
| Idempotent sends | no: a lost answer is uncertain |
| Group texts | no: one room per pair of numbers |

## Tests

- `crates/moqui-channels/src/tests.rs`, against a fake Telnyx on loopback (each well under a second): a signed text in,
  the pair room and its members; a replayed, tampered, wrongly keyed, unsigned and stale webhook refused; a duplicate
  callback and the same message under another event; a text to a number that is not ours; a reply out with its headers,
  receipts in order and a carrier's refusal; MMS in (a file fetched, a page kept as bytes, a file that 404s named) and out
  (the signed address served to that bridge only, while it works), over the count and size limits; a lost answer
  uncertain and not sent again, a 503 tried again and taken, a STOP block from the provider, the provider unreachable; a
  STOP and START, international, a user with no number, a promotional text with no consent and the day's limit, all
  before the provider; the pair room shared by a text and a call (`receive#PhoneCall`); a sign-in code through the real
  sender, in no room.
- `crates/moqui-core/src/service/chat_bridge_web.rs`: a file address signed for its bridge that runs out; a file from a
  network never kept as a page.
- `basic-comms/test/BasicCommsTexts.spec.js`: Text a number in the browser and the terminal, the room it opens, the same
  post from a stranger, an international number.

**Not checked live:** Telnyx itself (its signature and webhook shapes are from its documentation), carrier delivery,
and 10DLC registration.
