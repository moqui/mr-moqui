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

//! The qvt mode: the Vue template each element writes. A fixture screen in `script/testdata/screen/qvt/render/<name>.xml`
//! is rendered and compared with `<name>.qvt` beside it. Set `UPDATE_GOLDEN=1` to rewrite the expected files, then
//! read the diff. Every template, here and in the other tests that render one, is compiled with the vendored
//! Vue compiler in QuickJS and fails on any compiler error.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::context::ExecutionContextFactory;
use crate::screen::{make_render, vue_check};
use moqui_lib::object::Value;

fn render_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/script/testdata/screen/qvt/render")
}

fn compiler_script() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/base-component/webroot/screen/webroot/libs/vue/compiler-dom.cjs.js");
    std::fs::read_to_string(path).expect("compiler-dom.cjs.js")
}

/// Assert that every template compiles with the Vue compiler.
pub(crate) fn assert_compiles(names: &[String], templates: &[String]) {
    assert_eq!(names.len(), templates.len());
    let errors = vue_check::compile_errors(&compiler_script(), templates).unwrap();
    let named: Vec<String> = errors
        .iter()
        .map(|error| {
            let index: usize = error.split(':').next().and_then(|text| text.parse().ok()).unwrap_or(0);
            format!("{}: {error}", names.get(index).map_or("?", String::as_str))
        })
        .collect();
    assert!(named.is_empty(), "Vue compiler errors:\n{}", named.join("\n"));
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
        .render_mode("qvt")
        .parameters(map)
        .render()
        .unwrap_or_else(|err| panic!("{name}: {err}"));
    assert_eq!(output.content_type(), Some("text/html;charset=UTF-8"));
    output.body_text()
}

fn check(name: &str, parameters: &[(&str, &str)]) -> String {
    check_path(name, "", name, parameters)
}

fn check_path(name: &str, screen_path: &str, golden: &str, parameters: &[(&str, &str)]) -> String {
    let actual = render_path(name, screen_path, parameters);
    let path = render_dir().join(format!("{golden}.qvt"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &actual).unwrap();
    } else {
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {}", path.display()));
        assert_eq!(actual, expected, "{name} differs from {}", path.display());
    }
    assert_compiles(&[golden.to_string()], std::slice::from_ref(&actual));
    actual
}

#[test]
fn containers_links_and_labels() {
    let out = check("containers", &[]);
    // data is text and not an interpolation
    assert!(!out.contains("{{"), "{out}");
    // a container-dialog's tooltip, expanded; a hidden-form-link is a flat button with its text at the left, and its
    // background-* attributes are the m-form's submit-* properties, as a form-single's
    for expected in [
        r#"<m-container-dialog id="TipDlg" color="primary" button-text="" button-icon="sym_o_add" title="" tooltip="Add for Ada">"#,
        r#"<m-form action="/save" read-only submit-message="Saved Ada" submit-reload-id="Dyn" submit-hide-id="Dlg"><q-btn dense no-caps type="submit" flat class="m-link m-link-left text-primary">Mark done</q-btn></m-form>"#,
    ] {
        assert!(out.contains(expected), "{expected} in {out}");
    }
}

