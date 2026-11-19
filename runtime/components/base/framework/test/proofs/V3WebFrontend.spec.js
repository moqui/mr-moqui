// OWASP ASVS 5.0.0 V3 Web Frontend Security: the framework's proofs, each feature filed under the requirement its name
// starts with (docs/security/proofs/V3-Web-Frontend-Security.md). Ported from Moqui Java's pytest proofs; a comment
// names the Python test, as in test/client.
//
// The cross-origin proofs need a server that allows no foreign origin, which is the default (an empty
// webapp-list/webapp[webroot]@allow-origins): the production and load test confs. On the dev conf, which allows
// localhost on any port, they fail, as they should. A preflight answers 204, and the origin is compared whole, scheme,
// host and port (Java compares the host only). The session cookie is moqui.session, __Host-moqui.session over https.

const SESSION_COOKIE = server.secure ? "__Host-moqui.session" : "moqui.session"
const TOKEN_ERROR = "Session token is missing or is not valid"
const EVIL_ORIGIN = "https://evil.example"
const EVIL = "https://evil.example/phish"
const OWN_ORIGIN = server.url.replace(/\/+$/, "").toLowerCase()
const HOST = server.host.includes(":") ? `[${server.host}]` : server.host
const OTHER_PORT_ORIGIN = `${server.secure ? "https" : "http"}://${HOST}:${server.port == 65535 ? 65534 : server.port + 1}`
const OTHER_SCHEME_ORIGIN = `${server.secure ? "http" : "https"}://${HOST}:${server.port}`

// The account a proof signs in with: the run's (--param user=NAME, its password in MOQUI_TEST_PASSWORD), else in the
// fixtures profile the demo data's john.doe.
const account = () => {
    if (params.user) return { username: params.user, password: params.password ?? "" }
    needsFixtures("an account to sign in with (or --param user=NAME and MOQUI_TEST_PASSWORD)")
    return { username: "john.doe", password: "moqui" }
}

// test_a02_headers.py: test_session_cookie_samesite_lax
// Does not prove: the visitor and device cookies' attributes (the device cookie's are test/client SecurityDevice's).
feature("v5.0.0-3.3.2: the session cookie is SameSite=Lax", {
    when() { return { cookie: session().get("/Login").cookie(SESSION_COOKIE) } },
    then({ cookie }) {
        cookie != null
        cookie.sameSite.toLowerCase() == "lax"
    },
})

// test_a02_headers.py: test_session_cookie_is_httponly
// Does not prove: that the session id never appears in a page's text, which is sampled in code, not here.
feature("v5.0.0-3.3.4: the session cookie is HttpOnly, out of reach of page scripts", {
    when() { return { cookie: session().get("/Login").cookie(SESSION_COOKIE) } },
    then({ cookie }) {
        cookie != null
        cookie.httpOnly
    },
})

// test_a02_headers.py: test_cors_unknown_origin_is_401, test_cors_preflight_unknown_origin_is_401
// Does not prove: the origins a deployment lists itself; /Login stands for the screen chain.
feature("v5.0.0-3.4.2: a foreign origin is 401 with no Access-Control-Allow-Origin (#method)", {
    where: [
        { method: "GET", headers: { Origin: EVIL_ORIGIN } },
        { method: "OPTIONS", headers: { Origin: EVIL_ORIGIN, "Access-Control-Request-Method": "POST" } },
    ],
    when({ method, headers }) { return { res: session().request(method, "/Login", { headers }) } },
    then({ res }) {
        res.status == 401
        res.header("Access-Control-Allow-Origin") == null
    },
})

// test_a02_headers.py: test_cors_same_origin_returns_allow_origin, test_cors_preflight_same_origin_is_allowed
// The control of the refusals: the server's own origin is allowed, exactly and never as a wildcard.
// Does not prove: the origins a deployment adds to @allow-origins, which take the same path.
feature("v5.0.0-3.4.2: the server's own origin is echoed exactly with credentials, its preflight allowed", {
    when() {
        return {
            res: session().get("/Login", { headers: { Origin: OWN_ORIGIN } }),
            preflight: session().options("/Login", { headers: { Origin: OWN_ORIGIN, "Access-Control-Request-Method": "POST" } }),
        }
    },
    then({ res, preflight }) {
        res.status == 200
        (res.header("Access-Control-Allow-Origin") ?? "").toLowerCase() == OWN_ORIGIN
        (res.header("Access-Control-Allow-Credentials") ?? "").toLowerCase() == "true"
        preflight.status == 204
        (preflight.header("Access-Control-Allow-Origin") ?? "").toLowerCase() == OWN_ORIGIN
        (preflight.header("Access-Control-Allow-Methods") ?? "").includes("POST")
    },
})

