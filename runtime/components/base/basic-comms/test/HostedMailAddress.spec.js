// A hosted or shared mail account with no incoming server is delivered here: the inbound listener's mail to its address
// is stored in it. So only a holder of EMAIL_ACCOUNT_HOSTED (the ADMIN group), or a call with authorization disabled,
// makes one, gives one another address, or makes an account one (plans/Security-Review.md SEC-MAIL-01); a mail user
// still makes an account of another server.
// The quota of such an account is set by a holder too (SEC-MAIL-07).
// Run: ./moq test basic-comms --spec HostedMailAddress
const USER = "SPEC_HMA_USER"
const HOLDER = "SPEC_HMA_HOLDER"
const MAIL = "org.moqui.mail.EmailClientServices."

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const make = (entityName, fields) => {
    if (ec.entity.find(entityName).condition(fields).count() > 0) return
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
// a call as the user, with authorization on: the user is granted the mail services, as a screen of the app grants them
function as(userId, name, parameters) {
    ec.user.internalLoginUser(username(userId))
    let out = {}
    try {
        out = ec.service.sync().name(MAIL + name).parameters(parameters).call() || {}
    } catch (e) {
        out = { thrown: String(e.message || e) }
    }
    const error = (out.thrown || "") + (ec.message.hasError() ? ec.message.getErrorsString() : "")
    ec.message.clearErrors()
    ec.user.logoutUser()
    return Object.assign({}, out, { error })
}

spec({
    setupSpec() {
        sys(() => {
            make("moqui.security.UserGroup", { userGroupId: "SPEC_HMA_MAIL", description: "Spec hosted mail users" })
            make("moqui.security.ArtifactGroup", { artifactGroupId: "SPEC_HMA_MAIL", description: "Spec mail services" })
            make("moqui.security.ArtifactGroupMember", { artifactGroupId: "SPEC_HMA_MAIL", artifactName: "org\\.moqui\\.mail\\.EmailClientServices\\..*",
                nameIsPattern: "Y", artifactTypeEnumId: "AT_SERVICE", inheritAuthz: "Y" })
            make("moqui.security.ArtifactAuthz", { artifactAuthzId: "SPEC_HMA_MAIL", userGroupId: "SPEC_HMA_MAIL", artifactGroupId: "SPEC_HMA_MAIL",
                authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL" })
            make("moqui.security.UserGroup", { userGroupId: "SPEC_HMA_HOLDERS", description: "Spec hosted mail holders" })
            make("moqui.security.UserGroupPermission", { userGroupId: "SPEC_HMA_HOLDERS", userPermissionId: "EMAIL_ACCOUNT_HOSTED", fromDate: ec.user.nowTimestamp })
            for (const userId of [USER, HOLDER]) {
                make("moqui.security.UserAccount", { userId, username: username(userId), userFullName: "Spec " + userId,
                    emailAddress: username(userId) + "@hosted.example.invalid" })
                make("moqui.security.UserGroupMember", { userGroupId: "SPEC_HMA_MAIL", userId, fromDate: ec.user.nowTimestamp })
            }
            make("moqui.security.UserGroupMember", { userGroupId: "SPEC_HMA_HOLDERS", userId: HOLDER, fromDate: ec.user.nowTimestamp })
        })
    },
})

feature("a mail user is refused an unclaimed hosted address and its re-address; a holder makes one; another server's account is the user's", {
    when() {
        const run = Date.now().toString(36)
        const taken = as(USER, "create#EmailAccount", { accountKindEnumId: "EakHosted", emailAddress: `postmaster-${run}@hosted.example.invalid` })
        const shared = as(USER, "create#EmailAccount", { accountKindEnumId: "EakShared", emailAddress: `hr-${run}@hosted.example.invalid` })
        const external = as(USER, "create#EmailAccount", { accountKindEnumId: "EakExternal", emailAddress: `me-${run}@elsewhere.example.invalid`,
            storeHost: "127.0.0.1", storePort: "1", storeProtocol: "imap", mailPassword: "x" })
        // an account of another server becoming hosted here, and one hosted here taking another address
        const toHosted = as(USER, "update#EmailAccount", { emailAccountId: external.emailAccountId, accountKindEnumId: "EakHosted", storeHost: "" })
        const held = as(HOLDER, "create#EmailAccount", { accountKindEnumId: "EakHosted", emailAddress: `abuse-${run}@hosted.example.invalid` })
        sys(() => ec.entity.find("moqui.basic.email.EmailAccount").condition("emailAccountId", held.emailAccountId).one()
            .set("ownerUserId", USER).update())
        const readdress = as(USER, "update#EmailAccount", { emailAccountId: held.emailAccountId, emailAddress: `ceo-${run}@hosted.example.invalid` })
        const renamed = as(USER, "update#EmailAccount", { emailAccountId: held.emailAccountId, accountName: "Abuse desk" })
        const stored = sys(() => ec.entity.find("moqui.basic.email.EmailAccount").condition("emailAccountId", held.emailAccountId).one())
        return { taken, shared, external, toHosted, held, readdress, renamed, stored, run }
    },
    then({ taken, shared, external, toHosted, held, readdress, renamed, stored, run }) {
        taken.error.includes("mail administrator")
        !taken.emailAccountId
        shared.error.includes("mail administrator")
        external.error == ""
        !!external.emailAccountId
        toHosted.error.includes("mail administrator")
        held.error == ""
        !!held.emailAccountId
        readdress.error.includes("mail administrator")
        renamed.error == ""
        stored.emailAddress == `abuse-${run}@hosted.example.invalid`
        stored.accountName == "Abuse desk"
    },
})

// SEC-MAIL-07: a mailbox quota is set only by a mail administrator; a new one clears the account's full mark, so an edge
// node takes its mail again, and empty is the conf's default
feature("only a mail administrator sets a hosted account's quota, which clears its full mark", {
    when() {
        const run = Date.now().toString(36)
        const held = as(HOLDER, "create#EmailAccount", { accountKindEnumId: "EakHosted", emailAddress: `quota-${run}@hosted.example.invalid` })
        // the user manages it too, but is no mail administrator
        const shared = as(HOLDER, "add#EmailAccountUser", { emailAccountId: held.emailAccountId, userId: USER, permissionEnumId: "EapManage" })
        sys(() => ec.entity.find("moqui.basic.email.EmailAccount").condition("emailAccountId", held.emailAccountId).one()
            .set("quotaFullDate", ec.user.nowTimestamp).update())
        const read = () => sys(() => ec.entity.find("moqui.basic.email.EmailAccount").condition("emailAccountId", held.emailAccountId).one())
        const byOwner = as(USER, "update#EmailAccount", { emailAccountId: held.emailAccountId, quotaBytes: "1" })
        const afterOwner = read()
        const notNumber = as(HOLDER, "update#EmailAccount", { emailAccountId: held.emailAccountId, quotaBytes: "lots" })
        const byHolder = as(HOLDER, "update#EmailAccount", { emailAccountId: held.emailAccountId, quotaBytes: "5368709120" })
        const afterHolder = read()
        const cleared = as(HOLDER, "update#EmailAccount", { emailAccountId: held.emailAccountId, quotaBytes: "" })
        const afterClear = read()
        return { shared, byOwner, afterOwner, notNumber, byHolder, afterHolder, cleared, afterClear }
    },
    then({ shared, byOwner, afterOwner, notNumber, byHolder, afterHolder, cleared, afterClear }) {
        shared.error == ""
        byOwner.error.includes("mail administrator")
        afterOwner.quotaBytes == null
        afterOwner.quotaFullDate != null
        notNumber.error.includes("whole number")
        byHolder.error == ""
        Number(afterHolder.quotaBytes) == 5368709120
        afterHolder.quotaFullDate == null
        cleared.error == ""
        afterClear.quotaBytes == null
    },
})
