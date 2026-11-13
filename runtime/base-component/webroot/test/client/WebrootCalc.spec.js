/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The pure helpers of the CalcGrid sheet view (js/WebrootCalc.js, plans/IronCalc-DB.md), in a page with no server.
// Run: moqui test-client webroot --offline --spec WebrootCalc
spec({ server: false, timeout: 60 })
browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootCalc.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootCalc.page.js",
})
