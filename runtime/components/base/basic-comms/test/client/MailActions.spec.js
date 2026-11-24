// Mailbox state actions in the bundled Inbox with the server's success, partial failure and refusal responses.
spec({ server: false })

browser.runInPage({
    styles: ["/libs/quasar/quasar.prod.css"],
    scripts: ["/libs/vue/vue.runtime.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/libs/quasar/lang/en-US.umd.prod.js",
        "/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootExt.js", "/js/WebrootVue.qvt.render.js", "/js/WebrootVue.qvt.js",
        "/files/crates/moqui-desktop/shell/js/bundled-screens.js"],
    test: "/files/component/basic-comms/test/client/pages/MailActions.page.js",
})
