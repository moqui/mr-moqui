// org.moqui.admin.BackupServices (mr-boss plan/design-admin.md §11 Backups, §15.3 "Backup scope"; impl-admin P16): what of
// this instance's data is backed up, said truthfully; the archives kept, as metadata; and a backup asked for now. It is a view
// of the framework's own backup (org.moqui.impl.ServerServices.backup#EmbeddedDatabase, its seeded job
// backup_EmbeddedDatabase_daily and that job's runs), not a second one. No dependency on Mantle or an app.
//
// Coverage is what the records say, never what a person says. A run of the backup job is a success only when it ended
// without an error, was not skipped and names the archive it wrote; a run the service skipped (this process runs no embedded
// database), one that failed, one that wrote its archive and failed after (the mirror, another database of the cluster), one
// that never ended and one still running are each their own state. A database that is not the embedded one is its
// provider's to back up: its state here is unknown, never a success. A job that keeps only its failed runs has no record of
// a success, and that is said. Nothing here marks a backup done.
//
// What a request may name. Nothing of where or what: get#Coverage and find#Archives take no parameter but a page, and
// run#Backup a reason and an operation key. The archive directory, the mode, the counts kept and the mirror are the backup
// job's stored parameters and the instance's properties, set by who administers the instance (Tools, the deployment). An
// archive is answered as its name, database, kind, tier, time and size: never a path, and there is no download (that is
// later, under the sealed ADMIN_BACKUP_DOWNLOAD).
//
// A backup asked for is a run of the backup job with its stored parameters, started as a job run so its outcome is
// recorded as every run's is. It is refused while a run is under way, sooner than minMinutes after the last run started or
// was asked for, and past maxPerDay requests in a day: each run writes an archive and moves the kept dailies along, so asking
// often would push the older ones out. Requests are taken one after the other under a ledger row (an OperationResult, kind
// backup-requests: when the last requests were made), locked for the request. The run is started outside the request's
// transaction (start#BackupRun, which has none): the job runner writes the run's start and end from its own thread, and a
// run row the request had not yet committed would lose them.
//
// One backup at a time, the schedule's too. Two runs of the backup beside each other are not safe: each begins by removing
// every unfinished archive in the directory (crates/moqui-drivers/src/backup.rs sweep_partials), the other's too, and each
// renames and removes archives by its own retention plan. The scheduler starts a job only when it takes the job's lock
// (ServiceJobRunLock), and a run outside the schedule takes none (ec.service.job(name).run()): so a run asked for here takes
// the lock for itself once it has its run id (hold#BackupLock), and the job runner releases it when the run ends, as it
// does a scheduled run's (its release names the run). A scheduled run then waits for the next pass after the run ended, and
// a request is refused while a scheduled run holds the lock. A lock naming a run that has ended, or one that is gone, is
// nobody's: it is read as free here and taken over (the scheduler ignores it after the job's expireLockTime).
const lib = ec.resource.script("component://framework/service/org/moqui/admin/OperationLib.js")
const {find, one, rows, map, ms, text, refused} = lib
const VIEW = "ADMIN_BACKUP_VIEW"
const RUN = "ADMIN_BACKUP_RUN"
const JOB = "moqui.service.job.ServiceJob"
const JOB_RUN = "moqui.service.job.ServiceJobRun"
const LOCK = "moqui.service.job.ServiceJobRunLock"
const PARAMETER = "moqui.service.job.ServiceJobParameter"
const REMOTE = "moqui.service.message.SystemMessageRemote"
// the framework's backup and its seeded job (crates/moqui-core/src/service/backup.rs)
const BACKUP_SERVICE = "org.moqui.impl.ServerServices.backup#EmbeddedDatabase"
const BACKUP_JOB = "backup_EmbeddedDatabase_daily"
const DIR_PROPERTY = "embedded_db_backup_dir"
const START = "org.moqui.admin.BackupServices.start#BackupRun"
const HOLD = "org.moqui.admin.BackupServices.hold#BackupLock"
const LEDGER = "backup-requests:" + BACKUP_JOB
const minMinutes = 30
const maxPerDay = 3
const maxArchives = 500
const runsRead = 60
const zone = () => text(System.getProperty("default_time_zone")) || "UTC"
const zoned = value => (value == null ? "" : ec.l10n.format(Number(value), "yyyy-MM-dd HH:mm", null, zone()))
const now = () => ms(ec.user.nowTimestamp)

