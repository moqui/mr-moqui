// Native push registrations (org.moqui.ui.NotificationServices register#PushDevice, unregister#PushDevice, the
// /qapps/registerClientPush and unregisterClientPush transitions; service/web_push.rs and service/fcm.rs;
// plans/Mobile-Desktop.md §5): only a request signed in by a device key registers, the registration is tied to that key
// (revoking the key deletes it, renewing moves it, an ended key's goes at the next push), and send#WebPush sends it to
// FCM as a data message, a ring with high priority, disabling a token FCM answers UNREGISTERED.
// Run: ./moq test framework --spec PushDevice
const SUBSCRIPTION = "moqui.security.user.WebPushSubscription"
const LOGIN_KEY = "moqui.security.UserLoginKey"
const WEBROOT = "component://webroot/screen/webroot.xml"
// john.doe's userId
const JOHN = "EX_JOHN_DOE"
const OTHER_USER = "PUSHDEVICE_SPEC_OTHER"
const TOKEN_PREFIX = "pushdevice-spec-"
const TOKEN_URI = "https://oauth2.pushdevice-spec.invalid/token"
const FCM_BASE = "https://fcm.pushdevice-spec.invalid"
const SEND_URL = FCM_BASE + "/v1/projects/demo-project/messages:send"
// the QZ signer's RSA test key (crates/moqui-lib/testdata/qz-test-key-pkcs8.pem)
const KEY = [
    "-----BEGIN PRIVATE KEY-----",
    "MIIEvwIBADANBgkqhkiG9w0BAQEFAASCBKkwggSlAgEAAoIBAQC6yj4GKhXZtCnO",
    "zLOt9v7sjrio4UQDcdNIPt2+Ve8kmI8Aw5N5axIh0h1jy6pltJrjlqITRk0EgQCm",
    "qHVJ7nF1Jiu81x3uA7TN07ZmOXFzvj+MA5n9v2I6PAoWd5X3C43mtYWj24eq8Dp1",
    "uZsxroZbhZPt8vp+VLesH3uGaCD4LNJkQndape6Wr3zhDC1qpExiAoePzR0edCgY",
    "7d7E6aWI7JNyEoiSZz6kMCQ+ICETdf4s6xtRfVu0kTA1SzHMB1kW9E1z25XlsKe/",
    "8/W+wK0hLhX7O8qu6Ks908iYJbw9ksqX+Tpn4bh/RIDdj0gyT5Nm9bqFWjlBFLKt",
    "0P8uE9CJAgMBAAECggEAAK6v9XxOX6D6R8nffIAgPsjSW7WU9saXQmvjtKgM4K9D",
    "qHmjw91fBStxauBUg0dwCxWitRt8OQsSmklCbRkh+3mpTmjicFs9nBniUEh0mxBj",
    "xt/AKS6Q4zcTGga+UBclTHrhyISLCpYS6e6vZon6w4H/ABnI41DCagPPGEwwQKUs",
    "V4+asqkZou3l5Y0tfHgVA6j0RKURIQiiq9eNK1yP9eFcHPeh+J6xhgLFpKj5pSW3",
    "OPIvsj5nCRGLbl+MT6GUKg6iyKMQEoNYY/ITRL2+YAOaRBOkb17zUCzeCwYPWYo7",
    "ZIA5ZlnA7J2GARd0O2NOgnVrFZ1d2mSr9wf5gkZTDQKBgQDcVfmICi2d891BbjqJ",
    "kkNJ6vAQHJ6K94U67BReoSJU+L6mg/4+dNrha6PHMbSXeeN/ikqhBnN8q7bG57sH",
    "kB1xnOc10vkkOOh7fg0k5SI2FInZlJZ9gA4EgjMBMKZhOxBU9CdSBqQuAlSUi4Tb",
    "Xszaw7yLjpqcU0IbI2eZ0CKHDQKBgQDZBjvTrP/LTYKrPaHtpVZsglVIkFFL6yDI",
    "2CO2lkxzSVrAprXQe9L16E4Azs+LQEJZkpipmAJa7BNZnn48Zek7W5zPsigrBbVL",
    "7205p/loEkdx8JcIM4Vb1spWAB01H0rXOoz1CruwDW7cEcBMvTFjb7VZsv3X0TF2",
    "cZ2ods2QbQKBgQCNDm1lbj6t9EUdclf/Yqe5VrLBXdER3kxo4GJaAoZcJ7LGH5JQ",
    "obbOwX5kM6oNn3bJW9tTIzIkCjpFNG1D9jsjVPOgPaRKUDRletknWZmBTbzCkqGV",
    "TrmqoT/N7ztU298kw/m4RZAt9Ef5oSg42IQFOXInVfAdTB/yB8R3cAvqsQKBgQC8",
    "1W+4/w6TFx5bYYVEAljwkXcjbG9QX4ZnO6nXdW+P4IPWnTgV76ff0AIPMePkhkU1",
    "Kxkqd9mUw1US5ZVxeDs4Kr6G+b15sb3OFMY1JLMemskPmwfPWwTupDXqHq9LSa8V",
    "Fx0PGge1Z/rOAaQRIY2Xt9tT8SpuXtdq9/D2ZGj5AQKBgQCcEu99mS2RSrYxH96m",
    "kNC7iI1zpD4MebsRpjJbdePSRQ7j5rHn9mUcW2GaJukwfxJGB6y7fG1enonlu9EG",
    "MjrP3noOGYsuPt3d2uToofwIo+ee9GPwKb42r9/1U9w+afqA6kU076Bgqawy4LaJ",
    "aIuYpf4C2n/KY+x6AQx+tUGJyA==",
    "-----END PRIVATE KEY-----",
    ""
].join("\n")
const ACCOUNT = JSON.stringify({
    type: "service_account",
    project_id: "demo-project",
    client_email: "push@demo-project.iam.gserviceaccount.com",
    private_key: KEY,
    token_uri: TOKEN_URI
})
const http = ec.factory.getTool("HttpFixtures")

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const keyHashes = () =>
    sys(() =>
        ec.entity
            .find(LOGIN_KEY)
            .condition("userId", JOHN)
            .list()
            .map(row => row.loginKey)
    )
