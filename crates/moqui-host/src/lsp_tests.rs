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

//! The tests of `moqui lsp` (lsp.rs).

use super::*;

#[test]
fn uris_and_paths_both_ways() {
    let path = Path::new("/w/my site/screen/A.xml");
    assert_eq!(uri_of(path), "file:///w/my%20site/screen/A.xml");
    assert_eq!(path_of(&uri_of(path)).unwrap(), path);
    assert_eq!(path_of("file:///C:/w/a.xml").unwrap(), PathBuf::from("C:/w/a.xml"));
    assert!(path_of("untitled:1").is_none());
    // a first character of several bytes before a colon is no drive letter's slash, and is kept
    assert_eq!(path_of("file://\u{e9}:/a.xml").unwrap(), PathBuf::from("\u{e9}:/a.xml"));
    // a % before a character of several bytes is taken as it is, not sliced
    assert_eq!(
        path_of("file:///w/%a\u{e9}.xml").unwrap(),
        PathBuf::from("/w/%a\u{e9}.xml")
    );
}

#[test]
fn the_word_and_the_tag_under_the_cursor() {
    let text =
        "<screen>\n    <service-call name=\"mantle.order.OrderServices.place#Order\" in-map=\"\"/>\n    <entity-find ";
    assert_eq!(word_at(text, 1, 30), "mantle.order.OrderServices.place#Order");
    let context = |element: &str, place: TagPlace| {
        Some(TagContext {
            element: element.into(),
            place,
        })
    };
    assert_eq!(
        tag_context(text, 1, 30),
        context("service-call", TagPlace::Value("name".into()))
    );
    assert_eq!(tag_context(text, 2, 17), context("entity-find", TagPlace::Attributes));
    assert_eq!(tag_context("<screen>\n    <ent", 1, 8), context("ent", TagPlace::Name));
    assert_eq!(tag_context("<screen>text", 0, 10), None);
}

#[test]
fn a_flag_without_its_value_is_the_usage_error() {
    for flag in ["--server", "--java", "--project"] {
        let err = dispatch_lsp(&[flag.to_string()]).unwrap_err().to_string();
        assert!(err.contains("moqui help lsp"), "{flag}: {err}");
    }
}

const UP_ENTITIES: &str = r#"<entities>
    <entity entity-name="Thing" package="up">
        <field name="thingId" type="id" is-pk="true"/>
        <field name="heading" type="text-medium"/>
    </entity>
</entities>
"#;

const SITE_SERVICES: &str = r#"<services>
    <service verb="save" noun="Thing">
        <in-parameters><parameter name="label"/></in-parameters>
        <actions>
            <entity-find entity-name="up.Thing" list="things"><econdition field-name="heading" value="x"/></entity-find>
        </actions>
    </service>
    <service verb="make" noun="Thing">
        <actions><service-call name="site.ThingServices.save#Thing"><field-map field-name="label" value="x"/></service-call></actions>
    </service>
</services>
"#;

/// A file's name with its edits, each (0-based line, start character, new text).
type FileEdits = (String, Vec<(u64, u64, String)>);

/// Two components in a scratch directory, `up` (an entity) and `site` (services using it), on a factory of `conf`,
/// with a server on their memory index.
struct LspBed {
    dir: PathBuf,
    server: Server,
}

impl Drop for LspBed {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl LspBed {
    fn new(name: &str, conf: &str) -> Self {
        Self::with(name, conf, &[])
    }

    /// [`Self::new`] with the files `extra` (each a path under the directory and its text), in place of the standard
    /// ones of the same path.
    fn with(name: &str, conf: &str, extra: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("moqui-lsp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut files: Vec<(&str, &str)> = vec![
            ("up/entity/UpEntities.xml", UP_ENTITIES),
            ("site/service/site/ThingServices.xml", SITE_SERVICES),
        ];
        files.retain(|(path, _)| !extra.iter().any(|(other, _)| other == path));
        files.extend_from_slice(extra);
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        std::fs::create_dir_all(dir.join("framework")).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        // `dep` depends on `up`, and nothing refers to it
        let component = |name: &str, depends_on: &[&str]| moqui_core::context::ComponentInfo {
            name: name.to_string(),
            version: String::new(),
            location: dir.join(name),
            depends_on: depends_on.iter().map(|on| (*on).to_string()).collect(),
            purposes: Vec::new(),
        };
        let ctx = ExecutionContextFactory::test_context_with_components(
            conf,
            moqui_core::service::ServicePools::idle(),
            dir.join("framework"),
            vec![component("up", &[]), component("site", &[]), component("dep", &["up"])],
        );
        let factory = Arc::clone(ctx.factory());
        let mut files = index::directory_files("up", &dir.join("up"));
        files.extend(index::directory_files("site", &dir.join("site")));
        files.extend(index::directory_files("dep", &dir.join("dep")));
        let index = Box::new(MemoryIndex::of_files(&factory, &files));
        Self {
            dir,
            server: Server::new(Backend::Local { factory, index }),
        }
    }

    fn uri(&self, path: &str) -> String {
        uri_of(&self.dir.join(path))
    }

    fn services_uri(&self) -> String {
        self.uri("site/service/site/ThingServices.xml")
    }

    /// The answer to `method` with `params`: its result, or its error's message.
    fn ask(&mut self, method: &str, params: Json) -> Result<Json, String> {
        let response = self.server.answer(&Request::new(1.into(), method.to_string(), params));
        response.response_result.map_err(|error| error.message)
    }

