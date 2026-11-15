// The entity modeler's terminal twin (screen/DevAide/Entities/EditTui.xml, plans/design-dev-aide.md D14): Add field by
// kind stages a Picklist's field and relationship in the entity's file and its records in the site's seed file, an
// attribute edit and a Remove stage theirs, and nothing is written; the twin's one tui render (its own tui texts: the
// tabs) shows the pending line, the staged field and Remove's question with what uses the field; Save of a throw-away
// root's seed file loads its records after it; and a table change is planned for the full tier, run only with the
// table's name typed back. The example component stands in as the site for the spec, so its real files are staged and
// then discarded, never written.
// Run: ./moq test dev-aide --spec EntityEditTui
const fixture = () => ec.resource.script("component://dev-aide/test/GuidedFixture.js")
const dev = () => ec.screen.makeTest().rootScreen("component://dev-aide/screen/DevAide.xml")
const ROOT = "dev-aide-guided-entity-spec"
const EXAMPLE = "entity:moqui.example.Example"
const entitiesFile = "component://example/entity/ExampleEntities.xml"
const SEED = "<entity-facade-xml type=\"seed\">\n    <moqui.basic.EnumerationType enumTypeId=\"SpecGuidedType\" description=\"Guided spec\"/>\n" +
    "    <moqui.basic.Enumeration enumId=\"SpecGuidedOne\" enumTypeId=\"SpecGuidedType\" description=\"One\"/>\n</entity-facade-xml>\n"
const setting = (value) => {
    ec.artifactExecution.disableAuthz()
    try {
        if (value) { const row = ec.entity.makeValue("moqui.dev.DevAideSetting"); row.setFields({settingName: "siteComponents", settingValue: value}, true, null, null); row.createOrUpdate() }
        else ec.entity.find("moqui.dev.DevAideSetting").condition("settingName", "siteComponents").deleteAll()
    } finally { ec.artifactExecution.enableAuthz() }
}
const stagedFiles = () => ec.service.sync().name("org.moqui.devaide.WorkspaceServices.get#EditBuffer").parameters({target: EXAMPLE}).call().pending

spec({
    setupSpec() {
        login("john.doe")
        fixture().setup(ROOT, {"data/GuidedSeed.xml": "<entity-facade-xml type=\"seed\">\n</entity-facade-xml>\n"})
        setting("example")
    },
    cleanupSpec() {
        fixture().discard(EXAMPLE)
        setting("")
        fixture().cleanup(ROOT, ["data/GuidedSeed.xml"])
    },
})

