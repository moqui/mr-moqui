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

use super::define::{self, AllowHtml, Authenticate, Check, Required, ServiceKind};
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
/// in the machine time zone, is after `last_run_millis`. A schedule with nothing before now is not due. Equal times
/// are not due; the job runner's own skip still runs a job whose last run is that occurrence.
///
/// # Errors
///
/// Returns an error when the cron expression does not parse.
pub fn is_last_run_before_last_schedule(
    cron: &str,
    last_run_millis: Option<i64>,
    now_millis: Option<i64>,
) -> Result<bool, crate::context::Error> {
    let now = now_millis
        .and_then(|millis| jiff::Timestamp::from_millisecond(millis).ok())
        .unwrap_or_else(jiff::Timestamp::now);
    cron_is_due(
        cron,
        now.as_millisecond(),
        last_run_millis,
        super::jobs::job_time_zone(),
    )
}

/// True when `cron` is due at `now_millis` in `zone`: a previous occurrence exists, and `last_run_millis` is absent
/// or strictly before it. This is the comparison [`is_last_run_before_last_schedule`] uses.
///
/// # Errors
///
/// Returns an error when the cron expression does not parse.
pub(crate) fn cron_is_due(
    cron: &str,
    now_millis: i64,
    last_run_millis: Option<i64>,
    zone: jiff::tz::TimeZone,
) -> Result<bool, crate::context::Error> {
    let cron = super::schedule::parse_cron(cron)?;
    let Ok(now) = jiff::Timestamp::from_millisecond(now_millis) else {
        return Ok(false);
    };
    let Ok(last_schedule) = cron.find_previous_occurrence(&now.to_zoned(zone), true) else {
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
/// millis, as Quartz's `CronExpression.getNextValidTimeAfter` in the machine's time zone. `None` when it is never due
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
    let zoned = after.to_zoned(super::jobs::job_time_zone());
    Ok(cron
        .find_next_occurrence(&zoned, false)
        .ok()
        .map(|next| next.timestamp().as_millisecond()))
}

/// The definition of `name` as the service detail screen reads a `ServiceDefinition`: the names, type, location,
/// authentication, transaction settings, and the service XML. `None` when there is no such service.
pub fn service_definition_info(name: &str) -> Option<IndexMap<String, Value>> {
    // an entity-auto service has the definition a call makes for it
    let def = super::call::resolve(name, super::validate::current_facade().as_ref()).ok()?;
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
                            "parameterXml".to_string() => text(&param_node(param).write_string()),
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

/// A service as the dev-aide's service view reads it (plans/design-dev-aide.md 9.1): its names, type, location and method,
/// its settings, the file and line of its actions, what it implements, and its in- and out-parameters, nested, each
/// with its type, required, default, allow-html, checks and description; an in-parameter that `auto-parameters`
/// implies is marked `implied`. `None` when there is no such service.
pub fn service_detail(name: &str) -> Option<IndexMap<String, Value>> {
    // an entity-auto service has the definition a call makes for it
    let def = super::call::resolve(name, super::validate::current_facade().as_ref()).ok()?;
    let mut map = service_definition_info(name)?;
    let text = |value: &str| Value::string(value);
    map.insert("method".to_string(), text(&def.method));
    map.insert(
        "kind".to_string(),
        text(match def.kind {
            ServiceKind::Inline => "inline",
            ServiceKind::EntityAuto => "entity-auto",
            ServiceKind::Script => "script",
            ServiceKind::Interface => "interface",
            ServiceKind::RemoteRest => "remote-rest",
            ServiceKind::Rust => "rust",
            ServiceKind::Native => "native",
            ServiceKind::Unsupported => "unsupported",
        }),
    );
    map.insert("validate".to_string(), Value::Bool(def.validate_input));
    map.insert("readOnly".to_string(), Value::Bool(def.read_only));
    map.insert(
        "semaphore".to_string(),
        text(&format!("{:?}", def.semaphore).to_lowercase()),
    );
    if let Some(source) = &def.action_source {
        map.insert(
            "actionsFile".to_string(),
            source.file.as_deref().map_or(Value::Null, text),
        );
        map.insert("actionsLine".to_string(), Value::I64(i64::from(source.line)));
    }
    map.insert(
        "implements".to_string(),
        Value::List(def.implements.iter().map(|item| text(&item.service)).collect()),
    );
    fn param_detail(param: &define::ParamDef, implied: bool) -> Value {
        let mut row = IndexMap::new();
        row.insert("name".to_string(), Value::string(&param.name));
        row.insert("type".to_string(), Value::string(&param.java_type));
        row.insert(
            "required".to_string(),
            Value::string(match param.required {
                Required::True => "true",
                Required::Disabled => "disabled",
                Required::False => "false",
            }),
        );
        row.insert(
            "default".to_string(),
            param
                .default_value
                .as_deref()
                .or(param.default_expr.as_deref())
                .map_or(Value::Null, Value::string),
        );
        row.insert(
            "allowHtml".to_string(),
            Value::string(match param.allow_html {
                AllowHtml::None => "none",
                AllowHtml::Safe => "safe",
                AllowHtml::Any => "any",
            }),
        );
        row.insert(
            "checks".to_string(),
            Value::List(
                param
                    .checks
                    .iter()
                    .map(|check| Value::string(format!("{check:?}")))
                    .collect(),
            ),
        );
        row.insert(
            "description".to_string(),
            param.description.as_deref().map_or(Value::Null, Value::string),
        );
        row.insert("implied".to_string(), Value::Bool(implied));
        row.insert(
            "children".to_string(),
            Value::List(param.children.iter().map(|child| param_detail(child, false)).collect()),
        );
        Value::Map(row)
    }
    let facade = super::validate::current_facade();
    // an entity-auto service's parameters all come from its entity
    let entity_auto = matches!(def.kind, ServiceKind::EntityAuto);
    let declared: std::collections::BTreeSet<&str> = def.in_params.iter().map(|param| param.name.as_str()).collect();
    let in_params = super::validate::declared_in(&def, facade.as_ref()).ok()?;
    map.insert(
        "inParameters".to_string(),
        Value::List(
            in_params
                .iter()
                .map(|item| param_detail(item, entity_auto || !declared.contains(item.name.as_str())))
                .collect(),
        ),
    );
    // an entity-auto service with no parameters of its own takes the entity's fields: its key, required but on a create
    if entity_auto && in_params.is_empty() {
        if let Some(info) = facade
            .as_ref()
            .and_then(|facade| crate::entity::info::definition_info(facade, &def.noun))
        {
            if let Some(Value::List(fields)) = info.get("fields") {
                let rows: Vec<Value> = fields
                    .iter()
                    .filter_map(|field| match field {
                        Value::Map(field) => {
                            let text = |key: &str| {
                                field
                                    .get(key)
                                    .map(moqui_lib::object::to_plain_string)
                                    .unwrap_or_default()
                            };
                            let pk = matches!(field.get("isPk"), Some(Value::Bool(true)));
                            let mut row = IndexMap::new();
                            row.insert("name".to_string(), Value::string(text("name")));
                            row.insert("type".to_string(), Value::string(text("javaType")));
                            row.insert(
                                "required".to_string(),
                                Value::string(if pk && def.verb != "create" { "true" } else { "false" }),
                            );
                            row.insert("default".to_string(), Value::Null);
                            row.insert("allowHtml".to_string(), Value::string("none"));
                            row.insert("checks".to_string(), Value::List(Vec::new()));
                            row.insert("description".to_string(), Value::Null);
                            row.insert("implied".to_string(), Value::Bool(true));
                            row.insert("children".to_string(), Value::List(Vec::new()));
                            Some(Value::Map(row))
                        }
                        _ => None,
                    })
                    .collect();
                map.insert("inParameters".to_string(), Value::List(rows));
            }
        }
    }
    let declared_out: std::collections::BTreeSet<&str> =
        def.out_params.iter().map(|param| param.name.as_str()).collect();
    let out_params = super::validate::expand_params(&def, &def.out_params, facade.as_ref()).ok()?;
    map.insert(
        "outParameters".to_string(),
        Value::List(
            out_params
                .iter()
                .map(|item| param_detail(item, !declared_out.contains(item.name.as_str())))
                .collect(),
        ),
    );
    Some(map)
}

/// The `<parameter>` element of a service parameter, as Java's `ParameterInfo.parameterNode` holds it: the attributes
/// it sets, its description, its nested parameters and its checks.
fn param_node(param: &define::ParamDef) -> MNode {
    let node = MNode::new("parameter");
    node.set_attribute("name", param.name.as_str());
    if !param.java_type.is_empty() && param.java_type != "String" {
        node.set_attribute("type", param.java_type.as_str());
    }
    match param.required {
        Required::True => node.set_attribute("required", "true"),
        Required::Disabled => node.set_attribute("required", "disabled"),
        Required::False => {}
    }
    set_some(&node, "default", param.default_expr.as_deref());
    set_some(&node, "default-value", param.default_value.as_deref());
    match param.allow_html {
        AllowHtml::None => {}
        AllowHtml::Safe => node.set_attribute("allow-html", "safe"),
        AllowHtml::Any => node.set_attribute("allow-html", "any"),
    }
    set_some(&node, "entity-name", param.entity_name.as_deref());
    set_some(&node, "field-name", param.field_name.as_deref());
    set_some(&node, "format", param.format.as_deref());
    if let Some(description) = &param.description {
        node.append("description").set_text(Some(description.clone()));
    }
    for child in &param.children {
        node.append_node(param_node(child));
    }
    for check in &param.checks {
        node.append_node(check_node(check));
    }
    node
}

/// The validation element of a parameter check, with the attributes and children `parse_check` reads.
fn check_node(check: &Check) -> MNode {
    match check {
        Check::Matches { regexp, message } => {
            let node = MNode::new("matches");
            node.set_attribute("regexp", regexp.as_str());
            set_some(&node, "message", Some(message.as_str()).filter(|text| !text.is_empty()));
            node
        }
        Check::NumberRange {
            min,
            min_eq,
            max,
            max_eq,
            message,
        } => {
            let node = MNode::new("number-range");
            set_some(&node, "min", min.as_deref());
            if !*min_eq {
                node.set_attribute("min-include-equals", "false");
            }
            set_some(&node, "max", max.as_deref());
            if *max_eq {
                node.set_attribute("max-include-equals", "true");
            }
            set_some(&node, "message", message.as_deref().filter(|text| !text.is_empty()));
            node
        }
        Check::NumberInteger => MNode::new("number-integer"),
        Check::NumberDecimal => MNode::new("number-decimal"),
        Check::TextLength { min, max } => {
            let node = MNode::new("text-length");
            set_some(&node, "min", min.map(|len| len.to_string()).as_deref());
            set_some(&node, "max", max.map(|len| len.to_string()).as_deref());
            node
        }
        Check::TextEmail => MNode::new("text-email"),
        Check::TextUrl => MNode::new("text-url"),
        Check::TextLetters => MNode::new("text-letters"),
        Check::TextDigits => MNode::new("text-digits"),
        Check::TimeRange { after, before, format } => {
            let node = MNode::new("time-range");
            set_some(&node, "after", after.as_deref());
            set_some(&node, "before", before.as_deref());
            set_some(&node, "format", format.as_deref());
            node
        }
        Check::CreditCard { types } => {
            let node = MNode::new("credit-card");
            set_some(&node, "types", types.as_deref());
            node
        }
        Check::And(checks) => checks_node("val-and", checks),
        Check::Or(checks) => checks_node("val-or", checks),
        Check::Not(checks) => checks_node("val-not", checks),
    }
}

/// A validation that combines `checks`, as `val-and`, `val-or` or `val-not` with them as children.
fn checks_node(name: &str, checks: &[Check]) -> MNode {
    let node = MNode::new(name);
    for check in checks {
        node.append_node(check_node(check));
    }
    node
}

/// Sets the attribute `name` when `value` is some.
fn set_some(node: &MNode, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        node.set_attribute(name, value);
    }
}

/// The service element as the definition holds it: its attributes, the parameters as [`param_node`] builds them, and
/// the actions. Java keeps the parsed node; the port keeps what it runs, so it may differ in layout.
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
            list.append_node(param_node(param));
        }
    }
    if !def.actions.is_empty() {
        if let Ok(actions) = MNode::parse_text("service actions", &def.actions) {
            node.append_node(actions);
        }
    }
    node.write_string()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    /// A parameter with every check parses back from its `param_node` XML to the same checks and the same XML.
    #[test]
    fn a_parameter_with_every_check_round_trips_through_its_node() {
        let dir = std::env::temp_dir().join(format!("moqui-param-node-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let parse = |file: &str, xml: &str| {
            let path = dir.join(file);
            std::fs::write(&path, format!("<services>{xml}</services>")).unwrap();
            let (defs, failures) = crate::service::define::definitions_in(std::slice::from_ref(&path)).unwrap();
            assert!(failures.is_empty(), "{failures:?}");
            defs.into_iter().find(|def| def.verb == "round").unwrap().in_params
        };
        let first = parse(
            "first.xml",
            r#"<service verb="round" noun="Trip" authenticate="false"><in-parameters>
            <parameter name="every" type="Map" required="disabled" default-value="x" allow-html="safe" format="yyyy">
                <description>Every check</description>
                <parameter name="nested" required="true"/>
                <matches regexp="a+" message="letters"/>
                <number-range min="1" max="9" min-include-equals="false" max-include-equals="true" message="range"/>
                <number-integer/><number-decimal/>
                <text-length min="2" max="5"/><text-email/><text-url/><text-letters/><text-digits/>
                <time-range after="a" before="b" format="MM/yyyy"/><credit-card types="visa"/>
                <val-and><number-integer/></val-and><val-or><text-email/><text-url/></val-or><val-not><text-digits/></val-not>
            </parameter></in-parameters></service>"#,
        );
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].checks.len(), 14);
        let xml = super::param_node(&first[0]).write_string();
        let second = parse(
            "second.xml",
            &format!(
                r#"<service verb="round" noun="Trip" authenticate="false"><in-parameters>{xml}</in-parameters></service>"#
            ),
        );
        assert_eq!(format!("{:?}", first[0].checks), format!("{:?}", second[0].checks));
        assert_eq!(super::param_node(&second[0]).write_string(), xml);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 2026-10-19 12:00Z is 06:00 in America/Denver (MDT, UTC−6) and 12:00 in UTC. A `0 0 6 * * ?` job last run at
    /// 06:00Z is due in Denver (that slot is 12:00Z, after the last run) and not due in UTC (that slot is 06:00Z,
    /// equal to the last run, and equal is not due).
    #[test]
    fn a_six_am_job_is_due_in_the_zone_where_it_is_six() {
        let now = jiff::Timestamp::from_str("2026-10-19T12:00:00Z")
            .unwrap()
            .as_millisecond();
        let last = jiff::Timestamp::from_str("2026-10-19T06:00:00Z")
            .unwrap()
            .as_millisecond();
        let due = |zone: &str| {
            super::cron_is_due("0 0 6 * * ?", now, Some(last), jiff::tz::TimeZone::get(zone).unwrap()).unwrap()
        };
        assert!(due("America/Denver"));
        assert!(!due("UTC"));
    }
}
