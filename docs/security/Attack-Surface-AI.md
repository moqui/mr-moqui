# Attack surface: LLM, A2A, MCP and Assist

The model facade, A2A, MCP, the OpenAI-compatible gateway, Assist and the agent permissions. This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## LLM, A2A, MCP and Assist

Routes exist when the `llm` feature is compiled. While a protocol is off, its handlers answer 404. `llm-facade` `enabled` is `true`. That element is the model facade. It is not the A2A server, the MCP server, or the gateway. Assist answers 503 when the facade is disabled; `bootstrap` still answers (`docs/ai/Assist.md`).

A user with `EXTERNAL_USER` never holds `A2aServerUse` or `AgentUse`. The Assist contract says that permission also blocks the other agent permissions, so that user starts no run and gets no model call.

### A2A

Off. `a2a_enabled` is `false`, and `trust-forwarded-headers` is `false`. While it is off, `/a2a` and `GET /.well-known/agent-card.json` answer 404.

From `docs/ai/A2A.md`: a caller needs `A2aServerUse` and `AgentUse`. Credentials are a Bearer login key, an `api_key` or `login_key` header, or Basic with an app password or a login key as the password. The account password is refused (401). Credentials are taken only over TLS, a believed proxy's https, or loopback. Missing credentials are 401 `WWW-Authenticate: Bearer realm="a2a", Basic realm="a2a"`. No permission is 403. The body max is 4 MiB. Waits and streams are at most 256 on a node (503) and 16 per user (429). Every request except the public card needs `A2A-Version: 1.0`. OIDC is not built. An agent is reached only when `exposeA2a="Y"` and the surface includes `a2a`. The seed `assist` agent does not. Writes wait for the human owner.

### MCP

Off. `mcp_enabled` is `false`, and `trust-forwarded-headers` is `false`. While it is off, `/mcp` and its documents answer 404.

From `docs/ai/MCP.md`: a caller needs `McpServerUse` and `AgentUse`. Credential shapes match A2A, with realm `mcp`. No credentials is 401 JSON-RPC `-31401`. No permission is 403 `-31403`. `server/discover` needs no credentials. The body max is 4 MiB. The transport is `POST /mcp` only; GET and DELETE are 405. The seed agent `mcp` serves `mcp` with `exposeMcp="Y"`. The MCP client cannot approve a write. Public when on, and cached for an hour: `GET /mcp/server-card`, `/mcp/setup`, `/.well-known/mcp.json`, `/.well-known/ai-catalog.json`.

### Gateway

Off. `llm_gateway_enabled` is `false`. The live element is `<gateway enabled="${llm_gateway_enabled}" require-client="true" timeout="300"/>`. An `enabled="true"` example above it in the conf is inside a comment.

From `docs/ai/Gateway.md`: Bearer login key or app password, or Basic with those. The account password is refused. TLS, a believed proxy's https, or loopback. Missing or bad credentials are 401 `invalid_api_key`. The caller needs `LlmGatewayUse` and an `LlmApiClient` row. An app password always needs a client. An account's login key needs one because `require-client` defaults to true; a key made for the gateway alone (`create#UserScopedKey`, `UserLoginKey.scope` `gateway`) is its own client, since it opens nothing else and its user's permission and quota still apply. The body max is 16 MiB. The path is exempt from the webapp request deadline. The gateway `timeout` of 300 seconds answers 504. Paths: `GET /models`, `GET /models/{alias}`, `POST /chat/completions`, `POST /responses`, `POST /embeddings`. An unknown alias is 404. No tools run on the server: function-tool definitions pass through, and any other tool type is 400 `unsupported_tool`.

### Assist

The drawer is screen transitions on `assist.xml` (`require-authentication="false"`), not host routes. Each transition calls the Assist service. GET reads: `bootstrap`, `conversations`, `conversation`, `tasks`, `prompts`, `mentions`, `attachment`, `memories`, `skills`, `events`, `read`. POST: `turn`, `cancel`, `retry`, `follow`, `context`, `submitted`, `decide`, `preview`, `answer`, `feedback`, `queued`, `attachments`, `transcribe`, `textAction`, `memory`, `skill`, `canvasState`, `canvasAction`, `canvasAdjust`, `canvasProblems`.

`docs/ai/Assist.md` is the contract for those calls. The browser session cookie is the login. Every call acts for the current user and needs `AgentUse`. Another user's conversation, run, approval, question, hand-off, or message is 404. A call from inside a run is 403. POST requires `X-CSRF-Token` or `moquiSessionToken`, and a missing token is 401 `Session token is missing or is not valid` before the call. The qapps shell includes the drawer when the llm feature is on and the user has `AgentUse`. Per-call tool policy inside the Rust services was not read.

