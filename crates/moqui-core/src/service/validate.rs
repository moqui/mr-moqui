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

use super::define::{merge_param, AllowHtml, AutoParams, Check, Include, ParamDef, Required, ServiceDef};
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
    let mut failed: Vec<(String, String)> = Vec::new();
    let mut scope = DefaultScope::default();
    for param in declared.iter() {
        // `required="disabled"` is, as the XSD says, as if the parameter did not exist, so validating drops it. Java
        // only makes it not required and still converts a value passed for it (accepted, P6a-12).
        if param.name.is_empty() || param.required == Required::Disabled {
            continue;
        }
        known.insert(param.name.clone());
        match clean_param(param, &params, &mut scope, def.validate_input) {
            Ok(Some(value)) => {
                let value = nested(param, value, &param.name, &mut failed, def.validate_input);
                scope.set(&param.name, &value)?;
                params.insert(param.name.clone(), value);
            }
            Ok(None) => {}
            Err(messages) => failed.extend(messages.into_iter().map(|message| (param.name.clone(), message))),
        }
    }
    if !failed.is_empty() {
        let joined = failed
            .iter()
            .map(|(field, message)| format!("{message} (for field {field})"))
            .collect::<Vec<_>>()
            .join("; ");
        crate::context::ExecutionContext::with_current(|ec| {
            let mut messages = ec.message();
            for (field, message) in &failed {
                messages.add_validation_error("", field, &def.name, message);
            }
        });
        return Err(Error::of_kind(crate::context::ErrorKind::Validation, joined));
    }
    if def.validate_input {
        params.retain(|name, _| known.contains(name));
    }
    Ok(params)
}

/// Java's `nestedParameterClean` for a `Map` parameter with nested `<parameter>` elements: each entry is cleaned and
/// checked as a parameter, named `parent.child` in an error. Validating, only the declared entries are kept.
fn nested(param: &ParamDef, value: Value, path: &str, failed: &mut Vec<(String, String)>, validate: bool) -> Value {
    let Value::Map(input) = value else {
        return value;
    };
    if param.children.is_empty() {
        return Value::Map(input);
    }
    let source: BTreeMap<String, Value> = input
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let mut out = if validate { Default::default() } else { input.clone() };
    let mut scope = DefaultScope::default();
    for child in &param.children {
        if child.name.is_empty() || child.required == Required::Disabled {
            continue;
        }
        let child_path = format!("{path}.{}", child.name);
        match clean_param(child, &source, &mut scope, validate) {
            Ok(Some(value)) => {
                let value = nested(child, value, &child_path, failed, validate);
                out.insert(child.name.clone(), value);
            }
            Ok(None) => {
                if let Some(value) = input.get(&child.name) {
                    out.insert(child.name.clone(), value.clone());
                }
            }
            Err(messages) => failed.extend(messages.into_iter().map(|message| (child_path.clone(), message))),
        }
    }
    Value::Map(out)
}

/// One parameter: its default when blank, the required check, conversion to its type, HTML cleaning, and the
/// declared checks. The value to store, if any, or Java's message for each thing that failed: a missing required
/// value or a failed conversion stops there, and each check that fails is a message of its own. As Java,
/// `validate="false"` skips the required, HTML and declared checks but still converts.
fn clean_param(
    param: &ParamDef,
    params: &BTreeMap<String, Value>,
    scope: &mut DefaultScope,
    validate: bool,
) -> Result<Option<Value>, Vec<String>> {
    let mut defaulted = None;
    if is_blank(params.get(&param.name)) {
        defaulted = default_for(param, params, scope).map_err(|err| vec![err.message().to_string()])?;
    }
    let current = defaulted.clone().or_else(|| params.get(&param.name).cloned());
    if validate && param.required == Required::True && is_blank(current.as_ref()) {
        return Err(vec![localized("Field cannot be empty")]);
    }
    let Some(value) = current else {
        return Ok(None);
    };
    if is_empty(&value) {
        return Ok(defaulted);
    }
    // Convert first, so the checks see the type they were written for.
    let value = if matches!(value, Value::Map(_) | Value::List(_)) {
        value
    } else if let Some(items) = text_collection(&value, &param.java_type) {
        items
    } else if let Some(converted) = convert_text(param, &value) {
        converted
    } else if param.format.is_some() && matches!(value, Value::String(_)) && is_number_type(&param.java_type) {
        // Java parses a number by its format and adds this error when it does not read
        return Err(vec![not_converted(param, &value)]);
    } else {
        crate::l10n::convert_checked(&value, &param.java_type).ok_or_else(|| vec![not_converted(param, &value)])?
    };
    if !validate {
        return Ok(Some(value));
    }
    let mut failures = Vec::new();
    let value = clean_html(param, value, &mut failures);
    for check in &param.checks {
        failures.extend(apply_check(check, &value));
    }
    if failures.is_empty() {
        Ok(Some(value))
    } else {
        Err(failures)
    }
}

