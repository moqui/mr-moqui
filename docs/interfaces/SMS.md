# SMS and MMS

Texts to and from outside numbers, through the provider's messaging API and webhooks, on the chat bridge layer
([Chat bridges](Chat-Bridges.md)). Telnyx is the first provider (D-V4). A number's texts, calls and voicemail land in one
room per outside number. The design is [plans/archive/Comms-Framework.md](../../plans/archive/Comms-Framework.md) §5.6, §5.4 ("One
conversation per outside number"), §4.3 ("Business channels"), milestone C4-SMS. Texting needs no SIP engine: a number
with a messaging bridge and no trunk texts.

- **Rust:** `crates/moqui-channels` (the `channels` feature of the host, in Maximal and Enterprise): the
  provider-neutral adapter and its webhook (`sms.rs`), and each provider's module (`providers/`: Telnyx's `telnyx.rs` is
  the `sms` adapter). `crates/moqui-core/src/service/chat_bridge_web.rs`: the webhook and media
  routes' core (`chat_bridge::webhook`, `media`, `media_url`), `store#BridgeMedia` and `send#BridgeDirect`. The host's
  routes are `crates/moqui-host/src/channels.rs`.
- **Services:** `org.moqui.call.SmsServices` and `Sms.js` (the pair room, the checks before a send, texts that come in),
  `org.moqui.SmsServices.send#SmsMessage` (the system's sender), `ChatBridgeServices` (`ingest#Event` with media,
  `note#DeliveryReceipt`), the SMS parts of `VoiceServices`/`Voice.js` (a number's `messagingBridgeId`, the SMS limits).
- **Entities:** the C4-SMS block of `framework/entity/VoiceEntities.xml`: `SmsOptOut`, `SmsSendCount`, the SMS fields of
  `VoicePolicy`, a delivery's receipt on `ChatBridgeDelivery`; `ChatRoom` type `CrtExternal`.
- **Screens:** System, Chat Bridges (the bridge), System, Voice: Numbers (Texts through) and Fraud (the SMS limits, the
  system's number); Calls History's Text a number, and a number typed in Chat's new-chat pickers; the pair's room in
  Chat (its SMS icon and bridge chip).

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
   | `maxInboundMediaBytes` | 5 MiB (at most 20 MiB), the largest file taken from a text that comes in: each fetch reads no further |
   | `webhookToleranceSeconds` | 300, at most half the replay cache's live time (150 under its default 300 s), as a timestamp ahead of now is taken until the window past it |
   | `sendsPerMinute` | 60 (the bridge's own field wins); Telnyx's long-code rate is lower per number, so set it to what the account allows |
   | `allowInternalAddresses` | false; only a test's fake provider needs it |

4. **The numbers.** System, Voice, Numbers: Texts through the bridge. A number's users (the user it rings, a ring group's
   members, its caller-ID grants) are the members of its pair rooms; only users granted the number (Caller IDs, or a ring
   group's number) send from it.
5. **The server.** `channels_webhooks_enabled=true`; `channels_public_url` the https base the provider reaches this server
   at (an MMS that goes out is refused, `mediaUnreachable`, without it); System, Voice, Fraud: the SMS limits (people's
   texts a day each and in all; the system's own texts a day; promotional texts a day per number) and the system's
   number (sign-in codes).

**Behind an edge node** ([Run-Deploy](../Run-Deploy.md#a-local-server-with-an-edge-node), [plan](../../plans/archive/Edge-Node.md)
§5.4). A local server that providers cannot reach takes its texts through its edge node:
- The webhook URL in the portal is the edge node's: `https://<edge node>/channels/<chatBridgeId>/webhook`. The edge node
  checks the signature with the bridge's public key (replicated to it; the API key stays on the local node), answers 200
  once the text is queued, and the local node takes it when its link is up, once, without checking it again.
- `channels_webhooks_enabled=true` is on the edge node (`conf/edge/MoquiEdgeConf.xml`). On the local node `channels_public_url` is
  the edge node's address; the Edge Node wizard sets it.
- MMS. Each file of a text that goes out is put on the edge node before the send, and the provider fetches it from
  there (`/channels/<chatBridgeId>/media/<token>`, for `channels_media_minutes`). While the link is down the text waits.
  Files of texts that come in are fetched by the local node from the provider, as without an edge node.
- On an all-in-one node ([Run-Deploy](../Run-Deploy.md#an-all-in-one-node)) the server's own bridges and numbers and
  the local node's share the webhook route; each id and number belongs to one side, and the server refuses one the
  other side has.
- With several local nodes on one edge node ([Run-Deploy](../Run-Deploy.md#several-local-nodes-on-one-edge-node)) a
  number belongs to the first local node whose copies have it (another's record for it is kept as a conflict), and
  bridge ids may be the same on two: a local node whose ids may collide sets `channels_public_url` to one of its
  terminate host names and gives that address in the portal ([Chat-Bridges](Chat-Bridges.md#administration)).

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
  `open#PairRoom` (the same checks as a send) opens the room in Chat. Chat's new-chat pickers (New direct message, the
  phone's Start chat) do the same with a number typed in their search (`searchUsers` with `numbers=true`, then
  `textRoom`); several numbers picked start a group text (below), and a number with people picked says a text goes to
  numbers only. In the mobile app the pickers also find the phone's own contacts as the user types.
- **Names.** A phone room is named by its numbers. The page shows a name for each number it can find: the phone's own
  contacts in the mobile app first (read on the phone, never sent to the server), then the server's: `find#PhoneNames`
  (Chat's `phoneNames`, at most 50 numbers a request and 600 an hour a user) asks the apps' `call-context` providers,
  as the user, for each number's first contact's name, and only of numbers of the user's own phone rooms. The page asks
  the server for all of a room list's numbers, not only those the phone does not know, so the server cannot tell
  which numbers are in the phone's contacts. With no app that gives one, the numbers stay as they are; a contacts
  permission given later names the rooms again without a reload.

## Group texts

A group MMS is one room for its set of outside numbers: `CrtExternal`, named the numbers (`+1801..., +1385...`), its
`externalRoomId` `ours/<a>,<b>,...` with the others' numbers in E.164, unique and sorted (`Sms.js` `groupKey`), so the
same people make the same room whoever writes first. A group is our number and at most 8 others: a send has 8
recipients at most, and a group text that came in is its sender plus at most 7 others (the adapter's `group_max`,
`get#BridgeCapabilities` `groupMax`; Telnyx's limit is the carrier's, about 10 in all); a send to more is refused
(`tooManyNumbers`). On a bridge whose provider carries no group texts (`groupMax` 0) Text a number with several numbers
is refused and a group room refuses to send (`groupTextsNotSupported`), and a group text that comes in is the sender's
pair room. Our own numbers are never a group's
others: a group that has two of ours is ours, and a group MMS never goes to one of our numbers.

- **In.** A `message.received` with more than one recipient (Telnyx's `to` and `cc`, `providers/telnyx.rs` `recipients_of`, at
  most 32 read) passes `recipientList` to `receive#Sms`, which finds our number among them (one of this bridge's) and
  the group's room of the sender and the other recipients: the sender is always kept, then the first 7 others in
  order. A sender that is not a number in E.164 (a short code) keeps a room of its own, never another person's pair
  room. A text to one number is the pair's room, as before.
- **New groups are limited.** A new group room is made for at most 5 groups a day from one outside number and 50 a day
  for one of our numbers (`SmsSendCount` `group:<ours>:<sender>` and `groups:<ours>`); past that a group text goes to
  the sender's pair room, with a line that says it was a group text kept there, and no new room reaches the number's
  users. A group whose room is there already still takes its texts.
- **Out.** A reply in a group room goes as one group MMS (`POST /v2/messages/group_mms` with `to` as the list and no
  `messaging_profile_id`, which that endpoint does not take) from our number. Each number is checked as a single text
  is (`check#SmsSend`: opt-out, the destination's class, emergency) and counts as one text toward the day's limits
  (the carrier bills a group MMS once a recipient); one refused (a STOP from one member, for one) refuses the whole
  send before the provider and takes the counts back.
- **Receipts.** Telnyx sends one receipt a recipient, each naming the group's message (`group_message_id`, the
  delivery's remote id) and its recipient. Each recipient's is kept (`ChatBridgeDelivery.recipientReceipts`), and the
  message's Info lists them by number. The delivery's receipt is theirs together (the least of those not failed); one
  failed recipient does not fail the delivery while another got it, and the delivery fails only when every recipient
  failed.
- **Start one.** Text a number with several numbers (`open#PairRoom` with `toNumber` comma-separated, each checked as
  a send), or several numbers picked in a new-chat picker.

## Texts that come in

`POST /channels/{chatBridgeId}/webhook` (off unless `channels_webhooks_enabled`):

1. The host reads the raw body first, at most `channels_webhook_max_bytes` (256 KiB; a larger declared or sent body is
   413), and hands it with the headers to the bridge's adapter on a request worker. An unknown bridge (or an id that is
   not one), one this build has no adapter for, and a disabled one to anyone but its provider get the answer of an
   unsigned request (401 `not signed`, counted for web protection), so bridge ids cannot be probed. A disabled bridge
   checks the signature first and answers its provider 503, so the provider tries again once it is enabled.
2. The adapter checks `telnyx-signature-ed25519`, the Ed25519 signature of `{telnyx-timestamp}|{raw body}` under the
   bridge's `publicKey` (aws-lc-rs, the crypto provider rustls already links), before it parses anything. A missing or
   wrong signature, or one whose timestamp is more than `webhookToleranceSeconds` from now, is the one 401 above and
   counts for web protection as a refused login. A signature already worked within the window is answered 200
   `duplicate` and not worked again, one still being worked on another worker 409; neither counts for web protection
   (the signature is the provider's, and counting it could block the provider's address). A signature is held from
   when its request is taken and let go when it was not worked (not 2xx), so the provider's retry of one that failed
   goes through. The cache is the distributed one the system-message HMAC check uses (`moqui.security.hmac.replay`,
   `chat_bridge::replay_take`), so a copy that reaches another node of a cluster is caught there once that cache is
   shared (it stays on its node until a distributed cache is configured, as for the HMAC check); the ids below dedupe
   on any node. A request that cannot be checked against it (the cache missing) is 503 and not worked.
3. `message.received`: `SmsServices.receive#Sms` checks that our number's messaging bridge is this one (else 200,
   `ignored`), keeps a STOP, START or HELP (below), finds or makes the pair room, and hands the text to `ingest#Event`
   (`transportVerified`: no inbound token, honored only inside that bridge's webhook, which `chat_bridge::webhook` marks
   on the context; a caller passing the parameter alone gets `badToken`) as transaction the webhook's event id, item the message id. A webhook sent
   again is a `duplicate` and does nothing; so is the same message under a new event id (`ChatEventExternal`). Its files
   (`media`) are fetched by the adapter, each on its own, within `maxInboundMediaBytes`, and kept as `ChatAttachment`
   rows of the room with no uploader (`store#BridgeMedia`): a type outside images, audio, video, vCard, plain text and
   PDF is kept as `application/octet-stream`, so nothing from a network is served as a page. A file that cannot be
   fetched is named in the message (`[A file not received: image/png]`) and the rest go on. The first text of a new
   outside number is filed on a record by the apps' `external-conversation` providers (`file#ExternalConversation`, as
   the room's first local member: IronStone files it on the one party with that number); until one filed it
   (`ChatRoomBridge.filedDate`), each new text tries again, and a failure is logged and the text kept.
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
| `consentRequired` | A promotional text (`send#SmsMessage` `isPromotional`) with no `sms-consent` provider saying yes, or one saying no or failing: an app root's (asked as the sender, of the apps they may open) or one a component registers in its conf (asked for every sender, the system included; [Comms](../tools/Comms.md)) |
| `dailyLimit` | The sender's (`smsMaxPerUserPerDay`, 200) or all people's (`smsMaxPerDay`, 2000) texts of the day, counted in `SmsSendCount`; the system's own texts (sign-in codes) have a budget of their own (`smsSystemMaxPerDay`, 500, counted apart), so people's texts cannot use up what sign-in codes need. The check and the count are one step (`reserve`) under the lock of the day's count (all people's, or the system's), so sends at once cannot pass a limit together; a text that does not reach the provider (refused, retried, unreachable) is taken off again (`release#SmsSend`), and one whose answer was lost still counts. A promotional text also counts for its number (`smsMaxPromotionalPerNumber`, 1000, counted as `promo:<number>`); one with no user (a campaign's) counts there alone, not against the system's budget, so a campaign never uses up what sign-in codes need |
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
(its error 40300, kept as `optedOut`; the pair's `SmsOptOut` is then marked too, so the next text is refused before the
provider). A provider that tells of an opt-out or opt-in itself (an event, not a text) is kept the same way, once per
event (`note#SmsOptSignal`). The opt-out also gates the system's texts.

## The system's texts

`org.moqui.SmsServices.send#SmsMessage` keeps the interface of the Java stub (`countryCode`, `areaCode`, `contactNumber`,
`message`, `isPromotional`; `successful`, `messageId`, `errorMessage`) with `fromNumber` and `userId` added, and is
internal (`allow-remote="false"`). From `fromNumber`, else `VoicePolicy.smsSystemNumber`, it goes straight to the
provider (`send#BridgeDirect`: no room, no delivery row, no retry), so a sign-in code (`UserServices.send#AuthcCodeSms`)
is seen by nobody else. The checks are `check#SmsSend`'s, with the system's own day limit (`smsSystemMaxPerDay`) when
`userId` is empty: the people's texts do not count toward it, nor it toward theirs. A promotional text with no user
counts for its number's day of promotional texts instead (`smsMaxPromotionalPerNumber`). A refusal or a failure is an
error with its `reasonCode`. Marketing's texts (Mantle) go through it with `isPromotional`, sent as the system from the
sending profile's number, and Mantle answers the `sms-consent` hook from its consent records, registered in its
`MoquiConf.xml` so the system's sends are asked too.

## Capabilities

| Operation | SMS |
| --- | --- |
| Text, MMS files | yes, within the limits above |
| Delivery receipts | yes (`receiptStatus`; a failed one fails the delivery) |
| Edit, delete, reaction | no (refused before the change: `bridgeUnsupported`) |
| Idempotent sends | no: a lost answer is uncertain |
| Group texts | yes: group MMS, one room for a set of outside numbers (8 recipients at most, plus our number), receipts per recipient |

## Providers

### Twilio

**Not yet tested with Twilio.** The `sms-twilio` adapter (`crates/moqui-channels/src/providers/twilio.rs`, group texts
in `twilio/group.rs`) is built from Twilio's documentation and checked against a fake Twilio only; have it tested live
before relying on it. What is above holds for it too (the checks before a send, the pair and group rooms, opt-out,
the limits); this section is what differs.

1. **The account.** In the Twilio console: a phone number (a US or Canadian long code for group texts), an API key
   (`SK...` and its secret; or the account's Auth Token for the API too), and the account's primary Auth Token, which
   signs every webhook. For US texting the number needs A2P 10DLC registration through Twilio (or a verified toll-free
   number); Moqui does not register or check it (Twilio's 30034 and 30032 are kept as `notRegistered`).
2. **A Messaging Service** (optional): a text then goes from the service (`MessagingServiceSid`; its sender pool picks
   the number) instead of `From` our number. Leave the service's own status callback empty: each send names the
   bridge's webhook as its `StatusCallback`.
3. **The webhook URL.** On the number (or the Messaging Service's Integration), "A message comes in": `POST
   https://<server>/channels/<chatBridgeId>/webhook`, exactly as Moqui sees it: `channels_public_url` and that path,
   or the bridge's `webhookUrl` when Twilio reaches the server at another address (a proxy). Twilio signs the URL it
   posts to, so a different scheme, host, path or query fails the signature (the default port may be named or not:
   both are tried). Twilio does not try a webhook again after a 5xx by default (only a connection that could not be
   made, once): set the number's "Primary handler fails" fallback URL to the same URL, or add a connection override
   to the URL (`#rc=3&rp=5xx,ct`; Twilio does not sign the fragment, and Moqui leaves it out of the check), so a
   text that came while the server was busy is not lost ([Chat bridges](Chat-Bridges.md#the-adapter-contract),
   retries by provider).
4. **The bridge.** System, Chat Bridges, network SMS, adapter `sms-twilio`, configuration (kept encrypted):

   | Key | What |
   | --- | --- |
   | `accountSid` | The account, `AC...`, required |
   | `authToken` | The primary Auth Token, required: the webhooks' signature key (and the API's auth with no API key) |
   | `apiKeySid`, `apiKeySecret` | The API key the API calls and file fetches use (Basic auth), both or neither |
   | `messagingServiceSid` | `MG...`: texts go from the service instead of from our number |
   | `conversationServiceSid` | `IS...`: the Conversations service group texts are kept in; else the account's default service |
   | `webhookUrl` | The URL Twilio posts to, when it is not `channels_public_url` and the route's path |
   | `apiBaseUrl` | `https://api.twilio.com` |
   | `conversationsBaseUrl`, `mediaContentBaseUrl` | `https://conversations.twilio.com`, `https://mcs.us1.twilio.com` (a test's fake) |
   | `maxTextLength` | 1600 characters |
   | `maxMediaCount`, `maxMediaBytes` | 10 files and 5 MB in all for an MMS that goes out |
   | `maxInboundMediaBytes`, `sendsPerMinute`, `allowInternalAddresses` | As for Telnyx |

**Out.** `POST /2010-04-01/Accounts/{AccountSid}/Messages.json`, form-encoded, Basic auth: `To`, `From` or
`MessagingServiceSid`, `Body`, a `MediaUrl` for each file (the file's signed address), `StatusCallback`. Twilio's `sid`
is the delivery's remote id. Twilio has no idempotency key for a send, so an answer lost in flight is uncertain and
never sent again on its own. Its error codes:

| Code | The delivery |
| --- | --- |
| 21610 (the person sent STOP) | `optedOut`, and the pair is marked opted out (`note#SmsKeyword` STOP), so the next text is refused before Twilio |
| 21211, 21614 | `badNumber` |
| 21408 (the region is not enabled for the account) | `internationalNotEnabled` |
| 30007 | `filtered` |
| 30032, 30034 | `notRegistered` |
| 20429, 21611, HTTP 429 | tried again after the backoff |
| another | `providerRefused`, with Twilio's message |

**In.** Every webhook is a form post signed in `X-Twilio-Signature`: Base64 HMAC-SHA1 under the Auth Token of the URL
and each parameter's name and value, sorted by name (case-sensitive), with no separators; a JSON body is signed by its
SHA-256 in the URL's `bodySHA256` and the URL alone. It is compared in constant time. The token is a secret, so behind
an edge node the signature is checked on the local node ([Behind an edge node](#setup)); the edge node only queues.
The signature has no time in it: a copy is answered `duplicate` while the replay cache holds it, and after that the ids
make it a no-op (an inbound `MessageSid` is taken once, a receipt never moves a delivery back). With no `authToken` on
the local node a queued request waits rather than being refused. Each answer of 2xx, the edge node's too, is empty
TwiML (`<Response/>`, `text/xml`), so Twilio sends no reply of its own making.

- An inbound text (`MessageSid`, `From`, `To`, `Body`, `NumMedia` and `MediaUrl{N}`, `MediaContentType{N}`) goes to
  `receive#Sms` as Telnyx's does. Its files are fetched with the API's Basic auth (Twilio's media URLs need it); a
  redirect to another host (Twilio's file store) never gets the `Authorization` header.
- A status callback (`MessageStatus`, `ErrorCode`), one a change and in no set order: `sent` is sent, `delivered` and
  `read` delivered, `undelivered`, `failed` and `canceled` failed with Twilio's code (`Twilio error 30003: the phone is
  unreachable`); `queued`, `accepted`, `scheduled` and `sending` are before the carrier and left. A late `sent` never
  takes a delivered text back.

**Opt-out.** With Twilio's Advanced Opt-Out on (the default on a Messaging Service), Twilio answers STOP, START and
HELP itself, blocks sends to a number that sent STOP (21610, above), and still posts the keyword's text with
`OptOutType` (`STOP`, `START`, `HELP`). Moqui takes the opt-out from that inbound text only (once, by its
`MessageSid`), by its own keywords as for Telnyx; the text is still a message in the room. A keyword the account added
in Twilio that Moqui does not know is a text to Moqui, and Twilio's block of the next send (21610) marks the pair then.
Turn Twilio's own replies off in the console only if the organization answers keywords another way.

**Group texts** go through Twilio's Conversations API: its Messages API has no group MMS. A group is our number and at
most 9 others (a Twilio group Conversation has at most 10 participants), all US or Canadian (+1) long codes; another
is refused (`groupTextsNotSupported`). A group room's first send makes its Conversation: looked up by a unique name of
the bridge and the room's numbers first (so a send after a lost answer finds the one made before, never a second), else
made with that name, and its SID kept on the room's mapping at once (`ChatRoomBridge.externalThreadId`,
`store#MappingThread`), so a later try carries it; then our number joins as the chat participant
(`MessagingBinding.ProjectedAddress`), each other number as an SMS participant (`MessagingBinding.Address`; a later try
adds only those the Conversation lacks; when Twilio answers that another Conversation has these numbers, 50438, as one
it made for a group text that came in, that one is found by its participants and kept instead); and a webhook of the
Conversation names the bridge's webhook URL for `onMessageAdded` and `onDeliveryUpdated`. Nothing reaches a phone until the message is posted
(`POST /v1/Conversations/{CH}/Messages`, `Author` our number), so a step before it that fails is tried again; an answer
lost on the post itself is uncertain. A group text carries words only: Twilio takes a Conversation's files only as
uploads to its Media Content Service, which the adapter does not do yet, so a group text with files is refused
(`groupMediaNotSupported`). In: `onMessageAdded` is a text of the Conversation (`Author` the sender) in the room whose
mapping keeps it (`Inbound.thread_id`), else the group room of the Conversation's numbers; `onDeliveryUpdated` is one recipient's receipt (`ParticipantSid`, `Status`, `ErrorCode`),
kept per recipient as Telnyx's are. A Conversation's participants are read from Twilio once a process. A group that
someone else starts reaches Moqui only when the number's Messaging Service hands inbound messages to Conversations
(autocreation); set the Conversations service's webhook (post-event, `onMessageAdded`, `onDeliveryUpdated`) to the
bridge's webhook URL too. Turning that on moves every inbound text of the service into Conversations, so do it only
for a number kept for group texts.

**Not confirmed in Twilio's documentation** (check them in the live test): the HTTP status Twilio answers for a
Conversation or participant that is there already (taken as 409, or the codes 50416, 50433 and 50437); the `Media` parameter of `onMessageAdded` and its
files' address (`/v1/Services/{IS}/Media/{ME}/Content` on the Media Content Service); the codes 30032 and 30034.

### Sinch

**Not yet tested with Sinch**: the adapter (`providers/sinch.rs`, adapter name `sms-sinch`) follows Sinch's
documentation of the Conversation API ([API reference](https://developers.sinch.com/docs/conversation/api-reference/),
[callbacks](https://developers.sinch.com/docs/conversation/callbacks),
[MMS](https://developers.sinch.com/docs/conversation/channel-support/mms)), not the end-of-sale XMS `/batches` API.

1. **In the Sinch dashboard**, in one project:
   - a Conversation API app, in the region its data stays in (US, EU or BR); note the project id and the app id;
   - the app's **SMS** channel (a service plan with our numbers), and for texts with files its **MMS** channel (a Sinch
     MMS account, its sender the same number: plain-text replies to an MMS come on the SMS channel, so set up both);
   - an access key (key id and secret) of the project;
   - a webhook on the app to `https://<server>/channels/<chatBridgeId>/webhook` with a secret, its triggers
     `MESSAGE_INBOUND`, `MESSAGE_DELIVERY`, `OPT_IN` and `OPT_OUT`. The delivery reports come through it, signed with
     its secret: a send names no `callback_url`, whose reports Sinch would sign with another secret (the app's
     `secret_for_overridden_callback_urls`) and not send again once refused.
2. **The bridge**: network SMS, adapter name `sms-sinch`, configuration (besides the common keys above, whose defaults
   here are `maxMediaCount` 1, `maxMediaBytes` 1 MiB, `maxTextLength` 1600):

   | Key | What |
   | --- | --- |
   | `projectId`, `appId` | The project and the app, required |
   | `keyId`, `keySecret` | The access key, required: an OAuth2 token from `https://auth.sinch.com/oauth2/token`, kept until a minute before it runs out (an hour) |
   | `region` | `us` (default), `eu` or `br`, where the app was made; another is refused when the bridge starts, and an app in another region answers 404 (`wrongRegion`, the texts held) |
   | `webhookSecret` | The webhook's secret, required; behind an edge node, a request queued while the local node has none waits for it |
   | `smsSender` | Our number on the bridge, for an opt-out or opt-in Sinch reports (they name no number of ours) |
   | `mmsSender` | The MMS channel's sender; a text with a file from another number, or with no `mmsSender`, is refused (`mmsNotSetUp`) |

- **Out.** One text is one `messages:send` from the room's number: words on the SMS channel (`SMS_SENDER`), a file
  on the MMS channel (`MMS_SENDER`; words and a file as one `card_message`, its `description` the words). A text has
  at most one file (`tooManyFiles`: Sinch sends one a message). `correlation_id` is the delivery's id. Sinch dedupes
  nothing, so a lost answer is uncertain. A 401 drops the token and sends once more with a new one. What the bridge's
  setup stops (a second 401, a 403: `billingLimit` for the project's credit limit, else `providerRefused`; a 404:
  `wrongRegion`) holds the texts, tried again with the bridge not connected and its last error saying which, until an
  administrator fixes it; 429 and 503 are tried again; another 4xx fails (`providerRefused`).
- **In.** The signature is checked on the server that holds the secret (behind an edge node, the local node:
  [Behind an edge node](#setup)). Only the SMS and MMS channels are texts: a message on another channel of the app
  (Messenger, WhatsApp) is left. Sinch keeps a text's files 7 days, and they are fetched as the text comes in.
  `DELIVERED` and `READ` are delivered, `FAILED` fails the delivery with Sinch's reason; `QUEUED_ON_CHANNEL` and
  `SWITCHING_CHANNEL` are left.
- **Opt-out.** `OPT_IN` and `OPT_OUT` do not fire for a STOP texted on SMS: a STOP comes as a text and goes through
  the keyword rule ([Opt-out](#opt-out)), as with any provider. An `OPT_OUT` or `OPT_IN` Sinch took (consent
  management) is a STOP or START from their number to `smsSender`, applied once by its `request_id`.
- **Known limit: an MMS with several files.** Each file comes as its own event, so each is its own message in the
  room, and a caption sent with them is repeated on each.
- **No group texts**: the Conversation API documents none, so the bridge's `groupMax` is 0 and a group room on it
  refuses to send (`groupTextsNotSupported`).
- **Tests**: `crates/moqui-channels/src/tests/sinch.rs`, against a fake Sinch on loopback: the token fetched once,
  kept, refreshed and dropped on a 401; the SMS and MMS channels and their refusals; Sinch's answers by class; texts,
  files, reports, opt-outs and opt-ins in, another channel's message left; the signature good, bad, missing, old,
  ahead, not base64, of another algorithm and replayed; no group texts; a webhook an edge node queued checked on the
  local node (a forged one refused and kept, a good one handed to the adapter, one with no secret here waiting). No
  test takes a queued Sinch text into its room: the in-memory test database cannot store `ingest#Event`'s batch
  inside the queue's transaction.

## Tests

- `crates/moqui-channels/src/tests.rs`, against a fake Telnyx on loopback (each well under a second): a signed text in,
  the pair room and its members; a replayed, tampered, wrongly keyed, unsigned and stale webhook refused, each with the
  one answer, and the replay held in the shared replay cache (another adapter sees it); an unknown bridge and a
  disabled one answering an unsigned and a signed request as an unsigned request, the disabled one its provider 503; a duplicate
  callback and the same message under another event; a text to a number that is not ours; a reply out with its headers,
  receipts in order and a carrier's refusal; MMS in (a file fetched, a page kept as bytes, a file that 404s named, a file
  over `maxInboundMediaBytes` refused at the fetch) and out
  (the signed address served to that bridge only, while it works), over the count and size limits; a lost answer
  uncertain and not sent again, a 503 tried again and taken, a STOP block from the provider, the provider unreachable; a
  STOP and START, international, a user with no number, a promotional text with no consent, the people's day's limit
  reached with a sign-in code still sent, and the system's own day's limit, all before the provider; a campaign's
  promotional text with no user asked of a conf's `sms-consent` provider (a yes sent, a no and no answer refused, a
  conf's other hook left out) and counted for its number's day of promotional texts, a sign-in code still sent after it; the pair room shared by a text and a call (`receive#PhoneCall`); a sign-in code through the real
  sender, in no room.
- `crates/moqui-channels/src/tests.rs` also has group texts: one room for its numbers whoever writes first, the reply
  out as one group MMS to them all (no messaging profile in its body, counted once a recipient); each recipient's
  receipt, one failed not failing the delivery, all failed failing it; a STOP from one member refusing the whole send
  before the provider; twelve recipients keeping the sender and seven others, two of our numbers both ours, a short
  code in a room of its own, and a sixth new group of a day from one number kept in its pair room.
- `crates/moqui-channels/src/tests/twilio.rs`, against a fake Twilio on the same bed: the bridge's required
  settings; a reply out (form, Basic auth, `StatusCallback`) and its status callbacks out of order; a Messaging
  Service's send; 21211, 20429 and 429, a lost answer, and 21610 marking the pair; a STOP taken once; files fetched
  with Basic auth, kept on a same-host redirect and never sent to another host; the signature (good, wrong token,
  changed parameter, missing, another URL, a replay, a query string, the default port either way, a fragment, a JSON
  body's `bodySHA256`, the bridge's `webhookUrl`); the edge node's event keys; a queued request verified on the local
  node, a forged one refused and kept, and one waiting without an `authToken`; and group texts through a fake
  Conversations API (made once with its participants and webhook, then only posts; each recipient's receipt; a reply
  into the room; files refused; a lost post uncertain; a lost create found by its unique name; a Conversation Twilio
  made, its participants read and its files fetched; Twilio's 50438 taking the one it has).
- `basic-comms/test/BasicCommsTexts.spec.js` also has a group text opened in any order and with our own number left
  out, more than 8 refused, and `phoneNames` from a `call-context` provider of the component's own test app root
  (`test/screen/TextNamesApp.xml`) for the user's own phone rooms only.
- `crates/moqui-core/src/service/chat_bridge_web.rs`: a file address signed for its bridge that runs out; a file from a
  network never kept as a page.
- `basic-comms/test/BasicCommsTexts.spec.js`: Text a number in the browser and the terminal, the room it opens, the same
  post from a stranger, an international number.

**Not checked live:** Telnyx and Twilio themselves (their signatures, APIs and webhook shapes are from their
documentation; Twilio's Conversations group texts too), carrier delivery,
group MMS between real phones, and 10DLC registration.
