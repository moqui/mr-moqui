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

//! The settings that can be overridden (plans/archive/Monitor-Settings.md, Model: Catalog): each key in conf form
//! (`path@attr` or `path[selector]@attr`), its value type, and how its on-disk value is read. Nothing outside this list
//! can be overridden. The telemetry keys are left out of a build without the `telemetry` feature.

use std::sync::OnceLock;

use moqui_lib::node::MNode;

use super::log_filter::{self, Console};
use crate::context::ExecutionContextFactory;

/// The box a setting shows in on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Logging,
    Traces,
    Artifacts,
    Metrics,
    Operations,
    Browser,
    Receiver,
}

impl Group {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Logging => "logging",
            Self::Traces => "traces",
            Self::Artifacts => "artifacts",
            Self::Metrics => "metrics",
            Self::Operations => "operations",
            Self::Browser => "browser",
            Self::Receiver => "receiver",
        }
    }
}

/// What is rebuilt when a setting changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Applies {
    /// The process's log filter.
    Logging,
    /// The factory's live telemetry config.
    Telemetry,
    /// The `conf_setting` overlay, for settings read from the conf when used.
    Conf,
}

/// A setting's value type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A log level; TRACE only when `trace`.
    Level {
        trace: bool,
    },
    /// 0 to 1.
    Ratio,
    Whole {
        min: u64,
        max: u64,
    },
    Bool,
    Choice(&'static [&'static str]),
    /// A comma-separated list of attribute names, possibly empty.
    Attributes,
}

impl Kind {
    /// The values a drop-down offers, or none for a typed value.
    pub fn choices(self) -> Vec<&'static str> {
        match self {
            Self::Level { trace: true } => vec!["error", "warn", "info", "debug", "trace"],
            Self::Level { trace: false } => vec!["error", "warn", "info", "debug"],
            Self::Bool => vec!["true", "false"],
            Self::Choice(choices) => choices.to_vec(),
            Self::Ratio | Self::Whole { .. } | Self::Attributes => Vec::new(),
        }
    }

    /// The value in its canonical form, or why it is refused.
    pub fn parse(self, text: &str) -> Result<String, String> {
        let text = text.trim();
        match self {
            Self::Level { trace } => match log_filter::parse_level(text) {
                Some(tracing::Level::TRACE) if !trace => {
                    Err("trace is allowed only on a target rule; use debug here".to_string())
                }
                Some(level) => Ok(log_filter::level_name(level).to_string()),
                None => Err(format!(
                    "[{text}] is not a log level: error, warn, info, debug or trace"
                )),
            },
            Self::Ratio => match text.parse::<f64>() {
                Ok(value) if value.is_finite() && (0.0..=1.0).contains(&value) => Ok(value.to_string()),
                _ => Err(format!("[{text}] is not a ratio from 0 to 1")),
            },
            Self::Whole { min, max } => match text.parse::<u64>() {
                Ok(value) if (min..=max).contains(&value) => Ok(value.to_string()),
                _ => Err(format!("[{text}] is not a whole number from {min} to {max}")),
            },
            Self::Bool => match text {
                "true" | "false" => Ok(text.to_string()),
                _ => Err(format!("[{text}] is not true or false")),
            },
            Self::Choice(choices) => match choices.iter().find(|choice| **choice == text) {
                Some(choice) => Ok((*choice).to_string()),
                None => Err(format!("[{text}] is not one of {}", choices.join(", "))),
            },
            Self::Attributes => {
                let names: Vec<&str> = text.split(',').map(str::trim).filter(|name| !name.is_empty()).collect();
                if names.len() > 32 {
                    return Err("at most 32 attributes".to_string());
                }
                match names.iter().find(|name| !is_attribute_name(name)) {
                    Some(bad) => Err(format!(
                        "[{bad}] is not an attribute name (letters, digits, '_' and '.')"
                    )),
                    None => Ok(names.join(",")),
                }
            }
        }
    }
}

