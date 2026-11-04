// A07 Identification and Authentication Failures over HTTP, ported from Moqui Java's pytest proofs
// (framework/test/test_a07_authn.py, test_a07_more.py, test_api_rest.py; the catalog in README.md): the REST login,
// the session token on a POST, a new session id at login, api_key and Basic. Passes on any conf.
//
// Where Java's proofs give john.doe a wrong password, these use an unknown user: MR-Moqui locks an account on its
// fourth failed login, and the suite must not lock the demo user. The session-token failure is 401 "Session token is
// missing or is not valid", and the session cookie is moqui.session (Java's is JSESSIONID).

const USER = params.user ?? "john.doe"
const PASSWORD = params.password ?? "moqui"
const SHARED_TEXT = "the username or password is not valid"

spec({ setupSpec() { login(USER, PASSWORD) } })

// test_a07_authn.py: test_rest_login_succeeds_without_csrf_token
feature("A07 the REST login needs no session token", {
    when() { return { res: session().post("/rest/login", { form: { username: USER, password: PASSWORD }, accept: "json", csrf: false }) } },
    then({ res }) {
        res.status == 200
        res.json().loggedIn == true
        !res.text.toLowerCase().includes("token required")
        !res.text.toLowerCase().includes("csrf")
    },
})

// test_a07_more.py: test_rest_login_unknown_user_is_401
feature("A07 a REST login of an unknown user is 401 with the shared text", {
    when() {
        return { res: session().post("/rest/login", { json: { username: "sec.no.such.user.zzz", password: "wrong" }, accept: "json" }) }
    },
    then({ res }) {
        res.status == 401
        res.text.toLowerCase().includes(SHARED_TEXT)
        !res.text.replace(/\s/g, "").toLowerCase().includes('"loggedin":true')
    },
})

// test_a07_authn.py: test_post_with_csrf_token_executes_preference_round_trip, test_post_without_csrf_does_not_store_preference
feature("A07 a POST runs with the session token and stores nothing without it (csrf: #csrf)", {
    where: [{ csrf: true, status: 200, stored: true }, { csrf: false, status: 401, stored: false }],
    when({ csrf }) {
        const key = `secCsrfHttp-${csrf}-${Date.now()}`
        const res = http.post("/apps/setPreference", { form: { preferenceKey: key, preferenceValue: "csrf-ran" }, csrf })
        const prefs = http.get("/apps/getPreferences", { params: { keyRegexp: key } })
        // blank it again, as the Python proofs do
        if (csrf) http.post("/apps/setPreference", { form: { preferenceKey: key, preferenceValue: "" } })
        return { res, prefs, key }
    },
    then({ res, status, prefs, key, stored }) {
        res.status == status
        res.status != 401 || res.text.includes("Session token is missing or is not valid")
        prefs.status == 200
        (prefs.json()[key] == "csrf-ran") == stored
    },
})

// test_a07_authn.py: test_login_issues_a_new_session_id
feature("A07 the login form issues a new session id", {
    when() {
        const s = session()
        const before = s.get("/Login").cookie("moqui.session")
        const res = s.post("/Login/login", { form: { username: USER, password: PASSWORD } })
        return { before, res, after: res.cookie("moqui.session") }
    },
    then({ before, res, after }) {
        res.status == 302
        before != null
        after != null
        after.value != before.value
    },
})

// test_a07_authn.py: test_api_key_in_query_string_does_not_authenticate, test_api_key_header_with_garbage_does_not_authenticate
feature("A07 an api_key #where does not log in", {
    where: [
        { where: "in the query", opts: { params: { api_key: "not-a-real-key" } } },
        { where: "header that is not a key", opts: { headers: { api_key: "not-a-real-key" } } },
    ],
    when({ opts }) { return { res: session().get("/rest/e1/moqui.basic.Enumeration", opts) } },
    then({ res }) {
        res.status == 401
    },
})

// test_api_rest.py: test_rest_e1_basic_auth_wrong_password_is_401 (an unknown user here, not john.doe with a wrong
// password), test_rest_e1_basic_auth_valid_is_not_unauthenticated
feature("A07 Basic on /rest/e1: #who", {
    where: [
        { who: "an unknown user is 401", user: "sec.no.such.user.zzz", password: "wrong", valid: false },
        { who: "a valid user is not unauthenticated", user: USER, password: PASSWORD, valid: true },
    ],
    when({ user, password }) { return { res: session().get("/rest/e1/moqui.basic.Enumeration", { basic: [user, password], accept: "json" }) } },
    then({ res, valid }) {
        (res.status == 401) == !valid
        // entity REST has no catch-all authz: 403 is authenticated but not authorized
        !valid || [200, 403, 404].includes(res.status)
        !res.text.includes("currentPassword")
    },
})

