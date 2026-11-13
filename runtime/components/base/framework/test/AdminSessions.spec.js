// Sessions and account access, org.moqui.admin.IdentityServices (mr-boss plan/impl-admin.md P4; design-admin §4 Sessions,
// UserDetail AccessCredentials; F3, F4, F42): the caller's own sessions without any permission and another user's only with
// ADMIN_IDENTITY_VIEW, named by visit and never by cookie or token; a revoked session's visit ended and its web session gone,
// so its next request starts over signed out; and revoke#AccountAccess ending another user's sessions, keys, app passwords,
// SSH keys and agent triggers at once, refused without ADMIN_IDENTITY_MANAGE. The session refresh that signs a disabled
// account out on its next request, and the cluster end of a session, are cargo's (user::session_tests
// every_session_of_a_user_ends_and_a_disabled_account_signs_out).
// Run: ./moq test framework --spec AdminSessions
const S = "org.moqui.admin.IdentityServices."
const operator = "spec.sessions.operator"
const operatorId = "SPEC_SES_OPERATOR"
const viewerId = "SPEC_SES_VIEWER"
const targetId = "SPEC_SES_TARGET"
const sealedId = "SPEC_SES_SEALED"
const visit = n => "SPEC_SES_VISIT_" + n

const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const one = (entityName, conditions) => ec.entity.find(entityName).condition(conditions).disableAuthz().one()
function as(username, action) {
    ec.user.internalLoginUser(username)
    ec.artifactExecution.enableAuthz()
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
}
function invoke(name, parameters = {}) {
    try {
        const result = ec.service.sync().name(name).parameters(parameters).call() || {}
        const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
        ec.message.clearErrors()
        return Object.assign({}, result, {error})
    } catch (e) {
        ec.message.clearErrors()
        return {error: String(e.message || e)}
    }
}
// a browser session of userId: its visit and its web session row
function browser(n, userId, agent) {
    make("moqui.server.Visit", {
        visitId: visit(n),
        userId,
        fromDate: ec.user.nowTimestamp,
        initialUserAgent: agent,
        clientIpAddress: "203.0.113." + n
    })
    make("moqui.server.HttpSession", {sessionId: "SPEC_SES_SESSION_" + n, userId, visitId: visit(n), sessionToken: "secret-" + n})
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        make("moqui.security.UserAccount", {userId: operatorId, username: operator, userFullName: "Spec identity operator"})
        make("moqui.security.UserAccount", {
            userId: viewerId,
            username: "spec.sessions.viewer",
            userFullName: "Spec identity viewer"
        })
        make("moqui.security.UserAccount", {
            userId: targetId,
            username: "spec.sessions.target",
            userFullName: "Spec session target"
        })
        make("moqui.security.UserGroup", {userGroupId: "SPEC_SES_OPERATORS", description: "Spec identity operators"})
        make("moqui.security.UserGroup", {userGroupId: "SPEC_SES_VIEWERS", description: "Spec identity viewers"})
        for (const permission of ["ADMIN_IDENTITY_VIEW", "ADMIN_IDENTITY_MANAGE"])
            make("moqui.security.UserGroupPermission", {
                userGroupId: "SPEC_SES_OPERATORS",
                userPermissionId: permission,
                fromDate: 0
            })
        make("moqui.security.UserGroupPermission", {
            userGroupId: "SPEC_SES_VIEWERS",
            userPermissionId: "ADMIN_IDENTITY_VIEW",
            fromDate: 0
        })
        make("moqui.security.UserGroupMember", {userGroupId: "SPEC_SES_OPERATORS", userId: operatorId, fromDate: 0})
        make("moqui.security.UserGroupMember", {userGroupId: "SPEC_SES_VIEWERS", userId: viewerId, fromDate: 0})
        browser(1, targetId, "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) Firefox/140.0")
        browser(2, targetId, "Mozilla/5.0 (Windows NT 10.0) Chrome/130.0")
        browser(3, targetId, "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0) Safari/604.1")
        browser(4, operatorId, "Mozilla/5.0 (X11; Linux x86_64) Firefox/140.0")
        browser(5, operatorId, "Mozilla/5.0 (X11; Linux x86_64) Chrome/130.0")
        // an account holding a sealed permission the operator lacks (bounded delegation, review R1)
        make("moqui.security.UserAccount", {
            userId: sealedId,
            username: "spec.sessions.sealed",
            userFullName: "Spec sealed holder"
        })
        make("moqui.security.UserGroup", {userGroupId: "SPEC_SES_SEALED", description: "Spec sealed holders"})
        make("moqui.security.UserGroupPermission", {
            userGroupId: "SPEC_SES_SEALED",
            userPermissionId: "ADMIN_IDENTITY_GLOBAL",
            fromDate: 0
        })
        make("moqui.security.UserGroupMember", {userGroupId: "SPEC_SES_SEALED", userId: sealedId, fromDate: 0})
        browser(6, sealedId, "Mozilla/5.0 (X11; Linux x86_64) Firefox/140.0")
        // the target's credentials and an agent trigger that runs as the target
        make("moqui.security.UserLoginKey", {
            loginKey: "SPEC_SES_KEY_HASH",
            userId: targetId,
            fromDate: ec.user.nowTimestamp,
            description: "Spec token"
        })
        make("moqui.security.UserAppPassword", {
            appPasswordId: "SPEC_SES_APP",
            userId: targetId,
            name: "Spec phone mail",
            passwordHash: "x",
            createdDate: ec.user.nowTimestamp
        })
        make("moqui.security.UserSshKey", {
            sshKeyId: "SPEC_SES_SSH",
            userId: targetId,
            fingerprint: "SHA256:spec",
            publicKey: "ssh-ed25519 AAAA spec",
            fromDate: ec.user.nowTimestamp
        })
        make("moqui.agent.AgentTrigger", {
            triggerId: "SPEC_SES_TRIGGER",
            agentId: "SPEC_SES_AGENT",
            runAsUserId: targetId,
            description: "Spec trigger",
            paused: "N"
        })
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        for (const [entityName, conditions] of [
            ["moqui.agent.AgentTrigger", {triggerId: "SPEC_SES_TRIGGER"}],
            ["moqui.security.UserSshKey", {userId: targetId}],
            ["moqui.security.UserAppPassword", {userId: targetId}],
            ["moqui.security.UserLoginKey", {userId: targetId}],
            ["moqui.server.HttpSession", {userId: targetId}],
            ["moqui.server.HttpSession", {userId: operatorId}],
            ["moqui.server.Visit", {userId: targetId}],
            ["moqui.server.Visit", {userId: operatorId}],
            ["moqui.server.HttpSession", {userId: sealedId}],
            ["moqui.server.Visit", {userId: sealedId}]
        ])
            ec.entity.find(entityName).condition(conditions).disableAuthz().deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature("a user lists and ends their own sessions with no permission, never another user's", {
    when() {
        return as("spec.sessions.target", () => ({
            own: invoke(S + "find#Sessions"),
            other: invoke(S + "find#Sessions", {userId: operatorId}),
            // the operator's session is not the target's to end: refused and counted, not ended
            foreign: invoke(S + "revoke#Sessions", {sessionKeyList: [visit(4)]}),
            ended: invoke(S + "revoke#Sessions", {sessionKeyList: [visit(3)]})
        }))
    },
    then({own, other, foreign, ended}) {
        own.error == ""
        own.sessionList.length == 3 && own.activeCount == 3
        own.sessionList.every(row => row.userId == "SPEC_SES_TARGET" && row.state == "active")
        own.sessionList.some(row => row.clientSummary == "Firefox on macOS")
        // a session is its visit: no cookie, token or user agent comes back
        !JSON.stringify(own).includes("secret-") && !JSON.stringify(own).includes("SPEC_SES_SESSION_")
        !JSON.stringify(own).includes("Mozilla")
        other.error.includes("ADMIN_IDENTITY_VIEW")
        foreign.refusedCount == 1 && foreign.sessionCount == 0
        one("moqui.server.HttpSession", {sessionId: "SPEC_SES_SESSION_4"}) != null
        ended.sessionCount == 1 && ended.refusedCount == 0
        one("moqui.server.HttpSession", {sessionId: "SPEC_SES_SESSION_3"}) == null
        one("moqui.server.Visit", {visitId: visit(3)}).thruDate != null
    }
})

feature("an identity operator reads another user's sessions and ends one, a viewer only reads them", {
    when() {
        return {
            viewer: as("spec.sessions.viewer", () => ({
                found: invoke(S + "find#Sessions", {userId: targetId, activeOnly: true}),
                // every account's, narrowed to those whose name holds the text, whatever its case
                named: invoke(S + "find#Sessions", {allUsers: true, text: "SESSIONS.TARGET", activeOnly: true}),
                revoked: invoke(S + "revoke#Sessions", {sessionKeyList: [visit(2)]}),
                credentials: invoke(S + "get#AccountCredentials", {userId: targetId})
            })),
            operator: as(operator, () => ({
                listed: invoke(S + "find#Sessions", {userIdList: [targetId], activeOnly: true}),
                revoked: invoke(S + "revoke#Sessions", {userId: targetId, sessionKeyList: [visit(2)]})
            }))
        }
    },
    then({viewer, operator}) {
        viewer.found.error == "" && viewer.found.sessionList.length >= 1
        viewer.named.sessionList.length >= 1 && viewer.named.sessionList.every(row => row.userId == targetId)
        viewer.revoked.refusedCount == 1 && viewer.revoked.sessionCount == 0
        viewer.credentials.error == ""
        viewer.credentials.credentialList.some(row => row.kind == "accessToken" && row.description == "Spec token")
        viewer.credentials.credentialList.some(row => row.kind == "sshKey" && row.fingerprint == "SHA256:spec")
        !JSON.stringify(viewer.credentials).includes("ssh-ed25519") &&
            !JSON.stringify(viewer.credentials).includes("passwordHash")
        operator.listed.sessionList.every(row => row.userId == "SPEC_SES_TARGET")
        operator.revoked.sessionCount == 1
        one("moqui.server.HttpSession", {sessionId: "SPEC_SES_SESSION_2"}) == null
    }
})

feature("revoke#AccountAccess ends another user's sessions, keys, app passwords, SSH keys and triggers at once", {
    when() {
        const refused = as("spec.sessions.viewer", () => invoke(S + "revoke#AccountAccess", {userId: targetId}))
        const self = as(operator, () => invoke(S + "revoke#AccountAccess", {userId: operatorId}))
        const revoked = as(operator, () => invoke(S + "revoke#AccountAccess", {userId: targetId, reason: "Device lost"}))
        const sealed = as(operator, () => ({
            access: invoke(S + "revoke#AccountAccess", {userId: sealedId}),
            session: invoke(S + "revoke#Sessions", {sessionKeyList: [visit(6)]})
        }))
        return {refused, self, revoked, sealed}
    },
    then({refused, self, revoked, sealed}) {
        refused.error.includes("ADMIN_IDENTITY_MANAGE")
        self.error.includes("your account settings")
        // an account holding a sealed permission the operator lacks is not the operator's to sign out
        sealed.access.error.includes("ADMIN_IDENTITY_GLOBAL")
        sealed.session.refusedCount == 1 && sealed.session.sessionCount == 0
        one("moqui.server.HttpSession", {sessionId: "SPEC_SES_SESSION_6"}) != null
        revoked.error == "" && revoked.failedKinds.length == 0
        revoked.revokedCounts.sessions == 1
        revoked.revokedCounts.loginKeys == 1 && revoked.revokedCounts.appPasswords == 1 && revoked.revokedCounts.sshKeys == 1
        one("moqui.server.HttpSession", {sessionId: "SPEC_SES_SESSION_1"}) == null
        one("moqui.security.UserLoginKey", {loginKey: "SPEC_SES_KEY_HASH"}).thruDate != null
        one("moqui.security.UserAppPassword", {appPasswordId: "SPEC_SES_APP"}).revokedDate != null
        one("moqui.security.UserSshKey", {sshKeyId: "SPEC_SES_SSH"}).thruDate != null
        one("moqui.agent.AgentTrigger", {triggerId: "SPEC_SES_TRIGGER"}).paused == "Y"
        one("moqui.security.UserAccount", {userId: targetId}).hasLoggedOut == "Y"
        // the operator's own sessions are untouched
        one("moqui.server.HttpSession", {sessionId: "SPEC_SES_SESSION_4"}) != null
    }
})
