// A build with a start screen (client-conf start-path) in the desktop client's shell: the bundled shell with a stand-in host
// (window.moquiDesktop) whose state has startPath. The start screen is the home: no list of apps and no chat panel; Back
// goes back towards it and at it answers that it did nothing (so Android's back leaves the app); a link the host opens at a
// cold start has the start screen under it; a start screen the shell does not bundle opens as the server's page. Chat's
// single-room mode as the start screen: the first room alone, in bubbles, with no rail or chrome, and every Chat link keeps it.
spec({ server: false, timeout: 60 })

browser.runInPage({
    styles: ["/libs/quasar/quasar.prod.css", "/css/WebrootVue.qvt.css"],
    scripts: ["/libs/vue/vue.runtime.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/libs/quasar/lang/en-US.umd.prod.js",
        "/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootChat.js", "/js/WebrootChatExpiry.js", "/js/WebrootExt.js", "/js/WebrootEmoji.js", "/js/WebrootCrypto.js", "/js/WebrootVue.qvt.render.js", "/js/WebrootVue.qvt.js",
        "/files/crates/moqui-desktop/shell/js/bundled-screens.js"],
    test: "/files/component/basic-comms/test/client/pages/ShellStart.page.js",
})
