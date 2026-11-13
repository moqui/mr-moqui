# Database, data and ETL

The Tools app (`/apps/tools`) administers the databases of the entity groups and, outside production, external ones.
Its sections are Data (the data grid, record forms, views, load, export, snapshots), SQL, Schema, ETL and Database. The
design is [plans/archive/Tools-Database.md](../../plans/archive/Tools-Database.md); this guide says what was built.

The code:

- `crates/moqui-tools/src/`, the server side:
  - `capabilities.rs`, `catalog.rs`, `activity.rs`, `tables.rs`, `settings.rs`: what each database allows, and its
    catalog queries per dialect;
  - `schema.rs`, `jobs.rs`: the comparison, plans and jobs;
  - `sql.rs`, `sql_service.rs`: SQL runs, tokens, cancel, explain, history;
  - `grid.rs`, `grid_service.rs`, `files.rs`: the data grid and its file source;
  - `etl/`: mappings, runs and checks;
  - `ext.rs`: external databases;
  - `audit.rs`: the audit rows.
- Lower layers:
  - the schema comparison: `EntityDbMeta::compare` in `crates/moqui-entity/src/entity/meta.rs`, its catalog SQL in
    `crates/moqui-db/src/sql/`;
  - statement stats: `crates/moqui-entity/src/entity/exec.rs`;
  - batches and transactions of a load: `EtlLoader` in `crates/moqui-entity/src/entity/etl.rs`;
  - the extract-transform-load core: `crates/moqui-lib/src/etl.rs`.
- Services:
  - `runtime/base-component/framework/service/org/moqui/dev/DatabaseServices.xml`;
  - `EtlServices.xml`, beside it.
- Entities: `runtime/base-component/framework/entity/DatabaseEntities.xml` (packages `moqui.database` and
  `moqui.etl`).
- Screens: `runtime/base-component/tools/screen/Tools/` (Data, `Sql.xml`, Schema, `Etl.xml` and `Etl/`, Database),
  listed in the [tools README](../../runtime/base-component/tools/README.md).

## The database selector