// From SecurityAuthnMore.spec.js, merged here to share a spec run.
// A07 authentication over HTTP that SecurityAuthn does not already cover, ported from Moqui Java's
// framework/test/test_a07_authn.py and test_a07_more.py. A wrong password and the lockout use sec.lock.test (never
// john.doe); the lock feature is last and may leave that account locked. Fixture users come from ClientFixtures and a
// missing one skips. verifyOtp sends the session token from GET /Login so the refusal is the pre-auth gate.

const RESET_TEXT = "if an account exists for that username or email, a reset password was sent"
const LOGIN_KEY = "sec-test-login-key-fixed-40-chars-value"
const LOCK_USER = "sec.lock.test"
const LOCK_PASSWORD = "SecLock1!!"
const MFA_USER = "sec.mfa.totp"
const MFA_PASSWORD = "SecMfaTotp1!!"
const MFA_CODE = "87654321"
const SERVICE_RUN = "/apps/tools/Service/ServiceRun/run"
const GEO_SERVICE = "org.moqui.impl.BasicServices.get#GeoRegionsForDropDown"
const ADMIN_ERROR = "Can only create initial admin account if there are no UserAccount records"
const TOKEN_ERROR = "Session token is missing or is not valid"
const CREDENTIALS = ["password", "oldPassword", "newPassword", "authPassword", "api_key", "login_key"]

const loggedIn = (res) => res.status == 200 && res.text.replace(/\s/g, "").toLowerCase().includes('"loggedin":true')

// The fixture user can log in from here, else the feature skips. A session of its own, not logged out: a REST logout
// ends every session of the user (UserAccount.hasLoggedOut).
const requireFixture = (username, password) => {
    const res = session().post("/rest/login", { form: { username, password }, accept: "json", csrf: false })
    if (res.status == 401) skip(`${username} is not on this server: run test/ClientFixtures.spec.js on its database (moqui test framework --spec ClientFixtures --db existing), or use --serve`)
    return res
}

// GET /Login so the session has a token (response header or the page), which later POSTs send as X-CSRF-Token.
const sessionWithToken = () => {
    const s = session()
    const page = s.get("/Login")
    const token = s.token() || page.header("X-CSRF-Token") || page.header("moquiSessionToken")
    assert(token, "GET /Login did not return a session token")
    return s
}

const restLogin = (username, password, headers) => session().post("/rest/login", {
    json: { username, password }, accept: "json", csrf: false, headers,
})

// The public reset text, from the JSON message list (messageInfos, else messages).
const publicMessage = (res) => {
    const body = res.json()
    const infos = Array.isArray(body.messageInfos) ? body.messageInfos.map((item) => item.message || "").join("\n") : ""
    if (infos) return infos
    return body.messages == null ? "" : String(body.messages)
}

const noPrivateLoginReason = (res) => {
    const low = res.text.toLowerCase()
    assert(low.includes(SHARED_TEXT), "the login did not use the shared text")
    assert(!low.includes("no account found"))
    assert(!low.includes("password incorrect"))
    assert(!low.includes("[distmp]"))
    assert(!low.includes("[disprm]"))
    assert(!loggedIn(res))
    return true
}

// Values posted besides the password, so the JSON has to omit those names and not echo them.
const EXTRA_SECRETS = {
    oldPassword: "OldShouldNotEcho1!!",
    newPassword: "NewShouldNotEcho1!!",
    authPassword: "AuthShouldNotEcho1!!",
    api_key: "api-key-should-not-echo",
    login_key: "login-key-should-not-echo",
}

const omitsLoginSecrets = (res, secrets) => {
    for (const secret of secrets) assert(!res.text.includes(secret), `the login JSON contains ${JSON.stringify(secret)}`)
    const body = res.json()
    for (const map of [body.currentParameters || {}, body.screenParameters || {}]) {
        for (const name of CREDENTIALS) assert(map[name] == null, `${name} is in the login JSON`)
    }
    return true
}

const screenLogin = (username, password) => {
    const s = sessionWithToken()
    return s.post("/Login/login", {
        form: Object.assign({ username, password }, EXTRA_SECRETS),
        accept: "json",
    })
}

