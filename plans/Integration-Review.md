# Integration review

> 2026-10-26. Read-only sweep of the assembled framework. Eight seam reviews, then a check of each conflict and bug against the cited lines. No source changes.
>
> This is not a Java parity pass. Entity and service parity (2026-10-16) and screen and web parity (2026-10-20) stand. Rows already in [Remaining-Work](Remaining-Work.md) or [Parity-Diffs](Parity-Diffs.md) were left there. The unfinished gap review of the rest of [Parity-Mapping](Parity-Mapping.md) is a different job.

## Summary

21 findings: 9 bugs, 11 conflicts, 1 risk. The four P1 and five P2 findings were implemented on 2026-10-27; I-21 was already fixed. Eleven P3 findings remain. See [Implementation and verification](#implementation-and-verification) for regression evidence and the cluster rollout requirement.

The bugs are places a handoff drops a check the other side still assumes: a cluster SSH key can open the terminal client, an agent approval card reads past entity filters, a notification socket keeps a logged-out user, a failed login key on two streams never reaches the auth-failure ban, ending another visit deletes the session row before the visit update commits, a notification email is recorded as sent when SMTP did not run, Git LFS is listed as content and read back as the pointer, a failed log ship loses the batch on Postgres and can keep a partial batch on OpenSearch, and the browser host registers component services against a framework-only entity model.

The conflicts are two live paths for one job: the default HTTP listen port and the generated URL port, three cluster body limits, Date and Time text, empty form fields, server-static links and render-mode text, `file:` paths, log-index query strings, the `moqui load` timeout in the help text, and one OAuth retry that the delivered-notice sender does not share.

Commit order (cache flush, feed insert, commit, after-commit assembly), the service-call path for agent writes, and the login functions that screen, git, WebDAV, A2A, MCP, and the gateway share were consistent.

## Validation and priority

> 2026-10-26, second pass. Each finding re-read against the source. Verdict: CONFIRMED, PARTLY (write-up corrected in the note), or NOT REPRODUCED. Priority: P1 security or data loss, fix first; P2 wrong behavior users will hit; P3 consistency or cleanup.

| ID | Current verdict | Original priority | Validation note from 2026-10-26 |
|----|---------|----------|------|
| I-1 | FIXED 2026-10-27 | P2 | Needs the shared cluster key (`ssh/cluster.rs:50`), so cluster holders only. `shell_request` (`server.rs:1034`) is the only handler that skips `self.cluster`; exec returns at `:890`, subsystem filters at `:861`. Fix: refuse shell when `cluster`, ~3 lines plus a test next to `tests/it/ssh.rs:511`. Cheap auth-boundary hardening. |
| I-2 | FIXED 2026-10-27 | P1 | Also an existence oracle: `entity_target` (`lazy.rs:694`) reads the row before `user_permitted` (`:696`), so a user with no permission gets `not_found` versus `not_allowed`. Field values reach the card only after the permission check, which checks permission and never row filters. Preview side is narrower: `production_row` (`preview.rs:242`) shows only changed fields. Fix: apply user filters to the business-entity find in `entity_record` (filters while authz is off, or run that find with authz on), and move `entity_target` after `user_permitted`, ~20 lines. |
| I-3 | FIXED 2026-10-27 | P1 | Also: `deliver_notice` keeps routing pushes by `slot.user_id` after logout (`notws.rs:511`); idle timeout never fires because Pong resets `last_heard` (`:304`, `:318`); login-key sockets never see revoke or expiry. Default HTTP build. Fix: termws `SESSION_CHECK` loop in `run_socket`, key-row check for login-key sockets, ~30 lines. |
| I-4 | CONFIRMED, minor | P3 | Guard is off by default (`webapp_web_protection` and `webapp_trusted_proxies` empty, `MoquiDefaultConf.xml:59`, `guard.rs:260`), and login keys are 40 random chars stored hashed, so the missing ban counts add little. Fix: `key_user` returns `take_auth_failure()`, callers `guard.record`, ~20 lines in `notws.rs` and `llm/sse.rs`. |
| I-5 | PARTLY | P3 | Main trigger is not named: `delete_session_row(...)?` in the loop (`session.rs:568`) fails partway, the service `addError`s (`UserServices.xml:38`), and `thruDate` rolls back while earlier rows stay deleted. Effect is a stale "active" sign-in that a retry fixes. The separate delete transaction is the safer way to fail (a rollback must not revive sessions). Fix: log and go on per session, ~5 lines. |
| I-6 | FIXED 2026-10-27 | P2 | Lines accurate (`implementation.rs:136`, `:171`, `:177`; `message.rs:728`, `:763`). Common on dev and demo hosts with no SMTP host. Nothing resends a template `EmailMessage` left `ES_READY`. Java records it the same way, so this is old behavior, still wrong. Fix: write `sentDate` only when the output `messageId` is non-empty, keep `emailMessageId`, log the send's non-error notes, ~5 lines. |
| I-7 | FIXED 2026-10-27 | P2 | Path: `get#GitBlob` → `service/git.rs:508` → `browse.rs:217` `blob_at` → `ops::read_at` (`ops.rs:496`). Also `hosting.rs:189` swaps in the content size for any pointer-shaped blob, linked or not, while `file_content` (`lfs_files.rs:110`) swaps only when a `GitLfsObject` row exists. Seen in mr-boss Code > Files (`CodeServices.xml:372`): the file shows as the ~130 B pointer, while Raw/Download (`git://`) gives the content. Fix: `blob_at` uses `file_content` like `mod.rs:1070`; size only for linked oids. ~10 lines. |
| I-8 | CONFIRMED | P3 | Drain `logging.rs:226`, drop `:232`. On by default (`search_backend=postgres`, log indexes kept). Loss only while the store fails; lines stay in `moqui.log`. A blind requeue would duplicate OpenSearch auto-id items and retry a bad item forever: requeue failed items only, with a retry cap, inside `QUEUE_LIMIT` (`logging.rs:54`). ~25 lines. |
| I-9 | CONFIRMED | P3 | Call at `moqui-browser/src/host.rs:81`, helper `:286`, `open_with_driver` passes no components (`entity/facade.rs:153`). Shipped browser runtime (`crates/moqui-browser/runtime`) does not hit it; only `api.rs:123` `init` with a custom runtime does. Fix: move `component_entity_dirs` and conf merge from moqui-host into moqui-core and use them, ~20 lines. |
| I-10 | FIXED 2026-10-27 | P2 | No shipped conf sets the port or host (only docker `AppInstance` data, `ServerEntities.xml:261`), and the 8080 default is never written back (`lib.rs:1203`, `tui_cmd.rs:175` only read it). The git clone URL (`git/server/mod.rs:323`) has no request-URL fallback, so `httpsUrl` is `http://localhost` even in a request. DAV and SSO use the request URL first. Password-reset mail on a default dev install links to port 80. Fix: one effective-port resolver, 8080 wins, used by factory, dav, git; give git the request-URL fallback. ~30 lines. |
| I-11 | FIXED 2026-10-27 | P1 | A refused body retries 3 times and marks the peer down for 5 s (`cluster/mod.rs:606`, `:715`), and `send_batch` skips a down peer, so that peer misses entity-cache invalidations and serves stale data. Any user can trigger it: no WebSocket size cap (`notws.rs:149`), and the update is base64 twice, so ~390 KiB raw (a big paste) exceeds 1 MiB. Multi-member clusters only. Fix: one shared `MAX_CLUSTER_BODY`, sender skips over-limit bodies with a warning, 4xx neither retries nor marks down, cap collab updates and send the update once. ~40 lines. |
| I-12 | CONFIRMED, minor | P3 | `parse_date` (`moqui-lib/src/object.rs:419`) accepts both forms in UTC, so typed service parameters get the same value; only String parameters and string compares differ. Civil text should win (`to_plain_string`, `object.rs:194`), but it is widely used: audit callers first. |
| I-13 | PARTLY | P3 | Write-up is wrong on the key point: a field the user clears is dirty (`moqui-tui/src/state.rs:181`) and posts `""` like the web. Only an untouched empty field differs, and its stored value is usually already empty. Gap missed: the tui ignores `exclude-empty-fields`. Keep the tui rule; optionally honor `exclude-empty-fields`. |
| I-14 | CONFIRMED, unused | P3 | No shipped screen uses `~name=` segments, an image url with `?`, or a dynamic-options transition with `?`/`~`. Fix when needed: `static_url` returns `prepared.inline` as overriding parameters, ~13 lines. |
| I-15 | FIXED 2026-10-27 | P2 | 36 runtime screens opt into server-static qvt. Users see links and buttons they cannot use, and the server refuses the click (cosmetic, not a hole). The static template is cached per screen, not per user (`statics.rs:222`), so the condition and permission cannot be baked in. Fix: `exists=false` is disabled at compile time; the actions package sends a per-request map of disabled transitions bound to `:disable`. ~40 lines. |
| I-16 | PARTLY, unused | P3 | Top-level server-static uses `renderModeMacro` (`ScreenQvtStatic.js:2387`), the same rule as `anyMacros`. The cited `:1450` is only `renderNode` for widgets inside form fields. The tui first-match (`ScreenTui.js:1488`) does differ. No shipped screen has a mixed or duplicate render-mode. Fix: one shared chooser in ScreenSupport.js. |
| I-17 | FIXED 2026-10-27 | P1 | Real callers hit it. ElFinder root `file:runtime` (`ElFinder.xml:44`): subdirectories become `file:runtime/conf`, which `file_path` (`resource.rs:1081`) resolves to `/conf`, so browsing `runtime/lib` lists the system `/lib`, and outside production writes go to `/…`. Admin-only, and production refuses writes. Also `mr-marble/test/ScreenReport.spec.js:67` writes `file:dir/x.md` to `/x.md`. No caller uses `file://host/`. Fix: one shared resolver (strip `file:`, drop an empty or `localhost` host after `//`, keep relative paths relative, which `locate` does already), plus a `file:rel/sub` test, ~15 lines. |
| I-18 | CONFIRMED, narrower | P3 | Only fielded terms miss: a bare term matches through `all_text` (`postgres/mod.rs:491`). `thrown` and `mdc` are written (`logging.rs:671`, `:679`) but not mapped (`search/logs.rs:39`). `~`/`^` error at `search/parse.rs:185`, `:237`. Stale doc `logs.rs:16` ("only on OpenSearch"). Fix: map the `thrown` subfields, or fall back to a jsonb path match for log mappings; note Postgres syntax on LogViewer. ~20 lines. |
| I-19 | CONFIRMED | P3 | Same wrong wording in `entity/load_feed/mod.rs:78`. Per-record 60 s does apply for an entity in a different group from the file transaction (`loader.rs:449`, `:528`). Fix: reword the help and doc to "each data file in one transaction; omitted: 600 seconds", ~4 lines. |
| I-20 | CONFIRMED | P3 | Side B citation corrected: `send_built` is `service/mail/implementation.rs:1216`, single send at `:1257`; `delivery.rs:398` is its caller. Also `send_built` never marks the refused token, so later notices keep failing until expiry or a `lettre_send` refresh. Needs SMTP listener plus success DSN plus `EatOAuth2` plus early revoke. Fix: shared refuse-then-refresh helper, ~20 lines. |
| I-21 | FIXED 2026-10-26 | — | Locks moved to `resource/locks.rs`, keyed by resource location instead of the user's own href (which also meant a lock never held a second WebDAV user). `ResourceFacade` writes, deletes, renames, restores and change sets refuse a locked location per change, so the Files screens, SFTP drives, drive services and scripts are held and the other files of an upload or move still go. A folder delete or move is refused when something under it is locked. A finished delete or move drops the lock. Tests: `resource::locks`, `files_tests::a_webdav_lock_holds_every_writer…`, `dav::tests::a_lock_holds_another_users_client…`. |

Result: 18 confirmed (7 worse or wider than written, 4 minor or unused in shipped screens), 3 partly (I-5, I-13, I-16), none disproved. Corrections to the original write-up are in the notes; the sections below are left as first written.

### Fix order

P1 and P2 below are completed. The original order and estimates are retained as the review record.

1. P1, in this order (cheap and contained first):
   - I-17 `file:` resolver: ElFinder subdirectories leave `runtime` for `/`. ~15 lines.
   - I-3 `/notws` session re-check: logged-out sockets keep pushes and collab writes. ~30 lines.
   - I-2 agent record read: apply user filters, and check permission before the read. ~20 lines.
   - I-11 cluster body limit: one large co-edit marks peers down and drops cache invalidations. ~40 lines.
2. P2:
   - I-6 `sentDate` only on a real send. ~5 lines.
   - I-1 refuse the shell for the cluster key. ~3 lines.
   - I-7 `get#GitBlob` returns LFS content. ~10 lines.
   - I-10 one effective HTTP port for the listener and URLs. ~30 lines.
   - I-15 disabled links on server-static screens. ~40 lines; needs a per-request disabled map.
3. P3, take when nearby:
   - Quick: I-19 (help text), I-5 (log and go on per session), I-4, I-20.
   - Needs a decision first: I-12 (audit `to_plain_string` callers), I-18 (map `thrown`, or a jsonb fallback).
   - Defer until a screen needs it: I-8, I-9, I-13, I-14, I-16.

## Implementation and verification

> 2026-10-27. All nine P1/P2 findings have separate implementation commits and regression coverage. No database migration. P3 findings remain outside this batch.

| Finding | Implemented behavior and regression coverage |
|---------|----------------------------------------------|
| I-17 | One local-file resolver preserves relative children and supports absolute/localhost paths; remote authorities are refused. Context tests cover both resolvers, child browsing, nested writes and reads. |
| I-3 | Notification sockets retain the original session or hashed key and revalidate every 60 seconds, failing closed on lookup errors or identity changes. Key checks are read-only. Key tests cover expiry, deletion, reassignment and account disablement; a real-server test waits through the production interval and verifies logout/revocation/expiry close sockets while a healthy session survives. |
| I-2 | Permission precedes target lookup; business-row snapshots, AI field tools and preview comparisons use normal view authorization and filters. Preview after-values are checked inside the simulation too; denied reads produce unavailable values. Tests cover filtered cards, existing/absent keys under denial, approval-time permission revocation and secret/denied preview redaction. |
| I-11 | HTTPS, SSH and NOTIFY share a 16 MiB serialized-envelope limit. Oversized messages are refused without disabling a peer; permanent 4xx outcomes do not retry. Collaboration validates before apply/ack and carries the update once in `frame`. Boundary, retry classification and no-apply/no-ack tests cover this; real SSH and stored-NOTIFY tests exercise payloads above the former 1 MiB limit. |
| I-6 | Saved email IDs survive skipped delivery, but `sentDate` requires an SMTP `messageId`. Delivery notes are logged before restoring caller messages, and previous errors do not suppress independent recipient service calls. The SMTP sink test verifies actual delivery, preserved caller errors and an unsent draft with no sent date. |
| I-1 | Cluster-key connections cannot request a shell. The SSH integration test authenticates with the actual shared key, verifies shell refusal, and exercises the permitted cluster channel. |
| I-7 | Blob reads and tree metadata use repository-linked LFS content consistently. The git resource test covers linked content and an unlinked pointer-shaped file. |
| I-10 | The listener and generated HTTP fallback URLs share the empty-to-8080 resolver. Explicit public URLs and HTTPS ports stay distinct; Git gains the DAV request-origin fallback. Tests cover default/explicit ports, request origins, public overrides and the configured CORS origin's effective port. |
| I-15 | Static templates bind availability supplied per request and list row. Missing targets and unavailable package state disable controls. Shared components guard clicks, keyboard and programmatic submission. Rust tests share a cached template across users and row conditions; browser tests verify disabled and re-enabled controls. Generated templates, packages and precompiled component renders were refreshed. |

Validation: the full core suite passed 2,012 tests (three ignored), excluding only `l10n::tests::spanish_catalog_files_match_the_message_keys` because this checkout has no `runtime/component/mr-l10n/data/es` directory. The host unit suite passed 220 tests; real-server socket, SSH, collaboration and agent-preview tests passed. `moqui test framework --spec ScreenQvt` passed 16 features, and the selected offline QVT browser suites passed 51 features. Workspace and minimal-build lint passed. A final 20-case targeted run passed the later regression additions, including permission revocation, read-only key reassignment, caller-error mail delivery and preview redaction. The real-server agent suite was repeated after the final preview checks. Changed Rust files pass rustfmt, and `git diff --check` passes.

**Cluster rollout:** stop collaborative editing, upgrade and restart every member, then reopen editing. Mixed versions are unsupported: the compact envelope uses `body.frame.update`, whereas old receivers expect a second top-level update and old HTTPS listeners cap bodies at 1 MiB. The common limit is now 16 MiB on every transport. See [Security surface](../docs/SECURITY_SURFACE.md#integration-fix-cluster-rollout).

## Bugs

### I-1 — Severity: bug

- Seam: cluster SSH key to a shell channel
- Side A: `crates/moqui-host/src/ssh/server.rs:731` (`auth_publickey` for `moqui-cluster`)
- Side B: `crates/moqui-host/src/ssh/server.rs:1034` (`shell_request`)
- What happens: The cluster key sets `cluster` and `username` and returns `Auth::Accept` without `internal_login_user`. The connection comment says that peer may open `cluster <topic>` channels and nothing else. `exec_request` and `subsystem_request` follow that. `shell_request` does not look at `cluster`. `let_in` is true because the username is set, `can_shell` stays false, and the branch at line 1047 starts `apps` as `Who::User("moqui-cluster")`. The terminal client then calls `internal_login_user` on that name. With no such `UserAccount`, screen calls stop. If that account exists, the cluster key is that user, with no `UserSshKey` and no password. The default host features include `ssh` and `tui`.
- Already tracked: no

### I-2 — Severity: bug

- Seam: agent service-tool and approval preview versus entity filters
- Side A: `crates/moqui-core/src/llm/tools/lazy.rs:760` (`quietly` around `entity_record`)
- Side B: `crates/moqui-core/src/user/filter.rs:75` (`apply_filters` returns the condition unchanged when authorization is off)
- What happens: `quietly` (`llm/run/mod.rs:215`) turns authorization off for `moqui.agent` bookkeeping. `entity_record` (`llm/tools/catalog.rs:653`) also calls `find().disable_authz()` on the business entity the service writes. View permission is skipped and user entity filters are not applied. Encrypted fields and secret-like names are omitted. The row is the approval card's current values and, when the stamp changed, the text sent back to the model. The write that follows is `service::call::sync_checked` with authorization on, so the service's own finds stay filtered. The approval preview loads the production row the same way (`llm/approval/preview.rs`). A row the filters would hide is readable on the card and in the preview.
- Already tracked: no

### I-3 — Severity: bug

- Seam: `/notws` user versus logout, against `/termws`
- Side A: `crates/moqui-host/src/notws.rs:122` (session user read once at upgrade)
- Side B: `crates/moqui-host/src/termws.rs:267` (session re-read every minute)
- What happens: `/notws` stores `user_id` on the socket and `run_socket` does not call `session_user` again. Collab `join` and `apply` submit that captured id. After logout the socket still joins and applies as the user from upgrade. `/termws` uses the same lookup at upgrade, then closes when a later read finds another user or none.
- Already tracked: no

### I-4 — Severity: bug

- Seam: a rejected login key and the web-protection auth-failure ban
- Side A: `crates/moqui-host/src/notws.rs:223` (`key_user` turns a login error into `Ok(None)`)
- Side B: `crates/moqui-host/src/http.rs:797` (screen dispatch records `Event::AuthFailure`)
- What happens: `login_user_key` sets the thread-local auth-failure flag. `/notws` and the assist event stream (`llm/sse.rs`) swallow the error. Neither calls `guard.record`. The request-pool `WorkCleanup` drop clears the flag (`service/pool.rs:104`) before the HTTP task sees `Ok(None)`. Git HTTP, WebDAV, A2A, MCP, and the gateway record the flag inside their jobs. The ban is `auth-failure`, threshold 20 in 600 seconds. A bad `login_key` on `/notws` or the assist stream never reaches it. Loopback clients are exempt from the guard, so this is the non-loopback path.
- Already tracked: no

### I-5 — Severity: bug

- Seam: `end#UserVisit` visit update versus session delete and `session-end`
- Side A: `crates/moqui-core/src/user/session.rs:546` (`Visit.thruDate` on the service transaction)
- Side B: `crates/moqui-core/src/user/session.rs:2035` (`delete_session_row` through `with_reserve_tx`)
- What happens: The visit update commits when the service transaction finishes. `delete_session_row` commits on a reserve connection before `end_user_visit` returns, and `publish_session_end` posts `session-end` in that same window. A rollback of the service transaction, including a commit failure or an outer transaction the service joined, restores `thruDate`. The `HttpSession` rows stay deleted, and peers have already dropped their live copies.
- Already tracked: no

### I-6 — Severity: bug

- Seam: notification email to `send#EmailTemplate` when a message row is saved
- Side A: `crates/moqui-core/src/service/mail/implementation.rs:179` (delivery error returns `Ok` with `emailMessageId`)
- Side B: `crates/moqui-core/src/notify/message.rs:763` (`sentDate` stored for any non-empty id)
- What happens: With `createEmailMessage` / `emailMessageSave` (the notification loop sets this from the topic or the caller), a template send that never reaches SMTP still returns `emailMessageId`. An empty SMTP host (line 144), a recipient filter, and a delivery error all take that path. `send_emails` treats any non-empty id as sent and writes `sentDate`. `update_sent` runs only after `deliver` succeeds, so the `EmailMessage` stays `ES_READY`. The loop logs message-facade errors and then restores the previous message list, so the delivery note is dropped.
- Already tracked: no

### I-7 — Severity: bug

- Seam: an LFS file's bytes, directory listing and `git://` versus `get#GitBlob`
- Side A: `crates/moqui-core/src/resource/git/hosting.rs:196` (tree size replaced from the pointer)
- Side B: `crates/moqui-core/src/resource/git/ops.rs:499` (`read_at` returns the git object)
- What happens: `get#GitTree` publishes the LFS content size. `GitServices.xml` says that file is read with `get#GitBlob`, and that an LFS file's size is its content's. `get#GitBlob` returns `read_at`, the pointer text. `git://` read, stat, and open go through `file_content`, which returns the content blob when a `GitLfsObject` row exists. A linked LFS file is listed at its content size, shown as the pointer, and downloaded as the content.
- Already tracked: no

### I-8 — Severity: bug

- Seam: log shipper flush to `index_log_entries`
- Side A: `crates/moqui-host/src/logging.rs:230` (batch drained, then dropped on `Err`)
- Side B: `crates/moqui-core/src/search/opensearch.rs:837` (`_bulk` with `errors: true` returns `Err` after successful items are stored)
- What happens: The shipper takes up to 1000 entries and, on `Err`, writes to stderr and does not put the batch back. Postgres inserts in one transaction and rolls it back, so the batch is absent from `moqui_logs` and `moqui_http_log`. OpenSearch can already have stored some items. Log rows have empty ids, so those items are not replaced by a later write of the same lines. A failed ship loses the batch on Postgres and loses only the failed items on OpenSearch, with no retry either way. The lines remain in `moqui.log`. Feed document writes are a different path: a failed `index_documents` leaves the pending row, and those ids are stable.
- Already tracked: no

### I-9 — Severity: bug

- Seam: browser entity boot versus server entity boot, while component services are still registered
- Side A: `crates/moqui-browser/src/host.rs:287` (`open_entity_facade` on the framework directory)
- Side B: `crates/moqui-host/src/db/entity_facade.rs:87` (merged conf and `component_entity_dirs`)
- What happens: `Browser::open` loads entities through `EntityFacade::open_with_driver` with the framework directory only. Component entity directories, `extend-entity`, `load-entity`, `sql-name-style`, and `crypt-pass` from the merged conf never reach that facade. The same open then lints every non-framework component `service/` tree. The shipped browser runtime has an empty conf and one component with no `entity/` directory. Any other runtime passed to `Browser::open` still loses those definitions while its services are registered. The server path uses `open_entity_facade_for_factory`.
- Already tracked: no. Remaining-Work "Wasm browser host" records the missing login and the disabled authorization. This is the entity model on that host.

## Conflicts

### I-10 — Severity: conflict

- Seam: empty `webapp_http_port` as the listen port and as generated base URLs
- Side A: `crates/moqui-host/src/http.rs:678` (empty is 8080)
- Side B: `crates/moqui-core/src/context/factory.rs:645` (empty `http-port` is omitted, the URL default of port 80)
- What happens: `MoquiDefaultConf.xml` sets `webapp_http_port` to empty, and the webroot `http-port` is that property. The listener and `usage.rs` treat empty as 8080. `webapp_root_url_for` omits an empty port, so the URL is `http://localhost`. The git clone base URL and the DAV base URL use that same fallback. With no `webapp_http_host` and the default conf, password-reset, git, and CalDAV addresses point at port 80 while the process listens on 8080. HTTPS uses two keys on purpose: `webapp_https_listen_port` and `webapp_https_port`.
- Already tracked: no

### I-11 — Severity: conflict

- Seam: one cluster fan-out payload, three body limits
- Side A: `crates/moqui-host/src/cluster_edge.rs:44` and `:636` (HTTPS `BODY_LIMIT` 1 MiB, then HTTP 400)
- Side B: `crates/moqui-host/src/ssh/cluster.rs:41` (SSH `MAX_FRAME` 16 MiB) and `crates/moqui-host/src/cluster_notify.rs:41` (NOTIFY inlines 7,900 bytes and stores the rest in `ClusterMessage.payload`, an uncapped Postgres `TEXT`)
- What happens: `send_batch` posts the same JSON to every live peer. A body over 1 MiB is refused on HTTPS, and that failure marks the peer down for `PEER_BACKOFF`. SSH applies anything at or under 16 MiB. NOTIFY applies the loaded JSON with no size check. Co-editing publishes the client update with no length cap, and the base64 string is in the JSON twice (`notws/collab.rs:217`). A body between 1 MiB and 16 MiB is applied on SSH and on NOTIFY and refused on HTTPS. Topic names, `origin_id`, `message_id`, and the do-not-republish rule match.
- Already tracked: no

### I-12 — Severity: conflict

- Seam: Date and Time text in the script host and in the server-static actions package
- Side A: `crates/moqui-core/src/screen/sri/fields.rs` `getFieldValuePlainString` (epoch-millis text, posted by merged qvt, html, and tui for `also-hidden` and hidden fields)
- Side B: `crates/moqui-core/src/script/template.rs:979` (`unwrap_json`: a Date is `YYYY-MM-DD`, a Time is `HH:MM:SS`)
- What happens: Server-static display and hidden fields post `$mq.str` of the package value. A Date posts civil text from a static screen and millis text from merged qvt, html, and tui. A static expression that reads the field sees the civil text. A Timestamp is millis text on both sides.
- Already tracked: no

### I-13 — Severity: conflict

- Seam: an empty optional field on submit
- Side A: `WebrootVue.qvt.js` `submitGo` and `moqui-html.js` `submitBackground` (an empty control is posted as `""` unless `exclude-empty-fields="true"`)
- Side B: `crates/moqui-tui/src/forms.rs:1051` (`posted` leaves out an empty optional value the user did not touch)
- What happens: Entity-auto create, update, and store map a posted empty string to null, so the web post clears the field. An omitted name leaves the stored value. The tui comment says this avoids writing `""` into a foreign key. A field the user clears in the terminal and a field they clear in qvt or html do not reach the service as the same value.
- Already tracked: no

### I-14 — Severity: conflict

- Seam: `?` and `~` parameters on a screen URL, merged render versus the server-static template
- Side A: `crates/moqui-core/src/screen/urls.rs` `build_url_inline` (copies inline parameters from `prepare_screen_path`)
- Side B: `crates/moqui-core/src/screen/urls.rs:591` (`static_url` sets `path` to `encode_path` of the screen names and does not read `prepared.inline`)
- What happens: A static link's `splitQuery` saves a `?` query before `static_url`, so that query survives and a `~name=value` segment does not. `imageTag` and `remote` pass the raw url through, so a screen or transition image and a dynamic-options transition lose both. A url that contains `${}` is returned as raw text. `$mq.link` resolves `.` and `..` and leaves `~name=value` as a path segment, with no `ScreenPathAlias` and no `//` search. `script/runtime/urls.rs` is a wrapper over `UrlInstance` and is not a third rule.
- Already tracked: no

### I-15 — Severity: conflict

- Seam: `getDisableLink` on a link or form
- Side A: merged qvt, html, and tui (a false transition condition, a missing target, or a refused permission disables the control)
- Side B: `runtime/base-component/framework/template/screen/ScreenQvtStatic.js` `link` and `formSingleOpen` (they call `static_url` and never `getDisableLink`)
- What happens: `static_url` records `exists`, `read_only`, and `method`. A missing target becomes `path: "#"`. It does not record the condition or the permission check, so a server-static screen still writes the link and the form. The click is still authorized when it reaches the server.
- Already tracked: no

### I-16 — Severity: conflict

- Seam: which `render-mode` text is written
- Side A: `runtime/base-component/framework/template/screen/ScreenSupport.js:248` (`anyMacros` keeps one text; a typed match overwrites an untyped one; `xsl-fo` counts as `html-print`)
- Side B: `runtime/base-component/framework/template/screen/ScreenQvtStatic.js:1450` (every child whose type is missing, `any`, or includes `qvt`; `xsl-fo` is not rewritten) and `ScreenTui.js` (the first `tui` text, otherwise the first untyped text)
- What happens: A render-mode with an untyped text and a `qvt` text shows both bodies on a server-static screen and only the qvt body on merged qvt. Two `tui` texts emit the first. Two `qvt` texts on the merged path emit the last.
- Already tracked: no

### I-17 — Severity: conflict

- Seam: one `file:` location string, two path resolvers
- Side A: `crates/moqui-core/src/context/locate.rs:117` (strip `file:` and an optional `//`, then use the remainder)
- Side B: `crates/moqui-core/src/context/resource.rs:1090` (`file_path`: when the remainder has no leading `/`, take the path from the first `/`)
- What happens: `file://localhost/tmp/a` is the relative path `localhost/tmp/a` through `locate` and the absolute path `/tmp/a` through the facade. `file:foo/bar` is `foo/bar` through `locate` and `/bar` through the facade. `file:/abs` and `file:///abs` agree. Script text, service-file locations, and data files resolve with `locate`. Facade get and write resolve with `file_path`.
- Already tracked: no

### I-18 — Severity: conflict

- Seam: a log-index `queryString` on Postgres and on OpenSearch
- Side A: `crates/moqui-core/src/script/runtime/search.rs` (the string is `Query::Lucene` for whichever backend is installed)
- Side B: `crates/moqui-core/src/search/postgres/compile.rs:421` (a field that is not on the log mapping is `FALSE`)
- What happens: OpenSearch forwards the string as `query_string`. The shipper writes `thrown` and `mdc`. Those are not on the Postgres log mapping, so `thrown.message:…` matches on OpenSearch and matches nothing on Postgres. The same string with `~` or `^` is an error in the Postgres parser and a query on OpenSearch. `LogViewer.xml` sends this string and links the Elastic query-string syntax. The default backend is Postgres, and it does keep these indexes. `search#DataDocuments` is a different builder and was consistent across the two backends.
- Already tracked: no

### I-19 — Severity: conflict

- Seam: omitted `moqui load` timeout
- Side A: `crates/moqui-host/src/usage.rs:140` (omitted timeout is the transaction default, 60 seconds, for each record)
- Side B: `crates/moqui-core/src/entity/load_feed/loader.rs:146` (`FILE_TIMEOUT` is 600 seconds for the file)
- What happens: `LoadFlags` says each data file is one transaction and 600 seconds when the flag is absent. `load_options` passes `None`, and `in_file_transaction` uses 600 seconds. The transaction manager's own default is 60 seconds, which is the number the help names. Startup empty-db load uses the same `LoadFlags` default.
- Already tracked: no

### I-20 — Severity: conflict

- Seam: an OAuth SMTP token the server refuses
- Side A: `crates/moqui-core/src/service/mail/implementation.rs:1302` (`lettre_send` retries once on 454, 530, 534, 535, or an XOAUTH2 challenge)
- Side B: `crates/moqui-core/src/service/mail/delivery.rs:398` (`send_built` for the inbound delivered notice sends once)
- What happens: `send#EmailTemplate` and `send#EmailMessage` go through `lettre_send`. The SMTP listener's success DSN goes through `send_built`, which authenticates and sends the raw message once. On a refused token the listener logs the error and leaves the message delivered. Both paths share `captured()`. They do not share the retry. This is live when the account's server is `EatOAuth2`.
- Already tracked: no

## Risk

### I-21 — Severity: risk

- Seam: a WebDAV exclusive lock and an SFTP drives write of the same file
- Side A: `crates/moqui-core/src/dav/files.rs:462` (PUT, and the same check on DELETE and MOVE, calls `locks::check_write`)
- Side B: `crates/moqui-host/src/ssh/sftp_fs.rs:97` (drives `Store::write` calls `tree::write`)
- What happens: `locks::check_write` answers 423 when another user holds that href in the process-local table. `tree::write` (`dav/files.rs:876`) calls `upload#Resource` and does not call `check_write`. `tree::remove` and `tree::rename` likewise skip it. `sftp-drives` defaults on for a user without `SSH_SFTP`. A lock taken over WebDAV on this server leaves an SFTP drives client free to replace, trash, or move that file. Remaining-Work already records that the lock table is per server across a cluster. This is a second door on the same server. SFTP with `SSH_SFTP` writes a different tree and does not share the href.
- Already tracked: no

## Seams that agreed

- Boot. Conf merge order is default, then components, then the runtime conf. One Rust service location cannot sit in both the built-in table and a component catalogue. Host features match `docs/Packaging.md`. Detach and `moqui test --serve` do not build a second live factory on top of the process globals already listed under Remaining-Work.
- Commit order. Cache flush, the `DataFeedPending` insert, the enlisted commit, and `process#DataFeedPending` run in that order. Guarantee session flush runs in `end_request`. The scheduler queues job bodies onto the service pool. A nested borrow of the enlisted connection is refused. `force_new` stops at `park_budget`.
- Authorization doors. Password, login key, app password, and SSO external login reach `finish_login` and `session::set_user`. Git HTTP, WebDAV, A2A, MCP tool calls, and the gateway read `current_user_id` after those functions. The HTTP guard runs before the artifact tarpit on purpose. The browser host's disabled authorization stays in `moqui-browser`.
- Screens. `script/runtime/urls.rs` delegates to `UrlInstance`. Pass-through parameters override a link's own parameters in both merged and server-static qvt. `xsl-fo` is the `html-print` alias on the render-mode switch. No caller in these paths asks for FO XML bytes.
- Files. `classpath:` and `content:` agree between `locate` and the resource facade. `file:/…` and `file:///…` agree. A `dbresource` over the blob threshold is read back by `blobId`. `git://` writers and receive-pack take the same per-repository lock. `.` and `..` stay inside the served root on WebDAV, SFTP, static HTTP, and `git://`.
- Messages. `send#EmailTemplate` and `send#EmailMessage` share `deliver` and the `smtp` feature gate. Chat waits for the caller transaction, then publishes one `notification-message`. Collab's empty id is filled before fan-out. `queue#SystemMessage` matches its own description. The search feed schedules receive only from `on_commit`. Mail stores a `text/calendar` part and does not expand RRULEs. Recurrence stays in `pim/`.
- Search. One backend is installed. `search#DataDocuments` builds one query tree for Postgres and OpenSearch. A failed document index leaves the pending row. Artifact hits are entity rows, not a second copy of `moqui_logs`. The default Postgres build does fill the log indexes, and the log viewer reads them through `indexExists`.
- Agents. A mutating service tool runs through `sync_checked` after permission, schema, and argument checks. Screen and canvas writes go through `ScreenFacade` and stay held until approval. Delegate and MCP client calls use the sim remote gate. Waits do not hold a pooled connection or an open transaction. OpenUI evaluation borrows QuickJS only for the synchronous eval.

## Out of this pass

Java parity of the modules that have not had a phase review. Performance, monitoring, and the unbuilt proposals. Business screens in IronStone, Mantle, and Marble, except the repository file screen named from the LFS handoff. License. Fixes.
