// A02 CORS over HTTP, ported from Moqui Java's pytest proofs (framework/test/test_a02_headers.py; the catalog in
// README.md): an unknown origin is refused, and the server's own origin is allowed with credentials. These need a
// server that allows no foreign origin, the production conf (the default conf's empty webapp_allow_origins); the dev
// conf allows every origin, so the spec fails there, as the Python suite's require_production_conf fails it.
// A preflight answers 204 (Java 200). The origin is compared whole, scheme, host and port, where Java compares the host
// only: the server's host on another port or scheme is another site.

const EVIL_ORIGIN = "https://evil.example"
const OWN_ORIGIN = server.url.replace(/\/+$/, "").toLowerCase()
const HOST = server.host.includes(":") ? `[${server.host}]` : server.host
const OTHER_PORT_ORIGIN = `${server.secure ? "https" : "http"}://${HOST}:${server.port == 65535 ? 65534 : server.port + 1}`
const OTHER_SCHEME_ORIGIN = `${server.secure ? "http" : "https"}://${HOST}:${server.port}`

spec({
    setupSpec() {
        const res = session().get("/Login", { headers: { Origin: EVIL_ORIGIN } })
        if (res.status != 401 || res.header("Access-Control-Allow-Origin") != null) {
            throw new Error("this server allows every origin (the dev conf): run against MoquiProductionConf.xml or " +
                "MoquiLoadTestConf.xml, as --serve does " +
                `(GET /Login with Origin ${EVIL_ORIGIN} answered ${res.status}, Access-Control-Allow-Origin ` +
                `${res.header("Access-Control-Allow-Origin") ?? "none"})`)
        }
    },
})

// test_a02_headers.py: test_cors_unknown_origin_is_401, test_cors_preflight_unknown_origin_is_401
feature("A02 an unknown origin is 401 with no Access-Control-Allow-Origin (#method)", {
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

// test_a02_headers.py: test_cors_same_origin_returns_allow_origin
feature("A02 the server's own origin gets Access-Control-Allow-Origin with credentials", {
    when() { return { res: session().get("/Login", { headers: { Origin: OWN_ORIGIN } }) } },
    then({ res }) {
        res.status == 200
        (res.header("Access-Control-Allow-Origin") ?? "").toLowerCase() == OWN_ORIGIN
        (res.header("Access-Control-Allow-Credentials") ?? "").toLowerCase() == "true"
    },
})

// test_a02_headers.py: test_cors_preflight_same_origin_is_allowed
feature("A02 a preflight from the server's own origin is allowed", {
    when() {
        return { res: session().options("/Login", { headers: { Origin: OWN_ORIGIN, "Access-Control-Request-Method": "POST" } }) }
    },
    then({ res }) {
        res.status == 204
        (res.header("Access-Control-Allow-Origin") ?? "").toLowerCase() == OWN_ORIGIN
        (res.header("Access-Control-Allow-Methods") ?? "").includes("POST")
    },
})

// Not in Java, which compares the host only and so allows these
feature("A02 the server's host on #which is 401 with no Access-Control-Allow-Origin (#method)", {
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
