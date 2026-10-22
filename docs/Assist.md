# Assist: the drawer and its server

The Assist drawer is the agent of the `/qapps` shell: the user asks in a side panel, the agent works the user's own
screens through a server-side screen session as the user, the user's page follows it, and every write waits for the
user's approval. This page is the contract between the drawer (`js/WebrootAssist.js`, phase 13.5 part B) and the server
(part A), and how the server side works. The design is [plans/LLM-A2A-Assist.md](../plans/LLM-A2A-Assist.md) (4.4 runs,
4.5 events and the drawer's calls, 4.7 screens, 4.8 approvals, 4.11 the drawer, 4.18 the admin screens, 7.1 the tail).
As of Oct 22, 2026 (13.5 built: the server, the drawer and their Chromium smokes; 13.6: the OpenUI canvas, "The canvas" below;
13.9: chat members, triggers and AI tasks, "Chat members, triggers and AI tasks" below; 13.14: memory, lessons, Save as
skill and the composer extras, "The calls of 13.14" below).

The code: `crates/moqui-core/src/llm/assist/` (the calls: `mod.rs` the dispatcher, errors and parameters, `read.rs`,
`write.rs`, `state.rs` the page state, tail and follow stamp, `prompts.rs` the suggested-prompt providers, `audit.rs`
the admin services, `canvas.rs` the canvas calls and the read proxy), the webroot subscreen `runtime/base-component/webroot/screen/webroot/assist.xml` (one transition per
call, each calling `org.moqui.agent.AssistServices.call#Drawer`), the event stream in
`crates/moqui-host/src/llm/sse.rs`, and the admin screens under `runtime/base-component/tools/screen/System/Llm/`
(Conversations, Runs, Approvals). Tests: `crates/moqui-core/src/llm/assist/tests.rs` (the page state, parameters,
errors) and `crates/moqui-host/tests/it/assist.rs` (every call over HTTP on embedded Postgres with the scripted fake
provider, the full run → stream → decide → resume flow, and the audit log).

## Conventions

- **Where.** Each call is `/assist/<call>` under the webapp root (`appRootPath + "/assist/" + call`). The reads are
  `GET` with query parameters; the writes are `POST`. A call with the other method is 405.
- **Who.** The browser session's cookie. Every call acts for the current user only and needs the `AgentUse`
  permission. A user with the framework permission `EXTERNAL_USER` (an app's outside users, such as client contacts)
  never holds `AgentUse` or the other agent permissions, starts no run and gets no model call (`ec.llm` included). A conversation, run, approval, question, hand-off or message of anyone else is **not found** (404),
  never named. A call made from inside an agent's own run is refused (403): nobody, the agent included, drives a
  conversation that is not theirs.
- **The session token.** Every `POST` presents the session token, as every transition that runs actions does: the
  `X-CSRF-Token` header (what `moqui.request` sends from `moqui.sessionToken`), or a `moquiSessionToken` parameter.
  Missing or wrong is 401 `{"errorCode": 401, "errors": "Session token is missing or is not valid"}` from the render,
  before the call runs. A response may carry a new token in its `X-CSRF-Token` header (`moqui.onSessionToken`).
- **Bodies.** A `POST` body is JSON (`Content-Type: application/json`, an object whose fields are the parameters), or a
  form (`application/x-www-form-urlencoded`) where an object or list parameter (`state`, `arguments`, `values`,
  `choices`) is JSON text. `moqui.request` posts a plain object as a form (and a string body as a form too), so with it
  the client passes those parameters through `JSON.stringify`; a `fetch` of its own may post the whole body as JSON.
- **Answers.** 200 with a JSON object. An error is its status with `{"errorCode": <status>, "errors": "<text to
  show>", "code": "<code>"}`:

  | Status | `code` | When |
  | --- | --- | --- |
  | 400 | `invalid` | A parameter is missing or does not read (also a form answer that does not fit its form) |
  | 401 | `unauthorized` | No user (or the session token, from the render, with no `code`) |
  | 403 | `refused` | No `AgentUse`; a decision not from a browser session with its token; a call from inside a run |
  | 404 | `not_found` | Not the user's, or none |
  | 409 | `conflict` | The thing is not in a state that takes this: decided already, expired, the run ended or is active |
  | 429 | `tarpit` | The AT_LLM tarpit (too many runs); `Retry-After` has the seconds |
  | 500 | `failed` | Anything else (logged on the server) |
  | 501 | `not_available` | `transcribe` with no model bound to the `transcribe` feature |
  | 503 | `disabled`, `unavailable` | The agents are turned off (`llm-facade enabled="false"`; `bootstrap` still answers), or the database is down |

- **Ids the client makes.** `clientId` is the tab's id: made once per tab (random, kept in `sessionStorage`), sent
  with every turn and follow call. `clientMessageId` is the message's id: made per message and kept with the draft
  until the server answers, so a send tried again (a timeout, a reload) is the same send. Both are letters, digits,
  `-`, `_`, `.` and `:`, at most 100 characters.
- **Dates** are epoch milliseconds. **Run states** are the A2A wire values (`TASK_STATE_SUBMITTED`, `_WORKING`,
  `_INPUT_REQUIRED`, `_AUTH_REQUIRED`, `_COMPLETED`, `_FAILED`, `_CANCELED`, `_REJECTED`) with the `statusId`
  (`AgrSubmitted` … `AgrRejected`) beside; paused is INPUT_REQUIRED or AUTH_REQUIRED, ended is the last four.

## The page state

A turn, a retry and `context` carry `state`, what the user's page is (plan 4.11 "Follow on the page"). The server reads
it leniently (a part of the wrong type is left out), caps it, and drops every value whose name looks secret before it is
stored or put in the prompt; the client should leave those out itself (`bootstrap.secretFields`), with file, hidden and
password widgets.

```json
{
  "path": "/qapps/system/Security/UserAccount/UserAccountList",
  "parameters": {"username": "john"},
  "title": "User Accounts",
  "forms": [{"domId": "UserAccountList", "values": {"username": "john"}, "dirty": ["username"]}],
  "dialogs": ["CreateUserDialog"],
  "record": {"entityName": "moqui.security.UserAccount", "id": "EX_JOHN_DOE", "label": "john.doe"},
  "selections": [{"form": "UserAccountList", "ids": ["EX_JOHN_DOE", "EX_JANE"], "total": 5}],
  "filters": [{"form": "UserAccountList", "values": {"username": "john"}}],
  "formInfo": [{"name": "UserAccountList", "kind": "list", "fields": ["Username", "Email"], "rows": 25}],
  "canvas": {"title": "Logins by day", "version": 3}
}
```

- `path` is the screen path; `/qapps/…` is read as `/apps/…` (the screen sessions' tree), and one that is not a plain
  path from the root (a scheme, a host, `..`, `//`, a backslash, a control character) is no path. Parameters are
  flat strings (at most 50). `forms` are the registered `m-form`s of the page and its open dialogs, by the qvt
  `domId`, with the values the user changed (`moqui.fieldChanged`) and their names in `dirty` (at most 20 forms of
  100 values of 4000 characters); `dialogs` are the open container dialogs' ids.
- The sync-in (below) takes `path`, `parameters`, the dirty values and `dialogs`. The tail of the message (plan 7.1)
  takes `now` (the user's time zone), `page` (path, query and title), `record`, `selections`, `filters`, `formInfo`
  and `canvas`. All of it is optional.

## The calls

### `GET bootstrap`

What the drawer needs before anything else.

```json
{
  "user": {"userId": "EX_JOHN_DOE", "username": "john.doe", "locale": "en_US", "timeZone": "US/Central", "weekStart": "Sunday"},
  "agents": [{"agentId": "assist", "name": "Assist", "description": "…", "tier": null, "tierFixed": false,
              "tiers": ["fast", "balanced"], "dialects": [{"name": "openui", "label": "OpenUI Lang", "skill": "openui-lang"}],
              "startPath": "/apps"}],
  "defaultAgentId": "assist",
  "tiers": ["fast", "balanced", "thorough"],
  "features": {"enabled": true, "screenFollow": true, "preview": true, "canvas": true, "canvasExternal": false,
               "attachments": true, "transcribe": false, "browserDictation": false, "textActions": true,
               "saveSkill": true, "memory": true, "admin": true, "audit": false},
  "limits": {"maxMessageChars": 16000, "maxForms": 20, "maxFormValues": 100, "maxValueChars": 4000,
             "approvalTtlMinutes": 30, "maxSteps": 40, "maxRunSeconds": 900, "maxAttachmentBytes": 10485760,
             "maxAttachments": 5, "attachmentTypes": ["text/*", "application/json", "application/xml", "application/pdf",
             "image/png", "image/jpeg", "image/gif", "image/webp"], "maxAudioBytes": 10485760, "maxTextActionChars": 8000},
  "memory": {"preference": true},
  "secretFields": {"contains": ["password", "passwd", "token", "secret", "credential", "salt"], "endsWith": ["key"],
                   "names": []},
  "paths": {"calls": "/assist", "events": "/assist/runs/{runId}/events", "catchUp": "/assist/runs/{runId}/events.json",
            "sandbox": "/assist/sandbox"}
}
```

- `agents` are the agents the user may use in the drawer (not disabled, serving the drawer surface). An agent's `tiers`
  are those some usable model has; with `tierFixed` the agent names its own `tier` and the tier picker is hidden.
  `dialects` are the canvas dialects the drawer offers it (built on this server, with the drawer's `ui.write:<name>`
  capability, and named by the agent's `canvasDialects`, empty being `openui` only): `openui`, and `vue-sfc` for an
  agent that names it. `features.canvas` says whether any agent has one; `features.canvasExternal` is the user's
  preference `ASSIST_CANVAS_EXTERNAL` (a Vue SFC canvas may open other sites and show their images).
- `features.screenFollow` is false on a server without the screen sessions (no follow events will come);
  `features.preview` says whether approvals can be previewed (the sim overlay); `admin` and `audit` are the user's
  `AgentAdmin` and `AgentAudit`, for the admin `/` commands.
- `secretFields`: a field whose name, in lower case, contains a `contains` word, ends with an `endsWith` text, or is
  one of `names` (the conf's `content-log redact-keys`) is never sent.
- 13.14: `features.transcribe` is true when a model is bound to the `transcribe` feature (dictation records audio and
  posts `transcribe`); `browserDictation` when none is and the conf allows the browser's speech API
  (`llm-facade assist browser-dictation="true"`: it sends the audio to the browser's vendor, so it is off by default);
  `textActions` when a model can take the `text-action` feature (its binding, else the fast tier); `saveSkill` is the
  user's `AgentSkillAuthor`. The attachment, audio and text action limits are the conf's `assist` element.
  `memory.preference` is the user's permanent memory gate (`ASSIST_MEMORY`, else `assist memory-default`).

### `GET conversations`

The user's conversations, newest first. Parameters: `search` (text in the title or the user's messages), `filter`
(`all`, the default; `running`; `waiting`, for an approval, question or hand-off; `record`, with `recordEntityName` and
`recordId`), `surface` (`drawer`, `chat`, `tui`, …; all by default), `agentId`, `pageIndex` (0), `pageSize` (25, at
most 100).

```json
{"conversations": [{"conversationId": "100050", "title": "Say hello", "agentId": "assist", "surface": "drawer",
   "statusId": "AgcActive", "activeRunId": null, "activeRunState": null, "waiting": false,
   "lastMessageDate": 1761140000000, "createdDate": 1761139990000, "recordEntityName": null, "recordId": null}],
 "pageIndex": 0, "pageSize": 25, "hasMore": false}
```

A conversation's title is the first line of its first message (80 characters); 400 for an unknown `filter` or
`surface`.

### `GET conversation`

One conversation of the user's, to draw the thread and re-attach. Parameters: `conversationId` (required), `clientId`
(to learn whether this tab follows), `limit` (the newest messages, 200 by default, at most 500), `beforeSeq` (the
messages before that sequence, for an earlier page).

```json
{
  "conversation": {"conversationId": "100050", "title": "…", "agentId": "assist", "surface": "drawer",
                   "statusId": "AgcActive", "recordEntityName": null, "recordId": null, "createdDate": 0,
                   "lastMessageDate": 0, "lastMessageSeq": 4},
  "activeRun": {"runId": "100071", "agentId": "assist", "state": "TASK_STATE_INPUT_REQUIRED", "statusId": "AgrInputRequired",
                "pauseKind": "approval", "pauseRef": "9f…", "lastEventSeq": 14, "submittedDate": 0, "endedDate": null,
                "stepCount": 1, "toolCallCount": 1, "inputTokens": 900, "outputTokens": 40, "activeSeconds": 2,
                "tier": null, "errorCode": null, "errorText": null, "cost": null},
  "runs": [ /* every run of the conversation, oldest first, in the activeRun shape */ ],
  "items": [
    {"messageId": "100060", "seq": 1, "role": "user", "runId": "100051", "clientMessageId": "m-1", "createdDate": 0,
     "compacted": false, "feedback": null, "parts": [{"part": 1, "kind": "text", "text": "Say hello"}]},
    {"messageId": "100061", "seq": 2, "role": "assistant", "runId": "100051", "clientMessageId": null, "createdDate": 0,
     "compacted": false, "feedback": {"rating": "up", "note": "good", "userId": "EX_JOHN_DOE", "date": 0},
     "parts": [{"part": 1, "kind": "text", "text": "Hello."},
               {"part": 2, "kind": "tool-call", "toolCallId": "100062", "name": "run_tool", "arguments": "{…}",
                "status": "AgtcDone", "classification": "write", "approvalId": "9f…", "startedDate": 0, "endedDate": 0}]},
    {"messageId": "100063", "seq": 3, "role": "tool", "runId": "100051", "parts": [
               {"part": 1, "kind": "tool-result", "toolCallId": "100062", "name": "run_tool", "isError": false, "text": "…"}]}
  ],
  "hasEarlier": false,
  "pending": {"approvals": [ /* approval cards */ ], "questions": [ /* question cards */ ],
              "handoffs": [{"handoffId": "…", "approvalId": "…", "runId": "…", "screenPath": "component://…",
                            "transition": "updateOrder", "path": "/apps/Orders/Edit", "parameters": {}, "form": "EditOrder",
                            "values": [{"field": "statusId", "label": "Status", "value": "Closed"}], "status": "watching", "createdDate": 0, "expiresAt": 0}],
              "queued": [{"inputId": "…", "text": "Also this", "clientMessageId": "m-3", "receivedDate": 0}]},
  "canvas": {"artifactId": "100090", "conversationId": "100050", "runId": "100071", "kind": "openui", "title": "Visit hits",
             "version": 2, "program": "root = Stack([...])\n…", "statements": ["root", "visits", "chart"], "mutations": [],
             "state": {"q": "x"}, "problems": [], "closed": false, "updatedDate": 0},
  "follow": {"clientId": "tab-a", "contextVersion": 3, "following": true}
}
```

- `items` are the stored messages in order: `role` is `user`, `assistant` or `tool`; parts are `text`, `reasoning`,
  `tool-call` (the step row: arguments redacted and cut at 2000 characters, the call's status and class, its approval,
  and for a decided one `approval`, `{approvalId, runId, toolCallId, classification, targetName, summary, status,
  decisionChannel}`, so a decided card is drawn collapsed after a reload and in another tab),
  `tool-result` (redacted, cut at 2000), `file` and `data`. A user message never shows its `<context>` tail.
- `activeRun` is the run working or paused now (null when none), with `lastEventSeq`: open the event stream after it.
  `runs` give the done line of each (steps, tokens, seconds; `cost` only for an `AgentAdmin`).
- `pending` are the cards that wait for the user: approvals Pending, questions open, hand-offs watching (or refused
  once and watching again), and the messages queued for the active run that it has not folded in yet ("Queued" rows).
- **The approval card** (also the payload of `approval.requested`): `{approvalId, runId, toolCallId, classification
  (read|write|destructive|external), targetKind (screen|service|tool), targetName, summary, changes: [{field, label,
  current, proposed}], record, recordRefs, status (pending|approved|executing|executed|failed|rejected|expired|conflict|
  handed-off), decisionChannel, feedback, expiresAt, canEdit (Edit in screen is offered), preview, result, tool,
  canPreview}`.
- **The question card**: `{questionId, runId, toolCallId, text, options: [{label, recommended}], multi, fields: [{name,
  label, type, required?, options?, search?}], freeText, kind (ask|budget), status, createdDate, expiresAt}`. A budget
  question ("This run reached its limit of steps. Continue?") has the options `continue` and `stop`.
- `follow` is the follow owner (`clientId`), the page version the server has (`contextVersion`), and whether the
  asking tab is the owner.
- `canvas` is the conversation's canvas (null when it has none): the merged program, its version, its statement and
  Mutation names, the state the page last posted, the problems the page reported, and whether the user closed it.
- 13.14: `context` is the context meter, `{"usedTokens", "windowTokens", "budgetTokens", "percent", "compactAt",
  "modelId", "estimated"}` from the conversation's last model call of a step (its prompt and its reply) against its
  model's window (7.8: the budget is the window less the output and reasoning reserve, less 3%; compaction starts at
  `compactAt`), null before any call; `memory` is `{"preference", "conversation", "effective"}`, the gates (below), and
  `conversation.memoryCreate` the conversation's own (null, true or false). A user message's `file` parts are
  `{"part", "kind": "file", "attachmentId", "ref": "F…", "fileName", "mediaType", "size"}`.

### `POST turn`

The user's message. Parameters: `text` (required, at most `limits.maxMessageChars`), `clientMessageId` (required),
`clientId` (required), `conversationId` (none starts a new conversation), `agentId` (for a new conversation; `assist`
by default; an existing conversation keeps its agent), `tier` (`fast`, `balanced`, `thorough`; ignored when the agent
fixes its tier), `contextVersion` (the tab's page version), `state` (the page).

```json
{"conversationId": "100050", "runId": "100051", "state": "TASK_STATE_SUBMITTED", "statusId": "AgrSubmitted",
 "queued": false, "existing": false, "lastEventSeq": 1,
 "follow": {"clientId": "tab-a", "contextVersion": 1}, "eventsPath": "/assist/runs/100051/events"}
```

- An `@name` or `@name#section` in `text` that names a skill of the user's index loads it right after the message, as a
  `skill` call and its result the model did not make (at most three a message; queued messages too); any other `@` is
  text. The message's tail carries `skills that may fit: …` when the text strongly matches skills the conversation's
  frozen catalog does not list and it has not loaded (not on a conversation's first message, whose catalog is built
  from it).
- While the conversation's run is active the message is **queued** for that run (`queued` true, `runId` the active
  run's): it becomes an `AgentInput` the run folds into its next step (`input.folded`), and the run answers it. A message
  that arrives during the run's last model call keeps the run going for one more step rather than being dropped.
- The same `clientMessageId` again answers what the first send did (`existing` true, the same run, `queued` as it was)
  and changes nothing: it is unique with the user, for messages and queued input alike.
- The sending tab becomes the **follow owner** and its `contextVersion` is kept (the higher of the stored and the sent);
  only once the turn is taken (a run made or the message queued): a refused turn (the tarpit, another user's
  conversation) and a run rejected for the quota move neither the owner nor the session's page;
  its page **syncs into** the conversation's screen session (a new conversation's first session opens on it).
- A new run counts one hit of the user's AT_LLM tarpit (`agent:<agentId>`; the seed allows 30 a minute), where the run is
  made under the conversation's lock (so a send that finds the run active, or a second send of the same moment, counts none); a queued
  message counts none. Over the limit is 429 with `Retry-After`. A user over their daily quota gets a run in state
  `TASK_STATE_REJECTED` (200; its `run.done` says `quota_exhausted`).
- 404 for another user's conversation or an agent the user may not use in the drawer.
- 13.14: `attachments` (a list of attachment ids, JSON text in a form) carries files `POST attachments` stored: each
  must be the user's and not another conversation's (404), at most `limits.maxAttachments` (400); each becomes a
  `file` part of the message (of a queued one too), the model reads a line naming it, `[attached F12: stock.csv
  (text/csv, 28 bytes); read it with data artifact=F12]`, and `data artifact=F12` reads a text file's lines wrapped as
  `<untrusted from="attachment">` (an image or a PDF is named, not read). `surface` is `drawer` (the default) or `tui`: the terminal's Assist pane
  (docs/TUI.md "Assist") makes its conversations the terminal's, whose runs follow on the terminal's own screen and
  ask inline; a retry keeps the conversation's surface. A first message's tail names the user's learned skills that
  strongly match it (`skills that may fit`), as they never enter the catalog its prefix is built with.

### `POST cancel`

`runId` (required). Stops a run of the user's (Stop in the drawer): a run waiting or not yet working ends Canceled at
once; one working stops at its next check, and the text streamed so far is kept. `{"runId", "state", "statusId"}`.

### `POST retry`

Runs the conversation's last user message again after its run failed or was stopped: `conversationId`,
`clientMessageId` (a new one), `clientId` (required), `tier`, `contextVersion`, `state` (optional; the stored tail is used
without it). The message is sent again, so it shows twice in the history. 409 while a run is active or when there is no
message. Answers as `turn`.

### `POST follow`

`conversationId`, `clientId` (required), `contextVersion`, `release`. This tab follows the conversation's agent (its
`ui.*` events are for it): "Follow here". With `release: true` the tab gives it up (when it has it). `{"conversationId",
"follow": {"clientId", "contextVersion"}, "following"}`.

### `POST context`

The user navigated (or changed the page) while a conversation is open: `conversationId`, `clientId`, `contextVersion`
(required, raised by the tab before it posts), `state`. From the follow owner (or any tab when none follows, which then
does) the server keeps the version and syncs the page into the session, so the agent goes on from where the user went;
from another tab nothing changes (the owner is checked and claimed under one lock). `{"conversationId", "applied",
"follow": {"clientId", "contextVersion"}}`; the tab's own version is at least the answer's.

### `POST submitted`

A hand-off's form was answered in the browser (`moqui.onFormAnswer`): `handoffId` (required), and what the client saw
(`ok`, `messageInfos`, `errors`, `validationErrors`, `screenUrl`; never the form's values). It is a **signal only**: the user's own save
is observed on the server (the transition observer), which resumes the run; the answer is the watch as the server has
it: `{"handoffId", "approvalId", "runId", "status": "watching|saved|failed|abandoned"}`. With `abandoned` (a reason:
`navigated`, `skipped`) the watch is abandoned and the run goes on with that; like a decision, only from the browser
session with its token (403 otherwise).

### `POST decide`

The user's decision on an approval of theirs: `approvalId` and `decision` (required): `approve` (with `arguments`, an
object of edited arguments, for a service or tool call; they are checked again and hashed anew), `reject` (with
`feedback`, which becomes the agent's next task) or `edit` (Edit in screen: the write is handed to the page; with
`clientId` and `contextVersion` the deciding tab becomes the follow owner at the higher of its version and the stored one,
so it gets the hand-off's `ui.*` events even when it never reported its page). `{"approvalId", "status", "statusId",
"handoffId", "follow"}`: `follow` (`{clientId, contextVersion}`, for an `edit` with `clientId`) is what the hand-off's
events are stamped with, which the tab takes as its own.

Only from a **browser session by its cookie with the session token** (`approval::Decider::from_request`): a header
login (Basic, `api_key`, `login_key`) and a session a header login made are 403, so a program can never approve; another
user's approval is 404; a second decision, an expired approval and one whose run no longer waits are 409. The run
resumes with the decision (`approval.decided`, then the call's events).

### `POST preview`

`approvalId`. The effects of a pending approval run in the sim overlay as the user (slower: "Preview effects"), kept on
the approval: `{"approvalId", "preview": {"ok", "rows": [{"entity", "action", "key", "fields": [{"field", "before",
"after"}]}], …}}` (an encrypted or secret-named field shows `(hidden)`). 409 when the server cannot preview
(`bootstrap.features.preview` false) or the approval is not pending; 404 for another user's.

### `POST answer`

The answer to the question a run of the user's waits on: `runId` (required), `questionId` (when given it must be the
one the run waits on, else 409), and one of `choices` (a list of option labels) or `choice`, `values` (an object of a
form's values by field name; a record field's value may be `{id, label}`), `text` (free text). A form's values are
checked first (400 with the problems). The budget question takes `choice: "continue"` or `"stop"`. A run that waits
AuthRequired on a remote's link (a delegated A2A task or a remote MCP tool, 13.12: its `question.asked` event carries
`link` and `remote`) takes `choice: "Done"` or `"Cancel"` the same way. `{"runId", "state", "statusId"}`; 409 when the
run does not wait for an answer.

### `POST feedback`

`messageId` (an assistant message of the user's), `rating` (`up`, `down`, or empty to clear), `note` (at most 2000
characters). Kept in the message's metadata and shown back as the item's `feedback`. `{"messageId", "feedback"}`.

### `GET tasks`

The tasks pane, across every surface: `{"waiting": [...], "running": [...], "failed": [...], "done": [...], "inbox": [],
"counts": {"waiting": 2, "running": 1}}`. A waiting item is `{"kind": "approval|question|budget|handoff", the card's id
(approvalId, questionId or handoffId), runId, conversationId, conversationTitle, surface, summary, createdDate,
expiresAt}`; a run item is `{"kind": "run", runId, agentId, conversationId, conversationTitle, surface, state,
pauseKind, submittedDate, endedDate, errorCode, errorText}`. `failed` covers the last 7 days, `done` the last day;
`inbox` (threads started by triggered agents) comes with phase 13.9. `counts.waiting` is the badge.

### `GET prompts`

Suggested prompts for the page: `path` (the page path), `entityName`, `recordId`, `agentId` (`assist` by default).
`{"prompts": [{"promptId", "label", "text", "send", "source"}]}` (at most 8): the admin's `AgentSuggestedPrompt` rows
for the agent or any agent whose `screenPathPattern` (a regular expression over the whole `/apps/…` path) and
`entityName` fit, then the registered providers' (`source` is the provider's name). With `send` the chip sends at once,
else it fills the composer. Components register a provider in Rust:

```rust
#[derive(Default)]
struct OrderPrompts;
impl moqui_core::llm::assist::prompts::PromptProvider for OrderPrompts {
    fn name(&self) -> &str { "orders" }
    fn prompts(&self, facade: &EntityFacade, query: &PromptQuery) -> Result<Vec<SuggestedPrompt>, Error> { … }
}
moqui_core::submit_assist_prompts!(OrderPrompts);
```

Each provider runs under the read fence; one that fails or writes is logged and left out. Since 13.14 the prompts also
come from the apps' `<shell-providers><assist-prompt label text send entity screen sequence>` (source: the app's name;
on the app's pages, or on `screen` and below it, for records of `entity` when it names one; a screen the user may not
view leaves it out) and from the procedure skills whose `screens` hold the page (source `skill`, text `@name `, sent
by the user). The answer also carries `responseActions`: the framework's own `{"key": "download", "label": "Download
as Markdown", "icon", "builtin": true}` (the answer's Markdown as a file, on the client) and the apps'
`<shell-providers><response-action label icon screen text-parameter requires-record sequence>`, `{"key":
"app:<app>:<label slug>", "label", "icon", "url", "textParameter", "requiresRecord", "source"}`: the drawer opens the
`url` in the shell's quick-create frame with the page's record (`entityName`, `recordId`) and puts the answer's text in
the frame form's `textParameter` field once the form is there (the text waits in `sessionStorage`; it is never in the
URL, where access logs would keep it), and offers a `requiresRecord` one only on a record. "Send as message" and "Log as note" are
an application's (BOSS's), so the framework names no screen of theirs.

### `GET mentions`

The `@` pickers: `kind` and `q`.

- `kind=agents` (the default): `{"kind": "agents", "items": [{"kind": "agent", "agentId", "label", "description"}]}`.
- `kind=skills`: the skills of the user's index for `agentId` (default `assist`) on the drawer, personal ones shadowing
  the site's unless those are locked: names that start with `q` first, then retrieval's ranking of `q`, at most 10
  (`{"kind": "skill", "name", "label", "description", "personal"}`).
- `kind=records`: the palette's sources, as `/shellSearch` answers them, with `groups` and `limit`: `{"kind":
  "records", "groups": [{…palette group with rows carrying recordEntity and recordId…}]}`.

### The canvas calls

`POST canvasState`, `canvasAction`, `canvasAdjust`, `canvasProblems` and `GET read` are the canvas's ("The canvas"
below). Each takes the canvas's `artifactId` (a canvas of another user's conversation is 404) and, but for `read`, its
`version` (required; an older one is 409). The data a call changes is read for update and its version checked again in
the transaction that writes it, so no call overwrites a newer `write_ui` or another call.

The values a canvas posts (`state`, `values`) are taken as they are or refused, never cut: at most 100 values of 4,000
characters, 20,000 in all (400, or for a Mutation `ok` false). A secret-named value is never stored, and a Mutation that
would post one is refused (a canvas does not post secrets; a password form is a screen's own).

- `canvasState`: `state` (the canvas's values by name, secret names left out, at most 20,000 characters) and `closed`.
  `{"artifactId", "version", "closed"}`. Closing it sends the `canvas.closed` signal.
- `canvasAction`: a Button. `kind=continue` with `message`, `values`, `form` is a turn (answered as `turn`, with the tab's
  `clientMessageId`, `clientId`, `contextVersion`): the message and the values, never the program again. `kind=arm` with
  `count` (at most 5) and `activation` is one click: `{"nonces": [...]}`, kept in the browser session for 30 s, each
  running one Mutation of that canvas version, once. `kind=mutation` with `statement`, `nonce`, `state` and `activation`
  (the page's `navigator.userActivation.isActive` at the click) runs the program's own Mutation: only from the browser
  session with its token (`Decider::from_request`; a header login and a session one made are 403), only with the
  activation and an unused nonce of the click (403 without), and only when the user ∩ the agent may run its transition. `{"ok", "data" (the transition's JSON answer),
  "error", "approvalId", "statement", "channel": "canvas"}`; a Mutation the program does not have, or a request the
  gate does not hold (it does not write, the user may not run it), is 200 with `ok` false and the `error`.
- `canvasAdjust`: `text` (the user's correction) and `notices`, a turn: "Adjust canvas \"Visit hits\" v2: make it a line
  chart", with the notices on a line of their own.
- `canvasProblems`: `problems` (`[{source, code, message, statementId, component}]`, at most 10), and `revalidate`
  (default true). `{"artifactId", "version", "problems" (kept, each `{hash, skill, rule, problem, line, statement}`),
  "new", "revalidation"}`; `revalidation` is a turn's answer when the server asked the agent to fix them.
- `read` (`GET`): `path`, `list`, `query` (JSON text of flat values) and `artifactId` (or `conversationId`): the read
  proxy. 400 for a path that is not a plain `/apps` path, 403 for a transition that writes or a screen the user or the
  agent's role group may not open, 404 for none, 429 past 240 reads a minute, 502 for an answer that is not JSON.

### The calls of 13.14

Memory, lessons, Save as skill and the composer extras (plans/LLM-A2A-Assist.md 4.9, 4.11, 7.5, 7.6;
`crates/moqui-core/src/llm/assist/extras.rs`, `files.rs`, `llm/memory.rs`, `llm/skills/learn.rs`). Every call acts on
the caller's own rows only; anything of another user's is 404.

- **`POST queued`**: `inputId`, `op` `edit` (with `text`) or `cancel`: a message queued for the active run that it has
  not folded in. `{"inputId", "status": "queued"|"canceled", "text"}`; 409 when the run took it already (it is in the
  history: read the conversation again). The input row is read for update here and again by the run before it folds
  it in (with the text it has then), so an edit or a cancel and the fold never both win; a canceled one is taken
  without being folded.
- **`POST attachments`**: a multipart form with `file` and `conversationId` (optional). The file is checked against
  the conf's `assist` limits (bytes, media types with `type/*`; a program is refused whatever its type) and stored as a
  DbResource file at `dbresource://agent/files/<userId>/<artifactId>/<name>` (the property `agent_attachment_root`
  names another root) with an `AgentArtifact` of kind `file`. `{"attachmentId", "ref": "F…", "fileName",
  "mediaType", "size", "readable"}` (`readable`: a text type the agent reads); 400 `invalid` with the reason.
- **`GET attachment`**: `attachmentId`: the user's file as a download (`Content-Disposition: attachment`,
  `nosniff`).
- **`POST transcribe`**: a multipart form with `audio` (an `audio/*` type, at most `limits.maxAudioBytes`) and
  `language` (optional, `en`). The model bound to the `transcribe` feature on an OpenAI-compatible endpoint answers
  (`<endpoint url>/v1/audio/transcriptions`, the model's name, `response_format=json`): `{"text"}`, which the drawer
  puts in the composer to edit and never sends by itself. It is a model call as any other: the user's `api` quota is
  checked first (429 `quota`), it counts one hit of the AT_LLM tarpit (`assist:transcribe`, the seed's
  `AssistModelCalls` group: 60 a minute; 429 `tarpit`), the endpoint's address passes the private-network check, and
  it is an `LlmCall` (source `api`, its use estimated: the endpoint reports none). 501 `not_available` when no model is
  bound: the drawer then uses the browser's speech API only with `features.browserDictation`.
- **`POST textAction`**: `action` (`rewrite`, `shorten`, `translate`, `grammar`), `text` (plain text, at most
  `limits.maxTextActionChars`), `language` (the target of translate; the user's language by default), `field` (the
  field's label). One tool-free call on the `text-action` feature (else the fast tier): the system text is the fixed
  scaffold `framework/prompt/text-action.md` and a task line, and the text is the user message wrapped as
  `<untrusted from="field Note">…</untrusted>`, its markers elided, so it is data to change. `{"action", "text",
  "original"}`: a proposal the drawer shows with Replace, Insert below and Discard; nothing is applied by the server.
  It counts in the AT_LLM tarpit as `assist:textAction`; the quota and the `LlmCall` are `ec.llm.complete`'s.
- **`GET memories`**: `conversationId` (optional). `{"memories": [{"memoryId", "text", "agentId", "temporary",
  "thruDate", "createdDate", "lastUsedDate", "sourceConversationId", "status"}], "lessons": [{"lessonId", "text",
  "anchor" ("skill x", "screen /apps/…", "service a.b#C"), "status": "proposed"|"approved"|"rejected", "authorUserId",
  "createdDate", "useCount"}], "memory": {"preference", "conversation", "effective"}}`: the user's active memories and
  the lessons they wrote.
- **`POST memory`**: `op` `add` (`text` at most 200 characters, `days` for a temporary memory, 1 to 365), `edit`
  (`memoryId`, `text`, `days`), `forget` (`memoryId`: Forgotten, never used again), `setting` (`create` true or false;
  `scope` `always`, the user's `ASSIST_MEMORY` preference, or `conversation`, this conversation's own gate with
  `conversationId`, where an empty `create` clears it) and `lessonDelete` (`lessonId`, a proposed lesson of the
  user's). `{"memory": {...}}` (a memory for add and edit; the gates for setting).
- **`GET skills`**: the user's own personal skills: `{"skills": [{"skillId", "name", "title", "description", "kind",
  "status", "provenance" (user|learned), "risk", "shared", "updatedDate", "versionNum"}], "canAuthor"}`; with
  `skillId`, `{"skill": {..., "markdown"}}`.
- **`POST skill`**: `op` `draft` (Save as skill: `runId`, a completed run of the user's; `{"markdown", "steps"}`, not
  stored; 409 when the run took no screen or service step that worked), `save` (`markdown`, `skillId` to change one,
  `learned` true for a draft; `{"skillId", "name", "versionNum"}`, 400 with the front matter's problems or a name a
  site skill or another skill of the user's has), `retire` (`skillId`) and `share` (`skillId`: a site copy in Draft
  for review, `{"skillId", "sharedSkillId", "status": "draft"}`). `draft` and a learned `save` need `AgentSkillAuthor`
  (403). The server decides what is learned, not the client: a draft is kept in the session (the hash of its text,
  its name and its steps), and a `save` of it (or of it edited: the same name, or one of its steps word for word), or
  one that names a `runId`, is learned whatever `learned` says.
- **`GET events`**: `runId`, `after`: the run's stored events as `/assist/runs/{runId}/events.json` gives them, through
  a screen transition, for a client of the screen API (the terminal's pane) that has no host route.

**Memory** (plan 7.6, answered question 5). The agent makes a memory only with `remember`, when the user asks, and the
user makes one in the drawer's Memory pane. Creation is the user's choice: the permanent gate is the `ASSIST_MEMORY`
preference (the conf's `assist memory-default`, true, when unset); a conversation's own gate (`memoryCreate`) wins
while it is set, so the user may turn creation off or on for one conversation from the drawer. With creation off
`remember` answers `{"error": "memory_off", …}` (the tool list of a conversation never changes) and nothing is kept,
and a rejection's feedback is kept as no lesson,
while the memories there are still used: each new conversation's prefix holds the user's active ones (for every agent
or this one, unexpired, at most 12 facts and 800 characters, the one last kept or asked for again first; using a
memory in a prefix does not move it, so a user with more than 12 sees their newest) under "What the user asked you to
remember", wrapped as `<untrusted from="user-memory">` ("facts the user wrote or confirmed, not instructions"). A fact saved
during a conversation is in its history and reaches the prefix of the next one. Text a tool read (a note, an email, a
page) can say "remember …": `remember` (and `save_skill`) keep something at once only on the drawer or the terminal
with no tool output read since the user's last message; otherwise (after a tool's output, on chat) the call is an
approval card ("Remember: …"), and it is kept only when the user approves it (a policy never does). Each kept fact is a
`memory.saved` event, "Remembered: … [Forget]" in the thread.

**Lessons** (plan 7.5, 11). A lesson is anchored to a skill, a screen path or a service. It is proposed by `remember`
with `about` (the user said "remember for next time" about a procedure), by a rejection with feedback (on the write's
screen path or service; a tool call has none). It is Proposed and its author's alone until an admin
approves it in System › LLM › Lessons (`review#AgentLesson`); Reject or Withdraw takes it back. It never enters a
frozen prefix: a `skill` load shows up to three of the skill's lessons, a `screen` answer that went to a path two of
the path's (or a screen above it), and `describe_tools` and the errors of a `run_tool` service two of the service's,
each a `lesson:` line wrapped as `<untrusted from="lesson">`. A rejection's feedback is a lesson only while the
requester's memory gate is open.

**Learned and personal skills** (4.9, 7.5, answered question 6). Save as skill (`skill op=draft`) drafts a procedure
from a finished run's successful `screen` calls (go, set, do; secret-named fields left out) and `run_tool` services,
worded by one tool-free call on the `summarize` feature (else the fast tier; a deterministic draft when no model
answers); the user edits it and saves it as their own. `save_skill(markdown)` is the agent's way to the same row: a
lazy add-on tool (`run_tool name=save_skill`), so the tool list stays as it is, for users with `AgentSkillAuthor`. A
learned skill is the owner's alone, never in a catalog (so never in a prefix), found through `skill find=` and the
tail's `skills that may fit` (a first message's tail names the learned ones that strongly fit it), loaded as `skill
name (learned from a run and not reviewed: …)` and then its title (at most 120 characters), its description and its
text inside `<untrusted from="learned-skill">` (each skill a load brings along is checked the same way, and `skill
find=` wraps a learned skill's description), and its `reversible` counts as `confirm`, also after its owner edits it. Shared for review
(`op=share`), a Draft site copy waits in System › LLM › Skills; an admin publishing it makes it the site's own (provenance
`admin`), which may lower a risk. A personal skill a person wrote shadows a site skill of its name unless that is
locked; a learned one never takes the name of a site or file skill.

## The event stream

`GET /assist/runs/{runId}/events` is Server-Sent Events for the run's owner (anyone else 404; no user 401; an origin
the server does not allow 403; more than 256 streams on a node 503 with `Retry-After`). It replays the stored events
after `Last-Event-ID` (or `?after=`), then follows the run live. A stored event is `id: <seq>`, `event: <type>`, `data:
{"seq", "type", "payload"}`; a live one has no `id:` and `seq` null. `retry: 2000` comes first, `: ping` every 15 s. The
stream ends after `run.done`, or after a `run.state` that pauses the run (INPUT_REQUIRED or AUTH_REQUIRED): open it
again from the last id after the user acts. `GET /assist/runs/{runId}/events.json?after=` is the catch-up of a client
that polls: `{"events": [{"seq", "type", "payload"}], "lastEventSeq", "statusId", "state"}` (at most 1000).

The events (`crates/moqui-core/src/llm/run/events.rs` is the sink; the payloads are built where each is emitted):

| Event | Kept | Payload |
| --- | --- | --- |
| `run.state` | stored | `runId, conversationId, state, statusId, reason`; `pauseKind` (`Agps…`) and `pauseRef` on a pause; `errorCode` on a failure |
| `run.done` | stored | `runId, state, result` (`{text, messageId, invented?}` on Completed) `, errorCode` |
| `error` | stored | `code, message` (before a Failed `run.state`) |
| `message.start` | stored | `messageId, role, stepSeq, model` |
| `text.delta` | live | `messageId, part, offset, text` |
| `text.chunk` | stored | `messageId, part, offset, text` (deltas coalesced every 300 ms or 512 characters) |
| `part.done` | stored | `messageId, part, length` |
| `step.restart` | stored | `messageId, attempt, reason`: drop the text streamed for the message |
| `reasoning.delta` | live | `messageId, text` |
| `reasoning.summary` | stored | `messageId, text` |
| `tool.args` | live | `messageId, index, callId?, name?, text` |
| `tool.call` | stored | `toolCallId, messageId, name, arguments, classification` |
| `tool.result` | stored | `toolCallId, name, status (done|failed|unknown-outcome|denied), isError, text` |
| `input.folded` | stored | `inputId, kind`: a queued message became part of the history |
| `usage` | stored | `stepSeq, input, cachedInput, cacheRatio, output, reasoning, cost` (`cacheRatio`, the share of the prompt read from the provider's cache, is the cache-hit ratio of the call) |
| `compaction` | stored | `stepSeq, stages, beforeTokens, afterTokens, digested, summarized, summaryFallback, full`: the window was compacted before the step's call (plan 7.8); `stages` are those applied, of `digests, reasoning, canvas, tails, summary, current`; `full` ends the run `context_full` |
| `answer.references` | stored | `messageId, invented, text`: links no tool returned, to show plain |
| `approval.requested` | stored | the approval card |
| `approval.decided` | stored | `approvalId, toolCallId, decision (approve|reject|edit|expire), status, channel?, feedback?, handoffId?, policy?` |
| `question.asked` | stored | the question card's fields with `questionId, toolCallId, surface`; a budget one `questionId, text, choices, kind: "budget", exhausted` |
| `question.answered` | stored | `toolCallId, answer` or `questionId, continue` |
| `handoff.started` | stored | `handoffId, approvalId, path, parameters, form, transition, values: [{field, label, value}], expiresAt, clientId, contextVersion` |
| `handoff.done` | stored | `handoffId, approvalId, outcome (saved|abandoned), values?, messages?` |
| `ui.navigate` | stored | `path, parameters, title, handoffId?, clientId, contextVersion` |
| `ui.fill` | stored | `scope (page|dialog|find), form, domId, occurrence, row, dialog?, field, part, value, label, tuiId, handoffId?, clientId, contextVersion` |
| `ui.dialog` | stored | `id, title, open, clientId, contextVersion` |
| `ui.focus`, `ui.highlight` | stored | `tuiId, kind, label` and for a field `scope, form, domId, occurrence, row, field`; `clientId, contextVersion` |
| `canvas.open` | stored | the canvas as `conversation` gives it (a `write_ui` that checked) |
| `canvas.delta` | live | `messageId, index, patch, offset, text`: the program of a `write_ui` call as it streams, the new text past `offset` |
| `memory.saved` | stored | the memory `remember` kept (`memoryId, text, temporary, thruDate, …`): the drawer shows "Remembered: … [Forget]" |
| `lesson.proposed` | stored | `lessonId, text, anchor`: a lesson `remember` with `about` proposed |
| `artifact.update` | stored | `artifactId, kind: "canvas", name, version, dialect` (for the A2A projection) |

The low-rate signals go to every tab of the user over `/notws` as non-stored notifications of topic `AgentRun`: a run
started or changed state (the `run.state` payload), `{runId, conversationId, approvalId, event: "approval.requested",
summary}`, `{runId, approvalId, event: "approval.decided", status}`, `{runId, approvalId, handoffId, event:
"handoff.started"}`, `{runId, handoffId, event: "handoff.done", outcome}`, and for a canvas `{event: "canvas.closed" |
"canvas.problems", conversationId, artifactId, version, problems?}` and a click's `{event: "approval.decided",
approvalId, status, channel: "canvas", conversationId}` (no run carries these: a canvas is used after its run ended).
A tab that is not streaming reads
`conversation` or `tasks` again when one arrives.

## Follow

Only the **follow owner** applies the agent's page events. The server stamps every `ui.*` and `handoff.started` event
with the conversation's follow owner (`clientId`) and page version (`contextVersion`) as they stood when the command
began, so:

1. A tab makes its `clientId` once and keeps a `contextVersion` per conversation, starting from the one
   `conversation.follow` (or a turn's `follow`) gives.
2. Sending a turn makes the tab the owner. "Follow here" posts `follow`.
3. When the user navigates in the owner tab (the URL listener, not a navigation the agent caused) or opens a page
   with a conversation in the drawer, the tab raises its `contextVersion` and posts `context` with the page state.
4. The tab applies a `ui.*` event only when `payload.clientId` is its own, follow is on, `payload.contextVersion`
   equals its own, the event is past its applied sequence and no load is running; it never applies `ui.*` events on a
   replay after a reload. Another tab shows "Following in another tab".

On the server the conversation keeps the owner and the version (`AgentConversation.clientId`, `contextVersion`), and the
screen session takes the owner's page at each turn and `context` (`llm::screen::sync_in`, checked against the
conversation's owner), so the agent continues from where the user is.

## The drawer

`runtime/base-component/webroot/screen/webroot/js/WebrootAssist.js` is `moqui.assist`; its tests are `WebrootAssist.test.js`
(`node --test`, run by `cargo test -p moqui-core --lib the_qvt_client_passes_its_node_tests`). It loads on every `/qapps`
page after `WebrootChat.js` and before `WebrootVue.qvt.js` (`qapps.xml`), so its components are in `moqui.qvtComponents`,
which the client registers and `WebrootVue.qvt.render.js` precompiles (a `webapp_qvt_static_only` page runs it too); a part
that loads later registers through `qvt.registerLate(name, def)`. The page's settings carry `assist` (the `llm` feature built
and enabled and the user's `AgentUse`, from `qapps.xml`); without it the drawer makes no request at all. With it the root
calls `moqui.assist.start(root)` once mounted, which reads `bootstrap` (the button shows when an agent answers), the tasks
count, and reopens the tab's conversation of before a reload (`sessionStorage`), or the one `?assist=<conversationId>` names.

**The components.** `m-assist-button` (rightmost in the header, the waiting count as its badge) and `m-shell-assist`, one
right `q-drawer` of the shell's layout, mounted on first open and kept: nothing is reparented or teleported. It is
`persistent`: an overlay `q-drawer` hides itself on every navigation and on Esc, and this one follows the agent across pages
(it closes by Close, Mod+J and a second Esc). 420 px by
default, 320–720 by the handle on its left edge (kept as `localStorage` `ASSIST_WIDTH`), the full width below 600 px; it
pushes the page from 1440 px and overlays it below. Inside: the header (agent picker when there are several, which starts
a new conversation; the tier unless the agent fixes it; New, History, Tasks with the badge, a menu with Follow on this
page, Close), the status line, "Following in another tab" with Follow here, the chips, then `m-assist-thread`,
`m-assist-card`s and `m-assist-composer`, or `m-assist-history` or `m-assist-tasks`.

**The store** is a pure reducer of one conversation (`assist.emptyState`):

- `assist.snapshot(state, conversationJson, {clientId})` takes `GET conversation`: the items, runs, pending cards, queued
  input and the follow owner. This tab's messages the server does not list yet stay at the end, and a message the tab
  streams keeps its text when the stored one is shorter. A run first seen here replays its events from 0 (everything is
  idempotent) with its `floor` at the `lastEventSeq` the snapshot gives: a `ui.*` or `handoff.started` event at or below it
  is never applied, so a reload never replays a page action. A run the tab streams keeps its place.
- `assist.apply(state, runId, event, {clientId, followOn})` applies one event `{seq, type, payload}`. A stored event at or
  below the run's sequence changes nothing; one past the next answers `{kind: "gap", after}` and changes nothing (the
  transport reads `events.json?after=` and applies those in order). Text goes by `(messageId, part, offset)`: the
  stream's part `n` is the message's `n`-th text part (the stream counts from 0, the stored parts from 1), only the text past
  what is there is added, a live delta past the end waits for its stored chunk, and a stored chunk past the end or a
  `part.done` longer than the text reads the conversation again (`{kind: "refresh"}`); `step.restart` empties the message's
  text. A tool call is one step row by `toolCallId`; `tool.result` sets its status and result. The cards: an approval is
  pending until `approval.decided` (its status, or one from the decision: approved, rejected, handed-off, expired), a
  question open until `question.answered` (by `questionId` or `toolCallId`; the budget question's options are its
  choices), a hand-off watching until `handoff.done`. `input.folded` moves a queued message into the history (an unknown
  input id refreshes). `run.state` and `run.done` answer `paused` or `ended`: a paused run stays the active one.
- `assist.threadRows(state)` is what the thread draws: user messages (sending, failed with Retry, or queued at the end),
  an assistant message's text, its reasoning ("Thought") and its steps (each with its result, and its approval card in
  place), the done line after a run's last message (steps, tokens, seconds, and cost when the server gave it; Retry on a
  failed or canceled run), notes of the `/` commands.

**The transport.** A turn posts with `X-CSRF-Token` (`moqui.request`) and the object parameters as JSON text
(`assist.callParams`); its `clientMessageId` stays with the message until the server answers, so Retry sends the same
message. The active run's stream is `fetch` of `paths.events` with `Last-Event-ID` and `?after=` (the run's last sequence),
read frame by frame (`assist.parseSse`); it is open only while a run is active, not paused, and the tab visible (hidden:
closed; visible again: a catch-up, then the stream). It ends at a pause or `run.done`; a decision, an answer or Skip opens it
again from the last id. A stream that ends early is opened again after 500 ms doubling to 15 s (the count starts over once
a stream brings an event). A stream refused (503, no stream) or ended three times without a reason turns into polling
`events.json` each second while events come, slower (to 5 s) while none do, until the run ends or waits for the user (the
catch-up's `state` says so when its events do not). An `AgentRun` signal on the notification socket reads the tasks count again,
and the conversation when it is this tab's and the tab does not stream that run (a run started or decided in another tab).

**Follow on the page.**

- A `ui.*` (and `handoff.started`) event is applied (`assist.follow.applies`) only when follow is on in this tab, the
  payload's `clientId` is the tab's, its `contextVersion` equals the tab's, and it is past the run's floor; else it is a
  chip ("Assist opened Orders — Show", "Assist filled Name"; at most five, a navigation starts them over). Applied events
  wait in a queue until no load runs and the last navigation is a moment old, and are checked again then.
- `ui.navigate` goes through `assist.safePath` and `setUrl`, flagged so the URL listener does not count it as the user's
  (nor a navigation the page makes by itself within 1.5 s after it with no input of the user's between, such as a redirect).
- `ui.fill`, `ui.focus` and `ui.highlight` find their field with `assist.follow.locate(target, forms)` over the page's
  registered forms in DOM order (`assist.gatherForms`): a list row by its row form `<list>_<row>`, else a multi form's
  `<field>_<row>`; else the form whose qvt id is `domId` (several with it, as a static render repeats the bare name: the
  `occurrence`-th); else by name, the name or `<name>_<n>` (a merged render appends the section entry index), the
  `occurrence`-th within the scope (a dialog's forms for `dialog`, the page's before the dialogs' otherwise); a list's find
  form is `<list>_header`. Refused: a form outside `assistReach` (the canvas, the open tray, the quick-create dialog, and the
  screen dialogs; never the header, the drawer or the palette), a hidden, password or file field, a secret name, a field the
  form does not have. A fill sets the form's value (dependent fields follow), outlines the field with an "Assist" tag until
  it is focused or for 8 s, and is announced in a polite region; a form still mounting after a navigation is looked for
  again for 3 s. `ui.dialog` opens or closes a container dialog by its id.
- The user's own navigation (the root's URL listener) raises the tab's `contextVersion` at once (`assist.nextContextVersion`)
  and drops the queued page actions, then posts `context` with the page state once the page has loaded (from the owner tab,
  or any tab when none owns follow). Sending a turn makes the tab the owner; Follow here posts `follow`; "Follow on this page"
  off (kept as `localStorage` `ASSIST_FOLLOW`) turns the page actions into chips.
- **The page state** of a turn, retry and context (`assist.currentPageState`, `assist.pageState`): the root's
  `screenContext` (path, parameters, record, selection, filters and form summaries) as the contract names them, the screen
  title, the open dialogs, and `assist.follow.unsaved`: per form (by its qvt id) the values that differ from what it came
  with and their names in `dirty`. A secret name (`bootstrap.secretFields`) is never read, a hidden, password or file
  widget and the transition parameters are left out, a list value is joined with commas; at most 20 forms of 100 values of
  4000 characters (`bootstrap.limits`).
- **The hand-off.** Edit in screen posts `decide` with `edit`, the tab's `clientId` and its `contextVersion`, which makes the
  tab the owner and turns follow on; the tab takes the answer's `follow` (`assist.takeFollow`), so a tab that moved without
  reporting (it was not the owner) applies the hand-off's events too. `handoff.started`, then the agent's `ui.navigate` and `ui.fill`, show the filled form; the hand-off's form
  (its name, with a section entry index, or a form posting to its transition) has its Save ringed (`data-assist-save`), and the
  drawer shows the values Assist filled with Skip. The client hook `moqui.onFormAnswer(fn)` (called once per background
  submit of `m-form`, after its answer and in its catch for a refused one, with `{form, ok, json, status}`) reports the
  user's own save with `submitted` (`ok`, `messageInfos`, `errors`, `validationErrors`, `screenUrl`; never the form's
  values); a refused save keeps it watching ("The save was refused" in its card), and its form's Save works again at once
  (`m-form` takes its button's 3 s lock off when the answer is a refusal). Skip posts `submitted` with `abandoned: "skipped"`; the user leaving
  the hand-off's page (not within 3 s after a save that went through) posts `abandoned: "navigated"`.

**`assist.safePath(path, parameters, {linkBase})`**, the client's one path check (the server is the authority): a path under
`/apps` or `/qapps`; no scheme or host, no `//`, `..` or `.` segment, backslash, whitespace or control character, also once
decoded, no anchor; parameters flat (string, number or boolean values, names of letters, digits, `_`, `.` and `-`, at most
50). It answers the `/qapps` URL to `setUrl`. The tests port Java's `RequestTool.validatePath` and
`AssistOpenUiNav.validatePath` cases.

**The thread's text.** An assistant text is Markdown through `moqui.markdown.render` (Marked, with `escapeHtml`: raw HTML
in the text is shown as text, as the model never needs it) and `assist.sanitize`: DOMPurify (`assist.purifyOptions`: no
style, form, media, frame or SVG) to a fragment, then `assist.cleanTree` on the cleaned DOM (every image that is not this
site's own path a placeholder, every URL-bearing attribute but a checked link's `href` and a site image's `src` removed,
event attributes removed), drawn again at most once a frame while it streams
(`assist.renderMarkdown`): a path link only when `safePath` takes it, then through the shell; an external `http(s)` link in a
new tab with its host shown; a link the server named in `answer.references`, a mail, script, relative or anchor link as text;
images as placeholders with their host, never fetched (one of this site's own paths stays). A record chip
`[[record:Entity:id|label]]` is drawn only when the message's chip map (`item.chips`, by `Entity:id`, with `url` and
`label`) holds it, else it is its label; the user's own chips (from `@`) are drawn without a link. The server sends no chip
map yet (the model is not told of record chips and no tool returns record references to build one from; with 13.6's
skills), so an assistant's chip is its label.

**The composer.** Enter sends, Shift+Enter is a new line, ↑ in an empty composer recalls the last message. `@` searches
`mentions` with `kind=skills` and `kind=records` (the palette's sources; a row with `recordEntity` and `recordId`): up to
three skills first, inserted as `@name`, then records, inserted as their record chip. `/new`, `/tier
fast|balanced|thorough`, `/skill <name> [text]` (sent as `@name text`) and `/help`; an admin also has `/prompt`, `/tools` and
`/usage` (the System › LLM screens), `/context` (the page state the next turn sends) and `/skills` (`mentions kind=skills`).
Any other text that starts with `/` (a pasted `/apps` path) is a message. Stop posts `cancel` (the partial answer stays).

**The cards.** An approval: its summary, record chip, the risk a loaded skill gives the write (`risk: {skill, risk}`,
shown as a line; it never skips the card), "field: current → proposed" rows, Approve, Edit in screen (when
`canEdit`), Edit arguments for a service or tool call (approved with `arguments`), Reject with a feedback box (and the
server's `alternatives` as chips), Preview effects (when the server can preview), the preview's rows; decided, it collapses
into its status; a conflict offers Review again. A question: its options (the recommended one marked; several with `multi`),
its typed fields, free text; the budget question's continue and stop. A 409 or 404 on a decision reads the conversation
again. No card has a keyboard accelerator.

**The shell's hooks** (in `WebrootVue.qvt.js`): `moqui.onFormAnswer`; `qvt.registerLate`; the palette's last group "Ask
Assist ‘…’" (a `?` first is that row alone, Shift+Enter asks), which opens the drawer and sends the text; palette rows that
keep `recordEntity` and `recordId`; Mod+J opens the drawer and focuses the composer (from inside the drawer it closes),
Mod+. stops the run, Esc in the drawer returns focus to the page and a second Esc closes it (`moqui.keymap` actions
`assist` and `assistStop`).

**For the browser smokes** the drawer has stable hooks: `#m-assist-button` (badge `[data-assist-badge]`),
`#m-assist-drawer`, `#m-assist-input` (the composer's textarea), `#m-assist-send`, `#m-assist-stop`, `#m-assist-new`,
`#m-assist-history`, `#m-assist-tasks`, `#m-assist-status`, `#m-assist-elsewhere`, `#m-assist-follow-here`,
`#m-assist-chips`, `#m-assist-thread` (rows `[data-assist-item=user|assistant|step|done|queued|note|reasoning]`, a step's
`data-tool` and `data-status`, a local message's `data-assist-local`; an assistant row's text is its `.m-assist-markdown`), cards `[data-assist-card=approval|question|handoff]`
with `data-status` and `data-approval-id` / `data-question-id` / `data-handoff-id`, their buttons
`[data-assist-action=approve|edit|edit-arguments|reject|reject-send|preview|review|answer|skip|resend|retry]`, the feedback box
`[data-assist-feedback]`, a question's options `[data-assist-option]`; on the page a filled field
`[data-assist-touched=<field>]` (`[data-assist-highlight]` for a highlight) and the hand-off's Save
`[data-assist-save=<handoffId>]`: attributes, not classes, as a field's or a button's component sets its class again on every
update. The composer's id is on its textarea through `q-input`'s `for`.
`moqui.assist.state` (its `convo` the store) and `moqui.assist.clientId` can be read from the page.

**The Chromium smokes** are the `assist` module of `crates/moqui-host/tests/gate3_browser.rs` (`cargo test -p moqui-host
--test gate3_browser assist::`; the browser is `pdf-chromium-path`, for instance `pdf_chromium_path="/Applications/Google
Chrome.app/Contents/MacOS/Google Chrome"`, else the downloaded or `PATH` Chromium), against the scripted fake provider
(`fake::script` of the endpoint `gate-assist`) and a fixture component (`assistgate`: Edit saves an `EnumerationType`, Slow
takes 5 s to render): a run streams (Mod+J, Enter, the text in pieces, the done line) and a reload in the middle of the next
run re-attaches and ends with its text once; the drawer on a `webapp_qvt_static_only` page under a policy without
`unsafe-eval`, with an answer whose images and raw HTML fetch and run nothing; a hand-off on the gallery's Contact
(`example/screen/Gallery/Contact.xml`, a form whose save is refused with validation errors) reports the refused save and then
saves; two tabs, where only the sender follows, the other shows "Following in another tab" and a chip, an approval shows in
both and executes from the first while the second sees it executed; the user's own navigation while the agent's command runs
wins (the agent's navigation is a chip); and Edit in screen from the tab that does not follow, after it moved on its own,
follows there. With `MOQUI_SHELL_SHOTS=<dir>` they save `assist-<thread|approval|elsewhere|handoff>-<light|dark>-<1280|390>.png`;
`example/test/gallery-shots.mjs --assist` opens the drawer on a running server's pages.

**The extras of 13.14** (the calls are "The calls of 13.14").

- **Composer**: an attach button, drop and paste. Files are checked against `limits` (count, size, `type/*` patterns),
  then uploaded with `POST attachments`; chips show their state, and Send waits while one uploads. The turn posts
  `attachments` as JSON text, and a user message's `file` parts are chips that download through `GET attachment`.
  Dictation records with `MediaRecorder` and posts `transcribe` when `features.transcribe`, else uses the browser's
  speech API when `features.browserDictation`, else shows no microphone; the text goes into the draft and is never sent
  by itself.
- **Thread**: queued rows get Edit and Cancel (`POST queued`; a 409 or 404 reads the conversation again). Suggested
  prompts come from `GET prompts`, debounced and asked again when the path, record or agent changes: in the empty
  state, else above the composer; a `send` chip sends at once, another fills the composer. The header has the context
  meter (`conversation.context.percent` of the budget, amber from 80%, the tokens and model in its tooltip). Each answer
  gets its response actions: Copy, the thumbs, the builtin download, and the apps' actions opened through the shell's
  quick-create frame (`openQuickCreate`) with the page's record, the text put in the frame form's field (from
  `sessionStorage`, never in the URL); a `requiresRecord` one only on a record. A `memory.saved` event is a note
  "Remembered: … [Forget]" (`[data-assist-remembered=<id>]`, action `remembered-forget`), a `lesson.proposed` one "Lesson
  kept for …". A completed run's done line has Save as skill
  (`features.saveSkill`), which opens the skill editor on the draft.
- **Panes**: Memory (the memories with add, edit and forget, "Assist may remember what I ask" with "only for this
  conversation", and the user's lessons with their status and Delete for a proposed one) and My skills (open and edit,
  retire, share for review), from the header menu, which also toggles the permanent memory gate.
- **Inline text actions**: `m-rich-text` offers Rewrite, Shorten, Translate and Fix grammar in its toolbar when
  `moqui.assist.textActionsOn()` (not on a shared, co-edited document); the selection's text, or the field's as plain
  text, goes to `POST textAction`, and the proposal shows under the toolbar with Replace, Insert below and Discard. Only
  a click applies it, and a selection that changed meanwhile is never overwritten.
- **Degrading**: a 501 or 404 from prompts, attachments, transcribe, textAction, memories, skills or a skill draft turns
  that feature off; the drawer goes on.
- Hooks for the smokes: `#m-assist-attach`, `#m-assist-file`, `#m-assist-attachments`
  (`[data-assist-attachment=uploading|ready|failed]`), `#m-assist-dictate` (`data-assist-dictation=transcribe|browser`,
  `data-recording`), `#m-assist-prompts`, `#m-assist-prompts-empty` (`[data-assist-prompt][data-send]`),
  `#m-assist-context` (`data-assist-context-level=normal|warning`, `data-percent`), `#m-assist-remember-toggle`,
  `#m-assist-memory`, `#m-assist-skills`; in the thread `[data-assist-file=<id>]`, `[data-assist-response-action=<key>]`,
  queued rows' `[data-input-id]` and `[data-assist-queued-text]`, actions `queued-edit|queued-cancel|queued-save|save-skill`;
  `#m-assist-memory-pane` (`#m-assist-memory-create`, `#m-assist-memory-scope`, actions
  `memory-add|memory-edit|memory-save|memory-forget|lesson-delete`, `[data-assist-memory-id]`,
  `[data-assist-lesson-id][data-status]`); `#m-assist-skills-pane`, `#m-assist-skill-editor`, `#m-assist-skill-save`
  (actions `skill-share|skill-retire|skill-save|skill-back`, `[data-assist-skill-errors]`, `[data-assist-skill-saved]`);
  on a rich-text field `[data-assist-text-actions]`, `[data-assist-text-action=rewrite|shorten|translate|grammar]`,
  `[data-assist-text-proposal][data-state]`, `[data-assist-text-apply=replace|insert|discard]`. These have node tests
  (`WebrootAssist.test.js`), not Chromium smokes yet.

**Not in the client yet**: the canvas's phone tabs (Chat, Canvas) and its Wide and Page modes (the canvas takes the page
beside the docked drawer, "The canvas"); "Summarize earlier messages" beside the context meter; a language picker for
Translate (the user's language is the target); the history's filters, rename, export and attach; "Let Assist save it"
on a hand-off (the server has no call that turns a hand-off back into an approval) and the phone's "View page" pill.
The composer is a plain text area rather than `m-rich-text`.

## The canvas

A canvas is a generated view, the fallback when no screen shows what the user asked for (plan D5, 4.10): a chart of a
list, a dashboard of several lists, a batch confirm. The agent writes it with `write_ui` in OpenUI Lang. It takes the
page while the drawer stays docked beside it (the drawer pushes the page rather than covering it while a canvas is
shown, from 600 px), and Back to the page shows the screen again, which stayed mounted under it; Show canvas in the
drawer brings it back.

**`write_ui`** (`crates/moqui-core/src/llm/tools/ui.rs`: `kind` openui, `title`, `program` the whole program, `patch` only
the changed statements, `run` a Mutation of the canvas to run) is in the tool list from the start with a small schema
and answers `{"error":"skill_required","skill":"openui-lang",…}` until the skill `openui-lang` (any part of it) was
loaded in the conversation (`llm::skills::loaded`). It is READ for the policy and the fence; the canvas is the run's own
record, written under the run's lease with the fence lifted for it, as an approval is. A kind the agent does not write
(its `canvasDialects`) is answered `dialect_unavailable` before its skill is asked for; `vue-sfc` is
[below](#vue-sfc-canvases).

**Parsed once, on the server** (`crates/moqui-core/src/llm/ui/openui.rs`, J28). The one parser is the vendored
`@openuidev/lang-core` 0.2.18 (`webroot/libs/openui/lang-core.min.js`, the global `OpenUILang`), built by `libs/refresh.mjs`
from the npm package with two reviewed patches of `libs/patches/` (telemetry off; a failed Query is not fetched again
until its arguments change), recorded with their sha256 in `MANIFEST.json`; `node libs/refresh.mjs` verifies it. The
server runs the same bundle in a QuickJS runtime of its own (one a thread, 64 MiB, 5 s a call) with the library spec
`webroot/js/openui-library.spec.json`. A program is parsed and checked before anything is shown. A patch is parsed alone
first, then merged onto the canvas with lang-core's own `mergeStatements` (the same name replaces, a new one is added,
`name = null` removes one, what root no longer reaches is dropped), and the merged program is checked. There is no
repair layer: a problem is an error the model reads, `openui_parse` (it does not parse) or `openui_invalid` (it parses
and does not check), naming the skill section to load and quoting its rule:

```json
{"error":"openui_parse","instruction":"The program did not parse, so nothing was shown: a bracket of this Query or Mutation is not closed, so it swallows the lines after it. The rule is in skill openui-lang, section query; load it with skill name \"openui-lang#query\" if it is not above. Fix line 2 and send the whole program again.",
 "skill":"openui-lang#query","rule":"Query is Query(\"request\", {path, query: {...}}, {rows: []}): close the options object with }} before the default data.",
 "line":2,"snippet":"2 | visits = Query(\"request\", {path: \"/apps/system/Visit/VisitList\", query: {…}, {rows: []})"}
```

The checks and their sections: a statement that does not close, a line that is no statement, no root, named arguments,
a `$` that holds a Query or a computed value, quoted expressions and single quotes (`syntax`); unknown components (with
the nearest names) and arguments that do not fit, with the component's signature (`components`, or `forms` for a
control); invented `@` functions and JavaScript calls (`functions`); the Query and Mutation shapes, a path that is not an
`/apps` screen, a Mutation body that reads a Query (`query`); a name used and not defined, one root does not reach, a
Query no component shows, a visible filter no Query reads (`verify`); a Link that is not a screen path (`links`); a
patch that renames a statement (`edit`). A program that does not parse is reported for that alone, at most five
problems an error (the first in full, the others under `also`). Every rule is quoted verbatim in its section of
`runtime/base-component/framework/skill/openui-lang.md` (a test checks it, and that every signature of the spec is in the
skill), so the error and the skill say the same thing.

**Stored once** (J5). A conversation has one canvas, an `AgentArtifact` of kind `canvas` updated in place: part 1 the
merged program, part 2 its data (statement and Mutation names, the page's state, the reported problems with their hashes,
the revalidations, closed). The history keeps only what the model sent (a whole program once, or a patch); the result it
reads is `canvas v2 patched and shown: "Visit hits" (3 statements: root, visits, chart)`, never the program again; the
tail of the next message names the canvas (`canvas "Visit hits" v2`) unless the user closed it.

**The library and the renderer** (`js/WebrootAssistCanvas.js`, loaded with lang-core on the first canvas). The 46
components of the Java prototype's library are declared there (positional parameters, JSON Schema types, the skill's
group) and drawn with Vue 3 render functions over the shell's widgets: `m-text-line`, `m-text-area`, `m-date-time`,
`m-date-period`, `m-chart` (Chart.js 4), `m-markdown`, `m-mermaid` and Quasar's layout, so a static-only page and a CSP
without `unsafe-eval` run it. `canvas.librarySpecText()` is `openui-library.spec.json` (a node test fails when the file is
stale). The renderer ports `assist/AssistOpenUi.js`'s state reconciliation (`persistedState`, `reconcileUserEdits`: a
changed default replaces only a value the user left alone), the streaming parse (a whole program is drawn while it
streams, from `canvas.delta`; a patch shows that it updates until `canvas.open`), action routing and evaluation; it drops
the regex repair layer. The dialect registers itself (`moqui.assist.dialects.register("openui", {component:
"m-assist-openui"})`); the host is `m-assist-canvas` in `WebrootAssist.js`, shown in the root's page container.

**Nothing reaches another site.** A canvas's Markdown is the drawer's own in its strict mode (`m-assist-markdown strict`):
no image of any kind (each a placeholder) and no link but a checked path of the apps, so data in the text cannot be sent
anywhere. An `Image` is a static file of this site (an image's extension, no query, never under `/apps`, `/qapps`,
`/rest` or `/assist`, whose GET would run with the cookie); a `Link` goes through `safePath`; a Mermaid height is a CSS
length only. lang-core's bundle has no process, network or telemetry call (its telemetry and git code is emptied by
the reviewed patch, and `refresh.mjs` fails on `child_process`, `execFile`, `getBuiltinModule`, `sendBeacon`,
`XMLHttpRequest`, `fetch(` and the telemetry hosts).

**Reads: one proxy** (`crates/moqui-core/src/llm/ui/proxy.rs`, J12). A Query, a Lookup's options and a Display's value are
`GET /assist/read`, never a request of the page to a screen: the path renders in-process as the user from the agent's
origin, under the read fence with every write held, as `data` reads. A screen answers one of its lists from its `tui`
render (reading as the user ∩ the agent: the agent's role groups must permit the screen or the transition, viewed, and
the user must be allowed it), `{rows, totalCount, list, lists, columns: [{key, title}]}`, each row keyed by the names of the fields its columns
show (`ScreenTui.js` gives each list column its `fields`, and `data` shows them as a `row keys:` line where the drawer can
show a canvas), with `_id` and `_link`; the values are the text the list shows. A transition that answers JSON (a
lookup) answers it as it is; one that writes is 403.

**Writes: the click is the approval** (answered open question 13). A Mutation never posts from the page: the page names
the Mutation, and the server evaluates its arguments from the stored program with the posted state (lang-core's
`evaluate` in QuickJS), holds the request at the transition gate (a fenced render under `Hold`, which answers the held
request and its hash and runs nothing) and runs that exact request under `Approved` with the hash
(`crates/moqui-core/src/llm/ui/write.rs`). Before it is held, when an approval resumes and for a click, the user ∩ the
agent is checked as the screen tool checks its held writes: the agent's role groups on the transition and the user's own
permission for the path (`check_write`). In the page a Mutation is reached only from a Button's `act()`, through a token
whose key is a Symbol no program can write, so a Query (its arguments are program data) never reaches one; and the
server refuses a Query or Mutation option it does not take and any `__` name in them. A Button that saves shows what,
beside its label ("saves: updateEnumType", `[data-assist-canvas-saves]`), so the click is an informed approval. So:

- a **click** on a Button the framework drew (a trusted event with the page's user activation; `canvasAction` `kind=arm`,
  then `kind=mutation` with the click's nonce, from the browser session with its token) is the approval: no card is shown, and an `AgentApproval` row records it, decided through
  the channel `canvas`, Executed or Failed with the result;
- a Mutation the **agent proposes without a click** (`write_ui` with `run`, which the skill's `query` section offers) is an
  approval card like any other: the run pauses, the card shows the request's parameters, and approving it runs the held
  request with the state the canvas last posted. This is the phase's check "a canvas Mutation yields a card".

A Vue SFC submit never posts either: it is a confirm the page draws, whose Save is such a click
([below](#vue-sfc-canvases)).

**Adjust and revalidation.** Under the canvas the user can write a correction (Adjust): `canvasAdjust` is a turn that
names the canvas and its version, which the skill's `edit` section makes the agent's only task. After it draws, the page
reports its problems (lang-core's parse errors, names it could not resolve, evaluation errors, failed Queries) with
`canvasProblems`; the server maps each to its section and rule as its own checks are, deduplicates them by hash against
the canvas's, keeps at most 10, and once a version (`REVALIDATIONS`) asks the agent to fix the new ones with a patch (a
turn). The canvas shows the kept problems under it.

**For the browser smokes**: `#m-assist-canvas` (`data-version`, `data-streaming`), `#m-assist-canvas-title`,
`#m-assist-canvas-back`, `#m-assist-canvas-close`, `#m-assist-canvas-problems`, `#m-assist-show-canvas` (in the drawer while
the canvas is hidden), a canvas Button `[data-assist-canvas-button=<label>]`, a Link `[data-assist-canvas-link]`, a Form
`[data-assist-canvas-form]`. The smoke `the_canvas_shows_over_the_page_and_a_canvas_mutation_yields_a_card_in_chromium` (the
`assist` module of `gate3_browser.rs`, on the scripted fake): `skill`, then `write_ui` shows a canvas whose Query reads a
screen list through the proxy; typing on it and clicking Save runs its Mutation with no card; the agent's `write_ui` `run`
is a card, and approving it writes; Back to the page shows the screen (`MOQUI_SHELL_SHOTS` saves `assist-canvas-*` and
`assist-canvas-card-*`). `crates/moqui-host/tests/it/assist.rs` `the_canvas_over_http_on_postgres` covers the calls over
HTTP; `WebrootAssistCanvas.test.js` the library spec golden, lang-core's parses, the state reconciliation and the drawer's
canvas state; `crates/moqui-core/src/llm/ui/` the checks, the sections, the merge, the stream and the dialects. The
evaluation task "chart fallback" (`tools/test/agent/assist.agent.js`) passes on its hand-authored recording in two
`write_ui` calls.

## Vue SFC canvases

The escape hatch of the canvases (plan 4.10, J11, J12): a Vue 3 single-file component the agent writes when OpenUI Lang
cannot express a layout. It is **off by default**: only an agent whose `AgentDefinition.canvasDialects` names `vue-sfc`
(a comma list; empty is `openui` only) gets the `ui.write:vue-sfc` capability in its runs, the dialect in `bootstrap` and
the skill `vue-sfc` in its index (the skill of a dialect an agent does not write is neither listed nor loaded).

**The program** is the component's blocks: `<template>`, `<script>` (`module.exports = {...}`, the Options API; no
`import`, `export default`, `<script setup>` or TypeScript), an optional `<style>` and an optional `<actions>`, a JSON list
of `{name, label, path, fields, parameters}`: the writes it may ask for. A patch is a program with only the blocks that
change; each replaces the canvas's block of its name. The skill is
`runtime/base-component/framework/skill/vue-sfc.md` (sections `shape`, `sandbox`, `bridge`, `widgets`, `example`; every
rule of `llm/ui/vue_sfc.rs` is quoted in its section, and the example checks, which a test asserts).

**Checked on the server** (`crates/moqui-core/src/llm/ui/vue_sfc.rs`) before anything is shown, each problem naming its
section and quoting its rule (`vue_sfc_invalid`, with the `block` and its line): at most 64k characters; the blocks
(nothing outside them, none twice, a template); the template compiled with the vendored `libs/vue/compiler-dom.cjs.js`
in QuickJS (the compiler of the qvt screens; the same version as the browser build); the script compiled, not run, in a
QuickJS runtime of its own; names the frame does not have (`window`, `document`, `location`, `fetch`, `XMLHttpRequest`,
`WebSocket`, `eval`, `Function`, `navigator`, the storages, `open`, …, as free names outside strings and comments; a
property such as `row.location` is a field) and properties that lead out (`ownerDocument`, `defaultView`,
`constructor`, `innerHTML`, …), in the script and in the template's bindings and `{{ }}`; elements that load, post or
run (`script`, `style`, `iframe`, `object`, `embed`, `form`, `base`, `meta`, `link`), `v-html`, `srcdoc`, `http-equiv`,
`javascript:` URLs, a protocol-relative link; a `<style>` with `@import` or a `url()` that is not `data:`; an `m-`
component other than the six widgets, `router-link`; the actions (a JSON list of at most 10, a plain unique name, a
path that is an `/apps` transition, POST, PUT, PATCH or DELETE, plain field and parameter names, no secret name, fixed
parameters that are scalars); and a `$bridge.submit('name'` that no action declares. These checks tell the model what
does not work; they are not the boundary (a computed name such as `this.$el['owner' + 'Document']` passes them). The
boundary is the frame.

**The frame** (`GET /assist/sandbox`, `crates/moqui-host/src/llm/sandbox.rs`; the document and its policy are
`vue_sfc::frame_document`). The canvas is `<iframe sandbox="allow-scripts" src="/assist/sandbox?v=…#n=<nonce>"
referrerpolicy="no-referrer">`: without `allow-same-origin` the document has an opaque origin (no page DOM, cookies,
storage or session token), and without the other tokens no popups, forms, top navigation, modals or downloads. The
document holds no data and needs no session; it is a host route because the screens send `frame-ancestors 'none'` and a
`srcdoc` frame would inherit the drawer page's policy. Its own policy:

```
sandbox allow-scripts; default-src 'none';
script-src <origin>/libs/vue/ <origin>/libs/quasar/ <origin>/js/WebrootAssistSandbox.js 'unsafe-eval';
style-src <origin>/libs/ 'unsafe-inline'; font-src <origin>/libs/; img-src data: <origin>/images/ <origin>/libs/;
connect-src 'none'; media-src 'none'; object-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none';
manifest-src 'none'; form-action 'none'; frame-ancestors 'self' <origin>; base-uri 'none'
```

with `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`, a `Permissions-Policy` that turns the powerful
features off, and no `X-Frame-Options`. `<origin>` is written out from the request (its `Host`, which must be a plain
host and port, else 400, and HTTPS on TLS or from a trusted proxy), since `'self'` does not match an opaque origin. The
`sandbox` directive sandboxes the document even when it is opened on its own. `?x=1` (the user's
`ASSIST_CANVAS_EXTERNAL`) adds `https:` to `img-src` only. **Compiled in the frame**: the component's template is compiled
by Vue 3's full build (`libs/vue/vue.global.prod.js`) inside the frame, the only place `'unsafe-eval'` is allowed, and
the script is evaluated there as `new Function("module", "exports", script)`; the server compiled both first to report
errors, but ships source, not code. Fonts and styles are the vendored Quasar and Material ones; the nonce is in the URL
fragment, so no inline script is needed and it never reaches a server log.

**The frame runtime** (`js/WebrootAssistSandbox.js`) takes one MessageChannel port, once, from `window.parent` with
the nonce of its fragment, then mounts the component with Quasar 2, the six widget shims (`m-text-line`, `m-drop-down`
with options or `options-url` through the bridge, `m-date-time`, `m-display`, `m-date-period`, `m-container-box`) and
`this.$bridge`: `get(path, query, list)` (a Promise of the read's JSON) and `submit(name, values)` (a Promise of `{status:
saved | canceled | failed | ignored, error}`). A click on a link is caught (capture phase) and sent as `link`; the frame
never navigates for one. It pings every 2 s, reports its height, its compile and runtime errors and, on `pagehide`,
`leaving`. It is as untrusted as the component: nothing it says is believed beyond its shape.

| parent → frame | frame → parent |
| --- | --- |
| `port {nonce}` (window message, target `*`, the port transferred), then on the port: `init {template, script, style, values, dark, external}`, `values {values}`, `theme {dark}`, `result {id, ok, json, status, error}` | `ready`, `rendered`, `ping`, `input {values}`, `get {id, path, query, list}`, `submit {id, action, values}`, `link {href}`, `resize {height}`, `error {phase, code, message}`, `leaving` |

**The parent** (`m-assist-vue-sfc` in `js/WebrootAssistCanvas.js`; its pure parts are `moqui.assist.canvas.sfc`, under
node tests) listens only on its end of the channel, never on window messages, and passes every message through
`sfc.validate`: an object with a known string `t`, at most 256k characters as JSON, and only the fields of its type,
cleaned (flat values only: at most 100, plain names, no `__`, strings of at most 4000; a read path `/apps` or `/qapps`
of plain characters with no `..`, doubled slash or host, a flat query of at most 30 short values, a list name; a plain
action name; a height clamped to 60–4000). What fails is dropped (a read or submit is answered refused to its id).

- **Reads** go to the one read proxy, `GET /assist/read` with the canvas's id, as OpenUI's do (fenced, the user ∩ the
  agent, a transition that writes is 403), at most 6 at once and 60 in 10 s from a frame, an answer of at most 1.5 MB.
  A Vue SFC canvas reads **only the paths it declared** in its `<reads>` block (a JSON list of `/apps` screen paths,
  stored on the canvas as `reads` and checked by the proxy with `vue_sfc::read_declared`); an undeclared path is 403.
  And it does **not read at all while the user lets it reach other sites** (`ASSIST_CANVAS_EXTERNAL`): the proxy refuses
  every read then, and the client refuses it too, so the "Other sites" option and proxy reads are never on together
  (their combination is the only way an image of another site could carry a read's data out).
- **Writes** (J11). A `submit` counts only when the page has the user's transient activation (`navigator.userActivation
  .isActive`: a click in a child frame activates its ancestors), the frame has the focus (`document.activeElement` is the
  iframe, so the activation came from a click in the frame and not from typing in the drawer), the action is the
  program's, and none counted in the last 2 s; otherwise it is ignored (`status: "ignored"`, counted in
  `data-assist-sfc-ignored`, and a submit without a click is reported as a problem of the `bridge` section). A submit
  that counts never posts: the page draws a confirm outside the frame with the action's label, the transition it saves
  ("saves: updateEnumType") and every value the server will post (the action's fixed parameters and its fields from the
  values, `sfc.confirmRows`, which is what `vue_sfc::action_request` builds, with the method shown — for example
  `saves: DELETE updateEnumType` — when it is not a POST). The confirm is a modal `q-dialog` with Cancel focused, and its
  Save is dead for the first 600 ms after the confirm appears (`confirmArmMs`), and the frame cannot resize while it is
  open, so a frame moving under a waiting pointer cannot have a click land on Save (clickjacking). Only the user's
  trusted, armed click on Save runs it, exactly as an OpenUI Button: `canvasAction` `kind=arm` then `kind=mutation` with the click's nonce; the
  server builds the request from the **stored** program's action with the posted values, holds it at the transition gate,
  checks the user ∩ the agent and runs the held request under `Approved` (an `AgentApproval` through the channel
  `canvas`). The agent's `write_ui` `run` of an action is an approval card, as for OpenUI. **Trust model**: activation
  cannot be proven across the opaque frame boundary (the frame's code can call submit whenever the page happens to be
  active), so the activation and focus gate only keeps a component from asking without the user's gesture; the approval
  is the click on the framework-drawn confirm, which the frame cannot reach (another origin's DOM), cannot script
  (`isTrusted`), and whose content (the values posted) the server reproduces from its own copy of the program.
- **Links** open only from the user's click in the frame, with the same activation-and-focus gate as a submit (the
  runtime sends a `link` only for a trusted event; the parent re-checks `navigator.userActivation.isActive` and the frame
  focus), so no navigation happens without a gesture. An `/apps` or `/qapps` screen of this site (through `safePath`)
  opens beside the canvas as an OpenUI Link does; another site opens in a new tab (`noopener,noreferrer`) only when the
  user allows other sites; anything else (this site's other paths, `javascript:`, `data:`) is not opened and the canvas
  says so.
- **The watchdog** (`sfc.watchdog`, a timer of a second): a load of the frame after it took its port (the frame
  navigated itself), or a `leaving`, closes the frame (`navigation`); so does no `rendered` 15 s after the first load
  (`timeout`), no message for 10 s after it rendered (`unresponsive`), or more than 120 messages in a second
  (`flood`). A closed frame is removed (`[data-assist-sfc-killed=<reason>]`, with Show again) and the reason is
  reported to `canvasProblems` as source `sandbox`, kept on the canvas as a problem of `vue-sfc#sandbox`, in the tail,
  and revalidated once a version like any other.

**Residual risks**, accepted (answered open question 2: the watchdog and the link and resource restrictions). These are
why the dialect is **off by default per agent**, and why **the worker renderer is required before Vue SFC is enabled for
any agent that reads untrusted content** (a component in a worker that sends DOM operations to an allowlist applier with
no `href`, `action`, `<meta>` or navigation; the watchdog and the checks are not a substitute for it). A component the
model wrote, perhaps steered by text in data it read, can still move data it read out of the page through channels CSP
does not close, and the watchdog catches a navigation only after the request has left:

- **Self-navigation and downloads.** The frame can navigate itself (`location` through a computed name, a `<meta>`
  refresh written into the DOM, an `<a download>`), and a navigation answered **204/205 or a download** fires neither a
  `load` nor a `pagehide`, so it leaves **no watchdog signal at all**; even an ordinary navigation leaves only after the
  request, carrying whatever the component put in the URL, has gone. CSP ships no navigation directive (`navigate-to` was
  dropped) and the Navigation API's `navigate` event does not fire in an opaque-origin document, so the frame cannot be
  made to cancel it.
- **WebRTC.** `connect-src 'none'` does **not** govern `RTCPeerConnection`, so a STUN/TURN server URL could carry data
  out over DNS/UDP. The runtime deletes `RTCPeerConnection`, `webkitRTCPeerConnection` and `RTCDataChannel` before the
  component runs and the server's check refuses them, but a renderer the model replaced could restore them; only the
  worker renderer removes the capability for good.
- **Other sites' images.** The "Other sites" option adds `https:` to `img-src`, so `new Image().src` could carry a
  read's data to any host. That is why a Vue SFC canvas **cannot read through the proxy while other sites are on** (above):
  the two are mutually exclusive, which closes this channel but does not remove the toggle's own risk for content the
  user pastes in.
- A read returns to the component whatever the user ∩ the agent may read; a component that loops forever may hold a CPU
  core (a sandboxed frame is out of process in current Chromium, so the watchdog can still close it, but the browser does
  not promise a separate thread). `'unsafe-eval'` is on inside the frame only; DNS prefetch is not stopped by CSP in
  every browser. The server's name checks (which also ban `self`, `top`, `parent` and the WebRTC constructors) are
  **defence in depth only** — a computed name slips past them — so they are never the boundary; the frame is.

**For the browser smokes**: `[data-assist-sfc-frame]` (the iframe), `.m-assist-vue-sfc` (`data-assist-sfc-ignored`,
`data-assist-sfc-dropped`), `[data-assist-sfc-confirm=<action>]` with `[data-assist-sfc-confirm-row=<name>]`,
`[data-assist-sfc-confirm-save]` and `[data-assist-sfc-confirm-cancel]`, `[data-assist-sfc-killed=<reason>]`,
`[data-assist-sfc-notice]`, `[data-assist-sfc-external]`. The smoke
`a_vue_sfc_canvas_runs_sandboxed_and_holds_the_java_probes_in_chromium` (the `assist` module of `gate3_browser.rs`, on
the scripted fake) ports the probes of the Java prototype's `framework/test/vue_sfc_sandbox_browser_smoke.py`: the
route's headers; the frame's attributes and its opaque origin; from inside the frame, through computed names, the
parent's DOM, the top window, cookies, both storages, a popup and top navigation are blocked; fetch, XHR, a WebSocket,
another site's image, this site's image outside the static paths and a form post are blocked; a list read through the
bridge answers, a transition that writes is refused by the proxy (403) and the paths that are not `/apps` never leave the
page; a submit on mount is ignored, and a message posted to the page window is nothing; a real click in the frame shows
the confirm with what it saves, a scripted click on its Save arms nothing, the user's click saves; a component that
navigates its frame is closed and recorded as a `vue-sfc#sandbox` problem (`MOQUI_SHELL_SHOTS` saves `assist-sfc-*` and
`assist-sfc-killed-*`). `WebrootAssistCanvas.test.js` covers the validator, the blocks and actions, the read paths,
the link targets, the watchdog, the submit gate and the read limits; `crates/moqui-core/src/llm/ui/vue_sfc_tests.rs` the
checks and their sections, the merge, the actions' request, the skill, and the policy; `crates/moqui-host/src/llm/
sandbox.rs` the route's headers.

## On the server

- **The run.** `turn` builds a `StartRequest` (surface drawer, the tail from the page state, the client message id,
  the tier, a new conversation's screen state, `defer_submit`) for `llm::run::start_run`, which makes the run or queues the
  message, counts the tarpit where it makes a run (`run::count_run_hit`, under the conversation's lock) and answers an
  earlier send of the same client message id (`run::existing_message`); the turn then claims follow and syncs the page in,
  and hands the run to the pool (`run::submit_started`), so the run's first command sees the tab as the owner. A queued message is an `AgentInput` with its client message id, which the folded message
  keeps; a run completing checks for one under the conversation's lock (the lock a queued message takes first) and goes
  on one more step instead (`Change::unless_input`).
- **Links from other surfaces.** An approval of an A2A task or an MCP call ([MCP.md](MCP.md)) is decided in the drawer:
  its link (`/qapps?assist=<conversationId>`) opens the conversation, whose `pending.approvals` has the card. An MCP
  call's conversation (surface `mcp`, agent `mcp`) holds a user line naming the client, the tool call and its result;
  its run is never stepped by the pool, so a decision there is read by the MCP client's retry, which runs the write.
- **The tier.** `AgentRun.tier` keeps the client's choice; a step uses the agent's feature binding unless the agent
  fixes no tier and the bound model is of another tier, then the first usable model of the chosen one
  (`Registry::tier_model`).
- **The tail.** `llm::assist::state::DrawerState::tail_spec` and `llm::prompt::build_tail`: the user's local time and
  week start, the page with its title, the record, selections, filters and form summaries, the conversation's canvas as
  the server has it, capped at 1,200 characters.
- **Authority.** The calls run without artifact authorization (no app's screens cover them, as the shell and chat
  transitions do) and check the user, `AgentUse` and ownership themselves; the run's tools run as the user, with the
  user's own permissions, under the agent's policy (plan 4.3).
- **The window** (plan 7.8, `llm::window`). Before each call the step estimates the request (the newest reply's reported
  prompt tokens, `AgentMessage.tokenCount` with its request's `promptChars`, plus the characters since, at the model's
  calibrated characters a token) and compacts only above 85% of the budget, down to 55%, in stages: digests of older tool
  results, replayed reasoning, old canvases, old tails, a summary of the turns before the last three user messages (on the
  agent's model, `summary.md`), and last the turn in progress. What a compaction did is stored with the messages (a
  result part's `digest`, a message's `tailLine` and `reasoningStripped`, a call part's `compactedArguments`, `compacted`
  messages and the conversation's `compactionSummary`), so the drawer's history still shows every message whole and the
  calls between compactions share the provider's prefix cache. A provider's overflow compacts at once.
- **Small models** (plan 7.9). A model's `tool-mode="json"` freezes its conversations' prefix with the tools as signatures
  (`tools-json.md`; `promptVariant` stored as `small+json`), sends no native tools, and reads each reply as one protocol
  object (`llm::json_mode`); its protocol text is not streamed, the answer is sent once read. A native model's reply that is
  only calls written as text (`<tool_call>` blocks, or one bare call object) is taken as calls, the streamed text voided
  with `step.restart`; a call quoted in prose, a fence or quotes never runs. `intent-prefilter="hint"`
  adds `likely: <intent>` to the stored tail of a new message. A model's `max-tool-calls` (12 for the small variant, 30
  otherwise) caps a run's calls, the same failing call twice answers `repeated`, and arguments the deterministic repair
  cannot fit get one schema-constrained call when the model's `json-schema` verdict is yes.

## Chat members, triggers and AI tasks

Phase 13.9 (plan 4.13, 4.14). The code: `crates/moqui-core/src/llm/chat.rs` (chat agent members and the room's
cards), `trigger.rs` (triggers, the loop guard, batching, limits, the breaker and the inbox), `ai_task.rs` (typed AI
tasks, `ec.llm.summarize`/`classify`/`extract`/`startRun`, `propose#AiFieldValue` and its `set_field` tool) and
`run/observe.rs` (the observer worker that hands a run's committed events to them). Tests:
`crates/moqui-core/src/llm/members_tests.rs` (in memory, scripted fake), `crates/moqui-host/tests/it/agent_members.rs`
(the loop guard through the real data feed on embedded Postgres) and the node tests of `js/WebrootChat.test.js`.

### Chat agent members

- **Joining.** An agent's `AgentDefinition.chatUserId` is a service user (the seed's `_AGENT_ASSIST_`, "Assist", has no
  password). A moderator adds it to a room with role `agent` and the `agentRef`
  `org.moqui.agent.AgentServices.handle#ChatMessage` (`add#Member`, `set#MemberRole`). The agent must serve the chat
  surface.
- **What it answers.** `send#Message` hands each message to the agent members (`deliver#AgentMessage`, async).
  `handle#ChatMessage` reads the message from the room, so the sender is whoever posted it, and answers a message that
  mentions the agent, any message of a direct room with it, and the sender's next message in a thread where the agent
  already works for them. Messages of agents (the chat or run-as user of any agent) start nothing. A message sent
  before the agent joined or before the conversation began, one the conversation answered already, and one older than
  five minutes that is not its sender's newest are left, so handing the entry point an old message starts no run. Only
  an agent's chat user may be given the `agent` role (`add#Member`, `set#MemberRole`).
  The sender needs `AgentUse` (else a one-line reply says so).
- **The run.** As the sender, on the chat surface (`question.inline`, `approval.link`), in the conversation of
  `(room, thread root, sender)` (`AgentConversation.chatRoomId`, `threadRootEventSeq`); the reply's thread is the
  message's thread, or a new thread on the message, and a direct room's timeline. The first message of a conversation in
  a thread brings up to ten earlier messages as `<untrusted from="chat">`. The chat addendum names the room
  (`${roomName}`). The same message again starts no second run (`clientRequestId` `chat:<room>:<seq>:<agent>`).
- **In the room.** While the run works the agent types every 3 seconds (`signal#Typing` with `userId` and `statusText`,
  as "Searching…", from the tool it calls). An approval or a question is a card message, `content.assist`
  `{kind, runId, conversationId, itemId, requesterUserId, requesterName, title, summary, rows | options, state}`
  (`send#AssistCard`), changed in place by `update#AssistCard` (pushed as `update`, `fields.assist`); when the run ends
  each approval card shows how its approval ended. The answer is a message of kind `reply` naming the run, so a client
  offers "View run" to the requester. Steps stay out of the room.
- **Who decides** (answered question 12). Any human user who may post in the room decides an approval card, through
  the card's button: `POST /chatAssistCard` (webroot.xml) → `org.moqui.agent.AgentServices.decide#ChatCard`, taken only
  from a browser session by its cookie with the session token (`approval::Decider::from_request` with channel `chat`, so a
  header login, an agent's context and internal code are refused); an agent member, the chat or run-as user of any agent,
  or a user who may not post never decides (not found). The write runs as the requester. A member who is not the
  requester approves the write as it stands or rejects it: edited arguments, Edit in screen and a question's answer
  stay the requester's, and a member's rejection keeps no feedback and gives the agent no task (it reads only that
  another member rejected it, and asks the requester). The plan's section 1 said the card is "for the sender only": the card is a room
  message every member sees, naming the requester, and it is the decision that question 12 opens to the room. A card
  shows the changed fields' current and proposed values to the room, as the agent's reply would.
- **The client.** `js/WebrootChat.js`: `moqui.chat.assistCard(event, me)` (who may act, the lines to show),
  `moqui.chat.decideCard(roomId, eventSeq, params)`, `moqui.chat.typingText(roomId, userId)`, and the component
  `<m-assist-card :event :chat-room-id>` that a chat screen writes for a message with `event.assist` (BOSS's chat screen).

### Triggers

- **`AgentTrigger`** (System › LLM › Triggers; `org.moqui.agent.AgentTriggerServices.store#AgentTrigger`): an agent, a
  kind (`AgttCron` with `cronExpression`, `AgttDocument` with `dataDocumentId`), instructions, a run-as user (else the
  agent's `runAsUserId`), `batchSeconds`, `maxBatchEvents` (50), `maxRunsPerHour`, `maxRunsPerDay`, `maxConcurrent`,
  `failureLimit` (3), `approvalHold`, `paused`; `maxRunsPerHour` is 60 and `maxConcurrent` 2 when empty, so a loop the
  guard does not see stays bounded. A trigger runs as an agent's service user (an AgentDefinition's run-as or chat user)
  or as the admin who stores it, never as another person. The trigger row is locked from the limit check to the run
  tagged with it, so concurrent firings keep the limits. A run is on the job surface; its `triggerId` and `triggerRef` (the cause
  and the events) are on the `AgentRun`.
- **Firing.** A cron trigger's `ServiceJob` `agent_trigger_<triggerId>` calls `fire#AgentTrigger`. A document trigger's
  DataDocument is linked to the seeded `AgentTriggers` data feed, whose receive service stores each document as an
  `AgentTriggerEvent`; a window of 0 fires once the delivery commits, a longer one when the scheduler's tick finds its
  oldest event older than the window. One run takes the waiting events (claimed with a conditional update), with each
  record as `<untrusted from="record">`.
- **The loop guard.** Each write's data-feed touch records the `ExecutionContext`'s agent origin (set by every step of a
  run, approved writes included); `DataFeedPending.originAgentId` is the agent when every write of the record in the
  transaction came from it (async service calls, run-on-commit and async ECAs, and service jobs started from a run's
  context carry the origin to their worker; a call a full queue stores as a one-shot job does not), the delivery gives a non-search receive service the document with `_originAgentId` (a
  delete with `originAgentId`), and a trigger drops a document its own agent changed.
- **Limits and the breaker.** A firing over a limit is skipped (logged), and a document trigger's events wait. A failed
  run, or a firing that cannot start one, counts; a completed run resets the count; at `failureLimit` in a row the
  breaker opens (`breakerState` open, the cron job paused), the AgentAdmin users and the run-as user get an inbox
  message, and nothing fires until **Reset Breaker** (`reset#AgentTriggerBreaker`).
- **The inbox** (`AgentInbox` topic, kept; System › LLM › Inbox lists the viewer's own, D12): a job-surface run's result or
  failure goes to its user, and so does an approval it waits for (the drawer's tasks pane lists it, and the user decides
  it there). A background agent's policy rule approves by itself; a trigger with `approvalHold` Y lets a write its
  policy does not approve wait for its run-as user's decision, else that write is an error the model reads.

### AI tasks and AI fields

- **`org.moqui.agent.AiTaskServices`**: `run#Summarize` (`summary`), `run#Classify` (`label`, `labelList` with
  `multiple`), `run#Extract` (`valueMap` from `fieldMap` or `schema`), `run#Translate`, `run#Generate`, and `run#AiTask`
  by `taskType` with an `input` map; each is one structured completion (`LlmFacade::complete` with the task's JSON
  schema) on the task's feature (`summarize`, `classify`, …) when bound, else `assist`, with the caller's text wrapped as
  untrusted; `list#AiTaskTypes` is the registry. A component adds a task with `<ai-task name service feature
  description>` in its `llm-facade` conf, the service an implementation of `prepare#AiTask` (system, prompt, schema).
- **Async**: `async=true` answers a `taskId` (a tool-less `AgentRun` of the job surface, `triggerRef` `ai-task:<type>`)
  and runs the task on a service worker (`execute#AiTask`); `get#AiTask` gives its state and output; its user gets an
  inbox message and, with `webhookUrl` (https, public addresses only), a POST of `{taskId, taskType, state, output |
  error}`.
- **`ec.llm`**: `summarize(text, {maxWords})`, `classify(text, labels)`, `extract(text, fields)`, `startRun(agentId,
  text)` (a job-surface run as the current user, which needs `AgentUse` and `AgentAdmin`, as `start#AgentRun` does).
- **`propose#AiFieldValue`** (entityName, the key as `pk` or `recordId`, fieldName, instructions): the `ai-field`
  feature's model (else `assist`) proposes `proposedValue` with a `reason`; nothing is written. `mode=approval` ("Update
  with AI") starts a drawer run attached to the record whose first step is a seeded `set_field` call (the tool exists
  only for runs with the `ai.field` capability): its approval card shows current against proposed, it runs only when the
  user approves (never by a policy), the record's `lastUpdatedStamp` is compared first (a change meanwhile is a
  conflict), and the write is the entity-auto update as the user.

## Admin screens

System › LLM › **Triggers** (list, detail with Fire Now and Reset Breaker, the waiting events and the runs) and
**Inbox** (the viewer's own AgentInbox messages) come with phase 13.9.

System › LLM › **Lessons** (13.14): the lessons with their anchor, author and what taught them, Proposed first, with
Approve, Reject and Withdraw (`review#AgentLesson`); read a lesson as text that may carry an injected instruction
before approving it. A shared skill waits as a Draft site skill in **Skills**, where publishing it makes it the site's.
A user's own memories, lessons and personal skills are in the drawer's Memory and My skills panes.

System › LLM › **Conversations**, **Runs** and **Approvals** (`AgentAdmin`, as all of System › LLM). The lists show
who, which agent and surface, the status, the dates, counts, tokens and cost, never content. A detail screen shows its
content (a conversation's transcript, a run's tool arguments and results, events and approvals, an approval's
arguments, changes, edited arguments, preview and outcome) only to its own user or a user with `AgentAudit`, and each
such read writes an `AgentAuditLog` row (auditor, target user, conversation, run, reason):
`org.moqui.agent.AgentAdminServices.get#AgentAuditView`. Run detail has the state box (status, pause, lease, attempts,
budgets against their caps, tokens, cost, capabilities) with **Cancel**, which stops the run as its own Stop does
(`cancel#AgentRun`); nothing there acts in another user's conversation.
