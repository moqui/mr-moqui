// Regressions for administration displays: raw timestamps, missing metrics, service source and results after a redirect.
// These screens have no mode-specific templates, so each is rendered once in HTML with all of its checks together.
function render(path, root = "System", parameters = {}) {
    const result = ec.screen.makeTest().rootScreen("component://tools/screen/" + root + ".xml").renderMode("html")
        .render(path, parameters, null)
    return {status: result.status, errors: result.errorMessages.join(" | "), output: String(result.output || "")}
}

// fixture rows, written and read past entity authorization; the screens' own permission checks still apply
function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try { return run() } finally { if (!wasDisabled) ec.artifactExecution.enableAuthz() }
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
    }
})

feature("recovery editors load configuration, keep rejected blanks and separate new IDs from updates", {
    expect() {
        const source = ec.resource.getLocationText("component://tools/screen/System/Monitor/Recovery.xml")
        const actions = /^    <actions><script><!\[CDATA\[([\s\S]*?)\]\]><\/script><\/actions>/m.exec(source)[1]
        const policy = {policyId: "UI_RECOVERY_EDIT", description: "Loaded recovery policy", enabled: "N", sequenceNum: 1000000,
            serviceName: "example.*", minSeverity: "critical", mode: "advisory", ladderJson: '[{"action":"escalate"}]',
            adapterId: "UI_RECOVERY_ADAPTER", contactJson: '{"type":"notification"}', minConfidence: 0.999999,
            cooldownMinutes: 0, maxActionsPerDay: 10, deadlineMinutes: 20, verifyMinutes: 30, maxRollbackRate: 0.01234567}
        const adapter = {adapterId: "UI_RECOVERY_ADAPTER", description: "Loaded manual adapter", adapterType: "manual",
            url: "https://example.invalid/recovery", urlProperty: "review_recovery_url", statusUrl: "https://example.invalid/status",
            secretProperty: "review_recovery_signing_key", headerPropertiesJson: '{"Authorization":"review_recovery_auth"}',
            allowInternal: "N", instructions: "Review the incident before doing anything."}
        const inspect = (parameters = {}, errors = {}, allowed = true) => {
            let reads = 0
            const state = {
                ec: {
                    user: {hasPermission: () => allowed}, l10n: ec.l10n, web: {errorParameters: errors},
                    service: {sync: () => ({name: () => ({call: () => {
                        reads++
                        return {policies: [policy], adapters: [adapter], actions: [], stopped: false}
                    }})})}
                },
                editPolicyId: policy.policyId, editAdapterId: adapter.adapterId, incidentAllowed: null, adminAllowed: null,
                policyList: null, adapterList: null, actionList: null, recoveryStopped: null, selectedPolicy: null,
                selectedAdapter: null, policyMissing: null, adapterMissing: null, policyForm: null, adapterForm: null,
                policyTitle: null, adapterTitle: null, ...parameters
            }
            runInContext(actions, state)
            return {...state, reads}
        }
        const selected = inspect()
        for (const [name, value] of Object.entries(policy)) assert.equal(selected.policyForm[name], String(value), name)
        for (const [name, value] of Object.entries(adapter)) assert.equal(selected.adapterForm[name], String(value), name)
        assert.equal(selected.policyList[0].stepsText, "Escalate")
        assert.equal(selected.policyForm.sequenceNum, "1000000")
        assert.equal(selected.policyForm.cooldownMinutes, "0")
        const rejectedPolicy = inspect({}, {moquiFormName: "StorePolicy", description: "", contactJson: "", minConfidence: "not a number",
            sequenceNum: "1000001", ladderJson: "not JSON"})
        assert.equal(rejectedPolicy.policyForm.description, "")
        assert.equal(rejectedPolicy.policyForm.contactJson, "")
        assert.equal(rejectedPolicy.policyForm.minConfidence, "not a number")
        assert.equal(rejectedPolicy.policyForm.sequenceNum, "1000001")
        assert.equal(rejectedPolicy.policyForm.ladderJson, "not JSON")
        assert.equal(rejectedPolicy.adapterForm.description, adapter.description)
        const rejectedAdapter = inspect({}, {moquiFormName: "StoreAdapter", instructions: "", secretProperty: "", headerPropertiesJson: "{}"})
        assert.equal(rejectedAdapter.adapterForm.instructions, "")
        assert.equal(rejectedAdapter.adapterForm.secretProperty, "")
        assert.equal(rejectedAdapter.adapterForm.headerPropertiesJson, "{}")
        assert.equal(rejectedAdapter.policyForm.description, policy.description)
        const fresh = inspect({editPolicyId: "", editAdapterId: ""})
        assert.equal(fresh.policyForm.policyId, "")
        assert.equal(fresh.policyForm.mode, "advisory")
        assert.equal(fresh.policyForm.ladderJson, "[]")
        assert.equal(fresh.adapterForm.adapterType, "manual")
        assert.equal(fresh.adapterForm.secretProperty, "")
        assert.equal(fresh.adapterForm.headerPropertiesJson, "")
        const missing = inspect({editPolicyId: "missing", editAdapterId: "missing"})
        assert(missing.policyMissing && missing.adapterMissing)
        assert.equal(missing.selectedPolicy, null)
        assert.equal(missing.selectedAdapter, null)
        const denied = inspect({}, {}, false)
        assert.equal(denied.reads, 0)
        assert.equal(denied.selectedPolicy, null)
        assert.equal(denied.selectedAdapter, null)
        assert.equal(denied.adapterForm.secretProperty, "")
        // The one Recovery HTML render verifies the actual bound controls, not a second mode/screen sweep.
        sys(() => {
            for (const [entityName, fields] of [["moqui.monitor.RecoveryAdapter", adapter], ["moqui.monitor.RecoveryPolicy", policy]]) {
                const value = ec.entity.makeValue(entityName)
                value.setFields(fields, true, null, null)
                value.create()
            }
        })
        const page = render("Monitor/Recovery", "System", {editPolicyId: policy.policyId, editAdapterId: adapter.adapterId})
        assert.equal(page.status, 200)
        assert.equal(page.errors, "")
        assert(!/Error rendering|Exception in render/.test(page.output))
        const input = id => new RegExp('<input\\b[^>]*\\bid="' + id + '"[^>]*>').exec(page.output)?.[0] || ""
        assert(input("StorePolicy_sequenceNum").includes('value="1000000"'))
        assert(input("StorePolicy_minConfidence").includes('value="0.999999"'))
        assert(input("StorePolicy_maxRollbackRate").includes('value="0.01234567"'))
        assert(input("StorePolicy_cooldownMinutes").includes('value="0"'))
        assert(input("StorePolicy_policyId").includes('type="hidden"'))
        assert(input("StoreAdapter_adapterId").includes('type="hidden"'))
        assert(input("StoreAdapter_secretProperty").includes('value="review_recovery_signing_key"'))
        assert(page.output.includes("editPolicyId=UI_RECOVERY_EDIT"))
        assert(page.output.includes("editAdapterId=UI_RECOVERY_ADAPTER"))
        assert(page.output.includes("Manual: operator instructions"))
        assert(page.output.includes("Webhook: connection and signing"))
        assert(page.output.includes("Review the incident before doing anything."))
        // Exercise the actual transition boundary: a message + check-errors must skip the store, redirect,
        // and save the rejected inputs. An isolated script throw would bypass this entire recovery path.
        const test = ec.screen.makeTest().rootScreen("component://tools/screen/System.xml").renderMode("html")
        // The adapter transition has the same shape; one duplicate POST covers the boundary.
        for (const [kind, id, entityName] of [["Policy", policy.policyId, "moqui.monitor.RecoveryPolicy"]]) {
            const idName = kind === "Policy" ? "policyId" : "adapterId"
            const submitted = {moquiFormName: "Store" + kind, [idName]: id, ["recovery" + kind + "Mode"]: "create",
                description: "", enabled: "N", ladderJson: "[]", adapterType: "manual", instructions: "Rejected instructions"}
            const rejected = test.render("Monitor/Recovery/store" + kind, submitted, "post")
            assert.equal(rejected.status, 302, kind + " duplicate uses its error response")
            assert(rejected.errorMessages.join(" | ").includes("ID already exists"))
            assert(String(rejected.header("Location")).includes("_openDialog=" + kind + "Dialog"))
            const kept = ec.web.errorParameters
            assert.equal(kept.moquiFormName, "Store" + kind)
            assert.equal(kept.description, "", kind + " intentionally cleared description survives the error redirect")
            assert.equal(kept[idName], id)
            const unchanged = sys(() => ec.entity.find(entityName).condition(idName, id).one())
            assert.equal(unchanged.description, kind === "Policy" ? policy.description : adapter.description)
            if (kind === "Adapter") assert.equal(unchanged.instructions, adapter.instructions)
        }
    },
    cleanup() {
        sys(() => {
            ec.entity.find("moqui.monitor.RecoveryPolicy").condition("policyId", "UI_RECOVERY_EDIT").deleteAll()
            ec.entity.find("moqui.monitor.RecoveryAdapter").condition("adapterId", "UI_RECOVERY_ADAPTER").deleteAll()
        })
    }
})

