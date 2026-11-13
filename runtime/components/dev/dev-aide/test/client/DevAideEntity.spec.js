// The entity view's operation builders, in the page, against the vectors the dev-aide's Rust test applies.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/files/component/dev-aide/screen/devstatic/DevAide.js",
        "/files/component/dev-aide/screen/devstatic/DevAideEntity.js"],
    test: "/files/component/dev-aide/test/client/pages/DevAideEntity.page.js",
})
