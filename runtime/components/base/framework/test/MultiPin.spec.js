// A posted _isMulti=true with <name>_0 rows (plans/Security-Review.md SEC-RTE-81): a transition's service-call shortcut is a
// multi call only when it says multi="parameter" or a multi form of its screen posts to it, so on a screen that pins the
// record to the user (always-actions, as My Account pins userId) a post with testId_0 does not reach another record; and in
// a multi call a transition makes a posted row value is taken only for a field of the posting form and never for a name
// the screen pinned (decided from the screen's definitions, so repeating the pinned value as a plain parameter does not
// help), while the rows of a real multi form-list still each save. The rules themselves are crates/moqui-core screen::definition and service::inline_tests.
// Run: ./moq test framework --spec MultiPin
const PINNED = "component://framework/test/screen/MultiPin.xml"
const ROWS = "component://framework/test/screen/MultiRows.xml"

// a post as the user, with authorization off for the test entity (an app's artifact groups would give it)
function post(root, path, parameters) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        const out = ec.screen.makeTest().rootScreen(root).render(path, parameters, "post")
        return {status: out.status, errors: Array.from(out.errorMessages || []).join("\n")}
    } finally {
        ec.message.clearErrors()
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function row(testId) {
    return ec.entity.find("moqui.test.TestEntity").condition("testId", testId).useCache(false).disableAuthz().one()
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        try {
            for (const testId of ["MP_OTHER", "MP_john.doe", "MR0", "MR1"]) row(testId)?.delete()
            ec.entity.makeValue("moqui.test.TestEntity").setAll({testId: "MP_OTHER", testMedium: "theirs"}).create()
        } finally { ec.artifactExecution.enableAuthz() }
    }
})

feature("a post with _isMulti and another record's id as a row is one call on the user's own record", {
    when() {
        return post(PINNED, "updateMine", {testMedium: "mine", _isMulti: "true", testId_0: "MP_OTHER", testMedium_0: "stolen"})
    },
    then({status, errors}) {
        status < 400
        errors === ""
        row("MP_john.doe")?.testMedium === "mine"
        row("MP_OTHER").testMedium === "theirs"
    }
})

feature("a multi form-list's rows each save; a row value does not replace one the screen pinned, though the post repeats the pinned value, nor reach a field the form has not", {
    when() {
        return post(ROWS, "saveRows", {_isMulti: "true", value: "pinned", testId_0: "MR0", testMedium_0: "one", value_0: "posted",
            testIndicator_0: "Y", testId_1: "MR1", testMedium_1: "two"})
    },
    then({status, errors}) {
        status < 400
        errors === ""
        row("MR0")?.testMedium === "one"
        row("MR1")?.testMedium === "two"
        row("MR0").value === "pinned"
        row("MR1").value === "pinned"
        row("MR0").testIndicator == null
    }
})
