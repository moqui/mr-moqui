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

//! The `tui` render mode writes one JSON document: an envelope around a root array, and each section is an array.

use std::collections::BTreeMap;

use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::context::ExecutionContextFactory;
use crate::screen::{make_render, RenderOutput};

fn render(label: &str, xml: &str, parameters: BTreeMap<String, Value>) -> RenderOutput {
    let path = std::env::temp_dir().join(format!("moqui-tui-{}-{label}.xml", std::process::id()));
    std::fs::write(&path, xml).unwrap();
    let context = ExecutionContextFactory::test_context();
    let output = make_render(context.factory())
        .root_screen(path.to_string_lossy().into_owned())
        .render_mode("tui")
        .parameters(parameters)
        .render()
        .unwrap_or_else(|err| panic!("{label}: {err}"));
    let _ = std::fs::remove_file(&path);
    output
}

fn document(output: &RenderOutput) -> Json {
    let text = output.body_text();
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("{err}: {text}"))
}

fn objects<'a>(value: &'a Json, found: &mut Vec<&'a serde_json::Map<String, Json>>) {
    match value {
        Json::Array(items) => {
            for item in items {
                objects(item, found);
            }
        }
        Json::Object(map) => {
            found.push(map);
            for item in map.values() {
                objects(item, found);
            }
        }
        Json::Null | Json::Bool(_) | Json::Number(_) | Json::String(_) => {}
    }
}

fn nodes(doc: &Json) -> Vec<&serde_json::Map<String, Json>> {
    let mut found = Vec::new();
    objects(doc, &mut found);
    found
}

fn section<'a>(doc: &'a Json, name: &str) -> &'a serde_json::Map<String, Json> {
    nodes(doc)
        .into_iter()
        .find(|node| {
            node.get("t").and_then(Json::as_str) == Some("section")
                && node.get("name").and_then(Json::as_str) == Some(name)
        })
        .unwrap_or_else(|| panic!("no section {name} in {doc}"))
}

