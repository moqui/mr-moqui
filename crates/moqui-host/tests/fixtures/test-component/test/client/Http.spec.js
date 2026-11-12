// The client harness over HTTP against a running server: anonymous pages, redirects off and on, the cookie attributes,
// Basic, a raw request, a saved body, params, files and the polling helpers. No execution context: ec names its
// client equivalent.

feature("an anonymous GET /Login is HTML with a session cookie that is HttpOnly and SameSite=Lax", {
    when() { return { res: http.get("/Login") } },
    then({ res }) {
        res.status == 200
        res.ok
        res.contentType.includes("text/html")
        res.cookie("moqui.session").httpOnly
        res.cookie("moqui.session").sameSite == "Lax"
        res.headers("set-cookie").length >= 1
        res.timeMs >= 0
        http.token() != null
        res.assertContains("<html")
    },
})

feature("redirects are off unless asked: #path", {
    where: [{ path: "/apps/system/Home" }, { path: "/qapps/system/Home" }],
    when({ path }) { return { off: session().get(path), on: session().get(path, { follow: true }) } },
    then({ off, on }) {
        off.status == 302
        off.location.includes("/Login")
        on.status == 200
        on.url.includes("/Login")
    },
})

feature("Basic logs a request in; an unknown user is 401 (#user)", {
    where: [{ user: "john.doe", password: "moqui", status: 200 }, { user: "no.such.user", password: "x", status: 401 }],
    when({ user, password }) {
        return { res: session().get("/rest/s1/moqui/basic/enumTypes", { basic: [user, password], accept: "json" }) }
    },
    then({ res, status }) {
        res.status == status
    },
})

feature("a raw request is sent as it is and its answer parsed", {
    when() {
        return {
            login: http.raw("GET /Login HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
            dotdot: http.raw("GET /js/../../Login.xml HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
        }
    },
    then({ login, dotdot }) {
        login.status == 200
        login.text.includes("<html")
        // the path normalizes to /Login.xml, the Login screen in the xml render mode, not the screen's file
        !dotdot.text.includes("<screen")
        !dotdot.text.includes("<transition")
    },
})

feature("a body saves into the output directory", {
    when() { return { path: http.get("/Login").save("login.html") } },
    then({ path }) { path.endsWith("login.html") },
})

feature("params, server and files", {
    expect() {
        params.greeting == "hello"
        server.url.startsWith("http://127.0.0.1:")
        !server.secure
        files.list("test/client", /\.spec\.js$/).includes("test/client/Http.spec.js")
        files.read("component.xml").includes("moqui-test-fixture")
    },
})

feature("files stay in the component", {
    when() {
        const tried = (fn) => { try { fn(); return "read" } catch (e) { return e.message } }
        return { up: tried(() => files.read("../component.xml")), abs: tried(() => files.read("/etc/hostname")) }
    },
    then({ up, abs }) {
        up.includes("is not a path within the component")
        abs.includes("is not a path within the component")
    },
})

feature("waitUntil polls until true, and ec names the client equivalent", {
    when() {
        let n = 0
        const value = waitUntil(() => ++n >= 3 && n, { timeout: 5, interval: 10 })
        let timedOut = null
        try { waitUntil(() => false, { timeout: 0.05, interval: 10 }) } catch (e) { timedOut = e.message }
        let noEc = null
        try { ec.user.username } catch (e) { noEc = e.message }
        return { value, timedOut, noEc }
    },
    then({ value, timedOut, noEc }) {
        value == 3
        timedOut.startsWith("waitUntil: not true after 0.05s")
        noEc.includes("`ec` is the server's")
    },
})
