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

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use super::*;

/// A directory of its own for one test's seal, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "moqui-seal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn seal_path(&self) -> PathBuf {
        file::path_in(&self.0)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn keeper(scratch: &Scratch, purpose: &str) -> Keeper {
    Keeper::open(&scratch.seal_path(), SealConf::with_purpose(purpose)).unwrap()
}

fn conf_with(purpose: &str, method: &str) -> SealConf {
    SealConf {
        method: method.to_string(),
        ..SealConf::with_purpose(purpose)
    }
}

#[test]
fn a_first_start_on_a_dev_instance_makes_the_dev_seal_and_the_next_start_opens_it() {
    let scratch = Scratch::new();
    let first = keeper(&scratch, "dev");
    assert_eq!(first.start(Mode::Operator).unwrap(), Start::Unsealed);
    let status = first.status();
    assert_eq!(
        (status.state, status.method.as_str(), status.generation),
        (State::Unsealed, "dev", 1)
    );
    assert!(!status.ephemeral);
    let written = file::load(&scratch.seal_path()).unwrap().expect("seal.json written");
    assert_eq!(written.root_id, status.root_id);
    // a restart: the same seal opens, with the same root key
    let second = keeper(&scratch, "test");
    assert_eq!(second.start(Mode::Unattended).unwrap(), Start::Unsealed);
    assert_eq!(second.status().root_id, status.root_id);
}

#[test]
fn a_production_first_start_waits_for_init_and_an_unattended_boot_makes_no_seal() {
    let scratch = Scratch::new();
    let server = keeper(&scratch, "production");
    assert!(
        matches!(server.start(Mode::Operator).unwrap(), Start::Waiting(reason) if reason.contains("operator init"))
    );
    assert!(!server.is_unsealed());
    let tool = keeper(&scratch, "production");
    assert_eq!(tool.start(Mode::Unattended).unwrap(), Start::Unsealed);
    assert!(tool.status().ephemeral);
    assert!(
        !scratch.seal_path().exists(),
        "an unattended production boot writes no seal"
    );
}

#[test]
fn the_dev_seal_is_refused_on_a_production_instance() {
    let scratch = Scratch::new();
    let refused = keeper(&scratch, "production")
        .init(Init {
            method: MethodKind::Dev,
            passphrase: None,
        })
        .unwrap_err();
    assert!(
        refused.to_string().contains("refused on a production instance"),
        "{refused}"
    );
    // a dev seal made on a dev instance does not open once the purpose is production
    keeper(&scratch, "dev").start(Mode::Operator).unwrap();
    let production = keeper(&scratch, "production");
    let err = production.start(Mode::Operator).unwrap_err();
    assert!(err.to_string().contains("refused on a production instance"), "{err}");
    assert!(production.unseal(UnsealInput::Automatic).is_err());
    assert!(!production.is_unsealed());
}

#[test]
fn the_method_recorded_at_init_wins_over_the_conf() {
    let scratch = Scratch::new();
    keeper(&scratch, "dev").start(Mode::Operator).unwrap();
    // a conf that names another method neither moves the seal nor stops it opening
    let other = Keeper::open(&scratch.seal_path(), conf_with("dev", "passphrase")).unwrap();
    assert_eq!(other.start(Mode::Operator).unwrap(), Start::Unsealed);
    assert_eq!(other.status().method, "dev");
    assert_eq!(file::load(&scratch.seal_path()).unwrap().unwrap().method_kind(), "dev");
    // and init does not make a second seal over the first
    assert!(other
        .init(Init {
            method: MethodKind::Dev,
            passphrase: None,
        })
        .unwrap_err()
        .to_string()
        .contains("has a seal already"));
}

#[test]
fn seal_drops_the_key_tells_the_watchers_and_waits_for_unseal() {
    let scratch = Scratch::new();
    let keeper = Arc::new(keeper(&scratch, "dev"));
    keeper.start(Mode::Operator).unwrap();
    let told = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&told);
    let watch = keeper.on_seal(Arc::new(move || flag.store(true, Ordering::SeqCst)));
    keeper.seal();
    assert!(told.load(Ordering::SeqCst));
    assert!(!keeper.is_unsealed());
    assert_eq!(keeper.status().state, State::Sealed);
    // an automatic method does not open again at the next start once an operator sealed it
    assert!(matches!(keeper.start(Mode::Operator).unwrap(), Start::Waiting(_)));
    assert_eq!(keeper.unseal(UnsealInput::Automatic).unwrap().state, State::Unsealed);
    drop(watch);
    told.store(false, Ordering::SeqCst);
    keeper.seal();
    assert!(!told.load(Ordering::SeqCst), "a dropped watch is not told");
}

