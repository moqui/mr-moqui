// The ETL screens (screen/Tools/Etl.xml and Etl/): the JSON transitions Mapping.qvue and the grid's "Load into…" use
// (match, sample, save, read back, a write by GET refused), the Mappings, Run and terminal Mapping renders, and the
// Database › Connections and Settings screens. The services themselves are in framework/test (EtlServices) and
// crates/moqui-tools.
// Run: ./moq test tools --spec Etl
const tools = () => ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml")
const json = (r) => JSON.parse(String(r.output || "{}"))
// the spec's own file in dbresource://, written and removed with authorization set aside
const asSystem = (fn) => { const was = ec.artifactExecution.disableAuthz(); try { return fn() } finally { if (!was) ec.artifactExecution.enableAuthz() } }

spec({ setupSpec() { login("john.doe") } })

feature("a file mapping is matched, sampled with its errors, saved, listed and read back", {
    expect() {
        const steps = () => {
            const location = "dbresource://moqui/etl/spec-" + Date.now().toString(36) + ".csv"
            asSystem(() => ec.resource.getLocationReference(location).putText("Test Id,Test Number Integer\nETLSPEC1,12\nETLSPEC2,twelve\n"))
            const matched = json(tools().render("Etl/etlMatch", { columnsJson: JSON.stringify(["Test Id", "Test Number Integer"]),
                targetTypeEnumId: "EtltEntity", targetName: "moqui.test.TestEntity" }, "post"))
            const mapping = { description: "Etl spec", sourceTypeEnumId: "EtlsFile", sourceLocation: location, targetTypeEnumId: "EtltEntity",
                targetName: "moqui.test.TestEntity", loadModeEnumId: "EtllCreate",
                fields: (matched.matches || []).map((match) => ({ targetField: match.targetField, sourceColumn: match.column })) }
            const sample = json(tools().render("Etl/etlCheck", { mappingJson: JSON.stringify(mapping), maxRows: 200 }, "post"))
            const stored = json(tools().render("Etl/etlStore", { mappingJson: JSON.stringify(mapping) }, "post"))
            const readBack = json(tools().render("Etl/etlMapping", { mappingId: stored.mappingId }, "get"))
            const list = tools().render("Etl/Mappings", { term: "Etl spec" }, null)
            const tui = ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").renderMode("tui").render("Etl/Mapping", { mappingId: stored.mappingId }, null)
            const badCron = json(tools().render("Etl/etlSchedule", { mappingId: stored.mappingId, cronExpression: "every day" }, "post"))
            const badJobs = asSystem(() => ec.entity.find("moqui.service.job.ServiceJobParameter").condition("parameterName", "mappingId").condition("parameterValue", stored.mappingId).count())
            const scheduled = json(tools().render("Etl/etlSchedule", { mappingId: stored.mappingId, cronExpression: "0 0 3 * * ?" }, "post"))
            const jobUsers = asSystem(() => scheduled.jobName ? ec.entity.find("moqui.service.job.ServiceJobUser").condition("jobName", scheduled.jobName).list().map((row) => row.userId) : [])
            const job = asSystem(() => scheduled.jobName ? ec.entity.find("moqui.service.job.ServiceJob").condition("jobName", scheduled.jobName).one() : null)
            if (scheduled.jobName) asSystem(() => { for (const name of ["ServiceJobUser", "ServiceJobParameter", "ServiceJob"]) ec.entity.find("moqui.service.job." + name).condition("jobName", scheduled.jobName).deleteAll() })
            const getStore = tools().render("Etl/etlStore", { mappingJson: JSON.stringify(mapping) }, "get").status
            const deleted = tools().render("Etl/Mappings/deleteMapping", { mappingId: stored.mappingId }, "post").status
            asSystem(() => ec.resource.getLocationReference(location).delete())
            return { matched, sample, stored, readBack, list, tui, getStore, deleted, scheduled, jobUsers, badCron, badJobs, job }
        }
        const out = steps()
        out.matched.errors == null
        out.matched.matches.map((match) => match.targetField).join(",") == "testId,testNumberInteger"
        out.sample.errors == null
        out.sample.rows.length == 2
        out.sample.rows[0].values.testNumberInteger == 12
        Object.keys(out.sample.rows[1].errors).length > 0
        out.stored.errors == null && !!out.stored.mappingId
        out.readBack.mapping.fields.length == 2
        out.list.status == 200 && String(out.list.output).includes("Etl spec")
        out.tui.status == 200 && String(out.tui.output).includes('"MappingFields"')
        out.getStore == 405
        out.deleted < 400
        out.scheduled.errors == null && !!out.scheduled.jobName
        out.jobUsers.join(",") == ec.user.userId
        out.job != null && out.job.paused == "N" && out.job.serviceName == "org.moqui.impl.EtlServices.run#EtlMapping"
        !!out.badCron.errors && out.badJobs == 0
    },
})

feature("a run that is not there is no run, to anyone", {
    expect() {
        const r = tools().render("Etl/Run", { runId: "NO_SUCH_ETL_RUN" }, null)
        r.status == 200
        String(r.output).includes("NO_SUCH_ETL_RUN")
    },
})

feature("Database › Settings lists the database's settings, filtered by name", {
    expect() {
        const r = tools().render("Database/Settings", { nameFilter: "work_mem" }, null)
        r.status == 200
        r.errorMessages.length == 0
        String(r.output).includes("work_mem")
    },
})

feature("Database › Connections lists the connections, or reads Not available where it is not offered", {
    expect() {
        const r = tools().render("Database/Connections", {}, null)
        const out = String(r.output)
        r.status == 200
        r.errorMessages.length == 0
        // the server's gate decides which; exactly one of the two
        out.includes("External databases") != out.includes("Not available")
    },
})

feature("the mapping form is its qvue component", {
    expect() {
        const r = ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").renderMode("qvue").render("Etl/Mapping", {}, null)
        r.status == 200
        r.errorMessages.length == 0
        String(r.output).includes("Test against another file")
    },
})
