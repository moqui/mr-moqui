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

//! Foreign-key check before create, only with dummy foreign keys, as Java's `checkFks(true)` from the data loader: a
//! missing related row is created with the keys. Otherwise the database constraint decides. `one-nofk` and
//! auto-reverse are skipped.

use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::marker::PhantomData;
use std::sync::Arc;

use moqui_db::model::{EntityDefinition, EntityModel, Relationship};
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::value::FieldMap;
use super::facade::EntityFacade;
use super::value::EntityValue;

thread_local! {
    /// The `dummy_fks` and `disable_fk_create` of the data loads running on this thread, as Java keeps them on the
    /// loader and the `ExecutionContext`, not the entity facade.
    static SCOPE: Cell<ScopeFlags> = const { Cell::new(ScopeFlags { dummy_fks: false, disable_fk_create: false }) };
}

#[derive(Clone, Copy)]
struct ScopeFlags {
    dummy_fks: bool,
    disable_fk_create: bool,
}

/// Turns on `dummy_fks` and `disable_fk_create` for this thread, for one load, until dropped. Other threads keep the
/// facade flags. Scopes nest, and drop restores the outer scope, also on unwind.
pub(crate) struct FkScope {
    previous: ScopeFlags,
    _thread: PhantomData<*const ()>,
}

impl FkScope {
    pub(crate) fn enter(dummy_fks: bool, disable_fk_create: bool) -> Self {
        let previous = SCOPE.get();
        SCOPE.set(ScopeFlags {
            dummy_fks: previous.dummy_fks || dummy_fks,
            disable_fk_create: previous.disable_fk_create || disable_fk_create,
        });
        Self {
            previous,
            _thread: PhantomData,
        }
    }
}

impl Drop for FkScope {
    fn drop(&mut self) {
        SCOPE.set(self.previous);
    }
}

/// True when a create inserts a dummy row for a missing related row: the facade flag, or a load on this thread.
pub(crate) fn dummy_fks(facade: &EntityFacade) -> bool {
    facade.flags().dummy_fks || SCOPE.get().dummy_fks
}

/// True when foreign keys are neither checked on create nor created with tables: the facade flag, or a load on
/// this thread.
pub(crate) fn fk_create_disabled(facade: &EntityFacade) -> bool {
    facade.flags().disable_fk_create || SCOPE.get().disable_fk_create
}

struct FkJob {
    relationship: String,
    related: String,
    keys: FieldMap,
}

pub(crate) fn enforce(
    facade: &EntityFacade,
    entity_name: &str,
    values: &FieldMap,
    stack: &mut HashSet<String>,
) -> Result<(), Error> {
    if !dummy_fks(facade) {
        return Ok(());
    }
    let jobs = facade.with_entity(entity_name, |entity, _, model| fk_jobs(entity, model, values))?;
    let own_keys = facade.pk_map(entity_name, values).ok();
    let own_name = facade.with_entity(entity_name, |entity, _, _| Ok(entity.full_name.clone()))?;
    for job in jobs {
        // A row that names itself (an organization that owns itself) is satisfied by its own insert, as the
        // database's foreign key is.
        let related_name = facade.with_entity(&job.related, |entity, _, _| Ok(entity.full_name.clone()))?;
        if related_name == own_name && own_keys.as_ref() == Some(&job.keys) {
            continue;
        }
        if row_exists(facade, &job.related, &job.keys)? {
            continue;
        }
        tracing::warn!(
            "Creating dummy {} for foreign key {} on {entity_name}, keys {:?}",
            job.related,
            job.relationship,
            job.keys
        );
        let mut related = EntityValue::make(facade, &job.related)?;
        for (field, value) in &job.keys {
            related.set(field, value.clone());
        }
        facade.insert_record(&related, stack)?;
    }
    Ok(())
}

fn fk_jobs(
    entity: &EntityDefinition,
    model: &EntityModel,
    values: &FieldMap,
) -> Result<Vec<FkJob>, Error> {
    let mut jobs = Vec::new();
    for relationship in &entity.relationships {
        if relationship.is_auto_reverse || relationship.relationship_type != "one" {
            continue;
        }
        let related = model.entity(&relationship.related)?;
        let Some(keys) = related_keys(relationship, related, values)? else {
            continue;
        };
        jobs.push(FkJob {
            relationship: relationship.name.clone(),
            related: related.full_name.clone(),
            keys,
        });
    }
    Ok(jobs)
}

/// `None` when a key field is null or missing: the foreign key is optional.
fn related_keys(
    relationship: &Relationship,
    related: &EntityDefinition,
    values: &FieldMap,
) -> Result<Option<FieldMap>, Error> {
    let pairs = key_pairs(relationship, related)?;
    let mut keys = BTreeMap::new();
    for (source, target) in pairs {
        match values.get(source.as_str()) {
            Some(value) if !value.is_null() => {
                keys.insert(Arc::from(target), value.clone());
            }
            _ => return Ok(None),
        }
    }
    for key_value in &relationship.key_values {
        keys.insert(Arc::from(key_value.related.as_str()), Value::string(key_value.value.clone()));
    }
    let missing: Vec<&str> = related
        .pk_field_names()
        .into_iter()
        .filter(|name| !keys.contains_key(*name))
        .collect();
    // As Java, a dummy is only made when the keys fill the related primary key.
    if !missing.is_empty() {
        tracing::debug!(
            "foreign key {} cannot fill primary key {} on {}, no dummy",
            relationship.name,
            missing.join(", "),
            related.full_name
        );
        return Ok(None);
    }
    Ok(Some(keys))
}

