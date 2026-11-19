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

//! Web protection on the seeded server behind a trusted proxy (the test, from loopback, with `X-Forwarded-For`): the
//! per-client parts are on because there is a trusted proxy, wrong Basic credentials counted by the login inside the
//! request worker ban that client at the edge and no other, a rate limit answers 429 with `Retry-After`, and
//! `/metrics` shows it. The failures also count toward the account's own lockout. A screen hit past the demo tarpit
//! (`ALL_SCREENS`, 120 a minute) is 429 with `Retry-After` too.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use crate::common::{basic, exchange, header, http_request, start, start_with_conf};

const S1: &str = "/rest/s1/moqui/basic/enumTypes";

const CONF: &str = r#"<webapp-list><webapp name="webroot" client-ip-header="X-Forwarded-For" trusted-proxies="127.0.0.1">
    <web-protection>
        <rate-limit name="all" enabled="false"/>
        <rate-limit name="enums" path="/rest/s1/moqui/basic/**" requests="1" seconds="60" burst="8"/>
        <ban name="auth-failure" threshold="3"/>
    </web-protection>
</webapp></webapp-list>"#;

#[test]
fn wrong_credentials_ban_the_client_and_rate_limits_answer_429() {
    if !crate::common::own_process() {
        return;
    }
    let gate = start_with_conf("web-protection", CONF, &[]);
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");
    let wrong = basic("john.doe", "not-the-password");
    let get = |client: &str, authorization: &str| {
        let headers = [("Authorization", authorization), ("X-Forwarded-For", client)];
        exchange(address, &http_request("GET", S1, &headers, ""))
    };

    let (status, _, body) = get("203.0.113.5", &john);
    assert_eq!(status, 200, "{body}");
    // a scanner's path is refused before routing
    let scan = http_request("GET", "/wp-login.php", &[("X-Forwarded-For", "203.0.113.5")], "");
    assert_eq!(exchange(address, &scan).0, 404);

    for _ in 0..3 {
        let (status, _, body) = get("203.0.113.5", &wrong);
        assert_eq!(status, 401, "{body}");
    }
    // banned now, with the right password too
    assert_eq!(get("203.0.113.5", &john).0, 403);
    // another client behind the same proxy is not
    let (status, _, body) = get("203.0.113.6", &john);
    assert_eq!(status, 200, "{body}");
    // the client the proxy names is the rightmost untrusted hop, not one the client wrote first
    let (status, _, body) = get("203.0.113.5, 203.0.113.7", &john);
    assert_eq!(status, 200, "{body}");

    // 203.0.113.6 has a burst of eight on these paths, and has used one
    for _ in 0..7 {
        assert_eq!(get("203.0.113.6", &john).0, 200);
    }
    let (status, headers, _) = get("203.0.113.6", &john);
    assert_eq!(status, 429);
    assert!(header(&headers, "retry-after").is_some(), "{headers:?}");

    let (status, _, metrics) = exchange(address, &http_request("GET", "/metrics", &[], ""));
    assert_eq!(status, 200, "{metrics}");
    assert!(metrics.contains("edge_banned_clients 1\n"), "{metrics}");
    assert!(metrics.contains("edge_rate_limited 1\n"), "{metrics}");
    assert!(metrics.contains("request_queue_full 0\n"), "{metrics}");

    // the account's own lockout counts failures in a row too (a Postgres count is a decimal): the fourth disables
    // it, from two clients so neither is banned first
    for client in ["203.0.113.8", "203.0.113.8", "203.0.113.10", "203.0.113.10"] {
        assert_eq!(get(client, &wrong).0, 401);
    }
    assert_eq!(get("203.0.113.9", &john).0, 401);
}

/// The demo `ALL_SCREENS` tarpit (more than 120 hits of one screen in 60 seconds locks it for 60): the target screen of
/// each request counts, so the hit past the limit locks the screen for that user and the next is 429 with
/// `Retry-After`.
#[test]
fn a_screen_hit_past_its_tarpit_is_429_with_retry_after() {
    if !crate::common::own_process() {
        return;
    }
    let gate = start("screen-tarpit");
    let address = gate.server.address();
    let get = |authorization: &str| {
        exchange(
            address,
            &http_request("GET", "/apps/AppList", &[("Authorization", authorization)], ""),
        )
    };
    let john = basic("john.doe", "moqui");
    for hit in 1..=121 {
        let (status, _, body) = get(&john);
        assert_eq!(status, 200, "hit {hit}: {body}");
    }
    let (status, headers, body) = get(&john);
    assert_eq!(status, 429, "{body}");
    let retry: u64 = header(&headers, "retry-after")
        .unwrap_or_else(|| panic!("no Retry-After: {headers:?}"))
        .parse()
        .unwrap();
    assert!(retry > 0 && retry <= 60, "Retry-After {retry}");
    // a browser gets the error page, with the same status and header
    let page = http_request(
        "GET",
        "/apps/AppList",
        &[("Authorization", &john), ("Accept", "text/html")],
        "",
    );
    let (status, headers, body) = exchange(address, &page);
    assert_eq!(status, 429, "{body}");
    assert!(header(&headers, "retry-after").is_some(), "{headers:?}");
}

