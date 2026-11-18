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

use super::{
    cookie_value, form_pairs, host_without_port, is_localhost, pairs_to_parameters, parse_size, percent_decode,
};
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
    assert_eq!(host_without_port("[::1]"), "[::1]");
    assert_eq!(host_without_port("[::1"), "[::1");
    assert_eq!(host_without_port("example.com"), "example.com");
}

#[test]
fn an_upload_size_is_bytes_or_a_number_with_a_unit() {
    assert_eq!(parse_size("1000"), Some(1000));
    assert_eq!(parse_size(" 2K "), Some(2048));
    assert_eq!(parse_size("3 m"), Some(3 * 1024 * 1024));
    assert_eq!(parse_size("1g"), Some(1024 * 1024 * 1024));
    assert_eq!(parse_size(""), None);
    assert_eq!(parse_size("k"), None);
    assert_eq!(parse_size("0"), None);
    assert_eq!(parse_size("12x"), None);
    assert_eq!(parse_size("5é"), None);
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
    assert_eq!(percent_decode("%4"), "%4");
    assert_eq!(percent_decode("a%41"), "aA");
    assert_eq!(percent_decode("%%41"), "%A");
    assert_eq!(percent_decode("%C3%A9"), "\u{e9}");
}

/// Over https the session cookie is `__Host-moqui.session` only: a plain `moqui.session`, which another host of the
/// domain or a plain-http page could have set, is no session there (SEC-WEB-05). Over http the plain name is read.
#[test]
fn an_https_request_reads_only_the_host_prefixed_session_cookie() {
    let mut request = super::WebRequest::new("GET", "/apps");
    request.set_header("cookie", "moqui.session=planted.mac");
    assert_eq!(super::session_cookie_of(&request).as_deref(), Some("planted.mac"));
    request.https = true;
    assert_eq!(super::session_cookie_of(&request), None, "the plain name over https");
    request.set_header("cookie", "moqui.session=planted.mac; __Host-moqui.session=own.mac");
    assert_eq!(super::session_cookie_of(&request).as_deref(), Some("own.mac"));
    request.https = false;
    assert_eq!(super::session_cookie_of(&request).as_deref(), Some("planted.mac"));
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

/// The tracking image: the PNG for every request; a message marked `ES_VIEWED` once, and only by its view key, a keyed
/// MAC of its id (SEC-RTE-01); only a sent message, never one queued in the Outbox or received; a refused status change
/// logged with the image left in place.
#[test]
fn view_email_message_sends_the_png_and_marks_a_sent_message_by_its_key() {
    use moqui_lib::object::to_plain_string;

    use crate::context::ExecutionContext;
    use crate::entity::EntityFacade;
    use crate::script::IsolatedContext;

    struct ClearStack;
    impl Drop for ClearStack {
        fn drop(&mut self) {
            crate::script::clear_context_stack();
            moqui_lib::env::clear_property("moqui_conf__webapp_list__webapp__WEBROOT__session_config__cookie_key");
        }
    }

    let _key_lock = crate::user::session::cookie_property_lock();
    moqui_lib::env::set_property(
        "moqui_conf__webapp_list__webapp__WEBROOT__session_config__cookie_key",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0x0b_u8; 32]),
    );
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
        email_row(&facade, "QUEUED1", "ES_READY");
        email_row(&facade, "INBOX1", "ES_RECEIVED");
    }
    ctx.set_effective_time(Some(1_700_000_000_000));
    let key = |id: &str| super::email_view_key(id).unwrap();

    // The bare id, as Java's pixel had it, and a key of another message change nothing.
    show("PIXEL1.png");
    show(&format!("PIXEL1.{}.png", key("QUEUED1").split_once('.').unwrap().1));
    assert_eq!(status_of(&facade, "PIXEL1"), "ES_SENT");

    show(&format!("{}.png", key("PIXEL1")));
    let errors = ExecutionContext::with_current(|ec| ec.message().errors_string()).unwrap_or_default();
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(status_of(&facade, "PIXEL1"), "ES_VIEWED");
    let received = received_of(&facade, "PIXEL1");
    assert!(!received.is_empty(), "{received}");

    // Already viewed: a later effective time does not write `receivedDate` again.
    ctx.set_effective_time(Some(1_800_000_000_000));
    show(&format!("{}.png", key("PIXEL1")));
    assert_eq!(received_of(&facade, "PIXEL1"), received);
    assert_eq!(status_of(&facade, "PIXEL1"), "ES_VIEWED");

    // Even with its key, mail in the Outbox stays queued and received mail keeps its status and date.
    show(&format!("{}.png", key("QUEUED1")));
    assert_eq!(status_of(&facade, "QUEUED1"), "ES_READY");
    show(&format!("{}.png", key("INBOX1")));
    assert_eq!(status_of(&facade, "INBOX1"), "ES_RECEIVED");
    assert!(received_of(&facade, "INBOX1").is_empty());

    show(&format!("{}.png", key("NOTAREALID")));
    assert!(email_row_missing(&facade, "NOTAREALID"));
    show(".png");
    assert!(email_row_missing(&facade, ".png"));

    {
        let _hidden = IsolatedContext::push();
        show_current();
    }
    assert_eq!(status_of(&facade, "PIXEL1"), "ES_VIEWED");
    assert_eq!(received_of(&facade, "PIXEL1"), received);
    let _ = message_errors();

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

