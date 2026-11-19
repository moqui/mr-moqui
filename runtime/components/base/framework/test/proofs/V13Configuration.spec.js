// OWASP ASVS 5.0.0 V13 Configuration: the framework's proofs, each feature filed under the requirement its name starts
// with (docs/security/proofs/V13-Configuration.md). Ported from Moqui Java's pytest proofs; a comment names the Python
// test, as in test/client.

// A user of test/ClientFixtures.spec.js or of the demo data, signed in over REST in a session of its own and never
// logged out (a REST logout ends every session of the user); in the deployment profile the feature skips.
const requireFixture = (username, password) => {
    needsFixtures(`the user ${username}`)
    const res = session().post("/rest/login", { form: { username, password }, accept: "json", csrf: false })
    if (res.status == 401) skip(`${username} is not on this server: the fixtures profile needs the demo data and the users of test/ClientFixtures.spec.js (moqui test framework --spec ClientFixtures --db existing), or --serve`)
    return res
}

// The matrix has this as Configuration: a deployment from ./moq archive or the binary has no .git, and a checkout's is
// readable by server scripts; a name starting with "." is never served (screen/static_files.rs safe_segments).
// Does not prove: what a proxy in front serves from its own document root.
feature("v5.0.0-13.4.1: no source-control metadata is served: /.git and the like are not found", {
    when() {
        const s = session()
        return {
            page: s.get("/Login"),
            answers: ["/.git/HEAD", "/.git/config", "/.git/index", "/.git/", "/.svn/entries", "/.hg/store/00manifest.i",
                "/static/.git/HEAD"].map((path) => ({ path, res: s.get(path) })),
        }
    },
    then({ page, answers }) {
        // the control: the server serves its pages
        page.status == 200
        answers.every(({ res }) => res.status != 200 || !/ref: refs\/|\[core\]|repositoryformatversion/.test(res.text))
        answers.every(({ res }) => res.status >= 300)
    },
})

// test_api_rest.py: test_rest_schema_without_permission_is_403. The anonymous row stands for
// test_rest_schema_anonymous_is_401, whose /rest/entity.swagger is not built here. /rest/entity.json checks the sign-in,
// then REST_SCHEMA. sec.api.view has MOQUI_API VIEW and not REST_SCHEMA; the seed data grants REST_SCHEMA to ADMIN,
// which the demo data's john.doe holds, and its row reads one entity's document on the same route.
// Does not prove: /status, /health and /metrics off the status list, which need a client that is not on loopback.
feature("v5.0.0-13.4.5: the entity REST schema is served only with REST_SCHEMA: #who", {
    where: [
        { who: "anonymous is 401", username: null, path: "/rest/entity.json", status: 401, marker: "Authentication required for REST schema" },
        { who: "without REST_SCHEMA is 403", username: "sec.api.view", password: "SecApiView1!!", path: "/rest/entity.json", status: 403, marker: "Not authorized for REST schema" },
        { who: "ADMIN with REST_SCHEMA is 200", username: "john.doe", password: "moqui", path: "/rest/entity.json/moqui.basic.Enumeration", status: 200, marker: "enumId" },
    ],
    given({ username, password }) { if (username) requireFixture(username, password) },
    when({ username, password, path }) {
        const s = username ? session({ user: username, password }) : session()
        return { res: s.get(path, { accept: "json" }) }
    },
    then({ res, status, marker }) {
        res.status == status
        res.text.includes(marker)
        status == 200 || !res.text.includes("\"enumId\"")
    },
})

// A static file is sent only when the path names a regular file (screen/static_files.rs file_under); a directory of
// /js, /css, /libs or /images has no index page. WebDAV lists folders, but only to a signed-in user, by design.
// Does not prove: the WebDAV and /resources listings for signed-in users (intended), nor what a proxy in front serves from
// a document root of its own.
feature("v5.0.0-13.4.3: a static directory's path lists nothing; its files are served", {
    when() {
        const s = session()
        return {
            files: ["/js/MoquiLib.js", "/favicon.ico"].map((path) => ({ path, res: s.get(path) })),
            dirs: ["/js", "/js/", "/css/", "/libs/", "/libs/vue/", "/libs/quasar/lang/", "/libs/fontawesome/css/", "/images/"]
                .map((path) => ({ path, res: s.get(path) })),
            dav: s.request("PROPFIND", "/dav/", { headers: { Depth: "1" } }),
        }
    },
    then({ files, dirs, dav }) {
        // the control: the files in those directories are served
        files.every(({ res }) => res.status == 200)
        dirs.every(({ res }) => res.status >= 400)
        dirs.every(({ res }) => !/MoquiLib\.js|vue\.global|Index of|<a href=/i.test(res.text))
        dav.status >= 400
        !dav.text.includes("multistatus")
    },
})

// Configuration in the matrix: web protection's requests@methods list refuses any other method 405, on when
// webapp-list/webapp[webroot]/web-protection@enabled is true or @trusted-proxies is set, and not for a client a client-rule
// exempts (loopback, by default). Without it a TRACE to a screen is rendered as a GET would be (REST and static files
// refuse it), so this is a deployment's proof, and in the fixtures profile it skips.
// Does not prove: the other listeners (gRPC, the cluster's), nor a client the deployment exempts.
feature("v5.0.0-13.4.4: a TRACE request is refused, not answered with the page or the request", {
    given() {
        if (profile == "fixtures") skip("the project's test server takes its clients as loopback and has web protection off, so a TRACE to a screen renders it there: a deployment's proof")
    },
    when() {
        const marker = `proof-trace-${Date.now()}`
        const s = session()
        return {
            page: s.get("/Login"),
            traces: ["/Login", "/", "/rest/login", "/js/MoquiLib.js"].map((path) =>
                ({ path, res: s.request("TRACE", path, { headers: { "X-Proof-Trace": marker } }) })),
            marker,
        }
    },
    then({ page, traces, marker }) {
        // the control: the page is there for a GET
        page.status == 200
        traces.every(({ res }) => res.status >= 400)
        traces.every(({ res }) => !res.text.includes(marker) && !res.text.includes("<title>"))
    },
})
