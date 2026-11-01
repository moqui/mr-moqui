# Comms Framework

Status 2026-10-31: P0 (PWA and Web Push), A0–A4 (the extension model and BasicComms), W1 and W2 (calls on the embedded
SFU), W3 (call links and guests), W4 (TURN and ICE-TCP), M0 (the audio core), W5 (recording), W6 (captions), V1–V5
(phone calls: SIP trunks, voicemail, call handling, the registrar, the IVR, queues, transfer and merge), C1 (chat actors
and bridges), C7 (core: crypto devices and keys), W7 (cascading media nodes) and W8 (end-to-end encrypted calls) built;
see [§7.4](#74-built). Next: the rest of C7 (encrypted rooms, key backup, encrypted attachments, verification), then
C4-SMS, C4, C3a, C3b, C6a, C6b and C6c.
The decisions are in [§8](#8-decisions). The libraries, reference applications and review findings are in
[§10](#10-libraries-and-sources).

Comms moves from mr-boss into a framework component, `runtime/base-component/basic-comms`, beside `tools`. It gains
three things it does not have:

- chat that reaches outside Moqui (federation and bridges);
- voice over SIP and the phone network, with voicemail;
- web conferencing over WebRTC, with a selective forwarding unit (SFU) from the first call.

Each runs in the moqui binary; external servers are options, not requirements (§2).

The framework app is BasicComms. IronStone Comms in mr-boss stays the full app. It mounts the BasicComms screens and
adds BOSS's parts by screen-extend and providers (§3.4).

Email moves with the rest; this plan adds no new mail features. Its known limitations, and the outstanding live sync
and OAuth checks, remain in [Remaining-Work, Mail](Remaining-Work.md#mail).

Sections: [1 starting point](#1-starting-point) · [2 scope](#2-scope) · [3 the framework Comms app](#3-the-framework-comms-app) ·
[4 chat protocols](#4-chat-protocols) · [5 voice](#5-voice) · [6 web conferencing](#6-web-conferencing) ·
[7 order](#7-order) · [8 decisions](#8-decisions) · [9 risks](#9-risks) · [10 libraries and sources](#10-libraries-and-sources)

---

## 1. Starting point

What is built (mr-boss `plan/impl-drawers-commons.md` §5 M5, M10 and §8; [Remaining-Work](Remaining-Work.md#chat-and-presence)):

- **Mail.**
  - `moqui-mail`: SMTP, and the IMAP and POP3 sync with IDLE and OAuth sign-in.
  - The host's SMTP and LMTP listener, behind the `inbound-smtp` feature.
  - `org.moqui.impl.EmailClientServices`.
  - BOSS screens: Inbox, Compose, MessageDetail and the mail settings.
- **Chat.**
  - The entities are in `framework/entity/ChatEntities.xml`, package `moqui.chat`:
    - ChatRoom;
    - ChatRoomMember, keyed by room and user;
    - ChatEvent, keyed by room and `eventSeq`, with `senderUserId`;
    - ChatEventMention, ChatAttachment, the polls and ChatLinkPreview.

    UserStatus is defined there too, in package `moqui.security`.
  - `org.moqui.impl.ChatServices` (58 services), with Rust in `moqui-core` `service/chat.rs` and `chat_link.rs`.
    `append#Event` is internal, and its caller checks access.
  - Agent room members: `moqui-llm` `chat.rs`.
  - The wire is JSON. Each event is a NotificationMessage that is not stored, on `chatRoom:<id>`, `chatUser:<id>` and
    `userStatus`, pushed over `/notws`.
  - BOSS screens: `Chat.qvue`, `ChatPanel.qvue` and ChatSettings.
- **Calls.**
  - Designed in mr-boss `design-drawers-commons.md` §4 LiveCall, §14 and §19 "Chat and calls"; for the media design
    they point here.
  - `Comms/Calls.xml`, `Calls/Call.xml` and `Calls/LiveCall.xml` are placeholders.
  - `UserStatus.inCall` exists for the call services, and the `CetCall` chat event type is seeded.
- **Voice and SMS.**
  - No real-time audio exists.
  - Assist's `transcribe` posts to an OpenAI-compatible endpoint ([Assist](../docs/ai/Assist.md)).
  - `org.moqui.SmsServices.send#SmsMessage` is a stub that logs "No SMS implementation in place".
- **The socket.** `/notws` authenticates a local user by session cookie or login key. Besides topic subscriptions, it
  handles co-editing frames (`crates/moqui-host/src/notws.rs`, `notws/collab.rs`).
  - Co-editing crosses the cluster on `collab-update`.
  - The local topic bus does not cross nodes.
  - The cluster only broadcasts (`cluster::publish`); nothing sends to a single node.
- **App hooks.** Apps reach the framework through theme values that name a service or screen
  (`STRT_RECORD_VISIBLE_SERVICE`, `STRT_EMAIL_CONTACT_SERVICE`, `STRT_CHATTER_SCREEN`).
  - Each is read first-row-only from the user's Quasar theme (`screen/sri/theme.rs`), so it holds one provider.
  - An app root's `shell-providers` holds several providers in sequence, collected from the apps the user may open
    (`screen/shell.rs`).
- **Not there yet:**
  - no web manifest, service worker or Web Push;
  - no ACME (the host loads certificates from `runtime/cert`);
  - no schedule or business-hours entity, in the framework or in Mantle;
  - no local speech model.

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
- Video to SIP endpoints. SIP legs are audio only (D-V3).
- An external SFU, since the SFU is the hub for SIP legs, recording and captions. Matrix's Element Call needs a LiveKit
  SFU, and XMPP clients call each other peer to peer with Jingle; neither interoperates with Moqui calls.
- Fax (T.38).
- A Matrix server in the first round. It follows the XMPP server, as C6 (D-C1, D-C2).

**Rules this plan keeps**
- **Embedded first (D-E1).** Every capability runs in the moqui binary with its embedded Postgres:
  - the SFU and TURN;
  - the SIP trunk client, registrar and B2BUA;
  - voicemail, the IVR and queues;
  - recording;
  - the XMPP server and, later, the Matrix server.

  External servers are options behind the same seams. Embedded means statically linked, not pure Rust: the build
  already compiles bundled C (aws-lc-sys, ring, SQLite, QuickJS). It avoids system libraries, and OpenSSL, which
  `deny.toml` bans.
- **The framework stays Mantle-free** (mr-boss impl-drawers-commons §4). The basic-comms component reads no Mantle
  entity and calls no Mantle or `ironstone.*` service.
- **Failure tolerance** (AGENTS.md).
  - A failing bridge, trunk, registration, room or recording is logged by name, and the rest go on.
  - Authorization, encryption policy and durable acceptance stay strict.
  - A failed write never reports success.
- **Listeners off by default,** like the SSH, SMTP, A2A and MCP listeners ([Run-Deploy](../docs/Run-Deploy.md)).
  - A socket listener is off while its port setting is empty, and each port setting has a `*_host` (§6.5).
  - Routed endpoints, such as provider webhooks and the appservice, have an `*_enabled=false` switch.
- **Secrets.**
  - Platform keys are `is-secret` default-properties, generated when unset (README redesign 7): the TURN auth secret,
    the Web Push VAPID keys, the Matrix signing key, the XMPP dialback secret and the appservice tokens.
  - Provider credentials an admin enters (trunk auth, bot and Meta tokens) are `encrypt="true"` fields, as
    `EmailServer` and `LlmEndpoint` keep theirs, with a conf or environment alternative.
- **Licenses.** Each new crate is checked against `deny.toml`.
  - MPL-2.0 needs an exception like cssparser's.
  - GPL and AGPL code runs only as a separate, reviewed process.
  - Zimbra's server files are GPL-2.0, and its voice web client is CPAL-1.0.
  - The applications in §10.2 are design references. Nothing is copied from them unless its license allows it and the
    copy is recorded.
- **Crates above core** go through `moqui_core::extension` ([Crate-Split](Crate-Split.md#the-seam-rules)).

**What runs where** (libraries in [§10.1](#101-libraries-and-protocols))

| Capability | Embedded (the default) | External option | External by nature |
| --- | --- | --- | --- |
| Conferencing | the SFU in `moqui-rtc` | — | — |
| NAT traversal | STUN and TURN in `moqui-rtc` | coturn | — |
| Phone calls | trunk client, registrar and B2BUA in `moqui-sip` | an existing PBX as a trunk | trunk providers, emergency services |
| Voicemail, IVR, queues | `moqui-sip` and the voice services | — | — |
| Recording | per-track files and an audio mix | composited video export (ffmpeg, browser recorder) | — |
| Transcription, captions | Whisper on candle (`speech-local`, opt-in) | an OpenAI-compatible endpoint via `moqui-llm` | — |
| Prompts | recorded prompts, defaults shipped | text to speech via `moqui-llm` | — |
| Chat federation | XMPP server (C3), Matrix server (C6) | Moqui as a component of an XMPP server; as an appservice of a homeserver (C2) | — |
| Personal WhatsApp, Signal and others | — | mautrix bridges (AGPL processes) on C6c or a homeserver | the networks |
| Business channels, SMS | connectors in `moqui-channels` | — | the providers' APIs |
| Push to closed apps | — | — | browser and XMPP-client push services |
| Certificates | the host's `runtime/cert` loading | an ACME companion | the certificate authority |

---

## 3. The framework Comms app

### 3.1 Placement

- **The component** is `runtime/base-component/basic-comms`, with:
  - `component.xml`, `MoquiConf.xml`, `README.md`;
  - `screen/`, `service/`, `data/`, `template/` and `test/`.

  Its `MoquiConf.xml` mounts `component://basic-comms/screen/BasicComms.xml` under `apps.xml` as `BasicComms` (menu
  title "Comms"), the way tools mounts `system` and `tools`. It needs a free menu index; 95 is mr-marble's.
- **The data model stays in `framework/entity`.**
  - The chat entities stay where they are, since `moqui-llm` agents and the notification socket use them.
  - The new call, voice and bridge entities go there too (§4.4, §5.4).
- **Services** (D-A3).
  - Record services are in `org.moqui.impl`, as `ChatServices` and `EmailClientServices` are:
    - `CallServices` (start, join, answer, leave and end a call; admit a guest), used by screens, the socket, the SIP
      engine and agents;
    - `VoiceServices`: trunks, numbers, routing and voicemail;
    - `ChatBridgeServices` (§4.1).
  - The screen-facing services are in `org.moqui.comms`: `MailSettingsServices`, `InboxServices`, `ComposeServices`,
    `ChatScreenServices`, and the screen helpers for calls and voice.
- **Rust.** New crates above core, each behind a host feature that is off by default. The features are included in the
  Maximal, Enterprise and Debug packages, not Lite or Dev ([Build-Test](../docs/Build-Test.md#packages)).
  - `moqui-rtc`: the WebRTC endpoint, the SFU, TURN, RTP and the codecs, recording.
  - `moqui-sip`: the SIP user agent, registrar and B2BUA (back-to-back user agent), over `moqui-rtc` media.
  - `moqui-xmpp`: the XMPP server.
  - `moqui-matrix`: the Matrix server and the appservice connector.
  - `moqui-channels`: the business channel connectors and SMS.
  - `speech-local` (Whisper on candle) is opt-in in any package.

  Each spike records its crate's compile time and binary size.

The services report supported, configured and active capabilities separately, following Zimbra's feature model
(§5.3).
- A compiled feature does not mean its listener, provider or credential is ready.
- Screens show the capabilities, with the reason one is unavailable, and services enforce them.
- Optional features have the usual off stubs.

### 3.2 What moves, splits or stays

| Part | Depends on today | Plan |
| --- | --- | --- |
| Settings: MailAccounts and its Add wizard, SignIn, SyncProgress, Removed, Identities, Rules, TextBlocks, OutOfOffice, Chat | `ironstone.comms.MailSettingsServices`, `EmailClientServices`; the settings already handle Internal accounts | **Split and move.** Internal account settings come from the BOSS inbox provider. OutOfOffice keeps the existing `STRT_EMAIL_CONTACT_SERVICE` contact predicate; recipient search is a different contract |
| Chat, ChatPanel, ChatTui | about 20 `org.moqui.impl.ChatServices` calls a screen. BOSS `ChatServices` (753 lines) adds `decorate#Rooms`, `find#RecordRoom`, `find#MessageRecords`, `file#OnRecord`, `create#FromChat`, `search#PaletteRooms` and record cards | **Move.** The record features become providers and actions. Chat.qvue has no contact or party code, and about 54 record references |
| Inbox/Mail, MessageDetail, MailTui | `InboxServices` (1,444 lines): the Internal account and mailboxes (Mantle `CommunicationEvent`, `ContactList`), record links, create a task, request or event, the invitation card (`respond#EventInvitation`), the contact card | **Split.** The framework Inbox has the email accounts. BOSS adds the Internal account, mailboxes and records through providers. Mail.qvue has about 185 lines on internal messages and mailboxes and about 100 on contacts |
| Compose, ComposeTui | `ComposeServices` (1,035 lines): recipients from Mantle parties and contact books, `create#Message`, `send#EmailDraft` (to file the message on a record) | **Split.** The framework composes and sends email. Recipient search, internal messages and filing are providers |
| Home | `CommsHomeServices`: meetings (`get#FullCalendarData`), internal threads, mailboxes | **Split.** The framework Home shows unread mail, mentions, missed calls and voicemail; BOSS contributes meetings and mailboxes through providers (D-A4) |
| Calls, Call, LiveCall, CallSettings | placeholders | **Built in the framework** (§5, §6) |
| Consent, Moderation, Shared/Chatter, RecordFiles | Mantle `ConsentServices`, `mantle.forum`, record timelines | **Stay in BOSS** |
| Demo data `BOSS_DEMO_*` | Mantle parties | The framework gets its own users-only demo data. BOSS keeps its demo data for its additions |

**Access.**
- The services that move check only `isInGroup('ADMIN')`.
- The BOSS screens are covered by the `BOSS_COMMS` artifact group. `BOSS_CONTACT_ADMIN` is checked only in Consent and
  Commons, which stay.
- BasicComms gets its own artifact group and authorization.

**Internal accounts.**
- The framework account lists, settings and compose choices show Internal accounts only when the BOSS provider
  contributes them.
- Existing Internal account rows stay; the extraction deletes no user data.
- Mail account IDs, drafts, rules, OAuth callbacks and notification links are checked through the moved screens.
- OutOfOffice wording describes the configured contact predicate; it does not assume Mantle contact books.

### 3.3 How an app adds to the screens

**XML screens use `screen-extend`.** `merge_extend` (`crates/moqui-core/src/screen/definition.rs`) supports:
- `transition` and `parameter` replaced by name;
- `subscreens-item` added, or merged into by name;
- `actions-extend` before, after or replacing the actions;
- forms merged by name;
- `section` and `section-iterate` overridden by name;
- `widgets-extend` before, after, at the start of or at the end of a named element.

For Inbox, the framework's `threads` transition calls the inbox providers, and BOSS contributes its Internal threads
as one of them. It does not also replace the transition with a second aggregation path.

Two gaps to close:
- **`merge_extend` does not merge `shell-providers`.** Merge the children by element type and key:
  - `kind` for quick-create, `group` for palette-source, `service` for today-source;
  - `doc-type` for palette-type, `topic` for notification-topic;
  - `scope` then `keys` for keymap/key;
  - links by their screen, and response-action also by text-parameter;
  - page-action by its screen or agent, and assist-prompt by screen, entity and source text;
  - the comms providers below by `name`.

  A later extension replaces an equal key, in component order, and different keys accumulate. Labels never identify an
  entry, and sequence ties keep declaration order. A0 covers every child.
- **A `render-mode` can't be replaced.** That is not needed while the `.qvue` files stay generic.

**`.qvue` screens need a new extension point.** `screen-extend` works on the server's XML tree, and a `.qvue` is one
client component, so nothing can be added inside it. There are three layers, from the cheapest to the rarest:

1. **Providers.** The comms hooks need several providers (BOSS, later Customer Service), and they must work outside a
   screen render, as for an inbound call or a bridge worker. Theme values (§1) can do neither.

   So the providers are declared as `shell-providers` declares `today-source` and `palette-source`: child elements of
   an app root, merged by screen-extend, in sequence, collected from the app roots the user may open.
   - The framework calls each provider and combines the answers. One that fails is logged and left out.
   - With no providers, the feature is skipped.
   - Background work, such as filing a bridged conversation or looking up an inbound caller, runs as the user it is
     for.
   - The existing theme hooks keep their single-valued contracts.

   | Provider | Asked with | Answers | BOSS provides |
   | --- | --- | --- | --- |
   | `inbox-source` | operation (accounts, threads, settings), sourceId, folder, filters, page | account and settings contributions, or a page of threads | the Internal account and mailboxes (`find#InboxThreads`) |
   | `recipient-source` | text, kinds | people, groups and lists with addresses | parties and contact books |
   | `record-lookup` | text or id | record cards `{entityName, id, title, status, url}` | `lookup#ById` |
   | `record-link` | message or chat event, record | ok, the link | `link#CommunicationEventRecord`, `file#OnRecord` |
   | `create-from` | message or event, kind | the new record's url | `create#RecordFromChatMessage` and its mail twin |
   | `invitation` | the iMIP part, response | ok | `respond#EventInvitation` |
   | `contact-card` | address or number | a card, an "add to contacts" action | `get#ContactCard` |
   | `meetings-source` | userId, range | meetings with a call link (Calls tab, Home card) | `get#FullCalendarData` |
   | `call-context` | caller number or address | who it is, open records (a screen pop) | party match, open cases |
   | `home-source` | sourceId, range, page | other Home cards and their rows | mailboxes |
   | `external-conversation` | chatRoomId, chatBridgeId, chatActorId, idempotencyKey | the record link | file the conversation on a case |

   Contracts:
   - Each provider implements a service interface with declared inputs and outputs.
   - Authority comes from the authenticated ExecutionContext; a userId or appRoot passed as data grants nothing.
   - Collections carry provider-qualified `sourceId` and `itemId`, stable order and bounded pages, with the existing
     page metadata. Each source owns its page; pages are never concatenated and called one.
   - Empty and unavailable are distinct answers.
   - Optional reads run in a force-new transaction with `pushErrors`/`popErrors`, and return `failedParts`.
   - A mutation returns an explicit failure.
   - Background filing retries with its idempotency key, without replaying the message.
   - A missing or failing authorization provider denies.
2. **Declared actions.** An `<ext-action>` in a screen or a `screen-extend`; any `.qvue` screen can use it. Each one
   names:
   - a provider-qualified `name` and an integer `sequence` (default 0);
   - a `slot`, one of the app's. For Comms: `message-actions`, `thread-toolbar`, `room-header`,
     `chat-message-actions`, `compose-toolbar`, `call-actions` or `voicemail-actions`;
   - `label` and `icon`;
   - exactly one of `screen` (dialog or page) or `transition`;
   - explicit parameter mappings from the authorized item data, with no whole-context forwarding;
   - `condition`, a display expression over the item's fields, never an authorization rule.

   The screen's data carries the actions, and the component renders them in its menus. This is how BOSS adds "Create
   task", "Link to record" and "File on case".
   - Actions merge by `name`; a later extension replaces the whole descriptor.
   - The server leaves out unauthorized targets, and the target checks access again when called.
   - The same descriptors make the TUI actions.
3. **Panel slots.** For the few real panels: the contact card pane, a record pane in Chat, the mailbox sidebar. An
   `<ext-panel>` is declared like an action (`name`, `slot`, `sequence`, `screen`, explicit parameters), and the
   component renders `<m-ext-panel slot="…">`.
   - The panel's screen decides per item, and renders nothing when nothing applies.
   - Only installed, authorized screen locations are accepted, never a screen or URL from the item.
   - The browser loads it through `moqui.loadComponent`, and TUI renders the same screen's TUI form.
   - A read-only panel that fails shows a named unavailable state, and the other panels still render.

A0 covers the XSD, parser, screen-extend merge, serialization and both renderers for all three layers.

**Links.** The `.qvue` files have 12 hard-coded `/qapps/Comms` links, the screen XML 14 and the services 12.
- Screens make them relative to `mScreen.path` and `appRoot`.
- Services that build links for notifications, email and push use the `comms_app_root` default-property. It is
  `/qapps/BasicComms`, and BOSS sets it to `/qapps/Comms`.

### 3.4 BOSS after the move

- **BOSS keeps `screen/Comms.xml` at `/qapps/Comms` as its own root.** The root keeps:
  - the always-actions, with `setup#UserOrganizationInfo`;
  - `shell-providers`, with BOSS's comms providers;
  - Chatter, RecordFiles, Consent and Moderation.
- **It mounts the BasicComms screens** by location, Home included:
  `subscreens-item location="component://basic-comms/screen/BasicComms/Inbox.xml"` and so on.
- **Its additions** are `screen-extend` files under `mr-boss/screen-extend/basic-comms/screen/BasicComms/...`, plus
  its providers and actions.
- **The launcher shows one Comms.** A screen-extend of `apps.xml` cannot hide BasicComms, because conf subscreens
  items apply after the XML and replace it by name.
  - So BOSS's `MoquiConf.xml` declares the `BasicComms` item again with `menu-include="false"`.
  - A1 proves with a spec that the later component's item wins.
  - The BasicComms URLs stay valid.
- **Both mounts.** Extensions apply by screen location, so they affect both mounts.
  - Providers get and check their own organization context; they cannot rely on BOSS's root actions having run.
  - The BasicComms URL is a supported mount, not an authorization bypass or a hidden alias.

### 3.5 Tests and docs

- **Framework specs.**
  - Chat, ChatPanel, ChatSettings, Compose, Inbox, MailAccounts, MailSettings and MailSignIn move to
    `runtime/base-component/basic-comms/test`, with users-only data.
  - A new `CommsExtension.spec.js` proves each provider, action and panel. It covers two providers for one hook, a
    failing provider and a denied user.
  - The `AppStructure`-style render covers every BasicComms screen in qvt (with its qvue component) and in tui.
- **BOSS specs.**
  - InboxMailboxes, Consent, CommsModeration, Chatter and CommsHome stay in BOSS.
  - Inbox and Compose keep a BOSS spec for the Mantle parts.
  - Shared-screen cases run through both mounts, checking organization isolation, direct action requests, relative
    links and a single launcher entry.
  - Framework-only fixtures contain no Mantle data.
- **Docs.**
  - The user guide is `docs/tools/Comms.md`; the docs index's tools tier widens to include it.
  - SIP and trunks, XMPP, Matrix and TURN each get a guide in `docs/interfaces/` when built.
  - Every new listener goes in `docs/security/Attack-Surface.md` and in Run-Deploy's port table.
  - mr-boss `design-drawers-commons.md` §12–14 and §17 shrink to BOSS's additions and link to the guide.

---

## 4. Chat protocols

### 4.1 Approach

ChatEvent is Moqui's application timeline. It reaches outside Moqui two ways:
- **Connectors** map a Moqui room to a room or conversation elsewhere: the business channels (C4, C5), and a Matrix
  appservice on an external homeserver (C2).
- **Native endpoints** serve Moqui rooms themselves: the XMPP server (C3) and the Matrix server (C6).
  - Local users connect with XMPP or Matrix clients.
  - With federation on, remote users join the rooms that are exposed.

Both keep external IDs, timestamps and protocol metadata beside the event.
- A Matrix server keeps its own protocol store: signed events, the room DAG and state resolution. The local event
  sequence does not replace it.
- Bridged and federated rooms use the usual search and retention services, subject to the adapter's capabilities,
  room access and encryption.
- Linking a conversation to a record does not authorize exporting Chatter or record cards to a network.

**Inbound.** A new `org.moqui.impl.ChatBridgeServices.ingest#Event`:
- authenticates the connector or federated server;
- resolves the external actor and the mapped room;
- checks posting or moderation rights;
- validates content and attachment references;
- calls the shared message, edit, reaction and redaction logic, which keeps unread counts, mentions, thread summaries
  and agent dispatch as for local events.

`append#Event` stays an internal primitive whose caller has checked access. Adapters cannot use it to skip the checks
or to act as a local account. An account link grants no authority to send as that account.

**Outbound.**
- A delivery row for each eligible destination is written in the event's transaction.
- A tx-commit SECA only wakes the workers. `register_tx_rules` (`service/call.rs`) runs an in-memory callback, which a
  crash loses, so workers also scan pending rows at startup.
- Workers claim rows with expiring leases, retry with backoff and honor provider rate limits.
- One failing destination does not block another.
- Every retry uses a stable idempotency key. A provider without idempotent sends reports an uncertain outcome for
  reconciliation; nothing claims exactly-once delivery.
- Permanent failures stay visible, with an authorized retry action.

**Inbound durability.** This follows the [Matrix application-service transaction contract][matrix-as].
- An authenticated batch is stored before it is acknowledged, and a repeated transaction ID is a no-op.
- Unique external mappings dedupe events, and source provenance suppresses echoes.
- Edits, reactions and deletes wait for their target's mapping, and per-room ordering keeps an edit behind its send.
- A malformed item is rejected and recorded by name while valid items go on.
- A security failure rejects the whole request before acceptance.

### 4.2 The landscape

| Network | Open? | Federation | Reach | How to reach it |
| --- | --- | --- | --- | --- |
| **XMPP** | IETF standard | yes | a long-standing base, simpler to serve; clients Conversations, Monal, Dino, Gajim | Moqui as the XMPP server (C3) |
| **Matrix** | open spec, open servers | yes | Element and other Matrix clients | Moqui as the Matrix server (C6); optionally an appservice on an existing homeserver (C2) |
| **WhatsApp** | closed | no | business and personal messaging | The Cloud API for business numbers (the On-Premises API ended 2025-10-23). Personal accounts only through a mautrix bridge (D-C4) |
| **Messenger, Instagram** | closed | no | business pages and accounts | Meta's messaging APIs, on the same Graph API and webhooks as WhatsApp (C4) |
| **SMS and MMS** | carrier networks | — | every phone | The voice provider's messaging API (§5.6); RCS later through that provider (not self-service) |
| **Telegram** | open client API, closed server | no | large | The Bot API |
| **Discord** | closed | no | communities | The bot API over the Gateway (C5) |
| **Slack, Microsoft Teams** | closed | their own | where business teams already are | A Slack app (Events API or Socket Mode). A Teams bot on the Microsoft 365 Agents SDK (the Bot Framework SDK ended 2025-12-31), single-tenant, public HTTPS (C5) |
| **Signal, Google Messages** | closed | no | large | Only through mautrix bridges (§4.3) |
| IRC | open | network-based | small | Not planned |

### 4.3 Proposals

The order (D-C1) is:
1. the bridge layer (C1);
2. SMS (C4-SMS), then WhatsApp, Meta and Telegram (C4);
3. the XMPP server (C3);
4. the Matrix server (C6).

The appservice (C2) and Slack, Teams and Discord (C5) come when a deployment needs them.

- **Bridge layer (C1).** The entities are in §4.4.
  - Adapters register through `moqui_core::extension`, and a `ChatBridge` row selects the implementation and provider
    account.
  - The core hook contract exposes capabilities, lifecycle and delivery. Inbound work enters `ingest#Event` (§4.1).
  - Protocol I/O runs above core, and entity and service work runs on service workers, keeping core synchronous.
  - The Rooms screen marks a bridged or federated room. An external member shows their network's badge, and room
    settings show the bridge and its link.
  - A bridge that goes down marks its rooms "not connected" and keeps the local timeline.
  - Delivery state is saved locally, pending, accepted remotely, failed or uncertain.
  - An unsupported edit, delete or reaction is refused before the change, never shown as synchronized.
- **Business channels (C4).** `moqui-channels` connects to the bridge layer:
  - SMS and MMS through the voice provider (§5.6), replacing the `SmsServices` stub;
  - the WhatsApp Cloud API: templates, the 24-hour customer service window, media;
  - Messenger and Instagram on the same Meta platform;
  - the Telegram Bot API.

  **Transport.** Where a network offers an outbound connection, that is the default, so an install with no public URL
  works: Telegram's `getUpdates`, and in C5 the Discord Gateway and Slack Socket Mode. WhatsApp, Messenger, Instagram
  and most SMS providers deliver only by webhook, which needs a public HTTPS URL.

  **External conversations.** A conversation with an outside person is a ChatRoom of type `external`, with local
  participants and external actors. An app's `external-conversation` provider can file it on a record, such as a
  Customer Service case.

  **Webhooks.**
  - Read the raw body within a bound, and check the signature and replay limits over the signed bytes before parsing.
  - Persist acceptance through §4.1.
  - Setup covers media limits, delivery receipts and provider refusal reasons.
- **XMPP server (C3).** `moqui-xmpp`, on Tokio.
  - **C3a, local users:** client connections on 5222 (STARTTLS) and 5223 (direct TLS), and the WebSocket at
    `/xmpp-websocket` (RFC 7395).
  - **C3b, federation:** server to server on 5269 with TLS and SASL EXTERNAL, with dialback (XEP-0220) as the fallback.
    Remote users reach only rooms with federation on (§4.4).
  - **Sign-in:** an app password (`UserAppPassword`, as DAV and git use).
    - Today's app-password hash is unsalted SHA-256, which cannot answer SCRAM. An app password made for XMPP also
      stores salted SCRAM-SHA-256 verifiers.
    - The server offers SASL2 (XEP-0388), Bind 2 (XEP-0386) and FAST (XEP-0484) for quick mobile reconnects.
    - It supports channel binding (XEP-0440, SCRAM-SHA-256-PLUS), which Conversations 2.21 requires on new installs.
    - PLAIN over TLS is only a fallback.
    - Failed sign-ins are limited per source (§5.2).
  - **Mapping:** users are JIDs; rooms are MUC rooms (XEP-0045) on `rooms.<domain>`; history is MAM (XEP-0313) over
    ChatEvent.
  - **Target:** the [XEP-0479][xep-0479] suites IM Core and Advanced, Mobile Advanced, and A/V Calling Core:
    - service discovery and capabilities (0030, 0115), ping (0199);
    - stream management (0198), client state (0352), push (0357);
    - carbons (0280), MAM (0313), MUC with invitations and self-ping (0045, 0249, 0410);
    - HTTP upload to ChatAttachment (0363);
    - PEP (0163), bookmarks (0402), avatars (0054, 0153, 0084), blocking (0191);
    - typing (0085), read markers (0333), corrections (0308), retractions (0424), reactions (0444), replies (0461);
    - external services (0215), which hands clients the embedded TURN for their own Jingle calls.

    The verification record names the suite, XEP and client versions. C3 claims only levels whose mandatory features
    pass.
  - **OMEMO** ([XEP-0384][omemo]) is between XMPP clients only.
    - The server keeps ciphertext in messages, carbons and MAM.
    - It serves device lists and bundles through PEP, with persistent items and access checks, for both the legacy
      `eu.siacs.conversations.axolotl` namespace that current clients use and `urn:xmpp:omemo:2`.
    - Moqui marks opaque messages unreadable, and keeps them out of plaintext processing and other bridges.
    - Moqui's own encrypted rooms use Megolm (§4.5).
  - **Deployment** needs certificates for the domain and the MUC and upload hosts, and SRV records (§6.5).
  - **Libraries** (§10.1, D-C3): from tokio-xmpp, its stream acceptor; xmpp-parsers for stanzas; `sasl` for the server
    mechanisms. Stream negotiation, SASL2/Bind 2/FAST, routing, s2s and dialback are Moqui's.
  - **External option:** an existing XMPP server (Prosody, MIT; ejabberd, GPL, as a separate process), with Moqui as
    its XEP-0114 component for the Moqui rooms.
- **Matrix server (C6).** `moqui-matrix`, on ruma, stores in Postgres. Tuwunel and Continuwuity (Apache-2.0) are
  references; they keep RocksDB and have no library API.
  - **C6a, local users:** the client-server API, so Element connects to Moqui:
    - simplified sliding sync (MSC4186), which Element X requires;
    - rooms, state, power levels mapped from room roles, media, receipts, typing and push rules;
    - password login, and SSO through the OAuth 2.0 API once Moqui is an authorization server, as
      [MCP](../docs/ai/MCP.md) also plans;
    - C7's key endpoints (§4.5).
  - **C6b, federation:** signing keys, event signing, the room DAG, state resolution v2, backfill and media, on 8448 or
    443 with `.well-known` delegation (§6.5).
  - **C6c, appservices:** so mautrix bridges (WhatsApp, Signal, Telegram, Meta, Google Messages and others) attach to
    Moqui as to any homeserver.
    - Each is a separate AGPL process with its own Postgres (D-C4).
    - Their [end-to-bridge encryption][mautrix-encryption] ends at that process; it is not Moqui's browser E2EE.
  - Rooms, membership and events map to the timeline. Matrix state events, permissions and original metadata keep
    their protocol meaning; not every Moqui feature has the same semantics there.
  - C6 pins the spec version it implements (v1.19 at this writing).
- **Appservice on an external homeserver (C2, optional).** This is for a deployment that already runs Synapse (AGPL,
  a separate process), Tuwunel or Continuwuity.
  - The appservice owns a namespace of users (`@moqui_<user>:example.com`) and rooms (`#moqui_<room>:…`). Moqui users
    are puppets there, and federated users join Moqui rooms as external members.
  - Only unencrypted portals are bridged.
  - Moving from C2 to C6 needs a rehearsed migration that keeps the server name and signing keys, user and room IDs,
    room versions, state and history, media, appservice registrations, and device, key and sync state. Until it
    passes, the deployment stays on C2 with its homeserver backup.

The connector contract carries this capability matrix.
- Each implementation narrows it to the API version it verified.
- An unsupported operation returns a reason code, and the UI leaves out the action.
- A link to an external room grants no rights.

| Adapter | Initial supported mapping | Limits and refusals |
| --- | --- | --- |
| SMS/MMS C4 | text and media within provider limits; delivery status | edits, reactions, remote deletion, group-room semantics |
| WhatsApp, Messenger, Instagram, Telegram C4 | supported text and media; provider receipts | operations outside provider permissions, window or template rules; no fallback that pretends success |
| XMPP C3 | messages, MUC, MAM, receipts and the XEPs above | unadvertised XEPs; reading OMEMO content; converting encrypted content to another network |
| Matrix C6 | messages, membership, replies, edits, reactions, redactions, media, with state and ID mappings | Moqui polls, record cards and moderation without an equivalent |
| Matrix appservice C2 | as C6, through the homeserver | encrypted rooms |
| Slack, Teams, Discord C5 | documented against the selected APIs at C5 | no claimed support until the connector's operation and permission cases pass |

### 4.4 Data model (framework, `moqui.chat`)

- **`ChatBridge`** {chatBridgeId, typeEnumId (Xmpp, Matrix, MatrixAppservice, Sms, WhatsApp, Messenger, Instagram,
  Telegram, Slack, Teams, Discord), name, configJson with secrets encrypted, statusId, lastErrorText}. There is one row
  per connector account or federated network.
- **`ChatRoomBridge`** {chatRoomBridgeId, chatRoomId, chatBridgeId, externalRoomId, direction (both, in, out),
  fromDate, thruDate}. It is for connectors only.
  - Each external room has one active mapping per bridge.
  - Delivery rows keep this stable mapping ID.
  - A native endpoint addresses the Moqui room itself.
- **`ChatRoom.federationEnabled`** (default N).
  - Remote users of a native endpoint can join only rooms that have it on.
  - Local users reach any room they belong to with an XMPP or Matrix client.
- **`ChatActor`** {chatActorId, kindEnumId (local, external), userId, chatExternalUserId}.
  - Exactly one identity reference is set, unique within its kind.
  - Agents stay local service-user actors.
  - External actors have no login.
- **`ChatExternalUser`** {chatExternalUserId, chatBridgeId, externalUserId, displayName, avatarUrl, linkedUserId},
  unique on bridge and external user.
  - For a native endpoint, externalUserId is the JID or Matrix ID.
  - A verified puppet link is provenance, not permission to act as that local user.
- **`ChatRoomMember`** is keyed by `(chatRoomId, chatActorId)`.
  - Events use `senderActorId`. Mentions, votes, moderation authors and summaries resolve actors too.
  - Local-user inputs and topics still work by resolving userId to its actor, and local-user responses keep userId.
- **`ChatEventExternal`** maps a local event, its mapping or native endpoint, and the external room and event ID, with
  the original timestamp, origin and protocol metadata.
  - Bridge, room and event are unique together, even across mapping changes.
  - It dedupes input and maps edits, reactions and deletes.
- **`ChatBridgeInbound`** holds:
  - the authenticated bridge and transaction ID, unique together;
  - the bounded batch, the processing state and the per-item outcomes.

  Its retention outlasts the provider's retry window.
- **`ChatBridgeDelivery`** holds:
  - the local event and destination mapping, unique together;
  - an idempotency key and the target dependency;
  - the state, the attempt count and the next attempt;
  - the lease owner and expiry;
  - the remote ID and the last error.

  It keeps dedupe tombstones after content is purged, and no plaintext beyond the room's policy.

**The C1 migration.**
- Backfill one local actor per referenced user, then migrate membership keys, senders, mentions, poll votes, and the
  summary and read projections.
- Verify counts and referential consistency before switching reads and writes.
- Keep userId service compatibility and a rollback backup.
- Run it in a maintenance window, so no old writer uses the old key.
- There is no nullable-userId key, and no made-up UserAccount for a remote person.
- The fixtures include:
  - departed members, and deleted local accounts with retained history;
  - reactions, mentions and unread markers;
  - agents.
- Remote messages reach agents only under the room's explicit unencrypted-room policy.

### 4.5 End-to-end encrypted rooms (C7)

A room may be created encrypted, and that can't be turned off later. Messages use Matrix's Olm (between devices) and
Megolm (rotating group sessions).
- **The library.** The browser runs **matrix-sdk-crypto**, the established client state machine on vodozemac, as
  `@matrix-org/matrix-sdk-crypto-wasm`. It is about 9 MB, so it loads only when an encrypted room opens.
- **The endpoints.** Its `OlmMachine` does no I/O. C7 builds the Matrix-shaped endpoints it calls, which C6a later
  serves to Matrix clients:
  - keys upload, query and claim;
  - to-device messages;
  - key backup.
- **No rebuilding.** Device verification, backup and key management are not rebuilt from crypto primitives.
- **The server** stores and pushes ciphertext, public device keys and routing metadata. Plaintext and private keys stay
  in the browser.
- **Devices.**
  - `ChatDevice` holds the public identity, verification and revocation state, and device-list revision.
  - `ChatDeviceKey` holds signed public and one-time keys, with atomic claims.
  - Durable to-device queues and sync cursors deliver key messages until acknowledged.
  - Private keys, sessions and replay state stay in the browser's crypto store.
  - Membership changes and revocation rotate outbound sessions, and a revoked device gets no new keys. Rotation cannot
    take back what a device already holds.
- **Attachments.** Files and thumbnails are encrypted in the browser before upload, and their keys and hashes travel
  in the encrypted event. The server cannot make previews or extract text.
- **Key backup.**
  - Room keys are backed up to the server, encrypted with a recovery key the user keeps.
  - Without the recovery key, a new device restores history only from another authorized source, such as a verified
    device.
  - Tests cover a lost device and missing keys, with no silent trust reset and no plaintext sent.
- **Verification.** Emoji or QR-code device verification, and cross-signing. Trust is held by clients; a server-side
  flag cannot establish it.
- **An encrypted room gives up:**
  - server search;
  - agent members;
  - server link previews;
  - Chatter on records;
  - bridges;
  - "Create from" over the message text, unless the user lets that action disclose the selected text.
- **No plaintext leaves the browser.** None goes to bodyText, snippets, notifications, logs, analytics or agent
  inputs, and push says only that a message is waiting.
- **The screens** explain the features that are missing.
- **Retention** removes ciphertext, encrypted attachments and their records. It cannot erase recipients' copies.
- **Calls.** Media encryption is separate (W8, §6.4, built): its per-sender keys travel over C7 core's Olm to-device
  messages, never a room's Megolm session. A call that requires E2EE is unavailable on unsupported clients, and never
  silently downgrades. Other calls show their encryption mode.

---

## 5. Voice

### 5.1 Approach

Every call is one `CallSession`, whatever its transport. A call has **legs**: a browser over WebRTC, a guest's
browser, a SIP desk or soft phone, or a phone number through a trunk. `moqui-rtc` holds every leg's media:
- Browser legs use the SFU (§6).
- SIP legs are audio only. A phone takes one stream, so each SIP leg gets a mix of everyone but itself. Toward the
  browsers, each SIP leg is forwarded as one more participant.

So a phone call, a desk-phone call, a meeting and a meeting someone dials into are the same thing. Adding a phone
number to a meeting, or turning a call into a meeting, adds legs to the same session.
- That is subject to permissions, consent and media compatibility.
- An encrypted call cannot add SIP legs.
- Calls and messages with one outside number share one conversation (§5.4).

### 5.2 SIP and PSTN

- **Trunks (V1).** `moqui-sip` registers to, or is IP-authenticated by, a SIP trunk provider: for example Telnyx,
  Twilio Elastic SIP, Bandwidth, VoIP.ms or Flowroute. An existing PBX (Asterisk, FreeSWITCH, 3CX) can be set up as a
  trunk. Outbound calls go to the trunk, and inbound DIDs route by `PhoneNumber` (§5.4).

  The trunk side:
  - **transport:** UDP, TCP or TLS on 5060 and 5061;
  - **auth:** SIP Digest, with realm- and algorithm-specific credentials, and nonce and replay checks;
  - **media:** SRTP (SDES) on TLS, over the SIP RTP port range (§6.5), with symmetric RTP;
  - **DTMF:** RFC 4733;
  - **codecs:** G.711 μ-law and A-law, and Opus where offered. Browser Opus is transcoded to G.711 in `moqui-rtc`.

  STIR/SHAKEN attestation is the provider's. TLS with SRTP is the default. UDP, TCP or plain RTP need an explicit
  provider profile, never a silent downgrade, and only verified profiles are claimed (D-V4).
- **Fraud.** Scanners probe SIP listeners for weak credentials, then place international calls.
  - Failed digest attempts are limited and blocked per source address. They never lock the account, which would let a
    scanner lock users out.
  - International and premium destinations are off until enabled, per deployment and per user.
  - Caller IDs and routes are authorized per user.
  - Concurrency, per-user and per-day limits and provider spending limits are bounded by policy, with alerts.
- **Registrar for phones (V4).** Desk phones (Yealink, Polycom, Grandstream) and soft phones (Linphone, Zoiper,
  Groundwire) register with generated SIP credentials, shown once.
  - **Verifiers.** Store encrypted Digest verifiers (HA1) per algorithm and realm, not a login password hash. Prefer
    SHA-256 ([RFC 8760][sip-digest], [RFC 7616][http-digest]); legacy algorithms need an explicit device profile.
  - **Bindings** are separate expiring rows: contact, transport flow, Call-ID/CSeq and last refresh. A device may have
    several. Expired or revoked bindings cannot ring.
  - **Codecs:** G.711, G.722 for wideband desk phones, and Opus where available.
  - **NAT.** Phones behind NAT mostly don't do ICE.
    - The registrar honors `rport` (RFC 3581).
    - It keeps each binding alive over the flow the phone opened, using SIP Outbound (RFC 5626) where the phone
      supports it.
    - It latches RTP to the address the phone sends from.
    - Remote phones use TLS on 5061 and SRTP.
  - Provisioning files for the common desk phones come later.
- **Browser phone.** The browser never speaks SIP. It talks WebRTC to Moqui, and Moqui places the SIP leg. That keeps
  one media path, one recording path and one set of credentials, with no SIP over WebSocket or JsSIP.
- **Emergency calls (D-V2).** Emergency numbers go only to a trunk marked emergency-capable.
  - Each call needs the user's or phone's registered location, a validated callback identity and a configured
    notification destination.
  - Emergency routing comes before ordinary dial prefixes and destination limits.
  - Without these, emergency dialing is refused with a clear message, and the Phone settings say so.
  - The [US guidance on Kari's Law and RAY BAUM'S Act][emergency-guidance] covers direct dialing, notification and
    dispatchable location. Other jurisdictions and deployments need their own review.
  - V1's release gate includes provider location and callback provisioning, notification, location updates for mobile
    and remote users, and the operator's readiness review.
  - Live checks use the provider's approved test procedure, never an unarranged emergency call.

### 5.3 Features, from Zimbra and the usual phone systems

Zimbra's voice app (`../3p_other/Zimbra` from the repository root) was a front end to Cisco and Mitel unified
communications servers:
- the web client is `zm-web-client/WebRoot/js/zimbraMail/voicemail/` (`ZmVoiceApp.js`; `model/`, `controller/`,
  `view/`);
- the SOAP API is `zm-mailbox/soap/src/java/com/zimbra/soap/voice/` (`GetVoiceInfo`, `SearchVoice`, `VoiceMsgAction`,
  `GetVoiceFeatures`, `ModifyVoiceFeatures`, `GetVoiceMailPrefs`, `ModifyVoiceMailPrefs`, `ModifyVoiceMailPin`,
  `ModifyFromNum`, `UploadVoiceMail`);
- the provider setup is `zm-mailbox/store/docs/INSTALL-VOICE.txt`.

Here the server is Moqui itself. Zimbra's `ZmCallFeature` separates subscribed and active features. Moqui reports
supported, configured and active capabilities instead (§3.1). What Zimbra showed, and what it maps to:

| Zimbra | Here |
| --- | --- |
| Folders: Voicemail Inbox, Missed, Answered and Placed calls, Trash (`ZmVoiceFolder`) | Calls screen tabs: All, Missed, Voicemail. Placed and answered are filters. Voicemail has Trash |
| Voicemail actions: listen, mark heard and unheard, delete, empty folder, download, print, call back, reply by email, forward by email, new contact (`ZmVoicemailListController`) | Voicemail actions, plus the transcript, "Create from" and "Add to contacts" (providers) |
| Call list: call back, contact, new contact (`ZmCallListController`) | Call history actions, plus a note, and Chatter through the `call-context` provider |
| Call features (`ZmCallFeature`; SOAP `type/*Feature`): forwarding always, busy and no answer; selective forward, reject and accept with caller lists; anonymous call rejection; caller-ID blocking; call waiting | Phone settings "Call handling": the same rules, plus schedules from business hours or a calendar, and simultaneous ring to a mobile |
| Voicemail preferences: email notification address, PIN (`ModifyVoiceMailPin`) | Voicemail settings: greeting (recorded in the browser or uploaded), PIN, email notice with or without audio and transcript, number of rings |
| From number (`ModifyFromNum`) | The caller ID picker: my numbers, my ring groups' numbers |
| Presence through the UC provider | `UserStatus`: "in a call" set automatically; DND sends calls to voicemail |

Beyond Zimbra:
- dialer and keypad, with contact search through the recipient provider;
- hold, mute, blind and attended transfer, add a participant, merge into a meeting;
- ring groups and hunt groups (simultaneous, in order, round robin);
- an IVR and auto attendant: menus, business hours, prompts recorded or from text to speech;
- call queues with wait music, positions, agents and wrap-up time, used mainly by Customer Service through providers;
- default prompts and hold music shipped, so a new install works before anyone records;
- call recording, with notice to every leg (D-V5);
- voicemail and call transcription, by local Whisper or the `moqui-llm` endpoint (§6.4);
- screen pop through the `call-context` provider;
- call quality (jitter, loss, MOS) into telemetry, for the Monitor app.

### 5.4 Data model (framework, `moqui.call`; shared with §6)

- `CallSession` {callSessionId, chatRoomId (optional), kindEnumId (direct, group, meeting, phone), statusId, revision,
  directionEnumId, startedDate, answeredDate, endedDate, endReasonEnumId, mediaNodeId, mediaGeneration, encryptionMode}.
- `CallParticipant` {callParticipantId, callSessionId, identityKindEnumId (local, guest, phone), userId, callGuestId,
  chatActorId (optional), displayName, roleEnumId, admissionStatusId, admittedDate, revokedDate}.
  - A phone participant's presented number is on its leg, apart from any verified identity.
  - C1 can add the chat-actor link without blocking W1.
  - Participation grants only call permissions, not room membership.
- `CallGuest` {callGuestId, callLinkId, displayName, credentialHash, credentialExpiry, admittedDate, revokedDate}: a
  call-scoped guest with no login.
- `CallLeg` {callLegId, callSessionId, callParticipantId, legTypeEnumId (web, guest, sipDevice, pstn), phoneNumber,
  sipDeviceId, offeredDate, answeredDate, joinedDate, leftDate, statusId, endReasonEnumId, qualityJson}.
- `CallConnection` {callConnectionId, callLegId, generation, signalingNodeId, credentialExpiry, lastHeartbeatDate,
  statusId}. A reconnect replaces it; it makes no second participant or second answer.
- `CallLink` {callLinkId, tokenHash, name, ownerUserId, chatRoomId, policyEnumId (members, link, lobby), expiryDate,
  revokedDate}.
- `CallUserLog` {callSessionId, userId, roleEnumId (caller, callee, ringGroupMember, queueAgent), outcomeEnumId
  (placed, answered, missed, declined), seenDate}.
  - Each user's history and missed-call badge come from it.
  - Every member a ring-group or queue call rang gets a row.
- `CallRecording` {callRecordingId, callSessionId, ownerUserId, kind (per track, mixed), statusId, startedDate,
  endedDate, retentionUntilDate, lastErrorText}. Child `CallRecordingTrack` and `CallRecordingSegment` rows name:
  - the source participant, leg and track;
  - the time range;
  - the contentLocation (DbResource), duration and processing state.
- `CallTranscript` {callTranscriptId, callRecordingId or voicemailId, segment, speaker, statusId, text, lastErrorText}.
  One entity serves recordings and voicemail, with the source's access policy.
- `CallRecordingConsent` {participant or leg, recording scope, policy revision, notice time and method,
  acknowledgement time and method when required, withdrawal}.
- `VoiceTrunk` {voiceTrunkId, name, host, transport, authJson encrypted, registerFlag, emergencyCapable, statusId}.
- `PhoneNumber` {phoneNumber, voiceTrunkId, messagingBridgeId (its SMS connector, §4.4), routeTypeEnumId (user,
  ringGroup, ivr, queue), routeId, emergencyLocationId}. Messaging works with or without the SIP trunk.
- `EmergencyLocation`: a validated address and location, a callback identity and a provider reference. It does not use
  Mantle PostalAddress. User and device assignments carry the location a call used.
- `SipDevice` {sipDeviceId, userId, username, realm, credentialRevision, statusId}. Its encrypted verifiers and its
  expiring `SipRegistration` bindings are child rows (§5.2).
- `CommsSchedule` and `CommsScheduleRule` {weekly hours, dates and holidays, time zone}.
  - Business hours for handling rules, the IVR and queues.
  - A calendar-based schedule comes from a provider, such as BOSS's calendar.
- `RingGroup` and `RingGroupMember` {strategy, ring seconds, fallback}; `IvrMenu` and `IvrOption`; `CallQueue` and
  `CallQueueAgent`; `CallerList` and `CallerListEntry`.
- `CallHandlingRule` {userId, kind (always, busy, noAnswer, selectiveForward, selectiveReject, selectiveAccept,
  anonymousReject), target, callerListId, commsScheduleId, sequenceNum}.
- `VoicemailBox` {userId or ringGroupId, pinHash, greetingLocation, notifyEmail, attachAudio, attachTranscript, rings}.
- `Voicemail` {voicemailId, voicemailBoxId, callSessionId, fromNumber, receivedDate, durationMs, audioLocation,
  heardDate, folder (inbox, trash)}.
  - Its transcript is a `CallTranscript`.
  - A ring group's box shares heard state among its members.

**One conversation per outside number.** An outside number's SMS, calls and voicemail land in one `external` room for
the pair (our number, their number). The call links to it by `CallSession.chatRoomId` with a `CetCall` event, and a
voicemail is an event that links to its `Voicemail`.

**Mantle.** A finished call writes no Mantle record. BOSS's `Call` CommunicationEventType comes from a Mantle SECA on
the framework's `end#Call` (design §14).
- The subscriber stores idempotent work keyed by callSessionId in the same transaction.
- An after-commit callback only wakes it.

**Call state.** The call services enforce an explicit state machine:
- Sessions go from ringing to active to ended, or from ringing to ended.
- Legs keep their outcome: offered, answered, declined, canceled, timed out or failed.
- Idempotency keys and revision checks make repeated start, join, answer, leave and end requests safe.
- The first successful answer wins atomically and cancels the sibling offers, including other tabs and forked SIP
  legs.
- Presence derives from the user's active legs, so leaving one of two calls does not clear `UserStatus.inCall`.
- Cleanup expires abandoned connections and completes the affected history once. It never deletes call history.

**Recording** is off by default (D-V5). When it is on:
- Every leg gets the notice: an indicator in the browser, a voice prompt on a phone. That includes late joiners and
  transferred legs.
- The notice is logged per leg in `CallRecordingConsent`, and staying on the call is consent.
- A deployment policy can require an acknowledgement (a click, or a DTMF key). Recording then pauses while a current
  leg has not acknowledged, or after one withdraws; the call goes on, and resuming starts a new segment.
- Only authorized users record, download or read recordings, transcripts and voicemail, direct resource URLs included.
  BOSS record links do not widen that.
- Retention deletes audio, video, transcripts, previews and pending processing, and recovery jobs clean up partial
  uploads and segments.
- Email attachments of audio or transcripts are optional, off by default, and cannot be recalled.
- Recording and transcription policy is checked before external processing.
- The deployment's policy must meet its jurisdiction's rules; the notice alone does not.

### 5.5 Screens (framework BasicComms)

| Screen | Path | Purpose |
| --- | --- | --- |
| Calls | `BasicComms/Calls` | Tabs: All, Missed, Voicemail, Call links, Meetings (the meetings provider). Call back, play, the actions |
| Dialer | `BasicComms/Calls/Dialer`, also a tray panel | Keypad, search for a person or number, caller ID picker, call |
| Call | `BasicComms/Calls/Call`; guests at `/call/<token>` (D-A5) | Lobby and call view (§6.6). For a phone call: audio only, keypad for DTMF, hold, transfer, add, merge, record |
| LiveCall | the `q-page-sticky` slot | Mini window while in a call, across navigation |
| Incoming call | shell | Ring bar in every tab of the user (on `chatUser:<id>`): Answer, Decline, Message. Web push when permitted, missed-call recovery (§7.2) |
| VoicemailDetail | `BasicComms/Calls/Voicemail` | Player, transcript, the actions |
| Settings/Calls | `BasicComms/Settings/Calls` | Devices (kept in the browser), ringtone, join muted, camera off, blur |
| Settings/Phone | `BasicComms/Settings/Phone` | My numbers, call handling rules, voicemail (greeting, PIN, notices), my desk and soft phones (credentials shown once) |
| Admin: Voice | `System/Voice` (D-A6) | Trunks with status, numbers, ring groups, IVR menus, queues, schedules, fraud limits, emergency setup, media and SIP status |
| Admin: Bridges | `System/ChatBridges` (D-A6) | Bridges and federation with status and errors, bridged and federated rooms, connector setup |

The admin screen files are in basic-comms, and its `MoquiConf.xml` adds them to the tools `System` app, as the demo
component adds screens there. Every screen gets a TUI twin; in the terminal a call is not live, and the twin shows
history, voicemail transcripts and settings.

### 5.6 SMS

SMS and MMS go through the voice provider's messaging REST API and webhooks (Telnyx first, D-V4).
- `SmsServices.send#SmsMessage` becomes the real sender.
- Inbound SMS lands in the number pair's `external` room (§5.4).
- Marketing's SMS sends (mr-boss research-marketing) use the same sender, with consent gating in Mantle.
- SMS needs C1's delivery layer and a number with its messaging connector. It does not need the SIP engine.
- Provider registration (10DLC in the US) and consent rules are enforced, also through the `SmsServices` entry point.

### 5.7 Libraries

The libraries are in [§10.1](#101-libraries-and-protocols). The V1 spike tests these against an in-process fake trunk:
- trunk registration;
- calls each way;
- DTMF;
- re-INVITE hold;
- cancellation and fork races;
- media mixing;
- credential handling.

A live check with an authorized provider proves the documented profile (§7.3).

---

## 6. Web conferencing

### 6.1 Approach

The SFU is in from the first call. Every WebRTC call, a two-person one too, goes through `moqui-rtc`; there is no mesh
mode.
- One media path serves recording, captions, SIP legs, quality data and guests.
- Participants' media addresses are hidden from each other, as long as signaling exposes only the SFU's candidates.
  The SFU still sees connection addresses.
- Budget bandwidth and CPU for the codecs, layers and participant limits. One-to-one video also uses server egress.

The BOSS design (§14, §19) points here.

### 6.2 Library: str0m

**str0m** is the WebRTC core (D-W1), a sans-IO library. It has:
- ICE-lite over UDP and TCP;
- DTLS-SRTP;
- simulcast, TWCC and bandwidth estimation;
- every packet and frame exposed to the caller.

`moqui-rtc` writes the SFU on it: room membership, subscriptions, layer selection and forwarding. Owning the
forwarding is what gives SIP mixing, recording and captions their media.
- **Crypto.** str0m's default crypto, aws-lc-rs, is already in the build through rustls. `rust-crypto` with `dimpl` is
  the all-Rust option.
- **Fallback.** webrtc-rs `rtc`, also sans-IO, which exposes received RTP.
- **Reference only.** webrtc-rs's `sfu` forwards RTP internally and surfaces only keyframe requests, so it cannot feed
  the mixer or recorder.

Why sans-IO fits:
- `moqui-host` owns sockets and Tokio tasks, and no business logic runs on Tokio
  ([ContextObjects](archive/ContextObjects.md), [Architecture-Proposal](archive/Architecture-Proposal.md)).
- The host drives sockets and timers, with no business logic in the packet loop. Start with a bounded single-node
  driver, and measure capacity before promising that a node's rooms fit on one task.
- Tests run two peers in memory with no sockets, so they are fast.

The W1 spike shows:
- two-party and five-party calls;
- simulcast layer switching with TWCC;
- renegotiation, ICE restart and ICE-TCP;
- media taken out and put in, for SIP mixing, recording and captions.

It records versions, release dates, licenses and results. If neither str0m nor `rtc` passes, the dependent features
wait, and their requirements stay.

**The W1 gate (2026-10-31): str0m passed.**

- **Version and licenses:** str0m 0.24.1 with `aws-lc-rs`, on crates.io as of the gate, MIT OR Apache-2.0. Its new
  transitive crates are all MIT OR Apache-2.0 (License-Review). `cargo deny check licenses` passes.
- **Build cost:** the `rtc` feature adds 5.2 MiB to a Dev binary and 56 s to the first build after it is turned on.
- **Cases passed** (`cargo test -p moqui-rtc`, 14 tests on an in-memory network with simulated time, 0.2 s together):
  - two and five parties;
  - simulcast: the high layer unconstrained, the low layer at 350 kbps, up again when freed, audio only below the floor,
    and a subscription capping the layer;
  - renegotiation both ways, and glare (the SFU gives way);
  - ICE restart, and a TCP-only client;
  - generation fencing;
  - a tap seeing published frames, and an injected source heard by every peer.
- **Not yet shown:** five parties on real sockets; TWCC under real network loss. W4 and the live checks of §7.3 cover
  those.

**The M0 gate, the audio core (2026-10-31): pure-Rust Opus passed, as `opus-rs`.** M0 is the first part of V1: the
codecs, the mix-minus mixer and the taps that SIP legs, recording (W5) and captions (W6) share
(`crates/moqui-rtc/src/audio/`, the host's `src/rtc/media.rs`).

- **Opus candidates**, all BSD-3-Clause with no dependencies of their own, one lineage of libopus ports (`opus-pure`
  forks `rusty-opus`, which forks `opus-rs`): `opus-rs` 0.1.40 (2026-10-25, the first release 2026-02-28),
  `rusty-opus` 1.0.2 (2026-10-17), `opus-pure` 0.2.2 (2026-09-28). The reference was libopus 1.6.1, what browsers run,
  through `opus` 0.4.0 (2026-08-23, MIT OR Apache-2.0) and `opusic-sys` 0.7.6 (2026-10-26, BSD-3-Clause, cmake).
- **How they were measured** (a scratch crate, release build): mono 48 kHz, 20 ms frames, VoIP, 32 kbit/s, in-band FEC
  for 10% loss, complexity 9. The inputs were 3 s of a 440 Hz tone and 3 s of a speech-like signal (gliding voiced
  harmonics through two formants, fricative noise, pauses), and 10% random loss (19 of 200 frames).
- **Decoding the browsers' Opus** (libopus packets, each decoder against libopus's own output):
  - `opus-rs`: 142 dB SNR on the speech-like signal and 174 dB on the tone, which is float rounding. Concealment under
    loss 141 dB, FEC 142 dB, and DTX with comfort noise 142 dB.
  - `opus-pure`: the same, except DTX at 49.5 dB (its comfort noise differs).
  - `rusty-opus`: FEC at 24.8 dB and DTX at 49.5 dB.
- **Encoding** (each decoded by libopus, which is what the browsers hear):
  - Correlation with the source is within 0.003 of libopus for all three (speech-like 0.9455 against 0.9467).
  - Rate on speech, against libopus's 24 kbit/s: `opus-pure` 25, `opus-rs` 30, `rusty-opus` 37. On a steady tone
    `opus-rs` overshoots its 32 kbit/s budget (45).
  - Encode time per 20 ms frame: libopus 165 µs, `opus-pure` 191, `rusty-opus` 196, `opus-rs` 277. Decoding takes 13
    to 25 µs for all.
  - No encoder is byte-identical with libopus (none needs to be).
- **Robustness:** no decoder panicked on 20,000 random packets.
- **The choice:** `opus-rs`. Its decoder matches libopus in every case, and decoding is most of the load: every
  browser leg into the mixer, the recording and captions. It is the most used and most often released of the three,
  from the authors of `rsipstack` (D-V1). Its encoder only codes the server's audio toward browsers (a SIP leg), where
  its higher rate on tones costs some bandwidth and nothing in quality. `audio/opus.rs` is the only file that names it;
  `opus-pure` has the same API and is the next pick.
- **Build cost:** its release build takes 2.2 s on 24 cores (8 s of CPU), against libopus's 3.0 s (16 s of CPU) plus
  cmake and a C compiler. An encoder and decoder add 0.80 MiB to a release binary (libopus 0.51 MiB).
- **G.711 and G.722, in house.** `ezk-g711` and `ezk-g722` (MIT) bring the ezk media framework (`ezk`, `ezk-rtp`,
  `bytes`, `tokio`, `rand`).
  - G.711 μ-law and A-law are bit-exact with ITU-T G.191 (through `ezk-g711`) for all 65,536 samples and 256 codes.
  - G.722 at 64 kbit/s, written from the blocks of ITU-T G.722, is bit-exact with libg722 (through `ezk-g722`) on 5 s of
    a chirp with noise, encoding and decoding, from the same start state. `ezk-g722` starts its scale factors at 0, not
    G.722's 32 and 8, so its output differs from the standard's at the start.
- **Resampling, in house:** a polyphase Kaiser-windowed sinc of 288 taps for the integer ratios 8 and 16 kHz to and
  from 48 kHz. It is about 70 dB down in the stop band with a 3 ms delay. `rubato` 5.0.1 would bring `audioadapter`,
  `audioadapter-buffers`, `num-integer`, `num-traits`, `realfft`, `visibility` and `windowfunctions`, for general ratios
  the calls do not need.
- **Licenses:** one new crate, `opus-rs` (BSD-3-Clause), with nothing transitive (License-Review).
  `cargo deny check licenses` passes.
- **Cases passed** (`cargo test -p moqui-rtc`, 13 new tests, 0.5 s with the W1 and W4 ones):
  - codec round trips and levels: Opus, G.711, G.722 (and its reference vector), and each codec by its SDP name;
  - resampling keeps the level and stops aliases more than 50 dB down;
  - the jitter buffer puts reordered frames back in order, recovers losses from FEC, conceals, drops late frames,
    stops, and starts again after a pause;
  - the mixer gives each leg everyone but itself, over three legs, and saturates;
  - a full queue drops and counts rather than blocks; a stalled consumer drops while forwarding goes on, beside a live
    one on the same room;
  - end to end on the in-memory network: tap, decode, mix, encode, and inject, with a third peer hearing both other
    tones in the injected leg.

  The host's hooks: `rtc::media::tests` (3), with a stand-in driver thread.
- **Not yet shown:** the mixer on real sockets and real speech, and its CPU cost per room under load. V1's SIP legs and
  W5's mixed recording will exercise them.

**The V1 gate, SIP (2026-10-31): rsipstack passed.** D-V1 put `rsipstack` first and `rvoip` second; rsipstack passed
every case, so rvoip was not tried. rustpbx stayed a design reference: it was not checked out and nothing was copied.

- **Version and licenses:** `rsipstack` 0.7.12 (released 2026-10-29), MIT, with its default features off and
  `platform-tokio` and `rustls` on. It leaves out WebSocket (`tokio-tungstenite`) and SRV lookup (`hickory-resolver`).
  - New transitive crates: `if-addrs` 0.14.0 (MIT OR BSD-3-Clause) and `hashbrown` 0.15.5 (MIT OR Apache-2.0);
    `windows-sys` 0.59.0 on Windows only.
  - Its TLS turns on rustls's `ring` provider beside aws-lc-rs. ring was already linked, through `rustls-webpki`.
    rsipstack builds its TLS configs from the process default, so `moqui_sip::install_crypto` makes aws-lc-rs that
    default.
  - No OpenSSL. `cargo deny check licenses` passes (License-Review).
- **SRTP** is in house (`moqui-sip` `srtp.rs`): RFC 3711's key derivation, packet index and replay window, and the
  80-bit tag. It runs on str0m's public crypto provider, the aws-lc-rs one the browsers' DTLS-SRTP uses. str0m's own
  SRTP context is private. No SRTP crate is added.
- **What is rsipstack's and what is Moqui's.**
  - rsipstack: the transactions and dialogs; the UDP, TCP and TLS transports; CANCEL, including a 2xx that crosses
    it; forked 2xx answers.
  - Moqui: digest for the trunk's realm and algorithms; challenges with expiring nonces and replay checks; the
    per-source block; SDP; RTP; SRTP; RFC 4733 DTMF; the phone side of the media; registration; the outcome of an
    outbound leg (`race.rs`).
- **Tokio.** rsipstack's only host backend is Tokio; its embassy backend is for embedded targets. The user agent
  (`ua.rs`) runs on a two-thread runtime the host's `src/sip/` owns, beside no business logic. The rest is sans-IO,
  tested in memory with the time passed in: digest, the guard, the dial classes, SDP, RTP, SRTP, DTMF, the media and
  the race. The UA cases run on loopback sockets.
- **Build cost:** a Dev build with `sip` is 5.9 MiB larger than one with `rtc`, and its first build is 68 s longer.
- **Cases passed** (`cargo test -p moqui-sip`, 27 tests, 2.7 s together; the fake trunk is a raw SIP peer on a
  loopback socket, and for TLS a second user agent):
  - trunk registration with a SHA-256 challenge; a registrar challenging for another realm gets no credentials;
  - an outbound call answering a 407, with ringing, the answer, media both ways, DTMF (RFC 4733) and a hangup;
  - an inbound call that rings, is answered, carries media both ways and is hung up by the far end;
  - hold by re-INVITE both ways (`sendonly` answered `recvonly`; this side's `sendonly` answered);
  - CANCEL while ringing (487 and ACK), and a 2xx crossing the CANCEL (ACKed and ended with BYE, reported canceled);
  - two forked 2xx: the first wins, the second is ACKed and ended with BYE, and one answer is reported;
  - credentials: failed digests from one address block it (and only it) without touching the account; right
    credentials from another address are taken;
  - a call over TLS with SRTP between two user agents, with media and DTMF;
  - media mixing: two phone legs, G.711 and Opus, each hearing the other through the mix-minus and not itself;
  - each codec over SRTP, jitter and loss concealment, symmetric RTP, and forged or replayed SRTP refused;
  - vectors: RFC 7616's digest (SHA-256 and MD5) and RFC 3711's keystream and key derivation (B.2, B.3).

  On a real server: `cargo test -p moqui-host --test it sip::` (2.2 s). A fake trunk takes an outbound call and
  places an inbound one, and a browser stand-in (a str0m peer) hears the phone through the call's SFU room while the
  phone hears the room.
- **Not yet shown:**
  - a live provider (the Telnyx profile is unverified until an authorized live check, §7.3);
  - interoperation with real PBXes and phones;
  - the server behind NAT;
  - RTCP;
  - load.

**The W7 gate, cascading media nodes (2026-10-31): one str0m peer per room and edge node passed.**

- **The design spiked:** a relay link is a str0m `Rtc` on each node for one room, on the nodes' media sockets. The
  call's node (the origin) is ICE-lite, as to browsers; the other node (an edge) is full ICE, so it is the controlling
  side, and it offers first (with a data channel, so a link with no track yet still connects). The SDP and the rest go
  as `RelayMessage`s on `rtc-signal` (§6.3). The edge does not give way to the origin's offer while its own is out; the
  origin gives way and offers again, as the SFU does to a browser. Each message carries its link id (unique across a
  node's restarts); a new link from a node replaces the old one, and an old link's messages are dropped.
- **Kept:** it reuses the leg peer's negotiation, simulcast and keyframe paths (a relay is a peer flagged as one), so no
  second transport or crypto. A track crosses once per edge, which a peer per pair of legs would not; one link per pair
  of nodes for all rooms would save handshakes but needs its own room multiplexing, and waits for a measured need.
- **Relay tracks** carry what browsers publish, simulcast video too: the sender declares a track's layers and passes
  each frame on with its rid and RTP time. The receiver names the relayed tracks from the offer (leg, kind, source), so
  each node's subscribers, taps and active speaker see the original legs.
- **Bounded forwarding:** a video track crosses in its layers up to the highest any subscriber behind the link wants
  (`Want`, sent when it changes), nothing when none does; the origin's taps want the high layer. Caps: 8 links a room,
  64 a node, past them `Refused` (`tooManyRelays`).
- **Cases passed** (`cargo test -p moqui-rtc sfu::relay_tests`, 6 tests on two SFUs with simulated time, 0.4 s with the
  rest): media both ways over one link, with the speaker found across it and an origin track's audio crossing once for
  two edge subscribers; the layers crossing for the highest wanted, a layer change at the edge, the other layers stopping
  and no video when none is wanted; an edge's simulcast track reaching the origin's tap in all three layers, with its
  source renamed; the caps refusing a link at the origin and at the edge; a lost link reported, a new link replacing
  it, an old link's message dropped; the link closing when the edge's last leg leaves and when the room ends.
- **On real servers:** `cargo test -p moqui-host --test it -- rtc::cascade` (two servers of one cluster on one
  database, the second a child process of the test binary): media both ways, a layer change at the edge, and the edge
  killed ending only its leg.
- **Not yet shown:** a link between two hosts on a real network (the tests use loopback), a TWCC estimate for the link
  (it shares none out), and capacity with many rooms per link.

### 6.3 Signaling

- **Frames.** A new client frame kind on `/notws`, `rtc`, in `crates/moqui-host/src/notws/rtc.rs` beside
  `collab.rs`.
  - From the client:
    - `join`, `leave`, `offer`, `answer`, `candidate`;
    - `publish` and `unpublish` a track (camera, microphone, screen);
    - `subscribe` with the layer wanted (by tile size);
    - `mute`, `hand`, `reaction`.
  - From the server:
    - `joined`, `answer` and `offer` (renegotiation);
    - acknowledgements and structured errors;
    - `participant`, `activeSpeaker`, `quality` and `ended`.
- **Why the socket, not HTTP.** ICE candidates arrive in bursts. A service call per candidate, with a
  NotificationMessage after its commit, adds latency and database work for nothing stored. Frames go straight to the
  node holding the call's media.
- **Writes stay services.** `start#Call`, `join#Call`, `answer#Call`, `leave#Call`, `admit#Guest` and `end#Call` write
  the call state (§5.4).
  - Socket `join` binds an already admitted leg to a connection; it admits no one.
  - Socket `leave` calls the leave service.
  - Handlers check the call, leg, connection generation, and publish, subscribe and moderation rights. An SDP or
    candidate frame grants nothing on its own.
- **Guests.**
  - The call-link page exchanges its high-entropy token for a short-lived guest session. It checks the token hash,
    expiry, revocation and link policy, and records a `CallGuest`.
  - A waiting guest sees only lobby status.
  - Admission gives a credential bound to the call, participant, leg and connection generation.
  - A restricted `/notws` branch accepts that credential only for that call's RTC frames and control updates, never
    `ALL`, general topics, co-editing or room history.
  - Credentials are never query parameters or log fields.
  - Revoking the link or the participant invalidates live credentials and removes the leg.
  - Guests have no chat by default. Call chat needs an explicit, bounded call-only view.
- **Connections.**
  - A reconnect revalidates admission, claims a new generation and fences the old connection.
  - Heartbeats expire abandoned connections after a documented grace period. Stale frames cannot revive a left or
    revoked leg.
  - Negotiation handles offer collisions and candidates for obsolete ICE generations.
  - Room resources close after the last leg's grace period, and history stays.
  - The atomic answer settles competing tabs and devices, and every sibling offer gets the outcome (§5.4).
- **Ringing** rides `chatUser:<id>`, as room events do.
- **Cluster.** A call's media lives on one node (`CallSession.mediaNodeId`).
  - A frame from a page on another node goes to that node over the cluster's client-certificate HTTPS transport. W1
    adds this send to one node (§1); it is never a broadcast.
  - The envelope carries the request ID, the origin node and connection, the call and leg, and the connection and media
    generations. Replies return to that connection, and the target rechecks the binding.
  - Bounded queues per connection and per node reject overload explicitly.
  - Signaling is ephemeral: it is renegotiated after a reconnect, not stored.
  - Media goes straight from the browser to the media node's public address.
  - A dead media owner is fenced, and its legs end with a visible media-node-lost reason. Presence and history are
    reconciled, and users can start a new call.
  - There is no seamless failover. Cascading SFUs (W7) do not provide it.
  - With cascading media nodes (W7, `rtc_cascade_enabled`) a leg's media is on the node of its own socket when that
    node holds media (`CallLeg.mediaNodeId`), linked to the call's node (the §6.2 W7 gate). An edge lost ends only its
    legs; a link lost puts the edge's legs on the reconnect path; a link refused sends them to the call's node.

### 6.4 Media features

- **Codecs.** Opus with DTX and FEC; VP8 with three-layer simulcast first. VP9 or AV1 SVC, and H.264 for Safari's
  hardware, come later.
- **Congestion control.** TWCC bandwidth estimation, layer switching per subscriber and an audio-only fallback.
- **Speakers.** The audio-level header extension, for the active speaker and the speaker view.
- **Screen share.** `getDisplayMedia`, as a separate track at high resolution and a low frame rate.
- **Data channel** for reactions and hands, if the socket's latency is not enough.
- **Captions (W6).** Audio chunks transcribed by the `moqui-llm` endpoint (the `transcribe` binding); local Whisper
  (`speech-local`) later ([WebRTC](../docs/interfaces/WebRTC.md#captions)).
- **Recording (W5, [Call recording](../docs/interfaces/Call-Recording.md)):**
  - per-track Opus and VP8 without transcoding, Opus in Ogg and video in WebM (§10.1), with track and segment rows;
  - a mixed audio recording, from the same mix-minus mixer SIP legs use;
  - video calls played back by playing the tracks together in the browser;
  - a composited video file only as an optional export, through ffmpeg or a browser recorder process kept apart from
    the PDF renderer (as Nextcloud's is), each with its own license review;
  - §5.4's notice, access and retention rules;
  - a recorder failure ends the recording, not the call, and partial files are cleaned up.
- **Queues for captions and recording.**
  - Each has its own bounded queue, timeouts and limits, so a slow provider cannot stall forwarding. Captions show as
    unavailable, and the call goes on.
  - Sending audio to an external provider follows the consent policy.
  - Both refuse encrypted media.
- **End-to-end encryption (W8, D-W3).** SFrame (RFC 9605) in the browser: in JavaScript on WebCrypto (the user's choice
  over the `sframe` crate built to WASM, which needs a wasm-bindgen toolchain; the crate checks the same vectors in Rust),
  in an `RTCRtpScriptTransform` worker (Chrome, Firefox and Safari).
  - A dedicated call-key protocol is tied to the admitted roster, with per-sender keys and counters, and rotation on
    join, leave, revocation and reconnect. It never reuses the chat Megolm session.
  - Key management is checked against an independent implementation before release.
  - The server relays key material it cannot decrypt.
  - Required E2EE refuses unsupported clients, recording, captions and SIP legs, with no downgrade.
  - Chat and call encryption are shown separately.

### 6.5 Network

| Port | Protocol | Use | Switch |
| --- | --- | --- | --- |
| 8080, 8443 | TCP | HTTP and HTTPS: screens, `/notws`, webhooks, the XMPP WebSocket, the Matrix client API | existing |
| 10000 | UDP | WebRTC media (ICE, DTLS-SRTP), every browser leg on one socket | `rtc_media_port` |
| 10000 | TCP | ICE-TCP (passive), for networks that block UDP | `rtc_media_tcp_port` |
| 5060 | UDP, TCP | SIP | `sip_port` |
| 5061 | TCP | SIP over TLS | `sip_tls_port` |
| 20000–20999 | UDP | RTP and SRTP for SIP legs, one port per leg | `sip_rtp_ports` |
| 3478 | UDP, TCP | STUN and TURN | `turn_port` |
| 5349 | TCP | TURN over TLS | `turn_tls_port` |
| 443, own address | TCP | TURN over TLS where only HTTPS gets out | `turn_tls443_host` |
| 49152–65535 | UDP | TURN relays | `turn_relay_ports` |
| 5222, 5223 | TCP | XMPP clients, STARTTLS and direct TLS | `xmpp_port`, `xmpp_tls_port` |
| 5269 | TCP | XMPP server to server | `xmpp_s2s_port` |
| 8448 | TCP | Matrix federation, or 443 with `.well-known` | `matrix_federation_port` |

The switch names follow §2's style and are settled when built; the ranges are defaults.

- **Media.** The media port has a bind address, and a `public-address` for a node behind 1:1 NAT. ICE-lite needs a
  reachable, correctly advertised endpoint; verify the routing, since a configured address doesn't prove it.
- **ICE-TCP.** A browser that cannot reach UDP 10000 tries TCP before TURN, which is cheaper than a relay.
- **Built-in TURN (W4).** It is built on `turn-server-proto`, with coturn the external alternative under the same
  acceptance.
  - TLS on 443 needs its own address, since the HTTPS listener does not demultiplex TURN ([Nextcloud's
    guide][talk-turn]).
  - Relays have their own port range and firewall rules.
  - W4 checks client-to-TURN and TURN-to-media reachability, hairpin NAT included.
- **TURN credentials.**
  - Short-lived HMAC credentials go only to admitted legs, guests included.
  - Quotas limit allocations and bandwidth.
  - Standard TURN credentials don't limit relay peers to one call. The embedded server enforces that binding, and the
    coturn profile restricts relays to media nodes.
  - Revoked credentials stop renewing, and allocations are removed.
  - Tests show arbitrary relay targets refused, and UDP-blocked calls succeeding.
- **Certificates and DNS.**
  - The TLS listeners (SIP, TURN, XMPP, Matrix) use the host's certificates from `runtime/cert`. There is no ACME in
    the host; an ACME companion can fill that directory.
  - The names needed are the domain, the XMPP MUC and upload hosts, and the TURN host.
  - The DNS records are XMPP's SRV records (`_xmpp-client._tcp`, `_xmpps-client._tcp`, `_xmpp-server._tcp`), SIP's
    `_sips._tcp` for phones, and Matrix's `.well-known`.

### 6.6 Screens

The design's Call screen (mr-boss design-drawers-commons §14) moves to the framework unchanged in shape:
- a lobby with camera preview, device pickers, level meter and the guest name;
- grid and speaker views;
- the bottom bar: mute, camera, share, hand, reactions, participants, chat (authorized room members only), settings,
  record and leave;
- the admit prompt;
- the LiveCall mini window.

`m-call` and its parts are framework components, so any app can embed a call, such as a Customer Service video session
or a Projects stand-up, by `chatRoomId`. Embedding never bypasses call or room authorization.

---

## 7. Order

- Each step records its checks and documentation before it is complete.
- "Depends on" lists technical dependencies only, and independent steps can run in parallel.
- **Priority:**
  1. P0 (D-V6);
  2. A0–A4;
  3. the W, V and C streams. Chat runs C1, C4-SMS, C4, C3a, C3b, C7, C6a, C6b and C6c; C2 and C5 come when needed.
- W1 and V1 start with spikes that choose their libraries.
- W3 and V1 need W4 before public use: a call on a reachable local network is not a public release.
- New listeners stay off until configured.

### 7.1 Milestones and acceptance

| Step | What | Depends on | Observable acceptance |
| --- | --- | --- | --- |
| **P0** | PWA install and web push | — | Install, permission refusal, expired subscription, logout and account switch, recovery after missed pushes, no private responses cached (§7.2) |
| **A0** | Providers, actions, panels, shell-providers merge | — | Two providers for one hook both answer; deterministic replacement and order; a failing optional read is isolated; unauthorized direct writes fail in browser and TUI |
| **A1** | Component, mount, Settings extraction | A0 | Framework-only mail settings work without Internal accounts or Mantle; existing account IDs, rules and OAuth routes still work; BOSS's conf hides BasicComms from the launcher |
| **A2** | Chat and ChatPanel extraction | A1 | Framework-only chat works; BOSS record features appear only with authorized providers, through either mount |
| **A3** | Inbox, MessageDetail, Compose and Home split | A2 | Email-only framework; BOSS Internal accounts, mailboxes, recipient lookup, filing and Home cards keep their behavior and paging |
| **A4** | Extensions, split specs, Comms guide | A3 | One launcher entry, both mounts authorized, qvt/qvue and TUI renders checked, no framework Mantle or `ironstone` dependency |
| **W1** | Media spike (str0m), SFU, signaling, call state, Call/LiveCall, ringing, send to one node | A2 | The §6.2 gate; 1:1 and group calls, competing answers, stale generations, cross-node replies, reconnect, media-node loss |
| **W2** | Simulcast, active speaker, screen share, history | W1 | Layer changes under constrained bandwidth, share start and stop, audio fallback; `CallUserLog` history and missed calls correct |
| **W3** | Call links, guests, lobby, meetings provider | W2 | Waiting or revoked guests cannot negotiate media or read room history; expiry, admission and reconnect stay scoped |
| **W4** | Embedded TURN, ICE-TCP, coturn profile | W1 | UDP-blocked calls over ICE-TCP and TLS relay; advertised-address and NAT checks; quotas, expiry, refused relay targets, the dedicated 443 address |
| **V1** | SIP spike (rsipstack), trunks, mixer, caller ID, fraud controls, emergency readiness | W1 | The §5.7 cases; authorized caller ID; mix-minus audio; failed auth blocked per source; international refused until enabled; provider profile and emergency readiness documented |
| **V2** | Voicemail, greetings, PIN, notices, transcription | V1 | Busy and no-answer voicemail, PIN refusal and rate limits, private media, optional notices; a failed transcription keeps the audio and shows its state |
| **V3** | Handling rules, schedules, ring groups, DND, presence, dialer | V2 | Ordered and simultaneous ringing, fallback, business hours and holidays, first-answer race; one ended leg leaves another call's presence |
| **V4** | SIP registrar | V1 | Multiple bindings, expiry, revocation, nonce replay, credential rotation, routing to live contacts; NAT (rport, keepalive, RTP latching); G.722 |
| **V5** | IVR, queues, transfer, merge | V3 | DTMF routing, queue timeout and wrap-up, transfer and answer races, the recording notice on new legs |
| **W5** | Recording | W2 | Notice per leg, late joins and transfers included; pause and resume under the acknowledgement policy; tracks, segments and the audio mix; recorder failure; private downloads; retention |
| **W6** | Captions | W2 | Speaker and chunk order, provider timeout and backpressure, consent; failure cannot stall media; encrypted media refused |
| **C1** | Actor migration, ingestion, durable delivery | A2 | Local behavior kept; crash before and after remote acceptance, duplicate batches, reordered edits, echo loops, one failing destination |
| **C4-SMS** | SMS and MMS | C1 | A provider and number without the SIP engine; duplicate callbacks, receipts, media limits, uncertain sends, consent refusals |
| **C4** | WhatsApp, Messenger, Instagram, Telegram | C1 | Signature and replay failures rejected; retries; permission, window and template limits; unsupported changes refused; Telegram without a public URL |
| **C3a** | XMPP for local users | C1 | Recorded stanzas and the selected clients; SASL2, FAST, SCRAM, channel binding; MUC, MAM, resumption, push; PEP for both OMEMO namespaces; advertised features match results |
| **C3b** | XMPP federation | C3a | s2s with TLS and SASL EXTERNAL, and dialback, against an independent server; only rooms with federation on reachable |
| **C7** | Encrypted rooms, key endpoints | C1 | Browser storage, encrypted attachments, concurrent one-time-key claims, revoked devices, rotation, lost-device recovery, no plaintext side effects |
| **C6a** | Matrix client API | C1, C7 | Element Web and Element X: login, sliding sync, rooms, media, encrypted rooms through C7's endpoints |
| **C6b** | Matrix federation | C6a | Independent server-to-server tests: signing, state resolution, backfill, media; only rooms with federation on reachable |
| **C6c** | Appservice API | C6a | A mautrix bridge running against Moqui; transaction retries; namespace ownership |
| **C2** | Appservice on an external homeserver (optional) | C1 | Transaction retries, state and ID mapping, membership permissions, media; encrypted portals refused |
| **C5** | Slack, Teams, Discord | C1 | Versioned capability matrix, authorization and retry cases per connector |
| **W7** | Cascading SFUs | W2 | Cross-node subscriptions, bounded forwarding, node-loss outcomes |
| **W8** | Encrypted calls | W2, C7 | Independent key-protocol interop, roster changes and reconnect, unsupported clients refused, no downgrade, no recording, captions or SIP |

### 7.2 PWA, notifications and mobile

**P0:**
- The manifest and service worker.
- Push subscriptions per signed-in user and device, after a user gesture and permission, signed with the VAPID key.
- [WebKit's push requirements][web-push] include visible notifications. On iOS, web push works only for a web app
  added to the Home Screen.
- Push is not a phone ring. It promises no background ringing or media access. Record the supported browsers and OS
  versions, with their limits.
- Expired and rejected endpoints are handled. Logout unsubscribes, and subscriptions never cross an account switch.
- Only static shell assets are cached, never authenticated responses, messages, recordings or guest credentials.
- Encrypted-room notifications carry no plaintext.
- In-app notifications and history work without push permission.

**From W1:**
- Call invitations expire. An old push opens the current call state or the missed-call history, and never revives a
  finished invitation.
- On reconnect, clients refresh unread and call state from the server; push delivery is not authoritative.

**Mobile.**
- **Messages:** the PWA, with web push.
- **Native chat apps:** XMPP clients (C3a), which push through their own services (XEP-0357), and Element X with C6a.
- **Native calls:** SIP soft phones registered to Moqui (V4).
- **A native Moqui app:** later ([Usability](proposals/Usability.md)).

### 7.3 Verification and rollout

- **Gates.** A library named here is a candidate until its spike passes. The spike records:
  - versions and release dates;
  - licenses, transitive ones too;
  - results.

  A gate that could not be run is open, not passed. A release label, a local fake-media call or a refusal notice is
  not evidence for the cases it doesn't exercise.
- **Fast checks.**
  - Focused Rust tests with simulated clocks and in-memory peers, and Moqui server and client specs. Node is not a
    test dependency.
  - Negative authorization cases use the same valid parameters as the positive ones.
  - Crash and retry boundaries and migrations are tested as behavior.
  - Ordinary tests stay well under 15 seconds.
- **Browser and interoperability.**
  - Fake-media Chromium proves the application flow.
  - Independent clients and provider-approved live checks prove the selected interop and deployment cases.
  - Browser, real-provider, interop and other slow runs need explicit permission for the specific scenario, when its
    result is needed.
  - Over about 15 seconds is slow, and over 30 seconds is marked ([Build-Test](../docs/Build-Test.md#slow-tests)).
    Never pass `--slow` without that permission.
  - Record versions, topology, results and the cases left unverified.
- **Rollout.**
  - Pilot each connector and listener with bounded users and rooms, with the switches off elsewhere.
  - Monitor:
    - pending-delivery age and failures;
    - admission failures and stale connections;
    - media loss, jitter and resource limits;
    - recording and caption failures.

    Monitoring carries no message content, credentials or high-cardinality participant labels.
  - Disabling a connector keeps its pending deliveries.
  - Draining a media node stops new admissions, and the active calls finish.
  - Actor and homeserver migrations need backups and a rehearsed rollback (§4.3, §4.4).

### 7.4 Built

| Step | What was built | Commits (mr-moqui, mr-boss) | Tests |
| --- | --- | --- | --- |
| P0 | The `/qapps` manifest, icons and service worker (`sw.js`: versioned shell assets only); `WebPushSubscription` and `get#WebPushStatus`, `store#`, `delete#`, `send#WebPush` (`service/web_push.rs`); RFC 8291 and 8292 on `p256`, `hkdf`, `aes-gcm` (`notify/push_crypto.rs`, the `web-push` feature in every package); persisted notifications pushed per `pushNotifications`; the account menu's Notifications on this device (`js/WebrootPush.js`) as the only permission prompt; logout and account switch end a subscription | 79f11727 | RFC 8291 vector, VAPID, `sw.js` headers; `WebPush.spec.js` |
| A0 | `shell-providers` merged by element and key in a screen-extend; the provider hooks and `call#Providers`, `get#Providers` (each as its app, authorization required even under a caller that disabled it); `ext-action` and `ext-panel` (`screen/ext.rs`, `/extItems`, the menu data, `js/WebrootExt.js`, the terminal drawer) | e67efdca | `definition_tests`, `ext_tests`, `tests/shell.rs`, `moqui-tui` `shell` |
| A1 | `basic-comms` with the mail settings and `MailSettingsServices`; BOSS mounts them and hides BasicComms | c5fd370b, 4b32c17 | `BasicComms.spec.js`; BOSS MailAccounts, MailSettings |
| A2 | Chat, the tray panel and chat settings; records through `record-lookup`, `record-link`, `create-from`; ext items with fixed values and `hook` | 3048ffea, 32ff28b | `BasicCommsChat.spec.js`; BOSS Chat, ChatPanel, ChatSettings |
| A3 | Inbox, MessageDetail, Compose, Home; `run#Operation` (D-A8) with email-only framework services | c89628ba, 1ba7cd2 | `BasicCommsInbox.spec.js`; BOSS Inbox, Compose, CommsHome, InboxMailboxes |
| A4 | The dependency check, [docs/tools/Comms.md](../docs/tools/Comms.md), the attack surface | 6bd856fd | the whole of `basic-comms` and the BOSS specs above, AppStructure, TuiRender |
| W1 | `crates/moqui-rtc`: the SFU on str0m, with simulcast layers by TWCC, audio only below a floor, glare, generations, taps and injected sources. `moqui.call` (CallEntities) and `CallServices`/`Calls.js`: the state machine, ringing (chatUser pushes, Web Push with `ttlSeconds`), presence, and cleanup (`expire#Calls`). The host's media node (`src/rtc/`, `rtc_media_port`) and `/notws` rtc frames (`notws/rtc.rs`). `cluster::send_to` and the `rtc-signal` topic. BasicComms Calls, History, Call (`m-call`), CallTui, Settings/Calls and the Call button in Chat. `WebrootCall.js`: the store, the ring bar, LiveCall and m-call. BOSS mounts Calls | 45ab9425, ebc570d; with W2 | `moqui-rtc` (14); `cluster::tests::a_direct_send…`; `it rtc::` (a direct call on a real server, 3.4 s); `Calls.spec.js` (11); `BasicCommsCalls.spec.js`; `BasicComms.spec.js`; BOSS AppStructure, TuiRender, Chat, ChatPanel, CommsHome; client `CallFlow` (two Chromium pages with fake media) |
| W2 | The SFU's active speaker (the audio level extension, held for two looks) and a published track that stops (a screen share ending). In the browser: screen share by the page's own offer, the speaker view and the grid, layers asked by tile size (only the speaker off the call's screen), raised hands, reactions, the participants list, the audio-only notice. `find#Calls`, `count#MissedCalls`, `mark#MissedSeen`; History's All, Missed, Placed and Answered with Call back; Missed calls on Home (BasicComms and IronStone) and in Mantle's shell counts (`calls`) | 45ab9425, ebc570d, mr-mantle 59a57970; with W1 | `moqui-rtc` speaker and share cases; `Calls.spec.js` history; `BasicComms`, `BasicCommsCalls`; BOSS CommsHome, AppStructure, TuiRender; client `CallFlow` (hand and speaker view too); `tui_screens` goldens (the new enumerations) |
| W3 | `CallLink` and `CallGuest`, `CallSession.callLinkId`. `create#`, `find#`, `revoke#`, `open#CallLink`, `check#CallLinkToken` (the `call` token route), `join#AsGuest`, `get#GuestState`, `bind#GuestConnection`, `leave#AsGuest`, `admit#`/`deny#Guest`, `remove#Participant`; empty meetings and expired guest credentials in `expire#Calls`. The guest socket `/callws` (credential in its first frame, its own call and leg only, checked every 30 s). CallGuest (`m-call-guest` in the public shell); in `m-call` the lobby prompt and Remove; Call links and Meetings in History; the dock's notice of a waiting guest. IronStone's `meetings-source` over its calendar | 027b45ea, 338e41b | `Calls.spec.js` (links, lobby, admission, revocation, straight in, empty meeting); `it rtc::a_guest_by_link…`; `BasicCommsCalls` (Call links, Meetings through both mounts); client `CallGuestFlow` |
| W4 | `moqui-rtc` `turn/`: the embedded TURN server on `turn-server-proto` (TURN REST credentials, revocation, relay peers only the media nodes, quotas, stream framing). The host: ICE-TCP (`rtc_media_tcp_port`, RFC 4571, `rtc/tcp.rs`), the TURN listeners (UDP, TCP, TLS, 443) and relay sockets (`rtc/turn.rs`), revocation when a leg leaves. `CallMediaNode.turnUrls`; `get#IceServers`, `get#GuestIceServers`, `make#IceServers` (Rust: the credential); the browser fetches its ICE servers before its peer connection. `docs/interfaces/TURN.md` with the coturn profile | b75e9e21 | `moqui-rtc` turn (6); `it rtc::ice_tcp_and_a_turn_relay…` (a relay-only peer and a TCP-only peer on a real server); `Calls.spec.js` (ICE servers only for an admitted leg); client `CallFlow`, `CallGuestFlow` |
| DP | The drive picker in BasicComms: `FilePickerServices` (`get#PickerData` with `hasDrive`, `list#PickerFolder`, `find#PickerFiles`, `get#FileType`) over `FileServices`, its transitions in `template/FilePickerTransitions.xml`; `InboxServices.save#MessageFiles` (mail) and `save#FilesToDrive`; the buttons left out with no drive. IronStone keeps `pickerFolder` (the records drive's titles) and its internal messages' Save to Files, which hands mail to the framework | fbd4ce43, f1b0a3eb, 0ae962a | `BasicCommsFiles.spec.js`; BOSS Files, Inbox, RecordFiles, Compose, AppStructure, TuiRender |
| M0 | The audio core of V1, in `moqui-rtc` `audio/`: Opus on `opus-rs` (the §6.2 gate) with concealment and FEC, G.711 μ-law and A-law, G.722, a polyphase resampler, `Codec` by SDP name, `LegDecoder` (a jitter buffer by RTP time), the mix-minus `Mixer` and `AudioRoom`. Taps: several per room (`add_tap`, `remove_tap`), injected frames tapped too, a leg leaving told to them (`TapEvent::Left`), `QueueTap` (bounded, drops and counts). `AudioRoom` decodes a leg's microphone and screen-share audio as separate streams of one input. The host's hook points (`src/rtc/media.rs`, off stub in `rtc_off.rs`): `attach` a consumer to a room, `add_pcm_source` for an injected leg, paced on the media thread | 9f90f15d, 78f467d5 | `moqui-rtc` audio (13, with malformed Opus and a leg's two audio streams) and `sfu::audio_tests` (2: a stalled consumer; tap, mix and inject heard by a peer, then its leave); `rtc::media::tests` (3) |
| W5 | Call recording. `moqui-rtc` `record/` (sans-IO, in house): Ogg Opus (OpusHead, OpusTags, pages with their CRC, granule positions from RTP time, a gap as a granule jump), WebM VP8 (EBML: Segment, Cluster, SimpleBlock with keyframe flags, Cues), the per-track and mixed-audio recorder with segments at pause and resume and at size and time caps. The host: `rtc/recorder.rs`, a thread per recording on the media node (a `Kinds::All` consumer, an `AudioRoom` on a 20 ms tick encoded with `OpusEncoder` for the mix, a bounded queue, a spool under `runtime/tmp/call-recording`, keyframes asked of the room's publishers while a video track waits for one), started and stopped by `send#MediaControl` (`action: 'recording'`); the `rtc` frame `recording` to every leg and again to a late joiner (the driver's `RoomNotice`, one notice of each kind per room, which the captions notice shares). `CallRecording`, `CallRecordingTrack`, `CallRecordingSegment`, `CallRecordingConsent`; `CallRecordingServices` and `CallRecordings.js`: `start#`, `stop#`, `pause#`, `resume#Recording`, `get#RecordingState`, `ack#RecordingNotice`, `withdraw#RecordingConsent`, `find#Recordings`, `get#RecordingFile`, `delete#Recording` and `expire#Recordings` (job; with the recording's `CallTranscript` rows), `store#RecordingSegment` and `store#RecordingFile` (Rust) to `dbresource://moqui/call/recording/`, `note#RecorderEnd`, `note#CallChange` (Calls.js: a leg bound, a leg gone, the end), the hook `notify#LegRecording` for phone legs (V1). The `call_recording_*` properties (off by default). In `m-call`: Record, the indicator, the acknowledgement prompt and Withdraw; the guest page too. BasicComms Calls, Recordings: the list, a player of a period's tracks together, downloads, Delete; RecordingsTui. `docs/interfaces/Call-Recording.md` | 7680f1a3, a54ba33f | `moqui-rtc` record (8: Ogg pages, checksums and granule positions, a gap as a granule jump, a packet too big for a page dropped, a WebM walked as EBML with its clusters and cues, segments by period, cap and leg, layer switches and the keyframes they wait for, the size limit); `moqui-rtc` `sfu::tests::a_room_s_publishers_are_asked_for_keyframes`; `rtc::recorder` tests (4: a session with frames in memory, a failed store, the size limit, the limits); `it rtc::a_recorded_call…` (a recorded call on a real server: the notice on both sockets, a track per leg and the mix stored as Ogg, 2.4 s); `CallRecording.spec.js` (9); `BasicCommsCalls` (Recordings, the file refused for others); `BasicComms` (every screen in qvt and tui) |
| W6 | Captions. The transcription seam: `moqui-llm` `speech.rs` (`transcribe`: the `transcribe` binding, the user's `api` quota, an `LlmCall`; `transcribe_with`, which dictation now shares; `provider`), `moqui_core::llm_hooks::{transcribe, speech_provider}` (`notBuiltIn` without `llm`), `org.moqui.impl.SpeechServices.transcribe#Audio` and `get#SpeechProvider` (internal). The host's `rtc/captions.rs`: a consumer and an `AudioRoom` per call on its own thread, chunks gated by level and cut at a pause or `call_captions_chunk_seconds`, 16 kHz WAV, a bounded queue and workers on the service pool with a timeout each (at most half the pool across a node's calls), results in speaker and chunk order, the oldest waiting chunk dropped as a gap, `captions unavailable` on failure; the `Transcriber` trait; the driver's `ToRoom` and `RoomNotice` (the notice to each leg that joins later). `speech-local`: a host feature off in every package, a stub answering "not built in". `CallCaptionServices` (`start#`, `stop#Captions`, `get#CaptionState`; `CallCaptions.js`: the switch `call_captions_enabled`, in the call, not ended, not end-to-end encrypted, a provider) through `send#MediaControl`; `CallCaptions` and `CallTranscript` (appended to CallEntities; `call_captions_store`). `m-call`: CC, the notice, the speaker's lines, unavailable; `m-call-guest` the notice and lines. The spec harness's `withProperty` | 086466c9, ba1bdb16 | `rtc::captions::tests` (10: order with answers out of order on real Opus and simulated time, timeout, backpressure gap, silence, a hung provider with no frame dropped, the node's cap on jobs); `moqui-llm` `speech::tests` (4, a fake endpoint on loopback); `CallCaptions.spec.js` (5) |
| V1 | `crates/moqui-sip` on rsipstack (the V1 gate in §6.2): the UA and B2BUA (`ua.rs`); trunk registration; digest for a trunk's realm and algorithms, with nonce and replay checks; the per-source block of failed digests; inbound digest only for a PBX trunk; per-source request rates and caps on transactions and stream connections (`RequestGate`); SDP for PCMU, PCMA and Opus with telephone-event; RTP and SRTP by SDES; RFC 4733 DTMF; hold by re-INVITE; CANCEL and forked answers; the phone leg's media. The host's `sip` feature (`src/sip/`): `sip_port`, `sip_tls_port` and `sip_rtp_ports`; each leg's audio is a `PcmSource` in the call's room, and gets the room's mix-minus; the engine checks the dial again; the control `prompt`, which plays a call's notices to a phone leg (`prompt.rs`: a tone pattern each, then the WAV that `call_recording_prompt_audio` or `call_captions_prompt_audio` names). `send#SipControl` (Rust); `notify#PhoneLeg`, the one path of the phone's notices: W5's `notify#LegRecording`, its acknowledgement by pressing 1 (`ack#LegRecordingNotice`, `CamDtmf`) and W6's captions notice, a phone that answers later included. `VoiceEntities` (`VoiceTrunk`, `PhoneNumber`, `PhoneNumberUser`, `EmergencyLocation`, `VoiceUserSetting`, `VoicePolicy`, `VoiceAuthFailure`, `SipNode`) and `VoiceServices`/`Voice.js`: trunks and numbers, `place#PhoneCall`, inbound DIDs ringing their user, caller-ID grants, the fraud limits, the emergency path, `get#VoiceCapabilities`. `Calls.js` phone legs. System/Voice (Status, Trunks, Numbers, Caller IDs, Fraud, Emergency); Call a number in History; the phone tile and keypad in `m-call`. [docs/interfaces/SIP.md](../docs/interfaces/SIP.md) | 8df4ec6d, 29f56d6f; the merge with W5 and W6: c3afcf99 (the phone notices) and 3b2842c4 (the security follow-up) | `moqui-sip` (36: a provider trunk's digest refused for a call in and a PBX trunk's taken, a flood held per source, inbound ranges no wider than a /8 or a /32); `it sip::` (an outbound and an inbound call through a fake trunk, heard by a browser stand-in, 2.2 s); `sip::tests` (5: the notices' tones, WAV prompt audio, prompts queued); `Voice.spec.js` (12, a phone leg's notice and its 1, inbound digest only for a PBX and the inbound ranges refused); `Calls.spec.js`; `BasicComms.spec.js` (the Voice screens in qvt and tui); `tui_screens` goldens (the new enumerations) |
| V2 | Voicemail. The host's `src/sip/voicemail.rs`: the call's state (`Machine`, no I/O: the greeting and beep, the recording until hang-up, `#` or `voicemail_max_seconds`; `*` or the access number to a PIN, then the new messages with `7` delete, `9` save, `#` next), the engine's ring timer from `receive#PhoneCall`'s `voicemailSeconds`, `busy` straight to voicemail, the control `voicemail` from `decline#Call`; prompts as tone patterns plus the deployment's WAVs (`voicemail_prompt_location`). `src/sip/audio.rs`: Ogg Opus read and written (W5's `record/ogg.rs` writer, M0's Opus), WAV through `prompt.rs`, `convert#GreetingAudio` registered as a framework extension (`moqui_host::sip::`, the `sip` feature). `leg.rs`: a voicemail prompt played in place with its end told (`Play`, `Played`), the phone's audio recorded to a spool (`Record`, `Recorded`, kept when the leg ends). moqui-core `store#VoicemailFile` (beside `store#RecordingFile`, `store_spooled`). `VoicemailBox`, `Voicemail`, `VoicemailPinFailure` (VoiceEntities V2 block), `CallTranscript.voicemailId` related; `PnrVoicemail`; `VoicemailServices`/`Voicemail.js`: routing, `begin#` (Calls.js `phoneToVoicemail`: the user missed it), `store#Voicemail` (an audio that does not store keeps the message), notices (chatUser push, Web Push, email with the audio and transcript off by default and a warning when turned on), `check#VoicemailPin` (argon2, per box and per source, the same answer for no box), `act#PhoneVoicemail`, the owner's find, get, audio, mark, delete, restore, empty trash, retry, settings, PIN and greeting; `transcribe#Voicemail` (`voicemail_transcribe`, `CtsPending`/`CtsDone`/`CtsFailed`), `expire_VoicemailTrash_daily`. Calls History's Voicemail tab (Inbox and Trash, heard and unheard, the unheard count), `Calls/Voicemail` (player, transcript and Retry, mark, delete, Call back, download, `voicemail-actions`) and `VoicemailTui`; `Settings/Phone` (greeting recorded with MediaRecorder or uploaded, PIN, notices, rings) and `PhoneTui`; the Home Voicemail card (D-A4). [Voicemail](../docs/interfaces/Voicemail.md) | f2053e58, 6c835fe5 | `sip::voicemail::tests` (8: no answer to a recorded Ogg until `#`, the limit, the PIN wrong then right and the keys, a block or three refusals, the access number, a silent caller asked again then ended, prompts, a WAV greeting to Ogg Opus); `it sip::a_busy_user_s_caller_leaves_a_voicemail…` (a fake trunk on a real server, about 3.5 s of call); `it sip::the_access_number_takes_a_box_and_pin…` (the box number and PIN, a message played and deleted with `7`, about 5 s); `Voicemail.spec.js` (10); `BasicCommsCalls` (the Voicemail tab and its audio for its owner only); `BasicComms` (every screen in qvt and tui); `tui_screens` goldens (the new enumeration) |
| V3 | Call handling, schedules, ring groups, DND, presence, the dialer (§5.3, §5.4, §5.5). `CommsSchedule`, `CommsScheduleRule` (weekly hours, dated hours, holidays once or yearly, the time zone), `RingGroup`, `RingGroupMember`, `CallerList`, `CallerListEntry`, `CallHandlingRule`, `CallHandlingSetting` (call waiting, the mobile and its schedule, Hide my number), `CallRoute` (a call's plan and step) in VoiceEntities' V3 block; `VoicemailBox.ringGroupId` related; `VoiceUserSetting.forwardEnabled`; `CprForward`. `CallHandlingServices`/`CallHandling.js`: the plan of an inbound call (`inbound#PhoneCall` from `receive#PhoneCall`) from the user's rules in order (anonymous and selective reject, selective accept and forward by caller list, DND to voicemail, always, busy without call waiting, the ring with simultaneous ring to a mobile, no answer) or a ring group's strategy (simultaneous, in order, round robin with its state; DND and busy members skipped, each member's mobile too) and fallback (its box, another group, a number, the end), schedules read in their time zone (`ec.l10n.format(value, pattern, locale, timeZone)`, Java's overload), forwards bounded (`call_forward_max_hops`, loops, the caller, our own numbers routed inside); `advance#CallRoute` through `begin#Voicemail` (`mode: ring`); `evaluate#CallHandling` (the plan at `atMs`); the user's rules, lists, schedules and settings; the admin's groups and schedules; `get#DialerData`, `search#Dialable` (the first caller of `recipient-source`, else the framework's users with a number), `normalize#PhoneNumber`. `Voice.js`: `forwardCheck`, `dialForward` (the full fraud policy and the forward permission, the DID as caller ID), ring groups' numbers as caller IDs, `PnrRingGroup`, Hide my number (`Privacy: id`). `Calls.js`: `ringStep`, `closeOffers`, `forwardEvent`, forwarded legs, the first answer across every fork of an inbound call (`cancelOffers`). `Voicemail.js`: a group's box read by its members, heard state shared, the plan's box. `moqui-sip`: `EndReason::Declined` (603, 600) apart from `Busy` (486). The host: `declined`, 433 and 603 refusals, the ring timer replaced by a plan's next step (`RingTimers`), `mode: ring`, `Privacy`. BasicComms: Calls/Dialer and the tray's Panel/Dialer (`Dialer.qvue`, `DialerTui`; `STRT_TRAY_DIALER`), Settings/Call handling (`CallHandling.qvue`, `CallHandlingTui`), System, Voice, Ring groups and Schedules; Numbers to a ring group; Fraud's Forwards. IronStone: `recipient-source` (`get#RecipientSource`: parties and contacts with their numbers), the Home Voicemail card, the Dialer tray tab. [SIP, Call handling](../docs/interfaces/SIP.md#call-handling) | ae35dd79, 0823a49; the review fixes 8970426d, mr-boss d8680b8; a96b684a (a stale ring-step control) | `CallHandling.spec.js` (8: anonymous, selective, hours and holidays across a time zone, loops and hops, DND and call waiting, forward permission and the mobile's first answer, presence with a phone leg and a tab leg, ownership); `RingGroup.spec.js` (7: admin only, simultaneous with the first answer, a member's mobile, in order to the group's box, the shared box, round robin, schedule and fallbacks); `SipDevice.spec.js` (486 against 603); `moqui-sip` `a_busy_phone_ends_only_its_ring_and_a_decline_declines`; `sip::tests` (3 more); `ec_extra` (the zone); `BasicComms.spec.js` (the new screens in qvt and tui); BOSS `CommsHome` (the card, the provider for its owner only), `AppStructure`, `TuiRender`; `tui_screens` goldens (the new enumerations) |
| V4 | The registrar for desk and soft phones (§5.2, §7.2 native calls). `moqui-sip`: `registrar.rs` (sans-IO bindings: several per device, expiry between the minimum and maximum, 423 with `Min-Expires`, `Expires: 0` and `*`, Call-ID/CSeq ordering, the credential revision, RFC 5626 Outbound keyed by instance and reg-id with `Flow-Timer`, a short expiry behind a NAT, a stream flow's close ending its bindings, at most 10 a device); `ua_phones.rs` (REGISTER and a phone's INVITE answering the challenge against stored HA1 verifiers through the host's `DeviceDirectory`, SHA-256 first, MD5 only where the device has an MD5 verifier; the same nonce, replay and per-source block as V1; the flow by `rport`; a call forked to every live binding over its flow, the first 2xx winning, the others canceled or ended with BYE; `UaEvent::Binding`, `DeviceAnswered`; `IncomingCall.device`); `digest::verify_ha1`, `digest::ha1`; G.722 in the SDP (static PT 9, 16 kHz on its 8000 clock, the RTP step fixed in `media.rs`) and `DEVICE_CODECS`; `PhoneMedia::allow_source` (plain RTP also from the phone's signaling address, on any port until it latches). The host (`src/sip/phones.rs`): `sip_registrar_enabled` (off by default), `sip_domain`, `sip_registrar_*_expires`; `get#SipDeviceAuth`, `store#SipRegistration`, the `ringDevice` and `dropDevice` controls, `receive#DeviceCall`, `LegCommand::AllowSource`. Core: `make#SipCredential` (Rust: the password and its SHA-256 and MD5 HA1). `SipDevice`, `SipDeviceVerifier` (encrypted), `SipRegistration` (VoiceEntities' V4 block); `SipDeviceServices`/`SipDevices.js`: `create#SipDevice` (credentials once), `rotate#SipDeviceCredential`, `revoke#SipDevice`, `find#SipDevices`, `ring#SipDevices`, `expire#SipRegistrations` (job). `Calls.js`: a user's devices rung from `ring` beside the tabs, `deviceEvent` (the first answer wins: a phone over the tabs and other phones, a tab over the phones, decline stops them), a calling phone answered when the far end is, `startFromDevice`, `deviceCaller`; device legs left out of a tab's own leg and of the socket checks. `Voice.js` `placeFromDevice`. BasicComms Settings, Desk and soft phones (`PhoneDevices.qvue`, `PhoneDevicesTui`); System, Voice, Phones. [SIP, Registrar](../docs/interfaces/SIP.md#registrar) | 6089e7c2, 3725daf1; with V2 (phones and voicemail ring together, then voicemail cancels the phones; a phone's decline is the user's): db14d294 | `moqui-sip` (54: `registrar` 8 on simulated time; `ua::phones` 8 against in-memory phones over UDP and TCP: SHA-256 and MD5 refused for a standard device, replay, several bindings and unregistering, expiry, rotation and revocation, a fork's first answer, `rport` and an Outbound flow with its keepalive and close, a phone's signed call; `media` G.722 and NAT latching); `it sip::a_registered_desk_phone_rings_answers_and_is_heard` (1.8 s); `sip::tests` (6); `moqui-core` `sip_credential`; `SipDevice.spec.js` (9, phones and a voicemail box); `Voice`, `Calls`, `CallRecording`, `CallCaptions`; `BasicComms.spec.js` (System, Voice, Phones in qvt and tui) |
| V5 | The IVR, queues, transfer and merge (§5.1, §5.3, §5.4, §5.5, §3.3 `call-context`). `IvrMenu`, `IvrOption` (`CallTarget`: a user, a ring group, a queue, a menu, a voicemail box, a number, this menu again, the end), `CallIvrState` (the menus a call went through, at most 20), `CallQueue` (longest idle, round robin, all at once; ring, wait, length, wrap-up and announce limits, fallback, hours, hold music and position prompt), `CallQueueAgent`, `CallQueueEntry`, `CallTransfer` (blind or attended, its target and state), `CallLeg.heldDate`, `CerMerged`, `CerTransferred` (VoiceEntities' V5 block); `PnrIvr` and `PnrQueue` routed. `IvrServices`/`Ivr.js` (menus, options, prompts uploaded and made Ogg Opus by `convert#GreetingAudio`, `route#IvrCall`, targets into V3's plan through `CallHandling.js` `routeTo`), `CallQueueServices`/`CallQueues.js` (`enter`, `tick#QueueCall` on the engine's tick: offers by strategy skipping agents out, paused, in a call, in wrap-up or ringing, an offer that rings out missed and the next offered, wait and length limits to the fallback, abandon; agents' own status and the stats), `CallTransferServices`/`CallTransfers.js` (`transfer#Call` blind and attended with revision and one-open checks, `complete#`, `cancel#CallTransfer`, `expire#CallTransfer`, `add#CallParticipant`, `merge#Calls` on one media node, `convert#CallToMeeting`, `refer#Call`; numbers under the fraud policy of a call the user places: `forwardCheck` `transfer`). `Calls.js`: `afterAnswer`, `phoneLegGone`, `phoneCallOver` (a waiting far end, two bridged phones), a late offer is not a fork, `addPhoneLeg`, `offerUsers`, `transferLeave`, `setHeld`, `mergeInto` (legs and people moved, a browser told `moved`, a phone's engine `move`), `context` (`get#CallContext`: `call#Providers` as the user rung). The host (`src/sip/ivr.rs`): the menu's state machine (prompt, keys and `#`, timeout and retries, invalid), the queue's music and ticks and the caller's place, ringback, hold music and mute (`leg.rs` `Loop`, `Mute`, `Move`), the controls `route`, `ringback`, `connect`, `transferTimer`, `move`, a leg answered once (`sip_answered`); tone patterns only, prompts and music the admin's. `moqui-sip`: a phone's REFER (202, NOTIFY, `UaEvent::Refer`; Replaces refused). BasicComms: m-call's Hold and More (Transfer, Add a participant, Merge another call, Make it a meeting, Complete or Cancel transfer), `m-call-context` in the ring bar and the call (`WebrootCall.js`, one block), Calls/Queues (agent status, XML: qvt and tui); System, Voice, IVR menus and Queues (XML), Numbers to a menu or a queue. Examples: the gallery's two `call-context` providers (one failing on purpose). IronStone: `call-context` (`get#CallContext`: the parties with the caller's number under the Comms app's organization filters, their open requests). [SIP, IVR, Queues, Transfer and merge](../docs/interfaces/SIP.md#ivr) | 53c9b518, mr-boss 5da7150; pre-tasks a96b684a (a stale ring-step control), mr-boss 0588dd8 (the recipient search's organizations); the review fixes fd3b2c44, mr-boss ad6e7c4 | `sip::ivr::tests` (8, simulated time: an option, a key in the prompt, timeout and retries, invalid and its retry, an extension, nested menus, the answer, the tone patterns); `it sip::an_ivr_key_puts_the_caller_in_a_queue_and_its_agent_answers` (a fake trunk on a real server); `moqui-sip` refer target; `Ivr.spec.js` (4), `CallQueue.spec.js` (4), `Transfer.spec.js` (6: ringing, stale and second transfer refused, attended cancel and complete, both orders of the answer and hang-up race, no answer to voicemail, international, premium and emergency refused, the recording notice on a transferred number with its DTMF ack, add and encrypted, merge and different nodes, a meeting); `BasicCommsCalls` (`call-context` with two providers, one failing; a stranger refused); `BasicComms` (the new screens in qvt and tui); BOSS `CommsHome` (the provider for its owner only, never another organization's), Compose; `tui_screens` goldens (the new enumerations) |
| C1 | Chat actors, ingestion and durable delivery (§4.1, §4.3, §4.4). `ChatActor` (a local actor's id is its userId, made on first use by `get#LocalActors` with one create-or-update; an external actor's `XA` sequenced), `ChatExternalUser`, `ChatBridge` (`configJson` encrypted, the inbound token as its hash, status and last error), `ChatRoomBridge`, `ChatEventExternal`, `ChatBridgeInbound`, `ChatBridgeDelivery`, `ChatRoom.federationEnabled` and `agentsReadExternal`; `ChatRoomMember`, `ChatEventMention` and `ChatPollVote` keyed by actor with the local userId kept beside it (no userId key), `ChatEvent.senderActorId`, `CallParticipant.chatActorId` filled (ChatEntities' C1 block). `migrate#ChatActors` (moqui-core `chat_migrate.rs`: the actors, each row's actor by SQL, the three tables rebuilt on their new keys with `EntityFacade::rekey_table` keeping `*_PRE_C1` copies, then the references checked; run from `ChatBridgeSetupData.xml`). The adapter contract `ChatBridgeAdapter` (capabilities, start and stop, deliver) through `Extension::chat_bridge_adapters` or `register_adapter`; the delivery worker (`chat_bridge.rs`: rows written in the event's transaction by `append#Event`, a commit wake and a start and minute sweep, per-mapping order, a leased claim, a stable idempotency key, `lostInFlight` uncertain where the network cannot dedupe, backoff to 8 attempts, rate limits, the unsupported kind failed). `ChatBridgeServices`: `ingest#Event` (security failures refuse the batch, stored before acknowledged, a repeated transaction a no-op, each item in its own transaction, unique mapping, echo, waiting targets, stale edits), `get#EventDeliveries`, `push#DeliveryState`, `retry#Delivery`, `find#Bridges`, `store#Bridge`, `store#RoomBridge`, `end#RoomBridge`. ChatServices take `senderActorId` (send, edit, react, redact) and refuse an edit, delete or reaction a bridge cannot carry (`bridgeUnsupported`, `check#BridgeSupport`); rooms, members and timeline carry the bridges, external members and senders, and delivery. BasicComms Chat (`Chat.qvue`, `ChatTui.xml`): the bridged mark, the external badge, "not connected", the delivery state with Send again; System, Chat Bridges. Callers: `Calls.js`, moqui-llm `chat.rs`, `handle#ChatMessage`, `WebrootChat.js`, the search document, BOSS `ChatDemoServices`, Mantle's chat link. [Chat bridges](../docs/interfaces/Chat-Bridges.md), [Run-Deploy, Chat actors](../docs/Run-Deploy.md#chat-actors) | 3e3e6264 (mr-boss c961c51, mr-mantle 0de4ccd1); review fixes e4e633d3 | `chat_bridge_tests` (7: a crash before and after remote acceptance, one failing destination, duplicate batches and echo loops, reordered edits, security and malformed items, an unsupported edit, local actors); `chat_tests` (20, local behavior kept); `it chat_actors` (the migration on old-shape rows on embedded Postgres: a departed member, a deleted account's history, an agent, reactions, mentions, votes, unread markers; again a no-op); `it lock_then_insert` (one actor per racing join); `moqui-llm` lib (307, with members); `BasicCommsBridges.spec.js` (the bridged room not connected in qvt and tui, the external member and sender, the waiting and failed delivery with Send again, System, Chat Bridges, a member and a stranger refused); the whole of `basic-comms` (28); framework `Calls` (16); example `Chat` (4); BOSS Chat, ChatSettings, ChatPanel, ClientChatDrive, Chatter, CommsHome, AppStructure, TuiRender (89); Mantle `ChatRecords` (8); `WebrootChat` client (17, an external actor's reaction); `tui_screens` goldens (the new enumerations) |
| C7 (core) | The device and key layer of §4.5, for W8's call keys. `ChatDevice` (per user, Matrix-style: public identity keys and the signed `device_keys` as uploaded, fixed once set; name, browser, list revision, to-device stream position, verified and revoked dates), `ChatDeviceKey` (signed one-time and fallback keys; a claim locks the device's row), `ChatToDevice` (a queue per device by stream position, kept until acknowledged) in `ChatKeyEntities.xml`. `ChatKeyServices`/`ChatKeys.js` with Matrix's payloads: `create#`, `find#`, `update#`, `revoke#ChatDevice`, `get#ChatKeyIdentity`, `upload#`, `query#`, `claim#Keys`, `send#`, `sync#ToDevice` (cursor and acknowledgement, device-list changes, key counts), `clean#ChatToDevice` (`clean_ChatToDevice_daily`); Matrix user IDs by the spec's mapping of other character sets (`chat_server_name`); reach only to the user's own devices and those of users sharing a room or a live call (the room check reads `ChatRoomMember` by userId: the C1 seam, `ChatKeys.js` `roomPeers`); a wake on `chatUser:` (`{kind: 'toDevice'}`, no content). The `/ChatKeys` transitions (webroot `ChatKeys.xml`). The browser: `@matrix-org/matrix-sdk-crypto-wasm` 18.9.0 vendored (`libs/refresh.mjs`, its 240 crates and their licenses in `libs/licenses/`), `js/WebrootCrypto.js` (`moqui.crypto`: OlmMachine loaded only when needed, an IndexedDB store per user and device, the outgoing-requests loop, `ensureDevice`, `sendToDevices`, `onToDevice`, `devicesOf`; closed at logout and page hide). BasicComms Settings, My devices (qvue and its terminal twin) with Rename and Remove. [Chat keys](../docs/interfaces/Chat-Keys.md) | 60c1df3f; the review fixes 2376bd49 | `ChatKeys.spec.js` (7: user IDs, fixed identity keys, another user refused with the same parameters, reach by room and live call, 32 concurrent claims of 20 keys, the queue's cursor and acknowledgement with a content-free wake, device-list changes, a revoked device); client `CryptoFlow` (two Chromium pages of two users in a room: an Olm-encrypted to-device message decrypted with its sender's device, no plaintext in what was posted, nothing to a revoked device; 1.1 s); `BasicComms` (My devices in qvt and tui); `browser_libs` |
| W7 | Cascading media nodes (the §6.2 W7 gate). `moqui-rtc` `sfu/relay.rs`: `Sfu::link`, `handle_relay`, `unlink`, `relays`; `RelayMessage` (offer, answer, tracks, want, close, refused) with link ids; relay peers in the room (the origin ICE-lite, the edge full ICE and offering first, glare to the edge); simulcast relay tracks named by leg and source; the layers up to the highest wanted behind a link (`Want`), taps wanting the high layer; caps per room and node, refused explicitly; `RelayUp`, `RelayDown` (lost, refused, empty, ended, replaced); the active speaker by leg across links; `testnet` with a second SFU and a link that can be cut. The host: `Command::Relay`, `Link`, `RoomFrame`; the driver links an edge's room on a join with the call's node, sends relay messages and room frames (notices, participants, relayed mute/hand/reaction, captions) by `rtc-signal` to the room's other nodes, and on a link down puts the edge's legs on the reconnect path (lost), back to the call's node (refused, `note#MediaEvent` `relayRefused`) or ends them (ended); the socket's binding carries the leg's and the call's node. Services: `CallLeg.mediaNodeId`, `mediaNodePinned`; `rtc_cascade_enabled` (off); `bind#Connection` puts a leg's media on its signaling node when that is a live media node, else the call's, and answers `originNodeId`; `expire#Calls` ends a lost edge's legs only (`CerMediaNodeLost`); leg-level media and SIP controls go to the leg's node, a room's close to every node of the call; `merge#Calls` across nodes (phones stay on their node, linked); desk phones ring on the node they registered with (linked). | c9a3a4b1; the review fixes 8bdddb13 | `cargo test -p moqui-rtc` (6 new); `cargo test -p moqui-host --test it -- rtc:: sip::` (2 new two-server cases); `./moq test framework --spec Calls,Transfer,SipDevice` (2 new features, 1 extended) |
| W8 | End-to-end encrypted calls (§6.4, D-W3). The policy: the enumeration `CallEncryptionMode` (`dtls-srtp`, `e2ee-sframe`; any other mode counts as end to end), set at the start and never lowered: `start#Call` `requireE2ee`, a room's `callE2eeRequired` (`set#RoomCallEncryption`, owners and moderators; Room settings in Chat), a call link's `e2eeRequired` (New call and Call links in History); a room's plain call going on refused to an encrypted start (`callNotEncrypted`). A browser takes part only with `scriptTransform` and a crypto device of its user (`answer#`, `join#Call`, `open#CallLink`, `register#CallDevice`: `unsupportedClient`, `noCryptoDevice`); `bind#Connection` refuses a leg with none; `CallLeg.cryptoDeviceId` and `keyIndex` (CallEntities' W8 block). Refused: recording (`encrypted`, before the policy; `canRecord` false), captions, a number (`add#CallParticipant` before the number is read, `addPhoneLeg`), desk phones (`ring#SipDevices`, and `ring` skips them), forwarded numbers (`ringStep`), guests (`encryptedNoGuests`, the guest page told by the route context), merges (`merge#Calls`, `mergeInto`). The roster in the call's view (each leg's `deviceId`, `keyIndex`, `keyGeneration`), pushed as `rekey` when a device is named or a leg binds again; a revoked device's legs leave (`ChatKeys.js` revoke, `Calls.js` `deviceRevoked`). The browser: `js/WebrootSFrame.js` (RFC 9605 on WebCrypto, all five suites; calls use AES_128_GCM_SHA256_128), `js/WebrootSFrameWorker.js` (the `RTCRtpScriptTransform` worker: a counter per sender, no frame out before a key, a frame that does not decrypt dropped, a track decrypted only with its leg's keys; VP8's first 10 bytes of a keyframe and 3 of others in the clear as SFrame metadata, Opus whole), `js/WebrootCallKeys.js` (`moqui.callKeys`: a random base key per call and epoch, KID = keyIndex × 65536 + epoch, sent as the Olm to-device event `io.moqui.call.key` to the roster's other devices and taken only from the device the roster names; rotation on any roster change, the new key used a second after it is sent; the one-tab crypto store, `busy`), `WebrootCall.js` (the device before the media, the transforms on every sender and receiver, the refusal notices). The UI: the encryption chip per call (End-to-end encrypted, or Encrypted to the server), the lobby's and the guest page's notices, each participant's key state and unverified device, no Number or Merge in an encrypted call; CallTui shows the mode and devices and no Answer. Guests are refused, not given ephemeral devices (reach unchanged). [WebRTC, Encrypted calls](../docs/interfaces/WebRTC.md#encrypted-calls) | a7fccfbb; the review fixes 42362cb5 | `./moq test framework --spec CallEncryption` (7: the start, answer and join refused for a browser without `scriptTransform` or with another user's or a revoked device, the same parameters as the positive case; a leg that binds only with its device, a reconnect a new key generation; every refusal beside a plain call that passes; a link's guests; a room's policy and no downgrade; a revoked device's leg); `Transfer`, `CallCaptions` (the canonical mode); `./moq test basic-comms --spec BasicCommsCalls` (an encrypted call through the transitions and CallTui); `./moq test-client webroot --offline --spec WebrootSFrame` (5: the RFC 9605 vectors, 289 headers, 3 AEADs, 5 suites, and the call frames); `cargo test -p moqui-rtc --test sframe` (3: the same vectors and frames against the `sframe` crate); client `EncryptedCallFlow` (three Chromium pages: media both ways under SFrame, a late third joiner getting both keys, recording refused, a leave rotating the keys to the two left; 6 s); `tui_screens` goldens (the new enumerations) |

Left for later:
- `speech-local` (Whisper on candle, §10.1) is a stub that answers "not built in"; captions use the `moqui-llm`
  endpoint. A local Whisper server marked local keeps the audio in house meanwhile.
- Captions in a live call are proven with a fake transcriber in `rtc::captions` and by the policy spec; no client spec
  runs them (headless Chromium's fake microphone is a steady tone, and a real provider is a live check).
- The captions notice is shown to every leg, a phone's as a prompt, but not logged per leg, as the recording's is
  (`CallRecordingConsent`).
- A phone hears the notices as tones unless the deployment sets its prompt audio; Moqui ships no recorded speech.
- Nothing transcribes a recording yet (V2 transcribes voicemail).
- The terminal twin of a call shows no captions.
- BOSS's own links to its Comms (Chatter, the contact record, the palette and demo services) stay `/qapps/Comms`, its root.
- Cascading media nodes (W7): not checked live between two hosts on a real network (the tests use loopback on one
  host); the relay link has no TCP or TURN fallback and shares out no bandwidth estimate; one link per room and edge
  node, not per pair of nodes; no client spec runs a call across two servers in browsers (the `it rtc::cascade` cases
  use str0m peers); a call's node is still chosen at start among the live nodes, not by where its people are.
- Not yet checked: TURN over TLS and on 443, hairpin NAT, and a browser behind a real UDP-blocking firewall. The tests
  relay over UDP on loopback, and reach ICE-TCP on loopback. Check the deployment's network before relying on them.
- The browser shows the missed-call count on Home and the History tab. The `calls` shell count has no header badge yet.
- Screen share and the active speaker are proven in `moqui-rtc` and in code. The client spec does not exercise them,
  because headless Chromium has no fake screen picker and its fake microphone is a steady tone.
- The framework-only Home has no Needs reply: `find#Messages` has no such filter.
- mr-boss `design-drawers-commons.md` §12–14 and §17 still describe the screens in full.
- The hooks `invitation`, `contact-card`, `meetings-source`, `home-source` and `external-conversation` are declared but
  called by nothing yet: IronStone's `inbox-source` answers those parts of the Inbox whole (D-A8). `call-context` has
  its first caller (V5).
- Recording (W5): no composited video export (ffmpeg or a browser recorder process). The Recordings player is checked
  by its screen renders, not with real media in a browser, and the file transition sends whole files (no range
  requests), so seeking far into a long file waits for it. A guest sees the recording state only once let in, not in
  the lobby.
- V1 has not been checked with a live provider, nor V4 with a real desk phone or app (in-memory phones play their part,
  over UDP and TCP; the TLS path is V1's, tested between two user agents).
  - The Telnyx profile is documented and marked unverified until an authorized live check (§7.3).
  - No emergency call was placed. Emergency readiness is per deployment ([SIP](../docs/interfaces/SIP.md),
    "Emergency readiness").
- V1 leaves out:
  - the INVITE without an offer (late offer);
  - RTCP and SRTCP;
  - SRV lookups for a trunk's host;
  - IPv6, which is not tested.
- A trunk that does not register shows no status until a call fails: there are no OPTIONS pings.
- No client spec covers the phone tile and the keypad of `m-call`, records a voicemail greeting in a browser (headless
  Chromium's fake microphone), or drives the Desk and soft phones page (its screens render in qvt and tui; the services
  have their spec).
- Voicemail (V2):
  - The default prompts are tones plus the deployment's recordings (`voicemail_prompt_location`); no text-to-speech
    endpoint is called, as `moqui-llm` has none yet.
  - A voicemail is not an event in the number pair's `external` room: those rooms come with C1.
  - A ring group's box has no greeting, PIN or notices of its own in the screens, and no phone access (V3 routes calls
    to it and lets its members read it).
  - Zimbra's forward and reply by email are not built; download, Call back and the `voicemail-actions` slot are.
- The registrar (V4):
  - no provisioning files for desk phones;
  - a phone registered with another node than the call's rings there only with `rtc_cascade_enabled` (W7);
  - a UDP phone's STUN keepalive is not answered (its CRLF keepalive is);
  - an extension is a colleague's username: no numeric extensions;
- Call handling (V3):
  - a forwarded mobile that its carrier's voicemail answers takes the call: no "press 1 to accept" yet;
  - a forward presents the number the call came in on, not the caller's;
  - no calendar-based schedule from a provider (BOSS's calendar); schedules are hours and dates;
  - the user screens edit only the deployment's schedules' use, not schedules of their own (the services allow them);
  - the tray's Dialer tab is set by IronStone's theme; the framework alone has Calls, Dialer;
  - no client spec drives the Dialer or Call handling pages in a browser (their screens render in qvt and tui; the
    services have their specs), and no `it sip::` case runs a ring group's steps on a real server (the engine's timer
    replacement and `mode: ring` have unit tests);
  - not checked with a live provider: forwards, simultaneous ring and `Privacy: id`.
- The IVR, queues, transfer and merge (V5):
  - no text to speech (no seam exists): prompts are the admin's recordings or tones, and a queue place is spoken only
    with uploaded digit prompts (`call_queue_prompt_location`);
  - a desk phone's attended transfer (REFER with Replaces, RFC 3891) is refused; only its blind REFER is taken;
  - transfer moves a phone call's outside party; a call between users in the browser adds the person instead;
  - merging calls on two media nodes needs `rtc_cascade_enabled` (W7); a merged browser leg joins the new room again
    (a short gap);
  - queue offers follow the engine's tick (`call_queue_tick_seconds`): a newly free agent may wait a tick; no skills,
    priorities, callbacks or queue reports beyond the live stats;
  - no client spec drives m-call's Transfer, Add or Merge, the ring bar's screen pop, or the admin screens in a browser
    (they render in qvt and tui; the services have their specs); not checked with a live provider or a real phone;
  - a person transferred to or added is any enabled user, as for a call started from the Calls page: the framework has
    no organizations, so an app with several (BOSS) does not yet keep transfer and add targets to the user's own;
  - a desk phone's REFER is answered 202 and NOTIFY before the services check it; one they refuse is only logged, and
    the phone believes the transfer went;
  - the engine's retry of a failed queue tick and its ignoring of a finished transfer's timer have no engine-level test
    (the engine has no harness without SIP; `expired_route` has a unit test);
  - a call-context provider's links are shown as given: providers are installed apps, trusted to answer app links.
- Chat actors and bridges (C1):
  - no network adapter: C4-SMS adds the first, and each is checked against its provider (not checked live);
  - an adapter starts on every node that runs the worker; one whose network allows a single session (a long poll) needs
    its own leader election (C4);
  - files from a network are refused (`attachmentsUnsupported`), and outbound deliveries carry text only;
  - mentions of external actors, their read markers, typing and presence are not carried;
  - native endpoints (XMPP C3, Matrix C6) and `federationEnabled`'s use come with them;
  - no client spec drives the bridged room in a browser (the screens render in qvt and tui, and the services have their
    specs); the room settings do not yet show or edit the mappings (System, Chat Bridges and the services do);
  - the `*_PRE_C1` copies the migration keeps are dropped by hand.
- Chat keys (C7 core) built the device and key layer only. The rest of C7 waits for a later pass: encrypted rooms
  (Megolm, room keys over to-device, rotation on membership changes, the screens that explain what an encrypted room
  gives up), key backup with a recovery key, encrypted attachments, emoji and QR verification and cross-signing
  (`ChatDevice.verifiedDate` is unused; `query#Keys` answers no cross-signing keys and the browser drops a signature
  upload). Devices for call guests were not built: W8 refuses guests in encrypted calls instead.
  - The room check of key reach reads `ChatRoomMember` by its local `userId`, which C1 keeps beside the actor key (a
    local actor's id is its userId); external actors have no devices (`ChatKeys.js` `roomPeers`).
  - A retried to-device send is deduplicated only while its first copy is still queued.
  - Not checked live: Element or another Matrix client against these endpoints (C6a serves them), and Firefox and Safari
    (the client spec runs Chromium).
- Encrypted calls (W8):
  - not checked live: the call-key protocol against an independent implementation (it is Moqui's own over Olm; the SFrame
    layer alone is checked against the `sframe` crate), and Firefox and Safari (the client spec runs Chromium);
  - guests cannot join an encrypted call: an ephemeral crypto device bound to the CallGuest needs the key endpoints and
    a wake over the guest's own credential and socket, a surface of its own for a later pass;
  - devices are shown unverified: emoji and QR verification come with the rest of C7, and the roster the browsers send
    keys to is the server's (a server that adds a device of a user in the call gets that leg's keys);
  - the browser's crypto store is one tab's at a time: a call is encrypted only in the tab that holds it (`busy`);
  - VP8 only (its clear payload header): VP9, AV1 and H.264 get their own clear-byte rules when they come (§6.4);
  - a recording, captions or a phone in an encrypted call would need a client of the call's keys on the server side:
    not planned (D-W3);
  - Call back from History starts a plain call, whatever the call it calls back was;
  - no client spec runs an encrypted call across two media nodes (W7): the relay forwards frames as they come, and the
    `it rtc::cascade` cases use str0m peers.

---

## 8. Decisions

These were walked through with the user on 2026-10-30. The review of the same day (§10.3) revised D-C1 to D-C5, D-V1,
D-V5, D-W1 and D-W4, and added D-E1 and D-A7.

| # | Decision | Decided |
| --- | --- | --- |
| D-E1 | External dependencies | Embedded first: every capability runs in the moqui binary, and external servers are options (§2) |
| D-A1 | The `.qvue` extension model | Providers declared like `shell-providers`, declared actions and declared panels (§3.3) |
| D-A2 | The framework Inbox without Mantle | Email accounts only. Internal messages stay Mantle's, through the inbox provider |
| D-A3 | The services package | `org.moqui.comms.*` for screen-facing services; record services, calls and voice among them, are `org.moqui.impl.*` |
| D-A4 | Comms Home | Framework cards (unread by account, mentions, missed calls, voicemail) plus providers' cards. BOSS adds meetings and mailboxes |
| D-A5 | The guest call page (`/call/<token>`), given the Oct 28 rule that public pages go to mr-pass | The framework's public shell, since the framework owns calls. mr-pass can link to it |
| D-A6 | The voice and bridge admin screens | In the tools `System` app, with the files in basic-comms (§5.5) |
| D-A7 | The component and app names | `basic-comms`, mounted as `BasicComms`. BOSS keeps `Comms`, which mounts and extends it (§3.4) |
| D-C1 | The chat order | After C1: SMS and the business channels (C4), then the XMPP server (C3), then the Matrix server (C6). The appservice (C2) is an option |
| D-C2 | A Matrix server in Moqui | Yes, C6: client API, federation, appservices. Moving from C2 needs the migration rehearsal (§4.3) |
| D-C3 | XMPP library licenses | A `deny.toml` MPL-2.0 exception for `xmpp-parsers`, `minidom`, `tokio-xmpp`, `jid` and `sasl`, used unmodified, with a License-Review row |
| D-C4 | Personal WhatsApp, Signal and others | mautrix bridges are a documented option: separate AGPL processes on C6c or an external homeserver; the networks' terms are the operator's. The official business APIs are the supported path |
| D-C5 | End-to-end encryption in chat | Olm and Megolm through matrix-sdk-crypto in the browser (C7), whose key endpoints C6 reuses. C3 carries OMEMO between XMPP clients through PEP |
| D-V1 | The SIP stack | By the V1 spike: `rsipstack` first, `rvoip` second; rustpbx is the design reference |
| D-V2 | Emergency calling | Only through an emergency-capable trunk, with registered locations; otherwise refused with a notice |
| D-V3 | Video to SIP endpoints | No. SIP legs are audio |
| D-V4 | The first trunk and SMS provider | Telnyx, for the docs and authorized live checks. Other trunks need a verified profile. SMS does not depend on the SIP engine |
| D-V5 | Recording consent | Off by default. When on, every leg gets a notice, logged per leg, late joins and transfers included, and staying on is consent. A policy can require acknowledgement, pausing recording while one is missing |
| D-V6 | PWA and web push | Built first, as P0 |
| D-W1 | WebRTC library | str0m, with the SFU written in `moqui-rtc`; webrtc-rs `rtc` the fallback; webrtc-rs `sfu` a reference only, as it exposes no media |
| D-W2 | The SFU for every call, no mesh | Yes |
| D-W3 | Encrypted calls | Later (W8), by call or room policy, with SFrame and dedicated call keys. Required E2EE refuses unsupported clients, recording, captions and SIP legs |
| D-W4 | Embedded TURN or coturn | Embedded, on `turn-server-proto`, with coturn documented |
| D-A8 | An app's Inbox in the BasicComms screens (2026-10-31) | The app's `inbox-source` answers each Inbox, Compose, MessageDetail and Home operation (`run#Operation`), and its page is the list: IronStone's keeps mail and internal messages in one list by date. Without one, the framework answers over the user's mail alone. Nothing is joined |

## 9. Risks

- **Young crates.**
  - webrtc-rs `sfu` has little use, and is set aside (§6.2).
  - rsipstack and rvoip change quickly.
  - The pure-Rust Opus ports are under a year old.
  - The spikes decide on recorded results. The crate seams keep a swap local, but do not remove the fallback's work.
- **An XMPP server from parts.** No Rust XMPP server exists. xmpp-rs gives stanzas, a stream acceptor and SASL
  mechanisms; the rest is Moqui's.
- **Protocol size.** XMPP and Matrix have long tails. The plan targets what common clients need, through a declared
  capability and version matrix, and refuses the rest explicitly.
- **Real networks.** NAT, UDP-blocking firewalls and carrier quirks need W4 and the provider profiles.
- **Toll fraud.** An exposed SIP listener is scanned within hours. The §5.2 controls are release gates, not later
  hardening.
- **Regulation.** Emergency calling, recording notice, SMS registration (10DLC) and WhatsApp's template rules need
  readiness per deployment. Moqui supplies controls and notices; meeting the rules is the operator's job.
- **License traps.** turn-rs before 4.0.1, `audio-codec`'s defaults, `web-push`, the `webm` crate and Piper (§10.1).
  Each spike checks transitive licenses.
- **Build cost.** ruma, the WebRTC stack and candle add compile time and binary size. They stay out of Lite and Dev,
  and each spike measures them.
- **Recovery and retention.**
  - A failed destination must not lose accepted messages or stall the others.
  - Recordings and encrypted attachments need quotas and cleanup.
  - Dedupe metadata outlives retry windows without keeping content beyond policy.

## 10. Libraries and sources

### 10.1 Libraries and protocols

Observed on 2026-10-30 on crates.io, docs.rs and the upstream repositories. Each spike selects its libraries (§7.3).

| Need | Embedded choice | Version, license | External alternative | Notes |
| --- | --- | --- | --- | --- |
| WebRTC | [str0m][str0m] | 0.24.1, MIT/Apache-2.0 | — | Sans-IO, all media exposed; aws-lc-rs, or `rust-crypto` and `dimpl` |
| WebRTC fallback | webrtc-rs `rtc` | 0.21.1, MIT/Apache-2.0 | — | Sans-IO, exposes received RTP. [`sfu`][sfu-api] 0.5.0: reference only |
| TURN, STUN | `turn-server-proto` | 0.7.3, MIT/Apache-2.0 | coturn (BSD-3-Clause) | Sans-IO; TLS by rustls. Or webrtc-rs `turn` 0.17. turn-rs is MIT only from 4.0.1 |
| SIP | [rsipstack][rsipstack] (the V1 gate, §6.2) | 0.7.12 (2026-10-29), MIT | an existing PBX as a trunk | Transactions, dialogs, UDP/TCP/TLS; Tokio, in the host layer. B2BUA, digest policy, SDP, SRTP (on str0m's crypto provider), DTMF and registrar logic are Moqui's (`moqui-sip`) |
| SIP fallback | `rvoip` | 0.4.0, MIT | — | Beta registrar, proxy, B2BUA. `rsiprtp` is UA-only; ezk's media needs OpenSSL |
| SIP reference | [rustpbx][rustpbx] | MIT (community edition) | — | A Rust PBX on rsipstack. Check each file's license before reuse |
| Opus | `opus-rs` (M0, §6.2) | 0.1.40, BSD-3-Clause | — | Pure Rust, no dependencies; its decoder matches libopus 1.6.1 to float rounding. `opus-pure` 0.2.2 is the next pick; `opus` 0.4.0 + `opusic-sys` (libopus via cmake) the fallback |
| G.711, G.722 | in house (M0, `moqui-rtc` `audio/`) | — | — | Bit-exact with ITU-T G.191 and libg722. `ezk-g711` and `ezk-g722` bring the ezk framework and Tokio. Not `audio-codec` defaults |
| Resampling | in house (M0), a polyphase windowed sinc | — | — | 8 and 16 kHz to and from 48 kHz. `rubato` 5.0.1 brings seven crates for general ratios |
| Recording files | in house (W5, `moqui-rtc` `record/`); `ogg`, `webm-iterable` or `mkv-element` were the candidates | —; BSD-3-Clause; MIT | ffmpeg, composited export | No transcoding. Not the `webm` crate. The writers need only the write side with their own page, granule and cluster timing, a few hundred lines each |
| Web Push | in house on `p256` 0.14, `hkdf` 0.13, `aes-gcm` 0.11 (P0) | MIT/Apache-2.0 | — | RFC 8291 and 8292 in `moqui-core` `notify/push_crypto.rs`, checked against RFC 8291's example. `web-push-native` 0.5.0 was set aside: it links a second p256 (0.13), aes-gcm 0.10 and jwt-simple. Not `web-push` |
| XMPP | `xmpp-parsers`, `minidom`, `tokio-xmpp`, `jid`, `sasl`; `rxml` | MPL-2.0 (D-C3); rxml MIT | Prosody (MIT), ejabberd (GPL), with an XEP-0114 component | tokio-xmpp 6 `xmlstream::accept_stream`; no dialback module |
| Matrix | [ruma][ruma] | 0.17.2, MIT | Tuwunel, Continuwuity (Apache-2.0); Synapse (AGPL) | `client-api`, `federation-api`, `appservice-api`, `state-res`, `signatures`, `unstable-msc4186` |
| Chat E2EE | [matrix-sdk-crypto][matrix-crypto], as `@matrix-org/matrix-sdk-crypto-wasm` | 0.19.1, 18.9.0, Apache-2.0 | — | On vodozemac; about 9 MB, loaded for encrypted rooms |
| Call E2EE | `sframe` | 2.1.0, MIT/Apache-2.0 | — | WASM in an `RTCRtpScriptTransform` worker |
| Speech to text | Whisper on candle (later) | candle 0.11, MIT/Apache-2.0; weights MIT | an OpenAI-compatible endpoint | W6 uses the endpoint, through `moqui-llm` (the `transcribe` binding); `speech-local`, opt-in, is a stub until built, with the model downloaded on first use |
| Text to speech | recorded prompts | — | `moqui-llm` | Later: Kokoro on candle with a GPL-free G2P. Piper is GPL-3.0 |

### 10.2 Local application comparisons

Paths are relative to this plan. The review read local working trees at the HEADs shown, and did not build or run
them.

| Reference and HEAD | Source inspected | Lesson applied |
| --- | --- | --- |
| Zimbra web client `a868c61` (2026-10-12) | [ZmCallFeature.js][zimbra-feature], [ZmPhone.js][zimbra-phone] (CPAL-1.0) | Subscribed versus active features; capability-driven settings (§3.1, §5.3) |
| Zimbra mailbox `9121b24` (2026-10-14) | [VoiceInfo.java][zimbra-voice-info] (GPL-2.0), [INSTALL-VOICE.txt][zimbra-install] | Cisco and Mitel supplied the telephony engine (§5.3) |
| Nextcloud Talk `0ab8957` (2026-10-17) | [Attendee.php][talk-attendee], [Session.php][talk-session], [CallController.php][talk-call], [TURN.md][talk-turn], [matrix.md][talk-matrix] | Actors apart from sessions; admission gates; network constraints. Its Matrix integration decrypts on the server, unlike Moqui's E2EE |
| Nextcloud recording `bf25bf7` (2026-10-16) | [Participant.py][talk-recorder], [RecorderArgumentsBuilder.py][talk-recorder-args] | A browser participant plus a capture process, apart from PDF work (§6.4) |
| Odoo `8ff0e720` (2026-10-17) | [message_actions.js][odoo-actions], [discuss_channel_rtc_session.py][odoo-rtc] | Named, ordered actions; stale-session cleanup apart from history (§3.3, §5.4) |
| Frappe Suite `b741fd1` (2026-10-16) | [RoomJoinHandlers.ts][frappe-join], [RoomLifecycleCoordinator.ts][frappe-lifecycle] | Participant and connection ownership, reconnect replacement, delayed cleanup (§6.3) |
| rustpbx (not yet checked out) | to clone into `3p_other` for V1 | B2BUA, registrar and trunks in Rust |

### 10.3 Review findings

Two reviews on 2026-10-30. R1–R9 checked the original plan against the code. R10–R18 checked that revision against
the code, mr-boss and current upstream releases, with embedded first.

| ID | Finding | Resolved in |
| --- | --- | --- |
| R1 | [ChatEntities.xml][chat-entities] keys membership by room and user; a nullable external-user field cannot replace that | Actors and migration (§4.4) |
| R2 | [append#Event][chat-services] needs caller checks; [register_tx_rules][service-call] is in memory | Ingestion and durable delivery (§4.1) |
| R3 | [notws.rs][notws] requires a local user; no guest credential or reconnect ownership | Guests, connections, routing (§5.4, §6.3) |
| R4 | TURN shared HTTPS with no transport design; forwarding alone gives no mixer access | §6.2, §6.5 |
| R5 | E2EE lacked attachment and device lifecycle; W8 reused the chat session | §4.5, §6.4 |
| R6 | Namespaces equated with migration; OMEMO as pass-through alone | §4.3 |
| R7 | One SIP contact per device, call-level consent, no recording IDs | §5 |
| R8 | [The screen merge][screen-definition] lacks providers; [MailSettingsServices][mail-settings] already handles Internal accounts | §3 |
| R9 | Home and order contradicted §8; inCall and Zimbra's license misidentified | §1, §7, §10 |
| R10 | webrtc-rs `sfu` exposes no media, failing W1's own gate | str0m (§6.2, D-W1) |
| R11 | Theme hooks are first-row-only and theme-bound | Providers (§3.3, D-A1) |
| R12 | A screen-extend cannot hide a conf mount; index 95 taken; group checks and the link count (12, not 7) wrong | §3.2–3.4, D-A7 |
| R13 | Matrix federation first meant an external homeserver first | §4.3, D-E1, D-C1, D-C2 |
| R14 | The first review made the consent notice a required acknowledgement | §5.4, D-V5 |
| R15 | No toll-fraud controls, NAT for phones, G.722, or port and certificate plan | §5.2, §6.5 |
| R16 | No guest entity, transcript state, schedules, per-user call state or number messaging link | §5.4 |
| R17 | The XMPP exception missed `jid` and `sasl`; license traps; a "two-week rule" for crates that does not exist | §10.1, D-C3 |
| R18 | "Depends on" mixed order with dependency | §7 |

Kept by both reviews:
- the Mantle-free framework;
- the three extension layers;
- Zimbra as a UC front end;
- the SFU from the first call;
- embedded TURN with coturn available;
- SIP audio only;
- the slow-test policy.

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
[xep-0479]: https://xmpp.org/extensions/xep-0479.html
[str0m]: https://github.com/algesten/str0m
[sfu-api]: https://github.com/webrtc-rs/sfu
[rsipstack]: https://github.com/restsend/rsipstack
[rustpbx]: https://github.com/restsend/rustpbx
[ruma]: https://github.com/ruma/ruma
[matrix-crypto]: https://docs.rs/matrix-sdk-crypto/0.19.1/matrix_sdk_crypto/
[sip-digest]: https://www.rfc-editor.org/rfc/rfc8760.html
[http-digest]: https://www.rfc-editor.org/rfc/rfc7616.html
[emergency-guidance]: https://www.911.gov/issues/legislation-and-policy/kari-s-law-and-ray-baum-s-act/
[mautrix-encryption]: https://docs.mau.fi/bridges/general/end-to-bridge-encryption.html
[web-push]: https://webkit.org/blog/12945/meet-web-push/
