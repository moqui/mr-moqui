# 16. Phases and open decisions

Part of [Agent Code](README.md).

## 1. Rules for every phase

- Every phase ships on all four surfaces at once: web, desktop (in-process node and client mode), Android, and
  servers (single node and cluster). A feature is not done when it works only in the browser.
- A phase is done when:
  - its regression tests for the rules it added pass ([15](15-Tests.md));
  - `./moq test-framework` passes within the `agent-code` budget;
  - `moqui lint --screens` is clean;
  - the attack-surface rows for its routes, processes and tools are in `docs/security/` ([14 §8](14-Security.md#8-attack-surface-docs));
  - the user docs for it are in `docs/ai/Agent-Code*.md`;
  - the descriptor capabilities for it answer true ([09 §6](09-Wire-API.md#6-environment-descriptor)).
- The acceptance tests named in a phase are written in that phase. Running them is not part of Done.
- Failure tolerance in every loop over independent items (instances, projects, worktrees, webhooks, links, device
  hosts): log an error with the item's name and go on.
- Every T3 limit is kept unless the plan names another value and the reason.

## 2. Phases

| Phase | Scope | Usable when | Acceptance tests to write |
|---|---|---|---|
| **P0 Base** | `AgentConversation`'s surface services from the `external-conversation-support` branch (ported in PR #213: `ConversationServices.xml`, `moqui-llm` `conversation.rs`). Component `base/agent-code` and crate `moqui-code` skeletons. All entities, fields and enums of [01](01-Data-Mapping.md). Security seed ([14 §1](14-Security.md#1-permissions-and-groups)). Settings schema and services ([10 §9](10-Web-UI.md#9-settings)). Command pipeline, receipts, change feed, effects ([02 §1](02-Orchestration.md#1-the-command-pipeline)). `/agentws` with hello, frames, scopes and limits; descriptor ([09](09-Wire-API.md)). The REST resources and MCP tools refuse while the `/agentws` switch is off ([14 §1](14-Security.md#1-permissions-and-groups)). `/agentws` in `moqui-client` `PATHS`. Fix SEC-AI-12 and SEC-AI-01 in the shared code. | A test client opens `/agentws` on web, desktop client mode and Android, and gets the hello frame and an empty shell stream. | none |
| **P1 Threads** | Engine seam; native engine threads; process host and the `agent-code` conf element ([10 §9.5](10-Web-UI.md#95-conf-settings)); Codex and Claude drivers ([03](03-Providers.md)); provider instances and per-user sign-in. Workbench: sidebar sections, thread view, timeline rows, composer (plain and rich), model, effort and mode pickers, approvals, questions, plans, queue and steer, interrupt and resume ([10 §4-5](10-Web-UI.md#4-shell-and-sidebar)). Settle, snooze, pin, unread, search. Restart continuation. TUI Code view. Desktop: tray count, notifications, sleep inhibitor ([11 §7](11-Desktop.md#7-power-and-sleep)). Android: host-held socket, phone layout. | A user starts a Codex or Claude thread from a phone, approves a command on the desktop, and reads the answer in the browser and the TUI. | `AgentCodeScenario` (part 1), `AgentCodeHarnessReal` |
| **P2 Projects and work roots** | Projects, members, folders, scripts. Worktrees, scratch folders, cleanup. Checkpoints, rollback, diff pane, files viewed. VCS status, stacked actions ([04 §4](04-Git.md#4-vcs)). Thread terminals and the terminal drawer ([06 §1](06-Terminal-Preview-Devices.md#1-terminal)). Landlock (if O7 says so). Desktop open-in-editor, file drag, folder picker. | A thread works in its own worktree, shows its turn diff, rolls back a turn, and opens a terminal in the work root. | `AgentCodeAccess` |
| **P3 Code hosts** | Host accounts; GitHub, GitLab, MR Moqui hosting first, then the others ([04 §1](04-Git.md#1-code-hosts)). PR create, review, merge; PR pages ([10 §6](10-Web-UI.md#6-diff-and-pull-request-pages)); checks; PR watch; stacks; issues; review guides; triage. | A thread opens a PR, waits for checks, wakes on a failed check, fixes it and merges. | `AgentCodeScenario` (part 2) |
| **P4 More agents** | The other drivers (ACP registry, OpenCode, Cursor, Pi, Grok, and the rest of [03 §5](03-Providers.md#5-per-driver-plan)). Subagents, delegated tasks, side chats, fork, provider switch, handoff ([02 §8-10](02-Orchestration.md#8-fork-merge-back-provider-switch-handoff)). The full MCP tool set and aliases; outside agents by OAuth ([08](08-MCP-Tools.md)). Text generation features. Usage-limit continuation. | A Claude thread delegates a task to a Codex child and reads its result; an outside MCP client starts a thread with an approval code. | none |
| **P5 Automation and attention** | Scheduled tasks, intervals, once, webhooks ([05](05-Automation.md)). Notifications and topics ([07 §2](07-Secrets-Guest-Voice-Usage.md#2-notifications)). Push with agent preferences; Android click to link, Live Updates, ongoing activity, reply action, widgets, quick actions, share in ([12 §5](12-Mobile.md#5-push-live-updates-and-widgets), [§7](12-Mobile.md#7-share-deep-links-and-quick-actions)). Offline cache and outbox kinds `agent-message`, `agent-command` ([12 §6](12-Mobile.md#6-offline)). | A webhook starts a thread while the user is away; the phone gets a push, the user replies from the notification, and an offline follow-up flushes when the network comes back. | `AgentCodeEdge` (webhook part), Android installed-app walk |
| **P6 Servers and links** | Pairing codes and links, QR, token exchange, DPoP, device approval ([13 §3](13-Servers-Linking.md#3-pairing-and-sessions)), designed first on master's one scope per login key, declared by the REST resources ([14 §2](14-Security.md#2-scopes)). Client connection runtime: routes, learned routes, supervisor ([13 §4](13-Servers-Linking.md#4-client-connection-runtime)). Several environments in one sidebar. Peer links with fencing and forwarding ([13 §6](13-Servers-Linking.md#6-peer-links)). SSH launch ([13 §7](13-Servers-Linking.md#7-ssh-launch)). Tailscale. Edge queue kinds. Background service and `moqui update`. `moqui code` CLI. WSL node. Cluster forwarding ([13 §9](13-Servers-Linking.md#9-cluster-and-edge)). | A desktop launches a server on a remote host over SSH, a phone pairs to it by QR over Tailscale, and a thread on one server delegates to a linked server. | `AgentCodeLinks`, `AgentCodeEdge` (push part), `AgentCodeLoad` |
| **P7 Preview and devices** | Server preview runtime (headless Chromium), desktop CEF preview view, tunnel and port forwards, picking, recording, mini player ([11 §5](11-Desktop.md#5-preview-view)). HTML renders, MCP Apps ([06 §3-4](06-Terminal-Preview-Devices.md#3-html-renders-visual-replies)). Device hosts and sessions ([06 §5](06-Terminal-Preview-Devices.md#5-devices-simulators-and-emulators)). | An agent opens the app it runs in a preview tab, the user picks an element on the desktop and sends it as context; the phone watches the same tab as a screencast. | Desktop installed-app walk |
| **P8 Finish the surface** | Secrets with all backends and `request_secret` ([07 §1](07-Secrets-Guest-Voice-Usage.md#1-secrets)). Guest share. Voice input (O10). Usage pages and limits. Every settings page and the settings search; appearance, themes, custom CSS, keybinding editor. Snap Shot. Palette groups. Accessibility pass. | Every T3 settings name and every T3 keybinding works, on every surface where T3 has it. | `AgentCodeSweep`, `AgentCodeSweepTui`, `AgentCodeBrowser` |
| **P9 iOS** | With MD7: the iOS app on the client model; APNs; Live Activities; WidgetKit widgets; share extension; on-device speech ([12 §5.3](12-Mobile.md#53-ios-with-md7)). | The Android scenarios pass on an iPhone. | iOS installed-app walk |
| **P10 Import and polish** | Import from a T3 Code database ([02 §14](02-Orchestration.md#14-import-from-t3-code)). Session import from harness histories. Default browser (desktop), browser cookie import, passkeys. Expo and Simlock device hosts. | A T3 user moves to MR Moqui with their threads, projects and settings. | none |

Order: P0 and P1 first. P2 and P3 next in order. P4 to P8 can overlap after P3 when two people work on them. P9
waits for MD7.

## 3. Open decisions

| # | Decision | Options | Recommendation | Blocks |
|---|---|---|---|---|
| O1 | Component place | `base/agent-code` (production, gated by permissions) or `dev/agent-code` (dev instances only, as v1) | `base`: users want it on their servers; the gates of [14](14-Security.md) make that safe | P0 |
| O2 | App path and name | `/qapps/code`, "Agent Code" (working names) | Keep | P0 |
| O3 | Claude transport | `claude` CLI stream-JSON (no Node) or the Agent SDK (needs Node in MR Moqui) | CLI. The Cursor driver's shipped Node worker is the one exception the plan makes now (see O9) | P1 |
| O4 | Old external agent bridge (`external_agent.rs`, `launch`, `projection`) | Keep beside the new adapters, or move its users to the Codex and Claude drivers | Move, then remove in P4 | P4 |
| O5 | Harness process unit | One process per (instance, cwd, user), or shared per (instance, cwd) | Per user: separate auth and homes | P1 |
| O6 | Git submodules | `moqui-git` submodule support, or the `git` CLI where the crate lacks it | `git` CLI for submodules only, until the crate has it | P2 |
| O7 | Stronger process isolation | Landlock, bubblewrap, separate OS user, container ([14 §5.2](14-Security.md#52-stronger-isolation-open-decision-o7)) | Landlock in P2; separate OS user as an option in P6 | P2 |
| O8 | Terminal renderer | Keep the MR Moqui renderer, or move to `libghostty-vt` (WASM on web, native on phones) | Keep; measure on phones in P5 | none |
| O9 | Device hub | T3's Node device hub (`expo-device-hub`, `agent-device`) on the device host, or a Rust device hub in `moqui-code` | **Open.** "Node at pinned versions first" sits badly with O3, whose reason is that MR Moqui has no Node at runtime. That rule is already not kept whole: the Cursor driver runs a Node worker the component ships (`cursor-worker.mjs`, [03 §5](03-Providers.md#5-per-driver-plan)), so Agent Code owns Node code at runtime on a host that runs Cursor (Pi's Node, by contrast, is the harness's own). A device hub would be a second such case, on every device host and for every user of devices rather than only for those who pick one driver. Choices: accept Node as an optional host requirement for named drivers and the device hub (and restate O3 so), or a Rust device hub from the start (`adb` and the simulator tools driven from `moqui-code`, frames streamed on `/agentws`) and keep Node to the Cursor worker alone. Decide before P7 | P7 |
| O10 | Local speech engine | Whisper on candle (MR Moqui plan), or Parakeet GGUF (T3) | Whisper on candle (one model runtime in the tree) | P8 |
| O11 | Assist on `/agentws` | Move the Assist drawer from SSE to `/agentws` (ends the app poll and SEC-AI-06), or leave it | Move after P1 | none |
| O12 | Direct browser connections to other environments | Browser pairs to several environments (keys in IndexedDB, CORS for tickets), or peer links only | Peer links only until P6 ends; then decide by user need | P6 |
| O13 | Hosted relay | A moqui.org relay for users with no edge node, or edge node only | Edge node only | none |

## 4. What this plan does not port

| T3 piece | Why |
|---|---|
| T3 Connect hosted service (Clerk, Cloudflare Worker, tunnels) | Replaced by the edge node, direct push and pairing ([13 §8](13-Servers-Linking.md#8-relay-and-tailscale)). |
| T3 Chat account link | A T3 product tie-in. |
| React Native UI and its native modules | One UI in the webview; native parts only where needed ([12 §4.4](12-Mobile.md#44-native-parts)). |
| Electron main process | The Tauri desktop app and CEF. |
| `t3` npm and launcher installs | MR Moqui packaging and `moqui update`. |
| Telemetry to T3 | MR Moqui tracing and OTLP to the user's own collector. |
| Marketing site, waitlist, legal pages | Not product features. |