**Dev Assist's `hosting` tool** (the dev-aide crate, `src/agent/hosting.rs`; plans/design-dev-aide.md 21.8) reads a working tree's hosted repository on a connected group server as the connected user: merge requests, review threads, checks, a job's log and failed tests, an issue. Every text it returns is other people's (titles, descriptions, comments, logs, test messages): it reaches the model inside `<untrusted from="group-server">` with the wrapper's markers elided (`moqui_llm::prompt::untrusted`), so a comment that tells the agent to approve or push is data. It has no action that approves, merges, re-runs, resolves a thread, signs the contributor agreement or pushes. It gives an agent no diff (files, file texts, commits) of a merge request whose commits' authors have not all signed where the server requires it (`get#AuthorSigned` per author email, or the application's `diffRefused`), nor when the authors are not all known: no commit list, a cut one (`commitsTruncated`, fewer commits than `aheadBy`), a commit with no author email, more than 20 authors, or, for a repository an application keeps, an author the server did not decide (`known` false, which is also its answer when its agreement hook fails); the threads of such a merge request come without their line's text and without fenced or quoted code (a suggestion). A worktree of an agent's session is its maker's: it is made only for one of the caller's own conversations, only that user's session is rooted in it, only they or an administrator remove it (and only under `runtime/worktrees`), and a session with a worktree reaches neither another worktree nor a root inside the tree its worktree stands in for. Its reads run under the read fence: the tool opens a remote permit for the hosted server's origin only (`fence::enter_remote`), and the Dev AIDE's client lifts the fence for that one GET after `check_remote` passes (never in a sim, never for another origin, never for a write); a read that would write (resolving a root's hosted repository) is refused under the fence instead. A comment, a reply or a new merge request is an EXTERNAL approval whose card shows the text, the merge request, file and line, the repository and the server; the approved write goes through the same HostingServices as the Workspace's, so a header login, a fence and a sim refuse it.

`GET /assist/runs/{run_id}/events` and `.../events.json` authenticate like `/notws` (origin, session cookie, else `login_key`). The stream is for the run's owner. At most 256 streams; one more is 503 with `Retry-After` (`docs/ai/Assist.md`).

`GET /assist/sandbox` is public and has no session. The host must be a plain name, or the answer is 400. Query `v` is a cache buster. `x=1` adds `https:` to `img-src` only. The iframe is `sandbox="allow-scripts"` without `allow-same-origin` (`docs/ai/Assist.md`). The policy is the frame document, separate from the screen CSP:

```
sandbox allow-scripts; default-src 'none';
script-src <origin>/libs/vue/ <origin>/libs/quasar/ <origin>/js/WebrootAssistSandbox.js 'unsafe-eval';
style-src <origin>/libs/ 'unsafe-inline'; font-src <origin>/libs/;
img-src data: <origin>/images/ <origin>/libs/;
connect-src 'none'; media-src 'none'; object-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none';
manifest-src 'none'; form-action 'none'; frame-ancestors 'self' <origin>; base-uri 'none'
```

Response headers from that doc: `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`, a `Permissions-Policy` that turns features off, and no `X-Frame-Options`. `'unsafe-eval'` is in this frame policy. It is not in the default screen CSP.

Mantle's `user-facts` provider (`mantle.party.UserFactServices.get#OrganizationFacts`, named in Mantle's MoquiConf `llm-facade`) adds to a new conversation's prompt the names and ids of the user's internal organizations (at most twenty) and their active one; it reads only, answers nothing for another user, and is not remote. A tool still takes and checks its organization: the facts are fixed when the conversation starts.

## Agent permissions

`AgentTypeData.xml` defines `AgentUse`, `AgentAdmin`, `AgentAudit`, `LlmGatewayUse`, `A2aServerUse`, `McpServerUse`, and `AgentSkillAuthor`. The `ADMIN` grant rows in that file are `AgentUse`, `AgentAdmin`, and `AgentSkillAuthor`. `A2aServerUse`, `McpServerUse`, and `LlmGatewayUse` are not among those rows. `AgentAudit` is not among them either. `SecurityTypeData.xml` grants `SQL_RUNNER_WEB`, `SSH_SHELL`, `TERMINAL_HOST`, `LOAD_RUNNER`, and `ADMIN_LOGIN_AS` to `ADMIN_ADV`, and `ADMIN_PASSWORD` to `ADMIN`. `GROOVY_SHELL_WEB` is seeded on no group. No screen uses it; it is the "may run scripts" grant that an ETL mapping's `expression` fields need ([Database](../tools/Database.md#etl)).

## Open questions

These are gaps in this pass. A line that was read is stated above and is not repeated here.

- Assist authorization inside each Rust service body was not read. `docs/ai/Assist.md` is the contract cited above (cookie session, `AgentUse`, 404 for another user's object, 403 from inside a run, CSRF on POST).
