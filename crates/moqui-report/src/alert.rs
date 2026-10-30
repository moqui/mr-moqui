//! Deterministic per-series alert state machine. Missing and failed data never establish health.
use super::{fail, input, output, text, ExecutionContext, Parameters, ServiceInfo};
use moqui_lib::{error::Error, object::Value};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    #[default]
    Normal,
    Pending,
    Firing,
    Recovering,
    NoData,
    Error,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Instance {
    pub state: State,
    pub since: i64,
    pub last_evaluated: i64,
    pub previously_firing: bool,
}
#[derive(Clone, Copy, Debug)]
pub enum Observation {
    Healthy,
    Breaching,
    NoData,
    Error,
}
impl Instance {
    /// Timers use the evaluation clock. Backward/duplicate samples cannot advance them.
    pub fn evaluate(&mut self, observation: Observation, now: i64, pending_ms: i64, keep_firing_ms: i64) -> bool {
        if now < self.last_evaluated {
            return false;
        }
        self.last_evaluated = now;
        let old = self.state;
        let next = match observation {
            Observation::NoData => State::NoData,
            Observation::Error => State::Error,
            Observation::Breaching => match self.state {
                State::Firing | State::Recovering => State::Firing,
                State::Pending if now.saturating_sub(self.since) >= pending_ms => State::Firing,
                State::NoData | State::Error if self.previously_firing => State::Firing,
                _ if pending_ms <= 0 => State::Firing,
                _ => State::Pending,
            },
            Observation::Healthy => match self.state {
                State::Firing => {
                    if keep_firing_ms <= 0 {
                        State::Normal
                    } else {
                        State::Recovering
                    }
                }
                State::Recovering if now.saturating_sub(self.since) < keep_firing_ms => State::Recovering,
                State::NoData | State::Error if self.previously_firing && keep_firing_ms > 0 => State::Recovering,
                _ => State::Normal,
            },
        };
        if matches!(next, State::Firing | State::Recovering) {
            self.previously_firing = true;
        } else if next == State::Normal {
            self.previously_firing = false;
        }
        if old != next {
            self.state = next;
            self.since = now;
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Condition {
    Above(f64),
    Below(f64),
    Within(f64, f64),
    Outside(f64, f64),
}
impl Condition {
    pub fn observe(self, value: Option<f64>) -> Observation {
        let Some(v) = value.filter(|v| v.is_finite()) else {
            return Observation::NoData;
        };
        let breach = match self {
            Self::Above(limit) => v > limit,
            Self::Below(limit) => v < limit,
            Self::Within(lo, hi) => (lo..=hi).contains(&v),
            Self::Outside(lo, hi) => !(lo..=hi).contains(&v),
        };
        if breach {
            Observation::Breaching
        } else {
            Observation::Healthy
        }
    }
}

fn stamp(millis: i64) -> Value {
    Value::Timestamp(jiff::Timestamp::from_millisecond(millis).unwrap())
}
fn millis(value: Option<&Value>) -> i64 {
    value
        .and_then(|v| match v {
            Value::Timestamp(t) => Some(t.as_millisecond()),
            _ => moqui_lib::object::to_plain_string(v)
                .parse::<jiff::Timestamp>()
                .ok()
                .map(|t| t.as_millisecond()),
        })
        .unwrap_or(0)
}

fn state_from_row(row: &moqui_core::entity::EntityValue) -> Instance {
    Instance {
        state: row
            .get("state")
            .and_then(Value::as_str)
            .and_then(|s| serde_json::from_value(json!(s)).ok())
            .unwrap_or(State::Normal),
        since: millis(row.get("stateSince")),
        last_evaluated: millis(row.get("lastEvaluated")),
        previously_firing: row.get("previouslyFiring").and_then(Value::as_str) == Some("Y"),
    }
}

fn label_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        && ![
            "body",
            "message",
            "stack",
            "password",
            "secret",
            "token",
            "cookie",
            "authorization",
            "parameters",
            "arguments",
            "locals",
        ]
        .iter()
        .any(|blocked| lower.contains(blocked))
}
fn label_value(value: &Json) -> Result<Json, Error> {
    let text = value
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            if value.is_number() || value.is_boolean() {
                Some(value.to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| fail("alert labels must be scalar values"))?;
    if text.len() > 128 {
        return Err(fail("alert label value exceeds 128 bytes"));
    }
    Ok(json!(moqui_core::telemetry::redact_text(&text)))
}

fn approved_labels(ec: &ExecutionContext, body: &Json) -> Result<BTreeSet<String>, Error> {
    fn collect(ec: &ExecutionContext, query: &Json, approved: &mut BTreeSet<String>) -> Result<(), Error> {
        if query["mode"] == "documents" {
            return Err(fail(
                "alerts require aggregate queries; raw document rows may not become notification labels",
            ));
        }
        if let Some(queries) = query["queries"].as_array() {
            for query in queries {
                collect(ec, query, approved)?;
            }
            return Ok(());
        }
        let id = text(query, "datasetId", "otel_metrics");
        let dataset = super::dataset(ec, id)?;
        if dataset.service.is_some() {
            let definition = super::definitions::read(ec, id)?;
            for field in definition["body"]["allowedLabelFields"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Json::as_str)
            {
                if label_name(field) {
                    approved.insert(field.into());
                }
            }
            return Ok(());
        }
        let facade = moqui_core::search::current()?;
        let mappings = super::metadata(ec, || {
            facade
                .target_documents(&dataset.target)?
                .iter()
                .map(|id| facade.mapping(id))
                .collect::<Result<Vec<_>, _>>()
        })?;
        fn walk(
            aggs: &indexmap::IndexMap<String, moqui_core::search::Agg>,
            mappings: &[std::sync::Arc<moqui_core::search::DocumentMapping>],
            approved: &mut BTreeSet<String>,
        ) {
            for (name, agg) in aggs {
                if let moqui_core::search::Agg::Terms(terms) = agg {
                    let field = terms.field.strip_suffix(".keyword").unwrap_or(&terms.field);
                    let kinds: Vec<_> = mappings
                        .iter()
                        .filter_map(|mapping| mapping.root.fields.get(field))
                        .map(|field| field.kind)
                        .collect();
                    if label_name(name)
                        && !kinds.is_empty()
                        && kinds.iter().all(|kind| {
                            matches!(
                                kind,
                                moqui_core::search::FieldKind::Keyword
                                    | moqui_core::search::FieldKind::Long
                                    | moqui_core::search::FieldKind::Boolean
                            )
                        })
                    {
                        approved.insert(name.clone());
                    }
                }
                if let Some(nested) = agg.sub_aggs() {
                    walk(nested, mappings, approved);
                }
            }
        }
        walk(&super::compile_aggregations(query)?, &mappings, approved);
        Ok(())
    }
    let mut approved = BTreeSet::new();
    collect(ec, &body["query"], &mut approved)?;
    if let Some(fields) = body["labelFields"].as_array() {
        let requested = fields
            .iter()
            .map(|field| {
                field
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| fail("labelFields must contain names"))
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        if !requested.is_subset(&approved) {
            return Err(fail(
                "labelFields must name approved categorical bucket fields; service datasets require allowedLabelFields",
            ));
        }
        approved = requested;
    }
    if approved.len() > 32 {
        return Err(fail("an alert may have at most 32 dimension labels"));
    }
    Ok(approved)
}

/// The most series one rule tracks. New series past it are left out with a notice.
const MAX_SERIES: usize = 500;
/// Consecutive evaluations a series may be missing from a successful result before it is removed.
const PRUNE_AFTER_MISSING: i64 = 10;

fn validate_condition(body: &Json) -> Result<(), Error> {
    let condition = &body["condition"];
    if !condition.is_object() && !condition.is_null() {
        return Err(fail("alert condition must be an object"));
    }
    let number = |key: &str| -> Result<Option<f64>, Error> {
        match &condition[key] {
            Json::Null => Ok(None),
            value => value
                .as_f64()
                .filter(|v| v.is_finite())
                .map(Some)
                .ok_or_else(|| fail(format!("alert condition {key} must be a number"))),
        }
    };
    match text(condition, "type", "above") {
        "above" | "below" => {
            number("threshold")?;
            number("recoveryThreshold")?;
        }
        "within" | "outside" => {
            let (Some(min), Some(max)) = (number("min")?, number("max")?) else {
                return Err(fail("a within or outside alert condition needs min and max"));
            };
            if min > max {
                return Err(fail("alert condition min must not exceed max"));
            }
        }
        other => {
            return Err(fail(format!(
                "unknown alert condition {other}; use above, below, within or outside"
            )))
        }
    }
    if !body["pruneAfterMissing"].is_null() && body["pruneAfterMissing"].as_i64().is_none_or(|n| n < 1) {
        return Err(fail("pruneAfterMissing must be a positive whole number"));
    }
    Ok(())
}

pub(super) fn validate_rule(ec: &ExecutionContext, body: &Json) -> Result<(), Error> {
    validate_condition(body)?;
    validate_contacts(body)?;
    validate_refs(ec, body)?;
    approved_labels(ec, body)?;
    if body["labels"].as_object().is_some_and(|labels| labels.len() > 32) {
        return Err(fail("an alert may have at most 32 static labels"));
    }
    for (name, value) in body["labels"].as_object().into_iter().flat_map(|map| map.iter()) {
        if !label_name(name) {
            return Err(fail("alert static label name is not approved"));
        }
        label_value(value)?;
    }
    Ok(())
}

fn property_name(name: &str) -> bool {
    (name.starts_with("monitor.webhook.") || name.starts_with("MONITOR_WEBHOOK_"))
        && name.len() <= 128
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
}

pub(super) fn validate_contacts(body: &Json) -> Result<(), Error> {
    for contact in body["contactPoints"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|contact| contact["type"] == "webhook")
    {
        if contact
            .get("headers")
            .is_some_and(|headers| !headers.is_null() && headers.as_object().is_none_or(|headers| !headers.is_empty()))
        {
            return Err(fail("literal webhook headers may not be saved; use headerProperties referencing MONITOR_WEBHOOK_* or monitor.webhook.* server properties"));
        }
        if let Some(properties) = contact.get("headerProperties").filter(|value| !value.is_null()) {
            let properties = properties
                .as_object()
                .ok_or_else(|| fail("webhook headerProperties must be a map"))?;
            for (header, property) in properties {
                if header.is_empty()
                    || header.len() > 128
                    || !header.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
                    || !property.as_str().is_some_and(property_name)
                {
                    return Err(fail("webhook headers need valid names and dedicated MONITOR_WEBHOOK_* or monitor.webhook.* property references"));
                }
            }
        }
        if let Some(property) = contact["urlProperty"].as_str() {
            if !property_name(property) {
                return Err(fail(
                    "webhook urlProperty must reference a dedicated monitoring property",
                ));
            }
        } else {
            let url = text(contact, "url", "");
            let authority = url
                .split_once("://")
                .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""));
            if !(url.starts_with("https://") || url.starts_with("http://"))
                || url.contains(['\r', '\n'])
                || authority.is_none_or(|host| host.is_empty() || host.contains('@'))
                || url.contains('?')
            {
                return Err(fail("webhook URL must be HTTP(S) without embedded credentials or query values; use urlProperty for credential-bearing URLs"));
            }
        }
    }
    Ok(())
}

/// Old/imported rows are still protected on group reads; only property names may be shared.
pub(super) fn redact_contact_credentials(body: &mut Json) {
    for contact in body["contactPoints"].as_array_mut().into_iter().flatten() {
        redact_contact(contact);
    }
}

/// [`redact_contact_credentials`] for a definition of any type: a shared contact point's body is the contact.
pub(super) fn redact_definition_credentials(kind: &Json, body: &mut Json) {
    if kind == "contactPoint" {
        redact_contact(body);
    } else {
        redact_contact_credentials(body);
    }
}

fn redact_contact(contact: &mut Json) {
    let Some(map) = contact.as_object_mut() else {
        return;
    };
    for key in ["headers", "authorization", "password", "token", "apiKey"] {
        if map.remove(key).is_some() {
            map.insert("credentialHeadersOmitted".into(), json!(true));
        }
    }
}

fn series(
    result: &Json,
    body: &Json,
    approved: &BTreeSet<String>,
) -> Result<BTreeMap<String, (Json, Option<f64>)>, Error> {
    let mut values: BTreeMap<String, (Json, Vec<Json>)> = BTreeMap::new();
    for frame in result["frames"].as_array().into_iter().flatten() {
        let frame: super::frame::Frame = serde_json::from_value(frame.clone()).map_err(fail)?;
        for field in frame.fields.iter().filter(|f| {
            f.kind == "number" && !approved.contains(&f.name) && (body["field"].is_null() || body["field"] == f.name)
        }) {
            for (index, value) in field.values.iter().enumerate() {
                let mut labels = serde_json::Map::new();
                for (name, value) in body["labels"].as_object().into_iter().flat_map(|map| map.iter()) {
                    if !label_name(name) {
                        return Err(fail("alert static label name is not approved"));
                    }
                    labels.insert(name.clone(), label_value(value)?);
                }
                labels.insert("field".into(), label_value(&json!(field.name))?);
                for (k, v) in &field.labels {
                    if approved.contains(k) {
                        labels.insert(k.clone(), label_value(&json!(v))?);
                    }
                }
                for dimension in frame.fields.iter().filter(|f| approved.contains(&f.name)) {
                    if let Some(value) = dimension.values.get(index) {
                        if value.is_null() {
                            continue;
                        }
                        if value.as_str().is_some_and(|s| s.parse::<jiff::Timestamp>().is_ok()) {
                            continue;
                        }
                        labels.insert(dimension.name.clone(), label_value(value)?);
                    }
                }
                let labels = Json::Object(labels);
                let key = moqui_lib::json::canonical_sha256(&labels);
                let entry = values.entry(key).or_insert_with(|| (labels, Vec::new()));
                entry.1.push(value.clone());
            }
        }
    }
    Ok(values
        .into_iter()
        .map(|(key, (labels, values))| {
            (
                key,
                (
                    labels,
                    super::frame::reduce(&values, text(body, "reduce", "last")).as_f64(),
                ),
            )
        })
        .collect())
}

fn matches_labels(matchers: &Json, labels: &Json) -> bool {
    matchers
        .as_object()
        .is_none_or(|map| map.iter().all(|(key, value)| labels.get(key) == Some(value)))
}

fn muted(body: &Json, labels: &Json, now: i64) -> bool {
    for silence in body["silences"].as_array().into_iter().flatten() {
        let from = silence["from"]
            .as_str()
            .and_then(|s| moqui_core::search::time::parse(s, now).ok())
            .unwrap_or(0);
        let thru = silence["thru"]
            .as_str()
            .and_then(|s| moqui_core::search::time::parse_thru(s, now).ok())
            .unwrap_or(0);
        if now >= from && now < thru && matches_labels(&silence["matchers"], labels) {
            return true;
        }
    }
    let time = jiff::Timestamp::from_millisecond(now)
        .unwrap()
        .to_zoned(moqui_core::l10n::time_zone());
    let weekday = time.date().weekday().to_monday_zero_offset() as u64;
    for timing in body["muteTimings"].as_array().into_iter().flatten() {
        if timing["weekdays"]
            .as_array()
            .is_some_and(|days| !days.iter().any(|day| day.as_u64() == Some(weekday)))
        {
            continue;
        }
        let minute = i64::from(time.hour()) * 60 + i64::from(time.minute());
        let parse = |key: &str, default: i64| {
            timing[key]
                .as_str()
                .and_then(|s| s.split_once(':'))
                .and_then(|(h, m)| Some(h.parse::<i64>().ok()? * 60 + m.parse::<i64>().ok()?))
                .unwrap_or(default)
        };
        let from = parse("start", 0);
        let thru = parse("end", 1440);
        if if from <= thru {
            minute >= from && minute < thru
        } else {
            minute >= from || minute < thru
        } {
            return true;
        }
    }
    false
}

fn send_contact(contact: &Json, payload: &Json) -> Result<(), Error> {
    match text(contact, "type", "notification") {
        "notification" => {
            let mut note = moqui_core::notify::NotificationMessage::new();
            note.topic(text(contact, "topic", "ReportAlert"))
                .title(text(payload, "title", "Report alert"))
                .link(text(payload, "link", "/apps/dashboards/Alerts"))
                .notification_type("warning")
                .message_json(payload.to_string())
                .persist(true);
            if let Some(user) = contact["userId"].as_str() {
                note.user_id(user);
            }
            if let Some(group) = contact["userGroupId"].as_str() {
                note.user_group_id(group);
            }
            note.send().map_err(fail)?;
            Ok(())
        }
        "email" => {
            let params = Parameters::from([
                (
                    "emailTemplateId".into(),
                    Value::string(text(contact, "emailTemplateId", "")),
                ),
                ("toAddresses".into(), Value::string(text(contact, "toAddresses", ""))),
                ("bodyParameters".into(), moqui_lib::json::from_json(payload)),
            ]);
            let sent = moqui_core::service::call::sync_checked(
                "org.moqui.impl.EmailServices.send#EmailTemplate",
                &params,
                &moqui_core::service::call::CallOptions {
                    ignore_previous_error: true,
                    ..Default::default()
                },
            )
            .map_err(fail)?;
            if sent
                .get("sendError")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty())
                || sent.get("messageId").and_then(Value::as_str).is_none_or(str::is_empty)
            {
                return Err(fail("alert email delivery failed; contact remains pending for retry"));
            }
            Ok(())
        }
        "webhook" => {
            validate_contacts(&json!({"contactPoints":[contact]}))?;
            moqui_core::user::authorize_own("AT_SERVICE", "org.moqui.report.contact.webhook", "update")
                .map_err(fail)?;
            let url = if let Some(property) = contact["urlProperty"].as_str() {
                moqui_lib::env::get_property(property)
                    .ok_or_else(|| fail(format!("webhook URL property {property} is not configured")))?
            } else {
                text(contact, "url", "").into()
            };
            let mut headers = vec![("content-type".into(), "application/json".into())];
            for (header, property) in contact["headerProperties"]
                .as_object()
                .into_iter()
                .flat_map(|map| map.iter())
            {
                let property = property.as_str().unwrap();
                let value = moqui_lib::env::get_property(property)
                    .ok_or_else(|| fail(format!("webhook header property {property} is not configured")))?;
                if value.contains(['\r', '\n']) {
                    return Err(fail("webhook header property contains an invalid line break"));
                }
                headers.push((header.clone(), value));
            }
            let response = {
                let _suppressed = moqui_core::telemetry::suppress();
                moqui_core::outbound::send(moqui_core::outbound::Request {
                    method: moqui_core::outbound::Method::Post,
                    url,
                    headers,
                    body: Some(payload.to_string()),
                    timeout: Some(std::time::Duration::from_secs(10)),
                    deny_internal: true,
                })
            }
            .map_err(|_| fail("alert webhook request failed"))?;
            if !(200..300).contains(&response.status) {
                return Err(fail(format!("alert webhook returned {}", response.status)));
            }
            Ok(())
        }
        other => Err(fail(format!("unknown alert contact point {other}"))),
    }
}

/// Save a notification group's row now, in its own transaction, outside the rule's state write.
fn record_notification(
    ec: &ExecutionContext,
    entity: &moqui_core::entity::EntityFacade,
    row: &moqui_core::entity::EntityValue,
) -> Result<(), Error> {
    entity.transactions_for("moqui.report.AlertNotification")?.run(
        moqui_core::transaction::TransactionAttribute::ForceNew,
        None,
        || {
            let _storage = ec.disable_authz();
            row.create_or_update(entity)
        },
    )
}

/// Groups events per contact point, delivers what is due, and returns the notification rows to save. Nothing here
/// holds a lock: the rule's claim keeps other evaluators away while contact I/O runs. A failed group or delivery is
/// logged and left pending for retry; the other groups go on.
fn notify(
    ec: &ExecutionContext,
    definition: &Json,
    body: &Json,
    events: &[Json],
    now: i64,
) -> Result<Vec<moqui_core::entity::EntityValue>, Error> {
    let entity = ec
        .factory()
        .entity()
        .ok_or_else(|| fail("entity facade is unavailable"))?;
    let mut rows = Vec::new();
    for (index, contact) in body["contactPoints"].as_array().into_iter().flatten().enumerate() {
        let mut groups: BTreeMap<String, (Json, Vec<Json>)> = BTreeMap::new();
        for event in events.iter().filter(|event| {
            matches_labels(&contact["matchers"], &event["labels"]) && !muted(body, &event["labels"], now)
        }) {
            let keys = contact["groupBy"].as_array();
            let labels = Json::Object(
                event["labels"]
                    .as_object()
                    .into_iter()
                    .flat_map(|m| m.iter())
                    .filter(|(k, _)| keys.is_none_or(|keys| keys.iter().any(|v| v.as_str() == Some(k))))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            );
            let key = moqui_lib::json::canonical_sha256(&labels);
            groups
                .entry(key)
                .or_insert_with(|| (labels, Vec::new()))
                .1
                .push(event.clone());
        }
        for (group, (labels, events)) in groups {
            let contact_id = contact["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| index.to_string());
            let id = text(definition, "id", "");
            let state_key: Vec<_> = events
                .iter()
                .map(|event| json!({"seriesId":event["seriesId"],"state":event["state"]}))
                .collect();
            let hash = moqui_lib::json::canonical_sha256(&json!(state_key));
            let resolved = events.iter().all(|event| event["state"] == "Normal");
            let payload = json!({"ruleId":id,"title":definition["title"],"labels":labels,"events":events,"link":format!("/apps/dashboards/Alerts?definitionId={id}")});
            let result = (|| -> Result<moqui_core::entity::EntityValue, Error> {
                let existing = {
                    let _storage = ec.disable_authz();
                    entity
                        .find("moqui.report.AlertNotification")?
                        .eq("definitionId", Value::string(id))
                        .eq("contactId", Value::string(&contact_id))
                        .eq("groupId", Value::string(&group))
                        .one()?
                };
                let mut row = existing.unwrap_or(entity.make_value("moqui.report.AlertNotification")?);
                let changed = row.get("stateHash").and_then(Value::as_str) != Some(hash.as_str());
                let first = if changed { now } else { millis(row.get("firstPending")) };
                let sent = millis(row.get("lastSent"));
                let group_wait = if resolved {
                    0
                } else {
                    contact["groupWaitSeconds"].as_i64().unwrap_or(30).max(0) * 1000
                };
                let repeat = contact["repeatSeconds"].as_i64().unwrap_or(14_400).max(60) * 1000;
                let pending = row.get("lastSentHash").and_then(Value::as_str) != Some(hash.as_str());
                if changed || sent == 0 {
                    for (k, v) in [
                        ("definitionId", Value::string(id)),
                        ("contactId", Value::string(&contact_id)),
                        ("groupId", Value::string(&group)),
                        ("stateHash", Value::string(&hash)),
                        ("firstPending", stamp(first)),
                        ("payloadJson", Value::string(payload.to_string())),
                    ] {
                        row.set(k, v);
                    }
                }
                if now.saturating_sub(first) >= group_wait
                    && (pending || sent == 0 || now.saturating_sub(sent) >= repeat)
                {
                    // Recorded as sent before sending, in a transaction of its own: if the rule's state write then
                    // fails, the retry finds this group's state already sent and does not send it again. A failed send
                    // puts the previous record back, so it is retried.
                    let previous = (row.get("lastSent").cloned(), row.get("lastSentHash").cloned());
                    row.set("lastSent", stamp(now));
                    row.set("lastSentHash", Value::string(&hash));
                    record_notification(ec, &entity, &row)?;
                    if let Err(error) = send_contact(contact, &payload) {
                        tracing::error!(rule=id,contact=%contact_id,%error,"alert delivery failed; will retry");
                        row.set("lastSent", previous.0.unwrap_or(Value::Null));
                        row.set("lastSentHash", previous.1.unwrap_or(Value::Null));
                        if let Err(error) = record_notification(ec, &entity, &row) {
                            tracing::error!(rule=id,contact=%contact_id,%error,"failed alert delivery could not be marked for retry; it is retried at the repeat interval");
                        }
                    }
                }
                Ok(row)
            })();
            match result {
                Ok(row) => rows.push(row),
                Err(error) => {
                    tracing::error!(rule=text(definition,"id",""),contact=%contact_id,%error,"alert notification group failed, continuing")
                }
            }
        }
    }
    Ok(rows)
}

/// Shared alert definition types a rule names by id: the type, the rule body key with the ids, and the rule body
/// list the shared items join at evaluation.
pub(super) const SHARED: [(&str, &str, &str); 3] = [
    ("contactPoint", "contactPointRefs", "contactPoints"),
    ("silence", "silenceRefs", "silences"),
    ("muteTiming", "muteTimingRefs", "muteTimings"),
];

/// The most shared items of one type a rule may reference.
const MAX_REFS: usize = 50;

fn shared_refs<'a>(body: &'a Json, key: &str) -> impl Iterator<Item = &'a str> {
    body[key].as_array().into_iter().flatten().filter_map(Json::as_str)
}