/// A user's file shown inline is sandboxed and never sniffed; an active type is sent inline only when asked for.
#[test]
fn a_users_inline_file_is_sandboxed_and_active_types_are_attachments() {
    let header = |headers: &[(String, String)], wanted: &str| {
        headers
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, value)| value.clone())
    };
    let shown = super::file_headers("report.html", "report.html", true, false);
    assert_eq!(header(&shown, "Content-Security-Policy").as_deref(), Some("sandbox"));
    assert_eq!(header(&shown, "X-Content-Type-Options").as_deref(), Some("nosniff"));
    // browsers will not show a PDF in a sandbox
    let pdf = super::file_headers("report.pdf", "report.pdf", true, false);
    assert_eq!(header(&pdf, "Content-Security-Policy"), None);
    assert_eq!(header(&pdf, "X-Content-Type-Options").as_deref(), Some("nosniff"));
    let saved = super::file_headers("report.html", "report.html", false, false);
    assert!(header(&saved, "Content-Disposition").is_some_and(|value| value.starts_with("attachment")));
    assert_eq!(header(&saved, "Content-Security-Policy"), None);
    for active in [
        "text/html",
        "image/svg+xml",
        "application/xml",
        "application/javascript",
        "application/atom+xml",
    ] {
        assert!(super::is_active_content_type(active), "{active}");
    }
    for passive in [
        "image/png",
        "application/pdf",
        "text/plain",
        "application/json",
        "text/css",
    ] {
        assert!(!super::is_active_content_type(passive), "{passive}");
    }
}

/// A user's file asked inline (`sendResourceResponse(location, true)`, a PASS download whose authority says inline)
/// is an attachment when its type can run script, and shown sandboxed otherwise, a PDF unsandboxed.
#[test]
fn a_users_file_asked_inline_is_shown_only_when_passive_and_then_sandboxed() {
    use super::user_file_inline;
    assert_eq!(user_file_inline("text/html; charset=utf-8", true), (false, false));
    assert_eq!(user_file_inline("image/svg+xml", true), (false, false));
    assert_eq!(user_file_inline("image/png", true), (true, true));
    assert_eq!(user_file_inline("application/pdf", true), (true, false));
    assert_eq!(user_file_inline("image/png", false), (false, false));
}

