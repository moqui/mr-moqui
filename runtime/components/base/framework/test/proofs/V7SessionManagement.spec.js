// OWASP ASVS 5.0.0 V7 Session Management: the framework's proofs, each feature filed under the requirement its name
// starts with (docs/security/proofs/V7-Session-Management.md).

// The account a proof signs in with: the one the run's params give (--param user=NAME, with the password in
// MOQUI_TEST_PASSWORD so that it stays out of ps), else in the fixtures profile ClientFixtures' sec.view.only.
const account = () => {
    if (params.user) return { username: params.user, password: params.password ?? "" }
    needsFixtures("an account to sign in with (or --param user=NAME and MOQUI_TEST_PASSWORD)")
    return { username: "sec.view.only", password: "SecView1!!" }
}
// Whether an answer sends the browser to the sign-in page, as it does a request with no signed-in session.
const toSignIn = (res) => res.status == 302 && (res.location ?? "").includes("/Login")

const SESSION_COOKIE = server.secure ? "__Host-moqui.session" : "moqui.session"
// The cookie of a session with `cookies`, as a Cookie header.
const cookieHeader = (cookies) => cookies.map((c) => `${c.name}=${c.value}`).join("; ")

// The session cookie is the session id and a MAC of it under the server's key; a cookie whose MAC or id was changed is
// no session.
// Does not prove: the MAC's algorithm and key (HMAC-SHA256, in-process: session_tests.rs a_bad_mac_is_an_unknown_session).
feature("v5.0.0-7.2.1: a session cookie changed by the client signs nobody in", {
    when() {
        const { username, password } = account()
        const s = session()
        const signIn = s.formLogin(username, password)
        assert(signIn.status == 302 && !toSignIn(signIn), `the sign-in of ${username} did not succeed: HTTP ${signIn.status}`)
        const jar = s.cookies()
        const value = (jar.find((c) => c.name == SESSION_COOKIE) ?? {}).value ?? ""
        assert(value.includes("."), `no session cookie ${SESSION_COOKIE} after the sign-in`)
        const [id, mac] = [value.slice(0, value.lastIndexOf(".")), value.slice(value.lastIndexOf(".") + 1)]
        const flip = (text) => text.slice(0, -1) + (text.slice(-1) == "A" ? "B" : "A")
        const others = jar.filter((c) => c.name != SESSION_COOKIE)
        const withValue = (v) => cookieHeader(others.concat([{ name: SESSION_COOKIE, value: v }]))
        const get = (v) => session({ headers: { Cookie: withValue(v) } }).get("/apps/tools/Home")
        return { same: get(value), badMac: get(`${id}.${flip(mac)}`), badId: get(`${flip(id)}.${mac}`) }
    },
    then({ same, badMac, badId }) {
        // the control: the cookie as the server set it is signed in
        !toSignIn(same)
        toSignIn(badMac)
        toSignIn(badId)
    },
})

// The server makes every session id itself: a session cookie a client makes up is not taken, and the server sets its
// own.
// Does not prove: the generator's randomness, which is 7.2.3's and in-process.
feature("v5.0.0-7.2.2: a session cookie the client makes up is not taken; the server issues its own", {
    when() {
        const made = "0123456789abcdef0123456789abcdef01234567.bWFkZS11cA"
        const res = session({ headers: { Cookie: `${SESSION_COOKIE}=${made}` } }).get("/Login")
        return { made, cookie: res.cookie(SESSION_COOKIE) }
    },
    then({ made, cookie }) {
        cookie != null
        cookie.value != made
        !cookie.value.startsWith(made.split(".")[0])
    },
})

// Does not prove: that the ids come from a cryptographic generator (getrandom, in-process); it shows their length and
// that two are never the same.
feature("v5.0.0-7.2.3: a session id is at least 128 bits, a new one for each session", {
    when() {
        const ids = [0, 1, 2].map(() => {
            const cookie = session().get("/Login").cookie(SESSION_COOKIE)
            return cookie == null ? "" : cookie.value.slice(0, cookie.value.lastIndexOf("."))
        })
        return { ids }
    },
    then({ ids }) {
        // 32 hex digits are 128 bits
        ids.every((id) => /^[0-9a-f]{32,}$/i.test(id))
        new Set(ids).size == 3
    },
})

// Does not prove: that a session ends when it idles or when the account is disabled (V7.3, in-process); it signs one
// session out with the REST logout and replays its cookies.
feature("v5.0.0-7.4.1: a signed-out session's cookies do not sign in again", {
    when() {
        const { username, password } = account()
        const s = session()
        const signIn = s.formLogin(username, password)
        assert(signIn.status == 302 && !toSignIn(signIn), `the sign-in of ${username} did not succeed: HTTP ${signIn.status}`)
        const cookie = s.cookies().map((c) => `${c.name}=${c.value}`).join("; ")
        const before = s.get("/apps/tools/Home")
        s.logout()
        const replayed = session({ headers: { Cookie: cookie } }).get("/apps/tools/Home")
        return { before, replayed }
    },
    then({ before, replayed }) {
        // the control: before the logout the same request was signed in
        !toSignIn(before)
        toSignIn(replayed)
    },
})
