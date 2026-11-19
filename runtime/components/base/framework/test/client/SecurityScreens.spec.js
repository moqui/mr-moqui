// A01 Broken Access Control on screens over HTTP, ported from Moqui Java's pytest proofs
// (framework/test/test_a01_more.py; the catalog in README.md): public paths, the login form, FOP, the email
// tracking pixel, and the tool screens. Passes on any conf; the features that need sec.view.only skip without
// that user. There is no /htmlr here, so those proofs are 404s. The tool paths are the ones this server mounts,
// the same as Java's.

// The fixture user can log in from here, else the feature skips. A session of its own, not logged out: a REST logout
// ends every session of the user (UserAccount.hasLoggedOut).
const requireFixture = (username, password) => {
    const res = session().post("/rest/login", { form: { username, password }, accept: "json", csrf: false })
    if (res.status == 401) skip(`${username} is not on this server: run test/ClientFixtures.spec.js on its database (moqui test framework --spec ClientFixtures --db existing), or use --serve`)
    return res
}
// A session logged in through the login form, as the Python screen_login.
const formSession = (username, password) => {
    const s = session()
    const res = s.formLogin(username, password)
    assert(res.status == 302, `the login form did not log ${username} in: HTTP ${res.status}`)
    return s
}

const USER = params.user ?? "john.doe"
const PASSWORD = params.password ?? "moqui"
const VIEW_USER = "sec.view.only"
const VIEW_PASSWORD = "SecView1!!"

// test_a01_more.py: test_echopath_extra_path_is_html_escaped
feature("A01 an extra path on echopath is HTML-escaped", {
    when() { return { res: session().get("/echopath/<script>alert(1)</script>") } },
    then({ res }) {
        !res.text.includes("<script>alert(1)</script>")
    },
})

// test_a01_more.py: test_fop_without_login_is_not_a_pdf
feature("A01 FOP of a screen without a login is not a PDF", {
    when() {
        const res = session().get("/fop/apps/tools/Home")
        return { res, ctype: (res.contentType ?? "").toLowerCase() }
    },
    then({ res, ctype }) {
        res.status == 401 || res.status == 403 || res.status == 302 || res.status == 404 || !ctype.includes("pdf")
        res.status != 200 || res.magic != "%PDF"
    },
})

// test_a01_more.py: test_email_tracking_unknown_id_returns_png
feature("A01 an unknown email tracking id returns the 1x1 PNG", {
    when() { return { res: session().get("/email/NOTAREALID.png") } },
    then({ res }) {
        res.status == 200
        res.contentType.toLowerCase().includes("png")
        !res.contentType.toLowerCase().includes("charset")
        res.magic == "89504e47"
        res.length == 68
        res.header("Content-Disposition") == "inline"
    },
})

// plans/Security-Review.md SEC-TLS-01: Export's EntityExport writes on the server, so a GET with URL parameters (a link
// from another site) does not run it; it takes a POST with the session token.
feature("Tools Data Export does not run on a GET with URL parameters", {
    when() {
        requireFixture(USER, PASSWORD)
        const s = formSession(USER, PASSWORD)
        const res = s.get("/apps/tools/Data/Export/EntityExport?entityNames=moqui.basic.Enumeration&fileType=JSON&output=browser&filterMap=%7B%7D")
        return { res }
    },
    then({ res }) {
        !(res.header("Content-Disposition") ?? "").includes("attachment")
        !(res.text ?? "").includes("EnumerationType")
    },
})

