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

//! The scratch runtime: its snapshot key, the components a load reads and not every component directory there is,
//! and the downloaded Chromium it links; and the snapshots a run's end prunes.

use std::path::{Path, PathBuf};

use super::{data_hash, prune_snapshots, scratch_runtime, USED_MARK};

/// A runtime with the framework, `components/addon/plain` and the dev component `components/dev/devonly`.
fn runtime() -> PathBuf {
    // Beside the scratch runtime, on `/tmp`, so a file link stays on one volume.
    let root = PathBuf::from("/tmp").join(format!("moqui-fixture-key-{}", std::process::id()));
    super::super::interrupt::remove_scratch(&root);
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
    std::fs::create_dir_all(root.join("conf")).unwrap();
    let base = std::fs::canonicalize(repo.join("components/base")).unwrap();
    std::fs::create_dir_all(root.join("components")).unwrap();
    super::link(&base, &root.join("components/base")).unwrap();
    let manifest = base.parent().unwrap().join("components.xml");
    if manifest.is_file() {
        std::fs::copy(manifest, root.join("components/components.xml")).unwrap();
    }
    for (dir, component) in [("components/addon", "plain"), ("components/dev", "devonly")] {
        let location = root.join(dir).join(component);
        std::fs::create_dir_all(location.join("entity")).unwrap();
        std::fs::write(
            location.join("component.xml"),
            format!(r#"<component name="{component}"/>"#),
        )
        .unwrap();
        std::fs::write(location.join("entity/Things.xml"), "<entities/>").unwrap();
    }
    std::fs::canonicalize(root).unwrap()
}

/// The instance `MoquiConf.xml`, which sets `instance_purpose`.
fn set_purpose(runtime: &Path, purpose: &str) {
    std::fs::write(
        runtime.join("MoquiConf.xml"),
        format!(r#"<moqui-conf><default-property name="instance_purpose" value="{purpose}"/></moqui-conf>"#),
    )
    .unwrap();
}

fn framework() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/components/base/framework"))
        .unwrap()
}

/// The same component directories give another snapshot on a dev instance, which loads the dev component, than on a
/// production one, which does not; the scratch runtimes of one runtime, which link the same directories, get the same
/// one.
#[test]
fn the_snapshot_key_follows_the_components_that_load() {
    let runtime = runtime();
    set_purpose(&runtime, "dev");
    let first = scratch_runtime(&runtime).unwrap();
    let second = scratch_runtime(&runtime).unwrap();
    let dev = data_hash(&first, &framework()).unwrap();
    assert_eq!(dev, data_hash(&second, &framework()).unwrap());
    // a purpose set for the whole process (moqui test sets dev for a dev component's specs) wins over the conf
    if moqui_lib::env::get_prop_or_env("instance_purpose").is_empty() {
        set_purpose(&runtime, "production");
        assert_ne!(dev, data_hash(&first, &framework()).unwrap());
    }
    for dir in [first, second, runtime] {
        super::super::interrupt::remove_scratch(&dir);
    }
}

/// The scratch runtime links the runtime's `chromium/`, so PDF in a spec finds the Chromium of `./moq chromium-download`.
#[test]
fn the_scratch_runtime_links_the_downloaded_chromium() {
    let root = std::env::temp_dir().join(format!("moqui-fixture-chromium-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("conf")).unwrap();
    std::fs::create_dir_all(root.join("components/base")).unwrap();
    std::fs::create_dir_all(root.join("chromium")).unwrap();
    std::fs::write(root.join("chromium/chrome"), "").unwrap();
    let runtime = std::fs::canonicalize(&root).unwrap();
    let scratch = scratch_runtime(&runtime).unwrap();
    assert!(scratch.join("chromium/chrome").is_file());
    assert_eq!(
        crate::chromium::downloaded_chromium(&scratch),
        Some(scratch.join("chromium/chrome"))
    );
    for dir in [scratch, runtime] {
        super::super::interrupt::remove_scratch(&dir);
    }
}

/// The scratch runtime copies the conf with its topology directories, so a `MOQUI_CONF` such as
/// `conf/edge/MoquiEdgeConf.xml` is found there; a link to a directory (here a loop) is not followed.
#[test]
fn the_scratch_runtime_copies_the_conf_directories() {
    let root = std::env::temp_dir().join(format!("moqui-fixture-conf-{}", std::process::id()));
    super::super::interrupt::remove_scratch(&root);
    std::fs::create_dir_all(root.join("conf/edge")).unwrap();
    std::fs::create_dir_all(root.join("components/base")).unwrap();
    std::fs::write(root.join("conf/MoquiDevConf.xml"), "<moqui-conf/>").unwrap();
    std::fs::write(root.join("conf/edge/MoquiEdgeConf.xml"), "<moqui-conf/>").unwrap();
    super::link(&root.join("conf"), &root.join("conf/edge/loop")).unwrap();
    let runtime = std::fs::canonicalize(&root).unwrap();
    let scratch = scratch_runtime(&runtime).unwrap();
    assert!(scratch.join("conf/MoquiDevConf.xml").is_file());
    assert!(scratch.join("conf/edge/MoquiEdgeConf.xml").is_file());
    assert!(!scratch.join("conf/edge/loop").exists());
    let loop_link = runtime.join("conf/edge/loop");
    let _ = std::fs::remove_file(&loop_link);
    let _ = std::fs::remove_dir(&loop_link);
    super::super::interrupt::remove_scratch(&scratch);
    super::super::interrupt::remove_scratch(&runtime);
}

/// Removing a scratch runtime takes off the link to a component set and leaves the directory that link names.
#[test]
fn removing_a_scratch_runtime_leaves_the_linked_directory() {
    let root = std::env::temp_dir().join(format!("moqui-fixture-keep-{}", std::process::id()));
    super::super::interrupt::remove_scratch(&root);
    let marked = root.join("marked");
    std::fs::create_dir_all(&marked).unwrap();
    std::fs::write(marked.join("keep.txt"), "stay").unwrap();
    std::fs::create_dir_all(root.join("runtime/conf")).unwrap();
    std::fs::create_dir_all(root.join("runtime/components")).unwrap();
    super::link(&marked, &root.join("runtime/components/base")).unwrap();
    let runtime = std::fs::canonicalize(root.join("runtime")).unwrap();
    let scratch = scratch_runtime(&runtime).unwrap();
    assert!(scratch.join("components/base/keep.txt").is_file());
    super::super::interrupt::remove_scratch(&scratch);
    assert_eq!(std::fs::read_to_string(marked.join("keep.txt")).unwrap(), "stay");
    assert!(!scratch.exists());
    super::super::interrupt::remove_scratch(&runtime);
    let _ = std::fs::remove_dir_all(&root);
}

/// At a run's end the other snapshots go once no run has used them for an hour: the one the run used stays however old,
/// one another run restored a minute ago stays, an old one goes, and so does one with no mark whose dump is old; a
/// directory that is not a snapshot stays.
#[test]
fn a_runs_end_prunes_the_snapshots_no_run_has_used_for_an_hour() {
    let save = std::env::temp_dir().join(format!("moqui-fixture-prune-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&save);
    let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 60 * 60);
    let snapshot = |name: &str, dump_age: Option<std::time::SystemTime>, mark: Option<std::time::SystemTime>| {
        let dir = save.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let dump = std::fs::File::create(dir.join("moqui.dump")).unwrap();
        if let Some(time) = dump_age {
            dump.set_modified(time).unwrap();
        }
        if let Some(time) = mark {
            std::fs::File::create(dir.join(USED_MARK))
                .unwrap()
                .set_modified(time)
                .unwrap();
        }
        dir
    };
    let kept = snapshot("test-kept", Some(two_hours_ago), Some(two_hours_ago));
    let recent = snapshot("test-recent", Some(two_hours_ago), Some(std::time::SystemTime::now()));
    let old = snapshot("test-old", Some(two_hours_ago), Some(two_hours_ago));
    let unmarked = snapshot("test-unmarked", Some(two_hours_ago), None);
    let other = save.join("other");
    std::fs::create_dir_all(&other).unwrap();

    prune_snapshots(&save, &kept);

    assert!(kept.is_dir(), "the snapshot the run used stays");
    assert!(recent.is_dir(), "a snapshot another run used a minute ago stays");
    assert!(!old.exists(), "a snapshot no run has used for two hours goes");
    assert!(!unmarked.exists(), "a snapshot with no mark goes by its dump's age");
    assert!(other.is_dir(), "a directory that is not a snapshot stays");
    // a second prune, as each worker's end makes, finds them gone and says nothing of it
    prune_snapshots(&save, &kept);
    let _ = std::fs::remove_dir_all(&save);
}
