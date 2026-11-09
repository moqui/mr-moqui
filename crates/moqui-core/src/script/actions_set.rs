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

//! Actions that are only simple `<set>` elements, run without the script engine.
//!
//! A [`SetPlan`] is made once per action text, beside its generated script, when every element is a `<set>` of a bare
//! name from one of these:
//! - a `from` that is a bare name, a dotted path (`a.b`, `a?.b`), a number, a quoted string with no escape, `true`,
//!   `false`, `null`, `[]` or `{}`;
//! - a `value` (and a `default-value`) that is text with no `${`, or exactly `${name}` or `${a.b}`;
//! - with a `type` (`NewList` and `NewMap` too) and `set-if-empty`.
//!
//! The plan does what the generated script does (`XmlActions.js`'s `set` and `valueExpr`), as Java's template does:
//! the default when Groovy's `?:` would take it ([`is_groovy_false`]), then `basicConvert`, then the set, skipped
//! under `set-if-empty="false"` when Java's `ObjectUtilities.isEmpty` says the value is empty ([`is_empty_value`]).
//! Values keep their host types: a timestamp, date or time stays one, and a number its width, as
//! in Java, where the script's round trip makes them text and the narrowest number.
//!
//! What the plan cannot answer as the script would (a path through a value that is not a map or an entity value, a
//! relationship, a template of a value that is not text, a number, a boolean or a date) leaves the run to the script,
//! with nothing set: every value is found before the first is set.

use moqui_lib::object::Value;

use crate::context::Error;

/// The steps of actions that are only simple sets.
#[derive(Debug)]
pub(super) struct SetPlan {
    steps: Box<[Step]>,
}

#[derive(Debug)]
struct Step {
    field: String,
    source: Source,
    default: Option<Text>,
    convert: Option<String>,
    /// `set-if-empty="false"`: null or empty text is not set.
    if_not_empty: bool,
}

#[derive(Debug)]
enum Source {
    Path(Path),
    Constant(Value),
    Text(Text),
}

/// A `value` or `default-value`, as `ec.resource.expand` gives it.
#[derive(Debug)]
enum Text {
    Literal(String),
    /// `${path}`: the value at the path as a template literal makes it text.
    Path(Path),
}

/// A bare name and the members read through it, each with whether its read is `?.`.
#[derive(Debug)]
struct Path {
    first: String,
    members: Box<[(String, bool)]>,
}

/// The attributes the `set` macro reads.
const SET_ATTRIBUTES: &[&str] = &["field", "from", "value", "default-value", "type", "set-if-empty"];

/// Words a bare name or member here may not be: JavaScript's reserved words and the names a script does not read
/// from the context.
const RESERVED: &[&str] = &[
    "arguments", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do",
    "else", "enum", "eval", "export", "extends", "false", "finally", "for", "function", "if", "implements", "import",
    "in", "instanceof", "interface", "let", "new", "null", "package", "private", "protected", "public", "return",
    "static", "super", "switch", "this", "throw", "true", "try", "typeof", "var", "void", "while", "with", "yield",
];

/// The global object's own names that start with a lower-case letter, the runtime's builtins and the host's, which a
/// bare name reads before the context (a test keeps the list in step), with the names the bare-name object answers
/// apart from the context (`context`, and the QuickJS globals quickjs-oxide does not bind). A name that starts with
/// an upper-case letter is not planned at all.
pub(super) const GLOBALS: &[&str] = &[
    "atob",
    "basicConvert",
    "btoa",
    "compare",
    "console",
    "context",
    "decodeURI",
    "decodeURIComponent",
    "ec",
    "elasticQueryAutoWildcard",
    "encodeURI",
    "encodeURIComponent",
    "escape",
    "eval",
    "filterMapList",
    "filterMapListByDate",
    "globalThis",
    "isFinite",
    "isNaN",
    "numberToWords",
    "numberToWordsWithDecimal",
    "orderMapList",
    "parseFloat",
    "parseInt",
    "performance",
    "print",
    "queueMicrotask",
    "sri",
    "toDecimal",
    "toNumber",
    "undefined",
    "unescape",
];

/// `Object.prototype`'s members, which a map read as a script object answers by name.
const OBJECT_MEMBERS: &[&str] = &[
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        && !RESERVED.contains(&name)
}

/// A name the context answers: an identifier, not a global, not starting `__` or with an upper-case letter.
fn is_context_name(name: &str) -> bool {
    is_identifier(name)
        && !name.starts_with("__")
        && !name.starts_with(|c: char| c.is_ascii_uppercase())
        && !GLOBALS.contains(&name)
}

/// A member a map or entity value answers with its own entry or field.
fn is_plain_member(name: &str) -> bool {
    is_identifier(name)
        && !name.starts_with("__")
        && !OBJECT_MEMBERS.contains(&name)
        && !crate::script::oxide::bind::entities::is_value_method(name)
}

