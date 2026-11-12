// org.moqui.admin.ReadinessServices (plans/archive/Admin-Screens-Down.md A6; first IronStone Admin's registries, mr-boss
// plan/design-admin.md §3, §7, §10, §14, §16): readiness, refresh, the settings index and the support summary, read from
// the owners' registrations (moqui.server ReadinessProvider, RefreshOperation and SettingsEntry, each seeded by its
// owner). Registrations are trusted component definitions: a row naming a service or a permission that does not exist is
// logged by name when read and left out, and the others still answer. Nothing here takes a service or an index name from
// a request, and nothing grants what an owner refuses. No dependency on Mantle or an app.
//
// An owner's service is called because a row names it, as the caller, in a transaction and an error frame of its own.
// The services of this file are granted to every signed-in user by an ALLOW that is not inheritable
// (data/AdminConfigurationData.xml AdminReadinessServices), so an owner's service is authorized for the caller as a
// direct call is (below an app's screen its inheritable always-allow covers it, as everything there), and the registration
// adds no right. What a check covers (an organization, a store) is the owner's to understand: a request's scope
// parameters reach an owner's service only by the names its registration lists, and the owner checks them.
//
// Readiness: each provider whose required permission the caller holds is called and answers checks. One that refuses
// the caller (a reason code or wording of refusalCodes and refusalText below, or the framework's own "not authorized")
// is left out; one that fails otherwise is named, without its diagnostics, and shown unavailable while the others
// answer. Unavailable is never shown as ready. The framework's own checks are functions here, behind check#InstanceSetup.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationLib.js")
const calls = ec.resource.script("component://framework/service/org/moqui/admin/AdminCalls.js")
const {find, one, rows, ms, text} = lib

const PROVIDER = "moqui.server.ReadinessProvider"
const REFRESH = "moqui.server.RefreshOperation"
const SETTINGS = "moqui.server.SettingsEntry"
const OVERVIEW = "ADMIN_OVERVIEW_VIEW"
const minute = 60 * 1000
const states = {failed: "Failed", unavailable: "Unavailable", warning: "Warning", ready: "Ready"}
const stateOrder = {failed: 0, unavailable: 1, warning: 2, ready: 3}
const stateColors = {failed: "negative", unavailable: "grey-8", warning: "warning", ready: "positive"}
// the refusals an owner answers for a caller, a scope or a record it may not read, as reason codes or wording (the
// last is how the org.moqui.admin services refuse a missing permission: "ADMIN_SETTINGS_VIEW is required")
const refusalCodes = ["PERMISSION_DENIED", "ORGANIZATION_DENIED", "ACCESS_DENIED", "RECORD_UNAVAILABLE", "STORE_UNAVAILABLE"]
const refusalText = /authority is required|is not available|may not read|is not authorized|\b[A-Z][A-Z0-9_]{2,} is required/

const nowMs = () => ms(ec.user.nowTimestamp)
const timeText = value => (value == null ? "" : ec.l10n.format(value, "yyyy-MM-dd HH:mm"))
const permitted = permission => !permission || ec.user.hasPermission(permission)
function denied(reason, message) {
    const error = new Error(reason + ": " + message)
    error.reason = reason
    throw error
}
function reasonCode(error) {
    if (error?.reason) return error.reason
    const match = /(?:^|\s)([A-Z][A-Z0-9_]+): /.exec(String(error?.message ?? error ?? ""))
    return match ? match[1] : null
}
const isRefusal = error => refusalCodes.includes(reasonCode(error)) || refusalText.test(String(error?.message || ""))
function requireOverview() {
    lib.signedIn()
    if (!ec.user.hasPermission(OVERVIEW)) denied("PERMISSION_DENIED", OVERVIEW + " is required")
}
// The names a registration lists (comma separated), and a request's scope as plain names and values: a name is letters
// and digits, a value a text, a number or a boolean; anything else is dropped.
const names = value =>
    text(value)
        .split(",")
        .map(name => name.trim())
        .filter(name => /^[A-Za-z][A-Za-z0-9]{0,63}$/.test(name))
function plainScope(value) {
    const out = {}
    if (!value || typeof value !== "object") return out
    const source = value.getMap ? value.getMap() : value
    for (const name of Object.keys(source)) {
        const found = source[name]
        if (!/^[A-Za-z][A-Za-z0-9]{0,63}$/.test(name) || found == null || found === "") continue
        if (["string", "number", "boolean"].includes(typeof found)) out[name] = String(found).slice(0, 255)
    }
    return out
}

// ===== registrations =====
// The rows of a registry that are enabled and name what exists, each invalid one logged by name and left out;
// definitions are read once per call.
function registered(entityName, keyField, conditions = {}) {
    const services = new Map()
    const permissions = new Map()
    const serviceExists = name => {
        if (!services.has(name)) services.set(name, !!ec.service.getServiceDefinition(name))
        return services.get(name)
    }
    const permissionExists = id => {
        if (!permissions.has(id)) permissions.set(id, !!one("moqui.security.UserPermission", {userPermissionId: id}))
        return permissions.get(id)
    }
    const valid = []
    for (const row of rows(entityName, conditions)) {
        if (row.disabled === "Y") continue
        const key = row[keyField]
        try {
            if (entityName !== SETTINGS && !(text(row.serviceName) && serviceExists(text(row.serviceName)))) {
                ec.logger.error("Registry " + entityName + " " + key + " names a missing service " + text(row.serviceName))
                continue
            }
            const permission = entityName === SETTINGS ? row.viewPermission : row.requiredPermission
            if (permission && !permissionExists(permission)) {
                ec.logger.error("Registry " + entityName + " " + key + " names a missing permission " + permission)
                continue
            }
            valid.push(row)
        } catch (error) {
            ec.logger.error("Registry " + entityName + " " + key + " was not read: " + error.message)
        }
    }
    return valid.sort(
        (a, b) => (a.sequenceNum ?? 999) - (b.sequenceNum ?? 999) || String(a[keyField]).localeCompare(String(b[keyField]))
    )
}
const enumRows = enumTypeId =>
    rows("moqui.basic.Enumeration", {enumTypeId}).sort((a, b) => Number(a.sequenceNum ?? 99) - Number(b.sequenceNum ?? 99))
