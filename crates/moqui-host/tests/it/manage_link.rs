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

//! The manager link (plans/Vault-Instances.md §4.4, M2). The manager is a real server in the test's own process, its
//! HTTPS listener taking `moqui-manage/1`; the instance is the link's dialer driven with an explicit `ManageConfig` and a
//! fake [`Managed`] whose seal the test opens and closes, so it needs no factory and opens no port. A key not pinned is
//! refused, and so is a manager whose key is not the one pinned; the pairing token works once, and pins the instance's
//! key and its approvers; while sealed the instance makes only the sealed calls, and the manager runs only status on
//! it; once unsealed the bootstrap bundle goes into its seal and the manager's commands run over the link; a revoke
//! cuts it.

#![cfg(all(unix, feature = "manage", feature = "instances", feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_host::manage_link::bundle::Values;
use moqui_host::manage_link::proto::{DeployComponentCommand, InstanceStatus};
use moqui_host::manage_link::{Dialer, LinkKey, LinkPhase, LinkStatus, ManageConfig, Managed};
use moqui_lib::object::{to_plain_string, Value};
use sha2::Digest as _;
use tokio::sync::watch;
use zeroize::Zeroizing;

use crate::common::Gate;

const SERVICES: &str = "org.moqui.dev.InstanceLinkServices";
const TOKEN: &str = "a-pairing-token-of-the-test-ABCDEFGH";

/// An instance whose seal the test opens and closes, keeping what the link put in it.
#[derive(Default)]
struct Fake {
    sealed: AtomicBool,
    secrets: Mutex<Vec<String>>,
    bundle: Mutex<Option<BTreeMap<String, String>>>,
}