// Not in Java, which compares the host only and so allows these.
// Does not prove: a deployment that lists one of these origins on purpose, which fails it.
feature("v5.0.0-3.4.2: the server's host on #which is a foreign origin, refused (#method)", {
    where: [
        { which: "another port", method: "GET", headers: { Origin: OTHER_PORT_ORIGIN } },
        { which: "another scheme", method: "GET", headers: { Origin: OTHER_SCHEME_ORIGIN } },
        {
            which: "another port", method: "OPTIONS",
            headers: { Origin: OTHER_PORT_ORIGIN, "Access-Control-Request-Method": "POST" },
        },
        {
            which: "another scheme", method: "OPTIONS",
            headers: { Origin: OTHER_SCHEME_ORIGIN, "Access-Control-Request-Method": "POST" },
        },
    ],
    when({ method, headers }) { return { res: session().request(method, "/Login", { headers }) } },
    then({ res }) {
        res.status == 401
        res.header("Access-Control-Allow-Origin") == null
    },
})

// test_a07_authn.py: test_post_with_csrf_token_executes_preference_round_trip, test_post_without_csrf_does_not_store_preference
// Does not prove: the foreign-Origin refusal of a browser's request, nor calls authenticated by a header (Basic,
// api_key), which carry no session token by design.
feature("v5.0.0-3.5.1: a POST changes data only with the session's anti-forgery token (csrf: #csrf)", {
    where: [{ csrf: true, status: 200, stored: true }, { csrf: false, status: 401, stored: false }],
    when({ csrf }) {
        const { username, password } = account()
        const s = session()
        s.login(username, password)
        const key = `secCsrfHttp-${csrf}-${Date.now()}`
        const res = s.post("/apps/setPreference", { form: { preferenceKey: key, preferenceValue: "csrf-ran" }, csrf })
        const prefs = s.get("/apps/getPreferences", { params: { keyRegexp: key } })
        // blank it again, as the Python proofs do
        if (csrf) s.post("/apps/setPreference", { form: { preferenceKey: key, preferenceValue: "" } })
        return { res, prefs, key }
    },
    then({ res, status, prefs, key, stored }) {
        res.status == status
        res.status != 401 || res.text.includes(TOKEN_ERROR)
        prefs.status == 200
        (prefs.json()[key] == "csrf-ran") == stored
    },
})

// test_api_rest.py: test_entity_rest_post_without_csrf_does_not_create
// Does not prove: REST calls authenticated by a header (Basic, api_key), which carry no session token by design.
feature("v5.0.0-3.5.1: an entity REST POST without the session token is refused and creates nothing", {
    when() {
        const { username, password } = account()
        const res = session({ user: username, password }).post("/rest/e1/moqui.basic.Enumeration", {
            json: { enumId: "SEC_SHOULD_NOT_EXIST", description: "sec csrf" }, accept: "json", csrf: false,
        })
        // with the fixtures, the narrow VIEW user reads Enumeration: the row is not there
        const got = profile == "fixtures"
            ? session({ user: "sec.ent.view", password: "SecEntView1!!" })
                .get("/rest/e1/moqui.basic.Enumeration/SEC_SHOULD_NOT_EXIST", { accept: "json" })
            : null
        return { res, got }
    },
    then({ res, got }) {
        res.status == 401
        res.text.includes(TOKEN_ERROR)
        !res.text.includes("SEC_SHOULD_NOT_EXIST")
        got == null || got.status == 404
    },
})

// A form POST is a simple request, sent by a browser with no preflight; the server refuses a foreign Origin before it
// reads the form, so a site cannot submit the sign-in, or any form, from another origin.
// Does not prove: a request with no Origin header from a browser that sends none, which the session token covers (3.5.1).
feature("v5.0.0-3.5.2: a form POST from a foreign origin is refused before it is read, the own origin's is not", {
    when() {
        const { username, password } = account()
        const post = (origin) => {
            const s = session()
            s.get("/Login")
            return s.post("/Login/login", { form: { username, password }, headers: { Origin: origin } })
        }
        return { foreign: post(EVIL_ORIGIN), own: post(OWN_ORIGIN) }
    },
    then({ foreign, own }) {
        foreign.status == 401
        foreign.header("Access-Control-Allow-Origin") == null
        // the control: the same POST from the server's own origin signs in
        own.status == 302
        !(own.location ?? "").includes("/Login")
    },
})

