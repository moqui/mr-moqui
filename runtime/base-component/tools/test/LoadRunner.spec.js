// Load runs in the server (framework/service/org/moqui/dev/LoadServices.xml, crates/moqui-host/src/load_runner;
// plans/LoadRunner.md §7, §10, L3): a holder of LOAD_RUNNER starts the shipped ServiceCall.load.js, whose calls run in
// process as that user with authorization required (D10); the refusals of the permission, of a file that is not a
// load script, of a target the run may not reach and of a second run at once; a stop; errors counted per call.
// The screens with their own templates (plans/LoadRunner.md §8, L4) render once in each mode: Run for a finished run
// and NewRun for ServiceCall, in qvue (the server-static .qvue, with the JSON it asks for) and in tui.
// Run: ./moq test tools --spec LoadRunner
const S = "org.moqui.dev.LoadServices."
const SCRIPT = "component://tools/test/load/ServiceCall.load.js"
const holderId = "SPEC_LR_HOLDER"
const otherId = "SPEC_LR_OTHER"
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
// a service call's out-parameters and its errors, the errors cleared
function invoke(name, parameters = {}) {
    try {
        const result = ec.service.sync().name(name).parameters(parameters).call() || {}
        const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
        ec.message.clearErrors()
        return Object.assign({}, result, { error })
    } catch (e) {
        ec.message.clearErrors()
        return { error: String(e.message || e) }
    }
}
// a quick run: no ramp and no think, the given users and iterations
const quick = (users, iterations, more = {}) => ({ calls: Object.assign({ users, iterations, rampUp: 0, think: 0 }, more) })
const start = (parameters) => invoke(S + "start#LoadRun", Object.assign({ scriptLocation: SCRIPT, targetKind: "service" }, parameters))
const ended = (runId) => invoke(S + "get#LoadRunStatus", { runId, waitSeconds: 10 })
const result = (runId) => invoke(S + "get#LoadRunResult", { runId }).result
const runIds = []
const REACH = "component://example/test/load/ScriptReach.load.js"
const REFUSED_ENUM = "SPEC_LR_REFUSED"
// the message of a run's first error sample
const firstError = (runId) => result(runId).errorSamples[0].message
// a Tools › Load screen in a mode: its status, its errors and whether the output reports a render error
function render(path, parameters, mode) {
    const str = ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").renderMode(mode)
        .render(path, Object.assign({ lastStandalone: "-2" }, parameters), null)
    const output = String(str.output || "")
    ec.message.clearErrors()
    return { status: str.status, errors: str.errorMessages.join(" | "), output, broken: /Error rendering|Exception in render/.test(output) }
}

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        make("moqui.security.UserAccount", { userId: holderId, username: "spec.lr.holder", userFullName: "Spec Load Runner" })
        make("moqui.security.UserAccount", { userId: otherId, username: "spec.lr.other", userFullName: "Spec Load Other" })
        // both: the Tools app and the load services; the holder also LOAD_RUNNER and echo#Data, not find#Enumeration
        make("moqui.security.UserGroup", { userGroupId: "SPEC_LR_TOOLS", description: "Spec load runner tools" })
        make("moqui.security.UserGroup", { userGroupId: "SPEC_LR_HOLDERS", description: "Spec load runner holders" })
        make("moqui.security.ArtifactAuthz", { artifactAuthzId: "SPEC_LR_TOOLS_APP", userGroupId: "SPEC_LR_TOOLS",
            artifactGroupId: "TOOLS_APP", authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL" })
        make("moqui.security.ArtifactGroup", { artifactGroupId: "SPEC_LR_SERVICES", description: "Spec load services" })
        make("moqui.security.ArtifactGroupMember", { artifactGroupId: "SPEC_LR_SERVICES", artifactName: "org\\.moqui\\.dev\\.LoadServices\\..*",
            nameIsPattern: "Y", artifactTypeEnumId: "AT_SERVICE", inheritAuthz: "N" })
        make("moqui.security.ArtifactAuthz", { artifactAuthzId: "SPEC_LR_SERVICES", userGroupId: "SPEC_LR_TOOLS",
            artifactGroupId: "SPEC_LR_SERVICES", authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL" })
        make("moqui.security.ArtifactGroup", { artifactGroupId: "SPEC_LR_ECHO", description: "Spec load echo" })
        make("moqui.security.ArtifactGroupMember", { artifactGroupId: "SPEC_LR_ECHO", artifactName: "org.moqui.impl.BasicServices.echo#Data",
            nameIsPattern: "N", artifactTypeEnumId: "AT_SERVICE", inheritAuthz: "N" })
        // set#Preference writes a row keyed by the current user: what shows whom a call ran as
        // the dev example's fixture that leaves its caller's authorization off
        make("moqui.security.ArtifactGroupMember", { artifactGroupId: "SPEC_LR_ECHO", artifactName: "example.LoadFixtureServices.leave#AuthzDisabledAccountDisabled",
            nameIsPattern: "N", artifactTypeEnumId: "AT_SERVICE", inheritAuthz: "N" })
        make("moqui.security.ArtifactGroupMember", { artifactGroupId: "SPEC_LR_ECHO", artifactName: "example.LoadFixtureServices.leave#AuthzDisabled",
            nameIsPattern: "N", artifactTypeEnumId: "AT_SERVICE", inheritAuthz: "N" })
        make("moqui.security.ArtifactGroupMember", { artifactGroupId: "SPEC_LR_ECHO", artifactName: "org.moqui.impl.UserServices.set#Preference",
            nameIsPattern: "N", artifactTypeEnumId: "AT_SERVICE", inheritAuthz: "N" })
        make("moqui.security.ArtifactAuthz", { artifactAuthzId: "SPEC_LR_ECHO", userGroupId: "SPEC_LR_HOLDERS",
            artifactGroupId: "SPEC_LR_ECHO", authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL" })
        make("moqui.security.UserGroupPermission", { userGroupId: "SPEC_LR_HOLDERS", userPermissionId: "LOAD_RUNNER", fromDate: 0 })
        for (const userId of [holderId, otherId])
            make("moqui.security.UserGroupMember", { userGroupId: "SPEC_LR_TOOLS", userId, fromDate: 0 })
        make("moqui.security.UserGroupMember", { userGroupId: "SPEC_LR_HOLDERS", userId: holderId, fromDate: 0 })
        ec.artifactExecution.enableAuthz()
    },
    setup() { ec.user.internalLoginUser("spec.lr.holder") },
    cleanupSpec() {
        ec.user.internalLoginUser("spec.lr.holder")
        for (const runId of runIds) invoke(S + "delete#LoadRun", { runId })
        ec.artifactExecution.disableAuthz()
        ec.entity.find("moqui.security.UserPreference").condition("userId", "in", [holderId, otherId]).deleteAll()
        ec.entity.find("moqui.basic.Enumeration").condition("enumId", "in", [REFUSED_ENUM, "SPEC_LR_AUTHZ_LEAK"]).deleteAll()
        ec.entity.find("moqui.security.UserGroupMember").condition("userId", "in", [holderId, otherId]).deleteAll()
        ec.entity.find("moqui.security.UserGroupPermission").condition("userGroupId", "SPEC_LR_HOLDERS").deleteAll()
        ec.entity.find("moqui.security.ArtifactAuthz").condition("artifactAuthzId", "in", ["SPEC_LR_TOOLS_APP", "SPEC_LR_SERVICES", "SPEC_LR_ECHO"]).deleteAll()
        ec.entity.find("moqui.security.ArtifactGroupMember").condition("artifactGroupId", "in", ["SPEC_LR_SERVICES", "SPEC_LR_ECHO"]).deleteAll()
        ec.entity.find("moqui.security.ArtifactGroup").condition("artifactGroupId", "in", ["SPEC_LR_SERVICES", "SPEC_LR_ECHO"]).deleteAll()
        ec.entity.find("moqui.security.UserGroup").condition("userGroupId", "in", ["SPEC_LR_TOOLS", "SPEC_LR_HOLDERS"]).deleteAll()
        ec.entity.find("moqui.security.UserAccount").condition("userId", "in", [holderId, otherId]).deleteAll()
        ec.artifactExecution.enableAuthz()
    },
})

feature("a holder runs ServiceCall for 2 users and 2 iterations: completed with 4 requests, and listed", {
    expect() {
        const loggedOut = () => ec.entity.find("moqui.security.UserAccount").condition("userId", holderId).disableAuthz().one().hasLoggedOut
        const loggedOutBefore = loggedOut()
        const started = start({ scenarios: quick(2, 2), label: "spec" })
        started.error == ""
        runIds.push(started.runId)
        const status = ended(started.runId)
        // the run's own logins do not sign its starter out (hasLoggedOut is what a browser session checks)
        loggedOut() == loggedOutBefore
        result(started.runId).run.scenarioSettings.calls.users == 2
        result(started.runId).run.scenarioSettings.calls.iterations == 2
        status.running == false
        status.status == "completed"
        status.summary.requests == 4
        status.summary.errors == 0
        status.summary.user == "spec.lr.holder"
        const listed = invoke(S + "find#LoadRuns", { scriptLocation: SCRIPT }).runs
        listed.some(run => run.runId == started.runId && run.status == "completed" && run.targetKind == "service")
        // the script list reads each plan
        const script = invoke(S + "find#LoadScripts").scripts.find(s => s.location == SCRIPT)
        script.title == "Service call"
        script.uses.includes("service")
        script.scenarios[0].users == 10
        script.params.serviceName.default == "org.moqui.impl.BasicServices.echo#Data"
    },
})

feature("a call runs as the user who started the run", {
    expect() {
        const started = start({ scenarios: quick(1, 1), parameters: { serviceName: "org.moqui.impl.UserServices.set#Preference",
            parameters: '{"preferenceKey": "SpecLoadRunner", "preferenceValue": "ran"}' } })
        started.error == ""
        runIds.push(started.runId)
        ended(started.runId).summary.errors == 0
        ec.entity.find("moqui.security.UserPreference").condition("userId", holderId).condition("preferenceKey", "SpecLoadRunner")
            .disableAuthz().one().preferenceValue == "ran"
    },
})

feature("a load script reaches neither ec nor the server's own functions, and makes only the kinds of load it declares", {
    expect() {
        const reach = (mode) => {
            const started = start({ scriptLocation: REACH, parameters: { mode } })
            runIds.push(started.runId)
            ended(started.runId)
            return firstError(started.runId)
        }
        const ecSeen = reach("ec")
        ecSeen.includes("typeof threw")
        ecSeen.includes("disableAuthz threw")
        ecSeen.includes("after threw")
        const hostSeen = reach("host")
        hostSeen.includes("getProperty threw System.getProperty is not available")
        hostSeen.includes("setProperty threw __moquiTestHost.setProperty is not available")
        hostSeen.includes("sealed threw")
        reach("http").includes('uses: ["http"]')
    },
})

feature("a user without LOAD_RUNNER is refused", {
    expect() {
        ec.user.internalLoginUser("spec.lr.other")
        const refused = start({ scenarios: quick(2, 2) })
        refused.error.includes("LOAD_RUNNER")
        refused.runId == null
    },
})

feature("a call of a service the starting user is not authorized for is an error and does not run", {
    expect() {
        // a create the holder is not granted: refused, and nothing written
        const started = start({ scenarios: quick(1, 1), parameters: { serviceName: "create#moqui.basic.Enumeration",
            parameters: JSON.stringify({ enumId: REFUSED_ENUM, enumTypeId: "SpecLoad", description: "never" }) } })
        started.error == ""
        runIds.push(started.runId)
        ended(started.runId).summary.errors == 1
        firstError(started.runId).toLowerCase().includes("authoriz")
        ec.entity.find("moqui.basic.Enumeration").condition("enumId", REFUSED_ENUM).disableAuthz().count() == 0
    },
})

feature("a service error is counted and the user's next call still runs", {
    expect() {
        const started = start({ scenarios: quick(1, 2), parameters: { parameters: '{"numberIn": "not a number"}' } })
        started.error == ""
        runIds.push(started.runId)
        const summary = ended(started.runId).summary
        summary.iterations == 2
        summary.errors == 2
    },
})

feature("a file that is not a load script and a target the run may not reach are refused", {
    expect() {
        start({ scriptLocation: "component://tools/test/LoadRunner.spec.js" }).error.includes("not a load script")
        start({ scriptLocation: "component://tools/test/load/../LoadRunner.spec.js" }).error.includes("not a load script")
        // a dev instance reaches any URL; a production one only this server, loopback and load_runner_targets
        withProperty("instance_purpose", "production", () =>
            start({ targetKind: "url", targetUrl: "https://example.org/" }).error).includes("load_runner_targets")
        // ServiceCall calls services in process: over HTTP it is refused
        start({ targetKind: "this" }).error.includes("service target")
    },
})

feature("a second run while one runs is refused, its users change while it runs, and stop ends the run", {
    expect() {
        const long = start({ scenarios: { calls: { users: 1, duration: 30, rampUp: 0, think: 20 } } })
        long.error == ""
        runIds.push(long.runId)
        invoke(S + "get#LoadRunStatus", { runId: long.runId }).running == true
        withProperty("load_runner_max_runs", "1", () => start({ scenarios: quick(1, 1) }).error).includes("already running")
        invoke(S + "delete#LoadRun", { runId: long.runId }).error.includes("running")
        // the run page's control (Load/updateUsers, update#LoadRunUsers): the scenario's target is 2 users from now on,
        // and a rate for a closed scenario is refused
        const update = (parameters) => JSON.parse(String(ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml")
            .render("Load/updateUsers", Object.assign({ runId: long.runId, scenario: "calls" }, parameters), "post").output || "{}"))
        update({ users: 2 }).errors == null
        invoke(S + "get#LoadRunStatus", { runId: long.runId }).snapshot.targets.calls.users == 2
        update({ rate: 5 }).errors != null
        invoke(S + "stop#LoadRun", { runId: long.runId, now: true }).error == ""
        const status = ended(long.runId)
        status.status == "stopped"
        result(long.runId).run.stopReason == "stop-now"
    },
})

feature("a finished run's page and ServiceCall's run form render in qvue and in tui", {
    when() {
        const started = start({ scenarios: quick(1, 2), label: "spec render" })
        runIds.push(started.runId)
        ended(started.runId)
        const json = (path, parameters) => JSON.parse(String(ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml")
            .render(path, parameters, "get").output || "{}"))
        return {
            runId: started.runId,
            run: render("Load/Run", { runId: started.runId }, "qvue"),
            status: json("Load/runStatus", { runId: started.runId, withTitle: "true" }),
            runTui: render("Load/Run", { runId: started.runId }, "tui"),
            form: render("Load/NewRun", { scriptLocation: SCRIPT }, "qvue"),
            plan: json("Load/planForm", { scriptLocation: SCRIPT }),
            again: json("Load/planForm", { runId: started.runId }),
            formTui: render("Load/NewRun", { scriptLocation: SCRIPT }, "tui"),
        }
    },
    then({ runId, run, status, runTui, form, plan, again, formTui }) {
        // the qvue is the same for every run (server-static): its own parts, and the header and a name's row from the
        // runStatus it asks
        run.status == 200 && run.errors == "" && !run.broken
        run.output.includes("Requests and errors per second")
        run.output.includes("Run again")
        run.output.includes("Change a scenario's users") && run.output.includes("'Dropped'")
        status.found == true && status.status == "completed" && status.scriptTitle == "Service call" && status.label == "spec render"
        // a closed scenario: no Dropped card, and no scenario control once it has ended
        status.hasRate == false && status.targets.length == 0
        status.names[0].name == "org.moqui.impl.BasicServices.echo#Data" && status.names[0].count == 2
        // the terminal page has the run in it
        runTui.status == 200 && runTui.errors == "" && !runTui.broken
        runTui.output.includes('"text":"Service call · spec render"')
        runTui.output.includes("Completed · " + runId)
        runTui.output.includes('"cells":["org.moqui.impl.BasicServices.echo#Data","service","2","0"')
        runTui.output.includes('"LoadRunAgain"')
        !runTui.output.includes('"LoadRunCardDropped"') && !runTui.output.includes('"LoadRunTargets"')
        // the plan's title, its params and its scenario, and the one target a service script has
        form.status == 200 && form.errors == "" && !form.broken
        form.output.includes("Scenarios")
        plan.title == "Service call" && plan.targets.join(",") == "service"
        plan.params.some((param) => param.name == "serviceName" && param.kind == "text")
        // Run again: the run's script and label, and each scenario's settings as the run used them
        again.found == true && again.fromRunId == runId && again.label == "spec render"
        again.scenarios[0].users == 1 && again.scenarios[0].iterations == 2 && again.scenarios[0].rampUp == 0
        formTui.status == 200 && formTui.errors == "" && !formTui.broken
        formTui.output.includes('"text":"Service call"')
        formTui.output.includes('"name":"p_serviceName"')
        formTui.output.includes('"name":"s_calls_users"')
    },
})

// ========== L5: import and compare (plans/LoadRunner.md §7, L5) ==========
// a result as moqui test-load writes it (runtime/tmp/test-load/<runId>/result.json), made up
const madeUp = (requests, p95, label) => ({
    users: 2, iterations: 4, requests, errors: 0, errorRate: 0, rps: requests / 2, duration: 2,
    latency: { min: 1, mean: 2, p50: 2, p90: 3, p95, p99: p95 + 1, max: p95 + 2 },
    names: { "GET /ping": { kind: "http", count: requests, errors: 0, p50: 2, p95, p99: p95 + 1, statuses: { "200": requests } } },
    nameOrder: ["GET /ping"], errorSamples: [], generator: { where: "same-machine", cores: 4, cpu: 0.01 },
    run: { script: "component://mr-marble/test/load/OrderToCash.load.js", label, status: "completed", stopReason: null,
        target: "https://staging.example.com", start: "2026-11-10T15:42:43Z", end: "2026-11-10T15:42:45Z", user: "someone.else" },
})

feature("an import of a result file makes a completed run of the importer, and a file that is not a result is refused", {
    expect() {
        const imported = invoke(S + "import#LoadRunResult", { result: JSON.stringify(madeUp(40, 8, "from staging")) })
        imported.error == ""
        runIds.push(imported.runId)
        const status = invoke(S + "get#LoadRunStatus", { runId: imported.runId })
        status.status == "completed"
        status.summary.user == "spec.lr.holder"
        status.summary.label == "from staging"
        result(imported.runId).run.importedUser == "someone.else"
        invoke(S + "find#LoadRuns", { status: "completed" }).runs.some(run => run.runId == imported.runId)
        invoke(S + "import#LoadRunResult", { result: '{"a": 1}' }).error.includes("not a load result")
        invoke(S + "import#LoadRunResult", { result: "not json" }).error.includes("not JSON")
    },
})

feature("compare#LoadRuns sets two runs side by side with the percent changes from the first", {
    expect() {
        const first = invoke(S + "import#LoadRunResult", { result: JSON.stringify(madeUp(40, 8, "base")) }).runId
        const second = invoke(S + "import#LoadRunResult", { result: JSON.stringify(madeUp(30, 10, "after")) }).runId
        runIds.push(first, second)
        const compared = invoke(S + "compare#LoadRuns", { runIds: [first, second] })
        compared.error == ""
        compared.runs.map(run => run.label).join() == "base,after"
        const row = (section, name, metric) => compared.rows.find(r => r.section == section && r.name == name && r.metric == metric)
        row("summary", null, "requests").values.join() == "40,30"
        row("summary", null, "requests").changes[1] == -25
        row("summary", null, "p95").changes[1] == 25
        row("name", "GET /ping", "count").changes[1] == -25
        row("summary", null, "errors").changes[1] == null
        invoke(S + "compare#LoadRuns", { runIds: [first] }).error.includes("2 to 4")
    },
})

feature("a run on this server keeps the server's samples and notes that its generator CPU includes the server", {
    expect() {
        const started = start({ scenarios: quick(1, 1) })
        runIds.push(started.runId)
        ended(started.runId).status == "completed"
        const done = result(started.runId)
        // a run of milliseconds: one sample, at its end, with no CPU share over so short a span
        done.server.samples.length >= 1
        // the process's memory is read where the platform reports it (/proc, Linux); elsewhere the sample says so
        const memory = ec.factory.getStatusMap().Process.MemoryUsed
        // (a decimal where it is reported: compared by identity, since == asks a decimal for its valueOf)
        memory === null || memory === undefined ? done.server.samples[0].memoryMb == null : done.server.samples[0].memoryMb > 0
        done.server.samples[0].cpu == null
        done.generator.where == "in-target"
        done.generator.note.includes("server")
    },
})

// ========== The run's binding of its threads (plans/LoadRunner.md §10) ==========
// what a script reaches calling the host directly, around the prelude: example's dev-only ScriptReach.load.js
const reached = (mode) => {
    const started = start({ scriptLocation: REACH, parameters: { mode } })
    runIds.push(started.runId)
    ended(started.runId)
    // every error sample's message: a counted call's error comes before the iteration's own
    return result(started.runId).errorSamples.map(sample => sample.message).join(" | ")
}

feature("a script's own host calls neither act at its top level nor rebind or end its thread", {
    expect() {
        const top = reached("toplevel")
        top.includes("call threw a load script makes no request at its top level")
        top.includes("ramp threw load.ramp is only for a load user's scenario")
        top.includes("sleep threw sleep is only for a load user's scenario")
        top.includes("token ok null")
        const tampered = reached("tamper")
        tampered.includes("other threw thread.begin names a load that is not this thread's")
        tampered.includes("after threw this load user has ended")
    },
})

feature("a call that leaves authorization off is an error, and the user's next call is still authorized", {
    expect() {
        const seen = reached("authz")
        seen.includes("left authorization disabled")
        seen.includes("create threw")
        seen.toLowerCase().includes("authoriz")
        ec.entity.find("moqui.basic.Enumeration").condition("enumId", "SPEC_LR_AUTHZ_LEAK").disableAuthz().count() == 0
    },
})

feature("a stop now ends a script that spins without a host call", {
    expect() {
        const started = start({ scriptLocation: REACH, parameters: { mode: "spin" } })
        started.error == ""
        runIds.push(started.runId)
        invoke(S + "stop#LoadRun", { runId: started.runId, now: true }).error == ""
        const status = ended(started.runId)
        status.running == false
        status.status == "stopped"
    },
})

feature("when a call's changed context cannot be renewed, the user makes no more calls", {
    expect() {
        try {
            reached("renew").includes("next refused")
        } finally {
            // the fixture disabled the holder's account
            ec.artifactExecution.disableAuthz()
            const account = ec.entity.find("moqui.security.UserAccount").condition("userId", holderId).one()
            account.disabled = "N"
            account.update()
            ec.artifactExecution.enableAuthz()
        }
    },
})
