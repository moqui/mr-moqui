# MR Moqui — UI review: BasicComms

**Status — 2026-11-16:** review in progress. COM-B01–B07 cover mail setup/triage/compose/reply, identities/rules/automatic replies,
shared mailbox permissions, phone-width chat, ordinary chat participants, membership/actions/files, encrypted
exchange, key recovery and notification settings. Twenty-two open findings: thirteen observed UI/behavior issues,
one observed HTTP pagination defect and eight source-grounded proposal revisions. The merged [Comms Messaging plan](Comms-Messaging.md) informs the review; implementation remains
for a later combined pass. All activities remain incomplete; no application fixes. B03 ran only the existing fixture
spec; B04–B07 reused existing users without tests/builds; B06 mail setup used the existing administrator fixture,
and B07 shared-mailbox use included ordinary read/send members. No automated walk or acceptance suite was run.

Contents: [scope](#1-purpose-scope-and-sources) · [method](#2-review-method-and-refinement-criteria) ·
[inventory](#3-screen-and-surface-inventory) · [activities](#4-end-to-end-activities) ·
[verification](#5-existing-checks-and-future-verification) · [record](#6-coverage-findings-and-running-record) ·
[decisions](#7-decisions).

## 1. Purpose, scope and sources

Review communication as a continuous user task: establish an account/device, find the conversation, compose or
answer, understand delivery, and recover a failed or interrupted exchange. Include the independent framework Comms
app and its tray/public helpers without BOSS extensions.

Sources: [BasicComms README](../runtime/components/base/basic-comms/README.md),
[Comms guide](../docs/tools/Comms.md), [WebRTC](../docs/interfaces/WebRTC.md), [SIP](../docs/interfaces/SIP.md),
[Voicemail](../docs/interfaces/Voicemail.md), [Call recording](../docs/interfaces/Call-Recording.md),
[Chat keys](../docs/interfaces/Chat-Keys.md), [Desktop client](../docs/ui/Desktop-Client.md),
[Comms build record](archive/Comms-Framework.md), [Comms Messaging proposal](Comms-Messaging.md) (PR #202),
current BasicComms screens/QVue/twins/templates, setup data and
`org.moqui.comms` services. Use the [old review](archive/UI-Refinement.md) as a gap/history reference.

### Ownership and handoffs

| Surface | Owner / handoff |
| --- | --- |
| BasicComms app, mail/chat/call settings, tray Chat/Dialer content and CallGuest | This plan |
| Voice and ChatBridges administrator screens in this component | [System AI/integrations](ui-review-system-ai-integrations.md); only consumer-side operation here |
| Shell tray/floats/focus, common authentication/reauth/notification controls | [Shared/Gallery](ui-review-shared-gallery.md); inspect app content/context here |
| Relay/protocol readiness and mail outcome administration | System AI/integrations; users follow the owning account/message, not raw admin state |
| BOSS providers for record links, Files and create-from actions | Excluded; inspect honest framework-only options, using existing framework file-provider fixtures where supported |

`BasicComms` defaults to Inbox, while Home is separately routable. The addon launcher override and `/qapps/Comms`
are not assumed. `/call/<token>` is the configured public guest route and belongs to COM-13.

## 2. Review method and refinement criteria

Apply the [common review protocol](ui-review-shared-gallery.md#common-review-protocol) and
[verification policy](ui-review-shared-gallery.md#verification-policy-for-later-execution). The details below add this
area's fixtures, dependencies and judgment criteria.

### Environment, identities and records

- Prepare `review.comms.owner`, `review.comms.member`, `review.comms.other` in BASIC_COMMS_USERS, a no-Comms user,
  and an administrator only for setup. BASIC_COMMS_ADMIN and ADMIN are not substitutes for ordinary participants.
  Seed group/grant source is `basic-comms/data/BasicCommsSetupData.xml`; record per-account, room and call rights too.
- Mail fixtures: local hosted and synchronized account where available, shared member with restricted send/manage
  rights, multiple folders/labels, unread/threaded messages, drafts, attachments, mixed/failed delivery, identity,
  rule and out-of-office window. A hosted account may require `EMAIL_ACCOUNT_HOSTED`; test its holder and non-holder.
  Use local mail capture verified at fixture startup; OAuth/IMAP/SMTP success needs a corresponding local fixture.
- Chat/call fixtures: two browser identities plus optional third participant/guest; public/private/DM rooms,
  thread/poll/file, encrypted room/device state, local SIP/media endpoints, voicemail/queue/recording and call links.
  Verify rtc/media/crypto assets and browser capabilities first. Separate hostnames prevent session-cookie overlap.
- Never call a real phone/emergency number, send real mail or connect a production chat bridge. Restore memberships,
  DND/presets/rules, caller policy and account state; revoke guest links/app passwords and remove fixture devices.
  Desktop-only outbox/sync conflict states require a suitable local host and remain conditional if unavailable.

### Area-specific criteria

- Account, sender identity, recipients, room/thread and call participants stay visible at confirmation and outcome.
  Draft, queued, sent, delivered, failed and unknown are different states; optimistic content is not delivery proof.
- Cancellation, retry, navigation and delayed completions preserve the correct draft/attachment/selection. Selected
  recipients/labels show names, and a search/reset does not retain a hidden account/folder filter.
- Device verification, encryption, recording consent and guest admission are understandable without exposing keys
  or content to a non-participant. A readable encrypted-room shell is not evidence of decrypting its timeline.
- Desktop/mobile/narrow and TUI limits are explicit at the control. Read-only rich content must not be blanked by
  saving unrelated fields in a less capable mode. Media success needs observable local participant/sink evidence.

### Ordered waves

| Wave | Activities | Continuation |
| --- | --- | --- |
| 1. Mail | COM-01–06 | Account → triage → compose/delivery → policy/shared account lifecycle |
| 2. Chat | COM-07–10 | Membership → content/bridge → encrypted devices → preferences |
| 3. Calls | COM-11–16 | Direct call/dialer/guest → routing/devices → voicemail/queue/recording |
| 4. Recovery and reconciliation | COM-17–18 | Conditional host sync, remaining access/modes and handoffs |

Set up accounts before consuming messages; retain drafts for recovery checks before sending/discarding. Run device
revocation, account removal and room leave after their dependent content/media activities. Recording is tested on a
recordable call separate from the encrypted call that must refuse it.

### Incorporating the Comms Messaging proposal

PR #202 (`5a0dee2f9`) added `Comms-Messaging.md`; it did not update the archived Comms framework build record.
Per the user's direction, record proposed changes alongside review findings, labelled **observed UI** versus
**proposal revision / design judgment**. “Ready to build” in the proposal is not proof of current absence or approval
of every implementation decision. Review current behavior first, reconcile the proposal, and work on refinements
and new features together after more review. Keep the archived build record unchanged.

| Proposal concepts | Current review activity / boundary |
| --- | --- |
| Phone list/picker/bubbles, touch actions, desktop room search and threads (§2–4) | COM-07/18; exercise existing behavior at 390, around the current 640/1024 boundaries, desktop and compact tray. Source already contains some proposed features (COM-UI-003). |
| Media/files/voice/links/reactions detail tabs (§5) | COM-08; inventory actual tabs separately from proposed services. Stable activity/reaction paging requires a design change (COM-UI-005). |
| Network/privacy labels and device trust (§6) | COM-09/11/18; distinguish room membership/privacy from encryption, plain versus E2EE, bridge-readable content and required-encryption refusal (COM-UI-001/004). |
| Chat notifications, read synchronization, DND and host recovery (§7) | COM-10/17; existing browser notifications versus proposed durable/native background delivery. Screen-off/reboot/FCM/Keystore outcomes need native devices and separate evidence. |
| Calls, phone-number rooms, voicemail, device/audio behavior (§8–9) | COM-11–16; current local fixture flows first. Android Telecom, keyboard/back gesture/haptics, audio routing and impaired-network criteria are not established by a narrow browser. |
| SIM SMS/dialer/contacts, new adapters, RCS and history imports (§10–17) | Companion feature work; explicit prerequisites and design decisions before implementation. Do not count unimplemented proposals as broken current screens. No real number port, provider send, bridge pairing or external commitment authorized by this review. |

The proposal's external protocol, pricing and platform-restriction assertions were not reverified in this batch;
verify relevant authoritative sources when those implementation decisions are taken. Its suggested automated full
walks and network suites belong in acceptance and follow this repository's existing run policy, not automatic runs
whenever application or RTC code changes. COM-UI-006 hands shared shortcut ownership to Shared/Gallery.

## 3. Screen and surface inventory

This inventory accounts for **55 XML definitions** assigned to this file; screen containers and helper
definitions are included. Routes below are Material paths unless labelled otherwise; app routes have Standard
`/apps` counterparts. A file's physical presence does not prove access or a supported interaction in every mode.
Each row maps to §4; helper inspection does not replace its parent workflow. Counts describe definitions, not user
activities. Dialogs, templates and non-XML surfaces follow the route tables.

### basic-comms: `BasicComms`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/BasicComms/Calls` | [Calls.xml](../runtime/components/base/basic-comms/screen/BasicComms/Calls.xml) | Default: `History`. | COM-11, COM-12, COM-13, COM-16 |
| `/qapps/BasicComms/Chat` | [Chat.xml](../runtime/components/base/basic-comms/screen/BasicComms/Chat.xml) | Companions: [Chat.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Chat.tui.json), [Chat.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Chat.qvue). Authored text: `qvue`, `tui`. Declared modes: `vue,qvue,tui`. | COM-07, COM-08, COM-09 |
| `/qapps/BasicComms/ChatTui` | [ChatTui.xml](../runtime/components/base/basic-comms/screen/BasicComms/ChatTui.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `chatRoomId`, `limit`. | COM-07, COM-18 |
| `/qapps/BasicComms/Home` | [Home.xml](../runtime/components/base/basic-comms/screen/BasicComms/Home.xml) | Authored text: `qvt`, `tui`. | COM-01 |
| `/qapps/BasicComms/Inbox` | [Inbox.xml](../runtime/components/base/basic-comms/screen/BasicComms/Inbox.xml) | Default: `Mail`. | COM-01, COM-03 |
| `/qapps/BasicComms/Panel` | [Panel.xml](../runtime/components/base/basic-comms/screen/BasicComms/Panel.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | COM-01, COM-07, COM-12 |
| `/qapps/BasicComms/Settings` | [Settings.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings.xml) | Default: `Index`. | COM-01 |

### basic-comms: `BasicComms/Calls`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/BasicComms/Calls/Call` | [Call.xml](../runtime/components/base/basic-comms/screen/BasicComms/Calls/Call.xml) | Companions: [Call.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Calls/Call.qvue), [Call.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Calls/Call.tui.json). Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `callSessionId`. Declared modes: `vue,qvue,tui`. | COM-11, COM-12, COM-16 |
| `/qapps/BasicComms/Calls/CallTui` | [CallTui.xml](../runtime/components/base/basic-comms/screen/BasicComms/Calls/CallTui.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `callSessionId`. | COM-11, COM-18 |
| `/qapps/BasicComms/Calls/Dialer` | [Dialer.xml](../runtime/components/base/basic-comms/screen/BasicComms/Calls/Dialer.xml) | Companions: [Dialer.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Calls/Dialer.qvue), [Dialer.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Calls/Dialer.tui.json). Authored text: `qvue`, `tui`. Declared modes: `vue,qvue,tui`. | COM-12 |
| `/qapps/BasicComms/Calls/DialerTui` | [DialerTui.xml](../runtime/components/base/basic-comms/screen/BasicComms/Calls/DialerTui.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `text`. | COM-12, COM-18 |
| `/qapps/BasicComms/Calls/History` | [History.xml](../runtime/components/base/basic-comms/screen/BasicComms/Calls/History.xml) | Context: `outcome`, `pageIndex`, `folder`, `heard`. | COM-11, COM-12, COM-13, COM-15 |
| `/qapps/BasicComms/Calls/Queues` | [Queues.xml](../runtime/components/base/basic-comms/screen/BasicComms/Calls/Queues.xml) | Inspect the route and its visible controls through the activity. | COM-15 |
| `/qapps/BasicComms/Calls/Recordings` | [Recordings.xml](../runtime/components/base/basic-comms/screen/BasicComms/Calls/Recordings.xml) | Companions: [Recordings.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Calls/Recordings.qvue), [Recordings.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Calls/Recordings.tui.json). Authored text: `qvue`, `tui`. Context: `callSessionId`. Declared modes: `vue,qvue,tui`. | COM-16 |
| `/qapps/BasicComms/Calls/RecordingsTui` | [RecordingsTui.xml](../runtime/components/base/basic-comms/screen/BasicComms/Calls/RecordingsTui.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `callSessionId`. | COM-16, COM-18 |
| `/qapps/BasicComms/Calls/Voicemail` | [Voicemail.xml](../runtime/components/base/basic-comms/screen/BasicComms/Calls/Voicemail.xml) | Companions: [Voicemail.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Calls/Voicemail.qvue), [Voicemail.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Calls/Voicemail.tui.json). Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `voicemailId`. Declared modes: `vue,qvue,tui`. | COM-15 |
| `/qapps/BasicComms/Calls/VoicemailTui` | [VoicemailTui.xml](../runtime/components/base/basic-comms/screen/BasicComms/Calls/VoicemailTui.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `voicemailId`. | COM-15, COM-18 |

### basic-comms: `BasicComms/Inbox`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/BasicComms/Inbox/Compose` | [Compose.xml](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/Compose.xml) | Companions: [Compose.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/Compose.tui.json), [Compose.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/Compose.qvue). Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `emailMessageId`, `to`, `subject`, `recordEntity`, `recordId`. Declared modes: `vue,qvue,tui`. | COM-04 |
| `/qapps/BasicComms/Inbox/ComposeTui` | [ComposeTui.xml](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/ComposeTui.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `emailMessageId`, `replyToEmailMessageId`, `replyAll`, `replyToCommEventId`, `replyToSender`, `contactListId`, `to`, `cc`, `bcc`, `subject`, `body`, `recordEntity`, `recordId`. | COM-04, COM-18 |
| `/qapps/BasicComms/Inbox/Mail` | [Mail.xml](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/Mail.xml) | Companions: [Mail.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/Mail.tui.json), [Mail.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/Mail.qvue). Authored text: `html,qvt`, `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Declared modes: `vue,qvue,qvt,html,tui`. | COM-01, COM-03, COM-06 |
| `/qapps/BasicComms/Inbox/MailTui` | [MailTui.xml](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/MailTui.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `view`, `account`, `folder`, `label`, `mailbox`, `q`, `unreadOnly`, `cursor`, `thread`. | COM-03, COM-18 |
| `/qapps/BasicComms/Inbox/MessageDetail` | [MessageDetail.xml](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/MessageDetail.xml) | Authored text: `html,text`, `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `emailMessageId`, `communicationEventId`. | COM-03, COM-04 |

### basic-comms: `BasicComms/Panel`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/BasicComms/Panel/Chat` | [Chat.xml](../runtime/components/base/basic-comms/screen/BasicComms/Panel/Chat.xml) | Companions: [Chat.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Panel/Chat.tui.json). Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Declared modes: `vue,qvue,tui`. | COM-07 |
| `/qapps/BasicComms/Panel/Dialer` | [Dialer.xml](../runtime/components/base/basic-comms/screen/BasicComms/Panel/Dialer.xml) | Companions: [Dialer.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Panel/Dialer.tui.json). Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Declared modes: `vue,qvue,tui`. | COM-12 |

### basic-comms: `BasicComms/Settings`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/BasicComms/Settings/CallHandling` | [CallHandling.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/CallHandling.xml) | Companions: [CallHandling.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Settings/CallHandling.tui.json), [CallHandling.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Settings/CallHandling.qvue). Authored text: `qvue`, `tui`. Declared modes: `vue,qvue,tui`. | COM-14 |
| `/qapps/BasicComms/Settings/CallHandlingTui` | [CallHandlingTui.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/CallHandlingTui.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | COM-14, COM-18 |
| `/qapps/BasicComms/Settings/Calls` | [Calls.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Calls.xml) | Companions: [Calls.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Calls.tui.json), [Calls.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Calls.qvue). Authored text: `qvue`, `tui`. Declared modes: `vue,qvue,tui`. | COM-11, COM-14 |
| `/qapps/BasicComms/Settings/Chat` | [Chat.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Chat.xml) | Inspect the route and its visible controls through the activity. | COM-10 |
| `/qapps/BasicComms/Settings/ChatApps` | [ChatApps.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/ChatApps.xml) | Inspect the route and its visible controls through the activity. | COM-10 |
| `/qapps/BasicComms/Settings/Devices` | [Devices.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Devices.xml) | Companions: [Devices.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Devices.tui.json), [Devices.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Devices.qvue). Authored text: `qvue`, `tui`. Declared modes: `vue,qvue,tui`. | COM-09 |
| `/qapps/BasicComms/Settings/DevicesTui` | [DevicesTui.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/DevicesTui.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | COM-09, COM-18 |
| `/qapps/BasicComms/Settings/Identities` | [Identities.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Identities.xml) | Inspect the route and its visible controls through the activity. | COM-05 |
| `/qapps/BasicComms/Settings/Index` | [Index.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Index.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | COM-01 |
| `/qapps/BasicComms/Settings/MailAccounts` | [MailAccounts.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts.xml) | Default: `List`. | COM-02, COM-06 |
| `/qapps/BasicComms/Settings/OutOfOffice` | [OutOfOffice.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/OutOfOffice.xml) | Authored text: `qvt,html,html-print`, `tui`. Context: `emailAccountId`. | COM-05 |
| `/qapps/BasicComms/Settings/Phone` | [Phone.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Phone.xml) | Companions: [Phone.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Phone.qvue), [Phone.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Phone.tui.json). Authored text: `qvue`, `tui`. Declared modes: `vue,qvue,tui`. | COM-14 |
| `/qapps/BasicComms/Settings/PhoneDevices` | [PhoneDevices.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/PhoneDevices.xml) | Companions: [PhoneDevices.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Settings/PhoneDevices.tui.json), [PhoneDevices.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Settings/PhoneDevices.qvue). Authored text: `qvue`, `tui`. Declared modes: `vue,qvue,tui`. | COM-14 |
| `/qapps/BasicComms/Settings/PhoneDevicesTui` | [PhoneDevicesTui.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/PhoneDevicesTui.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | COM-14, COM-18 |
| `/qapps/BasicComms/Settings/PhoneTui` | [PhoneTui.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/PhoneTui.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | COM-14, COM-18 |
| `/qapps/BasicComms/Settings/Rules` | [Rules.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Rules.xml) | Companions: [Rules.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Rules.tui.json), [Rules.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Rules.qvue). Authored text: `qvue`, `tui`. Declared modes: `vue,qvue,tui`. | COM-05 |
| `/qapps/BasicComms/Settings/RulesTui` | [RulesTui.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/RulesTui.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | COM-05, COM-18 |
| `/qapps/BasicComms/Settings/TextBlocks` | [TextBlocks.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/TextBlocks.xml) | Context: `searchText`. | COM-05 |

### basic-comms: `BasicComms/Settings/MailAccounts`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/BasicComms/Settings/MailAccounts/Account` | [Account.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/Account.xml) | Authored text: `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `emailAccountId`, `setup`, `fix`, `runTest`. | COM-02, COM-06 |
| `/qapps/BasicComms/Settings/MailAccounts/Add` | [Add.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/Add.xml) | Default: `Address`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `emailAccountId`. | COM-02 |
| `/qapps/BasicComms/Settings/MailAccounts/List` | [List.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/List.xml) | Authored text: `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. | COM-02, COM-06 |
| `/qapps/BasicComms/Settings/MailAccounts/Removed` | [Removed.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/Removed.xml) | Companions: [Removed.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/Removed.tui.json), [Removed.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/Removed.qvue). Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Declared modes: `vue,qvue,tui`. | COM-06 |
| `/qapps/BasicComms/Settings/MailAccounts/SignIn` | [SignIn.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/SignIn.xml) | Companions: [SignIn.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/SignIn.tui.json), [SignIn.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/SignIn.qvue). Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Declared modes: `vue,qvue,tui`. | COM-02 |
| `/qapps/BasicComms/Settings/MailAccounts/SyncProgress` | [SyncProgress.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/SyncProgress.xml) | Companions: [SyncProgress.tui.json](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/SyncProgress.tui.json), [SyncProgress.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/SyncProgress.qvue). Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Declared modes: `vue,qvue,tui`. | COM-02, COM-06 |

### basic-comms: `BasicComms/Settings/MailAccounts/Add`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/BasicComms/Settings/MailAccounts/Add/Address` | [Address.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/Add/Address.xml) | Context: `emailAccountId`, `emailAddress`, `displayName`, `accountKindEnumId`, `delivery`, `fresh`. | COM-02 |
| `/qapps/BasicComms/Settings/MailAccounts/Add/Connect` | [Connect.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/Add/Connect.xml) | Authored text: `qvt`, `tui`. Context: `emailAccountId`, `emailAddress`, `displayName`, `accountKindEnumId`, `storeKind`, `storeSecurity`, `storeHost`, `storePort`, `smtpHost`, `smtpPort`, `smtpSecurity`, `mailUsername`, `smtpUsername`, `source`, `oauthProvider`, `delivery`. | COM-02 |
| `/qapps/BasicComms/Settings/MailAccounts/Add/Finish` | [Finish.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/Add/Finish.xml) | Context: `emailAccountId`. | COM-02 |
| `/qapps/BasicComms/Settings/MailAccounts/Add/Options` | [Options.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/Add/Options.xml) | Context: `emailAccountId`. | COM-02 |
| `/qapps/BasicComms/Settings/MailAccounts/Add/Test` | [Test.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/Add/Test.xml) | Authored text: `qvt`, `tui`. Context: `emailAccountId`. | COM-02 |

### basic-comms: `roots`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/BasicComms` | [BasicComms.xml](../runtime/components/base/basic-comms/screen/BasicComms.xml) | Default: `Inbox`. | COM-01, COM-18 |
| `/call/<token> (public)` | [CallGuest.xml](../runtime/components/base/basic-comms/screen/CallGuest.xml) | Companions: [CallGuest.qvue](../runtime/components/base/basic-comms/screen/CallGuest.qvue). Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Declared modes: `vue,qvue,tui`. | COM-13 |

### Embedded surfaces, dialogs, outputs and reconciliation

| Supporting surface | Activity / inspection |
| --- | --- |
| Inbox/Mail QVue: accounts/folders/labels/views, threads/search/saved searches, trust/block/unsubscribe/invite, record/file actions | COM-01/03; no BOSS-only records assumed |
| Compose QVue: recipient chips, identities/text blocks, rich body, attachment upload/Files picker, draft/schedule/cancel/send | COM-04; chunked upload and pending send are separate states |
| [UploadTransitions.xml](../runtime/components/base/basic-comms/template/UploadTransitions.xml) and [FilePickerTransitions.xml](../runtime/components/base/basic-comms/template/FilePickerTransitions.xml) | COM-04/08/18; upload status/chunks and provider folder/search/selection in the current account/room; interrupted and refused helper requests |
| MessageDetail body/attachments/resend/downloadEml and Mail/Compose TUI helpers | COM-03/04/18; actual bytes and truthful terminal content/edit limits |
| MailAccounts wizard, sign-in dialog/status, sync progress, Account server/options/folder/member forms, Removed list | COM-02/06; hosted delivery versus remote sync distinctions |
| Identities, Rules QVue/test/reorder/run and RulesTui, OutOfOffice and TextBlocks | COM-05; stored user policy and delayed job behavior |
| Chat QVue room/thread/message menus, mentions/reactions/pins/polls/search/forward/files, membership/bridge/encryption settings | COM-07–09; interactive states beyond a single XML route |
| [ChatPanel.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Panel/ChatPanel.qvue), served by Panel/Chat: unread/mention ordering, New message people picker, Mark all read, compact room, Back/Open in Chat | COM-07; inspect tray and quick-create mounts, reopen persistence and push refresh without losing the open room |
| Devices QVue/TUI, recent-auth dialog, key backup/settings, ChatApps/XMPP app password | COM-09/10; no key/secret evidence leaks |
| Chat preferences, DND hours/presets/default application and unread notifications | COM-10; date-sensitive job and per-room settings |
| Calls root signaling transitions and Call QVue/TUI, ring bar, encryption/device notices, hold/transfer/add/merge, captions and consent | COM-11/12/16; actual supported controls/capabilities only |
| [DialerTransitions.xml](../runtime/components/base/basic-comms/template/DialerTransitions.xml) shared search/data/dial | COM-12/18; full page and tray act on the intended identity/number with the same refusal boundary |
| Settings/Calls browser-local microphone/camera/speaker, Test/Cancel/Stop, ringtone and startup choices | COM-11; these settings belong to this browser, unlike server-side phone routing and SIP devices |
| Dialer and Panel/Dialer, Panel/Chat, call-history filters/links | COM-07/12; shared shell owns mounting/focus, this plan owns content/actions |
| Guest public page/lobby/admission, Phone/PhoneDevices, CallHandling rules/lists | COM-13/14; System owns trunks/numbers/fraud/emergency settings |
| Queues agent status, voicemail folders/detail/audio/transcription, recording player/delete | COM-15/16; authored terminal twins and download/playback boundaries |
| Desktop bundled Inbox/Compose/Chat/Call/Dialer offline copy, outbox and conflict UI | COM-17; conditional host branch, not another native installation project |

Prior chat TUI, device/encryption, successful provider/media and deep settings gaps remain explicit. BasicComms is
usable independently; missing BOSS providers are an expected boundary. Generic rendered success is never counted as
message delivery, key recovery or live audio/video.

## 4. End-to-end activities

Use visible controls and inspect intermediate states, confirmation and the resulting data/output. Unless narrowed
below, each activity has Material light/dark, actual supported TUI and a brief Standard/output branch. Select narrow
and keyboard checks where layout/interaction differs. Record each unattempted branch in §6; a service call or render
alone does not complete the activity. Names beginning `review.` are fixture proposals, not shipped accounts.

### COM-01 — Start from the right inbox and account

- **Actor/start:** Ordinary Comms user with two accounts, unread mail/chat/calls and no-data counterpart.
- **Objective/steps:** Open BasicComms default Inbox → switch account/folder/view → Home → follow unread/recent
  item → Settings index → return; open tray panel and return to original selection.
- **Expected:** Default Inbox, Home summaries and selected account agree; counts and destination scope are
  understandable; no-provider options are truthful.
- **Recovery:** No account, inaccessible recent item, unavailable source and filter-empty state leave a useful
  next action.
- **Screens/modes:** BasicComms/Home/Inbox/Mail, Settings/Index and tray entry; browser/TUI/Standard and narrow
  navigation.

### COM-02 — Set up and verify a mail account

- **Actor/start:** Account owner with hosted-address capability where needed; local mail/OAuth fixture, invalid
  credentials and ordinary non-holder.
- **Objective/steps:** MailAccounts/List → Add Address → Connect → Options → Test → Finish → Account →
  SyncProgress → Inbox; inspect SignIn/status and hosted versus remote options.
- **Expected:** Each step explains what is saved/tested; sender/store identity and account ownership remain
  clear; hosted delivery is not labelled remote synchronization.
- **Recovery:** Bad address/password/provider, failed test, canceled/previous step, delayed callback, duplicate
  finish and unavailable sync retain an appropriate non-secret draft.
- **Screens/modes:** All Add steps, Account, SignIn, SyncProgress and List; browser/TUI/Standard supported flow,
  narrow wizard; no real provider.

### COM-03 — Triage a thread and inspect its original evidence

- **Actor/start:** Mailbox member with threaded/unread messages, attachment and calendar invitation fixtures.
- **Objective/steps:** Mail → search/filter → open thread/message → mark/read/archive/move/label as offered →
  save search → reset → MessageDetail inspect/download EML/attachment → reply entry → return; exercise
  trust/block/unsubscribe/invite controls on local messages.
- **Expected:** Thread/message identity and folder/read state agree; reset clears intended scope;
  attachments/EML match selection and actions retain current account.
- **Recovery:** No matches, missing body/file, forbidden mailbox, refused action and stale selected thread
  retain understandable state and independent messages.
- **Screens/modes:** Mail QVue/MailTui, MessageDetail, folder/label/saved-search dialogs; actual output bytes
  and TUI/Standard content.

### COM-04 — Compose, recover and send a message once

- **Actor/start:** Sender and restricted shared-account member; local capture and recipients, text block,
  attachments and existing draft.
- **Objective/steps:** Compose → choose identity/recipients → write rich body → add upload/Files attachment →
  save draft → navigate/reopen → revise → send or schedule then cancel → inspect sent/delivery and captured
  content.
- **Expected:** Recipient chips, sender and attachment list match bytes delivered; saved snapshot versus later
  dirty edits is clear; queued/sent/failure status is accurate and retry preserves operation identity.
- **Recovery:** Invalid recipient, upload failure, stale draft, network interruption, canceled send/discard and
  lost send permission keep recoverable content without duplicate delivery.
- **Screens/modes:** Compose/ComposeTui, Mail/MessageDetail, text block/file picker and pending-send controls;
  browser keyboard/390, supported TUI/Standard and captured MIME.

### COM-05 — Make personal mail policy work for its consumer

- **Actor/start:** Account owner with two identities, reusable text, ordered rules and date-relative
  out-of-office window.
- **Objective/steps:** Settings/Identities set default → TextBlocks edit → Compose consume → Rules
  create/test/reorder/enable → apply to local incoming messages → OutOfOffice save → inspect captured reply →
  turn off.
- **Expected:** Default identity and text are predictable; rule order/test result and applied effect agree;
  auto-reply scope/window are clear and do not loop.
- **Recovery:** Invalid rule, no test matches, failed action, canceled delete, expired window and denied
  shared-account change retain prior settings and name errors.
- **Screens/modes:** Identities, TextBlocks, Rules/RulesTui, OutOfOffice and consumers; actual TUI/Standard
  limits, no blanking of unsupported rich fields.

### COM-06 — Manage a shared mailbox without losing ownership

- **Actor/start:** Owner, read member, send member and outsider; separate disposable mailbox with retained mail.
- **Objective/steps:** Account → options/folders/members → add/change member permission → member reads/sends
  valid fixture → pause/resume/sync/import local mail → leave/remove account → inspect Removed list and eligible
  cleanup.
- **Expected:** Ownership, membership and stored messages are distinct; leaving shared access is not deleting
  everyone’s mailbox; sync progress and removal explain retained versus deleted data.
- **Recovery:** Stale member, unauthorized send/manage, failed sync/import, canceled removal and removed account
  deep link leave safe evidence and no foreign content.
- **Screens/modes:** Account, List, SyncProgress, Removed and Mail; browser/TUI/Standard, removal after
  dependent mail activities.

### COM-07 — Carry a conversation across rooms and threads

- **Compact-panel branch:** open the tray's Chat tab and the quick-create Chat message dialog; choose one or several
  people, cancel/reopen/start, inspect unread/mention ordering, enter a compact room/thread, and use Back/Open in Chat.
  A push refresh or tray reopen must retain the intended room; a quick-create dialog must start in its declared new
  message context. Inspect keyboard focus and the 400px room's overlay panes, not only the full Chat page.
- **Actor/start:** Two ordinary participants and outsider; DM/private/public room plus thread, poll and message
  fixtures.
- **Objective/steps:** Chat → create/find/join room → send/reply/thread → react/pin/poll → edit/redact own
  message → search → forward to permitted room → mark read → open Panel/Chat and return.
- **Expected:** Room, author/thread and unread state remain clear; forwarded content names its origin; actions
  affect the selected message and membership controls what is readable.
- **Recovery:** No room, invalid message, failed send, revoked membership, canceled redact and delayed old-room
  response preserve draft/selection without leaking another room.
- **Screens/modes:** Chat QVue/ChatTui, message/room menus and Panel/Chat; keyboard entry to actions, narrow
  popup layout and actual terminal supported operations.

### COM-08 — Share files and maintain room membership or bridge context

- **Actor/start:** Room manager/member, read-only counterpart and local bridge/file-provider fixture.
- **Objective/steps:** Room settings → add/remove member/role → notification/bridge choice →
  upload/preview/download file → thread attachment → supported record/file action → inspect bridge result →
  return to room.
- **Expected:** File audience follows room rights; bridge mapping is distinguishable from membership;
  record-provider absence is explained without showing unusable BOSS actions.
- **Recovery:** Bad/large upload, missing file/provider, forbidden role, ended bridge and failed delivery
  preserve independent content and disclose no private file.
- **Screens/modes:** Chat membership/files/bridge dialogs and transitions; AI-18 owns System bridge recovery,
  actual bytes/TUI delegation.

### COM-09 — Recover encrypted chat and device trust

- **Actor/start:** Two encrypted-room users plus a new/revoked device and outsider; local key backup/recovery
  fixture.
- **Objective/steps:** Settings/Devices → inspect/rename → establish verification/key settings → open encrypted
  room → send/read → new device recovery → recent-auth revoke device → recheck room access; inspect
  verified-only/call-encryption choices.
- **Expected:** Device identity/trust and inability to decrypt are explicit; successful recovery actually reads
  intended messages; keys and recovery material stay out of evidence.
- **Recovery:** Wrong recovery material, missing keys, refused recent-auth, revoked device and unavailable
  crypto assets retain content securely and explain next steps.
- **Screens/modes:** Devices/DevicesTui, Chat encryption dialogs, ChatKeys/ReAuth support; browser crypto and
  truthful terminal/device-management limits.

### COM-10 — Set chat availability and external app access

- **Actor/start:** Ordinary user with room overrides, DND window/presets and local XMPP client fixture.
- **Objective/steps:** Settings/Chat → set defaults/DND/preset → apply eligible default → observe room
  notification → ChatApps create app password → local client sign-in → revoke → retry.
- **Expected:** Personal and room settings are distinct; DND dates and notification outcome agree; app password
  is shown once and revocation is observable.
- **Recovery:** Invalid window, unavailable job/client, canceled apply, wrong password and ended app access
  leave clear state without affecting unrelated sessions.
- **Screens/modes:** Chat, ChatApps and notification consumer; local fixture only, browser/TUI/Standard, no
  stored secret screenshots.

### COM-11 — Answer and manage a direct media call

- **Encrypted-call branch:** History → New call → End-to-end encrypted → select local participants → answer in
  their browsers → inspect encryption/unverified-device indicators → join with the selected third participant → leave
  and verify remaining media after key rotation → callback from History. Compare the verified-only device setting
  from COM-09 and an unsupported browser/missing-key failure. Calls must not downgrade to plain media; recording,
  captions, phones and anonymous guests are unavailable for this call type. Observe actual received media, not only
  the encrypted badge or rendered call controls.
- **Device-settings branch:** first open Settings/Calls, select microphone/camera/speaker and startup choices,
  start/cancel/stop the device test, then reload and compare a second browser. Choices persist only in their browser;
  denied permission, an unplugged device, leaving during the permission prompt and late media completion release
  the test stream and explain the failure. TUI/Standard must describe the browser-only device controls honestly.
- **Actor/start:** Two local browser participants with rtc/media permission and third participant for eligible
  actions.
- **Objective/steps:** History/New call → ring bar → answer → Call inspect participants/devices →
  mute/video/share/captions as offered → add/merge or leave → History inspect outcome.
- **Expected:** Actual local media and participant state agree; active, ringing, declined and ended states are
  distinguishable; leaving versus ending has clear effect.
- **Recovery:** Denied media, unavailable device, busy/declined/unanswered call, disconnect and failed join
  retain usable controls and truthful history.
- **Screens/modes:** Calls root/Call/CallTui, History, ring bar; browser media evidence and TUI
  control/delegation, fake devices only.

### COM-12 — Dial and transfer using the selected identity

- **Actor/start:** Permitted caller, denied caller and local SIP counterparts; valid test number and caller ID,
  active call.
- **Objective/steps:** Dialer or Panel/Dialer → choose identity/number → call → hold/resume → DTMF → choose
  transfer target → consult/complete or cancel → History callback/text entry.
- **Expected:** Number, caller identity and transfer target remain visible; cancel returns to original call;
  transfer/merge and separate call outcomes are distinct.
- **Recovery:** Invalid/forbidden number, policy/fraud refusal, failed transfer, unavailable SIP and stale
  active call cannot act on another call.
- **Screens/modes:** Dialer/DialerTui, Panel/Dialer, Call and History; AI-16 owns policy/configuration, local
  endpoints and supported modes.

### COM-13 — Invite and admit a bounded guest

- **Actor/start:** Call owner, anonymous guest and participant; local call link with expiry/revocation.
- **Objective/steps:** History create/open link → anonymous CallGuest enters name/waits → owner admit/deny →
  verify two-way media → remove/leave → revoke link → revisit.
- **Expected:** Guest state, host consent and participant identity are clear; public page reveals only its call;
  link creation is distinct from admission.
- **Recovery:** Expired/revoked token, denied lobby, duplicate join, missing media and disconnect recover
  without joining an unrelated call.
- **Screens/modes:** History link controls, /call/<token>, CallGuest transitions and Call participant UI;
  browser-only media with shared public-shell inspection.

### COM-14 — Configure a phone and call handling then observe routing

- **Actor/start:** User with phone rights, local device and caller; System-provisioned policy from AI-16/17.
- **Objective/steps:** Settings/Phone → notices/PIN/greeting → PhoneDevices add/rotate password → local
  registration → CallHandling rules/lists/order → incoming call → observe selected outcome → revoke device.
- **Expected:** Personal handling, administrative number routing and device credentials are distinct;
  greeting/PIN/device identity and actual call behavior agree.
- **Recovery:** Invalid PIN/rule, failed greeting upload, unknown caller, revoked device and canceled delete
  preserve working settings and safe credentials.
- **Screens/modes:** Phone/PhoneTui, PhoneDevices/twin, CallHandling/twin, Settings/Calls; local SIP only,
  TUI/Standard limitations explicit.

### COM-15 — Work a queue and resolve voicemail

- **Actor/start:** Local queue agent and caller; open/closed availability and voicemail with audio/transcription
  states.
- **Objective/steps:** Calls/Queues set availability → receive queue call → unanswered path to voicemail →
  History filter folders/heard → Voicemail play/mark/retry transcript → callback → delete/restore/trash.
- **Expected:** Queue availability affects routing; voicemail identity/audio/heard state agree; transcription
  pending/failed is distinct from no message; filter reset is complete.
- **Recovery:** No agents, failed media/transcript, missing audio, canceled delete, unauthorized mailbox and
  stale message keep other messages usable.
- **Screens/modes:** Queues, History, Voicemail/VoicemailTui and Phone settings; actual local audio/download,
  TUI supported management.

### COM-16 — Record with consent and inspect retained media

- **Actor/start:** Participants in recordable local call plus separate encrypted call; local recorder and
  disposable recording.
- **Objective/steps:** Call request/start recording → participants inspect consent → pause/resume/stop →
  Recordings open/play/download → inspect history → delete selected recording; attempt recording on encrypted
  call.
- **Expected:** Consent and active recording are explicit to participants; retained media belongs to selected
  session; unsupported encrypted recording is refused clearly.
- **Recovery:** Consent withdrawn, backend unavailable, interrupted recording, missing file and canceled delete
  preserve accurate state and access boundaries.
- **Screens/modes:** Call recording/caption controls, Recordings/RecordingsTui; actual media bytes, browser
  success and terminal truthful fallback.
- **Playback branch:** In Recordings, switch between periods created by pause/resume, seek and synchronize audio,
  camera and screen tracks; inspect a gap/missing segment and each offered segment download. Changing the selected
  recording must stop the previous media. Repeat read/delete as the recorder, call host and unrelated participant:
  playback rights and recorder-only deletion are distinct. Include a guest consent notice via COM-13 where enabled.

### COM-17 — Recover host-only queued work and conflicts

- **Actor/start:** Conditional local desktop host with bundled Comms/offline store; two devices editing a
  disposable draft/message.
- **Objective/steps:** Go offline → draft/send/change message → inspect outbox/pending/failed state → reconnect
  → observe delivery → create remote conflict → retain/copy/resolve draft → reopen online.
- **Expected:** Queued work is not called delivered; conflict recovery preserves the user's content and correct
  account; completed operations are not repeated on reconnect.
- **Recovery:** Lost connection during acknowledgment, revoked access, failed send, missing cached thread and
  canceled conflict resolution leave explicit recoverable state.
- **Screens/modes:** Desktop-bundled Inbox/Compose/Chat/Call/Dialer sync surfaces; if host unavailable mark
  blocked, not covered by web render or ShellSync fixture alone.

### COM-18 — Reconcile private content, helpers and mode limits

- **Actor/start:** Owner/member/outsider and no-Comms user with earlier valid message/room/call/file IDs.
- **Objective/steps:** Inspect all remaining inventory entries; pair valid owner actions/downloads with
  unauthorized direct helper requests; follow System account/bridge/voice issue → Comms consumer → return; check
  authored terminal twins.
- **Expected:** Private content stays outside unauthorized responses and bytes; each helper/dialog/mode has a
  recorded outcome; cross-app record identity survives.
- **Recovery:** Lost membership, missing provider, unsupported media/editor and removed account remain explicit
  partial/blocked branches.
- **Screens/modes:** All app/settings/helper/token routes; no addon mounts or duplicate full System/shared-shell
  walkthroughs.

## 5. Existing checks and future verification

This is an available-test map, not a results ledger. Read assertions and suite setup before selecting a run; static
shape checks, offline component fixtures, live browser walks and service rules provide different evidence.

| Component / suite | Existing specs | Coverage and limits |
| --- | --- | --- |
| basic-comms / server | [BasicComms](../runtime/components/base/basic-comms/test/BasicComms.spec.js), [BasicCommsInbox](../runtime/components/base/basic-comms/test/BasicCommsInbox.spec.js), [BasicCommsComposeRef](../runtime/components/base/basic-comms/test/BasicCommsComposeRef.spec.js), [BasicCommsFiles](../runtime/components/base/basic-comms/test/BasicCommsFiles.spec.js), [BasicCommsSync](../runtime/components/base/basic-comms/test/BasicCommsSync.spec.js), [BasicCommsTexts](../runtime/components/base/basic-comms/test/BasicCommsTexts.spec.js), [HostedMailAddress](../runtime/components/base/basic-comms/test/HostedMailAddress.spec.js) | Screen/service rules for mail, files, sync and texts/hosted addressing; no full account-provider walk. |
| basic-comms / server | [BasicCommsChat](../runtime/components/base/basic-comms/test/BasicCommsChat.spec.js), [BasicCommsChatChanges](../runtime/components/base/basic-comms/test/BasicCommsChatChanges.spec.js), [BasicCommsEncrypted](../runtime/components/base/basic-comms/test/BasicCommsEncrypted.spec.js), [ChatReach](../runtime/components/base/basic-comms/test/ChatReach.spec.js), [BasicCommsBridges](../runtime/components/base/basic-comms/test/BasicCommsBridges.spec.js) | Chat/change/encryption/reach/bridge rules; actual two-user interaction remains separate. |
| basic-comms / server | [BasicCommsCalls](../runtime/components/base/basic-comms/test/BasicCommsCalls.spec.js), [CallClientFixtures](../runtime/components/base/basic-comms/test/CallClientFixtures.spec.js), [CryptoClientFixtures](../runtime/components/base/basic-comms/test/CryptoClientFixtures.spec.js) | Call behavior and fixture preparation; setup does not establish media success. |
| basic-comms / client | [CryptoFlow](../runtime/components/base/basic-comms/test/client/CryptoFlow.spec.js), [ShellSync](../runtime/components/base/basic-comms/test/client/ShellSync.spec.js) | Existing crypto flow and offline shell synchronization states; inspect suite mode and feature prerequisites. |
| basic-comms / acceptance client | [CallFlow](../runtime/components/base/basic-comms/test/acceptance/client/CallFlow.spec.js), [CallGuestFlow](../runtime/components/base/basic-comms/test/acceptance/client/CallGuestFlow.spec.js), [EncryptedCallFlow](../runtime/components/base/basic-comms/test/acceptance/client/EncryptedCallFlow.spec.js), [EncryptedChatFlow](../runtime/components/base/basic-comms/test/acceptance/client/EncryptedChatFlow.spec.js) | Local multi-browser media/guest/encrypted journeys; do not infer coverage of every SIP/provider/settings branch. |
| framework / server | [ChatKeyBackup](../runtime/components/base/framework/test/ChatKeyBackup.spec.js), [SipDevice](../runtime/components/base/framework/test/SipDevice.spec.js), [Voicemail](../runtime/components/base/framework/test/Voicemail.spec.js), [Voice](../runtime/components/base/framework/test/Voice.spec.js), [RecentAuth](../runtime/components/base/framework/test/RecentAuth.spec.js) | Key backup, phone/device/voicemail/policy and recent-auth rules; UI recovery still pending. |

Future acceptance candidates: **BasicCommsMailWalk** (COM-01–06), **BasicCommsChatWalk** (COM-07/08/10),
**BasicCommsDeviceRecoveryWalk** (COM-09), **BasicCommsPhoneWalk** (COM-12/14/15),
**BasicCommsRecordingWalk** (COM-16) and **BasicCommsAccessWalk** (COM-18). Extend the existing call/guest/encrypted
walks for overlap; do not duplicate their full scenarios. COM-17 extends existing host/sync fixtures only when that
host is available. Phone width and complete TUI/access walks belong in acceptance; no regression screen sweep.

Follow the [verification policy](ui-review-shared-gallery.md#verification-policy-for-later-execution). Proposed
walk names above describe future acceptance work, not tests created or authorized to run by this document. Keep
focused bug/rule checks in regression and reuse existing sweeps/walks instead of adding duplicates.

## 6. Coverage, findings and running record

### Activity coverage ledger

Statuses: **pending**, **inspected**, **exercised** (pass/fail/partial named separately), **blocked**, **deferred**
(with reason). Existing tests and the archived review do not advance these statuses. Every branch is pending until
execution records it; “exercised” never implies that all modes and recovery paths passed.

| Activity | Status | Outcome / remaining branches | Evidence / findings |
| --- | --- | --- | --- |
| COM-01 — Start from the right inbox and account | exercised — partial | B01/B06 empty entry and account setup navigation; B06 populated hosted account, unified/account folders, unread counter, saved-search recall and Sent with administrator fixture. B07 ordinary B/C shared-account Inbox and read/send boundaries. Other accounts and alternate modes pending. | COM-B01/B06/B07; B06 04, 06, 13–16, 19; B07 14–20 |
| COM-02 — Set up and verify a mail account | exercised — partial | B06 ordinary B hosted-address refusal; existing administrator Address → Connect → Test (no remote sign-in) → Options → Finish → activated Account. Local SMTP sink attached separately as fixture setup. Narrow-holder positive blocked by grant approval review; external IMAP/POP/OAuth, transport setup/test, duplicate Finish/cancel/back and other modes pending. | COM-B01/B06; B06 01–04 |
| COM-03 — Triage a thread and inspect its original evidence | exercised — partial | B06 three imported messages grouped into two conversations; expansion/body/header/source and attachment bytes; no-match/advanced search, saved search/reset/recall, mark unread/filter/read, archive/Undo. EML/Print links return 404 through absent Comms mount. B07 read member can read/mark read, but active Archive refuses; owner bulk-unflags rule fixtures. Label/move/snooze, spam/trust/invites and remaining modes pending. | COM-B06/B07; COM-UI-016/019/021; B06 05–08, 13–14; B07 15–16, 24 |
| COM-04 — Compose, recover and send a message once | exercised — partial | B06 administrator reply, quote, pop-out/close, autosave and Drafts reopen; send/Undo/resend captured once; new-message missing/invalid-recipient recovery, schedule dialog cancel, To/Bcc capture and Sent; 390px composer inspection. Bcc-only refused. B07 alias/signature and ordinary shared sender captured; open draft refused after send-right downgrade, retained for owner after membership removal. Fast-entry commitment needs repeat; upload/Files, Reply all/forward, conflict/offline/provider failure, scheduled delivery and other modes pending. | COM-B01/B06/B07; COM-UI-017–019/021; B07 11, 17, 25 |
| COM-05 — Make personal mail policy work for its consumer | exercised — partial | B07 alias/default/reply-to/signature → captured compose; rule preview/no-match/cancel, automatic inbound flag and manual run; OOO required-body refusal, contacts/internal audiences, repeated/automated sender suppression, distinct internal body and turn-off. No-hook contacts audience replies to unknown outsider. TextBlocks, reply signature placement, invalid/multiple/ordered rules, blocked senders, dates/apply-all, provider failures and other modes pending. | COM-B07; COM-UI-020/022; B07 01–13, 22–23, 26–27 |
| COM-06 — Manage a shared mailbox without losing ownership | exercised — partial | B06 import/duplicate detection. B07 owner converts fixture to Shared, adds read and send members; read/send UI, downgrade refusal, leave cancel/confirm, former-member deep link and owner removal of C; retained draft and owner mailbox remain. Manager/last-owner, shared policy editing, pause/resume/provider sync, account removal/retention and other modes pending. | COM-B06/B07; COM-UI-021/022; B07 14–25 |
| COM-07 — Carry a conversation across rooms and threads | exercised — partial | B02/B03 persistence, DM picker, delivery/read clearing, replies/edit/pins/search/thread draft. B04 copy text/link, forward with comment, reaction add/remove/expanded picker, unread-from-message, edit cancel, delete confirm/cancel, poll create/two votes/close cancel+confirm. Membership in COM-08. Remaining branches include destructive delete submission, reaction-person disclosure, forward cancel/refusal, advanced polls, actual touch, native/host, dark and TUI. | COM-B02–B04; COM-UI-002/003/005–008 |
| COM-08 — Share files and maintain room membership or bridge context | exercised — partial | B04 ordinary private-group owner/member, promotion/demotion, remove/re-add, last-owner succession and role restoration; populated All/Pictures, image viewer/Show in chat, curl upload/download and revoked/restored byte access. Two attachments on one event expose a limit=1 cursor defect. UI upload/progress/cancel, large/failed files, other media, three-person role competition, public/record/bridge/encrypted rooms and other modes pending. | COM-B03/B04; COM-UI-009/010 |
| COM-09 — Recover encrypted chat and device trust | exercised — partial | B05 plain→encrypted presentation, two-user encrypted exchange, new-device missing-key state, rename, verification request/accept/cancel, stalled emoji comparison, backup recent-auth cancel/success, invalid-key refusal and successful second-device history recovery, verified-only exclusion, rotation/remove confirmations canceled. Actual revocation/start-over, verified completion/mismatch/QR, encrypted files/calls, outsider helpers and remaining modes pending. | COM-B02/B05; COM-UI-001/004/011–014 |
| COM-10 — Set chat availability and external app access | exercised — partial | B05 save/reload notification defaults, preview/sound/receipt flags; bulk apply cancel/confirm with two-room result; room mute/unmute; DND incomplete-input refusal, overnight interval save/clear with zone. Preferences restored. Actual push/audio, new-room inheritance, direct-message exclusion comparison, read-marker wire privacy, DND job timing/presets, ChatApps/XMPP and other modes pending. | COM-B05; COM-UI-015 |
| COM-11 — Answer and manage a direct media call | pending | All steps, modes and recovery branches pending. | — |
| COM-12 — Dial and transfer using the selected identity | pending | All steps, modes and recovery branches pending. | — |
| COM-13 — Invite and admit a bounded guest | pending | All steps, modes and recovery branches pending. | — |
| COM-14 — Configure a phone and call handling then observe routing | pending | All steps, modes and recovery branches pending. | — |
| COM-15 — Work a queue and resolve voicemail | pending | All steps, modes and recovery branches pending. | — |
| COM-16 — Record with consent and inspect retained media | pending | All steps, modes and recovery branches pending. | — |
| COM-17 — Recover host-only queued work and conflicts | pending | All steps, modes and recovery branches pending. | — |
| COM-18 — Reconcile private content, helpers and mode limits | exercised — partial | B03 nonmember private-room refusal/recovery and author actions. B04 removed member lost the open Files pane/room list entry; stale permalink, files list and text/image byte requests refused; re-add restored access. B07 shared mail downgrade/leave/removal boundaries exercised. No-Comms identity, wider helper/byte matrix, encrypted/record/bridge boundaries, remaining variants/output modes pending. | COM-B03/B04/B07; COM-UI-007/021 |

### Findings register

Use `COM-UI-001` onward. P1 blocks work or essential understanding; P2 causes substantial friction or misleading
presentation; P3 is a localized refinement. Label evidence as observed behavior, static suspicion or design judgment.

| ID / severity | Actor, activity step, route, mode / viewport | Reproduction and impact | Evidence / owning layer | State / correction / retest |
| --- | --- | --- | --- | --- |
| COM-UI-001 / P2 | **Observed UI / design judgment**; john.doe; COM-09/18; plain private room `/qapps/BasicComms/Chat?chatRoomId=100000`; Material light, 390×844 | Created with Encrypted off. Header and introductory area show closed padlocks, but no visible explanation that this room is server-readable. The icon represents private membership, yet resembles encryption assurance. Revise the §6.2 label design to distinguish membership privacy from E2EE and explain the plain private/DM state. No encryption failure is alleged. | B02 `02-phone-room.{jpg,txt}`, `06-room-return.jpg`; [Chat.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Chat.qvue) `ROOM_ICONS.CrtPrivate` (234), header's sealed-only Encrypted chip (789), room icon (1283). [Proposal §6](Comms-Messaging.md#6-e2ee-policy). | Open; priority provisional. Plain state observed only; encrypted/bridge/SMS states pending. |
| COM-UI-002 / P3 | **Observed UI**; john.doe; COM-07; same room and `thread=2`; Material light, 390×844 | Phone Send is a fixed circular 40px button, but full-page stacked Chat keeps both icon and Send text. Text/icon overflow the control; measured client width 40px versus scroll width 48px. Apply the proposal's compact composer consistently to stacked and compact modes while retaining an accessible Send name. | B02 `02-phone-room.jpg`, `05-send-geometry.json`, `06-room-return.jpg`; [Chat.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Chat.qvue) label conditional (580) tests compact only; [WebrootVue.qvt.css](../runtime/components/base/webroot/screen/webroot/css/WebrootVue.qvt.css) phone send sizing (722). | Open; no fix/retest. Sending worked; B03 `06-two-user-phone.jpg` also shows the cramped Send control. Native keyboard geometry and other widths pending. |
| COM-UI-003 / P2 | **Proposal revision / source reconciliation**; COM-07/18; Comms-Messaging §1–2, no live native-app claim | Rebase “card home/no bubbles/no long press” and rewrite tasks on a built/partial/missing inventory. Current code has bubble selection, a 450ms long-press handler/context menu, home FAB/maximized picker and direct mobile-home rendering. Review existing flows before component extraction; preserve full Chat, compact tray and host-home distinctions and 640/1024 boundaries. | B02 verifies own bubble and right-click sheet, not physical long press/native home. [Chat.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Chat.qvue) 160, 1300–1302, 1925–1949, 2613; [ChatPanel.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Panel/ChatPanel.qvue) 100–137; [Root.qvue](../crates/moqui-desktop/shell/screens/Root.qvue) 27, 154–156. | Open plan revision; not a newly observed missing-feature defect. B03 verifies two-user bubbles, full phone picker cancellation/start, keyboard reaction and existing Files All/Pictures. B04 adds existing forward/comment, expanded fixed reaction grid, polls, populated All/Pictures and image viewer/Show in chat. Searchable/category/recent emoji, physical touch, host home and proposed Links/Voice/Reactions remain separate work. Preserve destination/focus checks for the room and nested-pane Back controls, both currently named Back. |
| COM-UI-004 / P1 | **Proposal revision / policy conflict**; COM-11; Comms-Messaging §6.1.4 | Consent-to-fallback wording conflicts with current required-encryption refusal and permanently bound call encryption. Preserve refusal for required E2EE; any separately permitted plain call must be an explicit new action, not a downgrade. Resolve wording before implementation, including unsupported devices and existing plain calls. | [CallServices.xml](../runtime/components/base/framework/service/org/moqui/call/CallServices.xml) 48–66; [Calls.js](../runtime/components/base/framework/service/org/moqui/call/Calls.js) 719–722, 749–753. [Proposal §6](Comms-Messaging.md#6-e2ee-policy). | Open design decision; priority applies to the proposed change, not an observed runtime security defect. Current no-downgrade criterion retained; no media walk performed. |
| COM-UI-005 / P2 | **Proposal revision / data design**; COM-07/08; Comms-Messaging §5.2 | A global activity cursor cannot use a room-local event sequence as its chronological boundary. Reactions ordered by newest reaction likewise need more than the target-message sequence. Specify stable ordering, composite/opaque cursor and tie-breakers, including equal timestamps, multiple rooms, later reactions to old messages and concurrent updates; then define UI load-more/recovery expectations. | [ChatEntities.xml](../runtime/components/base/framework/entity/ChatEntities.xml) 151–167 defines per-room event sequence/composite key. Proposed RoomLinks, RoomReactions and ChatActivity services were not found in current implementation. [Proposal §5](Comms-Messaging.md#5-details-tabs-and-new-services). | Open proposal correction; no existing pagination failure observed. Focused cursor regressions later, full scrolling/access journeys in acceptance. |
| COM-UI-006 / P3 | **Proposal revision / shared ownership**; COM-07 and SH-12; Comms-Messaging §3 | Integrate room quick-switching with the existing shell palette, or specify an explicitly scoped shortcut. Do not add a competing global Ctrl/Cmd+K listener. Define editor exceptions, search context, keyboard selection and focus return. | [WebrootVue.qvt.js](../runtime/components/base/webroot/screen/webroot/js/WebrootVue.qvt.js) 10007–10010 owns Mod+K; [BasicComms.xml](../runtime/components/base/basic-comms/screen/BasicComms.xml) already registers Chat rooms palette source. [Shared/Gallery](ui-review-shared-gallery.md), [proposal §3](Comms-Messaging.md#3-desktop-gaps). | Open design handoff; no shortcut collision observed because proposed handler is not implemented. |
| COM-UI-007 / P2 | **Observed UI / source-confirmed inconsistency**; calls.client.b; COM-07/18; full Chat and Create → Chat message compact picker, room 100051; Material light, 1024×900 | The same conversation's latest message displays **12:41 AM** in full Chat and **6:41 PM** in the compact conversation list, without a zone explanation. Inconsistent chronology impedes recognizing the latest exchange. Reconcile formatting with the configured user locale/time zone and add that contract to proposal §2's shared components; do not assume the full-page zone itself is wrong. | B03 `18-compact-picker.{txt,jpg}`; [Chat.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Chat.qvue) 250–268 uses `moqui.l10n` zoned helpers; [ChatPanel.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Panel/ChatPanel.qvue) 155–162 uses browser-default Date formatting. [QvtConfig.js](../runtime/components/base/webroot/screen/includes/QvtConfig.js) 76 supplies the user zone. | Open; no fix/retest. Later verify an explicitly configured zone different from the browser, midnight/date boundaries and full/compact/host lists. |
| COM-UI-008 / P2 | **Proposal revision / status semantics**; COM-07; Comms-Messaging §4 Info; ordinary B/C live chat | Define the proposed Sent/Delivered/Read Info contract before building it. Existing member read positions support a read indicator, not proof of per-device delivery. Specify unavailable versus unknown delivery, receipt preferences/privacy, threads and external bridge acknowledgements; do not infer delivery solely from `lastReadEventSeq`. Preserve the distinction between a member's latest read position and a complete per-message recipient list. | B03 `05-c-reaction-read.txt`, `06-two-user-phone.jpg` show the current reader avatar; [Chat.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Chat.qvue) `readersBySeq` 1332–1346 maps each other member to the latest loaded message at/before their read position, honors `prefs.readReceipts`; `threadItems` 1349 omits reader indicators. Current action sheet has no Info action. Proposal §4 adds it. | Open design revision, not an observed delivery failure. Decide status/privacy semantics and evidence sources; later use focused rule regressions and multi-user/device acceptance scenarios. |
| COM-UI-009 / P2 | **Observed UI / design judgment**; calls.client.b owner and calls.client.c member; COM-08; room 100102; Material light, 1280×720 owner / 390×844 successor | Last owner B chose Leave. Confirmation explained lost messages and public-room rejoining but did not disclose ownership succession or identify C. After Leave, C automatically became owner; the event read “Calls Client B made Calls Client C an owner.” The continuity policy works, but the departing owner cannot assess the control transfer from its confirmation, and the event suggests a deliberate role change. Add last-owner consequences, intended successor and automatic-succession wording to proposal §5 People and membership flows; keep service authorization authoritative if membership changes while confirming. | B04 `16-owner-leave-warning.txt`, `17-successor-owner.{txt,jpg}`; [Chat.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Chat.qvue) `leave()` 2455–2462; [ChatServices.xml](../runtime/components/base/framework/service/org/moqui/chat/ChatServices.xml) `remove#Member` 1783–1835 selects the longest-standing moderator, else member. | Open; no fix/retest. Actual B→C succession observed, then B owner/C member restored. Multi-candidate selection and concurrent changes remain pending; not an unauthorized promotion finding. |
| COM-UI-010 / P2 | **Observed HTTP defect / UI-supporting verification**; calls.client.c, COM-08; `/apps/BasicComms/Chat/files`; room 100102/event 5, two attachments | `limit=1` returns only attachment 100000 and `nextBeforeEventSeq=5`; following that cursor returns no files. A `limit=30` request returns both 100000 and 100001. Thus a single message exceeding the page limit loses its remaining files from traversal. Source slices to the limit and cannot move a partial final message to the next page when that message is the entire page. Resolve the whole-message page contract or use an attachment-aware cursor before proposal §5 expands Media/Files/Voice. Also specify mixed image/video ordering and MIME exclusions; one prefix filter is not the proposed combined Media query. | B04 `22-file-pagination.json`; [ChatServices.xml](../runtime/components/base/framework/service/org/moqui/chat/ChatServices.xml) `find#RoomFiles` 3492–3537; [Chat.xml](../runtime/components/base/basic-comms/screen/BasicComms/Chat.xml) `files` 107–114; [Chat.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Chat.qvue) 2305 currently requests 30. | Open; no fix/retest. Actual HTTP traversal omission at supported limit=1, **not** a missing tile observed in the default 30-item UI. Later add a focused same-message page-boundary regression; full multi-page/mixed-media/revocation walks belong in acceptance. |
| COM-UI-011 / P3 | **Observed UI**; calls.client.b; COM-09; encrypted room 100153, Material light 1280×720 | Header offers Search in the room, opens a normal search field and accepts a query, then reports “An encrypted room is not searched on the server.” Creation already explains this limitation, but the consumer invites an unsupported operation without guidance until submission. Explain/disable this path in the encrypted pane, or explicitly label a separately implemented local search. Proposal §6.4–6.5 must preserve the server refusal and make the local-search decision visible. | B05 `04-encrypted-search-refusal.txt`; [Chat.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Chat.qvue) header/search pane; [ChatServices.xml](../runtime/components/base/framework/service/org/moqui/chat/ChatServices.xml) search refusal at 4042. | Open; no fix/retest. No server search or plaintext leakage occurred; local encrypted search is not claimed implemented. |
| COM-UI-012 / P1 | **Observed blocked workflow; candidate cause from source**; calls.client.b on two browser origins/devices; COM-09; Settings/Devices, Material light 1280×720 | Primary Verify → secondary Accept → primary Compare emoji removes the comparison choices on both devices but never shows emoji or Match/Differ; only Cancel remains. The other device stays Not verified. Observed through subsequent independent actions (over a minute), then canceled. Verification cannot complete through this path. Make intermediate states, timeout/retry and both sides' required protocol steps explicit before proposal §6 relies on verified-device labels/defaults. | B05 `08-verification-stalled-primary.{txt,jpg}`, `09-verification-stalled-secondary.txt`; [Devices.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Devices.qvue) verification controls; [WebrootCrypto.js](../runtime/components/base/webroot/screen/webroot/js/WebrootCrypto.js) `refreshVerifications`/`verificationNow` 1160–1187, 1227–1258. Adapter accepts the verification request and starts SAS but exposes no receiver SAS acceptance step; bundled SDK `Sas.accept()` returns an outgoing request. This is a candidate cause, not a fixed/proven diagnosis. | Open; no fix/retest. Initial opposite-direction request was not received during primary navigation/reload; fresh request after both pages were ready did arrive and reached the observed stalled comparison. QR/camera, post-backup verification and mismatch remain pending. |
| COM-UI-013 / P2 | **Observed UI / design judgment**; B sender, C unverified recipient; COM-09; encrypted room 100153, Material light 1280×720 | With B's verified-only setting on, event 5 appears normally sent for B but C receives the generic missing-key/restore-or-wait placeholder. No sender warning identifies that the recipient cannot read this message under the selected trust policy. The confidentiality restriction works; its delivery consequence is hidden. Proposal §6 needs a clear excluded/unverified-device state and an actionable verification route, distinct from missing backup/history. Do not label this as a failed encryption safeguard or infer successful delivery from the sent bubble. | B05 `14-verified-only-recipient.txt`, `15-verified-only-sender.txt`; [WebrootCrypto.js](../runtime/components/base/webroot/screen/webroot/js/WebrootCrypto.js) 861–903 rotates the session and selects `onlyTrustedDevices`; Chat's missing-key UI supplies generic recovery guidance. | Open; no fix/retest. C could read earlier post-setup messages, not this new restricted one. Same-user B-device verification is not proof of cross-user C trust. Setting restored off; no claim that already restricted event 5 became readable. |
| COM-UI-014 / P2 | **Observed responsive layout**; B secondary device; COM-09; `/qapps/BasicComms/Settings/Devices`, Material light 390×844 | My devices expands to **907px document width** in a 390px viewport. Explanations and backup/device actions run off the right edge, requiring horizontal scrolling for the recovery workflow. Stack backup controls/device actions, wrap key/identity text and bound child widths; include My devices in proposal §2/§6's phone recovery criteria. | B05 `16-devices-phone.{jpg,txt}`, DOM geometry recorded in batch; [Devices.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Settings/Devices.qvue) root column, backup `row ... no-wrap` and device list/action row. Those are owning layout surfaces, not a measured single-element root-cause claim. | Open; no fix/retest. Screenshot visually inspected; actual Android keyboard/camera, dark and TUI remain pending. |
| COM-UI-015 / P2 | **Proposal revision / current-state and delivery contract**; COM-10; Comms-Messaging §7 | Reconcile “DND applied by the client only”: current `apply_ChatDndHours` is a server job every five minutes, changing/restoring status in the user's zone while respecting manual changes. This does not implement the proposed durable push hold/summary. Keep presence scheduling, popup/sound suppression, persisted mention delivery and new push scheduling separate. Replace “never missed/already read must not ring” absolutes with explicit queue/expiry/retry/read-race outcomes; an app acknowledgment is not proof the person read a message. Preserve room/default/direct-message precedence and encrypted/no-preview privacy across each path. | B05 saved preferences and overnight hours/zone, with scheduled jobs deliberately disabled; [BasicCommsSetupData.xml](../runtime/components/base/basic-comms/data/BasicCommsSetupData.xml) 27–31; [ChatScreenServices.xml](../runtime/components/base/basic-comms/service/org/moqui/comms/ChatScreenServices.xml) `apply#DndHours` 212 onward; [ChatServices.xml](../runtime/components/base/framework/service/org/moqui/chat/ChatServices.xml) `notify#Mention` 1550 onward omits summaries for encrypted rooms or preview opt-out. | Open proposal revision. No push provider, closed-app/OS notification, scheduled DND transition or provider delivery result observed. Future acceptance covers those scenarios; focused rule regressions cover preference precedence/privacy and scheduling decisions. |
| COM-UI-016 / P2 | **Observed UI links / HTTP failure**; john.doe, own hosted account 100102; COM-03; MessageDetail, Material light 1280×720 | Print or view source opens the correct BasicComms message detail, but its Download .eml and Print links target `/apps/Comms/Inbox/MessageDetail`, absent in the framework-only runtime. Both return 404; the attachment link uses BasicComms and returns the correct 33 bytes. Derive both outputs from the owning app mount; preserve message identity and authorization through exports and printing. | B06 `07-message-detail.txt`, `08-eml.response`, `08-print.response`, `08-attachment.response`; [MessageDetail.xml](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/MessageDetail.xml) 103–104 hard-code Comms. | Open; no fix/retest. Observed link targets followed with curl per transfer preference; print rendering itself was not reached. Do not count an addon-mounted success as standalone coverage. |
| COM-UI-017 / P3 | **Observed validation feedback**; john.doe; COM-04; Inbox compose window, Material light 1280×720 | Sending a draft with `invalid-recipient` shows “Not an email address: invalid-recipient” twice in one notification. The subject/body survive and correcting the recipient permits sending. Consolidate duplicate validation messages without hiding independent errors or clearing the draft. | B06 `12-invalid-recipient.txt`; [Compose.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/Compose.qvue) send/error handling, [ComposeServices.xml](../runtime/components/base/basic-comms/service/org/moqui/comms/ComposeServices.xml) send/save delegation and [EmailClientServices.xml](../runtime/components/base/framework/service/org/moqui/mail/EmailClientServices.xml) 3533–3536 recipient validation. Owning error propagation layer requires triage; no root cause asserted. | Open; no fix/retest. Missing-recipient refusal also preserved content. No malformed-address delivery occurred. |
| COM-UI-018 / P2 | **Observed behavior / design judgment**; john.doe; COM-04; compose window with an explicit Bcc chip and empty To, Material light 390×844 | Send refuses a Bcc-only message with “Add a recipient in To,” although the composer accepts Bcc and its client gate permits any nonempty To/Cc/Bcc collection. A sender cannot use only hidden recipients through this path. Decide and align the contract: support Bcc-only sends with an appropriate visible header, or explain the To requirement before submission. Preserve envelope privacy either way. | B06 `17-phone-compose.{jpg,txt}`, `18-bcc-only-refusal.txt`, `15-capture-summary.json`, `19-final-sent.txt`; [Compose.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/Compose.qvue) 979–983 accepts any recipient collection; [EmailClientServices.xml](../runtime/components/base/framework/service/org/moqui/mail/EmailClientServices.xml) 3533 requires `toAddresses`. | Open; no fix/retest. Adding a To recipient succeeded: the sink received both envelope recipients, no Bcc MIME header; the sender's Sent view retained Bcc. This is not a disclosure finding. |
| COM-UI-019 / P2 | **Proposal revision / preserve mail semantics in the unified app**; COM-01–04/18; Comms-Messaging §1–2/6–7 and the network-room design | Add the existing email workflows to the built baseline and regression-preservation criteria: account/identity and folder scope, email threading, To/Cc/Bcc, quoted reply, drafts, undo-send hold, saved search, EML/attachment/print outputs and truthful sent/failed state. Phone chat redesign must retain these mail-specific tasks and mount-independent output links. Distinguish account activation, imported history, SMTP acceptance and recipient delivery/read; hosted setup's no-sign-in Test is not a gateway readiness test. Chat E2EE/defaults and read receipts must not imply that ordinary email is encrypted end-to-end or read by its recipient. | B06 hosted wizard and local mail walk; [Comms-Messaging.md](Comms-Messaging.md) §1 built baseline is chat/calls-oriented; [Mail.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/Mail.qvue), [Compose.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/Compose.qvue), [Finish.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/MailAccounts/Add/Finish.xml). | Open proposal revision; no proposal/application edits. Include framework-only and addon mounts in future acceptance, captured MIME/envelope checks, provider-failure recovery and ordinary/restricted actors. Keep focused rule/bug regressions separate from full walks. |
| COM-UI-020 / P1 | **Observed behavior / audience mismatch**; john.doe, account 100102; COM-05; Settings/OutOfOffice, Material light 1280×720; local SMTP only | Select **My contacts and people inside the organization** with no contacts hook installed, save On, then deliver mail from an unknown external address. The local sink captures a public automatic reply to that outsider. The selected audience silently behaves like Everyone for external senders, potentially disclosing an away message beyond the intended audience. Make unavailable contact classification explicit and resolve the fallback conservatively; never silently widen the selected audience. | B07 `06-ooo-outsider.json`, `10-ooo-contacts-enabled.{txt,jpg}`, `11-capture-summary.json`; [EmailClientServices.xml](../runtime/components/base/framework/service/org/moqui/mail/EmailClientServices.xml) `filter#ContactAddresses` explicitly defines null/no hook as all senders; `send#OutOfOfficeReplies` leaves `contacts` null and permits outsiders. [OutOfOffice.xml](../runtime/components/base/basic-comms/screen/BasicComms/Settings/OutOfOffice.xml) caption does not explain this fallback; its introductory comment instead says no hook means internal only. | Open; no fix/retest. All content synthetic and captured locally; no real disclosure. Internal-only correctly suppressed a separate outsider. Hook-present/failing/empty results and organization-domain classification require later coverage. Priority reflects the audience/privacy mismatch, not an authorization bypass. |
| COM-UI-021 / P2 | **Observed capability/feedback mismatch**; ordinary B read member and C downgraded sender; COM-03/04/06; Inbox/Mail and open composer, Material light 1280×720 | A read member sees an active Archive action; clicking returns “You may not change the mail of email account 100102.” The reading pane correctly explains no-send access. Separately, downgrade C from Read and send to Read while a saved composer is open: Send remains available with its alias selected, then refuses “Choose a mail account to send from.” Align mutation controls with account capabilities and explain a permission change with a recovery route while preserving the draft. | B07 `15-readonly-archive-refusal.txt`, `16-readonly-message.txt`, `17-revoked-send.txt`, `25-retained-draft.txt`; [Mail.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/Mail.qvue) row/selection actions versus `thread.canSend`; [ComposeServices.xml](../runtime/components/base/basic-comms/service/org/moqui/comms/ComposeServices.xml) `get#Identities` excludes accounts without canSend, send reports missing identity; [Compose.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Inbox/Compose.qvue) owns open-composer feedback. | Open; no fix/retest. Server refused both mutations; no revoked-send MIME was captured. C could send before downgrade. After removal C's refreshed Inbox was empty; owner retained the unsent draft. Do not characterize this as an access-control failure. |
| COM-UI-022 / P2 | **Proposal revision / mail policy and shared capabilities**; COM-05/06; Comms-Messaging §1–2/6–7 and unified/network-room design | Extend COM-UI-019's preservation criteria to account-scoped identities/reply-to/signatures, ordered server rules with explicit preview versus run scope, automatic-reply audiences and shared mailbox capabilities. Specify who owns shared policy and whose contacts classify senders; missing/failed classification must not silently broaden a promised audience. Keep mail OOO distinct from chat presence/DND or push suppression. A mail read member may mark read but may not archive or send; stale roles need clear draft-preserving recovery. Do not transplant chat membership/receipt semantics into shared mail. | B07 identity/rule/OOO consumers and ordinary shared users; [BasicEntities.xml](../runtime/components/base/framework/entity/BasicEntities.xml) EmailAccountUser EapRead/EapSend/EapManage; [MailSettingsServices.xml](../runtime/components/base/basic-comms/service/org/moqui/comms/MailSettingsServices.xml), [EmailClientServices.xml](../runtime/components/base/framework/service/org/moqui/mail/EmailClientServices.xml), [proposal](Comms-Messaging.md). | Open design handoff; no proposal/application edits. Future acceptance combines identity → incoming rule → audience-specific captured reply → shared role change/leave. Focused regressions cover audience fallback and capability/error contracts; full flows remain in acceptance. |

### Bounded batch record

For each later batch record:

- Intent, activities and branch limits; runtime/build/components, identities/grants, fixture IDs and dates.
- Observed steps and outcomes; evidence filenames, console/network/output observations and findings.
- Workarounds and blocked/deferred branches, with prerequisites and follow-up.
- In refinement only: decisions, owning files/components, correction, focused verification commands/results and retest.
- Temporary settings/grants/files/jobs restored, remaining cleanup and companion handoffs.

### COM-B01 — Preliminary empty-state and setup baseline, 2026-11-16

**Intent.** The user requested BasicComms next, combined with newly received suggestions. Repository inspection
found the existing Remaining-Work/Future-Plans Comms backlog but no clearly identified incoming suggestions.
Their text/location was requested. The subsequent COM-B02 resolved this to PR #202 / Comms-Messaging.md and
incorporated proposal revisions into findings, per the user. This preliminary batch establishes only the baseline below.

**Runtime and actor.** Restarted the previous disposable runtime `/tmp/mr-ui-four-apps-zf5098dh`, loopback port
18872, current source `83cf05b3b6b069812a6ac2802d95f918db6a9802` and already-built debug binary SHA-256
`f799a82c681a7c3c9a7973cf5e030a5d1ebf41fa8935280f0d527dbf61bb2004`. Reused scratch PostgreSQL includes prior
review history; this is not another fresh database. Six components framework/webroot/tools/basic-comms/dev-aide/
example, no addons; scheduled jobs and external listeners disabled, mail hosts restricted to loopback, no captured
mail/media/provider fixture. Existing browser session resumed as `john.doe` / `EX_JOHN_DOE` (ADMIN + ADMIN_ADV).
This is administrator orientation only, not the ordinary participant baseline. Material light/default comfortable,
1920×1080. No build, automated tests, application changes or messages sent.

**Observed sequence.**

- Default `/qapps/BasicComms` opened Inbox with explicit no-mail-account guidance and an empty conversation pane.
  Compose opened a floating editor explaining that no sending identity was available, disabled Send, and offered
  Mail accounts in a new tab plus Refresh sending identities. Closed it with no content entered.
- Add an account → Address: Next with an empty address produced “Field cannot be empty.” Entered local fixture
  `review-comms@localhost`, selected Hosted here and advanced to Connect. The page explained gateway delivery,
  no password and that the next step would add a paused account. Left via Mail accounts before that action;
  the account list remained in its no-account state. No account creation or connection success is claimed.
- Comms menu → Home showed zero Flagged/Snoozed/Mentions/Missed calls, no unread rooms and account guidance.
  The example component contributed Gallery notes; this is a loaded framework development extension, not BOSS.
  Home → Chat showed initial conversation guidance. New room defaulted Private with a membership explanation,
  encryption off, and Create disabled while Name was empty. Cancel was invoked but the immediate AX snapshot
  still contained the closing dialog; settled dismissal/focus was not verified and remains pending.

**Evidence and continuation.** Ignored `plans/ui-refinement-evidence/basic-comms/batch-01/` holds runtime metadata,
`01-inbox-empty.{txt,jpg}`, `02-compose-no-identity.txt`, `03-account-address-required.txt`,
`04-hosted-before-create.txt`, `05-no-account-created.txt`, `06-home.{txt,jpg}` and `07-new-room-empty.txt`.
No defect is established by this bounded inspection. Prepare ordinary participant identities and local capture
fixtures before advancing mail/chat success, access or delivery coverage; combine the next batch with the actual
suggestions identified in COM-B02. Alternate modes, rich content, multi-user interaction, calls and recovery remain pending.

**Cleanup.** No mail account, room, message, grant or preference was created/changed. Root viewport override reset
and temporary browser tab closed. Runtime shutdown is recorded in `runtime-cleanup.json`; scratch data/evidence are
retained. No logout outcome is claimed.

### COM-B02 — Populated phone-width chat and messaging-plan reconciliation, 2026-11-16

**Scope.** Review pass only, informed by merged PR #202 and the user's explicit request to include proposed
Comms-Messaging changes in the findings. Two observed UI issues and four source-grounded design revisions; neither
proposal adoption nor static inspection establishes completion of an activity. A supporting agent reconciled the
proposal/current code; root performed the live browser walk. No application changes, tests or external messages.

**Runtime.** Source `a302c9b07211add6cefbf28e39b088ab17751bf1`; rebuilt after the pulled Rust changes using
`cargo build --profile dev --no-default-features --features maximal,devtools -p moqui -j 4` (passed in about one
minute; linker exception-frame size warning). Debug binary SHA-256
`a83c6f287aa9a12b9feb1b9b23dece06813e254e91bd21f19a209307cfc54e2f`. Reused scratch runtime/database from COM-B01,
same six components and loopback-only configuration, `localhost:18872`; no addons. Administrator `john.doe`,
EX_JOHN_DOE with ADMIN + ADMIN_ADV, existing session. Material light/default comfortable at 1920×1080 then
390×844. Started 2026-11-17 00:22 UTC (Nov 16, 18:22 America/Chicago). No ordinary-user/multi-device proof.

**Observed walk.** Created private room `100000`, `COM-B02 review notes`, with no additional participants,
Encrypted off and explicit disposable-review topic. Sent event 2, then at 390px observed an own-message bubble.
Right-click on that message opened the bottom action sheet with reactions, reply/thread, copy, forward, edit,
pin, unread and delete; this verifies context-menu access, not physical touch or long-press behavior. Reacted 👍,
opened Reply in thread, sent reply event 4, then reloaded. Original message, reaction and one reply remained.
Thread Back returned to the room with the reply link; room Back returned to the room list. The private lock
presentation and phone Send overflow are COM-UI-001/002. One-person persistence does not establish delivery,
read receipts, cross-user notifications or encrypted content behavior.

**Evidence.** Ignored `plans/ui-refinement-evidence/basic-comms/batch-02/`: `runtime-metadata.json`, `build.log`,
`01-desktop-message.txt`, `02-phone-room.{jpg,txt}`, `03-message-actions.{jpg,txt}`,
`04-thread-reloaded.txt`, `05-send-geometry.json`, `06-room-return.{jpg,txt}`, `07-phone-list-return.txt`.
Source-grounded plan findings name their actual owning files and unresolved design choices. Comms-Messaging.md
and the archived framework plan remain unchanged; the review register is the consolidation point for now.

**Next bounded batch.** Ordinary owner/member/outsider fixtures; phone picker, true touch/keyboard message actions,
full/compact/host boundaries and details tabs; contrast encryption and bridge/plain labels; then read markers,
notification preferences and recovery. Preserve mail waves from COM-B01. Physical Android keyboard/back gesture,
background push and call media need appropriate devices/fixtures; width emulation is not a substitute. Keep all
unrecorded branches pending and defer implementation to the later combined pass.

**Cleanup.** Room 100000, original message, reaction and reply retained only in the disposable database as a named
follow-up fixture. No other participants, mail/providers or grants changed. Viewport reset, temporary tab closed,
server stopped with SIGINT and verified absent together with its HTTP listener and embedded PostgreSQL processes;
see `runtime-cleanup.json`. No logout result claimed. Scratch runtime and local evidence retained.


### COM-B03 — Two ordinary chat participants and phone navigation, 2026-11-16

**Scope and runtime.** Review only; root walked the UI and a supporting agent reconciled the messaging proposal
with current source. Source `63f84fea38c13c0c000bae0b26e5e96a12ce45ab`; changes since B02 were documentation,
so reused B02's debug binary (SHA-256 `a83c6f287aa9a12b9feb1b9b23dece06813e254e91bd21f19a209307cfc54e2f`).
Same disposable database and six individually linked components, no addons; loopback HTTP 18872, scheduled jobs
and external protocol listeners disabled, mail restricted to localhost. No provider/media fixture or external
communication. Started 2026-11-17 00:38 UTC (Nov 16, 18:38 America/Chicago). Material light/default comfortable;
390×844 phone width, selected Files checks at 640×900 and 1024×900, compact picker at 1024×900 and 390×844.
Width emulation does not establish physical touch, native keyboard or Android host behavior.

**Actors and fixture preparation.** Existing regression fixture
[CallClientFixtures.spec.js](../runtime/components/base/basic-comms/test/CallClientFixtures.spec.js), run with
`target/debug/moqui test basic-comms --spec CallClientFixtures --db existing` in the isolated runtime, prepared
`calls.client.b` / Calls Client B and `calls.client.c` / Calls Client C with `BASIC_COMMS_USERS`. No ADMIN grant;
BOSS_USERS is absent without addons. Result: one feature passed, zero failed, 0.70s spec / 0.31s feature.
This confirms fixture setup only, not UI behavior or the full Comms regression suite. No new/changed tests,
automated acceptance runs or build. B signed in after administrator logout on 127.0.0.1; C used localhost's separate
session. Visible account names distinguished the actors. C initially resumed a stale Standard route from the reused
session, then explicitly opened full Material Chat; this is not evidence of a fresh-login routing defect.

**Observed branches.**

- B's ordinary launcher offered Comms. At 390, New message opened the maximized recipient picker. Selecting C,
  Cancel and reopening left the recipient empty and Start disabled. Selecting C again and Start created DM
  **100051**. Both participants then used this plain local review conversation.
- B sent event **1**; C's open room list received its preview and unread badge live. Opening the room showed the
  new-messages boundary and cleared unread. C sent event **2**, received live by B. No reload needed for either.
- On B's focused incoming message, Tab reached the accessible thumbs-up reaction button; Enter applied it and
  C saw 👍1 live. A reader avatar appeared on the author's message. Mouse hold is not a touch event: the source's
  450ms touch handler was not exercised; right-click remained the verified phone action-sheet entry.
- B quoted C's message and sent event **4**. C edited event 2; B saw the changed text and edited marker live.
  The quoted excerpt also changed to the edited text. B's actions for C's message omitted Edit/Delete; this is
  visible action availability, not a direct service authorization test.
- B pinned C's message. C opened Room → Pinned messages, saw it, then unpinned; the pane showed its explicit
  empty state and B received the system update. Shared message pinning is distinct from personal Pin to top,
  which was not exercised.
- Files exposed All/Pictures and explicit no-files guidance. Inspected the phone pane and the 1024 layout where
  the room list returns; also captured 640. Search for a missing term showed “No messages match”; searching
  COM-B03 returned three messages including the edit, and selecting the result returned to that conversation.
- C opened a thread on event 1 and sent event **8**. B received the one-reply link live, opened it, entered an
  unsent thread draft, returned to the room and reopened the thread: the draft was restored. Cleared that test
  draft before leaving. This covers pane return, not reload/offline/native queued-work recovery.
- B directly opened prior administrator-only private room **100000** and received “This conversation is not
  available / It does not exist, or you are not in it,” with no private content shown. Selecting B's own DM
  recovered normally. No-Comms identities, content bytes, helper routes and all other refusal branches remain pending.
- Create → Chat message opened the compact picker. The same latest message had different times in full/compact
  lists (COM-UI-007). At 390, selecting the already-open conversation left the picker visible; this does not
  establish a different-room handoff result. Cancel removed recipient entry; Close dialog settled to no dialog.
  Host-home picker/FAB and genuine tray conversation switching remain pending.

**Findings and proposal reconciliation.** Added COM-UI-007 (observed time formatting disagreement) and COM-UI-008
(proposed Info status semantics); extended COM-UI-002/003 with B03 evidence. Current read markers, bubbles,
picker, pinning, All/Pictures and keyboard reaction are implemented behavior to preserve during extraction.
Proposed Info and Links/Voice/Reactions must be described as additional work. Phone room and pane/thread controls
both expose “Back”; define destination labels, focus return and ordered dismissal in §2's navigation criteria.
No new security failure or encryption/delivery guarantee inferred from these observations. No application fixes;
Comms-Messaging.md and the archive remain unchanged, with proposed revisions consolidated in this register.

**Evidence.** Ignored `plans/ui-refinement-evidence/basic-comms/batch-03/`: runtime metadata and fixture result;
`01-phone-picker.jpg`, `02-picker-reopened.txt`, `03-c-unread.txt`, `04-b-received.txt`,
`05-c-reaction-read.txt`, `06-two-user-phone.jpg`, `07-live-edit.txt`, `08-other-author-actions.txt`,
`09-c-pinned.txt`, `10-files-phone.{txt,jpg}`, `11-files-640.jpg`, `12-files-1024.jpg`,
`13-search-empty.txt`, `14-search-return.txt`, `15-b-thread.txt`, `16-thread-draft-restored.txt`,
`17-outsider-refusal.txt`, `18-compact-picker.{txt,jpg}`, `19-picker-closed.txt`, `runtime-cleanup.json`.
Visual judgments use the inspected 390/1024 captures; the 640 capture alone does not close a layout check.
No console/network-wide clean-bill claim.

**Unfinished branches / next batch.** Group owner/member/outsider changes, remaining message actions (forward,
delete, reaction removal/details, polls), populated files and transfer recovery, receipt/notification preferences,
encryption/device trust, genuine compact/host conversation use, dark mode, actual touch and physical Android
remain pending. Continue these separately from mail fixture success and call/media workflows COM-11–16;
“phone” in this batch refers to viewport/layout, not a completed telephony review. Keep no-provider and
no-device limits explicit; do not mark COM-07/08/18 complete from this partial walk.

**Cleanup.** B/C fixture users, DM 100051 and its local messages/reaction/thread/edit remain in the disposable
review database; shared pin removed and unsent thread draft cleared. Prior room 100000 retained. No external
accounts/messages, provider settings or application code changed. Reset the active viewport override and closed
both temporary tabs; browser inventory empty. Stopped server PID 40976 with SIGINT, observed successful process
exit and verified no matching scratch PostgreSQL process or HTTP 18872 listener remained. Scratch data/evidence
retained; no logout result claimed for B/C.


### COM-B04 — Group membership, message actions and populated files, 2026-11-16

**Scope/runtime.** Review pass only, using the unchanged source `63f84fea38c13c0c000bae0b26e5e96a12ce45ab`,
B03's binary SHA-256 `a83c6f287aa9a12b9feb1b9b23dece06813e254e91bd21f19a209307cfc54e2f` and retained isolated
runtime `/tmp/mr-ui-four-apps-zf5098dh`. Same six individually linked components, no addons, loopback HTTP 18872,
external protocol listeners/scheduled jobs disabled and mail restricted to localhost. No mail/media providers.
Started 2026-11-17 00:53 UTC (Nov 16, 18:53 America/Chicago). Ordinary B/C sessions from B03 resumed and account
labels were checked; both have BASIC_COMMS_USERS. B used 127.0.0.1 at the actual default 1280×720; C used localhost,
initial desktop then 390×844. Material light/default comfortable only. No builds, new fixture spec, automated tests
or app fixes. Curl handled transfers as requested; its distinct B/C sessions were authenticated to this scratch
instance. This does not exercise the browser file chooser, upload queue/progress or browser download handoff.

**Group and membership.** B created plain private room **100102**, `COM-B04 group review`, with topic and C selected
in Members. Both saw it. B's Members pane identified B as owner and exposed C's role menu; C's member view offered
Add people but no owner/moderator controls. B promoted C to moderator, C saw the role/event live, then B restored
member. Later B removed C while C's Files pane was open: C returned to the conversation list and the group vanished.
A direct stale permalink with eventSeq=4 showed the unavailable/not-in-room explanation. B re-added C and C reopened
the group with its previous content. No third participant, public-room join, bridge member or encrypted key change
was exercised.

B then left as the sole owner. The confirmation omitted the impending transfer; C automatically became owner and
saw an event attributing that promotion to B (COM-UI-009). C added B back as a member and explicitly promoted B;
B restored C to member. Final UI: B owner, C member, two members. This also verifies that re-add did not silently
restore B's former owner role. No administrative grant was needed, and these are group roles on local fixtures.

**Message actions.** B sent event **4**, `COM-B04 message for copy, forward, reactions and deletion.`

- Copy text produced that exact clipboard text; Copy link produced the owning room/eventSeq=4 permalink.
- Forward selected the existing B/C DM **100051**, added a comment and reported Forwarded/Open. C's destination
  showed comment, original body, source author and original time (DM event **9**). A copy already forwarded to
  another room is a separate destination record; source membership checks do not imply recalling copies.
- C reacted 🎉, B saw it live, C clicked the chip to remove it. More reactions opened the fixed emoji grid and C
  selected 🚀, retained on event 4. This is right-click/button interaction, not physical long-press or proof of
  the proposed searchable/categories/recent picker. Reactor-name tooltip/long-press disclosure remains pending.
- Mark unread from here showed its acknowledgment, badge and new-message boundary. This does not establish
  unread persistence through every subsequent navigation/device. Edit → changed text → Cancel restored the
  original; no edited marker/new content persisted in the inspected room.
- Delete message described removal for everyone with a room placeholder. Cancel retained the message. The
  irreversible submission was not performed: the browser tool requires action-time confirmation for permanent
  deletion. No deletion propagation, attachment redaction or forwarded-copy recall is claimed.
- C created poll **event 9**, `COM-B04: which fixture should we inspect?`, with Text notes / Blue picture.
  Empty creation disabled Post; B's blank poll was canceled. B voted Text notes, C voted Blue picture, live totals
  reached two. Close confirmation explained retained votes/no further votes; Cancel kept the poll open, then
  explicit Close retained both results with “2 votes · closed” and no voting buttons. Advanced options,
  duplicate/invalid options, expiry, anonymous/multiple votes and offline voting remain pending.

An initial phone poll click made while a preceding reaction sheet was closing had no visible result; retry from
the settled room opened it. This timing limitation is not recorded as a product defect. Desktop message actions
needed the message focused again after a menu closed; no keyboard-accessibility conclusion from locator timeouts.

**Populated files and access.** Curl as B uploaded a synthetic **63-byte text file** and **179-byte 96×64 PNG**,
then sent one fixture message **event 5** with both attachments: **100000** `COM-B04-notes.txt` and **100001**
`COM-B04-blue.png`. Both upload responses had empty failedList. The browser received the message live with file
name/size and picture. C's All list showed both; Pictures showed just the PNG. On the phone, opening the picture
showed its viewer, name and Download/Show in chat; Show in chat returned to the owning message. The restored
All view fit the inspected phone width with both rows and actions. This two-item view does not cover long names,
large lists, video/audio/PDF, media playback or encrypted metadata.

Curl downloads as member C returned HTTP 200, correct content types/names, `Cache-Control: private, no-store`
and exact uploaded bytes for both files. After removal, text download, inline PNG and files list each returned
HTTP 500 with a generic file/room-not-found JSON error and no fixture bytes. The refusal was effective; do not
interpret 500 as a successful response or a full HTTP-status design review. After re-add, the text download again
returned matching bytes and the browser showed both files. Bytes already downloaded or forwarded copies are not
revoked by later membership removal; future proposal copy should not promise otherwise.

**Pagination follow-up.** Source review of the existing whole-message cursor suggested an edge case. After the
UI walk and initial shutdown, briefly restarted the same isolated runtime for three read-only curl requests:
`files?chatRoomId=100102&limit=1`, then its returned `beforeEventSeq=5`, compared with `limit=30`. The first page
returned the text attachment, the next page empty, while the larger page returned both files. COM-UI-010 records
this **observed HTTP defect** separately from the successful default-size UI. No larger synthetic upload or test
suite was needed; pagination evidence is `22-file-pagination.json`.

**Proposal changes to carry into triage.** Extend COM-UI-003's built/partial/missing baseline with current
forward/comment, fixed expanded reactions, poll lifecycle and image viewer/Show in chat. For §5 People, include
role-sensitive commands, last-owner succession and recovery after removal/re-add (COM-UI-009). For §5 detail tabs,
resolve same-message cursor boundaries first (COM-UI-010), define combined image/video ordering and non-media MIME
exclusions, and preserve live removal of open panes plus server checks on every file/thumbnail request. Add an
explicit failure/recovery case for an in-flight pane request completing after removal. Current two-member removal
worked; that race remains untested. Preserve existing forwards as a separate retention decision and distinguish
removal of a source message from recalling exported/forwarded copies. No edits to Comms-Messaging.md or its archive;
these review findings remain the consolidation point until the combined refinement pass.

**Evidence.** Ignored `plans/ui-refinement-evidence/basic-comms/batch-04/` contains runtime metadata (including the
brief HTTP-only restart), upload/send JSON, `01-member-capabilities.txt`, `02-role-live.txt`,
`04-forward-destination.txt`, `05-files-desktop.txt`, `06-files-phone.jpg`, `07-picture-viewer.txt`,
`08-member-downloads.json`, `09-reaction-picker.txt`, `10-poll-closed.txt`, `11-mark-unread.txt`,
`12-delete-confirmation.txt`, `13-revoked-downloads.json`, `14-member-removed.txt`, `15-stale-link-refused.txt`,
`16-owner-leave-warning.txt`, `17-successor-owner.{txt,jpg}`, `18-membership-restored.txt`,
`19-files-restored.{txt,jpg}`, `20-edit-canceled.txt`, `21-restored-download.json`, `22-file-pagination.json`
and cleanup metadata. Inspected screenshots: phone Pictures and restored All. No console/network-wide clean-bill
claim; an inspected C console sample had no errors/warnings, which is not comprehensive monitoring.

**Remaining/cleanup.** All activities remain partial. Continue the listed action branches, three-person role
competition, public/record/bridge/encrypted groups, UI upload recovery and large/mixed media separately; dark,
TUI/Standard, touch/native and call/provider prerequisites remain unchanged. B/C group roles restored; retained
room 100102, original message/reaction, two files, closed poll, membership history and forwarded DM event 9 only in
the disposable database. No draft left in the inspected editors. Reset viewport override and closed both temporary
tabs (empty inventory). Both server runs stopped with SIGINT; session exits successful. Verified final server,
embedded PostgreSQL and port 18872 listener absent. Removed curl cookie jars and token-bearing scratch HTML;
fixture files/data and ignored evidence retained. Review document changes remain uncommitted.


### COM-B05 — Encrypted chat, device trust and notification preferences, 2026-11-16

**Scope/runtime.** Review only, unchanged source `63f84fea38c13c0c000bae0b26e5e96a12ce45ab`, same binary SHA-256
`a83c6f287aa9a12b9feb1b9b23dece06813e254e91bd21f19a209307cfc54e2f` and retained isolated runtime/database
`/tmp/mr-ui-four-apps-zf5098dh`. Same six individually linked components without addons, loopback HTTP 18872,
external protocol listeners/jobs disabled and mail restricted to localhost. Started 2026-11-17 01:08 UTC
(Nov 16, 19:08 America/Chicago), PID 42322. No build, automated tests, provider registration, external communication
or application fixes. Ordinary B/C BASIC_COMMS_USERS fixtures resumed. B's primary origin was 127.0.0.1, C's
localhost; `comms-b2.localhost` supplied a separate browser store/session for **the same B account**, signed in
through the normal UI. This is three isolated origins in one browser, not physical multi-device/Android proof.
Material light/default comfortable, desktop 1280×720; B secondary My devices also inspected at 390×844.

**Encrypted exchange and first use.** B's My devices initially said No devices yet / no encryption keys. From
Comms → Chat → New room, enabled Encrypted; the form explained irreversible E2EE and unavailable server search,
polls, forwards, bridges, agents, terminal and record filing. Created **100153**, `COM-B05 encrypted review`,
with C. Header showed Encrypted; the composer omitted Make a poll. B sent **event 2** before C had created a
crypto device. B read it, while C's new device showed explicit missing-key guidance (backup or another own device),
not plaintext. C then sent **event 3**, decrypted by B; B sent **event 4**, decrypted by C. C's pre-device event 2
remained unavailable. Room-list previews said Encrypted message. This is visible exchange/recovery behavior, not
an independent cryptographic audit or database ciphertext inspection. Encrypted Search remained enabled and only
explained its refusal after submission (COM-UI-011).

**Devices and trust.** Browser first-use registered B primary **EXSDUACBWH** and B secondary **FVELFMISOR**;
C's device was exercised but its ID was not separately inventoried. B renamed the primary `COM-B05 B primary`,
visible on the secondary, and restored the original `Chrome on macOS` at cleanup. Device rows exposed This browser,
IDs, public fingerprints, browser identity and times. Trust labels/Verify appeared after crypto metadata loaded
and Refresh; initial rows without those controls are not classified as verified. The first B-secondary→primary
verification did not arrive while the primary was reloading/navigating; canceled it. With both My devices pages
ready, B-primary→secondary Verify arrived; secondary Accept exposed Compare emoji/QR on both. Primary Compare
emoji left both pages with only Cancel and no emoji, Match/Differ or progress explanation. Independent work
continued for over a minute with that state unchanged; canceled, leaving the device Not verified (COM-UI-012).
No matching/mismatching emoji, QR scan, camera permission or successful trust establishment is claimed.

**Backup and recovery.** Primary Set up key backup prompted “Confirm it is you.” Cancel kept it unconfigured;
retry with the fixture's existing password succeeded and showed one recovery key and **2/2** backed-up room keys.
The one-time disclosure was dismissed after holding the fixture key transiently for this walk. Secondary Refresh
said the backup was set up on another device and offered Restore. Invalid fixture input produced “This is not a
recovery key” and remained editable. Correct recovery reported **Keys restored: 2**; reopening encrypted room on
the secondary displayed all three older messages (events 2–4), which had previously been placeholders there.
The primary's New recovery key confirmation explained that older keys only in the previous backup are not copied;
canceled it. No backup rotation/older-version restoration performed. Recovery key material was not written into
this plan or saved evidence; transient variables used for restore were cleared. The account's backup and browser
stores remain disposable review fixtures, not an operational recovery arrangement.

**Verified-only policy.** Enabled B's “Send my room keys only to verified devices,” then sent **event 5**.
B saw its normal sent message; C's already-working device could not decrypt this new event and received generic
missing-key guidance, without an exclusion warning on B (COM-UI-013). This demonstrates the unverified recipient
restriction, not successful verification or delivery to C. The setting was restored off through B secondary and
confirmed off on B primary. No assertion that event 5 was retroactively shared. Backup later showed **3/3** on
primary, reflecting the additional session. The phone My devices page overflowed horizontally, measured document
width 907 versus viewport 390 (COM-UI-014). Remove **this** device opened a warning that its local keys would be
deleted and a new device created next use; canceled. Actual destructive revoke/start-over and subsequent refusal
remain pending under the browser tool's action-time confirmation rule for permanent deletion.

**Notification preferences and quiet hours.** C's initial defaults: room-derived notification level, previews,
sounds, Enter-sends, receipts, typing and link previews on; auto-away 10 minutes, no DND hours. Saved Mentions only,
previews off, sounds off and receipts off; save acknowledgment and later page entry retained the values.
Apply to my rooms now explained that direct messages are excluded. Cancel returned without applying; later Confirm
reported **2 rooms set**. Opened the encrypted room menu, then toggled Mute and verified the next menu offered
Unmute; unmuted. This verifies controls/persistence, not sound playback, push delivery or complete room/direct
precedence. With receipts off, C's loaded timeline omitted reader indicators; no wire-level privacy test or old
recipient read-position comparison performed.

DND From 22:00 with To empty was refused (“Give both times ... or neither”), preserving From. Adding 07:00 saved
an explicit overnight summary **in UTC**, the effective user zone, with No do not disturb hours to clear it.
Cleared it again. Jobs remained disabled throughout, so status transitions/automatic restoration were not executed.
This source reconciliation is COM-UI-015, not a runtime failure. Restored previews/sounds/receipts on; saved All,
applied to restore the two private rooms (two-room acknowledgment), then restored the original room-derived default.
Typing/link previews, Enter behavior, auto-away and presets were unchanged. No notification permission/provider,
XMPP/app password, background/closed-app delivery, native push, sound or call test performed.

**Findings/proposal handoff.** Added COM-UI-011–015. §6 should distinguish initial/new-device history, missing backup,
unverified exclusion, successful same-user recovery, verification state and future cross-user verification entry.
Backup success must not imply verified counterpart devices; restored plaintext is the recovery criterion.
Keep unavailable server search honest while local search remains a decision. §2/§6's phone acceptance must include
My devices and recovery actions, not only message bubbles. §7 must separate current preference/status behavior from
new durable push work and define bounded retry/expiry/read-race outcomes. Preserve the complete review→triage→fix
sequence; Comms-Messaging.md and its archive remain unchanged, with proposed changes recorded here.

**Evidence.** Ignored `plans/ui-refinement-evidence/basic-comms/batch-05/`: runtime metadata;
`01-no-devices.txt`, `02-new-device-no-key.txt`, `03-encrypted-exchange.txt`, `04-encrypted-search-refusal.txt`,
`05-second-device-missing-keys.txt`, `06-notification-defaults.txt`, `07-applied-room-preference.txt`,
`08-verification-stalled-primary.{txt,jpg}`, `09-verification-stalled-secondary.txt`, `10-backup-reauth.txt`,
`11-invalid-recovery.txt`, `12-backup-restored.txt`, `13-recovered-history.txt`,
`14-verified-only-recipient.txt`, `15-verified-only-sender.txt`, `16-devices-phone.{txt,jpg}`,
`17-device-remove-confirmation.txt`, `18-dnd-validation.txt`, `19-dnd-saved.txt`,
`20-preferences-restored.txt`, `21-devices-final.txt`, geometry/cleanup metadata. Inspected screenshots: stalled
verification and phone device overflow. The search refusal produced a console HTTP-500 warning; expected no-backup
service errors were visible in the scratch log. No comprehensive console/network clean-bill claim.

**Remaining/cleanup.** COM-09/10 stay partial. Verification completion/mismatch/QR, actual revoke/start-over,
older backup recovery, encrypted files/threads, cross-user trust, outsider/key helpers, call E2EE, notification
provider/background behavior and scheduled DND remain pending, as do dark/Standard/TUI and physical devices.
B primary name, verified-only setting, C notifications/receipts/sounds/previews, both private-room notification
levels, mute and DND hours restored as above. Retained encrypted room 100153, events 2–5, three browser crypto
stores/devices and B's test backup in the disposable environment. No new grants; no secret recorded in repository
evidence. Reset viewport, closed all three temporary tabs and verified empty browser inventory. Stopped PID 42322
with SIGINT, successful session exit; verified server, scratch PostgreSQL and port 18872 listener absent. No logout
or persistent credential/device deletion claimed. Review documentation remains uncommitted.


### COM-B06 — Mail setup, inbox triage and compose/reply, 2026-11-16

**Scope/runtime.** COM-01–04 and supporting COM-06 import; review only on unchanged source
`63f84fea38c13c0c000bae0b26e5e96a12ce45ab`, the retained isolated runtime `/tmp/mr-ui-four-apps-zf5098dh`,
same six individually linked components, no addons, binary SHA-256
`a83c6f287aa9a12b9feb1b9b23dece06813e254e91bd21f19a209307cfc54e2f`. Initial sandbox startup failed to bind;
approved restart PID **43053** began 2026-11-17 01:39 UTC (Nov 16, 19:39 America/Chicago), serving loopback 18872.
Scheduled jobs, external protocol listeners and IMAP IDLE remained disabled; mail hosts restricted to localhost.
Existing B at 127.0.0.1; existing administrator John Doe at `comms-admin.localhost`. Material light/default
comfortable, most actions 1280×720, composer inspected at 390×844 and final Sent at 1920×1080; DOM widths matched
those viewports. No builds, tests, acceptance runs, external providers, real mail or application changes.

**Permission boundary.** Ordinary `calls.client.b` (BASIC_COMMS_USERS) entered a valid hosted address and reached
Connect, then Next refused: “Only a mail administrator makes an account hosted here; ask one, or add an account of
another server.” A planned fixture grant of only EMAIL_ACCOUNT_HOSTED was rejected by automatic approval review
because it would expand access beyond the review request. The grant script did not run; no group, membership or
permission was added. Continued with the already-authorized administrator fixture, using its own new account.
This is a broad-holder success, **not** a narrow-holder or ordinary-user success. No ownership reassignment or
other workaround to the rejected grant. Narrow-holder setup and ordinary/restricted populated-mail work remain open.

**Account setup.** John Doe followed Add an account → Address (`com-b06@review.invalid`, display name
`COM B06 Review`, Hosted here) → Connect → Test → Options → Finish → Activate account. Connect described a paused
account without a password; Test said nothing to sign in to. Options saved `COM B06 local mail` and Only what I link;
Finish repeated address, hosted delivery and private-until-linked filing. Activated **account 100102** displayed
Delivered here, six folders and Open the Inbox, without claiming remote sync. No provider handshake, domain routing
or gateway delivery was tested by this wizard. Source and screenshot are reconciled accordingly in COM-UI-019.

**Local mail fixtures.** Separately from the UI walk, admin Tools services created EmailServer **COM_B06_SINK**
(`127.0.0.1:18873`, no TLS/authentication, no external host) and attached it only to the new fixture account.
A Python stdlib SMTP sink captured MIME/envelopes under `mail-capture-b06/`; one direct loopback probe verified it
before any UI send. This bypasses transport configuration as fixture preparation, not a passed setup UI.
User-requested curl transfer imported a three-message `.mbox` into Inbox **100000**: original agenda with a 33-byte
text attachment, its reply, and an independent note. Import reported **3 imported / 0 duplicates / 0 failed**;
repeating the identical file reported **0 imported / 3 duplicates / 0 failed**. Prepared imports are not inbound
mail delivery. All addresses end in `.invalid`; no network mail left the local sink.

**Inbox triage.** Inbox showed two conversations, the agenda with two messages. Expanded the earlier message,
read both bodies, opened its More → Print or view source, inspected sender/recipient/account/folder and Headers
and source. The attachment's curl download returned the expected 33 bytes. The visible EML/Print links instead
returned 404 through `/apps/Comms` (COM-UI-016); no corrected-route substitute is counted as a passed output.
Open in Inbox returned the thread in All mail. A no-match query showed No conversations match; Clear search
restored the inbox. Advanced Subject + Has attachment returned the agenda thread; saved `COM-B06 attachments`,
cleared search, then selected the saved search and recovered the same query/result. Marked the independent note
unread, filtered Unread, archived it (empty unread Inbox and Archive count), then Undo restored it. Marked it read
again and cleared the unread filter. No label/move/snooze, spam/block/unsubscribe, calendar or bulk-action success
claimed. Imported messages began read; the explicit mark-unread step supplied the unread fixture.

**Reply/draft/send.** Reply on the latest received agenda message selected the expected identity, recipient,
subject and quoted message. Typed a reply, popped out to a dialog, expanded quoted text and observed Saved.
Closing the dialog returned the same docked composer; closing that and opening Drafts restored the body,
recipient and quote. Send showed Sending… / Undo; immediate Undo restored the draft. Resending reached the
loopback sink once with `In-Reply-To: <com-b06-2@review.invalid>` and the expected reply plus quoted HTML.
The existing browser `sendDue` timer dispatched the undo-held message despite scheduled jobs being disabled;
this does not establish future scheduled delivery after closing the browser. The Sent thread contained three
messages. No duplicate click/lost-response idempotency or independent recipient reading was tested.

**New compose, recipients and recovery.** Subject/body survived missing-recipient refusal and a malformed
`invalid-recipient`; the latter error appeared twice in one notification (COM-UI-017). Correcting To allowed the
message to send. Opened Schedule send and canceled without scheduling. An initial fast Bcc entry followed by Enter
and opening Send options did not establish a visible committed chip; its subsequent captured message contained only
To. A similar fast To entry in the next draft did not remain. Treat this as an **unfinished recipient-entry/timing
branch**, not proof that a committed recipient was dropped: repeat with manual keyboard cadence, pending server
search/autosave and blur before identifying a root cause. No transport/privacy defect is inferred from those inputs.

A separate Bcc entry, inspected before and after Enter, produced an explicit `hidden@review.invalid` chip, retained
in the phone composer screenshot. With To empty, Send returned Add a recipient in To (COM-UI-018). Adding and
confirming `receiver@review.invalid` permitted send. Sent displayed both To and Bcc to the sender; captured SMTP
envelope contained both recipients and MIME contained **no Bcc header**. The sink held exactly **three application
messages** (reply, corrected new message, To+Bcc check), plus its one readiness probe. These are local SMTP acceptance
and content checks, not remote delivery/read confirmations. No outgoing attachment, Cc/Reply all/forward, Files
provider, scheduled-send execution, offline/conflict, failed SMTP or revoked-send branch exercised.

**Presentation/evidence.** The 390×844 composer kept sender/recipient chips, subject, body, toolbar and Send within
390px; the virtual keyboard/touch/native app was not available. Final Sent at 1920×1080 had readable folder/list/
message panes; both screenshots were visually inspected. Dark, Standard and TUI remain pending. Ignored evidence
`plans/ui-refinement-evidence/basic-comms/batch-06/` includes runtime metadata, `01-hosted-permission-refusal.txt`,
`02-hosted-test.txt`, `03-hosted-finish.txt`, `04-account-active.txt`, `05-import-{1,2}.json`, `06-thread.txt`,
`07-message-detail.txt`, `08-{eml,print,attachment}.response`, `09-reply-saved.txt`, `10-reply-reopened.txt`,
`11-undo-restored.txt`, `12-invalid-recipient.txt`, `13-unread-filter.txt`, `14-saved-search.txt`,
`15-capture-summary.json` and three captured `.eml` files, `16-sent-new-message.txt`, `17-phone-compose.{txt,jpg}`,
`18-bcc-only-refusal.txt`, `19-final-sent.{txt,jpg}`, `20-console.json` and cleanup metadata. The sampled console
contains HTTP-500 warnings matching invalid-address and Bcc-only refusals; it is not a comprehensive network audit.

**Proposal/verification handoff.** Added COM-UI-016–019; Comms-Messaging and the archived framework plan unchanged.
Preserve the built email flows when reorganizing the shared app, distinguish email from chat/network-room privacy
and delivery semantics, and retain current mount context for outputs. Existing `HostedMailAddress` asserts the
hosted permission's holder/non-holder and external-account distinction; `BasicCommsComposeRef` asserts save/send
client-ref retry behavior and undo/resend state. These source-read tests were **not run**, and do not prove this
browser walk's narrower grants, keyboard entry, Bcc-only path or output links. Future fixes need focused regression
checks of mount-derived links, error aggregation and recipient validation; extend future BasicCommsMailWalk acceptance
for complete provider, responsive, ordinary/restricted and capture workflows. No new regression page sweep.

**Cleanup/remaining.** Inbox/read/archive/filter changes restored; no drafts or held sends left in the observed
lists. Retained new hosted account 100102, its sink record, imported/sent mail, attachment and saved search only in
the disposable environment; no permission changes or user credential changes. Browser viewport reset, both temporary
tabs closed, empty browser inventory. Server SIGINT/session exit 0, SMTP sink SIGINT/exit 130 (expected Python
KeyboardInterrupt); verified server, embedded PostgreSQL and ports 18872/18873 absent. Removed curl cookie jars and
scratch operation file. Runtime fixtures/MIME and ignored evidence retained. COM-01–04/06 remain partial, with the
permission and external-provider prerequisites above; identity/rule/out-of-office and shared-mailbox workflows are
still pending. Review documentation remains uncommitted; no fixes.

### COM-B07 — Identities, rules, out-of-office and shared mailboxes, 2026-11-16

**Scope/runtime.** COM-05/06 plus COM-01/03/04/18 consumers, unchanged source
`63f84fea38c13c0c000bae0b26e5e96a12ce45ab`, retained `/tmp/mr-ui-four-apps-zf5098dh`, same six individually linked
components/no addons and binary SHA-256 `a83c6f287aa9a12b9feb1b9b23dece06813e254e91bd21f19a209307cfc54e2f`.
Initial PID 43960 was stopped cleanly to enable a temporary inbound SMTP fixture; PID **44113** started
2026-11-17 02:02 UTC (Nov 16, 20:02 America/Chicago). HTTP 18872, incoming SMTP 18874 and capture sink 18873 were
loopback-only; SMTP allowed only 127.0.0.1, outgoing mail hosts restricted to localhost. Jobs and IMAP IDLE remained
disabled. Existing administrator John Doe owned account 100102; ordinary `calls.client.b` and `calls.client.c`
(BASIC_COMMS_USERS) used separate hostnames. Only synthetic mailbox membership changed, no global grants or
credentials. Material light at default 1280×720; no dark, 1920/390, Standard, TUI or physical-device pass this batch.
No builds, tests, external provider connections, real mail or application changes.

**Identity → compose.** Added `COM B07 Alias <alias-b07@review.invalid>` with reply-to
`reply-b07@review.invalid`, a distinct signature, above-quote placement and replies/forwards enabled; made it default.
New Compose selected that alias and inserted the signature. Sent **COM-B07 alias identity** to the local capture;
MIME had the selected From, Reply-To and signature. This proves the new-compose consumer, not actual provider alias
permission or reply/forward placement. Original identity/default restored after the walk; alias retained nondefault.

**Rule preview → incoming mail → run.** Created **COM-B07 flag review sender**, account 100102, From contains
`sender@review.invalid`, Flag action. Before saving, Test showed **3 of the newest 3** match and explicitly did
nothing. Saved enabled. Actual loopback SMTP delivery of **COM-B07 automatic rule check** created message 100052
and flagged it automatically. Run now on Inbox/all enabled rules reported **4 matched / 4 actions**; the rule then
showed **matched 5 times** (one automatic plus four manual). Edited the condition to a nonmatching address, Test
reported **0 of the newest 4**, canceled; subsequent rule list retained the original condition. No second rule,
reorder/stop interaction, invalid action, failed action, block/unblock or permanent deletion exercised. Turned the
rule off at cleanup and bulk-cleared its flags; Flagged showed Nothing flagged.

**Automatic replies and audience.** Enabling with an empty body refused with “Write the reply people get,” retaining
the form. Saved distinct public/internal bodies, no start/end dates, four-day repeat interval and **My contacts and
people inside the organization**. Screen showed On now. Inbound SMTP from `outsider-b07@outside.invalid` produced
a captured public reply despite no contacts integration (COM-UI-020); repeating that sender produced no second
reply. A sender at `review.invalid` produced the distinct internal body. A separate external message carrying
`Auto-Submitted: auto-generated` was suppressed, with that reason in the log. Changed audience to **Only people
inside the organization**; a new outsider was suppressed with explicit not-internal log reason. Turn off returned
Off and retained content. Captured replies carried Auto-Submitted: auto-replied, In-Reply-To and the default alias/
Reply-To. This was the actual incoming-delivery path, not a direct invocation of the auto-reply service. All addresses
were synthetic and all outgoing bytes stayed in the local sink. Date scheduling, every other loop guard, contacts
provider success/failure, repeat expiry/re-enable, multiple accounts and transport failure remain pending.

**Shared mailbox, ordinary members and stale rights.** Owner changed this disposable hosted account to Shared,
added B with Read and C with Read and send. Both saw its populated Inbox. B could open/read a message and its
unread count fell; the reading pane explicitly explained no-send access. B's active Archive control nevertheless
refused at submission (COM-UI-021). Account details showed read-only access and Leave, with management controls
absent. C sent **COM-B07 shared member send**, captured with the shared alias/signature. C then saved
**COM-B07 revoked send draft** to `must-not-send@outside.invalid`. Owner downgraded C to Read; C's still-open
composer retained the selected alias, but Send refused “Choose a mail account to send from.” Subject/recipient/body
remained. No message to that recipient reached capture.

B canceled Leave once, then confirmed; account list became empty with add-account guidance. Reopening the former
Account URL reported **Email account 100102 not found**. Owner list now contained only C. Owner confirmed removal
of C; owner page said Nobody shares it yet, and C's refreshed Inbox showed no account/mail. Owner still saw the
saved revoked-send draft **100061**. These checks establish scoped read/send/refusal and member removal, not
manager/last-owner transfer, complete stale-helper/attachment isolation or removing the account itself. Account
100102 remains Shared; its original Hosted kind was not restored through the UI, which now offers Personal/Shared.

**Capture, evidence and verification handoff.** Sink contains exactly **four B07 messages**: alias compose,
shared-member compose and two automatic replies (one external public, one internal). Earlier B06 captures are
separate; no revoked draft, repeated-sender, automated-sender or internal-only outsider reply appeared. Source-read
`BasicComms` feature **out of office retains user-zone date strings and supplies instants to the static form** checks
date/zone mapping and disabled-form output, not actual audience/delivery. `BasicCommsSync` feature **a caught-up
cursor answers reset when its user is added to an account, and again when removed** checks membership cursor reset,
not action gating or stale-composer explanations. Neither test was run. Later fixes need focused regressions for
no-hook/empty/failing contacts classification and capability/error contracts; complete policy/shared mailbox walks
belong in acceptance and run only when requested or when those tests are added/changed.

Ignored `plans/ui-refinement-evidence/basic-comms/batch-07/` contains runtime metadata;
`01-identity-saved.txt`, `02-rule-test.txt`, `03-inbound-rule.json`, `04-rule-run.txt`,
`05-rule-unsaved-test.txt`, `06-ooo-outsider.json`, `07-ooo-repeat.json`, `08-ooo-internal.json`,
`09-ooo-auto.json`, `10-ooo-contacts-enabled.{txt,jpg}`, `11-capture-summary.json` and four captured MIME files,
`12-internal-only-probe.json`, `13-ooo-off.txt`, `14-shared-members.txt`, `15-readonly-archive-refusal.txt`,
`16-readonly-message.txt`, `17-revoked-send.txt`, `18-left-account.txt`, `19-left-account-deep-link.txt`,
`20-member-removed.txt`, `21-members-cleaned.txt`, `22-rule-off.txt`, `23-identity-restored.txt`,
`24-flags-cleared.txt`, `25-retained-draft.txt`, `26-final-ooo-off.{txt,jpg}`, `27-mail-log.txt` and cleanup metadata.
Contacts-audience and final Off screenshots visually inspected. No comprehensive console/network clean-bill claim.

**Findings/proposal/cleanup.** Added COM-UI-020–022. Comms-Messaging changes are recommendations in this register;
that proposal and archived build record remain unchanged. COM-05/06 and consumer activities stay partial with the
branches above, TextBlocks and alternate modes pending. Original default restored, rule/OOO disabled, flags cleared,
B/C membership removed. Retained alias, disabled rule/OOO configuration (internal-only audience), Shared account,
new incoming/sent fixtures and one unsent draft in the disposable database; no broad permission changes. Closed all
three temporary tabs and verified empty browser inventory. Runtime SIGINT/session exit 0; sink SIGINT/exit 130
(expected Python KeyboardInterrupt). Verified server/scratch database processes and ports 18872/18873/18874 absent.
Restored scratch configuration to remove the temporary incoming SMTP listener. No fixes, commit or test execution;
review documentation remains uncommitted.

## 7. Decisions

- IDs are `COM`. The 54 app XML definitions plus CallGuest belong here; 13 System Voice/ChatBridges definitions
  belong to the AI/integrations plan while retaining basic-comms ownership for any later correction.
- Ordinary participant/account ownership is the daily-work baseline; administrator-only success is not sufficient.
- Conditional desktop/media/provider branches stay visible in the ledger. Unsupported terminal operations must
  explain delegation, not pretend to complete in a rendered placeholder.

- Preparing or auditing these plans changes documentation only. Application/API changes, test runs, runtime walks
  and fixture creation belong to separate review/refinement execution.
- Current code, mounts and grants take precedence over older plans. Historical outcomes, permissions and exceptions
  are not inherited. Preserve routes, stored preferences and public contracts in any later refinement.
- Finish a review with explicit branch outcomes and a candid unresolved ledger; missing prerequisites do not become
  passes. Building absent features is outside this review.
