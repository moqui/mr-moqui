// The Artifact Browser's pure parts (DevAideBrowser.js), in the page: the layout, the connectors and the sections.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/files/runtime/dev-component/dev-aide/screen/devstatic/DevAide.js",
        "/files/runtime/dev-component/dev-aide/screen/devstatic/DevAideBrowser.js"],
    test: "/files/runtime/dev-component/dev-aide/test/client/pages/DevAideBrowser.page.js",
})
