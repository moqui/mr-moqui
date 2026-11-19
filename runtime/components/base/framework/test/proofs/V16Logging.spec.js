// OWASP ASVS 5.0.0 V16 Security Logging and Error Handling: the framework's proofs, each feature filed under the
// requirement its name starts with (docs/security/proofs/V16-Security-Logging-and-Error-Handling.md). Ported from Moqui
// Java's pytest proofs; a comment names the Python test, as in test/client.
//
// A real internal failure cannot be caused from outside on purpose; these read the refusals and the error page a client
// can reach. What a real failure answers (an error id in production) is proved in-process.

const PAYLOAD = "<script>alert(1)</script>"
const ESCAPED = "&lt;script&gt;alert(1)&lt;/script&gt;"
// No script, stack, panic or source location in an error's text.
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

// test_a10_errors.py: test_rest_unauth_json_does_not_include_stack_frames
// Does not prove: what a real internal failure answers.
feature("v5.0.0-16.5.1: a REST refusal in JSON gives a message, no stack or source location", {
    when() { return { res: session().get("/rest/e1/moqui.basic.Enumeration", { accept: "json" }) } },
    then({ res }) {
        res.status == 401
        res.contentType.includes("json")
        typeof res.json().errors == "string"
        !res.text.includes("at org.moqui")
        !res.text.includes("Exception") || !res.text.toLowerCase().includes("stack")
        // nor a Rust location or backtrace
        !/\.rs:\d+/.test(res.text)
        !res.text.includes("stack backtrace")
    },
})

// test_a10_errors.py: test_rest_unauth_html_error_has_no_script_or_stack
// Does not prove: what a real internal failure answers.
feature("v5.0.0-16.5.1: a REST refusal asked for as HTML has no stack frame or script", {
    when() { return { res: session().get("/rest/e1/moqui.basic.Enumeration", { accept: "html" }) } },
    then({ res }) {
        res.status == 401
        noScriptOrStack(res.text)
    },
})

// test_a10_errors.py: test_internal_error_page_does_not_include_stack_trace_when_hit_directly
// Does not prove: the page after a real failure; it is rendered here with a message given in the request.
feature("v5.0.0-16.5.1: the internal error page shows no stack trace and escapes its message", {
    when() {
        return { res: session().get("/error/InternalError", { params: { errorMessage: PAYLOAD }, accept: "html" }) }
    },
    then({ res }) {
        res.text.includes(ESCAPED)
        !res.text.toLowerCase().includes("groovy:") || !res.text.includes("at ")
        noScriptOrStack(res.text)
    },
})