// test_a07_authn.py: test_post_without_csrf_token_rejected_after_login
feature("A07 a Service Run POST without the session token is rejected", {
    when() {
        const s = session()
        s.login(USER, PASSWORD)
        return { res: s.post(SERVICE_RUN, { form: { serviceName: GEO_SERVICE }, csrf: false }) }
    },
    then({ res }) {
        res.status == 401
        res.status != 200
        res.text.includes(TOKEN_ERROR)
    },
})

// test_a07_authn.py: test_post_with_csrf_token_is_not_a_csrf_error
feature("A07 a Service Run POST with the session token is not a session-token error", {
    when() {
        const s = session()
        const logged = s.login(USER, PASSWORD)
        const token = s.token() || logged.header("X-CSRF-Token") || logged.header("moquiSessionToken")
        assert(token, "the login response did not include a session token")
        return { res: s.post(SERVICE_RUN, { form: { serviceName: GEO_SERVICE } }) }
    },
    then({ res }) {
        res.status != 401
        !res.text.includes(TOKEN_ERROR)
        !res.text.toLowerCase().includes("session token required")
        !res.text.toLowerCase().includes("token does not match")
    },
})

// test_a07_authn.py: test_api_key_header_from_getLoginKey_authenticates
feature("A07 an api_key header of the fixture login key authenticates", {
    given() { requireFixture("sec.key.http", "SecKeyHttp1!!") },
    when() { return { res: session().get("/rest/e1/moqui.basic.Enumeration", { apiKey: LOGIN_KEY, accept: "json" }) } },
    then({ res }) {
        res.status == 200
    },
})

// test_a07_authn.py: test_login_key_header_authenticates
feature("A07 a login_key header of the fixture login key authenticates", {
    given() { requireFixture("sec.key.http", "SecKeyHttp1!!") },
    when() { return { res: session().get("/rest/e1/moqui.basic.Enumeration", { loginKey: LOGIN_KEY, accept: "json" }) } },
    then({ res }) {
        res.status == 200
    },
})

// test_a07_authn.py: test_http_basic_on_screen_is_not_unauthenticated
feature("A07 Basic auth on a screen is not unauthenticated", {
    when() { return { res: session().get("/menuData/apps", { basic: [USER, PASSWORD] }) } },
    then({ res }) {
        res.status != 401
        res.status != 302 && res.status != 303 || !String(res.location ?? "").toLowerCase().includes("login")
    },
})

// test_a07_more.py: test_rest_login_wrong_password_is_401
// sec.lock.test, not john.doe: a wrong password counts toward the lock, and the lock feature is last.
feature("A07 a REST login with the wrong password is 401 with the shared text", {
    given() { requireFixture(LOCK_USER, LOCK_PASSWORD) },
    when() { return { res: restLogin(LOCK_USER, "definitely-wrong-password") } },
    then({ res }) {
        res.status == 401
        !loggedIn(res)
        res.text.toLowerCase().includes(SHARED_TEXT)
    },
})

// test_a07_more.py: test_sendOtp_without_preauth_is_rejected
feature("A07 sendOtp without a pre-authenticated user is rejected", {
    when() {
        const s = sessionWithToken()
        return { res: s.post("/rest/sendOtp", { json: { factorId: "not-a-factor" }, accept: "json" }) }
    },
    then({ res }) {
        !loggedIn(res)
        !res.text.includes(TOKEN_ERROR)
        res.text.toLowerCase().includes("no user pre-authenticated")
    },
})

// test_a07_more.py: test_verifyOtp_without_preauth_does_not_log_in
// The token from GET /Login is sent, so this 401 is the pre-auth gate and not a missing session token.
feature("A07 verifyOtp without a pre-authenticated user does not log in", {
    when() {
        const s = sessionWithToken()
        return { res: s.post("/rest/verifyOtp", { json: { code: "000000" }, accept: "json" }) }
    },
    then({ res }) {
        res.status == 401
        !loggedIn(res)
        !res.text.includes(TOKEN_ERROR)
        res.text.toLowerCase().includes("no user pre-authenticated")
    },
})

// test_a07_more.py: test_mfa_password_only_does_not_fully_log_in
feature("A07 an MFA password alone does not fully log in", {
    given() { requireFixture(MFA_USER, MFA_PASSWORD) },
    when() { return { res: restLogin(MFA_USER, MFA_PASSWORD) } },
    then({ res }) {
        res.status == 200
        !loggedIn(res)
        res.text.toLowerCase().includes("factor") || res.text.toLowerCase().includes("authc")
    },
})

