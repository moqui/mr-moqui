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

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use moqui_vault::approvers::{Approver, ApproverSet};
use moqui_vault::keys::Key32;
use moqui_vault::sign::SigningKey;
use zeroize::Zeroizing;

use super::bundle::{self, Values};
use super::commands::{run, LinkContext, Managed};
use super::key::{KeyStore, LinkKey};
use super::proto::{command::Body, Command, DeployComponentCommand, DrainCommand, InstanceStatus, PutSecretCommand};
use super::state::{set_to_proto, LinkState};
use super::{Call, CommandKind};

/// An instance whose seal a test opens and closes, counting what the link ran on it.
#[derive(Default)]
struct Fake {
    sealed: AtomicBool,
    ran: AtomicUsize,
    secrets: Mutex<Vec<String>>,
}

impl Managed for Fake {
    fn status(&self) -> InstanceStatus {
        InstanceStatus {
            seal_state: if self.sealed.load(Ordering::SeqCst) {
                "sealed"
            } else {
                "unsealed"
            }
            .to_string(),
            link_version: super::LINK_VERSION,
            ..InstanceStatus::default()
        }
    }
    fn seal(&self) -> Result<(), String> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        self.sealed.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn restart(&self) -> Result<(), String> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn drain(&self, _drained: bool) -> Result<(), String> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn put_secret(&self, name: &str, _value: &str, _force: bool) -> Result<(), String> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        self.secrets.lock().unwrap().push(name.to_string());
        Ok(())
    }
    fn deploy_component(&self, _deploy: &DeployComponentCommand) -> Result<String, String> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        Ok("deployed".to_string())
    }
    fn export_seal(&self) -> Result<String, String> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        Ok("bundle".to_string())
    }
    fn store_bundle(&self, _values: &Values) -> Result<(), String> {
        Ok(())
    }
}

fn command(body: Body) -> Command {
    Command {
        id: 7,
        body: Some(body),
    }
}

fn signing(seed: u8) -> SigningKey {
    SigningKey::from_seed(&Key32::from_bytes([seed; 32]))
}

fn approver(user: &str, key: &SigningKey) -> Approver {
    Approver {
        user_id: user.to_string(),
        verify_public: *key.verifying_key().as_bytes(),
    }
}

fn context(approvers: &ApproverSet) -> std::sync::Arc<LinkContext> {
    let mut state = LinkState {
        app_instance_id: "prod-1".to_string(),
        ..LinkState::default()
    };
    state.pin_approvers(approvers);
    LinkContext::new(state, None, "fp-prod-1")
}

#[test]
fn a_sealed_instance_answers_only_the_sealed_calls() {
    let sealed_ok: Vec<Call> = [
        Call::Pair,
        Call::Hello,
        Call::Status,
        Call::Bootstrap,
        Call::Unseal,
        Call::Event,
        Call::Serve,
    ]
    .into_iter()
    .filter(|call| call.sealed_ok())
    .collect();
    assert_eq!(sealed_ok, [Call::Hello, Call::Status, Call::Bootstrap, Call::Unseal]);

    // and on the instance, only status runs while sealed; the others wait for the unseal
    let fake = Fake::default();
    fake.sealed.store(true, Ordering::SeqCst);
    let link = context(&ApproverSet {
        version: 1,
        approvers: Vec::new(),
    });
    let status = run(&fake, &link, command(Body::Status(Default::default())));
    assert!(status.ok, "{status:?}");
    assert_eq!(status.status.unwrap().seal_state, "sealed");
    for body in [
        Body::Seal(Default::default()),
        Body::Restart(Default::default()),
        Body::Drain(DrainCommand { drain: true }),
        Body::PutSecret(PutSecretCommand {
            name: "a".to_string(),
            value: "b".to_string(),
            force: false,
        }),
        Body::ExportSeal(Default::default()),
    ] {
        let reply = run(&fake, &link, command(body));
        assert!(!reply.ok && reply.error.starts_with("sealed:"), "{reply:?}");
    }
    assert_eq!(fake.ran.load(Ordering::SeqCst), 0);

    // unsealed, they run
    fake.sealed.store(false, Ordering::SeqCst);
    let reply = run(
        &fake,
        &link,
        command(Body::PutSecret(PutSecretCommand {
            name: "webapp-list/webapp[webroot]/session-config@cookie-key".to_string(),
            value: "v".to_string(),
            force: false,
        })),
    );
    assert!(reply.ok, "{reply:?}");
    assert_eq!(reply.id, 7);
    assert_eq!(fake.ran.load(Ordering::SeqCst), 1);
}

