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

const NAMES: &str = "Vault,Vaults,VaultKey,VaultContacts,RecoveryKeys";
const VAULT: &str = "default-src 'self'; script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'; connect-src 'self'; \
                     worker-src 'self' blob:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'";

/// The quick test before a path is resolved: a name of the path, decoded and without its render mode, is a vault
/// screen's; no names is none. Which of those are screens, not extra-path names, is `screen::tests::vault_pages`.
#[test]
fn a_path_mentions_a_vault_screen_by_a_decoded_name() {
    for path in [
        "/qapps/tools/System/Vaults",
        "/qapps/tools/System/Vaults/Lease?leaseId=1",
        "/apps/boss/Commons/Settings/VaultKey.qvt",
        "/qapps/boss/Commons/%56ault",
        "/Vault",
    ] {
        assert!(mentions_vault_screen(path, NAMES), "{path}");
    }
    for path in [
        "/qapps/tools/System/Users",
        "/qapps/boss/Commons/VaultItems",
        "/qapps/boss/Commons/vault",
    ] {
        assert!(!mentions_vault_screen(path, NAMES), "{path}");
    }
    assert!(!mentions_vault_screen("/qapps/boss/Commons/Vault", " , "));
}

/// The vault policy is the site's screen-render policy with the vault's directives it lacks; a directive the operator
/// set is kept, given only the worker's blob: and 'wasm-unsafe-eval'; a fetch directive the operator left to their
/// default-src starts from it.
#[test]
fn the_vault_policy_adds_to_the_sites_policy_and_never_loosens_it() {
    let merged = merged_policy("frame-ancestors 'none'; form-action 'self';", VAULT);
    for directive in [
        "frame-ancestors 'none'",
        "form-action 'self'",
        "default-src 'self'",
        "script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'",
        "connect-src 'self'",
        "worker-src 'self' blob:",
        "object-src 'none'",
        "base-uri 'self'",
    ] {
        assert!(
            merged.split("; ").any(|part| part == directive),
            "{directive}: {merged}"
        );
    }
    assert!(!merged.contains("unsafe-inline"), "{merged}");
    // an operator's own script-src and worker-src stay theirs, with only what the worker needs
    let operator = merged_policy(
        "default-src https://cdn.example; script-src 'self' https://cdn.example; worker-src 'none'; frame-ancestors 'self'",
        VAULT,
    );
    let directive = |name: &str| {
        operator
            .split("; ")
            .find(|part| part.split_whitespace().next() == Some(name))
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(
        directive("script-src"),
        "script-src 'self' https://cdn.example 'wasm-unsafe-eval'"
    );
    assert_eq!(directive("worker-src"), "worker-src blob:");
    assert_eq!(directive("frame-ancestors"), "frame-ancestors 'self'");
    assert_eq!(directive("default-src"), "default-src https://cdn.example");
    // left to the operator's default-src, not to the vault's 'self'
    assert_eq!(directive("connect-src"), "connect-src https://cdn.example");
    assert_eq!(directive("base-uri"), "base-uri 'self'");
}
