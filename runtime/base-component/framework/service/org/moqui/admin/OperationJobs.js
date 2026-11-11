// The jobs part of org.moqui.admin.OperationServices (mr-boss plan/design-admin.md §10 Jobs, JobDetail, JobRuns and
// JobRunDetail, §14 "Operational recovery", §15.3 "Monitor reuse"; impl-admin P14): the instance's scheduled jobs
// (moqui.service.job.ServiceJob) and their runs for the person who operates them, what of a registered job may be changed, and
// jobs as a kind of recovery target (OperationRecovery.js). No replacement scheduler: the framework's job runner runs every
// job, under the job's own lock.
//
// What is read. Every scheduled or explicitly run job but the queued calls (ASYNC_QUEUE_ rows, which hold a caller's
// parameters and are counted only): its schedule in words and its next run in the machine's time zone, which is the zone a
// cron expression is read in; its state (paused, running, failing, late, due, a run asked for, scheduled); its last run. A
// run answers its times, where it ran, its trace id, the class of its error and masked lines of it, the names of its
// parameters and results with a value only where the value is a number, a yes or no, or a parameter its owner registered
// as safe; never a message text, a result text or another parameter's value (a job run carries no organization, so the
// operator reads the runs of every organization's jobs). A run that ended without an error but whose results or whose
// owner count failed items is "ended with failed items", never a success.
//
// What is changed (store#Job). Nothing without a registration (moqui.server.JobRegistration, its owner's seed row): an
// unregistered job is read only. With one: the schedule (a cron expression that parses and comes due no more often than
// the registration allows), the pause, the priority and the wait after an error, each only where the registration says so;
// a parameter only where its owner registered it (JobParameterDefinition) and only a value of its type within its bounds,
// a text of plain words only (no expression, path, URL or address); a notification recipient only where the registration
// names the permission a recipient must hold, since a job's notification carries the run's errors, parameters and results.
// The job's service, the identity it runs as, its topic, its time limits and its lock settings are never written here: no
// parameter of store#Job names one.
//
// A run outside the schedule (the ServiceJob recovery kind). Only where its owner registered one (RecoveryDescriptor,
// targetKind ServiceJob, targetKey the jobName). Native: the job is marked due, in a transaction of its own with its lock
// row locked and only while no run holds the lock, and the scheduler then starts it at its next pass as it starts every
// run: by its own conditional claim of the lock, as itself (no user, the operator's neither), with the stored parameters.
// So a run asked for here never runs beside one under way, and never under the identity of who asked. A paused job, one
// outside its dates and one with no schedule are not started this way. Owner: the owner's own service is asked. A lock is
// never cleared or deleted here: a run whose server stopped is ended by the scheduler itself, which takes the lock over
// when its instance is gone or after the job's expireLockTime.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/OperationLib.js")
const engine = ec.resource.script("component://framework/service/org/moqui/admin/OperationRecovery.js")
const messages = ec.resource.script("component://framework/service/org/moqui/admin/OperationMessages.js")
const {find, one, rows, map, ms, iso, stamp, text, refused} = lib
const safeText = messages.safeText
const JOB = "moqui.service.job.ServiceJob"
const PARAMETER = "moqui.service.job.ServiceJobParameter"
const JOB_USER = "moqui.service.job.ServiceJobUser"
const RUN = "moqui.service.job.ServiceJobRun"
const LOCK = "moqui.service.job.ServiceJobRunLock"
const REGISTRATION = "moqui.server.JobRegistration"
const DEFINITION = "moqui.server.JobParameterDefinition"
const ACCOUNT = "moqui.security.UserAccount"
const APPLY = "org.moqui.admin.OperationServices.apply#JobRecovery"
// a stored queued call (jobs.rs QUEUE_PREFIX): never listed, read or changed here
const QUEUED = "ASYNC_QUEUE_"
// a job due for longer than this and not started is late (the scheduler looks every minute)
const lateMinutes = 10
// the framework's defaults (jobs.rs)
const defaultRetryMinutes = 5
const defaultExpireMinutes = 1440
// with no dates given, the run list reads this far back; a range is at most rangeDays
const defaultDays = 7
const rangeDays = 92
const maxItems = 200
const valueTypes = ["integer", "decimal", "boolean", "option", "text"]
const secretName =
    /pass(word|wd|code|phrase)?$|secret|token|credential|api[-_]?key|private[-_]?key|signature|\bpin$|pin(code|number)$/i
// a stored text shown as a number: a count, a limit or a setting, never a long run of digits (an account, a card, a phone)
const shortNumber = /^-?\d{1,9}(\.\d{1,6})?$/

const now = () => ms(ec.user.nowTimestamp)
const zone = () => text(ec.user.machineTimeZone) || "UTC"
const zoned = value => (value == null ? "" : ec.l10n.format(Number(value), "yyyy-MM-dd HH:mm", null, zone()))
const isQueued = jobName => text(jobName).startsWith(QUEUED)
const yes = value => value === "Y"
// a decimal field is a host decimal, which is read as a number through toNumber
const number = value => (value == null ? null : typeof value.toNumber === "function" ? value.toNumber() : Number(value))
const jobFind = () => find(JOB).condition("jobName", "not-like", QUEUED + "%")

// ===== registrations =====
// A job's registration, or null: none, disabled, or one naming a permission that does not exist (logged, left out).
function registrationOf(jobName) {
    const row = one(REGISTRATION, {jobName})
    if (!row || row.disabled === "Y") return null
    const registration = map(row)
    for (const field of ["managePermission", "recipientPermission"]) {
        const permission = text(registration[field])
        if (permission && !one("moqui.security.UserPermission", {userPermissionId: permission})) {
            ec.logger.error("Job registration " + jobName + " names the unknown permission " + permission + "; left out")
            return null
        }
    }
    return registration
}
// The parameters of a job its owner registered as safe to change, in order: each checked when read, and one of an unknown
// type, one named like a secret and one the job's service does not declare are logged by name and left out.
function definitionsOf(job) {
    const declared = ec.service.getServiceDefinition(text(job.serviceName))?.inParameterNames
    const names = declared ? Array.from(declared).map(String) : null
    const found = []
    for (const row of rows(DEFINITION, {jobName: job.jobName})) {
        const definition = map(row)
        const name = text(definition.parameterName)
        const problem = !valueTypes.includes(text(definition.valueType))
            ? "has an unknown type"
            : secretName.test(name)
              ? "is named like a secret"
              : !names || !names.includes(name)
                ? "is not a parameter of the job's service"
                : text(definition.valueType) === "option" && !optionsOf(definition).length
                  ? "has no options"
                  : ""
        if (problem) {
            ec.logger.error("Job parameter definition " + job.jobName + "." + name + " " + problem + "; left out")
            continue
        }
        found.push(definition)
    }
    return found.sort(
        (a, b) => Number(a.sequenceNum ?? 9999) - Number(b.sequenceNum ?? 9999) || a.parameterName.localeCompare(b.parameterName)
    )
}
const optionsOf = definition =>
    text(definition.optionValues)
        .split(",")
        .map(text)
        .filter(option => /^[\w .:-]{1,60}$/.test(option))

