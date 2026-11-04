// The Workspace client's pure parts, in the page. Was screen/devstatic/DevAide.test.js.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/files/runtime/dev-component/dev-aide/screen/devstatic/DevAide.js"],
    test: "/files/runtime/dev-component/dev-aide/test/client/pages/DevAide.page.js",
})

// From DevAideBrowser.spec.js, merged here to share a spec run.
// The Artifact Browser's pure parts (DevAideBrowser.js), in the page: the layout, the connectors and the sections.

browser.runInPage({
    scripts: ["/files/runtime/dev-component/dev-aide/screen/devstatic/DevAide.js",
        "/files/runtime/dev-component/dev-aide/screen/devstatic/DevAideBrowser.js"],
    test: "/files/runtime/dev-component/dev-aide/test/client/pages/DevAideBrowser.page.js",
})

// From DevAideGit.spec.js, merged here to share a spec run.
// The Workspace's git depth (DevAideGit.js), its pure parts in the page.

browser.runInPage({
    scripts: ["/files/runtime/dev-component/dev-aide/screen/devstatic/DevAide.js",
        "/files/runtime/dev-component/dev-aide/screen/devstatic/DevAideGit.js"],
    test: "/files/runtime/dev-component/dev-aide/test/client/pages/DevAideGit.page.js",
})
