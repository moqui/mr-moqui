// Entity data export (screen/Tools/Data/Export.xml, EntityExport): its filter is a JSON object of field values, parsed
// and never evaluated, since an expression there ran as a server script with the caller's ec (plans/Security-Review.md
// SEC-TLS-01). The transition is not read-only, so a GET with URL parameters does not run it: the client check of that
// is in framework/test/client/SecurityScreens.spec.js.
// Run: ./moq test tools --spec EntityExport
const marker = "runtime://tmp/EntityExportSpec.ran"
const ran = () => !!ec.resource.getLocationReference(marker)?.getExists()
const remove = () => {
    if (ran()) ec.resource.getLocationReference(marker).delete()
}
const exportOf = (filterMap) => ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").render("Data/Export/EntityExport",
    { entityNames: "moqui.basic.Enumeration", filterMap, fileType: "JSON", output: "browser" }, "post")

spec({
    setupSpec() { login("john.doe") },
    cleanup() { remove() },
})

feature("a script filter is refused without running; a JSON filter selects the records", {
    when() {
        remove()
        const script = exportOf(`(function () { ec.resource.getLocationReference('${marker}').putText('ran'); return {enumTypeId: 'X'} })()`)
        const scriptRan = ran()
        ec.message.clearErrors()
        const json = exportOf('{"enumTypeId": "EmailType"}')
        ec.message.clearErrors()
        return { script, scriptRan, json }
    },
    then({ script, scriptRan, json }) {
        !scriptRan
        script.errorMessages.join(" ").includes("not a JSON object")
        json.errorMessages.length == 0
        json.output.includes("EmailType")
        !json.output.includes("\"enumTypeId\": \"StatusType\"") && !json.output.includes("\"enumTypeId\":\"StatusType\"")
    },
})
