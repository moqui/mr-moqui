//! Monitor › Settings (plans/archive/Monitor-Settings.md): the services over [`moqui_core::live_settings`]. Listing needs
//! Monitor access (the MONITOR_APP artifact group); setting, extending and reverting need `MONITOR_SETTINGS`. Each
//! change is logged on the `moqui.monitor.audit` target by live_settings, and here written as a `config` annotation,
//! tagged `runtime-override` and the key, from this instance only: a set is a band to its expiry, an extend or an early
//! revert a point. A failed annotation is logged and does not undo the change.

use super::{fail, input, output, Error, ExecutionContext, Parameters, ServiceInfo, Value};
use moqui_core::live_settings::{self, catalog, Change, Override};
use serde_json::{json, Value as Json};

const PERMISSION: &str = "MONITOR_SETTINGS";

pub fn register(registry: &mut moqui_core::native::NativeRegistry) -> Result<(), moqui_core::context::Error> {
    for (name, function) in [
        ("list_runtime_settings", list as moqui_core::native::NativeService),
        ("set_runtime_setting", set),
        ("revert_runtime_setting", revert),
        ("revert_all_runtime_settings", revert_all),
    ] {
        registry.service_function(&format!("moqui_core::monitor::{name}"), function)?;
    }
    Ok(())
}

fn signed_in() -> Result<String, Error> {
    moqui_core::user::current_user_id()
        .map_err(fail)?
        .ok_or_else(|| fail("sign in to see the runtime settings"))
}

fn require() -> Result<String, Error> {
    let user = signed_in()?;
    if moqui_core::user::has_permission(PERMISSION).map_err(fail)? {
        Ok(user)
    } else {
        Err(fail(format!(
            "{PERMISSION} permission is required to change runtime settings"
        )))
    }
}

/// `list#RuntimeSettings`: every setting with its on-disk value and source, its value now and its override, the active
/// overrides, and this instance and its cluster.
fn list(ec: &ExecutionContext, _: &ServiceInfo<'_>, _: &Parameters) -> Result<Parameters, Error> {
    signed_in()?;
    let factory = ec.factory();
    let settings: Vec<Json> = live_settings::list(factory)
        .iter()
        .map(|setting| setting.to_json())
        .collect();
    let overrides: Vec<Json> = settings
        .iter()
        .filter(|setting| !setting["override"].is_null())
        .cloned()
        .collect();
    let (instance_id, members) = live_settings::instance(factory);
    Ok(output(json!({
        "settings": settings,
        "overrides": overrides,
        "instanceId": instance_id,
        "memberCount": members,
        "durations": live_settings::DURATIONS,
    })))
}

/// `set#RuntimeSetting`: set or extend the override of `key`, or of the target rule of `target`.
fn set(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = require()?;
    let options = input(p);
    let target = options["target"].as_str().map(str::trim).unwrap_or("");
    let key = match options["key"].as_str().map(str::trim) {
        Some(key) if !key.is_empty() => key.to_string(),
        _ if !target.is_empty() => catalog::target_key(target),
        _ => return Err(fail("a key or a target is required")),
    };
    let minutes = match p.get("minutes").map(moqui_lib::object::to_plain_string) {
        Some(text) => text
            .trim()
            .parse::<i64>()
            .map_err(|_| fail(format!("[{text}] is not a number of minutes")))?,
        None => 60,
    };
    let change = Change {
        key: &key,
        value: options["value"].as_str().unwrap_or(""),
        minutes,
        reason: options["reason"].as_str().unwrap_or(""),
        user: &user,
    };
    let applied = live_settings::set(ec.factory(), &change).map_err(fail)?;
    let made = &applied.made;
    let text = format!(
        "{} {} {} until {} (on disk: {}): {}",
        made.key,
        if applied.extended { "extended at" } else { "set to" },
        made.value,
        stamp(made.expires_at),
        shown(&applied.on_disk),
        made.reason
    );
    let band = (!applied.extended).then_some(made.expires_at);
    annotate(ec, &made.key, &text, made.set_at, band);
    Ok(output(
        json!({"override": made.to_json(), "onDisk": applied.on_disk, "extended": applied.extended}),
    ))
}

/// `revert#RuntimeSetting`: back to the on-disk value.
fn revert(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = require()?;
    let key = p.get("key").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let ended = live_settings::revert(ec.factory(), &key, &user).map_err(fail)?;
    if let Some(ended) = &ended {
        annotate_revert(ec, ended, &user);
    }
    Ok(output(json!({"reverted": ended.is_some()})))
}

/// `revert#AllRuntimeSettings`.
fn revert_all(ec: &ExecutionContext, _: &ServiceInfo<'_>, _: &Parameters) -> Result<Parameters, Error> {
    let user = require()?;
    let ended = live_settings::revert_all(ec.factory(), &user);
    for ended in &ended {
        annotate_revert(ec, ended, &user);
    }
    Ok(output(json!({"reverted": ended.len()})))
}

fn annotate_revert(ec: &ExecutionContext, ended: &Override, user: &str) {
    let now = moqui_core::live_settings::now_millis();
    let text = format!("{} reverted to on-disk by {user} (was {})", ended.key, ended.value);
    annotate(ec, &ended.key, &text, now, None);
}

fn shown(value: &str) -> &str {
    if value.is_empty() {
        "none"
    } else {
        value
    }
}

fn stamp(millis: i64) -> String {
    jiff::Timestamp::from_millisecond(millis).map_or_else(|_| millis.to_string(), |at| at.to_string())
}

/// A `config` annotation of a change, a band to `end` when given. Without search (a test), or when it cannot be
/// written, the change stands and the failure is logged.
fn annotate(ec: &ExecutionContext, key: &str, text: &str, time: i64, end: Option<i64>) {
    let Some(search) = moqui_core::search::installed(ec.factory()) else {
        return;
    };
    let result = (|| -> Result<(), Error> {
        let id = format!("marker-{}", moqui_core::user::random_string(24).map_err(fail)?);
        let tag: String = key.chars().take(128).collect();
        let resource = moqui_core::telemetry::for_factory(ec.factory()).resource();
        let mut annotation = json!({
            "_id": id,
            "time": jiff::Timestamp::from_millisecond(time).map_err(fail)?.to_string(),
            "source": "config",
            "scope": "service",
            "service.name": resource.service_name,
            "text": moqui_core::telemetry::redact_text(text),
            "tags": ["runtime-override", tag],
            "created_by": moqui_core::user::current_user_id().map_err(fail)?,
        });
        if let Some(end) = end {
            annotation["time_end"] = json!(jiff::Timestamp::from_millisecond(end).map_err(fail)?.to_string());
        }
        let Value::Map(document) = moqui_lib::json::from_json(&annotation) else {
            return Err(fail("annotation is not a map"));
        };
        if search.index_entries("monitor_annotations", vec![document])? != 1 {
            return Err(fail("annotation could not be indexed"));
        }
        Ok(())
    })();
    if let Err(error) = result {
        tracing::error!(key, %error, "runtime setting annotation was not written");
    }
}