// A key of john's as the REST login's createLoginKey with deviceName makes it: the key, and the hash that is stored.
function deviceKey(deviceName) {
    const before = keyHashes()
    const key = ec.user.getLoginKey()
    const hash = keyHashes().find(found => !before.includes(found))
    if (deviceName) {
        sys(() => {
            const row = ec.entity.find(LOGIN_KEY).condition("loginKey", hash).one()
            row.deviceName = deviceName
            row.update()
        })
    }
    return {key, hash}
}
// A POST to a /qapps transition with the device's key in its login_key header, as the app sends it: the JSON answer.
function post(transition, parameters, key) {
    let test = ec.screen.makeTest().rootScreen(WEBROOT)
    if (key) test = test.header("login_key", key)
    const str = test.render("qapps/" + transition, parameters || {}, "post")
    ec.message.clearErrors()
    let json = null
    try {
        json = JSON.parse(String(str.output || ""))
    } catch (err) {
        json = null
    }
    return {status: str.status, json, text: String(str.output || "")}
}
const register = (token, key) => post("registerClientPush", {provider: "fcm", token, appId: "org.moqui.mobile", platform: "android"}, key)
const row = token => sys(() => ec.entity.find(SUBSCRIPTION).condition("endpoint", token).one())
const sendPush = parameters =>
    sys(() => call("org.moqui.ui.NotificationServices.send#WebPush", Object.assign({userIdList: [JOHN], title: "Hi"}, parameters)))
