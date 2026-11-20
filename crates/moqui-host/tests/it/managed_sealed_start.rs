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

//! `ManagedSealedStart` (plans/Vault-Instances.md §6, M6), an acceptance test: a manager (a seeded server in this
//! process, taking `moqui-manage/1` on its HTTPS port) and a managed instance's sealed start as its gate runs it (the
//! dialer the gate starts, the manager's transit installed, a production seal kept by its keeper), on runtimes under
//! this test's. Each "restart" is the instance's `db` files (seal.json, its link key, its pairing) on a new runtime
//! directory, as a new process would find them.
//!
//! 1. The first start makes the seal with the manager's transit, takes its unseal by the manager once, and sends its
//!    recovery shares to the three holders whose keys the pinned approver checked, one each; each holder opens theirs
//!    against the instance's pinned link key and confirms it.
//! 2. A restart unseals by transit, with the same root key, and finds every share confirmed.
//! 3. With the manager out of reach the instance stays sealed (auto-unseal failed), and the manager's link-down gauge,
//!    which the shipped alert rule reads, fires; two holders' shares, entered on the operator socket, unseal it.
//! 4. Under the approval policy the transit holds the unseal; two holders seal their shares to the key the sealed
//!    instance made for this unseal (`moqui vault seal-share`'s steps), the manager relays them, and they unseal it.
//!
//! Run alone: `cargo test -p moqui-host --test it managed_sealed_start -- --ignored` (about two minutes).

#![cfg(all(unix, feature = "manage", feature = "instances", feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use moqui_core::seal::{Mode, SealConf, Start};
use moqui_lib::object::{to_plain_string, Value};
use moqui_vault::fingerprint::KeyCheck;
use moqui_vault::personal::PersonalKey;
use moqui_vault::rng::OsRng;
use moqui_vault::text::hex;
use sha2::Digest as _;
use zeroize::Zeroizing;

use crate::common::Gate;

const SERVICES: &str = "org.moqui.dev.InstanceLinkServices";
const TOKEN: &str = "a-pairing-token-of-the-acceptance-test";
const HOLDERS: [&str; 3] = ["ms.holder1", "ms.holder2", "ms.holder3"];

fn pem(label: &str, der: &[u8]) -> String {
    use base64::Engine as _;
    let text = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = text
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

/// The manager's rows: an environment whose method is the manager's transit, an approver with a personal key, and
/// three recovery holders whose keys that approver checked.
fn data(approver: &PersonalKey, holders: &[PersonalKey]) -> String {
    let mut rows = format!(
        r#"<entity-facade-xml type="seed">
    <moqui.security.UserAccount userId="MS_APPROVER" username="ms.approver"/>
    <moqui.security.UserGroup userGroupId="MS_APPROVERS"/>
    <moqui.security.UserGroupMember userGroupId="MS_APPROVERS" userId="MS_APPROVER" fromDate="2020-01-01 00:00:00"/>
    <moqui.security.vault.VaultPersonalKey personalKeyId="MS_PK_APPROVER" userId="MS_APPROVER" statusId="VpkActive"
            encryptPublic="{}" verifyPublic="{}"/>
    <moqui.security.UserGroup userGroupId="MS_HOLDERS"/>
    <moqui.security.ArtifactGroup artifactGroupId="MS_LINK_SERVICES"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="MS_LINK_SERVICES" artifactTypeEnumId="AT_SERVICE" inheritAuthz="Y"
            nameIsPattern="Y" artifactName="org\.moqui\.dev\.InstanceLinkServices\..*"/>
    <moqui.security.ArtifactAuthz artifactAuthzId="MS_LINK_SERVICES_ALL" userGroupId="ALL_USERS"
            artifactGroupId="MS_LINK_SERVICES" authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL"/>
    <moqui.server.instance.InstanceEnvironment environmentId="MS_ENV" approverUserGroupId="MS_APPROVERS"
            recoveryUserGroupId="MS_HOLDERS" transportEnumId="ItrLink" unsealPolicyEnumId="IupAutomatic"
            unsealMethod="transit"/>
    <moqui.server.instance.AppInstance appInstanceId="MS_APP" instanceKindId="mr-moqui" environmentId="MS_ENV" instanceUuid="ms-container"
            pairingTokenHash="{}" pairingTokenExpires="2099-01-01 00:00:00"/>
"#,
        hex(approver.public().encrypt.as_bytes()),
        hex(approver.public().verify.as_bytes()),
        hex(&sha2::Sha256::digest(TOKEN.as_bytes())),
    );
    for (n, (username, key)) in HOLDERS.iter().zip(holders).enumerate() {
        let user = format!("MS_HOLDER{}", n + 1);
        let key_id = format!("MS_PK_HOLDER{}", n + 1);
        let check = KeyCheck::of_personal(
            "MS_APPROVER",
            "MS_PK_APPROVER",
            approver,
            &user,
            &key_id,
            &key.public(),
            1_700_000_000_000,
        );
        rows.push_str(&format!(
            r#"    <moqui.security.UserAccount userId="{user}" username="{username}"/>
    <moqui.security.UserGroupMember userGroupId="MS_HOLDERS" userId="{user}" fromDate="2020-01-01 00:00:00"/>
    <moqui.security.vault.VaultPersonalKey personalKeyId="{key_id}" userId="{user}" statusId="VpkActive"
            encryptPublic="{}" verifyPublic="{}"/>
    <moqui.security.vault.VaultKeyCheck keyCheckId="MS_CHECK{n}" checkerUserId="MS_APPROVER" checkerKeyId="MS_PK_APPROVER"
            checkedKeyId="{key_id}" checkedUserId="{user}" checkedEncrypt="{}" checkedVerify="{}" checkedAt="{}"
            signature="{}"/>
"#,
            hex(key.public().encrypt.as_bytes()),
            hex(key.public().verify.as_bytes()),
            hex(&check.checked_encrypt),
            hex(&check.checked_verify),
            check.checked_at,
            hex(&check.signature),
        ));
    }
    rows.push_str("</entity-facade-xml>");
    rows
}

/// A service of InstanceLinkServices on the manager as `username`; as an administrator with authorization off when
/// none.
fn call(
    gate: &Gate,
    username: Option<&str>,
    verb_noun: &str,
    parameters: &[(&str, &str)],
) -> Result<BTreeMap<String, Value>, String> {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user(username.unwrap_or("john.doe")).unwrap();
    let _authz = username.is_none().then(|| context.disable_authz());
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), Value::string(*value)))
        .collect();
    let out =
        moqui_core::service::sync(&format!("{SERVICES}.{verb_noun}"), &parameters).map_err(|err| err.to_string())?;
    let errors = moqui_core::context::ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default();
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

