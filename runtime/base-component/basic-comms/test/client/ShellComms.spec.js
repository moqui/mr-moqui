// The Comms screens bundled with the desktop client's shell (crates/moqui-desktop/shell: Chat, the Inbox and Compose,
// precompiled from this component's .qvue files, shown by Shell/Root), on the runtime build of Vue as the shell runs them, with
// a stand-in host (window.moquiDesktop) and server: the shell's routing, the offline and pending presentation, the sign-out's
// unsent list; then the same screens with no host, as on the web, which show none of it; and an encrypted room in an app
// window, whose crypto device is the shell's.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/libs/vue/vue.runtime.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/libs/quasar/lang/en-US.umd.prod.js",
        "/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootChat.js", "/js/WebrootExt.js", "/js/WebrootCrypto.js", "/js/WebrootVue.qvt.render.js", "/js/WebrootVue.qvt.js",
        "/files/crates/moqui-desktop/shell/js/bundled-screens.js"],
    test: "/files/runtime/base-component/basic-comms/test/client/pages/ShellComms.page.js",
})
