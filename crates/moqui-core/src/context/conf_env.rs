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

//! Conf attributes set from the environment by their path (plans/archive/Conf-Paths.md §3): each name of the form
//! `moqui_conf__webapp_list__webapp__WEBROOT__http_port` in the process properties (`-D`), the OS environment or
//! `runtime/.env` sets, or adds, the attribute it names in the merged conf, over every conf file. A secret attribute
//! the conf leaves at `CHANGEME` is generated into the seal's store (`seal.json` or a `SecretValue` row), never into
//! `runtime/.env` (plans/Sealed-Secrets.md S4, `crate::seal::secrets`).

use std::path::Path;

use moqui_lib::env;
use moqui_lib::node::path::{self, AttrPath};
use moqui_lib::node::MNode;

use super::conf_schema;
use super::Error;

/// Where an override came from, in the order they win.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    /// A process property: `-D` on the command line, or one the server or a test set.
    Property,
    /// The OS environment.
    Environment,
    /// `runtime/.env`.
    DotEnv,
}

impl Source {
    /// Where it came from, for a log line and the conf view.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Property => "-D",
            Self::Environment => "the environment",
            Self::DotEnv => "runtime/.env",
        }
    }
}

/// What one environment name did to the conf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Set an attribute the conf files have.
    Set,
    /// Added an attribute the conf files do not have; `element_existed` when its element came from the files (most often
    /// a misspelled attribute), not made for this name.
    Added { element_existed: bool },
    /// Named an attribute another name already set: that name. `same_source` when both came from one source (two
    /// spellings of one name), which is likely a mistake; a lower source's name for the same attribute is not.
    Shadowed { by: String, same_source: bool },
    /// Named nothing that can be set: why.
    Refused(String),
}

/// One environment name of the conf, as applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Override {
    /// The name as given.
    pub name: String,
    pub source: Source,
    /// The attribute's selector (`webapp-list/webapp[webroot]@http-port`), unless refused.
    pub selector: Option<String>,
    /// Whether the attribute holds a secret, so its value is never shown.
    pub secret: bool,
    pub outcome: Outcome,
}

impl Override {
    /// Whether this name's value is in the conf.
    #[must_use]
    pub fn applied(&self) -> bool {
        matches!(self.outcome, Outcome::Set | Outcome::Added { .. })
    }
}

/// The environment names of the conf with their values, highest source first and in name order within a source, each
/// with a value that is not empty (an empty value is not applied). The `.env` ones are read from `env_file`, the
/// runtime's own `runtime/.env`, at each call, so what a server wrote there since it was loaded is seen and another
/// runtime's (a test's factories in one process share the loaded table) is not; `None` takes the loaded table.
#[must_use]
pub fn candidates(env_file: Option<&Path>) -> Vec<(String, String, Source)> {
    let ours = |name: &str, value: &str| !value.is_empty() && path::env_segments(name).is_some();
    let mut found = Vec::new();
    let mut properties: Vec<(String, String)> = env::property_entries()
        .into_iter()
        .filter(|(name, value)| ours(name, value))
        .collect();
    properties.sort();
    found.extend(
        properties
            .into_iter()
            .map(|(name, value)| (name, value, Source::Property)),
    );
    let mut environment: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .filter(|(name, value)| ours(name, value))
        .collect();
    environment.sort();
    found.extend(
        environment
            .into_iter()
            .map(|(name, value)| (name, value, Source::Environment)),
    );
    let entries = match env_file {
        Some(file) => std::fs::read_to_string(file)
            .map(|text| env::parse_dotenv(&text))
            .unwrap_or_default(),
        None => env::dotenv_entries(),
    };
    // once the runtime has its seal (not the dev seal), a secret is not taken from .env, which the server's own user
    // can write
    // (plans/Sealed-Secrets.md S4; the boot's first merge, before the gate, is put right by seal::secrets::prepare_boot)
    let sealed = env_file
        .and_then(Path::parent)
        .and_then(crate::seal::keeper_of)
        .is_some_and(|keeper| keeper.refuses_dotenv_secrets());
    let mut dotenv: Vec<(String, String)> = entries
        .into_iter()
        .filter(|(name, value)| ours(name, value) && !(sealed && env_name_secret(name)))
        .collect();
    dotenv.sort();
    found.extend(dotenv.into_iter().map(|(name, value)| (name, value, Source::DotEnv)));
    found
}

