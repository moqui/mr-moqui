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
