// The load wrap (service/moqui/tools/DatabaseLoadServices.xml run#LoadWrap) runs as a ServiceJob, which runs with
// authorization off: it needs DATABASE_ADMIN itself, and it loads as the user, authorized as from the Tools app
// (ec.artifactExecution.runAsUser), so the security checks of what it writes apply; runAsUser leaves the caller's own
// authorization state as it found it.
// Run: ./moq test tools --spec LoadWrap
const adminId = "SPEC_LW_ADMIN"
const dbaId = "SPEC_LW_DBA"
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const memberOf = (userId, groupId) => ec.entity.find("moqui.security.UserGroupMember").condition("userId", userId)
    .condition("userGroupId", groupId).disableAuthz().list().length
const membershipXml = (userId) =>
    `<entity-facade-xml><moqui.security.UserGroupMember userGroupId="ADMIN_ADV" userId="${userId}" fromDate="1265184000000"/></entity-facade-xml>`

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        // an ADMIN who is not in ADMIN_ADV: the Tools app, without DATABASE_ADMIN
        make("moqui.security.UserAccount", { userId: adminId, username: "spec.lw.admin", userFullName: "Spec Load Wrap" })
        make("moqui.security.UserGroupMember", { userGroupId: "ADMIN", userId: adminId, fromDate: ec.user.nowTimestamp })
        // an ADMIN with DATABASE_ADMIN through a group of its own, not ADMIN_ADV (set up with authorization off, which is
        // how only a seed load could grant it)
        make("moqui.security.UserAccount", { userId: dbaId, username: "spec.lw.dba", userFullName: "Spec Load Wrap DBA" })
        make("moqui.security.UserGroupMember", { userGroupId: "ADMIN", userId: dbaId, fromDate: ec.user.nowTimestamp })
        make("moqui.security.UserGroup", { userGroupId: "SPEC_LW_DBA_GROUP", description: "Spec load wrap DBA" })
        make("moqui.security.UserGroupPermission", { userGroupId: "SPEC_LW_DBA_GROUP", userPermissionId: "DATABASE_ADMIN", fromDate: ec.user.nowTimestamp })
        make("moqui.security.UserGroupMember", { userGroupId: "SPEC_LW_DBA_GROUP", userId: dbaId, fromDate: ec.user.nowTimestamp })
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        for (const userId of [adminId, dbaId]) {
            // the database jobs and audit rows the user's plans and runs left
            for (const job of ec.entity.find("moqui.database.DatabaseJob").condition("userId", userId).list())
                ec.entity.find("moqui.database.DatabaseJobStatement").condition("jobId", job.jobId).deleteAll()
            ec.entity.find("moqui.database.DatabaseJob").condition("userId", userId).deleteAll()
            ec.entity.find("moqui.database.DatabaseAudit").condition("userId", userId).deleteAll()
            ec.entity.find("moqui.security.UserGroupMember").condition("userId", userId).deleteAll()
            ec.entity.find("moqui.security.UserAccount").condition("userId", userId).deleteAll()
        }
        ec.entity.find("moqui.security.UserGroupPermission").condition("userGroupId", "SPEC_LW_DBA_GROUP").deleteAll()
        ec.entity.find("moqui.security.UserGroup").condition("userGroupId", "SPEC_LW_DBA_GROUP").deleteAll()
        ec.artifactExecution.enableAuthz()
    },
})

