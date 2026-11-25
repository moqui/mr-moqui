// A reader of Dev AIDE's view tier (DEV_AIDE_VIEW, with the app's VIEW grant) lists a root's files: a read whose verb is
// not view, find, get or search declares authz-action="view", or its call checks for all actions and a VIEW grant
// refuses it, as the Workspace tree's list did (plans/ui-review-dev-aide-workspace.md DEVW-UI-003). A user with the
// same grant and no Dev AIDE permission is refused by the tier.
// Run: ./moq test dev-aide --spec WorkspaceReader
const call = (name, parameters) => {
    try {
        const out = ec.service.sync().name("org.moqui.devaide.WorkspaceServices." + name).parameters(parameters || {}).call() || {}
        const errors = ec.message.getErrorsString()
        ec.message.clearErrors()
        return {out, errors}
    } catch (error) {
        ec.message.clearErrors()
        return {out: {}, errors: String(error.message)}
    }
}

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        try {
            const make = (entity, fields) => { const row = ec.entity.makeValue(entity); row.setFields(fields, true, null, null); row.createOrUpdate() }
            const since = new Date(Date.now() - 60000)
            for (const [group, permission] of [["SPEC_WS_READERS", "DEV_AIDE_VIEW"], ["SPEC_WS_OUTSIDERS", null]]) {
                make("moqui.security.UserGroup", {userGroupId: group, description: "Dev AIDE workspace spec"})
                if (permission) make("moqui.security.UserGroupPermission", {userGroupId: group, userPermissionId: permission, fromDate: since})
                make("moqui.security.ArtifactAuthz", {artifactAuthzId: group + "_APP", artifactGroupId: "DEV_AIDE_APP", userGroupId: group,
                    authzTypeEnumId: "AUTHZT_ALLOW", authzActionEnumId: "AUTHZA_VIEW"})
            }
            make("moqui.security.UserAccount", {userId: "specWsReader", username: "spec.ws.reader", userFullName: "Spec Workspace Reader"})
            make("moqui.security.UserGroupMember", {userGroupId: "SPEC_WS_READERS", userId: "specWsReader", fromDate: since})
            make("moqui.security.UserAccount", {userId: "specWsOutsider", username: "spec.ws.outsider", userFullName: "Spec Workspace Outsider"})
            make("moqui.security.UserGroupMember", {userGroupId: "SPEC_WS_OUTSIDERS", userId: "specWsOutsider", fromDate: since})
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    },
    cleanupSpec() {
        ec.user.internalLoginUser("john.doe")
    }
})

feature("a reader of the view tier lists a root's files, and a user without the tier is refused", {
    when() {
        ec.user.internalLoginUser("spec.ws.reader")
        const root = (call("get#Roots").out.roots || [])[0]
        const listed = call("list#Directory", {rootId: root.id, path: "runtime/components/dev/dev-aide/test"})
        ec.user.internalLoginUser("spec.ws.outsider")
        const refused = call("list#Directory", {rootId: root.id, path: "runtime/components/dev/dev-aide/test"})
        return {listed, refused}
    },
    then({listed, refused}) {
        listed.errors == ""
        listed.out.entries.length > 0
        refused.errors.includes("DEV_AIDE_VIEW")
    }
})
