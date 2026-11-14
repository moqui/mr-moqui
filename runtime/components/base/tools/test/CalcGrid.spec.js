// Calc mode of the data grid (screen/Tools/Data/Grid.xml, plans/IronCalc-DB.md Phase 2), over org.moqui.calc.CalcServices:
// an entity's rows as the data sheet of a workbook in a room of the user's, read a rectangle at a time; the document saved
// as a report definition with no row of the data in it, opened again and deleted; another user finds no such room.
// And CalcGrid in the Dashboards app (screen/Dashboards/CalcGrid.xml, template/CalcGrid.js, Phase 2b): how the viewer may use it,
// a new document over an entity, and a profile the viewer does not hold. The access profiles' own rules (the entities
// offered, the rows of the viewer's organizations) are held where a profile is, by mr-mantle's CalcGridSales spec.
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
const dashboardsCall = (transition, parameters, method) => {
    const result = ec.screen.makeTest().rootScreen("component://tools/screen/Dashboards.xml").render("CalcGrid/" + transition, parameters, method || null)
    let json = {}
    try { json = JSON.parse(String(result.output || "")) } catch (e) { json = {} }
    return {status: result.status, json}
}

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

feature("CalcGrid in Dashboards: an administrator has the Tools app's own access and authors, and a profile not held is refused", {
    when() {
        const dashboards = (transition, parameters, method) => {
            const result = ec.screen.makeTest().rootScreen("component://tools/screen/Dashboards.xml").render("CalcGrid/" + transition, parameters, method || null)
            const text = String(result.output || "")
            let json = {}
            try { json = JSON.parse(text) } catch (e) { json = {} }
            return {status: result.status, text, json}
        }
        const access = dashboards("calcAccess", {})
        const entities = dashboards("calcEntities", {term: "basic.Enumeration"})
        const entityNames = Array.from(entities.json.entityNames || [])
        const opened = dashboards("calcOpen", {entityName: "moqui.basic.Enumeration", conditionsJson: JSON.stringify({enumTypeId: "ArtifactType"}), title: "Artifact types"}, "post")
        const cells = dashboards("calcCells", {roomId: opened.json.roomId, sheet: 0, row: 1, column: 1, width: 3, height: 3})
        const cellTexts = Array.from(cells.json.texts || []).map(text => text.text)
        // the room was opened with no profile: it is not read under one
        const underProfile = dashboards("calcCells", {roomId: opened.json.roomId, profileId: "nosuch", sheet: 0, row: 1, column: 1, width: 3, height: 3})
        const notHeld = dashboards("calcOpen", {profileId: "nosuch", entityName: "moqui.basic.Enumeration"}, "post")
        dashboards("calcClose", {roomId: opened.json.roomId}, "post")
        // the screen's terminal render, its list and a sheet, is crates/moqui-host's monitor_tui test's
        ec.message.clearErrors()
        return {access, entityNames, opened, cellTexts, underProfile, notHeld}
    },
    then({access, entityNames, opened, cellTexts, underProfile, notHeld}) {
        access.status == 200
        access.json.tools === true
        access.json.canAuthor === true
        access.json.canShare === true
        access.json.fixedProfile == null
        entityNames.includes("moqui.basic.Enumeration")
        !opened.json.errors
        opened.json.title == "Artifact types"
        opened.json.sheets[0].data === true
        cellTexts.includes("enumId")
        cellTexts.includes("AT_ENTITY")
        String(underProfile.json.errors).includes("Not available")
        String(notHeld.json.errors).includes("Not available")
        !notHeld.json.roomId
    },
})

