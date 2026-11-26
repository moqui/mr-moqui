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
        <field name="name" type="text-medium"/>
    </entity>
</entities>
"#;

const SITE_SERVICES: &str = r#"<services>
    <service verb="save" noun="Thing">
        <in-parameters><parameter name="label"/></in-parameters>
        <actions>
            <entity-find entity-name="up.Thing" list="things"><econdition field-name="name" value="x"/></entity-find>
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
        let component = |name: &str| moqui_core::context::ComponentInfo {
            name: name.to_string(),
            version: String::new(),
            location: dir.join(name),
            depends_on: Vec::new(),
            purposes: Vec::new(),
        };
        let ctx = ExecutionContextFactory::test_context_with_components(
            conf,
            moqui_core::service::ServicePools::idle(),
            dir.join("framework"),
            vec![component("up"), component("site")],
        );
        let factory = Arc::clone(ctx.factory());
        let mut files = index::directory_files("up", &dir.join("up"));
        files.extend(index::directory_files("site", &dir.join("site")));
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
    <entity-find entity-name="other.Missing" list="found"><econdition field-name="name" value="x"/></entity-find>
</actions></service></services>"#;
    both.server.notify(&Notification::new(
        "textDocument/didOpen".into(),
        json!({ "textDocument": { "uri": other, "text": text } }),
    ));
    let message = both.rename(&uri, 4, 87, "title").unwrap_err();
    assert!(
        message.contains("OtherServices.xml:2 names name and does not resolve"),
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
    <field name="partId" type="id" is-pk="true"/><field name="thingId" type="id"/>
    <relationship type="one" related="up.Thing"><key-map field-name="thingId" related="thingId"/></relationship>
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
        message.contains("has no related, so it joins to the field by its own name thingId"),
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
        message.contains("label is written where the rename does not know what it names"),
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
    let (line, character) = position(UP_ENTITIES, "name=\"name\"", 7);
    let prefixed = viewed.rename(&entities, line, character, "title").unwrap_err();
    assert!(
        prefixed.contains("aliases it as thingName by a prefixed alias-all"),
        "{prefixed}"
    );
}

/// An entity field renamed at its definition, in a file of CRLF lines with characters of several bytes before the
/// name: the definition, the parameter its service's auto-parameters makes and the field-map naming it, the form field
/// bound to that parameter with its field-ref, and an exclude that leaves it out of another form; the alias-all's alias
/// of a view follows with no text of its own.
#[test]
fn a_rename_goes_through_alias_all_auto_parameters_and_a_form_binding() {
    let entities = "<entities>\r\n    <entity entity-name=\"Thing\" package=\"up\">\r\n        <field name=\"thingId\" type=\"id\" is-pk=\"true\"/>\r\n        <field description=\"\u{dc}n\u{ef}code \u{2713}\" name=\"name\" type=\"text-medium\"/>\r\n    </entity>\r\n    <view-entity entity-name=\"ThingView\" package=\"up\"><member-entity entity-alias=\"T\" entity-name=\"up.Thing\"/><alias-all entity-alias=\"T\"/></view-entity>\r\n</entities>\r\n";
    let services = r#"<services>
    <service verb="save" noun="Item">
        <in-parameters><auto-parameters entity-name="up.Thing"/></in-parameters>
    </service>
    <service verb="make" noun="Item">
        <actions><service-call name="site.ThingServices.save#Item"><field-map field-name="name" value="x"/></service-call></actions>
    </service>
</services>
"#;
    let screen = r#"<screen>
    <transition name="save"><service-call name="site.ThingServices.save#Item"/><default-response url="."/></transition>
    <widgets>
        <form-single name="Edit" transition="save">
            <field name="name"><default-field><text-line/></default-field></field>
            <field-layout><field-ref name="name"/></field-layout>
        </form-single>
        <form-list name="List" list="things"><auto-fields-entity entity-name="up.Thing"><exclude field-name="name"/></auto-fields-entity></form-list>
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
                vec![place(services, "field-name=\"name\"", 12)]
            ),
            (
                "Things.xml".to_string(),
                vec![
                    place(screen, "<field name=\"name\"", 13),
                    place(screen, "<field-ref name=\"name\"", 17),
                    place(screen, "<exclude field-name=\"name\"", 21),
                ]
            ),
            ("UpEntities.xml".to_string(), vec![title(3, 45)]),
        ]
    );
}
