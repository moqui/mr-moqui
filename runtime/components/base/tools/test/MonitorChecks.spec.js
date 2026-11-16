// Monitor › Checks: a scrape target or synthetic check reaches loopback and private addresses only with
// allowPrivateNetwork Y, which store#ScrapeTarget and store#SyntheticCheck let only a holder of MONITOR_PRIVATE_NETWORK set,
// or keep while the URL changes (SEC-OUT-03, SEC-TLS-13). The spec's user has System access only, through a group of its
// own, until the second feature grants that group the permission.
// Run: ./moq test tools --spec MonitorChecks
const SERVICES = "org.moqui.monitor.TelemetryServices."
const USER = "MONITOR_CHECKS_SPEC"
const USERNAME = "monitor.checks.spec"
const GROUP = "MONITOR_CHECKS_SPEC"
const TARGET = "CHECKS_SPEC_T"
const CHECK = "CHECKS_SPEC_C"

function sys(run) {
    ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        ec.artifactExecution.enableAuthz()
    }
}
function make(entityName, fields) {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
// The errors a call left, cleared.
function errorsOf(name, parameters) {
    ec.service.sync().name(SERVICES + name).parameters(parameters).call()
    const errors = ec.message.getErrorsString()
    ec.message.clearErrors()
    return errors
}
function flagOf(entityName, key, id) {
    return sys(() => {
        const row = ec.entity.find(entityName).condition(key, id).useCache(false).one()
        return row ? {flag: row.allowPrivateNetwork || "", url: row.url} : null
    })
}
function removeRows() {
    sys(() => {
        ec.entity.find("moqui.monitor.ScrapeTarget").condition("targetId", TARGET).deleteAll()
        ec.entity.find("moqui.monitor.SyntheticCheck").condition("checkId", CHECK).deleteAll()
    })
}
function ungrant() {
    sys(() => {
        const rows = ec.entity.find("moqui.security.UserGroupPermission").condition("userGroupId", GROUP).useCache(false).list()
        for (const row of rows) row.delete()
    })
}

spec({
    setupSpec() {
        sys(() => {
            if (ec.entity.find("moqui.security.UserAccount").condition("userId", USER).count() === 0) {
                make("moqui.security.UserAccount", {userId: USER, username: USERNAME, userFullName: "Monitor Checks Spec", disabled: "N"})
                make("moqui.security.UserGroup", {userGroupId: GROUP, description: "System app only (MonitorChecks spec)"})
                make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId: USER, fromDate: new Date(Date.now() - 60000)})
                make("moqui.security.ArtifactAuthz", {artifactAuthzId: GROUP, artifactGroupId: "SYSTEM_APP", userGroupId: GROUP,
                    authzTypeEnumId: "AUTHZT_ALLOW", authzActionEnumId: "AUTHZA_ALL"})
            }
        })
        removeRows()
        ec.user.internalLoginUser(USERNAME)
    },
    cleanupSpec() {
        ungrant()
        removeRows()
    }
})

feature("without MONITOR_PRIVATE_NETWORK a target or check cannot be let onto the private network", {
    when() {
        const raised = errorsOf("store#ScrapeTarget", {targetId: TARGET, url: "http://127.0.0.1:9100/metrics", allowPrivateNetwork: "Y"})
        const notMade = flagOf("moqui.monitor.ScrapeTarget", "targetId", TARGET)
        const plain = errorsOf("store#ScrapeTarget", {targetId: TARGET, url: "https://metrics.example.com/metrics"})
        const made = flagOf("moqui.monitor.ScrapeTarget", "targetId", TARGET)
        const checkRaised = errorsOf("store#SyntheticCheck", {checkId: CHECK, url: "http://10.0.0.5/", allowPrivateNetwork: "Y"})
        // an administrator allowed the row; the user may still change its interval, not its URL
        sys(() => ec.entity.find("moqui.monitor.ScrapeTarget").condition("targetId", TARGET).one().set("allowPrivateNetwork", "Y").update())
        const interval = errorsOf("store#ScrapeTarget", {targetId: TARGET, url: "https://metrics.example.com/metrics", intervalSeconds: 120})
        const moved = errorsOf("store#ScrapeTarget", {targetId: TARGET, url: "http://127.0.0.1:8080/admin"})
        const after = flagOf("moqui.monitor.ScrapeTarget", "targetId", TARGET)
        return {raised, notMade, plain, made, checkRaised, interval, moved, after}
    },
    then({raised, notMade, plain, made, checkRaised, interval, moved, after}) {
        raised.includes("MONITOR_PRIVATE_NETWORK")
        notMade == null
        plain == ""
        made.flag == ""
        checkRaised.includes("MONITOR_PRIVATE_NETWORK")
        interval == ""
        moved.includes("MONITOR_PRIVATE_NETWORK")
        after.flag == "Y"
        after.url == "https://metrics.example.com/metrics"
    }
})

feature("a holder of MONITOR_PRIVATE_NETWORK lets a check reach the private network", {
    given() {
        sys(() => make("moqui.security.UserGroupPermission", {userGroupId: GROUP, userPermissionId: "MONITOR_PRIVATE_NETWORK",
            fromDate: new Date(Date.now() - 60000)}))
    },
    when() {
        const stored = errorsOf("store#SyntheticCheck", {checkId: CHECK, name: "Exporter", url: "http://10.0.0.5/", allowPrivateNetwork: "Y"})
        const check = flagOf("moqui.monitor.SyntheticCheck", "checkId", CHECK)
        const moved = errorsOf("store#ScrapeTarget", {targetId: TARGET, url: "http://127.0.0.1:9100/metrics"})
        ungrant()
        return {stored, check, moved}
    },
    then({stored, check, moved}) {
        stored == ""
        check.flag == "Y"
        moved == ""
    }
})