/// Java's message for a value that does not convert to the parameter's type, naming the format when there is one.
fn not_converted(param: &ParamDef, value: &Value) -> String {
    let with_format = param.format.is_some();
    localized("Value entered ({0}) could not be converted to a {1}{2,choice,0#|1# using format [}{3}{2,choice,0#|1#]}")
        .replace("{0}", to_plain_string(value).trim())
        .replace("{1}", &param.java_type)
        .replace(
            "{2,choice,0#|1# using format [}",
            if with_format { " using format [" } else { "" },
        )
        .replace("{2,choice,0#|1#]}", if with_format { "]" } else { "" })
        .replace("{3}", param.format.as_deref().unwrap_or(""))
}

/// `text` in the session locale, or as it is when there is no localized message for it.
fn localized(text: &str) -> String {
    crate::l10n::localize(text).unwrap_or_else(|_| text.to_string())
}

/// Java's text for a failed check: `template` localized, then each `${name}` in it replaced by its value.
fn java_text(template: &str, values: &[(&str, &str)]) -> Vec<String> {
    let mut text = localized(template);
    for (name, value) in values {
        text = text.replace(&format!("${{{name}}}"), value);
    }
    vec![text]
}

/// A check's own `message`, localized and expanded with `values` in the context, as Java's `ec.resource.expand`.
fn expand_message(message: &str, values: &[(&str, Value)]) -> Vec<String> {
    let _frame = crate::script::ContextFrame::push();
    for (name, value) in values {
        let _ = crate::script::set_context_name(name, value.clone());
    }
    vec![crate::script::expand_localized(message).unwrap_or_else(|_| message.to_string())]
}

/// Java's message for a check that could not be done, an exception there.
fn failed_validation(check: &str, reason: &str) -> Vec<String> {
    java_text(
        "Value entered failed ${valNode.name} validation: ${t.message}",
        &[("valNode.name", check), ("t.message", reason)],
    )
}

fn is_number_type(java_type: &str) -> bool {
    matches!(
        short_type(java_type),
        "Integer" | "int" | "Long" | "long" | "Float" | "float" | "Double" | "double" | "BigDecimal" | "BigInteger"
    )
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
                // As Java's `mergeAutoParameters`, a field of the same name as a parameter so far sets its type,
                // `required`, `allow-html` and entity field, and a later parameter of that name merges into it.
                for field in expand_auto(def, auto, facade)? {
                    merge_param(&mut out, field);
                }
                continue;
            }
        }
        merge_param(&mut out, param.clone());
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
            field_name: Some(name.to_string()),
            format: None,
            // The entity model keeps no field `<description>`, and Java's mergeAutoParameters copies none.
            description: None,
            children: Vec::new(),
            given: BTreeSet::from(["type", "required", "allow-html", "entity-name", "field-name"]),
        });
    }
    params
}

/// Java's `ParameterInfo.convertType` for text: a number by the parameter's `format`, else as the user's locale writes
/// it, a date or time by its `format` then the usual forms, and a timestamp by its `format` then the usual forms, in the
/// session time zone unless the text has an offset (`ec.l10n.parseTimestamp`: the user's zone, else the server's), so a
/// date picker's `yyyy-MM-dd` is the user's midnight and a time a form shows in the user's zone reads back the same.
/// `None` leaves the value to the plain conversion.
fn convert_text(param: &ParamDef, value: &Value) -> Option<Value> {
    let Value::String(text) = value else {
        return None;
    };
    let text = text.trim();
    let format = param.format.as_deref();
    let parsed = match short_type(&param.java_type) {
        "Integer" | "int" | "Long" | "long" | "Float" | "float" | "Double" | "double" | "BigDecimal" | "BigInteger" => {
            crate::l10n::parse_number_format(text, format)
        }
        "Timestamp" => crate::l10n::parse_timestamp_format(text, format),
        "Date" => crate::l10n::parse_date_format(text, format),
        "Time" => crate::l10n::parse_time_format(text, format),
        _ => None,
    }?;
    convert_checked(&parsed, &param.java_type)
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
    // Text with nothing to expand (as `expand` decides) needs no scope of the parameters.
    if let (None, Some(text)) = (&param.default_expr, &param.default_value) {
        if !text.contains('$') || text.ends_with('$') {
            return Ok(Some(Value::string(text.as_str())));
        }
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

/// `allow-html`. Text with a `<` is an error unless the parameter allows HTML. With `safe` the
/// markup is cleaned instead: elements and attributes outside the allow-list of [`safe_html`] are
/// removed, script and style content goes with its element, and links lose unsafe URLs. Text with no
/// `<` is returned as it came, so plain values are never reformatted.
fn clean_html(param: &ParamDef, value: Value, failures: &mut Vec<String>) -> Value {
    if param.allow_html == AllowHtml::Any {
        return value;
    }
    match value {
        Value::String(text) => Value::String(clean_text(param, text, failures)),
        Value::List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(match item {
                    Value::String(text) => Value::String(clean_text(param, text, failures)),
                    other => other,
                });
            }
            Value::List(out)
        }
        other => other,
    }
}

