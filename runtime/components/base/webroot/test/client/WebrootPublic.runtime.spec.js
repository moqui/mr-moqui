// A public island on the runtime build of Vue (no template compiler), in the page (SEC-PSF-04): it loads a bundled .qvue
// with a script element and mounts it, with no eval and no new Function. The page has no #moqui-public-config and no
// island config when the libraries load, so WebrootPublic.qvt.js does not start by itself.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/libs/vue/vue.runtime.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/js/MoquiLib.js", "/js/MoquiL10n.js",
        "/js/WebrootVue.qvt.render.js", "/js/WebrootVue.qvt.js", "/js/WebrootPublic.qvt.js"],
    test: "/files/component/webroot/test/client/pages/WebrootPublic.runtime.page.js",
})
