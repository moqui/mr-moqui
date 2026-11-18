// Tools › Data's generic writes of a request's rows (plans/Security-Review.md SEC-TLS-81). Load's storeSelected writes
// the rows a check found, each posted as base64url JSON the holder chooses, so a row is written only where a generic
// write may be: not an entity WebUtilities.isGenericWriteRefused names (a hosted-instance row, a SqlRunToken), nor a
// UserAccount password or identity field, as a value or in the primary key map; the other rows are stored. It needs
// all actions on Tools, as load does. Load's load refuses a record that sets an account's password, and Browse's
// update leaves the password and identity fields as they are.
// Run: ./moq test tools --spec DataLoadStore
const TARGET = "SPEC_DLS_TARGET"
const UPDATER = "SPEC_DLS_UPDATER"
const rowOp = (row) => StringUtilities.base64UrlEncode(JSON.stringify(row))
const account = () => ec.entity.find("moqui.security.UserAccount").condition("userId", TARGET).disableAuthz().one()
const count = (entityName, field, value) => ec.entity.find(entityName).condition(field, value).disableAuthz().count()
const make = (entityName, fields) => {
    if (ec.entity.find(entityName).condition(fields).count() > 0) return
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const resetTarget = () => {
    ec.artifactExecution.disableAuthz()
    ec.entity.find("moqui.security.UserAccount").condition("userId", TARGET).deleteAll()
    make("moqui.security.UserAccount", { userId: TARGET, username: "spec.dls.target", userFullName: "Spec DLS",
        emailAddress: "target@spec.invalid", currentPassword: "spec-original-hash", passwordHashType: "SHA-256" })
    ec.artifactExecution.enableAuthz()
}
// a Tools transition as the user, as its form posts it
function post(username, path, parameters) {
    ec.user.internalLoginUser(username)
    ec.message.clearAll()
    const res = ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").render(path, parameters, "post")
    const out = { errors: res.errorMessages.join(" "), messages: ec.message.messages.join(" | ") }
    ec.message.clearAll()
    ec.user.logoutUser()
    return out
}

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        // a holder of view and update, not all, on the Tools app
        make("moqui.security.UserAccount", { userId: UPDATER, username: "spec.dls.updater", userFullName: "Spec DLS Updater" })
        make("moqui.security.UserGroup", { userGroupId: "SPEC_DLS_UPDATERS", description: "Spec Tools update holders" })
        make("moqui.security.UserGroupMember", { userGroupId: "SPEC_DLS_UPDATERS", userId: UPDATER, fromDate: ec.user.nowTimestamp })
        make("moqui.security.ArtifactAuthz", { artifactAuthzId: "SPEC_DLS_TOOLS_UPDATE", userGroupId: "SPEC_DLS_UPDATERS",
            artifactGroupId: "TOOLS_APP", authzTypeEnumId: "AUTHZT_ALLOW", authzActionEnumId: "AUTHZA_UPDATE" })
        make("moqui.security.ArtifactAuthz", { artifactAuthzId: "SPEC_DLS_TOOLS_VIEW", userGroupId: "SPEC_DLS_UPDATERS",
            artifactGroupId: "TOOLS_APP", authzTypeEnumId: "AUTHZT_ALLOW", authzActionEnumId: "AUTHZA_VIEW" })
        ec.artifactExecution.enableAuthz()
    },
    setup() { resetTarget() },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        ec.entity.find("moqui.security.UserAccount").condition("userId", TARGET).deleteAll()
        ec.artifactExecution.enableAuthz()
    },
})

