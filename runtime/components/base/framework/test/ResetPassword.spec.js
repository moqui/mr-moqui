// The security proof of OWASP ASVS 5.0.0 v5.0.0-6.4.1 (docs/security/proofs/V6-Authentication.md), in-process: the
// password the system makes, reset#Password's reset password (ec.user.randomString(12): twelve URL-safe base64
// characters of twelve bytes from the operating system's random source, getrandom, so 72 bits), is random, mailed and
// kept only as its argon2 hash, signs nobody in (update#Password alone takes it, as the old password), is taken once (a
// password change clears it), and is refused once older than user-facade/password@email-expire-hours (48 by default).
// The reset password is read from the mail capture, which needs a from address on the PASSWORD_RESET template: the spec
// sets one and puts the template back after. The in-date check alone is in crates/moqui-core/src/user/login_tests.rs.
// It does not show that the reset password cannot become the new password: update#PasswordInternal refuses the current
// password and those in the history, not the reset password.
// Run: ./moq test framework --spec ResetPassword
const ONCE = "RESETPW_SPEC_ONCE"
const LATE = "RESETPW_SPEC_LATE"
const PASSWORD = "Reset-spec-9!"
const NEW_PASSWORD = "Reset-spec-10!"
const NEWER_PASSWORD = "Reset-spec-11!"
const USER = "org.moqui.impl.UserServices."
const REFUSED = "Password could not be updated with the information provided."
const EXPIRE_HOURS = "user-facade/password@email-expire-hours"
const capture = ec.factory.getTool("MailCapture")
// the PASSWORD_RESET template's from address before the spec, and the reset password mailed to each account
let templateFrom = null
const mailed = {}

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
const email = (userId) => username(userId) + "@example.com"
const account = (userId) => sys(() => ec.entity.find("moqui.security.UserAccount").condition("userId", userId).one())
const matches = (plain, stored) => ec.user.passwordsMatch(plain, stored, "argon2", "", false)
function run(name, parameters) {
    let out = {}
    try {
        out = ec.service.sync().name(USER + name).parameters(parameters).call() || {}
    } catch (err) {
        out = {thrown: String(err.message || err)}
    }
    const said = [...ec.message.messages, ...ec.message.errors].join(" | ")
    ec.message.clearAll()
    return Object.assign({said}, out)
}
const change = (userId, oldPassword, newPassword) => run("update#Password", {username: username(userId), oldPassword,
    newPassword, newPasswordVerify: newPassword})
function makeUser(userId) {
    sys(() => {
        let row = ec.entity.find("moqui.security.UserAccount").condition("userId", userId).one()
        if (row == null) {
            row = ec.entity.makeValue("moqui.security.UserAccount")
            row.setFields({userId, username: username(userId), userFullName: "Reset " + userId.slice(-4), emailAddress: email(userId)},
                true, null, null)
            row.create()
        }
        row.setFields({currentPassword: ec.user.hashPassword(PASSWORD), passwordHashType: "argon2", passwordSalt: "",
            passwordBase64: "N", successiveFailedLogins: 0, disabled: "N", disabledDateTime: null, requirePasswordChange: "N",
            resetPassword: null, resetPasswordSetDate: null}, true, null, null)
        row.update()
        ec.entity.find("moqui.security.UserPasswordHistory").condition("userId", userId).deleteAll()
        ec.entity.find("moqui.security.UserAuthcFactor").condition("userId", userId).deleteAll()
    })
}
function setTemplateFrom(address) {
    sys(() => {
        const template = ec.entity.find("moqui.basic.email.EmailTemplate").condition("emailTemplateId", "PASSWORD_RESET").one()
        template.set("fromAddress", address)
        template.update()
    })
}
// the reset password in the mail to the account, which reset#Password sends after it commits
function mailedTo(userId) {
    const deadline = Date.now() + 10000
    while (Date.now() < deadline) {
        const message = Array.from(capture.getMessages()).find((sent) => Array.from(sent.toList).includes(email(userId)))
        const found = message && /reset password has been set to: ([A-Za-z0-9_-]+)/.exec(`${message.bodyText} ${message.body}`)
        if (found) return found[1]
        const pause = Date.now() + 20
        while (Date.now() < pause) { /* look again shortly */ }
    }
    return null
}

