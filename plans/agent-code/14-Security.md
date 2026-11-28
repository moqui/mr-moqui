# 14. Security

Part of [Agent Code](README.md). T3 sources: `t3:apps/server/src/auth/**`, `t3:packages/contracts/src/{auth,
clientRpcPermissions}.ts`, `t3:apps/server/src/mcp/{McpToolAccess,linkOrigin,threadAccess}.ts`,
`t3:docs/internals/environment-auth.md`. MR Moqui sources: [docs/security/](../../docs/security/Attack-Surface.md),
[plans/archive/Security-Review.md](../archive/Security-Review.md), [docs/security/Recent-Auth.md](../../docs/security/Recent-Auth.md).

Agent Code runs model-driven programs with shell access on a server that many people use. T3 trusts the one user of
the machine. MR Moqui cannot. This file lists every gate.

## 1. Permissions and groups

| Permission | Grants | Default group |
|---|---|---|
| `AgentCodeUse` | The app, own threads, projects one is a member of, own settings, own devices and pairing, own provider sign-ins, own automations and guest shares. Runtime modes up to `auto-accept-edits`. | `AGENT_CODE_USERS` |
| `AgentCodeHost` | Things that run as the server's OS user outside the provider's own sandbox: thread terminals, project scripts, `full-access` runtime mode, device hosts, local preview of host ports, opening a project on any host path. | `AGENT_CODE_HOSTS` |
| `AgentCodeAdmin` | Environment settings, policy, provider instances, peer links, other users' threads (read and act), storage and maintenance, all usage. | `AGENT_CODE_ADMINS` |

- Seed data in `runtime/components/base/agent-code/data/AgentCodeSecurityData.xml`: the three groups and an
  `AGENT_CODE_APP` artifact group with the screens and `org\.moqui\.code\..*` services, `inheritAuthz="Y"`. `ADMIN`
  gets `AUTHZT_ALWAYS`.
- `AgentCodeHost` and `AgentCodeAdmin` are **sealed** permissions: only a holder can grant them
  (`assertSealedPermissionGrant`). `AgentCodeHost` is code execution as the server's OS user, like `TERMINAL_HOST`.
- Agent Code also needs `AgentUse` (the agent runtime) for the native engine. A user without `AgentUse` can still use
  outside harnesses (Codex, Claude) when the admin allows it (the `AgentCodePolicy` row
  `access.harnessWithoutAgentUse`, default false; added to the `policy` section of `settings-schema.json` in P1, with
  the native engine seam).
