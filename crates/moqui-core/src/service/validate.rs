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

//! In-parameter checks: required, matches, ranges, and entity existence.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use moqui_lib::object::{
    basic_convert, convert_checked, decimal_compare, decimal_eq, is_empty, regex_full_match, to_plain_string, Value,
};

use super::define::{AllowHtml, AutoParams, Check, Include, ParamDef, Required, ServiceDef, ServiceKind};
use crate::context::Error;
use crate::entity::EntityFacade;

pub fn validate(name: &str, parameters: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, Error> {
    let def = super::define::find(name).ok_or_else(|| Error::new(format!("no service named {name}")))?;
    let facade = current_facade();
    clean(&def, parameters, facade.as_ref())
}

pub(crate) fn clean(
    def: &ServiceDef,
    input: &BTreeMap<String, Value>,
    facade: Option<&EntityFacade>,
) -> Result<BTreeMap<String, Value>, Error> {
    let declared = declared_in(def, facade)?;
    let mut params = input.clone();
    let mut known = BTreeSet::new();
    // As Java, every parameter is checked and each that fails is a validation error for its field, so a form shows
    // them all beside their fields; the service does not run when there is one.
    let mut failed: Vec<(String, Error)> = Vec::new();
    let mut scope = DefaultScope::default();
    for param in declared.iter() {
        if param.name.is_empty() || param.required == Required::Disabled {
            continue;
        }
        known.insert(param.name.clone());
        match clean_param(param, &params, &mut scope) {
            Ok(Some(value)) => {
                scope.set(&param.name, &value)?;
                params.insert(param.name.clone(), value);
            }
            Ok(None) => {}
            Err(err) => failed.push((param.name.clone(), err)),
        }
    }
    if !failed.is_empty() {
        // A validation error's message is Java's own, which does not name the field; the error text does.
        let shown = |err: &Error| match err.kind() {
            crate::context::ErrorKind::Validation => err.message().to_string(),
            _ => {
                let message = err.message();
                let text = message.strip_prefix("parameter ").unwrap_or(message);
                format!("{text} (in service {})", def.name)
            }
        };
        let joined = failed
            .iter()
            .map(|(field, err)| match err.kind() {
                crate::context::ErrorKind::Validation => format!("{} (for field {field})", err.message()),
                _ => err.message().to_string(),
            })
            .collect::<Vec<_>>()
            .join("; ");
        crate::context::ExecutionContext::with_current(|ec| {
            let mut messages = ec.message();
            for (field, err) in &failed {
                messages.add_validation_error("", field, &def.name, &shown(err));
            }
        });
        return Err(Error::of_kind(crate::context::ErrorKind::Validation, joined));
    }
    if def.validate_input {
        retain_known(def, &mut params, &known, facade)?;
    }
    Ok(params)
}

/// One parameter: its default when blank, the required check, conversion to its type, HTML cleaning, and the
/// declared checks. The value to store, if any.
fn clean_param(
    param: &ParamDef,
    params: &BTreeMap<String, Value>,
    scope: &mut DefaultScope,
) -> Result<Option<Value>, Error> {
    let mut defaulted = None;
    if is_blank(params.get(&param.name)) {
        if let Some(value) = default_for(param, params, scope)? {
            defaulted = Some(value);
        }
    }
    let current = defaulted.clone().or_else(|| params.get(&param.name).cloned());
    if param.required == Required::True && is_blank(current.as_ref()) {
        // Java's message, which the page shows with the field's name.
        return Err(Error::of_kind(
            crate::context::ErrorKind::Validation,
            crate::l10n::localize("Field cannot be empty").unwrap_or_else(|_| "Field cannot be empty".to_string()),
        ));
    }
    let Some(value) = current else {
        return Ok(None);
    };
    if is_empty(&value) {
        // As Java, an empty value with no default whose type is not the parameter's is null rather than converted,
        // so a blank price field reaches a BigDecimal parameter as null.
        let has_default = param.default_expr.is_some() || param.default_value.is_some();
        if defaulted.is_none() && !has_default && !empty_matches_type(&value, &param.java_type) {
            return Ok(Some(Value::Null));
        }
        return Ok(defaulted);
    }
    // Convert first, so the checks see the type they were written for.
    let value = if matches!(value, Value::Map(_) | Value::List(_)) {
        value
    } else if let Some(items) = text_collection(&value, &param.java_type) {
        items
    } else if let Some(stamp) = text_timestamp(&value, param) {
        // As Java's ParameterInfo, a time stamp entered as text is read by the parameter's format in the user's zone.
        stamp
    } else {
        convert_checked(&value, &param.java_type).ok_or_else(|| {
            Error::new(format!(
                "parameter {} value {} is not a valid {}",
                param.name,
                to_plain_string(&value),
                short_type(&param.java_type)
            ))
        })?
    };
    let value = clean_html(param, value)?;
    check_param(param, &value)?;
    Ok(Some(value))
}

/// A `Timestamp` parameter given as text, parsed as `ec.l10n.parseTimestamp(text, format)`. None for any other
/// parameter or value, and for text that is no time stamp, which the plain conversion then reports.
fn text_timestamp(value: &Value, param: &ParamDef) -> Option<Value> {
    if short_type(&param.java_type) != "Timestamp" {
        return None;
    }
    let text = value.as_str()?;
    crate::l10n::parse_timestamp_format(text.trim(), param.format.as_deref())
}

/// `ParameterInfo` for a `List` or `Set` parameter given as text: square brackets stripped, then split on commas, or
/// the one string. A set keeps the first of each value.
fn text_collection(value: &Value, java_type: &str) -> Option<Value> {
    let Value::String(text) = value else {
        return None;
    };
    let simple = java_type.rsplit('.').next().unwrap_or(java_type);
    let set = match simple {
        "List" | "ArrayList" | "Collection" => false,
        "Set" | "HashSet" | "TreeSet" | "LinkedHashSet" => true,
        _ => return None,
    };
    let inner = text
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(text);
    let mut items: Vec<Value> = Vec::new();
    for part in inner.split(',') {
        let item = Value::string(part);
        if !set || !items.contains(&item) {
            items.push(item);
        }
    }
    Some(Value::List(items))
}

/// A service's in-parameters with `auto-parameters` expanded, kept with the entity facade they were expanded from.
#[derive(Default)]
pub(crate) struct ExpandedParams(Mutex<Option<Expanded>>);

struct Expanded {
    /// The facade's model the parameters were expanded from, or None when no parameter reads one.
    facade: Option<Weak<dyn Any + Send + Sync>>,
    params: Arc<[ParamDef]>,
}

impl Clone for ExpandedParams {
    /// A changed copy of a service expands its own parameters.
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for ExpandedParams {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ExpandedParams")
    }
}

/// The facade's identity. The weak reference keeps the address from being reused while it is held.
fn facade_key(facade: &EntityFacade) -> Weak<dyn Any + Send + Sync> {
    let inner: Arc<dyn Any + Send + Sync> = facade.inner.clone();
    Arc::downgrade(&inner)
}

/// The in-parameters of `def` with `auto-parameters` expanded, made at the first call and kept on the definition.
pub(crate) fn declared_in(def: &ServiceDef, facade: Option<&EntityFacade>) -> Result<Arc<[ParamDef]>, Error> {
    let reads_entity = def
        .in_params
        .iter()
        .any(|param| param.name.is_empty() && param.auto.is_some());
    let key = facade.filter(|_| reads_entity).map(facade_key);
    {
        let cached = def.expanded_in.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cached) = cached.as_ref() {
            let same = match (&cached.facade, &key) {
                (None, None) => true,
                (Some(have), Some(want)) => Weak::ptr_eq(have, want),
                _ => false,
            };
            if same {
                return Ok(Arc::clone(&cached.params));
            }
        }
    }
    let params: Arc<[ParamDef]> = expand_params(def, &def.in_params, facade)?.into();
    *def.expanded_in.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(Expanded {
        facade: key,
        params: Arc::clone(&params),
    });
    Ok(params)
}

