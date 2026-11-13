// A01 Broken Access Control over HTTP for REST, ported from Moqui Java's pytest proofs
// (framework/test/test_api_rest.py): who may read and write entity and service REST, the session token on a POST,
// method override, password fields left out of user JSON, and REST_SCHEMA on /rest/entity.json. Passes on any conf; a
// feature that needs a fixture user skips without it. HMAC, SmatNone, and JSON-RPC are not here.

const USER = params.user ?? "john.doe"
const PASSWORD = params.password ?? "moqui"
// Exact JSON keys. resetPasswordSetDate, passwordSetDate and passwordHint may remain.
const SECRET_FIELDS = ["currentPassword", "resetPassword", "passwordSalt", "passwordHashType", "passwordBase64"]
const PLANTED = "plain-should-not-authenticate"

// The fixture user can log in from here, else the feature skips. A session of its own, not logged out: a REST logout
// ends every session of the user (UserAccount.hasLoggedOut).
const requireFixture = (username, password) => {
    const res = session().post("/rest/login", { form: { username, password }, accept: "json", csrf: false })
    if (res.status == 401) skip(`${username} is not on this server: run test/ClientFixtures.spec.js on its database (moqui test framework --spec ClientFixtures --db existing), or use --serve`)
    return res
}
const demo = () => session({ user: USER, password: PASSWORD })
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
const loggedIn = (res) => res.text.toLowerCase().replace(/\s/g, "").includes('"loggedin":true')
// A list is at most 100 rows, so ask for the username, then the unfiltered list.
const usersOf = (listed) => {
    if (listed.status != 200) return []
    let data
    try { data = listed.json() } catch (e) { return [] }
    if (Array.isArray(data)) return data
    if (data && typeof data == "object") {
        for (const key of ["users", "list", "valueList"]) if (Array.isArray(data[key])) return data[key]
        if (data.username) return [data]
    }
    return []
}
const userIdFor = (listed, username) => {
    for (const item of usersOf(listed)) if (item && item.username == username && item.userId) return item.userId
    return null
}
const userIdOf = (s, username) => {
    const filtered = s.get("/rest/s1/moqui/users", { params: { username }, accept: "json" })
    return userIdFor(filtered, username) || userIdFor(s.get("/rest/s1/moqui/users", { accept: "json" }), username)
}

// test_api_rest.py: test_rest_e1_user_account_without_entity_authz_is_not_200
feature("A01 john.doe reading entity REST UserAccount is not a password dump", {
    when() { return { res: demo().get("/rest/e1/moqui.security.UserAccount", { accept: "json" }) } },
    then({ res }) {
        ;[401, 403, 404, 200].includes(res.status)
        !res.text.includes("currentPassword")
        res.status != 200 || noUserAccountSecrets(res.json())
    },
})

// test_api_rest.py: test_x_http_method_override_without_auth_is_401, test_x_http_method_override_still_requires_csrf.
// The port's session-token failure is "Session token is missing or is not valid" (Java says "Session token required"
// or "Session token does not match"). CSRF is the raw POST, so an override to DELETE still needs the token.
feature("A01 a method override on entity REST #case", {
    where: [
        { case: "without a login is 401", login: false },
        { case: "with a login still needs the session token", login: true },
    ],
    when({ login }) {
        const s = login ? demo() : session()
        return {
            res: s.post("/rest/e1/moqui.basic.Enumeration", {
                headers: { "X-HTTP-Method-Override": "DELETE" },
                json: login ? { enumId: "SEC_OVERRIDE_NOPE", description: "sec override" } : {},
                accept: "json", csrf: false,
            }),
        }
    },
    then({ res, login }) {
        res.status == 401
        !login || res.text.includes("Session token is missing or is not valid")
    },
})

