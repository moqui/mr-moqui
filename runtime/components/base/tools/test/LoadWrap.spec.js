// The load wrap (service/moqui/tools/DatabaseLoadServices.xml run#LoadWrap) runs as a ServiceJob, which runs with
// authorization off: it needs DATABASE_ADMIN itself, and it loads as the user, authorized as from the Tools app
// (ec.artifactExecution.runAsUser), so the security checks of what it writes apply; runAsUser leaves the caller's own
// authorization state as it found it. A snapshot import, an administrator's whole-database restore, runs with
// authorization off inside it, as the plain import job does.
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
            const wrap = ec.service.sync().name("org.moqui.dev.DatabaseServices.plan#LoadWrap").parameters({ db: "group:transactional",
                entityNames: ["moqui.security.UserGroupMember"], description: "Spec load wrap" }).disableAuthz().call() || {}
            const planned = !!(wrap.dropJobId && wrap.restoreJobId)
            ec.message.clearErrors()
            // the text in a private file, as Data › Load passes it (SEC-DAT-96): read by the job and deleted
            const textLocation = ec.resource.writePrivateFile(membershipXml(dbaId), "xml")
            const written = ec.resource.getLocationReference(textLocation).getExists()
            // as the ServiceJob runs it: authorization off
            ec.artifactExecution.disableAuthz()
            const out = ec.service.sync().name("moqui.tools.DatabaseLoadServices.run#LoadWrap").parameters({ dropJobId: wrap.dropJobId,
                restoreJobId: wrap.restoreJobId, textLocation }).call() || {}
            ec.message.clearErrors()
            const deleted = !ec.resource.getLocationReference(textLocation).getExists()
            // a location that is not such a file is neither read nor deleted
            ec.service.sync().name("moqui.tools.DatabaseLoadServices.run#LoadWrap").parameters({ dropJobId: wrap.dropJobId,
                restoreJobId: wrap.restoreJobId, textLocation: "component://tools/data/ToolsSecurityData.xml" }).call()
            const otherRefused = ec.message.errorsString
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
            return { planned, out, direct, textLocation, written, deleted, otherRefused }
        }
        const { planned, out, direct, textLocation, written, deleted, otherRefused } = run()
        planned
        /^runtime:\/\/tmp\/private\/[0-9a-f]{32}\.xml$/.test(textLocation)
        written && deleted
        otherRefused.includes("not a private file of the load")
        out.dropStatusId == "DbjsDone"
        out.loaded === false
        String(out.loadError).includes("moqui.security.UserGroupMember")
        out.restoreStatusId == "DbjsDone"
        direct.includes("not authorized to change membership of group ADMIN_ADV")
        memberOf(dbaId, "ADMIN_ADV") == 0
    },
})

