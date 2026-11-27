# Agent Code: a full port of T3 Code onto the MR Moqui agent runtime

Design and implementation plan (Nov 22, 2026, not built).

## 1. Goal

Port every feature and the full UI of T3 Code (a coding-agent workbench) into MR Moqui as one complete application,
"Agent Code". Agent Code runs on every MR Moqui surface: the web, the Tauri desktop app, the Android app (iOS when
MD7 starts), and servers in every topology (single node, cluster, edge-linked, linked peers, SSH-launched hosts).

The port is not a wrapper and not a second store:

- **One data model.** A T3 thread is an `AgentConversation`. A T3 run is an `AgentRun`. A T3 turn item is an
  `AgentMessage`, an `AgentMessagePart`, an `AgentToolCall`, an `AgentApproval`, an `AgentQuestion` or an
  `AgentRunEvent`. New entities hold only what has no home today. See [01-Data-Mapping](01-Data-Mapping.md).
- **One engine seam.** Each conversation names its engine. The native `moqui-llm` loop is one engine. Codex, Claude,
  Cursor, OpenCode, Pi, ACP agents and the other T3 drivers are harness engines, run by Rust adapters.
- **Full mirror.** The adapter writes every harness event to the entities as it happens: messages, reasoning, tool
  calls, command output, file changes, approvals, questions, plans, subagents, usage.
- **The same setup as T3.** A T3 "environment" is an MR Moqui instance. Desktop, phone and web connect to one or
  more instances. Instances link to each other. A desktop can start an instance on another machine over SSH.

"LLM" in the request means the native `moqui-llm` runtime. Its providers, models, tools, approvals, skills, memory
and usage become features of Agent Code beside the external harnesses.

## 2. Source

| Item | Value |
|---|---|
| T3 Code repository | `pingdotgg/t3code`, MIT, Copyright (c) 2026 T3 Tools Inc. |
| T3 Code commit studied | `b9d0f70b3ef1917b75673dfa57b8278da6cffb36` (Nov 21, 2026) |
| MR Moqui base | `master` at `538eb519e` |
| Path notation | `t3:` = a path in the T3 tree. A bare path = a path in this repository. |

- The port is a rewrite. T3 Code is TypeScript, Effect, React, Expo, Electron and SQLite. Take its design,
  contracts, limits and behaviour. Do not copy its code layout.
- Keep the T3 MIT notice in the component's `NOTICE` file and in the third-party list. Some T3 files carry other MIT
  notices. Take each such file only with its own notice.
- T3 product hosting (marketing site, the Clerk account service, the Cloudflare relay service) is not ported as a
  hosted service. Its features are ported onto MR Moqui's own pieces (edge node, push, Comms). See
  [13-Servers-Linking](13-Servers-Linking.md).

## 3. Principles

1. **Extend the agent entities. Do not fork them.** Add fields to `AgentConversation`, `AgentRun`, `AgentToolCall`
   and the others. Add an entity only where no entity has the meaning.
2. **The engine is a field, not a code path.** Screens and services read the same entities for the native and the
   harness engines.
3. **Write as it happens.** Each harness event is written in its own short transaction. A crash loses at most the
   event in flight. The run lease (`crates/moqui-llm/src/run/lease.rs`) covers harness runs too.
4. **Large data goes to resources.** Diffs, command output, terminal scrollback, recordings and attachments go to
   `ContentBlob` or the run's spill location. Rows hold a summary and a location.
5. **Tools are permissions.** A runtime mode is a preset of `AgentToolPolicy` rows. A harness's shell, file and git
   actions map to `AgentClassification` values.
6. **Every surface, same screens.** Screens are XML screens with `.qvue` files. Desktop and Android show the same
   server pages. Native code adds only what a WebView cannot do (push, widgets, share target, speech, keys, SSH).
7. **Every T3 limit is kept** unless this plan names a different value and the reason.
8. **Failure tolerance.** One failed provider instance, project, worktree, webhook, peer link or device host logs an
   error with its name. The rest goes on. Stay strict only for security and data.

## 4. Where the code goes

