// A fixture of runtime/base-component/tools/test/LoadRunner.spec.js (a dev component, so it is not on a production
// server): what a load user's engine in a server run can reach. Each iteration fails with what it saw, which the spec
// reads from the run's error samples.
plan({
    title: "What a load user reaches",
    description: "A fixture of the Tools load runner's spec",
    uses: ["service"],
    params: { mode: { default: "ec", options: ["ec", "host", "http", "toplevel", "tamper", "spin", "authz"] } },
})

const seen = (fn) => {
    try {
        return "ok " + String(fn())
    } catch (e) {
        return "threw " + (e && e.message ? e.message : String(e))
    }
}
const ECHO = JSON.stringify({ name: "org.moqui.impl.BasicServices.echo#Data", parameters: { textIn1: "x" }, opts: {} })

// at the top level, around the prelude's refusal: the host refuses it too, before the user has begun
const topLevel = params.mode === "toplevel" ? seen(() => __moquiTestHost.call("service.call", ECHO)) : null

scenario("reach", {
    users: 1, iterations: 1,
    run() {
        if (params.mode === "ec") {
            fail([
                "typeof " + seen(() => typeof ec),
                "disableAuthz " + seen(() => ec.artifactExecution.disableAuthz()),
                "delete " + seen(() => delete globalThis.ec),
                "after " + seen(() => ec.user.username),
            ].join("; "))
        } else if (params.mode === "host") {
            fail([
                "getProperty " + seen(() => System.getProperty("entity_ds_password")),
                "setProperty " + seen(() => __moquiTestHost.setProperty("load_runner_max_runs", "100")),
                "sealed " + seen(() => UserSecurityChecks.sealedPermissions()),
            ].join("; "))
        } else if (params.mode === "toplevel") {
            fail("toplevel " + topLevel)
        } else if (params.mode === "tamper") {
            fail([
                // another run's load, and a second begin of this thread's
                "other " + seen(() => __moquiTestHost.call("thread.begin", JSON.stringify({ load: 987654321, scenario: 0, user: 0 }))),
                // an early end, which would drop the recorder and its stop checks: then nothing more is sent
                "end " + seen(() => __moquiTestHost.call("thread.end", "{}")),
                "after " + seen(() => __moquiTestHost.call("service.call", ECHO)),
            ].join("; "))
        } else if (params.mode === "spin") {
            // no host call: only the interrupt of a stop now ends it
            for (;;) { /* spin */ }
        } else if (params.mode === "authz") {
            // a service that leaves authorization off, then a create the user is not granted
            const left = service.call("example.LoadFixtureServices.leave#AuthzDisabled", {}, { expect: ["error"] })
            const create = seen(() => service.call("create#moqui.basic.Enumeration",
                { enumId: "SPEC_LR_AUTHZ_LEAK", enumTypeId: "SpecLoad", description: "never" }))
            // the create's own error is its sample; this names only whether it threw, so it is counted as well
            fail("left " + (left && left.error) + "; create " + (create.startsWith("threw") ? "threw" : "ran"))
        } else {
            // a script that declares only service makes no HTTP request
            http.get("/")
        }
    },
})