/// The out-parameters with `auto-parameters` expanded to the entity's fields; the declared list when it has none.
pub(crate) fn expanded_out(def: &ServiceDef) -> std::borrow::Cow<'_, [ParamDef]> {
    let has_auto = def
        .out_params
        .iter()
        .any(|param| param.name.is_empty() && param.auto.is_some());
    if has_auto {
        if let Ok(params) = expand_params(def, &def.out_params, current_facade().as_ref()) {
            return std::borrow::Cow::Owned(params);
        }
    }
    std::borrow::Cow::Borrowed(&def.out_params)
}

pub(crate) fn expand_params(
    def: &ServiceDef,
    params: &[ParamDef],
    facade: Option<&EntityFacade>,
) -> Result<Vec<ParamDef>, Error> {
    let mut out = Vec::new();
    for param in params {
        if param.name.is_empty() {
            if let Some(auto) = &param.auto {
                out.extend(expand_auto(def, auto, facade)?);
                continue;
            }
        }
        out.push(param.clone());
    }
    Ok(out)
}

fn expand_auto(def: &ServiceDef, auto: &AutoParams, facade: Option<&EntityFacade>) -> Result<Vec<ParamDef>, Error> {
    let Some(facade) = facade else {
        return Err(Error::new("no entity facade is installed"));
    };
    let entity_name = auto_entity_name(def, auto)?;
    facade
        .with_entity(&entity_name, |entity, _, model| Ok(auto_fields(auto, entity, model)))
        .map_err(Error::from)
}

