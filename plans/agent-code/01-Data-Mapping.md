# 01. Data mapping

Every T3 Code record, field and enum mapped onto MR Moqui. Part of [Agent Code](README.md).

Legend for "How":

| Mark | Meaning |
|---|---|
| **Use** | The existing entity or field holds it as it is. |
| **Ext** | Add fields or enum values to an existing entity. |
| **New** | A new entity. |
| **Calc** | No table. Computed from other rows, or kept in memory. |
| **Res** | A file or `ContentBlob`, located from a row. |

New entities are in package `moqui.agent.code` (file `runtime/components/base/agent-code/entity/AgentCodeEntities.xml`)
unless the row names another package. Existing entities are in
`runtime/components/base/framework/entity/` (`AgentEntities.xml`, `LlmEntities.xml`, `SecurityEntities.xml`,
`ResourceEntities.xml`, `TerminalEntities.xml`, `EdgeEntities.xml`, `ServerEntities.xml`).

## 1. Why not an event store

T3 is event-sourced: a decider writes events, projections and an effect outbox in one SQLite transaction
(`t3:apps/server/src/orchestration-v2/EventSink.ts`). MR Moqui keeps entities as the system of record:

| T3 piece | MR Moqui piece | Reason |
|---|---|---|
| Stored event with a monotonic `sequence` | `AgentRunEvent` (per run) plus `AgentCodeChange` (per environment, §9) | Clients resume from a cursor, as in T3. |
| Projection tables | The entities themselves | One store. Screens, services, audit and the System › LLM screens read them. |
| `CommandReceiptStore` (duplicate `commandId`) | `AgentCodeCommand` (§9) | Same receipt rule: a repeated `commandId` returns the first result. |
| `EffectOutbox` and `EffectWorker` | Service jobs plus the agent worker pool (`run/pool.rs`), with `AgentCodeEffect` rows (§9) | Same order rule per thread: a failed effect blocks later effects of that thread. |
| Event pruning after about 1 day | `AgentCodeChange` prune job keeps 1 day; older cursors get a snapshot | Same as T3. |

## 2. Threads

| T3 (`t3:packages/contracts/src/orchestrationV2.ts`) | MR Moqui | How |
|---|---|---|
| AppThread.id | `AgentConversation.conversationId` | **Use** |
| projectId | `AgentConversation.projectId` → `AgentProject` | **Ext** |
| title, titleRenamed | `title`, new `titleSourceEnumId` (AgtsPlaceholder, AgtsGenerated, AgtsUser) | **Ext** |
| defaultProvider, modelSelection {instanceId, model, options} | `providerInstanceId`, `modelId` (→ `LlmModel` when known), new `harnessModel`, `reasoningLevel`, `modelOptionsJson` | **Ext** |
| runtimeMode | `runtimeModeEnumId` (AgrmApprovalRequired, AgrmAutoAcceptEdits, AgrmAuto, AgrmFullAccess) | **Ext** |
| interactionMode | `interactionModeEnumId` (AgimDefault, AgimPlan) | **Ext** |
| branch, worktreePath | `workRootId` → `AgentWorkRoot` (§4); `branch` copied for lists | **Ext** |
| activeProviderThreadId | `harnessThreadId` → `AgentHarnessThread` | **Ext** |
| lineage.parentThreadId, relationshipToParent (fork, subagent), rootThreadId | `parentConversationId`, `relationEnumId` (AgcrFork, AgcrSubagent, AgcrSide, AgcrDelegated, AgcrLinked), `rootConversationId`, `forkPointJson` | **Ext** |
| createdBy (user, agent, system), creationSource | `createdByKindEnumId`, `creationSourceEnumId` (AgcsUi, AgcsMcp, AgcsSchedule, AgcsWebhook, AgcsCli, AgcsLink, AgcsImport, AgcsGuest) | **Ext** |
| createdAt, updatedAt | `createdDate`, `lastUpdatedStamp` | **Use** |
| archivedAt | `statusId` AgcArchived plus new `archivedDate` | **Ext** |
| deletedAt | new status AgcDeleted plus `deletedDate`; a purge job removes rows after the retention | **Ext** |
| scratch thread folder (`~/.t3/scratch/...`) | `AgentWorkRoot` kind AgwkScratch | **New** |
| importedNativeThread {driver, nativeId} | `AgentHarnessThread.nativeRef`, `importedDate` | **New** |
| linkOrigin (thread mirrored from a peer) | `linkId` → `AgentEnvironmentLink`, `remoteConversationId` | **Ext** |
| delegatedFrom {threadId, runId, nodeId} | `parentConversationId` plus `delegatedFromRunId`, `delegatedFromToolCallId` | **Ext** |
| remoteChild (delegated to a peer) | `AgentDelegatedTask` (§3) with `linkId` | **New** |
| guest share state | `AgentGuestShare` (§8) | **New** |
| auto-settle "Never" (`thread.auto-settle.set`) | `autoSettleDisabled` | **Ext** |
| responseStreamingMode override | `streamingModeEnumId` | **Ext** |
| tags (`thread.tag.set`, <= 64 chars, exact match) | `AgentConversationTag` (conversationId, tag) | **New** |
| summary search index (FTS5) | `searchText` plus the entity search index (OpenSearch when present, else SQL LIKE) | **Use** |

### 2.1 Per-user thread state

T3 keeps one owner per environment. MR Moqui has many users per instance. Sidebar state is per user.

