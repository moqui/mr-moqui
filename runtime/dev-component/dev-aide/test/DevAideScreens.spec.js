// The Dev AIDE screens on a dev instance (plans/design-dev-aide.md P2): Home and Settings render for the full tier in the
// browser's mode and the terminal's (their forms and lists are plain, so `moqui tui` reaches them), and the Workspace
// answers its roots as JSON. The services' own behaviour (saves, the upstream lock, git, Publish) is in the host's
// integration test (crates/moqui-host/tests/it/devaide.rs), which makes scratch repositories.
// Run: ./moq test dev-aide
const dev = () => ec.screen.makeTest().rootScreen("component://dev-aide/screen/DevAide.xml")

spec({
    setupSpec() { login("john.doe") },
})

feature("Home renders the roots and the reload log", {
    expect() {
        const page = dev().render("Home", null, null)
        page.status == 200
        page.errorMessages.length == 0
        page.output.includes("RootList")
        page.output.includes("Reload log")
    },
})

feature("Settings renders the site components and the roots", {
    expect() {
        const page = dev().render("Settings", null, null)
        page.status == 200
        page.errorMessages.length == 0
        page.output.includes("SiteComponentsForm")
        page.output.includes("RootSettings")
    },
})

feature("Home and Settings render in the terminal", {
    expect() {
        const home = dev().renderMode("tui").render("Home", null, null)
        const settings = dev().renderMode("tui").render("Settings", null, null)
        home.status == 200 && home.errorMessages.length == 0
        settings.status == 200 && settings.errorMessages.length == 0
        home.output.includes("Reload log")
    },
})

feature("the Workspace answers its roots", {
    expect() {
        const answer = dev().render("Workspace/roots", null, null)
        const roots = JSON.parse(answer.output)
        answer.status == 200
        roots.tier == "full"
        roots.roots.some((root) => root.components.some((component) => component.name == "dev-aide"))
    },
})

feature("Artifacts: search, references and customizations render in both modes", {
    expect() {
        const search = dev().render("Artifacts/Search", { text: "Example", kind: "entity" }, null)
        const refs = dev().render("Artifacts/Refs", { artifactId: "entity:moqui.example.Example" }, null)
        const custom = dev().render("Artifacts/Customizations", null, null)
        const refsTui = dev().renderMode("tui").render("Artifacts/Refs", { artifactId: "entity:moqui.example.Example" }, null)
        search.status == 200 && search.errorMessages.length == 0
        search.output.includes("moqui.example.Example")
        refs.status == 200 && refs.errorMessages.length == 0
        refs.output.includes("Used by")
        custom.status == 200 && custom.errorMessages.length == 0
        custom.output.includes("Site extensions")
        refsTui.status == 200 && refsTui.errorMessages.length == 0
    },
})

feature("the palette's Artifacts group answers entities and services", {
    expect() {
        const rows = ec.service.sync().name("org.moqui.devaide.ArtifactServices.search#Palette").parameters({ text: "create#UserAccount" }).call().rows
        rows.length > 0
        rows[0].url.startsWith("/qapps/dev/Workspace?rootId=")
    },
})

feature("the Workspace runs XML operations and answers the outline", {
    expect() {
        const ops = JSON.stringify([{ op: "insert", at: [], position: "last", xml: "<extend-entity entity-name=\"Example\"/>" }])
        const answer = dev().render("Workspace/xmlOp", { text: "<?xml version=\"1.0\"?>\n<entities>\n</entities>\n", ops }, null)
        const outline = dev().render("Workspace/outline", { text: "<entities><entity entity-name=\"A\"/></entities>" }, null)
        answer.status == 200
        JSON.parse(answer.output).text.includes("<extend-entity entity-name=\"Example\"/>")
        JSON.parse(answer.output).outline.children[0].name == "extend-entity"
        JSON.parse(outline.output).outline.children[0].attrs[0][1] == "A"
    },
})

feature("Repositories, Data and Settings's tokens render in both modes", {
    expect() {
        const repositories = dev().render("Repositories", null, null)
        const repositoriesTui = dev().renderMode("tui").render("Repositories", null, null)
        const data = dev().render("Data", { component: "dev-aide" }, null)
        const dataTui = dev().renderMode("tui").render("Data", { component: "dev-aide", type: "seed" }, null)
        repositories.errorMessages.join("; ") == ""
        repositories.status == 200
        repositories.output.includes("RepositoryList")
        repositories.output.includes("CloneForm")
        repositoriesTui.errorMessages.join("; ") == ""
        data.errorMessages.join("; ") == ""
        data.output.includes("DevAideAgentData.xml")
        dataTui.errorMessages.join("; ") == ""
        dataTui.output.includes("DevAideSecurityData.xml")
    },
})

feature("Tasks renders the source, my tasks and Quick create in both modes", {
    expect() {
        const tasks = dev().render("Tasks", null, null)
        const tasksTui = dev().renderMode("tui").render("Tasks", null, null)
        tasks.errorMessages.join("; ") == ""
        tasks.status == 200
        tasks.output.includes("TaskSourceForm")
        tasks.output.includes("TaskList")
        tasks.output.includes("TaskSourceAddForm")
        tasksTui.errorMessages.join("; ") == ""
    },
})
