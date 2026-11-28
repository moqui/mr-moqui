# 13. Servers, linking and remote access

Part of [Agent Code](README.md). T3 sources: `t3:apps/server/src/{bin,binCli,config,serviceLauncher}.ts`,
`t3:apps/server/src/cli/*.ts`, `t3:apps/server/src/{auth,peer,cloud,relay,environment}/**`, `t3:packages/{ssh,tailscale}`,
`t3:packages/client-runtime/src/connection/**`, `t3:docs/internals/{remote,environment-auth,server-updates,t3-connect}.md`,
`t3:docs/user/{remote-access,background-service,install,updating}.md`. MR Moqui sources:
[plans/Edge-Node2.md](../Edge-Node2.md), [plans/archive/Vault-Instances.md](../archive/Vault-Instances.md),
[docs/ui/Desktop-Client.md](../../docs/ui/Desktop-Client.md), `crates/moqui-host/src/{edge_link,ssh,cluster_edge}`.

Boundary rule (kept from T3): work runs in the environment that owns the workspace. A client never puts its own files,
provider credentials or machine state in place of the server's.

## 1. Environment

| T3 | MR Moqui |
|---|---|
| One user per environment | One MR Moqui instance (a single server or a cluster) is one environment with many users. Each user sees the projects they are members of ([§5](#5-users-on-one-environment)). |
| `environment-id` file, atomic, recovery file | `runtime/db/agent_code_environment_id`, written once with a temp file and rename. A cluster shares one id (the first node writes it to the `AgentCodeSetting` row `environment.id`; the file is a cache). |
| `label` | The `AgentCodeSetting` row `environment.label`; default the host name. |
| `platform {os, arch, machine}` | From the host. `machine` from the conf setting `agent-code@machine-kind` (empty: found from the host; server, cloud, desktop, laptop, ...). Desktop nodes report `desktop` or `laptop`. |
| `serverVersion`, `orchestrationProtocolVersion` | `moquiVersion`, `AGENT_CODE_PROTOCOL` ([09 §7](09-Wire-API.md#7-versioning)). |
| `capabilities` (about 90 booleans) | Same names where the feature exists, in the descriptor ([09 §6](09-Wire-API.md#6-environment-descriptor)). Clients check them because servers can be older. |
| `RepositoryIdentity` (canonical key follows `upstream`) | `AgentProject.repositoryKey` (same rule). It groups clones across environments, never routes work. |
| Advertised endpoints | New descriptor field `endpoints[]`: `{kind: lan, tailnet, vpn, public, edge, url}`. Filled from the conf setting `agent-code@advertise` (default `auto`: LAN interfaces when the HTTP port is open to them, Tailscale name when found, the web address, the edge host). |

Descriptor and well-known routes:

| Route | Auth | Content |
|---|---|---|
| `GET /.well-known/moqui/agent-code` | none | Descriptor (no user data). |
| `GET /.well-known/oauth-authorization-server` | none | OAuth metadata for MCP clients and pairing token exchange ([08 §1](08-MCP-Tools.md#1-transport-auth-and-injection)). |
| `GET /.well-known/oauth-protected-resource`, `.../mcp` | none | Resource metadata. |

## 2. Server process and deployment

| T3 | MR Moqui |
|---|---|
| `t3` / `t3 start` (open the browser) | `moqui` (existing) prints the URL; `moqui code open` opens `/qapps/code`. |
| `t3 serve` (headless, print pairing) | `moqui serve` (existing). When Agent Code is on (the `/agentws` endpoint, [14 §1](14-Security.md#1-permissions-and-groups)) it prints a pairing QR at start when no user has a device yet and the console is a TTY. |
| `--host`, `--port`, `--lan` | Existing listener conf (`webapp` host and port). `--lan` = bind `0.0.0.0` with `webapp-list/webapp[webroot]@private-network-only="true"` (new framework setting, default false, in `MoquiDefaultConf.xml` with its XSD attribute and conf schema entry): close a connection unless both ends are loopback, private, or tailnet addresses. |
| `--base-dir` (`~/.t3`) | `runtime/` (existing). Agent Code data under `agent-code@home` ([02 §2](02-Orchestration.md#2-thread-creation-and-launch)). |
| `server-runtime.json` (PID, origin, port) | `runtime/db/server-runtime.json` (new): pid, origin, port, environment id, start time. The SSH launch and the CLI read it. |
| Refuse a second interactive server on the same base dir | Existing lock on the runtime directory. The message names `moqui pair`. |
| HTTP only; HTTPS from Tailscale Serve, the relay, or a proxy | MR Moqui has TLS (existing ACME and certificate conf). Tailscale Serve and the edge node are extra ways (§8). |
| Dev auth token `T3CODE_DEV_AUTH_TOKEN` | Not ported. Dev instances use the normal sign-in. |
| Traces NDJSON, OTLP | Existing tracing and OTLP export. Span names `code.rpc.<method>`, `code.mcp.<tool>`, `code.effect.<type>`. |

Install on a remote machine (T3 `install.sh`): MR Moqui keeps its own install path (release archive, container
image, `./moq` for source). The SSH launch (§7) can install a release archive into `~/.moqui/runtime/<version>`.

## 3. Pairing and sessions

### 3.1 Auth policies

| T3 `ServerAuthPolicy` | MR Moqui |
|---|---|
| `desktop-managed-local` | In-process desktop node: the host holds the node session ([11 §1](11-Desktop.md#1-modes)). |
| `loopback-browser` | Normal sign-in. No "no password on loopback" mode. |
| `remote-reachable` | Normal sign-in, plus TLS required for login keys over a non-private address (existing `webapp` rules). |
| `unsafe-no-auth` | Not ported. |

### 3.2 Pairing codes and links

- `ClientPairingCode` ([01 §10](01-Data-Mapping.md#10-new-entities)): hash only, kinds `CpcDevice` (a new client
  device), `CpcLink` (a peer link), `CpcApproval` (device approval code).
- Code: 12 characters of `23456789ABCDEFGHJKLMNPQRSTUVWXYZ`. Default life 5 minutes for `CpcDevice`, 10 minutes for
  `CpcLink`. One use. 5 wrong tries end the code.
- Link: `https://<server>/qapps/code/pair#token=<code>`; app link `org.moqui.mobile:/code/pair#token=...`; desktop
  `moqui://code/pair#token=...`. The page reads the fragment, strips it from history, and posts it.
- QR content: the link plus the advertised endpoints (`&e=` list, base64url JSON) so a phone can try LAN and Tailscale
  first.
- Who can make a code: the user for their own devices (`AgentCodeUse`); for another user or a link
  `AgentCodeAdmin`. A code made by a user can carry only scopes that user has ([14 §2](14-Security.md#2-scopes)).
- Make one: Settings > Access **Pair a device**, the Welcome page, `moqui pair`, `moqui code pairing create
  [--ttl 5m] [--label] [--scope ...] [--token-only] [--json]`.

### 3.3 Exchange and keys

- `POST /oauth/token` (RFC 8693), form fields as T3: `grant_type=urn:ietf:params:oauth:grant-type:token-exchange`,
  `subject_token` (the pairing code), `subject_token_type`, `scope?`, `client_label?`, `client_device_type?`
  (desktop, mobile, tablet, bot, unknown), `client_os?`.
- The answer is a `UserLoginKey` for the code's user with `deviceName` = label and `scope` = the granted scopes
  (narrowed by `scope?`, never widened; unknown names dropped; no scope left = refused before the code is spent).
  This is T3's shape; master keeps one scope per login key, so how the granted scopes are stored is designed with P6
  on that model ([14 §2](14-Security.md#2-scopes), "Key scopes on master").
- The apps then use the key as they do today (`login_key` header, renewal every six hours, server ends the old key
  ten minutes later). So pairing is one more way to get the same device key that a password sign-in gets.
- DPoP (optional, T3 mobile uses it): the client sends a `DPoP` proof at exchange; the key gets
  `UserLoginKey.dpopThumbprint`; every later request needs a proof from that key (`htm`, `htu`, `iat` within 60 s,
  one-use `jti`). Desktop and Android keep the DPoP key in the OS key store.
- Sessions list and revoke: existing `/qapps/clientDevices` and `revokeClientDevice`, plus **Revoke all others**.

### 3.4 Device approval

- Server setting `requireDeviceApproval` (default false; T3 `requireConnectDeviceApproval`).
- When on, a new device key starts as `pending`: every request fails with `approval_pending` and the code
  `K7QM-2XFD` form (derived from the key thumbprint, stable). Request life 10 minutes.
- A signed-in device or an admin approves in Settings > Access or `moqui code device approve <code>`.
- Revoking a key revokes its sessions and its unspent codes.

### 3.5 Socket tickets

- `POST /rest/s1/code/wsTicket` with `use=rpc|media`: 5 minutes, one use. A DPoP key's ticket is bound to the proof
  key. Browsers use the cookie.

## 4. Client connection runtime

The port of `t3:packages/client-runtime/src/connection` in two places: `crates/moqui-client/src/code/` for the apps,
and `AgentCode.js` (`moqui.code.connection`) for the browser and for page-side state.

| T3 | MR Moqui |
|---|---|
| Targets: Primary (same origin), Bearer, Relay, Ssh | `primary` (page origin), `account` (app account, device key), `ssh` (desktop), `link` (through the server's peer link). The relay target is the edge node, a normal `account` route. |
| Route kinds `relay`, `loopback`, `lan`, `tailnet`, `vpn`, `public`, `ssh` | Same, with `edge` for `relay`. |
| Driver checks each direct route with the public descriptor before it sends a credential | Same. A route that answers with another environment id gets no key. |
| Supervisor: backoff to 5 min; preflight earlier routes (network change, foreground, every minute); cooldown; "Sign-in refused" | Same ([12 §2.3](12-Mobile.md#23-supervisor)). |
| Learned routes; never learn plain HTTP off the tailnet for an HTTPS-paired key | Same. |
| Probe deadlines 500 ms, 3 s, 15 s; replace a silent socket at once on LAN, after 2.5 s elsewhere | Same. |
| Shell snapshot over HTTP before the socket is ready | `GET /rest/s1/code/shell` first, then `subscribeShell` from its `changeSeq`. |
| `shellOlderSettled` keeps 25 newest settled threads | Same limit, with `olderSettled` count. |
| One live stream per thread; idle cache 5 min; desktop keeps running threads alive | Same. |
| Mutations never replayed on reconnect | Same for the socket. The outbox replays only by `commandId` receipt ([12 §6](12-Mobile.md#6-offline)). |
| States and reasons (`network`, `timeout`, ..., `approval-pending`) | Same names in `moqui.code.connection.state`, shown by the environment pill. |

## 5. Users on one environment

T3 is one user per environment. MR Moqui servers have many users. This section is new.

- A project has members (`AgentProjectMember`: owner, member, viewer). A user sees threads of projects they are a
  member of, and their own threads with no project.
- A thread belongs to its creator. Members can read it. Writing to another user's thread (send, approve, settle)
  needs `AgentCodeAdmin` or the project owner role. Approvals are always the thread owner's unless delegated in the
  project.
- Providers: an instance is environment-wide (admin sets it up), each user signs in with their own provider
  account (`AgentProviderAccount`, own home directory per user, [03 §1](03-Providers.md#1-drivers-and-instances)).
  A shared provider account per instance is allowed only when the admin marks it so.
- Work roots, terminals and harness processes run as the server's OS user, or as `agent-code@run-as` when it is set
  ([Terminal-Fancy2](../Terminal-Fancy2.md) TF6). Isolation between users is the process sandbox
  ([14 §5](14-Security.md#5-process-isolation)).
- Settings: client settings per user; server settings per environment (admin).
- Peer links are environment-wide, made by an admin; each link has an access level and a list of users who may use
  it (default: the user who made it).

## 6. Peer links

| T3 | MR Moqui |
|---|---|
| The linking environment signs in to the other's `/mcp` as an MCP client with a pairing code; the token goes to its secret store | Same. The receiving side sees one more `AgentMcpOAuthClient` (software id `moqui-code-peer-link`); its `AgentEnvironmentLink` row has direction `inbound`. The linking side stores the key as a secret source (`credentialRef`) in an `outbound` row. |
| `PeerLink` record and status | `AgentEnvironmentLink` ([01 §10](01-Data-Mapping.md#10-new-entities)). |
| Create input `url`, `alternateUrls`, `pairingCode`, `access`, `expectedEnvironmentId`, `expectedLabel`; failures `unreachable`, `code_refused`, `wrong_environment`, `other` | Same, service `create#EnvironmentLink`. |
| Gate `linkedEnvironmentsEnabled` (both sides) | Same setting (default false). Off on the linking side: link, link requests and forwarding refuse; read, wait and interrupt still work. Off on the receiving side: registration refused; link sessions get 403 (retryable). |
| Tool forwarding with `environmentId` target | Same for `code_*` tools ([08 §2](08-MCP-Tools.md#2-tool-list)). A tool that acts as the calling thread cannot target a link. |
| `T3-Mode-Limit` header; receiver only narrows; `T3-Link-Expires-At` | `Moqui-Code-Mode-Limit`, `Moqui-Code-Link-Expires`. The T3 names are read too, for T3 environments. |
| One session per link; idempotency keys hashed with the caller namespace; waits split into calls of 50 s | Same. |
| Linked thread read: `/api/links/thread/stream`, `/history`, `/item` | `/rest/s1/code/links/thread/{stream,history,item}` ([09 §5](09-Wire-API.md#5-http-routes)); stream as an `/agentws` stream on the receiver; read-only relay. |
| Fencing by `linkOrigin` for links below full access | `AgentConversation.linkId` set on threads a link session launches (and their subagents, forks, created threads). Such work changes only threads with the same link; no projects, settings, automations; no use of the receiver's own links; project setup scripts skipped. Reads not fenced. Full access is not fenced, still stamped ("From <machine>"). |
| Remote delegated task (`remoteChild`) | `AgentDelegatedTask.linkId` and `remoteConversationId`; a follower service waits and completes it; re-followed at start. |
| Client pairing through a full-access link | `peerLinks.pairClient` behind `access:write`; the receiver makes a `CpcDevice` code for a client of the linking side. |
| Address trust: token only to an address that reports the linked id; agent-given addresses need a pasted code; loopback never counts | Same. |
| UI: Settings > Environments map (drag to link, link several, link back, stacks, search) | Settings > Connections ([10 §9.4](10-Web-UI.md#94-settings-pages)). |
| CLI `t3 environment link/list/unlink`, `--from <file>` bulk | `moqui code link <address> --code <code> --access <mode> [--alt <address>]... [--from file\|-]`, `list`, `unlink <id>`. |
| Interop with T3 environments | A link to a T3 Code environment works: it is a normal MCP client with T3 tool names. A T3 environment can link to MR Moqui: the `t3_*` aliases answer ([08 §1](08-MCP-Tools.md#1-transport-auth-and-injection)). |

Linked work over an edge node: a link URL can be the other environment's edge host. `viaEdgeNodeId` records it.
Forwarded calls of 50 s fit the edge idle limits.

## 7. SSH launch

Start or reuse an MR Moqui server on a remote host over SSH, as T3 does for `t3 serve`. Desktop side in
[11 §8.1](11-Desktop.md#81-ssh-launch-desktop-side). This section is the remote side.

### 7.1 Launch script

`moqui-desktop` pipes a POSIX `sh` script on stdin of `ssh host 'exec $SHELL -l -c "exec sh -s -- <stateKey> <launcherId>"'`
(login shell, so provider CLIs on the user's PATH are found). The script:

1. Takes a lock `~/.moqui/ssh-launch/<stateKey>/lock` (wait max 360 s; `mkdir` lock with a PID file).
2. Finds `moqui`: on the PATH, or `~/.moqui/runtime/<version>/moqui`. When absent, downloads the release archive for
   the host's OS and arch (Linux x86_64, aarch64; macOS arm64) from the release URL, checks SHA-256 (download 240 s,
   checksum 30 s). When the host cannot download, the script prints the install command and stops.
3. Finds or makes a runtime directory `~/.moqui/ssh-launch/<stateKey>/runtime` (SQLite or the default embedded
   database, `instance@purpose` dev unless set).
4. Reads `server-runtime.json`. If a server with that runtime runs and answers `/health`, reuses it
   (`serverKind: external` when this launcher did not start it).
5. Else picks a free port from 3773 within 200 ports (`moqui __ssh-helper pick-port`), starts
   `moqui serve --host 127.0.0.1 --port N` with its log in `server.log`, and waits for `/health` and
   `server-runtime.json` (10 s after HTTP answers; ready max 60 s, archive launch max 900 s). `serverKind: managed`.
6. Prints one JSON line `{"remotePort":N,"serverKind":"managed","environmentId":"...","pairing":"<code>"}`. The
   pairing code is a `CpcDevice` code for the OS user's MR Moqui user (made by `moqui __ssh-helper pair`).
7. Holds the lock until the server is recorded and ready, so a second launch reuses it.

### 7.2 Users on the remote host

- The first launch on a fresh runtime makes an MR Moqui user named after the SSH user, with `AgentCodeUse` and
  `AgentCodeHost`, and no password (sign-in only by pairing over SSH). This is a dev or personal host, so this is
  safe: who can SSH in as that OS user already owns the files.
- Launching against an existing shared server (the runtime directory belongs to another user or is a production
  instance) is refused. The script stops when `instance@purpose` is `production`.

### 7.3 Forward and helpers

- `ssh -o ExitOnForwardFailure=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -L <local>:127.0.0.1:<remote>`.
  ControlMaster reuse when the user has one (2FA hosts).
- Helpers `moqui __ssh-helper pick-port|wait-ready|runtime-port|keep-server|pair|home-owned`.
- Cleanup stops the server only for `managed`.
- Timeouts as T3: SSH ready 20 s, remote ready 60 s, base launch 90 s, archive launch 900 s, tunnel stop 2 s.

### 7.4 Server-side SSH launches

- A server can also launch on another host for device hosts ([06 §5.2](06-Terminal-Preview-Devices.md#52-hosts)) or
  as a link target. It uses `moqui_core::sftp` `SshExec` with pinned host keys (`AgentSshHost.hostKeyPin`), never
  `~/.ssh`. Refused under the read fence and in sim mode (existing rules).

## 8. Relay and Tailscale

### 8.1 The edge node instead of T3 Connect

T3 Connect is a hosted relay (Clerk sign-in, Cloudflare tunnels, push, webhooks). MR Moqui has the edge node, self
hosted. Mapping:

| T3 Connect | MR Moqui |
|---|---|
| Managed Cloudflare tunnel hostname per environment | `EdgeHost` per paired local node (passthrough or terminate). The local node dials out; no open port. |
| Relay mints a bootstrap credential bound to the client DPoP key | Not needed: the client pairs with the environment itself through the edge host (§3). |
| Push (APNs, FCM) through the relay | The server sends push itself; a local node behind NAT can hand pushes to its edge node (`EdgeQueueItem` kind `push`, [12 §5.1](12-Mobile.md#51-push)). |
| Webhook forward `/v1/hooks/:endpointKey/:hookId/:token`, hold while offline (1 MiB, 24 h), `x-t3-relay-delivery-id` | The edge node takes `/hooks/{hookId}/{token}` for its hosts and holds them while the local node is down (`EdgeQueueItem` kind `agent-webhook`, 1 MiB, 24 h); header `x-moqui-edge-delivery-id` for dedupe ([05 §5](05-Automation.md#5-webhooks)). |
| `hooks/wake` | The edge link up event replays held items oldest first. |
| Device list, link list (`/v1/environments`) | The user's app accounts. There is no central list of a user's environments. |
| Account limits (3 machines) | None. |
| `t3 connect login/link/status/publish/unlink/logout`, `t3 up --connect` | `moqui code edge status`, `moqui code edge setup` (opens the existing attach wizard `/qapps/system/EdgeNode/Pair`). RPC `edge.status`, `edge.setup` ([09 §3](09-Wire-API.md#3-methods)). |

Open decision: a hosted relay (moqui.org) for users with no edge node. Not in this plan; the edge node covers it.

### 8.2 Tailscale

- `crates/moqui-host/src/tailscale.rs` (new): reads `tailscale status --json` (self IPs, MagicDNS name),
  `tailscale serve status --json`; runs `tailscale serve --bg --https=<port> http://127.0.0.1:<port>` and
  `tailscale serve --https=<port> off`; probes the HTTPS endpoint. Errors as T3 (spawn, exit, timeout, parse,
  HTTPS disabled).
- `100.64.0.0/10` counts as private for `private_network_only`.
- `moqui pair --tailscale [--tailscale-serve-port 443]` makes a pairing link with the tailnet URL.
- Server setting `tailscaleServe` (default false) turns it on at start.

## 9. Cluster and edge

- **Cluster**: one environment. Any node takes `/agentws`, `/mcp` and REST. Work-root actions (files, git, terminal,
  preview, harness, devices) run on the node in `hostInstanceId`. The receiving node forwards them over the cluster
  channel (`crates/moqui-host/src/cluster_edge`) as a `code.forward` call with the user, the method and the params;
  streams forward as a channel per stream.
- A harness process belongs to one node. When that node goes down, its runs are marked `interrupted`; resume needs
  that node back or a move of the work root ([02 §5](02-Orchestration.md#5-interrupt-stop-recovery)).
- New projects pick the host node by the `AgentCodeSetting` row `cluster.hostNodes` (a list, added to the `server`
  section of `settings-schema.json` in P6) and free disk; the user can choose.
- Effects (`AgentCodeEffect`) are leased per node; harness effects only on the work-root node.
- Schedulers (automations, PR watch, cleanup) run on one node by the existing ServiceJob lease.
- **Edge**: a local node behind an edge node is a normal environment at its edge host name. Through an edge node in
  terminate mode the client counts as remote: terminals ask for a recent sign-in (existing `RequestVia` rule).
  Held work for the local node while it is asleep: webhooks (§8.1), pushes, and queued link calls (`EdgeQueueItem`
  kind `agent-link`, 1 hour).
- **Desktop node + edge**: the desktop in-process node can pair with an edge node (existing attach wizard). Then the
  phone reaches the desktop's agents through the edge host while the desktop is awake; while it sleeps the phone
  shows the cached data and outbox ([12 §6](12-Mobile.md#6-offline)).

## 10. Background service and updates

### 10.1 Service

| T3 | MR Moqui |
|---|---|
| `t3 service install/restart/uninstall/status` (systemd user unit with linger, LaunchAgent, Task Scheduler) | `moqui service install/restart/uninstall/status` (new): systemd user unit `moqui.service` with linger; LaunchAgent `org.moqui.server.plist`; Windows Task Scheduler task. For system installs the existing packaging (system unit, container) stays. |
| Status problems `linger-unavailable`, `linger-disabled`, `user-manager-unavailable`, `service-disabled`, `service-stopped`, `restart-pending` | Same names in `moqui service status --json`. |
| `t3 service protect` (OOM protection) | `moqui service protect`: `OOMScoreAdjust=-500`, `MemoryMin` from a setting. Agents run at lower priority (`nice 10`) already in the process host ([03 §4](03-Providers.md#4-the-process-host)). |
| `service.env` overrides | `runtime/conf/service.env` (KEY=VALUE; cannot set `PATH` or the runtime directory). |
| `t3 up [--lan] [--headless]` | `moqui code up [--lan] [--tailscale] [--edge]`: asks for access routes, installs the service, prints a pairing QR. |
| `t3 status` | `moqui code status [--json]`. |

### 10.2 Updates

| T3 | MR Moqui |
|---|---|
| Exact-version staging, preflight, immutable runtime | `moqui update [version] [--channel stable\|nightly] [--allow-downgrade] [--yes]` (new for single-binary installs): downloads, checks SHA-256 and signature, runs `moqui --preflight` (config and migration check), keeps the old binary. |
| Launcher snapshots SQLite before the switch | Embedded database: back up first (existing `backup_EmbeddedDatabase`). Postgres: no snapshot; migrations are forward-only and checked by preflight. |
| Trial start, `prepared`, commit, else restore | Service launcher (`moqui __service-launcher`) starts the new binary as a trial: migrations, bind, then parks run resume at an activation gate and reports `prepared`; the launcher commits; else it restores the old binary and backup. |
| Clients match the update id with the ready event | Descriptor `updateId`; `/agentws` hello carries it. |
| Restart continuation (`continueThreadsAfterServerUpdate`, max 3 resends, refuse for settled, waiting on the user, 4th crash) | [02 §5](02-Orchestration.md#5-interrupt-stop-recovery); same rules. |
| Desktop app two-phase update with a prepare token | [11 §9](11-Desktop.md#9-updates-and-packaging). |
| `serverSelfUpdate` capability `boot-service`, `respawn`, `desktop-managed` | Same values ([09 §6](09-Wire-API.md#6-environment-descriptor)). Container and system-package installs report none; the UI says "Update through your package manager". |
| Cluster | Rolling update by the existing cluster rules; `/agentws` clients reconnect to another node. |

## 11. CLI

`moqui code <command>` (new, in `crates/moqui/src/cli/code.rs`; the logic is in `moqui-code`). It talks to the running server: the local one through
`server-runtime.json` and a local admin key, or `--server <url>` with a device key from `moqui code login`.

| T3 | MR Moqui |
|---|---|
| `t3 pair [--tailscale]` | `moqui pair [--label] [--tailscale] [--user <u>]` |
| `t3 auth pairing create/list/revoke` | `moqui code pairing create/list/revoke` |
| `t3 auth connect-device list/approve/revoke` | `moqui code device list/approve <code>/revoke` |
| `t3 auth session issue/list/revoke` | `moqui code session issue --scope ... [--ttl]/list/revoke` (a scoped `UserLoginKey` for scripts and bots) |
| `t3 environment link/list/unlink` | `moqui code link/links/unlink` (§6) |
| `t3 browser setup` | `moqui code browser setup` (headless Chromium download by the existing `./moq chromium-download` path, AppArmor profile, fonts) |
| `t3 project add/remove/rename` | `moqui code project add <root> [--title]/remove <id\|root> [--delete]/rename` |
| `t3 thread list/start/send/show` | `moqui code thread list [--project]/start <project> <message\|-> [--title] [--provider] [--model] [--worktree] [--branch]/send <thread> <message>/show <thread> [--limit]` |
| `t3 up`, `t3 status` | `moqui code up`, `moqui code status` |
| `t3 service ...` | `moqui service ...` (§10) |
| `t3 update`, `install`, `uninstall` | `moqui update` (§10.2); install and uninstall stay with the packaging |
| `t3 theme set/clear/show` | `moqui code theme set/clear/show` |
| `t3 trace summary` | `moqui code trace summary [--since 30m]` over the existing trace store |
| `t3 doctor [--fix]` | `moqui code doctor [--fix] [--dry-run]`: Agent Code storage size, stale worktrees, orphan harness homes, change feed size |
| `t3 policy show/check/templates/settings` | `moqui code policy show/check <file>/settings` (`AgentCodePolicy`) |
| `t3 triage` | `moqui code triage [--agent] [--model]` ([04 §12](04-Git.md#12-triage)) |
| `t3 connect ...` | `moqui code edge ...` (§8.1) |
| `t3 acp-mcp-bridge`, `acp-mcp-call` | `moqui code mcp-bridge` (stdio to `/mcp` for ACP agents), `moqui code mcp-call <tool> <json>` |
| `t3 app [dir]` | `moqui code open [dir]` (opens the desktop app at the project, or the browser) |

## 12. Maintenance and diagnostics

- Settings > Storage: worktree cleanup (inactive after N days, settled after N days, per project), browser artifacts
  and log age, a report per run (`AgentCodeCleanupRun`). Rules from T3 `storageCleanup.ts`: skip threads held for
  usage-limit recovery, with queued turns, or whose PR must merge first; keep orphan folders younger than 1 hour.
- Settings > Diagnostics: provider process list with CPU and memory, `/agentws` sockets per user, change feed lag,
  effect queue, the event loop stall list (Tokio task stalls over 2 s from the existing runtime metrics), trace
  summary.
- Settings > Maintenance: database size, prune of old `AgentCodeChange` and `AgentCodeEffect` rows, `doctor` actions.