// What an archive does not hold, and the warning shown with every coverage: fixed text, not a setting.
const notIncluded = [
    {key: "env", title: "The .env file", text: "The instance's settings and secrets (.env) are not in an archive."},
    {
        key: "encryptionKey",
        title: "The field-encryption key",
        text: "Encrypted fields in an archive cannot be read without the key they were encrypted under, and the key is not in the archive."
    },
    {
        key: "resourceFiles",
        title: "Resource files",
        text: "Files kept outside the database (the runtime directory's resources and components, an object store) are not in an archive."
    },
    {
        key: "externalStores",
        title: "External databases and search servers",
        text: "A database or a search server this process does not run itself is backed up by its own provider, not here."
    }
]
const warning =
    "A database archive is not the whole instance. The .env file and the field-encryption key are not in it: keep a copy of both with the archives, apart from this server, or an archive cannot be restored to a working instance. Files kept outside the database are not in it either."

function requireView() {
    lib.signedIn()
    if (!ec.user.hasPermission(VIEW) && !ec.user.hasPermission(RUN)) throw new Error(VIEW + " is required")
}
const json = value => {
    try {
        const found = JSON.parse(text(value) || "null")
        return found && typeof found === "object" && !Array.isArray(found) ? found : {}
    } catch (error) {
        return {}
    }
}

// ===== what is backed up =====
// The databases this process uses: embedded (the framework's backup covers them), external (their provider's), or both.
function databaseOf() {
    let sources = []
    try {
        const status = ec.factory.getStatusMap(true) || {}
        sources = Array.from(status.DataSources || [])
    } catch (error) {
        ec.logger.error("Backup coverage: the data sources could not be read: " + String(error?.message ?? error))
    }
    const embedded = sources.filter(source => source.embedded === true).length
    const external = sources.length - embedded
    const kind = !sources.length ? "unknown" : !external ? "embedded" : !embedded ? "external" : "mixed"
    return {
        kind,
        text: {
            embedded: "Embedded database, run by this server",
            external: "External database, not run by this server",
            mixed: "An embedded database and an external one",
            unknown: "The database could not be read"
        }[kind]
    }
}
// The backup job, or null: the seeded job while it still runs the framework's backup service.
function backupJob() {
    const found = one(JOB, {jobName: BACKUP_JOB})
    return found && text(found.serviceName) === BACKUP_SERVICE ? map(found) : null
}
// When backups were asked for here, newest first (epoch millis; the ledger keeps the last twenty).
const requestsOf = row => {
    try {
        const found = JSON.parse(text(row?.resultText) || "[]")
        return Array.isArray(found) ? found.map(Number).filter(Number.isFinite) : []
    } catch (error) {
        return []
    }
}
const jobParameters = () =>
    Object.fromEntries(rows(PARAMETER, {jobName: BACKUP_JOB}).map(row => [text(row.parameterName), text(row.parameterValue)]))
const timeoutMs = job => Math.max(Number(job?.transactionTimeout) || 1800, 60) * 1000
const paused = job => !(job.paused == null || job.paused === "N")
// The run that holds the job's lock, or "": a lock is held only while the run it names has not ended.
function lockHolder(lockRow) {
    const jobRunId = text(lockRow?.jobRunId)
    if (!jobRunId) return ""
    const run = one(JOB_RUN, {jobRunId})
    return run && run.endTime == null ? jobRunId : ""
}