/// Rows of `entity` on the manager, by a field, as their fields' text.
fn rows(gate: &Gate, entity: &str, field: &str, value: &str) -> Vec<BTreeMap<String, String>> {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    facade
        .find(entity)
        .unwrap()
        .eq(field, Value::string(value))
        .list()
        .unwrap()
        .iter()
        .map(|row| {
            row.fields()
                .iter()
                .map(|(name, value)| (name.to_string(), to_plain_string(value)))
                .collect()
        })
        .collect()
}

fn wait_until(what: &str, limit: Duration, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "{what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The instance's conf: its manager, the pin, and its pairing token's file.
fn conf(url: &str, pin: &str, token_file: &Path) -> moqui_lib::node::MNode {
    moqui_lib::node::MNode::parse_text(
        "managed-conf",
        &format!(
            r#"<moqui-conf><manage url="{url}" pin="{pin}" token-file="{}"/></moqui-conf>"#,
            token_file.display()
        ),
    )
    .unwrap()
}

fn seal_conf() -> SealConf {
    let mut seal = SealConf::with_purpose("production");
    seal.method = "transit".to_string();
    seal.shares = 3;
    seal.threshold = 2;
    seal
}

/// The instance's `db` files on a new runtime directory: a restart as a new process finds them.
fn restart_at(from: &Path, to: &Path) {
    std::fs::create_dir_all(to.join("db")).unwrap();
    for name in ["seal.json", "manage_link_key", "manage_link.json"] {
        std::fs::copy(from.join("db").join(name), to.join("db").join(name)).unwrap();
    }
}

fn link_state(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("db/manage_link.json")).unwrap_or_default())
        .unwrap_or_default()
}

fn stop_dialer(dir: &Path) {
    if let Some(dialer) = moqui_host::manage_link::dial::dialer_of(dir) {
        dialer.stop();
    }
}

