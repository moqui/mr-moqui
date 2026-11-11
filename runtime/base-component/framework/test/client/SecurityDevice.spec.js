// The shared-device credential over HTTP (docs/security/Shared-Devices.md; crates/moqui-core/src/user/device.rs): a
// registration code makes one browser the device, once, with a cookie no script reads; the device opens only its route
// group, signs in as no one, reads no key, and its web session holds no user; a forged or revoked device cookie is refused
// and removed. The fixtures are test/ClientFixtures.spec.js's (a code and a secret known to the tests, stored only as
// hashes). The rules of registering are the server spec DeviceCredential's. No Java counterpart.

const CODE = "secd-evic-ecod-e234-5678"
const REVOKED_SECRET = "sec-device-fixed-secret-of-43-characters-00"
const refused = (res) => res.status == 403
let device

feature("a registration code makes one browser the device, once, with an HttpOnly SameSite=Strict cookie", {
    when() {
        device = session()
        device.get("/Device")
        const redeemed = device.post("/Device/redeem", { form: { code: CODE } })
        const other = session()
        other.get("/Device")
        return { redeemed, again: other.post("/Device/redeem", { form: { code: CODE } }) }
    },
    then({ redeemed, again }) {
        const cookie = redeemed.cookie("moqui.device")
        cookie != null && cookie.httpOnly && cookie.sameSite == "Strict" && cookie.path == "/"
        cookie.secure == server.secure
        cookie.value.startsWith("SEC_DEV_1.") && !cookie.value.includes("secd")
        redeemed.status == 302 && String(redeemed.location).endsWith("/apps/AppList")
        again.cookie("moqui.device") == null
    },
})

feature("the device opens only its route group, signs in as no one, and its session holds no user", {
    when() {
        const sessionCookie = device.cookies().find((cookie) => cookie.name == "moqui.session")
        return {
            home: device.get("/apps/AppList"),
            page: device.get("/Device"),
            other: device.get("/apps/ScreenTree"),
            preferences: device.get("/apps/getPreferences"),
            rest: device.get("/rest/s1/moqui/basic/geos/USA", { loginKey: "sec-test-login-key-fixed-40-chars-value", accept: "json" }),
            login: device.post("/rest/login", { form: { username: params.user ?? "john.doe", password: params.password ?? "moqui" }, accept: "json", csrf: false }),
            after: device.get("/apps/ScreenTree"),
            // the browser's session cookie alone, without the device cookie: nobody is signed in on it
            sessionOnly: session().get("/apps/AppList", { headers: { Cookie: "moqui.session=" + (sessionCookie ? sessionCookie.value : "") } }),
            socket: ws.connect("/notws", { session: device, timeout: 5 }),
        }
    },
    then({ home, page, other, preferences, rest, login, after, sessionOnly, socket }) {
        home.status == 200
        page.status == 200 && page.text.includes("registered as Security test wall")
        refused(other) && refused(preferences) && refused(rest) && refused(login) && refused(after)
        other.text.includes("This device opens only SEC_DEV_WALLS")
        sessionOnly.status != 200
        !socket.open
    },
})

feature("a forged or revoked device cookie is refused and removed (#name)", {
    where: [
        { name: "forged", cookie: "SEC_DEV_1." + "x".repeat(43), text: "not a registered device" },
        { name: "revoked", cookie: "SEC_DEV_REVOKED." + REVOKED_SECRET, text: "registration was revoked" },
    ],
    when({ cookie }) {
        return { res: session().get("/apps/AppList", { headers: { Cookie: "moqui.device=" + cookie } }) }
    },
    then({ res, text }) {
        res.status == 403
        res.text.includes(text)
        res.cookie("moqui.device") != null && res.cookie("moqui.device").maxAge == 0
        res.cookie("moqui.session") == null
    },
})

feature("a device's daily new secret reaches its browser on any answer it may keep, a refusal too", {
    when() {
        const res = session().get("/apps/ScreenTree", { headers: { Cookie: "moqui.device=SEC_DEV_ROTATE." + REVOKED_SECRET } })
        const renewed = res.cookie("moqui.device")
        return { res, renewed, next: renewed && session().get("/apps/AppList", { headers: { Cookie: "moqui.device=" + renewed.value } }) }
    },
    then({ res, renewed, next }) {
        res.status == 403
        renewed != null && renewed.httpOnly && renewed.value.startsWith("SEC_DEV_ROTATE.") && !renewed.value.endsWith(REVOKED_SECRET)
        next.status == 200
    },
})