fn clean_text(param: &ParamDef, text: String, failures: &mut Vec<String>) -> String {
    if !text.contains('<') {
        return text;
    }
    if param.allow_html == AllowHtml::Safe {
        return safe_html().clean(&text).to_string();
    }
    failures.push(localized(
        "HTML not allowed including less-than (<), greater-than (>), etc symbols",
    ));
    text
}

/// HTML cleaned as an `allow-html="safe"` parameter is, for a script that cleans text it takes with `any` (the wiki's
/// HTML pages, where Java called Jsoup with its relaxed list).
pub(crate) fn clean_safe_html(text: &str) -> String {
    safe_html().clean(text).to_string()
}

/// The cleaner. ammonia's defaults are a maintained, conservative allow-list, so there is no
/// list of our own to keep up to date, with three additions on a `span`: `data-mention-id` (a person,
/// `<span data-mention-id="john.doe">@John Doe</span>`), and `data-record-entity` with `data-record-id`
/// (a record, `<span data-record-entity="mantle.order.OrderHeader" data-record-id="100050">#SO100050</span>`).
/// A record span is not a mention. The values are only ever text: ammonia escapes them. Tables (`table`,
/// `thead`, `tbody`, `tr`, `th`, `td`, with `colspan` and `rowspan`) are in ammonia's defaults.
fn safe_html() -> &'static ammonia::Builder<'static> {
    static CLEANER: std::sync::OnceLock<ammonia::Builder<'static>> = std::sync::OnceLock::new();
    CLEANER.get_or_init(|| {
        let mut builder = ammonia::Builder::default();
        builder.add_tag_attributes("span", &["data-mention-id", "data-record-entity", "data-record-id"]);
        builder
    })
}

/// The messages of a check that `value` fails, Java's text for each; none when it passes.
fn apply_check(check: &Check, value: &Value) -> Vec<String> {
    let pv = to_plain_string(value);
    match check {
        Check::Matches { regexp, message } => {
            if regexp.is_empty() {
                return Vec::new();
            }
            match regex_full_match(&pv, regexp) {
                Ok(true) => Vec::new(),
                Ok(false) if !message.is_empty() => {
                    expand_message(message, &[("pv", value.clone()), ("regexp", Value::string(regexp))])
                }
                Ok(false) => java_text(
                    "Value entered (${pv}) did not match expression: ${regexp}",
                    &[("pv", &pv), ("regexp", regexp)],
                ),
                Err(err) => failed_validation("matches", err.message()),
            }
        }
        Check::NumberRange {
            min,
            min_eq,
            max,
            max_eq,
            message,
        } => {
            let range = |template: &str, bound_name: &str, bound: &str| match message
                .as_deref()
                .filter(|text| !text.is_empty())
            {
                Some(message) => expand_message(message, &[("pv", value.clone()), (bound_name, Value::string(bound))]),
                None => java_text(template, &[("pv", &pv), (bound_name, bound)]),
            };
            if !is_decimal(value) {
                return failed_validation("number-range", &format!("{pv} is not a number"));
            }
            if let Some(min) = min.as_deref().filter(|text| !text.is_empty()) {
                let Some(ordering) = decimal_against(value, min) else {
                    return failed_validation("number-range", &format!("min {min} is not a number"));
                };
                if !min_eq && ordering.is_le() {
                    return range(
                        "Value entered (${pv}) is less than or equal to ${min}, must be greater than.",
                        "min",
                        min,
                    );
                }
                if *min_eq && ordering.is_lt() {
                    return range(
                        "Value entered (${pv}) is less than ${min} and must be greater than or equal to.",
                        "min",
                        min,
                    );
                }
            }
            if let Some(max) = max.as_deref().filter(|text| !text.is_empty()) {
                let Some(ordering) = decimal_against(value, max) else {
                    return failed_validation("number-range", &format!("max {max} is not a number"));
                };
                if *max_eq && ordering.is_gt() {
                    return range(
                        "Value entered (${pv}) is greater than ${max} and must be less than or equal to.",
                        "max",
                        max,
                    );
                }
                if !max_eq && ordering.is_ge() {
                    return range(
                        "Value entered (${pv}) is greater than or equal to ${max} and must be less than.",
                        "max",
                        max,
                    );
                }
            }
            Vec::new()
        }
        Check::NumberInteger if !is_whole(value) => {
            java_text("Value [${pv}] is not a whole (integer) number.", &[("pv", &pv)])
        }
        Check::NumberDecimal if !is_decimal(value) => {
            java_text("Value [${pv}] is not a decimal number.", &[("pv", &pv)])
        }
        Check::TextLength { min, max } => {
            let len = pv.chars().count();
            let length = len.to_string();
            if let Some(min) = min.filter(|min| len < *min) {
                return java_text(
                    "Value entered (${pv}), length ${str.length()}, is shorter than ${minStr} characters.",
                    &[("pv", &pv), ("str.length()", &length), ("minStr", &min.to_string())],
                );
            }
            if let Some(max) = max.filter(|max| len > *max) {
                return java_text(
                    "Value entered (${pv}), length ${str.length()}, is longer than ${maxStr} characters.",
                    &[("pv", &pv), ("str.length()", &length), ("maxStr", &max.to_string())],
                );
            }
            Vec::new()
        }
        Check::TextEmail if !is_email(&pv) => {
            java_text("Value entered (${str}) is not a valid email address.", &[("str", &pv)])
        }
        // Java's `UrlValidator(ALLOW_ALL_SCHEMES)`: any scheme, then an authority (a `file:` URL may leave it empty).
        Check::TextUrl
            if !regex_full_match(&pv, r"^(?:[A-Za-z][A-Za-z0-9+.\-]*://[^\s/?#]+|file://)[^\s]*$").unwrap_or(false) =>
        {
            java_text("Value entered (${str}) is not a valid URL.", &[("str", &pv)])
        }
        // Java's `Character.isLetter` and `isDigit`, which take a UTF-16 unit, so only the Basic Multilingual Plane.
        // Alphabetic is a little more than Java's letters: combining vowel signs and letter numbers (accepted, P6a-20).
        Check::TextLetters if !pv.chars().all(|ch| ch.is_alphabetic() && u32::from(ch) <= 0xFFFF) => {
            java_text("Value entered (${str}) must have only letters.", &[("str", &pv)])
        }
        Check::TextDigits if !pv.chars().all(is_java_digit) => {
            java_text("Value [${str}] must have only digits.", &[("str", &pv)])
        }
        Check::TimeRange { after, before, format } => {
            time_range(value, &pv, after.as_deref(), before.as_deref(), format.as_deref())
        }
        Check::CreditCard { types } => {
            let digits: String = pv.chars().filter(|ch| ch.is_ascii_digit()).collect();
            if luhn(&digits) && card_type_ok(&digits, types.as_deref()) {
                Vec::new()
            } else {
                java_text("Value entered is not a valid credit card number.", &[])
            }
        }
        Check::And(checks) => checks.iter().flat_map(|child| apply_check(child, value)).collect(),
        // As logic, not as Java, which records the failure of each child even when another passes (accepted, P6a-19):
        // when none passes each says why, and an empty `val-or` passes, as Java adds no error for it.
        Check::Or(checks) => {
            let mut reasons = Vec::new();
            for child in checks {
                let failed = apply_check(child, value);
                if failed.is_empty() {
                    return Vec::new();
                }
                reasons.extend(failed);
            }
            reasons
        }
        Check::Not(checks) => {
            if checks.iter().any(|child| apply_check(child, value).is_empty()) {
                java_text("Value entered (${pv}) failed val-not validation.", &[("pv", &pv)])
            } else {
                Vec::new()
            }
        }
        Check::NumberInteger
        | Check::NumberDecimal
        | Check::TextEmail
        | Check::TextUrl
        | Check::TextLetters
        | Check::TextDigits => Vec::new(),
    }
}