// ===== a job's schedule and state =====
// The latest time at or before now a cron expression came due, as epoch millis, or null when it never has: found by
// halving, with the scheduler's own test of a last run against the last schedule.
function lastSchedule(cron, nowMs) {
    const before = moment => ec.service.isLastRunBeforeLastSchedule(cron, moment, null, nowMs) === true
    if (!before(1)) return null
    let low = 1
    let high = nowMs + 1
    while (high - low > 1) {
        const middle = Math.floor((low + high) / 2)
        if (before(middle)) low = middle
        else high = middle
    }
    return high
}
// The newest run of a job, or null.
const latestRun = jobName => {
    const row = find(RUN, {jobName}).orderBy("-startTime").limit(1).list()[0]
    return row ? map(row) : null
}
// Whether a job's newest run is a failure nothing has run past (a job that keeps only its failed runs moves its lock's
// last run time past an old error when it succeeds, jobs.rs readiness).
function failing(lock, run) {
    if (!run || run.hasError !== "Y") return false
    const ended = ms(run.endTime) ?? ms(run.startTime)
    const lastRun = ms(lock?.lastRunTime)
    return !(ended != null && lastRun != null && lastRun >= ended)
}
// A job with its lock and, when read, its newest run: its schedule and its state now.
function scheduleOf(job, lock, run, nowMs) {
    const cron = text(job.cronExpression)
    const lastRunMs = ms(lock?.lastRunTime)
    const running = !!text(lock?.jobRunId)
    const out = {
        cron,
        scheduleText: "",
        cronValid: true,
        nextRun: null,
        lastRunMs,
        running,
        due: false,
        requested: false,
        lateSince: null
    }
    if (cron) {
        try {
            out.scheduleText = text(ec.service.getCronDescription(cron))
            out.nextRun = ec.service.nextCronExecution(cron, nowMs)
            out.due = ec.service.isLastRunBeforeLastSchedule(cron, lastRunMs, null, nowMs) === true
        } catch (error) {
            out.cronValid = false
        }
    }
    const from = ms(job.fromDate)
    const thru = ms(job.thruDate)
    const active = (from == null || from <= nowMs) && (thru == null || thru >= nowMs)
    // as the scheduler reads it: only an absent value or N is not paused
    const paused = !(job.paused == null || job.paused === "N")
    const isFailing = failing(lock, run)
    let state
    if (!cron) state = "manual"
    else if (!out.cronValid) state = "invalid"
    else if (running) state = "running"
    else if (!active) state = "ended"
    else if (paused) state = "paused"
    else if (out.due) {
        const last = lastSchedule(cron, nowMs)
        // a run asked for here leaves the lock a millisecond before the last schedule (applyJobRecovery)
        out.requested = last != null && lastRunMs === last - 1
        const retryUntil = isFailing
            ? (ms(run.endTime) ?? ms(run.startTime) ?? 0) + Number(job.minRetryTime || defaultRetryMinutes) * 60000
            : 0
        const dueSince = Math.max(last ?? nowMs, retryUntil)
        if (out.requested) state = "requested"
        else if (isFailing && retryUntil > nowMs) state = "failing"
        else if (nowMs - dueSince > lateMinutes * 60000) {
            state = "late"
            out.lateSince = dueSince
        } else state = isFailing ? "failing" : "due"
    } else state = isFailing ? "failing" : "scheduled"
    out.state = state
    out.paused = paused
    out.active = active
    out.nextRun = ["scheduled", "failing", "due", "late", "requested", "running"].includes(state) ? out.nextRun : null
    out.stateText = stateTextOf(state, out, run)
    return out
}
function stateTextOf(state, schedule, run) {
    switch (state) {
        case "manual":
            return "No schedule: started by its owner"
        case "invalid":
            return "Its schedule cannot be read"
        case "running":
            return "Running" + (schedule.lastRunMs != null ? " since " + zoned(schedule.lastRunMs) : "")
        case "ended":
            return "Outside its dates"
        case "paused":
            return "Paused"
        case "requested":
            return "A run was asked for: starts at the scheduler's next pass"
        case "late":
            return "Late: due since " + zoned(schedule.lateSince)
        case "due":
            return "Due: starts at the scheduler's next pass"
        case "failing":
            return "Failing" + (run?.startTime ? ": its last run, " + zoned(ms(run.startTime)) + ", failed" : "")
        default:
            return "Scheduled"
    }
}
const stateList = [
    {state: "all", text: "All"},
    {state: "attention", text: "Failing or late"},
    {state: "failing", text: "Failing"},
    {state: "late", text: "Late"},
    {state: "paused", text: "Paused"},
    {state: "running", text: "Running"}
]
// A job's revision for a change of its configuration: the job's own row, its parameters and its recipients.
function configRevision(job, parameters, recipients) {
    const newest = list => list.reduce((most, row) => Math.max(most, ms(row.lastUpdatedStamp) ?? 0), 0)
    return [stamp(job), parameters.length, newest(parameters), recipients.length, newest(recipients)].join(".")
}
// A job's revision for a run outside its schedule: its row and what its lock holds.
const runRevision = (job, lock) => [stamp(job), text(lock?.jobRunId) || "-", ms(lock?.lastRunTime) ?? "-"].join(".")

// The class of a run's failure, for telling causes apart without reading the error.
function errorClassOf(value) {
    const found = text(value)
    if (!found) return ""
    if (/server instance stopped|pool stopped before/i.test(found)) return "Its server stopped"
    if (/panicked/i.test(found)) return "The job's code failed unexpectedly"
    if (/cannot sign in; the job did not run/i.test(found)) return "The user it runs as cannot sign in"
    if (/queue is full/i.test(found)) return "The job queue was full"
    if (/interrupted|timed? ?out|time limit/i.test(found)) return "Time limit"
    if (/not authorized|requires login|permission|authz/i.test(found)) return "Authorization"
    if (/connect|resolve|unreachable|certificate|handshake|\btls\b|\bdns\b/i.test(found)) return "Connection"
    return "Processing"
}

// ===== the jobs =====
// A job as the list and the detail show it.
function described(job, lock, run, registration, descriptor, nowMs, runKnown = true) {
    const schedule = scheduleOf(job, lock, run, nowMs)
    return {
        jobName: job.jobName,
        title: text(registration?.title) || text(job.description) || job.jobName,
        description: text(job.description) || null,
        purpose: text(registration?.description) || null,
        registered: !!registration,
        ownerApp: text(registration?.ownerApp) || null,
        ownerText: registration ? text(registration.ownerApp) || "Registered" : "Not registered",
        ownerRoute: text(registration?.ownerRoute) || null,
        serviceName: text(job.serviceName) || null,
        cronExpression: schedule.cron || null,
        scheduleText: schedule.scheduleText || (schedule.cron ? "" : "No schedule"),
        paused: schedule.paused,
        priority: job.priority == null ? null : Number(job.priority),
        state: schedule.state,
        stateText: schedule.stateText,
        nextRun: schedule.nextRun == null ? null : iso(schedule.nextRun),
        nextRunText: schedule.nextRun == null ? "" : zoned(schedule.nextRun),
        timeZone: zone(),
        lastRunTime: schedule.lastRunMs == null ? null : iso(schedule.lastRunMs),
        lastRunText: schedule.state === "requested" ? "" : zoned(schedule.lastRunMs),
        activeJobRunId: text(lock?.jobRunId) || null,
        lastJobRunId: runKnown ? run?.jobRunId || null : null,
        lastRunFailed: runKnown ? run?.hasError === "Y" : null,
        lastErrorClass: runKnown && run?.hasError === "Y" ? errorClassOf(run.errors) : "",
        manualRunRegistered: !!text(descriptor?.retryMode),
        topic: text(job.topic) || null
    }
}
const lockMap = () => new Map(Array.from(find(LOCK).limit(5000).list()).map(row => [row.jobName, map(row)]))

