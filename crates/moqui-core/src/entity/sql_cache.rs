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

//! Rendered SQL for a stable statement shape. The driver still prepares by the text.
//!
//! A hit returns the text from the first render. Parameters are bound by the caller, so a different
//! primary key does not need a new string. The cap drops the oldest shapes when a varying column list
//! would otherwise grow the map without limit.
//!
//! [`read_shape`] keeps what every find of all of a plain entity's fields shares. Only file entities are plain (the
//! definitions built at runtime are views), so a shape does not go stale.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};

use moqui_db::dialect::Dialect;
use moqui_db::model::EntityDefinition;
use moqui_lib::error::Error;

use super::facade::EntityFacade;

/// A find of every field of a plain entity, in one dialect.
pub(crate) struct ReadShape {
    dialect: String,
    pub(crate) full_name: Arc<str>,
    /// Field name and Java type, the columns a row is read with.
    pub(crate) columns: Arc<[(Arc<str>, String)]>,
    /// The same names, sorted, for a value made of the entity to share ([`super::EntityValue::set`]).
    pub(crate) names: Arc<[Arc<str>]>,
    /// `SELECT <every column> FROM <table>`, for [`moqui_db::sql::select_with_head`].
    pub(crate) head: String,
    /// The select by the single primary key, rendered by the first such find.
    select_pk: OnceLock<Arc<str>>,
}

impl ReadShape {
    pub(crate) fn select_pk(&self, render: impl FnOnce() -> Result<String, Error>) -> Result<&str, Error> {
        if let Some(sql) = self.select_pk.get() {
            return Ok(sql);
        }
        let sql: Arc<str> = Arc::from(render()?);
        Ok(self.select_pk.get_or_init(|| sql))
    }
}

#[derive(Default)]
struct ReadShapes {
    by_entity: HashMap<String, Arc<ReadShape>>,
}

/// The shape of `entity`, a plain entity, built on its first find.
pub(crate) fn read_shape(facade: &EntityFacade, entity: &EntityDefinition, dialect: &Dialect) -> Arc<ReadShape> {
    let known = facade.with_slot_read(ReadShapes::default, |shapes| {
        shapes
            .by_entity
            .get(&entity.full_name)
            .filter(|shape| shape.dialect == dialect.name)
            .cloned()
    });
    if let Some(shape) = known {
        return shape;
    }
    let columns: Arc<[(Arc<str>, String)]> = entity
        .fields
        .iter()
        .map(|field| (Arc::from(field.name.as_str()), field.java_type.clone()))
        .collect();
    let mut names: Vec<Arc<str>> = columns.iter().map(|(name, _)| Arc::clone(name)).collect();
    names.sort();
    let shape = Arc::new(ReadShape {
        dialect: dialect.name.clone(),
        full_name: Arc::from(entity.full_name.as_str()),
        columns,
        names: names.into(),
        head: moqui_db::sql::select_head(entity, dialect, &[]),
        select_pk: OnceLock::new(),
    });
    facade.with_slot(ReadShapes::default, |shapes| {
        shapes.by_entity.insert(entity.full_name.clone(), Arc::clone(&shape));
    });
    shape
}

const CAP: usize = 256;

#[derive(Default)]
struct SqlText {
    entries: HashMap<Arc<str>, Arc<str>>,
    order: VecDeque<Arc<str>>,
}

impl SqlText {
    fn get(&self, key: &str) -> Option<Arc<str>> {
        self.entries.get(key).cloned()
    }

    fn insert(&mut self, key: &str, sql: Arc<str>) {
        if self.entries.contains_key(key) {
            return;
        }
        let key: Arc<str> = Arc::from(key);
        self.order.push_back(Arc::clone(&key));
        self.entries.insert(Arc::clone(&key), sql);
        while self.entries.len() > CAP {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            // An older queue item for a key that was inserted again is skipped: insert refuses a second copy.
            self.entries.remove(old.as_ref());
        }
    }
}

/// The SQL for `key`, rendering it once.
pub(crate) fn cached(
    facade: &EntityFacade,
    key: &str,
    render: impl FnOnce() -> Result<String, Error>,
) -> Result<Arc<str>, Error> {
    if let Some(hit) = facade.with_slot_read(SqlText::default, |cache| cache.get(key)) {
        return Ok(hit);
    }
    let sql: Arc<str> = Arc::from(render()?);
    let stored = Arc::clone(&sql);
    facade.with_slot(SqlText::default, |cache| cache.insert(key, stored));
    Ok(sql)
}

#[cfg(test)]
mod tests {
    use super::cached;
    use crate::testing;

    #[test]
    fn the_second_call_reuses_the_rendered_text_and_a_different_key_does_not() {
        let facade = testing::open();
        let mut renders = 0;
        let first = cached(&facade, "entity|insert|testId,", || {
            renders += 1;
            Ok("INSERT INTO TEST".to_string())
        })
        .unwrap();
        let second = cached(&facade, "entity|insert|testId,", || {
            renders += 1;
            Ok("SHOULD NOT RENDER".to_string())
        })
        .unwrap();
        assert_eq!(renders, 1);
        assert_eq!(&*first, "INSERT INTO TEST");
        assert_eq!(first, second);
        let other = cached(&facade, "entity|insert|testId,testMedium,", || {
            renders += 1;
            Ok("INSERT INTO TEST (two)".to_string())
        })
        .unwrap();
        assert_eq!(renders, 2);
        assert_eq!(&*other, "INSERT INTO TEST (two)");
    }
}
