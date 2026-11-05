# MySQL issues

Resolved 2026-11-04. The live part is in [Run-Deploy](../../docs/Run-Deploy.md#external-postgres-or-mysql) (utf8mb4,
which conf for MariaDB) and in the driver's acceptance tests (`crates/moqui-drivers/src/mysql/driver/network_tests.rs`).

On 2026-10-31, loading every data type (seed through demo, all the components in `runtime/component`) into MySQL 8.4
from the `mysql:8` image stored 29,609 records in 216 seconds. It skipped 10 data files and did not create 2 tables.
After the fixes below, the same load stores 33,356 records in 132 seconds, with no error or skipped file. The same holds
on MariaDB 11.4 with the `mysql` conf: 33,356 records in 71 seconds.

## How it is run

```sh
docker run -d --rm --name moqui-mysql-test -e MYSQL_ROOT_PASSWORD=moqui -e MYSQL_DATABASE=moqui \
    -p 127.0.0.1:33306:3306 mysql:8
entity_ds_db_conf=mysql8 entity_ds_host=127.0.0.1 entity_ds_port=33306 entity_ds_database=moqui \
    entity_ds_user=root entity_ds_password=moqui entity_ds_sslmode=disable entity_ds_crypt_pass=mysql-tryout-key \
    ./moq load --debug
docker stop moqui-mysql-test    # --rm removes the container and its volume
```

- **MariaDB:** use `mariadb:11.4` with `MARIADB_ROOT_PASSWORD` and `MARIADB_DATABASE`, and `entity_ds_db_conf=mysql`.
  `mysql8` writes `LATERAL`, which MariaDB does not have.
- **Package:** the Dev package, which `./moq load` builds by default, has no MySQL driver. The Debug, Maximal and
  Enterprise packages have it.
- **Tests:** the tests that need a server read `MOQUI_TEST_MYSQL=user:pass@host:port/database`. Give them a database
  apart from the load's, as the race test leaves rows a later load cannot add foreign keys over.
  - `cargo test -p moqui-drivers --features mysql -- mysql` (these skip without a server)
  - `cargo test -p moqui-drivers --features mysql --lib -- --ignored mysql::` (the network bounds, about 15 s; also run
    by `./moq accept-all` when `MOQUI_TEST_MYSQL` is set)
  - `cargo test -p moqui-host --test it -- gate1::mysql --ignored`
  - `cargo test -p moqui-host --test it -- create_or_update_race --ignored`

## What was found and how it was fixed

| # | Issue | Fix |
|---|---|---|
| 1 | `EMAIL_MESSAGE` over MySQL's 65,535 byte row limit: no table, every email send failed | `text-intermediate` and `text-long` are `TEXT` on MySQL, as on Postgres, stored off the row. Under utf8mb4 this also keeps `EmailOAuthProvider`, `RecoveryAction` and `WorkEffort` under the limit |
| 2 | `PRODUCT_VARIANT_IDENTITY` key over the 3,072 byte key limit (`featureKey`, `text-long`) | The key is `featureHash`, the SHA-256 hex of the sorted feature ids (`ec.resource.digestText`); `featureKey` is a plain field |
| 3 | 3-byte `utf8` tables: no emoji (chat demo skipped) | `mysql8` is `utf8mb4` / `utf8mb4_0900_ai_ci`, `mysql` is `utf8mb4` / `utf8mb4_unicode_ci`. At 4 bytes a character `LocalizedEntityField`'s key went over 3,072 bytes, so its `fieldName` is `text-short`. An existing utf8 database needs a reload or a manual `ALTER TABLE ... CONVERT TO CHARACTER SET utf8mb4` |
| 4 | Values over 255 characters in `text-medium` fields | The `ArtifactGroupMember.artifactName` patterns over 255 (BOSS setup and accounting data, XMPP security data) are split over several rows. `DataDocumentField.fieldPath` is `text-intermediate` |
| 4a | `BossZAccountingDemoData.xml` failed on `CAST(... AS INTEGER)` in `SettlementInvoiceItems`: MySQL has no INTEGER cast | A view expression's cast to an integer type is `SIGNED` on MySQL (`crates/moqui-db/src/sql/view.rs`) |
| 5 | Git updates lost after 50 s lock waits | A ref write in a transaction that held the repository row recorded its `GitUpdate` in a new transaction, whose foreign key check waited on that lock on the same thread. The update is now written in the caller's transaction, and its events are scheduled once that commits |
| 6 | The log line left out a field named `error` | The console and file lines end with `error=...` |
| 7 | The data feed's commit hook panicked ("RefCell already mutably borrowed") on a document whose entity is not in the model | The feed looks its names up in the file model only, and leaves out a name it does not have |
| 8 | `MonitoringSalesSecurityData.xml` was `seed` over `seed-initial` documents | It is `seed-initial` |
| 9 | The network bounds of HL-019 had no test against a server | Acceptance tests behind a relay that stops forwarding, on MySQL 8.4 and MariaDB 11.4. They found that a read past the read timeout held the worker twice as long: the pool sent a ROLLBACK on the dead connection. A connection whose socket failed or timed out now reports itself closed and is dropped |
| — | A `mysql`-only build warned about unused Postgres items | `#[cfg(feature = "postgres")]` |

## Not checked

- TCP keepalive ending a wait on a host that stopped answering without a reset. The relay is a live TCP peer that
  answers keepalive probes, so this needs packets dropped below TCP (a firewall rule) and at least 90 s of quiet.
- MySQL 5.7 with the `mysql` conf; only MySQL 8.4 and MariaDB 11.4 were run.