feature("a range of a saved document is a report dataset: a panel query answers its columns and rows as the viewer reads them", {
    when() {
        const opened = call("calcOpen", {sourceJson: JSON.stringify(source), columnsJson: JSON.stringify(["enumId", "description", "enumTypeId"]),
            sortJson: JSON.stringify([{field: "enumId", dir: "asc"}])}, "post")
        const saved = call("calcSave", {roomId: opened.json.roomId, title: "Frame types", definitionId: "spec-calcgrid-frame"}, "post")
        call("calcClose", {roomId: opened.json.roomId}, "post")
        const run = (parameters) => {
            const out = ec.service.sync().name("org.moqui.report.ReportServices.run#Query").parameters({query: {datasetId: "calcgrid", parameters}}).call() || {}
            const errors = ec.message.hasError() ? ec.message.getErrorsString() : ""
            ec.message.clearErrors()
            return {frame: Array.from(out.frames || [])[0] || {fields: []}, errors}
        }
        const answered = run({definitionId: "spec-calcgrid-frame", range: "Enumeration!A1:C3"})
        const fields = Array.from(answered.frame.fields || [])
        const again = run({definitionId: "spec-calcgrid-frame", range: "Enumeration!A1:A2"})
        const badRange = run({definitionId: "spec-calcgrid-frame", range: "Nowhere!A1:B2"})
        const missing = run({definitionId: "spec-calcgrid-none", range: "A1:B2"})
        // the access chosen is kept only when it is one the user has
        const chosen = dashboardsCall("calcProfile", {profileId: ""}, "post")
        const refusedChoice = dashboardsCall("calcProfile", {profileId: "nosuch"}, "post")
        const kept = ec.user.getPreference("calc.profile")
        call("calcDelete", {definitionId: "spec-calcgrid-frame"}, "post")
        ec.user.setPreference("calc.profile", "")
        return {saved, answered, names: fields.map(field => field.name), types: fields.map(field => field.type),
            first: fields.map(field => Array.from(field.values || [])[0]), rows: fields.length ? Array.from(fields[0].values || []).length : 0,
            againNames: Array.from(again.frame.fields || []).map(field => field.name), badRange, missing, chosen, refusedChoice, kept}
    },
    then({saved, answered, names, types, first, rows, againNames, badRange, missing, chosen, refusedChoice, kept}) {
        !saved.json.errors
        answered.errors == ""
        names.join(",") == "enumId,description,enumTypeId"
        types.join(",") == "string,string,string"
        first[0] == "AT_ENTITY"
        first[2] == "ArtifactType"
        rows == 2
        againNames.join(",") == "enumId"
        badRange.errors.includes("no sheet named Nowhere")
        !!missing.errors
        !chosen.json.errors
        String(refusedChoice.json.errors).includes("Not available")
        kept == "tools"
    },
})

feature("the workbook is edited beside its data sheet: a formula over the data, a style, an undo; a data sheet's cells are not edited", {
    when() {
        const opened = call("calcOpen", {sourceJson: JSON.stringify(source), columnsJson: JSON.stringify(["enumId", "sequenceNum"]),
            sortJson: JSON.stringify([{field: "enumId", dir: "asc"}])}, "post")
        const roomId = opened.json.roomId
        const edit = (ops) => call("calcEdit", {roomId, opsJson: JSON.stringify(ops)}, "post")
        const edited = edit([{op: "addSheet"}, {op: "renameSheet", sheet: 1, name: "Summary"},
            {op: "input", sheet: 1, row: 1, column: 1, value: "=COUNTA(Enumeration[enumId])"},
            {op: "input", sheet: 1, row: 2, column: 1, value: "Artifact types"},
            {op: "style", sheet: 1, row: 2, column: 1, width: 1, height: 1, path: "font.b", value: "true"}])
        const names = Array.from(edited.json.sheets || []).map(sheet => sheet.name)
        const cells = call("calcCells", {roomId, sheet: 1, row: 1, column: 1, width: 2, height: 2})
        const texts = Array.from(cells.json.texts || []).map(text => text.row + ":" + text.column + "=" + text.text)
        const content = call("calcContent", {roomId, sheet: 1, row: 1, column: 1})
        const dataEdit = edit([{op: "input", sheet: 0, row: 2, column: 1, value: "CHANGED"}])
        const undone = edit([{op: "undo"}])
        const afterUndo = call("calcCells", {roomId, sheet: 1, row: 1, column: 1, width: 2, height: 2})
        const bold = Array.from(afterUndo.json.styles || []).some(style => style.style && style.style.font && style.style.font.b === true)
        // values typed on a sheet of their own: the answer names the cells it changed, the formula over them included
        const added = edit([{op: "addSheet"}])
        const sheet = (added.json.sheets || []).length - 1
        const typed = edit([{op: "input", sheet, row: 3, column: 1, value: "7"}, {op: "input", sheet, row: 4, column: 1, value: "=A3*2"}])
        const retyped = edit([{op: "input", sheet, row: 3, column: 1, value: "8"}])
        const notJson = call("calcEdit", {roomId, opsJson: "{"}, "post")
        const byGet = tools().render("Data/Grid/calcEdit", {roomId, opsJson: "[]"}, "get")
        call("calcClose", {roomId}, "post")
        const changed = Array.from(retyped.json.changed || []).map(area => (area.sheet - sheet) + ":" + area.row + ":" + area.column)
        return {opened, edited, names, texts, content, added, typed, changed, dataEdit, undone, bold, notJson, byGet, dataRows: opened.json.sheets[0].dataRows}
    },
    then({opened, edited, names, texts, content, added, typed, changed, dataEdit, undone, bold, notJson, byGet, dataRows}) {
        !opened.json.errors
        !edited.json.errors
        // a sheet added and a style: anything may have changed; values typed: what changed, and the workbook's memory
        edited.json.changed == null
        added.json.changed == null
        Array.isArray(typed.json.changed)
        changed.join(",") == "0:3:1,0:4:1"
        opened.json.memoryBytes > 0
        names.join(",") == "Enumeration,Summary"
        edited.json.canUndo === true
        texts.includes("1:1=" + dataRows)
        texts.includes("2:1=Artifact types")
        content.json.content.startsWith("=COUNTA(Enumeration!$A$2:$A$")
        String(dataEdit.json.errors).includes("is a data sheet")
        // a refused edit still answers the room's state
        dataEdit.json.sheets.length == 2
        !undone.json.errors
        undone.json.canRedo === true
        bold === false
        String(notJson.json.errors).includes("opsJson is not JSON")
        byGet.status == 405
    },
})

