// The data grid's transitions (screen/Tools/Data/Grid.xml): an export download only from the user's own place, a save
// only by POST, and the terminal render of an entity's rows (its own tui template). A SQL source in a URL is refused in
// framework/test/client/SecurityScreens.spec.js, over HTTP.
// Opening an entity puts it first in the user's recent entities (plans/archive/Tools-Fancy.md B8, template/ToolsRecent.js).
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

// B8: the rule of template/ToolsRecent.js through the grid's gridColumns transition, which is what opens an entity.
feature("opening an entity in the grid puts it first in the user's recent entities, once, twenty at most", {
    when() {
        const recent = ec.resource.script("component://tools/template/ToolsRecent.js")
        const before = ec.user.getPreference(recent.KEY)
        const open = entityName => {
            const result = tools().render("Data/Grid/gridColumns", {sourceJson: JSON.stringify({type: "entity", entityName})}, null)
            return {status: result.status, text: String(result.output || "")}
        }
        // a full list of other names, so the cap and the order are both seen
        const others = Array.from({length: recent.LIMIT}, (unused, index) => "example.Recent" + index)
        ec.user.setPreference(recent.KEY, JSON.stringify(others))
        const first = open("moqui.basic.Enumeration")
        const afterFirst = recent.read()
        const second = open("moqui.basic.StatusItem")
        const again = open("moqui.basic.Enumeration")
        const afterAgain = recent.read()
        // an entity that does not open is not remembered
        const missing = open("example.NoSuchEntity")
        const afterMissing = recent.read()
        ec.user.setPreference(recent.KEY, before || "")
        return {recent, others, first, afterFirst, second, again, afterAgain, missing, afterMissing}
    },
    then({recent, others, first, afterFirst, second, again, afterAgain, missing, afterMissing}) {
        first.status == 200
        first.text.includes('"columns"')
        afterFirst.length == recent.LIMIT
        afterFirst[0] == "moqui.basic.Enumeration"
        afterFirst[1] == others[0]
        !afterFirst.includes(others[recent.LIMIT - 1])
        second.status == 200
        again.status == 200
        afterAgain.length == recent.LIMIT
        afterAgain[0] == "moqui.basic.Enumeration"
        afterAgain[1] == "moqui.basic.StatusItem"
        afterAgain.filter(name => name === "moqui.basic.Enumeration").length == 1
        missing.text.includes('"errors"')
        afterMissing.join(",") == afterAgain.join(",")
        // the rule itself: no name twice, the new one first, an empty name changes nothing
        recent.put(["a", "b", "a"], "b").join(",") == "b,a"
        recent.put(["a", "b"], "").join(",") == "a,b"
    }
})
