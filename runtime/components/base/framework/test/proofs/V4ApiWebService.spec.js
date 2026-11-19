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

// Configuration in the matrix: the pages open wss when served over https, and plain http serves ws, so a deployment
// meets it by serving nothing over plain http but a redirect (12.2.1). This checks a deployment over https
// (--server https://...; its plain-http address --param httpUrl=http://host[:port], else port 80) and skips against any
// other. A deployment that serves the app over plain http answers the upgrade 401 or opens it, and fails.
// Does not prove: /callws and /termws, which the same listener serves.
feature("v5.0.0-4.4.1: the plain-http address takes no WebSocket upgrade", {
    given() { if (!server.secure) skip("a deployment's proof over https: run it with --server https://<host>") },
    when() {
        const httpUrl = params.httpUrl ?? `http://${server.host.includes(":") ? `[${server.host}]` : server.host}`
        let socket = null
        let refused = null
        try {
            socket = ws.connect(httpUrl + "/notws", { session: session() })
            socket.close()
        } catch (e) {
            refused = String(e && e.message ? e.message : e)
        }
        return { socket, refused }
    },
    then({ socket, refused }) {
        refused != null || (!socket.open && [301, 302, 307, 308].includes(socket.status))
    },
})

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

const SESSION_COOKIE = server.secure ? "__Host-moqui.session" : "moqui.session"

// Configuration in the matrix: webapp-list/webapp[webroot]@client-ip-header names the header a proxy in front sets, and
// with @trusted-proxies the forwarding headers (the client address, X-Forwarded-Proto, -Host and -Port) are believed only
// from those proxies; by default none is believed. So the forwarding headers a client sends itself must not change what the
// server takes: a scheme it names changes neither the session cookie (__Host- and Secure over https, neither over http)
// nor HSTS, and a host it names is not taken as the server's own origin. Against a deployment behind a proxy it fails
// where the proxy passes a client's header on to a server that believes it.
// Does not prove: the client's address (15.3.4's proofs), nor a header an application reads itself.
feature("v5.0.0-4.1.3: a client's own forwarding headers change neither its scheme nor its host", {
    when() {
        const flipped = server.secure ? "http" : "https"
        const evil = `${server.secure ? "https" : "http"}://evil.example`
        return {
            plain: session().get("/Login"),
            scheme: session().get("/Login", {
                headers: { "X-Forwarded-Proto": flipped, "X-Forwarded-Ssl": server.secure ? "off" : "on", Forwarded: `proto=${flipped}` },
            }),
            host: session().get("/Login", {
                headers: {
                    Origin: evil, "X-Forwarded-Host": "evil.example", "X-Forwarded-Port": server.secure ? "443" : "80",
                    "X-Forwarded-Server": "evil.example", Forwarded: "host=evil.example",
                },
            }),
            own: session().get("/Login", { headers: { Origin: OWN_ORIGIN } }),
        }
    },
    then({ plain, scheme, host, own }) {
        plain.status == 200
        scheme.status == 200
        // the session cookie keeps the name and flags of the request's own scheme, and HSTS comes only over https
        plain.cookie(SESSION_COOKIE) != null
        scheme.cookie(SESSION_COOKIE) != null
        scheme.cookie(SESSION_COOKIE).secure == plain.cookie(SESSION_COOKIE).secure
        (scheme.header("Strict-Transport-Security") == null) == (plain.header("Strict-Transport-Security") == null)
        host.status == 401
        host.header("Access-Control-Allow-Origin") == null
        // the control: the server's own origin is taken
        own.status == 200
    },
})

// A header naming a user, as a proxy that signs people in sets (X-Remote-User, X-User-ID), signs nobody in: the framework
// takes a user only from its own session, a key or Basic credentials.
// Does not prove: a proxy sign-in an application adds itself.
feature("v5.0.0-4.1.3: a header naming a user signs nobody in; the user's own session does", {
    when() {
        const { username, password } = account()
        const headers = {}
        for (const name of ["X-Remote-User", "Remote-User", "X-Forwarded-User", "X-User-ID", "X-Auth-Request-User",
            "X-Authenticated-User", "X-Forwarded-Preferred-Username"]) headers[name] = username
        return { named: session().get("/qapps", { headers }), own: session({ user: username, password }).get("/qapps") }
    },
    then({ named, own }) {
        named.status == 302
        (named.location ?? "").includes("/Login")
        // the control: the user's own session opens the shell
        own.status == 200
    },
})

// Each request is sent raw on a connection of its own to the internal error page, which shows the errorMessage of the body
// it read, so its answer says how the server framed the body. Content-Length alone and chunked coding alone are read
// (and a short Content-Length is taken as given, so the page shows less); given both, in either order, the chunks are read
// and the Content-Length ignored, also one longer than the body, which a server reading it would wait on; two lengths
// that differ, a coding that does not end in chunked and chunked coding in HTTP/1.0 are refused (or, through a proxy that
// drops the header, never read as the body).
// Does not prove: HTTP/2 framing (a Content-Length against the DATA frames), nor how a proxy in front frames what it
// forwards, which is the deployment's; through one these answers are its own.
feature("v5.0.0-4.2.1: a body is framed by its chunks when lengths disagree; malformed framing is refused", {
    when() {
        const s = session()
        s.get("/Login")
        const marker = `PROOF${Date.now()}`
        const body = `errorMessage=${marker}`
        const chunks = `${body.length.toString(16)}\r\n${body}\r\n0\r\n\r\n`
        const port = server.port == (server.secure ? 443 : 80) ? "" : `:${server.port}`
        const host = (server.host.includes(":") ? `[${server.host}]` : server.host) + port
        const send = (framing, payload, version) => {
            try {
                const res = s.raw(`POST /error/InternalError HTTP/${version ?? "1.1"}\r\nHost: ${host}\r\nX-CSRF-Token: ${s.token()}\r\n` +
                    `Content-Type: application/x-www-form-urlencoded\r\n${framing}Connection: close\r\n\r\n${payload}`, { timeout: 5 })
                return { status: res.status, echo: res.text.includes(marker) }
            } catch (err) {
                return { status: 0, echo: false, error: String(err.message ?? err) }
            }
        }
        return {
            length: send(`Content-Length: ${body.length}\r\n`, body),
            short: send("Content-Length: 5\r\n", body),
            chunked: send("Transfer-Encoding: chunked\r\n", chunks),
            both: [
                send("Content-Length: 5\r\nTransfer-Encoding: chunked\r\n", chunks),
                send("Transfer-Encoding: chunked\r\nContent-Length: 5\r\n", chunks),
                send("Content-Length: 200\r\nTransfer-Encoding: chunked\r\n", chunks),
            ],
            malformed: [
                send(`Content-Length: ${body.length}\r\nContent-Length: 5\r\n`, body),
                send("Transfer-Encoding: chunked, identity\r\n", chunks),
                send("Transfer-Encoding: xchunked\r\n", chunks),
                send("Transfer-Encoding : chunked\r\n", chunks),
                send("Transfer-Encoding: chunked\r\n", chunks, "1.0"),
            ],
        }
    },
    then({ length, short, chunked, both, malformed }) {
        // the controls: each framing alone is read as it says
        length.status == 200 && length.echo
        chunked.status == 200 && chunked.echo
        short.status == 200 && !short.echo
        both.every((res) => res.status == 400 || (res.status == 200 && res.echo))
        malformed.every((res) => res.status == 400 || (res.status > 0 && !res.echo))
    },
})
