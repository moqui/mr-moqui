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

use super::*;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(label: &str) -> (Scratch, PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("dev-aide-confine-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let runtime = base.join("runtime");
    let root = runtime.join("components/addon/site");
    std::fs::create_dir_all(root.join("service")).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(runtime.join("db")).unwrap();
    std::fs::write(root.join("service/A.xml"), "<services/>").unwrap();
    let root = root.canonicalize().unwrap();
    (Scratch(base), root, runtime)
}

#[test]
fn a_relative_path_resolves_inside_its_root() {
    let (_scratch, root, runtime) = scratch("ok");
    let found = resolve(&root, &runtime, &[], "service/A.xml").unwrap();
    assert_eq!(found.path, root.join("service/A.xml"));
    assert_eq!(found.relative, "service/A.xml");
    // a file that does not exist yet, under a directory that does not either
    let new = resolve(&root, &runtime, &[], "screen\\New/Page.xml").unwrap();
    assert_eq!(new.relative, "screen/New/Page.xml");
    assert!(new.path.starts_with(&root));
    assert_eq!(resolve(&root, &runtime, &[], "").unwrap().path, root);
}

#[test]
fn what_leads_out_or_is_denied_is_refused() {
    let (_scratch, root, runtime) = scratch("refused");
    for (path, why) in [
        ("../other/x.xml", ".."),
        ("service/../../x", ".."),
        ("/etc/passwd", "absolute"),
        ("\\\\server\\share\\x", "absolute"),
        ("\\\\?\\C:\\x", "absolute"),
        ("C:\\Windows\\x", "absolute"),
        ("c:/x", "absolute"),
        (".git/config", ".git"),
        ("service/.git/x", ".git"),
        ("target/debug/x", "target"),
        ("a\0b", "NUL"),
    ] {
        let err = resolve(&root, &runtime, &[], path).unwrap_err().to_string();
        assert!(err.contains(why), "{path}: {err}");
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_that_leads_out_is_refused() {
    let (scratch, root, runtime) = scratch("symlink");
    let outside = scratch.0.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "x").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    let err = resolve(&root, &runtime, &[], "link/secret.txt")
        .unwrap_err()
        .to_string();
    assert!(err.contains("outside its root"), "{err}");
    // into the runtime's database, through a symlink inside the root
    std::os::unix::fs::symlink(runtime.join("db"), root.join("db-link")).unwrap();
    assert!(resolve(&root, &runtime, &[], "db-link/x").is_err());
}

#[cfg(unix)]
#[test]
fn a_symlink_inside_the_root_resolves_to_where_it_leads() {
    let (_scratch, root, runtime) = scratch("symlink-inside");
    std::fs::create_dir_all(root.join("components/addon/upstream/entity")).unwrap();
    std::fs::create_dir_all(root.join("components/addon/my-site")).unwrap();
    std::os::unix::fs::symlink(
        root.join("components/addon/upstream"),
        root.join("components/addon/my-site/up"),
    )
    .unwrap();
    let found = resolve(&root, &runtime, &[], "components/addon/my-site/up/entity/X.xml").unwrap();
    assert_eq!(found.relative, "components/addon/my-site/up/entity/X.xml");
    assert_eq!(found.real, "components/addon/upstream/entity/X.xml");
    assert_eq!(
        resolve(&root, &runtime, &[], "service/A.xml").unwrap().real,
        "service/A.xml"
    );
}

#[test]
fn a_runtime_root_does_not_reach_the_denied_directories() {
    let (_scratch, _root, runtime) = scratch("runtime");
    let runtime_root = runtime.canonicalize().unwrap();
    // SEC-DEV-04: the saved databases, the SFTP root, the installed programs and copies of .env too
    for path in [
        "db/x",
        "log/moqui.log",
        ".env",
        "cert/key.pem",
        "txlog/x",
        "tmp/x",
        "save/copy/db",
        "sftp/x",
        "install/x",
        "opensearch/config/x",
        ".env.bak",
        ".env.local",
    ] {
        let err = resolve(&runtime_root, &runtime, &[], path).unwrap_err().to_string();
        assert!(err.contains("does not open"), "{path}: {err}");
    }
    assert!(resolve(&runtime_root, &runtime, &[], "conf/MoquiDevConf.xml").is_ok());
    assert!(resolve(&runtime_root, &runtime, &[], "environment.md").is_ok());
}

/// SEC-DEV-04: the conf files are the full tier's; a root a non-admin adds is in the runtime or a root there is.
#[test]
fn conf_files_are_the_full_tier_s_and_a_non_admin_s_root_stays_near() {
    let (scratch, root, runtime) = scratch("conf");
    let runtime_root = runtime.canonicalize().unwrap();
    let conf = resolve(&runtime_root, &runtime, &[], "conf/MoquiDevConf.xml").unwrap();
    assert!(hidden_below_full(&conf.path, &runtime));
    let service = resolve(&root, &runtime, &[], "service/A.xml").unwrap();
    assert!(!hidden_below_full(&service.path, &runtime));
    let elsewhere = scratch.0.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let elsewhere = elsewhere.canonicalize().unwrap();
    assert!(root_location_open(&root.join("service"), &runtime, &[]));
    assert!(!root_location_open(&elsewhere, &runtime, &[]));
    assert!(!root_location_open(Path::new("/"), &runtime, &[&root]));
    assert!(root_location_open(&elsewhere, &runtime, &[&elsewhere]));
}

#[cfg(windows)]
#[test]
fn windows_case_is_folded() {
    assert!(inside(Path::new("C:\\Work\\Site\\x.xml"), Path::new("c:\\work\\site")));
    assert!(!inside(
        Path::new("C:\\Work\\Sitex\\x.xml"),
        Path::new("c:\\work\\site")
    ));
}

#[test]
fn binary_is_a_nul_byte_early() {
    assert!(is_binary(b"PNG\0\x01"));
    assert!(!is_binary("<a>ü</a>".as_bytes()));
}

/// A disposable runtime under the repository's `target`, its components copied in (the testing skill's): the repository
/// is their root, as a copied component has no `.git` of its own (crate::roots), beside the repository's build output.
/// Also the scratch runtime a test run makes elsewhere (`moqui test`), its component sets linked to that runtime's.
struct UnderTarget {
    _scratch: Scratch,
    root: PathBuf,
    runtime: PathBuf,
    /// The scratch runtime of a test run.
    run: PathBuf,
    /// The component as the test run loads it, through its link.
    component: PathBuf,
}

fn runtime_under_target(label: &str) -> UnderTarget {
    let base = std::env::temp_dir().join(format!("dev-aide-confine-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("repo");
    let runtime = root.join("target/rt");
    for dir in [
        ".git",
        "target/debug",
        "target/rt/components/dev/dev-aide/screen",
        "target/rt/conf",
        "target/rt/db",
    ] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(
        runtime.join("components/dev/dev-aide/screen/Workspace.xml"),
        "<screen/>",
    )
    .unwrap();
    std::fs::write(runtime.join("conf/MoquiDevConf.xml"), "<moqui-conf/>").unwrap();
    std::fs::write(root.join("target/debug/moqui"), "x").unwrap();
    let run = base.join("mt-run");
    std::fs::create_dir_all(run.join("components")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(runtime.join("components/dev"), run.join("components/dev")).unwrap();
    let component = run.join("components/dev/dev-aide");
    UnderTarget {
        _scratch: Scratch(base),
        root: root.canonicalize().unwrap(),
        runtime,
        run,
        component,
    }
}

const SCREEN: &str = "target/rt/components/dev/dev-aide/screen/Workspace.xml";

#[test]
fn a_runtime_under_a_root_s_target_is_not_build_output() {
    let UnderTarget {
        _scratch,
        root,
        runtime,
        ..
    } = runtime_under_target("runtime-in-target");
    assert_eq!(resolve(&root, &runtime, &[], SCREEN).unwrap().real, SCREEN);
    // a file not made yet
    assert!(resolve(&root, &runtime, &[], "target/rt/components/dev/dev-aide/screen/New.xml").is_ok());
    // its conf: no root of its own, as the repository holds it (crate::roots), and still the full tier's
    let conf = resolve(&root, &runtime, &[], "target/rt/conf/MoquiDevConf.xml").unwrap();
    assert!(hidden_below_full(&conf.path, &runtime));
    // the runtime's denied directories stay denied
    let err = resolve(&root, &runtime, &[], "target/rt/db/x").unwrap_err().to_string();
    assert!(err.contains("does not open"), "{err}");
}

/// A test run's runtime is a scratch one elsewhere, its components linked: the loaded component is not build output.
#[cfg(unix)]
#[test]
fn a_component_loaded_from_under_a_root_s_target_is_not_build_output() {
    let UnderTarget {
        _scratch,
        root,
        run,
        component,
        ..
    } = runtime_under_target("component-in-target");
    let err = resolve(&root, &run, &[], SCREEN).unwrap_err().to_string();
    assert!(err.contains("is a build output"), "{err}");
    assert_eq!(resolve(&root, &run, &[&component], SCREEN).unwrap().real, SCREEN);
    // the runtime it was copied into is not loaded, so its other files stay build output
    for path in [
        "target/rt/conf/MoquiDevConf.xml",
        "target/rt/components/dev/other/x.xml",
    ] {
        let err = resolve(&root, &run, &[&component], path).unwrap_err().to_string();
        assert!(err.contains("is a build output"), "{path}: {err}");
    }
}

#[test]
fn build_output_beside_or_inside_that_runtime_stays_refused() {
    let UnderTarget {
        _scratch,
        root,
        runtime,
        component,
        ..
    } = runtime_under_target("target-beside");
    for path in [
        "target",
        "target/debug/moqui",
        "target/debug/new",
        "target/rt/target/debug/x",
        "target/rt/components/dev/dev-aide/target/debug/x",
    ] {
        let err = resolve(&root, &runtime, &[&component], path).unwrap_err().to_string();
        assert!(err.contains("is a build output"), "{path}: {err}");
    }
    let err = resolve(&root, &runtime, &[], "target/rt/../debug/moqui")
        .unwrap_err()
        .to_string();
    assert!(err.contains("has .. in it"), "{err}");
    // a runtime or a component that is the root's target itself exempts nothing
    let target = root.join("target");
    let err = resolve(&root, &target, &[&target], "target/debug/moqui")
        .unwrap_err()
        .to_string();
    assert!(err.contains("is a build output"), "{err}");
    // nor is a runtime root's own target exempt
    let runtime_root = runtime.canonicalize().unwrap();
    std::fs::create_dir_all(runtime_root.join("target/debug")).unwrap();
    let err = resolve(&runtime_root, &runtime, &[], "target/debug/x")
        .unwrap_err()
        .to_string();
    assert!(err.contains("is a build output"), "{err}");
}

#[cfg(unix)]
#[test]
fn a_symlink_from_that_runtime_into_build_output_is_refused() {
    let UnderTarget {
        _scratch,
        root,
        runtime,
        component,
        ..
    } = runtime_under_target("target-symlink");
    std::os::unix::fs::symlink(root.join("target/debug"), runtime.join("components/dev/dev-aide/out")).unwrap();
    for path in [
        "target/rt/components/dev/dev-aide/out/moqui",
        "target/rt/components/dev/dev-aide/out/new",
    ] {
        let err = resolve(&root, &runtime, &[&component], path).unwrap_err().to_string();
        assert!(err.contains("is a build output"), "{path}: {err}");
    }
}
