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

fn state_from_row(row: &crate::entity::EntityValue) -> Instance {
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
    Ok(json!(crate::telemetry::redact_text(&text)))
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
        let facade = crate::search::current()?;
        let mappings = super::metadata(ec, || {
            facade
                .target_documents(&dataset.target)?
                .iter()
                .map(|id| facade.mapping(id))
                .collect::<Result<Vec<_>, _>>()
        })?;
        fn walk(
            aggs: &indexmap::IndexMap<String, crate::search::Agg>,
            mappings: &[std::sync::Arc<crate::search::DocumentMapping>],
            approved: &mut BTreeSet<String>,
        ) {
            for (name, agg) in aggs {
                if let crate::search::Agg::Terms(terms) = agg {
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
                                crate::search::FieldKind::Keyword
                                    | crate::search::FieldKind::Long
                                    | crate::search::FieldKind::Boolean
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

pub(super) fn validate_rule(ec: &ExecutionContext, body: &Json) -> Result<(), Error> {
    validate_contacts(body)?;
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
        if contact.get("headers").is_some() {
            contact.as_object_mut().unwrap().remove("headers");
            contact["credentialHeadersOmitted"] = json!(true);
        }
        for key in ["authorization", "password", "token", "apiKey"] {
            if contact.get(key).is_some() {
                contact.as_object_mut().unwrap().remove(key);
                contact["credentialHeadersOmitted"] = json!(true);
            }
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
            .and_then(|s| super::time::parse(s, now).ok())
            .unwrap_or(0);
        let thru = silence["thru"]
            .as_str()
            .and_then(|s| super::time::parse(s, now).ok())
            .unwrap_or(0);
        if now >= from && now < thru && matches_labels(&silence["matchers"], labels) {
            return true;
        }
    }
    let time = jiff::Timestamp::from_millisecond(now)
        .unwrap()
        .to_zoned(crate::l10n::time_zone());
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
            let mut note = crate::notify::NotificationMessage::new();
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
            let sent = crate::service::call::sync_checked(
                "org.moqui.impl.EmailServices.send#EmailTemplate",
                &params,
                &crate::service::call::CallOptions {
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
            crate::user::authorize_own("AT_SERVICE", "org.moqui.report.contact.webhook", "update").map_err(fail)?;
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
                let _suppressed = crate::telemetry::suppress();
                crate::outbound::send(crate::outbound::Request {
                    method: crate::outbound::Method::Post,
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

fn notify(ec: &ExecutionContext, definition: &Json, events: &[Json], now: i64) -> Result<(), Error> {
    let body = &definition["body"];
    let entity = ec
        .factory()
        .entity()
        .ok_or_else(|| fail("entity facade is unavailable"))?;
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
            let result = (|| -> Result<(), Error> {
                let existing = {
                    let _storage = ec.disable_authz();
                    entity
                        .find("moqui.report.AlertNotification")?
                        .eq("definitionId", Value::string(id))
                        .eq("contactId", Value::string(&contact_id))
                        .eq("groupId", Value::string(&group))
                        .for_update(true)
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
                    match send_contact(contact, &payload) {
                        Ok(()) => {
                            row.set("lastSent", stamp(now));
                            row.set("lastSentHash", Value::string(&hash));
                        }
                        Err(error) => {
                            tracing::error!(rule=id,contact=%contact_id,%error,"alert delivery failed; will retry")
                        }
                    }
                }
                let _storage = ec.disable_authz();
                row.create_or_update(&entity)
            })();
            if let Err(error) = result {
                tracing::error!(rule=text(definition,"id",""),contact=%contact_id,%error,"alert notification group failed, continuing");
            }
        }
    }
    Ok(())
}

fn evaluate_owned(ec: &ExecutionContext, definition: &Json, now: i64) -> Result<usize, Error> {
    let entity = ec
        .factory()
        .entity()
        .ok_or_else(|| fail("entity facade is unavailable"))?;
    let id = text(definition, "id", "");
    let body = &definition["body"];
    let interval = body["intervalSeconds"].as_i64().unwrap_or(60).max(60) * 1000;
    let mut rule_lock = {
        let _storage = ec.disable_authz();
        entity
            .find("moqui.report.AlertRuleLock")?
            .eq("definitionId", Value::string(id))
            .for_update(true)
            .one()?
            .ok_or_else(|| fail("alert rule has no evaluation lock; save it again"))?
    };
    if now.saturating_sub(millis(rule_lock.get("lastEvaluated"))) < interval {
        return Ok(0);
    }
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
    let rows = {
        let _storage = ec.disable_authz();
        entity
            .find("moqui.report.AlertInstance")?
            .eq("definitionId", Value::string(id))
            .for_update(true)
            .list()?
    };
    let existing: BTreeMap<String, crate::entity::EntityValue> = rows
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
    let mut changed_count = 0;
    for (key, (labels, value)) in series {
        let previous = existing.get(&key);
        let mut instance = previous.map(state_from_row).unwrap_or_default();
        if now.saturating_sub(instance.last_evaluated) < interval {
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
        ] {
            row.set(k, v);
        }
        {
            let _storage = ec.disable_authz();
            row.create_or_update(&entity)?;
        }
        if changed {
            changed_count += 1;
            let annotation = json!({"_id":format!("alert:{id}:{key}:{now}"),"time":jiff::Timestamp::from_millisecond(now).unwrap().to_string(),"source":"alert","text":format!("{}: {}",text(definition,"title",id),state.as_str().unwrap_or("Error")),"scope":"dashboard","dashboard":body["dashboardId"],"panel":body["panelId"],"rule_id":id,"series_id":key,"state":state,"labels":labels});
            if let Value::Map(document) = moqui_lib::json::from_json(&annotation) {
                if let Err(error) = crate::search::current()
                    .and_then(|search| search.index_entries("monitor_annotations", vec![document]))
                {
                    tracing::error!(rule=id,%error,"could not record alert annotation");
                }
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
    notify(ec, definition, &events, now)?;
    rule_lock.set("lastEvaluated", stamp(now));
    {
        let _storage = ec.disable_authz();
        rule_lock.update(&entity)?;
    }
    Ok(changed_count)
}

/// Scheduled pass: definitions are loaded as system metadata, then each query runs in a fresh
/// context authenticated as its owner. Independent rules keep running after a failure.
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
        crate::user::authorize_own("AT_SERVICE", "org.moqui.report.alert.evaluateAll", "update").map_err(fail)?;
    }
    let now = ec.effective_time().unwrap_or_else(crate::user::now_millis);
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
        let mut definitions = Vec::new();
        let mut invalid = 0;
        for row in rows.iter() {
            match super::definitions::row_json(row) {
                Ok(definition) => definitions.push(definition),
                Err(error) => {
                    invalid += 1;
                    tracing::error!(rule=?row.get("definitionId"),%error,"alert rule could not be read, continuing");
                }
            }
        }
        (definitions, invalid)
    };
    let mut evaluated = 0;
    let mut failed = invalid;
    for definition in definitions {
        if definition["body"]["paused"] == true {
            continue;
        }
        let result = (|| -> Result<usize, Error> {
            let owner = text(&definition, "ownerId", "");
            let account = {
                let _authz = ec.disable_authz();
                entity
                    .find("moqui.security.UserAccount")?
                    .eq("userId", Value::string(owner))
                    .one()?
                    .ok_or_else(|| fail("alert owner account no longer exists"))?
            };
            let context = ec.factory().new_context();
            let _entered = context.enter();
            crate::user::internal_login_user(account.get("username").and_then(Value::as_str).unwrap_or(""))
                .map_err(fail)?;
            let tx = entity.transactions_for("moqui.report.AlertInstance")?;
            tx.run(crate::transaction::TransactionAttribute::ForceNew, None, || {
                evaluate_owned(&context, &definition, now)
            })
        })();
        match result {
            Ok(_) => evaluated += 1,
            Err(error) => {
                failed += 1;
                tracing::error!(rule=text(&definition,"id",""),%error,"alert rule failed, continuing");
            }
        }
    }
    Ok(output(json!({"evaluated":evaluated,"failed":failed})))
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
    fn document_queries_cannot_be_alert_sources() {
        let ec = crate::context::ExecutionContextFactory::test_context();
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
}
