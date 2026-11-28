// The System home's attention list (plans/archive/Tools-Fancy.md §3.4, B5): moqui.tools.SystemHomeServices.get#SystemAttention.
// Rule tests, no screen is rendered. The first feature calls the service on the database: a failed job run is a
// readiness check that is not ready (org.moqui.admin.ReadinessServices.get#Readiness, the one source Health reads too;
// plans/archive/Admin-Screens-Down.md A6), listed for an administrator with its fix place, and left out for a user who may
// call the service but holds no provider's permission. The second runs the service's script over readings, finds and
// checks of its own, since a spec cannot fill a disk: a resource at warning is listed with its number, the resources
// come first, the checks that are not ready follow by their state with a System place or none, and one item's failure
// leaves the others. The listener items (plans/Runtime-Config.md §3.7) come from the same run, over a startup report
// and the answer of ListenerServices.get#Listeners of its own, since no listeners run under moqui test: one that
// failed after a change is apart from the start's failures, plain HTTP in public with no proxy is a warning, and no
// way in over the network is critical when a change while running left it so, not for a server that started that way
// (the desktop node); where each way in reaches is get#Listeners' own (reachable, plainPublic: the listener guards'
// rules), and each item links to Conf › Services and ports.
// Run: ./moq test tools --spec SystemAttention
const SERVICE = "moqui.tools.SystemHomeServices.get#SystemAttention"
const USER = "SYSTEM_ATTENTION_SPEC"
const USERNAME = "system.attention.spec"
const GROUP = "SYSTEM_ATTENTION_SPEC"
const JOB = "SystemAttentionSpec"
const RUN = "SYSTEM_ATTENTION_SPEC"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function make(entityName, fields) {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
function remove(entityName, field, value) {
    for (const row of ec.entity.find(entityName).condition(field, value).useCache(false).list()) row.delete()
}
// The service's items as `username`, and the errors the call left.
function attention(username) {
    ec.user.internalLoginUser(username)
    const out = ec.service.sync().name(SERVICE).call() || {}
    const errors = ec.message.getErrorsString()
    ec.message.clearErrors()
    return {items: Array.from(out.items || []), checked: out.checkedDate != null, errors}
}
function byKey(items, key) {
    return items.filter((item) => item.key === key)
}

spec({
    setupSpec() {
        sys(() => {
            if (ec.entity.find("moqui.security.UserAccount").condition("userId", USER).count() === 0) {
                const fromDate = new Date(Date.now() - 60000)
                make("moqui.security.UserAccount", {userId: USER, username: USERNAME, userFullName: "System Attention Spec", disabled: "N"})
                make("moqui.security.UserGroup", {userGroupId: GROUP, description: "The attention service only (SystemAttention spec)"})
                make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId: USER, fromDate})
                // the service alone, not inherited: what it reads is not this group's
                make("moqui.security.ArtifactGroup", {artifactGroupId: GROUP, description: "SystemAttention spec"})
                make("moqui.security.ArtifactGroupMember", {artifactGroupId: GROUP, artifactName: SERVICE, artifactTypeEnumId: "AT_SERVICE",
                    inheritAuthz: "N"})
                make("moqui.security.ArtifactAuthz", {artifactAuthzId: GROUP, artifactGroupId: GROUP, userGroupId: GROUP,
                    authzTypeEnumId: "AUTHZT_ALLOW", authzActionEnumId: "AUTHZA_VIEW"})
            }
            if (ec.entity.find("moqui.service.job.ServiceJob").condition("jobName", JOB).count() === 0) {
                make("moqui.service.job.ServiceJob", {jobName: JOB, description: "SystemAttention spec", paused: "Y",
                    serviceName: SERVICE})
            }
        })
    },
    cleanupSpec() {
        sys(() => {
            remove("moqui.service.job.ServiceJobRun", "jobName", JOB)
            remove("moqui.service.job.ServiceJob", "jobName", JOB)
        })
        ec.user.internalLoginUser("john.doe")
    }
})

