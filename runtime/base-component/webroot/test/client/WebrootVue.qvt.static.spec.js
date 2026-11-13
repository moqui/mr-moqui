// Server-static qvt, in the page. Was js/WebrootVue.qvt.static.test.js.
// Vue is loaded and Quasar is not, and this page has no #moqui-config, so qvt.start() does not run.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/libs/vue/vue.global.prod.js", "/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootVue.qvt.js"],
    test: "/files/component/webroot/test/client/pages/WebrootVue.qvt.static.page.js",
})
