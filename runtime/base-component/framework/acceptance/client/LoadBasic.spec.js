// A basic load test of the framework's own screens and REST (plans/archive/ClientTestHarness.md 8), as the user test.load
// that test/ClientFixtures.spec.js makes (VIEW on System, MOQUI_API and the Enumeration entity). By default 4 users for
// 10 seconds, a smoke test of each --serve run; a real run is
//     moqui test-client framework --spec LoadBasic --param users=50 --param duration=120
// with the JSON in <out>/load kept for comparison. Params: users, duration, maxP95 (ms, 2000 by default, loose so CI
// does not flake), loadPassword.
//
// No feature logs out: a logout marks the account logged out, which ends the other sessions of the same user.
//
// The tarpit: the demo data's ALL_SCREENS tarpit (120 hits of one screen in 60 s) is on ALL_USERS, which every user is
// in, and every matching tarpit applies, so no row of test.load's own can exempt it (as in Java). This suite's server
// therefore runs conf/MoquiLoadTestConf.xml, with tarpit-enabled="false" for AT_XML_SCREEN, AT_XML_SCREEN_TRANS and
// AT_SERVICE, as Java's load tests do; a server of your own needs the same for a real run. Screen and transition hits
// count against tarpits as in Java (the target screen of a request when it requires authentication, and every
// transition), so on the production conf the dashboard qvt here would be answered 429 after 120 hits a minute.

const LOAD_USER = "test.load"

spec({
    setupSpec() {
        try {
            login(LOAD_USER, params.loadPassword ?? "TestLoad1!!")
        } catch (e) {
            skip(`the ${LOAD_USER} user is not on this server (${e.message}): run ` +
                "moqui test framework --spec ClientFixtures --db existing on its database")
        }
    },
})

feature("each endpoint answers", {
    when() {
        const key = `test.load.endpoints.${Date.now()}`
        return {
            login: session().get("/Login"),
            shell: http.get("/qapps/system/dashboard"),
            qvt: http.get("/apps/system/dashboard.qvt"),
            tui: http.get("/apps/system/Security/UserGroup/UserGroupList", { params: { renderMode: "tui" } }),
            s1: http.get("/rest/s1/moqui/basic/enums", { params: { enumTypeId: "UserPreferenceKey" }, accept: "json" }),
            e1: http.get("/rest/e1/moqui.basic.Enumeration", { params: { enumTypeId: "UserPreferenceKey" }, accept: "json" }),
            set: http.post("/apps/setPreference", { form: { preferenceKey: key, preferenceValue: "v1" } }),
            prefs: http.get("/apps/getPreferences", { params: { keyRegexp: key } }),
            key,
        }
    },
    then({ login, shell, qvt, tui, s1, e1, set, prefs, key }) {
        login.status == 200
        login.contentType.startsWith("text/html")
        shell.status == 200
        shell.text.includes("moqui-config")
        qvt.status == 200
        qvt.text.includes("<template")
        tui.status == 200
        tui.json().screen != null
        tui.json().c.length > 0
        s1.status == 200
        s1.json().some((e) => e.enumId != null && e.enumTypeId == "UserPreferenceKey")
        e1.status == 200
        e1.json().length > 0
        set.status == 200
        prefs.json()[key] == "v1"
    },
})

feature("logins at once get distinct sessions", {
    when() {
        const users = Number(params.users ?? 4)
        const out = parallel(users, (index, args) => {
            login(args.user, args.password)
            const page = http.get("/apps/system/dashboard.qvt")
            const cookie = http.cookies().find((c) => c.name == "moqui.session")
            return { status: page.status, session: cookie ? cookie.value : null }
        }, { user: LOAD_USER, password: params.loadPassword ?? "TestLoad1!!" })
        return { users, out, sessions: new Set(out.results.map((r) => r && r.session)) }
    },
    then({ users, out, sessions }) {
        out.errors.length == 0
        out.results.every((r) => r.status == 200 && r.session != null)
        sessions.size == users
    },
})

feature("mixed reads and writes", {
    when() {
        return load({
            users: Number(params.users ?? 4), duration: Number(params.duration ?? 10), rampUp: 2, think: [50, 200],
            args: { user: LOAD_USER, password: params.loadPassword ?? "TestLoad1!!" },
            setup(user, args) { login(args.user, args.password) },            // once per user, not timed
            scenario(user, args) {                                             // repeated until the duration ends
                http.get("/rest/s1/moqui/basic/enums", { params: { enumTypeId: "UserPreferenceKey" }, accept: "json", name: "s1 enums" })
                http.get("/apps/system/dashboard.qvt", { name: "dashboard qvt" })
                http.get("/apps/system/Security/UserGroup/UserGroupList", { params: { renderMode: "tui" }, name: "UserGroupList tui" })
                http.post("/apps/setPreference", { form: { preferenceKey: `test.load.${user}`, preferenceValue: "x" },
                    name: "setPreference" })
            },
        })
    },
    then(stats) {
        stats.errors == 0
        stats.requests > 0
        stats.names["s1 enums"].p95 < Number(params.maxP95 ?? 2000)
        stats.names["dashboard qvt"].p95 < Number(params.maxP95 ?? 2000)
        stats.names["UserGroupList tui"].p95 < Number(params.maxP95 ?? 2000)
        stats.names["setPreference"].p95 < Number(params.maxP95 ?? 2000)
    },
})
