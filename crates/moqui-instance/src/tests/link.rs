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

//! M2's rules on the manager's rows (plans/Vault-Instances.md §6): a pairing token or code works once, in time, for its
//! instance only, and pins the instance's link key and its environment's approvers; a key not pinned is not linked; a
//! code is made only for INSTANCE_ADMIN; and SSH, the other transport, runs only operator commands with the key in a
//! file of its own and the value on standard input.

use std::os::unix::fs::PermissionsExt as _;

use serde_json::json;
use sha2::Digest as _;

use super::{create, field_of, login, new_id, setup, sys};
use crate::link;

const INSTANCE: &str = "moqui.server.instance.AppInstance";

fn hash(secret: &str) -> String {
    moqui_vault::text::hex(&sha2::Sha256::digest(secret.as_bytes()))
}

/// An environment whose approver group has one member with a personal key, and two instances with their tokens.
fn rows(facade: &moqui_core::entity::EntityFacade, tag: &str) -> (String, [u8; 32]) {
    let group = format!("IT_APPR_{tag}");
    let user = format!("appr-{tag}");
    let environment = format!("Env-{tag}");
    create(
        facade,
        "moqui.security.UserAccount",
        json!({ "userId": user, "username": user }),
    );
    create(facade, "moqui.security.UserGroup", json!({ "userGroupId": group }));
    create(
        facade,
        "moqui.security.UserGroupMember",
        json!({ "userGroupId": group, "userId": user, "fromDate": 0 }),
    );
    let verify = *moqui_vault::sign::SigningKey::from_seed(&moqui_vault::Key32::from_bytes([9; 32]))
        .verifying_key()
        .as_bytes();
    create(
        facade,
        "moqui.security.vault.VaultPersonalKey",
        json!({ "personalKeyId": new_id("pk"), "userId": user, "verifyPublic": moqui_vault::text::hex(&verify),
                "encryptPublic": "00", "statusId": "VpkActive" }),
    );
    create(
        facade,
        "moqui.server.instance.InstanceEnvironment",
        json!({ "environmentId": environment, "approverUserGroupId": group }),
    );
    (environment, verify)
}

fn instance(facade: &moqui_core::entity::EntityFacade, id: &str, environment: &str, token: &str, in_ms: i64) {
    create(
        facade,
        INSTANCE,
        json!({ "appInstanceId": id, "instanceKindId": "mr-moqui", "environmentId": environment,
                "pairingTokenHash": hash(token),
                "pairingTokenExpires": moqui_core::user::now_millis() + in_ms }),
    );
}

#[test]
fn a_pairing_token_works_once_in_time_for_its_instance_only_and_pins_the_approvers() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let (environment, verify) = rows(&facade, "pair");
    let bundle_key = [7_u8; 32];
    instance(&facade, "pair-a", &environment, "token-a", 60_000);
    instance(&facade, "pair-b", &environment, "token-b", 60_000);
    instance(&facade, "pair-c", &environment, "token-c", -1_000);

    let paired = sys(|| link::pair(&facade, "token-a", "fp-a", &bundle_key)).unwrap();
    assert_eq!(paired.app_instance_id, "pair-a");
    assert_eq!(paired.environment_id, environment);
    assert_eq!(paired.approvers.version, 1);
    assert_eq!(paired.approvers.approvers.len(), 1);
    assert_eq!(paired.approvers.approvers[0].verify_public, verify);
    // the token is spent: its hash is gone from the row, and the link key and bundle key are pinned
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", "pair-a", "pairingTokenHash"),
        ""
    );
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", "pair-a", "linkKeyFingerprint"),
        "fp-a"
    );
    let pinned = sys(|| link::pinned_approvers(&link::instance(&facade, "pair-a").unwrap()));
    assert_eq!(pinned, paired.approvers);

    // once: again it is refused; and it pairs no other instance
    let again = sys(|| link::pair(&facade, "token-a", "fp-x", &bundle_key)).unwrap_err();
    assert!(again.message().contains("unknown, used or expired"), "{again}");
    let other = sys(|| link::pair(&facade, "token-b", "fp-b", &bundle_key)).unwrap();
    assert_eq!(other.app_instance_id, "pair-b");
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", "pair-a", "linkKeyFingerprint"),
        "fp-a"
    );
    // in time: an expired token is refused and cleared
    assert!(sys(|| link::pair(&facade, "token-c", "fp-c", &bundle_key)).is_err());
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", "pair-c", "pairingTokenHash"),
        ""
    );
    // a bundle key that is not one, and an empty secret, are refused without a look-up
    assert!(sys(|| link::pair(&facade, "token-x", "fp", &[1, 2])).is_err());
    assert!(sys(|| link::pair(&facade, "", "fp", &bundle_key)).is_err());

    // an unpinned key is no instance's; a revoked one is no longer
    let linked = sys(|| link::linked(&facade, "fp-a")).unwrap().unwrap();
    assert_eq!(linked.app_instance_id, "pair-a");
    assert_eq!(linked.bundle_key, bundle_key);
    assert!(sys(|| link::linked(&facade, "fp-unknown")).unwrap().is_none());
    sys(|| link::revoke(&facade, "pair-a")).unwrap();
    assert!(sys(|| link::linked(&facade, "fp-a")).unwrap().is_none());
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", "pair-a", "linkState"),
        "revoked"
    );
}

