# 02. Orchestration

Threads, runs, the command pipeline and the lifecycle rules. Part of [Agent Code](README.md). T3 sources:
`t3:apps/server/src/orchestration-v2/`, `t3:packages/contracts/src/orchestrationV2.ts`,
`t3:docs/orchestration-v2/`.

## 1. The command pipeline

| T3 | MR Moqui |
|---|---|
| `orchestration.dispatchCommand` takes one command with a `commandId` | Service `org.moqui.agent.code.CommandServices.dispatch#Command` (in: `command` map with `type` and `commandId`; out: `changeSeq`, `result`). Also the `/agentws` method `command.dispatch`. |
| A duplicate `commandId` returns the stored result | `AgentCodeCommand` receipt. Same rule. The receipt is written in the same transaction as the change. |
| Per-thread lock in the decider | A row lock on `AgentConversation` (`for-update`) inside the command transaction. Cluster-safe, as the database holds the lock. |
| Events + projections + receipt + outbox in one transaction | Entity writes + `AgentCodeChange` row + `AgentCodeCommand` receipt + `AgentCodeEffect` rows in one transaction. |
| `EffectWorker` claims outbox rows after commit | `AgentCodeEffectWorker` (Rust, `crates/moqui-code/src/effect.rs`): a leased claim loop on `AgentCodeEffect`, woken by a cluster notify on commit. |
| Retry backoff blocks later effects of the same thread | Same rule: the claim query skips a thread whose oldest open effect is in backoff. |
| Failed rows are kept; done rows trimmed after about a week | Same. Job `clean#AgentCodeEffects` daily. |
| Internal-only commands | The same types are accepted only from services with `authenticate="anonymous-all"` off and an internal flag (`ec.context.internalCommand`). `dispatch#Command` refuses them from clients. |

Each command type is one Rust handler in `crates/moqui-code/src/command/` (one file per group) behind one service
dispatcher. Each handler checks the permission and the session scope (see [14-Security](14-Security.md)) before it
writes.

### 1.1 Client commands

