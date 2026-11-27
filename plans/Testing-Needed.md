# Testing needed

Testing that is still open, in one place. Each item says what has not been checked and links to where the detail is.
**Checks to run** need something the regression and acceptance suites do not have: a live model or provider, a real
account, server or device, another operating system or browser, a deployment, or a person's eyes. They can be picked up
by anyone with that platform at hand; record the run, its date and the platform in the linked plan or guide, and remove
the row. **Tests to write** are missing automated tests that an open item names.

Testing that another open plan already lists as needed stays in that plan: the deployed edge node checks
([Edge-Node2 §2.1](Edge-Node2.md#21-checks-on-a-deployed-edge-node)), the Dogfood pipeline and deploy runs on a
second machine ([Dogfood](Dogfood.md)), the live Qwen 3.8 and 8B evaluation runs and the Constitution's published
anchors ([LLM-A2A-Assist](LLM-A2A-Assist.md)), the sealed start's TPM reboot `SealedStart` and the other unbuilt
phases' checks ([Sealed-Secrets](Sealed-Secrets.md)), relevance and cover density on a real corpus
([Performance](Performance.md)), the screen walks of the `ui-review-*` plans, the browser terminal hand checks
([Mobile-Desktop-Exit-Checks](Mobile-Desktop-Exit-Checks.md)), and the Comms live checks that
[Comms-Messaging](Comms-Messaging.md) lists. Open work that is not testing is in [Remaining-Work](Remaining-Work.md)
(for v0.1.0) and [Future-Plans](Future-Plans.md) (after it).

## Checks to run, for v0.1.0

### Agents, Assist, A2A, MCP and the gateway

Most checks ran on the scripted fake provider, fixtures or loopback servers. One live model has been run, recorded below.
Plan [LLM-A2A-Assist](LLM-A2A-Assist.md); guide [Assist](../docs/ai/Assist.md).

| Area | Item | Source |
|---|---|---|
| Live models | One live model so far: Claude Haiku 5.5 (the dev conf's `claude-haiku`), the 24 tasks of `tools` at 3 runs each; its baseline `baseline-claude-haiku.json` (Nov 12, 2026) is pass@1 0.92, pass^3 0.83, 12% wrong calls. Its weakest task is resolved dates (0 of 3: it reads the visits rather than filtering them). Not run: live embeddings, transcription and text actions (the Qwen 3.8 27B and 8B suites are in LLM-A2A-Assist). The provider stream fixtures (`llm/provider/testdata/`) are hand-written from the documented formats; replace them with recorded streams when an endpoint is at hand | 13.1, 13.3, 13.7, 13.13, 13.14 |

### Mail

Guides [Mail-Inbound](../docs/interfaces/Mail-Inbound.md), [Mail-OAuth](../docs/interfaces/Mail-OAuth.md).

| Area | Item | Source |
|---|---|---|
| Sync | Not yet run against a real account (a Gmail app password) | M5d, `moqui-mail/src/sync.rs` |
| OAuth | Not yet run against real Google and Microsoft clients (the Testing 7-day limit and Google verification) | M5e, docs/interfaces/Mail-OAuth.md |

### Monitoring

Guide [Monitoring](../docs/tools/Monitoring.md).

| Area | Item | Source |
|---|---|---|
| Overhead | No per-milestone overhead baselines were captured; production-scale concurrency, cardinality, long retention and rollup load, storage growth and a multi-host soak are not measured. The per-batch export timing (`monitor_performance.rs`, now protobuf with gzip) has not been re-run against a live collector | [Monitoring, Overhead](archive/Monitoring.md#overhead) |
| Export | No real collector has received a gzipped request yet (the Docker test `collector_forwards_json_and_raw_protobuf_for_every_signal` would show it) | `telemetry/otlp.rs` |

### Tools, git and the shop

| Area | Item | Source |
|---|---|---|
| MySQL jobs | The MySQL and MariaDB job runner paths (lock wait timeout and retries, stage progress, `sys.innodb_lock_waits` and `INNODB_LOCK_WAITS`, `KILL QUERY` cancel by connection id) have never run against a server; the host tests run on Postgres only | `crates/moqui-tools/src/jobs.rs`, `crates/moqui-host/tests/it/database_jobs.rs`, [Database](../docs/tools/Database.md) |
| Git push mirrors | No mirror has pushed to GitHub, and an SSH mirror has pushed to no real server (the tests push over HTTPS to a local server) | `crates/moqui-git/src/mirror.rs`, [Git-Hosting](../docs/interfaces/Git-Hosting.md#push-mirrors) |
| Shop payments | Before production, an Authorize.Net sandbox check of the stored-credential indicators and renewals (F106, F115) | mr-pass `plan/impl-shop.md` |
| Mantle | Not called by `moqui test mr-mantle` because they need a network: the three Google services (`GooglePartyServices`) and `create#ContactRequestReCaptcha` | [ConvertProcess](archive/ConvertProcess.md), [MantleFlowTests](archive/MantleFlowTests.md) |

### Dev AIDE

Plan [design-dev-aide](archive/design-dev-aide.md) (archived 2026-11-26); guide [Dev-AIDE](../docs/tools/Dev-AIDE.md). Every dev-aide spec
runs under 15 s with the add-ons since 2026-11-26.

| Area | Item | Source |
|---|---|---|
| Dev agent | The live-model run of the dev agent's evaluation tasks, `moqui test dev-aide --agents --llm-profile <profile>`: it costs API spend, so it runs only with the user's OK (the replays pass) | [§19](archive/design-dev-aide.md#19-tests), P4 |
| Windows | The first result of the dev-aide's `confine::`, `vcs::` and `git::` tests in `.github/workflows/windows.yml`, at the next push | [§19](archive/design-dev-aide.md#19-tests), P1a |
| CI | A job's log and failed tests in the Workspace against a real CI run | [§21.5](archive/design-dev-aide.md#215-checks), P12 |
| Terminal twins | The terminal twins walked by hand | [ui-review-dev-aide-authoring](ui-review-dev-aide-authoring.md) |
| Desktop shell | The desktop shell's library list with Yjs before Tiptap (`libs.txt`), by `./moq test-clients desktop` | [§18](archive/design-dev-aide.md#18-phases), "Keymaps, legacy modes, co-editing" |
| MySQL | `EntityFacade::create_values`' multi-row inserts against a live MySQL server | moqui-entity `entity/value/create_values.rs` |
| Co-editing | One file co-edited in two browsers on one shared dev server: carets, a revoked writer, a room seeded again (the page test runs two tabs on a fake room) | `DevAideCoedit.page.js`, [§18](archive/design-dev-aide.md#18-phases) |
| Smithy | Smithy's side of P11, checked by its REST contract only | mr-mantle `test/smithy/SmithyDevAide.spec.js` |

### Browsers, terminals and screens

| Area | Item | Source |
|---|---|---|
| Zoom | Real 200% browser zoom of the framework apps, left pending by the UI review | [UI-Refinement, Coverage ledger](archive/UI-Refinement.md#coverage-ledger) |
| Terminal | The browser terminal's keys and paste in Chrome, Firefox and Safari; the mouse (clicks, the wheel and drag to copy in kitty and GNOME Terminal over SSH, in local mode, and OSC 52 in the browser terminal in Chrome and Firefox); F-keys, Alt and paste in xterm, kitty, GNOME Terminal, tmux and the Linux console | [Screen-TUI-Impl](archive/Screen-TUI-Impl.md) §5 Phase 4 findings, §7 |
| Wasm host | The WebSocket check is `cargo test -p moqui-host --test it -- wasm_browser`, against that test's `/notws` mock, not moqui-host. A live-host mode needs the package served from moqui-host's origin, a signed-in session, and checks of what the real `/notws` does (the mock's `echo:`, `drop` and `Ack` replies are its own) | [Wasm, Status](archive/Wasm.md) |

### Maps

Plan [Maps](archive/Maps.md) (archived 2026-11-26); guide [Maps](../docs/tools/Maps.md).

| Area | Item | Source |
|---|---|---|
| Geo data | The full Canadian, Dutch and British GeoNames sets (`CA_full`, `NL_full`, `GB_full`, the last about 1.8 million rows) have never been imported or measured | [§6.3](archive/Maps.md#63-extend-the-basic-geo-model), "Every country" |
| Overture | The memory of `OvertureRegionImport` and `OvertureAddressLookup`, run only on macOS where the status map reports none: run them on Linux | [Maps-Overture §6](archive/Maps-Overture.md#6-tests), [§8.3](archive/Maps-Overture.md#83-tests) |

### Apps

| Area | Item | Source |
|---|---|---|
| Maintenance | The full regression suites and the acceptance run: the screen sweeps MaintenanceTui and MaintenanceServerStatic, MaintenanceAccessWalk, MaintenancePhoneWalk and each phase's walks | mr-boss `plan/impl-maintenance.md` |
| People | The design §14.3 review walks (PeopleReviewWalk, PeopleNavigationWalk, PeoplePhoneWalk), written and not yet run | mr-boss `plan/impl-people.md` |
| Marble ERP | Acceptance work after the migration: wiki fixtures, exhaustive role and organization isolation, CSRF, every mounted QVT interaction, physical checks and scanners | [MarbleERPMigration, Limits](archive/MarbleERPMigration.md#limits) |

### Build and release

Guide [Build-Test, Offline source release](../docs/Build-Test.md#offline-source-release).

| Item | Source |
|---|---|
| `./moq trust-dev-cert --system --untrust` on Debian or Ubuntu now runs `update-ca-certificates --fresh`, so the removed root's `/etc/ssl/certs` links go too (they stayed dangling in the 2026-11-25 run). Checked only by a moq test with a stubbed store: run it on a real one and see that no `mr-moqui` link is left | [Install-DevCA, Linux verification](archive/Install-DevCA.md#linux-verification) |
| The release pipeline's `offline-source` job (`ci.xml`) needs its `linux-offline` pool: a runner with rootless Podman or Docker and the `localhost/moqui-build` image built with Chromium. The same check ran by hand on 2026-11-24 (`scripts/test-source-archive.sh`, Linux x86-64): the offline build and the Rust and moq tests passed, and the specs but for the Assist canvas frame in Chromium 156 ([Remaining-Work](Remaining-Work.md#tests)), so what is left is provisioning the pool and one tagged run | [JS-Build, Linux checks](archive/JS-Build.md#linux-checks); [Build-Test, Offline source release](../docs/Build-Test.md#offline-source-release) |

### Load and failure

| Item | Source |
|---|---|
| The opt-in acceptance scenarios the high-level review proposed and nobody wrote: sustained load; prolonged delivery failures; one of two members killed mid-job, timing how long until the job runs elsewhere; TURN and SIP connection floods; downloads, requests and agents saturating together; endpoint slots on two nodes; transitions calling AI tasks under load; a purge of millions of `ServiceJobRun` rows; 25 messages of 20 MiB through mail sync | [High-Level-Review, Verification follow-ups](archive/High-Level-Review.md#verification-follow-ups) |

### Vaults and managed instances

Plan [Vault-Instances](archive/Vault-Instances.md) (archived 2026-11-25); guides [Vaults](../docs/security/Vaults.md),
[Run-Deploy, Managed instances](../docs/Run-Deploy.md#managed-instances). Run here on 2026-11-25 (Ubuntu 26.04,
rootless Podman 5.7, Docker 29.7): the real-engine tests on both engines, `ManagedSealedStart`, `InstanceDeploy` against
a `./moq run` manager, the Android checks with the emulator smoke, and the desktop's lock watchers compiled for Linux
and Windows.

| Area | Item | Source |
|---|---|---|
| Sealed start | `SealedStart` (a TPM2 host and a reboot): on a spare machine or a VM with a TPM (swtpm under QEMU), MR Moqui installed under `systemd/moqui.service` and sealed with `--method tpm`. The user `moqui` runs as needs `systemd-creds` to reach the TPM: libtss2's libraries (`systemd-analyze has-tpm2` lists them; libtss2-rc was missing here) and `/dev/tpmrm0`, the tss group's | `test/acceptance/client/SealedStart.spec.js`, [Sealed-Secrets](Sealed-Secrets.md) S3 |
| Desktop, Windows | The vault locks on the session's lock (Win+L) and on sleep in the installed app: compiled for x86_64-pc-windows-msvc with cargo-xwin, never run | `moqui-desktop` `src/client/screen_lock.rs`; [Mobile-Desktop-Exit-Checks, V5](Mobile-Desktop-Exit-Checks.md#v5-the-vault-in-the-apps) |
| Desktop, Linux | The vault locks on the screen lock and on suspend in the installed app, under GNOME and KDE, Wayland and X11 (logind's `Lock`, `LockedHint` and `PrepareForSleep`, the screen saver's `ActiveChanged`): its unit test ran, and logind's `auto` session resolved here, but no lock was watched live | same |
| Android | The vault on a real phone: device memory in the Keystore behind the fingerprint or the lock screen. Built and smoke-tested on the emulator only (no screen lock there, so device memory was refused, as it should be) | [Mobile-Desktop-Exit-Checks, V5](Mobile-Desktop-Exit-Checks.md#v5-the-vault-in-the-apps) |
| A manager behind a firewall | A managed instance's link to a manager reached through the manager's edge node, by a passthrough host name, on deployed nodes; covered by a unit test of the passthrough here | `edge_link/down.rs`, [Run-Deploy, The manager link](../docs/Run-Deploy.md#the-manager-link) |
| Hosts | Managed instances on a second machine's Docker or Podman over HTTPS with a client key, and over SSH; here only this machine's sockets were used | [Run-Deploy, Managed instances](../docs/Run-Deploy.md#managed-instances) |
| Ubuntu 26.04 hosts | Rootless Podman cannot stop a network's pasta process (its AppArmor profile for pasta refuses Podman's signal), so every container removal there answers 500; the driver takes a removal as done when the container is gone, but the pasta processes may stay. Check that a profile allowing `signal (receive) peer=podman` ends them, and file it upstream | [Build-Test](../docs/Build-Test.md) |

## Tests to write

| Area | Item | Source |
|---|---|---|
| Assist drawer | Chromium smokes for the 13.14 drawer extras (they have node tests only) | docs/ai/Assist.md |
| Assist canvas | A canvas `Lookup` against real option transitions in the Chromium smoke | 13.6, 13.8 |
| Monitoring git sync | A test of the gitECA path end to end (a push over HTTP or SSH, including a pull that deletes) | `report/git_sync.rs` |
| Monitoring alerts | A test that drives a state write failing after a real delivery (the retry check stands in for it) | `report/alert.rs` |
| Data grid | A client test of the grid's filter operators and its Record link (the server's operators are tested; the `.qvue` has no page test) | `tools/screen/Tools/Data/Grid.qvue` |
| Chat | A Chromium smoke of BOSS's chat `m-assist-card` | mr-boss `plan/impl-drawers-commons.md` §5 M10; 13.9 |
| Shell | The 390 px header check (`gate3_browser` tray test) runs without theme plugins: mr-marble's `ActiveOrgNav.qvue` adds a button and its organization's pseudoId as a label, which a long id can still push past a phone's width | framework gaps 2026-10-19 |
| Co-editing | Co-editing across servers has no test: `cluster::publish` giving a message its `message_id` (without one a peer refuses `collab-update` posts) is checked only by a unit test of the post. A two-server test of a co-edited document should edit on one and see it on the other | `notws/collab.rs`, `cluster/mod.rs` |
| BOSS walks | Projects and Commons have no golden-path client walk in the regression suite (one per app) | [Test-Reduction, Order of work](archive/Test-Reduction.md#order-of-work) |
| Android screen share | A committed acceptance walk of the native share on the emulator (consent through `appops`, a browser leg seeing the screen, Stop, leave, rotation; and an encrypted call once the app's crypto device is set up in the walk) | [MM-Spike](archive/MM-Spike.md) §6, §7 |

## Checks to run, after v0.1.0

### Comms

The tests use fakes, loopback and headless Chromium; these need a deployment, a provider's consent or real devices
([Comms-Framework §7.4](archive/Comms-Framework.md#74-built) "Left for later"). The provider round trips, TURN on a
real host, the UDP-blocked path, captions through an edge node and the Recordings player with real media are in
Comms-Messaging, Edge-Node2 and [ui-review-basic-comms](ui-review-basic-comms.md).

| Area | Item | Source |
|---|---|---|
| Phone | Telnyx OPTIONS pings, simultaneous ring and `Privacy: id`; an emergency call | [Comms-Framework §7.4](archive/Comms-Framework.md#74-built) |
| Network | Hairpin NAT; cascading media nodes between two hosts | same |
| Clients | Element against the key endpoints; the XMPP apps Monal, Dino and Gajim (Conversations through the tunnel is in Edge-Node2); the XEP-0479 compliance tester; a push reaching an app server; Firefox and Safari for encrypted chat and calls | same |
| Media | What headless Chromium cannot drive: screen share and the active speaker, a recorded greeting (its microphone is a steady tone), QR verification (a camera) | same |

### Desktop and mobile apps

Screen share from the apps ([Comms-Whiteboard](archive/Comms-Whiteboard.md) MD and MM; guides
[Desktop-Client, Screen share](../docs/ui/Desktop-Client.md#screen-share),
[Mobile-Client, Sharing the screen](../docs/ui/Mobile-Client.md#sharing-the-screen-in-a-call)). Built and unit tested here; no run of the
installed apps has shared a screen yet.

| Area | Item | Source |
|---|---|---|
| Desktop, Linux | Share a window of another application and a whole screen into a call from `./moq desktop dev` on Wayland (the portal's picker over PipeWire) and on X11; a browser leg sees it; the annotation overlay shows over a shared screen on X11 with one monitor and is absent on Wayland | `src/capture.rs` |
| Desktop, macOS | The first share asks for Screen & System Audio Recording; the app's list shows the displays and the other applications' windows; a picked window is shared through the patched runtime's one grant (and a second `getUserMedia` without a new pick is refused); the overlay covers the picked window or screen and its strokes are in a recording | `src/capture.rs`, `patches/tauri-runtime-cef` |
| Desktop, Windows | Chromium's picker shares a window and a screen; the overlay with one monitor | `src/capture.rs` |
| Android, a phone | The system's consent dialog each share (Android 14+'s "Share one app" or "entire screen"), and Cancel shares nothing; a browser in a group call sees the phone's screen within 2 s of consent; Stop in the call, Stop sharing in the notification, leaving the call and the call's end each end the projection and its notification; a rotation keeps the share | `crates/moqui-mobile/src/screen.rs`, `ScreenShare.kt` |
| Android, encoders | On a phone with a hardware VP8 encoder it is the one picked, and its keyframe and bitrate requests take effect; on one without, the platform's software encoder keeps 15 fps at 1280 px | `ScreenShare.kt` |
| Android, network | The share over a phone's mobile network to a public media server (UDP), and with UDP blocked over ICE-TCP; the bitrate follows a constrained uplink down | `src/screen.rs` |
| Android, screen security | A room with Block screenshots shown during a share is black to the others (FLAG_SECURE in the projection) | `ScreenSecurity.kt` |
| Android, encrypted call | In an end-to-end encrypted call (two browsers and the phone), the phone's share decrypts in both browsers within 2 s of consent; a third joining (a rotation) keeps decrypting it; the media server's recording and captions stay refused; an older app build is refused with its message | `src/screen.rs`, `WebrootCallKeys.js` |

## Related documents

[Remaining-Work](Remaining-Work.md) (open work for v0.1.0), [Future-Plans](Future-Plans.md) (after it),
[Build-Test](../docs/Build-Test.md) (the suites and how to run them). The index is [README](README.md).