/// Apply the environment's names ([`candidates`], with the runtime's `env_file`) to the merged `conf`. Writes nothing
/// to disk and logs nothing: the caller logs the result once ([`log_overrides`]).
#[must_use]
pub fn apply_overrides(conf: &MNode, env_file: Option<&Path>) -> Vec<Override> {
    apply_from(conf, candidates(env_file))
}

/// [`apply_overrides`] with the names given, highest first.
#[must_use]
pub fn apply_from(conf: &MNode, names: Vec<(String, String, Source)>) -> Vec<Override> {
    let mut out: Vec<Override> = Vec::with_capacity(names.len());
    // the attributes set so far, by a key that compares as names do, with the name and source that set each
    let mut set: Vec<(String, String, Source)> = Vec::new();
    for (name, value, source) in names {
        let Some(segments) = path::env_segments(&name) else {
            continue;
        };
        let target = match path::resolve_env(conf, &segments, &conf_schema::keying, true) {
            Ok(target) => target,
            Err(error) => {
                out.push(Override {
                    name,
                    source,
                    selector: None,
                    secret: false,
                    outcome: Outcome::Refused(error.to_string()),
                });
                continue;
            }
        };
        let selector = target.path.selector();
        let secret = attribute_secret(&target.path);
        let key = compare_key(&target.path);
        if let Some((by, by_source)) = set
            .iter()
            .find(|(held, ..)| *held == key)
            .map(|(_, by, by_source)| (by.clone(), *by_source))
        {
            out.push(Override {
                name,
                source,
                selector: Some(selector),
                secret,
                outcome: Outcome::Shadowed {
                    by,
                    same_source: by_source == source,
                },
            });
            continue;
        }
        target.node.set_attribute(target.attribute.clone(), value);
        set.push((key, name.clone(), source));
        let outcome = if target.attribute_existed {
            Outcome::Set
        } else {
            Outcome::Added {
                element_existed: !target.created,
            }
        };
        out.push(Override {
            name,
            source,
            selector: Some(selector),
            secret,
            outcome,
        });
    }
    out
}

/// The environment names the boot's merge took into a factory's conf, each with its source and selector
/// (plans/Runtime-Config.md §8.2): kept so that what set a value can be told later (the listeners' upgrade warning, where
/// each listener setting comes from), and what outranks a value [`super::conf_reload::set`] keeps.
struct BootOverrides(Vec<Override>);

/// Keep `overrides`, what the boot's merge did with each environment name, on `factory` ([`boot_overrides`]); the
/// boot calls it once.
pub(crate) fn keep_boot_overrides(factory: &super::ExecutionContextFactory, overrides: Vec<Override>) {
    factory.extension(move |_| BootOverrides(overrides));
}

/// What the boot's merge did with each environment name of `factory`'s conf, highest source first: set or added an
/// attribute, shadowed by a higher name, or refused. Empty for a factory not booted from the conf files (a test's).
#[must_use]
pub fn boot_overrides(factory: &super::ExecutionContextFactory) -> Vec<Override> {
    factory
        .extension_if_present::<BootOverrides>()
        .map(|kept| kept.0.clone())
        .unwrap_or_default()
}

/// The sources whose names set or named the attribute at `path` at the boot, each once, highest first: a `-D` that set
/// it, then the environment when it names it too, and so on. A refused name names nothing.
#[must_use]
pub fn boot_sources(factory: &super::ExecutionContextFactory, path: &AttrPath) -> Vec<Source> {
    let Some(kept) = factory.extension_if_present::<BootOverrides>() else {
        return Vec::new();
    };
    let key = compare_key(path);
    let mut sources = Vec::new();
    for item in &kept.0 {
        if matches!(item.outcome, Outcome::Refused(_)) || sources.contains(&item.source) {
            continue;
        }
        let names = item
            .selector
            .as_deref()
            .and_then(|selector| AttrPath::parse(selector).ok())
            .is_some_and(|named| compare_key(&named) == key);
        if names {
            sources.push(item.source);
        }
    }
    sources
}

