/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! Day partitions of the telemetry tables (plans/Monitoring.md, Indexes: retention), with
//! `<search-facade><postgres partition-telemetry="true">`. A new `otel_spans`, `otel_logs` or
//! `monitor_annotations` table is `PARTITION BY RANGE` on its time column, one partition per UTC day
//! (`<table>_p<yyyymmdd>`) and a `<table>_default` for rows outside them; `otel_metrics` is first
//! `PARTITION BY LIST` on its granularity (`<table>_<granularity>`), since minute, hour and day rows keep
//! for different times, each of those by day. The key becomes `(doc_id, <list>, <day>)`. Partitions are
//! made for yesterday through `partition-days-ahead` days on, when the table is first used in a process
//! and by the daily retention job, which drops the days past retention before it deletes rows. A table
//! made before the option was on stays as it is, with a warning, until `partition_table` moves its rows.

use std::collections::HashSet;

use jiff::civil::Date;
use jiff::Timestamp;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use super::compile::{column, ident, literal};
use super::{ddl_transaction, insert_columns, lock, PostgresBackend};
use crate::search::mapping::DocumentMapping;
use crate::search::telemetry;

/// The partitioning option.
#[derive(Clone, Copy, Debug)]
pub(super) struct Partitioning {
    /// Day partitions made ahead of today.
    pub(super) days_ahead: i64,
}

/// How one table is split: the list column, when there is one, and the day column.
#[derive(Clone, Copy, Debug)]
pub(super) struct Split {
    pub(super) list: Option<&'static str>,
    pub(super) day: &'static str,
}

impl Split {
    /// The split of a telemetry index, by its document id, which is its index and table name.
    pub(super) fn of(mapping: &DocumentMapping) -> Option<Self> {
        if mapping.document_id != mapping.physical_name {
            return None;
        }
        let (list, day) = telemetry::partition_by(&mapping.document_id)?;
        Some(Self { list, day })
    }

    /// The key columns after `doc_id`.
    pub(super) fn key_columns(&self) -> Vec<&'static str> {
        self.list.into_iter().chain([self.day]).collect()
    }

    pub(super) fn conflict_target(&self) -> String {
        let mut target = "doc_id".to_string();
        for name in self.key_columns() {
            target.push_str(", ");
            target.push_str(&column(name));
        }
        target
    }
}

/// What `partition_table` did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartitionReport {
    /// Rows in the partitioned table after the swap.
    pub rows: u64,
    /// The old table, renamed, for the operator to drop.
    pub kept: String,
}

fn day_name(day: Date) -> String {
    day.strftime("%Y%m%d").to_string()
}

fn day_start(day: Date) -> String {
    format!("{} 00:00:00+00", day.strftime("%Y-%m-%d"))
}

fn today() -> Date {
    Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date()
}

impl PostgresBackend {
    /// Partition the telemetry tables made from now on, with `days_ahead` day partitions ahead of today.
    #[must_use]
    pub fn with_partitions(mut self, days_ahead: u32) -> Self {
        self.partitioning = Some(Partitioning {
            days_ahead: i64::from(days_ahead),
        });
        self
    }

    /// The split a new table of `mapping` is made with: only with the option on.
    pub(super) fn new_split(&self, mapping: &DocumentMapping) -> Option<Split> {
        self.partitioning.and(Split::of(mapping))
    }

    /// The split of the table that exists, read once from `pg_partitioned_table`, and the conflict target an insert
    /// into it takes.
    pub(super) fn conflict_target(&self, mapping: &DocumentMapping) -> String {
        match Split::of(mapping) {
            Some(split) if self.is_partitioned(mapping) => split.conflict_target(),
            _ => "doc_id".to_string(),
        }
    }

    pub(super) fn is_partitioned(&self, mapping: &DocumentMapping) -> bool {
        if Split::of(mapping).is_none() {
            return false;
        }
        if let Some(known) = lock(&self.partitioned).get(&mapping.document_id) {
            return *known;
        }
        let table = self.table(mapping);
        let found = self.tx.with_reserve(|conn| {
            conn.query(
                "SELECT EXISTS (SELECT 1 FROM pg_partitioned_table WHERE partrelid = to_regclass(?)) AS partitioned",
                &[Value::string(&table)],
            )
        });
        match found {
            Ok(rows) => {
                let partitioned = rows.first().and_then(|row| row.get("partitioned")) == Some(&Value::Bool(true));
                lock(&self.partitioned).insert(mapping.document_id.clone(), partitioned);
                partitioned
            }
            Err(error) => {
                tracing::warn!(%error, table, "could not read whether the search table is partitioned");
                false
            }
        }
    }