feature("an ADMIN's posted rows for a password field, an identity field, a hosted-instance row and a SqlRunToken are refused; an ordinary row is stored", {
    when() {
        const rowOps = [
            rowOp({ entity: "moqui.security.UserAccount", pk: { userId: TARGET }, field: "currentPassword", value: "spec-forged-hash" }),
            rowOp({ entity: "moqui.security.UserAccount", pk: { userId: TARGET, passwordHashType: "NONE" }, field: "userFullName", value: "Spec Smuggled" }),
            rowOp({ entity: "moqui.security.UserAccount", pk: { userId: TARGET }, field: "emailAddress", value: "taken@spec.invalid" }),
            rowOp({ entity: "moqui.server.instance.InstanceImage", pk: { instanceImageId: "SPEC_DLS_IMAGE" }, createValues: { authTokenCmd: "touch /tmp/spec-dls" } }),
            rowOp({ entity: "moqui.database.SqlRunToken", pk: { tokenHash: "SPEC_DLS_TOKEN" }, createValues: { userId: "john.doe", textHash: "x" } }),
            rowOp({ entity: "moqui.security.UserAccount", pk: { userId: TARGET }, field: "userFullName", value: "Spec Stored" }),
        ].join(",")
        const res = post("john.doe", "Data/Load/storeSelected", { rowOps })
        const stored = account()
        return {
            ...res, currentPassword: stored.currentPassword, passwordHashType: stored.passwordHashType, emailAddress: stored.emailAddress,
            userFullName: stored.userFullName,
            images: count("moqui.server.instance.InstanceImage", "instanceImageId", "SPEC_DLS_IMAGE"),
            tokens: count("moqui.database.SqlRunToken", "tokenHash", "SPEC_DLS_TOKEN"),
        }
    },
    then({ errors, messages, currentPassword, passwordHashType, emailAddress, userFullName, images, tokens }) {
        errors == ""
        currentPassword == "spec-original-hash"
        passwordHashType == "SHA-256"
        emailAddress == "target@spec.invalid"
        images == 0
        tokens == 0
        userFullName == "Spec Stored"
        messages.includes("Field currentPassword of moqui.security.UserAccount is not available through this tool")
        messages.includes("Field passwordHashType of moqui.security.UserAccount is not available through this tool")
        messages.includes("Field emailAddress of moqui.security.UserAccount is not available through this tool")
        messages.includes("Entity moqui.server.instance.InstanceImage is not available through this tool")
        messages.includes("Entity moqui.database.SqlRunToken is not available through this tool")
        messages.includes("Updated 1 fields")
    },
})

// before, storeSelected had no authz-action and so asked update, which this holder was refused as "action update"
feature("storeSelected needs all actions on Tools, as load does", {
    when() {
        const store = post("spec.dls.updater", "Data/Load/storeSelected",
            { rowOps: rowOp({ entity: "moqui.security.UserAccount", pk: { userId: TARGET }, field: "userFullName", value: "Spec Updater Stored" }) })
        return { errors: store.errors, userFullName: account().userFullName }
    },
    then({ errors, userFullName }) {
        errors.includes("SPEC_DLS_UPDATER is not authorized for AT_XML_SCREEN_TRANS [component://tools/screen/Tools/Data/Load.xml/storeSelected] action all")
        userFullName == "Spec DLS"
    },
})

feature("Browse's update writes the other fields of an account but not its password and identity fields", {
    when() {
        const edit = post("john.doe", "Data/Browse/AutoEdit/AutoEditMaster/update", { aen: "moqui.security.UserAccount", userId: TARGET,
            userFullName: "Spec Edited", currentPassword: "spec-forged-hash", emailAddress: "taken@spec.invalid" })
        const stored = account()
        return { edit, userFullName: stored.userFullName, currentPassword: stored.currentPassword, emailAddress: stored.emailAddress }
    },
    then({ edit, userFullName, currentPassword, emailAddress }) {
        edit.errors == ""
        userFullName == "Spec Edited"
        currentPassword == "spec-original-hash"
        emailAddress == "target@spec.invalid"
        edit.messages.includes("Fields currentPassword, emailAddress of moqui.security.UserAccount are not available through this tool")
    },
})

feature("Load's load refuses a record that sets an account's password", {
    when() {
        const forged = post("john.doe", "Data/Load/load", { xmlText:
            `<entity-facade-xml><moqui.security.UserAccount userId="${TARGET}" currentPassword="spec-forged-hash"/></entity-facade-xml>` })
        const forgedPassword = account().currentPassword
        const named = post("john.doe", "Data/Load/load", { xmlText:
            `<entity-facade-xml><moqui.security.UserAccount userId="${TARGET}" userFullName="Spec Loaded"/></entity-facade-xml>` })
        return { forgedText: forged.errors + " " + forged.messages, forgedPassword, named, userFullName: account().userFullName }
    },
    then({ forgedText, forgedPassword, named, userFullName }) {
        forgedPassword == "spec-original-hash"
        forgedText.includes("password fields")
        named.errors == ""
        userFullName == "Spec Loaded"
    },
})
