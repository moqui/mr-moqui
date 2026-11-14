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

use std::collections::BTreeMap;

use base64::Engine;
use moqui_lib::object::{to_plain_string, Value};

use super::ssh_key_fingerprint;
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;

const SERVICES: &str = "org.moqui.impl.UserServices";

fn entered() -> (EntityFacade, ExecutionContext) {
    let facade = crate::testing::open();
    let context = ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    context.factory().set_entity(facade.clone());
    (facade, context)
}

/// An ed25519 public key line whose key bytes are all `fill`.
fn key_line(fill: u8) -> String {
    let mut blob = Vec::new();
    for part in [b"ssh-ed25519".as_slice(), &[fill; 32]] {
        blob.extend_from_slice(&(part.len() as u32).to_be_bytes());
        blob.extend_from_slice(part);
    }
    format!(
        "ssh-ed25519 {} someone@laptop",
        base64::engine::general_purpose::STANDARD.encode(blob)
    )
}

/// Call a service of `UserServices` as `username`: what it gave and the errors it left.
fn call_as(username: &str, name: &str, parameters: &[(&str, Value)]) -> (BTreeMap<String, Value>, String) {
    if !username.is_empty() {
        crate::user::internal_login_user(username).unwrap();
    }
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let called = crate::service::call::sync(&format!("{SERVICES}.{name}"), &parameters);
    let mut errors = ExecutionContext::with_current(|ctx| {
        let text = ctx.message().errors_text().join("\n");
        ctx.message().clear();
        text
    })
    .unwrap_or_default();
    let result = called.unwrap_or_else(|err| {
        errors.push_str(err.message());
        BTreeMap::new()
    });
    if !username.is_empty() {
        crate::user::logout().unwrap();
    }
    (result, errors)
}

fn maps(out: &BTreeMap<String, Value>, key: &str) -> Vec<BTreeMap<String, String>> {
    let Some(Value::List(list)) = out.get(key) else {
        panic!("no {key} in {out:?}")
    };
    list.iter()
        .map(|item| {
            let Value::Map(map) = item else {
                panic!("{key} holds {item:?}")
            };
            map.iter()
                .map(|(name, value)| (name.clone(), to_plain_string(value)))
                .collect()
        })
        .collect()
}