impl Path {
    fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('.');
        let first = parts.next()?;
        let (first, mut optional) = match first.strip_suffix('?') {
            Some(name) => (name, true),
            None => (first, false),
        };
        if !is_context_name(first) {
            return None;
        }
        let mut members = Vec::new();
        for part in parts {
            let (name, next_optional) = match part.strip_suffix('?') {
                Some(name) => (name, true),
                None => (part, false),
            };
            if !is_plain_member(name) {
                return None;
            }
            members.push((name.to_string(), optional));
            optional = next_optional;
        }
        // a trailing `?` is not a path
        (!optional).then(|| Self {
            first: first.to_string(),
            members: members.into_boxed_slice(),
        })
    }
}

impl Text {
    /// A `value` or `default-value` as `ec.resource.expand` takes it: no `$` (or a trailing one) is the text itself.
    fn parse(text: &str) -> Option<Self> {
        if !text.contains('$') || text.ends_with('$') {
            // a backslash or backtick is the template's to read
            return Some(Self::Literal(text.to_string()));
        }
        let inner = text.strip_prefix("${")?.strip_suffix('}')?;
        Path::parse(inner).map(Self::Path)
    }
}

/// A `from` that is a literal, as the script's value of it is held.
fn constant(text: &str) -> Option<Value> {
    match text {
        "null" => return Some(Value::Null),
        "true" => return Some(Value::Bool(true)),
        "false" => return Some(Value::Bool(false)),
        "[]" => return Some(Value::List(Vec::new())),
        "{}" => return Some(Value::Map(indexmap::IndexMap::new())),
        _ => {}
    }
    for quote in ['\'', '"'] {
        if let Some(inner) = text.strip_prefix(quote).and_then(|rest| rest.strip_suffix(quote)) {
            return (!inner.contains(quote) && !inner.contains('\\') && !inner.contains(['\n', '\r', '\u{2028}', '\u{2029}']))
                .then(|| Value::String(inner.to_string()));
        }
    }
    number(text)
}

/// A decimal number literal: an integer that fits 32 bits as one, a larger safe integer as 64, else a double, as a
/// script number is held.
fn number(text: &str) -> Option<Value> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let plain = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    // a leading zero is an octal literal or an error in a script
    if whole.is_empty() || !plain(whole) || !plain(fraction) || (whole.len() > 1 && whole.starts_with('0')) {
        return None;
    }
    if digits.contains('.') && fraction.is_empty() {
        return None;
    }
    let parsed: f64 = text.parse().ok()?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    if parsed.fract() == 0.0 && parsed.abs() <= crate::script::runtime::SAFE_INTEGER as f64 {
        let integer = parsed as i64;
        return Some(match i32::try_from(integer) {
            Ok(small) => Value::I32(small),
            Err(_) => Value::I64(integer),
        });
    }
    parsed.is_finite().then_some(Value::F64(parsed))
}

impl SetPlan {
    /// The plan of `xml`, or `None` when it is not only simple sets.
    pub(super) fn of(xml: &str) -> Option<Self> {
        let root = moqui_lib::node::MNode::parse_text("actions", xml).ok()?;
        if !root.has_name("actions") {
            return None;
        }
        let mut steps = Vec::new();
        for node in root.child_iter() {
            steps.push(Step::of(&node)?);
        }
        if steps.is_empty() {
            return None;
        }
        Some(Self {
            steps: steps.into_boxed_slice(),
        })
    }

    /// Run the plan: `None` when a value is not one the plan answers as the script would, with nothing set.
    pub(super) fn run(&self) -> Option<Result<(), Error>> {
        let mut found: Vec<(&str, Value)> = Vec::with_capacity(self.steps.len());
        for step in &self.steps {
            if let Some(value) = step.value(&found)? {
                found.push((&step.field, value));
            }
        }
        for (field, value) in found {
            if let Err(err) = crate::script::runtime::set_context_name(field, value) {
                return Some(Err(err));
            }
        }
        Some(Ok(()))
    }
}

impl Step {
    fn of(node: &moqui_lib::node::MNode) -> Option<Self> {
        if !node.has_name("set") || node.child_iter().next().is_some() {
            return None;
        }
        if node.text().is_some_and(|text| !text.trim().is_empty()) {
            return None;
        }
        if node.attributes().iter().any(|(name, _)| !SET_ATTRIBUTES.contains(&name.as_str())) {
            return None;
        }
        let field = node.attribute("field")?;
        if !is_context_name(&field) {
            return None;
        }
        let kind = node.attribute("type").unwrap_or_default();
        let (source, default, convert) = match kind.as_str() {
            "NewList" => (Source::Constant(Value::List(Vec::new())), None, None),
            "NewMap" => (Source::Constant(Value::Map(indexmap::IndexMap::new())), None, None),
            _ => {
                let source = if let Some(from) = node.attribute("from") {
                    let from = from.trim();
                    match constant(from) {
                        Some(value) => Source::Constant(value),
                        None => Source::Path(Path::parse(from)?),
                    }
                } else if let Some(value) = node.attribute("value") {
                    Source::Text(Text::parse(&value)?)
                } else {
                    Source::Constant(Value::Null)
                };
                let default = match node.attribute("default-value") {
                    Some(text) => Some(Text::parse(&text)?),
                    None => None,
                };
                (source, default, (!kind.is_empty()).then_some(kind))
            }
        };
        Some(Self {
            field,
            source,
            default,
            convert,
            if_not_empty: node.attribute_is("set-if-empty", "false"),
        })
    }

