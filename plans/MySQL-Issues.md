# MySQL issues

Status 2026-10-31: found by loading every data type (seed through demo, with all the components in `runtime/component`)
into MySQL 8.4.13 from the `mysql:8` image. All are deferred. The MySQL fixes made at the time are listed at the end.

The load stored 29,609 records in 216 seconds and skipped 10 data files. Two tables were not created. About 200 of those
seconds were four lock wait timeouts (issue 5). Issues 1 to 5 are about MySQL. Issues 6 to 8 happen on any database and
were found on the way.

| # | Issue | Effect on MySQL | MySQL only |
|---|---|---|---|
| 1 | `EMAIL_MESSAGE` row is over MySQL's row size limit | No table: every email send fails, the mail demo does not load | Yes |
| 2 | `PRODUCT_VARIANT_IDENTITY` primary key is over MySQL's key length limit | No table: variant products cannot be created | Yes |
| 3 | Tables are 3-byte `utf8`, so a 4-byte character (an emoji) cannot be stored | The chat demo does not load; any emoji in user text fails | Yes |
| 4 | `text-medium` is `VARCHAR(255)` on MySQL and `TEXT` on Postgres | Two data files with longer values fail, and files after them that need their rows | Yes |
| 5 | Git update events wait on the load's locks until the 50 second timeout | Four git updates not recorded; the load takes about 200 seconds longer | Probably |
| 6 | The log line leaves out a field named `error` | No cause in the console or log file for many errors | No |
| 7 | The data feed's commit hook can panic on a borrowed connection | The commit fails with "RefCell already mutably borrowed" | No |
| 8 | A `seed` file depends on a `seed-initial` file | A seed-only load skips it | No |

## How it was run

```sh
docker run -d --rm --name moqui-mysql-test -e MYSQL_ROOT_PASSWORD=moqui -e MYSQL_DATABASE=moqui \
    -p 127.0.0.1:33306:3306 mysql:8
entity_ds_db_conf=mysql8 entity_ds_host=127.0.0.1 entity_ds_port=33306 entity_ds_database=moqui \
    entity_ds_user=root entity_ds_password=moqui entity_ds_sslmode=disable entity_ds_crypt_pass=mysql-tryout-key \
    ./moq load --debug
docker stop moqui-mysql-test    # --rm removes the container and its volume
```

- **Package:** the Dev package, which `./moq load` builds by default, has no MySQL driver and stops with "the mysql
  driver is not in this build". The Debug, Maximal and Enterprise packages have it.
- **Tests:** the tests that need a server read `MOQUI_TEST_MYSQL=user:pass@host:port/database`:
  `cargo test -p moqui-drivers --features mysql -- mysql` (these pass without a server, as they skip),
  `cargo test -p moqui-host --test it -- gate1::mysql --ignored` and
  `cargo test -p moqui-host --test it -- create_or_update_race --ignored`.
- **A database of its own:** give the tests and a load separate databases. The race test leaves `ArtifactAuthz` and
  `ArtifactGroupMember` rows whose enumeration ids are not in `ENUMERATION`, so a later load cannot add those foreign
  keys. A test facade with only the framework model, over a database holding the components' data, hits issue 7.

## Issues

### 1. `EMAIL_MESSAGE` is over the row size limit

`CREATE TABLE EMAIL_MESSAGE` fails with MySQL error 1118, "Row size too large": the limit is 65,535 bytes, not counting
`TEXT` and `BLOB` columns. `text-long` is `VARCHAR(4095)`, up to 12,287 bytes in 3-byte `utf8`. Java's `EmailMessage`
has four `text-long` fields (`note`, `toAddresses`, `ccAddresses`, `bccAddresses`), and the mail model here added two
more, `referencesHeader` and `sendError` (`runtime/base-component/framework/entity/`). Six of them, about 74,000 bytes,
are over the limit by themselves.

As the table is missing, every `send#EmailTemplate` fails, as does the BOSS mail demo (`BossMailDemoData.xml`) and the
list and unsubscribe emails of the BOSS demo.

