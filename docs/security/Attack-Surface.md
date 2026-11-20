# Attack surface of mr-moqui

This is a map of the HTTP routes, screens, REST, WebSockets, and the other listeners in this repository. It is for operators, security researchers, and review of this repository. It records what can be reached. It is not a vulnerability report, and it is not a WAF. Proof of the controls lives in [Proof-Tests.md](Proof-Tests.md).

The map is in several files, one for each area: the [table below](#the-files). The framework's files come first. Application components (mr-boss, mr-pass, and the Mantle services they reach) have their own two files; an application's surface is recorded there, never in a framework file.

To report an issue that should stay private until it is fixed, write to [moqui-board@googlegroups.com](mailto:moqui-board@googlegroups.com). The policy is in [SECURITY.md](../../SECURITY.md). Before 0.1.0 findings are recorded openly, in [Security-Review](../../plans/Security-Review.md).

## The files

| File | Area | Holds |
| --- | --- | --- |
| [Attack-Surface-Web.md](Attack-Surface-Web.md) | Web authentication and controls | How a request is signed in and authorized, parent frames, the session cookie, CSRF, lockout, tarpit, headers, CORS, the client address, web protection, the HTTP and HTTPS listeners, ACME |
| [Attack-Surface-Routes.md](Attack-Surface-Routes.md) | Routes, screens and REST | The host routes and servlets (`/health`, `/metrics`, the sockets, the proxies), the root screen tree and its public paths, token routes, `/rest` (with the `vault` root) |
| [Attack-Surface-Admin.md](Attack-Surface-Admin.md) | Administration services | Privileged groups, `org.moqui.admin.*`: access, sessions, shared devices, configuration, sign-in providers, connections, messages, jobs, imports and exports, backups, audit |
| [Attack-Surface-Tools.md](Attack-Surface-Tools.md) | Tools, System, Monitor and Dashboards | The Tools and System apps, load runs, component deploy and conf reload, Monitor, Dashboards and CalcGrid grants, jobs |
| [Attack-Surface-AI.md](Attack-Surface-AI.md) | LLM, A2A, MCP and Assist | `/a2a`, `/mcp`, `/llm/v1`, Assist, the agent permissions |
| [Attack-Surface-Git.md](Attack-Surface-Git.md) | Git hosting and pipelines | `/git`, Git LFS, `/ci/runner`, pipelines |
| [Attack-Surface-Mail.md](Attack-Surface-Mail.md) | Mail | Inbound SMTP, outbound mail, mailbox sync |
| [Attack-Surface-Files.md](Attack-Surface-Files.md) | DAV and SFTP | `/dav`, SFTP |
| [Attack-Surface-SSH.md](Attack-Surface-SSH.md) | SSH and the terminal | The SSH listener, the admin shell, the TUI, terminal sessions |
| [Attack-Surface-Comms.md](Attack-Surface-Comms.md) | Calls, chat and messaging | Call media, call links and guests, captions, recording, TURN, SIP, XMPP, voicemail, chat bridges, texts |
| [Attack-Surface-Cluster-Edge.md](Attack-Surface-Cluster-Edge.md) | Cluster and edge node | The cluster listener, the edge node and its link |
| [Attack-Surface-Clients.md](Attack-Surface-Clients.md) | Clients | The desktop and mobile apps |
| [Attack-Surface-Data.md](Attack-Surface-Data.md) | Data and stored secrets | The database, field encryption, where secrets are kept (the sealed start), the threat model for stored secrets and vaults ([Vaults](Vaults.md) has the vaults' design) |
| [Attack-Surface-Outbound.md](Attack-Surface-Outbound.md) | Outbound requests | Requests the server itself makes |
| [Attack-Surface-Process.md](Attack-Surface-Process.md) | Processes and build | Child processes, the operator socket, local secret files, the startup port check, build features |
| [Attack-Surface-Scripts.md](Attack-Surface-Scripts.md) | Scripts, templates and parsers | What a server script can reach, who can author code that runs, rendering and escaping, parsers of untrusted input |
| [Attack-Surface-Dev.md](Attack-Surface-Dev.md) | Development tools | What loads the dev components, Dev AIDE, the `aide` REST, the demo lock-down, test-only hooks |
| [Attack-Surface-Apps-BOSS.md](Attack-Surface-Apps-BOSS.md) | Staff applications | mr-boss and the Mantle services it reaches |
| [Attack-Surface-Apps-PASS.md](Attack-Surface-Apps-PASS.md) | Public applications | mr-pass: how its pages reach the authority, public accounts, sites and members |
| [Attack-Surface-Apps-PASS-Commerce.md](Attack-Surface-Apps-PASS-Commerce.md) | PASS commerce and portals | Shop, Billing, Supplier, Support, PASS Sales and PASS Marketing, and the Mantle services behind them |
| [Attack-Surface-Apps-PASS-Forum-Smithy.md](Attack-Surface-Apps-PASS-Forum-Smithy.md) | PASS Forum and Smithy | The Forum and public git hosting, and the Mantle services behind them |

## How to read this

A request goes from the client, through a reverse proxy or TLS, to hyper and axum in `crates/moqui-host/src/http.rs`, then screen dispatch, artifact authorization, and on to the database, search, mail, and jobs.

This server terminates TLS itself with rustls (`crates/moqui-host/src/tls.rs`) when the HTTPS listener is started. It is still meant to run behind a proxy. `webapp-list/webapp[webroot]@https-enabled` defaults to false and is the flag generated URLs use when they pick https. Starting the listener is the separate rule in [HTTP container](Attack-Surface-Web.md#http-container).

**Defaults** are `runtime/components/base/framework/conf/MoquiDefaultConf.xml`. **On** and **off** mean those defaults. Rows say what untrusted input can reach, and what to keep in mind during a review. Designed admin tools are privileged on purpose.

`./moq run` merges `runtime/conf/MoquiDevConf.xml` on top of that file. The dev file sets `instance@purpose` to `dev`, `webapp-list/webapp[webroot]@allow-origins` to `localhost,127.0.0.1` (this machine on any port), and `ssh-server@port` to `2222`, and it sets `tarpit-enabled` false for screens, screen transitions, and services. It does not declare an H2 console. The Jackrabbit repository element in it is commented out. The tables below stay on the default conf unless a row says otherwise.

## Heavy lock-down

A production install keeps the default conf's closed switches and does not publish the listeners that are on.

- Do not publish the HTTP port. Empty `webapp-list/webapp[webroot]@http-bind` is every address, and empty `webapp-list/webapp[webroot]@http-port` is 8080.
- Seal the server with a method other than `dev` (`moqui operator init --method tpm`, or `passphrase`, `shamir`, `kms`, `transit`), give the recovery shares to different holders, and keep the recovery bundle (`moqui operator export-seal`) apart from them ([Sealed start](../Run-Deploy.md#sealed-start)). A production instance refuses the `dev` seal, whose `seal.json` holds the root key in the clear. The crypt pass, the session cookie key and the other generated secrets are made into `runtime/db/seal.json` and the `SecretValue` table when the conf leaves them at `CHANGEME`, never into `runtime/.env`; a secret an earlier build left there is ignored once the runtime has a seal other than `dev` ([Secrets](Attack-Surface-Data.md#secrets)). `entity-facade@crypt-pass-old` stays the sentinel on purpose. Add operators to the `moqui-operator` group, never the `moqui` user.
- Do not load demo users.
- Leave tarpits on. The dev conf turns them off for screens, transitions, and services.
- Leave `webapp-list/webapp[webroot]@allow-origins` empty. The dev conf names localhost and 127.0.0.1. A foreign origin is then refused. Own origin stays allowed.
- Leave SSH off unless you want it. The default port is empty. The dev conf sets `2222` on `127.0.0.1`.
- Leave A2A, MCP, and the gateway off unless a client is configured. The model facade can stay on without those servers.
- Leave git `allow-http` false, and DAV `account-password` false.
- The cluster listener binds every interface on 8444 and requires a client certificate. Keep it on a private network.
- Leave inbound SMTP off unless a gateway and a non-empty allow-list are in place. An empty allow-list does not start the listener.
- Per-client web protection is off until `webapp-list/webapp[webroot]@trusted-proxies` is set or `webapp-list/webapp[webroot]/web-protection@enabled` is `true`. Set the proxy list when a reverse proxy is in front, so the client address and the per-client rules see the client rather than the proxy.

## Public demo server

A public demo breaks one rule above on purpose: it loads the demo users, and anyone can log in as `john.doe`, an
`ADMIN`. The demo component (`runtime/components/dev/demo`, loaded by `runtime/conf/MoquiDemoConf.xml`) narrows what
that login reaches:

- Tools' SQL, Data Load, Export and Snapshots, Database Speed Test, Activity, Queries and Connections, Service Run
  and Load testing (`Tools/Load`) are replaced by a screen that says they are disabled. Activity and Queries show every
  session's statements. Plans, jobs, maintenance and resets are refused by the VIEW-only authorization and the missing
  `DATABASE_ADMIN`.
- `ADMIN` is VIEW-only on Tools, System (Monitor with it), `/moqui`, EntitySync and SystemMessage receive, `/resources` (so refused: `run` is not a view verb), git repositories,
  report git sync and report webhooks. It loses `SearchRemote`, `DashboardRemote`, `ADMIN_PASSWORD`, `SSH_SFTP`,
  `SSH_GIT`, `MONITOR_INGEST`, `MONITOR_RECOVER`, `MONITOR_RECOVER_ADMIN`, `MONITOR_SETTINGS` and
  `REPORT_SAVE_LOCATION`, the managing Admin permissions (`ADMIN_ACCESS_MANAGE`, `ADMIN_IDENTITY_MANAGE`, backups,
  connections, transfers, settings, operations), the agent, chat bridge, voice and CI administration, the monitor's
  eval runs, replays and private-network setting, and `EMAIL_ACCOUNT_HOSTED`; Data Load and the Dev AIDE projects are
  VIEW-only. `john.doe` leaves `ADMIN_ADV`. What the lock-down leaves in place is the allow-list of the demo's
  `Demo.spec.js`, listed in [Attack-Surface-Dev](Attack-Surface-Dev.md#demo-component).
- SSH (`ssh-server enabled="false"`), DAV, the MCP server and client, A2A both ways and the gateway are off, and mail
  connects only to `localhost`.
- Password changes and resets, sign-in factors, and username, disabled and require-password-change changes are refused
  for the shared login, so one visitor cannot lock the others out.

Visitors can still change any business data, and `john.doe` can view every entity in Tools. Restore the database on a
schedule, and keep real keys out of it. Its README has the rest.

## How to use this for review

Walk each surface in the deployment you have: is it enabled, is it reachable from the networks you care about, how does it authenticate, what authorization (including inherit) applies, and what untrusted input does it accept. Application components add screens, services, and REST roots, and they need their own pass.

This file maps the surface. It does not prove the controls. The proof catalog is [Proof-Tests.md](Proof-Tests.md). The client suite those tests run is `runtime/components/base/framework/test/client/`. Reporting is in [SECURITY.md](../../SECURITY.md).

## Confirmed absent

| Surface | Where it was looked for | What this repository has |
| --- | --- | --- |
| Jetty, and a `web.xml` session store | `http.rs` binds a `TcpListener` and axum. A search for a file named `web.xml` under this tree found none. The name survives as a comment in `MoquiDefaultConf.xml` and as an exclusion in `plans/Parity-Mapping.md`. The conf comment still says "Embedded Jetty". | hyper, axum, and rustls. Sessions are `moqui.server.HttpSession` rows. Servlets and filters are conf elements read by `web/servlet.rs`. |
| H2 TCP server and the H2 console | No `/h2` route. No `H2Console` in `runtime/conf`. a `database-conf-name` of `h2` is the embedded database name. Parity notes say the H2 console was removed and `H2ServerToolFactory` is not ported. Port 9092 is not a bind. | Embedded Postgres, Unix socket, no TCP. On Windows, TCP to `127.0.0.1` on an ephemeral port. |
| Jackrabbit on 8081 | The repository element is commented XML in the dev conf. `JackrabbitRunToolFactory` is not ported. | No process listens on 8081 for it. (`8081` elsewhere is an example HTTP port.) |
| SubEtha SMTP | No SubEtha code. The entity comment still names `SubEthaSmtpToolFactory`. Parity mapping says that factory is not ported. | Inbound SMTP, off until `smtp-server@listen-port` and a non-empty allow-list are set. Mail from anywhere but the gateway is refused. |
| `/groovysh`, `Tools/GroovyShell` | No conf endpoint or host route for `/groovysh`; the obsolete `Tools/GroovyShell.xml` screen was removed. | These paths are 404. The browser Terminal UI, its command line and host sessions (dev only), and the SSH admin shell are described above. |
| `/rpc/json` | No host route. No `rpc.xml` under webroot. No file named `rpc.xml` in the tree. | The path is 404. |
| `/elastic`, `/kibana` | No routes and no servlet patterns with those paths. | `/search` (`SearchRemote`) and `/dashboard` (`DashboardRemote`). |
| `/htmlr` | No route. The client-suite README says there is no `/htmlr` mapping. | The path is not served. |
| `/vapps` | No `vapps` subscreen in `webroot.xml`. | `qapps` and `apps`. |
| `/rest/api_key`, `/rest/moquiSessionToken`, `/rest/userInfo` | Comments in `rest.xml` only. | Not served. |
