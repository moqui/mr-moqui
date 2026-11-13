/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The Calc mode walks of plans/IronCalc-DB.md, in the browser on the qvt shell (./moq accept tools --spec CalcGrid):
// an entity's rows in the grid, then as the data sheet of a workbook drawn from the server's cells; the document saved,
// listed, exported as XLSX and deleted; and Records again as it was.
spec({ timeout: 120 })

function gridPage() {
    const page = browser.newPage({ user: "john.doe" })
    page.allowProblems(/response: 404 Not Found .*\/apps\/Commons\/Settings\/Account\/userPhoto\?userId=EX_JOHN_DOE$/)
    return page
}

feature("an entity's rows open in Calc as a data sheet, and the document is saved, listed, exported and deleted", {
    expect() {
        const page = gridPage()
        page.goto("/qapps/tools/Data/Grid?entityName=moqui.basic.Enumeration&enumTypeId=ComparisonOperator").status == 200
        page.waitFor(() => !!(window.Tabulator && document.querySelector(".m-data-grid.tabulator") && document.querySelectorAll(".m-data-grid .tabulator-row").length >= 3))
        const recordRows = page.evaluate(() => {
            // the page's buttons by their label (a Quasar button's icon is a ligature in its text: its text without the icon)
            const label = (b) => Array.from(b.querySelectorAll(".block")).map((part) => part.textContent).join("").trim()
            window.walkButton = (text) => Array.from(document.querySelector(".m-data-grid").closest(".q-pa-sm").querySelectorAll("button")).find((b) => label(b).startsWith(text))
            window.walkTexts = () => Array.from(document.querySelectorAll(".m-calc-grid .tabulator-row")).map((row) => Array.from(row.querySelectorAll(".tabulator-cell")).map((cell) => cell.textContent))
            const count = Tabulator.findTable(".m-data-grid")[0].getRows().length
            window.walkButton("Calc").click()
            return count
        })
        recordRows >= 3
        // the sheet: a row header and lettered columns, the entity's fields in row 1, a record in row 2
        page.waitFor(() => { const rows = window.walkTexts(); return rows.length > 2 && rows[0].includes("enumId") && rows[1].some((text) => text.startsWith("ENTCO_")) })
        const sheet = JSON.parse(page.evaluate(() => JSON.stringify({ rows: window.walkTexts().slice(0, 2),
            headers: Array.from(document.querySelectorAll(".m-calc-grid .tabulator-col-title")).slice(0, 3).map((title) => title.textContent),
            tabs: Array.from(document.querySelectorAll(".m-calc-sheet .q-tab")).map((tab) => tab.textContent.replace("table_rows", "").trim()),
            bold: getComputedStyle(Array.from(document.querySelectorAll(".m-calc-grid .tabulator-row")[0].querySelectorAll(".tabulator-cell")).find((cell) => cell.textContent === "enumId")).fontWeight,
            recordsShown: document.querySelector(".m-data-grid:not(.m-calc-grid)").offsetParent !== null })))
        sheet.rows[0][0] == "1"
        sheet.rows[1][0] == "2"
        sheet.rows[0].includes("description")
        sheet.headers.map((title) => title.trim()).join(",") == ",A,B"
        sheet.tabs.join(",") == "Enumeration"
        Number(sheet.bold) >= 600
        sheet.recordsShown === false

        // save the document, find it in Documents, export it
        page.evaluate(() => { window.walkButton("Save document").click(); return true })
        page.waitFor(() => Array.from(document.querySelectorAll(".q-dialog .text-h6")).some((title) => title.textContent.includes("Save the document")))
        page.evaluate(() => {
            const input = document.querySelector(".q-dialog input")
            input.value = "Walk comparison operators"; input.dispatchEvent(new Event("input", { bubbles: true }))
            Array.from(document.querySelectorAll(".q-dialog button")).find((b) => b.textContent.includes("Save")).click()
            return true
        })
        page.waitFor(() => !document.querySelector(".q-dialog") && document.querySelector(".m-calc-sheet") && window.walkButton("Documents"))
        const listed = JSON.parse(page.evaluate(() => fetch(location.pathname.replace("/qapps/", "/apps/") + "/calcDocuments", { credentials: "same-origin" }).then((response) => response.text())))
        const doc = listed.documents.find((found) => found.title == "Walk comparison operators") || {}
        !!doc.definitionId
        page.evaluate(() => {
            window.downloads = []
            HTMLAnchorElement.prototype.click = function () { window.downloads.push(this.href) }
            window.walkButton("XLSX").click()
            return true
        })
        page.waitFor(() => window.downloads.length === 1)
        const download = JSON.parse(page.evaluate(() => fetch(window.downloads[0], { credentials: "same-origin" }).then((response) => response.arrayBuffer().then((buffer) =>
            JSON.stringify({ status: response.status, type: response.headers.get("content-type"), size: buffer.byteLength, zip: new TextDecoder().decode(buffer.slice(0, 2)) })))))
        download.status == 200
        download.type.includes("spreadsheetml")
        download.zip == "PK"
        download.size > 2000

        // Records again: the grid as it was
        page.evaluate(() => { window.walkButton("Records").click(); return true })
        page.waitFor(() => document.querySelector(".m-data-grid:not(.m-calc-grid)").offsetParent !== null && !document.querySelector(".m-calc-sheet"))
        page.evaluate(() => Tabulator.findTable(".m-data-grid")[0].getRows().length) == recordRows

        // the walk's document is deleted
        const deleted = JSON.parse(page.evaluate((id) => moqui.request(location.pathname.replace("/qapps/", "/apps/") + "/calcDelete", { method: "POST", params: { definitionId: id } }).then((result) => JSON.stringify(result.json || {})), doc.definitionId))
        !deleted.errors
        page.close()
    },
})

