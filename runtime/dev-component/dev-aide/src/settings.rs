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

//! The dev-aide settings (plans/design-dev-aide.md 16): the conf's `dev-aide` element, with what Settings stores over it in
//! `moqui.dev.DevAideSetting` rows (the site components, network git in shared mode).

use std::path::PathBuf;
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::EntityFacade;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

pub const SETTING: &str = "moqui.dev.DevAideSetting";

#[derive(Clone, Debug)]
pub struct Settings {
    /// The components the site owns, the first the default target of an extension. A root with none is upstream.
    pub site_components: Vec<String>,
    /// A team dev server (D19): network git commands off unless an admin enabled them.
    pub shared: bool,
    pub network_git: bool,
    /// The conf's default for a `DEV_AIDE` user's "Allow editing upstream components".
    pub allow_upstream_edits: bool,
    pub git_command: String,
    pub git_read_timeout: Duration,
    pub git_network_timeout: Duration,
    /// A write may run hooks (pre-commit), so it has longer than a read.
    pub git_write_timeout: Duration,
    /// Text over this opens read only.
    pub max_edit_bytes: u64,
    /// Roots the conf adds: (location, label).
    pub roots: Vec<(PathBuf, String)>,
    /// Paths left out of every root, as written (`runtime/db/**`).
    pub excludes: Vec<String>,
}

impl Settings {
    /// The settings of `factory`, with the stored ones when `facade` is given.
    #[must_use]
    pub fn of(factory: &ExecutionContextFactory, facade: Option<&EntityFacade>) -> Self {
        let attr = |name: &str| factory.conf_setting("dev-aide", name);
        let list = |text: &str| -> Vec<String> {
            text.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        };
        let seconds = |name: &str, default: u64| Duration::from_secs(attr(name).parse().unwrap_or(default));
        let mut settings = Self {
            site_components: list(&attr("site-component")),
            shared: attr("shared") == "true",
            network_git: false,
            allow_upstream_edits: attr("allow-upstream-edits") == "true",
            git_command: Some(attr("git-command"))
                .filter(|command| !command.is_empty())
                .unwrap_or_else(|| "git".into()),
            git_read_timeout: seconds("git-read-timeout", 15),
            git_network_timeout: seconds("git-network-timeout", 300),
            git_write_timeout: seconds("git-write-timeout", 120),
            max_edit_bytes: attr("max-edit-bytes").parse().unwrap_or(2_097_152),
            roots: Vec::new(),
            excludes: Vec::new(),
        };
        if let Ok(conf) = MNode::parse_shared("moqui-conf", factory.conf_text()) {
            for element in conf.named_iter("dev-aide") {
                for root in element.named_iter("root") {
                    if let Some(location) = root.attribute("location").filter(|location| !location.is_empty()) {
                        let label = root.attribute("label").unwrap_or_default();
                        settings.roots.push((PathBuf::from(location), label));
                    }
                }
                for exclude in element.named_iter("exclude") {
                    settings
                        .excludes
                        .extend(exclude.attribute("pattern").filter(|pattern| !pattern.is_empty()));
                }
            }
        }
        if let Some(facade) = facade {
            if let Some(stored) = stored(facade, "siteComponents") {
                settings.site_components = list(&stored);
            }
            settings.network_git = stored(facade, "networkGit").as_deref() == Some("Y");
        }
        settings
    }

    /// The component an extension goes in by default, when there is one.
    #[must_use]
    pub fn default_site(&self) -> Option<&str> {
        self.site_components.first().map(String::as_str)
    }
}

/// A `DevAideSetting` row's value.
#[must_use]
pub fn stored(facade: &EntityFacade, name: &str) -> Option<String> {
    let row = facade
        .find(SETTING)
        .ok()?
        .eq("settingName", Value::string(name))
        .one()
        .ok()??;
    row.get("settingValue")
        .map(moqui_lib::object::to_plain_string)
        .filter(|value| !value.is_empty())
}
