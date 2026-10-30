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

use super::{cookie_value, form_pairs, host_without_port, is_localhost, pairs_to_parameters, percent_decode};
use moqui_lib::object::Value;

#[test]
fn send_error_follows_accept() {
    let sent = |accept: &str| {
        let _web = super::install(std::collections::BTreeMap::new());
        let mut request = super::WebRequest::new("GET", "/apps/ship");
        if !accept.is_empty() {
            request.set_header("accept", accept);
        }
        super::set_request_info(&request);
        super::send_error(404, "No labelUrl", None);
        let (status, headers, body, sent) = super::take_output();
        assert!(sent);
        assert_eq!(status, 404);
        (
            headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                .map(|(_, value)| value.clone())
                .unwrap_or_default(),
            String::from_utf8(body).unwrap(),
        )
    };
    let (html_type, html) = sent("text/html");
    assert_eq!(html_type, "text/html;charset=UTF-8");
    assert_eq!(
        html,
        "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html;charset=utf-8\"/><title>Error 404 Not Found</title></head><body>\n<h2>Error 404 Not Found</h2>\n<p>Problem accessing /apps/ship</p>\n<p>Reason: No labelUrl</p>\n</body></html>\n"
    );
    let (json_type, json) = sent("application/json");
    assert_eq!(json_type, "application/json;charset=UTF-8");
    assert_eq!(
        json,
        "{'message':'No labelUrl','errorName':'Not Found','error':404,'path':'/apps/ship'}"
    );
    let (plain_type, plain) = sent("");
    assert_eq!(plain_type, "text/plain;charset=UTF-8");
    assert_eq!(plain, "404 No labelUrl /apps/ship");

    let _web = super::install(std::collections::BTreeMap::new());
    let mut request = super::WebRequest::new("GET", "/a<b");
    request.set_header("accept", "text/html, application/json");
    super::set_request_info(&request);
    super::send_error(404, "say \"hi\" <tag>", None);
    let (_, _, body, _) = super::take_output();
    let html = String::from_utf8(body).unwrap();
    assert!(html.contains("<p>Problem accessing /a&lt;b</p>"), "{html}");
    assert!(html.contains("<p>Reason: say &quot;hi&quot; &lt;tag&gt;</p>"), "{html}");

    let _web = super::install(std::collections::BTreeMap::new());
    let mut request = super::WebRequest::new("GET", "/apps/ship");
    request.set_header("accept", "application/json");
    super::set_request_info(&request);
    super::send_error(500, "", Some("boom\n\"x\""));
    let (_, _, body, _) = super::take_output();
    assert_eq!(
        String::from_utf8(body).unwrap(),
        "{'message':'boom\\n\\\"x\\\"','errorName':'Internal Server Error','error':500,'path':'/apps/ship'}"
    );
}

#[test]
fn request_details_leave_out_credential_parameters() {
    let parameters = [("orderId", "O1"), ("password", "hunter2"), ("api_key", "k-1")]
        .into_iter()
        .map(|(name, value)| (name.to_string(), Value::string(value)))
        .collect();
    let _web = super::install(parameters);
    let details = super::request_details();
    assert!(details.contains("Parameter: orderId = O1"), "{details}");
    assert!(!details.contains("hunter2") && !details.contains("k-1"), "{details}");
}

