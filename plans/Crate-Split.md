# Crate Split

Status 2026-10-30: Phases 0 to 2 done (9788e711, 0bda5fdc, 9aa631f9: `moqui_core::extension`, `moqui-git`,
`moqui-mail`); Phase 3 next ([Phase 3 in detail](#phase-3-in-detail), [Resuming](#resuming)). Splits
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
| 3 | `moqui-entity` (below core) | core `entity/`, `transaction/`, `driver.rs`, `testing.rs`, `context/error.rs`, `resource/{blob,source,textdiff}.rs` | 31,000 | `EntityHooks`: ambient, authz, script, services (EECA run), observers |
| 4 | `moqui-report` | core `report/`, `report_schedule.rs`, `monitor.rs` | 10,700 | native registrations, shipped reports after boot |
| 5 | `moqui-collab`, `moqui-dav`, `moqui-devkit` | core `collab/`; `dav/`, `pim/`, elFinder; devtools, typegen, the lint and convert tools, testkit | 1,900; 8,000; 14,000 | services, tools |
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

## Phase 3 in detail

`moqui-entity` depends only on `moqui-db` and `moqui-lib`, and core depends on it. It gets:

- all of `entity/`, except `EntityFacade::open_for_factory` (`entity/facade.rs`), which becomes a core free function;
- `transaction/` and `driver.rs`;
- `testing.rs`, the in-memory facade, behind `test-support`;
- `context/error.rs` (`Error`, `ErrorKind`);
- what dbresource needs: `resource/blob.rs`, `source.rs` and `textdiff.rs`, with `ResourceVersion`,
  `ResourceChangeInfo`, `VersionAuthor` and the thread-local of `context/resource/changes.rs`. Moving them is simpler
  than turning `impl EntityFacade` blocks into an extension trait.

The research behind it: EntityFacade, EntityValue and EntityFind hold no ExecutionContext. Every reach into core goes
through the ambient thread-local (`ExecutionContext::with_current`) or a free function that wraps it: about 70 call
sites in about 20 files. `transaction/` and `driver.rs` use only `context::Error` and one executor check.

**Core keeps its paths.** It re-exports `pub use moqui_entity::{entity, driver, transaction}`, `context::Error` and
`resource::{blob, source, textdiff}`, so the host (about 118 references), moqui-llm (about 115), dev-aide and
moqui-browser keep their `moqui_core::entity::...` paths.

**One hook table**, `moqui_entity::hooks::EntityHooks`, which core submits through inventory (as `llm_hooks`). When none
is linked, each entry answers as "no context entered", the branch the context-free tests already exercise. The entries:

- ambient: entered, the current facade, effective now, user, visit and agent id, in-group, locale and time zone (for an
  entity-local `convert` that replaces `l10n::basic_convert`), period range, entity ECA disabled, conf setting, locate,
  messages and errors, and the no-search-params flag;
- authz: disabled, a disable guard, the push and pop of `AT_ENTITY` (`entity/value.rs`), hit detail, stack names, the
  find and view filters (`entity/find.rs`, `entity/query_rest.rs`) and `note_write` (`entity/value.rs`);
- script: eval, expand, context value, and the object map and items;
- services: `run_eca`, which runs core `service/eca.rs`. Rule parsing and matching (`load_feed/eca.rs`) moves down, and
  `rule_actions::first_condition` with it. The others are the audit-log create (`entity/field_rw.rs`), entity-auto REST
  writes (`entity/query_rest.rs`), `store_entity`, data-file services, the service name and registration failures (the
  loader), and feed receive;
- observers: the cluster cache-invalidation note (`entity/cache.rs`) and the SQL span and duration (`entity/exec.rs`).

Smaller changes: `transaction`'s `single_threaded` becomes a flag core sets on the TransactionManager; `backup_runner`
becomes a `with_slot` entry; the six sites that read `facade.inner.groups` or `default_group` directly get accessors
(`report_schedule.rs`, `service/pool.rs`, `testkit/run.rs`, `service/request_pool.rs`, `script/runtime.rs`,
`context/status.rs`); about 35 `pub(crate)` entity items that core uses become `pub` (`exec::{execute_tx,
with_reserve_tx}`, `find_by_pk`, the generation counters).

Order within the phase, one checkable step at a time: (1) `error.rs`, `driver.rs` and `transaction/`; (2) the hook table
and the call-site rewrite, done inside core while entity is still a module there; (3) the move itself; (4) the tests.
The cache, document, data_writer, find, etl, fence, sequence, datasource and transaction tests move unchanged; about 23
tests that call `test_context` stay in core as `entity_ec_tests` (in `entity/tests.rs`, `load_feed/tests.rs`,
`search_form_tests`, `dbresource_tests`, `field_rw_tests` and the `exec.rs` inline tests). Build the wasm browser
(`scripts/wasm-env.sh`, then moqui-browser) after the move: it links core without features.

## Phases 4 to 6 in detail

- **4, `moqui-report`:** `report/`, `report_schedule.rs` and `monitor.rs` (about 10,700 lines every package compiles).
  Hooks: `register_native` (the report registrations in `native.rs`) and a new `after_boot` entry for `load_shipped`
  (`context/factory.rs`, after `native.configure`). The XML locations stay, since they are `format!` strings.
  `report::time` stays in core, beside search (`search/postgres/compile.rs`).
- **5, one commit each, register-only:** `moqui-collab` (`collab/` and `service/collab.rs`; yrs leaves core and the wasm
  build); `moqui-dav` (`dav/`, `pim/` and `service/elfinder*`, about 8,000 lines; the pim tool classes keep their
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

Test infrastructure since Phase 2 (in place for Phase 3): a host integration test is a module of `crates/moqui-host/
tests/it`, not a new file under `tests/`; one that starts a server begins with `if !crate::common::own_process() {
return; }` (78b74633); a test over about 15 seconds is for special cases and one over 30 seconds is marked slow
(docs/Build-Test.md "Slow tests"); the dev profile keeps dependency debug info to line tables and splits the rest out
of the binaries (e98c1c8e), so the `cargo test --no-run` row below must be measured again before it is compared.

## Measuring

The touch set: one probe edit (a dead private function appended, then the file restored) in each of core
`entity/value.rs`, `service/call.rs`, `screen/render.rs`, `resource/git/mod.rs`, `service/mail/implementation.rs`
(the send path; `sync.rs` is `imap`, which the Dev package leaves out), host `http.rs` and `db/pgwire.rs`, each rebuilt
twice with `cargo build --profile develop --no-default-features --features dev -p moqui` (the Dev package, `./moq
build`), on a warm cache with no other build running. A file that moved is probed at its new path. The probe, for each
file (restore the file afterwards, and build once more so the cache is warm for the next):

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

| Edit in | Before (2026-10-29) | After Phase 1 | After Phase 2 |
|---|---|---|---|
| core `entity/value.rs` | 16.4 s, 15.2 s | 13.1 s, 13.0 s | 13.7 s, 13.7 s |
| core `service/call.rs` | 15.5 s, 15.5 s | 13.5 s, 13.4 s | 13.7 s, 13.6 s |
| core `screen/render.rs` | 15.1 s, 14.8 s | 13.0 s, 13.0 s | 13.8 s, 13.7 s |
| git `lib.rs` (core `resource/git/mod.rs` before) | 15.1 s, 15.0 s | 2.5 s, 2.5 s | 2.5 s, 2.5 s |
| mail `implementation.rs` (core `service/mail/implementation.rs` before) | | 13.2 s, 13.1 s | 2.2 s, 2.2 s |
| host `http.rs` | 2.0 s, 1.9 s | 2.0 s, 1.9 s | 2.0 s, 2.0 s |
| host `db/pgwire.rs` | 2.5 s, 2.3 s | | |
| `cargo test -p moqui-core --no-run`, core `service/call.rs` | 9.5 s, 9.7 s | 9.5 s, 8.7 s | (not taken; the dev profile changed in e98c1c8e) |

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

So the host splits of Phase 6 save little on an edit (the host is 2 s) and are for modularity and the clean build;
they wait on the numbers after Phase 3.