// CalcGrid in the Dashboards app (Phase 2b): an author starts a document over an entity, saves it, finds it in the list
// when the page is opened again, and deletes it.
feature("in Dashboards an author starts a document over an entity, saves it, opens it from the list and deletes it", {
    expect() {
        const page = gridPage()
        page.goto("/qapps/dashboards/CalcGrid").status == 200
        page.waitFor(() => Array.from(document.querySelectorAll("button")).some((b) => b.textContent.includes("New")))
        page.evaluate(() => {
            const label = (b) => Array.from(b.querySelectorAll(".block")).map((part) => part.textContent).join("").trim()
            window.walkButton = (text) => Array.from(document.querySelectorAll("button")).find((b) => label(b).startsWith(text))
            window.walkTexts = () => Array.from(document.querySelectorAll(".m-calc-grid .tabulator-row")).map((row) => Array.from(row.querySelectorAll(".tabulator-cell")).map((cell) => cell.textContent))
            window.walkBase = location.pathname.replace("/qapps/", "/apps/")
            return true
        })
        // the entities offered answer a term, and a document over one opens as a sheet
        const offered = page.evaluate(() => fetch(window.walkBase + "/calcEntities?term=basic.Enumeration", { credentials: "same-origin" }).then((response) => response.json()))
        offered.entityNames.includes("moqui.basic.Enumeration")
        page.evaluate(() => { window.walkButton("New").click(); return true })
        page.waitFor(() => !!document.querySelector(".q-dialog input"))
        page.evaluate(() => {
            const input = document.querySelector(".q-dialog .q-select input")
            input.focus(); input.value = "basic.Enumeration"; input.dispatchEvent(new Event("input", { bubbles: true }))
            return true
        })
        page.waitFor(() => Array.from(document.querySelectorAll(".q-menu .q-item")).some((item) => item.textContent.trim() == "moqui.basic.Enumeration"))
        page.evaluate(() => { Array.from(document.querySelectorAll(".q-menu .q-item")).find((item) => item.textContent.trim() == "moqui.basic.Enumeration").click(); return true })
        page.evaluate(() => {
            const title = Array.from(document.querySelectorAll(".q-dialog input")).find((input) => !input.closest(".q-select"))
            title.value = "Walk enumerations"; title.dispatchEvent(new Event("input", { bubbles: true }))
            return true
        })
        page.waitFor(() => { const b = Array.from(document.querySelectorAll(".q-dialog button")).find((b) => b.textContent.includes("Open")); return !!b && !b.disabled })
        page.evaluate(() => { Array.from(document.querySelectorAll(".q-dialog button")).find((b) => b.textContent.includes("Open")).click(); return true })
        page.waitFor(() => { const rows = window.walkTexts(); return rows.length > 2 && rows[0].includes("enumId") })
        // save, then the page again: the document is in the list and opens from it
        page.evaluate(() => { window.walkButton("Save document").click(); return true })
        page.waitFor(() => Array.from(document.querySelectorAll(".q-dialog .text-h6")).some((title) => title.textContent.includes("Save the document")))
        page.evaluate(() => { Array.from(document.querySelectorAll(".q-dialog button")).find((b) => b.textContent.includes("Save")).click(); return true })
        page.waitFor(() => !document.querySelector(".q-dialog") && !!window.walkButton("Delete"))
        const saved = page.evaluate(() => fetch(window.walkBase + "/calcDocuments", { credentials: "same-origin" }).then((response) => response.json()))
        const definitionId = (saved.documents.find((doc) => doc.title == "Walk enumerations") || {}).definitionId
        !!definitionId
        page.goto("/qapps/dashboards/CalcGrid?definitionId=" + encodeURIComponent(definitionId)).status == 200
        page.waitFor(() => { const rows = Array.from(document.querySelectorAll(".m-calc-grid .tabulator-row")); return rows.length > 2 && rows[0].textContent.includes("enumId") })
        const listed = page.evaluate(() => fetch(location.pathname.replace("/qapps/", "/apps/") + "/calcDocuments", { credentials: "same-origin" }).then((response) => response.json()))
        listed.documents.filter((doc) => doc.definitionId == definitionId && doc.title == "Walk enumerations").length == 1
        const deleted = page.evaluate((id) => moqui.request(location.pathname.replace("/qapps/", "/apps/") + "/calcDelete", { method: "POST", params: { definitionId: id } }).then((result) => result.json || {}), definitionId)
        !deleted.errors
        page.close()
    },
})

