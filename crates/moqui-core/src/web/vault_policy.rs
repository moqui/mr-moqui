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

//! The vault screens' Content-Security-Policy (plans/Vault-Instances.md §3.6, docs/security/Vaults.md "The vault
//! screens' policy"). The vault worker runs in the page's document, and the `/qapps` shell loads every screen into the
//! one document it was served as, so the policy has to be on that document: a response to a path under `/qapps/` or
//! `/apps/` naming one of `vault@screen-names` gets `vault@screen-policy`, the shell's page as well as the screen's own
//! answers, by the path alone. The shell's client loads a vault screen as a new document unless the page was served at
//! one (WebrootVue.qvt.js `setUrl`), so a vault screen never runs in a document served without the policy; a screen the
//! page goes to after it keeps that document, and the policy, until the next full load. The names are the desktop and mobile apps' too (`moqui_client_tauri::vault`
//! `VAULT_SCREENS`), which call the native vault only from such a document.

use crate::context::{conf_env, ExecutionContextFactory};

/// Whether `path` (no query) is a vault screen's: under `/qapps/` or `/apps/`, with one of `names` (comma-separated)
/// among its names after the first, each name percent-decoded and taken without a render-mode extension.
#[must_use]
pub fn is_vault_screen_path(path: &str, names: &str) -> bool {
    let wanted: Vec<&str> = names
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    let bare = path.split(['?', '#']).next().unwrap_or("");
    let mut segments = bare.split('/').filter(|name| !name.is_empty());
    if !matches!(segments.next(), Some("qapps" | "apps")) {
        return false;
    }
    segments.any(|segment| {
        let name = super::percent_decode_path(segment);
        let name = name.split('.').next().unwrap_or("");
        wanted.contains(&name)
    })
}

/// `vault@screen-policy` for a response to `path` when it is a vault screen's, else None. An empty setting is none.
pub(crate) fn vault_policy(factory: &ExecutionContextFactory, path: &str) -> Option<String> {
    let names = conf_env::setting_on(factory, "vault@screen-names");
    if !is_vault_screen_path(path, &names) {
        return None;
    }
    let policy = conf_env::setting_on(factory, "vault@screen-policy");
    (!policy.is_empty()).then_some(policy)
}

/// Puts the vault screens' policy on a response to a vault screen's path that set no policy of its own (a preview's
/// frame policy is kept), before the conf's `screen-render` defaults fill in the rest.
pub(crate) fn apply(factory: &ExecutionContextFactory, path: &str, headers: &mut Vec<(String, String)>) {
    if headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-security-policy"))
    {
        return;
    }
    if let Some(policy) = vault_policy(factory, path) {
        headers.push(("Content-Security-Policy".to_string(), policy));
    }
}

#[cfg(test)]
#[path = "vault_policy_tests.rs"]
mod tests;