fn key_pairs(relationship: &Relationship, related: &EntityDefinition) -> Result<Vec<(String, String)>, Error> {
    if relationship.key_map.is_empty() {
        let pairs: Vec<(String, String)> = related
            .pk_field_names()
            .into_iter()
            .map(|name| (name.to_string(), name.to_string()))
            .collect();
        if pairs.is_empty() {
            return Err(Error::new(format!(
                "relationship {} has no key-map and {} has no primary key",
                relationship.name, related.full_name
            )));
        }
        return Ok(pairs);
    }
    let related_fields: HashSet<&str> = related.fields.iter().map(|field| field.name.as_str()).collect();
    let related_pk = related.pk_field_names();
    let mut pairs = Vec::with_capacity(relationship.key_map.len());
    for key in &relationship.key_map {
        let mut target = key
            .related
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| key.field_name.clone());
        let related_missing = key.related.as_deref().unwrap_or("").is_empty();
        let use_single_pk = related_pk.len() == 1
            && ((relationship.key_map.len() == 1 && related_missing) || !related_fields.contains(target.as_str()));
        if use_single_pk {
            target = related_pk[0].to_string();
        }
        pairs.push((key.field_name.clone(), target));
    }
    Ok(pairs)
}

fn row_exists(facade: &EntityFacade, entity_name: &str, keys: &FieldMap) -> Result<bool, Error> {
    // Existence does not access localized fields; keep cached/transaction-held rows usable.
    let _raw = super::field_rw::RawReadGuard::enter();
    // Java leaves FK existence to the database and makes no such read, so it is counted in a Rust-only bin.
    let _fk_check = super::RustOnlyEntityHits::enter(super::RustOnlyEntityHits::FK_CHECK);
    let mut find = facade.find(entity_name)?;
    for (field, value) in keys {
        find = find.eq(field, value.clone());
    }
    Ok(find.one()?.is_some())
}

#[cfg(test)]
mod tests {
    use moqui_lib::object::Value;

    use super::{dummy_fks, fk_create_disabled, FkScope};
    use crate::testing;

    #[test]
    fn a_load_scope_is_seen_only_on_its_thread_and_ends_with_it() {
        let facade = testing::open();
        facade.set_disable_fk_create(false);
        {
            let _scope = FkScope::enter(true, true);
            assert!(dummy_fks(&facade) && fk_create_disabled(&facade));
            let other = facade.clone();
            let seen = std::thread::spawn(move || (dummy_fks(&other), fk_create_disabled(&other)))
                .join()
                .unwrap();
            assert_eq!(seen, (false, false), "another thread keeps the facade flags");
            {
                let _inner = FkScope::enter(false, false);
                assert!(dummy_fks(&facade), "an inner scope keeps the outer one");
            }
            assert!(dummy_fks(&facade));
        }
        assert!(!dummy_fks(&facade) && !fk_create_disabled(&facade));
        assert!(!facade.flags().dummy_fks && !facade.flags().disable_fk_create);

        let unwound = std::panic::catch_unwind(|| {
            let _scope = FkScope::enter(true, false);
            panic!("load failed");
        });
        assert!(unwound.is_err());
        assert!(!dummy_fks(&facade), "the scope ends on unwind");
    }

    /// A load with dummy foreign keys makes the missing related row on its own thread only; another thread's create
    /// leaves the foreign key to the database, as Java does outside the data loader.
    #[test]
    fn dummy_fks_make_a_related_row_only_on_the_load_thread() {
        let facade = testing::open();
        facade.set_disable_fk_create(false);
        let _scope = FkScope::enter(true, false);
        let other = facade.clone();
        std::thread::spawn(move || {
            let mut value = other.make_value("moqui.basic.Enumeration")?;
            value.set("enumId", Value::string("E1"));
            value.set("enumTypeId", Value::string("OTHER_TYPE"));
            value.create(&other)
        })
        .join()
        .unwrap()
        .unwrap();
        let mut value = facade.make_value("moqui.basic.Enumeration").unwrap();
        value.set("enumId", Value::string("E2"));
        value.set("enumTypeId", Value::string("LOAD_TYPE"));
        value.create(&facade).unwrap();
        let type_of = |id: &str| {
            facade
                .find("moqui.basic.EnumerationType")
                .unwrap()
                .eq("enumTypeId", Value::string(id))
                .one()
                .unwrap()
        };
        assert!(type_of("OTHER_TYPE").is_none(), "no dummy outside the load thread");
        assert!(type_of("LOAD_TYPE").is_some(), "the load thread made a dummy");
    }
}