// test_api_rest.py: test_entity_rest_post_without_csrf_does_not_create
feature("A01 an entity REST POST without the session token does not create", {
    when() {
        const res = demo().post("/rest/e1/moqui.basic.Enumeration", {
            json: { enumId: "SEC_SHOULD_NOT_EXIST", description: "sec csrf" }, accept: "json", csrf: false,
        })
        // The narrow VIEW user can read Enumeration. When that fixture is loaded, the row must not be there.
        const probe = session().post("/rest/login", {
            form: { username: "sec.ent.view", password: "SecEntView1!!" }, accept: "json", csrf: false,
        })
        const present = probe.status == 200 && loggedIn(probe)
        const got = present ? session({ user: "sec.ent.view", password: "SecEntView1!!" })
            .get("/rest/e1/moqui.basic.Enumeration/SEC_SHOULD_NOT_EXIST", { accept: "json" }) : null
        return { res, got }
    },
    then({ res, got }) {
        res.status != 200
        !res.text.includes("SEC_SHOULD_NOT_EXIST")
        ;[400, 401, 403, 404].includes(res.status) || res.status >= 400
        res.status != 401 || res.text.includes("Session token is missing or is not valid")
        got == null || got.status == 404
    },
})

// test_api_rest.py: test_entity_rest_narrow_authz_allows_only_its_entity
feature("A01 a narrow entity VIEW reads only its entity and cannot create", {
    given() { requireFixture("sec.ent.view", "SecEntView1!!") },
    when() {
        const s = session({ user: "sec.ent.view", password: "SecEntView1!!" })
        return {
            allowed: s.get("/rest/e1/moqui.basic.Enumeration", { accept: "json" }),
            denied: s.get("/rest/e1/moqui.security.UserAccount", { accept: "json" }),
            created: s.post("/rest/e1/moqui.basic.Enumeration", {
                json: { enumId: "SEC_ENT_VIEW_NOPE", description: "nope" }, accept: "json",
            }),
        }
    },
    then({ allowed, denied, created }) {
        allowed.status == 200
        denied.status == 403
        !denied.text.includes("currentPassword")
        created.status == 403
    },
})

// test_api_rest.py: test_entity_rest_catchall_authz_allows_other_entities
feature("A01 a catch-all entity ALL reads another entity and can create", {
    given() { requireFixture("sec.ent.all", "SecEntAll1!!") },
    when() {
        const s = session({ user: "sec.ent.all", password: "SecEntAll1!!" })
        const read = s.get("/rest/e1/moqui.security.UserAccount", { accept: "json" })
        // A row left by a failed run would make this create fail.
        s.delete("/rest/e1/moqui.basic.Enumeration/SEC_ENT_ALL_OK")
        const created = s.post("/rest/e1/moqui.basic.Enumeration", {
            json: { enumId: "SEC_ENT_ALL_OK", description: "ok" }, accept: "json",
        })
        let got
        try {
            got = s.get("/rest/e1/moqui.basic.Enumeration/SEC_ENT_ALL_OK", { accept: "json" })
        } finally {
            s.delete("/rest/e1/moqui.basic.Enumeration/SEC_ENT_ALL_OK")
        }
        return { read, created, got }
    },
    then({ read, created, got }) {
        read.status == 200
        noUserAccountSecrets(read.json())
        created.status == 200
        got.status == 200
        got.json().enumId == "SEC_ENT_ALL_OK"
    },
})

// test_api_rest.py: test_service_rest_moqui_api_view_can_read_users, test_service_rest_moqui_api_all_can_read_users
feature("A01 MOQUI_API #who can list users and the JSON omits password fields", {
    where: [
        { who: "VIEW", username: "sec.api.view", password: "SecApiView1!!" },
        { who: "ALL", username: "sec.api.all", password: "SecApiAll1!!" },
    ],
    given({ username, password }) { requireFixture(username, password) },
    when({ username, password }) {
        return { res: session({ user: username, password }).get("/rest/s1/moqui/users", { accept: "json" }) }
    },
    then({ res }) {
        res.status == 200
        noUserAccountSecrets(res.json())
    },
})