/// The query wins over a JSON field and over a multipart field. A repeated name in the query,
/// or in the form body, stays a list.
#[test]
fn the_query_wins_over_json_and_multipart() {
    let body = |request: &super::WebRequest| match super::parse_body(request) {
        Ok(parsed) => parsed,
        Err(_) => panic!("body refused"),
    };
    let mut json = super::WebRequest::new("POST", "/x");
    json.query.push(("orderId".to_string(), "1".to_string()));
    json.set_header("content-type", "application/json");
    json.body = br#"{"orderId":2,"note":"n"}"#.to_vec();
    let parsed = body(&json);
    assert_eq!(parsed.parameters.get("orderId"), Some(&Value::string("1")));
    assert_eq!(parsed.parameters.get("note"), Some(&Value::string("n")));

    let mut form = super::WebRequest::new("POST", "/x");
    form.query.push(("a".to_string(), "1".to_string()));
    form.query.push(("a".to_string(), "2".to_string()));
    form.set_header("content-type", "application/x-www-form-urlencoded");
    form.body = b"a=3&b=4".to_vec();
    let parsed = body(&form);
    assert_eq!(
        parsed.parameters.get("a"),
        Some(&Value::List(vec![
            Value::string("1"),
            Value::string("2"),
            Value::string("3")
        ]))
    );
    assert_eq!(parsed.parameters.get("b"), Some(&Value::string("4")));

    let mut upload = super::WebRequest::new("POST", "/x");
    upload.query.push(("name".to_string(), "x".to_string()));
    upload.query.push(("q".to_string(), "1".to_string()));
    upload.query.push(("q".to_string(), "2".to_string()));
    upload.set_header("content-type", "multipart/form-data; boundary=XyZ");
    upload.body = b"\
--XyZ\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nfrom-body\r\n\
--XyZ\r\nContent-Disposition: form-data; name=\"other\"\r\n\r\ny\r\n\
--XyZ\r\nContent-Disposition: form-data; name=\"q\"\r\n\r\nfrom-body\r\n\
--XyZ\r\nContent-Disposition: form-data; name=\"name\"; filename=\"a.bin\"\r\nContent-Type: application/octet-stream\r\n\r\nxyz\r\n\
--XyZ\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"b.bin\"\r\nContent-Type: application/octet-stream\r\n\r\nxyz\r\n\
--XyZ--\r\n"
        .to_vec();
    let parsed = body(&upload);
    assert_eq!(parsed.parameters.get("name"), Some(&Value::string("x")));
    assert_eq!(parsed.parameters.get("other"), Some(&Value::string("y")));
    assert_eq!(
        parsed.parameters.get("q"),
        Some(&Value::List(vec![Value::string("1"), Value::string("2")]))
    );
    let mut parameters = parsed.parameters.clone();
    super::overlay_multipart(&mut parameters, &parsed.multipart);
    assert_eq!(parameters.get("name"), Some(&Value::string("x")));
    assert!(
        parameters
            .get("doc")
            .is_some_and(|value| matches!(value, Value::Object(_))),
        "{parameters:?}"
    );
}

