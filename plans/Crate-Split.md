# Crate Split

Status 2026-10-30: Phases 0 to 4 done (9788e711, 0bda5fdc, 9aa631f9, 4e962f93, 27ab3317: `moqui_core::extension`,
`moqui-git`, `moqui-mail`, `moqui-entity`, `moqui-report`) and Phase 5's `moqui-collab`; `moqui-dav` next ([Phases 5
and 6 in detail](#phases-5-and-6-in-detail), [Resuming](#resuming)). Splits
`moqui-core` along hook seams, above it for the tool subsystems and below it for the entity facade, and then the host,
so an edit rebuilds less and more of the build runs in parallel. It revises [Crate-Layout](archive/Crate-Layout.md),
whose "few, larger crates" rule this replaces
([Practices-Rust §1, §8](../docs/dev/Practices-Rust.md#1-crates-and-where-code-goes)).

## Why

`moqui-core` was 289,000 lines (205,000 outside tests). Its frontend runs serially on any edit, and every crate above
it waits for it. The first split, `moqui-llm` (commit 18d09dd3, 79,000 lines), measured the gain on a develop build
with one file edited: an edit in the agent runtime went from 20.5 s to 3.5 s, an edit in core `service/` from 20.8 s to
17.8 s, and core's own compile from 62 s to 45 s with `moqui-llm` (24 s) building beside the host.

A subsystem that only registers services and tools goes above core, so an edit there rebuilds that crate, the host and
the binary link. These crates build in parallel with each other. The entity facade goes below core, so a core edit
(screens, services, scripts) no longer recompiles it. After a core edit the critical path is core, then the slowest
crate above it, then the host: the gain is mostly for edits inside the split-out crates, and for core's own smaller
frontend.

## The seam rules

- A crate above core reaches it through `pub` items and registers through `moqui_core::extension` (Phase 1): an
  inventory entry with the features it is, its Rust services and its native registrations (tools, resource providers,
  runners), and a step at the start of the factory's shutdown (`before_shutdown`: mail's IDLE workers). A crate that
  needs a boot step (the shipped reports) adds that hook to `Extension` when it moves; the others start from
  `Extension::EMPTY`. A start the host makes once after boot is a direct call under the host feature, as
  `moqui_llm::start` and `moqui_mail::start_idle`. The binary links it with `use crate_name as _;`
  under the host feature, as `moqui_llm`.
- A service of a framework crate the binary does not link still loads and reports that the feature is not built into
  this host, as a service of a feature left out does now (`service/rust_services.rs` `missing_feature`).
- A crate below core does not know the ExecutionContext. Where it needs the thread's context (the user, authorization,
  scripts, services) it calls a hook table core installs; with none installed each hook answers as "no context
  entered", which is what code sees today outside a context.
- Core re-exports the moved modules at their old paths (`moqui_core::entity`, `transaction`, `driver`,
  `context::Error`), so the crates above keep their imports.
- Do not move code between crates as part of other work; each split here is its own commit, with the before and after
  timings of the touch set below in its message.

## Phases

| Phase | Crate | From | Lines (non-test) | Hooks |
|---|---|---|---|---|
| 0 (done, 9788e711) | | baseline, this plan, the SMTP listener's build without SSH | | |
| 1 (done, 0bda5fdc) | `moqui-git` | core `resource/git/`, `service/git.rs`, `service/git_hosting.rs` | 17,500 | `moqui_core::extension` (new): services, the `git://` resource provider |
| 2 (done, 9aa631f9) | `moqui-mail` | core `service/mail/` except `contacts.rs` (now `service/contacts.rs`) | 13,200 | services, idle stop at shutdown (`before_shutdown`) |
| 3 (done, 4e962f93) | `moqui-entity` (below core) | core `entity/`, `transaction/`, `driver.rs`, `testing.rs`, `context/error.rs`, `resource/{blob,source,textdiff}.rs`, `user/period.rs` | 31,000 | `moqui_entity::hooks::EntityHooks`: ambient, authz, script, services (EECA run), observers |
| 4 (done, 27ab3317) | `moqui-report` | core `report/` (but `time.rs`), `report_schedule.rs`, `monitor.rs` | 10,700 | native registrations, shipped reports after boot (`after_boot`, new) |
| 5 (collab done) | `moqui-collab`, `moqui-dav`, `moqui-devkit` | core `collab/`; `dav/`, `pim/`, elFinder; devtools, typegen, the lint and convert tools, testkit | 1,900; 8,000; 14,000 | services, tools |
| 6 | `moqui-drivers`, `moqui-test-client` | host `db/`; host `test_client/` and test discovery | 9,000; 7,000 | none (direct calls) |

Phase 3 is where the layout departs from Moqui Java, whose entity facade calls the service facade, the artifact stack,
`ec.user`, `ec.l10n` and the expression evaluator directly. The entity crate parses, indexes and matches EECA rules
(`load_feed/eca.rs`) and calls a hook to run the matched ones, which core implements with `service/eca.rs`. The audit
log create, entity-auto REST writes, data-file services and feed receive are hook calls the same way. Behaviour does
not change.

Not split, on purpose: `moqui-lib` (its heavy modules, barcode and signer, are used by core, so an edit there rebuilds
everything anyway); search core and telemetry (hubs, each referenced from about 27 core files); the xlsx render (part of
the render state); the small port traits (pdf, print, sso, backup, sftp); edi, system sync, chat and the `ec.*` script
hosts. SSH protocol code is host `ssh/` (3,600 lines), a hub over the terminal, the TUI, the cluster channel and git;
core's part is the 750-line `sftp.rs` port. It is split only if the host edit timings after Phase 6 call for it, with a
shared host-edge crate (the edge runtime, TLS, the cluster edge, the request-worker handoff) first.

## Phase 3, as done

`moqui-entity` depends only on `moqui-db` and `moqui-lib`, and core depends on it. It has `entity/`, `transaction/`,
`driver.rs`, `testing.rs` (behind its `test-support`, which core's turns on), `error.rs` (core's `context/error.rs`),
`period.rs` (core's `user/period.rs`), and `resource/{blob,source,textdiff,version}.rs`, `version.rs` holding
`ResourceVersion`, `ResourceChangeInfo`, `VersionAuthor` and the version-author thread-local, taken out of
`context/resource.rs` and `context/resource/changes.rs`.

Core keeps its paths: `driver` and `transaction` are re-exports, `context::Error` and `ErrorKind`, `resource::{blob,
source, textdiff, version}`, `user::period_range` and `telemetry::redact_sql` too. Core's `entity` module is its own
(`pub use moqui_entity::entity::*`) and adds `open_for_factory`, which was an `EntityFacade` associated function and is
now `moqui_core::entity::open_for_factory` (the host and the browser call it).

The hook table is `moqui_entity::hooks::EntityHooks`, one struct of fn pointers that core submits through inventory
(`crates/moqui-core/src/entity_hooks.rs`); `hooks::get()` answers `EntityHooks::NONE` when none is linked, which is what
code outside an entered context saw before. Its entries are the ones listed in the plan, plus:

- `inline_executor` in place of a flag on the TransactionManager for `single_threaded`: managers are made before the
  factory that knows its executor, and the check was per entered context anyway.
- `trace_statement`, returning a `StatementTrace` that holds the span, its attached context and the duration metric;
  the statement counters and the slow-statement warning stay in `entity/exec.rs`, and so does `redact_sql`.
- `locate`, whose no-context answer resolves a plain or `file:` path as core did (`moqui_entity::resource::file_path`,
  which core's `context/locate.rs` uses too).
- the script entries are frame push and pop, set name, eval, expand, context value and the map and items of a script
  value, so each call site keeps its shape (`hooks::ScriptFrame`).

Smaller changes: the backup runner is a facade slot (`moqui_core::backup::set_runner` and `runner`, called by the host
and `service/backup.rs`); the sites that read `facade.inner` use `EntityFacade::groups`, `default_group` and
`identity`; `db_system` is an accessor; `rule_actions::first_condition` is `entity::load_feed::first_condition`; about
40 entity items core uses are `pub`, and a few only core's tests use (`FkScope`, `expand_prefix`,
`EntityFind::conditions`, `EntityValue::from_read_fields`, `transaction::single_threaded`) are `pub` behind
`test-support` with `#[doc(hidden)]`.

Tests: 232 moved with the crate and pass there with no hooks linked. Those that need an entered context, the script
runtime or a service (a field default that is an expression, the audit-log service, ECA rules, `store_entity`, the
session time zone and locale, the shared entity values of a script) are in core, `crates/moqui-core/src/entity/
{tests, load_feed_tests, search_form_tests, dbresource_tests, field_rw_tests, document_tests}.rs`, with copies of the
fake drivers they use. The blob and most dbresource tests entered a context only for authorization and now run
without one.

What Phase 3 showed about a move below core:

- Rewrite the call sites to the hook table while the code is still in core (it compiles and its tests run unchanged),
  then move; the move is then the visibility changes the compiler names and the test split.
- A test that compiles in the lower crate can still need core at run time: run the crate's tests and move those that
  fail on a hook's no-context answer ("no script runtime", "no service facade").
- `cargo fix` clears the imports the split leaves unused; a scripted split that copies helpers can copy one twice.

## Phase 4, as done

`moqui-report` is the crate root of core's `report/mod.rs`, with its modules beside it, `schedule.rs` (core's
`report_schedule.rs`) and `monitor.rs`. Its `Extension` registers the three modules' native services and loads the
components' shipped definitions in a new entry, `after_boot`, which the factory calls for each linked crate after
`native.configure` (where `report::load_shipped` was called). The service locations stay `moqui_core::report::…`,
`moqui_core::report_schedule::…` and `moqui_core::monitor::…`: they are the strings the registration builds with
`format!`, so the service files are unchanged, and a test in the crate checks the registry against them. It is linked
in every build: moqui-host depends on it (its HTTP monitor receiver and release record call `moqui_report::` directly)
and moqui-browser names it. `report::time` is core's `search::time`, since search compiles relative times with it.

What Phase 4 showed: a crate with a format of its own needs the feature of its own (`xlsx`, the xlsx format of a
scheduled export, default on, the workspace entry without defaults so the browser leaves it out, the host's `xlsx`
turning it on); the core items the crate uses (`value_millis`, `job_time_zone`, `webapp_root_url`, `secret_parameter`,
`search::limits::remaining` and a few more) are `pub`; core's tests needed nothing of the reports, and the host's
report tests import `moqui_report as report`.

## Phases 5 and 6 in detail

- **5, one commit each, register-only:** `moqui-collab` (done: `collab/mod.rs` as the crate root and
  `service/collab.rs` as `moqui_collab::service`, whose two locations in `CollabServices.xml` are `moqui_collab::service::`;
  its feature is `collab`, in `extension::LOCATIONS`, so the browser build, which leaves it out, reports it not built in;
  the host depends on it, since its notification WebSocket calls the rooms; yrs left core and the browser build, and
  `user::session::Identity` and its save and restore are `pub`); `moqui-dav` (`dav/`, `pim/` and `service/elfinder*`, about 8,000 lines; the pim tool classes keep their
  names); `moqui-devkit` (devtools minus `is_dev_instance`, `restart`, `log_capture` and `index/calls`, which stay in
  core on hot paths; `typegen/`; `script/{screen_lint, groovy_convert, lint_hazards}`; `testkit/`; about 14,000 lines,
  and notify leaves core). The devkit is linked in every package, because `moqui test`, `lint` and `types` are; only its
  file watcher is dev-only. moqui-llm's `check_text` and `SpecFile` uses then depend on it.
- **6, after measuring host edits again:** `moqui-drivers` (host `db/`, about 9,000 lines, with `build.rs`, the 11.8 MB
  embedded archive and `xmlshim`; no tokio; the `SearchStore` parameter of `db/backup.rs` becomes a closure, or backup
  stays in the host; the host forwards `postgres`, `mysql` and `embedded-postgres`), so a host edit stops rewriting a
  68 MB rlib; `moqui-test-client` (`test_client/` with `test_cmd`'s discovery, selection and report, in every package as
  now). SSH only if the numbers justify it, as above.
- Also drop `moqui-lib`'s unused `hmac` 0.12 and `sha1` 0.10 dependencies.

## Adding a crate

- Root `Cargo.toml` `members` and `[workspace.dependencies]`; `Cargo.lock`.
- The host's feature that links it (and the package features that include that one), and `use crate_name as _;` under
  it in `crates/moqui-host/src/lib.rs` when nothing else names the crate.
- `moq` `write_app_crate`: the workspace crates kept at opt-level 0 in the generated `target/moqui-app`.
- `scripts/check-packages.sh` when the crate has features of its own.
- The Features table of [Build-Test](../docs/Build-Test.md) for a new feature; `[lints] workspace = true`; the
  crate's purpose in a comment at the head of its `Cargo.toml` and in its `lib.rs` `//!` doc (the crates have no
  README).

