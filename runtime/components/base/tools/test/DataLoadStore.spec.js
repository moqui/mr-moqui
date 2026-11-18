// Tools › Data › Load's storeSelected (screen/Tools/Data/Load.xml): it writes the rows a check found, each posted as
// base64url JSON the holder chooses, so a row is written only where a generic write may be: not an entity
// WebUtilities.isGenericWriteRefused names (a hosted-instance row, a SqlRunToken), nor a UserAccount password field, as
// a value or in the primary key map; the other rows are stored (plans/Security-Review.md SEC-TLS-81).
// Run: ./moq test tools --spec DataLoadStore
const TARGET = "SPEC_DLS_TARGET"
const rowOp = (row) => StringUtilities.base64UrlEncode(JSON.stringify(row))
const account = () => ec.entity.find("moqui.security.UserAccount").condition("userId", TARGET).disableAuthz().one()
const count = (entityName, field, value) => ec.entity.find(entityName).condition(field, value).disableAuthz().count()

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        ec.entity.find("moqui.security.UserAccount").condition("userId", TARGET).deleteAll()
        const row = ec.entity.makeValue("moqui.security.UserAccount")
        row.setFields({ userId: TARGET, username: "spec.dls.target", userFullName: "Spec DLS", currentPassword: "spec-original-hash",
            passwordHashType: "SHA-256" }, true, null, null)
        row.create()
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        ec.entity.find("moqui.security.UserAccount").condition("userId", TARGET).deleteAll()
        ec.artifactExecution.enableAuthz()
    },
})

feature("an ADMIN's posted rows for a password field, a hosted-instance row and a SqlRunToken are refused; an ordinary row is stored", {
    when() {
        login("john.doe")
        ec.message.clearAll()
        const rowOps = [
            rowOp({ entity: "moqui.security.UserAccount", pk: { userId: TARGET }, field: "currentPassword", value: "spec-forged-hash" }),
            rowOp({ entity: "moqui.security.UserAccount", pk: { userId: TARGET, passwordHashType: "NONE" }, field: "userFullName", value: "Spec Smuggled" }),
            rowOp({ entity: "moqui.server.instance.InstanceImage", pk: { instanceImageId: "SPEC_DLS_IMAGE" }, createValues: { authTokenCmd: "touch /tmp/spec-dls" } }),
            rowOp({ entity: "moqui.database.SqlRunToken", pk: { tokenHash: "SPEC_DLS_TOKEN" }, createValues: { userId: "john.doe", textHash: "x" } }),
            rowOp({ entity: "moqui.security.UserAccount", pk: { userId: TARGET }, field: "userFullName", value: "Spec Stored" }),
        ].join(",")
        const res = ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").render("Data/Load/storeSelected", { rowOps }, "post")
        const messages = ec.message.messages.join(" | ")
        const errors = res.errorMessages.join(" ")
        ec.message.clearAll()
        const stored = account()
        return {
            errors, messages, currentPassword: stored.currentPassword, passwordHashType: stored.passwordHashType, userFullName: stored.userFullName,
            images: count("moqui.server.instance.InstanceImage", "instanceImageId", "SPEC_DLS_IMAGE"),
            tokens: count("moqui.database.SqlRunToken", "tokenHash", "SPEC_DLS_TOKEN"),
        }
    },
    then({ errors, messages, currentPassword, passwordHashType, userFullName, images, tokens }) {
        errors == ""
        currentPassword == "spec-original-hash"
        passwordHashType == "SHA-256"
        images == 0
        tokens == 0
        userFullName == "Spec Stored"
        messages.includes("Field currentPassword of moqui.security.UserAccount is not available through this tool")
        messages.includes("Field passwordHashType of moqui.security.UserAccount is not available through this tool")
        messages.includes("Entity moqui.server.instance.InstanceImage is not available through this tool")
        messages.includes("Entity moqui.database.SqlRunToken is not available through this tool")
        messages.includes("Updated 1 fields")
    },
})

feature("storeSelected takes a POST only", {
    when() {
        login("john.doe")
        const rowOps = rowOp({ entity: "moqui.security.UserAccount", pk: { userId: TARGET }, field: "userFullName", value: "Spec Got" })
        const res = ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").render("Data/Load/storeSelected", { rowOps }, "get")
        ec.message.clearAll()
        return { status: res.status, userFullName: account().userFullName }
    },
    then({ status, userFullName }) {
        userFullName != "Spec Got"
        status != 200
    },
})
