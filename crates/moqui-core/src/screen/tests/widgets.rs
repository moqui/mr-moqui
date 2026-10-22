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

//! Render-level tests for the text, csv and xml macro modules. The expected text is derived from
//! `DefaultScreenMacros.{text,csv,xml}.ftl`: what each macro writes, and where FreeMarker kept or
//! removed whitespace.

use std::collections::BTreeMap;

use moqui_lib::object::Value;

use super::World;
use crate::context::{Error, ExecutionContextFactory};
use crate::screen::{make_render, RenderOutput};

fn render_root() -> String {
    format!("{}/src/screen/testdata/Render/Render.xml", env!("CARGO_MANIFEST_DIR"))
}

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::string(*value)))
        .collect()
}

fn render_with(
    factory: &std::sync::Arc<ExecutionContextFactory>,
    path: &str,
    parameters: &[(&str, &str)],
) -> Result<RenderOutput, Error> {
    make_render(factory)
        .root_screen(render_root())
        .screen_path(path)
        .parameters(params(parameters))
        .render()
}

fn body(factory: &std::sync::Arc<ExecutionContextFactory>, path: &str, parameters: &[(&str, &str)]) -> String {
    let output = render_with(factory, path, parameters).unwrap_or_else(|err| panic!("{path}: {err}"));
    assert_eq!(output.status, 200, "{path}");
    output.body_text()
}

/// A text-mode cell: the text padded on the right to the 132 character line.
fn line(text: &str) -> String {
    format!("{text:<132}")
}

/// A text-mode form-single line: the title right-aligned in 26 characters, a colon, and the value in 105.
fn field(title: &str, value: &str) -> String {
    format!("{title:>26}: {value:<105}\n")
}

#[test]
fn labels_containers_and_sections_in_text() {
    let context = ExecutionContextFactory::test_context();
    let expected = [
        line("Hello Ada"),
        line("in container"),
        "\n".to_string(),
        // A container-box shows its header at the full line width, then a line break, then its body.
        line("Header"),
        "\n".to_string(),
        line("Body"),
        line("in section"),
        line("a-1-0"),
        line("b-2-1"),
    ]
    .concat();
    assert_eq!(body(context.factory(), "/Labels.text", &[]), expected);
}

#[test]
fn labels_containers_and_sections_in_csv_and_xml() {
    let context = ExecutionContextFactory::test_context();
    // csv writes no labels, in a box or a section or anywhere: only a form-list has a place in it.
    assert_eq!(body(context.factory(), "/Labels.csv", &[]), "");
    // xml writes the screen element for each screen, and a line break around the widgets of each section.
    assert_eq!(
        body(context.factory(), "/Labels.xml", &[]),
        "<Render>\n<Labels>\n\n\n\n\n\n\n</Labels>\n</Render>\n"
    );
}

#[test]
fn a_form_single_and_a_form_list_in_text() {
    let context = ExecutionContextFactory::test_context();
    let text = body(context.factory(), "/Forms.text", &[]);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let single = [
        field("Item ID", "I1"),
        field("Item Name", "Widget, \"large\""),
        field("Status", "Active"),
        field("Flag", "Y"),
        field("Amount", "$1,234.50"),
        field("When Stamp", "2024-03-05 14:07"),
        // a hidden field leaves its line empty
        "\n".to_string(),
        field("Mode", "flag is Y"),
        // a submit leaves its line empty, and the form ends with an empty line
        "\n".to_string(),
        "\n".to_string(),
    ];
    assert_eq!(lines[..single.len()].concat(), single.concat());
    // 5 columns share 132 - 4 separators = 128 characters: 25, 25, 26, 26, 26 after rounding.
    let widths = [25, 25, 26, 26, 26];
    let cell = |text: &str, width: usize, right: bool| {
        if right {
            format!("{text:>width$}")
        } else {
            format!("{text:<width$}")
        }
    };
    let header = [
        cell("Item ID", widths[0], false),
        cell("Name", widths[1], false),
        cell("Price", widths[2], true),
        cell("Note", widths[3], false),
        cell("Do", widths[4], false),
    ]
    .join("|");
    let rule = widths
        .iter()
        .map(|width| "-".repeat(*width))
        .collect::<Vec<_>>()
        .join("+");
    // The submit field has a header but writes no cell, so a row ends at the separator before it.
    let row = |id: &str, name: &str, price: &str, note: &str| {
        format!(
            "{}|{}|{}|{}|\n",
            cell(id, widths[0], false),
            cell(name, widths[1], false),
            cell(price, widths[2], true),
            cell(note, widths[3], false)
        )
    };
    let list = [
        format!("{header}\n"),
        format!("{rule}\n"),
        row("I1", "Widget, \"large\"", "12.50", "first line"),
        row("I2", "Gadget", "3.00", ""),
        "\n".to_string(),
    ]
    .concat();
    assert_eq!(lines[single.len()..].concat(), list);
}