feature("an ADMIN not in ADMIN_ADV cannot load an ADMIN_ADV membership through the load wrap", {
    expect() {
        // (in a function: each statement at the top of expect is a condition)
        const run = () => {
            ec.user.internalLoginUser("spec.lw.admin")
            // as the ServiceJob runs it: authorization off
            ec.artifactExecution.disableAuthz()
            ec.service.sync().name("moqui.tools.DatabaseLoadServices.run#LoadWrap").parameters({ dropJobId: "none", restoreJobId: "none",
                xmlText: membershipXml(adminId) }).call()
            const refusal = ec.message.errorsString
            ec.message.clearErrors()
            // the user's own write, with authorization back on, meets the privileged-group check whatever disabled it
            let direct = null
            try {
                ec.artifactExecution.runAsUser("component://tools/screen/Tools.xml", () => {
                    make("moqui.security.UserGroupMember", { userGroupId: "ADMIN_ADV", userId: adminId, fromDate: ec.user.nowTimestamp })
                })
            } catch (e) { direct = String(e.message || e) }
            direct = direct || ec.message.errorsString
            ec.message.clearErrors()
            ec.artifactExecution.enableAuthz()
            return { refusal, direct }
        }
        const { refusal, direct } = run()
        refusal.includes("DATABASE_ADMIN")
        direct.includes("not authorized to change membership of group ADMIN_ADV")
        memberOf(adminId, "ADMIN_ADV") == 0
    },
})

// The data loader refuses the identity entities outright (restrictSensitiveEntities), so an ADMIN_ADV membership in the
// data never reaches the security checks: what the wrapped run proves is that it is refused and the keys still come back,
// and the user's own write inside runAsUser meets the privileged-group check although the user holds DATABASE_ADMIN.
feature("a DATABASE_ADMIN user's wrapped load is refused an ADMIN_ADV membership, the keys restored, and runAsUser checks the user", {
    expect() {
        const run = () => {
            ec.user.internalLoginUser("spec.lw.dba")
            const wrap = ec.service.sync().name("org.moqui.impl.DatabaseServices.plan#LoadWrap").parameters({ db: "group:transactional",
                entityNames: ["moqui.security.UserGroupMember"], description: "Spec load wrap" }).disableAuthz().call() || {}
            const planned = !!(wrap.dropJobId && wrap.restoreJobId)
            ec.message.clearErrors()
            // as the ServiceJob runs it: authorization off
            ec.artifactExecution.disableAuthz()
            const out = ec.service.sync().name("moqui.tools.DatabaseLoadServices.run#LoadWrap").parameters({ dropJobId: wrap.dropJobId,
                restoreJobId: wrap.restoreJobId, xmlText: membershipXml(dbaId) }).call() || {}
            ec.message.clearErrors()
            let direct = null
            try {
                ec.artifactExecution.runAsUser("component://tools/screen/Tools.xml", () => {
                    make("moqui.security.UserGroupMember", { userGroupId: "ADMIN_ADV", userId: dbaId, fromDate: ec.user.nowTimestamp })
                })
            } catch (e) { direct = String(e.message || e) }
            direct = direct || ec.message.errorsString
            ec.message.clearErrors()
            ec.artifactExecution.enableAuthz()
            return { planned, out, direct }
        }
        const { planned, out, direct } = run()
        planned
        out.dropStatusId == "DbjsDone"
        out.loaded === false
        String(out.loadError).includes("moqui.security.UserGroupMember")
        out.restoreStatusId == "DbjsDone"
        direct.includes("not authorized to change membership of group ADMIN_ADV")
        memberOf(dbaId, "ADMIN_ADV") == 0
    },
})

feature("runAsUser leaves the caller's authorization state as it was, whatever its body does", {
    expect() {
        const run = () => {
            ec.user.internalLoginUser("john.doe")
            ec.artifactExecution.disableAuthz()
            const before = ec.artifactExecution.authzDisabled()
            let inside = null
            ec.artifactExecution.runAsUser("component://tools/screen/Tools.xml", () => {
                inside = ec.artifactExecution.authzDisabled()
                // undo an earlier disable from inside, and disable again without enabling
                ec.artifactExecution.enableAuthz()
                ec.artifactExecution.disableAuthz()
            })
            const after = ec.artifactExecution.authzDisabled()
            ec.artifactExecution.enableAuthz()
            const enabled = !ec.artifactExecution.authzDisabled()
            return { before, inside, after, enabled }
        }
        const { before, inside, after, enabled } = run()
        before === true
        inside === false
        after === true
        enabled === true
    },
})