// ===== runs =====
const outcomeTexts = {
    success: "Succeeded",
    skipped: "Skipped: this server runs no embedded database, so nothing was backed up",
    failed: "Failed",
    partial: "Archive written, with a failure after it",
    running: "Running",
    notEnded: "Not ended: its server stopped, or the run was lost",
    unknown: "Ended without naming an archive"
}
// The outcome of one run of the backup job, from what the run recorded.
function outcomeOf(run, job, nowMs) {
    const results = json(run.results)
    const wrote = !!text(results.archivePath)
    let state
    if (run.endTime == null) {
        const started = ms(run.startTime) ?? ms(run.lastUpdatedStamp) ?? 0
        state = nowMs - started <= timeoutMs(job) ? "running" : "notEnded"
    } else if (run.hasError === "Y")
        // the service adds an error for a database that failed and for a mirror that did not finish: the local archive of
        // the others is in place
        state = wrote || /local archive is in place/i.test(text(run.errors)) ? "partial" : "failed"
    else if (results.skipped === "Y" || results.skipped === true) state = "skipped"
    else state = wrote ? "success" : "unknown"
    const started = ms(run.startTime)
    const ended = ms(run.endTime)
    return {
        jobRunId: run.jobRunId,
        state,
        stateText: outcomeTexts[state],
        startedDate: started,
        startedText: zoned(started),
        endedText: zoned(ended),
        durationMs: started != null && ended != null ? ended - started : null,
        startedBy: run.userId ? "A person" : "The schedule",
        byPerson: !!run.userId,
        mirrored: state === "success" || state === "partial" ? results.mirrored === "Y" : null,
        backupKind:
            state === "success"
                ? text(results.backupKind)
                      .replace(/[^A-Za-z0-9 _-]/g, "")
                      .slice(0, 40) || null
                : null,
        bytesWritten: state === "success" && Number.isFinite(Number(results.bytesWritten)) ? Number(results.bytesWritten) : null
    }
}
const recentRuns = (job, nowMs) =>
    Array.from(find(JOB_RUN, {jobName: BACKUP_JOB}).orderBy("-jobRunId").limit(runsRead).list())
        .map(row => outcomeOf(map(row), job, nowMs))
        .sort((a, b) => (b.startedDate ?? 0) - (a.startedDate ?? 0) || String(b.jobRunId).localeCompare(String(a.jobRunId)))

// ===== archives =====
const archiveName = /^(moqui|search|moqui_[A-Za-z0-9_]+)-(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})Z-([dwmy])(\.dump)?$/
const tierTexts = {d: "Daily", w: "Weekly", m: "Monthly", y: "Yearly"}
const databaseText = prefix =>
    prefix === "moqui" ? "Main database" : prefix === "search" ? "Search" : "Database " + prefix.slice("moqui_".length)
const sizeText = bytes =>
    bytes == null
        ? ""
        : bytes >= 1073741824
          ? (bytes / 1073741824).toFixed(1) + " GiB"
          : bytes >= 1048576
            ? (bytes / 1048576).toFixed(1) + " MiB"
            : Math.max(Math.ceil(bytes / 1024), 0) + " KiB"
