/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Floats (js/WebrootFloats.js, plans/Terminal-Fancy.md §10), in the page with Vue and Quasar: the store, the layer and a window
// with a kind of test content teleported into it. No terminal: that kind is the Terminal's own.
spec({ server: false, timeout: 60 })

browser.runInPage({
    styles: ["/libs/quasar/quasar.prod.css", "/css/WebrootVue.qvt.css"],
    scripts: ["/libs/vue/vue.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/js/MoquiLib.js", "/js/WebrootFloats.js", "/libs/vue/compiler-dom.cjs.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootFloats.page.js",
})
