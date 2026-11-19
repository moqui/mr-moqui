// The rules of Security.eecas.xml BackupDestination* (plans/Security-Review-Medium.md SEC-TLS-83): where a whole-database
// backup goes is its download, so a write that changes it needs the sealed ADMIN_BACKUP_DOWNLOAD, whatever screen or tool
// makes it. The destination parameters, the backup service and the permission are get#BackupDestination's (service/
// backup.rs, which refuses another destination named in a call by the same list), so they are kept once. A write with
// authorization disabled (a data load, a setup) answers for itself. Each check throws an Unauthorized error, which stops
// the write.
const text = value => String(value ?? "").trim()
const rules = () => ec.service.sync().name("org.moqui.impl.ServerServices.get#BackupDestination").disableAuthz().call()
const find = (entity, conditions) => ec.entity.find(entity).condition(conditions).disableAuthz()

// The rules when the signed-in user may not set a destination, else null.
function guarded() {
    if (ec.artifactExecution.authzDisabled()) return null
    const found = rules()
    return ec.user.hasPermission(found.permissionId) ? null : found
}
function refuse(found, what) {
    const refused = new Error("Where a database backup goes is a download of the whole database: " + what + " needs " + found.permissionId)
    refused.moquiErrorKind = "Unauthorized"
    throw refused
}
const backupJob = (jobName, found) => text(find("moqui.service.job.ServiceJob", {jobName}).one()?.serviceName) === found.serviceName
// whether a backup job mirrors to the remote
const mirrorRemote = (systemMessageRemoteId, found) =>
    Array.from(find("moqui.service.job.ServiceJobParameter", {parameterName: "systemMessageRemoteId", parameterValue: systemMessageRemoteId}).list())
        .some(row => backupJob(row.jobName, found))
// a field of the write that changes the row (any one given, for a create)
const changes = (operation, value, original) =>
    value != null && (operation !== "update" || text(value) !== text(original))

// ServiceJobParameter: a backup job's directory or mirror parameter, created, changed or removed
function parameter(operation, jobName, parameterName, parameterValue, original) {
    const found = guarded()
    if (!found || !Array.from(found.parameterNameList).map(String).includes(text(parameterName))) return
    if (operation === "update" && !changes(operation, parameterValue, original?.get("parameterValue"))) return
    if (backupJob(jobName, found)) refuse(found, "changing a backup job's " + parameterName)
}
// ServiceJob: a job with destination parameters made a backup
function job(operation, jobName, serviceName, original) {
    if (text(serviceName) === "" || text(serviceName) === text(original?.get("serviceName"))) return
    const found = guarded()
    if (!found || text(serviceName) !== found.serviceName) return
    const named = Number(find("moqui.service.job.ServiceJobParameter", {jobName}).condition("parameterName", "in", Array.from(found.parameterNameList)).count())
    if (named > 0) refuse(found, "making a job with a directory or a mirror a backup")
}
// SystemMessageRemote: the address, credentials or message type (whose sendPath is the mirror's path) of a backup mirror
function remote(operation, systemMessageRemoteId, fields, original) {
    if (!Object.keys(fields).some(name => changes(operation, fields[name], original?.get(name)))) return
    const found = guarded()
    if (found && mirrorRemote(systemMessageRemoteId, found)) refuse(found, "changing remote " + systemMessageRemoteId + ", a backup's mirror,")
}
// SystemMessageType: the sendPath of a type a backup mirror's remote has, the directory or key prefix the mirror writes
function messageType(operation, systemMessageTypeId, sendPath, original) {
    if (!changes(operation, sendPath, original?.get("sendPath"))) return
    const found = guarded()
    if (!found) return
    const mirrors = Array.from(find("moqui.service.message.SystemMessageRemote", {systemMessageTypeId}).list())
        .some(row => mirrorRemote(row.systemMessageRemoteId, found))
    if (mirrors) refuse(found, "changing the send path of " + systemMessageTypeId + ", a backup mirror's,")
}

;({parameter, job, remote, messageType})