// The archive directory, resolved here from the job's stored parameter, the instance's property or the default: never from
// a request. {archives} newest first, or {problem} why they could not be listed; the path is not answered.
function archivesOf() {
    const directory =
        text(jobParameters().backupDir) ||
        text(System.getProperty(DIR_PROPERTY)) ||
        String(ec.factory.getRuntimePath()) + "/db/backup"
    let entries
    try {
        const reference = ec.resource.getLocationReference(directory)
        if (!reference?.exists) return {archives: [], directoryExists: false}
        if (!reference.directory) return {archives: [], problem: "The backup directory is not a directory"}
        entries = reference.getDirectoryEntries()
    } catch (error) {
        ec.logger.error("Backup archives could not be listed: " + String(error?.message ?? error))
        return {archives: [], problem: "The backup directory could not be read"}
    }
    const archives = []
    for (const entry of entries) {
        const name = text(entry.fileName)
        const parsed = archiveName.exec(name)
        if (!parsed) continue
        const created = Date.UTC(+parsed[2], +parsed[3] - 1, +parsed[4], +parsed[5], +parsed[6], +parsed[7])
        // a dump is one file; a physical backup a directory, whose size is not summed here
        const bytes = parsed[9] && entry.file && Number.isFinite(Number(entry.size)) ? Number(entry.size) : null
        archives.push({
            archiveKey: name,
            database: parsed[1],
            databaseText: databaseText(parsed[1]),
            kind: parsed[9] ? "dump" : "physical",
            kindText: parsed[9] ? "Dump (one file)" : "Physical backup (a directory)",
            tier: parsed[8],
            tierText: tierTexts[parsed[8]],
            createdDate: created,
            createdText: zoned(created),
            sizeBytes: bytes,
            sizeText: sizeText(bytes),
            // there is no download here (later, under the sealed ADMIN_BACKUP_DOWNLOAD)
            download: false
        })
    }
    archives.sort((a, b) => b.createdDate - a.createdDate || a.archiveKey.localeCompare(b.archiveKey))
    return {archives, directoryExists: true}
}
// How long an archive is kept, in words, from the job's stored counts.
function retentionText(parameters) {
    const count = (name, fallback) => (text(parameters[name]) === "" ? fallback : Math.max(Number(parameters[name]) || 0, 0))
    const yearly = text(parameters.yearlyCount) === "" ? "every yearly one" : count("yearlyCount", 0) + " yearly"
    return (
        "Kept: " +
        count("dailyCount", 7) +
        " daily, " +
        count("weeklyCount", 5) +
        " weekly, " +
        count("monthlyCount", 12) +
        " monthly and " +
        yearly +
        ". An older archive is removed by the run that passes it."
    )
}

// ===== the mirror =====
function mirrorOf(parameters, lastWritten) {
    const remoteId = text(parameters.systemMessageRemoteId)
    const remote = remoteId ? one(REMOTE, {systemMessageRemoteId: remoteId}) : null
    if (!remote || !text(remote.sendUrl))
        return {state: "none", text: "No off-site mirror: the archives are kept on this server only", lastMirrored: null}
    const lastMirrored = lastWritten ? lastWritten.mirrored === true : null
    return {
        state: lastMirrored === false ? "notMirrored" : "configured",
        text:
            lastMirrored === false
                ? "An off-site mirror is set, and the last archive written was not copied to it"
                : lastMirrored
                  ? "An off-site mirror is set, and the last archive written was copied to it"
                  : "An off-site mirror is set; no archive has been written since",
        lastMirrored
    }
}

// ===== asking for a backup =====
// Why a backup may not be asked for now: {reasonCode, reason}, or null.
function runHold(database, job, lockRow, runs, requests, nowMs) {
    const hold = (reasonCode, reason) => ({reasonCode, reason})
    if (database.kind === "external")
        return hold(
            "EXTERNAL_DATABASE",
            "This instance's database is not run by this server: it is backed up by its provider's own means"
        )
    if (database.kind === "unknown") return hold("DATABASE_UNKNOWN", "The database could not be read, so no backup is started")
    if (!job)
        return hold(
            "NO_BACKUP_JOB",
            "This instance has no backup job (" + BACKUP_JOB + "): an instance administrator restores it"
        )
    if (lockHolder(lockRow) || runs.some(run => run.state === "running"))
        return hold("IN_PROGRESS", "A backup is running: a second one is not started beside it")
    const last = Math.max(runs.find(run => run.startedDate != null)?.startedDate ?? 0, requests[0] ?? 0)
    if (last && nowMs - last < minMinutes * 60000)
        return hold(
            "TOO_SOON",
            "The last backup started " +
                zoned(last) +
                ": another may be asked for " +
                minMinutes +
                " minutes after it (each one moves the kept daily archives along)"
        )
    if (requests.filter(asked => nowMs - asked < 86400000).length >= maxPerDay)
        return hold(
            "DAILY_LIMIT",
            "Backups were asked for " + maxPerDay + " times in the last day already: the schedule takes the next one"
        )
    return null
}