// The System home (plans/archive/Tools-Fancy.md §3), its one render: plain XML widgets, so html alone. `moqui test` serves no
// requests, so the runtime report has no listeners and the page says so; a server's page has a row for each.
feature("the System home shows the instance, its listeners or their absence, and what needs attention", {
    when() {
        const status = ec.factory.getStatusMap(true)
        const report = ec.factory.getStartupReport()
        const page = render("Home")
        return {
            page,
            version: report.version,
            listeners: Array.from(report.listeners || []),
            started: ec.l10n.format(status.Runtime.Start, "MMM d, HH:mm"),
            processCpu: status.Resources.Cpu.ProcessPercent
        }
    },
    then({page, version, listeners, started, processCpu}) {
        page.status == 200
        page.errors == ""
        !/Error rendering|Exception in render/.test(page.output)
        // present whatever the data
        page.output.includes("MR Moqui " + version)
        page.output.includes("since " + started)
        page.output.includes("Listeners and integrations")
        page.output.includes('id="SystemCounters"')
        listeners.length > 0
            ? page.output.includes('id="ListenerList_table"') && page.output.includes(">" + listeners[0].name + "<")
            : page.output.includes("This process reported no listeners")
        // the empty attention state or its rows, each with its level as a word
        page.output.includes("Nothing needs attention") ||
            (page.output.includes('id="AttentionList_table"') &&
                /<span data-level>(Critical|Warning|Note)<\/span>/.test(page.output))
        // a reading that is not reported is a dash with its reason, never zero or null
        processCpu != null || page.output.includes('title="Not reported by this platform"')
        !/>null<|: null|>undefined</.test(page.output.slice(page.output.indexOf('id="SystemIdentity"')))
        // the navigation comes from the catalog and the menu; what left the page is gone
        page.output.includes('id="NavFeaturedCards"')
        page.output.includes(">Users and access</a>")
        page.output.includes('id="NavGroupBoxes"')
        !page.output.includes("Loaded components")
        !page.output.includes("Database connections</h5>")
        // auto refresh is offered, off by default (it is in the address); a reading that is a share has its meter
        page.output.includes(">Auto refresh</a>")
        !page.output.includes('http-equiv="refresh"')
        processCpu == null || page.output.includes('<progress max="100"')
    }
})

