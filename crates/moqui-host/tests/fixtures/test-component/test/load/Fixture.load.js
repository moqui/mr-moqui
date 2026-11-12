// A load script for the harness's own client spec (test/client/Load.spec.js, plans/archive/LoadRunner.md section 4): a plan
// with params and a threshold, a constant and a helper at the top level that both scenarios use, and two scenarios
// that start together.
plan({
    title: "Fixture",
    description: "Reads of the enum types, logged in and anonymous",
    params: {
        user: { default: "john.doe" },
        password: { default: "moqui", secret: true },
        rounds: { default: 1, description: "Pairs of reads an iteration" },
    },
    thresholds: { errorRate: 0.5 },
})

const ENUMS = "/rest/s1/moqui/basic/enumTypes"
const enums = (name, opts) => http.get(ENUMS, Object.assign({ accept: "json", name }, opts))

scenario("readers", {
    users: 2, iterations: 2, pause: [1, 5],
    setup(user) { login(params.user, params.password) },
    run(user, iteration) {
        step("read twice", () => {
            for (let i = 0; i < params.rounds * 2; i++) enums("enum types")
        })
    },
})

scenario("anonymous", {
    users: 1, iterations: 3, newSession: true,
    run(user, iteration) {
        http.get("/Login", { name: "login page" })
        // each user's state is its own `this`, kept across iterations
        this.seen = (this.seen || 0) + 1
        if (this.seen != iteration + 1) fail(`seen ${this.seen} in iteration ${iteration}`)
        enums("anonymous enums", { expect: [401] })
    },
})
