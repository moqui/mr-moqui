// OWASP ASVS 5.0.0 V5 File Handling: the framework's proofs, each feature filed under the requirement its name starts
// with (docs/security/proofs/V5-File-Handling.md). Ported from Moqui Java's pytest proofs; a comment names the Python
// test, as in test/client.

// The account a proof signs in with: the run's (--param user=NAME, its password in MOQUI_TEST_PASSWORD), else in the
// fixtures profile the demo data's john.doe.
const account = () => {
    if (params.user) return { username: params.user, password: params.password ?? "" }
    needsFixtures("an account to sign in with (or --param user=NAME and MOQUI_TEST_PASSWORD)")
    return { username: "john.doe", password: "moqui" }
}
// A session signed in through the login form.
const formSession = (username, password) => {
    const s = session()
    const res = s.formLogin(username, password)
    assert(res.status == 302, `the login form did not sign ${username} in: HTTP ${res.status}`)
    return s
}

// test_a01_more.py: test_fop_filename_quote_stays_in_one_content_disposition
// Does not prove: mail attachment names, nor download paths other than the FOP render.
feature("v5.0.0-5.4.2: a quote in a requested file name stays inside one Content-Disposition", {
    when() {
        const { username, password } = account()
        const res = formSession(username, password).get("/fop/apps/tools/Home", { params: { filename: 'a"; x=y' } })
        if (res.status == 401 || res.status == 403) skip(`${username} may not render /fop/apps/tools/Home`)
        return { res, cd: res.header("Content-Disposition") ?? "" }
    },
    then({ res, cd }) {
        res.status == 200
        cd != ""
        !cd.includes('"; x=')
        !cd.includes("\r")
        !cd.includes("\n")
    },
})
