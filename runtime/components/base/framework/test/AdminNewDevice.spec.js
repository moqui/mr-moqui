// The new-device sign-in notice, org.moqui.admin.IdentityServices.notify#NewDeviceSignIn (mr-boss plan/impl-admin.md P10;
// design-admin §4 Sessions, F15): a successful sign-in's history row, written after the sign-in, sends one notice when its
// client (browser and system, and the network of its address) was not seen for the account within the window, and none for
// a known client, a failed sign-in or an account's first sign-in; the notice names the client and the network, never the
// user agent or the whole address, and goes to the account alone.
// Run: ./moq test framework --spec AdminNewDevice
const S = "org.moqui.admin.IdentityServices."
const VISIT = "moqui.server.Visit"
const HISTORY = "moqui.security.UserLoginHistory"
const userId = "SPEC_NDV_USER"
const freshId = "SPEC_NDV_FRESH"
const DAY = 86400000
const firefoxMac = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) Firefox/140.0"
const chromeWindows = "Mozilla/5.0 (Windows NT 10.0) Chrome/130.0"

const find = (entityName, conditions = {}) => ec.entity.find(entityName).condition(conditions).disableAuthz()
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const visitId = n => "SPEC_NDV_VISIT_" + n
const now = () => Date.now()
// a visit of the account from a client, some days ago
function visit(n, user, agent, address, daysAgo = 0) {
    make(VISIT, {
        visitId: visitId(n),
        userId: user,
        fromDate: new Date(now() - daysAgo * DAY),
        initialUserAgent: agent,
        clientIpAddress: address
    })
}
// the history row a sign-in leaves, as the server writes it (create#UserLoginHistory, which the notice's EECA follows)
function signIn(n, user, successful = "Y", secondsAgo = 0) {
    ec.service
        .sync()
        .name("create#" + HISTORY)
        .parameters({
            userId: user,
            fromDate: new Date(now() - secondsAgo * 1000),
            visitId: visitId(n),
            successfulLogin: successful
        })
        .call()
    const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
    ec.message.clearErrors()
    return error
}
// the notice asked for directly, in a transaction of its own as the EECA asks (a notification whose email cannot be sent
// marks its transaction, and the spec's database has no sender address)
function notify(user, n) {
    const out = ec.service
        .sync()
        .name(S + "notify#NewDeviceSignIn")
        .parameters({userId: user, visitId: visitId(n)})
        .requireNewTransaction(true)
        .call()
    ec.message.clearErrors()
    return out || {}
}
const notices = user =>
    Array.from(find("moqui.security.user.NotificationMessageByUser", {userId: user, topic: "AccountSignIn"}).list()).map(row => ({
        title: String(row.titleText || ""),
        message: String(row.messageJson || "")
    }))

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const id of [userId, freshId])
            make("moqui.security.UserAccount", {
                userId: id,
                username: id.toLowerCase().replace(/_/g, "."),
                userFullName: "Spec new device",
                emailAddress: id.toLowerCase() + "@example.com"
            })
        // the account's known client: Firefox on macOS from 203.0.113.x, ten days ago, in its login history
        visit(1, userId, firefoxMac, "203.0.113.5", 10)
        make(HISTORY, {userId, fromDate: new Date(now() - 10 * DAY), visitId: visitId(1), successfulLogin: "Y"})
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.artifactExecution.disableAuthz()
        const users = [userId, freshId]
        const messageIds = Array.from(
            ec.entity.find("moqui.security.user.NotificationMessageUser").condition("userId", "in", users).list()
        ).map(row => row.notificationMessageId)
        ec.entity.find("moqui.security.user.NotificationMessageUser").condition("userId", "in", users).deleteAll()
        if (messageIds.length)
            ec.entity
                .find("moqui.security.user.NotificationMessage")
                .condition("notificationMessageId", "in", messageIds)
                .deleteAll()
        ec.entity.find(HISTORY).condition("userId", "in", users).deleteAll()
        ec.entity.find(VISIT).condition("userId", "in", users).deleteAll()
        ec.entity.find("moqui.security.UserAccount").condition("userId", "in", users).deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature("a sign-in from a new client sends one notice to the account, and one from a known client none", {
    when() {
        const out = {errors: []}
        // a failed sign-in from a new client, then the known client from another address of its network
        visit(2, userId, chromeWindows, "198.51.100.7")
        out.errors.push(signIn(2, userId, "N", 240))
        visit(3, userId, firefoxMac, "203.0.113.77")
        out.errors.push(signIn(3, userId, "Y", 180))
        out.afterKnown = notices(userId).length
        // the new client signs in: one notice; again from another address of that network: no second one
        out.errors.push(signIn(2, userId, "Y", 120))
        out.afterNew = notices(userId)
        visit(4, userId, chromeWindows, "198.51.100.200")
        out.errors.push(signIn(4, userId, "Y", 60))
        out.afterRepeat = notices(userId).length
        // the same browser from another network is another client
        visit(5, userId, chromeWindows, "192.0.2.9")
        out.other = notify(userId, 5)
        // an account's first sign-in is its owner setting it up
        visit(6, freshId, chromeWindows, "198.51.100.7")
        out.errors.push(signIn(6, freshId))
        out.fresh = notify(freshId, 6)
        out.freshNotices = notices(freshId).length
        // a client last seen before the window is new again
        visit(7, freshId, firefoxMac, "203.0.113.5", 100)
        visit(8, freshId, firefoxMac, "203.0.113.5")
        out.outside = notify(freshId, 8)
        // another account's visit is never this account's sign-in
        out.foreign = notify(freshId, 5)
        out.topic = find("moqui.security.user.NotificationTopic", {topic: "AccountSignIn"}).one()
        out.template = find("moqui.basic.email.EmailTemplate", {emailTemplateId: "ACCOUNT_SIGN_IN"}).one()
        ec.message.clearErrors()
        // a signed-in user may not ask for a notice to an account: no group is authorized for the service
        ec.artifactExecution.enableAuthz()
        try {
            ec.service
                .sync()
                .name(S + "notify#NewDeviceSignIn")
                .parameters({userId, visitId: visitId(5)})
                .call()
            out.direct = ec.message.hasError() ? ec.message.getErrorsString() : ""
        } catch (e) {
            out.direct = String(e.message || e)
        } finally {
            ec.message.clearErrors()
            ec.artifactExecution.disableAuthz()
        }
        return out
    },
    then(out) {
        out.errors.join("") == ""
        out.afterKnown == 0
        out.afterNew.length == 1
        out.afterNew[0].title.includes("Chrome on Windows")
        out.afterNew[0].message.includes("198.51.100.x")
        !out.afterNew[0].message.includes("198.51.100.7")
        !out.afterNew[0].message.includes("Mozilla")
        out.afterRepeat == 1
        out.other.notified == true
        out.other.network == "192.0.2.x"
        out.fresh.reasonCode == "FIRST_SIGN_IN"
        out.freshNotices == 0
        out.outside.notified == true
        out.foreign.reasonCode == "NO_VISIT"
        out.topic.isPrivate == "Y"
        out.topic.emailNotifications == "Y"
        out.template.bodyScreenLocation.includes("AccountSignInEmail")
        out.direct != ""
    }
})