    /// A rename at the 0-based `line` and `character` of `uri` to `new_name`: each file's edits as (line, start,
    /// text), by the file's name.
    fn rename(&mut self, uri: &str, line: u32, character: u32, new_name: &str) -> Result<Vec<FileEdits>, String> {
        let result = self.ask(
            "textDocument/rename",
            json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character }, "newName": new_name }),
        )?;
        let mut files: Vec<FileEdits> = result["changes"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(uri, edits)| {
                let name = uri.rsplit('/').next().unwrap_or_default().to_string();
                let edits = edits
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|edit| {
                        (
                            edit["range"]["start"]["line"].as_u64().unwrap(),
                            edit["range"]["start"]["character"].as_u64().unwrap(),
                            edit["newText"].as_str().unwrap().to_string(),
                        )
                    })
                    .collect();
                (name, edits)
            })
            .collect();
        files.sort();
        Ok(files)
    }
}

/// A service renamed from a call: its verb at its definition and its full name at the call, in one component; and
/// prepareRename names the whole name. A parameter renamed from a field-map: its definition and the field-map.
#[test]
fn a_rename_edits_the_definition_and_each_reference() {
    let jobs = "<entity-facade-xml type=\"seed\">\n    <moqui.service.job.ServiceJob jobName=\"thing\" serviceName=\"site.ThingServices.save#Thing\"/>\n</entity-facade-xml>\n";
    let mut bed = LspBed::with("rename", "", &[("site/data/Jobs.xml", jobs)]);
    let uri = bed.services_uri();
    // line 8: the service-call of save#Thing
    let prepared = bed
        .ask(
            "textDocument/prepareRename",
            json!({ "textDocument": { "uri": uri }, "position": { "line": 8, "character": 40 } }),
        )
        .unwrap();
    assert_eq!(prepared["placeholder"], "site.ThingServices.save#Thing");
    // and a data file's job that runs it
    let service = bed.rename(&uri, 8, 40, "store#Thing").unwrap();
    assert_eq!(
        service,
        [
            (
                "Jobs.xml".to_string(),
                vec![(1, 63, "site.ThingServices.store#Thing".to_string())]
            ),
            (
                "ThingServices.xml".to_string(),
                vec![
                    (1, 19, "store".to_string()),
                    (8, 37, "site.ThingServices.store#Thing".to_string())
                ]
            )
        ]
    );
    // the field-map's field-name on the same line names the parameter
    let parameter = bed.rename(&uri, 8, 94, "caption").unwrap();
    assert_eq!(
        parameter,
        [(
            "ThingServices.xml".to_string(),
            vec![(2, 40, "caption".to_string()), (8, 91, "caption".to_string())]
        )]
    );
    // not a kind it renames, and not a name
    let entity = bed.rename(&uri, 4, 40, "Other").unwrap_err();
    assert!(entity.contains("this is the entity up.Thing"), "{entity}");
    assert!(bed.rename(&uri, 8, 94, "two words").unwrap_err().contains("is no name"));
}

/// An entity field renamed from a find edits two components: refused while the conf does not say which are the
/// site's, refused when the field is in an upstream one under the lock, and edited when both are the site's.
#[test]
fn a_rename_into_an_upstream_component_is_refused_under_the_lock() {
    let mut unknown = LspBed::new("unknown", "");
    let uri = unknown.services_uri();
    let message = unknown.rename(&uri, 4, 87, "title").unwrap_err();
    assert!(message.contains("cannot tell which of them are upstream"), "{message}");
    let mut locked = LspBed::new(
        "locked",
        r#"<moqui-conf><dev-aide site-component="site"/></moqui-conf>"#,
    );
    let uri = locked.services_uri();
    let message = locked.rename(&uri, 4, 87, "title").unwrap_err();
    assert!(message.contains("in the upstream component up"), "{message}");
    let mut both = LspBed::new(
        "both",
        r#"<moqui-conf><dev-aide site-component="site,up"/></moqui-conf>"#,
    );
    let uri = both.services_uri();
    assert_eq!(
        both.rename(&uri, 4, 87, "title").unwrap(),
        [
            ("ThingServices.xml".to_string(), vec![(4, 86, "title".to_string())]),
            ("UpEntities.xml".to_string(), vec![(3, 21, "title".to_string())]),
        ]
    );
    // a find of an entity the index does not know names the field too: it may be this one
    let other = both.uri("site/service/site/OtherServices.xml");
    let text = r#"<services><service verb="find" noun="Other"><actions>
    <entity-find entity-name="other.Missing" list="found"><econdition field-name="heading" value="x"/></entity-find>
</actions></service></services>"#;
    both.server.notify(&Notification::new(
        "textDocument/didOpen".into(),
        json!({ "textDocument": { "uri": other, "text": text } }),
    ));
    let message = both.rename(&uri, 4, 87, "title").unwrap_err();
    assert!(
        message.contains("OtherServices.xml:2 names heading and does not resolve"),
        "{message}"
    );
}