// Editing (Phase 3): in the Tools grid's Calc mode a sheet is added, a formula over the data sheet is typed in the formula
// bar and shows its value, a cell is made bold from the toolbar, and Undo takes the last edit back.
feature("in Calc a sheet is added, a formula over the data is typed in the bar, a cell is made bold, and Undo takes it back", {
    expect() {
        const page = gridPage()
        page.goto("/qapps/tools/Data/Grid?entityName=moqui.basic.Enumeration&enumTypeId=ComparisonOperator").status == 200
        page.waitFor(() => !!(window.Tabulator && document.querySelector(".m-data-grid.tabulator") && document.querySelectorAll(".m-data-grid .tabulator-row").length >= 3))
        page.evaluate(() => {
            const label = (b) => Array.from(b.querySelectorAll(".block")).map((part) => part.textContent).join("").trim()
            window.walkButton = (text) => Array.from(document.querySelector(".m-data-grid").closest(".q-pa-sm").querySelectorAll("button")).find((b) => label(b).startsWith(text))
            window.walkTexts = () => Array.from(document.querySelectorAll(".m-calc-grid .tabulator-row")).map((row) => Array.from(row.querySelectorAll(".tabulator-cell")).map((cell) => cell.textContent))
            window.walkTabs = () => Array.from(document.querySelectorAll(".m-calc-sheet .q-tab")).map((tab) => tab.textContent.replace("table_rows", "").trim())
            window.walkButton("Calc").click()
            return true
        })
        page.waitFor(() => { const rows = window.walkTexts(); return rows.length > 2 && rows[0].includes("enumId") })
        const dataRows = page.evaluate(() => { window.walkCount = String(window.walkTexts().filter((row) => row.some((text) => text.startsWith("ENTCO_"))).length); return Number(window.walkCount) })
        dataRows >= 3
        // a sheet beside the data sheet
        page.evaluate(() => { document.querySelector('.m-calc-sheet button[aria-label="Add a sheet"]').click(); return true })
        page.waitFor(() => window.walkTabs().length == 2 && window.walkTexts().length > 2 && window.walkTexts()[0].every((text, index) => index == 0 || text == ""))
        // the formula bar: a name is completed, and the formula's value shows in the cell
        page.evaluate(() => {
            const bar = document.querySelector(".m-calc-formula")
            bar.focus(); bar.value = "=COUNTA(Enum"; bar.setSelectionRange(12, 12); bar.dispatchEvent(new Event("input", { bubbles: true }))
            return true
        })
        page.waitFor(() => Array.from(document.querySelectorAll(".m-calc-suggest-item")).some((item) => item.textContent == "Enumeration["))
        page.evaluate(() => {
            const bar = document.querySelector(".m-calc-formula")
            bar.value = "=COUNTA(Enumeration[enumId])"; bar.dispatchEvent(new Event("input", { bubbles: true }))
            bar.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }))
            return true
        })
        page.waitFor(() => window.walkTexts()[0][1] == window.walkCount)
        page.evaluate(() => document.querySelector(".m-calc-formula").value).startsWith("=COUNTA(Enumeration!$A$2:$A$")
        // bold from the toolbar, then Undo
        page.evaluate(() => { document.querySelector('.m-calc-toolbar button[aria-label="Bold"]').click(); return true })
        page.waitFor(() => Number(getComputedStyle(document.querySelectorAll(".m-calc-grid .tabulator-row")[0].querySelectorAll(".tabulator-cell")[1]).fontWeight) >= 600)
        page.evaluate(() => { document.querySelector('.m-calc-toolbar button[aria-label="Undo"]').click(); return true })
        page.waitFor(() => Number(getComputedStyle(document.querySelectorAll(".m-calc-grid .tabulator-row")[0].querySelectorAll(".tabulator-cell")[1]).fontWeight) < 600)
        page.evaluate(() => window.walkTexts()[0][1] == window.walkCount) === true
        page.close()
    },
})

