# Rust Practices Review

A review of the workspace Rust against [Google's Rust style guide](https://google.github.io/styleguide/rustguide.html), limited to what helps mr-moqui. It is written for the model doing the review and changes. Each section says what to look for, how to find it, a before/after example from this code, and what to leave alone.

Scope: `crates/*`, `runtime/dev-component/dev-aide` and `runtime/base-component/example`. Component repositories (`runtime/component/*`) are separate git repos and out of scope.

Earlier review: [Code-Quality-Review](archive/Code-Quality-Review.md) (2026-10-14) covered panics reachable from input, locks, and unsafe. Do not redo its findings. This review is about idiom and ownership.

## How to work

- Do not change behaviour. If a fix changes what a caller sees, say so in the commit message. An error returned instead of a panic is such a change, and a wanted one.
- One theme per commit, one crate (or one moqui-core module) at a time. Commit messages follow the recent ones: what changed, and the section of this guide.
- Skip test code (`*_tests.rs`, `tests.rs`, `tests/`, `#[cfg(test)]`, `testkit` assertions). Tests may `unwrap`.
- Do not mass-rewrite. Change a site only when the new form is clearly better there. When in doubt, leave it and list it in the report.
- After each change, run `cargo lint` (clippy, warnings denied), `cargo lint-min` if `moqui-host` changed, and `cargo test -p <crate>` for the crates touched. Use one toolchain so the cache stays warm ([building skill](../.agents/skills/building/SKILL.md)).
- Format only the files you changed: `rustfmt --edition 2021 --config-path rustfmt.toml <files>`. Then check `git diff -U0` for hunks that are not yours and put them back. Never run `cargo fmt` on a whole crate.
- To find sites, run clippy with extra lints and a config that exempts tests. Do not add these lints to `[workspace.lints]`, which would break `cargo lint` until every site is fixed.

```sh
mkdir -p "$SCRATCH/clippy" && printf 'allow-unwrap-in-tests = true\nallow-expect-in-tests = true\nallow-panic-in-tests = true\nallow-indexing-slicing-in-tests = true\n' > "$SCRATCH/clippy/clippy.toml"
CLIPPY_CONF_DIR="$SCRATCH/clippy" cargo clippy --workspace --all-targets --message-format=short -- \
  -W clippy::unwrap_used -W clippy::expect_used -W clippy::string_slice \
  -W clippy::rc_buffer -W clippy::ref_option -W clippy::needless_pass_by_value \
  -W clippy::wildcard_imports -W clippy::undocumented_unsafe_blocks -W clippy::fn_params_excessive_bools \
  2>&1 | grep -E '^(crates|runtime)/' | sort -u > "$SCRATCH/clippy/hits.txt"
```

The short format gives the message, not the lint name (for example "used `unwrap()` on an `Option` value"). To see the lint for one site, run clippy without `--message-format=short`.

The workspace already denies the default Clippy set, so these are already clean: `unnecessary_unwrap` (`if x.is_some() { x.unwrap() }`), `ptr_arg` (`&String`, `&Vec<T>`), `should_implement_trait`, `len_without_is_empty`, `new_without_default` and `wrong_self_convention`. Do not hunt for them.

## 1. Unwraps and runtime type checks

Most of the ~11,600 `.unwrap()` calls are in tests. Non-test code has under a hundred, plus a few hundred `.expect(`. The Google rule is to match on the value, not to check it and then unwrap. When a panic is right (a bug, a broken invariant), use `expect` and give the reason the value is there.

Choose by where the value comes from:
- **Input** (a request, JSON, a file, a database row, conf): never panic. Return the function's existing error with `let ... else`, or with `?` on `ok_or_else`. Use the error type the function already returns; do not add a new one.
- **An invariant this code keeps** (a key inserted just above, a `TypeId` match): restructure the code so the compiler can see the invariant (`get_or_insert_with`, `entry`, bind once). If that is not possible, use `expect("why it holds")`.
- **Truly infallible** (`write!` into a `String`): match the file's existing form.

### 1a. Check, then unwrap
Ask first why the value might be missing. Often the check exists because of state the type does not need. `ContextStack` (`crates/moqui-lib/src/context_stack.rs`) kept `frames: Vec<Option<Frame>>` and an `index` that was always `frames.len() - 1`. Every method indexed with `frames[self.index]` or `frames[..=self.index]`, and `top_frame` checked and then unwrapped. Swapping in `get_or_insert_with` only moved the unwrap; the fix was to drop the redundant state.
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
The same applies to an `expect` that names its invariant. `alts.pop().expect("one regex alternative")` after `if alts.len() == 1` is better written by returning the first alternative before the vec is built. An `expect` message is the fallback when no restructure is clear, not a reason to keep the site.

### 1b. JSON shape checked by unwrap
`crates/moqui-core/src/report/mod.rs:1504`. `range` comes from a request, so a missing or non-string `from` panics the request.
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
The same applies to `as_object().unwrap()`, `as_array().unwrap()` and `as_i64().unwrap()` on a `serde_json::Value`. It also applies to a `Value::X(..)` variant that is checked with `matches!` and then pulled out with `unwrap`. Use `if let` or `let ... else` on the variant.

### 1c. A key looked up twice
`crates/moqui-core/src/report/mod.rs:1246`. Read where `bucket` is set. If it is always there, bind it once with `expect`; if not, use `if let`.
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

### 1d. Both ends of a map, after one is checked
`crates/moqui-core/src/telemetry/exponential.rs:284`
```rust
// before
let Some((&first, _)) = buckets.first_key_value() else { return ... };
let last = *buckets.last_key_value().unwrap().0;
// after
let (Some(&first), Some(&last)) = (buckets.keys().next(), buckets.keys().next_back()) else { return ... };
```

### 1e. Slices and strings
- `if v.len() > 0 { v[0] }` → `if let [first, ..] = v.as_slice()` or `v.first()`.
- `&s[..n]` on a `str` panics when `n` is not on a char boundary (the `object.rs` case in the earlier review). `clippy::string_slice` finds these sites. A `find` or `rfind` followed by `&s[..i]` and `&s[i + 1..]` is `split_once` or `rsplit_once`. Checking the first char and then taking `&s[1..]` is `strip_prefix`. Taking `n` bytes is `split_at_checked(n)` or `s.get(..n)`. Rewrite these even when the index is provably on a boundary: the new form says what the code means, and it needs no argument. Leave a slice only in a real scanner that keeps a byte position (`node/parse.rs`, `xml_edit/tokenize.rs`).
- A loop over an index into `text` (`while at < text.len() { .. text[at..] .. }`) is often clearer as a `rest: &str` cursor that `strip_prefix`, `split_once` and `split_at_checked` advance (`barcode.rs` `parse_gs1`).
- `let chars: Vec<char> = s.chars().collect()` built only to index `chars[i]` is a `Chars` iterator (`chars.next()`) or a `&str` cursor (`env.rs` `expand`, `string.rs` `is_url_decoder_safe`).

### 1f. Already right; leave
- `downcast_mut::<T>().expect("slot type matches its TypeId")` (`crates/moqui-core/src/entity/facade.rs:371`) states its invariant. This is the model for `expect` messages: say why the value is there, not "failed to …".
- `unwrap_or`, `unwrap_or_default`, `unwrap_or_else(PoisonError::into_inner)`.

In non-test code, `assert!` and `debug_assert!` get a message that shows the value: `assert!(n > 0, "batch size must be positive, got {n}")`.

## 2. Ownership and borrowing instead of Rc and Arc

Google says to prefer unique ownership and borrows, and to use `Rc`/`Arc` only when the alternative is unreasonably complex. Make a deep clone only when one is needed. Cloning an `Arc` is cheap and fine.

Most `Arc` here is right. The `ExecutionContextFactory`, the caches, the pools and the registries are shared across threads, so do not remove those. The targets are narrower.

### 2a. `&Arc<T>` parameters that never clone the Arc
About 300 non-test signatures take `&Arc<T>`, mostly `&Arc<ExecutionContextFactory>`. Take `&T` when the body only reads through the value, and so does everything it passes the value to. Callers keep passing `&factory`, because deref coercion turns `&Arc<T>` into `&T`.
```rust
// before (runtime/dev-component/dev-aide/src/changeset.rs:52)
fn preview_overlay(factory: &Arc<ExecutionContextFactory>, token: &str) -> Option<Overlay>
// after, if neither it nor buffer_overlay clones, stores, or spawns with it
fn preview_overlay(factory: &ExecutionContextFactory, token: &str) -> Option<Overlay>
```
Work bottom-up: change a leaf function first, then its callers. Keep `&Arc<T>` in any of these cases:
- the function or a callee calls `Arc::clone`;
- it moves the value into a thread or a closure that outlives the call;
- it stores the value;
- it calls a method with `self: &Arc<Self>`;
- it must match a hook type, such as `screen::overlay::Provider`.

### 2b. `Arc<String>`, `Arc<Vec<T>>`
For shared data that is never mutated, `Arc<str>` and `Arc<[T]>` save one allocation and one indirection (`clippy::rc_buffer`, ~20 sites). Change a site only when nothing mutates the value through `Arc::make_mut` or `get_mut`.

### 2c. `Rc<RefCell<..>>` with one owner
If only one place ever holds the `Rc`, own the value (`Option<T>`, `&mut T`). Read the comment first. Two known cases stay:
- `transaction/manager.rs` shares `Rc<RefCell<Option<PooledConn>>>` between the transaction and an open stream on purpose.
- `Rc` in the QuickJS bindings and in the `context_stack` frames is shared with the script runtime.

### 2d. Clones that a borrow would do
Non-test code has ~4,000 `.clone()`. Look for:
- `f(&x.clone())` → `f(&x)`.
- Cloning a `serde_json::Value`, a map or a `Vec` only to read one field → borrow it, then clone only that field.
- A `String`, `Vec` or map parameter that is taken by value but only read → `&str`, `&[T]` or `&Map` (`clippy::needless_pass_by_value`). Skip rquickjs binding signatures, which need owned values. The earlier review dismissed 533 of those.
- `.to_string()` on a `&str` that is passed to a function that only reads it → change the parameter to `&str`.
- Cloning, inside a loop, a value the loop does not change → hoist the clone or borrow.
- A value that is usually borrowed and sometimes built → `Cow<'_, str>`.

### 2e. Borrowed parameter types
`&Option<T>` → `Option<&T>` (callers pass `x.as_ref()`; `clippy::ref_option`, ~17 sites). `&Box<T>` → `&T`.

## 3. Other points from the Google guide that apply

- **Enum over bool.** Use an enum for a public function with two or more `bool` parameters, or with call sites like `f(x, true, false)`. `dev-aide`'s `Tier` is the model. A single well-named `bool` stays, and a literal `true` or `false` at a call site may get a `/* name */` comment.
- **Named fields over tuples.** If a function returns a tuple of three or more values, or of two values of the same type, return a small struct with named fields instead.
- **Newtypes.** Add a newtype only where two ids of the same type have been mixed up, or where the type has its own rules. Do not wrap every `String`.
- **No glob imports.** Replace `crates/moqui-core/src/script/runtime.rs:78-83` (`use convert::*;`, `use decimal::*;` …) with explicit lists. `use super::*` in test modules and `pub use sub::*` in a re-export module stay.
- **Iterator chains.** Do not put a chain in a `for` header; bind it to a variable first. Replace a `.for_each(...)` that has side effects with a `for` loop (~24 sites). Break a long chain into named steps.
- **Type annotations.** Annotate the result of `.parse()`, `.into()` and `.collect()` when the type is not obvious. Write `let port: u16 = text.parse()?`, not `text.parse::<u16>()?`.
- **`format!`.** Use it to join three or more strings and to fill in a template. Do not use `+` chains, or `str::replace` on a literal.
- **One poison-tolerant lock helper.** `moqui_lib::sync::{lock, read, write}` already exists. Delete the local copies (`crates/moqui-core/src/telemetry/mod.rs:71`, `devtools/watch.rs:256`, `devtools/index/mod.rs:575`, `devtools/reload.rs:699`) and use it. A non-test `.lock().unwrap()` becomes `lock(&m)`.
- **`unsafe`.** Every `unsafe` block has a `// SAFETY:` comment and every `unsafe fn` has a `# Safety` doc. Keep it that way (`clippy::undocumented_unsafe_blocks`), and add no new `unsafe`.
- **`#[must_use]`.** The builders already have it (`entity/find.rs`). Add it to a function when dropping its result is a bug, such as a guard or a computed "changed" flag. This is low priority.
- **Errors.** Use `Option` when there is nothing to say, `Result` when the caller needs to know why, and a panic only for bugs. Non-test code has ~290 `Result<_, String>` signatures. Leave them unless you are already changing the function, and do not add `anyhow` or `thiserror`.

## 4. Where mr-moqui departs from Google on purpose

Do not "fix" these:
- **`catch_unwind` and `panic = "unwind"`.** Google forbids `catch_unwind`. Here, every QuickJS host function, the service pool and the background jobs catch panics, so that one bad piece fails alone ([Architecture-Proposal](archive/Architecture-Proposal.md), `.cargo/config.toml`, the failure tolerance rule in AGENTS.md). New background loops follow the same form.
- **Poisoned locks.** Google says never to reach into a poisoned mutex. Here the policy is `unwrap_or_else(PoisonError::into_inner)` through `moqui_lib::sync`, so that one panicked request does not lock everyone out.
- **async and threads.** `moqui-host` uses Tokio and axum. `moqui-core` is synchronous on `std::thread` by design. Do not add async to core.
- **Small crates and flat file layout.** Not adopted. The crate cut is in [Crate-Layout](archive/Crate-Layout.md), and this review does not move code between crates.
- **Formatting.** `max_width = 120`, edition 2021 (`rustfmt.toml`).
- **Docs.** This is application code, so there is no `missing_docs`. Comments follow the repo's plain style and cite the plan section.
- **Library-API rules** (`impl Into<T>`, returning `impl Iterator`, `#[non_exhaustive]`, generics over `dyn`). This is application code in one workspace, so `Vec` returns and `dyn` registries are fine.

## Order

1. Section 1 (unwraps), crate by crate, smallest first: `moqui-lib`, `moqui-db`, `dev-aide`, `moqui-host`, `moqui-tui`, `moqui-llm`, then `moqui-core` by module.
2. Section 2 (borrows and Arc), in the same order.
3. The section 3 items.

## Results

Fill this in as the work is done. For each pass, list the sites changed, and the sites left and why.

### moqui-lib, section 1 and the section 3 tuples

Commit 76c54836 did a first pass. It changed `top_frame` to `get_or_insert_with` and argued the other sites away. That pass was redone:

- `context_stack.rs`: `top: Option<Frame>` and `below: Vec<Option<Frame>>` in place of the vec and its redundant `index` (1a). Lookups use `top_down()`, and the combined maps use `root_first()`. `top_frame` returns `&Frame`, and new frames are `Frame::default()`.
- `regex_match.rs` `parse_alt` and `collapse`, `etl.rs` `flat_xml` (it handles a closing text field before taking the entry), and `object.rs` `time_epoch_millis` (`duration_since(Time::midnight())`): no `expect` is left.
- String slices became `split_once`, `rsplit_once`, `strip_prefix` or `split_at_checked` in `resource.rs`, `markdown.rs`, `env.rs`, `object.rs` (`take_n_digits`, `take_hms_frac`, `parse_offset`) and `barcode.rs` (`parse_gs1` cursor, `ai_shape` byte pattern). The `Vec<char>` loops in `env.rs` `expand` and `string.rs` were replaced, and `node/mod.rs` `replace_child` uses `get_mut`. New tests cover dotenv quotes, URL-decoder-safe text, the path helpers, offsets and GS1. They pass on the code from before the change too. `parse_offset` now rejects hours or minutes that are not two digits, such as `+1:+2`, which `str::parse` used to take.
- Section 3: `xml_edit` edits are a `Splice { range, text }`, and `ai_shape` returns `AiShape { fixed, max, numeric }`.

Left, with `-W clippy::indexing_slicing -W clippy::string_slice`:

- Byte scanners that keep a position: `node/parse.rs`, `xml_edit/tokenize.rs`, `xml_edit/outline.rs`, and `xml_edit/mod.rs` (`doc.nodes[element]`, whose ids come from `locate`). Also the Code 128 and EAN encoders in `barcode.rs`, over digits checked on entry.
- Fixed tables indexed by a value already in range: `HEX[b >> 4]` (`codec.rs`, `string.rs`), `SUBTWENTY`, `DECADES` and `SCALES` (`string.rs`), `MONTHS[month - 1]` (`datefmt.rs`), and `CODE128`, `PATTERNS` and `EAN_L` (`barcode.rs`).
- The regex VM (`code[pc]`, `seen[pc]` and the backpatching in `regex_match.rs`) and the LIKE matcher in `object.rs`, which backtracks over two `Vec<char>`s. Both need random access.
- The pattern tokenizers in `datefmt.rs` and `format.rs` (`chars[index]` with lookahead over a format pattern). They could become `Peekable<Chars>`. This is not clearly better, so they were left.
- `unwrap_or`, `unwrap_or_else` and `unwrap_or_default`, including `sync.rs`, which recovers a poisoned lock (section 4).
- Test code, including `outline.rs:250` and `cache_tests.rs:191`. `allow-indexing-slicing-in-tests` does not silence `clippy::string_slice`.