#[test]
fn a_form_list_in_csv_and_xml_and_a_form_single_in_neither() {
    let context = ExecutionContextFactory::test_context();
    // csv quotes a value with a comma or a quote and doubles its quotes. The submit has a title
    // in the header and no cell, as in the FreeMarker macro.
    assert_eq!(
        body(context.factory(), "/Forms.csv", &[]),
        "Item ID,Name,Price,Note,Do\nI1,\"Widget, \"\"large\"\"\",12.50,first line\nI2,Gadget,3.00,\n"
    );
    // xml writes an entry for each row with an attribute for each displayed field.
    assert_eq!(
        body(context.factory(), "/Forms.xml", &[]),
        "<Render>\n<Forms>\n    <ItemList>\n\
         \x20       <ItemListEntry itemId=\"I1\" name=\"Widget, &quot;large&quot;\" price=\"12.50\" note=\"first line\"/>\n\
         \x20       <ItemListEntry itemId=\"I2\" name=\"Gadget\" price=\"3.00\" note=\"\"/>\n\
         \x20   </ItemList>\n</Forms>\n</Render>\n"
    );
}

#[cfg(feature = "xlsx")]
#[test]
fn a_form_list_in_xlsx_is_a_sheet_with_typed_cells() {
    use calamine::{Data, Reader};
    let context = ExecutionContextFactory::test_context();
    let output = render_with(context.factory(), "/Forms.xlsx", &[("saveFilename", "Forms.xlsx")]).unwrap();
    assert_eq!(output.status, 200);
    assert_eq!(
        output.content_type(),
        Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
    );
    assert!(output.header("Content-Disposition").unwrap().contains("Forms.xlsx"));
    let mut book = calamine::open_workbook_auto_from_rs(std::io::Cursor::new(output.body)).unwrap();
    // the form-single is not in the workbook, the form-list is one sheet
    assert_eq!(book.sheet_names(), vec!["ItemList".to_string()]);
    let range = book.worksheet_range("ItemList").unwrap();
    let row = |index: u32| -> Vec<Data> {
        (0..5)
            .map(|col| range.get_value((index, col)).cloned().unwrap_or(Data::Empty))
            .collect()
    };
    let text = |value: &str| Data::String(value.to_string());
    assert_eq!(
        row(0),
        vec![text("Item ID"), text("Name"), text("Price"), text("Note"), text("Do")]
    );
    // the price is a number, not the formatted text, and the submit is an empty cell
    assert_eq!(
        row(1),
        vec![
            text("I1"),
            text("Widget, \"large\""),
            Data::Float(12.5),
            text("first line"),
            Data::Empty
        ]
    );
    assert_eq!(row(2)[..3], [text("I2"), text("Gadget"), Data::Float(3.0)]);
}