// Editing together: a document open in the Tools grid and in Dashboards › CalcGrid is one room. Each page sees the
// other's name and place, an edit in one shows in the other, and Discard takes everyone's unsaved edits back.
feature("two pages on one document see each other, an edit in one shows in the other, and Discard takes it back in both", {
    expect() {
        const first = gridPage()
        first.goto("/qapps/tools/Data/Grid?entityName=moqui.basic.Enumeration&enumTypeId=ComparisonOperator").status == 200
        first.waitFor(() => !!(window.Tabulator && document.querySelector(".m-data-grid.tabulator") && document.querySelectorAll(".m-data-grid .tabulator-row").length >= 3))
        const helpers = () => {
            const label = (b) => Array.from(b.querySelectorAll(".block")).map((part) => part.textContent).join("").trim()
            window.walkButton = (text) => Array.from(document.querySelectorAll("button")).find((b) => label(b).startsWith(text))
            window.walkTexts = () => Array.from(document.querySelectorAll(".m-calc-grid .tabulator-row")).map((row) => Array.from(row.querySelectorAll(".tabulator-cell")).map((cell) => cell.textContent))
            window.walkTabs = () => Array.from(document.querySelectorAll(".m-calc-sheet .q-tab"))
            window.walkChips = () => Array.from(document.querySelectorAll(".m-calc-sheet .q-chip")).map((chip) => chip.textContent.trim())
            return true
        }
        first.evaluate(helpers)
        first.evaluate(() => { window.walkButton("Calc").click(); return true })
        first.waitFor(() => { const rows = window.walkTexts(); return rows.length > 2 && rows[0].includes("enumId") })
        // a sheet of the author's, then the document saved: its room is the document's now
        first.evaluate(() => { document.querySelector('.m-calc-sheet button[aria-label="Add a sheet"]').click(); return true })
        first.waitFor(() => window.walkTabs().length == 2 && window.walkTexts().length > 2)
        first.evaluate(() => { window.walkButton("Save document").click(); return true })
        first.waitFor(() => Array.from(document.querySelectorAll(".q-dialog .text-h6")).some((title) => title.textContent.includes("Save the document")))
        first.evaluate(() => {
            const input = document.querySelector(".q-dialog input")
            input.value = "Walk together"; input.dispatchEvent(new Event("input", { bubbles: true }))
            Array.from(document.querySelectorAll(".q-dialog button")).find((b) => b.textContent.includes("Save")).click()
            return true
        })
        first.waitFor(() => !document.querySelector(".q-dialog") && !!document.querySelector(".m-calc-sheet"))
        const listed = first.evaluate(() => fetch(location.pathname.replace("/qapps/", "/apps/") + "/calcDocuments", { credentials: "same-origin" }).then((response) => response.json()))
        const definitionId = (listed.documents.find((doc) => doc.title == "Walk together") || {}).definitionId
        !!definitionId
        // the second page opens it in Dashboards, on the author's sheet, and each sees the other
        const second = gridPage()
        second.goto("/qapps/dashboards/CalcGrid?definitionId=" + encodeURIComponent(definitionId)).status == 200
        second.waitFor(() => !!document.querySelector(".m-calc-sheet .q-tab"))
        second.evaluate(helpers)
        second.evaluate(() => { window.walkTabs()[1].click(); return true })
        second.waitFor(() => window.walkChips().length == 1)
        first.waitFor(() => window.walkChips().length == 1)
        // the first page's place on the author's sheet shows as an outline in the second
        first.evaluate(() => { window.walkTabs()[1].click(); return true })
        first.waitFor(() => window.walkTexts().length > 2 && window.walkTexts()[0].every((text, index) => index == 0 || text == ""))
        first.evaluate(() => {
            const cell = document.querySelectorAll(".m-calc-grid .tabulator-row")[1].querySelectorAll(".tabulator-cell")[2]
            for (const type of ["mousedown", "mouseup", "click"]) cell.dispatchEvent(new MouseEvent(type, { bubbles: true, button: 0 }))
            return true
        })
        second.waitFor(() => (document.querySelectorAll(".m-calc-grid .tabulator-row")[1].querySelectorAll(".tabulator-cell")[2].style.boxShadow || "").includes("inset"))
        // an edit in the first shows in the second, with the unsaved count
        first.evaluate(() => {
            const bar = document.querySelector(".m-calc-formula")
            bar.focus(); bar.value = "together"; bar.dispatchEvent(new Event("input", { bubbles: true }))
            bar.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }))
            return true
        })
        first.waitFor(() => window.walkTexts().some((row) => row.includes("together")))
        second.waitFor(() => window.walkTexts().some((row) => row.includes("together")) && document.querySelector(".m-calc-sheet").textContent.includes("1 unsaved edit"))
        // Discard in the second takes it back in both
        second.evaluate(() => { window.walkButton("Discard").click(); return true })
        second.waitFor(() => Array.from(document.querySelectorAll(".q-dialog button")).some((b) => b.textContent.includes("OK")))
        second.evaluate(() => { Array.from(document.querySelectorAll(".q-dialog button")).find((b) => b.textContent.includes("OK")).click(); return true })
        second.waitFor(() => !window.walkTexts().some((row) => row.includes("together")))
        first.waitFor(() => !!document.querySelector(".m-calc-sheet") && window.walkTexts().length > 2 && !window.walkTexts().some((row) => row.includes("together")))
        const deleted = first.evaluate((id) => moqui.request(location.pathname.replace("/qapps/", "/apps/") + "/calcDelete", { method: "POST", params: { definitionId: id } }).then((result) => result.json || {}), definitionId)
        !deleted.errors
        second.close()
        first.close()
    },
})
