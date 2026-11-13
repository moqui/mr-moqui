// Calc mode of the data grid (screen/Tools/Data/Grid.xml, plans/IronCalc-DB.md Phase 2), over org.moqui.calc.CalcServices:
// an entity's rows as the data sheet of a workbook in a room of the user's, read a rectangle at a time; the document saved
// as a report definition with no row of the data in it, opened again and deleted; another user finds no such room.
// The engine's work (the fill, the stored form, the rooms) is tested in crates/moqui-calc.
// Run: ./moq test tools --spec CalcGrid
const tools = () => ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml")
const call = (transition, parameters, method) => {
    const result = tools().render("Data/Grid/" + transition, parameters, method || null)
    const text = String(result.output || "")
    let json = null
    try { json = JSON.parse(text) } catch (e) { json = null }
    return {status: result.status, text, json: json || {}}
}
const source = {type: "entity", entityName: "moqui.basic.Enumeration", conditions: {enumTypeId: "ArtifactType"}}

const OUTSIDER = "spec.calcgrid.outsider"
const OUTSIDER_ID = "SPEC_CALCGRID_OUTSIDER"

spec({
    setupSpec() {
        login("john.doe")
        ec.artifactExecution.disableAuthz()
        const row = ec.entity.makeValue("moqui.security.UserAccount")
        row.setFields({userId: OUTSIDER_ID, username: OUTSIDER, userFullName: "Spec CalcGrid Outsider"}, true, null, null)
        row.create()
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        ec.entity.find("moqui.security.UserAccount").condition("userId", OUTSIDER_ID).deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature("an entity opens as the data sheet of a workbook, read a rectangle at a time, exported, and closed", {
    when() {
        const opened = call("calcOpen", {sourceJson: JSON.stringify(source), columnsJson: JSON.stringify(["enumId", "description", "enumTypeId"]),
            sortJson: JSON.stringify([{field: "enumId", dir: "asc"}])}, "post")
        const roomId = opened.json.roomId
        const sheet = (opened.json.sheets || [])[0] || {}
        const cells = call("calcCells", {roomId, sheet: 0, row: 1, column: 1, width: 3, height: 3})
        const texts = (cells.json.texts || []).map(text => text.row + ":" + text.column + "=" + text.text)
        const tooMany = call("calcCells", {roomId, sheet: 0, row: 1, column: 1, width: 300, height: 300})
        const exported = tools().render("Data/Grid/calcExport", {roomId}, null)
        // the room is its user's: another user's call finds none, even with authorization off
        ec.user.internalLoginUser(OUTSIDER)
        const outsider = ec.service.sync().name("org.moqui.calc.CalcServices.get#CalcCells").parameters({roomId, sheet: 0, row: 1, column: 1, width: 3, height: 3}).disableAuthz().call() || {}
        const others = {blockJson: outsider.blockJson, errors: ec.message.getErrorsString()}
        ec.message.clearErrors()
        login("john.doe")
        call("calcClose", {roomId}, "post")
        const closed = call("calcCells", {roomId, sheet: 0, row: 1, column: 1, width: 3, height: 3})
        return {opened, sheet, cells, texts, tooMany, exported, others, closed}
    },
    then({opened, sheet, cells, texts, tooMany, exported, others, closed}) {
        opened.status == 200
        !opened.json.errors
        !!opened.json.roomId
        sheet.name == "Enumeration"
        sheet.data === true
        sheet.columns == 3
        sheet.dataRows >= 5
        sheet.rows == sheet.dataRows + 1
        sheet.frozenRows == 1
        cells.status == 200
        texts.includes("1:1=enumId")
        texts.includes("1:2=description")
        texts.includes("2:1=AT_ENTITY")
        texts.includes("2:3=ArtifactType")
        cells.json.style_indices.length == 9
        String(tooMany.json.errors).includes("at most 20000 cells")
        exported.status == 200
        !others.blockJson
        others.errors.includes("ROOM_GONE")
        String(closed.json.errors).includes("ROOM_GONE")
    },
})

feature("a document is saved with its source and no row of the data, opened again with the data read again, and deleted", {
    when() {
        const opened = call("calcOpen", {sourceJson: JSON.stringify(source)}, "post")
        const saved = call("calcSave", {roomId: opened.json.roomId, title: "Artifact types"}, "post")
        const definitionId = saved.json.definitionId
        const stored = (ec.service.sync().name("org.moqui.report.ReportServices.get#Definition").parameters({definitionId}).call() || {}).definition || {}
        const body = stored.body || {}
        const listed = call("calcDocuments", {})
        const listedDoc = Array.from(listed.json.documents || []).find(doc => doc.definitionId == definitionId) || {}
        const again = call("calcOpen", {definitionId}, "post")
        const cells = call("calcCells", {roomId: again.json.roomId, sheet: 0, row: 1, column: 1, width: 12, height: 3})
        const cellTexts = Array.from(cells.json.texts || []).map(text => text.text)
        const refreshed = call("calcRefresh", {roomId: again.json.roomId}, "post")
        const deleted = call("calcDelete", {definitionId}, "post")
        const gone = call("calcOpen", {definitionId}, "post")
        call("calcClose", {roomId: opened.json.roomId}, "post")
        call("calcClose", {roomId: again.json.roomId}, "post")
        ec.message.clearErrors()
        return {saved, stored, body, listed, listedDoc, again, cellTexts, refreshed, deleted, gone, definitionId}
    },
    then({saved, stored, body, listed, listedDoc, again, cellTexts, refreshed, deleted, gone, definitionId}) {
        !saved.json.errors
        String(definitionId).startsWith("calc-artifact-types-")
        stored.type == "calcgrid"
        stored.title == "Artifact types"
        body.dataSheets.length == 1
        body.dataSheets[0].name == "Enumeration"
        body.dataSheets[0].source.entityName == "moqui.basic.Enumeration"
        // the workbook is stored without the data: the header's names are in it, a record's text is not
        String(body.overlay).length > 100
        !JSON.stringify(stored).includes("AT_ENTITY")
        listedDoc.title == "Artifact types"
        listedDoc.ownerId == "EX_JOHN_DOE"
        !listed.text.includes("overlay")
        !again.json.errors
        again.json.definitionId == definitionId
        again.json.title == "Artifact types"
        again.json.sheets[0].dataRows >= 5
        cellTexts.includes("AT_ENTITY")
        !refreshed.json.errors
        refreshed.json.sheets[0].dataRows == again.json.sheets[0].dataRows
        !deleted.json.errors
        !!gone.json.errors
    },
})

feature("a spreadsheet file opens as a workbook, every sheet with its cells, from a place the user may read", {
    when() {
        const location = "dbresource://moqui/database/export/" + ec.user.userId + "/spec-calcgrid.xlsx"
        const exported = call("gridExport", {sourceJson: JSON.stringify(source), format: "xlsx", location}, "post")
        const opened = call("calcOpen", {workbookLocation: location}, "post")
        const sheet = (opened.json.sheets || [])[0] || {}
        const cells = call("calcCells", {roomId: opened.json.roomId, sheet: 0, row: 1, column: 1, width: 4, height: 2})
        const cellTexts = Array.from(cells.json.texts || []).map(text => text.text)
        call("calcClose", {roomId: opened.json.roomId}, "post")
        const elsewhere = call("calcOpen", {workbookLocation: "runtime://conf/MoquiDevConf.xml"}, "post")
        ec.artifactExecution.disableAuthz()
        ec.resource.getLocationReference(location).delete()
        ec.artifactExecution.enableAuthz()
        ec.message.clearErrors()
        return {exported, opened, sheet, cellTexts, elsewhere}
    },
    then({exported, opened, sheet, cellTexts, elsewhere}) {
        !exported.json.errors
        !opened.json.errors
        opened.json.title == "spec-calcgrid.xlsx"
        sheet.data === false
        sheet.rows >= 6
        sheet.frozenRows == 1
        cellTexts.includes("AT_ENTITY")
        String(elsewhere.json.errors).includes("Not available")
        !elsewhere.json.roomId
    },
})