#[test]
fn sections_fail_widgets_include_screen_and_text() {
    let context = ExecutionContextFactory::test_context();
    let included = "src/screen/testdata/Render/Render/Included.xml";
    let text = body(context.factory(), "/Sections.text", &[("dynamicLocation", included)]);
    let expected = [
        // the section condition fails, so its fail-widgets render
        line("failed"),
        // include-screen with its own scope: the included screen's set is not visible after it
        line("included Grace"),
        line("shared Grace"),
        line("after null"),
        // with share-scope the set stays
        line("included Grace"),
        line("shared Grace"),
        line("after yes"),
        // section-include of a section of another screen, and a dynamic one
        line("shared Grace"),
        line("shared Grace"),
        // render-mode picks the text for the mode, and an untemplated text is written as is
        "text-only Grace".to_string(),
        "plain ${who}".to_string(),
        "template Grace".to_string(),
        // an include-screen that fails writes the error as text and the render goes on
        "Error rendering screen [src/screen/testdata/Render/Render/Nope.xml]: screen not found: src/screen/testdata/Render/Render/Nope.xml"
            .to_string(),
        // a section whose actions add an error writes the error instead of its widgets
        "oops Grace".to_string(),
        line("after the error"),
    ]
    .concat();
    // The first lines are padded labels and the text after them is not, so compare in two parts.
    assert_eq!(text, expected);
    let csv = body(context.factory(), "/Sections.csv", &[("dynamicLocation", included)]);
    assert!(csv.contains("csv-only"), "{csv}");
    assert!(!csv.contains("text-only"), "{csv}");
    assert!(!csv.contains("plain"), "{csv}");
    let xml = body(context.factory(), "/Sections.xml", &[("dynamicLocation", included)]);
    assert!(xml.contains("plain ${who}"), "{xml}");
    assert!(!xml.contains("csv-only") && !xml.contains("text-only"), "{xml}");
}

fn enumeration_world() -> World {
    let world = World::open("");
    let create = |entity: &str, fields: &[(&str, Value)]| {
        let mut row = world.facade.make_value(entity).unwrap();
        for (name, value) in fields {
            row.set(*name, value.clone());
        }
        row.create(&world.facade).unwrap();
    };
    create(
        "moqui.basic.EnumerationType",
        &[
            ("enumTypeId", Value::string("TypeA")),
            ("description", Value::string("Type A")),
        ],
    );
    create(
        "moqui.basic.EnumerationType",
        &[
            ("enumTypeId", Value::string("TypeB")),
            ("description", Value::string("Type B")),
            ("parentEnumTypeId", Value::string("TypeA")),
        ],
    );
    for (id, description, sequence) in [("E2", "second", 2), ("E1", "first", 1), ("E3", "third", 3)] {
        create(
            "moqui.basic.Enumeration",
            &[
                ("enumId", Value::string(id)),
                ("enumTypeId", Value::string("TypeB")),
                ("description", Value::string(description)),
                ("sequenceNum", Value::I64(sequence)),
            ],
        );
    }
    world
}

#[test]
fn entity_values_lists_options_and_display_entity() {
    let world = enumeration_world();
    let context = world.factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let text = body(&world.factory, "/Entities.text", &[("enumTypeId", "TypeB")]);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    assert_eq!(lines[0], field("Enum Type ID", "TypeB"));
    assert_eq!(lines[1], field("Description", "Type B"));
    // display-entity: the related value with the text expanded against it, or the default text
    assert_eq!(lines[2], field("Type Text", "Type: Type B"));
    assert_eq!(lines[3], field("Missing", "no such type"));
    // drop-down: the option text of the current value; there is none, so the no-current-selected-key
    // is the value, and it is not an option so it is shown as it is
    assert_eq!(lines[4], field("Pick", "B"));
    assert!(lines[5].trim().is_empty(), "{lines:?}");
    // 3 columns: 132 - 2 = 130 characters, 43.33 each: 43 each after rounding and the one left over goes to the first
    let rows: Vec<&str> = lines[6..].to_vec();
    let header = format!("{:<44}|{:<43}|{:<43}\n", "Enum ID", "Upper", "Enum Type ID");
    assert_eq!(rows[0], header);
    // rows come in sequence order, row-actions ran, and display-entity found the type
    assert_eq!(rows[2], format!("{:<44}|{:<43}|{:<43}\n", "E1", "FIRST", "Type B"));
    assert_eq!(rows[3], format!("{:<44}|{:<43}|{:<43}\n", "E2", "SECOND", "Type B"));
    assert_eq!(rows[4], format!("{:<44}|{:<43}|{:<43}\n", "E3", "THIRD", "Type B"));

    let csv = body(&world.factory, "/Entities.csv", &[("enumTypeId", "TypeB")]);
    assert_eq!(
        csv,
        "Enum ID,Upper,Enum Type ID\nE1,FIRST,Type B\nE2,SECOND,Type B\nE3,THIRD,Type B\n"
    );
    let xml = body(&world.factory, "/Entities.xml", &[("enumTypeId", "TypeB")]);
    assert!(
        xml.contains("<EnumListEntry enumId=\"E1\" upper=\"FIRST\" enumTypeId=\"Type B\"/>"),
        "{xml}"
    );
}

