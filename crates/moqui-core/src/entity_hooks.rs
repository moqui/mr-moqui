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

//! The framework's [`EntityHooks`]: what the entity facade reaches through the context entered on the thread.

use std::time::Instant;

use moqui_lib::object::Value;

use crate::context::{Authz, ExecutionContext, Executor, Tarpit};
use crate::telemetry::{ContextGuard, Span, SpanKind, Telemetry};
use moqui_entity::hooks::{EntityHooks, Guard, StatementArtifact, StatementTrace, TransactionTrace};

moqui_entity::hooks::submit! {
    EntityHooks {
        entered: crate::user::context_entered,
        current_facade: || ExecutionContext::with_current(|ec| ec.factory().entity()).flatten(),
        effective_millis: || ExecutionContext::with_current(ExecutionContext::effective_time).flatten(),
        user_id: || crate::user::current_user_id().ok().flatten(),
        visit_id: || crate::user::visit_id().ok().flatten(),
        agent_id: || ExecutionContext::with_current(|ec| ec.agent_origin()).flatten().map(|origin| origin.agent_id),
        in_group: crate::user::is_in_group,
        locale: crate::l10n::locale,
        time_zone: crate::l10n::time_zone,
        entity_eca_disabled: || ExecutionContext::with_current(|ec| ec.entity_eca_disabled()) == Some(true),
        conf_setting: |path, name| {
            ExecutionContext::with_current(|ec| ec.factory().conf_setting(path, name)).unwrap_or_default()
        },
        locate: crate::context::locate,
        add_message: |text| {
            ExecutionContext::with_current(|ec| ec.message().add_message(text));
        },
        add_error: |text| {
            ExecutionContext::with_current(|ec| ec.message().add_error(text));
        },
        message_errors: || ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default(),
        take_errors: crate::service::util::take_errors_string,
        note_no_search_params: || {
            ExecutionContext::with_current(|ec| {
                ec.shared_map().insert("_entityListNoSearchParms".to_string(), Value::Bool(true));
            });
        },
        authz_disabled: || ExecutionContext::with_current(|ec| ec.authz_disabled()).unwrap_or(false),
        disable_authz: || ExecutionContext::with_current(|ec| Box::new(ec.disable_authz()) as Guard),
        push_entity: |name, action, requires_authz, sub_type| {
            ExecutionContext::with_current(|ec| {
                ec.push_checked("AT_ENTITY", name, action, Authz::required_if(requires_authz), Tarpit::Skip, None)?;
                ec.set_hit_detail(Some(sub_type), None);
                Ok(())
            })
        },
        entity_permitted: |name, action, requires_authz| {
            ExecutionContext::with_current(|_| crate::user::entity_permitted(name, action, requires_authz))
        },
        pop_artifact: || {
            ExecutionContext::with_current(|ec| {
                let _ = ec.pop();
            });
        },
        set_hit_detail: |sub_type| {
            ExecutionContext::with_current(|ec| ec.set_hit_detail(Some(sub_type), None));
        },
        stack_names: || ExecutionContext::with_current(ExecutionContext::artifact_stack_names).unwrap_or_default(),
        entity_filters: crate::user::apply_entity_filters,
        view_filters: crate::user::apply_view_filters,
        note_write: crate::user::note_entity_write,
        push_frame: crate::script::push_context_frame,
        pop_frame: || drop(crate::script::ContextFrame),
        set_name: crate::script::set_context_name,
        eval: crate::script::eval_value,
        expand: crate::script::expand,
        context_value: crate::script::context_value,
        value_map: crate::script::value_map,
        value_items: crate::script::value_items,
        call: |name, parameters, disable_authz| {
            let options = crate::service::call::CallOptions { disable_authz, ..Default::default() };
            crate::service::call::sync_with(name, parameters, &options)
        },
        store_entity: crate::service::call::store_entity,
        run_eca: crate::service::run_matched_eca,
        run_data_file_service: crate::service::run_data_file_service,
        schedule_feed_receive: crate::service::schedule_feed_receive,
        defined_service_name: crate::service::define::defined_name,
        registration_failures: crate::service::define::registration_failures,
        cluster_note_write: note_write,
        trace_statement,
        trace_transaction,
        artifact_name: || {
            let (entity, artifact) =
                ExecutionContext::with_current(ExecutionContext::statement_artifact).unwrap_or_default();
            StatementArtifact { entity, artifact }
        },
        inline_executor: || {
            ExecutionContext::with_current(|ec| ec.factory().executor() == Executor::Inline).unwrap_or(false)
        },
    }
}

