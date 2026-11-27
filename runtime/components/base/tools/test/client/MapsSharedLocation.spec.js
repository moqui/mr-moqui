// The guest page of a location share's link (plans/Location-Sharing.md §3.7; tools Maps/SharedLocation.xml and .qvue,
// webroot WebrootPublic.qvt.js), its one render in the regression suite: the link made from Maps › Browse is this
// server's https link with the secret in its fragment; the shell page is sent with Referrer-Policy strict-origin,
// no-store and a Content-Security-Policy of this server; the page opened with the fragment shows the share's label and
// the address bar no longer holds the secret; and the read refuses a secret in its request line.
const state = {}

spec({
    setupSpec() {
        needsFixtures("john.doe, a Maps user who starts a share")
        state.owner = session({user: "john.doe"})
        const made = state.owner
            .post("/apps/maps/Browse/createShare", {
                form: {
                    audience: JSON.stringify([{audienceTypeEnumId: "LsaLink"}]),
                    thruDate: String(Date.now() + 3600000),
                    precisionEnumId: "LspExact",
                    keepTrail: "false",
                    label: "Spec guest van"
                }
            })
            .json()
        state.made = made
        const parts = /^https?:\/\/[^/]+(\/[^#]*)#(.*)$/.exec(String(made.linkUrl || "")) || []
        state.path = parts[1]
        state.secret = parts[2]
    },
    cleanupSpec() {
        if (state.made && state.made.shareId) state.owner.post("/apps/maps/Browse/stopShare", {form: {shareId: state.made.shareId}})
    }
})

feature("the guest page: its link, its headers, the fragment cleared as it loads, and the secret refused in a request line", {
    expect() {
        const guest = session()
        const shell = guest.get(state.path, {accept: "html"})
        const policy = String(shell.header("Content-Security-Policy") || "")
        const inUrl = guest.post(state.path + "/read", {params: {linkSecret: state.secret}, form: {}, csrf: false}).json()
        const page = browser.newPage()
        const opened = page.goto(state.path + "#" + state.secret)
        const shown = page.waitFor(`(() => { const label = document.getElementById("SharedLocationLabel")
            return label ? { label: label.textContent.trim(), hash: location.hash, href: location.href } : null })()`, {timeout: 10})

        state.made.applied == true
        /^https?:\/\/[^/]+\/location\/[A-Za-z0-9_-]{16}#[A-Za-z0-9_-]{32}$/.test(state.made.linkUrl)
        shell.status == 200
        shell.header("Referrer-Policy") == "strict-origin"
        /no-store/.test(String(shell.header("Cache-Control") || ""))
        /default-src 'self'/.test(policy) && /img-src 'self'/.test(policy) && /connect-src 'self'/.test(policy)
        !shell.text.includes(state.secret)
        // refused before the transition runs: a transition with actions takes no URL parameters (the read refuses one too)
        inUrl.errorCode == 400 && inUrl.share == null
        opened.status == 200
        shown.label == "Spec guest van"
        shown.hash == "" && !shown.href.includes(state.secret)
    }
})
