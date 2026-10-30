# Crate Split

Status 2026-10-29: Phase 0 done (baseline below); Phase 1 next. Splits `moqui-core` along hook seams, above it for the tool subsystems and
below it for the entity facade, and then the host, so an edit rebuilds less and more of the build runs in parallel. It
revises [Crate-Layout](archive/Crate-Layout.md), whose "few, larger crates" rule this replaces
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
  inventory entry with its Rust services, its native registrations (tools, resource providers, runners) and its boot
  and shutdown steps. The binary links it with `use crate_name as _;` under the host feature, as `moqui_llm`.
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
| 0 | | baseline, this plan, the SMTP listener's build without SSH | | |
| 1 | `moqui-git` | core `resource/git/`, `service/git.rs`, `service/git_hosting.rs` | 17,500 | `moqui_core::extension` (new): services, the `git://` resource provider |
| 2 | `moqui-mail` | core `service/mail/` except `contacts.rs` | 13,400 | services, idle stop at shutdown |
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

## Adding a crate

- Root `Cargo.toml` `members` and `[workspace.dependencies]`; `Cargo.lock`.
- The host's feature that links it (and the package features that include that one), and `use crate_name as _;` under
  it in `crates/moqui-host/src/lib.rs` when nothing else names the crate.
- `moq` `write_app_crate`: the workspace crates kept at opt-level 0 in the generated `target/moqui-app`.
- `scripts/check-packages.sh` when the crate has features of its own.
- The Features table of [Build-Test](../docs/Build-Test.md) for a new feature; `[lints] workspace = true`; the
  crate's purpose in a comment at the head of its `Cargo.toml` and in its `lib.rs` `//!` doc (the crates have no
  README).

## Measuring

The touch set: one probe edit (a dead private function appended, then the file restored) in each of core
`entity/value.rs`, `service/call.rs`, `screen/render.rs`, `resource/git/mod.rs`, `service/mail/sync.rs`, host
`http.rs` and `db/pgwire.rs`, each rebuilt twice with `cargo build --profile develop --no-default-features --features
dev -p moqui` (the Dev package, `./moq build`), on a warm cache with no other build running. A file that moved is
probed at its new path.

| Edit in | Before (2026-10-29) |
|---|---|
| core `entity/value.rs` | 16.4 s, 15.2 s |
| core `service/call.rs` | 15.5 s, 15.5 s |
| core `screen/render.rs` | 15.1 s, 14.8 s |
| core `resource/git/mod.rs` | 15.1 s, 15.0 s |
| core `service/mail/sync.rs` | 0.2 s (`imap` is not in the Dev package, so the file is not compiled) |
| host `http.rs` | 2.0 s, 1.9 s |
| host `db/pgwire.rs` | 2.5 s, 2.3 s |
| `cargo test -p moqui-core --no-run`, core `service/call.rs` | 9.5 s, 9.7 s |

Where a core edit's 15 s goes (`--timings`, `service/call.rs`): `moqui-core` 14.5 s, its metadata ready at about 5 s, so
`moqui-llm` (2.1 s) and `moqui-host` (3.1 s) finish while core is still in codegen, and the binary links (0.3 s) after
core. Core's own compile is the whole critical path; a host edit is 2 s. Measured on a 24-thread Linux machine, Rust
1.99.

So the host splits of Phase 6 save little on an edit (the host is 2 s) and are for modularity and the clean build;
they wait on the numbers after Phase 3.
