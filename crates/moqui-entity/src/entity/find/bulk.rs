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

//! Bulk writes of a find: `update_all`, `delete_all` and `purge`. They page through the rows by primary key in bounded
//! chunks, reading only the key (and the fields an update sets) unless an ECA rule, the data feed, a transaction cache
//! or a sim needs whole rows (plans/High-Level-Review.md HL-033).

use std::collections::HashMap;
use std::time::Instant;

use moqui_db::sql::{Condition, OrderBy};
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::EntityFind;
use crate::entity::load_feed;
use crate::entity::value::{EntityList, EntityValue};
use crate::transaction::TransactionAttribute;

/// Rows a bulk write reads in one query, unless [`EntityFind::bulk_chunk`] sets another.
pub const BULK_CHUNK: i64 = 1000;

/// What [`EntityFind::purge`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Purged {
    /// Rows deleted in the chunks that committed.
    pub deleted: i64,
    /// Chunks that failed, were rolled back and passed over.
    pub failed_chunks: usize,
    /// The deadline came before the last chunk: rows may be left for the next run.
    pub stopped_early: bool,
}

/// The rows of a find, a chunk at a time in primary-key order. A find with a limit, an offset or its own order, of a
/// view, of an entity with no primary key, or a delete of an entity that refers to itself is read in one chunk.
struct KeyPager {
    base: EntityFind,
    pk: Vec<String>,
    /// For a delete: each relationship of the entity to itself, as (field, related primary key field) pairs. The rows
    /// are then deleted referring rows first ([`children_first`]).
    self_refs: Vec<Vec<(String, String)>>,
    /// Rows per chunk; `None` reads all in one.
    chunk: Option<i64>,
    /// The primary key of the last row read.
    after: Option<Vec<Value>>,
    done: bool,
    /// Chunks read so far.
    reads: usize,
}

impl KeyPager {
    /// The next chunk, or `None` when every row was read.
    fn next(&mut self) -> Result<Option<EntityList>, Error> {
        if self.done {
            return Ok(None);
        }
        let mut find = self.base.clone();
        if let Some(after) = &self.after {
            find.conditions.push(after_key(&self.pk, after));
        }
        if let Some(chunk) = self.chunk {
            find.limit = Some(chunk);
        }
        let list = find.list_internal("list")?;
        self.reads += 1;
        let full = self
            .chunk
            .is_some_and(|chunk| i64::try_from(list.len()).is_ok_and(|len| len >= chunk));
        if let Some(last) = list.values().last() {
            let key = self
                .pk
                .iter()
                .map(|name| last.get(name).cloned().unwrap_or(Value::Null));
            self.after = Some(key.collect());
        }
        self.done = !full;
        if list.is_empty() {
            return Ok(None);
        }
        if self.self_refs.is_empty() {
            return Ok(Some(list));
        }
        Ok(Some(EntityList::new(children_first(
            list.into_values(),
            &self.pk,
            &self.self_refs,
        ))))
    }

    /// The key of the last row read, for a log line.
    fn last_key(&self) -> String {
        match &self.after {
            Some(values) => values
                .iter()
                .map(moqui_lib::object::to_plain_string)
                .collect::<Vec<_>>()
                .join(","),
            None => "the end".to_string(),
        }
    }
}

/// `rows` reordered so that a row another of them refers to through `self_refs` comes after it: a delete in this order
/// does not remove a row a later one still refers to (a self-referencing foreign key, as `DbViewEntityMember`'s
/// `joinFromAlias`). Rows in a cycle keep their order, at the end.
fn children_first(rows: Vec<EntityValue>, pk: &[String], self_refs: &[Vec<(String, String)>]) -> Vec<EntityValue> {
    let text = |values: Vec<Option<&Value>>| -> Option<String> {
        let mut key = String::new();
        for value in values {
            let value = value.filter(|value| !value.is_null())?;
            key.push_str(&moqui_lib::object::to_plain_string(value));
            key.push('\u{1f}');
        }
        Some(key)
    };
    let keys: Vec<Option<String>> = rows
        .iter()
        .map(|row| text(pk.iter().map(|name| row.get(name)).collect()))
        .collect();
    let at: HashMap<&str, usize> = keys
        .iter()
        .enumerate()
        .filter_map(|(index, key)| key.as_deref().map(|key| (key, index)))
        .collect();
    // Each row's targets: the rows of the set it refers to.
    let mut referrers = vec![0_usize; rows.len()];
    let mut targets: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
    for (index, row) in rows.iter().enumerate() {
        for pairs in self_refs {
            let target = pk.iter().map(|name| {
                pairs
                    .iter()
                    .find(|(_, related)| related == name)
                    .and_then(|(field, _)| row.get(field))
            });
            let Some(target) = text(target.collect()).and_then(|key| at.get(key.as_str()).copied()) else {
                continue;
            };
            if target != index {
                targets[index].push(target);
                referrers[target] += 1;
            }
        }
    }
    let mut order = Vec::with_capacity(rows.len());
    let mut ready: Vec<usize> = (0..rows.len()).rev().filter(|&index| referrers[index] == 0).collect();
    let mut placed = vec![false; rows.len()];
    while let Some(index) = ready.pop() {
        placed[index] = true;
        order.push(index);
        for &target in &targets[index] {
            referrers[target] -= 1;
            if referrers[target] == 0 {
                ready.push(target);
            }
        }
    }
    order.extend((0..rows.len()).filter(|&index| !placed[index]));
    let mut slots: Vec<Option<EntityValue>> = rows.into_iter().map(Some).collect();
    order.into_iter().filter_map(|index| slots[index].take()).collect()
}