#[test]
fn a_pairing_code_is_made_for_instance_admin_only_and_pairs_once() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let (environment, _) = rows(&facade, "code");
    instance(&facade, "code-a", &environment, "unused", 60_000);
    create(
        &facade,
        "moqui.security.UserAccount",
        json!({ "userId": "code-nobody", "username": "code-nobody" }),
    );
    login("code-nobody");
    let refused = link::make_pairing_code(&facade, "code-a").unwrap_err();
    assert!(refused.message().contains("INSTANCE_ADMIN"), "{refused}");
    let code = sys(|| link::make_pairing_code(&facade, "code-a")).unwrap();
    assert_eq!(code.len(), 32);
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", "code-a", "pairingTokenHash"),
        hash(&code)
    );
    let paired = sys(|| link::pair(&facade, &code, "fp-code", &[7; 32])).unwrap();
    assert_eq!(paired.app_instance_id, "code-a");
    assert!(sys(|| link::pair(&facade, &code, "fp-code", &[7; 32])).is_err());
}

#[test]
fn ssh_runs_only_operator_commands_with_its_key_in_a_file_of_its_own_and_the_value_on_stdin() {
    use crate::ssh::{remote_command, run, SshCommand, SshTarget};
    assert_eq!(
        remote_command(&SshCommand::Status).unwrap(),
        "'moqui' 'operator' 'status'"
    );
    let put = SshCommand::PutSecret {
        name: "webapp-list/webapp[webroot]/session-config@cookie-key".to_string(),
        value: zeroize::Zeroizing::new("the value".to_string()),
        force: true,
    };
    let remote = remote_command(&put).unwrap();
    assert!(
        remote.ends_with("'--force'") && !remote.contains("the value"),
        "{remote}"
    );
    for name in ["a b", "a'b", "a;b", ""] {
        let bad = SshCommand::PutSecret {
            name: name.to_string(),
            value: zeroize::Zeroizing::new(String::new()),
            force: false,
        };
        assert!(remote_command(&bad).is_err(), "{name:?}");
    }

    // a stub ssh records its arguments, the key file's mode and its standard input
    let dir = std::env::temp_dir().join(format!("moqui-ssh-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let stub = dir.join("ssh");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\nout='{}'\necho \"$@\" > \"$out/args\"\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = -i ]; then \
             ls -l \"$2\" > \"$out/key\"; echo \"$2\" > \"$out/keypath\"; fi\n  shift\ndone\ncat > \"$out/stdin\"\n\
             echo ok\n",
            dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
    let target = SshTarget {
        host: "box.example".to_string(),
        port: 2222,
        user: "moqui".to_string(),
        host_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample".to_string(),
    };
    let printed = run(&stub.display().to_string(), &target, b"PRIVATE KEY", &put).unwrap();
    assert_eq!(printed.trim(), "ok");
    let args = std::fs::read_to_string(dir.join("args")).unwrap();
    // no conf file and no known host but the pinned one; the user by -l, and the host after `--`
    for expected in [
        "-F /dev/null",
        "StrictHostKeyChecking=yes",
        "GlobalKnownHostsFile=/dev/null",
        "-l moqui -- box.example 'moqui'",
    ] {
        assert!(args.contains(expected), "{expected}: {args}");
    }
    assert!(!args.contains("the value") && !args.contains("PRIVATE KEY"), "{args}");
    assert!(std::fs::read_to_string(dir.join("key"))
        .unwrap()
        .starts_with("-rw-------"));
    assert_eq!(std::fs::read_to_string(dir.join("stdin")).unwrap(), "the value\n");
    let key_path = std::fs::read_to_string(dir.join("keypath")).unwrap();
    assert!(
        !std::path::Path::new(key_path.trim()).exists(),
        "the key file outlived the call"
    );

    // what ssh would read as an option, or what would add a line to the pinned known_hosts file, is refused before ssh
    // runs
    let calls = std::fs::read_to_string(dir.join("args")).unwrap();
    for (host, user, key) in [
        ("-oProxyCommand=touch /tmp/x", "moqui", "ssh-ed25519 AAAA"),
        ("box.example", "-oProxyCommand=x", "ssh-ed25519 AAAA"),
        ("box example", "moqui", "ssh-ed25519 AAAA"),
        ("box.example\nother", "moqui", "ssh-ed25519 AAAA"),
        ("box.example", "mo qui", "ssh-ed25519 AAAA"),
        (
            "box.example",
            "moqui",
            "ssh-ed25519 AAAA\nevil.example ssh-ed25519 BBBB",
        ),
        ("box.example", "moqui", "ssh-ed25519 AAAA comment"),
        ("box.example", "moqui", "not-a-key AAAA"),
    ] {
        let bad = SshTarget {
            host: host.to_string(),
            port: 22,
            user: user.to_string(),
            host_key: key.to_string(),
        };
        assert!(
            run(&stub.display().to_string(), &bad, b"K", &put).is_err(),
            "{host:?} {user:?} {key:?}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(dir.join("args")).unwrap(),
        calls,
        "ssh ran for a refused target"
    );
    let ip = SshTarget {
        host: "2001:db8::1".to_string(),
        port: 22,
        user: "moqui".to_string(),
        host_key: "ecdsa-sha2-nistp256 AAAA+/=".to_string(),
    };
    assert!(ip.check().is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_pairings_with_one_token_at_once_pair_once() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let (environment, _) = rows(&facade, "race");
    instance(&facade, "race-a", &environment, "token-race", 60_000);
    let factory = std::sync::Arc::clone(ctx.factory());
    let start = std::sync::Arc::new(std::sync::Barrier::new(4));
    let tries: Vec<_> = (0..4)
        .map(|n| {
            let (factory, facade, start) = (
                std::sync::Arc::clone(&factory),
                facade.clone(),
                std::sync::Arc::clone(&start),
            );
            std::thread::spawn(move || {
                let context = factory.new_context();
                let _entered = context.enter();
                start.wait();
                sys(|| link::pair(&facade, "token-race", &format!("fp-race-{n}"), &[7; 32])).is_ok()
            })
        })
        .collect();
    let paired = tries
        .into_iter()
        .map(|try_| try_.join().unwrap())
        .filter(|paired| *paired)
        .count()
        == 1;
    assert!(paired, "one token paired more than once, or never");
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", "race-a", "pairingTokenHash"),
        ""
    );
}

#[test]
fn a_caller_outside_instance_admin_or_the_environment_group_is_refused() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let (environment, _) = rows(&facade, "authz");
    instance(&facade, "authz-a", &environment, "unused-authz", 60_000);
    // an INSTANCE_ADMIN who is not in the environment's approver group, and a user with neither
    create(
        &facade,
        "moqui.security.UserAccount",
        json!({ "userId": "authz-admin", "username": "authz-admin" }),
    );
    create(
        &facade,
        "moqui.security.UserGroup",
        json!({ "userGroupId": "IT_AUTHZ_ADMINS" }),
    );
    create(
        &facade,
        "moqui.security.UserGroupMember",
        json!({ "userGroupId": "IT_AUTHZ_ADMINS", "userId": "authz-admin", "fromDate": 0 }),
    );
    create(
        &facade,
        "moqui.security.UserGroupPermission",
        json!({ "userGroupId": "IT_AUTHZ_ADMINS", "userPermissionId": "INSTANCE_ADMIN", "fromDate": 0 }),
    );
    create(
        &facade,
        "moqui.security.UserAccount",
        json!({ "userId": "authz-none", "username": "authz-none" }),
    );
    for user in ["authz-admin", "authz-none"] {
        login(user);
        let row = link::instance(&facade, "authz-a").unwrap();
        assert!(link::check_caller(&facade, &row).is_err(), "{user} passed");
        assert!(
            link::make_pairing_code(&facade, "authz-a").is_err(),
            "{user} made a code"
        );
        assert!(link::revoke(&facade, "authz-a").is_err(), "{user} revoked");
        let set = moqui_vault::approvers::ApproverSet {
            version: 2,
            approvers: Vec::new(),
        };
        assert!(link::check_approver_update(&facade, "authz-a", &set, &[0; 32], &[0; 64]).is_err());
        sys(|| moqui_core::user::logout_local().unwrap());
    }
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", "authz-a", "pairingTokenHash"),
        hash("unused-authz")
    );
}
