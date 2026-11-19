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