    /// Forget what is known of the table's partitioning, as after it is made or swapped.
    pub(super) fn forget_partitioning(&self, mapping: &DocumentMapping) {
        lock(&self.partitioned).remove(&mapping.document_id);
    }

    /// The default partitions a new partitioned `table` of `mapping` is made with, and for a list split each list
    /// partition, itself split by day.
    pub(super) fn default_partitions(&self, mapping: &DocumentMapping, table: &str, split: Split) -> Vec<String> {
        let schema = ident(&self.schema);
        let base = &mapping.physical_name;
        let mut out = Vec::new();
        if split.list.is_some() {
            for value in telemetry::GRANULARITIES {
                let child = format!("{schema}.{}", ident(&format!("{base}_{value}")));
                out.push(format!(
                    "CREATE TABLE {child} PARTITION OF {table} FOR VALUES IN ({}) PARTITION BY RANGE ({})",
                    literal(value),
                    column(split.day)
                ));
                out.push(format!(
                    "CREATE TABLE {schema}.{} PARTITION OF {child} DEFAULT",
                    ident(&format!("{base}_{value}_default"))
                ));
            }
        }
        out.push(format!(
            "CREATE TABLE {schema}.{} PARTITION OF {table} DEFAULT",
            ident(&format!("{base}_default"))
        ));
        out
    }

    /// The tables split by day, each with the prefix of its day partitions: the table itself, or each granularity's
    /// list partition (only `granularity`'s when it is given).
    fn day_parents(
        &self,
        mapping: &DocumentMapping,
        table: &str,
        split: Split,
        granularity: Option<&str>,
    ) -> Vec<(String, String)> {
        let base = &mapping.physical_name;
        match split.list {
            None => vec![(table.to_string(), base.clone())],
            Some(_) => telemetry::GRANULARITIES
                .iter()
                .filter(|value| granularity.is_none_or(|wanted| wanted == **value))
                .map(|value| {
                    let prefix = format!("{base}_{value}");
                    (format!("{}.{}", ident(&self.schema), ident(&prefix)), prefix)
                })
                .collect(),
        }
    }

    /// Make the day partitions of `days` under `table`. One that cannot be made, such as a day the default partition
    /// already holds rows of, is logged and skipped: its rows stay in the default partition, which retention deletes
    /// row by row.
    fn add_days(&self, mapping: &DocumentMapping, table: &str, split: Split, days: &[Date]) -> usize {
        let mut made = 0;
        for (parent, prefix) in self.day_parents(mapping, table, split, None) {
            for day in days {
                let Ok(next) = day.tomorrow() else {
                    continue;
                };
                let name = format!(
                    "{}.{}",
                    ident(&self.schema),
                    ident(&format!("{prefix}_p{}", day_name(*day)))
                );
                let statement = format!(
                    "CREATE TABLE IF NOT EXISTS {name} PARTITION OF {parent} FOR VALUES FROM ({}) TO ({})",
                    literal(&day_start(*day)),
                    literal(&day_start(next))
                );
                match self
                    .tx
                    .with_reserve(|conn| ddl_transaction(conn, &parent, |conn| conn.execute(&statement, &[])))
                {
                    Ok(_) => made += 1,
                    Err(error) => tracing::error!(%error, partition = name, "could not make the search day partition"),
                }
            }
        }
        made
    }

    /// Yesterday (a sender's clock may be behind) through `days_ahead` days on.
    fn upcoming_days(&self) -> Vec<Date> {
        let ahead = self.partitioning.map_or(0, |partitioning| partitioning.days_ahead);
        let today = today();
        (-1..=ahead)
            .filter_map(|offset| today.checked_add(jiff::Span::new().days(offset)).ok())
            .collect()
    }

    /// Keep a partitioned table's day partitions made ahead, or warn, once a process, that a table made before the
    /// option was on is not partitioned. Nothing without the option.
    pub(super) fn maintain(&self, mapping: &DocumentMapping, once: bool) {
        let Some(split) = self.new_split(mapping) else {
            return;
        };
        if once && !lock(&self.maintained).insert(mapping.document_id.clone()) {
            return;
        }
        if self.is_partitioned(mapping) {
            let table = self.table(mapping);
            self.add_days(mapping, &table, split, &self.upcoming_days());
        } else if lock(&self.warned).insert(mapping.document_id.clone()) {
            tracing::warn!(
                table = %self.table(mapping),
                "partition-telemetry is on but this telemetry table was made unpartitioned; run \
                 org.moqui.impl.TelemetryServices.partition#TelemetryTables (Monitor › Sources) to move its rows \
                 into a day-partitioned table. Until then retention deletes its rows one by one"
            );
        }
    }

