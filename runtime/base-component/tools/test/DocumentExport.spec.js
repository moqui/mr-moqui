// Data document export (screen/Tools/Data/Export.xml): DocumentExport.json, which report screens of other components
// transition-include, sends one document to the browser and never writes a file on the server; writing files is
// exportDocuments, a Tools action only.
// Run: ./moq test tools --spec DocumentExport
const fileName = "DocumentExportSpec.json"
const location = "runtime://tmp/" + fileName
const written = () => !!ec.resource.getLocationReference(location)?.getExists()
const remove = () => {
    if (written()) ec.resource.getLocationReference(location).delete()
}

spec({
    setupSpec() { login("john.doe") },
    cleanup() { remove() },
})

feature("an included DocumentExport.json does not write to a file; Tools' exportDocuments does", {
    expect() {
        const dataDocumentId = ec.entity.find("moqui.entity.document.DataDocument").disableAuthz().list()[0].dataDocumentId
        const path = ec.factory.getRuntimePath() + "/tmp/" + fileName
        remove()
        // as a report screen could post it: through System's ViewDataDocument, which includes the transition
        const included = ec.screen.makeTest().rootScreen("component://tools/screen/System.xml")
            .render("DataDocument/Edit/ViewDataDocument/DocumentExport.json", { dataDocumentIds: dataDocumentId, output: "file", path },
                "post")
        const includedWrote = written()
        ec.message.clearErrors()
        ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").render("Data/Export/exportDocuments",
            { dataDocumentIds: dataDocumentId, output: "file", path }, "post")
        included.errorMessages.join(" ").includes("No Data Document specified")
        !includedWrote
        written()
    },
})
