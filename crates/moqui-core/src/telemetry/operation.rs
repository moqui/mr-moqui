//! Business operations (plans/Monitoring.md, Later: Business operations). An operation is a service call that owns
//! its transaction frame, the outermost one or a `force-new` one, named by the service unless the service declares
//! `operation` or the conf names it. Its outcome comes from the frame's commit and rollback hooks: `commit`,
//! `rollback`, `retry` (rolled back, and the work will run again) or `pending` (queued for a worker). Runs are the
//! `moqui.operation.runs` counter and the `moqui.operation.duration` histogram, by operation name and outcome; the
//! correlation reference is a span attribute only, never a metric label.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

use crate::entity::document::Document;

/// The name recorded for operations past `max-names`.
pub const OTHER: &str = "other";
/// A call parameter that says a rollback of this call will be retried, for a caller that retries by calling again
/// (the SystemMessage consume and send services). It is removed before the service sees its parameters.
pub const RETRY_PARAMETER: &str = "_operationRetry";
const MAX_REFERENCE: usize = 128;

/// Which service calls are operations (telemetry/operations record).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// Every call that owns its transaction, unless a rule says not.
    Default,
    /// Only services that declare `operation`, or that a rule names.
    Declared,
    None,
}

#[derive(Clone, Debug)]
struct Rule {
    service: regex::Regex,
    name: Option<String>,
    reference: Option<String>,
    record: bool,
}

/// telemetry/operations.
#[derive(Clone, Debug)]
pub struct Conf {
    pub record: Record,
    pub max_names: usize,
    rules: Vec<Rule>,
}

impl Default for Conf {
    fn default() -> Self {
        Self {
            record: Record::Default,
            max_names: 500,
            rules: Vec::new(),
        }
    }
}

impl Conf {
    /// Read telemetry/operations, adding a warning for each part that is not used.
    pub(super) fn from_node(node: &MNode, warnings: &mut Vec<String>) -> Self {
        let mut conf = Self::default();
        match node.attribute("record").as_deref().map(str::trim) {
            None | Some("") | Some("default") => {}
            Some("declared") => conf.record = Record::Declared,
            Some("none") => conf.record = Record::None,
            Some(other) => warnings.push(format!(
                "telemetry operations record [{other}] is not default, declared or none; using default"
            )),
        }
        if let Some(text) = node.attribute("max-names").filter(|text| !text.trim().is_empty()) {
            match text.trim().parse::<usize>() {
                Ok(value) if value > 0 => conf.max_names = value,
                _ => warnings.push(format!(
                    "telemetry operations max-names [{text}] is not a positive whole number; using {}",
                    conf.max_names
                )),
            }
        }
        for item in node.named_iter("operation") {
            let Some(pattern) = item.attribute("service").filter(|text| !text.is_empty()) else {
                warnings.push("telemetry operation without a service is ignored".into());
                continue;
            };
            let service = match regex::Regex::new(&format!("^(?:{pattern})$")) {
                Ok(service) => service,
                Err(error) => {
                    warnings.push(format!(
                        "telemetry operation service [{pattern}] is not a pattern: {error}"
                    ));
                    continue;
                }
            };
            conf.rules.push(Rule {
                service,
                name: item.attribute("name").filter(|name| !name.is_empty()),
                reference: item.attribute("ref").filter(|name| !name.is_empty()),
                record: item.attribute("record").as_deref() != Some("false"),
            });
        }
        conf
    }

    /// The operation a call of `service` is, from the first rule that matches it and the service's own declaration.
    pub fn resolve(&self, service: &str, declared: Option<&str>, declared_ref: Option<&str>) -> Option<Operation> {
        if self.record == Record::None {
            return None;
        }
        let rule = self.rules.iter().find(|rule| rule.service.is_match(service));
        if rule.is_some_and(|rule| !rule.record) {
            return None;
        }
        let named = rule.and_then(|rule| rule.name.as_deref()).or(declared);
        if named.is_none() && rule.is_none() && self.record == Record::Declared {
            return None;
        }
        Some(Operation {
            name: named.unwrap_or(service).to_string(),
            reference: rule
                .and_then(|rule| rule.reference.as_deref())
                .or(declared_ref)
                .map(str::to_string),
        })
    }
}

/// An operation: its stable name and the parameter whose value is its correlation reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Operation {
    pub name: String,
    pub reference: Option<String>,
}

impl Operation {
    /// The correlation reference of a call: `parameter:value`, an id the call was given, never document contents.
    pub fn reference_of(&self, parameters: &std::collections::BTreeMap<String, Value>) -> Option<String> {
        let parameter = self.reference.as_deref()?;
        let value = moqui_lib::object::to_plain_string(parameters.get(parameter)?);
        if value.is_empty() {
            return None;
        }
        Some(format!(
            "{parameter}:{}",
            value.chars().take(MAX_REFERENCE).collect::<String>()
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Commit,
    Rollback,
    Retry,
    Pending,
}

impl Outcome {
    /// The outcome of a run that ended: a rollback is `retry` when the work will run again.
    pub fn ended(committed: bool, retries: bool) -> Self {
        match (committed, retries) {
            (true, _) => Self::Commit,
            (false, true) => Self::Retry,
            (false, false) => Self::Rollback,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Rollback => "rollback",
            Self::Retry => "retry",
            Self::Pending => "pending",
        }
    }
}

/// The resolved operation of each service, and the names recorded so far, held to `max-names`.
#[derive(Default)]
pub(super) struct Registry {
    resolved: Mutex<HashMap<String, Option<Arc<Operation>>>>,
    names: Mutex<HashSet<String>>,
    warned: std::sync::atomic::AtomicBool,
}

impl Registry {
    pub(super) fn operation(
        &self,
        conf: &Conf,
        service: &str,
        declared: Option<&str>,
        declared_ref: Option<&str>,
    ) -> Option<Arc<Operation>> {
        if let Some(known) = lock(&self.resolved).get(service) {
            return known.clone();
        }
        let resolved = conf.resolve(service, declared, declared_ref).map(|mut operation| {
            if !self.admit(conf, &operation.name) {
                operation.name = OTHER.to_string();
            }
            Arc::new(operation)
        });
        lock(&self.resolved).insert(service.to_string(), resolved.clone());
        resolved
    }

    /// Whether `name` is among the first `max-names` names, so the series stay bounded.
    fn admit(&self, conf: &Conf, name: &str) -> bool {
        let mut names = lock(&self.names);
        if names.contains(name) {
            return true;
        }
        if names.len() < conf.max_names {
            names.insert(name.to_string());
            return true;
        }
        if !self.warned.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!(
                limit = conf.max_names,
                operation = name,
                "telemetry operations max-names reached; further operations are recorded as other"
            );
        }
        false
    }
}

/// The metric attributes of a run.
pub(super) fn attributes(name: &str, outcome: Outcome) -> Document {
    let mut attributes = Document::new();
    attributes.insert("moqui.operation.name".into(), Value::string(name));
    attributes.insert("outcome".into(), Value::string(outcome.as_str()));
    attributes
}

#[cfg(test)]
#[path = "operation_tests.rs"]
mod tests;