// before, the worker gave the load no array for its failures: a file it could not load was only in the log, and the run
// and its notification read as a clean load (TOOL-UI-006)
feature("a wrapped load names a file it could not load in the outcome and fails the run, and the keys are restored", {
    expect() {
        const run = () => {
            ec.user.internalLoginUser("spec.lw.dba")
            const wrap = ec.service.sync().name("org.moqui.dev.DatabaseServices.plan#LoadWrap").parameters({ db: "group:transactional",
                entityNames: ["moqui.security.UserGroupMember"], description: "Spec load wrap missing file" }).disableAuthz().call() || {}
            ec.message.clearErrors()
            // as the ServiceJob runs it: authorization off
            ec.artifactExecution.disableAuthz()
            const out = ec.service.sync().name("moqui.tools.DatabaseLoadServices.run#LoadWrap").parameters({ dropJobId: wrap.dropJobId,
                restoreJobId: wrap.restoreJobId, location: "component://tools/data/NoSuchSpecLoadWrap.xml" }).call() || {}
            const errors = ec.message.errorsString
            ec.message.clearErrors()
            ec.artifactExecution.enableAuthz()
            return { out, errors }
        }
        const { out, errors } = run()
        out.dropStatusId == "DbjsDone"
        String(out.outcome).includes("could not load [component://tools/data/NoSuchSpecLoadWrap.xml]")
        out.restoreStatusId == "DbjsDone"
        errors.includes("component://tools/data/NoSuchSpecLoadWrap.xml")
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

// A snapshot restore is an administrator's whole-database operation: the wrapped import runs with authorization off, as
// the plain ImportEntityDataSnapshot job does, so a privileged-group membership in the snapshot is restored for a
// DATABASE_ADMIN user outside that group; the security-check rules the import's disableEntityEca keeps would refuse it
// with authorization on and roll back the whole file (plans/archive/Security-Review-Medium.md SEC-DAT-83)
feature("a DATABASE_ADMIN user's wrapped snapshot import restores a privileged-group membership", {
    expect() {
        const snapUser = "SPEC_LW_SNAP"
        const run = () => {
            ec.artifactExecution.disableAuthz()
            make("moqui.security.UserAccount", { userId: snapUser, username: "spec.lw.snap", userFullName: "Spec Load Wrap Snapshot" })
            make("moqui.security.UserGroupMember", { userGroupId: "ADMIN_ADV", userId: snapUser, fromDate: 1265184000000 })
            // the snapshot of the memberships, by an administrator who may take a full backup
            ec.user.internalLoginUser("john.doe")
            ec.service.sync().name("org.moqui.impl.EntityServices.export#EntityDataSnapshot").parameters({
                entitiesToInclude: ["moqui.security.UserGroupMember"], baseFilename: "SpecLoadWrapSnapshot" }).call()
            ec.entity.find("moqui.security.UserGroupMember").condition("userId", snapUser).deleteAll()
            ec.artifactExecution.enableAuthz()
            const removed = memberOf(snapUser, "ADMIN_ADV")
            ec.user.internalLoginUser("spec.lw.dba")
            const wrap = ec.service.sync().name("org.moqui.dev.DatabaseServices.plan#LoadWrap").parameters({ db: "group:transactional",
                entityNames: ["moqui.security.UserGroupMember"], description: "Spec snapshot load wrap" }).disableAuthz().call() || {}
            ec.message.clearErrors()
            // as the ServiceJob runs it: authorization off
            ec.artifactExecution.disableAuthz()
            const out = ec.service.sync().name("moqui.tools.DatabaseLoadServices.run#LoadWrap").parameters({ dropJobId: wrap.dropJobId,
                restoreJobId: wrap.restoreJobId, loadKind: "snapshot", zipFilename: "SpecLoadWrapSnapshot.zip" }).call() || {}
            ec.message.clearErrors()
            const restored = memberOf(snapUser, "ADMIN_ADV")
            ec.resource.getLocationReference(ec.factory.getRuntimePath() + "/db/snapshot/SpecLoadWrapSnapshot.zip").delete()
            ec.entity.find("moqui.security.UserGroupMember").condition("userId", snapUser).deleteAll()
            ec.entity.find("moqui.security.UserAccount").condition("userId", snapUser).deleteAll()
            ec.artifactExecution.enableAuthz()
            return { removed, out, restored }
        }
        const { removed, out, restored } = run()
        removed == 0
        out.loaded === true
        out.restoreStatusId == "DbjsDone"
        restored == 1
    },
})

// The import between the foreign key jobs of Data › Snapshots' load wrap, as the DATABASE_ADMIN user: its out-parameters
const wrapSnapshot = (zipFilename) => {
    ec.user.internalLoginUser("spec.lw.dba")
    const wrap = ec.service.sync().name("org.moqui.dev.DatabaseServices.plan#LoadWrap").parameters({ db: "group:transactional",
        entityNames: ["moqui.security.UserGroupMember"], description: "Spec snapshot load wrap" }).call() || {}
    ec.message.clearErrors()
    const wrapped = ec.service.sync().name("moqui.tools.DatabaseLoadServices.run#LoadWrap").parameters({ dropJobId: wrap.dropJobId,
        restoreJobId: wrap.restoreJobId, loadKind: "snapshot", zipFilename }).call() || {}
    ec.message.clearErrors()
    return wrapped
}
const snapshotFile = (name) => ec.resource.getLocationReference(ec.factory.getRuntimePath() + "/db/snapshot/" + name)

// A snapshot that cannot be read is a failed import, not a clean one with no records: the service's error names the
// file and the loader's reason, so the ImportEntityDataSnapshot job's run records it, and the wrapped import says it
// was not loaded (TOOL-UI-008)
feature("a corrupt snapshot fails the import, its job run and the wrapped import", {
    expect() {
        const zipFilename = "SpecCorruptSnapshot.zip"
        const run = () => {
            const file = snapshotFile(zipFilename)
            let jobRunId = null
            ec.artifactExecution.disableAuthz()
            try {
                file.putText("not a zip archive")
                ec.user.internalLoginUser("john.doe")
                const direct = ec.service.sync().name("org.moqui.impl.EntityServices.import#EntityDataSnapshot")
                    .parameters({ zipFilename }).call() || {}
                const directError = ec.message.errorsString
                ec.message.clearErrors()
                // the job Data › Snapshots starts, on a job worker: at most 5 seconds, well inside the spec's budget
                jobRunId = ec.service.job("ImportEntityDataSnapshot").parameters({ zipFilename }).run()
                const runOf = () => ec.entity.find("moqui.service.job.ServiceJobRun").condition("jobRunId", jobRunId).useCache(false).one()
                const until = Date.now() + 5000
                while (Date.now() < until && runOf()?.endTime == null) sleep(50)
                const jobRun = runOf()
                const job = { ended: jobRun?.endTime != null, hasError: jobRun?.hasError, errors: String(jobRun?.errors || "") }
                const wrapped = wrapSnapshot(zipFilename)
                return { direct, directError, job, wrapped }
            } finally {
                ec.message.clearErrors()
                if (jobRunId) ec.entity.find("moqui.service.job.ServiceJobRun").condition("jobRunId", jobRunId).deleteAll()
                // a reference's exists is as it was when made: a new one
                if (snapshotFile(zipFilename).getExists()) file.delete()
                ec.artifactExecution.enableAuthz()
            }
        }
        const { direct, directError, job, wrapped } = run()
        !direct.recordsLoaded
        directError.includes(zipFilename)
        job.ended === true
        job.hasError == "Y"
        job.errors.includes(zipFilename)
        wrapped.loaded === false
        wrapped.outcome.includes(zipFilename)
        wrapped.restoreStatusId == "DbjsDone"
    },
})

// A snapshot directory whose one file fails while another loads reads as a data load with a failed file: the records
// loaded, and the file that could not be loaded named in the outcome (TOOL-UI-008)
feature("a wrapped snapshot with a corrupt file beside a good one loads the good one and names the bad one", {
    expect() {
        const dirName = "SpecMixedSnapshot"
        const run = () => {
            const good = snapshotFile(dirName + "/Good.xml")
            const bad = snapshotFile(dirName + "/Bad.zip")
            ec.artifactExecution.disableAuthz()
            try {
                good.putText('<entity-facade-xml><moqui.basic.EnumerationType enumTypeId="SPEC_LW_MIXED" description="Spec mixed snapshot"/></entity-facade-xml>')
                bad.putText("not a zip archive")
                const wrapped = wrapSnapshot(dirName)
                const loaded = ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "SPEC_LW_MIXED").useCache(false).count()
                return { wrapped, loaded }
            } finally {
                ec.message.clearErrors()
                ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "SPEC_LW_MIXED").deleteAll()
                // a reference's exists is as it was when made: new ones
                for (const name of ["/Good.xml", "/Bad.zip", ""]) if (snapshotFile(dirName + name).getExists()) snapshotFile(dirName + name).delete()
                ec.artifactExecution.enableAuthz()
            }
        }
        const { wrapped, loaded } = run()
        loaded == 1
        wrapped.loaded === true
        wrapped.recordsLoaded == 1
        wrapped.outcome.includes("could not load")
        wrapped.outcome.includes("Bad.zip")
        wrapped.restoreStatusId == "DbjsDone"
    },
})
