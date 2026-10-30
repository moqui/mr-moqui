use super::*;
use crate::telemetry::ingest::Decoder;
use moqui_lib::object::Value;

fn identity() -> BrowserIdentity<'static> {
    BrowserIdentity {
        service_name: "moqui-browser",
        user_id: "EX_JOHN_DOE",
        visit_id: "VISIT_1",
    }
}

#[test]
fn a_page_cannot_pose_as_a_server_another_user_or_another_service() {
    let mut request = json!({"resourceSpans":[{"resource":{"attributes":[
        {"key":"service.name","value":{"stringValue":"billing"}},
        {"key":"user.id","value":{"stringValue":"ADMIN"}},
        {"key":"browser.platform","value":{"stringValue":"Linux"}}
    ]},"scopeSpans":[{"spans":[
        {"traceId":"6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a","spanId":"1111111111111111","name":"documentLoad","kind":1,
         "startTimeUnixNano":"1000000000","endTimeUnixNano":"2000000000",
         "attributes":[{"key":"user.id","value":{"stringValue":"ADMIN"}},{"key":"url.full","value":{"stringValue":"/apps/x"}}]},
        {"traceId":"6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a","spanId":"2222222222222222","name":"GET","kind":"SPAN_KIND_CLIENT",
         "startTimeUnixNano":"1000000000","endTimeUnixNano":"1500000000"},
        {"traceId":"6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a","spanId":"3333333333333333","name":"pretend server","kind":2,
         "startTimeUnixNano":"1000000000","endTimeUnixNano":"1500000000"}
    ]}]}]});
    assert_eq!(scrub(Signal::Traces, &mut request, &identity(), 100), 1);
    let decoded = Decoder::default().decode_value(Signal::Traces, &request, "browser", "browser");
    assert_eq!(decoded.rejected, 0);
    assert_eq!(decoded.documents.len(), 2);
    for document in &decoded.documents {
        assert_eq!(document.get("service.name"), Some(&Value::string("moqui-browser")));
        assert_eq!(document.get("user.id"), Some(&Value::string("EX_JOHN_DOE")));
        assert_eq!(document.get("moqui.visit.id"), Some(&Value::string("VISIT_1")));
        assert_ne!(document.get("kind"), Some(&Value::string("SERVER")));
    }
    let resource = format!("{:?}", decoded.documents[0].get("resource"));
    assert!(resource.contains("browser.platform"), "{resource}");
    assert!(!resource.contains("billing"), "{resource}");
}

#[test]
fn a_batch_keeps_at_most_its_record_limit() {
    let log = |body: &str| json!({"timeUnixNano":"1000000000","severityNumber":17,"body":{"stringValue":body}});
    let mut request = json!({"resourceLogs":[{"scopeLogs":[{"logRecords":[log("a"),log("b"),log("c")]}]}]});
    assert_eq!(scrub(Signal::Logs, &mut request, &identity(), 2), 1);
    let decoded = Decoder::default().decode_value(Signal::Logs, &request, "browser", "browser");
    assert_eq!(decoded.documents.len(), 2);
    assert_eq!(decoded.documents[0].get("user.id"), Some(&Value::string("EX_JOHN_DOE")));
}
