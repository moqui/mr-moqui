// Was js/WebrootVue.qvt.views.test.js.
// WebrootVue.qvt.js starts the Vue app only when the page has #moqui-config plus Vue and Quasar. This page has none of those.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootVue.qvt.js"],
    test: "/files/component/webroot/test/client/pages/WebrootVue.qvt.views.page.js",
})

// From WebrootVue.qvt.htmlview.spec.js, merged here to share a spec run.
// Was js/WebrootVue.qvt.htmlview.test.js.
// WebrootVue.qvt.js starts the Vue app only when the page has #moqui-config plus Vue and Quasar. This page has none of those.

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootVue.qvt.js"],
    test: "/files/component/webroot/test/client/pages/WebrootVue.qvt.htmlview.page.js",
})
