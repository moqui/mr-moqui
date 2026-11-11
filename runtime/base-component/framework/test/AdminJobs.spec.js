// Jobs and notification topics, org.moqui.admin.OperationServices (mr-boss plan/impl-admin.md P14; design-admin §10 Jobs,
// JobDetail, JobRuns, JobRunDetail, Topics and TopicDetail, §14 "Operational recovery"): a job is changed only where its owner
// registered it, and only in what was registered: never its service or the identity it runs as, a parameter only of its
// registered type and bounds (no expression, path, URL or address), a schedule the scheduler reads and no more often than
// allowed, a recipient only of the registered audience; a run outside the schedule is asked of the scheduler (the job is
// marked due under its lock row), never started beside a run under way, and a lock is never cleared or deleted to make a
// job look idle; a run that ended without an error while its results or its owner count failed items is not a success, and
// a run's errors and parameters are masked; a topic's delivery defaults are changed only where registered, nobody is
// subscribed outside a topic's registered audience (a private topic with none takes nobody), and receiving every message
// of a topic is never set here.
// Run: ./moq test framework --spec AdminJobs
const O = "org.moqui.admin.OperationServices."
const JOB = "moqui.service.job.ServiceJob"
const LOCK = "moqui.service.job.ServiceJobRunLock"
const RUN = "moqui.service.job.ServiceJobRun"
const TOPIC = "moqui.security.user.NotificationTopic"
const TOPIC_USER = "moqui.security.user.NotificationTopicUser"
const MESSAGE = "moqui.service.message.SystemMessage"
const BACKUP = "org.moqui.impl.ServerServices.backup#EmbeddedDatabase"
const NOOP = "org.moqui.impl.BasicServices.noop"
const SEND_JOB = "send_AllProducedSystemMessages_frequent"
const users = {manage: "SPEC_AJ_MANAGE", view: "SPEC_AJ_VIEW", settings: "SPEC_AJ_SETTINGS", none: "SPEC_AJ_NONE"}
const groups = {manage: "SPEC_AJ_MANAGERS", view: "SPEC_AJ_VIEWERS", settings: "SPEC_AJ_CONFIGURERS"}
const jobs = {
    config: "SPEC_AJ_CONFIG",
    plain: "SPEC_AJ_PLAIN",
    run: "SPEC_AJ_RUN",
    busy: "SPEC_AJ_BUSY",
    paused: "SPEC_AJ_PAUSED",
    twice: "SPEC_AJ_TWICE"
}
const runs = {busy: "SPEC_AJ_R_BUSY", items: "SPEC_AJ_R_ITEMS", failed: "SPEC_AJ_R_FAILED", send: "SPEC_AJ_R_SEND"}
const topics = {open: "SPEC_AJ_TOPIC", sealed: "SPEC_AJ_PRIVATE", approval: "SPEC_AJ_APPROVAL"}
const messageId = "SPEC_AJ_M_FAILED"
const hour = 3600000

const username = userId => userId.toLowerCase().replace(/_/g, ".")
const find = (entityName, conditions = {}) => ec.entity.find(entityName).condition(conditions).disableAuthz()
const one = (entityName, conditions) => find(entityName, conditions).one()
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
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
const has = (value, pattern) => pattern.test(String(value ?? ""))
const job = jobName => one(JOB, {jobName}).getMap()
const lock = jobName => one(LOCK, {jobName})?.getMap() || null
const detail = jobName => invoke(O + "get#Job", {jobName})
const store = (jobName, values) =>
    invoke(O + "store#Job", Object.assign({jobName, expectedRevision: detail(jobName).job?.revision}, values))
const runNow = (jobName, extra = {}) =>
    invoke(
        O + "execute#Recovery",
        Object.assign(
            {
                targetKind: "ServiceJob",
                action: "retry",
                targetId: jobName,
                expectedRevision: detail(jobName).job?.runRevision,
                reason: "Spec: run it now"
            },
            extra
        )
    )
const first = out => Array.from(out.resultList || [])[0] || {}
const audits = (entityName, key) =>
    Array.from(find("moqui.entity.EntityAuditLog", {changedEntityName: entityName, pkPrimaryValue: key}).list())
