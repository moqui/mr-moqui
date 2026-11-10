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

fn components(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("moqui-load-scripts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (tools, marble) = (root.join("tools"), root.join("marble"));
    std::fs::create_dir_all(tools.join("test/load")).unwrap();
    std::fs::create_dir_all(marble.join("test/load")).unwrap();
    std::fs::create_dir_all(marble.join("test/client")).unwrap();
    for file in ["ServiceCall.load.js", "Apple.load.js", "notes.txt", ".hidden.load.js"] {
        std::fs::write(tools.join("test/load").join(file), "").unwrap();
    }
    std::fs::write(marble.join("test/load/OrderToCash.load.js"), "").unwrap();
    std::fs::write(marble.join("test/client/Walk.load.js"), "").unwrap();
    (tools, marble)
}

#[test]
fn scripts_are_the_load_js_files_of_test_load() {
    let (tools, marble) = components("list");
    let scripts = scripts_in([
        ("tools", tools.as_path()),
        ("mr-marble", marble.as_path()),
        ("none", Path::new("/no")),
    ]);
    let locations: Vec<&str> = scripts.iter().map(|script| script.location.as_str()).collect();
    assert_eq!(
        locations,
        [
            "component://tools/test/load/Apple.load.js",
            "component://tools/test/load/ServiceCall.load.js",
            "component://mr-marble/test/load/OrderToCash.load.js",
        ]
    );
    assert_eq!(scripts[1].name, "ServiceCall");
    assert_eq!(scripts[2].component, "mr-marble");
    std::fs::remove_dir_all(tools.parent().unwrap()).unwrap();
}

#[test]
fn only_a_load_script_of_a_loaded_component_resolves() {
    let (tools, marble) = components("resolve");
    let loaded = [("tools", tools.as_path()), ("mr-marble", marble.as_path())];
    let script = resolve("component://tools/test/load/ServiceCall.load.js", loaded).unwrap();
    assert_eq!(script.path, tools.join("test/load/ServiceCall.load.js"));
    assert_eq!(script.name, "ServiceCall");

    for location in [
        "component://mr-marble/test/client/Walk.load.js",
        "component://tools/test/load/../load/ServiceCall.load.js",
        "component://tools/test/load/sub/ServiceCall.load.js",
        "component://tools/test/load/notes.txt",
        "component://tools/test/load/.hidden.load.js",
        "component://other/test/load/ServiceCall.load.js",
        "file:///etc/passwd",
        "/abs/test/load/ServiceCall.load.js",
        "",
    ] {
        let err = resolve(location, loaded).unwrap_err();
        assert!(err.contains("not a load script"), "{location}: {err}");
    }
    let err = resolve("component://tools/test/load/Missing.load.js", loaded).unwrap_err();
    assert!(err.contains("no such load script"), "{err}");
    std::fs::remove_dir_all(tools.parent().unwrap()).unwrap();
}
