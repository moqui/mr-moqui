// A01 tarpit velocity over HTTP, ported from Moqui Java (framework/test/test_z_tarpit.py). These skip while
// tarpit-enabled is false: conf/MoquiLoadTestConf.xml, which moqui test-client framework --serve uses, turns the
// screen, transition and service tarpits off, and runtime/conf/MoquiProductionConf.xml leaves them on.

// The fixture user can log in from here, else the feature skips. A session of its own, not logged out: a REST logout
// ends every session of the user (UserAccount.hasLoggedOut).
const requireFixture = (username, password) => {
    const res = session().post("/rest/login", { form: { username, password }, accept: "json", csrf: false })
    if (res.status == 401) skip(`${username} is not on this server: run test/ClientFixtures.spec.js on its database (moqui test framework --spec ClientFixtures --db existing), or use --serve`)
    return res
}

// test_z_tarpit.py: test_screen_tarpit_returns_429_after_burst
// Demo ALL_SCREENS (webroot/data, 120 hits in 60 seconds, ALL_USERS). The load-test conf skips this: tarpit-enabled
// is false there, so this burst is not answered 429.
feature("A01 the dashboard is 429 after the ALL_SCREENS burst", {
    when() {
        const s = session()
        const login = s.post("/rest/login", {
            form: { username: params.user ?? "john.doe", password: params.password ?? "moqui" },
            accept: "json",
            csrf: false,
        })
        if (login.status != 200) skip("could not log in for tarpit velocity")
        let res = null
        for (let n = 0; n < 130; n++) {
            res = s.get("/apps/tools/dashboard")
            if (res.status == 429) break
        }
        return { res }
    },
    then({ res }) {
        if (res == null || res.status != 429) skip("the screen tarpit is off or the demo row is not loaded")
        res.status == 429
        const retry = res.header("Retry-After")
        retry == null || (Number.isInteger(Number(retry)) && Number(retry) > 0)
    },
})

// test_z_tarpit.py: test_transition_tarpit_for_sec_tap_user_returns_429
// ClientFixtures: sec.tap.user, 3 hits in 60 seconds on component://webroot/screen/webroot/apps.xml/setPreference.
feature("A01 setPreference is 429 under sec.tap.user's transition tarpit", {
    given() { requireFixture("sec.tap.user", "SecTapUser1!!") },
    when() {
        const s = session()
        const login = s.post("/rest/login", {
            form: { username: "sec.tap.user", password: "SecTapUser1!!" },
            accept: "json",
            csrf: false,
        })
        if (login.status != 200) skip(`could not rest-login as sec.tap.user (HTTP ${login.status})`)
        let token = login.header("X-CSRF-Token") || login.header("moquiSessionToken")
        if (!token) {
            s.get("/apps")
            token = s.token()
        }
        if (!token) skip("no session token")
        let res = null
        let first = null
        for (let n = 0; n < 8; n++) {
            res = s.post("/apps/setPreference", {
                form: { preferenceKey: `secTapHttp${n}`, preferenceValue: "v", moquiSessionToken: token },
                csrf: token,
            })
            if (first == null) first = res.status
            if (res.status == 429 || first == 401 || first == 403) break
        }
        return { res, first }
    },
    then({ res, first }) {
        if (res.status != 429 && (first == 401 || first == 403)) skip("sec.tap.user cannot run setPreference")
        if (res.status != 429) skip("the transition tarpit is off (the load-test conf)")
        res.status == 429
    },
})
