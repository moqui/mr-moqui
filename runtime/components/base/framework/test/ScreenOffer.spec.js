// A screen's own condition (mr-xml-screen.xsd screen; docs/dev/JS-Screen-Scripts.md "The order of a render"): with no
// fail-widgets it says whether the screen is offered under its parent, read in the context the parent renders with (its
// always-actions' appRoot). A screen whose condition is false is left out of its parent's tab bar, and a request that
// names it says it is not offered there, with none of its actions run. The browser's menu data and a condition that
// fails to evaluate are crates/moqui-core screen::tests::menu.
// Run: ./moq test framework --spec ScreenOffer
const ROOT = "component://framework/test/screen/ScreenOffer.xml"

spec({
    setupSpec() { ec.user.internalLoginUser("john.doe") }
})

feature("a screen whose condition is false is left out of its parent's tabs and, asked for by its path, is not offered", {
    when() {
        const out = ec.screen.makeTest().rootScreen(ROOT).renderMode("html").render("Hidden", {}, null)
        const errors = Array.from(out.errorMessages || []).join("\n")
        ec.message.clearErrors()
        return {status: out.status, text: String(out.output || ""), errors}
    },
    then({status, text, errors}) {
        status == 200
        // the parent's tab bar: the other tab, not this one
        text.includes("Shown tab") && text.includes("Also tab")
        !text.includes("Hidden tab")
        // the screen itself: the reason, and none of its actions or widgets
        text.includes("Hidden screen is not offered here.")
        !text.includes("Hidden screen widgets")
        !text.includes("actions ran") && !errors.includes("actions ran")
    }
})