// The Tools home (plans/archive/Tools-Fancy.md §4), its one render: plain XML widgets, so html alone. john.doe holds the four
// Tools permissions; the line names each one whether or not it is held. The lists of what the user left off and of what
// is running are hidden when empty, so they are not checked here.
feature("the Tools home shows the instance, what the user may do, the jump row, the counters and the main tools", {
    when() {
        return {page: render("Home", "Tools"), purpose: ec.factory.getStartupReport().purpose, entities: ec.entity.getAllEntityNames().length}
    },
    then({page, purpose, entities}) {
        page.status == 200
        page.errors == ""
        !/Error rendering|Exception in render/.test(page.output)
        // present whatever the data: the instance line, the permission line with each permission named
        page.output.includes('id="ToolsInstance"')
        !purpose || page.output.includes(">" + purpose)
        page.output.includes("You can:")
        ["SQL_RUNNER_WEB", "DATABASE_ADMIN", "DATABASE_EXTERNAL", "LOAD_RUNNER"].every(permission =>
            page.output.includes("You hold the permission " + permission) || page.output.includes("Needs the permission " + permission)
        )
        // the jump row and the counters
        page.output.includes("Open an entity")
        page.output.includes("Open a service")
        page.output.includes('id="ToolsCounters"')
        entities > 0
        !/>null<|: null|>undefined<|>NaN</.test(page.output.slice(page.output.indexOf('id="ToolsInstance"')))
        // the main tools and the rest come from the catalog and the menu
        page.output.includes('id="NavFeaturedLargeCards"')
        page.output.includes(">Data grid</a>")
        page.output.includes('id="NavGroupBoxes"')
        page.output.includes(" REST API</a>")
        page.output.includes(" Terminal UI</a>")
        // what left the page is gone
        !page.output.includes("API reference")
        !page.output.includes("External databases</a>")
    }
})

feature("cache statistics explain missing process metrics instead of showing blank quantities", {
    when() {
        const process = ec.factory.getStatusMap().Process
        return {page: render("Cache/CacheList"), unavailable: process.MemoryUsed == null && process.Threads == null}
    },
    then({page, unavailable}) {
        page.status == 200
        page.errors == ""
        !/Error rendering|Exception in render/.test(page.output)
        page.output.includes("Clear All")
        page.output.includes("Cache name filter")
        page.output.includes("Unsupported patterns are treated as literal text.")
        !unavailable || page.output.includes("Process memory and thread metrics are not reported by this platform.")
        !page.output.includes("Threads: null")
        !page.output.includes("Memory Used:  MiB")
    }
})