#[test]
fn a_wait_ends_on_unseal_or_when_abandoned() {
    let scratch = Scratch::new();
    let keeper = Arc::new(keeper(&scratch, "dev"));
    keeper.start(Mode::Operator).unwrap();
    keeper.seal();
    let waiter = Arc::clone(&keeper);
    let waiting = std::thread::spawn(move || waiter.wait_unsealed());
    keeper.unseal(UnsealInput::Automatic).unwrap();
    waiting.join().unwrap().unwrap();
    keeper.seal();
    let waiter = Arc::clone(&keeper);
    let waiting = std::thread::spawn(move || waiter.wait_unsealed());
    std::thread::sleep(Duration::from_millis(20));
    keeper.abandon();
    assert!(waiting.join().unwrap().is_err());
}

#[test]
fn two_processes_making_a_seal_at_once_end_with_one() {
    let scratch = Scratch::new();
    let first = keeper(&scratch, "dev");
    let second = keeper(&scratch, "dev");
    first.start(Mode::Operator).unwrap();
    // the second keeper loaded no file when it was made, and opens the first one's instead of replacing it
    assert_eq!(second.start(Mode::Operator).unwrap(), Start::Unsealed);
    assert_eq!(first.status().root_id, second.status().root_id);
}

#[test]
fn the_seal_file_round_trips_and_refuses_what_it_does_not_read() {
    let scratch = Scratch::new();
    keeper(&scratch, "dev").start(Mode::Operator).unwrap();
    let text = std::fs::read_to_string(scratch.seal_path()).unwrap();
    let parsed = SealFile::parse(&text).unwrap();
    assert_eq!(parsed.format, file::FORMAT);
    assert_eq!(parsed.version, file::VERSION);
    assert!(
        !format!("{parsed:?}").contains(parsed.method_text("key").unwrap()),
        "Debug hides the dev key"
    );
    let newer = text.replace("\"version\": 1", "\"version\": 99");
    assert!(SealFile::parse(&newer).unwrap_err().to_string().contains("newer"));
    assert!(SealFile::parse("{\"format\": \"other\"}").is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(scratch.seal_path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "seal.json is readable by its owner only");
    }
}

