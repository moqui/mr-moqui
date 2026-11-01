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

//! The artifact kinds below an entity, a screen and a data file: relationships, indexes, view members, form fields,
//! subscreens items, ext items, widget templates, data records, service includes and ECA keys.

use super::*;
use extract::{extract, extract_with, record_id, FileInfo, RecordModel};

/// An entity model of a few entities: their full names, short names, primary keys and relationships.
struct Model;

impl RecordModel for Model {
    fn keys(&self, name: &str) -> Option<(String, Vec<String>)> {
        let (full, pk): (&str, &[&str]) = match name {
            "moqui.basic.Enumeration" | "Enumeration" => ("moqui.basic.Enumeration", &["enumId"]),
            "moqui.basic.StatusFlowTransition" => (
                "moqui.basic.StatusFlowTransition",
                &["statusFlowId", "statusId", "toStatusId", "transitionName"],
            ),
            "a.Party" | "Party" => ("a.Party", &["partyId"]),
            "a.PartyRole" => ("a.PartyRole", &["partyId", "roleTypeId"]),
            "a.Sequenced" => ("a.Sequenced", &["seqId"]),
            _ => return None,
        };
        Some((full.to_string(), pk.iter().map(|field| field.to_string()).collect()))
    }

    fn related(&self, entity: &str, name: &str) -> Option<(String, Vec<(String, String)>)> {
        (entity == "a.Party" && matches!(name, "roles" | "a.PartyRole" | "Owner#a.PartyRole")).then(|| {
            (
                "a.PartyRole".to_string(),
                vec![("partyId".to_string(), "partyId".to_string())],
            )
        })
    }
}

fn info<'a>(kind: ArtifactKind, location: &'a str) -> FileInfo<'a> {
    FileInfo {
        path: if kind == ArtifactKind::ScreenText {
            "/tmp/t/template/W.xml"
        } else {
            "/tmp/t/file.xml"
        },
        location,
        component: "c",
        kind,
    }
}

fn read(kind: ArtifactKind, location: &str, text: &str) -> extract::FileIndex {
    extract_with(&info(kind, location), text, &Model).unwrap()
}

fn row<'a>(index: &'a extract::FileIndex, id: &str) -> &'a ArtifactRow {
    index
        .artifacts
        .iter()
        .find(|artifact| artifact.id == id)
        .unwrap_or_else(|| {
            panic!(
                "no {id} in {:?}",
                index.artifacts.iter().map(|a| &a.id).collect::<Vec<_>>()
            )
        })
}

fn memory(files: &[&extract::FileIndex]) -> MemoryIndex {
    let mut index = MemoryIndex::default();
    for file in files {
        for artifact in &file.artifacts {
            index.resolver.add(artifact);
        }
        index.artifacts.extend(file.artifacts.iter().cloned());
        index.refs.extend(
            file.refs
                .iter()
                .cloned()
                .map(|row| ("/tmp/t/file.xml".to_string(), row)),
        );
    }
    let resolver = index.resolver.clone();
    for (_, row) in &mut index.refs {
        resolver.resolve(row, &|_| false);
    }
    index.position();
    index
}

const ENTITIES: &str = r#"<entities>
    <entity entity-name="Party" package="a">
        <field name="partyId" type="id" is-pk="true"/>
        <field name="ownerPartyId" type="id"/>
        <relationship type="one" title="Owner" related="Party" short-alias="owner">
            <key-map field-name="ownerPartyId" related="partyId"/></relationship>
        <relationship type="many" related="a.PartyRole" short-alias="roles"/>
        <index name="PARTY_OWNER" unique="true"><index-field name="ownerPartyId"/><index-field name="gone"/></index>
        <seed-data><moqui.basic.Enumeration enumId="PtyDefault" description="Default"/></seed-data>
    </entity>
    <view-entity entity-name="PartyAndRole" package="a">
        <member-entity entity-alias="PTY" entity-name="a.Party"/>
        <member-relationship entity-alias="ROL" join-from-alias="PTY" relationship="roles"/>
        <alias entity-alias="ROL" name="roleTypeId"/>
    </view-entity>
    <extend-entity entity-name="PartyRole" package="a">
        <relationship type="one" related="moqui.basic.Enumeration"/>
    </extend-entity>
