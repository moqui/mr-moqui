// The suite shell, in the page. Was js/WebrootVue.qvt.shell.test.js.
// WebrootVue.qvt.js starts the Vue app only when the page has #moqui-config plus Vue and Quasar. This page has none of those.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootVue.qvt.js"],
    test: "/files/component/webroot/test/client/pages/WebrootVue.qvt.shell.page.js",
})
