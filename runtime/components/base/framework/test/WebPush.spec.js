// Web Push subscriptions (org.moqui.ui.NotificationServices get#WebPushStatus, store#, delete#WebPushSubscription and
// send#WebPush; service/web_push.rs; plans/archive/Comms-Framework.md §7.2 P0): the status gives the VAPID key; a subscription
// is the signed-in user's only, moves when another user signs in on the same browser, and is deleted only by its user;
// keys are checked; a failing push counts against the subscription and disables it after 5.
// Run: ./moq test framework --spec WebPush
const SUBSCRIPTION = "moqui.security.user.WebPushSubscription"
const OTHER_USER = "WEBPUSH_SPEC_OTHER"
// RFC 8291 Appendix A's user agent keys: a valid P-256 point and a 16-byte secret
const P256DH = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"
const AUTH = "BTBZMqHH6r4Tts7J_aSIgg"
// .invalid never resolves (RFC 2606), so a push to it fails at once without the network
// john.doe's userId
const JOHN = "EX_JOHN_DOE"
const ENDPOINT = "https://push.webpush-spec.invalid/send/one"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
// as the /WebPush transitions call them: authorization off, as each works on the signed-in user's rows only
function attempt(name, parameters) {
    let out = {}
    let thrown = ""
    try {
        out = sys(() => call("org.moqui.ui.NotificationServices." + name, parameters)) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const row = endpoint => sys(() => ec.entity.find(SUBSCRIPTION).condition("endpoint", endpoint).one())
const store = endpoint => attempt("store#WebPushSubscription", {endpoint, p256dhKey: P256DH, authSecret: AUTH})

spec({
    requires: ["web-push"],
    setupSpec() {
        sys(() => {
            if (ec.entity.find("moqui.security.UserAccount").condition("userId", OTHER_USER).count() == 0) {
                const account = ec.entity.makeValue("moqui.security.UserAccount")
                account.setFields(
                    {userId: OTHER_USER, username: "webpush.spec.other", userFullName: "Push Spec"},
                    true,
                    null,
                    null
                )
                account.create()
            }
        })
    },
    setup() {
        sys(() => ec.entity.find(SUBSCRIPTION).condition("endpoint", "like", "https://push.webpush-spec.invalid/%").deleteAll())
        login("john.doe")
    },
    cleanupSpec() {
        sys(() => ec.entity.find(SUBSCRIPTION).condition("endpoint", "like", "https://push.webpush-spec.invalid/%").deleteAll())
    }
})

feature("the status gives the VAPID public key and whether this browser is subscribed", {
    when() {
        const before = attempt("get#WebPushStatus", {endpoint: ENDPOINT})
        const stored = store(ENDPOINT)
        const after = attempt("get#WebPushStatus", {endpoint: ENDPOINT})
        return {before, stored, after}
    },
    then({before, stored, after}) {
        before.supported == true
        before.configured == true
        // an uncompressed P-256 point, base64url: 65 bytes in 87 characters
        before.publicKey.length == 87
        before.subscribed == false
        stored.errors == ""
        stored.subscriptionId != null
        after.subscribed == true
        row(ENDPOINT).userId == JOHN
    }
})

feature("a subscription with keys that are not a P-256 point and a 16-byte secret, or an http endpoint, is refused", {
    when() {
        return {
            badPoint: attempt("store#WebPushSubscription", {endpoint: ENDPOINT, p256dhKey: "BAAA", authSecret: AUTH}),
            badAuth: attempt("store#WebPushSubscription", {endpoint: ENDPOINT, p256dhKey: P256DH, authSecret: "AAAA"}),
            http: attempt("store#WebPushSubscription", {
                endpoint: "http://push.webpush-spec.invalid/x",
                p256dhKey: P256DH,
                authSecret: AUTH
            }),
            kept: row(ENDPOINT)
        }
    },
    then({badPoint, badAuth, http, kept}) {
        badPoint.errors.includes("p256dh")
        badAuth.errors.includes("auth")
        http.errors.includes("https")
        kept == null
    }
})

feature("another user can neither see nor delete a subscription, and signing in on the browser moves it to them", {
    when() {
        store(ENDPOINT)
        ec.user.internalLoginUser("webpush.spec.other")
        const otherSees = attempt("get#WebPushStatus", {endpoint: ENDPOINT}).subscribed
        // the same parameters the owner deletes with
        attempt("delete#WebPushSubscription", {endpoint: ENDPOINT})
        const afterOtherDelete = row(ENDPOINT)
        const moved = store(ENDPOINT)
        const movedRow = row(ENDPOINT)
        login("john.doe")
        const johnSees = attempt("get#WebPushStatus", {endpoint: ENDPOINT}).subscribed
        return {otherSees, afterOtherDelete, moved, movedRow, johnSees}
    },
    then({otherSees, afterOtherDelete, moved, movedRow, johnSees}) {
        otherSees == false
        afterOtherDelete != null
        afterOtherDelete.userId == JOHN
        moved.errors == ""
        movedRow.userId == OTHER_USER
        johnSees == false
    }
})

feature("the owner deletes a subscription", {
    when() {
        store(ENDPOINT)
        const deleted = attempt("delete#WebPushSubscription", {endpoint: ENDPOINT})
        return {deleted, after: row(ENDPOINT), status: attempt("get#WebPushStatus", {endpoint: ENDPOINT})}
    },
    then({deleted, after, status}) {
        deleted.errors == ""
        after == null
        status.subscribed == false
    }
})

feature("nobody signed in cannot store a subscription", {
    when() {
        ec.user.logoutUser()
        const out = store(ENDPOINT)
        login("john.doe")
        return {out, after: row(ENDPOINT)}
    },
    then({out, after}) {
        out.errors.includes("logged in") || out.errors.includes("Sign in")
        after == null
    }
})

feature("a push that fails counts against the subscription, and the fifth failure in a row disables it", {
    when() {
        store(ENDPOINT)
        const first = sys(() => call("org.moqui.ui.NotificationServices.send#WebPush", {userIdList: [JOHN], title: "Hi"}))
        const counted = row(ENDPOINT)
        sys(() => {
            const value = ec.entity.find(SUBSCRIPTION).condition("endpoint", ENDPOINT).one()
            value.failureCount = 4
            value.update()
        })
        sys(() => call("org.moqui.ui.NotificationServices.send#WebPush", {userIdList: [JOHN], title: "Hi"}))
        const disabled = row(ENDPOINT)
        const status = attempt("get#WebPushStatus", {endpoint: ENDPOINT})
        // a disabled subscription is not sent to
        const none = sys(() => call("org.moqui.ui.NotificationServices.send#WebPush", {userIdList: [JOHN], title: "Hi"}))
        // the browser storing it again enables it
        store(ENDPOINT)
        return {first, counted, disabled, status, none, again: row(ENDPOINT)}
    },
    then({first, counted, disabled, status, none, again}) {
        first.failedCount == 1
        first.sentCount == 0
        counted.failureCount == 1
        counted.lastFailureDate != null
        counted.disabledDate == null
        disabled.failureCount == 5
        disabled.disabledDate != null
        status.subscribed == false
        none.failedCount == 0
        again.disabledDate == null
        again.failureCount == 0
    }
})