| Piece | Location | Notes |
|---|---|---|
| Component | `runtime/components/base/agent-code` | XML entities, services, screens, `.qvue`, data, specs. Production code, not dev-only. Gated by permissions and conf (see [14-Security](14-Security.md)). |
| Rust crate | `crates/moqui-code` | Harness adapters, process host, checkpoints, worktrees, PTY reuse, preview, devices, peer link, voice. Native component pattern (`submit_component!`, `ToolFactory`). Depends on `moqui-llm`, `moqui-git`, `moqui-core`. |
| Desktop host code | `crates/moqui-desktop/src/code/` | Folder picker, editor open, SSH launcher, port forward, preview view, server exposure, badge. |
| Client core | `crates/moqui-client/src/code/` | Several connected environments, routes, offline store, outbox kinds. Shared by desktop and mobile. |
| Android native | `crates/moqui-mobile/gen/android/.../code/` | Kotlin: activity notification, widgets, share target, speech. |
| iOS native | `crates/moqui-mobile/gen/apple/` (with MD7) | Swift: APNs, Live Activities, WidgetKit, share extension, Speech. |
| TUI | `crates/moqui-tui/src/code/` | Thread list, thread view, composer. |
| CLI | `crates/moqui/src/cli/code.rs` | `moqui code ...` (the `t3` command set). |
| Docs | `docs/ai/Agent-Code.md` and `docs/ai/Agent-Code-*.md` | User and admin guides when built. |

## 5. Files in this plan

| File | Content |
|---|---|
| [01-Data-Mapping](01-Data-Mapping.md) | Every T3 entity, field, enum and setting mapped to MR Moqui entities and fields. New entities. |
| [02-Orchestration](02-Orchestration.md) | Commands, thread lifecycle, settle, snooze, pin, queue, steer, interrupt, recovery, checkpoints, fork, handoff, subagents, worktrees, projects, compaction. |
| [03-Providers](03-Providers.md) | The adapter trait, the process host, each driver, capabilities, install and update, text generation, session import, replay tests. |
| [04-Git](04-Git.md) | VCS, stacked actions, code hosts, pull requests, review guides, files viewed, PR watch, stacks, CI, issues, triage. |
| [05-Automation](05-Automation.md) | Scheduled tasks and webhooks on `AgentTrigger`. |
| [06-Terminal-Preview-Devices](06-Terminal-Preview-Devices.md) | Terminal drawer, browser preview and automation, port forwards, HTML render, MCP Apps, simulators and emulators. |
| [07-Secrets-Guest-Voice-Usage](07-Secrets-Guest-Voice-Usage.md) | `request_secret`, guest share, notifications, voice, usage. |
| [08-MCP-Tools](08-MCP-Tools.md) | The full T3 MCP tool set as `moqui-llm` tools and MCP server tools. |
| [09-Wire-API](09-Wire-API.md) | The `/agentws` socket, push streams, REST, raw routes, the environment descriptor. |
| [10-Web-UI](10-Web-UI.md) | Routes, shell, sidebar, thread view, composer, diff, PR pages, palette, keys, every setting and settings page, responsive rules, TUI. |
| [11-Desktop](11-Desktop.md) | The Tauri desktop app as T3 Desktop. |
| [12-Mobile](12-Mobile.md) | Android now and iOS with MD7, as T3 Mobile. |
| [13-Servers-Linking](13-Servers-Linking.md) | Environments, pairing, auth sessions, routes, peer links, SSH launch, Tailscale, relay, background service, updates, cluster and edge. |
| [14-Security](14-Security.md) | Permissions, scopes, access levels, gates, threats, audit, attack-surface docs. |
| [15-Tests](15-Tests.md) | Regression and acceptance tests within the suite rules. |
| [16-Phases](16-Phases.md) | Phases, order, exit checks, open decisions. |

## 6. Surfaces at a glance

| T3 surface | MR Moqui surface | Main file |
|---|---|---|
| T3 server (`t3 serve`) | An MR Moqui instance with Agent Code on | [13](13-Servers-Linking.md) |
| Web app | `/qapps/code` | [10](10-Web-UI.md) |
| Desktop (Electron, local backend) | Tauri desktop, in-process node, Agent Code local | [11](11-Desktop.md) |
| Desktop thin client (`localEnvironmentEnabled` off) | Tauri desktop client mode | [11](11-Desktop.md) |
| Mobile (Expo) | Android app; iOS with MD7 | [12](12-Mobile.md) |
| SSH remote environment | Desktop SSH launcher plus `moqui code up` | [13](13-Servers-Linking.md) |
| Linked environments (peer links) | `AgentEnvironmentLink` over HTTPS or the edge link | [13](13-Servers-Linking.md) |
| T3 Connect (relay, push, Live Activities) | Edge node, FCM, APNs | [13](13-Servers-Linking.md), [12](12-Mobile.md) |
| Tailscale | Optional `tailscale` integration in `moqui-host` | [13](13-Servers-Linking.md) |
| CLI (`t3 ...`) | `moqui code ...` | [13](13-Servers-Linking.md) |
| TUI | `moqui-tui` Code view and SSH `agent-apps` | [10](10-Web-UI.md) |