impl Managed for Fake {
    fn status(&self) -> InstanceStatus {
        let sealed = self.sealed.load(Ordering::SeqCst);
        InstanceStatus {
            binary_version: "test".to_string(),
            seal_state: if sealed { "sealed" } else { "unsealed" }.to_string(),
            health: if sealed { "sealed" } else { "ok" }.to_string(),
            link_version: moqui_host::manage_link::LINK_VERSION,
            ..InstanceStatus::default()
        }
    }
    fn seal(&self) -> Result<(), String> {
        self.sealed.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn restart(&self) -> Result<(), String> {
        Ok(())
    }
    fn drain(&self, _drained: bool) -> Result<(), String> {
        Ok(())
    }
    fn put_secret(&self, name: &str, _value: &str, _force: bool) -> Result<(), String> {
        self.secrets.lock().unwrap().push(name.to_string());
        Ok(())
    }
    fn deploy_component(&self, _deploy: &DeployComponentCommand) -> Result<String, String> {
        Err("no deploy in this test".to_string())
    }
    fn export_seal(&self) -> Result<String, String> {
        Ok("wrapped values only".to_string())
    }
    fn store_bundle(&self, values: &Values) -> Result<(), String> {
        if self.sealed.load(Ordering::SeqCst) {
            return Err("sealed".to_string());
        }
        *self.bundle.lock().unwrap() = Some(
            values
                .iter()
                .map(|(name, value)| (name.clone(), value.to_string()))
                .collect(),
        );
        Ok(())
    }
}

fn data(token_hash: &str, approver_key: &str) -> String {
    format!(
        r#"<entity-facade-xml type="seed">
    <moqui.security.UserAccount userId="ML_APPROVER" username="ml.approver"/>
    <moqui.security.UserGroup userGroupId="ML_APPROVERS"/>
    <moqui.security.UserGroupPermission userGroupId="ML_APPROVERS" userPermissionId="INSTANCE_ADMIN" fromDate="2020-01-01 00:00:00"/>
    <moqui.security.UserAccount userId="ML_ADMIN" username="ml.admin"/>
    <moqui.security.UserGroup userGroupId="ML_ADMINS"/>
    <moqui.security.UserGroupMember userGroupId="ML_ADMINS" userId="ML_ADMIN" fromDate="2020-01-01 00:00:00"/>
    <moqui.security.UserGroupPermission userGroupId="ML_ADMINS" userPermissionId="INSTANCE_ADMIN" fromDate="2020-01-01 00:00:00"/>
    <moqui.security.UserAccount userId="ML_NOBODY" username="ml.nobody"/>
    <!-- the test lets every user call the services, so what refuses a caller is each service's own check -->
    <moqui.security.ArtifactGroup artifactGroupId="ML_LINK_SERVICES"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="ML_LINK_SERVICES" artifactTypeEnumId="AT_SERVICE" inheritAuthz="Y"
            nameIsPattern="Y" artifactName="org\.moqui\.dev\.InstanceLinkServices\..*"/>
    <moqui.security.ArtifactAuthz artifactAuthzId="ML_LINK_SERVICES_ALL" userGroupId="ALL_USERS"
            artifactGroupId="ML_LINK_SERVICES" authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL"/>
    <moqui.security.UserGroupMember userGroupId="ML_APPROVERS" userId="ML_APPROVER" fromDate="2020-01-01 00:00:00"/>
    <moqui.security.vault.VaultPersonalKey personalKeyId="ML_PK" userId="ML_APPROVER" encryptPublic="00"
            verifyPublic="{approver_key}" statusId="VpkActive"/>
    <moqui.server.instance.InstanceEnvironment environmentId="ML_ENV" approverUserGroupId="ML_APPROVERS"
            transportEnumId="ItrLink"/>
    <moqui.server.instance.AppInstance appInstanceId="ML_APP" instanceKindId="mr-moqui" environmentId="ML_ENV"
            pairingTokenHash="{token_hash}" pairingTokenExpires="2099-01-01 00:00:00"/>
</entity-facade-xml>"#
    )
}

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

/// A service of InstanceLinkServices on the manager, as an administrator.
fn call(gate: &Gate, verb_noun: &str, parameters: &[(&str, &str)]) -> Result<BTreeMap<String, Value>, String> {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let _authz = context.disable_authz();
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

/// A service of InstanceLinkServices on the manager as `user`, under that user's own authorization.
fn call_as(
    gate: &Gate,
    user: &str,
    verb_noun: &str,
    parameters: &[(&str, &str)],
) -> Result<BTreeMap<String, Value>, String> {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user(user).unwrap();
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

fn text(out: &BTreeMap<String, Value>, name: &str) -> String {
    out.get(name).map(to_plain_string).unwrap_or_default()
}

fn wait_status(dialer: &Dialer, what: &str, check: impl Fn(&LinkStatus) -> bool) -> LinkStatus {
    let started = Instant::now();
    loop {
        let status = dialer.status();
        if check(&status) {
            return status;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "{what}: still {status:?}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn wait_until(what: &str, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Short timings, so a link comes back and a status goes up in well under a second.
fn quick(config: &mut ManageConfig) {
    config.backoff = moqui_host::manage_link::Backoff {
        first: Duration::from_millis(100),
        max: Duration::from_millis(400),
    };
    config.refused_backoff = config.backoff;
    config.keepalive = Duration::from_secs(2);
    config.status_every = Duration::from_millis(150);
    config.connect_timeout = Duration::from_secs(3);
}

#[test]
fn an_instance_pairs_once_links_sealed_then_unsealed_and_a_revoke_cuts_it() {
    if !crate::common::own_process() {
        return;
    }
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let approver = moqui_vault::sign::SigningKey::from_seed(&moqui_vault::Key32::from_bytes([5; 32]));
    let token_hash = moqui_vault::text::hex(&sha2::Sha256::digest(TOKEN.as_bytes()));
    let port = crate::common::free_port();
    let port_text = port.to_string();
    let gate = crate::common::start_with_files(
        "manage-link",
        Some(&data(
            &token_hash,
            &moqui_vault::text::hex(approver.verifying_key().as_bytes()),
        )),
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
    let link = call(&gate, "get#InstanceLink", &[("appInstanceId", "ML_APP")]).unwrap();
    let pin = text(&link, "managerFingerprint");
    assert_eq!(pin.len(), 64, "{link:?}");
    assert_eq!(text(&link, "linked"), "false");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (_shutdown, shutdown) = watch::channel(false);
    let dir = gate.runtime.join("instance-side");
    std::fs::create_dir_all(&dir).unwrap();

    // a key that was never pinned is refused: the dialer thinks it paired, the manager knows no such key
    let stranger = Arc::new(LinkKey::generate("stranger").unwrap());
    let claimed = dir.join("stranger.json");
    std::fs::write(&claimed, r#"{"appInstanceId": "ML_APP", "url": "x", "pin": "y"}"#).unwrap();
    let mut config = ManageConfig::new(&url, &pin, stranger);
    config.state_file = Some(claimed);
    quick(&mut config);
    let fake = Arc::new(Fake::default());
    let refused = Dialer::start(
        config,
        Arc::clone(&fake) as Arc<dyn Managed>,
        rt.handle(),
        shutdown.clone(),
    );
    let status = wait_status(&refused, "an unpinned key refused", |status| {
        status.phase == LinkPhase::Refused
    });
    assert!(status.message.contains("not pinned"), "{status:?}");
    refused.stop();

    // a manager whose key is not the one pinned is refused, before any token is spent
    let key_a = Arc::new(LinkKey::generate("instance-a").unwrap());
    let mut wrong = ManageConfig::new(&url, &"00".repeat(32), Arc::clone(&key_a));
    wrong.secret = Some(Arc::new(Zeroizing::new(TOKEN.to_string())));
    quick(&mut wrong);
    let wrong = Dialer::start(
        wrong,
        Arc::clone(&fake) as Arc<dyn Managed>,
        rt.handle(),
        shutdown.clone(),
    );
    let status = wait_status(&wrong, "a wrong manager key refused", |status| {
        status.phase == LinkPhase::Refused
    });
    assert!(status.message.contains("pinned"), "{status:?}");
    wrong.stop();

    // the token pairs, sealed: the instance pins its approvers, links, and the manager runs only status on it
    moqui_instance::link::hold_bundle(
        "ML_APP",
        [(
            "entity-facade/datasource[transactional]@password".to_string(),
            Zeroizing::new("db-pass-of-the-test".to_string()),
        )]
        .into_iter()
        .collect(),
        moqui_core::user::now_millis() + 60_000,
    );
    fake.sealed.store(true, Ordering::SeqCst);
    let mut config = ManageConfig::new(&url, &pin, Arc::clone(&key_a));
    config.secret = Some(Arc::new(Zeroizing::new(TOKEN.to_string())));
    config.state_file = Some(dir.join("instance-a.json"));
    quick(&mut config);
    let dialer = Dialer::start(
        config,
        Arc::clone(&fake) as Arc<dyn Managed>,
        rt.handle(),
        shutdown.clone(),
    );
    wait_status(&dialer, "linked while sealed", |status| {
        status.phase == LinkPhase::Linked
    });
    let state = dialer.state();
    assert_eq!(state.app_instance_id, "ML_APP");
    assert_eq!(state.approvers_version, 1);
    assert_eq!(state.approver_set().approvers.len(), 1);
    assert_eq!(
        state.approver_set().approvers[0].verify_public,
        *approver.verifying_key().as_bytes()
    );
    wait_until("the manager sees it sealed", || {
        text(
            &call(&gate, "get#InstanceLink", &[("appInstanceId", "ML_APP")]).unwrap(),
            "sealState",
        ) == "sealed"
    });
    let link = call(&gate, "get#InstanceLink", &[("appInstanceId", "ML_APP")]).unwrap();
    assert_eq!(text(&link, "linked"), "true");
    assert_eq!(
        text(&link, "serving"),
        "false",
        "a sealed instance opens no Serve stream: {link:?}"
    );
    assert_eq!(text(&link, "linkKeyFingerprint"), key_a.fingerprint());
    let status = call(
        &gate,
        "call#InstanceCommand",
        &[("appInstanceId", "ML_APP"), ("command", "status")],
    )
    .unwrap();
    assert_eq!(text(&status, "sealState"), "sealed");
    let sealed = call(
        &gate,
        "call#InstanceCommand",
        &[("appInstanceId", "ML_APP"), ("command", "export-seal")],
    )
    .unwrap_err();
    assert!(sealed.contains("sealed"), "{sealed}");
    assert!(call(
        &gate,
        "call#InstanceCommand",
        &[("appInstanceId", "ML_APP"), ("command", "run-service")]
    )
    .unwrap_err()
    .contains("not a command of the manager link"));
    // the bundle was taken while sealed and waits in memory; nothing is in the seal yet
    assert!(fake.bundle.lock().unwrap().is_none());
    assert!(!moqui_instance::link::bundle_ready("ML_APP"));

    // the token works once: another key with it is refused
    let mut again = ManageConfig::new(&url, &pin, Arc::new(LinkKey::generate("instance-b").unwrap()));
    again.secret = Some(Arc::new(Zeroizing::new(TOKEN.to_string())));
    quick(&mut again);
    let again = Dialer::start(
        again,
        Arc::clone(&fake) as Arc<dyn Managed>,
        rt.handle(),
        shutdown.clone(),
    );
    let status = wait_status(&again, "the token spent", |status| status.phase == LinkPhase::Refused);
    assert!(status.message.contains("unknown, used or expired"), "{status:?}");
    again.stop();

    // unsealed: the bundle goes into the seal, Serve opens, and the manager's commands run over the link
    fake.sealed.store(false, Ordering::SeqCst);
    wait_status(&dialer, "serving once unsealed", |status| status.serving);
    let kept = fake.bundle.lock().unwrap().clone().unwrap();
    assert_eq!(
        kept["entity-facade/datasource[transactional]@password"],
        "db-pass-of-the-test"
    );
    assert!(dialer.state().bootstrap_done);
    let put = call(
        &gate,
        "call#InstanceCommand",
        &[
            ("appInstanceId", "ML_APP"),
            ("command", "put-secret"),
            ("name", "webapp-list/webapp[webroot]/session-config@cookie-key"),
            ("value", "a-cookie-key"),
        ],
    )
    .unwrap();
    assert_eq!(text(&put, "transport"), "link");
    assert_eq!(text(&put, "sealState"), "unsealed");
    assert_eq!(
        fake.secrets.lock().unwrap().as_slice(),
        ["webapp-list/webapp[webroot]/session-config@cookie-key"]
    );
    let exported = call(
        &gate,
        "call#InstanceCommand",
        &[("appInstanceId", "ML_APP"), ("command", "export-seal")],
    )
    .unwrap();
    assert_eq!(text(&exported, "text"), "wrapped values only");
    let surface = call(
        &gate,
        "call#InstanceCommand",
        &[("appInstanceId", "ML_APP"), ("command", "surface-report")],
    )
    .unwrap_err();
    assert!(surface.contains("not built"), "{surface}");

    // seal over the link: the Serve stream closes and only status goes on
    call(
        &gate,
        "call#InstanceCommand",
        &[("appInstanceId", "ML_APP"), ("command", "seal")],
    )
    .unwrap();
    wait_status(&dialer, "Serve closed once sealed", |status| !status.serving);
    wait_until("the manager sees it sealed again", || {
        text(
            &call(&gate, "get#InstanceLink", &[("appInstanceId", "ML_APP")]).unwrap(),
            "sealState",
        ) == "sealed"
    });

    // each manager service checks its caller under the caller's own authorization: INSTANCE_ADMIN outside the
    // environment's approver group and a user with neither are refused; a member of the group with it is not
    for user in ["ml.admin", "ml.nobody"] {
        for (service, parameters) in [
            ("get#InstanceLink", vec![("appInstanceId", "ML_APP")]),
            (
                "call#InstanceCommand",
                vec![("appInstanceId", "ML_APP"), ("command", "status")],
            ),
            ("create#InstancePairingCode", vec![("appInstanceId", "ML_APP")]),
            (
                "update#InstanceApprovers",
                vec![
                    ("appInstanceId", "ML_APP"),
                    ("version", "2"),
                    ("signerPublic", "00"),
                    ("signature", "00"),
                ],
            ),
            ("revoke#InstanceLink", vec![("appInstanceId", "ML_APP")]),
        ] {
            let refused = call_as(&gate, user, service, &parameters).unwrap_err();
            assert!(
                refused.contains("INSTANCE_ADMIN") || refused.contains("notEnvironmentMember"),
                "{user} was not refused by the service's own check of {service}: {refused}"
            );
        }
    }
    let allowed = call_as(&gate, "ml.approver", "get#InstanceLink", &[("appInstanceId", "ML_APP")]);
    assert!(
        allowed.is_ok(),
        "a member of the approver group with INSTANCE_ADMIN was refused: {allowed:?}"
    );
    assert_eq!(
        dialer.status().phase,
        LinkPhase::Linked,
        "a refused revoke cut the link"
    );

    // a revoke cuts the link, and the key is refused after
    call(&gate, "revoke#InstanceLink", &[("appInstanceId", "ML_APP")]).unwrap();
    let status = wait_status(&dialer, "refused after the revoke", |status| {
        status.phase == LinkPhase::Refused
    });
    assert!(status.message.contains("not pinned"), "{status:?}");
    dialer.stop();
}
