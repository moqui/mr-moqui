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

//! Service definition load. One bad service stays out of the registry.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use moqui_lib::node::MNode;
use moqui_lib::sync::{lock, read, write};

use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::transaction::TransactionAttribute;

const SEMAPHORE_ENTITY: &str = "moqui.service.semaphore.ServiceParameterSemaphore";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServiceKind {
    Inline,
    EntityAuto,
    Script,
    Interface,
    RemoteRest,
    /// A service whose body is a function in the Rust service registry. `location` is that function's path.
    Rust,
    Native,
    /// A script service whose language this port does not run, such as Groovy.
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Authenticate {
    Required,
    Off,
    AnonymousAll,
    AnonymousView,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SemaphoreMode {
    None,
    Fail,
    Wait,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Required {
    True,
    False,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Include {
    Pk,
    NonPk,
    All,
}

/// `allow-html`. The default, `none`, rejects text with a `<`. `safe` cleans the markup, as Jsoup
/// does in Java, and `any` leaves it alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AllowHtml {
    #[default]
    None,
    Safe,
    Any,
}

fn allow_html_of(node: &MNode) -> AllowHtml {
    match node.attribute("allow-html").as_deref() {
        Some("any") => AllowHtml::Any,
        Some("safe") => AllowHtml::Safe,
        _ => AllowHtml::None,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AutoParams {
    pub allow_html: AllowHtml,
    pub entity_name: Option<String>,
    pub include: Include,
    pub exclude: Vec<String>,
    pub required: Required,
}

#[derive(Clone, Debug)]
pub(crate) enum Check {
    Matches {
        regexp: String,
        message: String,
    },
    NumberRange {
        min: Option<String>,
        min_eq: bool,
        max: Option<String>,
        max_eq: bool,
        message: Option<String>,
    },
    NumberInteger,
    NumberDecimal,
    TextLength {
        min: Option<usize>,
        max: Option<usize>,
    },
    TextEmail,
    TextUrl,
    TextLetters,
    TextDigits,
    TimeRange {
        after: Option<String>,
        before: Option<String>,
        /// A Java date pattern (`MM/yyyy`) the value is read by, as the `format` attribute says.
        format: Option<String>,
    },
    CreditCard {
        types: Option<String>,
    },
    And(Vec<Check>),
    Or(Vec<Check>),
    Not(Vec<Check>),
}

#[derive(Clone, Debug)]
pub(crate) struct ParamDef {
    pub name: String,
    pub java_type: String,
    pub required: Required,
    /// `default-value`: text with `${}` expanded against the other parameters, then converted.
    pub default_value: Option<String>,
    /// `default`: a JavaScript expression over the other parameters.
    pub default_expr: Option<String>,
    pub allow_html: AllowHtml,
    pub checks: Vec<Check>,
    pub auto: Option<AutoParams>,
    /// `entity-name` and `field-name`: the entity field a form builds this parameter's widget from.
    pub entity_name: Option<String>,
    pub field_name: Option<String>,
    /// `format`, for the date and number widgets of a form.
    pub format: Option<String>,
    /// The `<description>` text, trimmed; none when absent or empty. The service OpenAPI documents it.
    pub description: Option<String>,
    /// Nested `<parameter>` elements: the entries of a `Map` parameter, cleaned and checked the same way.
    pub children: Vec<ParamDef>,
    /// The attributes the element sets, of [`MERGED_ATTRIBUTES`]: what a parameter of the same name takes from it in
    /// [`merge_param`], as Java's `mergeParameter` copies the element's attributes.
    pub given: BTreeSet<&'static str>,
}

/// The parameter attributes [`merge_param`] copies onto a parameter of the same name.
const MERGED_ATTRIBUTES: [&str; 8] = [
    "type",
    "required",
    "default-value",
    "default",
    "allow-html",
    "entity-name",
    "field-name",
    "format",
];

/// Java's `mergeParameter`: a parameter already in `params` with the same name takes the attributes `param` sets, its
/// checks and its nested parameters, and keeps its place, so one that sets only `required` keeps the earlier `type`;
/// any other is added at the end. `auto-parameters` are added as they are, to be expanded and merged at the call. As
/// Java replaces the `<description>` child, one that `param` has replaces the earlier one, and one it lacks keeps it.
pub(crate) fn merge_param(params: &mut Vec<ParamDef>, param: ParamDef) {
    let found = (!param.name.is_empty())
        .then(|| params.iter_mut().find(|have| have.name == param.name))
        .flatten();
    let Some(base) = found else {
        params.push(param);
        return;
    };
    for attribute in &param.given {
        match *attribute {
            "type" => base.java_type.clone_from(&param.java_type),
            "required" => base.required = param.required,
            "default-value" => base.default_value.clone_from(&param.default_value),
            "default" => base.default_expr.clone_from(&param.default_expr),
            "allow-html" => base.allow_html = param.allow_html,
            "entity-name" => base.entity_name.clone_from(&param.entity_name),
            "field-name" => base.field_name.clone_from(&param.field_name),
            "format" => base.format.clone_from(&param.format),
            _ => {}
        }
    }
    if param.description.is_some() {
        base.description = param.description;
    }
    base.given.extend(param.given.iter().copied());
    base.checks.extend(param.checks);
    for child in param.children {
        merge_param(&mut base.children, child);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ImplementRef {
    pub service: String,
    pub required: Option<bool>,
}

#[derive(Clone, Debug)]
pub(crate) struct ServiceDef {
    pub name: String,
    pub verb: String,
    pub noun: String,
    pub kind: ServiceKind,
    /// The `type` attribute, the sub-type of the service's artifact hits.
    pub service_type: String,
    /// `no-remember-parameters`: the hit does not keep the parameters. An implemented service passes it on.
    pub no_remember_parameters: bool,
    pub location: String,
    /// The registered function when `kind` is [`ServiceKind::Rust`].
    pub rust_fn: Option<super::rust_services::RustServiceFn>,
    pub native_fn: Option<crate::native::NativeServiceFn>,
    pub method: String,
    /// `<actions>` XML, or empty when the service has none.
    pub actions: String,
    pub action_source: Option<moqui_lib::error::SourceLocation>,
    pub authenticate: Authenticate,
    pub authz_action: Option<String>,
    pub validate_input: bool,
    pub transaction: TransactionAttribute,
    /// `no-tx-cache`: flush and stop the transaction cache in place before the service runs.
    pub no_tx_cache: bool,
    /// `read-only`: the service only reads (and may set the user's context), so a server-static screen's always-actions
    /// may call it. MR Moqui only.
    pub read_only: bool,
    pub timeout: Option<Duration>,
    pub semaphore: SemaphoreMode,
    pub semaphore_name: Option<String>,
    pub semaphore_timeout: Duration,
    pub semaphore_sleep: Duration,
    pub semaphore_ignore: Duration,
    pub semaphore_parameter: Option<String>,
    pub in_params: Vec<ParamDef>,
    pub out_params: Vec<ParamDef>,
    pub implements: Vec<ImplementRef>,
    /// The in-parameters with `auto-parameters` expanded, made at the first call (see `validate::declared_in`).
    pub expanded_in: super::validate::ExpandedParams,
}

impl ServiceDef {
    pub(crate) fn implicit(name: &str, verb: &str, noun: &str) -> Self {
        Self {
            name: name.to_string(),
            verb: verb.to_string(),
            noun: noun.to_string(),
            kind: ServiceKind::EntityAuto,
            service_type: "entity-implicit".to_string(),
            no_remember_parameters: false,
            location: String::new(),
            rust_fn: None,
            native_fn: None,
            method: String::new(),
            actions: String::new(),
            action_source: None,
            // As Java's `runImplicitEntityAuto`: no login is needed and the service itself is not authorized; each
            // entity operation it makes is.
            authenticate: Authenticate::Off,
            authz_action: None,
            // As Java's `runImplicitEntityAuto`, the parameters go to the entity-auto runner as they came: it reads the
            // entity's fields and the records nested under a relationship name, and nothing else.
            validate_input: false,
            transaction: TransactionAttribute::UseOrBegin,
            no_tx_cache: false,
            read_only: false,
            timeout: None,
            semaphore: SemaphoreMode::None,
            semaphore_name: None,
            semaphore_timeout: Duration::from_secs(120),
            semaphore_sleep: Duration::from_secs(5),
            semaphore_ignore: Duration::from_secs(3600),
            semaphore_parameter: None,
            in_params: Vec::new(),
            out_params: Vec::new(),
            implements: Vec::new(),
            expanded_in: super::validate::ExpandedParams::default(),
        }
    }

    pub(crate) fn semaphore_entity() -> &'static str {
        SEMAPHORE_ENTITY
    }
}

#[derive(Clone, Default)]
struct Registry {
    services: BTreeMap<String, Arc<ServiceDef>>,
    failures: Vec<String>,
    /// What the files read so far define, so a file found by service name is read alone and merged in
    /// (`implements` and `include` resolve across files).
    loaded: Loaded,
    /// Files looked for by service name, found or not, so a miss is not searched again.
    searched: BTreeSet<PathBuf>,
}

/// The service files read, with their services as defined, before includes and implements resolve.
#[derive(Clone, Default)]
struct Loaded {
    defined: BTreeMap<String, Arc<ServiceDef>>,
    /// The failures found reading the files.
    failures: Vec<String>,
    /// Each `service-include` by its local name, with the service it names or why it names none.
    includes: Vec<(String, Result<String, String>)>,
    /// The files read, normalized.
    files: BTreeSet<PathBuf>,
    /// For a service defined in a file under a `service` directory of [`service_roots`], the position of that
    /// directory: a later component's definition overrides an earlier one, as Java's lookup does.
    ranks: BTreeMap<String, usize>,
}

struct ServiceInclude {
    prefix: String,
    verb: String,
    noun: String,
    location: String,
}

#[derive(Default)]
struct Shared {
    current: RwLock<Arc<Registry>>,
    /// Held while the registry is built or a file is merged in, so two loads do not each replace the other's.
    load: Mutex<()>,
}

/// The entered context's factory and its registry, or an error when no context is entered.
fn entered() -> Result<Arc<Shared>, Error> {
    ExecutionContext::with_current(|ctx| registry_of(ctx.factory()))
        .ok_or_else(|| Error::new("no execution context is entered; service definitions belong to its factory"))
}

fn registry_of(factory: &Arc<ExecutionContextFactory>) -> Arc<Shared> {
    factory.extension::<Shared>(|_| Shared::default())
}

/// True once `factory` has read a service file, after which a rust service location registered on it would not be seen
/// by the definitions already read.
pub(crate) fn definitions_read(factory: &ExecutionContextFactory) -> bool {
    !snapshot(&factory.extension::<Shared>(|_| Shared::default()))
        .loaded
        .files
        .is_empty()
}

fn snapshot(shared: &Shared) -> Arc<Registry> {
    Arc::clone(&read(&shared.current))
}

fn replace(shared: &Shared, registry: Registry) {
    *write(&shared.current) = Arc::new(registry);
}

#[cfg(test)]
pub(crate) fn is_registered(name: &str) -> bool {
    find(name).is_some()
}

const MSG_REQUIRED: &str = "Please enter a value";
const MSG_NUMBER: &str = "Please enter a valid number";
const MSG_NUMBER_INT: &str = "Please enter a valid whole number";
const MSG_DIGITS: &str = "Please enter only numbers (digits)";
const MSG_LETTERS: &str = "Please enter only letters";
const MSG_EMAIL: &str = "Please enter a valid email address";
const MSG_URL: &str = "Please enter a valid URL";
const PATTERN_DIGITS: &str = r"^\d*$";
const PATTERN_LETTERS: &str = "^[a-zA-Z]*$";
const PATTERN_EMAIL: &str = r#"^(([^<>()\[\]\\.,;:\s@"]+(\.[^<>()\[\]\\.,;:\s@"]+)*)|(".+"))@((\[[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}])|(([a-zA-Z\-0-9]+\.)+[a-zA-Z]{2,}))$"#;
const PATTERN_URL: &str = r"((([A-Za-z]{3,9}:(?:\/\/)?)(?:[\-;:&=\+\$,\w]+@)?[A-Za-z0-9\.\-]+|(?:www\.|[\-;:&=\+\$,\w]+@)[A-Za-z0-9\.\-]+)((?:\/[\+~%\/\.\w\-_]*)?\??(?:[\-\+=&;%@\.\w_]*)#?(?:[\.\!\/\\\w]*))?)";

/// A check of a field's value made before a submit. qvt writes it as a JavaScript expression over `value`
/// ([`ClientRule::js`]); tui sends it as data ([`ClientRule::json`]) that the terminal client checks itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClientRule {
    Required,
    Number,
    Integer,
    /// A JavaScript regular expression that must match somewhere in a value that is not empty.
    Pattern(String),
    /// A number between the bounds that are set.
    Range {
        min: Option<String>,
        min_eq: bool,
        max: Option<String>,
        max_eq: bool,
    },
}

impl ClientRule {
    /// The rule as qvt checks it. The helpers are called as `$mq`, the name the server-static qvt scope has and the
    /// name the merged qvt mode's browser global has, so one rule text works in both modes.
    pub(crate) fn js(&self) -> String {
        match self {
            Self::Required => "!!value".to_string(),
            Self::Number => "!value || $mq.isStringNumber(value)".to_string(),
            Self::Integer => "!value || $mq.isStringInteger(value)".to_string(),
            Self::Pattern(pattern) => format!("!value || /{pattern}/.test(value)"),
            Self::Range {
                min,
                min_eq,
                max,
                max_eq,
            } => {
                let mut compare = String::new();
                if let Some(min) = min {
                    compare.push_str(&format!(
                        " && $mq.parseNumber(value) {} {min}",
                        if *min_eq { ">=" } else { ">" }
                    ));
                }
                if let Some(max) = max {
                    compare.push_str(&format!(
                        " && $mq.parseNumber(value) {} {max}",
                        if *max_eq { "<=" } else { "<" }
                    ));
                }
                format!("!value || (!Number.isNaN($mq.parseNumber(value)){compare})")
            }
        }
    }

    /// The rule as tui sends it: `{kind}` with `pattern`, or `min`/`minEq`/`max`/`maxEq` for a range.
    pub(crate) fn json(&self) -> serde_json::Value {
        match self {
            Self::Required => serde_json::json!({"kind": "required"}),
            Self::Number => serde_json::json!({"kind": "number"}),
            Self::Integer => serde_json::json!({"kind": "integer"}),
            Self::Pattern(pattern) => serde_json::json!({"kind": "pattern", "pattern": pattern}),
            Self::Range {
                min,
                min_eq,
                max,
                max_eq,
            } => serde_json::json!({"kind": "range", "min": min, "minEq": min_eq, "max": max, "maxEq": max_eq}),
        }
    }
}

/// `FormInstance.getFieldValidationJsRules` for a service parameter: each rule the browser checks with its message,
/// not localized yet. None when the service or parameter is not there.
pub(crate) fn input_rules(service: &str, parameter: &str) -> Option<Vec<(ClientRule, String)>> {
    let def = find(service)?;
    let param = def.in_params.iter().find(|param| param.name == parameter)?;
    let mut rules: Vec<(ClientRule, String)> = Vec::new();
    let mut push = |rule: ClientRule, message: &str| rules.push((rule, message.to_string()));
    let pattern = |text: &str| ClientRule::Pattern(text.to_string());
    if param.required == Required::True {
        push(ClientRule::Required, MSG_REQUIRED);
    }
    let mut found_number = false;
    for check in &param.checks {
        match check {
            Check::NumberInteger if !found_number => {
                push(ClientRule::Integer, MSG_NUMBER_INT);
                found_number = true;
            }
            Check::NumberDecimal if !found_number => {
                push(ClientRule::Number, MSG_NUMBER);
                found_number = true;
            }
            Check::TextDigits if !found_number => {
                push(pattern(PATTERN_DIGITS), MSG_DIGITS);
                found_number = true;
            }
            Check::TextLetters => push(pattern(PATTERN_LETTERS), MSG_LETTERS),
            Check::TextEmail => push(pattern(PATTERN_EMAIL), MSG_EMAIL),
            Check::TextUrl => push(pattern(PATTERN_URL), MSG_URL),
            Check::Matches { regexp, message } => push(pattern(regexp), message),
            Check::NumberRange {
                min,
                min_eq,
                max,
                max_eq,
                message,
            } => {
                let min = min.clone().filter(|text| !text.is_empty());
                let max = max.clone().filter(|text| !text.is_empty());
                let message = message
                    .clone()
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| match (&min, &max) {
                        (Some(min), Some(max)) => format!("Enter a number between {min} and {max}"),
                        (Some(min), None) => format!("Enter a number greater than {min}"),
                        (None, Some(max)) => format!("Enter a number less than {max}"),
                        (None, None) => MSG_NUMBER.to_string(),
                    });
                push(
                    ClientRule::Range {
                        min,
                        min_eq: *min_eq,
                        max,
                        max_eq: *max_eq,
                    },
                    &message,
                );
            }
            _ => {}
        }
    }
    let java_type = param.java_type.as_str();
    if !found_number {
        if ["BigInteger", "Long", "Integer"]
            .iter()
            .any(|suffix| java_type.ends_with(suffix))
        {
            push(ClientRule::Integer, MSG_NUMBER_INT);
        } else if ["BigDecimal", "Double", "Float", "Number"]
            .iter()
            .any(|suffix| java_type.ends_with(suffix))
        {
            push(ClientRule::Number, MSG_NUMBER);
        }
    }
    Some(rules)
}

/// What a form field shows of an input parameter's rules: the classes the macros turn into input attributes.
pub(crate) struct ParamHints {
    pub classes: Vec<&'static str>,
    pub regexp: Option<(String, String)>,
}

/// The validation hints of input parameter `parameter` of `service`, as `getFieldValidationClasses` reads them.
pub(crate) fn input_hints(service: &str, parameter: &str) -> Option<ParamHints> {
    let def = find(service)?;
    let param = def.in_params.iter().find(|param| param.name == parameter)?;
    let mut classes = Vec::new();
    if param.required == Required::True {
        classes.push("required");
    }
    let mut regexp = None;
    for check in &param.checks {
        match check {
            Check::NumberInteger | Check::NumberDecimal if !classes.contains(&"number") => classes.push("number"),
            Check::TextEmail => classes.push("email"),
            Check::TextUrl => classes.push("url"),
            Check::TextDigits => classes.push("digits"),
            Check::CreditCard { .. } => classes.push("creditcard"),
            Check::Matches {
                regexp: pattern,
                message,
            } if regexp.is_none() => {
                regexp = Some((pattern.clone(), message.clone()));
            }
            _ => {}
        }
    }
    let java_type = param.java_type.as_str();
    if [
        "BigDecimal",
        "BigInteger",
        "Long",
        "Integer",
        "Double",
        "Float",
        "Number",
    ]
    .iter()
    .any(|suffix| java_type.ends_with(suffix))
        && !classes.contains(&"number")
    {
        classes.push("number");
    }
    Some(ParamHints { classes, regexp })
}

/// A service of the entered context's factory. `None` when it is not defined or no context is entered.
pub(crate) fn find(name: &str) -> Option<Arc<ServiceDef>> {
    let shared = entered().ok()?;
    if let Some(def) = snapshot(&shared).services.get(name) {
        return Some(Arc::clone(def));
    }
    load_by_name(&shared, name)
}

/// The registered name of a service, as Java's `isServiceDefined` takes it: the name itself, or for a name
/// with no `#` the combined form split at its first capital (`a.b.C.initPartyConfig` is `a.b.C.init#PartyConfig`).
/// `None` when neither is a service.
pub(crate) fn defined_name(name: &str) -> Option<String> {
    if find(name).is_some() {
        return Some(name.to_string());
    }
    if name.contains('#') {
        return None;
    }
    let (path, verb_noun) = name.rsplit_once('.')?;
    let split = verb_noun.find(|ch: char| ch.is_ascii_uppercase())?;
    if split == 0 {
        return None;
    }
    let candidate = format!("{path}.{}#{}", &verb_noun[..split], &verb_noun[split..]);
    find(&candidate).map(|_| candidate)
}

/// Why `name` (or its combined camel-case form, as [`defined_name`] reads it) did not register, when its
/// file was read and the service failed there: the registration failures that name it.
pub(crate) fn registration_failures(name: &str) -> Vec<String> {
    let Ok(shared) = entered() else {
        return Vec::new();
    };
    let mut names = vec![name.to_string()];
    if let Some((path, verb_noun)) = name.rsplit_once('.').filter(|_| !name.contains('#')) {
        if let Some(split) = verb_noun
            .find(|ch: char| ch.is_ascii_uppercase())
            .filter(|split| *split > 0)
        {
            names.push(format!("{path}.{}#{}", &verb_noun[..split], &verb_noun[split..]));
        }
    }
    snapshot(&shared)
        .failures
        .iter()
        .filter(|failure| names.iter().any(|name| failure.contains(&format!("service {name} "))))
        .cloned()
        .collect()
}

/// Java's lookup by name: `a.b.C.verb#noun` is defined in `service/a/b/C.xml` of the framework or a
/// component, the first that has the file. Only that file, and the files it includes, are read, and their services
/// are merged into the registry.
fn load_by_name(shared: &Shared, name: &str) -> Option<Arc<ServiceDef>> {
    let path = service_file_of(name)?;
    let _loading = lock(&shared.load);
    // Another thread may have merged the file while this one waited.
    let current = snapshot(shared);
    if let Some(def) = current.services.get(name) {
        return Some(Arc::clone(def));
    }
    if current.searched.contains(&path) || current.loaded.files.contains(&normalize(&path)) {
        return None;
    }
    let mut next = Registry::clone(&current);
    if let Err(err) = next.merge(std::slice::from_ref(&path)) {
        tracing::warn!(error = %err, path = %path.display(), "could not read the service file for {name}");
        return None;
    }
    next.searched.insert(path);
    next.merge_implemented();
    let found = next.services.get(name).cloned();
    replace(shared, next);
    found
}

/// A file that defines `name`, `service/a/b/C.xml` for `a.b.C.verb#noun`, in the framework or the first component with
/// it. Reading it reads the files of the same path in the other components too (see [`load_file`]).
fn service_file_of(name: &str) -> Option<PathBuf> {
    let path_name = name.split('#').next()?;
    let (file, _) = path_name.rsplit_once('.')?;
    let relative = format!("{}.xml", file.replace('.', "/"));
    service_roots()
        .iter()
        .map(|root| root.join(&relative))
        .find(|candidate| candidate.is_file())
}

/// The `service` directories of the framework and then each component, in Java's search order: the classpath first,
/// then the components, a later one overriding an earlier. Normalized, each once.
fn service_roots() -> Vec<PathBuf> {
    let Some(factory) = ExecutionContext::with_current(|ctx| Arc::clone(ctx.factory())) else {
        return Vec::new();
    };
    let mut roots: Vec<PathBuf> = Vec::new();
    let dirs = std::iter::once(factory.framework_dir().to_path_buf())
        .chain(factory.components().iter().map(|info| info.location.clone()));
    for dir in dirs {
        let root = normalize(&dir.join("service"));
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

/// Java's `getKnownServiceNames`: the services of the `service-file` locations and of every `.xml` file under each
/// component's `service` directory (not `.secas.xml`, `.emecas.xml` or `.rest.xml`), read but not registered. A file
/// that does not parse is logged and skipped.
pub(crate) fn service_names_in_files() -> Vec<String> {
    let mut names = BTreeSet::new();
    let roots = service_roots();
    if let Ok(paths) = configured_paths() {
        for path in paths {
            let path = normalize(&path);
            match roots.iter().find_map(|root| path.strip_prefix(root).ok()) {
                Some(relative) => names_in_file(&path, relative, &mut names),
                None => tracing::warn!(
                    "Service file {} is not in a service directory, its services are not listed",
                    path.display()
                ),
            }
        }
    }
    for root in roots.iter().skip(1) {
        names_in_dir(root, root, &mut names);
    }
    names.into_iter().collect()
}

fn names_in_dir(root: &Path, dir: &Path, names: &mut BTreeSet<String>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = read.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect();
    paths.sort();
    for path in paths {
        let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if path.is_dir() {
            names_in_dir(root, &path, names);
        } else if file_name.ends_with(".xml")
            && ![".secas.xml", ".emecas.xml", ".gitecas.xml", ".rest.xml"]
                .iter()
                .any(|suffix| file_name.ends_with(suffix))
        {
            if let Ok(relative) = path.strip_prefix(root) {
                names_in_file(&path, relative, names);
            }
        }
    }
}

/// The names of the `service` and `service-include` elements of the file at `relative` in a `service` directory.
fn names_in_file(path: &Path, relative: &Path, names: &mut BTreeSet<String>) {
    let root = match MNode::parse_file(path) {
        Ok(root) => root,
        Err(err) => {
            tracing::error!(error = %err, "Could not read service file {} for the known service names", path.display());
            return;
        }
    };
    if !root.has_name("services") {
        return;
    }
    let prefix = relative.with_extension("").to_string_lossy().replace(['/', '\\'], ".");
    for child in root.children() {
        if matches!(child.name().as_str(), "service" | "service-include") {
            let verb = child.attribute("verb").unwrap_or_default();
            let noun = child.attribute("noun").unwrap_or_default();
            names.insert(service_name(&prefix, &verb, &noun));
        }
    }
}

/// Every service registered with the entered context's factory, by name. Type generation reads these.
///
/// # Errors
///
/// Returns an error when no context is entered.
pub(crate) fn registered() -> Result<Vec<ServiceDef>, Error> {
    Ok(snapshot(&*entered()?)
        .services
        .values()
        .map(|def| ServiceDef::clone(def))
        .collect())
}

/// The services `paths` define, and the registration failures, without registering them.
///
/// # Errors
///
/// Returns an error when a file cannot be read.
#[cfg(test)]
pub(crate) fn definitions_in(paths: &[PathBuf]) -> Result<(Vec<ServiceDef>, Vec<String>), Error> {
    let registry = build(paths)?;
    Ok((
        registry.services.into_values().map(Arc::unwrap_or_clone).collect(),
        registry.failures,
    ))
}

/// Register the configured service files with the entered context's factory.
///
/// # Errors
///
/// Returns an error when no context is entered or a file cannot be read.
pub fn load_definitions() -> Result<(), Error> {
    lint(&[])?;
    Ok(())
}

/// Register `paths`, or the configured service files when `paths` is empty, with the entered
/// context's factory, replacing what it had. Returns the registration failures.
///
/// # Errors
///
/// Returns an error when no context is entered or a file cannot be read.
pub fn lint(paths: &[PathBuf]) -> Result<Vec<String>, Error> {
    if paths.is_empty() {
        let configured = configured_paths()?;
        install(&configured)?;
    } else {
        install(paths)?;
    }
    Ok(snapshot(&*entered()?).failures.clone())
}

/// Registration failures for `paths`, or the configured service files when `paths` is empty,
/// without changing the services that are registered. This is what `moqui lint` runs.
///
/// # Errors
///
/// Returns an error when a file cannot be read.
pub fn check(paths: &[PathBuf]) -> Result<Vec<String>, Error> {
    if paths.is_empty() {
        return Ok(build(&configured_paths()?)?.failures);
    }
    // The files given, then the files of the services they implement, found by name as a server finds them (an
    // interface of another file, such as the framework's EmailServices.xml): only the failures of the given files'
    // services, and of the files themselves, are theirs to report.
    let mut registry = build(paths)?;
    let own = registry.failures.clone();
    let names: Vec<String> = registry
        .loaded
        .defined
        .keys()
        .cloned()
        .chain(registry.loaded.includes.iter().map(|(local, _)| local.clone()))
        .collect();
    registry.merge_implemented();
    Ok(registry
        .failures
        .into_iter()
        .filter(|failure| {
            own.contains(failure) || names.iter().any(|name| failure.contains(&format!("service {name} ")))
        })
        .collect())
}

fn install(paths: &[PathBuf]) -> Result<(), Error> {
    let shared = entered()?;
    let _loading = lock(&shared.load);
    replace(&shared, build(paths)?);
    Ok(())
}

fn build(paths: &[PathBuf]) -> Result<Registry, Error> {
    let mut registry = Registry::default();
    registry.merge(paths)?;
    Ok(registry)
}

impl Registry {
    /// Read `paths` and the files they include, those not read already, and resolve the services again. A service
    /// resolved before keeps its definition, unless a new file defines a service of the same name: then all resolve again.
    fn merge(&mut self, paths: &[PathBuf]) -> Result<(), Error> {
        let mut includes = Vec::new();
        let mut added = Vec::new();
        let mut pending: Vec<PathBuf> = paths.to_vec();
        let roots = service_roots();
        while let Some(path) = pending.pop() {
            load_file(&path, &roots, &mut self.loaded, &mut includes, &mut pending, &mut added)?;
        }
        for include in includes {
            let local = service_name(&include.prefix, &include.verb, &include.noun);
            let target = follow_include(&include, &[], 0);
            self.loaded.includes.push((local, target));
        }
        let previous = if added.iter().any(|name| self.services.contains_key(name)) {
            BTreeMap::new()
        } else {
            std::mem::take(&mut self.services)
        };
        let (services, failures) = resolve(&self.loaded, previous);
        self.services = services;
        self.failures = failures;
        Ok(())
    }

    /// A service that implements one of a file not read yet failed to register: read that file, as Java looks the
    /// implemented service up by name, and repeat for what that file implements. A file that cannot be read is logged.
    fn merge_implemented(&mut self) {
        loop {
            let missing: Vec<PathBuf> = self
                .loaded
                .defined
                .values()
                .flat_map(|def| def.implements.iter().map(|implement| implement.service.clone()))
                .filter(|target| !self.services.contains_key(target))
                .filter_map(|target| service_file_of(&target))
                .filter(|file| !self.searched.contains(file) && !self.loaded.files.contains(&normalize(file)))
                .collect();
            let Some(file) = missing.into_iter().next() else {
                break;
            };
            self.searched.insert(file.clone());
            if let Err(err) = self.merge(std::slice::from_ref(&file)) {
                tracing::warn!(error = %err, path = %file.display(), "could not read the service file for an implemented service");
            }
        }
    }
}

/// The services of `loaded` with includes and implements resolved, and the failures. The services in `previous` are
/// resolved already and stay as they are.
fn resolve(
    loaded: &Loaded,
    previous: BTreeMap<String, Arc<ServiceDef>>,
) -> (BTreeMap<String, Arc<ServiceDef>>, Vec<String>) {
    let mut failures = loaded.failures.clone();
    let done: BTreeSet<String> = previous.keys().cloned().collect();
    let mut services = previous;
    for (name, def) in &loaded.defined {
        if !done.contains(name) {
            services.insert(name.clone(), Arc::clone(def));
        }
    }
    // Includes first, so a service can implement a name that is only an include alias.
    resolve_includes(loaded, &mut services, &mut failures);
    resolve_implements(&mut services, &mut failures, done);
    (services, failures)
}

#[cfg(test)]
thread_local! {
    /// The service files this thread parsed, for the tests that count them.
    static PARSED: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Read one service file. A file in a `service` directory of `roots` also queues the files of the same path in the
/// other directories, and a service it defines replaces one of the same name only from a later directory, so a
/// component overrides the framework and the components before it, as Java's `findServiceNode`.
fn load_file(
    path: &Path,
    roots: &[PathBuf],
    loaded: &mut Loaded,
    includes: &mut Vec<ServiceInclude>,
    pending: &mut Vec<PathBuf>,
    added: &mut Vec<String>,
) -> Result<(), Error> {
    let path = normalize(path);
    if !loaded.files.insert(path.clone()) {
        return Ok(());
    }
    #[cfg(test)]
    PARSED.with(|parsed| parsed.borrow_mut().push(path.clone()));
    // A file that does not parse loses only its own services, as in Java, where a file is read when one of its
    // services is first looked up.
    let root = match MNode::parse_file(&path) {
        Ok(root) => root,
        Err(err) => {
            tracing::error!(error = %err, "Could not read service file {}, its services are not registered", path.display());
            loaded
                .failures
                .push(format!("{} could not be read: {err}", path.display()));
            return Ok(());
        }
    };
    let failures = &mut loaded.failures;
    if !root.has_name("services") {
        failures.push(format!("{} root is {} and not services", path.display(), root.name()));
        return Ok(());
    }
    let place = roots.iter().enumerate().find_map(|(rank, root)| {
        path.strip_prefix(root)
            .ok()
            .map(|relative| (rank, relative.to_path_buf()))
    });
    if let Some((_, relative)) = &place {
        for root in roots {
            let other = root.join(relative);
            if other != path && other.is_file() {
                pending.push(other);
            }
        }
    }
    let rank = place.map(|(rank, _)| rank);
    let prefix = service_prefix(&path);
    for child in root.children() {
        match child.name().as_str() {
            "service" => {
                if let Some(def) = load_service(&prefix, &child, failures) {
                    if let (Some(rank), Some(&have)) = (rank, loaded.ranks.get(&def.name)) {
                        if have > rank {
                            continue;
                        }
                        tracing::info!(
                            "Found service {} at {} which overrides an earlier one",
                            def.name,
                            path.display()
                        );
                    }
                    if let Some(rank) = rank {
                        loaded.ranks.insert(def.name.clone(), rank);
                    }
                    added.push(def.name.clone());
                    loaded.defined.insert(def.name.clone(), Arc::new(def));
                }
            }
            "service-include" => {
                let verb = child.attribute("verb").unwrap_or_default();
                let noun = child.attribute("noun").unwrap_or_default();
                let location = child.attribute("location").unwrap_or_default();
                if location.is_empty() {
                    failures.push(format!(
                        "service-include {}.{} is missing a location",
                        prefix,
                        service_tail(&verb, &noun)
                    ));
                    continue;
                }
                let included = match resolve_location(&location) {
                    Ok(included) => included,
                    Err(err) => {
                        failures.push(format!(
                            "service-include {}.{} location {location} could not be resolved: {err}",
                            prefix,
                            service_tail(&verb, &noun)
                        ));
                        continue;
                    }
                };
                pending.push(included);
                includes.push(ServiceInclude {
                    prefix: prefix.clone(),
                    verb,
                    noun,
                    location,
                });
            }
            _ => {}
        }
    }
    Ok(())
}

fn load_service(prefix: &str, node: &MNode, failures: &mut Vec<String>) -> Option<ServiceDef> {
    let Some(verb) = node.attribute("verb") else {
        failures.push(format!("service in {prefix} is missing verb"));
        return None;
    };
    let noun = node.attribute("noun").unwrap_or_default();
    let name = service_name(prefix, &verb, &noun);
    let type_name = node.attribute("type").unwrap_or_else(|| "inline".to_string());
    if type_name == "remote-json-rpc" {
        failures.push(format!("service {name} JSON-RPC is not included"));
        return None;
    }
    if type_name == "java" {
        failures.push(format!("service {name} has no Java runner"));
        return None;
    }
    if type_name == "camel" {
        failures.push(format!("service {name} type camel is not included"));
        return None;
    }
    let native_fn = ExecutionContext::with_current(|ec| {
        if type_name == "rust" {
            ec.factory()
                .native()
                .service_at(&node.attribute("location").unwrap_or_default())
        } else {
            ec.factory().native().runner_for(&type_name)
        }
    })
    .flatten();
    if native_fn.is_none()
        && !matches!(
            type_name.as_str(),
            "inline" | "entity-auto" | "script" | "interface" | "remote-rest" | "rust"
        )
    {
        failures.push(format!("service {name} has unknown type {type_name}"));
        return None;
    }
    if (type_name == "rust" || native_fn.is_some()) && node.first_named("actions").is_some() {
        failures.push(format!(
            "service {name} type {type_name} has actions; the registered function is the body"
        ));
        return None;
    }
    let mut expression_errors = Vec::new();
    if !expressions_ok(&name, node, &mut expression_errors) {
        failures.extend(expression_errors);
        return None;
    }
    // The generated script is what runs, so it is what must parse. This reaches every attribute
    // that becomes JavaScript, not only the ones named in `is_expression_attr`.
    if let Some(actions) = node.first_named("actions") {
        if let Err(err) = crate::script::check_actions(&actions.write_string()) {
            failures.push(format!("service {name} {err}"));
            return None;
        }
    }
    // As Java's `Integer.valueOf` and `Long.parseLong` of these attributes, text that is not a number fails the service.
    let numbers = (|| {
        Ok::<_, String>((
            timeout_of(node)?,
            seconds_attr(node, "semaphore-timeout", 120)?,
            seconds_attr(node, "semaphore-sleep", 5)?,
            seconds_attr(node, "semaphore-ignore", 3600)?,
        ))
    })();
    let (timeout, semaphore_timeout, semaphore_sleep, semaphore_ignore) = match numbers {
        Ok(numbers) => numbers,
        Err(err) => {
            failures.push(format!("service {name} {err}"));
            return None;
        }
    };
    let location = node.attribute("location").unwrap_or_default();
    let rust_fn = if type_name == "rust" && native_fn.is_none() {
        if location.is_empty() {
            failures.push(format!("service {name} type rust is missing a location"));
            return None;
        }
        match super::rust_services::get(&location) {
            Some(run) => Some(run),
            // Loads; a call says the feature is not built in (call.rs).
            None if super::rust_services::missing_feature(&location).is_some() => None,
            None => {
                failures.push(format!("service {name} rust location {location} is not registered"));
                return None;
            }
        }
    } else {
        None
    };
    let kind = if native_fn.is_some() {
        ServiceKind::Native
    } else if rust_fn.is_some() || type_name == "rust" {
        ServiceKind::Rust
    } else {
        kind_of(&type_name, &location)
    };
    let def = ServiceDef {
        name: name.clone(),
        verb,
        noun,
        kind,
        service_type: type_name,
        no_remember_parameters: bool_attr(node, "no-remember-parameters", false),
        location,
        rust_fn,
        native_fn,
        method: node.attribute("method").unwrap_or_default(),
        actions: node
            .first_named("actions")
            .map(|node| node.write_string())
            .unwrap_or_default(),
        action_source: node
            .first_named("actions")
            .map(|actions| moqui_lib::error::SourceLocation {
                file: actions.file_location(),
                line: actions.line(),
                column: actions.column(),
            }),
        authenticate: authenticate_of(node),
        authz_action: node.attribute("authz-action"),
        validate_input: bool_attr(node, "validate", true),
        transaction: transaction_of(node),
        no_tx_cache: bool_attr(node, "no-tx-cache", false),
        read_only: bool_attr(node, "read-only", false),
        timeout,
        semaphore: semaphore_of(node),
        semaphore_name: node.attribute("semaphore-name"),
        semaphore_timeout,
        semaphore_sleep,
        semaphore_ignore,
        semaphore_parameter: node.attribute("semaphore-parameter"),
        in_params: node
            .first_named("in-parameters")
            .map(|node| parse_param_list(&node))
            .unwrap_or_default(),
        out_params: node
            .first_named("out-parameters")
            .map(|node| parse_param_list(&node))
            .unwrap_or_default(),
        implements: node
            .children_named("implements")
            .iter()
            .filter_map(|child| {
                let service = child.attribute("service")?;
                let required = match child.attribute("required").as_deref() {
                    Some(value) if value.eq_ignore_ascii_case("true") => Some(true),
                    Some(value) if value.eq_ignore_ascii_case("false") => Some(false),
                    _ => None,
                };
                Some(ImplementRef { service, required })
            })
            .collect(),
        expanded_in: super::validate::ExpandedParams::default(),
    };
    Some(def)
}

fn kind_of(type_name: &str, location: &str) -> ServiceKind {
    match type_name {
        "entity-auto" => ServiceKind::EntityAuto,
        "script" if is_groovy_location(location) => ServiceKind::Unsupported,
        "script" => ServiceKind::Script,
        "interface" => ServiceKind::Interface,
        "remote-rest" => ServiceKind::RemoteRest,
        _ => ServiceKind::Inline,
    }
}

/// A Groovy script, as a component written for Java Moqui names it, is not runnable here.
fn is_groovy_location(location: &str) -> bool {
    let path = location.split(['?', '#']).next().unwrap_or(location);
    path.to_ascii_lowercase().ends_with(".groovy")
}

fn authenticate_of(node: &MNode) -> Authenticate {
    match node.attribute("authenticate").as_deref() {
        Some("false") => Authenticate::Off,
        Some("anonymous-all") => Authenticate::AnonymousAll,
        Some("anonymous-view") => Authenticate::AnonymousView,
        _ => Authenticate::Required,
    }
}

fn transaction_of(node: &MNode) -> TransactionAttribute {
    match node.attribute("transaction").as_deref() {
        Some("ignore") => TransactionAttribute::Ignore,
        Some("force-new") => TransactionAttribute::ForceNew,
        Some("cache") => TransactionAttribute::Cache,
        Some("force-cache") => TransactionAttribute::ForceCache,
        _ => TransactionAttribute::UseOrBegin,
    }
}

/// `transaction-timeout` in seconds; none when absent or not above zero. Text that is not a whole number is an error.
fn timeout_of(node: &MNode) -> Result<Option<Duration>, String> {
    let Some(text) = node.attribute("transaction-timeout").filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    let seconds = text
        .parse::<i32>()
        .map_err(|_| format!("transaction-timeout {text} is not a whole number"))?;
    Ok(u64::try_from(seconds)
        .ok()
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs))
}

fn semaphore_of(node: &MNode) -> SemaphoreMode {
    match node.attribute("semaphore").as_deref() {
        Some("fail") => SemaphoreMode::Fail,
        Some("wait") => SemaphoreMode::Wait,
        _ => SemaphoreMode::None,
    }
}

/// A seconds attribute, `default` when absent or empty. Text that is not a whole number is an error.
fn seconds_attr(node: &MNode, name: &str, default: u64) -> Result<Duration, String> {
    let Some(text) = node.attribute(name).filter(|text| !text.is_empty()) else {
        return Ok(Duration::from_secs(default));
    };
    let seconds = text
        .parse::<i64>()
        .map_err(|_| format!("{name} {text} is not a whole number"))?;
    Ok(Duration::from_secs(u64::try_from(seconds).unwrap_or(0)))
}

fn bool_attr(node: &MNode, name: &str, default: bool) -> bool {
    node.with_attribute(name, |value| match value {
        Some(value) if value.eq_ignore_ascii_case("true") => true,
        Some(value) if value.eq_ignore_ascii_case("false") => false,
        _ => default,
    })
}

fn expressions_ok(service: &str, node: &MNode, errors: &mut Vec<String>) -> bool {
    let mut ok = true;
    if node.has_name("script") {
        if let Some(text) = node.text() {
            if let Err(err) = check_javascript(service, &text) {
                errors.push(err);
                ok = false;
            }
        }
    }
    node.for_each_attribute(|key, value| {
        if is_expression_attr(key) {
            if let Err(err) = check_javascript(service, value) {
                errors.push(err);
                ok = false;
            }
        }
    });
    node.for_each_child(|child| {
        if !expressions_ok(service, child, errors) {
            ok = false;
        }
    });
    ok
}

fn is_expression_attr(key: &str) -> bool {
    matches!(key, "from" | "condition" | "default" | "in-map")
}

fn check_javascript(service: &str, text: &str) -> Result<(), String> {
    crate::script::parse_js(text).map_err(|err| format!("service {service} {err}"))
}

fn parse_param_list(node: &MNode) -> Vec<ParamDef> {
    let mut params = Vec::new();
    for child in node.children() {
        match child.name().as_str() {
            "parameter" => params.push(parse_parameter(&child)),
            "auto-parameters" => params.push(parse_auto(&child)),
            _ => {}
        }
    }
    params
}

fn parse_parameter(node: &MNode) -> ParamDef {
    ParamDef {
        name: node.attribute("name").unwrap_or_default(),
        java_type: node
            .attribute("type")
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "String".to_string()),
        required: required_of(node, Required::False),
        // As Java's `ParameterInfo`, an empty `default-value` is no default.
        default_value: node.attribute("default-value").filter(|text| !text.is_empty()),
        default_expr: node.attribute("default").filter(|text| !text.is_empty()),
        allow_html: allow_html_of(node),
        checks: parse_checks(node),
        auto: None,
        entity_name: node.attribute("entity-name").filter(|text| !text.is_empty()),
        field_name: node.attribute("field-name").filter(|text| !text.is_empty()),
        format: node.attribute("format").filter(|text| !text.is_empty()),
        description: node
            .first_named("description")
            .and_then(|child| child.text())
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty()),
        children: node.children_named("parameter").iter().map(parse_parameter).collect(),
        given: MERGED_ATTRIBUTES
            .into_iter()
            .filter(|name| node.attribute(name).is_some())
            .collect(),
    }
}

fn parse_auto(node: &MNode) -> ParamDef {
    let include = match node.attribute("include").as_deref() {
        Some("pk") => Include::Pk,
        Some("nonpk") => Include::NonPk,
        _ => Include::All,
    };
    let exclude = node
        .children_named("exclude")
        .iter()
        .filter_map(|child| child.attribute("field-name"))
        .collect();
    let required = required_of(node, Required::False);
    ParamDef {
        name: String::new(),
        java_type: "String".to_string(),
        required,
        default_value: None,
        default_expr: None,
        allow_html: AllowHtml::None,
        checks: Vec::new(),
        auto: Some(AutoParams {
            allow_html: allow_html_of(node),
            entity_name: node.attribute("entity-name"),
            include,
            exclude,
            required,
        }),
        entity_name: None,
        field_name: None,
        format: None,
        description: None,
        children: Vec::new(),
        given: BTreeSet::new(),
    }
}

fn required_of(node: &MNode, default: Required) -> Required {
    match node.attribute("required").as_deref() {
        Some("true") => Required::True,
        Some("false") => Required::False,
        Some("disabled") => Required::Disabled,
        _ => default,
    }
}

fn parse_checks(node: &MNode) -> Vec<Check> {
    let mut checks = Vec::new();
    for child in node.children() {
        if let Some(check) = parse_check(&child) {
            checks.push(check);
        }
    }
    checks
}

fn parse_check(node: &MNode) -> Option<Check> {
    let check = match node.name().as_str() {
        "matches" => Check::Matches {
            regexp: node.attribute("regexp").unwrap_or_default(),
            message: node.attribute("message").unwrap_or_default(),
        },
        "number-range" => Check::NumberRange {
            min: node.attribute("min"),
            min_eq: bool_attr(node, "min-include-equals", true),
            max: node.attribute("max"),
            max_eq: bool_attr(node, "max-include-equals", false),
            message: node.attribute("message"),
        },
        "number-integer" => Check::NumberInteger,
        "number-decimal" => Check::NumberDecimal,
        "text-length" => Check::TextLength {
            min: usize_attr(node, "min"),
            max: usize_attr(node, "max"),
        },
        "text-email" => Check::TextEmail,
        "text-url" => Check::TextUrl,
        "text-letters" => Check::TextLetters,
        "text-digits" => Check::TextDigits,
        "time-range" => Check::TimeRange {
            after: node.attribute("after"),
            before: node.attribute("before"),
            format: node.attribute("format"),
        },
        "credit-card" => Check::CreditCard {
            types: node.attribute("types"),
        },
        "val-or" => Check::Or(parse_checks(node)),
        "val-and" => Check::And(parse_checks(node)),
        "val-not" => Check::Not(parse_checks(node)),
        _ => return None,
    };
    Some(check)
}

fn usize_attr(node: &MNode, name: &str) -> Option<usize> {
    node.attribute(name).and_then(|text| text.parse().ok())
}

/// Resolve the implements of each service not in `done`, the names resolved already.
fn resolve_implements(
    services: &mut BTreeMap<String, Arc<ServiceDef>>,
    failures: &mut Vec<String>,
    mut done: BTreeSet<String>,
) {
    let names: Vec<String> = services.keys().filter(|name| !done.contains(*name)).cloned().collect();
    for name in names {
        let mut stack = Vec::new();
        resolve_one(&name, services, failures, &mut done, &mut stack);
    }
}

fn resolve_one(
    name: &str,
    services: &mut BTreeMap<String, Arc<ServiceDef>>,
    failures: &mut Vec<String>,
    done: &mut BTreeSet<String>,
    stack: &mut Vec<String>,
) {
    if done.contains(name) || !services.contains_key(name) {
        return;
    }
    if stack.iter().any(|item| item == name) {
        failures.push(format!("service {name} implements itself"));
        services.remove(name);
        return;
    }
    stack.push(name.to_string());
    let implements = services.get(name).map(|def| def.implements.clone()).unwrap_or_default();
    let mut targets = Vec::new();
    for implement in implements {
        if !services.contains_key(&implement.service) {
            failures.push(format!(
                "service {name} implements unknown service {}",
                implement.service
            ));
            services.remove(name);
            stack.pop();
            return;
        }
        resolve_one(&implement.service, services, failures, done, stack);
        if !services.contains_key(name) || !services.contains_key(&implement.service) {
            if services.contains_key(name) {
                failures.push(format!(
                    "service {name} implements unknown service {}",
                    implement.service
                ));
                services.remove(name);
            }
            stack.pop();
            return;
        }
        let Some(target) = services.get(&implement.service).map(Arc::clone) else {
            stack.pop();
            return;
        };
        targets.push((target, implement.required));
    }
    if !targets.is_empty() {
        if let Some(def) = services.get_mut(name) {
            let def = Arc::make_mut(def);
            // As Java: the implemented services' parameters first, then the service's own merged into them by name.
            let (mut in_params, mut out_params) = (Vec::new(), Vec::new());
            for (target, required) in &targets {
                def.no_remember_parameters |= target.no_remember_parameters;
                implemented(&mut in_params, &target.in_params, *required);
                implemented(&mut out_params, &target.out_params, *required);
            }
            for param in std::mem::take(&mut def.in_params) {
                merge_param(&mut in_params, param);
            }
            for param in std::mem::take(&mut def.out_params) {
                merge_param(&mut out_params, param);
            }
            def.in_params = in_params;
            def.out_params = out_params;
        }
    }
    done.insert(name.to_string());
    stack.pop();
}

/// The parameters of an implemented service, added before the service's own, with `required` set on each when the
/// `implements` element has it. A `required="disabled"` parameter is left out: it behaves as if it did not exist. When
/// two implemented services have a parameter of the same name the first one's is kept.
fn implemented(dest: &mut Vec<ParamDef>, extra: &[ParamDef], required: Option<bool>) {
    for param in extra {
        if param.required == Required::Disabled {
            continue;
        }
        if !param.name.is_empty() && dest.iter().any(|have| have.name == param.name) {
            continue;
        }
        let mut param = param.clone();
        if let Some(flag) = required {
            let flag = if flag { Required::True } else { Required::False };
            param.required = flag;
            param.given.insert("required");
            if let Some(auto) = param.auto.as_mut() {
                auto.required = flag;
            }
        }
        dest.push(param);
    }
}

/// Add a service under each include's local name, a copy of the service it names as that file defines it.
fn resolve_includes(loaded: &Loaded, services: &mut BTreeMap<String, Arc<ServiceDef>>, failures: &mut Vec<String>) {
    let mut aliases = Vec::new();
    for (local, target) in &loaded.includes {
        match target {
            Ok(target) if loaded.defined.contains_key(target) => aliases.push((local, target)),
            Ok(target) => failures.push(format!("service {local} include did not resolve to {target}")),
            Err(err) => failures.push(err.clone()),
        }
    }
    for (local, target) in aliases {
        if services.contains_key(local) {
            continue;
        }
        let Some(def) = loaded.defined.get(target) else {
            continue;
        };
        let mut alias = ServiceDef::clone(def);
        alias.name.clone_from(local);
        services.insert(local.clone(), Arc::new(alias));
    }
}

fn follow_include(include: &ServiceInclude, stack: &[PathBuf], depth: usize) -> Result<String, String> {
    if depth > 32 {
        return Err(format!(
            "service-include {} is too deep",
            service_name(&include.prefix, &include.verb, &include.noun)
        ));
    }
    let path = resolve_location(&include.location).map_err(|err| err.to_string())?;
    let path = normalize(&path);
    if stack.iter().any(|seen| seen == &path) {
        return Err(format!(
            "service-include {} is circular",
            service_name(&include.prefix, &include.verb, &include.noun)
        ));
    }
    let root = MNode::parse_file(&path).map_err(|err| err.to_string())?;
    let Some(node) = find_member(&root, &include.verb, &include.noun) else {
        return Err(format!(
            "service-include {} was not found in {}",
            service_name(&include.prefix, &include.verb, &include.noun),
            path.display()
        ));
    };
    if node.has_name("service-include") {
        let location = node.attribute("location").unwrap_or_default();
        let next = ServiceInclude {
            prefix: service_prefix(&path),
            verb: include.verb.clone(),
            noun: include.noun.clone(),
            location,
        };
        let mut stack = stack.to_vec();
        stack.push(path);
        return follow_include(&next, &stack, depth + 1);
    }
    let verb = node.attribute("verb").unwrap_or_else(|| include.verb.clone());
    let noun = node.attribute("noun").unwrap_or_default();
    Ok(service_name(&service_prefix(&path), &verb, &noun))
}

fn find_member(root: &MNode, verb: &str, noun: &str) -> Option<MNode> {
    root.children().into_iter().find(|child| {
        let name = child.name();
        if name != "service" && name != "service-include" {
            return false;
        }
        let child_verb = child.attribute("verb").unwrap_or_default();
        let child_noun = child.attribute("noun").unwrap_or_default();
        if noun.is_empty() {
            format!("{child_verb}{child_noun}") == verb || (child_verb == verb && child_noun.is_empty())
        } else {
            child_verb == verb && child_noun == noun
        }
    })
}

fn service_prefix(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let Some(index) = text.rfind("/service/") else {
        return String::new();
    };
    let rest = &text[index + "/service/".len()..];
    let rest = rest.strip_suffix(".xml").unwrap_or(rest);
    if rest.is_empty() {
        String::new()
    } else {
        rest.replace('/', ".")
    }
}

fn service_tail(verb: &str, noun: &str) -> String {
    if noun.is_empty() {
        verb.to_string()
    } else {
        format!("{verb}#{noun}")
    }
}

fn service_name(prefix: &str, verb: &str, noun: &str) -> String {
    let tail = service_tail(verb, noun);
    if prefix.is_empty() {
        tail
    } else {
        format!("{prefix}.{tail}")
    }
}

fn normalize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The `service-file` locations in the framework conf, resolved. [`lint`] with no paths registers
/// these; a host that registers more passes these first.
///
/// # Errors
///
/// Returns an error when the conf cannot be read. A location that cannot be resolved is logged and skipped.
pub fn configured_service_files() -> Result<Vec<PathBuf>, Error> {
    configured_paths()
}

fn configured_paths() -> Result<Vec<PathBuf>, Error> {
    let configured = ExecutionContext::with_current(|ec| ec.factory().conf_text().to_string())
        .filter(|conf| !conf.trim().is_empty())
        .map(|conf| MNode::parse_text("moqui-conf", &conf))
        .transpose()?;
    let root = match configured.filter(|root| root.first_named("service-facade").is_some()) {
        Some(root) => root,
        None => MNode::parse_file(framework_dir().join("conf/MoquiDefaultConf.xml"))?,
    };
    let facade = root
        .first_named("service-facade")
        .ok_or_else(|| Error::new("service-facade is missing"))?;
    let mut paths = Vec::new();
    for node in facade.children_named("service-file") {
        let Some(location) = node.attribute("location") else {
            continue;
        };
        if location.is_empty() {
            continue;
        }
        match resolve_location(&location) {
            Ok(path) => paths.push(path),
            Err(err) => {
                tracing::error!(error = %err, "Could not resolve service-file location {location}, skipping it")
            }
        }
    }
    Ok(paths)
}

/// `classpath://` and `component://` locations. Copied from the private entity loader.
fn resolve_location(location: &str) -> Result<PathBuf, Error> {
    crate::context::locate(location).map_err(|err| Error::new(err.message().to_string()))
}

fn framework_dir() -> PathBuf {
    crate::context::framework_dir()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn email_services_resolve_rust_locations() {
        let service_dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework/service/org/moqui");
        let paths = [
            service_dir.join("EmailServices.xml"),
            service_dir.join("impl/EmailServices.xml"),
        ];
        let (defs, failures) = super::definitions_in(&paths).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        let expect = [
            ("send#Email", "moqui_core::service::mail::send_template"),
            ("send#EmailTemplate", "moqui_core::service::mail::send_template"),
            ("send#EmailMessage", "moqui_core::service::mail::send_message"),
            ("poll#EmailServer", "moqui_core::service::mail::poll_server"),
        ];
        for (name, location) in expect {
            let full = format!("org.moqui.impl.EmailServices.{name}");
            let def = defs.iter().find(|def| def.name == full).expect(name);
            assert_eq!(def.kind, super::ServiceKind::Rust, "{full}");
            assert_eq!(def.location, location, "{full}");
            assert!(def.rust_fn.is_some(), "{full}");
        }
        for name in ["save#EcaEmailMessage", "send#ScreenRenderEmail"] {
            let full = format!("org.moqui.impl.EmailServices.{name}");
            let def = defs.iter().find(|def| def.name == full).expect(name);
            assert_eq!(def.kind, super::ServiceKind::Inline, "{full}");
        }
        // A Groovy script location from a component written for Java Moqui is still not runnable.
        assert_eq!(
            super::kind_of("script", "component://x/Send.groovy"),
            super::ServiceKind::Unsupported
        );
    }

    /// A service file that does not parse, or an include that cannot be resolved, loses only its own services; the
    /// other files still register.
    #[test]
    fn a_malformed_service_file_loses_only_its_own_services() {
        let broken = write_fixture("broken", r#"<services><service verb="bad" noun="File">"#);
        let good = write_fixture(
            "good-neighbor",
            r#"<services>
                <service verb="plain" noun="Neighbor"/>
                <service-include verb="lost" noun="Include" location="component://no-such-component/service/X.xml"/>
            </services>"#,
        );
        let (defs, failures) = super::definitions_in(&[broken.clone(), good.clone()]).unwrap();
        let _ = std::fs::remove_file(&broken);
        let _ = std::fs::remove_file(&good);
        let text = failures.join("\n");
        assert!(text.contains("could not be read"), "{text}");
        assert!(text.contains("could not be resolved"), "{text}");
        assert!(
            defs.iter().any(|def| def.name.ends_with("plain#Neighbor")),
            "{failures:?}"
        );
    }

    #[test]
    fn rust_service_with_a_bad_location_stays_out_and_keeps_the_neighbor() {
        let ctx = crate::context::ExecutionContextFactory::test_context();
        let _entered = ctx.enter();
        let path = write_fixture(
            "rust",
            r#"<services>
                <service verb="missing" noun="Location" type="rust"/>
                <service verb="unknown" noun="Location" type="rust" location="moqui_core::service::mail::no_such"/>
                <service verb="has" noun="Actions" type="rust" location="moqui_core::service::mail::send_template">
                    <actions><set field="x" value="y"/></actions>
                </service>
                <service verb="plain" noun="Neighbor"/>
            </services>"#,
        );
        let failures = super::lint(&[path]).unwrap();
        let text = failures.join("\n");
        assert!(text.contains("missing#Location"), "{text}");
        assert!(text.contains("missing a location"), "{text}");
        assert!(text.contains("unknown#Location"), "{text}");
        assert!(text.contains("is not registered"), "{text}");
        assert!(text.contains("has#Actions"), "{text}");
        assert!(text.contains("has actions"), "{text}");
        assert!(super::is_registered("plain#Neighbor"), "{text}");
        assert!(!super::is_registered("missing#Location"));
        assert!(!super::is_registered("unknown#Location"));
        assert!(!super::is_registered("has#Actions"));
    }

    #[test]
    fn groovy_leftover_fails_that_service_and_keeps_the_neighbor() {
        let ctx = crate::context::ExecutionContextFactory::test_context();
        let _entered = ctx.enter();
        let path = write_fixture(
            "groovy",
            r#"<services>
                <service verb="groovy" noun="Broken">
                    <actions><set field="x" from="a ?: b"/></actions>
                </service>
                <service verb="plain" noun="Neighbor">
                    <in-parameters><parameter name="ok" default-value="yes"/></in-parameters>
                </service>
            </services>"#,
        );
        let failures = super::lint(&[path]).unwrap();
        let text = failures.join("\n");
        assert!(text.contains("groovy#Broken"), "{text}");
        assert!(text.contains("?:"), "{text}");
        assert!(super::is_registered("plain#Neighbor"), "{text}");
        assert!(!super::is_registered("groovy#Broken"), "{text}");
    }

    #[test]
    fn remote_json_rpc_is_not_registered() {
        let ctx = crate::context::ExecutionContextFactory::test_context();
        let _entered = ctx.enter();
        let path = write_fixture(
            "rpc",
            r#"<services>
                <service verb="remote" noun="Rpc" type="remote-json-rpc"/>
                <service verb="plain" noun="Neighbor"/>
            </services>"#,
        );
        let failures = super::lint(&[path]).unwrap();
        let text = failures.join("\n");
        assert!(text.contains("JSON-RPC"), "{text}");
        assert!(!super::is_registered("remote#Rpc"));
        assert!(super::is_registered("plain#Neighbor"));
    }

    #[test]
    fn load_definitions_registers_email_services() {
        let ctx = crate::context::ExecutionContextFactory::test_context_in(
            "",
            crate::service::ServicePools::idle(),
            crate::context::framework_dir(),
        );
        let _entered = ctx.enter();
        super::load_definitions().unwrap();
        assert!(
            super::is_registered("org.moqui.EmailServices.send#EmailTemplate"),
            "{}",
            explain("Email")
        );
        assert!(
            super::is_registered("org.moqui.impl.EmailServices.poll#EmailServer"),
            "{}",
            explain("poll")
        );
    }

    fn explain(needle: &str) -> String {
        super::snapshot(&super::entered().unwrap())
            .failures
            .iter()
            .filter(|line| line.contains(needle))
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A framework directory whose `service/test/lazy` holds `files`, found by service name.
    fn lazy_framework(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("moqui-svc-lazy-{name}-{}", std::process::id()));
        let dir = root.join("service/test/lazy");
        std::fs::create_dir_all(&dir).unwrap();
        for (file, body) in files {
            std::fs::write(dir.join(file), body).unwrap();
        }
        root
    }

    fn lazy_context(root: PathBuf) -> crate::context::ExecutionContext {
        crate::context::ExecutionContextFactory::test_context_in("", crate::service::ServicePools::idle(), root)
    }

    fn parsed_count(file: &str) -> usize {
        super::PARSED.with(|parsed| parsed.borrow().iter().filter(|path| path.ends_with(file)).count())
    }

    #[test]
    fn a_file_found_by_name_is_read_alone_and_merged_with_the_files_before() {
        let root = lazy_framework(
            "merge",
            &[
                (
                    "A.xml",
                    r#"<services>
                        <service verb="run" noun="One">
                            <implements service="test.lazy.C.define#Shape"/>
                            <actions><set field="x" value="1"/></actions>
                        </service>
                        <service verb="run" noun="Base"><in-parameters><parameter name="fromBase"/></in-parameters></service>
                    </services>"#,
                ),
                (
                    "B.xml",
                    r#"<services>
                        <service verb="run" noun="Two">
                            <implements service="test.lazy.A.run#Base"/>
                            <actions><set field="y" value="2"/></actions>
                        </service>
                    </services>"#,
                ),
                (
                    "C.xml",
                    r#"<services>
                        <service verb="define" noun="Shape" type="interface">
                            <in-parameters><parameter name="fromShape"/></in-parameters>
                        </service>
                    </services>"#,
                ),
            ],
        );
        let ctx = lazy_context(root);
        let _entered = ctx.enter();
        super::PARSED.with(|parsed| parsed.borrow_mut().clear());

        // A's run#One implements a service of a file not read yet: that file is read on demand.
        assert!(super::find("test.lazy.A.run#Base").is_some());
        let two = super::find("test.lazy.B.run#Two").expect("B merged");
        assert!(
            two.in_params.iter().any(|param| param.name == "fromBase"),
            "implements across files"
        );
        assert_eq!(parsed_count("A.xml"), 1, "A is not read again when B is merged");
        assert_eq!(parsed_count("B.xml"), 1);

        assert!(super::find("test.lazy.C.define#Shape").is_some());
        let one = super::find("test.lazy.A.run#One").expect("resolved once C is read");
        assert!(one.in_params.iter().any(|param| param.name == "fromShape"));
        assert_eq!(parsed_count("C.xml"), 1, "C is read once, on demand");
        assert_eq!(parsed_count("A.xml"), 1);
        assert_eq!(parsed_count("B.xml"), 1);
        assert!(super::registration_failures("test.lazy.A.run#One").is_empty());
    }

    #[test]
    fn two_threads_loading_different_files_both_register() {
        let files: Vec<(String, String)> = (0..4)
            .map(|index| {
                (
                    format!("T{index}.xml"),
                    format!(
                        r#"<services><service verb="run" noun="Some"><actions><set field="n" value="{index}"/></actions></service></services>"#
                    ),
                )
            })
            .collect();
        let borrowed: Vec<(&str, &str)> = files
            .iter()
            .map(|(file, body)| (file.as_str(), body.as_str()))
            .collect();
        let ctx = lazy_context(lazy_framework("threads", &borrowed));
        let factory = std::sync::Arc::clone(ctx.factory());
        let start = std::sync::Arc::new(std::sync::Barrier::new(files.len()));
        let threads: Vec<_> = (0..files.len())
            .map(|index| {
                let factory = std::sync::Arc::clone(&factory);
                let start = std::sync::Arc::clone(&start);
                std::thread::spawn(move || {
                    let ec = factory.new_context();
                    let _entered = ec.enter();
                    start.wait();
                    super::find(&format!("test.lazy.T{index}.run#Some")).is_some()
                })
            })
            .collect();
        for thread in threads {
            assert!(thread.join().unwrap());
        }
        let _entered = ctx.enter();
        let registry = super::snapshot(&super::entered().unwrap());
        for index in 0..files.len() {
            assert!(
                registry.services.contains_key(&format!("test.lazy.T{index}.run#Some")),
                "T{index} kept"
            );
        }
    }

    /// A framework directory and two components, each with `service/test/ovr/Same.xml` holding `files`, with a context
    /// whose components are those two, in that order.
    fn override_context(name: &str, files: [&str; 3]) -> crate::context::ExecutionContext {
        let root = std::env::temp_dir().join(format!("moqui-svc-override-{name}-{}", std::process::id()));
        let mut components = Vec::new();
        for (index, body) in files.iter().enumerate() {
            let dir = root.join(["framework", "first", "second"][index]);
            std::fs::create_dir_all(dir.join("service/test/ovr")).unwrap();
            std::fs::write(dir.join("service/test/ovr/Same.xml"), body).unwrap();
            if index > 0 {
                components.push(crate::context::ComponentInfo {
                    name: ["framework", "first", "second"][index].to_string(),
                    version: String::new(),
                    location: dir,
                    depends_on: Vec::new(),
                });
            }
        }
        crate::context::ExecutionContextFactory::test_context_with_components(
            "",
            crate::service::ServicePools::idle(),
            root.join("framework"),
            components,
        )
    }

    fn service_with(verb: &str, location: &str) -> String {
        format!(
            r#"<service verb="{verb}" noun="Thing"><actions><set field="from" value="{location}"/></actions></service>"#
        )
    }

    /// Java's `findServiceNode`: of the files with the same path, the framework's then each component's in order, the
    /// last that has the service defines it, whichever file is read first.
    #[test]
    fn a_later_component_overrides_a_service_of_the_same_file_path() {
        let ctx = override_context(
            "lookup",
            [
                &format!(
                    "<services>{}{}</services>",
                    service_with("run", "framework"),
                    service_with("keep", "framework")
                ),
                &format!("<services>{}</services>", service_with("run", "first")),
                &format!("<services>{}</services>", service_with("other", "second")),
            ],
        );
        let _entered = ctx.enter();
        let defined_in = |name: &str| super::find(name).map(|def| def.actions.clone()).unwrap_or_default();
        assert!(defined_in("test.ovr.Same.run#Thing").contains("first"));
        assert!(defined_in("test.ovr.Same.keep#Thing").contains("framework"));
        assert!(defined_in("test.ovr.Same.other#Thing").contains("second"));
        // read as a configured file, the framework's, the component's still wins
        let framework_file = ctx.factory().framework_dir().join("service/test/ovr/Same.xml");
        assert!(super::lint(&[framework_file]).unwrap().is_empty());
        assert!(defined_in("test.ovr.Same.run#Thing").contains("first"));
        // and every component's service files are known names, read or not
        let names = crate::service::info::known_service_names();
        assert!(
            names.iter().any(|name| name == "test.ovr.Same.other#Thing"),
            "{names:?}"
        );
    }

    /// As Java's `Integer.valueOf` and `Long.parseLong`, a numeric attribute that is not a number fails the service.
    #[test]
    fn a_timeout_that_is_not_a_number_fails_the_service() {
        let ctx = crate::context::ExecutionContextFactory::test_context();
        let _entered = ctx.enter();
        let path = write_fixture(
            "timeout",
            r#"<services>
                <service verb="slow" noun="Words" transaction-timeout="ten"/>
                <service verb="slow" noun="Sleep" semaphore="wait" semaphore-sleep="1s"/>
                <service verb="zero" noun="Timeout" transaction-timeout="0"/>
            </services>"#,
        );
        let failures = super::lint(&[path]).unwrap().join("\n");
        assert!(
            failures.contains("slow#Words transaction-timeout ten is not a whole number"),
            "{failures}"
        );
        assert!(
            failures.contains("slow#Sleep semaphore-sleep 1s is not a whole number"),
            "{failures}"
        );
        assert!(!super::is_registered("slow#Words"));
        assert!(super::find("zero#Timeout").is_some_and(|def| def.timeout.is_none()));
    }

    /// `moqui lint <file>`: a service implementing an interface of another file (the framework's EmailServices.xml)
    /// resolves it by name as a server does, and only the file's own failures are reported, not those of the files
    /// read for its interfaces; an interface that is nowhere is still a failure.
    #[test]
    fn check_of_one_file_resolves_interfaces_of_other_files() {
        let ctx = crate::context::ExecutionContextFactory::test_context_in(
            "",
            crate::service::ServicePools::idle(),
            crate::context::framework_dir(),
        );
        let _entered = ctx.enter();
        let path = write_fixture(
            "lint-interfaces",
            r#"<services>
                <service verb="process" noun="UnsubscribeEmail">
                    <implements service="org.moqui.EmailServices.process#EmailEca"/>
                    <actions><script>const subject = fields?.subject</script></actions>
                </service>
                <service verb="process" noun="Nothing">
                    <implements service="org.moqui.EmailServices.no#SuchInterface"/>
                </service>
            </services>"#,
        );
        let failures = super::check(std::slice::from_ref(&path)).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(
            failures[0].contains("process#Nothing implements unknown service org.moqui.EmailServices.no#SuchInterface"),
            "{failures:?}"
        );
    }

    fn write_fixture(name: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("moqui-svc-{name}-{}.xml", std::process::id()));
        std::fs::write(&path, body).unwrap();
        path
    }
}
