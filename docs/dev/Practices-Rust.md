# MR Moqui Rust practices

A guide for agents and people writing Rust in MR Moqui: the workspace crates (`crates/*`), the framework's component
crates (`runtime/dev-component/example`, `runtime/dev-component/dev-aide`) and add-on component crates. It covers where
code goes, how a crate is organized, how tests are kept, and the idioms to use. It follows
[Google's Rust style guide](https://google.github.io/styleguide/rustguide.html) where that helps this code, and says
where MR Moqui departs from it on purpose (§8).

The review that applies these practices to the existing code, with its targets and results, is
[Rust-Practices-Review](../../plans/Rust-Practices-Review.md).

Contents: [1 crates](#1-crates-and-where-code-goes) · [2 organization](#2-organization-within-a-crate) ·
[3 tests](#3-tests) · [4 working on rust code](#4-working-on-rust-code) ·
[5 unwraps and invariants](#5-unwraps-and-invariants) · [6 ownership and borrowing](#6-ownership-and-borrowing) ·
[7 idiom](#7-idiom) · [8 departures from google](#8-where-mr-moqui-departs-from-google) ·
[9 checklist](#9-checklist)

## 1. Crates and where code goes

One workspace, one `src/` tree per crate. The cut and its reasons are in
[Crate-Layout](../../plans/archive/Crate-Layout.md), and the splits under way in
[Crate-Split](../../plans/Crate-Split.md). Dependencies point one way:

```
moqui (bin) → moqui-host → moqui-llm, moqui-git, moqui-mail, moqui-tui (optional features), moqui-report, moqui-collab, moqui-dav, moqui-devkit
              moqui-host → moqui-drivers → moqui-core
              moqui-host → moqui-core → moqui-entity → moqui-db → moqui-lib
moqui-llm → moqui-devkit
moqui-llm, moqui-git, moqui-mail, moqui-report, moqui-collab, moqui-dav, moqui-devkit → moqui-core
moqui-browser → moqui-core (default-features = false), moqui-report, moqui-dav
```

| Crate | What belongs there |
| --- | --- |
| `moqui-lib` | Helpers that need no ExecutionContext and know nothing of the framework: text, numbers, dates, XML nodes, barcodes, `sync`. |
| `moqui-db` | The entity model, SQL text and dialects. No facades and no database driver. |
| `moqui-entity` | The entity facade, the transaction manager and the `DbDriver` port. No ExecutionContext: what it needs of the thread's context (the user, authorization, scripts, services, entity ECA rules) it calls through `moqui_entity::hooks`, which core fills. Core re-exports it at `moqui_core::entity`, `transaction`, `driver` and `context::Error`; code above core uses those paths. |
| `moqui-core` | The framework: the ExecutionContext and facades, screens, services, scripts. Synchronous, on `std::thread`. |
| `moqui-llm` | The agent runtime and model providers. Core reaches it only through the hook traits in `moqui_core::llm_hooks`, so an edit here does not rebuild the framework. |
| `moqui-git` | `git://` locations, `org.moqui.impl.GitServices` and the git server, on gitoxide. A framework crate: core reaches it only through its `moqui_core::extension` entry ([Crate-Split](../../plans/Crate-Split.md)). |
| `moqui-mail` | The email services: outbound SMTP and, under its `imap` feature, IMAP and POP3, the mail client's sync, IDLE, import and OAuth sign-in, and the delivery of mail the host's SMTP listener received. A framework crate, as `moqui-git`. |
| `moqui-report` | Reports, dashboards and their alerts and annotations, scheduled report delivery and the monitoring sources' ingest keys. A framework crate, as `moqui-git`, linked in every build and in the browser. |
| `moqui-collab` | Co-edited documents (a Yjs document per record, on yrs) and `org.moqui.impl.CollabServices`. A framework crate, as `moqui-git`, linked in every host build and left out of the browser. |
| `moqui-dav` | The DAV server (WebDAV over the drives, CalDAV and CardDAV), the `VCard` and `ICalendar` tools and the elFinder connector. A framework crate, as `moqui-git`, linked in every build and in the browser. |
| `moqui-devkit` | The development kit: reload and the file watcher (its `watch` feature), `moqui check`, the artifact index, the Groovy converter, typegen (`moqui types`) and the spec runner (`moqui test`). A framework crate, linked in every host build; what core itself needs (`is_dev_instance`, restart, the observed calls) stays in `moqui_core::devtools`. |
| `moqui-tui` | The terminal client of the `tui` render mode. It does no I/O; the host's link answers its calls. |
| `moqui-drivers` | The database drivers: Postgres on the host's own blocking client, MySQL, and the embedded Postgres cluster with its archive and backups, with how the host opens the entity facade over them. No Tokio. The host links it as `moqui_host::db` and forwards its features. |
| `moqui-host` | The process edge: the HTTP server (Tokio, axum), process spawn, the integration tests. |
| `moqui-browser` | The browser host: a wasm32 worker on SQLite and fetch. |
| `moqui` | The binary: the CLI and the build packages and their features ([Packaging](../Build-Test.md#packages)). |

Component crates (`example`, `dev-aide`) are workspace members that the binary links under its `devtools` feature. An
add-on component's crate, a `Cargo.toml` beside its `component.xml`, is not a member: `./moq build` links it through a
generated crate in `target/moqui-app` ([Native-Components](Native-Components.md)).

- Put code in the lowest crate that can hold it. A helper that needs no ExecutionContext goes in `moqui-lib`.
- Never call up the chain. When a lower crate needs something a higher one supplies, define a trait or hook in the
  lower crate and install the implementation from above, as `moqui_core::llm_hooks` does for `moqui-llm` and core's
  `entity_hooks.rs` does for `moqui_entity::hooks`. A crate that only adds services, tools and resource providers
  submits a `moqui_core::extension::Extension` (`moqui-git`, `moqui-mail`, `moqui-report`, `moqui-collab`, `moqui-dav`, `moqui-devkit`).
- An entity facade test that needs an entered ExecutionContext, a script or a service is in core
  (`crates/moqui-core/src/entity/*_tests.rs`); the others are in `moqui-entity`, where no hooks are linked and each
  hook answers as with no context entered.
- Do not move code between crates as part of other work; a new crate or a move needs its own plan
  ([Crate-Split](../../plans/Crate-Split.md) is the current one, with the seam rules and the files a new crate touches).
- A feature-gated module has an off stub with the same API, so callers need no `cfg` of their own:

  ```rust
  #[cfg(feature = "xlsx")]
  pub(crate) mod xlsx;
  #[cfg(not(feature = "xlsx"))]
  #[path = "xlsx_off.rs"]
  pub(crate) mod xlsx;
  ```

  Check feature sets with `cargo lint-min` (the host without default features) and `scripts/check-packages.sh`
  (clippy on each package's feature set).

## 2. Organization within a crate

- **One concept per module.** A file is about one thing: a facade, a parser, a protocol, a screen widget family. When a
  module grows parts, make it a directory module (`foo/mod.rs` with `foo/bar.rs`), with `mod.rs` holding the shared
  types and the public entry points.
- **Split large files along their seams.** When a file's non-test code passes about 1,500 lines, look for the parts
  that can stand alone (a parser and its writer, the request side and the response side, one widget family) and make
  them submodules. Several files are 2,000–3,000 lines today: split one when you are doing real work in it, not as a
  separate churn commit.
- **Visibility.** `pub` is the crate's supported API; use `pub(crate)` for what other modules of the crate use, and
  keep the rest private.
- **Module docs.** Start a module with a `//!` doc that says what it is and cites the plan or doc it implements
  (`//! The terminal client of the tui render mode (plans/archive/Screen-TUI-Impl.md section 3).`). Keep the CC0
  header that every source file has.
- **Extension points** that have a second implementation, or that the host must supply, are traits in the module that
  uses them, not in a separate interface module.

## 3. Tests

Keep tests out of the code file. The source then stays readable, a test change does not churn the source's diff, and
a reader (or an agent) loading the code does not load its tests too.

- **Unit tests** go in a sibling file, wired at the end of the module:

  ```rust
  #[cfg(test)]
  #[path = "object_tests.rs"]
  mod tests;
  ```

  In a directory module use `foo/tests.rs`, or a `foo/tests/` directory when there are many (`moqui-core`'s
  `entity/tests.rs`, `screen/tests/`).
- **Inline `mod tests { }`** is only for a few small tests (roughly under 100 lines) in a small file. When you work in a
  file with a large inline test module, move it out to `<name>_tests.rs` first, in its own commit.
- **Integration tests** go in the crate's `tests/`. In `moqui-host` every test binary links the whole host, so add a
  module to the single `tests/it/` binary (see `tests/it/main.rs`) rather than a new file in `tests/`. Shared helpers
  are in `tests/common/`. A slow or opt-in suite (the Gate 3 browser tests, the speed tests) is a `[[test]]` with
  `test = false` in `Cargo.toml`, run by name.
- **Slow tests.** A test over about 15 seconds is slow and only for a special case (a specific scenario, not
  coverage); write tests to run in well under that. One over 30 seconds is `#[ignore = "slow: ..."]` (`cargo
  test-slow` runs those), or an opt-in `[[test]]` that `./moq test-all` runs only with `--slow` (`gate3_browser`).
  Never run one without the user's permission ([Build-Test](../Build-Test.md#slow-tests), AGENTS.md).
- **Fixtures and goldens** go in a `testdata/` directory beside the code that reads them. A golden is rewritten by an
  environment variable, never by hand (`UPDATE_GOLDEN=1`, `MOQUI_REGENERATE_VECTORS=1`), and the diff is read before
  it is committed.
- Tests may `unwrap`, `expect` and index; the rules of §5 are for non-test code.
- A test that needs a process property or starts a server runs in a process of its own, beside the others: in the
  host it begins with `if !crate::common::own_process() { return; }` (`tests/common`), and the gate starts and
  `common::serial` fail without it. A server's boot writes process-wide properties, so two in one process would share
  them; a test that only loads data with no property takes `common::shared` in the test binary's process.
- JavaScript specs (`*.spec.js`) run under `moqui test` and `moqui test-client`, not Node ([Testing](../Build-Test.md#component-specs),
  [Client-Testing](Client-Testing.md)).

## 4. Working on Rust code

- **Behaviour.** A cleanup does not change behaviour. If a change does change what a caller sees, say so in the commit
  message. An error returned instead of a panic is such a change, and a wanted one.
- **Checks.** After a change run `cargo lint` (clippy on the workspace, warnings denied), `cargo lint-min` when
  `moqui-host` changed, and `cargo test -p <crate>` for each crate touched. The workspace already denies Clippy's
  default set; pedantic, nursery and restriction lints stay off. Do not add lints to `[workspace.lints]` that the code
  does not already pass.
- **Toolchain.** Use one toolchain and incremental development builds so the cache stays warm
  ([building skill](../../.agents/skills/building/SKILL.md)).
- **Formatting.** `max_width = 120`, edition 2021 (`rustfmt.toml`). Format only the files you changed:
  `rustfmt --edition 2021 --config-path rustfmt.toml <files>`, then check `git diff -U0` for hunks that are not yours
  and put them back. Never run `cargo fmt` on a whole crate.
- **Failure tolerance.** When one independent piece fails (a DDL statement, a data file, a job, a service file, a row
  that will not decrypt) log an error naming it and go on with the rest; do not `?` out of a loop over independent
  items. Stay strict where it protects security or data.
- **Comments** follow the repo's plain style: say why, cite the plan or doc section, and do not narrate the change.

## 5. Unwraps and invariants

Match on a value instead of checking it and then unwrapping it. When a panic is right (a bug, a broken invariant), use
`expect` and give the reason the value is there.

Choose by where the value comes from:
- **Input** (a request, JSON, a file, a database row, conf): never panic. Return the function's existing error with
  `let ... else`, or with `?` on `ok_or_else`. Use the error type the function already returns; do not add a new one.
- **An invariant this code keeps** (a key inserted just above, a `TypeId` match): restructure the code so the compiler
  can see the invariant (`get_or_insert_with`, `entry`, bind once). If that is not possible, use
  `expect("why it holds")`.
- **Truly infallible** (`write!` into a `String`): match the file's existing form.

### 5a. Check, then unwrap

Ask first why the value might be missing. Often the check exists because of state the type does not need.
`ContextStack` (`crates/moqui-lib/src/context_stack.rs`) kept `frames: Vec<Option<Frame>>` and an `index` that was
always `frames.len() - 1`, and `top_frame` checked and then unwrapped. Swapping in `get_or_insert_with` only moved the
unwrap; the fix was to drop the redundant state.

```rust
// before
struct ContextStack { frames: Vec<Option<Frame>>, index: usize, .. }
fn top_frame(&mut self) -> Frame {
    if self.frames[self.index].is_none() {
        self.frames[self.index] = Some(Rc::new(RefCell::new(IndexMap::new())));
    }
    Rc::clone(self.frames[self.index].as_ref().unwrap())
}
// after: the top frame is a field, so "one frame is always present" holds by type
struct ContextStack { top: Option<Frame>, below: Vec<Option<Frame>>, .. }
fn top_frame(&mut self) -> &Frame {
    self.top.get_or_insert_with(Frame::default)
}
pub fn pop(&mut self) -> Result<Option<Frame>, Error> {
    let Some(next) = self.below.pop() else { return Err(..) };
    Ok(std::mem::replace(&mut self.top, next))
}
```

The same applies to an `expect` that names its invariant: `alts.pop().expect("one regex alternative")` after
`if alts.len() == 1` is better written by returning the first alternative before the vec is built. An `expect` message
is the fallback when no restructure is clear, not a reason to keep the site.

### 5b. JSON shape checked by unwrap

A `serde_json::Value` from a request is input, so a missing or mistyped field must not panic the request.

```rust
// before
let from = time::parse(range["from"].as_str().unwrap(), 0)?;
let thru = time::parse(range["thru"].as_str().unwrap(), 0)?;
// after
let (Some(from), Some(thru)) = (range["from"].as_str(), range["thru"].as_str()) else {
    return Err(/* the error this function already returns for a bad range */);
};
let (from, thru) = (time::parse(from, 0)?, time::parse(thru, 0)?);
```

The same applies to `as_object().unwrap()`, `as_array().unwrap()` and `as_i64().unwrap()`, and to a `Value::X(..)`
variant checked with `matches!` and then pulled out with `unwrap`: use `if let` or `let ... else` on the variant.

### 5c. A key looked up twice

Read where the key is set. If it is always there, bind it once with `expect`; if not, use `if let`.

```rust
// before
field.config.get_mut("bucket").unwrap()["filters"] = json!(filters);
field.config.get_mut("bucket").unwrap()["labels"] = json!(display);
// after
if let Some(bucket) = field.config.get_mut("bucket") {
    bucket["filters"] = json!(filters);
    bucket["labels"] = json!(display);
}
```

### 5d. Both ends of a map, after one is checked

```rust
// before
let Some((&first, _)) = buckets.first_key_value() else { return ... };
let last = *buckets.last_key_value().unwrap().0;
// after
let (Some(&first), Some(&last)) = (buckets.keys().next(), buckets.keys().next_back()) else { return ... };
```

### 5e. Slices and strings

- `if v.len() > 0 { v[0] }` → `if let [first, ..] = v.as_slice()` or `v.first()`.
- `&s[..n]` on a `str` panics when `n` is not on a char boundary. A `find` or `rfind` followed by `&s[..i]` and
  `&s[i + 1..]` is `split_once` or `rsplit_once`. Checking the first char and then taking `&s[1..]` is
  `strip_prefix`. Taking `n` bytes is `split_at_checked(n)` or `s.get(..n)`. Use these even when the index is provably
  on a boundary: the form says what the code means and needs no argument. A real scanner that keeps a byte position
  (`node/parse.rs`, `xml_edit/tokenize.rs`) may slice.
- A loop over an index into `text` (`while at < text.len() { .. text[at..] .. }`) is often clearer as a `rest: &str`
  cursor that `strip_prefix`, `split_once` and `split_at_checked` advance (`barcode.rs` `parse_gs1`).
- `let chars: Vec<char> = s.chars().collect()` built only to index `chars[i]` is a `Chars` iterator or a `&str`
  cursor (`env.rs` `expand`).

### 5f. Already right

- `downcast_mut::<T>().expect("slot type matches its TypeId")` (`crates/moqui-entity/src/entity/facade.rs`) states its
  invariant. This is the model for `expect` messages: say why the value is there, not "failed to …".
- `unwrap_or`, `unwrap_or_default`, `unwrap_or_else(PoisonError::into_inner)`.

In non-test code, `assert!` and `debug_assert!` get a message that shows the value:
`assert!(n > 0, "batch size must be positive, got {n}")`.

## 6. Ownership and borrowing

Prefer unique ownership and borrows; use `Rc`/`Arc` only when the alternative is unreasonably complex. Make a deep
clone only when one is needed. Cloning an `Arc` is cheap and fine.

Most `Arc` here is right: the `ExecutionContextFactory`, the caches, the pools and the registries are shared across
threads.

### 6a. `&Arc<T>` parameters

Take `&T` when the body only reads through the value, and so does everything it passes the value to. Callers keep
passing `&factory`, because deref coercion turns `&Arc<T>` into `&T`.

```rust
// before (crates/moqui-devkit/src/reload.rs)
fn reload_entities(factory: &Arc<ExecutionContextFactory>, files: &mut [FileReload]) -> bool
// after: neither it nor its callees clone, store, or spawn with it
fn reload_entities(factory: &ExecutionContextFactory, files: &mut [FileReload]) -> bool
```

dev-aide's `preview_overlay` looks the same but keeps `&Arc`: it is a `screen::overlay::Provider`.

Work bottom-up: change a leaf function first, then its callers. Keep `&Arc<T>` when:
- the function or a callee calls `Arc::clone`;
- it moves the value into a thread or a closure that outlives the call;
- it stores the value;
- it calls a method with `self: &Arc<Self>`;
- it must match a hook type, such as `screen::overlay::Provider`.

### 6b. `Arc<String>`, `Arc<Vec<T>>`

For shared data that is never mutated, `Arc<str>` and `Arc<[T]>` save one allocation and one indirection
(`clippy::rc_buffer`). Use them unless something mutates the value through `Arc::make_mut` or `get_mut`.

### 6c. `Rc<RefCell<..>>` with one owner

If only one place ever holds the `Rc`, own the value (`Option<T>`, `&mut T`). Read the comment first. Two known cases
are shared on purpose:
- `transaction/manager.rs` shares `Rc<RefCell<Option<PooledConn>>>` between the transaction and an open stream.
- `Rc` in the QuickJS bindings and in the `context_stack` frames is shared with the script runtime.

### 6d. Clones that a borrow would do

- `f(&x.clone())` → `f(&x)`.
- Cloning a `serde_json::Value`, a map or a `Vec` only to read one field → borrow it, then clone only that field.
- A `String`, `Vec` or map parameter that is taken by value but only read → `&str`, `&[T]` or `&Map`
  (`clippy::needless_pass_by_value`). rquickjs binding signatures need owned values; leave those.
- `.to_string()` on a `&str` passed to a function that only reads it → change the parameter to `&str`.
- Cloning, inside a loop, a value the loop does not change → hoist the clone or borrow.
- A value that is usually borrowed and sometimes built → `Cow<'_, str>`.

### 6e. Borrowed parameter types

`&Option<T>` → `Option<&T>` (callers pass `x.as_ref()`; `clippy::ref_option`). `&Box<T>` → `&T`.

## 7. Idiom

- **Enum over bool.** Use an enum for a public function with two or more `bool` parameters, or with call sites like
  `f(x, true, false)`. `dev-aide`'s `Tier` is the model. A single well-named `bool` stays, and a literal `true` or
  `false` at a call site may get a `/* name */` comment.
- **Named fields over tuples.** A function that returns three or more values, or two of the same type, returns a small
  struct with named fields (`xml_edit`'s `Splice { range, text }`).
- **Newtypes** only where two ids of the same type have been mixed up, or where the type has its own rules. Do not
  wrap every `String`.
- **No glob imports.** Name what is imported. `use super::*` in a test module and `pub use sub::*` in a re-export
  module are fine.
- **Iterator chains.** Do not put a chain in a `for` header; bind it to a variable first. Use a `for` loop, not a
  `.for_each(...)` with side effects. Break a long chain into named steps.
- **Type annotations.** Annotate the result of `.parse()`, `.into()` and `.collect()` when the type is not obvious,
  on the binding: `let port: u16 = text.parse()?`, not `text.parse::<u16>()?`.
- **`format!`** to join three or more strings and to fill in a template, not `+` chains or `str::replace` on a literal.
- **Locks** go through `moqui_lib::sync::{lock, read, write}`, which recover a poisoned lock (§8). Do not write a local
  helper or a non-test `.lock().unwrap()`.
- **`unsafe`.** Add none. Every existing `unsafe` block has a `// SAFETY:` comment and every `unsafe fn` a `# Safety`
  doc (`clippy::undocumented_unsafe_blocks`).
- **`#[must_use]`** on a function when dropping its result is a bug, such as a guard or a computed "changed" flag. The
  builders have it (`entity/find.rs`).
- **Errors.** `Option` when there is nothing to say, `Result` when the caller needs to know why, a panic only for bugs.
  Many functions return `Result<_, String>`; leave that unless you are already changing the function, and do not add
  `anyhow` or `thiserror`.

## 8. Where MR Moqui departs from Google

Do not "fix" these:
- **`catch_unwind` and `panic = "unwind"`.** Google forbids `catch_unwind`. Here every QuickJS host function, the
  service pool and the background jobs catch panics, so that one bad piece fails alone
  ([Architecture-Proposal](../../plans/archive/Architecture-Proposal.md), `.cargo/config.toml`, the failure tolerance rule).
  A new background loop does the same.
- **Poisoned locks.** Google says never to reach into a poisoned mutex. Here the policy is
  `unwrap_or_else(PoisonError::into_inner)` through `moqui_lib::sync`, so that one panicked request does not lock
  everyone out.
- **async and threads.** `moqui-host` uses Tokio and axum. `moqui-core` is synchronous on `std::thread` by design. Do
  not add async to core.
- **Crates along hook seams.** Google favors small crates and a flat layout. Here a crate is split off only along a
  seam a hook can carry (a register entry above core, a hook table below it), when the split saves rebuild time or
  keeps a heavy dependency out of core ([Crate-Split](../../plans/Crate-Split.md)). Modules inside a crate carry the
  rest of the structure; do not split by facade or by module for its own sake.
- **Docs.** This is application code, so there is no `missing_docs`. Module docs and comments follow §2 and §4.
- **Library-API rules** (`impl Into<T>`, returning `impl Iterator`, `#[non_exhaustive]`, generics over `dyn`). This is
  application code in one workspace, so `Vec` returns and `dyn` registries are fine.

## 9. Checklist

- The code is in the lowest crate that can hold it; nothing calls up the chain; feature-gated code has its off stub.
- New modules have a `//!` doc; no file grew past its seams without a split.
- Tests are in `_tests.rs`, `tests.rs` or `tests/`, not a large inline module; host integration tests are modules of
  `tests/it`; goldens rewritten by their variable, and the diff read.
- No panic reachable from input; each `expect` says why its value is there.
- No `&Arc<T>` that only reads, no clone a borrow would do, no local lock helper, no new `unsafe`.
- `cargo lint`, `cargo lint-min` if the host changed, `cargo test -p <crate>`; only your files formatted.
- Behaviour changes named in the commit message.
