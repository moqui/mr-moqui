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

use super::{ensure_runtime, missing_browser_libs, names};

#[test]
fn a_new_runtime_gets_its_directories_and_reads_the_built_in_files_in_place() {
    let root = std::env::temp_dir().join(format!("moqui-ensure-runtime-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let runtime = root.join("runtime");
    let framework = moqui_core::context::default_framework_dir(&runtime);
    ensure_runtime(&runtime, &framework).unwrap();
    let base = runtime.join("components/base");
    for dir in ["framework", "tools", "webroot"] {
        let dir = base.join(dir);
        assert!(dir.is_dir(), "{} is made", dir.display());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "nothing is written in {}",
            dir.display()
        );
    }
    assert!(!runtime.join("conf").exists(), "no conf is written");
    assert!(moqui_core::context::layout::is_runtime(&runtime));

    // Each built-in file not on disk is read in place.
    assert!(moqui_lib::vfs::is_file(framework.join("component.xml")));
    assert!(moqui_lib::vfs::is_file(base.join("tools/service/moqui.rest.xml")));
    assert!(moqui_lib::vfs::is_file(base.join("webroot/screen/webroot/rest.xml")));
    assert!(
        moqui_lib::vfs::is_file(base.join("webroot/screen/webroot/qapps.xml")),
        "the placeholder page"
    );
    assert!(moqui_lib::vfs::is_file(runtime.join("components/components.xml")));
    assert!(moqui_lib::vfs::is_file(runtime.join("conf/MoquiProductionConf.xml")));
    let dev = moqui_lib::vfs::read_to_string(runtime.join("conf/MoquiDevConf.xml")).unwrap();
    assert!(dev.contains("<moqui-conf"), "{dev}");
    let sets = moqui_core::context::layout::component_dirs(&runtime, &[moqui_core::context::layout::Set::Base]);
    assert_eq!(sets.len(), 3, "{sets:?}");

    // A file on disk is read in place of the built-in one.
    std::fs::create_dir_all(runtime.join("conf")).unwrap();
    std::fs::write(runtime.join("conf/MoquiDevConf.xml"), "<moqui-conf/>").unwrap();
    assert_eq!(
        moqui_lib::vfs::read_to_string(runtime.join("conf/MoquiDevConf.xml")).unwrap(),
        "<moqui-conf/>"
    );

    // A framework MOQUI_FRAMEWORK names elsewhere is not made, and is mounted where it is.
    let elsewhere = root.join("framework");
    ensure_runtime(&runtime, &elsewhere).unwrap();
    assert!(!elsewhere.exists());
    assert!(moqui_lib::vfs::is_file(elsewhere.join("component.xml")));
    for set in moqui_embedded::Set::ALL {
        moqui_lib::vfs::unmount(&set.dir_in(&runtime, &framework));
        moqui_lib::vfs::unmount(&set.dir_in(&runtime, &elsewhere));
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_compared_components_are_named_in_a_list() {
    use moqui_embedded::Set;
    let dir = std::path::PathBuf::new;
    assert_eq!(names(&[(Set::Framework, dir())]), "framework");
    assert_eq!(
        names(&[(Set::Framework, dir()), (Set::Webroot, dir())]),
        "framework and webroot"
    );
    assert_eq!(
        names(&[(Set::Framework, dir()), (Set::Tools, dir()), (Set::Webroot, dir())]),
        "framework, tools and webroot"
    );
}

#[test]
fn shared_files_come_after_the_runtime_and_the_binary_and_merge_addons() {
    let base = moqui_lib::private_fs::new_dir_in(&std::env::temp_dir(), "moqui-runtime-layer").unwrap();
    let local = base.join("local");
    let shared = base.join("shared");
    let framework = moqui_core::context::layout::framework_dir(&local);
    let shared_framework = moqui_core::context::layout::framework_dir(&shared);
    std::fs::create_dir_all(&shared_framework).unwrap();
    std::fs::write(shared_framework.join("component.xml"), "shared").unwrap();
    let addon = shared.join("components/addon/extra");
    std::fs::create_dir_all(&addon).unwrap();
    std::fs::write(addon.join("component.xml"), "<component name='extra'/>").unwrap();
    std::fs::write(shared.join("MoquiConf.xml"), "private").unwrap();
    std::fs::create_dir_all(shared.join("conf")).unwrap();
    std::fs::write(shared.join("conf/SharedOnlyConf.xml"), "<moqui-conf/>").unwrap();
    std::fs::write(shared.join("conf/MoquiDevConf.xml"), "stale").unwrap();
    super::mount_with_shared(&local, &framework, Some(&shared));
    // a file the binary has is read from it, not from a stale shared copy
    let built_in = moqui_lib::vfs::read_to_string(framework.join("component.xml")).unwrap();
    assert!(built_in.contains("<component"), "{built_in}");
    assert_ne!(
        moqui_lib::vfs::read_to_string(local.join("conf/MoquiDevConf.xml")).unwrap(),
        "stale"
    );
    // a file neither the runtime nor the binary has is read from the shared runtime
    assert_eq!(
        moqui_lib::vfs::read_to_string(local.join("conf/SharedOnlyConf.xml")).unwrap(),
        "<moqui-conf/>"
    );
    assert!(
        moqui_core::context::layout::component_dirs(&local, &[moqui_core::context::layout::Set::Addon])
            .iter()
            .any(|(_, p)| p.ends_with("addon/extra"))
    );
    assert!(!moqui_lib::vfs::exists(local.join("MoquiConf.xml")));
    std::fs::create_dir_all(&framework).unwrap();
    std::fs::write(framework.join("component.xml"), "local").unwrap();
    assert_eq!(
        moqui_lib::vfs::read_to_string(framework.join("component.xml")).unwrap(),
        "local"
    );
    for set in moqui_embedded::Set::ALL {
        moqui_lib::vfs::unmount(&set.dir_in(&local, &framework));
    }
    for tree in ["components", "conf"] {
        moqui_lib::vfs::mount_shared(&local.join(tree), None);
    }
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_component_names_each_library_whose_listed_files_are_missing() {
    let root = std::env::temp_dir().join(format!("moqui-missing-libs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let component = root.join("shop");
    let put = |path: &str, text: &str| {
        let file = component.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    };
    assert!(
        missing_browser_libs(&component).is_empty(),
        "no build/js-build.json, nothing to check"
    );
    put(
        "build/js-build.json",
        r#"{"output": "screen/shop/libs", "libraries": []}"#,
    );
    put(
        "screen/shop/libs/MANIFEST.json",
        r#"{"libraries": [
            {"id": "math", "files": [{"target": "client", "path": "math.js"}, {"target": "server", "path": "math.js"}]},
            {"id": "dates", "files": [{"target": "server", "path": "dates.js"}]}]}"#,
    );
    put("screen/shop/libs/math.js", "client");
    put("script/libs/math.js", "server");
    assert_eq!(
        missing_browser_libs(&component),
        vec!["shop/dates"],
        "a server file is looked for under script/libs"
    );
    put("script/libs/dates.js", "server");
    assert!(missing_browser_libs(&component).is_empty());
    put("build/js-build.json", "{");
    assert_eq!(
        missing_browser_libs(&component),
        vec!["shop (unreadable build/js-build.json)"]
    );
    let _ = std::fs::remove_dir_all(&root);
}
