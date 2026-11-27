// Where a whole-database backup goes is its download (plans/archive/Security-Review-Medium.md SEC-TLS-83): without the sealed
// ADMIN_BACKUP_DOWNLOAD a user who may write the job and remote rows (System's JobDetail and ConnectionDetail, Tools) neither
// sets a backup job's directory or mirror nor repoints the remote it mirrors to, its message type or that type's send path
// (Security.eecas.xml BackupDestination*, BackupDestination.js),
// and backup#EmbeddedDatabase called by name with another destination is refused before anything is dumped
// (service/backup.rs). Another job's parameters and the remote's name stay open; a holder changes the destination.
// Run: ./moq test framework --spec BackupDestination
const JOB = "backup_EmbeddedDatabase_daily"
const REMOTE = "EmbeddedDbBackup"
const OTHER_JOB = "SPEC_BKDEST_OTHER"
const users = {operator: "SPEC_BKDEST_OPERATOR", holder: "SPEC_BKDEST_HOLDER"}
const groups = {operator: "SPEC_BKDEST_OPERATORS", holder: "SPEC_BKDEST_HOLDERS"}
const ROWS = "SPEC_BKDEST_ROWS"
const PARAMETER = "moqui.service.job.ServiceJobParameter"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const one = (entityName, conditions) => sys(() => ec.entity.find(entityName).condition(conditions).useCache(false).one())
// a write as the signed-in user with authorization on, as a screen's transition makes it: its error, or ""
function attempt(write) {
    const wasDisabled = ec.artifactExecution.authzDisabled()
    if (wasDisabled) ec.artifactExecution.enableAuthz()
    try {
        write()
        return ""
    } catch (err) {
        return String(err.message || err)
    } finally {
        if (wasDisabled) ec.artifactExecution.disableAuthz()
        ec.message.clearAll()
    }
}
const setParameter = (jobName, parameterName, parameterValue) => () => {
    const row = ec.entity.makeValue(PARAMETER)
    row.setFields({jobName, parameterName, parameterValue}, true, null, null)
    row.createOrUpdate()
}
const updateRemote = fields => () => {
    const row = ec.entity.find("moqui.service.message.SystemMessageRemote").condition("systemMessageRemoteId", REMOTE).one()
    row.setFields(fields, true, null, null)
    row.update()
}
const as = (userId, run) => {
    ec.user.internalLoginUser(userId.toLowerCase().replace(/_/g, "."))
    try {
        return run()
    } finally {
        ec.user.logoutUser()
    }
}

