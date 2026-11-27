# 15. Tests

Part of [Agent Code](README.md). Rules: [.agents/skills/testing/SKILL.md](../../.agents/skills/testing/SKILL.md),
[docs/Build-Test.md](../../docs/Build-Test.md#the-two-suites), [docs/dev/JS-Testkit.md](../../docs/dev/JS-Testkit.md).

Agent Code is framework work: the component is in `runtime/components/base/agent-code` and the crate is
`crates/moqui-code`. Before a commit run `./moq test-framework`. While working run the specs of what changed
(`./moq test agent-code --spec ...`, `cargo test -p moqui-code <filter>`).

## 1. Where tests go

| Kind | Place | Suite |
|---|---|---|
| Rust unit tests | `crates/moqui-code/src/**/<name>_tests.rs`, `crates/moqui-desktop/src/code/*_tests.rs`, `crates/moqui-client/src/code/*_tests.rs` | regression (`cargo test` in `test-framework`) |
| Host integration tests | `crates/moqui-host/tests/it/code_*.rs` (the one `it` binary) | regression |
| Service and rule specs | `runtime/components/base/agent-code/test/*.spec.js` | regression |
| Client specs | `runtime/components/base/agent-code/test/client/*.spec.js` | regression (one golden-path walk only) |
| Scenarios, walks, sweeps, load | `runtime/components/base/agent-code/test/acceptance/**` | acceptance |
| Real-CLI checks (needs codex, claude, ...) | `#[ignore = "needs <cli>"]` or an opt-in `[[test]]` | acceptance (`./moq accept-all`) |

Budget: a new `test/budget` for `agent-code`. Start value set from the first full phase-1 run and agreed with the
user. No spec over 15 s; a Rust test over 15 s is ignored or opt-in.

## 2. Test tools

| Tool | Place | Use |
|---|---|---|
| Fake harnesses | `crates/moqui-code/src/driver/testing/fake_cli.rs` ([03 §11](03-Providers.md#11-testing-the-drivers)) | Scripted Codex JSON-RPC, Claude stream-JSON, ACP, Pi RPC and HTTP agents. A spec picks a script (`simple`, `approval`, `question`, `interrupt`, `crash`, `usage_limit`, ...). No real model, no network. |
| Replay fixtures | `crates/moqui-code/src/driver/testing/fixtures/` | Recorded sessions replayed through the adapter. |
| Native engine fake model | existing `moqui-llm` test provider | Native-engine threads. |
| Fake code host | `crates/moqui-code/src/scm/testing/fake_host.rs` | A small HTTP fake of the GitHub and GitLab APIs used (PRs, checks, comments, reviews). Local git repos in a temp dir. |
| Socket client | `testkit` `ec.test.agentws(user)` (new testkit member; add to [JS-Testkit](../../docs/dev/JS-Testkit.md) and `moqui-host.d.ts`) | Opens `/agentws` as a user or key, sends requests, reads streams. |
| Clock | existing testkit clock | Automations, TTLs, cleanup, backoff. |
| Fake push | existing FCM test sink plus an APNs sink | Push payloads. |

## 3. Regression tests by area

One test per rule and per refusal. Access: one holder and one non-holder per verb, and another user's record refused.

| Area | Tests |
|---|---|
| Data mapping ([01](01-Data-Mapping.md)) | Entity load and the enum seed; `AgentConversation` new fields round trip; import from a T3 database fixture (one small SQLite file) maps threads, runs, items, projects. |
| Command pipeline ([02 §1](02-Orchestration.md#1-the-command-pipeline)) | Same `commandId` twice gives the first result; change feed sequence has no gaps; effect retry and lease expiry; a failed effect stays. |
| Thread states ([02 §3-5](02-Orchestration.md#3-settle-snooze-pin-tags-unread)) | Settle, snooze, wake, pin order; auto-settle rules (days, keep PRs, on merge); queue, steer, interrupt, resume; restart continuation rules (max 3 resends, refused for settled and waiting threads). |
| Worktrees and checkpoints ([02 §7](02-Orchestration.md#7-checkpoints-and-rollback), [§9](02-Orchestration.md#9-worktrees)) | Create from origin, from local; checkpoint capture and rollback in a temp repo; cleanup skips held threads. |
| Subagents and links ([02 §10](02-Orchestration.md#10-subagents-and-delegated-tasks)) | Delegated task delivery states; remote child follow-up; fencing by `linkId`. |
| Providers ([03](03-Providers.md)) | Each driver against its fake: start, turn, approval, question, interrupt, crash, resume, usage limit; capabilities parse; env is cleaned (no server variable reaches the child); per-user home. |
| Git ([04](04-Git.md)) | Branch name rules, PR create and merge against the fake host, checks mapping, files viewed, PR watch wake rules, stack order. |
| Automation ([05](05-Automation.md)) | See [05 §8](05-Automation.md#8-tests). |
| Terminal, preview, devices ([06](06-Terminal-Preview-Devices.md)) | Thread terminal needs `AgentCodeHost`; preview tab ownership and take-back; tunnel refuses a port not owned by the thread; HTML render sandbox headers. |
| Secrets, guest, voice, usage ([07](07-Secrets-Guest-Voice-Usage.md)) | Secret request delivery ref and file; file expiry; guest token check and read-only; guest message waits for the owner; usage rollup and price override. |
| MCP tools ([08](08-MCP-Tools.md)) | Tool list per capability; session key scope and expiry; `t3_*` aliases answer; thread-acting tool refused without a thread; link forwarding narrows the mode. |
| Wire ([09](09-Wire-API.md)) | Frames: answer, error with `requiredScope`, stream, cancel, ping timeout; limits (streams, queue full); protocol mismatch messages; descriptor has no user data; every method has a scope (table test). |
| Web UI ([10](10-Web-UI.md)) | Settings schema refuses unknown names and bad types; keybinding `when` parser and last-match rule; custom CSS sanitizer. |
| Desktop ([11](11-Desktop.md)) | Rust unit tests in `code/*_tests.rs` ([11 §11](11-Desktop.md#11-tests)). |
| Mobile ([12](12-Mobile.md)) | Rust unit tests in `moqui-client/src/code/` ([12 §10](12-Mobile.md#10-tests)). |
| Servers and links ([13](13-Servers-Linking.md)) | Pairing code life, tries, one use; exchange narrows scopes; DPoP proof window and `jti` reuse; device approval; peer link create failures (`wrong_environment`, `code_refused`); SSH launch script in a temp `HOME` with a fake `moqui` (pick port, reuse, managed vs external, refuse on production). |
| Security ([14](14-Security.md)) | The proof tests of [14 §10](14-Security.md#10-proof-tests). |

### 3.1 Screens

- Render rule: each Agent Code screen at most once per mode in the whole regression suite, all checks on that render.
  A second mode only for screens with their own tui twin (`Workbench.tui.json`, `PullRequests`, `Usage`).
- No regression test walks every screen. No screen sweep outside acceptance.
- One golden-path client walk (`test/client/AgentCodeGolden.spec.js`): sign in, new thread with the fake Codex,
  send, see the streamed answer, approve one command, see the diff, settle the thread.

## 4. Acceptance tests to write

Each is a plan "usable when" scenario. They run only when asked or when written or changed.

| Test | Scenario |
|---|---|
| `AgentCodeScenario.spec.js` | Project, worktree thread, two turns, PR to the fake host, merge, auto-settle. |
| `AgentCodeAccess.spec.js` | Isolation walk: two users, two projects; every route and tool refuses the other user's thread. |
| `AgentCodeSweep.spec.js` (qvt) and `AgentCodeSweepTui.spec.js` | One screen sweep per mode. |
| `AgentCodeBrowser.spec.js` | Browser walk in headless Chromium: sidebar, composer keys, diff pane, right panel, settings search; phone width (390 px) pass. |
| `AgentCodeHarnessReal.spec.js` | Real Codex and Claude CLIs when present (skipped when absent). |
| `AgentCodeLinks.spec.js` | Two instances: pair a link, start a thread through the link, fencing, read the linked thread stream. |
| `AgentCodeEdge.spec.js` | Local node behind an edge node: webhook while the local node is down, replay; push through the edge. |
| `AgentCodeLoad.spec.js` | 50 users, 200 threads, 20 running fake harness turns; change feed lag and socket memory under limits. |
| Desktop installed-app walk | Open Agent Code in the in-process node; preview tab pick; sleep inhibitor on while a run works; quit order. |
| Android installed-app walk | Pair by QR; thread push opens the thread; reply from the notification; offline send then flush. |