#[test]
#[ignore = "acceptance: ManagedSealedStart (plans/Vault-Instances.md §6, M6); about two minutes"]
fn managed_sealed_start() {
    if !crate::common::own_process() {
        return;
    }
    let approver = PersonalKey::generate(&mut OsRng);
    let holders: Vec<PersonalKey> = (0..3).map(|_| PersonalKey::generate(&mut OsRng)).collect();
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let port = crate::common::free_port();
    let port_text = port.to_string();
    let gate = crate::common::start_with_files(
        "managed-sealed",
        Some(&data(&approver, &holders)),
        &[(
            "moqui_conf__webapp_list__webapp__WEBROOT__https_listen_port",
            &port_text,
        )],
        &[
            ("cert/default.crt", &pem("CERTIFICATE", cert.der())),
            ("cert/default.key", &pem("PRIVATE KEY", &key.serialize_der())),
        ],
    );
    let url = format!("https://localhost:{port}");
    let pin = to_plain_string(
        &call(&gate, None, "get#InstanceLink", &[("appInstanceId", "MS_APP")]).unwrap()["managerFingerprint"],
    );
    let base: PathBuf = gate.runtime.join("managed");
    let token_file = base.join("pairing-token");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(&token_file, TOKEN).unwrap();
    let conf = conf(&url, &pin, &token_file);

    // 1. the first start: the seal by the manager's transit, its unseal taken once, the shares to the holders
    let first = base.join("start-1");
    std::fs::create_dir_all(first.join("db")).unwrap();
    // as the gate does: the keeper first, then the dialer, then the seal opened
    let keeper = moqui_core::seal::keeper_for(&first, seal_conf()).unwrap();
    moqui_host::manage_link::dial::start_for_gate(&first, &conf);
    let dialer = moqui_host::manage_link::dial::dialer_of(&first).expect("the gate started the dialer");
    let started = Instant::now();
    while dialer.status().phase != moqui_host::manage_link::LinkPhase::Linked {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "not linked: {:?}",
            dialer.status()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(keeper.start(Mode::Operator).unwrap(), Start::Unsealed);
    let root_id = keeper.status().root_id;
    let sealed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(first.join("db/seal.json")).unwrap()).unwrap();
    assert_eq!(sealed["method"]["kind"], "transit");
    assert_eq!(sealed["method"]["backend"], moqui_host::manage_link::unseal::BACKEND);
    let started = Instant::now();
    loop {
        let link = call(&gate, None, "get#InstanceLink", &[("appInstanceId", "MS_APP")]).unwrap();
        if to_plain_string(&link["unsealVerified"]) == "true" {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the manager did not take its unsealed: {link:?}, the link {:?}, actions {:?}",
            dialer.status(),
            rows(&gate, "moqui.server.instance.InstanceAction", "appInstanceId", "MS_APP")
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    wait_until("the shares sent", Duration::from_secs(60), || {
        rows(
            &gate,
            "moqui.server.instance.InstanceRecoveryShare",
            "appInstanceId",
            "MS_APP",
        )
        .len()
            == 3
    });
    assert!(!keeper.status().shares_waiting, "the held shares were taken");
    // each holder opens their own share against the instance's pinned link key, and confirms it
    let instance = &rows(&gate, "moqui.server.instance.AppInstance", "appInstanceId", "MS_APP")[0];
    let link = moqui_vault::managed::pinned_link_key(
        &moqui_vault::text::unhex(&instance["linkSignPublic"]).unwrap(),
        &instance["linkKeyFingerprint"],
    )
    .unwrap();
    let mut shares = Vec::new();
    for (n, holder) in holders.iter().enumerate() {
        let user = format!("MS_HOLDER{}", n + 1);
        let sent = rows(&gate, "moqui.security.vault.VaultDelivery", "recipientUserId", &user);
        assert_eq!(sent.len(), 1, "one share to each holder");
        let row = &sent[0];
        let delivery = moqui_vault::delivery::Delivery {
            delivery_id: row["deliveryId"].clone(),
            recipient_user_id: row["recipientUserId"].clone(),
            recipient_key_id: row["recipientKeyId"].clone(),
            purpose: row["purpose"].clone(),
            sender_kind: row["senderKind"].clone(),
            sender_id: row["senderId"].clone(),
            sealed: moqui_vault::text::unhex(&row["sealedValue"]).unwrap(),
            signature: moqui_vault::text::unhex(&row["signature"]).unwrap(),
        };
        // the other holders cannot open it
        for (m, other) in holders.iter().enumerate() {
            if m != n {
                assert!(moqui_vault::delivery::open(&delivery, other, &link).is_err());
            }
        }
        let opened = moqui_vault::delivery::open(&delivery, holder, &link).unwrap();
        let value = moqui_vault::managed::ShareValue::parse(std::str::from_utf8(&opened).unwrap()).unwrap();
        assert_eq!((value.of, value.threshold), (3, 2));
        let confirmation = moqui_vault::managed::confirm_share(holder.signing_key(), &value);
        call(
            &gate,
            Some(HOLDERS[n]),
            "confirm#InstanceRecoveryShare",
            &[
                ("deliveryId", &delivery.delivery_id),
                ("confirmation", &hex(&confirmation)),
            ],
        )
        .unwrap();
        shares.push(value.share.clone());
    }

    // 2. a restart unseals by transit, with the same root key, and finds every share confirmed
    stop_dialer(&first);
    let second = base.join("start-2");
    restart_at(&first, &second);
    let keeper = moqui_core::seal::keeper_for(&second, seal_conf()).unwrap();
    moqui_host::manage_link::dial::start_for_gate(&second, &conf);
    assert_eq!(keeper.start(Mode::Operator).unwrap(), Start::Unsealed);
    assert_eq!(keeper.status().root_id, root_id);
    wait_until("every share confirmed", Duration::from_secs(60), || {
        link_state(&second)["shareSet"]["shares"]
            .as_array()
            .is_some_and(|list| list.len() == 3 && list.iter().all(|share| share["confirmed"] == true))
    });

    // 3. the manager out of reach: the instance stays sealed, the manager's link-down gauge fires, shares unseal it
    stop_dialer(&second);
    let third = base.join("start-3");
    restart_at(&second, &third);
    let mut state = link_state(&third);
    state["url"] = serde_json::json!(format!("https://localhost:{}", crate::common::free_port()));
    std::fs::write(third.join("db/manage_link.json"), state.to_string()).unwrap();
    let keeper = moqui_core::seal::keeper_for(&third, seal_conf()).unwrap();
    moqui_host::manage_link::dial::start_for_gate(&third, &conf);
    match keeper.start(Mode::Operator).unwrap() {
        Start::Waiting(reason) => assert!(reason.starts_with("auto-unseal failed"), "{reason}"),
        Start::Unsealed => panic!("unsealed with its manager out of reach"),
    }
    assert!(!keeper.is_unsealed());
    let down = {
        // read as the manager's gauges are, in its context
        let factory = gate.server.factory();
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        moqui_instance::monitor::refresh(&factory.entity().unwrap());
        moqui_instance::monitor::series("moqui.instance.link_down_seconds", moqui_core::user::now_millis())
    };
    assert!(
        down.iter().any(|(labels, seconds)| {
            labels.get("moqui.instance.id").map(to_plain_string).as_deref() == Some("MS_APP") && *seconds > 0.0
        }),
        "{down:?}"
    );
    for share in &shares[..2] {
        keeper
            .unseal(moqui_core::seal::UnsealInput::Recovery(Zeroizing::new(
                share.to_string(),
            )))
            .unwrap();
    }
    assert!(keeper.is_unsealed());
    assert_eq!(keeper.status().root_id, root_id);

    // 4. the approval policy holds the transit unseal; two holders seal their shares to this unseal's key
    stop_dialer(&third);
    {
        let factory = gate.server.factory();
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let mut env = facade
            .find("moqui.server.instance.InstanceEnvironment")
            .unwrap()
            .eq("environmentId", Value::string("MS_ENV"))
            .one()
            .unwrap()
            .unwrap();
        env.set("unsealPolicyEnumId", Value::string("IupApproval"));
        env.update(&facade).unwrap();
    }
    let fourth = base.join("start-4");
    restart_at(&second, &fourth);
    let keeper = moqui_core::seal::keeper_for(&fourth, seal_conf()).unwrap();
    moqui_host::manage_link::dial::start_for_gate(&fourth, &conf);
    match keeper.start(Mode::Operator).unwrap() {
        Start::Waiting(reason) => assert!(reason.contains("approve"), "{reason}"),
        Start::Unsealed => panic!("unsealed with no approval"),
    }
    let unseal_key = || {
        call(
            &gate,
            Some(HOLDERS[0]),
            "get#InstanceUnsealKey",
            &[("appInstanceId", "MS_APP")],
        )
        .unwrap()
    };
    wait_until(
        "the sealed instance offers an unseal key",
        Duration::from_secs(60),
        || to_plain_string(&unseal_key()["waiting"]) == "true",
    );
    for (n, share) in shares.iter().enumerate().skip(1) {
        let offered = call(
            &gate,
            Some(HOLDERS[n]),
            "get#InstanceUnsealKey",
            &[("appInstanceId", "MS_APP")],
        )
        .unwrap();
        let text = |name: &str| to_plain_string(&offered[name]);
        let key = moqui_vault::managed::UnsealKey {
            instance_id: "MS_APP".to_string(),
            unseal_public: moqui_vault::text::unhex(&text("unsealPublic"))
                .unwrap()
                .try_into()
                .unwrap(),
            signature: moqui_vault::text::unhex(&text("signature")).unwrap(),
        };
        key.verify(
            &moqui_vault::text::unhex(&text("linkPublic")).unwrap(),
            &text("linkKeyFingerprint"),
        )
        .unwrap();
        let sealed = moqui_vault::managed::seal_share(&key, share, &mut OsRng).unwrap();
        call(
            &gate,
            Some(HOLDERS[n]),
            "send#InstanceSealedShare",
            &[
                ("appInstanceId", "MS_APP"),
                ("unsealPublic", &text("unsealPublic")),
                ("sealed", &hex(&sealed)),
            ],
        )
        .unwrap();
    }
    wait_until("the relayed shares unseal it", Duration::from_secs(60), || {
        keeper.is_unsealed()
    });
    assert_eq!(keeper.status().root_id, root_id);
    stop_dialer(&fourth);
}
