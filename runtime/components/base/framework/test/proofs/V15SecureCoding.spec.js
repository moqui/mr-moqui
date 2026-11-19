// OWASP ASVS 5.0.0 V15 Secure Coding and Architecture: the framework's proofs, each feature filed under the requirement its
// name starts with (docs/security/proofs/V15-Secure-Coding-and-Architecture.md).

const loggedIn = (res) => res.status == 200 && res.text.replace(/\s/g, "").toLowerCase().includes('"loggedin":true')
// The common headers a proxy names a client's address in, each naming `address`.
const forwarding = (address) => ({
    "X-Forwarded-For": address, "X-Real-IP": address, Forwarded: `for=${address}`, "CF-Connecting-IP": address,
    "True-Client-IP": address, "X-Client-IP": address, "Fastly-Client-IP": address, "X-Cluster-Client-IP": address,
    "CloudFront-Viewer-Address": `${address}:443`,
})
const restLogin = (username, password, headers) =>
    session().post("/rest/login", { json: { username, password }, accept: "json", csrf: false, headers })
// The address an ipAllowed refusal names, when it names one.
const namedAddress = (res) => (res.text.match(/client IP (\S+) is not in allowed list/) ?? [])[1] ?? null

// Configuration in the matrix: a production build has no devtools feature, and a production instance purpose loads
// neither the dev nor the test component set (runtime/components/components.xml; ./moq archive leaves both out). From
// outside, their routes are not found: Dev AIDE's /devstatic files are anonymous, so a dev instance serves them, and Dev
// AIDE's app and the example component's Gallery mount under /apps, where a missing app is not found before any sign-in
// while an app that is there sends an anonymous client to sign in.
// Does not prove: the build's features (devtools), a component of the test set (none has a fixed route), nor the test
// hooks of a moqui test process, which the project's --serve server is, and which take effect only from loopback.
feature("v5.0.0-15.2.3: no development or example component's pages or files are served", {
    when() {
        const s = session()
        return {
            dev: ["/devstatic/DevAide.js", "/devstatic/DevAideGit.js", "/apps/dev", "/apps/dev/Workspace", "/apps/gallery"]
                .map((path) => ({ path, res: s.get(path) })),
            app: s.get("/apps/tools"),
            file: s.get("/js/MoquiLib.js"),
        }
    },
    then({ dev, app, file }) {
        // the controls: a base component's app sends the client to sign in, and its static file is served
        app.status == 302
        (app.location ?? "").includes("/Login")
        file.status == 200
        dev.every(({ res }) => res.status == 404)
    },
})

// Configuration in the matrix: behind a proxy the client's address is read from webapp-list/webapp[webroot]@client-ip-header,
// believed only from @trusted-proxies (the rightmost hop that is not one of them); by default no header is believed. The
// address decides an account's ipAllowed at sign-in, shown here with two fixture users: sec.ip.v4 may sign in only from
// 10.99.99.99, sec.ip.loop only from loopback. Every common forwarding header naming 10.99.99.99 neither lets the first in
// nor keeps the second out, and a refusal that names the address names the same one with the headers as without.
// Does not prove: a deployment behind its proxy (the opt-in proof below), nor the address written to the logs.
feature("v5.0.0-15.3.4: a client's forwarding headers do not change the address sign-in checks", {
    given() { needsFixtures("the users sec.ip.v4 and sec.ip.loop") },
    when() {
        const allowed = "10.99.99.99"
        return {
            allowed,
            plain: restLogin("sec.ip.v4", "SecIpV41!!"),
            named: restLogin("sec.ip.v4", "SecIpV41!!", forwarding(allowed)),
            loop: restLogin("sec.ip.loop", "SecIpLoop1!!", forwarding(allowed)),
        }
    },
    then({ allowed, plain, named, loop }) {
        plain.status == 401
        named.status == 401
        !loggedIn(named)
        // the control: the loopback user signs in although the headers name another address
        loggedIn(loop)
        namedAddress(named) == namedAddress(plain)
        namedAddress(named) != allowed
    },
})

// The deployment's proof of the same, through its proxy: a burst from this machine in which each request names another
// address in every common forwarding header is still limited as one client by web protection's per-client limit (its
// rate-limit "all", 50 a second with a burst of 500), which is keyed by the address the server takes. Loopback is exempt,
// so in the fixtures profile it skips; opt-in, as it gets this client's requests refused for a few seconds (20 clients,
// each stopping at its first 429). Run it with --param flood=yes, alone or with 2.4.1's.
// Does not prove: the address in the logs, nor a proxy that names the client in a header this list leaves out.
feature("v5.0.0-15.3.4: a burst giving each request a new address is still limited as one client", {
    given() {
        if (profile == "fixtures") skip("the project's test server takes its clients as loopback, which the per-client limits exempt: a deployment's proof")
        if (params.flood != "yes") skip("opt-in, as it gets this client's requests refused for a few seconds: --param flood=yes")
    },
    when() {
        return parallel(20, (index, args) => {
            let ok = 0
            for (let i = 0; i < args.max; i++) {
                const address = `198.18.${index}.${i % 250 + 1}`
                const res = http.get(args.path, {
                    headers: {
                        "X-Forwarded-For": address, "X-Real-IP": address, Forwarded: `for=${address}`, "CF-Connecting-IP": address,
                        "True-Client-IP": address, "X-Client-IP": address, "Fastly-Client-IP": address,
                        "X-Cluster-Client-IP": address, "CloudFront-Viewer-Address": `${address}:443`,
                    },
                })
                if (res.status == 429) return { ok, limited: true }
                if (res.status == 200) ok++
            }
            return { ok, limited: false }
        }, { path: params.floodPath ?? "/favicon.ico", max: Number(params.floodMax ?? 100) }, { timeout: 120 })
    },
    then({ results, errors }) {
        errors.length == 0
        // the control: the burst's first requests are answered
        results.reduce((sum, result) => sum + result.ok, 0) > 0
        results.some((result) => result.limited)
    },
})
