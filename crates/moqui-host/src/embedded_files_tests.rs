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

use super::{ensure_runtime, names};

#[test]
fn a_skeleton_is_written_only_for_the_runtime_s_own_missing_framework() {
    let root = std::env::temp_dir().join(format!("moqui-ensure-runtime-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let runtime = root.join("runtime");
    // MOQUI_FRAMEWORK elsewhere: nothing is written.
    ensure_runtime(&runtime, &root.join("framework")).unwrap();
    assert!(!runtime.exists());
    let framework = moqui_core::context::default_framework_dir(&runtime);
    ensure_runtime(&runtime, &framework).unwrap();
    assert!(framework.join("component.xml").is_file());
    assert!(runtime.join("base-component/tools/service/moqui.rest.xml").is_file());
    assert!(runtime.join("base-component/webroot/screen/webroot/rest.xml").is_file());
    assert!(runtime.join("base-component/webroot/screen/webroot/apps.xml").is_file());
    // A framework that is there is left alone.
    std::fs::remove_file(framework.join("conf/MoquiDefaultConf.xml")).unwrap();
    ensure_runtime(&runtime, &framework).unwrap();
    assert!(!framework.join("conf/MoquiDefaultConf.xml").exists());
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
