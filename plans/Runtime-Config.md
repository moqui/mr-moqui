# Runtime Config: services and ports on and off while running

Status 2026-11-27: reviewed against the code and being built, RC0 to RC10 in one run. Its open points were decided
with the user the same day (§6). The review's corrections are in place below, and the implementation design is §8.
Built so far: RC0.

Goal: an administrator turns services and ports on and off while the server runs, from a dialog on System Home, from
the terminal, or by editing the conf, and no listener setting waits for a restart. It serves two tasks: setting a
server up one service at a time, and an incident where some services go off for a while (HTTPS off with SSH left on,
or the reverse). With it the defaults shrink: a new server opens HTTPS (8443) and SSH (2222) and nothing else, and the
cluster port opens only when there is a cluster.

Sections: [1 Rules](#1-rules) · [2 What is there now](#2-what-is-there-now) · [3 Design](#3-design) ·
[4 Phases](#4-phases) · [5 Tests](#5-tests) · [6 Decisions](#6-decisions) · [7 Not in scope](#7-not-in-scope) ·
[8 Implementation design](#8-implementation-design)

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
| Listener classes | `LISTENER` and `BOOT` are restart classes: the webroot ports, `ssh-server`, `smtp-server`, `xmpp-server`, `sip-server`, `media-server`, `turn-server`, `cluster-member` (`BOOT`); `edge` and `endpoint[path]` have restart reasons of their own. `https-listen-port` and `http-bind` fall under the webapp's `Live` default though they are read once at listen | `conf_schema.rs` (`CATALOG`) |
| One setting, set live | `set_value` (`factory.set_conf_value`) patches one attribute of the running conf, whatever its class, and keeps it in `runtime/.env` (a secret in the seal store) or as a process property. It runs no hook, checks no retired value and writes no audit line. The DKIM key and the specs' `setConf` use it; edge pairing uses `conf_env::keep_in_env`. Readers through `conf_env::setting` and the reload merge rank a process property, then the OS environment, then `.env`, so a value the environment names is not held by a `.env` write past the next reload | `conf_reload/mod.rs`, `conf_env.rs` |
| How "off" is written | Differently for each: `http-listen-port="off"`, an empty `port` (SSH, SMTP, SIP, media), an empty `domain` (XMPP), `ssh-server@enabled`, and nothing at all for the cluster port | `runtime/components/base/framework/conf/MoquiDefaultConf.xml` |
| Default open ports | HTTP 8080 on every address, HTTPS 8443 when `runtime/cert` has a pair (a dev instance makes one), cluster 8444 on every address, always. SSH is off (`port=""`, host 127.0.0.1) | `MoquiDefaultConf.xml`, `lib.rs` (`listen_address`), `tls.rs` (`plan`) |
| Port check | Before the database opens every port is bound and released. HTTP, HTTPS, cluster and SSH are required (a busy one stops the boot); the rest are skipped when busy | `crates/moqui-host/src/port_check.rs` |
| SSH access | Read once in `start`: password and public key always offered, keyboard-interactive (the login dialog) when `login-dialog` is true, which lets anyone reach the TUI sign-in. No setting turns password auth off or requires a key. Keys, passwords and the `SSH_*` permissions are checked per connection | `crates/moqui-host/src/ssh/server.rs` |
| Cluster port | Bound whenever the process serves, whatever the transport and with no other member; a start without it fails ("http server has no cluster listener"). Members are `moqui.server.ClusterMember` rows; a new one is noticed at the next heartbeat (60 s by default) or the next cache-invalidating write. No event fires on a new row | `lib.rs`, `cluster_edge/listen.rs`, `crates/moqui-core/src/cluster/mod.rs`, [archive/Cluster.md](archive/Cluster.md) |
| System Home | The "Listeners and integrations" box lists `ec.factory.getStartupReport()` rows: what each listener did at startup. `StartupReport::record` replaces a row by name, so a later state can be recorded. Home has no write transition | `runtime/components/base/tools/screen/System/Home.xml`, `crates/moqui-core/src/startup.rs` |
| Conf screens | Conf › Edit saves a file and reloads (ADMIN and the sealed `CONF_EDIT`); Conf › Changes lists what waits for a restart; Protocols › Server configuration is read only; 41 `InstanceSetting` rows say `restartRequired="Y"` | `System/Conf/*.xml`, `crates/moqui-devkit/src/deploy/conf_edit.rs`, `framework/data/AdminConfigurationData.xml` |
| Ways in without a listener | The operator socket (seal commands only today; served by the seal gate, which the binary always runs) and SIGHUP (`systemctl reload`; its handler is registered inside `http::listen` and skipped when stdin is a terminal) | `crates/moqui-host/src/operator/`, `systemd/moqui.service` |
| Boot failures | A failed bind of SSH, the cluster port, HTTP or HTTPS stops the boot; gRPC, SMTP, call media, TURN, SIP and XMPP record a state and the boot goes on | `lib.rs`, `http.rs` (`fail`) |
| Closing | One `watch` stops every web loop. A listener's connections get `web-protection/connections@shutdown-timeout` (30 s) to finish. `/notws`, `/termws`, the call and XMPP websockets, SSE, A2A, MCP, the gateway and the edge and manage links close only on that process-wide signal | `http.rs` (`serve`), `notws.rs`, `termws.rs`, `llm/*` |
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
| Off, running | Stop: no new connections, 5 s for requests in flight (`LIVE_STOP_GRACE`; the process stop keeps `shutdown-timeout`), then its connections end; at once when the change asks for it (§3.6) |
| On, not running | Start. A failed bind is explained with the port check's holder text (`port_check::explain`); a separate bind and release first would only race the real one |
| Bind settings differ | Bind the new address first when it differs from the old, then stop the old; when the two overlap (the same port, a wildcard and a specific address) stop the old and bind again; a failed bind leaves the old one running, or binds it again |
| In-place settings differ | Swap them; new connections use them |

Each action records the listener's state with `startup::listener` (On, Off with the reason, Failed, Skipped), now with
the time and what caused it, so System Home shows the state now and not the state at startup. A failure is logged
naming the listener and reconcile goes on with the next entry. Skipped keeps its meaning (busy at the start check);
a listener that is on but lacks something it needs is Failed, naming what is missing.

Reconcile runs at boot, after a conf reload that changed a listener unit, after `set_value` on one, and when a signal
changes (a cluster member appears, ACME wants the listener). At boot it runs as four staged passes, since other boot
work sits between the listeners: `BeforeCluster` (SSH, whose fingerprint the member row carries), `ClusterPort` (after
`cluster::start`), `Comms` (call media, SIP, XMPP, SMTP) and `Web` (HTTP, HTTPS, gRPC, once `http::listen` has built
the router). Each pass is the same reconcile, so there is one code path. An entry is **live** once its phase is
built: until then a reconcile after boot leaves it alone, so its changes stay restart-class and listed in Conf ›
Changes; the flag and the attribute's conf class change in the same commit, and a test keeps them matched. The
registry sits behind an extension slot in `moqui-core`, as `web/protection_view.rs` does, so services in core and
devkit reach it without depending on the host. `ServeProcess` keeps owning the listeners; its `Drop` stops every
entry in today's order.

### 3.2 Conf shape

Every listener element gets `enabled` and a port with its number written in. One spelling of "off" replaces the
five there are now; the old spellings are retired and the boot refuses them by name (`refuse_retired`).

| Listener | Element | Switch | Default | Port default | Replaces |
|---|---|---|---|---|---|
| HTTP | `webapp[webroot]` | `http-listen-enabled` | false | `http-listen-port="8080"` | `http-listen-port="off"`, empty |
| HTTPS | `webapp[webroot]` | `https-listen-enabled` | true | `https-listen-port="8443"` | `https-listen-port="off"`, empty |
| gRPC | `webapp[webroot]` | `grpc-listen-enabled` | false | `grpc-listen-port="4317"` | `grpc-listen-port="off"` |
| SSH | `ssh-server` | `enabled` | true | `port="2222"`, `host="0.0.0.0"` | empty `port` |
| Cluster | `cluster-member` | `listen="auto"` (`auto`, `on`, `off`) | auto | `https-port="8444"` | always on |
| SMTP | `smtp-server` | `enabled` | false | `listen-port="2525"` | empty `listen-port` |
| XMPP | `xmpp-server` | `enabled` | false | `port="5222"`, `tls-port="5223"` | empty `domain` and ports |
| SIP | `sip-server` | `enabled` | false | `port="5060"`, `tls-port=""` | empty `port` |
| Call media | `media-server` | `enabled` | false | `port="10000"`, `tcp-port=""` | empty `port` |
| TURN | `media-server/turn-server` | `enabled` | false | `port="3478"`, `tls-port=""` | empty `port` |
| Edge tunnels | `edge` | by each port, as now | off | unchanged | unchanged |

The table gives the final defaults, which RC6 (the cluster) and RC7 (the rest) set. RC1 writes the switches and ports
with today's behavior: HTTP and gRPC on, SSH off on 127.0.0.1, the cluster port always on (§8.2). gRPC has a switch of
its own beside the services' `grpc-enabled`, because a service may be served over HTTPS with port 4317 closed (the dev
conf does so). SIP's and TURN's TLS ports are written empty, their usual numbers in the comment, because an
environment name cannot empty a written port and their TLS needs a certificate anyway; an empty plain port of SIP,
XMPP or TURN stays the way to leave that one transport off.

`webapp@https-enabled` keeps its meaning (the scheme of generated URLs) and is not the listener's switch. A listener
that is on but lacks something it needs (XMPP with no `domain`, SMTP with no `allowed-addresses`, SIP with call media
off) is recorded as Failed with what is missing, and the dialog asks for it. The written ports are unprivileged, as
8443 and 2222 are; the edge and all-in-one confs set SMTP's to 25, as they bind 443.

The retired spellings are refused by value: `http-listen-port`, `https-listen-port` and `grpc-listen-port` set to
`off` or empty, and an empty SSH `port`, SMTP `listen-port` or call media `port`. The CLI's and `moq`'s `-p off` and
`--https-port off` stay, as shorthand for the switch. `http-listen-port` no longer falls back to `http-port`. A
deployment that set a port without `enabled` (on, before) is warned at boot, naming the port, where it was set and the
switch, and the Off row on System Home says the same; it is not refused, since turning a listener off live writes
`enabled=false` and keeps the port.

The conf schema gets `Group::Listeners`. A listener's unit moves from `Restart(LISTENER)` or `BOOT` to
`Apply(Group::Listeners)` in the phase that makes it live, so Conf › Changes stops listing it as waiting; the host
registers one hook for the group that queues a reconcile. `https-listen-port` and `http-bind` get an explicit class.
`set_value` runs the hook of the group its attribute belongs to, once its lock is released, so a single setting set
from a service, the operator socket or a spec takes effect as a reload does; a change of several settings holds the
hook until the last. It refuses a retired value. Each attribute is added to the XSD and to the schema entry.

### 3.3 HTTP and HTTPS

`http::listen` binds each web listener through a loop like `standing_by`: wait until wanted, bind, serve until the
wanted settings change or the process stops, close, repeat. The router, guard, permits and TLS settings are built once
and shared, as now; only the socket comes and goes. The TLS material is built at boot whenever there is a web bind,
even with HTTPS off, so HTTPS can be turned on later; certificates already reload every 5 s, so they are not a bind
setting. Closing ends the listener's connections after the grace, including `/notws`, `/termws`, the call and XMPP
websockets, SSE, A2A, MCP, the gateway and the edge and manage links on that listener: each connection carries its
listener's close signal in the request's extensions, and those handlers wait on it instead of the process-wide one.
ACME's "wanted" becomes one input of the HTTPS entry, and `standing_by` goes. On an edge-role node the edge acceptor
is made only at boot, so HTTPS turned on live serves but takes edge links after a restart, and the answer says so.

A failed HTTP or HTTPS bind at boot no longer stops the boot (§6): it is a Failed row on System Home and in Needs
attention, and the port check treats both as optional. SSH follows in RC3 and the cluster port in RC6.

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
serve into the same intake, so turning SSH off while either is in use is refused by `set#Listener` and the operator
socket with the reason (or confirmed, §3.6); a conf reload that turns it off goes ahead and logs a warning naming
them. The intake (its counts and settings) lives as long as the server and each bind gets its own accept loop, so a
rebind keeps it. A start or rebind sets the fingerprint and address properties again and rewrites this member's row,
which carries them. A failed SSH bind at boot is a Failed row and the boot goes on (§6).

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
never (for `transport="ssh"` or `notify`). It and `lone-check-seconds` come in RC6, the first phase that reads them.

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
check treats the cluster port as optional in every mode (a busy port, at boot or when it opens later, is a Failed row
on Home and a Needs attention item, and the boot goes on). Nothing may use a cluster port of 0 to mean "reached in
process only" any more (RC0 moved the Cluster row from `record_web_listeners`, now gone, to the cluster entry). The startup report says "off: no other member" for a lone member. A forged `ClusterMember` row
with copied fingerprints can make a member open the port; what it exposes is the mTLS handshake pinned to the shared
certificate, as today (SEC-CLU-89 in [Security-Review-Low](Security-Review-Low.md)).

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

Kept where: `persist` writes `runtime/.env` on this server, which the conf merge reads at the next start, and clears a
process property of the same name that would shadow it. A setting the OS environment names cannot be kept there: the
change is set as a process property, which outranks the environment in readers and in a reload, so it holds until the
restart, and the answer says so. The answer names, for each setting, where it is kept (`runtime/.env`, the process
until a restart, the seal store), what outranks it, and whether it applies now or at the next start. Listener readers
read the factory's running conf, not `conf_env::setting`, so two servers in one test process do not share a value.

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
(`/notws`, `/termws`), `dav-server@enabled`, the git server, `mcp@server-enabled`, `a2a@enabled`,
`gateway@enabled`, `channels@webhooks-enabled`, `xmpp-server@websocket-enabled`, and the OTLP receiver. Each route
stays mounted and checks its flag on each request, from state built from the conf and built again after a reload; off
answers 404 and ends its open sockets. The phase starts with an audit of every such switch (its reader and its class)
and ends with each `Live` and listed on Services and ports under its listener.

### 3.9 Defaults

`MoquiDefaultConf.xml` opens HTTPS on 8443 and SSH on 2222 (every address, `access="open"`) and nothing else: HTTP
off, the cluster port `auto`, every optional listener off. So `MoquiProductionConf.xml` needs no listener line, and a
server is set up by turning on what it needs, in the conf beforehand or live. The services carried on a listener
(§3.4, §3.8) keep today's defaults: DAV, `/notws`, `/termws`, SFTP and git over SSH on, each still decided per user
by its permission; MCP, A2A, the gateway and webhooks off.

What follows the change: `MoquiDevConf.xml` drops the lines that become the default but keeps
`<ssh-server host="127.0.0.1"/>`, so a dev instance stays local; the desktop node's listen properties turn SSH off
and keep HTTP on; the topology confs whose nodes take plain HTTP from a load balancer or front node turn it on; `podman/` publishes 8443 and 2222 and its health
checks ask `https://127.0.0.1:8443/health`; `scripts/moq` port flags; `docs/Run-Deploy.md` (the ports table, the
firewall and cluster steps), `docs/security/Attack-Surface.md` and `Attack-Surface-Cluster-Edge.md`,
`docs/tools/Component-Deploy.md` (the classes); and the test fixtures that assume 8080 or 8444.

## 4 Phases

| Phase | Deliverable |
|---|---|
| RC0 | The registry and reconcile (§3.1) with every listener but the edge conf tunnels started through it at boot (four staged passes) and stopped through it at shutdown; states recorded with time and cause; the port check callable for one target. No change in behavior |
| RC1 | The conf shape (§3.2) with today's defaults: `enabled` and written ports, the retired values refused, the upgrade warning, `Group::Listeners`, `set_value` running a group's hook and keeping a value the environment names until the restart, XSD and schema entries, every reader of the old spellings. Listeners still restart-class |
| RC2 | HTTP, HTTPS and gRPC live (§3.3, without the self-signed certificate); a failed web bind at boot no longer stops it |
| RC3 | SSH live: start, stop, rebind, `access`, limits and sub-service flags (§3.4); a failed SSH bind at boot no longer stops it |
| RC4 | `ListenerServices`, `LISTENER_CONTROL`, recent sign-in, the lockout guard, the deferred apply, audit, and the operator socket commands (§3.6, this member only) |
| RC5 | The screens (§3.7): Home's dialog, Conf › Services and ports, Needs attention, the Protocols and `InstanceSetting` changes |
| RC6 | The cluster port on `auto` and `lone-check-seconds` (§3.5) |
| RC7 | The defaults (§3.9) and the self-signed certificate (§3.3): confs, podman, `moq`, docs, fixtures, and the test that every optional listener is off by default (Sealed-Secrets S12) |
| RC8 | SMTP, XMPP, SIP, call media, TURN and the edge conf tunnels live (TURN split out of call media and the conf tunnels out of `edge_link::start`; call media and SIP stop before they start again, as their engines are process-wide) |
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
| A listener that cannot bind at boot | A Failed row on System Home and in Needs attention, and the boot goes on, for HTTP, HTTPS, SSH and the cluster port as for the others; each changes in the phase that makes it live (decided with the user, Nov 27) | §1's failure tolerance; the operator socket and SIGHUP stay as ways back |
| How the work runs | RC0 to RC10 in one run, a commit per phase on master (decided with the user, Nov 27) | |
| gRPC's switch | `grpc-listen-enabled`, beside the services' own `grpc-enabled` | A service can be served over HTTPS with port 4317 closed |
| SIP's and TURN's TLS ports | Written empty, their numbers in the comment | An environment name cannot empty a written port; their TLS needs a certificate |
| A listener on but missing a setting | Failed, naming what is missing | Skipped already means busy at the start check |
| A port set without `enabled` (on, before) | A warning at boot and in the Off row, no refusal | A live turn-off writes `enabled=false` and keeps the port |
| A value the OS environment names | Set as a process property, held until the restart | A process property outranks the environment in readers and in a reload; `.env` would not |

## 7 Not in scope

- OpenSSH CA-signed user certificates.
- A timer that turns a service back on, and a Postgres NOTIFY for a faster cluster join (§6).
- HTTP/3, XMPP s2s, and any listener that does not exist today.
- Restarting a production server from the UI, and the settings that are restart-class for other reasons (datasources,
  keys, `instance@purpose`, worker pools).
- Background loops with no port (mail polling, the scheduler, telemetry export); Monitor › Settings and the recovery
  ladder cover the ones that can change today.
- The incident runbook itself ([Sealed-Secrets](Sealed-Secrets.md) §3.12, S15), which will name these switches.

## 8 Implementation design

From the review of 2026-11-27, checked against the code. Line numbers drift; the names are what to search for.

### 8.1 The registry (RC0)

**Core slot**, `crates/moqui-core/src/listeners.rs`, per factory as `web/protection_view.rs` is (tests run several
servers in one process, so not process-wide):

- `trait ListenerControl: Send + Sync { fn listeners(&self) -> Vec<ListenerInfo>; fn reconcile(&self, ReconcileRequest, wait: Duration) -> Result<Vec<Outcome>, String>; fn reconcile_later(&self, ReconcileRequest, delay: Duration); }`
  with `install(factory, Arc<dyn ListenerControl>)` and `control(factory) -> Option<Arc<dyn ListenerControl>>`.
- `Cause { Boot, Shutdown, ConfReload, SetValue(String), Signal(&'static str), Request { user, reason } }`, shown
  with `Display`.
- `ListenerInfo { name, state: startup::State, since, cause, address, live, pending, last_error, connections, dependents, selectors }`.
- `ReconcileRequest { cause, end_now: Vec<String> }`; `Outcome { name, action (same, started, stopped, rebound, applied, held, failed), detail }`.
- `SWITCHES: [(report name, enabled selector, port selectors)]` (RC1), which the upgrade warning, the protocol facts,
  `test_cmd/serve.rs`, the services and the default-surface test read.
- Which listener a request or SSH session came over belongs to the request (`WebRequest.listener`, RC4), not to
  this trait.

**Host registry**, `crates/moqui-host/src/listeners/` (`mod.rs`, `entry.rs`, `worker.rs`, `control.rs`, an entry
file each for `ssh`, `cluster`, `comms`, `web`, and `tests.rs`), gated as `boot_server_factory` is:

- `trait Listener { type Bind: Clone + PartialEq + Debug + Send; type InPlace: Clone + PartialEq + Debug + Send; type Run: Running<Self::InPlace>; fn name; fn stage -> Stage; fn stop_phase -> StopPhase; fn live() -> bool { false }; fn fatal_at_boot() -> bool { false }; fn selectors() -> &'static [&'static str]; fn wanted(&Ctx) -> Wanted<Bind, InPlace>; fn start(&Ctx, &Bind, &InPlace) -> Result<Option<Run>, String>; fn overlaps(&old, &new) -> bool; fn dependents(&Ctx) -> Vec<String>; fn target(&Bind) -> Option<port_check::Target> }`.
  `start` answering `Ok(None)` means a wrapped start of today found it off and recorded why itself (RC0 wrappers only).
- `Wanted { On { bind, in_place }, Off(reason), Skipped(reason), NotYet }`; `NotYet` is a web entry before the router
  exists.
- `trait Running<P>: Send { fn state; fn address; fn apply(&mut self, &P) {}; fn begin_stop(&mut self, StopHow); fn finished(&mut self, wait) -> bool; fn connections -> Option<usize> }`,
  `StopHow { Process, Grace(Duration), Now }`; `begin_stop` never waits out the grace.
- A generic `Slot<L>` holds the wanted-against-running diff once, tested with fake listeners, behind an object-safe
  `Entry` trait.
- `Registry(Arc<Inner>)`: `entries: Mutex<Vec<Box<dyn Entry>>>` held for a whole pass (the one lock), a `snapshot` of
  `ListenerInfo` published after each action so readers never wait on a pass, `web: OnceLock<WebContext>`,
  `shutting_down`. Methods `run(Scope, Cause)`, `install_web`, `request` (returns a receiver of the outcomes),
  `stop(StopPhase)`, `close`, `address(name)`, `snapshot`.
- `Ctx { factory, boot: BootInputs { bind, runtime_dir, cluster_posts }, web: Option<&WebContext>, cause }`. Entries
  read settings with `conf_env::setting_on(factory, …)` in RC0 and the factory's running conf (`factory.conf_value`
  of the keyed selector) from RC1 on, never the process-wide `conf_env::setting`.

**Threads.** `run` is called only on a plain thread with no ExecutionContext entered: the boot thread, a
`spawn_blocking` thread inside `http::listen` for the web pass, or the `moqui-listeners` worker. A conf hook may fire on
a thread that holds a transaction, so hooks, signals and services only enqueue; the worker merges what is waiting into
one pass (causes merged, `end_now` joined, deferred requests held until due) and answers each request that asked.
Async binds go through `on_runtime(handle, future)` (spawn plus a std channel, as `edge::block_on`), never
`Handle::block_on`. A `Running` is dropped only on a plain thread: the Drops of `ClusterEdge` (it blocks), `XmppNode`
and `SipNode` (they drop their own runtimes) panic or stall on an async thread. `stop(StopPhase::Web)` sets
`shutting_down` under the lock first, so a reload during the drain cannot start HTTP again.

**Live flag.** Every entry's `live()` is false in RC0. Boot passes and shutdown stops act on every entry. Any other
pass skips a non-live entry: it computes `wanted` only to fill `ListenerInfo.pending` ("waits for a restart") and
starts and stops nothing. A test `live_matches_the_conf_schema` (from RC1) checks that an entry's selectors are all
`Apply(Group::Listeners)` exactly when it is live.

**Boot passes and Drop.** `Listeners::All(ports, bool)` becomes `Listeners::All(ports, ListenBind)` (the cluster port
when the bind is not `None`). After the request pool and pass sender (after the empty-database re-boot, so the registry
belongs to the final factory): `run(Stage(BeforeCluster))?` (SSH, fatal in RC0), today's cluster notify,
`cluster::start` and workers, `run(Stage(ClusterPort))?` (fatal in RC0), today's installs, `run(Stage(Comms))` (call
media with TURN, SIP, XMPP, SMTP; never fails). `http::listen` builds the web context, `install_web`, then
`spawn_blocking(run(Stage(Web)))`. `ServeProcess` replaces `sshd`, `cluster_edge`, `rtc`, `sip`, `xmpp` and `smtpd`
with `listeners: Option<Registry>`; its `Drop` keeps today's order: join the worker, `stop(Web)`,
`runtime.shutdown_timeout(5s)`, `stop(BeforeFactory)` (XMPP, SIP, call media), `factory.shutdown()`, pass sender, pdf,
`stop(AfterFactory)` (SMTP, SSH, cluster), the notify listener, then the rest.

