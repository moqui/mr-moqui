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

use moqui_core::context::ComponentInfo;

use super::{entity_files, file_of, reload_files, xml_problems};

/// A component's files in a scratch directory, removed when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str, files: &[(&str, &str)]) -> Scratch {
    let dir = std::env::temp_dir().join(format!("moqui-deploy-load-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    Scratch(dir)
}

const FILES: &[(&str, &str)] = &[
    ("component.xml", "<component name=\"c\"/>"),
    ("MoquiConf.xml", "<moqui-conf/>"),
    ("Cargo.toml", "[package]"),
    ("src/lib.rs", ""),
    ("entity/CEntities.xml", "<entities/>"),
    ("entity/C.eecas.xml", "<eecas/>"),
    ("service/c/CServices.xml", "<services/>"),
    ("screen/C.xml", "<screen/>"),
    ("data/CSeedData.xml", "<entity-facade-xml/>"),
    ("test/C.spec.js", ""),
    (".git/HEAD", "ref: refs/heads/main"),
];

/// The reload pipeline takes a component's services, screens and entity ECA rules; not its conf, `component.xml`,
/// entity definitions (relinked first), data (loaded last), Rust code, tests or git directory.
#[test]
fn the_reload_takes_the_artifacts_the_load_does_not_apply_itself() {
    let dir = scratch("reload", FILES);
    let relative: Vec<String> = reload_files(&dir.0)
        .iter()
        .map(|path| path.strip_prefix(&dir.0).unwrap().to_string_lossy().replace('\\', "/"))
        .collect();
    assert_eq!(
        relative,
        ["entity/C.eecas.xml", "screen/C.xml", "service/c/CServices.xml"]
    );
    let entities: Vec<PathBuf> = entity_files(&dir.0);
    assert_eq!(entities, [dir.0.join("entity/CEntities.xml")]);
}

/// A file that does not parse is named; data files and tests are not parsed here (the load reports each data file).
#[test]
fn an_xml_file_that_does_not_parse_is_a_refusal() {
    let dir = scratch(
        "parse",
        &[
            ("component.xml", "<component name=\"c\"/>"),
            ("service/Broken.xml", "<services><service"),
            ("data/Big.xml", "<entity-facade-xml"),
        ],
    );
    let problems = xml_problems(&dir.0);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("Broken.xml"), "{problems:?}");
}

#[test]
fn a_component_location_names_its_file_before_it_is_registered() {
    let component = ComponentInfo {
        name: "c".to_string(),
        version: String::new(),
        location: PathBuf::from("/srv/runtime/components/addon/c"),
        depends_on: Vec::new(),
        purposes: Vec::new(),
    };
    assert_eq!(
        file_of(&component, "component://c/data/CSeedData.xml"),
        PathBuf::from("/srv/runtime/components/addon/c/data/CSeedData.xml")
    );
}