/// A committed write's observers: the cluster's cache invalidation, and the edge link's replication
/// (plans/Edge-Node.md §5.2), which notes the record's key for its set after commit.
fn note_write(
    facade: &crate::entity::EntityFacade,
    entity_name: &str,
    pk_fields: &[String],
    values: &crate::entity::FieldMap,
) -> Result<(), crate::context::Error> {
    crate::edge::replica::note_write(facade, entity_name, pk_fields, values);
    crate::cluster::note_write(facade, entity_name, pk_fields, values)
}

/// A statement's duration metric, and its span inside a trace: a scheduler poll or a data load outside any request or
/// job would otherwise write a trace per statement. It keeps its frame's head decision, so a frame sampled by its
/// type's ratio keeps its statements.
struct SqlTrace {
    started: Instant,
    telemetry: std::sync::Arc<Telemetry>,
    system: Option<std::sync::Arc<str>>,
    span: Option<Span>,
    _attached: Option<ContextGuard>,
}

/// The `AT_TRANSACTION` artifact type: a transaction's span, inside a trace (as a statement's), and its duration metric
/// `moqui.transaction.duration` by outcome and cache mode. `telemetry/artifact type="AT_TRANSACTION"` sets `spans`,
/// `metrics` and `sample-ratio`, as for the other types.
const AT_TRANSACTION: &str = "AT_TRANSACTION";

struct TxTrace {
    started: Instant,
    telemetry: std::sync::Arc<Telemetry>,
    metrics: bool,
    span: Option<Span>,
}

fn trace_transaction() -> Option<Box<dyn TransactionTrace>> {
    let telemetry = crate::telemetry::current()?;
    let (spans, metrics) = telemetry.artifact_enabled(AT_TRANSACTION);
    if !spans && !metrics {
        return None;
    }
    let span = (spans && crate::telemetry::capture().is_some())
        .then(|| telemetry.start_artifact_span(AT_TRANSACTION, "transaction", SpanKind::Internal));
    Some(Box::new(TxTrace {
        started: Instant::now(),
        telemetry,
        metrics,
        span,
    }))
}

impl TransactionTrace for TxTrace {
    fn finish(mut self: Box<Self>, committed: bool, attributes: Vec<(&'static str, Value)>) {
        let outcome = if committed { "commit" } else { "rollback" };
        if self.metrics {
            let mut metric = crate::entity::document::Document::new();
            metric.insert("outcome".into(), Value::string(outcome));
            if let Some((_, mode)) = attributes.iter().find(|(name, _)| *name == "moqui.transaction.cache") {
                metric.insert("moqui.transaction.cache".into(), mode.clone());
            }
            self.telemetry.record_duration(
                "moqui.transaction.duration",
                self.started.elapsed().as_secs_f64(),
                metric,
            );
        }
        if let Some(mut span) = self.span.take() {
            if span.is_recording() {
                span.set_attribute("moqui.transaction.outcome", Value::string(outcome));
                for (name, value) in attributes {
                    span.set_attribute(name, value);
                }
            }
            span.finish();
        }
    }
}

fn trace_statement(sql: &str) -> Option<Box<dyn StatementTrace>> {
    let started = Instant::now();
    let telemetry = crate::telemetry::current()?;
    let system =
        ExecutionContext::with_current(|context| context.factory().entity().and_then(|facade| facade.db_system()))
            .flatten();
    let span = crate::telemetry::capture().map(|parent| {
        let mut span = telemetry.start_child_span("sql", SpanKind::Client, parent);
        if span.is_recording() {
            span.set_attribute("db.query.text", Value::String(crate::telemetry::redact_sql(sql)));
            if let Some(system) = &system {
                span.set_attribute("db.system.name", Value::string(&**system));
            } else {
                span.set_attribute("moqui.db.system.uncertain", Value::Bool(true));
            }
        }
        span
    });
    let attached = span
        .as_ref()
        .and_then(Span::context)
        .map(|context| crate::telemetry::attach(Some(context)));
    Some(Box::new(SqlTrace {
        started,
        telemetry,
        system,
        span,
        _attached: attached,
    }))
}

impl StatementTrace for SqlTrace {
    fn finish(mut self: Box<Self>, error: Option<&str>) {
        let mut attributes = crate::entity::document::Document::new();
        if let Some(system) = &self.system {
            attributes.insert("db.system.name".into(), Value::string(&**system));
        }
        self.telemetry.record_duration(
            "db.client.operation.duration",
            self.started.elapsed().as_secs_f64(),
            attributes,
        );
        if let Some(mut span) = self.span.take() {
            if let Some(error) = error {
                span.set_error(error);
            }
            span.finish();
        }
    }
}