</entities>"#;

#[test]
fn an_entity_has_its_relationships_indexes_view_members_and_seed_records() {
    let location = "component://c/entity/A.xml";
    let file = read(ArtifactKind::Entity, location, ENTITIES);

    // titled, to its own entity named short (the model gives the full name), and untitled
    let owner = row(&file, "relationship:a.Party@Owner#a.Party");
    assert_eq!(owner.kind, "relationship");
    assert_eq!(owner.short_name, "Owner#Party");
    assert_eq!(owner.parent.as_deref(), Some("entity:a.Party"));
    assert_eq!(owner.summary, "one Party, short-alias owner");
    assert_eq!(owner.line, 5);
    assert_eq!(row(&file, "relationship:a.Party@a.PartyRole").short_name, "PartyRole");
    // an extend-entity's relationship belongs to the entity it extends
    assert_eq!(
        row(&file, "relationship:a.PartyRole@moqui.basic.Enumeration")
            .parent
            .as_deref(),
        Some("entity:a.PartyRole")
    );

    let index = row(&file, "index:a.Party@PARTY_OWNER");
    assert_eq!(
        (index.summary.as_str(), index.short_name.as_str()),
        ("unique: ownerPartyId, gone", "PARTY_OWNER")
    );
    assert_eq!(index.parent.as_deref(), Some("entity:a.Party"));

    let member = row(&file, "view-member:a.PartyAndRole@PTY");
    assert_eq!(
        (member.summary.as_str(), member.parent.as_deref()),
        ("a.Party", Some("entity:a.PartyAndRole"))
    );
    assert_eq!(
        row(&file, "view-member:a.PartyAndRole@ROL").summary,
        "relationship roles from PTY"
    );
    // a view's aliases stay its fields
    row(&file, "field:a.PartyAndRole.roleTypeId");

    // a seed record is its entity's child
    let seed = row(&file, "record:moqui.basic.Enumeration:PtyDefault");
    assert_eq!(
        (seed.parent.as_deref(), seed.summary.as_str()),
        (Some("entity:a.Party"), "Enumeration: Default")
    );

    // the references are made from the relationship, index and member that make them
    let index_memory = memory(&[&file]);
    let out = index_memory.refs("index:a.Party@PARTY_OWNER", Direction::Out);
    let fields: Vec<(&str, Option<&str>)> = out
        .iter()
        .map(|entry| (entry.to_name.as_str(), entry.to_id.as_deref()))
        .collect();
    assert_eq!(
        fields,
        [("ownerPartyId", Some("field:a.Party.ownerPartyId")), ("gone", None)]
    );
    let related = index_memory.refs("relationship:a.Party@a.PartyRole", Direction::Out);
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].ref_type, "relationship");
    let members = index_memory.refs("view-member:a.PartyAndRole@PTY", Direction::Out);
    assert_eq!(members[0].to_id.as_deref(), Some("entity:a.Party"));
    // the entity's own refs still take in its children's
    assert!(index_memory
        .refs("entity:a.Party", Direction::Out)
        .iter()
        .any(|entry| entry.from == "index:a.Party@PARTY_OWNER"));
    let children: Vec<String> = index_memory
        .children("entity:a.Party")
        .into_iter()
        .map(|row| row.kind)
        .collect();
    assert_eq!(
        children,
        ["field", "field", "relationship", "relationship", "index", "record"]
    );
}

#[test]
fn without_a_model_a_relationship_keeps_its_name_as_written_and_records_are_not_read() {
    let location = "component://c/entity/A.xml";
    let file = extract(&info(ArtifactKind::Entity, location), ENTITIES).unwrap();
    row(&file, "relationship:a.Party@Owner#Party");
    assert!(!file.artifacts.iter().any(|artifact| artifact.kind == "record"));
    let data = extract(
        &info(ArtifactKind::Data, "component://c/data/D.xml"),
        r#"<entity-facade-xml type="seed"><moqui.basic.Enumeration enumId="X"/></entity-facade-xml>"#,
    )
    .unwrap();
    assert_eq!(data.artifacts.len(), 1);
}

