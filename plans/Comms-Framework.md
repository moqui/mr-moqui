# Comms Framework

Status 2026-10-30: plan, no code. The decisions are in [§8](#8-decisions). The libraries, reference applications and
review findings are in [§10](#10-libraries-and-sources).

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
- **Calls.** Media encryption is separate (W8, §6.4). A call that requires E2EE is unavailable before W8 and on
  unsupported clients, and never silently downgrades. Other calls show their encryption mode.

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

### 6.4 Media features

- **Codecs.** Opus with DTX and FEC; VP8 with three-layer simulcast first. VP9 or AV1 SVC, and H.264 for Safari's
  hardware, come later.
- **Congestion control.** TWCC bandwidth estimation, layer switching per subscriber and an audio-only fallback.
- **Speakers.** The audio-level header extension, for the active speaker and the speaker view.
- **Screen share.** `getDisplayMedia`, as a separate track at high resolution and a low frame rate.
- **Data channel** for reactions and hands, if the socket's latency is not enough.
- **Captions (W6).** Audio chunks transcribed by local Whisper (`speech-local`) or the `moqui-llm` endpoint.
- **Recording (W5):**
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
- **End-to-end encryption (W8, D-W3).** SFrame (RFC 9605) in the browser: the `sframe` crate built to WASM, in an
  `RTCRtpScriptTransform` worker (Chrome, Firefox and Safari).
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
| SIP | [rsipstack][rsipstack] | 0.7.12, MIT | an existing PBX as a trunk | Transactions, dialogs, UDP/TCP/TLS/WS; Tokio. B2BUA and registrar logic is Moqui's |
| SIP fallback | `rvoip` | 0.4.0, MIT | — | Beta registrar, proxy, B2BUA. `rsiprtp` is UA-only; ezk's media needs OpenSSL |
| SIP reference | [rustpbx][rustpbx] | MIT (community edition) | — | A Rust PBX on rsipstack. Check each file's license before reuse |
| Opus | `opus` + `opusic-sys` | 0.4.0, MIT/Apache-2.0; libopus BSD-3-Clause | — | Bundled libopus via cmake. V1 tries pure-Rust `opus-rs`, `rusty-opus` (BSD-3-Clause) |
| G.711, G.722 | `ezk-g711`, `ezk-g722`, or in house | MIT | — | Pure Rust. Not `audio-codec` defaults |
| Resampling | `rubato` | 5.0.1, MIT/Apache-2.0 | — | 8 and 16 kHz to and from 48 kHz |
| Recording files | `ogg`; `webm-iterable` or `mkv-element` | BSD-3-Clause; MIT | ffmpeg, composited export | No transcoding. Not the `webm` crate |
| Web Push | `web-push-native` | 0.5.0, MIT/Apache-2.0 | — | RustCrypto; the host's HTTP client sends. Not `web-push` |
| XMPP | `xmpp-parsers`, `minidom`, `tokio-xmpp`, `jid`, `sasl`; `rxml` | MPL-2.0 (D-C3); rxml MIT | Prosody (MIT), ejabberd (GPL), with an XEP-0114 component | tokio-xmpp 6 `xmlstream::accept_stream`; no dialback module |
| Matrix | [ruma][ruma] | 0.17.2, MIT | Tuwunel, Continuwuity (Apache-2.0); Synapse (AGPL) | `client-api`, `federation-api`, `appservice-api`, `state-res`, `signatures`, `unstable-msc4186` |
| Chat E2EE | [matrix-sdk-crypto][matrix-crypto], as `@matrix-org/matrix-sdk-crypto-wasm` | 0.19.1, 18.9.0, Apache-2.0 | — | On vodozemac; about 9 MB, loaded for encrypted rooms |
| Call E2EE | `sframe` | 2.1.0, MIT/Apache-2.0 | — | WASM in an `RTCRtpScriptTransform` worker |
| Speech to text | Whisper on candle | candle 0.11, MIT/Apache-2.0; weights MIT | an OpenAI-compatible endpoint | `speech-local`, opt-in; model downloaded on first use |
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