feature("cache entry details distinguish native counts from an empty cache", {
    expect() {
        // The existing cache HTML coverage renders its rows; exercise this availability rule without another render.
        const source = ec.resource.getLocationText("component://tools/screen/System/Cache/CacheElements.xml")
        const actions = /<set field="cacheElementList"[^>]*\/>\s*<script><!\[CDATA\[([\s\S]*?)\]\]><\/script>/.exec(source)[1]
        ec.entity.find("moqui.basic.Enumeration").condition("enumId", "GEOT_STATE").useCache(true).disableAuthz().list()
        const nativeName = "entity.record.list.moqui.basic.Enumeration"
        const native = {
            ec, cacheName: nativeName, cacheElementList: ec.cache.makeElementInfoList(nativeName),
            cacheReportedSize: null, cacheEntryCount: null, cacheReportedSizeText: null, cacheEmptyText: null
        }
        runInContext(actions, native)
        assert(native.cacheReportedSize > 0, "the native entity cache reports the cached query")
        assert.equal(native.cacheEntryCount, 0)
        assert.equal(native.cacheEmptyText, "Entry details are not available in this view. Some framework caches expose counts only.")
        for (const [size, entries, message] of [
            [0, [], "No entries are reported for this cache."],
            [3, [{key: "available"}], "Entry details are not available in this view. Some framework caches expose counts only."],
            [null, [], "Entry details are not available in this view. Some framework caches expose counts only."]
        ]) {
            const state = {
                ec: {l10n: ec.l10n, cache: {getAllCachesInfo: () => [{name: "other", size: 99}, {name: "selected", size}]}},
                cacheName: "selected", cacheElementList: entries, cacheReportedSize: null, cacheEntryCount: null,
                cacheReportedSizeText: null, cacheEmptyText: null
            }
            runInContext(actions, state)
            assert.equal(state.cacheReportedSize, size)
            assert.equal(state.cacheEntryCount, entries.length)
            assert.equal(state.cacheEmptyText, message)
            assert.equal(state.cacheReportedSizeText, size == null ? "Unavailable" : String(size))
        }
    }
})

feature("thread counts and scheduler checks distinguish unavailable from zero and format timestamps", {
    expect() {
        // Test the screens' display expressions with both available and unavailable metrics, independent of the host OS.
        const threads = ec.resource.getLocationText("component://tools/screen/System/ThreadList.xml")
        const jobs = ec.resource.getLocationText("component://tools/screen/System/Operations/Jobs.xml")
        const expression = (source, field) => new RegExp('<set field="' + field + '" from="([^"]+)"').exec(source)[1]
            .replace(/&amp;/g, "&").replace(/&gt;/g, ">").replace(/&lt;/g, "<")
        const stamp = ec.user.nowTimestamp
        for (const source of [threads, jobs]) {
            const from = expression(source, "jobRunnerLastCheckText")
            for (const [jobRunner, expected] of [
                [{executeCount: 1, lastExecuteTime: stamp}, ec.l10n.format(stamp, null)],
                [{executeCount: 1, lastExecuteTime: null}, "Unavailable"],
                [{executeCount: 0, lastExecuteTime: stamp}, "Not run yet"]
            ]) {
                const state = {ec, jobRunner, actual: null}
                runInContext("actual = " + from, state)
                assert.equal(state.actual, expected)
            }
        }
        for (const [Threads, expected] of [[null, "Unavailable"], [0, "0"], [12, "12"]]) {
            const state = {ec, processInfo: {Threads}, actual: null}
            runInContext("actual = " + expression(threads, "threadCountText"), state)
            assert.equal(state.actual, expected)
        }
    }
})

// One html render of Threads, which has logic of its own: the thread that renders it runs this screen, so its row has
// what it runs and the artifact stack, whatever else the process is doing.
feature("the thread list shows what a thread runs and offers nothing this server cannot do", {
    when() {
        return {page: render("ThreadList", "System", {show: "running"})}
    },
    then({page}) {
        page.status == 200
        page.errors == ""
        !/Error rendering|Exception in render/.test(page.output)
        page.output.includes("Service worker pool")
        page.output.includes("Request pool")
        page.output.includes("(xml screen)")
        page.output.includes("ThreadList.xml")
        page.output.includes("StackDialog_")
        !page.output.includes("Interrupt")
        !page.output.includes("Lock Owner")
    }
})

