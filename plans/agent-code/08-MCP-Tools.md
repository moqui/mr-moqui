# 08. Orchestrator tools (the T3 MCP server)

Part of [Agent Code](README.md). T3 sources: `t3:docs/orchestration-v2/orchestrator-mcp-server.md`,
`t3:apps/server/src/mcp/*`, `t3:packages/contracts/src/{orchestratorMcp,threadMetadataMcp,worktreeMcp,
previewAutomation,device}.ts`.

## 1. Transport, auth and injection

### 1.1 One tool set, two ways in

Each tool is written once as a `moqui-llm` `AgentTool` (`submit_tool!`) in `crates/moqui-code/src/tools/`. It is
offered two ways:

| Caller | How it reaches the tools |
|---|---|
| The native engine (AghdNative) | In-process, as `moqui-llm` tools. The run's `ToolPolicy` and the runtime mode preset ([03 §9](03-Providers.md#9-native-engine-tools-for-coding)) choose which ones. |
| A harness engine (Codex, Claude, Cursor, ...) | Over MCP: the existing MCP server of `crates/moqui-llm/src/mcp/` at `/mcp`, with the session credential of §1.2. |
| An outside agent (Claude Code in a terminal, ChatGPT, bots) | The same `/mcp` with OAuth (§1.4). |

### 1.2 Session credential

- T3 `McpSessionRegistry` mints a bearer token per provider session. MR Moqui mints a `UserLoginKey` with:
  - `scope` = `code:mcp-session`;
  - new fields on the key's purpose row (`AgentMcpSessionKey`, see below): `conversationId`, `runId` (parent run),
    `providerInstanceId`, `harnessSessionId`, `capabilities` (`preview orchestration ...`);
  - expiry: max lifetime 24 h, idle 2 h. Revoked when the harness session is released.
- The raw key is never stored. Only its hash, as with all login keys.
- The MCP host (`crates/moqui-host/src/llm/mcp.rs`) resolves the key to an `McpInvocationScope`: user, conversation,
  run, instance, capabilities. Each tool checks its capability.
- New entity `AgentMcpSessionKey` (PK `loginKeyHash`): conversationId, runId, providerInstanceId, harnessSessionId,
  capabilitiesText, createdDate, idleExpireDate, expireDate, revokedDate.
- The MCP server must be on for Agent Code (`<mcp server-enabled>`). Agent Code turns on only the loopback listener
  path when the conf has it off: harness children reach `http://127.0.0.1:<port>/mcp`. Outside callers still need it on.

### 1.3 Injection per harness

| Harness | How the server is given | MR Moqui |
|---|---|---|
| Codex | `thread/start`, `thread/resume`, `thread/fork` config with `Authorization`. Registered twice: `moqui-code` (all but `code_thread_wait` and `delegate_task`) and `moqui-code-direct` (only those two, `omit_tools_from = ["code_mode","deferred"]`), because Code Mode yields scripts after about 30 s | Same |
| Claude | stream-JSON `--mcp-config` with `{"moqui-code": {type:"http", url, headers}}`; `--allowedTools mcp__moqui-code__*`. Token never logged | Same (O3 in [16](16-Phases.md)) |
| Cursor | `mcpServers` on `Agent.create` / `Agent.resume` and each `send`; ambient `settingSources` stay | Same, in the Node worker |
| Grok, fx and ACP agents | `mcpServers` in `session/new`, `session/load`, `session/fork` | Same |
| ACP agents without HTTP MCP | stdio bridge | `moqui code mcp-stdio` (a hidden mode of the server binary) bridges stdio to `/mcp` |
| Pi | A Pi extension with `MOQUI_MCP_URL` and `MOQUI_MCP_BEARER_TOKEN`; names `mcp__moqui-code__*` plus model-only `mcp__moqui_code__*` aliases; on Pi 0.99+ only `orchestrator_capabilities`, `delegate_task`, `task_status` direct, others through Pi `tool_search`; header `mcp-protocol-version: 2025-06-18` | Same. Extension file shipped in the component |
| OpenCode 2 | `moqui-code-<thread>` MCP entry | Same |

### 1.4 Outside agents (OAuth)