const enumNames = enumTypeId => Object.fromEntries(enumRows(enumTypeId).map(row => [row.enumId, row.description]))

// An owner's service as the caller, in its own transaction and error frame: its own checks apply, and a failure is an
// error the caller names, not a rollback of the page.
function ownerCall(serviceName, parameters) {
    ec.message.pushErrors()
    try {
        const result = ec.service.sync().name(serviceName).parameters(parameters).requireNewTransaction(true).call() || {}
        if (ec.message.hasError()) throw new Error(ec.message.getErrorsString())
        return result
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
}

// ===== checks =====
function check(checkKey, title, state, reason, message, extra = {}) {
    return Object.assign({checkKey, title, state, reasonCode: reason || null, message}, extra)
}
const ready = (checkKey, title, message, extra) => check(checkKey, title, "ready", null, message, extra)

// ===== the framework's own checks =====
// Each reads the framework's own records with authorization off, after its permission; none names a host, an address or
// a person. The keys are their registrations' (data/AdminConfigurationData.xml).
const backupJob = "backup_EmbeddedDatabase_daily"
const backupRemote = "EmbeddedDbBackup"
function lastSuccessfulRun(jobName) {
    const run = find("moqui.service.job.ServiceJobRun", {jobName})
        .condition("hasError", "not-equals", "Y")
        .condition("endTime", "is-not-null", null)
        .orderBy("-endTime")
        .limit(1)
        .one()
    return run?.endTime ?? null
}
const instanceChecks = {
    "admin.mailServer": () => {
        const server = one("moqui.basic.email.EmailServer", {emailServerId: "SYSTEM"})
        return [
            server?.smtpHost
                ? ready("admin.mailServer", "Outgoing mail", "The SYSTEM mail server has an SMTP host")
                : check(
                      "admin.mailServer",
                      "Outgoing mail",
                      "warning",
                      "MAIL_SERVER",
                      "The SYSTEM mail server has no SMTP host, so the instance sends no mail"
                  )
        ]
    },
    // on a fresh instance the job is paused and its mirror has no sendUrl, so both warn
    "admin.backup": () => {
        const job = one("moqui.service.job.ServiceJob", {jobName: backupJob})
        const remote = one("moqui.service.message.SystemMessageRemote", {systemMessageRemoteId: backupRemote})
        const watermark = job ? lastSuccessfulRun(backupJob) : null
        const mirrored = !!remote?.sendUrl
        const coverage = !job || job.paused === "Y" ? "None" : mirrored ? "Mirrored off the server" : "On the server only"
        const jobCheck = !job
            ? check(
                  "admin.backup.job",
                  "Nightly backup",
                  "unavailable",
                  "BACKUP_JOB_MISSING",
                  "The nightly backup job is not set up"
              )
            : job.paused === "Y"
              ? check("admin.backup.job", "Nightly backup", "warning", "BACKUP_PAUSED", "The nightly database backup is paused")
              : watermark == null
                ? check("admin.backup.job", "Nightly backup", "warning", "BACKUP_NONE_YET", "No backup has finished yet")
                : ready("admin.backup.job", "Nightly backup", "The nightly database backup runs")
        return [
            Object.assign(jobCheck, {watermark, coverage}),
            mirrored
                ? ready("admin.backup.mirror", "Off-site mirror", "Backups are mirrored off the server", {coverage})
                : check(
                      "admin.backup.mirror",
                      "Off-site mirror",
                      "warning",
                      "BACKUP_NOT_MIRRORED",
                      "Backups are not mirrored off the server",
                      {
                          coverage,
                          fixRoute: "adminConnectionDetail",
                          fixParameters: {adapterKey: "remote", connectionKey: backupRemote}
                      }
                  )
        ]
    },
    "admin.reportMail": () => {
        const template = one("moqui.basic.email.EmailTemplate", {emailTemplateId: "DASHBOARD_REPORT"})
        if (!template)
            return [
                check(
                    "admin.reportMail",
                    "Report mail",
                    "unavailable",
                    "REPORT_TEMPLATE_MISSING",
                    "The report email template is not installed"
                )
            ]
        const server = template.emailServerId
            ? one("moqui.basic.email.EmailServer", {emailServerId: template.emailServerId})
            : null
        // a missing mail server is fixed at the mail relays, not at the templates the provider's registration names
        return [
            server?.smtpHost
                ? ready("admin.reportMail", "Report mail", "Report emails have a mail server")
                : check(
                      "admin.reportMail",
                      "Report mail",
                      "warning",
                      "REPORT_MAIL_SERVER",
                      "Report emails have no mail server to send through",
                      {fixRoute: "adminMailRelay"}
                  )
        ]
    },
    "admin.reportAlertTopic": () => {
        const topic = one("moqui.security.user.NotificationTopic", {topic: "ReportAlert"})
        if (!topic)
            return [
                check(
                    "admin.reportAlertTopic",
                    "Report alerts",
                    "unavailable",
                    "TOPIC_MISSING",
                    "The report alert topic is not installed"
                )
            ]
        return [
            topic.receiveNotifications === "N"
                ? check("admin.reportAlertTopic", "Report alerts", "warning", "TOPIC_OFF", "Report alerts are not sent to anyone")
                : ready("admin.reportAlertTopic", "Report alerts", "Report alerts are sent to their recipients")
        ]
    },
    // a member of ADMIN or of a *_ADMIN group, enabled and not ended, with no authentication factor in date: counted
    "admin.adminFactors": () => {
        const now = nowMs()
        const inDate = row => (ms(row.fromDate) ?? -Infinity) <= now && (ms(row.thruDate) ?? Infinity) > now
        const groupIds = ["ADMIN"].concat(
            rows("moqui.security.UserGroup", {})
                .map(row => row.userGroupId)
                .filter(id => /_ADMIN$/.test(id))
        )
        const userIds = Array.from(
            new Set(
                Array.from(find("moqui.security.UserGroupMember").condition("userGroupId", "in", groupIds).list())
                    .filter(inDate)
                    .map(row => row.userId)
            )
        )
        const accounts = userIds.length
            ? Array.from(find("moqui.security.UserAccount").condition("userId", "in", userIds).list()).filter(
                  row => row.disabled !== "Y" && (ms(row.terminateDate) ?? Infinity) > now
              )
            : []
        const withFactor = new Set(
            accounts.length
                ? Array.from(
                      find("moqui.security.UserAuthcFactor")
                          .condition(
                              "userId",
                              "in",
                              accounts.map(row => row.userId)
                          )
                          .list()
                  )
                      .filter(inDate)
                      .map(row => row.userId)
                : []
        )
        const missing = accounts.filter(row => !withFactor.has(row.userId)).length
        return [
            missing
                ? check(
                      "admin.adminFactors",
                      "Administrator factors",
                      "warning",
                      "ADMIN_NO_FACTOR",
                      missing + (missing === 1 ? " administrator has" : " administrators have") + " no second sign-in factor"
                  )
                : ready("admin.adminFactors", "Administrator factors", "Every administrator has a second sign-in factor")
        ]
    },
    // failing runs of the last day, with the list of them as the fix place
    "admin.jobRuns": () => {
        const since = new Date(nowMs() - 24 * 60 * minute)
        const failed = Array.from(
            find("moqui.service.job.ServiceJobRun")
                .condition("hasError", "Y")
                .condition("startTime", "greater-equals", since)
                .selectField("jobName")
                .limit(500)
                .list()
        )
        const jobs = Array.from(new Set(failed.map(row => row.jobName)))
        return [
            failed.length
                ? check(
                      "admin.jobRuns",
                      "Job runs",
                      "failed",
                      "JOB_RUNS_FAILED",
                      failed.length +
                          (failed.length === 1 ? " job run" : " job runs") +
                          " failed in the last day: " +
                          jobs.slice(0, 3).join(", ") +
                          (jobs.length > 3 ? " and " + (jobs.length - 3) + " more" : ""),
                      {fixRoute: "adminJobRuns", fixParameters: {state: "failed", fromDate: ec.l10n.format(since, "yyyy-MM-dd")}}
                  )
                : ready("admin.jobRuns", "Job runs", "No job run failed in the last day")
        ]
    },
    // a member is silent past three heartbeats (one a scheduler pass, a minute by default); its peers delete its row
    // after ten
    "admin.cluster": () => {
        const members = rows("moqui.server.ClusterMember", {})
        const silent = members.filter(row => (ms(row.heartbeat) ?? 0) < nowMs() - 3 * minute).length
        return [
            silent
                ? check(
                      "admin.cluster",
                      "Cluster",
                      "warning",
                      "CLUSTER_STALE",
                      silent + " of " + members.length + " cluster members have not been heard from for three minutes"
                  )
                : ready("admin.cluster", "Cluster", members.length > 1 ? "Every cluster member is heard from" : "One server")
        ]
    },
    // the telemetry pipeline's counters, the framework's own service read with authorization off so a reader without
    // Monitor sees the state, not the counts
    "admin.telemetry": () => {
        const out =
            ec.service
                .sync()
                .name("org.moqui.monitor.MonitorServices.get#SourceStatus")
                .disableAuthz()
                .requireNewTransaction(true)
                .call() || {}
        if (ec.message.hasError()) throw new Error(ec.message.getErrorsString())
        const statuses = Array.from(out.sources || []).map(row => String(row.status || ""))
        if (statuses.some(status => status.includes("loss")))
            return [
                check(
                    "admin.telemetry",
                    "Telemetry",
                    "warning",
                    "TELEMETRY_LOSS",
                    "Telemetry is being lost, so health is unknown"
                )
            ]
        if (!statuses.length || statuses.some(status => status.includes("unknown")))
            return [
                check(
                    "admin.telemetry",
                    "Telemetry",
                    "unavailable",
                    "TELEMETRY_UNKNOWN",
                    "Telemetry is off or not searchable, so health is unknown"
                )
            ]
        return [ready("admin.telemetry", "Telemetry", "Telemetry is received")]
    },
    // the Constitution for the instance: the sources it passed over, counted, not shown
    "constitution.global": () => {
        const out = ownerCall("org.moqui.admin.ConstitutionServices.get#Constitution", {scope: "global"})
        const passedOver = Array.from(out.diagnosticList || []).length
        if (!out.available)
            return [
                check(
                    "constitution.global",
                    "Instance Constitution",
                    "unavailable",
                    "CONSTITUTION_UNAVAILABLE",
                    "The Constitution could not be read"
                )
            ]
        return [
            passedOver
                ? check(
                      "constitution.global",
                      "Instance Constitution",
                      "warning",
                      "CONSTITUTION_SOURCES",
                      passedOver + (passedOver === 1 ? " Constitution source was" : " Constitution sources were") + " passed over"
                  )
                : ready(
                      "constitution.global",
                      "Instance Constitution",
                      out.customized ? "The instance's own Constitution applies" : "The shipped Constitution applies"
                  )
        ]
    },
    // the protocols: a check for each one that is turned on and failing or running without its trust; one that is off
    // or not in the build is no problem. get#ProtocolReadiness checks its own permission, so this one asks no other
    "admin.protocols": () => {
        const protocols = Array.from(
            ownerCall("org.moqui.admin.ConfigurationServices.get#ProtocolReadiness", {}).protocolList || []
        )
        const found = protocols
            .filter(protocol => ["failing", "untrusted"].includes(protocol.state))
            .map(protocol =>
                check(
                    "admin.protocols." + protocol.protocolKey,
                    protocol.title,
                    protocol.state === "failing" ? "failed" : "warning",
                    protocol.state === "failing" ? "PROTOCOL_FAILING" : "PROTOCOL_UNTRUSTED",
                    protocol.reason || protocol.stateText
                )
            )
        const running = protocols.filter(protocol => protocol.state === "ready").length
        return found.length
            ? found
            : [ready("admin.protocols", "Protocols", running + (running === 1 ? " protocol is" : " protocols are") + " running")]
    }
}
// the checks that ask no permission here: the service each reads checks the caller itself
const ownPermission = ["admin.protocols"]
// check#InstanceSetup: one of the framework's checks, by its registration's key
function instanceSetup(p) {
    lib.signedIn()
    const key = text(p.providerKey)
    // the permission before the key, so a caller without it learns nothing of which checks there are
    if (!ownPermission.includes(key) && !ec.user.hasPermission(OVERVIEW)) denied("PERMISSION_DENIED", OVERVIEW + " is required")
    if (!Object.hasOwn(instanceChecks, key)) throw new Error("Unknown instance check " + key)
    return {checks: instanceChecks[key]()}
}

// ===== readiness =====
// The check rows a screen shows: the provider's and the check's names, the state, whom or what it is for, the times as
// text in the user's zone, the freshness facet and the owner's fix place as a key with its record and parameters (for a
// check that is not ready), never a URL.
function frequencyText(minutes) {
    const value = Number(minutes)
    if (value < 120) return value + (value === 1 ? " minute" : " minutes")
    if (value < 48 * 60) return Math.round(value / 60) + " hours"
    return Math.round(value / (24 * 60)) + " days"
}
function present(provider, row, context) {
    const state = Object.hasOwn(states, row.state) ? row.state : "unavailable"
    const refresh = context.refreshByProvider[provider.providerKey]
    const out = {
        providerKey: provider.providerKey,
        providerTitle: provider.title,
        ownerApp: provider.ownerApp,
        healthGroupEnumId: provider.healthGroupEnumId,
        groupName: context.groupNames[provider.healthGroupEnumId] || "",
        checkKey: row.checkKey || provider.providerKey,
        title: row.title || provider.title,
        state,
        stateName: states[state],
        reasonCode: row.state === state ? row.reasonCode || null : "STATE_UNKNOWN",
        message: String(row.message || ""),
        subject: String(row.subject || "Instance"),
        // what narrows a request to this check, as its owner answered it: sent back as a refresh's scope parameters
        scope: plainScope(row.scope),
        checkedText: context.checkedText,
        coverage: String(row.coverage || ""),
        watermarkText: "",
        lagText: "",
        // the lag in minutes, for a reader that applies a freshness policy of its own
        lagMinutes: null,
        expectedFrequencyText: "",
        fixRoute: null,
        fixId: null,
        fixParameters: {},
        refreshKey: refresh?.refreshKey || null,
        refreshTitle: refresh?.title || null
    }
    // the freshness facet: a data source's last watermark, its expected frequency and its lag, stale past the lag
    // allowed; on the checks that carry a watermark (none yet, for one never refreshed)
    if (provider.expectedFrequencyMinutes != null && Object.hasOwn(row, "watermark")) {
        out.expectedFrequencyText = frequencyText(provider.expectedFrequencyMinutes)
        const watermark = ms(row.watermark)
        if (watermark != null) {
            const lag = Math.max(0, Math.round((nowMs() - watermark) / minute))
            out.watermarkText = timeText(row.watermark)
            out.lagText = frequencyText(lag)
            out.lagMinutes = lag
            if (provider.maxLagMinutes != null && lag > provider.maxLagMinutes && out.state === "ready")
                Object.assign(out, {
                    state: "warning",
                    stateName: states.warning,
                    reasonCode: "STALE",
                    message: "Last refreshed " + out.lagText + " ago"
                })
        }
    }
    if (out.state !== "ready") {
        const own = text(row.fixRoute)
        out.fixRoute = own || text(provider.fixRoute) || null
        out.fixId = (own ? text(row.fixId) : text(provider.fixId)) || null
        out.fixParameters = own ? plainScope(row.fixParameters) : {}
    }
    out.stateColor = stateColors[out.state]
    return out
}
function readiness(p) {
    lib.signedIn()
    // the providers the caller may read; one whose permission the caller lacks is omitted, from the owners' names too
    const readable = registered(PROVIDER, "providerKey").filter(row => permitted(row.requiredPermission))
    const providers = readable.filter(
        row =>
            (!p.providerKey || row.providerKey === p.providerKey) &&
            (!p.ownerApp || row.ownerApp === p.ownerApp) &&
            (!p.healthGroupEnumId || row.healthGroupEnumId === p.healthGroupEnumId)
    )
    const refreshByProvider = {}
    for (const row of registered(REFRESH, "refreshKey"))
        if (row.providerKey && permitted(row.requiredPermission) && !refreshByProvider[row.providerKey])
            refreshByProvider[row.providerKey] = row
    const scope = plainScope(p.scopeParameters)
    const asked = Object.keys(scope)
    const instanceOnly = p.instanceOnly === true || p.instanceOnly === "true" || p.instanceOnly === "Y"
    const groups = enumRows("AdminHealthGroup")
    const context = {
        groupNames: Object.fromEntries(groups.map(row => [row.enumId, row.description])),
        checkedText: timeText(ec.user.nowTimestamp),
        refreshByProvider
    }
    const checks = []
    const answered = []
    const failedProviders = []
    // the providers whose service refused the caller: left out of the checks and of the owners and groups offered
    const refused = new Set()
    for (const provider of providers) {
        // a provider that takes none of the scope asked for has nothing of it: left out; one that takes a scope is not
        // the instance's own, and is left out when only those are asked for
        const scoped = names(provider.scopeParameters)
        const taken = scoped.filter(name => asked.includes(name))
        if (asked.length && !taken.length) continue
        if (instanceOnly && scoped.length) continue
        let found
        try {
            const parameters = Object.fromEntries(taken.map(name => [name, scope[name]]))
            parameters.providerKey = provider.providerKey
            found = Array.from(ownerCall(text(provider.serviceName), parameters).checks || [])
        } catch (error) {
            if (isRefusal(error)) {
                refused.add(provider.providerKey)
                continue
            }
            ec.logger.error(
                "Readiness provider " + provider.providerKey + " (" + provider.serviceName + ") failed: " + error.message
            )
            failedProviders.push(provider.title)
            found = [
                check(
                    provider.providerKey,
                    provider.title,
                    "unavailable",
                    "PROVIDER_FAILED",
                    provider.title + " could not be checked"
                )
            ]
        }
        if (!found.length) continue
        const presented = found.map(row => present(provider, row || {}, context))
        // a provider that failed for one of its subjects is named too, beside its unavailable row
        if (!failedProviders.includes(provider.title) && presented.some(row => row.reasonCode === "PROVIDER_FAILED"))
            failedProviders.push(provider.title)
        checks.push(...presented)
        answered.push({
            providerKey: provider.providerKey,
            title: provider.title,
            ownerApp: provider.ownerApp,
            state: presented.reduce((worst, row) => (stateOrder[row.state] < stateOrder[worst] ? row.state : worst), "ready")
        })
    }
    const shown = p.state ? checks.filter(row => row.state === p.state) : checks
    // the owners and groups offered are those of the providers the caller may read, whatever the find narrowed to, less
    // those whose service refused the caller here (a provider the find left out is not asked, so it stays offered)
    const offered = readable.filter(row => !refused.has(row.providerKey))
    const offeredGroups = new Set(offered.map(row => row.healthGroupEnumId))
    return {
        checks: shown,
        providers: answered,
        failedProviders,
        checkedText: context.checkedText,
        ownerOptions: Array.from(new Set(offered.map(row => row.ownerApp).filter(Boolean))).sort(),
        groupOptions: groups.filter(row => offeredGroups.has(row.enumId)).map(row => ({key: row.enumId, text: row.description}))
    }
}

// ===== refresh =====
// request#Refresh: a registered refresh operation by its key, refused without its own permission; the owner's service is
// called as the caller with the registration's key, the operation key, the reason and only the scope parameters the
// registration names, which the owner checks. A failure of the owner's service is its safe result, not an error of the
// page.
function requestRefresh(p) {
    lib.signedIn()
    const operation = registered(REFRESH, "refreshKey", {refreshKey: p.refreshKey || "_NA_"})[0]
    if (!operation) denied("REFRESH_UNAVAILABLE", "Refresh " + text(p.refreshKey) + " is not available")
    if (!permitted(operation.requiredPermission))
        denied("REFRESH_DENIED", operation.requiredPermission + " is required to refresh " + operation.title)
    const scope = plainScope(p.scopeParameters)
    // the operation key, the reason and the registration's key never pass as scope parameters, whatever a row lists
    const reserved = ["operationKey", "reason", "refreshKey"]
    const parameters = Object.fromEntries(
        names(operation.scopeParameters)
            .filter(name => !reserved.includes(name) && scope[name] != null)
            .map(name => [name, scope[name]])
    )
    // the registration's key, so an owner with one service for several sources reads which
    parameters.refreshKey = operation.refreshKey
    if (p.operationKey) parameters.operationKey = text(p.operationKey).slice(0, 64)
    if (p.reason) parameters.reason = String(p.reason).slice(0, 255)
    const result = {refreshKey: operation.refreshKey, title: operation.title, completedText: ""}
    try {
        ownerCall(text(operation.serviceName), parameters)
        return Object.assign(result, {
            state: "requested",
            message: operation.title + " was requested",
            completedText: timeText(ec.user.nowTimestamp)
        })
    } catch (error) {
        ec.logger.error("Refresh " + operation.refreshKey + " (" + operation.serviceName + ") failed: " + error.message)
        return Object.assign(result, {state: "failed", message: operation.title + " could not be requested"})
    }
}

// ===== overview =====
// The health cards (failed job runs and sign-in failures of the last day, system messages in error, backup coverage),
// each with the key of the place that lists it; a count that cannot be read is shown unavailable. They are counts of
// their own, so they run no provider: the backup's coverage is what the caller's readiness run found (the overview's),
// else what the backup check alone answers.
function cardsOf(backupCoverage) {
    const since = new Date(nowMs() - 24 * 60 * minute)
    const count = (label, read) => {
        try {
            return String(read())
        } catch (error) {
            ec.logger.error("Overview " + label + " not counted: " + error.message)
            return "Unavailable"
        }
    }
    return [
        {
            key: "failedJobs",
            title: "Failed job runs",
            value: count("failed jobs", () =>
                find("moqui.service.job.ServiceJobRun")
                    .condition("hasError", "Y")
                    .condition("startTime", "greater-equals", since)
                    .count()
            ),
            icon: "sym_o_event_busy",
            route: "adminJobRuns",
            routeId: null,
            parameters: {state: "failed"}
        },
        {
            key: "transports",
            title: "Unresolved transports",
            value: count("transports", () =>
                find("moqui.service.message.SystemMessage").condition("statusId", "SmsgError").count()
            ),
            icon: "sym_o_sync_problem",
            route: "adminSystemMessages",
            routeId: null,
            parameters: {state: "failed"}
        },
        {
            key: "backup",
            title: "Backup coverage",
            value: backupCoverage || "Unavailable",
            icon: "sym_o_backup",
            route: "adminJob",
            routeId: backupJob,
            parameters: {}
        },
        {
            key: "signIn",
            title: "Sign-in failures",
            value: count("sign-in failures", () =>
                find("moqui.security.UserLoginHistory")
                    .condition("successfulLogin", "N")
                    .condition("fromDate", "greater-equals", since)
                    .count()
            ),
            icon: "sym_o_lock_person",
            route: "adminVisits",
            routeId: null,
            parameters: {}
        }
    ]
}
// get#Overview: what needs attention (the checks that are not ready, worst first, at most ten) and the health cards. A
// reader of the overview's (ADMIN_OVERVIEW_VIEW).
function overview(p) {
    requireOverview()
    const found = readiness({scopeParameters: p.scopeParameters})
    const attention = found.checks
        .filter(row => row.state !== "ready")
        .sort((a, b) => stateOrder[a.state] - stateOrder[b.state])
        .slice(0, 10)
    const backup = found.checks.find(row => row.checkKey === "admin.backup.job")
    return {attention, cards: cardsOf(backup?.coverage), failedProviders: found.failedProviders, checkedText: found.checkedText}
}
// get#HealthCards: the health cards alone, with no provider run (Health's header shows the sign-in failures). A reader of
// the overview's (ADMIN_OVERVIEW_VIEW).
function healthCards() {
    requireOverview()
    let coverage = null
    try {
        coverage = instanceChecks["admin.backup"]().find(row => row.checkKey === "admin.backup.job")?.coverage || null
    } catch (error) {
        ec.logger.error("Overview backup coverage not read: " + error.message)
    }
    return {cards: cardsOf(coverage)}
}

// ===== settings index =====
// Whether a screen opens to the caller's own groups, as the framework decides a screen path (authz.rs is_permitted):
// the screens above it in its component's screen directory are counted down to it; an always-allow on a screen opens
// it and, inheritable, every screen below; a deny refuses its screen and any allow below it; the last screen needs an
// allow or always-allow of its own, or one inherited from the screen above it. The user's authorizations are read
// directly, so nothing of the frames on the stack (an app's inheritable always-allow) widens them.
function screenAuthz() {
    const groupIds = Array.from(ec.user.getUserGroupIdSet() || [])
    if (!groupIds.length) return []
    return Array.from(
        find("moqui.security.ArtifactAuthzCheckView")
            .condition("userGroupId", "in", groupIds)
            .condition("artifactTypeEnumId", "AT_XML_SCREEN")
            .condition("authzActionEnumId", "in", ["AUTHZA_ALL", "AUTHZA_VIEW"])
            .list()
    )
}
function locationsOf(location) {
    const parts = /^(component:\/\/[^/]+\/screen\/)(.+)\.xml$/.exec(text(location))
    if (!parts) return []
    const steps = parts[2].split("/")
    return steps.map((_, index) => parts[1] + steps.slice(0, index + 1).join("/") + ".xml")
}
function screenOpens(location, authzRows) {
    const locations = locationsOf(location)
    let inherited = null
    let refused = false
    for (const [index, step] of locations.entries()) {
        const last = index === locations.length - 1
        if (inherited?.inheritable && inherited.type === "AUTHZT_ALWAYS") return true
        const grants = authzRows.filter(row => {
            if (row.nameIsPattern !== "Y") return row.artifactName === step
            try {
                return new RegExp("^(?:" + row.artifactName + ")$").test(step)
            } catch (error) {
                return false
            }
        })
        const own =
            grants.find(row => row.authzTypeEnumId === "AUTHZT_ALWAYS") ||
            grants.find(row => row.authzTypeEnumId === "AUTHZT_DENY") ||
            (refused ? null : grants.find(row => row.authzTypeEnumId === "AUTHZT_ALLOW"))
        if (own) {
            if (own.authzTypeEnumId === "AUTHZT_DENY") refused = true
            if (last) return own.authzTypeEnumId !== "AUTHZT_DENY"
            inherited = {type: own.authzTypeEnumId, inheritable: own.inheritAuthz === "Y"}
        } else if (last) {
            return !!inherited?.inheritable && inherited.type !== "AUTHZT_DENY"
        } else if (!inherited?.inheritable) {
            inherited = null
        }
    }
    return false
}
// find#Settings: the owners' settings landings the caller may open, each shown only when its screen opens to the
// caller's own groups and its view permission holds, by group; searched by text in the query, by app and by group. Any
// signed-in caller's: one who opens no destination is answered none.
function settings(p) {
    lib.signedIn()
    const authzRows = screenAuthz()
    const opened = entry => {
        try {
            return (
                permitted(entry.viewPermission) && !!text(entry.destinationRoute) && screenOpens(entry.screenLocation, authzRows)
            )
        } catch (error) {
            ec.logger.error("Settings entry " + entry.settingsKey + " was not checked: " + error.message)
            return false
        }
    }
    const all = registered(SETTINGS, "settingsKey").filter(opened)
    let entries = all.filter(
        row =>
            (!p.ownerApp || row.ownerApp === p.ownerApp) &&
            (!p.settingsGroupEnumId || row.settingsGroupEnumId === p.settingsGroupEnumId)
    )
    const wanted = text(p.text)
    if (wanted && entries.length) {
        const factory = ec.entity.conditionFactory
        const like = "%" + wanted.replace(/[\\%_]/g, character => "\\" + character) + "%"
        const matching = new Set(
            Array.from(
                find(SETTINGS)
                    .condition(
                        "settingsKey",
                        "in",
                        entries.map(row => row.settingsKey)
                    )
                    .condition(
                        factory.makeCondition(
                            ["title", "description", "ownerApp", "configuredBy"].map(field =>
                                factory.makeCondition(field, "like", like).ignoreCase()
                            ),
                            "or"
                        )
                    )
                    .selectField("settingsKey")
                    .list()
            ).map(row => row.settingsKey)
        )
        entries = entries.filter(row => matching.has(row.settingsKey))
    }
    const groups = enumRows("AdminSettingsGroup")
    const groupOrder = Object.fromEntries(groups.map(row => [row.enumId, Number(row.sequenceNum ?? 99)]))
    const groupNames = Object.fromEntries(groups.map(row => [row.enumId, row.description]))
    const scopeNames = enumNames("AdminRegistryScope")
    const shown = entries.map(entry => ({
        settingsKey: entry.settingsKey,
        title: entry.title,
        description: entry.description || "",
        ownerApp: entry.ownerApp,
        settingsGroupEnumId: entry.settingsGroupEnumId,
        groupName: groupNames[entry.settingsGroupEnumId] || "",
        scopeName: scopeNames[entry.scopeEnumId] || "",
        configuredBy: entry.configuredBy || "",
        destinationRoute: text(entry.destinationRoute),
        providerKey: entry.providerKey || null,
        sequence: groupOrder[entry.settingsGroupEnumId] ?? 99
    }))
    shown.sort((a, b) => a.sequence - b.sequence)
    // the apps to choose from are those of the entries the caller may open, whatever the search
    return {
        entries: shown,
        appOptions: Array.from(new Set(all.map(row => row.ownerApp))).sort(),
        groupOptions: groups.map(row => ({key: row.enumId, text: row.description}))
    }
}

// ===== readiness and support summary =====
// get#SupportSummary: one projection for the support summary, its print and its redacted JSON, a reader of the
// overview's (ADMIN_OVERVIEW_VIEW). It holds the instance (its display name, the framework's version and package, when
// it was prepared), the readiness checks the caller may read (readiness() above: each provider's checks with their
// state, reason code, message, checked time and freshness, a provider that failed named), the protocols' states
// (get#ProtocolReadiness) and the effective settings (get#EffectiveSettings), each read with the caller's own
// permissions: a section the caller cannot read is marked unavailable with its reason, never shown empty. A secret
// setting is only "set" or "not set" (the settings service never answers its value). Never in it: the environment, the
// host's facts, listeners, paths, users, fix places, refresh keys, a check's scope or its diagnostics. Redacted (the
// JSON) also leaves out who prepared it and writes an email address in any text as [address], so the file can be given
// to someone outside.
const addressPattern = /[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/g
// The settings that name the server's inside: the addresses a listener binds, the gateway addresses it admits and a
// certificate file's path. The screen and print show them to a viewer who reads them in the instance settings anyway;
// the redacted JSON, for someone outside, says only whether each is set. The public names (publicHost, publicAddress,
// domain, the public URLs) stay, as they are given to every browser and client.
const internalSetting = key =>
    /^protocol\.[a-z]+\.(host|listenHost|mediaHost|allowedAddresses)$/.test(String(key || "")) || /File$/.test(String(key || ""))
function supportSummary(p) {
    requireOverview()
    const redacted = p.redacted === true || p.redacted === "true" || p.redacted === "Y"
    const clean = value => (value == null ? value : redacted ? String(value).replace(addressPattern, "[address]") : String(value))
    const section = (name, parameters, fallback) => calls.read(name, parameters, fallback)
    // the instance
    const named = section("org.moqui.admin.ConfigurationServices.get#InstancePreference", {settingKey: "instance.displayName"})
    let report = {}
    try {
        report = ec.factory.getStartupReport() || {}
    } catch (error) {
        report = {}
    }
    const instance = {
        displayName: clean(named.out?.value || ""),
        version: String(report.version || ec.factory.moquiVersion || ""),
        packageName: String(report.package || ""),
        preparedDate: ec.user.nowTimestamp ? new Date(nowMs()).toISOString() : null,
        preparedText: timeText(ec.user.nowTimestamp),
        preparedBy: redacted ? null : ec.user.userAccount?.userFullName || ec.user.username || null
    }
    // the readiness checks the caller may read
    let readinessSection
    try {
        const found = readiness({})
        readinessSection = {
            available: true,
            checkedText: found.checkedText,
            failedProviders: found.failedProviders,
            counts: Object.fromEntries(
                Object.keys(states).map(state => [state, found.checks.filter(row => row.state === state).length])
            ),
            checks: found.checks.map(row => ({
                providerKey: row.providerKey,
                providerTitle: row.providerTitle,
                ownerApp: row.ownerApp,
                groupName: row.groupName,
                checkKey: row.checkKey,
                title: row.title,
                subject: clean(row.subject),
                state: row.state,
                stateName: row.stateName,
                reasonCode: row.reasonCode,
                message: clean(row.message),
                coverage: clean(row.coverage),
                freshnessText: row.watermarkText
                    ? "Last " +
                      row.watermarkText +
                      (row.lagText ? ", " + row.lagText + " ago" : "") +
                      (row.expectedFrequencyText ? "; every " + row.expectedFrequencyText : "")
                    : row.expectedFrequencyText
                      ? "Every " + row.expectedFrequencyText
                      : "",
                lagMinutes: row.lagMinutes
            }))
        }
    } catch (error) {
        readinessSection = {
            available: false,
            reason: calls.reasonOf(error?.message) || "The checks are not available to you.",
            checks: []
        }
    }
    // the protocols
    const protocols = section(
        "org.moqui.admin.ConfigurationServices.get#ProtocolReadiness",
        {},
        "The protocols are not available to you."
    )
    const protocolSection = protocols.out
        ? {
              available: true,
              protocols: Array.from(protocols.out.protocolList || []).map(row => ({
                  protocolKey: row.protocolKey,
                  title: row.title,
                  built: !!row.built,
                  enabled: !!row.enabled,
                  state: row.state,
                  stateText: row.stateText,
                  reason: clean(row.reason || ""),
                  trustState: row.trustState || null,
                  checks: Array.from(row.checkList || []).map(found => ({
                      checkKey: found.checkKey,
                      title: found.title,
                      lastCheckPassed: found.lastCheckPassed ?? null,
                      lastCheckDate: found.lastCheckDate || null
                  }))
              }))
          }
        : {available: false, reason: protocols.failure, protocols: []}
    // the effective settings, a secret as set or not
    const settingsFound = section(
        "org.moqui.admin.ConfigurationServices.get#EffectiveSettings",
        {},
        "The settings are not available to you."
    )
    const settingSection = settingsFound.out
        ? {
              available: true,
              settings: Array.from(settingsFound.out.settingList || []).map(row => {
                  const hidden = redacted && internalSetting(row.settingKey) && !row.masked
                  const set = !(row.value === "" || row.value == null)
                  return {
                      settingKey: row.settingKey,
                      title: row.title,
                      settingGroup: row.settingGroup,
                      value: row.masked || hidden ? null : clean(row.value),
                      shownValue: hidden
                          ? set
                              ? "[internal] (set)"
                              : "Not set"
                          : row.masked
                            ? row.valueSet
                                ? "Hidden (set)"
                                : "Hidden (not set)"
                            : row.value === "" || row.value == null
                              ? "Not set"
                              : clean(row.value),
                      masked: !!row.masked,
                      internal: hidden,
                      valueSet: hidden ? set : !!row.valueSet,
                      source: row.source,
                      sourceText: row.sourceText,
                      restartRequired: !!row.restartRequired
                  }
              })
          }
        : {available: false, reason: settingsFound.failure, settings: []}
    return {
        summary: {
            kind: "moqui.admin.supportSummary",
            formatVersion: 1,
            redacted,
            instance,
            readiness: readinessSection,
            protocols: protocolSection,
            settings: settingSection
        }
    }
}

const operations = {readiness, requestRefresh, overview, healthCards, settings, instanceSetup, supportSummary}
;({
    execute: (op, parameters) => {
        if (!operations[op]) throw new Error("Unknown readiness operation " + op)
        return operations[op](parameters)
    }
})
