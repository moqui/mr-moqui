// OWASP ASVS 5.0.0 V12 Secure Communication: the framework's proofs, each feature filed under the requirement its name
// starts with (docs/security/proofs/V12-Secure-Communication.md).
//
// These requirements are Configuration in the matrix: the deployment meets them with an HTTPS listener (a certificate
// in runtime/cert, or ACME) or a TLS proxy, webapp-list/webapp[webroot]@https-enabled, and no plain-http address
// published. So these proofs check a deployment served over https (--server https://...) and skip against any other,
// the project's --serve server included. The TLS versions (12.1.1) are proved in-process (tls_tests.rs).

// The deployment's plain-http address: --param httpUrl=http://host[:port], else the server's host on port 80.
const httpUrl = () => params.httpUrl ?? `http://${server.host.includes(":") ? `[${server.host}]` : server.host}`
const httpsDeployment = () => {
    if (!server.secure) skip("a deployment's proof over https: run it with --server https://<host>")
}

// Does not prove: the other listeners of the server (SMTP, SSH, XMPP), nor a proxy's plain-http port other than the one
// named.
feature("v5.0.0-12.2.1: the plain-http address serves nothing but a redirect to https", {
    given() { httpsDeployment() },
    when() {
        let res = null
        let refused = null
        try {
            res = session().get(httpUrl() + "/Login")
        } catch (e) {
            refused = String(e && e.message ? e.message : e)
        }
        return { res, refused }
    },
    then({ res, refused }) {
        // nothing listens there, or it sends the browser to https
        refused != null || ([301, 302, 307, 308].includes(res.status) && (res.location ?? "").startsWith("https://"))
    },
})

// Does not prove: that the roots this client trusts are the ones a certifier means; it uses this machine's and the
// client's own, and skips with --insecure, which checks no certificate.
feature("v5.0.0-12.2.2: the deployment's certificate is one a public root vouches for", {
    given() {
        httpsDeployment()
        if (server.insecure) skip("--insecure accepts any certificate: run without it")
    },
    when() { return { res: session().get("/Login") } },
    then({ res }) {
        // the handshake checked the certificate, its chain and its name before this answer came
        res.status == 200
    },
})
