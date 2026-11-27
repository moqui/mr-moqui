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

//! The vault screens' Content-Security-Policy (plans/archive/Vault-Instances.md §3.6, docs/security/Vaults.md "The vault
//! screens' policy"). The vault worker runs in the page's document, and the `/qapps` shell loads every screen into the
//! one document it was served as, so the policy has to be on that document: a request whose path resolves to a vault
//! screen (one of `vault@screen-names` among the screens the path names, never an extra-path name or a path parameter)
//! gets the site's `screen-render` policy with `vault@screen-policy`'s directives added ([`merged_policy`]), the
//! shell's page as well as the screen's own answers. The shell's client loads a vault screen as a new document unless
//! the page was served as a vault screen's ([`DOCUMENT_ATTRIBUTE`], which `qapps.xml` gives it as
//! `qvtConfig.vaultDocument`). The names are the desktop and mobile apps' too (`moqui_client_tauri::vault`
//! `VAULT_SCREENS`), which call the native vault only from such a document.

use crate::context::{conf_env, ExecutionContextFactory};
use crate::screen::info;
use crate::screen::ScreenUrlInfo;

/// The request attribute that says the request is a vault screen's, set before a screen renders.
pub const DOCUMENT_ATTRIBUTE: &str = "moqui.vault.document";

/// The sources the vault worker needs in a directive the site's policy already has: the worker from a `blob:` URL
/// and its WebAssembly. Nothing else of the site's directive is changed.
const REQUIRED: [(&str, &[&str]); 2] = [("worker-src", &["blob:"]), ("script-src", &["'wasm-unsafe-eval'"])];

/// Directives that do not fall back to `default-src`.
const NOT_FETCH: [&str; 5] = [
    "base-uri",
    "form-action",
    "frame-ancestors",
    "sandbox",
    "upgrade-insecure-requests",
];

fn names_of(names: &str) -> Vec<&str> {
    names
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect()
}

/// A path name as the screens read it: percent-decoded, without a render-mode extension.
fn screen_name(segment: &str) -> String {
    let name = super::percent_decode_path(segment);
    name.split('.').next().unwrap_or("").to_string()
}

/// Whether any name of `path` (no query) is one of `names` (comma-separated): the quick test before the path is
/// resolved, which any request to a vault screen passes.
#[must_use]
pub fn mentions_vault_screen(path: &str, names: &str) -> bool {
    let wanted = names_of(names);
    let bare = path.split(['?', '#']).next().unwrap_or("");
    bare.split('/')
        .filter(|segment| !segment.is_empty())
        .any(|segment| wanted.contains(&screen_name(segment).as_str()))
}

/// Whether a request for `path` on `host` is a vault screen's: under the host's root screen (the `/qapps` page's path
/// read as the same path under `/apps`, the screen it shows), one of `vault@screen-names` is among the screens the path
/// names or its default subscreens, never an extra-path name. A path that does not resolve is not.
#[must_use]
pub fn is_vault_request(factory: &ExecutionContextFactory, host: &str, path: &str) -> bool {
    let names = conf_env::setting_on(factory, "vault@screen-names");
    if !mentions_vault_screen(path, &names) {
        return false;
    }
    let root = match info::root_for_host(factory, host, "webroot") {
        Ok(found) => found.location,
        Err(_) => info::default_root(factory),
    };
    let bare = path.split(['?', '#']).next().unwrap_or("");
    let screen_path = match bare.strip_prefix("/qapps") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("/apps{rest}"),
        _ => bare.to_string(),
    };
    let Ok(info) = ScreenUrlInfo::parse(factory, &root, &screen_path, 0) else {
        return false;
    };
    let wanted = names_of(&names);
    info.pre_transition_path
        .iter()
        .any(|name| wanted.contains(&screen_name(name).as_str()))
}

fn directives(policy: &str) -> Vec<(String, Vec<String>)> {
    policy
        .split(';')
        .filter_map(|directive| {
            let mut words = directive.split_whitespace();
            let name = words.next()?.to_ascii_lowercase();
            Some((name, words.map(str::to_string).collect()))
        })
        .collect()
}

/// The vault screens' policy: the site's `screen-render` policy `site` with each directive of `vault` it does not
/// have, and in each it has, only the sources the worker needs ([`REQUIRED`]), so an operator's directive is never
/// loosened. A fetch directive the site leaves to its `default-src` starts from that `default-src`.
#[must_use]
pub fn merged_policy(site: &str, vault: &str) -> String {
    let mut merged = directives(site);
    let site_default = merged
        .iter()
        .find(|(name, _)| name == "default-src")
        .map(|(_, sources)| sources.clone());
    for (name, sources) in directives(vault) {
        let required: &[&str] = REQUIRED
            .iter()
            .find(|(directive, _)| *directive == name)
            .map_or(&[], |(_, needed)| *needed);
        let add_required = |kept: &mut Vec<String>| {
            for needed in required {
                if !kept.iter().any(|source| source == needed) {
                    kept.retain(|source| source != "'none'");
                    kept.push((*needed).to_string());
                }
            }
        };
        if let Some((_, kept)) = merged.iter_mut().find(|(have, _)| *have == name) {
            add_required(kept);
            continue;
        }
        let mut sources = match &site_default {
            Some(default) if !NOT_FETCH.contains(&name.as_str()) && name != "default-src" => default.clone(),
            _ => sources,
        };
        add_required(&mut sources);
        merged.push((name, sources));
    }
    merged
        .into_iter()
        .map(|(name, sources)| {
            if sources.is_empty() {
                name
            } else {
                format!("{name} {}", sources.join(" "))
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The vault screens' policy for a request for `path` on `host` when it is a vault screen's ([`is_vault_request`]),
/// built on `site`, the policy the response has from the conf's `screen-render` headers; None otherwise, or when
/// `vault@screen-policy` is empty.
pub(crate) fn vault_policy(factory: &ExecutionContextFactory, host: &str, path: &str, site: &str) -> Option<String> {
    let vault = conf_env::setting_on(factory, "vault@screen-policy");
    if vault.is_empty() || !is_vault_request(factory, host, path) {
        return None;
    }
    Some(merged_policy(site, &vault))
}

/// After the conf's `screen-render` headers: a response to a vault screen that set no policy of its own (a preview's
/// frame policy is kept) gets the vault screens' policy in place of the site's, built on it. The other headers the
/// conf gave, `X-Frame-Options` among them, stay.
pub(crate) fn apply(
    factory: &ExecutionContextFactory,
    host: &str,
    path: &str,
    had_own: bool,
    headers: &mut [(String, String)],
) {
    if had_own {
        return;
    }
    let Some((_, value)) = headers
        .iter_mut()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-security-policy"))
    else {
        return;
    };
    if let Some(policy) = vault_policy(factory, host, path, value) {
        *value = policy;
    }
}

#[cfg(test)]
#[path = "vault_policy_tests.rs"]
mod tests;
