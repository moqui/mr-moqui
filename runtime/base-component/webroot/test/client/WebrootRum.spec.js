/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// Real-user monitoring (js/WebrootRum.js): its OTLP builders, sampling and masking, and, with RUM on, a request's
// traceparent and the span it posts.
spec({ server: false, timeout: 60 })
browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootRum.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootRum.page.js",
})