    /// Drop the day partitions wholly before `before`, of `granularity`'s list partition when the table has one.
    /// Answers the partitions dropped; a table that is not partitioned has none. One that cannot be dropped is logged
    /// and the rest go on; retention then deletes its rows.
    pub(super) fn drop_days_before(
        &self,
        mapping: &DocumentMapping,
        granularity: Option<&str>,
        before: Timestamp,
    ) -> Result<u64, Error> {
        let Some(split) = Split::of(mapping) else {
            return Ok(0);
        };
        if !self.is_partitioned(mapping) {
            return Ok(0);
        }
        let table = self.table(mapping);
        let cutoff = before.to_zoned(jiff::tz::TimeZone::UTC).date();
        let mut dropped = 0;
        for (parent, prefix) in self.day_parents(mapping, &table, split, granularity) {
            let rows = self.tx.with_reserve(|conn| {
                conn.query(
                    "SELECT c.relname AS name FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid \
                     WHERE i.inhparent = to_regclass(?)",
                    &[Value::string(&parent)],
                )
            })?;
            let marker = format!("{prefix}_p");
            for name in rows.iter().filter_map(|row| row.get("name")).map(to_plain_string) {
                let Some(day) = name
                    .strip_prefix(&marker)
                    .filter(|digits| digits.len() == 8)
                    .and_then(|digits| Date::strptime("%Y%m%d", digits).ok())
                else {
                    continue;
                };
                // Wholly before the cutoff: the partition's last instant is before the cutoff's day.
                if day.tomorrow().is_ok_and(|next| next <= cutoff) {
                    let statement = format!("DROP TABLE {}.{}", ident(&self.schema), ident(&name));
                    match self
                        .tx
                        .with_reserve(|conn| ddl_transaction(conn, &parent, |conn| conn.execute(&statement, &[])))
                    {
                        Ok(_) => dropped += 1,
                        Err(error) => {
                            tracing::error!(%error, partition = name, "could not drop the expired search partition")
                        }
                    }
                }
            }
        }
        Ok(dropped)
    }

