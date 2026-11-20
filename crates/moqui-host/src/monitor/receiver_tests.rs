/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

use super::note_refusal;

/// SEC-TLS-17: a refused ingest key counts as a failed sign-in of its client, so a flood of made-up keys bans it as
/// wrong passwords do; no key, a busy receiver and a rate limit do not count.
#[test]
fn a_refused_ingest_key_is_a_failed_sign_in_of_its_client() {
    let guard = crate::guard::tests::guard(
        r#"<web-protection enabled="true">
                <ban name="auth-failure" events="auth-failure" threshold="3" seconds="600" duration="900"/>
            </web-protection>"#,
        "",
    );
    let client: std::net::IpAddr = "203.0.113.5".parse().unwrap();
    let invalid = (401, "ingest key is invalid".to_string());
    for refusal in [
        (401, "an ingest key is required".to_string()),
        (503, "telemetry receiver is busy".to_string()),
        (429, "ingest key rate limit exceeded".to_string()),
    ] {
        let given = refusal.1 != "an ingest key is required";
        for _ in 0..5 {
            note_refusal(&guard, client, given, &refusal);
        }
    }
    assert!(
        guard.admit(client).is_some(),
        "no key, a busy receiver and a rate limit are not failed sign-ins"
    );
    note_refusal(&guard, client, true, &invalid);
    note_refusal(&guard, client, true, &invalid);
    assert!(guard.admit(client).is_some());
    note_refusal(&guard, client, true, &invalid);
    assert!(guard.admit(client).is_none(), "three refused keys ban the client");
}

/// Vault-Instances §4.8: a record naming a managed instance's `service.instance.id`, on itself or its resource, is
/// taken only under that instance's own ingest key, since Monitor's incidents and its `instance` recovery adapter act
/// on that id; a manager's instance metric is never taken from a sender; any other record passes.
#[test]
fn a_managed_instance_id_is_taken_only_under_its_own_key() {
    use moqui_core::entity::document::Document;
    use moqui_lib::object::Value;
    let document = |pairs: &[(&str, &str)], resource: Option<&str>| {
        let mut document = Document::default();
        for (name, value) in pairs {
            document.insert((*name).to_string(), Value::string(*value));
        }
        if let Some(id) = resource {
            let mut map = Document::default();
            map.insert("service.instance.id".to_string(), Value::string(id));
            document.insert("resource".to_string(), Value::Map(map));
        }
        document
    };
    let managed: std::collections::HashSet<String> = ["app-1".to_string()].into();
    let records = || {
        vec![
            document(
                &[("service.instance.id", "app-1"), ("name", "http.server.duration")],
                None,
            ),
            document(&[("name", "http.server.duration")], Some("app-1")),
            document(
                &[("service.instance.id", "other-9"), ("name", "http.server.duration")],
                None,
            ),
        ]
    };
    // another key: both claims of app-1 refused, the other instance's record taken
    let mut taken = records();
    assert_eq!(
        super::refuse_claims("otel_metrics", &mut taken, &managed, "instance:app-2"),
        2
    );
    assert_eq!(taken.len(), 1);
    let mut taken = records();
    assert_eq!(
        super::refuse_claims("otel_metrics", &mut taken, &managed, "ci-runner"),
        2
    );
    // its own key: all taken
    let mut taken = records();
    assert_eq!(
        super::refuse_claims("otel_metrics", &mut taken, &managed, "instance:app-1"),
        0
    );
    // a manager's instance metric, from any sender, its own instance's key too
    let mut taken = vec![document(&[("name", "moqui.instance.sealed_seconds")], Some("app-1"))];
    assert_eq!(
        super::refuse_claims("otel_metrics", &mut taken, &managed, "instance:app-1"),
        1
    );
    // a span of that name is no metric
    let mut taken = vec![document(&[("name", "moqui.instance.sealed_seconds")], None)];
    assert_eq!(super::refuse_claims("otel_spans", &mut taken, &managed, "x"), 0);
}
