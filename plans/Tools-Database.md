# Tools: Database, Data and ETL

November 5, 2026 — design and implementation plan; not implemented. Database administration, the data grid and ETL
become the main part of the existing **Tools** app (`component://tools/screen/Tools.xml`, mounted as `tools`,
menu-index 99), whose screens are reorganized around them. They cover:

- the database administration tasks MR Moqui can do on the embedded Postgres, an external Postgres, MySQL 8 and
  MariaDB;
- a data grid (Tabulator) for viewing, editing, exporting and saving data from the entity facade, from SQL and from
  CSV and XLSX files;
- ETL mappings, which load files, SQL results and entity data into entities and services.

They replace Tools' database screens (Entity, Data View, Auto Screen) and System's DataDocument export, fixing the
parts of them that do not work today. Services, Status Flows and Artifact Stats stay as they are. The library
research is summed up in §11, and the spreadsheet features left for later are in §12.

Contents: [1 decisions](#1-decisions) · [2 what moves](#2-what-exists-and-where-it-goes) ·
[3 layout](#3-tools-layout) · [4 backends](#4-databases-and-what-each-allows) · [5 schema](#5-schema-indexes-and-keys) ·
[6 jobs](#6-jobs) · [7 performance](#7-performance) · [8 SQL](#8-sql) · [9 data grid](#9-the-data-grid) ·
[10 ETL](#10-etl-mappings) · [11 library](#11-the-grid-library) · [12 future](#12-future-spreadsheet-features) ·
[13 code placement](#13-where-the-code-goes) · [14 security](#14-security-and-audit) · [15 delivery](#15-delivery)

## 1. Decisions

1. **In Tools, not a new app.** Everything here goes under `component://tools/screen/Tools.xml`, which keeps its
   mount and its artifact group, `TOOLS_APP`, granted to `ADMIN` (`tools/data/ToolsSecurityData.xml`). Tools' top
   level is grouped: Dashboard, Data, SQL, Schema, ETL, Database (the server), Services, Status Flows and Artifact
   Stats (§3).
2. **Grid library: Tabulator** (MIT, a plain global script). Formulas and the IronCalc engine come later (§12).
3. **ETL is the load path.** Saved mappings run through `moqui_lib::etl` and load into entities or services. Files,
   SQL results and entity finds are the sources. `load#SpreadsheetEntity` is the first form of this, and becomes a
   call of an unsaved mapping.
4. **No external databases in production.** Connections to databases the entity groups do not use are refused
   where `moqui_core::web::is_production()` holds, with one vague error, as the Dev AIDE's reach is
   (`moqui_devkit::project::cache::requested`). Dev and test instances allow them.
5. **`pg_stat_statements` is a per-datasource setting.** The `datasource` attribute `pg-stat-statements` is `false`
   in `MoquiDefaultConf.xml` and turned on for the dev datasources in `runtime/conf/MoquiDevConf.xml`.
6. **SQL that changes data is allowed,** as in Java, to holders of `SQL_RUNNER_WEB`. Each run needs a confirmation
   and writes an audit row.
7. **Transactions.** Saving grid edits is one transaction: every change commits, or none does. A load (an import or
   an ETL run) is one transaction by default. Large loads can set a batch size, which commits every N records.
8. **Moved screens leave nothing behind.** The old Tools screens and System's DataDocument Export are deleted, with
   no redirect screens: before 0.1.0 nothing outside these repositories links to them. "These repositories" include
   the component repositories under `runtime/component/` (mr-mantle, mr-boss, mr-marble), which do link to them.
   Links and tests are updated (§2.2).
9. **Out of scope:**
   - roles and grants;
   - setting up replication;
   - restoring a backup;
   - the database confs that have no driver (DB2, Oracle, SQL Server; `MoquiDefaultConf.xml` still lists them);
   - schema migrations (none before 0.1.0).

## 2. What exists and where it goes

### 2.1 Screens

| Today | State | In Tools |
| --- | --- | --- |
| `Tools/Entity/SqlRunner.xml` | Works. One statement per run in a text area, results in a table. The server side (`run_sql`, `crates/moqui-entity/src/entity/info.rs:742`) reads through a cursor, stops at the limit and checks the read fence. | **SQL** (§8) |
| `Tools/Entity/SqlScriptRunner.xml` | Works. Splits on `;` and shows no result sets. | Part of **SQL**: run the script |
| `Tools/Entity/QueryStats.xml` | Always empty: `ec.entity.getQueryStatsList` returns `[]` (`crates/moqui-core/src/script/runtime/tools_host.rs:122`). `entity-facade@query-stats` is read by nothing, although `MoquiDevConf.xml:106` sets it. | **Database › Queries** (§7.1) |
| `Tools/Entity/TableStats.xml` | Works, slowly: `count()` on every entity, one after another | **Database › Tables** (§7.4), estimates from the catalog |
| `Tools/Entity/DataEdit/EntityList.xml` | The list works. Check Tables, Drop FKs, Create FKs, Create Indexes and Liquibase Init XML call `ec.entity.entityDbMeta` / `getEntityDbMeta()` methods that have no script binding (no match in `crates/`), so they fail. | List: **Schema › Entities**. Buttons: **Schema › Indexes and Keys** as jobs (§5, §6). Liquibase: replaced by Download DDL |
| `Tools/Entity/DataEdit/EntityDetail.xml` | Works except Check Table (`entityDbMeta.forceCheckTableRuntime`, unbound) | **Schema › Entity** |
| `Tools/Entity/DataEdit/EntityDataFind.xml`, `EntityDataEdit.xml` | Work: a form-list per entity with CSV/XLSX buttons, and a single-record form | **Data › Grid** with an entity source (§9), and its row form |
| `Tools/Entity/DataImport.xml` | Works: entity XML/JSON/CSV text or location, with the check diff and row selection | **Data › Load** (entity data files), and **Data › Grid** with a file source plus ETL for column-mapped files (§10) |
| `Tools/Entity/DataExport.xml` | Works | **Data › Export** |
| `Tools/Entity/DataSnapshot.xml` | Works except its Drop FKs and Create FKs (unbound) | **Data › Snapshots**. Its foreign key buttons become an import option that runs a drop-then-create job (§6) |
| `Tools/Entity/SpeedTest.xml` | Works | **Database › Speed Test**, moved as is |
| `Tools/DataView/*` (`FindDbView`, `EditDbView`, `ViewDbView`) | Work: the `DbViewEntity` builder, with CSV | **Data › Views**. The builder moves; View opens the grid on the `DbViewEntity` |
| `Tools/AutoScreen/*` | Works: a master record and its related records. Its menu templates are `tools/template/AutoScreenMenu.{html,qvt}.fancy` | **Data › Browse**, moved with its templates. The grid's row action "Related" opens it |
| `System/DataDocument/Export.xml`, and the `DocumentExport.json` transition that it (line 19) and `Edit/ViewDataDocument.xml` (line 21) both have | Work. Four screens in other repositories `transition-include` `DocumentExport.json` from `ViewDataDocument.xml` (§2.2) | **Data › Export**, with DataDocument as an export kind. `Data/Export.xml` becomes the one home of `DocumentExport.json`, and the includes point there |
| `System/DataDocument/Edit/ViewDataDocument.xml` (the Report Builder's list) | Works: a form-list on `DataDocument.<id>` with saved finds | Replaced by **Data › Grid** on `DataDocument.<id>`, with saved grid views (§9.6). The Edit screen's View tab links there |
| `Tools/dashboard.xml`, the database links of four boxes: "Browse and edit data" (lines 35–40), "Import and export" (41–46), the SQL links of "Services and queries" (52–53), and Table statistics, Query statistics and Speed test in "Inspect and diagnose" (59–61) | Links | Regrouped by section: Data, SQL (still shown on `canSqlRunner`, line 29), Schema, ETL and Database, beside Services and the API reference box |
| `System/dashboard.xml` "Documents and feeds" box (line 130), its Export link (line 134) | Link | Points to **Tools › Data › Export** |

These stay where they are:

- **Tools:** Service (Run, Load Runner, Reference, Detail), StatusFlows, ArtifactStats, and the API reference box on
  its dashboard.
- **System:** the DataDocument definitions (Find, Edit, Fields, Index), which configure search feeds and reports.
  Also AuditLog, EntitySync, Instance, Cache, Resource and the rest.

### 2.2 References to update when a screen moves

Found by grep for the screen paths; repeat the grep after the move. The component repositories under
`runtime/component/` are gitignored here, so the grep must not honour ignore files (`rg --no-ignore`, or
`/usr/bin/grep -r`); the references a plain grep missed are all in them.

- **Framework client specs** (`runtime/base-component/framework/test/client/`):
  - `SecurityScreens.spec.js`: SqlRunner at 181 and 184, DataImport at 95 and 352, DataSnapshot at 229, 242, 258 and
    259.
  - `SecurityAccess.spec.js:95` and `SecurityHeaders.spec.js:220`.
  - `Java-Port-Catalog.md` holds only the test names, no paths, so it does not change.
- **Host integration tests:**
  - `crates/moqui-host/tests/it/tools_screens.rs`: the screen list at 93–115, SpeedTest at 268, the paths at 309–334
    and 440–558, `run_sql` at 632 with its `/Tools/Entity/{screen}.html`, and the `SqlRunner` and `SqlScriptRunner`
    names it passes at 657–700.
  - `tools_services.rs:214` and `qvt_report.rs:96`.
  - Comments that name `SpeedTest.xml`: `crates/moqui-host/Cargo.toml:279`, `crates/moqui-host/tests/entity_speed.rs:36`,
    `service_speed.rs:15` and `:33`.
- **Tools regression spec:** `runtime/base-component/tools/test/DataView.spec.js` renders `DataView/EditDbView` on
  the Tools root. It changes to `Data/Views/EditDbView`.
- **Relative links between screens that move:** `Tools/AutoScreen/AutoFind.xml:23` (to EntityDetail),
  `Tools/Entity/DataEdit/EntityDetail.xml:24` and `EntityList.xml:21` (to AutoFind).
- **Demo lock-down:**
  - `runtime/dev-component/demo/MoquiConf.xml:14–21` stubs six screens under `Tools/Entity.xml` (DataImport,
    DataExport, DataSnapshot, SpeedTest, SqlRunner, SqlScriptRunner). The stubs move to `Tools.xml` (Sql),
    `Tools/Data.xml` (Load, Export, Snapshots), `Tools/Database.xml` (SpeedTest, Connections), and every other action
    screen that writes.
  - `runtime/dev-component/demo/test/Demo.spec.js:24–29` and `:73` change with it, and so does its menu check at
    82–93, which renders `menuData/apps/tools/Entity` and expects "Query Stats".
  - The demo runs with `instance_purpose=production`, so external databases are refused there anyway. Connections
    is stubbed as well, as the other sharp tools are.
  - `runtime/dev-component/demo/README.md:42` names the Tools views.
- **Gallery:** `runtime/dev-component/example/screen/Gallery/Settings.xml` 54, 56 and 62 link to
  `//apps/tools/AutoScreen/AutoFind`.
- **Acceptance spec:** `runtime/base-component/basic-comms/acceptance/client/EncryptedChatFlow.spec.js:337` posts to
  `/apps/tools/Entity/DataEdit/EntityDataEdit/update`. It moves to the row form's transition, and the spec is run
  once after the change, since we changed it.
- **Dev AIDE:** `runtime/dev-component/dev-aide/screen/devstatic/DevAideEntity.js:396` `dataEditUrl()` (today
  `/qapps/tools/Entity/DataEdit/EntityDataFind`, used at :919) changes to `/qapps/tools/Data/Grid?entityName=…`. Its
  entity view's "Data" section (`plans/design-dev-aide.md` §8.1) links there too.
- **Other component repositories** (each a commit in its own repository):
  - mr-boss `screen/Insights/Reports.xml:20` and `screen/Insights/ReportBuilder/DatasetView.xml:20`, and mr-marble
    `screen/SimpleScreens/SimpleReport/ViewReport.xml:20` and `QuickViewReport.xml:20`: each `transition-include`s
    `DocumentExport.json` from `System/DataDocument/Edit/ViewDataDocument.xml`. They change to
    `component://tools/screen/Tools/Data/Export.xml`.
  - mr-mantle `data/MantleDocumentData.xml` (lines 106, 143, 215, 238, 264, 296, 325, 349, 373) and
    `MantleDocumentWorkData.xml:46`: the DataDocument `linkUrl` `/apps/tools/AutoScreen/AutoEdit/AutoEditMaster?…`
    changes to `/apps/tools/Data/Browse/AutoEdit/AutoEditMaster?…`. The checks of those links,
    `crates/moqui-host/tests/it/mantle_search.rs:464` and mr-mantle `test/search/MantleSearch.spec.js:213`, match only
    "AutoEditMaster"; confirm they still pass.
  - mr-marble `MIGRATION-STATUS.md:73`, and mr-boss `plan/research-admin.md` and `plan/research-insights.md`, name
    old paths in text.
- **Docs and plans:**
  - `docs/security/Attack-Surface.md:253` (the permission table row for SQL Runner) and `:810` (the tools the demo
    stubs), and the new rows of §14.
  - `docs/security/Proof-Tests.md:57` and `:78` name the SQL Runner proof.
  - `plans/Monitoring.md:363` and `plans/Profiling.md:12` and `:393` name `Tools/Entity/SpeedTest.xml`.
  - `plans/UI-Refinement.md`'s screen inventory (lines 1652–1916) names the Tools screens.

## 3. Tools layout

The root, `Tools.xml`, stays as it is: `<subscreens default-item="dashboard"/>` and a popup `subscreens-panel`. Under
`runtime/base-component/tools/screen/Tools/`:

```
dashboard.xml                   today's link boxes regrouped by section (§2.1), the selected database's health, the last jobs
Data.xml
  Data/Grid.xml, Grid.qvue      the data grid (§9); a tui render mode in the same screen (§9.3)
  Data/Browse/…                 Tools/AutoScreen, moved
  Data/Views/…                  Tools/DataView, moved
  Data/Export.xml               entity, DataDocument and snapshot exports (DataExport, System's DataDocument Export);
                                the one home of DocumentExport.json
  Data/Load.xml                 entity data files and text with the check diff (DataImport)
  Data/Snapshots.xml            DataSnapshot, moved
Sql.xml, Sql.qvue               the SQL editor and results (§8); the TUI renders Sql.xml's form, as SqlRunner.xml does now
Schema.xml
  Schema/Entities.xml           the entity list (from EntityList), with each table's size and estimated rows
  Schema/Entity.xml             one entity: fields and the table's columns, indexes, keys, check table, DDL (from EntityDetail)
  Schema/IndexesKeys.xml        the model against the database; create and drop as jobs (§5)
Etl.xml
  Etl/Mappings.xml, Etl/Mapping.xml, Etl/Mapping.qvue, Etl/Runs.xml, Etl/Run.xml, Etl/ValueMaps.xml   (§10)
Database.xml                    the database server
  Database/Overview.xml         the default: version, uptime, size, capabilities (§4.1), the backup link
  Database/Activity.xml         sessions, locks and blocking chains; cancel, terminate
  Database/Queries.xml          statement stats from three sources; Explain (§7)
  Database/Tables.xml           sizes, row estimates, dead rows, vacuum and analyze times; maintenance as jobs
  Database/Jobs.xml, JobDetail.xml   DDL and maintenance jobs: state, progress, lock waits, cancel (§6)
  Database/Settings.xml         server settings; the embedded cluster's editable ones (§4.4)
  Database/Connections.xml      external databases (refused in production; §4.3)
  Database/SpeedTest.xml        Tools/Entity/SpeedTest, moved as is
Service.xml, Service/…          unchanged
StatusFlows.xml, ArtifactStats.xml   unchanged
```

`Tools/Entity.xml`, `Tools/DataView.xml` and `Tools/AutoScreen.xml` and the screens under them are deleted (decision
8).

**The database selector.** Each screen of Data, SQL, Schema, ETL and Database takes a `db` parameter:
`group:<groupName>` for an entity group's database, or `ext:<connectionId>` for an external one. Groups that share a
database appear once, named by their groups. This uses the same per-database test as the foreign key rule
(`ensure_fks` and `warn_cross_database`, `crates/moqui-entity/src/entity/meta.rs`, through
`EntityFacade::same_database`). The last choice is kept as the user preference `database.selected`
(`ec.user.setPreference`, as `apps.xml`'s `setPreference` transition does). Screens that work on entities (Schema,
Data, ETL targets) list only the groups' databases. Services, Status Flows and Artifact Stats take no `db`.

## 4. Databases and what each allows

### 4.1 Capabilities

`org.moqui.impl.DatabaseServices.get#Capabilities` (`db`) probes once and caches the answer for five minutes. Screens
show what is available and say why the rest is not, such as "needs the pg_monitor role" or "pg_stat_statements is
not loaded".

MySQL and MariaDB are the `mysql` and `mysql8` database confs; both were loaded in full on MySQL 8.4 and MariaDB 11.4
(`archive/MySQL-Issues.md`). Their monitoring differs, so they have a column each.

| | Embedded Postgres | External Postgres | MySQL 8 | MariaDB |
| --- | --- | --- | --- | --- |
| Version, uptime, size | `version()`, `pg_postmaster_start_time()`, `pg_database_size` | Same | `@@version`, `Uptime` status, `information_schema.TABLES` | Same as MySQL |
| Statement stats in the database | `pg_stat_statements`, when `pg-stat-statements` is on (§7.2) | When the provider preloads it (RDS, Cloud SQL and Azure do); reading other roles' text needs `pg_read_all_stats` | `performance_schema.events_statements_summary_by_digest`, on by default | The same table, but `performance_schema` ships off and only a server restart turns it on |
| Activity, cancel | `pg_stat_activity`, `pg_cancel_backend`, `pg_terminate_backend`: everything (the app connects as the bootstrap superuser) | Own sessions, or every session with `pg_signal_backend` and `pg_read_all_stats` | `performance_schema.processlist` / `SHOW PROCESSLIST`, `KILL QUERY`, `KILL` (own threads, or all with `CONNECTION_ADMIN`) | `information_schema.PROCESSLIST`, `KILL QUERY`, `KILL` (all with the `CONNECTION ADMIN` privilege or `SUPER`) |
| Lock waits | `pg_blocking_pids`, `pg_locks` | Same | `performance_schema.data_lock_waits`, `sys.innodb_lock_waits` | `information_schema.INNODB_LOCK_WAITS` with `INNODB_TRX` |
| Table sizes and estimates | `pg_class.reltuples`, `pg_total_relation_size`, `pg_stat_user_tables`; exact bloat with `pgstattuple` | Same, with a bloat estimate query when `pgstattuple` is absent | `information_schema.TABLES` (`TABLE_ROWS`, `DATA_LENGTH`, `INDEX_LENGTH`, `DATA_FREE`) | Same as MySQL |
| Index usage | `pg_stat_user_indexes`, `indisvalid` | Same | `sys.schema_unused_indexes`, `sys.schema_redundant_indexes` | The same `sys` views (10.6 and later); unused indexes need `performance_schema` on |
| Index online | `CREATE INDEX CONCURRENTLY`, progress in `pg_stat_progress_create_index` | Same | `ALGORITHM=INPLACE, LOCK=NONE`. Progress in `performance_schema.events_stages_current` when the `stage/innodb/alter%` instruments are on | `ALGORITHM=INPLACE, LOCK=NONE`. Progress in the `PROGRESS` column of `information_schema.PROCESSLIST` |
| FK with a light lock | `ADD CONSTRAINT … NOT VALID`, then `VALIDATE CONSTRAINT` | Same | In place only with `foreign_key_checks=0`, which does not validate; otherwise the table is copied | Same as MySQL |
| Maintenance | `VACUUM (ANALYZE)`, `ANALYZE`, `REINDEX … CONCURRENTLY`; progress views for vacuum and analyze | Same | `ANALYZE TABLE`, `OPTIMIZE TABLE` | Same as MySQL |
| Integrity | `amcheck` (`bt_index_check`) | If installed | `CHECK TABLE` | Same as MySQL |
| Settings | `pg_settings`; a set can be changed (§4.4) | `pg_settings`, read only | `SHOW VARIABLES`, read only | Same as MySQL |
| Backups | `backup#EmbeddedDatabase` (`crates/moqui-drivers/src/backup.rs`), linked from Database › Overview | The provider's | — | — |

The embedded archive already has every extension used here: `pg_stat_statements`, `pgstattuple`, `amcheck`,
`pg_buffercache` and `pg_visibility`. Check with `ls runtime/install/postgres/18.6.0/share/extension`.

### 4.2 The catalog queries

The SQL text of each probe and catalog query is a constant, per dialect, in one of two places. The queries the schema
comparison needs (indexes, keys, invalid indexes, column types; §5.1) are in `crates/moqui-db/src/sql/`, beside the
DDL builders, because `EntityDbMeta::compare` in `moqui-entity` cannot depend on a crate above it. The rest are in
`catalog.rs` of the new crate (§13). Each query has a test that parses it against a live embedded cluster, and against
MySQL and MariaDB where `pg_external.rs`-style tests can reach a server. Queries run through the entity group's connection
(`run_sql`, `info.rs:742`), or through an external connection (§4.3). Their results are maps, not entity values.
These are the queries that need care:

- **Foreign keys with no index on their columns.** Postgres: `pg_constraint` (`contype = 'f'`) whose `conkey` is not
  a prefix of any `pg_index.indkey` of the table. MySQL: InnoDB makes one itself, so this check is Postgres only.
- **Unused indexes.** `pg_stat_user_indexes.idx_scan = 0`, leaving out unique and primary key indexes. Show the
  time since `pg_stat_reset` or since the server started, since the counters start then.
- **Duplicate indexes.** The same table and the same leading columns, as pghero checks.
- **Invalid indexes.** `NOT indisvalid`: a failed `CONCURRENTLY` leaves one.
- **Blocking chains.** `pg_blocking_pids(pid)` for each waiting session, joined back to `pg_stat_activity`.
- **Wraparound.** `age(datfrozenxid)` per database, and the oldest `age(relfrozenxid)` per table.

### 4.3 External databases (not in production)

- **Entity:** `moqui.database.DatabaseConnection`:
  - `connectionId`, `description`, `dbType` (`postgres`, `mysql`, `mysql8`, as `database-list` names them; MariaDB
    uses `mysql`);
  - `host`, `port`, `databaseName`, `userName`, `password` (`encrypt="true"`), `sslMode`, `sslRootCert`;
  - `readOnly` (Y by default), `ownerUserId`.
- **Connecting:** through `moqui_drivers::conf_driver` (`crates/moqui-drivers/src/lib.rs:64`), which core reaches
  as the conf connection factory (`install_conf_drivers`, `lib.rs:130`; `conf_db_of`, `tools_host.rs:2063`). One
  driver per connection is kept for ten idle minutes. A `readOnly` connection runs every statement in a read-only
  transaction.
- **What works on one:**
  - Activity, Queries (the database's own stats), Tables, Settings;
  - SQL, the grid on a SQL source, and ETL from a SQL source.
  - No entity model applies, so Schema is not offered, and the grid does not save to it in the first cut (§12).
- **The gate:** every service that takes `db=ext:…`, and the Connections screen, first checks that
  `moqui_core::web::is_production()` does not hold and that the user has the `DATABASE_EXTERNAL` permission. Failing
  either gives one error that names neither cause ("Not available"), as `moqui_devkit::project::cache::requested`
  (`crates/moqui-devkit/src/project/cache.rs:539`) does with `NOT_AVAILABLE`. This holds for the stored rows too: on a
  production instance a connection row cannot be read or used, even when one was loaded.

### 4.4 Settings

- **Every database:** a read-only list, filterable by name. Postgres shows `pg_settings` with `source` and
  `pending_restart`; MySQL shows `SHOW VARIABLES`.
- **The embedded cluster:**
  - Settings come from code (`settings()`, `crates/moqui-drivers/src/embedded.rs:717`: `shared_buffers` 16MB for the
    entity cluster and 8MB for the sim cluster, `max_connections` from `StartOptions`, `summarize_wal`, …).
  - A set of them gets `datasource` conf attributes, so they live in the conf and not in an `ALTER SYSTEM` file that
    the conf would not show: `shared_buffers`, `work_mem`, `maintenance_work_mem`, `effective_cache_size`.
  - The Settings screen shows each with its conf attribute and says a restart applies it. It does not write the conf.
  - This is the last step (§15). Raising `shared_buffers` from 16MB is a separate decision and is not part of this plan.

## 5. Schema: indexes and keys

### 5.1 The comparison

`EntityDbMeta::compare(names) -> SchemaDiff`, a new method in `crates/moqui-entity/src/entity/meta.rs`. For each
entity (view entities expand to their members, as `check`, `meta.rs:647`, does), it reports:

- missing table; missing, extra and differently typed columns (`table_columns`, `meta.rs:415`, with the column
  types read too);
- indexes: missing, extra and invalid. The expected set is what the DDL builders would make: each entity's `index`
  elements (`create_index`), and, when the dialect's `use_foreign_key_indexes` is set, one index per foreign key
  (`create_fk_index`, as `meta.rs:1159` makes them). An index is extra only when it is in neither, and it is not the
  index behind a primary key or unique constraint, or the index InnoDB makes for a foreign key;
- foreign keys: missing (from `type="one"` relationships, by the per-database rule of `ensure_fks`) and extra;
- foreign key columns with no index, on Postgres when the dialect does not make foreign key indexes. When it does, a
  missing one is already a missing index.

Indexes and keys are read from the catalog in one query per database, not one per table. That means `pg_index` with
`pg_class` and `pg_constraint` on Postgres, and `information_schema.STATISTICS`, `TABLE_CONSTRAINTS` and
`KEY_COLUMN_USAGE` on MySQL and MariaDB. Names are compared the way the DDL builder makes them: `shrink_name`
(`crates/moqui-db/src/sql/ident.rs:134`), `foreign_key_name` and `foreign_key_index_name`
(`crates/moqui-db/src/sql/statement.rs:753`, `:776`), and for MySQL foreign keys the columns, not the name
(Parity-Diffs P3-11).

The Dev AIDE's entity view (`plans/design-dev-aide.md` §8.1, "The table") uses this same comparison and does not
build its own.

### 5.2 Statements

- **The plan:** a selection (one row, a table's rows, every missing index) becomes a DDL plan built from the
  existing statement builders in `crates/moqui-db/src/sql/statement.rs` (`create_index` :622, `create_foreign_key`
  :670, `create_fk_index` :706), with the online forms:
  - Postgres:
    - `CREATE INDEX CONCURRENTLY IF NOT EXISTS`, `DROP INDEX CONCURRENTLY IF EXISTS`;
    - a foreign key as `ADD CONSTRAINT … NOT VALID` followed by `VALIDATE CONSTRAINT`;
    - an invalid index as `DROP INDEX CONCURRENTLY` then create again.
  - MySQL and MariaDB: `ALTER TABLE … ADD INDEX …, ALGORITHM=INPLACE, LOCK=NONE`, and `ADD CONSTRAINT … FOREIGN KEY`.
- **Before running:** the screen always shows the plan, and the job runs it as shown (§6).
- **Drop all keys.** "Drop all foreign keys" (Java's `dropAllForeignKeys`, used before a large load) is a selection
  of every known key. Data › Snapshots and Data › Load offer "drop foreign keys first and create them after" as a
  job around the load.
- **Download DDL** replaces Liquibase Init XML: the `CREATE TABLE`, index and foreign key statements of the selected
  entities as a `.sql` file, in the database's dialect, from the same builders.
- **Check tables** runs `check_tables_ddl` (`meta.rs:113`) and shows the DDL it ran (`TableCheck`). For one entity it
  runs the same with that name, replacing `forceCheckTableRuntime`. `forget_tables` (`meta.rs:520`) is called first,
  as `forceCheckExistingTables` was.

## 6. Jobs

DDL and maintenance can take minutes or hours on a large table. They run as jobs, not inside an HTTP request.

**Entities**, in `runtime/base-component/framework/entity/DatabaseEntities.xml`, package `moqui.database`:

- `DatabaseJob`: `jobId`, `db`, `description`, `kindEnumId` (DbjDdl, DbjMaintenance, DbjLoadWrap), `statusId`
  (DbjsPending, DbjsRunning, DbjsDone, DbjsFailed, DbjsCancelled), `userId`, `instanceId`, `createdDate`,
  `startTime`, `endTime`, `lockTimeoutMs`.
- `DatabaseJobStatement`: `jobId`, `statementSeqId`, `statementText`, `dependsOnSeqId` (VALIDATE depends on its NOT
  VALID), `statusId`, `backendPid` (Postgres pid or MySQL connection id), `startTime`, `endTime`, `rowsAffected`,
  `errorText`.

**Running a job** (`run#DatabaseJob`, called async on the worker pool):

- Each statement runs on a connection of its own, outside any transaction. `CREATE INDEX CONCURRENTLY` and `VACUUM`
  refuse to run inside one.
- First the runner records the backend id (`SELECT pg_backend_pid()` or `SELECT CONNECTION_ID()`).
- It clears the pool's `statement_timeout` (`SET statement_timeout = 0`; the driver's session options may set one,
  `crates/moqui-drivers/src/postgres.rs:63`).
- **Lock timeout, for statements that take a strong lock:** `ADD CONSTRAINT … NOT VALID`, other `ALTER TABLE`, a
  plain `DROP INDEX`, `VACUUM FULL`, `CLUSTER`, and MySQL DDL (`lock_wait_timeout`). The runner sets `lock_timeout`
  from the job (default 5 s). Such a statement waiting on a busy table then gives up, instead of queuing every other
  query on the table behind its lock request. It is retried three times, with a pause.
- **No lock timeout for the `CONCURRENTLY` statements** (`CREATE INDEX`, `DROP INDEX`, `REINDEX`) and for `VALIDATE
  CONSTRAINT`. Their locks do not block reads or writes, and `CONCURRENTLY` waits for every older transaction to
  finish, so a short timeout would fail them on any busy database. Their waits show in the progress (below).
- **Retrying a failed `CREATE INDEX CONCURRENTLY`.** A failure or cancel leaves an invalid index of that name, and
  `IF NOT EXISTS` would then skip the create. A retry, or a rerun of the job, first runs `DROP INDEX CONCURRENTLY IF
  EXISTS` when the index is invalid.
- On a failure the error is recorded and the next independent statement runs, as the failure-tolerance rule asks. A
  statement whose `dependsOnSeqId` failed is skipped and marked so.
- When the job ends, a notification goes to its user (topic `DatabaseJob`) with the counts. Jobs that change many
  tables, such as "create every missing index", also log a summary line.

**Progress.** `JobDetail` polls `get#DatabaseJobStatus` every two seconds.

- Postgres:
  - the running statement's `pg_stat_progress_create_index` row (phase, `blocks_done` of `blocks_total`,
    `tuples_done` of `tuples_total`, `lockers_done` of `lockers_total`);
  - `pg_stat_progress_vacuum` and `_analyze` for maintenance;
  - `pg_blocking_pids` for the job's pid, joined to `pg_stat_activity`. This gives "waiting for pid 1234, idle in
    transaction for 40 min, `UPDATE …`", with Cancel and Terminate on the blocker.
- MySQL: `events_stages_current` (`WORK_COMPLETED` of `WORK_ESTIMATED`) through `performance_schema.threads` by
  `PROCESSLIST_ID`, and `sys.innodb_lock_waits`.
- MariaDB: the `STAGE`, `MAX_STAGE` and `PROGRESS` columns of `information_schema.PROCESSLIST`, and
  `information_schema.INNODB_LOCK_WAITS`.

**Cancel** works from any cluster member, since the backend id is recorded: `pg_cancel_backend(pid)` or
`KILL QUERY id`, run on a fresh connection. On the instance that runs the job, the driver's canceller is used instead
(`DbConnection::canceller`, `postgres.rs:748`, `mysql.rs:560`).

**After a restart.** When an instance starts, any job left Running under its `instanceId` is marked Failed ("the
server stopped"). Postgres keeps running a `CREATE INDEX CONCURRENTLY` whose client went away, until it finishes or
fails. If one finishes, the next comparison shows a valid index; if it fails, an invalid one.

## 7. Performance

### 7.1 Statement stats from the entity facade

Java's Query Stats, done. What our stats show that `pg_stat_statements` cannot: the screen, service or REST path that
ran each statement.

- **Turned on by `entity-facade@query-stats`.** The XSD has the attribute (`moqui-conf-3.xsd:1522`), the default
  conf sets it to `false` (`MoquiDefaultConf.xml:1300`) and the dev conf to `true` (`MoquiDevConf.xml:106`). The
  facade reads it once at init.
- **Collected in `trace`** (`crates/moqui-entity/src/entity/exec.rs:207`), the one place every statement passes
  through, next to `STATEMENT_COUNT` and the slow-statement warning. When stats are off, it costs one relaxed load.
- **Key:** the statement text. Entity finds are parameterized already. For literal SQL from `run_sql`, the text goes
  through `redact_sql` (`exec.rs:225`), which replaces quoted strings and drops comments, and then through a small
  key normalizer that also replaces numeric literals and collapses whitespace. Without that second step,
  `WHERE id = 5` and `WHERE id = 6` would be two rows.
- **Kept per statement:** hits, errors, minimum, maximum, total, sum of squares (for the standard deviation), the
  first entity name, and counts by artifact. The artifact is the top of the artifact stack, through a new
  `artifact_name` field on the `EntityHooks` struct of hook functions (`crates/moqui-entity/src/hooks.rs:51`). Core
  fills it in `crates/moqui-core/src/entity_hooks.rs` from the context's artifact stack
  (`ExecutionContext::artifact_stack_names`, `crates/moqui-core/src/context/factory.rs:1403`, has the whole stack).
  The existing `trace_statement` hook (`hooks.rs:308`) opens the telemetry span of the same statement; the two stay
  separate, since stats must not depend on tracing being on.
- **Bounded:** at most 5,000 statements and 20 artifacts each. Past that, new statements count in one "(other)"
  row. The map is sharded by hash so threads rarely contend.
- **Scope:** per instance. The Queries screen names the instance. Gathering every member's stats goes over the
  cluster's request path later (§12).
- **Script access:** `ec.entity.getQueryStatsList(orderBy, entityFilter, sqlFilter)` and `ec.entity.clearQueryStats()`
  replace the stubs at `tools_host.rs:122–123`, keeping Java's names.

### 7.2 `pg_stat_statements`

- **The setting:** a new `datasource` attribute, `pg-stat-statements` (`boolean-expandable`, default `false`), in
  `moqui-conf-3.xsd` beside `runtime-add-fks` (line 1768).
  - It is parsed in `crates/moqui-entity/src/entity/datasource.rs` beside `runtime_add_fks` (about line 273), into
    `DatasourceConf.pg_stat_statements`.
  - `MoquiDefaultConf.xml`'s `transactional` and `content` datasources (lines 1341 and 1351) get
    `pg-stat-statements="false"`.
  - `MoquiDevConf.xml` turns it on with
    `<datasource group-name="transactional" pg-stat-statements="true"/>` and the same for `content`. It has no
    datasource elements today. The conf merges datasources by `group-name` and keeps the default's `inline-jdbc` or
    `jndi-jdbc` child when the override has none (`merge_entity_facade`, `crates/moqui-core/src/context/conf.rs:443`).
- **Embedded:**
  - When any embedded datasource has the setting on, `open_embedded` (`crates/moqui-drivers/src/entity_db.rs:193`)
    passes a new field of `StartOptions` (`embedded.rs:70`).
  - `settings()` (`embedded.rs:717`) then adds `shared_preload_libraries = 'pg_stat_statements'` and
    `track_io_timing = on`. `shared_preload_libraries` is postmaster-level and applies when this process starts the
    cluster, as the comment on `summarize_wal` says. `track_io_timing` is not, but is set in the same place.
  - After start, `CREATE EXTENSION IF NOT EXISTS pg_stat_statements` runs in each database whose datasource has the
    setting on, as the bootstrap superuser.
  - The sim and search clusters leave it off.
- **External Postgres:**
  - At startup, `SHOW shared_preload_libraries` must include it; otherwise a warning says the server needs it.
  - Then `CREATE EXTENSION IF NOT EXISTS`. A role that may not create it gets a warning, not an error.
- **MySQL:** the setting is ignored with a debug line. Statement digests are on with `performance_schema`.
- **Queries screen:** the entity facade's stats (§7.1) and the database's own rows. Postgres shows `calls`,
  `total_exec_time`, `mean_exec_time`, `rows`, `shared_blks_hit` and `shared_blks_read`, and `temp_blks_written`.
  MySQL shows `COUNT_STAR`, `SUM_TIMER_WAIT`, `SUM_ROWS_EXAMINED`, `SUM_NO_INDEX_USED`. Reset needs
  `DATABASE_ADMIN`: `pg_stat_statements_reset()`, or `TRUNCATE` of the digest table.

### 7.3 Activity

- **The list:** sessions with their state, wait event, query start, transaction start and statement text. Idle in
  transaction over a minute and running over ten seconds are marked. Blocking chains are shown as a tree.
- **Actions:** Cancel and Terminate, with a confirmation. Both need `DATABASE_ADMIN`.
- **The app's own sessions:** new in the drivers (nothing sets it now), each pooled connection sets
  `application_name` (Postgres) or a connection attribute (MySQL) to the instance id. Activity can then mark the
  app's own sessions, and tell one cluster member's from another's.

### 7.4 Tables

- **What's shown:** estimated rows, total, table and index size, dead rows, the last vacuum, autovacuum, analyze
  and autoanalyze, and sequential against index scans, from one catalog query.
- **Exact counts:** a "count" action per row, so the screen does not run hundreds of `count()` as `TableStats.xml`
  does.
- **Actions,** as jobs (§6): `VACUUM (ANALYZE)`, `ANALYZE`, `REINDEX TABLE CONCURRENTLY`; `ANALYZE TABLE` and
  `OPTIMIZE TABLE` on MySQL. `VACUUM FULL` and `CLUSTER` are offered only with a warning that names the exclusive lock.

### 7.5 Explain

From Queries and from SQL:

- **Postgres:** `EXPLAIN (FORMAT JSON, VERBOSE, BUFFERS)`. Explain Analyze adds `ANALYZE`. For a statement that
  changes data it runs inside a transaction that is rolled back, and needs Allow Changes and its confirmation (§8.2),
  since the read-only transaction would refuse it and a rollback does not undo everything (sequences, for one).
- **MySQL:** `EXPLAIN FORMAT=JSON`, and `EXPLAIN ANALYZE` (a text tree) from 8.0.18.
- **MariaDB:** `EXPLAIN FORMAT=JSON`, and `ANALYZE FORMAT=JSON` in place of Explain Analyze.
- **The plan view** (part of `Sql.qvue`) is a tree. Each node shows its kind, relation and index, estimated and
  actual rows, cost and time. It marks a sequential scan on a table over 10,000 estimated rows, a misestimate over
  ten times, and a sort or hash that spilled to disk.

## 8. SQL

### 8.1 The screen

`Sql.xml` declares `render-modes="vue,qvue,tui"` and `server-static="vue,qvue"`, as the gallery's `Board.xml` does.

- **TUI:** renders the current form and results: the `tui` text of `SqlRunner.xml` moves here.
- **Editor** (`Sql.qvue`): CodeMirror through `WebrootCode.js`.
  - The bundle exports only `sql` from `@codemirror/lang-sql` (`libs/refresh.mjs:198`), so add the `PostgreSQL`,
    `MySQL` and `MariaSQL` dialects to it.
  - `languageExtension(CM, language, schema)` (`WebrootCode.js:152`) already takes a `schema`, but only its `xml`
    case uses it; the `sql` case (:160) returns a plain `CM.sql()`. It then passes the dialect and the `schema` for
    completion.
  - The schema comes from a `catalog` transition: tables and columns from the database, plus entity names mapped to
    their tables. Hovering a table name shows its entity.
- **Running:** the statement at the cursor (Ctrl+Enter), the selection, or the whole script (Ctrl+Shift+Enter).
  - Statements are split by `split_sql` (`exec.rs:176`). It tracks only single quotes today, so it gains Postgres
    dollar-quoted bodies (`$$`, `$tag$`) and comments, with tests.
  - Each statement that gives rows gets its own result tab, in the grid (§9, read-only unless it qualifies, §9.5). The
    others get a line with their row count.
  - A tab whose statement is a plain query (`SELECT`, `WITH`, `VALUES`, `TABLE`) run read-only is a SQL source (§9.1):
    sorting, filtering and "fetch more" run it again. Any other tab holds the rows it fetched and pages over them,
    because running its statement again could write again: a statement run with Allow Changes, `INSERT … RETURNING`,
    `SHOW`, `EXPLAIN`.
- **Limits:** a row limit (default 500) and "fetch more" (§9.3). A timeout (default 5 min). Cancel (§8.3).
- **History and saved queries:**
  - History is the user's own audit rows (§14.2), so it needs no table of its own.
  - Saved queries are `moqui.database.SavedQuery`: `queryId`, `description`, `db`, `sqlText`, `ownerUserId`,
    `isShared`, `parametersJson`. A `:name` parameter is bound as a value, never pasted in.
- **Results:** CSV or XLSX download, save to a resource location, copy as TSV, JSON or `INSERT` statements.

### 8.2 Changing data

- **Read-only by default.** Statements run in a read-only transaction: `SET TRANSACTION READ ONLY` on Postgres,
  `START TRANSACTION READ ONLY` on MySQL and MariaDB. The database itself then refuses writes to tables, including a
  data-modifying CTE and DDL, so no parser of ours is the guard.
- **Read-only is not a sandbox.** The embedded cluster's connections are the bootstrap superuser's
  (`crates/moqui-drivers/src/embedded.rs:706`, `:712`). In a read-only transaction a superuser can still run
  `COPY … TO PROGRAM` (a shell command on the server), `pg_read_file`, `pg_terminate_backend` and `set_config`. The
  read-only path guards against mistakes, not against the user; that is why SQL needs an `ADMIN_ADV` permission (§14.1).
  Running it as a role of its own without superuser is in §12.
- **"Allow changes"** is a switch on the screen. With it on, Run first asks the server to classify the statements.
  It uses the same test as `run_sql`: the driver's `returns_rows`, else the first word (`is_query`, `info.rs:717`). It
  shows a confirmation that lists the statements that may change data and their count. The confirmation returns a
  one-time token, and the run transition requires it.
- **Production** (`moqui_core::web::is_production()`): the confirmation also asks for the database name to be typed.
- **Transactions:** a script that changes data runs in one transaction unless "each statement on its own" is chosen.
  Statements that cannot run in a transaction (`VACUUM`, `CREATE INDEX CONCURRENTLY`, `CREATE DATABASE`) are
  detected by first word and run on their own, outside it. On MySQL and MariaDB, DDL commits the transaction
  implicitly, so a script with DDL is not atomic there; the confirmation says so.
- **The read fence** still applies: `fence::check_sql` (`crates/moqui-entity/src/entity/fence.rs:419`) refuses
  non-queries under it, so an agent's READ tool can use the read-only path and nothing else.

### 8.3 Cancel

A run gets a `runId`. The server keeps `runId → (instanceId, backend id, canceller)` while it runs, and
`cancelSql(runId)` uses the canceller on that instance, or `pg_cancel_backend` / `KILL QUERY` elsewhere, as for
jobs (§6). `run_sql` gains an optional callback that is handed the connection's canceller when the statement
starts.

## 9. The data grid

### 9.1 Sources

| Source | Read | Save |
| --- | --- | --- |
| Entity (`entityName`) | An entity find with conditions, order and offset/limit pushed to SQL (the `search-form-inputs` rules of `crates/moqui-entity/src/entity/search_form.rs`) | Yes, when the entity is not a view and the rows have its primary key |
| View entity, `DbViewEntity`, `DataDocument.<id>` | The same find. These are entities to the facade (`ViewDataDocument.xml` finds on `DataDocument.<id>`) | No. The row action opens the member entity's record in the row form |
| SQL (`db`, `sql`) | The statement wrapped as `SELECT * FROM (<sql>) q` with `ORDER BY`, `LIMIT` and `OFFSET`; only `SELECT`, `WITH`, `VALUES`, `TABLE` | No, unless it is one table with its whole primary key (§9.5) |
| File (`location`, `sheet`, `headerRow`) | CSV, or XLSX, XLSM, XLSB, XLS or ODS through calamine (`read_sheet`, `crates/moqui-core/src/service/spreadsheet.rs:81`), from a resource location or an upload | Download, save to a location, or load through an ETL mapping (§10) |

**Uploads** go to `dbresource://moqui/database/upload/<userId>/<uploadId>/<name>`. That is in the content database,
so every cluster member can read them. A daily ServiceJob, `clean#GridUploads`, deletes them after seven days, as
`clean#PendingAttachments` does for chat (`runtime/base-component/framework/data/ChatSetupData.xml:50`).

### 9.2 Server contract

A grid is defined by its source spec, which the client holds and sends with each call:

```json
{"source": {"type": "entity", "entityName": "moqui.basic.Enumeration", "conditions": {...}},
 "columns": ["enumId", "enumTypeId", "description"]}
```

`Grid.xml` has JSON transitions (`ec.web.sendJsonResponse`, as `boardRows` in the gallery's `Board.xml`):

- `columns`: the column definitions. Each has a name, title, type (from the entity field type, the SQL result's
  column types, or the file's inferred types), whether it is editable, and its editor. Primary key, encrypted and
  related-entity facts come with each.
- `rows`: Tabulator's progressive load asks with `page`, `size`, `sort` and `filter` and is answered with
  `{last_page, last_row, data}`. No source counts on every load, for the reason Tables avoids `count()` (§7.4):
  - while pages come back full, `last_page` is the current page plus one, and the last short page ends it;
  - the total shown is the catalog estimate for an unfiltered entity source, and otherwise unknown until asked;
  - "Count rows" runs a `count()` of the entity find, or a `count(*)` over the wrapped SQL query;
  - for a file the total is known.
- `save`: the changes (§9.4).
- `export`: CSV or XLSX of the whole source, not only the loaded pages, streamed, to a download or a resource
  location.
- `lookup`: options for a related-entity column (§9.4), searched by text, 20 at a time.

**No server state per grid.** Each call runs the source again. For entities and SQL this needs nothing more. A file
is parsed once per node into an LRU cache keyed by location, its last-modified time and the sheet (64 MB cap). Any
member that misses the cache parses the file again, so a load balancer without stickiness still works, as sessions
in the database expect. calamine reads a whole sheet at once; `MAX_SHEET_CELLS` (`spreadsheet.rs:44`) stays the
bound. CSV is read as a stream.

**Exports** use the cursor paths: an entity find iterator, or `run_sql` with no limit. They write through the `csv`
crate or `XlsxBook` (`crates/moqui-core/src/screen/xlsx.rs:76`), the same writer the screen's xlsx mode uses. Before
using it for 100k+ rows, check whether rust_xlsxwriter's constant-memory mode suits `XlsxBook`; otherwise cap the
export and say so.

### 9.3 The browser side

`Grid.qvue` loads Tabulator only when it mounts: `moqui.loadScript("/libs/tabulator/tabulator.min.js", null, () =>
!!root.Tabulator)` and `moqui.loadStylesheet(...)` (`MoquiLib.js:228` and `:277`), as the Chart.js and Tiptap loaders
in `WebrootVue.qvt.js` do (lines 3430, 4025).

- **Requests** go through Tabulator's `ajaxRequestFunc` to `moqui.request`, so they carry the session's CSRF token
  and the shell's error handling. The settings are `progressiveLoad: "scroll"`, `sortMode: "remote"` and
  `filterMode: "remote"`.
- **Editors** by field type:
  - `id` and text: `input`;
  - `text-long` and longer: `textarea`;
  - numbers and currency: `number`;
  - `date`, `time` and `date-time`: Tabulator's date editors;
  - `text-indicator`: `tickCross`;
  - a related-entity field: `list`, with `lookup` as its remote values.
  Display formats come from `MoquiL10n.js`, so Tabulator's optional Luxon is not loaded.
- **Validators** come from the entity model: `not-null`, length for text types, number for numbers. The server
  checks again on save.
- **Editing:** edited cells are marked (`getEditedCells`). Undo and redo use `history`. Copy and paste of ranges use
  `selectableRange`, `clipboard` and `clipboardPasteAction: "range"`. Rows can be added and deleted (marked until
  saved).
- **Layout:** columns can be frozen, moved, resized and hidden, through the column header menu.
- **The toolbar:** Save (with the count of changes), Discard, Export, Save to resource, Count rows, Filters, and
  Columns.
- **Theme:** Tabulator's stylesheet is themed with a small CSS file of our own, `DataGrid.css`, that uses the Quasar
  palette, light and dark, in `webroot/screen/webroot/css/`.
- **TUI:** `Grid.xml` declares `render-modes="vue,qvue,tui"` and `server-static="vue,qvue"`, as `Sql.xml` does, and
  its `tui` render mode is a paged `form-list` over the same `rows` service, read-only, with a row edit form for
  entity sources. There is no second screen, so links (the Dev AIDE's, the row actions) have one path. The terminal
  client gets no spreadsheet editing.

### 9.4 Saving to an entity

`save#DataGridChanges` (`entityName`, `updates`, `creates`, `deletes`):

```json
{"updates": [{"pk": {"enumId": "X"}, "stamp": "2026-11-05T10:00:00Z", "fields": {"description": "New"}}],
 "creates": [{"fields": {"enumId": "Y", "enumTypeId": "T", "description": "Y"}}],
 "deletes": [{"pk": {"enumId": "Z"}, "stamp": "2026-11-05T09:00:00Z"}]}
```

- **One transaction:** everything commits, or nothing does (decision 7). The answer lists each refused row with its
  field errors. The grid marks those cells and keeps all the edits, so the user can fix them and save again.
- **Entity-auto services:** each change calls `update#`, `create#` or `delete#` of the entity (`sync_with`, as
  `load_spreadsheet_entity`, `spreadsheet.rs:489`, does). Authorization, EECA rules, the audit log and validation
  then apply as for any service call.
- **Only edited fields are sent.** The dirty-field update writes only those, plus `lastUpdatedStamp`.
- **Conflicts:** for an entity with `lastUpdatedStamp`, the stored stamp is read `for update` and compared with the
  one the grid read. A row changed since then is refused as "changed by someone else since you loaded it", and
  Reload offers to refresh that row. Entities with `optimistic-lock` are checked by the entity facade itself
  (`crates/moqui-entity/src/entity/value.rs:1054`).
- **Refused entirely:**
  - entities that `is_restricted_generic_entity` refuses (`crates/moqui-entity/src/entity/sensitive.rs:91`), as Data
    Edit refuses them today;
  - the `UserAccount` secret fields (`remove_user_account_secrets`).
- **Encrypted fields** are shown masked and cannot be edited in the grid. The row form edits them, as today.

### 9.5 Editing a SQL result

A result is editable when every column comes from one table, and the columns include the whole primary key of an
entity whose table that is. A database view does not qualify: its columns' origins name the view, not the table
under it. The `columns` answer then names the entity, and saving uses §9.4. The
test runs on the server from the result's column origins: the Postgres `RowDescription` table oid and column number,
or the MySQL column `org_table`. It is not done by parsing the SQL. Both wire protocols send the origins, but the
drivers keep only the column names today (`DbRow.columns`), so this step adds them. A result whose origins are
missing is read-only.

### 9.6 Saved grid views

`moqui.database.GridView`: `viewId`, `description`, `ownerUserId`, `isShared`, `sourceJson`, `layoutJson` (columns,
widths, order, frozen, filters, sort). It replaces the Report Builder's saved finds on `ViewDataDocument.xml`, and
it is what "save this view" stores for any source.

## 10. ETL mappings

### 10.1 What exists

- **`moqui_lib::etl`** (`crates/moqui-lib/src/etl.rs`):
  - `SimpleEtl` (line 239), the `Extractor`, `Transformer` and `Loader` traits (lines 118, 124, 137),
    `TransformConfiguration` (148) and `FlatXmlExtractor` (452);
  - counts of extracted, skipped and loaded entries, and the extract, transform and load errors.
- **Entity side** (`crates/moqui-entity/src/entity/etl.rs`): `EtlLoader` (line 35) loads each entry into the entity
  named by its type, in one transaction for the run, through the data loader's upsert or the entity-auto store, with
  dummy foreign keys optional. `EntityFind` is an extractor.
- **`JdbcExtractor`** (`crates/moqui-drivers/src/jdbc_extract.rs:30`) reads one select from a group's driver or a
  conf database (`set_db_info`).
- **`org.moqui.impl.SpreadsheetServices.load#SpreadsheetEntity`** (`load_spreadsheet_entity`, `spreadsheet.rs:489`)
  maps headers to fields (`fieldMap`) and calls `create#` or `store#` per row (`loadMode`), in the caller's
  transaction, stopping at the first error.
- **None of it is reachable from scripts or screens yet.**

### 10.2 Entities

In `DatabaseEntities.xml`, package `moqui.etl`:

- **`EtlMapping`:**
  - `mappingId`, `description`, `ownerUserId`;
  - the source: `sourceTypeEnumId` (EtlsEntity, EtlsSql, EtlsFile, EtlsFlatXml); `sourceEntityName` and
    `sourceConditionsJson`; `sourceDb` and `sourceSql`; `sourceLocation`, `fileFormat` (csv, xlsx, auto), `sheetName`,
    `headerRow`, `firstDataRow`, `csvDelimiter`, `csvCharset`;
  - the target: `targetTypeEnumId` (EtltEntity, EtltService) and `targetName`;
  - `loadModeEnumId` (EtllCreate, EtllStore, EtllUpdate), and `matchFields` (comma-separated, for a natural key match:
    an update or store finds the existing record by these fields when the source has no primary key);
  - `batchSize` (0 or empty: one transaction for the run), `stopOnError`, `dummyFks`, `timeoutSeconds`.
- **`EtlMappingField`:**
  - `mappingId`, `fieldSeqId`, `targetField`;
  - where the value comes from: `sourceColumn` (a header, or a column letter or number), `constantValue`,
    `defaultValue`, `expression` (JavaScript with `row`, `value` and `ec`);
  - how it is converted: `valueMapId`; `lookupEntityName`, `lookupMatchField` and `lookupResultField` (a product name
    to a `productId`); `formatPattern` for dates and numbers; `trim`, `required`, `skipIfEmpty`.
- **`EtlValueMap`, `EtlValueMapEntry`:** `valueMapId`, `fromValue`, `toValue`, matched case-insensitively. Example:
  "Active" → `ACTIVE_STATUS`.
- **`EtlRun`:**
  - `runId`, `mappingId` (empty for an unsaved mapping), `userId`, `instanceId`, `jobRunId` (`ServiceJobRun`, for a
    scheduled run);
  - `isCheck`, `statusId`, `startTime`, `endTime`;
  - the source as resolved;
  - `extractCount`, `skipCount`, `loadCount`, `errorCount`, `lastCommittedRow`;
  - `errorLocation`: a CSV of the failed rows with an error column, in the resource store.
- **`EtlRunError`:** `runId`, `errorSeqId`, `rowNumber`, `fieldName`, `message`, `rowJson`. The first 1,000 errors;
  the rest are in the file.

### 10.3 Running

`run#EtlMapping` (`mappingId` or an inline mapping, `check`, `startRow`, optionally a location that overrides the
mapping's) builds a `SimpleEtl`:

- **Extractors:**
  - `EntityFind`, which exists;
  - `JdbcExtractor`, which exists, given a group's driver or an external connection (§4.3; not in production);
  - a new `CsvExtractor` that streams through the `csv` crate. This is the plain table format; the Java entity CSV
    with an entity name line stays Data › Load's;
  - a new `SheetExtractor` over calamine's rows (`read_sheet`, or a streaming form for xlsx);
  - `FlatXmlExtractor`, which exists.
- **The transformer:** a new `MappingTransformer`, built from the `EtlMappingField` rows. It renames, sets constants
  and defaults, maps values, and looks up (one cached find per distinct value per run). It parses formats, runs
  expressions (QuickJS, one compiled function per field per run) and checks `required`. A row that fails is an
  `EtlError` with its row number and field.
- **Loaders:**
  - `EtlLoader` for entity targets. It gains `batch_size`: commit after every N entries and begin again.
  - A new `ServiceLoader` for service targets: one call per row, with the mapped fields as in-parameters, so one row
    can make several records. For example Mantle's `create#Person` makes the party, person and user. The
    transactions are the same.
- **Transactions** (decision 7):
  - With no batch size, the run is one transaction, and any error rolls it all back. Errors of the extract and
    transform steps (types, required fields, lookups, value maps) are all collected and reported. The first database
    error ends the run, since Postgres aborts the transaction then (below); the check run is how to see the database
    problems of every row at once.
  - With a batch size, a batch that fails is rolled back and recorded. The run stops (`stopOnError`, the default) or
    goes on with the next batch.
  - `lastCommittedRow` tells a rerun where to start (`startRow`).
  - Postgres aborts a transaction after a failed statement, so a single row cannot be skipped without a savepoint
    per row. The driver has savepoints (`crates/moqui-entity/src/driver.rs:118`), but a round trip per row
    loses the pipelined writes. A "skip failed rows" mode is left out of the first cut. The check run covers most of
    the need.
- **Check** (`check=true`) runs the extractor and transformer, and no loader. It reports:
  - per-row errors: type, required, lookups not found, value map misses, and foreign keys with no related record,
    checked with the rule `fk::enforce` uses (`crates/moqui-entity/src/entity/fk.rs:88`);
  - for an entity target, the diff against the database: which records are new and which fields would change. This
    uses the data loader's check (`check_info_with`, `crates/moqui-entity/src/entity/load_feed/check.rs:109`).
- **Job and progress:** a run is a job on the worker pool. `EtlRun`'s counts are written every 1,000 rows in their
  own transaction, so the Run screen shows progress.
- **Scheduled runs:** a `ServiceJob` that calls `run#EtlMapping` with a `mappingId`. A source location may expand
  `${}` with the run's date, for a nightly file from SFTP or Files.
- **`load#SpreadsheetEntity`** keeps its parameters and runs an inline mapping: `fieldMap` becomes fields, and
  `loadMode` the load mode. It stops at the first error, as now, and joins the caller's transaction as now.
  `EtlLoader::init` begins a transaction of its own on each group, so it gains a mode that uses the one in progress
  when there is one.

### 10.4 Screens

- **`Data/Grid` with a file source** is where a mapping starts. "Load into…" picks a target entity or service. Each
  column header then gets a target field drop-down.
  - The fields are matched automatically by name, pretty name, and then the nearest name.
  - Matched columns show the converted value and its error in the grid, from a `check` of the first 200 rows.
  - "Save mapping" stores it. "Check" runs a check of the whole file. "Load" runs it.
- **`Etl/Mappings`:** find, copy and delete.
- **`Etl/Mapping`:** the mapping's form, the field list (`Etl/Mapping.qvue` for the column-to-field table with sample
  values), test against a file or source, run, schedule, and the runs.
- **`Etl/Runs`, `Etl/Run`:** status, progress, counts, the errors (in the grid, with the error file to download),
  rerun from the last committed row.
- **`Etl/ValueMaps`:** the value maps.

## 11. The grid library

Survey of November 5, 2026. Versions, dates and licenses were checked against npm, crates.io and the packages'
LICENSE files.

| Library | License, version | Fits | Why not |
| --- | --- | --- | --- |
| **Tabulator** | MIT, 6.7.1 (Oct 22, 2026) | Remote progressive load with server sort and filter; typed editors; validators; edited-cell tracking; undo; range selection and range paste; frozen and movable columns. A plain global; about 104 KB gzip and 4 KB CSS | Its spreadsheet mode has no formulas |
| AG Grid Community | MIT, 36.3 | Infinite row model, editors | Range selection and the clipboard are Enterprise (paid) |
| RevoGrid | MIT core, 4.31 | Web component, clipboard, ranges | Undo, validation, remote paging and formulas are Pro |
| SlickGrid (6pac) | MIT, 5.20 | Small, Excel copy buffer, remote model | More to assemble |
| Univer | Apache-2.0 open part, 1.3 | About 500 formula functions | React; no global build since 1.0; about 1 MB gzip; whole workbook in memory; XLSX import and export are Pro; open packages moving to Pro |
| Jspreadsheet CE | MIT, 5.0 (Aug 2025) | Small, basic formulas | Cross-sheet references and names are Pro; validation and XLSX are paid |
| Handsontable, HyperFormula | Proprietary; GPL-3.0 or paid | — | License |

**Vendoring.** Tabulator is vendored with a `libs/refresh.mjs` entry like Tom Select's (`refresh.mjs:289`). It
copies `dist/js/tabulator.min.js` and `dist/css/tabulator.min.css` to `libs/tabulator/`, at the newest version two
weeks old when it is run. The entry also gets a `MANIFEST.json` record and its license in `libs/NOTICES.md`. The
optional Luxon, SheetJS and jsPDF are not vendored: dates are formatted by `MoquiL10n.js`, and files are written by
the server. It also needs a browser row in `plans/archive/License-Review.md`, as Tom Select's (line 273).

## 12. Future: spreadsheet features

Not in this plan's delivery. Recorded so the grid and the server are built not to block them.

- **IronCalc** (MIT OR Apache-2.0, Rust, 0.9.1 on Oct 21, 2026; its README calls it early stage):
  - `ironcalc_base` is the engine, with about 497 functions including dynamic arrays. `ironcalc` reads and writes
    XLSX.
  - `@ironcalc/wasm` (about 718 KB gzip) is a full model without the UI: input, evaluate, undo, copy and paste, row
    insert and delete, change diffs. Only its UI (`@ironcalc/workbook`) needs React.
  - In the browser it would be loaded only in formula mode. On the server the same crate would compute formulas, so
    an XLSX export carries computed values (`save_to_xlsx`, or rust_xlsxwriter's `Formula::set_result`), and a
    server-side load evaluates a formula exactly as the browser did.
  - Before committing, a spike: Tabulator and the IronCalc WASM in a `.qvue`, 100k rows paged from the server, one
    column formula, and an export. Check the API's churn, memory with a large mirrored range, and cargo-deny on its
    dependencies.
  - Formualizer (MIT OR Apache-2.0, Rust) is the server-side alternative; its WASM (3.5 MB gzip) is too heavy for
    the browser.
- **Column formulas:** a computed column over each row, referring to fields by name (`[@quantity]*[@unitAmount]`, the
  structured reference form of an Excel table). Each row is computed on its own, so it works on a server-paged grid.
  It is exported as an Excel table whose calculated column is that formula (rust_xlsxwriter `Table` and
  `TableColumn::set_formula`: confirm in the spike). It can be an ETL transform in place of a JavaScript `expression`.
  Footer formulas (SUM, AVERAGE) go under a column.
- **Free cells:** a sheet beside the data, or a workbook mode, with the full IronCalc model in memory, capped to what
  fits.
- **Formulas from imports:** calamine reads a cell's formula text and its cached value. A loaded file could keep its
  formulas, rather than only their values, when re-exported.
- **Other items deferred from this plan:**
  - saving grid edits to an external database's tables, by primary key from the catalog;
  - running SQL on the embedded cluster as a role of its own without superuser, so the read-only path is also a
    sandbox (§8.2);
  - query stats from every cluster member in one list;
  - a "skip failed rows" load with a savepoint per row (§10.3).

## 13. Where the code goes

**A new framework crate, `moqui-tools`**, the server side of the Tools app's Data, SQL, Schema, ETL and Database
sections. It is not named `moqui-database`, which is too close to the existing `moqui-db` (the entity model, SQL text
and dialects). As `moqui-collab` does, it adds services (`CollabServices.xml:20`: `type="rust"
location="moqui_collab::…"`) and submits a `moqui_core::extension::Extension` (`crates/moqui-core/src/extension.rs:35`),
so an edit to it does not rebuild core. It is linked in every host build and left out of the browser. The checklist is
[Crate-Split, Adding a crate](archive/Crate-Split.md#adding-a-crate): workspace members, the host feature and
`use … as _`, `moq` `write_app_crate`, `scripts/check-packages.sh`, the Features table, the `moqui_tools::` prefix in
`extension::LOCATIONS` (a private const, `extension.rs:79`), and a registry test against the service XML's locations.

| Piece | Where |
| --- | --- |
| Capabilities, catalog queries per dialect (other than the comparison's), activity, tables, settings, explain | `crates/moqui-tools/src/{capabilities,catalog,activity,explain}.rs` |
| Jobs: run, progress, cancel, restart cleanup | `crates/moqui-tools/src/jobs.rs` |
| SQL runs: read-only transactions, the confirmation token, the cancel registry, audit | `crates/moqui-tools/src/sql.rs`, on `run_sql` (`moqui-entity`), which gains the canceller callback and a read-only flag; `split_sql` gains dollar quotes (§8.1) |
| Grid: columns, rows, save, export, lookup, file cache | `crates/moqui-tools/src/grid.rs` |
| ETL services, `MappingTransformer`, `ServiceLoader`, `CsvExtractor`, `SheetExtractor` | `crates/moqui-tools/src/etl/` |
| `EtlLoader` batches, and joining a transaction in progress (§10.3) | `crates/moqui-entity/src/entity/etl.rs` |
| `CsvExtractor`, if it needs nothing above `moqui-lib` | `crates/moqui-lib/src/etl.rs` beside `FlatXmlExtractor`, with `csv` added to `moqui-lib`, which does not depend on it today (only `moqui-entity` does; the lowest crate that can hold it) |
| Schema comparison | `crates/moqui-entity/src/entity/meta.rs` (`EntityDbMeta::compare`), its catalog SQL beside the DDL builders in `crates/moqui-db/src/sql/` (§4.2) |
| Statement stats | `crates/moqui-entity/src/entity/exec.rs` (`trace`), the `artifact_name` hook in `crates/moqui-entity/src/hooks.rs`, installed in `crates/moqui-core/src/entity_hooks.rs` |
| Script bindings (`getQueryStatsList`, `clearQueryStats`) | `crates/moqui-core/src/script/runtime/tools_host.rs:122` |
| `pg-stat-statements` | `moqui-conf-3.xsd`, `datasource.rs`, `entity_db.rs` `open_embedded`, `embedded.rs` `settings()`, the two conf files |
| Spreadsheet reading for the grid and ETL | `crates/moqui-core/src/service/spreadsheet.rs`: `read_sheet` (private) made `pub`, and `preview_rows` (`pub(crate)` and `#[cfg(test)]`) made a `pub` non-test function, for the new crate; `load_spreadsheet_entity` becomes a call of the inline mapping |
| Service definitions | `runtime/base-component/framework/service/org/moqui/impl/DatabaseServices.xml`, `EtlServices.xml` (`type="rust"`, `location="moqui_tools::…"`) |
| Entities | `runtime/base-component/framework/entity/DatabaseEntities.xml` |
| Enumerations, status items, ServiceJobs (`clean#GridUploads`, `DatabaseAuditPurge`) | `runtime/base-component/framework/data/DatabaseSetupData.xml` (`seed`), beside the entities and services that use them, and listed in `MoquiDefaultConf.xml`'s `load-data`: a framework data file loads only when listed there |
| Security: `DATABASE_ADMIN`, `DATABASE_EXTERNAL` | `runtime/base-component/tools/data/ToolsSecurityData.xml`; the app itself stays under `TOOLS_APP` |
| Screens | `runtime/base-component/tools/screen/Tools.xml` and `Tools/**` (§3) |
| Tabulator, theme CSS | `runtime/base-component/webroot/screen/webroot/libs/tabulator/`, `css/DataGrid.css` |
| Guide | `docs/tools/Database.md`, beside `Monitoring.md`: the Tools app's Data, SQL, Schema, ETL and Database sections, the conf settings, the permissions, and external databases (refused in production) |

## 14. Security and audit

### 14.1 Who can do what

| Action | Needs |
| --- | --- |
| Open the app; view activity, queries, tables, schema, settings, jobs | `TOOLS_APP` (`ADMIN`) |
| Grid reads and saves, Browse, Export | The app, plus the entity's own authorization through the entity-auto services; restricted entities refused (§9.4) |
| Data › Load, Snapshots import, ETL runs | The app, with `authz-action="all"` on the transitions that write, as `DataImport.xml`'s `load` has |
| Cancel or terminate a session, run DDL and maintenance jobs, reset stats | `DATABASE_ADMIN` (new; `ADMIN_ADV`) |
| SQL | `SQL_RUNNER_WEB` (`ADMIN_ADV`, `SecurityTypeData.xml:75–76`), and authorized for all actions on the screen, as `SqlRunner.xml` checks now |
| External databases | `DATABASE_EXTERNAL` (new; `ADMIN_ADV`), and not a production instance (§4.3) |

`DATABASE_ADMIN` and `DATABASE_EXTERNAL` are added to the sealed permissions (`DEFAULT_SEALED_PERMISSIONS`,
`crates/moqui-core/src/service/security_checks.rs:32`) beside `SQL_RUNNER_WEB`. The demo lock-down stubs the screens
that write (§2.2).

SQL and ETL from SQL read past the entity facade's authorization and field encryption: an encrypted column shows its
ciphertext. On the embedded cluster they also run as the database superuser, which reaches the server's files and
shell even in a read-only transaction (§8.2). This is why they stay behind `ADMIN_ADV` permissions.

### 14.2 The audit

`moqui.database.DatabaseAudit`:

- `auditId`, `userId`, `visitId`, `instanceId`, `db`;
- `actionEnumId` (DbaSql, DbaJob, DbaCancel, DbaTerminate, DbaGridSave, DbaLoad, DbaEtlRun, DbaStatsReset);
- `statementText` (`text-very-long`), `isChange`, `rowsReturned`, `rowsAffected`, `durationMs`, `errorText`,
  `startTime`;
- `detailJson`: for a grid save, the entity and the counts, not the values. The audited fields' history stays in
  `EntityAuditLog`.

The row is written in a transaction of its own (`force-new`), so it survives a rollback of what it records. It is
written for failed statements too. Every SQL statement is recorded, reads included: reading data outside the entity
facade is the thing worth knowing about.

`DatabaseAudit` is added to the read fence's `ALLOWED_ENTITIES` (`fence.rs:62`), so a fenced read-only SQL run can
still record itself. A `DatabaseAuditPurge` ServiceJob keeps 90 days by default (property `database_audit_days`).

## 15. Delivery

Each step is a commit set in mr-moqui; step 1 also has commits in mr-mantle, mr-boss and mr-marble (§2.2). Before
each commit, run `./moq test-framework`, plus the specs touched while working. The tools regression budget is 3
seconds (`runtime/base-component/tools/test/budget`), so the new specs are small rule specs: each renders a screen at
most once per mode, and no spec walks the app.

1. **Tools layout and moves.**
   - The section screens of §3 (`Data.xml`, `Schema.xml`, `Etl.xml`, `Database.xml`, `Database/Overview.xml`), the
     regrouped dashboard and the database selector.
   - The screens of §2.1 move, keeping their behaviour for now. System's DataDocument Export moves into Data › Export
     with `DocumentExport.json`. Tools' and System's links change, and the references of §2.2 are updated, in the
     component repositories too.
   - The unbound `entityDbMeta` calls are replaced: Check Tables through `check_tables_ddl`; the key and index
     buttons are removed until step 3.
   - Exit: `./moq test-framework` and, for the component repositories' changes, `./moq test-apps` pass; the
     no-ignore grep of §2.2 finds no old path; the demo spec passes.
2. **Statement stats and `pg_stat_statements`.**
   - §7.1, §7.2 and §4.1 (capabilities).
   - Exit:
     - a regression spec that runs a find and sees its statement and artifact in `getQueryStatsList`;
     - a `moqui-drivers` test that an embedded cluster started with the option has `pg_stat_statements` in
       `pg_extension`;
     - a conf test that `MoquiDevConf.xml`'s new datasource override keeps the default's `inline-jdbc` child.
3. **Schema comparison and jobs.**
   - §5 and §6, the Indexes and Keys screen, Download DDL, Tables (§7.4), Activity (§7.3).
   - Exit:
     - `moqui-entity` tests of `compare` on a schema with a dropped index, an extra index and a missing key;
     - a host test that runs a `CREATE INDEX CONCURRENTLY` job, cancels a second one through `pg_cancel_backend`,
       sees the invalid index the cancel leaves, and reruns it to a valid index.
4. **SQL.**
   - §8, the audit (§14.2), Explain (§7.5), the CodeMirror dialects and completion.
   - Exit:
     - a write in the default read-only mode is refused by the database;
     - a write with Allow Changes needs the token;
     - every statement has an audit row, including one that failed;
     - cancel stops a `pg_sleep(30)`.
5. **The data grid on entities and SQL.**
   - Tabulator vendored with its license row, `Grid.qvue`, the grid's `tui` render mode, §9.1–9.6 without file
     sources.
   - Exit:
     - rule specs of `save#DataGridChanges`: a refused row rolls back the rest; a stale stamp is refused; a
       restricted entity is refused;
     - an acceptance browser walk (`tools/acceptance/client/`) that edits, pastes a range, undoes, saves and exports.
       It is run once, since it is new.
6. **Files and ETL.**
   - §10 and the file source of §9.1. `load#SpreadsheetEntity` on the inline mapping.
   - Exit:
     - `moqui-tools` tests of `MappingTransformer` (value map, lookup, format, expression, required);
     - a run in batches that fails in the third batch, keeps the first two and reports `lastCommittedRow`;
     - a check run that reports a missing foreign key;
     - an acceptance test that loads a 100k-row CSV in batches under its time budget.
7. **External databases.**
   - §4.3, Connections.
   - Exit: a host test that a production-purpose instance refuses `db=ext:…` with the vague error, and that a dev or
     test instance runs SQL on a second embedded database through a connection row.
8. **Settings and the guide.**
   - §4.4, `docs/tools/Database.md`, the `docs/security/Attack-Surface.md` rows, and this plan's status line.
   - When everything is built, the plan moves to `archive/` by the README's rule, and what is left goes to
     Remaining-Work.
