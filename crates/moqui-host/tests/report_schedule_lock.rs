//! Real PostgreSQL schedule claims, restart recovery, revocation, and independent destinations.
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "telemetry"))]
mod common;
use example as _;
use moqui_core::{
    context::ExecutionContextFactory,
    native::{NativeComponent, NativeRegistry, Parameters, ServiceInfo},
};
use moqui_lib::{error::Error, object::Value};
use serde_json::{json, Value as Json};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::Duration;

#[derive(Default)]
struct RenderGate {
    next: Mutex<Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>>,
    calls: AtomicUsize,
}
impl RenderGate {
    fn block_next(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered, waiting) = mpsc::sync_channel(1);
        let (release, blocked) = mpsc::channel();
        *self.next.lock().unwrap() = Some((entered, blocked));
        (waiting, release)
    }
}
fn blocking_rows(
    ec: &moqui_core::context::ExecutionContext,
    _: &ServiceInfo<'_>,
    _: &Parameters,
) -> Result<Parameters, Error> {
    let gate = ec.factory().extension(|_| RenderGate::default());
    let call = gate.calls.fetch_add(1, Ordering::Relaxed) + 1;
    let block = gate.next.lock().unwrap().take();
    if let Some((entered, release)) = block {
        // The data service itself is transaction=ignore, as is the report runner. A
        // blocked renderer must leave the lease connection available to other workers.
        assert!(!ec
            .factory()
            .entity()
            .unwrap()
            .transactions_for("moqui.screen.ScreenScheduledLock")
            .unwrap()
            .in_transaction());
        entered.send(()).unwrap();
        release
            .recv_timeout(Duration::from_secs(15))
            .map_err(|_| Error::new("fixture render release timed out"))?;
    }
    Ok(params(json!({"rows":[{"value":call}]})))
}
fn register(registry: &mut NativeRegistry) -> Result<(), moqui_core::context::Error> {
    registry.service_function("report_lock_fixture::rows", blocking_rows)
}
moqui_core::native::submit_component! { NativeComponent {name:"report_lock_fixture",register} }
fn params(value: Json) -> Parameters {
    let Value::Map(map) = moqui_lib::json::from_json(&value) else {
        panic!("map")
    };
    map.into_iter().collect()
}
fn call(factory: &Arc<ExecutionContextFactory>, name: &str, input: Json, owner: bool) -> Result<Json, String> {
    let ec = factory.new_context();
    let _entered = ec.enter();
    if owner {
        moqui_core::user::internal_login_user("john.doe").map_err(|error| error.to_string())?;
    }
    let output = moqui_core::service::sync(name, &params(input)).map_err(|error| error.to_string())?;
    let errors = ec.message().errors().to_vec();
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(moqui_lib::json::to_json(&Value::Map(output.into_iter().collect())))
}
fn run(factory: &Arc<ExecutionContextFactory>, id: &str) -> Result<Json, String> {
    call(
        factory,
        "org.moqui.report.ReportServices.run#ScheduledReport",
        json!({"screenScheduledId":id}),
        false,
    )
}
fn owner(factory: &Arc<ExecutionContextFactory>, name: &str, input: Json) -> Json {
    call(factory, name, input, true).unwrap_or_else(|error| panic!("{name}: {error}"))
}
fn update(factory: &Arc<ExecutionContextFactory>, table: &str, key: &str, id: &str, fields: Vec<(String, Value)>) {
    let ec = factory.new_context();
    let _entered = ec.enter();
    let _authz = ec.disable_authz();
    let entity = factory.entity().unwrap();
    let tx = entity.transactions_for(table).unwrap();
    tx.run(moqui_core::transaction::TransactionAttribute::ForceNew, None, || {
        entity.find(table)?.eq(key, Value::string(id)).update_all(fields)
    })
    .unwrap();
}
fn lock(factory: &Arc<ExecutionContextFactory>, id: &str) -> moqui_core::entity::EntityValue {
    let ec = factory.new_context();
    let _entered = ec.enter();
    let _authz = ec.disable_authz();
    factory
        .entity()
        .unwrap()
        .find("moqui.screen.ScreenScheduledLock")
        .unwrap()
        .use_cache(false)
        .eq("screenScheduledId", Value::string(id))
        .one()
        .unwrap()
        .unwrap()
}
fn schedule(factory: &Arc<ExecutionContextFactory>, location: &str, template: Option<&str>) -> String {
    owner(factory,"org.moqui.report.ReportServices.create#Schedule",json!({"definitionId":"lease-dashboard","cronExpression":"0 0 0 * * ?","renderMode":"csv","saveToLocation":location,"emailTemplateId":template,"userId":"EX_JOHN_DOE"}))["screenScheduledId"].as_str().unwrap().to_string()
}
#[test]
fn workers_restart_and_revocation_preserve_single_authorized_delivery() {
    if !crate::common::own_process() {
        return;
    }
    let fixture = r#"<entity-facade-xml type="seed">
      <moqui.security.UserAccount userId="EX_JOHN_DOE" emailAddress="lease-recipient@example.test"/>
      <moqui.security.ArtifactGroup artifactGroupId="REPORT_LEASE_FIXTURE"/>
      <moqui.security.ArtifactGroupMember artifactGroupId="REPORT_LEASE_FIXTURE" artifactName="fixture.ReportLock.rows#Fixture" artifactTypeEnumId="AT_SERVICE" inheritAuthz="N"/>
      <moqui.security.ArtifactAuthz artifactAuthzId="REPORT_LEASE_FIXTURE" artifactGroupId="REPORT_LEASE_FIXTURE" userGroupId="ADMIN" authzTypeEnumId="AUTHZT_ALLOW" authzActionEnumId="AUTHZA_ALL"/>
      <moqui.basic.email.EmailServer emailServerId="REPORT_LEASE_CAPTURE" smtpHost="127.0.0.1" smtpPort="25"/>
      <moqui.basic.email.EmailTemplate emailTemplateId="REPORT_LEASE_CAPTURE" emailServerId="REPORT_LEASE_CAPTURE" fromAddress="reports@example.test" subject="Lease report" bodyScreenLocation="component://framework/screen/DashboardReportEmail.xml" webappName="webroot"/>
      <moqui.basic.email.EmailTemplate emailTemplateId="REPORT_LEASE_FAIL_NO_BODY" emailServerId="REPORT_LEASE_CAPTURE" fromAddress="reports@example.test" subject="Intentionally invalid delivery template"/>
    </entity-facade-xml>"#;
    let mut properties = vec![("scheduled_job_check_time", "0")];
    let chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
    if std::path::Path::new(chrome).exists() {
        properties.push(("pdf_chromium_path", chrome));
    }
    let mut gate = common::start_with_files(
        "report-schedule-lock",
        Some(fixture),
        &properties,
        &[
            (
                "component/report_lock_fixture/component.xml",
                r#"<component name="report_lock_fixture" version="1.0"/>"#,
            ),
            (
                "component/report_lock_fixture/service/fixture/ReportLock.xml",
                r#"<services><service verb="rows" noun="Fixture" type="rust" transaction="ignore" location="report_lock_fixture::rows"><out-parameters><parameter name="rows" type="List"/></out-parameters></service></services>"#,
            ),
        ],
    );
    let output = gate.runtime.join("single.csv");
    let live_output = gate.runtime.join("live.csv");
    let expired_output = gate.runtime.join("expired.csv");
    let (live_id, expired_id) = {
        let factory = gate.server.factory();
        owner(
            factory,
            "org.moqui.report.ReportServices.save#Definition",
            json!({"definition":{"id":"lease-dataset","type":"dataset","title":"Lease fixture","body":{"serviceName":"fixture.ReportLock.rows#Fixture"}}}),
        );
        owner(
            factory,
            "org.moqui.report.ReportServices.save#Definition",
            json!({"definition":{"id":"lease-dashboard","type":"dashboard","title":"Lease fixture","body":{"panels":{"data":{"visualization":"table","queries":[{"datasetId":"lease-dataset"}]}}}}}),
        );
        let id = schedule(factory, output.to_str().unwrap(), None);
        let state = factory.extension(|_| RenderGate::default());
        let (entered, release) = state.block_next();
        let worker_factory = Arc::clone(factory);
        let worker_id = id.clone();
        let first = std::thread::spawn(move || run(&worker_factory, &worker_id));
        entered
            .recv_timeout(Duration::from_secs(10))
            .expect("first worker entered render after claiming lease");
        assert!(!output.exists());
        let current = lock(factory, &id);
        assert!(current
            .get("reportRunId")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()));
        let second = run(factory, &id).unwrap();
        assert_eq!(second["skipped"], true);
        assert_eq!(
            state.calls.load(Ordering::Relaxed),
            1,
            "second worker must not query or deliver"
        );
        release.send(()).unwrap();
        assert_eq!(first.join().unwrap().unwrap()["sent"], true);
        assert!(output.exists());
        assert_eq!(
            run(factory, &id).unwrap()["skipped"],
            true,
            "the same cron occurrence is already delivered"
        );
        assert_eq!(state.calls.load(Ordering::Relaxed), 1);
        let live = schedule(factory, live_output.to_str().unwrap(), None);
        let expired = schedule(factory, expired_output.to_str().unwrap(), None);
        let now = jiff::Timestamp::now().as_millisecond();
        for (id, until, token) in [
            (&live, now + 600_000, "restart-live"),
            (&expired, now - 1000, "restart-expired"),
        ] {
            update(
                factory,
                "moqui.screen.ScreenScheduledLock",
                "screenScheduledId",
                id,
                vec![
                    ("reportRunningUntil".into(), Value::I64(until)),
                    ("reportRunId".into(), Value::string(token)),
                ],
            );
        }
        (live, expired)
    };
    gate = gate.restart();
    let factory = gate.server.factory();
    let state = factory.extension(|_| RenderGate::default());
    assert_eq!(run(factory, &live_id).unwrap()["skipped"], true);
    assert!(!live_output.exists());
    assert_eq!(
        lock(factory, &live_id).get("reportRunId").and_then(Value::as_str),
        Some("restart-live")
    );
    assert_eq!(run(factory, &expired_id).unwrap()["sent"], true);
    assert!(expired_output.exists());
    assert_eq!(
        state.calls.load(Ordering::Relaxed),
        1,
        "only the expired persisted lease is recovered after restart"
    );
    let revoked_output = gate.runtime.join("revoked.csv");
    let revoked = schedule(factory, revoked_output.to_str().unwrap(), None);
    let (entered, release) = state.block_next();
    let worker_factory = Arc::clone(factory);
    let worker_id = revoked.clone();
    let worker = std::thread::spawn(move || run(&worker_factory, &worker_id));
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    update(
        factory,
        "moqui.security.UserAccount",
        "userId",
        "EX_JOHN_DOE",
        vec![("disabled".into(), Value::string("Y"))],
    );
    release.send(()).unwrap();
    assert!(
        worker.join().unwrap().is_err(),
        "owner revocation during render blocks delivery"
    );
    assert!(!revoked_output.exists());
    update(
        factory,
        "moqui.security.UserAccount",
        "userId",
        "EX_JOHN_DOE",
        vec![("disabled".into(), Value::string("N"))],
    );
    assert_eq!(
        run(factory, &revoked).unwrap()["sent"],
        true,
        "authorization failure before an effect leaves the occurrence retryable"
    );
    #[cfg(feature = "pdf")]
    if moqui_core::screen::PdfRenderer::available() {
        let capture = moqui_core::service::mail_capture::MailCapture::of(factory);
        capture.set_enabled(Some(true));
        capture.clear();
        let file_fails = schedule(factory, gate.runtime.to_str().unwrap(), Some("REPORT_LEASE_CAPTURE"));
        assert!(run(factory, &file_fails).is_err());
        assert_eq!(
            capture.messages().len(),
            1,
            "a bad file destination must not prevent email delivery"
        );
        let mail = capture.messages().pop().unwrap();
        assert!(mail.body.contains("cid:report-image@moqui"));
        assert_eq!(mail.to, ["lease-recipient@example.test"]);
        let calls = state.calls.load(Ordering::Relaxed);
        assert_eq!(
            run(factory, &file_fails).unwrap()["skipped"],
            true,
            "an attempted destination is not automatically duplicated"
        );
        assert_eq!(state.calls.load(Ordering::Relaxed), calls);
        let file_succeeds = gate.runtime.join("mail-failed.csv");
        let mail_fails = schedule(
            factory,
            file_succeeds.to_str().unwrap(),
            Some("REPORT_LEASE_FAIL_NO_BODY"),
        );
        let failure = run(factory, &mail_fails).unwrap_err();
        assert!(
            failure.contains("email has no body"),
            "expected the configured delivery failure: {failure}"
        );
        assert!(
            file_succeeds.exists(),
            "email failure must not roll back the independent file destination"
        );
        assert_eq!(capture.messages().len(), 1);
    } else {
        eprintln!("report destination email/PNG acceptance not verified: no renderer installed");
    }
}
