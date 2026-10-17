# Code Quality and Problematic-Pattern Review of `crates/`

> Archived 2026-10-16: resolved on 2026-10-15 (the six fix phases under [Resolution](#resolution-2026-10-15)). What was left open is listed in [Remaining-Work, Code quality review leftovers](../Remaining-Work.md#code-quality-review-leftovers); the behaviour changes that differ from Java on purpose are in [Parity-Diffs](../Parity-Diffs.md#code-quality-review-2026-10-15).

Date: 2026-10-14. Scope: all non-test Rust in `crates/` (about 138k lines across 6 crates). This review checks the code on its own terms, not for parity with Moqui Java. `../moqui` was used only to understand intent. Where an issue also exists in Java, it is marked **(inherited)**.

## Verdict

The base is sound. SQL assembly is injection-safe everywhere (identifiers are quoted, values are bound, names are resolved against definitions). Decimal math uses checked ops. Secret comparisons are constant-time. The lock-poisoning policy (`into_inner`) is consistent. rquickjs callbacks are panic-guarded. The two riskiest `unsafe` sites, the MySQL stream transmute and the browser FFI exports, are sound under their stated invariants. Clippy found no `unwrap`/`expect`/index panic reachable from external input: nearly every hit is in test code or a verified invariant.

The real problems are not in individual lines. They are a few repeated design patterns:
- per-call state kept in shared slots
- begin/end pairs matched by counting, not by RAII
- side effects that happen at write time instead of at commit
- locks held across I/O
- caps that are configured but not enforced
- several ways for one input to **abort** the process (stack overflow, allocation failure), which `catch_unwind` cannot stop

Totals after verification and de-duplication: **15 High, about 55 Med, about 95 Low.**

### Method
- Clippy with the review lints added: `cargo clippy --workspace --all-targets --message-format=json -- -W clippy::unwrap_used -W clippy::expect_used -W clippy::indexing_slicing -W clippy::panic -W clippy::large_enum_variant -W clippy::too_many_lines -W clippy::significant_drop_tightening -W clippy::await_holding_lock -W clippy::redundant_clone -W clippy::needless_pass_by_value`.
  - Non-test hits: 1,669 `unwrap_used`, 585 `needless_pass_by_value` (533 of them are rquickjs binding signatures, dismissed), 584 `indexing_slicing`, 177 `expect_used`, 59 `panic`, 46 `too_many_lines`, 41 `significant_drop_tightening`, 18 `redundant_clone`.
  - Each was triaged in context, and only one was a real issue (`session.rs:1221`). The worst panic, `object.rs:930`, is a `str` byte slice, and that falls under `clippy::string_slice`, which this lint set does not include.
- Grep sweeps for `unsafe` (34 sites), `#[allow(` (41), `let _ =` (319), globals and `thread_local!` (62), `Result<_, String>` (63) and `println!`.
- Eight parallel slice reviews read all non-test code in full. Every High finding below was then re-read at its `file:line` by the coordinator, and several were cross-checked against Java.

**Build hygiene:** `cargo clippy --workspace --all-targets` currently **fails**. `crates/moqui-host/tests/search_bench.rs:144` trips `clippy::type_complexity`, which the workspace denies. It was added by the most recent commit.

---

## Cross-cutting issues

### 1. Process-abort paths that `catch_unwind` cannot stop
A stack overflow or a failed allocation aborts the whole server, not just one request.
- **Unbounded recursion on input:**
  - `script/runtime/convert.rs:189` `js_value_to_host` (cyclic JS object → abort, **High**)
  - `moqui-lib/src/node/parse.rs:281` XML tree parser (data files and sync XML)
  - `entity/load_feed.rs:3837` hand-written JSON parser
  - `user/session.rs:1768-2019` hand-written JSON parser
  - `moqui-lib/src/object.rs:974` `like_match`
- **Allocation sized by input:**
  - `service/images.rs:217` resize (**High**)
  - `service/spreadsheet.rs:108` calamine dense range
- **Per-engine stack budget:** each nested QuickJS engine gets a full 1 MB stack on a 2 MB worker thread (`script/runtime.rs:527`).
- **No catch at the boundary:**
  - `extern "C"` browser exports have no `catch_unwind` (`moqui-browser/src/exports.rs:90`).
  - QuickJS class finalizers run payload `Drop`. `sftp.rs:514`'s `Drop` does blocking network I/O.
- **Pattern fix:**
  - Use a depth limit (with a visited set where cycles are possible) on every recursive walk of external data.
  - Replace hand-written JSON with `serde_json`.
  - Check dimensions and cell counts against a budget before allocating.
  - Give workers explicit larger stacks and set a smaller QuickJS stack per engine.
  - Wrap each FFI export in `catch_unwind`.

### 2. Per-call state kept in shared slots
- `entity/load_feed.rs:236-299`: matched ECA rule ids are stored in a facade-wide slot and read back later (**High**).
- `entity/load_feed.rs:187` `FlagGuard`: `dummy_fks` and `disable_fk_create` flip the facade-wide flags for one load (**High**).
- `entity/load_feed.rs:250,503`: data-feed touches and `feed_services` live in a facade-wide Vec (**High**).
- One facade `slots` mutex serializes all of it (`entity/facade.rs:253`).
- Process-global state that should belong to a factory:
  - `cluster/mod.rs:75-82`, where the last factory to start wins and `instance_id` is shared
  - `transaction/manager.rs:1053,1136` (deadline slots, lock tracking)
  - `script/runtime/facades.rs:125` `TEXTS`
  - `screen/pdf.rs:42`
  - `sftp.rs:165`, whose slot holds per-runtime `known_hosts` state
  - `web/mod.rs:598`
  - `user/authz.rs:40`

  Search already uses `factory.extension`. That is the pattern to copy. The stateless transport slots (print, SSO, outbound) are fine as they are.
- **Pattern fix:** pass per-call values as arguments or in per-frame or per-thread scope guards. Never write facade or process state for one operation.

### 3. Begin/end pairs matched by counting, not by RAII
- **Transaction join depth is a bare counter** (`transaction/manager.rs:185-213`). A joined `begin` that is never balanced turns the owner's rollback into a no-op, and `let _ = self.rollback()` at :777 hides it. The concrete trigger is `entity/load_feed.rs:1902-1920` `store_record` (**High**). `entity/etl.rs:114,177` and `entity/facade.rs:382` use the same raw pattern.
- **Context frames and stacks pushed and popped by hand:**
  - 16 sites in `screen/` (`sri.rs:333…`, `statics.rs:604…`, `tree.rs:188`)
  - `script/runtime/ec_extra.rs:41`, `template.rs:101`, `actions.rs:44`, `screen_host.rs:291`
  - generated `ScreenQvt.js` `pushContext`/`popContext` with no try/finally

  `host_guard` turns panics into catchable JS exceptions, so a skipped pop leaves later code running on a misaligned stack.
- **Other thread-locals restored only on the normal path:**
  - `script/runtime.rs:399` `with_context` (`tracked`, the wrapped `ec`)
  - `screen/cache.rs:341` `LOADING`
  - `entity/cache.rs:372` sim session, which is never cleared at request end, so later writes on that thread go to a throwaway schema
- **Cleanup that only some threads do:** service-worker threads never call `destroy_all_in_thread` (`service/pool.rs:702`). Only the request pool does.
- **Job lock left held for 24 h** after a panic or a shutdown (`service/pool.rs:706`, `jobs.rs:375`).
- **`ExecutionContext::enter` is unsound from safe code** (`context/factory.rs:589-629`). `mem::forget(guard)` or out-of-order guard drops leave `with_current` dereferencing a dangling pointer. No call site misuses it today. Fix: a scoped `run_entered(|| …)` API.
- **Pattern fix:** the RAII tools already exist. Make them the only way in:
  - `tx.run` / `UnwindGuard`
  - `ContextFrame`
  - `RequestCleanup`
  - a `TxScope` guard returned by `begin`

  Make the raw `begin`, `push_context_frame` and `pop_context_frame` `pub(crate)` or remove them.

### 4. Side effects at write time instead of commit
- `entity/cache.rs:278` `on_write` clears the entity cache when the statement runs. A cached find in the same transaction stores uncommitted rows, and a concurrent reader can re-cache old rows before the commit (**High**).
- `user/authz.rs:40` bumps the authz generation before commit, and `with_cache` stores checks loaded under an older generation.
- `service/mail.rs:1312` deletes mail from the server before the poll's transaction commits (**High**).
- `cluster::note_write` already defers its publish to `on_commit`. Use that model for all of these.

### 5. Locks held across I/O
- `user/session.rs:1219` `flush_id` holds the factory-wide session table lock across reserve-pool checkout and SQL (**High**). `pool.rs` shows the right pattern: copy under the lock, then do the I/O without it.
- `entity/sequence.rs:118`: one global mutex across the bank refill.
- `transaction/manager.rs:307,1109`: a global deadline mutex, plus an allocation, on every statement.
- `script/runtime/entities.rs:1014`: the entity-cell mutex is held across create and update, including the ECA scripts they run. That is a re-entry deadlock.

### 6. Caps configured but not enforced, and unbounded growth
- **`MCache` ignores `max_entries` when background eviction is off** (`moqui-lib/src/cache.rs:282-336`). `put` never evicts and `evict_overflow` has no production caller. `screen.url` and `screen.theme.icon` (`screen/cache.rs:245`) are therefore unbounded, and the URL key includes request paths (**High**).
- **Background eviction costs one OS thread per bounded cache**, each waking every 50 ms (`moqui-lib/src/cache.rs:644`).
- **Other unbounded growth:**
  - `screen/generate.rs:103` `Generated.units`
  - session table and Visitor/Visit rows per cookieless request (`user/session.rs:843`)
  - tarpit map (`context/factory.rs:56`)
  - SSH SFTP whole-file buffers and shell lines (`ssh/sftp_fs.rs:238`, `ssh/server.rs:532`)
  - Postgres aggregation rows, loaded before the cap check (`search/postgres/agg.rs:198`)
  - `pageSize`, hybrid `k`
- **The HTTP listener has no header, body or request timeout and no connection cap** (`moqui-host/src/http.rs:101`). axum 0.8 `serve` sets no timer.
- **Slots can be held without authenticating:** the cluster HTTPS slots (64) and SSH sessions (8, counted before auth).

### 7. Fail-open and swallowed errors
- **Authz:**
  - A DENY whose `filterMap` fails to evaluate is skipped (`user/authz.rs:264,328`).
  - Non-`<expression>` ECA conditions run the rule anyway (`service/rule_actions.rs:35`).
  - Security ECAs are skipped while error messages exist (`service/eca.rs:68`) (inherited).
- **Search:**
  - Any non-200 from `index_exists` counts as "missing" (`search/opensearch.rs:725`).
  - `_shards.failures` is ignored, and a manual-mapping service error is swallowed (`opensearch.rs:622,824`).
- **Screen:** a `SubscreensItem` query error is cached as "no rows" for the life of a production process (`screen/definition.rs:1322`).
- **SSO:** every error becomes a successful call that commits a partial account sync (`service/sso.rs:47`).
- **Logging:** a failed log reopen disables file logging for good, and a failed rename deletes the current log (`moqui-host/src/logging.rs:263`).
- **Errors identified by message text:**
  - `web/mod.rs:1258` `status_for`
  - `screen/render.rs:1676` "rollback-only"
  - `web/error_info.rs:97`
  - `context/error.rs:98` drops `ErrorKind` at the moqui-lib boundary.
- `let _ =` on rollback and close: `transaction/manager.rs:777,991,995,1004`.

### 8. Test hooks and dead code in the library
- **Stale `#[allow(dead_code)]` with false comments** on production functions: `user/login.rs:31,529…`, `user/session.rs:483…`, `web/mod.rs:447`. Only `add_flash_message` and `reset_session` are really test-only.
- **Test switches checked in production code:** mail `SINK`/`FAKE` on every send and poll (`service/mail.rs:1088`), `SEALED_ENTERED` (`security_checks.rs:41`), and public `ServicePools::bounded`.
- **Test code under `src/db`:** about 2,150 lines of test-only modules (`entity_cache.rs`, `field_rw.rs`, `load_feed.rs`, `query_rest.rs`).
- **The libxml2 shim is copied 7 times** in moqui-host. The 6 test copies call `set_var` without the lock.
- **Unused moqui-lib code:** about 30 public helpers and the whole `sgml.rs` module have no production caller.

### 9. Duplication and reinventing dependencies
- **`load_feed.rs` (4.7k lines)** implements its own ZIP, DEFLATE and JSON. `serde_json` is a direct dependency, and `flate2`/`zip` are already compiled. Its Huffman decode is a linear scan per bit.
- **Small helpers repeated:**
  - a second hand-written JSON codec in `session.rs`
  - FNV ×3 (`search/`)
  - hex encoders ×3
  - `constant_time_eq` ×2
  - `/dev/urandom` readers ×2, while `getrandom` is used elsewhere
  - statement LRU ×2 (MySQL, Postgres), with an O(n) touch on the find path
  - install slot ×4
  - lock helpers in every crate
  - service helpers (`has_error`, `now_millis`, `text`) in 6–8 files each
- **Oversized files:** `screen/sri.rs` (2k lines) and `service/mail.rs` (2.9k lines, with 49 `Result<_, String>`).
- **Pattern fix:** add a `moqui_lib::sync::lock` helper, `moqui_lib::codec` (hex, constant-time compare, base64), and one `InstallSlot<T>`. Split `load_feed.rs` into eca, feed, and loader/{xml,csv,json,zip}.

### 10. `set_var` with other threads running (soundness)
`moqui-host/src/lib.rs:1545` sets `LD_LIBRARY_PATH` after factory boot has started the worker and scheduler threads. It is reached from `db/entity_db.rs:124` and `search.rs:177`. That is UB if C code calls `getenv` on another thread at the same time. Set the variable on the child `Command`, or before any thread starts.

---

## High findings (verified)

| # | Where | Problem | Fix |
|---|---|---|---|
| H1 | `moqui-core/src/script/runtime/convert.rs:189-268` | `js_value_to_host` recurses with no depth or cycle guard. `a.self = a; result = a` or `list.push(list)` overflows the stack and **aborts the server**. It runs on every bare assignment and host-call argument. | Depth limit (about 200) plus a visited set of object pointers. Do the same for `screen_host.rs:247` `lit`. |
| H2 | `moqui-core/src/script/runtime/facades.rs:171` → `runtime.rs:354` | Script files (`type="script"` services, `ec.resource.script`) run as **global** code on the thread's long-lived engine. A top-level `let`/`const` throws "redeclaration" on the 2nd call on that thread. A top-level `var`/function becomes a `globalThis` property that hides the context proxy, so values **leak across requests and users**. `groovy_convert` output is not wrapped. | Wrap file text as `(function(){…})()`, as action scripts are, or reject top-level declarations at load. Freeze the helper globals. |
| H3 | `moqui-core/src/service/images.rs:217-221` | Resize height comes from the input aspect ratio with no cap. A 1×60000 PNG resized to the default 2160 width asks for about 1 TB, and the failed allocation **aborts**. Reachable from upload services. | Cap output dimensions and total pixels, refuse upscaling, set `ImageReader::limits`. |
| H4 | `moqui-lib/src/object.rs:930` `take_n_digits` | Checks length in bytes, then slices a `str`. `parse_time("1é")` panics. Reached from form fields, l10n, REST and every String→date/time conversion. | `input.get(..n)?` and an ASCII byte check. |
| H5 | `moqui-core/src/service/mail.rs:1312-1340` | The poll deletes or expunges messages on the server inside one 600 s service transaction. A later error (`note_error` → rollback-only) or a timeout rolls back the saved `EmailMessage` rows of messages already gone. **Mail is lost.** | One transaction per message, and delete only after that commit. `system_sync/sftp.rs:268` already does this. |
| H6 | `moqui-core/src/entity/load_feed.rs:236-299`, `value.rs:239-278`, `service/eca.rs:63` | Matched ECA ids are written to a facade-wide slot (`last_matched`) and read back by `run_matched_eca`. A concurrent write, or a nested write from a run-before rule, overwrites the slot, so the **outer write's rules are skipped**. | Pass the ids returned by `match_rules_with` into `run_matched_eca`, and remove `last_matched`. |
| H7 | `moqui-core/src/entity/load_feed.rs:187-221`, `etl.rs:129` | One load's `dummy_fks`/`disable_fk_create` set the facade-wide flags, so every thread sees them. A restore race can leave them on permanently. A sync partner can trigger it through `put#EntitySyncData dummyFks=Y`. | Pass per call (an argument, or a thread-scoped guard read by `fk::enforce`). |
| H8 | `moqui-core/src/entity/cache.rs:278-289`, `find.rs:245-258`, `cache.rs:208` | The entity cache is cleared at write time, not at commit or rollback. A cached find inside the writing transaction stores **uncommitted rows** that survive a rollback. Concurrent readers re-cache old rows before the commit, and they then stay. | Clear in `on_commit` and `on_rollback` hooks, as `cluster::note_write` does. Skip `after_find` while the frame has written that entity. |
| H9 | `moqui-core/src/entity/load_feed.rs:1902-1920` and `transaction/manager.rs:185-213,565-586,777` | `store_record` joins an outer transaction (`began == false`) and, on error, neither commits nor rolls back. The extra join makes the owner's rollback a no-op (`let _ =` hides it), the frame stays active with partial writes, and `destroy_all_in_thread` later **commits** it. | `store_record` → `tx.run(UseOrBegin, …)`. Structurally: `begin` returns a `TxScope` guard, and `run_bounded` checks that it completes its own frame. |
| H10 | `moqui-core/src/entity/load_feed.rs:250-258,503,788-793` | A `Touch` (a clone of every field) is pushed before the write can fail. The cleanup hook is registered only on success, so failed first writes leak it, and they later emit bogus `DataFeedPending` rows. `feed_services` is appended on every commit when any realtime feed exists. **Both grow forever.** | Keep touches per frame and push them after success. Make `feed_services` a set. |
| H11 | `moqui-core/src/user/session.rs:1219-1246` | `flush_id` holds the factory-wide session table lock across reserve-pool checkout (wait up to 30 s), SQL and `cache::on_write`. Every request takes that lock in bind, touch and unbind. **All requests stall during flushes**, and in guarantee mode after every request. | Copy under the lock, do the I/O unlocked, re-lock and apply when the revision is unchanged. |
| H12 | `moqui-lib/src/cache.rs:282-336` + `moqui-core/src/screen/cache.rs:245-256` | `max_entries` is silently ignored when `background(false)`. `screen.url` (key includes request path segments, so transition and extra-path suffixes vary) and `screen.theme.icon` grow without bound. | Enforce the cap inline in `put` (amortized `evict_overflow`), or reject that config. |
| H13 | `moqui-host/src/ssh/mod.rs:42-45`, `ssh/shell.rs:89-93` | `on_pool` holds `disable_authz` for the whole job. The shell's `service`/`entity`/`js` commands run after `internal_login_user` inside that job, and the depth counter is never reset, so **authorization is off**. That contradicts the doc comments of both the module and `on_pool`. | Hold the guard only around the login step. Add a refusal test. |
| H14 | `moqui-core/src/screen/saved_finds.rs:519-525` + `ScreenServices.xml:88`, `screen/definition.rs:281-306` | User-entered `_findDescription` goes verbatim into `ScreenScheduled.emailSubject`, which `render#ScheduledScreen` passes to `ec.resource.expand`. That is **server-side code execution** via `${…}`. The implicit `formSaveFind` transition needs no session token, so CSRF works too. **(inherited:** `ScreenForm.groovy:2400`, `ScreenDefinition.groovy:1135`). | Never build a template from user text: escape `${` or format the subject at render. Require a session token for `formSaveFind`/`formSelectColumns`. Check find ownership in ScheduleFind and MakeDefault. |
| H15 | `moqui-core/src/entity/query_rest.rs:1803-1833` | Long or multi-line values are written raw inside `<![CDATA[…]]>`. A value containing `]]>` **injects records** into entity-sync and export XML. `UserAccount` is not restricted on `put#EntitySyncData`. **(inherited:** `EntityValueBase.java:1170`). The same bug is in `moqui-lib/src/node/mod.rs:421`. | `text.replace("]]>", "]]]]><![CDATA[>")`. |

---

## Medium findings

### Foundations (moqui-lib, moqui-db)
- **One OS thread per bounded `MCache`, waking every 50 ms.**
  - Where: `cache.rs:644` (one thread per cached entity's list cache).
  - Fix: one shared evictor holding `Weak` handles, with a `Condvar` wait.
- **`MCache::put` drops a second put to the same key within the same millisecond.**
  - Where: `cache.rs:320`.
  - Fix: always overwrite, or compare `seq`.
- **The XML tree parser recurses with no depth limit** (abort on deeply nested data files or sync XML).
  - Where: `node/parse.rs:281`.
  - Fix: a depth limit, or an iterative build like `XmlEvents`.

### Entity
- **One facade `slots` mutex** for one-caches, ECA rules and touches. Cached reads clone the whole list under it.
  - Where: `facade.rs:253`, `cache.rs:1063`.
- **The sequence bank holds a global mutex across the reserve-connection refill.**
  - Where: `sequence.rs:118`.
  - Fix: lock per sequence name.
- **Every write transaction pays uncached DataFeed/DataDocument queries plus savepoints at commit.**
  - Where: `load_feed.rs:466,795`.
- **The sim/TX-cache session is a thread-local not tied to the EC.** A missed `stopTxCache` sends the thread's later writes to the sim schema.
  - Where: `cache.rs:51,372`.
- **The hand-written JSON parser** recurses without limit, rejects surrogate pairs, and parses numbers to f64.
  - Where: `load_feed.rs:3837`.
  - Fix: `serde_json`, keeping numbers as text.
- **`load_feed.rs` mixes 7 concerns** and reinvents zip, inflate and JSON. Its Huffman decode is linear per bit.
  - Where: `load_feed.rs:3345-4022`.
- **`feed_documents` pages with OFFSET over a live table** (O(n²), skips and duplicates rows).
  - Where: `document.rs:224`.
  - Fix: keyset pagination.
- **`LIST_MAX` is enforced only every 30 s,** and entries are whole result lists.
  - Where: `cache.rs:1071`.
- **The data writer holds every table in RAM, and zip sizes over 4 GiB are written as `u32::MAX`** (corrupt archive).
  - Where: `data_writer.rs:333,487`.

### Service
- **A panicking job keeps its run lock for 24 h,** and nothing is logged. Shutdown drops queued jobs with their locks claimed.
  - Where: `pool.rs:706`, `jobs.rs:375`, `pool.rs:903`.
- **`authUsername`/`authPassword` on a service call log the user in and never log out.** Java calls `logoutLocal`. This is reachable with REST parameters.
  - Where: `call.rs:272`.
- **ECA `<condition>` children other than `<expression>` (compare, and, or…) silently pass** (fail-open, latent).
  - Where: `rule_actions.rs:35`.
- **SSO links to a local account by `preferred_username`/`email` alone,** never comparing `externalUserId` and without `email_verified`.
  - Where: `sso.rs:376`.
  - (inherited)
- **SSO errors become `Ok`,** so a partial group sync commits. `call()` also clears all messages.
  - Where: `sso.rs:47,283`.
- **Lazy service-file loading re-parses every loaded file** on each new one (O(files²) at cold start), and concurrent loads overwrite each other.
  - Where: `define.rs:475`.
- **A sparse spreadsheet can make calamine allocate the full declared grid.** Confidence: Possible.
  - Where: `spreadsheet.rs:108`.

### Screen
- **`paginate_items` ignores `{list}AlreadyPaginated`,** so page 2 and later render empty.
  - Where: `sri.rs:369`.
  - Note: the JS twin in `tools_host.rs:31` handles this correctly.
- **A failed SubscreensItem/Default query is cached as "no rows",** and production never re-checks, so DB-mounted apps vanish until restart.
  - Where: `definition.rs:1322`, `url.rs:370`.
- **`Generated.units` is unbounded** (dynamic and user-column forms).
  - Where: `generate.rs:103`.
- **Render hot path:** a full `ScreenUrlInfo` and `TransitionDef` clone per link, and `entity_shape` copies the field list per option and per row.
  - Where: `url.rs:102`, `urls.rs:248`, `form.rs:820`.
- **Internal error text reaches clients** in production JSON errors and section errors.
  - Where: `screen/mod.rs:182`, `sri.rs:200`. Also `moqui-host/http.rs:227`.

### Script
- **The raw `JS_EvalFunction` path never resumes a panic that rquickjs stored.** The panic resurfaces in a later, unrelated call.
  - Where: `runtime/compiled.rs:86`.
  - Fix: `host_guard` on the remaining unwrapped callbacks.
- **`with_context` is not unwind-safe:** stale `tracked` lists and the wrapped `ec` survive into the next request on that worker.
  - Where: `runtime.rs:399,153,510`.
- **No script time limit outside a transaction.** Nested engines each get a 1 MB stack on 2 MB threads, and `HostFuture.get()` has no timeout.
  - Where: `runtime.rs:165,527`.
- **Compiled-code caches (12,288 entries, counted by number not size) live inside the 64 MB JS heap,** so OOM later shows up as script failures.
  - Where: `runtime.rs:88`.
- **Per-thread generated-script caches,** with about 39 KB of macro JS re-run on every miss and the source copied for each cache key.
  - Where: `actions.rs:34,107`, `compiled.rs:37`.
- **The service-call builder clones the whole parameter map per chained call,** and `.parameters(context)` round-trips the entire context.
  - Where: `runtime/services.rs:56,146`.

### Core runtime
- **Service-worker threads never clear leftover transaction frames,** and the inline drain then stops for good.
  - Where: `service/pool.rs:702`.
- **`IgnoreSuspend`: the first nested transactional call resumes the suspended outer transaction mid-body.**
  - Where: `transaction/manager.rs:541,682`.
- **`ExecutionContext::enter` soundness.**
  - Where: `context/factory.rs:589` (see cross-cutting issue §3).
- **The authz cache can hold stale checks indefinitely** (pre-commit bump, store under a stale generation).
  - Where: `user/authz.rs:40,487,546`.
- **A DENY whose `filterMap` or pattern fails to evaluate is skipped** (fail-open).
  - Where: `user/authz.rs:264,322,328`.
- **Global deadline mutex, linear scan and canceller allocation on every statement in a transaction.**
  - Where: `transaction/manager.rs:307,1109`.
- **The session table and Visitor/Visit rows grow with every cookieless request.**
  - Where: `user/session.rs:843,1069`.

### Search
- **OpenSearch `index_exists` maps every non-200 to `false`,** so feed deletes are lost and searches come back empty.
  - Where: `opensearch.rs:725`.
- **`_shards.failures` and `_delete_by_query` failures are ignored.** A multi-type sort uses the first mapping's names with no `unmapped_type`.
  - Where: `opensearch.rs:824`.
- **A manual-mapping service error is swallowed,** and the index is never rebuilt.
  - Where: `opensearch.rs:622`.
- **The feed drain reads `limit(1000)` with no order or skip.** More than 1000 undeliverable rows starve every retry.
  - Where: `feed.rs:72`.
- **Read-your-writes never forgets undelivered rows,** so every later fresh search waits the full deadline.
  - Where: `fresh.rs:38`.
  - Also: `script/runtime/search.rs:264` `Instant + Duration` panics on a huge `freshTimeout`.
- **A fixed-offset zone becomes `AT TIME ZONE '+HH:MM'`** (POSIX sign inverted), so histogram buckets are wrong.
  - Where: `agg.rs:354`.
- **Aggregation reads every group before checking `MAX_GROUPS`.**
  - Where: `postgres/agg.rs:198`.
  - Fix: `LIMIT MAX_GROUPS+1`.
- **Postgres indexing makes one round trip and two clones per document.**
  - Where: `postgres/mod.rs:771`.

### Host and browser
- **`set_var("LD_LIBRARY_PATH")` runs after threads start.**
  - Where: `lib.rs:1545` (see cross-cutting issue §10).
- **Unauthenticated connections hold the cluster HTTPS slots (64) and SSH sessions (8, counted pre-auth) indefinitely.**
  - Where: `cluster_edge.rs:210`, `ssh/server.rs:198`.
- **The public HTTP listener has no header, body or request timeout and no connection cap** (slowloris).
  - Where: `http.rs:101`.
- **Postgres NOTIFY drops cache invalidations on 503 when the request pool is full,** with no retry.
  - Where: `cluster_notify.rs:290`.
- **MySQL statement timeout:**
  - It uses `max_execution_time`, which MariaDB lacks, so every timed transaction fails there.
  - On MySQL it bounds only SELECTs.
  - A sub-millisecond remainder becomes 0, which means no limit (both drivers).
  - Where: `db/mysql.rs:403`, `postgres.rs:445`.
- **A panic in a browser export aborts the whole WASM module.**
  - Where: `moqui-browser/src/exports.rs:90`.
- **Log rotation fails silently.** A failed reopen stops file logging for good, and a failed rename deletes the current log.
  - Where: `logging.rs:263`.
- **SSH memory is unbounded per session:** each SFTP open reads the whole file (up to 256 MiB) with no handle cap, and shell buffers have no limit.
  - Where: `ssh/sftp_fs.rs:238`, `server.rs:532`.
- **The libxml2 shim is copied 7 times;** the test copies race without the lock.
  - Where: `lib.rs:1438` and `db/*.rs`.

---

## Low findings (one line each)

**Foundations:**
- `like_match` backtracks exponentially (`object.rs:974`).
- The `order_map_list` comparator turns into "all Equal" after the first error, which can panic since Rust 1.81 (`collection.rs:105`).
- `${}` expansion and `operator` are written into view SQL verbatim (latent) (`sql/view.rs:439,518`).
- The `RestClient` SSRF guard is unused and open to DNS rebinding (`http.rs:181,431`).
- The regex matcher is O(n²) on nested quantifiers (`regex_match.rs:445`).
- `Not` of an empty render inverts to "true", and an empty `Or` reads as true (`sql/condition.rs:601,745`).
- About 30 unused public helpers plus `sgml.rs`.
- Dotenv is never cleared, and core tests change properties without the shared lock (`env.rs`).
- `FileResource::delete` turns I/O errors into `Ok(false)`, and `write_bytes` is not atomic (`resource.rs:341,415`).
- Linear field and alias lookups, and views re-rendered per find (`model/mod.rs:189`).
- `add_numbers` wraps on i64 while decimal ops error (`object.rs:340`).
- `SimpleEtl` stops quietly on a load error (`etl.rs:430`).

**Entity:**
- 115 `indexing_slicing` hits, none reachable from input. The one fragile byte slice is `meta.rs:426`.
- FTS DDL uses the field name, not the column name, and duplicates moqui-db (`meta.rs:444`).
- `EntityFind::iterator` skips `disable_authz` and require-search-params (`find.rs:351`).
- `dynamic_view` skips authz and filters (public API, no production caller) (`query_rest.rs:45`).
- The DataDocument cache is not invalidated on DataDocument writes (`document.rs:149`).
- Per-row crypto config clone and key derivation (`field_rw.rs:101`).
- Secrets in derived `Debug` (`crypto.rs:41`, `datasource.rs:40`).
- `probe_*` swallow savepoint-rollback failures (`load_feed.rs:1035`).
- Negative `pageIndex` gives a negative OFFSET, and `pageSize` is unbounded (`search_form.rs:93`, `query_rest.rs:642`).
- `RawReadGuard` does not restore a nested value, plus dead params (`field_rw.rs:46`).
- `flush_held` and `probe_*` bypass `exec::trace`.
- `/dev/urandom` read directly (`load_feed.rs:1131`).

**Service:**
- Service workers skip `destroy_all_in_thread` (see the Med above).
- Security ECAs skipped while errors exist (inherited) (`eca.rs:68`).
- POP3 commands end in LF, not CRLF (`mail.rs:1675`).
- Test hooks in production: mail `SINK`/`FAKE`, `SEALED_ENTERED`, `ServicePools::bounded`.
- `service::acquire` takes the semaphore and releases it at once (`semaphore.rs:32`).
- Swallowed release, delete and `getrandom` errors, and a malformed Message-ID host (`jobs.rs:261`, `mail.rs:1078`).
- HMAC receive accepts an empty shared secret (`system_sync/message.rs:232`).
- elFinder delete follows directory symlinks (`elfinder.rs:429`).
- Per-call `ServiceDef` clones and linear SECA scans (`define.rs:440`, `seca.rs:42`).
- Duplicated helpers (`reserve`, `has_error`, `now_millis`, `text`, lock).
- Mail and remote calls do blocking network I/O inside service transactions (note).
- Mail attachments can name any locatable file (inherited).

**Screen:**
- 16 manual frame push/pop sites (cross-cutting §3).
- `LOADING` thread-local is not cleared on panic, and two threads loading screens that include each other deadlock (`cache.rs:44,322`).
- `tree.rs:221` puts unescaped URL text into a `javascript:` href.
- JSON-list REST calls commit item by item and still answer with an error (`dispatch.rs:111`).
- One xlsx cell over 32,767 characters fails the whole export (`xlsx.rs:166`).
- Errors classified by message text (`render.rs:1676`, `schema.rs:83`).
- Guarantee-mode session not flushed on the error path (`screen/mod.rs:170`).
- Fail-open with no EC (`sri.rs:1957`, `render.rs:833`).
- `Avg` uses unchecked Decimal `+` (`aggregate.rs:314`).
- Static files read whole into memory, with no Range support (`static_files.rs:237`).
- Process-global PDF slot.
- `vue_check` `compile_errors` has no deadline, and one compiler runtime is kept per thread.
- Long functions, duplicated form-unit blocks, and `sri.rs` needs splitting.
- Anonymous saved-find writes, and a predictable `/tmp` report path (`statics/report.rs:630`).
- Stale comment `ScreenServices.xml:94` ("saved finds are not part of this port").
- The implicit `actions` transition returns the whole context as JSON (design note).

**Script:**
- The entity-cell mutex is held across create/update and their ECA scripts (re-entry deadlock) (`entities.rs:1014`).
- Hand push/pop of frames and flags (cross-cutting §3).
- SFTP `Drop` blocks on network I/O in a GC finalizer (`sftp.rs:514`).
- `keep_assigned` is correct for rquickjs 0.14.0 but neither pinned nor tested (pin `=0.14.0`, add a refcount test) (`entities.rs:1187`).
- `i64::MIN.abs()` (`convert.rs:77`).
- `TEXTS` is process-wide and never re-checked in production (`facades.rs:125`).
- `encodeHtmlJsSafe` does not escape `\` or U+2028/9 (inherited) (`ec_extra.rs:494`).
- `LAST_KIND` misattributes security errors (`screen_host.rs:28`).
- Duplicated `opt_text` (×5) and `js_error` (×3), plus a dead `fallback` param.
- `host_guard` lets scripts catch Rust panics and then commit (consider marking the transaction rollback-only).

**Core runtime:**
- `applicable_filters` loads 3 whole tables per filtered find (`user/filter.rs:111`).
- A commit-hook panic leaves the frame (`manager.rs:982`).
- Stale cancellers can cancel another thread's statement (`manager.rs:547`).
- `let _ =` on rollback and close.
- Stale `#[allow(dead_code)]` (cross-cutting §8).
- Process-global factory state (cross-cutting §2).
- Tarpit map never pruned.
- Factory mutex per counted artifact pop (`hits.rs:208`).
- `ErrorKind` lost at the lib boundary, and HTTP status read from message text.
- Login timing reveals which users exist, and the lockout gate is checked before the increment (`login.rs:59`).
- `period.rs:106` overflows on `poffset`.
- Duplicated codecs (session JSON, base64, hex, `constant_time_eq`, visit creation).
- `request_details()` does not filter credential parameters (`web/mod.rs:309`).
- `notify/message.rs:251` uses eager `unwrap_or(new_id()?)`.
- The `web/mod.rs:796` doc says 1 MiB, but the default is 10 MiB.

**Search and misc:**
- Hybrid `k` bypasses `MAX_WINDOW` (`hybrid.rs:129`).
- `parse_offset` overflow (`agg.rs:325`).
- `Query` `lucene` does not round-trip (`query.rs:444`).
- Highlight fragments are not HTML-escaped (`highlight.rs:166`).
- `Near` with 3+ words cross-joins positions (`compile.rs:512`).
- Negative `registered()` lookups go to a reserve connection on every search (`postgres/mod.rs:440`).
- DDL on a reserve connection can deadlock against the caller's own transaction (`postgres/mod.rs:694`).
- Mapping caches go stale across nodes.
- One failing document stops the embedding worker (`search/mod.rs:404`).
- Secrets in derived `Debug` (`sftp.rs:37`, `sso.rs:31`).
- `call_checked` clears earlier messages (`feed.rs:185`).
- SFTP default port 115, and IPv6 hosts are split wrong (`sftp.rs:29,201`).
- `run_level`/`semantic_sql` take 8 arguments.

**Host and browser:**
- CDP early-event map is never pruned (`pdf/cdp.rs:151`).
- Postgres rollback with an open stream fails before sending ROLLBACK (`postgres.rs:311`).
- `rewrite_placeholders` handles only single quotes (`postgres.rs:577`).
- `println!`/`eprintln!` in library code (`pdf.rs:96`, `search.rs:245`, `cluster_edge.rs:780`, `types.rs:67`, `lib.rs:172,1259`).
- The proxy echoes the upstream URL, possibly with userinfo (`search_proxy.rs:133`).
- Cluster senders keep connections to peers that left.
- `EmbeddedCluster::drop` swallows a failed stop.
- Blocking work on the 2-thread edge runtime (`pdf.rs:391`, `ssh/client.rs:86`).
- PDF network isolation and profile-dir cleanup gaps.
- IPP URI doubles the port (`ipp.rs:48`).
- `[::1]` local check fails, and `su`-wrapped OpenSearch is orphaned (`search.rs:271,327`).
- A new reqwest `Client` per call (`ipp.rs:136`, `sso.rs:330`).
- MySQL transmute relies on the Box not being retagged (use `NonNull` from `Box::into_raw`) (`mysql.rs:277`).
- Duplicate statement LRU with O(n) touch.
- Graceful shutdown has no deadline.
- Repeated boot and CLI prologue code.
- MySQL zero dates fail the whole query.

---

## Checked and found sound
These are noted so they don't get re-reviewed:
- **SQL injection:** none, in moqui-db, `query_rest`, `search/postgres`, or saved finds.
- **Decimal arithmetic:** all checked. The one exception is `screen/aggregate.rs:314` `Avg`.
- **Sequence IDs:** no duplicates under contention or rollback.
- **Locks and guards:**
  - Semaphore release paths are correct.
  - call.rs transaction, semaphore and frame handling is correct.
  - The `render.rs`/`report.rs` `ACTIVE` thread-locals, `web` request thread-local, `TX_HOLD` and `RAW_READ` all have proper RAII.
- **Static files:** traversal and symlinks are handled correctly.
- **Session and credentials:**
  - Session fixation and rotation are correct.
  - Constant-time comparisons are used throughout.
  - HTML cleaner is ammonia's defaults.
  - Mail header injection is prevented by lettre.
- **FFI and `unsafe`:**
  - rquickjs callback panics are caught.
  - `compiled.rs` reference counting is correct.
  - `keep_assigned` is correct on every path.
  - `as_bytes` copies are safe.
  - The MySQL transmute invariants hold.
  - `EmbeddedCluster` `ManuallyDrop` is correct.
  - Browser alloc/dealloc and `IN_CALL` are correct.
- **Bounded queues:** hits, the cluster outbox and the notify bus are all bounded.

## Suggested fix order
1. **One-line or local fixes for abort, data-loss and security Highs.**
   - H4 `take_n_digits`
   - H1 depth/cycle guard
   - H3 image caps
   - H15/CDATA split (both writers)
   - H13 SSH authz scope
   - H14 saved-find template plus session token
   - H6 pass ECA ids
   - H9 `store_record` → `tx.run`
   - Fix `search_bench.rs:144` so `clippy --all-targets` passes again.
2. **Transaction and commit semantics.**
   - H5 per-message mail transaction
   - H8 cache clear on commit/rollback
   - authz generation on commit
   - H7 per-call FK flags
   - H10 per-frame touches
   - `IgnoreSuspend` resume
   - Make join accounting structural (a `TxScope`).
3. **RAII sweep.**
   - `ContextFrame` at every push/pop
   - `with_context`, `LOADING` and sim-session guards
   - `destroy_all_in_thread` on service workers
   - job-lock release guard
   - scoped `ExecutionContext::run_entered`
   - H2 wrap script files
4. **Lock scope and bounds.**
   - H11 session flush
   - sequence bank
   - per-statement deadline mutex
   - H12 `MCache` inline eviction plus a shared evictor
   - HTTP timeouts and connection cap, and cluster and SSH pre-auth limits
   - `Generated.units`
   - session-table cap
   - `set_var` moved to the child process
5. **Fail-open sweep.**
   - DENY evaluation
   - non-expression ECA conditions
   - OpenSearch status handling
   - SubscreensItem error caching
   - SSO error handling and account linking
   - log reopen
   - typed `ErrorKind` instead of message sniffing
6. **Structure and dedup.**
   - Split `load_feed.rs` and use `serde_json`/`flate2`.
   - Shared moqui-lib `sync`/`codec`/`InstallSlot` helpers.
   - Remove the libxml2 shim copies and move `src/db` tests to `tests/`.
   - Clean up dead code and stale allows.
   - Gate test hooks with `cfg(test)`.

---

## Resolution (2026-10-15)

All six fix phases are done on `initial`: 58 commits after the review commit (9f34eda), with about 170 new tests (854 → 1024 `#[test]`).

The final gate passes:
- `cargo fmt --check` and `cargo lint` are clean.
- `cargo test --workspace` passes, and moqui-core passes 5 runs in a row (752 tests).
- `cargo test-slow` passes, 5 of 5.

The stale golden file and the two flaky tests seen during the work were fixed at their causes:
- the cluster state was made per factory
- the `instance_purpose` override was scoped to the test thread
- the in-memory database lost-update in the SFTP test was fixed

Every finding was re-checked against the code before it was fixed. Each fix was made in an isolated worktree, reviewed, and then merged.

| Phase | What landed |
|---|---|
| 1. Abort, data-loss and security Highs | H1, H3, H4, H6, H9, H13, H14, H15, the node CDATA writer, the `search_bench` lint |
| 2. Transaction and commit semantics | H5, H7, H8, H10; `TxScope` and frame checks; `IgnoreSuspend`; commit-hook panics; authz generation on commit; SSO sync made atomic and linked by `externalUserId` |
| 3. RAII sweep | H2; `ContextFrame` everywhere; unwind-safe engine state; parked rquickjs panics; pool cleanup and job-lock guards; local `authUsername` login; `ExecutionContext::enter` made sound with owned handles (no `unsafe`) |
| 4. Lock scope and bounds | H11, H12; per-name sequence locks; ECA index; deadline canceller per frame; shared cache evictor; session cap; `set_var` removed; HTTP, cluster and SSH limits; SFTP and shell caps |
| 5. Fail-open sweep | Unevaluable DENY now denies; full ECA conditions; typed `ErrorKind` and statuses; production 500s show only an error id; subscreens errors no longer cached; search status and shard handling; cluster state per factory |
| 6. Structure, dedup, remaining Meds | `load_feed` and `sri` split into modules; serde_json and zip replace hand-written codecs; lazy service-file load; script deadlines, stack and size-capped caches; batched Postgres indexing; render hot path; NOTIFY, MariaDB and WASM panic fixes; shared `sync`/`codec`/`InstallSlot` helpers; dead code and test hooks removed; remaining Lows |

### Behaviour changes
- **SSH shell:** `service`, `entity` and `js` now run under the Tools app's authorization, as Java's Groovy shell did. Before, they ran with authorization off.
- **Saved finds:** `formSaveFind` and `formSelectColumns` take only a POST with the session token. A saved find can be scheduled or made the default only by its owner or one of its groups. A description is never template source.
- **Transactions:**
  - Request and service workers roll back a frame left open by a unit of work, with an error log. Before, a request worker committed it.
  - A joined `rollback()` marks the frame rollback-only.
- **Scripts:**
  - A script file runs as a function body. If its last statement is not an expression, `ec.resource.script` returns undefined.
  - Assigning to a host global (`compare`, `toNumber`, `System` and others) throws.
  - A script outside a transaction on a request thread is stopped at `webapp_request_timeout`.
- **Sessions:** a client that sends no cookie gets no Visitor or Visit row until the cookie comes back or it logs in. Live sessions are capped.
- **Authorization:**
  - A DENY whose `filterMap` or pattern cannot be evaluated now denies.
  - Non-expression ECA conditions are evaluated, and an unknown condition child is an error.
  - Any `ArtifactAuthz` data with a Groovy-form `filterMap` (`[a: b]`), for example in Mantle, should be checked.
- **Errors:**
  - HTTP status comes only from `ErrorKind`, so some general errors that used to give 404, 405 or 403 now give 500.
  - In production, 500 bodies and failed sections show an error id instead of the detail.
- **SSO:**
  - The local account is linked by `externalUserId`, and a mismatched account is refused.
  - Email is used as the username only when it is verified, or is the provider's own username field.
- **Search:** OpenSearch partial shard failures are errors. Highlight fragments are HTML-escaped.
- **Other:**
  - Entity REST `pageSize` is capped; `pageNoLimit` still lifts the cap, as in Java.
  - The SFTP default port is 22.
  - Data-writer zip entries are deflated.
  - A panicking browser export resets its state.

### New properties
All are optional, with these defaults:

| Property | Default | Meaning |
|---|---|---|
| `webapp_header_timeout` | 30 s | |
| `webapp_body_timeout` | 60 s | |
| `webapp_request_timeout` | 300 s | Also the script limit on request threads |
| `webapp_max_connections` | 1024 | |
| `webapp_shutdown_timeout` | 30 s | |
| `webapp_session_max` | 10000 | |
| `entity_rest_max_page_size` | 1000 | |
| `script_timeout` | unset | An explicit script limit for all threads; 0 means none |

### Not done, or left as noted
- **Java-parity design points:**
  - REST `pageNoLimit` bypasses the page cap.
  - JSON-list REST calls commit item by item.
  - The implicit `actions` transition returns the whole context.
  - Mail attachments can name any locatable file.
- **Lows:**
  - `service/mail.rs` still uses `Result<_, String>` (49 sites).
  - `moqui-lib` `regex_match` is still polynomial on nested quantifiers.
  - Static files are read whole, with no Range support.
  - Search mapping caches can go stale across cluster nodes.
  - The login lockout gate is checked before the failure increment.
  - `remember_client_ip` updates the Visit on every request.
  - HttpSession rows are never purged when `webapp_session_timeout` ≤ 0.
- **Global state that stays process-wide:** the notify bus and its origin id, the transaction deadline slots and lock tracking, the QZ signer, and `LAST_BOOTED`.
- **Not verified live:**
  - `catch_unwind` in the wasm32-emscripten build
  - the pgvector embedding SQL, whose test needs `PGVECTOR_URL`
  - MySQL and MariaDB integration, which needs a server
- **Known limits:**
  - On MySQL (not MariaDB), the statement timeout bounds only SELECT.
  - The entity JSON loader relies on serde_json's `arbitrary_precision` number key, which a test covers.
  - `cargo clippy -p moqui-host --no-default-features` already failed at baseline, and that configuration is not linted.
- **Test isolation:** `screen/cache.rs` and `script/runtime/facades.rs` still read `instance_purpose` directly.