| T3 | `AgentConversationUser` (conversationId*, userId*) | Notes |
|---|---|---|
| pinned, pin order key | `pinnedOrderKey` (fractional key, text) | null = not pinned |
| active order key (`thread.active.reorder`) | `activeOrderKey` | |
| settledAt | `settledDate` | |
| unsettle restoreUnsettledAt | `unsettleRestoreDate` | |
| snoozedUntil | `snoozeUntilDate` | |
| visitedAt, read watermark | `lastVisitedDate`, `readThroughSeq` | `thread.visit` |
| marked unread | `markedUnread` | |
| last came back to you (Working shelf order) | `lastReturnDate` | |
| viewers (`ThreadViewers.ts`) | **Calc** from `/agentws` subscriptions | |
| notification preference per thread | `notifyEnumId` (AgcnDefault, AgcnAll, AgcnNone) | |

Shared-thread rule: settle, pin and snooze are per user. Archive and delete are per thread and need the owner or
`AgentCodeAdmin`.

## 3. Runs and turn items

| T3 | MR Moqui | How |
|---|---|---|
| Run.id, threadId, ordinal | `AgentRun.runId`, `conversationId`, new `runOrdinal` | **Ext** |
| Run.status queued/starting/running/waiting/completed/interrupted/failed/cancelled/rolled_back | `AgentRun.statusId`: add AgrQueued, AgrPrepared, AgrInterrupted, AgrRolledBack, AgrLimited. AgrSubmitted = starting. AgrWorking = running. AgrInputRequired = waiting. | **Ext** |
| provider, providerThreadId | `providerInstanceId`, `harnessThreadId` | **Ext** |
| userMessageId | first `AgentMessage` of the run (role user) | **Use** |
| rootNodeId, activeAttemptId | `attempt` plus `AgentRunAttempt` | **New** (see below) |
| requestedAt, startedAt, completedAt | `submittedDate`, `startedDate`, `endedDate` | **Use** |
| checkpointId | `checkpointId` → `AgentCheckpoint` | **Ext** |
| contextHandoffId | `handoffId` → `AgentContextTransfer` | **Ext** |
| sourcePlanRef | `sourcePlanArtifactId` | **Ext** |
| dispatchMode | `dispatchModeEnumId` (AgdmStart, AgdmQueue, AgdmSteer, AgdmRestart, AgdmDefer) | **Ext** |
| queue order, held | `queueOrderKey`, `queueHeld` | **Ext** |
| outputSchema, submitted output (`t3_submit_output`) | `outputSchemaJson`, `resultJson` | **Ext** / **Use** |
| usage-limit continuation | `continuationOfRunId`, `continuationKindEnumId` (AgckRestart, AgckUsageLimit, AgckManual, AgckSteerFollowUp), `limitResetDate` | **Ext** |
| freshContext, idleCompaction | `freshContext`, `idleCompaction` | **Ext** |
| scheduledTaskId, senderThreadId, guestMessageId | `triggerId` (**Use**), `senderConversationId`, `guestMessageId` | **Ext** |
| notification (message flag) | `notifyOnDone` | **Ext** |
| RunAttempt (attemptOrdinal, reason initial/steering_restart/retry/provider_recovery, status ... superseded, providerTurnId) | `AgentRunAttempt` (runId*, attemptSeq*, reasonEnumId, statusId, harnessTurnRef, startedDate, endedDate, errorText) | **New**. T3 shows attempts in diagnostics; the run lease `attempt` counter stays. |
| ExecutionNode | **Calc**. Parent links live on `AgentToolCall.parentToolCallId`, `AgentRun.parentRunId` and `AgentConversation.parentConversationId`. `countsForRun` = node is the run's root. | **Calc** |
| CheckpointScope (root_run/subagent/tool/provider_thread/manual) | `AgentCheckpoint.scopeEnumId`, `parentCheckpointId` | **New** |
| ProviderSession | `AgentHarnessSession` | **New** |
| ProviderThread (status not_loaded/idle/active/archived/closed/error, firstRunOrdinal, lastRunOrdinal, forkedFrom) | `AgentHarnessThread` | **New** |
| ProviderTurn | `AgentRunStep` (one step per harness turn) plus `AgentRunAttempt.harnessTurnRef` | **Use** |
| RuntimeRequest | `AgentApproval` or `AgentQuestion` by kind; `AgentRun.pauseKindEnumId`, `pauseRef` | **Use** |
| ContextTransfer (fork/provider_handoff/merge_back/subagent_spawn/subagent_result, status pending...superseded) | `AgentContextTransfer` | **New** |
| ContextHandoff payload | `AgentContextTransfer.payloadLocation` (**Res**) | **New** |
| Delegated task (completionWake always/settled_only, delivery acknowledge/dispose) | `AgentDelegatedTask` | **New** |

### 3.1 Turn items

