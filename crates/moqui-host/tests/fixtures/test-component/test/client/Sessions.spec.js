// Logins and the session token: the REST login of the default session, the token sent on a POST and refused without
// it, the login form with redirects off, logout, and a session per user.
spec({ setupSpec() { login(params.user ?? "john.doe", params.password ?? "moqui") } })

feature("the default session is logged in by setupSpec", {
    when() { return { res: http.get("/rest/s1/moqui/basic/enums", { params: { enumTypeId: "UserPreferenceKey" }, accept: "json" }) } },
    then({ res }) {
        res.status == 200
        Array.isArray(res.json())
    },
})

feature("a POST runs with the session token and stores nothing without it (csrf: #csrf)", {
    where: [{ csrf: true, status: 200, stored: true }, { csrf: false, status: 401, stored: false }],
    when({ csrf }) {
        const key = `moqui.test.client.${csrf}.${Date.now()}`
        const res = http.post("/apps/setPreference", { form: { preferenceKey: key, preferenceValue: "v1" }, csrf })
        const prefs = http.get("/apps/getPreferences", { params: { keyRegexp: key } }).json()
        return { res, prefs, key }
    },
    then({ res, status, prefs, key, stored }) {
        res.status == status
        !!prefs[key] == stored
    },
})

feature("the login form answers a redirect, and its session is logged in", {
    when() {
        const s = session()
        const res = s.formLogin("john.doe", "moqui")
        return { res, after: s.get("/rest/s1/moqui/basic/enumTypes", { accept: "json" }), s }
    },
    then({ res, after, s }) {
        res.status == 302
        res.location != null
        res.cookie(server.url.startsWith("https://") ? "__Host-moqui.session" : "moqui.session") != null
        after.status == 200
        s.cookies().some((c) => c.name == (server.url.startsWith("https://") ? "__Host-moqui.session" : "moqui.session"))
    },
})

// Last: a logout marks the account logged out (UserAccount.hasLoggedOut), which ends the user's other sessions too.
feature("a session of its own logs in by opts, and logout ends its login", {
    when() {
        const other = session({ user: "john.doe" })
        const before = other.get("/rest/s1/moqui/basic/enumTypes", { accept: "json" }).status
        const answer = other.logout().text
        const after = other.get("/rest/s1/moqui/basic/enumTypes", { accept: "json" }).status
        let refused = null
        try { session().login("no.such.user", "x") } catch (e) { refused = e.message }
        return { before, answer, after, refused }
    },
    then({ before, answer, after, refused }) {
        before == 200
        answer == "successful"
        after == 401
        refused.startsWith("login of no.such.user failed: HTTP 401")
    },
})
