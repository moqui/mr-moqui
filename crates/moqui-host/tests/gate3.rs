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

//! Gate 3: a real framework screen rendered in the text, csv, xml, html-print, html and qvt modes (with the qjs and qvue
//! companions) through the HTTP server with the seeded embedded-Postgres data, a form post through its transition, the
//! server-static template and `/actions` pair, the `/qapps` shell with its menu data, and the compatibility report from the
//! binary. The browser smoke is `gate3_browser.rs`, on the same setup (`common/mod.rs`).
//!
//! The fixture is `component://tools/screen/System/Security/ArtifactGroup/ArtifactGroupDetail.xml`.
//! It has what the gate asks for:
//!
//! - a `form-single` (`ArtifactGroupForm`, a `display` and a `text-line` over the `artifactGroup` value),
//! - a `form-list` with row-actions (`ArtifactAuthzList`, with `display-entity`, `drop-down` with
//!   `entity-options`, `link` and a `section-iterate` inside a field) and three more form-lists,
//! - sections (the `section-iterate` of entity filter sets), and containers (`container-box`),
//! - actions that are only `entity-find` and `entity-find-one`: no service, no mail, no search, no LLM.
//!
//! The other candidates either keep their forms in dialogs, which the text modes do not show, or
//! call services, search or the file system.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

mod common;

// the example component loads with the repository runtime; link its tool factory as the moqui binary does
use example as _;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use common::{
    basic, cookies_of, exchange, header, http_request, merge_cookies, session_token, sign_in, start, start_with,
    FIXTURE,
};

#[test]
fn artifact_group_detail_renders_as_text_csv_xml_and_html_print() {
    let gate = start("modes");
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");
    let get = |mode: &str| {
        let path = format!("/System/Security/ArtifactGroup/ArtifactGroupDetail.{mode}?artifactGroupId=SYSTEM_APP");
        let (status, headers, body) = exchange(address, &http_request("GET", &path, &[("Authorization", &john)], ""));
        assert_eq!(status, 200, "{mode}: {body}");
        assert!(!body.contains("Error rendering"), "{mode}: {body}");
        let content_type = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.clone());
        (content_type, body)
    };

    // text: fixed-width lines. A form-single line is the title right-aligned in 26 characters, a colon and
    // the value in 105. A form-list has column headers, a rule, and one line for each row.
    let (content_type, text) = get("text");
    assert_eq!(content_type.as_deref(), Some("text/plain;charset=UTF-8"));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], format!("{:>26}: {:<105}", "Artifact Group ID", "SYSTEM_APP"));
    assert_eq!(
        lines[1],
        format!("{:>26}: {:<105}", "Description", "System App (via root screen)")
    );
    let line = |wanted: &str| lines.contains(&wanted);
    assert!(line("Artifact Type     |Artifact Name     |Name Is Pattern   |Inherit Authz     |Filter Map        |Update            |Delete Link       "), "{text}");
    assert!(line("------------------+------------------+------------------+------------------+------------------+------------------+------------------"), "{text}");
    assert!(
        line("Screen            |component://tools/|N                 |Y                 |                  ||"),
        "{text}"
    );
    assert!(line("ID             |User Group     |Authz Type     |Action          |Authz Service Na|Entity Filter Se|Update          |Delete Link     "), "{text}");
    assert!(
        line("SYSTEM_APP_ADMI|Administrators ADMIN          |Always Allow (o|All             |                |||"),
        "{text}"
    );
    assert!(
        line("SYSTEM_APP_ALL |All Users (all ALL_USERS      |Deny (overrides|Update          |                |||"),
        "{text}"
    );
    assert!(line("User Group            |Max Hits Count       |Max Hits Duration    |Tarpit Duration      |Update               |Delete Link          "), "{text}");
    assert!(line("All Users (all users mALL_USERS             |100                  |60                   |30                   ||"), "{text}");

    // csv: a header line and a line for each row of each form-list, values quoted when they need it.
    let (content_type, csv) = get("csv");
    assert_eq!(content_type.as_deref(), Some("text/csv;charset=UTF-8"));
    assert_eq!(
        csv,
        "Artifact Type,Artifact Name,Name Is Pattern,Inherit Authz,Filter Map,Update,Delete Link\n\
         Screen,component://tools/screen/System.xml,N,Y,,\n\
         ID,User Group,Authz Type,Action,Authz Service Name,Entity Filter Sets,Update,Delete Link\n\
         SYSTEM_APP_ADMIN,Administrators (full access) - ADMIN,Always Allow (overrides Deny),All,,,\n\
         SYSTEM_APP_ALL,All Users (all users members by default) - ALL_USERS,Deny (overrides Allow),Update,,,\n\
         User Group,Max Hits Count,Max Hits Duration,Tarpit Duration,Update,Delete Link\n\
         All Users (all users members by default) - ALL_USERS,100,60,30,\n"
    );

    // xml: an element for each screen in the path, and for each form-list an element of entries whose
    // attributes are the displayed fields.
    let (content_type, xml) = get("xml");
    assert_eq!(content_type.as_deref(), Some("text/xml;charset=UTF-8"));
    assert_eq!(
        xml,
        "<webroot>\n<System>\n<Security>\n<ArtifactGroup>\n<ArtifactGroupDetail>\n\
         \x20   <ArtifactGroupMemberList>\n\
         \x20       <ArtifactGroupMemberListEntry artifactTypeEnumId=\"Screen\" artifactName=\"component://tools/screen/System.xml\" nameIsPattern=\"N\" inheritAuthz=\"Y\" filterMap=\"\" deleteLink=\" \"/>\n\
         \x20   </ArtifactGroupMemberList>\n\
         \x20   <ArtifactAuthzList>\n\
         \x20       <ArtifactAuthzListEntry artifactAuthzId=\"SYSTEM_APP_ADMIN\" userGroupId=\"Administrators (full access) - ADMIN\" authzTypeEnumId=\"Always Allow (overrides Deny)\" authzActionEnumId=\"All\" authzServiceName=\"\" entityFilterSets=\"\" deleteLink=\" \"/>\n\
         \x20       <ArtifactAuthzListEntry artifactAuthzId=\"SYSTEM_APP_ALL\" userGroupId=\"All Users (all users members by default) - ALL_USERS\" authzTypeEnumId=\"Deny (overrides Allow)\" authzActionEnumId=\"Update\" authzServiceName=\"\" entityFilterSets=\"\" deleteLink=\" \"/>\n\
         \x20   </ArtifactAuthzList>\n\
         \x20   <ArtifactTarpitList>\n\
         \x20       <ArtifactTarpitListEntry userGroupId=\"All Users (all users members by default) - ALL_USERS\" maxHitsCount=\"100\" maxHitsDuration=\"60\" tarpitDuration=\"30\" deleteLink=\" \"/>\n\
         \x20   </ArtifactTarpitList>\n\
         </ArtifactGroupDetail>\n</ArtifactGroup>\n</Security>\n</System>\n</webroot>\n"
    );

    // html-print: one document with print CSS. The form-single is a table of title and value, each form-list
    // is a table with a repeated header, and .xsl-fo, the older name of the mode, is the same body.
    let (content_type, print) = get("html-print");
    assert_eq!(content_type.as_deref(), Some("text/html;charset=UTF-8"));
    assert!(
        print.starts_with("<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\">"),
        "{print}"
    );
    assert_eq!(print.matches("<!DOCTYPE").count(), 1, "{print}");
    assert!(
        print.contains("@page letter-portrait { size: 8.5in 11in; margin: 0.25in; }"),
        "{print}"
    );
    assert!(
        print.contains("@page a4-landscape { size: 297mm 210mm; margin: 15mm; }"),
        "{print}"
    );
    assert!(print.contains("<body class=\"letter-portrait\">"), "{print}");
    assert!(print.ends_with("</body></html>\n"), "{print}");
    assert!(
        print.contains("<table class=\"field\"><tr><td class=\"field-title\">Artifact Group ID</td><td class=\"field-value\">SYSTEM_APP</td></tr></table>"),
        "{print}"
    );
    assert!(
        print.contains("<td class=\"field-value\">System App (via root screen)</td>"),
        "{print}"
    );
    assert_eq!(print.matches("<table class=\"form-list\">").count(), 3, "{print}");
    assert!(
        print.contains("<th><div style=\"text-align:left\">Artifact Type</div></th>"),
        "{print}"
    );
    assert!(
        print.contains("<td><div style=\"text-align:left\">component://tools/screen/System.xml</div></td>"),
        "{print}"
    );
    assert!(print.contains("SYSTEM_APP_ADMIN"), "{print}");
    assert!(print.contains("Always Allow (overrides Deny)"), "{print}");
    // userGroupDetail is an alias transition, so the link goes to the screen it names, with that screen's parameter
    assert!(print.contains("All Users (all users members by default) - <a href=\"/System/Security/UserGroup/UserGroupDetail?userGroupId=ALL_USERS\">ALL_USERS</a>"), "{print}");
    // a print document has no client: no moqui-html.js and no script at all
    assert!(!print.contains("moqui-html.js"), "{print}");
    assert!(!print.contains("<script"), "{print}");
    let (content_type, xsl_fo) = get("xsl-fo");
    assert_eq!(content_type.as_deref(), Some("text/html;charset=UTF-8"));
    assert_eq!(xsl_fo, print);
    // The renderMode parameter takes either name, and the layout comes from layoutMaster.
    let path = "/System/Security/ArtifactGroup/ArtifactGroupDetail?artifactGroupId=SYSTEM_APP&renderMode=xsl-fo&layoutMaster=legal-landscape";
    let (status, _, landscape) = exchange(address, &http_request("GET", path, &[("Authorization", &john)], ""));
    assert_eq!(status, 200, "{landscape}");
    assert!(
        landscape.contains("@page { size: 14in 8.5in; margin: 0.25in;"),
        "{landscape}"
    );
    assert!(landscape.contains("<body class=\"legal-landscape\">"), "{landscape}");
}