// test_a07_more.py: test_mfa_login_with_wrong_code_does_not_log_in
// Before the success feature: the valid single-use code is consumed when it verifies.
feature("A07 an MFA login with the wrong code does not log in", {
    given() { requireFixture(MFA_USER, MFA_PASSWORD) },
    when() {
        return { res: session().post("/rest/login", {
            json: { username: MFA_USER, password: MFA_PASSWORD, code: "000000" }, accept: "json", csrf: false,
        }) }
    },
    then({ res }) {
        !loggedIn(res)
    },
})

// test_a07_more.py: test_mfa_login_with_code_succeeds
feature("A07 an MFA login with the single-use code succeeds", {
    given() { requireFixture(MFA_USER, MFA_PASSWORD) },
    when() {
        return { res: session().post("/rest/login", {
            json: { username: MFA_USER, password: MFA_PASSWORD, code: MFA_CODE }, accept: "json", csrf: false,
        }) }
    },
    then({ res }) {
        loggedIn(res)
    },
})

// test_a07_more.py: test_create_initial_admin_http_fails_when_users_exist
// The service error is the screen's JSON errors text. No new admin: that username still cannot log in.
feature("A07 create initial admin over HTTP fails when users exist", {
    when() {
        const s = sessionWithToken()
        const res = s.post("/Login/createInitialAdminAccount", {
            form: { username: "sec.should.not.exist", newPassword: "SecAdmin1!!", newPasswordVerify: "SecAdmin1!!" },
            accept: "json",
        })
        const probe = restLogin("sec.should.not.exist", "SecAdmin1!!")
        return { res, probe }
    },
    then({ res, probe }) {
        res.text.includes(ADMIN_ERROR)
        probe.status == 401
        !loggedIn(probe)
    },
})

// test_a07_more.py: test_short_password_is_rejected_over_http
// sec.all.only has AUTHZA_ALL, which the password-change transition needs. "short" is under the length rule.
feature("A07 a short password is rejected over HTTP", {
    when() {
        requireFixture("sec.all.only", "SecAll1!!")
        const s = session()
        s.login("sec.all.only", "SecAll1!!")
        const res = s.post("/Login/changePassword", {
            form: { username: "sec.all.only", oldPassword: "SecAll1!!", newPassword: "short", newPasswordVerify: "short" },
            accept: "json",
        })
        const still = restLogin("sec.all.only", "SecAll1!!")
        return { res, still }
    },
    then({ res, still }) {
        res.text.toLowerCase().includes("found issues with password so not updating")
        res.text.toLowerCase().includes("shorter")
        !res.text.toLowerCase().includes("password updated")
        loggedIn(still)
    },
})

// test_a07_more.py: test_login_json_response_omits_password_parameters
feature("A07 a login JSON response omits password parameters", {
    given() { requireFixture("sec.none.only", "SecNone1!!") },
    when() {
        const password = "SecNone1!!"
        return { res: screenLogin("sec.none.only", password), secrets: [password, ...Object.values(EXTRA_SECRETS)] }
    },
    then({ res, secrets }) {
        ;(res.contentType ?? "").toLowerCase().includes("json")
        res.status < 400
        omitsLoginSecrets(res, secrets)
    },
})

// test_a07_more.py: test_login_json_failed_omits_password_parameters
feature("A07 a failed login JSON response omits password parameters", {
    when() {
        const password = "SecShouldNotEcho1!!"
        return { res: screenLogin("sec.no.such.user.zzz", password), secrets: [password, ...Object.values(EXTRA_SECRETS)] }
    },
    then({ res, secrets }) {
        ;(res.contentType ?? "").toLowerCase().includes("json")
        !loggedIn(res)
        res.text.toLowerCase().includes(SHARED_TEXT)
        omitsLoginSecrets(res, secrets)
    },
})

// test_a07_more.py: test_login_unknown_and_wrong_password_share_public_text
// The existing account is sec.lock.test, not john.doe. A successful login here clears an earlier failure count.
feature("A07 an unknown user and a wrong password share the public text", {
    given() { requireFixture(LOCK_USER, LOCK_PASSWORD) },
    when() {
        return {
            screenUnknown: screenLogin("sec.no.such.user.zzz", "wrong"),
            screenWrong: screenLogin(LOCK_USER, "definitely-wrong-password"),
            restUnknown: restLogin("sec.no.such.user.zzz", "wrong"),
            restWrong: restLogin(LOCK_USER, "definitely-wrong-password"),
        }
    },
    then({ screenUnknown, screenWrong, restUnknown, restWrong }) {
        noPrivateLoginReason(screenUnknown)
        noPrivateLoginReason(screenWrong)
        restUnknown.status == 401
        restWrong.status == 401
        noPrivateLoginReason(restUnknown)
        noPrivateLoginReason(restWrong)
    },
})

