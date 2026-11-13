// A01 Broken Access Control over HTTP, ported from Moqui Java's pytest proofs (framework/test, the catalog in
// README.md): anonymous requests to the apps and REST, the REST transitions that were removed, a path with .. that is
// not normalized, the fixture users' limits (test/ClientFixtures.spec.js), and the login's returnTo. Passes on any
// conf; the features that need the fixture users skip without them, as the Python suite does.

// The fixture user can log in from here, else the feature skips. A session of its own, not logged out: a REST logout
// ends every session of the user (UserAccount.hasLoggedOut).
const requireFixture = (username, password) => {
    const res = session().post("/rest/login", { form: { username, password }, accept: "json", csrf: false })
    if (res.status == 401) skip(`${username} is not on this server: run test/ClientFixtures.spec.js on its database (moqui test framework --spec ClientFixtures --db existing), or use --serve`)
    return res
}
// A session logged in through the login form, as the Python screen_login.
const formSession = (username, password) => {
    const s = session()
    const res = s.formLogin(username, password)
    assert(res.status == 302, `the login form did not log ${username} in: HTTP ${res.status}`)
    return s
}

// test_a01_access.py: test_tools_without_session_is_not_the_tool
feature("A01 Tools without a session redirects to the login, not the tool", {
    when() { return { res: session().get("/apps/tools/Home") } },
    then({ res }) {
        res.status == 302
        res.location.includes("/Login")
        !res.text.toLowerCase().includes("auto screens")
    },
})

// test_a01_access.py: test_rest_e1_without_credentials_is_401, test_rest_s1_without_credentials_is_unauthorized
feature("A01 #path without credentials is 401", {
    where: [{ path: "/rest/e1/moqui.basic.Enumeration" }, { path: "/rest/s1/moqui/basic/enums" }],
    when({ path }) { return { res: session().get(path) } },
    then({ res }) {
        res.status == 401
        !res.text.includes("enumId")
    },
})

// test_a01_access.py: test_removed_rest_api_key_is_not_found, test_removed_rest_moqui_session_token_is_not_found;
// test_api_rest.py: test_rest_userInfo_transition_is_gone. General JSON-RPC is not served (A2A and MCP have their own),
// so a JSON-RPC call to /rpc/json is 404 too.
feature("A01 the removed endpoint #method #path is 404, not merely unauthenticated", {
    where: [
        { method: "GET", path: "/rest/api_key" },
        { method: "GET", path: "/rest/moquiSessionToken" },
        { method: "GET", path: "/rest/userInfo" },
        { method: "POST", path: "/rpc/json", json: { jsonrpc: "2.0", id: 1, method: "moqui.basic.find#Enumeration", params: {} } },
    ],
    when({ method, path, json }) { return { res: session().request(method, path, json ? { json, csrf: false } : {}) } },
    then({ res }) {
        res.status == 404
    },
})

// test_a01_more.py: test_apps_get_preferences_without_login_is_401
feature("A01 getPreferences without a login is 401 with no preferences", {
    when() { return { res: session().get("/apps/getPreferences", { params: { keyRegexp: ".*" } }) } },
    then({ res }) {
        res.status == 401
        !res.text.toLowerCase().includes("mantle.content.root")
        !res.text.toLowerCase().includes("authorize") || res.text.toLowerCase().includes("authentication")
    },
})

// test_a01_more.py: test_static_path_dotdot_does_not_escape. The path goes as it is (http.raw); it resolves to
// /Login.xml, the Login screen in the xml render mode, and the screen's source is not served.
feature("A01 a static path with .. does not serve screen XML", {
    when() { return { res: http.raw("GET /js/../../Login.xml HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n", { cookies: false }) } },
    then({ res }) {
        !res.text.includes("<screen")
        !res.text.includes("<transition")
        !res.text.toLowerCase().includes("require-session-token")
    },
})

// test_a01_more.py: test_none_user_cannot_open_tools
feature("A01 a user with no authorization cannot open Tools", {
    given() { requireFixture("sec.none.only", "SecNone1!!") },
    when() { return { res: formSession("sec.none.only", "SecNone1!!").get("/apps/tools/Home") } },
    then({ res }) {
        res.status == 403
        !res.text.toLowerCase().includes("auto screens")
    },
})

