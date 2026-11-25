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

use super::{check_clone_url, context, endpoint, public_description, REST_PATH};

#[test]
fn a_target_is_https_or_http_to_a_loopback_address_with_no_credentials() {
    assert_eq!(
        endpoint("https://demo.example.com/").unwrap(),
        format!("https://demo.example.com{REST_PATH}")
    );
    assert_eq!(
        endpoint("http://127.0.0.1:8080").unwrap(),
        format!("http://127.0.0.1:8080{REST_PATH}")
    );
    assert!(endpoint("http://[::1]:8080").is_ok());
    for refused in [
        "http://demo.example.com",
        "http://10.0.0.5",
        "ftp://demo.example.com",
        "demo.example.com",
        "https://user:secret@demo.example.com",
        "https:///path",
    ] {
        let err = endpoint(refused).unwrap_err();
        assert!(!err.contains("secret"), "{err}");
    }
    assert_eq!(context("prod"), "deploy/prod");
}

/// A clone URL is https or ssh with no credentials in it (an ssh account name is no secret); a refusal does not
/// repeat the URL.
#[test]
fn a_clone_url_is_https_or_ssh_with_no_credentials() {
    for good in [
        "https://source.example.com/git/site.git",
        "ssh://git@source.example.com/git/site.git",
        "ssh://source.example.com:2222/site.git",
    ] {
        assert!(check_clone_url(good).is_ok(), "{good}");
    }
    for refused in [
        "https://user:secret@source.example.com/site.git",
        "https://token@source.example.com/site.git",
        "ssh://git:secret@source.example.com/site.git",
        "http://source.example.com/site.git",
        "git@source.example.com:site.git",
        "file:///srv/site.git",
        "https:///site.git",
    ] {
        let err = check_clone_url(refused).unwrap_err();
        assert!(!err.contains("secret"), "{refused}: {err}");
    }
}

/// A deployment's public status says how it ended and no more: the target's words are in errorText only.
#[test]
fn a_deployments_public_status_says_how_it_ended_only() {
    assert_eq!(public_description(super::FAILED), "Deploy failed");
    assert_eq!(public_description(super::REFUSED), "Deploy refused");
    assert_eq!(public_description(super::CANCELLED), "Deploy cancelled");
    assert_eq!(public_description(super::DEPLOYED), "Deployed");
}

/// A target's token is the item of a binding of its own environment, its machine access an environment of its own,
/// and its lease names the deployment and where the attestation goes, never more than a lease holds and never with
/// credentials of the URL (plans/archive/Vault-Instances.md §4.6).
#[test]
fn a_targets_token_is_its_environments_vault_item_and_its_lease_names_the_deployment() {
    assert_eq!(super::token_binding("100042"), "GitCiEnvironment:100042:targetToken");
    assert_eq!(super::machine_environment("100042"), "git-ci-100042");
    let operation = super::sign_operation("100500", "https://target.example.com");
    assert_eq!(
        operation,
        "deploy GitDeployment 100500: sign its attestation for https://target.example.com/"
    );
    let long = format!("https://{}.example.com", "a".repeat(300));
    let operation = super::sign_operation("100500", &long);
    assert!(operation.len() <= 255, "{operation}");
    assert!(operation.contains("GitDeployment 100500"));
}