const fcmAnswer = (status, body) => ({method: "POST", url: SEND_URL, status, contentType: "application/json", body: JSON.stringify(body)})
const tokenAnswer = {
    method: "POST",
    url: TOKEN_URI,
    status: 200,
    contentType: "application/json",
    body: JSON.stringify({access_token: "ya29.spec", expires_in: 3599, token_type: "Bearer"})
}

spec({
    setupSpec() {
        sys(() => {
            if (ec.entity.find("moqui.security.UserAccount").condition("userId", OTHER_USER).count() == 0) {
                const account = ec.entity.makeValue("moqui.security.UserAccount")
                account.setFields({userId: OTHER_USER, username: "pushdevice.spec.other", userFullName: "Push Device Spec"}, true, null, null)
                account.create()
            }
        })
    },
    setup() {
        sys(() => ec.entity.find(SUBSCRIPTION).condition("endpoint", "like", TOKEN_PREFIX + "%").deleteAll())
        login("john.doe")
    },
    cleanup() {
        http.reset()
    },
    cleanupSpec() {
        sys(() => ec.entity.find(SUBSCRIPTION).condition("endpoint", "like", TOKEN_PREFIX + "%").deleteAll())
    }
})

feature("an app signed in with its device key registers its FCM token, tied to the key, and a new token replaces the old", {
    when() {
        const phone = deviceKey("Push spec phone")
        const first = register(TOKEN_PREFIX + "one", phone.key)
        const stored = row(TOKEN_PREFIX + "one")
        const config = post("clientConfig", {}, phone.key).json
        const second = register(TOKEN_PREFIX + "two", phone.key)
        return {phone, first, stored, config, second, old: row(TOKEN_PREFIX + "one"), now: row(TOKEN_PREFIX + "two")}
    },
    then({phone, first, stored, config, second, old, now}) {
        first.status == 200
        first.json.registered == true
        stored.userId == JOHN
        stored.pushProviderEnumId == "PushFcm"
        stored.loginKey == phone.hash
        stored.appId == "org.moqui.mobile"
        stored.platform == "android"
        stored.visitId == null
        config.server.features.nativePush.providers.includes("fcm")
        config.server.features.nativePush.registered == true
        second.status == 200
        old == null
        now.loginKey == phone.hash
    }
})

feature("a request not signed in by a device key of the user's cannot register", {
    when() {
        const plain = deviceKey(null)
        return {
            session: register(TOKEN_PREFIX + "x", null),
            plainKey: register(TOKEN_PREFIX + "x", plain.key),
            stored: row(TOKEN_PREFIX + "x"),
            // and the same request with a device key does
            device: register(TOKEN_PREFIX + "x", deviceKey("Push spec tablet").key)
        }
    },
    then({session, plainKey, stored, device}) {
        session.status == 400
        session.text.includes("device key")
        plainKey.status == 400
        stored == null
        device.status == 200
    }
})

feature("unregistering forgets the device's token, and another user cannot forget it", {
    when() {
        const phone = deviceKey("Push spec phone")
        register(TOKEN_PREFIX + "one", phone.key)
        ec.user.internalLoginUser("pushdevice.spec.other")
        const other = sys(() => call("org.moqui.ui.NotificationServices.unregister#PushDevice", {token: TOKEN_PREFIX + "one"}))
        const kept = row(TOKEN_PREFIX + "one")
        login("john.doe")
        const own = post("unregisterClientPush", {}, phone.key)
        return {other, kept, own, after: row(TOKEN_PREFIX + "one")}
    },
    then({other, kept, own, after}) {
        other.deletedCount == 0
        kept != null
        own.status == 200
        own.json.deletedCount == 1
        after == null
    }
})