#[test]
fn entity_value_fields_read_by_property_in_conditions_and_text() {
    let world = enumeration_world();
    let context = world.factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let text = body(&world.factory, "/DotFields.text", &[("enumTypeId", "TypeB")]);
    let lines = [
        "Type B! with 3",
        "first first",
        "E2 second of Type B",
        "E3 third of Type B",
        "3 3 E3",
    ];
    // The condition read a field and a many relationship by property, and the action's assignment by property
    // changed the value. In the loop: a field by property in the condition, by index, and a relationship name in
    // the text. Then the list by size, length and index.
    assert_eq!(text, lines.map(line).concat());
}

#[test]
fn a_localized_message_changes_a_label() {
    let world = World::open("");
    let mut row = world.facade.make_value("moqui.basic.LocalizedMessage").unwrap();
    row.set("original", Value::string("Hello ${who}"));
    row.set("locale", Value::string("fr"));
    row.set("localized", Value::string("Bonjour ${who}"));
    row.create(&world.facade).unwrap();
    let context = world.factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    assert!(body(&world.factory, "/Labels.text", &[]).starts_with(&line("Hello Ada")));
    crate::user::session::set_locale(Some("fr_CA"));
    let french = body(&world.factory, "/Labels.text", &[]);
    assert!(french.starts_with(&line("Bonjour Ada")), "{french}");
    crate::user::session::set_locale(None);
}

#[test]
fn boundary_comments_follow_the_conf_and_skip_the_first_screen() {
    let conf = "<moqui-conf><screen-facade boundary-comments=\"true\"/></moqui-conf>";
    let world = World::open(conf);
    let context = world.factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let text = body(
        &world.factory,
        "/Sections.text",
        &[("dynamicLocation", "src/screen/testdata/Render/Render/Included.xml")],
    );
    assert!(text.contains("<!-- BEGIN render-mode.text[inline][@template=false] -->plain ${who}<!-- END   render-mode.text[inline][@template=false] -->"), "{text}");
    assert!(
        text.contains("<!-- BEGIN render-mode.text[inline][@template=true] -->template Grace<!-- END"),
        "{text}"
    );
}