#[test]
fn artifact_group_detail_renders_as_html_with_the_shared_client() {
    let gate = start("html");
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");
    let path = "/System/Security/ArtifactGroup/ArtifactGroupDetail.html?artifactGroupId=SYSTEM_APP";
    let (status, headers, html) = exchange(address, &http_request("GET", path, &[("Authorization", &john)], ""));
    assert_eq!(status, 200, "{html}");
    assert_eq!(
        header(&headers, "content-type").as_deref(),
        Some("text/html;charset=UTF-8")
    );
    assert!(!html.contains("Error rendering"), "{html}");

    // the page shell: the shared style sheet, Font Awesome and the client script, with a version parameter
    assert!(html.starts_with("<!DOCTYPE html>\n<html lang=\"en-US\">"), "{html}");
    assert!(
        html.contains("<link rel=\"stylesheet\" href=\"/css/moqui-html.css?v="),
        "{html}"
    );
    assert!(
        html.contains("<link rel=\"stylesheet\" href=\"/libs/fontawesome/css/font-awesome.min.css?v="),
        "{html}"
    );
    assert!(html.contains("<script src=\"/js/moqui-html.js?v="), "{html}");
    assert!(
        html.contains("moqui-html.js?v=") && html.contains("\" defer></script>"),
        "{html}"
    );
    assert!(html.contains("<meta name=\"moquiSessionToken\" content=\""), "{html}");
    assert!(
        html.contains("<title>Artifact Groups - Artifact Group Detail</title>"),
        "{html}"
    );
    assert!(html.trim_end().ends_with("</body>\n</html>"), "{html}");

    // the form-single
    assert!(
        html.contains("<form name=\"ArtifactGroupForm\" id=\"ArtifactGroupForm\" method=\"post\" action=\"/System/Security/ArtifactGroup/ArtifactGroupDetail/updateArtifactGroup\">"),
        "{html}"
    );
    assert!(
        html.contains("<div class=\"field\"><label for=\"ArtifactGroupForm_artifactGroupId\">Artifact Group ID</label><div class=\"widget\"><span class=\"display String\">SYSTEM_APP</span>"),
        "{html}"
    );
    assert!(
        html.contains("<input type=\"text\" id=\"ArtifactGroupForm_description\" name=\"description\" value=\"System App (via root screen)\" size=\"40\">"),
        "{html}"
    );
    // the form-lists: row forms outside the table, inputs that point to them, and the rows
    assert!(
        html.contains("<table class=\"form-list\" id=\"ArtifactAuthzList_table\">"),
        "{html}"
    );
    assert_eq!(html.matches("<table class=\"form-list\"").count(), 3, "{html}");
    assert!(
        html.contains("<form name=\"ArtifactAuthzList_0\" id=\"ArtifactAuthzList_0\" method=\"post\""),
        "{html}"
    );
    assert!(
        html.contains("<span class=\"display String\">SYSTEM_APP_ADMIN</span>"),
        "{html}"
    );
    assert!(
        html.contains(
            "<select name=\"authzTypeEnumId\" id=\"ArtifactAuthzList_authzTypeEnumId_0\" form=\"ArtifactAuthzList_0\">"
        ),
        "{html}"
    );
    assert!(
        html.contains("<option value=\"AUTHZT_ALWAYS\" selected>Always Allow (overrides Deny)</option>"),
        "{html}"
    );
    // dialogs are native dialog elements, and the forms they hold post the session token
    assert!(
        html.contains("<dialog id=\"AddAuthzDialog\" class=\"container-dialog\""),
        "{html}"
    );
    assert!(html.contains("data-dialog=\"AddAuthzDialog\""), "{html}");
    // link forms saved for the end of the page are markup and not text
    assert!(
        html.contains(
            "<form method=\"post\" action=\"/System/Security/ArtifactGroup/ArtifactGroupDetail/deleteArtifactAuthz\""
        ),
        "{html}"
    );
    assert!(!html.contains("&lt;form"), "{html}");
    // menus go to the navbar through an attribute, not a script
    assert!(html.contains("data-move-to=\"header-menus\""), "{html}");
    // no jQuery, Bootstrap, Select2, Moment or inline per-widget script
    let lower = html.to_ascii_lowercase();
    for word in [
        "jquery",
        "bootstrap",
        "select2",
        "selectivity",
        "moment",
        "datetimepicker",
        "<script>",
        "form-control",
        "$(",
    ] {
        assert!(!lower.contains(word), "{word} in {html}");
    }
    assert_eq!(html.matches("<script").count(), 1, "{html}");
}