#[test]
fn form_single_with_every_field_type() {
    let out = check("fields", &[("_openDialog", "none")]);
    assert!(out.contains("v-slot:default=\"formProps\""), "{out}");
    assert!(out.contains(":fields-initial=\""), "{out}");
    assert!(out.contains(r#"<div class="q-my-md"><m-form name="Everything""#), "{out}");
    assert!(out.contains(r#"<div class="q-ma-sm invoice-field"><m-text-line id="Everything_firstName""#), "{out}");
    assert!(out.contains(r#"<div class="q-ma-sm invoice-idle"><m-text-area name="bio""#), "{out}");
    assert!(out.contains(r#"<div class="q-my-sm big-row"><div class="big-row-item">Big</div><div class="big-row-item"><m-radio"#), "{out}");
    for wrapper in [
        "m-text-line",
        "m-text-area",
        "m-drop-down",
        "m-check",
        "m-radio",
        "m-date-time",
        "m-date-period",
        "m-display",
        "m-file",
        "m-text-find",
        "m-range-find",
    ] {
        assert!(out.contains(&format!("<{wrapper}")), "{wrapper} in {out}");
    }
    for raw in ["<q-input", "<q-select", "<q-date", "<q-time", "<q-field"] {
        assert!(!out.contains(raw), "{raw} in {out}");
    }
}

/// `context` is an m-screen-context with the request's values (a title localized and encoded, an expression that comes to
/// nothing left out), and `shell-providers`, a child of the screen, renders nothing in any mode.
#[test]
fn the_screen_context_and_the_shell_providers() {
    let out = check("shell", &[]);
    assert!(
        out.contains(r#"<m-screen-context record-entity="mantle.order.OrderHeader" record-id="SO10023" title="Order {{x}} &amp; more" selection-form="ItemList"></m-screen-context>"#),
        "{out}"
    );
    assert!(
        out.contains(r#"<m-screen-context record-entity="mantle.order.OrderHeader"></m-screen-context>"#),
        "{out}"
    );
    for provider in ["quick-create", "QuickNote", "palette", "Records", "g l"] {
        assert!(!out.contains(provider), "{provider} in {out}");
    }
    for mode in ["html", "text", "csv", "xml"] {
        let context = ExecutionContextFactory::test_context();
        let output = make_render(context.factory())
            .root_screen(render_dir().join("shell.xml").to_string_lossy().into_owned())
            .screen_path("")
            .render_mode(mode)
            .render()
            .unwrap_or_else(|err| panic!("{mode}: {err}"));
        let text = output.body_text();
        // csv and xml write only forms
        assert!(
            matches!(mode, "csv" | "xml") || text.contains("the context renders nothing visible"),
            "{mode}: {text}"
        );
        for written in ["SO10023", "screen-context", "QuickNote", "Records"] {
            assert!(!text.contains(written), "{written} in {mode}: {text}");
        }
    }
}

/// `dashboard` is a grid of cells, one per child (a card whose condition is false leaves an empty cell the style sheet
/// hides), and `number-card` an m-number-card: the title localized and encoded, the value and delta expanded, a link to a
/// screen as its path with the query, a link elsewhere external. The html mode writes plain cards and the text mode a line.
#[test]
fn a_dashboard_lays_out_number_cards() {
    let out = check("Dashboard", &[]);
    for expected in [
        r#"<div id="Counters" class="m-dashboard counters" style="--m-dashboard-min:160px">"#,
        r#"<div class="m-dashboard-item"><m-number-card title="Open orders" value="42" delta="+3" icon="sym_o_receipt_long" href="/Orders?statusId=Open"></m-number-card>"#,
        r#"<m-number-card title="Late shipments" value="3" delta="+1" delta-good="down" href="https://example.com/late" external size="lg"></m-number-card>"#,
        r#"<div class="m-dashboard-item"></div>"#,
        r#"<m-number-card id="Revenue" title="Revenue {{x}} &amp; more" value="$1,200" delta="-5%" icon="fa fa-dollar" size="sm" class="wide"></m-number-card>"#,
        r#"<div class="m-dashboard"><div class="m-dashboard-item"><m-number-card title="Plain" value="7"></m-number-card>"#,
        // a ${} url is expanded and resolved against the screen on the server
        r#"<m-number-card title="Held orders" value="1" href="/Orders?statusId=Held"></m-number-card>"#,
    ] {
        assert!(out.contains(expected), "{expected} in {out}");
    }
    assert!(!out.contains("Hidden"), "{out}");
    for (mode, expected) in [
        (
            "html",
            vec![
                r#"<div id="Counters" class="dashboard counters" style="--dashboard-min:160px"><div class="dashboard-item">"#,
                r#"<a href="/Orders?statusId=Open" class="number-card"><span class="material-symbols-outlined icon" aria-hidden="true">receipt_long</span> <span class="number-card-title">Open orders</span><strong class="number-card-value">42</strong><span class="number-card-delta" data-tone="good">+3</span></a>"#,
                r#"<span class="number-card-delta" data-tone="bad">+1</span>"#,
                r#"<div id="Revenue" class="number-card wide" data-size="sm"><i class="fa fa-dollar"></i> <span class="number-card-title">Revenue {{x}} &amp; more</span>"#,
                r#"<span class="number-card-delta" data-tone="bad">-5%</span>"#,
            ],
        ),
        (
            "text",
            vec!["Open orders: 42 (+3)", "Late shipments: 3 (+1)", "Plain: 7"],
        ),
        (
            "html-print",
            vec![
                r#"<div class="number-card"><div class="number-card-title">Open orders</div><div class="number-card-value">42</div><div>+3</div></div>"#,
            ],
        ),
    ] {
        let context = ExecutionContextFactory::test_context();
        let output = make_render(context.factory())
            .root_screen(render_dir().join("Dashboard.xml").to_string_lossy().into_owned())
            .screen_path("")
            .render_mode(mode)
            .render()
            .unwrap_or_else(|err| panic!("{mode}: {err}"));
        let text = output.body_text();
        for expected in expected {
            assert!(text.contains(expected), "{expected} in {mode}: {text}");
        }
        assert!(!text.contains("Hidden"), "{mode}: {text}");
    }
}

/// `chatter` and `record-files` are an m-chatter and an m-record-files with the request's record and hints (an expression that
/// comes to nothing left out, a false condition nothing): the client finds the theme's subscreen of the current app root. A
/// text-area with editor-type rich or rich-light is an m-text-area with that editor type and the mention or record
/// transition's URL, which a plain text-area leaves out. The html mode writes no link without a theme value, and the text modes nothing.
#[test]
fn the_record_panels_and_the_rich_text_editors() {
    let out = check("records", &[]);
    for expected in [
        r#"<m-chatter id="TaskChatter" record-entity="mantle.work.effort.WorkEffort" record-id="T100" default-tab="activity" hide="message"></m-chatter>"#,
        r#"<m-chatter record-entity="mantle.work.effort.WorkEffort"></m-chatter>"#,
        r#"<m-record-files record-entity="mantle.work.effort.WorkEffort" record-id="T100" class="q-mt-sm"></m-record-files>"#,
        r#"editor-type="rich-light""#,
        r#"mention-url="/mentions""#,
        r#"editor-type="rich" rows="6""#,
        r#"record-url="/records""#,
    ] {
        assert!(out.contains(expected), "{expected} in {out}");
    }
    assert_eq!(out.matches("<m-chatter").count(), 2, "{out}");
    assert_eq!(out.matches("mention-url").count(), 1, "{out}");
    assert_eq!(out.matches("record-url").count(), 1, "{out}");
    for mode in ["html", "text", "csv", "xml"] {
        let context = ExecutionContextFactory::test_context();
        let output = make_render(context.factory())
            .root_screen(render_dir().join("records.xml").to_string_lossy().into_owned())
            .screen_path("")
            .render_mode(mode)
            .render()
            .unwrap_or_else(|err| panic!("{mode}: {err}"));
        let text = output.body_text();
        assert!(
            matches!(mode, "csv" | "xml") || text.contains("the panels render no text of their own"),
            "{mode}: {text}"
        );
        for written in ["T100", "chatter", "record-panel", "TaskChatter"] {
            assert!(!text.contains(written), "{written} in {mode}: {text}");
        }
    }
}

/// `html-view` is an m-html-view with the HTML and text as attribute values (so `{{` in a message is never compiled), the
/// allow-remote condition and the cid map bound; a false condition writes nothing. The html mode cleans the HTML on the
/// server into a frame sandboxed without scripts whose policy loads only data: images; the text modes write its text.
#[test]
fn the_html_view_element() {
    let out = check("htmlview", &[]);
    for expected in [
        r#"<m-html-view id="Body" html="&lt;p&gt;Hi {{ada}} &lt;b&gt;there&lt;/b&gt;&lt;script&gt;alert(1)&lt;/script&gt;&lt;img src=&quot;cid:logo@x&quot;&gt;&lt;/p&gt;" :allow-remote="true" :cid-map="{'logo@x':'/apps/mail/attachment?id=7'}" class="q-mt-sm"></m-html-view>"#,
        r#"<m-html-view text="Plain {{text}} &amp; more" :collapse-quotes="false" dark="off"></m-html-view>"#,
    ] {
        assert!(out.contains(expected), "{expected} in {out}");
    }
    assert_eq!(out.matches("<m-html-view").count(), 2, "{out}");
    let render = |mode: &str| {
        let context = ExecutionContextFactory::test_context();
        let output = make_render(context.factory())
            .root_screen(render_dir().join("htmlview.xml").to_string_lossy().into_owned())
            .screen_path("")
            .render_mode(mode)
            .render()
            .unwrap_or_else(|err| panic!("{mode}: {err}"));
        output.body_text()
    };
    let html = render("html");
    assert_eq!(html.matches("<iframe").count(), 2, "{html}");
    assert!(
        html.contains(r#"sandbox="allow-popups allow-popups-to-escape-sandbox""#),
        "{html}"
    );
    assert!(
        !html.contains("allow-scripts") && !html.contains("allow-same-origin"),
        "{html}"
    );
    assert!(html.contains("img-src data:;"), "{html}");
    assert!(
        !html.to_lowercase().contains("&lt;script"),
        "the cleaned body has no script: {html}"
    );
    assert!(html.contains("Hi {{ada}}"), "{html}");
    let text = render("text");
    assert!(text.contains("Hi {{ada}} there"), "{text}");
    assert!(text.contains("Plain {{text}} & more"), "{text}");
    assert!(!text.contains("alert(1)") || !text.contains("<script"), "{text}");
}

/// `code-view` is an m-code with the code as an attribute value (so `{{` in a file is never compiled), the file name or language
/// and the line options; a false condition writes nothing. The html and print modes write a pre of the code, escaped; the text
/// modes the code; the tui mode a pre block of an html node.
#[test]
fn the_code_view_element() {
    let out = check("codeview", &[]);
    for expected in [
        "<m-code id=\"Source\" text=\"fn main() {\n    let t = &quot;{{x}} &lt;b&gt;&quot;;\n}\n\" file-name=\"src/main.rs\" highlight-lines=\"2\" class=\"q-mt-sm\"></m-code>",
        "<m-code text=\"fn main() {\n    let t = &quot;{{x}} &lt;b&gt;&quot;;\n}\n\" language=\"rust\" :line-numbers=\"false\"></m-code>",
    ] {
        assert!(out.contains(expected), "{expected} in {out}");
    }
    assert_eq!(out.matches("<m-code").count(), 2, "{out}");
    let html = render_mode("codeview", "", "html", &[]);
    assert!(
        html.contains("<pre id=\"Source\" class=\"code-view q-mt-sm\"><code>fn main() {\n    let t = &quot;{{x}} &lt;b&gt;&quot;;\n}</code></pre>"),
        "{html}"
    );
    assert_eq!(html.matches("<pre").count(), 2, "{html}");
    let text = render_mode("codeview", "", "text", &[]);
    assert!(text.contains("fn main() {\n    let t = \"{{x}} <b>\";\n}\n"), "{text}");
    let tui = render_mode("codeview", "", "tui", &[]);
    assert!(tui.contains("pre") && tui.contains("{{x}} <b>"), "{tui}");
}

#[test]
fn form_lists() {
    let out = check("lists", &[]);
    assert!(out.contains("<m-form-paginate"), "{out}");
    assert!(out.contains("class=\"tr\""), "{out}");
}

/// An anchor to a transition that only redirects is a link to that screen. A transition with actions, and
/// `expand-transition-url="false"`, stay on the transition so a submit still posts it.
#[test]
fn an_alias_transition_link_points_at_the_screen() {
    // `.qvt` is always-standalone, so the body is List. A plain path renders Alias and stops at its placeholder.
    let out = render_path("Alias", "/List.qvt", &[]);
    assert_compiles(&["alias".to_string()], std::slice::from_ref(&out));
    assert!(out.contains("href=\"/Detail?id=I1&amp;fromResponse=yes\""), "{out}");
    assert!(
        out.contains(
            "<m-link href=\"/Detail?id=I1&amp;fromResponse=yes\" id=\"Rows_id_0_detail\" class=\"m-link-text\">"
        ),
        "{out}"
    );
    assert!(out.contains("href=\"/List/detail?id=I1\""), "{out}");
    assert!(out.contains("href=\"/List/acted?id=I1\""), "{out}");
    // expand defaults to true, so this submit goes to the screen. false keeps the post on the transition.
    assert!(out.contains("action=\"/Detail\""), "{out}");
    assert!(out.contains("action=\"/List/detail\""), "{out}");
    // The extra path of an alias is the path-parameter, outside the row so the row's id is not the source.
    assert!(
        out.contains("href=\"/Detail?id=I1\" class=\"m-link-text\">By path"),
        "{out}"
    );
    // type=none is not a screen URL, so the anchor is <a>. The alias above stays m-link. In a list cell a screen link is text.
    assert!(out.contains("<a href=\"/List/gone\" class=\"m-link-text\">"), "{out}");
    assert!(!out.contains("<m-link href=\"/List/gone\">"), "{out}");
    // An http(s) value with url-type screen is that string, not a screen walk.
    assert!(
        out.contains("<a href=\"https://example.com/x\" class=\"m-link-text\">"),
        "{out}"
    );
    // A screen that does not exist is `#`.
    assert!(out.contains(">#</span>"), "{out}");
    // The query of an alias transition's url goes to the screen it names, beside the response's parameters.
    assert!(
        out.contains("href=\"/Detail?id=Q2&amp;fromResponse=yes\" class=\"m-link-text\">With query"),
        "{out}"
    );
}

/// A render of a fixture screen in another mode, at a subscreen path.
fn render_mode(name: &str, path: &str, mode: &str, parameters: &[(&str, &str)]) -> String {
    let context = ExecutionContextFactory::test_context();
    let map: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::string(*value)))
        .collect();
    make_render(context.factory())
        .root_screen(render_dir().join(format!("{name}.xml")).to_string_lossy().into_owned())
        .screen_path(path)
        .render_mode(mode)
        .parameters(map)
        .render()
        .unwrap_or_else(|err| panic!("{name} {mode}: {err}"))
        .body_text()
}

/// `container-split` is an m-container-split with its attributes (the style expanded, back-text localized) and its panes as
/// the side and main slots; the selection is read from the URL in the browser. html is two columns with the selection and a
/// back link without it, and the text modes write the side and then the main pane.
#[test]
fn a_split_is_a_splitter_with_a_side_and_a_main_pane() {
    let out = check("split", &[("partyId", "P7")]);
    for expected in [
        r#"<m-container-split id="ContactSplit" side-width="300" min-main="520" select-parameter="partyId" back-text="All contacts" class="contacts-split"><template v-slot:side><p class="text-inline">the side list</p>"#,
        r#"</template><template v-slot:main><p class="text-inline">the selected P7</p>"#,
        r#"<m-container-split side-position="right" height="calc(100vh - 160px)"><template v-slot:side><span class="text-inline">right side</span>"#,
    ] {
        assert!(out.contains(expected), "{expected} in {out}");
    }
    assert_eq!(out.matches("</template></m-container-split>").count(), 2, "{out}");
    let html = render_mode("split", "", "html", &[("partyId", "P7")]);
    for expected in [
        r#"<div id="ContactSplit" class="split contacts-split" style="--split-side:300px" data-select="partyId" data-selected><div class="split-side">"#,
        r#"<div class="split-main"><a class="split-back" href="/">&larr; All contacts</a>"#,
        r#"<div class="split split-right" style="--split-side:320px"><div class="split-side">"#,
    ] {
        assert!(html.contains(expected), "{expected} in {html}");
    }
    let unselected = render_mode("split", "", "html", &[]);
    assert!(
        !unselected.contains("data-selected") && !unselected.contains("split-back"),
        "{unselected}"
    );
    for mode in ["text", "csv", "xml"] {
        let text = render_mode("split", "", mode, &[("partyId", "P7")]);
        if mode == "text" {
            let side = text.find("the side list").unwrap_or_else(|| panic!("{text}"));
            let main = text.find("the selected P7").unwrap_or_else(|| panic!("{text}"));
            assert!(
                side < main && text.contains("right side") && text.contains("left main"),
                "{text}"
            );
        }
        assert!(
            !text.contains("split-side") && !text.contains("m-container-split"),
            "{mode}: {text}"
        );
    }
}

/// `subscreens-panel type="wizard"` is an m-subscreens-wizard and `type="stack"` an m-subscreens-stack, which read the steps
/// from the menu data; html writes the steps as a numbered sequence of links with Back and Next, and a stack's breadcrumb.
#[test]
fn wizard_and_stack_panels() {
    let out = check_path("Wizard", "", "wizard", &[]);
    assert!(
        out.contains(r#"<m-subscreens-wizard id="ImportWizard" finish-text="Import"></m-subscreens-wizard>"#),
        "{out}"
    );
    let out = check_path("Stack", "", "stack", &[]);
    assert!(
        out.contains(r#"<m-subscreens-stack id="Pages"></m-subscreens-stack>"#),
        "{out}"
    );
    let html = render_mode("Wizard", "/Mapping", "html", &[]);
    for expected in [
        r#"<nav id="ImportWizard" class="wizard"><ol><li class="done"><a href="/Source">Source</a></li><li class="active"><span>Mapping</span></li><li><a href="/Review">Review</a></li></ol></nav>"#,
        r#"<div class="wizard-step">"#,
        "the mapping step",
        r#"<nav class="wizard-nav"><a data-btn="default" href="/Source">Back</a><a data-btn="primary" href="/Review">Next</a></nav>"#,
    ] {
        assert!(html.contains(expected), "{expected} in {html}");
    }
    let first = render_mode("Wizard", "", "html", &[]);
    assert!(
        first.contains(r#"<li class="active"><span>Source</span></li>"#) && first.contains("the source step"),
        "{first}"
    );
    assert!(
        first.contains(r#"<nav class="wizard-nav"><span></span><a data-btn="primary" href="/Mapping">Next</a></nav>"#),
        "{first}"
    );
    let stack = render_mode("Stack", "/Page", "html", &[]);
    assert!(
        stack.contains(
            r#"<nav id="Pages" class="stack-crumbs"><a href="/List">All pages</a> &rsaquo; <span>One page</span></nav>"#
        ) && stack.contains("a page over it"),
        "{stack}"
    );
    let bottom = render_mode("Stack", "", "html", &[]);
    assert!(
        !bottom.contains("stack-crumbs") && bottom.contains("the bottom of the stack"),
        "{bottom}"
    );
}

#[test]
fn subscreen_menus_tabs_and_panels() {
    check_path("Menu", "", "menu-one", &[]);
    check_path("Menu", "/Two", "menu-two", &[]);
    check_path("Panel", "", "panel-one", &[]);
    check_path("Panel", "/Two", "panel-two", &[("lastStandalone", "0")]);
}

#[test]
fn request_text_that_looks_like_an_interpolation_is_not_one() {
    let out = check("braces", &[]);
    // text is character references, and an attribute is not an interpolation in Vue
    assert!(
        out.contains("<p class=\"text-inline\">a &#123;&#123;b}} c</p>"),
        "{out}"
    );
    assert!(
        out.contains("<span class=\"text-inline\">&#123;&#123;literal}}</span>"),
        "{out}"
    );
}

#[test]
fn row_selection_multi_forms_find_dialogs_and_the_options_of_a_drop_down() {
    let out = check("extras", &[]);
    for wrapper in [
        "<m-checkbox-set",
        "<m-form-column-config",
        "<m-container-dialog",
        "<m-form-paginate",
        "<m-text-find",
        "<m-check",
        "<m-text-area",
        "<m-date-time",
        "<m-display",
        "<m-form-link",
    ] {
        assert!(out.contains(wrapper), "{wrapper} in {out}");
    }
    assert!(out.contains("editor-type=\"md\""), "{out}");
    // the checkbox of each row points at the state of the set
    assert!(out.contains(":model-value=\"formProps.checkboxStates[0]\""), "{out}");
    assert!(out.contains(":checkbox-values=\"['I1','I2']\""), "{out}");
    // a form that posts all rows has one set of fields with the row number after each name
    assert!(
        out.contains("'title_0':'One'") && out.contains("'title_1':'Two'"),
        "{out}"
    );
    // a box that starts closed binds the Boolean prop: the text "false" would be truthy and leave it open
    assert!(
        out.contains(r#"<m-container-box type="default" title="Closed" :initial-open="false">"#),
        "{out}"
    );
}

/// A qvt render error is logged and written in place of what failed, as the html render does: a section's error replaces
/// the section and the screen goes on, and an error in the screen itself replaces the screen, which `.qvt` renders alone.
/// What the failed part wrote is dropped so the template still compiles, and a brace in the message is not an
/// interpolation.
#[test]
fn a_qvt_render_error_is_logged_and_written_in_its_place() {
    let dir = std::env::temp_dir().join(format!("moqui-qvt-error-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    let write = |path: &str, text: &str| std::fs::write(dir.join(path), text).unwrap();
    write(
        "Root.xml",
        r#"<screen require-authentication="false"><subscreens default-item="Section"/>
            <widgets><subscreens-active/></widgets></screen>"#,
    );
    write(
        "Root/Section.xml",
        r#"<screen require-authentication="false"><widgets>
            <label text="before"/>
            <section name="Broken"><widgets><container><label text="inside"/>
                <render-mode><text type="qvt"><![CDATA[<b>${nope.bad()}</b>]]></text></render-mode>
            </container></widgets></section>
            <label text="after"/>
        </widgets></screen>"#,
    );
    write(
        "Root/Whole.xml",
        r#"<screen require-authentication="false"><widgets><container><label text="first"/>
            <render-mode><text type="qvt"><![CDATA[${(() => { throw new Error('a {{ brace }}') })()}]]></text></render-mode>
        </container></widgets></screen>"#,
    );
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let context = ExecutionContextFactory::test_context();
    let render = |path: &str| {
        make_render(context.factory())
            .root_screen(root.clone())
            .screen_path(path)
            .render_mode("qvt")
            .render()
            .unwrap_or_else(|err| panic!("{path}: {err}"))
            .body_text()
    };
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (section, whole) =
        tracing::subscriber::with_default(crate::testing::LogSubscriber(std::sync::Arc::clone(&lines)), || {
            (render("/Section.qvt"), render("/Whole.qvt"))
        });
    let section_location = dir.join("Root/Section.xml").to_string_lossy().into_owned();
    let whole_location = dir.join("Root/Whole.xml").to_string_lossy().into_owned();
    assert!(section.contains(">before<") && section.contains(">after<"), "{section}");
    assert!(
        section.contains(&format!(
            "Error rendering section [Broken] in screen [{section_location}]"
        )),
        "{section}"
    );
    assert!(!section.contains("inside"), "{section}");
    assert!(
        whole.contains(&format!("Error rendering screen [{whole_location}]")),
        "whole={whole:?}"
    );
    assert!(!whole.contains("first") && !whole.contains("{{"), "{whole}");
    assert!(whole.contains("a &#123;&#123; brace }}"), "{whole}");
    assert_compiles(&["section".to_string(), "whole".to_string()], &[section, whole]);
    let logged = lines.lock().unwrap().join("\n");
    assert!(
        logged.contains(&format!(
            "Error rendering section [Broken] in screen [{section_location}]"
        )),
        "{logged}"
    );
    assert!(
        logged.contains(&format!("Error rendering screen [{whole_location}]")),
        "{logged}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `WebrootVue.qvt.render.js` is the render functions of the client components as their templates are now: the file
/// is made again and compared. `UPDATE_GOLDEN=1` writes it.
#[test]
fn client_components_are_precompiled() {
    let js = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/base-component/webroot/screen/webroot");
    let read = |path: &str| std::fs::read_to_string(js.join(path)).unwrap_or_else(|err| panic!("{path}: {err}"));
    let compiler = read("libs/vue/compiler-dom.cjs.js");
    let made = crate::screen::vue_check::precompiled_components(
        &compiler,
        &[
            &read("js/MoquiLib.js"),
            &read("js/WebrootChat.js"),
            &read("js/WebrootAssist.js"),
            &read("js/WebrootVue.qvt.js"),
        ],
    )
    .unwrap();
    let path = js.join("js/WebrootVue.qvt.render.js");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &made).unwrap();
        return;
    }
    let kept = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        kept == made,
        "{} is not the render functions of the current templates: UPDATE_GOLDEN=1 cargo test -p moqui-core --lib client_components_are_precompiled",
        path.display()
    );
    // no with, no eval: the build without the compiler and a CSP without unsafe-eval can load it
    for banned in ["with (", "with(", "new Function", "eval("] {
        assert!(!made.contains(banned), "{banned}");
    }
}

/// A section in a form-list cell is a unit of its own: it starts from the list's state (its links are in-cell links with the
/// row's ids, a dialog's button is the flat in-cell one) and gives the state back, so the cells after it in the row and the
/// rows after it are still in the list. A link's btn-type is expanded in each row and entry, an editable is a select, a date
/// picker or the placeholder of an empty text with display-if-empty, and a container-row takes the row-col elements of a
/// section-iterate, their sizes expanded.
#[test]
fn a_section_in_a_list_cell_keeps_the_list_and_link_styles_are_expanded() {
    let out = check("cells", &[]);
    for expected in [
        // the container-dialog in the section has the row's id and is the in-cell flat button, its form's fields the row's ids
        r#"<m-container-dialog id="RefuseDialog_0" color="negative" button-text="Refuse" title="Refuse"><template v-slot:button><q-btn dense flat no-caps class="m-link" label="Refuse"></q-btn></template>"#,
        r#"id="RefuseForm_reason_0""#,
        // the link after the section, in this row and the next: a text link of the list with the row's id
        r#"<a href="/open" id="Cells_open_0_open" class="m-link-text">Open A1</a>"#,
        r#"<a href="/open" id="Cells_open_1_open" class="m-link-text">Open A2</a>"#,
        r#"<a href="/open" id="Cells_chip_1_open">"#,
        // btn-type from an expression: primary in the first row, danger (negative) in the second
        r#"<q-btn dense unelevated no-caps color="primary" class="m-link">Chip</q-btn>"#,
        r#"<q-btn dense unelevated no-caps class="m-btn-tonal text-negative m-link">Chip</q-btn>"#,
        // and in a section-iterate
        r#"<q-btn dense unelevated no-caps color="primary" class="m-link">All</q-btn>"#,
        r#"<q-btn dense unelevated no-caps class="m-btn-tonal text-primary m-link">Mine</q-btn>"#,
        // editable: the empty note of the second row with its placeholder, a select with options and the current key,
        // a select from editable-load, a date and an empty date-time
        r#"id="NoteEdit_1" label-type="span" label-value="""#,
        r#"widget-type="select""#,
        r#"value="B" :options="[{'value':'A','label':'First'},{'value':'B','label':'Second'}]""#,
        r#"load-url="/kindOptions""#,
        r#"widget-type="date""#,
        r#"widget-type="date-time""#,
        r#"placeholder="Click to edit""#,
        // container-row: the expanded sizes, and a column per entry of the section-iterate
        r#"<div class="q-px-xs col-lg-8 col-md-8 col-sm-12 col-xs-12">"#,
        r#"<div class="q-px-xs col-md-2 col-sm-12 col-xs-12"><span class="text-inline">col All</span>"#,
        r#"<div class="q-px-xs col-md-2 col-sm-12 col-xs-12"><span class="text-inline">col Mine</span>"#,
    ] {
        assert!(out.contains(expected), "{expected} in {out}");
    }
    assert!(out.contains(r#"<div class="q-my-md"><m-form name="RefuseForm""#), "{out}");
    assert!(out.contains(r#"<div class="q-ma-sm"><m-text-area name="reason""#), "{out}");
    // the open link of each row is in the list, never the tonal button of a link outside a list
    assert!(
        !out.contains(r#"class="m-btn-tonal text-primary m-link">Open"#),
        "{out}"
    );
    // an empty editable without display-if-empty writes nothing
    assert!(!out.contains(r#"id="Hidden""#), "{out}");
}

/// `columns type="mobile"` are the columns of a request with `_uiType=mobile` (the client sends it below 600 px), as Java;
/// the untyped columns otherwise.
#[test]
fn mobile_columns_are_chosen_by_the_ui_type() {
    let desktop = render_path("mobile", "", &[]);
    let mobile = render_path("mobile", "", &[("_uiType", "mobile")]);
    assert_compiles(
        &["mobile".to_string(), "mobile ui".to_string()],
        &[desktop.clone(), mobile.clone()],
    );
    // desktop: a column per field; mobile: the title and caption in one cell, the action in the other
    let count = |text: &str, needle: &str| text.matches(needle).count();
    assert_eq!(count(&desktop, "Widget"), 1, "{desktop}");
    assert!(
        count(&desktop, "class=\"th") > count(&mobile, "class=\"th"),
        "fewer header cells on a phone:\n{desktop}\n{mobile}"
    );
    assert!(mobile.contains("note A"), "{mobile}");
}

/// A test context with the framework's services and a database holding the EntitySync statuses and their Default flow.
pub(super) fn status_context() -> crate::context::ExecutionContext {
    let context = ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    facade.assume_db_tables();
    context.factory().set_entity(facade.clone());
    {
        let _entered = context.enter();
        let _authz = context.disable_authz();
        crate::service::load_definitions().unwrap();
        let row = |entity: &str, fields: &[(&str, &str)]| {
            let mut value = facade.make_value(entity).unwrap();
            for (name, field) in fields {
                value.set(*name, Value::string(*field));
            }
            value.create(&facade).unwrap();
        };
        row(
            "moqui.basic.StatusType",
            &[("statusTypeId", "EntitySync"), ("description", "Entity Sync")],
        );
        for (id, sequence, description) in [
            ("EsNotStarted", "1", "Not Started"),
            ("EsRunning", "2", "Running"),
            ("EsComplete", "4", "Complete"),
            ("EsOtherError", "98", "Other Error"),
        ] {
            row(
                "moqui.basic.StatusItem",
                &[
                    ("statusId", id),
                    ("statusTypeId", "EntitySync"),
                    ("sequenceNum", sequence),
                    ("description", description),
                ],
            );
        }
        row(
            "moqui.basic.StatusFlow",
            &[("statusFlowId", "Default"), ("description", "Default")],
        );
        for (from, to, name) in [
            ("EsNotStarted", "EsRunning", "Running"),
            ("EsRunning", "EsComplete", "Complete"),
            ("EsRunning", "EsOtherError", "Error"),
            ("EsRunning", "EsNotStarted", "Reset Not Started"),
            ("EsComplete", "EsRunning", "Running"),
            ("EsOtherError", "EsRunning", "Running"),
        ] {
            row(
                "moqui.basic.StatusFlowTransition",
                &[
                    ("statusFlowId", "Default"),
                    ("statusId", from),
                    ("toStatusId", to),
                    ("transitionName", name),
                ],
            );
        }
        // a flow of its own, which a board names with status-flow-id
        row(
            "moqui.basic.StatusFlow",
            &[("statusFlowId", "FastSync"), ("description", "Fast")],
        );
        row(
            "moqui.basic.StatusFlowTransition",
            &[
                ("statusFlowId", "FastSync"),
                ("statusId", "EsNotStarted"),
                ("toStatusId", "EsComplete"),
                ("transitionName", "Done at once"),
            ],
        );
    }
    context
}

/// The status-bar element (style-guide §8.2): the stages of the status type in sequence order with the current one filled
/// and the ones before it done, the statuses that wait or end badly off the row unless current, the colors by the status
/// color map, and the transitions from the current status as buttons that post the transition with the new status, the
/// next stage the primary one; read-only, allowed statuses, given stages and the html and text fallbacks.
#[test]
fn a_status_bar_shows_the_stages_and_the_transitions_of_the_flow() {
    let context = status_context();
    let _entered = context.enter();
    let render = |mode: &str, parameters: &[(&str, &str)]| -> String {
        let map: BTreeMap<String, Value> = parameters
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::string(*value)))
            .collect();
        make_render(context.factory())
            .root_screen(render_dir().join("status.xml").to_string_lossy().into_owned())
            .screen_path("")
            .render_mode(mode)
            .parameters(map)
            .render()
            .unwrap_or_else(|err| panic!("status {mode}: {err}"))
            .body_text()
    };
    let out = render("qvt", &[]);
    let path = render_dir().join("status.qvt");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &out).unwrap();
    } else {
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {}", path.display()));
        assert_eq!(out, expected, "status differs from {}", path.display());
    }
    assert_compiles(&["status".to_string()], std::slice::from_ref(&out));

    let bar = |id: &str| -> serde_json::Value {
        let start = out
            .find(&format!("<m-status-bar id=\"{id}\""))
            .unwrap_or_else(|| panic!("{id} in {out}"));
        let rest = &out[start..];
        let at = rest.find(":bar=\"").unwrap() + 6;
        let text = &rest[at..at + rest[at..].find('"').unwrap()];
        // a JavaScript object literal with single quotes (no label here has one)
        serde_json::from_str(&text.replace('\'', "\"").replace("&amp;", "&")).unwrap()
    };
    let states = |value: &serde_json::Value| -> Vec<String> {
        value["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|stage| {
                format!(
                    "{}:{}:{}",
                    stage["statusId"].as_str().unwrap(),
                    stage["state"].as_str().unwrap(),
                    stage["color"].as_str().unwrap()
                )
            })
            .collect()
    };
    let buttons = |value: &serde_json::Value| -> Vec<String> {
        value["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tr| {
                let next = if tr["isNext"] == true { "next" } else { "" };
                let danger = if tr["isDanger"] == true { "danger" } else { "" };
                format!("{}:{next}{danger}", tr["toStatusId"].as_str().unwrap())
            })
            .collect()
    };
    let running = bar("SyncStatus");
    assert_eq!(
        states(&running),
        [
            "EsNotStarted:done:grey-7",
            "EsRunning:current:primary",
            "EsComplete:todo:positive"
        ],
        "the error status waits off the row"
    );
    assert_eq!(
        buttons(&running),
        ["EsComplete:next", "EsNotStarted:", "EsOtherError:danger"]
    );
    assert!(
        out.contains(r#"action="/setStatus" :parameters="{'entitySyncId':'S1'}" title="Sync S1""#),
        "{out}"
    );
    let read_only = bar("ReadOnly");
    assert_eq!(states(&read_only).len(), 3);
    assert!(buttons(&read_only).is_empty());
    let read_only_tag = out[out.find("id=\"ReadOnly\"").unwrap()..]
        .split("</m-status-bar>")
        .next()
        .unwrap();
    assert!(!read_only_tag.contains("action="), "{out}");
    let staged = bar("Staged");
    assert_eq!(
        states(&staged),
        [
            "EsNotStarted:todo:grey-7",
            "EsRunning:todo:primary",
            "EsComplete:todo:positive",
            "EsOtherError:current:negative"
        ],
        "ended badly: last, nothing done"
    );
    assert_eq!(
        buttons(&staged),
        ["EsRunning:next"],
        "back to the earliest stage it leads to"
    );
    assert!(out.contains(r#":confirm="false""#), "{out}");
    assert_eq!(buttons(&bar("Allowed")), ["EsComplete:next"]);
    assert!(out.contains(r#"parameter-name="toStatusId""#), "{out}");
    assert!(!out.contains(r#"id="None""#), "no status, no bar: {out}");

    // the status from the request
    let complete = render("qvt", &[("statusId", "EsComplete")]);
    assert!(
        complete.contains("{'statusId':'EsComplete','label':'Complete','color':'positive','state':'current'}"),
        "{complete}"
    );

    // html: chips and a form per transition, the next one primary, each confirming
    let html = render("html", &[]);
    for expected in [
        r#"<div id="SyncStatus" class="status-bar" data-status="EsRunning">"#,
        r#"<span class="status-chip" data-state="done" data-color="grey-7" data-stage="EsNotStarted">&check; Not Started</span>"#,
        r#"data-state="current" data-color="primary" data-stage="EsRunning" aria-current="step">Running</span>"#,
        r#"<input type="hidden" name="entitySyncId" value="S1"><input type="hidden" name="statusId" value="EsComplete"><button type="submit" data-btn="primary" data-to-status="EsComplete" data-confirm="Complete: Sync S1?">Complete</button>"#,
        r#"data-btn="danger" data-to-status="EsOtherError""#,
    ] {
        assert!(html.contains(expected), "{expected} in {html}");
    }
    // text: the current status's description
    let text = render("text", &[]);
    assert!(text.contains("Status: Running"), "{text}");
}

/// A level of the shell in qvt is the screen the server renders, which writes `m-subscreens-active` for the levels below:
/// a parent whose default subscreens end at a `.qvue`-only screen renders in qvt (the client asks for that leaf on its own),
/// a request that renders the leaf in qvt is still not found, and `X-Moqui-Client-Modes` (ScreenTest's `client_modes`) gets
/// the leaf in qvue, with `X-Moqui-Qvt` saying so.
#[test]
fn a_parent_renders_in_qvt_over_a_qvue_only_default_leaf() {
    let dir = std::env::temp_dir().join(format!("moqui-qvt-default-leaf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("App/Inbox")).unwrap();
    let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    write(
        "App.xml",
        r#"<screen require-authentication="false" default-menu-title="App"><subscreens default-item="Inbox"/>
            <widgets><label text="App menu"/><subscreens-active/></widgets></screen>"#,
    );
    write(
        "App/Inbox.xml",
        r#"<screen require-authentication="false" default-menu-title="Inbox"><subscreens default-item="Mail"/>
            <widgets><label text="Inbox frame"/><subscreens-active/></widgets></screen>"#,
    );
    write(
        "App/Inbox/Mail.xml",
        r#"<screen require-authentication="false" default-menu-title="Mail" render-modes="vue,qvue">
            <widgets><render-mode><text type="qvue" template="false"><![CDATA[<template><p>Mail</p></template>
<script>module.exports = { data() { return {} } }</script>]]></text></render-mode></widgets></screen>"#,
    );
    let context = ExecutionContextFactory::test_context();
    let root = dir.join("App.xml").to_string_lossy().into_owned();
    let test = crate::screen::ScreenTest::new(context.factory()).root_screen(root.clone());
    let qvt = test.clone().render_mode("qvt");

    // the app root and the Inbox level, each in qvt with the placeholder of the level below
    for (path, parameters, text) in [("", vec![], "App menu"), ("Inbox.qvt", vec![], "Inbox frame")] {
        let tester = if path.contains('.') { &test } else { &qvt };
        let rendered = tester.test_render(path, parameters.into_iter().collect(), None);
        assert!(
            rendered.error_messages.is_empty(),
            "{path}: {:?}",
            rendered.error_messages
        );
        let out = rendered.text();
        assert!(
            out.contains(text) && out.contains("<m-subscreens-active>"),
            "{path}: {out}"
        );
        assert!(!out.contains("<p>Mail</p>"), "{path}: {out}");
    }
    let inbox = qvt.test_render("Inbox", BTreeMap::new(), None);
    assert!(inbox.error_messages.is_empty(), "{:?}", inbox.error_messages);
    assert!(inbox.text().contains("App menu"), "{}", inbox.text());

    // the leaf itself in qvt: named, or the screen a lastStandalone render starts at
    let named = test.test_render("Inbox/Mail.qvt", BTreeMap::new(), None);
    assert!(
        named
            .error_messages
            .iter()
            .any(|error| error.contains("render mode qvt is not supported")),
        "{:?}",
        named.error_messages
    );
    let standalone = qvt.test_render(
        "Inbox",
        [("lastStandalone".to_string(), Value::string("-2"))]
            .into_iter()
            .collect(),
        None,
    );
    assert!(
        standalone
            .error_messages
            .iter()
            .any(|error| error.contains("render mode qvt is not supported")),
        "{:?}",
        standalone.error_messages
    );
    // html renders the levels inline, so the leaf must take html
    let html = test.clone().render_mode("html").test_render("", BTreeMap::new(), None);
    assert!(
        html.error_messages
            .iter()
            .any(|error| error.contains("render mode html is not supported")),
        "{:?}",
        html.error_messages
    );

    // the best client mode: the leaf in qvue, X-Moqui-Qvt says so; a parent with no qvue of its own stays qvt
    let modes = test.clone().client_modes("qvue,qjs");
    let leaf = modes.test_render("Inbox/Mail.qvt", BTreeMap::new(), None);
    assert!(leaf.error_messages.is_empty(), "{:?}", leaf.error_messages);
    assert_eq!(
        leaf.output.as_ref().and_then(|out| out.header("X-Moqui-Qvt")),
        Some("qvue")
    );
    assert!(leaf.text().contains("<p>Mail</p>"), "{}", leaf.text());
    let parent = modes.test_render("Inbox.qvt", BTreeMap::new(), None);
    assert_eq!(
        parent.output.as_ref().and_then(|out| out.header("X-Moqui-Qvt")),
        Some("merged-fragment")
    );
    let headed =
        test.clone()
            .header("X-Moqui-Client-Modes", "qvue")
            .test_render("Inbox/Mail.qvt", BTreeMap::new(), None);
    assert_eq!(
        headed.output.as_ref().and_then(|out| out.header("X-Moqui-Qvt")),
        Some("qvue")
    );

    // the same from a script: ec.screen.makeTest().clientModes(...), and the response headers on the result
    let _entered = context.enter();
    let script = format!(
        "(() => {{ const t = ec.screen.makeTest().rootScreen({}).clientModes(['qvue', 'qjs']); \
         const leaf = t.render('Inbox/Mail.qvt', {{}}, null); \
         const plain = ec.screen.makeTest().rootScreen({0}).header('X-Moqui-Client-Modes', 'qjs').render('Inbox/Mail.qvt', {{}}, null); \
         return [leaf.status, leaf.header('x-moqui-qvt'), leaf.output.includes('<p>Mail</p>'), plain.errorMessages.length > 0].join(' ') }})()",
        serde_json::to_string(&root).unwrap()
    );
    let answered = crate::script::eval_value(&script).unwrap();
    assert_eq!(moqui_lib::object::to_plain_string(&answered), "200 qvue true true");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The views of a form-list (style-guide §8.2: `views`, `view-board`, `view-calendar`): an m-list-views around the list with the
/// fields the views read as plain values and their display text, the board's and calendar's settings (the transitions with
/// their parameters and the list's hidden-parameters, the key fields, the colors of color-map), and a card_{index} slot per row
/// written as the list writes the cells, outside the row's form; markdown-view is an m-markdown with its text as an attribute
/// value. html shows the list and the markdown as text.
#[test]
fn form_list_views_and_the_markdown_view() {
    let out = check("views", &[]);
    for expected in [
        r##"<m-markdown id="Notes" text="# Notes {{x}}"##,
        r#"base-path="/apps/wiki" strip-title="Notes" :wiki-links="true" class="q-mt-sm"></m-markdown>"#,
        r#"<m-list-views id="Tasks_views" form-name="Tasks""#,
        r#":views="['board','list','calendar']""#,
        "'groupField':'stage','titleField':'name','sumField':'points'",
        "'keyFields':['taskId']",
        "'transition':'/moveTask','parameters':{'projectId':'P9'}",
        r#"form-location="/#Tasks""#,
        "'colors':{'todo':'info','doing':'warning'}",
        "'createTransition':'/newTask'",
        "'modes':['week','month','day','list']",
        // range-find: the calendar asks the screen's form request for the rows of the dates it shows
        "'rangeFind':true,'rangeUrl':'/actions/Tasks','listName':'taskList'",
        "<template v-slot:card_0><div class=\"m-card-head row no-wrap\"><div class=\"m-card-title\">",
        "<span class=\"m-card-label m-muted\">Owner</span>",
        "</m-list-views>",
    ] {
        assert!(out.contains(expected), "{expected} in {out}");
    }
    // the rows: plain values with the display text where it differs (a status label, the formatted number), no request text compiled
    assert!(out.contains("'stage':'todo','stage_display':'To do'"), "{out}");
    assert!(out.contains("'points':3,'points_display':'3.0'"), "{out}");
    assert!(
        out.contains("class=\"m-link-text\">Write &#123;&#123;spec}}</a>"),
        "{out}"
    );
    // a list without a transition: no moves, its key the title field; a card per row, outside any row form
    assert!(
        out.contains(r#"<m-list-views id="Plain_views" form-name="Plain""#),
        "{out}"
    );
    assert_eq!(out.matches("<template v-slot:card_").count(), 9, "{out}");
    // group-fields: the first the default, a Group by choice of each with its title; all-rows asks the form request for up to
    // max-rows; peek-url: the detail's path and the key field, and a list row says its index
    for expected in [
        "'groupField':'owner'",
        "'groups':[{'field':'owner','label':'owner','columns':[],'flow':{}},{'field':'stage','label':'Stage'}]",
        "'allRows':true,'rowsUrl':'/actions/Grouped','listName':'taskList','maxRows':50",
        r#":peek="{'url':'/open','parameters':{},'keyField':'taskId'}""#,
        r#"<div class="tr" data-row="0">"#,
    ] {
        assert!(out.contains(expected), "{expected} in {out}");
    }
    assert!(
        out.contains(r#"<div class="tr"><m-form name="Tasks_0""#),
        "no index without a peek-url: {out}"
    );
    // view-tree: its settings (the columns less the roll-up, the roll-up's title, the transitions), the rows' parent, child count
    // and roll-up, and a tree_{index} slot per row: the row's cells in a form of the row, the first with the tree's lead
    for expected in [
        r#":views="['tree','list']""#,
        "'parentField':'parentId','keyFields':['nodeId'],'childCountField':'kids','rollups':[{'name':'totalPoints','label':'Total points'}]",
        "'columns':[{'fields':['label'],'label':'Item','align':''},{'fields':['points'],'label':'Points','align':'right'}]",
        "'childrenUrl':'/planTask'",
        "'transition':'/moveTask'",
        "{'nodeId':'N2','label':'Sign-in','parentId':'N1','kids':0,'totalPoints':8}",
        r#"<template v-slot:tree_1="mqTree"><m-form name="Outline_t1" id="Outline_t1" action="/open""#,
        r#"<div class="td"><div class="m-tree-cell row no-wrap items-center"><m-tree-lead :t="mqTree.t"></m-tree-lead><div class="m-tree-label">"#,
        r#"<m-text-line id="Outline_points_t1""#,
    ] {
        assert!(out.contains(expected), "{expected} in {out}");
    }
    let slot = &out[out.find(r#"<template v-slot:tree_0="#).unwrap()..];
    let slot = &slot[..slot.find("</template>").unwrap()];
    assert!(
        !slot.contains("totalPoints_0_display") && !slot.contains("Outline_totalPoints"),
        "the roll-up is no cell of the tree row: {slot}"
    );
    let html = render_mode("views", "", "html", &[]);
    assert!(
        !html.contains("m-list-views") && html.contains("Write {{spec}}"),
        "{html}"
    );
    assert!(
        html.contains(r##"<div id="Notes" class="markdown-view q-mt-sm" style="white-space:pre-wrap"># Notes {{x}}"##),
        "{html}"
    );
    // html: the tree, the view chosen first, is the page's rows under their parents, the first cell indented; the list as it is
    let tree = &html[html.find(r#"id="Outline_table""#).unwrap()..];
    assert!(
        tree.find(">Portal<").unwrap() < tree.find(">Sign-in<").unwrap()
            && tree.contains(r#"<tr data-level="2"><td style="padding-left:24px">"#),
        "{tree}"
    );
    let list = render_mode("views", "", "html", &[("view", "list")]);
    let list = &list[list.find(r#"id="Outline_table""#).unwrap()..];
    assert!(
        list.find(">Sign-in<").unwrap() < list.find(">Portal<").unwrap() && !list.contains("data-level"),
        "{list}"
    );
    let text = render_mode("views", "", "text", &[]);
    assert!(text.contains("See [the spec](spec/One)"), "{text}");
}

/// The part of the output from `start` to the first `end` after it.
fn part_of<'a>(out: &'a str, start: &str, end: &str) -> &'a str {
    let from = out.find(start).unwrap_or_else(|| panic!("no {start} in {out}"));
    let rest = &out[from..];
    &rest[..rest.find(end).unwrap_or(rest.len())]
}

/// A row-selection whose condition is false, or fails to evaluate, is written as a list without row-selection (no checkbox set,
/// checkbox column or actions); a view whose move-condition is false has no transition, so its cards and rows do not move. html
/// writes no selection either way.
#[test]
fn a_row_selection_condition_and_a_view_move_condition_turn_them_off() {
    let out = check("gated", &[]);
    assert_eq!(out.matches("<m-checkbox-set").count(), 1, "{out}");
    let on = part_of(&out, "<m-checkbox-set", "</m-checkbox-set>");
    assert!(
        on.contains(r#"checkbox-parameter="taskIds""#) && on.contains(r#"id="SelOn_table""#) && on.contains("Bulk On"),
        "{on}"
    );
    for (name, next) in [("SelOff", "SelBad_table"), ("SelBad", "BoardFixed_views")] {
        let off = part_of(&out, &format!(r#"id="{name}_table""#), next);
        assert!(
            !off.contains("checkbox") && !off.contains("Bulk") && !off.contains(r#"<div class="td">&nbsp;</div>"#),
            "{off}"
        );
        // the rows as a list's with no row-selection: the row's form first
        assert!(
            off.contains(&format!(r#"<div class="tr"><m-form name="{name}_0""#)),
            "{off}"
        );
    }
    let fixed = part_of(&out, r#"<m-list-views id="BoardFixed_views""#, ">");
    assert!(fixed.contains("'transition':null,'parameters':null"), "{fixed}");
    let moves = part_of(&out, r#"<m-list-views id="BoardMoves_views""#, ">");
    assert!(moves.contains("'transition':'/moveTask'"), "{moves}");
    let fixed = part_of(&out, r#"<m-list-views id="TreeFixed_views""#, ">");
    assert!(fixed.contains("'transition':null,'parameters':null"), "{fixed}");
    let moves = part_of(&out, r#"<m-list-views id="TreeMoves_views""#, ">");
    assert!(moves.contains("'transition':'/moveTask'"), "{moves}");
    let html = render_mode("gated", "", "html", &[]);
    assert!(
        html.contains("SelOn") && !html.contains("Bulk") && !html.contains("checkbox"),
        "{html}"
    );
}

/// A board by status writes the columns get#ListViewColumns answers: every status of the type of the rows' statuses in sequence
/// order with its label and color, then a value that is no status, and the flow's transitions between them; a calendar colored
/// by a status gets the status colors; column-order with a status asks for its labels too.
#[test]
fn a_status_board_has_the_columns_and_flow_of_the_status_type() {
    let context = status_context();
    let _entered = context.enter();
    let out = make_render(context.factory())
        .root_screen(render_dir().join("views-status.xml").to_string_lossy().into_owned())
        .screen_path("")
        .render_mode("qvt")
        .render()
        .unwrap_or_else(|err| panic!("views-status: {err}"))
        .body_text();
    assert_compiles(&["views-status".to_string()], std::slice::from_ref(&out));
    for expected in [
        "'columns':[{'value':'EsNotStarted','label':'Not Started','color':'grey-7'},{'value':'EsRunning','label':'Running','color':'primary'},\
         {'value':'EsComplete','label':'Complete','color':'positive'},{'value':'EsOtherError','label':'Other Error','color':'negative'},\
         {'value':'ZZ_UNKNOWN','label':'ZZ_UNKNOWN','color':''}]",
        "'flow':{'EsComplete':['EsRunning'],'EsNotStarted':['EsRunning'],'EsOtherError':['EsRunning'],'EsRunning':['EsComplete','EsNotStarted','EsOtherError']}",
        "'keyFields':['syncId']",
        "'colors':{'EsNotStarted':'grey-7','EsRunning':'primary','EsComplete':'positive','EsOtherError':'negative'}",
        "'columnOrder':['EsComplete','EsRunning']",
        "'statusId':'EsRunning','statusId_display':'Running'",
        // status-flow-id: that flow's transitions are the drag hints, not Default's
        "'statusFlowId':'FastSync'",
        "'flow':{'EsNotStarted':['EsComplete']}",
    ] {
        assert!(out.contains(expected), "{expected} in {out}");
    }
}