// test_a01_more.py: test_view_only_cannot_open_sql_runner
feature("A01 a VIEW-only user cannot open the SQL Runner", {
    given() { requireFixture("sec.view.only", "SecView1!!") },
    when() {
        const s = formSession("sec.view.only", "SecView1!!")
        return {
            tools: s.get("/apps/tools/Home"),
            res: s.get("/apps/tools/SqlTui", { params: { db: "group:transactional", sql: "SELECT 1" } }),
        }
    },
    then({ tools, res }) {
        // the control: the user is logged in and may view Tools
        tools.status == 200
        // the screen's actions refuse the user (SQL_RUNNER_WEB) and the page shows why, as in Java; no query ran
        res.status == 403 || res.text.includes("does not have permission to use the SQL Runner")
        !res.text.includes("Showing all")
        !res.text.includes("Query altered")
    },
})

// test_a01_redirect.py: test_login_returnTo_does_not_redirect_offsite
feature("A01 the login does not follow a returnTo off the site", {
    when() {
        const s = session()
        s.get("/Login", { params: { returnTo: "https://evil.example/phish" } })
        return { res: s.post("/Login/login", { form: { username: params.user ?? "john.doe", password: params.password ?? "moqui" } }) }
    },
    then({ res }) {
        res.status == 302
        !res.location.toLowerCase().includes("evil.example")
        !res.location.includes("/phish")
    },
})

// test_a01_redirect.py: test_login_returnTo_same_origin_path_is_allowed
feature("A01 the login follows a returnTo on the site", {
    when() {
        const s = session()
        s.get("/Login", { params: { returnTo: "/apps" } })
        return { res: s.post("/Login/login", { form: { username: params.user ?? "john.doe", password: params.password ?? "moqui" } }) }
    },
    then({ res }) {
        res.status == 302
        res.location.startsWith("/") || res.location.startsWith(server.url)
        res.location.includes("/apps")
    },
})

// From SecurityMessages.spec.js, merged here to share a spec run.
// A01 Broken Access Control for /rest/sm, ported from Moqui Java's pytest proofs (framework/test/test_api_rest.py and
// test_a01_more.py): SmatHmacSha256, SmatHmacSha256Timestamp (five minute window and replay), SmatNone, and an unknown
// type. message.rs answers 403 for a missing HMAC header, a bad signature, a timestamp outside the window, and a
// replay, the same codes as those proofs; a missing fixture remote is 400 "not valid" and the feature skips. No login.
// Passes on any conf.

const MESSAGE_TYPE = "SEC_SMT_TEST"
const REMOTE_HMAC = "SEC_SMR_HMAC"
const REMOTE_TS = "SEC_SMR_HMAC_TS"
const REMOTE_NONE = "SEC_SMR_NONE"
const SECRET = "sec-hmac-test-secret"
const HEADER = "X-Moqui-Signature"
const BODY = '{"probe":true}'

// 400 containing "not valid" is a missing SEC_SMR_* fixture, the Python skip. A refusal, including a bad signature's
// 403, is returned and asserted.
const postMessage = (remote, body, signature) => {
    const headers = { "Content-Type": "application/json" }
    if (signature != null) headers[HEADER] = signature
    const res = session().post(`/rest/sm/${MESSAGE_TYPE}/${remote}`, { body, headers, csrf: false })
    if (res.status == 400 && res.text.toLowerCase().includes("not valid")) {
        skip("the SEC_SMR_* remotes are not on this server: run test/ClientFixtures.spec.js on its database " +
            "(moqui test framework --spec ClientFixtures --db existing), or use --serve")
    }
    return res
}

// test_api_rest.py: test_system_message_hmac_missing_header_is_403
feature("A01 a /rest/sm HMAC post with no signature header is 403", {
    when() { return { res: postMessage(REMOTE_HMAC, BODY) } },
    then({ res }) {
        res.status == 403
        res.text.toLowerCase().includes("hmac") || res.text.toLowerCase().includes("header")
    },
})

// test_api_rest.py: test_system_message_hmac_bad_signature_is_403
feature("A01 a /rest/sm HMAC post with a bad signature is 403", {
    when() { return { res: postMessage(REMOTE_HMAC, BODY, "not-a-valid-hmac") } },
    then({ res }) {
        res.status == 403
        res.text.toLowerCase().includes("hmac")
    },
})

// test_api_rest.py: test_system_message_hmac_valid_does_not_require_login
feature("A01 a valid /rest/sm HMAC signature needs no login", {
    when() { return { res: postMessage(REMOTE_HMAC, BODY, hmac.sha256Base64(SECRET, BODY)) } },
    then({ res }) {
        res.status == 200
        !res.text.toLowerCase().includes("hmac verify failed")
        res.text.toLowerCase().includes("systemmessageidlist")
    },
})

