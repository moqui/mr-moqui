# 12. Mobile

Part of [Agent Code](README.md). T3 sources: `t3:apps/mobile/**` (Expo / React Native), `t3:packages/client-runtime`,
`t3:packages/contracts/src/relay.ts`, `t3:docs/internals/{mobile-navigation,mobile-layout,connection-runtime}.md`,
`t3:docs/user/{remote-access,mobile-notifications}.md`. MR Moqui sources: `crates/moqui-mobile`,
[docs/ui/Mobile-Client.md](../../docs/ui/Mobile-Client.md), [plans/Mobile-Desktop.md](../Mobile-Desktop.md).

## 1. Approach

| T3 | MR Moqui |
|---|---|
| A second UI in React Native with 13 native modules | No second UI. The phone shows the server's `/qapps/code` pages in the app webview (MR Moqui rule: one UI in Vue and Quasar). Phone layout rules are in the pages ([10 §10](10-Web-UI.md#10-responsive-layout)). |
| iOS 18+ and Android API 24+ | Android first (the existing Tauri Android app, `crates/moqui-mobile`). iOS follows the same plan when MD7 is built (phase 6, [16](16-Phases.md)). |
| Shared client runtime (`packages/client-runtime`) | `crates/moqui-client` (accounts, keys, sockets, store, outbox) plus `AgentCode.js` in the page. |
| Native modules where the webview is weak | A short list of native pieces where the webview cannot do the job (§4.4). Everything else is web. |
| Phone has no local server | Same. Phones are clients only. |

Why webview and not native widgets: the qvt client already runs on the phone, the terminal already has a phone key
bar, and one set of pages keeps web, desktop and phone the same. The cost is less native feel. §4 lists where the
plan adds native parts to pay that back.

## 2. Connection model

### 2.1 Environments on the phone

- Each app account is an environment (server plus user plus device key in the Android Keystore).
- **Add environment**: scan a pairing QR (desktop Welcome page, `moqui pair`, Settings > Access), open a pairing
  link, or sign in with a password or SSO (existing).
