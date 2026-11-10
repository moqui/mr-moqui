// load and the client's parallel: a tiny load timed by name, with an expected 401 that is not an error, a response
// that is (and ends its iteration), and a throw of the scenario; a load script (test/load/Fixture.load.js) with two
// scenarios, a broken threshold, a stop, and a request at a script's top level refused; then threads that each log in
// on a session of their own, one of them throwing.
const ARGS = { user: params.user ?? "john.doe", password: params.password ?? "moqui" }

feature("a tiny load times its requests by name", {
    when() {
        return load({
            users: 2, iterations: 3, think: 10, args: ARGS,
            setup(user, args) { login(args.user, args.password) },
            scenario(user, args, iteration) {
                http.get("/rest/s1/moqui/basic/enumTypes", { accept: "json", name: "enum types" })
                session().get("/rest/s1/moqui/basic/enumTypes", { accept: "json", expect: [401] })
                if (user == 1 && iteration == 2) throw new Error("planned")
            },
        })
    },
    then(stats) {
        stats.users == 2
        stats.iterations == 6
        stats.requests == 12
        stats.errors == 1
        stats.errorRate > 0
        stats.names["enum types"].count == 6
        stats.names["enum types"].statuses["200"] == 6
        stats.names["GET /rest/s1/moqui/basic/enumTypes"].statuses["401"] == 6
        stats.names["GET /rest/s1/moqui/basic/enumTypes"].errors == 0
        stats.errorSamples[0].message == "Error: planned"
        stats.latency.max >= stats.latency.p50
    },
})

feature("an error response ends its iteration", {
    when() {
        return load({
            users: 1, iterations: 2,
            scenario() {
                http.get("/rest/s1/moqui/basic/enumTypes", { accept: "json", name: "anonymous" })
                http.get("/Login", { name: "never" })
            },
        })
    },
    then(stats) {
        stats.requests == 2
        stats.errors == 2
        stats.names["anonymous"].statuses["401"] == 2
        stats.names["never"] == null
        stats.errorSamples[0].status == 401
    },
})

feature("a load script runs its scenarios together, each with its own users", {
    when() {
        return load({ script: "test/load/Fixture.load.js", params: { rounds: 1 }, label: "fixture" })
    },
    then(stats) {
        stats.errors == 0
        stats.users == 3
        stats.scenarios.readers == { users: 2, iterations: 4, errors: 0 }
        stats.scenarios.anonymous == { users: 1, iterations: 3, errors: 0 }
        stats.names["enum types"].count == 8
        stats.names["enum types"].kind == "http"
        stats.names["read twice"].kind == "step"
        stats.names["read twice"].count == 4
        stats.names["read twice"].outcomes.ok == 4
        stats.names["anonymous enums"].statuses["401"] == 3
        // the steps are not requests
        stats.requests == 14
        stats.binSeconds == 1
        stats.bins.length >= 1
        stats.bins.reduce((n, bin) => n + bin.requests, 0) == 14
        stats.passed
        stats.thresholds[0].metric == "errorRate"
        stats.run.script.endsWith("test/load/Fixture.load.js")
        stats.run.label == "fixture"
        stats.run.status == "completed"
        stats.run.params == { user: "john.doe", rounds: 1 }
        stats.run.scenarios.readers.pause == [1, 5]
        stats.generator.where == "same-machine"
    },
})

feature("a broken threshold fails the verdict, and scenarios names those that run", {
    when() {
        return load({
            script: "test/load/Fixture.load.js", scenarios: { anonymous: { iterations: 1 } },
            thresholds: { names: { "login page": { p95: 0 } } },
        })
    },
    then(stats) {
        stats.scenarios.readers == null
        stats.iterations == 1
        stats.passed == false
        stats.thresholds.find((t) => t.name == "login page").passed == false
        stats.thresholds.find((t) => t.metric == "errorRate").passed
    },
})

feature("a stop ends each user after its iteration", {
    when() {
        const started = Date.now()
        const stats = load({
            users: 2, duration: 30, think: 20, stopAfter: 0.3,
            scenario() { http.get("/Login", { name: "login page" }) },
        })
        return { stats, ms: Date.now() - started }
    },
    then({ stats, ms }) {
        ms < 10000
        stats.run.status == "stopped"
        stats.run.stopReason == "stop"
        stats.errors == 0
        stats.iterations > 0
    },
})

feature("a stop ends a long ramp at once", {
    when() {
        const started = Date.now()
        const stats = load({
            users: 2, iterations: 1, rampUp: 600, stopAfter: 0.3,
            scenario() { http.get("/Login", { name: "login page" }) },
        })
        return { stats, ms: Date.now() - started }
    },
    then({ stats, ms }) {
        ms < 10000
        // the second user, due at 300 s, never starts
        stats.iterations == 1
        stats.errors == 0
    },
})

feature("a request at a load script's top level is refused", {
    when() {
        try {
            load({ script: "test/load/TopLevel.load.js" })
            return { message: "ran" }
        } catch (e) {
            return { message: e.message }
        }
    },
    then({ message }) {
        message.includes("a load script makes no request at its top level")
        message.includes("TopLevel.load.js")
    },
})

feature("urlParam reads a query parameter of the response's url", {
    when() {
        const res = session().get("/Login?probe=a%20b&flag")
        return { probe: res.urlParam("probe"), flag: res.urlParam("flag"), missing: res.urlParam("missing") }
    },
    then({ probe, flag, missing }) {
        probe == "a b"
        flag == ""
        missing == null
    },
})

feature("parallel threads log in on sessions of their own", {
    when() {
        return parallel(3, (index, args) => {
            login(args.user, args.password)
            if (index == 2) throw new Error("thread two throws")
            return { index, cookie: http.cookies().some((c) => c.name == "moqui.session") }
        }, ARGS)
    },
    then({ results, errors }) {
        results.length == 3
        results[0].index == 0
        results[1].cookie
        results[2] == null
        errors.length == 1
        errors[0].startsWith("thread 2: Error: thread two throws")
    },
})
