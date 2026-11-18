/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The PWA's service worker (sw.js): what it keeps in its cache (SEC-CLI-02).
spec({ server: false, timeout: 60 })
browser.runInPage({
    scripts: ["/files/component/webroot/screen/webroot/sw.js"],
    test: "/files/component/webroot/test/client/pages/WebrootServiceWorker.page.js",
})