#[test]
fn a_command_not_on_the_list_is_refused_on_the_instance() {
    let fake = Fake::default();
    let link = context(&ApproverSet {
        version: 1,
        approvers: Vec::new(),
    });
    // an empty command, as one of a later version this binary does not know decodes
    let unknown = Command { id: 3, body: None };
    assert_eq!(CommandKind::of(&unknown), None);
    let reply = run(&fake, &link, unknown);
    assert!(!reply.ok, "{reply:?}");
    assert!(reply.error.contains("not a command of the manager link"), "{reply:?}");
    // decoded from bytes with a field number no command has, it is the same
    let bytes = [0x08, 0x05, 0xa2, 0x06, 0x00]; // id 5, field 100 empty
    let decoded = <Command as prost::Message>::decode(bytes.as_slice()).unwrap();
    assert!(!run(&fake, &link, decoded).ok);
    assert_eq!(fake.ran.load(Ordering::SeqCst), 0);
    // no command name outside the list parses either, so a service cannot name an arbitrary one
    assert_eq!(CommandKind::parse("run-service"), None);
    // the surface report is a slot until S12
    let reply = run(&fake, &link, command(Body::SurfaceReport(Default::default())));
    assert!(!reply.ok && reply.error.contains("not built"), "{reply:?}");
}

#[test]
fn a_changed_approver_set_is_taken_only_signed_by_a_pinned_approver() {
    let (alice, bob, mallory) = (signing(1), signing(2), signing(3));
    let pinned = ApproverSet {
        version: 1,
        approvers: vec![approver("alice", &alice)],
    };
    let link = context(&pinned);
    let fake = Fake::default();
    let next = ApproverSet {
        version: 2,
        approvers: vec![approver("alice", &alice), approver("bob", &bob)],
    };

    // signed by the manager's own key: refused, and the pinned set stays
    let mut forged = set_to_proto(&next);
    forged.signer_public = mallory.verifying_key().as_bytes().to_vec();
    forged.signature = next.sign("prod-1", "fp-prod-1", &mallory).to_vec();
    let reply = run(&fake, &link, command(Body::Approvers(forged)));
    assert!(!reply.ok && reply.error.contains("not an approver"), "{reply:?}");
    // unsigned: refused
    let mut unsigned = set_to_proto(&next);
    unsigned.signer_public = alice.verifying_key().as_bytes().to_vec();
    assert!(!run(&fake, &link, command(Body::Approvers(unsigned))).ok);
    assert_eq!(link.snapshot().approvers_version, 1);

    // signed by alice, whom it pinned: taken
    let mut signed = set_to_proto(&next);
    signed.signer_public = alice.verifying_key().as_bytes().to_vec();
    signed.signature = next.sign("prod-1", "fp-prod-1", &alice).to_vec();
    let reply = run(&fake, &link, command(Body::Approvers(signed)));
    assert!(reply.ok, "{reply:?}");
    let state = link.snapshot();
    assert_eq!(state.approvers_version, 2);
    assert_eq!(state.approver_set().approvers.len(), 2);
}