- Linked environments show through the account's server peer links
  ([13 §6](13-Servers-Linking.md#6-peer-links)).
- Accounts marked **Show in Agent Code** stay connected in the foreground at the same time
  ([11 §3](11-Desktop.md#3-sockets-and-environments)). The phone shows them in one sidebar.

### 2.2 Routes per machine

| T3 route | MR Moqui |
|---|---|
| LAN | A LAN address the server advertises in the descriptor (`endpoints`, [13 §1](13-Servers-Linking.md#1-environment)). |
| Tailscale, other VPN | A Tailscale name or VPN address from the descriptor. |
| Public URL | The server's web address or its edge node host name. |
| SSH | Not on the phone (T3 has it only for desktop launch). |
| T3 Connect relay | The edge node ([13 §8](13-Servers-Linking.md#8-relay-and-tailscale)). |

- Routes are ordered by speed. The user can reorder them in Settings > Environments > *env* > Routes.
- The app learns LAN and Tailscale addresses over a working route and adds them.
- It connects over the first route that answers. It moves back to an earlier route when that route works again
  (checked on network change, foreground and every minute while in the foreground).
- A route is pinned to the environment id and the server's TLS key (or the edge pin). A LAN route with a self-signed
  certificate is allowed only when its key matches the one seen on a trusted route (trust on first use through the
  trusted route, never on the LAN).
- Code: `crates/moqui-client/src/code/routes.rs`. Stored in the account record.

### 2.3 Supervisor

- One connection owner per environment (`crates/moqui-client/src/code/supervisor.rs`).
- Jittered exponential backoff, cap 5 minutes.
- Foreground, network change, retry or offline report: probe the session first. A probe with no answer in 500 ms
  starts a fresh descriptor request. Deadlines 3 s on loopback or LAN or after a long suspension, 15 s elsewhere.
- An involuntary disconnect keeps the registration and the cached data. **Remove environment** clears the key, the
  cache and the drafts.
- The `/agentws` socket is held by the host and bridged to the page, as `/termws`
  ([09 §2](09-Wire-API.md#2-agentws)).

## 3. Screens

The phone uses the same XML screens and `.qvue` files as the web. The table maps each T3 mobile screen to the
MR Moqui page or sheet. "Sheet" means a Quasar bottom sheet or a full-height dialog on a phone width.

| T3 area | T3 screens | MR Moqui on the phone |
|---|---|---|
| Home | `Home` | `/qapps/code`: on a phone width the sidebar is the root page (sections Pinned, active, Working, Snoozed, Settled), with **New** as the floating action. |
| Threads | `Thread`, `ThreadInspectorFullscreen`, `ThreadSettingsSheet`, `ThreadQueue`, `ThreadBackgroundWork`, `ThreadAgents`, `ThreadGuestShare`, `ThreadMcpApp`, `ThreadAttachment`, `LinkedThread`, `LinkedThreadFullscreen` | `/qapps/code/t/{id}` with the right-panel tabs as sheets: details, queue, background work, subagents, guest share; MCP App and attachment open full screen. Linked threads at `/qapps/code/linked/...`. |
| New task | `NewTaskProject`, `NewTaskEnvironment`, `NewTaskBranch`, `NewTaskRepositories`, `NewTaskFiles`, `NewTaskFile`, `NewTaskAttachment`, `NewTaskThreadSettings` | `/qapps/code/new` with pickers as sheets (project, environment, branch, repositories, files, settings). |
| Add project | `AddProject*` (repository, destination, local, new) | `/qapps/code/p/new` steps (clone, local folder on the server, new). |
| Thread tools | `ThreadTerminal`, `ThreadDevicePreview`, `ThreadBrowserPreview`, `ThreadLocalPreview`, `ThreadReview`, `ThreadReviewComment`, `ThreadFiles`, `ThreadFile` | Terminal page with the existing phone key bar; device and browser preview by server-run screencast ([06](06-Terminal-Preview-Devices.md)); review (diff) page; files tree and file view. |
| Git | `GitOverview`, `GitCommit`, `GitBranches`, `GitConfirm`, `ThreadPullRequests` | Git sheet of the thread: status, commit, branches, confirm, PRs. |
| Pull requests | `PullRequest`, `PullRequestFiles`, `PullRequestFile`, `PullRequestGuideStop`, `PullRequestReviewers`, `PullRequestLabels`, `PullRequestFilterSearch`, `PullRequestDiscussion`, `PullRequestInlineComment`, `PullRequestReview`, `PullRequestEdit` | `/qapps/code/PullRequests` list and detail; each T3 sub-screen is a sheet or a page in the detail. |
| Issues | `Issue`, `IssueEdit`, `IssueLabels`, `IssueAssignees`, `IssueFilterSearch` | `/qapps/code/Issues` list and detail with sheets. |
| Connections | `Connections`, `ConnectionsNew`, `ConnectOnboarding` | App shell pages (bundled, they work with no server): environment list, add (scan QR, link, sign-in). Edge onboarding is a server page. |
| Settings | `Settings*` (about 40) | `/qapps/code/Settings/*` ([10 §9.4](10-Web-UI.md#94-settings-pages)) plus app-only pages in the shell: Notifications, Client storage, Diagnostics, Voice microphones, Language (the phone's), Keyboard (hardware keyboard). |
| Other | `NotFound`, dev component catalog | Shell not-found page. No catalog. |

### 3.1 Navigation

- T3 has two intents: **select** (show a thread or PR as the only page) and **drill in** (push over the page). MR
  Moqui keeps them in `AgentCode.js`: `moqui.code.select(link)` replaces the page history to the sidebar plus one
  page; `moqui.code.drill(link)` pushes.
- Rows, palette, compose, notifications and deep links select. Buttons inside a page drill.
- Android back (predictive back) pops a sheet first, then a drilled page, then to the sidebar.

### 3.2 Width classes

| T3 class | Width | MR Moqui layout |
|---|---|---|
| Compact | < 600 | Sidebar is the root page; pages push; right panel tabs are sheets. |
| Medium | 600-720 | Sidebar beside the thread, floats over it when the thread is open. |
| Expanded | wide | Sidebar, thread and right panel columns (as the web at ≥ 1024). |

- The width class comes from the page width, not the device kind. A foldable changes class on unfold.

## 4. Composer and native parts

### 4.1 Composer

- The web composer ([10 §5.5](10-Web-UI.md#55-composer)) with phone rules: Return adds a line on the on-screen
  keyboard; the send button sends. While a turn runs the send button shows queue or steer
  (`followUpBehavior`); long-press gives the other one.
- Hardware keyboard: Return sends, Shift+Return new line, Ctrl+Return the other follow-up action. `Ctrl+1`-`9`,
  `Ctrl+K`, `Ctrl+Shift+H` as iPad `Cmd` keys.
- `/` commands and `$` skills also on **New task**.
- Side chats and queued messages show above the composer.

### 4.2 Attachments

- Photo library (HEIC to JPEG on the device), camera, files. The webview file input gives library and files. Camera
  uses the existing camera permission path.
- Uploads go in chunks (`uploadChunk`, existing client sync contract) so a large photo survives a network change.

### 4.3 Voice

- Order of engines ([07 §4](07-Secrets-Guest-Voice-Usage.md#4-voice-input)): on-device Android speech recognizer
  (Android 12+ with Google speech services) by a small Kotlin `SpeechService.kt`; else the environment's speech
  service (`SpeechServices`); else a cloud provider set by the admin.
- Max 5 minutes per recording. Microphone choice list.

### 4.4 Native parts

| Need | T3 module | MR Moqui |
|---|---|---|
| Ongoing and Live Update notifications | `t3-agent-notifications` | `AgentNotify.kt` (§5.2). |
| Home screen widgets | `t3-subscription-widget`, `t3-widget-expiry` | `UsageWidget.kt`, `ActivityWidget.kt` with Jetpack Glance (§5.4). |
| On-device speech | `t3-voice-transcription` | `SpeechService.kt` (§4.3). |
| Share in | expo-sharing | `ShareActivity.kt` (§7.1). |
| Quick actions | expo-quick-actions | Android static and dynamic shortcuts in `MainActivity.kt` (§7.3). |
| Terminal | `t3-terminal` (libghostty) | The existing web terminal with the phone key bar. A native renderer is open decision O8. |
| Rich composer | `t3-composer-editor` | The web editor; no native module. |
| Diff, source text, syntax highlight | `t3-review-diff`, `t3-source-text`, `t3-highlights` | Web (worker diff, highlight.js). Virtual scroll keeps big diffs fast. |
| WebSocket | `t3-web-socket` (iOS) | Host-held socket (existing). |
| Preview tunnel | `t3-preview-tunnel` | Not needed: phone previews are server-run screencasts. |
| DPoP keys | `T3DpopKeys.swift` | Android Keystore key (existing `KeyVault.kt`) signs DPoP proofs when the environment asks for them ([13 §3](13-Servers-Linking.md#3-pairing-and-sessions)). |

## 5. Push, Live Updates and widgets

### 5.1 Push

| T3 | MR Moqui |
|---|---|
| Push needs T3 Connect (the relay sends APNs and FCM) | The server sends push directly: FCM (exists, `web_push.rs` `send_native`) and APNs (new `crates/moqui-core/src/service/apns.rs`, token auth with a `.p8` key in sealed settings). No relay. |
| Device registration with preferences | Existing `register#PushDevice` (`WebPushSubscription`, `PushFcm`) plus a new field `agentPrefs` (JSON): `liveUpdatesEnabled`, `notificationsEnabled`, `notifyOnApproval`, `notifyOnInput`, `notifyOnCompletion`, `notifyOnFailure`. |
| Agent awareness phases `starting`, `running`, `waiting_for_approval`, `waiting_for_input`, `completed`, `failed`, `stale` | Same names, computed from the run and the open approval or question ([02 §4](02-Orchestration.md#4-messages-dispatch-modes-and-the-queue)). |
| Server publishes activity with a signed proof to the relay | The server's notification service sends to the user's own registered devices. No proof needed: the server holds the push credentials. |
| Alerts quiet in the foreground | Same: the app drops the push when the thread is on screen. |

- Payload rule (existing): a push carries title, link and topic, never the message body. For Agent Code it carries
  `kind` (`agent`), `phase`, `environmentId`, `threadId` and the thread title. A server setting
  `pushIncludeTitles` (default true) can drop the title.
- Topics: the four Agent Code topics of [07 §2](07-Secrets-Guest-Voice-Usage.md#2-notifications). The user's
  `NotificationTopicUser` rows and `agentPrefs` both must allow a push.
- A self-hosted server without FCM set up: the app has no background push. It shows a banner once in Settings >
  Notifications with the reason. Through an edge node: the edge node can hold the FCM and APNs credentials and send for
  the local node (`EdgeQueueItem` kind `push`), so a home server behind NAT still pushes.

### 5.2 Android notifications

- Click opens the thread. Today a click opens the app, not the link ([docs/ui/Mobile-Client.md](../../docs/ui/Mobile-Client.md)).
  Agent Code fixes that for all notifications: the intent carries the link and `MainActivity` selects it (§3.1).
- Grouped per environment. One notification per thread, updated in place (tag `code:<threadId>`).
- **Ongoing Agent Activity** (Android 7-15): one ongoing notification with the count of running threads and the
  newest phase; tap opens the sidebar Working section.
- **Agent Live Updates** (Android 16+): a progress-centric notification per running thread (`Notification.ProgressStyle`
  with `setRequestPromotedOngoing`), with the phase and elapsed time. It ends with completed or failed and goes away
  after 15 minutes.
- Actions: **Open**; for `waiting_for_input` an inline **Reply** (RemoteInput) that sends a follow-up message through
  the outbox; for `waiting_for_approval` **Review** only (opens the approval card). No approve action from the
  notification: an approval needs the person's session in the app ([14 §4](14-Security.md#4-approvals)).
- Channels: Agent activity (low), Needs you (high), Finished (default), Guest share (default).

### 5.3 iOS (with MD7)

- APNs alerts with the same payload.
- Live Activity `AgentActivity` with push-to-start tokens (`pushToStartToken` stored in `WebPushSubscription.agentPrefs`),
  updated by APNs `liveactivity` pushes from `apns.rs`; finished results stay 15 minutes.
- The widget and Live Activity need a small Swift WidgetKit extension. This is the one place iOS needs Swift UI code.

### 5.4 Widgets

| Widget | T3 | MR Moqui (Android, Glance) |
|---|---|---|
| SubscriptionUsage | iOS and Android; Codex and Claude periods | `UsageWidget`: the usage limits of [07 §5](07-Secrets-Guest-Voice-Usage.md#5-usage) for the chosen environment; resizable 2x2 to 4x3. |
| AgentActivity | iOS only | `ActivityWidget` on Android too: running and waiting threads, tap opens a thread. |

- Data: the app writes a small snapshot (counts, top 5 threads, usage bars) to the widget store on each shell stream
  update while in the foreground, and on each agent push.
- Unfinished work older than 10 minutes with no update shows as stale.
- No credentials in the widget store.

## 6. Offline

- Store: the existing per-account SQLite store (`client/data/<account>.db`).
- Kept for offline reading:
  - the shell snapshot (sidebar rows, counts) as structured rows;
  - the last 20 opened threads, each with its last 200 turn items and the plan;
  - the PR list and the last 20 opened PRs (summary, checks, comments; not full diffs);
  - drafts per thread and the prompt stash.
- Cache budget: Agent Code uses up to a quarter of `store-max-mb`; oldest threads go first.
- Outbox kind `agent-message` (new): `{environmentId, threadId, commandId, text, attachments, mode}` where mode is send,
  queue or steer. Limits: 50 waiting (the outbox limit), 64 KB text each; attachments upload first in chunks.
  The `commandId` receipt makes replay safe ([02 §1](02-Orchestration.md#1-the-command-pipeline)).
- Outbox kind `agent-command` (new): settle, snooze, pin, rename, archive. Not approvals, not secrets, not interrupt.
- Offline UI: a chip **Offline, shows saved data** on the page; sending shows the message as **Waiting to send**.
- Android background: the existing `FlushWorker` flushes `agent-message` and `agent-command` with the other kinds.
- A message whose thread was deleted or archived while offline comes back as failed with the reason; the draft stays.
- The same store and outbox serve the desktop in client mode.

## 7. Share, deep links and quick actions

### 7.1 Share in

- Android share intents for text, one URL, and up to 8 images, 8 videos and 8 files (`*/*`, single and multiple).
- Shared items land in an inbox and open **New task** with the items as attachments and the text in the composer.
  The user picks the environment and project.
- Files are copied into the app's own storage first; only files under the app's roots are read.

### 7.2 Deep links

| Link | Opens |
|---|---|
| `org.moqui.mobile:/code/t/{environmentId}/{threadId}` | A thread (notifications, widgets). |
| `org.moqui.mobile:/code/pair#token=...` | Pairing (from a QR). |
| `https://<server>/qapps/code/t/{threadId}` | Android App Links when the server is an account (later, with the App Links plan). |

### 7.3 Quick actions

- Launcher shortcuts: **New task**, **Waiting for you**, and up to 3 pinned threads (dynamic).

## 8. Tablet and foldables

- Rotation: full rotation when the smallest width is 600 dp or more, else portrait (T3 rule).
- Hinge-aware columns: the page gets the hinge rectangle from the host (`WindowLayoutInfo`) and keeps columns off
  the hinge.
- Hardware keyboard keys of §4.1.
- Android split screen: drag files into the composer.

## 9. Updates

- No OTA code updates: Agent Code pages come from the server, so they update with the server.
- The app shell and native parts update through the store or the APK (existing `./moq android build --release`).
- Version check: the descriptor `protocolVersion` ([09 §7](09-Wire-API.md#7-versioning)); the app shows **Update
  the app** when the server needs newer native parts (for example a new outbox kind).

## 10. Tests

See [15](15-Tests.md). Mobile specifics:

- Rust tests in `crates/moqui-client/src/code/`: routes order and fallback, supervisor backoff and deadlines, outbox
  `agent-message` replay with receipts, cache budget.
- Kotlin pieces: one instrumented test each for notification build and widget snapshot read; run only in the Android
  acceptance job.
- Phone layout: the web page tests at 390 px width cover the compact class (one render per screen and mode, as the
  rendering rule).