#[test]
fn forms_extend_include_widget_templates_and_add_entity_fields() {
    let context = ExecutionContextFactory::test_context();
    context.factory().set_entity(crate::testing::open());
    let text = body(context.factory(), "/Assembly.text", &[]);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    // BaseForm as written
    assert_eq!(lines[0], field("Base", "b1"));
    assert_eq!(lines[1], field("Base Own", "o1"));
    assert_eq!(lines[2], "\n");
    // ChildForm extends it: the base field, its own field with the child's title, and a field whose widgets
    // come from the widget-template-include, so its drop-down shows the text of the option N
    assert_eq!(lines[3], field("Base", "b1"));
    assert_eq!(lines[4], field("Child Own", "o1"));
    assert_eq!(lines[5], field("Yn", "No"));
    assert_eq!(lines[6], "\n");
    // AutoForm: auto-fields-entity of the non-primary-key fields of Enumeration, not enumCode, in the
    // entity's order, with the form's own field in the place of the generated one
    let titles: Vec<String> = lines[7..]
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line[..26].trim().to_string())
        .collect();
    assert_eq!(
        titles,
        [
            "Enum Type ID",
            "Parent Enum ID",
            "Sequence Num",
            "Changed",
            "Option Value",
            "Option Indicator",
            "Related Enum ID",
            "Related Enum Type ID",
            "Status Flow ID"
        ]
    );
    assert_eq!(lines[7 + 2], field("Sequence Num", "5"));
    assert_eq!(lines[7 + 3], field("Changed", "Desc"));
}

#[test]
fn sri_methods_called_from_actions() {
    let context = ExecutionContextFactory::test_context();
    context.factory().set_entity(crate::testing::open());
    let text = body(context.factory(), "/Sri.text", &[("who", "w")]);
    let (json, tail) = text.split_once("\n<script>").expect("script writer text follows");
    assert_eq!(tail, "\nscript1\n</script>\n");
    let Value::Map(map) = crate::service::parse_json(json).unwrap() else {
        panic!("{json}")
    };
    let text_of =
        |name: &str| moqui_lib::object::to_plain_string(map.get(name).unwrap_or_else(|| panic!("{name} in {json}")));
    assert_eq!(text_of("mode"), "text");
    assert_eq!(text_of("boundary"), "false");
    assert_eq!(text_of("active"), "Sri");
    // buildUrl is relative to the active screen, and a path starting with .. goes up
    assert_eq!(
        (text_of("path"), text_of("url"), text_of("readOnly")),
        ("/Forms".into(), "/Forms".into(), "true".into())
    );
    assert_eq!(text_of("clone"), "/Forms?a=b+x&c=1");
    assert_eq!(text_of("withParam"), "/Forms");
    // A plain URL is the url as written. Its path is empty: Java's getPath is the screen path, and a plain URL has none.
    assert_eq!(
        (text_of("made"), text_of("madePath")),
        ("/go?x=1".into(), String::new())
    );
    assert_eq!(text_of("trans"), "/Sri/go?p=1&q=two&who=w");
    assert_eq!(
        (text_of("transReadOnly"), text_of("changeReadOnly")),
        ("true".into(), "false".into())
    );
    assert_eq!(
        (
            text_of("anchor"),
            text_of("anchorAuto"),
            text_of("anchorAutoTransition"),
            text_of("anchorPlain")
        ),
        ("true".into(), "true".into(), "false".into(), "true".into())
    );
    assert_eq!(
        (text_of("current"), text_of("currentSame")),
        ("/Sri".into(), "/Sri".into())
    );
    assert_eq!(text_of("inner"), "inner");
    assert_eq!(text_of("innerAfter"), "object");
    assert_eq!(
        (text_of("viaSet"), text_of("viaFrom"), text_of("viaDefault")),
        ("v-from context".into(), "42".into(), "dd".into())
    );
    assert_eq!(
        (text_of("after"), text_of("afterAgain")),
        ("after1after2".into(), String::new())
    );
    assert_eq!(text_of("included"), "Named");
    assert_eq!(text_of("encode"), "&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;");
    assert_eq!(text_of("activeInMenu"), "false");
    let Some(Value::Map(values)) = map.get("values") else {
        panic!("{json}")
    };
    let value = |name: &str| moqui_lib::object::to_plain_string(&values[name]);
    // the form's map, then the context, then the expanded default
    assert_eq!(
        (value("fromMap"), value("fromMapName"), value("fromContext")),
        ("12.5".into(), "Ada".into(), "from context".into())
    );
    assert_eq!(value("defaulted"), "dflt from context");
    // format applies to a number, and text-format wins in text mode
    assert_eq!(
        (value("formatted"), value("textFormat")),
        ("12.500".into(), "12".into())
    );
    assert_eq!(
        (value("plain"), value("plainDefault"), value("valueClass")),
        ("12.5".into(), "dfrom context".into(), "Double".into())
    );
    assert_eq!(
        (value("from"), value("named"), value("namedContext")),
        ("3".into(), "Ada".into(), "from context".into())
    );
    // options: option, list-options with key and text, and plain items, in order
    let Some(Value::Map(options)) = map.get("options") else {
        panic!("{json}")
    };
    let pairs: Vec<String> = match &options["option"] {
        Value::List(items) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        pairs,
        ["Y,Yes", "from context,from context", "a,Alpha", "b,Beta", "p,p", "q,q"]
    );
    let Some(Value::Map(single)) = map.get("single") else {
        panic!("{json}")
    };
    assert_eq!(single.len(), 2);
    let Some(Value::Map(hidden)) = map.get("hidden") else {
        panic!("{json}")
    };
    assert_eq!(hidden.len(), 1);
}

