// The recent sign-in check and the re-authentication that satisfies it (org.moqui.sso.RecentAuthServices;
// crates/moqui-core/src/service/recent_auth.rs, user/login.rs and user/factor.rs; docs/security/Recent-Auth.md): an
// internal login is not a recent sign-in; the signed-in user's password, a wrong one counted as a failed login; with a
// second factor the code too, a wrong code counted, the right one (a single-use code) accepted without the password again;
// a code sent only once the password was checked, and not again within half a minute; another user's login drops the
// sign-in. The ChatKeys specs check the actions it guards.
// Run: ./moq test framework --spec RecentAuth
const A = "RECENTAUTH_SPEC_A"
const B = "RECENTAUTH_SPEC_B"
const PASSWORD = "Recent-spec-9!"
const SERVICES = "org.moqui.sso.RecentAuthServices."

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function username(userId) {
    return userId.toLowerCase().replace(/_/g, ".")
}
function as(userId, name, parameters) {
    if (userId) ec.user.internalLoginUser(username(userId))
    let out = {}
    let thrown = ""
    try {
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : SERVICES + name).parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "Recent " + userId.slice(-1), disabled: "N"}, true, null, null)
            account.create()
        }
        const account = ec.entity.find("moqui.security.UserAccount").condition("userId", userId).one()
        account.setFields({currentPassword: ec.user.hashPassword(PASSWORD), passwordHashType: "argon2", passwordSalt: "", passwordBase64: "N",
            successiveFailedLogins: 0, disabled: "N", disabledDateTime: null, requirePasswordChange: "N"}, true, null, null)
        account.update()
        ec.entity.find("moqui.security.UserAuthcFactor").condition("userId", userId).deleteAll()
    })
}
const failures = (userId) => Number(sys(() => ec.entity.find("moqui.security.UserAccount").condition("userId", userId).one()).successiveFailedLogins || 0)

spec({
    setupSpec() {
        for (const u of [A, B]) makeUser(u)
    }
})

feature("an internal login is not a recent sign-in; the user's password is, a wrong one counted as a failed login; another user's login drops it", {
    when() {
        ec.user.logoutUser()
        const before = as(A, "check#RecentAuth", {})
        const wrong = as(null, "reauthenticate#User", {password: "not-it"})
        const failed = failures(A)
        const right = as(null, "reauthenticate#User", {password: PASSWORD})
        const after = as(null, "check#RecentAuth", {})
        const required = as(null, "check#RecentAuth", {required: true})
        const switched = as(B, "check#RecentAuth", {})
        const back = as(A, "check#RecentAuth", {})
        return {before, wrong, failed, right, after, required, switched, back, reset: failures(A)}
    },
    then({before, wrong, failed, right, after, required, switched, back, reset}) {
        before.recent == false
        before.method == "password"
        before.secondFactor == false
        wrong.reauthenticated == false
        wrong.errors != ""
        failed == 1
        right.reauthenticated == true
        right.errors == ""
        reset == 0
        after.recent == true
        required.errors == ""
        switched.recent == false
        back.recent == false
    }
})

feature("with a second factor the password, then the code: a wrong code counted, a code sent only after the password and not twice in half a minute", {
    when() {
        const codes = sys(() => ec.service.sync().name("org.moqui.impl.UserServices.create#SingleUseAuthcCodes")
            .parameters({userId: A, numberOfCodes: 2}).call()).singleUseCodes
        ec.user.logoutUser()
        const check = as(A, "check#RecentAuth", {})
        const sendFirst = as(null, "send#ReauthCode", {factorId: "RECENTAUTH_NO_FACTOR"})
        const password = as(null, "reauthenticate#User", {password: PASSWORD})
        const stillOld = as(null, "check#RecentAuth", {})
        const sent = as(null, "send#ReauthCode", {factorId: "RECENTAUTH_NO_FACTOR"})
        const sentAgain = as(null, "send#ReauthCode", {factorId: "RECENTAUTH_NO_FACTOR"})
        const before = failures(A)
        const wrongCode = as(null, "reauthenticate#User", {code: "00000000"})
        const counted = failures(A) - before
        const rightCode = as(null, "reauthenticate#User", {code: String(codes[0])})
        const after = as(null, "check#RecentAuth", {})
        sys(() => ec.entity.find("moqui.security.UserAuthcFactor").condition("userId", A).deleteAll())
        return {check, sendFirst, password, stillOld, sent, sentAgain, wrongCode, counted, rightCode, after}
    },
    then({check, sendFirst, password, stillOld, sent, sentAgain, wrongCode, counted, rightCode, after}) {
        check.secondFactor == true
        sendFirst.sent == false
        sendFirst.errors.includes("password first")
        password.reauthenticated == false
        password.codeNeeded == true
        stillOld.recent == false
        // the factor does not exist: the service refused it, but the send was tried
        sent.sent == false
        sent.errors.includes("not found")
        sentAgain.errors.includes("half a minute")
        wrongCode.reauthenticated == false
        wrongCode.codeNeeded == true
        counted == 1
        rightCode.reauthenticated == true
        rightCode.errors == ""
        after.recent == true
    }
})