// test_api_rest.py: test_system_message_hmac_timestamp_outside_window_is_403
feature("A01 a /rest/sm timestamp HMAC outside the window is 403", {
    when() {
        // 400 seconds ago, as in the Python proof: the receive window is five minutes.
        const ts = Math.floor(Date.now() / 1000) - 400
        return { res: postMessage(REMOTE_TS, BODY, hmac.sha256Timestamp(SECRET, BODY, ts)) }
    },
    then({ res }) {
        res.status == 403
        res.text.toLowerCase().includes("hmac") || res.text.toLowerCase().includes("timestamp")
    },
})

// test_api_rest.py: test_system_message_hmac_timestamp_in_window_is_accepted
// Positive control: a remote that rejected every timestamp would still pass the refusals above.
feature("A01 a /rest/sm timestamp HMAC inside the window is accepted", {
    when() {
        const ts = Math.floor(Date.now() / 1000)
        return { res: postMessage(REMOTE_TS, BODY, hmac.sha256Timestamp(SECRET, BODY, ts)) }
    },
    then({ res }) {
        res.status == 200
        res.text.toLowerCase().includes("systemmessageidlist")
    },
})

// test_api_rest.py: test_system_message_smat_none_accepts_anonymous
feature("A01 a /rest/sm SmatNone remote accepts an anonymous post", {
    when() { return { res: postMessage(REMOTE_NONE, BODY) } },
    then({ res }) {
        res.status == 200
        res.text.toLowerCase().includes("systemmessageidlist")
    },
})

// test_api_rest.py: test_system_message_hmac_timestamp_replay_is_rejected
feature("A01 a replayed /rest/sm timestamp HMAC is rejected", {
    when() {
        // Signed once, at one unix second, and posted twice. A different body from the in-window proof, so the two
        // do not share a replay key when they run in the same second.
        const body = '{"probe":"replay"}'
        const ts = Math.floor(Date.now() / 1000)
        const header = hmac.sha256Timestamp(SECRET, body, ts)
        const first = postMessage(REMOTE_TS, body, header)
        const second = postMessage(REMOTE_TS, body, header)
        return { first, second }
    },
    then({ first, second }) {
        first.status == 200
        first.text.toLowerCase().includes("systemmessageidlist")
        second.status == 403
        second.text.toLowerCase().includes("hmac")
        second.text.toLowerCase().includes("replay")
    },
})

// test_a01_more.py: test_system_message_unknown_remote_is_rejected
// 400 "Message type NotAType not valid" here (the type is checked before the remote). Python accepts 400, 401, or 403.
feature("A01 an unknown /rest/sm remote is rejected", {
    when() {
        return { res: session().post("/rest/sm/NotAType/NotARemote", {
            body: BODY, headers: { "Content-Type": "application/json", Accept: "application/json" }, csrf: false,
        }) }
    },
    then({ res }) {
        res.status == 400
        res.text.toLowerCase().includes("not valid")
    },
})

// From SecurityProxy.spec.js, merged here to share a spec run.
// A01 Broken Access Control for the shipped proxies, ported from framework/test/test_a01_more.py. Java's /elastic
// (ElasticRemote) is /search with SearchRemote; Java's /kibana (KibanaRemote) is /dashboard with DashboardRemote
// (SearchAuthFilter, DashboardAuthFilter, SearchProxy, DashboardProxy). kibana_host defaults to 127.0.0.1, so
// DashboardProxy is mounted and an anonymous request is 401 from the filter, not an open proxy. An empty host makes
// proxyTo "http://:5601", which ProxyServlet answers 404 only after that same filter. Features that need
// sec.none.only skip without that fixture user, as the Python suite does.

// The fixture user can log in from here, else the feature skips. A session of its own, not logged out: a REST logout
// ends every session of the user (UserAccount.hasLoggedOut).
// A session logged in through the login form, as the Python screen_login.
// test_a01_more.py: test_elastic_proxy_without_login_is_401
feature("A01 /search without a login is 401", {
    when() { return { res: session().get("/search/") } },
    then({ res }) {
        res.status == 401
    },
})

// test_a01_more.py: test_elastic_proxy_logged_in_without_permission_is_denied
feature("A01 a logged-in user without SearchRemote is denied /search", {
    given() { requireFixture("sec.none.only", "SecNone1!!") },
    when() {
        const s = formSession("sec.none.only", "SecNone1!!")
        return {
            probe: s.get("/apps/getPreferences", { params: { keyRegexp: "x" } }),
            res: s.get("/search/"),
        }
    },
    then({ probe, res }) {
        probe.status != 401
        !(probe.status == 302 || probe.status == 303) || !(probe.location ?? "").toLowerCase().includes("login")
        res.status == 403
        res.text.toLowerCase().includes("searchremote") || res.text.toLowerCase().includes("permission")
    },
})

