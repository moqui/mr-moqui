// The Artifact Browser in the terminal (plans/design-dev-aide.md 7.3, D14): the BrowseTui twin with the
// artifact-browser component and the find below it, and the browse transition's answer, the references grouped and
// filtered on the server (get#ArtifactBrowse). The component's keys are the moqui-tui crate's tests
// (src/components/artifact_browser_tests.rs, tests/artifact_browser.rs); the grouping's cases are the dev-aide crate's
// (src/browser_tests.rs).
// Run: ./moq test dev-aide --spec ArtifactBrowser
const dev = () => ec.screen.makeTest().rootScreen("component://dev-aide/screen/DevAide.xml")
const nodeById = (node, id) => {
    if (!node || typeof node !== "object") return null
    if (node.id === id) return node
    for (const value of Object.values(node)) {
        const found = nodeById(value, id)
        if (found) return found
    }
    return null
}
const browse = (params) => JSON.parse(String(dev().render("Artifacts/BrowseTui/browse", params, null).output))

spec({
    setupSpec() {
        login("john.doe")
    }
})

feature("the terminal twin is the browser component on the artifact, with a find whose results open in the browser", {
    expect() {
        const terminal = dev().renderMode("tui")
            .render("Artifacts/BrowseTui", {artifactId: "entity:moqui.example.Example", text: "Example"}, null)
        const out = String(terminal.output)
        terminal.status == 200
        terminal.errorMessages.length == 0
        out.includes('"name":"artifact-browser"')
        out.includes('"DevArtifactBrowser"')
        out.includes('"artifactId":"entity:moqui.example.Example"')
        out.includes('"BrowseFind"')
        // the find goes to the screen, not a transition: the terminal opens it with the fields, no post
        nodeById(JSON.parse(out), "BrowseFind").method == "get"
        out.includes('"BrowseResults"')
        out.includes("Browse?artifactId=entity%3Amoqui.example.Example")
    }
})

feature("browse answers the center with its links and both sides in sections by kind, filtered", {
    expect() {
        const answer = browse({artifactId: "entity:moqui.example.Example"})
        const sides = answer.usedBy.concat(answer.uses)
        const keys = answer.center.links.map((link) => link.key).join(",")
        answer.center.id == "entity:moqui.example.Example"
        answer.center.lines[0] == "moqui.example.Example"
        keys == "t,d,r,D"
        answer.center.links[0].url.startsWith("/qapps/dev/Workspace?rootId=")
        answer.usedBy.some((section) => section.kind == "screen")
        sides.every((section) => section.count == section.rows.length && section.rows.every((row) => row.kind == section.kind))
        answer.kinds.includes("screen")
        // not a framework artifact: the framework's own users are hidden until asked for
        answer.filter.hideFramework == true
        // one kind, the framework shown
        const screens = browse({artifactId: "entity:moqui.example.Example", kinds: "screen", hideFramework: "N"})
        screens.usedBy.length == 1 && screens.usedBy[0].kind == "screen"
        screens.uses.length == 0
        screens.filter.hideFramework == false
        // with the framework shown the entity uses its enumerations
        browse({artifactId: "entity:moqui.example.Example", hideFramework: "N"}).uses.length > 0
        // an artifact not in the index
        const missing = browse({artifactId: "entity:no.such.Entity"})
        missing.center == null
        missing.message.includes("No artifact entity:no.such.Entity")
    }
})
