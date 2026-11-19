// OWASP ASVS 5.0.0 V1 Encoding and Sanitization: the framework's proofs, each feature filed under the requirement its
// name starts with (docs/security/proofs/V1-Encoding-and-Sanitization.md). Ported from Moqui Java's pytest proofs; a
// comment names the Python test, as in test/client.

// The account a proof signs in with: the run's (--param user=NAME, its password in MOQUI_TEST_PASSWORD), else in the
// fixtures profile the demo data's john.doe.
const account = () => {
    if (params.user) return { username: params.user, password: params.password ?? "" }
    needsFixtures("an account to sign in with (or --param user=NAME and MOQUI_TEST_PASSWORD)")
    return { username: "john.doe", password: "moqui" }
}

// test_a05_injection.py: test_sql_looking_preference_is_stored_as_data
// Does not prove: any query but the preference store's; the raw SQL paths are proved in-process.
feature("v5.0.0-1.2.4: a SQL-looking value is stored and read back as data", {
    when() {
        const { username, password } = account()
        const s = session()
        s.login(username, password)
        const key = "secSqliHttp"
        const sqli = "' OR '1'='1"
        const set = s.post("/apps/setPreference", { form: { preferenceKey: key, preferenceValue: sqli } })
        const prefs = s.get("/apps/getPreferences", { params: { keyRegexp: key } })
        s.post("/apps/setPreference", { form: { preferenceKey: key, preferenceValue: "" } })
        return { set, prefs, key, sqli }
    },
    then({ set, prefs, key, sqli }) {
        set.status != 401
        set.status != 403
        prefs.status == 200
        prefs.text.includes(sqli)
        prefs.json()[key] == sqli
        !prefs.text.toLowerCase().includes("syntax error")
        !prefs.text.toLowerCase().includes("sql exception")
    },
})
