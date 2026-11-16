// Who may make, end and use an account's sign-in credentials (plans/Security-Review-Medium.md SEC-ADM-03 to SEC-ADM-06,
// SEC-WEB-12; org.moqui.impl.UserServices check#AccountAction, org.moqui.sso.RecentAuthServices require#RecentAuth): a code
// stands in for the old password only in a pre-authenticated session, never a signed-in one; issuing codes and adding or
// ending a factor need a recent sign-in; another account's factors need ADMIN_IDENTITY_MANAGE and are refused when that
// account holds access the caller lacks (heldBeyond), as is setting its password or changing its email; a verified
// authenticator's secret is not given again; a code is sent at most once a half minute; an
// identity operator gives no bundle to their own account; a session changes its own email address or provider link only
// after a recent sign-in. The Rust side (keys, tokens, app passwords) is in
// crates/moqui-core/src/service/user_keys_tests.rs, an authenticator code taken once in screen/tests/authc.rs.
// Run: ./moq test framework --spec CredentialGuards
const USER = "org.moqui.impl.UserServices."
const PASSWORD = "Guard-spec-9!"
const OWNER = "CREDGUARD_SPEC_OWNER"
const OTHER = "CREDGUARD_SPEC_OTHER"
const ADMIN = "CREDGUARD_SPEC_ADMIN"
const SEALED = "CREDGUARD_SPEC_SEALED"
const USERS_GROUP = "CREDGUARD_SPEC_USERS"
const ADMIN_GROUP = "CREDGUARD_SPEC_ADMINS"
const SEALED_GROUP = "CREDGUARD_SPEC_SEALED"
const SERVICES = "CREDGUARD_SPEC_SERVICES"
const TOTP = "CREDGUARD_SPEC_TOTP"
const EMAIL = "CREDGUARD_SPEC_EMAIL"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
const make = (entityName, fields) => sys(() => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
})
const one = (entityName, conditions) => sys(() => ec.entity.find(entityName).condition(conditions).one())
// a call as the signed-in user, with authorization on as a screen's transition runs it
function call(name, parameters) {
    const wasDisabled = ec.artifactExecution.authzDisabled()
    if (wasDisabled) ec.artifactExecution.enableAuthz()
    let out = {}
    try {
        out = ec.service.sync().name(name.includes(".") ? name : USER + name).parameters(parameters || {}).call() || {}
    } catch (err) {
        out = {thrown: String(err.message || err)}
    } finally {
        if (wasDisabled) ec.artifactExecution.disableAuthz()
    }
    const errors = [...ec.message.errors].join(" | ")
    ec.message.clearAll()
    return Object.assign({errors}, out)
}
// signed in by an internal login: a session, not a recent sign-in
const signIn = (userId) => ec.user.internalLoginUser(username(userId))
const confirm = () => call("org.moqui.sso.RecentAuthServices.reauthenticate#User", {password: PASSWORD})
const passwordIs = (userId, password) => ec.user.passwordsMatch(password, one("moqui.security.UserAccount", {userId}).currentPassword, "argon2", "", false)
const inDate = (factorId) => {
    const row = one("moqui.security.UserAuthcFactor", {factorId})
    return row != null && (row.thruDate == null || ec.user.timestampMillis(row.thruDate) > ec.user.timestampMillis(ec.user.nowTimestamp))
}

