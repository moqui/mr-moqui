//! Real alert persistence, concurrency and delivery gates. Every evaluation uses a fresh
//! authenticated context; restart also discards the factory and all in-memory alert state.
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]
mod common;
use example as _;
use moqui_core::{
    context::{ExecutionContext, ExecutionContextFactory},
    entity::document::Document,
    native::{Parameters, ServiceInfo},
    report, search,
    transaction::TransactionAttribute,
};
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};
use std::sync::Arc;

const TOPIC: &str = "M6AlertFixture";
fn document(value: Json) -> Document {
    match moqui_lib::json::from_json(&value) {
        Value::Map(map) => map,
        _ => panic!("object required"),
    }
}
fn parameters(value: Json) -> Parameters {
    document(value).into_iter().collect()
}
fn result(value: Parameters) -> Json {
    moqui_lib::json::to_json(&Value::Map(value.into_iter().collect()))
}
fn service() -> ServiceInfo<'static> {
    ServiceInfo {
        name: "org.moqui.report.ReportServices.test#AlertFixture",
        service_type: "rust",
        location: "test",
        method: "",
    }
}
fn with_owner<T>(factory: &Arc<ExecutionContextFactory>, body: impl FnOnce(&ExecutionContext) -> T) -> T {
    let ec = factory.new_context();
    let _entered = ec.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    body(&ec)
}
fn save(factory: &Arc<ExecutionContextFactory>, id: &str, body: &Json) {
    with_owner(factory, |ec| {
        let entity = factory.entity().unwrap();
        let tx = entity.transactions_for("moqui.report.Definition").unwrap();
        tx.run(TransactionAttribute::UseOrBegin, None, || {
            report::save_definition(
                ec,
                &service(),
                &parameters(json!({"definition":{"id":id,"type":"alert","title":id,"body":body}})),
            )
        })
        .unwrap();
    });
}
fn evaluate(factory: &Arc<ExecutionContextFactory>, id: &str, clock: i64) -> Json {
    with_owner(factory, |ec| {
        ec.set_effective_time(Some(clock));
        result(report::evaluate_rules(ec, &service(), &parameters(json!({"definitionId":id}))).unwrap())
    })
}
fn concurrent(factory: &Arc<ExecutionContextFactory>, id: &str, clock: i64) {
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let threads = (0..4)
        .map(|_| {
            let factory = factory.clone();
            let id = id.to_owned();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                evaluate(&factory, &id, clock)
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        assert_eq!(thread.join().unwrap()["failed"], 0);
    }
}
fn rows(factory: &Arc<ExecutionContextFactory>, entity_name: &str, rule: Option<&str>) -> Vec<Json> {
    with_owner(factory, |ec| {
        let _inspection = ec.disable_authz();
        let entity = factory.entity().unwrap();
        let mut find = entity.find(entity_name).unwrap();
        if let Some(rule) = rule {
            find = find.eq("definitionId", Value::string(rule));
        }
        find.list()
            .unwrap()
            .iter()
            .map(|row| {
                Json::Object(
                    row.fields()
                        .iter()
                        .map(|(name, value)| (name.to_string(), moqui_lib::json::to_json(value)))
                        .collect(),
                )
            })
            .collect()
    })
}
fn states(factory: &Arc<ExecutionContextFactory>, rule: &str) -> Vec<String> {
    rows(factory, "moqui.report.AlertInstance", Some(rule))
        .iter()
        .map(|row| row["state"].as_str().unwrap().to_owned())
        .collect()
}
fn notification_count(factory: &Arc<ExecutionContextFactory>, rule: &str) -> usize {
    with_owner(factory, |ec| {
        let _inspection = ec.disable_authz();
        let entity = factory.entity().unwrap();
        entity
            .find("moqui.security.user.NotificationMessage")
            .unwrap()
            .eq("topic", Value::string(TOPIC))
            .list()
            .unwrap()
            .iter()
            .filter(|row| {
                row.get("messageJson")
                    .and_then(Value::as_str)
                    .and_then(|body| serde_json::from_str::<Json>(body).ok())
                    .is_some_and(|body| body["ruleId"] == rule)
            })
            .count()
    })
}
fn points(factory: &Arc<ExecutionContextFactory>, at: i64, value: f64) {
    let facade = search::installed(factory).unwrap();
    let docs=["one","two"].into_iter().map(|series|document(json!({"_id":format!("m6-{series}"),"bucket_start":at,"name":"m6-latency","service.name":series,"granularity":"minute","sum":value,"count":1}))).collect();
    assert_eq!(facade.index_entries("otel_metrics", docs).unwrap(), 2);
}
fn rule(at: i64) -> Json {
    json!({"query":{"datasetId":"otel_metrics","from":(at-86_400_000).to_string(),"thru":(at+30*86_400_000_i64).to_string(),"queryString":"name:m6-latency","aggs":{"service":{"terms":{"field":"service.name","size":20},"aggs":{"value":{"sum":{"field":"sum"}}}}}},"field":"value","labelFields":["service"],"labels":{"team":"operations"},"condition":{"type":"above","threshold":10,"recoveryThreshold":7},"pendingSeconds":120,"keepFiringSeconds":120,"intervalSeconds":60,"contactPoints":[{"id":"header","type":"notification","topic":TOPIC,"userId":"EX_JOHN_DOE","groupBy":["team"],"groupWaitSeconds":120,"repeatSeconds":180}]})
}

#[test]
fn alerts_persist_timers_serialize_workers_and_route_only_complete_observations() {
    moqui_host::logging::install();
    let fixture = r#"<entity-facade-xml type="seed">
      <moqui.service.job.ServiceJob jobName="evaluate_ReportAlerts_minute" paused="Y"/>
      <moqui.security.user.NotificationTopic topic="M6AlertFixture" description="Alert acceptance" isPrivate="Y" receiveNotifications="Y" persistOnSend="Y"/>
      <moqui.security.user.NotificationTopicUser topic="M6AlertFixture" userId="EX_JOHN_DOE" receiveNotifications="Y"/>
    </entity-facade-xml>"#;
    let mut gate = common::start_with_data("report-alerts", fixture);
    let at = "2026-10-26T12:00:00Z"
        .parse::<jiff::Timestamp>()
        .unwrap()
        .as_millisecond();
    let body = rule(at);
    let id = "persisted-alert";
    points(gate.server.factory(), at, 12.0);
    save(gate.server.factory(), id, &body);
    concurrent(gate.server.factory(), id, at);
    assert_eq!(states(gate.server.factory(), id), ["Pending", "Pending"]);
    assert_eq!(notification_count(gate.server.factory(), id), 0);
    assert_eq!(
        rows(gate.server.factory(), "moqui.report.AlertNotification", Some(id)).len(),
        1,
        "two series are one persisted notification group"
    );
    evaluate(gate.server.factory(), id, at + 60_000);
    let pending = rows(gate.server.factory(), "moqui.report.AlertInstance", Some(id));
    let notification = rows(gate.server.factory(), "moqui.report.AlertNotification", Some(id));
    let old_factory = Arc::downgrade(gate.server.factory());
    gate = gate.restart();
    assert!(
        old_factory.upgrade().is_none(),
        "restart must release the original factory"
    );
    assert_eq!(
        rows(gate.server.factory(), "moqui.report.AlertInstance", Some(id)),
        pending
    );
    assert_eq!(
        rows(gate.server.factory(), "moqui.report.AlertNotification", Some(id)),
        notification
    );
    concurrent(gate.server.factory(), id, at + 120_000);
    assert_eq!(states(gate.server.factory(), id), ["Firing", "Firing"]);
    evaluate(gate.server.factory(), id, at + 180_000);
    assert_eq!(
        notification_count(gate.server.factory(), id),
        0,
        "group wait survives restart and does not send early"
    );
    concurrent(gate.server.factory(), id, at + 240_000);
    assert_eq!(
        notification_count(gate.server.factory(), id),
        1,
        "concurrent evaluators send one grouped notification"
    );
    points(gate.server.factory(), at, 8.0);
    evaluate(gate.server.factory(), id, at + 300_000);
    assert_eq!(
        states(gate.server.factory(), id),
        ["Firing", "Firing"],
        "recovery threshold retains firing between 7 and 10"
    );
    assert_eq!(
        notification_count(gate.server.factory(), id),
        1,
        "repeat interval is persisted"
    );
    concurrent(gate.server.factory(), id, at + 420_000);
    assert_eq!(notification_count(gate.server.factory(), id), 2);
    points(gate.server.factory(), at, 6.0);
    evaluate(gate.server.factory(), id, at + 480_000);
    assert_eq!(states(gate.server.factory(), id), ["Recovering", "Recovering"]);
    evaluate(gate.server.factory(), id, at + 540_000);
    assert_eq!(states(gate.server.factory(), id), ["Recovering", "Recovering"]);
    concurrent(gate.server.factory(), id, at + 600_000);
    assert_eq!(states(gate.server.factory(), id), ["Normal", "Normal"]);
    assert_eq!(
        notification_count(gate.server.factory(), id),
        3,
        "resolution sends once without waiting for the firing repeat interval"
    );
    let history = search::installed(gate.server.factory())
        .unwrap()
        .search(
            &search::Target::Index("monitor_annotations".into()),
            &search::SearchRequest::new(search::Query::eq("rule_id", Value::string(id))),
        )
        .unwrap();
    assert_eq!(
        history.total, 8,
        "one transition annotation per instance, never duplicated by workers"
    );

    points(gate.server.factory(), at, 12.0);
    for (rule_id, policy) in [
        (
            "silenced-alert",
            json!({"silences":[{"from":(at-1000).to_string(),"thru":(at+86_400_000).to_string(),"matchers":{"team":"operations"}}]}),
        ),
        (
            "muted-alert",
            json!({"muteTimings":[{"weekdays":[0,1,2,3,4,5,6],"start":"00:00","end":"24:00"}]}),
        ),
        ("unmatched-alert", json!({"contactMatchers":{"team":"other-team"}})),
    ] {
        let mut muted = body.clone();
        muted["pendingSeconds"] = json!(0);
        muted["contactPoints"][0]["groupWaitSeconds"] = json!(0);
        for (key, value) in policy.as_object().unwrap() {
            if key == "contactMatchers" {
                muted["contactPoints"][0]["matchers"] = value.clone();
            } else {
                muted[key] = value.clone();
            }
        }
        save(gate.server.factory(), rule_id, &muted);
        evaluate(gate.server.factory(), rule_id, at);
        assert_eq!(states(gate.server.factory(), rule_id), ["Firing", "Firing"]);
        assert_eq!(
            notification_count(gate.server.factory(), rule_id),
            0,
            "{rule_id} must retain state while suppressing delivery"
        );
        assert!(rows(gate.server.factory(), "moqui.report.AlertNotification", Some(rule_id)).is_empty());
    }
    for (rule_id, query, state) in [
        ("error-alert", json!({"queryString":"("}), "Error"),
        ("incomplete-alert", json!({"maxBuckets":1}), "Error"),
        (
            "missing-alert",
            json!({"queryString":"name:no-matching-metric"}),
            "NoData",
        ),
    ] {
        let mut bad = body.clone();
        bad["contactPoints"] = json!([]);
        for (key, value) in query.as_object().unwrap() {
            bad["query"][key] = value.clone();
        }
        if rule_id == "incomplete-alert" {
            with_owner(gate.server.factory(), |ec| {
                let result = report::execute_query(ec, &bad["query"]).unwrap();
                assert!(
                    result["notices"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|notice| notice["complete"] == false),
                    "backend cap must produce an explicit incomplete result"
                );
            });
        }
        save(gate.server.factory(), rule_id, &bad);
        evaluate(gate.server.factory(), rule_id, at);
        assert_eq!(states(gate.server.factory(), rule_id), [state]);
    }
}

// The wire fixture deliberately keeps production SSRF enforcement intact. Its adapter
// accepts only authored HTTPS fixture URLs with deny_internal=true, then routes those
// requests through the real HTTP client to a loopback listener owned by this test.
const WIRE_ORIGIN: &str = "https://alerts.example.test";
const WIRE_URL_PROPERTY: &str = "MONITOR_WEBHOOK_M6_WIRE_URL";
const WIRE_AUTH_PROPERTY: &str = "MONITOR_WEBHOOK_M6_WIRE_AUTH";
const WIRE_MISSING_PROPERTY: &str = "MONITOR_WEBHOOK_M6_WIRE_MISSING";

#[derive(Clone)]
struct HttpWire {
    path: String,
    headers: std::collections::BTreeMap<String, String>,
    body: Json,
    status: u16,
}
#[derive(Default)]
struct WireObservations {
    http: std::sync::Mutex<Vec<HttpWire>>,
    smtp_commands: std::sync::Mutex<Vec<String>>,
    smtp_messages: std::sync::Mutex<Vec<String>>,
    smtp_attempts: std::sync::atomic::AtomicUsize,
    smtp_failures: std::sync::atomic::AtomicUsize,
    http_failures: std::sync::atomic::AtomicUsize,
}
struct WireListener {
    address: std::net::SocketAddr,
    stop: Arc<std::sync::atomic::AtomicBool>,
    failures: Arc<std::sync::Mutex<Vec<String>>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl WireListener {
    fn start(handler: impl Fn(std::net::TcpStream) -> std::io::Result<()> + Send + 'static) -> Self {
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let failures = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stopping = stop.clone();
        let errors = failures.clone();
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // BSD may inherit the listener's O_NONBLOCK on accepted sockets.
                        // Protocol conversations use bounded blocking reads.
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                        let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
                        if let Err(error) = handler(stream) {
                            errors.lock().unwrap().push(error.to_string());
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(2))
                    }
                    Err(error) => {
                        errors.lock().unwrap().push(error.to_string());
                        break;
                    }
                }
            }
        });
        Self {
            address,
            stop,
            failures,
            worker: Some(worker),
        }
    }
    fn assert_healthy(&self) {
        let failures = self.failures.lock().unwrap();
        assert!(failures.is_empty(), "wire listener failures: {:?}", failures);
    }
}
impl Drop for WireListener {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[allow(deprecated)] // Keep the workspace Rust 1.95 minimum.
fn transient(counter: &std::sync::atomic::AtomicUsize) -> bool {
    counter
        .fetch_update(
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
            |remaining| remaining.checked_sub(1),
        )
        .is_ok()
}
fn smtp_wire(stream: std::net::TcpStream, observed: &WireObservations) -> std::io::Result<()> {
    use std::io::{BufRead, Write};
    use std::sync::atomic::Ordering;
    let mut reader = std::io::BufReader::new(stream.try_clone()?);
    let mut stream = stream;
    stream.write_all(b"220 m6.fixture ESMTP\r\n")?;
    loop {
        let mut command = String::new();
        if reader.read_line(&mut command)? == 0 {
            return Ok(());
        }
        let command = command.trim_end_matches(['\r', '\n']);
        observed.smtp_commands.lock().unwrap().push(command.to_owned());
        let verb = command.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
        match verb.as_str() {
            "EHLO" | "HELO" => stream.write_all(b"250-m6.fixture\r\n250 AUTH PLAIN\r\n")?,
            "AUTH" => stream.write_all(b"235 2.7.0 authenticated\r\n")?,
            "MAIL" => {
                observed.smtp_attempts.fetch_add(1, Ordering::AcqRel);
                stream.write_all(b"250 2.1.0 sender accepted\r\n")?;
            }
            "RCPT" if transient(&observed.smtp_failures) => {
                stream.write_all(b"451 4.3.0 temporary fixture mailbox failure\r\n")?
            }
            "RCPT" => stream.write_all(b"250 2.1.5 recipient accepted\r\n")?,
            "DATA" => {
                stream.write_all(b"354 End data with <CRLF>.<CRLF>\r\n")?;
                let mut body = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line)? == 0 {
                        return Err(std::io::Error::other("SMTP message ended before its terminator"));
                    }
                    if line == ".\r\n" || line == ".\n" {
                        break;
                    }
                    body.push_str(line.strip_prefix('.').unwrap_or(&line));
                    if body.len() > 2_000_000 {
                        return Err(std::io::Error::other("SMTP fixture body exceeds its limit"));
                    }
                }
                observed.smtp_messages.lock().unwrap().push(body);
                stream.write_all(b"250 2.0.0 fixture accepted\r\n")?;
            }
            "QUIT" => {
                stream.write_all(b"221 2.0.0 bye\r\n")?;
                return Ok(());
            }
            "RSET" | "NOOP" => stream.write_all(b"250 2.0.0 ok\r\n")?,
            _ => return Err(std::io::Error::other(format!("unexpected SMTP command {verb}"))),
        }
    }
}
fn http_wire(stream: std::net::TcpStream, observed: &WireObservations) -> std::io::Result<()> {
    use std::io::{BufRead, Read, Write};
    let mut reader = std::io::BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
    let mut headers = std::collections::BTreeMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err(std::io::Error::other("HTTP fixture headers ended early"));
        }
        if line == "\r\n" {
            break;
        }
        let Some((key, value)) = line.trim_end().split_once(':') else {
            return Err(std::io::Error::other("invalid HTTP fixture header"));
        };
        headers.insert(key.to_ascii_lowercase(), value.trim().to_owned());
        if headers.len() > 100 {
            return Err(std::io::Error::other("HTTP fixture header limit"));
        }
    }
    let length: usize = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    if length > 2_000_000 {
        return Err(std::io::Error::other("HTTP fixture body limit"));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    let body: Json = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
    let status = if path.starts_with("/primary") && transient(&observed.http_failures) {
        503
    } else {
        200
    };
    observed.http.lock().unwrap().push(HttpWire {
        path,
        headers,
        body,
        status,
    });
    let mut stream = stream;
    write!(stream,"HTTP/1.1 {status} fixture\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}")
}
struct RoutedWebhook {
    address: std::net::SocketAddr,
    client: moqui_lib::http::RestClient,
}
impl moqui_core::outbound::HttpSender for RoutedWebhook {
    fn send(
        &self,
        request: moqui_core::outbound::Request,
    ) -> Result<moqui_core::outbound::Response, moqui_lib::error::Error> {
        assert_eq!(request.method, moqui_core::outbound::Method::Post);
        assert!(
            request.deny_internal,
            "production alert webhook must request strict SSRF protection"
        );
        let path = request
            .url
            .strip_prefix(WIRE_ORIGIN)
            .expect("only the authored public HTTPS fixture origin may be routed");
        assert!(matches!(path, "/primary" | "/primary-v2" | "/healthy" | "/missing"));
        let mut call = self
            .client
            .request(moqui_lib::http::Method::Post, format!("http://{}{path}", self.address));
        for (key, value) in request.headers {
            call = call.header(key, value);
        }
        if let Some(body) = request.body {
            call = call.text("application/json", body);
        }
        if let Some(timeout) = request.timeout {
            call = call.timeout(timeout);
        }
        let response = call.send(false)?;
        Ok(moqui_core::outbound::Response {
            status: response.status(),
            body: response.body().into(),
            bytes: response.bytes().to_vec(),
            headers: response.response_headers().to_vec(),
        })
    }
}
struct WireProperties(Vec<(&'static str, Option<String>)>);
impl WireProperties {
    fn new() -> Self {
        Self(
            [WIRE_URL_PROPERTY, WIRE_AUTH_PROPERTY, WIRE_MISSING_PROPERTY]
                .into_iter()
                .map(|key| (key, moqui_lib::env::get_property(key)))
                .collect(),
        )
    }
}
impl Drop for WireProperties {
    fn drop(&mut self) {
        moqui_core::outbound::clear_http_sender();
        for (key, value) in &self.0 {
            if let Some(value) = value {
                moqui_lib::env::set_property(key, value);
            } else {
                moqui_lib::env::clear_property(key);
            }
        }
    }
}
fn contact_sent(factory: &Arc<ExecutionContextFactory>, rule: &str, contact: &str) -> bool {
    rows(factory, "moqui.report.AlertNotification", Some(rule))
        .iter()
        .find(|row| row["contactId"] == contact)
        .and_then(|row| row.get("lastSent"))
        .is_some_and(|value| !value.is_null() && value.as_i64() != Some(0))
}

#[test]
fn alerts_deliver_real_smtp_and_webhooks_retry_failures_and_resolve_live_credentials() {
    use std::sync::atomic::Ordering;
    moqui_host::logging::install();
    let observed = Arc::new(WireObservations::default());
    observed.smtp_failures.store(1, Ordering::Release);
    observed.http_failures.store(1, Ordering::Release);
    let smtp = WireListener::start({
        let observed = observed.clone();
        move |stream| smtp_wire(stream, &observed)
    });
    let http = WireListener::start({
        let observed = observed.clone();
        move |stream| http_wire(stream, &observed)
    });
    let fixture = format!(
        r#"<entity-facade-xml type="seed">
        <moqui.service.job.ServiceJob jobName="evaluate_ReportAlerts_minute" paused="Y"/>
        <moqui.security.UserAccount userId="M6_WIRE_CONTROL" username="m6.wire.control" userFullName="M6 fixture administrator" disabled="N"/>
        <moqui.security.UserGroupMember userGroupId="ADMIN" userId="M6_WIRE_CONTROL" fromDate="0"/>
        <moqui.basic.email.EmailServer emailServerId="M6_WIRE_SMTP" smtpHost="127.0.0.1" smtpPort="{}" smtpStartTls="N" smtpSsl="N" mailUsername="m6-fixture-user" mailPassword="m6-fixture-password" allowedToDomains="example.test"/>
        <moqui.basic.email.EmailTemplate emailTemplateId="M6_WIRE_EMAIL" emailServerId="M6_WIRE_SMTP" fromAddress="alerts@example.test" subject="M6 wire alert" bodyScreenLocation="component://m6_wire/screen/AlertEmail.xml" webappName="webroot"/>
    </entity-facade-xml>"#,
        smtp.address.port()
    );
    let mut gate = common::start_with_files(
        "report-alert-wire",
        Some(&fixture),
        &[("scheduled_job_check_time", "0")],
        &[
            (
                "component/m6_wire/component.xml",
                r#"<component name="m6_wire" version="1.0"/>"#,
            ),
            (
                "component/m6_wire/screen/AlertEmail.xml",
                r#"<screen xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="http://moqui.org/xsd/xml-screen-3.xsd"><parameter name="title"/><parameter name="ruleId"/><widgets><render-mode><text type="html,html-print"><![CDATA[<html><body><p>Alert ${html(title)} / ${html(ruleId)}</p></body></html>]]></text><text type="text"><![CDATA[Alert ${title} / ${ruleId}]]></text></render-mode></widgets></screen>"#,
            ),
        ],
    );
    let _properties = WireProperties::new();
    moqui_lib::env::set_property(WIRE_URL_PROPERTY, format!("{WIRE_ORIGIN}/primary"));
    moqui_lib::env::set_property(WIRE_AUTH_PROPERTY, "Bearer m6-phase-one");
    moqui_lib::env::set_property(WIRE_MISSING_PROPERTY, "");
    moqui_core::outbound::install_http_sender(Arc::new(RoutedWebhook {
        address: http.address,
        client: moqui_lib::http::RestClient::new(),
    }));
    // The real default client still refuses loopback when strict policy is requested.
    assert!(moqui_lib::http::RestClient::new()
        .deny_internal_addresses(true)
        .request(
            moqui_lib::http::Method::Post,
            format!("http://{}/must-not-connect", http.address)
        )
        .json(&json!({}))
        .send(false)
        .is_err());
    let at = "2026-10-26T12:00:00Z"
        .parse::<jiff::Timestamp>()
        .unwrap()
        .as_millisecond();
    let id = "wire-alert";
    {
        let factory = gate.server.factory();
        moqui_core::service::mail_capture::MailCapture::of(factory).set_enabled(Some(false));
        points(factory, at, 12.0);
        let mut body = rule(at);
        body["pendingSeconds"] = json!(0);
        body["keepFiringSeconds"] = json!(0);
        body["contactPoints"] = json!([
            {"id":"smtp","type":"email","emailTemplateId":"M6_WIRE_EMAIL","toAddresses":"wire-recipient@example.test","groupBy":["team"],"groupWaitSeconds":0,"repeatSeconds":3600},
            {"id":"primary","type":"webhook","urlProperty":WIRE_URL_PROPERTY,"headerProperties":{"Authorization":WIRE_AUTH_PROPERTY},"groupBy":["team"],"groupWaitSeconds":0,"repeatSeconds":3600},
            {"id":"healthy","type":"webhook","url":format!("{WIRE_ORIGIN}/healthy"),"headerProperties":{"Authorization":WIRE_AUTH_PROPERTY},"groupBy":["team"],"groupWaitSeconds":0,"repeatSeconds":3600},
            {"id":"missing","type":"webhook","url":format!("{WIRE_ORIGIN}/missing"),"headerProperties":{"Authorization":WIRE_MISSING_PROPERTY},"groupBy":["team"],"groupWaitSeconds":0,"repeatSeconds":3600},
        ]);
        with_owner(factory, |ec| {
            let entity = factory.entity().unwrap();
            entity
                .transactions_for("moqui.report.Definition")
                .unwrap()
                .run(TransactionAttribute::UseOrBegin, None, || {
                    report::save_definition(
                        ec,
                        &service(),
                        &parameters(
                            json!({"definition":{"id":id,"type":"alert","title":id,"userGroupId":"ADMIN","body":body}}),
                        ),
                    )
                })
                .unwrap();
        });
        assert_eq!(evaluate(factory, id, at)["failed"], 0);
        assert_eq!(states(factory, id), ["Firing", "Firing"]);
        smtp.assert_healthy();
        assert_eq!(observed.smtp_attempts.load(Ordering::Acquire), 1, "SMTP commands: {:?}", observed.smtp_commands.lock().unwrap());
        assert!(observed.smtp_messages.lock().unwrap().is_empty());
        assert!(
            !contact_sent(factory, id, "smtp"),
            "SMTP451 must not establish successful notification state"
        );
        assert!(!contact_sent(factory, id, "primary"), "HTTP503 must remain retryable");
        assert!(
            !contact_sent(factory, id, "missing"),
            "missing server credential must remain retryable"
        );
        assert!(
            contact_sent(factory, id, "healthy"),
            "independent contact continues after named failures"
        );
        let requests = observed.http.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].status, 503);
        assert_eq!(requests[1].status, 200);
        assert_eq!(requests[0].headers["authorization"], "Bearer m6-phase-one");
        for request in requests.iter() {
            assert_eq!(request.body["ruleId"], id);
            assert_eq!(request.body["events"].as_array().unwrap().len(), 2);
            assert_eq!(request.body["labels"]["team"], "operations");
        }
    }
    let old = Arc::downgrade(gate.server.factory());
    gate = gate.restart();
    assert!(old.upgrade().is_none());
    moqui_core::service::mail_capture::MailCapture::of(gate.server.factory()).set_enabled(Some(false));
    moqui_lib::env::set_property(WIRE_URL_PROPERTY, format!("{WIRE_ORIGIN}/primary-v2"));
    moqui_lib::env::set_property(WIRE_AUTH_PROPERTY, "Bearer m6-phase-two");
    moqui_lib::env::set_property(WIRE_MISSING_PROPERTY, "Bearer m6-missing-now-configured");
    assert_eq!(evaluate(gate.server.factory(), id, at + 60_000)["failed"], 0);
    for contact in ["smtp", "primary", "healthy", "missing"] {
        assert!(
            contact_sent(gate.server.factory(), id, contact),
            "retry after restart: {contact}"
        );
    }
    assert_eq!(observed.smtp_attempts.load(Ordering::Acquire), 2);
    {
        let messages = observed.smtp_messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        let wire = &messages[0];
        assert!(
            wire.contains("wire-recipient@example.test")
                && wire.contains("alerts@example.test")
                && wire.contains("M6 wire alert")
                && wire.contains(id),
            "actual SMTP MIME must contain the alert and recipient"
        );
        let commands = observed.smtp_commands.lock().unwrap();
        use base64::Engine as _;
        let authentication = commands
            .iter()
            .find_map(|command| command.strip_prefix("AUTH PLAIN "))
            .expect("real SMTP authentication exchange");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(authentication)
                .unwrap(),
            b"\0m6-fixture-user\0m6-fixture-password"
        );
        assert!(commands.iter().any(|command| command.starts_with("MAIL FROM:")));
        assert!(commands.iter().any(|command| command.starts_with("RCPT TO:")));
        let requests = observed.http.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[2].path, "/primary-v2");
        assert_eq!(requests[2].headers["authorization"], "Bearer m6-phase-two");
        assert_eq!(requests[3].path, "/missing");
        assert_eq!(requests[3].headers["authorization"], "Bearer m6-missing-now-configured");
        assert_eq!(
            requests.iter().filter(|request| request.path == "/healthy").count(),
            1,
            "successful contacts do not duplicate on failed-contact retries"
        );
    }
    moqui_lib::env::set_property(WIRE_AUTH_PROPERTY, "Bearer m6-phase-three");
    moqui_lib::env::set_property(WIRE_MISSING_PROPERTY, "");
    assert_eq!(evaluate(gate.server.factory(), id, at + 3_660_000)["failed"], 0);
    assert_eq!(observed.smtp_messages.lock().unwrap().len(), 2);
    {
        let requests = observed.http.lock().unwrap();
        assert_eq!(requests.len(), 6);
        assert_eq!(requests[4].headers["authorization"], "Bearer m6-phase-three");
        assert_eq!(requests[5].path, "/healthy");
        assert_eq!(
            requests.iter().filter(|request| request.path == "/missing").count(),
            1,
            "revoked server property prevents another HTTP send while independent contacts continue"
        );
    }
    moqui_lib::env::set_property(WIRE_MISSING_PROPERTY, "Bearer m6-phase-four");
    assert_eq!(evaluate(gate.server.factory(), id, at + 3_720_000)["failed"], 0);
    assert_eq!(
        observed.smtp_messages.lock().unwrap().len(),
        2,
        "successful SMTP contact is not duplicated by another destination retry"
    );
    {
        let requests = observed.http.lock().unwrap();
        assert_eq!(requests.len(), 7);
        assert_eq!(requests[6].path, "/missing");
        assert_eq!(requests[6].headers["authorization"], "Bearer m6-phase-four");
        assert!(
            requests
                .iter()
                .all(|request| !request.body.to_string().contains("Bearer m6-")),
            "credentials belong only in transport headers"
        );
    }
    // Revoking the definition owner blocks every transport even for an administrator.
    with_owner(gate.server.factory(), |ec| {
        let _authz = ec.disable_authz();
        let entity = gate.server.factory().entity().unwrap();
        entity
            .transactions_for("moqui.security.UserAccount")
            .unwrap()
            .run(TransactionAttribute::ForceNew, None, || {
                entity
                    .find("moqui.security.UserAccount")?
                    .eq("userId", Value::string("EX_JOHN_DOE"))
                    .update_all([("disabled".into(), Value::string("Y"))])
            })
            .unwrap();
    });
    {
        let ec = gate.server.factory().new_context();
        let _entered = ec.enter();
        moqui_core::user::internal_login_user("m6.wire.control").unwrap();
        ec.set_effective_time(Some(at + 7_200_000));
        let result = result(report::evaluate_rules(&ec, &service(), &parameters(json!({"definitionId":id}))).unwrap());
        assert_eq!(result["failed"], 1);
        assert_eq!(result["evaluated"], 0);
    }
    assert_eq!(observed.smtp_messages.lock().unwrap().len(), 2);
    assert_eq!(observed.http.lock().unwrap().len(), 7);
    smtp.assert_healthy();
    http.assert_healthy();
}