**Startup rows.** `startup::Entry` gains `since` (the time `record` stamps) and `cause` (`set_cause`, which the
registry calls after each action, so today's recorders need no change). `to_value` adds them after `name`, `state` and
`detail`; consumers read only those three.

**Port check.** `port_check::check_one(&Target)` (the services' pre-check) and `explain(&Target, &io::Error)` (the
port, property, host and holder text, factored out of `check`). The process-wide `BUSY` list is read only for
`Cause::Boot`.

**RC0 entries.** SSH wraps `ssh::server::start`; the cluster entry wraps `cluster_edge::listen` (Off, "reached in
process only", when the bind is `None`); call media, SIP, XMPP and SMTP wrap today's starts (`Bind = ()`; `start_smtp`
moves into `comms.rs`); HTTP, HTTPS (standby included) and gRPC wrap today's binds in `http::listen`, from a
`WebContext` (the axum handle, `WebApp`, permits, the TLS acceptor, the process-wide shutdown receiver, limits, the
HTTPS plan). The edge conf tunnels stay inside `edge_link::start` until RC8.

### 8.2 The conf shape (RC1) and the defaults (RC6, RC7)

| Listener | Attributes | RC1 default | Later |
|---|---|---|---|
| HTTP | `webapp[webroot]@http-listen-enabled`, `@http-listen-port` | true, 8080 | RC7 false |
| HTTPS | `@https-listen-enabled`, `@https-listen-port` | true, 8443 | |
| gRPC | `@grpc-listen-enabled`, `@grpc-listen-port` | true, 4317 | RC7 false |
| SSH | `ssh-server@enabled`, `@port`, `@host` | false, 2222, 127.0.0.1 | RC7 true, 0.0.0.0 |
| Cluster | `cluster-member@https-port` (empty still means 8444) | 8444 | RC6 `listen="auto"`, `lone-check-seconds="15"` |
| SMTP | `smtp-server@enabled`, `@listen-port` | false, 2525 | |
| XMPP | `xmpp-server@enabled`, `@port`, `@tls-port` | false, 5222, 5223 | |
| SIP | `sip-server@enabled`, `@port`, `@tls-port` | false, 5060, empty | |
| Call media | `media-server@enabled`, `@port`, `@tcp-port` | false, 10000, empty | |
| TURN | `media-server/turn-server@enabled`, `@port`, `@tls-port` | false, 3478, empty | |

A switch is compared trimmed and case-insensitively with `true`. RC1's conf edits keep each server as it is: the dev
conf sets `http-listen-enabled="false" grpc-listen-enabled="false"` and `<ssh-server enabled="true"/>`; the load-test
and dev-group confs turn SSH on; the edge, edge-cluster, full-stack, multi-domain and all-in-one confs turn call media
and TURN on and list `smtp_server__enabled` and `sip_server__enabled` among their `.env` names; the basic-comms test
conf turns call media on at port 0. Test servers turn HTTPS off unless the test names it (`tests/common`
`TEST_LISTENERS_OFF`, `test_cmd/serve.rs`), since an Address bind no longer means no HTTPS; RC7 adds SSH there.

**Retired values.** `conf_env::RETIRED_VALUES: [(selector, values, replacement, hint)]` and
`retired_value(path, value)`, shared by `refuse_retired` (which runs on the merged conf, so environment names are
caught too) and `set_value`. RC3 adds `ssh-server@login-dialog` → `access` to `RETIRED_ATTRIBUTES` (true is `open`,
false is `login`). The XSD gets a digits-only type for a single port and digits-or-empty for a transport's port, and
`runtime_conf_tests` runs `refuse_retired` over every runtime conf.

**Upgrade warning.** At boot, after `refuse_retired`, for each `SWITCHES` entry: when it is off, a port selector was
set by the deployment (an override, or a conf file other than `MoquiDefaultConf.xml`) and `enabled` was not, log a
warning naming the port, its source and the switch, and keep it for the Off row. The boot overrides (name, source,
selector) are kept in a factory extension, which RC4's "where each value comes from" reads as well. The warning is
removed after one release.

**`set_value`.** `conf_reload::set(factory, selector, value, persist) -> SetOutcome { selector, env_name, previous,
before_kept, kept: DotEnv | SealStore | UntilRestart, outranked_by, class, problems }`; `set_value` stays as a wrapper
for `setConf`. With `persist`: when the OS environment names the attribute, a process property only (`UntilRestart`,
outranked by nothing until the restart, then by the environment); otherwise `.env`, clearing or setting a process
property of that name, and reporting a `-D` from the start as what outranks it at the next start. It refuses a retired
value. The `.env` value it reports as before is read from the file, not the table loaded at start. After `replace_conf`
it releases the `Applying` lock, then runs the hooks of the attribute's group (`conf_schema::attribute_class`, which
walks `CATALOG` as `diff.rs` does) unless the caller holds them for a change of several settings. Tests inject the
environment lookup rather than set environment variables.

