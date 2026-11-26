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

/// A form as `db_form_xml` builds it from a DbForm's rows: a field with a title, a tooltip and required; one with an
/// entry name, a widget attribute, options, entity options and a dependency; one with a condition.
const FORM: &str = r#"<form-single>
    <field name="dbNote" layoutSequenceNum="1">
        <default-field title="Db Note" tooltip="A note" required="true"><text-line size="30"/></default-field>
    </field>
    <field name="kind" from="kindId" layoutSequenceNum="2">
        <default-field title="Kind"><drop-down allow-empty="true"><option key="A" text="Alpha &amp; more"/><entity-options text="${description}"><entity-find entity-name="moqui.basic.Enumeration"><econdition field-name="enumTypeId" value="Kind"/><order-by field-name="sequenceNum"/></entity-find></entity-options></drop-down><visible-when field="dbNote" value="y"/></default-field>
    </field>
    <field name="secret">
        <conditional-field condition="ec.user.userId" title="Secret"><display/></conditional-field>
    </field>
</form-single>"#;

#[test]
fn the_fields_are_written_as_the_form_merge_has_them() {
    let fields = merge_fields(FORM).unwrap();
    assert_eq!(fields.len(), 3);
    assert!(fields.iter().all(|field| !field.contains("layoutSequenceNum")));
    assert!(fields[0].starts_with("<field name=\"dbNote\">"), "{}", fields[0]);
    assert!(fields[0].contains(r#"title="Db Note" tooltip="A note" required="true""#));
    assert!(fields[1].contains(r#"<field name="kind" from="kindId">"#));
    assert!(fields[1].contains(r#"text="Alpha &amp; more""#), "{}", fields[1]);
    assert!(fields[1].contains(r#"<econdition field-name="enumTypeId" value="Kind"/>"#));
    assert!(fields[1].contains(r#"<visible-when field="dbNote" value="y"/>"#));
    assert!(fields[2].contains(r#"<conditional-field condition="ec.user.userId" title="Secret">"#));
}

#[test]
fn a_new_file_gets_the_form_merge() {
    let fields = merge_fields(FORM).unwrap();
    let outline = moqui_lib::xml_edit::outline(EMPTY_EXTEND).unwrap();
    let text = moqui_lib::xml_edit::apply(EMPTY_EXTEND, &extension_ops(&outline, "form-single", "Facts", &fields))
        .unwrap()
        .text;
    let node = MNode::parse_text("extend", &text).unwrap();
    let merges = node.children_named("form-single");
    assert_eq!(merges.len(), 1, "{text}");
    assert_eq!(merges[0].attribute("name").as_deref(), Some("Facts"));
    let names: Vec<String> = merges[0]
        .children_named("field")
        .iter()
        .filter_map(|field| field.attribute("name"))
        .collect();
    assert_eq!(names, ["dbNote", "kind", "secret"]);
}

#[test]
fn a_form_merge_there_takes_the_fields_last() {
    let fields = merge_fields(FORM).unwrap();
    let current = "<screen-extend>\n    <form-list name=\"Facts\"/>\n    <form-single name=\"Facts\">\n        <field name=\"extraNote\" after=\"upId\"><default-field><display/></default-field></field>\n    </form-single>\n</screen-extend>\n";
    let outline = moqui_lib::xml_edit::outline(current).unwrap();
    let ops = extension_ops(&outline, "form-single", "Facts", &fields);
    assert_eq!(ops.len(), 3);
    let text = moqui_lib::xml_edit::apply(current, &ops).unwrap().text;
    let node = MNode::parse_text("extend", &text).unwrap();
    let merges = node.children_named("form-single");
    assert_eq!(merges.len(), 1, "{text}");
    let names: Vec<String> = merges[0]
        .children_named("field")
        .iter()
        .filter_map(|field| field.attribute("name"))
        .collect();
    assert_eq!(names, ["extraNote", "dbNote", "kind", "secret"]);
    // the form-list of the same name is not the form-single's merge
    assert!(node.children_named("form-list")[0].children_named("field").is_empty());
}
