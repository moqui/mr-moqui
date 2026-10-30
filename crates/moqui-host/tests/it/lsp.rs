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

//! `moqui lsp` (plans/design-dev-aide.md P9, 13.6) on a model with a site component: an open file's diagnostics as it
//! changes, go to definition of an entity name, its references across the files (the open buffer's among them),
//! completion of entity names in a value and of a form's attributes, hover, and the document's and the workspace's
//! symbols. The server is driven as a client drives it, by its requests and notifications.

#![cfg(all(
    feature = "embedded-postgres",
    feature = "axum",
    feature = "devtools",
    feature = "tui"
))]

use std::sync::Arc;

use lsp_server::{Notification, Request, RequestId};
use moqui_devkit::index::MemoryIndex;
use moqui_host::lsp::{Backend, Server};
use serde_json::{json, Value as Json};

use crate::common::start_with_files;

const ENTITIES: &str = r#"<entities>
    <entity entity-name="Widget" package="mysite">
        <field name="widgetId" type="id" is-pk="true"/>
        <field name="partId" type="id"/>
    </entity>
</entities>
"#;
const SERVICES: &str = r#"<services>
    <service verb="get" noun="Widget">
        <actions>
            <entity-find-one entity-name="mysite.Widget" value-field="widget"/>
        </actions>
    </service>
</services>
"#;

fn request(server: &mut Server, id: i32, method: &str, params: Json) -> Json {
    let response = server.answer(&Request::new(RequestId::from(id), method.to_string(), params));
    assert!(response.error.is_none(), "{method}: {:?}", response.error);
    response.result.unwrap_or(Json::Null)
}

#[test]
fn the_language_server_answers_on_the_example_component() {
    if !crate::common::own_process() {
        return;
    }
    let files = [
        ("component/my-site/component.xml", r#"<component name="my-site"/>"#),
        ("component/my-site/entity/MySiteEntities.xml", ENTITIES),
        ("component/my-site/service/mysite/Widgets.xml", SERVICES),
    ];
    let gate = start_with_files("lsp", None, &[("instance_purpose", "dev")], &files);
    let factory = Arc::clone(gate.server.factory());
    let index = MemoryIndex::build(&factory);
    let mut server = Server::new(Backend::Local { factory, index });
    let site = std::fs::canonicalize(gate.runtime.join("component/my-site")).unwrap();
    let services = site.join("service/mysite/Widgets.xml");
    let uri = moqui_host::lsp::uri_of(&services);

    // opened with an unknown entity: an error diagnostic on it, naming its rule
    let broken = SERVICES.replace("mysite.Widget\"", "mysite.NoSuch\"");
    let published = server.notify(&Notification::new(
        "textDocument/didOpen".into(),
        json!({"textDocument": {"uri": uri, "languageId": "xml", "version": 1, "text": broken}}),
    ));
    let diagnostics = &published[0].params["diagnostics"];
    assert!(
        diagnostics
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "reference.entity" && d["range"]["start"]["line"] == 3),
        "{diagnostics}"
    );
    // fixed: no error left
    let published = server.notify(&Notification::new(
        "textDocument/didChange".into(),
        json!({"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"text": SERVICES}]}),
    ));
    assert!(
        published[0].params["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["severity"] != 1),
        "{}",
        published[0].params
    );

    // go to definition of the entity name, and its references (the open file among them)
    let at = json!({"textDocument": {"uri": uri}, "position": {"line": 3, "character": 46}});
    let definition = request(&mut server, 1, "textDocument/definition", at.clone());
    assert!(
        definition["uri"]
            .as_str()
            .unwrap()
            .ends_with("entity/MySiteEntities.xml"),
        "{definition}"
    );
    assert_eq!(definition["range"]["start"]["line"], 1);
    let mut with_declaration = at.clone();
    with_declaration["context"] = json!({"includeDeclaration": true});
    let references = request(&mut server, 2, "textDocument/references", with_declaration);
    let uris: Vec<&str> = references
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["uri"].as_str())
        .collect();
    assert!(
        uris.iter().any(|u| u.ends_with("Widgets.xml")) && uris.iter().any(|u| u.ends_with("MySiteEntities.xml")),
        "{references}"
    );

    // hover: the artifact's kind and name
    let hover = request(&mut server, 3, "textDocument/hover", at);
    assert!(
        hover["contents"]["value"].as_str().unwrap().contains("mysite.Widget"),
        "{hover}"
    );

    // completion: entity names in an entity-name value, a service's attributes after its name
    let typing = SERVICES.replace("entity-name=\"mysite.Widget\"", "entity-name=\"mysite.Wid");
    server.notify(&Notification::new(
        "textDocument/didChange".into(),
        json!({"textDocument": {"uri": uri, "version": 3}, "contentChanges": [{"text": typing}]}),
    ));
    let names = request(
        &mut server,
        4,
        "textDocument/completion",
        json!({"textDocument": {"uri": uri}, "position": {"line": 3, "character": 47}}),
    );
    assert!(
        names
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "mysite.Widget"),
        "{names}"
    );
    let attributes = request(
        &mut server,
        5,
        "textDocument/completion",
        json!({"textDocument": {"uri": uri}, "position": {"line": 1, "character": 13}}),
    );
    assert!(
        attributes
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "noun"),
        "{attributes}"
    );

    // symbols: the document's elements, the workspace's artifacts by name
    server.notify(&Notification::new(
        "textDocument/didChange".into(),
        json!({"textDocument": {"uri": uri, "version": 4}, "contentChanges": [{"text": SERVICES}]}),
    ));
    let symbols = request(
        &mut server,
        6,
        "textDocument/documentSymbol",
        json!({"textDocument": {"uri": uri}}),
    );
    assert_eq!(symbols[0]["children"][0]["name"], "service get#Widget", "{symbols}");
    let workspace = request(&mut server, 7, "workspace/symbol", json!({"query": "Widget"}));
    assert!(
        workspace
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "mysite.Widget"),
        "{workspace}"
    );
    let unknown = server.answer(&Request::new(
        RequestId::from(8),
        "textDocument/rename".into(),
        json!({}),
    ));
    assert!(unknown.error.is_some());
}
