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
//! and the downloaded Chromium it links.

use std::path::{Path, PathBuf};

use super::{data_hash, scratch_runtime};

/// A runtime with the framework, `component/plain` and the dev component `dev-component/devonly`.
fn runtime() -> PathBuf {
    let root = std::env::temp_dir().join(format!("moqui-fixture-key-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
    std::fs::create_dir_all(root.join("conf")).unwrap();
    std::os::unix::fs::symlink(
        std::fs::canonicalize(repo.join("base-component")).unwrap(),
        root.join("base-component"),
    )
    .unwrap();
    for (dir, component) in [("component", "plain"), ("dev-component", "devonly")] {
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
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework")).unwrap()
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
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The scratch runtime links the runtime's `chromium/`, so PDF in a spec finds the Chromium of `./moq chromium-download`.
#[test]
fn the_scratch_runtime_links_the_downloaded_chromium() {
    let root = std::env::temp_dir().join(format!("moqui-fixture-chromium-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("conf")).unwrap();
    std::fs::create_dir_all(root.join("base-component")).unwrap();
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
        let _ = std::fs::remove_dir_all(dir);
    }
}
