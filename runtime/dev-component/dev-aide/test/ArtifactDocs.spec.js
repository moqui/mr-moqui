// Autodoc in the Dev AIDE (plans/archive/Autodoc.md): an artifact's Markdown doc made from the dev server's artifact index, by
// path, by artifact and by name, with its tables, its diagrams and its links up; the Docs screen in the browser's mode
// and the terminal's; and an export the upstream lock refuses. Exports that write, their manifest and the tiers are in the
// host's integration test (crates/moqui-host/tests/it/devaide.rs), on scratch repositories; what each kind's doc holds
// is in the devkit's goldens (crates/moqui-devkit/src/autodoc/testdata).
// Run: ./moq test dev-aide --spec ArtifactDocs
const doc = params => ec.service.sync().name("org.moqui.devaide.DocServices.get#ArtifactDoc").parameters(params).call()
const dev = () => ec.screen.makeTest().rootScreen("component://dev-aide/screen/DevAide.xml")

spec({
    setupSpec() {
        login("john.doe")
    }
})

feature("an entity's doc has its fields, relationships, diagrams and way up", {
    when() {
        entity = doc({artifactId: "entity:moqui.example.Example"})
    },
    then() {
        !ec.message.hasError()
        entity.docPath == "entity/moqui/example/Example.md"
        entity.markdown.startsWith("# Example\n")
        entity.markdown.includes("## Fields")
        entity.markdown.includes("| `exampleId` | `id` | yes |")
        entity.markdown.includes("## Relationships")
        entity.markdown.includes("```mermaid\nerDiagram")
        entity.markdown.includes("```mermaid\nflowchart LR")
        entity.markdown.includes("Up: [moqui.example](README.md)")
        entity.crumbs.map(crumb => crumb.docPath).join(",") == "README.md,entity/README.md,entity/moqui/example/README.md"
        entity.basePath == "/qapps/dev/Artifacts/Doc/entity/moqui/example"
        entity.browserUrl.includes("artifactId=entity:moqui.example.Example")
        entity.workspaceUrl.startsWith("/qapps/dev/Workspace?rootId=")
    }
})

feature("a field's id, a name and a path open the same doc", {
    when() {
        byField = doc({artifactId: "field:moqui.example.Example.exampleName"}).docPath
        byName = doc({artifactId: "moqui.example.Example"}).docPath
        byPath = doc({docPath: "entity/moqui/example/Example.md"}).docPath
    },
    then() {
        byField == "entity/moqui/example/Example.md"
        byName == byField
        byPath == byField
    }
})

feature("a service's doc has its parameters and, when asked, its definition", {
    when() {
        plain = doc({artifactId: "service:org.moqui.impl.UserServices.create#UserAccount"})
        withSource = doc({artifactId: "service:org.moqui.impl.UserServices.create#UserAccount", includeSource: true})
    },
    then() {
        plain.docPath == "service/org/moqui/impl/UserServices/create-UserAccount.md"
        plain.markdown.includes("## In parameters")
        plain.markdown.includes("`username`")
        !plain.markdown.includes("## Definition")
        withSource.markdown.includes("## Definition\n\n```xml\n<service")
    }
})

feature("a package page has an E-R diagram and the root lists every tree", {
    when() {
        page = doc({docPath: "entity/moqui/example/README.md"})
        top = doc({docPath: "README.md"})
    },
    then() {
        page.page == "package"
        page.markdown.includes("[Example](Example.md)")
        page.markdown.includes("```mermaid\nerDiagram")
        top.page == "root"
        top.markdown.includes("[Entities](entity/README.md)")
        top.markdown.includes("[REST APIs](rest/README.md)")
    }
})

feature("there is no doc of an artifact that is not there", {
    expect() {
        ec.message.pushErrors()
        const answer = ec.service
            .sync()
            .name("org.moqui.devaide.DocServices.get#ArtifactDoc")
            .parameters({artifactId: "entity:no.such.Entity"})
            .requireNewTransaction(true)
            .call()
        const errors = ec.message.hasError() ? ec.message.errorsString : ""
        ec.message.clearErrors()
        ec.message.popErrors()
        !answer.markdown
        errors.includes("No doc of entity:no.such.Entity")
    }
})

feature("Docs renders a doc in both modes, its links under the Docs screen", {
    expect() {
        const page = dev().render("Artifacts/Doc/entity/moqui/example/Example.md", null, null)
        const terminal = dev().renderMode("tui").render("Artifacts/Doc/entity/moqui/example/Example.md", null, null)
        const missing = dev().render("Artifacts/Doc", {artifactId: "entity:no.such.Entity"}, null)
        page.status == 200 && page.errorMessages.length == 0
        page.output.includes("DocText")
        page.output.includes("Open in the Browser")
        !page.output.includes("Error rendering") && !page.output.includes("Exception in render")
        terminal.status == 200 && terminal.errorMessages.length == 0
        terminal.output.includes("exampleId")
        missing.status == 200
        missing.output.includes("There is no doc of entity:no.such.Entity")
    }
})

feature("Export docs renders in both modes, and an export into an upstream root is refused", {
    expect() {
        const page = dev().render("Artifacts/DocExport", null, null)
        const terminal = dev().renderMode("tui").render("Artifacts/DocExport", null, null)
        page.status == 200 && page.errorMessages.length == 0
        page.output.includes("ExportDocs")
        terminal.status == 200 && terminal.errorMessages.length == 0
        // with the lock off (upstream edits allowed in this conf) the export would write: not tried then
        const info = ec.service.sync().name("org.moqui.devaide.WorkspaceServices.get#Roots").call()
        const upstream = info.roots.find(root => root.upstream && !root.readOnly)
        let refused = "the lock is off: not tried; is upstream"
        if (!info.upstreamEdits && upstream) {
            ec.message.pushErrors()
            ec.service
                .sync()
                .name("org.moqui.devaide.DocServices.export#ArtifactDocs")
                .parameters({rootId: upstream.id, path: "autodoc-spec"})
                .requireNewTransaction(true)
                .call()
            refused = ec.message.hasError() ? ec.message.errorsString : ""
            ec.message.clearErrors()
            ec.message.popErrors()
        }
        refused.includes("is upstream")
    }
})
