// ssh against the server's SSH listener (server.ssh: crates/moqui-host/tests/it/test_client.rs passes --ssh), as the
// load scripts use it (plans/archive/LoadRunner.md L7): an exec, an agent-apps session and a pty shell, each a sample of kind
// ssh in a load, with an exit status and a refused host key.
spec({
    setupSpec() {
        if (!server.ssh) skip("the server has no SSH listener here (pass --ssh host:port)")
    },
})

const ARGS = { user: params.user ?? "john.doe", password: params.password ?? "moqui" }

feature("outside a load an exec answers its exit status and a pinned key that differs is refused", {
    when() {
        const conn = ssh.connect({ user: ARGS.user, password: ARGS.password })
        const help = conn.exec("agent-apps help")
        const missing = conn.exec("agent-apps nosuch key x")
        conn.close()
        let refused = null
        try {
            ssh.connect({ user: ARGS.user, password: ARGS.password, fingerprint: "SHA256:notthiskey" })
        } catch (e) {
            refused = String(e.message || e)
        }
        return { help, missing, refused }
    },
    then({ help, missing, refused }) {
        help.status == 0
        help.stdout.includes("agent-apps open")
        missing.status == 2
        missing.stdout.includes("no session nosuch")
        refused.includes("not the pinned SHA256:notthiskey")
    },
})

feature("in a load, a connect, an exec, an agent-apps session and a shell are timed by name", {
    when() {
        return load({
            users: 2, iterations: 1, args: ARGS,
            scenario(user, args) {
                const conn = ssh.connect({ user: args.user, password: args.password })
                conn.exec("agent-apps help", { name: "help" })
                const apps = conn.agentApps("apps/system", { size: "100x30" })
                const answer = apps.command("elements", { name: "elements" })
                if (!answer.ok || !Array.isArray(answer.elements)) fail("no elements: " + JSON.stringify(answer).slice(0, 200))
                apps.close()
                // a user with the admin shell gets its prompt, any other the apps
                const shell = conn.shell({ cols: 100, rows: 30 })
                shell.expect(/moqui> |System/, { name: "first screen", timeout: 20 })
                shell.close()
                // an exit status the scenario does not expect ends its iteration as an error
                conn.exec("agent-apps nosuch key x", { name: "no session" })
                conn.exec("agent-apps help", { name: "never" })
            },
        })
    },
    then(stats) {
        stats.names["ssh connect"].kind == "ssh"
        stats.names["ssh connect"].outcomes.ok == 2
        stats.names["help"].outcomes["exit 0"] == 2
        stats.names["agent-apps open"].outcomes["exit 0"] == 2
        stats.names["elements"].outcomes["exit 0"] == 2
        stats.names["agent-apps close"].count == 2
        stats.names["first screen"].outcomes.ok == 2
        stats.names["no session"].outcomes["exit 2"] == 2
        stats.names["never"] == null
        stats.errors == 2
        stats.errorSamples[0].message.includes("exit 2")
    },
})
