// The Workspace's git depth (DevAideGit.js), its pure parts in the page.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/files/runtime/dev-component/dev-aide/screen/devstatic/DevAide.js",
        "/files/runtime/dev-component/dev-aide/screen/devstatic/DevAideGit.js"],
    test: "/files/runtime/dev-component/dev-aide/test/client/pages/DevAideGit.page.js",
})