feature("revoking the device key deletes its registration, and renewing the key moves it to the new key", {
    when() {
        const phone = deviceKey("Push spec phone")
        register(TOKEN_PREFIX + "phone", phone.key)
        const tablet = deviceKey("Push spec tablet")
        register(TOKEN_PREFIX + "tablet", tablet.key)
        sys(() => call("org.moqui.impl.UserServices.revoke#UserDeviceKey", {loginKey: phone.hash}))
        const renewed = post("renewClientDevice", {}, tablet.key)
        const renewedHash = keyHashes().find(hash => hash != tablet.hash && row(TOKEN_PREFIX + "tablet").loginKey == hash)
        return {phoneRow: row(TOKEN_PREFIX + "phone"), renewed, tabletRow: row(TOKEN_PREFIX + "tablet"), tablet, renewedHash}
    },
    then({phoneRow, renewed, tabletRow, tablet, renewedHash}) {
        phoneRow == null
        renewed.status == 200
        renewed.json.loginKey != null
        tabletRow != null
        tabletRow.loginKey != tablet.hash
        renewedHash == tabletRow.loginKey
    }
})

feature("a ring goes to FCM as a high-priority data message, and a token FCM answers UNREGISTERED is disabled", {
    when() {
        setProperty("fcm_service_account", ACCOUNT)
        setProperty("fcm_base_url", FCM_BASE)
        const phone = deviceKey("Push spec phone")
        register(TOKEN_PREFIX + "ring", phone.key)
        http.enable()
        http.add(tokenAnswer)
        http.add(fcmAnswer(200, {name: "projects/demo-project/messages/1"}))
        const ring = sendPush({
            title: "Call from Ann",
            link: "/qapps/comms/Calls?callSessionId=CS1",
            topic: "IncomingCall",
            tag: "call-CS1",
            ttlSeconds: 45,
            urgency: "high",
            callSessionId: "CS1",
            chatRoomId: "ROOM1",
            fromName: "Ann",
            video: true,
            expiresAt: 1800000045000
        })
        const sent = http.requests().find(request => request.url == SEND_URL)
        const delivered = row(TOKEN_PREFIX + "ring")
        http.clear()
        http.add(tokenAnswer)
        http.add(
            fcmAnswer(404, {
                error: {
                    code: 404,
                    status: "NOT_FOUND",
                    details: [{"@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError", errorCode: "UNREGISTERED"}]
                }
            })
        )
        const gone = sendPush({})
        const disabled = row(TOKEN_PREFIX + "ring")
        // a disabled registration is not sent to
        http.clear()
        const none = sendPush({})
        return {ring, sent: JSON.parse(sent.body), auth: sent.headers, delivered, gone, disabled, none, after: http.count()}
    },
    then({ring, sent, auth, delivered, gone, disabled, none, after}) {
        ring.sentCount == 1
        sent.message.token == TOKEN_PREFIX + "ring"
        sent.message.data.kind == "ring"
        sent.message.data.title == "Call from Ann"
        sent.message.data.callSessionId == "CS1"
        sent.message.data.chatRoomId == "ROOM1"
        sent.message.data.caller == "Ann"
        // FCM refuses a data message with a reserved key (from, message_type, notification, google.*, gcm.*)
        Object.keys(sent.message.data).every(key => !["from", "message_type", "notification"].includes(key) && !/^(google|gcm)\./.test(key))
        sent.message.data.video == "true"
        sent.message.data.expiresAt == "1800000045000"
        sent.message.android.priority == "HIGH"
        sent.message.android.ttl == "45s"
        Object.keys(auth).some(name => name.toLowerCase() == "authorization")
        delivered.lastSuccessDate != null
        gone.failedCount == 1
        disabled.disabledDate != null
        none.sentCount == 0
        none.failedCount == 0
        after == 0
    }
})

