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

//! The parameters and fields the index expands (resolve/expand.rs): a service's from its `auto-parameters` and
//! `implements`, a form's from its `auto-fields-entity`, `auto-fields-service` and `extends`, a form's bindings by a
//! path to each service the transition reached calls, and the check of a required in-parameter nothing gives.

use super::*;
use extract::{extract, FileInfo};
use std::collections::HashSet;

const ENTITIES: &str = r#"<entities>
    <entity entity-name="Example" package="c">
        <field name="exampleId" type="id" is-pk="true"/>
        <field name="name" type="text-medium"/>
        <field name="statusId" type="id"/>
        <field name="comments" type="text-very-long"/>
    </entity>
</entities>"#;

const SERVICES: &str = r#"<services>
    <service verb="define" noun="Info" type="interface">
        <in-parameters><parameter name="infoName" required="true"/><parameter name="kind"/></in-parameters>
    </service>
    <service verb="create" noun="Thing">
        <implements service="c.ThingServices.define#Info" required="false"/>
        <in-parameters><auto-parameters entity-name="c.Example" include="nonpk"/><parameter name="extra"/></in-parameters>
    </service>
    <service verb="make" noun="Thing">
        <actions><service-call name="c.ThingServices.create#Thing"><field-map field-name="name" from="label"/>
            <field-map field-name="infoName" value="x"/></service-call></actions>
    </service>
</services>"#;

const SCREEN: &str = r#"<screen>
    <transition name="save"><service-call name="c.ThingServices.create#Thing"/><default-response url="."/></transition>
    <widgets>
        <form-single name="ByEntity" transition="save">
            <auto-fields-entity entity-name="c.Example" field-type="edit"><exclude field-name="statusId"/></auto-fields-entity>
            <field name="name"><default-field><text-line/></default-field></field>
        </form-single>
        <form-single name="ByService" transition="save"><auto-fields-service service-name="c.ThingServices.create#Thing"/></form-single>
        <form-single name="Extended" extends="ByEntity" transition="save"/>
        <form-list name="Listed" list="examples"><auto-fields-entity entity-name="c.Example"/></form-list>
    </widgets>
</screen>"#;

const SCREEN_LOCATION: &str = "component://c/screen/Things.xml";

/// The files read into a memory index, resolved.
fn index_of(files: &[(ArtifactKind, &str, &str)]) -> MemoryIndex {
    let mut index = MemoryIndex::default();
    for (kind, location, text) in files {
        read_into(&mut index, *kind, location, text);
    }
    index.resolve_all(&|_| false);
    index
}

fn path_of(location: &str) -> String {
    format!("/tmp/derive/{}", location.trim_start_matches("component://"))
}

/// `text` read as the file at `location`, in place of what the index had of it (not resolved).
fn read_into(index: &mut MemoryIndex, kind: ArtifactKind, location: &str, text: &str) {
    let path = path_of(location);
    index.artifacts.retain(|artifact| artifact.path != path);
    index.refs.retain(|(file, _)| *file != path);
    index.bindings.retain(|(file, _)| *file != path);
    let read = extract(
        &FileInfo {
            path: &path,
            location,
            component: "c",
            kind,
        },
        text,
    )
    .unwrap();
    index.take_rows(path, read);
}

fn base_files() -> Vec<(ArtifactKind, &'static str, &'static str)> {
    vec![
        (ArtifactKind::Entity, "component://c/entity/Entities.xml", ENTITIES),
        (
            ArtifactKind::Service,
            "component://c/service/c/ThingServices.xml",
            SERVICES,
        ),
        (ArtifactKind::Screen, SCREEN_LOCATION, SCREEN),
    ]
}