fn validate_silence(silence: &Json) -> Result<(), Error> {
    if !silence.is_object() {
        return Err(fail("a silence must be an object"));
    }
    let now = moqui_core::user::now_millis();
    let at = |key: &str| {
        silence[key]
            .as_str()
            .and_then(|s| {
                if key == "thru" {
                    moqui_core::search::time::parse_thru(s, now)
                } else {
                    moqui_core::search::time::parse(s, now)
                }
                .ok()
            })
            .ok_or_else(|| fail(format!("silence {key} must be a time")))
    };
    if at("thru")? <= at("from")? {
        return Err(fail("a silence must end after it starts"));
    }
    if !silence["matchers"].is_null() && !silence["matchers"].is_object() {
        return Err(fail("silence matchers must be a map of label values"));
    }
    Ok(())
}

fn validate_mute(timing: &Json) -> Result<(), Error> {
    if !timing.is_object() {
        return Err(fail("a mute timing must be an object"));
    }
    if !timing["weekdays"].is_null()
        && timing["weekdays"]
            .as_array()
            .is_none_or(|days| days.is_empty() || days.iter().any(|day| day.as_u64().is_none_or(|day| day > 6)))
    {
        return Err(fail(
            "mute timing weekdays must list days 0 (Monday) through 6 (Sunday)",
        ));
    }
    for key in ["start", "end"] {
        if timing[key].is_null() {
            continue;
        }
        let minutes = timing[key]
            .as_str()
            .and_then(|s| s.split_once(':'))
            .filter(|(h, m)| h.len() == 2 && m.len() == 2)
            .and_then(|(h, m)| Some((h.parse::<u32>().ok()?, m.parse::<u32>().ok()?)))
            .filter(|(_, m)| *m < 60)
            .map(|(h, m)| h * 60 + m);
        let last = if key == "end" { 1440 } else { 1439 };
        if minutes.is_none_or(|minutes| minutes > last) {
            return Err(fail(format!("mute timing {key} must be HH:mm")));
        }
    }
    Ok(())
}