feature("a failed job run is a check that is not ready, and left out for a user who may not read the checks", {
    when() {
        const before = attention("john.doe")
        sys(() => make("moqui.service.job.ServiceJobRun", {jobRunId: RUN, jobName: JOB, hasError: "Y", errors: "spec",
            startTime: new Date(Date.now() - 60000), endTime: new Date()}))
        const admin = attention("john.doe")
        const limited = attention(USERNAME)
        const countOf = (result) => byKey(result.items, "admin.jobRuns").length
        const item = byKey(admin.items, "admin.jobRuns")[0] || {}
        // what the limited user may not read: every item counted from an entity, a dataset or another service, and
        // every readiness check (each of the framework's needs a permission)
        const guarded = ["systemMessages", "incidents", "alerts", "checks", "scrapeTargets", "errorGroups", "approvals", "readiness"]
        return {before: countOf(before), after: countOf(admin), item, admin, limited,
            textNamesRuns: /job runs? failed in the last day: /.test(item.text || ""),
            limitedGuarded: limited.items.filter((entry) => guarded.includes(entry.key) || /^(admin|constitution)\./.test(entry.key))
                .map((entry) => entry.key).join(",")}
    },
    then({before, after, item, textNamesRuns, admin, limited, limitedGuarded}) {
        admin.errors == ""
        admin.checked
        // the check is one item however many runs failed (another spec's failed run may be there already)
        before <= 1
        after == 1
        item.severity == "critical"
        textNamesRuns
        // its fix place is the list of the failed runs, from the day they are counted from
        item.link == "/apps/system/Operations/JobRuns"
        item.parameters.state == "failed"
        ;/^\d{4}-\d{2}-\d{2}$/.test(item.parameters.fromDate)
        // the same call for a user who may call the service and read nothing else: it answers, without the item
        limited.errors == ""
        limited.checked
        limitedGuarded == ""
    }
})

