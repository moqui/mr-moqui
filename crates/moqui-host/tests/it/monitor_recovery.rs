//! Recovery on a real seeded PostgreSQL server (plans/Monitoring.md, Later: Recovery): the default policy only
//! escalates; an approval policy proposes isolating the one failing instance and runs nothing until approved, then
//! drains it and verifies the service recovered; a drained instance answers /health with 503; the emergency stop
//! blocks everything but escalation; an automatic rollback through a webhook controller is retried with the same
//! idempotency key, completed by the controller's callback, and found Inconclusive when no telemetry follows it.
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "telemetry"))]
use crate::common;
use moqui_core::entity::document::Document;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

const MINUTE: i64 = 60_000;
const BOUNDS: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

fn document(value: Json) -> Document {
    match moqui_lib::json::from_json(&value) {
        Value::Map(map) => map.into_iter().collect(),
        _ => unreachable!(),
    }
}

fn minute(service: &str, version: &str, instance: &str, at: i64, ok: u64, failed: u64) -> Vec<Document> {
    let at = at - at.rem_euclid(MINUTE);
    [(200, ok), (500, failed)]
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(status, count)| {
            let mut buckets = vec![0_u64; BOUNDS.len() + 1];
            buckets[3] = count;
            document(json!({
                "_id": format!("{service}-{version}-{instance}-{at}-{status}"),
                "name": "http.server.request.duration", "granularity": "minute", "aggregation": "histogram",
                "bucket_start": at, "bucket_end": at + MINUTE,
                "service.name": service, "service.version": version, "service.instance.id": instance,
                "http.response.status_code": status, "count": count, "sum": count as f64 * 0.03,
                "bounds": BOUNDS, "buckets": buckets,
            }))
        })
        .collect()
}

fn release(service: &str, version: &str, at: i64) -> Document {
    document(json!({
        "_id": format!("release-{service}-{version}"), "time": at, "source": "release", "scope": "service",
        "service.name": service, "service.version": version, "service.instance.id": format!("{service}-a"),
        "vcs.ref.head.revision": format!("rev-{version}"), "text": format!("{service} {version}"),
        "tags": ["release"], "moqui.schema.hash": "same-schema",
    }))
}

fn call(service: &str, parameters: Json) -> Json {
    let Value::Map(map) = moqui_lib::json::from_json(&parameters) else {
        unreachable!()
    };
    let parameters: BTreeMap<String, Value> = map.into_iter().collect();
    let out = moqui_core::service::sync(service, &parameters).unwrap_or_else(|error| panic!("{service}: {error}"));
    let errors = moqui_core::context::ExecutionContext::with_current(|ctx| {
        let errors = ctx.message().errors().to_vec();
        ctx.message().clear_errors();
        errors
    });
    assert_eq!(errors, Some(Vec::new()), "{service}");
    moqui_lib::json::to_json(&Value::Map(out.into_iter().collect()))
}

fn decisions(incident: &Json) -> Vec<Json> {
    call(
        "org.moqui.monitor.MonitorServices.get#Incident",
        json!({"incidentId": incident}),
    )["recovery"]["decisions"]
        .as_array()
        .unwrap()
        .clone()
}

fn actions(incident: &Json) -> Vec<Json> {
    call(
        "org.moqui.monitor.MonitorServices.get#Incident",
        json!({"incidentId": incident}),
    )["recovery"]["actions"]
        .as_array()
        .unwrap()
        .clone()
}

/// A deployment controller: the first request fails, later ones are accepted; it keeps every body it was sent.
fn controller() -> (u16, Arc<Mutex<Vec<Json>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Arc<Mutex<Vec<Json>>> = Arc::default();
    let kept = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 4096];
            let body = loop {
                let Ok(read) = stream.read(&mut chunk) else { break None };
                if read == 0 {
                    break None;
                }
                buffer.extend_from_slice(&chunk[..read]);
                let text = String::from_utf8_lossy(&buffer).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if body.len() >= length {
                        break Some(body.to_string());
                    }
                }
            };
            let parsed: Json = body
                .and_then(|body| serde_json::from_str(&body).ok())
                .unwrap_or(Json::Null);
            let first = {
                let mut seen = kept.lock().unwrap();
                seen.push(parsed);
                seen.len() == 1
            };
            let (status, answer) = if first {
                ("500 Internal Server Error", "{}")
            } else {
                ("200 OK", r#"{"status":"accepted","externalId":"deploy-7"}"#)
            };
            let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len());
        }
    });
    (port, seen)
}

