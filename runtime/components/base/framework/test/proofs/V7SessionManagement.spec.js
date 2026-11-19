// OWASP ASVS 5.0.0 V7 Session Management: the framework's proofs, each feature filed under the requirement its name
// starts with (docs/security/proofs/V7-Session-Management.md).

// The account a proof signs in with: the one the run's params give (--param user=NAME, with the password in
// MOQUI_TEST_PASSWORD so that it stays out of ps), else in the fixtures profile ClientFixtures' sec.view.only.
const account = () => {
    if (params.user) return { username: params.user, password: params.password ?? "" }
    needsFixtures("an account to sign in with (or --param user=NAME and MOQUI_TEST_PASSWORD)")
    return { username: "sec.view.only", password: "SecView1!!" }
}
// Whether an answer sends the browser to the sign-in page, as it does a request with no signed-in session.
const toSignIn = (res) => res.status == 302 && (res.location ?? "").includes("/Login")

// Does not prove: that a session ends when it idles or when the account is disabled (V7.3, in-process); it signs one
// session out with the REST logout and replays its cookies.
feature("v5.0.0-7.4.1: a signed-out session's cookies do not sign in again", {
    when() {
        const { username, password } = account()
        const s = session()
        const signIn = s.formLogin(username, password)
        assert(signIn.status == 302 && !toSignIn(signIn), `the sign-in of ${username} did not succeed: HTTP ${signIn.status}`)
        const cookie = s.cookies().map((c) => `${c.name}=${c.value}`).join("; ")
        const before = s.get("/apps/tools/Home")
        s.logout()
        const replayed = session({ headers: { Cookie: cookie } }).get("/apps/tools/Home")
        return { before, replayed }
    },
    then({ before, replayed }) {
        // the control: before the logout the same request was signed in
        !toSignIn(before)
        toSignIn(replayed)
    },
})