spec({
    setupSpec() {
        sys(() => {
            for (const userId of [OWNER, OTHER, ADMIN, SEALED]) {
                if (one("moqui.security.UserAccount", {userId}) == null)
                    make("moqui.security.UserAccount", {userId, username: username(userId), userFullName: "Guard " + userId.slice(15),
                        emailAddress: username(userId) + "@example.com"})
                const account = ec.entity.find("moqui.security.UserAccount").condition("userId", userId).one()
                account.setFields({currentPassword: ec.user.hashPassword(PASSWORD), passwordHashType: "argon2", passwordSalt: "",
                    passwordBase64: "N", successiveFailedLogins: 0, disabled: "N", disabledDateTime: null, requirePasswordChange: "N"},
                    true, null, null)
                account.update()
                ec.entity.find("moqui.security.UserAuthcFactor").condition("userId", userId).deleteAll()
                ec.entity.find("moqui.security.UserPasswordHistory").condition("userId", userId).deleteAll()
            }
            // the services as a screen grants them, to every spec user
            if (one("moqui.security.UserGroup", {userGroupId: USERS_GROUP}) == null) {
                make("moqui.security.UserGroup", {userGroupId: USERS_GROUP, description: "Guard spec users"})
                make("moqui.security.UserGroup", {userGroupId: ADMIN_GROUP, description: "Guard spec identity administrators"})
                make("moqui.security.UserGroup", {userGroupId: SEALED_GROUP, description: "Guard spec sealed holders"})
                make("moqui.security.ArtifactGroup", {artifactGroupId: SERVICES, description: "Guard spec services"})
                make("moqui.security.ArtifactGroupMember", {artifactGroupId: SERVICES, artifactTypeEnumId: "AT_SERVICE", inheritAuthz: "Y",
                    nameIsPattern: "Y", artifactName: "org\\.moqui\\.(impl\\.UserServices|sso\\.RecentAuthServices)\\..*"})
                make("moqui.security.ArtifactAuthz", {artifactAuthzId: SERVICES, userGroupId: USERS_GROUP, artifactGroupId: SERVICES,
                    authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL"})
                for (const permission of ["ADMIN_IDENTITY_VIEW", "ADMIN_IDENTITY_MANAGE", "ADMIN_PASSWORD"])
                    make("moqui.security.UserGroupPermission", {userGroupId: ADMIN_GROUP, userPermissionId: permission, fromDate: 0})
                // a sealed permission the administrator does not hold
                make("moqui.security.UserGroupPermission", {userGroupId: SEALED_GROUP, userPermissionId: "SQL_RUNNER_WEB", fromDate: 0})
                for (const userId of [OWNER, OTHER, ADMIN, SEALED])
                    make("moqui.security.UserGroupMember", {userGroupId: USERS_GROUP, userId, fromDate: 0})
                make("moqui.security.UserGroupMember", {userGroupId: ADMIN_GROUP, userId: ADMIN, fromDate: 0})
                make("moqui.security.UserGroupMember", {userGroupId: SEALED_GROUP, userId: SEALED, fromDate: 0})
            }
            // the owner's authenticator, verified, and an email factor
            make("moqui.security.UserAuthcFactor", {factorId: TOTP, userId: OWNER, factorTypeEnumId: "UafTotp", factorOption: ec.user.totpSecret(),
                fromDate: ec.user.nowTimestamp, needsValidation: "N"})
            make("moqui.security.UserAuthcFactor", {factorId: EMAIL, userId: OWNER, factorTypeEnumId: "UafEmail",
                factorOption: "owner-factor@example.com", fromDate: ec.user.nowTimestamp, needsValidation: "N"})
            make("moqui.security.UserAuthcFactor", {factorId: TOTP + "_S", userId: SEALED, factorTypeEnumId: "UafTotp",
                factorOption: ec.user.totpSecret(), fromDate: ec.user.nowTimestamp, needsValidation: "N"})
        })
    },
    cleanupSpec() {
        sys(() => {
            for (const userId of [OWNER, OTHER, ADMIN, SEALED]) {
                ec.entity.find("moqui.security.UserAuthcFactor").condition("userId", userId).deleteAll()
                ec.entity.find("moqui.security.UserGroupMember").condition("userId", userId).deleteAll()
            }
        })
        ec.user.logoutUser()
    }
})

feature("a signed-in session's code does not stand in for the old password; issuing codes and adding or ending a factor need a recent sign-in", {
    when() {
        ec.user.logoutUser()
        signIn(OWNER)
        // single use codes the owner had before, to try as the code of a password change
        const issued = sys(() => ec.service.sync().name(USER + "create#SingleUseAuthcCodes").parameters({userId: OWNER, numberOfCodes: 2}).call())
        const byCode = call("update#Password", {userId: OWNER, oldPassword: "not-it", code: String(issued.singleUseCodes[0]),
            newPassword: "Taken-9!x", newPasswordVerify: "Taken-9!x"})
        const codesRefused = call("create#SingleUseAuthcCodes", {userId: OWNER, numberOfCodes: 3})
        const endRefused = call("invalidate#UserAuthcFactorEntry", {userId: OWNER, factorId: EMAIL})
        const addRefused = call("create#UserAuthcFactorEmail", {userId: OWNER, factorOption: "new-factor@example.com"})
        const emailAfterRefusal = inDate(EMAIL)
        // the owner has a second factor: the password, then one of the codes
        confirm()
        const confirmed = call("org.moqui.sso.RecentAuthServices.reauthenticate#User", {code: String(issued.singleUseCodes[1])})
        const codes = call("create#SingleUseAuthcCodes", {userId: OWNER, numberOfCodes: 3})
        const ended = call("invalidate#UserAuthcFactorEntry", {userId: OWNER, factorId: EMAIL})
        return {byCode, unchanged: passwordIs(OWNER, PASSWORD), codesRefused, endRefused, addRefused, emailAfterRefusal, confirmed,
            codes, ended, emailEnded: !inDate(EMAIL)}
    },
    then({byCode, unchanged, codesRefused, endRefused, addRefused, emailAfterRefusal, confirmed, codes, ended, emailEnded}) {
        byCode.updateSuccessful != true
        unchanged
        codesRefused.errors.includes("Confirm it is you")
        codesRefused.singleUseCodes == null
        endRefused.errors.includes("Confirm it is you")
        addRefused.errors.includes("Confirm it is you")
        emailAfterRefusal
        confirmed.reauthenticated == true
        codes.errors == ""
        codes.singleUseCodes.length == 3
        ended.errors == ""
        emailEnded
    }
})

feature("another account's factors need ADMIN_IDENTITY_MANAGE, and an account holding access the caller lacks is refused", {
    when() {
        ec.user.logoutUser()
        signIn(OTHER)
        const notYours = call("create#SingleUseAuthcCodes", {userId: OWNER, numberOfCodes: 2})
        const notYoursInfo = call("get#UserAuthcFactorInfo", {userId: OWNER})
        const notYoursEnd = call("invalidate#UserAuthcFactorEntry", {userId: OWNER, factorId: TOTP})
        signIn(ADMIN)
        const info = call("get#UserAuthcFactorInfo", {userId: OWNER})
        const adminCodes = call("create#SingleUseAuthcCodes", {userId: OTHER, numberOfCodes: 2})
        const heldCodes = call("create#SingleUseAuthcCodes", {userId: SEALED, numberOfCodes: 2})
        const heldEnd = call("invalidate#UserAuthcFactorEntry", {userId: SEALED, factorId: TOTP + "_S"})
        const heldPassword = call("update#Password", {userId: SEALED, oldPassword: "-", newPassword: "Taken-9!x", newPasswordVerify: "Taken-9!x"})
        const heldEmail = call("update#UserAccount", {userId: SEALED, emailAddress: "taken@example.com"})
        const otherPassword = call("update#Password", {userId: OTHER, oldPassword: "-", newPassword: "Admin-set-9!", newPasswordVerify: "Admin-set-9!"})
        const ownByAdmin = call("update#Password", {userId: ADMIN, oldPassword: "not-it", newPassword: "Taken-9!x", newPasswordVerify: "Taken-9!x"})
        return {notYours, notYoursInfo, notYoursEnd, totpKept: inDate(TOTP), info, adminCodes, heldCodes, heldEnd, heldKept: inDate(TOTP + "_S"),
            heldPassword, sealedUnchanged: passwordIs(SEALED, PASSWORD), heldEmail,
            sealedEmail: one("moqui.security.UserAccount", {userId: SEALED}).emailAddress, otherPassword, otherSet: passwordIs(OTHER, "Admin-set-9!"),
            ownByAdmin, adminUnchanged: passwordIs(ADMIN, PASSWORD)}
    },
    then(r) {
        r.notYours.errors.includes("not yours")
        r.notYoursInfo.errors.includes("not yours")
        r.notYoursEnd.errors.includes("not yours")
        r.totpKept
        r.info.errors == ""
        r.info.secondFactorRequired == true
        r.adminCodes.errors == ""
        r.adminCodes.singleUseCodes.length == 2
        r.heldCodes.errors.includes("SQL_RUNNER_WEB")
        r.heldEnd.errors.includes("SQL_RUNNER_WEB")
        r.heldKept
        r.heldPassword.errors.includes("SQL_RUNNER_WEB")
        r.sealedUnchanged
        r.heldEmail.errors.includes("SQL_RUNNER_WEB")
        r.sealedEmail == "credguard.spec.sealed@example.com"
        r.otherPassword.updateSuccessful == true
        r.otherSet
        // an administrator's own password needs the old one, as anyone's
        r.ownByAdmin.updateSuccessful != true
        r.adminUnchanged
    }
})

feature("a verified authenticator's secret is not given again", {
    when() {
        ec.user.logoutUser()
        signIn(OWNER)
        return {shown: call("setup#UserAuthcFactorTotp", {userId: OWNER, factorId: TOTP})}
    },
    then({shown}) {
        shown.errors == ""
        shown.qrSecret == null
        shown.qrUri == null
    }
})

feature("a code is sent to a factor at most once a half minute", {
    when() {
        ec.user.logoutUser()
        signIn(OWNER)
        // a code sent to the factor a moment ago: its single use row
        sys(() => make("moqui.security.UserAuthcFactor", {factorId: "CREDGUARD_SPEC_SENT", userId: OWNER, fromFactorId: EMAIL + "_2",
            factorTypeEnumId: "UafSingleUse", factorOption: "x", fromDate: ec.user.nowTimestamp}))
        make("moqui.security.UserAuthcFactor", {factorId: EMAIL + "_2", userId: OWNER, factorTypeEnumId: "UafEmail",
            factorOption: "owner-factor-2@example.com", fromDate: ec.user.nowTimestamp, needsValidation: "N"})
        return {again: call("send#ExternalAuthcCode", {factorId: EMAIL + "_2"})}
    },
    then({again}) {
        again.errors.includes("half a minute")
    }
})

feature("an identity operator gives no bundle to their own account", {
    when() {
        ec.user.logoutUser()
        signIn(ADMIN)
        return {own: call("org.moqui.admin.AccessServices.apply#GrantProposal", {userId: ADMIN, bundleGroupIdList: ["ADMIN"], reason: "spec"})}
    },
    then({own}) {
        own.applied == false
        own.reasonCode == "OWN_ACCOUNT"
    }
})

feature("a session changes its own email address or provider link only after a recent sign-in; an administrator changes another's", {
    when() {
        const account = () => one("moqui.security.UserAccount", {userId: OTHER})
        // the spec's password again and no factor (an earlier feature set another password and issued codes)
        sys(() => {
            ec.entity.find("moqui.security.UserAuthcFactor").condition("userId", OTHER).deleteAll()
            const row = ec.entity.find("moqui.security.UserAccount").condition("userId", OTHER).one()
            row.setFields({currentPassword: ec.user.hashPassword(PASSWORD), passwordHashType: "argon2", passwordSalt: "",
                passwordBase64: "N", successiveFailedLogins: 0, disabled: "N", disabledDateTime: null}, true, null, null)
            row.update()
        })
        const before = account()
        ec.user.logoutUser()
        signIn(OTHER)
        const nameOnly = call("update#UserAccount", {userId: OTHER, userFullName: "Guard Other Renamed"})
        const emailRefused = call("update#UserAccount", {userId: OTHER, emailAddress: "taken-over@example.com"})
        const linkRefused = call("update#UserAccount", {userId: OTHER, externalUserId: "sso-taken-over"})
        const afterRefusal = account()
        // OTHER has no second factor: the password confirms it
        const confirmed = confirm()
        const emailChanged = call("update#UserAccount", {userId: OTHER, emailAddress: "guard-other-new@example.com"})
        const afterConfirm = account().emailAddress
        ec.user.logoutUser()
        signIn(ADMIN)
        const byAdmin = call("update#UserAccount", {userId: OTHER, emailAddress: before.emailAddress, userFullName: before.userFullName})
        return {nameOnly, emailRefused, linkRefused, afterRefusal, before, confirmed, emailChanged, afterConfirm, byAdmin, restored: account().emailAddress}
    },
    then({nameOnly, emailRefused, linkRefused, afterRefusal, before, confirmed, emailChanged, afterConfirm, byAdmin, restored}) {
        nameOnly.errors == ""
        emailRefused.errors.includes("Confirm it is you")
        linkRefused.errors.includes("Confirm it is you")
        afterRefusal.emailAddress == before.emailAddress
        afterRefusal.externalUserId == before.externalUserId
        confirmed.reauthenticated == true
        emailChanged.errors == ""
        afterConfirm == "guard-other-new@example.com"
        byAdmin.errors == ""
        restored == before.emailAddress
    }
})
