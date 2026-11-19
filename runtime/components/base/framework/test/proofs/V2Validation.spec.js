// OWASP ASVS 5.0.0 V2 Validation and Business Logic: the framework's proofs, each feature filed under the requirement its
// name starts with (docs/security/proofs/V2-Validation-and-Business-Logic.md).

const PW_USER = "sec.pw.proof"
const PW = "SecPwProof1!!"
const loggedIn = (res) => res.status == 200 && res.text.replace(/\s/g, "").toLowerCase().includes('"loggedin":true')
const restLogin = (username, password) => session().post("/rest/login", { json: { username, password }, accept: "json", csrf: false })
// A burst from this machine: 20 clients at once, each a session of its own sending GET `path` until it is answered 429 or
// has sent `max`, so each is refused once at most. Their results: requests answered 200, and whether a 429 came.
const burst = () => parallel(20, (index, args) => {
    let ok = 0
    for (let i = 0; i < args.max; i++) {
        const res = http.get(args.path)
        if (res.status == 429) return { ok, limited: true }
        if (res.status == 200) ok++
    }
    return { ok, limited: false }
}, { path: params.floodPath ?? "/favicon.ico", max: Number(params.floodMax ?? 100) }, { timeout: 120 })
// A password change through the sign-in page's form, posted as a script would post it: no browser checks first.
const changePassword = (form) => {
    const s = session()
    s.get("/Login")
    return s.post("/Login/changePassword", { form, accept: "json" })
}

// The sign-in page's change form marks its fields required and the browser would not send it without them; the
// service checks them again, and the rule the form does not check (the two new passwords match).
// Does not prove: the validation of an application's own services, which declare their own parameters.
feature("v5.0.0-2.2.2: the server refuses a password change the form would not send", {
    given() { needsFixtures(`the user ${PW_USER}`) },
    when() {
        const missing = changePassword({ username: PW_USER, oldPassword: PW, newPassword: "SecPwOther2!!" })
        const differ = changePassword({
            username: PW_USER, oldPassword: PW, newPassword: "SecPwOther2!!", newPasswordVerify: "SecPwOther3!!",
        })
        return { missing, differ, still: restLogin(PW_USER, PW) }
    },
    then({ missing, differ, still }) {
        missing.json().validationErrors.some((error) => error.field == "newPasswordVerify")
        !missing.text.toLowerCase().includes("password updated")
        differ.text.includes("New Password and New Password Verify do not match")
        !differ.text.toLowerCase().includes("password updated")
        // the control: the password did not change
        loggedIn(still)
    },
})

// Configuration in the matrix: web protection's per-client limits (webapp-list/webapp[webroot]/web-protection, its
// rate-limit "all" of 50 requests a second with a burst of 500, and the bans) are on with web-protection@enabled or
// @trusted-proxies; the account lock, the global limits and the tarpits are on by default. Loopback is exempt, so this is
// a deployment's proof, and opt-in: it gets this client's requests refused for a few seconds. Each of its clients stops at
// its first 429, so the 20 refusals stay far under the abuse ban (300 a minute). Run it with --param flood=yes, alone or
// with 15.3.4's (--param floodPath=/a/cheap/file and floodMax=N for a deployment where /favicon.ico is not the server's).
// Does not prove: the limits of each costly function (the login limit is 6.3.1's, the tarpits are business quotas), nor
// a limit across clients.
feature("v5.0.0-2.4.1: a burst of requests from one client is answered 429 past its limit", {
    given() {
        if (profile == "fixtures") skip("the project's test server takes its clients as loopback, which the per-client limits exempt: a deployment's proof")
        if (params.flood != "yes") skip("opt-in, as it gets this client's requests refused for a few seconds: --param flood=yes")
    },
    when() { return burst() },
    then({ results, errors }) {
        errors.length == 0
        // the control: the burst's first requests are answered
        results.reduce((sum, result) => sum + result.ok, 0) > 0
        results.some((result) => result.limited)
    },
})
