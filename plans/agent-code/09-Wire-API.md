# 09. Wire protocol: `/agentws`, streams, HTTP routes and the descriptor

Part of [Agent Code](README.md). T3 sources: `t3:packages/contracts/src/{rpc,orchestrationV2,environmentHttp,
environment,auth,clientRpcPermissions}.ts`, `t3:apps/server/src/ws.ts`, `t3:apps/server/src/auth/RpcAuthorization.ts`.

## 1. Why a new socket

- T3 has one RPC socket (`GET /ws`) with 247 methods and about 20 streams.
- MR Moqui has `/notws` (notifications, collab, calc, rtc), `/termws` (terminal) and SSE for Assist events
  (`crates/moqui-host/src/llm/sse.rs`).
- SSE does not work in the apps: the Tauri app scheme answers whole bodies, so it returns 503 for an event stream and
  the Assist drawer polls (`crates/moqui-client-tauri/src/scheme.rs:277`). The apps already hold sockets for pages
  (`crates/moqui-client/src/socket.rs` `PATHS`).
- Agent Code adds `/agentws`: one socket per page for requests, answers and streams. The apps bridge it like `/notws`.
  This fixes the poll for Agent Code on desktop and phone. Assist can move to it later (open decision O11).

## 2. `/agentws`

### 2.1 Route and auth

- Route `GET /agentws` in `crates/moqui-host/src/http.rs`, handler `crates/moqui-code/src/ws/` (registered by the
  native component, as `/termws` is by `moqui-host`).
- Auth, as `/notws`: origin check, then the session cookie, else a `login_key` (device or link key). For a browser on
  another origin (none allowed by default) or for a peer link: a ticket from `POST /rest/s1/code/wsTicket` (5 min, one
  use, names its use `rpc` or `media`), passed as `?ticket=`.
