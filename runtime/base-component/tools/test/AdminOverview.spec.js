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
