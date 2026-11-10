# Service calls: errors and transactions

What a script calling services needs to know about errors, and the pattern for a call that may fail without failing
the caller.

## An error marks the transaction

A service call that ends with an error in `ec.message` (an `addError`, a parameter that fails validation, a script that
throws, or an error left by a service it called) rolls back the transaction it ran in, as Java's `callSingle` does: one
it began is rolled back, and one it joined (the caller's, as `transaction="use-or-begin"`, the default, joins) is marked
rollback-only (`service/call.rs`).

## What the client sees of a failure

A service whose body fails (a script that throws, an error from Rust) puts the failure's message in `ec.message`, and so
in the JSON answer of a transition, the errors saved for the next page and a toast: the message text only
(`context::client_text`), never the script's stack, the generated line, the service it ran in or the Rust source location
(`Rust: crates/...rs:line:col`). Those are in the server log, with the service's name (`Service ... failed: ...`). The
same holds for a service a script calls that is not there, the error answer of a transition that fails, and the error
page's message.

Taking the messages back does not take the mark back. After

```js
ec.message.pushErrors()
ec.service.sync().name('my.Services.check#Thing').parameters({thingId}).call()   // adds an error
ec.message.clearErrors()
ec.message.popErrors()
```

the caller has no error, but its transaction is still rollback-only: each later service call in it is refused with
"Transaction marked for rollback, not running service ...", and what it wrote is rolled back at the end. The same holds
for a Rust caller that takes the errors back (`service::shell::call_tolerant`).

## A probe call

A call whose failure the caller expects and gets past (one item of many, a check the screen only shows, a refresh that
may fail) runs in a transaction of its own:

```js
ec.message.pushErrors()
let info = null
try {
    info = ec.service.sync().name('org.moqui.resource.FileServices.get#ResourceInfo').parameters({driveId, path})
            .requireNewTransaction(true).call()
    if (ec.message.hasError()) { ec.logger.info(`${path} not read: ${ec.message.errorsString}`); info = null }
} finally {
    ec.message.clearErrors()
    ec.message.popErrors()
}
```

The caller's transaction waits while the probe runs and is untouched by its failure. The probe takes a second database
connection and does not see what the caller wrote and has not committed (a record made earlier in the same transaction
is not there for it), so a probe about the caller's own new work cannot be made this way.

## A check that answers instead

A service whose job is to be asked, such as an access or visibility check, answers with an out-parameter and adds no
error, so a caller asks it in its own transaction, sees its own new work, and has no mark to fear:

| Service | Answer |
|---|---|
| `mantle.party.CommunicationServices.get#RecordLinkInfo` with `checkOnly` | `recordVisible`, and `notVisibleReason` for a log |
| `org.moqui.chat.ChatServices.check#RecordVisible` | `recordVisible` from the theme's `STRT_RECORD_VISIBLE_SERVICE` hook |
| `org.moqui.resource.FileServices.get#DriveAccess`, `check#RecordPaths` | `level`; `visible` of each path |
| `org.moqui.resource.WikiServices.get#WikiPageAccess` | `canView`, `canUpdate`, `canAdmin` |

Write such a service the same way: every refusal an answer, nothing in `ec.message`, and only services that add no error
either on the paths it takes (`get#RecordLinkInfo` with `checkOnly` calls `setup#UserOrganizationInfo`,
`get#UserMailboxAccess`, `check#CalendarAccess` with a list and no permission, and `get#WikiPageAccess`, none of which
adds one there). A hook such as `STRT_RECORD_VISIBLE_SERVICE` is called in the caller's transaction for this reason: it
must see records the caller made, so it must answer with `checkOnly` and not fail.

## Reading a parameter's declaration

`ec.service.getServiceDefinition(name)` gives each `inParameterInfoMap` and `outParameterInfoMap` entry a `parameterNode`,
the declared `<parameter>` element as Java's `ParameterInfo` holds it: its attributes, `<description>`, nested
parameters and validations (`first('matches')`, `children()`). `getInParameter(name)` returns the same node. The
transition metadata of `ec.screen.getTransitionInfo` gives each `parameterMap` entry a `hasDefault`, true when the
transition's `<parameter>` sets a `from` or a `value`.

## Transaction cache

A service marked `transaction="cache"` (or `force-cache`, a frame of its own) holds the writes of its transaction in
memory and sends them to the database at the commit, in one batch: each row's writes merged where that is safe, the
statements sent without waiting (an entity's creates joined into multi-row inserts on Postgres) and, on Postgres,
the `COMMIT` sent behind them, one wait for it all (an update that must change a row is checked by the server, so it
fails the transaction rather than holding the `COMMIT` back).
While it holds them its finds see them, and a row it has read or written is not read from the database again. The
design and the numbers are in [plans/archive/TX-Cache.md](../../plans/archive/TX-Cache.md); the code is `moqui-entity`
`entity/cache/tx.rs`, `hold.rs` and `batch.rs`. The `entity_tx_cache` property (default true; `-Dentity_tx_cache=false`) turns it
off everywhere, each `cache` service then running as `use` does.

**Which service to mark.** A screen transition's transaction (and a screen's with `begin-transaction`) holds already:
it begins as `cache` does, unless the `screen_tx_cache` property is `false`. Otherwise the service an operation starts
with (a REST call's, a job's), not an inner service many callers share: a `cache` call that joins its caller's transaction turns holding on for the rest of
that transaction, as Java's `initTransactionCache` does, so a shared inner service would change its callers. A
`transaction` attribute on a `service-call` decides the cache for that call, as in Java: `force-new` (or `use`) there
runs a `cache` service without it, so call one in a transaction of its own with `transaction="force-cache"`.

**What reads see.** Within the transaction, the rows it holds and every row it has read, kept as first read: a row
another transaction commits a change to after this one read it is not seen again until this one ends (between
repeatable read and the database's read committed; usually what a service wants, and cheaper). A row it has not read
yet is read as last committed. A find by the whole key of a row the transaction created, or a list of the children of
a row it created (a condition on a type-one relationship's key), reads nothing from the database. A find the shared
entity cache answers is left to it. A service that must see another transaction's commit mid-way runs without the
cache.

**Locks.** A `for-update` find always goes to the database and locks there, then sees the held writes. The locks that
creates, updates and deletes take come at the commit, when the batch writes them, so they are held for the shortest
time. A service that needs an interim lock from a write (an insert that makes another request wait, a counter row
created so the next one waits on it) takes it with a `for-update` find of a row that is there, or makes that row in a
`force-new` call first (as `ChatServices.record#UserChange` makes a user's `ChatUserState` with `ensure#UserStates`),
or runs without the cache. A very large operation may want its locks as it goes, and runs without it too.

**Errors.** An update of a row the transaction deleted or read as missing fails at once; one of a row it knows nothing
of fails at the commit if it changes no row, as a direct update does. A create of a row the database has fails at the
first find that loads that row, else at the commit; the commit's failure names the write, its key and the service that
held it. A service that must report each row it could not write on its own (`save#DataGridChanges`) runs without the
cache.

**Views.** A find or count of a view entity sees the transaction's held rows of its members without writing them: on
Postgres each member table it holds rows of that may be in the result is read as a sub-select of the table's other rows
and the held ones (`UNION ALL` of their values), so the database joins, filters, groups and counts what the transaction
would see. A view built in code (a data document, a REST query view) reads its members so too, and a `distinct` or
`having` find or count of an entity the transaction holds rows of reads that entity's table so.

**What writes the held rows early** (an early flush, which costs a batch of its own): a view read that cannot be
shadowed so (another database, a member with a full-text field or an encrypted key, more than 500 held rows of its members),
a `distinct` or `having` find of an entity that cannot be shadowed so (the held writes up to that
entity's last); raw SQL (`run_sql`); a `no-tx-cache` service joining (after which the transaction writes
through and keeps what it read); more than 10,000 held writes. Do work that reads a document of the rows a service
wrote (a search index) after the commit (`registerOnCommit`).

**Strict and census modes.** `moqui.tx_cache.strict=true` (or `MOQUI_TX_CACHE_STRICT=true`, tests only) makes an early
flush mark the transaction rollback-only, so a test of a `cache` service fails when it does not commit in one batch.
`moqui.tx_cache.everywhere=true` (`MOQUI_TX_CACHE_EVERYWHERE`, dev and test only) makes every transaction hold; with
`MOQUI_TX_CACHE_CENSUS_FILE` set, each early flush and each failed commit batch is a line in that file, naming the
service: run it over a component's specs to find the next services to mark, and what stops them. With the file set,
in census mode or not, each transaction also writes a `frame` line when it ends: the service that ended it, its cache
mode, its held writes, its reads from memory and from the database, its early flushes and its waited round trips. Two
runs of the same specs, one with the cache everywhere and one without, give per service what the cache would save; a
run before and after marking services gives what it did. Each transaction's
counts (held writes, reads from memory and from the database, early flushes by cause, the batch's statements and waited
round trips) are a debug event at its end, and `entity::cache::tx_cache_stats()` sums them for the thread.