// test_a07_more.py: test_password_reset_unknown_user_shows_shared_message
feature("A07 a password reset for an unknown user shows the shared message", {
    when() {
        const s = sessionWithToken()
        return { res: s.post("/Login/resetPassword", {
            form: { username: "sec.no.such.user.zzz", initialTab: "reset" }, accept: "json",
        }) }
    },
    then({ res }) {
        publicMessage(res).toLowerCase().includes(RESET_TEXT)
        !res.text.toLowerCase().includes("could not find account")
        !res.text.toLowerCase().includes("does not have an email address")
    },
})

// test_a07_more.py: test_password_reset_unknown_and_existing_share_public_text
// john.doe is the existing user. This only requests a reset: it does not set his login password.
feature("A07 a password reset for an unknown and an existing user share the public text", {
    when() {
        const s = sessionWithToken()
        const post = (username) => s.post("/Login/resetPassword", {
            form: { username, initialTab: "reset" }, accept: "json",
        })
        return { unknown: post("sec.no.such.user.zzz"), existing: post("john.doe") }
    },
    then({ unknown, existing }) {
        publicMessage(unknown) == publicMessage(existing)
        publicMessage(unknown).toLowerCase().includes(RESET_TEXT)
        publicMessage(existing).toLowerCase().includes(RESET_TEXT)
        !unknown.text.toLowerCase().includes("could not find account")
        !existing.text.toLowerCase().includes("could not find account")
        !unknown.text.toLowerCase().includes("does not have an email address")
        !existing.text.toLowerCase().includes("does not have an email address")
    },
})

// test_a07_more.py: test_ip_restricted_user_cannot_login_from_loopback
// sec.none.only can log in from here, so a refusal of sec.ip.v4 (allow list 10.99.99.99) is not a missing fixture.
// A forwarded address does not make this client that address, or loopback for an account that is not allowed it.
// check_ip_allowed's 401 names the user and the client address. Java's public sentence is the shared invalid-login text.
feature("A07 a user restricted to another address cannot log in from here", {
    given() { requireFixture("sec.none.only", "SecNone1!!") },
    when() {
        return {
            res: restLogin("sec.ip.v4", "SecIpV41!!"),
            spoofed: restLogin("sec.ip.v4", "SecIpV41!!", { "X-Forwarded-For": "10.99.99.99" }),
            v6: restLogin("sec.ip.v4", "SecIpV41!!", { "X-Forwarded-For": "::1" }),
        }
    },
    then({ res, spoofed, v6 }) {
        res.status == 401
        !loggedIn(res)
        spoofed.status == 401
        !loggedIn(spoofed)
        v6.status == 401
        !loggedIn(v6)
    },
})

// test_a07_more.py: test_loopback_ip_allowed_user_can_login
// sec.none.only is the loaded-fixture probe. A 401 for sec.ip.loop is then a refused address, not a skip.
feature("A07 a user allowed from loopback can log in", {
    given() { requireFixture("sec.none.only", "SecNone1!!") },
    when() { return { res: restLogin("sec.ip.loop", "SecIpLoop1!!") } },
    then({ res }) {
        loggedIn(res)
    },
})

// test_a07_more.py: test_failed_logins_lock_dedicated_user
// Last: four failures lock sec.lock.test (max-failures 3). Do not log that user in successfully after this.
feature("A07 failed logins lock the dedicated user", {
    when() {
        const first = restLogin(LOCK_USER, LOCK_PASSWORD)
        if (loggedIn(first)) {
            for (let i = 0; i < 4; i++) restLogin(LOCK_USER, "definitely-wrong-password")
            return { after: restLogin(LOCK_USER, LOCK_PASSWORD) }
        }
        // Already locked, or the fixtures are not loaded. sec.none.only tells those apart.
        requireFixture("sec.none.only", "SecNone1!!")
        return { after: first }
    },
    then({ after }) {
        !loggedIn(after)
        after.text.toLowerCase().includes(SHARED_TEXT)
    },
})
