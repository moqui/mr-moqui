// Server-side rules shared by bookmarked Gallery screens and their transitions.
spec({setupSpec() { login("john.doe") }})

const transition = (path, params, method) => ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml")
    .renderMode("qvt").header("Accept", "application/json").render(path, params, method)

feature("finishing a draft requires the saved Options country and retains the customer on failure", {
    expect() {
        const saved = transition("Wizard/Customer/saveCustomer", {customerName: "Draft Customer"}, "post")
        assert.equal(saved.errorMessages.length, 0)
        const incomplete = transition("Wizard/Review/createCustomer", {}, "post")
        assert.ok(incomplete.errorMessages.join(" ").includes("Choose a country in Options"))
        const options = transition("Wizard/Options/saveOptions", {countryGeoId: "USA"}, "post")
        assert.equal(options.errorMessages.length, 0)
        assert.equal(transition("Wizard/Review/createCustomer", {}, "post").errorMessages.length, 0)
        assert.ok(transition("Wizard/Review/createCustomer", {}, "post").errorMessages.join(" ").includes("Start with the customer"))
    },
})

feature("the calendar returns events from one valid month even when the bookmark is invalid", {
    expect() {
        const events = month => JSON.parse(transition("Calendar/events", month ? {month} : {}, "get").output)
        const current = events().month
        for (const month of ["2026-00", "2026-13", "bad", "2026-12", "2027-01"]) {
            const result = events(month)
            assert.equal(result.month, ["2026-12", "2027-01"].includes(month) ? month : current)
            assert.ok(result.events.length && result.events.every(event => event.start.startsWith(result.month + "-")))
        }
    },
})