// test_api_rest.py: test_service_rest_users_one_omits_password_hash_fields,
// test_entity_rest_user_account_omits_password_hash_fields
feature("A01 #which omits UserAccount password fields", {
    where: [
        { which: "GET /moqui/users/EX_JOHN_DOE", username: "sec.api.view", password: "SecApiView1!!", path: "/rest/s1/moqui/users/EX_JOHN_DOE", account: true },
        { which: "GET entity REST UserAccount/EX_JOHN_DOE", username: "sec.ent.all", password: "SecEntAll1!!", path: "/rest/e1/moqui.security.UserAccount/EX_JOHN_DOE", account: false },
    ],
    given({ username, password }) { requireFixture(username, password) },
    when({ username, password, path }) {
        const res = session({ user: username, password }).get(path, { accept: "json" })
        // Service REST one of a missing user is 200 null; entity REST is 404.
        if (res.status == 404 || res.text.trim() == "null") skip("EX_JOHN_DOE not in this database")
        return { res }
    },
    then({ res, account }) {
        res.status == 200
        !account || res.json().username == "john.doe" || res.json().userId == "EX_JOHN_DOE"
        noUserAccountSecrets(res.json())
    },
})

// test_api_rest.py: test_service_rest_moqui_api_view_cannot_update_users, test_service_rest_no_api_authz_cannot_read_users
feature("A01 #who is 403", {
    where: [
        { who: "MOQUI_API VIEW patching a user", username: "sec.api.view", password: "SecApiView1!!", method: "PATCH", path: "/rest/s1/moqui/users/SEC_API_VIEW", body: { emailAddress: "sec-should-not-write@example.com" } },
        { who: "a user with no MOQUI_API authz reading users", username: "sec.none.only", password: "SecNone1!!", method: "GET", path: "/rest/s1/moqui/users", body: null },
    ],
    given({ username, password }) { requireFixture(username, password) },
    when({ username, password, method, path, body }) {
        const opts = { accept: "json" }
        if (body) opts.json = body
        return { res: session({ user: username, password }).request(method, path, opts) }
    },
    then({ res }) {
        res.status == 403
        res.text.toLowerCase().includes("not authorized")
    },
})

