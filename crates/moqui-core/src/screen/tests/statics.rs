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

//! Server-static qvt: the template holds no request data, compiles to a render function with no `with`, is cached and validated, and the
//! `actions` transition answers the data of the screen and of each form-list. The fixtures are under `testdata/Static`.

use std::collections::BTreeMap;

use moqui_lib::object::Value;

use crate::context::ExecutionContextFactory;
use crate::screen::{make_render, RenderOutput, ScreenFacade};

/// `response-header` `screen-server-static` of the shipped conf.
const SERVER_STATIC_CACHE: &str = "max-age=86400, must-revalidate, public";

fn root() -> String {
    format!("{}/src/screen/testdata/Static/Static.xml", env!("CARGO_MANIFEST_DIR"))
}

fn render(factory: &std::sync::Arc<ExecutionContextFactory>, path: &str, parameters: &[(&str, &str)]) -> RenderOutput {
    let map: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::string(*value)))
        .collect();
    make_render(factory)
        .root_screen(root())
        .screen_path(path)
        .parameters(map)
        .render()
        .unwrap_or_else(|err| panic!("{path}: {err}"))
}

fn html_of(factory: &std::sync::Arc<ExecutionContextFactory>, screen: &str, key: &str) -> String {
    let location = format!(
        "{}/src/screen/testdata/Static/Static/{screen}.xml",
        env!("CARGO_MANIFEST_DIR")
    );
    let def = ScreenFacade::of(factory).screen_def(factory, &location).unwrap();
    def.generated()
        .static_template(key)
        .expect("the template was cached")
        .html
        .clone()
}

/// Compare `actual` with the checked-in file under `testdata/Static/golden`. `UPDATE_GOLDEN=1` rewrites it. The Node tests of the client read the same files.
fn golden(name: &str, actual: &str) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/screen/testdata/Static/golden")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {}", path.display()));
    assert_eq!(actual, expected, "{name} differs from {}", path.display());
}

fn header<'a>(output: &'a RenderOutput, name: &str) -> &'a str {
    output
        .header(name)
        .unwrap_or_else(|| panic!("no {name} in {:?}", output.headers))
}

/// The service the number fields of the `Number` screen name, for the browser rules of a number field.
fn number_service() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/screen/testdata/Static/service/NumberService.xml")
}

/// A number field's browser rule calls a name of the scope, not a browser global: the server compiles the
/// template with Vue's `prefixIdentifiers`, so a free identifier becomes `_ctx.<name>` and the scope has to
/// have it. The scope has `$mq` and `ec`; it has no `moqui`.
#[test]
fn a_number_field_rule_calls_the_scope_and_not_a_browser_global() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let failures = crate::service::lint(&[number_service()]).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let output = render(context.factory(), "/Number.qvt", &[]);
    assert_eq!(output.status, 200, "{}", output.body_text());
    let script = output.body_text();
    assert!(!script.contains("_ctx.moqui"), "{script}");
    assert!(script.contains("_ctx.$mq.isStringNumber(value)"), "{script}");
    // count is an integer parameter, so its rule is the integer check, and the class-derived digits rule is not added beside it
    assert!(script.contains("_ctx.$mq.isStringInteger(value)"), "{script}");
    assert!(!script.contains("d*$"), "{script}");
    // the rule message is the host's localized string, quoted into the template; labels still localize in the browser
    assert!(script.contains("Please enter a valid whole number"), "{script}");
    assert!(script.contains("_ctx.ec.l10n.localize"), "{script}");
}

#[test]
fn the_template_holds_no_request_data_and_compiles_without_with() {
    let context = ExecutionContextFactory::test_context();
    let output = render(context.factory(), "/Items.qvt", &[("password", "ZEBRA-secret")]);
    assert_eq!(output.status, 200, "{}", output.body_text());
    assert_eq!(header(&output, "X-Moqui-Qvt"), "static-template");
    assert_eq!(header(&output, "Cache-Control"), SERVER_STATIC_CACHE);
    assert_eq!(header(&output, "Content-Type"), "text/javascript;charset=UTF-8");
    assert!(header(&output, "ETag").starts_with("W/\""), "{:?}", output.headers);
    let script = output.body_text();
    assert!(
        script.contains("(m.qvtStatic=m.qvtStatic||{})[\"/Items\"]=function(Vue){"),
        "{script}"
    );
    assert!(
        script.contains("_resolveComponent(\"m-form\")") && script.contains("m_static_form_list"),
        "{script}"
    );
    // the compiler wrote a render function over _ctx: no with, no new Function, no eval
    for banned in ["with (", "with(", "new Function", "eval("] {
        assert!(!script.contains(banned), "{banned} in {script}");
    }
    // nothing the actions made, and nothing of the request, is in the template or in the script
    let html = html_of(context.factory(), "Items", "/Items");
    for text in [&script, &html] {
        for data in ["ZEBRA", "ada.lovelace", "always-ran", "1234"] {
            assert!(!text.contains(data), "{data} in {text}");
        }
    }
    golden("Items.script.js", &script);
    golden("Items.template.html", &html);
    assert!(html.contains("<m-static-form-list name=\"ItemList\""), "{html}");
    assert!(html.contains("v-for=\"(tag, tag_index) in (tags)\""), "{html}");
    assert!(html.contains("<template v-if=\"(item != null)\">"), "{html}");
}

/// A server-static anchor uses the alias screen path. The transition path stays available for a submit. The parameters
/// the target gives a link are in the template as scope reads under the link's own, as the server render reads them.
#[test]
fn an_alias_transition_link_bakes_the_screen_path() {
    let context = ExecutionContextFactory::test_context();
    let output = render(context.factory(), "/Jump.qvt", &[]);
    assert_eq!(output.status, 200, "{}", output.body_text());
    assert_eq!(
        header(&output, "X-Moqui-Qvt"),
        "static-template",
        "{}",
        output.body_text()
    );
    let html = html_of(context.factory(), "Jump", "/Jump");
    assert!(
        html.contains("$mq.url('/Land',{},$mq.merge({'landId':$scope['landId']},null))"),
        "{html}"
    );
    assert!(
        html.contains("$mq.url('/Jump/open',{},$mq.merge({'jumpId':$scope['jumpId']},null))"),
        "{html}"
    );
    assert!(
        html.contains("$mq.url('/Jump/save',{},$mq.merge({'jumpId':$scope['jumpId']},null))"),
        "{html}"
    );
    assert!(html.contains(":href=\"'/Land'\""), "{html}");
    assert!(html.contains("$mq.url('/Land',{'landId':"), "{html}");
}

#[test]
fn every_request_gets_the_same_cached_template() {
    let context = ExecutionContextFactory::test_context();
    let first = render(context.factory(), "/Items.qvt", &[("itemId", "one")]);
    let second = render(
        context.factory(),
        "/Items.qvt?x=2",
        &[("itemId", "two"), ("other", "y")],
    );
    assert_eq!(first.body_text(), second.body_text());
    assert_eq!(first.header("ETag"), second.header("ETag"));
}

