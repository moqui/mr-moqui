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

use super::*;
use crate::web::{finish_response, WebRequest, WebResponse};

const NAMES: &str = "Vault,Vaults,VaultKey,VaultContacts,RecoveryKeys";
const POLICY: &str = "frame-ancestors 'none'; worker-src 'self' blob:; script-src 'self' 'wasm-unsafe-eval'";

/// A vault screen's path is one under the apps naming a vault screen, decoded and without its render mode; nothing
/// else is, and no names is none.
#[test]
fn a_vault_screens_path_is_named_under_the_apps() {
    for path in [
        "/qapps/tools/System/Vaults",
        "/qapps/tools/System/Vaults/Lease?leaseId=1",
        "/qapps/boss/Commons/Vault",
        "/apps/boss/Commons/Settings/VaultKey.qvt",
        "/qapps/boss/Admin/Security/RecoveryKeys",
        "/qapps/boss/Commons/%56ault",
    ] {
        assert!(is_vault_screen_path(path, NAMES), "{path}");
    }
    for path in [
        "/qapps/tools/System/Users",
        "/qapps",
        "/Vault",
        "/rest/s1/vault/vaults",
        "/qapps/boss/Commons/VaultItems",
        "/qapps/boss/Commons/vault",
    ] {
        assert!(!is_vault_screen_path(path, NAMES), "{path}");
    }
    assert!(!is_vault_screen_path("/qapps/boss/Commons/Vault", " , "));
}

/// The `/qapps` page served at a vault screen's path, and the screen's own answers, carry `vault@screen-policy`; any
/// other path the conf's screen-render policy; a response with a policy of its own keeps it.
#[test]
fn the_page_at_a_vault_screen_gets_the_vault_policy() {
    let ctx = crate::context::ExecutionContextFactory::test_context_with_conf(&format!(
        "<moqui-conf><vault screen-names=\"{NAMES}\" screen-policy=\"{POLICY}\"/></moqui-conf>"
    ));
    let factory = ctx.factory();
    let policy_of = |path: &str, own: Option<&str>| {
        let mut response = WebResponse::new(200, b"<html></html>".to_vec());
        if let Some(own) = own {
            response
                .headers
                .push(("Content-Security-Policy".to_string(), own.to_string()));
        }
        let finished = finish_response(factory, &WebRequest::new("GET", path), response, false);
        let policies: Vec<String> = finished
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-security-policy"))
            .map(|(_, value)| value.clone())
            .collect();
        assert_eq!(policies.len(), 1, "{path}: {policies:?}");
        policies[0].clone()
    };
    assert_eq!(policy_of("/qapps/tools/System/Vaults/VaultList", None), POLICY);
    assert_eq!(policy_of("/apps/boss/Commons/Vault.qvt", None), POLICY);
    let other = policy_of("/qapps/tools/System/Users", None);
    assert!(
        !other.contains("worker-src") && other.contains("frame-ancestors"),
        "{other}"
    );
    assert_eq!(
        policy_of("/qapps/boss/Commons/Vault", Some("frame-ancestors 'self'")),
        "frame-ancestors 'self'"
    );
}
