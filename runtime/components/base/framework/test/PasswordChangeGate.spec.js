// The old password of update#Password is behind the gate of a password login (plans/Security-Review-Medium.md
// SEC-WEB-02, SEC-ADM-02; ec.user.checkOldPassword): a wrong one counts as a failed login, a locked account refuses
// even the right one, and every refusal (wrong, locked, unknown user) answers the same message, so the second factor's
// message never confirms a password to a caller the gate refused. reset#Password leaves requirePasswordChange as it is
// (SEC-WEB-09, SEC-ADM-12). A new password ends the account's other sessions (SEC-WEB-06). The Rust side is in crates/moqui-core/src/user/login_tests.rs.
// Run: ./moq test framework --spec PasswordChangeGate
const GATED = "PWGATE_SPEC_GATED"
const FACTOR = "PWGATE_SPEC_FACTOR"
const RESET = "PWGATE_SPEC_RESET"
const ENDS = "PWGATE_SPEC_ENDS"
const PASSWORD = "Gate-spec-9!"
const NEW_PASSWORD = "Gate-spec-10!"
const USER = "org.moqui.impl.UserServices."
const REFUSED = "Password could not be updated with the information provided."

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
    const said = [...ec.message.messages, ...ec.message.errors].join(" | ")
    ec.message.clearAll()
    return Object.assign({said}, out)
}
const change = (userId, oldPassword) => call("update#Password", {username: username(userId), oldPassword,
    newPassword: NEW_PASSWORD, newPasswordVerify: NEW_PASSWORD})
function makeUser(userId, fields) {
    sys(() => {
        let row = ec.entity.find("moqui.security.UserAccount").condition("userId", userId).one()
        if (row == null) {
            row = ec.entity.makeValue("moqui.security.UserAccount")
            row.setFields({userId, username: username(userId), userFullName: "Gate " + userId.slice(-6), emailAddress: username(userId) + "@example.com"}, true, null, null)
            row.create()
        }
        row.setFields(Object.assign({currentPassword: ec.user.hashPassword(PASSWORD), passwordHashType: "argon2", passwordSalt: "",
            passwordBase64: "N", successiveFailedLogins: 0, disabled: "N", disabledDateTime: null, requirePasswordChange: "N",
            resetPassword: null, resetPasswordSetDate: null}, fields || {}), true, null, null)
        row.update()
        ec.entity.find("moqui.security.UserPasswordHistory").condition("userId", userId).deleteAll()
        ec.entity.find("moqui.security.UserAuthcFactor").condition("userId", userId).deleteAll()
    })
}

spec({
    setupSpec() {
        makeUser(GATED)
        makeUser(FACTOR)
        makeUser(RESET, {requirePasswordChange: "Y"})
        makeUser(ENDS)
        // a second factor in date, so a right old password with no code is answered "Authentication code required"
        sys(() => ec.service.sync().name("create#moqui.security.UserAuthcFactor").parameters({factorId: "PWGATE_SPEC_TOTP",
            userId: FACTOR, factorTypeEnumId: "UafTotp", factorOption: ec.user.totpSecret(), fromDate: ec.user.nowTimestamp}).call())
    }
})

feature("a wrong old password counts, a locked account refuses the right one, and every refusal reads the same", {
    when() {
        ec.user.logoutUser()
        const wrong = []
        for (let i = 0; i < 4; i++) wrong.push(change(GATED, "wrong-" + i))
        const locked = account(GATED)
        const right = change(GATED, PASSWORD)
        const unknown = change("PWGATE_SPEC_NOBODY", PASSWORD)
        return {wrong, locked, right, unknown, after: account(GATED)}
    },
    then({wrong, locked, right, unknown, after}) {
        wrong.every((out) => out.updateSuccessful != true && out.said == REFUSED)
        locked.disabled == "Y"
        locked.successiveFailedLogins == 4
        right.updateSuccessful != true
        right.said == REFUSED
        unknown.said == REFUSED
        after.successiveFailedLogins == 5
        ec.user.passwordsMatch(PASSWORD, after.currentPassword, "argon2", "", false)
    }
})

feature("the second factor's message answers only a caller with the right old password, and a locked one does not get it", {
    when() {
        ec.user.logoutUser()
        const right = change(FACTOR, PASSWORD)
        ec.web?.sessionAttributes?.remove("moquiPreAuthcUsername")
        for (let i = 0; i < 4; i++) change(FACTOR, "wrong-" + i)
        const locked = change(FACTOR, PASSWORD)
        return {right, locked}
    },
    then({right, locked}) {
        right.said.includes("Authentication code required")
        locked.said == REFUSED
    }
})

feature("a reset leaves requirePasswordChange as it was", {
    when() {
        ec.user.logoutUser()
        const reset = call("reset#Password", {username: username(RESET)})
        return {reset, after: account(RESET)}
    },
    then({reset, after}) {
        after.resetPassword != null
        after.requirePasswordChange == "Y"
    }
})

feature("a new password ends the account's other sessions", {
    when() {
        ec.user.logoutUser()
        // a browser of the account somewhere else: its visit and its web session row
        sys(() => {
            ec.entity.makeValue("moqui.server.Visit").setAll({visitId: "PWGATE_SPEC_VISIT", userId: ENDS,
                fromDate: ec.user.nowTimestamp, thruDate: null}).createOrUpdate()
            ec.entity.makeValue("moqui.server.HttpSession").setAll({sessionId: "PWGATE_SPEC_SESSION", userId: ENDS,
                visitId: "PWGATE_SPEC_VISIT", sessionToken: "pwgate-token"}).createOrUpdate()
        })
        const changed = sys(() => call("update#PasswordInternal", {userId: ENDS, newPassword: NEW_PASSWORD,
            newPasswordVerify: NEW_PASSWORD}))
        return sys(() => ({
            changed,
            session: ec.entity.find("moqui.server.HttpSession").condition("sessionId", "PWGATE_SPEC_SESSION").one(),
            visit: ec.entity.find("moqui.server.Visit").condition("visitId", "PWGATE_SPEC_VISIT").one()
        }))
    },
    then({changed, session, visit}) {
        changed.updateSuccessful == true
        session == null
        visit.thruDate != null
    }
})
