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
        ec.entity.find("moqui.basic.Enumeration").condition("enumId", REFUSED_ENUM).deleteAll()
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
        const started = start({ scenarios: quick(2, 2), label: "spec" })
        started.error == ""
        runIds.push(started.runId)
        const status = ended(started.runId)
        status.running == false
        status.status == "completed"
        status.summary.requests == 4
        status.summary.errors == 0
        status.summary.user == "spec.lr.holder"
        const listed = invoke(S + "find#LoadRuns", { scriptLocation: SCRIPT }).runs
        listed.some(run => run.runId == started.runId && run.status == "completed")
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

feature("a second run while one runs is refused, and stop ends the run", {
    expect() {
        const long = start({ scenarios: { calls: { users: 1, duration: 30, rampUp: 0, think: 20 } } })
        long.error == ""
        runIds.push(long.runId)
        invoke(S + "get#LoadRunStatus", { runId: long.runId }).running == true
        withProperty("load_runner_max_runs", "1", () => start({ scenarios: quick(1, 1) }).error).includes("already running")
        invoke(S + "delete#LoadRun", { runId: long.runId }).error.includes("running")
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
            formTui: render("Load/NewRun", { scriptLocation: SCRIPT }, "tui"),
        }
    },
    then({ runId, run, status, runTui, form, plan, formTui }) {
        // the qvue is the same for every run (server-static): its own parts, and the header and a name's row from the
        // runStatus it asks
        run.status == 200 && run.errors == "" && !run.broken
        run.output.includes("Requests and errors per second")
        run.output.includes("Run again")
        status.found == true && status.status == "completed" && status.scriptTitle == "Service call" && status.label == "spec render"
        status.names[0].name == "org.moqui.impl.BasicServices.echo#Data" && status.names[0].count == 2
        // the terminal page has the run in it
        runTui.status == 200 && runTui.errors == "" && !runTui.broken
        runTui.output.includes('"text":"Service call · spec render"')
        runTui.output.includes("Completed · " + runId)
        runTui.output.includes('"cells":["org.moqui.impl.BasicServices.echo#Data","service","2","0"')
        runTui.output.includes('"LoadRunAgain"')
        // the plan's title, its params and its scenario, and the one target a service script has
        form.status == 200 && form.errors == "" && !form.broken
        form.output.includes("Scenarios")
        plan.title == "Service call" && plan.targets.join(",") == "service"
        plan.params.some((param) => param.name == "serviceName" && param.kind == "text")
        formTui.status == 200 && formTui.errors == "" && !formTui.broken
        formTui.output.includes('"text":"Service call"')
        formTui.output.includes('"name":"p_serviceName"')
        formTui.output.includes('"name":"s_calls_users"')
    },
})
