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
fn a_validation_error_shows_by_its_field_and_a_message_keeps_its_type_after_the_redirect() {
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
    // The field's own error is under the field, not a line at the top of the page.
    assert!(
        html.contains("<div class=\"field-error\" role=\"alert\">Title needs a letter</div>"),
        "{html}"
    );
    assert!(!html.contains("data-type=\"danger\""), "{html}");
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

/// The webapp's `error-screen` of an error type names the page a browser gets, as Java's `getErrorScreenNode`.
#[test]
fn a_browser_error_shows_the_error_screen_the_conf_names() {
    let dir = std::env::temp_dir().join(format!("moqui-error-screen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Root.xml"),
        format!(
            r#"<screen require-authentication="false"><subscreens>
            <subscreens-item name="Oops" location="{}"/></subscreens><widgets><subscreens-active/></widgets></screen>"#,
            dir.join("Oops.xml").display()
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("Oops.xml"),
        r#"<screen require-authentication="false"><widgets><label text="Nothing at ${errorType}"/></widgets></screen>"#,
    )
    .unwrap();
    let world = World::open(
        r#"<moqui-conf><webapp-list><webapp name="webroot">
            <error-screen error="not-found" screen-path="Oops"/></webapp></webapp-list></moqui-conf>"#,
    );
    let mut get = request("GET", "/NoSuchScreen");
    get.set_header("accept", "text/html");
    let page = serve_at(&world.factory, get, &dir.join("Root.xml").to_string_lossy());
    assert_eq!(page.status, 404, "{}", page.body_text());
    assert!(
        page.body_text().contains("Nothing at not-found"),
        "{}",
        page.body_text()
    );
    let _ = std::fs::remove_dir_all(&dir);
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
    // error.xml names /css and /js paths. They are not screens or files under this temporary root, so each URL is
    // `#`, as Java's getUrlWithBase when the target does not exist. The version query is still appended.
    assert!(html.contains("<link rel=\"stylesheet\" href=\"#?v="), "{html}");
    assert!(html.contains("<script src=\"#?v="), "{html}");
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
    // `when` is a service timestamp, not an entity field named fromDate, so it has no auto default.
    assert!(!html.contains("ec.user.nowTimestamp"), "{html}");
    // qvt checks the Integer parameter as a whole number, from getFieldValidationJsRules
    let mut get = request("GET", "/");
    get.query.push(("renderMode".into(), "qvt".into()));
    let qvt = serve_at(&world.factory, get, &root).body_text();
    assert!(
        qvt.contains("$mq.isStringInteger(value)") && qvt.contains("Please enter a valid whole number"),
        "{qvt}"
    );
    // tui sends the same rules as data, and the required parameter as `required`
    let mut get = request("GET", "/");
    get.query.push(("renderMode".into(), "tui".into()));
    let tui = serve_at(&world.factory, get, &root).body_text();
    assert!(
        tui.contains(r#""rules":[{"kind":"integer","message":"Please enter a valid whole number"}]"#),
        "{tui}"
    );
    assert!(tui.contains(r#""name":"title","required":true"#), "{tui}");
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

/// A `group-by` field the list does not show is not in the group key, as Java's `includeFields`: a field left out of the
/// columns, a field whose `hide` holds, and a field outside the columns of the request's `_uiType`.
#[test]
fn a_group_by_field_the_list_does_not_show_does_not_split_the_groups() {
    let dir = std::env::temp_dir().join(format!("moqui-forms-group-shown-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, invoice: &str, columns: &str| {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!(
                r#"<screen require-authentication="false">
    <actions>
        <script>lines = [{{party: 'Acme', invoice: 'i1', amount: 1}}, {{party: 'Acme', invoice: 'i2', amount: 2}}, {{party: 'Zed', invoice: 'i3', amount: 4}}]</script>
    </actions>
    <widgets>
        <form-list name="Lines" list="lines">
            <field name="party" aggregate="group-by"><default-field title="Party"><display/></default-field></field>
            <field name="invoice" aggregate="group-by"{invoice}><default-field title="Invoice"><display/></default-field></field>
            <field name="amount" aggregate="sum"><default-field title="Amount"><display/></default-field></field>
            {columns}
        </form-list>
    </widgets>
</screen>"#
            ),
        )
        .unwrap();
        path.to_string_lossy().into_owned()
    };
    let world = World::open("");
    let acme_rows = |root: &str, ui_type: Option<&str>| {
        let mut get = request("GET", "/");
        if let Some(kind) = ui_type {
            get.query.push(("_uiType".into(), kind.into()));
        }
        let html = serve_at(&world.factory, get, root).body_text();
        let body = html[html.find("<tbody>").expect("rows")..].to_string();
        (body.matches(">Acme<").count(), body)
    };

    let columns = write(
        "Columns.xml",
        "",
        r#"<form-list-column><field-ref name="party"/></form-list-column><form-list-column><field-ref name="amount"/></form-list-column>"#,
    );
    let (count, body) = acme_rows(&columns, None);
    assert_eq!(count, 1, "invoice is not a column: {body}");
    assert!(body.contains(">3<"), "Acme's amounts summed: {body}");

    let hidden = write("Hidden.xml", r#" hide="true""#, "");
    let (count, body) = acme_rows(&hidden, None);
    assert_eq!(count, 1, "invoice is hidden: {body}");

    let typed = write(
        "Typed.xml",
        "",
        r#"<columns type="mobile"><column><field-ref name="party"/></column><column><field-ref name="amount"/></column></columns>
            <columns><column><field-ref name="party"/></column><column><field-ref name="invoice"/></column><column><field-ref name="amount"/></column></columns>"#,
    );
    let (count, body) = acme_rows(&typed, Some("mobile"));
    assert_eq!(count, 1, "the mobile columns leave invoice out: {body}");
    let (count, body) = acme_rows(&typed, None);
    assert_eq!(count, 2, "the untyped columns show invoice: {body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A group average includes the first row. 10, 20 and 30 show as 20, and the count field shows 3.
#[test]
fn an_average_form_list_includes_the_first_row() {
    let dir = std::env::temp_dir().join(format!("moqui-forms-average-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.join("Form.xml").to_string_lossy().into_owned();
    std::fs::write(
        &root,
        r#"<screen require-authentication="false">
    <actions>
        <script>rows = [{g:'A', n:10}, {g:'A', n:20}, {g:'A', n:30}]</script>
    </actions>
    <widgets>
        <form-list name="Avg" list="rows">
            <field name="g" aggregate="group-by"><default-field><display/></default-field></field>
            <field name="n" aggregate="avg"><default-field><display/></default-field></field>
            <field name="nCount"><default-field><display/></default-field></field>
        </form-list>
    </widgets>
</screen>"#,
    )
    .unwrap();
    let world = World::open("");
    let html = serve_at(&world.factory, request("GET", "/"), &root).body_text();
    assert_eq!(html.matches(">A<").count(), 1, "{html}");
    assert!(html.contains(">20<") || html.contains(">20.0000000000<"), "{html}");
    assert!(html.contains(">3<"), "{html}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A rendered form-list records `_formListRendered` and adds its row count on the shared map, after aggregation.
/// `noResultsAbort` reads those with property access: a count of 0 aborts, and a missing rendered flag does not.
#[test]
fn a_rendered_form_list_counts_its_rows_on_the_shared_map() {
    use indexmap::IndexMap;
    use moqui_lib::object::Value;

    let dir = std::env::temp_dir().join(format!("moqui-forms-list-count-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let lists = dir.join("Lists.xml");
    std::fs::write(
        &lists,
        r#"<screen require-authentication="false">
    <actions><script><![CDATA[
        none = []
        two = [{id:'a'},{id:'b'}]
        four = [{id:'c'},{id:'d'},{id:'e'},{id:'f'}]
    ]]></script></actions>
    <widgets>
        <form-list name="None" list="none"><field name="id"><default-field><display/></default-field></field></form-list>
        <form-list name="Two" list="two"><field name="id"><default-field><display/></default-field></field></form-list>
        <form-list name="Four" list="four"><field name="id"><default-field><display/></default-field></field></form-list>
    </widgets></screen>"#,
    )
    .unwrap();
    let empty = dir.join("Empty.xml");
    std::fs::write(
        &empty,
        r#"<screen require-authentication="false">
    <actions><script>none = []</script></actions>
    <widgets>
        <form-list name="None" list="none"><field name="id"><default-field><display/></default-field></field></form-list>
    </widgets></screen>"#,
    )
    .unwrap();
    let groups = dir.join("Groups.xml");
    std::fs::write(
        &groups,
        r#"<screen require-authentication="false">
    <actions><script>lines = [{party:'Acme', amount:2}, {party:'Zed', amount:5}, {party:'Acme', amount:3}]</script></actions>
    <widgets>
        <form-list name="Groups" list="lines">
            <field name="party" aggregate="group-by"><default-field><display/></default-field></field>
            <field name="amount" aggregate="sum"><default-field><display/></default-field></field>
        </form-list>
    </widgets></screen>"#,
    )
    .unwrap();
    let world = World::open("");
    let render = |path: &str| {
        let ctx = world.factory.new_context();
        let _entered = ctx.enter();
        let page = serve_at(&world.factory, request("GET", "/"), path);
        assert_eq!(page.status, 200, "{}", page.body_text());
        let shared = ctx.shared_map();
        (
            shared.get("_formListRendered").cloned(),
            shared.get("_formListResultCount").cloned(),
        )
    };
    assert_eq!(
        render(&empty.to_string_lossy()),
        (Some(Value::Bool(true)), Some(Value::I64(0)))
    );
    assert_eq!(
        render(&lists.to_string_lossy()),
        (Some(Value::Bool(true)), Some(Value::I64(6)))
    );
    // Three source rows in two groups. The count is the rows after aggregation.
    assert_eq!(
        render(&groups.to_string_lossy()),
        (Some(Value::Bool(true)), Some(Value::I64(2)))
    );

    let ctx = world.factory.new_context();
    let _entered = ctx.enter();
    let _ = serve_at(&world.factory, request("GET", "/"), &lists.to_string_lossy());
    // A bare name does not see the shared map.
    assert!(!crate::script::eval_condition("_formListRendered").unwrap());
    assert!(crate::script::eval_condition("ec.context.sharedMap._formListRendered").unwrap());
    let send = "attachmentInfo != null && (screenScheduled.noResultsAbort != 'Y' || !ec.context.sharedMap._formListRendered || ec.context.sharedMap._formListResultCount)";
    let mut scheduled = IndexMap::new();
    scheduled.insert("noResultsAbort".into(), Value::string("Y"));
    crate::script::set_context_name("screenScheduled", Value::Map(scheduled)).unwrap();
    crate::script::set_context_name("attachmentInfo", Value::Map(IndexMap::new())).unwrap();
    assert!(crate::script::eval_condition(send).unwrap(), "a count of 6 still sends");
    ctx.shared_map().insert("_formListResultCount".into(), Value::I64(0));
    assert!(!crate::script::eval_condition(send).unwrap(), "a count of 0 aborts");
    ctx.shared_map().shift_remove("_formListRendered");
    assert!(
        crate::script::eval_condition(send).unwrap(),
        "no rendered flag does not abort"
    );
    ctx.shared_map().insert("_formListRendered".into(), Value::Bool(true));
    let mut allow = IndexMap::new();
    allow.insert("noResultsAbort".into(), Value::string("N"));
    crate::script::set_context_name("screenScheduled", Value::Map(allow)).unwrap();
    assert!(
        crate::script::eval_condition(send).unwrap(),
        "noResultsAbort other than Y sends"
    );
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
    // `show` only redirects to `.`, so the link is the screen itself, with the node's parameter.
    assert_eq!(top[0]["a_attr"]["href"].as_str().unwrap(), "/?code=FR", "{top}");
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

/// `ec.web.sendTextResponse(text, contentType, filename)` sends the text with that type (UTF-8, text/plain by default) and with
/// a file name as an attachment of that name, a null text as empty; an uploaded file reads in the charset given to
/// `getString(charset)` (and its stream to `getText(charset)`), UTF-8 without one.
#[test]
fn a_text_response_takes_a_type_and_a_file_name_and_an_upload_reads_in_its_charset() {
    let dir = std::env::temp_dir().join(format!("moqui-forms-text-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.join("Text.xml").to_string_lossy().into_owned();
    std::fs::write(
        &root,
        r#"<screen require-authentication="false">
    <transition name="csv" read-only="true"><actions><script>ec.web.sendTextResponse('a,b\n', 'text/csv', 'Report 1.csv')</script></actions>
        <default-response type="none"/></transition>
    <transition name="page" read-only="true"><actions><script>ec.web.sendTextResponse('&lt;p&gt;x&lt;/p&gt;', 'text/html', null)</script></actions>
        <default-response type="none"/></transition>
    <transition name="none" read-only="true"><actions><script>ec.web.sendTextResponse(null)</script></actions>
        <default-response type="none"/></transition>
    <transition name="upload" require-session-token="false">
        <actions><script><![CDATA[
            const doc = ec.web.multiPartParameters.doc
            ec.web.sendJsonResponse({utf8: doc.getString(), w1252: doc.getString('windows-1252'), latin: doc.getString('ISO-8859-1'),
                stream: doc.getInputStream().getText('windows-1252')})
        ]]></script></actions>
        <default-response type="none"/>
    </transition>
    <widgets><label text="text"/></widgets>
</screen>"#,
    )
    .unwrap();
    let world = World::open("");
    let csv = serve_at(&world.factory, request("GET", "/csv"), &root);
    assert_eq!(csv.status, 200, "{}", csv.body_text());
    assert_eq!(csv.body_text(), "a,b\n");
    assert_eq!(csv.header("Content-Type"), Some("text/csv;charset=UTF-8"));
    assert_eq!(
        csv.header("Content-Disposition"),
        Some("attachment; filename=\"Report_1.csv\"; filename*=utf-8''Report%201.csv")
    );
    let html = serve_at(&world.factory, request("GET", "/page"), &root);
    assert_eq!(
        html.header("Content-Type"),
        Some("text/html;charset=UTF-8"),
        "{}",
        html.body_text()
    );
    assert_eq!(
        html.header("Content-Disposition"),
        Some("inline"),
        "as Java's with no file name"
    );
    assert_eq!(html.body_text(), "<p>x</p>");
    let none = serve_at(&world.factory, request("GET", "/none"), &root);
    assert_eq!(none.status, 200);
    assert_eq!(none.body_text(), "");
    assert_eq!(none.header("Content-Type"), Some("text/plain;charset=UTF-8"));

    // "café “quoted”" in windows-1252: é is E9, the quotes 93 and 94
    let mut body = Vec::new();
    body.extend_from_slice(
        b"--BoUnD\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"contacts.csv\"\r\nContent-Type: text/csv\r\n\r\n",
    );
    body.extend_from_slice(&[
        b'c', b'a', b'f', 0xE9, b' ', 0x93, b'q', b'u', b'o', b't', b'e', b'd', 0x94,
    ]);
    body.extend_from_slice(b"\r\n--BoUnD--\r\n");
    let mut post = request("POST", "/upload");
    post.set_header("content-type", "multipart/form-data; boundary=BoUnD");
    post.body = body;
    let answer = serve_at(&world.factory, post, &root);
    assert_eq!(answer.status, 200, "{}", answer.body_text());
    let json = crate::service::parse_json(&answer.body_text()).unwrap();
    let get = |key: &str| match &json {
        moqui_lib::object::Value::Map(map) => moqui_lib::object::to_plain_string(map.get(key).unwrap()),
        other => panic!("{other:?}"),
    };
    assert_eq!(get("w1252"), "café \u{201c}quoted\u{201d}");
    assert_eq!(get("stream"), "café \u{201c}quoted\u{201d}");
    assert_eq!(
        get("latin"),
        "café \u{93}quoted\u{94}",
        "ISO-8859-1 is byte for byte, as Java"
    );
    assert_eq!(
        get("utf8"),
        "caf\u{fffd} \u{fffd}quoted\u{fffd}",
        "not UTF-8: replaced, as Java's decoder"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// What a client shows of a refused transition carries the messages only, never the Rust source location of the
/// error (`crates/...rs:line:col` after "Rust:"), which stays in the server log: a service whose body fails in Rust
/// (its failure in the message facade), a service that is not there called from a script, and the JSON answer of a
/// script after a failed call.
#[test]
fn a_refused_transitions_errors_carry_no_rust_source_location() {
    let dir = std::env::temp_dir().join(format!("moqui-forms-rust-origin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let services = dir.join("OriginServices.xml");
    std::fs::write(
        &services,
        "<services><service verb=\"fail\" noun=\"InRust\" authenticate=\"anonymous-all\"><actions><script><![CDATA[\n\
         ec.resource.getLocationText('component://MissingDiagnostics/screen/A.xml')\n]]></script></actions></service></services>",
    )
    .unwrap();
    let screen = r#"<screen require-authentication="false">
    <transition name="failService" require-session-token="false">
        <service-call name="fail#InRust"/>
        <default-response url="."/><error-response url="."/>
    </transition>
    <transition name="missingService" require-session-token="false">
        <actions><script>ec.service.sync().name('no.such.Services.do#Nothing').call()</script></actions>
        <default-response url="."/><error-response url="."/>
    </transition>
    <transition name="throws" require-session-token="false">
        <actions><script>ec.resource.getLocationText('component://MissingDiagnostics/screen/B.xml')</script></actions>
        <default-response url="."/><error-response url="."/>
    </transition>
    <transition name="jsonAnswer" require-session-token="false">
        <actions><script>
            ec.service.sync().name('fail#InRust').call()
            ec.web.sendJsonResponse({done: false})
        </script></actions>
        <default-response type="none"/>
    </transition>
    <widgets><label text="the page"/></widgets>
</screen>"#;
    std::fs::write(dir.join("Origin.xml"), screen).unwrap();
    let root = dir.join("Origin.xml").to_string_lossy().into_owned();
    let world = World::open("");
    {
        let ctx = world.factory.new_context();
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&services)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    for path in ["/failService", "/missingService", "/throws", "/jsonAnswer"] {
        let mut post = request("POST", path);
        post.set_header("accept", "application/json");
        post.set_header("x-requested-with", "XMLHttpRequest");
        let answer = serve_at(&world.factory, post, &root);
        let text = answer.body_text();
        assert!(
            text.contains("MissingDiagnostics") || text.contains("no.such.Services"),
            "{path}: {} {text}",
            answer.status
        );
        assert!(!text.contains("crates/") && !text.contains("Rust:"), "{path}: {text}");
        assert!(!text.contains(".rs:"), "{path}: {text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// An auto foreign key with 1 to 200 related rows is a drop-down. `fromDate` on an edit defaults to now.
/// Zero rows and more than 200 are a text line with no size. Display stays `display-entity`.
#[test]
fn an_auto_foreign_key_is_a_drop_down_and_from_date_defaults_to_now() {
    use moqui_lib::node::MNode;
    use moqui_lib::object::Value;

    let dir = std::env::temp_dir().join(format!("moqui-forms-auto-fk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let services = dir.join("AutoServices.xml");
    std::fs::write(
        &services,
        r#"<services>
    <service verb="save" noun="Source" authenticate="false">
        <in-parameters>
            <parameter name="dataSourceTypeEnumId" entity-name="moqui.basic.DataSource"/>
            <parameter name="when" type="Timestamp"/>
            <parameter name="memberFrom" type="Timestamp" entity-name="moqui.security.UserGroupMember" field-name="fromDate"/>
        </in-parameters>
    </service>
</services>"#,
    )
    .unwrap();
    let widget = |field: &str, entity: &str, kind: &str| {
        format!(
            r#"<field name="{field}"><default-field><auto-widget-entity entity-name="{entity}" field-type="{kind}"/></default-field></field>"#
        )
    };
    let screen = format!(
        r#"<screen require-authentication="false"><widgets>
        <form-single name="MemberEdit"><auto-fields-entity entity-name="moqui.security.UserGroupMember" field-type="edit"/></form-single>
        <form-single name="MemberFind"><auto-fields-entity entity-name="moqui.security.UserGroupMember" field-type="find"/></form-single>
        <form-single name="EnumDisplay">{display}</form-single>
        <form-single name="EnumEdit">{edit}{indicator}</form-single>
        <form-single name="EnumFind">{find}</form-single>
        <form-list name="EnumHeader" list="rows">{header}</form-list>
        <form-single name="UpdateMasterEntityValue">{edit}</form-single>
        <form-single name="PlainEdit">{edit}</form-single>
        <form-single name="SourceEdit">{source}</form-single>
        <form-single name="ManyTypes">{edit}</form-single>
        <form-single name="Run"><auto-fields-service service-name="save#Source" field-type="edit"/></form-single>
    </widgets></screen>"#,
        display = widget("enumTypeId", "moqui.basic.Enumeration", "display"),
        edit = widget("enumTypeId", "moqui.basic.Enumeration", "edit"),
        indicator = widget("optionIndicator", "moqui.basic.Enumeration", "edit"),
        find = widget("enumTypeId", "moqui.basic.Enumeration", "find"),
        header = widget("enumTypeId", "moqui.basic.Enumeration", "find-display"),
        source = widget("dataSourceTypeEnumId", "moqui.basic.DataSource", "edit"),
    );
    std::fs::write(dir.join("Auto.xml"), screen).unwrap();
    let root = dir.join("Auto.xml").to_string_lossy().into_owned();
    let world = World::open("");
    let ctx = world.factory.new_context();
    let _entered = ctx.enter();
    let failures = crate::service::lint(std::slice::from_ref(&services)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");

    let assembled = |name: &str| {
        let facade = crate::screen::ScreenFacade::of(&world.factory);
        let def = facade.screen_def(&world.factory, &root).unwrap();
        let form = crate::screen::form::assemble(&facade, &world.factory, &def, name).unwrap();
        MNode::parse_text(name, &form.xml).unwrap()
    };
    let field_of = |form: &MNode, name: &str| {
        form.children_named("field")
            .into_iter()
            .find(|field| field.attribute("name").as_deref() == Some(name))
            .unwrap_or_else(|| panic!("{name} in {}", form.write_string()))
    };
    let sub_of = |form: &MNode, name: &str, sub: &str| {
        field_of(form, name)
            .first_named(sub)
            .unwrap_or_else(|| panic!("{sub} of {name} in {}", form.write_string()))
    };

    let member = assembled("MemberEdit");
    let group = sub_of(&member, "userGroupId", "default-field");
    let line = group.first_named("text-line").expect("no related rows is a text line");
    assert!(line.attribute("size").is_none(), "{}", line.write_string());
    assert!(group.first_named("drop-down").is_none());
    let from = sub_of(&member, "fromDate", "default-field")
        .first_named("date-time")
        .unwrap();
    assert_eq!(
        from.attribute("default-value").as_deref(),
        Some("${ec.l10n.format(ec.user.nowTimestamp, 'yyyy-MM-dd HH:mm')}")
    );
    let thru = sub_of(&member, "thruDate", "default-field")
        .first_named("date-time")
        .unwrap();
    assert!(thru.attribute("default-value").is_none(), "{}", thru.write_string());
    let member_find = assembled("MemberFind");
    let found = sub_of(&member_find, "fromDate", "default-field")
        .first_named("date-find")
        .unwrap();
    assert!(found.attribute("default-value").is_none(), "{}", found.write_string());

    let insert = |entity: &str, pairs: &[(&str, &str)]| {
        let mut row = world.facade.make_value(entity).unwrap();
        for (name, value) in pairs {
            row.set(*name, Value::string(*value));
        }
        row.create(&world.facade).unwrap();
    };
    for index in 1..=2 {
        insert(
            "moqui.basic.EnumerationType",
            &[("enumTypeId", &format!("T{index}")), ("description", "Type")],
        );
    }
    let edited = assembled("EnumEdit");
    let drop = sub_of(&edited, "enumTypeId", "default-field")
        .first_named("drop-down")
        .expect("1 to 200 related rows is a drop-down");
    assert_eq!(drop.attribute("allow-empty").as_deref(), Some("true"));
    let options = drop.first_named("entity-options").unwrap();
    assert_eq!(
        options.attribute("text").as_deref(),
        Some("${description || ''} [${enumTypeId}]")
    );
    let find = options.first_named("entity-find").unwrap();
    assert_eq!(
        find.attribute("entity-name").as_deref(),
        Some("moqui.basic.EnumerationType")
    );
    assert_eq!(find.attribute("limit").as_deref(), Some("200"));
    assert_eq!(
        find.first_named("order-by").unwrap().attribute("field-name").as_deref(),
        Some("description")
    );
    let indicator = sub_of(&edited, "optionIndicator", "default-field")
        .first_named("drop-down")
        .unwrap();
    let keys: Vec<String> = indicator
        .children_named("option")
        .iter()
        .filter_map(|option| option.attribute("key"))
        .collect();
    assert_eq!(keys, ["Y", "N"]);
    assert!(indicator.first_named("entity-options").is_none());

    let find_form = assembled("EnumFind");
    assert!(sub_of(&find_form, "enumTypeId", "default-field")
        .first_named("drop-down")
        .is_some());
    let header = assembled("EnumHeader");
    assert!(sub_of(&header, "enumTypeId", "header-field")
        .first_named("drop-down")
        .is_some());
    assert!(sub_of(&header, "enumTypeId", "default-field")
        .first_named("display-entity")
        .is_some());
    let display = assembled("EnumDisplay");
    assert!(sub_of(&display, "enumTypeId", "default-field")
        .first_named("display-entity")
        .is_some());
    assert!(sub_of(&display, "enumTypeId", "default-field")
        .first_named("drop-down")
        .is_none());

    let master = assembled("UpdateMasterEntityValue");
    let link = sub_of(&master, "enumTypeId", "default-field")
        .first_named("link")
        .expect("UpdateMasterEntityValue adds an edit link");
    assert_eq!(link.attribute("url").as_deref(), Some("edit"));
    assert_eq!(
        link.attribute("text").as_deref(),
        Some("Edit Enumeration Type [${fieldValues.enumTypeId}]")
    );
    assert_eq!(link.attribute("condition").as_deref(), Some("enumTypeId"));
    assert_eq!(link.attribute("link-type").as_deref(), Some("anchor"));
    let parameters = link.children_named("parameter");
    assert_eq!(parameters[0].attribute("name").as_deref(), Some("aen"));
    assert_eq!(
        parameters[0].attribute("value").as_deref(),
        Some("moqui.basic.EnumerationType")
    );
    assert_eq!(parameters[1].attribute("name").as_deref(), Some("enumTypeId"));
    assert_eq!(
        parameters[1].attribute("from").as_deref(),
        Some("fieldValues.enumTypeId")
    );
    assert!(sub_of(&assembled("PlainEdit"), "enumTypeId", "default-field")
        .first_named("link")
        .is_none());

    for index in 0..201 {
        insert(
            "moqui.basic.Enumeration",
            &[
                ("enumId", &format!("Other{index}")),
                ("enumTypeId", "OtherType"),
                ("description", "Other"),
            ],
        );
    }
    insert(
        "moqui.basic.Enumeration",
        &[
            ("enumId", "Wanted"),
            ("enumTypeId", "DataSourceType"),
            ("description", "Wanted"),
        ],
    );
    let source = assembled("SourceEdit");
    let source_drop = sub_of(&source, "dataSourceTypeEnumId", "default-field")
        .first_named("drop-down")
        .expect("the title filter keeps the count inside 1..=200");
    let source_find = source_drop
        .first_named("entity-options")
        .unwrap()
        .first_named("entity-find")
        .unwrap();
    let condition = source_find.first_named("econdition").unwrap();
    assert_eq!(condition.attribute("field-name").as_deref(), Some("enumTypeId"));
    assert_eq!(condition.attribute("value").as_deref(), Some("DataSourceType"));
    assert_eq!(
        source_find.attribute("entity-name").as_deref(),
        Some("moqui.basic.Enumeration")
    );

    for index in 3..=201 {
        insert(
            "moqui.basic.EnumerationType",
            &[("enumTypeId", &format!("T{index}")), ("description", "Type")],
        );
    }
    let many = assembled("ManyTypes");
    let many_line = sub_of(&many, "enumTypeId", "default-field")
        .first_named("text-line")
        .expect("more than 200 related rows is a text line");
    assert!(many_line.attribute("size").is_none(), "{}", many_line.write_string());
    assert!(sub_of(&many, "enumTypeId", "default-field")
        .first_named("drop-down")
        .is_none());

    let run = assembled("Run");
    assert!(sub_of(&run, "dataSourceTypeEnumId", "default-field")
        .first_named("drop-down")
        .is_some());
    assert!(sub_of(&run, "when", "default-field")
        .first_named("date-time")
        .unwrap()
        .attribute("default-value")
        .is_none());
    assert_eq!(
        sub_of(&run, "memberFrom", "default-field")
            .first_named("date-time")
            .unwrap()
            .attribute("default-value")
            .as_deref(),
        Some("${ec.l10n.format(ec.user.nowTimestamp, 'yyyy-MM-dd HH:mm')}")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