feature("a registration whose device key has ended is deleted at the next push, and nothing goes to FCM unconfigured", {
    when() {
        // the ring's feature set it for the rest of the spec
        setProperty("fcm_service_account", null)
        const phone = deviceKey("Push spec phone")
        register(TOKEN_PREFIX + "ended", phone.key)
        const tablet = deviceKey("Push spec tablet")
        register(TOKEN_PREFIX + "unset", tablet.key)
        // the key ends without the revoke service (it expired)
        sys(() => {
            const key = ec.entity.find(LOGIN_KEY).condition("loginKey", phone.hash).one()
            key.thruDate = ec.user.nowTimestamp
            key.update()
        })
        http.enable()
        const out = sendPush({})
        return {out, ended: row(TOKEN_PREFIX + "ended"), unset: row(TOKEN_PREFIX + "unset"), requests: http.count()}
    },
    then({out, ended, unset, requests}) {
        ended == null
        // fcm_service_account is not set in this feature: the device is left as it was, and nothing is sent
        unset != null
        unset.disabledDate == null
        unset.failureCount == 0
        out.sentCount == 0
        requests == 0
    }
})

feature("a ring's end goes to the apps only; a message FCM refuses, or an FCM outage, leaves every device as it is", {
    when() {
        setProperty("fcm_service_account", ACCOUNT)
        setProperty("fcm_base_url", FCM_BASE)
        register(TOKEN_PREFIX + "a", deviceKey("Push spec phone").key)
        register(TOKEN_PREFIX + "b", deviceKey("Push spec tablet").key)
        http.enable()
        http.add(tokenAnswer)
        http.add(fcmAnswer(200, {name: "projects/demo-project/messages/2"}))
        const ended = sendPush({kind: "ringEnded", reason: "answered", callSessionId: "CS1", tag: "call-CS1", ttlSeconds: 60, urgency: "high"})
        const endedBody = JSON.parse(http.requests().find(request => request.url == SEND_URL).body)
        // a 400 about the message, not the token: logged, and both are still tried and kept
        http.clear()
        http.add(tokenAnswer)
        http.add(
            fcmAnswer(400, {
                error: {code: 400, status: "INVALID_ARGUMENT", details: [{fieldViolations: [{field: "message.data[0].key"}]}]}
            })
        )
        const rejected = sendPush({})
        const rejectedTries = http.requests().filter(request => request.url == SEND_URL).length
        // FCM down: tried three times for the first device, the second not tried, neither counted against
        http.clear()
        http.add(tokenAnswer)
        http.add(fcmAnswer(503, {error: {code: 503, status: "UNAVAILABLE"}}))
        const down = sendPush({})
        const downTries = http.requests().filter(request => request.url == SEND_URL).length
        return {ended, endedBody, rejected, rejectedTries, down, downTries, a: row(TOKEN_PREFIX + "a"), b: row(TOKEN_PREFIX + "b")}
    },
    then({ended, endedBody, rejected, rejectedTries, down, downTries, a, b}) {
        ended.sentCount == 2
        endedBody.message.data.kind == "ringEnded"
        endedBody.message.data.callSessionId == "CS1"
        endedBody.message.data.reason == "answered"
        endedBody.message.android.priority == "HIGH"
        endedBody.message.android.ttl == "60s"
        rejected.failedCount == 2
        rejectedTries == 2
        down.failedCount == 2
        downTries == 3
        a.disabledDate == null
        a.failureCount == 0
        b.disabledDate == null
        b.failureCount == 0
    }
})

feature("a browser's subscription with no provider is still a Web Push subscription", {
    when() {
        const endpoint = "https://push.pushdevice-spec.invalid/send/legacy"
        sys(() => {
            ec.entity.find(SUBSCRIPTION).condition("endpoint", endpoint).deleteAll()
            const value = ec.entity.makeValue(SUBSCRIPTION)
            value.setFields({userId: JOHN, endpoint, endpointHash: "pushdevice-spec-legacy-hash"}, true, null, null)
            value.setSequencedIdPrimary()
            value.create()
        })
        const status = sys(() => call("org.moqui.ui.NotificationServices.get#WebPushStatus", {}))
        sys(() => ec.entity.find(SUBSCRIPTION).condition("endpoint", endpoint).deleteAll())
        return {status}
    },
    then({status}) {
        status.subscriptionCount >= 1
    }
})
