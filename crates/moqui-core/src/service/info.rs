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

//! What the tools screens read about services: the known names, a definition for the detail screen, and the SECA
//! rules. `ServiceFacadeImpl.getKnownServiceNames`, `getServiceDefinition`, `getSecaRuleCount`, `secaRulesByServiceName`.

use indexmap::IndexMap;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use super::define::{self, Authenticate, Required, ServiceKind};
use crate::transaction::TransactionAttribute;

/// Java's `getKnownServiceNames`, sorted: the services of the `service-file` locations and of every service file in
/// a component, read or not, and those registered from elsewhere. Implicit entity-auto names are not included.
pub fn known_service_names() -> Vec<String> {
    let mut names: std::collections::BTreeSet<String> = define::registered()
        .unwrap_or_default()
        .into_iter()
        .map(|def| def.name)
        .collect();
    names.extend(define::service_names_in_files());
    names.into_iter().collect()
}

/// Java's `getSecaRuleCount`: the rules counted once for each service they run for, so a rule whose name is a pattern
/// counts once for each known service it matches.
pub fn seca_rule_count() -> usize {
    let Some(rules) = super::eca_rules::current() else {
        return 0;
    };
    let mut known = None;
    rules.seca.iter().map(|rule| rule.service_count(&mut known)).sum()
}

/// How many SECA rules are loaded, each once, as Java logs after loading them.
pub fn seca_rules_active() -> usize {
    super::eca_rules::current().map_or(0, |rules| rules.seca.len())
}

/// The SECA rules that apply to `service`, each as the XML of its rule.
pub fn seca_rule_texts(service: &str) -> Vec<String> {
    super::eca_rules::current().map_or_else(Vec::new, |rules| {
        rules
            .seca
            .iter()
            .filter(|rule| rule.applies_to(service))
            .map(|rule| rule.describe())
            .collect()
    })
}

/// `ScheduledJobRunner.isLastRunBeforeLastSchedule`: true with no last run, or when the latest time `cron` came due,
/// in the server time zone, is after `last_run_millis`. A schedule with nothing before now is not due.
///
/// # Errors
///
/// Returns an error when the cron expression does not parse.
pub fn is_last_run_before_last_schedule(
    cron: &str,
    last_run_millis: Option<i64>,
    now_millis: Option<i64>,
) -> Result<bool, crate::context::Error> {
    let cron = super::schedule::parse_cron(cron)?;
    let now = now_millis
        .and_then(|millis| jiff::Timestamp::from_millisecond(millis).ok())
        .unwrap_or_else(jiff::Timestamp::now)
        .to_zoned(jiff::tz::TimeZone::system());
    let Ok(last_schedule) = cron.find_previous_occurrence(&now, true) else {
        return Ok(false);
    };
    Ok(last_run_millis.is_none_or(|last_run| last_run < last_schedule.timestamp().as_millisecond()))
}

/// An English description of a Quartz cron expression, Java's `ScheduledJobRunner.getCronDescription`. The locale and
/// short format are not used: the text is the cron parser's own.
///
/// # Errors
///
/// Returns an error when the cron expression does not parse.
pub fn cron_description(cron: &str) -> Result<String, crate::context::Error> {
    Ok(super::schedule::parse_cron(cron)?.describe())
}

/// The first time a Quartz cron expression (seconds field first) is due strictly after `after_millis`, as epoch
/// millis, as Quartz's `CronExpression.getNextValidTimeAfter` in the server's time zone. `None` when it is never due
/// again.
///
/// # Errors
///
/// Returns an error when the cron expression does not parse.
pub fn next_cron_execution(cron: &str, after_millis: i64) -> Result<Option<i64>, crate::context::Error> {
    let cron = super::schedule::parse_cron(cron)?;
    let Ok(after) = jiff::Timestamp::from_millisecond(after_millis) else {
        return Ok(None);
    };
    let zoned = after.to_zoned(jiff::tz::TimeZone::system());
    Ok(cron
        .find_next_occurrence(&zoned, false)
        .ok()
        .map(|next| next.timestamp().as_millisecond()))
}