/// The quick fixes of an unknown entity and service: the nearest known names, each an edit of the value.
#[test]
fn an_unknown_name_gets_the_nearest_known_names_as_quick_fixes() {
    let mut bed = LspBed::new("fixes", "");
    let uri = bed.uri("site/service/site/Other.xml");
    let text = "<services><service verb=\"run\" noun=\"Other\"><actions>\n    <entity-find-one entity-name=\"up.Thng\" value-field=\"thing\"/>\n    <service-call name=\"site.ThingServices.sav#Thing\"/>\n</actions></service></services>\n";
    bed.server.notify(&Notification::new(
        "textDocument/didOpen".into(),
        json!({ "textDocument": { "uri": uri, "text": text } }),
    ));
    let diagnostic = |line: u32, rule: &str, message: &str| {
        json!({ "range": { "start": { "line": line, "character": 4 }, "end": { "line": line, "character": 10 } },
            "code": rule, "message": message })
    };
    let actions = bed
        .ask(
            "textDocument/codeAction",
            json!({ "textDocument": { "uri": uri },
                "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 2, "character": 0 } },
                "context": { "diagnostics": [
                    diagnostic(1, "reference.entity", "entity up.Thng: No entity or view entity has this name. [reference.entity]"),
                    diagnostic(2, "reference.service", "service site.ThingServices.sav#Thing: No service has this name. [reference.service]"),
                    diagnostic(2, "expression", "something else"),
                ] } }),
        )
        .unwrap();
    let fixes: Vec<(String, u64, u64, String)> = actions
        .as_array()
        .unwrap()
        .iter()
        .map(|action| {
            let edit = &action["edit"]["changes"][uri.as_str()][0];
            (
                action["title"].as_str().unwrap().to_string(),
                edit["range"]["start"]["line"].as_u64().unwrap(),
                edit["range"]["start"]["character"].as_u64().unwrap(),
                edit["newText"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        fixes,
        [
            ("Change to up.Thing".to_string(), 1, 34, "up.Thing".to_string()),
            // the nearest first
            (
                "Change to site.ThingServices.save#Thing".to_string(),
                2,
                24,
                "site.ThingServices.save#Thing".to_string()
            ),
            (
                "Change to site.ThingServices.make#Thing".to_string(),
                2,
                24,
                "site.ThingServices.make#Thing".to_string()
            ),
        ]
    );
}

#[test]
fn the_nearest_names_are_within_a_third_of_the_names_length() {
    let known = ["OrderHeader", "OrderItem", "OrderPart", "Party"]
        .map(String::from)
        .to_vec();
    assert_eq!(edit::rank_nearest("OrderHeadr", known.clone()), ["OrderHeader"]);
    assert_eq!(edit::rank_nearest("OrderItm", known.clone()), ["OrderItem"]);
    assert!(edit::rank_nearest("Shipment", known).is_empty());
    assert_eq!(
        edit::unknown_name(
            "reference.field",
            "field statusId of mantle.order.OrderHeader: The entity ..."
        ),
        Some(edit::UnknownName::Field {
            name: "statusId".into(),
            entity: "mantle.order.OrderHeader".into()
        })
    );
    // several fields at once: no one fix
    assert_eq!(edit::unknown_name("reference.field", "field a, b of E: x"), None);
}

/// A relationship to the field's entity that joins by its key: its key-map's `related` is edited with the field, and
/// one with no `related`, whose related field has the key-map's own field's name, refuses the rename.
#[test]
fn a_rename_edits_a_key_maps_related_and_refuses_an_implied_one() {
    let site = r#"<entities><entity entity-name="Part" package="site">
    <field name="partId" type="id" is-pk="true"/><field name="partThing" type="id"/>
    <relationship type="one" related="up.Thing"><key-map field-name="partThing" related="thingId"/></relationship>
</entity></entities>"#;
    let conf = r#"<moqui-conf><dev-aide site-component="site,up"/></moqui-conf>"#;
    let mut bed = LspBed::with("key-maps", conf, &[("site/entity/SiteEntities.xml", site)]);
    let uri = bed.uri("up/entity/UpEntities.xml");
    let (line, character) = position(UP_ENTITIES, "name=\"thingId\"", 7);
    let edits = bed.rename(&uri, line, character, "thingKey").unwrap();
    let (related_line, related_column) = position(site, "related=\"thingId\"", 9);
    assert_eq!(
        edits,
        [
            (
                "SiteEntities.xml".to_string(),
                vec![(
                    u64::from(related_line),
                    u64::from(related_column),
                    "thingKey".to_string()
                )]
            ),
            ("UpEntities.xml".to_string(), vec![(2, 21, "thingKey".to_string())]),
        ]
    );
    let implied = site.replace(" related=\"thingId\"", "");
    let mut bed = LspBed::with("key-maps-implied", conf, &[("site/entity/SiteEntities.xml", &implied)]);
    let uri = bed.uri("up/entity/UpEntities.xml");
    let message = bed.rename(&uri, line, character, "thingKey").unwrap_err();
    assert!(
        message.contains("has no related, so it joins to the field thingId by its own field"),
        "{message}"
    );
}

/// The safety scan: the quoted name left in a component the rename touches, where the index knows no artifact it
/// names, refuses the rename and says each place.
#[test]
fn a_rename_is_refused_where_the_quoted_name_is_left() {
    let uses = "<services>\n    <service verb=\"use\" noun=\"Label\"><actions><set field=\"copy\" from=\"label\"/></actions></service>\n</services>\n";
    let mut bed = LspBed::with("scan", "", &[("site/service/site/Uses.xml", uses)]);
    let uri = bed.services_uri();
    let message = bed.rename(&uri, 8, 94, "caption").unwrap_err();
    assert!(
        message.contains("label is written where the rename does not edit it"),
        "{message}"
    );
    assert!(message.contains("Uses.xml:2"), "{message}");
}

/// Two references of a script file on one line are each edited at its own place, by the column the index read it at.
#[test]
fn a_rename_edits_each_script_literal_at_its_column() {
    let script = "// twice\nec.service.sync().name(\"site.ThingServices.save#Thing\").call(); ec.service.sync().name(\"site.ThingServices.save#Thing\").call()\n";
    let mut bed = LspBed::with("scripts", "", &[("site/script/Twice.groovy", script)]);
    let uri = bed.services_uri();
    let edits = bed.rename(&uri, 8, 40, "store#Thing").unwrap();
    let line = script.lines().nth(1).unwrap();
    let column = |at: usize| u64::try_from(line[..at].encode_utf16().count() + 1).unwrap();
    let new = "site.ThingServices.store#Thing".to_string();
    assert_eq!(
        edits[1],
        (
            "Twice.groovy".to_string(),
            vec![
                (1, column(line.find('"').unwrap()), new.clone()),
                (1, column(line.rfind("\"site").unwrap()), new),
            ]
        )
    );
}

/// The 0-based line and UTF-16 character of the first character of `needle` in `text`, plus `into` characters.
fn position(text: &str, needle: &str, into: u32) -> (u32, u32) {
    let at = text.find(needle).expect("the needle is in the text");
    let line = u32::try_from(text[..at].matches('\n').count()).unwrap();
    let start = text[..at].rfind('\n').map_or(0, |found| found + 1);
    let column = u32::try_from(text[start..at].encode_utf16().count()).unwrap();
    (line, column + into)
}

const MORE_SERVICES: &str = r#"<services>
    <service verb="find" noun="More">
        <actions>
            <entity-find entity-name="up.Thing" list="found"><econdition field-name="thingId"/></entity-find>
            <service-call name="site.ThingServices.save#Thing"><field-map field-name="label" from="caption"/></service-call>
            <service-call name="site.Missing.run#Gone"><field-map field-name="label" value="x"/></service-call>
        </actions>
    </service>
    <service verb="update" noun="Widget"><in-parameters><auto-parameters/></in-parameters></service>
</services>
"#;

/// The refusals, each with its reason: a name taken, a use that reads the context value of the name (an `econdition`
/// with no `from`), an unresolved reference of the name whose service the index does not know, a noun change of a
/// service whose auto-parameters read its noun, a prefixed alias-all; and a cursor on a `from` renames nothing.
#[test]
fn a_rename_is_refused_where_it_cannot_follow_the_name() {
    let mut bed = LspBed::with("refused", "", &[("site/service/site/MoreServices.xml", MORE_SERVICES)]);
    let uri = bed.uri("site/service/site/MoreServices.xml");
    let at = |needle: &str, into: u32| position(MORE_SERVICES, needle, into);
    let refused = |bed: &mut LspBed, (line, character): (u32, u32), new_name: &str| {
        bed.rename(&uri, line, character, new_name).unwrap_err()
    };
    let taken = refused(&mut bed, at("site.ThingServices.save#Thing", 3), "make#Thing");
    assert!(taken.contains("make#Thing is there already"), "{taken}");
    let implied = refused(&mut bed, at("field-name=\"thingId\"", 13), "thingKey");
    assert!(
        implied.contains("has no from, so it reads the value named thingId"),
        "{implied}"
    );
    let unresolved = refused(&mut bed, at("field-name=\"label\" from", 13), "caption");
    assert!(
        unresolved.contains("MoreServices.xml:6 names label and does not resolve"),
        "{unresolved}"
    );
    let noun = refused(&mut bed, at("noun=\"Widget\"", 7), "update#Gadget");
    assert!(noun.contains("reads its noun"), "{noun}");
    // the from of a field-map names a value, not the parameter
    let from = refused(&mut bed, at("from=\"caption\"", 7), "other");
    assert!(from.contains("the from of field-map names no artifact"), "{from}");

    let views = r#"<entities><view-entity entity-name="ThingView" package="up">
    <member-entity entity-alias="T" entity-name="up.Thing"/><alias-all entity-alias="T" prefix="thing"/>
</view-entity></entities>"#;
    let mut viewed = LspBed::with("prefixed", "", &[("up/entity/UpViews.xml", views)]);
    let entities = viewed.uri("up/entity/UpEntities.xml");
    let (line, character) = position(UP_ENTITIES, "name=\"heading\"", 7);
    let prefixed = viewed.rename(&entities, line, character, "title").unwrap_err();
    assert!(
        prefixed.contains("aliases it as thingHeading by a prefixed alias-all"),
        "{prefixed}"
    );
}

/// An entity field renamed at its definition, in a file of CRLF lines with characters of several bytes before the
/// name: the definition, the parameter its service's auto-parameters makes and the field-map naming it, the form field
/// bound to that parameter with its field-ref, and an exclude that leaves it out of another form; the alias-all's alias
/// of a view follows with no text of its own.
#[test]
fn a_rename_goes_through_alias_all_auto_parameters_and_a_form_binding() {
    let entities = "<entities>\r\n    <entity entity-name=\"Thing\" package=\"up\">\r\n        <field name=\"thingId\" type=\"id\" is-pk=\"true\"/>\r\n        <field description=\"\u{dc}n\u{ef}code \u{2713}\" name=\"heading\" type=\"text-medium\"/>\r\n    </entity>\r\n    <view-entity entity-name=\"ThingView\" package=\"up\"><member-entity entity-alias=\"T\" entity-name=\"up.Thing\"/><alias-all entity-alias=\"T\"/></view-entity>\r\n</entities>\r\n";
    let services = r#"<services>
    <service verb="save" noun="Item">
        <in-parameters><auto-parameters entity-name="up.Thing"/></in-parameters>
    </service>
    <service verb="make" noun="Item">
        <actions><service-call name="site.ThingServices.save#Item"><field-map field-name="heading" value="x"/></service-call></actions>
    </service>
</services>
"#;
    let screen = r#"<screen>
    <transition name="save"><service-call name="site.ThingServices.save#Item"/><default-response url="."/></transition>
    <widgets>
        <form-single name="Edit" transition="save">
            <field name="heading"><default-field><text-line/></default-field></field>
            <field-layout><field-ref name="heading"/></field-layout>
        </form-single>
        <form-list name="List" list="things"><auto-fields-entity entity-name="up.Thing"><exclude field-name="heading"/></auto-fields-entity></form-list>
    </widgets>
</screen>
"#;
    let mut bed = LspBed::with(
        "through",
        r#"<moqui-conf><dev-aide site-component="site,up"/></moqui-conf>"#,
        &[
            ("up/entity/UpEntities.xml", entities),
            ("site/service/site/ThingServices.xml", services),
            ("site/screen/Things.xml", screen),
        ],
    );
    let uri = bed.uri("up/entity/UpEntities.xml");
    // the field's definition: line 3, its name after the description's characters of several bytes
    let edits = bed.rename(&uri, 3, 46, "title").unwrap();
    let title = |line: u64, start: u64| (line, start, "title".to_string());
    let place = |text: &str, needle: &str, into: u32| {
        let (line, character) = position(text, needle, into);
        title(u64::from(line), u64::from(character))
    };
    assert_eq!(
        edits,
        [
            (
                "ThingServices.xml".to_string(),
                vec![place(services, "field-name=\"heading\"", 12)]
            ),
            (
                "Things.xml".to_string(),
                vec![
                    place(screen, "<field name=\"heading\"", 13),
                    place(screen, "<field-ref name=\"heading\"", 17),
                    place(screen, "<exclude field-name=\"heading\"", 21),
                ]
            ),
            ("UpEntities.xml".to_string(), vec![title(3, 45)]),
        ]
    );
}

/// The safety scan reaches every component that depends on the target's, and matches the name as a token in any text:
/// an expression's `row.heading` in a `.qvue` of a component that refers to nothing.
#[test]
fn the_safety_scan_reaches_dependent_components_and_expressions() {
    let qvue = "<template><div>{{ row.heading }}</div></template>\n";
    let conf = r#"<moqui-conf><dev-aide site-component="site,up,dep"/></moqui-conf>"#;
    let mut bed = LspBed::with("dependents", conf, &[("dep/screen/Thing.qvue", qvue)]);
    let uri = bed.uri("up/entity/UpEntities.xml");
    let (line, character) = position(UP_ENTITIES, "name=\"heading\"", 7);
    let message = bed.rename(&uri, line, character, "title").unwrap_err();
    assert!(message.contains("dep/screen/Thing.qvue:1"), "{message}");
}

/// A relationship with no key-map joins the related primary key to an own field of its name, and a key-map with no
/// related joins a field of its own name: renaming a field in such an implied map is refused, on either side.
#[test]
fn a_rename_is_refused_where_a_relationship_implies_the_key() {
    let conf = r#"<moqui-conf><dev-aide site-component="site,up"/></moqui-conf>"#;
    let thing_id = position(UP_ENTITIES, "name=\"thingId\"", 7);
    let refused = |name: &str, entities: &str| {
        let mut bed = LspBed::with(name, conf, &[("site/entity/SiteEntities.xml", entities)]);
        let uri = bed.uri("up/entity/UpEntities.xml");
        bed.rename(&uri, thing_id.0, thing_id.1, "thingKey").unwrap_err()
    };
    let no_key_map = refused(
        "implied-pk",
        r#"<entities><entity entity-name="Part" package="site">
    <field name="partId" type="id" is-pk="true"/><field name="thingId" type="id"/>
    <relationship type="one" related="up.Thing"/>
</entity></entities>"#,
    );
    assert!(
        no_key_map.contains("has no key-map, so it joins to the primary key thingId"),
        "{no_key_map}"
    );
    let many = refused(
        "implied-many",
        r#"<entities><entity entity-name="Part" package="site">
    <field name="partId" type="id" is-pk="true"/><field name="thingId" type="id"/>
    <relationship type="many" related="up.Thing"><key-map field-name="thingId"/></relationship>
</entity></entities>"#,
    );
    assert!(many.contains("has no related"), "{many}");
    // a view member joining from a member the index cannot follow
    let unknown = refused(
        "unknown-member",
        r#"<entities><view-entity entity-name="PartView" package="site">
    <member-entity entity-alias="T" entity-name="up.Thing"/>
    <member-relationship entity-alias="X" join-from-alias="T" relationship="Nope"/>
    <member-entity entity-alias="U" entity-name="up.Thing" join-from-alias="X"><key-map field-name="thingId" related="thingId"/></member-entity>
</view-entity></entities>"#,
    );
    assert!(unknown.contains("of a member the index cannot follow"), "{unknown}");
}

/// The scan reads plain text in every component and every open buffer: a data record's child element of the field,
/// a search suffix (`heading_op`), a `$heading`, a CDATA script, the upper snake form for SQL, a new buffer not saved
/// yet each refuse the rename; a binary file is passed over, and a file that is not UTF-8 refuses it, named.
#[test]
fn the_safety_scan_reads_plain_text_everywhere() {
    let conf = r#"<moqui-conf><dev-aide site-component="site,up,dep"/></moqui-conf>"#;
    let heading = position(UP_ENTITIES, "name=\"heading\"", 7);
    let thing_id = position(UP_ENTITIES, "name=\"thingId\"", 7);
    let refused = |name: &str, path: &str, text: &str, (line, character): (u32, u32), new: &str| {
        let mut bed = LspBed::with(name, conf, &[(path, text)]);
        let uri = bed.uri("up/entity/UpEntities.xml");
        bed.rename(&uri, line, character, new).unwrap_err()
    };
    let cases = [
        (
            "child",
            "dep/data/Things.xml",
            "<entity-facade-xml>\n<up.Thing thingId=\"1\"><heading>x</heading></up.Thing>\n</entity-facade-xml>\n",
            heading,
            "Things.xml:2",
        ),
        (
            "suffix",
            "dep/screen/Find.xml",
            "<screen>\n<parameter name=\"heading_op\"/>\n</screen>\n",
            heading,
            "Find.xml:2",
        ),
        (
            "dollar",
            "dep/template/Thing.ftl",
            "<p>$heading</p>\n",
            heading,
            "Thing.ftl:1",
        ),
        (
            "cdata",
            "dep/service/dep/Scripts.xml",
            "<services><service verb=\"run\" noun=\"It\"><actions><script><![CDATA[\nif (a < 3) x = row.heading\n]]></script></actions></service></services>\n",
            heading,
            "Scripts.xml:2",
        ),
        (
            "sql",
            "dep/sql/things.sql",
            "SELECT THING_ID FROM THING\n",
            thing_id,
            "things.sql:1",
        ),
    ];
    for (name, path, text, at, place) in cases {
        let new = if at == heading { "title" } else { "thingKey" };
        let message = refused(name, path, text, at, new);
        assert!(message.contains(place), "{name}: {message}");
    }
    // a binary file is passed over; a text file that is not UTF-8 refuses the rename, named
    let mut binary = LspBed::with("binary", conf, &[]);
    std::fs::create_dir_all(binary.dir.join("dep/images")).unwrap();
    std::fs::write(binary.dir.join("dep/images/thing.png"), b"\x89PNG heading").unwrap();
    std::fs::write(binary.dir.join("dep/images/raw.dat"), b"heading\x00").unwrap();
    let uri = binary.uri("up/entity/UpEntities.xml");
    binary.rename(&uri, heading.0, heading.1, "title").unwrap();
    std::fs::write(binary.dir.join("dep/images/latin.txt"), b"caf\xe9").unwrap();
    let not_utf8 = binary.rename(&uri, heading.0, heading.1, "title").unwrap_err();
    assert!(not_utf8.contains("latin.txt is not UTF-8"), "{not_utf8}");
    std::fs::remove_file(binary.dir.join("dep/images/latin.txt")).unwrap();
    // a new buffer not saved yet
    let new_file = binary.uri("dep/screen/New.xml");
    binary.server.notify(&Notification::new(
        "textDocument/didOpen".into(),
        json!({ "textDocument": { "uri": new_file, "text": "<screen><label text=\"${heading}\"/></screen>" } }),
    ));
    let unsaved = binary.rename(&uri, heading.0, heading.1, "title").unwrap_err();
    assert!(unsaved.contains("New.xml:1"), "{unsaved}");
}

// ---- Extend in my-site ----

const UP_SCREEN: &str = r#"<screen>
    <widgets>
        <form-single name="EditThing" transition="update">
            <field name="thingId"><default-field><display/></default-field></field>
            <field name="heading"><default-field><text-line/></default-field></field>
            <field-layout><field-ref name="thingId"/><field-ref name="heading"/></field-layout>
        </form-single>
        <form-list name="ThingList" list="things">
            <field name="thingId"><default-field><display/></default-field></field>
            <field name="heading"><default-field><display/></default-field></field>
            <form-list-column><field-ref name="thingId"/></form-list-column>
            <form-list-column><field-ref name="heading"/></form-list-column>
        </form-list>
    </widgets>
</screen>
"#;

/// `git` in `dir`, kept from the desktop's configuration and prompts (the testing skill).
fn git_in(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "credential.helper=",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
        ])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "false")
        .env("SSH_ASKPASS", "false")
        .env("HOME", dir)
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A bed whose files are committed, with the upstream screen beside the entity.
fn committed_bed(name: &str, conf: &str) -> LspBed {
    let mut bed = LspBed::with(name, conf, &[("up/screen/Things.xml", UP_SCREEN)]);
    git_in(&bed.dir, &["init", "-q"]);
    git_in(&bed.dir, &["add", "-A"]);
    git_in(&bed.dir, &["commit", "-q", "-m", "base"]);
    bed.server.set_client(&CREATES_FILES);
    bed
}

/// A client's `initialize` whose `WorkspaceEdit`s take `documentChanges` with a `create`.
static CREATES_FILES: std::sync::LazyLock<Json> = std::sync::LazyLock::new(|| {
    json!({ "capabilities": { "workspace": { "workspaceEdit": {
        "documentChanges": true, "resourceOperations": ["create", "rename", "delete"] } } } })
});

/// `text` with one protocol `TextEdit` applied.
fn applied(text: &str, edit: &Json) -> String {
    let offset = |position: &Json| {
        let line = usize::try_from(position["line"].as_u64().unwrap()).unwrap();
        let character = usize::try_from(position["character"].as_u64().unwrap()).unwrap();
        let start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
        let rest = &text[start..];
        let mut units = 0;
        let at = rest
            .char_indices()
            .find(|(_, ch)| {
                let here = units >= character;
                units += ch.len_utf16();
                here
            })
            .map_or(rest.len(), |(at, _)| at);
        start + at
    };
    let (from, to) = (offset(&edit["range"]["start"]), offset(&edit["range"]["end"]));
    format!("{}{}{}", &text[..from], edit["newText"].as_str().unwrap(), &text[to..])
}

impl LspBed {
    /// Open `path` with `text`: its diagnostics' (0-based line, rule) pairs.
    fn open(&mut self, path: &str, text: &str) -> Vec<(u64, String)> {
        let notes = self.server.notify(&Notification::new(
            "textDocument/didOpen".into(),
            json!({ "textDocument": { "uri": self.uri(path), "text": text, "version": 3 } }),
        ));
        notes
            .iter()
            .flat_map(|note| note.params["diagnostics"].as_array().cloned().unwrap_or_default())
            .map(|diagnostic| {
                (
                    diagnostic["range"]["start"]["line"].as_u64().unwrap(),
                    diagnostic["code"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    /// The code actions at the 0-based `line` of `path`, with no diagnostics sent.
    fn actions_at(&mut self, path: &str, line: u32) -> Vec<Json> {
        let result = self
            .ask(
                "textDocument/codeAction",
                json!({ "textDocument": { "uri": self.uri(path) },
                    "range": { "start": { "line": line, "character": 0 }, "end": { "line": line, "character": 0 } },
                    "context": { "diagnostics": [] } }),
            )
            .unwrap();
        result.as_array().unwrap().clone()
    }
}

const SITE_CONF: &str = r#"<moqui-conf><dev-aide site-component="site"/></moqui-conf>"#;

/// A field added in place to an upstream entity: an `upstream.edit` diagnostic at it, and Extend in site, which takes
/// the entity file back to its committed text and makes the site's entity file with the field in an extend-entity.
#[test]
fn a_field_added_to_an_upstream_entity_is_extended_in_the_site() {
    let mut bed = committed_bed("extend-entity", SITE_CONF);
    let path = "up/entity/UpEntities.xml";
    let edited = UP_ENTITIES.replace(
        "        <field name=\"heading\" type=\"text-medium\"/>\n",
        "        <field name=\"heading\" type=\"text-medium\"/>\n        <field name=\"note\" type=\"text-long\">\n            <description>A note on it</description>\n        </field>\n",
    );
    assert!(edited != UP_ENTITIES);
    let diagnostics = bed.open(path, &edited);
    assert!(
        diagnostics.contains(&(4, "upstream.edit".to_string())),
        "{diagnostics:?}"
    );
    // nothing on a line it did not add
    assert!(bed.actions_at(path, 1).is_empty());
    let actions = bed.actions_at(path, 5);
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert_eq!(actions[0]["title"], "Extend in site: field note of up.Thing");
    let changes = actions[0]["edit"]["documentChanges"].as_array().unwrap();
    assert_eq!(changes[0]["textDocument"]["uri"], bed.uri(path));
    // the open file's edit is of the version it was made on
    assert_eq!(changes[0]["textDocument"]["version"], 3);
    assert_eq!(applied(&edited, &changes[0]["edits"][0]), UP_ENTITIES);
    let site_file = bed.uri("site/entity/SiteEntities.xml");
    assert_eq!(changes[1]["kind"], "create");
    assert_eq!(changes[1]["uri"], site_file);
    // a file made since is not written over
    assert_eq!(
        changes[1]["options"],
        json!({ "overwrite": false, "ignoreIfExists": false })
    );
    assert_eq!(changes[2]["textDocument"]["uri"], site_file);
    let made = applied("", &changes[2]["edits"][0]);
    assert!(
        made.contains("    <extend-entity entity-name=\"Thing\" package=\"up\">\n        <field name=\"note\" type=\"text-long\">\n            <description>A note on it</description>\n        </field>\n    </extend-entity>\n</entities>"),
        "{made}"
    );
    // a site file that extends the entity already gets the field in its extend-entity, after its last field (before
    // its relationship), in the file's own line endings
    let existing = "<entities>\r\n    <extend-entity entity-name=\"Thing\" package=\"up\">\r\n        <field name=\"other\" type=\"id\"/>\r\n        <relationship type=\"one\" related=\"up.Thing\"/>\r\n    </extend-entity>\r\n</entities>\r\n";
    std::fs::create_dir_all(bed.dir.join("site/entity")).unwrap();
    std::fs::write(bed.dir.join("site/entity/Ext.xml"), existing).unwrap();
    let actions = bed.actions_at(path, 4);
    let changes = actions[0]["edit"]["documentChanges"].as_array().unwrap();
    assert_eq!(changes.len(), 2, "no file made: {changes:?}");
    assert_eq!(changes[1]["textDocument"]["uri"], bed.uri("site/entity/Ext.xml"));
    assert_eq!(
        applied(existing, &changes[1]["edits"][0]),
        "<entities>\r\n    <extend-entity entity-name=\"Thing\" package=\"up\">\r\n        <field name=\"other\" type=\"id\"/>\r\n        <field name=\"note\" type=\"text-long\">\r\n            <description>A note on it</description>\r\n        </field>\r\n        <relationship type=\"one\" related=\"up.Thing\"/>\r\n    </extend-entity>\r\n</entities>\r\n"
    );
    // a client that takes no create in its edits is offered none
    bed.server
        .set_client(&json!({ "capabilities": { "workspace": { "workspaceEdit": { "documentChanges": true } } } }));
    assert!(bed.actions_at(path, 4).is_empty());
    bed.server.set_client(&CREATES_FILES);
    // more than an added field is no mechanical edit: the diagnostic at the top, no action
    let changed = edited.replace("type=\"text-medium\"", "type=\"text-long\"");
    let diagnostics = bed.open(path, &changed);
    assert!(
        diagnostics.contains(&(0, "upstream.edit".to_string())),
        "{diagnostics:?}"
    );
    assert!(bed.actions_at(path, 4).is_empty());
}

/// A field added to an upstream form with its layout ref, and a column added to an upstream list with a column of its
/// own: form merges in the site's screen-extend of the screen, each field after the one before it.
#[test]
fn a_field_and_a_column_added_to_upstream_forms_are_form_merges() {
    let mut bed = committed_bed("extend-screen", SITE_CONF);
    let path = "up/screen/Things.xml";
    let form_field = "<field name=\"heading\"><default-field><text-line/></default-field></field>\n";
    let list_field = "<field name=\"heading\"><default-field><display/></default-field></field>\n";
    let edited = UP_SCREEN
        .replace(
            form_field,
            &format!("{form_field}            <field name=\"note\"><default-field><text-area/></default-field></field>\n"),
        )
        .replace(
            "<field-ref name=\"heading\"/></field-layout>",
            "<field-ref name=\"heading\"/><field-ref name=\"note\"/></field-layout>",
        )
        .replace(
            list_field,
            &format!("{list_field}            <field name=\"note\"><default-field><display/></default-field></field>\n"),
        )
        .replace(
            "<form-list-column><field-ref name=\"heading\"/></form-list-column>\n",
            "<form-list-column><field-ref name=\"heading\"/></form-list-column>\n            <form-list-column><field-ref name=\"note\"/></form-list-column>\n",
        );
    assert_eq!(edited.matches("\"note\"").count(), 4, "{edited}");
    bed.open(path, &edited);
    let actions = bed.actions_at(path, 5);
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert_eq!(
        actions[0]["title"],
        "Extend in site: field note of form EditThing, column note of form ThingList"
    );
    let changes = actions[0]["edit"]["documentChanges"].as_array().unwrap();
    assert_eq!(applied(&edited, &changes[0]["edits"][0]), UP_SCREEN);
    assert_eq!(changes[1]["uri"], bed.uri("site/screen-extend/up/screen/Things.xml"));
    let made = applied("", &changes[2]["edits"][0]);
    assert!(made.starts_with("<?xml"), "{made}");
    assert!(
        made.contains("xsi:noNamespaceSchemaLocation=\"http://moqui.org/xsd/mr-xml-screen.xsd\">\n    <form-single name=\"EditThing\">\n        <field name=\"note\" after=\"heading\"><default-field><text-area/></default-field></field>\n    </form-single>\n    <form-list name=\"ThingList\">\n        <field name=\"note\" after=\"heading\"><default-field><display/></default-field></field>\n    </form-list>\n</screen-extend>"),
        "{made}"
    );
    // a field put first, which a merge's after= cannot place, is not mechanical
    let first = UP_SCREEN.replace(
        "<form-single name=\"EditThing\" transition=\"update\">\n",
        "<form-single name=\"EditThing\" transition=\"update\">\n            <field name=\"note\"><default-field><text-area/></default-field></field>\n",
    );
    bed.open(path, &first);
    assert!(bed.actions_at(path, 3).is_empty());
    // nor a field of a list with columns that has no column of its own
    let no_column = UP_SCREEN.replace(
        list_field,
        &format!("{list_field}            <field name=\"note\"><default-field><display/></default-field></field>\n"),
    );
    bed.open(path, &no_column);
    assert!(bed.actions_at(path, 10).is_empty());
}

/// With no site component in the conf nothing is upstream: no diagnostic and no action, and the same with the lock off.
#[test]
fn with_no_site_component_nothing_is_extended() {
    let path = "up/entity/UpEntities.xml";
    let edited = UP_ENTITIES.replace(
        "        <field name=\"heading\" type=\"text-medium\"/>\n",
        "        <field name=\"heading\" type=\"text-medium\"/>\n        <field name=\"note\" type=\"text-long\"/>\n",
    );
    for (name, conf) in [
        ("extend-none", ""),
        (
            "extend-unlocked",
            r#"<moqui-conf><dev-aide site-component="site" allow-upstream-edits="true"/></moqui-conf>"#,
        ),
    ] {
        let mut bed = committed_bed(name, conf);
        let diagnostics = bed.open(path, &edited);
        assert!(
            !diagnostics.iter().any(|(_, rule)| rule == "upstream.edit"),
            "{name}: {diagnostics:?}"
        );
        let diagnostic = json!({ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } },
            "code": "upstream.edit", "message": "x" });
        let actions = bed
            .ask(
                "textDocument/codeAction",
                json!({ "textDocument": { "uri": bed.uri(path) },
                    "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 4, "character": 0 } },
                    "context": { "diagnostics": [diagnostic] } }),
            )
            .unwrap();
        assert_eq!(actions, json!([]), "{name}");
    }
}

/// An entity with no package is extended with no package attribute; a file of mixed line endings goes back to HEAD's
/// bytes exactly.
#[test]
fn an_entity_with_no_package_and_a_file_of_mixed_line_endings() {
    let mut bed = committed_bed("extend-plain", SITE_CONF);
    let path = "up/entity/Plain.xml";
    let committed = "<entities>\r\n    <entity entity-name=\"Plain\">\n        <field name=\"plainId\" type=\"id\" is-pk=\"true\"/>\r\n    </entity>\n</entities>\n";
    std::fs::write(bed.dir.join(path), committed).unwrap();
    git_in(&bed.dir, &["add", path]);
    git_in(&bed.dir, &["commit", "-q", "-m", "plain"]);
    let edited = committed.replace(
        "is-pk=\"true\"/>\r\n",
        "is-pk=\"true\"/>\r\n        <field name=\"note\" type=\"text-long\"/>\n",
    );
    bed.open(path, &edited);
    let actions = bed.actions_at(path, 3);
    assert_eq!(actions.len(), 1, "{actions:?}");
    let changes = actions[0]["edit"]["documentChanges"].as_array().unwrap();
    assert_eq!(applied(&edited, &changes[0]["edits"][0]), committed);
    let made = applied("", &changes[2]["edits"][0]);
    assert!(
        made.contains("<extend-entity entity-name=\"Plain\">\n        <field name=\"note\" type=\"text-long\"/>\n    </extend-entity>"),
        "{made}"
    );
}

/// A partial clone whose committed blob is missing, its promisor remote an `ext::` command: reading the committed text
/// (which opening an upstream file does) fetches nothing and runs nothing.
#[cfg(unix)]
#[test]
fn the_committed_text_never_fetches_from_a_planted_promisor_remote() {
    let bed = committed_bed("extend-promisor", SITE_CONF);
    let marker = bed.dir.join("RAN");
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&bed.dir)
        .args(["rev-parse", "HEAD:up/entity/UpEntities.xml"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    let blob = String::from_utf8(output.stdout).unwrap();
    let blob = blob.trim();
    std::fs::remove_file(bed.dir.join(".git/objects").join(&blob[..2]).join(&blob[2..])).unwrap();
    let url = format!("ext::sh -c touch% {}% >&2", marker.display());
    for (key, value) in [
        ("core.repositoryformatversion", "1"),
        ("extensions.partialClone", "origin"),
        ("remote.origin.url", url.as_str()),
        ("remote.origin.promisor", "true"),
        ("remote.origin.partialclonefilter", "blob:none"),
        ("protocol.ext.allow", "always"),
    ] {
        git_in(&bed.dir, &["config", key, value]);
    }
    assert_eq!(extend::committed_text(&bed.dir.join("up/entity/UpEntities.xml")), None);
    assert!(
        !marker.exists(),
        "reading the committed text ran the promisor's command"
    );
}
