// The guest page of a location share's link (plans/Location-Sharing.md §3.7; tools Maps/SharedLocation.xml and .qvue,
// webroot WebrootPublic.qvt.js), its one render in the regression suite: the link made from Maps › Browse is this
// server's link with the secret in its fragment; the shell page, rendered once (the browser's navigation, whose response
// headers goto answers), is sent with Referrer-Policy strict-origin, no-store and its Content-Security-Policy, which it
// enforces (an outside image refused) with nothing it loads refused; the page opened with the fragment
// shows the share's label and the address bar no longer holds the secret; and the read refuses a secret in its request
// line.
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

feature("the guest page: its link, its policy enforced with nothing it needs refused, the fragment cleared as it loads, and the secret refused in a request line", {
    expect() {
        const guest = session()
        const inUrl = guest.post(state.path + "/read", {params: {linkSecret: state.secret}, form: {}, csrf: false}).json()
        // the one render of the shell page: the browser's navigation, with its response headers
        const page = browser.newPage()
        const opened = page.goto(state.path + "#" + state.secret)
        const shown = page.waitFor(`(() => { const label = document.getElementById("SharedLocationLabel")
            return label ? { label: label.textContent.trim(), hash: location.hash, href: location.href } : null })()`, {timeout: 10})
        // nothing the page loaded was refused by its policy (a refusal is a console error, a page problem)
        const loadProblems = page.problems().filter(problem => /Content Security Policy|Refused to/i.test(String(problem)))
        // the policy is enforced: an image of a host that is not this server's nor a public basemap's is refused
        const probe = page.evaluate(`new Promise((resolve) => {
            const seen = []
            document.addEventListener("securitypolicyviolation", (event) => seen.push(event.violatedDirective))
            const image = new Image()
            image.onerror = () => setTimeout(() => resolve(seen), 50)
            image.onload = () => resolve(seen.concat(["loaded"]))
            image.src = "https://tiles.example.invalid/0/0/0.png"
        })`)
        page.takeProblems()

        state.made.applied == true
        /^https?:\/\/[^/]+\/location\/[A-Za-z0-9_-]{16}#[A-Za-z0-9_-]{32}$/.test(state.made.linkUrl)
        // refused before the transition runs: a transition with actions takes no URL parameters (the read refuses one too)
        inUrl.errorCode == 400 && inUrl.share == null
        opened.status == 200
        opened.headers["referrer-policy"] == "strict-origin"
        /no-store/.test(opened.headers["cache-control"] || "")
        /default-src 'self'/.test(opened.headers["content-security-policy"] || "")
        shown.label == "Spec guest van"
        shown.hash == "" && !shown.href.includes(state.secret)
        loadProblems.length == 0
        probe.length == 1 && /^img-src/.test(probe[0])
    }
})