**Readers.** Each family's readers move with its default and XSD change:

- Web: `listen_address` (a Property bind gives the port when enabled; an Address bind is unchanged), `tls::plan`,
  `planned_port` and `listen_port` (no `use_default_port`; HTTPS is required in the port check when enabled and a
  certificate exists or the instance is dev), `grpc_listen`, `port_check`, core `web/mod.rs` `local_web_ports`,
  `edge/ports.rs` (reserve both the set port and the usual one), the edge and ACME messages, `usage.rs` and its help,
  `scripts/moq` (`build.sh`, `test.sh`, `core.sh`), the desktop node's `listen_properties`.
- SSH and SMTP: `ssh/server.rs` listen targets, the Off reasons in `lib.rs`, `moqui-dav` (the SFTP link),
  `moqui-git` (`SshAddress`), the core cluster message, `smtp.rs` `listen_target_by`, `edge_link/offer.rs`,
  `edge/setup.rs`, `protocol_admin.rs` (the `enabled` attributes, targets gated on them), `ConfigurationProtocols.js`,
  mr-mantle `SmithyGit.js`.
- Comms: XMPP is on when enabled with a domain (enabled with none is Failed); `moqui_core::comms::xmpp_domain(factory)`
  answers None unless enabled and replaces the domain test in `xmpp_off.rs`, `http.rs`, `edge_link/tunnel.rs`,
  `probe.rs`, `acme/services.rs`, core `edge/claims.rs` and `service/edge.rs`; the screens and scripts that test the
  domain (`Maps/Browse.xml`, `EdgeNode/Pair/Discover.xml`, `Status.xml`, `LocationShares.js`, `XmppServices.xml`)
  test `enabled` too. SIP, call media and TURN settings and their `_off` stubs; their `listen_targets(conf)` read the
  merged conf, as SSH's and SMTP's do. `Calls.js`, `Voice.js`, `SipDevices.js` and the Voice texts.