#[test]
fn a_write_stopped_before_its_rename_leaves_the_old_file() {
    let scratch = Scratch::new();
    keeper(&scratch, "dev").start(Mode::Operator).unwrap();
    let before = std::fs::read_to_string(scratch.seal_path()).unwrap();
    let mut changed = SealFile::parse(&before).unwrap();
    changed.generation = 7;
    // a crash: the staged file is left on disk and never renamed
    std::mem::forget(file::stage(&scratch.seal_path(), &changed).unwrap());
    assert_eq!(std::fs::read_to_string(scratch.seal_path()).unwrap(), before);
    // the next keeper removes the leftover and opens the old seal
    let next = keeper(&scratch, "dev");
    assert_eq!(next.start(Mode::Operator).unwrap(), Start::Unsealed);
    assert_eq!(next.status().generation, 1);
    let leftovers = std::fs::read_dir(scratch.seal_path().parent().unwrap())
        .unwrap()
        .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().contains(".tmp-"))
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn kms_and_transit_are_not_built_yet_and_an_unknown_method_is_refused() {
    for kind in [MethodKind::Kms, MethodKind::Transit] {
        let err = methods::automatic(kind).err().unwrap();
        assert!(err.to_string().contains("not built yet"), "{err}");
    }
    assert!(MethodKind::parse("rot13").is_err());
    let conf =
        moqui_lib::node::MNode::parse_text("conf", r#"<moqui-conf><seal method="rot13"/></moqui-conf>"#).unwrap();
    assert!(SealConf::from_conf(&conf).is_err());
    let conf =
        moqui_lib::node::MNode::parse_text("conf", r#"<moqui-conf><seal shares="2" threshold="3"/></moqui-conf>"#)
            .unwrap();
    assert!(SealConf::from_conf(&conf).is_err());
}

#[test]
fn the_root_key_hides_itself() {
    let root = RootKey::random().unwrap();
    assert_eq!(format!("{root:?}"), "RootKey(..)");
    assert_eq!(root.id().len(), 32);
    assert_ne!(root.id(), RootKey::random().unwrap().id());
}

/// Passphrase seals at the vault crate's floor in these tests, which a production instance refuses.
fn cheap_passphrases() {
    methods::passphrase::set_cost_for_tests(moqui_vault::kdf::Argon2Params::FLOOR);
}

fn texts(shares: &[Zeroizing<String>]) -> Vec<Zeroizing<String>> {
    shares.iter().map(|share| Zeroizing::new(share.to_string())).collect()
}

fn init(keeper: &Keeper, method: MethodKind, passphrase: Option<&str>) -> Issued {
    keeper
        .init(Init {
            method,
            passphrase: passphrase.map(|text| Zeroizing::new(text.to_string())),
        })
        .unwrap()
}

/// A share of the right form that is not one of the seal's: a share of another split.
fn foreign_share() -> Zeroizing<String> {
    let other = Scratch::new();
    let keeper = keeper(&other, "test");
    init(&keeper, MethodKind::Shamir, None).method_shares.swap_remove(0)
}

#[test]
fn a_passphrase_seal_opens_with_its_passphrase_and_a_wrong_one_doubles_the_wait() {
    cheap_passphrases();
    let scratch = Scratch::new();
    let made = keeper(&scratch, "test");
    let issued = init(&made, MethodKind::Passphrase, Some("correct horse battery"));
    assert_eq!(issued.recovery_shares.len(), 5);
    assert!(issued.method_shares.is_empty());
    let root_id = made.status().root_id;
    let keeper = keeper(&scratch, "test");
    keeper.set_base_delay(Duration::from_millis(40));
    assert!(matches!(keeper.start(Mode::Operator).unwrap(), Start::Waiting(_)));
    let wrong = |keeper: &Keeper| keeper.unseal(UnsealInput::Passphrase(Zeroizing::new("wrong horse".into())));
    assert!(wrong(&keeper).unwrap_err().to_string().contains("wrong passphrase"));
    let first = keeper.status();
    assert_eq!(first.failures, 1);
    assert!(first.retry_after_ms > 0 && first.retry_after_ms <= 40);
    // the next attempt before the wait is over is not taken, and not counted
    assert!(wrong(&keeper).unwrap_err().to_string().contains("wait"));
    assert_eq!(keeper.status().failures, 1);
    std::thread::sleep(Duration::from_millis(50));
    assert!(wrong(&keeper).is_err());
    let second = keeper.status();
    assert_eq!(second.failures, 2);
    assert!(
        second.retry_after_ms > 40,
        "the wait doubles: {}",
        second.retry_after_ms
    );
    std::thread::sleep(Duration::from_millis(90));
    let opened = keeper
        .unseal(UnsealInput::Passphrase(Zeroizing::new("correct horse battery".into())))
        .unwrap();
    assert_eq!((opened.state, opened.failures), (State::Unsealed, 0));
    assert_eq!(opened.root_id, root_id);
}

#[test]
fn the_wait_after_wrong_entries_stops_at_the_most_set() {
    cheap_passphrases();
    let scratch = Scratch::new();
    init(
        &keeper(&scratch, "test"),
        MethodKind::Passphrase,
        Some("correct horse battery"),
    );
    let keeper = Keeper::open(
        &scratch.seal_path(),
        SealConf {
            unseal_max_delay: Duration::from_millis(30),
            ..SealConf::with_purpose("test")
        },
    )
    .unwrap();
    keeper.set_base_delay(Duration::from_millis(20));
    for _ in 0..3 {
        let _ = keeper.unseal(UnsealInput::Passphrase(Zeroizing::new("wrong horse".into())));
        assert!(keeper.status().retry_after_ms <= 30);
        std::thread::sleep(Duration::from_millis(35));
    }
}

#[test]
fn a_production_instance_refuses_a_passphrase_seal_under_the_server_cost() {
    cheap_passphrases();
    let scratch = Scratch::new();
    let err = keeper(&scratch, "production")
        .init(Init {
            method: MethodKind::Passphrase,
            passphrase: Some(Zeroizing::new("correct horse battery".into())),
        })
        .unwrap_err();
    assert!(err.to_string().contains("256 MiB"), "{err}");
    // a cheap seal made elsewhere does not open on a production instance either
    init(
        &keeper(&scratch, "test"),
        MethodKind::Passphrase,
        Some("correct horse battery"),
    );
    let production = keeper(&scratch, "production");
    let err = production
        .unseal(UnsealInput::Passphrase(Zeroizing::new("correct horse battery".into())))
        .unwrap_err();
    assert!(err.to_string().contains("256 MiB"), "{err}");
}

#[test]
fn a_shamir_seal_opens_with_its_threshold_of_shares_and_refuses_a_wrong_one_on_entry() {
    let scratch = Scratch::new();
    let issued = init(&keeper(&scratch, "production"), MethodKind::Shamir, None);
    assert_eq!((issued.method_shares.len(), issued.recovery_shares.len()), (5, 5));
    let keeper = keeper(&scratch, "production");
    keeper.set_base_delay(Duration::from_millis(10));
    let share = |text: &Zeroizing<String>| UnsealInput::Share(Zeroizing::new(text.to_string()));
    assert_eq!(
        keeper.unseal(share(&issued.method_shares[4])).unwrap().state,
        State::Unsealing
    );
    // a share of another split is refused at once and counted; the one entered stays
    assert!(keeper
        .unseal(share(&foreign_share()))
        .unwrap_err()
        .to_string()
        .contains("wrong share"));
    assert!(keeper
        .unseal(UnsealInput::Share(Zeroizing::new("not a share".into())))
        .is_err());
    std::thread::sleep(Duration::from_millis(30));
    // the same share twice is refused, not counted
    let failures = keeper.status().failures;
    assert!(keeper
        .unseal(share(&issued.method_shares[4]))
        .unwrap_err()
        .to_string()
        .contains("already"));
    assert_eq!(keeper.status().failures, failures);
    let status = keeper.unseal(share(&issued.method_shares[1])).unwrap();
    assert_eq!((status.state, status.entered, status.needed), (State::Unsealing, 2, 3));
    // a recovery share is not one of the method's
    assert!(keeper.unseal(share(&issued.recovery_shares[0])).is_err());
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        keeper.unseal(share(&issued.method_shares[0])).unwrap().state,
        State::Unsealed
    );
}

#[test]
fn the_recovery_shares_open_any_method_and_rekey_changes_only_the_method() {
    cheap_passphrases();
    let scratch = Scratch::new();
    let made = keeper(&scratch, "test");
    let issued = init(&made, MethodKind::Passphrase, Some("correct horse battery"));
    made.set_bootstrap_secret("transactional-password", b"pg-secret")
        .unwrap();
    let data_key = made.secret_value_key().unwrap();
    let root_id = made.status().root_id;
    // a lost passphrase: three recovery shares open the seal
    let keeper = keeper(&scratch, "test");
    for share in &issued.recovery_shares[1..4] {
        keeper
            .unseal(UnsealInput::Recovery(Zeroizing::new(share.to_string())))
            .unwrap();
    }
    assert!(keeper.is_unsealed());
    assert_eq!(keeper.status().root_id, root_id);
    // rekey to shamir: new method shares, the same root key, recovery shares and bootstrap secrets
    let before = keeper.seal_file().unwrap();
    let rekeyed = keeper
        .rekey(rotate::Rekey {
            shares: texts(&issued.recovery_shares[..3]),
            method: MethodKind::Shamir,
            passphrase: None,
        })
        .unwrap();
    assert_eq!(rekeyed.method_shares.len(), 5);
    assert!(rekeyed.recovery_shares.is_empty());
    let after = keeper.seal_file().unwrap();
    assert_eq!(
        (after.method_kind(), after.generation, after.root_id.as_str()),
        ("shamir", 2, root_id.as_str())
    );
    assert_eq!(
        after.recovery.as_ref().unwrap().share_hashes,
        before.recovery.as_ref().unwrap().share_hashes
    );
    let reopened = reopen(&scratch, "test");
    for share in &rekeyed.method_shares[..3] {
        reopened
            .unseal(UnsealInput::Share(Zeroizing::new(share.to_string())))
            .unwrap();
    }
    assert_eq!(
        reopened
            .bootstrap_secret("transactional-password")
            .unwrap()
            .unwrap()
            .as_slice(),
        b"pg-secret"
    );
    assert_eq!(reopened.secret_value_key().unwrap(), data_key);
    // too few or wrong recovery shares do not rekey
    assert!(reopened
        .rekey(rotate::Rekey {
            shares: texts(&issued.recovery_shares[..2]),
            method: MethodKind::Shamir,
            passphrase: None
        })
        .is_err());
    let mut wrong = texts(&issued.recovery_shares[..2]);
    wrong.push(foreign_share());
    assert!(reopened
        .rekey(rotate::Rekey {
            shares: wrong,
            method: MethodKind::Shamir,
            passphrase: None
        })
        .unwrap_err()
        .to_string()
        .contains("wrong recovery share"));
}

#[test]
fn rotate_root_rewraps_everything_and_one_stopped_before_its_rename_keeps_the_old_root() {
    let scratch = Scratch::new();
    let keeper = Arc::new(keeper(&scratch, "production"));
    let issued = init(&keeper, MethodKind::Shamir, None);
    keeper.set_bootstrap_secret("crypt-pass", b"field-key-pass").unwrap();
    let data_key = keeper.secret_value_key().unwrap();
    let old_root = keeper.status().root_id;
    let old_text = std::fs::read_to_string(scratch.seal_path()).unwrap();

    // stopped before the rename: the new file was staged and never put in place
    {
        let mut inner = lock(&keeper.inner);
        let (seal, _, _) = keeper
            .rotated(
                &mut inner,
                &rotate::RotateRoot {
                    shares: texts(&issued.recovery_shares[..3]),
                    passphrase: None,
                },
            )
            .unwrap();
        assert_ne!(seal.root_id, old_root);
        std::mem::forget(file::stage(&scratch.seal_path(), &seal).unwrap());
    }
    assert_eq!(std::fs::read_to_string(scratch.seal_path()).unwrap(), old_text);
    let after_crash = reopen(&scratch, "production");
    for share in &issued.method_shares[..3] {
        after_crash
            .unseal(UnsealInput::Share(Zeroizing::new(share.to_string())))
            .unwrap();
    }
    assert_eq!(after_crash.status().root_id, old_root);
    assert_eq!(
        after_crash.bootstrap_secret("crypt-pass").unwrap().unwrap().as_slice(),
        b"field-key-pass"
    );

    // a rotation that completes: a new root key, new method shares, the same recovery shares and secrets
    let rotated = after_crash
        .rotate_root(rotate::RotateRoot {
            shares: texts(&issued.recovery_shares[2..5]),
            passphrase: None,
        })
        .unwrap();
    assert_eq!(rotated.method_shares.len(), 5);
    let new_root = after_crash.status().root_id;
    assert_ne!(new_root, old_root);
    let next = reopen(&scratch, "production");
    for share in &issued.recovery_shares[..3] {
        next.unseal(UnsealInput::Recovery(Zeroizing::new(share.to_string())))
            .unwrap();
    }
    assert_eq!(next.status().root_id, new_root);
    assert_eq!(
        next.bootstrap_secret("crypt-pass").unwrap().unwrap().as_slice(),
        b"field-key-pass"
    );
    assert_eq!(next.secret_value_key().unwrap(), data_key);
    // the old method shares no longer open it
    let old_shares = reopen(&scratch, "production");
    old_shares.set_base_delay(Duration::from_millis(1));
    assert!(old_shares
        .unseal(UnsealInput::Share(Zeroizing::new(issued.method_shares[0].to_string())))
        .is_err());
}

#[test]
fn a_bootstrap_secret_moved_to_another_name_does_not_open() {
    let scratch = Scratch::new();
    let keeper = keeper(&scratch, "test");
    keeper.start(Mode::Operator).unwrap();
    keeper.set_bootstrap_secret("a", b"first").unwrap();
    keeper.set_bootstrap_secret("b", b"second").unwrap();
    keeper.set_bootstrap_secret("b", b"second again").unwrap();
    let mut seal = file::load(&scratch.seal_path()).unwrap().unwrap();
    assert_eq!(seal.bootstrap["b"].generation, 2);
    let moved = seal.bootstrap["a"].clone();
    seal.bootstrap.insert("b".to_string(), moved);
    file::stage(&scratch.seal_path(), &seal).unwrap().commit().unwrap();
    let reopened = reopen(&scratch, "test");
    reopened.start(Mode::Operator).unwrap();
    assert_eq!(reopened.bootstrap_secret("a").unwrap().unwrap().as_slice(), b"first");
    assert!(reopened.bootstrap_secret("b").is_err());
    assert!(reopened.bootstrap_secret("none").unwrap().is_none());
    keeper.seal();
    assert!(keeper.bootstrap_secret("a").is_err(), "sealed: no bootstrap secret");
}

/// A stub of `systemd-creds` in `dir`: base64 for encrypt and decrypt, failing while `fail` is in `dir`.
fn stub_systemd_creds(dir: &std::path::Path) -> String {
    let path = dir.join("systemd-creds");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n[ -e '{}/fail' ] && {{ echo 'no TPM' >&2; exit 1; }}\ncase \"$1\" in\n  encrypt) base64 ;;\n  decrypt) base64 -d ;;\n  *) exit 2 ;;\nesac\n",
            dir.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

#[cfg(unix)]
#[test]
fn a_tpm_first_start_holds_the_shares_once_and_a_failed_unseal_waits_for_the_recovery_shares() {
    let scratch = Scratch::new();
    let command = stub_systemd_creds(&scratch.0);
    let conf = SealConf {
        method: "tpm".to_string(),
        systemd_creds_command: command.clone(),
        ..SealConf::with_purpose("production")
    };
    let first = Keeper::open(&scratch.seal_path(), conf.clone()).unwrap();
    assert_eq!(first.start(Mode::Operator).unwrap(), Start::Unsealed);
    assert!(first.status().shares_waiting);
    let shares = first.take_shares().unwrap();
    assert_eq!(shares.len(), 5);
    assert!(first.take_shares().is_err(), "the shares are read once");
    assert!(!first.status().shares_waiting);
    let seal = file::load(&scratch.seal_path()).unwrap().unwrap();
    assert_eq!(
        (seal.method_kind(), seal.method_text("command")),
        ("tpm", Some(command.as_str()))
    );

    // an unattended reboot of the same host unseals itself
    let reboot = Keeper::open(&scratch.seal_path(), conf.clone()).unwrap();
    assert_eq!(reboot.start(Mode::Operator).unwrap(), Start::Unsealed);
    assert_eq!(reboot.status().root_id, first.status().root_id);

    // the TPM refuses: the server waits sealed, and the recovery shares open it
    std::fs::write(scratch.0.join("fail"), "").unwrap();
    let failed = Keeper::open(&scratch.seal_path(), conf.clone()).unwrap();
    assert!(
        matches!(failed.start(Mode::Operator).unwrap(), Start::Waiting(reason) if reason.starts_with("auto-unseal failed"))
    );
    assert!(failed.status().reason.contains("no TPM"));
    let unattended = Keeper::open(&scratch.seal_path(), conf).unwrap();
    assert!(unattended.start(Mode::Unattended).is_err());
    for share in &shares[..3] {
        failed
            .unseal(UnsealInput::Recovery(Zeroizing::new(share.to_string())))
            .unwrap();
    }
    assert!(failed.is_unsealed());
    assert!(failed.status().reason.is_empty());
}

#[test]
fn a_bundle_restored_on_a_new_runtime_unseals_with_the_recovery_shares() {
    let scratch = Scratch::new();
    let made = keeper(&scratch, "production");
    let issued = init(&made, MethodKind::Shamir, None);
    made.set_bootstrap_secret("embedded-postgres-password", b"pw").unwrap();
    let bundle = made.export_bundle().unwrap();
    let parsed: Value = serde_json::from_str(&bundle).unwrap();
    assert_eq!(parsed["format"], export::BUNDLE_FORMAT);
    assert!(parsed["keyring_id"].is_null());
    assert!(!bundle.contains("pw\""), "the bundle holds wrapped values only");

    let elsewhere = Scratch::new();
    let restored = keeper(&elsewhere, "production");
    restored.restore(&bundle).unwrap();
    assert!(restored
        .restore(&bundle)
        .unwrap_err()
        .to_string()
        .contains("has a seal already"));
    assert_eq!(restored.status().state, State::Sealed);
    for share in &issued.recovery_shares[..3] {
        restored
            .unseal(UnsealInput::Recovery(Zeroizing::new(share.to_string())))
            .unwrap();
    }
    assert_eq!(restored.status().root_id, made.status().root_id);
    assert_eq!(
        restored
            .bootstrap_secret("embedded-postgres-password")
            .unwrap()
            .unwrap()
            .as_slice(),
        b"pw"
    );
    assert!(restored.restore("{\"format\": \"other\"}").is_err());
}

/// [`keeper`], for a test whose own variable is named `keeper`.
fn reopen(scratch: &Scratch, purpose: &str) -> Keeper {
    keeper(scratch, purpose)
}