#[test]
fn theme_values_and_icon_classes_come_from_the_theme_entities() {
    let world = World::open("");
    let create = |entity: &str, fields: &[(&str, Value)]| {
        let mut row = world.facade.make_value(entity).unwrap();
        for (name, value) in fields {
            row.set(*name, value.clone());
        }
        row.create(&world.facade).unwrap();
    };
    create(
        "moqui.basic.Enumeration",
        &[
            ("enumId", Value::string("STT_INTERNAL")),
            ("enumCode", Value::string("TEST_THEME")),
        ],
    );
    create(
        "moqui.screen.ScreenTheme",
        &[
            ("screenThemeId", Value::string("TEST_THEME")),
            ("screenThemeTypeEnumId", Value::string("STT_INTERNAL")),
        ],
    );
    for (sequence, value) in [(2, "/second.css"), (1, "/first.css")] {
        create(
            "moqui.screen.ScreenThemeResource",
            &[
                ("screenThemeResourceId", Value::string(format!("R{sequence}"))),
                ("screenThemeId", Value::string("TEST_THEME")),
                ("resourceTypeEnumId", Value::string("STRT_STYLESHEET")),
                ("sequenceNum", Value::I64(sequence)),
                ("resourceValue", Value::string(value)),
            ],
        );
    }
    create(
        "moqui.screen.ScreenThemeIcon",
        &[
            ("screenThemeIconId", Value::string("I1")),
            ("screenThemeId", Value::string("TEST_THEME")),
            ("textPattern", Value::string(".*Delete.*")),
            ("iconClass", Value::string("fa fa-trash")),
        ],
    );
    let context = world.factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let screen = std::env::temp_dir().join(format!("moqui-theme-{}.xml", std::process::id()));
    std::fs::write(
        &screen,
        r#"<screen require-authentication="false"><actions><script>
            ec.web.sendJsonResponse({styles: sri.getThemeValues('STRT_STYLESHEET'), none: sri.getThemeValues('STRT_OTHER'),
                trash: sri.getThemeIconClass('Delete it'), plain: sri.getThemeIconClass('Other')})
        </script></actions><widgets/></screen>"#,
    )
    .unwrap();
    let output = make_render(&world.factory)
        .root_screen(screen.to_string_lossy().into_owned())
        .render_mode("text")
        .render()
        .unwrap();
    let _ = std::fs::remove_file(&screen);
    let Value::Map(map) = crate::service::parse_json(&output.body_text()).unwrap() else {
        panic!()
    };
    assert_eq!(
        moqui_lib::object::to_plain_string(&map["styles"]),
        "/first.css,/second.css"
    );
    assert_eq!(moqui_lib::object::to_plain_string(&map["none"]), "");
    assert_eq!(moqui_lib::object::to_plain_string(&map["trash"]), "fa fa-trash");
    assert!(map["plain"].is_null());
}