#[test]
fn data_records_by_primary_key_with_nested_records_and_escapes() {
    let location = "component://c/data/D.xml";
    let file = read(
        ArtifactKind::Data,
        location,
        r#"<entity-facade-xml type="seed">
    <moqui.basic.StatusFlowTransition statusFlowId="Default" statusId="A" toStatusId="B" transitionName="Go"/>
    <a.Party partyId="P1"><roles roleTypeId="Customer"/><a.PartyRole roleTypeId="Vendor"/></a.Party>
    <Enumeration description="Field element"><enumId>Enum,1%</enumId></Enumeration>
    <a.Sequenced description="sequenced by the loader"/>
    <unknown.Thing thingId="T"/>
</entity-facade-xml>"#,
    );
    let ids: Vec<&str> = file.artifacts.iter().map(|artifact| artifact.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "data:component://c/data/D.xml",
            "record:moqui.basic.StatusFlowTransition:Default,A,B,Go",
            "record:a.Party:P1",
            "record:a.PartyRole:P1,Customer",
            "record:a.PartyRole:P1,Vendor",
            "record:moqui.basic.Enumeration:Enum%2C1%25",
        ]
    );
    let flow = row(&file, "record:moqui.basic.StatusFlowTransition:Default,A,B,Go");
    assert_eq!(flow.parent.as_deref(), Some("data:component://c/data/D.xml"));
    assert_eq!(
        (flow.short_name.as_str(), flow.summary.as_str()),
        ("Default,A,B,Go", "StatusFlowTransition")
    );
    assert_eq!(flow.line, 2);
    assert_eq!(row(&file, "record:a.PartyRole:P1,Customer").line, 3);
    assert_eq!(
        record_id("e.E", &["a,b".into(), "100%".into()]),
        "record:e.E:a%2Cb,100%25"
    );

    // reachable by id, and a find of a key value finds it after the definitions people look for first
    let index = memory(&[&file]);
    assert_eq!(
        index.describe("record:a.Party:P1").map(|row| row.kind),
        Some("record".to_string())
    );
    assert_eq!(index.find("P1", &[], 5)[0].id, "record:a.Party:P1");
    // a data file's refs are not asked for each of its records
    let data_id = "data:component://c/data/D.xml";
    assert!(index
        .artifacts
        .iter()
        .any(|row| row.parent.as_deref() == Some(data_id) && row.kind == "record"));
    assert!(index.refs(data_id, Direction::In).is_empty());
}

/// A nested record's element is read as the loader reads it: `Title-entity` is the relationship `Title#entity`, and an
/// empty key value is filled from the parent.
#[test]
fn nested_records_by_titled_relationship_and_an_empty_key_from_the_parent() {
    let file = read(
        ArtifactKind::Data,
        "component://c/data/N.xml",
        r#"<entity-facade-xml type="seed">
    <a.Party partyId="P2"><Owner-a.PartyRole roleTypeId="Owner"/><roles partyId="" roleTypeId="Agent"/></a.Party>
</entity-facade-xml>"#,
    );
    let ids: Vec<&str> = file.artifacts.iter().map(|artifact| artifact.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "data:component://c/data/N.xml",
            "record:a.Party:P2",
            "record:a.PartyRole:P2,Owner",
            "record:a.PartyRole:P2,Agent",
        ]
    );
}

