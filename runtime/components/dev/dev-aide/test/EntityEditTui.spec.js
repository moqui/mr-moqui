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
const SEED =
    '<entity-facade-xml type="seed">\n    <moqui.basic.EnumerationType enumTypeId="SpecGuidedType" description="Guided spec"/>\n' +
    '    <moqui.basic.Enumeration enumId="SpecGuidedOne" enumTypeId="SpecGuidedType" description="One"/>\n</entity-facade-xml>\n'
const setting = value => {
    ec.artifactExecution.disableAuthz()
    try {
        if (value) {
            const row = ec.entity.makeValue("moqui.dev.DevAideSetting")
            row.setFields({settingName: "siteComponents", settingValue: value}, true, null, null)
            row.createOrUpdate()
        } else ec.entity.find("moqui.dev.DevAideSetting").condition("settingName", "siteComponents").deleteAll()
    } finally {
        ec.artifactExecution.enableAuthz()
    }
}
const stagedFiles = () =>
    ec.service.sync().name("org.moqui.devaide.WorkspaceServices.get#EditBuffer").parameters({target: EXAMPLE}).call().pending

spec({
    setupSpec() {
        login("john.doe")
        fixture().setup(ROOT, {"data/GuidedSeed.xml": '<entity-facade-xml type="seed">\n</entity-facade-xml>\n'})
        setting("example")
    },
    cleanupSpec() {
        fixture().discard(EXAMPLE)
        setting("")
        fixture().cleanup(ROOT, ["data/GuidedSeed.xml"])
    }
})

feature("edits are staged, not written, and the twin in the terminal shows them with Remove's question", {
    when() {
        const before = ec.resource.getLocationText(entitiesFile, false)
        const twin = dev()
        const picklist = twin.render(
            "Entities/EditTui/addField",
            {artifact: EXAMPLE, fieldKind: "Picklist", fieldWords: "Priority", fieldValues: "High\nLow"},
            "post"
        )
        const described = twin.render(
            "Entities/EditTui/updateField",
            {artifact: EXAMPLE, fieldName: "exampleName", fieldType: "text-medium", description: "What the example is called"},
            "post"
        )
        const view = ec.service
            .sync()
            .name("org.moqui.devaide.EntityServices.get#EntityView")
            .parameters({entityName: "moqui.example.Example"})
            .call().view
        const files = ec.service
            .sync()
            .name("org.moqui.devaide.WorkspaceServices.get#EditBuffer")
            .parameters({
                target: EXAMPLE,
                files: JSON.stringify([
                    {rootId: view.target.rootId, path: view.target.path},
                    {rootId: view.seedFile.rootId, path: view.seedFile.path}
                ])
            })
            .call().files
        const r = twin
            .renderMode("tui")
            .render("Entities/EditTui", {artifact: EXAMPLE, tab: "fields", confirmRemove: "exampleName"}, null)
        const removed = dev().render("Entities/EditTui/removeField", {artifact: EXAMPLE, fieldName: "exampleName"}, "post")
        const afterRemove = ec.service
            .sync()
            .name("org.moqui.devaide.WorkspaceServices.get#EditBuffer")
            .parameters({target: EXAMPLE, files: JSON.stringify([{rootId: view.target.rootId, path: view.target.path}])})
            .call().files[0].text
        dev().render("Entities/EditTui/discardBuffer", {artifact: EXAMPLE}, "post")
        return {
            before,
            picklist,
            described,
            files,
            r,
            out: String(r.output),
            removed,
            afterRemove,
            pending: stagedFiles(),
            now: ec.resource.getLocationText(entitiesFile, false)
        }
    },
    then({before, picklist, described, files, r, out, removed, afterRemove, pending, now}) {
        picklist.status == 302
        described.status == 302
        // the field and its relationship in the entity's file, the type and values in the seed file, as the browser makes them
        files[0].staged
        files[0].text.includes('<field name="priorityEnumId" type="id"/>')
        // laid out a line an element, as the file is
        /<relationship type="one" title="Priority" related="moqui.basic.Enumeration">\n\s*<key-map field-name="priorityEnumId" related="enumId"\/>\n\s*<\/relationship>/.test(files[0].text)
        files[0].text.includes("<description>What the example is called</description>")
        files[1].staged
        files[1].text.includes('<moqui.basic.EnumerationType enumTypeId="ExamplePriority"')
        files[1].text.includes('enumId="ExamplePriorityHigh"')
        // the one render: no error, the header with where the edits go and the pending line, the tabs, the staged field, Remove's question
        r.status == 200
        r.errorMessages.length == 0
        !out.includes("Error rendering")
        !out.includes('"t":"error"')
        out.includes("Edits go to ")
        out.includes("2 unsaved changes in ExampleEntities.xml")
        out.includes('"t":"subscreens"')
        out.includes('"title":"Text 2"')
        out.includes('"priorityEnumId","id","Picklist"')
        out.includes('"staged"')
        out.includes("Remove exampleName? It is used in")
        out.includes('"AddFieldForm"')
        // Remove stages the field's removal; Discard drops it all; the file was never written
        removed.status == 302
        !afterRemove.includes('<field name="exampleName"')
        pending.count == 0
        now == before
    }
})