// plans/Security-Review.md SEC-WEB-04: a transition that changes data does not run on a GET another site started (a
// link the browser follows with the Lax cookie); the app's own link still does. Logout is the one that needs no
// parameter: refused cross-site, the session stays signed in.
feature("a GET of a writing transition that another site started is refused; the app's own runs", {
    when() {
        requireFixture(USER, PASSWORD)
        const s = formSession(USER, PASSWORD)
        const before = s.get("/apps/tools/Home")
        const crossSite = s.get("/Login/logout", { headers: { "Sec-Fetch-Site": "cross-site" } })
        const otherReferer = s.get("/Login/logout", { headers: { Referer: "https://evil.example/page" } })
        const still = s.get("/apps/tools/Home")
        const own = s.get("/Login/logout", { headers: { "Sec-Fetch-Site": "same-origin" } })
        const after = s.get("/apps/tools/Home")
        return { before, crossSite, otherReferer, still, own, after }
    },
    then({ before, crossSite, otherReferer, still, own, after }) {
        before.status == 200
        crossSite.status == 403
        otherReferer.status == 403
        still.status == 200
        own.status != 403
        after.status != 200 || after.text.toLowerCase().includes("login")
    },
})

// plans/Security-Review-Low.md SEC-CLI-102: the apps' pages sign in every request by a device key, and the desktop's
// host derives the `Sec-Fetch-Site` its runtime does not send a custom scheme, so a key-signed GET of a writing transition
// that says another site started it (a frame inside the page) is refused as a browser's is; the page's own, and a
// program's with no fetch metadata, run. The login key is ClientFixtures' (sec.key.http).
const KEY_USER = "sec.key.http"
const KEY_PASSWORD = "SecKeyHttp1!!"
const KEY = "sec-test-login-key-fixed-40-chars-value"
feature("a key-signed GET of a writing transition that says another site started it is refused", {
    when() {
        requireFixture(KEY_USER, KEY_PASSWORD)
        const get = (headers) => session().get("/Login/logout", { loginKey: KEY, headers })
        return { crossSite: get({ "Sec-Fetch-Site": "cross-site" }), own: get({ "Sec-Fetch-Site": "same-origin" }), program: get({}) }
    },
    then({ crossSite, own, program }) {
        crossSite.status == 403
        own.status != 403
        program.status != 403
    },
})

// plans/Security-Review-Low.md SEC-RTE-08, SEC-ADM-09: the read-only transitions that changed state on a GET are not
// read-only now. /sso/logout another site started is refused and the session stays signed in; setPrefGoLast takes only a
// POST, so a GET with the preference in its URL sets nothing; the DataDocument field tree's createFieldDynamic is a POST
// from the tree, and a GET with its parameters in the URL is refused.
feature("a GET of sso/logout, setPrefGoLast or createFieldDynamic changes nothing", {
    when() {
        requireFixture(USER, PASSWORD)
        const s = formSession(USER, PASSWORD)
        const key = "SPEC_RTE08_PREF"
        const read = () => s.get("/apps/getPreferences?keyRegexp=" + key, { accept: "json" }).json()?.[key] ?? null
        const before = read()
        const pref = s.get("/apps/setPrefGoLast?preferenceKey=" + key + "&preferenceValue=from-a-link")
        const after = read()
        const field = s.get("/apps/tools/Data/Documents/Edit/DataDocFields/FieldList/createFieldDynamic?dataDocumentId=MoquiEmailMessage&fieldPath=specRte08")
        const crossSite = s.get("/sso/logout", { headers: { "Sec-Fetch-Site": "cross-site" } })
        const still = s.get("/apps/tools/Home")
        return { before, pref, after, field, crossSite, still }
    },
    then({ before, pref, after, field, crossSite, still }) {
        pref.status >= 400
        after == before
        after != "from-a-link"
        field.status == 400
        field.text.includes("URL parameters")
        crossSite.status == 403
        still.status == 200
    },
})

// plans/Security-Review.md SEC-RTE-05, SEC-FIL-01: /resources is read-only, so its file changes are refused when another
// site started the request; a read is not
feature("a /resources file change another site started is refused", {
    when() {
        const s = formSession(USER, PASSWORD)
        const page = s.get("/apps/system/Resource")
        if (page.status == 401 || page.status == 403) skip("user cannot open System Resource")
        const opened = s.get("/resources?cmd=open&init=1", { headers: { "Sec-Fetch-Site": "same-origin" } })
        let hash = null
        try { hash = opened.json()?.cwd?.hash } catch (e) { hash = null }
        if (!hash) skip("no root folder to name")
        const made = s.get(`/resources?cmd=mkdir&target=${encodeURIComponent(hash)}&name=spec-cross-site`,
            { headers: { "Sec-Fetch-Site": "cross-site" } })
        const crossRead = s.get("/resources?cmd=open&init=1", { headers: { "Sec-Fetch-Site": "cross-site" } })
        return { made: made.text.toLowerCase(), crossRead }
    },
    then({ made, crossRead }) {
        made.includes("another site")
        crossRead.status == 200
    },
})