    /// Move an unpartitioned telemetry table into a day-partitioned one: make `<table>_partitioned` with a partition
    /// for each day the rows have, copy the rows a day at a time, then with the old table locked copy what was
    /// written meanwhile and swap the names. The old table is kept as `<table>_unpartitioned` for the operator to
    /// drop. `None` when the table does not exist (it will be made partitioned) or is already partitioned.
    pub(super) fn move_into_partitions(&self, mapping: &DocumentMapping) -> Result<Option<PartitionReport>, Error> {
        let Some(split) = self.new_split(mapping) else {
            return Err(Error::new(format!(
                "{} is not a telemetry table that can be partitioned, or partition-telemetry is off",
                mapping.document_id
            )));
        };
        self.forget_partitioning(mapping);
        let old = self.table(mapping);
        let exists = self
            .tx
            .with_reserve(|conn| conn.query("SELECT to_regclass(?)::text AS existing", &[Value::string(&old)]))?
            .first()
            .and_then(|row| row.get("existing"))
            .is_some_and(|value| !value.is_null());
        if !exists || self.is_partitioned(mapping) {
            return Ok(None);
        }
        let schema = ident(&self.schema);
        let base = &mapping.physical_name;
        let kept_name = format!("{base}_unpartitioned");
        let kept = format!("{schema}.{}", ident(&kept_name));
        let new = format!("{schema}.{}", ident(&format!("{base}_partitioned")));
        let kept_exists = self
            .tx
            .with_reserve(|conn| conn.query("SELECT to_regclass(?)::text AS existing", &[Value::string(&kept)]))?
            .first()
            .and_then(|row| row.get("existing"))
            .is_some_and(|value| !value.is_null());
        if kept_exists {
            return Err(Error::new(format!(
                "{kept} is left from an earlier move; drop it (DROP TABLE {kept}) before partitioning {old} again"
            )));
        }
        // A copy left by a move that stopped part way is made again.
        let mut statements = vec![format!("DROP TABLE IF EXISTS {new}")];
        statements.extend(self.create_statements_for(mapping, &new, Some(split)));
        statements.extend(self.default_partitions(mapping, &new, split));
        self.tx.with_reserve(|conn| {
            ddl_transaction(conn, &new, |conn| {
                for statement in &statements {
                    conn.execute(statement, &[])?;
                }
                Ok(())
            })
        })?;
        let day = column(split.day);
        let day_value = format!("COALESCE({day}, indexed_at)");
        let rows = self.tx.with_reserve(|conn| {
            conn.query(
                &format!(
                    "SELECT DISTINCT to_char(({day_value}) AT TIME ZONE 'UTC', 'YYYY-MM-DD') AS day FROM {old} \
                     UNION SELECT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD')"
                ),
                &[],
            )
        })?;
        let mut days: Vec<Date> = rows
            .iter()
            .filter_map(|row| row.get("day"))
            .filter_map(|value| to_plain_string(value).parse().ok())
            .collect();
        days.extend(self.upcoming_days());
        let days: Vec<Date> = days.into_iter().collect::<HashSet<_>>().into_iter().collect();
        self.add_days(mapping, &new, split, &days);
        let started = self
            .tx
            .with_reserve(|conn| conn.query("SELECT now()::text AS started", &[]))?
            .first()
            .and_then(|row| row.get("started"))
            .map(to_plain_string)
            .ok_or_else(|| Error::new("the database did not give its time"))?;
        let copy = self.copy_sql(mapping, &old, &new, split);
        // Every row's day is among `days`; a day whose partition could not be made goes to the default partition.
        for day in &days {
            let Ok(next) = day.tomorrow() else {
                continue;
            };
            let statement = format!(
                "{} WHERE {day_value} >= ?::timestamptz AND {day_value} < ?::timestamptz ON CONFLICT DO NOTHING",
                copy.0
            );
            self.tx.with_reserve(|conn| {
                conn.execute(
                    &statement,
                    &[Value::String(day_start(*day)), Value::String(day_start(next))],
                )
            })?;
        }
        let unqualified = ident(base);
        self.tx.with_reserve(|conn| {
            ddl_transaction(conn, &old, |conn| {
                conn.execute(&format!("LOCK TABLE {old} IN ACCESS EXCLUSIVE MODE"), &[])?;
                conn.execute(
                    &format!(
                        "{} WHERE indexed_at >= ?::timestamptz ON CONFLICT ({}) DO UPDATE SET {}",
                        copy.0,
                        split.conflict_target(),
                        copy.1
                    ),
                    &[Value::string(&started)],
                )?;
                conn.execute(&format!("ALTER TABLE {old} RENAME TO {}", ident(&kept_name)), &[])?;
                conn.execute(&format!("ALTER TABLE {new} RENAME TO {unqualified}"), &[])?;
                Ok(())
            })
        })?;
        self.forget_partitioning(mapping);
        let count = self
            .tx
            .with_reserve(|conn| conn.query(&format!("SELECT count(*) AS n FROM {old}"), &[]))?
            .first()
            .and_then(|row| row.get("n"))
            .map(|value| to_plain_string(value).parse().unwrap_or(0))
            .unwrap_or(0);
        tracing::info!(
            table = old,
            rows = count,
            "the telemetry table is day-partitioned; the old rows are kept in {kept}: DROP TABLE {kept} once it is \
             not needed"
        );
        Ok(Some(PartitionReport {
            rows: count,
            kept: format!("{}.{kept_name}", self.schema),
        }))
    }

    /// `INSERT INTO new (...) SELECT ... FROM old`, and the `SET` list of an upsert of the same columns. The
    /// generated columns are left to the new table.
    fn copy_sql(&self, mapping: &DocumentMapping, old: &str, new: &str, split: Split) -> (String, String) {
        let mut names = vec!["doc_id".to_string(), "indexed_at".to_string(), "body".to_string()];
        names.extend(insert_columns(mapping).iter().map(|item| column(item.name)));
        if self.vectors.is_some() {
            names.extend(["embedding", "embedding_model", "embedding_indexed_at"].map(str::to_string));
        }
        let selected: Vec<String> = names
            .iter()
            .map(|name| {
                if split.list.is_some_and(|list| column(list) == *name) {
                    format!("COALESCE({name}, '')")
                } else if column(split.day) == *name {
                    format!("COALESCE({name}, indexed_at)")
                } else {
                    name.clone()
                }
            })
            .collect();
        let set: Vec<String> = names
            .iter()
            .skip(1)
            .filter(|name| !split.key_columns().iter().any(|key| column(key) == **name))
            .map(|name| format!("{name} = EXCLUDED.{name}"))
            .collect();
        (
            format!(
                "INSERT INTO {new} ({}) SELECT {} FROM {old}",
                names.join(", "),
                selected.join(", ")
            ),
            set.join(", "),
        )
    }
}