spec({
    setupSpec() {
        sys(() => {
            // the rows System's job and connection screens write, to both
            make("moqui.security.ArtifactGroup", {artifactGroupId: ROWS, description: "Spec backup rows"})
            make("moqui.security.ArtifactGroupMember", {artifactGroupId: ROWS, artifactTypeEnumId: "AT_ENTITY", nameIsPattern: "Y",
                artifactName: "moqui\\.service\\.(job|message)\\..*", inheritAuthz: "Y"})
            for (const [key, userId] of Object.entries(users)) {
                make("moqui.security.UserAccount", {userId, username: userId.toLowerCase().replace(/_/g, "."), userFullName: "Spec backup " + key})
                make("moqui.security.UserGroup", {userGroupId: groups[key], description: "Spec backup " + key})
                make("moqui.security.UserGroupMember", {userGroupId: groups[key], userId, fromDate: 0})
                make("moqui.security.ArtifactAuthz", {artifactAuthzId: ROWS + "_" + key, userGroupId: groups[key], artifactGroupId: ROWS,
                    authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL"})
            }
            make("moqui.security.UserGroupPermission", {userGroupId: groups.holder, userPermissionId: "ADMIN_BACKUP_DOWNLOAD", fromDate: 0})
            make("moqui.service.job.ServiceJob", {jobName: OTHER_JOB, serviceName: "org.moqui.impl.ServerServices.clean#PrintJobs", paused: "Y"})
        })
    },
    cleanupSpec() {
        sys(() => {
            ec.entity.find(PARAMETER).condition("jobName", "in", [JOB, OTHER_JOB]).condition("parameterName", "in", ["backupDir", "daysToKeep"]).deleteAll()
            ec.entity.find("moqui.service.job.ServiceJob").condition("jobName", OTHER_JOB).deleteAll()
            ec.entity.find("moqui.security.UserGroupPermission").condition("userGroupId", groups.holder).deleteAll()
            ec.entity.find("moqui.security.ArtifactAuthz").condition("artifactGroupId", ROWS).deleteAll()
            ec.entity.find("moqui.security.ArtifactGroupMember").condition("artifactGroupId", ROWS).deleteAll()
            ec.entity.find("moqui.security.ArtifactGroup").condition("artifactGroupId", ROWS).deleteAll()
            ec.entity.find("moqui.security.UserGroupMember").condition("userId", "in", Object.values(users)).deleteAll()
            ec.entity.find("moqui.security.UserGroup").condition("userGroupId", "in", Object.values(groups)).deleteAll()
        })
    }
})

feature("a backup's directory and mirror are set only by a holder of ADMIN_BACKUP_DOWNLOAD", {
    when() {
        const before = {mirror: one(PARAMETER, {jobName: JOB, parameterName: "systemMessageRemoteId"})?.parameterValue,
            sendUrl: one("moqui.service.message.SystemMessageRemote", {systemMessageRemoteId: REMOTE})?.sendUrl}
        const out = as(users.operator, () => ({
            dir: attempt(setParameter(JOB, "backupDir", "/srv/www/static")),
            mirror: attempt(setParameter(JOB, "systemMessageRemoteId", "SPEC_TAKER")),
            remote: attempt(updateRemote({sendUrl: "sftp://taker.example.net/backups", username: "taker"})),
            // the mirror's path is its message type's sendPath: neither the remote's type nor that path is changed
            remoteType: attempt(updateRemote({systemMessageTypeId: "SPEC_BKDEST_TYPE"})),
            sendPath: attempt(() => {
                const row = ec.entity.find("moqui.service.message.SystemMessageType").condition("systemMessageTypeId", REMOTE).one()
                row.setFields({sendPath: "../../srv/www/static"}, true, null, null)
                row.update()
            }),
            renamed: attempt(updateRemote({description: "Embedded database backup mirror (spec)"})),
            other: attempt(setParameter(OTHER_JOB, "daysToKeep", "30")),
            byName: attempt(() => {
                ec.service.sync().name("org.moqui.impl.ServerServices.backup#EmbeddedDatabase")
                    .parameters({systemMessageRemoteId: "SPEC_TAKER"}).call()
                if (ec.message.hasError()) throw new Error(ec.message.getErrorsString())
            })
        }))
        out.afterOperator = {mirror: one(PARAMETER, {jobName: JOB, parameterName: "systemMessageRemoteId"})?.parameterValue,
            sendUrl: one("moqui.service.message.SystemMessageRemote", {systemMessageRemoteId: REMOTE})?.sendUrl,
            dir: one(PARAMETER, {jobName: JOB, parameterName: "backupDir"})}
        out.holder = as(users.holder, () => attempt(setParameter(JOB, "backupDir", "/var/moqui/backup-spec")))
        out.holderDir = one(PARAMETER, {jobName: JOB, parameterName: "backupDir"})?.parameterValue
        out.before = before
        return out
    },
    then(out) {
        out.dir.includes("ADMIN_BACKUP_DOWNLOAD")
        out.mirror.includes("ADMIN_BACKUP_DOWNLOAD")
        out.remote.includes("ADMIN_BACKUP_DOWNLOAD")
        out.remoteType.includes("ADMIN_BACKUP_DOWNLOAD")
        out.sendPath.includes("ADMIN_BACKUP_DOWNLOAD")
        out.byName.includes("ADMIN_BACKUP_DOWNLOAD")
        out.renamed == ""
        out.other == ""
        out.afterOperator.mirror == out.before.mirror
        out.afterOperator.sendUrl == out.before.sendUrl
        out.afterOperator.dir == null
        out.holder == ""
        out.holderDir == "/var/moqui/backup-spec"
    },
})
