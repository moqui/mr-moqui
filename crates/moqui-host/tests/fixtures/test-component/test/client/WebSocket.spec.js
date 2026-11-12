// The notification socket: an anonymous upgrade is refused with its status, and a logged-in session's cookies open
// it.
feature("an anonymous /notws upgrade is refused, not thrown", {
    when() { return { socket: ws.connect("/notws", { session: session() }) } },
    then({ socket }) {
        !socket.open
        socket.status == 401
    },
})

feature("a logged-in session opens /notws, sends and closes", {
    when() {
        const s = session({ user: "john.doe" })
        const socket = ws.connect("/notws", { session: s })
        socket.send("subscribe:ALL")
        const frame = socket.receive({ timeout: 0.3 })
        socket.close()
        return { socket, frame }
    },
    then({ socket, frame }) {
        socket.status == 101
        frame == null
        !socket.open
    },
})

// In a load (plans/archive/LoadRunner.md L7) a handshake, a named send and a receive are samples of kind ws: a refused
// handshake and a receive that times out are errors unless the call expects them.
feature("in a load, handshakes, named sends and receives are timed by name", {
    when() {
        return load({
            users: 2, iterations: 2, args: { user: params.user ?? "john.doe", password: params.password ?? "moqui" },
            setup(user, args) { login(args.user, args.password) },
            scenario(user, args, iteration) {
                const socket = ws.connect("/notws", { name: "notws" })
                socket.send("subscribe:ALL", { name: "subscribe" })
                socket.send("subscribe:ALL")
                // nothing answers a bare subscribe: this receive expects its timeout
                socket.receive({ timeout: 0.1, expect: ["timeout"] })
                socket.close()
                ws.connect("/notws", { session: session(), name: "anonymous", expect: [401] })
                // the last iteration's receive expects nothing, so its timeout is an error
                if (iteration == 1) ws.connect("/notws", { name: "again" }).receive({ timeout: 0.1, name: "unexpected" })
            },
        })
    },
    then(stats) {
        stats.names["notws"].kind == "ws"
        stats.names["notws"].outcomes["101"] == 4
        stats.names["notws"].statuses == null
        stats.names["subscribe"].count == 4
        stats.names["ws receive"].outcomes.timeout == 4
        stats.names["ws receive"].errors == 0
        stats.names["ws receive"].p50 >= 90
        stats.names["anonymous"].outcomes["401"] == 4
        stats.names["unexpected"].outcomes.timeout == 2
        stats.errors == 2
        stats.errorSamples[0].message.includes("no frame within 0.1 s")
    },
})
