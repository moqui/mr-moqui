# Entity and Service Parity Review

> Archived 2026-10-16: phases 1 to 8 are worked, with 7 open findings (none High) and one open decision, listed in [Remaining-Work, Parity gap review](../Remaining-Work.md#parity-gap-review). The accepted differences moved to [Parity-Diffs](../Parity-Diffs.md). The phase tables and the "checked and found equivalent" lists stay here as the record of what was compared.

Scope: Rust modules in `crates/moqui-core/src/entity/` and `crates/moqui-core/src/service/` (with the
`moqui-db` SQL/model layer they delegate to), compared against the Moqui Java / Groovy / XML code they
were ported from.

Revalidated 2026-10-16 against `d9d5f9f`, and worked through phase by phase on 2026-10-16 (commits 7255066 through
the Phase 8b/8c commit). Findings fixed by 956954e, 48e9d36, 62a3def and 2ce6ecd and in those phases, and findings
that proved invalid or duplicated, have been removed; the IDs that remain keep their original numbers, so gaps in the
numbering are expected. Deliberate, documented differences are collected in
[Parity-Diffs](../Parity-Diffs.md) rather than the phase tables.

Paths: Rust locations are relative to `crates/moqui-core/src/` unless they start with `moqui-db/`,
`moqui-lib/` or `moqui-host/` (siblings under `crates/`), or `runtime/`. Java locations are file names
under `moqui/framework/src/main/{groovy,java}/org/moqui/impl/` or framework service XML. Line numbers
are as of the revalidation commit.

Severity: **High** = wrong result, data loss, authorization gap, or crash; **Med** = missing branch,
changed default, or edge-case divergence; **Low** = cosmetic, message text, or provably equivalent
ordering.

## Summary

7 open findings and 66 accepted differences. Open by phase:

| Phase | Open | High |
|---|---|---|
| 1 Entity value / field / condition | 2 | — |
| 2 Entity find / query | 0 | — |
| 3 Facade / metadata / DDL / cache | 2 | — |
| 4 Data loader / feed / ECA / writer / document / ETL | 1 | — |
| 5 Entity query / REST | 1 | — |
| 6 Service definition / validation / call | 0 | — |
| 7 ECA / SECA / pools / jobs | 0 | — |
| 8 REST / remote / tools / mail / components | 1 | — |

No High finding is open.

### Cross-cutting threads

No access-control finding is open.

1. **Full-text search and model sources** (P3-20, P3-21): a `LIKE` on a `text-fts` field is not rewritten to a
   full-text search (the DDL now follows `FtsDdl`), and a foreign key is recognized only by its computed name. DB view
   entities and `DataDocument.<id>` names are now built at runtime, in an overlay on the file model (P3-14, P5-18).
2. **View fields** (P5-19): the entity JSON schema and the data writer's CSV of a view read no fields (REST finds and
   form auto fields now read the aliases, and `pq-expression` aliases are evaluated). That now includes the views the
   overlay builds.

### Open decisions

- **P6b-01** — decided 2026-10-16 and done: Java's record-and-return contract is adopted; see
  [Phase 6](#phase-6-checked-and-found-equivalent).
- **P8a-41** — whether every print job asks for portrait and monochrome, as cups4j's builder defaults make Java's do,
  or keeps the print queue's defaults, as now; see [P8a-41](#p8a-41--portrait-and-monochrome-on-every-job).

### Stale code comments (small to-dos)

None open. The last one, the loader's comment that every record uses the entity-auto store path, now says that CSV
rows do not (P4a-02, `entity/load_feed/loader.rs:466-467`).

### Outside this review

- `cargo test -p moqui-host --test gate3` has 3 failures that are not from this work; they fail the same way with
  the Phase 5 and Phase 8a changes stashed: `artifact_group_detail_renders_as_text_csv_xml_and_html_print`,
  `artifact_group_detail_static_qvt_is_a_data_free_template_with_its_data_on_actions` and
  `the_login_page_and_the_root_render_as_html_and_a_failed_login_comes_back_with_its_message` (screen-render
  assertions: an ALL_USERS link, a `$mq.url` template, a login alert div).
- `user/preference.rs` `date_open` treats `thruDate` as exclusive; Java's `filterByDate` is inclusive (as fixed for
  jobs in P7b-11 and security checks in P7a-14).

---

## Phase 1 — Entity value / field / condition core

Compared `entity/{value,field_rw,crypto,fk}.rs` (plus `moqui-db/src/sql/condition.rs`) against
`EntityValueBase.java`, `EntityValueImpl.java`, `EntityList(Impl).java`, `FieldInfo.java`,
`EntityJavaUtil.java` and `condition/*`.

