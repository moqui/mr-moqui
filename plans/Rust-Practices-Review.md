# Rust Practices Review

A review of the workspace Rust against [Practices-Rust](../docs/dev/Practices-Rust.md), the practices drawn from
[Google's Rust style guide](https://google.github.io/styleguide/rustguide.html) for mr-moqui. That document holds the
rules and examples; this one applies them to the existing code: the targets, the order, and the results. It is written
for the model doing the review and changes.

Scope: `crates/*`, `runtime/dev-component/dev-aide` and `runtime/base-component/example`. Component repositories (`runtime/component/*`) are separate git repos and out of scope.

Earlier review: [Code-Quality-Review](archive/Code-Quality-Review.md) (2026-10-14) covered panics reachable from input, locks, and unsafe. Do not redo its findings. This review is about idiom and ownership.

## How to work

- Follow [Practices-Rust §4](../docs/dev/Practices-Rust.md#4-working-on-rust-code) (behaviour, checks, formatting).
- One theme per commit, one crate (or one moqui-core module) at a time. Commit messages follow the recent ones: what changed, and the section of Practices-Rust.
- Skip test code (`*_tests.rs`, `tests.rs`, `tests/`, `#[cfg(test)]`, `testkit` assertions). Tests may `unwrap`.
- Do not mass-rewrite. Change a site only when the new form is clearly better there. When in doubt, leave it and list it in the report.
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

## Targets

Counts are of non-test code when the review began.

### Unwraps and runtime type checks ([Practices-Rust §5](../docs/dev/Practices-Rust.md#5-unwraps-and-invariants))

Most of the ~11,600 `.unwrap()` calls are in tests. Non-test code has under a hundred, plus a few hundred `.expect(`.
Known sites:
- `crates/moqui-core/src/report/mod.rs:1504`: `range["from"].as_str().unwrap()` on a request's range (§5b).
- `crates/moqui-core/src/report/mod.rs:1246`: `get_mut("bucket").unwrap()` twice (§5c).
- `crates/moqui-core/src/telemetry/exponential.rs:284`: `last_key_value().unwrap()` after checking the first (§5d).
- `clippy::string_slice` lists the string slices (§5e).

### Ownership and borrowing ([Practices-Rust §6](../docs/dev/Practices-Rust.md#6-ownership-and-borrowing))

- About 300 signatures take `&Arc<T>`, mostly `&Arc<ExecutionContextFactory>` (§6a), such as `preview_overlay` in
  `runtime/dev-component/dev-aide/src/changeset.rs:52`.
- ~20 `Arc<String>` / `Arc<Vec<T>>` sites (`clippy::rc_buffer`, §6b).
- ~4,000 `.clone()` (§6d). `clippy::needless_pass_by_value` also flags rquickjs binding signatures; the earlier review
  dismissed 533 of those.
- ~17 `&Option<T>` parameters (`clippy::ref_option`, §6e).

### Idiom ([Practices-Rust §7](../docs/dev/Practices-Rust.md#7-idiom))

- Glob imports at `crates/moqui-core/src/script/runtime.rs:78-83` (`use convert::*;`, `use decimal::*;` …).
- ~24 `.for_each(...)` with side effects.
- Local lock helpers to delete in favour of `moqui_lib::sync`: `crates/moqui-core/src/telemetry/mod.rs:71`,
  `devtools/watch.rs:256`, `devtools/index/mod.rs:575`, `devtools/reload.rs:699`.
- ~290 `Result<_, String>` signatures: leave unless already changing the function.
- `#[must_use]` additions are low priority.

The deliberate departures from Google are in [Practices-Rust §8](../docs/dev/Practices-Rust.md#8-where-mr-moqui-departs-from-google); do not "fix" them.

## Order

1. Unwraps (Practices-Rust §5), crate by crate, smallest first: `moqui-lib`, `moqui-db`, `dev-aide`, `moqui-host`, `moqui-tui`, `moqui-llm`, then `moqui-core` by module.
2. Borrows and Arc (Practices-Rust §6), in the same order.
3. The idiom items (Practices-Rust §7).

## Results

Fill this in as the work is done. For each pass, list the sites changed, and the sites left and why.

### moqui-lib, unwraps (§5) and the tuples (§7)

Commit 76c54836 did a first pass. It changed `top_frame` to `get_or_insert_with` and argued the other sites away. That pass was redone:

- `context_stack.rs`: `top: Option<Frame>` and `below: Vec<Option<Frame>>` in place of the vec and its redundant `index` (Practices-Rust §5a). Lookups use `top_down()`, and the combined maps use `root_first()`. `top_frame` returns `&Frame`, and new frames are `Frame::default()`.
- `regex_match.rs` `parse_alt` and `collapse`, `etl.rs` `flat_xml` (it handles a closing text field before taking the entry), and `object.rs` `time_epoch_millis` (`duration_since(Time::midnight())`): no `expect` is left.
- String slices became `split_once`, `rsplit_once`, `strip_prefix` or `split_at_checked` in `resource.rs`, `markdown.rs`, `env.rs`, `object.rs` (`take_n_digits`, `take_hms_frac`, `parse_offset`) and `barcode.rs` (`parse_gs1` cursor, `ai_shape` byte pattern). The `Vec<char>` loops in `env.rs` `expand` and `string.rs` were replaced, and `node/mod.rs` `replace_child` uses `get_mut`. New tests cover dotenv quotes, URL-decoder-safe text, the path helpers, offsets and GS1. They pass on the code from before the change too. `parse_offset` now rejects hours or minutes that are not two digits, such as `+1:+2`, which `str::parse` used to take.
- Tuples (§7): `xml_edit` edits are a `Splice { range, text }`, and `ai_shape` returns `AiShape { fixed, max, numeric }`.

Left, with `-W clippy::indexing_slicing -W clippy::string_slice`:

- Byte scanners that keep a position: `node/parse.rs`, `xml_edit/tokenize.rs`, `xml_edit/outline.rs`, and `xml_edit/mod.rs` (`doc.nodes[element]`, whose ids come from `locate`). Also the Code 128 and EAN encoders in `barcode.rs`, over digits checked on entry.
- Fixed tables indexed by a value already in range: `HEX[b >> 4]` (`codec.rs`, `string.rs`), `SUBTWENTY`, `DECADES` and `SCALES` (`string.rs`), `MONTHS[month - 1]` (`datefmt.rs`), and `CODE128`, `PATTERNS` and `EAN_L` (`barcode.rs`).
- The regex VM (`code[pc]`, `seen[pc]` and the backpatching in `regex_match.rs`) and the LIKE matcher in `object.rs`, which backtracks over two `Vec<char>`s. Both need random access.
- The pattern tokenizers in `datefmt.rs` and `format.rs` (`chars[index]` with lookahead over a format pattern). They could become `Peekable<Chars>`. This is not clearly better, so they were left.
- `unwrap_or`, `unwrap_or_else` and `unwrap_or_default`, including `sync.rs`, which recovers a poisoned lock (Practices-Rust §8).
- Test code, including `outline.rs:250` and `cache_tests.rs:191`. `allow-indexing-slicing-in-tests` does not silence `clippy::string_slice`.
