# Comms Framework

Status 2026-10-30: plan, no implementation of this move or the new protocols. The architecture was settled the same day
([§8](#8-decisions)); the review refinements and their sources are in [§10](#10-review-and-sources). Comms moves from mr-boss into a
framework component, `runtime/base-component/comms`, beside `tools`, and gains three things it does not have:

- chat that reaches outside Moqui (federation and bridges);
- voice over SIP and the phone network, with voicemail;
- web conferencing over WebRTC, with a selective forwarding unit (SFU) from the first call.

IronStone Comms in mr-boss becomes the framework app with BOSS's additions mounted on it or added by screen-extend.
Email moves with the rest; this plan adds no new mail features. Its known limitations and outstanding live sync and
OAuth checks remain in [Remaining-Work, Mail](Remaining-Work.md#mail); the move does not imply those checks have passed.

Sections: [1 starting point](#1-starting-point) · [2 scope](#2-scope) · [3 the framework Comms app](#3-the-framework-comms-app) ·
[4 chat protocols](#4-chat-protocols) · [5 voice](#5-voice) · [6 web conferencing](#6-web-conferencing) ·
[7 order](#7-order) · [8 decisions](#8-decisions) · [9 risks](#9-risks) · [10 review and sources](#10-review-and-sources)

---

## 1. Starting point

What is built (mr-boss `plan/impl-drawers-commons.md` §5 M5, M10 and §8; [Remaining-Work](Remaining-Work.md#chat-and-presence)):

- **Mail.**
  - `moqui-mail`: SMTP, the IMAP and POP3 sync with IDLE, OAuth sign-in.
  - The host's SMTP and LMTP listener, behind the `inbound-smtp` feature.
  - `org.moqui.impl.EmailClientServices`.
  - BOSS screens: Inbox, Compose, MessageDetail and the mail settings.
- **Chat.**
  - The entities: `framework/entity/ChatEntities.xml` (ChatRoom, ChatRoomMember, ChatEvent, ChatEventMention,
    ChatAttachment, the polls, ChatLinkPreview, UserStatus).
  - The services: `org.moqui.impl.ChatServices` (about 60), with Rust in `moqui-core` `service/chat.rs` and `chat_link.rs`.
  - Agent room members: `moqui-llm` `chat.rs`.
  - The wire is JSON: each event is a NotificationMessage that is not stored, on `chatRoom:<id>`, `chatUser:<id>` and
    `userStatus`, pushed over `/notws`.
  - BOSS screens: `Chat.qvue`, `ChatPanel.qvue` and ChatSettings.
- **Calls.**
  - Designed only: mr-boss `design-drawers-commons.md` §4 LiveCall, §14 and §19 "Chat and calls".
  - `Comms/Calls.xml`, `Calls/Call.xml` and `Calls/LiveCall.xml` are placeholders.
  - `UserStatus.inCall` exists for the future call services. The `CetCall` chat event type is seeded.
- **Voice and SMS.** No real-time audio exists. Assist's `transcribe` call reaches an OpenAI-compatible endpoint
  ([Assist](../docs/ai/Assist.md)). `org.moqui.SmsServices.send#SmsMessage` is a stub that logs "No SMS implementation in
  place".
- **The socket.** `/notws` authenticates a local user through a session cookie or login key. Besides topic subscriptions,
  it handles co-editing frames (`crates/moqui-host/src/notws.rs`, `notws/collab.rs`). Co-editing crosses the cluster on
  `collab-update`; the local topic bus alone does not cross nodes. Guests need the restricted path in §6.3.

BOSS Comms today is about 11,400 lines of screens and 6,350 lines of `ironstone.comms` services.

## 2. Scope

**In scope**
- The framework Comms app: mail, chat, calls and voice screens, with their services, usable with only the framework.
- An extension model so BOSS, or any app, adds links to its own records and features.
- Chat federation and bridges, and the business messaging channels.
- Voice:
  - SIP trunks to the phone network (PSTN);
  - a SIP registrar for desk and soft phones;
  - numbers, call routing, voicemail, forwarding, ring groups, an IVR and queues;
  - SMS through the same providers.
- WebRTC calls and meetings on an SFU in the server, with STUN and TURN built in, recording and captions.

**Not in scope**
- New mail features, beyond keeping mail working through the move.
- Moqui as a carrier. Numbers come from SIP trunk providers.
- Video to SIP endpoints. SIP legs are audio only (decision D-V3).
- A Matrix homeserver in the first round. It is a later step, C6 (D-C2).

**Rules this plan keeps**
- **The framework stays Mantle-free** (mr-boss impl-drawers-commons §4). The comms component reads no Mantle entity and
  calls no Mantle or `ironstone.*` service.
- **Failure tolerance** (AGENTS.md). A failing bridge, trunk, registration, room or recording is logged by name, and the
  rest go on. Authorization, encryption policy and durable acceptance stay strict; a failed write never reports success.
- **Listeners off by default,** as the SSH, SMTP, A2A and MCP listeners are ([Run-Deploy](../docs/Run-Deploy.md)): SIP,
  media, TURN and XMPP each have a conf switch.
- **Licenses.** Each new crate is checked against `deny.toml` and its allow list. MPL-2.0 needs an exception like
  cssparser's; AGPL and GPL dependencies run only as separately reviewed external processes. The inspected Zimbra
  server files are GPL-2.0 and its voice web client is CPAL-1.0. The applications in §10 are design references;
  nothing is copied from them. Process separation does not replace checking the distribution's license obligations.
- **Crates above core** go through `moqui_core::extension` ([Crate-Split](Crate-Split.md#the-seam-rules)).

---

## 3. The framework Comms app

### 3.1 Placement

- **The component.** `runtime/base-component/comms`, with `component.xml`, `MoquiConf.xml`, `screen/`, `service/`,
  `data/`, `template/` and `test/`. Its `MoquiConf.xml` mounts `component://comms/screen/Comms.xml` under `apps.xml` as
  `comms` ("Comms", menu index 95), as tools mounts `system` and `tools`.
- **The data model stays in `framework/entity`.** The chat entities stay where they are, since `moqui-llm` agents and
  the notification socket use them. The new call, voice and bridge entities go there too (§5.4, §4.4). The comms
  component holds the screens and the screen-facing services.
- **Services.** The screen-facing services become `org.moqui.comms.*` (`MailSettingsServices`, `InboxServices`,
  `ComposeServices`, `ChatScreenServices`, `CallServices`, `VoiceServices`). The record services stay in
  `org.moqui.impl.*` (`ChatServices`, `EmailClientServices`). The package name is decision D-A3.
- **Rust.** New crates above core, each a host feature off by default:
  - `moqui-rtc`: the WebRTC endpoint, the SFU, TURN, RTP and the codecs, recording.
  - `moqui-sip`: the SIP user agent, registrar and B2BUA (back-to-back user agent), over `moqui-rtc` media.
  - `moqui-xmpp`: the XMPP server.
  - `moqui-matrix`: the Matrix appservice, later the homeserver.
  - `moqui-channels`: the business channel connectors and SMS.

The services expose supported, configured and active capabilities separately, following Zimbra's feature model
(§10.2). A compiled feature alone does not mean its listener, provider or credential is ready. Screens use these
capabilities, including unavailable reasons, and services enforce them. Optional features have the usual off stubs.

### 3.2 What moves, splits or stays

| Part | Depends on today | Plan |
| --- | --- | --- |
| Settings: MailAccounts and its Add wizard, SignIn, SyncProgress, Removed, Identities, Rules, TextBlocks, OutOfOffice, Chat | `ironstone.comms.MailSettingsServices`, `EmailClientServices`; the settings already handle Internal accounts | **Split and move.** Internal account settings come from the BOSS source provider. OutOfOffice retains the existing `STRT_EMAIL_CONTACT_SERVICE` contact predicate; recipient search is a different contract. |
| Chat, ChatPanel, ChatTui | about 20 `org.moqui.impl.ChatServices` calls a screen. BOSS `ChatServices` (753 lines) adds `decorate#Rooms`, `find#RecordRoom`, `find#MessageRecords`, `file#OnRecord`, `create#FromChat`, `search#PaletteRooms` and record cards | **Move.** The record features become hooks. Chat.qvue has no contact or party code, and about 54 record references. |
| Inbox/Mail, MessageDetail, MailTui | `InboxServices` (1,444 lines): the Internal account and mailboxes (Mantle `CommunicationEvent`, `ContactList`), record links, create a task, request or event, the invitation card (`respond#EventInvitation`), the contact card | **Split.** The framework Inbox has the email accounts. BOSS adds the Internal account, mailboxes and records through hooks. Mail.qvue has about 185 lines on internal messages and mailboxes and about 100 on contacts. |
| Compose, ComposeTui | `ComposeServices` (1,035 lines): recipients from Mantle parties and contact books, `create#Message`, `send#EmailDraft` (to file the message on a record) | **Split.** The framework composes and sends email. Recipient search, internal messages and filing are hooks. |
| Home | `CommsHomeServices`: meetings (`get#FullCalendarData`), internal threads, mailboxes | **Split.** Framework Home shows unread mail, mentions, missed calls and voicemail; BOSS contributes meetings and mailboxes through providers (D-A4). |
| Calls, Call, LiveCall, CallSettings | placeholders | **Built in the framework** (§5, §6). |
| Consent, Moderation, Shared/Chatter, RecordFiles | Mantle `ConsentServices`, `mantle.forum`, record timelines | **Stay in BOSS.** |
| Demo data `BOSS_DEMO_*` | Mantle parties | The framework gets its own users-only demo data. BOSS keeps its demo data for its additions. |

BOSS group checks in the services (`BOSS_COMMS`, `BOSS_CONTACT_ADMIN`) become framework permissions that BOSS grants
with `UserGroupPermission` rows.

The framework account lists, settings and compose choices omit Internal accounts unless the BOSS source provider
contributes them. Existing Internal account rows are preserved; the extraction does not delete user data. Mail account
IDs, drafts, rules, OAuth callbacks and notification links are checked through the moved screens. OutOfOffice wording
must describe the configured contact predicate rather than assume Mantle contact books are installed.

### 3.3 How an app adds to the screens

**XML screens use `screen-extend`.** `merge_extend`
(`crates/moqui-core/src/screen/definition.rs:1165`) supports:
- `transition` and `parameter` replaced by name;
- `subscreens-item` added or replaced;
- `actions-extend` before, after or replacing the actions;
- forms merged by name;
- `section` and `section-iterate` overridden by name;
- `widgets-extend` before, after, at the start of or at the end of a named element.

For Inbox, the framework's `threads` transition uses the inbox-source dispatcher; BOSS contributes Internal threads
through that hook. It does not also replace the transition with a second aggregation path.

Two gaps to close:
- `merge_extend` does not merge `shell-providers`. Merge children by element type and their actual key: `kind` for
  quick-create, `group` for palette-source, `service` for today-source, `doc-type` for palette-type, `topic` for
  notification-topic, and `scope` then `keys` for keymap/key. Link entries use their screen; response-action also uses
  text-parameter, page-action uses its screen or agent, and assist-prompt uses screen, entity and source text.
  A later extension replaces an equal key in the existing component extension order; different keys accumulate.
  Labels never identify an action. Sequence ties keep declaration order. Cover every shell-provider child in A0.
- A `render-mode` can't be replaced. It is not needed if the `.qvue` files stay generic.

**`.qvue` screens need a new extension point.** `screen-extend` works on the server's XML tree, and a `.qvue` is one
client component, so nothing can be added inside it. The proposal has three layers, from the cheapest to the rarest:

1. **Hook services.** The framework already reaches apps through theme values that name a service or screen
   (`STRT_RECORD_VISIBLE_SERVICE`, `STRT_EMAIL_CONTACT_SERVICE`, `STRT_CHATTER_SCREEN`). The comms services ask a hook
   for what an app contributes, and skip the optional feature when the hook is empty. Each theme selects one
   dispatcher per hook; it may aggregate several app providers. The hooks:

   | Hook | Asked with | Answers | BOSS fills it with |
   | --- | --- | --- | --- |
   | `STRT_COMMS_INBOX_SOURCE` | operation (accounts, threads, settings), sourceId, folder, filters, page | extra account/settings contributions or a page of thread rows | the Internal account and mailboxes (`find#InboxThreads`) |
   | `STRT_COMMS_RECIPIENT_SEARCH` | text, kinds | people, groups and lists with addresses | parties and contact books |
   | `STRT_COMMS_RECORD_LOOKUP` | text or id | record cards `{entityName, id, title, status, url}` | `lookup#ById` |
   | `STRT_COMMS_RECORD_LINK` | message or chat event, record | ok, the link | `link#CommunicationEventRecord`, `file#OnRecord` |
   | `STRT_COMMS_CREATE_FROM` | message or event, kind | the new record's url | `create#RecordFromChatMessage` and its mail twin |
   | `STRT_COMMS_INVITATION` | the iMIP part, response | ok | `respond#EventInvitation` |
   | `STRT_COMMS_CONTACT_CARD` | address or number | a card, an "add to contacts" action | `get#ContactCard` |
   | `STRT_COMMS_MEETINGS` | userId, range | meetings with a call link | `get#FullCalendarData` |
   | `STRT_COMMS_CALL_CONTEXT` | a caller number or address | who it is and their open records (a screen pop) | party match and open cases |
   | `STRT_COMMS_HOME_SOURCES` | sourceId, range, page | cards and pages of rows | meetings and mailboxes |
   | `STRT_COMMS_EXTERNAL_CONVERSATION` | chatRoomId, chatBridgeId, chatActorId, idempotencyKey | record link, when configured | file the conversation on a case |
   | `STRT_COMMS_PANELS` | slot, item reference, appRoot | authorized panel descriptors | contact, record and mailbox panels |

   Contracts are service interfaces with declared inputs and outputs. The authenticated ExecutionContext supplies
   authority; a userId or appRoot passed as data cannot grant it. Collection contributions have provider-qualified
   `sourceId` and `itemId`, stable ordering and bounded pages, using the existing page metadata convention. Each source
   owns its page; the dispatcher does not concatenate independently paged lists and claim they are one global page.
   Empty results and unavailable providers are distinct. Optional reads use a force-new probe transaction with
   `pushErrors`/`popErrors`, logging provider and operation and returning `failedParts`. A mutation returns an explicit
   failure; background external-conversation filing retries with its idempotency key without replaying the message.
   Missing or failing authorization hooks deny access. `STRT_EMAIL_CONTACT_SERVICE` and
   `STRT_RECORD_VISIBLE_SERVICE` retain their existing contracts.
2. **Declared actions.** A new element, `<comms-action>`, can sit in a screen or a `screen-extend`.
   Each action names:
   - a provider-qualified `name` and an integer `sequence` (default 0);
   - `slot`: `message-actions`, `thread-toolbar`, `room-header`, `chat-message-actions`, `compose-toolbar`,
     `call-actions` or `voicemail-actions`;
   - `label` and `icon`;
   - exactly one of `screen` (with dialog or page presentation) or `transition`;
   - explicit parameter mappings from the authorized item data; no whole-context forwarding;
   - `condition` (a display expression over the item's fields, never an authorization rule).

   The screen's data carries the declared actions, and the component renders them in its menus. This is how BOSS adds
   "Create task", "Link to record" and "File on case". Merge by `name`: a later extension replaces the entire
   descriptor, and sequence ties keep declaration order. The server filters unauthorized targets and the target
   service checks access again on invocation. The same descriptors produce TUI actions. A0 covers the XSD, parser,
   screen-extend merge, serialization and both renderers, following the existing shell-provider path.
3. **Panel slots.** For the few real panels (the contact card pane, a record pane in Chat, the mailbox sidebar), the
   component renders `<m-ext-panel slot="…">`. `STRT_COMMS_PANELS` registers descriptors with `name`, `slot`, `sequence`,
   `screen` and explicit parameters. Only installed, authorized screen locations are accepted, never a screen or URL
   supplied by the item. The browser loads that screen through `moqui.loadComponent`; TUI renders the same screen's
   TUI representation. A read-only panel failure leaves a named unavailable state without failing other panels.

Hard-coded `/qapps/Comms/...` links in the `.qvue` files (about 7) become relative to `mScreen.path` and `appRoot`.

### 3.4 BOSS after the move

- **BOSS keeps `screen/Comms.xml` as its own root.** The root keeps:
  - the always-actions, with `setup#UserOrganizationInfo`;
  - `shell-providers`;
  - Chatter and RecordFiles.
- **It mounts the framework screens** by location (`subscreens-item location="component://comms/screen/Comms/Inbox.xml"`
  and so on), including Home, beside Consent and Moderation.
- **Its additions are `screen-extend` files** under `mr-boss/screen-extend/comms/screen/Comms/...`, matching each
  framework screen's component location, plus the hook theme values in `BossSetupData.xml`.
- **The launcher shows one Comms.** With BOSS installed, the framework's `comms` app is hidden from the launcher by a
  screen-extend of `apps.xml` (`menu-include="false"`), and its URLs stay valid.
- BOSS mounts to retain its organization setup and BOSS-only screens. The shell-provider merge is fixed in A0, not
  used as a lasting reason for this placement. Extensions apply by screen location and therefore affect both mounts.
  Hook services obtain and check their own organization context; they cannot rely on BOSS parent actions having run.
  The framework URL is a supported mount, not an authorization bypass or a hidden alias for the BOSS route.

### 3.5 Tests and docs

- **Framework specs.** These move to `runtime/base-component/comms/test` with users-only data: Chat, ChatPanel,
  ChatSettings, Compose, Inbox, MailAccounts, MailSettings and MailSignIn. A new `CommsExtension.spec.js` proves each
  hook, action and panel with test providers, including a failing provider and a denied user. The framework's
  `AppStructure`-style render covers every Comms screen in qvt (including its qvue component) and tui.
- **BOSS specs.** InboxMailboxes, Consent, CommsModeration, Chatter and CommsHome stay in BOSS. Inbox and Compose keep a
  BOSS spec for the Mantle parts. Run the shared-screen cases through both mounts, checking organization isolation,
  direct action requests, relative links and a single launcher entry. Framework-only fixtures contain no Mantle data.
- **Docs.** A new user guide, `docs/tools/Comms.md`. mr-boss `design-drawers-commons.md` §12–14 and §17 shrink to BOSS's
  additions and link to it.

---

## 4. Chat protocols

### 4.1 Approach

ChatEvent is Moqui's application timeline. Adapters map rooms, actors and events while preserving external IDs,
timestamps and protocol metadata separately. In C2 the Matrix homeserver owns Matrix state; C6 adds its own protocol
store for signed events, the DAG and state resolution. A local event sequence is not a replacement for that state.
Bridged rooms use the usual search and retention services subject to adapter capabilities, room access and encryption.
Record-room access restrictions still apply: linking a conversation to a record does not authorize exporting Chatter
or record cards to a network.

**Inbound.** A new `org.moqui.impl.ChatBridgeServices.ingest#Event` authenticates the configured connector, resolves
the external actor and mapped room, checks posting or moderation rights, validates content and attachment references,
and calls shared message/edit/reaction/redaction logic. That logic maintains unread, mentions, thread summaries and
agent dispatch as for local events. `append#Event` remains an internal primitive whose caller has checked access;
adapters cannot use it to bypass those checks or impersonate a local account. An account-link row alone grants no
authority to send as that account.

**Durability.** Store an outbound delivery row for each eligible room-bridge mapping in the same transaction as the
event. A tx-commit SECA only wakes workers: today's `service/call.rs::register_tx_rules` runs an in-memory callback,
so restart recovery scans persisted pending work. Workers claim expiring leases, retry with backoff and honor provider
rate limits. One failed destination does not block another. Use a stable provider transaction/idempotency key on every
retry; a provider without idempotent sends must expose an uncertain outcome for reconciliation rather than claim
exactly-once delivery. Permanent failures remain visible with an authorized retry action.

Persist authenticated inbound batches before acknowledging them; repeated transaction IDs are no-ops. Workers dedupe
events with unique external mappings, retain source provenance to suppress echoes, and hold edits/reactions/deletes
until their target mapping exists. Per-room/per-destination ordering prevents an edit overtaking its send. A malformed
independent item is durably rejected with a named error while valid items continue; security failures reject the
request before acceptance. This follows the [Matrix application-service transaction contract][matrix-as].

### 4.2 The landscape

| Network | Open? | Federation | Reach | How to reach it |
| --- | --- | --- | --- | --- |
| **Matrix** | open spec, open servers | yes, server to server | Element and other Matrix clients | Phase 1: an appservice on an external homeserver. Later: Moqui as the homeserver (D-C2) |
| **XMPP** | IETF standard | yes, server to server | a long-standing base, simpler to serve. Clients: Conversations, Monal, Dino, Gajim | Moqui as an XMPP server: client and server-to-server connections |
| **WhatsApp** | closed | no | business and personal messaging | The WhatsApp Business Platform Cloud API (official, for business numbers). Personal accounts only through a Matrix bridge (D-C4) |
| **SMS and MMS** | carrier networks | — | every phone | The SIP trunk provider's messaging API (§5.6); RCS later |
| **Telegram** | open client API, closed server | no | large | The Bot API (official) |
| **Slack, Microsoft Teams** | closed | Slack Connect and Teams federation are their own | where business teams already are | A Slack app (Events and Web API). A Teams bot (Bot Framework) |
| **Signal, Messenger, Discord, Google Messages** | closed | no | large | Only through the mautrix bridges on the Matrix homeserver (§4.3) |
| IRC | open | network-based | small | Not planned |

### 4.3 Proposals

- **Bridge layer (C1).** Framework entities, §4.4. Adapter implementations register through `moqui_core::extension`;
  a configured `ChatBridge` row selects the implementation and provider account. The core hook contract exposes
  capabilities, lifecycle and delivery operations; inbound work enters the ingestion service in §4.1. Protocol I/O
  runs above core and entity/service work runs on service workers, keeping core synchronous.
  - The Rooms screen marks a bridged room.
  - An external member shows their network's badge.
  - Room settings show the bridge and its link.
  - A bridge that goes down marks its rooms as "not connected" and keeps the local timeline.
  - Delivery state distinguishes locally saved, pending, accepted remotely, failed and uncertain. Unsupported edits,
    deletes or reactions are refused before mutation, not silently represented as synchronized.
- **Matrix, phase 1: appservice (C2).** Moqui registers as an application service on a homeserver run beside it:
  - The homeserver is Synapse (AGPL-3.0, a separate process) or Tuwunel (a Rust homeserver; license to confirm).
  - The appservice owns a namespace of users (`@moqui_<user>:example.com`) and rooms (`#moqui_<room>:…`), so Moqui users
    are puppets on Matrix, and federated users join Moqui rooms as external members.
  - Federation comes from the homeserver. So does every mautrix bridge (WhatsApp, Signal, Telegram, Slack, Discord, Meta,
    Google Messages), each a separate AGPL process the deployment adds.
    Their [end-to-bridge encryption][mautrix-encryption] may expose plaintext to that process. C2 supports only
    unencrypted Matrix portals; encrypted portals are explicitly unavailable. Installing C6 does not make a
    third-party bridge browser-to-browser E2EE or remove its trust boundary.
  - Types and APIs come from **ruma** (MIT): `ruma-events` and the appservice and client-server API types.
  - Rooms, membership and related events map to the application timeline, but Matrix state events, permissions and
    original event metadata retain their protocol meaning (§4.1). The adapter does not promise identical semantics
    for every Moqui feature.
  - Encrypted Matrix rooms are not bridged by C2. Native client interoperability requires both C6 and the C7 device,
    sync and key-distribution contracts, proved with an independent client (§4.5).
- **XMPP server (C3).** `moqui-xmpp`, a server on Tokio:
  - **Listeners:** client connections on 5222 (STARTTLS) and 5223 (direct TLS); the WebSocket at `/xmpp-websocket`
    (RFC 7395); server to server on 5269 with TLS and SASL EXTERNAL, and dialback as a fallback.
  - **Sign-in** is by an app password (`UserAppPassword`, as DAV and git use).
  - **Mapping:** users are JIDs (`user@domain`); rooms are MUC rooms (XEP-0045) on `rooms.domain`; history is MAM
    (XEP-0313) over ChatEvent.
  - **XEPs for today's clients:**
    - 0030 service discovery and 0115 entity capabilities;
    - 0198 stream management;
    - 0280 carbons;
    - 0313 message archive;
    - 0363 HTTP file upload, to ChatAttachment;
    - 0357 push;
    - 0085 typing and 0333 read markers;
    - 0308 corrections, 0424 retractions, 0444 reactions and 0461 replies.

    C3 records the exact Compliance Suite revision, supported XEP versions and client versions in its verification
    record; it claims only the levels whose mandatory features pass.
  - **OMEMO** (XEP-0384): XMPP clients talking to each other may use it. Besides preserving ciphertext in messages,
    carbons and MAM, C3 supplies device-list and bundle storage/notifications through XEP-0060 pubsub and XEP-0163 PEP
    with access checks. Record the OMEMO namespaces supported by the selected Conversations, Monal, Dino or Gajim
    versions; the [reviewed XEP-0384 revision][omemo] also depends on XEP-0420. Do not infer client compatibility from
    a namespace alone. Moqui marks opaque messages unreadable and excludes them from plaintext processing and other
    bridges. Its own encrypted rooms use Megolm (§4.5), not OMEMO.
  - **Library:** `xmpp-parsers` and `tokio-xmpp` are MPL-2.0, which needs a `deny.toml` exception like cssparser's.
    D-C3 selects that exception, including `minidom`; a parser substitution is not an automatic equivalent server.
- **Business channels (C4).** `moqui-channels`, each a REST and webhook connector into the bridge layer:
  - WhatsApp Cloud API: templates, the 24-hour customer service window, media.
  - Telegram Bot API.
  - SMS and MMS through the voice provider (§5.6), which replaces the `SmsServices` stub.
  - Slack and Teams later (C5).

  A conversation with an outside person is a ChatRoom of type `external`, with local participants and external actors.
  An app's hook (`STRT_COMMS_EXTERNAL_CONVERSATION`) can file it on a record, such as a Customer Service case in BOSS.
  Inbound webhooks use bounded raw request bodies. Verify the signature and replay constraints over the provider's required bytes
  before parsing or accepting events; a signature cannot be checked without reading those bytes. Persist acceptance
  through §4.1. Connector setup includes media limits, delivery receipts and provider-specific refusal reasons.
- **Matrix, phase 2: homeserver (C6).** Moqui serves the Matrix server-to-server and client-server APIs
  itself, on ruma: event signing, the room DAG, state resolution v2, backfill and the media repository. Element and
  other clients then connect to Moqui directly. Tuwunel and conduwuit are references, not imported implementations.
  Replacement of the C2 server is conditional on a migration rehearsal preserving the server name, signing keys,
  user and room IDs, room versions, state/history, media references, appservice registrations, and device/key/sync
  state. Namespace ownership alone migrates none of this. Keep the external server and its backup available until
  the acceptance gate passes; cutover has one writer, a bounded write freeze and a verified rollback checkpoint.
  If a lossless migration cannot be demonstrated, C2 remains supported and C6 does not claim an in-place replacement.

The connector contract carries this capability matrix. Each implementation narrows it to the verified API/version;
an unsupported operation returns a reason code and the UI omits the action. A link to an external room grants no rights.

| Adapter | Initial supported mapping | Explicit limits or refusals |
| --- | --- | --- |
| Matrix C2 | messages, membership, replies, edits, reactions, redactions and media with ID/state mappings | encrypted rooms; Moqui polls, record cards and moderation without an equivalent |
| XMPP C3 | messages, MUC, MAM, receipts and the negotiated XEPs above | unadvertised XEPs; reading OMEMO content in Moqui; conversion of encrypted content to another network |
| SMS/MMS C4 | text/media within provider limits and delivery status | edits, reactions, remote deletion and group-room semantics |
| WhatsApp/Telegram C4 | supported text/media and provider receipts | operations outside provider permissions, template/window rules or API capabilities; no fallback that pretends success |
| Slack/Teams C5 | mappings documented against the selected APIs at C5 | no claimed support until its connector's operation and permission cases pass |

### 4.4 Data model (framework, `moqui.chat`)

- `ChatBridge` {chatBridgeId, typeEnumId (Matrix, Xmpp, WhatsApp, Telegram, Sms, Slack, Teams), name, configJson with
  secrets encrypted, statusId, lastErrorText}.
- `ChatRoomBridge` {chatRoomBridgeId, chatRoomId, chatBridgeId, externalRoomId, direction (both, in, out), fromDate,
  thruDate}. Each external room has one active mapping per bridge; delivery rows retain this stable mapping ID.
- `ChatActor` {chatActorId, kindEnumId (local, external), userId, chatExternalUserId}. Exactly one identity reference
  is set, unique within its kind. Local agents remain local service-user actors; external actors have no login.
- `ChatExternalUser` {chatExternalUserId, chatBridgeId, externalUserId, displayName, avatarUrl, linkedUserId}, unique
  on bridge and external user. A verified puppet link is provenance, not permission to become that local user.
- `ChatRoomMember` uses `(chatRoomId, chatActorId)` as its key. Events use `senderActorId`; mentions, votes, moderation
  authors and room/thread summaries resolve actors too. Existing local-user inputs and local notification topics
  remain supported by resolving userId to its local actor, with userId retained in local-user response fields.
- `ChatEventExternal` maps local event, chatRoomBridgeId and external room/event ID, with original timestamp,
  origin and protocol metadata. The bridge/room/event tuple is unique even across mapping changes; it dedupes input
  and maps edits, reactions and deletes.
- `ChatBridgeInbound` stores the authenticated bridge/transaction ID (unique), bounded batch payload, processing state
  and per-item outcomes. Its retention preserves dedupe beyond the provider retry window.
- `ChatBridgeDelivery` stores local event and destination mapping (unique), stable idempotency key, target dependency,
  state, attempt count, next attempt, lease owner/expiry, remote ID and last error. Retain dedupe tombstones when
  message content is purged; do not retain plaintext beyond the room's policy.

C1 backfills one local actor per referenced user, then migrates membership keys, sender references, mentions, poll
votes and summary/read projections. Verify counts and referential consistency before switching reads and writes;
keep local userId service compatibility and a rollback backup. A maintenance window prevents old writers using the
old key during migration. There is no nullable-userId primary key and no fabricated UserAccount for a remote person.
Migration fixtures include departed members, deleted local accounts with retained history, reactions, mentions,
unread markers and agents. Remote messages reach agents only under the room's explicit unencrypted-room policy.

### 4.5 End-to-end encrypted rooms (C7)

A room may be created as encrypted, and it can't be turned off later. Messages use Matrix's Olm (between devices) and
Megolm (rotating group sessions). Prefer **matrix-sdk-crypto**, the established client encryption state machine over
**vodozemac**, built to WASM; C7 proves its browser storage and protocol integration before exposing encrypted rooms.
Do not rebuild device verification, backup and key management from crypto primitives. The server stores and pushes
ciphertext, public device keys and necessary routing metadata; plaintext and private device keys stay in the browser.

- **Device lifecycle.** `ChatDevice` holds public identity, verification/revocation state and device-list revision;
  `ChatDeviceKey` holds signed public and one-time keys, with atomic claims. Durable to-device queues and sync cursors
  deliver key messages once acknowledged. Private keys, crypto sessions and replay state persist in the browser's
  crypto store. Membership changes and device revocation rotate outbound sessions; a revoked device receives no new
  keys. Rotation cannot retract keys or plaintext already held by a device.
- **Attachments.** Encrypt files and thumbnails in the browser before upload. Their keys and hashes travel inside
  the encrypted event; the server stores only encrypted objects and cannot make previews or extract their text.
- **Key backup.** Room keys are backed up to the server, encrypted with a recovery key the user keeps. Without it, a
  new device cannot restore backed-up history without another authorized key source, such as a verified existing
  device. Test lost-device recovery and missing keys without silently resetting trust or sending plaintext.
- **Device verification.** Emoji or QR-code device verification, and cross-signing, so one verified device vouches for
  the others. Clients verify signatures and trust locally; a server-side verification flag cannot establish trust.
- **What an encrypted room gives up:**
  - server search;
  - agent members;
  - link previews made by the server;
  - Chatter on records;
  - "Create from" over the message text, unless the user explicitly authorizes that action to disclose selected text;
  - bridges; native Matrix interoperability requires the C6/C7 client acceptance gate, not just matching algorithms.

  No plaintext is written to bodyText, room snippets, notifications, logs, analytics or agent inputs. Push says only
  that an encrypted message is available. The screens explain unavailable features. Retention removes ciphertext,
  encrypted attachments and associated server-side records; it cannot erase copies held by recipients.
- **Calls.** Message encryption and media encryption are separate capabilities. W8 uses a dedicated call-key lifecycle
  tied to admitted participants (§6.4), not a reused Megolm session. A call requiring E2EE is unavailable before W8 or
  on unsupported clients; it never silently downgrades. Other calls explicitly show their media encryption mode.

---

## 5. Voice

### 5.1 Approach

Every call is one `CallSession`, whatever its transport. A call has **legs**: a browser over WebRTC, a guest's browser,
a SIP desk phone or soft phone, or a phone number through a trunk. `moqui-rtc` holds the media of every leg:
- Browser legs use the SFU (§6).
- SIP legs are audio only. They are mixed for the SIP side, because a phone takes one stream: each SIP leg gets a mix of
  everyone but itself. They are forwarded as one more participant to the browser side.

So a phone call, a desk-phone call, a meeting and a meeting someone dials into are the same thing. Moving between them
(add a phone number to a meeting, turn a call into a meeting) adds legs to the same session, subject to permissions,
consent and media compatibility. Encrypted calls cannot add SIP legs.

### 5.2 SIP and PSTN

- **Trunks (V1).** `moqui-sip` registers to, or is IP-authenticated by, a SIP trunk provider (for example Telnyx,
  Twilio Elastic SIP, Bandwidth, VoIP.ms or Flowroute). Outbound calls go to the trunk. Inbound DIDs
  route by `PhoneNumber` (§5.4). The trunk side carries:
  - transport: UDP, TCP or TLS on 5060 and 5061;
  - auth: SIP Digest, with realm/algorithm-specific credentials and nonce/replay checks;
  - media: SRTP (SDES) on TLS;
  - DTMF: RFC 4733;
  - codecs: G.711 μ-law and A-law, with Opus where the trunk offers it.

  Browser Opus is transcoded to G.711 in `moqui-rtc`. STIR/SHAKEN attestation is the provider's. TLS with SRTP is the
  default security profile; UDP/TCP or plain RTP require an explicit provider profile and are not a silent downgrade.
  Claim interoperability only for documented, verified profiles. Caller IDs and outbound routes are authorized for
  the user; destinations, call concurrency and provider spending limits are bounded by deployment policy.
- **Registrar for phones (V4).** Desk phones (Yealink, Polycom, Grandstream) and soft phones (Linphone, Zoiper, Groundwire)
  register to Moqui with generated SIP credentials shown once. Store encrypted, algorithm/realm-specific Digest
  verifiers (HA1), not a generic login password hash. Prefer SHA-256 under [RFC 8760][sip-digest]; legacy algorithms
  require an explicit device profile. Registration bindings are separate expiring rows, with contact, transport flow,
  Call-ID/CSeq and last refresh, so one device may have several contacts. Expired or revoked bindings cannot ring.
  Provisioning files for the common desk phones come later.
- **Browser phone.** The browser never speaks SIP. It talks WebRTC to Moqui, and Moqui places the SIP leg. That keeps
  one media path, one recording path and one set of credentials, and needs no SIP over WebSocket (RFC 7118) or JsSIP.
- **Emergency calls (D-V2).** Preserve the chosen routing rule, with deployment readiness checked before PSTN use.
  The [US guidance on Kari's Law and RAY BAUM'S Act][emergency-guidance] includes direct dialing, notification and
  dispatchable location for covered systems; other jurisdictions and deployment types require their own review.
  - Emergency numbers go only to a trunk marked emergency-capable, with each user's or phone's registered location,
    validated callback identity and a configured notification destination. Emergency routing precedes ordinary dial
    prefixes and discretionary destination restrictions.
  - Without one, emergency dialing is refused with a clear message, and the Phone settings say so.
  - That refusal notice is not evidence of compliance. V1's release gate includes provider-supported location and
    callback provisioning, notification behavior, mobile/remote-user location updates, and the operator's readiness
    review. Live verification uses a provider-approved test procedure, never an unarranged emergency call.

### 5.3 Features, from Zimbra and the usual phone systems

Zimbra's voice app (`../3p_other/Zimbra` from the repository root; server GPL-2.0, web client CPAL-1.0) was a front end to Cisco and Mitel
unified-communications servers:
- the web client is `zm-web-client/WebRoot/js/zimbraMail/voicemail/` (`ZmVoiceApp.js`; the `model/`, `controller/` and
  `view/` directories);
- the SOAP API is `zm-mailbox/soap/src/java/com/zimbra/soap/voice/` (`GetVoiceInfo`, `SearchVoice`, `VoiceMsgAction`,
  `GetVoiceFeatures`, `ModifyVoiceFeatures`, `GetVoiceMailPrefs`, `ModifyVoiceMailPrefs`, `ModifyVoiceMailPin`,
  `ModifyFromNum`, `UploadVoiceMail`);
- the provider setup is `zm-mailbox/store/docs/INSTALL-VOICE.txt`.

Here the server is Moqui itself. Zimbra's `ZmCallFeature` distinguishes subscribed and active features; Moqui adopts
explicit supported/configured/active capability reporting (§3.1), rather than showing every provider the same settings.
What Zimbra showed, and what it maps to:

| Zimbra | Here |
| --- | --- |
| Folders: Voicemail Inbox, Missed, Answered and Placed calls, Trash (`ZmVoiceFolder`) | Calls screen tabs: All, Missed, Voicemail. Placed and answered are filters. Voicemail has Trash |
| Voicemail actions: listen, mark heard and unheard, delete, empty folder, download, print, call back, reply by email, forward by email, new contact (`ZmVoicemailListController`) | Voicemail actions, plus the transcript, "Create from" (the hook) and "Add to contacts" (the contact card hook) |
| Call list: call back, contact, new contact (`ZmCallListController`) | Call history actions, plus a note, and Chatter through the call context hook |
| Call features (`ZmCallFeature`; SOAP `type/*Feature`): call forwarding always, busy and no answer; selective forward, reject and accept with caller lists; anonymous call rejection; caller-ID blocking; call waiting | Phone settings "Call handling": the same rules, plus schedules from business hours or a calendar, and simultaneous ring to a mobile number |
| Voicemail preferences: email notification address, PIN (`ModifyVoiceMailPin`) | Voicemail settings: greeting (recorded in the browser or uploaded), PIN for phone access, email notice with or without the audio and the transcript, the number of rings |
| From number (`ModifyFromNum`) | The caller ID picker: my numbers, my ring groups' numbers |
| Presence through the UC provider | `UserStatus`: "in a call" set automatically, and DND sends calls to voicemail |

Beyond Zimbra:
- dialer and keypad, with contact search through the recipient hook;
- hold, mute, blind and attended transfer, add a participant, merge into a meeting;
- ring groups and hunt groups (simultaneous, in order, round robin);
- an IVR and auto attendant: menus, business hours, prompts recorded or from text to speech;
- call queues with wait music, positions, agents and wrap-up time. Customer Service is the main user, through hooks;
- call recording, with the consent rule (D-V5);
- voicemail and call transcription through `moqui-llm`'s `transcribe` binding;
- screen pop through `STRT_COMMS_CALL_CONTEXT`;
- call quality (jitter, loss, MOS) into telemetry, for the Monitor app.

### 5.4 Data model (framework, `moqui.call`; shared with §6)

- `CallSession` {callSessionId, chatRoomId, kindEnumId (direct, group, meeting, phone), statusId, revision,
  directionEnumId, startedDate, answeredDate, endedDate, endReasonEnumId, mediaNodeId, mediaGeneration, encryptionMode}.
- `CallParticipant` {callParticipantId, callSessionId, identityKindEnumId (local, guest, phone), userId,
  guestIdentityId, chatActorId (optional), displayName, roleEnumId, admissionStatusId, admittedDate, revokedDate}.
  Local participants resolve to userId; guest identities are call-scoped, and phone identities carry their presented
  number on the leg separately from any verified local identity. C1 can add the optional chat-actor link without
  blocking W1's local calls. Participation grants only call permissions; it creates no room membership by itself.
- `CallLeg` {callLegId, callSessionId, callParticipantId, legTypeEnumId (web, guest, sipDevice, pstn), phoneNumber,
  sipDeviceId, offeredDate, answeredDate, joinedDate, leftDate, statusId, endReasonEnumId, qualityJson}.
- `CallConnection` {callConnectionId, callLegId, generation, signalingNodeId, credentialExpiry, lastHeartbeatDate,
  statusId}. Reconnecting replaces connection state; it does not create a second participant or a second answered call.
- `CallLink` {callLinkId, tokenHash, name, ownerUserId, chatRoomId, policyEnumId (members, link, lobby), expiryDate,
  revokedDate}.
- `CallRecording` {callRecordingId, callSessionId, ownerUserId, kind (per track, mixed), statusId, startedDate,
  endedDate, retentionUntilDate, lastErrorText}. Child `CallRecordingTrack` and `CallRecordingSegment` rows identify
  source participant/leg/track, time range, contentLocation (DbResource), duration and processing state. Transcripts
  reference recording/segment and speaker, with their own processing state and the same access policy.
- `CallRecordingConsent` records participant/leg, recording scope, policy revision, notice time, acknowledgement
  time/method and withdrawal. A call-level Boolean cannot represent late joiners or transferred legs.
- `VoiceTrunk` {voiceTrunkId, name, host, transport, authJson encrypted, registerFlag, emergencyCapable, statusId}.
- `PhoneNumber` {phoneNumber, voiceTrunkId, routeTypeEnumId (user, ringGroup, ivr, queue), routeId, smsEnabled,
  emergencyLocationId}. Messaging configuration can exist before the SIP trunk is enabled.
- `EmergencyLocation` holds framework-owned validated address/location data, callback identity and provider reference;
  it does not depend on Mantle PostalAddress. User/device assignments carry the location used for a call.
- `SipDevice` {sipDeviceId, userId, username, realm, credentialRevision, statusId}; encrypted digest verifiers and
  expiring `SipRegistration` bindings are separate children (§5.2).
- `RingGroup` and `RingGroupMember` {strategy, ring seconds, fallback}.
- `IvrMenu` and `IvrOption`.
- `CallQueue` and `CallQueueAgent`.
- `CallHandlingRule` {userId, kind (always, busy, noAnswer, selectiveForward, selectiveReject, selectiveAccept,
  anonymousReject), target, callerListId, scheduleJson, sequenceNum}.
- `CallerList` and `CallerListEntry`.
- `VoicemailBox` {userId or ringGroupId, pinHash, greetingLocation, notifyEmail, attachAudio, attachTranscript, rings}.
- `Voicemail` {voicemailId, voicemailBoxId, callSessionId, fromNumber, receivedDate, durationMs, audioLocation, heardDate,
  folder (inbox, trash), transcriptText}.

A finished call writes no Mantle record. BOSS's `Call` CommunicationEventType comes from a SECA on the framework's
`end#Call`, in Mantle (design §14). The subscriber persists idempotent work keyed by callSessionId in the same transaction;
an after-commit callback only wakes it. The framework doesn't call Mantle; Mantle subscribes.

Call services enforce an explicit state machine: sessions progress from ringing to active to ended, or from ringing
directly to ended; legs retain offered, answered, declined, canceled, timed-out and failed outcomes as appropriate.
Idempotency keys and revision checks make repeated start/join/answer/leave/end requests safe. The first successful
answer for a ringing invitation wins atomically and cancels sibling offers, including other tabs and forked SIP legs.
Presence derives from the user's active legs, so leaving one of several calls does not clear `UserStatus.inCall`.
Cleanup expires abandoned connections and completes the affected history once; it does not delete call history.

Recording is off by default. When enabled, each participating leg receives the notice and acknowledges it (browser
action or phone prompt with DTMF). Before recording starts, or a new/transferred leg contributes media, all required
acknowledgements for the current recording scope must exist. Missing acknowledgement or withdrawal pauses recording;
the ordinary call can continue. Restarting after a pause creates a new segment. Only authorized users may record,
download or read transcripts/voicemail, including through direct resource URLs. BOSS record links do not widen those
rights. Retention deletes audio/video, derived transcripts, previews and pending processing work; recovery jobs clean
up interrupted uploads and partial segments. Email attachments are optional disclosures, off by default, which cannot
be recalled by server retention. Recording and transcription policy is checked before external processing begins.

### 5.5 Screens (framework comms)

| Screen | Path | Purpose |
| --- | --- | --- |
| Calls | `Comms/Calls` | Tabs: All, Missed, Voicemail, Call links, Meetings (the meetings hook). Call back, play, the actions |
| Dialer | `Comms/Calls/Dialer`, also a tray panel | Keypad, search for a person or number, caller ID picker, call |
| Call | `Comms/Calls/Call`; guests at `/call/<token>` (D-A5) | Lobby and call view (§6.6). For a phone call: audio only, keypad for DTMF, hold, transfer, add, merge, record |
| LiveCall | the `q-page-sticky` slot | Mini window while in a call, across navigation |
| Incoming call | shell | Ring bar in every tab of the user (on `chatUser:<id>`), with Answer, Decline, Message. Permission-dependent web push and missed-call recovery (P0, §7) |
| VoicemailDetail | `Comms/Calls/Voicemail` | Player, transcript, the actions |
| Settings/Calls | | Devices (kept in the browser), ringtone, join muted, camera off, blur |
| Settings/Phone | | My numbers, call handling rules, voicemail (greeting, PIN, notices), my desk and soft phones (credentials shown once) |
| Admin: Voice | tools `System/Voice` (D-A6) | Trunks with status, numbers, ring groups, IVR menus, queues, the emergency setup, media and SIP status |
| Admin: Bridges | tools `System/ChatBridges` | Bridges with status and errors, bridged rooms, connector setup |

Every screen gets its TUI twin. In the terminal client a call is not live: the twin shows history, voicemail
transcripts and settings.

### 5.6 SMS

SMS and MMS go through the voice provider's messaging REST API and webhooks (Telnyx first, D-V4).
`SmsServices.send#SmsMessage` becomes the real sender. Inbound SMS to a user's number lands in an `external` chat room
(§4.3). Marketing's SMS sends (mr-boss research-marketing) use the same sender with consent gating in Mantle.
SMS requires C1's delivery layer and a configured messaging provider/number, not the V1 SIP engine. Provider
registration and consent constraints remain enforced even when the caller uses the legacy `SmsServices` entry point.

### 5.7 Libraries

| Need | Candidate | License | Notes |
| --- | --- | --- | --- |
| SIP stack | `rsipstack` (transaction, dialog and transport layers on `rsip`) | verify selected version | first candidate under D-V1; Tokio |
| SIP stack, sans-IO | `rsiprtp` (audio UA, its own parser) | check | sans-IO like `rtc`, but no registrar or proxy roles |
| SIP, wider | `rvoip` (SIP core, transport, transactions, call engine, RTP) | check | younger and broad |
| SIP parser | `rsip` | MIT | under rsipstack |
| Opus | `opus` / `audiopus_sys` over libopus | crate MIT or Apache; libopus BSD-3-Clause | a C build (cc) |
| G.711, G.722 | in house | — | small |
| Resampling | `rubato` | MIT | 8 and 16 kHz to 48 kHz |
| Text to speech for IVR prompts | `moqui-llm` binding (a new `speech` feature) or recorded prompts | — | recorded prompts first |

These are candidates, not locked or interoperability-tested dependencies. V1 records the selected versions, package
and transitive licenses, release age and `deny.toml` result before adoption. Its spike proves trunk registration,
calls each way, DTMF, re-INVITE hold, cancellation/fork races, media mixing and credential handling with an in-process
fake trunk. A separately authorized live-provider check proves the documented deployment profile. An unavailable
live check is recorded as an unmet release gate, not as a passing interoperability claim.

---

## 6. Web conferencing

### 6.1 Approach

The SFU is in from the first call: every WebRTC call, a two-person one too, goes through `moqui-rtc`. There is no mesh
mode. This gives one media path for recording, captions, SIP legs, quality data and guests, and it hides each
participant's media address from the others when signaling exposes only the SFU's candidates. The SFU still sees
connection addresses. Budget bandwidth and CPU for the configured codecs, layers and participant limits; one-to-one
video also uses server egress. The BOSS design's "mesh for small calls, an SFU later" (§19) is replaced.

### 6.2 Library: webrtc-rs's sans-IO stack

**`rtc`** is the sans-IO WebRTC protocol core; **`sfu`** supplies room forwarding on that core. The **`webrtc`** crate
wraps the core with async I/O. The review inspected the public API and source manifest, not a working Moqui build;
the observed versions and license evidence are recorded in §10.3.

Why sans-IO fits here:
- `moqui-host` owns sockets and Tokio tasks.
- No business logic runs on Tokio (the execution model in [Messaging](proposals/Messaging.md#existing-execution-model)).
- A sans-IO stack lets the host drive sockets and timers without business logic in the packet loop. Start with a
  bounded single-node driver; measure capacity before promising all of a node's rooms fit on one task.
- Tests run two peers in memory with no sockets, which keeps them fast.

W1 first evaluates `sfu` on `rtc`, with **str0m** as D-W1's fallback. It must demonstrate two-party and five-party
calls, simulcast layer switching/TWCC, renegotiation, ICE restart, and extraction/injection of media for SIP mixing,
recording and captions. The [reviewed SFU API][sfu-api] handles forwarded RTP internally rather than surfacing it to
the caller; prove a supported integration path instead of assuming endpoint support supplies those hooks. A str0m
fallback includes the forwarding/scheduling work it does not supply as an SFU. Keep this seam inside `moqui-rtc`.
Record exact versions, release age under the two-week rule, licenses and the capability results before selection.
If neither candidate meets the gate, dependent features remain blocked; do not silently drop their requirements.

### 6.3 Signaling

- **Frames.** A new client frame kind on `/notws`, `rtc`, handled in `crates/moqui-host/src/notws/rtc.rs` beside
  `collab.rs`. The frames:
  - `join`, `leave`, `offer`, `answer`, `candidate`;
  - `publish` and `unpublish` a track (camera, microphone, screen);
  - `subscribe` with the layer wanted (by tile size);
  - `mute`, `hand`, `reaction`.

  The server sends:
  - `joined`, `answer` and `offer` (renegotiation), acknowledgements and structured errors;
  - `participant` updates;
  - `activeSpeaker`;
  - `quality`;
  - `ended`.
- **Why the socket and not HTTP.** ICE candidates arrive in bursts, and a service call per candidate, with a
  NotificationMessage after its commit (`push#Signal`), adds latency and database work for nothing stored. Frames go
  straight to the node holding the call's media.
- **Writes stay services.** `start#Call`, `join#Call`, `leave#Call`, `admit#Guest` and `end#Call` are services that
  write the call state (§5.4). Socket `join` binds an already admitted leg to a connection; it cannot admit anyone.
  Socket `leave` requests the same leave service. Signaling handlers verify call, leg, connection generation and
  publish/subscribe/moderation permissions; an SDP or candidate frame grants no authority on its own.
- **Guests.** The public call-link page exchanges its high-entropy bearer token for a short-lived guest session,
  using the stored token hash and checking expiry, revocation and link policy. A waiting guest gets only lobby
  status; admission exchanges that state for a credential bound to call, participant, leg and connection generation.
  A restricted `/notws` authentication branch accepts it solely for that call's RTC frames and control updates;
  it cannot use `ALL`, general notification topics, co-editing or room history. Credentials are not query parameters
  or log fields. Link/participant revocation invalidates live credentials and removes the leg on the media node.
  Guests have no chat access by default; any call-chat feature needs an explicit, bounded call-only view.
- **Connections.** A reconnect revalidates admission, claims a new generation and fences the old connection. Heartbeats
  expire abandoned connections after a documented grace period; stale frames cannot resurrect a left or revoked leg.
  Media negotiation handles offer collisions and candidates for obsolete ICE generations. Room resources close after
  the last live leg's grace period, while call history remains. An atomic answer service settles competing tab/device
  answers, and all sibling offers receive the terminal outcome (§5.4).
- **Ringing.** It rides `chatUser:<id>` as today's room events do.
- **Cluster.** A call's media lives on one node (`CallSession.mediaNodeId`). A frame from a page on another node is
  sent to that node over authenticated cluster transport, not broadcast to every member. The envelope carries request
  ID, origin node/connection, call/leg, connection generation and media generation; replies return to that exact
  origin connection. The target rechecks the admitted binding. Bounded per-connection and per-node queues reject
  overload explicitly; ephemeral signaling is renegotiated after reconnect, not stored as chat events.
  The browser's media goes straight to the media node's public address. A stale/dead media owner is fenced; its active
  legs are ended with a visible media-node-lost reason and presence/history are reconciled. Users can start a new call.
  Seamless media failover is outside W1; cascading SFUs is W7 and does not imply failover.

### 6.4 Media features

- **Codecs.** Opus with DTX and FEC; VP8 with simulcast (three layers) first. VP9 or AV1 SVC and H.264 for Safari's
  hardware come later.
- **Congestion control.** Transport-wide congestion control (TWCC) bandwidth estimation, layer switching per
  subscriber, and an audio-only fallback.
- **Speakers.** Audio-level header extension for the active speaker and the speaker view.
- **Screen share.** `getDisplayMedia`, as a separate track at a high resolution and low frame rate.
- **Data channel** for reactions and hands, if the socket's latency is not enough.
- **Captions** from the audio, through `moqui-llm` `transcribe` in chunks, as a later step (W6).
- **Recording** (W5):
  - per-track Opus and VP8 into WebM without transcoding, with track/segment records under `CallRecording`;
  - mixed recordings later use a dedicated recorder process/session with scoped credentials, isolated from the PDF
    renderer's browser and queue. Nextcloud's recorder uses browser participation plus a capture process; reusing a
    Chromium executable does not mean sharing the PDF worker. An external ffmpeg build has its own license review;
  - §5.4's notice, acknowledgement, access and retention rules apply. Recorder failure ends that recording, not the
    call; failed segments are marked and partial resources cleaned up.
  Captions and recording have independent bounded work queues, timeouts and resource limits. A slow transcription
  provider cannot stall forwarding; show captions unavailable and continue the call. Provider disclosure follows the
  configured consent policy, and both features refuse encrypted media.
- **End-to-end encryption** (W8, D-W3): SFrame over supported browser transforms. Use a dedicated call-key protocol
  tied to the authenticated admitted roster, with per-sender keys/counters, rotation on join/leave/revocation and
  reconnect handling; never reuse the chat Megolm session as a media key. Verify the chosen key-management integration
  with an independent implementation before release. The server relays encrypted key material but cannot decrypt it.
  E2EE-required calls reject unsupported clients, recording, captions and SIP legs without downgrade. Chat and call
  encryption modes remain separately visible.

### 6.5 Network

- **Media UDP 10000** is the default shared ICE/DTLS-SRTP port, with an explicit bind address and `public-address` for
  a node behind 1:1 NAT. ICE-lite requires a reachable, correctly advertised media endpoint; verify NAT routing rather
  than assuming a configured public address proves it.
- **Built-in TURN (W4)** defaults to UDP/TCP 3478 and TLS 5349. For UDP-blocking networks offer TLS 443 on a dedicated
  bind address; the existing HTTPS listener does not demultiplex TURN. Same-address 443 sharing would need a separate
  proven transport demultiplexer and is outside W4. The [Nextcloud TURN guide][talk-turn] describes the port conflict.
- **Relay allocations** have an explicit relay address/port range and firewall rules, separate from the SFU media
  port. W4 verifies client-to-TURN and TURN-to-media-node reachability, including NAT hairpin behavior. Coturn remains
  a documented external alternative with the same acceptance scenarios.
- **Credentials and limits.** Issue short-lived HMAC TURN credentials only to admitted legs, including guests; enforce
  allocation/bandwidth quotas and allowed media destinations. Standard TURN credentials do not themselves restrict
  relay peers to a call: the embedded server enforces the binding, and the coturn profile restricts relay destinations
  to media nodes with expiry/quotas. Stop renewing revoked credentials and remove embedded allocations. Validate
  refusal of arbitrary relay targets as well as successful UDP-blocked calls.

### 6.6 Screens

The design's Call screen (mr-boss design-drawers-commons §14) moves to the framework unchanged in shape:
- a lobby with camera preview, device pickers, level meter and the guest name;
- grid and speaker views;
- the bottom bar: mute, camera, share, hand, reactions, participants, chat (authorized room members only), settings, record and
  leave;
- the admit prompt;
- the LiveCall mini window.

`m-call` and its parts are framework components, so any app can embed a call. A Customer Service video session or a
Projects stand-up embeds the room's call by `chatRoomId`; embedding never bypasses call or room authorization.

---

## 7. Order

Each step records its checks and documentation before being called complete. Dependencies allow independent work to
proceed in parallel; P0 precedes A0 as agreed. W1/V1 start with feasibility spikes before their library choices are
used by dependent steps. A development call on a reachable local network is not a public-network release: W3 and
V1 require W4's connectivity acceptance before public use. All new listeners stay off until configured.

### 7.1 Milestones and acceptance

| Step | What | Depends on | Observable acceptance |
| --- | --- | --- | --- |
| **P0** | PWA install and web push | — | Install, permission refusal, expired subscription, logout/account switch and recovery after missed push work without caching private responses (§7.2) |
| **A0** | Hooks, actions, panels and shell-provider merge | P0 | Two contributions coexist; replacement/order is deterministic; failing optional reads are isolated; unauthorized direct writes fail in browser and TUI |
| **A1** | Component/mount and Settings extraction | A0 | Framework-only mail settings work without Internal accounts or Mantle; existing account IDs/rules/OAuth routes remain usable |
| **A2** | Chat and ChatPanel extraction | A1 | Framework-only chat works; BOSS record features appear only with authorized providers through either mount |
| **A3** | Inbox, MessageDetail, Compose and Home split | A2 | Email-only framework; BOSS Internal accounts/mailboxes, recipient lookup, filing and Home contributions retain behavior and pagination |
| **A4** | Extensions, split specs and Comms guide | A3 | One launcher entry, both mounts authorized, qvt/qvue and TUI renders checked, no framework Mantle/ironstone dependency |
| **W1** | Media spike, SFU, signaling, call state, Call/LiveCall and ringing | A2 | §6.2 capability gate; 1:1/group calls, competing answers, stale generations, cross-node replies, reconnect and media-node loss |
| **W2** | Simulcast, active speaker, screen share and history | W1 | Layer changes under constrained bandwidth, share start/stop and audio fallback; ended/missed history stays correct |
| **W3** | Call links, guests, lobby and meetings hook | W2 | Waiting/revoked guests cannot negotiate media or read room history; expiry, admission and reconnect remain scoped; W4 before public release |
| **W4** | Embedded TURN and coturn deployment profile | W1 | UDP-blocked TLS relay, advertised-address/NAT checks, quotas, expiry, forbidden relay targets and dedicated-address 443 profile |
| **V1** | SIP spike, trunks, mixer, caller ID and emergency readiness | W1 | Fake-trunk cases in §5.7, authorized caller ID, mix-minus audio; provider profile and emergency readiness documented; W4 before public release |
| **V2** | Voicemail, greetings, PIN, notices and transcription | V1 | Busy/no-answer voicemail, PIN refusal/rate limits, private media access, optional notices; transcription failure preserves audio |
| **V3** | Handling rules, ring groups, DND, presence and dialer | V2 | Ordered/simultaneous ringing, fallback, schedules, first-answer race; one ended leg does not clear another active call's presence |
| **C1** | Actor migration, ingestion, durable bridge delivery | A2 | Migration preserves local behavior; crash before/after remote acceptance, duplicate batches, reordered edits, echo loops and one failing destination |
| **C2** | Matrix appservice | C1 | Transaction retries, state/ID mapping, membership permissions and media; encrypted portals refused; independent homeserver interoperability |
| **C3** | XMPP server and OMEMO transport | C2 | Recorded stanzas plus selected clients; MUC/MAM/stream resume, PEP bundle access and OMEMO namespaces; advertised capability set matches results |
| **C4-SMS** | SMS/MMS sender and webhooks | C1 | Configured provider/number without SIP engine; duplicate callbacks, receipts, media limits, uncertain sends and consent refusals |
| **C4** | WhatsApp Cloud API and Telegram | C1 | Signature/replay failures rejected; retries and provider permissions/window/template limits; unsupported mutations refused |
| **V4** | SIP registrar | V1 | Multiple bindings, expiry, revocation, nonce replay, credential rotation and correct routing to live contacts |
| **V5** | IVR, queues, transfer and merge | V3 | DTMF routing, queue timeout/wrap-up, transfer/answer races and recording consent for new legs |
| **W5** | Recording | W2 | Per-leg acknowledgement, late join/withdrawal pause, multiple tracks/segments, recorder failure, private downloads and retention |
| **W6** | Captions | W2 | Speaker/chunk ordering, provider timeout/backpressure and consent; failure cannot stall media; encrypted media refused |
| **C6** | Native Matrix homeserver and migration | C2 | Independent client/server tests and §4.3 migration/rollback rehearsal; C2 remains available until replacement is proven |
| **C7** | Encrypted rooms | C1 | Browser storage, encrypted attachments, concurrent one-time-key claims, revoked devices, rotation, lost-device recovery and no plaintext side effects; Matrix client interop also requires C6 |
| **C5** | Slack and Teams | C1 | Versioned capability matrix, authorization and retry cases for each connector |
| **W7** | Cascading SFUs | W2 | Cross-node subscriptions, bounded forwarding and node-loss outcomes; no implied seamless failover |
| **W8** | Encrypted calls | W2, C7 | Independent key-protocol interop, roster changes/reconnect, unsupported-client refusal, no downgrade or recording/captions/SIP |

### 7.2 PWA and notification behavior

P0 installs the manifest and service worker and registers push subscriptions per signed-in user/device after a user
gesture and permission. [WebKit's push requirements][web-push] include visible notifications; push is not a promise
of uninterrupted background ringing or automatic media access. Record supported browser/OS versions and limitations.
Handle expired/rejected endpoints, unsubscribe on logout, and prevent subscriptions crossing an account switch.
Cache static shell assets only; exclude authenticated API responses, messages, recordings and guest credentials.
Encrypted-room notifications contain no plaintext. W1 adds expiring call invitations: an old push opens current call
state or missed-call history, never revives a completed invitation. On reconnect, clients refresh durable unread and
call state rather than treating push delivery as authoritative. Absence of push permission does not disable normal
in-app notifications or history.

### 7.3 Verification and rollout

- **Fast checks.** Use focused Rust tests with simulated clocks/in-memory peers and Moqui server/client specs; no Node
  test dependency. Include negative authorization cases with the same valid parameters as the positive case. Cover
  crash/retry boundaries and migrations as behavior, not assertions that mirror implementation. Do not promise a
  runtime until measured; keep ordinary tests well under 15 seconds.
- **Browser and interoperability.** Fake-media Chromium can prove the application flow; independent clients and
  provider-approved live checks prove selected interoperability and deployment cases. Browser, real-provider,
  interoperability and other slow runs require explicit permission for the specific scenario when its result is
  needed; this plan grants no blanket permission. A test over about 15 seconds is slow; one over 30 seconds must be marked as
  prescribed in [Build-Test](../docs/Build-Test.md#slow-tests). Never pass `--slow` without explicit permission.
  Record versions, topology, results and unverified cases; lack of permission leaves a gate unverified, not passed.
- **Rollout.** Pilot each connector/listener with bounded users and rooms, keeping feature switches off elsewhere.
  Monitor pending-delivery age/failures, admission failures, stale connections, media loss/jitter and resource limits,
  and recording/caption failures without message content, credentials or high-cardinality participant labels.
  Disabling a connector retains pending delivery state; draining a media node stops new admission before active calls
  finish. Actor and homeserver migrations require backups and rehearsed rollback (§§4.3–4.4).
- **This documentation revision.** Check links, section/decision references, dependency acyclicity, schema terminology
  and the framework/Mantle boundary. It adds no application code or tests and does not satisfy the implementation gates.

---

## 8. Decisions

Walked through with the user on 2026-10-30. The review preserves that direction. The table reflects the accepted
refinements below; neither a candidate library nor a release gate is described as implemented or verified.

| # | Decision | Decided |
| --- | --- | --- |
| D-A1 | The `.qvue` extension model (§3.3) | The three layers: hook services, declared actions, panel slots |
| D-A2 | What the framework Inbox holds without Mantle | Email accounts only. Internal messages stay Mantle's and come through the inbox source hook |
| D-A3 | The services package | `org.moqui.comms.*` for the screen-facing services. The record services stay `org.moqui.impl.*` |
| D-A4 | Comms Home | A framework Home whose cards come from providers (unread by account, mentions, missed calls, voicemail). BOSS adds meetings and mailboxes |
| D-A5 | Where the guest call page (`/call/<token>`) goes, given the Oct 28 rule that public pages go to mr-pass | The framework's public shell, since the framework owns calls. mr-pass can link to it |
| D-A6 | Where the voice and bridge admin screens live | Under the tools component's `System` app (§5.5) |
| D-C1 | Which federation first | The Matrix appservice (C2), then the XMPP server (C3) |
| D-C2 | A native Matrix homeserver in Moqui | Planned as a later step, C6, after the appservice. Replacement of the external homeserver requires the migration gate (§4.3) |
| D-C3 | XMPP library license | A `deny.toml` MPL-2.0 exception for `xmpp-parsers`, `minidom` and `tokio-xmpp` used unmodified, with a License-Review row |
| D-C4 | Personal WhatsApp, Signal and others through the mautrix bridges | A documented deployment option: separate AGPL processes, and the networks' terms are the operator's. The official Business API is the supported path |
| D-C5 | End-to-end encryption in chat | Olm/Megolm, preferring matrix-sdk-crypto over vodozemac for the device lifecycle (C7, §4.5). C3 supplies OMEMO transport plus PEP/pubsub support, subject to client interoperability |
| D-V1 | The SIP stack | Chosen by the V1 spike, `rsipstack` first |
| D-V2 | Emergency calling | Only through a trunk marked emergency-capable, with registered addresses. Otherwise refused with a notice |
| D-V3 | Video to SIP endpoints | No. SIP legs are audio |
| D-V4 | The first trunk and SMS provider | Telnyx, for docs and authorized live checks. Other SIP trunks need a verified profile; SMS does not depend on the SIP engine |
| D-V5 | Recording consent | Off by default. Every leg is notified and acknowledges the recording scope; missing acknowledgement or withdrawal pauses recording, including late joins/transfers (§5.4). Deployment policy must meet its applicable rules |
| D-V6 | PWA and web push | Built first, as step P0, before A0 (§7) |
| D-W1 | SFU library | webrtc-rs `sfu` on `rtc` first, subject to the expanded W1 capability gate, with str0m the fallback |
| D-W2 | The SFU for every call, no mesh | Yes |
| D-W3 | End-to-end encryption for calls | Later (W8), chosen per call or room policy, with dedicated call keys. Required E2EE refuses unsupported clients, recording, captions and SIP legs |
| D-W4 | Embedded TURN or coturn | Embedded, with coturn documented |

Review refinements accepted for this revision (2026-10-30):

| ID | Refines | Required clarification |
| --- | --- | --- |
| R1 | C1 data model | Stable actor identity and migration; no nullable-userId key or remote login account (§4.4) |
| R2 | C1 delivery | Authorized ingestion and transactional durable delivery; after-commit is only a wakeup (§4.1) |
| R3 | D-A5, W1/W3 | Restricted guest credentials, separate participant/connection/leg state and targeted cluster routing (§§5.4, 6.3) |
| R4 | D-W1, D-W4 | Demonstrate media integration hooks; fixed port layout and dedicated-address TURN TLS 443 (§§6.2, 6.5) |
| R5 | D-C5, D-W3 | Browser crypto lifecycle and attachments; dedicated call keys and no silent downgrade (§§4.5, 6.4) |
| R6 | D-C2, D-C3, D-C4 | Matrix migration gate, explicit adapter capabilities and OMEMO dependencies; bridge trust boundary (§4.3) |
| R7 | D-V2, D-V4, D-V5 | SIP bindings/credentials, per-leg outcomes and recording consent, access/retention and emergency readiness (§5) |
| R8 | D-A1, D-A2, D-A4 | Complete extension contracts, Internal settings split, framework Home and both-mount access checks (§3) |
| R9 | D-V6 and plan status | Correct dependencies, push limits, sourced versions/licenses and known mail limitations (§§1, 7, 10) |

## 9. Risks

- **Media and SIP integration.** A release label does not prove capability, stability or a usable recording/mixer seam.
  W1/V1 decide on recorded results; the crate boundaries keep a swap local but do not remove the fallback's work.
- **Real networks.** NAT, UDP-blocking firewalls and carrier quirks need the W4 and provider-profile checks. A passing
  local fake-media call is not evidence for those cases.
- **Regulation.** Emergency calling, recording consent, SMS registration (10DLC in the US) and WhatsApp's template rules
  need deployment-specific readiness. The implementation supplies enforceable controls and notices; those notices
  alone do not establish that the operator meets the rules.
- **The size of the protocol surface.** XMPP and Matrix each have long tails. The plan targets what common clients need
  using a declared capability/version matrix, and explicitly refuses unsupported behavior.
- **Running a homeserver beside Moqui** for Matrix phase 1 adds a process to deploy. Run-Deploy gets the steps.
- **Recovery and retention.** A failed destination must not lose accepted messages or stall unrelated destinations;
  recordings and encrypted attachments need quotas and cleanup. Dedupe metadata must outlive retry windows without
  retaining message content beyond policy. Cross-node signaling does not provide transparent media failover.

## 10. Review and sources

Reviewed 2026-10-30 against the current framework and local application checkouts. Nine findings (seven high, two
medium) are addressed in this design revision; implementation and interoperability remain open. R1–R9 in §8 preserve
the link from the findings to the agreed refinements. This is a design comparison, not a claim that the reference
applications are free of defects or that their code can be copied into Moqui.

### 10.1 Findings and disposition

| ID | Severity | Evidence in the original plan or current code | Revision |
| --- | --- | --- | --- |
| R1 | High | [ChatEntities.xml][chat-entities] keys membership by room/user and references local senders; an alternative nullable external-user field cannot replace that key | Actor model and migration, §4.4 |
| R2 | High | [append#Event][chat-services] requires caller access checks; [register_tx_rules][service-call] schedules in-memory work after commit | Authorized ingestion, durable acceptance/delivery and retries, §4.1 |
| R3 | High | [notws.rs][notws] requires a local user; original call schema had no guest credential or reconnect ownership | Participant/leg/connection lifecycle, guest scope and targeted routing, §§5.4, 6.3 |
| R4 | High | Original §6.5 shared TURN with HTTPS without a transport design; SFU forwarding API alone does not establish mixer/recorder access | Network layout and expanded spike, §§6.2, 6.5 |
| R5 | High | Original §4.5 specified algorithms but omitted attachment/device lifecycle; W8 reused a chat session concept | Client crypto state, privacy limits and separate call keys, §§4.5, 6.4 |
| R6 | High | Original C6 equated namespaces with migration; C3 described OMEMO as pass-through alone | Protocol state, migration rehearsal and capability/client matrix, §4.3 |
| R7 | High | Original §5.4 had one SIP contact and call-level consent, with no recording/track IDs | SIP bindings/credentials, outcomes, consent, retention and emergency readiness, §5 |
| R8 | Medium | [screen extension merge][screen-definition] lacks provider merging; [MailSettingsServices][mail-settings] already handles Internal accounts; contact predicate already exists | Explicit extension contracts and corrected extraction/mount behavior, §3 |
| R9 | Medium | Home/order text contradicted §8; baseline inCall field and Zimbra license were misidentified; versions and mail readiness lacked qualification | Corrected baseline, dependencies, push limitations and evidence, §§1, 7–10 |

Checked and retained: the Mantle-free framework boundary; the three extension layers; Zimbra's role as a UC front
end; SFU from the first call; embedded TURN with coturn available; SIP audio only; Matrix appservice before native
homeserver; and the existing permission-required slow-test policy. No architectural alternative supersedes §8.

### 10.2 Local application comparisons

Paths below are relative to this plan. The revision column records each checkout's HEAD; the review read local
working-tree sources and did not build or run these applications. Follow-up reviews should record their own snapshots.

| Reference and checkout HEAD | Source inspected | Applied lesson |
| --- | --- | --- |
| Zimbra web client `a868c61` (2026-10-12) | [ZmCallFeature.js][zimbra-feature], [ZmPhone.js][zimbra-phone]; inspected headers are CPAL-1.0 | Supported/subscribed versus active voice features; capability-driven settings, §§3.1, 5.3 |
| Zimbra mailbox `9121b24` (2026-10-14) | [VoiceInfo.java][zimbra-voice-info] (GPL-2.0 header), [INSTALL-VOICE.txt][zimbra-install] | Cisco/Mitel extensions supply the telephony engine; Zimbra's UI is not evidence for a native SIP engine, §5.3 |
| Nextcloud Talk `0ab8957` (2026-10-17) | [Attendee.php][talk-attendee], [Session.php][talk-session], [CallController.php][talk-call], [TURN.md][talk-turn], [matrix.md][talk-matrix] | Separate actors from live sessions, gate admission/consent, document network constraints. Its Matrix integration decrypts on the server; Moqui's browser E2EE deliberately keeps a different trust boundary |
| Nextcloud recording `bf25bf7` (2026-10-16) | [Participant.py][talk-recorder], [RecorderArgumentsBuilder.py][talk-recorder-args] | Dedicated browser participant plus capture process; isolate recording from PDF workloads, §6.4 |
| Odoo `8ff0e720` (2026-10-17) | [message_actions.js][odoo-actions], [discuss_channel_rtc_session.py][odoo-rtc] | Named/ordered actions and stale-session cleanup separate from history, §§3.3, 5.4 |
| Frappe Suite `b741fd1` (2026-10-16) | [RoomJoinHandlers.ts][frappe-join], [RoomLifecycleCoordinator.ts][frappe-lifecycle] | Explicit participant/connection ownership, reconnect replacement and delayed resource cleanup, §6.3 |

### 10.3 Protocol and dependency evidence

These are review observations, not selected build dependencies. Moving upstream branches and `latest` specifications
can change. At each spike, record an immutable source/package version, release date, license and transitive review,
and the actual acceptance results. A manifest declaring a version does not prove a published release or its age.

| Evidence inspected on 2026-10-30 | Observation and use |
| --- | --- |
| [Matrix application-service API][matrix-as] | Transaction IDs, retries and durable acknowledgment motivate §4.1; pin the implemented specification version at C2 |
| [XEP-0384][omemo], version 0.9.1 (2026-04-06), Experimental | PEP/pubsub dependencies and client namespace compatibility are part of C3 |
| [SFU repository API][sfu-api] and [master Cargo.toml][sfu-manifest] | Manifest declares sfu 0.5.0, rtc dependency 0.21.0, MIT OR Apache-2.0 for sfu. This replaces the old blanket rc-status claim; W1 still verifies published versions, all dependency licenses and media hooks |
| [matrix-sdk-crypto API 0.19.1][matrix-crypto] | Client crypto state machine with WASM support flags; prefer it to reconstructing the lifecycle from vodozemac. Browser persistence and package/transitive licenses remain C7 gates |
| [RFC 8760][sip-digest] and [RFC 7616][http-digest] | SIP Digest algorithms and realm-specific verifier storage, §5.2 |
| [911.gov MLTS guidance][emergency-guidance] | Direct dialing, notification, callback/location readiness; deployment-specific review remains required, §5.2 |
| [mautrix encryption guide][mautrix-encryption] | Encryption may end at a bridge process; do not describe it as Moqui browser-only E2EE, §4.3 |
| [WebKit Web Push][web-push] | Permission and visible-notification requirements; push is not the durable call state, §7.2 |

[chat-entities]: ../runtime/base-component/framework/entity/ChatEntities.xml
[chat-services]: ../runtime/base-component/framework/service/org/moqui/impl/ChatServices.xml
[service-call]: ../crates/moqui-core/src/service/call.rs
[notws]: ../crates/moqui-host/src/notws.rs
[screen-definition]: ../crates/moqui-core/src/screen/definition.rs
[mail-settings]: ../runtime/component/mr-boss/service/ironstone/comms/MailSettingsServices.xml
[zimbra-feature]: ../../3p_other/Zimbra/zm-web-client/WebRoot/js/zimbraMail/voicemail/model/ZmCallFeature.js
[zimbra-phone]: ../../3p_other/Zimbra/zm-web-client/WebRoot/js/zimbraMail/voicemail/model/ZmPhone.js
[zimbra-voice-info]: ../../3p_other/Zimbra/zm-mailbox/soap/src/java/com/zimbra/soap/voice/type/VoiceInfo.java
[zimbra-install]: ../../3p_other/Zimbra/zm-mailbox/store/docs/INSTALL-VOICE.txt
[talk-attendee]: ../../3p_other/nextcloud/spreed/lib/Model/Attendee.php
[talk-session]: ../../3p_other/nextcloud/spreed/lib/Model/Session.php
[talk-call]: ../../3p_other/nextcloud/spreed/lib/Controller/CallController.php
[talk-turn]: ../../3p_other/nextcloud/spreed/docs/TURN.md
[talk-matrix]: ../../3p_other/nextcloud/spreed/docs/matrix.md
[talk-recorder]: ../../3p_other/nextcloud/nextcloud-talk-recording/src/nextcloud/talk/recording/Participant.py
[talk-recorder-args]: ../../3p_other/nextcloud/nextcloud-talk-recording/src/nextcloud/talk/recording/RecorderArgumentsBuilder.py
[odoo-actions]: ../../3p_other/odoo/odoo/addons/mail/static/src/core/common/message_actions.js
[odoo-rtc]: ../../3p_other/odoo/odoo/addons/mail/models/discuss/discuss_channel_rtc_session.py
[frappe-join]: ../../3p_other/frappe/suite/suite/meet/sfu-server/src/server/handlers/RoomJoinHandlers.ts
[frappe-lifecycle]: ../../3p_other/frappe/suite/suite/meet/sfu-server/src/server/RoomLifecycleCoordinator.ts
[matrix-as]: https://spec.matrix.org/latest/application-service-api/
[omemo]: https://xmpp.org/extensions/xep-0384.html
[sfu-api]: https://github.com/webrtc-rs/sfu
[sfu-manifest]: https://raw.githubusercontent.com/webrtc-rs/sfu/master/Cargo.toml
[matrix-crypto]: https://docs.rs/matrix-sdk-crypto/0.19.1/matrix_sdk_crypto/
[sip-digest]: https://www.rfc-editor.org/rfc/rfc8760.html
[http-digest]: https://www.rfc-editor.org/rfc/rfc7616.html
[emergency-guidance]: https://www.911.gov/issues/legislation-and-policy/kari-s-law-and-ray-baum-s-act/
[mautrix-encryption]: https://docs.mau.fi/bridges/general/end-to-bridge-encryption.html
[web-push]: https://webkit.org/blog/12945/meet-web-push/
