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

//! Entity definitions built at runtime, an overlay on the file model: a DB view entity from its
//! `moqui.entity.view.DbViewEntity` rows, as Java's `EntityFacadeImpl.loadEntityDefinition`, and `DataDocument.<id>`,
//! a data document's view, as Java's `EntityDataDocument.makeEntityDefinition`.
//!
//! A name the file model does not have is looked up here ([`lookup`]): built on its first lookup and kept, and a name
//! that is neither kept as not found, so asking again costs no query. A write to a `DbViewEntity` or `DataDocument`
//! entity, here or from a cluster peer, drops them all ([`note_write`]). A definition that does not build is logged
//! with its name and is not found until then; other lookups go on.
//!
//! Both are completed against the file model, so their members are file entities. Neither is ever cached: Java caches a
//! `DbViewEntity` with `cache="Y"`, but the entity cache would then need to forget a view's finds when its definition
//! is dropped, and a member's write to reach the views built here.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use moqui_db::model::EntityDefinition;
use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::{read, write};

use super::facade::EntityFacade;
use crate::transaction::TransactionAttribute;

const DB_VIEW: &str = "moqui.entity.view.DbViewEntity";
const DB_VIEW_MEMBER: &str = "moqui.entity.view.DbViewEntityMember";
const DB_VIEW_KEY_MAP: &str = "moqui.entity.view.DbViewEntityKeyMap";
const DB_VIEW_ALIAS: &str = "moqui.entity.view.DbViewEntityAlias";
const DOCUMENT: &str = "moqui.entity.document.DataDocument";
const DOCUMENT_PREFIX: &str = "DataDocument.";

/// With this many names kept, the names kept as not found are dropped, so lookups of names that are not entities do
/// not grow the map without end.
const MAX_ENTRIES: usize = 10_000;

/// The overlay, kept with the facade.
#[derive(Default)]
pub(crate) struct DerivedDefinitions {
    /// Counts the drops, so a build that read the rows before one is not kept.
    generation: AtomicU64,
    /// By the name looked up and by the full name; `None` is a name known not to be one.
    entries: RwLock<HashMap<String, Option<Arc<EntityDefinition>>>>,
}

