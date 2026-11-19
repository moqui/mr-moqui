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