/// A form post of the html page runs the fixture's `updateArtifactGroup` transition with the session cookie and the token of the page,
/// and the response is the redirect of the transition to the screen, which then shows the new value. Without the token it is refused.
#[test]
fn a_form_post_runs_the_transition_and_returns_its_response() {
    let gate = start("post");
    let address = gate.server.address();
    let cookie = sign_in(address, "john.doe", "moqui");
    let page = format!("{FIXTURE}.html?artifactGroupId=SYSTEM_APP");
    let (status, headers, html) = exchange(address, &http_request("GET", &page, &[("Cookie", &cookie)], ""));
    assert_eq!(status, 200, "{html}");
    let cookie = merge_cookies(&cookie, &headers);
    let token = session_token(&html).expect("the page carries the session token");
    assert!(
        html.contains(&format!("action=\"{FIXTURE}/updateArtifactGroup\"")),
        "{html}"
    );
    let post = |body: &str| {
        exchange(
            address,
            &http_request(
                "POST",
                &format!("{FIXTURE}/updateArtifactGroup"),
                &[
                    ("Cookie", &cookie),
                    ("Content-Type", "application/x-www-form-urlencoded"),
                    ("Accept", "text/html"),
                ],
                body,
            ),
        )
    };

    // no token: refused before the service runs
    let (status, _, body) = post("artifactGroupId=SYSTEM_APP&description=No+token");
    assert_eq!(status, 401, "{body}");

    // the form as the browser sends it: the fields and the token
    let (status, headers, body) = post(&format!(
        "artifactGroupId=SYSTEM_APP&description=Changed+by+the+gate&submitButton=Update&moquiSessionToken={token}"
    ));
    assert_eq!(status, 302, "{body}");
    let location = header(&headers, "location").expect("the transition's response is a redirect");
    assert!(location.starts_with(FIXTURE), "{location}");
    assert!(location.contains("artifactGroupId=SYSTEM_APP"), "{location}");
    let (status, _, after) = exchange(address, &http_request("GET", &location, &[("Cookie", &cookie)], ""));
    assert_eq!(status, 200, "{after}");
    assert!(after.contains("value=\"Changed by the gate\""), "{after}");
    assert!(!after.contains("No token"), "{after}");

    // the qvt m-form posts with fetch and accepts JSON: the redirect is the screen URL in a JSON answer
    let (status, headers, body) = exchange(
        address,
        &http_request(
            "POST",
            &format!("{FIXTURE}/updateArtifactGroup"),
            &[
                ("Cookie", &cookie),
                ("Content-Type", "application/x-www-form-urlencoded"),
                ("Accept", "application/json"),
                ("X-CSRF-Token", &token),
            ],
            "artifactGroupId=SYSTEM_APP&description=Changed+by+fetch",
        ),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(header(&headers, "content-type").as_deref(), Some("application/json"));
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        json["screenUrl"],
        format!("{FIXTURE}?artifactGroupId=SYSTEM_APP"),
        "{body}"
    );
    assert_eq!(json["screenParameters"]["artifactGroupId"], "SYSTEM_APP", "{body}");
    assert_eq!(json["currentParameters"]["description"], "Changed by fetch", "{body}");
    assert!(json["currentParameters"].get("moquiSessionToken").is_none(), "{body}");

    // an error page under the html theme loads the client and the style sheet once, though both the theme and error.xml name them
    let (status, _, error) = exchange(
        address,
        &http_request(
            "GET",
            "/System/NoSuchScreen",
            &[("Cookie", &cookie), ("Accept", "text/html")],
            "",
        ),
    );
    assert_eq!(status, 404, "{error}");
    assert_eq!(error.matches("/js/moqui-html.js").count(), 1, "{error}");
    assert_eq!(error.matches("/css/moqui-html.css").count(), 1, "{error}");
}

/// The seed load reads each component's `data` directory: the tools app groups (`ToolsSecurityData.xml`, whose short-alias
/// elements need the alias lookup) and the default themes of webroot are rows after a load that names no location.
#[test]
fn the_seed_load_reads_the_data_directory_of_each_component() {
    use moqui_lib::object::Value;

    let gate = start("data");
    let factory = gate.server.factory();
    let ctx = factory.new_context();
    let entered = ctx.enter();
    let authz = ctx.disable_authz();
    let facade = factory.entity().unwrap();
    let text = |entity: &str, conditions: &[(&str, &str)], field: &str| -> Vec<String> {
        let mut find = facade.find(entity).unwrap();
        for (name, value) in conditions {
            find = find.eq(name, Value::string(*value));
        }
        find.list()
            .unwrap()
            .values()
            .iter()
            .map(|row| row.get(field).and_then(Value::as_str).unwrap_or("").to_string())
            .collect()
    };
    // tools: the groups, their members through the `artifacts` alias and their authorizations through `authz`
    assert_eq!(
        text(
            "moqui.security.ArtifactGroup",
            &[("artifactGroupId", "TOOLS_APP")],
            "description"
        ),
        ["Tools App (via root screen)"]
    );
    assert_eq!(
        text(
            "moqui.security.ArtifactGroupMember",
            &[("artifactGroupId", "TOOLS_APP")],
            "artifactName"
        ),
        ["component://tools/screen/Tools.xml"]
    );
    assert_eq!(
        text(
            "moqui.security.ArtifactAuthz",
            &[("artifactAuthzId", "MOQUI_API_ADMIN")],
            "artifactGroupId"
        ),
        ["MOQUI_API"]
    );
    // webroot: the html theme's style sheet and client, the Quasar theme, and the localized pagination text
    let resources = text(
        "moqui.screen.ScreenThemeResource",
        &[("screenThemeId", "DEFAULT")],
        "resourceValue",
    );
    assert!(
        resources.iter().any(|value| value.contains("moqui-html.css"))
            && resources.iter().any(|value| value.contains("moqui-html.js")),
        "{resources:?}"
    );
    assert_eq!(
        text(
            "moqui.screen.ScreenTheme",
            &[("screenThemeId", "DEFAULT_QUASAR")],
            "screenThemeTypeEnumId"
        ),
        ["STT_INTERNAL_QUASAR"]
    );
    assert!(!text("moqui.basic.LocalizedMessage", &[], "original").is_empty());
    drop(authz);
    drop(entered);
}