Fix: make `referencesHeader` and `sendError` `text-very-long` (`LONGTEXT`, stored off the row). Nothing indexes or
sorts them. With 4-byte `utf8mb4` (issue 3), Java's four `text-long` fields alone take 65,528 bytes, and the other
columns put the row over the limit, so a move to `utf8mb4` needs more of them off the row.

### 2. `PRODUCT_VARIANT_IDENTITY` key is over the key length limit

`CREATE TABLE PRODUCT_VARIANT_IDENTITY` fails with MySQL error 1071, "Specified key was too long; max key length is
3072 bytes". mr-mantle's `ProductVariantIdentity` (`entity/ProductCatalogEntities.xml`) has `featureKey` as `text-long`
in its primary key. Even its documented limit of 32 serialized feature ids, about 1,300 characters, is over 3,072
bytes at 3 bytes a character.

As the table is missing, `create#VariantProduct` fails, and so does `ZcaProductDemoData.xml`.

Fix, in mr-mantle: key on a hash of the sorted feature ids (a SHA-256 hex digest fits an `id-long`) and keep the list
in a non-key field, or limit the key to about 980 characters (`text-intermediate` is 1,023, too long by a little).

### 3. Tables are 3-byte `utf8`

The `mysql8` database in `MoquiDefaultConf.xml` names `character-set="utf8" collate="utf8_general_ci"`, as Java's
does, so each `CREATE TABLE` uses utf8mb3. Each connection runs `SET NAMES utf8mb4`. A 4-byte character fails with
error 3988, "Conversion from collation utf8mb4_0900_ai_ci into utf8mb3_general_ci impossible for parameter". So
`ChatDemoServices.load#ChatDemo` fails and `BossChatDemoData.xml` is skipped. Mail, chat and Comms text with emoji
cannot be stored on MySQL.

Fix: `utf8mb4` (with `utf8mb4_0900_ai_ci`) on `mysql8`. Each character can then take 4 bytes, which affects issues 1
and 2 and every index on a long text column: at 3,072 bytes a key column fits 768 characters, so `text-medium` (255)
still fits and `text-intermediate` (1,023) does not. An existing database needs its tables converted
(`ALTER TABLE ... CONVERT TO CHARACTER SET utf8mb4`), which should be a documented upgrade step, not something done at
startup.

### 4. `text-medium` is 255 characters on MySQL only

The Postgres database types map `text-medium`, `text-intermediate` and `text-long` to `TEXT`. MySQL keeps the
dictionary's lengths: `VARCHAR(255)`, `VARCHAR(1023)` and `VARCHAR(4095)`. MySQL's strict mode then refuses a longer
value with error 1406, "Data too long", where Postgres stores it:

- `MantleDocumentSalesData.xml` (mr-mantle, `seed-initial`): a `DataDocumentField.fieldPath` expression of about 270
  characters.
- `BossSetupData.xml` (mr-boss): `ArtifactGroupMember.artifactName` patterns of 291 and 754 characters.

Files that need the rows of a skipped file then fail on a foreign key: `MonitoringSalesSecurityData.xml` (the
DataDocuments of the first file), and probably `BossUsersDemoData.xml` (`UserGroupMember` to a user group of
`BossSetupData.xml`) and `BossZAccountingDemoData.xml` (`GlAccountOrganization` to a `GlAccount`). The last two were
not traced.

Fix: give the long values a longer type (`artifactName` is part of a key, so it stays within 768 characters with
issue 3), or shorten the values: split the 754-character pattern into several `ArtifactGroupMember` rows. Running the
`seed` through `demo` loads on MySQL now and then would catch new ones.

### 5. Git update events wait on the load's locks

During the git demo data, `moqui_git::events` logged "cannot record a git update ... Lock wait timeout exceeded" four
times (repositories `iron` and `acme-portal`), each after InnoDB's default `innodb_lock_wait_timeout` of 50 seconds.
"git update events run without their user" was logged once, on `moqui-request-0`. The data load did not fail, but those
updates were not recorded.

