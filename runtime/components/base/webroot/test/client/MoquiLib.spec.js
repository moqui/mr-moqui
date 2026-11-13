// MoquiLib's own functions, in the page. Was js/MoquiLib.test.js.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js"],
    test: "/files/component/webroot/test/client/pages/MoquiLib.page.js",
})