#[test]
fn recovery_decides_waits_for_approval_acts_through_its_adapters_and_verifies() {
    if !common::own_process() {
        return;
    }
    let gate = common::start_with_files(
        "monitor-recovery",
        None,
        &[],
        &[
            (
                "component/recovery-fixture/component.xml",
                r#"<component name="recovery-fixture"/>"#,
            ),
            (
                "component/recovery-fixture/MoquiConf.xml",
                r#"<moqui-conf><telemetry><regressions lookback-hours="1" window-minutes="30"/><recovery allow-automatic="true"/></telemetry></moqui-conf>"#,
            ),
        ],
    );
    let factory = gate.server.factory();
    let address = gate.server.address();
    let search = moqui_core::search::installed(factory).unwrap();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let now = jiff::Timestamp::now().as_millisecond();
    let released = now - 40 * MINUTE;

    // shop 2.0 fails on shop-a only; cart 2.0 fails on both its instances.
    let mut metrics = Vec::new();
    for step in 1..=60 {
        for (service, instance) in [
            ("shop", "shop-a"),
            ("shop", "shop-b"),
            ("shop", "shop-c"),
            ("cart", "cart-a"),
            ("cart", "cart-b"),
        ] {
            metrics.extend(minute(service, "1.0", instance, released - step * MINUTE, 199, 1));
        }
    }
    for step in 0..30 {
        let at = now - (step + 1) * MINUTE;
        metrics.extend(minute("shop", "2.0", "shop-a", at, 140, 60));
        metrics.extend(minute("shop", "2.0", "shop-b", at, 200, 0));
        metrics.extend(minute("shop", "2.0", "shop-c", at, 200, 0));
        metrics.extend(minute("cart", "2.0", "cart-a", at, 150, 50));
        metrics.extend(minute("cart", "2.0", "cart-b", at, 150, 50));
    }
    search.index_entries("otel_metrics", metrics).unwrap();
    search
        .index_entries(
            "monitor_annotations",
            vec![
                release("shop", "1.0", released - 5 * 60 * MINUTE),
                release("shop", "2.0", released),
                release("cart", "1.0", released - 5 * 60 * MINUTE),
                release("cart", "2.0", released),
            ],
        )
        .unwrap();
    let pass = call("org.moqui.monitor.MonitorServices.detect#Regressions", json!({}));
    assert_eq!(pass["incidents"], 2, "{pass}");
    let incidents = call("org.moqui.monitor.MonitorServices.list#Incidents", json!({}))["incidents"].clone();
    let incident_of = |service: &str| {
        incidents
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["serviceName"] == service)
            .unwrap()["incidentId"]
            .clone()
    };
    let (shop, cart) = (incident_of("shop"), incident_of("cart"));

    // The shipped policy is advisory and only escalates.
    let run = call("org.moqui.monitor.MonitorServices.run#Recovery", json!({}));
    assert_eq!(run["failed"], json!([]), "{run}");
    let escalated = actions(&shop);
    assert_eq!(
        (escalated[0]["action"].as_str(), escalated[0]["statusId"].as_str()),
        (Some("escalate"), Some("Succeeded")),
        "{escalated:?}"
    );

    // An approval policy for shop: isolate the failing instance. Nothing runs before approval.
    call(
        "org.moqui.monitor.MonitorServices.store#RecoveryPolicy",
        json!({"policyId": "SHOP", "sequenceNum": 1, "serviceName": "shop", "mode": "approval", "adapterId": "IN_PROCESS",
            "ladderJson": r#"[{"action":"isolate-instance"},{"action":"escalate"}]"#, "verifyMinutes": 1, "cooldownMinutes": 5}),
    );
    call("org.moqui.monitor.MonitorServices.run#Recovery", json!({}));
    let proposed = decisions(&shop)[0].clone();
    assert_eq!(
        (
            proposed["action"].as_str(),
            proposed["target"].as_str(),
            proposed["statusId"].as_str()
        ),
        (Some("isolate-instance"), Some("shop-a"), Some("AwaitingApproval")),
        "{proposed}"
    );
    assert_eq!(proposed["blastRadius"], "1 of 3 instances");
    let drained = |instance: &str| {
        let _authz = context.disable_authz();
        factory
            .entity()
            .unwrap()
            .find("moqui.monitor.InstanceDrain")
            .unwrap()
            .eq("instanceId", Value::string(instance))
            .one()
            .unwrap()
            .map(|row| moqui_lib::object::to_plain_string(row.get("drained").unwrap()))
    };
    assert_eq!(drained("shop-a"), None, "nothing runs before approval");
    // A second pass with nothing changed makes no new decision.
    call("org.moqui.monitor.MonitorServices.run#Recovery", json!({}));
    assert_eq!(decisions(&shop)[0]["decisionId"], proposed["decisionId"]);

    call(
        "org.moqui.monitor.MonitorServices.approve#RecoveryDecision",
        json!({"decisionId": proposed["decisionId"]}),
    );
    assert_eq!(drained("shop-a").as_deref(), Some("Y"));
    let isolated = actions(&shop)
        .into_iter()
        .find(|row| row["action"] == "isolate-instance")
        .unwrap();
    assert_eq!(isolated["statusId"], "Succeeded");
    assert_eq!(
        call(
            "org.moqui.monitor.MonitorServices.get#Incident",
            json!({"incidentId": shop})
        )["incident"]["statusId"],
        "Mitigating"
    );

    // Two minutes on, the service is healthy: verified, and the incident is watched.
    let later = now + 3 * MINUTE;
    let mut healthy = Vec::new();
    for step in 0..4 {
        for instance in ["shop-b", "shop-c"] {
            healthy.extend(minute("shop", "2.0", instance, later - step * MINUTE, 200, 0));
        }
    }
    search.index_entries("otel_metrics", healthy).unwrap();
    context.set_effective_time(Some(later + MINUTE));
    call("org.moqui.monitor.MonitorServices.run#Recovery", json!({}));
    let isolated = actions(&shop)
        .into_iter()
        .find(|row| row["action"] == "isolate-instance")
        .unwrap();
    assert_eq!(isolated["verificationStatus"], "Verified", "{isolated}");
    assert_eq!(
        call(
            "org.moqui.monitor.MonitorServices.get#Incident",
            json!({"incidentId": shop})
        )["incident"]["statusId"],
        "Monitoring"
    );
    // Reverting brings the instance back.
    call(
        "org.moqui.monitor.MonitorServices.revert#RecoveryAction",
        json!({"actionId": isolated["actionId"]}),
    );
    assert_eq!(drained("shop-a").as_deref(), Some("N"));
    context.set_effective_time(None);

    // A drained instance is taken out of the load balancer.
    let own = moqui_core::drain::instance_id(factory);
    moqui_core::drain::set_drained(factory, &own, true, "test", None).unwrap();
    let (status, _, body) = common::exchange(address, &common::http_request("GET", "/health", &[], ""));
    assert_eq!((status, body.contains("draining")), (503, true), "{body}");
    moqui_core::drain::set_drained(factory, &own, false, "test", None).unwrap();
    assert_eq!(
        common::exchange(address, &common::http_request("GET", "/health", &[], "")).0,
        200
    );

    // cart: an automatic rollback through a deployment controller, held by the emergency stop first.
    let (port, seen) = controller();
    call(
        "org.moqui.monitor.MonitorServices.store#RecoveryAdapter",
        json!({"adapterId": "DEPLOYER", "adapterType": "webhook", "url": format!("http://127.0.0.1:{port}/deploy"), "allowInternal": "Y"}),
    );
    call(
        "org.moqui.monitor.MonitorServices.store#RecoveryPolicy",
        json!({"policyId": "CART", "sequenceNum": 2, "serviceName": "cart", "mode": "automatic", "adapterId": "DEPLOYER",
            "ladderJson": r#"[{"action":"rollback"},{"action":"escalate","adapterId":"IN_PROCESS"}]"#, "verifyMinutes": 1}),
    );
    call(
        "org.moqui.monitor.MonitorServices.set#RecoveryStop",
        json!({"stopped": "Y", "reason": "change freeze"}),
    );
    call("org.moqui.monitor.MonitorServices.run#Recovery", json!({}));
    let held = decisions(&cart)[0].clone();
    // The default policy escalated the incident already, inside the cooldown: only observing is left.
    assert_eq!(
        (held["action"].as_str(), held["statusId"].as_str()),
        (Some("observe"), Some("Blocked")),
        "{held}"
    );
    assert!(
        held["blockedReason"].as_str().unwrap().contains("emergency stop"),
        "{held}"
    );
    assert!(seen.lock().unwrap().is_empty());
    call(
        "org.moqui.monitor.MonitorServices.set#RecoveryStop",
        json!({"stopped": "N"}),
    );
    call("org.moqui.monitor.MonitorServices.run#Recovery", json!({}));
    let rollback = decisions(&cart)[0].clone();
    assert_eq!(
        (
            rollback["action"].as_str(),
            rollback["target"].as_str(),
            rollback["statusId"].as_str()
        ),
        (Some("rollback"), Some("1.0"), Some("Approved")),
        "{rollback}"
    );
    // The first request failed; the next pass tries again with the same idempotency key.
    let action = actions(&cart)
        .into_iter()
        .find(|row| row["action"] == "rollback")
        .unwrap();
    assert_eq!(action["statusId"], "Persisted", "{action}");
    call("org.moqui.monitor.MonitorServices.run#Recovery", json!({}));
    let action = actions(&cart)
        .into_iter()
        .find(|row| row["action"] == "rollback")
        .unwrap();
    assert_eq!(
        (action["statusId"].as_str(), action["externalId"].as_str()),
        (Some("Running"), Some("deploy-7")),
        "{action}"
    );
    let sent = seen.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(
        sent[0]["idempotencyKey"], sent[1]["idempotencyKey"],
        "one action, however often it is sent"
    );
    assert_eq!(sent[0]["action"], "rollback");
    let token = sent[0]["callbackToken"].as_str().unwrap().to_string();
    assert!(sent[1].get("callbackToken").is_none());

    // The controller reports back on /v1/recovery with an ingest key and the action's token.
    let key = call(
        "org.moqui.monitor.MonitorServices.create#IngestKey",
        json!({"source": "deployer"}),
    );
    let bearer = format!("Bearer {}", key["accessToken"].as_str().unwrap());
    let report = |token: &str| {
        let body = json!({"actionId": action["actionId"], "callbackToken": token, "status": "succeeded", "message": "rolled back"}).to_string();
        common::exchange(
            address,
            &common::http_request(
                "POST",
                "/v1/recovery",
                &[("Authorization", &bearer), ("Content-Type", "application/json")],
                &body,
            ),
        )
        .0
    };
    assert_eq!(report("not-the-token"), 400);
    assert_eq!(report(&token), 200);
    let action = actions(&cart)
        .into_iter()
        .find(|row| row["action"] == "rollback")
        .unwrap();
    assert_eq!(action["statusId"], "Succeeded", "{action}");

    // No telemetry since the rollback: not verified, and after twice the verify time, Inconclusive.
    let effective = jiff::Timestamp::now().as_millisecond();
    context.set_effective_time(Some(effective + 90_000));
    call("org.moqui.monitor.MonitorServices.run#Recovery", json!({}));
    let action = actions(&cart)
        .into_iter()
        .find(|row| row["action"] == "rollback")
        .unwrap();
    assert!(action["verificationStatus"].is_null(), "waits for data: {action}");
    context.set_effective_time(Some(effective + 3 * MINUTE));
    call("org.moqui.monitor.MonitorServices.run#Recovery", json!({}));
    let action = actions(&cart)
        .into_iter()
        .find(|row| row["action"] == "rollback")
        .unwrap();
    assert_eq!(action["verificationStatus"], "Inconclusive", "{action}");
}
