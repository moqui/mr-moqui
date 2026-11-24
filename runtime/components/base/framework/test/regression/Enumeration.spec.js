// Regression spec (moqui test-regression, plans/Regression-Test.md): the life of an enumeration type and one
// enumeration, through Tools > Service > Run, as a user does it. The spec holds no expected values. The runner records
// each answer and each row that changed, and compares them with the other engine's run. The features run in order:
// each one works on the rows of the features before it.

const TYPE = { enumTypeId: "REGRESSION_TYPE" }
const ENUM = { enumId: "REGRESSION_ENUM" }

// The service ran; what it answered is for the comparison.
const ran = (res) => res.ok

feature("create an enumeration type", {
    expect() { ran(app.service("create#moqui.basic.EnumerationType", { ...TYPE, description: "Regression enum type" })) },
})

feature("create an enumeration of the type", {
    expect() {
        ran(app.service("create#moqui.basic.Enumeration",
            { ...ENUM, ...TYPE, description: "Created", sequenceNum: "7" }))
    },
})

feature("a second create of the enumeration is refused and changes no row", {
    // the refusal's status and errors are compared; a redirect would mean the service did not run
    expect() {
        const res = app.service("create#moqui.basic.Enumeration", { ...ENUM, ...TYPE, description: "Must not overwrite" })
        res.status < 300 || res.status >= 400
    },
})

feature("update the enumeration's description", {
    expect() { ran(app.service("update#moqui.basic.Enumeration", { ...ENUM, description: "Updated" })) },
})

feature("store the enumeration", {
    expect() { ran(app.service("store#moqui.basic.Enumeration", { ...ENUM, description: "Stored" })) },
})

feature("delete the enumeration", {
    expect() { ran(app.service("delete#moqui.basic.Enumeration", ENUM)) },
})

feature("delete the enumeration type", {
    expect() { ran(app.service("delete#moqui.basic.EnumerationType", TYPE)) },
})