feature("edits are staged, not written, and the twin in the terminal shows them with Remove's question", {
    when() {
        const before = ec.resource.getLocationText(entitiesFile, false)
        const twin = dev()
        const picklist = twin.render("Entities/EditTui/addField", {artifact: EXAMPLE, fieldKind: "Picklist", fieldWords: "Priority", fieldValues: "High\nLow"}, "post")
        const described = twin.render("Entities/EditTui/updateField", {artifact: EXAMPLE, fieldName: "exampleName", fieldType: "text-medium",
            description: "What the example is called"}, "post")
        const view = ec.service.sync().name("org.moqui.devaide.EntityServices.get#EntityView").parameters({entityName: "moqui.example.Example"}).call().view
        const files = ec.service.sync().name("org.moqui.devaide.WorkspaceServices.get#EditBuffer").parameters({target: EXAMPLE,
            files: JSON.stringify([{rootId: view.target.rootId, path: view.target.path}, {rootId: view.seedFile.rootId, path: view.seedFile.path}])}).call().files
        const r = twin.renderMode("tui").render("Entities/EditTui", {artifact: EXAMPLE, tab: "fields", confirmRemove: "exampleName"}, null)
        const removed = dev().render("Entities/EditTui/removeField", {artifact: EXAMPLE, fieldName: "exampleName"}, "post")
        const afterRemove = ec.service.sync().name("org.moqui.devaide.WorkspaceServices.get#EditBuffer").parameters({target: EXAMPLE,
            files: JSON.stringify([{rootId: view.target.rootId, path: view.target.path}])}).call().files[0].text
        dev().render("Entities/EditTui/discardBuffer", {artifact: EXAMPLE}, "post")
        return {before, picklist, described, files, r, out: String(r.output), removed, afterRemove, pending: stagedFiles(),
            now: ec.resource.getLocationText(entitiesFile, false)}
    },
    then({before, picklist, described, files, r, out, removed, afterRemove, pending, now}) {
        picklist.status == 302
        described.status == 302
        // the field and its relationship in the entity's file, the type and values in the seed file, as the browser makes them
        files[0].staged
        files[0].text.includes("<field name=\"priorityEnumId\" type=\"id\"/>")
        files[0].text.includes("<relationship type=\"one\" title=\"Priority\" related=\"moqui.basic.Enumeration\"><key-map field-name=\"priorityEnumId\" related=\"enumId\"/></relationship>")
        files[0].text.includes("<description>What the example is called</description>")
        files[1].staged
        files[1].text.includes("<moqui.basic.EnumerationType enumTypeId=\"ExamplePriority\"")
        files[1].text.includes("enumId=\"ExamplePriorityHigh\"")
        // the one render: no error, the header with where the edits go and the pending line, the tabs, the staged field, Remove's question
        r.status == 200
        r.errorMessages.length == 0
        !out.includes("Error rendering")
        !out.includes("\"t\":\"error\"")
        out.includes("Edits go to ")
        out.includes("2 unsaved changes in ExampleEntities.xml")
        out.includes("\"t\":\"subscreens\"")
        out.includes("\"title\":\"Text 2\"")
        out.includes("\"priorityEnumId\",\"id\",\"Picklist\"")
        out.includes("\"staged\"")
        out.includes("Remove exampleName? It is used in")
        out.includes("\"AddFieldForm\"")
        // Remove stages the field's removal; Discard drops it all; the file was never written
        removed.status == 302
        !afterRemove.includes("<field name=\"exampleName\"")
        pending.count == 0
        now == before
    }
})

feature("Save writes a throw-away root's seed file and loads its records after it", {
    when() {
        const f = fixture()
        f.discard(EXAMPLE)
        const disk = f.staged(EXAMPLE, [{rootId: ROOT, path: "data/GuidedSeed.xml"}]).files[0]
        ec.service.sync().name("org.moqui.devaide.WorkspaceServices.put#EditBuffer").parameters({target: EXAMPLE,
            files: JSON.stringify([{rootId: ROOT, path: "data/GuidedSeed.xml", text: SEED, baseHash: disk.baseHash, load: true}])}).call()
        f.guard(EXAMPLE, ROOT)
        const saved = dev().render("Entities/EditTui/saveBuffer", {artifact: EXAMPLE}, "post")
        const loaded = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "SpecGuidedOne").disableAuthz().one()
        return {saved, text: f.read(ROOT, "data/GuidedSeed.xml"), loaded, pending: stagedFiles()}
    },
    then({saved, text, loaded, pending}) {
        saved.status == 302
        text == SEED
        loaded != null
        loaded.description == "One"
        pending.count == 0
    }
})

feature("a table change is planned for the full tier and runs only with the table's name typed back", {
    when() {
        const twin = dev()
        twin.render("Entities/EditTui/planDdl", {artifact: EXAMPLE, ddlAction: "drop-column", columnName: "old_column"}, "post")
        const plan = JSON.parse(ec.web.sessionAttributes["DevAideGuided.ddl.moqui.example.Example"] || "{}")
        const refused = twin.render("Entities/EditTui/runDdl", {artifact: EXAMPLE, ddlAction: "drop-column", columnName: "old_column", confirmTable: "wrong"}, "post")
        const left = JSON.parse(ec.web.sessionAttributes["DevAideGuided.ddl.moqui.example.Example"] || "null")
        twin.render("Entities/EditTui/cancelDdl", {artifact: EXAMPLE}, "post")
        return {plan, refused, left, cancelled: ec.web.sessionAttributes["DevAideGuided.ddl.moqui.example.Example"]}
    },
    then({plan, refused, left, cancelled}) {
        plan.statements[0].includes("DROP COLUMN")
        plan.table.toLowerCase() == "example"
        refused.errorMessages.some((text) => text.includes("type the table's name"))
        left != null
        !cancelled
    }
})
