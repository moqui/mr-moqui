// qvt.start, in the page, with stand-ins for Vue and Quasar: what is set before the mount, which waits for the language catalog.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootVue.qvt.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.start.page.js",
})