#[test]
fn the_login_page_and_the_root_render_as_html_and_a_failed_login_comes_back_with_its_message() {
    let gate = start("login");
    let address = gate.server.address();
    let get = |path: &str, cookie: &str| {
        let headers: Vec<(&str, &str)> = if cookie.is_empty() {
            vec![]
        } else {
            vec![("Cookie", cookie)]
        };
        exchange(address, &http_request("GET", path, &headers, ""))
    };
    let (status, headers, login) = get("/Login", "");
    assert_eq!(status, 200, "{login}");
    assert_eq!(
        header(&headers, "content-type").as_deref(),
        Some("text/html;charset=UTF-8")
    );
    assert!(
        login.contains("<link rel=\"stylesheet\" href=\"/css/moqui-html.css?v="),
        "{login}"
    );
    assert!(
        login.contains("<form method=\"post\" action=\"/Login/login\" class=\"form-signin\" id=\"login_form\">"),
        "{login}"
    );
    assert!(
        login.contains("<input type=\"password\" name=\"password\" required"),
        "{login}"
    );
    assert!(
        login.contains("<nav class=\"tabs form-signin\" data-tab-hash=\"login\">"),
        "{login}"
    );
    let lower = login.to_ascii_lowercase();
    assert!(!lower.contains("jquery") && !lower.contains("bootstrap"), "{login}");
    let cookie = cookies_of(&headers);

    // the root goes to the sign-in page when nobody is signed in
    let (status, headers, _) = get("/", "");
    assert_eq!(status, 302);
    assert_eq!(header(&headers, "location").as_deref(), Some("/Login"));

    // a wrong password: the transition redirects back, and the next page shows the message once with the user name kept
    let body = "username=john.doe&password=wrong";
    let post = http_request(
        "POST",
        "/Login/login",
        &[
            ("Cookie", &cookie),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        body,
    );
    let (status, headers, text) = exchange(address, &post);
    assert_eq!(status, 302, "{text}");
    assert_eq!(header(&headers, "location").as_deref(), Some("/Login"));
    let cookie = merge_cookies(&cookie, &headers);
    let (status, _, again) = get("/Login", &cookie);
    assert_eq!(status, 200, "{again}");
    assert!(
        again.contains("<div class=\"alert\" data-type=\"danger\" role=\"alert\">"),
        "{again}"
    );
    assert!(
        again.contains("name=\"username\" type=\"text\" value=\"john.doe\""),
        "{again}"
    );
    let (_, _, third) = get("/Login", &cookie);
    assert!(!third.contains("class=\"alert\""), "{third}");

    // the same login as a multipart form: the fields are parameters, and a body over the cap is refused before any screen runs
    let multipart = |name: &str, extra: &str| {
        let body = format!(
            "--B0\r\nContent-Disposition: form-data; name=\"username\"\r\n\r\njohn.doe\r\n--B0\r\nContent-Disposition: form-data; name=\"password\"\r\n\r\nwrong\r\n{extra}--B0--\r\n"
        );
        let request = http_request(
            "POST",
            "/Login/login",
            &[
                ("Cookie", &cookie),
                ("Content-Type", "multipart/form-data; boundary=B0"),
            ],
            &body,
        );
        let _ = name;
        exchange(address, &request)
    };
    let (status, headers, text) = multipart("fields", "");
    assert_eq!(status, 302, "{text}");
    assert_eq!(header(&headers, "location").as_deref(), Some("/Login"));
    moqui_lib::env::set_property("webapp_max_upload", "1k");
    let big = format!(
        "--B0\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"a.txt\"\r\n\r\n{}\r\n",
        "x".repeat(2000)
    );
    let (status, _, text) = multipart("big", &big);
    moqui_lib::env::clear_property("webapp_max_upload");
    assert_eq!(status, 413, "{text}");

    // the applications screen is html for a signed-in user, with the navbar the shell builds
    let john = basic("john.doe", "moqui");
    let (status, headers, apps) = exchange(address, &http_request("GET", "/apps", &[("Authorization", &john)], ""));
    assert_eq!(status, 200, "{apps}");
    // the page is on the screen history of the session, which the navbar's menu lists
    let session = cookies_of(&headers);
    let (_, _, apps_again) = exchange(
        address,
        &http_request("GET", "/apps", &[("Authorization", &john), ("Cookie", &session)], ""),
    );
    assert!(
        apps_again.contains("<li><a href=\"/apps/AppList\"><i class=\"fa fa-link\"></i> AppList</a></li>"),
        "{apps_again}"
    );
    assert!(apps.contains("<header><nav class=\"navbar\">"), "{apps}");
    assert!(apps.contains("<ul id=\"header-menus\"></ul>"), "{apps}");
    assert!(apps.contains("data-theme-toggle"), "{apps}");
    assert!(
        apps.contains("<a href=\"/apps/System\" data-btn=\"primary\" data-size=\"lg\" class=\"app-list-link\">"),
        "{apps}"
    );
    assert!(apps.contains("<div id=\"apps-footer-content\">"), "{apps}");
}

#[test]
fn static_files_are_served_with_validators_compression_and_no_way_out() {
    let gate = start("static");
    let address = gate.server.address();
    let get = |path: &str, headers: &[(&str, &str)]| exchange(address, &http_request("GET", path, headers, ""));
    let (status, headers, css) = get("/css/moqui-html.css", &[]);
    assert_eq!(status, 200);
    assert_eq!(
        header(&headers, "content-type").as_deref(),
        Some("text/css; charset=UTF-8")
    );
    assert_eq!(
        header(&headers, "cache-control").as_deref(),
        Some("max-age=86400, must-revalidate, public")
    );
    assert!(css.contains("--primary"), "{css}");
    let tag = header(&headers, "etag").unwrap();
    let (status, headers304, body) = get("/css/moqui-html.css", &[("If-None-Match", &tag)]);
    assert_eq!(status, 304);
    assert!(body.is_empty());
    assert_eq!(header(&headers304, "etag").as_deref(), Some(tag.as_str()));
    let (status, _, versioned) = get("/js/moqui-html.js?v=abc", &[]);
    assert_eq!(status, 200);
    assert!(versioned.contains("moqui"), "{versioned}");
    let (_, headers, _) = get("/js/moqui-html.js?v=abc", &[]);
    assert_eq!(
        header(&headers, "content-type").as_deref(),
        Some("text/javascript; charset=UTF-8")
    );
    assert_eq!(
        header(&headers, "cache-control").as_deref(),
        Some("public, max-age=31536000, immutable")
    );
    let (status, headers, _) = get("/favicon.ico", &[]);
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "content-type").as_deref(), Some("image/x-icon"));
    // HEAD has the headers and no body
    let (status, headers, body) = exchange(address, &http_request("HEAD", "/css/moqui-html.css", &[], ""));
    assert_eq!(status, 200);
    assert!(body.is_empty());
    assert_eq!(header(&headers, "etag").as_deref(), Some(tag.as_str()));
    // one byte range, not compressed even when the client takes gzip; a range past the end is 416; an If-Range of
    // another version gets the whole file
    let (status, headers, part) = get(
        "/css/moqui-html.css",
        &[("Range", "bytes=2-11"), ("Accept-Encoding", "gzip")],
    );
    assert_eq!(status, 206);
    assert_eq!(part.len(), 10, "{part}");
    assert_eq!(part, css[2..12]);
    let content_range = header(&headers, "content-range").unwrap();
    assert!(content_range.starts_with("bytes 2-11/"), "{content_range}");
    assert!(header(&headers, "content-encoding").is_none());
    let (status, headers, _) = get("/css/moqui-html.css", &[("Range", "bytes=99999999-")]);
    assert_eq!(status, 416);
    assert!(header(&headers, "content-range").is_some_and(|value| value.starts_with("bytes */")));
    let (status, _, whole) = get(
        "/css/moqui-html.css",
        &[("Range", "bytes=0-9"), ("If-Range", "W/\"other\"")],
    );
    assert_eq!(status, 200);
    assert_eq!(whole, css);

    // gzip when the client takes it
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    stream
        .write_all(http_request("GET", "/css/moqui-html.css", &[("Accept-Encoding", "gzip")], "").as_bytes())
        .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|window| window == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    assert!(head.contains("content-encoding: gzip"), "{head}");
    assert!(raw.len() < css.len(), "{} compressed, {} plain", raw.len(), css.len());

    // nothing outside the directory, no hidden files, no screen or template sources
    for path in [
        "/css/../../../../etc/passwd",
        "/../../etc/passwd",
        "/css/%2e%2e/%2e%2e/%2e%2e/etc/passwd",
        "/css/..%2f..%2f..%2fetc%2fpasswd",
        "/css/..%5c..%5cetc%5cpasswd",
        "/.git/config",
        "/Login.html.fancy",
    ] {
        let (status, _, body) = get(path, &[]);
        assert!(status == 404 || status == 400, "{path}: {status} {body}");
        assert!(!body.contains("root:"), "{path}: {body}");
        assert!(!body.contains("<screen"), "{path}: {body}");
    }
}