#[test]
fn a_screen_not_marked_for_qvt_stays_a_server_merge() {
    let context = ExecutionContextFactory::test_context();
    let output = render(context.factory(), "/Plain.qvt", &[]);
    assert_eq!(header(&output, "X-Moqui-Qvt"), "merged-fragment");
    assert_eq!(header(&output, "Cache-Control"), "private, no-store");
    assert!(
        output.body_text().contains("PLAIN-MERGED-VALUE"),
        "{}",
        output.body_text()
    );
    // marked, but dynamic: no template and no static cache header, with the reason
    let output = render(context.factory(), "/Dynamic.qvt", &[]);
    assert_eq!(header(&output, "X-Moqui-Qvt"), "merged-fragment");
    assert_eq!(header(&output, "Cache-Control"), "private, no-store");
    let location = format!(
        "{}/src/screen/testdata/Static/Static/Dynamic.xml",
        env!("CARGO_MANIFEST_DIR")
    );
    let facade = ScreenFacade::of(context.factory());
    let def = facade.screen_def(context.factory(), &location).unwrap();
    let eligibility = crate::screen::statics::eligibility_of(
        &facade,
        context.factory(),
        &def,
        "framework/template/screen/ScreenQvt.js",
        false,
    );
    assert!(!eligibility.eligible && eligibility.marked, "{eligibility:?}");
    assert!(
        eligibility.reasons.iter().any(|reason| reason.contains("dynamic")),
        "{eligibility:?}"
    );
    assert!(
        eligibility
            .reasons
            .iter()
            .any(|reason| reason.contains("extends an expression")),
        "{eligibility:?}"
    );
}

fn json_of(output: &RenderOutput) -> serde_json::Value {
    serde_json::from_slice(&output.body).unwrap_or_else(|err| panic!("{err}: {}", output.body_text()))
}

#[test]
fn actions_answers_the_package_without_the_lists_and_with_plain_values() {
    let context = ExecutionContextFactory::test_context();
    let output = render(
        context.factory(),
        "/Items/actions",
        &[("password", "ZEBRA-secret"), ("keep", "yes")],
    );
    assert_eq!(output.status, 200, "{}", output.body_text());
    assert_eq!(header(&output, "Cache-Control"), "private, no-store");
    assert_eq!(header(&output, "Content-Type"), "application/json");
    let package = json_of(&output);
    assert_eq!(package["greeting"], "ZEBRA greeting for Ada Lovelace");
    assert_eq!(package["item"]["itemId"], "ZEBRA-I1");
    assert_eq!(package["tags"][1], "ZEBRA-t2");
    // always-actions ran, a decimal is a string, the request's own parameters are there, and a credential is not
    assert_eq!(package["alwaysTag"], "always-ran");
    assert_eq!(package["amount"], "1234.50");
    assert_eq!(package["keep"], "yes");
    assert!(package.get("password").is_none(), "{package}");
    assert!(package["item"].get("currentPassword").is_none(), "{package}");
    // the list of the form-list is a request of its own
    assert!(package.get("itemList").is_none(), "{package}");
    golden("Items.package.json", &String::from_utf8(output.body.clone()).unwrap());
    // the context holds nothing the browser cannot use: no ec, sri, finds
    for dropped in ["ec", "sri", "context"] {
        assert!(package.get(dropped).is_none(), "{dropped} in {package}");
    }
}

#[test]
fn a_form_request_answers_its_rows_and_page_keys() {
    let context = ExecutionContextFactory::test_context();
    let output = render(context.factory(), "/Items/actions/ItemList", &[]);
    assert_eq!(output.status, 200, "{}", output.body_text());
    assert_eq!(header(&output, "Cache-Control"), "private, no-store");
    let body = json_of(&output);
    let rows = body["itemList"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{body}");
    assert_eq!(rows[0]["itemId"], "ZEBRA-R1");
    assert_eq!(rows[1]["name"], "ZEBRA row two");
    // the body is the context-shaped map, not a bare array
    assert!(body.is_object());
    golden("Items.ItemList.json", &output.body_text());
    // the multi form-list of the same list answers the same rows
    let multi = render(context.factory(), "/Items/actions/ItemMulti", &[]);
    assert_eq!(multi.status, 200, "{}", multi.body_text());
    assert_eq!(json_of(&multi)["itemList"].as_array().map(Vec::len), Some(2));
    golden("Items.ItemMulti.json", &multi.body_text());
    // a form that is not a list, and a name that is not a form
    let single = render(context.factory(), "/Items/actions/ItemForm", &[]);
    assert_eq!(single.status, 400, "{}", single.body_text());
    let missing = render(context.factory(), "/Items/actions/NoSuch", &[]);
    assert_eq!(missing.status, 404, "{}", missing.body_text());
}

fn render_fixture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/script/testdata/screen/qvt/render")
}

/// The static template of a fixture screen of the qvt render tests, as the report makes it: the screen is not marked, so it is forced.
fn static_fixture(name: &str) -> (String, crate::screen::statics::StaticReport) {
    let (html, _, report) = static_fixture_compiled(name);
    (html, report)
}

/// [`static_fixture`] with the render function the server compiled from the template.
fn static_fixture_compiled(name: &str) -> (String, String, crate::screen::statics::StaticReport) {
    let context = ExecutionContextFactory::test_context();
    let guard = crate::screen::statics::ReportGuard::start(true);
    let location = render_fixture_dir()
        .join(format!("{name}.xml"))
        .to_string_lossy()
        .into_owned();
    make_render(context.factory())
        .root_screen(location.clone())
        .screen_path("")
        .render_mode("qvt")
        .render()
        .unwrap_or_else(|err| panic!("{name}: {err}"));
    let report = guard.finish();
    let def = ScreenFacade::of(context.factory())
        .screen_def(context.factory(), &location)
        .unwrap();
    let template = def
        .generated()
        .static_template("")
        .unwrap_or_else(|| panic!("{name} made no template: {report:?}"));
    (template.html.clone(), template.script.clone(), report)
}

