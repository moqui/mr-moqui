# Runtime Config: services and ports on and off while running

Status 2026-11-27: plan; nothing built. Its open points were decided with the user the same day (§6).

Goal: an administrator turns services and ports on and off while the server runs, from a dialog on System Home, from
the terminal, or by editing the conf, and no listener setting waits for a restart. It serves two tasks: setting a
server up one service at a time, and an incident where some services go off for a while (HTTPS off with SSH left on,
or the reverse). With it the defaults shrink: a new server opens HTTPS (8443) and SSH (2222) and nothing else, and the
cluster port opens only when there is a cluster.

Sections: [1 Rules](#1-rules) · [2 What is there now](#2-what-is-there-now) · [3 Design](#3-design) ·
[4 Phases](#4-phases) · [5 Tests](#5-tests) · [6 Decisions](#6-decisions) · [7 Not in scope](#7-not-in-scope)

## 1 Rules

- One mechanism. The dialog, System › Conf, `runtime/.env`, an environment name, SIGHUP and the operator socket all
  change the same conf settings, and one reconcile makes the listeners match the running conf. The dialog keeps no
  state of its own.
- A switch is a conf setting: an `enabled` attribute on the listener's own element with its default written there, and
  a port with its number written there (AGENTS.md, Conf settings). Turning a listener off never loses its port.
- Off means closed: the port is unbound and its connections end. A listener that cannot start says why on System Home
  and the others go on (AGENTS.md, Failure tolerance).
- No lockout by accident. Turning off the last way in is refused unless confirmed, and a way back that needs no
  listener always exists (§3.6).
- Every change is logged with who and why.

## 2 What is there now

| Area | Now | Where |
|---|---|---|
| Listener lifecycle | Each listener binds once in `boot_server_factory` or `http::listen` and is owned by `ServeProcess` until the process stops. No service or screen starts or stops one | `crates/moqui-host/src/lib.rs`, `crates/moqui-host/src/http.rs` |
| Live already | The HTTPS listener of a node with no certificate binds and closes on a watch (`standing_by`). Edge per-pairing ports (`EdgePort` rows) open and close within seconds (`watch_rows`, `reconcile`). Certificates in `runtime/cert` are re-read every 5 s | `http.rs`, `edge_link/tunnel.rs`, `tls.rs` |
| Stop handles | Each listener already has one: a shared watch for HTTP, HTTPS and gRPC; a watch sent on `Drop` for SSH, SMTP and TURN; an own runtime for XMPP and SIP; an `AtomicBool` for call media | `ssh/server.rs`, `smtp.rs`, `xmpp/mod.rs`, `sip/mod.rs`, `rtc/mod.rs` |
| Conf reload | The files are merged again and compared with the running conf by unit; a unit is `Live`, `Apply(Group)` or `Restart(reason)`. Hooks exist for Screens and WebProtection only. Triggers: System › Conf save, `reload#Conf`, SIGHUP, the dev watcher, a component load | `crates/moqui-core/src/context/conf_reload/mod.rs`, `conf_schema.rs` |
| Listener classes | `LISTENER` and `BOOT` are restart classes: the webroot ports, `ssh-server`, `smtp-server`, `xmpp-server`, `sip-server`, `media-server`, `turn-server`, `cluster-member`, `edge`, `endpoint`. `https-listen-port` and `http-bind` fall under the webapp's `Live` default though they are read once at listen | `conf_schema.rs` (`CATALOG`) |
| One setting, set live | `set_value` (`factory.set_conf_value`) patches one attribute of the running conf and keeps it in `runtime/.env` (a secret in the seal store) or as a process property. It runs no hook. Edge pairing and the DKIM key use it; specs use it as `setConf` | `conf_reload/mod.rs` |
| How "off" is written | Differently for each: `http-listen-port="off"`, an empty `port` (SSH, SMTP, SIP, media), an empty `domain` (XMPP), `ssh-server@enabled`, and nothing at all for the cluster port | `runtime/components/base/framework/conf/MoquiDefaultConf.xml` |
| Default open ports | HTTP 8080 on every address, HTTPS 8443 when `runtime/cert` has a pair (a dev instance makes one), cluster 8444 on every address, always. SSH is off (`port=""`, host 127.0.0.1) | `MoquiDefaultConf.xml`, `lib.rs` (`listen_address`), `tls.rs` (`plan`) |
| Port check | Before the database opens every port is bound and released. HTTP, HTTPS, cluster and SSH are required (a busy one stops the boot); the rest are skipped when busy | `crates/moqui-host/src/port_check.rs` |
| SSH access | Read once in `start`: password and public key always offered, keyboard-interactive (the login dialog) when `login-dialog` is true, which lets anyone reach the TUI sign-in. No setting turns password auth off or requires a key. Keys, passwords and the `SSH_*` permissions are checked per connection | `crates/moqui-host/src/ssh/server.rs` |
| Cluster port | Bound whenever the process serves, whatever the transport and with no other member; a start without it fails ("http server has no cluster listener"). Members are `moqui.server.ClusterMember` rows; a new one is noticed at the next heartbeat (60 s by default) or the next cache-invalidating write. No event fires on a new row | `lib.rs`, `cluster_edge/listen.rs`, `crates/moqui-core/src/cluster/mod.rs`, [archive/Cluster.md](archive/Cluster.md) |
| System Home | The "Listeners and integrations" box lists `ec.factory.getStartupReport()` rows: what each listener did at startup. `StartupReport::record` replaces a row by name, so a later state can be recorded. Home has no write transition | `runtime/components/base/tools/screen/System/Home.xml`, `crates/moqui-core/src/startup.rs` |
| Conf screens | Conf › Edit saves a file and reloads (ADMIN and the sealed `CONF_EDIT`); Conf › Changes lists what waits for a restart; Protocols › Server configuration is read only; 41 `InstanceSetting` rows say `restartRequired="Y"` | `System/Conf/*.xml`, `crates/moqui-devkit/src/deploy/conf_edit.rs`, `framework/data/AdminConfigurationData.xml` |
| Ways in without a listener | The operator socket (seal commands only today) and SIGHUP (`systemctl reload`) | `crates/moqui-host/src/operator/`, `systemd/moqui.service` |
| Related plans | [Sealed-Secrets](Sealed-Secrets.md) §3.9 "Less attack surface" and phase S12 (`moqui operator surface`, a test that every optional listener is off by default), §3.8 (refuse a public bind with no TLS), §3.12 and S15 (incident runbook) | not built |

## 3 Design

### 3.1 The listener registry and reconcile

A registry in `moqui-host` (`crates/moqui-host/src/listeners/`) holds one entry for each listener, in a fixed order.
Each entry answers two things:

- **wanted**: from the running conf (and, for two listeners, a signal: cluster membership, ACME), the settings it
  should run with, or none when it is off. The settings are one comparable struct for each listener, split into what
  needs a new bind (address, port, TLS files, protocol) and what applies in place (limits, access policy).
- **start** and **stop**: what `boot_server_factory` and `http::listen` do today, moved behind the entry.

`reconcile` compares wanted with running for every entry under one lock:

| Wanted against running | Action |
|---|---|
| Same | Nothing |
| Off, running | Stop: no new connections, 5 s for requests in flight, then its connections end; at once when the change asks for it (§3.6) |
| On, not running | Check the port is free (the port check's own bind and release, for this one target), then start |
| Bind settings differ | Bind the new address first when it differs from the old, then stop the old; a failed bind leaves the old one running |
| In-place settings differ | Swap them; new connections use them |

Each action records the listener's state with `startup::listener` (On, Off with the reason, Failed, Skipped), now with
the time and what caused it, so System Home shows the state now and not the state at startup. A failure is logged
naming the listener and reconcile goes on with the next entry.

Reconcile runs at boot (the first reconcile is the start, so there is one code path), after a conf reload that changed
a listener unit, after `set_value` on one, and when a signal changes (a cluster member appears, ACME wants the
listener). The registry sits behind an extension slot in `moqui-core`, as `startup` does, so services in core and
devkit reach it without depending on the host. `ServeProcess` keeps owning the listeners; its `Drop` stops every
entry in today's order.

### 3.2 Conf shape

Every listener element gets `enabled` and a port with its number written in. One spelling of "off" replaces the
five there are now; the old spellings are retired and the boot refuses them by name (`refuse_retired`).

| Listener | Element | Switch | Default | Port default | Replaces |
|---|---|---|---|---|---|
| HTTP | `webapp[webroot]` | `http-listen-enabled` | false | `http-listen-port="8080"` | `http-listen-port="off"`, empty |
| HTTPS | `webapp[webroot]` | `https-listen-enabled` | true | `https-listen-port="8443"` | `https-listen-port="off"`, empty |
| gRPC | `webapp[webroot]` | the services' own `grpc-enabled` | false | `grpc-listen-port="4317"` | `grpc-listen-port="off"` |
| SSH | `ssh-server` | `enabled` | true | `port="2222"`, `host="0.0.0.0"` | empty `port` |
| Cluster | `cluster-member` | `listen="auto"` (`auto`, `on`, `off`) | auto | `https-port="8444"` | always on |
| SMTP | `smtp-server` | `enabled` | false | `listen-port="2525"` | empty `listen-port` |
| XMPP | `xmpp-server` | `enabled` | false | `port="5222"`, `tls-port="5223"` | empty `domain` and ports |
| SIP | `sip-server` | `enabled` | false | `port="5060"`, `tls-port="5061"` | empty `port` |
| Call media | `media-server` | `enabled` | false | `port="10000"`, `tcp-port` | empty `port` |
| TURN | `media-server/turn-server` | `enabled` | false | `port="3478"`, `tls-port="5349"` | empty `port` |
| Edge tunnels | `edge` | by each port, as now | off | unchanged | unchanged |

`webapp@https-enabled` keeps its meaning (the scheme of generated URLs) and is not the listener's switch. A listener
that is on but lacks something it needs (XMPP with no `domain`, SMTP with no `allowed-addresses`, SIP with call media
off) is recorded as Skipped with what is missing, and the dialog asks for it. The written ports are unprivileged, as
8443 and 2222 are; the edge and all-in-one confs set SMTP's to 25, as they bind 443.

The conf schema gets `Group::Listeners`. A listener's unit moves from `Restart(LISTENER)` or `BOOT` to
`Apply(Group::Listeners)` in the phase that makes it live, so Conf › Changes stops listing it as waiting; the host
registers one hook for the group that calls reconcile. `https-listen-port` and `http-bind` get an explicit class.
`set_value` runs the hook of the group its attribute belongs to, so a single setting set from a service, the operator
socket or a spec takes effect as a reload does. Each attribute is added to the XSD and to the schema entry.

### 3.3 HTTP and HTTPS

`http::listen` binds each web listener through a loop like `standing_by`: wait until wanted, bind, serve until the
wanted settings change or the process stops, close, repeat. The router, guard, permits and TLS settings are built once
and shared, as now; only the socket comes and goes. Closing ends the listener's connections after the grace, including
`/notws`, `/termws` and edge links on that listener. ACME's "wanted" becomes one input of the HTTPS entry.

A production server with nothing in `runtime/cert` makes a self-signed certificate there on first start, as a dev
instance does, so 8443 answers at once. The certificate is told apart by its file name, serves any name that has no
certificate of its own, and is no longer used once one is installed or set up with ACME. System Home shows it under
Needs attention with a link to Certificates. A self-signed certificate does not count as one for an edge node's
pairing: that node still takes no links until it has a real certificate, as standby means today.

Turning HTTP on with a public bind and no proxy believed asks for a confirmation that names the risk (the dialog,
the terminal and the operator socket), then starts, and stays under Needs attention while it is so. Set in the conf
or the environment it starts with the same warning. Whether a boot refuses it is
[Sealed-Secrets](Sealed-Secrets.md) §3.8's to decide.

### 3.4 SSH

Start, stop and a new port or host go through the registry. The edge SSH tunnel and the cluster's `transport="ssh"`
serve into the same intake, so turning SSH off while either is in use is refused with the reason (or confirmed, §3.6).

One attribute replaces `login-dialog`, read for each new connection:

| `ssh-server@access` | Connect | Methods offered |
|---|---|---|
| `open` (default) | Anyone; an unauthenticated client gets only the TUI sign-in dialog, within `max-dialog-sessions` | public key, keyboard-interactive, password |
| `login` | An account's password or one of its registered keys | public key, password |
| `key` | A registered key (`UserSshKey`) only | public key |

Under `key` an app password no longer opens SFTP. A change applies to new connections; sessions still at the sign-in
dialog end when the mode leaves `open`, and signed-in sessions stay (System › Terminal Sessions ends them). The limits
(`max-sessions*`, `max-dialog-sessions*`, `sftp-max-file`) and `git-password-auth`, `git-any-account` and
`dav-server@sftp-drives` move from the struct filled at start to settings read per connection. Four flags turn the
sub-services off one at a time, each checked when a channel asks for it: `tui-enabled`, `sftp-enabled`,
`git-enabled`, `shell-enabled` (all true; the `SSH_*` permissions still decide per user).

### 3.5 The cluster port

`cluster-member@listen`: `auto` (the default) opens the port only while a cluster is detected, `on` always, `off`
never (for `transport="ssh"` or `notify`).

- **At start**: after `cluster::start` writes this member's row, live peers with the same key and certificate
  fingerprints (`has_live_peers`) mean a cluster; the port opens before the server reports ready.
- **Later**: the heartbeat's `refresh_peers` and the re-read in `current_peers` call a membership hook when the first
  peer appears; the host's reconcile opens the port. While a member is alone the heartbeat thread reads the member
  table every `cluster-member@lone-check-seconds` (15; one indexed read, the setting read on each pass), and at the
  heartbeat's own interval once it has a peer.
- **Closing**: under `auto` the port closes when the last peer's row is deleted as dead (10 windows).
- **The joiner's side**: its first posts to a member whose port is still closed fail and back off (`PEER_BACKOFF`);
  the peer is marked dirty and gets a clear-all on first contact, which the code does today for any new peer. The
  member that was alone sends to the joiner from its next write, since the joiner's port is already open.

`boot_server_factory` no longer requires the listener, `Server::cluster_address` answers an `Option`, and the port
check treats the cluster port as optional under `auto` (a busy port when it opens later is a Failed row on Home and a
Needs attention item). The startup report says "off: no other member" for a lone member. A forged `ClusterMember` row
with copied fingerprints can make a member open the port; what it exposes is the mTLS handshake pinned to the shared
certificate, as today (SEC-CLU-89 in [Security-Review-Low](archive/Security-Review-Low.md)).

### 3.6 Services, authorization and the ways back

`org.moqui.admin.ListenerServices` (Rust, beside the conf services):

- `get#Listeners`: for each registry entry its state, address, the settings behind it with where each value comes
  from (conf file, `.env`, the environment, a process property), the connections it holds, whether the caller's own
  request came over it, and what depends on it.
- `set#Listener`: name, `enabled`, optionally port, host and the listener's own settings (SSH `access`), a reason
  (required to turn off), `persist` (default true), `endNow` (turning off ends the connections at once, with no
  grace) and `allMembers`. It checks the port is free, writes each setting with `set_conf_value`, and reconcile
  applies it. A switch has no timer: a service turned off stays off until someone turns it on. When the change
  closes the connection the request came over, the answer is sent first and the change applies a second later.

Authorization: a new sealed permission `LISTENER_CONTROL`, granted to `ADMIN_ADV` as `CONF_EDIT` is, checked in Rust,
with a recent sign-in required (`RecentAuthServices.require#RecentAuth`; the terminal's "Confirm it is you"). It is
apart from `CONF_EDIT` so an incident responder can switch services without editing the whole conf.

Kept where: `persist` writes `runtime/.env` on this server, which the conf merge reads at the next start. A setting
the OS environment names cannot be kept there; the change holds until the restart and the answer says so.

Lockout: HTTPS, HTTP and SSH are the ways in. Turning off the last one that is on, or the one the caller is on while
no other is, is refused unless `confirmLockout` is sent, and the message names the ways back (public HTTP is
confirmed the same way, with `confirmPublicHttp`, §3.3):

- `moqui operator listeners` and `moqui operator listener <name> on|off [--now]` over the operator socket, which
  needs no network listener. `listeners` is the `moqui operator surface` of [Sealed-Secrets](Sealed-Secrets.md) S12.
- Edit the conf or `runtime/.env` and reload (`systemctl reload`, SIGHUP), or restart.

Audit: a line on `moqui.monitor.audit` (user, listener, from, to, reason, as conf saves and the recovery stop write)
and a `config` annotation on the Monitor charts, as Monitor › Settings writes.

Cluster: `allMembers` publishes the change on a cluster topic before applying it here; each member writes its own
`.env`, reconciles and logs. It is refused for the cluster listener itself. Each member writes a short listener
summary on its `ClusterMember` row at its heartbeat, and Home's members table shows it, so a member that missed the
change is visible.

### 3.7 Screens

- **System Home**: the "Listeners and integrations" box gets a "Services and ports" button in its toolbar, shown to a
  holder of `LISTENER_CONTROL`. Its dialog lists each listener with its state, address and a Turn on or Turn off
  control; a row that needs more (a port, a host, SSH access, a reason) opens its form, and the turn-off form has
  "End connections now". The row the user is connected through is marked. Home stays usable by a view-only
  administrator: the button and its transitions are conditioned on the permission.
- **System › Conf › Services and ports**: the same list as a screen, included by the Home dialog so there is one
  definition (as `Conf/Logging.xml` includes `Monitor/Settings.xml`), plus each listener's own settings, the
  sub-service switches of §3.4 and §3.8, where each value comes from, and the recent changes.
- **Other screens**: Protocols › Server configuration links each row to the control and drops "After a restart" for
  switched listeners; the `InstanceSetting` rows lose `restartRequired` and their notes change; Conf › Changes no
  longer lists listener settings. Home's link table sends each listener name to the new screen (and its stale
  `ServiceJob` entry to `Operations/Jobs`).
- **Terminal**: plain XML widgets (`form-list`, `container-dialog`, links with `confirmation`), so the screens work in
  the TUI unchanged. With HTTPS off the SSH TUI is the admin's screen, and with SSH off the browser terminal is not
  needed for it.
- **Needs attention** (`get#SystemAttention`): a listener that failed after a live change, the self-signed
  certificate, HTTP open on a public address, and both HTTPS and SSH off.

### 3.8 Services carried on a listener

Routes on the HTTP and HTTPS listeners are built once, so their switches wait for a restart: `endpoint[path]@enabled`
(`/notws`, `/termws`, and, once [Agent Code](agent-code/README.md) P0 is merged, `/agentws` with its descriptor, whose
`enabled` also takes `dev`: on only on a dev instance), `dav-server@enabled`, the git server, `mcp@server-enabled`,
`a2a@enabled`, `gateway@enabled`, `channels@webhooks-enabled`, `xmpp-server@websocket-enabled`, and the OTLP receiver.
Each route stays mounted and checks its flag on each request, from state built from the conf and built again after a
reload; off answers 404 and ends its open sockets. The phase starts with an audit of every such switch (its reader and
its class) and ends with each `Live` and listed on Services and ports under its listener.

### 3.9 Defaults

`MoquiDefaultConf.xml` opens HTTPS on 8443 and SSH on 2222 (every address, `access="open"`) and nothing else: HTTP
off, the cluster port `auto`, every optional listener off. So `MoquiProductionConf.xml` needs no listener line, and a
server is set up by turning on what it needs, in the conf beforehand or live. The services carried on a listener
(§3.4, §3.8) keep today's defaults: DAV, `/notws`, `/termws`, SFTP and git over SSH on, each still decided per user
by its permission; MCP, A2A, the gateway and webhooks off; `/agentws` `dev` (once Agent Code P0 is merged).

What follows the change: `MoquiDevConf.xml` drops the lines that become the default; the topology confs whose nodes
take plain HTTP from a load balancer or front node turn it on; `podman/` publishes 8443 and 2222 and its health
checks ask `https://127.0.0.1:8443/health`; `scripts/moq` port flags; `docs/Run-Deploy.md` (the ports table, the
firewall and cluster steps), `docs/security/Attack-Surface.md` and `Attack-Surface-Cluster-Edge.md`,
`docs/tools/Component-Deploy.md` (the classes); and the test fixtures that assume 8080 or 8444.

## 4 Phases

| Phase | Deliverable |
|---|---|
| RC0 | The registry and reconcile (§3.1) with every listener started through it at boot and stopped through it at shutdown; states recorded with time and cause; the port check callable for one target. No change in behavior |
| RC1 | The conf shape (§3.2): `enabled` and written ports, the retired spellings refused, `Group::Listeners`, `set_value` running a group's hook, XSD and schema entries. Listeners still restart-class |
| RC2 | HTTP, HTTPS and gRPC live (§3.3, without the self-signed certificate) |
| RC3 | SSH live: start, stop, rebind, `access`, limits and sub-service flags (§3.4) |
| RC4 | `ListenerServices`, `LISTENER_CONTROL`, recent sign-in, the lockout guard, the deferred apply, audit, and the operator socket commands (§3.6, this member only) |
| RC5 | The screens (§3.7): Home's dialog, Conf › Services and ports, Needs attention, the Protocols and `InstanceSetting` changes |
| RC6 | The cluster port on `auto` and `lone-check-seconds` (§3.5) |
| RC7 | The defaults (§3.9) and the self-signed certificate (§3.3): confs, podman, `moq`, docs, fixtures, and the test that every optional listener is off by default (Sealed-Secrets S12) |
| RC8 | SMTP, XMPP, SIP, call media, TURN and the edge conf tunnels live |
| RC9 | Services carried on a listener (§3.8) |
| RC10 | `allMembers` and the member listener summary (§3.6) |

Order: RC0 to RC5 give the first usable slice (HTTP, HTTPS and SSH switched from Home and the terminal) with today's
defaults. RC6 and RC7 change the defaults and wait for it, since a server that opens only 8443 and 2222 needs the
switches to be set up. RC8 to RC10 can follow in any order.

## 5 Tests

Listeners do not exist under `moqui test`, so their behavior is checked by Rust integration tests in
`crates/moqui-host/tests/it/` and the screens and services by regression specs; nothing here needs an acceptance test.

| What | Where |
|---|---|
| Reconcile: the action for each wanted-against-running case, a failed start leaving the others, the order | unit tests beside `listeners/` |
| Conf: the retired spellings refused, each listener attribute's class, `set_value` running the hook, every runtime conf still passing the schema | `conf_schema`, `conf_reload` and `runtime_conf_tests.rs` |
| HTTP on then off on a running server: connects, then refused; a port change binds the new before closing the old; a busy port leaves the old running | `tests/it/listener_control.rs` |
| SSH off and on; each `access` mode's offered methods; a dialog session ended when the mode leaves `open` | `tests/it/ssh.rs` |
| A lone member has no cluster port; it opens when a second member starts (a short `lone-check-seconds`) and the joiner's first messages arrive after the clear-all | `tests/it/cluster_notify.rs` |
| The self-signed certificate is made once, serves a name with none, and gives way to an installed one | `tests/it/acme.rs`, `tls_tests.rs` |
| `set#Listener`: a holder and a non-holder, the reason required, the lockout and public HTTP refusals and their confirmations, `endNow`, the audit line, a setting the environment names | `runtime/components/base/framework/test/AdminListeners.spec.js`, with a stub registry |
| Home's button and dialog for a holder, and their absence without the permission, on Home's one html render; Services and ports rendered once | `runtime/components/base/tools/test/AdminOverview.spec.js`, a new `ServicesPorts.spec.js` |
| Operator socket: `listeners` and `listener <name> on|off` | `operator/socket_tests.rs` |
| Fixtures that assume 8080 or 8444 by default | `port_check_tests.rs`, `startup_report_tests.rs`, `test_cmd/serve.rs`, `tests/common/mod.rs`, `AdminProtocols.spec.js`, `SystemAttention.spec.js` |

## 6 Decisions

| Question | Choice | Why |
|---|---|---|
| A new production server with no certificate | A self-signed certificate until one is installed or set up with ACME (decided with the user, Nov 27) | 8443 answers at first start, so setup and health checks need no HTTP port |
| SSH default access | `open`: public to the TUI sign-in (decided with the user, Nov 27) | Today's `login-dialog` behavior, and the only password route for an account with a second factor |
| What "cert required" means for SSH | A registered public key (`UserSshKey`) (decided with the user, Nov 27) | It exists; OpenSSH user certificates are not in scope |
| Where a switch is kept, and its reach | This server's `runtime/.env`, with an opt-in "all members" (decided with the user, Nov 27) | It survives a restart, and the dialog, the conf, SIGHUP and the socket stay one mechanism |
| SMTP's written port | 2525; the edge and all-in-one confs set 25 (decided with the user, Nov 27) | Unprivileged, as 8443 and 2222 are, so turning it on works without the right to bind a low port |
| Which carried services are on by default | Today's: DAV, `/notws`, `/termws`, SFTP and git over SSH on; MCP, A2A, the gateway and webhooks off (decided with the user, Nov 27) | Only the listener defaults change; each of these is already decided per user by a permission |
| A timer on a switch | None (decided with the user, Nov 27) | Off until a person turns it on is the safe direction in an incident, and the log says who and why |
| How a lone member notices a second | The member table read every `lone-check-seconds`, 15 by default (decided with the user, Nov 27) | No extra database connection on a quiet single server; a deployment that adds members often lowers it |
| HTTP on a public address with no TLS and no proxy believed | Confirm, then warn under Needs attention (decided with the user, Nov 27) | A server behind a load balancer the conf does not know still works; a boot-time refusal stays Sealed-Secrets §3.8's |
| Connections when a listener closes | 5 s for requests in flight, and "End connections now" on the turn-off form (decided with the user, Nov 27) | An incident does not wait, and no listener gets another setting |
| How "off" is written | An `enabled` attribute beside a written port; the old spellings refused | One spelling, and a switch that keeps the port |
| Rebuild the router or check a flag | Check a flag per request (§3.8) | The router, guard and permits stay as built; a flag read is cheap |
| A new permission or `CONF_EDIT` | `LISTENER_CONTROL`, sealed | A responder switches services without the right to edit the conf |
| Detecting a cluster | The member table, read more often while alone | [archive/Cluster.md](archive/Cluster.md) rules out any discovery but the table; no extra database connection on a quiet single server |

## 7 Not in scope

- OpenSSH CA-signed user certificates.
- A timer that turns a service back on, and a Postgres NOTIFY for a faster cluster join (§6).
- HTTP/3, XMPP s2s, and any listener that does not exist today.
- Restarting a production server from the UI, and the settings that are restart-class for other reasons (datasources,
  keys, `instance@purpose`, worker pools).
- Background loops with no port (mail polling, the scheduler, telemetry export); Monitor › Settings and the recovery
  ladder cover the ones that can change today.
- The incident runbook itself ([Sealed-Secrets](Sealed-Secrets.md) §3.12, S15), which will name these switches.
