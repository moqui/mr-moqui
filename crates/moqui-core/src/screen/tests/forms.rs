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

//! A form posted to a transition through the serve path: the session token, the redirect, the messages and
//! typed values that the next html page shows once, a multipart upload, and the error pages.

use std::path::PathBuf;

use super::{request, World};
use crate::screen::serve_at;
use crate::web::{WebRequest, WebResponse};

struct Site {
    dir: PathBuf,
    root: String,
}

impl Site {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("moqui-forms-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let messages = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../runtime/base-component/webroot/screen/includes/Messages.html.fancy");
        let screen = r#"<screen require-authentication="false">
    <transition name="save">
        <actions><script><![CDATA[
            if (!title || title == 'bad') ec.message.addError('Title is not valid')
            else ec.message.addMessage('Saved ' + title)
        ]]></script></actions>
        <default-response url="."/>
        <error-response url="."/>
    </transition>
    <transition name="upload">
        <actions><script><![CDATA[
            const doc = ec.web.multiPartParameters.doc
            ec.web.sendJsonResponse({title: title, name: doc.name, type: doc.contentType, size: doc.size,
                text: doc.getString(), count: ec.web.multiPartParameters._fileUploadList.length})
        ]]></script></actions>
        <default-response type="none"/>
    </transition>
    <transition name="check">
        <actions><script><![CDATA[
            ec.message.addMessage('Check this', 'warning')
            ec.message.addValidationError('Edit', 'title', 'save#Thing', 'Title needs a letter')
        ]]></script></actions>
        <default-response url="."/>
        <error-response url="."/>
    </transition>
    <transition name="ajax" require-session-token="false">
        <actions><script>ec.web.sendJsonResponse({ok: true})</script></actions>
        <default-response type="none"/>
    </transition>
    <widgets>
        <render-mode><text type="html" location="MESSAGES"/></render-mode>
        <form-single name="Edit" transition="save">
            <field name="title"><default-field><text-line/></default-field></field>
            <field name="go"><default-field><submit/></default-field></field>
        </form-single>
    </widgets>
</screen>"#
            .replace("MESSAGES", &messages.display().to_string());
        std::fs::write(dir.join("Form.xml"), screen).unwrap();
        let root = dir.join("Form.xml").to_string_lossy().into_owned();
        Self { dir, root }
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The cookies a response sets, as a `Cookie` header value.
pub(super) fn cookies(response: &WebResponse, jar: &mut Vec<(String, String)>) {
    for (name, value) in &response.headers {
        if name.eq_ignore_ascii_case("set-cookie") {
            let pair = value.split(';').next().unwrap_or("");
            if let Some((key, val)) = pair.split_once('=') {
                jar.retain(|(existing, _)| existing != key);
                jar.push((key.to_string(), val.to_string()));
            }
        }
    }
}

pub(super) fn with_cookies(mut request: WebRequest, jar: &[(String, String)]) -> WebRequest {
    if !jar.is_empty() {
        let header: Vec<String> = jar.iter().map(|(key, val)| format!("{key}={val}")).collect();
        request.set_header("cookie", header.join("; "));
    }
    request
}

pub(super) fn token_of(html: &str) -> String {
    let marker = "name=\"moquiSessionToken\" value=\"";
    let start = html.find(marker).unwrap_or_else(|| panic!("no token in {html}")) + marker.len();
    html[start..start + html[start..].find('"').unwrap()].to_string()
}

pub(super) fn form_post(path: &str, body: &str) -> WebRequest {
    let mut post = request("POST", path);
    post.set_header("content-type", "application/x-www-form-urlencoded");
    post.body = body.as_bytes().to_vec();
    post
}

#[test]
fn a_validation_error_shows_by_its_field_and_at_the_top_and_a_message_keeps_its_type_after_the_redirect() {
    let site = Site::new("validation");
    let world = World::open("");
    let mut jar = Vec::new();
    let page = serve_at(&world.factory, request("GET", "/"), &site.root);
    cookies(&page, &mut jar);
    let token = token_of(&page.body_text());
    let posted = serve_at(
        &world.factory,
        with_cookies(
            form_post(
                "/check",
                &format!("title=x&moquiFormName=Edit&moquiSessionToken={token}"),
            ),
            &jar,
        ),
        &site.root,
    );
    assert_eq!(posted.status, 302, "{}", posted.body_text());
    cookies(&posted, &mut jar);
    let html = serve_at(&world.factory, with_cookies(request("GET", "/"), &jar), &site.root).body_text();
    assert!(html.contains("data-type=\"warning\""), "{html}");
    assert!(html.contains("Check this"), "{html}");
    // The field's own error is under the field, and at the top of the page with the field's name as Java's apps.xml
    // header, which is what shows an error for a field that is not on the page, once.
    assert!(
        html.contains("<div class=\"field-error\" role=\"alert\">Title needs a letter</div>"),
        "{html}"
    );
    assert_eq!(html.matches("data-type=\"danger\"").count(), 1, "{html}");
    assert!(html.contains("Title needs a letter (for field Title)</div>"), "{html}");
}

/// A required service parameter left empty is Java's "Field cannot be empty" for that field, which the page shows
/// at the top when the form has no such field, as Java's header shows a service's validation error.
#[test]
fn an_empty_required_service_parameter_shows_as_java_on_the_next_page() {
    let site = Site::new("required");
    let services = site.dir.join("NeedServices.xml");
    std::fs::write(
        &services,
        r#"<services>
    <service verb="need" noun="Facility" authenticate="false">
        <in-parameters><parameter name="facilityId" required="true"/></in-parameters>
    </service>
</services>"#,
    )
    .unwrap();
    let screen = std::fs::read_to_string(&site.root).unwrap().replacen(
        "<widgets>",
        r#"<transition name="need"><service-call name="need#Facility"/><default-response url="."/></transition>
    <widgets>"#,
        1,
    );
    std::fs::write(&site.root, screen).unwrap();
    let world = World::open("");
    {
        let ctx = world.factory.new_context();
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&services)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    let mut jar = Vec::new();
    let page = serve_at(&world.factory, request("GET", "/"), &site.root);
    cookies(&page, &mut jar);
    let token = token_of(&page.body_text());
    let posted = serve_at(
        &world.factory,
        with_cookies(form_post("/need", &format!("moquiSessionToken={token}")), &jar),
        &site.root,
    );
    assert_eq!(posted.status, 302, "{}", posted.body_text());
    cookies(&posted, &mut jar);
    let html = serve_at(&world.factory, with_cookies(request("GET", "/"), &jar), &site.root).body_text();
    assert!(
        html.contains("Field cannot be empty (for field Facility ID)</div>"),
        "{html}"
    );
    assert_eq!(html.matches("data-type=\"danger\"").count(), 1, "{html}");
}

/// A transition's service-call with web-send-json-response sends the service's result, or the named expression over it,
/// as the JSON response, as the dynamic options of FindOrder's facility drop-down read them.
#[test]
fn a_service_call_with_web_send_json_response_sends_its_result() {
    let site = Site::new("send-json");
    let services = site.dir.join("ListServices.xml");
    std::fs::write(
        &services,
        r#"<services>
    <service verb="list" noun="Facilities" authenticate="false">
        <in-parameters><parameter name="ownerPartyId"/></in-parameters>
        <out-parameters><parameter name="resultList" type="List"/></out-parameters>
        <actions><script>resultList = [{facilityId: ownerPartyId + '_WH', label: 'Warehouse'}]</script></actions>
    </service>
</services>"#,
    )
    .unwrap();
    let screen = std::fs::read_to_string(&site.root).unwrap().replacen(
        "<widgets>",
        r#"<transition name="facilities" require-session-token="false">
        <service-call name="list#Facilities" in-map="{ownerPartyId: ownerPartyId}" web-send-json-response="resultList"/>
        <default-response type="none"/></transition>
    <transition name="whole" require-session-token="false">
        <service-call name="list#Facilities" web-send-json-response="true"/>
        <default-response type="none"/></transition>
    <widgets>"#,
        1,
    );
    std::fs::write(&site.root, screen).unwrap();
    let world = World::open("");
    {
        let ctx = world.factory.new_context();
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&services)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    let listed = serve_at(
        &world.factory,
        form_post("/facilities", "ownerPartyId=ZIRET"),
        &site.root,
    );
    assert_eq!(listed.status, 200, "{}", listed.body_text());
    let json: serde_json::Value = serde_json::from_str(&listed.body_text()).unwrap();
    assert_eq!(
        json,
        serde_json::json!([{"facilityId": "ZIRET_WH", "label": "Warehouse"}])
    );

    let whole = serve_at(&world.factory, form_post("/whole", "ownerPartyId=ZIRET"), &site.root);
    assert_eq!(whole.status, 200, "{}", whole.body_text());
    let json: serde_json::Value = serde_json::from_str(&whole.body_text()).unwrap();
    assert_eq!(
        json["resultList"],
        serde_json::json!([{"facilityId": "ZIRET_WH", "label": "Warehouse"}])
    );
}

#[test]
fn a_saved_validation_error_reaches_the_qvt_m_form_of_its_form() {
    let site = Site::new("validation-qvt");
    let world = World::open("");
    let mut jar = Vec::new();
    let page = serve_at(&world.factory, request("GET", "/"), &site.root);
    cookies(&page, &mut jar);
    let token = token_of(&page.body_text());
    let posted = serve_at(
        &world.factory,
        with_cookies(
            form_post(
                "/check",
                &format!("title=x&moquiFormName=Edit&moquiSessionToken={token}"),
            ),
            &jar,
        ),
        &site.root,
    );
    assert_eq!(posted.status, 302, "{}", posted.body_text());
    cookies(&posted, &mut jar);
    let mut get = request("GET", "/");
    get.query.push(("renderMode".into(), "qvt".into()));
    let qvt = serve_at(&world.factory, with_cookies(get, &jar), &site.root).body_text();
    assert!(qvt.contains(":validation-errors-initial="), "{qvt}");
    assert!(qvt.contains("Title needs a letter"), "{qvt}");
    assert!(qvt.contains("fieldPretty"), "{qvt}");
}

#[test]
fn a_posted_form_redirects_and_the_next_page_shows_its_messages_and_values_once() {
    let site = Site::new("flash");
    let world = World::open("");
    let mut jar = Vec::new();

    let page = serve_at(&world.factory, request("GET", "/"), &site.root);
    assert_eq!(page.status, 200, "{}", page.body_text());
    cookies(&page, &mut jar);
    let html = page.body_text();
    assert!(
        html.contains("<form name=\"Edit\" id=\"Edit\" method=\"post\" action=\"/save\">"),
        "{html}"
    );
    let token = token_of(&html);
    assert!(!html.contains("class=\"alert\""), "{html}");

    // without the token the transition is refused
    let refused = serve_at(
        &world.factory,
        with_cookies(form_post("/save", "title=bad&moquiFormName=Edit"), &jar),
        &site.root,
    );
    assert_eq!(refused.status, 401, "{}", refused.body_text());

    // a bad value: redirect back to the form
    let bad = serve_at(
        &world.factory,
        with_cookies(
            form_post(
                "/save",
                &format!("title=bad&moquiFormName=Edit&moquiSessionToken={token}"),
            ),
            &jar,
        ),
        &site.root,
    );
    assert_eq!(bad.status, 302, "{}", bad.body_text());
    assert_eq!(bad.header("Location"), Some("/"));
    cookies(&bad, &mut jar);

    let again = serve_at(&world.factory, with_cookies(request("GET", "/"), &jar), &site.root);
    let html = again.body_text();
    assert!(
        html.contains("<div class=\"alert\" data-type=\"danger\" role=\"alert\">"),
        "{html}"
    );
    assert!(html.contains("Title is not valid</div>"), "{html}");
    // the typed value is back in the field
    assert!(html.contains("name=\"title\" value=\"bad\""), "{html}");

    // shown once
    let third = serve_at(&world.factory, with_cookies(request("GET", "/"), &jar), &site.root);
    let html = third.body_text();
    assert!(!html.contains("Title is not valid"), "{html}");
    assert!(html.contains("name=\"title\" value=\"\""), "{html}");

    // a good value: the message is info and the value is not carried back
    let good = serve_at(
        &world.factory,
        with_cookies(
            form_post(
                "/save",
                &format!("title=Fine&moquiFormName=Edit&moquiSessionToken={token}"),
            ),
            &jar,
        ),
        &site.root,
    );
    assert_eq!(good.status, 302, "{}", good.body_text());
    let after = serve_at(&world.factory, with_cookies(request("GET", "/"), &jar), &site.root).body_text();
    assert!(after.contains("<div class=\"alert\" data-type=\"info\""), "{after}");
    assert!(after.contains("Saved Fine</div>"), "{after}");
    assert!(after.contains("name=\"title\" value=\"\""), "{after}");

    // the token also comes as a header, as the client sends it
    let mut ajax = form_post("/save", "title=Header");
    ajax.set_header("x-csrf-token", token);
    let by_header = serve_at(&world.factory, with_cookies(ajax, &jar), &site.root);
    assert_eq!(by_header.status, 302, "{}", by_header.body_text());

    // a transition that turns the check off
    let open = serve_at(&world.factory, form_post("/ajax", "a=1"), &site.root);
    assert_eq!(open.status, 200, "{}", open.body_text());
}

/// A multipart field: its name, its value and for a file the file name and type.
type Part<'a> = (&'a str, &'a str, Option<(&'a str, &'a str)>);

fn multipart(parts: &[Part<'_>]) -> (String, Vec<u8>) {
    let mut body = Vec::new();
    for (name, value, file) in parts {
        body.extend_from_slice(b"--BoUnD\r\n");
        match file {
            Some((file_name, content_type)) => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\nContent-Type: {content_type}\r\n\r\n"
                )
                .as_bytes(),
            ),
            None => body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes()),
        }
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"--BoUnD--\r\n");
    ("multipart/form-data; boundary=BoUnD".to_string(), body)
}

