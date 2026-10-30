# Comms Framework

Status 2026-10-30: plan, no code. Its decisions were settled the same day ([§8](#8-decisions)). Comms moves from mr-boss into a
framework component, `runtime/base-component/comms`, beside `tools`, and gains three things it does not have:

- chat that reaches outside Moqui (federation and bridges);
- voice over SIP and the phone network, with voicemail;
- web conferencing over WebRTC, with a selective forwarding unit (SFU) from the first call.

IronStone Comms in mr-boss becomes the framework app with BOSS's additions mounted on it or added by screen-extend.
Email is in good shape. It moves with the rest and is refined over time; this plan adds nothing new to it.

Sections: [1 starting point](#1-starting-point) · [2 scope](#2-scope) · [3 the framework Comms app](#3-the-framework-comms-app) ·
[4 chat protocols](#4-chat-protocols) · [5 voice](#5-voice) · [6 web conferencing](#6-web-conferencing) ·
[7 order](#7-order) · [8 decisions](#8-decisions) · [9 risks](#9-risks)

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
  - `ChatRoomMember.inCall` exists and nothing sets it. The `CetCall` chat event type is seeded.
- **Voice and SMS.** No real-time audio exists. Assist's `transcribe` call reaches an OpenAI-compatible endpoint
  ([Assist](../docs/ai/Assist.md)). `org.moqui.SmsServices.send#SmsMessage` is a stub that logs "No SMS implementation in
  place".
- **The socket.** `/notws` takes one kind of client frame, co-editing (`crates/moqui-host/src/notws.rs:427`, handled in
  `notws/collab.rs`). Co-editing crosses the cluster on the topic `collab-update` (`moqui-core` `notify/topic.rs`).

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
  rest go on.
- **Listeners off by default,** as the SSH, SMTP, A2A and MCP listeners are ([Run-Deploy](../docs/Run-Deploy.md)): SIP,
  media, TURN and XMPP each have a conf switch.
- **Licenses.** Each new crate is checked against `deny.toml` and its allow list. MPL-2.0 needs an exception like
  cssparser's; AGPL and GPL code only runs as a separate process. Zimbra is GPL-2.0: its code is a design reference,
  and nothing is copied from it.
- **Crates above core** go through `moqui_core::extension` ([Crate-Split](Crate-Split.md#the-seam-rules)).

---

## 3. The framework Comms app

### 3.1 Placement

- **The component.** `runtime/base-component/comms`, with `component.xml`, `MoquiConf.xml`, `screen/`, `service/`,
  `data/`, `template/` and `test/`. Its `MoquiConf.xml` mounts `component://comms/screen/Comms.xml` under `apps.xml` as
  `comms` ("Comms", menu index 95), as tools mounts `system` and `tools`.
- **The data model stays in `framework/entity`.** The chat entities stay where they are, since `moqui-llm` agents and
  the notification socket use them. The new call, voice and bridge entities go there too (§5.4, §4.4, §6.6). The comms
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

### 3.2 What moves, splits or stays

| Part | Depends on today | Plan |
| --- | --- | --- |
| Settings: MailAccounts and its Add wizard, SignIn, SyncProgress, Removed, Identities, Rules, TextBlocks, OutOfOffice, Chat | `ironstone.comms.MailSettingsServices` (1,637 lines, no Mantle reference), `EmailClientServices` | **Move.** OutOfOffice's one `mantle.party.ContactBookServices` call becomes the recipient hook (§3.4). |
| Chat, ChatPanel, ChatTui | about 20 `org.moqui.impl.ChatServices` calls a screen. BOSS `ChatServices` (753 lines) adds `decorate#Rooms`, `find#RecordRoom`, `find#MessageRecords`, `file#OnRecord`, `create#FromChat`, `search#PaletteRooms` and record cards | **Move.** The record features become hooks. Chat.qvue has no contact or party code, and about 54 record references. |
| Inbox/Mail, MessageDetail, MailTui | `InboxServices` (1,444 lines): the Internal account and mailboxes (Mantle `CommunicationEvent`, `ContactList`), record links, create a task, request or event, the invitation card (`respond#EventInvitation`), the contact card | **Split.** The framework Inbox has the email accounts. BOSS adds the Internal account, mailboxes and records through hooks. Mail.qvue has about 185 lines on internal messages and mailboxes and about 100 on contacts. |
| Compose, ComposeTui | `ComposeServices` (1,035 lines): recipients from Mantle parties and contact books, `create#Message`, `send#EmailDraft` (to file the message on a record) | **Split.** The framework composes and sends email. Recipient search, internal messages and filing are hooks. |
| Home | `CommsHomeServices`: meetings (`get#FullCalendarData`), internal threads, mailboxes | Framework Home with providers, or BOSS only: decision D-A4. |
| Calls, Call, LiveCall, CallSettings | placeholders | **Built in the framework** (§5, §6). |
| Consent, Moderation, Shared/Chatter, RecordFiles | Mantle `ConsentServices`, `mantle.forum`, record timelines | **Stay in BOSS.** |
| Demo data `BOSS_DEMO_*` | Mantle parties | The framework gets its own users-only demo data. BOSS keeps its demo data for its additions. |

BOSS group checks in the services (`BOSS_COMMS`, `BOSS_CONTACT_ADMIN`) become framework permissions that BOSS grants
with `UserGroupPermission` rows.

### 3.3 How an app adds to the screens

**XML screens use `screen-extend`, which already does what is needed.** `merge_extend`
(`crates/moqui-core/src/screen/definition.rs:1165`) supports:
- `transition` and `parameter` replaced by name;
- `subscreens-item` added or replaced;
- `actions-extend` before, after or replacing the actions;
- forms merged by name;
- `section` and `section-iterate` overridden by name;
- `widgets-extend` before, after, at the start of or at the end of a named element.

For example, BOSS replaces the framework Inbox's `threads` transition with one that calls Mantle's
`find#InboxThreads`, which merges `find#Messages`.

Two gaps to close:
- `merge_extend` does not merge `shell-providers`. Add it, merged by `kind` and `group`.
- A `render-mode` can't be replaced. It is not needed if the `.qvue` files stay generic.

**`.qvue` screens need a new extension point.** `screen-extend` works on the server's XML tree, and a `.qvue` is one
client component, so nothing can be added inside it. The proposal has three layers, from the cheapest to the rarest:

1. **Hook services.** The framework already reaches apps through theme values that name a service or screen
   (`STRT_RECORD_VISIBLE_SERVICE`, `STRT_EMAIL_CONTACT_SERVICE`, `STRT_CHATTER_SCREEN`). The comms services ask a hook
   for what an app contributes, and skip the feature when the hook is empty. A hook that fails is logged, and the
   screen renders without its part. The hooks:

   | Hook | Asked with | Answers | BOSS fills it with |
   | --- | --- | --- | --- |
   | `STRT_COMMS_INBOX_SOURCE` | userId, folder, filters, page | extra accounts and their thread rows | the Internal account and mailboxes (`find#InboxThreads`) |
   | `STRT_COMMS_RECIPIENT_SEARCH` | text, kinds | people, groups and lists with addresses | parties and contact books |
   | `STRT_COMMS_RECORD_LOOKUP` | text or id | record cards `{entityName, id, title, status, url}` | `lookup#ById` |
   | `STRT_COMMS_RECORD_LINK` | message or chat event, record | ok, the link | `link#CommunicationEventRecord`, `file#OnRecord` |
   | `STRT_COMMS_CREATE_FROM` | message or event, kind | the new record's url | `create#RecordFromChatMessage` and its mail twin |
   | `STRT_COMMS_INVITATION` | the iMIP part, response | ok | `respond#EventInvitation` |
   | `STRT_COMMS_CONTACT_CARD` | address or number | a card, an "add to contacts" action | `get#ContactCard` |
   | `STRT_COMMS_MEETINGS` | userId, range | meetings with a call link | `get#FullCalendarData` |
   | `STRT_COMMS_CALL_CONTEXT` | a caller number or address | who it is and their open records (a screen pop) | party match and open cases |

   `STRT_EMAIL_CONTACT_SERVICE` and `STRT_RECORD_VISIBLE_SERVICE` stay as they are.
2. **Declared actions.** A new element, `<comms-action>` (the name is open), can sit in a screen or a `screen-extend`.
   Each action names:
   - `slot`: `message-actions`, `thread-toolbar`, `room-header`, `chat-message-actions`, `compose-toolbar`,
     `call-actions` or `voicemail-actions`;
   - `label` and `icon`;
   - `screen` (opened in a dialog or the page) or `transition`;
   - `condition` (an expression over the item's fields).

   The screen's data carries the declared actions, and the component renders them in its menus. This is how BOSS adds
   "Create task", "Link to record" and "File on case". It is the same idea as `shell-providers`' `quick-create`.
3. **Panel slots.** For the few real panels (the contact card pane, a record pane in Chat, the mailbox sidebar), the
   component renders `<m-ext-panel slot="…">`. It loads a screen the host names, through `moqui.loadComponent`, with the
   item as parameters, as `chatter` and `record-files` load theirs.

Hard-coded `/qapps/Comms/...` links in the `.qvue` files (about 7) become relative to `mScreen.path` and `appRoot`.

### 3.4 BOSS after the move

- **BOSS keeps `screen/Comms.xml` as its own root.** The root keeps:
  - the always-actions, with `setup#UserOrganizationInfo`;
  - `shell-providers`;
  - Chatter and RecordFiles.
- **It mounts the framework screens** by location (`subscreens-item location="component://comms/screen/Comms/Inbox.xml"`
  and so on), beside Consent, Moderation and Home.
- **Its additions are `screen-extend` files** under `mr-boss/screen-extend/comms/...`, plus the hook theme values in
  `BossSetupData.xml`.
- **The launcher shows one Comms.** With BOSS installed, the framework's `comms` app is hidden from the launcher by a
  screen-extend of `apps.xml` (`menu-include="false"`), and its URLs stay valid.
- Extending the framework root instead of mounting it would lose `shell-providers` until §3.3's gap is closed, so BOSS
  mounts.

### 3.5 Tests and docs

- **Framework specs.** These move to `runtime/base-component/comms/test` with users-only data: Chat, ChatPanel,
  ChatSettings, Compose, Inbox, MailAccounts, MailSettings and MailSignIn. A new `CommsExtension.spec.js` proves each
  hook and declared action with a test hook service. The framework's `AppStructure`-style render covers every comms
  screen in qvt, qvue and tui.
- **BOSS specs.** InboxMailboxes, Consent, CommsModeration, Chatter and CommsHome stay in BOSS. Inbox and Compose keep a
  BOSS spec for the Mantle parts.
- **Docs.** A new user guide, `docs/tools/Comms.md`. mr-boss `design-drawers-commons.md` §12–14 and §17 shrink to BOSS's
  additions and link to it.

---

## 4. Chat protocols

### 4.1 Approach

The ChatEvent store stays the one source of truth. Every external protocol is an adapter: it maps rooms, users and
events in and out, and keeps an id map for edits, reactions and deletes. A bridged room is an ordinary ChatRoom with a
bridge row, so search, agents, Chatter and retention treat it like any other room. Inbound events are written through
`append#Event` as an external user, and outbound events go to the adapter from an `append#Event` SECA, after the
commit.

### 4.2 The landscape

| Network | Open? | Federation | Reach | How to reach it |
| --- | --- | --- | --- | --- |
| **Matrix** | open spec, open servers | yes, server to server | the most-used open federated chat today: Element, governments (France's Tchap, Germany's BwMessenger, Switzerland), many open source projects | Phase 1: an appservice on an external homeserver. Later: Moqui as the homeserver (D-C2) |
| **XMPP** | IETF standard | yes, server to server | a long-standing base, simpler to serve. Clients: Conversations, Monal, Dino, Gajim | Moqui as an XMPP server: client and server-to-server connections |
| **WhatsApp** | closed | no | the most-used messenger in the world | The WhatsApp Business Platform Cloud API (official, for business numbers). Personal accounts only through a Matrix bridge (D-C4) |
| **SMS and MMS** | carrier networks | — | every phone | The SIP trunk provider's messaging API (§5.6); RCS later |
| **Telegram** | open client API, closed server | no | large | The Bot API (official) |
| **Slack, Microsoft Teams** | closed | Slack Connect and Teams federation are their own | where business teams already are | A Slack app (Events and Web API). A Teams bot (Bot Framework) |
| **Signal, Messenger, Discord, Google Messages** | closed | no | large | Only through the mautrix bridges on the Matrix homeserver (§4.3) |
| IRC | open | network-based | small | Not planned |

### 4.3 Proposals

- **Bridge layer (C1).** Framework entities, §4.4. A `ChatBridge` conf element names each adapter. The adapters
  implement one Rust trait in `moqui-core` (connect, send event, apply inbound) and register through
  `moqui_core::extension`.
  - The Rooms screen marks a bridged room.
  - An external member shows their network's badge.
  - Room settings show the bridge and its link.
  - A bridge that goes down marks its rooms as "not connected" and keeps the local timeline.
- **Matrix, phase 1: appservice (C2).** Moqui registers as an application service on a homeserver run beside it:
  - The homeserver is Synapse (AGPL-3.0, a separate process) or Tuwunel (a Rust homeserver; license to confirm).
  - The appservice owns a namespace of users (`@moqui_<user>:example.com`) and rooms (`#moqui_<room>:…`), so Moqui users
    are puppets on Matrix, and federated users join Moqui rooms as external members.
  - Federation comes from the homeserver. So does every mautrix bridge (WhatsApp, Signal, Telegram, Slack, Discord, Meta,
    Google Messages), each a separate AGPL process the deployment adds.
  - Types and APIs come from **ruma** (MIT): `ruma-events` and the appservice and client-server API types.
  - The mapping is close: the store is already Matrix-shaped (rooms, members, an append-only timeline; edits, reactions
    and redactions as events). That was the design's intent: "keep the event model close enough to Matrix that a bridge
    is possible".
  - Encrypted Matrix rooms are not bridged by the appservice. With C6 they join Moqui's own encrypted rooms (§4.5).
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

    The target is the XMPP Compliance Suites' IM levels.
  - **OMEMO** (XEP-0384): XMPP clients talking to each other may use it. Moqui passes those messages through and
    marks them as unreadable. Moqui's own encrypted rooms use Megolm (§4.5), not OMEMO.
  - **Library:** `xmpp-parsers` and `tokio-xmpp` are MPL-2.0, which needs a `deny.toml` exception like cssparser's.
    Without one, stanzas are parsed on `quick-xml` (MIT). Decision D-C3.
- **Business channels (C4).** `moqui-channels`, each a REST and webhook connector into the bridge layer:
  - WhatsApp Cloud API: templates, the 24-hour customer service window, media.
  - Telegram Bot API.
  - SMS and MMS through the voice provider (§5.6), which replaces the `SmsServices` stub.
  - Slack and Teams later (C5).

  A conversation with an outside person is a ChatRoom of type `external`, with one external member. An app's hook
  (`STRT_COMMS_EXTERNAL_CONVERSATION`) can file it on a record, such as a Customer Service case in BOSS. Inbound webhooks
  are POST transitions with the provider's signature checked before anything is read.
- **Matrix, phase 2: homeserver (C6).** Moqui serves the Matrix server-to-server and client-server APIs
  itself, on ruma: event signing, the room DAG, state resolution v2, backfill and the media repository. Element and
  other clients then connect to Moqui directly. It replaces the external homeserver of phase 1, and the appservice
  namespaces become local users and rooms. This is large. Tuwunel and conduwuit are the references.

### 4.4 Data model (framework, `moqui.chat`)

- `ChatBridge` {chatBridgeId, typeEnumId (Matrix, Xmpp, WhatsApp, Telegram, Sms, Slack, Teams), name, configJson with
  secrets encrypted, statusId, lastErrorText}.
- `ChatRoomBridge` {chatRoomId, chatBridgeId, externalRoomId, direction (both, in, out), fromDate, thruDate}.
- `ChatExternalUser` {chatExternalUserId, chatBridgeId, externalUserId, displayName, avatarUrl, linkedUserId when it is a
  local user's own puppet}. `ChatRoomMember` gains `chatExternalUserId` as an alternative to `userId`.
- `ChatEventExternal` {chatRoomId, eventSeq, chatBridgeId, externalEventId}: dedupe, and maps edits, reactions and
  deletes.

### 4.5 End-to-end encrypted rooms (C7)

A room may be created as encrypted, and it can't be turned off later. Messages use Matrix's Olm (between devices) and
Megolm (a session per room) on **vodozemac** (Apache-2.0, Matrix's Rust implementation), built to WASM for the browser.
The server stores and pushes only ciphertext.

- **What the server stores.** `ChatEvent.contentJson` holds the Megolm payload. Each device's keys and its one-time keys
  are in new entities, `ChatDevice` and `ChatDeviceKey`, and the server relays the Olm to-device messages that share
  room keys.
- **Key backup.** Room keys are backed up to the server, encrypted with a recovery key the user keeps. Without it, a
  new device can't read old messages.
- **Device verification.** Emoji or QR-code device verification, and cross-signing, so one verified device vouches for
  the others.
- **What an encrypted room gives up:**
  - server search;
  - agent members;
  - link previews made by the server;
  - Chatter on records;
  - "Create from" over the message text, unless the client sends the text with the action;
  - bridges, except to Matrix rooms once Moqui is the homeserver (C6). Element then sees the same Megolm sessions.

  The screens say so where a feature is missing, and retention deletes the ciphertext as usual.
- **Calls.** Calls in an encrypted room may use SFrame (W8), keyed from the room's session.

---

## 5. Voice

### 5.1 Approach

Every call is one `CallSession`, whatever its transport. A call has **legs**: a browser over WebRTC, a guest's browser,
a SIP desk phone or soft phone, or a phone number through a trunk. `moqui-rtc` holds the media of every leg:
- Browser legs use the SFU (§6).
- SIP legs are audio only. They are mixed for the SIP side, because a phone takes one stream: each SIP leg gets a mix of
  everyone but itself. They are forwarded as one more participant to the browser side.

So a phone call, a desk-phone call, a meeting and a meeting someone dials into are the same thing. Moving between them
(add a phone number to a meeting, turn a call into a meeting) needs no handover.

### 5.2 SIP and PSTN

- **Trunks (V1).** `moqui-sip` registers to, or is IP-authenticated by, a SIP trunk provider (for example Telnyx,
  Twilio Elastic SIP, Bandwidth, VoIP.ms, Flowroute, or any SIP provider). Outbound calls go to the trunk. Inbound DIDs
  route by `PhoneNumber` (§5.4). The trunk side carries:
  - transport: UDP, TCP or TLS on 5060 and 5061;
  - auth: digest;
  - media: SRTP (SDES) on TLS;
  - DTMF: RFC 4733;
  - codecs: G.711 μ-law and A-law, with Opus where the trunk offers it.

  Browser Opus is transcoded to G.711 in `moqui-rtc`. STIR/SHAKEN attestation is the provider's.
- **Registrar for phones (V4).** Desk phones (Yealink, Polycom, Grandstream) and soft phones (Linphone, Zoiper, Groundwire)
  register to Moqui with SIP credentials made like app passwords (shown once). Provisioning files for the common desk
  phones come later.
- **Browser phone.** The browser never speaks SIP. It talks WebRTC to Moqui, and Moqui places the SIP leg. That keeps
  one media path, one recording path and one set of credentials, and needs no SIP over WebSocket (RFC 7118) or JsSIP.
- **Emergency calls (D-V2).** In the US, a multi-line telephone system must dial 911 directly and send a dispatchable
  location (Kari's Law, RAY BAUM'S Act). Other countries have their own rules.
  - The proposal: emergency numbers go only to a trunk marked emergency-capable, with each user's or phone's registered
    address.
  - Without one, emergency dialing is refused with a clear message, and the Phone settings say so.
  - The rules need checking before V1 ships.

### 5.3 Features, from Zimbra and the usual phone systems

Zimbra's voice app (`../3p_other/Zimbra`, GPL-2.0, design reference only) was a front end to Cisco and Mitel
unified-communications servers:
- the web client is `zm-web-client/WebRoot/js/zimbraMail/voicemail/` (`ZmVoiceApp.js`; the `model/`, `controller/` and
  `view/` directories);
- the SOAP API is `zm-mailbox/soap/src/java/com/zimbra/soap/voice/` (`GetVoiceInfo`, `SearchVoice`, `VoiceMsgAction`,
  `GetVoiceFeatures`, `ModifyVoiceFeatures`, `GetVoiceMailPrefs`, `ModifyVoiceMailPrefs`, `ModifyVoiceMailPin`,
  `ModifyFromNum`, `UploadVoiceMail`);
- the provider setup is `zm-mailbox/store/docs/INSTALL-VOICE.txt`.

Here the server is Moqui itself. What Zimbra showed, and what it maps to:

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

- `CallSession` {callSessionId, chatRoomId, kindEnumId (direct, group, meeting, phone), directionEnumId, startedDate,
  answeredDate, endedDate, endReasonEnumId, mediaNodeId, recordingConsent}.
- `CallLeg` {callSessionId, callLegId, legTypeEnumId (web, guest, sipDevice, pstn), userId, guestName, phoneNumber,
  sipDeviceId, joinedDate, leftDate, statusId, qualityJson}.
- `CallLink` {callLinkId, token, name, ownerUserId, chatRoomId, policyEnumId (members, link, lobby), expiryDate,
  revokedDate}.
- `CallRecording` {callSessionId, contentLocation (DbResource), kind (per track, mixed), durationMs, transcriptText}.
- `VoiceTrunk` {voiceTrunkId, name, host, transport, authJson encrypted, registerFlag, emergencyCapable, statusId}.
- `PhoneNumber` {phoneNumber, voiceTrunkId, routeTypeEnumId (user, ringGroup, ivr, queue), routeId, smsEnabled,
  emergencyAddressId}.
- `SipDevice` {sipDeviceId, userId, username, passwordHash, userAgent, lastRegisterDate, contactUri}.
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
`end#Call`, in Mantle (design §14). The framework doesn't call Mantle; Mantle subscribes.

### 5.5 Screens (framework comms)

| Screen | Path | Purpose |
| --- | --- | --- |
| Calls | `Comms/Calls` | Tabs: All, Missed, Voicemail, Call links, Meetings (the meetings hook). Call back, play, the actions |
| Dialer | `Comms/Calls/Dialer`, also a tray panel | Keypad, search for a person or number, caller ID picker, call |
| Call | `Comms/Calls/Call`; guests at `/call/<token>` (D-A5) | Lobby and call view (§6.5). For a phone call: audio only, keypad for DTMF, hold, transfer, add, merge, record |
| LiveCall | the `q-page-sticky` slot | Mini window while in a call, across navigation |
| Incoming call | shell | Ring bar in every tab of the user (on `chatUser:<id>`), with Answer, Decline, Message. Web push for a closed tab (P0) |
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

### 5.7 Libraries

| Need | Candidate | License | Notes |
| --- | --- | --- | --- |
| SIP stack | `rsipstack` (transaction, dialog and transport layers on `rsip`) | check | the most complete of the Rust stacks found; Tokio |
| SIP stack, sans-IO | `rsiprtp` (audio UA, its own parser) | check | sans-IO like `rtc`, but no registrar or proxy roles |
| SIP, wider | `rvoip` (SIP core, transport, transactions, call engine, RTP) | check | younger and broad |
| SIP parser | `rsip` | MIT | under rsipstack |
| Opus | `opus` / `audiopus_sys` over libopus | crate MIT or Apache; libopus BSD-3-Clause | a C build (cc) |
| G.711, G.722 | in house | — | small |
| Resampling | `rubato` | MIT | 8 and 16 kHz to 48 kHz |
| Text to speech for IVR prompts | `moqui-llm` binding (a new `speech` feature) or recorded prompts | — | recorded prompts first |

The SIP stack is chosen at V1 by a spike: the stack must pass a trunk registration, a call each way, DTMF and
re-INVITE hold against a real provider and against an in-process fake trunk.

---

## 6. Web conferencing

### 6.1 Approach

The SFU is in from the first call: every WebRTC call, a two-person one too, goes through `moqui-rtc`. There is no mesh
mode. This gives one media path for recording, captions, SIP legs, quality data and guests, and it hides each
participant's IP address from the others. The cost is server bandwidth for one-to-one calls, which is small for audio and
modest for video. The design's "mesh for small calls, an SFU later" (§19) is replaced.

### 6.2 Library: webrtc-rs's sans-IO stack

- **`rtc`** (first release 0.3.0, January 2026; feature parity with the async `webrtc` crate by 0.8.0, January 2026): a
  pure Rust WebRTC stack with no I/O of its own.
- **`sfu`** (v0.20.0-rc.3, July 2026): an SFU on the same Protocol trait. `Sfu::poll_timeout()` is the minimum of every
  room's deadline.
- **`webrtc`** 0.20 is the async rewrite on `rtc`. 0.17 is the mature Tokio line.

Why sans-IO fits here:
- `moqui-host` owns sockets and Tokio tasks.
- No business logic runs on Tokio (the execution model in [Messaging](proposals/Messaging.md#existing-execution-model)).
- A sans-IO stack lets one task per media port drive all of a node's rooms, with timers from `poll_timeout`.
- Tests run two peers in memory with no sockets, which keeps them fast.

The alternative is **str0m** (MIT or Apache-2.0), an older sans-IO WebRTC library used in production SFUs. A spike at
D1 builds a two-party and a five-party call on `sfu` and `rtc`, with str0m as the fallback if `sfu` is still a release
candidate or misses a need (simulcast layer switching, TWCC). The versions are checked against the two-week rule and
`deny.toml` then. Decision D-W1.

### 6.3 Signaling

- **Frames.** A new client frame kind on `/notws`, `rtc`, handled in `crates/moqui-host/src/notws/rtc.rs` beside
  `collab.rs`. The frames:
  - `join`, `leave`, `offer`, `answer`, `candidate`;
  - `publish` and `unpublish` a track (camera, microphone, screen);
  - `subscribe` with the layer wanted (by tile size);
  - `mute`, `hand`, `reaction`.

  The server sends:
  - `joined`, `answer` and `offer` (renegotiation);
  - `participant` updates;
  - `activeSpeaker`;
  - `quality`;
  - `ended`.
- **Why the socket and not HTTP.** ICE candidates arrive in bursts, and a service call per candidate, with a
  NotificationMessage after its commit (`push#Signal`), adds latency and database work for nothing stored. Frames go
  straight to the node holding the call's media.
- **Writes stay services.** `start#Call`, `join#Call`, `leave#Call`, `admit#Guest` and `end#Call` are services that
  write `CallSession` and `CallLeg`. The socket only carries the media negotiation for a leg the service admitted.
- **Ringing.** It rides `chatUser:<id>` as today's room events do.
- **Cluster.** A call's media lives on one node (`CallSession.mediaNodeId`). A frame from a page on another node is
  forwarded over the cluster on a new topic `rtc-signal`, as `collab-update` forwards co-editing, and the browser's media
  goes straight to the media node's public address. Cascading SFUs across nodes is later (W7).

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
  - per-track Opus and VP8 into WebM with no transcoding, as `CallRecording` per track;
  - a mixed recording later, through the PDF engine's headless Chromium playing the call view (as Nextcloud Talk's
    recorder does) or an external ffmpeg process (LGPL or GPL, a separate process);
  - consent is shown to every participant before recording starts.
- **End-to-end encryption** (W8, D-W3): SFrame over insertable streams in browsers that have them. It turns off server
  recording, captions and SIP legs for that call.

### 6.5 Network

- **One UDP port** for all media (ICE and DTLS-SRTP multiplexed; the conf's `rtc-server media-port`, default 3478 or
  10000), with `public-address` for a node behind 1:1 NAT.
- **ICE-lite on the server.** The server has a public address, so it does not need full ICE.
- **TURN built in** (W4): UDP 3478, and TLS on 443 through the host's TLS listener for networks that block UDP.
  Credentials are short-lived HMAC credentials (the TURN REST convention) given at join. Coturn stays documented as an
  external alternative.
- **Guests** use the same TURN credentials, scoped to their call link and leg.

### 6.6 Screens

The design's Call screen (mr-boss design-drawers-commons §14) moves to the framework unchanged in shape:
- a lobby with camera preview, device pickers, level meter and the guest name;
- grid and speaker views;
- the bottom bar: mute, camera, share, hand, reactions, participants, chat (the room's timeline), settings, record and
  leave;
- the admit prompt;
- the LiveCall mini window.

`m-call` and its parts are framework components, so any app can embed a call. A Customer Service video session or a
Projects stand-up embeds the room's call by `chatRoomId`.

---

## 7. Order

Each step ends tested, documented and usable. Steps on one line can run in parallel.

| Step | What | Depends on |
| --- | --- | --- |
| **P0** | PWA install (manifest, service worker) and web push for chat, mentions and notifications. Ringing uses it from W1 | — |
| **A0** | The extension model: hook services, `<comms-action>`, `m-ext-panel`, `shell-providers` in `merge_extend`; the test hook | — |
| **A1** | The comms component, mount, Settings screens and `MailSettingsServices` moved; BOSS mounts them | A0 |
| **A2** | Chat and ChatPanel moved; BOSS's record features through hooks and actions | A1 |
| **A3** | Inbox, MessageDetail, Compose split; BOSS's Internal account, mailboxes and records through hooks; Home (D-A4) | A2 |
| **A4** | BOSS screen-extends complete, specs split, the launcher hide, `docs/tools/Comms.md` | A3 |
| **W1** | `moqui-rtc` spike and core: `/notws` `rtc` frames, `CallSession` and `CallLeg`, the SFU in one node; a 1:1 and a group call; Call, LiveCall, ring bar | A2 |
| **W2** | Simulcast, active speaker, screen share, the Calls screen, call history | W1 |
| **W3** | Call links, guests and the lobby (route per D-A5), meetings through the hook | W2 |
| **W4** | Built-in TURN | W1 |
| **V1** | `moqui-sip` spike and trunks: outbound and inbound PSTN with a browser leg, the mixer, caller ID, the emergency rule | W1 |
| **V2** | Voicemail: boxes, greetings, PIN over DTMF, the Voicemail tab and detail, email notices, transcription | V1 |
| **V3** | Call handling rules, ring groups, DND and presence, the dialer | V2 |
| **C1** · **C4 (SMS)** | The bridge layer; SMS through the provider (replaces the stub) | A2 · V1 |
| **C2** | Matrix appservice | C1 |
| **C4** | WhatsApp Cloud API and Telegram | C1 |
| **C3** | XMPP server | C1 |
| **V4** | SIP registrar for desk and soft phones | V1 |
| **V5** | IVR, queues, transfer and merge | V3 |
| **W5** · **W6** | Recording · captions | W2 |
| **C6** | The Matrix homeserver in Moqui, replacing the external one | C2 |
| **C7** | End-to-end encrypted rooms (§4.5) | C1 |
| **C5** · **W7** · **W8** | Slack and Teams · cascading SFU across nodes · encrypted calls (SFrame) | — · W2 · C7 |

**Tests.**
- **Rust.** The sans-IO peers run in memory, so call setup, a re-INVITE, simulcast switching and a SIP call against the
  in-process fake trunk each run well under a second. XMPP and Matrix are tested against recorded stanzas and events and
  an in-process peer.
- **Browser.** Chromium with fake media (`--use-fake-device-for-media-stream`) proves one call end to end. If it runs
  over about 15 s it is a slow test, marked and run only with permission (AGENTS.md).
- **Live checks.** Against a real Telnyx trunk and a real homeserver, done by hand and recorded in a verification log.

---

## 8. Decisions

Walked through with the user on 2026-10-30. Each answer is written into the sections it names.

| # | Decision | Decided |
| --- | --- | --- |
| D-A1 | The `.qvue` extension model (§3.3) | The three layers: hook services, declared actions, panel slots |
| D-A2 | What the framework Inbox holds without Mantle | Email accounts only. Internal messages stay Mantle's and come through the inbox source hook |
| D-A3 | The services package | `org.moqui.comms.*` for the screen-facing services. The record services stay `org.moqui.impl.*` |
| D-A4 | Comms Home | A framework Home whose cards come from providers (unread by account, mentions, missed calls, voicemail). BOSS adds meetings and mailboxes |
| D-A5 | Where the guest call page (`/call/<token>`) goes, given the Oct 28 rule that public pages go to mr-pass | The framework's public shell, since the framework owns calls. mr-pass can link to it |
| D-A6 | Where the voice and bridge admin screens live | Under the tools component's `System` app (§5.5) |
| D-C1 | Which federation first | The Matrix appservice (C2), then the XMPP server (C3) |
| D-C2 | A native Matrix homeserver in Moqui | Planned as a later step, C6, after the appservice. It replaces the external homeserver (§4.3) |
| D-C3 | XMPP library license | A `deny.toml` MPL-2.0 exception for `xmpp-parsers`, `minidom` and `tokio-xmpp` used unmodified, with a License-Review row |
| D-C4 | Personal WhatsApp, Signal and others through the mautrix bridges | A documented deployment option: separate AGPL processes, and the networks' terms are the operator's. The official Business API is the supported path |
| D-C5 | End-to-end encryption in chat | Moqui's own rooms may be end-to-end encrypted, with Matrix Olm and Megolm on `vodozemac` (C7, §4.5). XMPP OMEMO between XMPP clients is passed through, built with C3 (§4.3) |
| D-V1 | The SIP stack | Chosen by the V1 spike, `rsipstack` first |
| D-V2 | Emergency calling | Only through a trunk marked emergency-capable, with registered addresses. Otherwise refused with a notice |
| D-V3 | Video to SIP endpoints | No. SIP legs are audio |
| D-V4 | The first trunk and SMS provider | Telnyx, for the docs and the V1 live checks. Any SIP trunk works for calls |
| D-V5 | Recording consent | Off by default per deployment. When on, every leg is told and shown (a voice prompt for phone legs), assuming two-party consent |
| D-V6 | PWA and web push | Built first, as step P0, before A0 (§7) |
| D-W1 | SFU library | webrtc-rs `sfu` on `rtc`, confirmed by the W1 spike, with str0m the fallback |
| D-W2 | The SFU for every call, no mesh | Yes |
| D-W3 | End-to-end encryption for calls | Later (W8), per call or per room, matching the encrypted chat rooms. It turns off recording, captions and SIP legs for that call |
| D-W4 | Embedded TURN or coturn | Embedded, with coturn documented |

## 9. Risks

- **Young Rust media and SIP crates.** `sfu` is a release candidate, and the SIP stacks are young. The spikes at W1 and V1
  decide before anything depends on them, and the seams (`moqui-rtc`, `moqui-sip`) keep a swap local.
- **Real networks.** NAT, UDP-blocking firewalls and carrier quirks don't show in tests. TURN over 443 and the live checks
  cover the common cases.
- **Regulation.** Emergency calling, recording consent, SMS registration (10DLC in the US) and WhatsApp's template rules
  are the operator's to meet, and the screens must make them clear.
- **The size of the protocol surface.** XMPP and Matrix each have long tails. The plan targets what common clients need
  and stops there.
- **Running a homeserver beside Moqui** for Matrix phase 1 adds a process to deploy. Run-Deploy gets the steps.