| T3 turn item type | MR Moqui | How |
|---|---|---|
| user_message | `AgentMessage` role user; parts AgpkText, AgpkFile | **Use** |
| assistant_message | `AgentMessage` role assistant; part AgpkText; new `phaseEnumId` (AgmpCommentary, AgmpPartial, AgmpFinal) on the part | **Ext** |
| reasoning | part AgpkReasoning | **Use** |
| proposed_plan | `AgentArtifact` artifactKind `plan`, versioned; new `statusId` (AgasDraft, AgasActive, AgasCompleted, AgasSuperseded) | **Ext** |
| todo_list | `AgentArtifact` artifactKind `todo`; items in `AgentArtifactPart` | **Ext** |
| user_input_request | `AgentQuestion` (`choicesJson` holds options and multiSelect; per-question attachments as parts) | **Use** |
| file_change | `AgentToolCall` toolKind file_change plus `AgentFileChange` rows | **Ext** / **New** |
| command_execution | `AgentToolCall` toolKind command; new part kind AgpkCommand; output **Res** in `spillLocation` | **Ext** |
| file_search, web_search | `AgentToolCall` toolKind search / web | **Ext** |
| approval_request | `AgentApproval`; `targetKind` values command, file-read, file-change, mcp-elicitation, permission | **Ext** |
| checkpoint | `AgentCheckpoint` | **New** |
| run_interrupt_request / run_interrupt_result | `AgentRunEvent` run.interrupt / run.interrupted | **Use** |
| system_notice, error, notification | `AgentRunEvent` notice / error; `AgentRun.errorCode` | **Use** |
| compaction | `AgentConversation.compactedThroughSeq`, `compactionSummary`; event compaction | **Use** |
| handoff, fork | `AgentContextTransfer` plus event | **New** |
| subagent | `AgentToolCall` toolKind subagent plus child `AgentRun` (parentRunId) and child conversation (AgcrSubagent) | **Use** |
| dynamic_tool | `AgentToolCall` toolKind dynamic | **Ext** |
| thread_created (agent made a thread) | `AgentRunEvent` thread.created with the new conversationId | **Use** |
| secret_request | `AgentQuestion` questionKind secret plus `AgentSecretRequest` (§8) | **Ext** / **New** |
| link_request (peer link asked by an agent) | `AgentQuestion` questionKind link | **Ext** |
| prompt suggestion | `AgentSuggestedPrompt` | **Use** |

### 3.2 Field additions on run content

| Entity | New fields |
|---|---|
| `AgentToolCall` | parentToolCallId, harnessItemRef, toolKindEnumId (AgtkCommand, AgtkFileChange, AgtkFileRead, AgtkSearch, AgtkWeb, AgtkMcp, AgtkDynamic, AgtkSubagent, AgtkPreview, AgtkDevice, AgtkOrchestration), cwd, exitCode, durationMs, outputBytes, nodeStatusEnumId |
| `AgentMessagePart` | phaseEnumId, harnessItemRef |
| `AgentApproval` | scopeEnumId (AgasOnce, AgasSession, AgasAlways), decisionOptionsJson, harnessRequestRef, notResumable |
| `AgentQuestion` | questionKind (question, secret, link, dialog), harnessRequestRef, dismissedDate, asyncAnswer (Codex answers by message) |
| `AgentArtifact` | statusId |
| `AgentMessage` | senderConversationId, guestMessageId, linkedSenderJson, outputSchemaJson |

## 4. Projects, folders and worktrees

