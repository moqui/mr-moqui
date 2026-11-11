// Backups, org.moqui.admin.BackupServices (mr-boss plan/impl-admin.md P16; design-admin §11 Backups, §15.3 "Backup scope"):
// coverage says what the backup job's runs recorded and never more: a run that succeeded, one the service skipped, one that
// failed, one that wrote its archive and failed after, and one that never ended are each their own state, and only the first
// is a success; the fixed warning of what an archive does not hold is always there; the archives are answered as metadata,
// never a path; a backup asked for is one run of the job with its stored parameters (nothing of the request names a path, a
// mode or a destination), one at a time and bounded in a day; reading needs ADMIN_BACKUP_VIEW and asking ADMIN_BACKUP_RUN.
// Run: ./moq test framework --spec AdminBackups
const B = "org.moqui.admin.BackupServices."
const JOB = "backup_EmbeddedDatabase_daily"
const RUN = "moqui.service.job.ServiceJobRun"
const LOCK = "moqui.service.job.ServiceJobRunLock"
const users = {run: "SPEC_BK_RUN", view: "SPEC_BK_VIEW", none: "SPEC_BK_NONE"}
const groups = {run: "SPEC_BK_RUNNERS", view: "SPEC_BK_VIEWERS"}
const minute = 60000
const hour = 3600000

const find = (entityName, conditions = {}) => ec.entity.find(entityName).condition(conditions).disableAuthz()
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
function as(userId, action) {
    ec.user.internalLoginUser(userId.toLowerCase().replace(/_/g, "."))
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
const has = (value, pattern) => pattern.test(String(value ?? ""))
const now = () => Number(ec.user.nowTimestamp)
const runRows = () => Array.from(find(RUN, {jobName: JOB}).list()).map(row => row.getMap())
const clearRuns = () => {
    // the lock names a run: it goes first
    find(LOCK, {jobName: JOB}).deleteAll()
    find(RUN, {jobName: JOB}).deleteAll()
}
const LEDGER = "backup-requests:" + JOB
const clearLedger = () => find("moqui.server.OperationResult", {operationKey: LEDGER}).deleteAll()
const ledger = times =>
    make("moqui.server.OperationResult", {
        operationKey: LEDGER,
        operationKind: "backup-requests",
        targetKey: JOB,
        resultText: JSON.stringify(times)
    })
let seq = 0
// A run of the backup job as the job runner records one.
function recordRun(startedAgo, fields = {}) {
    const started = now() - startedAgo
    make(
        RUN,
        Object.assign(
            {jobRunId: "SPEC_BK_R" + ++seq, jobName: JOB, startTime: started, endTime: started + 2000, hasError: "N"},
            fields
        )
    )
}
const written = {
    archivePath: "/var/moqui/backup/moqui-20261110T020000Z-d.dump",
    backupKind: "dump",
    bytesWritten: 4096,
    mirrored: "N",
    skipped: "N"
}
const coverage = () => invoke(B + "get#Coverage").coverage || {}
const archiveDir = () => String(ec.factory.getRuntimePath()) + "/db/backup"
const archiveFiles = [
    "moqui-20261101T020000Z-d.dump",
    "moqui_content-20261101T020000Z-w.dump",
    "notes.txt",
    "moqui-20261102T020000Z-d.dump.partial"
]

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const [key, userId] of Object.entries(users))
            make("moqui.security.UserAccount", {
                userId,
                username: userId.toLowerCase().replace(/_/g, "."),
                userFullName: "Spec backups " + key
            })
        for (const [key, userGroupId] of Object.entries(groups)) {
            make("moqui.security.UserGroup", {userGroupId, description: "Spec backups " + key})
            make("moqui.security.UserGroupMember", {userGroupId, userId: users[key], fromDate: 0})
        }
        for (const userPermissionId of ["ADMIN_BACKUP_VIEW", "ADMIN_BACKUP_RUN"])
            make("moqui.security.UserGroupPermission", {userGroupId: groups.run, userPermissionId, fromDate: 0})
        make("moqui.security.UserGroupPermission", {userGroupId: groups.view, userPermissionId: "ADMIN_BACKUP_VIEW", fromDate: 0})
        clearRuns()
    },
    cleanupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        clearRuns()
        find(LOCK, {jobName: JOB}).deleteAll()
        find("moqui.server.OperationResult").condition("operationKey", "like", "backup:SPEC_BK%").deleteAll()
        clearLedger()
        for (const name of archiveFiles) {
            const file = ec.resource.getLocationReference(archiveDir() + "/" + name)
            if (file.exists) file.delete()
        }
        for (const userGroupId of Object.values(groups)) {
            find("moqui.security.UserGroupPermission", {userGroupId}).deleteAll()
            find("moqui.security.UserGroupMember", {userGroupId}).deleteAll()
            find("moqui.security.UserGroup", {userGroupId}).deleteAll()
        }
        for (const userId of Object.values(users)) find("moqui.security.UserAccount", {userId}).deleteAll()
    }
})