    /// The value to set, `Some(None)` for none under `set-if-empty="false"`, or `None` to leave the run to the script.
    fn value(&self, found: &[(&str, Value)]) -> Option<Option<Value>> {
        let mut value = match &self.source {
            Source::Path(path) => path.read(found)?,
            Source::Constant(value) => value.clone(),
            Source::Text(text) => text.value(found)?,
        };
        if let Some(default) = &self.default {
            if is_groovy_false(&value) {
                value = default.value(found)?;
            }
        }
        if let Some(kind) = &self.convert {
            value = crate::l10n::basic_convert(&value, kind).value;
        }
        if self.if_not_empty && is_empty_value(&value) {
            return Some(None);
        }
        Some(Some(value))
    }
}

/// Whether `value` is empty as Java's `ObjectUtilities.isEmpty` has it: null, empty text, or an empty list (an entity
/// list among them), Set or map (an entity value with no fields among them). A zero, false or empty bytes is not.
pub(crate) fn is_empty_value(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::List(items) => items.is_empty(),
        Value::Map(entries) => match entries.get(crate::script::runtime::SET_KEY) {
            Some(Value::List(items)) if entries.len() == 1 => items.is_empty(),
            _ => entries.is_empty(),
        },
        Value::Object(object) => {
            if let Some(list) = object.downcast::<crate::script::host::entities::ScriptList>() {
                list.cells().is_empty()
            } else if let Some(list) = object.downcast::<crate::entity::EntityList>() {
                list.is_empty()
            } else if let Some(cell) = object.downcast::<crate::script::runtime::EntityCell>() {
                moqui_lib::sync::lock(&cell).fields().is_empty()
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Whether `value` is false by Groovy truth, as the `?:` of Java's template takes a default: what
/// [`is_empty_value`] calls empty, false, a zero number or decimal, and empty bytes.
pub(crate) fn is_groovy_false(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => !flag,
        Value::I32(number) => *number == 0,
        Value::I64(number) => *number == 0,
        Value::F64(number) => *number == 0.0,
        Value::Decimal(number) => moqui_lib::object::decimal_is_zero(*number),
        Value::Bytes(bytes) => bytes.is_empty(),
        other => is_empty_value(other),
    }
}

impl Path {
    /// The value at the path, the steps' values first: `None` when a script would read it otherwise.
    fn read(&self, found: &[(&str, Value)]) -> Option<Value> {
        let mut value = match found.iter().rev().find(|(name, _)| *name == self.first) {
            Some((_, value)) => value.clone(),
            None => crate::script::runtime::context_value(&self.first).unwrap_or(Value::Null),
        };
        for (member, optional) in &self.members {
            value = match &value {
                // `a?.b` of null is undefined, and so is the rest of the chain
                Value::Null if *optional => return Some(Value::Null),
                Value::Map(entries) => {
                    // a map with a key the conversion treats apart (a tools map, a Set) is not a plain object
                    if entries.keys().any(|key| key.starts_with("__")) {
                        return None;
                    }
                    entries.get(member).cloned().unwrap_or(Value::Null)
                }
                Value::Object(object) => {
                    let cell = object.downcast::<crate::script::runtime::EntityCell>()?;
                    match crate::script::host::entities::entity_field(&cell, member) {
                        crate::script::host::entities::EntityField::Value(value) => value,
                        crate::script::host::entities::EntityField::Null => Value::Null,
                        _ => return None,
                    }
                }
                // a member of null is the script's TypeError, and other values have members of their own
                _ => return None,
            };
        }
        Some(value)
    }
}

impl Text {
    fn value(&self, found: &[(&str, Value)]) -> Option<Value> {
        match self {
            Text::Literal(text) => Some(Value::String(text.clone())),
            Text::Path(path) => template_text(path.read(found)?).map(Value::String),
        }
    }
}

/// A value as a template literal makes it text, for the values a script holds as text, a boolean or an integer.
fn template_text(value: Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text),
        Value::Null => Some("null".to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::I32(number) => Some(number.to_string()),
        Value::I64(number) if number.unsigned_abs() <= crate::script::runtime::SAFE_INTEGER.unsigned_abs() => {
            Some(number.to_string())
        }
        Value::Timestamp(_) | Value::Date(_) | Value::Time(_) => Some(crate::script::runtime::temporal_text(&value)),
        _ => None,
    }
}

#[cfg(test)]
#[path = "actions_set_tests.rs"]
mod tests;