feature("factor presentation omits authenticator secrets and recovery hashes; explicit setup and code verification retain them", {
    when() {
        const users = "org.moqui.impl.UserServices."
        try {
            const totp = as(B, users + "create#UserAuthcFactorTotp", {userId: B})
            const email = as(null, users + "create#UserAuthcFactorEmail", {userId: B, factorOption: "recent.auth@example.invalid"})
            const sms = as(null, users + "create#UserAuthcFactorSms", {userId: B, factorOption: "+15555550123"})
            const child = as(null, users + "create#SingleUseAuthcFactor", {userId: B, fromFactorId: email.factorId})
            const hash = sys(() => ec.entity.find("moqui.security.UserAuthcFactor").condition("factorId", child.factorId).one()).factorOption
            const expired = as(null, users + "create#UserAuthcFactorTotp", {userId: B, thruDate: Date.now() - 1000})
            const info = as(null, users + "get#UserAuthcFactorInfo", {userId: B})
            const rows = info.userAuthcFactorList || []
            const app = rows.find(row => row.factorId === totp.factorId) || {}
            const recovery = rows.find(row => row.factorId === child.factorId) || {}
            const setup = as(null, users + "setup#UserAuthcFactorTotp", {userId: B, factorId: totp.factorId})
            const verified = as(null, users + "validate#UserAuthcFactorCode", {userId: B, code: child.code})
            // The same app row stays redacted when verified, while its presentation metadata is retained.
            sys(() => ec.service.sync().name("update#moqui.security.UserAuthcFactor")
                .parameters({factorId: totp.factorId, needsValidation: "N"}).call())
            const after = as(null, users + "get#UserAuthcFactorInfo", {userId: B})
            const appAfter = (after.userAuthcFactorList || []).find(row => row.factorId === totp.factorId) || {}
            // Return only booleans: failed assertions must not print even the generated fixture secrets.
            return {
                created: [totp, email, sms, child, expired].every(out => out.errors === "" && !!out.factorId),
                noErrors: [info, setup, verified, after].every(out => out.errors === ""),
                redacted: app.factorOption == null && recovery.factorOption == null &&
                    !JSON.stringify(info).includes(totp.factorOption) && !JSON.stringify(info).includes(hash),
                metadata: app.userId === B && app.factorTypeEnumId === "UafTotp" && app.fromDate != null &&
                    app.thruDate != null && app.needsValidation === "Y" && recovery.fromFactorId === email.factorId,
                onlyActive: rows.length === 4 && !rows.some(row => row.factorId === expired.factorId),
                parentSummary: JSON.stringify(info.factorTypeEnumIds) === JSON.stringify(["UafEmail", "UafSms", "UafTotp"]),
                destinations: info.sendableFactors.length === 2 && info.sendableFactors.some(row => row.factorId === email.factorId && row.factorOption === "recent.auth@example.invalid") &&
                    info.sendableFactors.some(row => row.factorId === sms.factorId && row.factorOption === "+15555550123"),
                setupSecret: setup.qrSecret === totp.factorOption && setup.qrUri.includes(encodeURIComponent(totp.factorOption)),
                codeVerified: verified.verified === true && verified.factorId === child.factorId,
                verifiedRedacted: appAfter.needsValidation === "N" && appAfter.factorOption == null && !JSON.stringify(after).includes(totp.factorOption)
            }
        } finally {
            sys(() => ec.entity.find("moqui.security.UserAuthcFactor").condition("userId", B).deleteAll())
        }
    },
    then({created, noErrors, redacted, metadata, onlyActive, parentSummary, destinations, setupSecret, codeVerified, verifiedRedacted}) {
        created == true
        noErrors == true
        redacted == true
        metadata == true
        onlyActive == true
        parentSummary == true
        destinations == true
        setupSecret == true
        codeVerified == true
        verifiedRedacted == true
    }
})