thread_local! {
    /// The names being built on this thread. A document whose primary entity is its own view is not found, rather than
    /// built again without end.
    static BUILDING: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Takes `name` off [`BUILDING`] when the build is over, also when it panics.
struct Building;

impl Drop for Building {
    fn drop(&mut self) {
        BUILDING.with(|building| {
            building.borrow_mut().pop();
        });
    }
}

/// The definition built for `name`, a name the file model does not have. `None` when it is not a DB view entity or a
/// data document, or did not build.
pub(crate) fn lookup(facade: &EntityFacade, name: &str) -> Option<Arc<EntityDefinition>> {
    if name.is_empty() {
        return None;
    }
    let derived = &facade.inner.derived;
    if let Some(entry) = read(&derived.entries).get(name) {
        return entry.clone();
    }
    if BUILDING.with(|building| building.borrow().iter().any(|known| known == name)) {
        return None;
    }
    let generation = derived.generation.load(Ordering::Acquire);
    let built = {
        BUILDING.with(|building| building.borrow_mut().push(name.to_string()));
        let _building = Building;
        build(facade, name)
    };
    let entity = match built {
        Ok(entity) => entity.map(Arc::new),
        Err(err) => {
            tracing::error!("Could not build the entity definition {name}, not found until its rows change: {err}");
            None
        }
    };
    let mut entries = write(&derived.entries);
    if derived.generation.load(Ordering::Acquire) == generation {
        if entries.len() >= MAX_ENTRIES {
            entries.retain(|_, entry| entry.is_some());
        }
        if let Some(entity) = &entity {
            entries.insert(entity.full_name.clone(), Some(Arc::clone(entity)));
        }
        entries.insert(name.to_string(), entity.clone());
    }
    entity
}

/// Drop every built definition and every name kept as not found after a write to a `DbViewEntity` or `DataDocument`
/// entity. The entity write hook calls it, at the write and again at the end of its transaction, and so does a cluster
/// peer's cache invalidation.
pub(crate) fn note_write(facade: &EntityFacade, full_name: &str) {
    if full_name.starts_with(DB_VIEW) || full_name.starts_with(DOCUMENT) {
        clear(facade);
    }
}

/// Drop every built definition, as the tools screens' `clearEntityDefinitionFromCache` (Java drops the one named).
pub(crate) fn clear(facade: &EntityFacade) {
    let derived = &facade.inner.derived;
    let mut entries = write(&derived.entries);
    derived.generation.fetch_add(1, Ordering::AcqRel);
    entries.clear();
}

/// As Java's `find` of a `DataDocument.<id>` name: the document's conditions that are not `postQuery`. `None` for any
/// other entity.
pub(crate) fn document_conditions(facade: &EntityFacade, full_name: &str) -> Result<Option<Vec<Condition>>, Error> {
    let Some(id) = full_name.strip_prefix(DOCUMENT_PREFIX) else {
        return Ok(None);
    };
    if facade.with_model(|model| model.entity_full(full_name).is_some()) {
        return Ok(None);
    }
    let info = super::document::document_info(facade, id)?;
    info.conditions
        .iter()
        .filter(|condition| !condition.post_query)
        .map(super::document::document_condition)
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn build(facade: &EntityFacade, name: &str) -> Result<Option<EntityDefinition>, Error> {
    let document_id = name.strip_prefix(DOCUMENT_PREFIX);
    // As Java, only when the model has the entity of the rows.
    let rows_entity = if document_id.is_some() { DOCUMENT } else { DB_VIEW };
    if facade.with_model(|model| model.entity_full(rows_entity).is_none()) {
        return Ok(None);
    }
    // The definition rows are read whoever asks, as Java reads them.
    let _authz = crate::context::ExecutionContext::with_current(|ec| ec.disable_authz());
    // Outside the caller's transaction, which is suspended, where Java reads in it: a failed read, as on a database
    // without the table, then cannot mark it for rollback, and the loader asks whether each data file element is an
    // entity inside its file transaction. Rows that transaction wrote are not seen until it commits, when the write
    // hook drops the overlay.
    let view = facade
        .transactions_for(rows_entity)?
        .run(TransactionAttribute::IgnoreSuspend, None, || match document_id {
            Some(id) => document_view(facade, id),
            None => db_view(facade, name),
        })?;
    Ok(view.map(|mut view| {
        view.cache = "never".to_string();
        view
    }))
}

/// Java's `loadEntityDefinition` of a `DbViewEntity`: a `view-entity` of its members (each joined from its
/// `joinFromAlias` by its key maps, optional with `joinOptional` `Y`) and aliases. A name with a package must be the
/// row's package; a row with none has Java's full name `null.{name}`.
fn db_view(facade: &EntityFacade, name: &str) -> Result<Option<EntityDefinition>, Error> {
    let (package, view_name) = match name.rsplit_once('.') {
        Some((package, view_name)) => (Some(package), view_name),
        None => (None, name),
    };
    let key = || Value::string(view_name);
    let Some(row) = facade
        .find(DB_VIEW)?
        .eq("dbViewEntityName", key())
        .use_cache(false)
        .one()?
    else {
        return Ok(None);
    };
    let package_name = text(row.get("packageName")).unwrap_or_else(|| "null".to_string());
    if package.is_some_and(|package| package != package_name) {
        return Ok(None);
    }
    let node = MNode::new("view-entity");
    node.set_attribute("entity-name", view_name);
    node.set_attribute("package", package_name);
    let members = facade
        .find(DB_VIEW_MEMBER)?
        .eq("dbViewEntityName", key())
        .order_by("entityAlias", false)
        .use_cache(false)
        .list()?;
    for member in members.iter() {
        let entity_alias = text(member.get("entityAlias")).unwrap_or_default();
        let member_node = node.append("member-entity");
        member_node.set_attribute("entity-alias", entity_alias.clone());
        member_node.set_attribute("entity-name", text(member.get("entityName")).unwrap_or_default());
        let Some(join_from) = text(member.get("joinFromAlias")) else {
            continue;
        };
        member_node.set_attribute("join-from-alias", join_from.clone());
        if text(member.get("joinOptional")).as_deref() == Some("Y") {
            member_node.set_attribute("join-optional", "true");
        }
        let key_maps = facade
            .find(DB_VIEW_KEY_MAP)?
            .eq("dbViewEntityName", key())
            .eq("joinFromAlias", Value::string(join_from))
            .eq("entityAlias", Value::string(entity_alias))
            .order_by("fieldName", false)
            .use_cache(false)
            .list()?;
        for key_map in key_maps.iter() {
            let key_node = member_node.append("key-map");
            key_node.set_attribute("field-name", text(key_map.get("fieldName")).unwrap_or_default());
            if let Some(related) = text(key_map.get("relatedFieldName")) {
                key_node.set_attribute("related", related);
            }
        }
    }
    let aliases = facade
        .find(DB_VIEW_ALIAS)?
        .eq("dbViewEntityName", key())
        .order_by("fieldAlias", false)
        .use_cache(false)
        .list()?;
    for alias in aliases.iter() {
        let alias_node = node.append("alias");
        alias_node.set_attribute("name", text(alias.get("fieldAlias")).unwrap_or_default());
        alias_node.set_attribute("entity-alias", text(alias.get("entityAlias")).unwrap_or_default());
        if let Some(field) = text(alias.get("fieldName")) {
            alias_node.set_attribute("field", field);
        }
        if let Some(function) = text(alias.get("functionName")) {
            alias_node.set_attribute("function", function);
        }
    }
    facade.with_model(|model| model.derive_view(&node)).map(Some)
}

/// Java's `makeEntityDefinition`: the document's view, `DataDocument.{id}`. A document that is not there is not found
/// without an error.
fn document_view(facade: &EntityFacade, id: &str) -> Result<Option<EntityDefinition>, Error> {
    if facade
        .find(DOCUMENT)?
        .eq("dataDocumentId", Value::string(id))
        .use_cache(false)
        .one()?
        .is_none()
    {
        return Ok(None);
    }
    let info = super::document::document_info(facade, id)?;
    let node = super::document::entity_view_node(facade, &info)?;
    facade.with_model(|model| model.derive_view(&node)).map(Some)
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .filter(|value| !matches!(value, Value::Null))
        .map(to_plain_string)
        .filter(|text| !text.is_empty())
}