// test_a01_more.py: test_kibana_proxy_without_login_is_401
feature("A01 /dashboard without a login is 401", {
    when() { return { res: session().get("/dashboard/") } },
    then({ res }) {
        res.status == 401
    },
})

// test_a01_more.py: test_elastic_proxy_basic_auth_without_permission_is_403
// AuthFilter takes Basic credentials from the request, not only a session cookie.
feature("A01 Basic auth without SearchRemote is 403 on /search", {
    given() { requireFixture("sec.none.only", "SecNone1!!") },
    when() { return { res: session().get("/search/", { basic: ["sec.none.only", "SecNone1!!"] }) } },
    then({ res }) {
        res.status == 403
    },
})

// test_a01_more.py: test_elastic_proxy_bad_basic_auth_is_401
feature("A01 bad Basic auth on /search is 401", {
    given() { requireFixture("sec.none.only", "SecNone1!!") },
    when() { return { res: session().get("/search/", { basic: ["sec.none.only", "definitely-wrong-password"] }) } },
    then({ res }) {
        res.status == 401
    },
})

// test_a01_more.py: test_elastic_proxy_api_key_header_garbage_is_401
feature("A01 a garbage api_key on /search does not authenticate", {
    when() { return { res: session().get("/search/", { apiKey: "not-a-real-key" }) } },
    then({ res }) {
        res.status == 401 || res.status == 403
    },
})

// test_a01_more.py: test_elastic_proxy_basic_auth_with_permission_reaches_the_cluster
// Java required HTTP 200 and a cluster_name or version body, and skipped when the cluster was down or the user
// lacked ElasticRemote. john.doe is in ADMIN, which seed data grants SearchRemote. The default search backend is
// postgres, so requireElasticApi answers 404 after the filter; a down OpenSearch is a proxy error, not an auth
// failure. This proof only asserts the request got past authorization (not 401 and not 403). A live cluster body
// is not required.
feature("A01 Basic auth with SearchRemote gets past authorization on /search", {
    when() {
        return { res: session().get("/search/", { basic: [params.user ?? "john.doe", params.password ?? "moqui"] }) }
    },
    then({ res }) {
        res.status != 401
        res.status != 403
    },
})

// From SecurityStatus.spec.js, merged here to share a spec run.
// A01 Broken Access Control for /status over HTTP, ported from Moqui Java's pytest proofs
// (framework/test/test_a01_more.py; the catalog in README.md). Each request connects from loopback, as that suite
// does. With webapp_client_ip_header empty, X-Forwarded-For and Forwarded are ignored, so they neither hide the
// loopback JSON nor grant the sensitive map. /status calls getStatusMap() with no flag, so DataSources is omitted;
// Heap and VmVendor are Java keys this server never sends. Passes on any conf.

// test_a01_more.py: test_status_from_loopback_is_json
feature("A01 /status from loopback is JSON", {
    when() { return { res: session().get("/status") } },
    then({ res }) {
        res.status == 200
        res.contentType.includes("json")
        res.text.trim().startsWith("{")
        // Utilization is this server's load key. Heap is a Java key it never sends, so it is absent, not required.
        res.text.toLowerCase().includes("utilization")
        !res.text.toLowerCase().includes("heap")
        // DataSources is the sensitive key this map can carry; the public call leaves it out. VmVendor is not a key.
        !res.text.toLowerCase().includes("datasources")
        !res.text.toLowerCase().includes("vmvendor")
        res.json().Utilization != null
        res.json().DataSources == null
        res.json().Heap == null
        res.json().VmVendor == null
    },
})

// test_a01_more.py: test_status_x_forwarded_for_does_not_grant_access_from_spoofed_loopback
feature("A01 X-Forwarded-For does not grant a spoofed loopback on /status", {
    when() { return { res: session().get("/status", { headers: { "X-Forwarded-For": "8.8.8.8" } }) } },
    then({ res }) {
        // still the loopback map: the header is not the client address, and it adds no DataSources or VmVendor
        res.status == 200
        res.contentType.includes("json")
        res.text.trim().startsWith("{")
        res.text.toLowerCase().includes("utilization")
        !res.text.toLowerCase().includes("heap")
        !res.text.toLowerCase().includes("datasources")
        !res.text.toLowerCase().includes("vmvendor")
        res.json().Utilization != null
        res.json().DataSources == null
        res.json().Heap == null
        res.json().VmVendor == null
    },
})

