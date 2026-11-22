//! A presenter's screen location, resolved against installed application screens on the
//! request worker. Followers fetch it with their own browser session and record permissions.
//! The page's own rules (WebrootCall.js `followPath`) match these: both are checked against the
//! vectors in webroot's test/client/pages/follow-vectors.json.
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

/// The record parameters of a presenter's screen that followers get: those the screen at `path`
/// (or one above it) takes, or one of its read-only transitions does (Chat's `room` takes the
/// room, kept off the screen so its links do not carry it), under the rules of [`record_param`];
/// `None` when `path` is no plain application screen. This loads the screen definitions, so the
/// socket keeps its last answer.
pub(crate) fn resolve(factory: &ExecutionContextFactory, path: &str, given: &Json) -> Option<Json> {
    let target = screen_path(path)?;
    let info = ScreenUrlInfo::parse(factory, &info::default_root(factory), &target, 0).ok()?;
    if info.target_transition_name.is_some() || info.file_resource.is_some() || !info.extra_path.is_empty() {
        return None;
    }
    let mut params = serde_json::Map::new();
    if let Some(fields) = given.as_object() {
        if fields.len() > 32 {
            return None;
        }
        for (key, value) in fields {
            if record_param(key)
                && (info.parameter_items.contains_key(key)
                    || info.target_screen.parameters.iter().any(|p| p.name == *key)
                    || info
                        .target_screen
                        .transitions
                        .iter()
                        .any(|t| t.read_only && t.parameters.iter().any(|p| p.name == *key)))
            {
                if let Some(text) = value.as_str().filter(|v| identifier(v)) {
                    params.insert(key.clone(), json!(text));
                } else if let Some(n) = value.as_i64() {
                    params.insert(key.clone(), json!(n));
                }
            }
        }
    }
    Some(Json::Object(params))
}

/// The follow frame of a screen already resolved: its path and parameters, with the place on it
/// that the presenter's frame names (its section, open dialog and pointer), each checked here. A
/// `locate` frame changes only these, so it needs no resolution.
pub(crate) fn located(path: &str, params: &Json, frame: &Json) -> Json {
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
    out
}

#[cfg(test)]
#[path = "follow_tests.rs"]
mod tests;
