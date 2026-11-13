// The chat store, in the page. Was js/WebrootChat.test.js.
// WebrootVue.qvt.js starts the Vue app only when the page has #moqui-config plus Vue and Quasar. This page has none of those.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootChat.js", "/libs/vue/compiler-dom.cjs.js", "/js/WebrootVue.qvt.js"],
    test: "/files/component/webroot/test/client/pages/WebrootChat.page.js",
})