fn is_attribute_name(name: &str) -> bool {
    name.len() <= 128 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// A target rule's target: a module path or a dotted target.
pub fn is_target(target: &str) -> bool {
    !target.is_empty()
        && target.len() <= 200
        && target
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.'))
}

/// One setting.
#[derive(Clone, Debug)]
pub struct Entry {
    pub key: String,
    pub name: String,
    pub group: Group,
    pub kind: Kind,
    /// The value when the conf leaves the attribute out.
    pub default: &'static str,
}

impl Entry {
    fn new(key: impl Into<String>, name: impl Into<String>, group: Group, kind: Kind, default: &'static str) -> Self {
        Self {
            key: key.into(),
            name: name.into(),
            group,
            kind,
            default,
        }
    }
}

pub(crate) fn applies(key: &str) -> Applies {
    if key.starts_with("logging") {
        Applies::Logging
    } else if key.starts_with("telemetry/browser@") || key.starts_with("telemetry/receiver@") {
        Applies::Conf
    } else {
        Applies::Telemetry
    }
}

/// The artifact types of telemetry/artifact.
pub const ARTIFACT_TYPES: [&str; 7] = [
    "AT_XML_SCREEN",
    "AT_XML_SCREEN_CONTENT",
    "AT_XML_SCREEN_TRANS",
    "AT_SERVICE",
    "AT_REST_PATH",
    "AT_ENTITY",
    "AT_LLM",
];

/// The instruments of telemetry/metric.
pub const INSTRUMENTS: [&str; 6] = [
    "http.server.request.duration",
    "moqui.artifact.duration",
    "db.client.operation.duration",
    "moqui.job.runs",
    "moqui.operation.runs",
    "moqui.operation.duration",
];

const LOG_LEVEL: &str = "logging@level";
const CONSOLE_LEVEL: &str = "logging@console-level";

/// The fixed settings, in screen order. Target rules are not here: any valid target is one ([`lookup`]).
pub fn entries() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let mut entries = vec![
            Entry::new(
                LOG_LEVEL,
                "Global level",
                Group::Logging,
                Kind::Level { trace: false },
                "info",
            ),
            Entry::new(
                CONSOLE_LEVEL,
                "Console level",
                Group::Logging,
                Kind::Level { trace: true },
                "info",
            ),
        ];
        if cfg!(feature = "telemetry") {
            entries.push(Entry::new(
                "telemetry@sample-ratio",
                "Sample ratio",
                Group::Traces,
                Kind::Ratio,
                "0",
            ));
            entries.push(Entry::new(
                "telemetry@slow-root-ms",
                "Slow root (ms)",
                Group::Traces,
                Kind::Whole { min: 1, max: 3_600_000 },
                "1000",
            ));
            for kind in ARTIFACT_TYPES {
                let (spans, metrics) = default_artifact(kind);
                for (attr, label, default) in [
                    ("spans", "spans", if spans { "true" } else { "false" }),
                    ("metrics", "metrics", if metrics { "true" } else { "false" }),
                ] {
                    entries.push(Entry::new(
                        format!("telemetry/artifact[{kind}]@{attr}"),
                        format!("{kind} {label}"),
                        Group::Artifacts,
                        Kind::Bool,
                        default,
                    ));
                }
                entries.push(Entry::new(
                    format!("telemetry/artifact[{kind}]@sample-ratio"),
                    format!("{kind} sample ratio"),
                    Group::Artifacts,
                    Kind::Ratio,
                    "",
                ));
            }
            for name in INSTRUMENTS {
                entries.push(Entry::new(
                    format!("telemetry/metric[{name}]@attributes"),
                    name,
                    Group::Metrics,
                    Kind::Attributes,
                    "",
                ));
            }
            entries.push(Entry::new(
                "telemetry/operations@record",
                "Record",
                Group::Operations,
                Kind::Choice(&["default", "declared", "none"]),
                "default",
            ));
            entries.push(Entry::new(
                "telemetry/operations@max-names",
                "Max names",
                Group::Operations,
                Kind::Whole { min: 1, max: 100_000 },
                "500",
            ));
        }
        for (attr, name, kind, default) in [
            ("enabled", "RUM enabled", Kind::Bool, "false"),
            ("sample-ratio", "Sample ratio", Kind::Ratio, "1"),
            ("error-sample-ratio", "Error sample ratio", Kind::Ratio, "1"),
            ("replay-enabled", "Replay enabled", Kind::Bool, "false"),
            ("replay-sample-ratio", "Replay sample ratio", Kind::Ratio, "0.1"),
        ] {
            entries.push(Entry::new(
                format!("telemetry/browser@{attr}"),
                name,
                Group::Browser,
                kind,
                default,
            ));
        }
        for (attr, name, kind, default) in [
            ("enabled", "Enabled", Kind::Bool, "true"),
            (
                "requests-per-minute",
                "Requests per minute",
                Kind::Whole { min: 1, max: 1_000_000 },
                "600",
            ),
            (
                "body-limit",
                "Body limit (bytes)",
                Kind::Whole {
                    min: 1024,
                    max: 64 * 1024 * 1024,
                },
                "4194304",
            ),
        ] {
            entries.push(Entry::new(
                format!("telemetry/receiver@{attr}"),
                name,
                Group::Receiver,
                kind,
                default,
            ));
        }
        entries
    })
}

