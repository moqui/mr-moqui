// The suite shell, in the page. Was js/WebrootVue.qvt.shell.test.js.
// WebrootVue.qvt.js starts the Vue app only when the page has #moqui-config plus Vue and Quasar. This page has none of those.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootVue.qvt.js"],
    test: "/files/component/webroot/test/client/pages/WebrootVue.qvt.shell.page.js",
})

// The shared palette with the Webroot shell bindings and the real input, dialog and focus stack.
browser.runInPage({
    styles: ["/libs/quasar/quasar.prod.css"],
    scripts: ["/libs/vue/vue.runtime.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/js/MoquiLib.js", "/js/MoquiL10n.js",
        "/js/WebrootVue.qvt.render.js", "/js/WebrootVue.qvt.js"],
    test: "/files/component/webroot/test/client/pages/WebrootPalette.page.js",
})