// test_a01_more.py: test_login_form_action_is_login_path
feature("A01 the login form posts to the login path", {
    when() { return { body: session().get("/Login").text } },
    then({ body }) {
        body.includes("Login/login")
        !body.toLowerCase().includes("javascript:")
    },
})

// test_a01_more.py: test_unauthenticated_data_import_does_not_fetch_location. A load that ran redirects to the
// screen, so a 302 counts only when it is the login.
feature("A01 an anonymous data import does not fetch a location", {
    when() {
        const res = session().post("/apps/tools/Data/Load/load", {
            form: { location: "http://127.0.0.1:9/sec-ssrf" },
        })
        return { res, location: res.location ?? "" }
    },
    then({ res, location }) {
        res.status == 401 || res.status == 403 || (res.status == 302 && location.includes("/Login"))
        res.status != 200
    },
})

// test_a01_more.py: test_apps_qzsign_without_login_is_401
feature("A01 qzSign without a login is 401 and not a signature", {
    when() { return { res: session().get("/apps/qzSign", { params: { message: "hello" } }) } },
    then({ res }) {
        res.status == 401 || res.status == 403
        !res.text.toLowerCase().includes("qz-private-key")
        res.magic != "%PDF"
    },
})

// test_a01_more.py: test_htmlr_anonymous_error_does_not_include_stack, test_htmlr_tools_without_login_is_not_a_pdf.
// There is no /htmlr here; the proof that nothing answers at its path is kept.
feature("A01 no HTML renderer is mounted at #path", {
    where: [{ path: "/htmlr" }, { path: "/htmlr/Login" }, { path: "/htmlr/apps/tools/Home" }],
    when({ path }) {
        const res = session().get(path)
        return { res, ctype: (res.contentType ?? "").toLowerCase(), location: res.location ?? "" }
    },
    then({ res, ctype, location }) {
        res.status == 404
        !ctype.includes("pdf")
        res.magic != "%PDF"
        !res.text.includes("at org.moqui")
        !res.text.includes("Caused by:")
        !res.text.includes("MalformedURLException")
        !res.text.includes("NoClassDefFoundError")
        !location.includes("/htmlr")
    },
})

// test_a01_more.py: test_login_get_with_password_is_rejected. The password is the demo user's own: a wrong one would
// lock the account. A login that succeeded is not 200 either, so the same session must still be sent to login.
feature("A01 a GET of the login with a password does not log in", {
    when() {
        const s = session()
        const res = s.get("/Login/login", { params: { username: USER, password: PASSWORD } })
        const tools = s.get("/apps/tools/Home")
        return { res, tools, flat: res.text.replace(/\s/g, "").toLowerCase(), location: tools.location ?? "" }
    },
    then({ res, tools, flat, location }) {
        res.status != 200 || res.text.toLowerCase().includes("non-secure") || res.text.toLowerCase().includes("url parameters")
        !flat.includes('"loggedin":true')
        tools.status == 302
        location.includes("/Login")
    },
})

// test_a01_more.py: test_web_inf_is_not_served
feature("A01 WEB-INF is not served", {
    when() { return { res: session().get("/WEB-INF/web.xml") } },
    then({ res }) {
        res.status == 404 || res.status == 403
        !res.text.toLowerCase().includes("<web-app")
    },
})

// test_a01_more.py: test_notws_http_get_is_not_a_websocket_handshake
feature("A01 a GET of /notws is not a WebSocket handshake", {
    when() {
        const res = session().get("/notws")
        return { res, connection: (res.header("Connection") ?? "").toLowerCase() }
    },
    then({ res, connection }) {
        res.status != 101
        !connection.includes("upgrade")
    },
})

