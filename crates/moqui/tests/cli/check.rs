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

//! `moqui check` and `moqui refs`.

use super::{moqui, repo};

fn scratch(name: &str, text: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-cli-check-{}", std::process::id()));
    let path = dir.join("service/clitest").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn a_reference_error_fails_with_its_rule_and_skill_section() {
    let path = scratch(
        "Bad.xml",
        r#"<services>
    <service verb="run" noun="Thing">
        <actions><entity-find entity-name="no.such.Entity" list="x"/></actions>
    </service>
</services>"#,
    );
    let output = moqui(&["check", "--json", path.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let problem = stdout.lines().next().unwrap();
    for part in [
        r#""line":3,"#,
        r#""severity":"error""#,
        r#""rule":"reference.entity""#,
        r#""skill":"framework/skill/moqui-coding-agent.md#References""#,
    ] {
        assert!(problem.contains(part), "{part} not in {problem}");
    }
}

#[test]
fn a_clean_file_passes_and_warnings_do_not_fail() {
    let path = scratch(
        "Clean.xml",
        r#"<services>
    <service verb="run" noun="Thing" not-an-attribute="x">
        <actions><entity-find entity-name="moqui.basic.Enumeration" list="enums"/></actions>
    </service>
</services>"#,
    );
    let output = moqui(&["check", path.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains(":2:") && stdout.contains("warning:"), "{stdout}");
    assert!(
        stdout.contains("[schema.unknown-attribute, framework/skill/moqui-coding-agent.md#Checks]"),
        "{stdout}"
    );
}

#[test]
fn refs_lists_what_uses_an_entity() {
    // --kind keeps the references whose other side, here the screen that finds the entity, is of that kind
    let output = moqui(&["refs", "moqui.example.Example", "--kind", "screen"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        stdout.contains("runtime/dev-component/example/screen/Gallery/ExampleDetail.xml:"),
        "{stdout}"
    );
    let _ = repo();
}