/// The entity `auto-parameters` reads: its `entity-name`, else the service noun.
pub(crate) fn auto_entity_name(def: &ServiceDef, auto: &AutoParams) -> Result<String, Error> {
    let entity_name = auto
        .entity_name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| def.noun.clone());
    if entity_name.is_empty() {
        return Err(Error::new(format!(
            "service {} auto-parameters has no entity",
            def.name
        )));
    }
    Ok(entity_name)
}

/// The parameters `auto-parameters` makes from the fields of `entity`.
pub(crate) fn auto_fields(
    auto: &AutoParams,
    entity: &moqui_db::model::EntityDefinition,
    model: &moqui_db::model::EntityModel,
) -> Vec<ParamDef> {
    // a view-entity has aliases, not fields
    let fields: Vec<(&str, bool, String)> = if entity.is_view {
        entity
            .aliases
            .iter()
            .map(|alias| {
                let java = if alias.java_type.is_empty() {
                    "String".to_string()
                } else {
                    alias.java_type.clone()
                };
                (alias.name.as_str(), alias.is_pk, java)
            })
            .collect()
    } else {
        entity
            .fields
            .iter()
            .map(|field| {
                let java = model
                    .dictionary_type(&field.field_type)
                    .map(|entry| entry.java_type.clone())
                    .unwrap_or_else(|| "String".to_string());
                (field.name.as_str(), field.is_pk, java)
            })
            .collect()
    };
    let mut params = Vec::new();
    for (name, is_pk, java) in fields {
        let take = match auto.include {
            Include::Pk => is_pk,
            Include::NonPk => !is_pk,
            Include::All => true,
        };
        if !take || auto.exclude.iter().any(|excluded| excluded == name) {
            continue;
        }
        params.push(ParamDef {
            name: name.to_string(),
            java_type: java,
            required: auto.required,
            // Java's mergeAutoParameters takes no default: the entity field's `default` is an expression for a
            // create, and as a parameter default an update would write it over the stored value.
            default_value: None,
            default_expr: None,
            allow_html: auto.allow_html,
            checks: Vec::new(),
            auto: None,
            entity_name: Some(entity.full_name.clone()),
            field_name: None,
            format: None,
        });
    }
    params
}

