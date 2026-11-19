// OWASP ASVS 5.0.0 V3 Web Frontend Security: the framework's proofs, each feature filed under the requirement its name
// starts with (docs/security/proofs/V3-Web-Frontend-Security.md). Ported from Moqui Java's pytest proofs; a comment
// names the Python test, as in test/client.
//
// The cross-origin proofs need a server that allows no foreign origin, which is the default (an empty
// webapp-list/webapp[webroot]@allow-origins): the production and load test confs. On the dev conf, which allows
// localhost on any port, they fail, as they should. A preflight answers 204, and the origin is compared whole, scheme,
// host and port (Java compares the host only). The session cookie is moqui.session, __Host-moqui.session over https.

const SESSION_COOKIE = server.secure ? "__Host-moqui.session" : "moqui.session"
const TOKEN_ERROR = "Session token is missing or is not valid"
const EVIL_ORIGIN = "https://evil.example"
const EVIL = "https://evil.example/phish"
const OWN_ORIGIN = server.url.replace(/\/+$/, "").toLowerCase()
const HOST = server.host.includes(":") ? `[${server.host}]` : server.host
const OTHER_PORT_ORIGIN = `${server.secure ? "https" : "http"}://${HOST}:${server.port == 65535 ? 65534 : server.port + 1}`
const OTHER_SCHEME_ORIGIN = `${server.secure ? "http" : "https"}://${HOST}:${server.port}`

// The account a proof signs in with: the run's (--param user=NAME, its password in MOQUI_TEST_PASSWORD), else in the
// fixtures profile the demo data's john.doe.
const account = () => {
    if (params.user) return { username: params.user, password: params.password ?? "" }
    needsFixtures("an account to sign in with (or --param user=NAME and MOQUI_TEST_PASSWORD)")
    return { username: "john.doe", password: "moqui" }
}

// test_a02_headers.py: test_session_cookie_samesite_lax
// Does not prove: the visitor and device cookies' attributes (the device cookie's are test/client SecurityDevice's).
feature("v5.0.0-3.3.2: the session cookie is SameSite=Lax", {
    when() { return { cookie: session().get("/Login").cookie(SESSION_COOKIE) } },
    then({ cookie }) {
        cookie != null
        cookie.sameSite.toLowerCase() == "lax"
    },
})

// test_a02_headers.py: test_session_cookie_is_httponly
// Does not prove: that the session id never appears in a page's text, which is sampled in code, not here.
feature("v5.0.0-3.3.4: the session cookie is HttpOnly, out of reach of page scripts", {
    when() { return { cookie: session().get("/Login").cookie(SESSION_COOKIE) } },
    then({ cookie }) {
        cookie != null
        cookie.httpOnly
    },
})

// test_a02_headers.py: test_cors_unknown_origin_is_401, test_cors_preflight_unknown_origin_is_401
// Does not prove: the origins a deployment lists itself; /Login stands for the screen chain.
feature("v5.0.0-3.4.2: a foreign origin is 401 with no Access-Control-Allow-Origin (#method)", {
    where: [
        { method: "GET", headers: { Origin: EVIL_ORIGIN } },
        { method: "OPTIONS", headers: { Origin: EVIL_ORIGIN, "Access-Control-Request-Method": "POST" } },
    ],
    when({ method, headers }) { return { res: session().request(method, "/Login", { headers }) } },
    then({ res }) {
        res.status == 401
        res.header("Access-Control-Allow-Origin") == null
    },
})

// test_a02_headers.py: test_cors_same_origin_returns_allow_origin, test_cors_preflight_same_origin_is_allowed
// The control of the refusals: the server's own origin is allowed, exactly and never as a wildcard.
// Does not prove: the origins a deployment adds to @allow-origins, which take the same path.
feature("v5.0.0-3.4.2: the server's own origin is echoed exactly with credentials, its preflight allowed", {
    when() {
        return {
            res: session().get("/Login", { headers: { Origin: OWN_ORIGIN } }),
            preflight: session().options("/Login", { headers: { Origin: OWN_ORIGIN, "Access-Control-Request-Method": "POST" } }),
        }
    },
    then({ res, preflight }) {
        res.status == 200
        (res.header("Access-Control-Allow-Origin") ?? "").toLowerCase() == OWN_ORIGIN
        (res.header("Access-Control-Allow-Credentials") ?? "").toLowerCase() == "true"
        preflight.status == 204
        (preflight.header("Access-Control-Allow-Origin") ?? "").toLowerCase() == OWN_ORIGIN
        (preflight.header("Access-Control-Allow-Methods") ?? "").includes("POST")
    },
})

// Not in Java, which compares the host only and so allows these.
// Does not prove: a deployment that lists one of these origins on purpose, which fails it.
feature("v5.0.0-3.4.2: the server's host on #which is a foreign origin, refused (#method)", {
    where: [
        { which: "another port", method: "GET", headers: { Origin: OTHER_PORT_ORIGIN } },
        { which: "another scheme", method: "GET", headers: { Origin: OTHER_SCHEME_ORIGIN } },
        {
            which: "another port", method: "OPTIONS",
            headers: { Origin: OTHER_PORT_ORIGIN, "Access-Control-Request-Method": "POST" },
        },
        {
            which: "another scheme", method: "OPTIONS",
            headers: { Origin: OTHER_SCHEME_ORIGIN, "Access-Control-Request-Method": "POST" },
        },
    ],
    when({ method, headers }) { return { res: session().request(method, "/Login", { headers }) } },
    then({ res }) {
        res.status == 401
        res.header("Access-Control-Allow-Origin") == null
    },
})

