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

//! Conf attributes set from the environment by their path (plans/Conf-Paths.md §3): each name of the form
//! `moqui_conf__webapp_list__webapp__WEBROOT__http_port` in the process properties (`-D`), the OS environment or
//! `runtime/.env` sets, or adds, the attribute it names in the merged conf, over every conf file. A secret attribute
//! the conf leaves at `CHANGEME` is generated and kept in `runtime/.env` under its canonical name.

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
    let mut dotenv: Vec<(String, String)> = entries.into_iter().filter(|(name, value)| ours(name, value)).collect();
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

/// A name of a table that lists settings: a conf selector (it has an `@`) read by [`setting`], else a property not yet
/// moved to the conf (plans/Conf-Paths.md §7), read as before. For the name tables (the edge's own ports, the protocol
/// facts) while the settings move; a selector's own text is its name in their messages.
#[must_use]
pub fn setting_or_property(name: &str) -> String {
    if name.contains('@') {
        setting(name)
    } else {
        env::get_prop_or_env(name).trim().to_string()
    }
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

/// Generate each secret attribute the schema marks generated whose value still ends in `CHANGEME` and no override
/// set, write each to `env_path` (`runtime/.env`) under its canonical name, and set it in `conf`. Answers the selectors
/// generated, after logging each.
///
/// # Errors
///
/// When random bytes cannot be read or `env_path` cannot be written: the server does not start with a secret it could
/// not keep.
pub fn generate_secrets(conf: &MNode, overrides: &[Override], env_path: &Path) -> Result<Vec<String>, Error> {
    let overridden: Vec<&str> = overrides
        .iter()
        .filter(|item| item.applied())
        .filter_map(|item| item.selector.as_deref())
        .collect();
    let mut pairs = Vec::new();
    let mut made = Vec::new();
    let mut nodes = Vec::new();
    collect_generated(conf, conf_schema::CATALOG, &mut nodes);
    for (node, attribute) in nodes {
        let value = node.attribute(attribute).unwrap_or_default();
        if !value.ends_with("CHANGEME") {
            continue;
        }
        let Some(path) = AttrPath::of(&node, attribute, &conf_schema::keying) else {
            continue;
        };
        let selector = path.selector();
        if overridden.contains(&selector.as_str()) {
            continue;
        }
        let Some(env_name) = path.env_name() else {
            tracing::error!("Conf secret {selector} is at CHANGEME and has no environment name to keep it under");
            continue;
        };
        let secret = super::factory::generate_secret()?;
        node.set_attribute(attribute, secret.clone());
        pairs.push((env_name, secret));
        made.push(selector);
    }
    if !pairs.is_empty() {
        env::update_dotenv_file(env_path, &pairs, &[])?;
        for selector in &made {
            tracing::info!("Generated conf secret {selector}, kept in runtime/.env");
        }
    }
    Ok(made)
}

/// Each element of `node`'s children that a unit of `units` marks with a generated secret attribute, with the
/// attribute, through every keyed element on the way.
fn collect_generated(node: &MNode, units: &'static [conf_schema::Unit], out: &mut Vec<(MNode, &'static str)>) {
    for unit in units {
        for child in node.children_named(unit.name) {
            for (attribute, mark) in unit.secrets {
                if *mark == conf_schema::Secret::Generated {
                    out.push((child.clone(), attribute));
                }
            }
            collect_generated(&child, unit.children, out);
        }
    }
}

#[cfg(test)]
#[path = "conf_env_tests.rs"]
mod tests;