// get#Coverage: what is backed up, by what, how it last went and what is not included.
function getCoverage() {
    requireView()
    const nowMs = now()
    const database = databaseOf()
    const job = backupJob()
    const parameters = job ? jobParameters() : {}
    const lockRow = job ? one(LOCK, {jobName: BACKUP_JOB}) : null
    const runs = job ? recentRuns(job, nowMs) : []
    const running = runs.find(run => run.state === "running") || null
    const lastEnded = runs.find(run => run.state !== "running") || null
    const lastSuccess = runs.find(run => run.state === "success") || null
    const lastWritten = runs.find(run => run.state === "success" || run.state === "partial") || null
    const successesKept = !!job && job.errorHistoryOnly !== "Y"
    const listed = database.kind === "external" || database.kind === "unknown" ? {archives: []} : archivesOf()
    const dumps = listed.archives.filter(archive => archive.sizeBytes != null)
    const newest = listed.archives[0] || null
    let schedule = {scheduled: false, text: "No backup job", nextRunText: ""}
    if (job) {
        const cron = text(job.cronExpression)
        let description = ""
        let next = null
        try {
            description = cron ? text(ec.service.getCronDescription(cron)) : ""
            next = cron && !paused(job) ? ec.service.nextCronExecution(cron, nowMs) : null
        } catch (error) {
            description = ""
        }
        schedule = {
            scheduled: !!cron && !paused(job),
            text: !cron
                ? "No schedule: it runs only when asked for"
                : paused(job)
                  ? "Paused: no backup is made on a schedule until an instance administrator resumes the job"
                  : "Scheduled" + (description ? ": " + description : ""),
            nextRunText: next == null ? "" : zoned(next)
        }
    }
    // the one word for the instance's coverage: never a success for what was not written by a run that says so
    let state
    let stateText
    if (database.kind === "external") {
        state = "external"
        stateText =
            "Unknown: this instance's database is not run by this server. Its backups are its provider's, and nothing here knows whether they are made."
    } else if (database.kind === "unknown") {
        state = "unknown"
        stateText = "Unknown: the database could not be read"
    } else if (!job) {
        state = "none"
        stateText = "Not backed up: this instance has no backup job"
    } else if (!lastEnded) {
        // a first run under way is said so: nothing has been backed up yet, and "never" would not be all of it
        state = running ? "running" : successesKept ? "never" : "unknown"
        stateText = running
            ? "The first backup is running: nothing has been backed up yet"
            : successesKept
              ? "No backup has run yet"
              : "Unknown: the backup job keeps only its failed runs, so a run that succeeded left no record"
    } else {
        state = lastEnded.state
        stateText = "The last backup: " + lastEnded.stateText + (lastEnded.startedText ? ", " + lastEnded.startedText : "")
    }
    // a success is that one backup's, not the instance's: with the job paused or without a schedule no next one is made,
    // so the instance is not read as covered by a success that nothing will follow
    if (state === "success" && !schedule.scheduled) {
        state = "unscheduled"
        stateText +=
            ". No next backup is scheduled: " +
            (paused(job) ? "the backup job is paused" : "the backup job has no schedule") +
            ", so the data changed since is not backed up until one is asked for"
    }
    if (database.kind === "mixed" && state === "success") {
        state = "partial"
        stateText = "The embedded database was backed up; the external one is its provider's, and its state is unknown here"
    }
    const requests = requestsOf(one(lib.RESULT, {operationKey: LEDGER}))
    const hold = ec.user.hasPermission(RUN) ? runHold(database, job, lockRow, runs, requests, nowMs) : null
    return {
        coverage: {
            database,
            provider: database.kind === "external" ? "The database's own provider" : "Embedded database backup",
            state,
            stateText,
            running: !!running,
            runningText: running ? "A backup is running since " + running.startedText : "",
            jobExists: !!job,
            paused: !!job && paused(job),
            schedule,
            successesKept,
            lastRun: lastEnded,
            lastSuccessText: lastSuccess?.endedText || lastSuccess?.startedText || "",
            lastSuccess,
            archiveCount: listed.archives.length,
            archiveBytesText: dumps.length ? sizeText(dumps.reduce((sum, archive) => sum + archive.sizeBytes, 0)) : "",
            newestArchiveText: newest ? newest.createdText : "",
            archiveProblem: listed.problem || null,
            retentionText: job ? retentionText(parameters) : "",
            mirror: job ? mirrorOf(parameters, lastWritten) : {state: "none", text: "No off-site mirror", lastMirrored: null},
            // no restore of an archive is made or recorded here
            verificationText: "No archive has been verified by a restore here: test a restore on another machine",
            notIncluded,
            warning,
            timeZone: zone()
        },
        canRun: ec.user.hasPermission(RUN) && !hold,
        mayRun: ec.user.hasPermission(RUN),
        holdReasonCode: hold?.reasonCode || null,
        holdReason: hold?.reason || null,
        minMinutes,
        maxPerDay
    }
}