// test_api_rest.py: test_x_http_method_override_authz_uses_override_action.
// The GET row is the positive: a VIEW user may read through the override, so a refusal of every override does not pass.
feature("A01 a method override is authorized as #action", {
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

// test_api_rest.py: test_service_rest_users_patch_does_not_store_current_password.
// One failed login of sec.api.all, after this feature's own successful login, so the counter does not accumulate.
// john.doe is not sent a wrong password.
feature("A01 a user PATCH does not store currentPassword", {
    given() { requireFixture("sec.api.all", "SecApiAll1!!") },
    when() {
        const s = session({ user: "sec.api.all", password: "SecApiAll1!!" })
        const target = userIdOf(s, "sec.api.all") || "sec.api.all"
        s.patch("/rest/s1/moqui/users/" + target, { json: { currentPassword: PLANTED }, accept: "json" })
        const attempt = session().post("/rest/login", {
            json: { username: "sec.api.all", password: PLANTED }, accept: "json", csrf: false,
        })
        return { attempt }
    },
    then({ attempt }) {
        attempt.status != 200 || !loggedIn(attempt)
    },
})

// test_api_rest.py: test_service_rest_users_patch_does_not_store_disabled
feature("A01 a user PATCH does not store disabled", {
    given() {
        requireFixture("sec.api.all", "SecApiAll1!!")
        requireFixture("sec.none.only", "SecNone1!!")
    },
    when() {
        const s = session({ user: "sec.api.all", password: "SecApiAll1!!" })
        const target = userIdOf(s, "sec.none.only")
        if (!target) skip("sec.none.only not in /moqui/users list")
        const patched = s.patch("/rest/s1/moqui/users/" + target, {
            json: { disabled: "Y", emailAddress: "sec-should-not@example.com" }, accept: "json",
        })
        const got = s.get("/rest/s1/moqui/users/" + target, { accept: "json" })
        const again = session().post("/rest/login", {
            json: { username: "sec.none.only", password: "SecNone1!!" }, accept: "json", csrf: false,
        })
        return { patched, got, again }
    },
    then({ patched, got, again }) {
        ;[200, 400, 403, 404].includes(patched.status)
        got.status != 200 || got.json().disabled != "Y"
        got.status != 200 || got.json().emailAddress != "sec-should-not@example.com"
        again.status == 200
        loggedIn(again)
    },
})

// test_api_rest.py: test_service_rest_users_patch_stores_full_name
feature("A01 a user PATCH stores userFullName", {
    given() {
        requireFixture("sec.api.all", "SecApiAll1!!")
        requireFixture("sec.none.only", "SecNone1!!")
    },
    when() {
        const s = session({ user: "sec.api.all", password: "SecApiAll1!!" })
        const target = userIdOf(s, "sec.none.only")
        if (!target) skip("sec.none.only not in /moqui/users list")
        const before = s.get("/rest/s1/moqui/users/" + target, { accept: "json" })
        const original = before.status == 200 && before.json() ? before.json().userFullName : null
        const path = "/rest/s1/moqui/users/" + target
        let patched, got
        try {
            patched = s.patch(path, { json: { userFullName: "sec none only" }, accept: "json" })
            got = s.get(path, { accept: "json" })
        } finally {
            s.patch(path, { json: { userFullName: original != null ? original : "sec.none.only" }, accept: "json" })
        }
        return { patched, got }
    },
    then({ patched, got }) {
        patched.status == 200
        got.status == 200
        got.json().userFullName == "sec none only"
    },
})

// test_api_rest.py: test_entity_rest_does_not_create_user_login_key
feature("A01 entity REST does not create or read UserLoginKey", {
    given() { requireFixture("sec.ent.all", "SecEntAll1!!") },
    when() {
        const s = session({ user: "sec.ent.all", password: "SecEntAll1!!" })
        return {
            created: s.post("/rest/e1/moqui.security.UserLoginKey", {
                json: { loginKey: "http-should-not-store", userId: "EX_JOHN_DOE", fromDate: "2026-09-01 12:00:00.000" },
                accept: "json",
            }),
            listed: s.get("/rest/e1/moqui.security.UserLoginKey", { accept: "json" }),
        }
    },
    then({ created, listed }) {
        ;[401, 403].includes(created.status)
        ;[401, 403].includes(listed.status)
        !created.text.includes("http-should-not-store")
    },
})

// test_api_rest.py: test_email_server_host_not_writable_without_admin
feature("A01 an email server host is not writable without ADMIN", {
    given() { requireFixture("sec.api.all", "SecApiAll1!!") },
    when() {
        const s = session({ user: "sec.api.all", password: "SecApiAll1!!" })
        return {
            listed: s.get("/rest/s1/moqui/email/servers", { accept: "json" }),
            patched: s.patch("/rest/s1/moqui/email/servers/MOQUI_LOCAL", {
                json: { storeHost: "127.0.0.1", storePort: "9143" }, accept: "json",
            }),
            got: s.get("/rest/s1/moqui/email/servers/MOQUI_LOCAL", { accept: "json" }),
        }
    },
    then({ listed, patched, got }) {
        listed.status == 200
        ;[401, 403].includes(patched.status)
        got.status != 200 || got.json().storeHost != "127.0.0.1"
    },
})

// test_api_rest.py: test_rest_schema_without_permission_is_403.
// Anonymous 401 is added because test_rest_schema_anonymous_is_401 hits /rest/entity.swagger, which is not applicable
// (Swagger 2 is not built; that path 404s before auth). /rest/entity.json checks the login, then REST_SCHEMA, and does
// not 404 first. sec.api.view has MOQUI_API VIEW and not REST_SCHEMA. The ADMIN row reads one entity document on the
// same route (the seed data grants REST_SCHEMA to ADMIN): same gate, and the body is the document, not only a status.
feature("A01 /rest/entity.json #who", {
    where: [
        { who: "anonymous is 401", username: null, path: "/rest/entity.json", status: 401, marker: "Authentication required for REST schema" },
        { who: "without REST_SCHEMA is 403", username: "sec.api.view", password: "SecApiView1!!", path: "/rest/entity.json", status: 403, marker: "Not authorized for REST schema" },
        { who: "ADMIN with REST_SCHEMA is 200", username: USER, password: PASSWORD, path: "/rest/entity.json/moqui.basic.Enumeration", status: 200, marker: "enumId" },
    ],
    given({ username, password }) { if (username && username != USER) requireFixture(username, password) },
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
