// OWASP ASVS 5.0.0 V2 Validation and Business Logic: the framework's proofs, each feature filed under the requirement its
// name starts with (docs/security/proofs/V2-Validation-and-Business-Logic.md).

const PW_USER = "sec.pw.proof"
const PW = "SecPwProof1!!"
const loggedIn = (res) => res.status == 200 && res.text.replace(/\s/g, "").toLowerCase().includes('"loggedin":true')
const restLogin = (username, password) => session().post("/rest/login", { json: { username, password }, accept: "json", csrf: false })
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