#[test]
fn a_bootstrap_bundle_opens_only_for_its_instance_and_key() {
    let key = LinkKey::generate("i").unwrap();
    let other = LinkKey::generate("i").unwrap();
    let values: Values = [(
        "entity-facade/datasource[transactional]@password".to_string(),
        Zeroizing::new("s3cret".to_string()),
    )]
    .into_iter()
    .collect();
    let sealed = bundle::seal(&key.bundle_public().unwrap(), "prod-1", &values).unwrap();
    assert!(!sealed.windows(6).any(|window| window == b"s3cret"));
    let opened = bundle::open(key.bundle_secret(), "prod-1", &sealed).unwrap();
    assert_eq!(
        opened["entity-facade/datasource[transactional]@password"].as_str(),
        "s3cret"
    );
    assert!(bundle::open(key.bundle_secret(), "prod-2", &sealed).is_err());
    assert!(bundle::open(other.bundle_secret(), "prod-1", &sealed).is_err());
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-manage-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_link_key_is_kept_owner_only_and_gives_the_same_keys_again() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = scratch("key");
    let file = dir.join("db").join(super::KEY_FILE);
    let made = LinkKey::load_or_create(&file, &KeyStore::File, "i").unwrap();
    let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let again = LinkKey::load_or_create(&file, &KeyStore::File, "i").unwrap();
    assert_eq!(made.fingerprint(), again.fingerprint());
    assert_eq!(made.bundle_public().unwrap(), again.bundle_public().unwrap());
    assert_eq!(made.fingerprint().len(), 64);
    let debug = format!("{made:?}");
    assert!(!debug.contains("seed"), "{debug}");

    // with systemd-creds the file holds what it made, and only it opens it (a stub that records its calls)
    let stub = dir.join("systemd-creds");
    let log = dir.join("calls");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\necho \"$1\" >> '{}'\nLC_ALL=C exec tr 'A-Za-z' 'N-ZA-Mn-za-m'\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = KeyStore::SystemdCreds {
        command: stub.display().to_string(),
    };
    let tpm_file = dir.join("db").join("tpm_key");
    let made = LinkKey::load_or_create(&tpm_file, &store, "i").unwrap();
    let again = LinkKey::load_or_create(&tpm_file, &store, "i").unwrap();
    assert_eq!(made.fingerprint(), again.fingerprint());
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "encrypt\ndecrypt\n");
    // the bytes the stub wrote are not taken as a plain file's seed
    let plain = LinkKey::load_or_create(&tpm_file, &KeyStore::File, "i").unwrap();
    assert_ne!(plain.fingerprint(), made.fingerprint());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_pairing_state_reads_back_as_kept() {
    let dir = scratch("state");
    let file = dir.join(super::STATE_FILE);
    let mut state = LinkState {
        url: "https://manager:8443".to_string(),
        pin: "ab".repeat(32),
        app_instance_id: "prod-1".to_string(),
        environment_id: "Prod".to_string(),
        manager_id: "mgr".to_string(),
        bootstrap_done: true,
        ..LinkState::default()
    };
    state.pin_approvers(&ApproverSet {
        version: 3,
        approvers: vec![approver("alice", &signing(1))],
    });
    state.save(&file).unwrap();
    assert_eq!(LinkState::load(&file), Some(state.clone()));
    assert_eq!(state.approver_set().approvers.len(), 1);
    std::fs::write(&file, "not json").unwrap();
    assert_eq!(LinkState::load(&file), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn manager_urls_take_a_host_and_an_optional_port_over_https_only() {
    assert_eq!(
        super::parse_url("https://m.example").unwrap(),
        ("m.example".to_string(), 443)
    );
    assert_eq!(super::parse_url("https://m:8443/").unwrap(), ("m".to_string(), 8443));
    assert!(super::parse_url("http://m").is_err());
    assert!(super::check_versions(1, 2, "x").is_ok());
    assert!(super::check_versions(1, 3, "x").is_err());
    assert!(super::check_versions(1, 0, "x").is_err());
}

#[test]
fn an_unknown_key_store_is_refused_not_taken_as_the_plain_file() {
    let setting = |value: &'static str| {
        move |name: &str| {
            if name == "manage@key-store" {
                value.to_string()
            } else {
                String::new()
            }
        }
    };
    assert_eq!(KeyStore::from_settings(&setting("")), Ok(KeyStore::File));
    assert_eq!(KeyStore::from_settings(&setting("file")), Ok(KeyStore::File));
    assert!(matches!(
        KeyStore::from_settings(&setting("systemd-creds")),
        Ok(KeyStore::SystemdCreds { .. })
    ));
    assert!(KeyStore::from_settings(&setting("tpm")).is_err());
}

#[test]
fn a_bundle_keeps_only_bootstrap_secrets_and_never_another_crypt_pass() {
    use moqui_core::seal::{secrets, Init, Keeper, MethodKind, SealConf};
    let dir = scratch("bundle");
    let keeper = Keeper::open(&dir.join("db").join("seal.json"), SealConf::with_purpose("dev")).unwrap();
    let sealed: Values = [(
        secrets::DATASOURCE_PASSWORD.to_string(),
        Zeroizing::new("p".to_string()),
    )]
    .into_iter()
    .collect();
    assert!(super::commands::store_bundle_in(&keeper, None, &sealed)
        .unwrap_err()
        .starts_with("sealed"));
    keeper
        .init(Init {
            method: MethodKind::Dev,
            passphrase: None,
        })
        .unwrap();
    let kept_pass = keeper.bootstrap_secret(secrets::CRYPT_PASS).unwrap().unwrap();
    let values: Values = [
        (
            secrets::DATASOURCE_PASSWORD.to_string(),
            Zeroizing::new("db-pass".to_string()),
        ),
        (
            secrets::CRYPT_PASS.to_string(),
            Zeroizing::new("another-pass".to_string()),
        ),
        (
            "webapp-list/webapp[webroot]/session-config@cookie-key".to_string(),
            Zeroizing::new("not a bootstrap secret".to_string()),
        ),
    ]
    .into_iter()
    .collect();
    let refused = super::commands::store_bundle_in(&keeper, None, &values).unwrap_err();
    assert!(
        refused.contains(secrets::CRYPT_PASS) && refused.contains("cookie-key"),
        "{refused}"
    );
    // the datasource password is kept; the crypt pass is the one there was
    assert_eq!(
        keeper
            .bootstrap_secret(secrets::DATASOURCE_PASSWORD)
            .unwrap()
            .unwrap()
            .as_slice(),
        b"db-pass"
    );
    assert_eq!(
        keeper.bootstrap_secret(secrets::CRYPT_PASS).unwrap().unwrap(),
        kept_pass
    );
    let _ = std::fs::remove_dir_all(&dir);
}
