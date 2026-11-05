// Regressions for administration displays: raw timestamps, missing metrics, service source and results after a redirect.
// These screens have no mode-specific templates, so each is rendered once in HTML with all of its checks together.
function render(path, root = "System", parameters = {}) {
    const result = ec.screen.makeTest().rootScreen("component://tools/screen/" + root + ".xml").renderMode("html")
        .render(path, parameters, null)
    return {status: result.status, errors: result.errorMessages.join(" | "), output: String(result.output || "")}
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
    }
})

feature("the instance overview formats its start time and names unavailable metrics", {
    when() {
        const status = ec.factory.getStatusMap(true)
        const page = render("dashboard")
        return {
            page,
            started: ec.l10n.format(status.Runtime.Start, null),
            memory: status.Process.MemoryUsed,
            threads: status.Process.Threads,
            loadAvailable: status.System.OsName === "linux"
        }
    },
    then({page, started, memory, threads, loadAvailable}) {
        page.status == 200
        page.errors == ""
        !/Error rendering|Exception in render/.test(page.output)
        page.output.includes("Instance overview")
        page.output.includes("Started: " + started)
        memory != null || page.output.includes("Memory used: Unavailable")
        threads != null || page.output.includes("Threads: Unavailable")
        loadAvailable || page.output.includes("Load: Unavailable")
        !page.output.includes("Threads: null")
        !page.output.includes("Hazelcast")
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
        const jobs = ec.resource.getLocationText("component://tools/screen/System/ServiceJob/Jobs/ServiceJobList.xml")
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
        const failed = test.render("Service/ServiceRun/run", {serviceName, numberIn: "not a number"}, "post")
        return {run, kept, page, textOut1: textOut1 && textOut1[1], textOut2: textOut2 && textOut2[1],
            failedErrors: failed.errorMessages, staleResults: ec.web.sessionAttributes.toolsServiceRunResults}
    },
    then({run, kept, page, textOut1, textOut2, failedErrors, staleResults}) {
        run.errorMessages.length == 0
        kept.serviceName == "org.moqui.impl.BasicServices.echo#Data"
        kept.results.textOut1 == "UI review echo"
        Number(kept.results.numberOut) == 0
        page.status == 200
        page.errors == ""
        textOut1 == "UI review echo"
        textOut2 == "ping"
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


feature("job run outcome and timing distinguish incomplete history and retain diagnostic windows", {
    expect() {
        // The existing tools acceptance sweep owns this screen's HTML render. Check its actual display rules here.
        const source = ec.resource.getLocationText("component://tools/screen/System/ServiceJob/JobRuns/JobRunDetail.xml")
        const actions = /<script><!\[CDATA\[([\s\S]*?)\]\]><\/script>/.exec(source)[1]
        const start = new Date("2026-11-04T18:28:00.123Z")
        const end = new Date("2026-11-04T18:28:00.130Z")
        const run = (overrides) => ({jobRunId: "UI_JOB", jobName: "ui.fixture", userId: "EX_JOHN_DOE",
            startTime: start, endTime: end, hasError: "N", errors: "", runThread: 'worker "one"', ...overrides})
        const inspect = serviceJobRun => {
            const state = {ec, serviceJobRun, runUser: {username: "john.doe", userFullName: "John Doe"},
                runOutcomeText: null, runOutcomeStyle: null, runOutcomeDetail: null, runElapsedText: null,
                runStartText: null, runEndText: null, runUserText: null,
                logFrom: null, logThru: null, traceFrom: null, traceThru: null, logQueryString: null}
            runInContext(actions, state)
            return state
        }
        const complete = inspect(run({results: '{"removedCount":0}'}))
        assert.equal(complete.runOutcomeText, "Completed successfully")
        assert.equal(complete.runElapsedText, "7 ms")
        assert.equal(complete.runStartText, ec.l10n.format(start, "yyyy-MM-dd HH:mm:ss.SSS") + " " + ec.user.timeZone)
        assert.equal(complete.runEndText, ec.l10n.format(end, "yyyy-MM-dd HH:mm:ss.SSS") + " " + ec.user.timeZone)
        assert(complete.runStartText.includes(".123"), "display retains millisecond precision")
        assert.equal(complete.runUserText, "john.doe — John Doe (EX_JOHN_DOE)")
        assert.equal(complete.serviceJobRun.results, '{"removedCount":0}')
        assert.equal(complete.logFrom, "2026-11-04T18:28:00.123Z")
        assert.equal(complete.logThru, "2026-11-04T18:28:01.130Z")
        assert.equal(complete.traceFrom, "2026-11-04T18:27:00.123Z")
        assert.equal(complete.traceThru, "2026-11-04T18:29:00.130Z")
        assert.equal(complete.logQueryString, 'thread.name:' + JSON.stringify('worker "one"'))
        for (const [row, outcome, duration] of [
            [run({hasError: "Y", errors: "Fixture failure"}), "Completed with errors", "7 ms"],
            [run({errors: "An error was recorded without a flag"}), "Completed with errors", "7 ms"],
            [run({hasError: null}), "Finished — outcome not recorded", "7 ms"],
            [run({endTime: null}), "Not finished", "Unavailable"],
            [run({endTime: null, hasError: "Y"}), "Not finished", "Unavailable"],
            [run({startTime: null, endTime: null}), "Not finished", "Unavailable"],
            [run({endTime: "not a timestamp"}), "Completion time unavailable", "Unavailable"],
            [run({startTime: end, endTime: start}), "Completed successfully", "Unavailable"],
            [run({startTime: 0, endTime: 0}), "Completed successfully", "0 ms"],
            [null, "Run unavailable", "Unavailable"]
        ]) {
            const state = inspect(row)
            assert.equal(state.runOutcomeText, outcome)
            assert.equal(state.runElapsedText, duration)
        }
        const unfinished = inspect(run({endTime: null}))
        assert(unfinished.runOutcomeDetail.includes("may still be active or may have stopped"))
        assert.equal(unfinished.runEndText, "Not recorded")
        assert.equal(unfinished.logThru, "now")
        assert.equal(unfinished.traceThru, "now")
        const missing = inspect(null)
        assert.equal(missing.logFrom, "now-1d")
        assert.equal(missing.traceFrom, "now-7d")
        assert.equal(missing.logQueryString, "")
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
