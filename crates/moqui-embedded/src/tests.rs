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

use super::select::{gather, is_excluded, pack, Level};
use super::{
    children, compare, entries, file, first_differing_line, is_dir, is_file, unified_diff, write_runtime, Archive,
    Difference, Kind, Set, LEVEL,
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

/// Write every built-in file into the runtime `dir`, with its framework in the base set.
fn write_all(dir: &Path) -> Vec<PathBuf> {
    write_runtime(dir, &dir.join("components/base/framework")).unwrap()
}

/// The component set directories of the runtime `runtime`, as [`compare`] takes them.
fn component_dirs(runtime: &Path) -> Vec<(Set, PathBuf)> {
    let framework = runtime.join("components/base/framework");
    Set::COMPONENTS
        .into_iter()
        .map(|set| (set, set.dir_in(runtime, &framework)))
        .collect()
}

fn borrowed(dirs: &[(Set, PathBuf)]) -> Vec<(Set, &Path)> {
    dirs.iter().map(|(set, dir)| (*set, dir.as_path())).collect()
}

fn compare_all(runtime: &Path) -> Vec<Difference> {
    compare(&borrowed(&component_dirs(runtime))).unwrap()
}

#[test]
fn the_sets_hold_framework_and_tools_without_their_tests_and_the_webroot_stub() {
    if LEVEL != Level::Framework {
        return;
    }
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
    assert!(!entries
        .iter()
        .any(|entry| entry.set.is_whole_component() && entry.path.starts_with("test/")));
}

#[test]
fn the_repository_runtime_matches_what_was_packed() {
    let differences = compare_all(&repo_runtime());
    // a component cloned into the runtime after the build is not built in; nothing else differs
    let differences: Vec<_> = differences
        .into_iter()
        .filter(|difference| difference.kind != Kind::NotBuiltIn)
        .collect();
    assert_eq!(differences, Vec::<Difference>::new());
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
    if LEVEL == Level::Framework {
        let skeleton = children(Set::Skeleton, "components/base/webroot/screen");
        assert_eq!(skeleton, vec![("webroot".to_string(), true)]);
    }
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
    assert_eq!(compare_all(&scratch.0), Vec::<Difference>::new());

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

    let differences = compare_all(&scratch.0);
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
    if LEVEL == Level::Framework {
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
    }
    let compiled = differences
        .iter()
        .find(|difference| difference.path == "template/XmlActions.js")
        .unwrap();
    assert!(compiled.is_compiled_in());
    assert!(compiled
        .to_string()
        .ends_with("(ignored: the binary uses its built-in copy)"));
    let expected = if LEVEL == Level::Framework { 6 } else { 7 };
    assert_eq!(differences.len(), expected, "{differences:#?}");

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

#[test]
fn the_archive_excludes_are_left_out_at_any_depth_and_target_at_the_top() {
    for name in [
        ".git",
        ".gitignore",
        ".venv",
        "__pycache__",
        ".DS_Store",
        "notes~",
        "postmaster.pid",
    ] {
        assert!(is_excluded(name, false), "{name}");
        assert!(is_excluded(name, true), "{name}");
    }
    assert!(is_excluded("target", true));
    assert!(!is_excluded("target", false), "a target/ deeper in a component is kept");
    for name in ["component.xml", "screen", "test", "plan", "data~old"] {
        assert!(!is_excluded(name, true), "{name}");
    }
}

/// Write `text` at `path`, making its directory.
fn put(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A small runtime with a component of each kind, and the crate files `gather` reads (a one-line webroot list and a
/// skeleton with the placeholder page).
fn small_runtime(root: &Path) -> (PathBuf, PathBuf) {
    let runtime = root.join("runtime");
    let crate_dir = root.join("crate");
    put(&crate_dir.join("webroot-files.txt"), "# the stub\ncomponent.xml\n");
    put(
        &crate_dir.join("skeleton/components/base/webroot/screen/webroot/qapps.xml"),
        "<screen placeholder/>",
    );
    let components = runtime.join("components");
    put(&components.join("components.xml"), "<components/>");
    for (component, extra) in [
        ("base/framework", "entity/A.xml"),
        ("base/tools", "screen/Tools.xml"),
        ("base/webroot", "screen/webroot/qapps.xml"),
        ("base/basic-comms", "service/Comms.xml"),
        ("addon/shop", "screen/Shop.xml"),
        ("dev/example", "screen/Example.xml"),
        ("test/fixture", "data/Fixture.xml"),
    ] {
        let dir = components.join(component);
        put(
            &dir.join("component.xml"),
            &format!("<component name=\"{component}\"/>"),
        );
        put(&dir.join(extra), "<x/>");
        put(&dir.join("test/Spec.js"), "// test");
        put(&dir.join("plan/Plan.md"), "# plan");
        put(&dir.join(".git/HEAD"), "ref");
        put(&dir.join("target/debug/out"), "build");
        put(&dir.join("src/target/kept.rs"), "// kept");
        put(&dir.join("data/old.xml~"), "backup");
    }
    put(&components.join("addon/README.md"), "# add-ons");
    put(
        &components.join("base/webroot").join(super::select::VAULT_MODULE),
        "wasm",
    );
    put(&runtime.join("conf/MoquiDevConf.xml"), "<moqui-conf/>");
    (runtime, crate_dir)
}

#[test]
fn the_whole_webroot_is_packed_only_with_the_vault_module_built() {
    let scratch = Scratch::new("vault-module");
    let (runtime, crate_dir) = small_runtime(&scratch.0);
    let module = format!("webroot/{}", super::select::VAULT_MODULE);
    assert!(gathered(&runtime, &crate_dir, Level::Production).contains(&module));
    assert!(!gathered(&runtime, &crate_dir, Level::Framework).contains(&module));

    std::fs::remove_file(
        runtime
            .join("components/base/webroot")
            .join(super::select::VAULT_MODULE),
    )
    .unwrap();
    for level in [Level::Production, Level::All] {
        let err = gather(&runtime, &crate_dir, level, &mut |_| {}).unwrap_err();
        assert!(
            err.to_string()
                .contains("moqui_vault.wasm is not built: run scripts/vault-wasm.sh"),
            "{err}"
        );
    }
    assert!(
        gather(&runtime, &crate_dir, Level::Framework, &mut |_| {}).is_ok(),
        "the stub has no module"
    );
}

/// The names `gather` takes from `runtime` at `level`.
fn gathered(runtime: &Path, crate_dir: &Path, level: Level) -> Vec<String> {
    gather(runtime, crate_dir, level, &mut |_| {})
        .unwrap()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// An archive of `runtime` packed at `level`, as build.rs packs one, kept for the rest of the test run.
fn packed_at(runtime: &Path, crate_dir: &Path, level: Level) -> Archive {
    let files = gather(runtime, crate_dir, level, &mut |_| {}).unwrap();
    let (blob, index) = pack(&files).unwrap();
    Archive::new(
        Box::leak(blob.into_boxed_slice()),
        Box::leak(index.into_boxed_str()),
        level,
    )
}

#[test]
fn each_level_packs_its_components_less_tests_plans_and_the_archive_excludes() {
    let scratch = Scratch::new("levels");
    let (runtime, crate_dir) = small_runtime(&scratch.0);

    let framework = gathered(&runtime, &crate_dir, Level::Framework);
    assert!(framework.contains(&"framework/entity/A.xml".to_string()));
    assert!(
        framework.contains(&"framework/plan/Plan.md".to_string()),
        "{framework:?}"
    );
    assert!(framework.contains(&"framework/src/target/kept.rs".to_string()));
    assert!(framework.contains(&"webroot/component.xml".to_string()));
    assert!(
        !framework.contains(&"webroot/screen/webroot/qapps.xml".to_string()),
        "the stub only"
    );
    assert!(framework.contains(&super::select::PLACEHOLDER.to_string()));
    assert!(framework.contains(&"skeleton/components/components.xml".to_string()));
    assert!(framework.contains(&"conf/MoquiDevConf.xml".to_string()));
    assert!(
        !framework.iter().any(|name| name.starts_with("components/")),
        "{framework:?}"
    );
    for name in &framework {
        assert!(
            !name.contains("/test/") && !name.contains(".git") && !name.ends_with('~'),
            "{name}"
        );
        assert!(!name.starts_with("framework/target/"), "{name}");
    }

    let production = gathered(&runtime, &crate_dir, Level::Production);
    for name in [
        "webroot/screen/webroot/qapps.xml",
        "components/base/basic-comms/component.xml",
        "components/base/basic-comms/service/Comms.xml",
        "components/addon/shop/screen/Shop.xml",
        "components/addon/shop/src/target/kept.rs",
        "components/addon/README.md",
    ] {
        assert!(production.contains(&name.to_string()), "{name} in {production:?}");
    }
    assert!(!production.contains(&super::select::PLACEHOLDER.to_string()));
    for name in &production {
        assert!(
            !name.contains("/test/") && !name.contains("/plan/") && !name.contains(".git"),
            "{name}"
        );
        assert!(!name.contains("/shop/target/") && !name.ends_with('~'), "{name}");
        assert!(
            !name.starts_with("components/base/framework") && !name.starts_with("components/base/tools"),
            "{name}: the framework level's components are sets of their own"
        );
        assert!(
            !name.starts_with("components/dev/") && !name.starts_with("components/test/"),
            "{name}"
        );
    }

    let all = gathered(&runtime, &crate_dir, Level::All);
    for name in [
        "framework/test/Spec.js",
        "components/addon/shop/test/Spec.js",
        "components/addon/shop/plan/Plan.md",
        "components/dev/example/screen/Example.xml",
        "components/test/fixture/data/Fixture.xml",
    ] {
        assert!(all.contains(&name.to_string()), "{name} in {all:?}");
    }
    assert!(!all
        .iter()
        .any(|name| name.contains(".git") || name.contains("/shop/target/")));
}

#[test]
fn a_production_archive_reads_writes_and_compares_its_components() {
    let scratch = Scratch::new("production");
    let (source, crate_dir) = small_runtime(&scratch.0);
    let archive = packed_at(&source, &crate_dir, Level::Production);
    assert_eq!(archive.components(), vec!["addon/shop", "base/basic-comms"]);
    assert_eq!(
        archive.children(Set::Components, "addon"),
        vec![("README.md".to_string(), false), ("shop".to_string(), true)]
    );
    assert_eq!(
        archive.file(Set::Components, "addon/shop/screen/Shop.xml").unwrap(),
        Some(b"<x/>".to_vec())
    );

    let runtime = scratch.0.join("deployed");
    let framework = runtime.join("components/base/framework");
    let written = archive.write_runtime(&runtime, &framework).unwrap();
    assert!(written.contains(&runtime.join("components/addon/shop/component.xml")));
    assert!(runtime
        .join("components/base/webroot/screen/webroot/qapps.xml")
        .is_file());
    let dirs = component_dirs(&runtime);
    assert_eq!(archive.compare(&borrowed(&dirs)).unwrap(), Vec::<Difference>::new());

    let shop = runtime.join("components/addon/shop");
    std::fs::write(shop.join("screen/Shop.xml"), "<y/>").unwrap();
    std::fs::remove_file(runtime.join("components/base/basic-comms/service/Comms.xml")).unwrap();
    put(&shop.join("screen/New.xml"), "<new/>");
    put(&shop.join("test/Spec2.js"), "// not compared");
    put(&shop.join("target/out"), "// not compared");
    put(&runtime.join("components/addon/other/component.xml"), "<component/>");
    put(&runtime.join("components/dev/example/component.xml"), "<component/>");
    put(
        &runtime.join("components/base/webroot/screen/webroot/New.xml"),
        "<new/>",
    );

    let differences = archive.compare(&borrowed(&dirs)).unwrap();
    let found: Vec<(Set, &str, &Kind)> = differences
        .iter()
        .map(|difference| (difference.set, difference.path.as_str(), &difference.kind))
        .collect();
    assert_eq!(
        found,
        vec![
            (Set::Webroot, "screen/webroot/New.xml", &Kind::Added),
            (Set::Components, "addon/other", &Kind::NotBuiltIn),
            (Set::Components, "addon/shop/screen/New.xml", &Kind::Added),
            (
                Set::Components,
                "addon/shop/screen/Shop.xml",
                &Kind::Changed { first_line: 1 }
            ),
            (Set::Components, "base/basic-comms/service/Comms.xml", &Kind::Missing),
        ],
        "a whole webroot reports what is added; the dev set is not packed at production"
    );
    let diff = archive
        .unified_diff(
            Set::Components,
            "addon/shop/screen/Shop.xml",
            &runtime.join("components"),
        )
        .unwrap()
        .unwrap();
    assert!(diff.contains("+<y/>"), "{diff}");
}