feature("alert creation links discard an incoming notification record identity", {
    expect() {
        // One focused render of Alerts; a notification's definitionId must not turn any New action into Edit.
        const result = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml").renderMode("html")
            .render("apps/dashboards/Alerts", {lastStandalone:"-2",definitionId:"ui080-stale-alert",dashboardId:"ui080-old-dashboard",panelId:"ui080-old-panel"}, null)
        assert.equal(result.status, 200)
        assert.equal(result.errorMessages.join(" | "), "")
        const output = String(result.output || "")
        assert(!/Error rendering|Exception in render/.test(output))
        const links = Array.from(output.matchAll(/<a\b[^>]*\bhref="([^"]*)"[^>]*>([\s\S]*?)<\/a>/g)).map(match => ({
            url:match[1].replace(/&amp;/g, "&"),text:match[2].replace(/<[^>]*>/g, "").replace(/\s+/g, " ").trim()
        }))
        const create = (text) => {
            const link = links.find(item => item.text === text)
            assert(link, "creation link exists: " + text)
            assert(!/[?&]definitionId=/.test(link.url), "New must not inherit a definition: " + link.url)
            return link.url
        }
        const rule = create("New alert rule")
        assert.equal(rule.split("?")[0].endsWith("/AlertRule"), true)
        assert(!/[?&](dashboardId|panelId)=/.test(rule), "a new rule from the list has no inherited panel context")
        for (const [text, kind] of [["New contact point","contactPoint"],["New silence","silence"],["New mute timing","muteTiming"]]) {
            const url = create(text)
            assert.equal(url.split("?")[0].endsWith("/AlertShared"), true)
            assert(url.includes("sharedType=" + kind), "the chosen creation type remains in the URL")
        }
    }
})

feature("service inspection shows the XML Actions source exposed by its definition", {
    when() {
        return {page: render("Service/ServiceDetail", "Tools", {serviceName: "org.moqui.impl.BasicServices.echo#Data"})}
    },
    then({page}) {
        page.status == 200
        page.errors == ""
        !/Error rendering|Exception in render/.test(page.output)
        page.output.includes("XML Actions")
        page.output.includes("&lt;set field=&quot;textOut1&quot; from=&quot;textIn1&quot;")
        !page.output.includes("Generated Groovy")
        page.output.includes("Timeout: Default")
        !page.output.includes("Timeout: null")
    }
})

feature("a service run preserves its output separately from the input across the redirect", {
    when() {
        const serviceName = "org.moqui.impl.BasicServices.echo#Data"
        const test = ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").renderMode("html")
        const run = test.render("Service/ServiceRun/run", {serviceName, textIn1: "UI review echo", textIn2: "", numberIn: "0"}, "post")
        const kept = JSON.parse(ec.web.sessionAttributes.toolsServiceRunResults || "null")
        const page = render("Service/ServiceRun", "Tools", {serviceName, doingRun: "Y"})
        const textOut1 = /<label for="ServiceResults_textOut1">[\s\S]*?<span[^>]*>([\s\S]*?)<\/span>/.exec(page.output)
        const textOut2 = /<label for="ServiceResults_textOut2">[\s\S]*?<span[^>]*>([\s\S]*?)<\/span>/.exec(page.output)
        // shown once: the screen took the output out of the session, so a Back or refresh does not show it again
        const shownAfter = ec.web.sessionAttributes.toolsServiceRunResults
        const failed = test.render("Service/ServiceRun/run", {serviceName, numberIn: "not a number"}, "post")
        return {run, kept, page, textOut1: textOut1 && textOut1[1], textOut2: textOut2 && textOut2[1],
            shownAfter, failedErrors: failed.errorMessages, staleResults: ec.web.sessionAttributes.toolsServiceRunResults}
    },
    then({run, kept, page, textOut1, textOut2, shownAfter, failedErrors, staleResults}) {
        run.errorMessages.length == 0
        kept.serviceName == "org.moqui.impl.BasicServices.echo#Data"
        kept.results.textOut1 == "UI review echo"
        Number(kept.results.numberOut) == 0
        page.status == 200
        page.errors == ""
        textOut1 == "UI review echo"
        textOut2 == "ping"
        shownAfter == null
        failedErrors.length > 0
        staleResults == null
    },
    cleanup() {
        ec.web.sessionAttributes.remove("toolsServiceRunResults")
    }
})

feature("service selection accepts implicit entity services and rejects missing names before building forms", {
    expect() {
        // Exercise the screen action directly, leaving its single HTML render above to verify the actual output form.
        const source = ec.resource.getLocationText("component://tools/screen/Tools/Service/ServiceRun.xml")
        const actions = /^    <actions>\s*<script><!\[CDATA\[([\s\S]*?)\]\]><\/script>/m.exec(source)[1]
        const cases = [
            ["org.moqui.impl.BasicServices.echo#Data", true],
            ["create#moqui.basic.Enumeration", true],
            ["update-moqui.basic.Enumeration", true],
            ["create#ui.review.MissingEntity", false],
            ["ui.review.missing#Service", false],
            ["", false]
        ]
        for (const [serviceName, expected] of cases) {
            const selection = {ec, serviceName, doingRun: null, serviceExists: null, serviceResults: null}
            runInContext(actions, selection)
            assert.equal(selection.serviceExists, expected, serviceName)
            assert.equal(selection.serviceName, serviceName)
        }
    }
})


