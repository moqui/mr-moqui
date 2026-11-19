// OWASP ASVS 5.0.0 V14 Data Protection: the framework's proofs, each feature filed under the requirement its name starts
// with (docs/security/proofs/V14-Data-Protection.md). Ported from Moqui Java's pytest proofs; a comment names the Python
// test, as in test/client.

// test_a02_headers.py: test_login_html_is_not_publicly_cacheable
// Does not prove: signed-in pages, /dav and the host routes; the sign-in page stands for the screen chain's header.
feature("v5.0.0-14.3.2: a screen's answer is Cache-Control no-store and never public", {
    when() { return { cc: (session().get("/Login").header("Cache-Control") ?? "").toLowerCase() } },
    then({ cc }) {
        cc.includes("no-store")
        !cc.includes("public")
    },
})

// The account a proof signs in with: the run's (--param user=NAME, its password in MOQUI_TEST_PASSWORD), else in the
// fixtures profile the demo data's john.doe.
const account = () => {
    if (params.user) return { username: params.user, password: params.password ?? "" }
    needsFixtures("an account to sign in with (or --param user=NAME and MOQUI_TEST_PASSWORD)")
    return { username: "john.doe", password: "moqui" }
}
const OWN_ORIGIN = server.url.replace(/^(https?:\/\/[^/]+).*$/i, "$1").toLowerCase()
// Whether a URL a page loads is on another site: absolute or scheme-relative, and not this server's origin.
const offSite = (url) => /^\s*(https?:)?\/\//i.test(url) && !url.trim().toLowerCase().startsWith(OWN_ORIGIN + "/")
// What a page's tags load: script, style sheet and other links, images, frames, media and objects (the first URL of a
// srcset). Inline JSON and text are not loads; a URL there (a font a user may choose) loads only once chosen.
const loads = (html) => [...html.matchAll(/<(script|link|img|iframe|frame|source|audio|video|track|embed|object|input)\b[^>]*?\s(src|href|data|srcset|poster)="([^"]*)"/gi)]
    .map((match) => match[3].split(",")[0].trim().split(/\s+/)[0])
// What a style sheet loads: its @import and url() targets.
const sheetLoads = (css) => [...css.matchAll(/@import\s+(?:url\()?\s*["']?([^"')\s;]+)|url\(\s*["']?([^"')]+)/gi)].map((match) => match[1] ?? match[2])
// A page's loads, with those of the style sheets it links on this server.
const pageLoads = (s, res) => {
    const own = loads(res.text)
    const sheets = [...res.text.matchAll(/<link\b[^>]*\brel="stylesheet"[^>]*\bhref="([^"]+)"/gi)].map((match) => match[1]).filter((href) => !offSite(href))
    return own.concat(sheets.flatMap((href) => sheetLoads(s.get(href).text)))
}

// No third-party script, analytics, tag manager, font or pixel is on the framework's pages: their scripts, style sheets,
// fonts and images are the server's own (webroot/libs), and the browser telemetry (RUM, off by default) posts to the
// server's /rum. So no request of these pages tells another site who is using them.
// Does not prove: what a script loads at run time (a font a user chooses from Google Fonts loads from there once chosen,
// as the Appearance panel says), nor the links a user follows, nor an application's own pages.
feature("v5.0.0-14.2.3: the sign-in page loads nothing from another site", {
    when() {
        const s = session()
        const res = s.get("/Login")
        return { res, loaded: pageLoads(s, res) }
    },
    then({ res, loaded }) {
        res.status == 200
        // the control: the page does load scripts and style sheets, from this server
        loaded.length > 0
        loaded.filter(offSite).length == 0
    },
})

// Does not prove: as above; the app pages are the shell (/qapps) and the plain HTML app list (/apps/AppList).
feature("v5.0.0-14.2.3: the signed-in shell and app pages load nothing from another site", {
    when() {
        const { username, password } = account()
        const s = session({ user: username, password })
        const pages = ["/qapps", "/apps/AppList"].map((path) => ({ path, res: s.get(path) }))
        return { pages, loaded: pages.flatMap(({ res }) => pageLoads(s, res)) }
    },
    then({ pages, loaded }) {
        pages.every(({ res }) => res.status == 200)
        loaded.length > 0
        loaded.filter(offSite).length == 0
    },
})