/// The Vue compiler bundle of the vendored libraries.
fn vue_compiler() -> String {
    std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../runtime/base-component/webroot/screen/webroot/libs/vue/compiler-dom.cjs.js"),
    )
    .unwrap()
}

#[test]
fn artifact_group_detail_renders_as_qvt_and_answers_qjs_and_qvue() {
    let gate = start("qvt");
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");
    let get = |path: &str| exchange(address, &http_request("GET", path, &[("Authorization", &john)], ""));

    // qvt is the template of the screen under /apps, as the shell fetches it: the screen alone, with the request's values merged in
    let path = "/apps/System/Security/ArtifactGroup/ArtifactGroupDetail.qvt?artifactGroupId=SYSTEM_APP";
    let (status, headers, qvt) = get(path);
    assert_eq!(status, 200, "{qvt}");
    assert_eq!(
        header(&headers, "content-type").as_deref(),
        Some("text/html;charset=UTF-8")
    );
    assert_eq!(header(&headers, "cache-control").as_deref(), Some("private, no-store"));
    assert_eq!(header(&headers, "x-moqui-qvt").as_deref(), Some("merged-fragment"));
    assert!(!qvt.contains("Error rendering"), "{qvt}");
    // standalone: no document, no shell, no page header
    assert!(
        !qvt.contains("<html") && !qvt.contains("<body") && !qvt.contains("moqui-html.js"),
        "{qvt}"
    );
    // the form-single is an m-form whose fields start from fields-initial, and the widgets are wrappers
    assert!(
        qvt.contains("<m-form name=\"ArtifactGroupForm\" id=\"ArtifactGroupForm\" action=\"/apps/System/Security/ArtifactGroup/ArtifactGroupDetail/updateArtifactGroup\""),
        "{qvt}"
    );
    assert!(qvt.contains("'artifactGroupId':'SYSTEM_APP'"), "{qvt}");
    assert!(qvt.contains("'description':'System App (via root screen)'"), "{qvt}");
    assert!(qvt.contains("<m-text-line"), "{qvt}");
    assert!(qvt.contains("v-model=\"formProps.fields.description\""), "{qvt}");
    // the form-lists are div tables with a form for each row, and the authz type is a drop-down with its options
    assert!(qvt.contains("id=\"ArtifactAuthzList_table\""), "{qvt}");
    assert!(
        qvt.contains("<m-form name=\"ArtifactAuthzList_0\" id=\"ArtifactAuthzList_0\""),
        "{qvt}"
    );
    assert!(qvt.contains("'artifactAuthzId':'SYSTEM_APP_ADMIN'"), "{qvt}");
    // a display-entity followed by a link in one field: the display text is the entity text, and the link does not replace it
    assert!(
        qvt.contains("'userGroupId':'ADMIN','userGroupId_display':'Administrators (full access) - '"),
        "{qvt}"
    );
    assert!(
        qvt.contains("<m-drop-down name=\"authzTypeEnumId\" id=\"ArtifactAuthzList_authzTypeEnumId_0\""),
        "{qvt}"
    );
    assert!(
        qvt.contains("'value':'AUTHZT_ALWAYS','label':'Always Allow (overrides Deny)'"),
        "{qvt}"
    );
    assert!(qvt.contains("<m-container-dialog id=\"AddAuthzDialog\""), "{qvt}");
    // no raw Quasar input, and nothing of the jQuery stack
    for raw in [
        "<q-input",
        "<q-select",
        "<q-date",
        "<q-time",
        "jquery",
        "bootstrap",
        "select2",
        "moment",
    ] {
        assert!(!qvt.to_ascii_lowercase().contains(raw), "{raw} in {qvt}");
    }
    // the Vue compiler accepts it
    let errors = moqui_core::screen::vue_check::compile_errors(&vue_compiler(), std::slice::from_ref(&qvt)).unwrap();
    assert!(errors.is_empty(), "{errors:?}\n{qvt}");

    // qjs and qvue answer with the companion text the screen has, and the type of the mode. This screen has none.
    for (extension, content_type) in [
        ("qjs", "application/javascript;charset=UTF-8"),
        ("qvue", "text/html;charset=UTF-8"),
    ] {
        let path =
            format!("/apps/System/Security/ArtifactGroup/ArtifactGroupDetail.{extension}?artifactGroupId=SYSTEM_APP");
        let (status, headers, body) = get(&path);
        assert_eq!(status, 200, "{extension}: {body}");
        assert_eq!(header(&headers, "content-type").as_deref(), Some(content_type));
        assert_eq!(body, "", "{extension}");
    }

    // a screen with companions: the qvt is its template with the text safe from interpolation, the qjs the object of the
    // component, and the qvue a component file
    let (status, headers, template) = get("/apps/GateCompanion.qvt");
    assert_eq!(status, 200, "{template}");
    assert_eq!(header(&headers, "x-moqui-qvt").as_deref(), Some("merged-fragment"));
    assert_eq!(
        template,
        "<p class=\"text-inline\">Companion &#123;&#123;literal}} text</p>\n"
    );
    let (status, headers, qjs) = get("/apps/GateCompanion.qjs");
    assert_eq!(status, 200, "{qjs}");
    assert_eq!(
        header(&headers, "content-type").as_deref(),
        Some("application/javascript;charset=UTF-8")
    );
    assert_eq!(header(&headers, "cache-control").as_deref(), Some("private, no-store"));
    assert!(header(&headers, "x-moqui-qvt").is_none());
    assert_eq!(
        qjs,
        "define([], function () { return { data: function () { return { count: 1 }; } }; });"
    );
    let (status, headers, qvue) = get("/apps/GateCompanion.qvue");
    assert_eq!(status, 200, "{qvue}");
    assert_eq!(
        header(&headers, "content-type").as_deref(),
        Some("text/html;charset=UTF-8")
    );
    assert!(qvue.starts_with("<template><p>{{count}}</p></template>"), "{qvue}");
    assert!(qvue.contains("export default"), "{qvue}");

    // a .qvt request that names the client modes (moqui.loadComponent for a container's URL) gets the first of them the screen's
    // render-modes name, and X-Moqui-Qvt says which: the gallery's QuickTask has qvue and tui, no qvt. GateCompanion's modes
    // are all, so it stays qvt; without the header a qvue screen has no qvt.
    let with_modes = |path: &str| {
        exchange(
            address,
            &http_request(
                "GET",
                path,
                &[("Authorization", &john), ("X-Moqui-Client-Modes", "qvue,qjs")],
                "",
            ),
        )
    };
    let (status, headers, task) = with_modes("/apps/gallery/QuickTask.qvt?recordId=R1");
    assert_eq!(status, 200, "{task}");
    assert_eq!(header(&headers, "x-moqui-qvt").as_deref(), Some("qvue"));
    assert_eq!(header(&headers, "cache-control").as_deref(), Some("private, no-store"));
    assert!(task.contains("<template>") && task.contains("module.exports"), "{task}");
    let (status, _, body) = get("/apps/gallery/QuickTask.qvt");
    assert_eq!(status, 404, "{body}");
    let (status, headers, body) = with_modes("/apps/GateCompanion.qvt");
    assert_eq!(status, 200, "{body}");
    assert_eq!(header(&headers, "x-moqui-qvt").as_deref(), Some("merged-fragment"));
    assert_eq!(body, template);

    // unknown render modes and /vapps are not there
    let (status, _, _) = get("/apps/System/Security/ArtifactGroup/ArtifactGroupDetail.vuet?artifactGroupId=SYSTEM_APP");
    assert_eq!(status, 404);
    for path in ["/vapps", "/vapps/System", "/vapps.qvt"] {
        let (status, _, body) = get(path);
        assert_eq!(status, 404, "{path}: {body}");
    }
}

