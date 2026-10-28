// The Design view of a .qvue screen (DevAideQvue.js): its template's elements and their edits, in the page.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/files/runtime/dev-component/dev-aide/screen/devstatic/DevAide.js",
        "/files/runtime/dev-component/dev-aide/screen/devstatic/DevAideQvue.js"],
    test: "/files/runtime/dev-component/dev-aide/test/client/pages/DevAideQvue.page.js",
})