/// Checks the body of a shared contact point, silence or mute timing before it is saved or shipped.
pub(super) fn validate_shared(kind: &str, body: &Json) -> Result<(), Error> {
    match kind {
        "contactPoint" => {
            if !body.is_object() {
                return Err(fail("a contact point must be an object"));
            }
            match text(body, "type", "notification") {
                "notification" | "email" | "webhook" => {}
                other => {
                    return Err(fail(format!(
                        "unknown contact point type {other}; use notification, email or webhook"
                    )))
                }
            }
            validate_contacts(&json!({ "contactPoints": [body] }))
        }
        "silence" => validate_silence(body),
        "muteTiming" => validate_mute(body),
        _ => Ok(()),
    }
}

/// A rule's shared references are lists of ids, each naming a definition of the matching type.
fn validate_refs(ec: &ExecutionContext, body: &Json) -> Result<(), Error> {
    for (kind, key, _) in SHARED {
        let refs = &body[key];
        if refs.is_null() {
            continue;
        }
        let refs = refs
            .as_array()
            .filter(|refs| refs.iter().all(Json::is_string))
            .ok_or_else(|| fail(format!("{key} must be a list of {kind} ids")))?;
        if refs.len() > MAX_REFS {
            return Err(fail(format!("an alert rule may reference at most {MAX_REFS} of {key}")));
        }
        for reference in refs.iter().filter_map(Json::as_str) {
            if super::definitions::read(ec, reference)?["type"] != kind {
                return Err(fail(format!("{reference} in {key} is not a {kind}")));
            }
        }
    }
    Ok(())
}

