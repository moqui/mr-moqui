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

//! The framework's development tools (plans/design-dev-aide.md 17): the reload of what a changed file defines, and
//! restart from a running server. They serve the dev-aide component, the file watcher, `moqui check` and agents
//! outside the server alike, so they are the framework's and not the dev-aide's.

pub mod check;
pub mod index;
pub mod refs;
pub mod reload;
pub mod restart;
pub mod schema;
#[cfg(feature = "devtools")]
pub mod watch;

pub use reload::{on_reload, recent_reports, reload_paths, ArtifactKind, FileReload, ReloadReport};

use serde::Serialize;

/// Whether this instance's purpose is `dev`, where the development tools run.
#[must_use]
pub fn is_dev_instance() -> bool {
    moqui_lib::env::get_prop_or_env("instance_purpose") == "dev"
}

/// The permissions of the dev-aide's tiers (plans/design-dev-aide.md 3.4); `ADMIN` holds `DEV_AIDE` by its seed.
pub const DEV_AIDE_PERMISSIONS: [&str; 3] = ["DEV_AIDE_VIEW", "DEV_AIDE_EXTEND", "DEV_AIDE"];

/// Whether `user_id` may use the development tools here: a dev instance with the dev-aide component loaded, and one of
/// its permissions. The agents' dev tools and the `screen-xml` dialect are offered on this (plans/design-dev-aide.md 13.2).
#[must_use]
pub fn dev_aide_user(factory: &crate::context::ExecutionContextFactory, user_id: &str) -> bool {
    is_dev_instance()
        && !user_id.is_empty()
        && factory
            .components()
            .iter()
            .any(|component| component.name == "dev-aide")
        && DEV_AIDE_PERMISSIONS
            .iter()
            .any(|permission| crate::user::user_has_permission(user_id, permission).unwrap_or(false))
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
