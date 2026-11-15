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

//! A `container-box` `condition`: evaluated as a section's (`sri.checkCondition`), so a false one renders nothing of
//! the box (header, toolbar, body) in each mode, a true one renders it, and one that fails is logged and its error
//! written in the box's place while the rest of the screen renders. In qvt-static it is a v-if over the box.

use crate::context::ExecutionContextFactory;
use crate::screen::{make_render, ScreenFacade};

const SCREEN: &str = r#"<screen require-authentication="false"><actions>
    <set field="shown" from="true"/><set field="hidden" from="false"/></actions><widgets>
    <container-box id="ShownBox" condition="shown"><box-header title="Shown title"><label text="shown head"/></box-header>
        <box-toolbar><label text="shown tool"/></box-toolbar><box-body><label text="shown body"/></box-body></container-box>
    <container-box id="HiddenBox" condition="hidden"><box-header title="Hidden title"><label text="hidden head"/></box-header>
        <box-toolbar><label text="hidden tool"/></box-toolbar><box-body><label text="hidden body"/></box-body></container-box>
    <container-box id="BrokenBox" condition="missingThing.field"><box-body><label text="broken body"/></box-body></container-box>
    <label text="after boxes"/>
</widgets></screen>"#;

fn screen_file(label: &str, xml: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("moqui-boxes-{}-{label}.xml", std::process::id()));
    std::fs::write(&path, xml).unwrap();
    path
}

#[test]
fn a_box_condition_shows_or_hides_the_whole_box_in_each_render_mode() {
    let path = screen_file("modes", SCREEN);
    let context = ExecutionContextFactory::test_context();
    for mode in ["qvt", "html", "text", "tui"] {
        let output = make_render(context.factory())
            .root_screen(path.to_string_lossy().into_owned())
            .render_mode(mode)
            .render()
            .unwrap_or_else(|err| panic!("{mode}: {err}"));
        let body = output.body_text();
        assert_eq!(output.status, 200, "{mode}: {body}");
        for shown in ["shown head", "shown tool", "shown body", "after boxes"] {
            // text writes no toolbar
            if mode == "text" && shown == "shown tool" {
                continue;
            }
            assert!(body.contains(shown), "{mode}: {shown} in {body}");
        }
        for hidden in [
            "Hidden title",
            "hidden head",
            "hidden tool",
            "hidden body",
            "HiddenBox",
            "broken body",
        ] {
            assert!(!body.contains(hidden), "{mode}: no {hidden} in {body}");
        }
        assert!(
            body.contains("Error rendering container-box [BrokenBox]"),
            "{mode}: {body}"
        );
        if mode == "tui" {
            let doc: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|err| panic!("{err}: {body}"));
            assert!(doc.to_string().contains(r#""t":"error""#), "{doc}");
        }
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_box_condition_is_a_v_if_over_the_box_in_a_static_template() {
    let xml = r#"<screen require-authentication="false"><widgets>
        <container-box id="ShownBox" condition="shown"><box-header title="Shown"/><box-body><label text="a"/></box-body></container-box>
        <container-box id="PlainBox"><box-body><label text="b"/></box-body></container-box>
    </widgets></screen>"#;
    let path = screen_file("static", xml);
    let location = path.to_string_lossy().into_owned();
    let context = ExecutionContextFactory::test_context();
    let guard = crate::screen::statics::ReportGuard::start(true);
    make_render(context.factory())
        .root_screen(location.clone())
        .render_mode("qvt")
        .render()
        .unwrap_or_else(|err| panic!("{err}"));
    let report = guard.finish();
    let def = ScreenFacade::of(context.factory())
        .screen_def(context.factory(), &location)
        .unwrap();
    let html = def
        .generated()
        .static_template("")
        .unwrap_or_else(|| panic!("no template: {report:?}"))
        .html
        .clone();
    let _ = std::fs::remove_file(&path);
    let shown = html
        .find(r#"<template v-if="(shown)"><m-container-box"#)
        .unwrap_or_else(|| panic!("{html}"));
    let close = html[shown..]
        .find("</m-container-box>\n</template>")
        .unwrap_or_else(|| panic!("{html}"));
    assert!(!html[shown..shown + close].contains("PlainBox"), "{html}");
    assert_eq!(html.matches("<template v-if").count(), 1, "{html}");
}
