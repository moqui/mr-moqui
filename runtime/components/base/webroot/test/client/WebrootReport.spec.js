/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
spec({ server: false, timeout: 60 })
browser.runInPage({
    scripts: ["/libs/vue/vue.runtime.global.prod.js", "/js/MoquiLib.js", "/libs/chartjs/chart.umd.min.js", "/js/WebrootReport.js"],
    test: "/files/component/webroot/test/client/pages/WebrootReport.page.js",
})