// test_a07_authn.py: test_post_with_csrf_token_executes_preference_round_trip, test_post_without_csrf_does_not_store_preference
// Does not prove: the foreign-Origin refusal of a browser's request, nor calls authenticated by a header (Basic,
// api_key), which carry no session token by design.
feature("v5.0.0-3.5.1: a POST changes data only with the session's anti-forgery token (csrf: #csrf)", {
    where: [{ csrf: true, status: 200, stored: true }, { csrf: false, status: 401, stored: false }],
    when({ csrf }) {
        const { username, password } = account()
        const s = session()
        s.login(username, password)
        const key = `secCsrfHttp-${csrf}-${Date.now()}`
        const res = s.post("/apps/setPreference", { form: { preferenceKey: key, preferenceValue: "csrf-ran" }, csrf })
        const prefs = s.get("/apps/getPreferences", { params: { keyRegexp: key } })
        // blank it again, as the Python proofs do
        if (csrf) s.post("/apps/setPreference", { form: { preferenceKey: key, preferenceValue: "" } })
        return { res, prefs, key }
    },
    then({ res, status, prefs, key, stored }) {
        res.status == status
        res.status != 401 || res.text.includes(TOKEN_ERROR)
        prefs.status == 200
        (prefs.json()[key] == "csrf-ran") == stored
    },
})

// test_api_rest.py: test_entity_rest_post_without_csrf_does_not_create
// Does not prove: REST calls authenticated by a header (Basic, api_key), which carry no session token by design.
feature("v5.0.0-3.5.1: an entity REST POST without the session token is refused and creates nothing", {
    when() {
        const { username, password } = account()
        const res = session({ user: username, password }).post("/rest/e1/moqui.basic.Enumeration", {
            json: { enumId: "SEC_SHOULD_NOT_EXIST", description: "sec csrf" }, accept: "json", csrf: false,
        })
        // with the fixtures, the narrow VIEW user reads Enumeration: the row is not there
        const got = profile == "fixtures"
            ? session({ user: "sec.ent.view", password: "SecEntView1!!" })
                .get("/rest/e1/moqui.basic.Enumeration/SEC_SHOULD_NOT_EXIST", { accept: "json" })
            : null
        return { res, got }
    },
    then({ res, got }) {
        res.status == 401
        res.text.includes(TOKEN_ERROR)
        !res.text.includes("SEC_SHOULD_NOT_EXIST")
        got == null || got.status == 404
    },
})

// test_a01_redirect.py: test_login_returnTo_does_not_redirect_offsite, test_login_returnTo_same_origin_path_is_allowed
// Does not prove: redirects an application builds itself, nor the return targets of SSO.
feature("v5.0.0-3.7.2: the sign-in follows a returnTo on the site and never one off it", {
    when() {
        const { username, password } = account()
        const signIn = (returnTo) => {
            const s = session()
            s.get("/Login", { params: { returnTo } })
            return s.post("/Login/login", { form: { username, password } })
        }
        return { off: signIn(EVIL), on: signIn("/apps") }
    },
    then({ off, on }) {
        off.status == 302
        !off.location.toLowerCase().includes("evil.example")
        !off.location.includes("/phish")
        // the control: a returnTo on the site is followed
        on.status == 302
        on.location.startsWith("/") || on.location.startsWith(server.url)
        on.location.includes("/apps")
    },
})

// test_a01_redirect.py: test_login_referer_does_not_redirect_offsite
// Does not prove: redirects an application builds itself.
feature("v5.0.0-3.7.2: the sign-in does not redirect to an off-site Referer", {
    when() {
        const { username, password } = account()
        const s = session()
        s.get("/Login", { headers: { Referer: EVIL } })
        return { res: s.post("/Login/login", { form: { username, password } }) }
    },
    then({ res }) {
        [200, 301, 302, 303, 307, 308].includes(res.status)
        !(res.location ?? "").toLowerCase().includes("evil.example")
    },
})

// test_a01_redirect.py: test_login_returnTo_does_not_follow_host_header
// returnTo is compared with the request Host (webroot/Login.xml). The Host is forged with raw(): the redirect stays on
// the site, and a CR/LF in the request line is refused, not turned into a header.
// Does not prove: what a proxy in front does with a forged Host; where one refuses it, the feature skips.
feature("v5.0.0-3.7.2: the sign-in does not redirect off the site under a forged Host", {
    when() {
        const { username, password } = account()
        const s = session()
        const form = `username=${encodeURIComponent(username)}&password=${encodeURIComponent(password)}`
        const page = s.raw("GET /Login?returnTo=" + encodeURIComponent(EVIL) +
            " HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n")
        if (page.status != 200) skip(`a forged Host is refused before the sign-in page (HTTP ${page.status})`)
        const res = s.raw("POST /Login/login HTTP/1.1\r\nHost: evil.example\r\n" +
            "Content-Type: application/x-www-form-urlencoded\r\nContent-Length: " + form.length +
            "\r\nConnection: close\r\n\r\n" + form)
        const split = s.raw("GET /Login\r\nLocation: https://evil.example/phish HTTP/1.1\r\n" +
            "Host: localhost\r\nConnection: close\r\n\r\n", { cookies: false })
        return { page, res, split }
    },
    then({ page, res, split }) {
        page.cookie(SESSION_COOKIE) != null
        res.status == 301 || res.status == 302 || res.status == 303 || res.status == 307 || res.status == 308
        !(res.location ?? "").toLowerCase().includes("evil.example")
        !(res.location ?? "").includes("/phish")
        split.status == 400
        !(split.location ?? "").toLowerCase().includes("evil.example")
        !(split.location ?? "").includes("/phish")
    },
})