feature("a job run's trace and log windows surround the run, and an item links by the mount's route for it", {
    expect() {
        // The run's outcome is the service's (framework AdminJobs); the screen's own rules are its diagnostic windows and item links.
        const source = ec.resource.getLocationText("component://tools/screen/System/Operations/JobRunDetail.xml")
        const actions = /<script><!\[CDATA\[([\s\S]*?)\]\]><\/script>/.exec(source)[1]
        const run = overrides => ({jobRunId: "UI_JOB", jobName: "ui.fixture", title: "UI fixture", timeZone: "UTC", state: "succeeded",
            startTime: "2026-11-04T18:28:00.123Z", endTime: "2026-11-04T18:28:00.130Z", traceId: "abc", runThread: 'worker "one"', ...overrides})
        const itemList = [
            {title: "Message 1", state: "failed", recoveryKind: "SystemMessage", recoveryTargetId: "M1"},
            {title: "Job 1", state: "failed", recoveryKind: "ServiceJob", recoveryTargetId: "job.one"},
            {title: "Other", state: "done", recoveryKind: "Unknown", recoveryTargetId: "X"}
        ]
        const inspect = (found, routes) => {
            const state = {ec, found: {run: found, parameterList: [], itemList}, recordFound: !!found, mount: {routes},
                run: null, stateColor: null, traceOpen: null, traceParameters: null, logParameters: null, evidence: null,
                errorLines: null, parameters: null, items: null}
            runInContext(actions, state)
            return state
        }
        const messageRoute = {systemMessage: {url: "//Mount/Messages/Detail", idParameter: "messageId", parameters: {tab: "recovery"}}}
        const complete = inspect(run({}), messageRoute)
        assert.equal(complete.traceParameters.from, "2026-11-04T18:27:00.123Z")
        assert.equal(complete.traceParameters.thru, "2026-11-04T18:29:00.130Z")
        assert.equal(complete.logParameters.from, "2026-11-04T18:28:00.123Z")
        assert.equal(complete.logParameters.thru, "2026-11-04T18:28:01.130Z")
        assert.equal(complete.logParameters.queryString, "thread.name:" + JSON.stringify('worker "one"'))
        // an item recovered on a page of its own: a message by the mount's route with its parameters, a job beside this screen
        assert.equal(complete.items[0].itemUrl, "//Mount/Messages/Detail")
        assert.equal(JSON.stringify(complete.items[0].itemParameters), JSON.stringify({tab: "recovery", messageId: "M1"}))
        assert.equal(complete.items[1].itemUrl, "../JobDetail")
        assert.equal(complete.items[1].itemParameters.jobName, "job.one")
        assert.equal(complete.items[2].itemUrl, "")
        // a mount with no place for a message links none
        assert.equal(inspect(run({}), {}).items[0].itemUrl, "")
        // a run still going ends now; one that never started has the widest window
        const unfinished = inspect(run({endTime: null}), {})
        assert.equal(unfinished.traceParameters.thru, "now")
        assert.equal(unfinished.logParameters.thru, "now")
        const waiting = inspect(run({startTime: null, endTime: null, runThread: null}), {})
        assert.equal(waiting.traceParameters.from, "now-7d")
        assert.equal(waiting.logParameters.from, "now-1d")
        assert.equal(waiting.logParameters.queryString, "")
    }
})


feature("alert evaluation feedback distinguishes requests, skipped rules and reported failures", {
    expect() {
        // A service 'evaluated' count also includes interval/claim skips; no extra Alerts render is needed.
        const source = ec.resource.getLocationText("component://tools/screen/Dashboards/Alerts.xml")
        const transition = /<transition name="evaluate"[\s\S]*?<\/transition>/.exec(source)[0]
        const actions = /<script><!\[CDATA\[([\s\S]*?)\]\]><\/script>/.exec(transition)[1]
        const inspect = (evaluation, hasError = false) => {
            const messages = []
            runInContext(actions, {evaluation, ec: {message: {
                hasError: () => hasError,
                addMessage: (text, type) => messages.push({text, type})
            }}})
            return messages
        }
        const processed = inspect({evaluated: 1, failed: 0, notices: []})
        assert.equal(processed.length, 1)
        assert.equal(processed[0].type, "info")
        assert(processed[0].text.includes("Evaluation requested."))
        assert(processed[0].text.includes("evaluation interval"))
        assert(processed[0].text.includes("already being evaluated"))
        assert(processed[0].text.includes("Evaluated time and state"))
        assert(!/successful|completed|1 rule evaluated/i.test(processed[0].text))
        const paused = inspect({evaluated: 0, failed: 0})
        assert.equal(paused.length, 1)
        assert(paused[0].text.includes("No active alert rule was processed."))
        assert(paused[0].text.includes("paused"))
        const failed = inspect({evaluated: 0, failed: 1, notices: [{message: "Fixture series limit"}]})
        assert.equal(failed.length, 2)
        assert.equal(failed[0].type, "warning")
        assert(failed[0].text.includes("1 failed rule(s)"))
        assert(failed[0].text.includes("server logs"))
        assert.equal(failed[1].text, "Fixture series limit")
        assert.equal(failed[1].type, "warning")
        assert.equal(inspect({evaluated: 1, failed: 0}, true).length, 0)
        assert.equal(inspect(null).length, 0)
    }
})


