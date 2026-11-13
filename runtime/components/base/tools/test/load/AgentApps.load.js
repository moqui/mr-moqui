// runtime/components/base/tools/test/load/AgentApps.load.js: the apps over SSH as an agent drives them (agent-apps,
// docs/ui/TUI.md "For agents"), each user a session of its own that finds a user account in System, one exec a step.
// Each step is a sample of kind ssh named by what it does (plans/archive/LoadRunner.md L7). The users share one account,
// whose terminal sessions are limited (terminal max-sessions-per-user, 4 by default; conf/MoquiLoadTestConf.xml raises
// it), as are the server's SSH connections (ssh-server max-sessions, 8).
plan({
    title: "Agent apps",
    description: "The apps over SSH, as an agent drives them: open, find a user account, close",
    uses: ["ssh"],
    params: {
        user: { default: "john.doe", description: "The account every virtual user logs in as" },
        password: { default: "moqui", secret: true, description: "Its password" },
        username: { default: "john", description: "What the find of user accounts looks for" },
    },
    thresholds: { errorRate: 0 },
})

const LIST = "/apps/system/Security/UserAccount/UserAccountList"

scenario("agents", {
    users: 4, rampUp: 2, iterations: 5, think: [500, 1500],
    setup(user) {
        // one connection a user for its life; the session is the iteration's
        this.conn = ssh.connect({ user: params.user, password: params.password })
    },
    run(user, iteration) {
        step("find a user account", () => {
            const apps = this.conn.agentApps(LIST, { size: "120x40" })
            apps.command("key f", { name: "open find" })
            apps.command(`set Username ${params.username}`, { name: "set username" })
            const found = apps.command("key Enter", { name: "find" })
            if (!found.ok) fail(`the find answered ${found.reason}`)
            apps.close()
        })
    },
    teardown(user) { this.conn.close() },
})
