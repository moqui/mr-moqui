// OWASP ASVS 5.0.0 V8 Authorization: the framework's proofs, each feature filed under the requirement its name starts
// with (docs/security/proofs/V8-Authorization.md). Ported from Moqui Java's pytest proofs; a comment names the Python
// test, as in test/client.
//
// These are 8.3.1, the server enforcing a partial grant: a user who may view a tool is refused what the tool's page or a
// header set by the client would reach. A user with no grant refused is deny by default, 8.2.1, a Gap today, and those
// refusals stay in test/client until it closes. They need the users of test/ClientFixtures.spec.js, so they skip in the
// deployment profile.

const VIEW_USER = "sec.view.only"
const VIEW_PASSWORD = "SecView1!!"

// A user of test/ClientFixtures.spec.js, signed in over REST in a session of its own and never logged out (a REST
// logout ends every session of the user); in the deployment profile the feature skips.
const requireFixture = (username, password) => {
    needsFixtures(`the user ${username}`)
    const res = session().post("/rest/login", { form: { username, password }, accept: "json", csrf: false })
    if (res.status == 401) skip(`${username} is not on this server: run test/ClientFixtures.spec.js on its database (moqui test framework --spec ClientFixtures --db existing), or use --serve`)
    return res
}
// A session signed in through the login form, as the Python screen_login.
const formSession = (username, password) => {
    const s = session()
    const res = s.formLogin(username, password)
    assert(res.status == 302, `the login form did not sign ${username} in: HTTP ${res.status}`)
    return s
}
// Exact JSON keys of a user's password fields. resetPasswordSetDate, passwordSetDate and passwordHint may remain.
const SECRET_FIELDS = ["currentPassword", "resetPassword", "passwordSalt", "passwordHashType", "passwordBase64"]
const noUserAccountSecrets = (obj) => {
    if (obj == null || typeof obj != "object") return true
    if (Array.isArray(obj)) {
        for (const item of obj) noUserAccountSecrets(item)
        return true
    }
    for (const field of SECRET_FIELDS) assert(!Object.prototype.hasOwnProperty.call(obj, field), `password field ${field} is in the JSON`)
    for (const value of Object.values(obj)) noUserAccountSecrets(value)
    return true
}

// test_a01_more.py: test_view_only_cannot_open_sql_runner
// Does not prove: that every service checks its own permission (8.2.1, a Gap); only the SQL Runner under a VIEW grant.
feature("v5.0.0-8.3.1: a user who may view Tools is refused the SQL Runner on direct request", {
    given() { requireFixture(VIEW_USER, VIEW_PASSWORD) },
    when() {
        const s = formSession(VIEW_USER, VIEW_PASSWORD)
        return {
            tools: s.get("/apps/tools/Home"),
            res: s.get("/apps/tools/SqlTui", { params: { db: "group:transactional", sql: "SELECT 1" } }),
        }
    },
    then({ tools, res }) {
        // the control: the user is signed in and may view Tools
        tools.status == 200
        // the screen's actions refuse the user (SQL_RUNNER_WEB) and the page shows why, as in Java; no query ran
        res.status == 403 || res.text.includes("does not have permission to use the SQL Runner")
        !res.text.includes("Showing all")
        !res.text.includes("Query altered")
    },
})

// test_a01_more.py: test_view_only_cannot_run_service_run_over_http. A run that is allowed redirects (302), so a
// redirect is not a denial.
// Does not prove: that every service checks its own permission (8.2.1, a Gap); only Service Run under a VIEW grant.
feature("v5.0.0-8.3.1: a user who may view Tools is refused a service run posted directly", {
    given() { requireFixture(VIEW_USER, VIEW_PASSWORD) },
    when() {
        const s = formSession(VIEW_USER, VIEW_PASSWORD)
        const tools = s.get("/apps/tools/Home")
        if (!s.token()) skip("no session token after the VIEW-only sign-in")
        const res = s.post("/apps/tools/Service/ServiceRun/run", {
            form: {
                serviceName: "org.moqui.impl.BasicServices.get#GeoRegionsForDropDown",
                moquiSessionToken: s.token(),
            },
        })
        return { tools, res, body: res.text.toLowerCase() }
    },
    then({ tools, res, body }) {
        // the control: the user is signed in and may view Tools
        tools.status == 200
        res.status == 401 || res.status == 403
        !body.includes("session token required")
        !body.includes("session token is missing")
    },
})

// test_a01_more.py: test_datasnapshot_view_cannot_download
// Does not prove: other download transitions; only the snapshot download under a VIEW grant.
feature("v5.0.0-8.3.1: a user who may view Tools is refused a data snapshot download", {
    given() { requireFixture(VIEW_USER, VIEW_PASSWORD) },
    when() {
        const s = formSession(VIEW_USER, VIEW_PASSWORD)
        return {
            tools: s.get("/apps/tools/Home"),
            res: s.get("/apps/tools/Data/Snapshots/downloadSnapshot", { params: { filename: "anything.zip" } }),
        }
    },
    then({ tools, res }) {
        // the control: the user is signed in and may view Tools
        tools.status == 200
        res.status == 401 || res.status == 403
    },
})

// test_api_rest.py: test_x_http_method_override_authz_uses_override_action. The GET row is the control: a VIEW user may
// read through the override, so a refusal of every override does not pass.
// Does not prove: other services' own permission checks; only the users update under a MOQUI_API VIEW grant.
feature("v5.0.0-8.3.1: a method-override header is authorized as the action it names (#action)", {
    where: [
        { action: "view", method: "GET", path: "/rest/s1/moqui/users", status: 200 },
        { action: "update", method: "PATCH", path: "/rest/s1/moqui/users/SEC_API_VIEW", status: 403 },
    ],
    given() { requireFixture("sec.api.view", "SecApiView1!!") },
    when({ method, path, action }) {
        const opts = { headers: { "X-HTTP-Method-Override": method }, accept: "json" }
        if (action == "update") opts.json = { emailAddress: "sec-should-not-write@example.com" }
        return { res: session({ user: "sec.api.view", password: "SecApiView1!!" }).post(path, opts) }
    },
    then({ res, status, action }) {
        res.status == status
        action != "update" || res.text.toLowerCase().includes("update") || res.text.toLowerCase().includes("not authorized")
        action != "update" || !res.text.toLowerCase().includes("create")
        action != "view" || noUserAccountSecrets(res.json())
    },
})
