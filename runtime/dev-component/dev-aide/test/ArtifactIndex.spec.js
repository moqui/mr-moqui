// The artifact index on a dev instance (plans/design-dev-aide.md P3, 7.1): built from every component's files into the
// moqui.devtools rows, found by name with segment and camel case matching, its references both ways, and the
// extensions with whether their targets are there. The per-file update after a reload, the restart that keeps the rows
// and a broken anchor are in the host's integration test (crates/moqui-host/tests/it/devaide.rs).
// Run: ./moq test dev-aide --spec ArtifactIndex
const svc = (name) => ec.service.sync().name("org.moqui.impl.DevToolsServices." + name)

spec({
    setupSpec() { login("john.doe") },
})

feature("the index builds from the components' files", {
    expect() {
        const started = Date.now()
        const result = svc("rebuild#ArtifactIndex").parameters({ full: true }).call()
        const report = result.report
        ec.logger.info("artifact index: " + JSON.stringify(report) + " in " + (Date.now() - started) + " ms")
        !ec.message.hasError()
        report.read > 500
        report.artifacts > 10000
        report.refs > 10000
    },
})

feature("an unchanged file is not read again", {
    expect() {
        const report = svc("rebuild#ArtifactIndex").call().report
        report.read == 0
        report.unchanged > 500
    },
})

feature("find matches segments and camel case, an exact name first", {
    expect() {
        const byShort = svc("find#Artifacts").parameters({ text: "Example", kinds: "entity" }).call().artifacts
        const byAbbreviation = svc("find#Artifacts").parameters({ text: "UsrAcct", kinds: "entity" }).call().artifacts
        const services = svc("find#Artifacts").parameters({ text: "create#UserAccount", kinds: "service" }).call().artifacts
        byShort[0].id == "entity:moqui.example.Example"
        byShort[0].line > 0
        byAbbreviation[0].id == "entity:moqui.security.UserAccount"
        services[0].id == "service:org.moqui.impl.UserServices.create#UserAccount"
    },
})

feature("references both ways", {
    expect() {
        const users = svc("get#ArtifactRefs").parameters({ artifactId: "entity:moqui.example.Example" }).call().refs
        const uses = svc("get#ArtifactRefs")
            .parameters({ artifactId: "screen:component://example/screen/Gallery/ExampleDetail.xml", direction: "out" })
            .call().refs
        users.some((ref) => ref.refType == "entity-find-one" && ref.from.startsWith("screen:component://example/"))
        users.some((ref) => ref.refType == "data" && ref.other.kind == "data")
        uses.some((ref) => ref.toId == "entity:moqui.example.Example")
    },
})

feature("go to definition from a line", {
    expect() {
        const detail = svc("get#Artifact").parameters({ artifactId: "ExampleDetail" }).call().artifact
        const refs = svc("get#ArtifactRefs").parameters({ artifactId: detail.id, direction: "out" }).call().refs
        const first = refs.find((ref) => ref.toId == "entity:moqui.example.Example")
        const at = svc("get#ArtifactAt").parameters({ path: first.path, line: first.line, word: "moqui.example.Example" }).call()
        at.artifact.id == "entity:moqui.example.Example"
        at.artifact.path.endsWith("ExampleEntities.xml")
    },
})

feature("extensions with their state", {
    expect() {
        const result = svc("get#ArtifactExtensions").call()
        result.extensions.length > 0
        result.extensions.some((extension) => extension.kind == "extend-entity" && extension.resolved)
        result.brokenCount == 0
    },
})

feature("parameters have their own artifact and incoming field-map references", {
    expect() {
        const id = "parameter:service:org.moqui.impl.UserServices.get#UserAuthcFactorRequired@in.userId"
        const result = svc("get#Artifact").parameters({artifactId: id}).call()
        const refs = svc("get#ArtifactRefs").parameters({artifactId: id}).call().refs
        result.artifact.id == id
        result.artifact.kind == "parameter"
        result.artifact.parent == "service:org.moqui.impl.UserServices.get#UserAuthcFactorRequired"
        refs.some(ref => ref.refType == "field-map" && ref.toId == id && ref.resolved)
    }
})
