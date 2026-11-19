// A02 Security Misconfiguration over HTTP, ported from Moqui Java's pytest proofs (framework/test/test_a02_headers.py;
// the catalog in README.md): the headers of an HTML page, CORS for a request with no Origin, and no H2 console. Passes
// on any conf. The session cookie's attributes, the caching of the login page and the CORS refusals are proofs now
// (test/proofs/V3WebFrontend.spec.js, V14DataProtection.spec.js).

// test_a02_headers.py: test_html_response_has_csp_and_frame_options
feature("A02 an HTML page has a CSP with frame-ancestors and form-action, X-Frame-Options and nosniff", {
    when() { return { res: session().get("/Login") } },
    then({ res }) {
        res.status == 200
        res.header("Content-Security-Policy").toLowerCase().includes("frame-ancestors")
        res.header("Content-Security-Policy").toLowerCase().includes("form-action")
        !!res.header("X-Frame-Options")
        res.header("X-Content-Type-Options").toLowerCase().includes("nosniff")
    },
})

// test_a02_headers.py: test_cors_no_origin_header_has_no_allow_origin
feature("A02 a request with no Origin gets no Access-Control-Allow-Origin", {
    when() { return { res: session().get("/Login") } },
    then({ res }) {
        res.status == 200
        res.header("Access-Control-Allow-Origin") == null
    },
})

// test_a02_headers.py: test_h2_console_is_not_mounted. There is no H2 console here; the proof that nothing answers
// at its path is kept.
feature("A02 no H2 console is mounted", {
    when() { return { res: session().get("/h2/") } },
    then({ res }) {
        res.status == 404
        !res.text.toLowerCase().includes("h2 console")
        !res.text.toLowerCase().includes("login.jsp")
        !(res.location ?? "").includes("/h2")
    },
})

// From SecurityErrors.spec.js and SecurityMisc.spec.js, merged here to share a spec run.
// A01 and A10 proofs ported from Moqui Java's pytest (framework/test/test_a01_more.py, test_a10_errors.py; the catalog
// in README.md): the unauthorized page's escaping, and an executable upload. Passes on any conf. The demo user is
// john.doe. The redirect, injection and error-answer proofs are in test/proofs now (V3WebFrontend, V1Encoding,
// V16Logging).

const USER = params.user ?? "john.doe"
const PASSWORD = params.password ?? "moqui"
// Spock's payload (SecurityErrorTests): the page must escape it, and Rust will not emit a Java stack class.
const PAYLOAD = "<script>alert(1)</script>"
const ESCAPED = "&lt;script&gt;alert(1)&lt;/script&gt;"

// No inline script, and no Rust panic or stack frame. Same stack rule as SecurityErrors' JSON proof: "Exception"
// alone is not a frame. A Java class name is not required. assert, because a call outside then is not rewritten.
const noScriptOrStack = (text) => {
    assert(!text.toLowerCase().includes("<script>"), "inline script tag")
    assert(!text.includes(PAYLOAD), "reflected script tag")
    assert(!/\.rs:\d+/.test(text), "Rust source location")
    assert(!text.includes("stack backtrace"), "stack backtrace")
    assert(!text.toLowerCase().includes("panic"), "panic")
    assert(!text.toLowerCase().includes("stack"), "stack")
    assert(!text.includes("Exception") || !text.toLowerCase().includes("stack"), "exception stack")
    return true
}

// test_a10_errors.py: test_unauthorized_error_page_html_has_no_script_tag
feature("A10 the unauthorized page escapes a message and has no script tag", {
    when() {
        return { res: session().get("/error/Unauthorized", { params: { errorMessage: PAYLOAD }, accept: "html" }) }
    },
    then({ res }) {
        res.text.includes(ESCAPED)
        noScriptOrStack(res.text)
    },
})

// test_a01_more.py: test_multipart_executable_upload_is_rejected
// The check is on every upload (WebUtilities.isExecutable). This transition is the Python path. An MZ program and a
// #! script are rejected; a ZIP (PK) is not a program. magic is the response's first four bytes, not the upload.
feature("A01 an uploaded #kind is #verdict", {
    where: [
        { kind: "MZ program", verdict: "rejected", fileName: "evil.exe", contentType: "application/octet-stream",
            base64: "TVoAAFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhY", magic: "4d5a0000" },
        { kind: "shell script", verdict: "rejected", fileName: "run.sh", contentType: "text/x-sh",
            text: "#!/bin/sh\necho hi\n", magic: "#!/b" },
        { kind: "ZIP", verdict: "accepted", fileName: "ok.zip", contentType: "application/zip",
            base64: "UEsDBFhYWFhYWFhYWFhYWFhYWFg=", magic: "504b0304" },
    ],
    when({ fileName, contentType, base64, text }) {
        const s = session()
        s.login(USER, PASSWORD)
        const part = { fileName, contentType }
        if (base64 != null) part.base64 = base64
        if (text != null) part.text = text
        return {
            res: s.post("/apps/tools/Data/Snapshots/uploadSnapshot", { multipart: { snapshotFile: part } }),
        }
    },
    then({ res, verdict, fileName, magic }) {
        const rejected = verdict == "rejected"
        rejected == (res.status == 415)
        !rejected || res.text.toLowerCase().includes("executable")
        !rejected || res.text.includes(fileName)
        rejected || res.status == 302
        res.magic != magic
    },
})