// test_a01_more.py: test_sql_runner_query_string_sql_is_not_executed. The statement may be shown in the text area; it
// must not have been run. The POST, with the session token, does run it.
feature("A01 the SQL Runner does not run a statement from the query string", {
    when() {
        const s = formSession(USER, PASSWORD)
        const probe = "sec_sql_probe_col"
        const sql = "SELECT 1 AS " + probe
        const get = s.get("/apps/tools/SqlTui", { params: { db: "group:transactional", sql } })
        if (get.status == 401 || get.status == 403) skip("logged-in user lacks SQL_RUNNER_WEB (need ADMIN_ADV / demo john.doe)")
        if (!s.token()) skip("no CSRF token after login")
        const post = s.post("/apps/tools/SqlTui/run", {
            form: { db: "group:transactional", sql, moquiSessionToken: s.token() },
            follow: true,
        })
        return { getBody: get.text.toLowerCase(), postBody: post.text.toLowerCase(), probe: probe.toLowerCase() }
    },
    then({ getBody, postBody, probe }) {
        !getBody.includes("result set")
        !getBody.includes("showing all")
        !postBody.includes("session token required")
        postBody.includes("result set") || postBody.includes(probe) || postBody.includes("showing all")
    },
})

// The data grid takes a SQL source only in the request body, as the SQL screen takes its SQL.
feature("A01 the data grid does not run a SQL source from the query string", {
    when() {
        const s = formSession(USER, PASSWORD)
        const sourceJson = JSON.stringify({ type: "sql", db: "group:transactional", sql: "SELECT 1 AS sec_grid_probe" })
        const res = s.get("/apps/tools/Data/Grid/gridColumns", { params: { sourceJson } })
        if (res.status == 401 || res.status == 403) skip("logged-in user cannot open the Tools app")
        return { res, body: res.text.toLowerCase() }
    },
    then({ res, body }) {
        !body.includes("sec_grid_probe")
        body.includes("request body")
    },
})

// test_a01_more.py: test_menu_data_tools_without_login_is_not_the_menu
feature("A01 menu data for Tools without a login is not the menu", {
    when() {
        const res = session().get("/menuData/apps/tools")
        return { res, body: res.text.toLowerCase() }
    },
    then({ res, body }) {
        res.status == 401 || res.status == 403 || res.status == 302 || !body.includes('"schema"')
        !body.includes("groovyshell") || res.status != 200
    },
})

// test_a01_more.py: test_datasnapshot_parent_segment_filename_is_not_found
feature("A01 a DataSnapshot download of a parent segment is not found", {
    when() {
        const res = formSession(USER, PASSWORD).get("/apps/tools/Data/Snapshots/downloadSnapshot", {
            params: { filename: "../../conf/MoquiProductionConf.xml" },
        })
        if (res.status == 401 || res.status == 403) skip("user cannot open DataSnapshot")
        return { res, body: res.text.toLowerCase() }
    },
    then({ res, body }) {
        res.status == 400 || res.status == 404
        !body.includes("moqui-conf")
        !body.includes("crypt-pass")
    },
})

// test_a01_more.py: test_datasnapshot_parent_segment_delete_is_rejected, test_datasnapshot_parent_segment_import_is_rejected
feature("A01 a DataSnapshot #name of a parent segment is rejected", {
    where: [
        { name: "delete", path: "/apps/tools/Data/Snapshots/deleteSnapshot", field: "filename" },
        { name: "import", path: "/apps/tools/Data/Snapshots/importSnapshot", field: "zipFilename" },
    ],
    when({ path, field }) {
        const s = formSession(USER, PASSWORD)
        if (!s.token()) skip("no CSRF token after login")
        const res = s.post(path, { form: { [field]: "../../conf/MoquiProductionConf.xml", moquiSessionToken: s.token() } })
        if (res.status == 401 || res.status == 403) skip("user cannot open DataSnapshot")
        return { res, body: res.text.toLowerCase() }
    },
    then({ res, body }) {
        res.status != 500
        !body.includes("crypt-pass")
        !body.includes("moqui-conf")
    },
})

