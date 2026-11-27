// A typed Grid date arrives from gridRows as decimal milliseconds in a string. Formatting and leaving
// its editor must preserve that value, including when datetime-local normalizes away zero seconds.
feature("the Grid displays typed timestamps and preserves their raw values", {
    expect() {
        const page = browser.newPage({user: "john.doe"})
        page.allowProblems(/response: 404 Not Found .*\/apps\/Commons\/Settings\/Account\/userPhoto\?userId=EX_JOHN_DOE$/)
        page.goto("/qapps/tools/Data/Grid?entityName=moqui.example.Example").status == 200
        page.waitFor(() => window.Tabulator && Tabulator.findTable(".m-data-grid")[0]?.getRows().some(row => row.getData().exampleId === "EX_100"))
        const result = page.evaluate(() => {
            const table = Tabulator.findTable(".m-data-grid")[0]
            const row = table.getRows().find(row => row.getData().exampleId === "EX_100")
            const cell = row.getCell("exampleDate")
            const raw = cell.getValue()
            const text = cell.getElement().textContent
            const expected = moqui.l10n.formatTimestamp(Number(raw))
            cell.edit(true)
            cell.getElement().querySelector("input").blur()
            const format = (name, value) => table.getColumn(name).getDefinition().formatter({
                getValue: () => value, getElement: () => document.createElement("div")
            })
            return {text, expected, unchanged: cell.getValue() === raw, undo: table.getHistoryUndoSize(),
                empty: format("exampleDate", null), numericText: format("exampleName", "00123")}
        })
        result.text == result.expected
        result.unchanged
        result.undo == 0
        result.empty == "null"
        result.numericText == "00123"
    }
})

// TOOL-UI-003/004: a filtered Grid counts one row as "1 row", and its Record's Back to Find opens it filtered again.
feature("the Grid counts one row as a row, and Back to Find keeps its filter", {
    expect() {
        const page = browser.newPage({user: "john.doe"})
        page.allowProblems(/response: 404 Not Found .*\/apps\/Commons\/Settings\/Account\/userPhoto\?userId=EX_JOHN_DOE$/)
        page.goto("/qapps/tools/Data/Grid?entityName=moqui.example.Example").status == 200
        page.waitFor(() => window.Tabulator && Tabulator.findTable(".m-data-grid")[0]?.getRows().length > 1)
        // Tabulator filters on the input's key events, which fill does not send
        page.getByLabel("Filter by Example ID").focus()
        page.keyboard.type("=EX_100")
        const count = page.waitFor(() => {
            const rows = Tabulator.findTable(".m-data-grid")[0].getRows()
            const caption = [...document.querySelectorAll(".text-caption")].map(node => node.textContent.trim()).find(text => /\brows?$/.test(text))
            return rows.length === 1 && caption
        })
        count == "1 row"
        page.locator('.m-data-grid .tabulator-cell[tabulator-field="_record"]').first().click()
        page.waitForURL(url => url.includes("/Data/Record?") && url.includes("gridFilters="))
        const find = page.getByRole("link", {name: "Back to Find"})
        find.waitFor()
        String(find.attribute("href")).includes("gridFilters=")
        find.click()
        page.waitForURL(url => url.includes("/Data/Grid?") && url.includes("gridFilters="))
        const back = page.waitFor(() => {
            const grid = window.Tabulator && Tabulator.findTable(".m-data-grid")[0]
            return grid && grid.getRows().length === 1 && {filters: grid.getHeaderFilters().map(item => item.field + ":" + item.value),
                ids: grid.getRows().map(row => row.getData().exampleId)}
        })
        back.filters.join(",") == "exampleId:=EX_100"
        back.ids.join(",") == "EX_100"
        String(page.getByLabel("Filter by Example ID").value()) == "=EX_100"
    }
})