/// A path as names compare: each element, key and the attribute normalized; two paths that name one attribute have one
/// key.
#[must_use]
pub fn compare_key(path: &AttrPath) -> String {
    let mut out = String::new();
    for step in &path.steps {
        out.push_str(&path::normalize(&step.element));
        for key in &step.keys {
            out.push('[');
            out.push_str(&path::normalize(key));
        }
        out.push('/');
    }
    out.push_str(&path::normalize(&path.attribute));
    out
}

/// Whether two selectors name one attribute, however each is spelled ([`compare_key`]); text that is no selector names
/// only itself.
#[must_use]
pub fn same_selector(first: &str, second: &str) -> bool {
    let key = |selector: &str| AttrPath::parse(selector).ok().map(|path| compare_key(&path));
    first == second || key(first).is_some_and(|first| key(second) == Some(first))
}

/// The `webroot` webapp's path, for a selector of one of its settings (`{WEBROOT}@http-port`).
pub const WEBROOT: &str = "webapp-list/webapp[webroot]";
/// The instance's purpose (production, dev, test): what much of the server's behavior turns on.
pub const PURPOSE: &str = "instance@purpose";
/// The instance's id, when the conf names one; else the cluster member keeps its own (`runtime/db/instance_id`).
pub const INSTANCE_ID: &str = "instance@id";
/// The zone of a session with no user zone, and of the server's own times.
pub const TIME_ZONE: &str = "instance@default-time-zone";

/// The canonical environment name of `selector` (`moqui_conf__instance__purpose`); empty when it does not parse or
/// has a key with no letter or digit.
#[must_use]
pub fn env_name_of(selector: &str) -> String {
    AttrPath::parse(selector)
        .ok()
        .and_then(|path| path.env_name())
        .unwrap_or_default()
}

/// [`setting`] of the instance's purpose ([`PURPOSE`]).
#[must_use]
pub fn instance_purpose() -> String {
    setting(PURPOSE)
}

/// A conf attribute by its selector (`ssh-server@port`), trimmed, for code with no factory at hand: a process property
/// (`-D`) or OS environment variable of the attribute's canonical environment name, which an override sets it to
/// over every conf file; else the current execution context's factory's running conf; else that of the server factory
/// this process booted last; else the loaded `runtime/.env`'s value of the name (a command before any boot). Empty
/// when not set. A secret is answered too: this is for the server's own code, never a script's
/// (`ec.factory.getConfValue`).
#[must_use]
pub fn setting(selector: &str) -> String {
    let name = AttrPath::parse(selector).ok().and_then(|path| path.env_name());
    if let Some(name) = &name {
        let given = env::get_property(name).or_else(|| std::env::var(name).ok().filter(|value| !value.is_empty()));
        if let Some(value) = given {
            return value.trim().to_string();
        }
    }
    let of = |factory: &super::ExecutionContextFactory| {
        factory
            .conf_value(selector)
            .map(|found| found.value.trim().to_string())
            .unwrap_or_default()
    };
    if let Some(value) = super::ExecutionContext::with_current(|ec| of(ec.factory())) {
        return value;
    }
    if let Some(factory) = super::ExecutionContextFactory::running() {
        return of(&factory);
    }
    name.map(|name| env::get_prop_or_env(&name).trim().to_string())
        .unwrap_or_default()
}

