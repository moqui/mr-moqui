# 03. Providers and the harness adapters

Part of [Agent Code](README.md). T3 sources: `t3:packages/provider-*`, `t3:apps/server/src/provider/`,
`t3:docs/internals/providers.md`, `t3:docs/internals/adding-a-provider.md`, `t3:docs/user/providers-*.md`.

## 1. Drivers and instances

| T3 | MR Moqui |
|---|---|
| A driver is a provider kind | `AgentHarnessDriver` enum ([01 §12](01-Data-Mapping.md#12-new-enums-and-status-flows)). |
| An instance is one configured copy with its own settings, home, login and environment | `AgentProviderInstance`, `AgentProviderInstanceEnv`, `AgentProviderAccount`. |
| All work routes by instance, not by kind | Same: runs and threads hold `providerInstanceId`. |
| Built-in order: Codex, Claude, Cursor, Grok, fx, OpenCode, Antigravity, Kiro, Pi, Muse, ACP Registry | Same order in `sequenceNum` of the seed data, with **Moqui** (the native engine) first. |
| A provider package with `settings`, `client`, `server`, `testing` entry points | One Rust module per driver in `crates/moqui-code/src/driver/<name>/` with `settings.rs`, `adapter.rs`, `probe.rs`, `testing.rs`; the settings form is a screen section per driver. |
| Register: `builtInDrivers.ts`, status order, `model-manifest.json` | `driver/mod.rs` registry (`submit_driver!` macro, same pattern as `submit_tool!`); `AgentHarnessManifest` seed data. |
| Three ways to add an agent: ACP Registry (no code), ACP flavor, native adapter | Same three ways. |
| Instance env vars can hold secret references | `AgentProviderInstanceEnv.secretSourceId`. The value is read at process start and never logged. |

### 1.1 The native engine as a driver

The `moqui-llm` loop is driver AghdNative. Its "instance" binds to `LlmEndpoint` and `LlmModel` rows. It is always
present when an endpoint is set. It has the full capability record (steer, fork by copy, rollback by checkpoint,
plan mode, subagents through `parentRunId` and A2A, compaction, usage). It needs no process host. Its tools come from
`ToolFactory`, plus the Agent Code tool set ([08](08-MCP-Tools.md)) and file, shell and git tools bound to the work
root (§9).

## 2. The adapter trait

Port of `t3:packages/provider-core/src/server/ProviderAdapter.ts` into `crates/moqui-code/src/adapter.rs`.

```rust
pub trait HarnessDriver: Send + Sync {
    fn kind(&self) -> DriverKind;
    fn capabilities(&self, inst: &InstanceConfig) -> Capabilities;
    fn plan_selection_transition(&self, from: &ModelSelection, to: &ModelSelection) -> Transition; // InPlace | Restart | Handoff
    fn checks_version_at_handshake(&self) -> bool;
    async fn probe(&self, inst: &InstanceConfig) -> ProbeResult;   // never opens a session, never touches auth
    async fn open_session(&self, input: OpenSession, host: &dyn ProcessHost) -> Result<Box<dyn SessionRuntime>>;
}

#[async_trait]
pub trait SessionRuntime: Send {
    async fn ensure_thread(&mut self, t: &ThreadCtx) -> Result<NativeThreadRef>;
    async fn resume_thread(&mut self, r: &NativeThreadRef) -> Result<()>;
    async fn inject_history(&mut self, h: &Handoff) -> Result<()> { Err(Unsupported) }
    async fn start_turn(&mut self, input: StartTurn) -> Result<TurnRef>;
    async fn resume_turn(&mut self, t: &TurnRef) -> Result<()> { Err(Unsupported) }
    async fn compact_thread(&mut self) -> Result<()> { Err(Unsupported) }
    async fn steer_turn(&mut self, m: &UserMessage) -> Result<()>;
    async fn apply_model_selection(&mut self, s: &ModelSelection) -> Result<()> { Err(Unsupported) }
    async fn apply_thread_title(&mut self, t: &str) -> Result<()> { Ok(()) }
    async fn stop_task(&mut self, id: &str) -> Result<()> { Err(Unsupported) }
    async fn await_background_tasks(&mut self) -> Result<()> { Ok(()) }
    async fn interrupt_turn(&mut self) -> Result<InterruptOutcome>; // may ask for a runtime restart
    async fn stop_subagent(&mut self, id: &str) -> Result<()> { Err(Unsupported) }
    async fn unload_thread(&mut self) -> Result<()> { Ok(()) }
    async fn respond_runtime_request(&mut self, r: RuntimeResponse) -> Result<()>;
    async fn read_thread_snapshot(&mut self) -> Result<NativeSnapshot>;
    async fn upload_feedback(&mut self, f: Feedback) -> Result<()> { Err(Unsupported) }
    fn mcp_apps(&mut self) -> Option<&mut dyn McpApps> { None }       // list_tools, call_tool, read_resource
    fn mcp_sign_in(&mut self) -> Option<&mut dyn McpSignIn> { None }  // start, complete(callback_url)
    async fn rollback_thread(&mut self, to: RollbackTarget) -> Result<()>; // ThreadStart | ProviderTurn
    async fn fork_thread(&mut self, at: ForkPoint) -> Result<NativeThreadRef>;
    fn has_pending_background_work(&self) -> bool;
    fn background_tasks_end_with_process(&self) -> bool;
    fn model_context_window(&self) -> Option<u64>;
    fn can_resume_after_restart(&self) -> bool;
    fn events(&mut self) -> &mut EventStream<HarnessEvent>;
}
```

`OpenSession` and `StartTurn` carry the same fields as T3's `openSession` and `startTurn` inputs: conversation and
harness session ids, model selection, runtime policy, prompt suggestions, resume data, initial native thread id;
and for a turn the run id and ordinal, harness turn ordinal, attempt, the message, app context, inline context
length, `restartContinuationOfRunId`, `resume`.

### 2.1 Harness events and the mirror

| T3 adapter event | `HarnessEvent` | Entity writes | `AgentRunEvent` |
|---|---|---|---|
| `app_thread.created` | `ThreadCreated` | child `AgentConversation` (subagent or agent-created) | thread.created |
| `provider_session.updated` | `SessionState` | `AgentHarnessSession.statusId` | session.state |
| `provider_thread.updated` | `ThreadState {nativeRef, headRef}` | `AgentHarnessThread` | — |
| `provider_turn.updated` | `TurnState` | `AgentRunAttempt.harnessTurnRef`, `AgentRunStep` | run.state |
| `node.updated` | `NodeState` | `AgentToolCall.nodeStatusEnumId` | tool.state |
| `subagent.updated` | `Subagent {state, childRef}` | child `AgentRun` and conversation | subagent.state |
| `message.updated` | `MessageDelta` / `MessageDone` | `AgentMessage` and part on done | text.delta (live), text.chunk, part.done |
| `turn_item.updated` | `Item {kind, ...}` | by kind: reasoning part, `AgentToolCall`, `AgentFileChange`, artifact | reasoning.*, tool.*, file.change |
| `steer.updated` | `SteerState` | `AgentInput.consumedDate` | input.consumed |
| `background_work.updated` | `Background {count}` | `AgentRun` background count | background.updated |
| `command_output.delta` | `CommandOutput {toolCallId, delta}` | append to spill file | command.output (live) |
| `prompt_suggestion.updated` | `PromptSuggestion` | `AgentSuggestedPrompt` | suggestion |
| `runtime_request.updated` | `Approval` / `Question` / `Dialog` | `AgentApproval` or `AgentQuestion`; run AgrInputRequired | approval.requested, question.asked |
| `plan.updated` | `Plan` | `AgentArtifact` plan or todo, new version | plan.update |
| `turn.terminal` | `TurnDone {reason}` | run end through the completion barrier | run.done |

Rules:

- A delta is live only. The done event is stored.
- The writer is idempotent on `harnessItemRef`. A replay after a crash writes no second row.
- Usage caps (`maxTokens`, `maxCost`) stop the turn with `interrupt_turn`.
- Errors: `AdapterEventStream`, `AdapterProtocol`, `AdapterSessionStopped`. Turn failure reasons include
  `previous_turn_active` and `attachment_unreadable`.

### 2.2 Shared helpers

Ports of `t3:packages/provider-core/src/server/` into `crates/moqui-code/src/driver/common/`:
`text_generation_prompts.rs`, `usage_limits.rs`, `mcp_session.rs` (inject the Agent Code MCP server into each
session), `orchestration_instructions.rs`, `runtime_instructions.rs`, `model_catalog.rs`, `maintenance.rs`,
`continuation_requests.rs`, `handoff_budget.rs`, `attachment_prompt.rs`, `subagent_projection.rs`,
`thread_title_context.rs`.

## 3. Capabilities

| Group | Fields (as T3) |
|---|---|
| sessions | resume after restart, process model (per thread, per instance, in process) |
| threads | `canRollbackThread`, `canForkThread`, native history |
| turns | `supportsActiveSteering`, `supportsSteeringByInterruptRestart`, `terminalStatusQuality` (strong, weak, none) |
| streaming | text deltas, reasoning deltas |
| tools | command output, file changes, MCP tools |
| approvals | kinds, decision options |
| planning | plan mode, plan updates |
| subagents | native subagents, stop subagent |
| context | compaction, context window, usage reporting |
| checkpointing | native checkpoints |
| identity | native thread id quality |
| runtime modes | `supportedRuntimeModes`, `showInteractionModeToggle` |

Rules, as T3: capabilities are honest and the UI hides what is not declared. A stored mode the driver does not
offer runs as Supervised. A status check never opens a session or touches auth. Each session gets the Agent Code
MCP server.

Traps to keep in the driver guide (`docs/dev/Agent-Code-Drivers.md`): apply settings before you advertise them;
`"default"` is a sentinel model id; stop the old process before you start a new one; use continuation requests and
`has_pending_background_work`; subagents can outlive their run; settle the turn on every terminal path; classify
`usage_limit` with a reset time; resolve symlinked paths.

## 4. The process host

`crates/moqui-code/src/host/`. The JavaScript host cannot start processes, so all of this is Rust.

| Item | Rule |
|---|---|
| Unit | One process per (instance, cwd, user) by default; drivers with one server per instance (OpenCode 2.x) share it per (instance, user). See open decision O5. |
| Where | On the cluster member that holds the work root (`AgentWorkRoot.hostInstanceId`). The run lease is taken on that member. |
| OS user | The server's OS user. A server can set `agent-code@run-as` (empty by default) to a separate OS account, started through a setuid helper (`moqui-code-runas`), on Linux only. See [14 §5](14-Security.md#5-process-isolation). |
| Environment | A clean environment: `PATH`, `HOME` (the account home), `LANG`, the instance's env rows, the driver's own variables. Ambient credentials are removed (for example `GEMINI_API_KEY` for Antigravity, `OPENCODE_SERVER_PASSWORD` for an external OpenCode). |
| Stdio | Piped. Stderr goes to `<agent-code@home>/logs/<instance>/<session>.log`, 64 KiB diagnostics budget per session in the UI. |
| Limits | `agent-code@max-sessions` (default 16 per member), idle reaper after `agent-code@session-idle-minutes` (default 30) when no work is pending ([10 §9.5](10-Web-UI.md#95-conf-settings)). |
| Stop | Close stdin, wait 5 s, SIGTERM, wait 5 s, SIGKILL (the same steps as the desktop sidecar). |
| Sandbox | Codex sandbox modes map from the runtime mode. Others run in the work root with the OS user's rights. Linux can add a Landlock rule set that limits writes to the work root and `agent-code@home` (open decision O7). |
| Health | `providerHealthRefreshInterval` (default 5 min) runs `probe`. |
| Keep awake | Desktop holds a sleep inhibitor while sessions run. |

## 5. Per-driver plan

Each row lists the T3 behaviour to keep. All drivers write through §2.1.

| Driver | Process and protocol | Adapter module | Notes to keep |
|---|---|---|---|
| Codex | `codex app-server` plus launch args; JSON-RPC over stdio | `driver/codex` | `setupMode` managed or existing; CODEX_HOME per account; shadow home for several accounts; probe timeout `agent-code/codex@probe-timeout-ms` (a conf setting, default 10000, ours; not an environment variable); image generation launch arg; async questions answered by a new user message; app access approvals; MCP sign-in; `/goal` runs own several native turns; `/feedback` → `upload_feedback`; opts out of `turn/diff/updated` (diffs from checkpoints); native `thread/rollback`; native history injection. |
| Claude | `claude` CLI in stream-JSON mode both ways (`--input-format stream-json --output-format stream-json --verbose`), permission prompts through the Agent Code MCP permission tool | `driver/claude` | T3 runs the Agent SDK in process; MR Moqui has no Node runtime, so it uses the CLI (open decision O3). `CLAUDE_CONFIG_DIR` per account; `autoCompactWindow` 100,000-1,000,000; `idleCompaction` default, onReturn, beforeCacheExpires, off; Compact chip; usage limits read; steering waits for running tool calls; skills from `.claude/skills` and plugins in the `$` menu; `CLAUDE_CODE_ENABLE_TODO_TOOLS=1`; MCP sign-in; `--chrome` allowed; goal state from the transcript. |
| Cursor | `@cursor/sdk` local-agent runtime in a Node worker (`cursor-worker.mjs` shipped in the component) | `driver/cursor` | Needs Node 22.13+ on the host, only for this driver. `auto` → SDK `default`; MCP from `.cursor/mcp.json`, `~/.cursor/mcp.json`, `~/.cursor/plugins`; "Restart agent session"; per-instance login; Change account and Sign out stop the instance's threads. |
| Grok | `grok agent stdio`; Auto adds `--permission-mode auto`; Full access `agent --always-approve stdio`; ACP | `driver/grok` (ACP flavor) | Probes skip auth and sessions. |
| fx | `fx acp`; ACP | `driver/fx` (ACP flavor) | fx 0.0.13+; update by `<binary> upgrade` or Homebrew `fx-agent`; backend is part of the model choice; Effort (default Medium); skills in `$`; MCP from `~/.fx/mcp.json`; modes Supervised, Auto, Full access (0.0.14+), no Auto-accept edits; mode change restarts the connection; steering stops and answers in the same session; subagents as child threads; no text generation. |
| OpenCode | 1.x: `opencode serve --hostname= --port=` per thread; 2.x: `serve --service` per instance; HTTP SDK API | `driver/opencode` (Rust HTTP client) | Needs 1.14.19+, supports 2.0.18+; 1.x "Limited support"; MCP entry `moqui-code-<thread>`; "Always" grants reply `once`; own compaction; `/compact`; Server URL empty = local, set = external with own password; update restarts the local service when the URL matches. |
| Antigravity | Managed install of the Google Antigravity ACP agent | `driver/antigravity` (ACP flavor) | Off by default; `authMethod` oauth-personal, oauth-business, gemini-api-key, agent-platform; `apiKey` secret, `gcpProject`, `gcpLocation`; immutable releases, atomic pointer, leases; per-instance profiles; ambient Google credentials removed; remote sign-in by pasted code or URL, bound to the starting auth session; sign-out stops processes; checkpoints without rollback; text-generation helpers deny tools and reject profiles with hooks or MCP. |
| Kiro | `kiro-cli acp --agent-engine=v3 --auth-method=cli`; ACP | `driver/kiro` (ACP flavor) | `kiro-cli login` or `KIRO_API_KEY`; models from the account, default `auto`; Agent setting (Spec, Plan, `~/.kiro/agents`); modes Supervised and Full access; "Always allow" only when no one-time choice; steer joins after the current step; images stop the turn. |
| Pi | `pi --mode rpc`; RPC over stdio | `driver/pi` | Pi 0.84.3+, Node 22.19+ (Pi's own need); refuse launch args that change mode or select a session; `--provider` needs `--model`; update `pi update`; service tiers → speed picker (`service_tier`); session files back resume, rollback, forks; extension dialogs (select, confirm, input, editor) in the composer; skills in `$`. |
| Muse | `muse serve --trust-workspace [--disable-sandbox]`; Muse HTTP API | `driver/muse` | Off by default; `META_API_KEY` only from the instance env; status skips login; Supervised and Full access only; goes on without Agent Code tools if it cannot reach them; skills not in `$`. |
| ACP Registry | Registry download or local command; ACP v2 preview, falls back to v1 | `driver/acp` | Search shows only agents that run on the connected server; native drivers hidden; flavors claudeAcp, copilotAcp, devinAcp, droidAcp, kimiAcp, ompAcp, opencodeAcp; `CLAUDE_AGENT_ACP_EXPERIMENTAL_V2=0` forces v1. Registry cached in `AgentAcpRegistryEntry`. |
| Local ACP command | Executable plus one literal argument per row; no shell | `driver/acp` | Settings › Providers › Add provider › Show more. |

Default binary names: `codex`, `claude`, `grok`, `kiro-cli`, `fx`, `muse`, `opencode`, `pi`.

Default models, as T3: Codex its constant (text generation `gpt-6-luna`, reasoning low); Claude `claude-opus-5-5`
(text generation `claude-haiku-5-5`); Cursor `auto` (`composer-2`); Grok `grok-build`; fx, Kiro, Muse, ACP, Pi
`default`; OpenCode `openai/gpt-5` (`opencode/big-pickle`); Antigravity its constant. Slug aliases go in
`LlmModelAlias`.

### 5.1 ACP client

`crates/moqui-code/src/acp/`: the Agent Client Protocol over stdio (JSON-RPC), v2 preview with v1 fallback. It
gives `fs/read_text_file` and `fs/write_text_file` inside the work root only, `terminal/*` through the Agent Code
terminal manager ([06](06-Terminal-Preview-Devices.md)), `session/request_permission` to `AgentApproval`, and
`session/update` to the mirror. Flavor modules set the launch command, modes and quirks.

## 6. Install, update, maintenance

Port of `providerMaintenanceRunner.ts` into `driver/common/maintenance.rs`.

| Install kind | Update rule |
|---|---|
| Homebrew, npm | Ownership proven by the real path; npm with `--prefix` |
| pnpm, Bun, Yarn, Vite+, Volta | Each its own ownership rule |
| mise | Checked first; `mise upgrade` |
| Nix, Chocolatey | Manual |
| No proven owner | `pi update --self`, `grok update` |
| Claude, Codex, OpenCode native installers | Manual unless the native layout is proven |
| Managed (Antigravity) | Immutable release folders, atomic pointer, lease; continues after reconnect |

- `AgentProviderInstall` holds the result. The Providers settings page shows Install, Update and the output.
- An update needs `AgentCodeAdmin` and runs as an effect on the instance's host member.
- An update of the installed CLI restarts the instance's sessions only when no run is active, else after.

## 7. Usage limits and attachments

- Usage-limit errors are classified `usage_limit` with a reset time. Drivers with limits: Claude, Codex, Grok, Pi
  and others.
- Extra usage source: CLIProxy (`AgentUsageLimitSource`: label, url, management key as a secret, enabled).
- Limits per account in `AgentProviderAccount.limitsJson`; shown on the Usage page and in the composer.
- `provider.consumeResetCredit` → service `consume#ResetCredit` where the driver offers it.
- Attachments live outside the work root, in `<agent-code@home>/attachments/<conversationId>/` (or `ContentBlob`).
  The attachment prompt names the file paths the harness may read.

## 8. Text generation

| T3 | MR Moqui |
|---|---|
| Tasks: titles, commit messages, PR title and body, branch names, voice cleanup, review guides | `LlmFeatureBinding` features `code-title`, `code-commit`, `code-pr`, `code-branch`, `code-voice-cleanup`, `code-merge-back`. Review guides always use a coding harness (Codex or Claude) or the native engine with file tools. |
| Default: a coding agent writes the text, one session per request | Default: the native engine with the bound model; if none, a one-shot harness session. |
| A text generator is a plain model API (OpenAI, Anthropic, OpenRouter, OpenAI-compatible) with the key on the environment; **Test** sends a tiny prompt | `LlmEndpoint` rows already are this. **Test** calls `test#LlmEndpoint`. |
| Settings › Threads › Text generation model; Source control and Voice can pick another | Settings bound to the features above. |
| A managed policy that limits providers turns off text generators | `AgentCodePolicy` `providers.allowed`. |

Prompt builders ported from `textGenerationPrompts.ts`: commit message JSON `{subject, body[, branch]}`, subject
imperative, at most 72 chars, no period, staged summary 6,000 chars, patch 40,000 chars, policy
`commitInstructions`; PR content (base and head, commit summary, diff summary, patch, change request template,
thread context, policy); review guide; branch name; thread title (project title and fork source).

## 9. Native engine tools for coding

So the native engine can do what a harness does, it gets these tools when the conversation has a work root
(`crates/moqui-code/src/tools/`, each with `submit_tool!` and a classification):

| Tool | Classification |
|---|---|
| `read_file`, `list_files`, `grep`, `glob` | Read |
| `write_file`, `apply_patch`, `edit_file` | Write |
| `run_command` (PTY through the terminal manager; output streamed) | External (by runtime mode preset) |
| `git_status`, `git_diff`, `git_log` | Read |
| `git_commit`, `git_branch` | Write |
| `git_push` | External |
| `web_fetch`, `web_search` | External |

The runtime mode presets set `AgentToolPolicy` rows: approval-required asks for every Write and External;
auto-accept-edits allows Write in the work root and asks for External; auto allows both and asks for Destructive;
full-access allows all, and needs the project setting and `AgentCodeHost`.

## 10. Session import and outside agents

- Import sources `claudeAgent`, `codex`, `pi`: scan → project candidates with git info → import. Service
  `scan#HarnessSessions` and `import#HarnessSessions` read the native histories on the host member and write
  conversations with `AgentHarnessThread.importedDate`.
- Outside agents (Claude Code, Codex in a terminal, ChatGPT, bots) use `<server>/mcp` with OAuth
  ([08 §1](08-MCP-Tools.md#1-transport-auth-and-injection)). The palette has **Use Agent Code in another app (MCP)**.
  Cloud assistants reach the server through the edge node. An older client uses a one-time 5-minute code.

## 11. Testing the drivers

Port of `t3:packages/provider-testing/src/` into `crates/moqui-code/src/driver/testing/`:

- `fake_cli.rs`: a scripted fake harness process per protocol (Codex JSON-RPC, stream-JSON, ACP, Pi RPC, HTTP).
- `replay.rs`: replay fixtures recorded from real sessions: `simple`, `multi_turn`, `queued_turn`, `turn_interrupt`,
  `message_steering`, `provider_thread_resume`, `tool_call_*`.
- A recorder: `moqui code record-fixture --driver <name> --scenario <name>` (needs the real CLI; not run in tests).
- The regression suite runs the fakes and replays only. See [15-Tests](15-Tests.md).