/// The fields of the form `name` of the screen, with each one's origin (empty: its own), in name order.
fn fields(index: &MemoryIndex, form: &str) -> Vec<(String, String)> {
    let id = format!("form:{SCREEN_LOCATION}#{form}");
    let mut found: Vec<(String, String)> = index
        .children(&id)
        .into_iter()
        .filter(|artifact| artifact.kind == "form-field")
        .map(|artifact| (artifact.id.rsplit_once('@').unwrap().1.to_string(), artifact.origin))
        .collect();
    found.sort();
    found
}

#[test]
fn auto_parameters_and_implements_give_a_service_its_parameters() {
    let index = index_of(&base_files());
    let service = "service:c.ThingServices.create#Thing";
    let mut params: Vec<(String, String, bool)> = index
        .children(service)
        .into_iter()
        .filter(|artifact| artifact.kind == "parameter")
        .map(|artifact| {
            (
                artifact.short_name.clone(),
                artifact.origin.clone(),
                artifact.summary.contains("required=true"),
            )
        })
        .collect();
    params.sort();
    assert_eq!(
        params,
        [
            // the implemented service's, not required as the implements says; the entity's non-key fields; its own
            ("comments".to_string(), "auto-parameters".to_string(), false),
            ("extra".to_string(), String::new(), false),
            ("infoName".to_string(), "implements".to_string(), false),
            ("kind".to_string(), "implements".to_string(), false),
            ("name".to_string(), "auto-parameters".to_string(), false),
            ("statusId".to_string(), "auto-parameters".to_string(), false),
        ]
    );
    // an expanded parameter is in the service's file, and a field-map to one resolves
    let name = index
        .describe("parameter:service:c.ThingServices.create#Thing@in.name")
        .unwrap();
    assert_eq!(name.path, path_of("component://c/service/c/ThingServices.xml"));
    let maps: Vec<(&str, bool)> = index
        .refs
        .iter()
        .map(|(_, row)| row)
        .filter(|row| row.to_kind == ToKind::Parameter)
        .map(|row| (row.to_name.as_str(), row.resolved))
        .collect();
    assert_eq!(maps, [("name", true), ("infoName", true)]);
}