// test_a01_redirect.py: test_login_returnTo_does_not_redirect_offsite, test_login_returnTo_same_origin_path_is_allowed
// Does not prove: redirects an application builds itself, nor the return targets of SSO.
feature("v5.0.0-3.7.2: the sign-in follows a returnTo on the site and never one off it", {
    when() {
        const { username, password } = account()
        const signIn = (returnTo) => {
            const s = session()
            s.get("/Login", { params: { returnTo } })
            return s.post("/Login/login", { form: { username, password } })
        }
        return { off: signIn(EVIL), on: signIn("/apps") }
    },
    then({ off, on }) {
        off.status == 302
        !off.location.toLowerCase().includes("evil.example")
        !off.location.includes("/phish")
        // the control: a returnTo on the site is followed
        on.status == 302
        on.location.startsWith("/") || on.location.startsWith(server.url)
        on.location.includes("/apps")
    },
})

// test_a01_redirect.py: test_login_referer_does_not_redirect_offsite
// Does not prove: redirects an application builds itself.
feature("v5.0.0-3.7.2: the sign-in does not redirect to an off-site Referer", {
    when() {
        const { username, password } = account()
        const s = session()
        s.get("/Login", { headers: { Referer: EVIL } })
        return { res: s.post("/Login/login", { form: { username, password } }) }
    },
    then({ res }) {
        [200, 301, 302, 303, 307, 308].includes(res.status)
        !(res.location ?? "").toLowerCase().includes("evil.example")
    },
})

// test_a01_redirect.py: test_login_returnTo_does_not_follow_host_header
// returnTo is compared with the request Host (webroot/Login.xml). The Host is forged with raw(): the redirect stays on
// the site, and a CR/LF in the request line is refused, not turned into a header.
// Does not prove: what a proxy in front does with a forged Host; where one refuses it, the feature skips.
feature("v5.0.0-3.7.2: the sign-in does not redirect off the site under a forged Host", {
    when() {
        const { username, password } = account()
        const s = session()
        const form = `username=${encodeURIComponent(username)}&password=${encodeURIComponent(password)}`
        const page = s.raw("GET /Login?returnTo=" + encodeURIComponent(EVIL) +
            " HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n")
        if (page.status != 200) skip(`a forged Host is refused before the sign-in page (HTTP ${page.status})`)
        const res = s.raw("POST /Login/login HTTP/1.1\r\nHost: evil.example\r\n" +
            "Content-Type: application/x-www-form-urlencoded\r\nContent-Length: " + form.length +
            "\r\nConnection: close\r\n\r\n" + form)
        const split = s.raw("GET /Login\r\nLocation: https://evil.example/phish HTTP/1.1\r\n" +
            "Host: localhost\r\nConnection: close\r\n\r\n", { cookies: false })
        return { page, res, split }
    },
    then({ page, res, split }) {
        page.cookie(SESSION_COOKIE) != null
        res.status == 301 || res.status == 302 || res.status == 303 || res.status == 307 || res.status == 308
        !(res.location ?? "").toLowerCase().includes("evil.example")
        !(res.location ?? "").includes("/phish")
        split.status == 400
        !(split.location ?? "").toLowerCase().includes("evil.example")
        !(split.location ?? "").includes("/phish")
    },
})

// Configuration in the matrix: none ships; a response-header of type screen-render named Referrer-Policy under
// webapp-list/webapp[webroot] sets one on every screen. A deployment's proof: the sign-in page's policy, by its header or
// a meta tag (which a browser takes over the header), sends no path or query to another site. The project's test server
// sets none, so in the fixtures profile it skips.
// Does not prove: the host name an origin policy still sends (no-referrer or same-origin keep it for an internal site),
// nor answers that are not screens, nor a link an application marks itself.
const SAFE_REFERRER = ["no-referrer", "same-origin", "origin", "strict-origin", "origin-when-cross-origin", "strict-origin-when-cross-origin"]
const KNOWN_REFERRER = SAFE_REFERRER.concat(["unsafe-url", "no-referrer-when-downgrade"])
feature("v5.0.0-3.4.5: the pages' referrer policy sends no path or query to other sites", {
    given() {
        if (profile == "fixtures") skip("the project's test server sets no Referrer-Policy (none ships): a deployment's proof")
    },
    when() {
        const res = session().get("/Login")
        // of a list, a browser takes the last policy it knows
        const last = (text) => text.split(",").map((p) => p.trim().toLowerCase()).filter((p) => KNOWN_REFERRER.includes(p)).pop() ?? null
        const meta = (res.text.match(/<meta\b[^>]*\bname="referrer"[^>]*\bcontent="([^"]*)"/i) ?? [])[1] ?? ""
        return { res, policy: last(meta) ?? last(res.headers("Referrer-Policy").join(",")) }
    },
    then({ res, policy }) {
        res.status == 200
        SAFE_REFERRER.includes(policy)
    },
})

