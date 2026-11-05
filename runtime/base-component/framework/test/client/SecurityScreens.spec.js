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
        const res = session().get("/fop/apps/tools/dashboard")
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

// test_a01_more.py: test_login_form_action_is_login_path
feature("A01 the login form posts to the login path", {
    when() { return { body: session().get("/Login").text } },
    then({ body }) {
        body.includes("Login/login")
        !body.toLowerCase().includes("javascript:")
    },
})

// test_a01_more.py: test_view_only_cannot_run_service_run_over_http. A run that is allowed redirects (302), so a
// redirect is not a denial.
feature("A01 a VIEW-only user cannot run a service over HTTP", {
    given() { requireFixture(VIEW_USER, VIEW_PASSWORD) },
    when() {
        const s = formSession(VIEW_USER, VIEW_PASSWORD)
        if (!s.token()) skip("no CSRF token after VIEW-only login")
        const res = s.post("/apps/tools/Service/ServiceRun/run", {
            form: {
                serviceName: "org.moqui.impl.BasicServices.get#GeoRegionsForDropDown",
                moquiSessionToken: s.token(),
            },
        })
        return { res, body: res.text.toLowerCase() }
    },
    then({ res, body }) {
        res.status == 401 || res.status == 403
        !body.includes("session token required")
        !body.includes("session token is missing")
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
    where: [{ path: "/htmlr" }, { path: "/htmlr/Login" }, { path: "/htmlr/apps/tools/dashboard" }],
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
        const tools = s.get("/apps/tools/dashboard")
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
        const get = s.get("/apps/tools/Sql", { params: { db: "group:transactional", sql } })
        if (get.status == 401 || get.status == 403) skip("logged-in user lacks SQL_RUNNER_WEB (need ADMIN_ADV / demo john.doe)")
        if (!s.token()) skip("no CSRF token after login")
        const post = s.post("/apps/tools/Sql", {
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

// test_a01_more.py: test_fop_filename_quote_stays_in_one_content_disposition
feature("A01 a FOP filename quote stays in one Content-Disposition", {
    when() {
        const res = formSession(USER, PASSWORD).get("/fop/apps/tools/dashboard", { params: { filename: 'a"; x=y' } })
        if (res.status == 401 || res.status == 403) skip("user cannot render /fop/apps/tools/dashboard")
        return { cd: res.header("Content-Disposition") ?? "" }
    },
    then({ cd }) {
        !cd.includes('"; x=')
        !cd.includes("\r")
        !cd.includes("\n")
    },
})

// test_a01_more.py: test_datasnapshot_view_cannot_download
feature("A01 a VIEW-only user cannot download a data snapshot", {
    given() { requireFixture(VIEW_USER, VIEW_PASSWORD) },
    when() {
        return {
            res: formSession(VIEW_USER, VIEW_PASSWORD).get("/apps/tools/Data/Snapshots/downloadSnapshot", {
                params: { filename: "anything.zip" },
            }),
        }
    },
    then({ res }) {
        res.status == 401 || res.status == 403
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
        const res = session().get("/apps/tools/dashboard.csv")
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
        const res = formSession(VIEW_USER, VIEW_PASSWORD).get("/apps/tools/dashboard.csv")
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
        const res = formSession(USER, PASSWORD).get("/apps/tools/dashboard.csv")
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
