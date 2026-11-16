// An account disabled for good (disable#UserAccount: disabled Y, no disabledDateTime) stays closed on the password path
// (plans/Security-Review.md SEC-WEB-01, SEC-ADM-01): failed attempts do not give it a lock time that ends by itself, its
// own password does not change it, and no reset password is made for it. A tarpit lock (with a disabledDateTime) still
// ends with a password change, as in Java. The Rust side (login.rs) is in user/login_tests.rs.
// Run: ./moq test framework --spec ClosedAccounts
const CLOSED = "CLOSEDACCT_SPEC_CLOSED"
const LOCKED = "CLOSEDACCT_SPEC_LOCKED"
const PASSWORD = "Closed-spec-9!"
const NEW_PASSWORD = "Closed-spec-10!"
const USER = "org.moqui.impl.UserServices."

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
const account = (userId) => sys(() => ec.entity.find("moqui.security.UserAccount").condition("userId", userId).disableAuthz().one())
function call(name, parameters) {
    let out = {}
    try {
        out = ec.service.sync().name(USER + name).parameters(parameters).call() || {}
    } catch (err) {
        out = {thrown: String(err.message || err)}
    }
    const errors = ec.message.hasError() ? ec.message.errorsString : ""
    ec.message.clearAll()
    return Object.assign({errors}, out)
}
function makeUser(userId, disabledDateTime) {
    sys(() => {
        let row = ec.entity.find("moqui.security.UserAccount").condition("userId", userId).one()
        if (row == null) {
            row = ec.entity.makeValue("moqui.security.UserAccount")
            row.setFields({userId, username: username(userId), userFullName: "Closed " + userId.slice(-6), emailAddress: username(userId) + "@example.com"}, true, null, null)
            row.create()
        }
        row.setFields({currentPassword: ec.user.hashPassword(PASSWORD), passwordHashType: "argon2", passwordSalt: "", passwordBase64: "N",
            successiveFailedLogins: 0, disabled: "Y", disabledDateTime, requirePasswordChange: "N", resetPassword: null,
            resetPasswordSetDate: null}, true, null, null)
        row.update()
        ec.entity.find("moqui.security.UserPasswordHistory").condition("userId", userId).deleteAll()
    })
}

spec({
    setupSpec() {
        makeUser(CLOSED, null)
        makeUser(LOCKED, ec.user.nowTimestamp)
    }
})

feature("an account disabled for good keeps its disable through failed attempts, its own password change and a reset", {
    when() {
        ec.user.logoutUser()
        for (let i = 0; i < 6; i++) call("increment#UserAccountFailedLogins", {userId: CLOSED})
        const counted = account(CLOSED)
        const change = call("update#Password", {username: username(CLOSED), oldPassword: PASSWORD, newPassword: NEW_PASSWORD, newPasswordVerify: NEW_PASSWORD})
        const afterChange = account(CLOSED)
        const reset = call("reset#Password", {username: username(CLOSED)})
        const afterReset = account(CLOSED)
        return {counted, change, afterChange, reset, afterReset}
    },
    then({counted, change, afterChange, reset, afterReset}) {
        counted.disabled == "Y"
        counted.disabledDateTime == null
        counted.successiveFailedLogins == 6
        change.updateSuccessful != true
        afterChange.disabled == "Y"
        ec.user.passwordsMatch(PASSWORD, afterChange.currentPassword, "argon2", "", false)
        reset.errors == ""
        afterReset.resetPassword == null
    }
})

// The old password is behind the login gate (SEC-WEB-02): during the lock window update#Password refuses even the
// right one, so the lock ends here by a change that needs no old password (a password administrator's).
feature("a tarpit lock ends with a password change", {
    when() {
        ec.user.logoutUser()
        const refused = call("update#Password", {username: username(LOCKED), oldPassword: PASSWORD, newPassword: NEW_PASSWORD, newPasswordVerify: NEW_PASSWORD})
        const change = sys(() => call("update#PasswordInternal", {userId: LOCKED, newPassword: NEW_PASSWORD, newPasswordVerify: NEW_PASSWORD}))
        return {refused, change, after: account(LOCKED)}
    },
    then({refused, change, after}) {
        refused.updateSuccessful != true
        change.updateSuccessful == true
        after.disabled == "N"
    }
})
