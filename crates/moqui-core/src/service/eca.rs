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

//! Entity ECA actions, data-file service elements, and feed receive.
//!
//! Security scripts from `Security.eecas.xml` run as Rust checks. Other action
//! bodies run through `script::run_actions`. Feed receive submits the pending rows a commit
//! inserted to a service worker (`crate::search::feed`).

use std::collections::BTreeMap;

use moqui_lib::error::Error;
use moqui_lib::object::{HostObject, Value};
use moqui_lib::sync::lock;

use super::rule_actions::{condition_allows, publish_names, run_stored_actions};
use super::util::{has_error, take_errors_string};
use crate::context::ExecutionContext;
use crate::entity::load_feed::{self, DataFileService, EcaRule};
use crate::entity::EntityFacade;

/// Whether feed receive found search down. Kept with the facade, so it is the same on every thread
/// and one facade's state never shows on another.
#[derive(Default)]
struct SearchState {
    down: std::sync::atomic::AtomicBool,
}

/// True after [`schedule_feed_receive`] remembered at least one receive service for this facade.
pub fn search_not_up(facade: &EntityFacade) -> bool {
    facade.with_slot(SearchState::default, |state| {
        state.down.load(std::sync::atomic::Ordering::Relaxed)
    })
}

/// Run the ECA rules `load_feed::match_rules_with` matched for this write. `before` selects run-before rules.
///
/// As Java's `EntityEcaRule.runIfMatches`: the rule's context has the field values, `entityValue` (a value
/// scripts and called services share, so a field they set on it is set on the write for a run-before rule),
/// `originalValue` (the row before an update or delete, with `get-original-value`: read here for a before rule, and
/// `pre_row`, read by the write before it ran, for an after rule), and
/// `eecaOperation`. `get-entire-entity` fills unset fields from the row for an update or delete. With
/// `set-results`, non-primary-key fields the actions set in the rule's context are set on the values. A rule
/// is skipped while the message facade has errors unless it has `run-on-error` or is one of the Rust security
/// checks.
pub fn run_matched_eca(
    facade: &EntityFacade,
    entity_name: &str,
    operation: &str,
    before: bool,
    rules: &[EcaRule],
    values: &mut crate::entity::FieldMap,
    pre_row: Option<&crate::entity::EntityValue>,
) -> Result<(), Error> {
    if !rules.iter().any(|rule| rule.run_before == before) {
        return Ok(());
    }
    tracing::debug!(entity = entity_name, operation, before, "running matched entity eca");
    let update_or_delete = operation == "update" || operation == "delete";
    for rule in rules {
        if rule.run_before != before {
            continue;
        }
        // The security checks are checks, not business rules, so an earlier error does not skip them.
        if !rule.run_on_error && has_error() && !is_security_check(&rule.actions) {
            continue;
        }
        // Name the rule in a failure: an action error alone does not say where it came from.
        let in_rule = |err: Error| {
            Error::new(format!(
                "entity ECA {} on {operation} of {entity_name}: {}",
                rule.id,
                err.message()
            ))
        };
        let db_row = if update_or_delete && (rule.get_entire_entity || (before && rule.get_original_value)) {
            let keys = facade.pk_map(entity_name, values).map_err(in_rule)?;
            facade.find_by_pk(entity_name, &keys).map_err(in_rule)?
        } else {
            None
        };
        if rule.get_entire_entity {
            if let Some(row) = &db_row {
                for (name, value) in row.fields() {
                    values.entry(name.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        let original = match (&db_row, before && rule.get_original_value) {
            (Some(row), true) => entity_object(row.clone()),
            // an after rule gets the row as the write found it, which is what Java keeps of the old database values
            _ => match pre_row {
                Some(row) if !before && rule.get_original_value && update_or_delete => entity_object(row.clone()),
                _ => Value::Null,
            },
        };
        let mut current = facade.make_value(entity_name).map_err(in_rule)?;
        for (name, value) in values.iter() {
            current.set(std::sync::Arc::clone(name), value.clone());
        }
        let cell: std::sync::Arc<std::sync::Mutex<crate::entity::EntityValue>> =
            std::sync::Arc::new(std::sync::Mutex::new(current));

        // Every field of the entity is a name in the rule's context, null when not set, as the Java value's map is, so an
        // unset field does not read a variable of the same name from the caller.
        let mut published = crate::entity::string_map(values);
        let names = facade
            .with_entity(entity_name, |entity, _, _| {
                Ok(entity.field_names().into_iter().map(str::to_string).collect::<Vec<_>>())
            })
            .map_err(in_rule)?;
        for name in names {
            published.entry(name).or_insert(Value::Null);
        }
        let _frame = publish_names(&published).map_err(to_lib).map_err(in_rule)?;
        let put = |name: &str, value: Value| {
            crate::script::set_context_name(name, value)
                .map_err(to_lib)
                .map_err(in_rule)
        };
        put("entityValue", Value::Object(HostObject(cell.clone())))?;
        put("originalValue", original)?;
        put("eecaOperation", Value::string(operation))?;
        if !condition_allows(&rule.condition).map_err(to_lib).map_err(in_rule)? {
            continue;
        }
        run_stored_actions(&rule.actions).map_err(to_lib).map_err(in_rule)?;
        if before {
            // Fields set on `entityValue` are fields of the write, as the Java value is the one written.
            let shared = lock(&cell);
            for (name, value) in shared.fields() {
                if values.get(&**name) != Some(value) {
                    values.insert(name.clone(), value.clone());
                }
            }
        }
        if rule.set_results {
            let set = crate::script::top_context_entries();
            let fields = facade
                .with_entity(entity_name, |entity, _, _| {
                    let pks = entity.pk_field_names();
                    Ok(entity
                        .field_names()
                        .into_iter()
                        .filter(|name| !pks.contains(name))
                        .map(str::to_string)
                        .collect::<Vec<_>>())
                })
                .map_err(in_rule)?;
            for name in fields {
                if let Some(value) = set.get(&name) {
                    values.insert(name.into(), value.clone());
                }
            }
        }
    }
    Ok(())
}

/// A value scripts see as an entity value.
fn entity_object(value: crate::entity::EntityValue) -> Value {
    Value::Object(HostObject(std::sync::Arc::new(std::sync::Mutex::new(value))))
}

/// Run one data-file service element through [`crate::service::call::sync`].
///
/// `sync` reads the facade on the current execution context. A load that has not
/// entered a context returns an error and does not call the service.
pub fn run_data_file_service(facade: &EntityFacade, service: &DataFileService) -> Result<(), Error> {
    tracing::debug!(
        service = %service.name,
        location = %service.location,
        feed_off = facade.flags().disable_data_feed,
        "running data-file service"
    );
    let mut params = BTreeMap::new();
    for (name, value) in &service.parameters {
        params.insert(name.clone(), Value::string(value));
    }
    // The service runs in the entered context. Without one it cannot run, and saying it did would
    // lose the data-file element silently.
    if !crate::user::context_entered() {
        return Err(Error::new(format!(
            "data-file service {} cannot run without an execution context",
            service.name
        )));
    }
    let _guard = ExecutionContext::with_current(|ctx| ctx.disable_authz());
    let started = std::time::Instant::now();
    tracing::info!(
        "Calling service {} in data load from {} with {} parameters",
        service.name,
        service.location,
        service.parameters.len()
    );
    match super::call::sync(&service.name, &params) {
        Ok(_) => {
            tracing::info!(
                "Called service {} in data load in {}ms",
                service.name,
                started.elapsed().as_millis()
            );
            // As Java's `handleService`: errors the call left fail the element, and are cleared.
            match take_errors_string() {
                Some(errors) => Err(Error::new(format!("Error handling data load service call: {errors}"))),
                None => Ok(()),
            }
        }
        Err(err) => {
            tracing::warn!(
                "Service {} called in data load from {} failed after {}ms: {err}",
                service.name,
                service.location,
                started.elapsed().as_millis()
            );
            Err(to_lib(err))
        }
    }
}

/// The service that consumes `DataFeedPending` rows on a worker.
pub const PROCESS_PENDING_SERVICE: &str = "org.moqui.EntityServices.process#DataFeedPending";

/// After commit, hand the pending rows this transaction inserted to a service worker, which
/// assembles the documents and calls each feed's receive or delete service.
///
/// With no search facade installed, [`search_not_up`] reports it. The worker still runs feeds
/// whose receive service is not the search index. Rows it cannot deliver stay for the
/// `IndexDataFeedDocuments` drain. Without an entered context nothing is submitted.
pub fn schedule_feed_receive(facade: &EntityFacade, pending_ids: Vec<String>) -> Result<(), Error> {
    let search_up =
        ExecutionContext::with_current(|ec| crate::search::installed(ec.factory()).is_some()).unwrap_or(false);
    if !search_up && !load_feed::feed_service_names(facade).is_empty() {
        facade.with_slot(SearchState::default, |state| {
            state.down.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }
    if pending_ids.is_empty() || !crate::user::context_entered() {
        return Ok(());
    }
    // A later search of this session that asks for read-your-writes waits for these rows.
    crate::search::fresh::remember_pending(&pending_ids);
    let mut params = BTreeMap::new();
    params.insert(
        "dataFeedPendingIds".to_string(),
        Value::List(pending_ids.into_iter().map(Value::String).collect()),
    );
    super::call::async_call(PROCESS_PENDING_SERVICE, &params).map_err(to_lib)
}

/// Whether a rule's actions call one of the `UserSecurityChecks` asserts (`Security.eecas.xml`). Those are checks, not
/// business rules, so they run even when the message facade already has errors.
fn is_security_check(actions: &str) -> bool {
    actions.contains("UserSecurityChecks.assertPrivilegedGroupMembership(")
        || actions.contains("UserSecurityChecks.assertSealedPermissionGrant(")
}

pub(super) fn to_lib(err: crate::context::Error) -> Error {
    Error::new(err.message())
}

#[cfg(test)]
#[path = "eca_tests.rs"]
mod tests;
