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

//! Golden tests for the screen macro modules: a fixture XML and the JavaScript each mode generates
//! for it. The fixtures are in `script/testdata/screen/<name>.xml` and the expected code for a mode
//! is in `script/testdata/screen/<mode>/<name>.js`. Set `UPDATE_GOLDEN=1` to rewrite the expected files, then
//! read the diff.

use std::path::PathBuf;

use crate::screen::generate::generate_xml;

const FIXTURES: &[&str] = &[
    "structure",
    "render-mode",
    "links-labels",
    "field-widgets",
    "form-list",
    "ignored",
    "screen-root",
    "fail-widgets",
];

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/script/testdata/screen")
}

/// The generated unit parses, and declares no global: the names it declares start with `$`, so a screen field cannot be hidden.
/// This runs on what each golden test generated, so it covers every fixture of every mode without generating them again.
fn parses_and_declares_no_global(mode: &str, name: &str, body: &str) {
    for line in body.lines() {
        let trimmed = line.trim_start();
        for keyword in ["let ", "const ", "var "] {
            if let Some(rest) = trimmed.strip_prefix(keyword) {
                assert!(rest.starts_with('$'), "{mode}/{name}: {line}");
            }
        }
    }
    let source = format!("(function () {{\n{body}}})");
    crate::script::parse_js(&source).unwrap_or_else(|err| panic!("{mode}/{name}: {err}"));
}

fn check(mode: &str, name: &str, debug: bool) {
    let xml = std::fs::read_to_string(testdata().join(format!("{name}.xml"))).unwrap();
    let actual = generate_xml(mode, &xml, debug).unwrap_or_else(|err| panic!("{mode}/{name}: {err}"));
    // Generating twice gives the same text.
    assert_eq!(
        actual,
        generate_xml(mode, &xml, debug).unwrap(),
        "{mode}/{name} is not repeatable"
    );
    if !debug {
        parses_and_declares_no_global(mode, name, &actual);
    }
    let suffix = if debug { "-debug" } else { "" };
    let path = testdata().join(mode).join(format!("{name}{suffix}.js"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {}", path.display()));
    assert_eq!(actual, expected, "{mode}/{name} differs from {}", path.display());
}

#[test]
fn golden_text() {
    for name in FIXTURES {
        check("text", name, false);
    }
}

#[test]
fn golden_csv() {
    for name in FIXTURES {
        check("csv", name, false);
    }
}

#[test]
fn golden_xml() {
    for name in FIXTURES {
        check("xml", name, false);
    }
}

/// Fixtures only the print mode has, for the elements it lays out itself.
const PRINT_FIXTURES: &[&str] = &["print-layout"];

#[test]
fn golden_html_print() {
    for name in FIXTURES.iter().chain(PRINT_FIXTURES) {
        check("html-print", name, false);
    }
}

/// Fixtures only the html mode has, for the elements it writes itself.
const HTML_FIXTURES: &[&str] = &["html-containers", "html-fields", "html-form-list"];

#[test]
fn golden_html() {
    for name in FIXTURES.iter().chain(HTML_FIXTURES) {
        check("html", name, false);
    }
}

/// Fixtures only the qvt mode has: row selection, a form that posts every row, the find and column dialogs, and the rest of the widgets.
const QVT_FIXTURES: &[&str] = &["qvt-extras"];

#[test]
fn golden_qvt() {
    for name in FIXTURES.iter().chain(HTML_FIXTURES).chain(QVT_FIXTURES) {
        check("qvt", name, false);
    }
}

#[test]
fn golden_qvt_static() {
    for name in FIXTURES.iter().chain(HTML_FIXTURES).chain(QVT_FIXTURES) {
        check("qvt-static", name, false);
    }
}

/// A dialog body, a depends-on drop-down, a `text type="tui"` that is not JSON, and the dynamic widgets, row forms, typed
/// columns, row actions, status bar, tree, button menu and wizard of Phase 3.
const TUI_FIXTURES: &[&str] = &["tui-dialog", "tui-depends", "tui-text", "tui-dynamics"];

#[test]
fn golden_tui() {
    for name in FIXTURES.iter().chain(TUI_FIXTURES) {
        check("tui", name, false);
    }
}

#[test]
fn golden_qjs_and_qvue() {
    for mode in ["qjs", "qvue"] {
        for name in FIXTURES {
            check(mode, name, false);
        }
    }
}

#[test]
fn debug_comments_name_each_element() {
    check("text", "links-labels", true);
}
