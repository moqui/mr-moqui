//! Source-scoped ingestion credentials and operator status. Only hashes are persisted.
use base64::Engine;
use moqui_core::{
    context::ExecutionContext,
    native::{NativeRegistry, Parameters, ServiceInfo},
};
use moqui_lib::{error::Error, object::Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
const KEYS: &str = "moqui.monitor.IngestKey";
fn text<'a>(p: &'a Parameters, key: &str) -> &'a str {
    p.get(key).and_then(Value::as_str).unwrap_or("")
}
fn entity(ec: &ExecutionContext) -> Result<moqui_core::entity::EntityFacade, Error> {
    ec.factory()
        .entity()
        .ok_or_else(|| Error::new("entity facade is unavailable"))
}
fn owner() -> Result<String, Error> {
    moqui_core::user::current_user_id()?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::new("monitor keys require an authenticated user"))
}
fn require_ingest() -> Result<(), Error> {
    if moqui_core::user::has_permission("MONITOR_INGEST").map_err(auth_unavailable)? {
        Ok(())
    } else {
        Err(Error::new("MONITOR_INGEST permission is required"))
    }
}

pub fn register(registry: &mut NativeRegistry) -> Result<(), moqui_core::context::Error> {
    for (name, function) in [
        ("source_status", source_status as moqui_core::native::NativeService),
        ("list_keys", list_keys),
        ("create_key", create_key),
        ("revoke_key", revoke_key),
    ] {
        registry.service_function(&format!("moqui_core::monitor::{name}"), function)?;
    }
    Ok(())
}
fn list_keys(ec: &ExecutionContext, _: &ServiceInfo<'_>, _: &Parameters) -> Result<Parameters, Error> {
    let user = owner()?;
    let _authz = ec.disable_authz();
    let rows = entity(ec)?.find(KEYS)?.eq("userId", Value::string(user)).list()?;
    let keys = rows
        .iter()
        .map(|row| {
            Value::Map(
                [
                    "keyId",
                    "source",
                    "userId",
                    "createdDate",
                    "expiresDate",
                    "lastSeen",
                    "revoked",
                ]
                .into_iter()
                .filter_map(|key| row.get(key).map(|v| (key.into(), v.clone())))
                .collect(),
            )
        })
        .collect();
    Ok(BTreeMap::from([("keys".into(), Value::List(keys))]))
}
fn create_key(ec: &ExecutionContext, _: &ServiceInfo<'_>, parameters: &Parameters) -> Result<Parameters, Error> {
    require_ingest()?;
    let user = owner()?;
    let source = text(parameters, "source").trim();
    if source.is_empty()
        || source.len() > 128
        || !source
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(&c))
    {
        return Err(Error::new("source must contain 1–128 letters, digits, _, ., : or -"));
    }
    let days = parameters
        .get("expireDays")
        .map(moqui_lib::object::to_plain_string)
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0 && *v <= 3650.0)
        .unwrap_or(90.0);
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(|error| Error::new(format!("could not generate an ingest key: {error}")))?;
    let access_token = format!(
        "mqi_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret)
    );
    let key_id = key_hash(&access_token);
    let _authz = ec.disable_authz();
    let mut row = entity(ec)?.make_value(KEYS)?;
    for (key, value) in [
        ("keyId", Value::string(&key_id)),
        ("source", Value::string(source)),
        ("userId", Value::string(user)),
        ("createdDate", Value::I64(moqui_core::user::now_millis())),
        (
            "expiresDate",
            Value::I64(moqui_core::user::now_millis().saturating_add((days * 86_400_000.0) as i64)),
        ),
        ("revoked", Value::string("N")),
    ] {
        row.set(key, value);
    }
    row.create(&entity(ec)?)?;
    Ok(BTreeMap::from([
        ("keyId".into(), Value::string(key_id)),
        ("accessToken".into(), Value::string(access_token)),
    ]))
}
fn revoke_key(ec: &ExecutionContext, _: &ServiceInfo<'_>, parameters: &Parameters) -> Result<Parameters, Error> {
    let user = owner()?;
    let key = text(parameters, "keyId");
    let _authz = ec.disable_authz();
    let mut row = entity(ec)?
        .find(KEYS)?
        .use_cache(false)
        .eq("keyId", Value::string(key))
        .one()?
        .filter(|row| row.get("userId").and_then(Value::as_str) == Some(user.as_str()))
        .ok_or_else(|| Error::new("ingest key not found"))?;
    row.set("revoked", Value::string("Y"));
    row.update(&entity(ec)?)?;
    entity(ec)?
        .find("moqui.security.UserLoginKey")?
        .eq("loginKey", Value::string(key))
        .delete_all()?;
    Ok(Parameters::new())
}
fn source_status(ec: &ExecutionContext, _: &ServiceInfo<'_>, _: &Parameters) -> Result<Parameters, Error> {
    moqui_core::user::authorize_own("AT_SERVICE", "org.moqui.report.dataset.otel_spans", "view")?;
    let counters = moqui_core::telemetry::for_factory(ec.factory()).counters();
    let mut out = BTreeMap::new();
    out.insert(
        "counters".into(),
        Value::Map(indexmap::IndexMap::from([
            ("queued".into(), Value::I64(counters.queued as i64)),
            ("exported".into(), Value::I64(counters.exported as i64)),
            ("dropped".into(), Value::I64(counters.dropped as i64)),
            ("failed".into(), Value::I64(counters.failed as i64)),
            (
                "remoteExported".into(),
                Value::I64(counters.remote_exported.min(i64::MAX as u64) as i64),
            ),
            (
                "remoteDropped".into(),
                Value::I64(counters.remote_dropped.min(i64::MAX as u64) as i64),
            ),
            (
                "remoteFailed".into(),
                Value::I64(counters.remote_failed.min(i64::MAX as u64) as i64),
            ),
        ])),
    );
    let resource = moqui_core::telemetry::for_factory(ec.factory());
    let mut sources = vec![Value::Map(indexmap::IndexMap::from([
        ("service".into(), Value::string(&resource.resource().service_name)),
        ("instance".into(), Value::string(&resource.resource().instance_id)),
        (
            "status".into(),
            Value::string(if !resource.config().enabled {
                "disabled; health unknown"
            } else if moqui_core::search::installed(ec.factory()).is_none() {
                "search unavailable; health unknown"
            } else if counters.failed > 0 || counters.dropped > 0 {
                "telemetry loss; health unknown"
            } else if counters.exported == 0 {
                "awaiting successful export; health unknown"
            } else {
                "local exporter ready; health unknown"
            }),
        ),
    ]))];
    if !ec.factory().conf_setting("telemetry", "otlp-endpoint").is_empty() {
        sources.push(Value::Map(indexmap::IndexMap::from([
            ("service".into(), Value::string("external collector")),
            ("instance".into(), Value::Null),
            (
                "status".into(),
                Value::string(if counters.remote_failed > 0 || counters.remote_dropped > 0 {
                    "external export loss; health unknown"
                } else if counters.remote_exported == 0 {
                    "awaiting external export; health unknown"
                } else {
                    "external exporter ready; health unknown"
                }),
            ),
        ])));
    }
    let _authz = ec.disable_authz();
    let rows = entity(ec)?.find(KEYS)?.eq("userId", Value::string(owner()?)).list()?;
    for row in rows.iter() {
        sources.push(Value::Map(indexmap::IndexMap::from([
            ("service".into(), row.get("source").cloned().unwrap_or(Value::Null)),
            ("instance".into(), Value::Null),
            ("lastSeen".into(), row.get("lastSeen").cloned().unwrap_or(Value::Null)),
            (
                "status".into(),
                Value::string(if row.get("revoked").and_then(Value::as_str) == Some("Y") {
                    "revoked"
                } else {
                    "unknown health"
                }),
            ),
        ])));
    }
    out.insert("sources".into(), Value::List(sources));
    Ok(out)
}
/// Called in a fresh entered context, before decoding any untrusted records.
pub fn authenticate(ec: &ExecutionContext, token: &str) -> Result<(String, String), Error> {
    if !token.starts_with("mqi_") || token.len() != 47 {
        return Err(Error::new("ingest key is invalid or revoked"));
    }
    let key = key_hash(token);
    let _authz = ec.disable_authz();
    let facade = entity(ec).map_err(auth_unavailable)?;
    let row = facade
        .find(KEYS)
        .map_err(auth_unavailable)?
        .use_cache(false)
        .eq("keyId", Value::string(&key))
        .one()
        .map_err(auth_unavailable)?
        // EntityFind::one narrows full-primary-key finds, so security predicates are
        // checked on the loaded row rather than supplied as extra equality conditions.
        .filter(|row| row.get("revoked").and_then(Value::as_str) == Some("N"))
        .ok_or_else(|| Error::new("ingest key is invalid or revoked"))?;
    if row
        .get("expiresDate")
        .and_then(moqui_core::user::value_millis)
        .is_none_or(|expires| expires <= moqui_core::user::now_millis())
    {
        return Err(Error::new("ingest key is expired"));
    }
    let account = facade
        .find("moqui.security.UserAccount")
        .map_err(auth_unavailable)?
        .use_cache(false)
        .eq("userId", row.get("userId").cloned().unwrap_or(Value::Null))
        .one()
        .map_err(auth_unavailable)?
        .ok_or_else(|| Error::new("ingest key owner is unavailable"))?;
    let username = account
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let source = row
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new("ingest source is missing"))?
        .to_owned();
    drop(_authz);
    moqui_core::user::internal_login_user(&username)?;
    require_ingest()?;
    Ok((key, source))
}

fn key_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn auth_unavailable(error: impl std::error::Error + Send + Sync + 'static) -> Error {
    moqui_core::context::Error::of_kind(
        moqui_core::context::ErrorKind::Unavailable,
        "ingest authorization storage is unavailable",
    )
    .with_source(error)
    .into()
}
