# Rust Practices Review

A review of the workspace Rust against [Practices-Rust](../docs/dev/Practices-Rust.md), the practices drawn from
[Google's Rust style guide](https://google.github.io/styleguide/rustguide.html) for mr-moqui. That document holds the
rules and examples; this one applies them to the existing code: the targets, the order, and the results. It is written
for the model doing the review and changes.

Scope: `crates/*`, `runtime/dev-component/dev-aide` and `runtime/dev-component/example`. Component repositories (`runtime/component/*`) are separate git repos and out of scope.

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
- `crates/moqui-report/src/lib.rs:1529`: `range["from"].as_str().unwrap()` on a request's range (§5b).
- `crates/moqui-report/src/lib.rs:1271`: `get_mut("bucket").unwrap()` twice (§5c).
- `crates/moqui-core/src/telemetry/exponential.rs:284`: `last_key_value().unwrap()` after checking the first (§5d).
- `clippy::string_slice` lists the string slices (§5e).

### Ownership and borrowing ([Practices-Rust §6](../docs/dev/Practices-Rust.md#6-ownership-and-borrowing))

- About 300 signatures take `&Arc<T>`, mostly `&Arc<ExecutionContextFactory>` (§6a), such as `reload_entities` in
  `crates/moqui-devkit/src/reload.rs`.
- ~20 `Arc<String>` / `Arc<Vec<T>>` sites (`clippy::rc_buffer`, §6b).
- ~4,000 `.clone()` (§6d). `clippy::needless_pass_by_value` also flags rquickjs binding signatures; the earlier review
  dismissed 533 of those.
- ~17 `&Option<T>` parameters (`clippy::ref_option`, §6e).

### Idiom ([Practices-Rust §7](../docs/dev/Practices-Rust.md#7-idiom))

- Glob imports at `crates/moqui-core/src/script/runtime.rs:78-83` (`use convert::*;`, `use decimal::*;` …).
- ~24 `.for_each(...)` with side effects.
- Local lock helpers to delete in favour of `moqui_lib::sync`: `crates/moqui-core/src/telemetry/mod.rs:71`,
  `crates/moqui-devkit/src/watch.rs:256`, `index/mod.rs:576`, `reload.rs:695`.
- ~290 `Result<_, String>` signatures: leave unless already changing the function.
- `#[must_use]` additions are low priority.

The deliberate departures from Google are in [Practices-Rust §8](../docs/dev/Practices-Rust.md#8-where-mr-moqui-departs-from-google); do not "fix" them.

## Order

The trial did moqui-lib theme by theme. The rest was done crate by crate after the crate split, all three themes in each
(one commit per theme), with moqui-core and the crates below it before the crates above, so that §6a was decided
bottom-up: moqui-db, moqui-drivers and moqui-entity; moqui-core by module; then moqui-host and moqui-browser, moqui-llm,
moqui-git, moqui-mail, moqui-report, moqui-collab and moqui-dav, and moqui-devkit, dev-aide and moqui-tui.

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

### moqui-core script/ and search/ (§5, §6, §7)

Commits a12983e3, 46c0af79, 938b2fca.

- §5: `search/time.rs` `eval` steps over the operator with a `Chars` cursor and reads `+N` and `/unit` with `split_at_checked` and `strip_prefix`; the rounding unit carries its step letter. A report time with a non-ASCII character after `now` panicked and is now an "invalid report time" error. `search/postgres/agg.rs` `histogram_buckets` matches the fill range in one pattern in place of a length check and two unwraps. Slices after a find became `split_once`, `rsplit_once` or `split_at_checked` in `search/agg.rs` `parse_offset`, `script/actions.rs` `with_source_line`, `script/parse.rs` `file_function` and `has_closure_call` (`match_indices`), and `script/template.rs` `tag_row_text` and `split_last_word`; length checks before an index became slice patterns in `search/parse.rs` (the `[low TO high]` range, the lone star), `search/postgres/compile.rs` `near_leaf` and `template.rs` `locked_format`. New tests for each.
- §6: `SearchFacade::backend` returns `&dyn SearchBackend`; `semantic.rs` `embedding_text`, `agg.rs` (filter query via `es_json::from_map`), `error_info.rs` `get_artifact_info` and `ec_extra.rs` `getMapListValues` no longer clone to read; `opensearch.rs` uses `as_deref`; the argument closures in `conditions.rs` and `facades.rs` take `Option<&JsValue>`.
- §7: `script/runtime.rs` names its imports in place of 11 globs; eight inline poison-recovering locks in `actions.rs`, `template.rs` and `runtime/facades.rs` use `moqui_lib::sync::lock`; `thumbnail_png` returns `Thumbnail { width, height, png }` and the fancy-markdown pieces are `Held { placeholder, original, is_tag }`.

Left:
- Scanners that keep a position: `template.rs` `tokenize`, `scan_expression`, the fancy-markdown loop and `line_of`; the runtime's template-literal escaping; `highlight.rs` `words` and `mark`; the `Vec<char>` tokenizer and `word_query` in `search/parse.rs`.
- Indexes guarded just above or by the caller (`ec_extra.rs` `is_date_text`, `actions.rs` `lines[..line - 1]`, `bounded.rs` `evict`, the `postgres/mod.rs` cursor, `search/telemetry.rs` `fields[0]`).
- About 1,200 `needless_pass_by_value` hits, all rquickjs bindings (§6d); `compiled.rs` `eval_keyed`'s `Option<&Arc<str>>` (it uses `Arc::ptr_eq`); the clones in `hybrid.rs` (consumed and also kept) and `entities.rs` `related_find` (releases the entity lock before a service call).
- `use super::*` in the runtime's 24 submodule files: they are parts of one split file sharing its namespace, and naming would mean lists of up to 45 items.

### moqui-db, moqui-drivers, moqui-entity (§5, §6, §7), and moqui-lib (§6, §7)

Commits 53ec663e, 624f667c, b79ea52c, 12b9cfe0, 399de630, f3d0558f. moqui-lib had nothing in §6 or §7 worth changing.

- §5, moqui-db: slice patterns in `model/mod.rs` `expand_key_map` and `resolve_relationships`, `sql/view.rs` (the sub-select alias, `join_keys`), `sql/statement.rs` `insert_or_update`; `OrderBy::parse` walks a `Chars`.
- §5, moqui-drivers: `postgres.rs` `numeric_to_text` reads its header with `split_first_chunk::<8>` and its digits with `get` and `as_chunks`; `dollar_quote_end` uses `split_once`; `pgwire.rs` `has_error_message` uses `split_first_chunk::<5>` and `get`.
- §5, moqui-entity: slice patterns in `find.rs`, `field_rw.rs`, `document.rs` `ids_condition`, `schema.rs`, `fk.rs` and `sim_view.rs`; `split_last` in `value.rs` `next_secondary_id`, `loader.rs` and `feed.rs` `related_entities`; string slices to `split_once`, `strip_prefix` and the like in `document.rs` `field_path_to_list`, `info.rs` `name_prefix`, `pull.rs` `take_text`, `xml.rs` `decode_xml_entities`, `sensitive.rs` `simple_entity_key`, `value.rs` `FieldDefault::parse` (an empty default no longer indexes `bytes[0]`), `period.rs` `days_of` and `textdiff.rs`; `feed.rs` `xmin_query` reads the `is_pk` fields in place of an `expect` per name. New tests for the order-by prefix, Postgres numerics, backend messages, XML entities, the simple entity key and hunk headers.
- §6: `pgwire.rs` `row_of` and `Wire::query` take `&[Column]`; in moqui-entity `EntityFind::lookup` takes `Option<&Condition>`, `sim_view.rs` `walk` and `matching_rows` take slices, `schema.rs` `one_entity` takes `Option<&str>`, and `cache.rs` `copy_replica` and `fence.rs` `refuse` move where they cloned.
- §7: `BlobStore::collect` returns `BlobGc { blobs, bytes, uploads }` (callers in core `service/content.rs` and moqui-git's tests) and `textdiff.rs` `parse_header` a `HunkHeader`; the pool's local `lock` helper is gone, and `driver.rs`, `datasource.rs`, `share.rs`, `sequence.rs` and the feed lock through `moqui_lib::sync`.

Left:
- Scanners that keep a position: `load_feed/xml.rs` and `json.rs`, the SQL splitting in `exec.rs`, `pull.rs` buffers; the drivers' `rewrite_placeholders`, `RowBatch` ranges and read buffers; moqui-db `shrink_name` and the `view.rs` tokenizer. Arena indexes in moqui-db `model/mod.rs`.
- Expects that state their invariant (`facade.rs` downcast, `pool.rs` `PooledConn::conn`); the drivers' `build.rs`; `testing.rs`, the in-memory test database.
- `&Arc` parameters that clone it (`negotiate_tls` for rustls, `other_groups`, `cache.rs` `on_write_shared`, `feed.rs` `note_touch`, `value.rs` `with_l10n`). `resource/source.rs` `Arc<Vec<u8>>`: the bytes arrive as a `Vec`, and `Arc<[u8]>` would copy them once. Owned parameters that are kept or consumed (`documents_json`, `select_sql`, `finish_write`, the hook and service types).
- Condvar waits that recover in place (`pool.rs`, moqui-lib `cache.rs`). Functions with two bools whose callers pass named flags (`FkScope::enter`, `all_entities_info`) or that are private. Tuples of mixed types or standard shapes (`prepare_run`, `civil_from_unix_days`, `refill_targets`, `entity_find`).

### moqui-core screen/, web/, user/ and context/ (§5, §6, §7)

Commits caeb7a38, e01bbc51, fa1320a0.

- §5: string slices after a find to `split_once`, `strip_prefix` and the like in `context/resource.rs` (`scheme_of`, `split_version`, `strip_hash`), `screen/definition.rs`, `pdf.rs` `data_url`, `saved_finds.rs`, `url.rs` (`Walk::run`, `prepare_screen_path`), `render.rs` `url_path_and_query`, `xlsx.rs` `cell_text`, `web/address.rs` `parse_hop`, and `web/mod.rs` (`full_url`, `mark_email_message_viewed`, `parse_size`, `host_without_port`); `web/multipart.rs` `disposition_param` walks a cursor with an `unquote` helper; `web/servlet.rs` `under` returns the rest of the path, so `match_servlet` does not slice by the prefix length. Length checks before an index became slice patterns, `split_last`, `ends_with` or `get` in `definition.rs`, `dispatch.rs`, `form.rs`, `info.rs`, `menu.rs`, `render.rs` `json_envelope`, `shell.rs` (`app_head`, `menu_level`), `static_ast.rs`, `url.rs`, `urls.rs`, `web/mod.rs` (`pairs_to_parameters`, `percent_decode`), `servlet.rs` `chain_next` and `pdf.rs` `image_type`. New tests in `context/resource.rs`, `web/tests.rs` and `web/multipart_tests.rs`.
- §6: `&Arc<T>` that only reads became `&T` in `screen/statics.rs` (`structure_reasons`, `prepare_screen`, `walk_widgets` and seven more), `statics/report.rs`, `render.rs` `run_transition_steps` (also `&RenderState` for `&Rc`) and `session.rs` (`resolve_id`, `ensure_visit`). The authz and tarpit check lists, the `user/filter` filter sets and an in-memory upload are `Arc<[T]>`, and `UploadedFile::reader` reads the Arc through a `Cursor` without copying it. `json_envelope` moves the rendered nodes, parameters and messages into the map (`json!` copied them on every JSON render); `present_extension` takes `&str`, `attach_session` `Option<&str>`; the exact servlet match and the section include no longer build a String.
- §7: locks through `moqui_lib::sync` in `resource.rs`, `form.rs`, `statics.rs`, `filter.rs`, `login.rs` and `upstream.rs`; `for` loops in place of `for_each` in `urls.rs`, `web` `strip_credential_parameters`, `form.rs`, `static_ast.rs` and `definition.rs`; named structs `PasswordRules`, `LoginRules`, `StackAuthz` (with `StackAuthz::NO_CONTEXT`), `FormLocation` and `HttpLogIdentity` (pub, `user::http_log_identity`) in place of tuples.

Left:
- Scanners that keep a position: `stamp.rs` (`inject`, `qvue_elements`, `qvue_tag`), `pdf.rs` (`inline_images`, `src_value`), `statics/report.rs` `names_identifier`, the multipart byte parser, the `upstream.rs` read buffer. Indexes bounded by `.min(len)` or a loop (`render.rs`, `menu.rs`, `url.rs` `render_defs`, `sri/*`, `artifact.rs`, `component.rs`); `shell.rs` `apps` and the `path_defs.len() - 1` sites, non-empty by construction.
- `&Arc` that clones, stores, spawns or calls `new_context`: the `screen/mod.rs` serve and request functions, `render.rs` `make_render` and the job runners, `token_route.rs`, `sri/sections.rs`, `url.rs` `enter`, `authz.rs` `from_check`, the `session.rs` installers, `servlet.rs` `started` and `stopping`, `factory.rs`. The `cache.rs` `&key.to_string()` lookups, which the cache API needs. The ~635 `.clone()` calls were not swept.
- `ExecutionContext::push_checked(.., requires_authz: bool, count_tarpit: bool, ..)` is a public two-bool function with 31 calls across core, moqui-llm, moqui-host and tests; an enum there is its own change. Private two-bool functions (`form.rs` `merge_form_nodes`, which mirrors Java's `mergeFormNodes`, `dispatch.rs` `send_logged_in`, `factory.rs` `log_property`, `cache.rs` `LoadingMark::new`). `info.rs`'s 3-tuple parameter. The `session.rs` `Condvar::wait` poison recovery.

### moqui-core service/, telemetry/, notify/, cluster/, devtools/, resource/ and the top-level files (§5, §6, §7)

Commits 89a49258, 262cad2a, 4d2e56f5.

- §5: `telemetry/exponential.rs` `json` takes both ends of the bucket map at once (the known §5d site); `align` collects the absorbed buckets with their counts instead of removing and unwrapping; `delta` is one pass that returns `None` at a decreased bucket (a missing bucket with a zero count used to reach an `unwrap`). `TraceContext::extract` matches `["00", trace_id, span_id, flags]`; `sdk.rs` and `scrape.rs` `sample` use `get` and `split_at`. `backup.rs` `parse_stamp` takes each field with `get`, and `parse_archive_name` reads the tier with a slice pattern: an archive name with a multi-byte character across a stamp field panicked and is now not an archive name. `split_once` and the like in `edi.rs` `segment_id`, `resource/locks.rs` `key`, `sftp.rs` (`split_host_port`, `rename_file`, `parent_of`), `service/define.rs`, `service/remote.rs` `expand_location` and `service/chat_link.rs`; slice patterns or `first` in `service/rest.rs`, `system_sync/message.rs`, `service/call.rs` `expand_key_map` and `edi.rs`; `external_agent.rs` `invoke` uses `split_first` and `expect` on the piped stdin and stdout. New tests for archive names, URL parameters, traceparents, sftp paths, the meta charset and lock keys.
- §6: `&ExecutionContextFactory` in `define.rs` `registry_of`, `rest.rs` `roots_of` and `eca_rules.rs` (`rules_of`, `reload`, `install`); `&Shared` in `jobs.rs` (`run_scheduled`, `pass`, `dispatch`) and `pool.rs` (`queue_closure`, `run_work`); cluster `send_batch` takes a slice and prometheus `number` an `Option<&Value>`.
- §7: the `telemetry/mod.rs` lock helper is gone, and 26 inline poison recoveries in the ECA, SECA, REST, call, validate, devtools, notify, locks, sftp and l10n code use `moqui_lib::sync`. `for` loops in place of `for_each` in contacts, shell and tools. `scrape.rs` `histogram_point` returns `HistogramPoint` (the `type_complexity` allow is gone) and a series' samples are `HistogramSamples`; `service::reload_eca_rules` returns `EcaRuleCounts { seca, email, git }` (callers in moqui-devkit).

Left:
- `expect` on constant regexes (§5f); the `test-support` helpers (`eca_rules` `from_xml`, `with_git`, pool `bounded`, rest `install`); scanners that keep a position (chat_link `extract_urls`, the scrape label cursor, the EDI segment index); the `devtools/calls.rs` tick index, checked just above.
- `&Arc` that clones, downgrades, spawns or calls `new_context` (shell `request_sends`, calls `start` and `flush`, `register_runtime_gauges`, the cluster starters and `fan_out`, the pool's reserve and persist functions); hook types (`native.rs` `ToolFactory`, `ResourceConstructor`, `llm_hooks` `canvas_overlay`); cluster `post_with_retry`'s `&Arc<dyn Sender>`, which every caller and test would need to change; maps built at the call site for one call (sso `call`, system_sync).
- Condvar waits; `service/coverage.rs`, which skips a count on a poisoned lock where the helper would recover it (a behaviour change); `ssh_key_fingerprint`'s 3-tuple, whose other two parts only its test reads; the OTLP `protobuf` and `log_capture::with` tuples, destructured at once by their one caller.

### moqui-host, moqui-browser and the example component (§5, §6, §7)

Commits cd25c399, 42c68f53, eb9ae797. The example component needed nothing.

- §5: string slices after a find to `split_once`, `strip_prefix`, `split_at_checked` and the like in `test_client/http.rs` (`domain_match`, `path_match`, `default_path`, `meta_token`, `attribute`, `config_token`), `test_client/raw.rs` `with_header`, `test_client/browser.rs` `url_path`, `git_http.rs`, `ipp.rs` `uri_path`, `tui.rs` `local_path` and `smtp.rs` (`strip_keyword`, the DATA dot-unstuffing); slice patterns in `tui.rs` `app_menu` and `ssh/agent.rs` `one_shot`. Two panics are gone: `lsp.rs` `path_of` on a URI whose first path character is several bytes (`file://é:/a.xml`), and the test client's `note_page_report` when taking or closing problems left the page's mark past the end of the list, which now reports no problems for that page. New tests for cookie domains and paths, cut-off tags and tokens, `url_path`, `local_path` and `path_of`.
- §6: the co-editing handlers in `notws/collab.rs` (`update`, `awareness`, `leave`, `socket_closed`) take `&ExecutionContextFactory`, and `update` and `awareness` `&Slot`; `cluster_edge.rs` `spawn_conn` takes its join list as `&Mutex`.
- §7: 54 inline poison recoveries in `tui`, `tui_remote`, `term` (with its local lock closure), `llm/screens`, `ssh/*`, `smtp`, `logging`, `monitor`, `devtools_remote` and moqui-browser's `api.rs` use `moqui_lib::sync::lock`. `lsp.rs` `tag_context` returns `TagContext { element, place: TagPlace }` (`Name`, `Attributes`, `Value(attribute)`) in place of `(String, Option<String>, bool)`, `Server::position` a `TextPosition`, and `cdp.rs` `key_codes` a `KeyCodes`.

Left:
- The CLI flag arms `other if other.starts_with("--server=") => other["--server=".len()..]` in `devtools_cmd.rs` and `lsp.rs`: an if-let guard needs a newer Rust, and the workarounds are not clearly better. `llm/screens.rs`'s `rfind("\n--- ")`, which keeps the marker. Byte scanners and buffers that keep a checked position (`guard.rs`, `cluster_edge`, `git_bridge`, `ssh/*`, `term`, `termws`, the SMTP PROXY header, `video.rs` `jpeg_size`, `raw.rs` dechunk, `lsp.rs` `word_at`, `devtools_remote.rs` `component_name`). `expect`s that state their reason (the `factory()` accessors, `http.rs` `inner`, the edge runtime, moqui-browser's `build.rs`).
- About 55 `&Arc` sites that reach `new_context` or `try_submit`, clone into a task or thread, or downgrade (`tui`, `smtp`, `ssh/cluster`, `monitor`); the example's `ToolFactory` methods; `Slot::preauth` and `connect`, which clone. `needless_pass_by_value` on `map_err` mappers, values a thread owns, the sftp trait methods and `ipp` `http_url`. `test_client/mod.rs` `Arc<Vec<u8>>`, which wraps an owned `Vec` that `Arc<[u8]>` would copy.
- `smtp.rs` `received_protocol(lmtp, esmtp, utf8, tls)`: private, with one call site that passes named fields.

### moqui-llm (§5, §6, §7)

Commits 072b1762, b9dfb1d1, 6cec3a09, deeb0daf. moqui-llm had no non-test `unwrap()`.

- §5: check-then-expect restructured in `approval::decide_in` (the waiting run bound by `filter` and let-else), `mcp::client` (the first input request taken in the let-else that checks the map), `tools::lazy` `resume` (the add-on entry read once) and `tools::schema::name_matches` (`split_once` and `rsplit_once`, no expects). String slices to `split_once`, `strip_prefix`, `split_at_checked` or `get` in `json_mode` (`only_tool_call_blocks`, `first_object`, `tail_with_intent`), `skills` (headings, `unquote`, `front_values`, the front-matter split), `tools::search::word_forms`, `tools::data`, `tools::screen::replace_outcome`, `prompt::cap_text`, `memory`, `window::canvas_version`, `ui::openui`, `ui::qvue_screen::parts`, `ui::screen_xml::header_named` and the MCP server's etag. `header_named` panicked on a program whose first comment closes inside its own opening (`<!-->`); it now reads to the next `-->`. New tests in `json_mode_tests.rs`, `skills/tests.rs` and the inline modules.
- §6: 20 functions that only read the factory take `&ExecutionContextFactory` (`a2a::ops::wait_canceled`, `a2a::push`, `chat::observe`, `trigger` `observe`, `failed` and `inbox`, `run::lease::renew`, `run::wait_for`, seven in `tools/screen.rs`, `ui/write.rs` `openui_request`, `check_write` and `check_read`). Read-only parameters borrowed in `a2a::card`, `tools::search::add_group`, `run::step` `run_calls`, `assist::read::approval_card`, `assist::canvas`, `ai_task::task_service`, `chat::chat_call`, `trigger::inbox` (`&Json`; callers in moqui-host's `agent_members` test), `tools::screen::held_write` and `tools::lazy::run_add_on` (so `resume` no longer clones the call's parameters); `run::step` `add_active_time` and `usage::larger` lose a clone; the `mcp::tools` list cache is `Arc<[Listed]>`.
- §7: twelve inline poison recoveries in `embed`, `provider` and `usage` use `moqui_lib::sync::lock`; `for` loops in place of `for_each` in the JSON walks (`window::collect_ids`, `tools::data`, `ui::proxy`); `a2a::ops::Follow::of` drops its two bools, and the two callers that want one set it by name.

Left:
- 72 `&Arc<ExecutionContextFactory>` parameters that reach `new_context` or `screen::make_render`, clone or downgrade for a thread, ticker or `Weak`, are stored (`a2a::ops::Ctx::new`, `mcp::tools::Ctx::new`), or match a hook (`canvas_overlay`), and their callers. `&Arc<LocalRun>` in `run/mod` `forget_if` and `run/step`, for `Arc::ptr_eq`.
- Byte scanners that keep a position (`prompt::elide_markers`, `tools::format`, `ui::partial_program`, `usage`'s redact flush) and ranges a scanner returns (`json_mode`, the provider's JSON range). `expect`s that state an invariant (`ui::vue_sfc`'s block names, `mcp::state`'s HMAC key).
- The a2a JSON builders, which take their parts by value as `json!` does; `script.rs` `llm_complete` (rquickjs); `provider::fake`. Condvar waits. `a2a::log_in(request, bearer, api_key)`, `pub(crate)` with named conf fields at the A2A caller. Eight 3-or-more-tuple returns whose callers bind names at once (`jsonrpc::envelope`, `classify::error_fields`, `ui::write::openui_request`, `qvue_screen::parts`, `assist::write`, `run::step` `build_request`, two in `skills::learn`).

### moqui-devkit, dev-aide and moqui-tui (§5, §6, §7)

Commits fc48c1d8, 7f1f43bd, 7440d3cc, a8bc1eea, d49170ea.

- §5: devkit `extract::service_prefix` and `extended_location`, `check::parse_error_position`, `lint_hazards` (a literal's body by `Chars::as_str`), testkit `named_not_built` and `http_not_built` use `split_once` and the like, and `index::find` matches `[head, tail @ ..]`. dev-aide `entities::data_type_of` is rewritten on `split_once`; `repo`, `vcs` and `git::parse_status` use `get`; `propose::new_titles` `split_once`; `changeset::get_change_set` loops over the prepared files in place of `prepared[0]`. tui `report_editor` names a new panel with a counting loop in place of `(1..).find().unwrap()` and completes the last word as `suggestions()` does; `session/assist.rs` uses `strip_prefix("turn:")`; `forms::row_suffix` uses `rsplit_once`. Three panics on input are gone: dev-aide `data_type_of` with a non-ASCII quote after `type=` or `type=` at the end of the head it reads, the tui query editor's Tab after multi-byte whitespace, and an assist lane `"turn"` with no message id. New tests for each.
- §6: devkit `reload_entities`, `reload_services`, `reload_eecas`, `reload_screens`, `reload_skills`, `check_extensions` and `check_references`, and dev-aide `screens::path_of` and `buffer_overlay`, take `&ExecutionContextFactory`; testkit `load` and `install_prelude_for` take `Option<&dyn SpecHost>`. dev-aide reads a ChangeSet with `ChangeSet::deserialize(&args)` in place of `from_value(args.clone())` (4 sites); tui `write_document` moves the document, and three `&x.clone()` in `overlays` and `shell` are gone.
- §7: the lock helpers in devkit `watch.rs`, `index/mod.rs` and `reload.rs` are gone, and devkit `schema.rs` and dev-aide `git`, `notify`, `screens` and `services` lock through `moqui_lib::sync`. The 8 tui `session/*.rs` modules name their imports in place of `use super::*`; the 5 `for_each` in tui `layout.rs` and `model.rs` are `for` loops; tui `decode::press(key, alt, ctrl, shift)` became `alt(key)` and `ctrl(key)`. Named structs in place of tuples: devkit `ScriptName`, dev-aide `FormField`, `RootFile`, `StoredToken`, `Tracking` and `Found`, tui `DrawerRow` and `CardLine`.

Left:
- Scanners that keep a position: `groovy_convert.rs` (about 170 sites), `screen_lint.rs`, most of `lint_hazards.rs`, testkit `lines.rs` and `rewrite.rs`. Slices on boundaries the code holds (pulldown-cmark ranges in tui `markdown.rs`, the `Vec<char>` cursors in `wrap_word` and the draw modules, `layout.rs` truncation, regex match starts). Indexes after a length check (the git field parsers, `chunks()` batches, `report.rs`'s computed ranges). `expect` on static regexes.
- `&Arc` that reaches `new_context` or clones into a thread: the index API, `reload_paths`, `run`, `reload_secas`, `check_text`, `check_file`, `typegen::generate`, testkit `run_parallel` and `run_thread`, `watch`; dev-aide `notify` (the reload listener type); tui `run::send` and `session_loop::settle`. `changeset::preview_overlay` keeps `&Arc`: it must match `screen::overlay::Provider`. `build_files(Vec<Located>)`, which the host's tests call, and `audit_git(detail: Value)`, which feeds `json!`.
- tui `FakeLink::answers()` keeps an inline poison recovery (moqui-tui does not depend on moqui-lib); `after_screen(navigate, path_changed)`, `pub(super)` with named variables at its one call; testkit `SpecHost::after_case(failed, allow_problems)`, a trait moqui-host implements; `groovy_convert`'s count tuples and `rewrite_expression`.

### moqui-report, moqui-mail, moqui-dav, moqui-git and moqui-collab (§5, §6, §7)

Commits 8fc22878, fc361767, dc7f7caa, 5ce59fcc, b569e3ab, 1d96940f, 56ad5d93.

- §5, report: `execute_comparison` and `normalize_intervals` read the range's from and thru with let-else (the known §5b site), and the bucket config is bound once with `if let` (§5c). `constraints` makes its timestamps with `map_err`, `alert.rs` `stamp` returns a `Result` and `muted` matches no timing for a time out of range: a request or alert time outside a timestamp's range is an error where it panicked. A non-string webhook `headerProperties` value is a send error. The subtotal count goes into the map before it becomes JSON; the default sorts are `SortField` literals in place of `parse(..).unwrap()`; `identity_filter_value` matches `strip_prefix("userAccount.")`. `frame.rs`, `definitions.rs` `copy_definition` and `repeat.rs` `substitute` (a `split_once` walk) need no unwrap or index.
- §5, mail: `bare_address`, `is_message_id`, `decode_mutf7`, `header_message_id`, `copy_uid` (a slice pattern), `apply_rename_folder`, `issuer_matches` and `find_recipient` use `split_once` and the like.
- §5, dav: a new `pim::line::strip_prefix_ignore_case` reads `mailto:`, `TZID=` and `RRULE:`, and `ical_utc_millis` reads its digits with `get`, so a calendar-query time range, an attendee, an RRULE or an RDATE/EXDATE with a non-ASCII character there gives no match or no time where it panicked. `line.rs` `unfold` and `parse_line`, `zones.rs` `parse_offset`, `destination_path`, the multiget hrefs and the elFinder rename and `parent_location` use `split_once` and `rsplit_once`.
- §5, git: short ids come from `to_hex_with_len(10)`; `index.rs` and `gc/local.rs` use `rsplit_once` and `split_at`; the webhook HMAC `expect`s "HMAC takes a key of any length" in place of a fallback to an empty key that could not happen.
- New tests for `substitute`, the mail address, message id, COPYUID, issuer and mUTF-7 helpers, dav `parse_offset`, `parent_location`, `strip_prefix_ignore_case` and the non-ASCII calendar times.
- §6: collab `tick_room`, `flush` and `save_room` take `&Mutex<Room>`; the report's shipped definitions are `Arc<[Json]>`; git `schedule_on` takes `&str` and `put_file` `&[String]`.
- §7: report `annotations.rs`, `definitions.rs` and `git_sync.rs` name their imports; report locks go through `moqui_lib::sync` (a poisoned pending push or remove slot in `git_sync.rs` is now recovered and run, where `.lock().ok()` skipped it); collab's `locked` helper is gone (22 calls). mail `TestStep { step, ok, message }` and `SrvRecord { priority, weight, port, target }` replace tuples, and the `dns_lookup` bools get `/* with_srv */` and `/* with_mx */`; git `index.rs` `walk` returns `WalkEnd { Stop, Max, Root }` in place of two bools never both true.

Left:
- `serde_json::to_string(..).unwrap()` on JSON values in `frame.rs`, which cannot fail; git `revwalk.rs`'s `expect("a queued commit has its node")`. Fixed-width slices of text checked to be ASCII digits or hex (dav `vcard.rs`, `zones.rs`, `ics.rs` `parse_date`, the lock token), dav `line.rs` `value_start`, `&after[slash..]` that keeps the slash. The git LFS routing arms (`&path["objects/".len()..]`), which need if-let guards. Byte scanners: mail `discover.rs` DNS packets, git `gc.rs`, `delta.rs` and the pack code.
- `&Arc<ExecutionContextFactory>` in report `git_sync.rs` (`new_context`), mail `idle.rs` and `remote.rs` `request_push` (clone, downgrade, spawn), git `construct` and `install` (the register hook). git `db.rs` `ObjectCache`'s `Arc<Vec<u8>>`, built from `Vec`s and handed back by `Arc::unwrap_or_clone`. `map_err` adapters, receive's `report`, `git_sync.rs` `record`, schedule's `out(Json)`.
- collab `read_page` and git `into_content` 3-tuples of distinct types with one caller each; `dns_lookup`'s two bools, private and now commented.

### Open after the review

- `ExecutionContext::push_checked(.., requires_authz: bool, count_tarpit: bool, ..)` is a public two-bool function
  (§7) with 31 calls across core, moqui-llm, moqui-host and tests; an enum there is a change of its own.
- Match arms that slice after a `starts_with` guard (`devtools_cmd.rs` and `lsp.rs` `--server=`, the git LFS routes)
  wait for if-let guards in match arms.
- `moqui_lib::sync` has no helper for `Condvar::wait` and `wait_timeout`, so those keep `unwrap_or_else(PoisonError::into_inner)`.
