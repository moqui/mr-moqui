// OWASP ASVS 5.0.0 V4 API and Web Service: the framework's proofs, each feature filed under the requirement its name
// starts with (docs/security/proofs/V4-API-and-Web-Service.md). Ported from Moqui Java's pytest proofs; a comment names
// the Python test, as in test/client.
//
// The origin of a WebSocket handshake is checked against the same list as CORS, empty by default, and compared whole:
// scheme, host and port (Java compares the host only and accepts an anonymous upgrade, ignoring its null user).

const EVIL_ORIGIN = "https://evil.example"
const OWN_ORIGIN = server.url.replace(/\/+$/, "")
const HOST = server.host.includes(":") ? `[${server.host}]` : server.host
const OTHER_PORT_ORIGIN = `${server.secure ? "https" : "http"}://${HOST}:${server.port == 65535 ? 65534 : server.port + 1}`
const OTHER_SCHEME_ORIGIN = `${server.secure ? "http" : "https"}://${HOST}:${server.port}`

// The account a proof signs in with: the run's (--param user=NAME, its password in MOQUI_TEST_PASSWORD), else in the
// fixtures profile the demo data's john.doe.
const account = () => {
    if (params.user) return { username: params.user, password: params.password ?? "" }
    needsFixtures("an account to sign in with (or --param user=NAME and MOQUI_TEST_PASSWORD)")
    return { username: "john.doe", password: "moqui" }
}

// test_a01_ws.py: test_websocket_evil_origin_is_rejected
// Does not prove: /xmpp-websocket, off by default, which checks no Origin; nor the origins a deployment lists.
feature("v5.0.0-4.4.2: a /notws upgrade from a foreign Origin is refused, the own origin opens", {
    when() {
        const { username, password } = account()
        const s = session({ user: username, password })
        const evil = ws.connect("/notws", { session: s, origin: EVIL_ORIGIN })
        const evilOpened = evil.open
        evil.close()
        // the control: the same session opens it from the server's own origin
        const own = ws.connect("/notws", { session: s, origin: OWN_ORIGIN })
        const ownOpened = own.open
        own.close()
        return { evil, evilOpened, own, ownOpened }
    },
    then({ evil, evilOpened, own, ownOpened }) {
        !evilOpened
        evil.status == 403
        ownOpened
        own.status == 101
    },
})

// Not in Java, which compares the host only and so opens these.
// Does not prove: /termws and /callws, which share the check in code.
feature("v5.0.0-4.4.2: a /notws upgrade from the server's host on #which is refused", {
    where: [
        { which: "another port", origin: OTHER_PORT_ORIGIN },
        { which: "another scheme", origin: OTHER_SCHEME_ORIGIN },
    ],
    when({ origin }) {
        const { username, password } = account()
        const socket = ws.connect("/notws", { session: session({ user: username, password }), origin })
        const opened = socket.open
        socket.close()
        return { socket, opened }
    },
    then({ socket, opened }) {
        !opened
        socket.status == 403
    },
})

// Replaces test_a01_ws.py: test_notws_anonymous_upgrade_accepts_or_closes (Java accepts it). The control, a signed-in
// session opening it, is the first proof above.
// Does not prove: /callws, whose guest credential is checked in-process, nor the recheck of a socket already open.
feature("v5.0.0-4.4.4: a /notws upgrade without a signed-in session is refused", {
    when() {
        const socket = ws.connect("/notws", { session: session() })
        const opened = socket.open
        socket.close()
        return { socket, opened }
    },
    then({ socket, opened }) {
        !opened
        socket.status == 401
    },
})