#[test]
fn an_empty_section_a_failed_section_and_a_section_iterate_are_one_document() {
    let output = render(
        "sections",
        r#"<screen require-authentication="false" default-menu-title="Enums"><actions><script>
            ec.message.addMessage("saved ok")
            ec.message.addMessage("watch this", "danger")
        </script></actions><widgets>
            <label text="before"/>
            <section name="Empty"><widgets/></section>
            <section-iterate name="Vacant" list="[]" entry="row"><widgets><label text="nope"/></widgets></section-iterate>
            <section-iterate name="None" list="['a','b']" entry="row"><widgets/></section-iterate>
            <section-iterate name="Each" list="['a','b','c']" entry="row"><widgets><label text="${row}"/></widgets></section-iterate>
            <section name="Broken"><widgets><label text="inside-broken"/>
                <render-mode><text type="tui"><![CDATA[${(() => { throw new Error('boom { brace }') })()}]]></text></render-mode>
            </widgets></section>
            <section name="Checked"><actions><script>
                ec.message.addError("plain problem")
                ec.message.addValidationError("EditForm", "name", "", "Name is required")
            </script></actions><widgets><label text="inside-checked"/></widgets></section>
            <label text="after"/>
        </widgets></screen>"#,
        BTreeMap::new(),
    );
    assert_eq!(output.status, 200);
    assert!(
        output.content_type().unwrap_or("").starts_with("application/json"),
        "{}",
        output.content_type().unwrap_or("")
    );
    let doc = document(&output);
    assert_eq!(doc["v"], json!(1));
    assert_eq!(doc["screen"]["path"], json!("/"));
    assert_eq!(doc["screen"]["pathList"], json!([]));
    assert_eq!(doc["screen"]["title"], json!("Enums"));
    assert_eq!(doc["screen"]["standalone"], json!(0));
    assert_eq!(doc["focus"], json!(""));
    assert_eq!(doc["messages"]["info"], json!(["saved ok"]));
    assert_eq!(doc["messages"]["errors"], json!(["watch this", "plain problem"]));
    assert_eq!(
        doc["messages"]["validationErrors"],
        json!([{"form": "EditForm", "field": "name", "message": "Name is required"}])
    );
    let body = output.body_text();
    assert!(!body.contains("inside-broken"), "{body}");
    assert!(!body.contains("inside-checked"), "{body}");
    assert!(body.contains("before") && body.contains("after"), "{body}");
    assert!(section(&doc, "Empty")["c"].as_array().unwrap().is_empty(), "{doc}");
    assert!(section(&doc, "Vacant")["c"].as_array().unwrap().is_empty(), "{doc}");
    assert!(section(&doc, "None")["c"].as_array().unwrap().is_empty(), "{doc}");
    let each = section(&doc, "Each")["c"].as_array().unwrap();
    assert_eq!(
        each.iter()
            .map(|node| node["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert!(!body.contains("]["), "{body}");
    let broken = section(&doc, "Broken")["c"].as_array().unwrap();
    assert_eq!(broken[0]["t"], json!("error"));
    assert!(broken[0]["text"].as_str().unwrap().contains("boom"), "{broken:?}");
    assert!(broken[0]["text"].as_str().unwrap().contains("brace"), "{broken:?}");
    let checked = section(&doc, "Checked")["c"].as_array().unwrap();
    assert_eq!(checked[0]["t"], json!("error"));
    assert!(
        checked[0]["text"].as_str().unwrap().contains("Name is required"),
        "{checked:?}"
    );
    let model = moqui_tui::ScreenModel::from_json(&body).unwrap_or_else(|err| panic!("{err}: {body}"));
    assert_eq!(model.v, 1);
    assert_eq!(model.screen.title, "Enums");
    assert_eq!(model.messages.info, ["saved ok"]);
    assert_eq!(model.messages.validation_errors[0].message, "Name is required");
}

#[test]
fn a_password_a_credential_parameter_and_a_control_sequence_are_left_out() {
    let mut parameters = BTreeMap::new();
    parameters.insert("password".to_string(), Value::string("s3cret-value"));
    parameters.insert("keep".to_string(), Value::string("kept-visible"));
    parameters.insert("moquiSessionToken".to_string(), Value::string("token-value"));
    let output = render(
        "secret",
        r#"<screen require-authentication="false" default-menu-title="Secret"><actions><script>
            bad = "pre\u001b]52;\u0080post"
        </script></actions><widgets>
            <label text="${bad}"/>
            <form-single name="F"><field name="pw"><default-field><password default-value="s3cret-value"/></default-field></field></form-single>
        </widgets></screen>"#,
        parameters,
    );
    let body = output.body_text();
    assert!(!body.contains("s3cret-value"), "{body}");
    assert!(!body.contains("token-value"), "{body}");
    assert!(!body.as_bytes().contains(&0x1b), "{body}");
    let doc = document(&output);
    assert_eq!(doc["parameters"]["keep"], json!("kept-visible"));
    assert!(doc["parameters"].get("password").is_none(), "{doc}");
    assert!(doc["parameters"].get("moquiSessionToken").is_none(), "{doc}");
    let text = nodes(&doc)
        .into_iter()
        .find(|node| node.get("t").and_then(Json::as_str) == Some("text"))
        .unwrap();
    assert_eq!(text["text"], json!("pre]52;post"));
    let field = nodes(&doc)
        .into_iter()
        .find(|node| node.get("t").and_then(Json::as_str) == Some("field"))
        .unwrap();
    assert_eq!(field["w"]["kind"], json!("password"));
    assert!(field["w"].get("value").is_none(), "{field:?}");
    let model = moqui_tui::ScreenModel::from_json(&body).unwrap_or_else(|err| panic!("{err}: {body}"));
    assert_eq!(model.parameters.get("keep").map(String::as_str), Some("kept-visible"));
    assert!(!model.parameters.contains_key("password"));
}

#[test]
fn a_dialog_a_dependent_drop_down_a_list_and_a_tui_text_are_one_document() {
    let output = render(
        "edit",
        r#"<screen require-authentication="false" default-menu-title="Edit"><actions><script>
            rows = [{id:'r1', title:'One'}]
        </script></actions><widgets>
            <container-dialog id="New" button-text="New" title="Create">
                <label text="inside"/>
                <form-single name="NewForm" transition="create" focus-field="name" background-hide-id="New" submit-message="Saved">
                    <field name="name"><default-field title="Name"><text-line/></default-field></field>
                    <field name="pw"><default-field><password default-value="s3cret-value"/></default-field></field>
                </form-single>
            </container-dialog>
            <form-single name="Find">
                <field name="countryGeoId"><default-field title="Country">
                    <drop-down><option key="US" text="United States"/></drop-down>
                </default-field></field>
                <field name="stateGeoId"><default-field title="State">
                    <drop-down><dynamic-options transition="getGeoCountryStates" value-field="geoId" label-field="geoName">
                        <depends-on field="countryGeoId"/>
                    </dynamic-options></drop-down>
                </default-field></field>
            </form-single>
            <form-list name="Rows" list="rows" id-field="id">
                <field name="id"><default-field><display/></default-field></field>
                <field name="title">
                    <header-field title="Title"><text-find/></header-field>
                    <default-field><link url="/open" text="Open" link-type="anchor" url-type="plain"/></default-field>
                </field>
            </form-list>
            <text type="tui"><![CDATA[[{"t":"text","text":"from-tui"}]]]></text>
            <text type="tui">this is not json</text>
            <render-mode><text type="qvt">only qvt</text></render-mode>
        </widgets></screen>"#,
        BTreeMap::new(),
    );
    let body = output.body_text();
    assert!(!body.contains("s3cret-value"), "{body}");
    assert!(!body.contains("is not written yet"), "{body}");
    let doc = document(&output);
    assert_eq!(doc["focus"], json!("NewForm.name"));
    let dialog = nodes(&doc)
        .into_iter()
        .find(|node| node.get("t").and_then(Json::as_str) == Some("dialog"))
        .unwrap();
    assert_eq!(dialog["button"], json!("New"));
    assert_eq!(dialog["title"], json!("Create"));
    assert!(dialog["c"].to_string().contains("inside"), "{dialog:?}");
    let country = nodes(&doc)
        .into_iter()
        .find(|node| node.get("name").and_then(Json::as_str) == Some("countryGeoId"))
        .unwrap();
    assert_eq!(country["w"]["options"][0]["label"], json!("United States"));
    let state = nodes(&doc)
        .into_iter()
        .find(|node| node.get("name").and_then(Json::as_str) == Some("stateGeoId"))
        .unwrap();
    assert_eq!(state["w"]["dependsOn"], json!({"countryGeoId": "countryGeoId"}));
    assert!(state["w"].get("optionsParameters").is_none(), "{state:?}");
    assert!(state["w"]["options"].as_array().unwrap().is_empty(), "{state:?}");
    assert!(state["w"].get("optionsUrl").is_some(), "{state:?}");
    let list = nodes(&doc)
        .into_iter()
        .find(|node| node.get("t").and_then(Json::as_str) == Some("formList"))
        .unwrap();
    assert_eq!(list["rows"][0]["cells"][0], json!("r1"));
    assert_eq!(list["rows"][0]["cells"][1]["t"], json!("link"));
    assert!(
        list["rows"][0]["cells"][1]["url"].as_str().unwrap().contains("/open"),
        "{list:?}"
    );
    assert_eq!(list["headerFields"][0]["w"]["kind"], json!("textFind"));
    assert_eq!(list["paging"]["count"], json!(1));
    let texts: Vec<&str> = nodes(&doc)
        .iter()
        .filter_map(|node| node.get("text").and_then(Json::as_str))
        .collect();
    assert!(texts.contains(&"from-tui"), "{texts:?}");
    assert!(
        texts
            .iter()
            .any(|text| text.contains("is not JSON") && text.contains("inline")),
        "{texts:?}"
    );
    assert!(nodes(&doc).iter().any(|node| {
        node.get("t").and_then(Json::as_str) == Some("placeholder")
            && node
                .get("url")
                .and_then(Json::as_str)
                .is_some_and(|url| !url.is_empty())
    }));
    moqui_tui::ScreenModel::from_json(&body).unwrap_or_else(|err| panic!("{err}: {body}"));
}

fn ids(doc: &Json) -> Vec<String> {
    nodes(doc)
        .into_iter()
        .filter(|node| node.contains_key("t"))
        .filter_map(|node| node.get("id").and_then(Json::as_str).map(str::to_string))
        .collect()
}

fn first_of<'a>(doc: &'a Json, t: &str) -> &'a serde_json::Map<String, Json> {
    nodes(doc)
        .into_iter()
        .find(|node| node.get("t").and_then(Json::as_str) == Some(t))
        .unwrap_or_else(|| panic!("no {t} in {doc}"))
}

#[test]
fn links_keep_their_parameters_and_options_send_fixed_and_dependent_ones() {
    let output = render(
        "params",
        r#"<screen require-authentication="false"><actions><script>
            rows = [{enumId:'E1'}, {enumId:'E2'}]
        </script></actions><widgets>
            <link url="/apps/Detail" url-type="plain" text="Open"><parameter name="enumId" value="E0"/></link>
            <link url="/apps/Views" url-type="plain" text="Board" parameter-map="{view:'board'}"/>
            <form-list name="Rows" list="rows" id-field="enumId">
                <field name="enumId"><default-field>
                    <link url="/apps/Detail" url-type="plain" text="${enumId}"><parameter name="enumId"/></link>
                </default-field></field>
            </form-list>
            <form-single name="Geo">
                <field name="country"><default-field><text-line/></default-field></field>
                <field name="state"><default-field><drop-down>
                    <dynamic-options transition="/apps/states" value-field="geoId" label-field="geoName"
                            parameter-map="{geoTypeEnumId:'GEOT_STATE'}">
                        <depends-on field="country" parameter="countryGeoId"/>
                    </dynamic-options>
                </drop-down></default-field></field>
            </form-single>
        </widgets></screen>"#,
        BTreeMap::new(),
    );
    let doc = document(&output);
    let urls: Vec<&str> = nodes(&doc)
        .into_iter()
        .filter(|node| node.get("t").and_then(Json::as_str) == Some("link"))
        .filter_map(|node| node.get("url").and_then(Json::as_str))
        .collect();
    assert!(
        urls.iter().any(|url| url.ends_with("/apps/Detail?enumId=E0")),
        "{urls:?}"
    );
    assert!(
        urls.iter().any(|url| url.ends_with("/apps/Views?view=board")),
        "{urls:?}"
    );
    assert!(
        urls.iter().any(|url| url.ends_with("/apps/Detail?enumId=E2")),
        "{urls:?}"
    );
    let list = first_of(&doc, "formList");
    assert_eq!(list["rows"][0]["id"], json!("E1"));
    assert_eq!(list["rows"][1]["id"], json!("E2"));
    let state = nodes(&doc)
        .into_iter()
        .find(|node| node.get("name").and_then(Json::as_str) == Some("state"))
        .unwrap();
    assert_eq!(state["w"]["dependsOn"], json!({"countryGeoId": "country"}));
    assert_eq!(state["w"]["optionsParameters"], json!({"geoTypeEnumId": "GEOT_STATE"}));
    assert!(!state["w"]["optionsUrl"].as_str().unwrap().contains('?'), "{state:?}");
    let model = moqui_tui::ScreenModel::from_json(&output.body_text()).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(model.v, 1);
}

#[test]
fn texts_from_a_location_and_single_node_fragments_are_checked_before_they_are_written() {
    let dir = std::env::temp_dir().join(format!("moqui-tui-texts-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tui = dir.join("Part.tui.json");
    std::fs::write(&tui, r#"{"t":"text","text":"from-file"}"#).unwrap();
    let plain = dir.join("Plain.tui.json");
    std::fs::write(&plain, r#"[{"t":"text","text":"plain ${not expanded}"}]"#).unwrap();
    let html = dir.join("Part.html");
    std::fs::write(&html, "<p>any-from-file</p>").unwrap();
    let xml = format!(
        r#"<screen require-authentication="false"><widgets>
            <render-mode><text type="tui" location="{}"/></render-mode>
            <render-mode><text type="tui" location="{}" template="false"/></render-mode>
            <render-mode><text location="{}"/></render-mode>
            <render-mode><text type="tui"><![CDATA[{{"t":"text","text":"single"}}]]></text></render-mode>
            <render-mode><text type="tui"><![CDATA[[1, 2]]]></text></render-mode>
        </widgets></screen>"#,
        tui.display(),
        plain.display(),
        html.display()
    );
    let output = render("texts", &xml, BTreeMap::new());
    let _ = std::fs::remove_dir_all(&dir);
    let doc = document(&output);
    let texts: Vec<&str> = nodes(&doc)
        .iter()
        .filter_map(|node| node.get("text").and_then(Json::as_str))
        .collect();
    assert!(texts.contains(&"from-file"), "{texts:?}");
    assert!(texts.contains(&"plain ${not expanded}"), "{texts:?}");
    assert!(texts.iter().any(|text| text.contains("any-from-file")), "{texts:?}");
    assert!(texts.contains(&"single"), "{texts:?}");
    assert!(
        texts
            .iter()
            .any(|text| text.contains("is not a node or an array of nodes")),
        "{texts:?}"
    );
    assert!(!texts.iter().any(|text| text.contains("is not JSON")), "{texts:?}");
    moqui_tui::ScreenModel::from_json(&output.body_text()).unwrap_or_else(|err| panic!("{err}"));
}

#[test]
fn node_ids_are_unique_across_sections_includes_and_iterations() {
    let dir = std::env::temp_dir().join(format!("moqui-tui-ids-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let empty = dir.join("Empty.xml");
    std::fs::write(&empty, r#"<screen require-authentication="false"><widgets/></screen>"#).unwrap();
    let other = dir.join("Other.xml");
    std::fs::write(
        &other,
        r#"<screen require-authentication="false"><widgets>
            <section name="First"><widgets><label text="two"/></widgets></section>
        </widgets></screen>"#,
    )
    .unwrap();
    let xml = format!(
        r#"<screen require-authentication="false"><widgets>
            <section name="First"><widgets><label text="one"/></widgets></section>
            <section-include name="First" location="{}"/>
            <section-iterate name="Each" list="['a','b','c']" entry="row"><widgets>
                <label text="${{row}}"/>
                <container-box><box-header title="Box ${{row}}"/><box-body><label text="in"/></box-body></container-box>
            </widgets></section-iterate>
            <include-screen location="{}"/>
            <subscreens-panel id="Pop" type="popup"/>
            <form-list name="Cols" list="[{{a:1}}]" select-columns="true">
                <field name="a"><default-field><display/></default-field></field>
            </form-list>
        </widgets></screen>"#,
        other.display(),
        empty.display()
    );
    let output = render("ids", &xml, BTreeMap::new());
    let _ = std::fs::remove_dir_all(&dir);
    let body = output.body_text();
    let doc = document(&output);
    let found = ids(&doc);
    let mut seen = std::collections::HashSet::new();
    for id in &found {
        assert!(seen.insert(id), "{id} repeated in {found:?}");
    }
    assert!(found.iter().any(|id| id.ends_with("~2")), "{found:?}");
    let include = nodes(&doc)
        .into_iter()
        .find(|node| node.get("name").and_then(Json::as_str) == Some("include"))
        .unwrap_or_else(|| panic!("{doc}"));
    assert_eq!(include["c"], json!([]), "{doc}");
    // A popup panel draws no tabs but still holds the active level, as qvt's m-subscreens-active.
    let popup = first_of(&doc, "subscreens");
    assert_eq!(popup["kind"], json!("popup"));
    assert!(popup["c"].is_array(), "{popup:?}");
    assert!(
        nodes(&doc)
            .iter()
            .any(|node| node.get("t").and_then(Json::as_str) == Some("formList")),
        "{doc}"
    );
    moqui_tui::ScreenModel::from_json(&body).unwrap_or_else(|err| panic!("{err}: {body}"));
}

#[test]
fn every_phase_two_widget_and_region_is_a_node_the_client_reads() {
    let mut parameters = BTreeMap::new();
    parameters.insert("descr_op".to_string(), Value::string("begins"));
    parameters.insert("descr_not".to_string(), Value::string("Y"));
    parameters.insert("amount_from".to_string(), Value::string("5"));
    let output = render(
        "widgets",
        r#"<screen require-authentication="false"><widgets>
            <container-box><box-header title="Order"/><box-toolbar><label text="tool"/></box-toolbar>
                <box-body><label text="body" type="h3"/></box-body></container-box>
            <container-panel><panel-left size="200"><label text="side"/></panel-left><panel-center><label text="mid"/></panel-center></container-panel>
            <container-split side-width="240"><split-side><label text="list"/></split-side><split-main><label text="record"/></split-main></container-split>
            <form-single name="All" transition="save">
                <field name="note"><default-field tooltip="A note"><text-area rows="4" editor-type="md"/></default-field></field>
                <field name="body"><default-field><text-area editor-type="html"/></default-field></field>
                <field name="descr"><default-field><text-find/></default-field></field>
                <field name="when"><default-field><date-find type="date"/></default-field></field>
                <field name="period"><default-field><date-period/></default-field></field>
                <field name="amount"><default-field><range-find/></default-field></field>
                <field name="upload"><default-field><file multiple="true"/></default-field></field>
                <field name="shown"><default-field><display text="Shown"/></default-field></field>
                <field name="caption"><default-field><label text="Caption" type="h4"/></default-field></field>
                <field name="code"><default-field><text-line input-type="number" mask="999" size="6"/></default-field></field>
                <field name="types"><default-field><drop-down allow-multiple="true" allow-empty="true" show-not="true">
                    <option key="A" text="Alpha"/></drop-down></default-field></field>
                <field name="flag"><default-field><check><option key="Y" text="Yes"/></check></default-field></field>
                <field name="extra"><default-field>
                    <visible-when field="flag" value="Y"/><text-line/></default-field></field>
                <field name="go"><default-field><submit confirmation="Sure?"/></default-field></field>
            </form-single>
            <link url="/apps/x" url-type="plain" text="Do" btn-type="danger" icon="delete" badge="3" link-type="anchor-button"/>
        </widgets></screen>"#,
        parameters,
    );
    let body = output.body_text();
    let doc = document(&output);
    let field = |name: &str| {
        nodes(&doc)
            .into_iter()
            .find(|node| node.get("t").and_then(Json::as_str) == Some("field") && node["name"] == json!(name))
            .unwrap_or_else(|| panic!("no field {name} in {doc}"))
    };
    assert_eq!(field("note")["w"]["kind"], json!("textArea"));
    assert_eq!(field("note")["w"]["rows"], json!(4));
    assert_eq!(field("note")["tooltip"], json!("A note"));
    assert_eq!(field("body")["w"]["readOnly"], json!(true));
    assert_eq!(field("descr")["w"]["op"], json!("begins"));
    assert_eq!(field("descr")["w"]["not"], json!(true));
    assert_eq!(field("when")["w"]["kind"], json!("dateFind"));
    assert_eq!(field("period")["w"]["kind"], json!("datePeriod"));
    assert_eq!(field("amount")["w"]["from"], json!("5"));
    assert_eq!(field("upload")["w"]["multiple"], json!(true));
    assert_eq!(field("shown")["w"]["text"], json!("Shown"));
    assert_eq!(field("caption")["w"]["style"], json!("h4"));
    assert_eq!(field("code")["w"]["inputType"], json!("number"));
    assert_eq!(field("types")["w"]["multiple"], json!(true));
    assert_eq!(field("types")["w"]["allowEmpty"], json!(true));
    assert_eq!(field("flag")["w"]["options"][0]["label"], json!("Yes"));
    assert_eq!(field("extra")["visibleWhen"], json!({"field": "flag", "value": "Y"}));
    assert_eq!(field("go")["w"]["confirm"], json!("Sure?"));
    let regions: Vec<&str> = nodes(&doc)
        .into_iter()
        .filter(|node| node.get("t").and_then(Json::as_str) == Some("region"))
        .filter_map(|node| node["name"].as_str())
        .collect();
    assert_eq!(regions, ["toolbar", "left", "center", "side", "main"]);
    let left = nodes(&doc)
        .into_iter()
        .find(|node| node.get("name") == Some(&json!("left")))
        .unwrap();
    assert_eq!(left["size"], json!(200));
    assert_eq!(first_of(&doc, "split")["sideWidth"], json!(240));
    let link = first_of(&doc, "link");
    assert_eq!(link["btnType"], json!("danger"));
    assert_eq!(link["icon"], json!("delete"));
    assert_eq!(link["badge"], json!("3"));
    assert!(
        nodes(&doc)
            .iter()
            .any(|node| node.get("t") == Some(&json!("text")) && node.get("style") == Some(&json!("h3"))),
        "{doc}"
    );
    assert!(!body.contains("\"placeholder\""), "{body}");
    let model = moqui_tui::ScreenModel::from_json(&body).unwrap_or_else(|err| panic!("{err}: {body}"));
    let mut kinds = Vec::new();
    moqui_tui::walk(&model.c, &mut |node| {
        if let moqui_tui::Node::Field { w, .. } = node {
            kinds.push(format!("{w:?}").split([' ', '{']).next().unwrap_or("").to_string());
        }
    });
    assert!(kinds.contains(&"DatePeriod".to_string()), "{kinds:?}");
}

/// The document read as the client reads it: no node or widget is one the client does not know (from_json makes those
/// placeholders, and an unknown widget a display of `[kind]`).
fn client_model(body: &str) -> moqui_tui::ScreenModel {
    let model = moqui_tui::ScreenModel::from_json(body).unwrap_or_else(|err| panic!("{err}: {body}"));
    moqui_tui::walk(&model.c, &mut |node| match node {
        moqui_tui::Node::Placeholder { text, .. } => panic!("placeholder {text} in {body}"),
        moqui_tui::Node::Field {
            w: moqui_tui::Widget::Display { text, .. },
            ..
        } => assert!(
            !(text.starts_with('[') && text.ends_with(']')),
            "unknown widget {text} in {body}"
        ),
        _ => {}
    });
    model
}

/// Every node of the client model, list cells, header fields and buttons included.
fn client_nodes(model: &moqui_tui::ScreenModel) -> Vec<moqui_tui::Node> {
    let mut found = Vec::new();
    moqui_tui::walk(&model.c, &mut |node| found.push(node.clone()));
    found
}

fn field_named<'a>(doc: &'a Json, name: &str) -> &'a serde_json::Map<String, Json> {
    nodes(doc)
        .into_iter()
        .find(|node| node.get("t").and_then(Json::as_str) == Some("field") && node["name"] == json!(name))
        .unwrap_or_else(|| panic!("no field {name} in {doc}"))
}

fn list_named<'a>(doc: &'a Json, name: &str) -> &'a serde_json::Map<String, Json> {
    nodes(doc)
        .into_iter()
        .find(|node| node.get("t").and_then(Json::as_str) == Some("formList") && node["name"] == json!(name))
        .unwrap_or_else(|| panic!("no list {name} in {doc}"))
}

fn hidden_map(value: &Json) -> BTreeMap<String, String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("no hidden list: {value}"))
        .iter()
        .map(|pair| {
            (
                pair["name"].as_str().unwrap().to_string(),
                pair["value"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// An autocomplete text-line is a server-search drop-down; a default-transition, a dynamic display and dynamic options carry
/// their URL, fixed parameters and depends-on; an editable is a click-to-edit field; a file field makes the form multipart.
#[test]
fn autocomplete_default_and_dynamic_values_and_editables_carry_their_transitions() {
    let output = render(
        "dynamics",
        r#"<screen require-authentication="false">
            <transition name="save"><default-response type="none"/></transition>
            <transition name="searchParties"><default-response type="none"/></transition>
            <transition name="getCity"><default-response type="none"/></transition>
            <transition name="getTotal"><default-response type="none"/></transition>
            <transition name="states"><default-response type="none"/></transition>
            <transition name="saveNote"><default-response type="none"/></transition>
            <transition name="dlg"><default-response type="none"/></transition>
            <actions><script>
            fv = {partyId: 'P1', city: '', total: '12', note: 'hello'}
        </script></actions><widgets>
            <form-single name="Edit" map="fv" transition="save">
                <field name="orgId"><default-field><text-line/></default-field></field>
                <field name="partyId"><default-field title="Party">
                    <text-line ac-transition="searchParties" ac-min-length="2" ac-use-actual="true" depends-optional="true"
                            ac-initial-text="Party ${partyId}" parameter-map="{roleTypeId:'Customer', blank:''}">
                        <depends-on field="orgId" parameter="ownerPartyId"/></text-line>
                </default-field></field>
                <field name="city"><default-field>
                    <text-line prefix="${'@'}" disabled="true" default-transition="getCity"><depends-on field="orgId"/></text-line>
                </default-field></field>
                <field name="total"><default-field>
                    <display dynamic-transition="getTotal" parameter-map="{currency:'USD'}"><depends-on field="orgId"/></display>
                </default-field></field>
                <field name="state"><default-field><drop-down>
                    <dynamic-options transition="states" depends-optional="true" min-length="3" server-search="true">
                        <depends-on field="orgId"/></dynamic-options>
                </drop-down></default-field></field>
                <field name="note"><default-field>
                    <editable text="${note}" transition="saveNote" parameter-name="noteText" widget-type="select" value="raw"
                            parameter-map="{partyId: fv.partyId}"><option key="a" text="A"/></editable>
                </default-field></field>
                <field name="blank"><default-field><editable text="" transition="saveNote"/></default-field></field>
                <field name="shown"><default-field><editable text="" transition="saveNote" display-if-empty="true"/></default-field></field>
                <field name="upload"><default-field><file/></default-field></field>
            </form-single>
            <form-single name="Plain"><field name="a"><default-field><text-line/></default-field></field></form-single>
            <dynamic-dialog id="Dd" button-text="Open" transition="dlg"/>
            <dynamic-dialog id="Dt" button-text="Open" title="Edit It" transition="dlg"/>
        </widgets></screen>"#,
        BTreeMap::new(),
    );
    let body = output.body_text();
    let doc = document(&output);
    let party = &field_named(&doc, "partyId")["w"];
    assert_eq!(party["kind"], json!("dropDown"), "{party}");
    assert_eq!(party["serverSearch"], json!(true));
    assert_eq!(party["allowEmpty"], json!(true));
    assert_eq!(party["combo"], json!(true));
    assert_eq!(party["minLength"], json!(2));
    assert_eq!(party["dependsOptional"], json!(true));
    assert_eq!(party["options"], json!([{"value": "P1", "label": "Party P1"}]));
    assert_eq!(party["dependsOn"], json!({"ownerPartyId": "orgId"}));
    assert_eq!(party["optionsParameters"], json!({"roleTypeId": "Customer"}));
    let url = party["optionsUrl"].as_str().unwrap();
    assert!(url.ends_with("searchParties") && !url.contains('?'), "{party}");
    let city = &field_named(&doc, "city")["w"];
    assert_eq!(city["kind"], json!("textLine"));
    assert!(city["defaultUrl"].as_str().unwrap().ends_with("getCity"), "{city}");
    assert_eq!(city["dependsOn"], json!({"orgId": "orgId"}));
    assert!(city.get("defaultParameters").is_none(), "{city}");
    assert_eq!(city["prefix"], json!("@"));
    assert_eq!(city["disabled"], json!(true));
    let total = &field_named(&doc, "total")["w"];
    assert_eq!(total["kind"], json!("display"));
    assert_eq!(total["text"], json!("12"));
    assert!(total["valueUrl"].as_str().unwrap().ends_with("getTotal"), "{total}");
    assert_eq!(total["valueParameters"], json!({"currency": "USD"}));
    assert_eq!(total["dependsOn"], json!({"orgId": "orgId"}));
    let state = &field_named(&doc, "state")["w"];
    assert_eq!(state["dependsOptional"], json!(true));
    assert_eq!(state["minLength"], json!(3));
    assert_eq!(state["serverSearch"], json!(true));
    let note = &field_named(&doc, "note")["w"];
    assert_eq!(note["kind"], json!("editable"));
    assert_eq!(note["text"], json!("hello"));
    assert!(note["url"].as_str().unwrap().ends_with("saveNote"), "{note}");
    assert_eq!(note["parameters"], json!({"partyId": "P1"}));
    assert_eq!(note["parameterName"], json!("noteText"));
    assert_eq!(note["widgetType"], json!("select"));
    assert_eq!(note["value"], json!("raw"));
    assert_eq!(note["options"], json!([{"value": "a", "label": "A"}]));
    assert!(
        !nodes(&doc).iter().any(|node| node.get("name") == Some(&json!("blank"))),
        "an empty editable is not written: {doc}"
    );
    let shown = &field_named(&doc, "shown")["w"];
    assert_eq!(shown["parameterName"], json!("value"));
    assert_eq!(shown["widgetType"], json!("textarea"));
    assert!(shown.get("value").is_none(), "{shown}");
    let forms: Vec<_> = nodes(&doc)
        .into_iter()
        .filter(|node| node.get("t").and_then(Json::as_str) == Some("form"))
        .collect();
    assert_eq!(forms[0]["multipart"], json!(true));
    assert!(forms[1].get("multipart").is_none(), "{doc}");
    let dialogs: Vec<_> = nodes(&doc)
        .into_iter()
        .filter(|node| node.get("t").and_then(Json::as_str) == Some("dynamicDialog"))
        .collect();
    assert_eq!(dialogs[0]["title"], json!("Open"));
    assert_eq!(dialogs[1]["title"], json!("Edit It"));
    let model = client_model(&body);
    let kinds: Vec<String> = client_nodes(&model)
        .iter()
        .filter_map(|node| match node {
            moqui_tui::Node::Field { name, w, .. } => Some(format!(
                "{name}:{}",
                format!("{w:?}").split([' ', '{']).next().unwrap_or("")
            )),
            _ => None,
        })
        .collect();
    for expected in [
        "partyId:DropDown",
        "city:TextLine",
        "total:Display",
        "note:Editable",
        "shown:Editable",
    ] {
        assert!(kinds.contains(&expected.to_string()), "{expected} in {kinds:?}");
    }
    assert!(model
        .c
        .iter()
        .any(|node| matches!(node, moqui_tui::Node::Form { multipart: true, .. })));
}

const ROWS: &str = "rows = [{id:'a1', name:'Alpha', qty:2, note:'n1'}, {id:'b2', name:'Beta', qty:3, note:''}]";

/// A list with a transition is a form: a multi list is one form of every row (fields `name_N`, `_isMulti`, the buttons once)
/// and any other a form per row (each row its hidden values, and its buttons in its cells). A list without one is text.
#[test]
fn a_list_with_a_transition_is_a_multi_form_or_a_form_per_row() {
    let output = render(
        "row-forms",
        &format!(
            r#"<screen require-authentication="false">
            <transition name="saveRows"><default-response type="none"/></transition>
            <transition name="saveRow"><default-response type="none"/></transition>
            <transition name="saveNote"><default-response type="none"/></transition>
            <actions><script>{ROWS}</script></actions><widgets>
            <form-list name="Multi" list="rows" transition="saveRows" multi="true" id-field="id">
                <hidden-parameters><parameter name="orderId" value="O1"/></hidden-parameters>
                <field name="id"><default-field><hidden/></default-field></field>
                <field name="name"><default-field><text-line/></default-field></field>
                <field name="qty"><default-field><display/></default-field></field>
                <field name="note"><default-field><editable text="${{note}}" transition="saveNote" parameter-map="{{id: id}}"/></default-field></field>
                <field name="submitButton"><default-field title="Save All"><submit/></default-field></field>
            </form-list>
            <form-list name="Each" list="rows" transition="saveRow">
                <field name="id"><default-field><hidden/></default-field></field>
                <field name="name"><default-field><text-line/></default-field></field>
                <field name="go"><default-field title="Go"><submit/></default-field></field>
            </form-list>
            <form-list name="Shown" list="rows">
                <field name="name"><default-field><text-line/></default-field></field>
                <field name="go"><default-field title="Go"><submit/></default-field></field>
            </form-list>
        </widgets></screen>"#
        ),
        BTreeMap::new(),
    );
    let body = output.body_text();
    let doc = document(&output);
    let multi = list_named(&doc, "Multi");
    assert!(
        multi["transitionUrl"].as_str().unwrap().ends_with("saveRows"),
        "{multi:?}"
    );
    assert_eq!(multi["multi"], json!(true));
    let hidden = hidden_map(&multi["hidden"]);
    assert_eq!(hidden.get("orderId").map(String::as_str), Some("O1"), "{hidden:?}");
    assert_eq!(hidden.get("moquiFormName").map(String::as_str), Some("Multi"));
    assert_eq!(hidden.get("_isMulti").map(String::as_str), Some("true"));
    assert_eq!(hidden.get("pageIndex").map(String::as_str), Some("0"));
    let row = &multi["rows"][0];
    assert_eq!(row["id"], json!("a1"));
    let name = &row["cells"][0];
    assert_eq!(name["t"], json!("field"), "{row}");
    assert_eq!(name["name"], json!("name_0"));
    assert_eq!(name["id"], json!("Multi.name.a1"));
    assert_eq!(name["w"]["value"], json!("Alpha"));
    assert_eq!(multi["rows"][1]["cells"][0]["name"], json!("name_1"));
    assert_eq!(row["cells"][1], json!("2"));
    assert_eq!(row["cells"][2]["w"]["kind"], json!("editable"));
    assert_eq!(row["cells"][2]["w"]["parameters"], json!({"id": "a1"}));
    assert_eq!(row["cells"][3], json!(""), "the buttons are the list's: {row}");
    assert_eq!(multi["rows"][1]["cells"][2], json!(""), "an empty editable: {multi:?}");
    let row_hidden = hidden_map(&row["hidden"]);
    assert_eq!(row_hidden.get("id_0").map(String::as_str), Some("a1"), "{row_hidden:?}");
    assert_eq!(row_hidden.get("qty_0").map(String::as_str), Some("2"));
    let buttons = multi["buttons"].as_array().unwrap();
    assert_eq!(buttons.len(), 1, "{buttons:?}");
    assert_eq!(buttons[0]["w"]["kind"], json!("submit"));
    assert_eq!(buttons[0]["name"], json!("submitButton"));

    let each = list_named(&doc, "Each");
    assert!(each["transitionUrl"].as_str().unwrap().ends_with("saveRow"), "{each:?}");
    assert!(each.get("multi").is_none() && each.get("buttons").is_none(), "{each:?}");
    let hidden = hidden_map(&each["hidden"]);
    assert_eq!(hidden.get("moquiFormName").map(String::as_str), Some("Each"));
    assert!(!hidden.contains_key("_isMulti"), "{hidden:?}");
    let row = &each["rows"][1];
    assert_eq!(row["cells"][0]["name"], json!("name"));
    assert_eq!(row["cells"][0]["id"], json!("Each.name.1"));
    assert_eq!(row["cells"][1]["w"]["kind"], json!("submit"));
    assert_eq!(row["cells"][1]["w"]["text"], json!("Go"));
    assert_eq!(hidden_map(&row["hidden"]).get("id").map(String::as_str), Some("b2"));

    let shown = list_named(&doc, "Shown");
    assert!(
        shown.get("transitionUrl").is_none() && shown.get("hidden").is_none(),
        "{shown:?}"
    );
    assert_eq!(shown["rows"][0]["cells"], json!(["Alpha", "Go"]));
    assert!(shown["rows"][0].get("hidden").is_none(), "{shown:?}");

    let model = client_model(&body);
    let lists: Vec<moqui_tui::FormList> = model
        .c
        .iter()
        .filter_map(|node| match node {
            moqui_tui::Node::FormList { list } => Some(list.clone()),
            _ => None,
        })
        .collect();
    assert!(lists[0].multi && lists[0].buttons.len() == 1);
    assert_eq!(lists[0].rows[0].hidden.len(), 2);
    assert!(matches!(
        lists[0].rows[0].cells[0].nodes()[0],
        moqui_tui::Node::Field {
            w: moqui_tui::Widget::TextLine { .. },
            ..
        }
    ));
    assert!(matches!(
        lists[1].rows[0].cells[1].nodes()[0],
        moqui_tui::Node::Field {
            w: moqui_tui::Widget::Submit { .. },
            ..
        }
    ));
}

/// A column of several fields stacks their parts; a widget with no text of its own gives its value or nothing, never its
/// element's name; typed columns follow the request's `_uiType`.
#[test]
fn stacked_columns_are_parts_and_no_cell_is_an_element_name() {
    let xml = format!(
        r#"<screen require-authentication="false"><actions><script>{ROWS}</script></actions><widgets>
            <form-list name="Stack" list="rows">
                <field name="name"><default-field><display/></default-field></field>
                <field name="qty"><default-field><display/></default-field></field>
                <field name="when"><default-field><date-find/></default-field></field>
                <field name="lbl"><default-field><label text="Lbl ${{name}}"/></default-field></field>
                <field name="pic"><default-field><image url="/x.png" alt="Pic"/></default-field></field>
                <field name="up"><default-field><file/></default-field></field>
                <field name="range"><default-field><range-find/></default-field></field>
                <field name="look"><default-field><lookup/></default-field></field>
                <form-list-column><field-ref name="name"/><field-ref name="qty"/></form-list-column>
                <form-list-column><field-ref name="when"/><field-ref name="lbl"/></form-list-column>
                <form-list-column><field-ref name="pic"/><field-ref name="up"/></form-list-column>
                <form-list-column><field-ref name="range"/><field-ref name="look"/></form-list-column>
            </form-list>
            <form-list name="Typed" list="rows">
                <field name="name"><default-field><display/></default-field></field>
                <field name="qty"><default-field><display/></default-field></field>
                <columns><column><field-ref name="name"/></column><column><field-ref name="qty"/></column></columns>
                <columns type="mobile"><column><field-ref name="name"/><field-ref name="qty"/></column></columns>
            </form-list>
        </widgets></screen>"#
    );
    let output = render("stacked", &xml, BTreeMap::new());
    let body = output.body_text();
    let doc = document(&output);
    let list = list_named(&doc, "Stack");
    assert_eq!(list["columns"][0]["title"], json!("Name / Qty"));
    // the names of the fields a column shows, in the order of its cell's parts (the keys of a canvas read's rows)
    assert_eq!(list["columns"][0]["fields"], json!(["name", "qty"]));
    assert_eq!(
        list["rows"][0]["cells"],
        json!([["Alpha", "2"], "Lbl Alpha", "Pic", ""])
    );
    for name in ["date-find", "label", "image", "file", "range-find", "lookup"] {
        assert!(!body.contains(&format!("\"{name}\"")), "{name} in {body}");
    }
    let typed = list_named(&doc, "Typed");
    assert_eq!(typed["columns"].as_array().unwrap().len(), 2, "{typed:?}");
    assert_eq!(typed["rows"][0]["cells"], json!(["Alpha", "2"]));
    let model = client_model(&body);
    let stack = model
        .c
        .iter()
        .find_map(|node| match node {
            moqui_tui::Node::FormList { list } => Some(list.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(stack.rows[0].cells[0].parts().len(), 2);

    let mut mobile = BTreeMap::new();
    mobile.insert("_uiType".to_string(), Value::string("mobile"));
    let output = render("stacked-mobile", &xml, mobile);
    let doc = document(&output);
    let typed = list_named(&doc, "Typed");
    assert_eq!(typed["columns"], json!([{"title": "Name / Qty", "fields": ["name", "qty"]}]), "{typed:?}");
    assert_eq!(typed["rows"][1]["cells"], json!([["Beta", "3"]]));
    client_model(&output.body_text());
}

/// A container-dialog in a list cell (an Edit per row) is a dialog node of the row, its form filled from the row; a
/// section in such a dialog cannot be captured and is an error node, with the rest of the document whole.
#[test]
fn a_container_dialog_in_a_list_cell_is_a_dialog_of_its_row() {
    let xml = format!(
        r#"<screen require-authentication="false"><transition name="update"><default-response url="."/></transition>
        <actions><script>{ROWS}</script></actions><widgets>
            <form-list name="Rows" list="rows" skip-form="true">
                <field name="name"><default-field><display/></default-field></field>
                <field name="edit"><default-field title="">
                    <container-dialog id="EditDialog" button-text="Edit" title="Edit ${{name}}">
                        <form-single name="EditRow" transition="update">
                            <field name="name"><default-field><display/></default-field></field>
                            <field name="qty"><default-field><text-line/></default-field></field>
                            <field name="save"><default-field><submit/></default-field></field>
                        </form-single>
                    </container-dialog>
                </default-field></field>
            </form-list>
            <section name="After"><widgets><label text="after the list"/></widgets></section>
        </widgets></screen>"#
    );
    let output = render("row-dialog", &xml, BTreeMap::new());
    let body = output.body_text();
    let doc = document(&output);
    let list = list_named(&doc, "Rows");
    let rows = list["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{list:?}");
    let dialog = |row: usize| {
        rows[row]["cells"]
            .as_array()
            .unwrap()
            .iter()
            .find(|cell| cell["t"] == json!("dialog"))
            .unwrap_or_else(|| panic!("no dialog in row {row}: {}", rows[row]))
            .clone()
    };
    let (first, second) = (dialog(0), dialog(1));
    assert_eq!(first["title"], json!("Edit Alpha"));
    assert_eq!(second["title"], json!("Edit Beta"));
    assert_ne!(first["id"], second["id"]);
    let qty = |dialog: &Json| {
        nodes(dialog)
            .into_iter()
            .find(|node| node.get("name").and_then(Json::as_str) == Some("qty"))
            .map(|node| node["w"]["value"].clone())
    };
    assert_eq!(qty(&first), Some(json!("2")), "{first}");
    assert_eq!(qty(&second), Some(json!("3")), "{second}");
    assert!(body.contains("after the list"), "{body}");
    client_model(&body);

    let nested = xml.replace(
        r#"<form-single name="EditRow""#,
        r#"<section name="Inner"><widgets><label text="x"/></widgets></section><form-single name="EditRow""#,
    );
    let output = render("row-dialog-nested", &nested, BTreeMap::new());
    let body = output.body_text();
    let doc = document(&output);
    assert!(body.contains("after the list"), "{body}");
    assert!(
        nodes(&doc)
            .iter()
            .any(|node| node.get("t").and_then(Json::as_str) == Some("error")),
        "{body}"
    );
}

/// A row-selection action with a dialog is a dialog after the list, naming the list, with the action's form.
#[test]
fn a_row_selection_action_is_a_dialog_of_its_list() {
    let output = render(
        "row-actions",
        &format!(
            r#"<screen require-authentication="false"><actions><script>{ROWS}</script></actions><widgets>
            <form-list name="Sel" list="rows">
                <row-selection id-field="id" parameter="ids">
                    <action><dialog button-text="Bulk Edit" title="Edit Selected"/>
                        <form-single name="BulkForm" transition="bulk">
                            <field name="status"><default-field><text-line/></default-field></field>
                            <field name="go"><default-field><submit/></default-field></field>
                        </form-single>
                    </action>
                    <action><dialog button-text="Close"/>
                        <form-single name="CloseForm" transition="close"><field name="go"><default-field><submit/></default-field></field></form-single>
                    </action>
                </row-selection>
                <field name="name"><default-field><display/></default-field></field>
            </form-list>
        </widgets></screen>"#
        ),
        BTreeMap::new(),
    );
    let body = output.body_text();
    let doc = document(&output);
    let list = list_named(&doc, "Sel");
    assert_eq!(list["rows"][0]["selectId"], json!("a1"));
    assert_eq!(list["rowSelection"]["parameter"], json!("ids"));
    let dialogs: Vec<_> = nodes(&doc)
        .into_iter()
        .filter(|node| node.get("t").and_then(Json::as_str) == Some("dialog"))
        .collect();
    assert_eq!(dialogs.len(), 2, "{doc}");
    assert_eq!(dialogs[0]["list"], list["id"]);
    assert_eq!(dialogs[0]["title"], json!("Edit Selected"));
    assert_eq!(dialogs[0]["button"], json!("Bulk Edit"));
    assert_eq!(dialogs[0]["c"][0]["t"], json!("form"));
    assert_eq!(dialogs[0]["c"][0]["name"], json!("BulkForm"));
    assert_eq!(dialogs[0]["c"][0]["c"][0]["name"], json!("status"));
    assert_eq!(dialogs[1]["title"], json!("Close"));
    let model = client_model(&body);
    assert!(model.c.iter().any(|node| matches!(
        node,
        moqui_tui::Node::Dialog { list, .. } if list == "Sel"
    )));
}

/// A row-selection whose condition is false, or fails to evaluate, is left out: no rowSelection, no selectId on a row and no
/// action dialogs, as a list without one; a board whose move-condition is false has no move URL.
#[test]
fn a_row_selection_condition_and_a_board_move_condition_turn_them_off() {
    let list_xml = |name: &str, condition: &str| {
        format!(
            r#"<form-list name="{name}" list="rows" views="list,board">
                <row-selection id-field="id" parameter="ids" condition="{condition}">
                    <action><dialog button-text="Bulk {name}"/>
                        <form-single name="Bulk{name}" transition="bulk"><field name="go"><default-field><submit/></default-field></field></form-single>
                    </action>
                </row-selection>
                <view-board group-field="name" transition="move" move-condition="{condition}"/>
                <field name="name"><default-field><display/></default-field></field>
            </form-list>"#
        )
    };
    let output = render(
        "row-selection-condition",
        &format!(
            r#"<screen require-authentication="false"><transition name="move"><default-response type="none"/></transition>
            <actions><script>{ROWS}; canBulk = true</script></actions><widgets>{}{}{}</widgets></screen>"#,
            list_xml("On", "canBulk"),
            list_xml("Off", "!canBulk"),
            list_xml("Bad", "noSuchThing.call()")
        ),
        BTreeMap::new(),
    );
    let body = output.body_text();
    let doc = document(&output);
    let on = list_named(&doc, "On");
    assert_eq!(on["rowSelection"]["parameter"], json!("ids"), "{body}");
    assert_eq!(on["rows"][0]["selectId"], json!("a1"), "{body}");
    assert!(
        on["views"]["board"]["move"]
            .as_str()
            .is_some_and(|url| url.ends_with("/move")),
        "{body}"
    );
    for name in ["Off", "Bad"] {
        let off = list_named(&doc, name);
        assert!(off.get("rowSelection").is_none(), "{body}");
        assert!(off["rows"][0].get("selectId").is_none(), "{body}");
        assert_eq!(off["views"]["board"]["move"], json!(""), "{body}");
    }
    let dialogs: Vec<_> = nodes(&doc)
        .into_iter()
        .filter(|node| node.get("t").and_then(Json::as_str) == Some("dialog"))
        .collect();
    assert_eq!(dialogs.len(), 1, "{doc}");
    assert_eq!(dialogs[0]["button"], json!("Bulk On"));
    let model = client_model(&body);
    let selecting: Vec<_> = client_nodes(&model)
        .into_iter()
        .filter_map(|node| match node {
            moqui_tui::Node::FormList { list } => Some((list.id.clone(), list.row_selection.is_some())),
            _ => None,
        })
        .collect();
    assert_eq!(selecting.iter().filter(|(_, on)| *on).count(), 1, "{selecting:?}");
}

/// The exports of a list are URLs of the screen in the export's mode (with the current parameters, which
/// `a_list_gives_the_users_saved_finds` shows), and Show All is the screen without a page limit; the column choice lists
/// the fields that have no column.
#[test]
fn a_list_gives_its_exports_and_columns() {
    let output = render(
        "exports",
        &format!(
            r#"<screen require-authentication="false"><actions><script>{ROWS}</script></actions><widgets>
            <form-list name="Exp" list="rows" show-csv-button="true" show-xlsx-button="true" show-text-button="true"
                    show-pdf-button="true" show-all-button="true" select-columns="true">
                <field name="name"><header-field title="Name"/><default-field><display/></default-field></field>
                <field name="qty"><default-field><display/></default-field></field>
                <field name="extra"><default-field title="More"><display/></default-field></field>
                <field name="hid"><default-field><hidden/></default-field></field>
                <form-list-column><field-ref name="name"/></form-list-column>
                <form-list-column><field-ref name="qty"/></form-list-column>
            </form-list>
            <form-list name="Bare" list="rows"><field name="name"><default-field><display/></default-field></field></form-list>
        </widgets></screen>"#
        ),
        BTreeMap::new(),
    );
    let body = output.body_text();
    let doc = document(&output);
    let list = list_named(&doc, "Exp");
    let exports = list["exports"].as_array().unwrap();
    let kinds: Vec<&str> = exports.iter().map(|item| item["kind"].as_str().unwrap()).collect();
    for kind in ["csv", "text", "pdf", "all"] {
        assert!(kinds.contains(&kind), "{kind} in {exports:?}");
    }
    let url = |kind: &str| {
        exports
            .iter()
            .find(|item| item["kind"] == json!(kind))
            .and_then(|item| item["url"].as_str())
            .unwrap()
            .to_string()
    };
    for part in [
        "renderMode=csv",
        "pageNoLimit=true",
        "lastStandalone=true",
        "saveFilename=Exp.csv",
    ] {
        assert!(url("csv").contains(part), "{part} in {}", url("csv"));
    }
    assert!(url("csv").starts_with('/'), "{}", url("csv"));
    for part in ["renderMode=text", "lineCharacters=132", "saveFilename=Exp.txt"] {
        assert!(url("text").contains(part), "{part} in {}", url("text"));
    }
    // Print is the page as a PDF under /fop, Java's MoquiFopServlet, with every row
    assert!(url("pdf").starts_with("/fop/"), "{}", url("pdf"));
    assert!(
        url("pdf").contains("pageNoLimit=true") && !url("pdf").contains("renderMode"),
        "{}",
        url("pdf")
    );
    assert!(url("all").contains("pageNoLimit=true"), "{}", url("all"));
    let all = exports.iter().find(|item| item["kind"] == json!("all")).unwrap();
    assert_eq!(all["label"], json!("Show All"));
    let columns = &list["selectColumns"];
    assert!(
        columns["url"].as_str().unwrap().ends_with("formSelectColumns"),
        "{columns}"
    );
    assert_eq!(columns["formLocation"], json!("Exp"));
    assert_eq!(
        columns["shown"],
        json!([[{"name": "name", "title": "Name"}], [{"name": "qty", "title": "Qty"}]])
    );
    assert_eq!(columns["hidden"], json!([{"name": "extra", "title": "More"}]));
    assert_eq!(columns["parameters"], json!({}));
    let bare = list_named(&doc, "Bare");
    for key in ["exports", "selectColumns", "savedFinds"] {
        assert!(bare.get(key).is_none(), "{key} in {bare:?}");
    }
    // a list whose columns the user may choose is in a fragment of its own
    let model = client_model(&body);
    let exp = client_nodes(&model)
        .into_iter()
        .find_map(|node| match node {
            moqui_tui::Node::FormList { list } if list.name == "Exp" => Some(list),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{:?}", model.c));
    assert_eq!(exp.select_columns.unwrap().hidden.len(), 1);
    assert!(exp.exports.len() >= 4);
}

/// A user's saved finds of a list: each with the URL that applies it, the user's own marked, the current find's parameters
/// and the URL that clears it.
#[test]
fn a_list_gives_the_users_saved_finds() {
    let dir = std::env::temp_dir().join(format!("moqui-tui-finds-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    std::fs::write(
        dir.join("Root.xml"),
        r#"<screen require-authentication="false"><subscreens default-item="Find"/><widgets><subscreens-active/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Root/Find.xml"),
        r#"<screen require-authentication="false">
            <parameter name="a"/>
            <actions><script>rows = [{a: 'A1'}]</script></actions>
            <widgets><form-list name="Rows" list="rows" saved-finds="true" select-columns="true" show-csv-button="true">
                <field name="a"><header-field title="Col A"><text-find/></header-field><default-field><display/></default-field></field>
            </form-list></widgets>
        </screen>"#,
    )
    .unwrap();
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = super::World::open("");
    world.user("ada", None);
    let mut post = super::request("POST", "/Find/formSaveFind");
    post.set_header("authorization", super::basic("ada"));
    post.set_header("content-type", "application/x-www-form-urlencoded");
    post.body = b"formLocation=Rows&a=foo&_findDescription=Mine".to_vec();
    let saved = crate::screen::serve_at(&world.factory, post, &root);
    assert_eq!(saved.status, 302, "{}", saved.body_text());
    let mut get = super::request("GET", "/Find");
    get.set_header("authorization", super::basic("ada"));
    for (name, value) in [("renderMode", "tui"), ("a", "foo"), ("pageIndex", "0")] {
        get.query.push((name.to_string(), value.to_string()));
    }
    let response = crate::screen::serve_at(&world.factory, get, &root);
    let _ = std::fs::remove_dir_all(&dir);
    let body = response.body_text();
    assert_eq!(response.status, 200, "{body}");
    let doc: Json = serde_json::from_str(&body).unwrap_or_else(|err| panic!("{err}: {body}"));
    let list = list_named(&doc, "Rows");
    let finds = &list["savedFinds"];
    assert!(
        finds["saveUrl"].as_str().unwrap().ends_with("/Find/formSaveFind"),
        "{finds}"
    );
    assert!(finds["formLocation"].as_str().unwrap().contains("Rows"), "{finds}");
    // the find's parameters: not the page, the render mode or the find itself
    assert_eq!(finds["parameters"], json!({"a": "foo"}), "{finds}");
    assert_eq!(list["selectColumns"]["parameters"], json!({"a": "foo"}), "{list:?}");
    assert!(
        finds["clearUrl"]
            .as_str()
            .unwrap()
            .ends_with("/Find?formListFindId=_clear"),
        "{finds}"
    );
    let csv = list["exports"][0]["url"].as_str().unwrap();
    assert!(csv.contains("a=foo") && csv.contains("renderMode=csv"), "{csv}");
    let find = &finds["finds"][0];
    assert_eq!(find["name"], json!("Mine"), "{finds}");
    assert_eq!(find["mine"], json!(true));
    assert_eq!(find["active"], json!(false));
    assert_eq!(find["default"], json!(false));
    let id = find["id"].as_str().unwrap();
    assert!(
        find["url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/Find?formListFindId={id}")),
        "{find}"
    );
    let model = client_model(&body);
    let list = client_nodes(&model)
        .into_iter()
        .find_map(|node| match node {
            moqui_tui::Node::FormList { list } => Some(list),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{:?}", model.c));
    assert_eq!(list.saved_finds.unwrap().finds[0].name, "Mine");
    assert!(list.select_columns.is_some());
}

/// A tree is the URL its items load from; a button-menu holds its items; a component of a tui text gets the screen's path
/// as its base unless it names one, and a component with no name makes the text an error node.
#[test]
fn trees_button_menus_and_components() {
    let output = render(
        "structures",
        r#"<screen require-authentication="false"><transition name="treeItems"><default-response type="none"/></transition><widgets>
            <tree name="Cats" root-node-name="n" open-path="${'a/b'}"/>
            <tree name="Dogs" root-node-name="n" transition="treeItems" parameter-map="{kind:'dog'}"/>
            <button-menu text="More"><link url="/a" url-type="plain" text="A"/><link url="/b" url-type="plain" text="B"/></button-menu>
            <render-mode><text type="tui"><![CDATA[{"t":"component","name":"chart","props":{"type":"bar","series":[1,2]}}]]></text></render-mode>
            <render-mode><text type="tui"><![CDATA[{"t":"box","id":"wrap","c":[{"t":"component","name":"board","base":"/elsewhere"}]}]]></text></render-mode>
            <render-mode><text type="tui"><![CDATA[[{"t":"box","id":"b","c":[{"t":"component","props":{}}]}]]]></text></render-mode>
        </widgets></screen>"#,
        BTreeMap::new(),
    );
    let body = output.body_text();
    let doc = document(&output);
    let trees: Vec<_> = nodes(&doc)
        .into_iter()
        .filter(|node| node.get("t").and_then(Json::as_str) == Some("tree"))
        .collect();
    assert_eq!(trees[0]["id"], json!("Cats"));
    assert!(
        trees[0]["url"].as_str().unwrap().ends_with("/actions/Cats"),
        "{trees:?}"
    );
    assert_eq!(trees[0]["openPath"], json!("a/b"));
    assert!(trees[1]["url"].as_str().unwrap().ends_with("treeItems"), "{trees:?}");
    assert_eq!(trees[1]["parameters"], json!({"kind": "dog"}));
    let menu = first_of(&doc, "buttonMenu");
    assert_eq!(menu["text"], json!("More"));
    let items: Vec<&str> = menu["c"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["text"].as_str().unwrap())
        .collect();
    assert_eq!(items, ["A", "B"]);
    let components: Vec<_> = nodes(&doc)
        .into_iter()
        .filter(|node| node.get("t").and_then(Json::as_str) == Some("component"))
        .collect();
    assert_eq!(components.len(), 2, "{doc}");
    assert_eq!(components[0]["base"], json!("/"));
    assert_eq!(components[0]["props"], json!({"type": "bar", "series": [1, 2]}));
    assert_eq!(components[1]["base"], json!("/elsewhere"));
    let errors: Vec<&str> = nodes(&doc)
        .into_iter()
        .filter(|node| node.get("t").and_then(Json::as_str) == Some("error"))
        .filter_map(|node| node["text"].as_str())
        .collect();
    assert!(
        errors.len() == 1 && errors[0].contains("has a component with no name"),
        "{errors:?}"
    );
    let model = client_model(&body);
    let found = client_nodes(&model);
    assert_eq!(
        found
            .iter()
            .filter(|node| matches!(node, moqui_tui::Node::Tree { .. }))
            .count(),
        2
    );
    assert!(found.iter().any(|node| matches!(
        node,
        moqui_tui::Node::ButtonMenu { c, .. } if c.len() == 2
    )));
    assert!(found.iter().any(|node| matches!(
        node,
        moqui_tui::Node::Component { name, base, .. } if name == "chart" && base == "/"
    )));
}

fn qvt_render_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/script/testdata/screen/qvt/render")
}

/// A parent level asked for alone, as the terminal asks for each level, holds an empty active node: the level under it is
/// the client's next request, so a parent with no default item is not an error (Tools' ServiceJob, Security).
#[test]
fn a_parent_level_alone_has_an_empty_active_node() {
    let dir = std::env::temp_dir().join(format!("moqui-tui-parent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root/Parent")).unwrap();
    std::fs::write(
        dir.join("Root.xml"),
        r#"<screen require-authentication="false"><widgets><subscreens-active/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Root/Parent.xml"),
        r#"<screen require-authentication="false"><widgets><label text="Above"/><subscreens-active/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Root/Parent/Child.xml"),
        r#"<screen require-authentication="false"><widgets><label text="Leaf"/></widgets></screen>"#,
    )
    .unwrap();
    let context = ExecutionContextFactory::test_context();
    let render_at = |path: &str| {
        make_render(context.factory())
            .root_screen(dir.join("Root.xml").to_string_lossy().into_owned())
            .screen_path(path)
            .render_mode("tui")
            .render()
            .unwrap_or_else(|err| panic!("{path}: {err}"))
            .body_text()
    };
    let parent = render_at("Parent.tui");
    let child = render_at("Parent/Child");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!parent.contains("Tried to render subscreen"), "{parent}");
    let doc: Json = serde_json::from_str(&parent).unwrap_or_else(|err| panic!("{err}: {parent}"));
    assert_eq!(first_of(&doc, "active")["c"], json!([]), "{parent}");
    assert!(parent.contains("Above"), "{parent}");
    assert!(child.contains("Leaf") && !child.contains("Above"), "{child}");
}

/// A wizard or stack panel is a subscreens node like the tabs: the steps, the active one, a wizard's finish text, and the
/// active level inside.
#[test]
fn wizard_and_stack_panels_are_subscreens_nodes() {
    // The terminal asks for each level on its own (`.tui` on its last name stops the walk down the default subscreens).
    let dir = std::env::temp_dir().join(format!("moqui-tui-panels-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Root.xml"),
        format!(
            r#"<screen require-authentication="false"><subscreens>
                <subscreens-item name="Wiz" location="{}"/><subscreens-item name="Stk" location="{}"/>
            </subscreens><widgets><subscreens-active/></widgets></screen>"#,
            qvt_render_dir().join("Wizard.xml").display(),
            qvt_render_dir().join("Stack.xml").display()
        ),
    )
    .unwrap();
    let context = ExecutionContextFactory::test_context();
    let render_at = |path: &str| {
        make_render(context.factory())
            .root_screen(dir.join("Root.xml").to_string_lossy().into_owned())
            .screen_path(path)
            .render_mode("tui")
            .render()
            .unwrap_or_else(|err| panic!("{path}: {err}"))
            .body_text()
    };
    let body = render_at("Wiz.tui");
    let doc: Json = serde_json::from_str(&body).unwrap_or_else(|err| panic!("{err}: {body}"));
    let wizard = first_of(&doc, "subscreens");
    assert_eq!(wizard["kind"], json!("wizard"));
    assert_eq!(wizard["id"], json!("ImportWizard"));
    assert_eq!(wizard["finishText"], json!("Import"));
    let titles: Vec<&str> = wizard["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Source", "Mapping", "Review"]);
    assert!(
        wizard["items"][1]["url"].as_str().unwrap().ends_with("/Wiz/Mapping"),
        "{wizard:?}"
    );
    assert!(wizard["c"].is_array(), "{wizard:?}");
    assert!(!body.contains("-step\""), "{body}");
    let model = client_model(&body);
    assert!(client_nodes(&model).iter().any(|node| matches!(
        node,
        moqui_tui::Node::Subscreen { kind, finish_text, .. } if kind == "wizard" && finish_text == "Import"
    )));
    let body = render_at("Stk.tui");
    let _ = std::fs::remove_dir_all(&dir);
    let doc: Json = serde_json::from_str(&body).unwrap_or_else(|err| panic!("{err}: {body}"));
    let stack = first_of(&doc, "subscreens");
    assert_eq!(stack["kind"], json!("stack"));
    assert_eq!(stack["id"], json!("Pages"));
    assert_eq!(stack["items"].as_array().unwrap().len(), 2, "{stack:?}");
    assert!(stack.get("finishText").is_none(), "{stack:?}");
    client_model(&body);
}

/// A status-bar is the stages of the status's flow and a link per transition that posts the new status, as qvt's
/// m-status-bar: the next stage primary, an error danger, each confirming unless confirm="false"; read-only has no links;
/// no status, no stages.
#[test]
fn a_status_bar_has_its_stages_and_a_link_per_transition() {
    let context = super::qvt::status_context();
    let _entered = context.enter();
    let body = make_render(context.factory())
        .root_screen(qvt_render_dir().join("status.xml").to_string_lossy().into_owned())
        .screen_path("")
        .render_mode("tui")
        .render()
        .unwrap_or_else(|err| panic!("status: {err}"))
        .body_text();
    let doc: Json = serde_json::from_str(&body).unwrap_or_else(|err| panic!("{err}: {body}"));
    let bar = |id: &str| {
        nodes(&doc)
            .into_iter()
            .find(|node| node.get("t").and_then(Json::as_str) == Some("statusBar") && node["id"] == json!(id))
            .unwrap_or_else(|| panic!("no bar {id} in {doc}"))
    };
    let stages = |bar: &serde_json::Map<String, Json>| -> Vec<String> {
        bar["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|stage| {
                format!(
                    "{}:{}",
                    stage["statusId"].as_str().unwrap(),
                    stage["state"].as_str().unwrap()
                )
            })
            .collect()
    };
    let sync = bar("SyncStatus");
    assert_eq!(sync["text"], json!("Running"));
    assert_eq!(
        stages(sync),
        ["EsNotStarted:done", "EsRunning:current", "EsComplete:todo"]
    );
    let links = sync["c"].as_array().unwrap();
    let shape: Vec<String> = links
        .iter()
        .map(|link| format!("{}:{}", link["id"].as_str().unwrap(), link["btnType"].as_str().unwrap()))
        .collect();
    assert_eq!(
        shape,
        [
            "SyncStatus.EsComplete:primary",
            "SyncStatus.EsNotStarted:",
            "SyncStatus.EsOtherError:danger"
        ]
    );
    let first = &links[0];
    assert_eq!(first["text"], json!("Complete"));
    assert_eq!(first["method"], json!("post"));
    assert_eq!(first["kind"], json!("hidden-form-link"));
    assert_eq!(first["confirm"], json!("Complete: Sync S1?"));
    let url = first["url"].as_str().unwrap();
    for part in ["setStatus", "entitySyncId=S1", "statusId=EsComplete"] {
        assert!(url.contains(part), "{part} in {url}");
    }
    let read_only = bar("ReadOnly");
    assert_eq!(stages(read_only).len(), 3);
    assert!(read_only["c"].as_array().unwrap().is_empty(), "{read_only:?}");
    let staged = bar("Staged");
    assert_eq!(staged["c"][0]["id"], json!("Staged.EsRunning"));
    assert!(staged["c"][0].get("confirm").is_none(), "{staged:?}");
    let allowed = bar("Allowed");
    assert_eq!(allowed["c"].as_array().unwrap().len(), 1);
    assert!(
        allowed["c"][0]["url"]
            .as_str()
            .unwrap()
            .contains("toStatusId=EsComplete"),
        "{allowed:?}"
    );
    let none = bar("None");
    assert_eq!(none["text"], json!(""));
    assert!(none["stages"].as_array().unwrap().is_empty(), "{none:?}");
    let model = client_model(&body);
    assert!(client_nodes(&model).iter().any(|node| matches!(
        node,
        moqui_tui::Node::StatusBar { stages, c, .. } if stages.len() == 3 && c.len() == 3
    )));
}

/// The screenDoc transition answers a terminal (renderMode=tui) with the document as an html node: blocks and links.
#[test]
fn a_screen_document_for_the_terminal_is_an_html_node() {
    let dir = std::env::temp_dir().join(format!("moqui-tui-doc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Doc.xml"),
        r#"<screen require-authentication="false"><widgets><label text="body"/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Help.html"),
        r#"<h1>Help</h1><p>Read <a href="/apps/more">more</a> here.</p>"#,
    )
    .unwrap();
    let root = dir.join("Doc.xml").to_string_lossy().into_owned();
    let world = super::World::open("");
    let mut row = world.facade.make_value("moqui.screen.ScreenDocument").unwrap();
    row.set("screenLocation", Value::string(root.clone()));
    row.set("docIndex", Value::I64(1));
    row.set("docTitle", Value::string("Help"));
    row.set(
        "docLocation",
        Value::string(dir.join("Help.html").to_string_lossy().into_owned()),
    );
    row.create(&world.facade).unwrap();
    let ask = |query: &[(&str, &str)]| {
        let mut get = super::request("GET", "/screenDoc");
        for (name, value) in query {
            get.query.push(((*name).to_string(), (*value).to_string()));
        }
        crate::screen::serve_at(&world.factory, get, &root)
    };
    let response = ask(&[("docIndex", "1"), ("renderMode", "tui")]);
    let body = response.body_text();
    assert_eq!(response.status, 200, "{body}");
    assert!(
        response
            .header("content-type")
            .unwrap_or("")
            .starts_with("application/json"),
        "{:?}",
        response.headers
    );
    let node: Json = serde_json::from_str(&body).unwrap_or_else(|err| panic!("{err}: {body}"));
    assert_eq!(node["t"], json!("html"));
    assert_eq!(node["id"], json!("screenDoc"));
    assert_eq!(node["blocks"][0], json!({"kind": "h1", "text": "Help"}));
    assert!(node["blocks"][1]["text"].as_str().unwrap().contains("more"), "{node}");
    assert_eq!(node["links"][0]["url"], json!("/apps/more"));
    let parsed: moqui_tui::Node = serde_json::from_value(node).unwrap();
    assert!(matches!(parsed, moqui_tui::Node::Html { ref links, .. } if links.len() == 1));
    let html = ask(&[("docIndex", "1")]);
    assert!(html.header("content-type").unwrap_or("").starts_with("text/html"));
    assert!(html.body_text().contains("<h1>Help</h1>"), "{}", html.body_text());
    let missing = ask(&[("docIndex", "9"), ("renderMode", "tui")]);
    assert_eq!(missing.status, 404, "{}", missing.body_text());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A link to a transition, asked for as a `tui` level (Tools' Run Job), answers its redirect as JSON with the messages
/// the transition left: the terminal's next request has no session to keep them in for the screen it goes to.
#[test]
fn a_transition_asked_for_in_tui_answers_its_redirect_as_json() {
    let dir = std::env::temp_dir().join(format!("moqui-tui-redirect-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Root.xml"),
        r#"<screen require-authentication="false">
            <transition name="runIt"><actions><script>ec.message.addMessage("Ran it")</script></actions>
                <default-response url="."><parameter name="thing" value="one"/></default-response></transition>
            <widgets><label text="Root"/></widgets></screen>"#,
    )
    .unwrap();
    let context = ExecutionContextFactory::test_context();
    let output = make_render(context.factory())
        .root_screen(dir.join("Root.xml").to_string_lossy().into_owned())
        .screen_path("runIt")
        .render_mode("tui")
        .render()
        .unwrap_or_else(|err| panic!("{err}"));
    let _ = std::fs::remove_dir_all(&dir);
    let body = output.body_text();
    assert!(!(300..400).contains(&output.status), "{} {body}", output.status);
    let doc: Json = serde_json::from_str(&body).unwrap_or_else(|err| panic!("{err}: {body}"));
    assert!(
        doc["screenUrl"].as_str().is_some_and(|url| url.contains("thing=one")),
        "{body}"
    );
    assert!(body.contains("Ran it"), "{body}");
}

/// A list with a `view-tree` (style-guide §3.15) has the tree among its views: the page's rows with their key, parent, label, the
/// other columns' text and the roll-ups, and the titles of each; the client swaps the list for the `tree` component when that
/// view is chosen. A board of `group-fields` only groups by the first.
#[test]
fn a_tree_view_is_the_rows_with_their_parents_and_a_board_groups_by_the_first_group_field() {
    let xml = r#"<screen require-authentication="false">
        <transition name="moveNode"><default-response type="none"/></transition>
        <actions>
            <set field="nodeList" from="[{nodeId: 'N2', parentId: 'N1', label: 'Sign-in', owner: 'Bo', totalPoints: 8},
                {nodeId: 'N1', parentId: null, label: 'Portal', owner: 'Ada', totalPoints: 10}]"/>
        </actions>
        <widgets>
            <form-list name="Outline" list="nodeList" views="tree,list,board" paginate="false" skip-form="true">
                <view-tree parent-field="parentId" rollup-fields="totalPoints" transition="moveNode"/>
                <view-board group-fields="owner,label"/>
                <field name="nodeId"><default-field><hidden/></default-field></field>
                <field name="label"><default-field title="Item"><display/></default-field></field>
                <field name="owner"><default-field title="Owner"><display/></default-field></field>
                <field name="totalPoints"><default-field title="Total points"><display/></default-field></field>
            </form-list>
        </widgets>
    </screen>"#;
    let output = render("tree-view", xml, BTreeMap::new());
    let body = output.body_text();
    let doc = document(&output);
    let list = list_named(&doc, "Outline");
    let views = &list["views"];
    assert_eq!(views["kinds"], json!(["tree", "list", "board"]), "{body}");
    assert_eq!(views["chosen"], json!("tree"), "{body}");
    let tree = &views["tree"];
    assert_eq!(tree["columns"], json!(["Item", "Owner"]), "{body}");
    assert_eq!(tree["rollups"], json!(["Total points"]), "{body}");
    let first = &tree["rows"][0];
    assert_eq!(
        [
            &first["key"],
            &first["parent"],
            &first["label"],
            &first["tc"],
            &first["tr"]
        ],
        [
            &json!("N2"),
            &json!("N1"),
            &json!("Sign-in"),
            &json!(["Bo"]),
            &json!(["8"])
        ],
        "{body}"
    );
    assert_eq!(tree["rows"][1]["parent"], json!(""), "{body}");
    assert_eq!(views["board"]["rows"][0]["group"], json!("Bo"), "{body}");
    // the client reads it, and the tree component draws the outline
    let model = client_model(&body);
    let found = client_nodes(&model).into_iter().find_map(|node| match node {
        moqui_tui::Node::FormList { list } if list.id.ends_with("Outline") => list.views.map(|views| views.tree),
        _ => None,
    });
    let props = found.expect("the list's tree");
    let lines: Vec<String> = moqui_tui::find_component("tree")
        .expect("the tree component")
        .lines(&props, &moqui_tui::ComponentState::default(), 60, false)
        .iter()
        .map(moqui_tui::line_text)
        .collect();
    assert!(
        lines[1].starts_with("- Portal") && lines[2].starts_with("    Sign-in"),
        "{lines:?}"
    );
}

fn render_in(label: &str, mode: &str, xml: &str) -> RenderOutput {
    let path = std::env::temp_dir().join(format!("moqui-tui-{}-{label}-{mode}.xml", std::process::id()));
    std::fs::write(&path, xml).unwrap();
    let context = ExecutionContextFactory::test_context();
    let output = make_render(context.factory())
        .root_screen(path.to_string_lossy().into_owned())
        .render_mode(mode)
        .render()
        .unwrap_or_else(|err| panic!("{label} {mode}: {err}"));
    let _ = std::fs::remove_file(&path);
    output
}

/// The `m-form`s of a qvt render by id, with the `name` of each widget inside, in order: the submit buttons and the
/// form's own name left out.
fn qvt_forms(body: &str) -> BTreeMap<String, Vec<String>> {
    let attribute = |tag: &str, name: &str| {
        let key = format!(" {name}=\"");
        tag.find(&key).map(|at| {
            let rest = &tag[at + key.len()..];
            rest[..rest.find('"').unwrap_or(rest.len())].to_string()
        })
    };
    let mut forms = BTreeMap::new();
    let mut rest = body;
    while let Some(at) = rest.find("<m-form") {
        rest = &rest[at..];
        let open_end = rest.find('>').unwrap();
        let open = &rest[..open_end];
        if !open[7..].starts_with([' ', '>']) {
            rest = &rest[7..];
            continue;
        }
        let close = rest.find("</m-form>").unwrap_or(rest.len());
        let inner = &rest[open_end..close];
        let mut names = Vec::new();
        for tag in inner.split('<').skip(1) {
            let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
            if tag.starts_with("q-btn") || tag.starts_with('/') {
                continue;
            }
            if let Some(name) = attribute(&format!(" {tag}"), "name") {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        if let Some(id) = attribute(open, "id") {
            forms.insert(id, names);
        }
        rest = &rest[open_end..];
    }
    forms
}

/// What the browser names a tui form by (`domId`) and the fields it posts are the qvt render's: a form in each entry
/// of a section-iterate, a multi form-list (fields `name_N`) and a form per row (`<list>_<row>`), with `writes` where
/// the transition is not read-only (plans/LLM-A2A-Assist.md 4.7). The golden is the list below, which both renders
/// must give.
#[test]
fn form_dom_ids_and_field_names_match_the_qvt_render() {
    let xml = format!(
        r#"<screen require-authentication="false">
        <transition name="saveThing"><actions><set field="saved" value="y"/></actions><default-response type="none"/></transition>
        <transition name="peek" read-only="true"><actions><set field="seen" value="y"/></actions><default-response type="none"/></transition>
        <transition name="saveRows"><actions><set field="saved" value="y"/></actions><default-response type="none"/></transition>
        <transition name="saveRow"><actions><set field="saved" value="y"/></actions><default-response type="none"/></transition>
        <actions><script>{ROWS}</script></actions><widgets>
        <form-single name="EditThing" transition="saveThing">
            <field name="description"><default-field><text-line/></default-field></field>
            <field name="statusId"><default-field><drop-down><option key="A"/><option key="B"/></drop-down></default-field></field>
            <field name="submitButton"><default-field title="Save"><submit/></default-field></field>
        </form-single>
        <section-iterate name="Entries" list="['x','y']" entry="entry"><widgets>
            <form-single name="EntryNote" transition="peek">
                <field name="note"><default-field><text-line/></default-field></field>
            </form-single>
        </widgets></section-iterate>
        <form-list name="Multi" list="rows" transition="saveRows" multi="true">
            <field name="id"><default-field><hidden/></default-field></field>
            <field name="name"><default-field><text-line/></default-field></field>
            <field name="submitButton"><default-field title="Save All"><submit/></default-field></field>
        </form-list>
        <form-list name="Each" list="rows" transition="saveRow">
            <field name="id"><default-field><hidden/></default-field></field>
            <field name="name"><default-field><text-line/></default-field></field>
            <field name="go"><default-field title="Go"><submit/></default-field></field>
        </form-list>
    </widgets></screen>"#
    );
    let golden: BTreeMap<String, (Vec<String>, bool)> = [
        ("EditThing", vec!["description", "statusId"], true),
        ("EntryNote_0", vec!["note"], false),
        ("EntryNote_1", vec!["note"], false),
        ("Multi", vec!["name_0", "name_1"], true),
        ("Each_0", vec!["name"], true),
        ("Each_1", vec!["name"], true),
    ]
    .into_iter()
    .map(|(id, names, writes)| {
        (
            id.to_string(),
            (names.into_iter().map(str::to_string).collect(), writes),
        )
    })
    .collect();

    let tui = render_in("dom-ids", "tui", &xml);
    let model = client_model(&tui.body_text());
    let index = moqui_tui::Index::of([&model]);
    let tui_forms: BTreeMap<String, (Vec<String>, bool)> = index
        .forms
        .values()
        .filter(|form| !form.dom_id.is_empty())
        .map(|form| {
            let names = form
                .fields
                .iter()
                .filter_map(|id| index.fields.get(id))
                .flat_map(moqui_tui::parameter_keys)
                .map(|(name, _)| name)
                .collect();
            (form.dom_id.clone(), (names, form.writes))
        })
        .collect();
    assert_eq!(tui_forms, golden, "{}", tui.body_text());

    let qvt = render_in("dom-ids", "qvt", &xml);
    let body = qvt.body_text();
    assert_eq!(qvt.status, 200, "{body}");
    let qvt_forms = qvt_forms(&body);
    let qvt_golden: BTreeMap<String, Vec<String>> = golden
        .iter()
        .map(|(id, (names, _))| (id.clone(), names.clone()))
        .collect();
    assert_eq!(qvt_forms, qvt_golden, "{body}");
    // qvt marks the read-only transition's form, the agent's writes are the others
    for (id, (_, writes)) in &golden {
        let open = &body[body
            .find(&format!("<m-form name=\"{id}\""))
            .unwrap_or_else(|| panic!("{id}: {body}"))..];
        let open = &open[..open.find('>').unwrap()];
        assert_eq!(open.contains("read-only"), id.starts_with("EntryNote"), "{open}");
        let _ = writes;
    }

    // a link to a transition that writes says so; one to a read-only transition or a screen does not
    let links = render_in(
        "writes-links",
        "tui",
        r#"<screen require-authentication="false">
        <transition name="deleteIt"><actions><set field="gone" value="y"/></actions><default-response type="none"/></transition>
        <transition name="peek" read-only="true"><actions><set field="seen" value="y"/></actions><default-response type="none"/></transition>
        <widgets>
            <link url="deleteIt" text="Delete"/>
            <link url="peek" text="Peek"/>
            <link url="." text="Here" url-type="screen"/>
        </widgets></screen>"#,
    );
    let doc = document(&links);
    let writes: Vec<(String, bool)> = nodes(&doc)
        .into_iter()
        .filter(|node| node.get("t").and_then(Json::as_str) == Some("link"))
        .map(|node| {
            (
                node["text"].as_str().unwrap_or_default().to_string(),
                node.get("writes") == Some(&json!(true)),
            )
        })
        .collect();
    assert_eq!(
        writes,
        [
            ("Delete".to_string(), true),
            ("Peek".to_string(), false),
            ("Here".to_string(), false)
        ],
        "{doc}"
    );
}

/// A drop-down's `no-current-selected-key` given as an expression (a widget template's `${noCurrentSelectedKey ||
/// ''}`) is expanded: its value, or nothing, never the expression's text.
#[test]
fn a_no_current_selected_key_expression_is_expanded() {
    for (actions, expected) in [("", ""), (r#"<set field="dflt" value="B"/>"#, "B")] {
        let output = render(
            "no-current-key",
            &format!(
                r#"<screen require-authentication="false"><actions>{actions}</actions><widgets>
                <form-single name="Pick" transition="pick">
                    <field name="kind"><default-field><drop-down allow-empty="true" no-current-selected-key="${{dflt || ''}}">
                        <option key="A" text="Alpha"/><option key="B" text="Beta"/></drop-down></default-field></field>
                </form-single>
            </widgets></screen>"#
            ),
            BTreeMap::new(),
        );
        let body = output.body_text();
        assert!(!body.contains("dflt"), "{body}");
        let doc = document(&output);
        let field = nodes(&doc)
            .into_iter()
            .find(|node| node.get("name").and_then(Json::as_str) == Some("kind"))
            .unwrap_or_else(|| panic!("{doc}"));
        assert_eq!(field["w"]["value"], json!(expected), "{field:?}");
    }
}