feature("a saved document is edited together: one room, edits in its log as typed, each person's own undo, the others' places, discarded edits", {
    when() {
        const opened = call("calcOpen", {sourceJson: JSON.stringify(source), columnsJson: JSON.stringify(["enumId", "description"])}, "post")
        call("calcEdit", {roomId: opened.json.roomId, opsJson: JSON.stringify([{op: "addSheet"}, {op: "renameSheet", sheet: 1, name: "Notes"}])}, "post")
        // saved, the scratch room is the document's room, under a new id
        const saved = call("calcSave", {roomId: opened.json.roomId, title: "Together", definitionId: "spec-calcgrid-together"}, "post")
        const roomId = saved.json.roomId
        const edit = (ops) => call("calcEdit", {roomId, opsJson: JSON.stringify(ops)}, "post")
        const edited = edit([{op: "input", sheet: 1, row: 1, column: 1, value: "=COUNTA(Enumeration[enumId])"}, {op: "input", sheet: 1, row: 2, column: 1, value: "first"}])
        const second = edit([{op: "input", sheet: 1, row: 2, column: 1, value: "second"}])
        // opened again, it is the same room, with the edits
        const again = call("calcOpen", {definitionId: "spec-calcgrid-together"}, "post")
        const text = (cells, row) => (Array.from(cells.json.texts || []).find(cell => cell.row === row && cell.column === 1) || {}).text || ""
        const read = (extra) => call("calcCells", Object.assign({roomId, sheet: 1, row: 1, column: 1, width: 2, height: 3}, extra || {}))
        const afterEdits = read()
        ec.artifactExecution.disableAuthz()
        const rows = Array.from(ec.entity.find("moqui.calc.CalcDocumentEdit").condition("definitionId", "spec-calcgrid-together").orderBy("seq").list())
            .map(row => ({seq: row.seq, kind: row.kind, ops: String(row.opsJson)}))
        ec.artifactExecution.enableAuthz()
        // a view that polls is told nothing changed; two views of the document see each other's place
        const unchanged = read({since: String(afterEdits.json.seq)})
        read({client: "spec-view-1", selectionJson: JSON.stringify({row: 2, column: 1, width: 1, height: 1})})
        const seen = read({client: "spec-view-2"})
        // undo takes back the caller's own last edit, then the one before; redo applies it again
        const undone = edit([{op: "undo"}])
        const afterUndo = read()
        const redone = edit([{op: "redo"}])
        const afterRedo = read()
        // discarded, the document is its saved version again, with nothing unsaved
        const reverted = call("calcRevert", {roomId}, "post")
        const afterRevert = call("calcCells", {roomId: reverted.json.roomId, sheet: 1, row: 1, column: 1, width: 2, height: 3})
        call("calcDelete", {definitionId: "spec-calcgrid-together"}, "post")
        ec.artifactExecution.disableAuthz()
        const left = ec.entity.find("moqui.calc.CalcDocumentEdit").condition("definitionId", "spec-calcgrid-together").count()
        ec.artifactExecution.enableAuthz()
        ec.message.clearErrors()
        return {opened, saved, roomId, edited, second, again, afterEdits: text(afterEdits, 2), count: text(afterEdits, 1), rows, unchanged, seen,
            undone, afterUndo: text(afterUndo, 2), redone, afterRedo: text(afterRedo, 2), reverted, afterRevert: text(afterRevert, 2), left,
            dataRows: opened.json.sheets[0].dataRows}
    },
    then({opened, saved, roomId, edited, second, again, afterEdits, count, rows, unchanged, seen, undone, afterUndo, redone, afterRedo, reverted, afterRevert, left, dataRows}) {
        !saved.json.errors
        String(roomId).startsWith("d")
        roomId != opened.json.roomId
        !edited.json.errors
        edited.json.shared === true
        edited.json.seq == 1
        second.json.seq == 2
        second.json.unsaved == 2
        second.json.canUndo === true
        again.json.roomId == roomId
        afterEdits == "second"
        count == String(dataRows)
        // the log holds the edits as typed: the table's name, and none of its rows
        rows.length == 2
        rows[0].kind == "edit"
        rows[0].ops.includes("Enumeration[enumId]")
        !rows[0].ops.includes("AT_ENTITY")
        unchanged.json.unchanged === true
        unchanged.json.seq == 2
        seen.json.peers.length == 1
        seen.json.peers[0].client == "spec-view-1"
        seen.json.peers[0].area.row == 2
        !undone.json.errors
        undone.json.canRedo === true
        afterUndo == "first"
        !redone.json.errors
        afterRedo == "second"
        !reverted.json.errors
        reverted.json.unsaved == 0
        afterRevert == ""
        left == 0
    },
})

