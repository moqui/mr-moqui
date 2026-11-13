// MoquiL10n's formatter, in the page. Was js/MoquiL10n.test.js.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiL10n.js"],
    test: "/files/component/webroot/test/client/pages/MoquiL10n.page.js",
})
