// Assist, in the page. Was js/WebrootAssist.test.js.
// Assist loads before WebrootVue.qvt.js so qvt.registerLate still sees components registered later.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootChat.js", "/js/WebrootAssist.js", "/libs/vue/compiler-dom.cjs.js", "/js/WebrootVue.qvt.js", "/libs/marked/marked.umd.min.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootAssist.page.js",
})

// Assist undocked (plans/Terminal-Fancy.md §10.5, §10.7): the drawer and the float layer mounted with Vue and Quasar.
browser.runInPage({
    styles: ["/libs/quasar/quasar.prod.css", "/css/WebrootVue.qvt.css"],
    scripts: ["/libs/vue/vue.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootFloats.js", "/js/WebrootAssist.js",
        "/js/WebrootVue.qvt.js", "/libs/marked/marked.umd.min.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootAssist.float.page.js",
})

// From WebrootAssistCanvas.spec.js, merged here to share a spec run.
// The Assist canvas, in the page. Was js/WebrootAssistCanvas.test.js.

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootAssist.js", "/libs/openui/lang-core.min.js",
        "/files/runtime/base-component/webroot/test/client/pages/WebrootAssistCanvas-module.js", "/js/WebrootAssistCanvas.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootAssistCanvas.page.js",
})

// The Constitution's reading view and reference dialog (js/WebrootConstitution.js): its logic, without Vue.

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootAssist.js", "/js/WebrootConstitution.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootConstitution.page.js",
})