Not traced. A likely cause: InnoDB's foreign key check takes a shared lock on the parent row, where Postgres takes
`FOR KEY SHARE`, which does not conflict with an update of other columns. If the load's transaction inserts a row that
refers to the repository, and the event recorder updates that repository row in a transaction of its own while the
load waits for it, that is a deadlock across two transactions, which ends only at the timeout. To check: turn on
`innodb_print_all_deadlocks` and the lock wait views in `performance_schema.data_lock_waits`, and run the git demo
file alone.

### 6. The log line leaves out the `error` field

`tracing::error!(error = %err, ...)` is the usual form in this tree, but `Fields` in `crates/moqui-host/src/logging.rs`
keeps a field named `error` only for the telemetry document's `thrown`. It is never written to the console or to the
log file. The table checks (`moqui_entity::entity::meta`, "Could not create table ...") logged their SQL and no cause:
the causes of issues 1 and 2 above came from running that SQL by hand.

Fix: append the error to the console and file lines (for example `error=...` after the message, as the other fields
are), and add a logging test that a field named `error` is in the line.

### 7. The data feed's commit hook can panic

`run_conn_hooks` (`crates/moqui-entity/src/transaction/manager.rs`) holds the connection's `RefCell` borrowed while
it runs each commit hook. The feed insert hook (`insert_pending` in `entity/load_feed/feed.rs`) resolves its feeds'
entity names (`loader::canonical_name`). A name that is not in the model is looked up as a database-defined view
(`entity/derived.rs`), in a suspended transaction, and `TransactionManager::suspend` borrows that same connection
again. The panic is caught and the commit fails with "commit hook panicked: RefCell already mutably borrowed".

Seen with a facade that has only the framework model over a database holding mr-mantle's DataFeeds and DataDocuments.
A server would hit it once a DataDocument names an entity no longer in the model, such as one from a component taken
out, and then every commit with a write to a fed entity would fail.

Fix: resolve the feed targets before the hook runs (when the hook is registered, outside the commit), or give
`insert_pending` the model only and leave out a name it does not have, with a warning.

### 8. A `seed` file depends on `seed-initial` data

`MonitoringSalesSecurityData.xml` (mr-mantle, `seed`) adds `DataDocumentUserGroup` rows for `MantleSalesOrderItem`
and `MantleSalesOrderPart`, which `MantleDocumentSalesData.xml` defines as `seed-initial`. So `./moq load seed` skips
the file on any database. Fix: make it `seed-initial`, the type of the documents it grants.

### Smaller

- A build with `mysql` and without `postgres` warns that `POSTGRES_PORT` and `search_path`
  (`crates/moqui-drivers/src/entity_db.rs`) are never used. They need `#[cfg(feature = "postgres")]`.

## Fixed at the time

On the same branch, before and while this was found:

- Each connection to MySQL or an external Postgres sets the datasource's isolation level (`default-isolation-level`,
  `ReadCommitted`), so MySQL no longer runs at InnoDB's `REPEATABLE READ`. A Postgres level other than
  `ReadCommitted` is logged as a warning.
- `render#ScheduledScreen` locks `ScreenScheduled` before its lock record, so two runs cannot deadlock on gap locks.
- A create-or-update or data load upsert on MySQL is not one `ON DUPLICATE KEY UPDATE` for an entity with a unique
  index other than its key, as that statement takes any unique index and could update another row. Where the insert
  meets another row's unique value, the error now says so, not "Tried to update a value that does not exist".
- An update that leaves the row as it was is no error on MySQL. MySQL counts the rows an update changes, so one that
  rounds an amount to the stored value counted none and failed with "Tried to update a value that does not exist":
  `OrderServices.update#OrderPartTotal`, in `BossProductsDemoData.xml` and `BossProjectsDemoDataTimeBilling.xml`. The
  fix has a test against the server (`gate1::mysql_metadata_insert_and_select`); the data was not loaded again after it.