/// SEC-PSF-08: a user's file a service sends from its own storage (`send#RunArtifact`) is answered by its own type only
/// when inert, else as bytes, so `x.js` is no script a page of the origin could load; with `default-src 'none';
/// sandbox` but on an inline PDF, and a page the caller vouches for (`InlineActive`) sandboxed by its own type.
#[test]
fn a_users_download_has_an_inert_type_and_the_sandbox() {
    use super::{user_download_headers, FileDisposition, USER_FILE_SANDBOX};
    let header = |name: &str, disposition: FileDisposition, wanted: &str| {
        user_download_headers(name, disposition)
            .into_iter()
            .find(|(header, _)| header == wanted)
            .map(|(_, value)| value)
            .unwrap_or_default()
    };
    for disposition in [
        FileDisposition::Attachment,
        FileDisposition::Inline,
        FileDisposition::InlineActive,
    ] {
        assert_eq!(header("x.js", disposition, "Content-Type"), "application/octet-stream");
        assert_eq!(
            header("x.js", disposition, "Content-Security-Policy"),
            USER_FILE_SANDBOX
        );
        assert!(header("x.js", disposition, "Content-Disposition").starts_with("attachment"));
        assert_eq!(header("x.js", disposition, "X-Content-Type-Options"), "nosniff");
    }
    assert_eq!(
        header("site.css", FileDisposition::Inline, "Content-Type"),
        "application/octet-stream"
    );
    assert_eq!(
        header("TEST-unit.xml", FileDisposition::Attachment, "Content-Type"),
        "application/octet-stream"
    );
    assert_eq!(
        header("app.tar.gz", FileDisposition::Attachment, "Content-Security-Policy"),
        USER_FILE_SANDBOX
    );
    assert_eq!(header("shot.png", FileDisposition::Inline, "Content-Type"), "image/png");
    assert_eq!(
        header("shot.png", FileDisposition::Inline, "Content-Disposition"),
        "inline"
    );
    assert_eq!(
        header("shot.png", FileDisposition::Inline, "Content-Security-Policy"),
        USER_FILE_SANDBOX
    );
    assert_eq!(
        header("doc.pdf", FileDisposition::Inline, "Content-Security-Policy"),
        ""
    );
    assert_eq!(
        header("report.html", FileDisposition::InlineActive, "Content-Type"),
        "text/html"
    );
    assert_eq!(
        header("report.html", FileDisposition::InlineActive, "Content-Security-Policy"),
        "sandbox"
    );
    assert_eq!(
        header("report.html", FileDisposition::Inline, "Content-Type"),
        "application/octet-stream"
    );
    assert!(super::is_inert_content_type("video/mp4"));
    assert!(super::is_inert_content_type("text/plain; charset=utf-8"));
    assert!(!super::is_inert_content_type("text/css"));
    assert!(!super::is_inert_content_type("application/json"));
}

/// SEC-RTE-07: a multipart body that may pass `webapp-list/webapp[webroot]/web-protection/requests@max-body` has an anonymous cap of `webapp-list/webapp[webroot]/web-protection/requests@max-body`, but on the
/// paths that take uploads with no user, which a climbing path does not reach.
#[test]
fn an_anonymous_upload_stops_at_the_body_limit_but_on_the_open_paths() {
    use super::{anonymous_upload_limit, open_upload_path, DEFAULT_MAX_BODY};
    let mut upload = super::WebRequest::new("POST", "/apps/files/upload");
    upload.set_header("content-type", "multipart/form-data; boundary=x");
    assert_eq!(anonymous_upload_limit(None, &upload), Some(DEFAULT_MAX_BODY));
    let mut form = super::WebRequest::new("POST", "/apps/files/save");
    form.set_header("content-type", "application/x-www-form-urlencoded");
    assert_eq!(anonymous_upload_limit(None, &form), None);
    assert!(open_upload_path("/s/, /drop/", "/s/abc/upload"));
    assert!(open_upload_path("/s/, /drop/", "/drop/x"));
    assert!(!open_upload_path("/s/, /drop/", "/apps/s/x"));
    assert!(!open_upload_path("/s/", "/s/../apps/upload"));
    assert!(!open_upload_path("/s/", "/s/%2E%2E/apps/upload"));
    assert!(!open_upload_path("", "/s/abc/upload"));
}

/// SEC-WEB-04: a request another site started is told by `Sec-Fetch-Site`, else by a `Referer` of another host; a
/// request with neither, as a client that is not a browser sends, is not.
#[test]
fn a_request_another_site_started_is_told_by_its_fetch_site_or_referer() {
    let request = |headers: &[(&str, &str)]| {
        let mut request = super::WebRequest::new("GET", "/Login/logout");
        request.set_header("Host", "app.example.com");
        for (name, value) in headers {
            request.set_header(*name, *value);
        }
        super::started_by_another_site(&request)
    };
    assert!(request(&[("Sec-Fetch-Site", "cross-site")]));
    assert!(request(&[("Sec-Fetch-Site", "same-site")]));
    assert!(!request(&[("Sec-Fetch-Site", "same-origin")]));
    assert!(!request(&[("Sec-Fetch-Site", "none")]));
    // the fetch site wins over a referer
    assert!(!request(&[("Sec-Fetch-Site", "same-origin"), ("Referer", "https://evil.example/")]));
    assert!(request(&[("Referer", "https://evil.example/page")]));
    assert!(request(&[("Referer", "https://app.example.com.evil.example/")]));
    assert!(!request(&[("Referer", "https://APP.example.com/apps/tools")]));
    assert!(!request(&[]));
}