/// The most shared silences one evaluation pass loads to match by labels.
const MAX_SHARED_SILENCES: usize = 500;

/// The saved shared silences, read once per evaluation pass. Each applies by its matchers to every rule whose owner may
/// read it, not only to rules that reference it; one with no matchers applies only where referenced.
fn shared_silences(ec: &ExecutionContext) -> Vec<Json> {
    let rows = {
        let _metadata = ec.disable_authz();
        ec.factory().entity().map(|entity| {
            entity
                .find("moqui.report.Definition")
                .and_then(|find| find.eq("definitionType", Value::string("silence")).list())
        })
    };
    let rows = match rows {
        Some(Ok(rows)) => rows,
        Some(Err(error)) => {
            tracing::error!(%error, "shared silences could not be read; only referenced silences apply this pass");
            return Vec::new();
        }
        None => return Vec::new(),
    };
    let mut silences = Vec::new();
    for row in rows.iter() {
        match super::definitions::row_json(row) {
            Ok(silence) if silence["body"]["matchers"].as_object().is_some_and(|m| !m.is_empty()) => {
                silences.push(silence)
            }
            Ok(_) => {}
            Err(error) => {
                tracing::error!(silence=?row.get("definitionId"),%error,"shared silence could not be read, continuing")
            }
        }
        if silences.len() >= MAX_SHARED_SILENCES {
            tracing::warn!("more than {MAX_SHARED_SILENCES} shared silences; the rest apply only where referenced");
            break;
        }
    }
    silences
}