fn check_param(param: &ParamDef, value: &Value) -> Result<(), Error> {
    for check in &param.checks {
        apply_check(&param.name, check, value)?;
    }
    Ok(())
}

/// Whether an empty string, map, or list is already an instance of the parameter's type.
fn empty_matches_type(value: &Value, java_type: &str) -> bool {
    let simple = short_type(java_type);
    match value {
        Value::Map(_) => matches!(simple, "Map" | "HashMap" | "LinkedHashMap" | "TreeMap" | "Object"),
        Value::List(_) => matches!(simple, "List" | "ArrayList" | "Collection" | "Object"),
        Value::String(_) => matches!(simple, "String" | "CharSequence" | "Object"),
        _ => true,
    }
}

fn short_type(java_type: &str) -> &str {
    java_type.rsplit('.').next().unwrap_or(java_type)
}

/// The script frame parameter defaults are evaluated in. It is pushed for the first default with the parameters so
/// far, and each parameter cleaned after that is set in it, so the parameters are copied once, not once per default.
#[derive(Default)]
struct DefaultScope(Option<crate::script::ContextFrame>);

impl DefaultScope {
    fn enter(&mut self, params: &BTreeMap<String, Value>) -> Result<(), Error> {
        if self.0.is_none() {
            let frame = crate::script::ContextFrame::push();
            for (name, value) in params {
                crate::script::set_context_name(name, value.clone())?;
            }
            self.0 = Some(frame);
        }
        Ok(())
    }

    fn set(&self, name: &str, value: &Value) -> Result<(), Error> {
        if self.0.is_some() {
            crate::script::set_context_name(name, value.clone())?;
        }
        Ok(())
    }
}

/// The default of a parameter nobody supplied: the `default` expression, or the `default-value`
/// text expanded against the parameters so far.
fn default_for(
    param: &ParamDef,
    params: &BTreeMap<String, Value>,
    scope: &mut DefaultScope,
) -> Result<Option<Value>, Error> {
    if param.default_expr.is_none() && param.default_value.is_none() {
        return Ok(None);
    }
    scope.enter(params)?;
    if let Some(expression) = &param.default_expr {
        let value = crate::script::eval_value(expression)?;
        return Ok((!value.is_null()).then_some(value));
    }
    let Some(text) = &param.default_value else {
        return Ok(None);
    };
    let expanded = crate::script::expand(text)?;
    Ok(Some(Value::string(expanded)))
}

