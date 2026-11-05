/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The data grid walk of plans/Tools-Database.md §15 step 5, in the browser on the qvt shell (./moq accept tools --spec
// DataGrid): the grid on an entity's rows, an edit, a pasted range, undo, a save that the next read shows, and an export
// downloaded from the user's own place.
spec({ timeout: 120 })

function gridPage() {
    const page = browser.newPage({ user: "john.doe" })
    page.allowProblems(/response: 404 Not Found .*\/apps\/Commons\/Settings\/Account\/userPhoto\?userId=EX_JOHN_DOE$/)
    return page
}

feature("the grid edits, pastes a range, undoes, saves and exports", {
    expect() {
        const page = gridPage()
        page.goto("/qapps/tools/Data/Grid?entityName=moqui.basic.Enumeration&enumTypeId=ComparisonOperator").status == 200
        page.waitFor(() => !!(window.Tabulator && document.querySelector(".m-data-grid.tabulator") && document.querySelectorAll(".m-data-grid .tabulator-row").length >= 3))
        // an edit of the first row's description, then a range of the next two pasted over
        const stamp = page.evaluate(() => {
            const stamp = String(Date.now())
            // the page's buttons by their label
            // (a Quasar button's icon is a ligature in its text: its text without the icon)
            const label = (b) => Array.from(b.querySelectorAll(".block")).map((part) => part.textContent).join("").trim()
            window.walkButton = (text) => Array.from(document.querySelectorAll(".m-data-grid")[0].closest(".q-pa-sm").querySelectorAll("button")).find((b) => label(b).startsWith(text))
            const table = Tabulator.findTable(".m-data-grid")[0]
            const rows = table.getRows()
            window.walk = { first: rows[0].getData().enumId, firstBefore: rows[0].getData().description, second: rows[1].getData().enumId, third: rows[2].getData().enumId,
                secondBefore: rows[1].getData().description, thirdBefore: rows[2].getData().description, stamp }
            rows[0].getCell("description").setValue("Walk " + stamp)
            table.addRange(rows[1].getCell("description"), rows[2].getCell("description"))
            return stamp
        })
        // a range takes its bounds a moment after it is added; then the paste as the browser hands it to the clipboard module
        page.waitFor(() => Tabulator.findTable(".m-data-grid")[0].getRanges()[0].getCells().flat().length === 2)
        page.evaluate(() => {
            const table = Tabulator.findTable(".m-data-grid")[0]
            table.modules.clipboard.paste({ target: table.element.querySelector(".tabulator-tableholder"), clipboardData: { getData: () => "Pasted 1\nPasted 2" }, preventDefault() {} })
            return true
        })
        page.waitFor(() => { const b = window.walkButton("Save"); return !!b && b.textContent.includes("(3)") })
        // undo takes the paste back in one step; the edit stays
        page.evaluate(() => { window.walkButton("Undo").click(); return true })
        page.waitFor(() => { const b = window.walkButton("Save"); return !!b && b.textContent.includes("(1)") })
        page.evaluate(() => { const rows = Tabulator.findTable(".m-data-grid")[0].getRows(); return rows[1].getData().description === window.walk.secondBefore && rows[2].getData().description === window.walk.thirdBefore }) === true
        // save: the rows are read again with the edit
        page.evaluate(() => { window.walkButton("Save").click(); return true })
        page.waitFor(() => { const table = Tabulator.findTable(".m-data-grid")[0]; const row = table.getRows().find((r) => r.getData().enumId === window.walk.first)
            const b = window.walkButton("Save")
            return !!row && row.getData().description === "Walk " + window.walk.stamp && !!b && !b.textContent.includes("(") })
        // export as CSV: the download link of the user's own place answers with the rows
        page.evaluate(() => {
            window.downloads = []
            HTMLAnchorElement.prototype.click = function () { window.downloads.push(this.href) }
            window.walkButton("Export").click()
            return true
        })
        page.waitFor(() => Array.from(document.querySelectorAll(".q-item")).some((item) => item.textContent.includes("CSV download")))
        page.evaluate(() => { Array.from(document.querySelectorAll(".q-item")).find((item) => item.textContent.includes("CSV download")).click(); return true })
        page.waitFor(() => window.downloads.length === 1)
        const answer = page.evaluate(() => fetch(window.downloads[0], { credentials: "same-origin" }).then((response) => response.text().then((text) => JSON.stringify({ status: response.status, type: response.headers.get("content-type"), text }))))
        const download = JSON.parse(answer)
        download.status == 200
        download.type.startsWith("text/csv")
        download.text.includes("enumId") && download.text.includes("Walk " + stamp)
        // the description the walk changed is put back, through the grid
        page.evaluate(() => {
            const table = Tabulator.findTable(".m-data-grid")[0]
            table.getRows().find((r) => r.getData().enumId === window.walk.first).getCell("description").setValue(window.walk.firstBefore)
            return true
        })
        page.waitFor(() => { const b = window.walkButton("Save"); return !!b && b.textContent.includes("(1)") })
        page.evaluate(() => { window.walkButton("Save").click(); return true })
        page.waitFor(() => { const row = Tabulator.findTable(".m-data-grid")[0].getRows().find((r) => r.getData().enumId === window.walk.first)
            const b = window.walkButton("Save")
            return !!row && row.getData().description === window.walk.firstBefore && !!b && !b.textContent.includes("(") })
    },
})
