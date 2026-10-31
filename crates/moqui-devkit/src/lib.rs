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

//! The development kit (plans/design-dev-aide.md 17, plans/Crate-Split.md): the reload of what a changed file defines
//! ([`reload`], with the file watcher [`watch`] under the `watch` feature), `moqui check` ([`check`], [`schema`],
//! [`screen_lint`], [`lint_hazards`]), the artifact index ([`index`]), other projects' artifacts ([`project`]) and references ([`refs`]), entity ECA rule lists
//! ([`eca`]), the Groovy to JavaScript converter ([`groovy_convert`]), the TypeScript declarations of `moqui types`
//! ([`typegen`]), the spec runner of `moqui test` ([`testkit`]) and `org.moqui.impl.DevToolsServices` ([`service`]).
//! They serve the dev-aide component, the file watcher, `moqui check`, `moqui test` and agents outside the server
//! alike, so they are the framework's and not the dev-aide's.
//!
//! This crate is on moqui-core, which reaches it only through the [`moqui_core::extension`] entry below (the feature
//! `devkit` and the Rust services of `DevToolsServices`); what core itself needs of the tools stays in
//! `moqui_core::devtools`. moqui-host links it in every build, since `moqui test`, `lint` and `types` are in every
//! package; only the file watcher is dev-only. The browser build leaves it out, and its services report that `devkit` is
//! not built in.

use serde::Serialize;

pub mod check;
pub mod eca;
pub mod groovy_convert;
pub mod index;
pub mod project;
mod lint_hazards;
pub mod refs;
pub mod reload;
pub mod schema;
pub mod screen_lint;
pub mod service;
pub mod testkit;
pub mod typegen;
#[cfg(feature = "watch")]
pub mod watch;

pub use reload::{on_reload, recent_reports, reload_paths, ArtifactKind, FileReload, ReloadReport};

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["devkit"],
        register_services: service::register,
        ..moqui_core::extension::Extension::EMPTY
    }
}

/// One problem a check or a reload found in a file, as the Problems panel, `moqui check` and the agents read it.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub path: String,
    /// 1-based; 0 when the problem is the file's as a whole.
    pub line: u32,
    /// 1-based; 0 when not known.
    pub column: u32,
    pub severity: Severity,
    /// The rule that found it, such as `reload.service` or `schema.unknown-attribute`.
    pub rule: String,
    pub message: String,
    /// The skill section with the rule, as `file#heading`, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Problem {
    /// An error of the file as a whole.
    #[must_use]
    pub fn file_error(path: &str, rule: &str, message: impl Into<String>) -> Self {
        Self {
            path: path.to_string(),
            line: 0,
            column: 0,
            severity: Severity::Error,
            rule: rule.to_string(),
            message: message.into(),
            skill: None,
        }
    }
}

#[cfg(test)]
mod tests;