/// The server-static pair of Gate 3: the template holds no row data, and `/actions` returns the JSON package.
#[test]
fn artifact_group_detail_static_qvt_is_a_data_free_template_with_its_data_on_actions() {
    let gate = start_with("static", true);
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");
    let get = |path: &str, extra: &[(&str, &str)]| {
        let mut headers = vec![("Authorization", john.as_str())];
        headers.extend_from_slice(extra);
        exchange(address, &http_request("GET", path, &headers, ""))
    };
    let screen = "/apps/System/Security/ArtifactGroup/ArtifactGroupDetail";

    // the template: a script that registers a render function, cached publicly, with a validator
    let (status, headers, template) = get(&format!("{screen}.qvt?artifactGroupId=SYSTEM_APP"), &[]);
    assert_eq!(status, 200, "{template}");
    assert_eq!(header(&headers, "x-moqui-qvt").as_deref(), Some("static-template"));
    assert_eq!(
        header(&headers, "cache-control").as_deref(),
        Some("max-age=86400, must-revalidate, public")
    );
    assert_eq!(
        header(&headers, "content-type").as_deref(),
        Some("text/javascript;charset=UTF-8")
    );
    let tag = header(&headers, "etag").expect("a validator");
    assert!(
        template.contains(&format!(
            "(m.qvtStatic=m.qvtStatic||{{}})[\"{screen}\"]=function(Vue){{"
        )),
        "{template}"
    );
    // no row data and no user: the values of the artifact group, its members, the user and the seed are all absent
    for data in [
        "SYSTEM_APP",
        "System App (via root screen)",
        "john.doe",
        "John Doe",
        "Always Allow",
        "Administrators",
        "ALL_USERS",
        "component://tools/screen/System.xml",
    ] {
        assert!(!template.contains(data), "{data} in the template");
    }
    // the render function reads names from the scope: no with and no new Function
    for banned in ["with (", "with(", "new Function", "eval("] {
        assert!(!template.contains(banned), "{banned} in the template");
    }
    assert!(
        template.contains("_resolveComponent(\"m-static-form-list\")"),
        "{template}"
    );
    assert!(template.contains("_ctx.artifactGroup"), "{template}");
    // links and forms point at the screen's own path, resolved on the server, with no query and no host
    assert!(
        template.contains(&format!("action: \"{screen}/updateArtifactGroup\"")),
        "{template}"
    );
    // an alias transition's link goes to the screen it names, with that screen's parameter read from the row
    assert!(
        template.contains(
            "$mq.url('/apps/System/Security/UserGroup/UserGroupDetail',{},_ctx.$mq.merge({'userGroupId':_ctx.$scope['userGroupId']}"
        ),
        "{template}"
    );
    // a request with no user gets no template
    let (status, _, _) = exchange(address, &http_request("GET", &format!("{screen}.qvt"), &[], ""));
    assert_eq!(status, 401);
    // the same bytes for another request, and 304 for the validator
    let (_, other_headers, other) = get(&format!("{screen}.qvt?artifactGroupId=OTHER"), &[]);
    assert_eq!(other, template);
    assert_eq!(header(&other_headers, "etag").as_deref(), Some(tag.as_str()));
    let (status, _, body) = get(&format!("{screen}.qvt"), &[("If-None-Match", &tag)]);
    assert_eq!(status, 304);
    assert!(body.is_empty());

    // the package: the context after data prep, per user, never cached, without the lists of the form-lists
    let (status, headers, package) = get(&format!("{screen}/actions?artifactGroupId=SYSTEM_APP"), &[]);
    assert_eq!(status, 200, "{package}");
    assert_eq!(header(&headers, "cache-control").as_deref(), Some("private, no-store"));
    assert_eq!(header(&headers, "content-type").as_deref(), Some("application/json"));
    let json: serde_json::Value = serde_json::from_str(&package).unwrap();
    assert_eq!(json["artifactGroup"]["artifactGroupId"], "SYSTEM_APP");
    assert_eq!(json["artifactGroup"]["description"], "System App (via root screen)");
    for list in ["memberList", "authzList", "tarpitList"] {
        assert!(
            json.get(list).is_none(),
            "{list} is a form-list's list and is a request of its own: {package}"
        );
    }
    // the options of the drop-downs in the add dialogs were looked up on the server
    let options = json["mqExtra_CreateArtifactAuthz"]
        .as_object()
        .unwrap_or_else(|| panic!("no extras for the dialog form: {package}"));
    assert!(options.values().any(|value| value.is_array()), "{package}");
    assert!(!package.contains("currentPassword"), "{package}");

    // a form-list: its rows, with the text of display-entity and the options of its drop-downs
    let (status, headers, form) = get(
        &format!("{screen}/actions/ArtifactAuthzList?artifactGroupId=SYSTEM_APP"),
        &[],
    );
    assert_eq!(status, 200, "{form}");
    assert_eq!(header(&headers, "cache-control").as_deref(), Some("private, no-store"));
    let json: serde_json::Value = serde_json::from_str(&form).unwrap();
    let rows = json["authzList"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{form}");
    let admin = rows
        .iter()
        .find(|row| row["artifactAuthzId"] == "SYSTEM_APP_ADMIN")
        .unwrap();
    assert_eq!(admin["userGroupId_display"], "Administrators (full access) - ");
    assert!(admin["entityFilterSetList"].is_array(), "{form}");
    let key = "mqOpt_ArtifactAuthzList_authzTypeEnumId_0_0";
    let kinds = json[key].as_array().unwrap_or_else(|| panic!("no {key}: {form}"));
    assert!(
        kinds
            .iter()
            .any(|option| option["value"] == "AUTHZT_ALWAYS" && option["label"] == "Always Allow (overrides Deny)"),
        "{form}"
    );
    // the other two lists, and a name that is not a form
    let (status, _, members) = get(
        &format!("{screen}/actions/ArtifactGroupMemberList?artifactGroupId=SYSTEM_APP"),
        &[],
    );
    assert_eq!(status, 200, "{members}");
    let members: serde_json::Value = serde_json::from_str(&members).unwrap();
    assert_eq!(members["memberList"][0]["artifactTypeEnumId_display"], "Screen");
    let (status, _, _) = get(&format!("{screen}/actions/NoSuchForm?artifactGroupId=SYSTEM_APP"), &[]);
    assert_eq!(status, 404);
    // without a session there is no package
    let (status, _, _) = exchange(address, &http_request("GET", &format!("{screen}/actions"), &[], ""));
    assert_eq!(status, 401);
}

#[test]
fn the_qapps_shell_and_its_menu_data_work_for_a_signed_in_user() {
    let gate = start("qapps");
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");

    // nobody signed in: /qapps goes to the sign-in page
    let (status, headers, _) = exchange(address, &http_request("GET", "/qapps", &[], ""));
    assert_eq!(status, 302);
    assert_eq!(header(&headers, "location").as_deref(), Some("/Login"));

    // the shell for john.doe: the Quasar theme, the libraries in order, the Vue shell and the one settings block
    let (status, headers, shell) = exchange(
        address,
        &http_request("GET", "/qapps/System", &[("Authorization", &john)], ""),
    );
    assert_eq!(status, 200, "{shell}");
    assert_eq!(
        header(&headers, "content-type").as_deref(),
        Some("text/html;charset=UTF-8")
    );
    assert!(!shell.contains("Error rendering"), "{shell}");
    for sheet in [
        "/libs/fontawesome/css/font-awesome.min.css",
        "/libs/material-symbols/outlined.css",
        "/libs/material-icons/material-icons.css",
        "/libs/quasar/quasar.prod.css",
        "/css/WebrootVue.qvt.css",
    ] {
        assert!(
            shell.contains(&format!("<link rel=\"stylesheet\" href=\"{sheet}")),
            "{sheet} in {shell}"
        );
    }
    let mut at = 0;
    for script in [
        "/libs/vue/vue.global.prod.js",
        "/libs/quasar/quasar.umd.prod.js",
        "/libs/quasar/lang/en-US.umd.prod.js",
        "/libs/quasar/icon-set/material-symbols-outlined.umd.prod.js",
        "/js/MoquiLib.js",
        "/js/MoquiL10n.js",
        "/js/WebrootVue.qvt.render.js",
        "/js/WebrootVue.qvt.js",
    ] {
        let found = shell[at..]
            .find(&format!("<script src=\"{script}\""))
            .unwrap_or_else(|| panic!("{script} in order in {shell}"));
        at += found;
    }
    // the shell is the precompiled m-qapps-root component: the page has only the element it mounts on
    assert!(
        shell.contains("<div id=\"apps-root\" style=\"display:none;\"></div>"),
        "{shell}"
    );
    assert!(!shell.contains("<q-layout"), "{shell}");
    let marker = "<script type=\"application/json\" id=\"moqui-config\">";
    let start = shell.find(marker).expect("settings block") + marker.len();
    let end = shell[start..].find("</script>").unwrap() + start;
    let config: serde_json::Value = serde_json::from_str(&shell[start..end]).unwrap();
    assert_eq!(config["username"], "john.doe");
    assert_eq!(config["basePath"], "/apps");
    assert_eq!(config["linkBasePath"], "/qapps");
    assert_eq!(config["appRootPath"], "");
    assert_eq!(config["locale"], "en-US");
    assert_eq!(config["quasarLang"], "enUS");
    assert_eq!(config["darkMode"], false);
    assert_eq!(config["notificationEndpoint"], "/notws");
    assert_eq!(config["qvt"]["handles"][0], "merged-fragment");
    assert_eq!(config["qvt"]["handles"][1], "static-template");
    assert_eq!(config["qvt"]["build"], "compiler");
    // what the shell shows that the server knows: the theme's footer and the logout URL, and its labels in the user's language
    assert!(
        config["shell"]["footerHtml"]
            .as_str()
            .is_some_and(|html| html.contains("Moqui")),
        "{config}"
    );
    assert_eq!(config["shell"]["logoutUrl"], "/Login/logout");
    assert_eq!(config["labels"]["screenHistory"], "Screen History");
    assert_eq!(config["labels"]["signIn"], "Sign in");
    // the user snapshot of the browser ec: the account without password columns, and the preferences
    assert!(
        config["userAccount"].is_object() && config["preferences"].is_object(),
        "{config}"
    );
    assert!(
        !shell.contains("currentPassword") && !shell.contains("passwordSalt"),
        "{shell}"
    );
    assert!(
        config["sessionToken"].as_str().is_some_and(|token| !token.is_empty()),
        "{config}"
    );
    // no jQuery, Moment or inline script besides the data block
    let lower = shell.to_ascii_lowercase();
    for word in ["jquery", "moment", "bootstrap", "select2", "httpvueloader"] {
        assert!(!lower.contains(word), "{word} in {shell}");
    }
    assert_eq!(shell.matches("<script>").count(), 0, "{shell}");

    // menu data: a screen on the path for each name, the subscreens the user may view, and the target last
    let path = "/menuData/apps/System/Security/ArtifactGroup/ArtifactGroupDetail?artifactGroupId=SYSTEM_APP";
    let (status, headers, body) = exchange(address, &http_request("GET", path, &[("Authorization", &john)], ""));
    assert_eq!(status, 200, "{body}");
    assert_eq!(header(&headers, "content-type").as_deref(), Some("application/json"));
    let menu: serde_json::Value = serde_json::from_str(&body).unwrap();
    let names: Vec<&str> = menu
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["apps", "System", "Security", "ArtifactGroup", "ArtifactGroupDetail"],
        "{body}"
    );
    let system = &menu[1];
    assert_eq!(system["path"], "/apps/System");
    let subscreens = system["subscreens"].as_array().unwrap();
    assert!(
        subscreens
            .iter()
            .any(|item| item["name"] == "Security" && item["active"] == true),
        "{body}"
    );
    let last = menu.as_array().unwrap().last().unwrap();
    assert_eq!(last["path"], "/apps/System/Security/ArtifactGroup/ArtifactGroupDetail");
    assert_eq!(
        last["pathWithParams"],
        "/apps/System/Security/ArtifactGroup/ArtifactGroupDetail?artifactGroupId=SYSTEM_APP"
    );
    assert!(last["extraPathList"].as_array().unwrap().is_empty());
    assert!(
        last["renderModes"].as_array().unwrap().iter().any(|mode| mode == "all"),
        "{body}"
    );

    // not signed in is 401 and a path that is no screen is 404
    let (status, _, body) = exchange(address, &http_request("GET", "/menuData/apps/System", &[], ""));
    assert_eq!(status, 401, "{body}");
    let (status, _, body) = exchange(
        address,
        &http_request("GET", "/menuData/apps/NoSuchScreen", &[("Authorization", &john)], ""),
    );
    assert_eq!(status, 404, "{body}");

    // the catalog of messages for the browser
    let (status, _, body) = exchange(
        address,
        &http_request("GET", "/apps/l10nCatalog?locale=en_US", &[("Authorization", &john)], ""),
    );
    assert_eq!(status, 200, "{body}");
    let catalog: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(catalog["locale"], "en_US");
    assert!(catalog["messages"].is_object(), "{body}");
}

