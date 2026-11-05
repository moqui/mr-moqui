// The data grid's transitions (screen/Tools/Data/Grid.xml): an export download only from the user's own place, a save
// only by POST, and the terminal render of an entity's rows (its own tui template). A SQL source in a URL is refused in
// framework/test/client/SecurityScreens.spec.js, over HTTP.
// Run: ./moq test tools --spec DataGrid
const tools = () => ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml")

spec({ setupSpec() { login("john.doe") } })

feature("an export download is refused for another user's place and a path with .., and a save by GET", {
    expect() {
        const own = "runtime://tmp/grid-export/" + ec.user.userId + "/"
        tools().render("Data/Grid/gridDownload", { location: "runtime://tmp/grid-export/SOMEONE_ELSE/x/Enumeration.csv" }, null).status == 404
        tools().render("Data/Grid/gridDownload", { location: own + "x/../../SOMEONE_ELSE/Enumeration.csv" }, null).status == 404
        tools().render("Data/Grid/gridDownload", { location: "runtime://conf/MoquiDevConf.xml" }, null).status == 404
        tools().render("Data/Grid/gridSave", { saveEntityName: "moqui.basic.Enumeration", updatesJson: "[]" }, "get").status == 405
    },
})

feature("the terminal shows a page of the entity's rows, each with its record form", {
    expect() {
        const r = ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").renderMode("tui")
            .render("Data/Grid", { entityName: "moqui.basic.Enumeration" }, null)
        const out = String(r.output)
        r.status == 200
        r.errorMessages.length == 0
        out.includes('"GridRows"')
        out.includes("Record?selectedEntity=moqui.basic.Enumeration")
    },
})