#[test]
fn a_screen_has_form_fields_subscreens_items_and_ext_items() {
    let location = "component://c/screen/A.xml";
    let file = read(
        ArtifactKind::Screen,
        location,
        r#"<screen>
    <transition name="createTask"><default-response url="."/></transition>
    <subscreens><subscreens-item name="Detail" location="component://c/screen/A/Detail.xml"/></subscreens>
    <ext-action name="boss.createTask" slot="message-actions" label="Create task" transition="createTask"/>
    <ext-panel name="boss.tasks" slot="room-panel" screen="Tasks"/>
    <widgets>
        <form-single name="EditA" transition="createTask">
            <field name="aId"><default-field title="A"><text-line/></default-field></field>
            <field name="submit"><default-field><submit/></default-field></field>
        </form-single>
        <form-list name="ListA" list="rows">
            <field name="aId"><header-field show-order-by="true"/><default-field><link url="createTask" text="Go"/></default-field></field>
        </form-list>
    </widgets>
</screen>"#,
    );
    let field = row(&file, "form-field:component://c/screen/A.xml#EditA@aId");
    assert_eq!(field.kind, "form-field");
    assert_eq!((field.name.as_str(), field.short_name.as_str()), ("EditA@aId", "aId"));
    assert_eq!(field.parent.as_deref(), Some("form:component://c/screen/A.xml#EditA"));
    assert_eq!(field.summary, "text-line, title A");
    assert_eq!(
        row(&file, "form-field:component://c/screen/A.xml#EditA@submit").summary,
        "submit"
    );
    assert_eq!(
        row(&file, "form-field:component://c/screen/A.xml#ListA@aId").summary,
        "link"
    );

    let item = row(&file, "subscreen:component://c/screen/A.xml#Detail");
    assert_eq!(
        (item.summary.as_str(), item.parent.as_deref()),
        (
            "component://c/screen/A/Detail.xml",
            Some("screen:component://c/screen/A.xml")
        )
    );
    let action = row(&file, "ext:component://c/screen/A.xml#ext-action:boss.createTask");
    assert_eq!(action.summary, "ext-action, slot message-actions, Create task");
    row(&file, "ext:component://c/screen/A.xml#ext-panel:boss.tasks");

    let index = memory(&[&file]);
    // an ext-action posts to a transition of its screen; a subscreens item names its screen
    let posts = index.refs(
        "ext:component://c/screen/A.xml#ext-action:boss.createTask",
        Direction::Out,
    );
    assert_eq!(
        posts[0].to_id.as_deref(),
        Some("transition:component://c/screen/A.xml#createTask:any")
    );
    let item_refs = index.refs("subscreen:component://c/screen/A.xml#Detail", Direction::Out);
    assert_eq!(
        (item_refs[0].to_kind.as_str(), item_refs[0].ref_type.as_str()),
        ("screen", "subscreens-item")
    );
    // a field's references stay its form's
    let form_refs = index.refs("form:component://c/screen/A.xml#ListA", Direction::Out);
    assert!(form_refs
        .iter()
        .any(|entry| entry.ref_type == "link" && entry.from == "form:component://c/screen/A.xml#ListA"));
}

#[test]
fn a_screen_extend_has_its_own_transitions_forms_and_form_fields() {
    let location = "component://site/screen-extend/c/A.xml";
    let file = read(
        ArtifactKind::ScreenExtend,
        location,
        r#"<screen-extend>
    <transition name="added" method="post"><parameter name="aId" required="true"/><service-call name="a.B.run"/><default-response url="."/></transition>
    <form-single name="EditA" transition="added"><field name="note"><default-field><text-area/></default-field></field></form-single>
</screen-extend>"#,
    );
    let owner = Some("screen-extend:component://site/screen-extend/c/A.xml");
    let transition = row(&file, "transition:component://site/screen-extend/c/A.xml#added:post");
    assert_eq!(
        (transition.parent.as_deref(), transition.summary.as_str()),
        (owner, "calls a.B.run")
    );
    row(
        &file,
        "parameter:transition:component://site/screen-extend/c/A.xml#added:post@in.aId",
    );
    assert_eq!(
        row(&file, "form:component://site/screen-extend/c/A.xml#EditA")
            .parent
            .as_deref(),
        owner
    );
    row(&file, "form-field:component://site/screen-extend/c/A.xml#EditA@note");
    // the transition's service call is made from it, and the anchors stay the screen-extend's
    assert!(file
        .refs
        .iter()
        .any(|row| row.to_name == "a.B.run"
            && row.from == "transition:component://site/screen-extend/c/A.xml#added:post"));
    assert!(file
        .refs
        .iter()
        .filter(|row| row.ref_type == "anchor")
        .all(|row| row.from == "screen-extend:component://site/screen-extend/c/A.xml"));
}