/// The rule body with the shared contact points, silences and mute timings it references added to its own, plus each
/// of `silences` (the pass's shared silences) its owner may read. A reference the owner cannot read, or that names
/// another type, is logged and left out; the rest still apply.
fn effective_body(ec: &ExecutionContext, id: &str, body: &Json, silences: &[Json]) -> Json {
    let mut effective = body.clone();
    for (kind, key, collection) in SHARED {
        for reference in shared_refs(body, key).take(MAX_REFS) {
            let item = match super::definitions::read(ec, reference) {
                Ok(definition) if definition["type"] == kind && definition["body"].is_object() => {
                    let mut item = definition["body"].clone();
                    if kind == "contactPoint" {
                        item["id"] = json!(reference);
                    }
                    item
                }
                Ok(_) => {
                    tracing::error!(rule = id, reference, "alert rule reference is not a {kind}, skipping");
                    continue;
                }
                Err(error) => {
                    tracing::error!(rule=id,reference,%error,"alert rule {kind} reference could not be read, skipping");
                    continue;
                }
            };
            if !effective[collection].is_array() {
                effective[collection] = json!([]);
            }
            if let Some(items) = effective[collection].as_array_mut() {
                items.push(item);
            }
        }
    }
    let referenced: BTreeSet<&str> = shared_refs(body, "silenceRefs").collect();
    for silence in silences {
        if referenced.contains(text(silence, "id", "")) {
            continue;
        }
        match super::definitions::readable(silence) {
            Ok(true) => {
                if !effective["silences"].is_array() {
                    effective["silences"] = json!([]);
                }
                if let Some(items) = effective["silences"].as_array_mut() {
                    items.push(silence["body"].clone());
                }
            }
            Ok(false) => {}
            Err(error) => {
                tracing::error!(rule=id,silence=?silence["id"],%error,"shared silence access could not be checked, skipping")
            }
        }
    }
    effective
}

const LOCK: &str = "moqui.report.AlertRuleLock";
/// How long a claim keeps other evaluators off a rule. A claim older than this was abandoned by a stopped evaluator.
const CLAIM_MS: i64 = 300_000;