| T3 command | Handler and effect in MR Moqui |
|---|---|
| `thread.create` (threadId, projectId, title, modelSelection, runtimeMode, interactionMode, branch, worktreePath, importedNativeThread) | Create `AgentConversation` (engine from the provider instance), `AgentConversationUser` for the creator, optional `AgentHarnessThread` for an import. No harness process starts (lazy, as T3). |
| `thread.stop` (reason, stopDelegatedTasks) | Interrupt the active run, hold the queue, stop child tasks depth-first (§5). |
| `thread.archive` / `thread.unarchive` | Status AgcArchived. Archive needs the delegated tasks to be done or cancelled. Archive also archives and stops side chats. |
| `thread.delete` (onlyIfUnused) | Status AgcDeleted. Deletes side chats and the triggers that post into it. `onlyIfUnused` deletes only an empty draft. The worktree follows the cleanup rule `worktreeOnDelete`. |
| `thread.settle` (expectedLatestRunId, settledAt) | §3. Refused when the latest run is not the expected one. |
| `thread.auto-settle` (snapshotAt) | Internal job only. Refused when the thread changed since the snapshot or has a user override. |
| `thread.unsettle` (restoreUnsettledAt) | Clear `settledDate`; restore order to top. |
| `thread.snooze` / `thread.unsnooze` | `AgentConversationUser.snoozeUntilDate`. |
| `thread.auto-settle.set` (enabled) | `AgentConversation.autoSettleDisabled`. |
| `thread.pin`, `thread.unpin`, `thread.pin.reorder`, `thread.active.reorder` (orderKey) | `pinnedOrderKey`, `activeOrderKey` on `AgentConversationUser`. Fractional keys from `crates/moqui-code/src/order_key.rs` (port of T3's fractional index). |
| `thread.tag.set` (tag <= 64 chars, single line, exact match; tagged) | `AgentConversationTag` insert or delete. |
| `thread.visit` (visitedAt, opened, searchQuery <= 200) | `lastVisitedDate`, `readThroughSeq`; search query feeds palette frecency. |
| `thread.mark-unread` | `markedUnread` = Y. |
| `thread.metadata.update` (title, regenerateTitle, branch, worktreePath, expected values, continuation, expectedEmpty, limitRecovery, linkedPullRequest) | Field updates with compare-and-set on the expected values. `regenerateTitle` adds an effect `title.generate`. |
| `thread.pull-request.link`, `unlink`, `link.sync`, `watch`, `sync` | `AgentConversationLink`, `AgentPrWatch` ([04-Git](04-Git.md)). |
| `thread.issue.link`, `unlink` | `AgentConversationLink` kind issue. |
| `thread.title.regeneration.complete` | Internal: set title, `titleSourceEnumId` AgtsGenerated. |
| `thread.runtime-mode.set`, `thread.interaction-mode.set`, `thread.model-selection.set` | Field update. Runtime mode raise is checked against the policy ceiling (§4.4). |
| `provider-session.detach` | Effect: stop the harness session; `AgentHarnessSession` AghsStopped; thread keeps its native ref. |
| `message.dispatch` (messageId, text, context, attachments, modelSelection, ... dispatchMode) | §4. Writes `AgentMessage` and parts, then an `AgentRun` and an `AgentInput`, by dispatch mode. |
| `prepared-run.release`, `progress`, `fail`, `retry` | `AgentRun` AgrPrepared plus `AgentCodeLaunch` progress (§9). |
| `notification.delivery.accept` | Ack of an at-least-once notification message (T3 `NotificationMailbox`). Marks the `NotificationMessageUser` row. |
| `subagent.stop` (runId, subagentId) | Adapter `stop_subagent`; for app subagents, stop the child run. |
| `run.interrupt` (runId, reason, holdQueue) | §5. |
| `queued-message.promote-to-steer` | Move a queued run's message into the active run as steer input. |
| `queue.resume`, `queued-run.start`, `reorder`, `cancel`, `edit` | Queue operations on `AgentRun` AgrQueued rows (§4.2). |
| `runtime-request.respond` (requestId, decision, feedback, answers, attachmentsByQuestionId) | `decide#AgentApproval` or `answer#AgentQuestion`; the adapter's `respond_runtime_request`. Fails `not_resumable` when the live callback is gone. |
| `thread.user-input.dismiss` | `AgentQuestion.dismissedDate`. |
| `checkpoint.rollback` (scopeId, checkpointId, restoreFiles) | §7. |
| `thread.fork`, `thread.merge_back` (sourcePoint latest_stable/run/checkpoint, title, sideChat) | §8. |
| `delegated_task.request`, `wake-policy`, `completion-delivery.acknowledge`, `dispose` | §10. |
| `thread.created.record` | Internal: record an agent-created thread on the parent run (`AgentRunEvent` thread.created). |
| `provider.switch` (modelSelection) | §8.3. |
| `thread.guest-share.close`, `thread.guest-message.dismiss` | [07 §3](07-Secrets-Guest-Voice-Usage.md#3-guest-share). |

Internal-only commands, as in T3: `run.output.submit`, `thread.guest-share.open|redeem`,
`thread.guest-message.submit`, `thread.project.adopt-folder`, `thread.pull-request-watch.checks-reopen|sync`,
`delegated_task.remote.request|complete|stop`, `checkpoint.rollback.fail`, `runtime-request.delivery.fail`,
`thread.interrupt.unresponsive`, `thread.background-work.settle`, `secret_request.record`, `link_request.record`.

## 2. Thread creation and launch

| T3 rule | MR Moqui |
|---|---|
| Creation is separate from provider work; the harness starts at first run | Same. `AgentHarnessSession` is opened by the first run's effect. |
| `ThreadLaunchService`: thread plus initial message in one commit | Service `launch#Thread`: one transaction writes the conversation, the message, the run (AgrPrepared when a worktree is needed) and the launch row. |
| Workspace strategy `root {branch}`, `existing_worktree {worktreePath, branch}`, `worktree {baseRef, branch, startFromOrigin, repositories}` (default all tracked folders, none above 8) | `workspaceStrategy` map, same shapes. `worktree` writes `AgentWorkRoot` and an `AgentCodeLaunch`. Base ref and branch name are checked before the worktree is made. |
| Placeholder title, replaced by the first message | `titleSourceEnumId` AgtsPlaceholder, then effect `title.generate` ([03 §8](03-Providers.md#8-text-generation)). |
| Fork title uses inherited history | Same, in the title prompt. |
| `threadTitleInstructions` (project override) | `AgentCodeSetting` plus `AgentProjectSetting`. |
| Scratch threads: folder `<home>/scratch/<date>-<first-words>-<8-hex>`; delete keeps folder; no git controls; "Turn into project" | `AgentWorkRoot` AgwkScratch under `agent_code_home/scratch`. "Turn into project" is `adopt#ScratchFolder` (internal command `thread.project.adopt-folder`). Not offered when the data dir is inside a git checkout. Key `mod+alt+n`. |
| Agent-created threads (`create_threads`, `t3_thread_launch`) | Same service, `creationSourceEnumId` AgcsMcp, recorded on the parent run. |

`agent_code_home` is a new property: default `runtime/agent-code/` on a server, the app data directory on desktop.
It holds `scratch/`, `worktrees/`, `projects/`, `artifacts/`, `browser/`, `voice/` and `logs/`.

## 3. Settle, snooze, pin, tags, unread

| T3 rule | MR Moqui |
|---|---|
| Manual settle removes pin, dismisses unanswered async questions, closes idle terminals, runs `runOnSettle` scripts in the worktree, stops `stopOnSettle` actions | Service `settle#Thread`. Each step is an effect. A failed script logs and does not block the settle. |
| Auto-settle after `sidebarAutoSettleAfterDays` (1-90, default 7, null off) | Service job `autoSettle#Threads`, hourly. Per user, from `AgentConversationUser`. |
| Auto-settle on merged PR (`sidebarAutoSettleOnMerge` true); closed PR can settle an idle thread; `sidebarAutoSettleKeepPullRequests` false | The PR sync effect calls `autoSettle#Thread` when every linked review is terminal. |
| Blockers: running work, pending approvals or questions, waiting guest message, PR watch, background work | Same checks in `check#SettleBlockers`. |
| Only user messages count as resuming | Same. |
| Rules run when every app is closed | Same: the job runs on the server. |
| Snooze presets (max 8): 60 min, 180 min, today 18:00, tomorrow 09:00, monday 09:00; kinds `after {minutes <= 30 days}`, `at {day, HH:MM}` | `snoozePresets` client setting, same defaults and limits. Times are in the user's time zone (`UserAccount.timeZone`). |
| Wake early on approval, question, failed run, user message or answer, waiting guest message | The run event handler clears `snoozeUntilDate` for those events. Job `wake#SnoozedThreads` each minute for timed wakes. |
| Undo toast 5 s for unpin, settle, snooze, archive, discard draft (`mod+z`) | Client keeps the inverse command for 5 s. |
| Working shelf (`sidebarWorkingShelfEnabled` true) | Client groups threads with running work or resumable waits. |
| Tags hidden by `hiddenThreadTags` and `hideTaggedThreads` | Environment settings; the shell stream filters. |
| Unread: `thread.visit` moves the watermark; capability `threadVisitedTracking` | `readThroughSeq` against `AgentConversation.lastMessageSeq`. |
| Side chats: `thread.fork sideChat:true`; `/side` or `/btw`; parent activity after the fork point prepended to first turn; not in the sidebar; closing an unused one deletes it | `relationEnumId` AgcrSide. The handoff builder (§8.4) adds the parent delta. |
| Search: FTS over messages; a word of 3 letters (2 for CJK); recency by the user's last message; palette frecency, last pick kept 3 weeks | Service `search#Threads` on `AgentConversation.searchText` and `AgentMessagePart.textValue` through the entity search index; same word rules. Frecency in `UserPreference` `code.palette.frecency`. |

## 4. Messages, dispatch modes and the queue

### 4.1 `message.dispatch`

1. Write `AgentMessage` (role user) and parts. Attachments go to the attachment store (§11).
2. Pick the dispatch mode:

| `dispatchMode` | Action |
|---|---|
| `start_immediately` | New `AgentRun`; start effect. |
| `queue_after_active` | New `AgentRun` AgrQueued with `queueOrderKey` after the last. |
| `steer_active {targetRunId}` | `AgentInput` kind steer on the target run; adapter `steer_turn`. |
| `restart_active {targetRunId}` | Interrupt the target, then a new `AgentRunAttempt` with reason steering_restart. |
| `defer_start {workspaceStrategy}` | `AgentRun` AgrPrepared; waits for `AgentCodeLaunch` AgclReady. |

3. Policy (port of `CommandPolicy.ts`): steer in place when the harness declares `supportsActiveSteering`; else
   interrupt and restart with a new attempt when it declares `supportsSteeringByInterruptRestart`; else queue.
   Compaction and sign-out turns take no steer. A goal command into active work stops that work.
4. Client setting `followUpBehavior` (default `steer`, else `queue`) picks the default mode.

The native engine has steer today (`AgentInput` with `clientMessageId`). The same rule applies to it.

### 4.2 Queue

- Queued runs are ordered by `queueOrderKey`. Delegated completions interleave by request time (port of
  `QueuedRunOrder.ts`).
- `run.interrupt holdQueue` and a restart hold the queue (`queueHeld`). `queue.resume` releases it.
- Queued messages can be edited (text, attachments, context), reordered (`beforeRunId`), cancelled, started now,
  or promoted to steer.
- Desktop and phone outbox: a queued message written offline goes as outbox kind `agent-message` with its
  `commandId`. The receipt makes the replay safe ([12-Mobile §6](12-Mobile.md#6-offline)).

### 4.3 Run state machine

Port of `RunExecutionService.ts` and `RunFinalizationService.ts` into `crates/moqui-code/src/run/`:

```
AgrQueued ─► AgrSubmitted (starting) ─► AgrWorking (running) ◄─► AgrInputRequired (waiting)
   │              │                          │
   └─► AgrCancelled                          └─► AgrCompleted | AgrInterrupted | AgrFailed | AgrCancelled | AgrRolledBack | AgrLimited
AgrPrepared ─► AgrQueued or AgrSubmitted (on launch ready) | AgrFailed (on launch fail)
```

- Completion barrier, in this order: flush assistant text, close messages, finalize plans and todos, close done
  child tool calls, capture the checkpoint, write the terminal state, publish.
- A run is "running" before the harness accepts it. Only a harness turn ref proves the prompt arrived. Recovery
  treats a run without one as not started.
- The native engine already has this barrier in `run/step.rs`. Harness runs reuse the same finalize code.

### 4.4 Mode ceilings

- Administrator ceiling: `AgentCodePolicy` `limits.maxRuntimeMode`.
- Caller ceilings: the session's access level (MCP grant, peer link, guest) and the `X-Moqui-Mode-Limit` header on
  forwarded calls. Lowering is always allowed.
- Checked under the command lock (port of `DispatchModeLimit.ts`, `RuntimePolicy.ts`).
- A stored mode that the harness does not offer runs as Supervised (approval-required).

## 5. Interrupt, stop, recovery

| T3 rule | MR Moqui |
|---|---|
| `run.interrupt` only acknowledges; the terminal state comes from the harness | Same. `AgentRun.cancelRequested` = Y; adapter `interrupt_turn`. |
| Harness never answers: `thread.interrupt.unresponsive`; capability `threadForceStop` | After 15 s with no turn end, the process host kills the session and writes AgrInterrupted with reason `unresponsive`. |
| Stop button = `run.interrupt holdQueue`: holds queue, ends PR watches, drops owed wakes, stops child threads depth-first | Service `stop#Thread`; child stop as effects in depth-first order. |
| Recovery on restart, idle reaper or crash: session stopped/error, live handle dropped, native refs kept, resume on next work | `recover#HarnessSessions` at node start and when a run lease expires (`run/lease.rs`). |
| Resume failure: provider thread not loaded, replacement thread from reconstructed context, pending live requests `not_resumable` | Same. A replacement uses the portable handoff (§8.4). Open approvals get `notResumable` = Y. |
| Lost approval callbacks fail; persisted questions can be asked again | Same. |
| Restart continuation (`continueThreadsAfterServerUpdate`, default true); a turn that keeps crashing the server stops being retried (at most 3 tries) | Setting `continueThreadsAfterServerUpdate`. `AgentRunAttempt` rows count tries; the 4th is refused with error `restart_loop`. |
| Prepared-run retry after worktree or setup failure | `prepared-run.retry` restarts the `AgentCodeLaunch`. |

Cluster rule: a harness session lives on one member (`AgentHarnessSession.hostInstanceId`). When that member dies,
another member can resume only if the work root is on shared storage. Else the thread shows "Host offline" and
waits. See [13 §9](13-Servers-Linking.md#9-cluster-and-edge).

## 6. Usage-limit continuation and background work

| T3 | MR Moqui |
|---|---|
| "Limited" status when a harness stops on a usage or rate limit | AgrLimited with `limitResetDate`. The adapter classifies `usage_limit` errors with a reset time. |
| Resume (manual continuation, when the harness supports it) | `message.dispatch` with `manualContinuationOfRunId`. |
| Resume at reset | One-shot service job at `limitResetDate`. |
| Snooze until reset | `snoozeUntilDate` = reset time. |
| `autoResumeLimitedThreads` (false), `snoozeLimitedThreads` (false) | Environment settings. |
| Overdue continuations resume after restart; stop, new message, archive or settle cancel a pending one; an expired window starts no retry loop | Same. |
| Background work (monitors, background commands, PR watches, delegated tasks) | `AgentRunEvent` background.updated; `AgentRun` holds `backgroundWork` count. `t3_background_wait` tool ([08](08-MCP-Tools.md)). Background work blocks auto-settle and keeps the thread in Working. |
| Restart note | After a restart, a notice event is added to threads that had background work. |
| Background activity profile (balanced, performance, battery-saver, custom): `automaticGitFetchInterval` 30 s, `providerHealthRefreshInterval` 5 min, pause when host locked or on battery | Environment setting `backgroundActivity`. Desktop host gives the power state ([11 §7](11-Desktop.md#7-power-and-sleep)). Servers use the balanced profile. |
| `keepAwakeWhileAgentsWork` (true) | Desktop only: the host holds an OS sleep inhibitor while any run is active. |

## 7. Checkpoints and rollback

Port of `CheckpointService.ts`, `CheckpointCaptureService.ts`, `CheckpointRestoreSafety.ts` into
`crates/moqui-code/src/checkpoint/`, on `moqui-git`.

| Step | Rule |
|---|---|
| Baseline | Before the root execution, write the work tree as a tree object and keep it under a hidden ref `refs/moqui-code/checkpoints/<conversationId>/<checkpointId>`. Untracked files are included; ignored files are not. |
| Capture | After the run (completion barrier), capture again. Write `AgentCheckpoint` and one `AgentFileChange` per changed path, with the diff in a `ContentBlob`. |
| Child scopes | A subagent or tool scope is a child checkpoint (`parentCheckpointId`). Only root-run checkpoints advance the run count. |
| Scratch folders | Checkpointed the same way with a private git dir under `agent_code_home/checkpoints/`. |
| Turn diff | Diff against the prior checkpoint. Files brought in by pull, merge or rebase are left out unless the turn edited, committed or fixed a conflict in them. |
| Rollback `checkpoint.rollback {scopeId, checkpointId, restoreFiles}` | Safety check (no running work, no foreign changes since, or the user confirms), restore files, harness rollback if supported (Codex `thread/rollback`), later runs AgrRolledBack. Failure: internal `checkpoint.rollback.fail`. |
| Retention | Hidden refs are removed with the thread or by the cleanup job. |

The native engine gets checkpoints too when its conversation has a work root (file tools write in it).

## 8. Fork, merge-back, provider switch, handoff

### 8.1 Fork

- `thread.fork {sourceThreadId, sourcePoint (latest_stable | run | checkpoint), title, sideChat}`.
- Writes the new conversation with `parentConversationId`, `relationEnumId` AgcrFork, `forkPointJson`, and an
  `AgentContextTransfer` type fork, status pending.
- The first run resolves it: `native` when the adapter's `fork_thread` works, else `portable` (§8.4).
- The native engine forks by copying `AgentMessage` rows up to the fork point.

### 8.2 Merge-back

`thread.merge_back` summarizes the eligible child delta into the target as a context transfer of type merge_back.
The text is made by the text-generation feature `code-merge-back`.

### 8.3 Provider switch

- `provider.switch {modelSelection}` or a message with a new model selection.
- Same conversation. A new or reactivated `AgentHarnessThread`. The handoff is the full eligible history, or the
  delta since that harness thread last took part.
- Switching between the native engine and a harness works the same way. The native engine reads `AgentMessage`
  rows, so it needs no handoff text; a harness gets the portable handoff.

### 8.4 Portable handoff

Port of `ContextHandoffService.ts` into `crates/moqui-code/src/handoff.rs`.

| Rule | Value |
|---|---|
| Items | user and assistant messages, command executions, file changes, checkpoints, earlier handoffs |
| Budget | `agent_code_handoff_token_cap`, default 16,000 (1,024-64,000); 1 token per UTF-8 byte |
| Imported history ceiling | 64,000 bytes |
| Fallback window | 128,000 tokens; reserve max(16,000, 25%) |
| Attachments | images 8,192 tokens each; other files 4,096 |
| No room | Start a fresh harness conversation |
| Native injection | When the adapter has `inject_history` (Codex), inject; else prepend as text |
| `freshContext` message | New harness session without earlier context |

## 9. Worktrees

| T3 | MR Moqui |
|---|---|
| Thread env modes `local`, `worktree` | `AgentWorkRoot.kindEnumId` AgwkLocal, AgwkWorktree. |
| `defaultThreadEnvMode` (null inherits `t3.json`, then local) | Same resolution: project setting, environment setting, project file, built-in. |
| `newWorktreesStartFromOrigin` (true) | Same. Fetch before the worktree is made. |
| `worktreeSubmodules` recursive, top-level, none | Same, on `moqui-git` submodule support (or the `git` CLI when the crate lacks it; see open decision O6 in [16](16-Phases.md)). |
| `worktreesDirectory` (default `<home>/worktrees`) | `agent_code_home/worktrees/<project-slug>/<branch>`. |
| `fastWorktreeCheckout` (APFS clone cache, Git >= 2.42) | Desktop on macOS only. |
| Sparse checkout from `worktreeSparsePatternsPath` | Same field in the project file. |
| Branch naming `branchNamingMode` static (prefix `t3`), semantic, custom; `branchNameInstructions`; keep the temporary name when generation fails | Same modes. Static prefix default `moqui`. |
| `WorktreeBranchSync` follows branch renames | `sync#WorkRootBranch` on each VCS status refresh. |
| Worktree tools (`t3_worktree_list`, `status`, `handoff`) | [08-MCP-Tools](08-MCP-Tools.md). Handoff moves a thread between the local checkout and a worktree. |
| Setup progress, cancel | `AgentCodeLaunch`; `/agentws` stream `worktreeSetup`; `worktreeSetup.cancel`. Project scripts with `runOnWorktreeCreate` run in a terminal session. |
| Cleanup rules (`worktreeKeepWhen`, `worktreeAfterDays` 1-3650, `worktreeSettledAfterDays` 0-3650, `worktreeOnMerge`, `worktreeOnDelete`, `worktreeUnchanged`, `browserArtifactsAfterDays`, `logsAfterDays`) | Job `clean#AgentCodeStorage`: at start, on change, hourly. Only Agent Code worktrees. Active sessions and shared worktrees block. "Delete now" writes an `AgentCodeCleanupRun` report (removed, kept, failed, unused). |

Dev AIDE integration: a worktree made here shows in Dev AIDE (dev instances) through `DevAideRoot.workRootId`.
Dev AIDE's `create#Worktree` can hand its root to Agent Code.

## 10. Subagents and delegated tasks

| T3 | MR Moqui |
|---|---|
| Native (harness) subagents map to subagent nodes with nested provider threads; child completion never ends the parent | `AgentToolCall` toolKind subagent; a child `AgentHarnessThread` with `ownerToolCallId`; the parent run stays open. |
| `subagent.stop` | Adapter `stop_subagent`. |
| App delegated tasks (`delegated_task.request`, `delegate_task`): child thread with own workspace strategy; completion back to the parent as a message; wake `always` or `settled_only` (default) | `AgentDelegatedTask` plus a child conversation AgcrDelegated. The completion is a message on the parent with `senderConversationId`. The wake policy picks when the parent run starts. |
| Delivery acknowledge and dispose | `deliveryStatusId`. |
| Subagent threads cannot take messages; their approvals show in the parent | Same. The parent thread view lists child approvals. |
| Remote child through a peer link | `AgentDelegatedTask.linkId`; [13 §6](13-Servers-Linking.md#6-peer-links). |
| Native engine delegation | `moqui-llm` already has `parentRunId` and A2A delegation. `delegate_task` from a native agent uses the same service. |

## 11. Projects

| T3 | MR Moqui |
|---|---|
| Project = folder on one environment; same repository on several environments groups by repository identity (`sidebarProjectGroupingMode` repository, repository_path, separate) | `AgentProject.rootLocation` on `hostInstanceId`; grouping by `repositoryKey` across linked environments in the client. |
| Covers every repository in the folder (one level down, one more in non-repo folders); extra folders (max 128); `.code-workspace` suggestions; `ignoreRepositories`; submodules as own repositories | `scan#ProjectRepositories` writes `AgentProjectFolder` rows. |
| Clone: base dir `addProjectBaseDirectory`; `ghqCloneLayout` `<base>/<host>/<owner>/<repo>`; clone in background; prompt can be written first; send waits for files; cancel, retry; SSH refused falls back to HTTPS | `clone#Project` effect on `moqui-git` fetch; progress on the `projectClone` stream. |
| New project: git repo in `<home>/projects/<slug>` with README, icon, first commit; option to create a private repo on GitHub | `create#NewProject`; the hosted repo can also be a `GitRepository` on this server or a linked group server. |
| Publish repository (create hosted repo, add origin, push) | `publish#ProjectRepository` for GitHub and for MR Moqui hosting (`GitRestServices`). |
| Pin and archive (all machines connected) | `pinnedOrderKey`, `archivedDate`. |
| Icons: auto-detect, emoji, monogram, image | `iconKindEnumId`, `iconValue`, `iconLocation`. |
| Project file `t3.json` (schema `https://t3.codes/schema/t3.json`) | Read `moqui-code.json` first, then `t3.json`. Same fields. Schema served at `/schema/moqui-code.json`. |
| Scripts in settings (`defaultProjectScripts`, `projectScriptOverrides`), import from the file | `AgentProjectScript` rows with `source`. |
| Auto pull (`defaultAutoPull` false; fast-forward, clean checkout, default branch with upstream) | Job per project on the background-activity interval. |
| Resolution order: project override, environment setting, project file, built-in | Service `get#EffectiveSetting` with the same order. |
| Members | New in MR Moqui: `AgentProjectMember`. A project's threads are visible to members. T3 has one owner, so its UI shows no member list; Agent Code adds one in Project settings. |

## 12. Idle compaction

| T3 | MR Moqui |
|---|---|
| Claude only; modes "When you return" (Compact chip, default) and "Before the cache expires (beta)"; threshold 200K, not below 100K; timers in memory, restart skips them | Same for the Claude adapter. Timer as a one-shot in-memory task in the process host. |
| Visible `/compact` turn | A run with `idleCompaction` = Y. |
| Native engine | Already compacts (`compactedThroughSeq`, `compactionSummary`). The Compact chip calls `compact#AgentConversation` for native threads. |

## 13. Other orchestration services

| T3 | MR Moqui |
|---|---|
| `CommandOutputHub` (live command output) | Live-only `command.output` events on the run hub; stored output in the spill file. |
| `LiveStreamBudget`, `ThreadLiveEventCoalescer`, `responseStreamingMode` paragraph (default) or turn | `run/events.rs` coalescing; a paragraph mode flushes on blank lines; turn mode sends text at turn end. |
| `ShellStream`, `ThreadStream`, `WireProjection`, history paging | `/agentws` streams `shell` and `thread` with bounded snapshots and pages ([09](09-Wire-API.md)). |
| `ThreadViewers` | Viewers from `/agentws` subscriptions, sent as `thread.viewers`. |
| `NotificationMailbox` (at-least-once) | `NotificationMessage` plus `notification.delivery.accept`. |
| `ResourceCleanupService` (device claims released at thread end) | Effect `release#ThreadResources`. |
| Legacy V1 importer | Not needed. A one-time import from a T3 `statev2.sqlite` is offered as `import#T3State` (§14). |
| Activity log summaries ("Sent 5 messages to 2 threads"); `toolCallVisibility` showAll, hideSuccessful, hideAll, zen | Summaries made in the thread view from `AgentToolCall` rows of toolKind orchestration. Client setting with the same values. |

## 14. Import from T3 Code

A user who moves from T3 gets their data. Service `import#T3State` (admin, one time per environment):

| T3 source | Target |
|---|---|
| `statev2.sqlite` projections (threads, runs, messages, turn items, projects, checkpoints, PR links) | The entities of [01](01-Data-Mapping.md), with `creationSourceEnumId` AgcsImport |
| `settings.json` | `AgentCodeSetting`, `AgentProviderInstance` |
| `keybindings.json` | `UserPreference` `code.keybindings` |
| scheduled tasks | `AgentTrigger` |
| `worktrees/` | `AgentWorkRoot` rows pointing at the same folders |
| secrets | Not imported. The user enters them again. |

The importer reads the SQLite file read-only and logs each failed row by id, then goes on.
