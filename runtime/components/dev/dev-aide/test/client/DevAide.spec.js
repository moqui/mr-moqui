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

// Every component template of the Dev AIDE's scripts compiles, its expressions too.

browser.runInPage({
    scripts: ["/files/component/dev-aide/test/client/pages/DevAideTemplates.setup.js",
        "/libs/vue/vue.global.prod.js", "/libs/vue/compiler-dom.cjs.js",
        "/files/component/dev-aide/screen/devstatic/DevAide.js",
        "/files/component/dev-aide/screen/devstatic/DevAideBlocks.js",
        "/files/component/dev-aide/screen/devstatic/DevAideGit.js",
        "/files/component/dev-aide/screen/devstatic/DevAideBrowser.js",
        "/files/component/dev-aide/screen/devstatic/DevAideService.js",
        "/files/component/dev-aide/screen/devstatic/DevAideEntity.js",
        "/files/component/dev-aide/screen/devstatic/DevAideScreen.js",
        "/files/component/dev-aide/screen/devstatic/DevAideQvue.js"],
    test: "/files/component/dev-aide/test/client/pages/DevAideTemplates.page.js",
})
