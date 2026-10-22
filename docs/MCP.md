# MCP: the tool server

Moqui serves its tools to AI agents and coding assistants over the Model Context Protocol, version 2026-07-28: the
read tools `screen`, `search`, `data` and `skill`, and the services an admin exposes, each as the signed-in user with
that user's permissions. A read runs at once under the read fence; every write waits for its human owner, who approves
or rejects it in Moqui through a link the MCP client shows (a URL elicitation in a multi round-trip request, MRTR),
and the client's retry runs it once. The MCP client can never approve. The design is
[plans/LLM-A2A-Assist.md](../plans/LLM-A2A-Assist.md) (4.16, D10, 4.3, 4.6, 4.8, 9 "Approvals"). As of Oct 22, 2026
(13.11 built; the client, [Calling remote MCP servers](#calling-remote-mcp-servers), 13.12).

The code: `crates/moqui-core/src/llm/mcp/` (`mod.rs` the codes, conf and identity, `server.rs` the transport, its checks,
`server/discover` and the public documents, `tools.rs` `tools/list` and `tools/call` over the tool registry and the run
model, `state.rs` the `requestState`) and the host adapter `crates/moqui-host/src/llm/mcp.rs`. Tests:
`crates/moqui-core/src/llm/mcp/tests.rs` (the state, the header checks, discover, the tool list, reads, a write through
its approval on the in-memory database) and `crates/moqui-host/tests/it/mcp.rs` (over HTTP on embedded Postgres, the
owner deciding in a browser session, and the official SDK's smoke script, below). The shapes were checked against the
published 2026-07-28 specification (transport, MRTR, discovery, tools, caching, elicitation and the schema reference)
and the `mcp` Python SDK 2.3.0 client.

## Turning it on

The server is off by default; while it is off `/mcp` and its documents answer 404.

```xml
<llm-facade>
    <mcp server-enabled="${mcp_enabled}" protocol-version="2026-07-28" tools-ttl-ms="300000" agent="mcp"
            public-url="${mcp_public_url}" trust-forwarded-headers="false" request-state-key="${mcp_request_state_key}"
            approval-wait="20" max-direct-tools="40"/>
</llm-facade>
```

- `mcp_enabled=true` turns it on. A caller needs the `McpServerUse` and `AgentUse` permissions.
- `agent` names the `AgentDefinition` that bounds the tools: it must serve `mcp` and have `exposeMcp="Y"` (the seed's
  `mcp` agent does). Its `AgentToolPolicy` rows are the server's allowlist and its role group the MCP role group, kept
  apart from the assistant's; what a caller gets is their own authority ∩ that role group ∩ that policy (plan 4.3). The
  agent has no model: nothing is generated on the server.
- A service becomes a tool with an `AgentToolExposure` row whose `exposeMcp="Y"` (System › LLM › Tools); `exposeAssistant`
  is the drawer's, separately. The row's `title` and `description` are what MCP clients show and models read, so an
  admin renames a tool for local terms there; each edit moves `lockVersion` on, which `tools/list` gives as
  `_meta["org.moqui/lockVersion"]`.
- `request-state-key` (standard base64, 32 bytes or more) signs the `requestState` of a write waiting for its approval;
  without it a key derived from `session_cookie_key` (which a cluster has, so every node checks every state), and on a
  single node without either a random key of the process (a waiting write must then be retried before a restart).
- `public-url` is the base URL of the approval links and the documents; without it the request's scheme and `Host`, and
  a believed proxy's forwarded headers only with `trust-forwarded-headers="true"`, as A2A takes them.
- `approval-wait` is how long a retry of a call whose approval is still pending waits for the decision (seconds); a
  write handed off with Edit in screen counts as pending until the user's own save (or its abandonment).
- `tools-ttl-ms` is the freshness hint of `tools/list` and `server/discover`, and how long the server keeps a caller's
  built tool list (what a call runs is checked again as it runs).
- `call-timeout` and `client-enabled` are the MCP client's (13.12).

## Authentication

`Authorization: Bearer <login key>`, the `api_key` or `login_key` header, or HTTP Basic with an app password (or a
login key as the password); never the account password. Credentials are taken only over TLS, a believed proxy's https
or loopback (403 otherwise). Without valid credentials the answer is 401 with
`WWW-Authenticate: Bearer realm="mcp", Basic realm="mcp"` and the JSON-RPC code `-31401`; a user without the
permissions is 403 (`-31403`). A refused login counts for web protection. `server/discover` needs no credentials.

OAuth comes later: Moqui as the OAuth 2.1 authorization server, with general JWT validation (plan 11, answered question
10). The seam is `llm::mcp::www_authenticate`, where a 401 will name its protected resource metadata.

## The transport

Streamable HTTP at `POST /mcp`, one JSON-RPC request per POST, answered with one `application/json` object; there is no
session, no `initialize` and no stream (`GET` and `DELETE` are 405, as the spec has it for a server of this version
only). Before the request is used:

| Check | Refusal |
| --- | --- |
| `Origin`, when sent: this machine (`localhost`, `127.*`, `[::1]`), the configured host (`public-url`, else the webroot's), a believed proxy's forwarded host, or the webroot's `allow-origins`; never the request's own `Host`, which a DNS rebinding page sets to its name | 403 |
| `Content-Type: application/json` | 415 |
| `Accept`, when sent, takes `application/json` | 406 |
| one request object (no batch, no response); a notification is 202 with no body | 400 `-32700`/`-32600` |
| `MCP-Protocol-Version`, `Mcp-Method` and (for `tools/call`) `Mcp-Name` present and equal to the body (`Mcp-Name` may be `=?base64?…?=`) | 400 `-32020` HeaderMismatch |
| `_meta` names `io.modelcontextprotocol/protocolVersion` and `io.modelcontextprotocol/clientCapabilities` | 400 `-32602` |
| a version the server takes (`protocol-version`) | 400 `-32022` with `{supported, requested}` |
| a method it implements (`server/discover`, `tools/list`, `tools/call`) | 404 `-32601` |

Every result has `resultType` and `_meta["io.modelcontextprotocol/serverInfo"]`. The CORS preflight allows the
`MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name` headers; robots.txt disallows `/mcp`.

## Discovery and the tools

`server/discover` answers the supported versions, the capabilities (`{"tools": {}}`) and the connect-time instructions
(`framework/prompt/mcp.md`: the order to discover in, an absent field is null, paging, dates, writes), with `ttlMs` and
`cacheScope: "public"`.

`tools/list` is the caller's, in one page, sorted by name, with `ttlMs` (`tools-ttl-ms`) and `cacheScope: "private"`:

- the read tools `data`, `screen` (when the host has screen sessions), `search` and `skill`, as the registry has them
  for the `mcp` surface, whose one capability is `approval.link` (no `screen.follow`, no `write_ui`, no `ask`);
- each exposed service the MCP agent's policy and role group and the caller's own permissions let run, by its tool name
  made safe for MCP (characters outside `[A-Za-z0-9_.-]` become `_`); its schema is the exposure's override or the
  service's parameters; a write says it waits for approval. Past `max-direct-tools` services, `find_tools`,
  `describe_tools` and `run_tool` are listed instead, as the drawer has them;
- `annotations` (`readOnlyHint`, `destructiveHint`, `openWorldHint`) from the class, and `_meta["org.moqui/classification"]`.

`screen` takes a `session`: leave it out and the call opens a new screen session, whose id the result names (in its
text and as `_meta["org.moqui/session"]`); pass it to go on from that screen. A session is the caller's alone, and one
waiting for an approval is busy until it is decided.

## Calls and approvals

A read (`data`, `search`, `skill`, a READ service) runs as the caller under the read fence, with no run: a service
mislabeled as a read that tries to write fails and changes nothing. Its result is text (`isError` for a tool's error).

A call that may write (a write service, `run_tool`, `screen`) is a run of the `mcp` surface: a conversation (for
`screen`, its session's), one tool call and no model, so it has events and its approval is an `AgentApproval` like any
other, with the card's summary, current and proposed values and Preview effects. A write answers:

```json
{"resultType": "input_required",
 "inputRequests": {"approval": {"method": "elicitation/create",
   "params": {"mode": "url", "url": "https://erp.example.com/qapps?assist=100230",
              "message": "Change an enumeration type: update MCP_TYPE: this waits for your approval in Moqui. …"}}},
 "requestState": "eyJ2IjoxLCJhaWQiOi….3q2-…",
 "_meta": {"org.moqui/approval": {"type": "moqui.approval", "approvalId": "…", "url": "…", "expiresAt": "…",
   "summary": "…", "status": "pending"}}}
```

The link opens the Assist drawer on the call's conversation, which shows the approval card; only its owner, signed in
to Moqui in a browser (or the terminal), may decide it (`Decider::from_request`: a header login, the MCP client's
included, never can). A client that declares no `elicitation.url` capability cannot write: the call is 400 `-32021`
with `requiredCapabilities: {"elicitation": {"url": {}}}`, and nothing is asked, for a write service and for
`run_tool` naming one alike (for `screen`, whose press is known to write only once held, the held write's run is
canceled, which expires its approval).

The `requestState` is `base64url(json{v, aid, sub, cid, tool, dig, exp, n})` `.` `base64url(HMAC-SHA256)`: the approval,
the principal (the user's id), the run, the tool, a digest of the call (`{method, name, arguments}` as RFC 8785
canonical JSON), the approval's expiry and a nonce. On the retry (the same name and arguments, `requestState`, and
`inputResponses`) the MAC is checked first, then the version, the expiry, the principal and the request; any failure is
`-32602` and nothing runs. Then the approval:

- **pending**: the retry waits up to `approval-wait` for the decision and answers `input_required` again with the same
  state and no new elicitation (the SDK retries such an answer at once); `inputResponses.approval.action` `decline` or
  `cancel` is an error result instead, the approval left pending until it expires.
- **approved**: the run goes on here, once: Approved moves to Executing conditionally, the arguments are hashed again
  against the approved ones, the record is compared for a conflict, and the service runs as the caller, unfenced. The
  result is the answer, stored on the run.
- **rejected, expired, a conflict**: an error result saying why (a rejection with the owner's feedback).
- **executed** (the run ended): the stored answer again; nothing runs twice. Another user, or other arguments, with the
  same state are refused.

A decision does not run the write by itself: the run engine never steps an MCP run (one taken over after its node
stopped fails as `mcp_interrupted`), so an approved write the client never retries expires with its run's pause. Each
authenticated request counts against the AT_LLM tarpit before anything is built (`mcp:tools/list`, `mcp:<tool>`
for a call, an unknown name too; 120 a minute by default); a caller over it gets 429 with
`Retry-After` and `-31429`. A retry's wait counts with the user's A2A waits and streams (16 at once).

## The documents

All public, cacheable for an hour, readable from any origin, 404 while the server is off:

- `GET /mcp/server-card`: the Server Card (the MCP Server Card extension, SEP-2127): `name` `org.moqui/moqui`, the
  version, and the one remote, `{type: "streamable-http", url: <base>/mcp, headers: [Authorization], supportedProtocolVersions}`.
- `GET /.well-known/mcp.json`: the same card, at the name the plan used (the card is the published form of that
  manifest).
- `GET /.well-known/ai-catalog.json`: the AI Catalog that names the card, for domain-level discovery.
- `GET /mcp/setup`: the setup prompt for coding agents (Markdown): the endpoint, the auth, the approval flow and a
  client configuration to paste.

## The SDK smoke script

`crates/moqui-host/tests/fixtures/mcp/smoke_mcp_sdk.py` runs the official MCP Python SDK's client against a server: the
connection in mode `auto` (it must find 2026-07-28 by `server/discover`), the tool list, a read, and a write whose URL
elicitation the script answers as the user would, by approving the card of the link's conversation in the owner's
browser session; the SDK's retry with the `requestState` runs it. A client with no URL elicitation is refused the
write. The host test runs it when `MCP_SDK_PYTHON` names a python with the SDK, and says SKIPPED otherwise:

```sh
python3 -m venv /tmp/mcp-venv && /tmp/mcp-venv/bin/pip install 'mcp==2.3.0'
MCP_SDK_PYTHON=/tmp/mcp-venv/bin/python LANG=en_US.UTF-8 cargo test -p moqui-host --test it mcp
```

## The conformance suite

The published suite, `@modelcontextprotocol/conformance` (0.2.0-alpha.12; `list --requirements 2026-07-28` names 37
server scenarios), tests a server by its reference server's own diagnostic tools, resources and prompts for most of
them. The host test runs the scenarios that hold for any server, `server-stateless`, `tools-list`,
`dns-rebinding-protection` and `caching`, through a loopback proxy that adds the Bearer key (the suite sends no
credentials), and passes when every check passes but the ones that need those tools or resources and prompts (31 checks
passed as built). It runs when `MCP_CONFORMANCE` names the suite's CLI, with Node 24 on the PATH:

```sh
mkdir /tmp/conformance && cd /tmp/conformance && npm install @modelcontextprotocol/conformance@0.2.0-alpha.12
MCP_CONFORMANCE=/tmp/conformance/node_modules/.bin/conformance LANG=en_US.UTF-8 \
    cargo test -p moqui-host --test it mcp::the_published_conformance_suite_where_it_applies -- --nocapture
```

## Calling remote MCP servers

The other direction: an agent here calls the tools of remote MCP servers as tools `mcp__<server>__<tool>`. A server is
an `AgentMcpServer` row and each tool an agent may call an `AgentMcpServerTool` row (System > LLM > Remote > MCP
Servers, where List Tools Now shows what the server lists, with Allow for each; `org.moqui.agent.RemoteAgentServices`).
The code is `crates/moqui-core/src/llm/mcp/client.rs` (the client), `crates/moqui-core/src/llm/tools/mcp_client.rs`
(the tools) and `crates/moqui-core/src/llm/remote.rs` (addresses, credentials, SSRF limits, shared with the A2A client).

- **Which tools**: only those a row allows, of servers whose `allowedAgents` names the agent (`*` for all), and only
  when the server lists them. They are in the lazy catalog (`find_tools`, `describe_tools`, `run_tool`); the agent's
  tool policy applies to the names (`mcp__files__*` patterns work).
- **Names** are the server id and the tool name made plain (letters, digits, `-`, `_`; no `__` inside a part), joined
  by `__`. A part that had to change, a name over 64 characters, or one another remote took ends in `_` and a hash of
  the parts as they were, so `get.user` and `get_user`, or two servers' `search`, never collide.
- **Classification**: EXTERNAL unless the row says otherwise, so each call asks for the user's approval (its arguments
  on the card, no sim preview) and runs once approved. An admin may mark a tool READ (or WRITE or DESTRUCTIVE, which a
  policy treats as such).
- **A READ remote tool** runs at once with no approval. It is still an outbound effect, and the fence decides it this
  way: it runs under the read fence with a permit for its own server's origin only
  (`crate::entity::fence::enter_remote`), so its one call goes out while every local write, resource write,
  notification and other remote call stays refused; a sim refuses it as well, and so does a read fence with no permit
  (a screen render, the MCP server's own reads, any other READ tool). READ is the admin's claim about the remote, which
  the fence cannot check on the other side: the model's arguments still leave this server, so mark READ only what the
  remote documents as having no effects. An agent capped at READ keeps its READ remote tools.
- **Untrusted text**: the server's description of a tool (unless the row gives one), every result and every error
  message of the server are wrapped as untrusted (`<untrusted from="mcp:<server>">`, markers elided inside). The input
  schema is rebuilt from an allowlist: properties with plain identifier names (others are dropped and cannot be sent),
  each with only its type, description (markers elided, cut at 200), enum and default values (cleaned, cut at 80, 20
  values at most), a plain format, numeric bounds and nested `items` or `properties`; `describe_tools` shows the
  parameter lines of a remote tool wrapped as untrusted. Labels and approval summaries use the admin's names (the
  allowed tool name, the row's server name), never the title the server chose. Images, audio and resources are named, never fetched; a result past
  `max-result-bytes` is refused.
- **Protocol**: `server/discover` at 2026-07-28 (stateless, `_meta` with the version, the client's capabilities, URL
  elicitations, and its info; the `MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name` headers), else the legacy
  protocol: `initialize` at 2025-11-25, `notifications/initialized`, the `Mcp-Session-Id` on later requests (made again
  once when the server forgets it). Answers may be JSON or event streams. `tools/list` is read page by page (4 MB for
  all the pages) and kept for its `ttlMs` (else `tools-ttl-ms`, and at least 10 s) per server, address and credential; a server that fails is not asked again for a
  minute, and its tools alone are left out of the catalog (logged).
- **MRTR**: an `input_required` with a URL elicitation pauses the local run AuthRequired with the server's link (https,
  or http only to an internal host of a private row; its host on a line of its own) and Done or Cancel; Done goes on
  only to the origin the call waits on (a row moved meanwhile sends nothing); Done retries the call with its `requestState` and the elicitation accepted (the server's own approval
  may still wait: the run waits again with the same link), Cancel ends it. Against a Moqui server this is its approval
  link: the remote's owner approves there, then Done here runs the call once.
- **Addresses and credentials**: as for remote agents in [A2A](A2A.md#delegating-to-remote-agents): admin URLs only,
  https and no internal address unless `allowPrivateNetwork` (and http only to an internal host), internal addresses refused at connect time, no redirects,
  the row's `timeoutSeconds` (else `call-timeout`); `authJson` encrypted and never shown. With `perUserAuth` each user
  calls with their own token (`AgentMcpUserToken.tokenJson`, the same shapes), and a user without one gets no tools of
  that server.
- **Conf**: `<mcp client-enabled="true" call-timeout="60" tools-ttl-ms="300000" max-result-bytes="1000000"/>`.

Tested in `crates/moqui-core/src/llm/tools/remote_tests.rs` against fixture servers on loopback (a 2026-07-28 server, a
legacy one with sessions and event streams, the fence and sim refusals, the SSRF limits) and in
`crates/moqui-host/tests/it/agent_remote.rs` against this server's own MCP server on a second server over loopback
(a READ tool, and a write through both servers' approvals).

## Not yet

- OAuth 2.1 (Moqui as the authorization server, protected resource metadata, JWT validation) and `LlmApiClient` with the
  `mcp` scope.
- `subscriptions/listen` and `listChanged` (the TTL is the freshness hint), resources and prompts, progress
  notifications and SSE answers.
- The legacy `initialize` protocol (2025-11-25 and earlier) for older clients; such a client gets 400 HeaderMismatch or
  UnsupportedProtocolVersion and does not connect.
- A client that cannot show a URL elicitation has no other way to have a write approved.
- In the client: OAuth sign-in for `perUserAuth` (each user's token is set as a row for now), form elicitations, a
  legacy server's requests in its streams (such a call fails), resources and prompts of remote servers.
- The conformance scenarios that need the suite's reference tools (`tools-call-*`, `input-required-result-*`,
  resources, prompts, completion, SSE) do not apply to this server's tools and were not run.