// Configuration in the matrix: an application on a host of its own is a root-screen for that host with
// framework-routes="refuse" (runtime/conf/MoquiSitesConf.xml, a PASS site's); by default every app shares the server's
// origin. A deployment's proof: --param siteUrl=https://<the host of another application> names such a host. There the
// framework's apps, REST and socket are not found while the server's host serves them, and the session cookie of each host
// names no Domain, so the browser sends it to that host alone.
// Does not prove: that each other application of the deployment has a host (it checks the one named), nor the apps under
// /apps, which share the server's host by design.
feature("v5.0.0-3.5.4: another application's host serves none of the framework's apps; cookies stay per host", {
    given() {
        if (!params.siteUrl) skip("a deployment's proof: --param siteUrl=https://<the host of another application of the deployment>")
    },
    when() {
        const site = params.siteUrl.replace(/\/+$/, "")
        const paths = ["/apps/tools/Home", "/qapps/tools/Home", "/rest/e1/moqui.basic.Enumeration", "/rest/s1/moqui/basic/enums"]
        const socket = ws.connect(site + "/notws", { session: session() })
        socket.close()
        return {
            there: paths.map((path) => ({ path, res: session().get(site + path) })),
            here: paths.map((path) => ({ path, res: session().get(path) })),
            socket,
            cookies: [session().get("/Login"), session().get(site + "/")].flatMap((res) => res.cookies),
        }
    },
    then({ there, here, socket, cookies }) {
        // the control: the server's host has them, sending an anonymous client to sign in
        here.every(({ res }) => res.status == 302 || res.status == 401)
        there.every(({ res }) => res.status == 404)
        !socket.open
        cookies.every((cookie) => cookie.domain == null || cookie.domain == "")
    },
})