const topicRevision = topic => invoke(O + "get#Topic", {topic}).topic?.revision
const defaults = (topic, values) =>
    invoke(O + "store#TopicDefaults", Object.assign({topic, expectedRevision: topicRevision(topic)}, values))
const subscription = (topic, userId) => one(TOPIC_USER, {topic, userId})?.getMap() || null
const subscribe = (topic, name, values = {}) => {
    const userId = one("moqui.security.UserAccount", {username: name})?.userId
    const current = userId ? subscription(topic, userId) : null
    return invoke(
        O + "store#TopicSubscription",
        Object.assign({topic, username: name, expectedRevision: current ? String(Number(current.lastUpdatedStamp)) : ""}, values)
    )
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const [key, userId] of Object.entries(users))
            make("moqui.security.UserAccount", {userId, username: username(userId), userFullName: "Spec jobs " + key})
        for (const [key, userGroupId] of Object.entries(groups))
            make("moqui.security.UserGroup", {userGroupId, description: "Spec jobs " + key})
        make("moqui.security.UserPermission", {userPermissionId: "SPEC_AJ_AUDIENCE", description: "Spec jobs audience"})
        const grant = (userGroupId, permissions) =>
            permissions.forEach(userPermissionId =>
                make("moqui.security.UserGroupPermission", {userGroupId, userPermissionId, fromDate: 0})
            )
        grant(groups.manage, ["ADMIN_OPERATION_VIEW", "ADMIN_OPERATION_MANAGE", "SPEC_AJ_AUDIENCE"])
        grant(groups.view, ["ADMIN_OPERATION_VIEW"])
        grant(groups.settings, ["ADMIN_SETTINGS_VIEW", "ADMIN_SETTINGS_MANAGE"])
        for (const key of Object.keys(groups))
            make("moqui.security.UserGroupMember", {userGroupId: groups[key], userId: users[key], fromDate: 0})

        // a job whose owner registered its schedule, pause and two parameters; and one nobody registered
        make(JOB, {
            jobName: jobs.config,
            description: "Spec jobs configured",
            serviceName: BACKUP,
            cronExpression: "0 0 3 * * ?",
            paused: "Y"
        })
        make("moqui.service.job.ServiceJobParameter", {jobName: jobs.config, parameterName: "dailyCount", parameterValue: "7"})
        make("moqui.service.job.ServiceJobParameter", {
            jobName: jobs.config,
            parameterName: "fullEvery",
            parameterValue: "spec private deployment value"
        })
        make("moqui.server.JobRegistration", {
            jobName: jobs.config,
            ownerApp: "Spec owner",
            title: "Spec configured job",
            scheduleEditable: "Y",
            minIntervalMinutes: 15,
            pauseEditable: "Y",
            recipientPermission: "SPEC_AJ_AUDIENCE"
        })
        make("moqui.server.JobParameterDefinition", {
            jobName: jobs.config,
            parameterName: "dailyCount",
            title: "Daily archives",
            valueType: "integer",
            minValue: 1,
            maxValue: 30
        })
        make("moqui.server.JobParameterDefinition", {
            jobName: jobs.config,
            parameterName: "backupMode",
            title: "Mode",
            valueType: "text",
            maxLength: 20
        })
        // one whose owner registered a parameter its service does not have: left out
        make("moqui.server.JobParameterDefinition", {jobName: jobs.config, parameterName: "serviceName", valueType: "text"})
        make(JOB, {
            jobName: jobs.plain,
            description: "Spec jobs unregistered",
            serviceName: NOOP,
            cronExpression: "0 0 3 * * ?",
            paused: "N"
        })
        // each has run since its schedule last came due, so neither is due while the spec runs
        make(LOCK, {jobName: jobs.config, lastRunTime: Date.now()})
        make(LOCK, {jobName: jobs.plain, lastRunTime: Date.now()})

        // jobs for a run outside the schedule: free, held by a run, paused, and one for two requests at one revision
        const scheduled = (jobName, fields = {}, heldBy = null) => {
            make(
                JOB,
                Object.assign(
                    {jobName, description: "Spec jobs run", serviceName: NOOP, cronExpression: "0 0 3 1 1 ?", paused: "N"},
                    fields
                )
            )
            if (heldBy) make(RUN, {jobRunId: heldBy, jobName, startTime: Date.now() - 600000})
            make(LOCK, {jobName, jobRunId: heldBy, lastRunTime: Date.now() - (heldBy ? 600000 : hour)})
            make("moqui.server.RecoveryDescriptor", {
                descriptorKey: "specAj" + jobName,
                targetKind: "ServiceJob",
                targetKey: jobName,
                ownerApp: "Spec owner",
                title: "Spec job run",
                retryMode: "native",
                idempotencyNote: "A second run does nothing new."
            })
        }
        scheduled(jobs.run)
        scheduled(jobs.twice)
        scheduled(jobs.paused, {paused: "Y"})
        scheduled(jobs.busy, {}, runs.busy)

        // runs: one that ended without an error and counts failed items, one that failed, and one of the framework's send job
        make(RUN, {
            jobRunId: runs.items,
            jobName: jobs.plain,
            startTime: Date.now() - 3 * hour,
            endTime: Date.now() - 3 * hour + 4000,
            hasError: "N",
            parameters: JSON.stringify({daysToKeep: "30", apiKey: "spec-aj-key-1", note: "spec private parameter"}),
            results: JSON.stringify({sentCount: 3, failedCount: 2, note: "spec private result"}),
            messages: "spec private message line"
        })
        make(RUN, {
            jobRunId: runs.failed,
            jobName: jobs.plain,
            startTime: Date.now() - 2 * hour,
            endTime: Date.now() - 2 * hour + 1000,
            hasError: "Y",
            errors: "Could not connect to https://user:pw@files.example.test/private/path?token=abc for bob@example.test\nsecond line"
        })
        make(RUN, {
            jobRunId: runs.send,
            jobName: SEND_JOB,
            startTime: Date.now() - 5 * hour,
            endTime: Date.now() - 5 * hour + 60000,
            hasError: "N"
        })
        make(MESSAGE, {
            systemMessageId: messageId,
            statusId: "SmsgError",
            isOutgoing: "Y",
            initDate: Date.now() - 6 * hour,
            lastAttemptDate: Date.now() - 5 * hour + 30000,
            failCount: 3,
            messageText: "spec private payload"
        })
        // the send job's items are its owner's registration (the framework's seed row); a database loaded before it has none
        if (!find("moqui.server.RecoveryDescriptor", {targetKind: "ServiceJob", targetKey: SEND_JOB}).count())
            make("moqui.server.RecoveryDescriptor", {
                descriptorKey: "specAjSendItems",
                targetKind: "ServiceJob",
                targetKey: SEND_JOB,
                itemsService: O + "find#MessageJobRunItems"
            })

        make(TOPIC, {
            topic: topics.open,
            description: "Spec jobs topic",
            titleTemplate: "spec private title ${x}",
            showAlert: "N",
            isPrivate: "N"
        })
        make(TOPIC, {topic: topics.sealed, description: "Spec jobs private topic", isPrivate: "Y"})
        make(TOPIC, {topic: topics.approval, description: "Spec jobs approval", approveServiceName: NOOP, persistOnSend: "Y"})
        make("moqui.server.TopicRegistration", {
            topic: topics.open,
            ownerApp: "Spec owner",
            editableFields: "showAlert,emailNotifications",
            subscriptionPermission: "SPEC_AJ_AUDIENCE"
        })
        make("moqui.server.TopicRegistration", {topic: topics.approval, ownerApp: "Spec owner", editableFields: "persistOnSend"})
        // a user's own preference, of a user outside the audience
        make(TOPIC_USER, {topic: topics.open, userId: users.none, receiveNotifications: "N"})
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const jobNames = Object.values(jobs)
        const all = (entityName, field, values) => ec.entity.find(entityName).condition(field, "in", values).deleteAll()
        ec.entity.find(MESSAGE).condition("systemMessageId", messageId).deleteAll()
        all(LOCK, "jobName", jobNames)
        all(RUN, "jobRunId", Object.values(runs))
        all(RUN, "jobName", jobNames)
        all("moqui.service.job.ServiceJobUser", "jobName", jobNames)
        all("moqui.service.job.ServiceJobParameter", "jobName", jobNames)
        all("moqui.server.JobParameterDefinition", "jobName", jobNames)
        all("moqui.server.JobRegistration", "jobName", jobNames)
        ec.entity.find("moqui.server.RecoveryDescriptor").condition("descriptorKey", "like", "specAj%").deleteAll()
        ec.entity.find("moqui.server.OperationResult").condition("operationKey", "like", "recovery:spec-aj%").deleteAll()
        all(JOB, "jobName", jobNames)
        all(TOPIC_USER, "topic", Object.values(topics))
        all("moqui.server.TopicRegistration", "topic", Object.values(topics))
        all(TOPIC, "topic", Object.values(topics))
        all("moqui.security.UserGroupMember", "userGroupId", Object.values(groups))
        all("moqui.security.UserGroupPermission", "userGroupId", Object.values(groups))
        all("moqui.security.UserGroup", "userGroupId", Object.values(groups))
        ec.entity.find("moqui.security.UserPermission").condition("userPermissionId", "SPEC_AJ_AUDIENCE").deleteAll()
        all("moqui.security.UserAccount", "userId", Object.values(users))
    }
})