#[test]
#[cfg(feature = "llm")]
fn rejected_stream_login_keys_count_toward_the_client_ban() {
    if !crate::common::own_process() {
        return;
    }
    let gate = start_with_conf("stream-key-ban", CONF, &[]);
    let address = gate.server.address();
    let key = {
        let context = gate.server.factory().new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        moqui_core::user::get_login_key(None).unwrap()
    };
    for _ in 0..4 {
        let request = http_request(
            "GET",
            "/assist/runs/missing/events",
            &[("X-Forwarded-For", "203.0.113.60"), ("login_key", &key)],
            "",
        );
        assert_eq!(
            exchange(address, &request).0,
            404,
            "valid keys must not contribute failures"
        );
    }
    for (client, path) in [
        ("203.0.113.61", "/notws"),
        ("203.0.113.62", "/assist/runs/missing/events"),
    ] {
        for _ in 0..3 {
            let request = http_request(
                "GET",
                path,
                &[
                    ("X-Forwarded-For", client),
                    ("login_key", "invalid-login-key"),
                    ("Upgrade", "websocket"),
                    ("Sec-WebSocket-Version", "13"),
                    ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
                ],
                "",
            );
            let request = request.replace("Connection: close", "Connection: Upgrade");
            let (status, _, body) = exchange(address, &request);
            assert_eq!(status, 401, "{path}: {body}");
        }
        let (status, _, _) = exchange(
            address,
            &http_request("GET", "/apps/AppList", &[("X-Forwarded-For", client)], ""),
        );
        assert_eq!(status, 403, "{path}");
    }
}

/// SEC-WEB-81: a rate limit and a path rule apply to the screen path a request reached, not only the path it came with:
/// a token route's transition with extra path after it (`/tk/<token>/files/x` is `/tk/<token>/files`), and a screen
/// through the FOP servlet (`/fop/apps/tools` is `/apps/tools`); a transition through the FOP servlet is not found.
#[test]
fn a_limit_and_a_rule_apply_to_the_path_a_request_reached() {
    if !crate::common::own_process() {
        return;
    }
    let token = crate::common::LINK_TOKEN;
    let data = format!(
        r#"<entity-facade-xml type="seed">
    <moqui.resource.ResourceDrive driveId="WPD" driveName="Reached drive" driveKindEnumId="RdkShared" rootLocation="dbresource://reacheddrive"
            ownerUserId="EX_JOHN_DOE"/>
    <moqui.resource.DbResource resourceId="WPR1" filename="reacheddrive" isFile="N"/>
    <moqui.resource.ResourceShare shareId="WPS1" driveId="WPD" path="" isDirectory="Y" isLink="Y" linkTokenHash="{}"
            permissionEnumId="RspView" note="For the guard" createdByUserId="EX_JOHN_DOE" openedCount="0"/>
</entity-facade-xml>"#,
        moqui_core::user::single_use_hash(token).unwrap()
    );
    let conf = r#"<webapp-list><webapp name="webroot" client-ip-header="X-Forwarded-For" trusted-proxies="127.0.0.1">
        <token-route name="tk" screen-location="component://example/screen/PublicLink.xml"
            token-service="org.moqui.resource.FileServices.check#PublicShareToken"/>
        <web-protection>
            <rate-limit name="all" enabled="false"/>
            <rate-limit name="files" path="/tk/*/files" requests="1" seconds="60" burst="1"/>
            <path-rule name="tools" action="deny" path="/apps/tools/**"/>
        </web-protection>
    </webapp></webapp-list>"#;
    let gate = crate::common::start_with_conf_properties("web-protection-reached", Some(&data), conf, &[]);
    let address = gate.server.address();
    let send = |method: &str, path: &str, client: &str| {
        exchange(address, &http_request(method, path, &[("X-Forwarded-For", client)], ""))
    };
    let (status, _, body) = send("GET", &format!("/tk/{token}/files"), "203.0.113.70");
    assert_eq!(status, 200, "{body}");
    let (status, headers, body) = send("GET", &format!("/tk/{token}/files/x"), "203.0.113.70");
    assert_eq!(status, 429, "{body}");
    assert!(header(&headers, "retry-after").is_some(), "{headers:?}");
    // the screen through the FOP servlet meets the rule of the screen
    let (status, _, body) = send("GET", "/fop/apps/tools", "203.0.113.71");
    assert_eq!(status, 404, "{body}");
    let (status, _, body) = send("POST", "/fop/Login/login", "203.0.113.72");
    assert_eq!(status, 404, "{body}");
}

/// SEC-WEB-91: behind a trusted proxy that appends to `X-Forwarded-Proto`, the request's scheme is the proxy's value,
/// read from the client address's hop, not the first one a client sent: a plain-http client that names `https` itself
/// gets the plain session cookie, and one the proxy says came over https gets the secure one.
#[test]
fn a_forwarded_scheme_is_the_proxys_not_one_the_client_sent_first() {
    if !crate::common::own_process() {
        return;
    }
    let gate = start_with_conf("web-protection-forwarded", CONF, &[]);
    let address = gate.server.address();
    let cookie_of = |proto: &str| {
        let headers = [("X-Forwarded-For", "203.0.113.90"), ("X-Forwarded-Proto", proto)];
        let (status, headers, body) = exchange(address, &http_request("GET", "/Login", &headers, ""));
        assert_eq!(status, 200, "{body}");
        header(&headers, "set-cookie").unwrap_or_default()
    };
    let spoofed = cookie_of("https, http");
    assert!(spoofed.starts_with("moqui.session="), "{spoofed}");
    let secure = cookie_of("https");
    assert!(secure.starts_with("__Host-moqui.session="), "{secure}");
}
