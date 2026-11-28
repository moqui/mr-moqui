/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The pure helpers of the CalcGrid sheet view (js/WebrootCalc.js, plans/archive/IronCalc-DB.md), and the Tabulator 6.7
// internals it takes over (plans/IronCalc-DB2.md CG2), in a page with no server.
// Run: moqui test-client webroot --offline --spec WebrootCalc
spec({ server: false, timeout: 60 })
browser.runInPage({
    styles: ["/libs/tabulator/tabulator.min.css", "/css/DataGrid.css"],
    scripts: ["/libs/vue/vue.global.prod.js", "/libs/tabulator/tabulator.min.js", "/js/MoquiLib.js", "/js/WebrootCalc.js"],
    test: "/files/component/webroot/test/client/pages/WebrootCalc.page.js",
})
