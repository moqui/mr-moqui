// The public shell, in the page. Was js/WebrootPublic.qvt.test.js.
// WebrootPublic.qvt.js starts only when #moqui-public-config, Vue and Quasar are all present. This page has none of those.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootVue.qvt.js", "/js/WebrootPublic.qvt.js"],
    test: "/files/component/webroot/test/client/pages/WebrootPublic.qvt.page.js",
})
