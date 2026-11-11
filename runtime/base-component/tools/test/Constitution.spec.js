// The instance's Constitution (docs/ai/Constitution.md; org.moqui.admin.ConstitutionServices): a store needs the hash the
// document was read at and the sealed ADMIN_CONSTITUTION_GLOBAL permission, and leaves the file as it was when refused;
// customize, edit and reset move the global document between the shipped default and the instance's own; the trusted
// reference origins are exact https origins; System › Constitution › Rules renders once in qvt and once in tui (it has a
// template of each). Organization documents are Mantle's (mr-mantle test/party/Constitution.spec.js).
// Run: ./moq test tools --spec Constitution
const ADMIN = "org.moqui.admin.ConstitutionServices"
// a user in no group, the spec's own: the framework ships no such user (john.doe is in ADMIN)
const OUTSIDER = "spec.constitution.outsider"
const OUTSIDER_ID = "SPEC_CONSTITUTION_OUTSIDER"
const systemRoot = "component://tools/screen/System.xml"

function call(name, params) {
    const out = ec.service.sync().name(name).parameters(params || {}).call() || {}
    const errors = ec.message.getErrorsString()
    ec.message.clearErrors()
    return Object.assign({}, out, {errors})
}
function render(path, mode) {
    let test = ec.screen.makeTest().rootScreen(systemRoot).renderMode(mode)
    if (mode === "qvt") test = test.withStaticData()
    const str = test.render(path, {lastStandalone: "-2"}, null)
    const output = String(str.output || "")
    ec.message.clearErrors()
    return {errors: str.errorMessages.join(" | "), output}
}
const global = () => call(`${ADMIN}.get#Constitution`, {scope: "global", includeDocument: true, locale: "en"})

spec({
    stepwise: true,
    setupSpec() {
        login("john.doe")
        ec.artifactExecution.disableAuthz()
        const row = ec.entity.makeValue("moqui.security.UserAccount")
        row.setFields({userId: OUTSIDER_ID, username: OUTSIDER, userFullName: "Spec Constitution Outsider"}, true, null, null)
        row.create()
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        ec.entity.find("moqui.security.UserAccount").condition("userId", OUTSIDER_ID).deleteAll()
        ec.artifactExecution.enableAuthz()
    },
    cleanup() { login("john.doe") },
})

feature("a store refuses a stale hash and a user without the sealed permission; the shipped document stays", {
    when() {
        const before = global()
        const retitled = call(`${ADMIN}.edit#ConstitutionDocument`, {documentText: before.documentText, operation: "setDocumentText",
            locale: "en", title: "Stale Rules"}).documentText
        const stale = call(`${ADMIN}.store#Constitution`, {documentText: retitled, expectedHash: "0".repeat(64)})
        ec.user.internalLoginUser(OUTSIDER)
        const outsider = call(`${ADMIN}.store#Constitution`, {documentText: retitled, expectedHash: before.globalHash})
        const outsiderReset = call(`${ADMIN}.create#ConstitutionCustomization`, {})
        logout(); login("john.doe")
        return {before, stale, outsider, outsiderReset, after: global()}
    },
    then({before, stale, outsider, outsiderReset, after}) {
        before.source == "shipped" && before.customized == "N" && before.canEdit == "Y"
        before.ruleCount == 13 && before.title == "Constitution"
        stale.errors.includes("changed since it was read")
        outsider.errors.includes("ADMIN_CONSTITUTION_GLOBAL")
        outsiderReset.errors.includes("ADMIN_CONSTITUTION_GLOBAL")
        after.source == "shipped" && after.customized == "N" && after.contentHash == before.contentHash
    }
})

feature("customize, edit and reset move the global document between the instance's own and the shipped one", {
    when() {
        const customized = call(`${ADMIN}.create#ConstitutionCustomization`, {})
        const current = global()
        const added = call(`${ADMIN}.edit#ConstitutionDocument`, {documentText: current.documentText, operation: "addRule",
            title: "Leave the place better", description: "Clean up after yourself.", instructions: "Tidy what you touch."})
        const stored = call(`${ADMIN}.store#Constitution`, {documentText: added.documentText, expectedHash: current.globalHash})
        const edited = global()
        const badUrl = call(`${ADMIN}.edit#ConstitutionDocument`, {documentText: edited.documentText, operation: "addReference",
            ruleId: "C14", title: "Elsewhere", url: "https://untrusted.example/page"})
        const refused = call(`${ADMIN}.store#Constitution`, {documentText: badUrl.documentText, expectedHash: edited.globalHash})
        const preview = call(`${ADMIN}.preview#ConstitutionPrompt`, {scope: "global", locale: "es_MX"})
        const reset = call(`${ADMIN}.reset#Constitution`, {expectedHash: global().globalHash})
        return {customized, added, stored, edited, refused, preview, reset, after: global()}
    },
    then({customized, added, stored, edited, refused, preview, reset, after}) {
        customized.errors == "" && added.newId == "C14" && stored.errors == ""
        edited.source == "instance" && edited.customized == "Y" && edited.ruleCount == 14
        edited.ruleList[13].ruleId == "C14" && edited.ruleList[13].title == "Leave the place better"
        refused.errors.includes("untrusted.example is not a trusted reference origin")
        preview.locale == "es_MX" && preview.promptText.includes("C14 — Leave the place better") && preview.fallbackList.length == 2
        preview.promptText.startsWith("## Constitución")
        reset.errors == ""
        after.source == "shipped" && after.customized == "N" && after.ruleCount == 13
    }
})

feature("the trusted origins take exact https origins with the revision they were read at", {
    when() {
        const policy = call(`${ADMIN}.get#ReferenceOrigins`, {})
        const wildcard = call(`${ADMIN}.store#ReferenceOrigins`, {origins: ["https://*.example.org"], expectedRevision: policy.revision})
        const added = call(`${ADMIN}.store#ReferenceOrigins`, {origins: policy.originList.concat(["https://Example.org/"]), expectedRevision: policy.revision})
        const stale = call(`${ADMIN}.store#ReferenceOrigins`, {origins: [], expectedRevision: policy.revision})
        const now = call(`${ADMIN}.get#ReferenceOrigins`, {})
        const restored = call(`${ADMIN}.store#ReferenceOrigins`, {origins: policy.originList, expectedRevision: now.revision})
        return {policy, wildcard, added, stale, now, restored}
    },
    then({policy, wildcard, added, stale, now, restored}) {
        policy.originList.length == 1 && policy.originList[0] == "https://grok-the-rock.org"
        wildcard.errors.includes("wildcards are not allowed")
        added.errors == "" && now.originList.includes("https://example.org")
        stale.errors.includes("changed since they were read")
        restored.errors == ""
    }
})

feature("System › Constitution › Rules renders the rules in qvt and in tui", {
    when() { return {qvt: render("Constitution/Rules", "qvt"), tui: render("Constitution/Rules", "tui")} },
    then({qvt, tui}) {
        qvt.errors == "" && tui.errors == ""
        qvt.output.includes("m-constitution-view") && qvt.output.includes("C13")
        tui.output.includes("C01 — ")
    }
})
