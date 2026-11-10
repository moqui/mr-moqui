// A fixture of runtime/base-component/tools/test/LoadRunner.spec.js (a dev component, so it is not on a production
// server): what a load user's engine in a server run can reach. Each iteration fails with what it saw, which the spec
// reads from the run's error samples.
plan({
    title: "What a load user reaches",
    description: "A fixture of the Tools load runner's spec",
    uses: ["service"],
    params: { mode: { default: "ec", options: ["ec", "host", "http"] } },
})

const seen = (fn) => {
    try {
        return "ok " + String(fn())
    } catch (e) {
        return "threw " + (e && e.message ? e.message : String(e))
    }
}

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
        } else {
            // a script that declares only service makes no HTTP request
            http.get("/")
        }
    },
})