#[test]
fn a_user_lists_only_their_own_keys_and_tokens_without_a_secret() {
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    {
        let _authz = context.disable_authz();
        for name in ["keys.ann", "keys.bob"] {
            let mut row = facade.make_value("moqui.security.UserAccount").unwrap();
            row.set("userId", Value::string(name));
            row.set("username", Value::string(name));
            row.create(&facade).unwrap();
        }
    }
    let text = |value: &str| Value::string(value);
    // ann has a key in use and one she ended, bob one of his own
    let (kept, errors) = call_as(
        "keys.ann",
        "create#UserSshKey",
        &[("publicKey", text(&key_line(1))), ("description", text("laptop"))],
    );
    assert!(errors.is_empty(), "{errors}");
    let (ended, _) = call_as("keys.ann", "create#UserSshKey", &[("publicKey", text(&key_line(2)))]);
    call_as(
        "keys.ann",
        "delete#UserSshKey",
        &[("sshKeyId", ended["sshKeyId"].clone())],
    );
    call_as("keys.bob", "create#UserSshKey", &[("publicKey", text(&key_line(3)))]);

    let (found, errors) = call_as("keys.ann", "find#UserSshKeys", &[]);
    assert!(errors.is_empty(), "{errors}");
    let keys = maps(&found, "sshKeyList");
    assert_eq!(keys.len(), 1, "{keys:?}");
    assert_eq!(keys[0]["sshKeyId"], to_plain_string(&kept["sshKeyId"]));
    assert_eq!(keys[0]["fingerprint"], ssh_key_fingerprint(&key_line(1)).unwrap().1);
    assert_eq!(
        (keys[0]["keyType"].as_str(), keys[0]["description"].as_str()),
        ("ssh-ed25519", "laptop")
    );
    assert!(
        !keys[0]["fromDate"].is_empty() && !keys[0].contains_key("publicKey"),
        "{keys:?}"
    );
    let (all, _) = call_as("keys.ann", "find#UserSshKeys", &[("includeEnded", Value::Bool(true))]);
    let all = maps(&all, "sshKeyList");
    assert_eq!(all.len(), 2);
    assert!(all.iter().any(|key| !key["thruDate"].is_empty()), "{all:?}");

    // tokens: the list names each by an id that is neither the token nor its stored hash
    let (made, errors) = call_as("keys.ann", "create#UserAccessToken", &[("description", text("ci"))]);
    assert!(errors.is_empty(), "{errors}");
    let (token, hash, token_id) = (
        to_plain_string(&made["accessToken"]),
        to_plain_string(&made["loginKey"]),
        to_plain_string(&made["tokenId"]),
    );
    let (bobs, _) = call_as("keys.bob", "create#UserAccessToken", &[]);
    let (found, errors) = call_as("keys.ann", "find#UserAccessTokens", &[]);
    assert!(errors.is_empty(), "{errors}");
    let tokens = maps(&found, "accessTokenList");
    assert_eq!(tokens.len(), 1, "{tokens:?}");
    assert_eq!(
        (tokens[0]["tokenId"].as_str(), tokens[0]["description"].as_str()),
        (token_id.as_str(), "ci")
    );
    assert!(!tokens[0]["fromDate"].is_empty() && !tokens[0]["thruDate"].is_empty());
    assert!(
        tokens[0].values().all(|value| *value != token && *value != hash) && token_id != hash,
        "{tokens:?}"
    );
    // nobody ends another user's token, by either name of it
    for (parameter, value) in [("tokenId", &bobs["tokenId"]), ("loginKey", &bobs["loginKey"])] {
        let (_, refused) = call_as("keys.ann", "revoke#UserAccessToken", &[(parameter, value.clone())]);
        assert!(refused.contains("no such access token"), "{refused}");
    }
    assert_eq!(
        maps(&call_as("keys.bob", "find#UserAccessTokens", &[]).0, "accessTokenList").len(),
        1
    );
    // her own, by the list's id: gone from the list, and there with the ended ones
    let (_, errors) = call_as("keys.ann", "revoke#UserAccessToken", &[("tokenId", text(&token_id))]);
    assert!(errors.is_empty(), "{errors}");
    assert!(maps(&call_as("keys.ann", "find#UserAccessTokens", &[]).0, "accessTokenList").is_empty());
    let (ended, _) = call_as(
        "keys.ann",
        "find#UserAccessTokens",
        &[("includeEnded", Value::Bool(true))],
    );
    assert_eq!(maps(&ended, "accessTokenList").len(), 1);
    // with nobody logged in there is nothing to list
    for name in ["find#UserSshKeys", "find#UserAccessTokens"] {
        let (out, refused) = call_as("", name, &[]);
        let listed = out
            .values()
            .any(|value| matches!(value, Value::List(list) if !list.is_empty()));
        assert!(!listed && !refused.is_empty(), "{name}: {out:?} {refused}");
    }
}

#[test]
fn a_key_line_gives_the_fingerprint_ssh_keygen_prints() {
    assert!(ssh_key_fingerprint("ssh-dss AAAA").is_err());
    assert!(ssh_key_fingerprint("not a key").is_err());
    // the blob of an RSA key under the ed25519 type
    assert!(ssh_key_fingerprint("ssh-ed25519 AAAAB3NzaC1yc2EAAAADAQAB").is_err());
    let dir = std::env::temp_dir().join(format!("moqui-ssh-key-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("id");
    let made = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "test@example", "-f"])
        .arg(&key)
        .status();
    if !made.is_ok_and(|status| status.success()) {
        eprintln!("skipped the comparison: ssh-keygen is not on the PATH");
        return;
    }
    let line = std::fs::read_to_string(dir.join("id.pub")).unwrap();
    let listed = std::process::Command::new("ssh-keygen")
        .arg("-lf")
        .arg(dir.join("id.pub"))
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
    let (kind, fingerprint, bare) = ssh_key_fingerprint(&line).unwrap();
    assert_eq!(kind, "ssh-ed25519");
    assert_eq!(listed.split_whitespace().nth(1), Some(fingerprint.as_str()), "{listed}");
    assert!(line.starts_with(&bare) && !bare.contains("test@example"));
    let _ = std::fs::remove_dir_all(&dir);
}
