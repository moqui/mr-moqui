# Transaction cache

Archived 2026-11-08: its live status, what remains and the ideas are in [Profiling2](../Profiling2.md); this file is the record.

Status 2026-11-06: Phases 0 to 6 built, 45 application services on the cache (Round 23), view reads shadowed instead of flushed (Round 24), distinct reads and the shared payment and shipment services (Round 25), 13 more with the Mantle operations timed on an external Postgres (Round 26), and shadows of up to 10,000 rows as arrays (Round 27), Phase 6 as a first profiling pass: the module move, D1's instruments, strict and census modes, the round-trip count, the authorization reads, `tx_cache_speed`, G1 to G8, D6's commit batch, D2 to D5's reads, S1 to S5 on the cache, a first pass of F1 to F5, and the guide ([Services](../../docs/logic/Services.md#transaction-cache)). [Results](#results) has the censuses and the numbers; [Profiling, Rounds 14 to 27](Profiling.md#round-27-large-shadows-as-arrays-a-profile-of-the-mantle-operations-and-the-script-engines-compile-and-heap-check) the profiles and the tuning. Each transaction is an `AT_TRANSACTION` telemetry span with the cache's counts ([Monitoring](../../docs/tools/Monitoring.md)).

The transaction cache holds a transaction's writes in memory and writes them all at commit; while it holds them, the transaction's reads see them. Done well, it gives a large speedup for high-volume transactional work and a large cut in database load, without refactoring reusable services and without callers keeping records in hand to avoid re-reading them: a service that reads `OrderItem` again, as each of several services in one operation does, gets it from memory. Moqui Java got real gains from this in a few places. Here the framework is ours to change and services can be reviewed in bulk, so the aim is more: make the cache fast and correct enough that a common transactional service runs from its first write to its commit without touching the database for anything it has already read or written, and its writes go out in one batch.

This first pass makes the cache itself correct and fast, then applies it to five frequently called framework services and five pieces of framework logic outside services. mr-mantle comes in the next round, with the tools this pass builds for finding where the cache flushes early.

## Goal and measures