// The framework's two postMessage listeners, run in a browser on the scripts the server serves. The email sign-in window's
// answer (MoquiLib.js oauthPopup) is taken only from the window it opened, on the page's own origin, typed
// moqui-email-oauth and naming its sign-in; each message here is the MessageEvent a browser would make, with its origin
// and source. The Assist canvas frame (/assist/sandbox, WebrootAssistSandbox.js, an opaque origin) takes its port only from
// its parent window with the frame's nonce; those messages are posted, from the parent and from another window.
// Does not prove: the listeners of an application's own scripts, nor the sign-in answer's BroadcastChannel, which the
// browser keeps to the page's origin, nor the frame's refusal of another window where the page's policy forbids eval.
feature("v5.0.0-3.5.5: postMessage from another origin or window, or malformed, is dropped", {
    when() {
        const page = browser.newPage()
        page.goto("/Login")
        page.addScript("/js/MoquiLib.js")
        const oauth = page.evaluate(async () => {
            const tick = () => new Promise((resolve) => setTimeout(resolve, 30))
            let answer = null
            const signIn = window.moqui.oauthPopup(() => ({ signInId: "S1", authUrl: location.origin + "/favicon.ico" }),
                () => ({ status: "pending" }), { statusPollMs: 600000, pollMs: 600000 })
            signIn.then((result) => { answer = result })
            await tick()
            const popup = window.open("", "moquiOAuth")
            const send = (origin, source, data) => window.dispatchEvent(new MessageEvent("message", { origin, source, data }))
            const good = { type: "moqui-email-oauth", signInId: "S1", status: "success", emailAddress: "proof@example.com" }
            send("https://evil.example", popup, good)
            send(location.origin, window, good)
            send(location.origin, popup, Object.assign({}, good, { type: "moqui-other" }))
            send(location.origin, popup, Object.assign({}, good, { signInId: "S2" }))
            send(location.origin, popup, JSON.stringify(good))
            await tick()
            const dropped = answer
            send(location.origin, popup, good)
            await tick()
            signIn.cancel()
            return { popup: !!popup, dropped, taken: answer }
        })
        const frame = page.evaluate(async (nonce) => {
            const sandbox = document.createElement("iframe")
            sandbox.setAttribute("sandbox", "allow-scripts")
            sandbox.src = "/assist/sandbox#n=" + nonce
            const loaded = new Promise((resolve) => sandbox.addEventListener("load", resolve))
            document.body.appendChild(sandbox)
            await loaded
            const got = {}
            const offer = (name, data) => {
                const channel = new MessageChannel()
                channel.port1.onmessage = (event) => { got[name] = event.data && event.data.t }
                sandbox.contentWindow.postMessage(data, "*", [channel.port2])
            }
            offer("wrongNonce", { t: "port", nonce: "f".repeat(32) })
            offer("noNonce", { t: "port" })
            offer("wrongType", { t: "ports", nonce })
            // the same message from another window of this origin, not the frame's parent: a function of that window's
            // realm posts it (made by eval, which a page policy without 'unsafe-eval' refuses: then it is not tried)
            try {
                const other = document.createElement("iframe")
                document.body.appendChild(other)
                const postFromOther = other.contentWindow.eval(
                    "(target, data) => { const channel = new MessageChannel(); target.postMessage(data, '*', [channel.port2]); return channel.port1 }")
                postFromOther(sandbox.contentWindow, { t: "port", nonce }).onmessage = (event) => { got.otherWindow = event.data && event.data.t }
            } catch (err) {
                got.otherWindow = "not tried: " + err.message
            }
            // last, the control: the parent's message with the nonce opens the port
            offer("parent", { t: "port", nonce })
            for (let i = 0; i < 100 && !got.parent; i++) await new Promise((resolve) => setTimeout(resolve, 20))
            await new Promise((resolve) => setTimeout(resolve, 50))
            return got
        }, "0123456789abcdef0123456789abcdef")
        return { oauth, frame }
    },
    then({ oauth, frame }) {
        oauth.popup
        oauth.dropped == null
        // the control: the window's own well-formed answer is taken
        oauth.taken != null && oauth.taken.status == "success" && oauth.taken.signInId == "S1"
        frame.parent == "ready"
        frame.wrongNonce == null && frame.noNonce == null && frame.wrongType == null
        frame.otherWindow == null || frame.otherWindow.startsWith("not tried")
    },
})

// The tags of a page that embed a plugin, and attributes naming a plugin's file (Flash, Java applets, Silverlight,
// Shockwave).
const PLUGIN_TAG = /<(object|embed|applet)\b/i
const PLUGIN_FILE = /\b(src|data|href|code|archive)="[^"]*\.(swf|jar|xap|class|dcr)\b/i
// The major version a library's banner names in its first lines: "vue v3.5.44", "Quasar Framework v2.35.1".
const bannerMajor = (text, name) => Number((text.slice(0, 400).match(new RegExp(`${name} v(\\d+)\\.\\d+\\.\\d+`, "i")) ?? [])[1] ?? NaN)

// The pages run on Vue 3 and Quasar 2, both supported lines (Vue 2 ended on 2023-12-31, Quasar 1 with it), and embed no
// plugin. The versions are read from the banners of the files the signed-in shell loads.
// Does not prove: that every library under /libs is current (the list is libs/MANIFEST.json; Font Awesome is its old 4.7
// line, a style sheet and font), nor an application's own scripts.
feature("v5.0.0-3.7.1: pages embed no plugin; the shell runs on supported Vue 3 and Quasar 2", {
    when() {
        const { username, password } = account()
        const s = session({ user: username, password })
        const login = session().get("/Login")
        const shell = s.get("/qapps")
        const sources = [...shell.text.matchAll(/<script\b[^>]*\bsrc="([^"]+)"/gi)].map((match) => match[1])
        const vue = sources.find((src) => /\/vue[^/]*\.js/i.test(src))
        const quasar = sources.find((src) => /\/quasar[^/]*\.umd[^/]*\.js/i.test(src))
        return {
            login, shell, vue, quasar,
            vueMajor: vue ? bannerMajor(s.get(vue).text, "vue") : NaN,
            quasarMajor: quasar ? bannerMajor(s.get(quasar).text, "Quasar Framework") : NaN,
        }
    },
    then({ login, shell, vue, quasar, vueMajor, quasarMajor }) {
        login.status == 200
        shell.status == 200
        [login.text, shell.text].every((text) => !PLUGIN_TAG.test(text) && !PLUGIN_FILE.test(text))
        vue != null && quasar != null
        vueMajor == 3
        quasarMajor == 2
    },
})
