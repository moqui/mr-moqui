// A dataset's services (crates/moqui-report/src/definitions.rs `service_refusal`; plans/Security-Review-Medium.md
// SEC-TLS-91): a `serviceName` is called in process as each viewer with the parameters a panel's author wrote, and a
// `rowFilterService` as each viewer too, so a saved dataset names only a service a shipped dataset names there (CalcGrid's
// get#CalcFrame), and as its row filter also a registered scope policy. save#Definition refuses any other, and a dataset
// row written past it is refused at its read. Run: ./moq test framework --spec ReportDatasetServices
const R = "org.moqui.report.ReportServices."

function attempt(run) {
    try {
        return {out: run()}
    } catch (error) {
        ec.message.clearErrors()
        return {error: String(error.message)}
    }
}
const save = (id, body) =>
    attempt(() => call(R + "save#Definition", {definition: {id, type: "dataset", title: id, body}}))

spec({
    setupSpec() {
        login("john.doe")
    },
    cleanupSpec() {
        ec.message.clearErrors()
    }
})

feature("a saved dataset names only a shipped dataset's service, or a registered scope policy as its row filter", {
    when() {
        const echo = save("rds-echo", {serviceName: "org.moqui.impl.BasicServices.echo#Data"})
        const filter = save("rds-filter", {documentType: "RdsNone", rowFilterService: "org.moqui.impl.BasicServices.echo#Data"})
        const frame = save("rds-frame", {serviceName: "org.moqui.calc.CalcServices.get#CalcFrame"})
        const policy = save("rds-policy", {documentType: "RdsNone", rowFilterService: R + "get#UserGroupRowFilter"})
        // a row written past save#Definition
        const wasDisabled = ec.artifactExecution.disableAuthz()
        try {
            const row = ec.entity.makeValue("moqui.report.Definition")
            row.setFields({definitionId: "rds-written", definitionType: "dataset", title: "rds-written", ownerId: "EX_JOHN_DOE",
                readOnly: "N", versionNumber: 1, bodyJson: JSON.stringify({serviceName: "org.moqui.impl.BasicServices.echo#Data"})},
                true, null, null)
            row.create()
        } finally {
            if (!wasDisabled) ec.artifactExecution.enableAuthz()
        }
        const read = attempt(() => call(R + "run#Query", {query: {datasetId: "rds-written", from: "now-1h", thru: "now"}}))
        return {echo, filter, frame, policy, read}
    },
    then({echo, filter, frame, policy, read}) {
        echo.error.includes("serviceName is one a shipped dataset names")
        filter.error.includes("rowFilterService is one a shipped dataset names or a registered scope policy")
        frame.error === undefined
        policy.error === undefined
        read.error.includes("dataset rds-written: a dataset's serviceName")
    }
})