For each selected operation, with the cache on:
- **No early flush.** Neither an in-place flush (held rows written before commit while the frame goes on holding, as a view read does now) nor the switch to read-through (Java's `makeReadOnly`, our `flush_and_disable`).
- **One commit batch.** Every write reaches the database at commit, pipelined and several rows to a statement where the driver can.
- **A record read once.** A row the transaction has read or written is not read from the database again in that transaction, except by a `for-update` find, which always goes to the database for its lock.

Measured per operation, cache off against cache on: waited round trips, statements, microseconds per call (embedded Postgres, and an external Postgres with 200 µs of added delay, as in [Profiling](Profiling.md#how-to-measure)), allocations, and early flushes by cause. The correctness gate: the same rows after commit, the same results, and the same ECA, audit and data feed effects, with the cache on and off.

Not in this pass: mr-mantle services, the cache on by default, MySQL multi-row statements.

## Terms

- **Frame**: one transaction level ([`transaction/manager.rs`](../../crates/moqui-entity/src/transaction/manager.rs)); a `force-new` call runs in its own frame, and a suspended frame's held rows are not seen by the frame above it.
- **Hold mode**: writes are held in memory (`transaction="cache"` or `force-cache`, `useTransactionCache(true)`).
- **Read-through mode**: writes go to the database at once, and reads are still kept and answered by the frame (Java's read-only cache). Rust has no such mode yet: `flush_and_disable` turns the cache off.
- **Early flush**: any write of held rows before the commit. The **commit batch** is the flush at commit.

## How it works today

**Rust** ([`entity/cache.rs`](../../crates/moqui-entity/src/entity/cache.rs), [`entity/cache/hold.rs`](../../crates/moqui-entity/src/entity/cache/hold.rs)):
- `gate_write` (`cache.rs:1031`) holds each create, update, delete and upsert in the frame's `FrameHold`: kept in write order, indexed by entity and key (HL-034). A create then a delete of the same row collapse to nothing; nothing else is merged.
- A find of a non-view entity always queries the database; `overlay_rows` (`cache.rs:1843`) applies the held writes to the rows it reads. A find by the whole key with a held write still queries (`cache.rs:1872-1894`), even for a row the frame created. The only rows kept are the stored rows under held updates (`stored_rows`).
- A find or count of a view first writes every held row if the frame holds rows of a member entity (`flush_for_view`, `cache.rs:2008`), then goes on holding.
- `no-tx-cache="true"` on a service writes the held rows and turns holding off for the rest of the frame (`service/call.rs:577`, `flush_and_disable`).
- At commit, `try_commit` (`manager.rs:1485`) runs the cache flush, then the data feed insert, then `COMMIT`. `flush_held` (`cache.rs:1656`) sends one `conn.execute` per held write and waits for each. So today a held frame sends as many statements as an unheld one, waits as often, and pays for the overlay on top.
- A `cache` call that joins its caller's frame turns holding on for the rest of that frame (`manager.rs:1123`), as Java's `initTransactionCache` does.
- `use-transaction-cache` in the conf (default on) can turn the cache off everywhere.

**Java** (moqui-framework `org.moqui.impl.context.TransactionCache`) had what Rust lacks:
- `readOneCache`, the rows read by key, for the life of the transaction; a find by key answered from memory.
- `readListCache`, list results by condition, kept current by each write (`mapMatches`).
- `knownLocked`: a `for-update` find of a row the transaction created or already locked runs no query.
- `listGet`'s shortcut: a list find whose condition pins a type-one relationship to a row created in this transaction (the `OrderItem`s of a new `OrderHeader`) is answered from the held creates, with no query.
- `checkUpdateValue`: a row read from the database that the transaction holds as a create fails at once, not at commit.
- `makeReadOnly` writes the held rows and keeps the read caches.

Java read views without the held rows (stale); Rust's flush is the safer choice and stays, made rarer (D5).

**Who uses it.** No framework service. In mr-mantle, 3 services are `transaction="cache"` (`ShipmentServices.xml:526, 690, 1719`), 19 are `no-tx-cache` (mostly `AssetServices.xml`), and comments record where Java's cache was taken off because it gave wrong results: `OrderServices.xml:244, 316, 603, 812, 1128`, `PaymentServices.xml:801`. Those failures (stale lock reads, lists missing new rows) are what D2 to D4 address, and are the starting list for the mr-mantle round.

## Gaps to fix before the cache is turned on anywhere

| ID | Gap | Where | Fix |
|---|---|---|---|
| G1 | A `for-update`, `distinct`, `having` or select-field find skips the overlay and reads the database without the frame's held rows; for a view it also skips the flush. A count runs the overlay and then discards it. Every "lock the counter row, then read it" pattern returns the stale value once the row is held: `ChatServices.record#UserChange` fails with "No chat change counter" on a user's first change, and a second `append#Event` in one frame collides on `lastEventSeq` | `find.rs:305-307, 351-356, 592, 513-516` | D4 |
| G2 | Run-after entity ECA rules do not run for a held write; nothing runs them at the flush. Java runs them. `Email.eecas.xml:22, 53, 67` (`note#MessageChange`) would be lost | `value.rs:1366-1368` (the rules run only in the other branch, `1511-1520`) | Run the after rules (and read `get-original-value` / `get-entire-entity` rows through the overlay) for held writes, as for direct ones |
| G3 | A held update's data feed touch carries no old row, so the update is fed even when no document field changed | `value.rs:1367` | Pass the stored row the update read (or the overlay's) |
| G4 | A held update of a row that is not there raises no error; the flush ignores row counts | `flush_held` | Check at hold time from the overlay or stored row, which the read cache makes free; failing that, check the count in the commit batch (`execute_deferred` with `zero_rows`) |
| G5 | A held create of a row the database has fails only at commit, and the error does not say which write or service it came from | `hold.rs:72-76`, `flush_held` | As Java's `checkUpdateValue`: a find that loads a row held as a create fails then. Flush errors name the entity, the key and the service that held the write (kept on `Held`) |
| G6 | An audited update reads its stored row twice under hold: `prepare_handled` calls `before_write` without the row that `write_update` already read | `cache.rs:2265`, `field_rw.rs:334`, `value.rs:939-950` | Pass the stored row through the gate |
| G7 | A find by key of a row the frame created still queries | `cache.rs:1872-1894` | D2 |
| G8 | An insert used as a lock (a claim row, or a counter row created so that a second request waits on it) does not reach the database until commit, so a concurrent request no longer waits: it fails at its own commit on the key, or an upsert overwrites. `ClientSyncServices.xml:53` (the claim, made before the cache is on, so fine), `ChatServices.xml:629-638` (`ChatUserState`) | | By design: the locks a write takes come at commit (D4). The `ChatUserState` case is a change to S2 and S3 |
| G9 | A `cache` service joined by a caller turns holding on for the rest of the caller's frame | `manager.rs:1123` | Kept, as Java does. Mark the service an operation starts with, not an inner service many callers share; written down in the guide |

## Design

### D1. Frame modes and instruments

- Three modes per frame: hold, read-through, off. `flush_and_disable` (and `no-tx-cache`) switches to read-through and keeps the read cache; writes in read-through go to the database and update the read cache.
- Per-frame counts: writes held by operation, reads answered from memory, database reads, early flushes by cause (view, selective find, raw SQL, bulk statement, `no-tx-cache`, size cap) with the entity and the service on the artifact stack, and the commit batch (statements, rows, waited round trips).
- A debug log line per early flush, naming the cause, entity and service; the counts as attributes on the transaction's telemetry span.
- **Strict mode** (`moqui.tx_cache.strict`, tests only): an early flush in a hold frame is an error, so a test can assert that a service commits in one batch.
- **Census mode** (`moqui.tx_cache.everywhere`, dev and test only): every service transaction holds. Run over the framework regression suite and the example specs, it lists each service that flushes early (by cause) or fails under the cache. It finds the next candidates, mr-mantle's included, from data rather than reading.
- A waited round-trip count in the driver (the wire client knows when it waits), beside `entity::statement_stats`, so a test can count round trips without a delay.

### D2. Read cache

Java's `readOneCache`, per frame, in hold and read-through modes:
- A whole row read by key, or loaded by a list find, is kept (plaintext, as the overlay keeps it). A later find by the whole key is answered from it with the held writes applied, with no query; a select-field find by key is the projection of the kept row.
- A row the frame created, or a held update whose stored row is known, is answered with no query (G7).
- Each write of the frame goes through the gate, so the kept rows stay current. Writes of other transactions are not seen once a row is kept; Java's cache does the same. A transaction in hold mode reads between repeatable read (a row it has read stays as it read it) and the database's read committed (a row it has not read yet is as last committed), and keeps the database's locks for `for-update` finds (D4). That is usually what a service wants, and cheaper; a service that must see another transaction's commit mid-way runs without the cache.
- A cap per frame (rows and bytes, about 10,000 rows to start); past it, nothing more is kept. Reaching the cap is not a flush.
- Not for views, and not for finds with `useClone`.

### D3. List cache and lists of new rows

- Java's `readListCache`: a list find with no limit or offset is kept per frame under the canonical find key (`with_lookup_key`), and kept current on each write with the existing `judge` and `may_match` (a write whose effect cannot be judged drops that entity's lists). Start with conditions that are only equalities (the foreign key lookups services repeat); widen once the census shows which others repeat.
- Java's lists of new rows: a list find whose condition pins every key field of a type-one relationship to a row the frame created is answered from the frame's held creates, with no query.

### D4. Locking and selective finds under hold

- **Explicit locks are taken at once.** A `for-update` find always goes to the database (`FOR UPDATE`), whatever the frame holds or has read; Java's `knownLocked` shortcut is not kept. The held writes are then applied to what it read, as any find's are (G1), and the rows it read are kept (D2).
- **The locks a write takes come at commit.** A held create, update or delete locks its row only when the commit batch writes it. Nothing is written early to take a lock sooner. A service or framework operation that turns the cache on accounts for this, which is usually what it wants: its locks are held for the shortest time. One that needs a lock mid-way takes it with a `for-update` find, or runs without the cache (`no-tx-cache`, or not marked `cache`); a very large operation may be one of those. The guide says so (Phase 6).
- **Selective finds.** A select-field find of an entity with held rows reads whole rows through the overlay and returns the fields asked for (with `for-update`, it locks the whole rows). A `distinct`, `having` or aggregate find of an entity with held rows writes the prefix of the hold that holds that entity's rows first (an early flush, cause "selective find"); the census shows whether any selected service needs more than that.

### D5. Views without a flush

A view read flushes today when the frame holds rows of any member. Flush only when a held member row may match the view's condition: map the view's condition to member fields through its aliases and ask `may_match` of each held row. A view a service reads of rows it just wrote still flushes; for those, the service reads the member entity (D7's census names them).

### D6. The commit batch

- `flush_held` sends deferred writes: creates and upserts by `execute_deferred_row` (the driver joins consecutive one-row inserts of the same SQL into multi-row inserts, `postgres.rs` `RowBatch`), updates and deletes by `execute_deferred` (updates with the `zero_rows` check, G4), then `finish_deferred`, so a failure is the flush's and not the feed hook's. With the feed insert batched too (F5), a commit waits about once for the batch and once for `COMMIT`. Since Profiling Round 17 the commit's batch is not waited for on its own: the `COMMIT` goes out behind it when no update in it carries the check (an update of a row the frame locked carries none), and the feed insert waits for it first only when it has statements of its own.
- Merge the writes of one row before sending: a create and its later updates become one insert of the final values; updates become one update of the union of their columns; a delete after updates is the delete. ECA rules and audit rows already ran per write.
- Same SQL for runs of one entity: a create writes every column of the entity (null where unset) when no column has a database default, so creates with different unset fields share one statement.
- Keep the held order (a parent before its child). Group creates of one entity across other writes only where no foreign key between the entities in the batch forbids it; measure first, since the gain depends on how interleaved real batches are.
- A size cap (writes and bytes held per frame) writes the hold and goes on holding (cause "size"), so a long batch job does not grow without bound.
- MySQL and SQLite keep the trait's defaults: the same statements as today, sent one at a time. Multi-row SQL for MySQL is later.

## Candidates

### Five services

All five are in the framework, run often, and write several rows per call. "Reads again" counts finds of a row the call has already read or written.

| # | Service | Called | Reads again | Under hold today | To make it run in one batch |
|---|---|---|---|---|---|
| S1 | `ChatServices.heartbeat#UserStatus` (`ChatServices.xml:4197`) | each open page, once a minute, and on return from idle; each XMPP session. The framework's most frequent write | `UserStatus` 4-5 times, `UserPreference` twice | Runs to commit: its locking finds come before its writes | `publish#UserStatus` returns `myStatus`, dropping the second `get#UserStatuses` (`:4232`); the row passed down, not read again at `:4110`. `set#UserStatus` and `disconnect#UserStatus` have the same shape |
| S2 | `ChatServices.mark#Read` (`:2967`) | each room opened, or scrolled to its end; XMPP display markers | the same `ChatRoomMember` twice (`:204`, `:2984`) | Fails on a user's first change (G1 in `record#UserChange`) | G1 and D4. `record#UserChange` makes a missing `ChatUserState` in a `force-new` call (create if missing; one row per user, ever), then locks with its `for-update` find and uses those rows, so two first changes of one user still wait on each other (G8). The member row taken from `get#RoomAccess` |
| S3 | `ChatServices.send#Message` (`:2141`) | each chat message: composer, XMPP, bridges, agents, polls | `ChatRoom` 2-3 times, `ChatRoomMember` 3, `get#LocalActors` 2-3 | Wrong with two appends in one frame (lock read of `lastEventSeq`), and its member list by selected fields misses members made in the frame | G1 and D4; the `record#UserChange` change of S2; the room locked once and `lastEventSeq` passed down; room, members and actor passed into `append#Event`; the same in `record#RoomChange`. These fixes also serve `edit#`, `react#` and `redact#Message`, `add#Member` and the bridges |
| S4 | `WikiServices.update#WikiPage` (`WikiServices.xml:570`) | each wiki save and autosave, collaborative saves | `WikiPage` 4-5 times, `WikiSpace` 3-4 | Expected to run to commit for `dbresource://` spaces; `git://` spaces not checked (`moqui-git/src/db.rs` has its own transactions, and a `force-new` in `service/hosting.rs:1402`) | Page, space and reference passed to `get#WikiPageAccess` and `index#WikiPageLinks`; check `git://` |
| S5 | `EmailClientServices.set#MessageFlags` (`EmailClientServices.xml:2204`) | each message opened, flagged or swiped | `EmailMessage` 3 times per message; the account rows 2-3 times; `note#MessageChange`'s 4 reads per message | No flush, but `note#MessageChange` never runs (G2), so the change feeds miss the change | G2; `note#MessageChange` reads the whole message (`:2625` reads selected fields and misses one made in the frame) or takes its keys as parameters |

If S5 waits on G2, take `ChatServices.expire#UserStatuses` (`:4264`, every minute, safe as it is).

Not candidates: `SystemMessageServices` writes its status rows in `force-new` frames on purpose; the data feed and search services hand off to Rust. Read-only access checks called in nearly every tree (`get#RoomAccess`, `get#EmailAccountAccess`, `FileServices.get#DriveAccess`, `get#WikiPageAccess`) gain from D2 inside a held frame and need no change of their own.

### Five pieces of framework logic

| # | What | Where | Today | With the cache |
|---|---|---|---|---|
| F1 | Notification persist | `moqui-core/src/notify/message.rs:559-613` | A `force-new` frame: insert `NotificationMessage`, one insert of `NotificationMessageUser` per user, commit: N+2 round trips. Each emailed user's row is read back and updated (`:786-812`) | A `force-cache` frame and D6: about 2-3 round trips; the email message id set on the held row, not read back |
| F2 | `ServiceJob` scheduling and runs | `moqui-core/src/service/jobs.rs` | Each pass (every 60 s): the jobs, then one `ServiceJobRunLock` read per scheduled job (`readiness`, `:401`), and the latest run of each due one. Each run, about 13 round trips over 5 small transactions and reserved connections: `create_run` (`:1143`), the lock claim (`LockPlan::claim`, a conditional write), `job_parameters`, `holds_lock`, the start `record` (`:815`); at the end `release_lock`, then the end `record` (`:971`) or `delete_run`, which reads before it deletes (`:1170`); a queued call also deletes its job, parameters and lock, each row read first (`delete_queued`, `:1120`) | The claim stays its own committed write: it is a lock other instances wait on (D4). The pass reads every lock in one query. The end of a run is one hold frame: the release (a conditional statement, sent deferred), the end record or the delete by key with no read first, and a queued call's deletions as bulk deletes by job name, in one commit batch. The start record and `holds_lock` read go together in one transaction |
| F3 | Session start and login | `moqui-core/src/user/session.rs:1620-1779, 709-741, 1557`; `user/login.rs:1281-1319, 1426-1445` | Outside any transaction, so each statement commits on its own: `Visitor` and `Visit` inserts, the `Visit` read back after its insert, one uncached `UserGroup` read per group for the IP check, `UserAccount` read again before its update, `Visit` updated with the user just after it was created with it. For stateless Basic and API-key clients this is every request | One hold frame around the session start and the login: one commit in place of 4-8, the redundant reads and the second `Visit` write gone |
| F4 | Entity audit log | `moqui-entity/src/entity/field_rw.rs:307-361, 429-480` | One `create#moqui.entity.EntityAuditLog` service call (service machinery, a sequenced id, an insert) per changed audited field: Mantle audits `statusId` fields widely, `ProductPrice` and `ProductCost` on create | In a hold frame the rows are held and go out in the commit batch as multi-row inserts. Outside one, write the rows as deferred rows when the service has no SECA rules, and read the stored row once (G6) |
| F5 | Data feed insert at commit | `moqui-entity/src/entity/load_feed/feed.rs:240-393, 1049-1074` | Runs after the cache flush on every commit that touched a feed document. Each statement is in its own savepoint (savepoint, statement, release: 3 waited round trips); each touch reads its row again and walks to the primary entity one step and one touch at a time; one insert per document. A 10-item order is about 60 round trips | Part of the commit batch: one savepoint around the insert, touches deduplicated by entity and key, one read per entity for the rows a touch lacks, one walk per path for all rows, one multi-row insert: about 6. The touch of a held update carries its old row (G3) |

Next in line: agent run events (`moqui-llm/src/run/events.rs:372-399`: every 300 ms or 512 characters while a run streams, the `AgentRun` lock, one insert per event, the `AgentRun` update and a commit, N+3 round trips; hold mode and D6 make it about 3, but it is busy only while an agent streams), the `HttpSession` flush (a transaction per dirty session each minute, `user/session.rs:1881-2048`), and the feed receiver's delete per pending row (`moqui-core/src/search/feed.rs:480-496`).

### Second set (Profiling Round 19)

A survey of the framework for frequent transactions with repeated reads or several writes, beyond the first set. The
first three are on the cache and timed (`tx_cache_speed` N1 to N3); the rest are ranked by frequency times statements
saved, not yet changed. Counts are from reading the code unless timed.

| # | Where | How often | What repeats | State |
|---|---|---|---|---|
| N1 | Storing a received message: `moqui-mail` `store::store_message` from IMAP and POP3 sync, SMTP/LMTP delivery and import | each message received; thousands on a first sync | `note#MessageChange` (Email.eecas.xml) for the message and each attachment and label reads the account, its users and their cursors each time; attachments write DbResource rows | `force_cache` on the four store paths and `apply_flags`; select-field finds kept in the frame (D3, Round 19). 51 to 23 statements a message with two attachments, 15.8 to 10.2 ms with the delay |
| N2 | `react#Message`, `edit#Message`, `redact#Message`, `vote#Poll` (ChatServices.xml) | each reaction, edit, vote | as S3: the room twice, access, the event, `append#Event` | `transaction="cache"`; `redact#Message` removes from the index after commit. `react#Message` 21 to 15 statements |
| N3 | `mark#NotificationsViewed` (NotificationServices.xml); `mark#AllRead` (ChatServices.xml) has the same shape | each opening of the notification panel, each click | N updates by key after one read | `transaction="cache"`: 6 to 1 statement for five |
| 4 | Agent step bookkeeping, `moqui-llm/src/run/step.rs` `store_reply`, `store_result_with`, `set_call_status` | each model step and tool call | the `AgentRun` lock twice, the conversation lock, step, call, message and part inserts, about 13 round trips for a reply with two calls | `run::in_cache_tx` for a reply, a tool result, a call's status and the active time; the reply reuses the locked run row (Round 20): a run of two steps and four calls 300 to 257 waited round trips |
| 5 | IMAP flag sync `apply_flags` (`moqui-mail/src/sync.rs`) | each sync pass with changes | N updates, each with the rule's original-value read and `note#MessageChange` | on the cache with N1; the rows that change read in one `in` find first (Round 21) |
| 6 | `save#Draft` (EmailClientServices.xml) | each compose save and autosave | the draft created then updated again, attachments deleted and made again then listed | `transaction="cache"` (Round 21): an autosave 15 to 9 statements (`tx_cache_speed` N4) |
| 7 | `process#InboundItem` (ChatBridgeServices.xml) | each bridged event | 8-10 reads, the room read twice, then `send#Message` joins | `transaction="cache"` (Round 21) |
| 8 | `send#WebPush` (NotificationServices.xml, `service/web_push.rs`) | each persisted notification for users with a subscription | one update per subscription, and a transaction open across the push HTTP calls | `transaction="ignore"`, the outcomes written after the pushes in one cache frame (Round 20) |
| 9 | `expire#UserStatuses` (ChatServices.xml) | each minute, per online user | `publish#UserStatus` reads `UserStatus` again after the list | one batch, split on a failure down to the failing user (Round 22) |
| 10 | `run#EmailRules` | each delivered Inbox message | the stored message read again, account configuration | in the store's frame for delivered mail, the message stored alone when that fails (Round 22) |
| 11 | Alert rule finish, `moqui-report/src/alert.rs` | each rule each minute | K state rows deleted and written | not changed |

Not candidates, checked: artifact hits (telemetry, not rows), `UserLoginHistory` (async, once a minute a user), LLM
quota admission (lock rows and aggregate counts), collab document writes (one lock, one write), the edge queue (claims
as locks), search index writes (raw SQL), the per-minute mail and git jobs (usually nothing to do).

### Found along the way, not the transaction cache

- **Authorization and tarpit checks are read with no cache.** `load_group_checks` and `load_tarpit_checks` (`moqui-core/src/user/authz.rs:896-961`) find `ArtifactAuthzCheckView` and `ArtifactTarpitCheckView` per group without `use_cache`; Java reads both with `useCache(true)` (`UserFacadeImpl.groovy:884, 901`). With the default tarpit on every screen, that is 13-16 queries per request. Taken in Phase 0: read both through the cache (a member write already evicts a view's cached finds through `views_by_member`), and keep `ArtifactTarpitLock` in memory with its invalidation (add it to `is_authz_entity`, `authz.rs:74-84`, so a cluster peer's change reaches it).
- **Theme preferences**: 13 keys read one at a time, two queries each, per appearance build (`moqui-core/src/screen/sri/appearance.rs:463-469`); read them with `get_preferences()`.
- **ETL loader**: no pipelined writes inside a batch (`entity/etl.rs:169-262`), as the data loader has. Not with the cache: under hold every row is read first.

## Phases

Each phase ends with its regression tests passing and `./moq test-framework` before its commit.

0. **Preparation.**
   - Move the transaction cache code (hold, overlay, stored rows, flush) out of `cache.rs` (2,921 lines) into `entity/cache/tx.rs` beside `hold.rs`, in its own commit, as [Practices-Rust](../../docs/dev/Practices-Rust.md) asks of a large file that gets real work.
   - D1's counts, strict mode, census mode and the round-trip count.
   - A new opt-in speed test, `tx_cache_speed`, with a row for each of the ten candidates, cache off and (where it runs) on. Record the baseline.
   - The authorization read fix.
   - A first census run, recorded here.
1. **Correctness: G1 to G9.** A unit test per gap that fails before its fix.
2. **The commit batch: D6.** Speed test rows before and after; the external-Postgres run with delay.
3. **Reads: D2, D3, D4, D5.** Unit tests counting statements, in the style of `held_updates_read_their_stored_rows_once_per_frame`.
4. **The five services.** Turn the cache on for each (S1 to S5), run its tests in strict mode, and change it until it commits in one batch with results the same as cache off.
5. **The five pieces of framework logic** (F1 to F5).
6. **Profile and tune.** samply and heaptrack on the speed test rows ([Profiling](Profiling.md#how-to-measure)): the overlay, the read cache lookups, the batch's statement building. Record the numbers as a new round in Profiling and a line in [Performance-Summary](../Performance-Summary.md). Write a "Transaction cache" section in [docs/logic/Services.md](../../docs/logic/Services.md): when to mark a service `cache` (the operation's first service, not a shared inner one), what reads see (the [decisions](#decisions)), that the locks writes take come at commit while `for-update` locks at once, when to run without the cache instead (an interim lock from a write, a very large operation), what makes it flush, and strict and census modes.

## Results

### Census 1 (Phase 0)

Census mode (`MOQUI_TX_CACHE_EVERYWHERE=true`, every frame a `run` begins or joins holds) over the specs of `basic-comms`, `framework`, `tools`, `webroot`, `dev-aide` and `example` (`./moq test`), before any fix: 393 features, 235 passed, 33 failed, 125 skipped after a failure in a stepwise spec, and 2 spec setups failed (`ChatKeys`, `ChatKeyBackup`).

- **Chat and everything over it** (the Chat, Calls, Voice, XMPP and basic-comms specs): `No chat change counter for user ...` (`record#UserChange`'s lock-then-read of `ChatUserState`) and `Chat room ... not found` (a `for-update` or select-field find of a room the frame created). Both are G1.
- **Commit flush failures**: `DatabaseServices.save#DataGridChanges` (a held create of an `Enumeration` the database has fails only at commit, so the rows saved before it are not rolled back one by one as the spec expects: G5), and one with no service on the stack, an insert of `WorkEffortParty` whose `WorkEffort` was not there (a data load).
- **Early flushes, all of views** (in Mantle services run by the demo data load): `ProductServices.create#VariantProduct` 12 (`ProductAssocAndToFeatureAppl`), `TimeServices.create#TimePeriod` 3 (`PartyDetail`), and one each in `CalendarServices.store#CalendarFeedEvents`, `LedgerServices.get#OrganizationFiscalTimePeriods` and `CommunicationServices.create#EmailCommunicationEvent`.

### Census 5 (mr-boss and mr-mantle)

Census mode over the specs of mr-boss and mr-mantle (Profiling Round 22): 3,058 features, 2,822 passed and 179 failed
at first. Five faults of the frame's reads, not of the services, explained nearly all of it: a created row without its
unset fields, condition text values not at their fields' types, numbers of different kinds not compared, encrypted
fields' conditions not encrypted in the frame's reads, and reads kept past a nested frame's commit on the same thread
(with a view of views missing its base entities' held rows). Fixed, 3,062 of 3,067 pass; the five left pass run alone
but four, and WorkCenters' load count fails. The early flushes (4,396 in the run) are view reads of rows the service
wrote (`TrackingServices.refresh#Mode` 1,320, `TaskServices.update#ProjectTimeFromTask` 923, `update#TaskFromTime`,
`DuplicateServices.find#PartyDuplicates`, `OrderServices.handle#OrderMajorChange`, `ShipmentServices.add#OrderPartToShipment`),
the `AssetServices` reservations marked `no-tx-cache` (742), and `pack#Shipment`'s selective find; those services read
the member entity instead, or stay off the cache, before it is turned on for them.

### Census 6 (every component, measured)

Every component's specs under the census (Profiling Round 23), with a `frame` line per transaction: 108,019
transactions, 3,659 of 3,665 features passing, and none of the six failures the cache's (see Round 23). The ranking
of services by round trips saved picked the 45 now marked `cache`; across them the specs' waited round trips fell 34%.

### Census 7 (view shadows)

Every component's specs under the census after view reads were shadowed (Profiling Round 24): 149,966 transactions,
913 early flushes against 4,482 in Census 6 (view 3,652 to 34; `no-tx-cache` 788, all Mantle's asset services, since
taken off; selective finds 66; size 25), 4,779 view reads shadowed, the same features passing.

### Census 8 (distinct reads shadowed)

mr-mantle, mr-boss and mr-pass under the census after Round 25: 145,367 transactions, 19 early flushes (15 of a bulk
frame past the 500 held rows a shadow takes, 4 of a data document), 5,697 reads shadowed.

### Census 9 (every component, after Commerce and Shop)

Every component's specs under the census (Profiling Round 26): 152,290 transactions, 42 early flushes (27 data loads
past the size cap, 15 of one bulk frame past the shadow's 500 rows), 5,706 reads shadowed, no commit batch failing.
Its ranking picked the 13 services of Round 26.

### Census 2 (Phase 1)

The same run after G1 to G6: 393 features, 389 passed, 4 failed, no spec setup failed. The chat failures were G1 and, under it, G4's quiet update: a `createOrUpdate` of a missing `ChatUserState` sent its update probe, which a held write answered as done, so the row was never made. Left:

- `DataGrid` (`save#DataGridChanges`): the held create of an `Enumeration` the database has fails at the commit, naming itself (`Could not write the create of moqui.basic.Enumeration [enumId=GS_B] held by create#moqui.basic.Enumeration`), not row by row as the spec expects. By design (G5 checks only rows a find loads); a service that saves rows one by one and reports each refusal runs without the cache.
- `Units` (`UnitServices.store#UnitConversion`): `Conversion changed; reload before saving`.
- `BasicCommsEncrypted`, a poll in a plain room: `cannot read property 'contentJson' of null`.
- `Chat` search: a member finds nothing (the search reads what was indexed).

These are census findings for the next round, not candidates of this one.

### Baseline (Phase 0)

`tx_cache_speed` (`crates/moqui-host/tests/tx_cache_speed.rs`), release, embedded Postgres over a Unix socket, 200 calls x 5 batches, one transaction a call. `sql` is statements per call, `rt` waited round trips per call (begin and commit among them). The second table is the transaction cache's own counts per call of the "on" rows. F2 runs on the job workers and is not timed here; F1 and F3 have no "on" row until Phase 5.

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200     62.437    312.18       3203    1416.0     240784      9478   6.32   7.32   58.953-68.873  
  S3 send#Message, cache off                         200    220.110   1100.55        909    4387.1     573201     23306  13.00  14.00  206.356-240.316 
  S2 mark#Read, cache off                            200     42.976    214.88       4654    1052.8     210520      6783   3.00   4.00   38.208-49.768  
  S4 update#WikiPage, cache off                      200   1043.396   5216.98        192   29090.4    6430497    103006 116.00 117.00  671.923-1415.849
  S5 set#MessageFlags, cache off                     200    234.737   1173.69        852    5363.9    1982923     20836  15.99  21.00  218.151-348.654 
  F1 notification persist, 3 users, cache off        200     30.627    153.13       6530     653.7      66536       261   6.02   8.04   26.833-31.025  
  F4 audited update UserAccount.username, cache off     200     24.755    123.78       8079     644.8     274059      1816   3.02   4.04   23.147-25.037  
  F5 fed update EmailMessage.isFlagged, cache off     200    174.212    871.06       1148    1857.1     664188      2879   8.97  13.96   85.657-312.590 
  F3 session start and password login, cache off     200   2261.358  11306.79         88     602.7   19990339      3738   6.04   8.05 2230.679-2262.019
  S1 heartbeat#UserStatus, cache on                  200     69.626    348.13       2872    1545.7     246774      9275   6.00   7.35   56.709-86.028  
  S3 send#Message, cache on                          200    214.338   1071.69        933    4492.6     583795     20925  10.00  14.01  199.635-228.953 
  S2 mark#Read, cache on                             200     40.467    202.33       4942    1053.3     210573      6783   3.00   4.00   36.592-47.793  
  S4 update#WikiPage, cache on                       200   1220.113   6100.57        164   33054.9    7418241    100202 112.00 117.00  802.206-1549.231
  S5 set#MessageFlags, cache on                      200    236.380   1181.90        846    4251.1    1634534     18594  13.99  20.00  174.542-349.577 
  F4 audited update UserAccount.username, cache on     200     30.222    151.11       6618     772.2     291265       925   2.02   5.05   28.951-35.003  
  F5 fed update EmailMessage.isFlagged, cache on     200    193.355    966.77       1034     870.1     335624      1284   6.99  12.98   70.316-314.949 
  transaction cache per call                      frames    held   mem rd    db rd    early batch st batch rt
  F4 audited update UserAccount.username, cache on    1.00    2.00     0.00     2.00     0.00     2.00     2.00
  F5 fed update EmailMessage.isFlagged, cache on    1.00    1.00     0.00     2.00     0.00     1.00     1.00
  S1 heartbeat#UserStatus, cache on                 1.00    0.34     0.00     6.00     0.00     0.34     0.34
  S2 mark#Read, cache on                            1.00    0.00     0.00     3.00     0.00     0.00     0.00
  S3 send#Message, cache on                         1.00    3.00     0.00     9.00     0.00     3.00     3.00
  S4 update#WikiPage, cache on                      1.00    4.00     0.00   112.00     0.00     4.00     4.00
  S5 set#MessageFlags, cache on                     1.00    1.00     0.00     9.00     0.00     1.00     1.00
```

Read with care: this machine's times swing; the counts do not. With the cache on today, a call sends as many statements as with it off (the held writes go out one at a time at commit, `batch rt` equals `batch st`), and pays the overlay on top. S4 sends 116 statements a save and S5 16, mostly reads of rows the call already read. F3's time is the argon2 password check.

### The commit batch (Phase 2)

D6 built: each row's held writes merged, sent deferred (creates as every column, so an entity's creates share one statement), one wait. Embedded Postgres, same run as the baseline:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200     68.453    342.27       2922    1421.6     241135      9506   6.35   7.36   58.588-76.494  
  S3 send#Message, cache off                         200    210.585   1052.92        950    4396.7     573845     23577  13.00  14.00  191.767-252.862 
  S2 mark#Read, cache off                            200     41.576    207.88       4810    1054.7     210558      6783   3.00   4.00   38.512-56.255  
  S4 update#WikiPage, cache off                      200   1094.382   5471.91        183   29092.2    6430543    103003 116.00 117.00  649.418-1434.187
  S5 set#MessageFlags, cache off                     200    235.799   1179.00        848    5283.8    1966625     21037  15.98  20.98  216.029-340.126 
  F1 notification persist, 3 users, cache off        200     30.323    151.62       6596     656.3      66146       -26   6.04   8.07   28.296-30.990  
  F4 audited update UserAccount.username, cache off     200     25.556    127.78       7826     644.7     274035      1816   3.02   4.04   24.194-26.900  
  F5 fed update EmailMessage.isFlagged, cache off     200    174.017    870.09       1149    1867.2     664258      2879   8.96  13.96   86.202-307.625 
  F3 session start and password login, cache off     200   2249.121  11245.60         89     603.0   19990264      3717   6.05   8.06 2219.746-2283.337
  S1 heartbeat#UserStatus, cache on                  200     72.100    360.50       2774    1558.8     247870      9275   6.00   7.37   61.145-97.521  
  S3 send#Message, cache on                          200    208.496   1042.48        959    4591.6     599169     20929  10.00  12.00  202.104-250.809 
  S2 mark#Read, cache on                             200     39.866    199.33       5017    1055.0     210570      6789   3.00   4.00   38.471-41.072  
  S4 update#WikiPage, cache on                       200   1157.530   5787.65        173   33135.3    7429128    100216 112.00 114.01  770.099-1575.478
  S5 set#MessageFlags, cache on                      200    251.470   1257.35        795    5307.2    1967507     20202  14.99  21.00  219.659-382.364 
  F4 audited update UserAccount.username, cache on     200     26.308    131.54       7602     759.6     284722       929   1.02   3.06   23.570-28.780  
  F5 fed update EmailMessage.isFlagged, cache on     200    210.718   1053.59        949    1915.1     669326      2879   7.97  13.96   89.443-342.457 
  transaction cache per call                      frames    held   mem rd    db rd    early batch st batch rt
  F4 audited update UserAccount.username, cache on    1.00    2.00     0.00     1.00     0.00     2.00     1.00
  F5 fed update EmailMessage.isFlagged, cache on    1.00    1.00     0.00     3.00     0.00     1.00     1.00
  S1 heartbeat#UserStatus, cache on                 1.00    0.37     0.00     6.00     0.00     0.37     0.38
  S2 mark#Read, cache on                            1.00    0.00     0.00     3.00     0.00     0.00     0.00
  S3 send#Message, cache on                         1.00    3.00     0.00     9.00     0.00     3.00     1.01
  S4 update#WikiPage, cache on                      1.00    4.00     0.00   112.00     0.00     4.00     1.01
  S5 set#MessageFlags, cache on                     1.00    1.00     0.00    10.00     0.00     1.00     1.00
                                                                     | : postgres connect: the database system is shutting down: db error: FATAL: the database system is shutting down
                                                                        | : postgres connect: the database system is shutting down: db error: FATAL: the database system is shutting down
```

External Postgres (`postgres:18` in docker, `tc netem delay 200us`), where a waited round trip costs about 0.3 ms:

```
database: external Postgres 172.17.0.2:5432, database speed_1431787_1793933666294584
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200    467.194   2335.97        428    1480.9     246630      9967   7.02   8.03  450.462-495.699 
  S3 send#Message, cache off                         200    983.837   4919.18        203    4397.4     573563     23345  13.00  14.00  899.221-1080.645
  S2 mark#Read, cache off                            200    282.807   1414.04        707    1055.2     210551      6783   3.00   4.00  268.353-296.107 
  S4 update#WikiPage, cache off                      200   6843.412  34217.06         29   29088.5    6430393    103003 116.00 117.00 6193.866-9674.747
  S5 set#MessageFlags, cache off                     200   1008.869   5044.35        198    5378.4    1983163     20842  15.98  20.98  945.252-1045.636
  F1 notification persist, 3 users, cache off        200    430.336   2151.68        465     655.9      66059        43   6.04   8.06  412.404-474.090 
  F4 audited update UserAccount.username, cache off     200    250.973   1254.87        797     645.0     274060      1842   3.02   4.04  238.023-294.990 
  F5 fed update EmailMessage.isFlagged, cache off     200    510.792   2553.96        392    1863.1     664189      2879   8.97  13.97  484.461-1979.463
  F3 session start and password login, cache off     200   2728.953  13644.76         73     602.7   19990154      3615   6.04   8.04 2692.070-3092.982
  S1 heartbeat#UserStatus, cache on                  200    460.231   2301.15        435    1852.4     264611      9321   6.00   8.01  453.785-471.572 
  S3 send#Message, cache on                          200    848.218   4241.09        236    4593.3     599400     21066  10.00  12.02  822.449-896.153 
  S2 mark#Read, cache on                             200    282.237   1411.18        709    1056.2     210641      6789   3.00   4.00  267.186-292.460 
  S4 update#WikiPage, cache on                       200   6831.868  34159.34         29   33139.2    7429388    100320 112.00 114.02 6249.937-7695.375
  S5 set#MessageFlags, cache on                      200    987.036   4935.18        203    5314.3    1967408     20189  14.98  20.98  949.195-1028.794
  F4 audited update UserAccount.username, cache on     200    194.971    974.86       1026     759.7     284695       933   1.04   3.08  187.539-211.547 
  F5 fed update EmailMessage.isFlagged, cache on     200    501.739   2508.70        399    1911.1     669250      2879   7.98  13.98  473.855-519.366 
  F4 audited update UserAccount.username, cache on    1.00    2.00     0.00     1.00     0.00     2.00     1.00
  F5 fed update EmailMessage.isFlagged, cache on    1.00    1.00     0.00     3.00     0.00     1.00     1.00
  S1 heartbeat#UserStatus, cache on                 1.00    1.00     0.00     6.00     0.00     1.00     1.00
  S2 mark#Read, cache on                            1.00    0.00     0.00     3.00     0.00     0.00     0.00
  S3 send#Message, cache on                         1.00    3.00     0.00     9.00     0.00     3.00     1.00
  S4 update#WikiPage, cache on                      1.00    4.00     0.00   112.00     0.00     4.00     1.01
  S5 set#MessageFlags, cache on                     1.00    1.00     0.00    10.00     0.00     1.00     1.00
```

The batch waits once (`batch rt` 1.00 where a frame holds several writes): S3 goes from 14 to 12 waited round trips a call and from 4.92 to 4.24 ms with the delay; F4 from 4 to 3 and 1.25 to 0.97 ms. The rest of each call is its reads (`db rd`), which Phase 3 takes on. The size cap is 10,000 held writes (a byte cap is not built). Creates of one entity are not regrouped across other writes yet.

### Reads (Phase 3)

D2 to D5 built. A frame with the cache on (hold or read-through) keeps each row it reads as the database has it, and each list find with no page by the find's key, and applies its held writes on top when it reads; while it holds its writes the database does not change under it, so nothing it kept needs updating write by write. An early flush, a write in read-through mode, or raw SQL drops what it read (of the entity, or all). A find by the whole key (only the key: a condition with more goes to the database, since a value of another type compares there as it would not in memory) is answered from the kept row, or for a row the frame created from its held writes alone (G7); a list find pinned to a row the frame created by a type-one relationship from the held creates alone; a find the shared entity cache answers is left to it. A view read flushes only when a held member row may be in its result (D5), judged on the row before and after the write through the view's aliases; a row whose stored values the frame does not know may be.

D6 changed with it: merging a row's writes moved an update past another row's write that depended on it, and the census found one (`delete#LocalMessages` clears a thread's references, then deletes the messages: the update of a reply folded into its delete, so its parent went first and the foreign key refused it). A write now joins its row's previous write when they are adjacent, an update joins the row's earlier write across others only when it sets no relationship key field, and a delete is never merged.

Census 3 (the same specs as census 1): 390 of 393 pass; the three left are census 2's `DataGrid`, `Units` and `Chat` search. Twenty early flushes for size, all in the demo data load.

Embedded Postgres:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200     57.628    288.14       3471    1411.6     248778     20423   6.29   7.30   55.351-71.746  
  S3 send#Message, cache off                         200    212.722   1063.61        940    4364.2     564230     23306  13.00  14.00  191.424-254.172 
  S2 mark#Read, cache off                            200     38.546    192.73       5189    1045.0     207575      6801   3.00   4.00   37.832-49.249  
  S4 update#WikiPage, cache off                      200   1018.391   5091.96        196   29026.5    6395778    103003 116.00 117.00  626.798-1421.702
  S5 set#MessageFlags, cache off                     200    231.816   1159.08        863    5361.3    1979184     20855  15.98  20.98  204.020-348.617 
  F1 notification persist, 3 users, cache off        200     34.304    171.52       5830     648.9      64094       -53   6.04   8.06   29.049-38.176  
  F4 audited update UserAccount.username, cache off     200     25.092    125.46       7971     641.0     273060      1820   3.02   4.04   23.980-31.400  
  F5 fed update EmailMessage.isFlagged, cache off     200    176.891    884.46       1131    1856.1     662178      2879   8.97  13.96   79.099-319.217 
  F3 session start and password login, cache off     200   2222.715  11113.57         90     604.0   19989411      3780   6.04   8.06 2210.353-2231.319
  S1 heartbeat#UserStatus, cache on                  200     61.362    306.81       3259    1498.1     233097      6496   3.00   4.31   50.971-82.714  
  S3 send#Message, cache on                          200    223.097   1115.49        896    5299.8     632684     19779   9.00  11.01  204.325-250.006 
  S2 mark#Read, cache on                             200     44.566    222.83       4488    1488.6     237105      6783   3.00   4.00   40.669-61.360  
  S4 update#WikiPage, cache on                       200   1205.470   6027.35        166   48142.3   10077729     18998  15.00  17.00  577.386-1783.209
  S5 set#MessageFlags, cache on                      200    252.977   1264.89        791    6101.0    1990416     15887  12.99  19.00  218.257-372.299 
  F4 audited update UserAccount.username, cache on     200     30.320    151.60       6596     998.2     300288       925   1.02   3.05   26.884-36.517  
  F5 fed update EmailMessage.isFlagged, cache on     200    224.505   1122.53        891    2299.9     686012      2879   6.96  12.96   87.037-325.706 
  transaction cache per call                      frames    held   mem rd    db rd    early batch st batch rt
  F4 audited update UserAccount.username, cache on    1.00    2.00     0.00     1.00     0.00     2.00     1.00
  F5 fed update EmailMessage.isFlagged, cache on    1.00    1.00     1.00     2.00     0.00     1.00     1.00
  S1 heartbeat#UserStatus, cache on                 1.00    0.32     3.00     3.00     0.00     0.32     0.32
  S2 mark#Read, cache on                            1.00    0.00     0.00     3.00     0.00     0.00     0.00
  S3 send#Message, cache on                         1.00    3.00     1.00     8.00     0.00     3.00     1.01
  S4 update#WikiPage, cache on                      1.00    4.00    97.00    15.00     0.00     4.00     1.00
  S5 set#MessageFlags, cache on                     1.00    1.00     2.00     8.00     0.00     1.00     1.00
```

External Postgres with 200 us of added delay:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200    468.297   2341.49        427    1474.8     254425     20796   7.03   8.03  448.649-506.913 
  S3 send#Message, cache off                         200    947.996   4739.98        211    4367.4     564307     23328  13.00  14.00  903.768-981.539 
  S2 mark#Read, cache off                            200    265.810   1329.05        752    1045.3     207524      6783   3.00   4.00  263.856-331.293 
  S4 update#WikiPage, cache off                      200   6658.733  33293.67         30   29026.0    6395781    103034 116.00 117.00 6102.077-7174.792
  S5 set#MessageFlags, cache off                     200    972.655   4863.27        206    5360.7    1979015     20842  15.98  20.98  946.624-1014.958
  F1 notification persist, 3 users, cache off        200    422.970   2114.85        473     648.9      64006       -53   6.04   8.07  401.929-436.390 
  F4 audited update UserAccount.username, cache off     200    247.856   1239.28        807     640.6     272991      1820   3.02   4.04  228.035-260.781 
  F5 fed update EmailMessage.isFlagged, cache off     200    476.105   2380.53        420    1857.8     662139      2879   8.97  13.97  465.464-502.774 
  F3 session start and password login, cache off     200   2664.083  13320.41         75     603.7   19989235      3549   6.03   8.04 2641.862-2707.195
  S1 heartbeat#UserStatus, cache on                  200    310.062   1550.31        645    1596.5     243116      6496   3.00   5.00  305.146-322.771 
  S3 send#Message, cache on                          200    758.444   3792.22        264    5300.7     633157     20113   9.00  11.02  742.371-787.174 
  S2 mark#Read, cache on                             200    271.328   1356.64        737    1490.0     237176      6783   3.00   4.00  264.857-279.015 
  S4 update#WikiPage, cache on                       200   1994.024   9970.12        100   48141.1   10077554     18998  15.00  17.00 1504.741-2535.807
  S5 set#MessageFlags, cache on                      200    837.987   4189.93        239    6100.1    1990172     15900  12.97  18.98  821.657-846.744 
  F4 audited update UserAccount.username, cache on     200    189.825    949.13       1054     998.7     300294       919   1.04   3.08  185.673-208.212 
  F5 fed update EmailMessage.isFlagged, cache on     200    444.358   2221.79        450    2301.7     685981      2879   6.97  12.97  425.131-479.072 
  transaction cache per call                      frames    held   mem rd    db rd    early batch st batch rt
  F4 audited update UserAccount.username, cache on    1.00    2.00     0.00     1.00     0.00     2.00     1.00
  F5 fed update EmailMessage.isFlagged, cache on    1.00    1.00     1.00     2.00     0.00     1.00     1.00
  S1 heartbeat#UserStatus, cache on                 1.00    1.00     3.00     3.00     0.00     1.00     1.00
  S2 mark#Read, cache on                            1.00    0.00     0.00     3.00     0.00     0.00     0.00
  S3 send#Message, cache on                         1.00    3.00     1.00     8.00     0.00     3.00     1.00
  S4 update#WikiPage, cache on                      1.00    4.00    97.00    15.00     0.00     4.00     1.01
  S5 set#MessageFlags, cache on                     1.00    1.00     2.00     8.00     0.00     1.00     1.00
```

With the delay, cache on against off: S1 2.34 to 1.55 ms (-34%), S3 4.74 to 3.79 ms (-20%), S4 33.3 to 10.0 ms (-70%: 117 waited round trips to 17), S5 4.86 to 4.19 ms (-14%), F4 1.24 to 0.95 ms (-23%), S2 the same. Allocations are up with the cache on (S4 29k to 48k a call): the kept rows and lists are copied out for each read, a first target for Phase 6.

### The five services (Phase 4)

S1 to S5 are `transaction="cache"`. Their Rust tests (`chat_tests.rs`, `wiki_tests.rs`, `client_tests.rs`, `moqui-host` `user_status` and `lock_then_insert`) pass in strict mode, and so do the 393 features of the framework and example specs (`MOQUI_TX_CACHE_STRICT=true ./moq test`): none of the five, nor a call that joins one, flushes early. What it took:

- **G8** (`record#UserChange`): a missing `ChatUserState` is made by `ensure#UserStates` in a `force-new` call, then locked with the `for-update` find, so two first changes of one user wait on each other. A database that gives the outer transaction its snapshot (the test driver) does not show the row: then it is made in the frame, as before. `racing_first_chat_changes_of_one_user_all_succeed` (`moqui-host/tests/it/lock_then_insert.rs`, first changes in six rooms at once) fails without it on a duplicate key at the commit batch.
- **S3**: `send#Message` and `edit#Message` index the message for search once the transaction commits (`registerOnCommit`), not in it: the `MoquiChatEvent` document is read through a view built in code, which writes the held rows first (an early flush). A message that rolls back is no longer indexed.
- **Views built in code** (data documents, REST query views) now write the frame's held rows of their members first, as a view entity does; they read the database without them before.
- **In-memory conditions** compare as the database does: a number given as a float or as text equals the stored integer (`create#Poll` reads its held `ChatPoll` with `eventSeq in [...]` of JavaScript numbers).

The table's other changes (rows passed down instead of read again) are not made: those reads now come from the frame (`mem rd`), and what is left is the next round's. With 200 us of added delay:

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200    447.136   2235.68        447    1474.4     243596      9977   7.02   8.03  435.094-482.836 
  S3 send#Message, cache off                         200    983.847   4919.24        203    4338.9     561743     23610  13.99  15.00  941.861-1040.824
  S2 mark#Read, cache off                            200    262.418   1312.09        762    1045.1     207526      6801   3.00   4.00  258.737-273.477 
  S4 update#WikiPage, cache off                      200   6917.929  34589.64         29   35430.9    8157048    103003 116.00 129.19 6105.629-7213.672
  S5 set#MessageFlags, cache off                     200   1002.151   5010.75        200    5360.3    1978999     20836  15.97  20.98  925.424-1124.715
  F1 notification persist, 3 users, cache off        200    421.866   2109.33        474     649.1      64549       288   6.04   8.06  401.704-433.774 
  F4 audited update UserAccount.username, cache off     200    252.018   1260.09        794     640.6     272983      1816   3.02   4.04  234.082-258.632 
  F5 fed update EmailMessage.isFlagged, cache off     200    487.651   2438.25        410    1858.0     662148      2879   8.97  13.97  470.064-491.494 
  F3 session start and password login, cache off     200   2706.959  13534.80         74     603.5   19989284      3686   6.04   8.05 2646.991-3369.451
  S1 heartbeat#UserStatus, cache on                  200    313.768   1568.84        637    1596.4     243104      6496   3.00   5.00  311.412-343.049 
  S3 send#Message, cache on                          200    787.476   3937.38        254    5272.4     630069     20080  10.00  12.01  773.793-813.118 
  S2 mark#Read, cache on                             200    269.825   1349.13        741    1489.6     237156      6783   3.00   4.00  266.984-300.107 
  S4 update#WikiPage, cache on                       200   1981.140   9905.70        101   48140.8   10077607     19012  15.00  17.01 1483.592-2555.674
  S5 set#MessageFlags, cache on                      200    842.898   4214.49        237    6099.8    1990193     15893  12.97  18.98  835.916-848.195 
  F4 audited update UserAccount.username, cache on     200    193.646    968.23       1033     998.6     300284       929   1.04   3.07  184.243-230.927 
  F5 fed update EmailMessage.isFlagged, cache on     200    449.040   2245.20        445    2301.8     686000      2879   6.97  12.97  427.822-471.243 
  transaction cache per call                      frames    held   mem rd    db rd    early batch st batch rt
  F4 audited update UserAccount.username, cache on    1.00    2.00     0.00     1.00     0.00     2.00     1.00
  F5 fed update EmailMessage.isFlagged, cache on    1.00    1.00     1.00     2.00     0.00     1.00     1.00
  S1 heartbeat#UserStatus, cache on                 1.00    1.00     3.00     3.00     0.00     1.00     1.00
  S2 mark#Read, cache on                            1.00    0.00     0.00     3.00     0.00     0.00     0.00
  S3 send#Message, cache on                         1.00    3.00     1.00     8.00     0.00     3.00     1.00
  S4 update#WikiPage, cache on                      1.00    4.00    97.00    15.00     0.00     4.00     1.01
  S5 set#MessageFlags, cache on                     1.00    1.00     2.00     8.00     0.00     1.00     1.00
```

S3's statements rise by one in both rows: the search index now runs after the commit, on a worker the process-wide count still sees.

### The framework logic (Phase 5)

A first pass of each:

- **F1**: the notification persist is a `force-cache` frame: the message and its users go out in one batch. The email message id is still written after, by `store_email_message_id`.
- **F2**: the end of a job run (its end record, or its row removed with no read first, and a queued call's job and parameters removed) is one `force-cache` frame; when that batch fails, each step runs on its own as before, each failure logged. The lock claim and release stay their own transactions on the reserve connection (a lock other instances wait on); the pass's per-job lock reads and the start record are not changed. Not timed: runs go to the job workers.
- **F3**: the visitor and visit of a new visit are one `force-cache` frame, and so are a login's account update and the visit's link to the user (`session::in_cache_frame`; inside a transaction they join it, as before). Session start and login are still two calls, so not yet one frame.
- **F4**: the audit rows of a held update are held with it (the audit service joins the frame), so they go in its batch; nothing more was needed. Outside a cache frame they are as before.
- **F5**: the pending rows of a commit are inserted in one savepoint, sent deferred, with one wait, in place of a savepoint per row (three waited round trips each). The per-touch re-read and walk to the primary entity are unchanged.

With 200 us of added delay (F1 and F3 have only an "on" row now; compare with the baseline):

```
  operation                                        calls  median ms   us/call    calls/s    allocs      bytes      kept    sql     rt        min-max ms
  S1 heartbeat#UserStatus, cache off                 200    452.562   2262.81        442    1475.6     254465     20808   7.00   8.00  440.532-459.395 
  S3 send#Message, cache off                         200    973.536   4867.68        205    4339.2     561767     23557  13.99  15.01  924.805-986.130 
  S2 mark#Read, cache off                            200    266.088   1330.44        752    1045.3     207535      6783   3.00   4.00  264.227-269.352 
  S4 update#WikiPage, cache off                      200   6704.952  33524.76         30   29033.4    6396039    103132 116.00 117.01 6097.290-7418.027
  S5 set#MessageFlags, cache off                     200    940.512   4702.56        213    5261.2    1960054     20873  15.98  20.98  933.254-1018.972
  F4 audited update UserAccount.username, cache off     200    237.450   1187.25        842     640.8     272997      1820   3.04   4.07  223.137-256.671 
  F5 fed update EmailMessage.isFlagged, cache off     200    479.433   2397.16        417    1865.1     663202      2879   8.98  13.98  471.309-500.177 
  S1 heartbeat#UserStatus, cache on                  200    311.604   1558.02        642    1595.9     243066      6484   3.02   5.03  308.526-350.653 
  S3 send#Message, cache on                          200    819.755   4098.78        244    5271.8     629945     20133   9.99  12.00  753.600-869.730 
  S2 mark#Read, cache on                             200    271.746   1358.73        736    1489.1     237098      6783   3.00   4.00  265.582-318.710 
  S4 update#WikiPage, cache on                       200   1969.028   9845.14        102   48141.1   10077588     19012  15.00  17.01 1530.967-4359.339
  S5 set#MessageFlags, cache on                      200    839.526   4197.63        238    6204.8    2011157     15850  12.98  18.98  813.103-891.563 
  F1 notification persist, 3 users, cache on         200    276.777   1383.88        723     864.9      89138       -53   2.04   5.07  268.810-285.129 
  F4 audited update UserAccount.username, cache on     200    194.532    972.66       1028     998.1     300246       925   1.01   3.03  185.092-200.594 
  F5 fed update EmailMessage.isFlagged, cache on     200    449.478   2247.39        445    2308.9     687037      2879   6.97  12.97  424.486-486.156 
  F3 session start and password login, cache on      200   2573.854  12869.27         78     799.5   20010001      3465   3.04   7.05 2544.336-2639.901
  transaction cache per call                      frames    held   mem rd    db rd    early batch st batch rt
  F1 notification persist, 3 users, cache on        2.00    4.00     0.00     0.00     0.00     4.00     1.00
  F3 session start and password login, cache on     1.00    3.00     0.00     0.00     0.00     2.00     1.00
  F4 audited update UserAccount.username, cache on    1.00    2.00     0.00     1.00     0.00     2.00     1.00
  F5 fed update EmailMessage.isFlagged, cache on    1.00    1.00     1.00     2.00     0.00     1.00     1.00
  S1 heartbeat#UserStatus, cache on                 1.00    1.00     3.00     3.00     0.00     1.00     1.00
  S2 mark#Read, cache on                            1.00    0.00     0.00     3.00     0.00     0.00     0.00
  S3 send#Message, cache on                         1.00    3.00     1.00     8.00     0.00     3.00     1.00
  S4 update#WikiPage, cache on                      1.00    4.00    97.00    15.00     0.00     4.00     1.00
  S5 set#MessageFlags, cache on                     1.00    1.00     2.00     8.00     0.00     1.00     1.00
```

F1 goes from 2.15 to 1.38 ms a call (8 to 5 waited round trips); F3 from 8 to 7 round trips (its time is the argon2 check). F5 is unchanged here: one pending row costs a savepoint, the insert and a release either way; the batch helps only a commit with several pending rows.

## Tests

- **Rust unit tests** beside the code for each gap and each design item; most belong in the moved transaction cache module's tests.
- **One regression check per selected service and piece of logic**: it runs with the cache on in strict mode, with the same rows and results as cache off. Add it to the tests that already cover it: `moqui-core/src/service/chat_tests.rs`, `moqui-host/tests/it/user_status.rs`, `moqui-git/src/service/wiki_tests.rs`, `moqui-mail/src/client_tests.rs`, `moqui-core/src/service/schedule_tests.rs`, `runtime/dev-component/example/test/Chat.spec.js`, `Wiki.spec.js`, `MailClient.spec.js`, inside the regression budget. No new walks.
- **Two first changes of one user at once** (S2's `record#UserChange` change) wait on each other and both commit: a test with two threads, in `moqui-host/tests/it/`, beside `lock_then_insert.rs`.
- **`tx_cache_speed`** is opt-in (`test = false`, `#[ignore]`), as the other speed tests are.
- **The census** is a dev run, not a test in either suite.

## Later rounds

- **mr-mantle**: the census over its specs; then order placement and item changes, shipment receipt, invoicing and payment application, starting from the comments where Java's cache was taken off and the 19 `no-tx-cache` services. Its repeated `OrderItem`, `OrderPart` and `OrderHeader` reads are what D2 and D3 are for.
- **Append-only entities held in any frame**: audit rows, events and change logs are written and seldom read in the same transaction; they could always go to the commit batch.
- **The cache on by default** for some kinds of service, once the census is clean for them.
- **MySQL**: multi-row inserts in `moqui-db` for the commit batch.

## Decisions

Made 2026-11-05.

1. **Reads.** In a hold frame a row read again comes from memory, even if another transaction has committed a change to it since: a mix of repeatable read and read committed, for speed, with the database's locks kept for `for-update` finds. The database is usually read committed, and this is usually the better behaviour, depending on the service (D2).
2. **Locks.** A `for-update` find goes to the database for its explicit lock, always. The locks that creates, updates and deletes take are deferred to the commit; the services and framework operations that turn the cache on account for that, which is usually desirable. A very large operation may not want it, and a service that needs an interim lock runs without the cache (D4).
3. **The authorization cache fix** goes in Phase 0.
4. **`ServiceJob` scheduling and runs** replace agent run events as F2: a few operations, run often.
