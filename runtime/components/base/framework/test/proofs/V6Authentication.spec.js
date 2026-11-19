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