feature("a saved document is scheduled as its workbook: an Excel file with its data and calculated values, in no other format", {
    when() {
        const opened = call("calcOpen", {sourceJson: JSON.stringify(source), columnsJson: JSON.stringify(["enumId", "sequenceNum"])}, "post")
        const roomId = opened.json.roomId
        call("calcEdit", {roomId, opsJson: JSON.stringify([{op: "addSheet"}, {op: "renameSheet", sheet: 1, name: "Summary"},
            {op: "input", sheet: 1, row: 1, column: 1, value: "=COUNTA(Enumeration[enumId])"}])}, "post")
        call("calcSave", {roomId, title: "Scheduled types", definitionId: "spec-calcgrid-scheduled"}, "post")
        call("calcClose", {roomId}, "post")
        const R = "org.moqui.report.ReportServices."
        const location = "runtime://tmp/spec-calcgrid-scheduled.xlsx"
        const schedule = (renderMode) => {
            const out = ec.service.sync().name(R + "create#Schedule").parameters({definitionId: "spec-calcgrid-scheduled", cronExpression: "0 0 7 * * ?",
                renderMode, saveToLocation: location}).call() || {}
            const errors = ec.message.hasError() ? ec.message.getErrorsString() : ""
            ec.message.clearErrors()
            return {id: out.screenScheduledId, errors}
        }
        const asPdf = schedule("pdf")
        const asDefault = schedule(null)
        const sent = ec.service.sync().name(R + "send#Report").parameters({screenScheduledId: asDefault.id}).call() || {}
        const sentErrors = ec.message.hasError() ? ec.message.getErrorsString() : ""
        ec.message.clearErrors()
        const read = ec.service.sync().name("org.moqui.resource.SpreadsheetServices.calculate#Spreadsheet")
            .parameters({location, readCells: ["Summary!A1", "Enumeration!A1"]}).disableAuthz().call() || {}
        const preview = ec.service.sync().name(R + "preview#Report").parameters({definitionId: "spec-calcgrid-scheduled", renderMode: "xlsx"}).call() || {}
        ec.artifactExecution.disableAuthz()
        for (const occurrence of ec.entity.find("moqui.report.ReportOccurrence").condition("screenScheduledId", asDefault.id).list())
            ec.entity.find("moqui.report.ReportRecipientDelivery").condition("occurrenceId", occurrence.occurrenceId).deleteAll()
        for (const entity of ["moqui.report.ReportOccurrence", "moqui.screen.ScreenScheduledLock", "moqui.screen.ScreenScheduled"])
            ec.entity.find(entity).condition("screenScheduledId", asDefault.id).deleteAll()
        ec.resource.getLocationReference(location).delete()
        ec.artifactExecution.enableAuthz()
        call("calcDelete", {definitionId: "spec-calcgrid-scheduled"}, "post")
        ec.message.clearErrors()
        return {asPdf, asDefault, sent, sentErrors, values: read.valueMap || {}, preview, dataRows: opened.json.sheets[0].dataRows}
    },
    then({asPdf, asDefault, sent, sentErrors, values, preview, dataRows}) {
        asPdf.errors.includes("sent as xlsx")
        !asPdf.id
        asDefault.errors == ""
        !!asDefault.id
        sentErrors == ""
        sent.sent === true
        // the file holds the data and the formula's value over it, calculated as the owner reads the data now
        values["Enumeration!A1"] == "enumId"
        values["Summary!A1"] == dataRows
        preview.contentType == "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        String(preview.filename).endsWith(".xlsx")
    },
})

