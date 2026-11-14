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

use super::{
    children, compare, entries, file, first_differing_line, is_dir, is_file, unified_diff, write_runtime, Difference,
    Kind, Set,
};

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

fn repo_runtime() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime")
}

fn repo_base() -> PathBuf {
    repo_runtime().join("components/base")
}

/// Write every built-in file into the runtime `dir`, with its framework in the base set.
fn write_all(dir: &Path) -> Vec<PathBuf> {
    write_runtime(dir, &dir.join("components/base/framework")).unwrap()
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
    assert!(has(Set::Skeleton, "components/base/webroot/screen/webroot/qapps.xml"));
    assert!(has(Set::Skeleton, "components/components.xml"));
    assert!(has(Set::Conf, "MoquiDevConf.xml"));
    assert!(has(Set::Conf, "MoquiProductionConf.xml"));
    assert!(has(Set::Conf, "edge/MoquiEdgeConf.xml"));
    assert!(!has(Set::Webroot, "screen/webroot/qapps.xml"));
    assert!(!entries.iter().any(|entry| entry.set == Set::Webroot
        && (entry.path.contains("/libs/fontawesome/")
            || entry.path.contains("/libs/material-symbols/")
            || entry.path.contains("/libs/vue/")
            || entry.path.contains("/images/"))));
    assert!(!entries.iter().any(|entry| entry.set.is_whole_component()
        && entry.path.starts_with("test/")));
}

#[test]
fn the_repository_runtime_matches_what_was_packed() {
    assert_eq!(compare_all(&repo_base()), Vec::<Difference>::new());
}

#[test]
fn one_file_and_a_directory_are_read_without_the_rest() {
    let packed = file(Set::Conf, "MoquiDevConf.xml").unwrap().unwrap();
    assert_eq!(
        packed,
        std::fs::read(repo_runtime().join("conf/MoquiDevConf.xml")).unwrap()
    );
    assert_eq!(file(Set::Conf, "NoSuchConf.xml").unwrap(), None);
    assert!(is_file(Set::Framework, "conf/MoquiDefaultConf.xml"));
    assert!(!is_file(Set::Framework, "conf"));
    assert!(is_dir(Set::Framework, "conf"));
    assert!(is_dir(Set::Framework, "conf/"));
    assert!(is_dir(Set::Framework, ""));
    assert!(!is_dir(Set::Framework, "conf/MoquiDefaultConf.xml"));
    assert!(!is_dir(Set::Framework, "test"));
    let top = children(Set::Framework, "");
    assert!(top.contains(&("component.xml".to_string(), false)), "{top:?}");
    assert!(top.contains(&("entity".to_string(), true)), "{top:?}");
    let skeleton = children(Set::Skeleton, "components/base/webroot/screen");
    assert_eq!(skeleton, vec![("webroot".to_string(), true)]);
    assert!(children(Set::Conf, "no-such-dir").is_empty());
}

#[test]
fn a_skeleton_compares_clean_and_each_kind_of_change_is_reported() {
    let scratch = Scratch::new("kinds");
    let written = write_all(&scratch.0);
    let base = scratch.0.join("components/base");
    let framework = base.join("framework");
    let tools = base.join("tools");
    let webroot = base.join("webroot");
    assert!(written.contains(&framework.join("component.xml")));
    assert!(written.contains(&tools.join("component.xml")));
    assert!(
        webroot.join("screen/webroot/qapps.xml").is_file(),
        "the placeholder page is written"
    );
    assert!(
        written.contains(&scratch.0.join("components/components.xml")),
        "the sets' manifest is written"
    );
    assert!(
        written.contains(&scratch.0.join("conf/MoquiDevConf.xml")),
        "the confs are written"
    );
    assert!(written.contains(&scratch.0.join("conf/edge/README.md")));
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
fn writing_the_runtime_keeps_the_files_there_and_fills_in_the_rest() {
    let scratch = Scratch::new("keep");
    let base = scratch.0.join("components/base");
    let framework = base.join("framework");
    std::fs::create_dir_all(framework.join("conf")).unwrap();
    std::fs::write(framework.join("conf/MoquiDefaultConf.xml"), "mine").unwrap();
    std::fs::create_dir_all(base.join("webroot/screen/webroot")).unwrap();
    std::fs::write(base.join("webroot/screen/webroot/qapps.xml"), "<screen/>").unwrap();
    std::fs::create_dir_all(base.join("tools")).unwrap();
    std::fs::create_dir_all(scratch.0.join("conf")).unwrap();
    std::fs::write(scratch.0.join("conf/MoquiDevConf.xml"), "mine").unwrap();
    let written = write_all(&scratch.0);
    assert_eq!(
        std::fs::read_to_string(framework.join("conf/MoquiDefaultConf.xml")).unwrap(),
        "mine"
    );
    assert_eq!(
        std::fs::read_to_string(base.join("webroot/screen/webroot/qapps.xml")).unwrap(),
        "<screen/>",
        "the placeholder does not replace the web client's screen"
    );
    assert_eq!(
        std::fs::read_to_string(scratch.0.join("conf/MoquiDevConf.xml")).unwrap(),
        "mine"
    );
    assert!(framework.join("component.xml").is_file());
    assert!(
        base.join("tools/component.xml").is_file(),
        "an empty tools is filled in"
    );
    assert!(base.join("webroot/screen/webroot/rest.xml").is_file());
    assert!(scratch.0.join("conf/MoquiProductionConf.xml").is_file());
    assert!(!written.contains(&framework.join("conf/MoquiDefaultConf.xml")));
    assert!(write_all(&scratch.0).is_empty(), "a second run writes nothing");
}

#[test]
fn a_framework_elsewhere_is_written_where_it_is() {
    let scratch = Scratch::new("elsewhere");
    let runtime = scratch.0.join("runtime");
    let framework = scratch.0.join("framework");
    write_runtime(&runtime, &framework).unwrap();
    assert!(framework.join("component.xml").is_file());
    assert!(!runtime.join("components/base/framework").exists());
    assert!(runtime.join("components/base/tools/component.xml").is_file());
}

#[test]
fn the_first_differing_line_counts_from_one() {
    assert_eq!(first_differing_line(b"a\nb\nc", b"a\nb\nd"), 3);
    assert_eq!(first_differing_line(b"a", b"b"), 1);
    assert_eq!(first_differing_line(b"a\n", b"a\nb"), 2);
}