// find#Jobs: the jobs, by name, a page at a time, each with its schedule, state, owner and last run.
function findJobs(p) {
    lib.requireView()
    const canManage = ec.user.hasPermission(lib.MANAGE)
    const {pageIndex, pageSize} = lib.pageOf(p)
    const state = text(p.state) || "all"
    if (!stateList.some(option => option.state === state)) throw new Error("Unknown job state " + state)
    const term = text(p.term).toLowerCase()
    const ownerApp = text(p.ownerApp)
    const nowMs = now()
    const registrations = new Map(
        rows(REGISTRATION)
            .filter(row => row.disabled !== "Y")
            .map(row => [row.jobName, map(row)])
    )
    const descriptors = engine.descriptorsOf(recoveryKind.targetKind)
    const locks = lockMap()
    // the jobs with a failed run of the last five weeks: only those are asked for their newest run before the page is cut
    const failedLately = new Set(
        Array.from(
            find(RUN)
                .condition("hasError", "Y")
                .condition("startTime", "greater-equals", iso(nowMs - 35 * 86400000))
                .selectField("jobName")
                .distinct(true)
                .limit(2000)
                .list()
        ).map(row => row.jobName)
    )
    const runs = new Map()
    const runOf = jobName => {
        if (!runs.has(jobName)) runs.set(jobName, latestRun(jobName))
        return runs.get(jobName)
    }
    let jobs = Array.from(jobFind().orderBy("jobName").limit(2000).list())
        .map(map)
        .filter(job => !isQueued(job.jobName))
    if (term)
        jobs = jobs.filter(job =>
            [job.jobName, job.description, registrations.get(job.jobName)?.title].some(value =>
                text(value).toLowerCase().includes(term)
            )
        )
    if (ownerApp === "_none") jobs = jobs.filter(job => !registrations.has(job.jobName))
    else if (ownerApp) jobs = jobs.filter(job => text(registrations.get(job.jobName)?.ownerApp) === ownerApp)
    const describedJob = job =>
        described(
            job,
            locks.get(job.jobName),
            failedLately.has(job.jobName) || runs.has(job.jobName) ? runOf(job.jobName) : null,
            registrations.get(job.jobName),
            engine.descriptorFor(descriptors, {targetKey: job.jobName}),
            nowMs,
            failedLately.has(job.jobName) || runs.has(job.jobName)
        )
    const all = jobs.map(job => ({job, row: describedJob(job)}))
    const counts = {failing: 0, late: 0, paused: 0}
    for (const found of all) if (Object.hasOwn(counts, found.row.state)) counts[found.row.state]++
    const wanted = state === "attention" ? ["failing", "late"] : state === "all" ? null : [state]
    const listed = wanted ? all.filter(found => wanted.includes(found.row.state)) : all
    // the page's own rows read their newest run too, for the last outcome
    const pageRows = listed.slice(pageIndex * pageSize, (pageIndex + 1) * pageSize).map(found => {
        if (runs.has(found.job.jobName)) return found.row
        runOf(found.job.jobName)
        return describedJob(found.job)
    })
    return {
        jobList: pageRows,
        jobCount: listed.length,
        pageIndex,
        pageSize,
        canManage,
        stateList,
        ownerList: Array.from(new Set(Array.from(registrations.values()).map(row => text(row.ownerApp))))
            .filter(Boolean)
            .sort(),
        failingCount: counts.failing,
        lateCount: counts.late,
        pausedCount: counts.paused,
        queuedCallCount: Number(
            find(JOB)
                .condition("jobName", "like", QUEUED + "%")
                .count()
        ),
        timeZone: zone()
    }
}

// One job with everything a change or a run is decided on.
function loadJob(jobName) {
    const name = text(jobName)
    const row = name && !isQueued(name) ? one(JOB, {jobName: name}) : null
    if (!row) return null
    const job = map(row)
    const registration = registrationOf(name)
    return {
        job,
        lock: one(LOCK, {jobName: name}) ? map(one(LOCK, {jobName: name})) : null,
        registration,
        definitions: registration ? definitionsOf(job) : [],
        parameters: rows(PARAMETER, {jobName: name}).map(map),
        recipients: rows(JOB_USER, {jobName: name}).map(map)
    }
}
const lockOf = loaded => loaded.lock
// A parameter's stored value as shown: whole only where its owner registered it as safe.
function parameterRows(loaded) {
    const stored = new Map(loaded.parameters.map(row => [row.parameterName, row]))
    const registered = loaded.definitions.map(definition => {
        const value = stored.get(definition.parameterName)?.parameterValue
        return {
            parameterName: definition.parameterName,
            title: text(definition.title) || definition.parameterName,
            description: text(definition.description) || null,
            valueType: definition.valueType,
            value: value == null ? null : String(value),
            valueText: value == null ? "The service's default" : String(value),
            boundsText: boundsText(definition),
            optionList: definition.valueType === "option" ? optionsOf(definition) : [],
            required: yes(definition.required),
            editable: true,
            sourceText: value == null ? "Default" : "Set for this job"
        }
    })
    const names = new Set(loaded.definitions.map(definition => definition.parameterName))
    const others = loaded.parameters
        .filter(row => !names.has(row.parameterName))
        .map(row => ({
            parameterName: row.parameterName,
            title: row.parameterName,
            description: null,
            valueType: null,
            value: null,
            valueText: shownValue(row.parameterName, row.parameterValue),
            boundsText: "",
            optionList: [],
            required: false,
            editable: false,
            sourceText: "Its owner's deployment"
        }))
    return registered.concat(others.sort((a, b) => a.parameterName.localeCompare(b.parameterName)))
}
function boundsText(definition) {
    const type = definition.valueType
    if (type === "option") return "One of: " + optionsOf(definition).join(", ")
    if (type === "boolean") return "true or false"
    if (type === "text") return "Plain words, at most " + maxLengthOf(definition) + " characters"
    const low = number(definition.minValue)
    const high = number(definition.maxValue)
    const kind = type === "integer" ? "A whole number" : "A number"
    return low != null && high != null
        ? kind + " from " + low + " to " + high
        : low != null
          ? kind + ", at least " + low
          : high != null
            ? kind + ", at most " + high
            : kind
}
const maxLengthOf = definition => Math.min(Math.max(Number(definition.maxLength) || 60, 1), 255)
// A value nobody registered as safe: a short number or a yes or no is shown, anything else is not, and nothing of a
// parameter named like a secret.
const shownValue = (name, value) => {
    const found = text(value)
    if (!found) return ""
    return !secretName.test(text(name)) && (shortNumber.test(found) || /^(true|false|Y|N)$/.test(found)) ? found : "(not shown)"
}
// Whether a user holds a permission now, by the user's groups (and ALL_USERS).
function userHolds(userId, permission) {
    const nowMs = now()
    const within = (from, thru) => (ms(from) == null || ms(from) <= nowMs) && (ms(thru) == null || ms(thru) > nowMs)
    const held = rows("moqui.security.UserPermissionCheck", {userId, userPermissionId: permission}).some(
        row => within(row.groupFromDate, row.groupThruDate) && within(row.permissionFromDate, row.permissionThruDate)
    )
    return (
        held ||
        rows("moqui.security.UserGroupPermission", {userGroupId: "ALL_USERS", userPermissionId: permission}).some(row =>
            within(row.fromDate, row.thruDate)
        )
    )
}
// An account that may sign in: not disabled, not past its end date.
const accountOpen = account =>
    !!account && account.disabled !== "Y" && (ms(account.terminateDate) == null || ms(account.terminateDate) > now())
