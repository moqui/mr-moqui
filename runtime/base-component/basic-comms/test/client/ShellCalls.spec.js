// Calls in the desktop client's shell (crates/moqui-desktop/shell: Calls/Call and Calls/Dialer precompiled from this
// component's .qvue files, m-call and the ring bar from WebrootCall.js, shown by Shell/Root), on the runtime build of Vue as
// the shell runs them, with a stand-in host (window.moquiDesktop) and server: the routing of a call, a tel: link's Dialer and
// a room's call button to the shell, the ring bar and a ringing call's notification link, the encrypted call's SFrame worker
// by the path the shell's CSP allows, and "needs a connection" offline; then an app window, which offers an encrypted call
// in the app's Calls (its crypto device is the shell's) and keeps a plain one; and the web, as it was. No media: the call's
// join is stood in for (a real call is EncryptedCallFlow, with a server).
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/libs/vue/vue.runtime.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/libs/quasar/lang/en-US.umd.prod.js",
        "/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootChat.js", "/js/WebrootExt.js", "/js/WebrootCall.js", "/js/WebrootCall.render.js",
        "/js/WebrootCrypto.js",
        "/js/WebrootCallKeys.js", "/js/WebrootVue.qvt.render.js", "/js/WebrootVue.qvt.js", "/files/crates/moqui-desktop/shell/js/bundled-screens.js"],
    test: "/files/runtime/base-component/basic-comms/test/client/pages/ShellCalls.page.js",
})
