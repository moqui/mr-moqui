/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The emoji picker (js/WebrootEmoji.js, plans/Comms-Messaging.md §4), in the page with Vue and Quasar and the vendored emoji
// data (libs/emoji/emoji.json): search by name, the groups as tabs, and the user's recent emoji kept in this browser.
spec({ server: false, timeout: 60 })

browser.runInPage({
    styles: ["/libs/quasar/quasar.prod.css", "/css/WebrootVue.qvt.css"],
    scripts: ["/libs/vue/vue.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/js/MoquiLib.js", "/js/WebrootEmoji.js"],
    test: "/files/component/webroot/test/client/pages/WebrootEmoji.page.js",
})