// find#Archives: the archives kept, as metadata.
function findArchives(p) {
    requireView()
    const database = databaseOf()
    if (database.kind === "external" || database.kind === "unknown")
        return {archiveList: [], archiveCount: 0, pageIndex: 0, pageSize: 50, problem: null, external: true}
    const job = backupJob()
    const listed = archivesOf()
    const {pageIndex, pageSize} = lib.pageOf(p)
    const all = listed.archives.slice(0, maxArchives)
    return {
        archiveList: all.slice(pageIndex * pageSize, (pageIndex + 1) * pageSize),
        archiveCount: all.length,
        pageIndex,
        pageSize,
        problem: listed.problem || null,
        external: false,
        retentionText: job ? retentionText(jobParameters()) : ""
    }
}

// run#Backup: ask for a backup now: one run of the backup job with its stored parameters.
function runBackup(p) {
    lib.signedIn()
    if (!ec.user.hasPermission(RUN)) throw new Error(RUN + " is required")
    const reason = lib.requiredReason(p)
    return lib.once(lib.operationKey(p), "backup", BACKUP_JOB, () => {
        // requests are taken one after the other: the ledger row is locked until this one commits
        if (!one(lib.RESULT, {operationKey: LEDGER}))
            lib.write(
                lib.RESULT,
                {operationKey: LEDGER, operationKind: "backup-requests", targetKey: BACKUP_JOB, resultText: "[]"},
                true
            )
        const ledger = find(lib.RESULT, {operationKey: LEDGER}).forUpdate(true).one()
        const nowMs = now()
        const database = databaseOf()
        const job = backupJob()
        const lockRow = job ? one(LOCK, {jobName: BACKUP_JOB}) : null
        const hold = runHold(database, job, lockRow, job ? recentRuns(job, nowMs) : [], requestsOf(ledger), nowMs)
        if (hold) return refused(hold.reasonCode, hold.reason, {requested: false})
        // the job's own run: its stored parameters and nothing of this request, recorded as every run is; started on a
        // thread of its own with no transaction, so the run's row is committed before the job runner writes to it
        const started = ec.service.async().name(START).disableAuthz().callFuture().get(60000) || {}
        const jobRunId = text(started.jobRunId)
        if (!jobRunId) throw new Error("The backup could not be started")
        lib.write(
            lib.RESULT,
            {
                operationKey: LEDGER,
                userId: ec.user.userId,
                operationDate: ec.user.nowTimestamp,
                resultText: JSON.stringify([nowMs].concat(requestsOf(ledger)).slice(0, 20))
            },
            false
        )
        lib.auditRow(JOB, "backupRequest", BACKUP_JOB, jobRunId, null, "Backup asked for", reason)
        return {
            applied: true,
            requested: true,
            jobRunId,
            reasonCode: null,
            reason: null,
            stateText: "A backup was started. Its outcome shows here when it ends."
        }
    })
}
// start#BackupRun: one run of the backup job, with its stored parameters. Called only by run#Backup, after its checks.
function startBackupRun() {
    lib.signedIn()
    if (!ec.user.hasPermission(RUN)) throw new Error(RUN + " is required")
    if (ec.transaction.isTransactionInPlace()) throw new Error("A backup run is started outside a transaction")
    if (!backupJob()) throw new Error("This instance has no backup job")
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        const jobRunId = text(ec.service.job(BACKUP_JOB).run())
        // the run takes the job's lock, so the schedule starts none beside it. The run is started whatever comes of this:
        // a lock that could not be taken is logged, and the request answers its run
        if (jobRunId)
            try {
                const hold = ec.service.sync().name(HOLD).parameters({jobRunId}).call() || {}
                // a run that ended before the lock named it released nothing: what it left is cleared here
                if (hold.held === true) ec.service.sync().name(HOLD).parameters({jobRunId, settle: true}).call()
                else if (text(hold.heldBy))
                    ec.logger.error(
                        "Backup run " + jobRunId + " runs beside " + hold.heldBy + ", which took the backup job's lock first"
                    )
            } catch (error) {
                ec.logger.error(
                    "Backup run " + jobRunId + " could not take the backup job's lock: " + String(error?.message ?? error)
                )
            } finally {
                ec.message.clearErrors()
            }
        return {jobRunId}
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
// hold#BackupLock: the backup job's lock for a run asked for here (the comment above). In a transaction of its own, the lock
// row read for update first and the run's after it, the order the job runner ends a run in (the lock released, then the
// run's end written). Without settle: the lock is given to the run when it is free, or names a run that ended, and the run
// has not ended. With settle: a lock that names the run is cleared when the run has ended or is gone (it ended before the
// lock was its own, so its release found nothing). Called only by start#BackupRun.
function holdBackupLock(p) {
    lib.signedIn()
    if (!ec.user.hasPermission(RUN)) throw new Error(RUN + " is required")
    const jobRunId = text(p.jobRunId)
    if (!jobRunId) throw new Error("A run is required")
    const lockRow = find(LOCK, {jobName: BACKUP_JOB}).forUpdate(true).one()
    const run = find(JOB_RUN, {jobRunId}).forUpdate(true).one()
    if (run && text(run.jobName) !== BACKUP_JOB) throw new Error("Not a run of the backup job")
    const ended = !run || run.endTime != null
    if (p.settle === true) {
        if (ended && lockRow && text(lockRow.jobRunId) === jobRunId)
            lib.write(LOCK, {jobName: BACKUP_JOB, jobRunId: null, instanceId: null}, false)
        return {held: !ended && !!lockRow && text(lockRow.jobRunId) === jobRunId, heldBy: null}
    }
    if (ended) return {held: false, heldBy: null}
    const holder = lockHolder(lockRow)
    if (holder && holder !== jobRunId) return {held: false, heldBy: holder}
    // lastRunTime as the scheduler's claim sets it: it is what lets a lock left behind expire
    lib.write(LOCK, {jobName: BACKUP_JOB, jobRunId, lastRunTime: ec.user.nowTimestamp, instanceId: null}, !lockRow)
    return {held: true, heldBy: null}
}

const operations = {getCoverage, findArchives, runBackup, startBackupRun, holdBackupLock}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown backup operation " + op)
        return operations[op](parameters)
    }
})
