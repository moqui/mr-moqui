// A 100,000 row CSV loaded through run#EtlMapping in batches (plans/Tools-Database.md §15 step 6): every row loads in
// committed batches of 5,000, with no errors, under a time budget. The file is made here, in dbresource://moqui/etl/.
// The run is here with wait: true; the queued path (a job on the worker pool, its EtlRun from pending to done) is the
// etl_runs host test (crates/moqui-host/tests/it/etl_runs.rs).
// Run: ./moq accept tools --spec EtlLoad
const ROWS = 100000
// seconds for the run: the whole feature (making the file, the run in 20 batches, the count) took 34 s on a debug build
// on the embedded database (Nov 5, 2026), so about 3,300 rows a second; the budget allows a slower or busier machine
const BUDGET_SECONDS = 120
const asSystem = (fn) => { const was = ec.artifactExecution.disableAuthz(); try { return fn() } finally { if (!was) ec.artifactExecution.enableAuthz() } }
// each piece of the cleanup on its own, so one that fails leaves the rest done
const tidy = (what, fn) => { try { asSystem(fn) } catch (e) { ec.logger.error("ETL load acceptance: could not remove " + what + ": " + String(e.message || e)) } }

feature("a 100,000 row CSV loads through an ETL mapping in batches under the time budget", {
    timeout: 600,
    expect() {
        const steps = () => {
            login("john.doe")
            const tag = "EA" + Date.now().toString(36).toUpperCase()
            const location = "dbresource://moqui/etl/accept-" + tag + ".csv"
            let out = {}
            try {
                const lines = ["Test Id,Test Medium,Test Number Integer,Test Date"]
                for (let i = 1; i <= ROWS; i++) lines.push(tag + "-" + i + ",Row " + i + "," + i + ",2026-11-" + String(1 + (i % 28)).padStart(2, "0"))
                asSystem(() => ec.resource.getLocationReference(location).putText(lines.join("\n") + "\n"))
                const mapping = { description: "ETL load acceptance", sourceTypeEnumId: "EtlsFile", sourceLocation: location, fileFormat: "csv",
                    targetTypeEnumId: "EtltEntity", targetName: "moqui.test.TestEntity", loadModeEnumId: "EtllCreate", batchSize: 5000, stopOnError: "N",
                    fields: [{ targetField: "testId", sourceColumn: "Test Id" }, { targetField: "testMedium", sourceColumn: "Test Medium" },
                        { targetField: "testNumberInteger", sourceColumn: "Test Number Integer" }, { targetField: "testDate", sourceColumn: "Test Date" }] }
                const started = Date.now()
                // as from the Tools app (its authorization for all actions), as the ETL screens call it
                ec.artifactExecution.runAsUser("component://tools/screen/Tools.xml", () => {
                    out = ec.service.sync().name("org.moqui.impl.EtlServices.run#EtlMapping").parameters({ mapping, wait: true }).call() || {}
                })
                const seconds = (Date.now() - started) / 1000
                const errors = ec.message.hasError() ? ec.message.errorsString : null
                ec.message.clearErrors()
                const loaded = asSystem(() => ec.entity.find("moqui.test.TestEntity").condition("testId", "like", tag + "-%").count())
                ec.logger.info("ETL load acceptance: " + ROWS + " rows in " + seconds.toFixed(1) + " s, " + (out.committedBatches || 0) + " batches, status " + out.statusId)
                return { out, seconds, errors, loaded }
            } finally {
                ec.message.clearErrors()
                tidy("the loaded rows", () => ec.entity.find("moqui.test.TestEntity").condition("testId", "like", tag + "-%").deleteAll())
                tidy("the file", () => ec.resource.getLocationReference(location).delete())
                if (out.runId) {
                    tidy("the run's errors", () => ec.entity.find("moqui.etl.EtlRunError").condition("runId", out.runId).deleteAll())
                    tidy("the run", () => ec.entity.find("moqui.etl.EtlRun").condition("runId", out.runId).deleteAll())
                }
            }
        }
        const r = steps()
        r.errors == null
        r.out.statusId == "EtlrDone"
        r.out.loadCount == ROWS
        r.out.errorCount == 0
        r.out.committedBatches == ROWS / 5000
        r.loaded == ROWS
        r.seconds < BUDGET_SECONDS
    },
})