/// Rows after `last` in primary-key order: `k1 > v1 OR (k1 = v1 AND k2 > v2) OR ...`.
fn after_key(pk: &[String], last: &[Value]) -> Condition {
    let mut any = Vec::with_capacity(pk.len());
    for (index, (name, value)) in pk.iter().zip(last).enumerate() {
        let mut all: Vec<Condition> = pk
            .iter()
            .zip(last)
            .take(index)
            .map(|(name, value)| Condition::equals(name.as_str(), value.clone()))
            .collect();
        all.push(Condition::greater(name.as_str(), value.clone()));
        any.push(if all.len() == 1 {
            all.remove(0)
        } else {
            Condition::and(all)
        });
    }
    if any.len() == 1 {
        any.remove(0)
    } else {
        Condition::or(any)
    }
}

impl EntityFind {
    /// Rows `update_all`, `delete_all` and `purge` read in one query; at least 1. [`BULK_CHUNK`] when not set.
    #[must_use]
    pub fn bulk_chunk(mut self, rows: i64) -> Self {
        self.bulk_chunk = Some(rows.max(1));
        self
    }

    /// Whether a bulk `operation` must read whole rows: an ECA rule of the operation runs, the data feed keeps the
    /// write's values, or a transaction cache or sim answers the find (they do not answer a find of selected fields).
    fn bulk_reads_whole_rows(&self, operation: &str, holds: bool) -> Result<bool, Error> {
        let facade = &self.facade;
        let eca =
            crate::hooks::entered() && !load_feed::match_rules_with(facade, &self.entity_name, operation).is_empty();
        let held = holds && facade.transactions_for(&self.entity_name)?.transaction_cache_active();
        Ok(eca || held || facade.sim_active() || load_feed::feed_keeps_values(facade, &self.entity_name))
    }

    /// The pager of this find's rows. With `select`, the rows read only the primary key and these fields. A `delete`
    /// orders the rows of an entity that refers to itself so that none is deleted before a row that refers to it.
    fn key_pager(mut self, select: Option<Vec<String>>, delete: bool) -> Result<KeyPager, Error> {
        let (pk, self_refs) = self.facade.with_entity(&self.entity_name, |entity, _, _| {
            if entity.is_view {
                return Ok((Vec::new(), Vec::new()));
            }
            let pk: Vec<String> = entity.pk_field_names().into_iter().map(str::to_string).collect();
            let self_refs: Vec<Vec<(String, String)>> = entity
                .relationships
                .iter()
                .filter(|relationship| {
                    delete
                        && relationship.relationship_type.starts_with("one")
                        && relationship.related == entity.full_name
                        && !relationship.key_pairs.is_empty()
                })
                .map(|relationship| relationship.key_pairs.clone())
                .collect();
            Ok((pk, self_refs))
        })?;
        // The rows of a self-referencing entity are ordered as a whole, and a find's own order is kept, as before: such a
        // delete or update is read in one query.
        let paged = !pk.is_empty()
            && self.limit.is_none()
            && self.offset.is_none()
            && self.order.is_empty()
            && self_refs.is_empty();
        let chunk = self.bulk_chunk.unwrap_or(BULK_CHUNK);
        self = self.use_cache(false);
        // A find that already selects fields needs the key too, to page by it and to order the deletes.
        if (paged || !self_refs.is_empty()) && (select.is_some() || !self.select_fields.is_empty()) {
            let refs = self_refs.iter().flatten().map(|(field, _)| field);
            for name in pk.iter().chain(select.iter().flatten()).chain(refs) {
                self = self.select_field(name);
            }
        }
        if paged {
            self.order = pk.iter().map(|name| OrderBy::asc(name.as_str())).collect();
        }
        Ok(KeyPager {
            base: self,
            pk,
            self_refs,
            chunk: paged.then_some(chunk),
            after: None,
            done: false,
            reads: 0,
        })
    }