/// Keep settings at run time, as pairing does: each named by a conf selector, or by a property name. A secret selector
/// ([`attribute_secret`]) goes to the seal's store of the runtime `env_path` is in (`seal.json` or a `SecretValue` row,
/// plans/Sealed-Secrets.md S4), never to `.env` or a process property; any other setting is kept in `env_path`
/// (`runtime/.env`) under its environment name, and a selector's value is also set as the process property of that
/// name, which [`setting`] and the scripts' `getConfValue` read first, so it holds at once, before any conf reload; a
/// removed one is cleared.
///
/// # Errors
///
/// When the file cannot be written, or a secret cannot be kept (no running server of that runtime, or no seal).
pub fn keep_in_env(env_path: &Path, set: &[(String, String)], remove: &[&str]) -> Result<(), Error> {
    // a test's factory has no seal: its secrets stay where they were kept before (the file and a process property)
    let test_factory = super::ExecutionContext::with_current(|ec| ec.factory().keeps_secrets_in_env()).unwrap_or(false);
    let secret = |given: &str| given.contains('@') && selector_secret(given) && !test_factory;
    let name = |given: &str| {
        if given.contains('@') {
            env_name_of(given)
        } else {
            given.to_string()
        }
    };
    let secrets: Vec<&(String, String)> = set.iter().filter(|(given, _)| secret(given)).collect();
    let removed_secrets: Vec<&str> = remove.iter().copied().filter(|given| secret(given)).collect();
    if !secrets.is_empty() || !removed_secrets.is_empty() {
        let factory = runtime_factory(env_path)
            .filter(|factory| factory.seal_keeper().is_some())
            .ok_or_else(|| {
                Error::new(format!(
                    "no running server of {} to keep its secrets in (plans/Sealed-Secrets.md S4)",
                    env_path.display()
                ))
            })?;
        for (given, value) in &secrets {
            crate::seal::secrets::put(&factory, given, value)?;
        }
        for given in &removed_secrets {
            crate::seal::secrets::remove(&factory, given)?;
        }
        // a line an earlier build kept for one of them in .env would win over the store at a seal-less boot: it goes
        let stale: Vec<String> = secrets
            .iter()
            .map(|(given, _)| given.as_str())
            .chain(removed_secrets.iter().copied())
            .map(env_name_of)
            .filter(|name| !name.is_empty())
            .collect();
        if !stale.is_empty() && env_path.is_file() {
            let stale: Vec<&str> = stale.iter().map(String::as_str).collect();
            env::update_dotenv_file(env_path, &[], &stale)?;
        }
    }
    let pairs: Vec<(String, String)> = set
        .iter()
        .filter(|(given, _)| !secret(given))
        .map(|(given, value)| (name(given), value.clone()))
        .collect();
    let removed: Vec<String> = remove
        .iter()
        .filter(|given| !secret(given))
        .map(|given| name(given))
        .collect();
    if !pairs.is_empty() || !removed.is_empty() {
        let removed_refs: Vec<&str> = removed.iter().map(String::as_str).collect();
        env::update_dotenv_file(env_path, &pairs, &removed_refs)?;
    }
    for (given, value) in set {
        if given.contains('@') && !secret(given) {
            env::set_property(&name(given), value.clone());
        }
    }
    for given in remove {
        if given.contains('@') {
            env::clear_property(&name(given));
        }
    }
    Ok(())
}

/// The factory of the runtime whose `.env` is `env_path`: the current execution context's, else the server factory
/// this process booted last, when its runtime is that one.
fn runtime_factory(env_path: &Path) -> Option<std::sync::Arc<super::ExecutionContextFactory>> {
    let runtime = env_path.parent()?;
    let same = |factory: &super::ExecutionContextFactory| {
        let ours = factory.runtime_dir();
        ours == runtime || std::fs::canonicalize(runtime).is_ok_and(|canonical| canonical == ours)
    };
    super::ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
        .filter(|factory| same(factory))
        .or_else(|| super::ExecutionContextFactory::running().filter(|factory| same(factory)))
}

/// The handle of the secret conf attribute `selector` (plans/Sealed-Secrets.md §3.1, Secret handles), for code with no
/// factory at hand: a process property or OS environment variable of its environment name (an operator's, or a
/// test's), else the current execution context's factory's [`super::ExecutionContextFactory::secret`], else that of
/// the server factory this process booted last. `None` when none has it; an error of the store is logged.
#[must_use]
pub fn secret(selector: &str) -> Option<zeroize::Zeroizing<String>> {
    let name = env_name_of(selector);
    if !name.is_empty() {
        let given = env::get_property(&name).or_else(|| std::env::var(&name).ok().filter(|value| !value.is_empty()));
        if let Some(value) = given {
            return Some(zeroize::Zeroizing::new(value.trim().to_string()));
        }
    }
    let of = |factory: &super::ExecutionContextFactory| {
        factory.secret(selector).unwrap_or_else(|err| {
            tracing::warn!("The secret {selector} could not be read: {err}");
            None
        })
    };
    if let Some(found) = super::ExecutionContext::with_current(|ec| of(ec.factory())) {
        return found;
    }
    super::ExecutionContextFactory::running().and_then(|factory| of(&factory))
}