fn claim_token(lock: &moqui_core::entity::EntityValue) -> Option<&str> {
    lock.get("claimToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
}

/// Claims the rule in a short transaction: `None` when it ran within its interval or another evaluator holds it.
fn claim(
    ec: &ExecutionContext,
    entity: &moqui_core::entity::EntityFacade,
    id: &str,
    now: i64,
    interval: i64,
) -> Result<Option<String>, Error> {
    entity
        .transactions_for(LOCK)?
        .run(moqui_core::transaction::TransactionAttribute::ForceNew, None, || {
            let _storage = ec.disable_authz();
            let mut lock = entity
                .find(LOCK)?
                .eq("definitionId", Value::string(id))
                .for_update(true)
                .one()?
                .ok_or_else(|| fail("alert rule has no evaluation lock; save it again"))?;
            if now.saturating_sub(millis(lock.get("lastEvaluated"))) < interval {
                return Ok(None);
            }
            // Claims expire on the wall clock: they guard against stopped evaluators, not evaluation time.
            let wall = moqui_core::user::now_millis();
            let held_until = lock
                .get("claimedUntil")
                .map(moqui_lib::object::to_plain_string)
                .and_then(|until| until.parse::<i64>().ok())
                .unwrap_or(0);
            if claim_token(&lock).is_some() && held_until > wall {
                return Ok(None);
            }
            let token = moqui_core::entity::uuid_v4()?;
            lock.set("claimToken", Value::string(&token));
            lock.set("claimedUntil", Value::I64(wall + CLAIM_MS));
            lock.update(entity)?;
            Ok(Some(token))
        })
}

/// What one evaluation writes, saved together once the query and deliveries are done.
#[derive(Default)]
struct Outcome {
    saved: Vec<moqui_core::entity::EntityValue>,
    removed: Vec<moqui_core::entity::EntityValue>,
    annotations: Vec<moqui_core::entity::document::Document>,
    changed: usize,
    notices: Vec<Json>,
}

/// Writes the outcome and the rule's evaluation time in a short transaction, if this evaluator still holds the claim.
/// With `outcome` `None` it only gives the claim back, so the rule runs again at the next pass.
fn finish(
    ec: &ExecutionContext,
    entity: &moqui_core::entity::EntityFacade,
    id: &str,
    token: &str,
    now: i64,
    outcome: Option<&Outcome>,
) -> Result<(), Error> {
    entity
        .transactions_for(LOCK)?
        .run(moqui_core::transaction::TransactionAttribute::ForceNew, None, || {
            let _storage = ec.disable_authz();
            let mut lock = entity
                .find(LOCK)?
                .eq("definitionId", Value::string(id))
                .for_update(true)
                .one()?
                .ok_or_else(|| fail("alert rule evaluation lock disappeared"))?;
            if claim_token(&lock) != Some(token) {
                return Err(fail(
                    "alert rule claim expired and another evaluator took the rule; this result is discarded",
                ));
            }
            if let Some(outcome) = outcome {
                for row in &outcome.removed {
                    row.delete(entity)?;
                }
                for row in &outcome.saved {
                    row.create_or_update(entity)?;
                }
                lock.set("lastEvaluated", stamp(now));
            }
            lock.set("claimToken", Value::Null);
            lock.set("claimedUntil", Value::Null);
            lock.update(entity)
        })
}

/// Claims the rule, evaluates and delivers with no lock held, then saves the result. A failure gives the claim back
/// without moving the rule's evaluation time, so the next pass retries it.
fn evaluate_owned(
    ec: &ExecutionContext,
    definition: &Json,
    now: i64,
    silences: &[Json],
) -> Result<(usize, Vec<Json>), Error> {
    let entity = ec
        .factory()
        .entity()
        .ok_or_else(|| fail("entity facade is unavailable"))?;
    let id = text(definition, "id", "");
    let interval = definition["body"]["intervalSeconds"].as_i64().unwrap_or(60).max(60) * 1000;
    let Some(token) = claim(ec, &entity, id, now, interval)? else {
        return Ok((0, Vec::new()));
    };
    let result = evaluate_claimed(ec, &entity, definition, now, interval, silences)
        .and_then(|outcome| finish(ec, &entity, id, &token, now, Some(&outcome)).map(|()| outcome));
    match result {
        Ok(outcome) => {
            // History is written after the state it records is saved.
            if !outcome.annotations.is_empty() {
                if let Err(error) = moqui_core::search::current()
                    .and_then(|search| search.index_entries("monitor_annotations", outcome.annotations))
                {
                    tracing::error!(rule=id,%error,"could not record alert annotations");
                }
            }
            Ok((outcome.changed, outcome.notices))
        }
        Err(error) => {
            if let Err(release) = finish(ec, &entity, id, &token, now, None) {
                tracing::error!(rule=id,error=%release,"could not release alert rule claim; it expires on its own");
            }
            Err(error)
        }
    }
}

/// Runs the query, steps each series and delivers notifications while the rule is claimed but no lock is held.
fn evaluate_claimed(
    ec: &ExecutionContext,
    entity: &moqui_core::entity::EntityFacade,
    definition: &Json,
    now: i64,
    interval: i64,
    silences: &[Json],
) -> Result<Outcome, Error> {
    let id = text(definition, "id", "");
    let body = &definition["body"];
    let result = super::execute_query(ec, &body["query"]);
    let mut query_failed = result.is_err();
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            tracing::error!(rule=id,%error,"alert query failed");
            json!({"frames":[],"notices":[{"complete":false}]})
        }
    };
    let complete = result["notices"]
        .as_array()
        .is_none_or(|notices| notices.iter().all(|n| n["complete"] != false));
    let labels = match approved_labels(ec, body) {
        Ok(labels) => labels,
        Err(error) => {
            query_failed = true;
            tracing::error!(rule=id,%error,"alert label policy refused query");
            BTreeSet::new()
        }
    };
    let mut series = if query_failed || !complete {
        BTreeMap::new()
    } else {
        match series(&result, body, &labels) {
            Ok(series) => series,
            Err(error) => {
                query_failed = true;
                tracing::error!(rule=id,%error,"alert label policy refused result");
                BTreeMap::new()
            }
        }
    };
    let has_actual_data = !series.is_empty();
    // A series counts as missing only when the query ran and returned a complete result.
    let observed = !query_failed && complete;
    let prune_after = body["pruneAfterMissing"].as_i64().unwrap_or(PRUNE_AFTER_MISSING).max(1);
    let rows = {
        let _storage = ec.disable_authz();
        entity
            .find("moqui.report.AlertInstance")?
            .eq("definitionId", Value::string(id))
            .list()?
    };
    let existing: BTreeMap<String, moqui_core::entity::EntityValue> = rows
        .iter()
        .map(|row| {
            (
                row.get("seriesId")
                    .map(moqui_lib::object::to_plain_string)
                    .unwrap_or_default(),
                row.clone(),
            )
        })
        .collect();
    let mut outcome = Outcome::default();
    let fresh = series
        .keys()
        .filter(|key| !existing.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let room = MAX_SERIES.saturating_sub(existing.len());
    if fresh.len() > room {
        for key in &fresh[room..] {
            series.remove(key);
        }
        let dropped = fresh.len() - room;
        tracing::warn!(
            rule = id,
            dropped,
            "alert rule exceeds {MAX_SERIES} series; new series past it are not evaluated"
        );
        outcome.notices.push(json!({"level":"warning","code":"series_limit","rule":id,"message":format!("Alert rule {id} has more than {MAX_SERIES} series; {dropped} new series are not evaluated")}));
    }
    let reported = series.keys().cloned().collect::<BTreeSet<_>>();
    if series.is_empty() && existing.is_empty() {
        series.insert("__rule".into(), (json!({"rule":id}), None));
    }
    for (key, row) in &existing {
        series.entry(key.clone()).or_insert_with(|| {
            let labels = row
                .get("labelsJson")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| json!({}));
            (labels, None)
        });
    }
    let mut events = Vec::new();
    let mut pruned = 0;
    for (key, (labels, value)) in series {
        let previous = existing.get(&key);
        let mut instance = previous.map(state_from_row).unwrap_or_default();
        if now.saturating_sub(instance.last_evaluated) < interval {
            continue;
        }
        // The rule placeholder is missing only once real series exist.
        let missing = observed && !reported.contains(&key) && (key != "__rule" || has_actual_data);
        let missing_count = if missing {
            previous
                .and_then(|row| row.get("missingCount"))
                .map(moqui_lib::object::to_plain_string)
                .and_then(|count| count.parse::<i64>().ok())
                .unwrap_or(0)
                .saturating_add(1)
        } else {
            0
        };
        if missing_count >= prune_after {
            if let Some(row) = previous {
                outcome.removed.push(row.clone());
            }
            pruned += 1;
            if instance.state != State::Normal {
                // Removing a series that was not normal resolves it for its contacts.
                events.push(json!({"seriesId":key,"state":"Normal","labels":labels,"value":Json::Null,"pruned":true}));
            }
            continue;
        }
        let condition = &body["condition"];
        let threshold = if instance.previously_firing {
            condition["recoveryThreshold"]
                .as_f64()
                .or_else(|| condition["threshold"].as_f64())
        } else {
            condition["threshold"].as_f64()
        }
        .unwrap_or(0.0);
        let condition = match text(condition, "type", "above") {
            "above" => Condition::Above(threshold),
            "below" => Condition::Below(threshold),
            "within" => Condition::Within(
                condition["min"].as_f64().unwrap_or(0.0),
                condition["max"].as_f64().unwrap_or(0.0),
            ),
            "outside" => Condition::Outside(
                condition["min"].as_f64().unwrap_or(0.0),
                condition["max"].as_f64().unwrap_or(0.0),
            ),
            other => return Err(fail(format!("unknown alert condition {other}"))),
        };
        let observation = if query_failed || !complete {
            Observation::Error
        } else if key == "__rule" && has_actual_data {
            Observation::Healthy
        } else {
            condition.observe(value)
        };
        let changed = instance.evaluate(
            observation,
            now,
            body["pendingSeconds"].as_i64().unwrap_or(0).max(0) * 1000,
            body["keepFiringSeconds"].as_i64().unwrap_or(0).max(0) * 1000,
        );
        let state = serde_json::to_value(instance.state).map_err(fail)?;
        let mut row = previous
            .cloned()
            .unwrap_or(entity.make_value("moqui.report.AlertInstance")?);
        for (k, v) in [
            ("definitionId", Value::string(id)),
            ("seriesId", Value::string(&key)),
            ("state", moqui_lib::json::from_json(&state)),
            ("stateSince", stamp(instance.since)),
            ("lastEvaluated", stamp(now)),
            (
                "previouslyFiring",
                Value::string(if instance.previously_firing { "Y" } else { "N" }),
            ),
            ("labelsJson", Value::string(labels.to_string())),
            ("missingCount", Value::I64(missing_count)),
        ] {
            row.set(k, v);
        }
        outcome.saved.push(row);
        if changed {
            outcome.changed += 1;
            let annotation = json!({"_id":format!("alert:{id}:{key}:{now}"),"time":jiff::Timestamp::from_millisecond(now).unwrap().to_string(),"source":"alert","text":format!("{}: {}",text(definition,"title",id),state.as_str().unwrap_or("Error")),"scope":"dashboard","dashboard":body["dashboardId"],"panel":body["panelId"],"rule_id":id,"series_id":key,"state":state,"labels":labels});
            if let Value::Map(document) = moqui_lib::json::from_json(&annotation) {
                outcome.annotations.push(document);
            }
        }
        if matches!(
            instance.state,
            State::Firing | State::Recovering | State::NoData | State::Error
        ) || changed
        {
            events.push(json!({"seriesId":key,"state":state,"labels":labels,"value":value}));
        }
    }
    if pruned > 0 {
        tracing::info!(
            rule = id,
            pruned,
            "alert series missing for {prune_after} evaluations were removed"
        );
    }
    if !events.is_empty() {
        let effective = effective_body(ec, id, body, silences);
        outcome.saved.extend(notify(ec, definition, &effective, &events, now)?);
    }
    Ok(outcome)
}

