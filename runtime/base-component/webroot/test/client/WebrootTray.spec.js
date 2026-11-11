/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The tray's tabs undocked into floats (plans/Terminal-Fancy.md §10.6): m-shell-tray and the float layer mounted with Vue and
// Quasar, a stand-in for a tab's screen.
spec({ server: false, timeout: 60 })

browser.runInPage({
    styles: ["/libs/quasar/quasar.prod.css", "/css/WebrootVue.qvt.css"],
    scripts: ["/libs/vue/vue.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootFloats.js", "/js/WebrootVue.qvt.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootTray.page.js",
})