| T3 | MR Moqui | How |
|---|---|---|
| Project (folder on one environment) | `AgentProject` | **New** |
| repository identity (group by repo across environments) | `AgentProject.repositoryKey` (normalized remote URL), `repositoryId` → `GitRepository` when hosted here | **New** |
| tracked folders (max 128) | `AgentProjectFolder` | **New** |
| scripts (max 50; name, command <= 4096, icon, runOnWorktreeCreate, runOnSettle, stopOnSettle, async, previewUrl, autoOpenPreview) | `AgentProjectScript` | **New** |
| `t3.json` | read by `sync#AgentProjectFile`; `moqui-code.json` is the native name; `t3.json` is read too | service |
| project-scoped server settings (27 keys, see [10-Web-UI §9](10-Web-UI.md#9-settings)) | `AgentProjectSetting` (projectId, settingName, settingValue) | **New** |
| pin and archive of projects | `AgentProject.pinnedOrderKey`, `archivedDate` | **New** |
| icon (auto, emoji, monogram, image; `iconPath`) | `iconKindEnumId`, `iconValue`, `iconLocation` | **New** |
| ghq clone layout, base directory | environment settings | **Use** |
| issue tracker bindings (Linear teams per project) | `AgentProjectTracker` | **New** |
| project members (T3 has none; MR Moqui is multi-user) | `AgentProjectMember` | **New** |
| worktree of a thread, local checkout, scratch folder | `AgentWorkRoot` | **New** |
| `DevAideRoot` worktree fields (dev only) | stays; a Dev AIDE root can point at an `AgentWorkRoot` with `DevAideRoot.workRootId` | **Ext** (dev component) |
| worktree setup progress (`subscribeWorktreeSetup`) | `AgentCodeLaunch` | **New** |
| storage cleanup rules | `AgentProjectSetting` worktreeCleanup plus environment setting | **New** |

`AgentWorkRoot` replaces the dev-only `DevAideRoot` worktree fields for production use. Dev AIDE keeps its own roots
and gains a link, so one worktree shows in both apps.

## 5. Providers and models

| T3 | MR Moqui | How |
|---|---|---|
| driver kind (Codex, Claude, Cursor, Grok, fx, OpenCode, Antigravity, Kiro, Pi, Muse, ACP Registry, local ACP) | `AgentHarnessDriver` enum plus AghdNative for `moqui-llm` | **New** enum |
| provider instance (driver, displayName, accentColor, environment, settings) | `AgentProviderInstance` | **New** |
| instance environment variables with secret references | `AgentProviderInstanceEnv` (name, value or `secretRef`) | **New** |
| instance account and home (CODEX_HOME, CLAUDE_CONFIG_DIR, shadow home) | `AgentProviderAccount` per (instance, user) | **New** |
| provider status cache (version, auth, models, health) | `AgentProviderInstance.statusJson`, `lastCheckDate`, `lastError` | **New** |
| models list (refreshModels), custom models | `LlmModel` with new `providerInstanceId`; `endpointId` null for harness models | **Ext** |
| model aliases (`5.4` → `gpt-5.4`) | `LlmModelAlias` | **Use** |
| model manifest (`broken` version ranges, `versionPattern`) | `AgentHarnessManifest` seed data | **New** |
| capabilities record | `AgentProviderInstance.capabilitiesJson` plus `AgentHarnessSession.capabilitiesJson` | **New** |
| text generator (openaiCompatible or anthropic, key as secret) | `LlmEndpoint` plus `LlmFeatureBinding` features `code-title`, `code-commit`, `code-pr`, `code-branch`, `code-voice-cleanup` | **Use** |
| usage-limit source (CLIProxy) | `AgentUsageLimitSource` | **New** |
| usage-limit snapshot (reset time, percent) | `AgentProviderAccount.limitsJson`, `limitsDate` | **New** |
| ACP registry entries | `AgentAcpRegistryEntry` cache | **New** |
| install and update state | `AgentProviderInstall` | **New** |
| session import (claudeAgent, codex, pi) | `AgentHarnessThread.importedDate`; `AgentCodeImport` job rows | **New** |

## 6. Git, pull requests and issues

| T3 | MR Moqui | How |
|---|---|---|
| source-control provider account per host | `AgentCodeHostAccount` (hostKindEnumId, host, username, credentialRef, enabled, authStatus) | **New** |
| thread PR link (`ThreadPullRequestLink`, sources manual/created/agent/stack/stack-dismissed, snapshot) | `AgentConversationLink` kind AgclPr | **New** |
| PR on this server | `GitMergeRequest` | **Use** |
| PR watch (until checks/merged, wakes, awaitingBaseline, checksPending) | `AgentPrWatch` | **New** |
| thread issue link (manual, agent) | `AgentConversationLink` kind AgclIssue | **New** |
| issue on this server | mantle `WorkEffort` or the Git hosting issues | **Use** |
| files viewed (store `environment`) | `AgentReviewFile` | **New** |
| review guide (headSha, stops) | `AgentReviewGuide` | **New** |
| saved PR and issue Views (client-local in T3) | `UserPreference` `code.views.<page>` | **Use** |
| CI checks on this server | `GitCommitStatus`, `GitPipelineRun` | **Use** |
| PR sharing (Read PRs, Read and act) per environment | `AgentEnvironmentLink.prSharingEnumId` | **New** |
| Linear account | `AgentCodeHostAccount` hostKind linear | **New** |

## 7. Automation

| T3 `ScheduledTask` field | `AgentTrigger` | How |
|---|---|---|
| id | triggerId | **Use** |
| title, prompt | description, new `prompt` | **Ext** |
| enabled | `paused` (inverse) | **Use** |
| schedule interval {everyMs >= 60,000} | triggerTypeEnumId AgttInterval, new `intervalSeconds` (>= 60) | **Ext** |
| schedule fixed_time {HH:MM, weekdays} | AgttCron with a generated `cronExpression`; new `timeOfDay`, `weekdays` for the editor | **Ext** |
| schedule once {runAt} | AgttOnce, new `runAtDate` | **Ext** |
| schedule webhook | AgttWebhook plus `AgentWebhook` | **Ext** / **New** |
| projectId, threadId, threadMode (new_thread, continue, fresh_session) | new `projectId`, `conversationId`, `threadModeEnumId` | **Ext** |
| workspaceStrategy | new `workspaceStrategyJson` | **Ext** |
| modelSelection, runtimeMode, interactionMode | new `providerInstanceId`, `modelId`, `harnessModel`, `runtimeModeEnumId`, `interactionModeEnumId` | **Ext** |
| createdBy, creationSource | new `createdByUserId`, `creationSourceEnumId` | **Ext** |
| nextRunAt, lastRunAt | new `nextRunDate`; `lastRunDate` | **Ext** / **Use** |
| lastRunStatus (never, running, succeeded, failed), lastRunError, runCount | new `lastRunStatusEnumId`, `lastFailureText` (**Use**), new `runCount` | **Ext** |
| blockedByPolicy | **Calc** from policy | **Calc** |
| webhook delivery | `AgentTriggerEvent` plus `AgentWebhookDelivery` | **New** |
| replayKey and coalesce windows | `AgentWebhookDedupe` | **New** |

## 8. Terminal, preview, devices, secrets, guests, voice, usage

| T3 | MR Moqui | How |
|---|---|---|
| terminal session key {threadId, terminalId} | `TerminalSession` of kind `thread` plus new `conversationId`, `clientTerminalId`, `label`, `stopOnSettle`, `workRootId`, `providerInstanceId` ([Terminal-Fancy2](../Terminal-Fancy2.md) TF7); the worktree path and `env` are open parameters, not fields | **Ext** |
| terminal retained output (5,000 lines, 8 MiB) | **Res** history file per session: a bounded recording (Terminal-Fancy2 TF5), its location in `historyLocation` | **Res** |
| preview tab (owner agent/human/unclaimed, url, profile) | `AgentPreviewTab` | **New** |
| browser profile (persistent, incognito) | `AgentBrowserProfile` | **New** |
| preview recording | `AgentPreviewRecording` (**Res** video) | **New** |
| discovered local servers | **Calc** (port scan of session processes) | **Calc** |
| port forward (desktop) | client-side, in `moqui-client` account store | client |
| device host (local, SSH, Expo, Simlock) | `AgentDeviceHost` | **New** |
| device session (claim by thread) | `AgentDeviceSession` | **New** |
| secret reference (`op://...`) | `AgentSecretSource` plus the sealed-secret store | **New** |
| secret request card, one-use SecretRef, file delivery 24 h | `AgentSecretRequest` | **New** |
| guest share (one browser, 24 h) | `AgentGuestShare` | **New** |
| guest message (pending, sent, dismissed; 4 files, 10 MB) | `AgentGuestMessage` | **New** |
| HTML render | `AgentArtifact` artifactKind `html`, dialect html, **Res** page | **Ext** |
| voice model install, dictionary | `AgentVoiceModel`, `UserPreference` `code.voice.*` | **New** / **Use** |
| usage summary (92 days) | `LlmUsageDaily` plus new `AgentHarnessUsageDaily` from native histories | **Use** / **New** |
| model price overrides | `LlmModel` prices plus `AgentModelPriceOverride` for harness models | **Use** / **New** |
| notifications (finish, fail, approval, input) | `NotificationTopic` AgentCode, AgentCodeApproval, AgentCodeInput | **Use** (seed data) |
| Live Activity / Live Update tokens | `WebPushSubscription` rows with new `pushProviderEnumId` PushApns and new `purposeEnumId` (WpsAlert, WpsLiveActivity, WpsPushToStart) | **Ext** |
| widget state | **Calc** from `/qapps/code/widgetState` | **Calc** |

## 9. Environment, links, devices and sessions

| T3 | MR Moqui | How |
|---|---|---|
| environment id (`userdata/environment-id`) | the `AgentCodeSetting` row `environment.id`, generated once and cached on each node in `runtime/db/agent_code_environment_id`; one per cluster | **New** |
| environment label | the `AgentCodeSetting` row `environment.label` | **New** |
| environment descriptor (`/.well-known/t3/environment`) | `/.well-known/moqui/agent-code` | route ([09](09-Wire-API.md)) |
| auth session (cookie, bearer, DPoP; 30 days) | Moqui `HttpSession` for browsers; `UserLoginKey` for devices and links | **Use** |
| session scopes | `UserLoginKey.scope`: one scope per key, empty for the full account, as master keeps it; a new Agent Code scope in `KEY_SCOPES`, declared by its REST resources, with T3's finer scopes checked inside it ([14 §2](14-Security.md#2-scopes)) | **Ext** (values) |
| DPoP key binding | new `UserLoginKey.dpopThumbprint` | **Ext** |
| pairing code (12 chars, 5 min) and pairing link `/pair#token=` | `ClientPairingCode` (package `moqui.security`) | **New** |
| connect device approval code (K7QM-2XFD) | `ClientPairingCode` kind device-approval | **New** |
| desktop bootstrap (24 h) | in-process node: none needed (the host holds the node's session) | **Calc** |
| MCP OAuth client, grant, access level | `AgentMcpOAuthClient`, `AgentMcpOAuthGrant` | **New** |
| peer link (environmentId, label, urls, access, linkedAt, expiresAt, lastReachedAt, lastError, status) | `AgentEnvironmentLink` | **New** |
| link fencing (linkOrigin) | `AgentConversation.linkId` | **Ext** |
| linked thread cache (25 newest settled threads) | client store; server cache `AgentLinkedThreadCache` | client / **New** |
| client saved connections, learned routes | `moqui-client` account store (`client/accounts.json` gains `routes[]`) | client |
| load balancing mode (Normal, Prefer, Less often, Manual only) | `AgentEnvironmentLink.balanceEnumId`; client route weight | **New** |
| SSH host (launcher target) | client account `ssh` route; server `AgentSshHost` for device hosts and server-side launches | client / **New** |
| managed policy (`limits.maxRuntimeMode`, `webhookTasks`, ...) | `AgentCodePolicy` (one row per policy key) | **New** |
| server settings (`settings.json`) | `AgentCodeSetting` (settingName, settingValue) | **New** |
| client settings | `UserPreference` `code.<name>` plus device-local keys in the client store | **Use** |
| keybindings.json (256 rules) | `UserPreference` `code.keybindings` (JSON) | **Use** |
| themes | existing theme rows plus `UserPreference` | **Use** |
| storage cleanup report | `AgentCodeCleanupRun` | **New** |
| traces (NDJSON, OTLP) | existing tracing (`traceId` on runs and jobs) | **Use** |

### 9.1 Command receipts and change feed

| Entity | PK | Fields | Purpose |
|---|---|---|---|
| `AgentCodeCommand` | commandId | commandType, userId, conversationId, receivedDate, resultJson, changeSeq, errorText | Receipt per `commandId` (T3 `CommandReceiptStore`). Kept 7 days. |
| `AgentCodeChange` | changeSeq (sequence, per environment) | changeType, conversationId, projectId, userId (null = all), payloadJson, changeDate | Shell stream cursor (T3 `subscribeShell` sequence). Kept 1 day. |
| `AgentCodeEffect` | effectId | conversationId, effectType, payloadJson, statusId, attemptCount, nextAttemptDate, leaseOwner, leaseExpireDate, lastErrorText, createdDate, doneDate | Effects after commit (T3 effect outbox). Done rows kept 7 days. Failed rows kept. |

## 10. New entities

| Entity | PK | Fields |
|---|---|---|
| `AgentProject` | projectId | title, slug, rootLocation, hostInstanceId, repositoryKey, repositoryId, remoteUrl, defaultBranch, iconKindEnumId, iconValue, iconLocation, ownerUserId, pinnedOrderKey, archivedDate, scratchFolder, createdDate, lastUsedDate, fileHash |
| `AgentProjectFolder` | projectId, folderSeq | location, label, source (manual, workspace-file, t3-json), vcsKindEnumId (git, jj, none) |
| `AgentProjectScript` | projectId, scriptSeq | name, command, icon, runOnWorktreeCreate, runOnSettle, stopOnSettle, async, previewUrl, autoOpenPreview, source (setting, t3-json) |
| `AgentProjectSetting` | projectId, settingName | settingValue |
| `AgentProjectMember` | projectId, userId | roleEnumId (AgpmOwner, AgpmMember, AgpmViewer), fromDate, thruDate |
| `AgentProjectTracker` | projectId, trackerSeq | hostAccountId, teamKey, teamName |
| `AgentWorkRoot` | workRootId | projectId, conversationId, kindEnumId (AgwkLocal, AgwkWorktree, AgwkScratch), location, hostInstanceId, branch, baseRef, startFromOrigin, repositoriesJson, sparse, submodulesEnumId, createdDate, removedDate, keepReason, lastCheckedDate |
| `AgentCodeLaunch` | launchId | conversationId, projectId, workRootId, statusId (AgclQueued, AgclWorktree, AgclSetup, AgclReady, AgclFailed, AgclCancelled), stepText, setupOutputLocation, lastError, createdDate, endedDate |
| `AgentProviderInstance` | providerInstanceId | driverEnumId, displayName, accentColor, enabled, hostInstanceId, binaryPath, homePath, shadowHomePath, launchArgs, serverUrl, setupModeEnumId (managed, existing), authMethodEnumId, settingsJson, capabilitiesJson, statusJson, versionText, lastCheckDate, lastError, sequenceNum, isDefault |
| `AgentProviderInstanceEnv` | providerInstanceId, envName | envValue, secretSourceId |
| `AgentProviderAccount` | providerInstanceId, userId | homePath, authStatusEnumId, accountLabel, signedInDate, limitsJson, limitsDate, lastError |
| `AgentProviderInstall` | providerInstanceId | installKindEnumId (homebrew, npm, pnpm, bun, yarn, vite-plus, volta, mise, nix, chocolatey, native, managed), ownerProven, currentVersion, latestVersion, updateState, lastRunDate, lastOutputLocation |
| `AgentHarnessManifest` | driverEnumId, rangeSeq | versionPattern, brokenRange, note |
| `AgentAcpRegistryEntry` | registryId | name, description, version, platformsJson, distributionJson, iconUrl, fetchedDate |
| `AgentUsageLimitSource` | sourceId | kind (cliproxy), label, url, managementKeyRef, enabled |
| `AgentHarnessSession` | harnessSessionId | providerInstanceId, userId, hostInstanceId, cwd, pid, statusId (AghsStarting, AghsReady, AghsRunning, AghsWaiting, AghsStopped, AghsError), modelText, capabilitiesJson, protocolVersion, startedDate, lastSeenDate, endedDate, lastError |
| `AgentHarnessThread` | harnessThreadId | conversationId, providerInstanceId, harnessSessionId, ownerToolCallId, nativeRef, nativeHeadRef, statusId (AghtNotLoaded, AghtIdle, AghtActive, AghtArchived, AghtClosed, AghtError), firstRunOrdinal, lastRunOrdinal, forkedFromId, importedDate, resumeJson |
| `AgentRunAttempt` | runId, attemptSeq | reasonEnumId, statusId, harnessThreadId, harnessTurnRef, startedDate, endedDate, errorText |
| `AgentContextTransfer` | transferId | transferTypeEnumId, sourceConversationId, targetConversationId, sourcePointJson, basePointJson, statusId, resolutionEnumId (native, portable), payloadLocation, tokenEstimate, createdByKind, createdDate, consumedDate |
| `AgentDelegatedTask` | taskId | parentConversationId, parentRunId, parentToolCallId, childConversationId, linkId, remoteConversationId, title, task, wakeEnumId (always, settled_only), statusId, deliveryStatusId (pending, delivered, acknowledged, disposed), resultSummary, createdDate, endedDate |
| `AgentConversationUser` | conversationId, userId | see §2.1 |
| `AgentConversationTag` | conversationId, tag | taggedDate, userId |
| `AgentConversationLink` | conversationId, linkSeq | linkKindEnumId (AgclPr, AgclIssue), sourceEnumId (manual, created, agent, stack, stack-dismissed), hostKindEnumId, host, owner, repository, number, url, repositoryWebUrl, mergeRequestId, workEffortId, displayKey, title, snapshotJson, linkedDate, syncedDate, dismissedDate |
| `AgentPrWatch` | conversationId, linkSeq | untilEnumId (checks, merged), wakes, commentOnlyWakes, failedReads, awaitingBaseline, checksAwaitedSince, checksPending, fingerprint, lastPollDate, nextPollDate, endedDate, endReason |
| `AgentCheckpoint` | checkpointId | conversationId, runId, toolCallId, scopeEnumId, parentCheckpointId, workRootId, gitRef, treeHash, baselineRef, statusId (AgcpReady, AgcpMissing, AgcpError, AgcpRolledBack), filesTotal, additions, deletions, capturedDate |
| `AgentFileChange` | checkpointId, path | repositoryPath, changeKind (add, modify, delete, rename, rename-changed), oldPath, additions, deletions, binary, diffLocation |
| `AgentReviewFile` | reviewKey, userId, path | headSha, viewedDate, changedSince |
| `AgentReviewGuide` | reviewKey, headSha | guideJson, modelSelectionJson, statusId (running, ready, failed), generatedDate, failedDate, errorText |
| `AgentWebhook` | triggerId | hookId, tokenHash, signatureHeader, signatureEncoding, signaturePrefix, secretRef, maxDeliveryAgeMinutes, replayKeyTemplate, coalesceKeyTemplate, coalesceWindowSeconds, filtersJson, holdOffline |
| `AgentWebhookDelivery` | deliveryId | triggerId, receivedDate, method, contentType, bodyBytes, outcomeEnumId, signatureVerified, pendingWindow, missingFields, bodyLocation, promptLocation, runId, errorText |
| `AgentWebhookDedupe` | triggerId, keyHash | kindEnumId (replay, coalesce), firstDate, windowEndDate, lastDeliveryId |
| `AgentPreviewTab` | tabId | conversationId, ownerEnumId (agent, human, unclaimed), controlUserId, url, title, profileId, runtimeEnumId (desktop, server), width, height, colorScheme, statusText, createdDate, closedDate |
| `AgentBrowserProfile` | profileId | userId, kindEnumId (persistent, incognito), location, label, lastUsedDate |
| `AgentPreviewRecording` | recordingId | tabId, runId, statusId (running, succeeded, failed, interrupted), location, startedDate, endedDate |
| `AgentDeviceHost` | deviceHostId | kindEnumId (local, ssh, expo, simlock), label, sshTarget, identitySecretRef, port, settingsJson, statusText, lastTestDate, lastError, enabled |
| `AgentDeviceSession` | deviceSessionId | deviceHostId, platform (ios, android), deviceRef, deviceName, conversationId, claimRunId, ownerEnumId, statusId, openedDate, closedDate |
| `AgentSecretSource` | secretSourceId | kindEnumId (value, sealed, onepassword, vault), reference, label, lastCheckDate, lastError |
| `AgentSecretRequest` | requestId | conversationId, runId, questionId, clientRequestId, fieldsJson, deliveryEnumId (ref, file), statusId (pending, saved, declined, referenced, cancelled), secretRef, fileLocation, fileExpireDate, pageUrl, answeredDate |
| `AgentGuestShare` | shareId | conversationId, ownerUserId, tokenHash, redeemedDate, browserKeyHash, expireDate, endedDate, endReason |
| `AgentGuestMessage` | guestMessageId | shareId, messageText, attachmentsJson, statusId (pending, sent, dismissed), receivedDate, decidedDate, runId |
| `AgentVoiceModel` | modelKey | hostInstanceId, stateEnumId (downloadable, downloading, verifying, installed, failed), location, sizeBytes, tierEnumId (cpu, accelerated), selected |
| `AgentHarnessUsageDaily` | usageDate, hostInstanceId, driverEnumId, model, accountKey | inputTokens, cachedInputTokens, cacheWriteTokens, outputTokens, reasoningTokens, cost, costSourceEnumId (providerReported, modelPriced, unpriced), premiumCost, otherCost, sourceStatus |
| `AgentModelPriceOverride` | driverEnumId, model | inputPrice, outputPrice, cacheReadPrice, cacheWritePrice, mapToModel, lastSaveError |
| `AgentCodeHostAccount` | hostAccountId | userId, hostKindEnumId (github, gitlab, forgejo, bitbucket, azure, gitcafe, tanstack, cursor-origin, linear, moqui), host, username, credentialRef, cliPath, authStatusEnumId, enabled, lastCheckDate |
| `AgentEnvironmentLink` | linkId | remoteEnvironmentId, label, urlsJson, directionEnumId (outbound, inbound), accessEnumId (read-only, approval-required, auto-accept-edits, auto, full-access), credentialRef, linkedByUserId, linkedDate, expireDate, lastReachedDate, lastError, statusEnumId (reachable, unreachable, expired, revoked), balanceEnumId, prSharingEnumId, viaEdgeNodeId |
| `AgentLinkedThreadCache` | linkId, remoteConversationId | title, statusText, lastActivityDate, rowJson, cachedDate |
| `AgentSshHost` | sshHostId | label, target, port, identitySecretRef, hostKeyPin, purposeEnumId (device-host, launch), lastTestDate, lastError |
| `ClientPairingCode` (`moqui.security`) | pairingCodeId | codeHash, kindEnumId (CpcDevice, CpcLink, CpcApproval), userId, scopeText, accessEnumId, label, createdDate, expireDate, usedDate, usedByLoginKey, attempts |
| `AgentMcpOAuthClient` | clientId | clientName, redirectUrisJson, createdDate, registeredByAddress |
| `AgentMcpOAuthGrant` | grantId | clientId, userId, accessEnumId, scopeText, codeHash, codeExpireDate, loginKey, approvedDate, revokedDate |
| `AgentCodeSetting` | settingName | settingValue, lastUpdatedByUserId |
| `AgentCodePolicy` | policyKey | policyValue, note |
| `AgentCodeCleanupRun` | cleanupRunId | startedDate, endedDate, removedJson, keptJson, failedJson, unusedJson |
| `AgentMcpSessionKey` | loginKeyHash | conversationId, runId, providerInstanceId, harnessSessionId, capabilitiesText, createdDate, idleExpireDate, expireDate, revokedDate ([08](08-MCP-Tools.md)) |
| `AgentCodeAccessLog` | logId | userId, actionEnumId (pair, device-approve, revoke, link, unlink, mode-full-access), targetText, clientIp, date ([14 §7](14-Security.md#7-audit)) |
| `AgentCodeCommand`, `AgentCodeChange`, `AgentCodeEffect` | see §9.1 | |

## 11. Field additions to existing entities

| Entity | New fields |
|---|---|
| `AgentConversation` | engineEnumId, projectId, workRootId, branch, providerInstanceId, harnessThreadId, harnessModel, reasoningLevel, modelOptionsJson, runtimeModeEnumId, interactionModeEnumId, titleSourceEnumId, parentConversationId, rootConversationId, relationEnumId, forkPointJson, delegatedFromRunId, delegatedFromToolCallId, createdByKindEnumId, creationSourceEnumId, linkId, remoteConversationId, autoSettleDisabled, streamingModeEnumId, hostInstanceId, archivedDate, deletedDate, plus the four service names of `external-conversation-support` (`startServiceName`, `cancelServiceName`, `inputServiceName`, `eventsServiceName`; ported with P0 in PR #213) |
| `AgentRun` | runOrdinal, providerInstanceId, harnessThreadId, harnessModel, reasoningLevel, runtimeModeEnumId, interactionModeEnumId, dispatchModeEnumId, queueOrderKey, queueHeld, outputSchemaJson, checkpointId, handoffId, sourcePlanArtifactId, continuationOfRunId, continuationKindEnumId, limitResetDate, freshContext, idleCompaction, senderConversationId, guestMessageId, notifyOnDone, hostInstanceId |
| `AgentToolCall`, `AgentMessagePart`, `AgentApproval`, `AgentQuestion`, `AgentArtifact`, `AgentMessage` | see §3.2 |
| `AgentTrigger` | prompt, intervalSeconds, timeOfDay, weekdays, runAtDate, projectId, conversationId, threadModeEnumId, workspaceStrategyJson, providerInstanceId, modelId, harnessModel, runtimeModeEnumId, interactionModeEnumId, createdByUserId, creationSourceEnumId, nextRunDate, lastRunStatusEnumId, runCount |
| `LlmModel` | providerInstanceId, harnessSlug, contextWindow (if absent), serviceTiersJson |
| `TerminalSession` | conversationId, clientTerminalId, label, stopOnSettle, workRootId, providerInstanceId, historyLocation |
| `WebPushSubscription` | purposeEnumId, activityRef, agentPrefs (JSON, [12 §5.1](12-Mobile.md#51-push)); new provider value PushApns |
| `UserLoginKey` | dpopThumbprint; new scope values |
| `DevAideRoot` (dev component) | workRootId |

## 12. New enums and status flows

Seed in `runtime/components/base/agent-code/data/AgentCodeTypeData.xml`:

- Engines: AgentEngine (AgengNative, AgengHarness).
- AgentHarnessDriver: AghdNative, AghdCodex, AghdClaude, AghdCursor, AghdGrok, AghdFx, AghdOpenCode, AghdAntigravity,
  AghdKiro, AghdPi, AghdMuse, AghdAcpRegistry, AghdAcpLocal.
- AgentRuntimeMode, AgentInteractionMode, AgentDispatchMode, AgentToolKind, AgentConversationRelation,
  AgentApprovalScope, AgentContinuationKind, AgentAttemptReason, AgentTransferType, AgentCreationSource,
  AgentTitleSource, AgentWorkRootKind, AgentLinkAccess, AgentLinkDirection, AgentLinkStatus, AgentBalanceMode,
  AgentPrSharing, AgentCodeHostKind, AgentWebhookOutcome, AgentTriggerThreadMode, AgentDeviceHostKind,
  AgentSecretSourceKind, AgentPairingKind, AgentPushPurpose.
- New `AgentRun` statuses AgrQueued, AgrPrepared, AgrInterrupted, AgrRolledBack, AgrLimited, with transitions in
  `StatusFlowTransition`.
- New statuses for `AgentHarnessSession`, `AgentHarnessThread`, `AgentCheckpoint`, `AgentCodeLaunch`,
  `AgentRunAttempt`, `AgentContextTransfer`, `AgentDelegatedTask`, `AgentReviewGuide`, `AgentSecretRequest`,
  `AgentGuestMessage`, `AgentPreviewRecording`, `AgentDeviceSession`.
- AgentPartKind: AgpkCommand, AgpkFileChange. AgentSurface: AgsfCode. AgentTriggerType: AgttInterval, AgttOnce,
  AgttWebhook. AgentConversation status: AgcDeleted. PushProvider: PushApns.
