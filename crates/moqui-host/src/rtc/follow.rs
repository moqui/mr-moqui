//! A presenter's screen location, resolved against installed application screens on the
//! request worker. Followers fetch it with their own browser session and record permissions.
use moqui_core::context::ExecutionContextFactory;
use moqui_core::screen::{info, ScreenUrlInfo};
use serde_json::{json, Value as Json};

fn identifier(text: &str) -> bool {
    !text.is_empty() && text.len() <= 100 && text.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-:.".contains(&c))
}

fn record_param(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    identifier(name)
        && (name == "id" || name.ends_with("Id") || name.ends_with("Name"))
        && (!lower.contains("session") || name == "callSessionId")
        && ![
            "token",
            "password",
            "secret",
            "credential",
            "apikey",
            "accesskey",
            "privatekey",
            "signingkey",
            "authorization",
            "csrf",
            "cookie",
        ]
        .iter()
        .any(|part| lower.contains(part))
}

/// Return the path to resolve under the actual applications root. Reject absolute URLs,
/// transitions, encoded path separators, path parameters and inline query strings.
fn screen_path(path: &str) -> Option<String> {
    let app = path.strip_prefix("/qapps/").or_else(|| path.strip_prefix("/apps/"))?;
    if app.len() > 1024
        || !app.split('/').all(|s| {
            !s.is_empty() && s.len() <= 100 && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        })
    {
        return None;
    }
    Some(format!("/apps/{app}"))
}

pub(crate) fn navigate(factory: &ExecutionContextFactory, frame: &Json) -> Option<Json> {
    let path = frame["path"].as_str()?;
    let target = screen_path(path)?;
    let info = ScreenUrlInfo::parse(factory, &info::default_root(factory), &target, 0).ok()?;
    if info.target_transition_name.is_some() || info.file_resource.is_some() || !info.extra_path.is_empty() {
        return None;
    }
    let mut params = serde_json::Map::new();
    if let Some(fields) = frame["params"].as_object() {
        if fields.len() > 32 {
            return None;
        }
        for (key, value) in fields {
            if record_param(key)
                && (info.parameter_items.contains_key(key)
                    || info.target_screen.parameters.iter().any(|p| p.name == *key))
            {
                if let Some(text) = value.as_str().filter(|v| identifier(v)) {
                    params.insert(key.clone(), json!(text));
                } else if let Some(n) = value.as_i64() {
                    params.insert(key.clone(), json!(n));
                }
            }
        }
    }
    let mut out = json!({"rtc":"follow","action":"navigate","path":path,"params":params});
    for key in ["section", "dialog"] {
        if let Some(value) = frame[key].as_str().filter(|x| identifier(x)) {
            out[key] = json!(value);
        }
    }
    let p = &frame["pointer"];
    if let (Some(id), Some(x), Some(y)) = (
        p["id"].as_str().filter(|x| identifier(x)),
        p["x"].as_f64(),
        p["y"].as_f64(),
    ) {
        if (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y) {
            out["pointer"] = json!({"id":id,"x":x,"y":y});
        }
    }
    Some(out)
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
    assert!(navigate(
        ctx.factory(),
        &json!({"path":"/apps/BasicComms/Boards/Board/saveBoard"})
    )
    .is_none());
}
