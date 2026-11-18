# Chat bridges

A chat room can reach outside Moqui: a connector maps it to a room or conversation on another network (SMS, WhatsApp,
Telegram, Slack and others), and a native endpoint (XMPP, Matrix) serves Moqui rooms to their clients. This is the layer
under both: chat actors, the input from a network, and the durable delivery of what goes out. The design is
[plans/archive/Comms-Framework.md](../../plans/archive/Comms-Framework.md) §4.1, §4.3 and §4.4, milestone C1. C1 has no network adapter
of its own: C4-SMS adds the SMS adapters, one for each provider (`sms` on Telnyx, `sms-twilio` on Twilio, not yet tested with Twilio; [SMS](SMS.md)), and business RCS on the same core (`rcs-twilio`, `rcs-sinch`, bridges of network RCS, not yet tested with the provider; [RCS](RCS.md)). The GroupMe adapter (`groupme`, network GroupMe, not yet tested with GroupMe; [GroupMe](GroupMe.md)) is one person's account, which connects out to GroupMe's push channel. The user guide is [Comms](../tools/Comms.md#bridged-rooms).

- **Entities:** the C1 block at the end of `framework/entity/ChatEntities.xml` (`ChatActor`, `ChatExternalUser`,
  `ChatBridge`, `ChatRoomBridge`, `ChatEventExternal`, `ChatBridgeInbound`, `ChatBridgeDelivery`), with
  `ChatRoom.federationEnabled` and `agentsReadExternal`, and the actor keys of `ChatRoomMember`, `ChatEventMention` and
  `ChatPollVote`.
- **Services:** `org.moqui.chat.ChatBridgeServices` (input, delivery state, retry, administration) and the actor parts of
  `org.moqui.chat.ChatServices` (`get#LocalActors`, `get#ActorNames`, `get#RoomBridges`, `check#BridgeSupport`).
- **Rust:** `crates/moqui-core/src/service/chat_bridge.rs` (the adapter contract, the delivery worker, local actors,
  tokens).
- **Screens:** BasicComms Chat (`Chat.qvue` and its terminal twin `ChatTui.xml`); System, Chat Bridges
  (`basic-comms/screen/System/ChatBridges.xml`).

## Actors

Everyone who acts in chat is a `ChatActor`: a local user (`CakLocal`) or a person on a bridge's network (`CakExternal`,
with a `ChatExternalUser`). Exactly one identity is set, unique within its kind.

- A local actor's id is its userId, made on first use (`get#LocalActors`, one create-or-update, so two first uses at once
  both succeed). An external actor's id is sequenced with the prefix `XA`.
- `ChatRoomMember` is keyed by `(chatRoomId, chatActorId)`, `ChatEventMention` by `(chatRoomId, eventSeq, chatActorId)`
  and `ChatPollVote` by `(chatRoomId, eventSeq, chatActorId, optionSeq)`. No key holds a userId.
- The rows of a local actor keep its userId beside the actor (`ChatRoomMember.userId`, `ChatEvent.senderUserId`,
  `ChatEventMention.userId`, `ChatPollVote.userId`), so the services still take and answer userIds for local users, and a
  read by user is one find. An external actor's rows have no userId.
- An external actor has no login and no `UserAccount`. A verified puppet link (`ChatExternalUser.linkedUserId`) is
  provenance only: it gives no right to act as that user.
- Agents stay local service-user actors. An external actor's message reaches a room's agents only when the room's
  `agentsReadExternal` is Y. The framework's own agents (`AgentServices.handle#ChatMessage`) never run for one: a run is
  always its local sender's, and they do not act in a room an external actor is in.

## The adapter contract

A network adapter implements `ChatBridgeAdapter` (moqui-core `service/chat_bridge.rs`):

