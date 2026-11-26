// The Workspace client's pure parts, in the page. Was screen/devstatic/DevAide.test.js.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/files/component/dev-aide/screen/devstatic/DevAide.js"],
    test: "/files/component/dev-aide/test/client/pages/DevAide.page.js",
})

// From DevAideBrowser.spec.js, merged here to share a spec run.
// The Artifact Browser's pure parts (DevAideBrowser.js), in the page: the layout, the connectors and the sections.

browser.runInPage({
    scripts: ["/files/component/dev-aide/screen/devstatic/DevAide.js",
        "/files/component/dev-aide/screen/devstatic/DevAideBrowser.js"],
    test: "/files/component/dev-aide/test/client/pages/DevAideBrowser.page.js",
})

// From DevAideGit.spec.js, merged here to share a spec run.
// The Workspace's git depth (DevAideGit.js), its pure parts in the page.

browser.runInPage({
    scripts: ["/files/component/dev-aide/screen/devstatic/DevAide.js",
        "/files/component/dev-aide/screen/devstatic/DevAideGit.js"],
    test: "/files/component/dev-aide/test/client/pages/DevAideGit.page.js",
})

// Co-editing a file (DevAideCoedit.js, F15): the pure parts, and two tabs on one fake room bound to m-code-editor's
// y-codemirror.next extensions, with moqui.coedit.Provider's protocol.

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootVue.qvt.js", "/libs/codemirror/codemirror.min.js", "/js/WebrootCode.js",
        "/libs/yjs/yjs.min.js", "/libs/codemirror/y-codemirror.min.js", "/files/component/dev-aide/screen/devstatic/DevAideCoedit.js"],
    test: "/files/component/dev-aide/test/client/pages/DevAideCoedit.page.js",
})

// The TypeScript checker (DevAideTypes.js, 5.4): its pure parts, and its worker on the vendored compiler.

browser.runInPage({
    scripts: ["/files/component/dev-aide/screen/devstatic/DevAideTypes.js"],
    test: "/files/component/dev-aide/test/client/pages/DevAideTypes.page.js",
})