feature("alert numeric edit values round trip without grouping or display precision loss", {
    expect() {
        // Execute the screens' actual field assignments and the formatter used by text-line, without another screen render.
        const fields = [
            ["AlertRule", "threshold", "threshold", true, 0],
            ["AlertRule", "minimum", "min", true, 0],
            ["AlertRule", "maximum", "max", true, 0],
            ["AlertRule", "recoveryThreshold", "recoveryThreshold", true, ""],
            ["AlertRule", "pendingSeconds", "pendingSeconds", false, 0],
            ["AlertRule", "keepFiringSeconds", "keepFiringSeconds", false, 0],
            ["AlertRule", "intervalSeconds", "intervalSeconds", false, 60],
            ["AlertShared", "groupWaitSeconds", "groupWaitSeconds", false, 30],
            ["AlertShared", "repeatSeconds", "repeatSeconds", false, 14400]
        ]
        const actions = {}
        for (const screen of ["AlertRule", "AlertShared"]) {
            const source = ec.resource.getLocationText("component://tools/screen/Dashboards/" + screen + ".xml")
            actions[screen] = /^    <actions><script><!\[CDATA\[([\s\S]*?)\]\]><\/script>/m.exec(source)[1]
        }
        for (const [screen, field, key, condition, fallback] of fields) {
            const expression = new RegExp("(?:^|[;\\n])\\s*" + field + "=([^;\\n]+)").exec(actions[screen])[1]
            for (const value of [1000000000, -12.12345678901234, 0.000000000000123, 0.1, 0, null]) {
                const body = condition ? {condition: {[key]: value}} : {[key]: value}
                const state = {body, actual: null}
                runInContext("actual = " + expression, state)
                const expected = value == null || (field === "intervalSeconds" && value === 0) ? fallback : value
                assert.equal(typeof state.actual, "string", screen + "." + field)
                assert.equal(state.actual, String(expected))
                // String values bypass locale grouping/decimal conversion in every text-line renderer.
                const shown = ec.l10n.format(state.actual, null)
                assert.equal(shown, String(expected))
                if (shown !== "") assert.equal(Number(shown), expected, "canonical numeric precision is retained")
            }
        }
    }
})

