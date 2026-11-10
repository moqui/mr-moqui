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

use moqui_lib::node::MNode;

use moqui_db::model::EntityModel;

use super::{add_entity_fields, merge_field_node, merge_form_nodes, read_entity_shape};

fn parse(xml: &str) -> MNode {
    MNode::parse_text("test", xml).unwrap()
}

fn refs_in(node: &MNode) -> Vec<String> {
    node.children()
        .iter()
        .filter(|child| child.has_name("field-ref"))
        .map(|child| child.attribute("name").unwrap_or_default())
        .collect()
}

/// A field an extension adds with `after` lands after that field in the layout, inside its row
/// (plans/design-dev-aide.md F16); without `after`, or with an anchor that is not there, it goes last.
#[test]
fn an_extension_field_goes_after_the_named_field_in_the_layout() {
    let form = parse(
        r#"<form-single name="F"><field name="a"/><field name="b"/><field name="c"/>
                <field-layout><field-row><field-ref name="a"/><field-ref name="b"/></field-row><field-ref name="c"/></field-layout>
            </form-single>"#,
    );
    let extension = parse(
        r#"<form-single name="F"><field name="due" after="a"/><field name="tail"/><field name="lost" after="nope"/></form-single>"#,
    );
    merge_form_nodes(&form, &extension, true, true);
    let layout = form.first_named("field-layout").unwrap();
    let row = layout.first_named("field-row").unwrap();
    assert_eq!(refs_in(&row), ["a", "due", "b"]);
    assert_eq!(refs_in(&layout), ["c", "tail", "lost"]);
}

