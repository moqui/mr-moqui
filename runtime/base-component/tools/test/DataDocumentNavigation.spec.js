// Change 1: the section entry must resolve after the legacy Search screen was removed.
spec({ setupSpec() { login("john.doe") } })

feature("data document section opens the existing editor", {
    when() {
        const result = ec.screen.makeTest().rootScreen("component://tools/screen/System.xml")
            .renderMode("html").render("DataDocument", {}, null)
        return {errors: result.errorMessages, output: String(result.output || "")}
    },
    then({errors, output}) {
        errors.length === 0
        output.includes("Create Data Document")
        output.includes("FindDataDocument")
    },
})