feature("incident display dates preserve raw values and missing records retain the permission boundary", {
    expect() {
        // The existing Monitoring acceptance owns the screens' renders. Exercise their actual data mapping only.
        const scripts = Object.fromEntries(["Incidents", "IncidentDetail"].map(name => {
            const source = ec.resource.getLocationText("component://tools/screen/System/Monitor/" + name + ".xml")
            return [name, /<script><!\[CDATA\[([\s\S]*?)\]\]><\/script>/.exec(source)[1]]
        }))
        const instant = new Date("2026-11-04T18:28:00.123Z"), epoch = instant.getTime()
        const formatted = ec.l10n.format(instant, "yyyy-MM-dd HH:mm:ss")
        const incident = {incidentId:"000123", onsetDate:"2026-11-04T18:28:00.123Z", detectedDate:String(epoch), statusId:"Open"}
        const response = {
            incident,
            evidence:[{evidenceSeqId:"000001", evidenceDate:epoch, kind:"note", summary:"Fixture note"}],
            recovery:{decisions:[{decisionId:"000002", decidedDate:0, statusId:"AwaitingApproval"}],
                actions:[{actionId:"000003", createdDate:null, statusId:"AwaitingOperator", adapterId:"UI_MANUAL",
                    requestJson:JSON.stringify({instructions:"Review fixture only.\nDo not restart."})},
                    {actionId:"000004", createdDate:"invalid", statusId:"Failed", requestJson:'{"instructions":"do not display a failed request"}'},
                    {actionId:"000005", statusId:"AwaitingOperator", requestJson:'{"instructions":""}'},
                    {actionId:"000006", statusId:"AwaitingOperator", requestJson:'malformed secret payload'},
                    {actionId:"000007", statusId:"AwaitingOperator", requestJson:'{"instructions":"not a manual shape","callbackToken":"secret"}'}]}
        }
        const inspect = (name, allowed = true, exists = true, failure = null, signedIn = true) => {
            let reads = 0, calls = 0
            const logged = []
            const services = {
                "org.moqui.monitor.MonitorServices.list#Incidents": {incidents:[incident]},
                "org.moqui.monitor.MonitorServices.get#Incident": response
            }
            const fake = {logger:{error:message => logged.push(message)}, l10n:ec.l10n, user:{userId:signedIn ? "EX_JOHN_DOE" : null, timeZone:ec.user.timeZone, hasPermission:permission => allowed && permission === "MONITOR_INCIDENT"},
                entity:{find(entityName) {
                    assert.equal(entityName,"moqui.monitor.MonitorIncident");reads++
                    return {condition(field,value) {assert.equal(field,"incidentId");assert.equal(value,"000123");return this},
                        useCache() {return this}, disableAuthz() {return this}, one() {return exists ? {incidentId:"000123"} : null}}
                }},
                service:{sync() {return {name(value) {this.serviceName=value;return this}, parameters() {return this},
                    call() {calls++;if (failure) throw new Error(failure);return services[this.serviceName]}}}}
            }
            const state = {ec:fake, incidentId:"000123", statusId:"all", serviceName:"ui.fixture", incidentAllowed:null,
                incidentFound:null, incidentList:null, statusOptions:null, incident:null, causeList:null, evidenceList:null,
                regressionList:null, decisionList:null, actionList:null, manualActionList:null, recoveryStopped:null, recoverAllowed:null}
            runInContext(scripts[name], state)
            return {state,reads,calls,logged}
        }
        const list = inspect("Incidents").state
        assert.equal(list.incidentList[0].detectedText,formatted)
        assert.equal(list.incidentList[0].detectedDate,String(epoch))
        assert.equal(list.incidentList[0].incidentId,"000123")
        const inspected = inspect("IncidentDetail"), detail = inspected.state
        assert.equal(detail.incident.onsetText,formatted)
        assert.equal(detail.incident.detectedText,formatted)
        assert.equal(detail.incident.detectedDate,String(epoch))
        assert.equal(detail.incident.incidentId,"000123")
        assert.equal(detail.evidenceList[0].evidenceText,formatted)
        assert.equal(detail.evidenceList[0].evidenceDate,epoch)
        assert.equal(detail.decisionList[0].decidedText,ec.l10n.format(new Date(0),"yyyy-MM-dd HH:mm:ss"))
        assert.equal(detail.decisionList[0].decisionId,"000002")
        assert.equal(detail.decisionList[0].waiting,true)
        assert.equal(detail.actionList[0].createdText,ec.l10n.localize("Not recorded"))
        assert.equal(detail.actionList[0].actionId,"000003")
        assert.equal(detail.actionList[0].active,true)
        assert.equal(detail.actionList[1].createdText,ec.l10n.localize("Unavailable"))
        assert.equal(detail.actionList[0].manualInstructions,"Review fixture only.\nDo not restart.")
        assert.equal(detail.actionList[0].adapterId,"UI_MANUAL")
        assert.equal(detail.actionList[1].manualInstructions,"")
        assert.equal(detail.actionList[2].manualInstructions,ec.l10n.localize("No operator instructions were recorded for this manual action."))
        assert.equal(detail.actionList[3].manualInstructions,ec.l10n.localize("Operator instructions are unavailable. Contact the recovery administrator before marking this action done."))
        assert.equal(detail.actionList[4].manualInstructions,detail.actionList[3].manualInstructions)
        assert.deepEqual(detail.manualActionList.map(row => row.actionId),["000003","000005","000006","000007"])
        assert.equal(inspected.logged.length,2)
        assert.ok(inspected.logged[0].includes("000006") && inspected.logged[1].includes("000007"))
        assert.ok(!inspected.logged.join(" ").includes("secret"),"diagnostics never include request payloads")
        const missing = inspect("IncidentDetail",true,false)
        assert.equal(missing.state.incidentFound,false);assert.equal(missing.reads,1);assert.equal(missing.calls,0)
        const denied = inspect("IncidentDetail",false,true)
        assert.equal(denied.state.incidentFound,false);assert.equal(denied.reads,0);assert.equal(denied.calls,0)
        const anonymous = inspect("IncidentDetail",true,true,null,false)
        assert.equal(anonymous.state.incidentFound,false);assert.equal(anonymous.reads,0);assert.equal(anonymous.calls,0)
        let actualFailure = ""
        try { inspect("IncidentDetail",true,true,"fixture service unavailable") } catch (error) { actualFailure=String(error) }
        assert.ok(actualFailure.includes("fixture service unavailable"),"real service failures remain errors, not missing records")
    }
})