// test_a01_more.py: test_status_forwarded_for_header_does_not_hide_loopback
feature("A01 Forwarded does not hide /status from loopback", {
    when() { return { res: session().get("/status", { headers: { Forwarded: "for=8.8.8.8" } }) } },
    then({ res }) {
        // the header does not hide the loopback JSON, and DataSources stays out
        res.status == 200
        res.contentType.includes("json")
        res.text.trim().startsWith("{")
        res.text.toLowerCase().includes("utilization")
        !res.text.toLowerCase().includes("heap")
        !res.text.toLowerCase().includes("datasources")
        !res.text.toLowerCase().includes("vmvendor")
        res.json().Utilization != null
        res.json().DataSources == null
        res.json().Heap == null
        res.json().VmVendor == null
    },
})

// From SecurityWebSocket.spec.js, merged here to share a spec run.
// A01 Broken Access Control on the notification socket (/notws), ported from Moqui Java's pytest proofs
// (framework/test/test_a01_ws.py; the catalog in README.md). An anonymous upgrade is 401 here, where Java accepts it
// and ignores the null user, so the foreign-origin proof uses a logged-in session: the origin, not the missing user,
// must be what refuses it. That proof needs a server that allows no foreign origin (the production conf) and skips on
// the dev conf, which allows every origin; SecurityCors fails there. The origin is compared whole, scheme, host and
// port, where Java compares the host only.

const EVIL_ORIGIN = "https://evil.example"
const HOST = server.host.includes(":") ? `[${server.host}]` : server.host
const OTHER_PORT_ORIGIN = `${server.secure ? "https" : "http"}://${HOST}:${server.port == 65535 ? 65534 : server.port + 1}`
const OTHER_SCHEME_ORIGIN = `${server.secure ? "http" : "https"}://${HOST}:${server.port}`

// test_a01_ws.py: test_websocket_evil_origin_is_rejected
feature("A01 a /notws upgrade from a foreign Origin is refused, with a logged-in session", {
    given() {
        const probe = session().get("/Login", { headers: { Origin: EVIL_ORIGIN } })
        if (probe.status != 401) skip("this server allows every origin (the dev conf): run against MoquiProductionConf.xml or " +
            "MoquiLoadTestConf.xml, as --serve does")
    },
    when() {
        const s = session({ user: params.user ?? "john.doe", password: params.password ?? "moqui" })
        const evil = ws.connect("/notws", { session: s, origin: EVIL_ORIGIN })
        const evilOpened = evil.open
        evil.close()
        // the control: the same session opens it from the server's own origin
        const own = ws.connect("/notws", { session: s, origin: server.url.replace(/\/+$/, "") })
        const ownOpened = own.open
        own.close()
        return { evil, evilOpened, own, ownOpened }
    },
    then({ evil, evilOpened, own, ownOpened }) {
        !evilOpened
        evil.status == 403
        ownOpened
        own.status == 101
    },
})

// Not in Java, which compares the host only and so opens these
feature("A01 a /notws upgrade from the server's host on #which is refused, with a logged-in session", {
    where: [
        { which: "another port", origin: OTHER_PORT_ORIGIN },
        { which: "another scheme", origin: OTHER_SCHEME_ORIGIN },
    ],
    given() {
        const probe = session().get("/Login", { headers: { Origin: EVIL_ORIGIN } })
        if (probe.status != 401) skip("this server allows every origin (the dev conf): run against MoquiProductionConf.xml or " +
            "MoquiLoadTestConf.xml, as --serve does")
    },
    when({ origin }) {
        const s = session({ user: params.user ?? "john.doe", password: params.password ?? "moqui" })
        const socket = ws.connect("/notws", { session: s, origin })
        const opened = socket.open
        socket.close()
        return { socket, opened }
    },
    then({ socket, opened }) {
        !opened
        socket.status == 403
    },
})

// Replaces test_a01_ws.py: test_notws_anonymous_upgrade_accepts_or_closes (Java accepts it)
feature("A01 an anonymous /notws upgrade is 401", {
    when() {
        const socket = ws.connect("/notws", { session: session() })
        const opened = socket.open
        socket.close()
        return { socket, opened }
    },
    then({ socket, opened }) {
        !opened
        socket.status == 401
    },
})