#[test]
fn widget_templates_are_read_and_an_include_resolves_to_one() {
    let location = "component://c/template/W.xml";
    let templates = read(
        ArtifactKind::ScreenText,
        location,
        r#"<widget-templates><widget-template name="partyField"><drop-down/></widget-template></widget-templates>"#,
    );
    let template = row(&templates, "widget-template:component://c/template/W.xml#partyField");
    assert_eq!(
        (template.name.as_str(), template.summary.as_str()),
        ("partyField", "drop-down")
    );
    // a template directory's other text is not read
    let other = read(ArtifactKind::ScreenText, location, "<html>${x}</html>");
    assert!(other.artifacts.is_empty());

    let screen = read(
        ArtifactKind::Screen,
        "component://c/screen/A.xml",
        r#"<screen><widgets><form-single name="F"><field name="p"><default-field>
            <widget-template-include location="component://c/template/W.xml#partyField"/></default-field></field></form-single></widgets></screen>"#,
    );
    let index = memory(&[&templates, &screen]);
    let users = index.refs("widget-template:component://c/template/W.xml#partyField", Direction::In);
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].from, "form:component://c/screen/A.xml#F");
    assert_eq!(
        resolve::Resolver::names_of(&templates.artifacts),
        ["component://c/template/W.xml#partyField"]
    );

    // only a template directory's XML files are read
    let root = Path::new("/tmp/t");
    assert!(locate_in("c", root, Path::new("/tmp/t/template/W.xml")).is_some());
    assert!(locate_in("c", root, Path::new("/tmp/t/template/W.html.ftl")).is_none());
    assert!(locate_in("c", root, Path::new("/tmp/t/screen/A.qvue")).is_none());
}

#[test]
fn eca_keys_are_the_id_rule_name_or_target_and_event() {
    let location = "component://c/service/A.secas.xml";
    let file = read(
        ArtifactKind::Seca,
        location,
        r#"<secas>
    <seca service="a.B.run" when="post-service"/>
    <seca service="a.B.run" when="post-service"/>
    <seca id="Named" service="a.B.run" when="post-service"/>
    <seca service="a.B.other" when="tx-commit"/>
</secas>"#,
    );
    let keys: Vec<&str> = file
        .artifacts
        .iter()
        .map(|artifact| artifact.id.rsplit_once(".xml#").map_or("", |(_, key)| key))
        .collect();
    assert_eq!(
        keys,
        [
            "a.B.run:post-service",
            "a.B.run:post-service:2",
            "Named",
            "a.B.other:tx-commit"
        ]
    );

    let eecas = read(
        ArtifactKind::Eeca,
        "component://c/entity/A.eecas.xml",
        r#"<eecas><eeca entity="a.Party" on-create="true" on-update="true"/><eeca entity="a.Party" on-delete="true"/></eecas>"#,
    );
    let keys: Vec<&str> = eecas
        .artifacts
        .iter()
        .map(|artifact| artifact.id.rsplit_once('#').unwrap().1)
        .collect();
    assert_eq!(keys, ["a.Party:create+update", "a.Party:delete"]);

    let emecas = read(
        ArtifactKind::Seca,
        "component://c/service/A.emecas.xml",
        r#"<emecas><emeca rule-name="Receive"/></emecas>"#,
    );
    assert_eq!(
        emecas.artifacts[0].id,
        "emeca:component://c/service/A.emecas.xml#Receive"
    );
}

#[test]
fn a_service_include_defines_the_service_by_its_file_and_refers_to_the_included_one() {
    let file = read(
        ArtifactKind::Service,
        "component://c/service/a/Shipments.xml",
        r#"<services><service-include verb="get" noun="Info" location="component://c/service/a/InfoServices.xml"/></services>"#,
    );
    let include = row(&file, "service:a.Shipments.get#Info");
    assert_eq!(
        (include.kind.as_str(), include.summary.as_str()),
        ("service", "includes a.InfoServices.get#Info")
    );
    let included = read(
        ArtifactKind::Service,
        "component://c/service/a/InfoServices.xml",
        r#"<services><service verb="get" noun="Info"/></services>"#,
    );
    let index = memory(&[&file, &included]);
    let out = index.refs("service:a.Shipments.get#Info", Direction::Out);
    assert!(out.iter().any(|entry| entry.ref_type == "service-include"
        && entry.to_id.as_deref() == Some("service:a.InfoServices.get#Info")));
}