#[test]
fn a_dynamic_form_is_assembled_from_the_context_each_time_it_renders() {
    let context = ExecutionContextFactory::test_context();
    context.factory().set_entity(crate::testing::open());
    let types = body(
        context.factory(),
        "/Dynamic.text",
        &[("entityName", "moqui.basic.EnumerationType")],
    );
    assert_eq!(types, format!("{}\n", field("Description", "Dyn type")));
    let enums = body(
        context.factory(),
        "/Dynamic.text",
        &[("entityName", "moqui.basic.Enumeration")],
    );
    assert!(enums.contains(&field("Sequence Num", "7")), "{enums}");
    assert!(enums.contains("Parent Enum ID"), "{enums}");
    // the same entity again makes the same code: one unit for each different assembled form
    let again = body(
        context.factory(),
        "/Dynamic.text",
        &[("entityName", "moqui.basic.EnumerationType")],
    );
    assert_eq!(again, types);
    let csv = body(
        context.factory(),
        "/Dynamic.csv",
        &[("entityName", "moqui.basic.EnumerationType")],
    );
    assert_eq!(csv, "");
}

/// A section-include inside a section that is itself included from another file: what the inner include brings (its
/// section's own nested sections and forms) is found from the screen that renders, as Java's recursive
/// `pullSectionInclude` pulls it.
#[test]
fn a_section_include_inside_an_included_section_finds_the_nested_sections_of_what_it_includes() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    let dir = std::env::temp_dir().join(format!("moqui-nested-include-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    std::fs::create_dir_all(dir.join("Parts")).unwrap();
    let mid = dir.join("Parts/Mid.xml").to_string_lossy().into_owned();
    let inner = dir.join("Parts/Inner.xml").to_string_lossy().into_owned();
    std::fs::write(
        dir.join("Root.xml"),
        "<screen require-authentication=\"false\"><widgets><subscreens-active/></widgets><subscreens default-item=\"Outer\"/></screen>",
    )
    .unwrap();
    std::fs::write(
        dir.join("Root/Outer.xml"),
        format!(
            "<screen require-authentication=\"false\"><actions><set field=\"who\" value=\"Ada\"/></actions><widgets>\
             <section-include name=\"Mid\" location=\"{mid}\"/></widgets></screen>"
        ),
    )
    .unwrap();
    std::fs::write(
        &mid,
        format!(
            "<screen require-authentication=\"false\"><widgets><section name=\"Mid\"><widgets><label text=\"mid WHO\"/>\
             <section-include name=\"Inner\" location=\"{inner}\"/>\
             <section-include location=\"{inner}#Other\"/></widgets></section></widgets></screen>"
        )
        .replace("WHO", "${who}"),
    )
    .unwrap();
    std::fs::write(
        &inner,
        "<screen require-authentication=\"false\"><widgets>\
         <section name=\"Inner\"><widgets><label text=\"inner ${who}\"/>\
           <section name=\"InnerChild\"><widgets><label text=\"child ${who}\"/></widgets></section>\
           <section-iterate name=\"InnerRows\" list=\"['a', 'b']\" entry=\"letter\"><widgets><label text=\"row ${letter}\"/></widgets></section-iterate>\
           <form-single name=\"InnerForm\"><field name=\"note\"><default-field title=\"Inner note\"><display text=\"noted\"/></default-field></field></form-single>\
         </widgets></section>\
         <section name=\"Other\"><widgets><section name=\"OtherChild\"><widgets><label text=\"other child\"/></widgets></section></widgets></section>\
         </widgets></screen>",
    )
    .unwrap();
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let output = make_render(factory)
        .root_screen(root)
        .screen_path("/Outer.text")
        .render()
        .unwrap();
    let text = output.body_text();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!text.contains("No section with name"), "{text}");
    for expected in [
        "mid Ada",
        "inner Ada",
        "child Ada",
        "row a",
        "row b",
        "Inner note",
        "noted",
        "other child",
    ] {
        assert!(text.contains(expected), "{expected} in {text}");
    }
}