/// Java's `time-range`: text is read by the `format` (a value that does not read is not checked), and the bounds,
/// with `now` as the current time written and read again by the format, are inclusive.
fn time_range(value: &Value, pv: &str, after: Option<&str>, before: Option<&str>, format: Option<&str>) -> Vec<String> {
    let format = format.filter(|pattern| !pattern.is_empty());
    let stamp = match value {
        Value::String(text) => match crate::l10n::parse_timestamp_format(text, format)
            .as_ref()
            .and_then(stamp_millis)
        {
            Some(stamp) => stamp,
            None => return Vec::new(),
        },
        other => match stamp_millis(other) {
            Some(stamp) => stamp,
            None => return failed_validation("time-range", &format!("{pv} is not a date")),
        },
    };
    if let Some(after) = after.filter(|text| !text.is_empty()) {
        match bound_millis(after, format) {
            Some(bound) if stamp < bound => {
                return java_text(
                    "Value entered (${pv}) is before ${after}.",
                    &[("pv", pv), ("after", after)],
                );
            }
            Some(_) => {}
            None => return failed_validation("time-range", &format!("{after} is not a date")),
        }
    }
    if let Some(before) = before.filter(|text| !text.is_empty()) {
        match bound_millis(before, format) {
            Some(bound) if stamp > bound => {
                return java_text(
                    "Value entered (${pv}) is after ${before}.",
                    &[("pv", pv), ("before", before)],
                );
            }
            Some(_) => {}
            None => return failed_validation("time-range", &format!("{before} is not a date")),
        }
    }
    Vec::new()
}

/// Java's `Character.isDigit` for a UTF-16 unit: a decimal digit (general category Nd) of the Basic Multilingual
/// Plane. Each such run of ten starts at one of these.
fn is_java_digit(ch: char) -> bool {
    const ZEROS: [u32; 37] = [
        0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66, 0xDE6, 0xE50, 0xED0,
        0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90, 0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620,
        0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
    ];
    let code = u32::from(ch);
    ZEROS.iter().any(|zero| (*zero..zero + 10).contains(&code))
}

