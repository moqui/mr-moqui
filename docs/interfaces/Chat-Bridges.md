# Chat bridges

A chat room can reach outside Moqui: a connector maps it to a room or conversation on another network (SMS, WhatsApp,
Telegram, Slack and others), and a native endpoint (XMPP, Matrix) serves Moqui rooms to their clients. This is the layer
under both: chat actors, the input from a network, and the durable delivery of what goes out. The design is
[plans/Comms-Framework.md](../../plans/Comms-Framework.md) §4.1, §4.3 and §4.4, milestone C1. C1 has no network adapter
of its own: C4-SMS adds the first, `sms` on Telnyx ([SMS](SMS.md)). The user guide is [Comms](../tools/Comms.md#bridged-rooms).

- **Entities:** the C1 block at the end of `framework/entity/ChatEntities.xml` (`ChatActor`, `ChatExternalUser`,
  `ChatBridge`, `ChatRoomBridge`, `ChatEventExternal`, `ChatBridgeInbound`, `ChatBridgeDelivery`), with
  `ChatRoom.federationEnabled` and `agentsReadExternal`, and the actor keys of `ChatRoomMember`, `ChatEventMention` and
  `ChatPollVote`.
- **Services:** `org.moqui.impl.ChatBridgeServices` (input, delivery state, retry, administration) and the actor parts of
  `org.moqui.impl.ChatServices` (`get#LocalActors`, `get#ActorNames`, `get#RoomBridges`, `check#BridgeSupport`,
  `migrate#ChatActors`).
- **Rust:** `crates/moqui-core/src/service/chat_bridge.rs` (the adapter contract, the delivery worker, local actors,
  tokens), `chat_migrate.rs` (the migration) and `EntityFacade::table_primary_key` and `rekey_table` in
  `crates/moqui-entity/src/entity/meta.rs`.
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

Moving a database from before C1 is [Run-Deploy, Chat actors](../Run-Deploy.md#chat-actors).

## The adapter contract

A network adapter implements `ChatBridgeAdapter` (moqui-core `service/chat_bridge.rs`):

| Part | What it does |
| --- | --- |
| `name` | The name a `ChatBridge` row selects it by: its `adapterName`, else its network's enumeration code (`sms`, `whatsapp`, `xmpp` and so on). |
| `capabilities` | `edit`, `delete`, `reaction`, `idempotent` (the network dedupes a send by its idempotency key), `sends_per_minute` (0: no limit). Each implementation narrows these to the API version it verified. |
| `start`, `stop` | Connect the bridge on this node, or disconnect it. An error marks the bridge not connected, with the error as its last error, and the next round tries again. |
| `deliver` | Send one event (`DeliveryRequest`: the idempotency key, the external room, the kind, the text and HTML, the sender's name and local user, the message's files (`attachments`, its `ChatAttachment` rows in order), the network id of the message an edit, reaction or delete is about, the reaction key, the attempt) and answer `Accepted` (the network's id), `Retry` (with `not_connected`), `Failed` (a reason code) or `Uncertain`. A request with no room is a direct send (`send#BridgeDirect`, below). |
| `webhook` | A request to the bridge's webhook route, `POST /channels/{chatBridgeId}/webhook` (C4-SMS): the raw body, read within its bound, and the headers. The adapter checks the network's signature over those bytes and its replay window before it parses anything, then hands what came to `ingest#Event`. The default answers as an unsigned request (`WebhookResponse::not_signed`). |
| `verify_webhook` | The signature step alone, for a disabled bridge: whether the request is the network's. The default refuses every request. |
| `fetch_media` | The bytes of a file a network's message names, within a bound (`store#BridgeMedia`). The default fetches nothing. |

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

An operation the adapter does not carry is refused before the local change: `edit#Message`, `redact#Message` and
`react#Message` answer `reasonCode` `bridgeUnsupported` for a message that went to (or came from) such a bridge, and the
screens leave the action out (`get#Room` `bridgeCapabilities`). An inbound edit relayed to such a network fails there
with the same reason; it is never shown as synchronized.

## Input

An adapter, after its own transport checks (a webhook's signature over the raw body within a bound, a gateway's
session), hands a batch to `ChatBridgeServices.ingest#Event`, which follows the Matrix application-service transaction
contract:

1. **Security first, for the whole batch.** An unknown or disabled bridge, a token that does not match the bridge's
   `inboundTokenHash` (SHA-256, compared in constant time; not asked with `transportVerified`, which only an adapter that
   checked the network's signature itself sets, from its webhook, and which holds only while `chat_bridge::webhook` is
   working that bridge's request), a malformed transaction id, a batch over 200 items or 1 MB, or
   an item that names a local user (`userId`, `senderUserId`, `localUserId`, `chatActorId`, `senderActorId`) refuses the
   batch with its reason code before anything is stored.
2. **Stored before it is acknowledged.** The batch is a `ChatBridgeInbound` row, unique on the bridge and transaction
   id, written in a transaction of its own. A transaction id sent before answers with the outcomes it has so far (an
   item not yet worked is `waiting`) and does nothing again: the first call, or the worker for one that a crash left
   unfinished, finishes it. Outcomes are merged under the batch's row lock, so a second worker of a batch (another
   node's round) never turns a final outcome back into waiting or an accepted item into one rejected by the race it lost.
3. **Each item on its own**, in order, in a transaction of its own (`process#InboundItem`): a malformed item is rejected
   by its `itemId` (`malformed`, `attachmentsUnsupported`, `noMapping`, `notInbound`, `notMember`, `archived`, `notOwn`)
   and the others go on. A message's files come as `media` (up to 10 addresses): each is fetched by the adapter in the
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
bridges and the failed deliveries with Send again, and disables or enables a bridge.

While the copies the chat actors migration kept are there (`CHAT_ROOM_MEMBER_PRE_C1`, `CHAT_EVENT_MENTION_PRE_C1`,
`CHAT_POLL_VOTE_PRE_C1`), System, Chat Bridges lists them with their rows and the table's (`get#PreC1Backups`) and has
Drop the old copies (`drop#PreC1Backups`, both `ChatBridgeAdmin`, in `chat_migrate.rs`). Each table is checked again
first: a copy goes only when its table is on its new key with at least the copy's rows; one whose table has fewer rows
(a rebuild cut off) stays, named with why, as it may be the only copy of them. A copy that fails to drop is logged and
the rest go on.

## Tests

- `crates/moqui-core/src/service/chat_bridge_tests.rs`: an in-memory adapter registered only in the tests, with crash and
  failure injection, on a simulated clock: a crash before and after the network took a message (once on a network that
  dedupes, uncertain on one that cannot, then the authorized retry, refused to another member and to a stranger), one
  failing destination while another delivers (in order, backed off, then delivered; a permanent failure visible in the
  admin list and sent again), duplicate batches and echo loops between two networks, reordered edits, a reaction and a
  stale edit, a whole batch refused for security and malformed items rejected by name, an unsupported edit refused before
  the change.
- `crates/moqui-host/tests/it/chat_actors.rs`: the migration on embedded Postgres (below), then the copies: refused to a
  user without `ChatBridgeAdmin`, the two whose tables hold their rows dropped, the emptied table's copy kept.
- `crates/moqui-channels/src/tests.rs`: an unknown and a disabled bridge answering as an unsigned request, the
  disabled one its network 503; a webhook's replay held in the shared replay cache.
- `crates/moqui-host/tests/it/lock_then_insert.rs`: racing first joins make one actor.
- `basic-comms/test/BasicCommsBridges.spec.js`: a bridged room that is not connected in the browser, the terminal and the
  admin screen, and who may retry and administer.

**Not checked live:** no real network. C1 has no network adapter; each one is checked against its provider. C4-SMS's
Telnyx adapter is checked against a fake Telnyx ([SMS, Tests](SMS.md#tests)), not Telnyx itself.