feature("a seed record's Edit, Add row and Remove are staged in the site's seed file, with the tab's list and dialogs", {
    when() {
        const f = fixture()
        f.discard(EXAMPLE)
        const view = ec.service
            .sync()
            .name("org.moqui.devaide.EntityServices.get#EntityView")
            .parameters({entityName: "moqui.example.Example"})
            .call().view
        const seed = {rootId: view.seedFile.rootId, path: view.seedFile.path}
        f.stage(EXAMPLE, seed.rootId, seed.path, SEED, f.staged(EXAMPLE, [seed]).files[0].baseHash)
        const twin = dev()
        // the type is record 0, the enumeration record 1
        const edited = twin.render(
            "Entities/EditTui/setSeedValue",
            {
                artifact: EXAMPLE,
                seedPath: "1",
                seedEntity: "moqui.basic.Enumeration",
                seed_description: "Uno",
                seedAttrName: "sequenceNum",
                seedAttrValue: "5"
            },
            "post"
        )
        const stale = twin.render(
            "Entities/EditTui/setSeedValue",
            {artifact: EXAMPLE, seedPath: "1", seedEntity: "moqui.basic.EnumerationType", seed_description: "Wrong"},
            "post"
        )
        const added = twin.render(
            "Entities/EditTui/addSeedRecord",
            {
                artifact: EXAMPLE,
                seedEntity: "moqui.basic.Enumeration",
                seed_enumId: "SpecGuidedTwo",
                seed_enumTypeId: "SpecGuidedType",
                seed_description: "Two"
            },
            "post"
        )
        const afterAdd = f.staged(EXAMPLE, [seed]).files[0].text
        const removed = twin.render(
            "Entities/EditTui/removeSeedRecord",
            {artifact: EXAMPLE, seedPath: "0", seedEntity: "moqui.basic.EnumerationType"},
            "post"
        )
        const afterRemove = f.staged(EXAMPLE, [seed]).files[0].text
        // the Seed data tab's nodes for the terminal, from the records as staged
        const guided = ec.resource.script("component://dev-aide/service/org/moqui/devaide/GuidedEdit.js")
        const entity = guided.entityContext(EXAMPLE)
        const nodes = guided.seedListNodes(
            EXAMPLE,
            entity.ops.entity.recordTables(entity.seedFile.outline),
            true,
            (name, parameters) =>
                name +
                (parameters
                    ? "?" +
                      Object.keys(parameters)
                          .map(key => key + "=" + parameters[key])
                          .join("&")
                    : "")
        )
        f.discard(EXAMPLE)
        return {edited, stale, added, afterAdd, removed, afterRemove, nodes, pending: stagedFiles()}
    },
    then({edited, stale, added, afterAdd, removed, afterRemove, nodes, pending}) {
        edited.status == 302
        afterAdd.includes(
            '<moqui.basic.Enumeration enumId="SpecGuidedOne" enumTypeId="SpecGuidedType" description="Uno" sequenceNum="5"/>'
        )
        // a record that is not the one the page showed is refused
        stale.errorMessages.some(text => text.includes("not in the seed file as staged"))
        !afterAdd.includes("Wrong")
        // Add row goes after the entity's last record, its columns in the table's order
        added.status == 302
        afterAdd.includes('<moqui.basic.Enumeration enumId="SpecGuidedTwo" enumTypeId="SpecGuidedType" description="Two"/>')
        afterAdd.indexOf('sequenceNum="5"') < afterAdd.indexOf('enumId="SpecGuidedTwo"')
        removed.status == 302
        !afterRemove.includes("<moqui.basic.EnumerationType")
        afterRemove.includes('enumId="SpecGuidedTwo"')
        // the list of each entity: its columns, Edit's dialog with a field per column, Remove with its question, Add row
        nodes.length == 3
        nodes[0].text == "moqui.basic.Enumeration"
        nodes[1].t == "formList"
        nodes[1].columns.map(column => column.title).join(",") == "enumId,enumTypeId,description,sequenceNum,"
        nodes[1].rows[0].id == "0"
        nodes[1].rows[0].cells[2] == "Uno"
        nodes[1].rows[0].cells[4][0].t == "dialog"
        nodes[1].rows[0].cells[4][0].c[0].transitionUrl == "setSeedValue"
        nodes[1].rows[0].cells[4][0].c[0].c.map(field => field.name).join(",") ==
            "seed_enumId,seed_enumTypeId,seed_description,seed_sequenceNum,seedAttrName,seedAttrValue,submitButton"
        nodes[1].rows[0].cells[4][0].c[0].c[2].w.value == "Uno"
        nodes[1].rows[0].cells[4][1].confirm == "Remove this moqui.basic.Enumeration record?"
        nodes[1].rows[0].cells[4][1].url.includes("seedPath=0")
        nodes[2].button == "Add row"
        pending.count == 0
    }
})

feature("Save writes a throw-away root's seed file and loads its records after it", {
    when() {
        const f = fixture()
        f.discard(EXAMPLE)
        const disk = f.staged(EXAMPLE, [{rootId: ROOT, path: "data/GuidedSeed.xml"}]).files[0]
        ec.service
            .sync()
            .name("org.moqui.devaide.WorkspaceServices.put#EditBuffer")
            .parameters({
                target: EXAMPLE,
                files: JSON.stringify([
                    {rootId: ROOT, path: "data/GuidedSeed.xml", text: SEED, baseHash: disk.baseHash, load: true}
                ])
            })
            .call()
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
        const refused = twin.render(
            "Entities/EditTui/runDdl",
            {artifact: EXAMPLE, ddlAction: "drop-column", columnName: "old_column", confirmTable: "wrong"},
            "post"
        )
        const left = JSON.parse(ec.web.sessionAttributes["DevAideGuided.ddl.moqui.example.Example"] || "null")
        twin.render("Entities/EditTui/cancelDdl", {artifact: EXAMPLE}, "post")
        return {plan, refused, left, cancelled: ec.web.sessionAttributes["DevAideGuided.ddl.moqui.example.Example"]}
    },
    then({plan, refused, left, cancelled}) {
        plan.statements[0].includes("DROP COLUMN")
        plan.table.toLowerCase() == "example"
        refused.errorMessages.some(text => text.includes("type the table's name"))
        left != null
        !cancelled
    }
})