- **The `db` parameter.** Each Data, SQL, Schema, ETL and Database screen takes `db`:
  - `group:<groupName>` for an entity group's database;
  - `ext:<connectionId>` for an external one (see [External databases](#external-databases)).
- **Groups that share a database** appear once, named by their groups (`get#DatabaseList`).
- **The last choice** is the user preference `database.selected`. The picker is `template/DatabaseParts.xml`.
- **Where external databases are not offered:** Schema, the grid's entity sources and ETL targets list only the groups'
  databases. No entity model applies to an external one.

## The data grid

Data › Grid (`Data/Grid.xml`, `Grid.qvue`) is a Tabulator grid. Its `tui` render mode is a paged read-only list with
a record form. The grid keeps no state on the server: each call (`get#DataGridColumns`, `get#DataGridRows`, …) takes
the source spec and runs the source again.

**Sources:**

| Source | Read | Saves |
| --- | --- | --- |
| An entity | An entity find. Conditions are search-form inputs, and the order ends with the primary key so pages are stable | Yes |
| A view entity, `DbViewEntity` or `DataDocument.<id>` | The same find. A data document's expression fields are read-only columns, computed in the rows | No |
| SQL (`db`, `sql`) | One `SELECT`, `WITH`, `VALUES` or `TABLE`, wrapped as `SELECT * FROM (sql) q`, run read only and audited | Only when every column is of one table, with its entity's whole primary key and `lastUpdatedStamp`, from a statement with no join, set operation, `DISTINCT`, `GROUP BY` or subquery in its `FROM` |
| A file (`location`, `sheet`, `headerRow`, `format`, `delimiter`) | CSV, or XLSX, XLSM, XLSB, XLS or ODS. A sheet is cached per node (64 MB); a CSV is read for each call | No |

- **Opening a source.** The screen opens an entity by name, a saved view, a SQL result handed over by the SQL
  screen's "Open in grid", or a file.
- **File sources.** File uploads a CSV or spreadsheet (`upload#GridFile`), or names a location. Then it picks the
  sheet, the header row and the format (`get#GridFileInfo`), and the grid shows the file read only (the `type: "file"`
  source).
- **Load into…** (a file source only) loads the file through ETL, over the JSON transitions of `Etl.xml`:
  - it matches the file's columns to a target's fields (`match#EtlFields`) and shows the first 200 rows converted, with
    each field's error (`check#EtlSample`);
  - then Save mapping keeps it as a saved mapping, Check checks the whole file with nothing written, and Load runs it.
- **Where a file may come from.** Only the places ETL reads (see [ETL](#etl)). Uploads go to
  `dbresource://moqui/database/upload/<userId>/<uploadId>/<name>`, at most 100 MB, and `clean#GridUploads` (daily)
  removes them after seven days.

**Filters.** Each column's header has a filter. Plain text is "contains", case ignored, for a text column. Text
that begins with an operator filters by it, which is how a number or date column is filtered:

| Typed | Rows where the column |
| --- | --- |
| `=X`, `!=X` | is, or is not, X |
| `>X`, `>=X`, `<X`, `<=X` | compares so with X |
| `^X` | begins with X |
| `in:A,B,C` | is one of the list |
| `empty`, `!empty` | has no value (or an empty text), or has one |

**A record.** An entity's row has Record, which opens that record's form (Data › Record) with its related records'
links; a view's or a data document's has Related, which opens its main entity in Data › Browse.

**Pages and counts.**

- No source is counted on each load. While pages come back full, the last page is the current one plus one.
- An unfiltered entity source shows the catalog's row estimate.
- Count rows runs `count()`, or `count(*)` over the wrapped SQL.

**Editing and saving.**

- **In the grid:** cells edit in place by field type. Related-entity fields get a lookup list (`get#DataGridLookup`).
  Undo and redo, range copy and paste, and adding and deleting rows work; the changes stay marked until saved.
- **Save** (`save#DataGridChanges`) is one transaction, through each entity's `update#`, `create#` and `delete#`.
  Authorization, validation, ECAs and the audit log apply.
- **Only edited fields are sent**, and a refused row rolls back every other. The answer lists each refused row and its
  field errors, and the grid keeps the edits so they can be fixed and saved again.
- **Not saved in the grid:** views, data document definitions, encrypted fields (shown masked), key fields of an
  update and `lastUpdatedStamp`.

**Conflicts.**

- Each row carries `_stamp`, its `lastUpdatedStamp`.
- A save sends the stamp back. A row changed or removed since it was loaded, or no longer visible to the user, is
  refused: "changed or removed by someone else since you loaded it: reload it".

**Export** (`export#DataGrid`): the whole source (conditions, filter, sort, columns), not only the loaded pages, as
CSV or XLSX.

- **Size:** an entity source is read through its cursor. XLSX stops at 100,000 rows, a SQL source at 200,000, and
  `truncated` says so.
- **Where it goes:** to a download, or to the user's own `runtime://tmp/grid-export/<userId>/` or
  `dbresource://moqui/database/export/<userId>/`. With `DATABASE_ADMIN` it may also go to any `dbresource://`
  location or under `runtime://export/`; never to `component://` or `file:`.

### Calc mode

The grid has two modes over the same source. Records is the grid above. Calc shows the source as the data sheet of a
workbook on the server's spreadsheet engine (IronCalc; plan [IronCalc-DB](../../plans/IronCalc-DB.md)), over
`org.moqui.calc.CalcServices` (`crates/moqui-calc`). Editing the workbook (formulas, formats, new sheets) is the plan's
Phase 3; today Calc reads.

- **The data sheet** is the source with the columns shown and the sort and filters set when Calc is opened: a header
  row of the columns' names, then a row for each record, typed (numbers, dates and times by their formats, text as it
  is), read through the grid's reader as the user, with its refusals. It takes at most 100,000 rows and says so when a
  source has more. Encrypted and binary columns are left out.
- **A spreadsheet file** opens as a workbook from the File dialog (Open as a workbook): every sheet, and for an xlsx
  its formulas, styles and names, calculated by the engine. An xls, xlsb or ods, and an xlsx the engine does not
  take, open as values.
- **The view** (`js/WebrootCalc.js`) holds no data: the server answers the cells in view and a margin
  (`get#CalcCells`, at most 20,000 cells a request), so a jump to a far row loads nothing between. Sheet tabs are
  below the sheet, a data sheet marked; the document's charts are below those.
- **A room** is the open workbook in the server's memory: one for each open document of a user, found only by that
  user, at most eight a user, dropped after twenty minutes without a request. A request for a room that is gone is
  answered `ROOM_GONE` and the screen opens the document again. A room is on the server that opened it.
- **Refresh** reads the data sheets again and calculates. **XLSX** downloads the workbook as it is, data and
  calculated values.
- **A document** (Save document, Documents) is a report definition of type `calcgrid`: the data sheets' sources,
  the charts, and the workbook without its data. No row of a data sheet is stored, no value calculated from one, and
  none of the data's texts; opening reads the data again as the user who opens it. Its versions and sharing are the
  definitions' ([Monitoring](Monitoring.md#the-dashboards-app)); it is at most 8 MB.
- **Formulas over a data sheet** name its columns (`=SUM(Enumeration[sequenceNum])`): the sheet is a table named for
  it. Every formula is calculated again on a refresh, so many sums over a large sheet are slow: the toolbar shows the
  time when it is over a quarter of a second.

**Saved views** (`moqui.database.GridView`).

- **What one holds:** the source spec and the layout (columns, widths, order, frozen columns, filters, sort).
- **Who sees it:** owned by their user, and shared or not.
- **A view of a SQL source** can be saved only by a user who may run SQL. The screen shows its SQL before running
  another user's view.

**Secrets and restricted entities.** The grid refuses, for reading and saving:

- the identity-admin, secret-config and audit entities, and any view with one of them as a member
  (`UserAccountAndGroup`, `UserGroupMemberUser`);
- `UserAccount`'s password fields, whether by name or as a view's alias that renames one. These are left out of
  columns, rows and exports, and take no sort, filter or condition.

## SQL

Tools › SQL (`Sql.xml`, `Sql.qvue`, with `SqlTui.xml` for the terminal) needs `SQL_RUNNER_WEB` and authorization for
all actions on the Tools app. The services check both whatever calls them.

- **The editor:**
  - CodeMirror with the Postgres, MySQL or MariaDB dialect. Completion of tables, columns and entity names comes from
    `get#SqlCatalog`.
  - Ctrl+Enter runs the statement at the cursor or the selection; Ctrl+Shift+Enter runs the script.
  - `:name` parameters are bound as values.
- **Read only by default.** `run#Sql` runs statements in a read-only transaction (`SET TRANSACTION READ ONLY`,
  `START TRANSACTION READ ONLY`), so the database refuses writes. This guards against mistakes, not against the user:
  the embedded cluster's connections are the bootstrap superuser's, and a superuser can still run
  `COPY … TO PROGRAM` in a read-only transaction. That is why SQL is an `ADMIN_ADV` permission.
- **Allow changes:**
  - With the switch on, Run first asks `classify#Sql`, which lists the statements that may change data.
  - The confirmation gives a one-time token (`moqui.database.SqlRunToken`) for this user, `db` and text, good for ten
    minutes on any instance; `run#Sql` spends it.
  - A script with changes is one transaction, stopping at its first failure, unless each statement runs on its own.
  - `VACUUM`, `CREATE INDEX CONCURRENTLY` and `CREATE DATABASE` run outside the transaction.
  - Transaction control (`COMMIT`, `BEGIN`, `SET TRANSACTION`, …) is refused in every mode.
  - On MySQL and MariaDB, DDL commits implicitly, and the confirmation says so.
- **Production** (`instance_purpose` production): a run with changes also needs the database's name typed.
- **Limits:** 500 rows per query (at most 100,000), with Fetch more for a read-only query. The timeout is 5 minutes
  (at most an hour).
- **Results:** each query gets a result tab. A plain query run read only can be sorted, filtered and fetched again;
  other results hold the rows they fetched. Results copy, download as XLSX (`SqlDownload.xml`), or open in the grid.
- **Cancel** (`cancel#SqlRun`): a run has a `runId`.
  - On its own instance, the connection's canceller stops it.
  - On another instance, it is found by its running audit row and stopped with `pg_cancel_backend` or `KILL QUERY`,
    once the session is checked to be the same one.
  - A user cancels their own runs; `DATABASE_ADMIN` cancels any.
- **Explain** (`explain#Sql`): `EXPLAIN (FORMAT JSON, VERBOSE, BUFFERS)` on Postgres, `EXPLAIN FORMAT=JSON` on MySQL
  and MariaDB. Explain analyze runs the statement. For one that may change data it needs the token and runs in a
  transaction that is rolled back.
- **History and saved queries:**
  - History is the user's own audit rows (`find#SqlHistory`).
  - Saved queries are `moqui.database.SavedQuery`, owned and optionally shared.
- **The audit:** every statement writes a `DatabaseAudit` row (`DbaSql`), reads and failures included, in a
  transaction of its own. So does a refused attempt with changes. See [The audit](#the-audit).
- **The read fence:** an agent's read tool still refuses non-queries (`fence::check_sql`).

## Export and snapshots

- **Export** (`Data/Export.xml`) writes entity data as XML, JSON or CSV to the browser (a download), a file or a
  directory on the server. More than one entity as CSV to the browser is a zip with a file for each. Filter is a
  JavaScript object of field values the records must have, such as `{statusId: 'X'}`.
- **Snapshots** (`Data/Snapshots.xml`) exports and imports whole snapshots as zips in `runtime/db/snapshot`. Upload
  Snapshot puts a zip there, replacing one of the same name; Download and Delete act on one.

## Schema

- **Entities and Entity** (`Schema/Entities.xml`, `Schema/Entity.xml`): the entity list with table sizes, and one
  entity's fields with its table's differences.
- **Check Tables** runs the framework's table check and shows the DDL it ran.
- **Indexes and keys** (`Schema/IndexesKeys.xml`, `compare#Schema`): the model against the database. It shows:
  - missing tables;
  - missing, extra and differently typed columns;
  - missing, extra and invalid indexes;
  - missing and extra foreign keys;
  - on Postgres, foreign keys no index leads with.
- **Plans** (`plan#SchemaChanges`): a selection becomes the statements that fix it, in their online forms:
  - Postgres: `CREATE INDEX CONCURRENTLY IF NOT EXISTS`, `DROP INDEX CONCURRENTLY IF EXISTS`, and a foreign key as
    `ADD CONSTRAINT … NOT VALID` then `VALIDATE CONSTRAINT`. An invalid index is dropped and made again.
  - MySQL: `ALGORITHM=INPLACE, LOCK=NONE`.
  - The screen shows the plan. With `DATABASE_ADMIN` it is stored as a Pending job that runs as shown.
- **Download DDL** (`get#SchemaDdl`): `CREATE TABLE`, index and foreign key statements of the selected entities, in the
  database's dialect, from the builders the table check uses.
- **Load wrap** (`plan#LoadWrap`): Data › Load and Snapshots can drop the foreign keys of the loaded tables first and
  make them again after (Postgres `NOT VALID`, then `VALIDATE CONSTRAINT`).
  - The load runs on a worker as the `DatabaseLoadWrap` ServiceJob (`tools/service/moqui/tools/DatabaseLoadServices.xml`).
  - It runs as the user, through `ec.artifactExecution.runAsUser`.

**Jobs** (`moqui.database.DatabaseJob` and `DatabaseJobStatement`). A job is only a plan the server made:
`plan#SchemaChanges`, `plan#TableMaintenance` or `plan#LoadWrap`. No service takes statement text for a job.

- **Running** (`run#DatabaseJob`, on a worker):
  - Each statement runs on a connection of its own, outside a transaction, with no statement timeout. Its backend id
    and start are recorded.
  - Statements that take a strong lock wait at most the job's lock timeout (5 s by default) and are tried three more
    times. The `CONCURRENTLY` forms, `VALIDATE CONSTRAINT`, plain `VACUUM` and `ANALYZE` take none.
  - A failed statement does not stop the others, and one whose dependency failed is skipped.
  - When the job ends, its user is notified on topic `DatabaseJob`.
- **Progress** (`get#DatabaseJobStatus`, which the job's page polls) and the sessions blocking it:
  - Postgres: `pg_stat_progress_*`, and `pg_blocking_pids` joined to `pg_stat_activity`;
  - MySQL: `performance_schema` stages and `sys.innodb_lock_waits`;
  - MariaDB: the `PROCESSLIST` progress.
- **Cancel** (`cancel#DatabaseJob`) works from any cluster member: the running statement is stopped by its recorded
  backend id, or by the local canceller.
- **After a restart:** a job left Running by an instance that is gone is marked Failed. A plan never run is deleted
  after a day.
- **Permission:** running and cancelling jobs need `DATABASE_ADMIN`.

## ETL

Mappings load files, SQL results and entity data into entities or services. The services are `EtlServices.xml` and
`crates/moqui-tools/src/etl/`.

**The screens** (`Etl.xml` and `Etl/`). `Etl.xml` also holds the JSON transitions that the mapping form and the grid's
Load into… call.

- **Mappings** (`Etl/Mappings.xml`): the saved mappings, found by description or target, with copy and delete.
- **Mapping** (`Etl/Mapping.xml`, `Mapping.qvue`; read only in the terminal):
  - the source, the target and the options;
  - the source column to target field table, with each field's converted sample value and error (`check#EtlSample`, the
    first 200 rows), and a test against another file;
  - Save, Check (the whole source, nothing written), Run, Schedule, and the mapping's runs.
  - An expression field needs `GROOVY_SHELL_WEB` and a SQL source `SQL_RUNNER_WEB`. Without them those inputs are
    disabled.
- **Schedule** (on the Mapping, the `etlSchedule` transition) makes a `ServiceJob` that calls `run#EtlMapping` with
  the `mappingId`. See [Scheduling](#scheduling) below.
  - The cron expression is checked first: one that does not parse, or is never due, is refused. The answer gives the
    next run.
  - The mapping is stored again, so the schedule gets the same checks as a change of the mapping. That limits it to the
    mapping's owner or a `DATABASE_ADMIN` holder.
  - The job is made paused. Its `mappingId` parameter and a `ServiceJobUser` of the mapping's owner (notified of each
    run) are added, and only then is the job unpaused, so the scheduler never sees half a job. A failure removes what
    was made.
- **Runs** (`Etl/Runs.xml`, `get#EtlRuns`): the latest runs, newest first. These are the user's own, or everyone's with
  `DATABASE_ADMIN`; with `mappingId`, one mapping's.
- **Run** (`Etl/Run.xml`, `get#EtlRun`): the status and counts as the run goes, the first errors, and the error file
  of every failed row as a download. Rerun (`rerun#EtlRun`, from `nextStartRow`; see below) is offered only when
  `canRerun` says it would be taken.
- **Value Maps** (`Etl/ValueMaps.xml`): the value maps, each changed or deleted by its owner or a `DATABASE_ADMIN`
  holder.

- **A mapping** (`moqui.etl.EtlMapping` and its `EtlMappingField` rows, or an inline map with the same names):
  - **the source:** an entity find, SQL on a group's database, a CSV or spreadsheet, or a flat record XML file;
  - **the target:** an entity (create, store or update, with `matchFields` for a natural-key match), or a service
    called once per row;
  - **each field:** a source column (a header, or a column letter or number), a constant or default, a value map, a
    lookup, a format pattern, an expression, and `trim`, `required` and `skipIfEmpty`.
  - Repeated headers are named as the grid names them (`id`, `id_2`). Number patterns read grouping as `DecimalFormat`
    does: `#,##0` is a whole number, `#.##0,00` has a decimal comma.
- **Value maps and lookups:**
  - A value map (`EtlValueMap`, its entries matched ignoring case) turns "Active" into `ACTIVE`. A field may carry an
    inline `valueMap` instead; saving the mapping stores it as a value map of its own.
  - A lookup finds a record by one field and takes another (a product name to its `productId`), one find per distinct
    value per run, as the user.
- **Expressions** are JavaScript with `row`, `value` and `ec`. A user needs the sealed `GROOVY_SHELL_WEB` permission to
  save, check or run a mapping that has one, and the run checks again before it evaluates one.
- **Running** (`run#EtlMapping`): on the worker pool, or here with `wait`, which suspends the caller's transaction.
  - **The run is the user's own,** with authorization on, also on a worker or in a ServiceJob, whose calls are made
    with it off. The Tools app's authorization for all actions is checked again as the user and pushed as the run's
    frame (`etl::access::run_as_user`). So every write is an entity-auto or target service call that runs the entity's
    ECAs and the security rules.
  - **The `EtlRun` row** has the counts, written every 1,000 rows: `extractCount` rows read, `loadCount` rows committed,
    `errorCount` rows with an error. The first 1,000 errors are `EtlRunError` rows.
  - **The error file** is `dbresource://moqui/etl-run/<userId>/<runId>/errors.csv`. It is read through `get#EtlRun`
    with `includeErrorFile`, by the run's user or a `DATABASE_ADMIN` holder.
- **Batches and `nextStartRow`:**
  - With no batch size the run is one transaction: any error rolls it back. Every extract and transform error is
    reported, and the first database error ends the loading.
  - With a batch size, a failed batch is rolled back and listed in `failedRanges`. `stopOnError` (the default) then
    ends the run as Failed, and the batch it was in is rolled back too.
  - `lastCommittedRow` is the last row of the last batch committed before any batch failed. `startRow` is the first row
    read, so a rerun from `nextStartRow` (`lastCommittedRow + 1`) loads no committed row again and every failed one.
    A run that committed nothing has its own `startRow` as `nextStartRow`. The `EtlRun` row keeps both.
  - **`rerun#EtlRun(runId)`** runs the run's saved mapping again from its `nextStartRow`. A file source reads the run's
    `sourceLocation`: the location as that run expanded it, so `${yesterday}` is still that run's day.
    - It is for the run's user or a `DATABASE_ADMIN` holder, and the new run is the caller's, with all of
      `run#EtlMapping`'s checks.
    - It refuses a run still pending or running, a check, a run done with no errors (nothing left), an inline or
      deleted mapping, and a run of uploaded content (`contentBytes` is not kept).
    - `get#EtlRun`'s `canRerun` says whether it would take the run.
- **Check** (`check=true`, or `check#EtlSample` for the first 200 rows) extracts and transforms only and never stops
  at an error. It reports:
  - each row's errors;
  - the foreign keys with no related record;
  - for an entity target, the diff against the database (the first 1,000 rows).
  - A parent that an earlier row of the file would create is reported missing, since a check writes nothing.
- **Allowed locations.** A file is read only from:
  - the user's own `dbresource://moqui/database/upload/<userId>/`;
  - the import root `dbresource://moqui/etl/`;
  - `runtime://import/`;
  - with `DATABASE_ADMIN`, any `dbresource://` location.
  - Anything else (`component://`, `file:`, `runtime://conf`, another user's uploads) is "Not available".
  - `${date}`, `${yesterday}`, `${now}` and `${nowMillis}` in a location are replaced as text, and nothing else is
    expanded. `load#SpreadsheetEntity` reads a location from the same places.
- **What a mapping may not reach:**
  - an identity-admin, secret-config or audit entity (the `moqui.etl` entities among them), or a view over one, as the
    source, target or lookup, or as the noun of an entity-auto service target;
  - a `delete#` service as a target;
  - a `UserAccount` password field, by name or as a renamed view alias, as a target or lookup field. An entity
    source's rows leave those fields out.
  - A SQL source needs `SQL_RUNNER_WEB` and writes a `DbaSql` audit row.
- **Owners:**
  - A mapping or value map is changed, copied over or deleted only by its owner or a `DATABASE_ADMIN` holder.
  - `get#EtlRuns` and `get#EtlRun` show the user's own runs, or every run with `DATABASE_ADMIN`.
- <a id="scheduling"></a>**Scheduling:** a `ServiceJob` that calls `run#EtlMapping` with a `mappingId`. The
  Mapping screen's Schedule makes one.
  - A cron run has no user signed in (see [Attack-Surface, Jobs](../security/Attack-Surface.md#jobs)), so the run signs
    in as the mapping's `ownerUserId` and then runs as that user. All of the owner's own checks apply: the Tools app,
    `GROOVY_SHELL_WEB`, `SQL_RUNNER_WEB`, the locations and the `ext:` gate.
  - The owner must be a `ServiceJobUser` of the job. An `ADMIN` may write both, so may schedule another user's saved
    mapping to run as that user; that is allowed on purpose.
  - The run reads the mapping's own source: the job's `location` and `contentBytes` parameters are ignored.
  - The owner signs in as any login does. A missing owner, an account that is disabled or terminated, and a password
    that must change or has expired refuse the run, with a logged error. So an expired password fails the owner's
    scheduled runs until it is changed. An inline mapping is refused.
  - The sign-in lasts for the one run: the previous identity (none) comes back after it, so the next `run#EtlMapping`
    in the same job signs in as its own mapping's owner.
  - A run-now of the job runs as its caller.
  - A job's run is done on the job's own worker, as with `wait`. Its `EtlRun.jobRunId` names the `ServiceJobRun`
    (`moqui_core::service::current_job_run_id`), and its `DbaEtlRun` audit row's detail names the `jobName`.
- **`load#SpreadsheetEntity`** keeps its parameters and runs as an inline mapping in the caller's transaction, stopping
  at the first error.

## Database

- **Overview** (`Database/Overview.xml`): the server, version, uptime and size, and what the Tools screens can do on
  it and why not the rest (`get#Capabilities`, probed once and kept five minutes). For example, "needs the pg_monitor
  role" or "pg_stat_statements is not loaded".
- **Activity** (`get#DatabaseActivity`):
  - **the sessions:** state, wait event, query and transaction start, and statement. Idle in transaction over a minute
    and running over ten seconds are marked.
  - **blocking chains** as trees, and this server's own sessions by their `application_name` (the instance id).
  - **index health** (`get#IndexHealth`): unindexed foreign keys, unused, duplicate and invalid indexes, and
    wraparound ages.
  - **Cancel and End** need `DATABASE_ADMIN`. A session of this server needs `force`.
- **Queries:**
  - the entity facade's statement stats of this instance (when `query-stats` is on), with the artifact that ran each
    statement;
  - the database's own stats (`pg_stat_statements`, or `performance_schema` digests).
  - Resetting either needs `DATABASE_ADMIN`.
- **Tables** (`get#TableStats`):
  - **what's shown:** estimated rows, sizes, dead rows, the last vacuum and analyze times, and scans, from one catalog
    query. An exact count is per row.
  - **maintenance, planned as jobs:** `VACUUM (ANALYZE)`, `ANALYZE` and `REINDEX TABLE CONCURRENTLY`, and on MySQL
    `ANALYZE TABLE` and `OPTIMIZE TABLE`.
  - `VACUUM FULL` and `CLUSTER` take an exclusive lock and need it allowed.
- **Jobs**, and a job's page with its statements, progress, blockers and Cancel.
- **Settings** (`Database/Settings.xml`, `get#DatabaseSettings`):
  - **the server's settings, read only:** `pg_settings` with source and pending restart, or `SHOW VARIABLES`.
  - **on the embedded cluster,** `shared_buffers`, `work_mem`, `maintenance_work_mem` and `effective_cache_size` also
    show their conf attribute and whether the running value differs from it. A restart of the server applies a changed
    conf value.
- **Connections** (`Database/Connections.xml`): the external databases (`find#`, `store#`, `delete#`,
  `test#DatabaseConnection`); see [External databases](#external-databases).
  - It is not in the menu. Overview links to it ("External databases") only when the server offers external
    databases to the user.
  - Saving replaces a connection whole, under the password-clearing rule.
- **Speed Test:** Java's entity speed test, as it was.

## External databases

A `moqui.database.DatabaseConnection` row is used as `db=ext:<connectionId>`. That works for Activity, Tables,
statement stats, capabilities, Settings, SQL and the grid's SQL source. It is never used for schema, jobs, session
cancel, table maintenance or entity sources.

- **Refused in production.** Every service that takes `ext:`, and every connection service, refuses unless the
  instance is not a production one and the user has `DATABASE_EXTERNAL`. The one error, "Not available", names
  neither cause. On production a connection row is neither read nor used, even when one was loaded.
- **Owners.** Only the owner or a `DATABASE_ADMIN` holder replaces, deletes or tests a connection; others may use it.
  The password is stored encrypted and never returned (`hasPassword`).
- **The password-clearing rule.** A store with no password keeps the stored one unless `dbType`, `host`, `port`,
  `userName`, `sslMode` or `sslRootCert` changed. Then it is cleared (`passwordCleared`) and must be entered again, so
  a changed host cannot be handed the old password.
- **TLS defaults:**
  - An empty `sslMode` is `verify-full` for a host that is not loopback, and no TLS for localhost.
  - `sslRootCert` is the path of a PEM file of the authorities to trust.
  - MySQL's mode names (`REQUIRED`, `VERIFY_CA`, `VERIFY_IDENTITY`) are taken too.
- **Read only by default.** A `readOnly` connection (the default) runs every statement in a read-only transaction and
  takes no changes.
- **Pools.** One per connection, closed after ten idle minutes, through the host's conf connection factory.

## Conf settings

- **`entity-facade@query-stats`** (default `false`; `true` in `MoquiDevConf.xml`): collect the entity facade's
  statement stats per instance for Database › Queries.
  - The stats are keyed by the statement with its literals replaced, and kept per artifact.
  - At most 5,000 statements and 20 artifacts each.
  - Scripts read them with `ec.entity.getQueryStatsList()` and clear them with `ec.entity.clearQueryStats()`.
- **`datasource@pg-stat-statements`** (default `false`; `true` for `transactional` and `content` in the dev conf):
  - **the embedded cluster** preloads `pg_stat_statements` and turns on `track_io_timing` when it starts;
  - **an external Postgres** must have it in `shared_preload_libraries`, or a warning says so;
  - `CREATE EXTENSION IF NOT EXISTS` runs after start, and a role that may not create it gets a warning;
  - it is ignored on MySQL, whose digests are in `performance_schema`.
- **The embedded cluster's sizes** on the `transactional` datasource:
  - `shared-buffers` (empty keeps the code's 128MB, Postgres's own default; `conf/MoquiLoadTestConf.xml` sets 256MB);
  - `work-mem`, `maintenance-work-mem` and `effective-cache-size` (empty keeps Postgres's defaults).
  - Each is a size such as `64MB`, and applies when this process starts the cluster. Database › Settings shows the
    conf value beside the running one.
- **`database_audit_days`** (property, 90): how long `DatabaseAudit` rows are kept. The `DatabaseAuditPurge` ServiceJob
  also removes expired SQL tokens.

```xml
<entity-facade query-stats="true">
    <datasource group-name="transactional" pg-stat-statements="true" shared-buffers="64MB" work-mem="8MB"/>
</entity-facade>
```

## Permissions

| Action | Needs |
| --- | --- |
| Open the Tools app and view activity, queries, tables, schema, settings and jobs | The Tools app's artifact authorization `TOOLS_APP` (`ADMIN`), checked by every service whatever calls it |
| Grid reads and saves, Browse, Export | The app, plus the entity's own authorization through the entity-auto services. Restricted entities and views over them are refused |
| ETL runs and checks, saving mappings, Data › Load, Snapshots | The app, authorized for all actions |
| ETL expressions | `GROOVY_SHELL_WEB` (sealed; seeded on no group) |
| Writing a DataDocument field expression (a `fieldPath` with `(`) | `DATABASE_ADMIN`, checked by `Security.eecas.xml` on every create and update; a seed load is not checked |
| SQL, the grid's SQL source, a SQL grid view, ETL from SQL | `SQL_RUNNER_WEB` (`ADMIN_ADV`) and all actions on the Tools app |
| Cancel or end a session, run DDL and maintenance jobs, reset stats, cancel another user's SQL, wider export and ETL locations, other users' runs | `DATABASE_ADMIN` (`ADMIN_ADV`) |
| External databases | `DATABASE_EXTERNAL` (`ADMIN_ADV`), and not a production instance |

`DATABASE_ADMIN` and `DATABASE_EXTERNAL` are seeded in `runtime/base-component/tools/data/ToolsSecurityData.xml`. They
are sealed permissions, beside `SQL_RUNNER_WEB` and `GROOVY_SHELL_WEB`: a `UserGroupPermission` row for one can be
written only by a caller who has it.

SQL and ETL from SQL read past the entity facade's authorization and field encryption; an encrypted column shows its
ciphertext. On the embedded cluster they run as the database superuser. That is why they need `ADMIN_ADV`
permissions.

The public demo replaces SQL, Data Load, Export and Snapshots, Activity, Queries, Connections and Speed Test with a
disabled screen (`runtime/dev-component/demo/MoquiConf.xml`).

## The audit

`moqui.database.DatabaseAudit` has a row for each:

- SQL statement (`DbaSql`, from the SQL screen, the grid's SQL source and ETL SQL sources);
- job (`DbaJob`), cancel (`DbaCancel`) and terminate (`DbaTerminate`);
- grid save (`DbaGridSave`: the entity and the counts, not the values);
- load (`DbaLoad`) and ETL run (`DbaEtlRun`);
- stats reset (`DbaStatsReset`).

Each row records the user, visit, instance, `db`, statement, rows, duration and error. It is written in a transaction
of its own, so it survives a rollback of what it records.

`DatabaseAudit`, `SqlRunToken`, `DatabaseConnection` and the `moqui.etl` entities are written only by their own
services. The generic tools, entity REST, request loads, ETL targets and every entity-auto service refuse them
(`AUDIT_ENTITY_KEYS`, `crates/moqui-entity/src/entity/sensitive.rs`).

## Not built

- The spreadsheet features and the other items deferred by the plan:
  - saving grid edits to an external database;
  - SQL on the embedded cluster as a role without superuser;
  - query stats gathered from every cluster member;
  - a load that skips failed rows.
  - Listed in [plans/Remaining-Work.md](../../plans/Remaining-Work.md).
- The MySQL and MariaDB job runner paths (lock timeouts, progress, `KILL QUERY` cancel) have not been run against a
  server. Their host tests run on Postgres.
