/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Convert to site extension (plans/design-dev-aide.md 7.2, question 16; src/dbform.rs): Customizations lists the DbForms
// that change a screen form, and the conversion refuses a DbForm it would not keep equivalent: one with responses (a
// survey) and one that modifies no screen form (a form extends it). The conversion itself, its review, digest, the
// written screen-extend and the deleted rows on a scratch site component, is in the host's integration test
// (crates/moqui-host/tests/it/devaide.rs, a_db_form_converts_to_a_site_extension).
// Run: ./moq test dev-aide --spec DbFormConvert
const call = (name, parameters) => {
    try {
        const out = ec.service.sync().name("org.moqui.devaide.ArtifactServices." + name).parameters(parameters || {}).call() || {}
        const errors = ec.message.getErrorsString()
        ec.message.clearErrors()
        return {out, errors}
    } catch (error) {
        ec.message.clearErrors()
        return {out: {}, errors: String(error.message)}
    }
}
const SCREEN = "component://example/screen/Gallery/ExampleDetail.xml"
const FORMS = ["SPEC_DBF_CHANGE", "SPEC_DBF_SURVEY", "SPEC_DBF_PLAIN"]

function asSystem(body) {
    ec.artifactExecution.disableAuthz()
    try {
        return body()
    } finally {
        ec.artifactExecution.enableAuthz()
    }
}
function removeFixtures() {
    asSystem(() => {
        for (const entity of ["FormResponse", "DbFormField", "DbFormUserGroup", "DbForm"]) {
            ec.entity.find("moqui.screen.form." + entity).condition("formId", "in", FORMS).deleteAll()
        }
    })
}

spec({
    setupSpec() {
        login("john.doe")
        removeFixtures()
        asSystem(() => {
            const make = (entity, fields) => { const row = ec.entity.makeValue("moqui.screen.form." + entity); row.setFields(fields, true, null, null); row.create() }
            make("DbForm", {formId: "SPEC_DBF_CHANGE", description: "Spec change", modifyXmlScreenForm: SCREEN + "#ExampleForm"})
            make("DbFormField", {formId: "SPEC_DBF_CHANGE", fieldName: "specNote", title: "Spec Note", fieldTypeEnumId: "DBFFT_display"})
            make("DbFormUserGroup", {formId: "SPEC_DBF_CHANGE", userGroupId: "ADMIN"})
            make("DbForm", {formId: "SPEC_DBF_SURVEY", modifyXmlScreenForm: SCREEN + "#ExampleForm"})
            make("FormResponse", {formResponseId: "SPEC_DBF_RESPONSE", formId: "SPEC_DBF_SURVEY", responseDate: new Date()})
            make("DbForm", {formId: "SPEC_DBF_PLAIN", description: "Extended by a form, not modifying one"})
        })
    },
    cleanupSpec() {
        removeFixtures()
    },
})

feature("the DbForms that change a screen form are listed, with their groups and fields", {
    expect() {
        const listed = call("get#DbFormChanges")
        const rows = Array.from(listed.out.dbForms || [])
        const change = rows.find((row) => row.formId == "SPEC_DBF_CHANGE")
        const plainListed = rows.filter((row) => row.formId == "SPEC_DBF_PLAIN").length
        listed.errors == ""
        change.formName == "ExampleForm"
        change.screenLocation == SCREEN
        change.userGroups == "ADMIN"
        change.fieldCount == 1
        plainListed == 0
    },
})

feature("Customizations lists a DbForm's change with Convert", {
    expect() {
        const str = ec.screen.makeTest().rootScreen("component://dev-aide/screen/DevAide.xml").renderMode("tui")
            .render("Artifacts/Customizations", {}, null)
        str.status == 200
        str.errorMessages.length == 0
        str.output.includes("SPEC_DBF_CHANGE")
        str.output.includes("Convert")
    },
})

feature("a DbForm with responses, or one that changes no screen form, stays a DbForm", {
    expect() {
        const survey = call("get#DbFormConversion", {formId: "SPEC_DBF_SURVEY"})
        const plain = call("get#DbFormConversion", {formId: "SPEC_DBF_PLAIN"})
        const converted = call("convert#DbForm", {formId: "SPEC_DBF_SURVEY", digest: "any"})
        survey.errors.includes("has responses")
        plain.errors.includes("modifies no screen form")
        converted.errors.includes("has responses")
        asSystem(() => ec.entity.find("moqui.screen.form.DbForm").condition("formId", "SPEC_DBF_SURVEY").count()) == 1
    },
})