feature("a resource at warning is listed with its number, resources first, then the checks that are not ready, and one item's failure leaves the others", {
    when() {
        const source = ec.resource.getLocationText("component://tools/service/moqui/tools/SystemHomeServices.xml")
        const script = /<actions><script><!\[CDATA\[([\s\S]*?)\]\]><\/script><\/actions>/.exec(source)[1]
        const GIB = 1024 * 1024 * 1024
        const status = {
            Draining: false,
            DataSources: [{group: "transactional", embedded: false, waiting: 0, checkedOut: 1, max: 50}],
            Resources: {
                Cpu: {Load5: 1, Processors: 16, LoadPercent: 6.25, Level: "ok"},
                Memory: {ProcessBytes: 3.28 * GIB, LimitBytes: 4 * GIB, ProcessPercent: 82, Level: "warning", AvailablePercent: 40, AvailableLevel: "ok"},
                Swap: {TotalBytes: 0, UsedPercent: null, Level: null},
                OpenFiles: {Count: 412, Limit: 65536, UsedPercent: 0.63, Level: "ok"},
                Filesystems: [
                    {mountPoint: "/", holds: ["system temp"], usedPercent: 38, availableBytes: 62 * GIB, level: "ok", inodesLevel: "ok"},
                    {mountPoint: "/var", holds: ["runtime", "database"], usedPercent: 91.2, availableBytes: 4 * GIB, level: "critical",
                        inodesUsedPercent: 12, inodesLevel: "ok"}
                ],
                // the certificates of the HTTPS listener: one inside the warning days, one far from them
                Certificates: [{name: "shop.example.com", daysLeft: 12.4, level: "warning", notAfter: Date.now() + 12.4 * 86400000},
                    {name: "example.com", daysLeft: 200, level: "ok", notAfter: Date.now() + 200 * 86400000}]
            }
        }
        // a name set up with ACME whose renewal failed, ten days before its certificate runs out (System › Certificates)
        const acmeExpiry = new Date(Date.now() + 10.5 * 86400000)
        const rows = {"moqui.server.AcmeName": [{hostName: "www.example.com", statusId: "AcmeFailed", notAfter: acmeExpiry}]}
        // what the readiness service answers: one check of each state, two with a System place and one an app's
        const readiness = {checks: [
            {checkKey: "spec.ready", state: "ready", message: "Fine"},
            {checkKey: "spec.warning", state: "warning", subject: "Instance", message: "Backups are not mirrored off the server",
                fixRoute: "adminConnectionDetail", fixParameters: {adapterKey: "remote"}},
            {checkKey: "spec.failed", state: "failed", message: "3 job runs failed in the last day: Spec", fixRoute: "adminJobRuns",
                fixParameters: {state: "failed"}},
            {checkKey: "spec.owner", state: "unavailable", subject: "Spec organization", message: "Accounting setup could not be checked",
                fixRoute: "accountingSetup"}
        ]}
        // a listener's row as get#Listeners answers it: on at an address, or off
        // a way in as get#Listeners answers it: its state and address, whether clients reach it (the guard's rule), and
        // what changed it last
        const listener = (name, address, state, more) =>
            Object.assign({name, state: state || (address ? "on" : "off"), address, wayIn: true, reachable: false, cause: "boot"}, more || {})
        // HTTP on in public with no proxy; HTTPS failed after a person moved it; SSH failed at the start
        const startedListeners = [{name: "HTTP", state: "on", detail: "tcp 0.0.0.0:8080", cause: "boot"},
            {name: "SSH", state: "failed", detail: "the port is in use", cause: "boot"},
            {name: "HTTPS", state: "failed", detail: "tcp 0.0.0.0:9443: address in use", cause: "request by john.doe: move it"}]
        const listening = {running: true, listenerList: [listener("HTTP", "0.0.0.0:8080", null, {reachable: true, plainPublic: true}),
            listener("HTTPS", null, "failed", {cause: "request by john.doe: move it"}), listener("SSH", null, "failed")]}
        const logged = []
        const finder = (entityName) => {
            // one item's source fails
            if (entityName === "moqui.service.message.SystemMessage") throw new Error("the spec's failure")
            const find = {condition: () => find, limit: () => find, disableAuthz: () => find, list: () => rows[entityName] || []}
            return find
        }
        // the service's script over a startup report's listeners and get#Listeners' answer
        const run = (reported, listeners) => {
            const answer = (name) => name === "org.moqui.admin.ReadinessServices.get#Readiness" ? readiness
                : name === "org.moqui.admin.ListenerServices.get#Listeners" ? listeners : {}
            const state = {
                ec: {
                    factory: {
                        getStatusMap: () => status,
                        getStartupReport: () => ({listeners: reported, integrations: [{name: "PDF", state: "off", detail: "no Chromium found"}]}),
                        workerPool: {activeCount: 3, maximumPoolSize: 22, queue: {size: () => 0, remainingCapacity: () => 100}}
                    },
                    entity: {find: finder},
                    artifactExecution: {isPermitted: () => true},
                    user: {hasPermission: () => true, nowTimestamp: new Date()},
                    service: {sync: () => ({name: (name) => ({parameters: () => ({call: () => answer(name)}), call: () => answer(name)})})},
                    resource: ec.resource,
                    message: {hasError: () => false},
                    logger: {error: (message) => logged.push(String(message))},
                    l10n: ec.l10n
                },
                items: null, checkedDate: null
            }
            runInContext(script, state)
            return state
        }
        const state = run(startedListeners, listening)
        const items = Array.from(state.items || [])
        const firstLogged = logged.join(" | ")
        // the listener items alone, for the ways in: HTTPS turned off with only SSH left, on loopback; HTTPS on loopback
        // behind a proxy on this machine; a server that started with no way in on purpose (the desktop node: HTTP on
        // loopback, HTTPS and SSH off); and no listeners in this process
        const listenerKeys = (result) => Array.from(result.items || []).filter((item) => ["startup", "listenerFailed", "publicHttp", "noWayIn"].includes(item.key))
        const up = [{name: "HTTPS", state: "off", detail: "", cause: "request by john.doe: incident"}]
        const incident = {cause: "request by john.doe: incident"}
        const sshOnly = listenerKeys(run(up, {running: true, listenerList: [listener("HTTP", null), listener("HTTPS", null, null, incident),
            listener("SSH", "127.0.0.1:2222")]}))
        const proxied = listenerKeys(run(up, {running: true, listenerList: [listener("HTTP", null),
            listener("HTTPS", "127.0.0.1:8443", null, Object.assign({reachable: true}, incident)), listener("SSH", null)]}))
        const desktop = listenerKeys(run(up, {running: true, listenerList: [listener("HTTP", "127.0.0.1:8080"), listener("HTTPS", null),
            listener("SSH", null)]}))
        const notRunning = listenerKeys(run(up, {running: false, listenerList: [listener("HTTP", null), listener("HTTPS", null, null, incident),
            listener("SSH", null)]}))
        return {items, keys: items.map((item) => item.key).join(","), logged: firstLogged, checkedDate: state.checkedDate,
            acmeUntil: ec.l10n.format(acmeExpiry, "yyyy-MM-dd"), sshOnly, proxied, desktop, notRunning}
    },
    then({items, keys, logged, checkedDate, acmeUntil, sshOnly, proxied, desktop, notRunning}) {
        // the resources first, the critical one before the warning; then the others, the critical one first
        keys == "disk,memory,certificates,acmeRenewal,startup,listenerFailed,spec.failed,publicHttp,spec.owner,spec.warning"
        items[0].severity == "critical"
        items[0].text == "Disk /var is 91 % full, 4.0 GiB free (runtime, database)"
        items[0].value == 91.2
        items[0].link == "/apps/system/Monitor/Metrics"
        items[0].parameters.metricName == "system.filesystem.utilization"
        items[1].severity == "warning"
        items[1].text == "The server uses 82 % of the 4.0 GiB it may use"
        items[1].value == 82
        // a certificate inside its warning days, with the whole days it has left; the one far from them is not listed
        items[2].severity == "warning"
        items[2].text == "The certificate of shop.example.com expires in 12 days"
        items[2].value == 12
        items[2].parameters.metricName == "moqui.tls.certificate.days_left"
        // a failed ACME renewal, with the day its certificate expires, linked to Certificates
        items[3].severity == "warning"
        items[3].value == 10
        items[3].text == "Renewing the certificate of www.example.com by ACME failed; it expires " + acmeUntil
        items[3].link == "/apps/system/Certificates"
        // a listener that failed at the start, linked to where listeners are switched; one that failed after a change
        // apart from it, naming the change
        items[4].severity == "critical"
        items[4].text == "Listener SSH failed to start: the port is in use"
        items[4].link == "/apps/system/Conf/ServicesPorts"
        items[5].severity == "critical"
        items[5].text == "Listener HTTPS failed after a change (request by john.doe: move it): tcp 0.0.0.0:9443: address in use"
        items[5].link == "/apps/system/Conf/ServicesPorts"
        // the checks that are not ready: the failed one critical, the others warnings, the unavailable before the
        // warning; a fix place that is a System screen is linked, an app's own place is not
        items[6].severity == "critical"
        items[6].text == "3 job runs failed in the last day: Spec"
        items[6].link == "/apps/system/Operations/JobRuns"
        items[6].parameters.state == "failed"
        // plain HTTP on every address with no proxy believed in front
        items[7].severity == "warning"
        items[7].text.startsWith("HTTP serves plain HTTP at 0.0.0.0:8080 with no proxy believed in front")
        items[7].link == "/apps/system/Conf/ServicesPorts"
        items[8].severity == "warning"
        items[8].text == "Accounting setup could not be checked (Spec organization)"
        items[8].link == null
        items[9].text == "Backups are not mirrored off the server"
        items[9].link == "/apps/system/Integrations/ConnectionDetail"
        items[9].parameters.adapterKey == "remote"
        // no way in over the network after HTTPS was turned off: SSH on loopback is none
        sshOnly.length == 1
        sshOnly[0].key == "noWayIn"
        sshOnly[0].severity == "critical"
        sshOnly[0].text.includes("moqui operator listener HTTPS on")
        sshOnly[0].link == "/apps/system/Conf/ServicesPorts"
        // HTTPS on loopback that the guard counts (behind a proxy on this machine) is a way in; a server that started with
        // none on purpose is not listed; with no listeners in this process there is nothing to say
        proxied.length == 0
        desktop.length == 0
        notRunning.length == 0
        // the item that failed is named in the log and the rest came back
        logged == "System attention item systemMessages failed: Error: the spec's failure"
        checkedDate != null
    }
})
