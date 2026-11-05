// Reliable sync (MD5) in the desktop client's shell: the bundled Chat, Inbox and Compose (precompiled from this component's
// .qvue files) with a stand-in host (window.moquiDesktop) whose outbox the test moves through the states the real one
// takes (moqui_client::sync::flush), and a stand-in server and /notws: a lost answer then a retry and a push that comes twice
// show one message; a reconnect after a feed reset reads the room as the store has it; a message's edit, reaction and
// deletion go through the outbox where the server takes them once, and a refused one shows why with Retry and Discard; a
// thread change and a send that conflicted say so, the send's copy opened in Compose; an internal message waits in the
// outbox where the server takes its send once; a revoked key holds what waits, and one held item stops no other.
spec({ server: false, timeout: 60 })

browser.runInPage({
    styles: ["/libs/quasar/quasar.prod.css", "/css/WebrootVue.qvt.css"],
    scripts: ["/libs/vue/vue.runtime.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/libs/quasar/lang/en-US.umd.prod.js",
        "/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootChat.js", "/js/WebrootExt.js", "/js/WebrootCrypto.js", "/js/WebrootVue.qvt.render.js", "/js/WebrootVue.qvt.js",
        "/files/crates/moqui-desktop/shell/js/bundled-screens.js"],
    test: "/files/runtime/base-component/basic-comms/test/client/pages/ShellSync.page.js",
})