feature("a job is changed only where its owner registered it: never its service, a parameter only within its type and bounds", {
    when() {
        const out = {}
        out.viewer = as(users.view, () => store(jobs.config, {paused: "N"}))
        as(users.manage, () => {
            out.unregistered = store(jobs.plain, {paused: "Y"})
            // no parameter of the service names the job's service, its user, its topic or its time limit
            out.service = store(jobs.config, {serviceName: NOOP, userId: users.manage, topic: "SpecTopic", transactionTimeout: 5})
            out.unknownParameter = store(jobs.config, {parameterName: "fullEvery", parameterValue: "3"})
            out.undeclared = store(jobs.config, {parameterName: "serviceName", parameterValue: "x"})
            out.expression = store(jobs.config, {parameterName: "backupMode", parameterValue: "${ec.user.userId}"})
            out.path = store(jobs.config, {parameterName: "backupMode", parameterValue: "../etc/passwd"})
            out.url = store(jobs.config, {parameterName: "backupMode", parameterValue: "https://files.example.test/x"})
            out.address = store(jobs.config, {parameterName: "backupMode", parameterValue: "bob@example.test"})
            out.low = store(jobs.config, {parameterName: "dailyCount", parameterValue: "0"})
            out.fraction = store(jobs.config, {parameterName: "dailyCount", parameterValue: "7.5"})
            out.everyMinute = store(jobs.config, {cronExpression: "0 * * * * ?"})
            out.nonsense = store(jobs.config, {cronExpression: "every day at noon"})
            out.cleared = store(jobs.config, {cronExpression: ""})
            out.stale = invoke(O + "store#Job", {jobName: jobs.config, expectedRevision: "1", paused: "N"})
            out.untouched = JSON.stringify(job(jobs.config))
            // what was registered is taken, with the reason
            out.saved = store(jobs.config, {
                parameterMap: {dailyCount: "9", backupMode: "dump"},
                cronExpression: "0 0 4 * * ?",
                reason: "Spec: keep more"
            })
            out.resumed = store(jobs.config, {paused: "N"})
            out.pausedAgain = store(jobs.config, {paused: "Y"})
            // a recipient: only of the registered audience; an unknown name and an outsider get one answer
            out.outsider = store(jobs.config, {recipientUsername: username(users.none)})
            out.nobody = store(jobs.config, {recipientUsername: "spec.aj.nobody"})
            out.recipient = store(jobs.config, {recipientUsername: username(users.manage)})
            out.detail = detail(jobs.config)
            out.listed = invoke(O + "find#Jobs", {term: "Spec jobs"})
        })
        out.none = as(users.none, () => invoke(O + "find#Jobs", {}))
        return out
    },
    then(out) {
        has(out.viewer.error, /ADMIN_OPERATION_MANAGE/)
        out.unregistered.applied === false && out.unregistered.reasonCode == "NOT_REGISTERED"
        job(jobs.plain).paused == "N"
        job(jobs.config).serviceName == BACKUP && job(jobs.config).topic == null && job(jobs.config).transactionTimeout == null
        out.unknownParameter.reasonCode == "NOT_REGISTERED_PARAMETER" && out.undeclared.reasonCode == "NOT_REGISTERED_PARAMETER"
        ;[out.expression, out.path, out.url, out.address, out.low, out.fraction].every(
            found => found.reasonCode == "INVALID_VALUE"
        )
        ;[out.everyMinute, out.nonsense, out.cleared].every(found => found.reasonCode == "INVALID_SCHEDULE")
        has(out.everyMinute.reason, /15 minutes/)
        out.stale.reasonCode == "STALE_REVISION"
        // nothing of all that was written
        JSON.parse(out.untouched).cronExpression == "0 0 3 * * ?" && JSON.parse(out.untouched).paused == "Y"
        out.saved.applied === true && out.saved.changedList.length == 3
        job(jobs.config).cronExpression == "0 0 4 * * ?"
        one("moqui.service.job.ServiceJobParameter", {jobName: jobs.config, parameterName: "dailyCount"}).parameterValue == "9"
        one("moqui.service.job.ServiceJobParameter", {jobName: jobs.config, parameterName: "backupMode"}).parameterValue == "dump"
        audits(JOB, jobs.config).some(
            row => row.changedFieldName == "parameter.dailyCount" && row.changeReason == "Spec: keep more"
        )
        // resumed: its next run, in the machine's time zone
        out.resumed.applied === true && out.resumed.state == "scheduled"
        has(out.resumed.nextRunText, /^\d{4}-\d\d-\d\d 04:00 \(.+\)$/)
        out.pausedAgain.applied === true && out.pausedAgain.state == "paused" && !out.pausedAgain.nextRunText
        out.outsider.reasonCode == "NOT_ELIGIBLE" && out.nobody.reasonCode == "NOT_ELIGIBLE"
        out.outsider.reason == out.nobody.reason
        out.recipient.applied === true
        one("moqui.service.job.ServiceJobUser", {jobName: jobs.config, userId: users.manage}).receiveNotifications == "Y"
        // the detail: the registered parameters with their values; another's value only where it is a number or a yes or no
        out.detail.job.registered === true &&
            out.detail.job.scheduleEditable === true &&
            out.detail.job.priorityEditable === false
        out.detail.parameterList.find(row => row.parameterName == "dailyCount").value == "9"
        out.detail.parameterList.find(row => row.parameterName == "fullEvery").editable === false
        !JSON.stringify(out.detail).includes("spec private")
        !out.detail.parameterList.some(row => row.parameterName == "serviceName")
        out.detail.recipientList.length == 1 && out.detail.recipientList[0].eligible === true
        out.listed.jobList.some(row => row.jobName == jobs.config && row.state == "paused" && row.ownerApp == "Spec owner")
        out.listed.jobList.some(row => row.jobName == jobs.plain && row.registered === false)
        has(out.none.error, /ADMIN_OPERATION_VIEW/)
    }
})