spec({
    setupSpec() {
        makeUser(ONCE)
        makeUser(LATE)
        templateFrom = sys(() => ec.entity.find("moqui.basic.email.EmailTemplate").condition("emailTemplateId", "PASSWORD_RESET")
            .one().fromAddress)
        setTemplateFrom("accounts@reset-spec.example.test")
        capture.enable()
        capture.clear()
    },
    cleanupSpec() {
        capture.reset()
        setTemplateFrom(templateFrom)
    }
})

feature("a reset password is random, mailed, kept only as its argon2 hash, and signs nobody in", {
    when() {
        ec.user.logoutUser()
        const asked = [ONCE, LATE].map((userId) => run("reset#Password", {username: username(userId)}))
        for (const userId of [ONCE, LATE]) mailed[userId] = mailedTo(userId)
        const stored = account(ONCE)
        const signedIn = ec.user.loginUser(username(ONCE), mailed[ONCE])
        ec.message.clearAll()
        if (signedIn) ec.user.logoutUser()
        return {asked, once: mailed[ONCE], late: mailed[LATE], stored, signedIn, after: account(ONCE)}
    },
    then({asked, once, late, stored, signedIn, after}) {
        asked.every((out) => out.said.includes("a reset password was sent"))
        /^[A-Za-z0-9_-]{12}$/.test(once)
        /^[A-Za-z0-9_-]{12}$/.test(late)
        once != late
        stored.resetPassword.startsWith("$argon2id$")
        !stored.resetPassword.includes(once)
        matches(once, stored.resetPassword)
        !matches(late, stored.resetPassword)
        stored.resetPasswordSetDate != null
        signedIn == false
        matches(PASSWORD, after.currentPassword)
    }
})

feature("a reset password changes the password once, and the change clears it", {
    when() {
        ec.user.logoutUser()
        const first = change(ONCE, mailed[ONCE], NEW_PASSWORD)
        const changed = account(ONCE)
        const again = change(ONCE, mailed[ONCE], NEWER_PASSWORD)
        return {first, changed, again, after: account(ONCE)}
    },
    then({first, changed, again, after}) {
        first.updateSuccessful == true
        changed.resetPassword == null
        changed.resetPasswordSetDate == null
        matches(NEW_PASSWORD, changed.currentPassword)
        again.updateSuccessful != true
        again.said == REFUSED
        matches(NEW_PASSWORD, after.currentPassword)
    }
})

feature("a reset password older than email-expire-hours is refused, and taken while the setting keeps it in date", {
    when() {
        ec.user.logoutUser()
        // the reset of the first feature, as if made 49 hours ago
        sys(() => {
            const row = ec.entity.find("moqui.security.UserAccount").condition("userId", LATE).one()
            row.set("resetPasswordSetDate", new Date(Date.now() - 49 * 3600000))
            row.update()
        })
        const expired = withConf(EXPIRE_HOURS, "48", () => change(LATE, mailed[LATE], NEW_PASSWORD))
        const kept = account(LATE)
        const inDate = withConf(EXPIRE_HOURS, "72", () => change(LATE, mailed[LATE], NEW_PASSWORD))
        return {expired, kept, inDate, after: account(LATE)}
    },
    then({expired, kept, inDate, after}) {
        expired.updateSuccessful != true
        expired.said == REFUSED
        kept.resetPassword != null
        matches(PASSWORD, kept.currentPassword)
        inDate.updateSuccessful == true
        after.resetPassword == null
        matches(NEW_PASSWORD, after.currentPassword)
    }
})
