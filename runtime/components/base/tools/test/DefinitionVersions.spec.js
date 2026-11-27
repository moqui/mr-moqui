// A definition's version history shows when each version was saved, in the viewer's zone with the zone named once, not
// the epoch milliseconds the versions service answers (plans/ui-review-dashboards.md DASH-UI-005).
// Run: ./moq test tools --spec DefinitionVersions
const R = "org.moqui.report.ReportServices."
const ID = "definition-versions-spec"

function removeDefinition() {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        for (const entityName of [
            "moqui.report.DefinitionReference",
            "moqui.report.DefinitionVersion",
            "moqui.report.Definition"
        ])
            ec.entity.find(entityName).condition("definitionId", ID).deleteAll()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}

spec({
    setupSpec() {
        login("john.doe")
        removeDefinition()
    },
    cleanupSpec() {
        ec.message.clearErrors()
        removeDefinition()
    }
})

feature("the version history shows each saved time as a date in the viewer's zone", {
    when() {
        const saved = call(R + "save#Definition", {
            definition: {
                id: ID,
                type: "panel",
                title: "Versions spec",
                body: {title: "Versions spec", visualization: "text", text: "versions"}
            }
        })
        const versions = call(R + "versions#Definition", {definitionId: ID}).versions || []
        const result = ec.screen
            .makeTest()
            .rootScreen("component://tools/screen/Dashboards.xml")
            .renderMode("qvt")
            .withStaticData()
            .render("Definition", {lastStandalone: "true", definitionId: ID}, null)
        const output = String(result.output || "")
        const millis = String(versions[0]?.changedDate || "")
        return {
            saved,
            versions,
            status: result.status,
            output,
            millis,
            shown: /^\d+$/.test(millis) ? ec.l10n.format(new Date(Number(millis)), "yyyy-MM-dd HH:mm:ss") : ""
        }
    },
    then({saved, versions, status, output, millis, shown}) {
        saved.error === undefined
        versions.length >= 1
        // the service answers epoch milliseconds; the screen shows them as a date
        millis.length == 13 && String(Number(millis)) == millis
        status == 200
        output.includes(shown)
        !output.includes(millis)
        output.includes("Times shown in " + ec.user.timeZone + ".")
    }
})