#[test]
fn a_json_body_is_its_fields_a_list_or_a_parse_error_and_always_the_body_text() {
    let body = |text: &str| {
        let mut request = super::WebRequest::new("POST", "/x");
        request.set_header("content-type", "application/json");
        request.body = text.as_bytes().to_vec();
        match super::parse_body(&request) {
            Ok(parsed) => parsed,
            Err(_) => panic!("body refused"),
        }
    };
    let object = body(r#"{"a": 1}"#);
    assert_eq!(object.parameters.get("a"), Some(&Value::I64(1)));
    assert_eq!(
        object.parameters.get("_requestBodyText"),
        Some(&Value::string(r#"{"a": 1}"#))
    );
    let list = body("[1, 2]");
    assert!(matches!(list.parameters.get("_requestBodyJsonList"), Some(Value::List(items)) if items.len() == 2));
    assert!(list.json_list.is_none());
    let broken = body("{not json");
    assert!(broken.parameters.contains_key("_requestBodyJsonParseError"));
}

#[test]
fn localhost_hosts_get_no_hsts() {
    assert!(is_localhost("localhost:8443"));
    assert!(is_localhost("LocalHost"));
    assert!(is_localhost("app.localhost:8443"));
    assert!(!is_localhost("localhost.example.com"));
    assert!(!is_localhost("[::1]:8443"));
    assert!(!is_localhost(""));
    assert_eq!(host_without_port("[::1]:8443"), "[::1]");
    assert_eq!(host_without_port("example.com:8080"), "example.com");
}

#[test]
fn a_repeated_name_becomes_a_list() {
    let map = pairs_to_parameters(form_pairs("a=1&b=x&a=2&a=3"));
    assert_eq!(map.get("b"), Some(&Value::string("x")));
    assert_eq!(
        map.get("a"),
        Some(&Value::List(vec![
            Value::string("1"),
            Value::string("2"),
            Value::string("3")
        ]))
    );
}

/// A lone non-breaking space is no parameter, as Java's `simplifyRequestParameters`; the name stays, null.
#[test]
fn a_lone_non_breaking_space_is_a_null_parameter() {
    let map = pairs_to_parameters(form_pairs("statusId=%C2%A0&note=%C2%A0x&a=%C2%A0&a=1"));
    assert_eq!(map.get("statusId"), Some(&Value::Null));
    assert_eq!(map.get("note"), Some(&Value::string("\u{a0}x")));
    assert_eq!(
        map.get("a"),
        Some(&Value::List(vec![Value::string("\u{a0}"), Value::string("1")]))
    );
}

/// Only a text body is `_requestBodyText`, and only a form type is a form, as Java's `WebFacadeImpl` reads them.
#[test]
fn a_binary_or_untyped_body_is_no_parameter() {
    let parse = |content_type: Option<&str>, body: &[u8]| {
        let mut request = super::WebRequest::new("POST", "/x");
        if let Some(content_type) = content_type {
            request.set_header("content-type", content_type);
        }
        request.body = body.to_vec();
        match super::parse_body(&request) {
            Ok(parsed) => parsed.parameters,
            Err(_) => panic!("body refused"),
        }
    };
    assert!(parse(Some("application/octet-stream"), b"a=1\xff").is_empty());
    assert!(parse(None, b"a=1").is_empty());
    let text = parse(Some("text/plain; charset=utf-8"), b"a=1");
    assert_eq!(text.get("_requestBodyText"), Some(&Value::string("a=1")));
    assert_eq!(text.get("a"), None);
    assert_eq!(
        parse(Some("application/x-www-form-urlencoded"), b"a=1").get("a"),
        Some(&Value::string("1"))
    );
}

/// Java's `getRequestUrl`: the host without a port of 80 or 443, and the query string as the client sent it.
#[test]
fn the_request_url_keeps_the_query_as_sent_and_drops_a_default_port() {
    let mut request = super::WebRequest::new("GET", "/apps/Find");
    request.set_header("host", "site.test:80");
    request.query = form_pairs("q=a%20b&n=%0A");
    assert_eq!(super::full_url(&request), "http://site.test/apps/Find?q=a+b&n=%0A");
    request.raw_query = Some("q=a%20b&n=%0A".to_string());
    request.set_header("host", "site.test:8080");
    assert_eq!(
        super::full_url(&request),
        "http://site.test:8080/apps/Find?q=a%20b&n=%0A"
    );
    request.https = true;
    request.set_header("host", "[::1]:443");
    assert_eq!(super::full_url(&request), "https://[::1]/apps/Find?q=a%20b&n=%0A");
}

#[test]
fn percent_decode_keeps_bad_escapes_and_never_panics() {
    assert_eq!(percent_decode("a%20b+c"), "a b c");
    assert_eq!(percent_decode("%a\u{e9}"), "%a\u{e9}");
    assert_eq!(percent_decode("tail%"), "tail%");
    assert_eq!(percent_decode("%zz%41"), "%zzA");
}

#[test]
fn cookie_value_skips_parts_without_an_equals_sign() {
    assert_eq!(
        cookie_value("junk; moqui.session=abc; x=y", "moqui.session").as_deref(),
        Some("abc")
    );
    assert_eq!(cookie_value("a=b", "moqui.session"), None);
    // Two Cookie headers as the host joins them.
    assert_eq!(
        cookie_value("a=1; moqui.session=abc; b=2", "moqui.session").as_deref(),
        Some("abc")
    );
}

#[test]
fn a_kind_keeps_its_status_through_a_transaction_and_a_message_picks_none() {
    use crate::context::{Error, ErrorKind};
    use crate::transaction::TransactionAttribute;

    let facade = crate::testing::open();
    let tx = facade.transactions_for("moqui.basic.Enumeration").unwrap();
    let err = tx
        .run(TransactionAttribute::UseOrBegin, None, || -> Result<(), _> {
            Err(Error::of_kind(ErrorKind::Tarpit, "User u has accessed AT_SERVICE x too many times").into())
        })
        .unwrap_err();
    let err = Error::from(err);
    assert_eq!(err.kind(), ErrorKind::Tarpit, "{err}");
    assert_eq!(super::status_for(&err), 429);
    // A tarpit's seconds come through a lib error as the response's Retry-After.
    let wrapped =
        moqui_lib::error::Error::from(Error::of_kind(ErrorKind::Tarpit, "too many times").with_retry_after(42));
    let response = super::json_error_of(&Error::from(wrapped));
    assert_eq!(response.status, 429);
    assert_eq!(response.header("Retry-After"), Some("42"));
    assert_eq!(super::json_error_of(&err).header("Retry-After"), None);
    // The kind is found under a lib error that names it as its source.
    let wrapped = moqui_lib::error::Error::new("outer").with_source(Error::of_kind(ErrorKind::Unavailable, "full"));
    assert_eq!(super::status_for(&Error::from(wrapped)), 503);
    // An unavailable database under a general error is still the request's 503.
    let beneath = Error::new("find failed").with_source(Error::of_kind(ErrorKind::Unavailable, "no connection"));
    assert_eq!(super::status_for(&beneath), 503);
    let not_found = Error::new("service failed").with_source(Error::of_kind(ErrorKind::NotFound, "no row"));
    assert_eq!(super::status_for(&not_found), 500);
    // A general error is a 500 whatever its words.
    for message in [
        "Operation x not supported",
        "User is not authorized",
        "No value found",
        "tarpit",
    ] {
        assert_eq!(super::status_for(&Error::new(message)), 500, "{message}");
    }
}

#[test]
fn a_production_500_names_only_an_error_id() {
    use crate::context::{Error, ErrorKind};

    let err = Error::new("SELECT * FROM SECRET_TABLE failed");
    super::TEST_PRODUCTION.with(|production| production.set(Some(true)));
    let hidden = super::error_text(500, &err);
    let shown = super::error_text(400, &Error::of_kind(ErrorKind::BadRequest, "bad name"));
    // Not the process property: a test that boots the default conf leaves it production.
    super::TEST_PRODUCTION.with(|production| production.set(Some(false)));
    let not_production = super::error_text(500, &err);
    super::TEST_PRODUCTION.with(|production| production.set(None));
    assert!(hidden.starts_with("internal error, id "), "{hidden}");
    assert!(!hidden.contains("SECRET_TABLE"), "{hidden}");
    assert_eq!(shown, "bad name");
    assert_eq!(not_production, "SELECT * FROM SECRET_TABLE failed");
}

/// Java's tracking pixel: the PNG for every id, a known message marked `ES_VIEWED` once, a suffix dropped only when
/// the dot is not first, and a status change the flow refuses logged with the image left in place.
#[test]
fn view_email_message_sends_the_png_and_marks_a_known_message() {
    use moqui_lib::object::to_plain_string;

    use crate::context::ExecutionContext;
    use crate::entity::EntityFacade;
    use crate::script::IsolatedContext;

    struct ClearStack;
    impl Drop for ClearStack {
        fn drop(&mut self) {
            crate::script::clear_context_stack();
        }
    }

    let ctx = crate::context::ExecutionContextFactory::test_context_with_conf(
        "<moqui-conf><artifact-execution-facade><artifact-execution type=\"AT_ENTITY\" authz-enabled=\"true\"/>\
             <artifact-execution type=\"AT_SERVICE\" authz-enabled=\"true\"/></artifact-execution-facade></moqui-conf>",
    );
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _clear = ClearStack;
    {
        let _authz = ctx.disable_authz();
        crate::testing::seed_status_flows(&facade);
        email_row(&facade, "PIXEL1", "ES_SENT");
        email_row(&facade, "A", "ES_SENT");
        email_row(&facade, "DRAFT1", "ES_DRAFT");
    }
    ctx.set_effective_time(Some(1_700_000_000_000));

    show("PIXEL1.png");
    let errors = ExecutionContext::with_current(|ec| ec.message().errors_string()).unwrap_or_default();
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(status_of(&facade, "PIXEL1"), "ES_VIEWED");
    let received = received_of(&facade, "PIXEL1");
    assert!(!received.is_empty(), "{received}");

    // Already viewed: a later effective time does not write `receivedDate` again.
    ctx.set_effective_time(Some(1_800_000_000_000));
    show("PIXEL1.png");
    assert_eq!(received_of(&facade, "PIXEL1"), received);
    assert_eq!(status_of(&facade, "PIXEL1"), "ES_VIEWED");

    // `a.b.c.png` keeps only the text before the first dot.
    show("A.b.c.png");
    assert_eq!(status_of(&facade, "A"), "ES_VIEWED", "{}", message_errors());
    assert!(email_row_missing(&facade, "A.b"));

    show("NOTAREALID.png");
    assert!(email_row_missing(&facade, "NOTAREALID"));

    // A leading dot is not a suffix (`dotIndex > 0` in Java).
    show(".png");
    assert!(email_row_missing(&facade, ".png"));
    assert_eq!(received_of(&facade, "PIXEL1"), received);

    {
        let _hidden = IsolatedContext::push();
        show_current();
    }
    assert_eq!(status_of(&facade, "PIXEL1"), "ES_VIEWED");
    assert_eq!(received_of(&facade, "PIXEL1"), received);

    // No `ES_DRAFT` to `ES_VIEWED` transition. The image is still the response and the row stays a draft.
    show("DRAFT1.png");
    assert_eq!(status_of(&facade, "DRAFT1"), "ES_DRAFT");
    assert!(received_of(&facade, "DRAFT1").is_empty());
    assert!(!message_errors().is_empty());

    fn email_row(facade: &EntityFacade, id: &str, status: &str) {
        let mut row = facade.make_value("moqui.basic.email.EmailMessage").unwrap();
        row.set("emailMessageId", Value::string(id));
        row.set("statusId", Value::string(status));
        row.create(facade).unwrap();
    }

    fn show(id: &str) {
        crate::script::set_context_name("emailMessageId", Value::string(id)).unwrap();
        show_current();
    }

    fn show_current() {
        let _web = super::install(std::collections::BTreeMap::new());
        super::view_email_message();
        let (status, headers, body, sent) = super::take_output();
        assert!(sent);
        assert_eq!(status, 200);
        let header = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        assert_eq!(header("content-type"), "image/png");
        assert_eq!(header("content-disposition"), "inline");
        assert_eq!(body, super::TRACKING_PNG);
    }

    fn status_of(facade: &EntityFacade, id: &str) -> String {
        field(facade, id, "statusId")
    }

    fn received_of(facade: &EntityFacade, id: &str) -> String {
        field(facade, id, "receivedDate")
    }

    fn field(facade: &EntityFacade, id: &str, name: &str) -> String {
        let row = facade
            .find("moqui.basic.email.EmailMessage")
            .unwrap()
            .eq("emailMessageId", Value::string(id))
            .use_cache(false)
            .disable_authz()
            .one()
            .unwrap()
            .unwrap_or_else(|| panic!("no EmailMessage {id}"));
        row.get(name).map(to_plain_string).unwrap_or_default()
    }

    fn email_row_missing(facade: &EntityFacade, id: &str) -> bool {
        facade
            .find("moqui.basic.email.EmailMessage")
            .unwrap()
            .eq("emailMessageId", Value::string(id))
            .use_cache(false)
            .disable_authz()
            .one()
            .unwrap()
            .is_none()
    }

    fn message_errors() -> String {
        ExecutionContext::with_current(|ec| ec.message().errors_string()).unwrap_or_default()
    }
}
