# 05. Scheduled tasks and webhooks

Part of [Agent Code](README.md). T3 sources: `t3:packages/contracts/src/scheduledTask.ts`,
`t3:apps/server/src/scheduledTasks/*`, `t3:apps/server/src/scheduling/{Scheduler,DueHint}.ts`,
`t3:docs/user/project-settings.md`, `t3:docs/user/managed-policy.md`.

A T3 scheduled task is an `AgentTrigger`. The data mapping is in [01 §7](01-Data-Mapping.md#7-automation). This file
gives the behaviour.

## 1. What exists now

- `AgentTrigger` (`AgentEntities.xml:638`) has types `AgttCron` and `AgttDocument`. `store#AgentTrigger` keeps a
  ServiceJob `agent_trigger_[triggerId]` for a cron trigger.
- It has a run-as user, batch window, run limits (60 per hour by default, 2 concurrent), a breaker after
  `failureLimit` failures, `approvalHold` and a loop guard.
- Agent Code keeps all of these. A T3 task gets them free: the breaker and the run limits apply to Agent Code tasks
  too.

## 2. New trigger types

| Enum | enumCode | T3 schedule | Fields | Rules |
|---|---|---|---|---|
| `AgttInterval` | interval | `interval {everyMs}` | `intervalSeconds` | At least 60 on save (`MIN_SCHEDULED_TASK_INTERVAL_MS`). An imported T3 row below 60 s loads and runs every 60 s, with a warning. |
| `AgttCron` (kept) | cron | `fixed_time {timeOfDay, weekdays?}` | `timeOfDay` `HH:MM`, `weekdays` (`0,1,...,6`, 0 = Sunday); the service writes `cronExpression` | Default every day. Server time zone (`ec.user` default zone of the instance; shown in the editor as `timeZone`). A plain cron trigger with no `timeOfDay` stays a raw cron editor. |
| `AgttOnce` | once | `once {runAt}` | `runAtDate` | Day validity checked. An enabled once-task with a past time is a save error. If the server is down at that time, it runs when it starts again. |
| `AgttWebhook` | webhook | `webhook {...}` | `AgentWebhook` row (§4) | Runs per request. **Run now** is refused. |
| `AgttDocument` (kept) | document | none | as today | Not shown in T3-style UI unless Advanced is on. An Agent Code extra. |

## 3. Scheduler

- Code: `crates/moqui-code/src/schedule/`. It is a port of T3 `Scheduler.ts` and `DueHint.ts`.
- One job `AgentCodeScheduler` runs each minute on the cluster's job leader. It selects due triggers by `nextRunDate`.
  A due hint wakes it early when a save makes a trigger due within the minute.
- Interval and once triggers use `nextRunDate` only. They need no ServiceJob. Cron triggers keep their ServiceJob as
  today, so non-Agent-Code cron triggers do not change.
- Next run after an interrupted run: a one-time task counts from the run's start (`lastRunDate`).
- `RUN_OUTCOME_WAIT_MS` = 24 h: the service waits at most one day for a run's outcome. After that
  `lastRunStatusEnumId` becomes failed with "outcome not seen".
- Status: `lastRunStatusEnumId` AgtrNever, AgtrRunning, AgtrSucceeded, AgtrFailed. `runCount` goes up on each start.
- Cluster: the leader claims a due trigger with an update that checks `nextRunDate` (compare and set). Two nodes never
  start the same run.
- Failure tolerance: one trigger that fails to start logs an error with its id and the loop goes on.

## 4. Thread modes and launch

| `threadModeEnumId` | Rule |
|---|---|
| AgtmNewThread (`new_thread`) | Each run calls `launch#Thread` ([02 §2](02-Orchestration.md#2-thread-creation-and-launch)) with the trigger's project, workspace strategy, model, runtime mode and interaction mode. |
| AgtmContinue (`continue`) | The first run creates the thread and writes `conversationId`. Later runs send a message to it. |
| AgtmFreshSession (`fresh_session`) | Same thread. Each later run starts a new harness session (`AgentRun.freshContext` = Y). |

- Old rows with no mode: `new_thread` when `conversationId` is null, else `continue`.
- Deleting a thread deletes the triggers that post into it. `new_thread` triggers stay.
- The run is created with `createdByKindEnumId` AgckSchedule (or AgckWebhook), and the run's `triggerId`.
- The run acts as `runAsUserId`, else `createdByUserId`. That user must still hold `AgentCodeUse` and access to the
  project. If not, the run fails with a clear reason and the breaker counts it.
- Runtime mode ceiling: a scheduled run never gets a mode above the policy `limits.maxRuntimeMode`, and never above
  `auto-accept-edits` unless the creator held `AgentCodeAdmin` when saving.

## 5. Webhooks

### 5.1 Endpoint

- Route: `/hooks/{hookId}/{token}` in `crates/moqui-host/src/http.rs`, beside `/notws` and `/termws`. Handler
  `crates/moqui-code/src/schedule/webhook.rs`. It is not under `/rest` because it needs the raw body before any
  parsing.
- Methods GET, POST, PUT, PATCH. HEAD returns 405.
- Body at most 1 MiB (`WEBHOOK_MAX_BODY_BYTES`). Above that, 413.
- Response header `x-moqui-hook-outcome` holds the outcome (T3 `x-t3-hook-outcome`). The edge node reads it.
- `tokenHash` is a SHA-256 of the token. The compare is constant time. **Rotate** (`rotate#WebhookToken`) makes a
  new token. The old one stops at once.
- The URL shown in the editor is the server's public URL (`webapp_https_url` or the edge node's public URL). With no
  public URL, the editor shows the path only, as T3.
- No session and no CSRF. The request runs as no user until the trigger is found. Then the run acts as in §4.

### 5.2 Signature

- Optional HMAC-SHA256 over the raw body. Fields `signatureHeader` (for example `x-hub-signature-256`),
  `signatureEncoding` hex or base64, `signaturePrefix` (for example `sha256=`).
- Save takes `secret` or `secretRef` (from `request_secret`, spent by the save). The secret is stored in the sealed
  secret store ([07 §1](07-Secrets-Guest-Voice-Usage.md#1-secrets)), never in the row.
- Without a signature only the URL token protects the hook. The editor says so.
- The phone app cannot turn a signature on (as T3). It can turn one off only after `check#RecentAuth`.

### 5.3 Prompt template

Port of `t3:webhookTemplate.ts` to `crates/moqui-code/src/schedule/template.rs`:

- Placeholders `{{body.path}}`, `{{headers.name}}`, `{{query.name}}`, `{{body}}`, `{{request}}`. A `[]` segment maps
  over array items (`labels[].name`).
- Each value is wrapped in `<webhook-data-XXXX>` tags with a random tag per request, and marked untrusted. The agent
  system prompt for a webhook run says the tagged text is data, not instructions.
- An invisible character goes after every `$`, currency sign, `@` and word-leading `/`, so mentions and slash commands
  do not act.
- Whole-set placeholders (`{{body}}`, `{{request}}`, `{{headers}}`) redact credentials (Authorization, Cookie,
  `*token*`, `*secret*`, `*key*`, signature headers). Named placeholders give raw values.
- A missing value renders empty. A filled prompt above the run's input limit fails with "The filled-in prompt is too
  long."

### 5.4 Filters

- At most 16. All must match. `path` starts with `body`, `headers` or `query`.
- Ops `equals`, `notEquals`, `startsWith`, `in` (list, no commas or edge spaces), `exists`. Values compare as text.
  Objects pass only `exists`.
- A non-match is logged as **Filtered** with the first failed filter.

### 5.5 Dedupe

- `replayKeyTemplate`: the same key is ignored for 7 days per trigger. Row in `AgentWebhookDedupe` kind replay.
- `coalesceKeyTemplate` plus `coalesceWindowSeconds` (10-3600, default 120): one run with the latest body after the
  window from the first delivery closes. Row kind coalesce with `windowEndDate`. The scheduler of §3 closes windows,
  so pending windows survive a restart.
- A key must contain a request placeholder. A missing placeholder or an empty render skips the rule.

### 5.6 Limits

| Constant | Value |
|---|---|
| Rate per trigger | 60 per minute (also on the edge node); 429 above |
| Queued per trigger | 20; above that the delivery is `rate_limited` |
| Delivery log kept | 50 (dedupe 20, filtered 20) per trigger |
| Dedupe and filtered log rows | 10 per minute each |
| Logged body, logged prompt | 64 KiB each, in `ContentBlob` (`bodyLocation`, `promptLocation`) |

The run limits of `AgentTrigger` (`maxRunsPerHour`, `maxConcurrent`) apply after these.

### 5.7 Outcomes

`outcomeEnumId`: accepted, dispatch_failed, duplicate, coalesced, rejected_signature, disabled, rate_limited, expired,
filtered. Summary fields as T3: `deliveryId`, `triggerId`, `receivedDate`, `method`, `contentType`, `bodyBytes`,
`outcomeEnumId`, `signatureVerified`, `pendingWindow`, `missingFields`, `errorText`. The editor's **Deliveries** menu
shows each request and the prompt it made.

### 5.8 Offline hold

- T3 holds requests in T3 Connect for up to 24 h. MR Moqui holds them on the **edge node** that fronts the server
  (`EdgeQueueItem` kind `agent-webhook`). See [13 §9](13-Servers-Linking.md#9-cluster-and-edge).
- Setting **Hold webhooks while offline** (`AgentWebhook.holdOffline`, and the server setting of the same name) turns
  it on.
- The edge node stores the raw request, its headers and its receive time. It replays in order when the link is back.
- An edge replay carries `x-moqui-edge-received` and a proof signed with the edge link key. The server trusts the
  receive time only with a valid proof. The proof covers the 24-hour hold plus 10 minutes.
- `maxDeliveryAgeMinutes` (1-1440, `MAX_WEBHOOK_DELIVERY_AGE_MINUTES`) makes an old one `expired`.
- With no edge node, the sender gets an error while the server is down (as T3 without Connect).

### 5.9 Policy

`AgentCodePolicy` `webhookTasks` false blocks webhook triggers. Existing ones stay and show `blockedByPolicy`. They do
not run. A delivery gets outcome `disabled`.

## 6. Services, RPCs and tools

| T3 | MR Moqui service (`ScheduleServices.xml`) |
|---|---|
| `scheduledTasks.subscribe` | `/agentws` stream `scheduledTasks` from the change feed |
| `scheduledTasks.upsert` (+ `requireExisting?`, `commandId?`, `unbindThread?`) | `store#ScheduledTask` (calls the existing `store#AgentTrigger`) |
| `scheduledTasks.setEnabled` | `set#ScheduledTaskEnabled` |
| `scheduledTasks.delete` | `delete#ScheduledTask` |
| `scheduledTasks.runNow` | `run#ScheduledTaskNow` (refused for webhook) |
| `scheduledTasks.rotateWebhookToken` | `rotate#WebhookToken` |
| `scheduledTasks.listWebhookDeliveries` | `find#WebhookDeliveries` |
| `scheduledTasks.getWebhookDelivery` | `get#WebhookDelivery` |
| MCP `schedule_task`, `list_scheduled_tasks`, `run_scheduled_task_now`, `delete_scheduled_task` | Tools of [08](08-MCP-Tools.md) |

- Results list includes `timeZone` (the server zone).
- An agent that creates a task gets `creationSourceEnumId` AgcsAgent. The task's runtime mode cannot be higher than
  the creating run's mode.

## 7. UI

- **Settings > Scheduled tasks** on web, desktop and phone: list, edit, pause, resume, run now, delete.
- Editor: title, prompt, project, thread mode, schedule kind, model, runtime mode, interaction mode, workspace
  strategy. Webhook part: URL, rotate, signature, template help, filters, dedupe, max age, hold offline, Deliveries.
- Phone: filters by environment and project. **Once** uses the phone's time zone and converts. Unsaved changes ask
  before discard.
- Advanced (Agent Code extra): run-as user, run limits, breaker state and reset, approval hold, the document trigger
  type. Only for `AgentCodeAdmin`.
- Screens in [10 §9](10-Web-UI.md#9-settings).

## 8. Tests

See [15](15-Tests.md). Regression: template escaping and redaction, filters, dedupe and coalesce windows, interval
minimum, once in the past, thread modes, rate limit, signature check, policy block. Acceptance: a webhook through the
edge node while the server is down, then replay.