/// [`setting`] on `factory`: the process property or OS environment variable of the attribute's environment name, else
/// `factory`'s running conf. For code with its factory at hand, which must not read another factory's conf (a process
/// of several, a test's).
#[must_use]
pub fn setting_on(factory: &super::ExecutionContextFactory, selector: &str) -> String {
    let name = env_name_of(selector);
    if !name.is_empty() {
        let given = env::get_property(&name).or_else(|| std::env::var(&name).ok().filter(|value| !value.is_empty()));
        if let Some(value) = given {
            return value.trim().to_string();
        }
    }
    factory
        .conf_value(selector)
        .map(|found| found.value.trim().to_string())
        .unwrap_or_default()
}

/// The attribute of `factory`'s conf at `selector` as code reads it now: a process property of its environment name
/// first (set after the boot, as a test or a spec does; at the boot one is applied into the conf), else the running
/// conf's; `None` when neither has it. What `ec.factory.getConfValue` answers.
#[must_use]
pub fn conf_value_now(factory: &super::ExecutionContextFactory, selector: &str) -> Option<ConfValue> {
    let path = AttrPath::parse(selector).ok()?;
    let given = path.env_name().and_then(|name| env::get_property(&name));
    match given {
        Some(value) => Some(ConfValue {
            value,
            secret: attribute_secret(&path),
        }),
        None => factory.conf_value(selector),
    }
}

/// The attribute of the merged `conf` at `selector`, trimmed; empty when it has none: for code that reads the conf
/// before the factory is built (a `before_init` hook such as the port check).
#[must_use]
pub fn conf_attribute(conf: &MNode, selector: &str) -> String {
    AttrPath::parse(selector)
        .ok()
        .and_then(|path| path::resolve(conf, &path, &conf_schema::keying, false).ok())
        .and_then(|target| target.node.attribute(&target.attribute))
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

/// A name of a table that lists settings: a conf selector (it has an `@`) read by [`setting`], else a process property:
/// the state the server sets as it runs, which is not configuration (the XMPP server's resolved addresses, a cluster
/// member's SSH address; plans/archive/Conf-Paths.md, Decisions). For the name tables and the lookup closures that read both
/// (the edge's own ports, the protocol facts, the SIP and media settings); a selector's own text is its name in their
/// messages.
#[must_use]
pub fn setting_or_property(name: &str) -> String {
    if name.contains('@') {
        setting(name)
    } else {
        env::get_prop_or_env(name).trim().to_string()
    }
}

/// Whether a listener's switch (`http-listen-enabled`, plans/Runtime-Config.md §8.2) is on: `true`, trimmed and in any
/// case. Anything else, empty included, is off.
#[must_use]
pub fn switch_on(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("true")
}

/// [`setting`] of an attribute of the `webroot` webapp (`http-host`).
#[must_use]
pub fn webroot_setting(attribute: &str) -> String {
    setting(&format!("{WEBROOT}@{attribute}"))
}

/// An attribute of the running conf, as [`attribute_values`] keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfValue {
    pub value: String,
    /// Whether it holds a secret ([`attribute_secret`]), which is never shown or given to a script.
    pub secret: bool,
}

/// Every attribute of the conf in `text` by [`compare_key`] of its path, so a selector finds it however it spells names
/// and keys; of two elements a single unit has, the first one's, as a selector reads it. An attribute of an element in
/// a list with no key has no path and is not here.
#[must_use]
pub fn attribute_values(text: &str) -> std::collections::HashMap<String, ConfValue> {
    let mut out = std::collections::HashMap::new();
    if let Ok(conf) = MNode::parse_shared("moqui-conf", text) {
        collect_values(&conf, &mut out);
    }
    out
}

fn collect_values(node: &MNode, out: &mut std::collections::HashMap<String, ConfValue>) {
    for child in node.children() {
        child.for_each_attribute(|name, value| {
            if let Some(path) = AttrPath::of(&child, name, &conf_schema::keying) {
                out.entry(compare_key(&path)).or_insert_with(|| ConfValue {
                    value: value.to_string(),
                    secret: attribute_secret(&path),
                });
            }
        });
        collect_values(&child, out);
    }
}

