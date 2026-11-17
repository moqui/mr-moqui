// An owner's registered service is called under the caller's own authorization (SEC-ADM-10; OperationServices
// call#OwnerService, OperationLib.js callOwner): the operation services' ALL_USERS ALLOW does not carry into it, so a
// recovery whose owner service the operator holds no grant to is refused through execute#Recovery, and one the operator
// is granted runs. The owner here is BasicServices.echo#Data, which changes nothing and answers no applied, so a run is
// the owner's "did not take it" with no authorization error.
// Run: ./moq test framework --spec AdminOwnerCall
const O = "org.moqui.admin.OperationServices."
const OWNER = "org.moqui.impl.BasicServices.echo#Data"
const MESSAGE = "moqui.service.message.SystemMessage"
const users = {granted: "SPEC_OC_GRANTED", ungranted: "SPEC_OC_UNGRANTED"}
const ids = {granted: "SPEC_OC_M_GRANTED", ungranted: "SPEC_OC_M_UNGRANTED"}

const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const username = userId => userId.toLowerCase().replace(/_/g, ".")
function as(userId, action) {
    ec.user.internalLoginUser(username(userId))
    ec.artifactExecution.enableAuthz()
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
}
function invoke(name, parameters = {}) {
    try {
        const result = ec.service.sync().name(name).parameters(parameters).call() || {}
        const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
        ec.message.clearErrors()
        return Object.assign({}, result, {error})
    } catch (e) {
        ec.message.clearErrors()
        return {error: String(e.message || e)}
    }
}
// cancel one message by its owner's service, as the user
function cancel(userId, systemMessageId) {
    return as(userId, () => {
        const listed = invoke(O + "find#Messages", {reference: systemMessageId, state: "all"}).messageList?.[0]
        const out = invoke(O + "execute#Recovery", {
            targetKind: "SystemMessage",
            action: "cancel",
            targetList: [{targetId: systemMessageId, expectedRevision: listed?.revision}],
            reason: "Spec owner call"
        })
        return Object.assign({}, out, {row: Array.from(out.resultList || [])[0] || {}})
    })
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const userId of Object.values(users))
            make("moqui.security.UserAccount", {userId, username: username(userId), userFullName: "Spec owner call " + userId})
        make("moqui.security.UserGroup", {userGroupId: "SPEC_OC_OPERATORS", description: "Spec owner call operators"})
        for (const userPermissionId of ["ADMIN_OPERATION_VIEW", "ADMIN_OPERATION_MANAGE"])
            make("moqui.security.UserGroupPermission", {userGroupId: "SPEC_OC_OPERATORS", userPermissionId, fromDate: 0})
        for (const userId of Object.values(users))
            make("moqui.security.UserGroupMember", {userGroupId: "SPEC_OC_OPERATORS", userId, fromDate: 0})
        // the owner's own grant, as an app grants its owner services; only one operator holds it
        make("moqui.security.UserGroup", {userGroupId: "SPEC_OC_OWNERS", description: "Spec owner call owner grant"})
        make("moqui.security.UserGroupMember", {userGroupId: "SPEC_OC_OWNERS", userId: users.granted, fromDate: 0})
        make("moqui.security.ArtifactGroup", {artifactGroupId: "SPEC_OC_OWNER", description: "Spec owner service"})
        make("moqui.security.ArtifactGroupMember", {artifactGroupId: "SPEC_OC_OWNER", artifactName: OWNER,
            artifactTypeEnumId: "AT_SERVICE", nameIsPattern: "N", inheritAuthz: "N"})
        make("moqui.security.ArtifactAuthz", {artifactAuthzId: "SPEC_OC_OWNER_ALLOW", userGroupId: "SPEC_OC_OWNERS",
            artifactGroupId: "SPEC_OC_OWNER", authzTypeEnumId: "AUTHZT_ALLOW", authzActionEnumId: "AUTHZA_ALL"})

        make("moqui.service.message.SystemMessageType", {systemMessageTypeId: "SPEC_OC_TYPE",
            description: "Spec owner call", sendServiceName: "org.moqui.impl.SystemMessageServices.send#SystemMessageRest"})
        make("moqui.service.message.SystemMessageRemote", {systemMessageRemoteId: "SPEC_OC_REMOTE",
            description: "Spec owner call remote", sendUrl: "https://oc.example.test/hook", sendAuthEnumId: "SmatNone"})
        make("moqui.server.RecoveryDescriptor", {descriptorKey: "specOcOwner", targetKind: "SystemMessage",
            targetKey: "SPEC_OC_TYPE", ownerApp: "Spec owner", title: "Spec owner messages", cancelMode: "owner",
            recoverService: OWNER})
        for (const systemMessageId of Object.values(ids))
            make(MESSAGE, {systemMessageId, systemMessageTypeId: "SPEC_OC_TYPE", systemMessageRemoteId: "SPEC_OC_REMOTE",
                statusId: "SmsgError", isOutgoing: "Y", initDate: Date.now() - 3600000, lastAttemptDate: Date.now() - 1800000,
                failCount: 3, messageText: "{}"})
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.artifactExecution.disableAuthz()
    }
})

feature("an owner service the operator holds no grant to is refused through execute#Recovery, a granted one runs", {
    when() {
        const direct = as(users.ungranted, () => invoke(O + "call#OwnerService", {serviceName: "org.moqui.impl.BasicServices.noop"}))
        return {ungranted: cancel(users.ungranted, ids.ungranted), granted: cancel(users.granted, ids.granted), direct}
    },
    then({ungranted, granted, direct}) {
        // no grant: the owner's service is not authorized for the operator, whatever the dispatching service's grant
        ungranted.error == "" && ungranted.applied === false
        ungranted.row.reasonCode == "OWNER_REFUSED"
        /authoriz/i.test(ungranted.row.reason)
        // granted: the owner's service ran and answered (echo#Data takes nothing)
        granted.error == "" && granted.row.reasonCode == "OWNER_REFUSED"
        !/authoriz/i.test(granted.row.reason)
        granted.row.reason.includes("did not take it")
        // a service no registration names is not called
        direct.error.includes("No registration names the owner service")
    },
})