/// The spans and metrics of an artifact type when the conf does not name it (telemetry `Config::default`).
fn default_artifact(kind: &str) -> (bool, bool) {
    match kind {
        "AT_ENTITY" => (false, false),
        "AT_XML_SCREEN_CONTENT" => (false, true),
        _ => (true, true),
    }
}

/// The key of the target rule for `target`.
pub fn target_key(target: &str) -> String {
    format!("logging/target[{target}]@level")
}

/// The target of a target rule's key.
pub fn target_of(key: &str) -> Option<&str> {
    key.strip_prefix("logging/target[")?.strip_suffix("]@level")
}

/// The setting of `key`, or why there is none.
pub fn lookup(key: &str) -> Result<Entry, String> {
    if let Some(target) = target_of(key) {
        if !is_target(target) {
            return Err(format!(
                "[{target}] is not a log target: a module path or dotted name of letters, digits, '_', ':' and '.'"
            ));
        }
        if target.starts_with(log_filter::RSIPSTACK) {
            return Err("rsipstack is held at WARN: its INFO lines carry SIP keys and numbers".to_string());
        }
        return Ok(Entry::new(key, target, Group::Logging, Kind::Level { trace: true }, ""));
    }
    entries()
        .iter()
        .find(|entry| entry.key == key)
        .cloned()
        .ok_or_else(|| format!("[{key}] is not a setting that can be changed here"))
}

/// Where an on-disk value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Conf,
    /// The attribute is absent.
    Default,
    Env,
    /// Fixed when the process started (the console).
    Started,
    /// A target rule, which has no on-disk value.
    None,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conf => "conf",
            Self::Default => "default",
            Self::Env => "env moqui_log_level",
            Self::Started => "as started",
            Self::None => "none",
        }
    }
}

/// The on-disk value of `entry` and where it comes from.
pub fn on_disk(factory: &ExecutionContextFactory, entry: &Entry) -> (String, Source) {
    if target_of(&entry.key).is_some() {
        return (String::new(), Source::None);
    }
    match entry.key.as_str() {
        LOG_LEVEL => {
            let level = log_filter::level_name(log_filter::disk().level).to_string();
            let source = if moqui_lib::env::get_prop_or_env("moqui_log_level").trim().is_empty() {
                Source::Default
            } else {
                Source::Env
            };
            (level, source)
        }
        CONSOLE_LEVEL => {
            let disk = log_filter::disk();
            let value = match disk.console {
                Console::Off => "off",
                Console::All => log_filter::level_name(disk.level),
                Console::AtLeast(level) => log_filter::level_name(level),
            };
            (value.to_string(), Source::Started)
        }
        key => {
            let (path, attr) = split_key(key);
            match applies(key) {
                Applies::Conf => {
                    let value = factory.conf_setting_on_disk(path, attr);
                    if value.is_empty() {
                        (entry.default.to_string(), Source::Default)
                    } else {
                        (value, Source::Conf)
                    }
                }
                Applies::Telemetry => telemetry_on_disk(factory, entry, path, attr),
                Applies::Logging => (String::new(), Source::None),
            }
        }
    }
}