#[test]
fn auto_fields_and_extends_give_a_form_its_fields_and_bindings() {
    let mut index = index_of(&base_files());
    let own = String::new;
    let of = |origin: &str| origin.to_string();
    assert_eq!(
        fields(&index, "ByEntity"),
        [
            ("comments".to_string(), of("auto-fields-entity")),
            ("exampleId".to_string(), of("auto-fields-entity")),
            ("name".to_string(), own()),
        ]
    );
    assert_eq!(
        fields(&index, "ByService"),
        ["comments", "extra", "infoName", "kind", "name", "statusId"]
            .map(|name| (name.to_string(), of("auto-fields-service")))
    );
    assert_eq!(
        fields(&index, "Extended"),
        ["comments", "exampleId", "name"].map(|name| (name.to_string(), of("extends")))
    );
    // a form-list leaves out the very long text
    assert_eq!(
        fields(&index, "Listed"),
        ["exampleId", "name", "statusId"].map(|name| (name.to_string(), of("auto-fields-entity")))
    );
    // the expanded fields flow into the transition's service
    let bound: Vec<String> = index
        .bindings_of(&format!("form:{SCREEN_LOCATION}#ByService"), "")
        .into_iter()
        .map(|entry| entry.binding.to_id)
        .collect();
    assert!(
        bound.contains(&"parameter:service:c.ThingServices.create#Thing@in.infoName".to_string()),
        "{bound:?}"
    );

    // an entity field added: the services and forms that read the entity have it, in their own files
    read_into(
        &mut index,
        ArtifactKind::Entity,
        "component://c/entity/Entities.xml",
        &ENTITIES.replace("</entity>", r#"<field name="color" type="id"/></entity>"#),
    );
    index.resolve_all(&|_| false);
    assert!(index
        .describe("parameter:service:c.ThingServices.create#Thing@in.color")
        .is_some());
    assert!(fields(&index, "ByService").iter().any(|(name, _)| name == "color"));
    assert!(fields(&index, "Extended").iter().any(|(name, _)| name == "color"));
}

#[test]
fn a_path_form_binds_to_each_service_of_the_transition_by_its_field_maps() {
    let tasks = "component://c/screen/App/Tasks.xml";
    let save = r#"<screen>
    <transition name="save" method="get"><default-response url="."/></transition>
    <transition name="save" method="post">
        <service-call name="a.TaskServices.save#Task"><field-map field-name="name" from="taskName"/></service-call>
        <actions><service-call name="a.TaskServices.log#Task" in-map="true"/></actions>
        <default-response url="."/></transition>
</screen>"#;
    let mut index = index_of(&[
        (ArtifactKind::Screen, "component://c/screen/App.xml", "<screen/>"),
        (
            ArtifactKind::Screen,
            "component://c/screen/App/Order.xml",
            r#"<screen><widgets><form-single name="Find" transition="../Tasks/save">
                <field name="taskName"><default-field><text-line/></default-field></field></form-single></widgets></screen>"#,
        ),
        (ArtifactKind::Screen, tasks, save),
    ]);
    let bound = |index: &MemoryIndex| -> Vec<(String, String)> {
        index
            .bindings_of("form:component://c/screen/App/Order.xml#Find", "")
            .into_iter()
            .map(|entry| {
                let slot = entry.binding.to_path.unwrap_or_default();
                (entry.binding.to_id, slot.rsplit_once('#').unwrap().1.to_string())
            })
            .collect()
    };
    // the post transition's: the field-map renames the field into the first, the second takes the request
    assert_eq!(
        bound(&index),
        [
            (
                "parameter:service:a.TaskServices.save#Task@in.name".to_string(),
                "0".to_string()
            ),
            (
                "parameter:service:a.TaskServices.log#Task@in.taskName".to_string(),
                "1".to_string()
            ),
        ]
    );
    // the second call gone: one binding again
    read_into(
        &mut index,
        ArtifactKind::Screen,
        tasks,
        &save.replace(r#"<service-call name="a.TaskServices.log#Task" in-map="true"/>"#, ""),
    );
    index.resolve_all(&|_| false);
    assert_eq!(
        bound(&index),
        [(
            "parameter:service:a.TaskServices.save#Task@in.name".to_string(),
            "0".to_string()
        )]
    );
}

#[test]
fn a_required_in_parameter_no_field_gives_is_a_warning_on_the_form() {
    let location = "component://c/screen/Jobs.xml";
    let index = index_of(&[
        (
            ArtifactKind::Service,
            "component://c/service/c/JobServices.xml",
            r#"<services><service verb="run" noun="Job"><in-parameters>
                <parameter name="jobName" required="true"/><parameter name="level" required="true" default-value="1"/>
            </in-parameters></service></services>"#,
        ),
        (
            ArtifactKind::Screen,
            location,
            r#"<screen>
    <transition name="run"><service-call name="c.JobServices.run#Job"/><default-response url="."/></transition>
    <transition name="mapped"><service-call name="c.JobServices.run#Job"><field-map field-name="jobName" from="label"/></service-call>
        <default-response url="."/></transition>
    <widgets>
        <form-single name="Given" transition="run"><field name="jobName"><default-field><text-line/></default-field></field></form-single>
        <form-single name="Missing" transition="run"><field name="other"><default-field><text-line/></default-field></field></form-single>
        <form-single name="ByMap" transition="mapped"><field name="label"><default-field><text-line/></default-field></field></form-single>
        <form-single name="Hidden" transition="run"><hidden-parameters parameter-map="[jobName:name, x:1]"/></form-single>
        <form-single name="Through" transition="run" pass-through-parameters="true"/>
        <form-list name="Jobs" list="jobs"><row-selection id-field="jobName">
            <action><form-single name="Selected" transition="run"/></action></row-selection></form-list>
    </widgets>
</screen>"#,
        ),
    ]);
    let problems = crate::check::required_problems(&index);
    let forms: Vec<&str> = problems
        .iter()
        .map(|problem| problem.message.split(' ').nth(1).unwrap_or_default())
        .collect();
    assert_eq!(forms, ["Missing"], "{problems:?}");
    assert_eq!(problems[0].severity, crate::Severity::Warning);
    assert_eq!(problems[0].rule, "reference.required");
    assert!(
        problems[0].message.contains("jobName") && !problems[0].message.contains("level"),
        "{}",
        problems[0].message
    );
}

/// The bindings of a form, by the parameter each reaches.
fn bound_of(index: &MemoryIndex, form: &str) -> Vec<String> {
    let mut ids: Vec<String> = index
        .bindings_of(&format!("form:{SCREEN_LOCATION}#{form}"), "")
        .into_iter()
        .map(|entry| entry.binding.to_id)
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

#[test]
fn a_declared_field_with_no_widget_keeps_the_binding_of_the_field_it_merges_into() {
    let screen = SCREEN.replace(
        "</widgets>",
        r#"<form-single name="Header" extends="ByEntity" transition="save">
            <field name="exampleId"><header-field><text-find/></header-field></field></form-single>
        <form-single name="Titled" extends="ByEntity" transition="save">
            <field name="exampleId"><default-field title="Example"/></field></form-single>
        <form-single name="Button" extends="ByEntity" transition="save">
            <field name="exampleId"><default-field><submit/></default-field></field></form-single>
    </widgets>"#,
    );
    let mut files = base_files();
    files[2].2 = &screen;
    let index = index_of(&files);
    let example = "parameter:service:c.ThingServices.create#Thing@in.exampleId".to_string();
    // a header-field only: the inherited widget stays, and its binding; a submit of its own: none
    assert!(
        bound_of(&index, "Header").contains(&example),
        "{:?}",
        bound_of(&index, "Header")
    );
    assert!(
        !bound_of(&index, "Button").contains(&example),
        "{:?}",
        bound_of(&index, "Button")
    );
    // an empty default-field only sets the title: the inherited widget stays too
    assert!(
        bound_of(&index, "Titled").contains(&example),
        "{:?}",
        bound_of(&index, "Titled")
    );
    // declared, so no expanded field of its name
    assert!(fields(&index, "Header").contains(&("exampleId".to_string(), String::new())));
}

#[test]
fn parameters_merge_by_what_each_sets_as_moqui_merges_them() {
    let services = r#"<services>
    <service verb="define" noun="Shape" type="interface">
        <in-parameters><auto-parameters include="pk"/><parameter name="mode" required="true"/>
            <parameter name="hidden" required="disabled"/><parameter name="size" type="Long"/></in-parameters>
    </service>
    <service verb="update" noun="Example">
        <implements service="c.ShapeServices.define#Shape"/>
        <in-parameters><parameter name="mode"/><auto-parameters entity-name="c.Example" include="nonpk" required="true"/>
            <parameter name="size" required="true"/></in-parameters>
    </service>
</services>"#;
    let index = index_of(&[
        (ArtifactKind::Entity, "component://c/entity/Entities.xml", ENTITIES),
        (
            ArtifactKind::Service,
            "component://c/service/c/ShapeServices.xml",
            services,
        ),
    ]);
    let summary = |name: &str| {
        index
            .describe(&format!("parameter:service:c.ShapeServices.update#Example@in.{name}"))
            .map(|row| (row.origin, row.summary))
    };
    // the interface's auto-parameters read the implementing service's noun
    assert_eq!(
        summary("exampleId"),
        Some(("implements".to_string(), "in id, required=false".to_string()))
    );
    // a declared parameter that sets nothing keeps the implemented one's required, and its row says so
    assert_eq!(
        summary("mode"),
        Some((String::new(), "in String, required=true".to_string()))
    );
    // one that sets required keeps the implemented type
    assert_eq!(
        summary("size"),
        Some((String::new(), "in Long, required=true".to_string()))
    );
    // an auto field sets its type and required
    assert_eq!(
        summary("name"),
        Some((
            "auto-parameters".to_string(),
            "in text-medium, required=true".to_string()
        ))
    );
    // a disabled one is none
    assert_eq!(summary("hidden"), None);
}

#[test]
fn auto_fields_take_only_the_includes_moqui_takes() {
    let screen = r#"<screen><widgets>
        <form-single name="Odd"><auto-fields-entity entity-name="c.Example" include="some"/></form-single>
        <form-list name="Auto" list="xs"><auto-fields-service service-name="create#c.Example"/></form-list>
    </widgets></screen>"#;
    let index = index_of(&[
        (ArtifactKind::Entity, "component://c/entity/Entities.xml", ENTITIES),
        (ArtifactKind::Screen, SCREEN_LOCATION, screen),
    ]);
    assert!(fields(&index, "Odd").is_empty());
    // an entity-auto name: the noun's fields, a form-list's without the very long text
    let names: Vec<String> = fields(&index, "Auto").into_iter().map(|(name, _)| name).collect();
    assert_eq!(names, ["exampleId", "name", "statusId"]);
}

#[test]
fn a_form_extending_a_form_has_the_fields_a_screen_extend_adds_to_it() {
    let extend = "component://site/screen-extend/c/screen/Things.xml";
    let screen = r#"<screen><widgets>
        <form-single name="Base"><field name="a"><default-field><text-line/></default-field></field></form-single>
        <form-single name="Child" extends="Base"/>
    </widgets></screen>"#;
    let index = index_of(&[
        (ArtifactKind::Screen, SCREEN_LOCATION, screen),
        (
            ArtifactKind::ScreenExtend,
            extend,
            r#"<screen-extend><form-single name="Base"><field name="b"><default-field><text-line/></default-field></field></form-single></screen-extend>"#,
        ),
    ]);
    assert_eq!(
        fields(&index, "Child"),
        [
            ("a".to_string(), "extends".to_string()),
            ("b".to_string(), "extends".to_string())
        ]
    );
}

#[test]
fn of_a_service_defined_twice_the_expansion_follows_one_definition_and_falls_back() {
    let one = "component://c/service/c/ThingServices.xml";
    let override_text = SERVICES.replace(r#"include="nonpk""#, r#"include="pk""#);
    let mut index = index_of(&base_files());
    read_into(
        &mut index,
        ArtifactKind::Service,
        "component://site/service/c/ThingServices.xml",
        &override_text,
    );
    index.resolve_all(&|_| false);
    let path = |index: &MemoryIndex| {
        index
            .describe("parameter:service:c.ThingServices.create#Thing@in.infoName")
            .map(|row| row.path)
    };
    // the file that sorts last, whatever order they were read in: the site's, with its own auto-parameters
    assert_eq!(
        path(&index),
        Some(path_of("component://site/service/c/ThingServices.xml"))
    );
    assert!(index
        .describe("parameter:service:c.ThingServices.create#Thing@in.exampleId")
        .is_some());
    // the override gone: the other definition's
    let site = path_of("component://site/service/c/ThingServices.xml");
    index.artifacts.retain(|artifact| artifact.path != site);
    index.refs.retain(|(file, _)| *file != site);
    index.bindings.retain(|(file, _)| *file != site);
    index.resolve_all(&|_| false);
    assert_eq!(path(&index), Some(path_of(one)));
    assert!(index
        .describe("parameter:service:c.ThingServices.create#Thing@in.exampleId")
        .is_none());
}

#[test]
fn an_entity_read_again_keeps_its_fields_order_before_its_extensions() {
    let extend = r#"<entities><extend-entity entity-name="Example" package="c"><field name="added" type="id"/></extend-entity></entities>"#;
    let mut index = index_of(&[
        (ArtifactKind::Entity, "component://c/entity/Entities.xml", ENTITIES),
        (ArtifactKind::Entity, "component://site/entity/SiteEntities.xml", extend),
        (
            ArtifactKind::Service,
            "component://c/service/c/ThingServices.xml",
            SERVICES,
        ),
    ]);
    let order = |index: &MemoryIndex| -> Vec<String> {
        index
            .artifacts
            .iter()
            .filter(|row| row.origin == "auto-parameters")
            .map(|row| row.short_name.clone())
            .collect()
    };
    let before = order(&index);
    assert_eq!(before.last().map(String::as_str), Some("added"), "{before:?}");
    read_into(
        &mut index,
        ArtifactKind::Entity,
        "component://c/entity/Entities.xml",
        ENTITIES,
    );
    index.resolve_all(&|_| false);
    assert_eq!(order(&index), before);
}

#[test]
fn a_link_reaches_a_get_transition_and_a_form_a_post_one() {
    let index = index_of(&[
        (ArtifactKind::Screen, "component://c/screen/App.xml", "<screen/>"),
        (
            ArtifactKind::Screen,
            "component://c/screen/App/Tasks.xml",
            r#"<screen><transition name="go" method="get"><default-response url="."/></transition>
                <transition name="go" method="post"><default-response url="."/></transition></screen>"#,
        ),
        (
            ArtifactKind::Screen,
            "component://c/screen/App/Order.xml",
            r#"<screen><widgets><link url="../Tasks/go" text="Go"/>
                <form-single name="Post" transition="../Tasks/go"><field name="a"><default-field><text-line/></default-field></field></form-single></widgets></screen>"#,
        ),
    ]);
    let targets: Vec<(&str, Option<&str>)> = index
        .refs
        .iter()
        .map(|(_, row)| row)
        .filter(|row| row.to_kind == ToKind::ScreenPath)
        .map(|row| (row.ref_type.as_str(), row.to_id.as_deref()))
        .collect();
    let tasks = "transition:component://c/screen/App/Tasks.xml#go";
    assert_eq!(
        targets,
        [
            ("link", Some(format!("{tasks}:get").as_str())),
            ("form", Some(format!("{tasks}:post").as_str()))
        ]
    );
}

#[test]
fn a_reader_of_an_entity_by_its_short_name_is_found_after_the_entity_is_gone() {
    let services = r#"<services>
    <service verb="save" noun="Example"><in-parameters><auto-parameters entity-name="Example"/></in-parameters></service>
    <service verb="make" noun="Example"><implements service="create#Example"/></service>
</services>"#;
    let entity = extract(
        &FileInfo {
            path: "/tmp/derive/e.xml",
            location: "component://c/entity/Entities.xml",
            component: "c",
            kind: ArtifactKind::Entity,
        },
        ENTITIES,
    )
    .unwrap();
    let service = extract(
        &FileInfo {
            path: "/tmp/derive/s.xml",
            location: "component://c/service/c/ExampleServices.xml",
            component: "c",
            kind: ArtifactKind::Service,
        },
        services,
    )
    .unwrap();
    let mut resolver = Resolver::default();
    for artifact in entity.artifacts.iter().chain(&service.artifacts) {
        resolver.add(artifact);
    }
    let readers = |resolver: &Resolver, id: &str| {
        let mut found: Vec<String> = resolver
            .dependents(&HashSet::from([id.to_string()]))
            .into_iter()
            .collect();
        found.sort();
        found
    };
    let both = [
        "service:c.ExampleServices.make#Example",
        "service:c.ExampleServices.save#Example",
    ];
    assert_eq!(readers(&resolver, "entity:c.Example"), both);
    for artifact in &entity.artifacts {
        resolver.remove(artifact);
    }
    // the update names it by its short name too (mod.rs `apply`)
    assert_eq!(readers(&resolver, "entity:Example"), both);
}
