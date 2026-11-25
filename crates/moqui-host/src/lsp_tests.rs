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
        let dir = std::env::temp_dir().join(format!("moqui-lsp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in [
            ("up/entity/UpEntities.xml", UP_ENTITIES),
            ("site/service/site/ThingServices.xml", SITE_SERVICES),
        ] {
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
    let mut bed = LspBed::new("rename", "");
    let uri = bed.services_uri();
    // line 8: the service-call of save#Thing
    let prepared = bed
        .ask(
            "textDocument/prepareRename",
            json!({ "textDocument": { "uri": uri }, "position": { "line": 8, "character": 40 } }),
        )
        .unwrap();
    assert_eq!(prepared["placeholder"], "site.ThingServices.save#Thing");
    let service = bed.rename(&uri, 8, 40, "store#Thing").unwrap();
    assert_eq!(
        service,
        [(
            "ThingServices.xml".to_string(),
            vec![
                (1, 19, "store".to_string()),
                (8, 37, "site.ThingServices.store#Thing".to_string())
            ]
        )]
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
