// System › Instance (framework/service/org/moqui/dev/InstanceServices.xml): hosted instances run an image's
// authTokenCmd, start containers and make databases, so every InstanceServices service asks the sealed INSTANCE_ADMIN,
// which an ADMIN outside ADMIN_ADV does not have; and the configuration rows that hold a command, a script or a
// service name are not written through the generic tools (plans/Security-Review.md SEC-TLS-02).
// Run: ./moq test tools --spec HostedInstances
const ADMIN_ONLY = "SPEC_HI_ADMIN"
const HOLDER = "SPEC_HI_HOLDER"
const make = (entityName, fields) => {
    if (ec.entity.find(entityName).condition(fields).count() > 0) return
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
// a System › Instance transition as the user, as its button posts it: the services run under the screen's authorization
function as(userId, transition, parameters) {
    ec.user.internalLoginUser(userId.toLowerCase().replace(/_/g, "."))
    const res = ec.screen.makeTest().rootScreen("component://tools/screen/System.xml")
        .render("Instance/InstanceList/" + transition, parameters, "post")
    const error = res.errorMessages.join(" ")
    ec.message.clearErrors()
    ec.user.logoutUser()
    return { error }
}

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        for (const userId of [ADMIN_ONLY, HOLDER]) {
            make("moqui.security.UserAccount", { userId, username: userId.toLowerCase().replace(/_/g, "."), userFullName: "Spec " + userId })
            make("moqui.security.UserGroupMember", { userGroupId: "ADMIN", userId, fromDate: ec.user.nowTimestamp })
        }
        make("moqui.security.UserGroup", { userGroupId: "SPEC_HI_HOLDERS", description: "Spec hosted instance holders" })
        make("moqui.security.UserGroupPermission", { userGroupId: "SPEC_HI_HOLDERS", userPermissionId: "INSTANCE_ADMIN", fromDate: ec.user.nowTimestamp })
        make("moqui.security.UserGroupMember", { userGroupId: "SPEC_HI_HOLDERS", userId: HOLDER, fromDate: ec.user.nowTimestamp })
        ec.artifactExecution.enableAuthz()
    },
})

feature("an ADMIN without INSTANCE_ADMIN is refused by the instance services; a holder passes the check", {
    when() {
        const refused = as(ADMIN_ONLY, "initAppInstance", { appInstanceId: "SPEC_HI_NONE" })
        const database = as(ADMIN_ONLY, "createAppDatabase", { appInstanceId: "SPEC_HI_NONE" })
        const held = as(HOLDER, "initAppInstance", { appInstanceId: "SPEC_HI_NONE" })
        return { refused, database, held }
    },
    then({ refused, database, held }) {
        refused.error.includes("INSTANCE_ADMIN")
        database.error.includes("INSTANCE_ADMIN")
        !held.error.includes("INSTANCE_ADMIN")
        held.error.includes("No InstanceHostType")
    },
})

feature("the generic tools do not write an InstanceImage, whose authTokenCmd runs on the host", {
    when() {
        login("john.doe")
        const res = ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").render("Data/Browse/AutoEdit/AutoEditMaster/update",
            { aen: "moqui.server.instance.InstanceImage", instanceImageId: "SPEC_HI_IMAGE", authTokenCmd: "touch /tmp/spec-hi" }, "post")
        ec.message.clearErrors()
        const written = ec.entity.find("moqui.server.instance.InstanceImage").condition("instanceImageId", "SPEC_HI_IMAGE").disableAuthz().count()
        return { errors: res.errorMessages.join(" "), written }
    },
    then({ errors, written }) {
        errors.includes("not available through this tool")
        written == 0
    },
})