/// One rule in its own context, authenticated as the rule's owner, with the evaluator's time as its effective time
/// so relative query ranges and state timers agree.
fn evaluate_rule(
    factory: &std::sync::Arc<moqui_core::context::ExecutionContextFactory>,
    definition: &Json,
    now: i64,
    silences: &[Json],
) -> Result<(usize, Vec<Json>), Error> {
    let entity = factory.entity().ok_or_else(|| fail("entity facade is unavailable"))?;
    let context = factory.new_context();
    let _entered = context.enter();
    let owner = text(definition, "ownerId", "");
    let account = {
        let _authz = context.disable_authz();
        entity
            .find("moqui.security.UserAccount")?
            .eq("userId", Value::string(owner))
            .one()?
            .ok_or_else(|| fail("alert owner account no longer exists"))?
    };
    moqui_core::user::internal_login_user(account.get("username").and_then(Value::as_str).unwrap_or(""))
        .map_err(fail)?;
    context.set_effective_time(Some(now));
    evaluate_owned(&context, definition, now, silences)
}

/// Scheduled pass: definitions are loaded as system metadata, then each rule runs in a fresh context authenticated
/// as its owner. Rules run on a few workers at once (`report` setting `alert-workers`, 4 by default); independent
/// rules keep running after one fails.
pub fn evaluate_rules(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    params: &Parameters,
) -> Result<Parameters, Error> {
    let params = input(params);
    let selected = params["definitionId"].as_str().filter(|id| !id.is_empty());
    if let Some(id) = selected {
        let definition = super::definitions::read(ec, id)?;
        if definition["type"] != "alert" {
            return Err(fail("selected definition is not an alert rule"));
        }
    } else {
        moqui_core::user::authorize_own("AT_SERVICE", "org.moqui.report.alert.evaluateAll", "update").map_err(fail)?;
    }
    let now = ec.effective_time().unwrap_or_else(moqui_core::user::now_millis);
    let entity = ec
        .factory()
        .entity()
        .ok_or_else(|| fail("entity facade is unavailable"))?;
    let (definitions, invalid) = {
        let _guard = ec.disable_authz();
        let mut find = entity
            .find("moqui.report.Definition")?
            .eq("definitionType", Value::string("alert"));
        if let Some(id) = selected {
            find = find.eq("definitionId", Value::string(id));
        }
        let rows = find.list()?;
        let mut definitions = std::collections::VecDeque::new();
        let mut invalid = 0;
        for row in rows.iter() {
            match super::definitions::row_json(row) {
                Ok(definition) if definition["body"]["paused"] == true => {}
                Ok(definition) => definitions.push_back(definition),
                Err(error) => {
                    invalid += 1;
                    tracing::error!(rule=?row.get("definitionId"),%error,"alert rule could not be read, continuing");
                }
            }
        }
        (definitions, invalid)
    };
    let silences = shared_silences(ec);
    let factory = ec.factory().clone();
    let workers = factory
        .conf_number::<usize>("report", "alert-workers", 4)
        .clamp(1, 32)
        .min(definitions.len().max(1));
    let queue = std::sync::Mutex::new(definitions);
    let totals = std::sync::Mutex::new((0usize, invalid, Vec::new()));
    let work = || loop {
        let Some(definition) = queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front() else {
            break;
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            evaluate_rule(&factory, &definition, now, &silences)
        }))
        .unwrap_or_else(|_| Err(fail("alert rule evaluation panicked")));
        let mut totals = totals.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok((_, rule_notices)) => {
                totals.0 += 1;
                totals.2.extend(rule_notices);
            }
            Err(error) => {
                totals.1 += 1;
                tracing::error!(rule=text(&definition,"id",""),%error,"alert rule failed, continuing");
            }
        }
    };
    std::thread::scope(|scope| {
        for _ in 1..workers {
            if let Err(error) = std::thread::Builder::new()
                .name("alert-evaluator".into())
                .spawn_scoped(scope, work)
            {
                tracing::error!(%error,"alert evaluator worker could not start; the others take its rules");
            }
        }
        // This thread works the queue too, so the pass finishes even when no worker starts.
        work();
    });
    let (evaluated, failed, notices) = totals.into_inner().unwrap_or_else(|e| e.into_inner());
    Ok(output(json!({"evaluated":evaluated,"failed":failed,"notices":notices})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timers_and_unknown_health() {
        let mut i = Instance::default();
        assert!(i.evaluate(Observation::Breaching, 100, 20, 30));
        assert_eq!(i.state, State::Pending);
        i.evaluate(Observation::Breaching, 119, 20, 30);
        assert_eq!(i.state, State::Pending);
        i.evaluate(Observation::Breaching, 120, 20, 30);
        assert_eq!(i.state, State::Firing);
        i.evaluate(Observation::NoData, 130, 20, 30);
        assert_eq!(i.state, State::NoData);
        i.evaluate(Observation::Healthy, 140, 20, 30);
        assert_eq!(i.state, State::Recovering);
        i.evaluate(Observation::Healthy, 169, 20, 30);
        assert_eq!(i.state, State::Recovering);
        i.evaluate(Observation::Healthy, 170, 20, 30);
        assert_eq!(i.state, State::Normal);
    }
    #[test]
    fn error_is_not_normal_and_stale_samples_do_not_change_state() {
        let mut i = Instance::default();
        i.evaluate(Observation::Error, 100, 0, 0);
        i.evaluate(Observation::Healthy, 99, 0, 0);
        assert_eq!(i.state, State::Error);
        assert!(matches!(
            Condition::Above(1.0).observe(Some(f64::NAN)),
            Observation::NoData
        ));
    }
    #[test]
    fn alert_instances_keep_only_approved_record_dimensions() {
        let result = json!({"frames":[{"fields":[{"name":"total","type":"number","values":[9]},{"name":"productId","type":"string","values":["P100"]},{"name":"body","type":"string","values":["private invoice text"]}]}]});
        let instances = series(
            &result,
            &json!({"field":"total"}),
            &BTreeSet::from(["productId".into()]),
        )
        .unwrap();
        let (labels, value) = instances.values().next().unwrap();
        assert_eq!(labels["productId"], "P100");
        assert!(labels.get("body").is_none());
        assert!(!labels.to_string().contains("private invoice text"));
        assert_eq!(*value, Some(9.0));
    }
    #[test]
    fn conditions_need_a_known_type_and_ordered_bounds() {
        for good in [
            json!({}),
            json!({"condition":{"type":"above","threshold":10,"recoveryThreshold":7}}),
            json!({"condition":{"type":"below"}}),
            json!({"condition":{"type":"within","min":1,"max":1}}),
            json!({"condition":{"type":"outside","min":-5,"max":5},"pruneAfterMissing":3}),
        ] {
            assert!(validate_condition(&good).is_ok(), "{good}");
        }
        for bad in [
            json!({"condition":{"type":"abvoe","threshold":10}}),
            json!({"condition":{"type":"above","threshold":"ten"}}),
            json!({"condition":{"type":"within","min":5,"max":1}}),
            json!({"condition":{"type":"outside","min":1}}),
            json!({"condition":5}),
            json!({"condition":{"type":"above"},"pruneAfterMissing":0}),
        ] {
            assert!(validate_condition(&bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn document_queries_cannot_be_alert_sources() {
        let ec = moqui_core::context::ExecutionContextFactory::test_context();
        assert!(approved_labels(
            &ec,
            &json!({"query":{"queries":[{"mode":"documents","datasetId":"otel_logs"}]}})
        )
        .is_err());
    }
    #[test]
    fn shared_contacts_use_property_names_and_never_literal_credentials() {
        let mut legacy = json!({"contactPoints":[{"type":"webhook","url":"https://example.test/alerts","headers":{"Authorization":"Bearer private-fixture-token"}}]});
        assert!(validate_contacts(&legacy).is_err());
        redact_contact_credentials(&mut legacy);
        assert!(!legacy.to_string().contains("private-fixture-token"));
        assert!(validate_contacts(&json!({"contactPoints":[{"type":"webhook","urlProperty":"MONITOR_WEBHOOK_ALERT_URL","headerProperties":{"Authorization":"MONITOR_WEBHOOK_ALERT_AUTH"}}]})).is_ok());
        assert!(validate_contacts(
            &json!({"contactPoints":[{"type":"webhook","url":"https://user:password@example.test/alerts"}]})
        )
        .is_err());
    }
    #[test]
    fn shared_items_validate_like_rule_items() {
        for (kind, good) in [
            ("contactPoint", json!({"type":"notification","topic":"ReportAlert"})),
            (
                "contactPoint",
                json!({"type":"webhook","urlProperty":"MONITOR_WEBHOOK_OPS_URL"}),
            ),
            (
                "silence",
                json!({"from":"2026-10-26T00:00:00Z","thru":"2026-10-27T00:00:00Z","matchers":{"team":"ops"}}),
            ),
            ("silence", json!({"from":"now","thru":"now+2h"})),
            ("muteTiming", json!({"weekdays":[5,6],"start":"18:00","end":"24:00"})),
            ("muteTiming", json!({"start":"22:00","end":"06:00"})),
        ] {
            assert!(validate_shared(kind, &good).is_ok(), "{kind} {good}");
        }
        for (kind, bad) in [
            ("contactPoint", json!({"type":"pager"})),
            (
                "contactPoint",
                json!({"type":"webhook","url":"https://example.test/a","headers":{"Authorization":"x"}}),
            ),
            ("contactPoint", json!("ops")),
            (
                "silence",
                json!({"from":"2026-10-27T00:00:00Z","thru":"2026-10-26T00:00:00Z"}),
            ),
            ("silence", json!({"from":"2026-10-26T00:00:00Z"})),
            ("silence", json!({"from":"now","thru":"now+1h","matchers":["team"]})),
            ("muteTiming", json!({"weekdays":[]})),
            ("muteTiming", json!({"weekdays":[7]})),
            ("muteTiming", json!({"start":"24:00"})),
            ("muteTiming", json!({"end":"24:01"})),
            ("muteTiming", json!({"start":"9:00"})),
        ] {
            assert!(validate_shared(kind, &bad).is_err(), "{kind} {bad}");
        }
    }
    #[test]
    fn shared_contact_points_hide_credentials_from_other_viewers() {
        let mut body = json!({"type":"webhook","url":"https://example.test/a","headers":{"Authorization":"Bearer fixture"},"headerProperties":{"Authorization":"MONITOR_WEBHOOK_A"}});
        redact_definition_credentials(&json!("contactPoint"), &mut body);
        assert!(!body.to_string().contains("Bearer fixture"));
        assert_eq!(body["credentialHeadersOmitted"], true);
        assert_eq!(body["headerProperties"]["Authorization"], "MONITOR_WEBHOOK_A");
    }
    #[test]
    fn shared_items_join_the_rule_body_for_muting() {
        let monday_noon = "2026-10-26T12:00:00Z"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .as_millisecond();
        let labels = json!({"team":"ops"});
        let mut body = json!({});
        assert!(!muted(&body, &labels, monday_noon));
        body["silences"] =
            json!([{"from":"2026-10-26T11:00:00Z","thru":"2026-10-26T13:00:00Z","matchers":{"team":"ops"}}]);
        assert!(muted(&body, &labels, monday_noon));
        assert!(!muted(&body, &json!({"team":"sales"}), monday_noon));
    }
}
