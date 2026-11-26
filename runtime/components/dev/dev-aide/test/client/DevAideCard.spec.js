// The change set card of propose_edit from a patch, in the page: m-assist-change-set (webroot js/WebrootAssist.js) on
// the card the server makes (test/vectors/change-card-vectors.json, which src/agent/propose_card_tests.rs holds to
// change_card), with Vue and Quasar.
spec({ server: false, timeout: 60 })

browser.runInPage({
    styles: ["/libs/quasar/quasar.prod.css"],
    scripts: ["/libs/vue/vue.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/js/MoquiLib.js", "/js/MoquiL10n.js",
        "/js/WebrootAssist.js"],
    test: "/files/component/dev-aide/test/client/pages/DevAideCard.page.js",
})
