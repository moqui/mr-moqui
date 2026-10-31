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

use std::path::{Path, PathBuf};

use super::{detect, detect_java, requested_dir, Layout};

/// A fresh directory with `files` (empty, their parents made) and `dirs`.
fn tree(name: &str, files: &[&str], dirs: &[&str]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("moqui-project-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for file in files {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
    }
    for dir in dirs {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::canonicalize(root).unwrap()
}

fn clean(root: &Path) {
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_java_checkout_with_groovy_only() {
    let root = tree(
        "groovy",
        &["framework/src/main/groovy/org/moqui/Moqui.groovy"],
        &["runtime/component"],
    );
    let project = detect(&root).unwrap();
    assert_eq!(project.layout, Layout::Java);
    assert_eq!(project.root, root);
    assert_eq!(project.runtime, root.join("runtime"));
    // Its runtime directory names the same checkout.
    assert_eq!(detect(&root.join("runtime")).unwrap().root, root);
    clean(&root);
}

#[test]
fn a_java_checkout_with_java_only() {
    let root = tree("java", &["framework/src/main/java/org/moqui/Moqui.java"], &["runtime"]);
    assert_eq!(detect(&root).unwrap().layout, Layout::Java);
    clean(&root);
}

#[test]
fn a_java_checkout_by_its_gradle_build_and_sources() {
    let root = tree(
        "gradle",
        &["framework/build.gradle", "framework/src/org/moqui/impl/Tool.groovy"],
        &["runtime"],
    );
    assert_eq!(detect(&root).unwrap().layout, Layout::Java);
    // A build.gradle with no sources is no Java framework.
    let bare = tree(
        "gradle-bare",
        &["framework/build.gradle"],
        &["runtime", "framework/src"],
    );
    assert!(detect(&bare).is_err());
    clean(&root);
    clean(&bare);
}

#[test]
fn a_java_framework_needs_its_runtime() {
    let root = tree("no-runtime", &["framework/src/main/groovy/A.groovy"], &[]);
    let err = detect(&root).unwrap_err().to_string();
    assert!(err.contains("no runtime/"), "{err}");
    clean(&root);
}

#[test]
fn an_mr_clone() {
    let root = tree(
        "mr-clone",
        &["runtime/base-component/framework/component.xml"],
        &["crates/moqui"],
    );
    let project = detect(&root).unwrap();
    assert_eq!(project.layout, Layout::Mr);
    assert_eq!(project.root, root);
    assert_eq!(project.runtime, root.join("runtime"));
    clean(&root);
}

#[test]
fn an_mr_runtime_dir() {
    let root = tree(
        "mr-runtime",
        &["base-component/framework/component.xml"],
        &["component"],
    );
    let project = detect(&root).unwrap();
    assert_eq!(project.layout, Layout::Mr);
    assert_eq!(project.runtime, root);
    clean(&root);
}

#[test]
fn neither_is_a_clear_error() {
    let root = tree("neither", &["README.md"], &["src"]);
    let err = detect(&root).unwrap_err().to_string();
    assert!(err.contains("is not a Moqui project"), "{err}");
    assert!(err.contains("framework/src/main/groovy"), "{err}");
    assert!(err.contains("base-component/framework"), "{err}");
    // crates/ without a runtime framework says what it lacks.
    let crates = tree("crates-only", &[], &["crates/moqui"]);
    let err = detect(&crates).unwrap_err().to_string();
    assert!(err.contains("runtime/base-component/framework"), "{err}");
    clean(&root);
    clean(&crates);
}

#[test]
fn forced_java_takes_any_framework_but_not_an_mr_clone() {
    let root = tree("forced", &["framework/build.gradle"], &["runtime"]);
    assert_eq!(detect_java(&root).unwrap().layout, Layout::Java);
    let clone = tree(
        "forced-mr",
        &["runtime/base-component/framework/component.xml"],
        &["framework"],
    );
    assert!(detect_java(&clone).is_err());
    clean(&root);
    clean(&clone);
}

#[test]
fn a_requested_dir_is_absolute_and_plain() {
    assert!(requested_dir("runtime").is_err());
    assert!(requested_dir("").is_err());
    assert!(requested_dir("/tmp/../etc").is_err());
    assert!(requested_dir("/tmp/./x").is_err());
    assert!(requested_dir("/no/such/moqui/dir").is_err());
    let root = tree("requested", &["a"], &[]);
    assert_eq!(requested_dir(&root.display().to_string()).unwrap(), root);
    assert!(requested_dir(&root.join("a").display().to_string()).is_err());
    clean(&root);
}
