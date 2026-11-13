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

use super::{compare, entries, first_differing_line, unified_diff, write_skeleton, Difference, Kind, Set};

/// An empty directory of its own for one test, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("moqui-embedded-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn repo_base() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component")
}

/// The component directories under `base`, as [`compare`] takes them.
fn components(base: &Path) -> Vec<(Set, PathBuf)> {
    Set::COMPONENTS
        .into_iter()
        .map(|set| (set, base.join(set.name())))
        .collect()
}

fn compare_all(base: &Path) -> Vec<Difference> {
    let dirs = components(base);
    let dirs: Vec<(Set, &Path)> = dirs.iter().map(|(set, dir)| (*set, dir.as_path())).collect();
    compare(&dirs).unwrap()
}

#[test]
fn the_sets_hold_framework_and_tools_without_their_tests_and_the_webroot_stub() {
    let entries = entries().unwrap();
    let has = |set: Set, path: &str| entries.iter().any(|entry| entry.set == set && entry.path == path);
    assert!(has(Set::Framework, "component.xml"));
    assert!(has(Set::Framework, "conf/MoquiDefaultConf.xml"));
    assert!(has(Set::Framework, "data/MoquiSetupData.xml"));
    assert!(has(Set::Tools, "component.xml"));
    assert!(has(Set::Tools, "MoquiConf.xml"));
    assert!(has(Set::Tools, "service/moqui.rest.xml"));
    assert!(has(Set::Webroot, "screen/webroot/rest.xml"));
    assert!(has(Set::Webroot, "screen/webroot/Login.xml"));
    assert!(has(Set::Webroot, "screen/webroot/apps.xml"));
    assert!(has(
        Set::Webroot,
        "screen/webroot/libs/tom-select/tom-select.complete.min.js"
    ));
    assert!(has(Set::Skeleton, "webroot/screen/webroot/qapps.xml"));
    assert!(!has(Set::Webroot, "screen/webroot/qapps.xml"));
    assert!(!entries.iter().any(|entry| entry.set == Set::Webroot
        && (entry.path.contains("/libs/fontawesome/")
            || entry.path.contains("/libs/material-symbols/")
            || entry.path.contains("/libs/vue/")
            || entry.path.contains("/images/"))));
    assert!(!entries.iter().any(|entry| entry.set.is_whole_component()
        && (entry.path.starts_with("test/") || entry.path.starts_with("acceptance/"))));
}

#[test]
fn the_repository_runtime_matches_what_was_packed() {
    assert_eq!(compare_all(&repo_base()), Vec::<Difference>::new());
}

#[test]
fn a_skeleton_compares_clean_and_each_kind_of_change_is_reported() {
    let scratch = Scratch::new("kinds");
    let written = write_skeleton(&scratch.0).unwrap();
    let base = scratch.0.join("base-component");
    let framework = base.join("framework");
    let tools = base.join("tools");
    let webroot = base.join("webroot");
    assert!(written.contains(&framework.join("component.xml")));
    assert!(written.contains(&tools.join("component.xml")));
    assert!(
        webroot.join("screen/webroot/qapps.xml").is_file(),
        "the placeholder page is written"
    );
    assert_eq!(compare_all(&base), Vec::<Difference>::new());

    let service = framework.join("conf/MoquiDefaultConf.xml");
    let text = std::fs::read_to_string(&service).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    lines[2] = "<!-- edited -->";
    std::fs::write(&service, lines.join("\n") + "\n").unwrap();
    std::fs::remove_file(framework.join("data/CurrencyData.xml")).unwrap();
    std::fs::write(framework.join("entity/Extra.xml"), "<entities/>").unwrap();
    std::fs::write(framework.join("template/XmlActions.js"), "// edited").unwrap();
    std::fs::write(webroot.join("screen/webroot/rest.xml"), "<screen/>").unwrap();
    std::fs::write(webroot.join("screen/webroot/Other.xml"), "<screen/>").unwrap();
    std::fs::write(tools.join("screen/Extra.xml"), "<screen/>").unwrap();

    let differences = compare_all(&base);
    let kind_of = |set: Set, path: &str| {
        differences
            .iter()
            .find(|difference| difference.set == set && difference.path == path)
            .map(|difference| difference.kind.clone())
    };
    assert_eq!(
        kind_of(Set::Framework, "conf/MoquiDefaultConf.xml"),
        Some(Kind::Changed { first_line: 3 })
    );
    assert_eq!(kind_of(Set::Framework, "data/CurrencyData.xml"), Some(Kind::Missing));
    assert_eq!(kind_of(Set::Framework, "entity/Extra.xml"), Some(Kind::Added));
    assert_eq!(kind_of(Set::Tools, "screen/Extra.xml"), Some(Kind::Added));
    assert_eq!(
        kind_of(Set::Webroot, "screen/webroot/rest.xml"),
        Some(Kind::Changed { first_line: 1 })
    );
    assert_eq!(
        kind_of(Set::Webroot, "screen/webroot/Other.xml"),
        None,
        "the webroot stub is a subset"
    );
    assert_eq!(
        kind_of(Set::Webroot, "screen/webroot/qapps.xml"),
        None,
        "the placeholder is not compared"
    );
    let compiled = differences
        .iter()
        .find(|difference| difference.path == "template/XmlActions.js")
        .unwrap();
    assert!(compiled.is_compiled_in());
    assert!(compiled
        .to_string()
        .ends_with("(ignored: the binary uses its built-in copy)"));
    assert_eq!(differences.len(), 6, "{differences:#?}");

    let diff = unified_diff(Set::Framework, "conf/MoquiDefaultConf.xml", &framework)
        .unwrap()
        .unwrap();
    assert!(diff.contains("+<!-- edited -->"), "{diff}");
    assert!(
        diff.starts_with("--- framework/conf/MoquiDefaultConf.xml (built in)"),
        "{diff}"
    );
    assert_eq!(unified_diff(Set::Framework, "component.xml", &framework).unwrap(), None);
}

#[test]
fn a_skeleton_keeps_existing_files_and_an_existing_webroot_or_tools() {
    let scratch = Scratch::new("keep");
    let framework = scratch.0.join("base-component/framework");
    std::fs::create_dir_all(framework.join("conf")).unwrap();
    std::fs::write(framework.join("conf/MoquiDefaultConf.xml"), "mine").unwrap();
    std::fs::create_dir_all(scratch.0.join("base-component/webroot")).unwrap();
    std::fs::create_dir_all(scratch.0.join("base-component/tools")).unwrap();
    let written = write_skeleton(&scratch.0).unwrap();
    assert_eq!(
        std::fs::read_to_string(framework.join("conf/MoquiDefaultConf.xml")).unwrap(),
        "mine"
    );
    assert!(framework.join("component.xml").is_file());
    assert!(
        written.iter().all(|path| path.starts_with(&framework)),
        "nothing written into a webroot or tools that is there"
    );
}

#[test]
fn the_first_differing_line_counts_from_one() {
    assert_eq!(first_differing_line(b"a\nb\nc", b"a\nb\nd"), 3);
    assert_eq!(first_differing_line(b"a", b"b"), 1);
    assert_eq!(first_differing_line(b"a\n", b"a\nb"), 2);
}