/// Java's `EmailValidator` (commons-validator): a user part of dot-separated words, each plain characters, escaped
/// ones or a quoted string, at most 64 long, then a bracketed IP address or a host name of letter, digit and hyphen
/// labels whose last is a top-level domain. Java also looks that domain up in the IANA list and turns a Unicode
/// host into its ASCII form; here the last label must only be letters or hyphens and digits after a letter, two or
/// more long, and a Unicode host is not valid (accepted, P6a-20).
fn is_email(text: &str) -> bool {
    if text.ends_with('.') {
        return false;
    }
    let Some((user, domain)) = text.rsplit_once('@') else {
        return false;
    };
    if user.is_empty()
        || user.encode_utf16().count() > 64
        || domain.is_empty()
        || domain.chars().any(char::is_whitespace)
    {
        return false;
    }
    if !is_email_user(user) {
        return false;
    }
    if let Some(address) = domain.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
        return address.parse::<std::net::IpAddr>().is_ok();
    }
    is_host_name(domain)
}

/// The user part of an address: `\s*WORD(\.WORD)*`, a word being characters other than space, control and
/// `()<>@,;:'"\.[]`, a backslash and any character, an apostrophe, or a quoted string.
fn is_email_user(user: &str) -> bool {
    let chars: Vec<char> = user.trim_start().chars().collect();
    let mut index = 0;
    loop {
        if chars.get(index) == Some(&'"') {
            index += 1;
            loop {
                match chars.get(index) {
                    None => return false,
                    Some('"') => break,
                    Some('\\') if chars.get(index + 1) == Some(&'"') => index += 2,
                    Some(_) => index += 1,
                }
            }
            index += 1;
        } else {
            let start = index;
            while let Some(&ch) = chars.get(index) {
                if ch == '\\' && index + 1 < chars.len() {
                    index += 2;
                } else if ch == '\'' || !(ch.is_whitespace() || ch.is_control() || "()<>@,;:'\"\\.[]".contains(ch)) {
                    index += 1;
                } else {
                    break;
                }
            }
            if index == start {
                return false;
            }
        }
        match chars.get(index) {
            None => return true,
            Some('.') => index += 1,
            Some(_) => return false,
        }
    }
}

