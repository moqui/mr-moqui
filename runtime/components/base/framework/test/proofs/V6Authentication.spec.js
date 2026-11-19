// OWASP ASVS 5.0.0 V6 Authentication: the framework's proofs, each feature filed under the requirement its name starts
// with (docs/security/proofs/V6-Authentication.md). They read the framework's own login page, /Login; an application
// with a login page of its own proves its page in its own suite.

// The <input> tags of the anonymous /Login page whose name is one of `names`.
const loginInputs = (names) => {
    const res = session().get("/Login")
    assert(res.status == 200, `GET /Login answered ${res.status}`)
    return (res.text.match(/<input\b[^>]*>/g) ?? []).filter((tag) => names.some((name) => tag.includes(`name="${name}"`)))
}
const attribute = (tag, name) => (tag.match(new RegExp(`\\b${name}="([^"]*)"`)) ?? [])[1] ?? null
const ADMIN_ERROR = "Can only create initial admin account if there are no UserAccount records"
const loggedIn = (res) => res.status == 200 && res.text.replace(/\s/g, "").toLowerCase().includes('"loggedin":true')
const restLogin = (username, password) => session().post("/rest/login", { json: { username, password }, accept: "json", csrf: false })
// A user of test/ClientFixtures.spec.js, signed in over REST in a session of its own and never logged out (a REST
// logout ends every session of the user); in the deployment profile the feature skips.
const requireFixture = (username, password) => {
    needsFixtures(`the user ${username}`)
    const res = session().post("/rest/login", { form: { username, password }, accept: "json", csrf: false })
    if (res.status == 401) skip(`${username} is not on this server: run test/ClientFixtures.spec.js on its database (moqui test framework --spec ClientFixtures --db existing), or use --serve`)
    return res
}

const PW_USER = "sec.pw.proof"
const PW = "SecPwProof1!!"
// A password change through the sign-in page's form; no sign-in first, as the form needs none.
const changePassword = (form) => {
    const s = session()
    s.get("/Login")
    return s.post("/Login/changePassword", { form, accept: "json" })
}

