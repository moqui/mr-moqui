// The service writer's and the logic composer's pure parts, in the page.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/files/component/dev-aide/screen/devstatic/DevAide.js",
        "/files/component/dev-aide/screen/devstatic/DevAideBlocks.js",
        "/files/component/dev-aide/screen/devstatic/DevAideService.js"],
    test: "/files/component/dev-aide/test/client/pages/DevAideService.page.js",
})
