// Tools › SQL's terminal form (screen/Tools/SqlTui.xml): in the terminal the statements are the code component over
// the run form's hidden sql, so Run SQL and Ctrl+S post them. The run itself, read only and with changes confirmed, is
// crates/moqui-host/tests/it/tools_screens.rs (the plain page over HTTP).
// Run: ./moq test tools --spec SqlTui
spec({
    setupSpec() {
        login("john.doe")
    }
})

// the first node of the tree that matches, at any depth
function find(node, match) {
    if (!node || typeof node != "object") return null
    if (!Array.isArray(node) && match(node)) return node
    for (const value of Object.values(node)) {
        const found = find(value, match)
        if (found) return found
    }
    return null
}

feature("the terminal edits the statements in the code component, bound to the run form", {
    when() {
        const page = ec.screen
            .makeTest()
            .rootScreen("component://tools/screen/Tools.xml")
            .renderMode("tui")
            .render("SqlTui", {}, null)
        const output = String(page.output || "")
        ec.message.clearErrors()
        const tree = JSON.parse(output || "null")
        return {
            page,
            output,
            editor: find(tree, node => node.t == "component" && node.id == "SqlText") || {},
            sqlField: find(tree, node => node.t == "field" && node.name == "sql") || {}
        }
    },
    then({page, output, editor, sqlField}) {
        page.status == 200
        page.errorMessages.length == 0
        !/Error rendering|Exception in render/.test(output)
        editor.name == "code"
        editor.props.language == "sql"
        editor.props.form == "SqlOptions"
        editor.props.field == "sql"
        sqlField.w.kind == "hidden"
        output.includes("Run SQL")
    }
})
