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

//! The html mode: the markup each element writes. A fixture screen in `script/testdata/screen/html/render/<name>.xml`
//! is rendered and compared with `<name>.html` beside it. Set `UPDATE_GOLDEN=1` to rewrite the expected files, then
//! read the diff.

use std::collections::BTreeMap;
use std::path::PathBuf;

use moqui_lib::object::Value;

use crate::context::ExecutionContextFactory;
use crate::screen::make_render;

fn render_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/script/testdata/screen/html/render")
}

fn render_path(name: &str, path: &str, parameters: &[(&str, &str)]) -> String {
    let context = ExecutionContextFactory::test_context();
    let map: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::string(*value)))
        .collect();
    let output = make_render(context.factory())
        .root_screen(render_dir().join(format!("{name}.xml")).to_string_lossy().into_owned())
        .screen_path(path)
        .render_mode("html")
        .parameters(map)
        .render()
        .unwrap_or_else(|err| panic!("{name}: {err}"));
    assert_eq!(output.content_type(), Some("text/html;charset=UTF-8"));
    // The session token differs for each render.
    let text = output.body_text();
    let marker = "name=\"moquiSessionToken\" value=\"";
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find(marker) {
        let end = at + marker.len();
        out.push_str(&rest[..end]);
        out.push_str("TOKEN");
        rest = &rest[end..];
        rest = &rest[rest.find('"').unwrap_or(0)..];
    }
    out.push_str(rest);
    out
}

fn check(name: &str, parameters: &[(&str, &str)]) {
    check_path(name, "", name, parameters);
}

fn check_path(name: &str, screen_path: &str, golden: &str, parameters: &[(&str, &str)]) {
    let actual = render_path(name, screen_path, parameters);
    let path = render_dir().join(format!("{golden}.html"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {}", path.display()));
    assert_eq!(actual, expected, "{name} differs from {}", path.display());
}

#[test]
fn containers_links_and_labels() {
    check("containers", &[]);
}

#[test]
fn form_single_with_every_field_type() {
    check("fields", &[("_openDialog", "none")]);
}

#[test]
fn form_lists() {
    check("lists", &[]);
}

/// A rich text-area is the cleaned note, not a textarea of the tags. `editor-type="html"` stays a textarea
/// (`html-fields`).
#[test]
fn a_rich_text_area_is_the_note() {
    let html = render_path("rich-text", "", &[]);
    assert!(html.contains("m-rich-text-content"), "{html}");
    assert!(html.contains("<strong>there</strong>"), "{html}");
    assert!(html.contains("data-rich-text"), "{html}");
    assert!(!html.contains("<textarea"), "{html}");
    assert!(!html.contains("<script"), "{html}");
}

#[test]
fn subscreen_menus_tabs_and_panels() {
    check_path("Menu", "", "menu-one", &[]);
    check_path("Menu", "/Two", "menu-two", &[]);
    check_path("Panel", "", "panel-one", &[]);
    check_path("Panel", "/Two", "panel-two", &[("lastStandalone", "0")]);
}