feature("coverage says what the runs recorded: only a run that wrote its archive without an error is a success", {
    when() {
        const out = {}
        out.stranger = as(users.none, () => [
            invoke(B + "get#Coverage"),
            invoke(B + "find#Archives"),
            invoke(B + "run#Backup", {reason: "Spec"})
        ])
        out.viewerRun = as(users.view, () => invoke(B + "run#Backup", {reason: "Spec"}))
        as(users.view, () => {
            out.never = invoke(B + "get#Coverage")
            ec.artifactExecution.disableAuthz()
            recordRun(50 * hour, {results: JSON.stringify(written)})
            ec.artifactExecution.enableAuthz()
            out.success = coverage()
            ec.artifactExecution.disableAuthz()
            recordRun(40 * hour, {results: JSON.stringify({archivePath: "", bytesWritten: 0, mirrored: "N", skipped: "Y"})})
            ec.artifactExecution.enableAuthz()
            out.skipped = coverage()
            ec.artifactExecution.disableAuthz()
            recordRun(30 * hour, {hasError: "Y", errors: "moqui: pg_dump exited with status 1", results: null})
            ec.artifactExecution.enableAuthz()
            out.failed = coverage()
            ec.artifactExecution.disableAuthz()
            recordRun(20 * hour, {
                hasError: "Y",
                errors: "embedded database backup mirror did not finish; the local archive is in place",
                results: null
            })
            ec.artifactExecution.enableAuthz()
            out.partial = coverage()
            ec.artifactExecution.disableAuthz()
            recordRun(10 * hour, {endTime: null, hasError: null})
            ec.artifactExecution.enableAuthz()
            out.notEnded = coverage()
            ec.artifactExecution.disableAuthz()
            recordRun(5 * hour, {results: JSON.stringify({bytesWritten: 0, mirrored: "N", skipped: "N"})})
            ec.artifactExecution.enableAuthz()
            out.unnamed = coverage()
        })
        return out
    },
    then(out) {
        // reading needs the view permission, asking the run permission
        for (const refused of out.stranger) assert(has(refused.error, /ADMIN_BACKUP_(VIEW|RUN) is required/), refused.error)
        assert(has(out.viewerRun.error, /ADMIN_BACKUP_RUN is required/), out.viewerRun.error)
        assert(out.never.mayRun === false && out.never.canRun === false)
        // nothing has run: not a success, the seeded job paused, no mirror, and the warning there whatever the state
        const never = out.never.coverage
        assert(
            never.database.kind === "embedded" && never.state === "never" && never.lastSuccessText === "",
            JSON.stringify(never)
        )
        assert(
            never.jobExists && never.paused === true && never.schedule.scheduled === false && has(never.schedule.text, /^Paused/)
        )
        assert(never.mirror.state === "none" && has(never.mirror.text, /No off-site mirror/))
        assert(has(never.warning, /\.env file/) && has(never.warning, /field-encryption key/), never.warning)
        assert(never.notIncluded.map(item => item.key).join() === "env,encryptionKey,resourceFiles,externalStores")
        assert(has(never.verificationText, /No archive has been verified/))
        // the outcomes are each a state of their own, and only the first run is a success: with the job paused nothing
        // follows it, so the instance reads as not scheduled, not as covered (the job is not resumed here: a spec does
        // not set a backup running on a schedule)
        assert(
            out.success.state === "unscheduled" && has(out.success.stateText, /Succeeded.*No next backup is scheduled.*paused/),
            out.success.stateText
        )
        assert(out.success.lastRun.state === "success" && out.success.lastSuccessText !== "", JSON.stringify(out.success.lastRun))
        assert(out.success.lastRun.mirrored === false && out.success.lastRun.bytesWritten === 4096)
        assert(out.skipped.state === "skipped" && has(out.skipped.stateText, /Skipped/), out.skipped.stateText)
        assert(out.failed.state === "failed" && out.partial.state === "partial" && out.notEnded.state === "notEnded")
        assert(out.unnamed.state === "unknown")
        // the last success stays the one run that was, through every later state
        for (const later of [out.skipped, out.failed, out.partial, out.notEnded, out.unnamed])
            assert(later.lastSuccess.jobRunId === out.success.lastSuccess.jobRunId && later.warning === never.warning)
        // an archive's path, which a run records, is never answered
        assert(!JSON.stringify(out.success).includes("/var/moqui"), "a path was answered")
    }
})