/// `DomainValidator`'s host name: ASCII letter and digit labels with inner hyphens, up to 63 long and 253 in all, two
/// or more, the last starting with a letter.
fn is_host_name(domain: &str) -> bool {
    let domain = domain.strip_suffix('.').unwrap_or(domain);
    let labels: Vec<&str> = domain.split('.').collect();
    if domain.len() > 253 || labels.len() < 2 {
        return false;
    }
    let label_ok = |label: &str| {
        let bytes = label.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 63
            && bytes.iter().all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
            && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
    };
    let tld = labels.last().copied().unwrap_or_default();
    labels.iter().all(|label| label_ok(label)) && tld.len() >= 2 && tld.as_bytes()[0].is_ascii_alphabetic()
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

fn is_blank(value: Option<&Value>) -> bool {
    value.is_none_or(is_empty)
}

pub(crate) fn current_facade() -> Option<EntityFacade> {
    crate::context::ExecutionContext::with_current(|ctx| ctx.factory().entity()).flatten()
}

/// A date, time or timestamp value, or text Java's conversion reads as one, as epoch millis.
fn stamp_millis(value: &Value) -> Option<i64> {
    match crate::l10n::basic_convert(value, "Timestamp").value {
        Value::Timestamp(stamp) => Some(stamp.as_millisecond()),
        _ => None,
    }
}

/// A `time-range` bound as epoch millis: `now` is the effective time, written by `format` and read back so it has the
/// format's precision, as Java does; other text is read by the format, then the usual forms.
fn bound_millis(text: &str, format: Option<&str>) -> Option<i64> {
    if text != "now" {
        return crate::l10n::parse_timestamp_format(text, format)
            .as_ref()
            .and_then(stamp_millis);
    }
    let now = crate::user::now_effective_millis();
    let Some(pattern) = format else {
        return Some(now);
    };
    let stamp = jiff::Timestamp::from_millisecond(now).ok()?;
    let written = moqui_lib::format::format_timestamp(stamp, Some(pattern), &crate::l10n::time_zone()).ok()?;
    crate::l10n::parse_timestamp_format(&written, Some(pattern))
        .as_ref()
        .and_then(stamp_millis)
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

    /// Java's `time-range`: the value read by the `format`, `now` at the format's precision, inclusive bounds, and a
    /// value that does not read is not checked.
    #[test]
    fn a_time_range_reads_the_value_by_its_format() {
        let after_now = Check::TimeRange {
            after: Some("now".to_string()),
            before: None,
            format: Some("MM/yyyy".to_string()),
        };
        // a month and year far ahead pass, one long past does not, and text that does not fit the pattern is skipped
        assert!(apply_check(&after_now, &Value::string("12/2099")).is_empty());
        let past = apply_check(&after_now, &Value::string("01/2001"));
        assert_eq!(past, ["Value entered (01/2001) is before now."]);
        assert!(apply_check(&after_now, &Value::string("soon")).is_empty());
        // this month is not before now read as a month
        let this_month =
            moqui_lib::format::format_timestamp(jiff::Timestamp::now(), Some("MM/yyyy"), &crate::l10n::time_zone())
                .unwrap();
        assert!(apply_check(&after_now, &Value::string(&this_month)).is_empty());
        // the bounds are inclusive
        let between = Check::TimeRange {
            after: Some("2024-01-01".to_string()),
            before: Some("2024-12-31".to_string()),
            format: Some("yyyy-MM-dd".to_string()),
        };
        assert!(apply_check(&between, &Value::string("2024-01-01")).is_empty());
        assert!(apply_check(&between, &Value::string("2024-12-31")).is_empty());
        assert_eq!(
            apply_check(&between, &Value::string("2025-01-01")),
            ["Value entered (2025-01-01) is after 2024-12-31."]
        );
        // without a format the usual forms read, epoch milliseconds among them
        let plain = Check::TimeRange {
            after: Some("now".to_string()),
            before: None,
            format: None,
        };
        assert!(apply_check(&plain, &Value::string("4102444800000")).is_empty());
        assert!(!apply_check(&plain, &Value::string("2001-01-01")).is_empty());
    }
}

#[cfg(test)]
mod parameter_tests {
    use super::*;

    fn param(name: &str, java_type: &str, required: Required) -> ParamDef {
        ParamDef {
            name: name.to_string(),
            java_type: java_type.to_string(),
            required,
            default_value: None,
            default_expr: None,
            allow_html: AllowHtml::None,
            checks: Vec::new(),
            auto: None,
            entity_name: None,
            field_name: None,
            format: None,
            description: None,
            children: Vec::new(),
            given: BTreeSet::new(),
        }
    }

    /// Java's `nestedParameterClean`: a nested parameter is converted and checked, named `parent.child` in an error,
    /// and, validating, an undeclared entry is dropped.
    #[test]
    fn nested_parameters_are_cleaned_and_checked() {
        let mut parent = param("address", "Map", Required::False);
        parent.children = vec![
            param("zip", "Integer", Required::True),
            param("city", "String", Required::False),
        ];
        let input = Value::Map(
            [
                ("zip".to_string(), Value::string("12345")),
                ("city".to_string(), Value::string("Town")),
                ("extra".to_string(), Value::string("x")),
            ]
            .into_iter()
            .collect(),
        );
        let mut failed = Vec::new();
        let Value::Map(out) = nested(&parent, input.clone(), "address", &mut failed, true) else {
            panic!("a map");
        };
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(out.get("zip"), Some(&Value::I32(12345)));
        assert!(!out.contains_key("extra"));
        let Value::Map(kept) = nested(&parent, input, "address", &mut failed, false) else {
            panic!("a map");
        };
        assert!(kept.contains_key("extra"));
        let missing = Value::Map([("city".to_string(), Value::string("Town"))].into_iter().collect());
        nested(&parent, missing, "address", &mut failed, true);
        assert_eq!(
            failed,
            [("address.zip".to_string(), "Field cannot be empty".to_string())]
        );
    }

    /// `val-or` passes when one check passes and otherwise has each check's message; `text-url` takes any scheme.
    #[test]
    fn val_or_keeps_the_child_messages_and_a_url_takes_any_scheme() {
        let either = Check::Or(vec![Check::TextDigits, Check::TextLetters]);
        assert!(apply_check(&either, &Value::string("abc")).is_empty());
        assert_eq!(
            apply_check(&either, &Value::string("a1!")),
            [
                "Value [a1!] must have only digits.",
                "Value entered (a1!) must have only letters."
            ]
        );
        assert!(apply_check(&Check::Or(Vec::new()), &Value::string("x")).is_empty());
        for url in [
            "https://moqui.org/x",
            "ftp://host/file",
            "s3://bucket/key",
            "file:///tmp/x",
        ] {
            assert!(apply_check(&Check::TextUrl, &Value::string(url)).is_empty(), "{url}");
        }
        for url in ["moqui.org", "https://", "not a url"] {
            assert!(!apply_check(&Check::TextUrl, &Value::string(url)).is_empty(), "{url}");
        }
    }

    /// Java's `convertType`: a timestamp's text, by the parameter's `format` or not, is read in the session time zone
    /// unless it has an offset, so a date picker's "due today" is the user's midnight, not UTC's (which is the evening
    /// before in America/Chicago).
    #[test]
    fn a_timestamp_is_read_in_the_session_time_zone() {
        let context = crate::context::ExecutionContextFactory::test_context();
        let _entered = context.enter();
        crate::user::session::set_time_zone(Some("America/Chicago"));
        let stamp = |text: &str| Value::Timestamp(text.parse::<jiff::Timestamp>().unwrap());
        let mut due = param("dueDate", "java.sql.Timestamp", Required::False);
        due.format = Some("yyyy-MM-dd".to_string());
        assert_eq!(
            convert_text(&due, &Value::string("2026-10-19")),
            Some(stamp("2026-10-19T05:00:00Z"))
        );
        // text the format does not read takes the usual forms, in the same zone
        assert_eq!(
            convert_text(&due, &Value::string("2026-10-19 08:30")),
            Some(stamp("2026-10-19T13:30:00Z"))
        );
        let plain = param("fromDate", "Timestamp", Required::False);
        assert_eq!(
            convert_text(&plain, &Value::string("2026-01-05 00:00:00.000")),
            Some(stamp("2026-01-05T06:00:00Z")),
            "standard time"
        );
        assert_eq!(
            convert_text(&plain, &Value::string("2026-10-19")),
            Some(stamp("2026-10-19T05:00:00Z"))
        );
        // an offset and epoch millis are what they say
        assert_eq!(
            convert_text(&plain, &Value::string("2026-10-19T10:00:00Z")),
            Some(stamp("2026-10-19T10:00:00Z"))
        );
        assert_eq!(
            convert_text(&plain, &Value::string("1791979200000")),
            Some(stamp("2026-10-14T12:00:00Z"))
        );
        crate::user::session::set_time_zone(Some("UTC"));
        assert_eq!(
            convert_text(&due, &Value::string("2026-10-19")),
            Some(stamp("2026-10-19T00:00:00Z"))
        );
    }

    /// Java's `convertType`: a date by the parameter's `format`, a number as the locale writes it or by its `format`.
    #[test]
    fn text_converts_by_the_parameter_format_and_the_locale() {
        let mut date = param("when", "java.sql.Date", Required::False);
        date.format = Some("dd.MM.yyyy".to_string());
        assert_eq!(
            convert_text(&date, &Value::string("15.01.2024")),
            Some(Value::Date(jiff::civil::date(2024, 1, 15)))
        );
        let mut amount = param("amount", "BigDecimal", Required::False);
        assert_eq!(
            convert_text(&amount, &Value::string("1,234.50")),
            Some(Value::Decimal("1234.50".parse().unwrap()))
        );
        amount.format = Some("0.00%".to_string());
        assert_eq!(
            convert_text(&amount, &Value::string("12.50%")),
            Some(Value::Decimal("0.1250".parse().unwrap()))
        );
        // text the format does not read is Java's error, which names the format
        let mut scope = DefaultScope::default();
        let params = BTreeMap::from([("amount".to_string(), Value::string("12.50"))]);
        assert_eq!(
            clean_param(&amount, &params, &mut scope, true),
            Err(vec![
                "Value entered (12.50) could not be converted to a BigDecimal using format [0.00%]".to_string()
            ])
        );
        let plain = param("count", "Integer", Required::False);
        let params = BTreeMap::from([("count".to_string(), Value::string("many"))]);
        assert_eq!(
            clean_param(&plain, &params, &mut scope, true),
            Err(vec![
                "Value entered (many) could not be converted to a Integer".to_string()
            ])
        );
    }

    /// Java's `validate="false"` skips the required, HTML and declared checks, and still converts.
    #[test]
    fn validate_false_converts_but_does_not_check() {
        let mut code = param("code", "Integer", Required::True);
        code.checks = vec![Check::NumberRange {
            min: Some("10".to_string()),
            min_eq: true,
            max: None,
            max_eq: false,
            message: None,
        }];
        let mut scope = DefaultScope::default();
        let empty = BTreeMap::new();
        assert_eq!(
            clean_param(&code, &empty, &mut scope, true),
            Err(vec!["Field cannot be empty".to_string()])
        );
        assert_eq!(clean_param(&code, &empty, &mut scope, false), Ok(None));
        let low = BTreeMap::from([("code".to_string(), Value::string("5"))]);
        assert_eq!(
            clean_param(&code, &low, &mut scope, true),
            Err(vec![
                "Value entered (5) is less than 10 and must be greater than or equal to.".to_string()
            ])
        );
        assert_eq!(clean_param(&code, &low, &mut scope, false), Ok(Some(Value::I32(5))));
        let html = param("note", "String", Required::False);
        let markup = BTreeMap::from([("note".to_string(), Value::string("<b>x</b>"))]);
        assert!(clean_param(&html, &markup, &mut scope, true).is_err());
        assert_eq!(
            clean_param(&html, &markup, &mut scope, false),
            Ok(Some(Value::string("<b>x</b>")))
        );
    }

    /// As Java, each check of a parameter that fails is a message of its own, and an HTML error does not stop them.
    #[test]
    fn every_failed_check_has_its_message() {
        let mut code = param("code", "String", Required::False);
        code.checks = vec![
            Check::TextLength {
                min: Some(5),
                max: None,
            },
            Check::TextDigits,
            Check::Matches {
                regexp: "[a-z<>]+".to_string(),
                message: "Use ${regexp} for ${pv}".to_string(),
            },
        ];
        let mut scope = DefaultScope::default();
        let params = BTreeMap::from([("code".to_string(), Value::string("A<"))]);
        assert_eq!(
            clean_param(&code, &params, &mut scope, true),
            Err(vec![
                "HTML not allowed including less-than (<), greater-than (>), etc symbols".to_string(),
                "Value entered (A<), length 2, is shorter than 5 characters.".to_string(),
                "Value [A<] must have only digits.".to_string(),
                "Use [a-z<>]+ for A<".to_string(),
            ])
        );
    }

    /// Java's `EmailValidator`, `Character.isLetter` and `Character.isDigit`.
    #[test]
    fn email_letters_and_digits_follow_java() {
        for email in [
            "user@example.com",
            "first.last+tag@sub.example.org",
            "o'brien@example.ie",
            "\"quoted user\"@example.com",
            "user@[192.168.0.1]",
        ] {
            assert!(is_email(email), "{email}");
        }
        for email in [
            "user@localhost",
            "user@example.c",
            "user@-example.com",
            "user.@example.com",
            ".user@example.com",
            "us er@example.com",
            "user@example.com.",
            "user@exam_ple.com",
            "@example.com",
            "user@",
        ] {
            assert!(!is_email(email), "{email}");
        }
        assert!(apply_check(&Check::TextLetters, &Value::string("Ærøskøbing")).is_empty());
        assert!(!apply_check(&Check::TextLetters, &Value::string("abc1")).is_empty());
        // Arabic-Indic and fullwidth digits are digits, a superscript two is not
        assert!(apply_check(&Check::TextDigits, &Value::string("٣٤５")).is_empty());
        assert!(!apply_check(&Check::TextDigits, &Value::string("1²")).is_empty());
    }
}

#[cfg(test)]
mod safe_html_tests {
    use super::*;

    /// What the qvt rich-text editor writes (`m-rich-text`, js/README.md) survives `allow-html="safe"`: its blocks, marks,
    /// links, images and the mention span with its id; a link gains ammonia's `rel`, and what the editor never writes goes.
    #[test]
    fn the_rich_text_editor_output_survives_the_safe_cleaning() {
        let editor = concat!(
            "<h2>Heading</h2><p><strong>Bold</strong> <em>italic</em> <s>struck</s> <u>under</u> <code>code</code> ",
            "<a href=\"https://example.com/x\">a link</a> <span data-mention-id=\"john.doe\">@John Doe</span></p>",
            "<ul><li><p>one</p></li></ul><ol start=\"3\"><li><p>three</p></li></ol>",
            "<blockquote><p>quoted</p></blockquote><pre><code>let x = 1;</code></pre>",
            "<p>line<br>break</p><hr><img src=\"https://example.com/a.png\" alt=\"a picture\">"
        );
        let cleaned = clean_safe_html(editor);
        assert_eq!(
            cleaned,
            editor.replace(
                "<a href=\"https://example.com/x\">",
                "<a href=\"https://example.com/x\" rel=\"noopener noreferrer\">"
            )
        );
        // cleaned again it is the same: what the server keeps the editor reads back unchanged
        assert_eq!(clean_safe_html(&cleaned), cleaned);
        // only the mention id is kept on a span, a value is escaped, and script, handlers and javascript: links go
        assert_eq!(
            clean_safe_html(
                "<p><span data-mention-id=\"a&quot;b\" data-other=\"x\" class=\"mention\" onclick=\"steal()\">@A</span>\
                 <a href=\"javascript:steal()\">x</a><script>steal()</script></p>"
            ),
            "<p><span data-mention-id=\"a&quot;b\">@A</span><a rel=\"noopener noreferrer\">x</a></p>"
        );
        // a table keeps colspan; a handler on a cell goes. Attribute order is ammonia's.
        let table = clean_safe_html(
            "<table><thead><tr><th colspan=\"2\">Item</th></tr></thead><tbody><tr>\
             <td colspan=\"2\" onclick=\"steal()\">Bolt</td></tr></tbody></table>",
        );
        assert!(table.contains("<table>"), "{table}");
        assert!(table.contains("colspan=\"2\""), "{table}");
        assert!(table.contains(">Bolt<") || table.contains(">Bolt</"), "{table}");
        assert!(!table.contains("onclick"), "{table}");
        // a record span keeps its entity and id; a class and a handler go. It is not a mention.
        let record = clean_safe_html(
            "<p><span data-record-entity=\"mantle.order.OrderHeader\" data-record-id=\"100050\" \
             class=\"chip\" onclick=\"steal()\">#SO100050</span></p>",
        );
        assert!(
            record.contains("data-record-entity=\"mantle.order.OrderHeader\""),
            "{record}"
        );
        assert!(record.contains("data-record-id=\"100050\""), "{record}");
        assert!(record.contains(">#SO100050<"), "{record}");
        assert!(!record.contains("onclick") && !record.contains("class="), "{record}");
        assert!(!record.contains("data-mention-id"), "{record}");
    }
}
