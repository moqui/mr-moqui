# A2A: the agent-to-agent server

Moqui serves its agents to other agents over A2A 1.0, with both bindings: JSON-RPC 2.0 and HTTP+JSON. A remote agent
sends a message, the same run engine as the Assist drawer works it (the agent uses its screens and data as the caller,
with the headless addendum), and the answer comes back as the task's `response` artifact. Every write the agent wants
waits for its human owner: the task goes to `TASK_STATE_AUTH_REQUIRED` with a link the owner opens in Moqui, and it goes
on by itself once they decide. The design is [plans/LLM-A2A-Assist.md](../plans/LLM-A2A-Assist.md) (4.15, D9, D10,
4.5, 4.8). As of Oct 22, 2026 (13.10 built; the client, [Delegating to remote agents](#delegating-to-remote-agents),
13.12).

The code: `crates/moqui-core/src/llm/a2a/` (`mod.rs` the request, authentication and answers, `types.rs` the wire types
and checks, `ops.rs` the operations over the run model and the stream projection, `jsonrpc.rs` and `rest.rs` the two
bindings, `card.rs` the agent cards, `push.rs` the push notification configs and their delivery) and the host adapter
`crates/moqui-host/src/llm/a2a.rs`. Tests: `crates/moqui-core/src/llm/a2a/tests.rs` (the wire rules) and
`crates/moqui-host/tests/it/a2a.rs` (both bindings over HTTP on embedded Postgres with the scripted fake provider, and
the official SDK's interop script, below). The protocol shapes were checked against the A2A 1.0 schema as the
`a2a-sdk` 1.2.3 package carries it (`a2a.proto` as `a2a_pb2`) and its client and reference server.

## Turning it on

The server is off by default; while it is off `/a2a` and `/.well-known/agent-card.json` answer 404.

```xml
<llm-facade>
    <a2a enabled="${a2a_enabled}" public-url="${a2a_public_url}" default-agent="assist" blocking-wait="120"
            max-part-bytes="65536" max-parts="32" trust-forwarded-headers="false">
        <push allow-http="false" allow-hosts="" timeout="15" max-attempts="6"/>
        <security api-key="true" bearer="true" oidc-issuer="${a2a_oidc_issuer}" oidc-audience="${a2a_oidc_audience}"/>
    </a2a>
</llm-facade>
```

- `a2a_enabled=true` turns it on. An agent is reached over A2A only when its `AgentDefinition` has `exposeA2a="Y"` and
  serves `a2a` (`surfaces`, for example `drawer,chat,tui,a2a`); the seed's `assist` agent does not, so an admin opts it
  in (System › LLM › Agents). `a2aSkillsJson` (an array of A2A `AgentSkill` objects) describes the agent's skills on the
  extended card; without it the card lists one skill for the agent.
- A caller needs the `A2aServerUse` and `AgentUse` permissions, which a user with `EXTERNAL_USER` (an app's outside users,
  such as client contacts) never holds, whatever group grants them. What the agent may do is the caller's authority
  intersected with the agent's role group and tool policy, as everywhere (plan 4.3).
- `public-url` is the base URL advertised in the cards (the interfaces are `<base>/a2a`); without it the request's
  scheme and `Host`, and a proxy's `X-Forwarded-Proto` and `X-Forwarded-Host` only with
  `trust-forwarded-headers="true"` and from a peer the `client-ip-header` conf believes (a trusted proxy). A value that is not a plain origin (user info, a query, a fragment, `//`, control characters) is
  never advertised.

## Authentication

`Authorization: Bearer <login key>` (`security bearer`), the `api_key` or `login_key` header (`security api-key`), or
HTTP Basic with an app password (or a login key as the password) — never the account password, which is 401. Credentials are taken
only over TLS, a believed proxy's https or loopback. Without credentials the answer is 401 with
`WWW-Authenticate: Bearer realm="a2a", Basic realm="a2a"`; a user without the permissions is 403. A refused login counts
for web protection as a refused screen login does. Each run counts once against the AT_LLM tarpit (`agent:<agentId>`);
a caller over it gets 429 with `Retry-After`. OIDC JWTs come later (plan 11, answered question 10); until then the card
lists no `openIdConnect` scheme, whatever `oidc-issuer` says.

Every request but the public card needs `A2A-Version: 1.0` (or a 1.0 patch such as `1.0.1`); without it a request is
0.3, which is not served: `-32009` (JSON-RPC) or 400 `VERSION_NOT_SUPPORTED` (HTTP+JSON).

## The bindings

JSON-RPC 2.0 at `POST /a2a`; HTTP+JSON under `/a2a/`. Both carry the A2A 1.0 proto JSON (camelCase fields, enums by
name such as `TASK_STATE_COMPLETED` and `ROLE_AGENT`, times as RFC 3339). Results have only the fields of their message,
since the SDKs parse strictly; Moqui's own values are in `metadata` (a task's `agentId`) or in `data` parts.

| Operation | JSON-RPC method | HTTP+JSON |
| --- | --- | --- |
| Send a message | `SendMessage` | `POST /a2a/message:send` |
| Send, streamed | `SendStreamingMessage` | `POST /a2a/message:stream` |
| A task | `GetTask` (`id`, `historyLength`) | `GET /a2a/tasks/{id}?historyLength=` |
| The caller's tasks | `ListTasks` | `GET /a2a/tasks?contextId=&status=&pageSize=&pageToken=&historyLength=&statusTimestampAfter=&includeArtifacts=` |
| Cancel | `CancelTask` (`id`) | `POST /a2a/tasks/{id}:cancel` |
| Follow a task | `SubscribeToTask` (`id`) | `GET` or `POST /a2a/tasks/{id}:subscribe` |
| Push configs | `CreateTaskPushNotificationConfig`, `GetTaskPushNotificationConfig`, `ListTaskPushNotificationConfigs`, `DeleteTaskPushNotificationConfig` | `POST`/`GET /a2a/tasks/{id}/pushNotificationConfigs`, `GET`/`DELETE …/pushNotificationConfigs/{configId}` |
| The extended card | `GetExtendedAgentCard` | `GET /a2a/extendedAgentCard` |

**Errors.** JSON-RPC errors are HTTP 200 with `{jsonrpc, id, error: {code, message, data: [google.rpc.ErrorInfo]}}`, the
`ErrorInfo` carrying the A2A reason (`TASK_NOT_FOUND`, …) and the domain `a2a-protocol.org`. HTTP+JSON errors are a
`google.rpc.Status`, `{error: {code, status, message, details: [ErrorInfo]}}`, with the HTTP status of the error:

| Code | Reason | HTTP |
| --- | --- | --- |
| -32700 / -32600 / -32602 | JSON_PARSE, INVALID_REQUEST (a batch, a malformed envelope), INVALID_PARAMS | 400 |
| -32601 | METHOD_NOT_FOUND (a 0.3 name such as `message/send`, an unknown path) | 404 |
| -32001 | TASK_NOT_FOUND: no such task, or another user's (owner-scoped: never named) | 404 |
| -32002 | TASK_NOT_CANCELABLE: the task has ended | 400 |
| -32004 | UNSUPPORTED_OPERATION: a message to an ended task or to one waiting for its owner's approval; subscribing to an ended task | 400 |
| -32005 | CONTENT_TYPE_NOT_SUPPORTED: no accepted output mode is `text/plain` or `application/json` | 400 |
| -32009 | VERSION_NOT_SUPPORTED | 400 |
| -32603 | INTERNAL_ERROR: always "Internal error"; the cause is logged, never sent | 500 |

## Tasks

A context is a conversation and a task a run of the `a2a` surface; the task states are the run states, whose
`statusCode` is the A2A wire value (plan 4.4).

- **A message** (`message.role` `ROLE_USER`, a `messageId`, parts) starts a run of the agent in a new conversation, or
  in its `contextId`'s (the caller's, else `-32602`), or with a `taskId` goes to that task: it answers the task's
  question (`TASK_STATE_INPUT_REQUIRED`; a lone `data` part is a form's values or `{choice}`), or the running task
  folds it in at its next step. The agent is the request's or the message's `metadata.agentId`, else the agent whose
  card lists `metadata.skillId`, else `default-agent`. `referenceTaskIds` must be the caller's tasks.
- **Parts**: exactly one of `text`, `raw`, `url` and `data` each, at most `max-parts`, each at most `max-part-bytes`. A
  `url` part must be a `dbresource://` location of this server the caller may read (in a drive they may view, or a
  spill of one of their own runs) and is never fetched; a missing location and an unreadable one get the same answer; a `raw` part's bytes are
  not kept (the model and the history read `[Attached file …]`). The parts come back as sent in the history.
- **Idempotency**: the `messageId` is unique with the user. The same message again returns the task it made, as it now
  stands, and nothing runs twice; another user's message with the same id is their own.
- **Blocking**: `SendMessage` waits for the task to end or pause, up to `blocking-wait` seconds, then returns it as it
  stands; the wait is on a thread of its own, not a request worker. `configuration.returnImmediately` returns the task
  at once. `configuration.historyLength` limits the history returned.
- **The task**: `status.state`; `status.message` says what a paused task waits for (below) or why it failed (its
  error code only); `history` is the run's user and agent messages (`historyLength`: unset all, 0 none, n the newest);
  `artifacts` is the `response` artifact with the answer, once there is one.
- **ListTasks**: the caller's tasks, newest first, `pageSize` 1–100 (50) with an opaque keyset cursor over the submitted
  time and the task id, so a page never repeats or skips a task while tasks change; `totalSize`; filters by context,
  state and `statusTimestampAfter` (any change at or after it). A page's status times, pauses, history and parts are
  loaded in a fixed number of queries; artifacts only with `includeArtifacts`.
- **CancelTask**: a task that has not ended is asked to stop (a service call already running finishes); the task comes
  back canceled, or as it stands after up to 5 seconds, waited for on a thread of its own as a blocking send is. When it
  ends some other way first, that end stands.

**Waiting for someone.** A question of the agent (`ask`) is `TASK_STATE_INPUT_REQUIRED` with a text part and a data part
`{type: "moqui.question", questionId, text, options: [{label, recommended}], fields?, kind}`; the caller answers with a
message to the task. A write is `TASK_STATE_AUTH_REQUIRED` with a text part and a data part `{type: "moqui.approval",
approvalId, url, expiresAt, summary, status}`: `url` is `<base>/qapps?assist=<contextId>`, the drawer on the task's
conversation, where the owner sees the approval card and decides in their own browser session (Approve, Reject with
feedback, or Edit in screen). Nothing a remote caller sends can approve: a message to such a task is `-32004`, and the
decision is taken only from an interactive session (plan 4.8). The task resumes by itself after the decision; a
subscriber or a push config hears it.

## Streams

`SendStreamingMessage` and `SubscribeToTask` answer `text/event-stream`. The first item is the `task` as it stands, then
the run's stored, sequenced events projected: each state change is a `statusUpdate`, the answer's text an
`artifactUpdate` of the `response` artifact (`append` after the first chunk of a message; when the task completes, the
whole answer replaces it with `lastChunk: true`, before the final `statusUpdate`). The stream closes on a terminal or
interrupted state; a subscriber of a task waiting for its owner's approval follows it through the decision to the next
pause or the end. JSON-RPC frames are `data: {jsonrpc, id, result: StreamResponse}`, HTTP+JSON frames a bare
`StreamResponse`; `: ping` comes every 15 seconds. When a model call starts again (a retry after streamed text), the
text it streamed is void: an `artifactUpdate` replaces the `response` artifact with an empty text. A stream follows the run's events in the database and the node's live
hub as the drawer's does, so it has no gap when the run moves between nodes. At most 256 streams and blocking waits are
open on a node, and 16 of one user; one more is 503 (the node) or 429 (the user), with `Retry-After`.

## Push notifications

A push config (`taskPushNotificationConfig` of a send, or the config methods) names a `url`, an optional `token` and an
optional `authentication` `{scheme, credentials}`; the token and the credentials are stored encrypted and never
returned. As the 1.0 schema and its reference sender have it, a notification is a `POST` of a `StreamResponse`
(`statusUpdate` for each state, and a completed task's whole answer as an `artifactUpdate` first) with
`X-A2A-Notification-Token: <token>` and `Authorization: <scheme> <credentials>`; the 1.0 schema has no signed
notifications. A push URL is absolute http(s) with no user info or fragment, https unless `push allow-http`, and never
an internal address (written out, or resolved: the client checks the address it connects to) and no redirect is
followed; `push allow-hosts` lifts the https and address checks for the hosts it names (a loopback receiver in a test).
The outbox (`AgentPushDelivery`) is drained by a thread per node every second. It follows only the configs with new
events (each config's last projected event is kept; a round reads the configs made since the last one), sends only the
head notification of each config that has one due, and sends with four threads, at most two at once to one host, a
3 second connect timeout and a 20 second budget per round, so a slow or dead receiver holds up only its own configs. A
failed post waits 1 min, 5 min, 30 min, then 2 h, up to `max-attempts` in all, and the notifications of a config go in
order. A task takes at most 10 configs and a caller 50. A config lasts 7 days: once an hour the thread removes the
older configs with their notifications, and delivered or failed notifications of that age.

## Cards

`GET /.well-known/agent-card.json` is public and minimal: name, description and version (`a2a_agent_name`,
`a2a_agent_description`, `a2a_agent_version`), the JSON-RPC and HTTP+JSON interfaces at `<base>/a2a`, the security
schemes (`bearer`, `basic`, `apiKey`, and `oidc` when configured) with their requirements, the capabilities (streaming,
push notifications, the extended card) and one generic skill; it may be cached for five minutes. The extended card
(authenticated) lists the skills of the agents the caller may use.

## The interop script

`crates/moqui-host/tests/fixtures/a2a/interop_a2a_sdk.py` is the Java prototype's script ported to A2A 1.0 and both
bindings: discovery, then over JSON-RPC and over HTTP+JSON each SendMessage, GetTask, ListTasks, CancelTask of an ended
and of an unknown task, SendStreamingMessage, SubscribeToTask, CancelTask of a working task, a push notification to its
own loopback receiver, the push configs without secrets, and the extended card.

The venv it was checked with (Python 3.13, `a2a-sdk` 1.2.3, which brings `httpx` and `protobuf`), outside the
repository:

```sh
python3 -m venv ~/.cache/moqui-a2a-venv
~/.cache/moqui-a2a-venv/bin/pip install 'a2a-sdk==1.2.3' 'httpx>=0.28'
~/.cache/moqui-a2a-venv/bin/python -c 'import a2a.types, httpx; print("a2a-sdk ok")'
A2A_SDK_PYTHON=~/.cache/moqui-a2a-venv/bin/python LANG=en_US.UTF-8 \
    cargo test -p moqui-host --test it a2a:: -- --nocapture
```

The host test runs it against its own server (the scripted fake provider answers, the conf allows `127.0.0.1` in
`push allow-hosts`) when `A2A_SDK_PYTHON` is set, and prints its checks (`33 passed, 0 failed`). Without it the Rust
cases run alone and the test prints `SKIPPED: the a2a-sdk interop check … did not run`, so a run without Python says
it did not check interop. Against a running server:
`A2A_TOKEN=<login key> python interop_a2a_sdk.py https://moqui.example.com` (its push check needs the server's conf to
allow the script's host).

## Delegating to remote agents

The other direction: an agent here delegates to a remote A2A 1.0 agent through a tool `a2a__<remoteId>`, one for each
`AgentRemoteA2a` row (System > LLM > Remote > Remote Agents, `org.moqui.agent.RemoteAgentServices`). The code is
`crates/moqui-core/src/llm/a2a/client.rs` (the client), `crates/moqui-core/src/llm/tools/delegate.rs` (the tools) and
`crates/moqui-core/src/llm/remote.rs` (the addresses, credentials and SSRF limits it shares with the MCP client).

- **Which agents**: a row's `allowedAgents` lists the local agents that may delegate to it (comma separated, `*` for
  all, empty for none). The tools are in the lazy catalog: the model finds them with `find_tools`, reads their one
  parameter (`message`, all the remote needs: it sees nothing else of the conversation) with `describe_tools` and calls
  them through `run_tool`. The agent's tool policy applies to the name (`a2a__*` patterns work), with the class
  EXTERNAL.
- **Approval**: a delegation is EXTERNAL, which counts as a write: it asks for the user's approval with the message as
  its arguments (no sim preview), and runs only once approved, or at once for a background agent whose policy says
  `policy`. An approval that waits while an admin moves the row to another origin, or while the policy stops offering
  it, runs nothing.
- **Addresses**: the card URL is the admin's only; https, no user info, no internal address unless the row sets
  `allowPrivateNetwork`, which also allows http, but only to an internal host (an internal address, `localhost`, or a
  name that resolves only to internal addresses), so a credential never crosses the internet in cleartext. Internal addresses are refused at connect
  time too, by what a name resolves to (DNS rebinding included); a decimal, hex or IPv4-mapped address is read as the
  URL parser reads it. No redirect is followed. The card is read and kept on the row for an hour, for the URL it was
  read from (a changed URL, or a kept card that no longer reads, is read again); only its 1.0
  interfaces on the card's own origin are called (JSON-RPC before HTTP+JSON), so a card cannot send the credential
  elsewhere.
- **Credential**: `authJson`, encrypted: `{"bearer": key}`, `{"basic": {username, password}}` or `{"header": {name,
  value}}`, set on the screen by kind and secret and never shown. A Moqui remote takes a login key of the user the
  delegations run as there.
- **The call**: `SendStreamingMessage` when the card streams, else `SendMessage` and `GetTask` while the task works,
  until it ends or is interrupted, within the row's `timeoutSeconds` (else `a2a client-timeout`, 300 s); past it, or
  when the local run is canceled, the remote task is canceled. A second delegation to the same remote in a run goes on
  in the remote context of the first.
- **The answer** is the task's artifacts (else its status message), wrapped as untrusted text
  (`<untrusted from="a2a:<remoteId>">`, its markers elided inside), spilled when large as any result is. A failed,
  canceled or rejected task is an error the model reads; a remote's own error message is wrapped the same way.
- **A remote that waits**: `TASK_STATE_INPUT_REQUIRED` pauses the local run with the remote's question to the user (a
  form with one answer field); the answer goes back to the same remote task. `TASK_STATE_AUTH_REQUIRED` pauses it
  AuthRequired with the remote's link (a `url` in its status data, as Moqui's approval link is, else the first URL of its
  text; https, or http only to an internal host of a private row, else no link is shown) with the link's host on a line
  of its own, said to be the remote's, and Done or Cancel; Done reads the task again and follows it, Cancel cancels it.
  The task waits for the origin it was approved for: when an admin moves the row meanwhile, Done sends nothing. A local
  run on the A2A surface that waits so is `TASK_STATE_AUTH_REQUIRED` with a `moqui.remoteLink` data part (the text, the
  `url`, the options Done and Cancel); the caller answers it with a message, and a stream ends there (it follows only
  an approval or a hand-off through the owner's decision). A surface where nobody can
  answer (a job) cancels the remote task and says so. The waiting state is an `AgentRemoteTask` row of the tool call.
- **Never** in a sim, and never under the read fence: a delegation is no read.
- **Conf**: `<a2a client-enabled="true" client-timeout="300"/>` (the client is on by default; without rows there are no
  tools).

Tested in `crates/moqui-core/src/llm/tools/remote_tests.rs` against fixture servers on loopback (blocking and streaming,
a question, a link, a redirect) and in `crates/moqui-host/tests/it/agent_remote.rs` against this server's own A2A server
on a second server over loopback.

## Not yet

Long remote tasks: a delegation waits for its task within its timeout, and a `RemoteTask` pause resumed by the remote's
push notifications is not built; OIDC JWT validation (and with it the `openIdConnect` scheme on
the card); `LlmApiClient` client records with the `a2a` scope; tenants in the HTTP+JSON paths; the admin screen of A2A
tasks; the cleanup of old tasks (`clean#AgentData`; the push rows are cleaned by the push thread).