/// The definition of `name` as the service detail screen reads a `ServiceDefinition`: the names, type, location,
/// authentication, transaction settings, and the service XML. `None` when there is no such service.
pub fn service_definition_info(name: &str) -> Option<IndexMap<String, Value>> {
    let def = define::find(name)?;
    let mut map = IndexMap::new();
    let text = |value: &str| Value::string(value);
    map.insert("serviceName".to_string(), text(&def.name));
    map.insert("serviceNameNoHash".to_string(), text(&def.name.replace('#', "")));
    map.insert("verb".to_string(), text(&def.verb));
    map.insert("noun".to_string(), text(&def.noun));
    map.insert("serviceType".to_string(), text(&def.service_type));
    map.insert("location".to_string(), text(&def.location));
    map.insert("method".to_string(), Value::Null);
    let authenticate = match def.authenticate {
        Authenticate::Required => "true",
        Authenticate::Off => "false",
        Authenticate::AnonymousAll => "anonymous-all",
        Authenticate::AnonymousView => "anonymous-view",
    };
    map.insert("authenticate".to_string(), text(authenticate));
    map.insert(
        "txIgnore".to_string(),
        Value::Bool(def.transaction == TransactionAttribute::Ignore),
    );
    map.insert(
        "txForceNew".to_string(),
        Value::Bool(matches!(
            def.transaction,
            TransactionAttribute::ForceNew | TransactionAttribute::ForceCache
        )),
    );
    map.insert("noTxCache".to_string(), Value::Bool(def.no_tx_cache));
    map.insert(
        "txUseCache".to_string(),
        Value::Bool(matches!(
            def.transaction,
            TransactionAttribute::Cache | TransactionAttribute::ForceCache
        )),
    );
    map.insert(
        "txTimeout".to_string(),
        def.timeout.map_or(Value::Null, |timeout| {
            Value::I64(i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX))
        }),
    );
    map.insert(
        "xmlAction".to_string(),
        Value::Bool(matches!(def.kind, ServiceKind::Inline) && !def.actions.is_empty()),
    );
    let parameters = |params: &[define::ParamDef]| {
        Value::Map(
            params
                .iter()
                .map(|param| {
                    (
                        param.name.clone(),
                        Value::Map(indexmap::indexmap! {
                            "name".to_string() => text(&param.name),
                            "type".to_string() => text(&param.java_type),
                            "required".to_string() => Value::Bool(param.required == Required::True),
                        }),
                    )
                })
                .collect(),
        )
    };
    let facade = super::validate::current_facade();
    let in_params = super::validate::declared_in(&def, facade.as_ref()).ok()?;
    let out_params = super::validate::expand_params(&def, &def.out_params, facade.as_ref()).ok()?;
    map.insert("inParameterInfoMap".to_string(), parameters(&in_params));
    map.insert("outParameterInfoMap".to_string(), parameters(&out_params));
    map.insert(
        "inParameterNames".to_string(),
        Value::List(in_params.iter().map(|param| text(&param.name)).collect()),
    );
    map.insert("serviceXml".to_string(), text(&service_xml(&def)));
    Some(map)
}

/// The service element as the definition holds it: its attributes, the parameters, and the actions. Java keeps the
/// parsed node; the port keeps what it runs, so a description or a nested parameter is not here.
fn service_xml(def: &define::ServiceDef) -> String {
    let node = MNode::new("service");
    node.set_attribute("verb", def.verb.as_str());
    if !def.noun.is_empty() {
        node.set_attribute("noun", def.noun.as_str());
    }
    node.set_attribute("type", def.service_type.as_str());
    if !def.location.is_empty() {
        node.set_attribute("location", def.location.as_str());
    }
    for (element, params) in [("in-parameters", &def.in_params), ("out-parameters", &def.out_params)] {
        if params.is_empty() {
            continue;
        }
        let list = node.append(element);
        for param in params {
            let child = list.append("parameter");
            child.set_attribute("name", param.name.as_str());
            if !param.java_type.is_empty() && param.java_type != "String" {
                child.set_attribute("type", param.java_type.as_str());
            }
            if param.required == Required::True {
                child.set_attribute("required", "true");
            }
            if let Some(default) = &param.default_expr {
                child.set_attribute("default", default.as_str());
            }
            if let Some(default) = &param.default_value {
                child.set_attribute("default-value", default.as_str());
            }
        }
    }
    if !def.actions.is_empty() {
        if let Ok(actions) = MNode::parse_text("service actions", &def.actions) {
            node.append_node(actions);
        }
    }
    node.write_string()
}