#[test]
fn the_qvt_fixture_screens_become_templates_that_compile() {
    for name in [
        "containers",
        "fields",
        "lists",
        "extras",
        "braces",
        "shell",
        "Dashboard",
        "records",
        "split",
        "panels",
        "cells",
        "status",
        "mobile",
        "views",
        "gated",
    ] {
        let (html, report) = static_fixture(name);
        if name == "fields" {
            assert!(html.contains(r#"<div class="q-ma-sm invoice-field"><m-text-line"#), "{html}");
            assert!(html.contains(r#":class="'q-ma-sm '+ec.resource.expandNoL10n("#), "{html}");
            assert!(html.contains(r#"class="q-my-sm big-row""#), "{html}");
            assert!(html.contains(r#"class="big-row-item""#), "{html}");
        }
        if name == "cells" {
            assert!(html.contains(r#"<div class="q-my-md"><m-form name="RefuseForm""#), "{html}");
            assert!(html.contains(r#"<div class="q-ma-sm"><m-text-area name="reason""#), "{html}");
        }
        golden(&format!("render/{name}.html"), &html);
        eprintln!("{}", report.render());
    }
}

/// `columns type="mobile"` in a server-static screen: the template is cached for every request, so it holds the list of each column
/// set and the client shows the one of the `_uiType` the request carries (the screen package has it), as the merged render picks it.
#[test]
fn mobile_columns_of_a_static_template_are_chosen_by_the_ui_type_in_the_client() {
    let (html, _) = static_fixture("mobile");
    let mobile_at = html
        .find(r#"<template v-if="_uiType === 'mobile'">"#)
        .unwrap_or_else(|| panic!("{html}"));
    let default_at = html.find(r#"<template v-else>"#).unwrap_or_else(|| panic!("{html}"));
    assert!(mobile_at < default_at, "{html}");
    assert_eq!(html.matches("<m-static-form-list").count(), 2, "{html}");
    // the phone's list puts the title and the caption in one cell: fewer header cells than the default's column per field
    let (phone, desktop) = html.split_at(default_at);
    let cells = |text: &str| text.matches("class=\"th").count();
    assert!(cells(phone) < cells(desktop), "{html}");
}

/// A number-card of a server-static screen is bound over the scope as a label is: the value and delta expressions are
/// the scope's, the title localized, the link the screen's path with its query, and a condition a v-if. A link url with
/// `${}` (a card's or a link's) is bound too and resolved against the screen's path in the browser (`$mq.link`).
#[test]
fn a_number_card_is_bound_over_the_scope_in_a_static_template() {
    let (html, _) = static_fixture("Dashboard");
    for expected in [
        r#"<div id="Counters" class="m-dashboard counters" style="--m-dashboard-min:160px"><div class="m-dashboard-item">"#,
        r#":value="ec.resource.expandNoL10n('${openCount}',{'openCount':()=&gt;(openCount)})""#,
        r#":title="ec.l10n.localize('Open orders')""#,
        r#"<template v-if="(showHidden)"><m-number-card"#,
        // the query of the url is kept, over a parameter of the target screen from the scope (a link's too)
        r#":href="$mq.url('/Orders',{'statusId':'Open'},$mq.merge({'statusId':$scope['statusId']},null))""#,
        r#"<m-link :href="$mq.url('/Orders',{'statusId':'Open','sort':'-date'},$mq.merge({'statusId':$scope['statusId']},null))""#,
    ] {
        assert!(html.contains(expected), "{expected} in {html}");
    }
    assert!(!html.contains(">42<") && !html.contains("value=\"42\""), "{html}");
    // a ${} url is bound over the scope and resolved against the screen in the browser, not written as its text
    for expected in [
        r#":href="$mq.link(ec.resource.expandNoL10n('Orders?statusId=${orderStatus}',{'orderStatus':()=&gt;(orderStatus)}),'/')""#,
        r#"<m-link :href="$mq.url($mq.link(ec.resource.expandNoL10n('${\'Orders\'}',{'\'Orders\'':()=&gt;('Orders')}),'/'),{'sort':"#,
    ] {
        assert!(html.contains(expected), "{expected} in {html}");
    }
    assert!(!html.contains("$mq.url('${"), "{html}");
}

/// The screen context of a server-static screen is bound in the browser over the screen's data (R18): each `${}` of the
/// record id and title is an expression of the scope, the title localized, and nothing is expanded on the server.
#[test]
fn the_screen_context_is_bound_over_the_scope_in_a_static_template() {
    let (html, _) = static_fixture("shell");
    assert!(
        html.contains(r#"<m-screen-context record-entity="mantle.order.OrderHeader" :record-id="ec.resource.expandNoL10n('${orderId}',{'orderId':()=&gt;(orderId)})" :title="ec.resource.expand('${orderName}',{'orderName':()=&gt;(orderName)})" selection-form="ItemList"></m-screen-context>"#),
        "{html}"
    );
    assert!(!html.contains("SO10023") && !html.contains("quick-create"), "{html}");
}

/// The record panels of a server-static screen are bound over the scope (R18): the record id and the hints are expressions of
/// the screen's data, a condition a v-if, and nothing is expanded on the server; a rich editor's mention and record transitions are its paths.
#[test]
fn the_record_panels_are_bound_over_the_scope_in_a_static_template() {
    let (html, _) = static_fixture("records");
    for expected in [
        r#"<m-chatter id="TaskChatter" record-entity="mantle.work.effort.WorkEffort" :record-id="ec.resource.expandNoL10n('${taskId}',{'taskId':()=&gt;(taskId)})" :default-tab="ec.resource.expandNoL10n('${tab}',{'tab':()=&gt;(tab)})" hide="message"></m-chatter>"#,
        r#"<template v-if="(false)"><m-chatter"#,
        r#"<m-record-files record-entity="mantle.work.effort.WorkEffort" :record-id="ec.resource.expandNoL10n('${taskId}',{'taskId':()=&gt;(taskId)})" class="q-mt-sm"></m-record-files>"#,
        r#"editor-type="rich-light""#,
        r#"mention-url="/mentions""#,
        r#"record-url="/records""#,
    ] {
        assert!(html.contains(expected), "{expected} in {html}");
    }
    assert!(!html.contains("T100"), "{html}");
}

/// An html-view of a server-static screen is bound over the scope: the html and text are expressions of the screen's data,
/// allow-remote and cid-map expressions, the condition a v-if; no message text is in the template.
#[test]
fn the_html_view_is_bound_over_the_scope_in_a_static_template() {
    let (html, _) = static_fixture("htmlview");
    for expected in [
        r#"<m-html-view id="Body" :html="ec.resource.expandNoL10n('${body}',{'body':()=&gt;(body)})" :allow-remote="!!(trusted)" :cid-map="(cids)" class="q-mt-sm"></m-html-view>"#,
        r#"<m-html-view :text="ec.resource.expandNoL10n('${plain}',{'plain':()=&gt;(plain)})" :collapse-quotes="false" dark="off"></m-html-view>"#,
        r#"<template v-if="(false)"><m-html-view"#,
    ] {
        assert!(html.contains(expected), "{expected} in {html}");
    }
    assert!(!html.contains("{{ada}}") && !html.contains("alert(1)"), "{html}");
}

/// A code-view of a server-static screen is bound over the scope: the text and file name are expressions of the screen's data,
/// the condition a v-if; no code is in the template.
#[test]
fn the_code_view_is_bound_over_the_scope_in_a_static_template() {
    let (html, _) = static_fixture("codeview");
    for expected in [
        r#"<m-code id="Source" :text="ec.resource.expandNoL10n('${source}',{'source':()=&gt;(source)})" :file-name="ec.resource.expandNoL10n('${path}',{'path':()=&gt;(path)})" highlight-lines="2" class="q-mt-sm"></m-code>"#,
        r#"<m-code :text="ec.resource.expandNoL10n('${source}',{'source':()=&gt;(source)})" language="rust" :line-numbers="false"></m-code>"#,
        r#"<template v-if="(false)"><m-code"#,
    ] {
        assert!(html.contains(expected), "{expected} in {html}");
    }
    assert!(!html.contains("{{x}}"), "{html}");
}

/// A split of a server-static screen is the same m-container-split, its panes the side and main slots, the selection read from
/// the URL in the browser: nothing in it is request data. A wizard or stack panel reads its steps from the menu data.
#[test]
fn splits_wizards_and_stacks_hold_no_request_data_in_a_static_template() {
    let (html, _) = static_fixture("split");
    for expected in [
        r#"<m-container-split id="ContactSplit" side-width="300" min-main="520" select-parameter="partyId" :back-text="ec.l10n.localize('All contacts')" :class="ec.resource.expandNoL10n('${paneStyle}',{'paneStyle':()=&gt;(paneStyle)})"><template v-slot:side>"#,
        r#"</template><template v-slot:main>"#,
        r#"<m-container-split height="calc(100vh - 160px)" side-position="right"><template v-slot:side>"#,
    ] {
        assert!(html.contains(expected), "{expected} in {html}");
    }
    let (panels, _) = static_fixture("panels");
    assert!(
        panels.contains(
            r#"<m-subscreens-wizard id="ImportWizard" :finish-text="ec.l10n.localize('Import')"></m-subscreens-wizard>"#
        ),
        "{panels}"
    );
    assert!(
        panels.contains(r#"<m-subscreens-stack id="Pages"></m-subscreens-stack>"#),
        "{panels}"
    );
}

#[test]
fn entity_screens_look_up_display_options_and_sets_on_the_server_and_page_by_the_request() {
    use crate::entity::EntityFacade;
    let world = super::World::open("");
    world.enumeration_type();
    let make = |facade: &EntityFacade, id: &str, sequence: i64| {
        let mut row = facade.make_value("moqui.basic.Enumeration").unwrap();
        row.set("enumId", Value::string(id));
        row.set("enumTypeId", Value::string("TestType"));
        row.set("description", Value::string(format!("Desc {id}")));
        row.set("sequenceNum", Value::I64(sequence));
        row.create(facade).unwrap();
    };
    for (index, id) in ["E1", "E2", "E3", "E4", "E5"].iter().enumerate() {
        make(&world.facade, id, i64::try_from(index).unwrap());
    }
    // the screen package: the form-single's extras hold the looked-up text, the option list and the field the set made; the list and its page keys are not in it
    let screen = render(&world.factory, "/Enums/actions", &[("enumTypeId", "TestType")]);
    assert_eq!(screen.status, 200, "{}", screen.body_text());
    let package = json_of(&screen);
    let extras = &package["mqExtra_TypeForm"];
    assert_eq!(extras["typeText_display"], "Type: Test", "{package}");
    assert_eq!(extras["shown"], "TestType-set", "{package}");
    let options = extras["mqOpt_TypeForm_pick_0_0"]
        .as_array()
        .unwrap_or_else(|| panic!("{package}"));
    assert_eq!(options[0]["value"], "TestType");
    assert_eq!(options[0]["label"], "Test");
    assert!(
        package.get("enumList").is_none() && package.get("enumListCount").is_none(),
        "{package}"
    );
    // the form request: rows with their lookups, and the page keys the find set, which the request's page and size choose
    let page = render(
        &world.factory,
        "/Enums/actions/EnumList",
        &[("enumTypeId", "TestType"), ("pageSize", "2"), ("pageIndex", "1")],
    );
    assert_eq!(page.status, 200, "{}", page.body_text());
    let body = json_of(&page);
    let rows = body["enumList"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{body}");
    assert_eq!(rows[0]["enumId"], "E3");
    assert_eq!(rows[0]["upper"], "DESC E3");
    assert_eq!(rows[0]["enumTypeId_display"], "Test");
    assert_eq!(body["enumListCount"], 5, "{body}");
    assert_eq!(body["enumListPageIndex"], 1);
    assert_eq!(body["enumListPageSize"], 2);
    assert_eq!(body["enumListPageMaxIndex"], 2);
    assert_eq!(body["enumListPageRangeLow"], 3);
    assert_eq!(body["enumListPageRangeHigh"], 4);
    assert_eq!(header(&page, "X-Total-Count"), "5");
    assert_eq!(header(&page, "X-Page-Max-Index"), "2");
    // the template reads the page keys of the list from the form scope
    let template = render(&world.factory, "/Enums.qvt", &[]);
    assert_eq!(header(&template, "X-Moqui-Qvt"), "static-template");
    assert!(
        template.body_text().contains("_ctx.enumListCount"),
        "{}",
        template.body_text()
    );
}

fn report(
    factory: &std::sync::Arc<ExecutionContextFactory>,
    screen: &str,
    full: bool,
) -> crate::screen::statics::StaticReport {
    crate::screen::statics::static_report(factory, &root(), &format!("/{screen}"), &BTreeMap::new(), full).unwrap()
}

#[test]
fn a_screen_with_nothing_for_the_browser_to_refuse_has_a_clean_report() {
    let context = ExecutionContextFactory::test_context();
    let half = report(context.factory(), "Clean", false);
    assert!(!half.is_clean(), "{}", half.render());
    assert!(
        half.marked && half.eligible && half.template_bytes > 0 && !half.full_check,
        "{}",
        half.render()
    );
    assert!(half.render().contains("verdict static half only"), "{}", half.render());
    assert!(!half.render().contains("verdict clean"), "{}", half.render());
    let full = report(context.factory(), "Clean", true);
    assert!(full.is_clean() && full.full_check, "{}", full.render());
    assert!(full.render().contains("verdict clean"), "{}", full.render());
}

/// Hidden parameters, pass-through, and all-checked are expressions over the scope. The package holds the pass-through
/// map, including when it is empty, and a credential name inside it is stripped. A screen that does not use the attribute
/// does not get the key.
#[test]
fn hidden_parameters_pass_through_and_all_checked_read_the_scope() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    let output = render(factory, "/Pass.qvt", &[]);
    assert_eq!(output.status, 200, "{}", output.body_text());
    assert_eq!(header(&output, "X-Moqui-Qvt"), "static-template");
    let html = html_of(factory, "Pass", "/Pass");
    assert!(
        html.contains("$mq.merge(($scope['mqPassThrough']||{}),{'partyId':$scope['partyId'],'fromStatus':(statusId)},"),
        "{html}"
    );
    assert!(html.contains("'moquiFormName':'EditForm'"), "{html}");
    assert!(
        html.contains("([{value:'A',label:ec.l10n.localize('A')},{value:'B',label:ec.l10n.localize('B')}]).map((item) =&gt; item.value)"),
        "{html}"
    );
    assert!(
        html.contains(":hidden-initial=\"(_ctx) =&gt; ({'partyId':$scope['partyId']})\""),
        "{html}"
    );
    assert!(
        html.contains(":row-initial=\"(_ctx) =&gt; ({'rowId':$mq.str($scope['rowId'])})\""),
        "{html}"
    );
    assert!(
        html.contains("$mq.merge({'partyId':$scope['partyId']},{'rowId':$mq.str($scope['rowId']),'moquiFormName':'SingleRows','pageIndex':$mq.str($scope['pageIndex']||0)})"),
        "{html}"
    );
    assert!(
        html.contains("$mq.merge({'partyId':$scope['partyId']},{'rowId':$mq.str($scope['rowId']),'rowId_op':($scope['rowId_op']||'contains')"),
        "{html}"
    );
    // url="." of the transition is this screen, so the path is /Pass. Pass-through is the parameter
    // argument of $mq.url, which wins over the screen parameters in the map argument.
    assert!(
        html.contains("$mq.url('/Pass',$mq.merge({},($scope['mqPassThrough']||{})),"),
        "{html}"
    );
    let plain_at = html.find("Plain").expect("plain link");
    let plain_href = &html[html[..plain_at].rfind("<m-link").expect("plain m-link")..plain_at];
    assert!(
        plain_href.contains("$mq.url('/Pass',{},") && !plain_href.contains("mqPassThrough"),
        "{plain_href}"
    );
    // the row form's initial is not the pass-through map, and the multi row-initial does not carry the hidden name
    let row_initial = html
        .split(":row-initial=\"")
        .nth(1)
        .unwrap_or("")
        .split('"')
        .next()
        .unwrap_or("");
    assert!(
        !row_initial.contains("partyId") && !row_initial.contains("mqPassThrough"),
        "{row_initial}"
    );

    let empty = json_of(&render(factory, "/Pass/actions", &[]));
    assert_eq!(empty["mqPassThrough"], serde_json::json!({}), "{empty}");
    let filled = json_of(&render(
        factory,
        "/Pass/actions",
        &[("partyId", "P1"), ("password", "secret"), ("moquiSessionToken", "tok")],
    ));
    assert_eq!(filled["mqPassThrough"]["partyId"], "P1", "{filled}");
    assert!(filled["mqPassThrough"].get("password").is_none(), "{filled}");
    assert!(filled["mqPassThrough"].get("moquiSessionToken").is_none(), "{filled}");
    assert!(filled.get("password").is_none(), "{filled}");
    let clean = json_of(&render(factory, "/Clean/actions", &[]));
    assert!(clean.get("mqPassThrough").is_none(), "{clean}");
}

/// A link in an included screen is enough for the package key. The including screen's own widgets do not use the attribute.
#[test]
fn pass_through_on_an_included_screen_is_on_the_package() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    let dir = std::env::temp_dir().join(format!("moqui-static-pass-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    let part = dir.join("Part.xml");
    let part_location = part.to_string_lossy().into_owned();
    std::fs::write(
        &part,
        r#"<screen require-authentication="false">
            <parameter name="partyId"/>
            <transition name="go"><default-response url="."/></transition>
            <widgets><link url="go" text="Included" pass-through-parameters="true"/></widgets>
        </screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Root.xml"),
        "<screen require-authentication=\"false\"><widgets><subscreens-active/></widgets><subscreens default-item=\"Holder\"/></screen>",
    )
    .unwrap();
    std::fs::write(
        dir.join("Root/Holder.xml"),
        format!(
            "<screen require-authentication=\"false\" server-static=\"qvt\"><parameter name=\"partyId\"/>\
             <widgets><include-screen location=\"{}\"/><label text=\"holder\"/></widgets></screen>",
            part_location
        ),
    )
    .unwrap();
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let package = make_render(factory)
        .root_screen(root)
        .screen_path("/Holder/actions")
        .parameters({
            let mut map = BTreeMap::new();
            map.insert("partyId".to_string(), Value::string("P9"));
            map
        })
        .render()
        .unwrap();
    assert_eq!(package.status, 200, "{}", package.body_text());
    let json = json_of(&package);
    assert_eq!(json["mqPassThrough"]["partyId"], "P9", "{json}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A static send is the screen the user got, so it is recorded. `make_render` does not enforce request checks, so it does not.
#[test]
fn a_static_screen_is_recorded_in_the_screen_history() {
    // World sets the session cookie key and holds its lock. A second lock on this thread would deadlock.
    // The cookie that comes back starts a visit, which needs an entity facade. The in-memory one is enough.
    let world = super::World::open("");
    let mut request = crate::web::WebRequest::new("GET", "/Clean.qvt");
    request.peer_ip = "127.0.0.1".to_string();
    request.query.push(("keep".to_string(), "1".to_string()));
    let response = crate::screen::serve_at(&world.factory, request, &root());
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(response.header("X-Moqui-Qvt"), Some("static-template"));
    let cookie = response.header("Set-Cookie").expect("session cookie");
    let value = cookie
        .split(';')
        .next()
        .expect("pair")
        .split_once('=')
        .expect("value")
        .1;
    let context = world.factory.new_context();
    let entered = context.enter();
    let guard = crate::user::bind(&world.factory, Some(value), None, false).expect("bind");
    let history = crate::user::screen_history().expect("history");
    drop(guard);
    drop(entered);
    assert_eq!(history, vec!["/Clean?keep=1".to_string()], "{history:?}");
}

#[test]
fn the_report_names_what_a_browser_cannot_do_and_what_the_form_request_repeats() {
    let world = super::World::open("");
    world.enumeration_type();
    let report = report(&world.factory, "Report", true);
    let text = report.render();
    assert!(!report.is_clean() && report.eligible && report.full_check, "{text}");
    let has = |kind: &str, needle: &str| {
        report.violations.iter().any(|violation| {
            violation.kind == kind && format!("{} {}", violation.element, violation.detail).contains(needle)
        })
    };
    // the static walk of the expressions the macros wrote
    assert!(has("ec", "ec.entity.find"), "{text}");
    assert!(has("decimal", "amount.add"), "{text}");
    assert!(has("sri", "sri.getScreenUrlInstance"), "{text}");
    // the full check: the same widget expression ran with the wrapped ec and was recorded, and the render went on
    assert!(has("wrapped-ec", "ec.entity"), "{text}");
    assert!(!has("full-check", ""), "the render itself did not fail: {text}");
    // always-actions and pre-actions with side effects, which the form request repeats
    assert!(has("side-effect", "always-actions/message"), "{text}");
    assert!(has("side-effect", "pre-actions/entity-make-value"), "{text}");
    // a service call that writes is one; a get, find or setup service (or one declared read-only) only reads
    assert!(
        has("side-effect", "service-call moqui.test.WriteServices.update#Counts"),
        "{text}"
    );
    assert!(!has("side-effect", "get#Counts"), "{text}");
    // the list of a form-list in a section-iterate and a label that reads its page keys
    assert!(has("form-list-key", "iterates enumList"), "{text}");
    assert!(has("form-list-key", "enumListCount"), "{text}");
    // a condition outside the form-list that reads its list
    assert!(
        has("form-list-key", "section@NoEnums@condition reads enumList"),
        "{text}"
    );
    // a form-list inside a section-iterate is one form request, outside the iteration
    assert!(has("form-list-iterate", "form-list@GroupRows"), "{text}");
    assert!(!has("form-list-iterate", "EnumList"), "{text}");
    // a content screen marked for html is a stale-cache bug
    assert!(has("stale-cache", "server-static names html"), "{text}");
    // the report is one block: location, mode, verdict, and each violation with its screen and element
    assert!(text.starts_with("screen "), "{text}");
    assert!(
        text.contains("  mode qvt\n") && text.contains("verdict violations"),
        "{text}"
    );
    assert!(text.contains("violation [ec] Report.xml label@text"), "{text}");
    let kinds = report.kinds();
    for kind in [
        "decimal",
        "ec",
        "form-list-key",
        "side-effect",
        "sri",
        "stale-cache",
        "wrapped-ec",
    ] {
        assert!(kinds.iter().any(|found| found == kind), "{kind} in {kinds:?}");
    }
}

#[test]
fn a_screen_that_stays_a_server_merge_says_why_and_has_no_template() {
    let context = ExecutionContextFactory::test_context();
    let report = report(context.factory(), "Dynamic", false);
    assert!(
        !report.eligible && report.marked && report.template_bytes == 0,
        "{}",
        report.render()
    );
    assert!(
        report
            .reasons
            .iter()
            .any(|reason| reason.contains("extends an expression")),
        "{}",
        report.render()
    );
    assert!(
        report.render().contains("verdict stays a server merge"),
        "{}",
        report.render()
    );
}

fn eligibility(factory: &std::sync::Arc<ExecutionContextFactory>, screen: &str) -> crate::screen::statics::Eligibility {
    let location = format!(
        "{}/src/screen/testdata/Static/Static/{screen}.xml",
        env!("CARGO_MANIFEST_DIR")
    );
    let facade = ScreenFacade::of(factory);
    let def = facade.screen_def(factory, &location).unwrap();
    (*crate::screen::statics::eligibility_of(&facade, factory, &def, "framework/template/screen/ScreenQvt.js", false))
        .clone()
}

#[test]
fn server_static_names_qvt_through_a_list_all_the_automatic_case_and_the_forms() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    // the list "qvt,html" and "all" mark it, and the subscreens-only screen is marked by the schema's own rule
    assert!(eligibility(factory, "Items").eligible);
    assert!(eligibility(factory, "Report").marked);
    assert!(eligibility(factory, "All").eligible);
    assert!(eligibility(factory, "Auto").eligible);
    // not named: not marked, and the reason is that
    let html_only = eligibility(factory, "HtmlOnly");
    assert!(!html_only.marked && !html_only.eligible, "{html_only:?}");
    assert!(html_only.reasons[0].contains("does not name qvt"), "{html_only:?}");
    // a form that says it is not static for qvt keeps a marked screen a server merge, and a form that says qvt does not mark an unmarked screen
    let out = eligibility(factory, "FormOut");
    assert!(
        out.marked && !out.eligible && out.reasons.iter().any(|reason| reason.contains("leaves out qvt")),
        "{out:?}"
    );
    assert!(!eligibility(factory, "FormIn").eligible);
    // an extra path is lost by the data request
    let extra = eligibility(factory, "ExtraPath");
    assert!(
        !extra.eligible && extra.reasons.iter().any(|reason| reason.contains("allow-extra-path")),
        "{extra:?}"
    );
}

#[test]
fn qvt_of_a_marked_screen_is_a_template_and_of_an_unmarked_one_a_merge_with_the_header_to_say_so() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    for (screen, static_template) in [
        ("All", true),
        ("Auto", true),
        ("HtmlOnly", false),
        ("FormOut", false),
        ("ExtraPath", false),
    ] {
        let output = render(factory, &format!("/{screen}.qvt"), &[]);
        assert_eq!(output.status, 200, "{screen}: {}", output.body_text());
        let expected = if static_template {
            "static-template"
        } else {
            "merged-fragment"
        };
        assert_eq!(header(&output, "X-Moqui-Qvt"), expected, "{screen}");
        let cache = header(&output, "Cache-Control");
        if static_template {
            assert_eq!(cache, SERVER_STATIC_CACHE, "{screen}");
        } else {
            assert_eq!(cache, "private, no-store", "{screen}");
        }
    }
}

#[test]
fn a_mode_other_than_qvt_marked_static_sends_the_cache_header_and_the_automatic_case_does_not() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    // explicit: html gets the header; the automatic subscreens case does not, since an html page holds the navbar of the user
    let marked = render(factory, "/HtmlOnly.html", &[]);
    assert_eq!(header(&marked, "Cache-Control"), SERVER_STATIC_CACHE);
    let all = render(factory, "/All.html", &[]);
    assert_eq!(header(&all, "Cache-Control"), SERVER_STATIC_CACHE);
    let auto = render(factory, "/Auto.html", &[]);
    assert!(auto.header("Cache-Control").is_none(), "{:?}", auto.headers);
    // a screen not marked for the mode has none
    let plain = render(factory, "/Plain.html", &[]);
    assert!(plain.header("Cache-Control").is_none(), "{:?}", plain.headers);
}

fn write_screen(dir: &std::path::Path, field: &str) -> String {
    std::fs::create_dir_all(dir).unwrap();
    let root = dir.join("Root.xml");
    std::fs::write(
        &root,
        "<screen require-authentication=\"false\"><widgets><subscreens-active/></widgets><subscreens default-item=\"Edited\"/></screen>",
    )
    .unwrap();
    let screen = dir.join("Root/Edited.xml");
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    std::fs::write(
        &screen,
        format!(
            "<screen require-authentication=\"false\" server-static=\"qvt\"><widgets>\
             <form-single name=\"F\"><field name=\"{field}\"><default-field><text-line/></default-field></field></form-single>\
             </widgets></screen>"
        ),
    )
    .unwrap();
    // a later modification time, so the cached definition is seen as old
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(&screen)
        .unwrap()
        .set_modified(later)
        .unwrap();
    root.to_string_lossy().into_owned()
}

#[test]
fn the_cached_template_follows_the_resolved_definition() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    crate::screen::ScreenFacade::of(factory).set_check_interval(Some(std::time::Duration::ZERO));
    let dir = std::env::temp_dir().join(format!("moqui-static-identity-{}", std::process::id()));
    let root = write_screen(&dir, "alpha");
    let first = make_render(factory)
        .root_screen(root.clone())
        .screen_path("/Edited.qvt")
        .render()
        .unwrap();
    assert!(first.body_text().contains("alpha"), "{}", first.body_text());
    let tag = header(&first, "ETag").to_string();
    // the same definition gives the same validator
    let again = make_render(factory)
        .root_screen(root.clone())
        .screen_path("/Edited.qvt")
        .render()
        .unwrap();
    assert_eq!(header(&again, "ETag"), tag);
    // an edit of the screen file makes a new definition, a new template, and a new validator
    let root = write_screen(&dir, "beta");
    let edited = make_render(factory)
        .root_screen(root)
        .screen_path("/Edited.qvt")
        .render()
        .unwrap();
    assert!(
        edited.body_text().contains("beta") && !edited.body_text().contains("alpha"),
        "{}",
        edited.body_text()
    );
    assert_ne!(header(&edited, "ETag"), tag);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_template_has_a_validator_and_answers_304_and_head() {
    use crate::web::WebRequest;
    let world = super::World::open("");
    let get = |method: &str, tag: Option<&str>| {
        let mut request = WebRequest::new(method, "/Items.qvt");
        request.peer_ip = "127.0.0.1".to_string();
        if let Some(tag) = tag {
            request.set_header("if-none-match", tag);
        }
        crate::screen::serve_at(&world.factory, request, &root())
    };
    let first = get("GET", None);
    assert_eq!(first.status, 200, "{}", first.body_text());
    let tag = first.header("ETag").expect("a validator").to_string();
    assert_eq!(first.header("X-Moqui-Qvt"), Some("static-template"));
    // the shared headers of a screen response are there too, and the cache header of the template was not replaced by the private one
    assert_eq!(first.header("Cache-Control"), Some(SERVER_STATIC_CACHE));
    assert_eq!(first.header("X-Content-Type-Options"), Some("nosniff"));
    let same = get("GET", Some(&tag));
    assert_eq!(same.status, 304);
    assert!(same.body.is_empty());
    assert_eq!(same.header("ETag"), Some(tag.as_str()));
    let other = get("GET", Some("W/\"nope\""));
    assert_eq!(other.status, 200);
    assert_eq!(other.body, first.body);
    let head = get("HEAD", None);
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert_eq!(head.header("ETag"), Some(tag.as_str()));
    // the data is not cached anywhere
    let mut request = WebRequest::new("GET", "/Items/actions");
    request.peer_ip = "127.0.0.1".to_string();
    let actions = crate::screen::serve_at(&world.factory, request, &root());
    assert_eq!(actions.status, 200, "{}", actions.body_text());
    assert_eq!(actions.header("Cache-Control"), Some("private, no-store"));
    assert!(actions.header("ETag").is_none());
}

/// An editable select of a server-static screen takes the options of its list-options (and entity-options) too: the package has them by
/// the editable's id, and the template adds them to its option elements, for the page and in a form-single.
#[test]
fn an_editable_select_takes_its_list_options_from_the_package() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    let dir = std::env::temp_dir().join(format!("moqui-static-editable-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    std::fs::write(
        dir.join("Root.xml"),
        "<screen require-authentication=\"false\"><widgets><subscreens-active/></widgets><subscreens default-item=\"Kinds\"/></screen>",
    )
    .unwrap();
    std::fs::write(
        dir.join("Root/Kinds.xml"),
        r#"<screen require-authentication="false" server-static="qvt">
            <transition name="saveKind"><default-response type="none"/></transition>
            <actions><set field="kindList" from="[{code: 'X', name: 'Ex'}, {code: 'Y', name: 'Why'}]"/></actions>
            <widgets>
                <editable id="KindPick" text="Why" value="Y" transition="saveKind" widget-type="select">
                    <option key="Z" text="Zed"/><list-options list="kindList" key="${code}" text="${name}"/></editable>
                <form-single name="KindForm" transition="saveKind">
                    <field name="kind"><default-field><editable id="FormKind" text="Ex" transition="saveKind" widget-type="select">
                        <list-options list="kindList" key="${code}" text="${name}"/></editable></default-field></field>
                </form-single>
            </widgets>
        </screen>"#,
    )
    .unwrap();
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let template = make_render(factory)
        .root_screen(root.clone())
        .screen_path("/Kinds.qvt")
        .render()
        .unwrap();
    assert_eq!(
        header(&template, "X-Moqui-Qvt"),
        "static-template",
        "{}",
        template.body_text()
    );
    let script = template.body_text();
    assert!(
        script.contains("mqOpt_editable_KindPick") && script.contains("mqOpt_editable_FormKind"),
        "{script}"
    );
    // the option element is in the template, the looked-up ones are not
    assert!(script.contains("Zed") && !script.contains("kindList"), "{script}");
    let package = json_of(
        &make_render(factory)
            .root_screen(root)
            .screen_path("/Kinds/actions")
            .render()
            .unwrap(),
    );
    assert_eq!(
        package["mqOpt_editable_KindPick"],
        serde_json::json!([{"value": "X", "label": "Ex"}, {"value": "Y", "label": "Why"}]),
        "{package}"
    );
    assert_eq!(
        package["mqExtra_KindForm"]["mqOpt_editable_FormKind"],
        serde_json::json!([{"value": "X", "label": "Ex"}, {"value": "Y", "label": "Why"}]),
        "{package}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn include_screen_and_section_include_are_written_inline_with_their_actions_in_the_package() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    let dir = std::env::temp_dir().join(format!("moqui-static-include-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    let part = format!(
        "{}/src/screen/testdata/Static/Static/Part.xml",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::write(
        dir.join("Root.xml"),
        "<screen require-authentication=\"false\"><widgets><subscreens-active/></widgets><subscreens default-item=\"Includer\"/></screen>",
    )
    .unwrap();
    std::fs::write(
        dir.join("Root/Includer.xml"),
        format!(
            "<screen require-authentication=\"false\" server-static=\"qvt\"><widgets><label text=\"before\"/>\
             <include-screen location=\"{part}\"/><section-include name=\"Shared\" location=\"{part}\"/></widgets></screen>"
        ),
    )
    .unwrap();
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let template = make_render(factory)
        .root_screen(root.clone())
        .screen_path("/Includer.qvt")
        .render()
        .unwrap();
    assert_eq!(
        header(&template, "X-Moqui-Qvt"),
        "static-template",
        "{}",
        template.body_text()
    );
    // the included widgets are in the template, the values are not
    let script = template.body_text();
    assert!(
        !script.contains("PART-ZEBRA-VALUE") && !script.contains("SECTION-ZEBRA"),
        "{script}"
    );
    assert!(
        script.contains("part says ${partValue}") && script.contains("section says ${sectionValue}"),
        "{script}"
    );
    assert!(script.contains("_ctx.partValue"), "{script}");
    // the package has what the actions of the included screen and of its sections made
    let package = make_render(factory)
        .root_screen(root)
        .screen_path("/Includer/actions")
        .render()
        .unwrap();
    let json = json_of(&package);
    assert_eq!(json["partValue"], "PART-ZEBRA-VALUE", "{json}");
    assert_eq!(json["sectionValue"], "SECTION-ZEBRA", "{json}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_template_the_compiler_rejects_leaves_a_clean_merged_render_and_is_not_built_again() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    let dir = std::env::temp_dir().join(format!("moqui-static-broken-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    std::fs::write(
        dir.join("Root.xml"),
        r#"<screen require-authentication="false"><widgets><subscreens-active/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Root/Broken.xml"),
        r#"<screen require-authentication="false" server-static="qvt"><widgets>
            <label text="before"/><render-mode><text type="qvt"><![CDATA[<div><span>not closed</div>]]></text></render-mode>
        </widgets></screen>"#,
    )
    .unwrap();
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let render = || {
        make_render(factory)
            .root_screen(root.clone())
            .screen_path("/Broken.qvt")
            .render()
            .unwrap()
    };
    let first = render();
    assert_eq!(
        header(&first, "X-Moqui-Qvt"),
        "merged-fragment",
        "{}",
        first.body_text()
    );
    // Only the merged render's text: the template that was written before the compiler refused it is gone.
    assert_eq!(first.body_text().matches("before").count(), 1, "{}", first.body_text());
    let def = ScreenFacade::of(factory)
        .screen_def(factory, &dir.join("Root/Broken.xml").to_string_lossy())
        .unwrap();
    assert!(def.generated().static_failure("/Broken").is_some());
    let second = render();
    assert_eq!(header(&second, "X-Moqui-Qvt"), "merged-fragment");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The Parity fixture: the XML screen and form features at parity with Java Moqui, as a template, its package and its form requests.
#[test]
fn the_parity_fixture_is_a_template_with_its_data_in_the_packages() {
    let context = ExecutionContextFactory::test_context();
    let output = render(context.factory(), "/Parity.qvt", &[]);
    assert_eq!(output.status, 200, "{}", output.body_text());
    assert_eq!(header(&output, "X-Moqui-Qvt"), "static-template");
    let html = html_of(context.factory(), "Parity", "/Parity");
    // a condition element is one v-if: and, or, not, compare and expression, with its fail-widgets as the v-else
    assert!(
        html.contains("$mq.compare(status,'equals','ACTIVE',null,null,'Object') &amp;&amp; (qty &gt; 3)"),
        "{html}"
    );
    assert!(html.contains("<template v-else>"), "{html}");
    golden("Parity.script.js", &output.body_text());
    golden("Parity.template.html", &html);
    let package = render(context.factory(), "/Parity/actions", &[]);
    assert_eq!(package.status, 200, "{}", package.body_text());
    golden("Parity.package.json", &package.body_text());
    // a paginated section-iterate: the page keys in the package, and the list is the page
    let json = json_of(&package);
    assert_eq!(json["linesCount"], 25, "{json}");
    assert_eq!(json["lines"].as_array().map(Vec::len), Some(20), "{json}");
    let second = json_of(&render(context.factory(), "/Parity/actions", &[("pageIndex", "1")]));
    assert_eq!(second["lines"][0]["lineId"], "L21", "{second}");
    assert_eq!(second["linesPageRangeLow"], 21, "{second}");
    assert!(
        html.contains("<m-form-paginate v-if=\"$scope['linesCount']\""),
        "{html}"
    );
    // text-map: a name of the text reads the map first
    assert!(html.contains("$mq.inMap((product),'productName')"), "{html}");
    // the accordion: a group of expansion items, the second open; the button-menu: a dropdown of its children
    assert!(html.contains("group=\"ProductForm_accordion\""), "{html}");
    assert!(html.contains("<q-btn-dropdown"), "{html}");
    // the aggregated list: rows grouped by order, each with its sub-list of items and their total
    let orders = render(context.factory(), "/Parity/actions/OrderList", &[]);
    assert_eq!(orders.status, 200, "{}", orders.body_text());
    let orders = json_of(&orders);
    assert_eq!(orders["orderLines"].as_array().map(Vec::len), Some(2), "{orders}");
    assert_eq!(
        orders["orderLines"][0]["aggregateSubList"][1]["item"], "Bolt",
        "{orders}"
    );
    golden("Parity.OrderList.json", &serde_json::to_string(&orders).unwrap());
    assert!(html.contains("v-slot:subrow"), "{html}");
    // first and last rows: the map-first-row map in the form request, and slots of their own in the template
    let edits = json_of(&render(context.factory(), "/Parity/actions/EditList", &[]));
    assert_eq!(edits["itemsForEditFirst"]["itemId"], "NEW", "{edits}");
    assert!(edits.get("itemsForEditLast").is_some(), "{edits}");
    golden("Parity.EditList.json", &serde_json::to_string(&edits).unwrap());
    assert!(
        html.contains("v-slot:row_first") && html.contains("v-slot:row_last"),
        "{html}"
    );
    // row selection: the id field for the checkbox set, a checkbox for each row, and the action form posting through the set
    assert!(
        html.contains("selection-id-field=\"itemId\"")
            && html.contains("mqSel.clickCheckbox(event || {}, rowIndex)")
            && html.contains("mqSel.checkboxStates[rowIndex] = value;"),
        "{html}"
    );
    assert!(html.contains(":parent-checkbox-set=\"mqSelSet\""), "{html}");
    let picks = json_of(&render(context.factory(), "/Parity/actions/PickList", &[]));
    golden("Parity.PickList.json", &serde_json::to_string(&picks).unwrap());
}

/// A status-bar of a server-static screen is bound over the scope (the status, the title, a read-only or list expression)
/// and asks the root screen's statusBar transition for the flow in the browser; a btn-type and row-col sizes that are
/// expressions are bound through `$mq.btnKind` and `$mq.cols`, and an empty editable is hidden unless display-if-empty.
#[test]
fn a_status_bar_and_expression_styles_are_bound_in_a_static_template() {
    let (status, _) = static_fixture("status");
    for expected in [
        r#"<m-status-bar id="SyncStatus" :status-id="ec.resource.expandNoL10n('${statusId}',{'statusId':()=&gt;(statusId)})" action="/setStatus" :parameters="$mq.merge(({entitySyncId: syncId}),{})""#,
        r#":read-only="String(ec.resource.expandNoL10n('${!canUpdate}',{'!canUpdate':()=&gt;(!canUpdate)}))==='true'""#,
        r#"stages="EsNotStarted,EsRunning,EsComplete""#,
        r#":allowed-status-ids="(['EsComplete'])""#,
        r#"parameter-name="toStatusId""#,
    ] {
        assert!(status.contains(expected), "{expected} in {status}");
    }
    assert!(!status.contains(":bar="), "the flow is not in the template: {status}");
    let (cells, _) = static_fixture("cells");
    for expected in [
        r#"v-bind="$mq.btnKind(ec.resource.expandNoL10n('${chip.on ? \'primary\' : \'\'}'"#,
        r#":class="$mq.cols(ec.resource.expandNoL10n('${wide}',{'wide':()=&gt;(wide)}),ec.resource.expandNoL10n('${wide}',{'wide':()=&gt;(wide)}),'','',false)""#,
        r#"value="B" :options="[{value:'A',label:ec.l10n.localize('First')},{value:'B',label:ec.l10n.localize('Second')}]""#,
        r#":placeholder="ec.l10n.localize('Click to edit')""#,
    ] {
        assert!(cells.contains(expected), "{expected} in {cells}");
    }
}

/// A row-selection condition and a view's move-condition in a server-static screen are evaluated by the template over the scope: the
/// list's selection-id-field is bound to the condition (no id field, no selection), the selection cell of a row is there only with the
/// checkbox set and a first row's only when the condition holds; the board's and tree's transition is the path when the move-condition
/// holds, else null. A condition that throws in the browser does not hold. A row-selection with no condition is written as before.
#[test]
fn a_row_selection_condition_and_a_move_condition_are_bound_in_a_static_template() {
    let (html, script, report) = static_fixture_compiled("gated");
    for expected in [
        r#":selection-id-field="(() =&gt; { try { return !!(canBulk); } catch { return false; } })() ? 'taskId' : null""#,
        r#":selection-id-field="(() =&gt; { try { return !!(!canBulk); } catch { return false; } })() ? 'taskId' : null""#,
        r#":selection-id-field="(() =&gt; { try { return !!(noSuchThing.call()); } catch { return false; } })() ? 'taskId' : null""#,
        r#"<div class="td" v-if="mqSel"><q-checkbox"#,
        r#"<div class="tr first"><div class="td" v-if="(() =&gt; { try { return !!(canBulk); } catch { return false; } })()">&nbsp;</div>"#,
        "'transition':((() =&gt; { try { return !!(!canBulk); } catch { return false; } })() ? '/moveTask' : null)",
        "'transition':((() =&gt; { try { return !!(canBulk); } catch { return false; } })() ? '/moveTask' : null)",
    ] {
        assert!(html.contains(expected), "{expected} in {html}");
    }
    assert!(!html.contains(r#" selection-id-field="#), "{html}");
    // the names of a condition are the scope's, and the catch binds no name
    assert!(
        script.contains("_ctx.canBulk") && script.contains("_ctx.noSuchThing.call()") && script.contains("catch {"),
        "{script}"
    );
    let (extras, _) = static_fixture("extras");
    assert!(
        extras.contains(r#" selection-id-field="id""#) && !extras.contains(r#"v-if="mqSel""#),
        "{extras}"
    );
    eprintln!("{}", report.render());
}

/// The views of a form-list in a server-static screen: m-static-form-list gets the board's and calendar's settings (the transitions as
/// paths of the screen, the key fields, color-map) and a card slot over the row's scope with the title, badge and card fields as the
/// row slot writes them (no hidden input for a display); the rows are the ones it fetches. markdown-view is bound over the scope.
#[test]
fn form_list_views_and_the_markdown_view_in_a_static_template() {
    let (html, report) = static_fixture("views");
    for expected in [
        r#":views="['board','list','calendar']""#,
        "'groupField':'stage','titleField':'name','sumField':'points'",
        "'keyFields':['taskId'],'columnOrder':[]",
        "'transition':'/moveTask'",
        "'createTransition':'/newTask'",
        "'colors':{'todo':'info','doing':'warning'}",
        r#"views-location="/#Tasks""#,
        r#"<template v-slot:card="_ctx"><div class="m-card-head row no-wrap"><div class="m-card-title">"#,
        r#"<span class="m-card-label m-muted" v-text="ec.l10n.localize('Owner')"></span>"#,
        r#"<m-markdown id="Notes""#,
        r#":wiki-links="true""#,
    ] {
        assert!(html.contains(expected), "{expected} in {html}");
    }
    let card = &html[html.find(r#"<template v-slot:card="_ctx">"#).unwrap()..];
    let card = &card[..card.find("</template>").unwrap()];
    assert!(!card.contains(r#"type="hidden""#), "no hidden input in a card: {card}");
    assert!(card.contains("Tasks_card_"), "a card's ids are its own: {card}");
    // group-fields with their titles as expressions, all-rows from the form request, a peek-url and the rows' index; the tree's
    // settings and its slot over the row's scope: the row's cells in a form of the row, the first with the tree's lead
    for expected in [
        "'groups':[{'field':'owner','label':ec.l10n.localize('owner')},{'field':'stage','label':ec.l10n.localize('Stage')}]",
        "'allRows':true,'rowsUrl':'/actions/Grouped','listName':'taskList','maxRows':50",
        r#":peek="{'url':'/open','parameters':{},'keyField':'taskId'}""#,
        r#":data-row="rowIndex""#,
        "'parentField':'parentId','keyFields':['nodeId'],'childCountField':'kids','rollups':[{'name':'totalPoints','label':ec.l10n.localize('Total points')}]",
        "'childrenUrl':'/planTask'",
        r#"<template v-slot:tree="_ctx">"#,
        r#"<m-tree-lead :t="mqTree.t"></m-tree-lead>"#,
    ] {
        assert!(html.contains(expected), "{expected} in {html}");
    }
    let tree = &html[html.find(r#"<template v-slot:tree="_ctx">"#).unwrap()..];
    let tree = &tree[..tree.find("</template>").unwrap()];
    assert!(
        tree.contains("<m-form") && !tree.contains(r#"name="totalPoints""#),
        "the roll-up is no cell of the tree row: {tree}"
    );
    eprintln!("{}", report.render());
}