/// `path` and `attr` of `path@attr`.
pub(crate) fn split_key(key: &str) -> (&str, &str) {
    key.rsplit_once('@').unwrap_or((key, ""))
}

/// `base` and `selector` of `base[selector]`.
pub(crate) fn split_selector(path: &str) -> (&str, Option<&str>) {
    match path.split_once('[') {
        Some((base, rest)) => (base, rest.strip_suffix(']')),
        None => (path, None),
    }
}

/// A telemetry setting's value from the config the factory started with, and whether the conf names it.
fn telemetry_on_disk(factory: &ExecutionContextFactory, entry: &Entry, path: &str, attr: &str) -> (String, Source) {
    let telemetry = crate::telemetry::for_factory(factory);
    let config = telemetry.config();
    let (base, selector) = split_selector(path);
    let value = match (base, selector) {
        ("telemetry", _) => match attr {
            "sample-ratio" => config.sample_ratio.to_string(),
            "slow-root-ms" => config.slow_root.as_millis().to_string(),
            _ => String::new(),
        },
        ("telemetry/artifact", Some(kind)) => {
            let (spans, metrics) = config.artifact_types.get(kind).copied().unwrap_or((true, true));
            match attr {
                "spans" => spans.to_string(),
                "metrics" => metrics.to_string(),
                "sample-ratio" => config
                    .artifact_sample_ratios
                    .get(kind)
                    .map(f64::to_string)
                    .unwrap_or_default(),
                _ => String::new(),
            }
        }
        ("telemetry/metric", Some(name)) => config
            .metric_attributes
            .get(name)
            .map(|keys| keys.join(","))
            .unwrap_or_default(),
        ("telemetry/operations", _) => match attr {
            "record" => config.operations.record.as_str().to_string(),
            "max-names" => config.operations.max_names.to_string(),
            _ => String::new(),
        },
        _ => String::new(),
    };
    let named = conf_names(&factory.conf_text(), base, selector, attr);
    let value = if value.is_empty() && !named {
        entry.default.to_string()
    } else {
        value
    };
    (value, if named { Source::Conf } else { Source::Default })
}

/// Whether the merged conf has a non-empty `attr` on the element at `base` (below the root), selected by its `type`
/// or `name` when `selector` is given.
fn conf_names(conf_text: &str, base: &str, selector: Option<&str>, attr: &str) -> bool {
    let Ok(root) = MNode::parse_shared("moqui-conf", conf_text) else {
        return false;
    };
    let mut names = base.split('/');
    let Some(first) = names.next() else {
        return false;
    };
    let Some(mut node) = root.first_named(first) else {
        return false;
    };
    let rest: Vec<&str> = names.collect();
    for (index, name) in rest.iter().enumerate() {
        let last = index + 1 == rest.len();
        let found = match (last, selector) {
            (true, Some(selected)) => node.named_iter(name).find(|child| {
                child.attribute("type").as_deref() == Some(selected)
                    || child.attribute("name").as_deref() == Some(selected)
            }),
            _ => node.first_named(name),
        };
        let Some(found) = found else {
            return false;
        };
        node = found;
    }
    node.attribute(attr).is_some_and(|value| {
        let value = moqui_lib::env::expand(&value).unwrap_or(value);
        !value.trim().is_empty()
    })
}