// test_a01_more.py: test_elfinder_component_webroot_put_does_not_write_screen, now at /resources (webroot.xml), the
// elFinder protocol without the removed screen. files.list stays in this component, so it cannot see webroot's
// screens; the command's answer is the proof that the put was refused.
feature("A01 a /resources put does not write a screen under component://webroot", {
    when() {
        const s = formSession(USER, PASSWORD)
        const page = s.get("/apps/system/Resource")
        if (page.status == 401 || page.status == 403) skip("user cannot open System Resource")
        if (!s.token()) skip("no CSRF token after login")
        const res = s.post("/resources", {
            form: {
                cmd: "put",
                resourceRoot: "component://webroot",
                target: "v0_cm9vdA",
                content: '<screen require-authentication="false"></screen>',
                moquiSessionToken: s.token(),
            },
        })
        const body = res.text.toLowerCase()
        if ((res.status == 401 || res.status == 403) && body.includes("session token")) {
            return { body, ctype: (res.contentType ?? "").toLowerCase(), csrfRejected: true }
        }
        if (res.status == 401 || res.status == 403) skip("user cannot run /resources commands")
        return { body, ctype: (res.contentType ?? "").toLowerCase(), csrfRejected: false }
    },
    then({ body, ctype, csrfRejected }) {
        !csrfRejected
        ctype.includes("json")
        body.includes("write not allowed") || body.includes("error")
        !body.includes("changed")
    },
})

// test_a01_more.py: test_anonymous_tools_csv_is_not_a_data_dump
feature("A01 an anonymous Tools CSV is not a data dump", {
    when() {
        const res = session().get("/apps/tools/Home.csv")
        return { res, ctype: (res.contentType ?? "").toLowerCase(), body: res.text.toLowerCase() }
    },
    then({ res, ctype, body }) {
        res.status == 401 || res.status == 403 || res.status == 302 || res.status == 303 || body.includes("login")
        res.status != 200 || !ctype.includes("text/csv")
        res.status != 200 || !body.includes("auto screen")
    },
})

// test_a01_more.py: test_view_only_tools_csv_is_not_a_privilege_escalation
feature("A01 a VIEW-only Tools CSV is not a privilege escalation", {
    given() { requireFixture(VIEW_USER, VIEW_PASSWORD) },
    when() {
        const res = formSession(VIEW_USER, VIEW_PASSWORD).get("/apps/tools/Home.csv")
        return { res, body: res.text.toLowerCase() }
    },
    then({ res, body }) {
        res.status != 500
        !body.includes("sql runner") || res.status == 401 || res.status == 403
    },
})

// test_a01_more.py: test_authorized_tools_csv_is_csv
feature("A01 a Tools CSV for a user who may open Tools is CSV", {
    when() {
        const res = formSession(USER, PASSWORD).get("/apps/tools/Home.csv")
        if (res.status == 401 || res.status == 403) skip("user cannot open Tools dashboard")
        return { res, ctype: (res.contentType ?? "").toLowerCase() }
    },
    then({ res, ctype }) {
        res.status == 200
        ctype.includes("csv") || res.text.includes(",") || ctype.includes("text/plain")
    },
})

// test_a01_more.py: test_production_data_import_rejects_remote_location
feature("A01 a data import rejects a remote location", {
    when() {
        const s = formSession(USER, PASSWORD)
        if (!s.token()) skip("no CSRF token after login")
        const res = s.post("/apps/tools/Data/Load/load", {
            form: { location: "http://127.0.0.1:9/sec-ssrf", moquiSessionToken: s.token() },
            follow: true,
        })
        return { res, body: res.text.toLowerCase() }
    },
    then({ res, body }) {
        res.status != 500
        body.includes("not allowed") || body.includes("production") || res.status == 401 || res.status == 403
    },
})
