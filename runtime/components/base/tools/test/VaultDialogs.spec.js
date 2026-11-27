// System › Vaults' lease dialog and take-in (ToolsSecurityData.xml SYSTEM_VAULT_DIALOGS; plans/archive/Vault-Instances.md §3.7):
// a vault's approver who is no administrator reaches the lease dialog at /qapps/system/Vaults/Lease, where a deployment's
// page, an incident or a lease's notification sends them, and its transitions, whose vault services check the caller;
// an application that denies its groups System.xml (as mr-boss's Admin groups are) does not take the dialog from them;
// and nothing else of System is reachable. The dialog is rendered once, as the user with the deny.
// Run: ./moq test tools --spec VaultDialogs
const APPROVER = "SPEC_VD_APPROVER"
const DENIED = "SPEC_VD_DENIED"
const VAULT = "SPEC_VD_VAULT"
const make = (entityName, fields) => {
    if (ec.entity.find(entityName).condition(fields).count() > 0) return
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const usernameOf = userId => userId.toLowerCase().replace(/_/g, ".")
/* A render under /apps (what the /qapps shell loads a screen and its transitions from) as the user: its status, errors and output. */
function as(userId, path, parameters, method, mode) {
    ec.user.internalLoginUser(usernameOf(userId))
    try {
        const test = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
        const res = (mode ? test.renderMode(mode) : test).render(path, parameters, method || null)
        return {status: res.status, errors: res.errorMessages.join(" | "), output: String(res.output || "")}
    } finally {
        ec.message.clearErrors()
        ec.user.logoutUser()
    }
}

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        for (const userId of [APPROVER, DENIED])
            make("moqui.security.UserAccount", {userId, username: usernameOf(userId), userFullName: "Spec " + userId})
        // the deny an application gives its groups on System.xml, as mr-boss's BOSS_ADM_SYSTEM does
        make("moqui.security.UserGroup", {userGroupId: "SPEC_VD_NO_SYSTEM", description: "Spec: System.xml denied"})
        make("moqui.security.UserGroupMember", {userGroupId: "SPEC_VD_NO_SYSTEM", userId: DENIED, fromDate: ec.user.nowTimestamp})
        make("moqui.security.ArtifactGroup", {artifactGroupId: "SPEC_VD_SYSTEM", description: "Spec: System.xml"})
        make("moqui.security.ArtifactGroupMember", {
            artifactGroupId: "SPEC_VD_SYSTEM",
            artifactName: "component://tools/screen/System.xml",
            artifactTypeEnumId: "AT_XML_SCREEN",
            inheritAuthz: "Y"
        })
        make("moqui.security.ArtifactAuthz", {
            artifactAuthzId: "SPEC_VD_SYSTEM_DENY",
            userGroupId: "SPEC_VD_NO_SYSTEM",
            artifactGroupId: "SPEC_VD_SYSTEM",
            authzTypeEnumId: "AUTHZT_DENY",
            authzActionEnumId: "AUTHZA_ALL"
        })
        // both approve for an operational vault (its rows only, as a member's client leaves them)
        make("moqui.security.vault.Vault", {
            vaultId: VAULT,
            vaultKindEnumId: "VkOperational",
            lockEnumId: "VlSharedKey",
            ownerUserId: APPROVER,
            keyVersion: 1,
            revision: 1,
            vaultName: "Spec deploy tokens"
        })
        for (const userId of [APPROVER, DENIED])
            make("moqui.security.vault.VaultMember", {
                vaultId: VAULT,
                userId,
                keyVersion: 1,
                roleEnumId: "VrApprove",
                fromDate: ec.user.nowTimestamp
            })
        ec.artifactExecution.enableAuthz()
    }
})

feature("a vault's approver who is no administrator reaches the lease dialog and its transitions, and nothing else of System", {
    when() {
        return {
            // its one template, the qvue the shell loads
            dialog: as(DENIED, "apps/system/Vaults/Lease", {leaseId: "SPEC_VD_NO_LEASE"}, null, "qvue"),
            data: as(APPROVER, "apps/system/Vaults/Lease/leaseData", {leaseId: "SPEC_VD_NO_LEASE"}, "get"),
            takeIn: as(APPROVER, "apps/system/Vaults/TakeIn/takeInData", {vaultId: VAULT}, "get"),
            list: as(APPROVER, "apps/system/Vaults/VaultList", {}),
            system: as(APPROVER, "apps/system", {})
        }
    },
    then({dialog, data, takeIn, list, system}) {
        dialog.status == 200
        !/not authorized/i.test(dialog.errors)
        // the transitions answer the vault services' own words (no such lease), not a refusal of the screen
        data.status == 200
        !/not authorized/i.test(data.errors + data.output)
        data.output.includes('"errors"')
        takeIn.status == 200
        !/not authorized/i.test(takeIn.errors + takeIn.output)
        // the rest of System stays the administrators'
        list.status == 403 || /not authorized/i.test(list.errors)
        system.status == 403 || /not authorized/i.test(system.errors)
    }
})