feature("a range of a saved document is added to a dashboard of the viewer's own as a panel that reads it", {
    when() {
        const opened = call("calcOpen", {sourceJson: JSON.stringify(source), columnsJson: JSON.stringify(["enumId", "sequenceNum"])}, "post")
        call("calcSave", {roomId: opened.json.roomId, title: "Panel types", definitionId: "spec-calcgrid-panel"}, "post")
        call("calcClose", {roomId: opened.json.roomId}, "post")
        ec.service.sync().name("org.moqui.report.ReportServices.save#Definition").parameters({definition: {id: "spec-calcgrid-board", type: "dashboard", title: "Spec CalcGrid board", body: {panels: {}}}}).call()
        const listed = Array.from(dashboardsCall("calcDashboards", {}).json.dashboards || []).map(option => option.value)
        const added = dashboardsCall("calcAddPanel", {definitionId: "spec-calcgrid-panel", dashboardId: "spec-calcgrid-board", range: "Enumeration!A1:B5", panelTitle: "Types", visualization: "table"}, "post")
        const again = dashboardsCall("calcAddPanel", {definitionId: "spec-calcgrid-panel", dashboardId: "spec-calcgrid-board", range: "Enumeration!A1:A3", visualization: "bar"}, "post")
        const board = (ec.service.sync().name("org.moqui.report.ReportServices.get#Definition").parameters({definitionId: "spec-calcgrid-board"}).call() || {}).definition || {}
        const panels = board.body.panels || {}
        const panel = panels[added.json.panelId] || {}
        // the panel's query answers the range
        const frames = (ec.service.sync().name("org.moqui.report.ReportServices.run#Query").parameters({query: panel.queries[0]}).call() || {}).frames
        const names = Array.from((Array.from(frames || [])[0] || {}).fields || []).map(field => field.name)
        const notSaved = dashboardsCall("calcAddPanel", {dashboardId: "spec-calcgrid-board", range: "A1:B2"}, "post")
        const notDashboard = dashboardsCall("calcAddPanel", {definitionId: "spec-calcgrid-panel", dashboardId: "spec-calcgrid-panel", range: "A1:B2"}, "post")
        const badKind = dashboardsCall("calcAddPanel", {definitionId: "spec-calcgrid-panel", dashboardId: "spec-calcgrid-board", range: "A1:B2", visualization: "logs"}, "post")
        for (const definitionId of ["spec-calcgrid-board", "spec-calcgrid-panel"]) ec.service.sync().name("org.moqui.report.ReportServices.delete#Definition").parameters({definitionId}).call()
        ec.message.clearErrors()
        return {listed, added, again, panelCount: Object.keys(panels).length, panel, names, notSaved, notDashboard, badKind}
    },
    then({listed, added, again, panelCount, panel, names, notSaved, notDashboard, badKind}) {
        listed.includes("spec-calcgrid-board")
        !added.json.errors
        added.json.panelId == "calc-spec-calcgrid-panel"
        !again.json.errors
        again.json.panelId == "calc-spec-calcgrid-panel-2"
        panelCount == 2
        panel.title == "Types"
        panel.visualization == "table"
        panel.queries[0].datasetId == "calcgrid"
        panel.queries[0].parameters.definitionId == "spec-calcgrid-panel"
        panel.queries[0].parameters.range == "Enumeration!A1:B5"
        names.join(",") == "enumId,sequenceNum"
        String(notSaved.json.errors).includes("Save the document first")
        String(notDashboard.json.errors).includes("Choose a dashboard")
        String(badKind.json.errors).includes("A panel of a range is")
    },
})