// test_a07_more.py: test_short_password_is_rejected_over_http. sec.all.only has AUTHZA_ALL, which the password-change
// transition needs; "short" is under the length rule.
// Does not prove: the deployment's own minimum beyond one five-character try, nor the admin and REST password paths.
feature("v5.0.0-6.2.1: a new password shorter than the minimum length is refused", {
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

// Does not prove: the change of a password by a password administrator (ADMIN_PASSWORD), which needs no old password.
feature("v5.0.0-6.2.3: a password change needs the current password, and a wrong one is refused", {
    given() { needsFixtures(`the user ${PW_USER}`) },
    when() {
        const form = { username: PW_USER, newPassword: "SecPwOther2!!", newPasswordVerify: "SecPwOther2!!" }
        const without = changePassword(form)
        const wrong = changePassword(Object.assign({ oldPassword: "NotTheOld1!!" }, form))
        // a wrong old password counts as a failed sign-in; this one clears the count
        return { without, wrong, still: restLogin(PW_USER, PW) }
    },
    then({ without, wrong, still }) {
        !without.text.toLowerCase().includes("password updated")
        !wrong.text.toLowerCase().includes("password updated")
        // the control: the old password still signs in, so neither changed it
        loggedIn(still)
    },
})

// Does not prove: how the password is compared inside (argon2, in-process); it shows only that no variant is taken.
feature("v5.0.0-6.2.8: a password is taken only exactly as it was set", {
    given() { needsFixtures(`the user ${PW_USER}`) },
    when() {
        // each wrong try counts as a failed sign-in, and each exact one clears the count, so the account never locks
        const spaced = restLogin(PW_USER, PW + " ")
        const exact1 = restLogin(PW_USER, PW)
        const cased = restLogin(PW_USER, PW.toLowerCase())
        const exact2 = restLogin(PW_USER, PW)
        const cut = restLogin(PW_USER, PW.slice(0, -1))
        const exact3 = restLogin(PW_USER, PW)
        return { spaced, cased, cut, exact1, exact2, exact3 }
    },
    then({ spaced, cased, cut, exact1, exact2, exact3 }) {
        spaced.status == 401
        cased.status == 401
        cut.status == 401
        loggedIn(exact1)
        loggedIn(exact2)
        loggedIn(exact3)
    },
})

// Does not prove: the password fields of an application's own forms (the <password/> widget renders type="password",
// which the render goldens check in-process).
feature("v5.0.0-6.2.6: the sign-in and password-change forms mask every password field", {
    when() { return { inputs: loginInputs(["password", "oldPassword", "newPassword", "newPasswordVerify"]) } },
    then({ inputs }) {
        // the control: the four fields are on the page
        inputs.length == 4
        inputs.every((tag) => attribute(tag, "type") == "password")
    },
})

// Does not prove: that a browser's password manager fills them, or that no script of an application blocks paste.
feature("v5.0.0-6.2.7: the password fields let a password manager fill them and do not block paste", {
    when() { return { inputs: loginInputs(["username", "password", "oldPassword", "newPassword", "newPasswordVerify"]) } },
    then({ inputs }) {
        // the control: the four password fields are on the page
        inputs.filter((tag) => attribute(tag, "type") == "password").length == 4
        attribute(inputs.find((tag) => attribute(tag, "id") == "login_form_username"), "autocomplete") == "username"
        attribute(inputs.find((tag) => attribute(tag, "name") == "password"), "autocomplete") == "current-password"
        attribute(inputs.find((tag) => attribute(tag, "name") == "oldPassword"), "autocomplete") == "current-password"
        inputs.filter((tag) => attribute(tag, "name").startsWith("newPassword")).every((tag) => attribute(tag, "autocomplete") == "new-password")
        !inputs.some((tag) => /\bon(paste|copy|drop)=/i.test(tag) || attribute(tag, "autocomplete") == "off")
    },
})

// Does not prove: that no other account of the deployment has a guessable password; it tries the one well-known pair,
// the Moqui demo data's, once. A deployment's proof: the project's test server loads every data type, the demo data
// with it, so in the fixtures profile it skips.
feature("v5.0.0-6.3.2: the demo data's well-known account does not sign in", {
    given() {
        if (profile == "fixtures") skip("the project's test server loads the demo data and its account: this proof is for a deployment")
    },
    when() {
        return { res: session().post("/rest/login", { form: { username: "john.doe", password: "moqui" }, accept: "json", csrf: false }) }
    },
    then({ res }) {
        res.status == 401
        !res.text.includes("\"loggedIn\":true")
    },
})

// test_a07_more.py: test_create_initial_admin_http_fails_when_users_exist. The service's refusal is the screen's JSON
// errors text, and the username it named still cannot sign in.
// Does not prove: the first start on an empty database, which loopback or the setup secret guards.
feature("v5.0.0-6.3.2: no initial administrator can be created over HTTP once accounts exist", {
    when() {
        const s = session()
        s.get("/Login")
        assert(s.token(), "GET /Login did not give a session token")
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

// Configuration in the matrix: an account locks after user-facade/login@max-failures (on by default, test/client
// SecurityAuthn "A07 failed logins lock the dedicated user"), and the per-client sign-in limit, the login rate limit of
// webapp-list/webapp[webroot]/web-protection (ten a minute), is on only with web-protection@enabled or trusted-proxies.
// Loopback is exempt, so this is a deployment's proof, and opt-in: it gets this client's sign-ins limited for a minute,
// and its ten unknown users count toward the auth-failure ban (twenty in ten minutes bans the client for fifteen), so
// run it alone, once: --spec V6Authentication --feature v5.0.0-6.3.1 --param bruteForce=yes.
// Does not prove: a limit per account across clients (credential stuffing from many addresses), which SEC-WEB-81 and
// SEC-WEB-83 leave open.
feature("v5.0.0-6.3.1: one client's sign-ins are limited after ten tries a minute", {
    given() {
        if (profile == "fixtures") skip("the project's test server takes its clients as loopback, which the per-client limits exempt: a deployment's proof")
        if (params.bruteForce != "yes") skip("opt-in, as it limits this client's sign-ins: --param bruteForce=yes, alone (--spec V6Authentication --feature v5.0.0-6.3.1)")
    },
    when() {
        const statuses = []
        for (let i = 0; i < 11; i++) {
            statuses.push(restLogin(`proof.unknown.${Date.now()}.${i}`, "NotAPassword1!").status)
        }
        return { statuses }
    },
    then({ statuses }) {
        // the control: the first tries are answered, as an unknown user
        statuses[0] == 401
        statuses.includes(429)
    },
})

// A password hint or a secret question would be a second, weaker secret. The sign-in page's forms (sign in, reset,
// change) ask for none; a reset asks only for the account and sends a temporary password.
// Does not prove: an application's own registration or account forms.
feature("v5.0.0-6.4.2: no form of the sign-in page asks for a password hint or a secret question", {
    when() {
        const res = session().get("/Login")
        return { res, names: (res.text.match(/<input\b[^>]*>/g) ?? []).map((tag) => attribute(tag, "name") ?? "") }
    },
    then({ res, names }) {
        res.status == 200
        // the control: the forms are there
        names.includes("password") && names.includes("newPassword")
        !names.some((name) => /hint|question|answer|secret/i.test(name))
        !/password hint|security question|secret question/i.test(res.text)
    },
})

// sec.pw.long's password, which the 6.2.9 proof changes and ClientFixtures sets again on every run.
const LONG_USER = "sec.pw.long"
const LONG_PW = "SecPwLong1!!"

// A long password is set through the sign-in page's change form and signs in whole; the first 64 or 72 of its
// characters (where a hash that cuts long passwords would stop) do not.
// Does not prove: the longest password taken (none is set), nor the admin and REST paths, which call the same service.
feature("v5.0.0-6.2.9: a 128-character password is set and signs in whole, never cut short", {
    given() { needsFixtures(`the user ${LONG_USER}`) },
    when() {
        const long = `SecPwLong${Date.now()}!`.padEnd(128, "-abcdefghijklmnopqrstuvwxyz")
        const inputs = loginInputs(["password", "oldPassword", "newPassword", "newPasswordVerify"])
        const res = changePassword({ username: LONG_USER, oldPassword: LONG_PW, newPassword: long, newPasswordVerify: long })
        // the cut ones first, each a failed sign-in; the whole one then clears the count
        const first64 = restLogin(LONG_USER, long.slice(0, 64))
        const first72 = restLogin(LONG_USER, long.slice(0, 72))
        return { long, inputs, res, first64, first72, whole: restLogin(LONG_USER, long) }
    },
    then({ long, inputs, res, first64, first72, whole }) {
        long.length == 128
        // the form's fields set no shorter limit
        inputs.length == 4
        inputs.every((tag) => attribute(tag, "maxlength") == null || Number(attribute(tag, "maxlength")) >= 128)
        res.text.toLowerCase().includes("password updated")
        loggedIn(whole)
        first64.status == 401
        first72.status == 401
    },
})

// Configuration in the matrix: moqui.security.UserGroup.requireAuthcFactor Y on ALL_USERS (a data step) asks every password
// sign-in for a second factor; it is off by default. A deployment's proof with its account (--param user=NAME, the password
// in MOQUI_TEST_PASSWORD): the password alone does not sign it in. The project's test server asks no factor of every
// account, so in the fixtures profile it skips.
// Does not prove: other accounts than the one given, nor sign-ins without a password (login keys, app passwords, SSO),
// which ask no second factor. Where it holds, the proofs that sign in with --param user cannot: the harness sends no code.
feature("v5.0.0-6.3.3: the deployment's account is not signed in by its password alone", {
    given() {
        if (profile == "fixtures") skip("the project's test server asks no second factor of every account: a deployment's proof")
        if (!params.user) skip("needs the deployment's account: --param user=NAME, its password in MOQUI_TEST_PASSWORD")
    },
    when() { return { res: restLogin(params.user, params.password ?? "") } },
    then({ res }) {
        // the password was right (a wrong one is 401), and the answer asks for a factor instead of signing in
        res.status == 200
        res.json().secondFactorRequired == true
        res.json().loggedIn != true
    },
})

// Last in the file: it changes sec.pw.proof's password, which ClientFixtures sets again on the next run.
// Does not prove: the change in an application's own account screens, which call the same service.
feature("v5.0.0-6.2.2: a user changes their password, and only the new one signs in after", {
    given() { needsFixtures(`the user ${PW_USER}`) },
    when() {
        const fresh = `SecPw${Date.now() % 1000000}!a`
        const res = changePassword({ username: PW_USER, oldPassword: PW, newPassword: fresh, newPasswordVerify: fresh })
        return { res, withNew: restLogin(PW_USER, fresh), withOld: restLogin(PW_USER, PW) }
    },
    then({ res, withNew, withOld }) {
        res.text.toLowerCase().includes("password updated")
        loggedIn(withNew)
        withOld.status == 401
    },
})