/// An anonymous token route (screen/token_route.rs): `/tk/<token>` is the public shell page with the theme and the
/// m-* components and no session user; `/tk/<token>.qvt` the route's screen with the check's answer in its context;
/// `/tk/<token>/files` a transition below it; a bad or revoked token gets the short page and nothing of the screen runs.
#[test]
fn a_token_route_renders_its_screen_in_the_public_shell_after_the_token_check() {
    use common::{LINK_TOKEN, REVOKED_TOKEN};
    let gate = common::start_token_route("tokenroute");
    let address = gate.server.address();
    let html = [("Accept", "text/html")];

    // the shell page: the theme's style sheets, the libraries, the components and the public entry, its own settings block
    let (status, _, shell) = exchange(address, &http_request("GET", &format!("/tk/{LINK_TOKEN}"), &html, ""));
    assert_eq!(status, 200, "{shell}");
    assert!(
        shell.contains("<link rel=\"stylesheet\" href=\"/css/WebrootVue.qvt.css"),
        "{shell}"
    );
    let mut at = 0;
    for script in [
        "/libs/vue/vue.global.prod.js",
        "/libs/quasar/quasar.umd.prod.js",
        "/js/MoquiLib.js",
        "/js/WebrootVue.qvt.js",
        "/js/WebrootPublic.qvt.js",
    ] {
        let found = shell[at..]
            .find(&format!("<script src=\"{script}\""))
            .unwrap_or_else(|| panic!("{script} in order in {shell}"));
        at += found;
    }
    assert!(shell.contains("<div id=\"public-root\""), "{shell}");
    assert!(
        !shell.contains("apps-root") && !shell.contains("id=\"moqui-config\""),
        "no /qapps shell: {shell}"
    );
    assert!(shell.contains("<title>Shared with you</title>"), "{shell}");
    let marker = "<script type=\"application/json\" id=\"moqui-public-config\">";
    let start = shell.find(marker).expect("settings block") + marker.len();
    let end = shell[start..].find("</script>").unwrap() + start;
    let config: serde_json::Value = serde_json::from_str(&shell[start..end]).unwrap();
    assert_eq!(config["route"]["basePath"], format!("/tk/{LINK_TOKEN}"));
    assert_eq!(config["context"]["note"], "For the gate");
    assert_eq!(config["context"]["sharerName"], "John Doe");
    assert!(
        config["context"].get("shareId").is_none(),
        "only what the page may show"
    );

    // the screen itself, as the shell loads it, with the check's answer in its context
    let (status, _, screen) = exchange(
        address,
        &http_request("GET", &format!("/tk/{LINK_TOKEN}.qvt"), &html, ""),
    );
    assert_eq!(status, 200, "{screen}");
    assert!(
        screen.contains("For the gate") && screen.contains("Shared by John Doe"),
        "{screen}"
    );
    // a transition below it: the public-share service checks the token again and lists the folder
    let (status, _, files) = exchange(
        address,
        &http_request("GET", &format!("/tk/{LINK_TOKEN}/files"), &[], ""),
    );
    assert_eq!(status, 200, "{files}");
    let files: serde_json::Value = serde_json::from_str(&files).unwrap();
    assert_eq!(files["fileList"][0]["name"], "docs", "{files}");

    // a bad token and a revoked one: the short page, and the transition does not run
    for token in ["NoSuchToken000000000000", REVOKED_TOKEN] {
        let (status, headers, page) = exchange(address, &http_request("GET", &format!("/tk/{token}"), &html, ""));
        assert_eq!(status, 404, "{page}");
        assert!(page.contains("This link has expired or was not found."), "{page}");
        assert_eq!(header(&headers, "cache-control").as_deref(), Some("no-store"));
        let (status, _, answer) = exchange(address, &http_request("GET", &format!("/tk/{token}/files"), &[], ""));
        assert_eq!(status, 404, "{answer}");
        assert!(!answer.contains("docs"), "{answer}");
    }
    // a path that is no token is not the route's
    let (status, _, _) = exchange(address, &http_request("GET", "/tk/x", &html, ""));
    assert_eq!(status, 404);

    // a POST to the page's URL itself (an RFC 8058 one-click): with the route's post-transition, that transition runs
    // after the token check; without one it is the shell page, as a GET is
    let form = [("Content-Type", "application/x-www-form-urlencoded")];
    let (status, _, posted) = exchange(
        address,
        &http_request(
            "POST",
            &format!("/tkp/{LINK_TOKEN}"),
            &form,
            "List-Unsubscribe=One-Click",
        ),
    );
    assert_eq!(status, 200, "{posted}");
    let posted: serde_json::Value = serde_json::from_str(&posted).unwrap_or_else(|err| panic!("{err}: {posted}"));
    assert_eq!(posted["fileList"][0]["name"], "docs", "{posted}");
    let (status, _, page) = exchange(address, &http_request("GET", &format!("/tkp/{LINK_TOKEN}"), &html, ""));
    assert_eq!(status, 200, "{page}");
    assert!(
        page.contains("<div id=\"public-root\""),
        "a GET is still the shell: {page}"
    );
    let (status, _, page) = exchange(
        address,
        &http_request(
            "POST",
            &format!("/tk/{LINK_TOKEN}"),
            &form,
            "List-Unsubscribe=One-Click",
        ),
    );
    assert_eq!(status, 200, "{page}");
    assert!(page.contains("<div id=\"public-root\""), "no post-transition: {page}");
    for token in ["NoSuchToken000000000000", REVOKED_TOKEN] {
        let (status, _, answer) = exchange(
            address,
            &http_request("POST", &format!("/tkp/{token}"), &form, "List-Unsubscribe=One-Click"),
        );
        assert_eq!(status, 404, "{answer}");
        assert!(!answer.contains("docs"), "{answer}");
    }
}
