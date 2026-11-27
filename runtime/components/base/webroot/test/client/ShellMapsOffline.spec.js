/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The apps' offline Maps screen (plans/Maps-Offline.md §2.5; crates/moqui-desktop/shell/screens/MapsOffline.qvue, bundled
// and precompiled into the shell's bundled-screens.js) with a stand-in host (window.moquiDesktop: its packages and outbox):
// the server's and the device's packages in one list, with Download (only on Wi-Fi on the phone), Update, Cancel and
// Delete; offline, the device's packages, opened from their manifest at the package path, the screen saying it is offline
// and when the package was built, its sources as text; a search of the package's places, postal codes, Overture places
// and (through the host) addresses within its region; a place saved as an outbox item with its clientRef; and the shell's
// Maps entry, Browse when the server answers and the offline screen when it does not.
spec({ server: false, timeout: 60 })

browser.runInPage({
    styles: ["/libs/quasar/quasar.prod.css", "/css/WebrootVue.qvt.css"],
    scripts: ["/libs/vue/vue.runtime.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/libs/quasar/lang/en-US.umd.prod.js",
        "/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootChat.js", "/js/WebrootChatExpiry.js", "/js/WebrootExt.js", "/js/WebrootEmoji.js",
        "/js/WebrootCrypto.js", "/js/WebrootVue.qvt.render.js", "/js/WebrootVue.qvt.js", "/files/crates/moqui-desktop/shell/js/bundled-screens.js"],
    test: "/files/component/webroot/test/client/pages/ShellMapsOffline.page.js",
})
