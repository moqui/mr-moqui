// The screen view's and the block tree's pure parts, in the page.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/files/runtime/dev-component/dev-aide/screen/devstatic/DevAide.js",
        "/files/runtime/dev-component/dev-aide/screen/devstatic/DevAideBlocks.js",
        "/files/runtime/dev-component/dev-aide/screen/devstatic/DevAideScreen.js"],
    test: "/files/runtime/dev-component/dev-aide/test/client/pages/DevAideScreen.page.js",
})