What Phases 1 and 2 showed about a move above core:

- `rust_service!` keys a service by `module_path!`, so the `location` of each moved service changes in the framework's
  service XML (`moqui_git::service::...`, `moqui_mail::...`); add the crate's prefixes to `extension::LOCATIONS` (with
  the finer per-service entries when the crate has a feature of its own, as mail's `smtp` and `imap`), so a binary
  without it still reports the feature. Add a test in the crate that compares its registry with the locations the
  service files name (`moqui-git` `tests.rs`, `moqui-mail` `lib.rs`), and grep the component repositories for the old
  locations.
- `git mv` the files, then rewrite `crate::` to `moqui_core::` (and the moved module's own paths to `crate::`) with a
  script, then let the compiler name what must become `pub` in core; a helper only the tests use goes behind
  `#[cfg(any(test, feature = "test-support"))] #[doc(hidden)] pub`.
- Core cannot dev-depend on a crate above it (two copies of core, and its inventory entries in the wrong one), so the
  core tests that need the moved code move with it, and core's own tests check that its locations load and report the
  feature missing (`rust_services_tests.rs`, `define_tests.rs`). A test that passed in core only because an earlier
  test warmed a process cache fails in the smaller binary (the PDF preview did): give it its own setup.
- Rewrite the feature gates: the crate is its base feature (mail is `smtp`, with `imap` its own); delete the gates
  that are now always on, and list the crate's other feature sets in `scripts/check-packages.sh`.

## Resuming

At the start of a session: `git log --oneline -8` for where the phases ended; read this file, Practices-Rust §1 and §8,
and `crates/moqui-core/src/extension.rs`; re-take the "Before" timings of the next phase's touch files, since other work
changes them. Each phase is its own commit with the before and after timings in its message; commit when the user says.

Verification of a phase:

- `cargo lint`, `cargo lint-min` and `scripts/check-packages.sh` (Clippy on every package's feature set);
- `cargo test -p <new crate>`, `cargo test -p moqui-core`, `cargo test -p moqui-host --test it`, `cargo test -p
  moqui-llm`; or `cargo test --workspace` (about 80 s with its build since e98c1c8e);
- `./moq build`, then `./moq test-all` (every suite but the slow ones; never `--slow` without the user's permission,
  AGENTS.md) or, for a narrower check, `./moq test <component>`;
- the wasm browser build when core's features or the entity facade change;
- `cargo tree -p moqui-core -e normal` to confirm the dependencies that left;
- the touch set below, before and after, in the commit message and the table.

Test infrastructure since Phase 2: a host integration test is a module of `crates/moqui-host/
tests/it`, not a new file under `tests/`; one that starts a server begins with `if !crate::common::own_process() {
return; }` (78b74633); a test over about 15 seconds is for special cases and one over 30 seconds is marked slow
(docs/Build-Test.md "Slow tests"); the dev profile keeps dependency debug info to line tables and splits the rest out
of the binaries (e98c1c8e); the `cargo test --no-run` row below was measured again under it for Phase 3.

## Measuring

The touch set: one probe edit (a dead private function appended, then the file restored) in each of
`entity/value.rs` (moqui-entity since Phase 3), core `service/call.rs`, `screen/render.rs`, `resource/git/mod.rs`,
`service/mail/implementation.rs` (the send path; `sync.rs` is `imap`, which the Dev package leaves out), `report/mod.rs`
(moqui-report's `lib.rs` since Phase 4), `collab/mod.rs` (moqui-collab's `lib.rs` since Phase 5), host `http.rs` and
`db/pgwire.rs`, each rebuilt twice with `cargo build --profile develop --no-default-features --features dev -p
moqui` (the Dev package, `./moq build`), on a warm cache with no other build running. A file that moved is probed at
its new path. To take the "Before" column of a phase with its work in the tree, commit the work on a branch, switch
back to master's HEAD for the probe, and switch back. The probe, for each file (restore the file afterwards, and build
once more so the cache is warm for the next):

```sh
cp "$f" "$f.orig"
for n in 1 2; do
    cp "$f.orig" "$f"
    printf '\n#[allow(dead_code)]\nfn timing_probe_%s() -> u64 {\n    %s\n}\n' "$n" "$RANDOM" >> "$f"
    start=$(date +%s.%N)
    cargo build --profile develop --no-default-features --features dev -p moqui -q
    echo "$f run$n $(echo "$(date +%s.%N) - $start" | bc) s"
done
mv "$f.orig" "$f"
```

Add `--timings` to one run to see where an edit's time goes (`target/cargo-timings/`).

| Edit in | Before (2026-10-29) | After Phase 1 | After Phase 2 | Before Phase 3 (86c9e10e) | After Phase 3 (4e962f93), before Phase 4 | After Phase 4 |
|---|---|---|---|---|---|---|
| report `lib.rs` (core `report/mod.rs` before) | | | | | 10.6 s, 10.4 s | 2.5 s, 2.5 s |
| `entity/value.rs` (entity since Phase 3, core before) | 16.4 s, 15.2 s | 13.1 s, 13.0 s | 13.7 s, 13.7 s | 14.2 s, 13.9 s | 10.6 s, 10.5 s | 10.5 s, 10.5 s |
| core `service/call.rs` | 15.5 s, 15.5 s | 13.5 s, 13.4 s | 13.7 s, 13.6 s | 13.8 s, 14.7 s | 10.8 s, 9.9 s | 9.8 s, 9.8 s |
| core `screen/render.rs` | 15.1 s, 14.8 s | 13.0 s, 13.0 s | 13.8 s, 13.7 s | 13.9 s, 13.9 s | 9.9 s, 9.9 s | 9.9 s, 9.9 s |
| git `lib.rs` (core `resource/git/mod.rs` before) | 15.1 s, 15.0 s | 2.5 s, 2.5 s | 2.5 s, 2.5 s | 2.7 s, 2.6 s | 2.6 s, 2.6 s | 2.5 s, 2.6 s |
| mail `implementation.rs` (core `service/mail/implementation.rs` before) | | 13.2 s, 13.1 s | 2.2 s, 2.2 s | 2.2 s, 2.2 s | 2.2 s, 2.3 s | 2.8 s, 2.2 s |
| host `http.rs` | 2.0 s, 1.9 s | 2.0 s, 1.9 s | 2.0 s, 2.0 s | 2.0 s, 2.0 s | 2.0 s, 2.1 s | 2.0 s, 2.0 s |
| host `db/pgwire.rs` | 2.5 s, 2.3 s | | | 2.5 s, 2.0 s | 2.0 s, 3.0 s | 2.2 s, 2.0 s |
| `cargo test -p moqui-core --no-run`, core `service/call.rs` | 9.5 s, 9.7 s | 9.5 s, 8.7 s | (not taken) | 7.7 s, 7.7 s | 6.7 s, 7.0 s | 6.3 s, 6.2 s |

Phase 5, one crate at a time (the edits of the main table unchanged within the spread of these runs: core
`service/call.rs` and `screen/render.rs` 9.8 to 10.0 s):

| Edit in | Before | After |
|---|---|---|
| collab `lib.rs` (core `collab/mod.rs` before) | 9.8 s, 9.8 s (27ab3317) | 2.6 s, 2.2 s |

Where a core edit's 15 s goes (`--timings`, `service/call.rs`): `moqui-core` 14.5 s, its metadata ready at about 5 s, so
`moqui-llm` (2.1 s) and `moqui-host` (3.1 s) finish while core is still in codegen, and the binary links (0.3 s) after
core. Core's own compile is the whole critical path; a host edit is 2 s. Measured on a 24-thread Linux machine, Rust
1.99.

After Phase 1, the same edit: `moqui-core` 12.6 s, then `moqui-git` (1.0 s), `moqui-llm` (3.3 s) and `moqui-host`
(1.9 s) beside its codegen, and the binary link (0.3 s).

After Phase 2, the same edit: `moqui-core` 13.3 s, then `moqui-mail` (0.2 s), `moqui-git` (0.6 s), `moqui-llm` (2.4 s)
and `moqui-host` (3.0 s) beside its codegen, and the binary link (0.3 s). The Dev package has only mail's send half,
so a core edit there is unchanged within the spread of these runs; what the phase buys is an edit in mail (13.2 s to
2.2 s), and core no longer compiles lettre, imap and mail-parser, nor the receive half in the packages with `imap`.

Before Phase 3, the same edit: `moqui-core` 13.4 s, with `moqui-llm` (2.6 s) and `moqui-host` (3.0 s) beside it.
After Phase 3: `moqui-core` 9.4 s, then `moqui-git` (0.6 s), `moqui-llm` (2.1 s) and `moqui-host` (1.9 s) beside its
codegen, and the binary link (0.3 s). An edit in the entity facade compiles `moqui-entity` (0.8 s) and then core
(9.5 s): core depends on it, so it still rebuilds core, but core's own frontend is smaller by the 31,000 lines, and
every core edit is about 4 s shorter.

After Phase 4, the same edit: `moqui-core` 9.3 s, then `moqui-report` (0.5 s), `moqui-git` (0.7 s), `moqui-llm`
(2.2 s) and `moqui-host` (1.9 s) beside its codegen, and the binary link (0.3 s); a core edit is unchanged, since the
report code was not on its critical path. An edit in the reports is `moqui-report` (0.8 s), `moqui-host` (1.7 s) and
the link: 10.5 s to 2.5 s.

So the host splits of Phase 6 save little on an edit (the host is 2 s) and are for modularity and the clean build; a
core edit is now about 10 s, all of it core's own compile.