feature("a backup asked for is one run of the job with its stored parameters: nothing of the request, one at a time, bounded", {
    when() {
        const out = {}
        clearRuns()
        out.inNames = Array.from(ec.service.getServiceDefinition(B + "run#Backup").inParameterNames).map(String)
        out.readNames = ["get#Coverage", "find#Archives"].flatMap(name =>
            Array.from(ec.service.getServiceDefinition(B + name).inParameterNames).map(String)
        )
        const runtime = String(ec.factory.getRuntimePath())
        for (const name of archiveFiles) ec.resource.getLocationReference(archiveDir() + "/" + name).putText("spec archive")
        as(users.run, () => {
            out.noReason = invoke(B + "run#Backup", {})
            out.before = invoke(B + "get#Coverage")
            // what a request may not name is not read, whatever it carries
            const request = {
                reason: "Spec: before an upgrade",
                operationKey: "SPEC_BK_K1",
                backupDir: "/tmp/spec-elsewhere",
                backupMode: "incremental",
                systemMessageRemoteId: "SPEC_BK_REMOTE",
                jobName: "SPEC_BK_OTHER"
            }
            out.first = invoke(B + "run#Backup", request)
            out.repeat = invoke(B + "run#Backup", request)
            out.second = invoke(B + "run#Backup", {reason: "Spec: again"})
            out.after = invoke(B + "get#Coverage")
            out.archives = invoke(B + "find#Archives")
        })
        out.runs = runRows()
        // the run took the job's lock, or ended and left none: never a lock naming a run that ended
        const lock = find(LOCK, {jobName: JOB}).one()
        const held = lock?.jobRunId ? find(RUN, {jobRunId: lock.jobRunId}).one() : null
        out.lock = {jobRunId: lock?.jobRunId || "", ended: !!lock?.jobRunId && (!held || held.endTime != null)}
        out.audits = Array.from(
            find("moqui.entity.EntityAuditLog", {
                changedFieldName: "backupRequest",
                pkPrimaryValue: JOB,
                changedByUserId: users.run
            }).list()
        ).map(row => row.getMap())
        out.direct = as(users.run, () => invoke(B + "start#BackupRun"))
        out.directHold = as(users.run, () => invoke(B + "hold#BackupLock", {jobRunId: out.first.jobRunId}))
        out.viewerArchives = as(users.view, () => invoke(B + "find#Archives"))
        out.runtime = runtime
        // three requests within a day, the last long enough ago: the day's bound
        clearRuns()
        clearLedger()
        ledger([now() - 2 * hour, now() - 10 * hour, now() - 20 * hour])
        out.limited = as(users.run, () => invoke(B + "run#Backup", {reason: "Spec: a fourth"}))
        // a run of the schedule's ten minutes ago: not right after it
        clearLedger()
        recordRun(10 * minute, {results: JSON.stringify(written)})
        out.soon = as(users.run, () => invoke(B + "run#Backup", {reason: "Spec: too soon"}))
        out.left = runRows().length
        // the schedule's lock: naming a run that ended two hours ago it is nobody's; naming one that has not ended (long
        // past the job's time, so only the lock says it runs) a request waits
        find(LOCK, {jobName: JOB}).deleteAll()
        clearRuns()
        recordRun(3 * hour, {results: JSON.stringify(written)})
        make(LOCK, {jobName: JOB, jobRunId: "SPEC_BK_R" + seq, lastRunTime: now() - 3 * hour})
        out.staleLock = as(users.run, () => invoke(B + "get#Coverage"))
        find(LOCK, {jobName: JOB}).deleteAll()
        recordRun(9 * hour, {endTime: null, hasError: null})
        make(LOCK, {jobName: JOB, jobRunId: "SPEC_BK_R" + seq, lastRunTime: now() - 9 * hour})
        out.liveLock = as(users.run, () => invoke(B + "get#Coverage"))
        find(LOCK, {jobName: JOB}).deleteAll()
        return out
    },
    then(out) {
        // no parameter of the three names where, what or how
        assert(out.inNames.slice().sort().join() === "operationKey,reason", out.inNames.join())
        assert(out.readNames.slice().sort().join() === "pageIndex,pageSize", out.readNames.join())
        assert(has(out.noReason.error, /reason/i), out.noReason.error)
        assert(out.before.canRun === true && out.before.holdReason == null, out.before.holdReason)
        // one run, as the job's stored parameters have it
        assert(
            out.first.applied === true && out.first.requested === true && out.first.jobRunId,
            out.first.error || out.first.reason
        )
        assert(out.runs.length === 1 && out.runs[0].jobRunId === out.first.jobRunId && out.runs[0].userId === users.run)
        const parameters = JSON.parse(out.runs[0].parameters)
        assert(
            parameters.systemMessageRemoteId === "EmbeddedDbBackup" && parameters.backupMode === "dump",
            out.runs[0].parameters
        )
        assert(!("backupDir" in parameters) && !("jobName" in parameters), out.runs[0].parameters)
        // its repeat answers the first result; another is not started beside or right after it
        assert(out.repeat.repeated === true && out.repeat.jobRunId === out.first.jobRunId)
        assert(out.second.applied === false && ["IN_PROGRESS", "TOO_SOON"].includes(out.second.reasonCode), out.second.reasonCode)
        assert(out.after.canRun === false && out.after.holdReason, "a second backup was offered")
        assert(out.audits.length === 1 && out.audits[0].changeReason === "Spec: before an upgrade", JSON.stringify(out.audits))
        assert(["", out.first.jobRunId].includes(out.lock.jobRunId) && !out.lock.ended, JSON.stringify(out.lock))
        // the start and the lock are nobody's to call
        assert(out.direct.error && !out.direct.jobRunId, "the run was started directly")
        assert(out.directHold.error && !out.directHold.held, "the lock was taken directly")
        // a lock left naming a run that ended is nobody's: it does not hold a request back
        assert(out.staleLock.holdReasonCode !== "IN_PROGRESS" && out.liveLock.holdReasonCode === "IN_PROGRESS")
        // the archives as metadata: the two that are archives, newest first, never a path, no download
        for (const listed of [out.archives, out.viewerArchives]) {
            assert(listed.archiveCount === 2, listed.error || JSON.stringify(listed.archiveList))
            assert(
                listed.archiveList.map(archive => archive.archiveKey).join() ===
                    "moqui-20261101T020000Z-d.dump,moqui_content-20261101T020000Z-w.dump"
            )
            assert(
                listed.archiveList.every(
                    archive => archive.download === false && archive.kind === "dump" && archive.sizeBytes > 0
                )
            )
            assert(
                !JSON.stringify(listed).includes(out.runtime) && !JSON.stringify(listed).includes("/db/backup"),
                "a path was answered"
            )
        }
        assert(
            out.archives.archiveList[1].tierText === "Weekly" && out.archives.archiveList[1].databaseText === "Database content"
        )
        assert(out.after.coverage.archiveCount === 2 && !JSON.stringify(out.after).includes(out.runtime))
        // bounded in a day, and not right after a run
        assert(out.limited.applied === false && out.limited.reasonCode === "DAILY_LIMIT", out.limited.reasonCode)
        assert(out.soon.applied === false && out.soon.reasonCode === "TOO_SOON" && out.left === 1, out.soon.reasonCode)
    }
})