Worked 2026-10-16: P1-02, P1-03, P1-06 and P1-10 fixed; P1-07 and P1-13 moved to
[Accepted differences](#accepted-differences); P1-14 and P1-15 found along the way. Line numbers in this
section are as of that change.

| ID | Rust location | Java/XML location | Divergence | Severity | Intentional? |
|---|---|---|---|---|---|
| P1-14 | `entity/value.rs:750` | `EntityValueImpl.java:142-143` | An update that matches no row returns `Ok`; Java's `updateExtended` throws "Tried to update a value that does not exist" | Med | No |
| P1-15 | `entity/value.rs:464-534` | `EntityValueBase.java:1324-1343`, `1654-1658` | A `lastUpdatedStamp` the caller sets on a found value is replaced unless the update reads the row; Java keeps it, since `put` records the old value in `dbValueMap` | Low | No |

#### P1-14 — Update of a missing row is silent

`finish_write` takes the affected-row count from `execute_tx` and only the create-or-update probe looks at
it. Java throws when `executeUpdate()` returns 0, so `value.update()` and the entity-auto `update#` of a
row that is not there fail in Java and do nothing in Rust. An entity whose update reads the row first
(see the equivalences below) returns early in both, since Java then compares the value with itself.
Making the zero count an error needs a pass over the Rust callers that update rows that may be gone.

#### P1-15 — Caller-set stamp without a read

Java's `putKnownField` keeps the old value of a field changed on a value read from the database, and
`update` counts `lastUpdatedStamp` as modified, so keeps the caller's stamp, when that old value is
non-null and differs. Rust's `EntityValue` does not track what it read, so it can tell only against a row
the update reads (audit-logged, create-only or defaulted fields, optimistic locking, or a data feed). It
matters only to a caller that sets the stamp on purpose on an entity with none of those.

#### Phase 1 checked and found equivalent

- `update` (P1-02, fixed). Java reads the stored row first only for `needsAuditLog`, `createOnlyAny`,
  `optimisticLock`, `hasFieldDefaults` or a data feed document; then it merges the stored fields into the
  value, writes only those that differ, and returns after the run-before EECA rules when none do. With no
  read it writes every field the value holds (`isFieldModified` is true for any field `dbValueMap` lacks),
  as Rust always did. Rust now reads the row in the same cases (`update_reads_row` plus
  `feed_wants_old`; the create-or-update probe is skipped for them), reuses the row create-or-update just
  read, and writes, skips and refuses as Java: a stamp alone is no update, a changed create-only field is
  an error, and optimistic locking refuses a stale stamp. Only the changed fields and the key are sealed,
  so an unchanged field that failed to decrypt is not written back.
- `lastUpdatedStamp` (P1-03, fixed): the transaction start time (`TransactionManager::start_time_millis`),
  else the wall clock; on create only when unset; on update kept when it differs from the stored one (see
  P1-15 for the case with no read).
- Field defaults (P1-06, fixed): any expression runs in the script runtime with the stored fields and then
  the value's fields in the context, as Java pushes `dbValueMap` and the value map; quoted literals,
  numbers, field names and `ec.user.nowTimestamp`/`userId` skip the script. `""` is empty; an unknown name
  is null, as `ContextBinding`. Primary-key defaults are filled before an update's read and the others
  after it. The one Groovy-only default in the runtime (`DataDocumentField.sequenceNum`) is converted, and
  `moqui convert --groovy` now treats an entity field's `default` as an expression.
- Audit rows (P1-10, fixed) go through `create#moqui.entity.EntityAuditLog` with authorization off when the
  context's facade is the one writing, so that service's SECA rules run; with no context they are created
  directly. An update's audit compares with the row it already read.
- Field encryption: constant IV, salt padding, iteration default, algorithm, base64-vs-hex decode,
  `decrypt-alt` retry — test vectors pass. Decrypt failure uses `_DECRYPT_FAILED_` and refuses to
  re-encrypt it (`entity/field_rw.rs:116-121,153-157`).
- Localization: localized text is cached per locale and never written back to the stored field;
  single-PK only and `"default"` locale fallback, as Java.
- FK existence check only under `dummy_fks` (loader), as Java (`entity/fk.rs:15-17`).
- Audit: field selection for `true`/`update`, `{field}_changeReason`, `artifactStack`.
- Null forms of equal/not-equal/like/between → `IS (NOT) NULL`; empty `IN`/`NOT IN` → constant
  false/true; `ignore-case` upper-casing keyed off a String column.
- `order-by` parsing (`OrderBy::parse`) incl. `NULLS FIRST/LAST`, `DESC/ASC`, `UPPER()/LOWER()`,
  `never-nulls`; `create_or_update` applies PK defaults before the existence check.

---

## Phase 2 — Entity find / query

Compared `entity/{find,search_form,schema}.rs` (with `moqui-db/src/sql/` and `screen/aggregate.rs`)
against `EntityFindBase.groovy`, `EntityFindImpl.java`, `EntityFindBuilder.java`,
`EntityQueryBuilder.java`, `AggregationUtil.java`, `FtsDdl.java`, `FtsSql.java` and
`RestSchemaUtil.groovy`.

Worked 2026-10-16: P2-02, P2-03, P2-04, P2-06, P2-07 and P2-11 fixed; P2-08 moved to
[Accepted differences](#accepted-differences); P2-13 and P2-15 found and fixed, and P2-14 found and accepted,
along the way. No Phase 2 finding is open. Line numbers in this section are as of that change.

#### Phase 2 checked and found equivalent

- Repeated equality (P2-03, fixed): `eq` and `condition` keep one equality (or `IS NULL`) per plain field, as
  Java's `simpleAndMap`, so a second one for the field replaces the first in place, and an `AND` is unrolled so
  its equalities do the same (`add_condition`, `entity/find.rs:723`). A search-form condition goes the same way,
  so a form value replaces the find's own equality on that field, as in Java.
- Search-form cache (P2-02, fixed): `search_form_map` sets `useCache(false)` when the find did not choose, and a
  find that asked for the cache (Java's `shouldCache`) is not paged (`entity/search_form.rs:66-69`, `:98`).
- Search-form `_ic` (P2-04, fixed) applies to `equals`/`like`/`contains`/`begins` only, not `in`.
- Search-form `_not` (P2-13, fixed): on `equals` and `in` it also finds the rows with no value, Java's `orNull`
  (`or_null`, `entity/search_form.rs:314`); `like`/`contains`/`begins` stay a plain `NOT LIKE`. Before, Rust
  left out the null rows.
- `pageNoLimit` (P2-15, fixed) removes paging only for `true` or the text `"true"`, as Java; Rust took any
  non-empty value but `false`, so `Y` removed paging.
- JSON schema relationships (P2-06, fixed): an entity's dependent relationships, or a master's details (with
  `use-master`), are properties named by short alias or relationship name with a `$ref` to the related schema,
  an array for `many`; the related schemas go in `$defs`, nested as Java nests them, and a reference back to a
  standalone document's own entity is `#` (`entity/schema.rs:126-240`). The catalog also holds any view entity
  a relationship refers to, so every `$ref` resolves; Java refers to a definition it never writes.
- JSON schema `enum` (P2-07, fixed): the first non-auto-reverse `one` relationship keyed by the field alone,
  titled and to `Enumeration` or `StatusItem`, lists that type's ids by `sequenceNum` (`FieldEnums::of`,
  `entity/schema.rs:260`, which the service OpenAPI also uses, P8a-24); a read that fails is logged and the field
  has no `enum`.
- Distinct count (P2-11, fixed): with no field selected it counts `SELECT DISTINCT *` in a sub-select, as Java
  wraps all fields (`moqui-db/src/sql/statement.rs:262`).
- `one()` with no condition or an empty PK returns `None` without querying (`entity/find.rs:368-406`).
- `select_field` of an unknown field is skipped with a warning (`entity/find.rs:622-634`); its doc comment now
  says so.
- Search-form operators (`equals`/`like`/`contains`/`begins`/`empty`/`in`, `_from`/`_thru`, `_period`),
  date-range widening, `_period`/`getPeriodRange` windows, unknown op → no condition.
- Pagination defaults (page size 20, offset `index*size`, `pageNoLimit`, negative clamp); paged and
  no-condition finds are not cached.
- `narrow_to_full_pk`, `shouldCache` core rules, `FOR UPDATE` requiring a transaction.
- Order-by (`ORDER BY 1` when paging, `NULLS LAST` unless `never-nulls`, `UPPER`/`LOWER`), distinct
  order/select inclusion.
- View query building (member trimming, join paths, `entity-condition`, group-by/having,
  `COUNT(*) FROM (…)`) and aggregation (`min`/`max`/`sum`/`avg`/`count`/`first`/`last`, total row).
- Full-text conditions (`FtsSql`); find authorization push and find filters (fail-closed on a missing
  view alias); `requireSearchFormParameters`.

**Not ported (scope, not findings):** `updateAll`/`deleteAll`/`extract`/`listMaster`/`oneMaster` and
JDBC-only options (`fetchSize`, `maxRows`, `resultSetType/Concurrency`, cursor `offset-style`).

---

## Phase 3 — Facade / metadata / sequence / datasource / cache

Reviewed the entity facade, model/DDL metadata, sequenced-id, datasource, tools-info, cache and
dbresource modules. Physical naming, DDL text and the model loader live in `moqui-db`; those findings
cite their `moqui-db` location.

Worked 2026-10-16: P3-01, P3-02, P3-07, P3-09, P3-10, P3-12, P3-13, P3-16, P3-17, P3-18, P3-19, P3b-03,
P3b-04, P3b-07, P3b-08 and P3b-09 fixed; P3-06 and P3-11 moved to [Accepted differences](#accepted-differences);
P3-08 removed as invalid (H2, SQL Server and Oracle have no driver in this port and `datasource_conf` refuses
them, so no full-text DDL can run for those styles); P3-14 left open; P3-22 found and fixed, and P3-20 and P3-21
found and left open, along the way. Line numbers in this section are as of that change. P3-14 fixed in a later pass,
with P5-18, on an overlay of definitions built at runtime; its line numbers are as of that pass.

### Findings — 3a facade / metadata / sequence / datasource

| ID | Rust location | Java location | Divergence | Severity | Intentional? |
|----|---|---|---|---|---|
| P3-20 | `moqui-db/src/sql/condition.rs:811` (`matches` only) | `FtsSql.java:81-123`; `FieldValueCondition.java:73` | A `LIKE`/`NOT LIKE` on a `text-fts` field stays a SQL `LIKE`; Java rewrites it to the full-text predicate plus a `LOWER(col) LIKE` per word, and falls back to `LIKE` when the group's full-text setup failed | Med | No |
| P3-21 | `entity/meta.rs:733` | `EntityDbMeta.groovy:881`, `1047`, `1132` | A foreign key counts as there only by its computed name; Java matches the key columns, so a key Java made under another name gets a second one | Low | No |

No 3b (cache / dbresource) finding is open.

#### P3-20 — `LIKE` on a `text-fts` field is not a full-text search

Java's `FieldValueCondition` hands a `LIKE`/`NOT LIKE` on a `text-fts` field to `FtsSql.appendIfRewritten`,
which (unless the pattern is a prefix-only one, the words are all shorter than three letters, a hold overlay is
active, or the group's FTS is disabled) writes the `fts-style` predicate (`{col}_tsv @@ plainto_tsquery(...)`,
`MATCH ... AGAINST`, ...) and a `LOWER(col) LIKE ?` filter per word. Rust renders every `LIKE` as SQL `LIKE`;
the full-text predicate is reachable only through `EntityFind::matches`, which no component calls. The
Phase 2 note on `FtsSql` covered only that predicate's text. Rust also has no `FtsSql.disable`: when the
`_tsv` column could not be added (P3-07), `matches` on Postgres fails rather than falling back to `LIKE`.

#### P3-21 — Foreign key existence by name

`ensure_fks` skips a foreign key when the schema has a constraint of the name `foreign_key_name` gives. Java's
`foreignKeyExists` reads the imported keys and matches the key columns. A database whose keys Java created under
another name (MySQL's `fk-style="name_fk"` on MySQL 8.0.16 and later names them `{table}_ibfk_N`, see P3-11)
gets a second, identically keyed constraint from the first Rust check. Matching by key columns needs a catalog
read of the foreign key columns per group, like `existing_constraints`.

#### Phase 3 checked and found equivalent

- DB view entities (P3-14, fixed; line numbers as of that pass): a name the file model does not have is looked up in an
  overlay of definitions built at runtime, kept with the facade behind a lock (`entity/derived.rs`), which `with_entity`
  and `with_read_group` consult after the file model (`definition`, `entity/facade.rs:569`), so finds, counts,
  iterators, the sim and the SQL builders take it as any view; its members are completed against the file model. As
  Java's `loadEntityDefinition`, a `DbViewEntity` named by its name, or by `{packageName}.{name}` (Java's `null.{name}`
  with no package), becomes a `view-entity` of its `DbViewEntityMember` rows (joined from `joinFromAlias` by their
  `DbViewEntityKeyMap` rows, optional for `joinOptional` `Y`) and `DbViewEntityAlias` rows (`field`, `function`), parsed
  and completed as a file view is (`db_view`, `entity/derived.rs:182`; `EntityModel::derive_view`,
  `moqui-db/src/model/mod.rs:464`). Built on its first lookup and kept under the name and the full name; a name that is
  not one is kept as not found, so asking again reads nothing (`lookup`, `:82`; the misses are dropped past 10,000
  names). A write to a `moqui.entity.view.DbViewEntity*` or `moqui.entity.document.DataDocument*` entity drops the whole
  overlay, from the entity write hook at the write and again at its transaction's end, and from a cluster peer's
  `entity-cache-invalidate`, which already reaches `clear_one_cache` (`note_write`, `entity/cache.rs:541`, `:871`,
  `:889`); a build that read the rows before a drop is not kept. The data view edit screen's
  `clearEntityDefinitionFromCache` drops it too (all of it, where Java drops the one named;
  `script/runtime/tools_host.rs:924`). A view that does not build (a member that is not a file entity, an alias of a
  field its member lacks) is logged with its name and is not found until its rows change, and other lookups go on
  (AGENTS.md failure tolerance). `isEntityDefined` and the tools' `getEntityDefinition` see it (`entity/info.rs:51`,
  `:350`), as do form auto fields, read each time rather than kept with the model's shapes (`screen/form.rs:859`), the
  entity REST paths, the JSON schema of one entity (`entity/schema.rs`), and an OpenAPI parameter enum
  (`service/rest.rs:1057`); the whole-model listings (entity lists, the schema catalog, typegen, `getAllEntityNames`)
  stay on the file model, so they leave a DB view entity out, where Java's `getAllEntityNames` has it. The data view
  screen shows the view as Java's does (`ViewDbView.xml`, now Java's). Its rows are read outside the caller's
  transaction and the view is never cached (accepted, P3-14 in [Accepted differences](#accepted-differences)). A
  `DbViewEntity` member that is itself one is not found (Java builds it in turn).
- Global `sequenced-id-prefix` (P3-01, fixed): read once with the conf (`sequenced_id_prefix`,
  `entity/facade.rs:601`) and put in front of every id a bank issues, for an entity (inside its own
  `sequence-primary-prefix`) and for `sequencedIdPrimary`; not on a UUID or a sim id, as Java
  (`entity/sequence.rs:211`, `:257`).
- Entity `sequence-primary-prefix` (P3-22, found and fixed): `${}` runs with the value's fields in the context,
  as Java's `setSequencedIdPrimary` expands it (`expand_prefix`, `entity/sequence.rs:216`). Before, the text was
  used as written.
- Datasource `sequence-primary-use-uuid` (P3-16, fixed): the entity's attribute or its group's datasource (the
  default group's when it has none) gives a UUID, for `setSequencedIdPrimary` and `sequencedIdPrimary`
  (`datasource_uses_uuid`, `entity/facade.rs:307`).
- `needsAuditLog` in the tools info (P3-02, fixed): a field (the entity's attribute as its default) with `true`
  or `update` (`entity/info.rs:572`).
- Full-text DDL (P3-07, fixed): on Postgres the generated column `{column}_tsv` (`to_tsvector(config,
  coalesce(col, ''))`, stored) and a GIN index `fts_{table}_{column}` on it; on MySQL a `FULLTEXT` index
  `{table}_{column}_FTS`; names with characters other than letters, digits and `_` as `_`, clipped to 60, as
  `FtsDdl` (`moqui-db/src/sql/statement.rs:439-520`). `matches` on a `text-fts` field searches the `_tsv` column
  (`moqui-db/src/sql/condition.rs:817-825`); a view alias still computes `to_tsvector(...)`, which is right but
  not indexed. A plain entity with a `text-fts` field no longer reads `SELECT *`, which would carry the generated
  column. **Upgrade:** a database this port set up before has an expression index `{table}_{field}_FTS` clipped
  to 30 and no `_tsv` column. The next table check (startup, or the runtime check) adds the column and its index
  and then drops that old index (`ensure_fts`, `entity/meta.rs:667`); on MySQL a `FULLTEXT` index under the old
  name is dropped after the new one is added. On Postgres adding the stored column rewrites the table once, so a
  large `text-fts` table makes that start slow; set `fts-ddl="false"` and add the column and index by hand to
  avoid it. Checked against embedded Postgres, including the upgrade.
- `fts-ddl="false"` (P3-09, fixed): no full-text column or index, in create table or in the check, and no old
  index is dropped. The attribute is now in the runtime's `moqui-conf-3.xsd`.
- `use-fk-initially-deferred` (P3-10, fixed): `INITIALLY DEFERRED` after the references
  (`moqui-db/src/sql/statement.rs:599-601`). Only new foreign keys get it; an existing one is not altered.
- `always-use-constraint-keyword` (P3-12, fixed): `CONSTRAINT NOT NULL` for a not-null column of a create table
  (`moqui-db/src/sql/statement.rs:737-739`). No shipped profile sets it.
- `<load-entity location>` (P3-13, fixed): the conf's files are read after the framework's `entity` directory and
  before the components', as Java's `getAllEntityFileLocations`; a file already in a model directory (the
  framework's own list) is read once, and `*.eecas.xml` ones join the EECA rules (`load_entity_files`,
  `entity/facade.rs:624`; `EntityModel::load_paths`, `moqui-db/src/model/mod.rs:399`). `component://` is the
  framework or a component whose directory has that name; a location that cannot be found is logged and skipped.
- Repeated entity names (P3-17, fixed): a full name defined again is a warning naming both locations and the
  later definition is used; an entity name in more than one package is a warning at load and means the
  framework's (`moqui.`) definition, else the full name that sorts last, as Java's definition cache ends up after
  `loadAllEntityDefinitions` (`preferred`, `moqui-db/src/model/mod.rs:497`).
- `extend-entity` with no matching entity or another package (P3-18, fixed): skipped with a warning naming it
  (`moqui-db/src/model/mod.rs:586`); Java skips it silently.
- JDBC URI parameters (P3-19, fixed): those with an equivalent in this port's drivers are read (`sslmode`/
  `sslMode`, MySQL `useSSL`, Postgres `currentSchema` when there is no `schema-name`); any other is logged by
  name and left out, since the Postgres and MySQL clients here are not JDBC drivers (`jdbc_uri`,
  `entity/datasource.rs:295`).
- Sim rollback poison (P3b-03, fixed): the first sim write of a transaction adds a rollback hook that ends the
  sim, as Java's `noteWrite`/`poison`; afterwards every find and sim write fails with Java's message until the
  sim is exited (`note_sim_write`, `entity/cache.rs:211`; `sim_check_find`, `:183`). A transaction that commits,
  one that never wrote to the sim, and a write with no transaction are not tracked, and the sim's own records on
  production (`SIM_PRODUCTION_ENTITIES`) are still read. Checked against an embedded overlay.
- dbresource write across a file/dir boundary (P3b-04, fixed): the parent directories go through
  `directory_id`, so a file in the path is Java's "found file instead" error, and a directory at the location is
  an error rather than becoming a file (`write_db_resource`, `entity/dbresource.rs:120`).
- `DbResourceFile.mimeType` (P3b-07, fixed): set on the first write from the file name
  (`content_type_for_name`, `entity/dbresource.rs:366`); that list is shorter than Java's MIME map, so a rarer
  extension gets none.
- Directory children (P3b-08, fixed) are listed by `filename` (`entity/dbresource.rs:313`).
- `getNextVersions` (P3b-09, fixed): the history rows whose `previousVersionName` is the version
  (`db_resource_next_versions`, `entity/dbresource.rs:466`), and `ResourceReference.getNextVersions` in scripts.
- `<database use-indexes>`, `use-indexes-unique(-where-not-null)`, `use-foreign-keys`
  (`entity/meta.rs:609,725`, `moqui-db/src/dialect.rs:217-223`); unknown entity group falls back to
  the default group (`entity/facade.rs:536-546`).
- Entity-operation authorization (`AT_ENTITY` for create/update/delete/view, `authorize-skip`
  semantics); facade `begin`/`commit`/`rollback`/`transactions_for` per group.
- Sequenced-id bank start 100000, bank size 50, stagger, `tempSet/tempReset`, per-name locking,
  per-entity `sequence-primary-prefix`.
- `enable-localization`, `encrypt`, `not-null`, PK `CONSTRAINT` naming, constraint-name clip 30;
  `use-foreign-key-indexes`, `never-nulls`, `never-try-insert`, `use-binary-type-for-blob`,
  `table-engine`, `character-set`, `collate`, `for-update`; FK/index naming; dictionary type mapping.
- Cache: one/list keying, `entity.record.list.{entity}`, LFU no-TTL, `useCache`/`neverCache`; member
  writes evict cached views over that member, nested views included (`entity/cache.rs:584-586,705-746`, coarser
  than Java but safe); count cache; limit/offset finds not cached.
- Per-transaction hold cache: create-after-delete collapse, delete-after-create, chronological flush,
  rollback drop, last-value-wins update; `flush_and_disable`; `stopTxCache`.
- Sim: view reads answered from the overlay (`sim_view.rs`, used by list/count/iterator); production
  allow-list matches `TransactionCacheDb.java:128-131`; overlay lifecycle.
- dbresource versioning, delete, move, missing-path `exists:false`, `stat` last-modified.
- Write-hook ordering (cache clear, feed registration, audit after the DB write).

---

## Phase 4 — Data loader / feed / ECA / writer / document / ETL

Reviewed `entity/load_feed/*`, `entity/data_writer.rs`, `entity/document.rs` and `entity/etl.rs`
against `EntityDataLoaderImpl`, `EntityDataFeed`, `EntityEcaRule`, `EntityDataWriterImpl`,
`EntityDataDocument`, `SimpleEtl` and the relevant `EntityFacadeImpl`/`EntityValueBase` methods.
`pull.rs` has no Java counterpart.

Worked 2026-10-16: P4a-01, P4a-02, P4a-05, P4a-08, P4a-09, P4a-10, P4a-11, P4a-12, P4a-13, P4a-14, P4a-15,
P4b-01, P4b-02, P4b-03, P4b-05 and P4b-06 fixed; P4a-04 and P4b-08 moved to
[Accepted differences](#accepted-differences); P4a-16, P4a-17 and P4a-21 found and fixed, P4a-19 and P4a-20 found
and accepted, and P4a-18 (the `list()` half of P4a-10) split off and left open, along the way. The shipped data
(`moqui load types=all` in a copy of `runtime/`: the framework, tools and webroot seed, install and demo files, 1389
records) loads with no failed file or error. Line numbers in this section are as of that change.

### Findings — 4a data loader

| ID | Rust location | Java location | Divergence | Severity | Intentional? |
|----|---|---|---|---|---|
| P4a-18 | `script/runtime/tools_host.rs:220-262` (no `list`) | `EntityDataLoaderImpl.groovy:209-217,633-645` | `EntityDataLoader.list()` unported: the values a load would write, as an `EntityList`, with no service called | Low | No |

No 4b (feed / ECA / writer / document / ETL) finding is open.

#### P4a-18 — `EntityDataLoader.list()` not ported

Split from P4a-10 when `checkInfo` was ported. Java's `ListValueHandler` collects each record (nested records through
`addValuesFromPlainMapRecursive`) into an `EntityList` and logs each service element instead of calling it. No screen,
service or component in `runtime/` calls `list()`, and the DataImport screen uses `check`/`checkInfo`, so it is left
open for want of a caller. A port would divert records the way a check does (`entity/load_feed/check.rs`), turn each
`Record` tree into entity values in Java's order, and bind `l.list` in the script data loader.

#### Phase 4 checked and found equivalent

- CSV BLOB cells (P4a-01, fixed): a `binary-very-long` cell is Base64 (the padding optional, as Java's decoder), and
  text that is not Base64 fails the file (`entity/load_feed/csv.rs:156`; `decode_base64`,
  `entity/load_feed/loader.rs:334`).
- CSV write path (P4a-02, fixed): a CSV row gets Java's own primary-key check (`complete_csv_pk`,
  `entity/load_feed/loader.rs:508`): with the fields converted, a missing one-field key is sequenced and a missing key
  of a longer one fails the file. It is then stored as Java's `handleValue` (`store_value`, `:481`): created when
  missing under `onlyCreate`, an upsert under `useTryInsert` (not where the database is `never-try-insert`), and
  otherwise `createOrUpdate`; no entity-auto `fromDate` default, secondary sequence or status-flow check. XML and
  JSON records still use the entity-auto store, as Java's plain-map handler; the comment there now says so (`:466`).
- Bad type text (P4a-05, fixed): `convert_fields` (`entity/load_feed/loader.rs:618`) fails the record with Java's
  "The value [..] is not valid for type .. for field .." when the text is not of the field's type, for XML, JSON and
  CSV, and in a check. Number text is read as a BigDecimal first, as `L10nFacade.parseNumber`, so `118,500` is
  118500 and `1800.0` is a whole 1800 for a Long.
- Empty values (P4a-08, fixed): an empty CSV cell, an empty XML attribute, and a JSON `null` or `""` are null, so a
  store clears the column, and a `defaultValues` entry does not fill a field the record sets empty
  (`entity/load_feed/csv.rs:151`; `entity/load_feed/loader.rs:258`, `:770-784`; `entity/load_feed/json.rs:287`). An
  empty field element is still not a value, as Java's `if (currentFieldValue)`.
- Service names (P4a-09, fixed): XML, CSV and JSON all take any name `isServiceDefined` takes, or a `verb#noun`
  entity-auto name (`data_service_name`, `entity/load_feed/loader.rs:237`; `entity/load_feed/csv.rs:106`;
  `entity/load_feed/json.rs:120`).
- `checkInfo` and `messageNoActionFiles` (P4a-10, fixed): `check_info_with` (`entity/load_feed/check.rs:99`) gives
  Java's diff rows (`entity`, `pk`, `notFound`, `pkComplete`, `location`, with `createValues` or `field`, `value`,
  `dbValue`) and no message for them; the script data loader has `checkInfo()` and `checkInfo(diffInfoList,
  messages)`, which the DataImport screen calls. Each file a check reads is followed by `-- Checked data (N records)
  in {location}` unless `messageNoActionFiles(false)` and the file gave no messages (`check_file`, `:146`); XML, CSV
  and JSON text are named `xmlText`, `csvText`, `jsonText` and get no such line, as Java's.
- Short CSV rows (P4a-11, fixed): a row with fewer columns than the header fails the file
  (`entity/load_feed/csv.rs:143`).
- `options.components` (P4a-12, fixed): `load_entity_data` reads any location it is given; the host's conf
  `load-data` locations and the script loader's gathered conf locations are narrowed with `location_in_components`
  (`entity/load_feed/loader.rs:876`; `moqui-host/src/lib.rs:1257`; `script/runtime/tools_host.rs:1311`).
- Field element text (P4a-13, fixed): the XML reader parses each element with `MNode::parse_text_raw`
  (`moqui-lib/src/node/mod.rs:94`), so a field element's or service parameter's text is kept as written, whitespace
  included, as Java's SAX `characters` (`entity/load_feed/loader.rs:270`).
- Root elements (P4a-14, fixed): a root other than `entity-facade-xml` or `seed-data` is read as an `<entities>` file
  is: only the `<seed-data>` (type `seed`) and `<entity-facade-xml>` (its `type`) elements in it are data, and a file
  with none loads nothing, not an error (`load_embedded_document`, `entity/load_feed/xml.rs:41`).
- Boolean text (P4a-15, fixed): the loader's conversion takes only `true`, in any case, as true (`Boolean.valueOf`,
  `convert_text`, `entity/load_feed/loader.rs:655`); `moqui-lib`'s `to_bool` keeps `ObjectUtilities`' `y`. No shipped
  dictionary type is a Boolean.
- CSV column that is not a field (P4a-16, found and fixed): fails the file, as Java's field lookup does; it was
  dropped (`entity/load_feed/csv.rs:120`).
- The text `null` (P4a-17, found and fixed): is null for any field type, as `FieldInfo.convertFromString`
  (`entity/load_feed/loader.rs:655`).
- A CSV header name given twice (P4a-21, found and fixed): reads the later column, as Java's header map; the first
  was read (`entity/load_feed/csv.rs:94`).
- EECA entity names (P4b-01, fixed): a rule's `entity` is resolved to its full name, and a rule for an entity that is
  not defined is skipped with Java's warning (`EcaIndex::ingest`, `entity/load_feed/eca.rs:180`).
- EECA order (P4b-02, fixed): an entity's rules with no id run first, then those with an id, each in file order
  (`entity/load_feed/eca.rs:214-223`); Java's id'd rules are in `HashMap` order, which file order is one instance of.
- Data document merging (P4b-03, fixed): rows merge by document id only when every primary key field is a field alias
  (`has_all_primary_pks`, `entity/document.rs:60`, `:633`); otherwise each row is a document (`:863`), and its `_id`
  is made, as `makeDocId`, of the document's fields named for the key, so a one-field key that is not a field gives
  no `_id` (`doc_id`, `:127`). Before, the `_id` was always the full key, read from the query's own key aliases.
- XML writer count (P4b-05, fixed): an XML record counts its nested records too, as `plainMapXmlWriter`; JSON and CSV
  count each top record (`data_body`, `entity/query_rest.rs:1555`, `xml_record_count`, `:1583`;
  `write_rows`, `entity/data_writer.rs:472`).
- Writer directories (P4b-06, fixed): `file()` needs its parent directory and `directory()` creates only the last
  one (`mkdir`), else the write is a message error; the zip writes still create the parents, as Java's
  (`entity/data_writer.rs:254`, `:300-315`, `write_new`, `:575`).
- `defaultValues` merged into XML, JSON and CSV records (`entity/load_feed/loader.rs:770-791`).
- Per-location error isolation (log and continue) and one transaction per file (`entity/load_feed/loader.rs:104-134,
  188-199`).
- `ec.entity.getEntityDataDocument()` + `writeDocumentsTo*` bound for the shipped DataDocument screens
  (`script/runtime/tools_host.rs:266-281`).
- XML `binary-very-long` child-element Base64 decode with UTF-8 fallback (see P4a-19 for the trim); XML
  entity/field/relationship parsing, nested-record key inheritance, `<seed-data>`/`entity-facade-xml` type skipping.
- JSON array-vs-object roots, `_dataType` marker, per-element `_entity` dispatch; zip entry dispatch.
- `data_types` skip semantics; `only_create`; restricted-entity list; `fromDate` default and PK
  sequencing for XML/JSON; `check` messages, `lastUpdatedStamp` exclusion, PK display format.
- `SimpleEtl` skip rules and error accounting; feed pending registration, delete handling and
  reassembly; `get#DataFeedLatestDocuments`.
- Document control fields, `_index` lowercased, `manualDataServiceName`, expression fields,
  `postQuery`; writer CSV/JSON/XML framing; ETL `dummy_fks`, `extract`, `store_entity` fallback.

The feed's pending-table vs in-memory `DataFeedSynchronization` architecture is a documented
intentional difference and is not counted.

---

## Phase 5 — Entity query / REST

Reviewed `entity/query_rest.rs` (dynamic view build, REST find/list/create/store/update/delete,
path→PK resolution, master/dependent expansion, JSON/XML/CSV writers) with `entity/search_form.rs`,
`entity/sensitive.rs`, `entity/schema.rs`, `screen/dispatch.rs`, `web/mod.rs` and `moqui-db`
`model/{mod,view}.rs` against `EntityFacadeImpl.rest`, `EntityAutoServiceRunner`,
`EntityFindBase.searchFormMap`/`processInputFields`, `EntityDynamicViewImpl`, `EntityDefinition`,
`WebUtilities`, `ContextJavaUtil.jacksonMapper` and `RestSchemaUtil`. JSON-schema differences are
tracked once under P2-06/P2-07/P2-12.

Worked 2026-10-16: P5-01, P5-11, P5-13, P5-15 and P5-16 fixed (P5-16 in a later pass, once evaluation in moqui-core's
value layer was chosen); P5-17 (form auto fields of a view) found and fixed; P5-18 and P5-19 found and left open, and
P5-18 fixed in a later pass with P3-14, on the overlay of definitions built at runtime. The JSON wire change (P5-11,
P5-15) has no consumer in this repository that reads the old text: the browser code
(`runtime/base-component/webroot/screen/webroot/js`, `crates/moqui-browser/js`, `scripts/*.mjs`) parses with
`JSON.parse`/`response.json()`, `MoquiL10n.toInstant` already takes a millis number (the old quoted millis text was an
invalid date to it), the qvt screen data and `crates/moqui-browser` use `moqui_lib::json::to_json`, not this writer, the
`moqui test` specs run in-process, and only some Rust tests matched `"name": value` text (now `"name" : value`). Line
numbers in this section are as of that change.

| ID | Rust location | Java location | Divergence | Severity | Intentional? |
|----|---|---|---|---|---|
| P5-19 | `entity/schema.rs:145`; `entity/data_writer.rs:522-538` | `RestSchemaUtil.groovy:144-147`; `EntityDataWriterImpl.groovy:349-359` | The entity JSON schema and the data writer's CSV header and rows of a **view-entity** have no fields; Java uses the aliases (`getAllFieldNames`) | Med | No |

#### P5-19 — Entity schema and data-writer CSV of a view have no fields

The model keeps a view's fields as `aliases` and leaves `fields` empty. `entity/schema.rs` builds a schema's
`properties` from `entity.fields`, and the data writer's CSV header and rows (`entity/data_writer.rs`) from the same,
so a view's schema has only `_entity` and its CSV no columns. Java uses `getAllFieldNames`, the aliases. Found while
fixing P5-01 (the same cause) and left open as it belongs to the schema (P2-12) and writer (Phase 4) code; the fix
is to read `field_names`/`pk_field_names` and the alias types for a view, as `EntitySnap` and the form shape now do.
The CSV header with `tableColumnNames` would also need a column name for an alias.

#### Phase 5 checked and found equivalent

- `DataDocument.<id>` names (P5-18, fixed; line numbers as of that pass): the overlay of P3-14 (Phase 3) builds the
  document's view as Java's `makeEntityDefinition`: `DataDocument.{id}`, the primary entity as `PRIM` and each
  relationship an optional member with its key maps and its key values as a member condition (Java's
  `addRelationshipMember`), each field an alias named by `fieldNameAlias` with its `functionName` and `defaultDisplay`,
  and an expression field a `pq-expression` alias (P5-16), which a read evaluates and form auto fields show with
  `defaultDisplay` `Y` (`entity_view_node`, `entity/document.rs:773`; `document_view`, `entity/derived.rs:259`). As
  Java's `find`, a find on it has the document's conditions that are not `postQuery` and reads from a clone when one is
  open (`EntityFacade::find`, `entity/facade.rs:508`). As Java's `isEntityDefined`, the name is not an entity name. The
  aliases come in the order of the document's field tree, each member's fields before the members below it, where Java's
  tree keeps the order fields and relationships first appear; the document's own query (`code_view`) is unchanged. Never
  cached, as a P3-14 view.
- `pq-expression` aliases (P5-16, fixed; line numbers as of that pass): moqui-db only keeps the expression. As
  `EntityDefinition`, such an alias is kept apart from the view's aliases, in `pq_aliases`
  (`moqui-db/src/model/mod.rs:158`, `:715-723`; `parse_pq_alias`, `model/view.rs:261`), so no SQL builder sees it and
  it is not one of the view's fields, and its name keeps `alias-all` from the member field of the same name (`:478`);
  a REST dynamic view's alias spec with `pq-expression` is kept the same way (`pq_alias_from_spec`,
  `entity/query_rest.rs:1255`), and as there its rows are plain maps it only does that. moqui-core's value layer
  evaluates it lazily, where the localized reads are: a find's values carry the view's `pq-expression` aliases with
  their localized fields (`l10n_targets`, `entity/field_rw.rs:568`), and `get_localized`, `localized_fields` and a
  script's or template's read of a value (Java's `get` and `getMap`) evaluate it in the script runtime with the
  row's stored fields in the context, as Java's `valueMapInternal` (`eval_pq_alias`, `entity/value.rs:1202`). As
  Java's `get`, it runs on every read and is not remembered, since an expression may read more than the row (the
  user, the time, the context); only the localized lookups are (`read_localized`, `:170`). The evaluation runs outside
  the value's lock and the memo's, as the localized lookup (`LocalizedRead`, `:87`), so an expression that reads the
  same value does not deadlock. `fields()`, and so REST output, never has it, as Java's `getPlainValueMap`. A failing
  expression is logged with the alias and entity names and reads as null, where Java throws (accepted, P5-16 in
  [Accepted differences](#accepted-differences)). A script's read of a name that is both a `pq-expression` alias and a
  relationship gets the relationship, as Java's `get` (`held_value`, `script/runtime/entities.rs:1045`). Form auto
  fields show one with `default-display="true"` as a display field after the others, whatever `include`, the
  excludes and the field type, as `ScreenForm.addEntityFields` (`screen/form.rs:965-976`).
- REST find of a view-entity (P5-01, fixed): `EntitySnap` takes `field_names`/`pk_field_names`, the aliases for a
  view (`entity/query_rest.rs:282-311`), so a view row renders its alias values (`render`, `:737-747`), and a
  complete primary key of aliases, from the path or the parameters, finds one row as Java's `containsPrimaryKey`
  (`find_rest`, `:478`). The identity and secret stripping (`sensitive::strip_user_account_secrets`) goes by key at
  any depth, so view rows get it too. The query-REST CSV writer (`render_csv`) also gets a view's aliases.
- REST JSON dates (P5-11, fixed): `encode_json` (`entity/query_rest.rs:1739-1781`), which every web JSON response
  and JSON error goes through (`web/mod.rs:514`, `:525`, `:1263`), writes a `Timestamp` or `Date` as an unquoted
  epoch-millis number and a `Time` as `"HH:mm:ss"`, as Jackson with `WRITE_DATES_AS_TIMESTAMPS` and its
  `SqlTimeSerializer` (checked against jackson-databind 2.22.2). Before, all three were quoted millis text. A
  `Date` is midnight UTC, as everywhere in this port; Java's is midnight in the JVM zone. The data writer's JSON
  files are unchanged.
- REST JSON layout (P5-15, fixed): the same writer indents as Jackson's `DefaultPrettyPrinter`
  (`INDENT_OUTPUT`): object entries on their own lines two spaces deeper per enclosing object, `"name" : value`,
  arrays on one line as `[ a, b ]`, and `{ }`/`[ ]` when empty.
- `default-display` (P5-13, fixed): the model keeps it on an alias (`moqui-db/src/model/view.rs:134`, `:279`), as
  does a REST dynamic view's alias spec (`entity/query_rest.rs:1243`), and a form-list's auto fields honor it as
  `ScreenForm.addEntityFields` (`screen/form.rs:934-943`). As in Java, a dynamic view never reaches form auto fields,
  so on a REST dynamic view it has no effect.
- Form auto fields of a view-entity (P5-17, found and fixed): `read_entity_shape` (`screen/form.rs:862-915`) read
  `entity.fields`, so `auto-fields-entity` of a view (the Tools entity data screens with a view selected) gave no
  fields. A view's fields are now its aliases, with their types (`text-long` when none, as Java's `fi.type ?:
  "text-long"`) and primary-key flags.
- Identity-admin/secret-config read and write bans and `stripUserAccountSecrets` on every response
  (`entity/query_rest.rs:185-210`, `entity/sensitive.rs` mirrors `WebUtilities`).
- REST writes go through entity-auto (`write_rest`, `entity/query_rest.rs:233-254`), so status-flow checks
  and `oldStatusId`/`statusChanged` apply.
- REST field conditions via `search_form_map`: `_ic`, `_period`/`_poffset`/`_pdate`, date-only `_thru`
  padding, aggregate aliases to HAVING, unknown `_op` adds nothing.
- Multi-step relationship paths (`entity/query_rest.rs:415-436`); path→entity+PK resolution,
  `masterNameInPath` fallback, verb→operation mapping.
- Dynamic view `<complex-alias>`/`case`/`when`/`else`, alias attributes, member `<field>`, `alias-all`
  expansion; view `group-by`/`having` inference; `<order-by>`/`<entity-condition>`.
- Body nested-map/child recursion and parent-PK propagation; response shape, `X-Total-Count`/`X-Page-*`
  headers, error JSON and status codes; decimal formatting; REST authorization frame.
- Out of scope (not findings): Swagger/RAML, Elasticsearch/DataDocument `documentList`, JSON-RPC,
  runners, service-group, Hazelcast, LLM/A2A, extra drivers.

---

## Phase 6 — Service definition / validation / call / semaphore

Reviewed `service/{define,validate,info,call,semaphore}.rs` against `ServiceDefinition.java`,
`ParameterInfo.java`, `ServiceFacadeImpl.groovy`, `ServiceCallImpl.java`, `ServiceCallSyncImpl.java`,
`ServiceCallSpecialImpl.groovy` and the inline/entity-auto/script runners. Async is in Phase 7.

Worked 2026-10-16: P6a-01, P6a-03, P6a-04, P6a-06, P6a-07, P6a-08, P6a-09, P6a-10, P6a-11, P6a-13, P6a-14, P6a-15,
P6a-16, P6a-17, P6b-01, P6b-10, P6b-11 and P6b-12 fixed (P6b-01 once Java's record-and-return contract was chosen);
P6a-12 and P6b-14 moved to [Accepted differences](#accepted-differences). Found along the way: P6a-18, P6a-21 and
P6a-22 (fixed), P6a-19 and P6a-20 (the parts of
P6a-13 and P6a-08 that stay, accepted) and P6a-23 (fixed: the errors text that P6a-14's messages left without a
field). Two changes reach shipped services: parameters of `implements` now come first (an order change only: a dump of
the 37 `runtime/` services with `implements` gives the same name, type, `required`, checks and default for each
parameter as before), and an `auto-parameters` field merges with a parameter of the same name, so
`EntityServices.create#DataDocumentField` and `update#DataDocumentField`, whose `fieldPath required="true"` comes
before `<auto-parameters include="nonpk"/>`, no longer require `fieldPath`, as in Java, where the later
`auto-parameters` sets `required="false"`; in the other 7 such services the merged parameter keeps the type,
`required`, default and checks of the two before. Line numbers in this section are as of that change.

### Findings — 6a definition / validation / info

None open.

### Findings — 6b call / semaphore

None open.

#### Phase 6 checked and found equivalent

- Service-body failure (P6b-01, fixed: Java's record-and-return contract adopted 2026-10-16): as `callSingle`'s
  `catch (Throwable t)`, a failure of the body, its pre-service or post-service rules, or the commit is recorded in the
  message facade (the diagnostic with its Rust origin, then any cause its text does not show), the transaction rolls
  back, the callbacks hear the error (a commit failure: the results they heard before it), the post-commit rules run,
  and `sync` returns `Ok` with the results as they were, empty when the runner failed (`sync_with`,
  `service/call.rs:507-544`; `record_failure`, `:550-567`). `Err` stays for what Java throws: an unknown or interface
  service (the interface check now comes before the run, `:440-445`), authentication, a semaphore, a failing
  pre-validate or pre-auth rule, an authorization failure in the body (Java rethrows `ArtifactAuthorizationException`),
  and a failure with no message facade to record it in. An authorization failure in an inline or script body keeps its
  kind across the script, by the error property the screen runtime already used (`throw_diagnostic`, `mark_kind`,
  `caught_kind`, `script/runtime.rs:1260-1312`; the compiled-script catch paths), and a `service-call` the user may not
  make is thrown out of the calling body (`script/runtime/services.rs:287`), so REST answers 403 and nothing is
  recorded; the rollback is P6b-15. Callers read the message facade, as Java's read `hasError()`: the REST,
  screen-login, job, SSO, system-message and SSH paths already did; a data-file service's errors now fail its element
  and are cleared, as `handleService` (`service/eca.rs:213-217`), a manual data service's are logged and cleared, as
  `EntityDataDocument` (`entity/document.rs:970-985`); an authz service that leaves errors still fails the check
  (P6b-16; `sync_checked`, `service/call.rs:189-204`; `user/authz.rs:419`). A body failure under service REST is now
  Java's 400 with the errors, not a 500. A script's `call()` gets the empty map, as Java's, so an `out-map` now gets
  `{}` too.
- `validate="false"` (P6a-01, fixed): skips the `required` check, the `allow-html` check and the declared checks, as
  Java's `nestedParameterClean`, and still converts (`clean_param`, `service/validate.rs:124-170`); unknown
  parameters are kept and nested maps not trimmed, as before.
- Number `format` (P6a-03, fixed): a number parameter with a `format` is read by that `DecimalFormat` pattern for
  the session locale, as `BigDecimalValidator.validate(text, pattern, locale)` (`convert_text`,
  `service/validate.rs:436`; `l10n::parse_number_format`, `l10n.rs:365`; `format::parse_decimal_pattern`,
  `moqui-lib/src/format.rs:330`): its prefix and suffix or negative sub-pattern, grouping only when the pattern
  groups, `%` and per-mille scaled back. Text it does not read is Java's error naming the format.
- Known service names (P6a-04, fixed): `known_service_names` (`service/info.rs:27`) adds to the registered services
  the services of the `service-file` locations and of every service file under each component's `service`
  directory, read but not registered, as `getKnownServiceNames` (`service_names_in_files`,
  `service/define.rs:639-702`); a file that does not parse is logged and skipped. Like Java, the framework's own
  `service` directory is listed only through `service-file`.
- `implements` merge (P6a-06, fixed): the implemented services' parameters come first, then the service's own are
  merged in by name, as `ServiceDefinition`'s `mergeParameter`: an own parameter sets only the attributes it has (a
  parameter that adds a check keeps the interface's `type` and `required`), its checks are added and its nested
  parameters merged (`merge_param`, `service/define.rs:162-202`; `resolve_one`/`implemented`,
  `service/define.rs:1363-1455`). `implements required` also sets the `required` of an `auto-parameters` it copies.
  A `required="disabled"` implemented parameter is still left out (P6a-12), and of two implemented services with a
  parameter of the same name the first one's is kept (Java appends both and the later wins its map).
- `time-range` (P6a-07, fixed): text is read by the `format` and a value that does not read is not checked; the
  bounds are inclusive, and `now` is the effective time written and read back by the format, so `after="now"` with
  `MM/yyyy` takes this month (`time_range`, `service/validate.rs:735-775`; `bound_millis`, `:912-927`).
- Character classes (P6a-08, fixed): `text-letters` is Unicode alphabetic in the Basic Multilingual Plane, as
  `Character.isLetter` of a UTF-16 unit, `text-digits` the decimal digits there, as `Character.isDigit`
  (`is_java_digit`, `service/validate.rs:779`), and `text-email` follows `EmailValidator`'s user and host rules
  (`is_email`, `:794-875`); what stays different is P6a-20.
- SECA rule count (P6a-09, fixed): `getSecaRuleCount` counts a pattern rule once per known service it matches
  (`seca_rule_count`, `service/info.rs:39`; `SecaRule::service_count`, `service/seca.rs:299-310`). The startup log
  keeps Java's "SECA rules active" count of rules (`seca_rules_active`, `service/info.rs:48`), and a SECA load reads
  the known names at most once (`service_defined`, `service/seca.rs:282-297`).
- `field-name` (P6a-10, fixed): an `auto-parameters` field sets `field-name` (`auto_fields`,
  `service/validate.rs:424`).
- Empty `default-value` (P6a-11, fixed): no default, as `ParameterInfo` (`parse_parameter`,
  `service/define.rs:1237`); an empty `default-value` on an own parameter still clears an implemented one's.
- Each failed check (P6a-13, fixed): every check of a parameter that fails is its own validation error, and an HTML
  error does not stop the checks (`clean_param`, `service/validate.rs:161-170`; `apply_check`, `:576-731`); `val-and`
  reports each child; `val-or`/`val-not` are P6a-19.
- Message text (P6a-14, fixed): Java's localized messages: `Field cannot be empty`, `Value entered ({0}) could not be
  converted to a {1}` (with `using format [..]`), the HTML message, and each check's `Value entered …` text or its own
  `message` expanded with `pv`, `regexp`, `min`/`max` (`apply_check`, `service/validate.rs:576-731`). The field and
  service are on the validation error, not in its text; the errors text adds them (P6a-23).
- Errors text (P6a-23, found and fixed): the JSON `errors` of a REST or screen response (`web/mod.rs:778-781`,
  `screen/dispatch.rs:372-375`) and a script's `ec.message.errorsString` (`script/runtime/facades.rs:350-352`) are
  Java's `getErrorsString`: the errors, then each validation error as `toStringPretty` writes it, its message, `(`,
  `for field`, ` on form` and ` of service` with the pretty names, and no closing parenthesis, as Java
  (`MessageFacade::errors_text`, `ValidationError::to_string_pretty`, `context/message.rs:44-67`, `:132-156`); a
  validation error's message, which the errors list also holds, is written once. The lines are joined without
  Java's newline after the last. The browser still leaves a placed field error out of its toasts: `placeFieldErrors`
  in `WebrootVue.qvt.js` (`moqui.validationErrorText`) and `moqui-html.js` match the line in either form. Before,
  the line was the message alone, which since P6a-14 named no field.
- File override (P6a-15, fixed): reading a service file under a framework or component `service` directory reads
  the files of the same path in the others, and a service is defined by the last of them that has it, in the order
  framework then components, as `findServiceNode`; this holds for the `service-file` files read at startup too
  (`load_file`, `service/define.rs:844-946`; `service_roots`, `:620-634`). A file that does not parse loses only its
  own services.
- Numeric attributes (P6a-16, fixed; P6a-22, found and fixed): a `transaction-timeout`, `semaphore-timeout`,
  `semaphore-sleep` or `semaphore-ignore` that is not a whole number fails that service with a registration failure,
  where Java's `Integer.valueOf`/`Long.parseLong` fails its definition; before, the first was ignored and the others
  took their defaults (`service/define.rs:1006-1021`; `timeout_of`, `:1142`; `seconds_attr`, `:1164`).
- Implicit entity-auto (P6a-17, fixed): the implicit definition does not clean its parameters
  (`validate_input: false`, `service/define.rs:263-269`), so SECA rules and the runner get the map as it came, as
  `runImplicitEntityAuto`; the runner sets only the entity's fields, as `setFields` (`run_entity`,
  `service/call.rs:1001`).
- Declared entity-auto (P6a-18, found and fixed): a declared `type="entity-auto"` service kept every entity field
  and every map or list parameter beyond its declared parameters; it now keeps only those declared, as any
  validating service, so `create#UomConversion` (`auto-parameters include="nonpk"`) no longer takes a caller's
  `uomConversionId` (`clean`, `service/validate.rs:78-80`).
- `auto-parameters` merge (P6a-21, found and fixed): an `auto-parameters` field and a parameter of the same name were
  two parameters, each cleaned in turn, so an untyped parameter after a `Timestamp` field converted the value back to
  text; they are now one, merged by name in declaration order, as `mergeAutoParameters` (`expand_params`,
  `service/validate.rs:321-341`; the same in `typegen/services.rs:34-61`).
- Semaphore scope (P6b-10, fixed): an empty `semaphore-parameter` is `_NA_` and an empty value is `""`; only a missing
  or null value is `_NULL_` (`scope_value`, `service/semaphore.rs:237-245`).
- Semaphore text (P6b-11, fixed): Java's "An instance of service semaphore … is already running (thread [..], locked
  at ..) and it is setup to fail on semaphore conflict." and the wait form with its seconds, with the holder read
  from the row (`take`, `service/semaphore.rs:33-77`; `try_lock`, `:151-216`).
- Status transition text (P6b-12, fixed): `StatusFlowTransitionNotFoundTemplate` localized and expanded with the
  status ids and names, its default text when the seed data is not loaded (`status_transition_message`,
  `service/call.rs:1228-1243`).
- Nested `<parameter>` children parsed and validated (`service/define.rs:1245`, `service/validate.rs:84-118`).
- Implicit entity-auto skips authentication (`Authenticate::Off`, `service/define.rs:263-265`); an `AT_SERVICE`
  frame is still pushed but with `requires_authz=false`.
- Early "errors already in the facade" abort with `ErrorsAside` push/pop, and the rollback-only guard
  (`service/call.rs:330-350,484-498`).
- `verb_action` maps `get`/`search` to `view` (`service/call.rs:846`); entity-auto `delete` requires a full PK
  or `*` wildcard (`delete_entity`, `service/call.rs:1069-1112`) and returns an empty map.
- Service `type` default, `authenticate` values, `transaction`/`no-tx-cache` defaults,
  `no-remember-parameters`, interface pass-through; semaphore attribute defaults and none/fail/wait.
- `auto-parameters` include/exclude/required/allow-html; `default` precedence, requirement-after-
  defaults, empty-value handling; `min/max-include-equals`, `matches`/`number-*`/`text-length`/
  `credit-card`, any-scheme `text-url`, `allow-html` modes.
- Service-name construction and combined-camel-case resolution; transaction attribute mapping and
  timeout selection; input-validation no-throw/empty result; SECA phases and tx-hook registration.
- `multi(true)` row gathering; `authUsername`/`authPassword` login/logout; semaphore fail/wait,
  staleness, insert-race retry, keying by `semaphore-name`.
- Entity-auto `checkFromDate`, sequencing, `oldStatusId`/`statusChanged`, `StatusFlowTransition`;
  `store` create-vs-createOrUpdate; nested recursion and parent-PK propagation.
- Out of scope (not findings): JSON-RPC, Groovy/Java runners, `service-group`, Swagger/RAML, XML-RPC.

---

## Phase 7 — ECA / SECA / email ECA / pools / jobs / scheduler

Reviewed `service/{eca,eca_rules,seca,rule_actions,email_eca,security_checks}.rs` and
`service/{pool,request_pool,jobs,schedule}.rs` against `EntityEcaRule.groovy`, `ServiceEcaRule.groovy`,
`EmailEcaRule.groovy`, `ServiceFacadeImpl.groovy`, `ServiceCallAsyncImpl`/`ServiceCallJobImpl`/
`ScheduledJobRunner.groovy`, `ExecutionContextFactoryImpl`, `UserSecurityChecks.groovy` +
`Security.eecas.xml`, and `MoquiDefaultConf.xml`.

Worked 2026-10-16: P7a-03, P7a-06, P7a-10, P7a-11, P7a-12, P7b-01, P7b-02, P7b-05, P7b-07, P7b-08, P7b-10, P7b-11,
P7b-12, P7b-13, P7b-14 and P7b-15 fixed; P7a-05 fixed for `ALL_USERS` membership, and its permission half removed as
invalid: Java's `hasPermission` reads the `UserPermissionCheck` view, a join of the user's own `UserGroupMember` rows,
so a permission granted to `ALL_USERS` counts only for a user with a membership row for it, as Rust already did (a test
now holds both). P7a-07 and P7a-08 moved to [Accepted differences](#accepted-differences). Found along the way: P7a-13,
P7a-14, P7a-16 and P7b-17 (fixed), P7a-15 and P7b-16 (the parts of P7a-12 and P7b-14 that stay, accepted) and P7b-18
(accepted). Changes a deployment may see: an async call (`async_call`, `call_future`, `start_future`, a script's
`ec.service.async()`) of an unknown service, or by a caller not authorized for the service, is now an error where it is
made (in a script, an error message, as the script host reports a failed call), and its run on a worker has
authorization disabled; an async call in a sim session is skipped; shutdown runs the
queued async calls for up to 30 seconds; and the first scheduled job pass is 120 seconds after start. No `runtime/`
component has a SECA rule, so the live `parameters` (P7a-11, P7a-13) and the SECA phase order (P7a-03, P7a-16) change
only rules a deployment adds. The scheduler tests run their passes with `ServicePools::tick()`, not the thread, so none
waits on the initial delay and no test override was needed. Line numbers in this section are as of that change.

### Findings — 7a ECA / SECA / email / security

None open.

### Findings — 7b pools / async jobs / scheduler

None open.

#### Phase 7 checked and found equivalent

- Implicit entity-auto SECA phases (P7a-03, fixed): an implicit entity-auto call runs its `pre-validate` rules a second
  time where Java's `runImplicitEntityAuto` does, after the authentication check, and runs no `pre-auth` rules, which
  Java never reaches for it (`sync_with`, `service/call.rs:403-410`).
- `pre-auth` order (P7a-16, found and fixed): the `pre-auth` rules ran right after validation, before the
  `authUsername` login and the authentication check; they now run after both, as Java's, so they see the user the call
  logged in and do not run for a call that fails either (`service/call.rs:403-410`).
- `ALL_USERS` (P7a-05, fixed): `user_in_group` is true for `ALL_USERS` with no lookup and no user, as `isInGroupById`
  (`service/security_checks.rs:142-146`); a permission granted to `ALL_USERS` counts only with a membership row for
  it, as Java's `UserPermissionCheck` view (`user_has_permission`, `:162-164`).
- Membership dates (P7a-14, found and fixed): a membership or grant whose `thruDate` is now was closed; both ends are
  now inclusive, as `filterByDate` (`membership_open`, `service/security_checks.rs:219-234`).
- EECA and EMECA conditions (P7a-06, fixed): only the first child of `<condition>` is the condition, as
  `EntityEcaRule` and `EmailEcaRule` (`first_condition`, `service/rule_actions.rs:33-40`; used at
  `entity/load_feed/eca.rs:198` and `service/email_eca.rs:34`); SECA already did.
- `post-commit` results (P7a-10, fixed): the `post-commit` rules of a call that failed get an empty result map, Java's
  `result` as it began, instead of none (`service/call.rs:535-537`); the call returns that map (P6b-01).
- Live `parameters` and `results` (P7a-11, fixed; P7a-13, found and fixed): after each rule, the `parameters` and
  `results` maps in its frame are read back into the call's (`run_seca_rules`, `read_back`, `service/seca.rs:47-102`), so a
  `pre-validate` rule changes what is validated (the input is copied only when there are such rules,
  `service/call.rs:352-356`), a `pre-auth` or `pre-service` rule what the service runs with, and a `post-service` or
  `post-commit` rule what is returned. A rule that sets a plain name still sets it only in its own frame, as Java's.
- `tx-rollback` registration (P7a-12, fixed): the `tx-rollback` rules are registered once the runner ran, with its
  results (empty when it failed), as Java's `finally` after `runService`, so a call stopped before it runs registers
  none (`service/call.rs:436-442`). The `tx-commit` rules are still registered after the `post-service` rules, so a write there
  can still register the feed hook first, but with the parameters and results as they were before those rules, the
  copies Java registers (`service/call.rs:443-444`); they run only when the transaction commits, so Java registering
  them on an error as well changes nothing. Pattern rules are P7a-15.
- Scheduler (P7b-01, P7b-02, fixed): `scheduled-job-check-time="0"` starts no job runner, with Java's warning: no job
  passes run, also from `tick()`, and `runner_stats` is `None`, so the tools screens show "No Service Job Runner
  active" (`settings`, `service/pool.rs:765-778`; `runner_stats`, `:413-419`; `tick`, `:567-581`). The first job pass
  is 120 seconds after start, then one every check time, as `scheduleAtFixedRate(jobRunner, 120, rate)`; the tick
  hooks, this port's own (the cluster heartbeat), keep running every interval from the start (`Clock`,
  `service/schedule.rs:66-96`).
- Shutdown (P7b-05, fixed): as `workerPool.shutdown()` and `awaitTermination(30s)`, the pool takes no new work, the
  scheduler and job workers end, and the service workers run the queued calls for up to 30 seconds; what is left then
  fails as before, with Java's "NOT YET terminated" warning (`shutdown`, `service/pool.rs:616-635`; `drain`,
  `:1137-1160`; `next_work`, `:921-939`). On one of the pool's own workers it does not wait. Queued job runs are
  abandoned at once, as before (Java never shuts its `jobWorkerPool` down).
- Async queue-time check (P7b-07, P7b-08, fixed): before an async call is queued, as the caller, an unknown or
  interface service is an error, the parameters are converted and validated, the service is pushed and popped to
  check authorization (no artifact hit), and `authUsername` is added, as Java's `validateCall`; the worker logs in the
  caller and runs the call with authorization disabled, as `AsyncServiceInfo` (`prepare_async`,
  `service/call.rs:256-313`; `submit`, `service/pool.rs:529-565`; `submit_future`, `:672-701`; `invoke`, `:998-1015`;
  `call_now`, `:1019-1042`). Input that fails validation queues nothing (P7b-18).
- Async in a sim session (P7b-17, found and fixed): an async call in a sim session ran on a worker, outside the sim,
  against the real database; it is now skipped with Java's log line, and a future gets `simSkipped` and `serviceName`,
  as `ServiceCallAsyncImpl` (`prepare_async`, `service/call.rs:275-281`).
- A run that cannot be started (P7b-10, fixed): once the pass has taken the lock, a run whose parameters cannot be
  read or that the job queue does not take is a failed run, as Java's `ScheduledJobRunner`: the lock is cleared and
  keeps this pass's run time, and the `ServiceJobRun` row records the error with start and end times; before, the row
  was deleted, and a failure reading the parameters left the lock held (`dispatch`, `service/jobs.rs:261-322`).
- Job dates and `paused` (P7b-11, P7b-12, fixed): a job whose `thruDate` is now is still in date, as `filterByDate`
  (`within_dates`, `service/jobs.rs:822-827`), and a `paused` that is not null or `N`, an empty one too, pauses the job
  (`paused_ok`, `:829-835`).
- Queue default (P7b-13, fixed): with no `worker-queue`, the queue holds 65536, Java's code default
  (`service/pool.rs:44-45`); `MoquiDefaultConf.xml` sets 65535 in both.
- Run host (P7b-14, fixed): a job run records `hostAddress` and `hostName`, from the address Java's
  `InetAddress.getLocalHost()` would give (`server_address`, `context/hits.rs:520`), with `runThread` and `startTime`
  (`run_job`, `service/jobs.rs:463-475`). The thread's name is P7b-16.
- Runner counters (P7b-15, fixed): a pass that failed or panicked still counts in the runner's counters, as Java's
  (`run_scheduled`, `service/jobs.rs:89-113`).
- SECA/EMECA discovery per component, id-overwrite, priority ordering; SECA rules for undefined
  services skipped at load (`service/eca_rules.rs:105-113`, `service/seca.rs:282-297`); SECA uses only the first
  `<condition>` child (`service/seca.rs:209-213`).
- Runtime name matching; `run-on-error` gating for SECA; phase set/ordering and tx hooks; tx rules run
  on a worker in a new transaction and skip sim sessions (`service/call.rs:607-652`); condition-false skip.
  SECA context carries `parameters`/`results`, the call's own (P7a-11).
- Action dispatch `service-call`/`entity-*`/`script`/`in-map="context"`; email ECA context fields.
- Security-check logic: empty-id, `authzDisabled`, privileged/sealed membership, date-ranged lookups,
  error strings.
- Registered callbacks run (`call_callbacks`, `service/call.rs:469`).
- Jobs run on a separate job queue sized by `job-pool-core` (`service/pool.rs:260-263,867-894`); job topic
  notification plus `ServiceJobError` to the user and `ServiceJobUser` recipients (`service/jobs.rs:602-671`).
- Cron parsing, lock-claim concurrency, `expireLockTime`/`minRetryTime`/`transactionTimeout` defaults,
  expired-lock run, `repeatCount` pause, `lastSchedule < lastRunTime` skip, min-retry-after-error;
  run-history write and clipping; `run_job_now`; parallel-count accounting.
- Out of scope (not findings): JSON-RPC, Groovy/Java runners, `service-group`, Swagger/RAML,
  Elasticsearch, Hazelcast cluster job coordination, LLM/A2A, extra drivers. `request_pool.rs` has no
  Java counterpart.

---

## Phase 8 — REST/remote/tools, mail, and component services

Reviewed `service/{rest,remote,tools}.rs`, `service/mail/{implementation,mod}.rs`, and
`service/{sso,elfinder,images,spreadsheet}.rs` + `service/system_sync/*` against `RestApi.groovy`,
`ServiceServices.xml`, `RemoteRestServiceRunner.groovy`, `PrintServices.xml`, `EntityServices.xml`, the
mail Groovy resources and `EmailServices.xml`, `ElFinderConnector.groovy`, `EntitySyncServices.xml`,
`SystemMessageServices.xml`/`WebFacadeImpl.handleSystemMessage`, `SftpMessageServices.xml`,
`AuthServices.xml`/`AuthenticationFlow.groovy`/`MoquiSecurityGrantedAccessAdapter.groovy`, and the
image service XML. The `*.rest.xml` HTTP route/CORS layer lives in `screen/dispatch.rs` /
`web/mod.rs`.

The component Java sources are in the checkout under `/home/jonesde/work/moquiwork/moqui/runtime/component/`. The
print bodies are inline Groovy on cups4j in `moqui-cups/service/org/moqui/impl/PrintServices.xml`; they were compared
against moqui-cups on 2026-10-16, with cups4j's own behavior (its builder defaults, what `CupsPrinter.print` sends,
how it parses a job's attributes) read from the `cups4j-0.8.3.jar` in `moqui-cups/lib` with `javap`. The image
services are `moqui-image/service/org/moqui/impl/ImageServices.xml`, already used here.
`org.moqui.impl.SpreadsheetServices` (read/load) has no Java counterpart: it is a port addition, and `moqui-poi` has
only the xlsx screen render (`ScreenWidgetRenderExcel.groovy`, `FormListExcelRender.groovy`), which is screen scope.

Worked 2026-10-16 (8a): P8a-03, P8a-04, P8a-06, P8a-08, P8a-11, P8a-12,
P8a-13, P8a-15, P8a-18, P8a-19, P8a-20, P8a-21, P8a-22 and P8a-24 fixed; P8a-17 moved to
[Accepted differences](#accepted-differences). Found along the way: P8a-25, P8a-27, P8a-28 and P8a-29 (the parts of
P8a-21, P8a-20, P8a-24 and P8a-11 that stay, accepted) and P8a-26 (fixed once its merge rule was decided, below).
For P8a-08 the shipped callers were checked first: the only `remote-rest` service in the tree, the browser smoke
`get#Echo`, is answered with a JSON object, so none depends on the lenient body handling. Changes a deployment may
see: a `/rest/s1` service result has no `null` out-parameters, no `null` fields one map down, and no `UserAccount`
password fields; a POST with `X-HTTP-Method-Override` reaches the overriding method on `/rest/s1` and is authorized for
it; a `remote-rest` call whose 2xx body is not a JSON object or array, an empty one too, now fails; its GET query
carries empty and list values; a `method="HEAD"` or `"OPTIONS"` is sent as that method (and fails with parameters, which
it cannot carry as a body); a `<method type="GET">` (upper case, outside the XSD) is no longer served; and the service
OpenAPI (`/rest/service.swagger/<root>`) now documents each service method's parameters and response. Its entity methods
still carry only a summary, as before (Swagger and RAML are out of scope, below).

Worked 2026-10-16 (8b and 8c): P8c-03, P8c-07 and P8c-16 fixed; P8b-03, P8b-04, P8b-06, P8b-07, P8b-09, P8c-05,
P8c-06 and P8c-17 moved to [Accepted differences](#accepted-differences); P8c-11 fixed once its format was decided,
below. Removed as invalid: P8b-10, since commons-email's `buildMimeMessage` sets the `Subject` only for a subject that
is not empty, as Rust does; and P8c-02 and P8c-04, since an elFinder entry here is a `DbResource` or a file path (any
other location is an error), which Java's `supportsWrite()` answers true for, so its check never refuses and the `write`
flag is the same. For P8b-03 the readers of the stored addresses were checked first: `send#EmailMessage`
(`send_message`, as Java's `sendEmailMessage.groovy`) splits `toAddresses`, `ccAddresses` and `bccAddresses` on commas,
and no screen or service in `runtime/` reads them otherwise. For P8b-09, none of the email body screens in `runtime/`
sets `render-modes`, so each has a text render. Changes a deployment may see: an elFinder command on the root says
"Operation not allowed on the root", and a hash of only the volume ID is the root; an SSO profile claim of `0` is not
mapped to an account field; and an SSO logout ends the local login even when the provider's end-session URL cannot be
had (no SSO client, a failed discovery). Line numbers in this section are as of that change.

Decided and worked 2026-10-16: P8a-26 and P8c-11 fixed. Changes a deployment may see: the service OpenAPI's
parameter schemas carry the parameter's `<description>`; and `convert#Image` writes the original as
`outputImageFormat` (a png, gif, bmp or tiff original was kept in its own format), so the file its caller names
`..._Original.${outputImageFormat}` is in that format. Line numbers for these two are as of that change.

Worked 2026-10-16 (print, against moqui-cups): P8a-30 through P8a-36 fixed; P8a-37 through P8a-40 moved to
[Accepted differences](#accepted-differences); P8a-41 left open for a decision (below). Changes a deployment may see:
the PrintJob screen's Send (`send#PrintJob`) and its REST method send the job, where each such send failed with
"Could not find NetworkPrinter []" and left the job as it was; a job with no copies is sent as one copy and one with
no duplex as one-sided, as cups4j sends them; page ranges that are not page numbers fail the send (`PtjSendFailed`)
rather than printing every page; a job with no document fails its send; `get#PrintJobDetailsFromServer` gives
`jobUrl` and `printerUrl` as `http://` URLs; a hold, release or cancel of a job the server never took is an error
rather than an info message; and a screen document's default job name lists the `screenParameters` values in the
order given. Line numbers for these are as of that change.

### Findings — 8a REST / remote REST / tools / print

| ID | Rust location | Java/XML location | Divergence | Severity | Intentional? |
|----|---|---|---|---|---|
| P8a-41 | `moqui-host/src/ipp.rs:312-362` (`print`); `print.rs` (`PrintDocument`) | moqui-cups `PrintServices.xml` `send#PrintJobInternal`; cups4j `PrintJob.Builder`, `CupsPrinter.print` | Every Java job asks for `orientation-requested` portrait and `output-mode` monochrome, cups4j's builder defaults, which the Groovy never changes; Rust sends neither, so the queue's defaults apply (color on a color printer) | Med | Unsure |

#### P8a-41 — Portrait and monochrome on every job

cups4j's `PrintJob.Builder` starts with `portrait = true` and `color = false`, and `CupsPrinter.print` always adds
`orientation-requested:enum:3` (or `:4`) and `output-mode:keyword:monochrome` (or `:color`); moqui-cups sets neither,
so every Java job asks for portrait, monochrome output. Rust's `PrintDocument` has no such fields and the IPP client
sends neither. Left open for a decision: matching Java greys every job on a color printer and fixes the orientation,
which a deployment moving from Java has today but may not have chosen; keeping the queue's defaults would be an
accepted difference. Setting either per job (a `PrintJob` field or a `print#DocumentInterface` parameter) would be an
addition beyond Java.

### Findings — 8b mail / email services

None open.

### Findings — 8c SSO / elFinder / system sync / images

None open.

#### Phase 8 checked and found equivalent

- Service results stripped of secrets (P8a-03, fixed): a `/rest/s1` service result goes through
  `strip_user_account_secrets`, as Java's `RestResult`, so the `UserAccount` password fields are left out of every map
  in it at any depth: an out-parameter that is an account, a list of them, or one nested further (`run_service`,
  `service/rest.rs:585-594`).
- Method override (P8a-04, fixed): `/rest/s1` takes `X-HTTP-Method-Override` on a POST as `/rest/e1` does, and the
  method it names is the one the path frames are authorized for (`AT_REST_PATH` view, create, update or delete) as
  well as the one that runs, as Java's `effectiveRequestMethod` in both `getActionFromMethod` and `runByMethod`
  (`effective_verb`, `screen/dispatch.rs:333-345`; used at `:87`, `:155`). An override cannot reach a method the
  user may not run: a view-only user's POST with `PUT` is refused at the path.
- Resource description (P8a-06, fixed): read from the `description` attribute, as written in the real `*.rest.xml` and
  the XSD, as Java (`service/rest.rs:356`); the tools dashboard's tooltip and child links show it.
- Remote-REST body (P8a-08, fixed): a 2xx body that is a JSON object is the result and an array is `response`; any
  other, an empty body too, fails the call with Java's "Error parsing JSON response from request to ...", the URL
  redacted (`response_map`, `service/remote.rs:174-183`; `:100-101`).
- Result nulls (P8a-11, fixed): as `nestedRemoveNullsFromResultMap`, the null out-parameters, and the null fields of
  each map out-parameter and of the maps that lead a list, are left out (`remove_result_nulls`,
  `service/rest.rs:596-615`); the plain-map part that stays is P8a-29.
- Remote-REST error text (P8a-12, fixed): "Remote service error 503: Service Unavailable", with the standard reason
  phrase of the status (`service/remote.rs:92`; `reason_phrase`, `outbound.rs:183-216`, shared with the script
  `rest()` host, which already used it); the transport does not pass on the server's own phrase.
- GET query (P8a-13, fixed): as `parametersMapToString`, every parameter but a null one, an empty value as `name=` and
  a list as its items joined by commas (`encode_query`, `service/remote.rs:149-160`). Java's stray `&` for a null is
  not copied, and a space is `%20` where `URLEncoder` writes `+`, which servers read the same.
- HEAD and OPTIONS (P8a-15, fixed): sent as themselves, as Java's `RestClient.Method`, where they fell back to POST
  (`outbound.rs:28-72`; `moqui-lib/src/http.rs:57-107`); with parameters, which they cannot carry as a body, the call
  fails before it is sent, as `RestClient.text` refuses one (`service/remote.rs:71-75`). The script `rest()` host
  takes them too.
- `require-authentication=""` (P8a-18, fixed): an empty value inherits the parent's, as Java's `?:`, on a resource,
  an id and a method (`service/rest.rs:311-315`, `:368-371`).
- Method `type` (P8a-19, fixed): methods are keyed by `type` as written, as Java's `methodMap`, so a `type="GET"`,
  outside the XSD's lower-case list, is never matched (`service/rest.rs:326-344`).
- A method that does not load (P8a-20, fixed): an operation other than `service` or `entity` is logged and that method
  left out, and the rest of the file is served, as Java registers no handler for it (`service/rest.rs:326-344`); what
  else is left out where Java does otherwise is P8a-27.
- DataDocument notifications (P8a-21, fixed): the `DataDocument` and its links are read through the entity cache, as
  `useCache(true)`, and user ids are looked for in the maps of a list but not in a list in a list, as
  `findAllFieldsNestedMap` (`find_nested`, `service/tools.rs:615-632`; `:669-687`). The empty id is P8a-25.
- Tools dashboard counts (P8a-22, fixed): each resource has `childPaths`, Java's count of the resources and ids under
  it, beside `childMethods` (`path_count`, `service/rest.rs:163-169`). Java's `methodMap` of handler objects is read
  only by its LLM browse tool (out of scope) and is not built.
- Service OpenAPI parameters (P8a-24, fixed): a service method in `/rest/service.swagger/<root>` has its path
  parameters, its other in-parameters as query parameters of a method with no body (not objects or arrays) or as the
  JSON `requestBody` of a `post`, `put` or `patch`, and its out-parameters as the `200` response, each with Java's
  `getJsonType`/`getJsonFormat`, its default, nested parameters, and the `enum` Java's `addParameterEnums` gives a
  parameter with `entity-name` and `field-name`, through P2-07's `FieldEnums` (`service_parameters`,
  `service/rest.rs:900-979`; `param_schema`, `:1009-1047`; `parameter_enums`, `:1049-1067`). The schemas are inline
  in OpenAPI 3.1 form where Java's Swagger 2 refers to `definitions`; a `required="disabled"` parameter is left out,
  as validation drops it (P6a-12). P8a-28 is what stays.
- Parameter descriptions (P8a-26, fixed: decided 2026-10-16): a parameter's `<description>` text, trimmed, is kept on
  `ParamDef` (`service/define.rs:155`, `:1251-1255`) and is the `description` of its OpenAPI schema, as
  `getJsonSchemaPropMap`, and of a path parameter's schema, where Java's Swagger 2 puts it on the parameter
  (`param_schema`, `service/rest.rs:1021-1023`; `:936-938`). In `merge_param` a parameter's own description replaces
  the inherited one and one with none keeps it, as Java's `mergeParameter` replaces the `<description>` child, for an
  `implements` and a redeclared or auto-parameter alike (`service/define.rs:200-202`). An auto-parameter has none: the
  entity model keeps no field `<description>`, and Java's `mergeAutoParameters` copies none (`auto_fields`,
  `service/validate.rs:426-427`).
- The "Original" image format (P8c-11, fixed: decided 2026-10-16): `convert#Image` writes the original as
  `outputImageFormat` when that format can be written, as its one caller (`WeCreateServices.xml`) names the file
  `..._Original.${outputImageFormat}`, and in the input's format otherwise (`convert_image`,
  `service/images.rs:318-327`). Java keeps a PNG, GIF, BMP or TIFF original in its own format and converts a JPEG
  one, as its check compares the reader's format name with the writers' suffixes (impl `ImageServices.xml:109`); the
  module doc says so.
- `send#PrintJob` (P8a-30, found and fixed): the `PrintJob` it finds reaches `send#PrintJobInternal` as an entity value,
  which the body read only as a map, so every send by id (the PrintJob screen's Send, the `send` method in
  `moqui.rest.xml`) failed with "Could not find NetworkPrinter []" and recorded nothing on the job; the job is read as
  a map or an entity value (`send_print_job_internal`, `service/tools.rs:336-339`).
- Copies and duplex (P8a-31, fixed): as the Groovy on cups4j's `PrintJob.Builder`, a job with no copies, or 0, is sent
  as one copy and a negative count sends none, and a `duplex` other than `Y`, none too, is `sides` one-sided (`Y` is
  two-sided-long-edge); Rust sent neither attribute for a job without the field (`send_job`, `service/tools.rs:398-412`).
- Page ranges (P8a-32, fixed): empty text and `1-` send no `page-ranges`, as cups4j; each comma item is a page or a
  range; text that is not page numbers (`x`, an open `3-`) fails the send with a message, recorded as `PtjSendFailed`,
  as cups4j's `Integer.parseInt` throws into the Groovy's `catch`, where Rust printed every page (`page_ranges`,
  `moqui-host/src/ipp.rs:247-270`).
- No document (P8a-33, fixed): a job with no `document` (a `send#PrintJob` of a record not stored) is a failed send,
  "PrintJob [id] has no document", as cups4j's builder cannot be made from a null document, where Rust sent an empty
  one (`service/tools.rs:387-396`).
- Job URLs (P8a-34, fixed): `jobUrl` and `printerUrl` are `job-uri` and `job-printer-uri` with `ipp://` made `http://`,
  as cups4j's `PrintJobAttributes` URLs, where Rust gave the `ipp://` URIs (`http_url`, `moqui-host/src/ipp.rs:273-275`,
  `:398-400`).
- Hold, release or cancel of a job not sent (P8a-35, fixed): a job with no `jobId` fails the call with "PrintJob [id]
  has not been sent to the print server", as `get#PrintJobDetailsFromServer` does and as Java's `(int) printJob.jobId`
  throws, where Rust left an info message and the call succeeded (`job_action`, `service/tools.rs:613-619`).
- Screen job name (P8a-36, fixed): the default `jobName` is the screen name and the parameter values in the order
  `screenParameters` gives them, as Java's `[:]` keeps them, then those of `screenParametersStr` in key order, as
  `eval_map` returns a sorted map; all were in key order (`print_screen_document`, `service/tools.rs:193-204`,
  `:245-252`).
- Print, found equivalent against moqui-cups: `get#ServerPrinters` finds the server's records by `serverHost` and
  `serverPort` (default 631), returns their ids and those of the records it makes, and makes one for each server
  printer whose name no record has, with the host, port, `printerName`, `description` (`printer-info`) and `location`
  (`printer-location`), as cups4j's `CupsPrinter` (`get_server_printers`, `service/tools.rs:452-495`). A create that
  fails stops the loop with its error, as the XML `service-call` returns; AGENTS.md failure tolerance gains nothing
  there, since the error rolls back the call's transaction and the records it made.
- `make#CupsPrinter`: the `NetworkPrinter` by id ("Could not find NetworkPrinter [id]"), then the server printer of its
  `printerName` from CUPS-Get-Printers, or CUPS-Get-Default when it names none ("Print not found with name [...] on
  server [...] for NetworkPrinter [...]"), from `http://host:port`, cups4j's `toURI`; a job is posted to the server at
  the path of the printer's `printer-uri-supported`, as cups4j's printer URI is the server URL plus that path
  (`make_printer`, `service/tools.rs:295-322`; `print`, `moqui-host/src/ipp.rs:312-362`).
- Job attributes: `requesting-user-name` is the job's `username`, else the process user (`$USER`, else `anonymous`, as
  cups4j's `user.name` default); `job-name` only when the job has one; copies, sides and page ranges as above; and no
  resolution, media or media source, which the Groovy never sets.
- `send#PrintJobInternal`: refused in a sim session with Java's message; taken, the job is `PtjPending` with the
  server's `jobId`; refused (an IPP status that is not successful, as cups4j's `isSuccessfulResult`) or failed, it is
  `PtjSendFailed` with the `jobId` and the message as `errorMessage`; each update is in its own transaction, and the
  error is left in the message facade with the results returned, under P6b-01's record-and-return contract.
  `print#Document` creates the `PtjNotSent` job in its own transaction first, so it stays whatever the send does, and
  returns `printJobId` and `jobId` (`service/tools.rs:116-150`, `:327-373`).
- `print#ResourceDocument`: "Could not find resource at [...]" for a missing location; the bytes are printed and not
  stored, with the content type of the location's name (`service/tools.rs:153-175`).
- `get#PrintJobDetailsFromServer`: the job's non-PK fields and the server's `completeTime` and `createTime` (seconds to
  timestamps), `jobUrl`, `pagesPrinted` (`job-media-sheets-completed`), `printerUrl` and `size` (`job-k-octets`); the
  job state names map to `PtjAborted`, `PtjCanceled`, `PtjCompleted`, `PtjPending`, `PtjPendingHeld`, `PtjProcessing`
  and `PtjProcessingStopped` as Java's `stateMap`, and a changed status is saved with `update#PrintJob`, which checks
  the status flow; a job not sent, or one the server does not know, is an error in both (`status_of_state`,
  `get_print_job_details`, `service/tools.rs:498-568`).
- Hold, release and cancel: Hold-Job, Release-Job and Cancel-Job as the process user; done (an IPP success status, as
  cups4j's `isSuccessfulResult`), the job moves to `PtjPendingHeld`, `PtjPending` or `PtjCanceled` through
  `update#PrintJob`; not done, Java's info message "Could not hold job [...] on print server at [...] for PrintJob
  [...]" and no change (`job_action`, `service/tools.rs:592-638`). None of the print services has a SECA or EECA, in
  the framework or in moqui-cups.
- elFinder root refusal (P8c-03, fixed): `rm`, `rename`, `upload` and `put` on the volume root answer Java's
  "Operation not allowed on the root" (`ROOT_REFUSED`, `service/elfinder.rs:45`).
- Volume-ID hash (P8c-16, fixed): a hash of only the 3-character volume ID decodes to the empty path, the root, as
  Java's `unhash`; `rm` of it is refused as the root, and a hash shorter than the ID is still an error (`unhash`,
  `service/elfinder.rs:756`).
- SSO field mapping (P8c-07, fixed): a mapped claim is taken by Groovy truth, as `if (srcFieldValue)`, so `false`, an
  empty value and a zero number are not mapped, and the text `"0"` is (`groovy_truth`, `service/sso.rs:353`, `:383-392`).
- 8b and 8c, found equivalent this round: an empty expanded subject leaves out the `Subject` header in both, as
  commons-email sets it only when not empty (P8b-10, `service/mail/implementation.rs:985`); and the elFinder `write`
  flag and the `mkdir`/`mkfile` `supportsWrite()` check (P8c-02, P8c-04), since every entry is a `DbResource` or a
  file, which `supportsWrite()` answers true for (`service/elfinder.rs:319`, `:636`).
- 8a: identity-admin read ban and identity/secret write bans with UserAccount input stripping
  (`service/rest.rs:641-656`); entity results stripped and returned without `_entity` (`service/rest.rs:664,682-692`;
  master path keeps it, as Java); `count` via `search_form_map` with `X-Total-Count`; remote-REST
  `method` attribute honored (`service/call.rs:886-894`); `rewrite#EntityEncryptedFields` logs and skips
  records that won't decrypt or update (`service/tools.rs:785-807`).
- 8a: root/path 404s and error kinds; `*.rest.xml` discovery and root replacement; named-child
  precedence; `allow-extra-path`; `AT_REST_PATH` frame/authz; `anonymous-all`/`anonymous-view`; entity
  operation set and `{operation}#{entity}` dispatch; find-one-not-found → null; default list limit 100
  and pagination headers; `masterName`; remote-REST location/error/GET-vs-body handling; print
  services; `send#DataDocumentNotifications`; `rewrite#EntityEncryptedFieldsAll`.
- 8b: draft lifecycle; cc/bcc merge; allowed-host DNS-suffix; SMTP auth/TLS/port; attachment branches;
  IMAP/POP3 protocol set and config; Email-ECA field mapping; SIM-session skip.
- 8c: elFinder hash/unhash, response fields and command set; SSO service names/params/flows, account
  sync, profile mapping, OIDC/OAuth config, failure redirect; `login` refusing disabled/inbound flows
  matches Java's callback `buildAll()` filter; entity-sync lock/guards/status/history — including the
  early return when `internalRun` hits an error, which Java's XML `service-call` also does
  (`XmlActions.groovy.ftl:69`); `get#EntitySyncData` split loop, include-list expansion, put options;
  system-message REST send/receive and missing-HMAC 403; SFTP send/poll; image convert/write
  (`ImageServices.xml`). Spreadsheet read/load is a port addition with no Java counterpart (above).
- Out of scope (not findings): JSON-RPC, `RemoteJsonRpcServiceRunner`, Groovy/Java runners,
  `service-group`, Swagger/RAML (Rust's `openapi_document` is an addition), Elasticsearch, Hazelcast,
  LLM/A2A, extra drivers.

---

## Accepted differences

Moved to [Parity-Diffs](../Parity-Diffs.md) (66 rows, grouped by area, with the same IDs) when this review was archived.
