// Changes 2 and 3: shipped diagnostic screens use the supported renderer facade.
spec({setupSpec() { login("john.doe") }})

feature("the site map renders the authorized hierarchy", {
    when() {
        const result = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
            .renderMode("html").render("apps/ScreenTree", {showAll: "true"}, null)
        return {errors: result.errorMessages, output: String(result.output || "")}
    },
    then({errors, output}) {
        errors.length === 0
        output.includes("Site Map")
        output.includes('class="ScreenTreeNested"')
        output.includes("/apps/tools")
        !output.includes("Error rendering")
    },
})

feature("the echo diagnostic reads the remaining path from renderer URL metadata", {
    when() {
        const result = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
            .renderMode("html").render("echopath/alpha/beta", {}, null)
        return {errors: result.errorMessages, output: String(result.output || "")}
    },
    then({errors, output}) {
        errors.length === 0
        output.includes("alpha")
        output.includes("beta")
        output.includes("Extra path:")
    },
})

feature("the nested site map initializes its path when opened directly", {
    when() {
        const result = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
            .renderMode("html").render("apps/ScreenTree/ScreenTreeNested", {}, null)
        return {errors: result.errorMessages, output: String(result.output || "")}
    },
    then({errors, output}) {
        errors.length === 0
        output.includes('class="ScreenTreeNested"')
        !output.includes("Error rendering")
    },
})

feature("the site map retains branch containers in its QVT markup", {
    when() {
        const result = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
            .renderMode("qvt").render("apps/ScreenTree", {lastStandalone: "-2"}, null)
        return {errors: result.errorMessages, output: String(result.output || "")}
    },
    then({errors, output}) {
        errors.length === 0
        output.includes('class="ScreenTreeNested"')
        !output.includes("Error rendering")
    },
})