- One switch: the webroot's `<endpoint path="/agentws" enabled="..."/>` (`true`, `false`, or `dev`, the default: on only
  on a dev instance), as PR #213 builds it. It turns the `/agentws` socket and the descriptor on; on a production
  instance the admin sets it to `true`. It is the switch for all of Agent Code, not the socket alone: the REST resources
  (`/rest/s1/code/*`) and the MCP tools (`code_thread_send` and the others) read the same endpoint value and refuse
  while it is off, so turning `/agentws` off stops Agent Code, and the screens show that it is off on this instance.
  While it is on, the screens, REST resources, services and tools are gated by `AgentCodeUse` and the sealed
  `AgentCodeHost` and `AgentCodeAdmin`. Today the route is mounted only when the switch is on at listen, and `endpoint`
  is a restart class, so a change waits for a restart; once [Runtime-Config
  §3.8](../Runtime-Config.md#38-services-carried-on-a-listener) is built the route stays mounted and reads its switch on
  each request, so it turns off and on while the server runs ([09 §2.1](09-Wire-API.md#21-route-and-auth)).

## 2. Scopes

Every `/agentws` method, REST route and MCP tool names one scope. A browser session holds every scope its
permissions allow. A login key holds only the scopes it was granted (stored on master's one-scope-per-key model,
"Key scopes on master" below). A request passes only when both the scope and the permission pass.

| T3 scope | MR Moqui scope | Needs permission |
|---|---|---|
| `orchestration:read` | `code:read` | AgentCodeUse |
| `orchestration:operate` | `code:operate` | AgentCodeUse |
| `settings:write` | `code:settings` | AgentCodeUse (own), AgentCodeAdmin (environment) |
| `providers:manage` | `code:providers` | AgentCodeAdmin (instances); AgentCodeUse (own sign-in) |
| `environment:maintain` | `code:maintain` | AgentCodeAdmin |
| `preview:operate` | `code:preview` | AgentCodeUse; host ports need AgentCodeHost |
| `diagnostics:read` | `code:diagnostics` | AgentCodeAdmin |
| `terminal:read` | `code:terminal-read` | AgentCodeHost |
| `terminal:operate` | `code:terminal` | AgentCodeHost |
| `filesystem:read` | `code:files-read` | AgentCodeUse, inside the user's project roots; outside them AgentCodeHost |
| `filesystem:write` | `code:files-write` | as `code:files-read` |
| `review:write` (not grantable) | `code:review` (not grantable to a key; browser session only) | AgentCodeUse |
| source-control write | `code:scm-write` | AgentCodeUse |
| `access:read` | `code:access-read` | AgentCodeUse (own), AgentCodeAdmin (all) |
| `access:write` | `code:access-write` | as `code:access-read` |
| `relay:read`, `relay:write`, `relay:configure` | `code:edge-read`, `code:edge-write` | AgentCodeAdmin |
| (MCP session token) | `code:mcp-session` | the run's user; tools by capability ([08 §1](08-MCP-Tools.md#1-transport-auth-and-injection)) |

Rules (kept from T3):

- A pairing code or a new key can carry only scopes the maker holds. Making a pairing code for others needs
  `code:access-write` plus every scope it gives.
- Exchange narrows, never widens. Unknown names are dropped. No scope left fails before the code is spent.
- Errors carry `requiredScope` and `requiredPermission`.
- Default device pairing grants `code:read code:operate code:settings code:preview code:files-read code:scm-write
  code:access-read`, plus `code:terminal` and `code:files-write` when the user holds `AgentCodeHost` and ticks them.
- The existing `UserLoginKey.scope` meaning stays: empty = full account. A key with any `code:*` scope is limited to
  Agent Code routes and the client sync routes. A full-account device key (empty scope) has every `code:*` scope its
  permissions allow, so the existing app keys keep working.
- Scope table in code: `crates/moqui-code/src/scopes.rs`, one entry per method, checked by a test that every
  method has one.

**Key scopes on master (to design P6 on).** The table above is T3's model, a list of scopes per key. MR Moqui's
login keys now hold one scope each: `UserLoginKey.scope` is empty (the full account) or one name, a key of one
scope is made by `create#UserScopedKey` for a name in `KEY_SCOPES` (`gateway`, `git`, `tasks`;
`crates/moqui-core/src/user/login.rs`), and it signs in only where that scope is declared: a REST resource by its
`key-scope` attribute (for it and the resources below it), a service's own login by `login_user_key_for(key,
scope)` (df3a194e3). A request a scoped key signs in takes no web session (f66acffcf). The pairing, exchange and link
keys of P6 ([13 §3](13-Servers-Linking.md#3-pairing-and-sessions), [16](16-Phases.md#2-phases)) are designed on that
model before they are built: a new key scope (for example `code`) added to `KEY_SCOPES`, declared by Agent Code's
REST resources and checked by `/agentws` at upgrade, with the finer `code:*` rows of the table becoming checks of the
user's permissions and the key's grants inside that one scope, not a list in `UserLoginKey.scope`. Until that design
is written, the `code:*` rules above are T3's and not a storage format.

## 3. Runtime modes and access levels

| Mode (T3) | Meaning | Needs |
|---|---|---|
| `approval-required` | Every write and command asks. | AgentCodeUse |
| `auto-accept-edits` | File edits in the work root pass; commands ask. | AgentCodeUse |
| `auto` | Provider's auto mode (Codex `auto`, Claude `acceptEdits` plus allow list). | AgentCodeUse and project setting `allowAutoMode` |
| `full-access` | No asks; provider sandbox off. | AgentCodeHost and project setting `allowFullAccess` |

- Default `approval-required` ([10 §9.3](10-Web-UI.md#93-server-settings)).
- Policy `limits.maxRuntimeMode` caps the mode for everyone. A project can cap it lower.
- Switching a running thread to `full-access` needs a recent sign-in (`check#RecentAuth`, 300 s).
- MCP clients and peer links have an access level (`read-only` ... `full-access`) that is a ceiling on the modes of
  the threads they start; the receiver only narrows (`Moqui-Code-Mode-Limit`).
- Automations: a trigger saved by a user without `AgentCodeAdmin` runs at most `auto-accept-edits`
  ([05 §4](05-Automation.md#4-thread-modes-and-launch)).

## 4. Approvals

- Harness approvals (Codex `requestApproval`, Claude `canUseTool`, ACP `session/request_permission`) become
  `AgentApproval` rows, the same as native engine writes ([03 §2](03-Providers.md#2-the-adapter-trait)). One code
  path decides them.
- Who decides: the thread owner, in a person's session (browser session, or the app with the device key in the
  foreground). Not a login key with only `code:operate` from a script: deciding an approval needs scope
  `code:review`, which no key can hold.
  - The app device key (empty scope) counts as a person's session for approvals, because the app is the person's
    device. The app shows the approval card; a notification action never approves ([12 §5.2](12-Mobile.md#52-android-notifications)).
- Project delegation: a project owner can let members decide each other's approvals (`AgentProjectSetting`
  `approvalDelegation`). Off by default. This avoids SEC-AI-01 (a room member approves a write that runs as another
  user): the decider must be the owner or an explicit delegate, never just someone who can see the thread.
- TTL: `approval-ttl-minutes` (30) as today; an expired approval answers the harness `denied` with reason `expired`.
- A one-click write is never offered from a model-made UI (MCP Apps, HTML renders, plan cards). Every write from such a
  UI becomes an approval card with the full action text. This avoids SEC-AI-02.
- Approvals across links: a linked environment's approval is decided on the environment that runs the work, by its
  own user. The linking side can show it read-only.

## 5. Process isolation

### 5.1 Today's baseline

- Harness processes, terminals and scripts run as the server's OS user, in the work root, with a cleaned environment:
  `env_clear()` and only the variables the instance and the provider instance set (`AgentProviderInstanceEnv`),
  plus `HOME` = the provider account home. This avoids SEC-AI-12 (the external agent adapter inherits the server
  environment); Agent Code fixes the old adapter in the same phase.
- Each user's provider home is separate (`<agent-code@home>/homes/<userId>/<instance>/`, mode 0700), so one user's
  Codex or Claude login is not another user's.
- Work roots are under the worktrees directory (the `worktreesDirectory` setting, else `<agent-code@home>/worktrees/`,
  [02 §9](02-Orchestration.md#9-worktrees)) or a project root the admin allowed (the `AgentCodePolicy` row
  `access.projectRoots`, a list, added to the `policy` section of `settings-schema.json` in P2). Opening a project
  elsewhere needs `AgentCodeHost`.
- Lower priority: `nice 10`, and on Linux a cgroup per thread (`agent-code.slice/<conversationId>`) with memory and
  CPU limits from settings when systemd is present.
- The runtime directory (`runtime/conf`, `runtime/db`, keys, `.env`) is not readable from a work root by path rules in
  the file tools; the OS user can still read it from a shell. That is why terminals and `full-access` need
  `AgentCodeHost`.

### 5.2 Stronger isolation (open decision O7)

| Option | What it gives | Cost |
|---|---|---|
| Landlock (Linux 5.13+) | Write only to the work root, `<agent-code@home>/homes/<user>`, `/tmp/<thread>`; read deny for `runtime/conf`, `runtime/db`, `.env` | Small; no root; per process at spawn. Needs a fallback on older kernels and macOS. |
| bubblewrap namespaces | Own mount view: only the work root, the home, system dirs read-only; no `runtime/` at all | Needs `bwrap` installed; harder with provider CLIs that need `~/.config`. |
| Separate OS user (`agent-code@run-as`, started by [Terminal-Fancy2](../Terminal-Fancy2.md) TF6's sudo-gated `moqui term-exec` supervisor, the same path as a person's own OS account for host sessions) | Real OS boundary from the server's files | Needs root setup (a sudoers line and a root-owned `/etc/moqui/run-as.conf`); file ownership of work roots. |
| Container per thread | Strongest | Heavy; not for desktop. |

Recommendation: Landlock on Linux in P2, the separate OS user as an admin option in P6 ([16](16-Phases.md) O7). macOS
and Windows desktops stay on the baseline (one user owns the machine there).

### 5.3 Network

- Provider processes need the network (model APIs). No egress filter in phase 1.
- The preview tunnel and port forward only reach loopback ports owned by the thread's processes (Linux owner check;
  elsewhere the ports of the work root's process tree) ([06 §2.1](06-Terminal-Preview-Devices.md#21-model)).
- `private_network_only` for LAN binds ([13 §2](13-Servers-Linking.md#2-server-process-and-deployment)).

## 6. Threats and answers

| Threat | Answer |
|---|---|
| A prompt in a repo, issue, PR comment, webhook body or web page tells the agent to leak secrets or push code | Runtime mode asks for commands by default; SCM writes are EXTERNAL approvals (as the Dev AIDE rule); webhook text is put in a marked untrusted block ([05 §5](05-Automation.md#5-webhooks)); secrets reach the agent only as refs or files ([07 §1](07-Secrets-Guest-Voice-Usage.md#1-secrets)). |
| Cross-user read through a shared project | Project members only; thread owner rule (§4); the file tools stay in the user's project roots. |
| Stolen device key | DPoP binding when on; key renewal; revoke in Settings > Access; device approval setting. |
| Pairing code guessing | 12 chars of 32 letters (60 bits), 5 tries, 5 min, hash only, rate limit by the existing tarpit. |
| Pairing link leak through history or Referer | Code in the fragment; page strips it; `Referrer-Policy: no-referrer` on `/qapps/code/pair`. |
| Cross-site socket | Origin check on `/agentws` as `/notws`; tickets one use. |
| One user holds every stream | Per-user limits: 4 sockets per device, 64 streams per socket, 16 waits per user. This avoids repeating SEC-AI-06. |
| Malicious peer link | Access level ceiling, fencing by `linkId`, address trust, receiver-owned expiry ([13 §6](13-Servers-Linking.md#6-peer-links)). |
| Webhook forgery and replay | Token in the path (hash stored), signature check, max age, replay keys ([05 §5](05-Automation.md#5-webhooks)). |
| Guest link abuse | Guest sees one thread read-only; messages wait for the owner; HMAC token; off by default ([07 §3](07-Secrets-Guest-Voice-Usage.md#3-guest-share)). |
| MCP App or HTML render runs script against the app | Sandboxed iframe on a separate origin (`/codestatic/sandbox/`), no same-origin, CSP; tools only through `postMessage` with approval for writes ([06 §3-4](06-Terminal-Preview-Devices.md#3-html-renders-visual-replies)). |
| Custom CSS and themes | Sanitized; no `@import`, no other-origin `url()` ([10 §9.4](10-Web-UI.md#94-settings-pages)). |
| Desktop sidecar open to other local users | Agent Code on a sidecar node adds a Host check and a per-start token for loopback; the in-process node has no port. This closes SEC-CLI-01 for Agent Code and should be done for the whole sidecar. |
| Provider sign-in tokens | In the user's provider home (0700) or secret sources; never in settings rows or logs. |
| Logs leak prompts and secrets | Secret values are scrubbed from tool output by the known refs; trace attributes carry ids, not text. |
| Harness binary swapped on the PATH | `binaryPath` pinned per instance by the admin; the version check records the path and hash; a change shows a warning and needs a re-approve. |

## 7. Audit

| Event | Where |
|---|---|
| Every command (`commandId`, type, user, thread) | `AgentCodeCommand` (7 days), plus `AgentAuditLog` for admin actions on other users' threads |
| Approvals and decisions | `AgentApproval` (existing) |
| Terminal sessions | `TerminalSession` (existing) with `conversationId` |
| Pairing, device approve, revoke, link, unlink | `AgentCodeAccessLog` (new: logId, userId, actionEnumId, targetText, clientIp, date) |
| Runtime mode raised to `full-access` | `AgentCodeAccessLog` |
| Webhook deliveries | `AgentWebhookDelivery` |
| Authz failures | `ArtifactAuthzFailure` (existing) |


## 8. Attack-surface docs

The port adds routes, sockets and child processes. These rows go into the map in the same change as the code:

| File | New rows |
|---|---|
| [Attack-Surface-Routes.md](../../docs/security/Attack-Surface-Routes.md) | `/agentws`; `/.well-known/moqui/agent-code`; `/.well-known/oauth-*`; `/oauth/token`; `/oauth/mcp/*`; `/rest/s1/code/*` (shell, history, wsTicket, links thread routes, pullRequests, attachments); `/hooks/{hookId}/{token}`; `/guest/{shareId}` and `/guest/api/*`; `/preview/tunnel`; `/qapps/code/pair`; `/codestatic/*` and the sandbox origin. Each with auth, scope, limits. |
| [Attack-Surface-Process.md](../../docs/security/Attack-Surface-Process.md) | Harness processes (codex, claude, ACP agents, opencode, cursor, ...), project scripts, thread terminals, headless Chromium, device hubs, `tailscale`, `ssh`, `git`, `gh`/`glab`, `op`, the speech worker; with env rules, cwd, user, limits, and the isolation of §5. |
| [Attack-Surface-AI.md](../../docs/security/Attack-Surface-AI.md) | Agent Code MCP tools and their classes ([08 §2](08-MCP-Tools.md#2-tool-list)); MCP session keys; harness approvals; runtime modes; prompts from webhooks, PRs and issues; MCP Apps and HTML renders. |
| [Attack-Surface-Dev.md](../../docs/security/Attack-Surface-Dev.md) | How Agent Code differs from Dev AIDE: a production component gated by permissions, not by `is_dev_instance`; Dev AIDE roots shared through `DevAideRoot.workRootId` only on dev instances. |
| [Attack-Surface-Clients.md](../../docs/security/Attack-Surface-Clients.md) | Host-held `/agentws`; new desktop commands ([11 §2](11-Desktop.md#2-host-api)); preview CEF browsers; askpass helper; outbox kinds; widget store. |
| [Attack-Surface-Cluster-Edge.md](../../docs/security/Attack-Surface-Cluster-Edge.md) | `code.forward` cluster calls; edge queue kinds `agent-webhook`, `push`, `agent-link`. |
| [Attack-Surface-Outbound.md](../../docs/security/Attack-Surface-Outbound.md) | Code host APIs, provider APIs called by harnesses, peer links, APNs, Tailscale. |
| [Attack-Surface-SSH.md](../../docs/security/Attack-Surface-SSH.md) | SSH launch script and helpers; server-side SSH launches. |

## 9. Findings that touch the port

All are fixed in [plans/archive/Security-Review.md](../archive/Security-Review.md) (SEC-FIL-64 in part: the ticket
is still reusable for its 300 seconds). The port keeps each rule below and must not bring the weakness back.

| Id | Finding | What the port does |
|---|---|---|
| SEC-AI-01 | A room member approves a write that runs as another user | Owner or explicit delegate only (§4). Fix the shared approval code in phase 1 since both paths use it. |
| SEC-AI-02 | OpenUI button runs a model write on one click | No one-click writes from model UI (§4). |
| SEC-AI-06 | One user holds every Assist event stream | Per-user stream limits on `/agentws` (§6). If Assist moves to `/agentws` (O11), this one closes. |
| SEC-AI-09 | Three agent services check nothing | Agent Code services all declare authz; a spec lists every service and checks its artifact group. |
| SEC-AI-12 | External agent adapter inherits the server environment | `env_clear` for all harness spawns (§5.1); fix the old adapter in the same change. |
| SEC-DEV-01, SEC-DEV-04 | Dev AIDE export and deny list | Agent Code does not use Dev AIDE file tools; its own file tools use the project root rule. |
| SEC-FIL-02 | App password opens the admin shell over SSH | Not used by Agent Code; SSH launch uses the user's own SSH keys to a remote host. |
| SEC-FIL-64 | Terminal ticket reusable, in the query string | Thread terminals reuse `/termws`; the Agent Code ticket (`wsTicket`) is one use; propose the same for `confirm#TerminalUser`. |
| SEC-FIL-65 | `command-line`/`host-sessions` on in production for most values | Thread terminals need `AgentCodeHost` and the switch `terminal` `host-sessions`; the port fixes the switch to "on only for `true` or a dev instance". |
| SEC-CLI-01 | Sidecar loopback listener open to every local user | Host check and start token (§6). |
| SEC-CLI-07 | Shell commands not bound to an account | New `code_*` commands check the account origin (§2 of [11](11-Desktop.md#22-rules)). |

## 10. Proof tests

Each gate of this file gets a proof test in the regression suite (see [15](15-Tests.md) and
[docs/security/Proof-Tests.md](../../docs/security/Proof-Tests.md)): scope refused, permission refused, approval by
a non-owner refused, `full-access` without `AgentCodeHost` refused, pairing code reuse refused, wrong environment id
gets no key, link fencing, guest write refused, sandbox iframe has no same-origin, harness env has no server
variables.