| Part | What it does |
| --- | --- |
| `name` | The name a `ChatBridge` row selects it by: its `adapterName`, else its network's enumeration code (`sms`, `whatsapp`, `xmpp` and so on). |
| `capabilities` | `edit`, `delete`, `reaction`, `idempotent` (the network dedupes a send by its idempotency key), `sends_per_minute` (0: no limit), `encryption` (below), `group_max` (the most outside numbers of a group text: Telnyx 8, Twilio 9; 0 for none, and a group room then refuses to send, `groupTextsNotSupported`). Each implementation narrows these to the API version it verified. |
| `start`, `stop` | Connect the bridge on this node, or disconnect it. An error marks the bridge not connected, with the error as its last error, and the next round tries again. An adapter that keeps a connection of its own (GroupMe's push channel) starts its node's coordinator here, which holds a lease per bridge so one node of a cluster connects (`moqui.edge.EdgeSenderLock`), and keeps the bridge's status itself (`chat_bridge::note_status`). |
| `keeps_status` | True for an adapter whose own connection's outcome is the bridge's status (GroupMe): a `start` that succeeds then does not mark the bridge connected. The default is false. |
| `deliver` | Send one event (`DeliveryRequest`: the idempotency key, the external room and the network's own conversation of it when the adapter stored one (`external_thread_id`, `store#MappingThread`), the kind, the text and HTML, the sender's name and local user, the message's files (`attachments`, its `ChatAttachment` rows in order), the network id of the message an edit, reaction or delete is about, the reaction key, a rich message's cards and buttons (`rich`, its `content.rich`: an adapter that cannot send them sends the text, their text form), the attempt) and answer `Accepted` (the network's id), `Retry` (with `not_connected`), `Failed` (a reason code) or `Uncertain`. A request with no room is a direct send (`send#BridgeDirect`, below). |
| `webhook` | A request to the bridge's webhook route, `POST /channels/{chatBridgeId}/webhook` (C4-SMS): the raw body, read within its bound, and the headers. The adapter checks the network's signature over those bytes and its replay window before it parses anything, then hands what came to `ingest#Event`. The default answers as an unsigned request (`WebhookResponse::not_signed`). |
| `verify_webhook` | The signature step alone, for a disabled bridge and behind an edge node: whether the request is the network's. It keeps no replay state. The default refuses every request. |
| `webhook_verification` | Where `verify_webhook` runs behind an edge node: `Edge` (the default; with the bridge's replicated copy, a public key) or `Core { signature_header }` (a secret only the local node holds: the edge node checks the header is there and queues). The local node checks every queued request again, either way. |
| `webhook_verifiable` | Whether this node has what `verify_webhook` needs (a key or secret, a configuration that read): a queued request it cannot check waits rather than being refused as forged. |
| `edge_answer` | An edge node's answer to the network for a request it queued (2xx) or refused for its rate (429), in the network's shape (Twilio's TwiML); never an unsigned request's. |
| `webhook_event_id`, `apply_webhook` | For an edge node's queue: the event's id (a repeat is dropped) and what orders it, and the work of `webhook` after its checks, on the local node. |
| `fetch_media` | The bytes of a file a network's message names, within a bound (`store#BridgeMedia`). The default fetches nothing. An adapter that uploads a message's files to its network (GroupMe's image service) reads them with `chat_bridge::delivery_file`, only for a message with a delivery to its bridge. |
| `typing` | Show the other side that a user of ours is typing (`send#BridgeTyping`, which `signal#Typing` starts asynchronously, at most once in 15 seconds a room): business RCS. Nothing is kept or retried; a failure is logged. The default sends nothing. |
| `delete_content` | Delete a content the adapter made at its network for a send (`ChatBridgeDelivery.providerContentId`, kept by `note#DeliveryContent`: a Twilio Content of a rich message), on the message's final receipt or by the delivery worker's daily sweep of those older than 2 days (`clean#ProviderContent`). The default has none. |
| `lookup_number` | What the network's provider knows of an outside number (`NumberLookup`: line type, carrier, caller name), for the spam filter (`lookup#BridgeNumber`, [SMS, Blocked numbers and spam](SMS.md#blocked-numbers-and-spam)); a paid request, asked only when its answer can change a verdict. The default has none (`notSupported`); the SMS adapter asks its provider's `lookup_request` (Twilio Lookup v2). |

The web side is `chat_bridge_web.rs`: `chat_bridge::webhook` (the host's route; an unknown bridge, one with no adapter
and a disabled one to anyone but its network answer as an unsigned request does, 401, so bridge ids cannot be probed; a
disabled one answers its network, by its signature, 503), `replay_take` and `replay_finish` (the webhooks' replay cache:
the distributed `moqui.security.hmac.replay` the system-message HMAC check uses, so a copy that reaches another node is
caught there once that cache is shared; until a distributed cache is configured it is the node's own), `media_url` and `media` (a file of a message that goes out by address, signed under
`channels_media_key` for one bridge and an expiry, served only for a message delivered to that bridge),
`store#BridgeMedia` and `send#BridgeDirect` (one message straight to an adapter outside any room, no delivery row: the
system's sign-in codes by SMS).

A framework crate lists its adapters in `Extension::chat_bridge_adapters` (`moqui_core::extension`);
`chat_bridge::register_adapter` adds one to a factory at run time (the tests' in-memory adapter). Protocol I/O is the
adapter's, above moqui-core: `deliver` runs on the worker's thread and may block.

**SMS providers.** The SMS adapters share one implementation, `crates/moqui-channels/src/sms.rs` (`SmsAdapter`): the
checks before the provider (`check#SmsSend` once a recipient, its reserve let go when the text does not reach the
provider), the bridge's limits and its files' addresses, the guarded client, the answer's classes (2xx accepted, 429 and
503 retry, 401 and 403 retry with the bridge not connected, another 4xx as the provider reads it, 5xx uncertain; a request
whose answer was lost is uncertain, never sent again), and the webhook's POST, replay cache and events (`receive#Sms`,
`note#DeliveryReceipt`, `note#SmsKeyword`). A provider module (`crates/moqui-channels/src/providers/`) implements
`SmsProvider`: its settings, its send request (endpoint, auth, a JSON or form body) and how it reads the answer, its
group maximum, its webhook signature (and whether an edge node can check it), its event key and its events, and its
files' fetch auth. Each provider is one adapter, chosen by `ChatBridge.adapterName` (`sms` is Telnyx's,
`sms-twilio` Twilio's, `sms-sinch` Sinch's Conversation API: [SMS](SMS.md#providers); `rcs-twilio` and `rcs-sinch` the
same providers' business RCS: [RCS](RCS.md)), so an edge node,
which sees no `configJson`, picks it too. A provider that signs the URL it posted to checks it against
`chat_bridge::webhook_public_url`: the bridge's `webhookUrl`, else `channels_public_url` and the request's path and query
as received (`WebhookRequest.url`).

What the SMS core does for every provider, so a provider module does not: a refusal the provider reads as `optedOut`
(Telnyx 40300, Twilio 21610) marks the pair opted out (`note#SmsKeyword` STOP) for a text to one number; an opt-out or
opt-in the provider tells of as an event (`SmsEvent::OptSignal`) is applied once by its event id
(`note#SmsOptSignal`, a done `ChatBridgeInbound` `signal:<id>`); a text in a conversation of the provider's own
(`Inbound.thread_id`) comes to the room of the mapping that keeps it (`ChatRoomBridge.externalThreadId`, stored by the
adapter with `ChatBridgeServices.store#MappingThread` right after it made the conversation; `sms::store_thread`).
A provider value serves every bridge of its adapter on every thread: a cache in it (an OAuth token) is keyed by the
bridge or the credential's id.

Replay, by provider: the replay cache catches a copy of a signature within its window (`moqui.security.hmac.replay`);
past it texts dedupe by their ids (`ingest#Event`), receipts never move a delivery back, and opt signals are applied once
by event id. Telnyx's and Sinch's signatures carry a timestamp, checked within `webhookToleranceSeconds`; Twilio's
carries none, so a captured Twilio request can be sent again after the window, and only those dedupes stop it.

Retries, by provider: the contract is that 503 means "the provider tries again" (a database away, an edge node's queue
not reachable). Telnyx retries a webhook that fails. Twilio does not retry a 5xx by default: a Twilio bridge needs a
fallback URL on the number or messaging service, or the connection overrides on the webhook URL (`#rc=` retry count,
`#rp=` retry policy), or a 503 is a lost event (a status callback, or a text that never reaches the room).

An operation the adapter does not carry is refused before the local change: `edit#Message`, `redact#Message` and
`react#Message` answer `reasonCode` `bridgeUnsupported` for a message that went to (or came from) such a bridge, and the
screens leave the action out (`get#Room` `bridgeCapabilities`). An inbound edit relayed to such a network fails there
with the same reason; it is never shown as synchronized.

`encryption` is how far the network keeps a message from others: `None` (the network and its providers read it: SMS and
MMS, every SMS adapter), `NoneByNetwork` (the same, and the adapter says so of its network: it has no end-to-end
encryption at all; GroupMe), `Transport` (TLS in transit only), `BridgeDecrypted` (end to end on the network, decrypted by
the bridge here: WhatsApp through a bridge) or `EndToEnd` (end to end through the bridge; none yet). An adapter that says
nothing says `None`, and so does a bridge with no adapter in the build. `get#BridgeCapabilities` returns it, each bridge
of `get#RoomBridges` (and so `get#Room`'s `bridges`) has it, and `find#Rooms` gives each room's `bridgeNetworks`
(`{network, networkName, encryption, outbound}`). A bridged room is never end-to-end encrypted on this server, whatever
the network does: its label (`moqui.chat.e2eeLabel`, [Comms-Messaging §6.2](../../plans/Comms-Messaging.md#62-labels))
is the SMS one for a text; for a room whose bridges all say `NoneByNetwork` (GroupMe), "{network} has no end-to-end
encryption, and the bridge reads messages to pass them on"; else "the bridge reads messages to pass them on" (`None`
alone may be an adapter that said nothing). The room
settings name each bridge's encryption.

## Input

An adapter, after its own transport checks (a webhook's signature over the raw body within a bound, a gateway's
session), hands a batch to `ChatBridgeServices.ingest#Event`, which follows the Matrix application-service transaction
contract:

1. **Security first, for the whole batch.** An unknown or disabled bridge, a token that does not match the bridge's
   `inboundTokenHash` (SHA-256, compared in constant time; not asked with `transportVerified`, which only an adapter that
   checked the network's signature itself sets, from its webhook, and which holds only while `chat_bridge::webhook` is
   working that bridge's request, or while the adapter's own authenticated connection to the network is (GroupMe's push
   channel, `sms::with_mark`)), a malformed transaction id, a batch over 200 items or 1 MB, or
   an item that names a local user (`userId`, `senderUserId`, `localUserId`, `chatActorId`, `senderActorId`) refuses the
   batch with its reason code before anything is stored.
2. **Stored before it is acknowledged.** The batch is a `ChatBridgeInbound` row, unique on the bridge and transaction
   id, written in a transaction of its own. A transaction id sent before answers with the outcomes it has so far (an
   item not yet worked is `waiting`) and does nothing again: the first call, or the worker for one that a crash left
   unfinished, finishes it. Outcomes are merged under the batch's row lock, so a second worker of a batch (another
   node's round) never turns a final outcome back into waiting or an accepted item into one rejected by the race it lost.
3. **Each item on its own**, in order, in a transaction of its own (`process#InboundItem`): a malformed item is rejected
   by its `itemId` (`malformed`, `attachmentsUnsupported`, `noMapping`, `notInbound`, `notMember`, `archived`, `notOwn`)
   and the others go on. An item's `mentions` may be `['@room']` only (GroupMe's `@all`): the text's first `@all` or
   `@everyone` becomes the room's mention. A message's files come as `media` (up to 10 addresses): each is fetched by the adapter in the
   item's transaction (`store#BridgeMedia`, a safe content type, no uploader) and sent with the message; one that cannot
   be fetched is named in the message's text and the rest go on. An `attachments` list is still refused.
4. **Mapped once.** An item maps to a local event through `ChatEventExternal`, unique on the bridge, the external room
   and the external event id, written before the event: one seen before is a `duplicate`, and one this server sent there
   (origin outbound) is an `echo`. Both change nothing.
5. **Edits wait for their message.** An edit, reaction or delete whose target has no mapping yet is `waiting`; the batch
   stays `CbiWaiting` and is worked again when later input maps its target, so an edit stays behind its send. One that
   waits more than a day is rejected (`targetMissing`). An edit older (by its `originDate`) than the last one applied is
   `stale`.
6. **The shared logic.** An accepted item goes through `send#Message`, `edit#Message`, `react#Message` or
   `redact#Message` as the external actor (`senderActorId`, authorization disabled only), so unread counts, threads and
   agent dispatch work as for a local event. The external person is a member of the mapped room from their first item
   (notification level none); one a moderator took out is refused (`notMember`).

The delivery worker also works the batches that wait, and those stored but never worked a minute after they came (a
crash), each round (`process#PendingInbound`; a disabled bridge's wait until it is enabled). A batch done keeps only its
items' names and outcomes, not their content, and is removed after 30 days, longer than a provider retries; the
external mappings (`ChatEventExternal`) dedupe its events after that.

## Delivery

- **Written with the event.** `append#Event` writes a `ChatBridgeDelivery` for each active mapping a message, edit,
  reaction or delete goes out on (direction both or out), in the event's transaction, unique on the event and mapping.
  Nothing goes back to the bridge an event came from (`sourceChatBridgeId`, an external actor's only; a local user's
  event goes to every bridge): with echo suppression by the external
  mapping, a relay between two networks does not loop. An edit, reaction or delete goes only where its target went or
  came from; its row depends on the target's delivery (`dependsOnDeliveryId`) for the network's id.
- **A scheduled message** (plans/Comms-Messaging.md phase 15) is an ordinary message of its sender at its time:
  `ChatScheduleServices.send#DueMessages` sends it through `send#Message` as the sender, so its delivery rows are written
  then, and the adapter's checks (`check#SmsSend`: the destination, the opt-out, the day's limits; its capabilities and
  fallback) are those of that moment. Nothing about a bridge is decided when it is scheduled.
- **Woken after the commit.** `wake#BridgeDelivery` wakes this node's worker once the transaction commits. The commit hook
  is in memory and a crash loses it, so the worker (a thread per node, started by the host after the cluster) also runs a
  round at start, and once a minute while any bridge exists.
- **A round** (`chat_bridge::round`): start each active bridge's adapter on this node; work the waiting input; then each
  mapping's oldest pending row, claimed with a conditional update that takes a 60-second lease
  (`leaseOwner`, `leaseExpiresDate`) so one node sends it. An outcome is kept only while the row is still on the attempt
  that worker claimed, so a worker whose lease ran out mid-send does not overwrite the next one's; an acceptance is kept
  over any outcome but another acceptance. A mapping's rows go in order; one failing destination holds up only its own
  mapping.
- **Every attempt carries the row's stable idempotency key** (`moqui-chat:<room>:<seq>:<mapping>`). An attempt whose lease
  ran out with no outcome (a crash, or an adapter that panicked) is sent again to a network that dedupes, and ends
  `CdsUncertain` (`lostInFlight`) on one that cannot. Nothing claims exactly-once delivery.
- **Failures.** A retry waits 5 s, 30 s, 2 min, 10 min, 30 min, then an hour, up to 8 attempts in all, then fails
  (`attempts`). A permanent failure fails at once with its reason. A failed or uncertain row stays visible, with
  `retry#Delivery` for the message's sender, a moderator of the room or a user with `ChatBridgeAdmin` (the same key; an
  uncertain one may then arrive twice).
- **Rate limits.** A node sends at most the bridge's `sendsPerMinute` (else the adapter's) to it a minute; the rest wait
  for a later round.
- **State.** `get#EventDeliveries` gives a message's state (pending, accepted, failed or uncertain, the worst of its
  destinations) for `get#Timeline`, and `push#DeliveryState` pushes each change to the room (`kind: 'update'`,
  `fields.delivery`). A row keeps no message text, and stays after the event's content is purged, as a dedupe tombstone.
- **Status.** A bridge is `CbsConnected` once a send gets through or its adapter starts, `CbsNotConnected` when the
  adapter is missing or will not start or a send reports `not_connected` (its rooms show "not connected" and keep their
  local timeline), and `CbsDisabled` when an administrator disables it (no deliveries, no input).

## Administration

The `ChatBridgeAdmin` permission (the `ADMIN` group has it, `ChatBridgeSetupData.xml`): `find#Bridges` (status, last
error, mapped rooms, the deliveries waiting, failed and uncertain, the latest ones that did not go), `store#Bridge` (the
configuration is encrypted and never returned; the inbound token is kept only as its hash), `store#RoomBridge` (one active
mapping of an external room per bridge; no direct conversation) and `end#RoomBridge`. System, Chat Bridges lists the
bridges and the failed deliveries with Send again, and disables or enables a bridge. A room's mappings are in Chat, Room
settings, Bridges: each with its network, its name there and which way it carries, and for a bridge administrator
(`get#ChatData` `me.bridgeAdmin`) Bridge to another network, Change and End (the Chat transitions `bridgeChoices`,
`storeRoomBridge` and `endRoomBridge`, over the services above, which check the permission again). The terminal names
each mapping with its name there and its direction; it does not change them.

**Behind an edge node** ([plan](../../plans/archive/Edge-Node.md) §5.4). A network's webhook URL is the edge node's,
`/channels/{chatBridgeId}/webhook`; the edge node checks the request with the bridge's replicated copy (its type,
adapter, status and webhook key, never `configJson`) and queues it, and the local node applies it through the adapter's
`apply_webhook` with the same input checks and dedupe. A network that signs with a secret (`webhook_verification`
`Core`) is checked on the local node instead: the edge node checks only the method, the size, that the signature header
is there, a rate per client address and per bridge (`channels_edge_webhooks_per_client_minute`, 120, and
`channels_edge_webhooks_per_minute`, 600), a smaller body (`channels_edge_unverified_max_bytes`, 32 KiB) and how many of
the bridge's items wait (`channels_edge_unverified_waiting`, 500), reads the event key from the unverified body and
queues the request with its path and query, keyed by its signature too. The local node's `apply#EdgeWebhook` checks
every queued request's signature again, whichever way it was queued: one that is not the network's is refused, kept as
a `ChatBridgeWebhookRefused` for 30 days (`replay#RefusedWebhooks`, System › Chat Bridges' Replay refused, after the
secret or `webhookUrl` is fixed), counted there ("N webhooks refused since ...") and logged once per bridge every ten
minutes; one it cannot check (no secret) waits. Files a message sends by address (MMS) are put on the edge node
and served there at `/channels/{chatBridgeId}/media/{token}`. The local node's `channels_public_url` names the edge
node; the Edge Node wizard sets it and lists each bridge's webhook URL ([SMS](SMS.md) has the steps). Networks that
only connect out (GroupMe's push channel; Telegram's `getUpdates`, Slack Socket Mode, the Discord Gateway) need no edge node. On an
all-in-one node ([Run-Deploy](../Run-Deploy.md#an-all-in-one-node)) the webhook and media routes look for the server's
own bridge or file first, then the local node's copy; a bridge id both sides have gets 409, naming it.

With several local nodes on one edge node ([Run-Deploy](../Run-Deploy.md#several-local-nodes-on-one-edge-node)) bridge
ids come from separate databases and may be the same, so a webhook goes by its host name first: at a local node's
terminate host name only that node's bridges answer (the edge node queues them while it is away), and at the edge
node's own names the one local node with the id, where an id two have gets 401, logged. A local node whose ids may
collide sets `channels_public_url` to one of its terminate host names, so its networks' webhooks and its MMS files
name it.

## Tests

- `crates/moqui-core/src/service/chat_bridge_tests.rs`: an in-memory adapter registered only in the tests, with crash and
  failure injection, on a simulated clock: a crash before and after the network took a message (once on a network that
  dedupes, uncertain on one that cannot, then the authorized retry, refused to another member and to a stranger), one
  failing destination while another delivers (in order, backed off, then delivered; a permanent failure visible in the
  admin list and sent again), duplicate batches and echo loops between two networks, reordered edits, a reaction and a
  stale edit, a whole batch refused for security and malformed items rejected by name, an unsupported edit refused before
  the change.
- `crates/moqui-channels/src/tests.rs`: an unknown and a disabled bridge answering as an unsigned request, the
  disabled one its network 503; a webhook's replay held in the shared replay cache; a group room on a bridge whose
  provider carries no group texts refused at the send and at Text a number. Its bed takes the provider under test
  (`Provider`: adapter, settings, the fake's answer, the signer).
- `crates/moqui-core/src/service/chat_bridge_edge_tests.rs`: a queued webhook carrying its path and query; an edge
  node's checks of a webhook it cannot verify (size, signature header, the rate cap); a queued webhook verified on the
  local node over its public URL (`webhookUrl`, else `channels_public_url`), applied when good, refused, kept and counted
  when not, and applied by the replay once the secret is fixed; one with no secret waiting; a bridge whose adapter
  verifies on the edge node checked again; the edge node's whole path for one it cannot verify (no check, keyed by its
  signature, the network's answer shape, the cap on what waits); the rate per client address and per bridge; a
  mapping's thread kept once and carried by its sends.
- `crates/moqui-channels/src/groupme/tests.rs`: the GroupMe adapter against a fake GroupMe ([GroupMe, Tests](GroupMe.md#tests)).
- `crates/moqui-channels/src/tests.rs` also: a provider's opt-out signal kept once per event; a provider's opt-out
  refusal marking the pair; a text in a provider's conversation coming to its room.
- `crates/moqui-host/tests/it/lock_then_insert.rs`: racing first joins make one actor.
- `basic-comms/test/BasicCommsBridges.spec.js`: a bridged room that is not connected in the browser, the terminal and the
  admin screen, and who may retry and administer.

**Not checked live:** no real network. C1 has no network adapter; each one is checked against its provider. C4-SMS's
SMS adapters are checked against a fake provider ([SMS, Tests](SMS.md#tests)), not the provider itself.