/// A form-list with `form-list-column`s gives a new field a column after the column of its `after` field.
#[test]
fn an_extension_field_gets_a_column_after_the_named_one() {
    let form = parse(
        r#"<form-list name="L" list="rows"><field name="a"/><field name="b"/>
                <form-list-column><field-ref name="a"/></form-list-column>
                <form-list-column><field-ref name="b"/></form-list-column>
            </form-list>"#,
    );
    let extension = parse(r#"<form-list name="L"><field name="due" after="a"/><field name="end"/></form-list>"#);
    merge_form_nodes(&form, &extension, true, true);
    let columns: Vec<Vec<String>> = form.children_named("form-list-column").iter().map(refs_in).collect();
    assert_eq!(columns, [vec!["a"], vec!["due"], vec!["b"], vec!["end"]]);
}

/// The column sets worked out once give what [`super::list_column_fields`] gives for each `_uiType`.
#[test]
fn list_columns_kept_with_the_form_match_the_fields_read_from_it() {
    let form = parse(
        r#"<form-list name="L" list="rows">
                <field name="a"><default-field><display/></default-field></field>
                <field name="b" hide="hideB"><default-field><display/></default-field></field>
                <field name="c"><default-field><hidden/></default-field></field>
                <field name="d"><default-field><hidden default-value="x"/></default-field></field>
                <columns type="mobile"><column><field-ref name="b"/></column></columns>
                <columns type="mobile"><column><field-ref name="a"/></column></columns>
                <columns type=""><column><field-ref name="a"/><field-ref name="c"/></column></columns>
            </form-list>"#,
    );
    let columns = super::ListColumns::of(&form);
    for ui_type in [None, Some(""), Some("mobile"), Some("desktop")] {
        assert_eq!(
            columns.for_ui_type(ui_type),
            super::list_column_fields(&form, ui_type).as_slice(),
            "{ui_type:?}"
        );
    }
    assert_eq!(
        columns.for_ui_type(Some("mobile")),
        [("b".to_string(), Some("hideB".to_string()))]
    );
    assert_eq!(
        columns.hidden_with_default,
        super::hidden_list_fields_with_default(&form)
    );
}

#[test]
fn an_override_form_replaces_attributes_the_entity_find_and_the_layout() {
    let base = parse(
        r#"<form-list name="A" list="one"><entity-find entity-name="X"/><row-actions><set field="a"/></row-actions>
               <field-layout><field-ref name="f"/></field-layout><field name="f"><default-field title="F"><display/></default-field></field></form-list>"#,
    );
    let over = parse(
        r#"<form-list name="A" list="two"><entity-find entity-name="Y"/><row-actions><set field="b"/></row-actions>
               <field-layout><field-ref name="g"/></field-layout>
               <field name="f"><default-field title="Changed"/></field><field name="g"><default-field><text-line/></default-field></field></form-list>"#,
    );
    merge_form_nodes(&base, &over, true, true);
    assert_eq!(base.attribute("list").as_deref(), Some("two"));
    let finds = base.children_named("entity-find");
    assert_eq!(finds.len(), 1);
    assert_eq!(finds[0].attribute("entity-name").as_deref(), Some("Y"));
    // row-actions of both run
    let actions: Vec<String> = base
        .first_named("row-actions")
        .unwrap()
        .children()
        .iter()
        .filter_map(|node| node.attribute("field"))
        .collect();
    assert_eq!(actions, ["a", "b"]);
    // the layout is the override's, with no field-ref added for a field it already names
    let layout = base.first_named("field-layout").unwrap();
    assert_eq!(
        layout
            .children()
            .iter()
            .filter_map(|node| node.attribute("name"))
            .collect::<Vec<_>>(),
        ["g"]
    );
    // the field of the same name is merged in place, the new one is appended
    let fields = base.children_named("field");
    assert_eq!(fields.len(), 2);
    let default = fields[0].first_named("default-field").unwrap();
    assert_eq!(default.attribute("title").as_deref(), Some("Changed"));
    assert!(
        default.first_named("display").is_some(),
        "a default-field without children keeps the base widgets"
    );
}

#[test]
fn conditional_fields_merge_by_condition() {
    let base = parse(
        r#"<form-single name="A"><field name="f"><conditional-field condition="a"><display/></conditional-field>
               <default-field><display/></default-field></field></form-single>"#,
    );
    let over = parse(
        r#"<field name="f"><conditional-field condition="a" title="T"><text-line/></conditional-field>
               <conditional-field condition="b"><hidden/></conditional-field></field>"#,
    );
    merge_field_node(&base, &over, false);
    let field = base.first_named("field").unwrap();
    let conditions: Vec<String> = field
        .children_named("conditional-field")
        .iter()
        .filter_map(|node| node.attribute("condition"))
        .collect();
    assert_eq!(conditions, ["a", "b"]);
    let first = &field.children_named("conditional-field")[0];
    assert_eq!(first.attribute("title").as_deref(), Some("T"));
    assert!(first.first_named("text-line").is_some() && first.first_named("display").is_none());
}

/// As Java's `addEntityFields`: a view's auto fields are its aliases, and in a form-list an alias's
/// `default-display` decides whether it gets a field to show, over the default that leaves out the long text types.
#[test]
fn a_view_alias_default_display_decides_a_form_list_auto_field() {
    let conf = parse(
        r#"<database-list>
                <dictionary-type type="id" java-type="String" default-sql-type="VARCHAR(40)"/>
                <dictionary-type type="text-very-long" java-type="String" default-sql-type="TEXT"/>
            </database-list>"#,
    );
    let entities = parse(
        r#"<entities>
                <entity entity-name="Item" package="t" no-update-stamp="true">
                    <field name="itemId" type="id" is-pk="true"/><field name="note" type="text-very-long"/>
                    <field name="code" type="id"/><field name="body" type="text-very-long"/></entity>
                <view-entity entity-name="ItemView" package="t">
                    <member-entity entity-alias="I" entity-name="t.Item"/>
                    <alias name="itemId" entity-alias="I"/>
                    <alias name="note" entity-alias="I" default-display="true"/>
                    <alias name="code" entity-alias="I" default-display="false"/>
                    <alias name="body" entity-alias="I"/>
                </view-entity></entities>"#,
    );
    let model = EntityModel::load(&conf, &[entities]).unwrap();
    let shape = read_entity_shape(&model, "t.ItemView").unwrap();
    assert_eq!(shape.pk_fields, ["itemId"]);
    let form = parse(r#"<form-list name="L" list="items"/>"#);
    let shown = add_entity_fields(&shape, "all", "display", &[], &form, "L");
    assert_eq!(shown, ["itemId", "note", "code"]);
    let field = |name: &str| {
        form.children_named("field")
            .into_iter()
            .find(|field| field.attribute_is("name", name))
            .unwrap()
    };
    assert!(field("note").first_named("default-field").is_some());
    assert!(field("code").first_named("default-field").is_none());
}

/// A find-display number totals with the view alias function when that is min, max, or avg.
/// An alias with no function, and a field of an entity that is not a view, stay sum.
#[test]
fn a_view_alias_function_sets_the_find_display_total() {
    let conf = parse(
        r#"<database-list>
                <dictionary-type type="id" java-type="String" default-sql-type="VARCHAR(40)"/>
                <dictionary-type type="number-decimal" java-type="BigDecimal" default-sql-type="NUMERIC(26,6)"/>
            </database-list>"#,
    );
    let entities = parse(
        r#"<entities>
                <entity entity-name="Line" package="t" no-update-stamp="true">
                    <field name="lineId" type="id" is-pk="true"/>
                    <field name="qty" type="number-decimal"/>
                    <field name="low" type="number-decimal"/>
                    <field name="counted" type="number-decimal"/>
                </entity>
                <view-entity entity-name="LineView" package="t">
                    <member-entity entity-alias="L" entity-name="t.Line"/>
                    <alias name="lineId" entity-alias="L"/>
                    <alias name="qty" entity-alias="L"/>
                    <alias name="low" entity-alias="L" function="min"/>
                    <alias name="counted" entity-alias="L" function="count"/>
                </view-entity></entities>"#,
    );
    let model = EntityModel::load(&conf, &[entities]).unwrap();
    let total = |form: &MNode, name: &str| {
        form.children_named("field")
            .into_iter()
            .find(|field| field.attribute_is("name", name))
            .and_then(|field| field.attribute("show-total"))
    };
    let view = read_entity_shape(&model, "t.LineView").unwrap();
    let form = parse(r#"<form-list name="L"/>"#);
    add_entity_fields(&view, "all", "find-display", &[], &form, "L");
    assert_eq!(total(&form, "low").as_deref(), Some("min"));
    assert_eq!(total(&form, "qty").as_deref(), Some("sum"));
    assert_eq!(total(&form, "counted").as_deref(), Some("sum"));
    let entity = read_entity_shape(&model, "t.Line").unwrap();
    assert!(entity.alias_function.is_empty());
    let plain = parse(r#"<form-list name="L"/>"#);
    add_entity_fields(&entity, "nonpk", "find-display", &[], &plain, "L");
    assert_eq!(total(&plain, "qty").as_deref(), Some("sum"));
    assert_eq!(total(&plain, "low").as_deref(), Some("sum"));
}

/// As Java's `addEntityFields`: a view's `pq-expression` alias with `default-display="true"` is a display field
/// whatever the form, `include`, field type and excludes, and is not one of the names shown; one without it gets no
/// field.
#[test]
fn a_pq_expression_alias_with_default_display_is_an_auto_display_field() {
    let conf = parse(
        r#"<database-list>
                <dictionary-type type="id" java-type="String" default-sql-type="VARCHAR(40)"/>
            </database-list>"#,
    );
    let entities = parse(
        r#"<entities>
                <entity entity-name="Item" package="t" no-update-stamp="true">
                    <field name="itemId" type="id" is-pk="true"/><field name="code" type="id"/></entity>
                <view-entity entity-name="ItemView" package="t">
                    <member-entity entity-alias="I" entity-name="t.Item"/>
                    <alias-all entity-alias="I"/>
                    <alias name="label" pq-expression="itemId + ':' + code" default-display="true"/>
                    <alias name="quiet" pq-expression="code"/>
                </view-entity></entities>"#,
    );
    let model = EntityModel::load(&conf, &[entities]).unwrap();
    let shape = read_entity_shape(&model, "t.ItemView").unwrap();
    assert_eq!(shape.pq_display, ["label"]);
    let form = parse(r#"<form-single name="S"/>"#);
    let shown = add_entity_fields(&shape, "pk", "edit", &["label".to_string()], &form, "S");
    assert_eq!(shown, ["itemId"]);
    let names: Vec<String> = form
        .children_named("field")
        .iter()
        .filter_map(|field| field.attribute("name"))
        .collect();
    assert_eq!(names, ["itemId", "label"]);
    let label = form.children_named("field")[1].first_named("default-field").unwrap();
    assert_eq!(label.attribute("validate-entity").as_deref(), Some("t.ItemView"));
    assert_eq!(label.attribute("validate-field").as_deref(), Some("label"));
    assert!(label.first_named("display").is_some());
    assert!(label.first_named("text-area").is_none() && label.first_named("text-line").is_none());
}

/// Java's `addEntityFields` of a `DataDocument.<id>` name: the document's view, its fields' `defaultDisplay` and
/// an expression field with `defaultDisplay` `Y` a display field; its shape is read again after its rows change.
#[test]
fn a_data_document_name_gives_the_auto_fields_of_its_view() {
    use moqui_lib::object::Value;
    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let create = |entity: &str, pairs: &[(&str, &str)]| {
        let mut row = facade.make_value(entity).unwrap();
        for (name, value) in pairs {
            row.set(*name, Value::string(*value));
        }
        row.create(&facade).unwrap();
    };
    create(
        "moqui.entity.document.DataDocument",
        &[
            ("dataDocumentId", "FormDoc"),
            ("primaryEntityName", "moqui.basic.Enumeration"),
        ],
    );
    let field = |seq: &str, path: &str, extra: &[(&str, &str)]| {
        let mut pairs = vec![("dataDocumentId", "FormDoc"), ("fieldSeqId", seq), ("fieldPath", path)];
        pairs.extend_from_slice(extra);
        create("moqui.entity.document.DataDocumentField", &pairs);
    };
    field("01", "enumId", &[]);
    field("02", "description", &[("defaultDisplay", "N")]);
    field(
        "03",
        "(enumId + '!')",
        &[("fieldNameAlias", "shout"), ("defaultDisplay", "Y")],
    );
    let shape = super::entity_shape("DataDocument.FormDoc").unwrap();
    assert_eq!(shape.full_name, "DataDocument.FormDoc");
    assert_eq!(shape.pq_display, ["shout"]);
    let form = parse(r#"<form-list name="L"/>"#);
    let shown = add_entity_fields(&shape, "all", "display", &[], &form, "L");
    assert_eq!(shown, ["enumId", "description"]);
    let fields = form.children_named("field");
    let names: Vec<String> = fields.iter().filter_map(|field| field.attribute("name")).collect();
    assert_eq!(names, ["enumId", "description", "shout"]);
    assert!(fields[1].first_named("default-field").is_none(), "defaultDisplay N");
    assert!(fields[2]
        .first_named("default-field")
        .unwrap()
        .first_named("display")
        .is_some());

    field("04", "enumTypeId", &[]);
    let shape = super::entity_shape("DataDocument.FormDoc").unwrap();
    assert!(shape.fields.iter().any(|(name, _, _)| name == "enumTypeId"));
}

/// A `display-entity` of a status or an enumeration shows its `description`: the first non-key field ending in
/// `Name` would come first (StatusItem once gained a `colorName`, and every status in a form showed empty).
#[test]
fn a_status_and_an_enumeration_are_described_by_their_description() {
    let framework = crate::testing::framework_dir();
    let conf_text = std::fs::read_to_string(framework.join("conf/MoquiDefaultConf.xml")).unwrap();
    let conf = parse(&conf_text);
    let model =
        EntityModel::load_directory(&conf.first_named("database-list").unwrap(), &framework.join("entity")).unwrap();
    for entity_name in ["moqui.basic.StatusItem", "moqui.basic.Enumeration"] {
        let shape = read_entity_shape(&model, entity_name).unwrap();
        assert_eq!(shape.description_field, "description", "{entity_name}");
    }
}
