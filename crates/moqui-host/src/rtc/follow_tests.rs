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

use moqui_core::context::ExecutionContextFactory;
use serde_json::{json, Value as Json};

use super::{identifier, located, record_param, resolve, screen_path};

/// A presenter's `navigate` frame as followers get it, as the socket makes it: [`resolve`], then [`located`].
fn navigate(factory: &ExecutionContextFactory, frame: &Json) -> Option<Json> {
    let path = frame["path"].as_str()?;
    let params = resolve(factory, path, &frame["params"])?;
    Some(located(path, &params, frame))
}

#[test]
fn accepts_application_paths_and_record_keys_only() {
    assert!(screen_path("/qapps/BasicComms/Calls/History").is_some());
    for path in [
        "/qapps/../login",
        "//other/qapps/Foo",
        "/apps/BasicComms/Call?token=a",
        "/qapps/A/%2e",
    ] {
        assert!(screen_path(path).is_none());
    }
    assert!(record_param("wikiPageId"));
    assert!(record_param("callSessionId"));
    for key in [
        "apiKeyId",
        "sessionId",
        "tokenName",
        "password",
        "description",
        "credentialId",
    ] {
        assert!(!record_param(key));
    }
}

/// The page's rules (WebrootCall.js `followPath`, checked in the WebrootCall client page test)
/// and these are one rule: both read the same vectors.
#[test]
fn the_shared_follow_vectors_hold_here() {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/components/base/webroot/test/client/pages/follow-vectors.json");
    let vectors: Json = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let cases = |list: &str, field: &str| -> Vec<(String, bool)> {
        vectors[list]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| (v[field].as_str().unwrap().to_string(), v["ok"].as_bool().unwrap()))
            .collect()
    };
    for (path, ok) in cases("paths", "path") {
        assert_eq!(screen_path(&path).is_some(), ok, "path {path}");
    }
    for (name, ok) in cases("names", "name") {
        assert_eq!(record_param(&name), ok, "parameter {name}");
    }
    for (value, ok) in cases("values", "value") {
        assert_eq!(identifier(&value), ok, "value {value:?}");
    }
}

/// A `locate` keeps the screen resolved last and checks only the place on it.
#[test]
fn a_located_frame_keeps_only_a_checked_section_dialog_and_pointer() {
    let params = json!({"wikiPageId":"100123"});
    let out = located(
        "/qapps/BasicComms/Boards/Board",
        &params,
        &json!({"path":"/qapps/Other","params":{"junkId":"x"},"section":"Notes","dialog":"bad dialog",
            "pointer":{"id":"Notes","x":0.25,"y":1.5}}),
    );
    assert_eq!(
        out,
        json!({"rtc":"follow","action":"navigate","path":"/qapps/BasicComms/Boards/Board",
            "params":{"wikiPageId":"100123"},"section":"Notes"})
    );
    let pointer = located(
        "/qapps/A",
        &json!({}),
        &json!({"dialog":"EditDialog","pointer":{"id":"Row-1","x":0.5,"y":0}}),
    );
    assert_eq!(pointer["dialog"], "EditDialog");
    assert_eq!(pointer["pointer"], json!({"id":"Row-1","x":0.5,"y":0.0}));
    assert!(pointer.get("section").is_none());
}

#[test]
fn the_board_record_id_survives_but_its_write_transition_cannot_be_presented() {
    let framework = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/components/base/framework");
    let conf = std::fs::read_to_string(framework.parent().unwrap().join("basic-comms/MoquiConf.xml")).unwrap();
    let components = ["webroot", "basic-comms", "tools"]
        .iter()
        .map(|name| moqui_core::context::ComponentInfo {
            name: name.to_string(),
            version: String::new(),
            location: framework.parent().unwrap().join(name),
            depends_on: Vec::new(),
            purposes: Vec::new(),
        })
        .collect();
    let ctx = ExecutionContextFactory::test_context_with_components(
        &conf,
        moqui_core::service::ServicePools::idle(),
        framework,
        components,
    );
    let _entered = ctx.enter();
    let valid = navigate(
        ctx.factory(),
        &json!({"path":"/qapps/BasicComms/Boards/Board",
        "params":{"wikiPageId":"100123","tokenName":"hidden","junkId":"ignored"}}),
    )
    .unwrap();
    assert_eq!(valid["params"], json!({"wikiPageId":"100123"}));
    // Chat's room is declared by its room transition, not the screen
    let room = navigate(
        ctx.factory(),
        &json!({"path":"/qapps/BasicComms/Chat","params":{"chatRoomId":"R1","junkId":"ignored"}}),
    )
    .unwrap();
    assert_eq!(room["params"], json!({"chatRoomId":"R1"}));
    assert!(navigate(
        ctx.factory(),
        &json!({"path":"/apps/BasicComms/Boards/Board/saveBoard"})
    )
    .is_none());
}
