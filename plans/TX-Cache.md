# Transaction cache

Status 2026-11-05: Phase 0 built (the module move, D1's instruments, strict and census modes, the round-trip count, the authorization reads, `tx_cache_speed`); [Results](#results) has the first census and the baseline. Written from a review of the cache code and two surveys of the framework (services, and Rust code outside services). Line numbers are as of commit `9a13cbe4`.

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

- **Frame**: one transaction level ([`transaction/manager.rs`](../crates/moqui-entity/src/transaction/manager.rs)); a `force-new` call runs in its own frame, and a suspended frame's held rows are not seen by the frame above it.
- **Hold mode**: writes are held in memory (`transaction="cache"` or `force-cache`, `useTransactionCache(true)`).
- **Read-through mode**: writes go to the database at once, and reads are still kept and answered by the frame (Java's read-only cache). Rust has no such mode yet: `flush_and_disable` turns the cache off.
- **Early flush**: any write of held rows before the commit. The **commit batch** is the flush at commit.

## How it works today

**Rust** ([`entity/cache.rs`](../crates/moqui-entity/src/entity/cache.rs), [`entity/cache/hold.rs`](../crates/moqui-entity/src/entity/cache/hold.rs)):
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

- `flush_held` sends deferred writes: creates and upserts by `execute_deferred_row` (the driver joins consecutive one-row inserts of the same SQL into multi-row inserts, `postgres.rs` `RowBatch`), updates and deletes by `execute_deferred` (updates with the `zero_rows` check, G4), then `finish_deferred`, so a failure is the flush's and not the feed hook's. With the feed insert batched too (F5), a commit waits about once for the batch and once for `COMMIT`.
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

### Found along the way, not the transaction cache

- **Authorization and tarpit checks are read with no cache.** `load_group_checks` and `load_tarpit_checks` (`moqui-core/src/user/authz.rs:896-961`) find `ArtifactAuthzCheckView` and `ArtifactTarpitCheckView` per group without `use_cache`; Java reads both with `useCache(true)` (`UserFacadeImpl.groovy:884, 901`). With the default tarpit on every screen, that is 13-16 queries per request. Taken in Phase 0: read both through the cache (a member write already evicts a view's cached finds through `views_by_member`), and keep `ArtifactTarpitLock` in memory with its invalidation (add it to `is_authz_entity`, `authz.rs:74-84`, so a cluster peer's change reaches it).
- **Theme preferences**: 13 keys read one at a time, two queries each, per appearance build (`moqui-core/src/screen/sri/appearance.rs:463-469`); read them with `get_preferences()`.
- **ETL loader**: no pipelined writes inside a batch (`entity/etl.rs:169-262`), as the data loader has. Not with the cache: under hold every row is read first.

## Phases

Each phase ends with its regression tests passing and `./moq test-framework` before its commit.

0. **Preparation.**
   - Move the transaction cache code (hold, overlay, stored rows, flush) out of `cache.rs` (2,921 lines) into `entity/cache/tx.rs` beside `hold.rs`, in its own commit, as [Practices-Rust](../docs/dev/Practices-Rust.md) asks of a large file that gets real work.
   - D1's counts, strict mode, census mode and the round-trip count.
   - A new opt-in speed test, `tx_cache_speed`, with a row for each of the ten candidates, cache off and (where it runs) on. Record the baseline.
   - The authorization read fix.
   - A first census run, recorded here.
1. **Correctness: G1 to G9.** A unit test per gap that fails before its fix.
2. **The commit batch: D6.** Speed test rows before and after; the external-Postgres run with delay.
3. **Reads: D2, D3, D4, D5.** Unit tests counting statements, in the style of `held_updates_read_their_stored_rows_once_per_frame`.
4. **The five services.** Turn the cache on for each (S1 to S5), run its tests in strict mode, and change it until it commits in one batch with results the same as cache off.
5. **The five pieces of framework logic** (F1 to F5).
6. **Profile and tune.** samply and heaptrack on the speed test rows ([Profiling](Profiling.md#how-to-measure)): the overlay, the read cache lookups, the batch's statement building. Record the numbers as a new round in Profiling and a line in [Performance-Summary](Performance-Summary.md). Write a "Transaction cache" section in [docs/logic/Services.md](../docs/logic/Services.md): when to mark a service `cache` (the operation's first service, not a shared inner one), what reads see (the [decisions](#decisions)), that the locks writes take come at commit while `for-update` locks at once, when to run without the cache instead (an interim lock from a write, a very large operation), what makes it flush, and strict and census modes.

## Results

### Census 1 (Phase 0)

Census mode (`MOQUI_TX_CACHE_EVERYWHERE=true`, every frame a `run` begins or joins holds) over the specs of `basic-comms`, `framework`, `tools`, `webroot`, `dev-aide` and `example` (`./moq test`), before any fix: 393 features, 235 passed, 33 failed, 125 skipped after a failure in a stepwise spec, and 2 spec setups failed (`ChatKeys`, `ChatKeyBackup`).

- **Chat and everything over it** (the Chat, Calls, Voice, XMPP and basic-comms specs): `No chat change counter for user ...` (`record#UserChange`'s lock-then-read of `ChatUserState`) and `Chat room ... not found` (a `for-update` or select-field find of a room the frame created). Both are G1.
- **Commit flush failures**: `DatabaseServices.save#DataGridChanges` (a held create of an `Enumeration` the database has fails only at commit, so the rows saved before it are not rolled back one by one as the spec expects: G5), and one with no service on the stack, an insert of `WorkEffortParty` whose `WorkEffort` was not there (a data load).
- **Early flushes, all of views** (in Mantle services run by the demo data load): `ProductServices.create#VariantProduct` 12 (`ProductAssocAndToFeatureAppl`), `TimeServices.create#TimePeriod` 3 (`PartyDetail`), and one each in `CalendarServices.store#CalendarFeedEvents`, `LedgerServices.get#OrganizationFiscalTimePeriods` and `CommunicationServices.create#EmailCommunicationEvent`.

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