- Data and docs: `protocol.*.enabled` `InstanceSetting` rows (restart-required until the listener is live), the XSD
  documentation, Run-Deploy's ports and settings tables with an upgrade note, the pre-production skill's upgrade
  list, Sealed-Secrets §3.9 ("until it is enabled"), mr-l10n strings that quote `media-server@port`.

**Test fixtures.** A one-off rewrite over `crates/moqui-host/tests/` puts an `…__enabled` "true" tuple beside each
`ssh_server__port`, `smtp_server__listen_port`, `media_server__port`, `turn_server__*`, `sip_server__*` and
`xmpp_server__domain` tuple, and turns `…_listen_port` "off" into `…_listen_enabled` "false"; the XMPP, SIP and TURN
tests set unwanted written ports to 0 by hand. RC6: members and the edge-cluster tests set `listen=on`, one test runs
`auto` with `lone_check_seconds=1`, the desktop sets `listen=off`. RC7: SSH off in `TEST_LISTENERS_OFF`, the spawned
servers of `server_restart.rs` and `skeleton.rs`, and each direct `start_server` caller checked.

**RC7.** The default conf and XSD defaults; a self-signed certificate (`tls::ensure_self_signed`, used for a name with
none of its own, given way to by an installed or ACME certificate, ignored by edge pairing and the expiry source,
under Needs attention); the dev conf keeps `<ssh-server host="127.0.0.1"/>`; the pass authority conf keeps 4317 if it
needs it; podman publishes 8443 and 2222 and checks `https://127.0.0.1:8443/health`; the desktop turns SSH off; the
docs of §3.9 plus `docs/Build-Test.md` and the V12 proof page; the test `the_default_conf_opens_only_https_and_ssh` in
`crates/moqui-devkit/src/runtime_conf_tests.rs` (Sealed-Secrets S12).

