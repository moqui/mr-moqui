/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The Calc mode walk of plans/IronCalc-DB.md Phase 2, in the browser on the qvt shell (./moq accept tools --spec CalcGrid):
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
