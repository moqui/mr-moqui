// The screen view's and the block tree's pure parts, in the page.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootVue.qvt.js", "/files/component/dev-aide/screen/devstatic/DevAide.js",
        "/files/component/dev-aide/screen/devstatic/DevAideBlocks.js",
        "/files/component/dev-aide/screen/devstatic/DevAideScreen.js"],
    test: "/files/component/dev-aide/test/client/pages/DevAideScreen.page.js",
})

// From DevAideQvue.spec.js, merged here to share a spec run.
// The Design view of a .qvue screen (DevAideQvue.js): its template's elements and their edits, in the page.

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootVue.qvt.js", "/files/component/dev-aide/screen/devstatic/DevAide.js",
        "/files/component/dev-aide/screen/devstatic/DevAideQvue.js"],
    test: "/files/component/dev-aide/test/client/pages/DevAideQvue.page.js",
})