### 8.3 HTTP, HTTPS and gRPC live (RC2)

Binds: HTTP a `SocketAddr`; HTTPS `(SocketAddr, Serving | AcmeOnly)`, wanted when enabled and a certificate is there
or ACME wants it (a task turns changes of `acme.wanted()` into a `Signal("ACME")` request); gRPC `{ addr, tls, otlp,
pass }`, its routes built at each start. `tls::plan` splits into the material (built once) and the bind.
`serve(listener, tls, web, permits, stop, ended)`: `stop` yields the grace (`shutdown-timeout` for the process,
`LIVE_STOP_GRACE` or 0 for a live stop); after it `ended` fires and each connection task drops its connection, the
edge and manage links included. `Peer` gains the listener's name and its `closing` receiver, put into the request as
`OnListener`; `http::closing_of(&extensions, &state)` answers it, or the process-wide signal for a connection in
process, and the ten handlers that wait on `state.shutdown` use it. `Server::address()` reads the current address from
an `Addresses` extension; `https_address()` and a new `listener_address(name)` too.

### 8.4 SSH live (RC3)

`Bind { host, port }`, the host key read at each bind. `InPlace = SshLive { access, limits, git_password_auth,
git_any_account, sftp_drives, tui, sftp, git, shell }` in an `ArcSwap` on the intake; the session maxima become
atomics. Three russh configs are made at start, one per `access` (`{public key, keyboard-interactive, password}`,
`{public key, password}`, `{public key}`); a connection takes the one for the access it sees at accept, and the auth
handlers check it too (the cluster's key works in every mode). A `dialog_off` watch ends the sessions still at the
sign-in dialog when access leaves `open`. Each accept loop has its own `ended` signal; edge tunnel streams use the
intake's. A start or rebind sets `SSH_FINGERPRINT_PROPERTY` and `SSH_ADDRESS_PROPERTY` and calls a new
`moqui_core::cluster::refresh_member(factory)`.

### 8.5 How it is built

Each batch is implemented by one agent and reviewed by another before it is fixed and its phase committed; while
working only the batch's own tests run, and the regression suites run once at the end. Batches: RC0-A (the core slot,
the startup fields, the port check pieces, the registry with its unit tests, not wired), RC0-B (the SSH, cluster and
comms entries, the staged boot, `ServeProcess`), RC0-C (the web entries, `http::listen`); RC1-a (the core mechanism),
RC1-b (web), RC1-c (SSH and SMTP), RC1-d (XMPP, SIP, call media, TURN), RC1-e (the warning, `SWITCHES`, data, docs);
RC2; RC3; RC4-a (the services), RC4-b (the operator socket); RC5; RC6; RC7-a (the certificate), RC7-b (the defaults);
RC8-a (SMTP and the edge conf tunnels), RC8-b (XMPP), RC8-c (call media, TURN, SIP); RC9; RC10.