- The gate: the webroot's `endpoint[/agentws]@enabled` (`true`, `false`, or `dev`, the default) and
  `agent-code@enabled` ([14 §1](14-Security.md#1-permissions-and-groups)). It follows
  [Runtime-Config](../Runtime-Config.md#38-services-carried-on-a-listener) §3.8: the route stays mounted and checks
  its switch on each request, from state built from the conf and built again after a reload, so an admin turns
  `/agentws` off or on while the server runs. Off answers 404 and ends the open sockets.
- A DPoP-bound key (`UserLoginKey.dpopThumbprint`) needs a fresh proof in the `dpop` query parameter at upgrade. The
  proof `jti` is spent.
- Query `protocol=<n>` (T3 `orchestrationProtocol`). Current `AGENT_CODE_PROTOCOL` = 1. Header
  `x-moqui-agent-code-protocol` on HTTP. Rules as T3: server newer than client → "client not supported"; server older →
  "needs a newer server", with `serverUpdateRequired` when the server can update itself.
- Query `clientCapabilities=` (comma list, for example `settingsSecretReferences`).
- No execution context is held for the socket. Each request runs in its own context as the socket's user, like a
  REST call.

### 2.2 Frames

JSON text frames. Binary frames for terminal-like sub-streams (voice audio, preview frames).

| From | Frame | Meaning |
|---|---|---|
| client | `{"id":7,"m":"projects.readFile","p":{...}}` | Request. `id` unique per socket. |
| server | `{"id":7,"r":{...}}` | Answer. |
| server | `{"id":7,"e":{"code":"...","message":"...","requiredScope":"code:read","requiredPermission":"..."}}` | Error. |
| client | `{"id":8,"m":"subscribeThread","p":{...},"s":true}` | Open a stream. |
| server | `{"id":8,"i":{...}}` | Stream item. |
| server | `{"id":8,"end":true}` or `{"id":8,"end":true,"e":{...}}` | Stream end. |
| client | `{"id":8,"cancel":true}` | End a stream. |
| server | `{"hello":{"environmentId","protocol","serverVersion","capabilities","user"}}` | First frame. |
| both | `{"ping":n}` / `{"pong":n}` | Each 25 s; a socket silent for 60 s is closed. |
| binary | 4-byte stream id + payload | Voice audio in, preview frames out. |

Limits: 64 open streams per socket; 4 sockets per user per device; request frame at most 1 MiB (uploads go over
HTTP); server queue per socket 8 MiB, above that the socket closes with 1008 "queue full" and the client reconnects.

### 2.3 Method scopes

- Every method declares a required scope and an `AgentCode*` permission (port of `t3:clientRpcPermissions.ts`). A
  middleware checks both before the handler.
- A browser session has every scope its permissions allow. A login key has only its `scope` list.
- Scope list in [14 §2](14-Security.md#2-scopes).

### 2.4 Cluster

- A socket lands on any node. Requests run on that node.
- Streams read the change feed (`AgentCodeChange`) and the run event stream, which every node sees through the
  existing cluster notify (`crates/moqui-host/src/cluster_notify.rs`).
- Work-root actions (files, git, terminal, preview, harness) run on the work root's host node
  (`hostInstanceId`). The receiving node forwards the call there over the cluster channel and relays the answer.
  See [13 §9](13-Servers-Linking.md#9-cluster-and-edge).

### 2.5 Apps

- `crates/moqui-client/src/socket.rs` `PATHS` gains `/agentws`. The host holds it with the device key and bridges it
  to the page, as `/notws`.
- Pushes on `/agentws` for the shell stream are written into the offline store ([12 §6](12-Mobile.md#6-offline)).
- A socket drop marks the shell heads stale. On reconnect the client resumes the shell stream from its last
  `changeSeq`.

## 3. Methods

Each method is a service. The socket calls `ec.service.sync()` on it with the frame's params. Services are in the
component's `service/org/moqui/code/` tree. The names keep T3's groups so a reader can map them.

| Group | Methods | Service file |
|---|---|---|
| orchestration | `orchestration.dispatchCommand` (the one write path for thread commands, [02 §1](02-Orchestration.md#1-the-command-pipeline)), `getTurnDiff`, `getFullThreadDiff`, `searchThreads`, `searchThread`, `searchThreadStream`, `getArchivedShellSnapshot`, `getThreadFamily`, `getThreadTitles`, `getThreadLinkMetadata`, `getWorktreeThreadIds`, `getOlderSettledThreads`, `getThreadProjection`, `getWorkflowScript`, `getNativeChildWork`, `stopWorkflow`, `forceStopThread`, `getTurnItem`, `launchThread` | `CommandServices.xml`, `ThreadServices.xml` |
| projects | `projects.listEntries`, `listIgnoredFiles`, `readFile`, `searchContents`, `searchEntries`, `writeFile`, `mutate`, `ensureScratch`, `createNew`, `createFromThread` | `ProjectServices.xml` |
| shell | `shell.openInEditor` (desktop host opens; the server only checks the path) | `ProjectServices.xml` |
| filesystem | `filesystem.browse`, `getMetadata` | `FileServices.xml` |
| workspace | `workspace.listFolders`, `scanFolders`, `taskFolder`, `addTaskFolderRepository` | `ProjectServices.xml` |
| agentSessions | `agentSessions.scan`, `import` | `ProviderServices.xml` |
| assets, attachments | `assets.createUrl`, `persistChatAttachments`, `attachments.createUploadUrl`, `delete` | `AttachmentServices.xml` |
| mcpApps | `mcpApps.callTool`, `toolInfo`, `readResource`, `updateModelContext` | `McpAppServices.xml` |
| provider | `provider.uploadFeedback`, `mcpSignIn.start`, `mcpSignIn.complete`, `auth.start`, `auth.complete`, `auth.respond`, `auth.cancel`, `auth.removeMethod`, `auth.logout`, `auth.subscribe`, `consumeResetCredit`, `chatgpt.handoff.subscribe`, `chatgpt.reconnect-profile`, `chatgpt.import-profile`, `install.start`, `install.cancel`, `install.subscribe`, `install.remove` | `ProviderServices.xml` |
| vcs, git, review | `vcs.*`, `git.runStackedAction`, `resolvePullRequest`, `preparePullRequestThread`, `review.*` ([04](04-Git.md)) | `VcsServices.xml` |
| terminal | `terminal.open`, `attach`, `observe`, `write`, `resize`, `clear`, `close` (byte traffic stays on `/termws`; these manage sessions) | existing `TerminalServices.xml` |
| preview | `preview.*` ([06](06-Terminal-Preview-Devices.md)) | `PreviewServices.xml` |
| device | `device.*` | `DeviceServices.xml` |
| server config and updates | `server.probe`, `getConfig`, `getRelease` (T3 `getNpmRelease`), `refreshProviders`, `updateProvider`, `updateServer`, `updateServerWithProgress`, `commitDesktopUpdate`, `upsertKeybinding`, `removeKeybinding`, `getSettings`, `updateSettings` | `ServerServices.xml` |
| server storage | `runStorageCleanup`, `getStorageCleanupReport`, `getDatabaseStorage`, `reclaimDatabaseSpace` | `ServerServices.xml` |
| server other | `discoverSourceControl`, `checkSecretSource`, ACP registry (`searchAcpRegistry`, `prepareAcpRegistryAgent`, `uninstallAcpRegistryManagedBinary`, `acceptAcpRegistryUrlAuth`, `listAcpRegistrySessions`, `importAcpRegistrySession`, `deleteAcpRegistrySession`, `listAcpRegistryProviders`, `setAcpRegistryProvider`, `disableAcpRegistryProvider`, `logoutAcpRegistry`), diagnostics (`getTraceDiagnostics`, `getProcessDiagnostics`, `getHostResources`, `getProcessResourceHistory`, `getResourceTelemetryHistory`, `retryResourceTelemetry`, `checkOtlpEndpoint`, `prepareTriage`, `signalProcess`), `listTextGeneratorModels`, `reportClientActivity`, `getUsageSummary`, `refreshUsageRates` | `ServerServices.xml`, `ProviderServices.xml` |
| peerLinks | `peerLinks.list`, `names`, `link`, `unlink`, `answerRequest`, `subscribeThread`, `threadHistoryPage`, `threadItem`, `pairClient`, `pairLink` ([13 §6](13-Servers-Linking.md#6-peer-links)) | `LinkServices.xml` |
| scheduledTasks | `scheduledTasks.*` ([05 §6](05-Automation.md#6-services-rpcs-and-tools)) | `ScheduleServices.xml` |
| secrets, guestShare | `secrets.answerRequest`, `guestShare.create` | `SecretServices.xml`, `GuestServices.xml` |
| cloud → edge | T3 `cloud.getRelayClientStatus`, `installRelayClient` → `edge.status`, `edge.setup` ([13 §8](13-Servers-Linking.md#8-relay-and-tailscale)) | `LinkServices.xml` |
| pullRequests | read: `list`, `listStats`, `filterOptions`, `summary`, `routing`, `routingIdentity`, `stack`, `linkedThreads`, `detail`, `preview`, `checks`, `activity`, `reviewGuide`, `threadComments`, `diffFileContents`, `filesViewed`, `changeBreakdown`, `reviewerCandidates`, `labelCandidates`; write: `generateReviewGuide`, `setFilesViewed`, `runAction`, `update`, `comment`, `updateComment`, `submitReview`, `replyToThread`, `setThreadResolution`, `setReaction`, `invalidate`, `reportState`, `requestReviewers`, `setLabels` | `PullRequestServices.xml` |
| issues, issueTrackers | `issues.*`, `issueTrackers.list`, `connect`, `bindProject` | `IssueServices.xml` |
| sourceControl, projectClone, worktreeSetup | `sourceControl.*`, `projectClone.start`, `cancel`, `retry`, `worktreeSetup.cancel` | `VcsServices.xml` |
| access (new name for T3 auth HTTP) | `access.clients`, `access.revokeClient`, `access.revokeOthers`, `access.createPairing`, `access.revokePairing`, `access.approveDevice`, `access.revokeDevice` | `AccessServices.xml` |

## 4. Streams

| Stream | Content | Source |
|---|---|---|
| `subscribeShell` | Projects, thread summaries, unread, attention; cursor `changeSeq`; omits idle subagent children and keeps the 25 newest settled top-level threads (`olderSettled` counts the rest) | `AgentCodeChange` |
| `subscribeThread` | One thread's projection updates | run event stream + change feed |
| `subscribeCommandOutput` | Command output, modes replace and append | run spill files |
| `subscribePromptSuggestion` | Next prompt suggestion | text generation |
| `subscribeProjectClones` | Clone progress | effect worker |
| `subscribeVcsStatus`, `subscribeVcsBranch` | Working tree and branch state per checkout | file watcher on the work root host |
| `subscribeWorktreeSetup` | Setup script progress | `AgentCodeLaunch` |
| `subscribeTerminalMetadata` | Terminal list, titles, running state | `TerminalSession` |
| `subscribePreviewEvents`, `subscribeDiscoveredLocalServers` | Preview tabs; local servers | preview broker |
| `subscribeDeviceState` | Simulator and emulator state | device host |
| `subscribeServerConfig` | Server config and provider status | settings, provider instances |
| `subscribeServerLifecycle` | start, ready, update, shutdown | node lifecycle |
| `subscribeAuthAccess` | Client sessions and pairing changes | `UserLoginKey`, `ClientPairingCode` |
| `subscribeResourceTelemetry` | CPU and memory samples | existing metrics |
| `provider.auth.subscribe`, `provider.install.subscribe`, `provider.chatgpt.handoff.subscribe` | Provider sign-in and install progress | process host |
| `scheduledTasks.subscribe` | Task list | change feed |
| `peerLinks.subscribeThread` | A thread from a linked environment, read-only | peer relay |
| `pullRequests.subscribeRefreshes`, `subscribeLinkedThreadRefreshes` | PR invalidation | PR watch and sync |
| `voice` (binary) | Audio in, partial text out | voice engine |
| `preview.frames` (binary) | Screencast frames out, input in | preview broker |

Rules from T3, kept:

- A thread stream is shared by all mounted views of that thread on one page. The client cache is kept 5 idle minutes.
  The desktop keeps every thread whose run is starting or running alive.
- Mutations are never replayed on reconnect. The command receipt makes a retry with the same `commandId` safe.
- The shell loads first over HTTP (§5 `/rest/s1/code/shell`) before the socket is ready, so the first paint does not
  wait for the socket.

## 5. HTTP routes

| T3 route | MR Moqui route | Notes |
|---|---|---|
| `GET /.well-known/t3/environment` | `GET /.well-known/moqui/agent-code` | §6. No auth. |
| `GET /api/auth/session` | `GET /rest/s1/code/session` | Current user, scopes, environment. |
| `POST /api/auth/browser-session` | `POST /rest/s1/code/browserSession` | Exchange a pairing token for a browser session cookie. |
| `POST /oauth/token` (RFC 8693) | `POST /oauth/token` | Token exchange for pairing tokens and DPoP keys ([13 §3](13-Servers-Linking.md#3-pairing-and-sessions)). |
| `POST /api/auth/websocket-ticket[/:use]` | `POST /rest/s1/code/wsTicket` | 5 min, `rpc` or `media`. |
| `POST /api/auth/pairing-token`, `pairing-links/revoke`, `clients/revoke`, `clients/revoke-others`, `connect-devices/approve`, `/revoke` | `access.*` methods and `/rest/s1/code/access/*` | |
| MCP OAuth (`/.well-known/oauth-*`, `/oauth/mcp/*`) | Same paths | [08 §1.4](08-MCP-Tools.md#14-outside-agents-oauth). |
| `GET /api/orchestration/shell`, `/threads/:threadId`, `.../transcript`, `.../bounded`, `.../history` | `GET /rest/s1/code/shell`, `/rest/s1/code/threads/{id}`, `.../transcript`, `.../bounded`, `.../history` | Gzip. ETag on `changeSeq`. |
| `GET /api/projects`, `POST /api/projects/mutate` | `/rest/s1/code/projects`, `/rest/s1/code/projects/mutate` | |
| `POST /api/pull-requests/diff` | `/rest/s1/code/pullRequests/diff` | Large diffs. |
| `/api/voice/*` | `/rest/s1/code/voice/*` | [07 §4](07-Secrets-Guest-Voice-Usage.md#4-voice-input). |
| `/api/connect/*`, `/api/t3-connect/*` | `/rest/s1/code/edge/*` | Edge-node replacements ([13 §8](13-Servers-Linking.md#8-relay-and-tailscale)). |
| `POST /api/peer-link/client-pairing` | `POST /rest/s1/code/links/clientPairing` | Link session only. |
| `/api/hooks/:hookId/:token` | `/hooks/{hookId}/{token}` | [05 §5](05-Automation.md#5-webhooks). |
| `GET /ws` | `GET /agentws` | §2. |
| `GET /ws/voice` | `/agentws` stream `voice` | |
| `POST /api/observability/v1/traces` | existing `/v1/traces` | Browser OTLP. |
| `GET /api/assets/*` | `GET /code/assets/{ticket}` | Signed URL for one file and its device and inode. |
| `POST /api/attachments/upload/*` | `POST /code/upload/{ticket}` | Upload targets; 25 MB per file default. |
| `GET /api/preview/tunnel`, `/api/preview-stream` | `/preview/tunnel`, `/agentws` stream `preview.frames` | [06 §2](06-Terminal-Preview-Devices.md#2-browser-preview-and-automation). |
| `GET /api/links/thread/stream`, `/history`, `/item` | `/rest/s1/code/links/thread/stream` (as an `/agentws` stream for the link), `/history`, `/item` | [13 §6](13-Servers-Linking.md#6-peer-links). |
| `* /api/device-hub/*` | `/device-hub/*` | Authenticated proxy. |
| `GET /callback` | `GET /code/cli-callback` | CLI sign-in callback on loopback. |
| `/api/guest/*` | `/guest/api/*` | [07 §3](07-Secrets-Guest-Voice-Usage.md#3-guest-share). |
| `/mcp` | existing `/mcp` | |
| `GET *` static web app | `/qapps/code/...` screens | [10](10-Web-UI.md). |

Every new route is added to `docs/security/Attack-Surface-Routes.md` with its auth, origin rule and limits
([14 §8](14-Security.md#8-attack-surface-docs)).

## 6. Environment descriptor

`GET /.well-known/moqui/agent-code` (no auth, no secret, cacheable 60 s):

```json
{
  "environmentId": "env_…",
  "label": "build box",
  "platform": {"os": "linux", "arch": "x64", "machine": "server"},
  "serverVersion": "<moqui version>",
  "protocol": 1,
  "databaseMigration": null,
  "capabilities": {"pullRequests": true, "...": true}
}
```

- `environmentId`: [13 §1](13-Servers-Linking.md#1-environment).
- `label`: the `AgentCodeSetting` row `environment.label`, else the host name.
- Capabilities: the T3 list ([t3:environment.ts]) is kept by name, so a T3-trained client reads them. Agent Code
  answers true only for what it has built. A capability added in a later phase starts false.
- Agent Code extras: `moquiNativeEngine`, `moquiHosting`, `edgeLink`, `clusterNodes`.
- `serverSelfUpdate`: `boot-service` when installed as a service ([13 §10](13-Servers-Linking.md#10-background-service-and-updates)),
  `desktop-managed` in the desktop in-process node, else absent.
- `serverInstallation`: `{kind: "moqui-package"}`, `{kind: "moqui-launcher", launcher, baseDir}` (SSH launch) or
  `{kind: "desktop"}`.
- The descriptor answers 404 when Agent Code is off on the instance.

## 7. Versioning

- `AGENT_CODE_PROTOCOL` goes up only on a breaking change. Methods and fields are added without a bump. Clients
  check `capabilities` first.
- A remote server can be older than the client. The client hides features whose capability is absent.
- The typegen d.ts (`crates/moqui-devkit/src/typegen/moqui-host.d.ts`) gets the `/agentws` client API for `.qvue`
  code.