const accountName = account => text(account?.userFullName) || text(account?.username) || text(account?.userId)

// get#Job: one job's configuration as it may be changed here, and its run outside the schedule.
function getJob(p) {
    lib.requireView()
    const canManage = ec.user.hasPermission(lib.MANAGE)
    const loaded = loadJob(p.jobName)
    if (!loaded) throw new Error("No job " + text(p.jobName))
    const {job, registration} = loaded
    const lock = lockOf(loaded)
    const nowMs = now()
    const target = targetOf(job, lock, nowMs)
    const assessed = engine.assess(recoveryKind, [target]).get(job.jobName)
    const ownerPermission = text(registration?.managePermission)
    const ownerHeld = !ownerPermission || ec.user.hasPermission(ownerPermission)
    const may = flag => canManage && !!registration && ownerHeld && yes(registration[flag])
    const recipientPermission = text(registration?.recipientPermission)
    const accounts = new Map(
        loaded.recipients.length
            ? Array.from(
                  find(ACCOUNT)
                      .condition(
                          "userId",
                          "in",
                          loaded.recipients.map(row => row.userId)
                      )
                      .list()
              ).map(row => [row.userId, map(row)])
            : []
    )
    return {
        job: Object.assign(described(job, lock, latestRun(job.jobName), registration, assessed.descriptor, nowMs), {
            minRetryTime: Number(job.minRetryTime || defaultRetryMinutes),
            expireLockTime: Number(job.expireLockTime || defaultExpireMinutes),
            transactionTimeout: job.transactionTimeout == null ? null : Number(job.transactionTimeout),
            minIntervalMinutes: Math.max(Number(registration?.minIntervalMinutes) || 1, 1),
            scheduleEditable: may("scheduleEditable") && !!text(job.cronExpression),
            pauseEditable: may("pauseEditable"),
            priorityEditable: may("priorityEditable"),
            retryEditable: may("retryEditable"),
            parametersEditable: canManage && !!registration && ownerHeld,
            recipientsEditable: canManage && !!registration && ownerHeld && !!recipientPermission,
            recipientPermission: recipientPermission || null,
            ownerPermission: ownerPermission || null,
            readOnlyReason: !registration
                ? "Its owner registered nothing an administrator changes here: it is its owner's deployment, or an instance administrator's in Tools"
                : !ownerHeld
                  ? "Changing it also needs its owner's own permission (" + ownerPermission + ")"
                  : !canManage
                    ? lib.MANAGE + " is required to change it"
                    : null,
            revision: configRevision(job, loaded.parameters, loaded.recipients),
            runRevision: target.revision
        }),
        parameterList: parameterRows(loaded),
        recipientList: loaded.recipients
            .map(row => ({
                userId: row.userId,
                username: text(accounts.get(row.userId)?.username) || row.userId,
                name: accountName(accounts.get(row.userId)) || row.userId,
                receiveNotifications: row.receiveNotifications !== "N",
                open: accountOpen(accounts.get(row.userId)),
                eligible: !!recipientPermission && userHolds(row.userId, recipientPermission)
            }))
            .sort((a, b) => a.name.localeCompare(b.name)),
        actionList: engine.actionList(recoveryKind, target, assessed, canManage),
        registration: engine.registrationOf(assessed),
        canManage
    }
}

