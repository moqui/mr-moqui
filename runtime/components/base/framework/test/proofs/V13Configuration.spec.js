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
