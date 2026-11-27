// PATCH /rest/s1/moqui/users/{userId} (tools/service/moqui.rest.xml) is org.moqui.impl.UserServices.update#UserAccount (through
// patch#UserAccount, which takes the sign-in fields from a member of ADMIN only; framework SecurityRest.spec.js), not
// the entity update (plans/archive/Security-Review-Medium.md SEC-ADM-81): a member of ADMIN changes another account's email address
// only when that account holds no access beyond theirs (Identity.js heldBeyond) and after a recent sign-in, as on the
// account screens, while the full name changes as before. An empty value clears a stored one, and is a change like any
// other: it does not re-enable such an account or lift its address rule.
// Run: ./moq test tools --spec UsersRest
const PASSWORD = "Users-rest-spec-7!"
const ADMIN = "USERSREST_SPEC_ADMIN"
const OTHER = "USERSREST_SPEC_OTHER"
const SEALED = "USERSREST_SPEC_SEALED"
const SEALED_GROUP = "USERSREST_SPEC_SEALED"
const ACCOUNT = "moqui.security.UserAccount"

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
const account = (userId) => sys(() => ec.entity.find(ACCOUNT).condition("userId", userId).useCache(false).one())
const patch = (userId, body) => {
    const out = screenTest("rest").render("s1/moqui/users/" + userId, body, "patch")
    ec.message.clearAll()
    return out
}

spec({
    setupSpec() {
        sys(() => {
            for (const userId of [ADMIN, OTHER, SEALED])
                make(ACCOUNT, {userId, username: username(userId), userFullName: "Users rest " + userId.slice(17),
                    emailAddress: username(userId) + "@example.com", currentPassword: ec.user.hashPassword(PASSWORD),
                    passwordHashType: "argon2", passwordSalt: "", passwordBase64: "N", disabled: "N", requirePasswordChange: "N"})
            make("moqui.security.UserGroupMember", {userGroupId: "ADMIN", userId: ADMIN, fromDate: 0})
            // a sealed permission ADMIN does not hold
            make("moqui.security.UserGroup", {userGroupId: SEALED_GROUP, description: "Users rest spec sealed"})
            make("moqui.security.UserGroupPermission", {userGroupId: SEALED_GROUP, userPermissionId: "SQL_RUNNER_WEB", fromDate: 0})
            make("moqui.security.UserGroupMember", {userGroupId: SEALED_GROUP, userId: SEALED, fromDate: 0})
        })
        ec.user.internalLoginUser(username(ADMIN))
    },
    cleanupSpec() {
        ec.user.logoutUser()
        sys(() => {
            ec.entity.find("moqui.security.UserGroupMember").condition("userId", "in", [ADMIN, OTHER, SEALED]).deleteAll()
            ec.entity.find("moqui.security.UserGroupPermission").condition("userGroupId", SEALED_GROUP).deleteAll()
            ec.entity.find("moqui.security.UserGroup").condition("userGroupId", SEALED_GROUP).deleteAll()
        })
    }
})

feature("an ADMIN's PATCH of a user takes the name, and changes an email address only by update#UserAccount's rules", {
    when() {
        const renamed = patch(OTHER, {userFullName: "Users rest renamed"})
        // an account holding a sealed permission the caller lacks: refused, with or without a recent sign-in
        const sealed = patch(SEALED, {emailAddress: "taker@example.net"})
        // another account's email address needs the caller's recent sign-in
        const stale = patch(OTHER, {emailAddress: "stale@example.net"})
        ec.service.sync().name("org.moqui.sso.RecentAuthServices.reauthenticate#User").parameters({password: PASSWORD}).disableAuthz().call()
        ec.message.clearAll()
        const confirmed = patch(OTHER, {emailAddress: "confirmed@example.net"})
        const sealedAgain = patch(SEALED, {emailAddress: "taker@example.net"})
        return {renamed, sealed, stale, confirmed, sealedAgain}
    },
    then({renamed, sealed, stale, confirmed, sealedAgain}) {
        renamed.status == 200
        account(OTHER).userFullName == "Users rest renamed"
        sealed.status != 200
        stale.status != 200
        confirmed.status == 200
        sealedAgain.status != 200
        account(SEALED).emailAddress == username(SEALED) + "@example.com"
        account(OTHER).emailAddress == "confirmed@example.net"
    },
})

feature("an empty value given for a stored sign-in field is a change, refused on an account holding more access", {
    when() {
        sys(() => {
            const row = ec.entity.find(ACCOUNT).condition("userId", SEALED).one()
            row.setFields({disabled: "Y", ipAllowed: "10.9.8.7"}, true, null, null)
            row.update()
        })
        return {
            enabled: patch(SEALED, {disabled: ""}),
            unbound: patch(SEALED, {ipAllowed: ""}),
            unforced: patch(SEALED, {requirePasswordChange: ""}),
        }
    },
    then({enabled, unbound, unforced}) {
        enabled.status != 200
        unbound.status != 200
        unforced.status != 200
        account(SEALED).disabled == "Y"
        account(SEALED).ipAllowed == "10.9.8.7"
    },
})