/// `allow-html`. Text with a `<` is rejected unless the parameter allows HTML. With `safe` the
/// markup is cleaned instead: elements and attributes outside ammonia's default allow-list are
/// removed, script and style content goes with its element, and links lose unsafe URLs. Text with no
/// `<` is returned as it came, so plain values are never reformatted.
fn clean_html(param: &ParamDef, value: Value) -> Result<Value, Error> {
    if param.allow_html == AllowHtml::Any {
        return Ok(value);
    }
    match value {
        Value::String(text) => clean_text(param, text).map(Value::String),
        Value::List(items) => items
            .into_iter()
            .map(|item| match item {
                Value::String(text) => clean_text(param, text).map(Value::String),
                other => Ok(other),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::List),
        other => Ok(other),
    }
}

fn clean_text(param: &ParamDef, text: String) -> Result<String, Error> {
    if !text.contains('<') {
        return Ok(text);
    }
    if param.allow_html == AllowHtml::Safe {
        return Ok(safe_html().clean(&text).to_string());
    }
    Err(Error::new(format!(
        "parameter {}: HTML not allowed including less-than (<), greater-than (>), etc symbols",
        param.name
    )))
}

/// HTML cleaned as an `allow-html="safe"` parameter is, for a script that cleans text it takes with `any` (the wiki's
/// HTML pages, where Java called Jsoup with its relaxed list).
pub(crate) fn clean_safe_html(text: &str) -> String {
    safe_html().clean(text).to_string()
}

/// The cleaner. ammonia's defaults are a maintained, conservative allow-list, so there is no
/// list of our own to keep up to date.
fn safe_html() -> &'static ammonia::Builder<'static> {
    static CLEANER: std::sync::OnceLock<ammonia::Builder<'static>> = std::sync::OnceLock::new();
    CLEANER.get_or_init(ammonia::Builder::default)
}

fn apply_check(name: &str, check: &Check, value: &Value) -> Result<(), Error> {
    match check {
        Check::Matches { regexp, message } => {
            let text = to_plain_string(value);
            let matched = regex_full_match(&text, regexp)?;
            if !matched {
                let detail = if message.is_empty() {
                    format!("parameter {name} does not match {regexp}")
                } else {
                    format!("parameter {name} {message}")
                };
                return Err(Error::new(detail));
            }
        }
        Check::NumberRange {
            min,
            min_eq,
            max,
            max_eq,
            message,
        } => {
            if !is_decimal(value) {
                return Err(Error::new(format!("parameter {name} is not a number")));
            }
            if let Some(min) = min {
                let ordering = decimal_against(value, min)
                    .ok_or_else(|| Error::new(format!("parameter {name} min is not a number")))?;
                if ordering.is_lt() || (ordering.is_eq() && !min_eq) {
                    return Err(range_error(name, message));
                }
            }
            if let Some(max) = max {
                let ordering = decimal_against(value, max)
                    .ok_or_else(|| Error::new(format!("parameter {name} max is not a number")))?;
                if ordering.is_gt() || (ordering.is_eq() && !max_eq) {
                    return Err(range_error(name, message));
                }
            }
        }
        Check::NumberInteger => {
            if !is_whole(value) {
                return Err(Error::new(format!("parameter {name} is not an integer")));
            }
        }
        Check::NumberDecimal => {
            if !is_decimal(value) {
                return Err(Error::new(format!("parameter {name} is not a decimal")));
            }
        }
        Check::TextLength { min, max } => {
            let len = to_plain_string(value).chars().count();
            if min.is_some_and(|min| len < min) || max.is_some_and(|max| len > max) {
                return Err(Error::new(format!("parameter {name} length is out of range")));
            }
        }
        Check::TextEmail => require_match(name, value, r"^[^@\s]+@[^@\s]+\.[^@\s]+$", "is not an email address")?,
        Check::TextUrl => require_match(name, value, r"^https?://\S+$", "is not a URL")?,
        Check::TextLetters => require_match(name, value, "^[A-Za-z]+$", "is not letters")?,
        Check::TextDigits => require_match(name, value, "^[0-9]+$", "is not digits")?,
        Check::TimeRange { after, before, format } => {
            let read = match format {
                Some(pattern) if !value.is_null() => {
                    crate::l10n::parse_timestamp_format(&to_plain_string(value), Some(pattern))
                }
                _ => None,
            };
            let stamp = stamp_millis(read.as_ref().unwrap_or(value))
                .ok_or_else(|| Error::new(format!("parameter {name} is not a date")))?;
            if let Some(after) = after {
                let bound = bound_millis(after)?;
                if stamp <= bound {
                    return Err(Error::new(format!("parameter {name} is not after {after}")));
                }
            }
            if let Some(before) = before {
                let bound = bound_millis(before)?;
                if stamp >= bound {
                    return Err(Error::new(format!("parameter {name} is not before {before}")));
                }
            }
        }
        Check::CreditCard { types } => {
            let digits: String = to_plain_string(value)
                .chars()
                .filter(|ch| ch.is_ascii_digit())
                .collect();
            if !luhn(&digits) || !card_type_ok(&digits, types.as_deref()) {
                return Err(Error::new(format!("parameter {name} is not a credit card number")));
            }
        }
        Check::And(checks) => {
            for child in checks {
                apply_check(name, child, value)?;
            }
        }
        Check::Or(checks) => {
            if checks.is_empty() {
                return Err(Error::new(format!("parameter {name} failed validation")));
            }
            let mut passed = false;
            for child in checks {
                if apply_check(name, child, value).is_ok() {
                    passed = true;
                    break;
                }
            }
            if !passed {
                return Err(Error::new(format!("parameter {name} failed validation")));
            }
        }
        Check::Not(checks) => {
            for child in checks {
                if apply_check(name, child, value).is_ok() {
                    return Err(Error::new(format!("parameter {name} failed validation")));
                }
            }
        }
    }
    Ok(())
}

fn range_error(name: &str, message: &Option<String>) -> Error {
    match message {
        Some(message) if !message.is_empty() => Error::new(format!("parameter {name} {message}")),
        _ => Error::new(format!("parameter {name} is out of range")),
    }
}

fn require_match(name: &str, value: &Value, pattern: &str, detail: &str) -> Result<(), Error> {
    let text = to_plain_string(value);
    if !regex_full_match(&text, pattern)? {
        return Err(Error::new(format!("parameter {name} {detail}")));
    }
    Ok(())
}

fn is_decimal(value: &Value) -> bool {
    basic_convert(value, "BigDecimal").value.as_decimal().is_some()
}

fn is_whole(value: &Value) -> bool {
    let Some(number) = basic_convert(value, "BigDecimal").value.as_decimal() else {
        return false;
    };
    decimal_eq(number, number.trunc())
}

fn decimal_against(value: &Value, bound: &str) -> Option<std::cmp::Ordering> {
    let left = basic_convert(value, "BigDecimal").value.as_decimal()?;
    let right = basic_convert(&Value::string(bound), "BigDecimal").value.as_decimal()?;
    Some(decimal_compare(left, right))
}

fn retain_known(
    def: &ServiceDef,
    params: &mut BTreeMap<String, Value>,
    known: &BTreeSet<String>,
    facade: Option<&EntityFacade>,
) -> Result<(), Error> {
    if def.kind == ServiceKind::EntityAuto {
        let Some(facade) = facade else {
            return Err(Error::new("no entity facade is installed"));
        };
        let fields = facade
            .with_entity(&def.noun, |entity, _, _| {
                Ok(entity
                    .fields
                    .iter()
                    .map(|field| field.name.clone())
                    .collect::<BTreeSet<_>>())
            })
            .map_err(Error::from)?;
        params.retain(|name, value| {
            known.contains(name) || fields.contains(name) || matches!(value, Value::Map(_) | Value::List(_))
        });
        return Ok(());
    }
    params.retain(|name, _| known.contains(name));
    Ok(())
}

fn is_blank(value: Option<&Value>) -> bool {
    value.is_none_or(is_empty)
}

pub(crate) fn current_facade() -> Option<EntityFacade> {
    crate::context::ExecutionContext::with_current(|ctx| ctx.factory().entity()).flatten()
}

fn stamp_millis(value: &Value) -> Option<i64> {
    if value.is_null() {
        return None;
    }
    to_plain_string(value).parse().ok()
}

fn bound_millis(text: &str) -> Result<i64, Error> {
    if text == "now" {
        let millis = moqui_lib::clock::now_millis();
        return Ok(millis);
    }
    let converted = basic_convert(&Value::string(text), "Timestamp").value;
    stamp_millis(&converted).ok_or_else(|| Error::new(format!("time {text} is not a timestamp")))
}

fn luhn(digits: &str) -> bool {
    let chars: Vec<char> = digits.chars().collect();
    if chars.len() < 12 {
        return false;
    }
    let mut sum = 0;
    let mut alternate = false;
    for ch in chars.iter().rev() {
        let mut digit = ch.to_digit(10).unwrap_or(0);
        if alternate {
            digit *= 2;
            if digit > 9 {
                digit -= 9;
            }
        }
        sum += digit;
        alternate = !alternate;
    }
    sum % 10 == 0
}

fn card_type_ok(digits: &str, types: Option<&str>) -> bool {
    let Some(types) = types.filter(|text| !text.is_empty()) else {
        return true;
    };
    let kind = if digits.starts_with('4') {
        "visa"
    } else if digits.starts_with("34") || digits.starts_with("37") {
        "amex"
    } else if digits.starts_with("6011") || digits.starts_with("65") {
        "discover"
    } else if digits.starts_with("36") || digits.starts_with("38") {
        "dinersclub"
    } else {
        "mastercard"
    };
    types
        .split(',')
        .any(|allowed| allowed.trim().eq_ignore_ascii_case(kind))
}

#[cfg(test)]
mod time_range_tests {
    use super::*;

    #[test]
    fn a_time_range_reads_the_value_by_its_format() {
        let after_now = Check::TimeRange {
            after: Some("now".to_string()),
            before: None,
            format: Some("MM/yyyy".to_string()),
        };
        // a month and year far ahead pass, one long past does not, and text that does not fit the pattern is not a date
        assert!(apply_check("expireDate", &after_now, &Value::string("12/2099")).is_ok());
        let past = apply_check("expireDate", &after_now, &Value::string("01/2001")).unwrap_err();
        assert!(past.message().contains("not after now"), "{}", past.message());
        let bad = apply_check("expireDate", &after_now, &Value::string("soon")).unwrap_err();
        assert!(bad.message().contains("not a date"), "{}", bad.message());
        // without a format the value is epoch milliseconds, as before
        let plain = Check::TimeRange {
            after: Some("now".to_string()),
            before: None,
            format: None,
        };
        assert!(apply_check("when", &plain, &Value::string("4102444800000")).is_ok());
        assert!(apply_check("when", &plain, &Value::string("12/2099")).is_err());
    }
}

#[cfg(test)]
mod empty_param_tests {
    use super::*;

    fn param(name: &str, java_type: &str) -> ParamDef {
        ParamDef {
            name: name.to_string(),
            java_type: java_type.to_string(),
            required: Required::False,
            default_value: None,
            default_expr: None,
            allow_html: AllowHtml::None,
            checks: Vec::new(),
            auto: None,
            entity_name: None,
            field_name: None,
            format: None,
        }
    }

    #[test]
    fn an_empty_string_for_a_typed_parameter_is_null() {
        let mut scope = DefaultScope::default();
        let params = BTreeMap::from([
            ("unitAmount".to_string(), Value::string("")),
            ("comments".to_string(), Value::string("")),
        ]);
        // a BigDecimal gets null, as Java, so the service calculates the price rather than taking a manual one
        let amount = clean_param(&param("unitAmount", "BigDecimal"), &params, &mut scope).unwrap();
        assert!(matches!(amount, Some(Value::Null)), "{amount:?}");
        // a String keeps its empty value
        let comments = clean_param(&param("comments", "String"), &params, &mut scope).unwrap();
        assert!(comments.is_none(), "{comments:?}");
    }

    #[test]
    fn a_time_stamp_entered_as_text_is_in_the_users_zone() {
        let context = crate::context::ExecutionContextFactory::test_context();
        let _entered = context.enter();
        crate::user::session::set_time_zone(Some("US/Central"));
        let mut scope = DefaultScope::default();
        // a date-time field posts the wall time the user sees, which is 15:00 UTC in Chicago
        let params = BTreeMap::from([("packDate".to_string(), Value::string("2026-10-23 10:00"))]);
        let packed = clean_param(&param("packDate", "java.sql.Timestamp"), &params, &mut scope).unwrap();
        let Some(Value::Timestamp(stamp)) = packed else {
            panic!("{packed:?}")
        };
        assert_eq!(stamp.to_string(), "2026-10-23T15:00:00Z");
        crate::user::session::set_time_zone(None);
    }
}
