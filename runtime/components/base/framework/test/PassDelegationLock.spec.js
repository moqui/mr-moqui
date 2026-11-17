// A PASS delegation (public/Account.js authenticateDelegation) holds through a tarpit lock of its account, which anyone's
// failed sign-ins set, and ends when the account is disabled for good (plans/Security-Review-Low.md SEC-PSA-02).
// Run: ./moq test framework --spec PassDelegationLock
const ACCOUNT_JS = "component://framework/service/org/moqui/public/Account.js"
const USER = "PASS_DELEGATION_SPEC"

// A delegation of USER with its account set to `fields`, then authenticated as a PASS server's call does.
function authenticate(fields) {
    const token = ec.user.randomString(40)
    ec.artifactExecution.disableAuthz()
    try {
        let account = ec.entity.find("moqui.security.UserAccount").condition("userId", USER).one()
        if (!account) {
            account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId: USER, username: "pass.delegation.spec", userFullName: "PASS Delegation Spec"},
                true, null, null)
            account.create()
        }
        account.setFields(fields, true, null, null)
        account.update()
        const delegation = ec.entity.makeValue("moqui.security.public.DelegatedSession")
        delegation.setFields({delegationId: ec.user.randomString(32), userId: USER, clientId: "SPEC_CLIENT", siteId: "SPEC_SITE",
            tokenHash: ec.user.singleUseCodeHash(token), issuedAt: ec.user.nowTimestamp,
            expiresAt: ec.user.timestampMillis(ec.user.nowTimestamp) + 900000}, true, null, null)
        delegation.create()
        // as the PASS authority's caller runs it, with authorization off
        ec.resource.script(ACCOUNT_JS).authenticateDelegation(token, "SPEC_CLIENT", "SPEC_SITE")
        return {held: true, said: ""}
    } catch (err) {
        return {held: false, said: String(err.message || err)}
    } finally {
        ec.user.logoutUser()
        ec.message.clearAll()
        ec.artifactExecution.enableAuthz()
    }
}

feature("a tarpit lock keeps the account's delegation, and an account disabled for good ends it", {
    when() {
        return {
            locked: authenticate({disabled: "Y", disabledDateTime: ec.user.nowTimestamp}),
            closed: authenticate({disabled: "Y", disabledDateTime: null}),
            open: authenticate({disabled: "N", disabledDateTime: null}),
        }
    },
    then({locked, closed, open}) {
        locked.held
        !closed.held
        closed.said.includes("Session expired")
        open.held
    }
})
