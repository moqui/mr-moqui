# MR Moqui — UI review: BasicComms

**Status — 2026-11-16:** planned; every activity pending. Source inspection only; no review or test results claimed.

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
[Comms build record](archive/Comms-Framework.md), current BasicComms screens/QVue/twins/templates, setup data and
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
| MessageDetail body/attachments/resend/downloadEml and Mail/Compose TUI helpers | COM-03/04/18; actual bytes and truthful terminal content/edit limits |
| MailAccounts wizard, sign-in dialog/status, sync progress, Account server/options/folder/member forms, Removed list | COM-02/06; hosted delivery versus remote sync distinctions |
| Identities, Rules QVue/test/reorder/run and RulesTui, OutOfOffice and TextBlocks | COM-05; stored user policy and delayed job behavior |
| Chat QVue room/thread/message menus, mentions/reactions/pins/polls/search/forward/files, membership/bridge/encryption settings | COM-07–09; interactive states beyond a single XML route |
| [ChatPanel.qvue](../runtime/components/base/basic-comms/screen/BasicComms/Panel/ChatPanel.qvue), served by Panel/Chat: unread/mention ordering, New message people picker, Mark all read, compact room, Back/Open in Chat | COM-07; inspect tray and quick-create mounts, reopen persistence and push refresh without losing the open room |
| Devices QVue/TUI, recent-auth dialog, key backup/settings, ChatApps/XMPP app password | COM-09/10; no key/secret evidence leaks |
| Chat preferences, DND hours/presets/default application and unread notifications | COM-10; date-sensitive job and per-room settings |
| Calls root signaling transitions and Call QVue/TUI, ring bar, hold/transfer/add/merge, captions and consent | COM-11/12/16; actual supported controls/capabilities only |
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
| COM-01 — Start from the right inbox and account | pending | All steps, modes and recovery branches pending. | — |
| COM-02 — Set up and verify a mail account | pending | All steps, modes and recovery branches pending. | — |
| COM-03 — Triage a thread and inspect its original evidence | pending | All steps, modes and recovery branches pending. | — |
| COM-04 — Compose, recover and send a message once | pending | All steps, modes and recovery branches pending. | — |
| COM-05 — Make personal mail policy work for its consumer | pending | All steps, modes and recovery branches pending. | — |
| COM-06 — Manage a shared mailbox without losing ownership | pending | All steps, modes and recovery branches pending. | — |
| COM-07 — Carry a conversation across rooms and threads | pending | All steps, modes and recovery branches pending. | — |
| COM-08 — Share files and maintain room membership or bridge context | pending | All steps, modes and recovery branches pending. | — |
| COM-09 — Recover encrypted chat and device trust | pending | All steps, modes and recovery branches pending. | — |
| COM-10 — Set chat availability and external app access | pending | All steps, modes and recovery branches pending. | — |
| COM-11 — Answer and manage a direct media call | pending | All steps, modes and recovery branches pending. | — |
| COM-12 — Dial and transfer using the selected identity | pending | All steps, modes and recovery branches pending. | — |
| COM-13 — Invite and admit a bounded guest | pending | All steps, modes and recovery branches pending. | — |
| COM-14 — Configure a phone and call handling then observe routing | pending | All steps, modes and recovery branches pending. | — |
| COM-15 — Work a queue and resolve voicemail | pending | All steps, modes and recovery branches pending. | — |
| COM-16 — Record with consent and inspect retained media | pending | All steps, modes and recovery branches pending. | — |
| COM-17 — Recover host-only queued work and conflicts | pending | All steps, modes and recovery branches pending. | — |
| COM-18 — Reconcile private content, helpers and mode limits | pending | All steps, modes and recovery branches pending. | — |

### Findings register

Use `COM-UI-001` onward. P1 blocks work or essential understanding; P2 causes substantial friction or misleading
presentation; P3 is a localized refinement. Label evidence as observed behavior, static suspicion or design judgment.

| ID / severity | Actor, activity step, route, mode / viewport | Reproduction and impact | Evidence / owning layer | State / correction / retest |
| --- | --- | --- | --- | --- |
| — | — | No findings recorded. | — | — |

### Bounded batch record

For each later batch record:

- Intent, activities and branch limits; runtime/build/components, identities/grants, fixture IDs and dates.
- Observed steps and outcomes; evidence filenames, console/network/output observations and findings.
- Workarounds and blocked/deferred branches, with prerequisites and follow-up.
- In refinement only: decisions, owning files/components, correction, focused verification commands/results and retest.
- Temporary settings/grants/files/jobs restored, remaining cleanup and companion handoffs.

No execution batches have been started.

## 7. Decisions

- IDs are `COM`. The 54 app XML definitions plus CallGuest belong here; 13 System Voice/ChatBridges definitions
  belong to the AI/integrations plan while retaining basic-comms ownership for any later correction.
- Ordinary participant/account ownership is the daily-work baseline; administrator-only success is not sufficient.
- Conditional desktop/media/provider branches stay visible in the ledger. Unsupported terminal operations must
  explain delegation, not pretend to complete in a rendered placeholder.

- This is a planning record only. No application/API change, test run, runtime walk, fixture creation or commit is
  part of the document-writing task. Review/refinement execution is separate.
- Current code, mounts and grants take precedence over older plans. Historical outcomes, permissions and exceptions
  are not inherited. Preserve routes, stored preferences and public contracts in any later refinement.
- Finish a review with explicit branch outcomes and a candid unresolved ledger; missing prerequisites do not become
  passes. Building absent features is outside this review.