/// Whether the attribute at `path` holds a secret: the schema marks it, or its name says so.
#[must_use]
pub fn attribute_secret(path: &AttrPath) -> bool {
    let names: Vec<&str> = path.steps.iter().map(|step| step.element.as_str()).collect();
    conf_schema::attribute_secret(&names, &path.attribute)
}

/// Whether `name` is the environment name of a secret conf attribute (`moqui_conf__entity_facade__crypt_pass`), by the
/// conf schema or by its attribute's name ([`attribute_secret`]); its segments split as the schema keys them, with no
/// conf tree needed. False for any other name.
#[must_use]
pub fn env_name_secret(name: &str) -> bool {
    env_name_path(name).is_some_and(|path| attribute_secret(&path))
}

/// The attribute the environment name `name` sets, in any of its spellings, as the conf merge resolves it ([`apply_from`]),
/// its segments split as the schema keys them, with no conf tree needed; `None` for a name that is not one of the conf.
/// Two names of one attribute have paths with one [`compare_key`].
#[must_use]
pub fn env_name_path(name: &str) -> Option<AttrPath> {
    let segments = path::env_segments(name)?;
    let scratch = MNode::new("moqui-conf");
    path::resolve_env(&scratch, &segments, &conf_schema::keying, true)
        .ok()
        .map(|target| target.path)
}

/// Whether `selector` names a secret attribute, by the conf schema or by its name ([`attribute_secret`]); true for
/// text that is no selector, so a caller refusing secrets refuses it too.
#[must_use]
pub fn selector_secret(selector: &str) -> bool {
    AttrPath::parse(selector).map_or(true, |path| attribute_secret(&path))
}

/// Whether `selector` names an attribute the conf schema itself marks secret: this server's own keys and the
/// framework's credentials (the crypt pass, a session key, an LLM endpoint's key), not an add-on's attribute that is a
/// secret only by its name (mr-pass's `pass@client-secret`, a credential this server sends to another). True for text
/// that is no selector.
#[must_use]
pub fn declared_secret(selector: &str) -> bool {
    AttrPath::parse(selector).map_or(true, |path| {
        let names: Vec<&str> = path.steps.iter().map(|step| step.element.as_str()).collect();
        conf_schema::unit(&names).is_some_and(|unit| unit.secrets.iter().any(|(name, _)| *name == path.attribute))
    })
}

/// The conf attributes renamed away, each with the attribute that replaced it and a hint saying what to write there
/// (empty when the value carries over): the gRPC listener's port and address moved to the webroot webapp beside the
/// HTTP and HTTPS ports (`grpc-listen-port`, `grpc-listen-bind`), which every gRPC service shares, and the SSH login
/// dialog's flag became one of the SSH access modes (plans/Runtime-Config.md §3.4). Nothing reads the old ones, so a
/// conf that sets one is refused rather than silently ignored.
pub const RETIRED_ATTRIBUTES: &[(&str, &str, &str)] = &[
    (
        "pass-authority@grpc-host",
        "webapp-list/webapp[webroot]@grpc-listen-bind",
        "",
    ),
    (
        "pass-authority@grpc-port",
        "webapp-list/webapp[webroot]@grpc-listen-port",
        "",
    ),
    (
        "telemetry/receiver@grpc-port",
        "webapp-list/webapp[webroot]@grpc-listen-port",
        "",
    ),
    (
        "telemetry/receiver@grpc-bind",
        "webapp-list/webapp[webroot]@grpc-listen-bind",
        "",
    ),
    (
        "ssh-server@login-dialog",
        "ssh-server@access",
        "true is access=\"open\", false is access=\"login\"",
    ),
];

/// Why setting the attribute at `path` is refused, when it is a renamed one ([`RETIRED_ATTRIBUTES`]): the message
/// names it and its environment name, and its replacement, its own and the hint; `None` for any other attribute.
#[must_use]
pub fn retired_attribute(path: &AttrPath) -> Option<String> {
    let key = compare_key(path);
    RETIRED_ATTRIBUTES.iter().find_map(|(retired, replacement, hint)| {
        let retired_path = AttrPath::parse(retired).ok()?;
        (compare_key(&retired_path) == key).then(|| retired_attribute_message(retired, replacement, hint))
    })
}