    /// Java `updateAll`: set `fields` on each row found and update the rows that change, so each update runs its
    /// entity ECA rules. Returns how many rows were updated. Not for a `create-only` entity. The find skips the cache.
    /// The rows are read [`BULK_CHUNK`] at a time by primary key, in the caller's transaction; a find with its own order
    /// is read in one query and updated in that order.
    pub fn update_all(self, fields: impl IntoIterator<Item = (String, Value)>) -> Result<i64, Error> {
        let fields: Vec<(String, Value)> = fields.into_iter().collect();
        let _authz = self.authz_guard();
        let facade = self.facade.clone();
        self.refuse_create_only("updated")?;
        let select = if self.bulk_reads_whole_rows("update", true)? {
            None
        } else {
            Some(fields.iter().map(|(name, _)| name.clone()).collect())
        };
        let mut pager = self.key_pager(select, false)?;
        let mut updated = 0_i64;
        while let Some(list) = pager.next()? {
            for value in list.values() {
                let mut changed = value.clone();
                for (name, new_value) in &fields {
                    changed.set(name.clone(), new_value.clone());
                }
                // As Java's `updateAll`, only a row the fields change is updated, and only those fields are written.
                if changed.is_modified() {
                    changed.update(&facade)?;
                    updated += 1;
                }
            }
        }
        Ok(updated)
    }

    /// Java `deleteAll`: delete each row found through the value path, so each delete runs its entity ECA rules.
    /// Returns how many rows were deleted. Not for a `create-only` entity. The find skips the cache. The rows are read
    /// [`BULK_CHUNK`] at a time by primary key, in the caller's transaction. A find with its own order is read in one
    /// query and deleted in that order; the rows of an entity with a relationship to itself are read in one query and
    /// deleted referring rows first, so a self-referencing foreign key does not refuse the delete (Java deletes in the
    /// order the database returns, which only happens to work).
    pub fn delete_all(self) -> Result<i64, Error> {
        let _authz = self.authz_guard();
        let facade = self.facade.clone();
        self.refuse_create_only("deleted")?;
        let select = (!self.bulk_reads_whole_rows("delete", true)?).then(Vec::new);
        let mut pager = self.key_pager(select, true)?;
        let mut deleted = 0_i64;
        while let Some(list) = pager.next()? {
            for value in list.values() {
                value.delete(&facade)?;
            }
            deleted = deleted.saturating_add(i64::try_from(list.len()).unwrap_or(i64::MAX));
        }
        Ok(deleted)
    }

    /// A purge: delete the rows the find matches as [`Self::delete_all`] does, with their ECA rules, but each chunk in a
    /// transaction of its own that commits before the next chunk is read, so a backlog larger than one transaction
    /// can hold still goes. A chunk that fails is rolled back, logged and passed over. No chunk starts after
    /// `deadline`: what was deleted stays, and the next run goes on from there.
    pub fn purge(self, deadline: Option<Instant>) -> Result<Purged, Error> {
        self.purge_while(|| deadline.is_none_or(|deadline| Instant::now() < deadline))
    }

    /// [`Self::purge`], asking `more` before each chunk whether to begin it.
    pub fn purge_while(self, mut more: impl FnMut() -> bool) -> Result<Purged, Error> {
        let _authz = self.authz_guard();
        let facade = self.facade.clone();
        let entity_name = self.entity_name.clone();
        self.refuse_create_only("deleted")?;
        let tx = facade.transactions_for(&entity_name)?;
        // Each chunk begins a transaction of its own, which holds nothing in a transaction cache.
        let select = (!self.bulk_reads_whole_rows("delete", false)?).then(Vec::new);
        let mut pager = self.key_pager(select, true)?;
        let mut purged = Purged::default();
        while !pager.done {
            if !more() {
                purged.stopped_early = true;
                tracing::info!(
                    entity = %entity_name,
                    deleted = purged.deleted,
                    "purge reached its deadline after the rows up to key {}; the rest is left for the next run",
                    pager.last_key()
                );
                break;
            }
            let reads = pager.reads;
            let chunk = tx.run(TransactionAttribute::ForceNew, None, || {
                let Some(list) = pager.next()? else {
                    return Ok(None);
                };
                for value in list.values() {
                    value.delete(&facade)?;
                }
                Ok(Some(i64::try_from(list.len()).unwrap_or(i64::MAX)))
            });
            match chunk {
                Ok(Some(deleted)) => purged.deleted = purged.deleted.saturating_add(deleted),
                Ok(None) => break,
                // A chunk that was read and failed is passed over; one that could not be read gives no key to go on
                // from, so the purge stops.
                Err(err) if pager.reads > reads => {
                    purged.failed_chunks += 1;
                    tracing::error!(
                        error = %err,
                        entity = %entity_name,
                        "purge chunk up to key {} failed and was rolled back; going on with the next",
                        pager.last_key()
                    );
                }
                Err(err) => {
                    tracing::error!(error = %err, entity = %entity_name, "purge could not read its next chunk; stopping");
                    purged.failed_chunks += 1;
                    break;
                }
            }
        }
        Ok(purged)
    }
}

#[cfg(test)]
#[path = "bulk_tests.rs"]
mod tests;