feature("a run outside the schedule is asked of the scheduler under the job's lock; a lock is never cleared to idle a job", {
    when() {
        const out = {}
        out.viewer = as(users.view, () => runNow(jobs.run))
        out.before = lock(jobs.run)
        as(users.manage, () => {
            out.busyPreview = invoke(O + "preview#Recovery", {targetKind: "ServiceJob", targetId: jobs.busy}).previewList?.[0]
            out.busy = runNow(jobs.busy)
            out.plain = runNow(jobs.plain)
            out.paused = runNow(jobs.paused)
            out.staleRevision = runNow(jobs.run, {expectedRevision: "0.-.0"})
            out.asked = runNow(jobs.run, {operationKey: "spec-aj-run"})
            out.listed = invoke(O + "find#Jobs", {term: jobs.run}).jobList?.[0]
            // asked again: by its key the first answer, and without one it is due already
            out.repeated = runNow(jobs.run, {operationKey: "spec-aj-run"})
            out.again = runNow(jobs.run)
        })
        // two requests at one revision: the lock row is locked for each, and only the first finds the job as it was read
        const revision = as(users.manage, () => detail(jobs.twice).job.runRevision)
        const apply = () => invoke(O + "apply#JobRecovery", {jobName: jobs.twice, expectedRevision: revision, step: "request"})
        out.first = apply()
        out.second = apply()
        return out
    },
    then(out) {
        has(out.viewer.error, /ADMIN_OPERATION_MANAGE/)
        // a job a run holds: nothing is offered, and its lock is as it was
        out.busyPreview.state == "running" && out.busyPreview.actionList[0].eligible === false
        out.busyPreview.actionList[0].reasonCode == "IN_PROGRESS"
        first(out.busy).applied === false && first(out.busy).reasonCode == "IN_PROGRESS"
        has(first(out.busy).reason, /lock is not cleared here/)
        lock(jobs.busy).jobRunId == runs.busy
        one(RUN, {jobRunId: runs.busy}).endTime == null
        first(out.plain).reasonCode == "NOT_REGISTERED" && first(out.paused).reasonCode == "PAUSED"
        first(out.staleRevision).reasonCode == "STALE_REVISION"
        // asked for: the job is due for the scheduler, which claims the lock itself; no run row is made here
        out.asked.applied === true && first(out.asked).state == "requested"
        lock(jobs.run).jobRunId == null && Number(lock(jobs.run).lastRunTime) < Number(out.before.lastRunTime)
        ec.service.isLastRunBeforeLastSchedule("0 0 3 1 1 ?", Number(lock(jobs.run).lastRunTime), null, Date.now()) === true
        find(RUN, {jobName: jobs.run}).count() == 0
        out.listed.state == "requested" && has(out.listed.stateText, /scheduler's next pass/)
        audits(JOB, jobs.run).some(
            row => row.changedFieldName == "runRequest" && has(row.changeReason, /Run now: Spec: run it now/)
        )
        out.repeated.repeated === true && out.repeated.applied === true
        first(out.again).reasonCode == "ALREADY_DUE"
        out.first.taken === true && out.second.taken === false
    }
})

feature("a run that ended without an error but with failed items is not a success; a run's errors and parameters are masked", {
    when() {
        const out = {}
        as(users.view, () => {
            out.list = invoke(O + "find#JobRuns", {jobName: jobs.plain})
            out.items = invoke(O + "get#JobRun", {jobRunId: runs.items})
            out.failed = invoke(O + "get#JobRun", {jobRunId: runs.failed})
            out.send = invoke(O + "get#JobRun", {jobRunId: runs.send})
            out.running = invoke(O + "find#JobRuns", {jobName: jobs.busy, state: "running"})
        })
        out.none = as(users.none, () => invoke(O + "get#JobRun", {jobRunId: runs.failed}))
        return out
    },
    then(out) {
        out.list.runCount == 2
        out.list.runList.find(row => row.jobRunId == runs.items).state == "itemsFailed"
        has(out.list.runList.find(row => row.jobRunId == runs.items).stateText, /2 failed items/)
        out.list.runList.find(row => row.jobRunId == runs.failed).errorClass == "Connection"
        !has(JSON.stringify(out.list), /spec private|bob@|token=abc|user:pw|private\/path/)
        // one run: its numbers, the names of what else it answered, and a parameter's value only where it is a number
        out.items.run.state == "itemsFailed" && out.items.run.countList.some(row => row.name == "failedCount" && row.value == 2)
        out.items.run.otherResultList.includes("note") && out.items.run.messageLineCount == 1
        out.items.parameterList.find(row => row.name == "daysToKeep").valueText == "30"
        out.items.parameterList.find(row => row.name == "apiKey").valueText == "(not shown)"
        out.items.parameterList.find(row => row.name == "note").valueText == "(not shown)"
        out.items.itemsAvailable === false
        !has(JSON.stringify(out.items), /spec private|spec-aj-key/)
        out.failed.run.state == "failed" && out.failed.run.errorLineCount == 2
        has(out.failed.run.errorLineList[0], /^Could not connect to https:\/\/files\.example\.test for \(address\)$/)
        // the framework's send job: the run has no error, and its owner's items say a message failed
        out.send.run.state == "itemsFailed" && out.send.itemsAvailable === true
        out.send.itemList.some(row => row.itemKey == messageId && row.state == "failed" && row.recoveryKind == "SystemMessage")
        !JSON.stringify(out.send).includes("spec private payload")
        out.running.runList.some(row => row.jobRunId == runs.busy && row.state == "running")
        has(out.none.error, /ADMIN_OPERATION_VIEW/)
    }
})

feature("a topic's defaults are changed only where registered; nobody is subscribed outside its audience or to every message", {
    when() {
        const out = {}
        as(users.settings, () => {
            out.shown = defaults(topics.open, {
                showAlert: "Y",
                isPrivate: "Y",
                titleTemplate: "${ec.user.userId}",
                reason: "Spec: alert"
            })
            out.notEditable = defaults(topics.open, {receiveNotifications: "N"})
            out.noTemplate = defaults(topics.open, {emailNotifications: "Y"})
            out.approval = defaults(topics.approval, {persistOnSend: "N"})
            out.unregistered = defaults(topics.sealed, {showAlert: "Y"})
            out.subscribes = subscribe(topics.open, username(users.manage), {receiveNotifications: "Y"})
            out.settingsDetail = invoke(O + "get#Topic", {topic: topics.open})
        })
        as(users.manage, () => {
            out.operatorDefaults = defaults(topics.open, {showAlert: "N"})
            // a private topic nobody registered an audience for: not the administrator either
            out.sealed = subscribe(topics.sealed, username(users.manage), {receiveNotifications: "Y"})
            out.outsider = subscribe(topics.open, username(users.none), {receiveNotifications: "Y"})
            out.nobody = subscribe(topics.open, "spec.aj.nobody", {receiveNotifications: "Y"})
            // within the audience; receiving every message is not something this service sets
            out.subscribed = subscribe(topics.open, username(users.manage), {
                receiveNotifications: "Y",
                digest: "Y",
                allNotifications: "Y"
            })
            out.afterSubscribe = subscription(topics.open, users.manage)
        })
        // one who does receive every message (set in Tools): here it is only ended
        const row = one(TOPIC_USER, {topic: topics.open, userId: users.manage})
        row.set("allNotifications", "Y")
        row.update()
        as(users.manage, () => {
            out.detail = invoke(O + "get#Topic", {topic: topics.open})
            out.ended = subscribe(topics.open, username(users.manage), {endAllNotifications: true})
            out.list = invoke(O + "find#Topics", {term: "Spec jobs"})
        })
        out.none = as(users.none, () => invoke(O + "find#Topics", {}))
        return out
    },
    then(out) {
        out.shown.applied === true && out.shown.changedList.join() == "showAlert"
        one(TOPIC, {topic: topics.open}).showAlert == "Y" && one(TOPIC, {topic: topics.open}).isPrivate == "N"
        one(TOPIC, {topic: topics.open}).titleTemplate == "spec private title ${x}"
        audits(TOPIC, topics.open).some(row => row.changedFieldName == "showAlert" && row.changeReason == "Spec: alert")
        out.notEditable.reasonCode == "NOT_EDITABLE" && out.noTemplate.reasonCode == "NO_EMAIL_TEMPLATE"
        out.approval.reasonCode == "APPROVAL_KEPT" && out.unregistered.reasonCode == "NOT_REGISTERED"
        has(out.subscribes.error, /ADMIN_OPERATION_MANAGE/) && has(out.operatorDefaults.error, /ADMIN_SETTINGS_MANAGE/)
        // the configuration administrator reads the defaults and no subscriber's name
        out.settingsDetail.subscriptionList.length == 0 && out.settingsDetail.otherSubscriptionCount == 1
        out.settingsDetail.fieldList.find(row => row.field == "showAlert").editable === true
        out.settingsDetail.fieldList.find(row => row.field == "persistOnSend").editable === false
        out.sealed.reasonCode == "NOT_REGISTERED" && subscription(topics.sealed, users.manage) == null
        out.outsider.reasonCode == "NOT_ELIGIBLE" &&
            out.nobody.reasonCode == "NOT_ELIGIBLE" &&
            out.outsider.reason == out.nobody.reason
        subscription(topics.open, users.none).receiveNotifications == "N"
        out.subscribed.applied === true && out.afterSubscribe.receiveNotifications == "Y" && out.afterSubscribe.digest == "Y"
        out.afterSubscribe.allNotifications != "Y"
        // the operator reads the audience's subscriptions by name, and the other user's own preference as a count
        out.detail.subscriptionList.length == 1 && out.detail.subscriptionList[0].username == username(users.manage)
        out.detail.subscriptionList[0].allNotifications === true && out.detail.otherSubscriptionCount == 1
        out.ended.applied === true && subscription(topics.open, users.manage).allNotifications == "N"
        out.list.topicList.length == 3 && !JSON.stringify(out.list).includes("spec private title")
        out.list.topicList.find(row => row.topic == topics.sealed).isPrivate === true
        out.list.topicList.find(row => row.topic == topics.approval).actionable === true
        has(out.none.error, /ADMIN_OPERATION_VIEW/)
    }
})