/// The message for a renamed attribute `retired` that is set: its replacement, both environment names and the hint.
fn retired_attribute_message(retired: &str, replacement: &str, hint: &str) -> String {
    let hint = if hint.is_empty() {
        String::new()
    } else {
        format!(" ({hint})")
    };
    format!(
        "{retired} ({}) is renamed: set {replacement} ({}) instead{hint}",
        env_name_of(retired),
        env_name_of(replacement)
    )
}

/// A conf value retired while its attribute stays (plans/Runtime-Config.md §3.2): the attribute's selector, the values
/// no longer read (trimmed and in lower case, `""` for empty), the attribute that replaced them, and a hint saying what
/// to write.
pub type RetiredValue = (&'static str, &'static [&'static str], &'static str, &'static str);

/// The conf values retired, each refused by name wherever it is set: in a conf file, by its environment name
/// ([`refuse_retired`]) or while running ([`super::conf_reload::set`]). The listeners' old spellings of "off" (a port of
/// `off` or empty, plans/Runtime-Config.md §8.2) join it as each family gets its `enabled` switch.
pub const RETIRED_VALUES: &[RetiredValue] = &[
    (
        "webapp-list/webapp[webroot]@http-listen-port",
        &["off", ""],
        "webapp-list/webapp[webroot]@http-listen-enabled",
        "false keeps it closed; otherwise write the port, 8080 by default: http-listen-port no longer follows http-port",
    ),
    (
        "webapp-list/webapp[webroot]@https-listen-port",
        &["off", ""],
        "webapp-list/webapp[webroot]@https-listen-enabled",
        "false keeps it closed; otherwise write the port, 8443 by default",
    ),
    (
        "webapp-list/webapp[webroot]@grpc-listen-port",
        &["off", ""],
        "webapp-list/webapp[webroot]@grpc-listen-enabled",
        "false keeps it closed; otherwise write the port, 4317 by default",
    ),
    (
        "ssh-server@port",
        &[""],
        "ssh-server@enabled",
        "false keeps it closed; otherwise write the port, 2222 by default",
    ),
    (
        "smtp-server@listen-port",
        &[""],
        "smtp-server@enabled",
        "false keeps it closed; otherwise write the port, 2525 by default",
    ),
    // an empty plain port of SIP, XMPP or TURN stays valid: that one transport off
    (
        "media-server@port",
        &[""],
        "media-server@enabled",
        "false keeps call media off; otherwise write the port, 10000 by default",
    ),
];

/// Why `value` of the attribute at `path` is refused, when it is a retired value ([`RETIRED_VALUES`]): the message names
/// the attribute and its environment name, and the replacement and its own; `None` for any other value.
#[must_use]
pub fn retired_value(path: &AttrPath, value: &str) -> Option<String> {
    retired_value_in(RETIRED_VALUES, path, value)
}

/// [`retired_value`] by the rows of `table`.
pub(crate) fn retired_value_in(table: &[RetiredValue], path: &AttrPath, value: &str) -> Option<String> {
    table.iter().find_map(|row| retired_row_message(row, path, value))
}

/// The message for `value` of the attribute at `path` when `row` retires it.
fn retired_row_message(row: &RetiredValue, path: &AttrPath, value: &str) -> Option<String> {
    let (selector, values, replacement, hint) = row;
    let retired = AttrPath::parse(selector).ok()?;
    let given = value.trim().to_ascii_lowercase();
    if compare_key(&retired) != compare_key(path) || !values.contains(&given.as_str()) {
        return None;
    }
    let shown = match value.trim() {
        "" => "empty".to_string(),
        trimmed => format!("\"{trimmed}\""),
    };
    let hint = if hint.is_empty() {
        String::new()
    } else {
        format!(" ({hint})")
    };
    Some(format!(
        "{} ({}) set {shown} is a retired value: set {replacement} ({}) instead{hint}",
        path.selector(),
        path.env_name().unwrap_or_default(),
        env_name_of(replacement)
    ))
}

/// Refuse a merged conf that still uses the retired properties (plans/archive/Conf-Paths.md, phase 7): a `default-property`,
/// which nothing reads, and an attribute holding `${`, which is not expanded (an `actions` block's XML actions are not
/// conf attributes and keep theirs); a renamed attribute ([`RETIRED_ATTRIBUTES`]); and a retired value
/// ([`RETIRED_VALUES`]); from a conf file or from its environment name, which the merged conf holds too. Each is logged,
/// naming its attribute and the environment name that sets it, and for a renamed one or a retired value its
/// replacement.
///
/// # Errors
///
/// When the conf has any.
pub fn refuse_retired(conf: &MNode) -> Result<(), Error> {
    refuse_retired_with(conf, RETIRED_VALUES)
}

/// [`refuse_retired`] with the retired values of `values`.
pub(crate) fn refuse_retired_with(conf: &MNode, values: &[RetiredValue]) -> Result<(), Error> {
    let mut found = Vec::new();
    for (retired, replacement, hint) in RETIRED_ATTRIBUTES {
        let set = AttrPath::parse(retired)
            .ok()
            .and_then(|path| path::resolve(conf, &path, &conf_schema::keying, false).ok())
            .is_some_and(|target| target.node.attribute(&target.attribute).is_some());
        if set {
            found.push(retired_attribute_message(retired, replacement, hint));
        }
    }
    for row in values {
        let Ok(path) = AttrPath::parse(row.0) else {
            continue;
        };
        let Ok(target) = path::resolve(conf, &path, &conf_schema::keying, false) else {
            continue;
        };
        let message = target
            .node
            .attribute(&target.attribute)
            .and_then(|value| retired_row_message(row, &target.path, &value));
        if let Some(message) = message {
            found.push(message);
        }
    }
    for node in conf.children_named("default-property") {
        found.push(format!(
            "<default-property name=\"{}\"> is not read: set the attribute it fed (`moqui conf env` lists each \
             attribute with its environment name)",
            node.attribute("name").unwrap_or_default()
        ));
    }
    expressions_in(conf, &mut found);
    if found.is_empty() {
        return Ok(());
    }
    for item in &found {
        tracing::error!("Conf: {item}");
    }
    Err(Error::new(format!(
        "the conf uses retired properties, attributes or values (plans/archive/Conf-Paths.md, docs/Run-Deploy.md): {}",
        found.join("; ")
    )))
}

/// Each attribute under `node` that holds `${`, outside `actions` blocks.
fn expressions_in(node: &MNode, found: &mut Vec<String>) {
    for child in node.child_iter() {
        if child.has_name("actions") || child.has_name("default-property") {
            continue;
        }
        for (attribute, value) in child.attributes() {
            if !value.contains("${") {
                continue;
            }
            let path = AttrPath::of(&child, &attribute, &conf_schema::keying);
            let selector = path
                .as_ref()
                .map_or_else(|| format!("{}@{attribute}", child.name()), AttrPath::selector);
            let env_name = path.as_ref().and_then(AttrPath::env_name).unwrap_or_default();
            found.push(format!(
                "{selector} holds a ${{}}, which is not expanded: write its value, or set it from the environment \
                 ({env_name})"
            ));
        }
        expressions_in(&child, found);
    }
}

/// Log what the environment did to the conf, once, at boot: each name set or added at INFO (never a secret's value),
/// an attribute added to an element of the files at WARN, a refused name and two spellings of one name at ERROR.
pub fn log_overrides(overrides: &[Override]) {
    for item in overrides {
        let Override {
            name,
            source,
            selector,
            outcome,
            ..
        } = item;
        let selector = selector.as_deref().unwrap_or("");
        let source = source.name();
        match outcome {
            Outcome::Set => tracing::info!("Conf override {name} from {source} sets {selector}"),
            Outcome::Added { element_existed: true } => tracing::warn!(
                "Conf override {name} from {source} adds {selector}, an attribute the conf files do not have: check the name"
            ),
            Outcome::Added { element_existed: false } => {
                tracing::info!("Conf override {name} from {source} adds {selector}");
            }
            Outcome::Shadowed { by, same_source: true } => tracing::error!(
                "Conf override {name} from {source} names {selector}, which {by} already sets; {name} is not used"
            ),
            Outcome::Shadowed { .. } => {}
            Outcome::Refused(why) => tracing::error!("Conf override {name} from {source} is not used: {why}"),
        }
    }
}

#[cfg(test)]
#[path = "conf_env_tests.rs"]
mod tests;