- T3 serves `/oauth/mcp/*`. MR Moqui MCP auth has bearer keys and app passwords. OAuth is "later" in
  [docs/ai/MCP.md](../../docs/ai/MCP.md). Agent Code builds it:
  - dynamic client registration `POST /oauth/mcp/register` → `AgentMcpOAuthClient`;
  - authorize page `/oauth/mcp/authorize` (a Moqui screen; the user signs in, picks an access level: read-only,
    approval-required, auto-accept-edits, full-access; recent sign-in needed for full-access);
  - token `POST /oauth/mcp/token` (PKCE S256 only) → `AgentMcpOAuthGrant` plus a scoped `UserLoginKey`;
  - metadata `/.well-known/oauth-authorization-server` and `/.well-known/oauth-protected-resource`.
- Grants are listed in **Settings > Connections > Agents** with revoke.
- An outside agent is a caller without a parent run. Tools that need one (`delegate_task`, worktree handoff) return
  `parent_not_active`.

### 1.5 Access wrappers

Port of `t3:McpToolAccess.ts` to `crates/moqui-code/src/tools/access.rs`:

| Wrapper | Rule |
|---|---|
| `reads` | Read in the caller's scope. |
| `reads_as_caller` | Read as the calling user, also in other threads the user can see. |
| `acts_as_caller` | Act as the user on objects with their own permission check (PRs, issues). |
| `writes` | Write objects that have no thread (uploads, scheduled tasks). |
| `writes_with_capability` | Write if the credential has the named capability. |
| `writes_threads` | Write another thread: needs the caller's live run, and modes no broader than the caller's. |
| `starts_threads` | Start threads with modes no broader than the caller's. |
| `either` | One of two. |
| `after_forwarding` | The target is a linked environment: forward the call ([13 §6](13-Servers-Linking.md#6-peer-links)), then check there. |
| `writes_environment` | Change environment preferences: needs `AgentCodeAdmin`. |

Every tool also has a `moqui-llm` `Classification` (Read, Write, Destructive, External). Write and Destructive tools
go through `AgentApproval` unless the runtime mode preset allows them.

### 1.6 Exposure

- `tool_search` (`query`, max 500 chars) finds optional tools when the harness supports deferred tools. It maps to the
  existing lazy catalog (`find_tools`, `describe_tools`, `run_tool` in `crates/moqui-llm/src/tools/lazy.rs`).
- The tool list is sorted by name and fixed per run, so the prompt prefix stays stable.

### 1.7 Names

- MR Moqui names drop the `t3_` prefix for `code_`: `t3_thread_read` → `code_thread_read`. Names without the prefix
  stay (`delegate_task`, `preview_open`, `html_render`).
- The T3 names are accepted as aliases, so prompts and skills written for T3 still work. Aliases are not listed, only
  resolved. Other plan files quote T3 names; read them as the `code_` names.
- The MCP server key is `moqui-code`.

## 2. Tool list

`?` = optional. `environmentId` = this environment or a linked one. A tool with a linked `environmentId` is
forwarded. Class: R read, W write, D destructive, X external.

### 2.1 Orchestration and delegation

| Tool | Inputs | Class | Service |
|---|---|---|---|
| `orchestrator_capabilities` | `environmentId`, `search?` | R | `get#OrchestratorCapabilities` |
| `delegate_task` | `task`, `target?` {`providerInstanceId?`, `driverKind?`, `model?`, `options?`, `environmentId?`, `projectId?`} (object or JSON string), `title?`, `role?`, `isolation?` (shared default, worktree), `mode?` (async default, wait), `timeoutMs?` (wait only, max 45 s), `clientRequestId?`, `runtimeMode?`, `interactionMode?` | W | `request#DelegatedTask` |
| `task_status` | `taskId` | R | `get#DelegatedTask` |
| `task_cancel` | `taskId`, `reason?`, `clientRequestId?` | W | `cancel#DelegatedTask` |
| `code_workflow_stop` | `environmentId`, stop-workflow fields | W | `stop#Workflow` |
| `code_background_wait` | `taskIds`, `waiting?` | R | `wait#BackgroundWork` |
| `create_threads` | `threads[]` (1-20) of {`prompt`, `title`, `target`, `runtimeMode`, `interactionMode`}, `clientRequestId?`; local only | W | `create#Threads` |
| `code_thread_list` | `environmentId`, `projectId`, `allProjects?`, `statuses?`, `titleContains?`, `settled?`, `snoozed?`, `includeSubagents?`, `cursor?`, `limit?` | R | `find#Threads` |
| `code_thread_read` | `environmentId`, `threadId`, `itemId?`, `textOffset?`, `view?` (messages, activity), `afterPosition?`, `beforePosition?`, `limit?`, `runLimit?`, `maxCharsPerItem?` | R | `read#Thread` |
| `code_thread_update` | `threadId?`, `action` (rename, regenerate_title), `title?`, `clientRequestId?` | W | `update#Thread` |
| `code_thread_send` | `environmentId`, `threadId`, `message`, `mode?` (auto, queue, steer, restart), `freshContext?`, `outputSchema?`, `clientRequestId?`; result `delivery` started, queued, steered, restarted | W | `send#ThreadMessage` |
| `code_thread_wait` | `environmentId`, `threadId`, `runId?`, `timeoutMs?`, `untilDelegatedWorkSettled?`, `untilOutput?` | R | `wait#Thread` |
| `code_thread_interrupt` | `environmentId`, `threadId`, `runId?`, `reason?`, `stop?`, `clientRequestId?` | W | `interrupt#Thread` |
| `schedule_task` | `environmentId`, `projectId`, `prompt`, `schedule`, `title?`, `enabled?`, `bindToCurrentThread?`, `freshSession?`, `clientRequestId?`, `scheduledTaskId?` | W | `store#ScheduledTask` |
| `list_scheduled_tasks` | `environmentId`, `projectId?`, `scheduledTaskId?` | R | `find#ScheduledTasks` |
| `delete_scheduled_task` | `environmentId`, `scheduledTaskId` | D | `delete#ScheduledTask` |
| `run_scheduled_task_now` | `environmentId`, `taskId` | W | `run#ScheduledTaskNow` |
| `request_secret` | `label`, `reason`, `url?`, `placeholder?`, `delivery?` (secretRef, file), `fields?`, `value?`, `timeoutMs?`, `clientRequestId?` | W | `request#Secret` ([07 §1.2](07-Secrets-Guest-Voice-Usage.md#12-request_secret)) |
| `code_environment_links` | none | R | `find#EnvironmentLinks` |
| `code_environment_link` | `environmentId?`, `url?`, `hint?`, `reason?`, `requestedAccess?`, `clientRequestId?` | X | `request#EnvironmentLink` (opens a card; the user pairs) |
| `code_environment_unlink` | `environmentId` | D | `delete#EnvironmentLink` |

### 2.2 Thread control

| Tool | Inputs | Class | Service |
|---|---|---|---|
| `code_thread_organize` | `environmentId`, `threadId?`, `action` (pin, unpin, snooze, unsnooze, settle, unsettle, archive, unarchive, mark_unread), `snoozedUntil?` | W | `organize#Thread` |
| `code_queue` | `action` (read actions, edit, cancel, reorder, promote_to_steer), `environmentId`, `threadId?`, `queuedRunId?`, `text?` (max 100,000), `beforeRunId?`, `targetRunId?`, `cursor?`, `limit?` | W | `update#Queue` |
| `code_pending_request` | `action` (list, read, respond), `environmentId`, `threadId?`, `requestId?`, `answers?` | W | `respond#PendingRequest` (questions only; approvals are never answered by a tool) |
| `code_thread_configure` | `environmentId`, `threadId?`, `modelSelection?` | W | `configure#Thread` |
| `code_thread_transfer` | `action` (fork, merge_back, status), `environmentId`, `threadId?`, `sourcePoint?`, `targetThreadId?`, `title?` | W | `transfer#Thread` |
| `code_thread_search` | `environmentId`, query, match source (title, user, assistant), `projectId?` | R | `search#Threads` |
| `code_submit_output` | `json` (string, max 200,000) | W | `submit#RunOutput` (validates against `outputSchemaJson`) |
| `send_to_user` | `message` (Markdown shown as written, mid-turn) | W | writes an `AgentMessage` kind notice |

### 2.3 Projects

| Tool | Inputs | Class | Service |
|---|---|---|---|
| `code_project_list` | `environmentId`, `cursor?`, `limit?` | R | `find#Projects` |
| `code_project_read` | `environmentId`, `projectId` | R | `get#Project` |
| `code_project_create` | `environmentId`, create payload, `workspaceRoot?` | W | `create#Project` |
| `code_project_update` | `environmentId`, `projectId`, update payload | W | `update#Project` |
| `code_project_delete` | `environmentId`, `projectId`, `force?` (needed when nonempty; folder stays) | D | `delete#Project` |
| `code_project_clone` | `environmentId`, clone input | X | `clone#Project` |
| `code_thread_launch` | `environmentId`, `projectId?`, `scratch?`, `title`, `generateTitle?`, `modelSelection?`, `runtimeMode?`, `interactionMode?`, `workspaceStrategy?` (root default, worktree + `baseRef` + `startFromOrigin`, existing_worktree + `worktreePath`), `message?`, `attachments?`, `outputSchema?`, `clientRequestId?`, `delegatedFrom?` | W | `launch#Thread` |

### 2.4 Worktrees

| Tool | Inputs | Class | Service |
|---|---|---|---|
| `code_worktree_handoff` | `branch`, `baseRef?`, `startFromOrigin?`, `runSetupScript?`, `continuationPrompt?` | W | `handoff#Worktree` |
| `code_worktree_status` | none | R | `get#WorktreeStatus` |
| `code_worktree_list` | `environmentId`, `threadId?`, `query`, `cursor`, `limit`, `refKind`, `includeMatchingRemoteRefs` | R | `find#Worktrees` |

### 2.5 Pull requests and CI

PR target = `environmentId`, `threadId?`, `url?` or (`repository` + `number`), `host?`.

| Tool | Inputs | Class |
|---|---|---|
| `link_pull_request`, `unlink_pull_request` | PR target | W |
| `link_pull_requests` | `environmentId`, `threadId?`, `urls` | W |
| `unlink_pull_requests` | `environmentId`, `threadId?`, `states` | W |
| `list_thread_pull_requests` | `environmentId`, `threadId?`, `states?`, `cursor?` | R |
| `watch_pull_request` | PR target, `until` (checks, merged) | W |
| `unwatch_pull_request` | PR target (stays linked) | W |
| `write_pull_request_guides` | `environmentId`, `pullRequests` | W |
| `read_pull_request_guide` | `environmentId`, guide target | R |
| `pull_request_read` | thread PR fields, `include?`, `includeResolved?`, `cursor?` | R |
| `pull_request_list` | `environmentId`, `state?`, `involvement?`, `query?`, `limit?`, `cursor?` | R |
| `pull_request_create` | `environmentId`, `title`, `body`, `base?`, `draft?` (pushes, opens and links, or returns the open PR) | X |
| `pull_request_update` | thread PR fields, `title?`, `body?`, `ready?`, `state?`, `addLabels?`, `removeLabels?`, `requestReviewers?` | X |
| `pull_request_respond` | thread PR fields, `reviewThreadId?`, `body?`, `resolved?` | X |
| `pull_request_review` | thread PR fields, `verdict`, `body?`, `comments?` | X |
| `pull_request_merge` | thread PR fields, `expectedHeadSha`, `method?`, `auto?`, `stack?` | D |
| `ci_logs` | thread PR fields, `check?`, `tailLines?` | R |
| `ci_rerun` | thread PR fields, `check?` | X |

Services are in `PullRequestServices.xml` ([04](04-Git.md)). `pull_request_merge` refuses when `expectedHeadSha` is
not the head the run last read.

### 2.6 Issues

Issue target = `url`, `key`, `projectId`.

| Tool | Inputs | Class |
|---|---|---|
| `list_issues` | `projectId?`, `state?`, `query?`, `limit?`, `cursors?` | R |
| `read_issue` | issue target | R |
| `create_issue` | `projectId?`, `scope?`, `title`, `body?`, `labels?`, `assignees?` | X |
| `comment_on_issue` | issue target, `body` | X |
| `set_issue_state` | issue target, `state` | X |
| `link_issue`, `unlink_issue` | issue target + thread target | W |
| `list_thread_issues` | thread target | R |

### 2.7 Browser preview

Tab target = `tabId?`. All need capability `preview` and setting `enableAgentBrowserAccess`.

| Tool | Inputs | Class |
|---|---|---|
| `preview_status` | tab target | R |
| `preview_open` | tab target, `url?`, `open?`, `show?`, `reuseExistingTab?`, `profileId?` | W |
| `preview_dialog` | tab target, `accept`, `promptText?` | W |
| `preview_navigate` | tab target, `url?`, `target?`, `readiness?`, `timeoutMs` | W |
| `preview_resize` | tab target, `mode`, `preset?`, `width?`, `height?`, `orientation?`, `timeoutMs` | W |
| `preview_set_appearance` | tab target, `colorScheme` | W |
| `preview_snapshot` | tab target, `includeImage?` (default false), `save?` (writes PNG, returns `screenshotPath`) | R |
| `preview_act` | `action`, tab target, `locator?`, `selector?`, `x?`, `y?`, `button?`, `clickCount?`, `text?`, `clear?`, `values?`, `source?`, `target?`, `paths?`, `key?`, `modifiers`, `deltaX?`, `deltaY?`, `timeoutMs?` | W |
| `preview_evaluate` | tab target, `expression`, `awaitPromise?` | W |
| `preview_wait_for` | tab target, `selector?`, `locator?`, `text?`, `urlIncludes?`, `timeoutMs` | R |
| `preview_recording_start`, `preview_recording_stop` | tab target | W |
| `code_preview_list` | `cursor?`, `limit?` | R |
| `code_preview_close` | `tabId` | W |

Preview tools are classed W but are allowed in every runtime mode. They act on a sandboxed browser, not on data.
`upload` paths must be inside the work root.

### 2.8 HTML, devices, attachments, environment, theme

| Tool | Inputs | Class |
|---|---|---|
| `html_preview` | `path?`, `html?`, `width?`, `appearance?` (returns PNG, contentHeight, console output) | R |
| `html_render` | `path?`, `html?`, `title`, `height` (shows the page above the final reply) | W |
| `device_list` | `hostId?` | R |
| `device_open` | `deviceId`, `platform`, `hostId` | W |
| `device_screenshot` | `deviceId`, `hostId` | R |
| `device_close` | `deviceId`, `hostId`, `shutdown` | W |
| `code_attachment_prepare_upload` | `upload` | W |
| `code_attachment_discard` | attachment id (pending upload only) | W |
| `code_thread_send_attachments` | `threadId?`, `message?`, `attachments` | W |
| `code_environment_read` | `environmentId` | R |
| `code_environment_preferences_update` | `environmentId`, `defaultThreadEnvMode`, `newWorktreesStartFromOrigin`, `enableProviderUpdateChecks`, `backgroundActivity?`, `sourceControlWritingStyle` | W (`writes_environment`) |
| `code_theme_read` | `themeId?` | R |
| `code_theme_set` | `themeId?`, `theme?`, `appearance?` | W (user preference) |
| `tool_search` | `query` | R |

### 2.8a Terminals

Reading and driving a thread terminal is the framework's ([Terminal-Fancy2](../Terminal-Fancy2.md) TF3): the services
`get#TerminalScreen`, `find#TerminalEvents`, `send#TerminalInput` and `wait#TerminalSession`, and the dev tools'
`terminal` tool with one approval per session per run. Agent Code offers them under its scopes, not as tools of its
own:

| Tool | Inputs | Class |
|---|---|---|
| `terminal_read` | `threadId`, `terminalId`, `scope?` (visible, scrollback, last), `lines?` (`get#TerminalScreen`) | R (`code:terminal-read`) |
| `terminal_events` | `threadId`, `terminalId`, `after?` (`find#TerminalEvents`) | R (`code:terminal-read`) |
| `terminal_send` | `threadId`, `terminalId`, `text?`, `keys?` (`send#TerminalInput`) | W (`code:terminal`) |
| `terminal_wait` | `threadId`, `terminalId`, one of `status`, `pattern`, `prompt`, `quietMs`, `ended`; `timeoutSeconds` (`wait#TerminalSession`) | R (`code:terminal`) |

### 2.9 Native-only coding tools

The native engine also needs file, shell and git tools that a harness has built in. They are in
[03 §9](03-Providers.md#9-native-engine-tools-for-coding) and are not offered over MCP.

## 3. Behaviour rules

Port of `t3:ThreadManagementService` to `crates/moqui-code/src/threads/manage.rs`. It is the one boundary for
`/agentws` commands and tools: lookup, list, send-mode choice, send checks, wait, interrupt.

### 3.1 Capabilities

`orchestrator_capabilities` returns the inherited instance and model, parent modes, instances with models, child-task
availability and feature flags. Model lists are short: custom, favorite and caller models, plus all current models for
Codex, Claude, Muse, Grok and the native engine, else the first 8. `omittedModels` counts the rest; `search` finds
them.

### 3.2 Delegation

- Omitted provider, model and modes are inherited. A driver-only target picks an available instance. An explicit
  instance id must be available.
- `target.options` such as `{effort:"max"}` override; `{}` clears.
- `isolation: "worktree"` makes a prepared checkout from the parent commit. Linked-environment targets refuse worktree
  isolation.
- A runtime mode the target does not offer is refused. An inherited one steps down.
- Needs an active parent run owned by the credential's session. Becomes command `delegated_task.request`
  ([02 §10](02-Orchestration.md#10-subagents-and-delegated-tasks)).
- `wait` ends on result, timeout (`waitTimedOut`), a blocking question (`hasPendingQuestion`) or an approval
  (`pendingApproval`). With `next`, completion wakes the parent; the parent does not poll.
- A child question notifies the parent (steered in if possible, else queued; cancelled if answered first). Approvals
  are not routed to the parent. The user decides them.
- Replay after restart is idempotent (the `AgentDelegatedTask` row and the command receipt).

### 3.3 Status, cancel, read, send, wait, interrupt

- `task_status`: `status: "starting"` until the child's turn starts; `workState`; `hasPendingChildRuns`;
  `latestTerminal*`.
- `task_cancel`: stops nested tasks, then stops the child thread, cancels the task's queued turn and disposes parent
  delivery. A completed result settles as `interrupted`.
- `code_thread_read`: result cap about 30,000 chars. Agents get newest first; clients oldest first. Links use
  `<moqui-code://v2/<environmentId>/<threadId>>`. OAuth clients get `https://<server>/qapps/code/t/<threadId>`. Results
  carry `createdBy`, `creationSource`, `snoozed`, `snoozedUntil`, `pinned`, `blockedOnThreadWait`,
  `pendingBackgroundTasks`.
- `code_thread_send` `restart`: interrupt and restart, or stop and queue first where restart is not supported.
  Refused for `/compact` and `/logout`. `outputSchema` makes the run return data through `code_submit_output`; such
  sends queue, not steer.
- `code_thread_wait` end states: completed, failed, cancelled, interrupted, rolled_back. The wait is a subscription on
  the change feed, not a poll.
- `code_thread_interrupt` uses run interrupt; `no_active_run` when idle.
- `schedule_task`: `bindToCurrentThread` defaults to true only when the caller has a thread in the task's project.

### 3.4 Policy

- Child runtime mode equal or narrower. Interaction mode may narrow default → plan, never widen.
- Changing another thread needs the caller's live run and no broader modes.
- Providers must be enabled, installed, available, signed in and adapter-backed. Requested models must be advertised.
- `clientRequestId` gives stable command, thread and message ids per harness session.
- Mode ceilings of [02 §4](02-Orchestration.md#4-messages-dispatch-modes-and-the-queue) and the policy
  `limits.maxRuntimeMode` apply to every tool that starts or changes a run.

### 3.5 Failure codes

`capability_denied`, `parent_not_active`, `provider_unavailable`, `model_unavailable`,
`runtime_mode_escalation_denied`, `interaction_mode_escalation_denied`, `task_not_found`, `task_not_cancellable`,
`thread_not_found`, `run_not_found`, `thread_not_sendable`, `thread_not_interruptible`, `invalid_request`,
`orchestration_error`. Same strings, so T3 prompts read them.

## 4. Code layout

| T3 file | MR Moqui |
|---|---|
| OrchestratorMcpService.ts | `crates/moqui-code/src/tools/orchestrator.rs` |
| ThreadMetadataMcpService.ts | `tools/thread.rs` |
| WorktreeMcpService.ts | `tools/worktree.rs` |
| PreviewAutomationBroker.ts | `crates/moqui-code/src/preview/broker.rs` |
| McpHttpServer.ts | the existing `crates/moqui-host/src/llm/mcp.rs` |
| McpSessionRegistry.ts | `crates/moqui-code/src/tools/session_key.rs` |
| ProviderSessionManager.ts | `crates/moqui-code/src/host/sessions.rs` ([03 §4](03-Providers.md#4-the-process-host)) |
| toolkits/pullRequests, issues | `tools/pull_requests.rs`, `tools/issues.rs` |
| toolkits/devices | `tools/devices.rs` |
