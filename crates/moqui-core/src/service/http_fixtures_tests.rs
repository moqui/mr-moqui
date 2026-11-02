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

// The fixtures are process-wide, and other tests of this binary send HTTP, so these tests use their own HttpFixtures
// value and never turn the process's on; the Mantle spec GatewayContracts runs them through `outbound` in `moqui test`.

use super::*;
use crate::context::ExecutionContextFactory;
use crate::outbound::Method;

fn request(method: Method, url: &str, body: Option<&str>) -> Request {
    Request {
        method,
        url: url.to_string(),
        headers: vec![
            ("Authorization".to_string(), "Bearer secret".to_string()),
            ("X-Api-Key".to_string(), "key".to_string()),
            ("Idempotency-Key".to_string(), "mq1".to_string()),
        ],
        body: body.map(str::to_string),
        timeout: None,
        deny_internal: false,
    }
}

fn fixture(pairs: &[(&str, Value)]) -> Fixture {
    let map = pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.clone()))
        .collect();
    fixture_of(&Value::Map(map)).unwrap()
}

#[test]
fn fixtures_answer_in_order_record_redacted_and_refuse_the_rest() {
    let fixtures = HttpFixtures::default();
    let base = "https://gateway.test/api";
    // a lost answer once, then the answer; a prefix for any other path, only for a body that names it
    fixtures.add(fixture(&[
        ("method", Value::string("post")),
        ("url", Value::string(base)),
        ("error", Value::string("operation timed out")),
        ("times", Value::I64(1)),
    ]));
    fixtures.add(fixture(&[
        ("url", Value::string(base)),
        ("status", Value::I64(201)),
        ("body", Value::string("{\"ok\":true}")),
        ("contentType", Value::string("application/json")),
    ]));
    fixtures.add(fixture(&[
        ("url", Value::string("https://gateway.test/other/*")),
        ("bodyContains", Value::string("refund")),
        ("status", Value::I64(402)),
    ]));
    let lost = fixtures.answer(&request(Method::Post, base, Some("{}"))).unwrap_err();
    assert!(lost.message().contains("operation timed out"));
    let answered = fixtures.answer(&request(Method::Post, base, Some("{}"))).unwrap();
    assert_eq!((answered.status, answered.body.as_str()), (201, "{\"ok\":true}"));
    assert!(answered
        .headers
        .contains(&("Content-Type".to_string(), "application/json".to_string())));
    let prefixed = fixtures.answer(&request(Method::Get, "https://gateway.test/other/x", Some("a refund")));
    assert_eq!(prefixed.unwrap().status, 402);
    // no fixture matches: refused, never sent
    let refused = fixtures
        .answer(&request(Method::Get, "https://gateway.test/other/x", None))
        .unwrap_err();
    assert!(refused.message().contains("no HTTP fixture for GET"));
    let recorded = fixtures.requests();
    assert_eq!(
        recorded.iter().map(|row| row.fixture).collect::<Vec<_>>(),
        [Some(0), Some(1), Some(2), None]
    );
    let header = |name: &str| {
        recorded[0]
            .headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(header("Authorization").as_deref(), Some("[redacted]"));
    assert_eq!(header("X-Api-Key").as_deref(), Some("[redacted]"));
    assert_eq!(header("Idempotency-Key").as_deref(), Some("mq1"));
    let Value::Map(last) = recorded_value(&recorded[3]) else {
        panic!()
    };
    assert_eq!(last.get("fixture"), Some(&Value::I64(-1)));
    fixtures.clear();
    assert!(fixtures.requests().is_empty());
}

#[test]
fn only_a_test_run_enables_them_and_bad_fixtures_are_refused() {
    // no `moqui test` runner in this binary has called allow()
    assert!(!allowed());
    let ctx = ExecutionContextFactory::test_context();
    let tool = HttpFixturesTool;
    assert!(tool.call(&ctx, "enable", &[]).is_err());
    assert_eq!(tool.call(&ctx, "isEnabled", &[]).unwrap(), Value::Bool(false));
    assert!(intercept(&request(Method::Get, "https://gateway.test/", None)).is_none());
    tool.call(&ctx, "reset", &[]).unwrap();
    assert!(tool.call(&ctx, "add", &[Value::Null]).is_err());
    assert!(fixture_of(&Value::Map(IndexMap::new())).is_err());
    let bad_status = [
        ("url".to_string(), Value::string("x")),
        ("status".to_string(), Value::I64(7)),
    ];
    assert!(fixture_of(&Value::Map(bad_status.into_iter().collect())).is_err());
}