fn upload_request(token: &str, parts: &[Part<'_>]) -> WebRequest {
    let mut all = vec![("moquiSessionToken", token, None)];
    all.extend_from_slice(parts);
    let (content_type, body) = multipart(&all);
    let mut post = request("POST", "/upload");
    post.set_header("content-type", content_type);
    post.body = body;
    post
}

#[test]
fn a_multipart_form_gives_parameters_and_the_uploaded_file() {
    let site = Site::new("upload");
    let world = World::open("");
    let mut jar = Vec::new();
    let page = serve_at(&world.factory, request("GET", "/"), &site.root);
    cookies(&page, &mut jar);
    let token = token_of(&page.body_text());

    let parts = [
        ("title", "Report", None),
        ("doc", "hello,world\n", Some(("C:\\fakepath\\data.csv", "text/csv"))),
    ];
    let response = serve_at(
        &world.factory,
        with_cookies(upload_request(&token, &parts), &jar),
        &site.root,
    );
    assert_eq!(response.status, 200, "{}", response.body_text());
    let json = crate::service::parse_json(&response.body_text()).unwrap();
    let get = |key: &str| match &json {
        moqui_lib::object::Value::Map(map) => moqui_lib::object::to_plain_string(map.get(key).unwrap()),
        other => panic!("{other:?}"),
    };
    assert_eq!(get("title"), "Report");
    assert_eq!(get("name"), "data.csv");
    assert_eq!(get("type"), "text/csv");
    assert_eq!(get("size"), "12");
    assert_eq!(get("text"), "hello,world\n");
    assert_eq!(get("count"), "1");

    // without the token a multipart post is refused as a urlencoded one is
    let (content_type, body) = multipart(&parts);
    let mut bare = request("POST", "/upload");
    bare.set_header("content-type", content_type);
    bare.body = body;
    assert_eq!(
        serve_at(&world.factory, with_cookies(bare, &jar), &site.root).status,
        401
    );

    // a program is not accepted unless the web app allows it
    let program = [("doc", "MZ\u{90}\u{0}", Some(("setup.exe", "application/octet-stream")))];
    let refused = serve_at(
        &world.factory,
        with_cookies(upload_request(&token, &program), &jar),
        &site.root,
    );
    assert_eq!(refused.status, 415, "{}", refused.body_text());
    assert!(refused.body_text().contains("setup.exe"), "{}", refused.body_text());
    let script = [("doc", "#!/bin/sh\necho hi", Some(("run.sh", "text/x-sh")))];
    assert_eq!(
        serve_at(
            &world.factory,
            with_cookies(upload_request(&token, &script), &jar),
            &site.root
        )
        .status,
        415
    );

    // a body that is not multipart at all
    let mut broken = request("POST", "/upload");
    broken.set_header("content-type", "multipart/form-data; boundary=BoUnD");
    broken.body = b"--BoUnD\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nno end".to_vec();
    let response = serve_at(&world.factory, with_cookies(broken, &jar), &site.root);
    assert_eq!(response.status, 400, "{}", response.body_text());
}

#[test]
fn the_upload_cap_is_webapp_max_upload_and_other_bodies_stop_at_webapp_max_body() {
    let site = Site::new("cap");
    let world = World::open("");
    moqui_lib::env::set_property("webapp_max_upload", "2k");
    let big = "x".repeat(3000);
    let parts = [("doc", big.as_str(), Some(("a.txt", "text/plain")))];
    let (content_type, body) = multipart(&parts);
    let mut post = request("POST", "/upload");
    post.set_header("content-type", content_type.clone());
    post.body = body;
    let response = serve_at(&world.factory, post, &site.root);
    moqui_lib::env::clear_property("webapp_max_upload");
    assert_eq!(response.status, 413, "{}", response.body_text());

    // a form post has its own cap, webapp_max_body (default 10 MiB), even when uploads may be larger
    moqui_lib::env::set_property("webapp_max_body", "1m");
    let mut large = request("POST", "/save");
    large.set_header("content-type", "application/x-www-form-urlencoded");
    large.body = vec![b'a'; 1024 * 1024 + 1];
    let status = serve_at(&world.factory, large, &site.root).status;
    moqui_lib::env::clear_property("webapp_max_body");
    assert_eq!(status, 413);
}

#[test]
fn a_failed_request_in_a_browser_is_an_html_error_page_and_elsewhere_json() {
    let site = Site::new("errors");
    let world = World::open("");
    let mut get = request("GET", "/NoSuchScreen");
    get.set_header("accept", "text/html,application/xhtml+xml");
    let page = serve_at(&world.factory, get, &site.root);
    // the fixture root has no error screens, so the host falls back to the JSON body
    assert_eq!(page.status, 404, "{}", page.body_text());
    let json = serve_at(&world.factory, request("GET", "/NoSuchScreen"), &site.root);
    assert_eq!(json.status, 404);
    assert_eq!(json.header("Content-Type"), Some("application/json"));
}

/// A root screen that wraps what is below it in the real header and footer, and mounts the real error screens.
fn shell_root(name: &str) -> Site {
    let dir = std::env::temp_dir().join(format!("moqui-shell-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let webroot = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/webroot/screen");
    let webroot = webroot.canonicalize().unwrap();
    let screen = r#"<screen require-authentication="false">
    <always-actions>
        <set field="html_scripts" from="new Set()"/>
        <set field="footer_scripts" from="new Set()"/>
        <set field="html_stylesheets" from="new Set()"/>
        <set field="footer_stylesheets" from="new Set()"/>
    </always-actions>
    <subscreens default-item="Home">
        <subscreens-item name="error" location="WEBROOT/webroot/error.xml"/>
    </subscreens>
    <widgets>
        <render-mode><text type="html" location="WEBROOT/includes/Header.html.fancy" no-boundary-comment="true"/></render-mode>
        <render-mode><text template="true">${sri.renderSubscreen()}</text></render-mode>
        <render-mode><text type="html" location="WEBROOT/includes/Footer.html.fancy"/></render-mode>
    </widgets>
</screen>"#
        .replace("WEBROOT", &webroot.display().to_string());
    std::fs::write(dir.join("Shell.xml"), screen).unwrap();
    std::fs::create_dir_all(dir.join("Shell")).unwrap();
    std::fs::write(
        dir.join("Shell/Home.xml"),
        r#"<screen require-authentication="false" default-menu-title="Home Page"><widgets><label text="the home page"/></widgets></screen>"#,
    )
    .unwrap();
    let root = dir.join("Shell.xml").to_string_lossy().into_owned();
    Site { dir, root }
}

#[test]
fn the_shell_wraps_a_page_and_the_real_error_pages_show_the_error_context() {
    let site = shell_root("errors");
    let world = World::open("");
    let mut home = request("GET", "/");
    home.set_header("accept", "text/html");
    let page = serve_at(&world.factory, home, &site.root);
    let html = page.body_text();
    assert_eq!(page.status, 200, "{html}");
    assert!(html.starts_with("<!DOCTYPE html>\n<html lang="), "{html}");
    assert!(html.contains("<meta name=\"moquiSessionToken\" content=\""), "{html}");
    assert!(html.contains("<title>Shell - Home Page</title>"), "{html}");
    assert!(html.contains("<span>the home page</span>"), "{html}");
    assert!(html.trim_end().ends_with("</body>\n</html>"), "{html}");

    let mut missing = request("GET", "/NoSuchScreen");
    missing.set_header("accept", "text/html");
    let page = serve_at(&world.factory, missing, &site.root);
    let html = page.body_text();
    assert_eq!(page.status, 404, "{html}");
    assert!(
        html.contains("<h1><i class=\"fa fa-link text-danger\"></i> Page Not Found (404)</h1>"),
        "{html}"
    );
    assert!(html.contains("Could not find <code>NoSuchScreen</code>"), "{html}");
    assert!(
        html.contains("<link rel=\"stylesheet\" href=\"/css/moqui-html.css"),
        "{html}"
    );
    assert!(html.contains("<script src=\"/js/moqui-html.js"), "{html}");
    assert!(html.contains("defer"), "{html}");
    let lower = html.to_ascii_lowercase();
    assert!(!lower.contains("jquery") && !lower.contains("bootstrap"), "{html}");

    // an API path keeps the JSON error
    let mut rest = request("GET", "/rest/NoSuchService");
    rest.set_header("accept", "text/html");
    assert_eq!(
        serve_at(&world.factory, rest, &site.root).header("Content-Type"),
        Some("application/json")
    );
}

/// `auto-fields-service` builds a field for each in-parameter, `auto-widget-service` the widget of one field, and the form's
/// transition that calls one service puts `validate-service` on the fields that are its parameters.
#[test]
fn a_form_builds_its_fields_from_a_service() {
    let dir = std::env::temp_dir().join(format!("moqui-forms-service-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let services = dir.join("FormServices.xml");
    std::fs::write(
        &services,
        r#"<services>
    <service verb="save" noun="Widget" authenticate="false">
        <in-parameters>
            <parameter name="title" required="true"/>
            <parameter name="count" type="Integer"/>
            <parameter name="when" type="Timestamp"/>
            <parameter name="note"/>
        </in-parameters>
    </service>
</services>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Form.xml"),
        r#"<screen require-authentication="false">
    <transition name="save"><service-call name="save#Widget"/><default-response url="."/></transition>
    <widgets>
        <form-single name="Auto" transition="save">
            <auto-fields-service service-name="save#Widget"/>
            <field name="note"><default-field title="Note"><text-area/></default-field></field>
            <field name="submitButton"><default-field><submit/></default-field></field>
        </form-single>
        <form-single name="Single" transition="save">
            <field name="when"><default-field><auto-widget-service service-name="save#Widget" field-type="find"/></default-field></field>
            <field name="title"><default-field><text-line/></default-field></field>
        </form-single>
    </widgets>
</screen>"#,
    )
    .unwrap();
    let world = World::open("");
    {
        let ctx = world.factory.new_context();
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&services)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    let root = dir.join("Form.xml").to_string_lossy().into_owned();
    let page = serve_at(&world.factory, request("GET", "/"), &root);
    let html = page.body_text();
    assert_eq!(page.status, 200, "{html}");
    assert!(html.contains("name=\"title\""), "{html}");
    assert!(html.contains("id=\"Auto_count\""), "{html}");
    assert!(
        html.contains("<input type=\"datetime-local\" name=\"when\" id=\"Auto_when\""),
        "{html}"
    );
    // validate-service: the required parameter is required, the integer one numeric
    assert!(html.contains("name=\"title\" value=\"\" required"), "{html}");
    assert!(
        html.contains("name=\"count\" value=\"\" data-wide inputmode=\"decimal\""),
        "{html}"
    );
    // the form's own field replaces the service's
    assert!(
        html.contains("<textarea") && html.contains("id=\"Auto_note\""),
        "{html}"
    );
    // a date find from auto-widget-service
    assert!(html.contains("name=\"when_from\""), "{html}");
    // qvt checks the Integer parameter as a whole number, from getFieldValidationJsRules
    let mut get = request("GET", "/");
    get.query.push(("renderMode".into(), "qvt".into()));
    let qvt = serve_at(&world.factory, get, &root).body_text();
    assert!(
        qvt.contains("moqui.isStringInteger(value)") && qvt.contains("Please enter a valid whole number"),
        "{qvt}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `DbForm` for `ALL_USERS` that modifies a screen form adds its field when the form renders, and a form that extends
/// `DbForm#id` is made of the `DbForm` fields.
#[test]
fn db_forms_modify_and_extend_a_form() {
    let dir = std::env::temp_dir().join(format!("moqui-forms-dbform-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.join("Form.xml").to_string_lossy().into_owned();
    std::fs::write(
        &root,
        r#"<screen require-authentication="false">
    <widgets>
        <form-single name="Plain">
            <field name="title"><default-field><text-line/></default-field></field>
        </form-single>
        <form-single name="FromDb" extends="DbForm#GATE_BASE"/>
    </widgets>
</screen>"#,
    )
    .unwrap();
    let world = World::open("");
    {
        let ctx = world.factory.new_context();
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let row = |entity: &str, fields: &[(&str, &str)]| {
            let mut value = world.facade.make_value(entity).unwrap();
            for (name, text) in fields {
                value.set(*name, moqui_lib::object::Value::string(*text));
            }
            value.create(&world.facade).unwrap();
        };
        row(
            "moqui.screen.form.DbForm",
            &[
                ("formId", "GATE_MOD"),
                ("modifyXmlScreenForm", &format!("{root}#Plain")),
            ],
        );
        row(
            "moqui.screen.form.DbFormUserGroup",
            &[("formId", "GATE_MOD"), ("userGroupId", "ALL_USERS")],
        );
        row(
            "moqui.screen.form.DbFormField",
            &[
                ("formId", "GATE_MOD"),
                ("fieldName", "extra"),
                ("fieldTypeEnumId", "DBFFT_text-line"),
                ("title", "Extra"),
            ],
        );
        row("moqui.screen.form.DbForm", &[("formId", "GATE_BASE")]);
        row(
            "moqui.screen.form.DbFormField",
            &[
                ("formId", "GATE_BASE"),
                ("fieldName", "kind"),
                ("fieldTypeEnumId", "DBFFT_drop-down"),
                ("title", "Kind"),
            ],
        );
        row(
            "moqui.screen.form.DbFormFieldOption",
            &[
                ("formId", "GATE_BASE"),
                ("fieldName", "kind"),
                ("sequenceNum", "1"),
                ("keyValue", "A"),
                ("text", "Alpha"),
            ],
        );
        row(
            "moqui.screen.form.DbFormFieldAttribute",
            &[
                ("formId", "GATE_BASE"),
                ("fieldName", "kind"),
                ("attributeName", "allow-empty"),
                ("value", "true"),
            ],
        );
    }
    let page = serve_at(&world.factory, request("GET", "/"), &root);
    let html = page.body_text();
    assert_eq!(page.status, 200, "{html}");
    assert!(html.contains("id=\"Plain_title\""), "{html}");
    assert!(html.contains("<label for=\"Plain_extra\">Extra</label>"), "{html}");
    assert!(html.contains("<label for=\"FromDb_kind\">Kind</label>"), "{html}");
    assert!(html.contains(">Alpha</option>"), "{html}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A form-list that groups by a field sums the others over the group, draws its sub-list fields in rows under the
/// group's row, and ends with the show-total row, in html and in qvt (whose template the Vue compiler takes).
#[test]
fn an_aggregated_form_list_groups_rows_with_sub_rows_and_totals() {
    let dir = std::env::temp_dir().join(format!("moqui-forms-aggregate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.join("Form.xml").to_string_lossy().into_owned();
    std::fs::write(
        &root,
        r#"<screen require-authentication="false">
    <actions>
        <script>lines = [{party: 'Acme', amount: 2, item: 'Bolt'}, {party: 'Zed', amount: 5, item: 'Nut'}, {party: 'Acme', amount: 3, item: 'Gear'}]</script>
    </actions>
    <widgets>
        <form-list name="Lines" list="lines">
            <field name="party" aggregate="group-by"><default-field title="Party"><display/></default-field></field>
            <field name="amount" aggregate="sum" show-total="true"><default-field title="Amount"><display/></default-field></field>
            <field name="item" aggregate="sub-list"><default-field title="Item"><display/></default-field></field>
        </form-list>
    </widgets>
</screen>"#,
    )
    .unwrap();
    let world = World::open("");
    let html = serve_at(&world.factory, request("GET", "/"), &root).body_text();
    let body = &html[html.find("<tbody>").expect("rows")..];
    let acme = body.find(">Acme<").expect("the Acme group");
    assert_eq!(body.matches(">Acme<").count(), 1, "one row for the group: {body}");
    assert!(body[acme..].find(">5<").is_some(), "Acme's amounts summed: {body}");
    let sub = body.find("<tr class=\"sub-rows\">").expect("sub rows");
    assert!(
        body[sub..].find(">Bolt<").is_some() && body[sub..].find(">Gear<").is_some(),
        "{body}"
    );
    let total = body.find("<tr data-total=\"\">").expect("the total row");
    assert!(body[total..].contains(">10<"), "{body}");
    assert!(
        body.contains("id=\"Lines_item_0_1\""),
        "a sub row's inputs have ids of their own: {body}"
    );
    assert!(
        html.contains("<tr class=\"sub-header\">") && html.contains(">Item<"),
        "{html}"
    );

    let mut get = request("GET", "/");
    get.query.push(("renderMode".into(), "qvt".into()));
    let qvt = serve_at(&world.factory, get, &root).body_text();
    assert!(
        qvt.contains("tr sub-rows") && qvt.contains("Gear") && qvt.contains("text-weight-bold"),
        "{qvt}"
    );
    // as Java: the sub-list titles once in the head, not over each row's sub-rows
    assert_eq!(qvt.matches("tr sub-header").count(), 1, "{qvt}");
    assert!(qvt.matches("tr sub-rows").count() > 1, "{qvt}");
    super::qvt::assert_compiles(&["aggregate".to_string()], &[qvt]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `{screen}/actions/{tree}` answers the jstree nodes of a tree: the root's sub-nodes for `treeNodeId=#`, a node's own
/// for its id and `treeNodeName`, with the children of the nodes on `treeOpenPath`.
#[test]
fn a_tree_answers_its_node_data() {
    let dir = std::env::temp_dir().join(format!("moqui-forms-tree-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.join("Tree.xml").to_string_lossy().into_owned();
    std::fs::write(
        &root,
        r#"<screen require-authentication="false">
    <transition name="show"><default-response url="."/></transition>
    <widgets>
        <tree name="Places">
            <tree-node name="Country">
                <actions><set field="cities" from="nodeList_entry.cities"/></actions>
                <link url="show" text="${nodeList_entry.name}" id="${nodeList_entry.code}"><parameter name="code" from="nodeList_entry.code"/></link>
                <tree-sub-node node-name="City" list="cities"/>
            </tree-node>
            <tree-node name="City">
                <condition><expression>nodeList_entry.name != 'Hidden'</expression></condition>
                <label text="${nodeList_entry.name}" id="${treeNodeId}/${nodeList_entry.name}"/>
            </tree-node>
            <tree-sub-node node-name="Country" list="countries">
                <actions><script>countries = [{code: 'FR', name: 'France', cities: [{name: 'Paris'}, {name: 'Hidden'}]}, {code: 'IS', name: 'Iceland', cities: []}]</script></actions>
            </tree-sub-node>
        </tree>
    </widgets>
</screen>"#,
    )
    .unwrap();
    let world = World::open("");
    let fetch = |pairs: &[(&str, &str)]| -> serde_json::Value {
        let mut get = request("GET", "/actions/Places");
        get.query = pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        let response = serve_at(&world.factory, get, &root);
        assert_eq!(response.status, 200, "{}", response.body_text());
        serde_json::from_str(&response.body_text()).unwrap()
    };
    let top = fetch(&[("treeNodeId", "#")]);
    assert_eq!(top[0]["id"], "FR", "{top}");
    assert_eq!(top[0]["text"], "France");
    assert_eq!(top[0]["li_attr"]["treeNodeName"], "Country");
    assert_eq!(top[0]["children"], true, "France has a city to show: {top}");
    assert_eq!(top[1]["children"], false, "{top}");
    // `show` only redirects to this screen, so the link goes to the screen, as Java expands an alias transition.
    assert_eq!(top[0]["a_attr"]["href"], "/?code=FR", "{top}");
    let opened = fetch(&[("treeNodeId", "#"), ("treeOpenPath", "FR")]);
    assert_eq!(opened[0]["state"]["opened"], true, "{opened}");
    assert_eq!(opened[0]["children"][0]["id"], "FR/Paris", "{opened}");
    assert_eq!(
        opened[0]["children"].as_array().unwrap().len(),
        1,
        "the condition leaves out Hidden: {opened}"
    );
    let cities = fetch(&[("treeNodeId", "FR"), ("treeNodeName", "Country")]);
    // a node's own request has no Country entry in the context, so no cities
    assert!(cities.as_array().unwrap().is_empty(), "{cities}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A template file under a screen that says `include-child-content` renders inside the screen with the request in its
/// context; under another screen it is refused. A binary file is still sent.
#[test]
fn template_files_under_a_screen_render_as_its_child_content() {
    let dir = std::env::temp_dir().join(format!("moqui-forms-child-content-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Alone")).unwrap();
    std::fs::create_dir_all(dir.join("Wrapped")).unwrap();
    std::fs::write(
        dir.join("Alone.xml"),
        r#"<screen require-authentication="false"><widgets><label text="alone screen"/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(dir.join("Alone/Note.html.fancy"), "<p>Note for ${who}</p>").unwrap();
    std::fs::write(
        dir.join("Wrapped.xml"),
        r#"<screen require-authentication="false" include-child-content="true"><widgets><label text="wrapper"/><subscreens-active/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(dir.join("Wrapped/Page.md.fancy"), "# Hello ${who}\n").unwrap();
    std::fs::write(dir.join("Wrapped/dot.png"), [0x89u8, b'P', b'N', b'G']).unwrap();
    let world = World::open("");
    let get = |root: &str, path: &str| {
        let mut get = request("GET", path);
        get.query.push(("who".into(), "Ada".into()));
        serve_at(&world.factory, get, &dir.join(root).to_string_lossy())
    };
    // Under a screen that does not include child content a template is a source, and refused.
    let alone = get("Alone.xml", "/Note.html.fancy");
    assert_eq!(alone.status, 404, "{}", alone.body_text());
    let wrapped = get("Wrapped.xml", "/Page.md.fancy").body_text();
    assert!(
        wrapped.contains("wrapper") && wrapped.contains("<h1>Hello Ada</h1>"),
        "{wrapped}"
    );
    let image = get("Wrapped.xml", "/dot.png");
    assert_eq!(image.status, 200);
    assert_eq!(image.header("content-type"), Some("image/png"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A drop-down with dynamic-options whose current value is not among its options shows the label the transition gives
/// for it, as `getFieldTransitionValue`.
#[test]
fn a_dynamic_drop_down_shows_the_transition_label_of_its_value() {
    let dir = std::env::temp_dir().join(format!("moqui-forms-transition-value-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.join("Form.xml").to_string_lossy().into_owned();
    std::fs::write(
        &root,
        r#"<screen require-authentication="false">
    <transition name="kinds" read-only="true"><actions><script>ec.web.sendJsonResponse([{value: term, label: 'Kind ' + term}])</script></actions>
        <default-response type="none"/></transition>
    <actions><set field="kind" value="K7"/></actions>
    <widgets>
        <form-single name="Pick">
            <field name="kind"><default-field><drop-down><dynamic-options transition="kinds" server-search="true"/></drop-down></default-field></field>
        </form-single>
    </widgets>
</screen>"#,
    )
    .unwrap();
    let world = World::open("");
    let mut get = request("GET", "/");
    get.query.push(("renderMode".into(), "qvt".into()));
    let qvt = serve_at(&world.factory, get, &root).body_text();
    assert!(qvt.contains("Kind K7"), "{qvt}");
    let html = serve_at(&world.factory, request("GET", "/"), &root).body_text();
    assert!(html.contains(">Kind K7</option>"), "{html}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A multi form-list posts `name_N` for each row with `_isMulti=true`; the transition's service-call shortcut (whose `multi`
/// defaults to `parameter`) then calls the service once for each row.
#[test]
fn a_multi_form_post_calls_the_transition_service_for_each_row() {
    let dir = std::env::temp_dir().join(format!("moqui-forms-multi-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let services = dir.join("RowServices.xml");
    std::fs::write(
        &services,
        r#"<services>
    <service verb="save" noun="Row" authenticate="false">
        <in-parameters><parameter name="testId"/><parameter name="testMedium"/></in-parameters>
        <actions><entity-make-value entity-name="moqui.test.TestEntity" value-field="made" map="context"/><entity-create value-field="made"/></actions>
    </service>
</services>"#,
    )
    .unwrap();
    let root = dir.join("Rows.xml").to_string_lossy().into_owned();
    std::fs::write(
        &root,
        r#"<screen require-authentication="false">
    <transition name="saveRows" require-session-token="false"><service-call name="save#Row"/><default-response type="none"/></transition>
    <widgets><label text="rows"/></widgets>
</screen>"#,
    )
    .unwrap();
    let world = World::open("");
    {
        let ctx = world.factory.new_context();
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&services)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    let posted = serve_at(
        &world.factory,
        form_post(
            "/saveRows",
            "testId_0=R0&testMedium_0=first&testId_1=R1&testMedium_1=second&_isMulti=true",
        ),
        &root,
    );
    assert!(posted.status < 400, "{}", posted.body_text());
    let medium = |id: &str| {
        let ctx = world.factory.new_context();
        let _entered = ctx.enter();
        world
            .facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", moqui_lib::object::Value::string(id))
            .one()
            .unwrap()
            .and_then(|row| row.get("testMedium").map(moqui_lib::object::to_plain_string))
    };
    assert_eq!(medium("R0").as_deref(), Some("first"));
    assert_eq!(medium("R1").as_deref(), Some("second"));
    let _ = std::fs::remove_dir_all(&dir);
}
