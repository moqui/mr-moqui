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

//! The conf of attested deploys and the gate (docs/tools/Component-Deploy.md "The gate"): the `component-deploy`
//! element, read once when the server starts (a conf reload or a live override changes none of it, so whoever edits
//! the conf of a running server does not turn the gate off or point it at another secret or repository until the next
//! start), with its `allow` entries: the components, repository URLs and branches an attested deploy may bring.

use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;

/// One `allow` entry: an attested deploy of `component` from `repository_url` and `branch`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Allow {
    pub component: String,
    pub repository_url: String,
    pub branch: String,
}

/// The `component-deploy` conf as it was when the server started.
#[derive(Clone, Default)]
pub struct GateConf {
    /// `require-passed-run`: only attested commits are put in place.
    pub require_passed_run: bool,
    /// `attestation-secret`; empty takes no attested deploy.
    pub(crate) secret: String,
    /// `max-age-seconds`, in milliseconds.
    pub max_age_millis: i64,
    /// This server's public base URLs an attestation's audience must be one of: `public-urls`, else the webroot
    /// webapp's URL when `webapp-list/webapp[webroot]@http-host` is set; empty when it cannot tell, and then every attestation is refused.
    pub public_urls: Vec<String>,
    /// The `allow` entries; none takes no attested deploy.
    pub allows: Vec<Allow>,
}

impl std::fmt::Debug for GateConf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // never the secret
        f.debug_struct("GateConf")
            .field("require_passed_run", &self.require_passed_run)
            .field("has_secret", &!self.secret.is_empty())
            .field("max_age_millis", &self.max_age_millis)
            .field("public_urls", &self.public_urls)
            .field("allows", &self.allows)
            .finish()
    }
}

impl GateConf {
    /// The conf of `factory`'s merged conf as it is now, with no live override.
    #[must_use]
    pub fn read(factory: &ExecutionContextFactory) -> Self {
        let setting = |name: &str| factory.conf_setting_on_disk("component-deploy", name);
        let max_age: i64 = setting("max-age-seconds").parse().unwrap_or(300);
        let mut public_urls: Vec<String> = setting("public-urls")
            .split([',', ' ', '\n', '\t'])
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string)
            .collect();
        if public_urls.is_empty() && !factory.webapp_setting("webroot", "http-host").is_empty() {
            let root = factory.webapp_root_url("webroot");
            if !root.is_empty() {
                public_urls.push(root);
            }
        }
        Self {
            require_passed_run: setting("require-passed-run") == "true",
            secret: setting("attestation-secret"),
            max_age_millis: max_age.max(1).saturating_mul(1000),
            public_urls,
            allows: allows(&factory.conf_text()),
        }
    }

    /// The entry that allows an attested deploy of `component` from `repository_url` and `branch`, compared as given
    /// but for the URL's scheme, host and trailing slash ([`moqui_git::ci::attest::base_url`] where git is linked).
    #[must_use]
    pub fn allowed(&self, component: &str, repository_url: &str, branch: &str) -> Option<&Allow> {
        let url = same_url(repository_url);
        self.allows.iter().find(|allow| {
            allow.component == component && same_url(&allow.repository_url) == url && allow.branch == branch
        })
    }
}

/// A repository URL as two are compared: trimmed, the scheme and host in lower case, no trailing slash.
pub(crate) fn same_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest
                .split_once('/')
                .map_or((rest, ""), |(authority, path)| (authority, path));
            let path = if path.is_empty() {
                String::new()
            } else {
                format!("/{path}")
            };
            format!(
                "{}://{}{path}",
                scheme.to_ascii_lowercase(),
                authority.to_ascii_lowercase()
            )
        }
        None => url.to_string(),
    }
}

/// The `allow` entries of the conf's `component-deploy`; one that leaves out its component, URL or
/// branch is left out, logged.
fn allows(conf_text: &str) -> Vec<Allow> {
    let Ok(conf) = moqui_lib::node::MNode::parse_shared("moqui-conf", conf_text) else {
        return Vec::new();
    };
    let Some(element) = conf.first_named("component-deploy") else {
        return Vec::new();
    };
    let attribute = |node: &moqui_lib::node::MNode, name: &str| {
        let raw = node.attribute(name).unwrap_or_default();
        raw.trim().to_string()
    };
    let mut out = Vec::new();
    for node in element.children_named("allow") {
        let allow = Allow {
            component: attribute(&node, "component"),
            repository_url: attribute(&node, "repository-url"),
            branch: attribute(&node, "branch"),
        };
        if allow.component.is_empty() || allow.repository_url.is_empty() || allow.branch.is_empty() {
            tracing::error!("component-deploy allow needs component, repository-url and branch; left out: {allow:?}");
            continue;
        }
        out.push(allow);
    }
    out
}

/// The `component-deploy` conf of `factory` as it was when first read: at start ([`super::git::install`]), so a conf
/// reload or a live override does not change it on a running server.
pub fn conf(factory: &ExecutionContextFactory) -> Arc<GateConf> {
    factory.extension(GateConf::read)
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
