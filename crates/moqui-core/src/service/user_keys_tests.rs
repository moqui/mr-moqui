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

/// The framework's REST file, which declares the scopes of its git and hosting resources.
fn framework_rest() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/components/base/tools/service/moqui.rest.xml")
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

/// Call a service of `UserServices` as `username`, signed in a moment ago as on an account page (the user id is the
/// username in these tests): what it gave and the errors it left.
fn call_as(username: &str, name: &str, parameters: &[(&str, Value)]) -> (BTreeMap<String, Value>, String) {
    if !username.is_empty() {
        crate::user::internal_login_user(username).unwrap();
        crate::user::note_recent_auth(username);
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

/// A key made for the gateway alone logs in nowhere else, opens the gateway's scope, and its holder ends it by the
/// key itself; another user's key is not ended so.
#[test]
fn a_scoped_key_opens_only_its_scope_and_its_holder_ends_it() {
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
    let (made, errors) = call_as(
        "keys.ann",
        "create#UserScopedKey",
        &[("deviceName", Value::string("Dev AIDE gateway"))],
    );
    assert!(errors.is_empty(), "{errors}");
    let key = to_plain_string(&made["loginKey"]);
    assert_eq!(key.len(), 40);
    assert!(
        crate::user::login_user_key(&key).is_err(),
        "an account login refuses it"
    );
    assert!(
        crate::user::apply_header_keys(Some(&key), None, None).is_err(),
        "a REST header login refuses it"
    );
    crate::user::login_user_key_for(&key, "gateway").unwrap();
    assert_eq!(crate::user::current_username().unwrap().as_deref(), Some("keys.ann"));
    crate::user::logout().unwrap();
    assert!(crate::user::login_user_key_for(&key, "mcp").is_err());
    // a socket's recheck by the key's hash has no user for it either
    let hashed = crate::user::hash_login_key(&key).unwrap();
    assert_eq!(crate::user::login_key_user_by_hash(&hashed).unwrap(), None);
    let (_, errors) = call_as(
        "keys.ann",
        "create#UserScopedKey",
        &[("scope", Value::string("everything"))],
    );
    assert!(errors.contains("not a scope"), "{errors}");

    // bob cannot end ann's key; ann ends it by the key she holds
    let (theirs, _) = call_as("keys.bob", "revoke#UserHeldKey", &[("loginKey", Value::string(&key))]);
    assert_eq!(theirs["revoked"], Value::Bool(false));
    let (own, errors) = call_as("keys.ann", "revoke#UserHeldKey", &[("loginKey", Value::string(&key))]);
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(own["revoked"], Value::Bool(true));
    assert!(crate::user::login_user_key_for(&key, "gateway").is_err());

    // SEC-RTE-85: a browser session with no recent sign-in makes none, whatever header it carries that did not sign it
    // in (a Bearer token the login does not take); a client that signed the request in by its own key does
    let _web = crate::web::install(BTreeMap::new());
    let made_by = |signed_in_by_header: bool| {
        let mut request = crate::web::WebRequest::new("POST", "/rest/s1/moqui/hosting/gatewayKey");
        if signed_in_by_header {
            request.set_header("login_key", "the-device-key");
        } else {
            request.set_header("Authorization", "Bearer junk");
        }
        crate::web::set_request(std::sync::Arc::new(request), bytes::Bytes::new());
        crate::user::session::reset_session();
        crate::user::internal_login_user("keys.ann").unwrap();
        let called = crate::screen::gate::with_request_frame(
            crate::screen::RenderOrigin::Browser,
            signed_in_by_header,
            false,
            || crate::service::call::sync(&format!("{SERVICES}.create#UserScopedKey"), &BTreeMap::new()),
        );
        let errors = ExecutionContext::with_current(|ctx| {
            let text = ctx.message().errors_text().join("\n");
            ctx.message().clear();
            text
        })
        .unwrap_or_default();
        crate::user::logout().unwrap();
        called.ok().is_some_and(|out| out.contains_key("loginKey")) && errors.is_empty()
    };
    assert!(
        !made_by(false),
        "a session with no recent sign-in, a junk Authorization header beside it"
    );
    assert!(made_by(true), "a client by its own key");
}

/// A git key and a tasks key (plans/archive/design-dev-aide.md question 18) each sign in only where a REST resource declares
/// their scope (`key-scope`), an application's as the framework's, and neither where it declares the other's or none;
/// signed in by one, its holder ends that key alone (SEC-RTE-85).
#[test]
fn git_and_tasks_keys_open_only_the_resources_that_declare_them_and_end_only_themselves() {
    let dir = std::env::temp_dir().join(format!("moqui-key-scopes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tasks_rest = dir.join("mantle.rest.xml");
    std::fs::write(
        &tasks_rest,
        r#"<resource name="mantle">
    <resource name="my"><resource name="tasks" key-scope="tasks"><method type="get"><service name="a.B.get#C"/></method></resource>
        <resource name="noticeCounts"><method type="get"><service name="a.B.get#D"/></method></resource></resource>
    <resource name="gitRepositories"><id name="repositoryId"><resource name="issues" key-scope="tasks">
        <method type="get"><service name="a.B.get#E"/></method>
        <id name="issueId"><method type="get"><service name="a.B.get#F"/></method></id></resource></id></resource>
    <resource name="docs"><id name="docPath" allow-extra-path="true" key-scope="tasks">
        <method type="get"><service name="a.B.get#G"/></method></id></resource>
</resource>"#,
    )
    .unwrap();
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    crate::service::install_rest(context.factory(), &[framework_rest(), tasks_rest]);
    let _ = std::fs::remove_dir_all(&dir);
    {
        let _authz = context.disable_authz();
        let mut row = facade.make_value("moqui.security.UserAccount").unwrap();
        row.set("userId", Value::string("keys.dee"));
        row.set("username", Value::string("keys.dee"));
        row.create(&facade).unwrap();
    }
    let make = |scope: &str| {
        let (made, errors) = call_as(
            "keys.dee",
            "create#UserScopedKey",
            &[
                ("scope", Value::string(scope)),
                ("deviceName", Value::string(format!("Dev AIDE {scope}"))),
            ],
        );
        assert!(errors.is_empty(), "{scope}: {errors}");
        to_plain_string(&made["loginKey"])
    };
    let (git, tasks, gateway) = (make("git"), make("tasks"), make("gateway"));
    let signs_in = |key: &str, path: &str| {
        let scopes = crate::user::request_key_scopes(path);
        let scopes: Vec<&str> = scopes.iter().map(String::as_str).collect();
        let ok = crate::user::apply_header_keys_scoped(Some(key), None, &scopes).is_ok();
        if ok {
            crate::user::logout().unwrap();
        }
        ok
    };
    for path in [
        "/rest/s1/mantle/my/tasks",
        "/rest/s1/mantle/gitRepositories/R1/issues",
        "/rest/s1/mantle/gitRepositories/R1/issues/12",
        // an id that takes the extra path, and the dispatcher's fallback to it
        "/rest/s1/mantle/docs/a/b/c.md",
        "/rest/s1/mantle/docs",
    ] {
        assert!(signs_in(&tasks, path), "the tasks key on {path}");
        assert!(!signs_in(&git, path), "the git key on {path}");
    }
    for path in [
        "/rest/s1/moqui/git/R1/mergeRequests",
        "/rest/s1/moqui/hosting/repository",
    ] {
        assert!(signs_in(&git, path), "the git key on {path}");
        assert!(!signs_in(&tasks, path), "the tasks key on {path}");
    }
    for path in [
        "/rest/s1/mantle/my/noticeCounts",
        "/rest/s1/mantle/gitRepositories/R1",
        "/rest/s1/moqui/hosting/gatewayKey",
        "/rest/s1/mantle/my/tasks/../noticeCounts",
        "/apps/my/tasks",
    ] {
        assert!(!signs_in(&tasks, path), "the tasks key on {path}");
    }
    assert!(crate::user::login_user_key(&tasks).is_err(), "no account login");
    crate::user::login_user_key_for(&git, crate::user::ACCESS_TOKEN_SCOPE).unwrap();
    crate::user::logout().unwrap();

    // signed in by the tasks key, its holder ends it and not the gateway's, which it knows too
    let revoke_by = |by: &str, key: &str| {
        crate::user::apply_header_keys_scoped(Some(by), None, &["git", "tasks", "gateway"]).unwrap();
        let parameters: BTreeMap<String, Value> = [("loginKey".to_string(), Value::string(key))].into();
        let out =
            crate::service::call::sync(&format!("{SERVICES}.revoke#UserHeldKey"), &parameters).unwrap_or_default();
        let errors = ExecutionContext::with_current(|ctx| {
            let text = ctx.message().errors_text().join("\n");
            ctx.message().clear();
            text
        })
        .unwrap_or_default();
        crate::user::logout().unwrap();
        (out.get("revoked") == Some(&Value::Bool(true)), errors)
    };
    let (revoked, errors) = revoke_by(&tasks, &gateway);
    assert!(!revoked && errors.contains("ends only itself"), "{errors}");
    crate::user::login_user_key_for(&gateway, "gateway").unwrap();
    crate::user::logout().unwrap();
    let (revoked, errors) = revoke_by(&tasks, &tasks);
    assert!(revoked, "{errors}");
    assert!(!signs_in(&tasks, "/rest/s1/mantle/my/tasks"), "ended");
    assert!(revoke_by(&gateway, &gateway).0);
    assert!(revoke_by(&git, &git).0);
}

/// An SSH key, an access token and an app password are made only after a sign-in in the last five minutes (SEC-ADM-03,
/// SEC-ADM-07, SEC-COM-21), so a session or key that came without one cannot make itself lasting credentials. A token
/// lasts at most 366 days and is git's alone (SEC-PFS-61): git and the git REST take it, every other login refuses it, as
/// it does a token made before tokens had a scope.
#[test]
fn credentials_need_a_recent_sign_in_and_an_access_token_is_for_git_alone() {
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    {
        let _authz = context.disable_authz();
        let mut row = facade.make_value("moqui.security.UserAccount").unwrap();
        row.set("userId", Value::string("keys.cat"));
        row.set("username", Value::string("keys.cat"));
        row.create(&facade).unwrap();
    }
    let text = |value: &str| Value::string(value);
    // an internal login (as a key's or a stolen session's) is no recent sign-in: each is refused, nothing made
    crate::user::internal_login_user("keys.cat").unwrap();
    for (name, parameters) in [
        ("create#UserSshKey", vec![("publicKey", text(&key_line(9)))]),
        ("create#UserAccessToken", vec![]),
        (
            "create#UserAppPassword",
            vec![("name", text("phone")), ("scope", text("dav"))],
        ),
    ] {
        let (out, refused) = call_as("", name, &parameters);
        assert!(refused.contains("Confirm it is you"), "{name}: {refused}");
        assert!(
            out.values().all(|value| matches!(value, Value::Null)),
            "{name}: {out:?}"
        );
    }
    crate::user::logout().unwrap();

    let (_, refused) = call_as("keys.cat", "create#UserAccessToken", &[("expireDays", text("400"))]);
    assert!(refused.contains("at most 366"), "{refused}");
    let (made, errors) = call_as("keys.cat", "create#UserAccessToken", &[("description", text("ci"))]);
    assert!(errors.is_empty(), "{errors}");
    let token = to_plain_string(&made["accessToken"]);
    let refused_everywhere = |token: &str| {
        let account = crate::user::login_user_key(token).is_err();
        let rest = crate::user::apply_header_keys(Some(token), None, None).is_err();
        let socket = crate::user::login_key_user_by_hash(&crate::user::hash_login_key(token).unwrap())
            .unwrap()
            .is_none();
        account && rest && socket
    };
    assert!(refused_everywhere(&token), "a token opens no login but git's");
    crate::user::login_user_key_for(&token, crate::user::ACCESS_TOKEN_SCOPE).unwrap();
    assert_eq!(crate::user::login_key_kind(), Some(crate::user::LoginKeyKind::Token));
    crate::user::logout().unwrap();
    // the scopes a path takes are what its REST resource declares (key-scope): the framework's own here
    crate::service::install_rest(context.factory(), &[framework_rest()]);
    let scopes = |path: &str| crate::user::request_key_scopes(path).to_vec();
    let git_scopes = scopes("/rest/s1/moqui/git/R1/commits");
    let git_scopes: Vec<&str> = git_scopes.iter().map(String::as_str).collect();
    assert!(
        crate::user::apply_header_keys_scoped(Some(&token), None, &git_scopes).unwrap(),
        "the git REST takes it"
    );
    crate::user::logout().unwrap();
    assert_eq!(scopes("/rest/s1/moqui/hosting/repository"), ["git"]);
    assert!(scopes("/rest/s1/moqui/git/../../mantle/orders").is_empty());
    assert!(scopes("/apps/git").is_empty());
    // SEC-RTE-85: a token for git alone makes no key for another service, and ends no other key
    assert_eq!(scopes("/rest/s1/moqui/hosting/offer"), ["git"]);
    assert!(scopes("/rest/s1/moqui/hosting/gatewayKey").is_empty());
    assert_eq!(scopes("/rest/s1/moqui/hosting/keyRevoke"), ["git", "tasks", "gateway", "terminal"]);
    // the terminal's driving paths take a key made for the terminal alone (plans/Terminal-Fancy2.md §4.3)
    assert_eq!(scopes("/rest/s1/moqui/terminal/s1/screen"), ["terminal"]);

    // a token made before tokens had a scope is git's too
    {
        let _authz = context.disable_authz();
        facade
            .find("moqui.security.UserLoginKey")
            .unwrap()
            .eq("loginKey", Value::string(crate::user::hash_login_key(&token).unwrap()))
            .update_all([("scope".to_string(), Value::Null)])
            .unwrap();
    }
    assert!(refused_everywhere(&token), "an older token opens no login but git's");
    crate::user::login_user_key_for(&token, crate::user::ACCESS_TOKEN_SCOPE).unwrap();
    crate::user::logout().unwrap();
}

/// A scoped key's login runs its request as the key's user while the web session keeps its own identity (none here):
/// the session moves to no new id, the request declines it, the HTTP log names the user, and the session bound again
/// later has nobody in it.
#[test]
fn a_scoped_keys_login_is_held_for_its_request_and_never_the_sessions() {
    let (facade, context) = entered();
    let _entered = context.enter();
    crate::service::load_definitions().unwrap();
    {
        let _authz = context.disable_authz();
        let mut row = facade.make_value("moqui.security.UserAccount").unwrap();
        row.set("userId", Value::string("keys.eve"));
        row.set("username", Value::string("keys.eve"));
        row.create(&facade).unwrap();
    }
    let (made, errors) = call_as("keys.eve", "create#UserScopedKey", &[("scope", Value::string("tasks"))]);
    assert!(errors.is_empty(), "{errors}");
    let key = to_plain_string(&made["loginKey"]);
    // an anonymous session the client already holds
    let held = {
        let guard = crate::user::bind(context.factory(), None, None, false).unwrap();
        crate::user::session::set_visit("visit-eve", "visitor-eve");
        // a password sign-in waiting for its second factor, which the key's login leaves as it is
        crate::user::session::set_session_attribute("moquiPreAuthcUsername", Some("keys.eve"));
        guard.session_id()
    };
    {
        let _request = crate::user::session::RequestSessionScope::begin();
        let guard = crate::user::session::bind_id(context.factory(), Some(&held), false).unwrap();
        assert!(crate::user::apply_header_keys_scoped(Some(&key), None, &["tasks"]).unwrap());
        assert_eq!(crate::user::current_username().unwrap().as_deref(), Some("keys.eve"));
        assert_eq!(
            crate::user::session::stored_user_id(),
            None,
            "the session never holds it"
        );
        assert!(crate::user::session::session_declined());
        assert_eq!(guard.session_id(), held, "no new id");
        assert_eq!(
            crate::user::session::http_log_identity().username.as_deref(),
            Some("keys.eve")
        );
        // what the request writes to the session's attributes is the request's
        crate::user::note_header_login();
    }
    assert!(!crate::user::session::identity_held(), "the hold ends with its request");
    {
        let _guard = crate::user::session::bind_id(context.factory(), Some(&held), false).unwrap();
        assert_eq!(
            crate::user::current_user_id().unwrap(),
            None,
            "the held session has nobody in it"
        );
        assert_eq!(
            crate::user::session::session_attribute("moquiPreAuthcUsername").as_deref(),
            Some("keys.eve"),
            "the pending second factor is the session's still"
        );
        assert_eq!(crate::user::session::session_attribute("moquiHeaderLogin"), None);
    }
    // a key whose login fails (its account disabled) leaves the session's own identity, and no hold
    {
        let _authz = context.disable_authz();
        facade
            .find("moqui.security.UserAccount")
            .unwrap()
            .eq("userId", Value::string("keys.eve"))
            .update_all([("disabled".to_string(), Value::string("Y"))])
            .unwrap();
    }
    let _request = crate::user::session::RequestSessionScope::begin();
    let _guard = crate::user::session::bind_id(context.factory(), Some(&held), false).unwrap();
    assert!(crate::user::apply_header_keys_scoped(Some(&key), None, &["tasks"]).is_err());
    assert!(!crate::user::session::identity_held());
    assert_eq!(crate::user::current_user_id().unwrap(), None);
}