// ===== store#Job =====
// A cron expression as it may be set here: one that parses and comes due no more often than the registration allows, by
// its next occurrences.
function checkedCron(value, minMinutes) {
    const cron = lib.plainText(value, "The schedule", 60)
    if (!/^[\w*?,\/#\- ]+$/.test(cron)) return {problem: "The schedule is not a cron expression"}
    let next
    try {
        ec.service.getCronDescription(cron)
        next = ec.service.nextCronExecution(cron, now())
    } catch (error) {
        return {problem: "The schedule is not a cron expression the scheduler reads"}
    }
    if (next == null) return {problem: "That schedule never comes due again"}
    let shortest = null
    let moment = next
    for (let count = 0; count < 30; count++) {
        const following = ec.service.nextCronExecution(cron, moment)
        if (following == null) break
        shortest = shortest == null ? following - moment : Math.min(shortest, following - moment)
        moment = following
    }
    if (shortest != null && shortest < minMinutes * 60000)
        return {problem: "That schedule runs more often than every " + minMinutes + " minutes, the least its owner allows here"}
    return {cron}
}
// A parameter's value as its definition takes it: {value} the text stored, or {problem}.
function checkedValue(definition, given) {
    const found = text(given)
    const title = text(definition.title) || definition.parameterName
    const no = problem => ({problem: title + ": " + problem})
    switch (definition.valueType) {
        case "integer":
        case "decimal": {
            if (!(definition.valueType === "integer" ? /^-?\d{1,9}$/ : /^-?\d{1,9}(\.\d{1,6})?$/).test(found))
                return no(definition.valueType === "integer" ? "a whole number is needed" : "a number is needed")
            const low = number(definition.minValue)
            const high = number(definition.maxValue)
            if (low != null && Number(found) < low) return no("at least " + low)
            if (high != null && Number(found) > high) return no("at most " + high)
            return {value: found}
        }
        case "boolean":
            return ["true", "false"].includes(found) ? {value: found} : no("true or false is needed")
        case "option":
            return optionsOf(definition).includes(found) ? {value: found} : no("it is not one of its options")
        default:
            // plain words only: nothing that reads as an expression, a path, a URL or an address
            if (found.length > maxLengthOf(definition)) return no("at most " + maxLengthOf(definition) + " characters")
            return /^[\p{L}\p{N} .,:_-]+$/u.test(found) && !found.includes("..")
                ? {value: found}
                : no("only letters, digits, spaces and . , : _ - may be used")
    }
}
const given = value => value !== undefined && value !== null

// store#Job: the changes a registration allows, each checked before any is written; all or none.
function storeJob(p) {
    lib.requireManage()
    // the job's row is locked before it is read, so two changes at one revision are taken one after the other and the
    // second finds the revision moved (its parameters and recipients are written only under this lock)
    if (text(p.jobName) && !isQueued(p.jobName))
        find(JOB, {jobName: text(p.jobName)})
            .forUpdate(true)
            .one()
    const loaded = loadJob(p.jobName)
    if (!loaded) throw new Error("No job " + text(p.jobName))
    const {job, registration} = loaded
    const reason = lib.reasonText(p)
    if (reason) lib.plainText(reason, "The reason")
    const revision = configRevision(job, loaded.parameters, loaded.recipients)
    if (!registration)
        return refused(
            "NOT_REGISTERED",
            "Its owner registered nothing an administrator changes here: it is its owner's deployment, or an instance administrator's in Tools",
            {revision}
        )
    const ownerPermission = text(registration.managePermission)
    if (ownerPermission && !ec.user.hasPermission(ownerPermission))
        return refused("OWNER_PERMISSION", "This also needs its owner's own permission (" + ownerPermission + ")", {revision})
    if (text(p.expectedRevision) !== revision)
        return refused("STALE_REVISION", "It changed since it was read; read it again and repeat", {revision})
    const no = (reasonCode, problem) => refused(reasonCode, problem, {revision})
    const jobFields = {}
    const audits = []
    const changed = []

    // --- the scheduling controls ---
    if (given(p.cronExpression) && text(p.cronExpression) !== text(job.cronExpression)) {
        if (!yes(registration.scheduleEditable)) return no("NOT_EDITABLE", "Its schedule is its owner's and is not changed here")
        // a job its owner runs by hand is not given a schedule here, and a schedule is never taken away: pause it instead
        if (!text(job.cronExpression)) return no("NOT_EDITABLE", "It has no schedule: its owner starts it")
        if (!text(p.cronExpression)) return no("INVALID_SCHEDULE", "A schedule is not cleared: pause the job instead")
        const checked = checkedCron(p.cronExpression, Math.max(Number(registration.minIntervalMinutes) || 1, 1))
        if (checked.problem) return no("INVALID_SCHEDULE", checked.problem)
        jobFields.cronExpression = checked.cron
        audits.push(["cronExpression", text(job.cronExpression), checked.cron])
        changed.push("schedule")
    }
    if (given(p.paused) && text(p.paused)) {
        const wanted = text(p.paused)
        if (!["Y", "N"].includes(wanted)) throw new Error("Paused is Y or N")
        const paused = !(job.paused == null || job.paused === "N")
        if ((wanted === "Y") !== paused) {
            if (!yes(registration.pauseEditable)) return no("NOT_EDITABLE", "Its owner does not let it be paused or resumed here")
            jobFields.paused = wanted
            changed.push(wanted === "Y" ? "paused" : "resumed")
        }
    }
    const numberField = (name, flag, title, low, high) => {
        if (!given(p[name]) || text(p[name]) === "") return null
        if (!/^\d{1,6}$/.test(text(p[name])) || Number(p[name]) < low || Number(p[name]) > high)
            return no("INVALID_VALUE", title + " is a whole number from " + low + " to " + high)
        const wanted = Number(p[name])
        // a wait that is not set is the framework's default, which a form posts back: no change
        const current = job[name] != null ? Number(job[name]) : name === "minRetryTime" ? defaultRetryMinutes : null
        if (current === wanted) return null
        if (!yes(registration[flag])) return no("NOT_EDITABLE", title + " is its owner's and is not changed here")
        jobFields[name] = wanted
        audits.push([name, job[name] == null ? null : String(Number(job[name])), String(wanted)])
        changed.push(name)
        return null
    }
    const held =
        numberField("priority", "priorityEditable", "The priority", 0, 999) ||
        numberField("minRetryTime", "retryEditable", "The wait after an error", 1, 1440)
    if (held) return held

    // --- a registered parameter ---
    const parameterWrites = []
    const parameterMap = p.parameterMap ? map(p.parameterMap) : {}
    if (text(p.parameterName)) parameterMap[text(p.parameterName)] = p.parameterValue
    const stored = new Map(loaded.parameters.map(row => [row.parameterName, row]))
    for (const name of Object.keys(parameterMap)) {
        const definition = loaded.definitions.find(found => found.parameterName === name)
        if (!definition)
            return no(
                "NOT_REGISTERED_PARAMETER",
                "The parameter " + safeText(name).slice(0, 60) + " is not one its owner registered as changed here"
            )
        const current = stored.get(name)?.parameterValue
        if (text(parameterMap[name]) === "") {
            if (current == null) continue
            if (yes(definition.required)) return no("INVALID_VALUE", (text(definition.title) || name) + " is not cleared")
            parameterWrites.push({name, value: null})
            audits.push(["parameter." + name, String(current), null])
        } else {
            const checked = checkedValue(definition, parameterMap[name])
            if (checked.problem) return no("INVALID_VALUE", checked.problem)
            if (current != null && String(current) === checked.value) continue
            parameterWrites.push({name, value: checked.value, create: current == null})
            audits.push(["parameter." + name, current == null ? null : String(current), checked.value])
        }
        changed.push("parameter " + name)
    }

    // --- a notification recipient ---
    let recipientWrite = null
    const username = text(p.recipientUsername)
    if (username) {
        const permission = text(registration.recipientPermission)
        if (!permission) return no("NOT_EDITABLE", "Its owner registered nobody as its recipients here")
        const action = text(p.recipientAction) || "add"
        if (!["add", "mute", "remove"].includes(action)) throw new Error("A recipient is added, muted or removed")
        const account = map(one(ACCOUNT, {username: username.toLowerCase()}) || one(ACCOUNT, {username}) || null)
        const current = account.userId ? loaded.recipients.find(row => row.userId === account.userId) : null
        if (action === "remove" || action === "mute") {
            // taking someone off narrows who is told: anyone listed, whatever they hold now
            if (!current) return no("NOT_A_RECIPIENT", "That user is not one of its recipients")
            recipientWrite = {userId: account.userId, action}
        } else {
            // one answer for an unknown name and for a user outside the audience, so names are not found out here
            if (!accountOpen(account) || !userHolds(account.userId, permission))
                return no(
                    "NOT_ELIGIBLE",
                    "No user of that name may receive this job's notifications: a recipient is an account that signs in and holds " +
                        permission
                )
            if (current && current.receiveNotifications !== "N")
                return no("ALREADY_A_RECIPIENT", "That user is one of its recipients already")
            recipientWrite = {userId: account.userId, action, create: !current}
        }
        audits.push([
            "recipient." + recipientWrite.userId,
            current ? (current.receiveNotifications === "N" ? "muted" : "recipient") : null,
            action === "add" ? "recipient" : action === "mute" ? "muted" : null
        ])
        changed.push("recipient")
    }
    if (!changed.length) return {applied: true, changedList: [], revision, nextRunText: ""}

    // --- write ---
    if (Object.keys(jobFields).length) {
        if (jobFields.paused && reason) lib.auditReason(reason, ["paused"])
        const row = find(JOB, {jobName: job.jobName}).forUpdate(true).one()
        row.setFields(jobFields, true, null, null)
        row.update()
    }
    for (const write of parameterWrites) {
        if (write.value == null) one(PARAMETER, {jobName: job.jobName, parameterName: write.name})?.delete()
        else lib.write(PARAMETER, {jobName: job.jobName, parameterName: write.name, parameterValue: write.value}, write.create)
    }
    if (recipientWrite) {
        if (recipientWrite.action === "remove") one(JOB_USER, {jobName: job.jobName, userId: recipientWrite.userId})?.delete()
        else
            lib.write(
                JOB_USER,
                {
                    jobName: job.jobName,
                    userId: recipientWrite.userId,
                    receiveNotifications: recipientWrite.action === "mute" ? "N" : "Y"
                },
                recipientWrite.create === true
            )
    }
    for (const [field, before, after] of audits) lib.auditRow(JOB, field, job.jobName, null, before, after, reason)
    const afterwards = loadJob(job.jobName)
    const schedule = scheduleOf(afterwards.job, lockOf(afterwards), null, now())
    return {
        applied: true,
        changedList: changed,
        revision: configRevision(afterwards.job, afterwards.parameters, afterwards.recipients),
        state: schedule.state,
        stateText: schedule.stateText,
        nextRun: schedule.nextRun == null ? null : iso(schedule.nextRun),
        nextRunText: schedule.nextRun == null ? "" : zoned(schedule.nextRun) + " (" + zone() + ")"
    }
}

// ===== a job as a recovery target =====
function targetOf(job, lock, nowMs) {
    const run = latestRun(job.jobName)
    const schedule = scheduleOf(job, lock, run, nowMs)
    return {
        targetId: job.jobName,
        targetKey: job.jobName,
        direction: "",
        title: "Job " + job.jobName,
        state: schedule.state,
        stateText: schedule.stateText,
        revision: runRevision(job, lock),
        settled: false,
        hasLock: !!lock,
        cron: schedule.cron,
        failing: failing(lock, run),
        retryMinutes: Number(job.minRetryTime || defaultRetryMinutes)
    }
}
function read(ids) {
    const found = new Map()
    const nowMs = now()
    for (const jobName of ids) {
        if (isQueued(jobName)) continue
        const row = one(JOB, {jobName})
        if (!row) continue
        const lockRow = one(LOCK, {jobName})
        found.set(jobName, targetOf(map(row), lockRow ? map(lockRow) : null, nowMs))
    }
    return found
}
const actions = {
    retry: {label: "Run now", noun: "run outside its schedule", modeField: "retryMode", ownerField: "retryEligible"}
}
// What never holds for a run outside the schedule, whatever is registered: {reasonCode, reason}, or null.
function baseline(target, action, descriptor) {
    const hold = (reasonCode, reason) => ({reasonCode, reason})
    if (target.uncertain)
        return hold(
            "OWNER_RECONCILES",
            "Its owner says the outcome of its last run is unknown: that is settled in the owner's own place first"
        )
    if (target.state === "running")
        return hold(
            "IN_PROGRESS",
            target.stateText +
                ". A second run is not started beside it, and its lock is not cleared here: if its server stopped, the scheduler takes the lock over itself"
        )
    if (text(descriptor?.retryMode) !== "native") return null
    switch (target.state) {
        case "manual":
            return hold("NOT_SCHEDULED", "It has no schedule the scheduler starts it by: its owner starts it")
        case "invalid":
            return hold("NOT_SCHEDULED", "Its schedule cannot be read, so the scheduler does not start it")
        case "paused":
            return hold("PAUSED", "It is paused: resume it first")
        case "ended":
            return hold("NOT_ACTIVE", "It is outside the dates it runs in")
        case "requested":
            return hold("ALREADY_DUE", "A run was asked for already: the scheduler starts it at its next pass")
        case "due":
        case "late":
            return hold("ALREADY_DUE", "It is due already: the scheduler starts it at its next pass")
        default:
            return target.hasLock ? null : hold("ALREADY_DUE", "It has not run yet and is due at the scheduler's next pass")
    }
}
// One step on a job in a transaction of its own (apply#JobRecovery): whether it was taken.
const step = (target, name) => {
    const taken = lib.callRegistered(APPLY, {jobName: target.targetId, expectedRevision: target.revision, step: name})
    if (taken.failure) throw new Error(taken.failure)
    return taken.out.taken === true
}
const stale = () => refused("STALE_REVISION", "It ran or changed since it was read; read it again and repeat")
function after(jobName) {
    const target = read([jobName]).get(jobName)
    return {state: target?.state, stateText: target?.stateText, revision: target?.revision}
}
function consequence(target, action, descriptor) {
    const note = text(descriptor?.idempotencyNote)
    if (text(descriptor?.retryMode) === "owner")
        return engine.ownerName(descriptor) + " starts it its own way, under its own checks." + (note ? " " + note : "")
    return (
        "The job is marked due, and the scheduler starts one run at its next pass: under the job's own lock, as the scheduler runs it and not as you, with its stored parameters. It never starts beside a run under way." +
        (target.failing
            ? " Its last run failed, so the scheduler first waits the job's " + target.retryMinutes + " minutes after an error."
            : "") +
        (note ? " " + note : "")
    )
}
const recoveryKind = {
    targetKind: "ServiceJob",
    noun: "job",
    actions,
    read,
    baseline,
    nativeApply: target => (step(target, "request") ? {applied: true} : stale()),
    // an owner's run: only while the job is as it was read and no run holds its lock; nothing is written here
    claim: target => step(target, "check"),
    after,
    consequence,
    resolutionOf: (target, resolution) => ({resolution, label: resolution}),
    uncertainText: () => "Outcome unknown at its owner",
    safeText,
    audit: (target, afterwards, reasonText) =>
        lib.auditRow(
            JOB,
            "runRequest",
            target.targetId,
            null,
            target.stateText,
            afterwards.stateText || null,
            reasonText.slice(0, 255)
        )
}

// apply#JobRecovery: one step on a job in a transaction of its own, its lock row locked, still as it was read and with no
// run holding the lock. check takes nothing (an owner's own run follows). request marks the job due: the lock's last run
// time is set a millisecond before the schedule's last occurrence, which is what the scheduler compares, so its next pass
// claims the lock and runs the job as it runs every run. The lock's run is never cleared and the row never deleted.
function applyJobRecovery(p) {
    if (!ec.artifactExecution.authzDisabled() && !ec.user.hasPermission(lib.MANAGE)) throw new Error(lib.MANAGE + " is required")
    const jobName = text(p.jobName)
    const name = text(p.step)
    if (!["check", "request"].includes(name)) throw new Error("Unknown recovery step " + name)
    if (isQueued(jobName)) return {taken: false}
    // the job's row is locked too, so a pause or a change of its schedule (store#Job) is not made between this read and the mark
    const jobRow = find(JOB, {jobName}).forUpdate(true).one()
    if (!jobRow) return {taken: false}
    // whoever calls this step: a job is marked due only where its owner registered a native run
    if (
        name === "request" &&
        text(
            engine.descriptorFor(engine.descriptorsOf(recoveryKind.targetKind), {targetKey: jobName, direction: ""})?.retryMode
        ) !== "native"
    )
        return {taken: false}
    const lockRow = find(LOCK, {jobName}).forUpdate(true).one()
    const nowMs = now()
    const target = targetOf(map(jobRow), lockRow ? map(lockRow) : null, nowMs)
    if (target.revision !== text(p.expectedRevision) || target.state === "running") return {taken: false}
    if (name === "check") return {taken: true}
    if (!lockRow || !["scheduled", "failing"].includes(target.state)) return {taken: false}
    const last = lastSchedule(target.cron, nowMs)
    if (last == null) return {taken: false}
    lockRow.set("lastRunTime", last - 1)
    lockRow.update()
    return {taken: true}
}

// ===== runs =====
const runStateList = [
    {state: "all", text: "All"},
    {state: "failed", text: "Failed"},
    {state: "running", text: "Running or not ended"},
    {state: "ended", text: "Ended without an error"}
]
// The numbers and yes-or-noes of a run's results, by name; and the names of what else it answered.
function resultCounts(run) {
    let results = null
    try {
        results = run.results ? JSON.parse(String(run.results)) : null
    } catch (error) {
        results = null
    }
    const counts = []
    const others = []
    if (results && typeof results === "object" && !Array.isArray(results))
        for (const name of Object.keys(results).slice(0, 60)) {
            const value = results[name]
            // a result named like a secret is never a count, whatever it holds
            if (secretName.test(name)) {
                if (value != null) others.push(safeText(name).slice(0, 60))
            } else if (typeof value === "number" || typeof value === "boolean")
                counts.push({name: safeText(name).slice(0, 60), value})
            else if (typeof value === "string" && shortNumber.test(value))
                counts.push({name: safeText(name).slice(0, 60), value: Number(value)})
            else if (Array.isArray(value)) counts.push({name: safeText(name).slice(0, 60) + " (count)", value: value.length})
            else if (value != null) others.push(safeText(name).slice(0, 60))
        }
    return {counts, others}
}
// How many items a run's own results say failed: the numbers named for a failure, an error, a rejection or a skip.
const failedByResults = counts =>
    counts
        .filter(count => typeof count.value === "number" && /fail|error|reject|skip|unsent|invalid/i.test(count.name))
        .reduce((sum, count) => sum + Math.max(count.value, 0), 0)
// A run's state: failed (its service ended with an error), running (the run its job's lock holds), stopped (never ended and
// not what the lock holds), itemsFailed (ended without an error, with failed items by its results or its owner), succeeded.
function runStateOf(run, lock, failedItems) {
    if (run.hasError === "Y") return "failed"
    if (run.endTime == null)
        return text(lock?.jobRunId) === run.jobRunId ? "running" : run.startTime == null ? "waiting" : "stopped"
    return failedItems > 0 ? "itemsFailed" : "succeeded"
}
const runStateText = (state, failedItems) =>
    ({
        failed: "Failed",
        running: "Running",
        waiting: "Waiting for a worker",
        stopped: "Not ended: its server stopped, or it was started by hand and never finished",
        itemsFailed: "Ended with " + failedItems + " failed " + (failedItems === 1 ? "item" : "items"),
        succeeded: "Succeeded"
    })[state] || state
function describedRun(run, lock, registration, failedItems, counts) {
    const state = runStateOf(run, lock, failedItems)
    const started = ms(run.startTime)
    const ended = ms(run.endTime)
    return {
        jobRunId: run.jobRunId,
        jobName: run.jobName,
        title: text(registration?.title) || run.jobName,
        ownerApp: text(registration?.ownerApp) || null,
        state,
        stateText: runStateText(state, failedItems),
        startTime: iso(run.startTime),
        endTime: iso(run.endTime),
        startText: zoned(started),
        durationMs: started != null && ended != null ? ended - started : null,
        errorClass: state === "failed" ? errorClassOf(run.errors) : "",
        errorText: state === "failed" ? safeText(run.errors) : "",
        failedItemCount: failedItems,
        countList: counts,
        traceId: /^[0-9a-f]{16,64}$/i.test(text(run.traceId)) ? text(run.traceId) : null,
        startedBy: run.userId ? "A person" : "The scheduler",
        timeZone: zone()
    }
}

// find#JobRuns: a page of runs, newest first, within a bounded range.
function findJobRuns(p) {
    lib.requireView()
    const {pageIndex, pageSize} = lib.pageOf(p)
    const state = text(p.state) || "all"
    if (!runStateList.some(option => option.state === state)) throw new Error("Unknown run state " + state)
    const nowMs = now()
    const from = ms(p.fromDate)
    const thru = ms(p.thruDate)
    if ((p.fromDate && (from == null || Number.isNaN(from))) || (p.thruDate && (thru == null || Number.isNaN(thru))))
        throw new Error("A date is not a date")
    const until = thru ?? nowMs + 60000
    const since = Math.max(from ?? until - defaultDays * 86400000, until - rangeDays * 86400000)
    const factory = ec.entity.conditionFactory
    const runFind = () => {
        let found = find(RUN).condition("jobName", "not-like", QUEUED + "%")
        if (text(p.jobName)) found = found.condition("jobName", text(p.jobName))
        // a run not yet started has no start time: it is listed with the running ones
        const inRange = factory.makeCondition(
            [
                factory.makeCondition("startTime", "greater-equals", iso(since)),
                factory.makeCondition("startTime", "less", iso(until))
            ],
            "and"
        )
        if (state === "running") found = found.condition("endTime", "equals", null)
        else found = found.condition(inRange)
        if (state === "failed") found = found.condition("hasError", "Y")
        else if (state === "ended")
            found = found
                .condition("endTime", "not-equals", null)
                .condition(
                    factory.makeCondition(
                        [factory.makeCondition("hasError", "equals", null), factory.makeCondition("hasError", "equals", "N")],
                        "or"
                    )
                )
        return found
    }
    const runCount = Number(runFind().count())
    const found = Array.from(
        runFind()
            .orderBy("-startTime,-jobRunId")
            .offset(pageIndex * pageSize)
            .limit(pageSize)
            .list()
    ).map(map)
    const locks = lockMap()
    const registrations = new Map(rows(REGISTRATION).map(row => [row.jobName, map(row)]))
    return {
        runList: found.map(run => {
            const {counts} = resultCounts(run)
            return describedRun(
                run,
                locks.get(run.jobName),
                registrations.get(run.jobName),
                failedByResults(counts),
                counts.slice(0, 6)
            )
        }),
        runCount,
        pageIndex,
        pageSize,
        stateList: runStateList,
        fromDate: iso(since),
        thruDate: iso(until),
        defaultDays,
        rangeDays,
        timeZone: zone()
    }
}

// A run's stored parameters as shown: every name, and a value only of a parameter its owner registered as safe, or a
// number or a yes or no.
function shownParameters(run, definitions) {
    let stored = null
    try {
        stored = run.parameters ? JSON.parse(String(run.parameters)) : null
    } catch (error) {
        stored = null
    }
    if (!stored || typeof stored !== "object" || Array.isArray(stored)) return []
    const registered = new Set(definitions.map(definition => definition.parameterName))
    return Object.keys(stored)
        .slice(0, 60)
        .map(name => {
            const value = stored[name]
            // a job's stored parameters are text: one that reads as a number or a yes or no is shown as one
            const plain =
                typeof value === "number" ||
                typeof value === "boolean" ||
                (typeof value === "string" && (shortNumber.test(value) || /^(true|false|Y|N)$/.test(value)))
            const shown = !secretName.test(name) && (plain || (registered.has(name) && typeof value === "string"))
            return {
                name: safeText(name).slice(0, 60),
                valueText: value == null || value === "" ? "" : shown ? String(value).slice(0, 255) : "(not shown)",
                registered: registered.has(name)
            }
        })
}
function loadRun(jobRunId) {
    const row = text(jobRunId) ? one(RUN, {jobRunId: text(jobRunId)}) : null
    if (!row || isQueued(row.jobName)) return null
    return map(row)
}
// The independent items of a run, by what its job's owner registered (RecoveryDescriptor itemsService): each answer read as
// data, its texts cut and masked, its states of a fixed set.
const itemStates = {done: "Done", failed: "Failed", skipped: "Skipped", uncertain: "Outcome unknown", waiting: "Waiting"}
function runItems(run) {
    const descriptor = engine.descriptorFor(engine.descriptorsOf(recoveryKind.targetKind), {targetKey: run.jobName})
    const service = text(descriptor?.itemsService)
    const none = (available, reason) => ({
        itemsAvailable: available,
        itemsReason: reason,
        itemList: [],
        failedItemCount: 0,
        itemCount: 0
    })
    if (!descriptor || !service) return none(false, "Its owner registered no detail of what each run did")
    if (!ec.service.getServiceDefinition(service)) {
        ec.logger.error("Recovery descriptor " + descriptor.descriptorKey + " names an items service that is not defined")
        return none(false, "Its owner's detail of the run is not available")
    }
    const asked = lib.callRegistered(service, {
        jobName: run.jobName,
        jobRunId: run.jobRunId,
        startTime: ms(run.startTime),
        endTime: ms(run.endTime)
    })
    if (asked.failure) {
        ec.logger.error(
            "Recovery descriptor " + descriptor.descriptorKey + ": its owner did not answer a run's items: " + asked.failure
        )
        return none(false, engine.ownerName(descriptor) + " could not be asked what this run did")
    }
    const itemList = Array.from(asked.out.itemList || [])
        .slice(0, maxItems)
        .map(map)
        .map(item => {
            const state = Object.hasOwn(itemStates, text(item.state)) ? text(item.state) : "uncertain"
            return {
                itemKey: safeText(item.itemKey).slice(0, 80),
                title: safeText(item.title).slice(0, 120) || safeText(item.itemKey).slice(0, 80),
                state,
                stateText: itemStates[state],
                reason: safeText(item.reason).slice(0, 200) || null,
                ownerRef: text(item.ownerRef).slice(0, 80) || null,
                // where the item is itself something recovered here, by its kind and id (never a URL)
                recoveryKind: ["SystemMessage", "ServiceJob"].includes(text(item.recoveryKind)) ? text(item.recoveryKind) : null,
                recoveryTargetId: text(item.recoveryTargetId).slice(0, 80) || null,
                nextStep: safeText(item.nextStep).slice(0, 200) || null
            }
        })
    const counted = Number(asked.out.failedItemCount)
    const failedItemCount =
        Number.isFinite(counted) && counted >= 0 ? Math.floor(counted) : itemList.filter(item => item.state === "failed").length
    const total = Number(asked.out.itemCount)
    return {
        itemsAvailable: true,
        itemsReason: null,
        itemList,
        failedItemCount,
        itemCount: Number.isFinite(total) && total >= itemList.length ? Math.floor(total) : itemList.length,
        ownerApp: engine.ownerName(descriptor)
    }
}

// get#JobRunItems: what each independent item of a run came to, by the job's owner.
function getJobRunItems(p) {
    lib.requireView()
    const run = loadRun(p.jobRunId)
    if (!run) throw new Error("No job run " + text(p.jobRunId))
    return Object.assign({jobRunId: run.jobRunId, jobName: run.jobName}, runItems(run))
}

// get#JobRun: one run's evidence.
function getJobRun(p) {
    lib.requireView()
    const run = loadRun(p.jobRunId)
    if (!run) throw new Error("No job run " + text(p.jobRunId))
    const jobRow = one(JOB, {jobName: run.jobName})
    const job = jobRow ? map(jobRow) : null
    const registration = registrationOf(run.jobName)
    const definitions = job && registration ? definitionsOf(job) : []
    const lockRow = one(LOCK, {jobName: run.jobName})
    const lock = lockRow ? map(lockRow) : null
    const {counts, others} = resultCounts(run)
    // its owner's items decide with its own results whether a run without an error is a success
    let items
    try {
        items = runItems(run)
    } catch (error) {
        ec.logger.error("The items of job run " + run.jobRunId + " could not be read: " + String(error?.message ?? error))
        items = {
            itemsAvailable: false,
            itemsReason: "The run's items could not be read",
            itemList: [],
            failedItemCount: 0,
            itemCount: 0
        }
    }
    const failedItems = Math.max(failedByResults(counts), Number(items.failedItemCount) || 0)
    const lines = value => text(value).split(/\r?\n/).map(safeText).filter(Boolean)
    const errorLines = lines(run.errors)
    return {
        run: Object.assign(describedRun(run, lock, registration, failedItems, counts), {
            jobFound: !!job,
            jobDescription: text(job?.description) || null,
            errorLineList: errorLines.slice(0, 5),
            errorLineCount: errorLines.length,
            messageLineCount: lines(run.messages).length,
            otherResultList: others,
            hostName: safeText(run.hostName).slice(0, 80) || null,
            instanceId: safeText(run.instanceId).slice(0, 60) || null,
            runThread: safeText(run.runThread).slice(0, 60) || null,
            isLockHolder: text(lock?.jobRunId) === run.jobRunId
        }),
        parameterList: shownParameters(run, definitions),
        itemsAvailable: items.itemsAvailable === true,
        itemsReason: items.itemsReason || null,
        itemList: Array.from(items.itemList || []),
        itemCount: Number(items.itemCount) || 0,
        itemsOwnerApp: items.ownerApp || null,
        canManage: ec.user.hasPermission(lib.MANAGE)
    }
}

// find#MessageJobRunItems: the framework's own answer for its two system message jobs (the registered itemsService of
// send_AllProducedSystemMessages_frequent and consume_AllReceivedSystemMessages_frequent): the messages the run tried, by
// the time of their last attempt (a send is queued by the run and may end just after it), each with its state now. A row
// names a message's id, type and state only; its evidence is SystemMessageDetail's.
function findMessageJobRunItems(p) {
    lib.requireView()
    const outgoing = {send_AllProducedSystemMessages_frequent: "Y", consume_AllReceivedSystemMessages_frequent: "N"}
    const jobName = text(p.jobName)
    if (!Object.hasOwn(outgoing, jobName)) return {itemList: [], itemCount: 0, failedItemCount: 0}
    const started = ms(p.startTime)
    if (started == null) return {itemList: [], itemCount: 0, failedItemCount: 0}
    const ended = (ms(p.endTime) ?? now()) + 5 * 60000
    const tried = () =>
        find("moqui.service.message.SystemMessage")
            .condition("isOutgoing", outgoing[jobName])
            .condition("lastAttemptDate", "greater-equals", iso(started))
            .condition("lastAttemptDate", "less", iso(ended))
    const itemCount = Number(tried().count())
    const states = {
        done: "done",
        failed: "failed",
        retrying: "failed",
        uncertain: "uncertain",
        settled: "uncertain",
        inProgress: "waiting",
        waiting: "waiting"
    }
    const all = Array.from(tried().orderBy("-lastAttemptDate").limit(1000).list()).map(map)
    const itemList = all
        .map(message => ({message, state: states[messages.stateOf(message)] || "skipped"}))
        // the failed and unknown first: a page of them, not of what went through
        .sort((a, b) => (a.state === "done" ? 1 : 0) - (b.state === "done" ? 1 : 0))
        .slice(0, maxItems)
        .map(({message, state}) => ({
            itemKey: message.systemMessageId,
            title:
                "Message " +
                message.systemMessageId +
                (message.systemMessageTypeId ? " (" + message.systemMessageTypeId + ")" : ""),
            state,
            reason: state === "failed" ? "Failed " + Number(message.failCount || 0) + " times" : null,
            recoveryKind: "SystemMessage",
            recoveryTargetId: message.systemMessageId
        }))
    return {
        itemList,
        itemCount,
        failedItemCount: all.filter(message => ["failed", "retrying"].includes(messages.stateOf(message))).length
    }
}

const operations = {findJobs, getJob, storeJob, findJobRuns, getJobRun, getJobRunItems, applyJobRecovery, findMessageJobRunItems}
;({
    recoveryKind,
    userHolds,
    accountOpen,
    accountName,
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown job operation " + op)
        return operations[op](parameters)
    }
})
