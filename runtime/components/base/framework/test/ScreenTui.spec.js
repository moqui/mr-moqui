// The tui render mode's document for widgets the terminal once drew wrong (TUI review 2026-11-15): a list link whose text
// is a localized template reads each row's values, a skip-header list says so, a section-iterate in a list cell is its
// entries, the show-total row is marked and holds only the total, encode="false" text is plain text (and an &nbsp;
// spacer is left out), an html-view with HTML draws the HTML, and a board's expression column-order is expanded.
// One render, with every check on it. Run: ./moq test framework --spec ScreenTui
const ROOT = "component://framework/test/screen/ScreenTui.xml"
const TEMPLATE = "TuiSpecRowTemplate"

// the nodes of a tui document, wherever they are
function nodes(value, out = []) {
    if (Array.isArray(value)) value.forEach((item) => nodes(item, out))
    else if (value && typeof value === "object") {
        if (value.t) out.push(value)
        Object.values(value).forEach((item) => nodes(item, out))
    }
    return out
}
function message() { return ec.entity.find("moqui.basic.LocalizedMessage").condition({original: TEMPLATE, locale: "default"}) }

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        message().deleteAll()
        ec.entity.makeValue("moqui.basic.LocalizedMessage")
            .setAll({original: TEMPLATE, locale: "default", localized: "${itemId}: ${itemName}"}).create()
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        message().deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature("the tui document of list links, headers, cells, totals, markup and a board", {
    when() {
        const out = ec.screen.makeTest().rootScreen(ROOT).renderMode("tui").render("", {lastStandalone: "true"}, null)
        const errors = Array.from(out.errorMessages || []).join("\n")
        ec.message.clearErrors()
        const text = String(out.output || "")
        const all = nodes(JSON.parse(text))
        const list = (name) => all.find((node) => node.t === "formList" && node.name === name)
        return {status: out.status, errors, text, all, items: list("ItemList"), bare: list("BareList"), board: list("StageBoard")}
    },
    then({status, errors, text, all, items, bare, board}) {
        status == 200 && errors == ""
        // the link text expands the localized template on each row, not the first row's value on every row
        items.rows[0].cells[0].text == "I1: Alpha"
        items.rows[1].cells[0].text == "I2: Beta"
        // a section-iterate cell holds its entries
        JSON.stringify(items.rows[0].cells[1]).includes("#red") && JSON.stringify(items.rows[0].cells[1]).includes("#blue")
        JSON.stringify(items.rows[1].cells[1]).includes("#green")
        // the show-total row is marked, holds the total and nothing in the other columns, and is not counted
        items.rows.length == 3
        items.rows[2].total == true
        items.rows[2].cells[0] == "" && items.rows[2].cells[1] == ""
        String(items.rows[2].cells[2]) == "5"
        items.paging.count == 2
        items.skipHeader == null
        // skip-header
        bare.skipHeader == true
        // encode="false": plain text, and the &nbsp; spacer left out
        all.some((node) => node.t === "text" && node.text === "Sales Orders & Quotes" && node.style === "h3")
        !text.includes("&lt;strong") && !text.includes("<strong>") && !text.includes("&amp;amp;")
        !all.some((node) => node.t === "text" && node.style === "div")
        all.some((node) => node.t === "link" && node.text === "Customer Requests")
        // an html-view with HTML draws its blocks, its text not taking its place
        const html = all.find((node) => node.t === "html" && node.blocks.some((block) => block.text === "First paragraph"))
        html.blocks.some((block) => block.kind === "quote" && block.text === "A quote")
        !text.includes("Plain words only")
        // the board's column-order is the expanded expression
        board.views.board.groups.map((group) => group.value).join() == "Open,Won,Lost"
    }
})
