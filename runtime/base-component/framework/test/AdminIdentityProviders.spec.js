// Sign-in providers and sign-in policy, org.moqui.admin.ConfigurationServices and AccessServices (mr-boss plan/impl-admin.md
// P10; design-admin §6 SignInPolicy, IdentityProviders, IdentityProviderDetail, §8.1): a role mapping never gives a
// privileged group or a sealed grant, whoever asks, and only a group the actor may assign; a change of an endpoint clears the
// stored secret and leaves the provider disabled until a test passes and an access administrator enables it; a secret is
// never returned; the parts are held apart by permission; the sign-in policy the server runs with is read only, and a
// group's sign-in rules are previewed before they are saved. The sign-in's own bounds (an existing account linked only by a
// verified email, no privileged group given or ended) are cargo's (service::sso tests).
// Run: ./moq test framework --spec AdminIdentityProviders
const C = "org.moqui.admin.ConfigurationServices."
const A = "org.moqui.admin.AccessServices."
const FLOW = "moqui.security.sso.AuthFlow"
const OAUTH = "moqui.security.sso.OauthFlow"
const ROLE_MAP = "moqui.security.sso.AuthFlowRoleMap"
const flowId = "SPEC_IDP_FLOW"
const liveFlowId = "SPEC_IDP_LIVE"
const innerFlowId = "SPEC_IDP_INNER"
const users = {config: "SPEC_IDP_CONFIG", access: "SPEC_IDP_ACCESS", none: "SPEC_IDP_NONE", linked: "SPEC_IDP_LINKED"}
const groups = {
    config: "SPEC_IDP_CONFIGS",
    access: "SPEC_IDP_ACCESSES",
    staff: "SPEC_IDP_STAFF",
    sealed: "SPEC_IDP_SEALED",
    plain: "SPEC_IDP_PLAIN"
}
const secret = "spec-idp-secret-9f3k"

const find = (entityName, conditions = {}) => ec.entity.find(entityName).condition(conditions).disableAuthz()
const one = (entityName, conditions) => find(entityName, conditions).one()
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
function as(userId, action) {
    ec.user.internalLoginUser(userId.toLowerCase().replace(/_/g, "."))
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
const provider = authFlowId => invoke(C + "find#IdentityProviders", {authFlowId}).provider
const audit = (entityName, fieldName, authFlowId) =>
    Array.from(
        find("moqui.entity.EntityAuditLog", {
            changedEntityName: entityName,
            changedFieldName: fieldName,
            pkPrimaryValue: authFlowId
        }).list()
    )

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const [key, userId] of Object.entries(users))
            make("moqui.security.UserAccount", {
                userId,
                username: userId.toLowerCase().replace(/_/g, "."),
                userFullName: "Spec provider " + key,
                externalUserId: key === "linked" ? "spec-sub-linked" : null
            })
        for (const [key, userGroupId] of Object.entries(groups))
            make("moqui.security.UserGroup", {userGroupId, description: "Spec provider " + key})
        const grant = (userGroupId, permissions) =>
            permissions.forEach(userPermissionId =>
                make("moqui.security.UserGroupPermission", {userGroupId, userPermissionId, fromDate: 0})
            )
        grant(groups.config, ["ADMIN_CONNECTION_VIEW", "ADMIN_CONNECTION_MANAGE", "ADMIN_SETTINGS_VIEW"])
        grant(groups.access, ["ADMIN_ACCESS_VIEW", "ADMIN_ACCESS_MANAGE"])
        grant(groups.sealed, ["ADMIN_PASSWORD"])
        make("moqui.security.UserGroupMember", {userGroupId: groups.config, userId: users.config, fromDate: 0})
        make("moqui.security.UserGroupMember", {userGroupId: groups.access, userId: users.access, fromDate: 0})
        // an instance administrator who signs in through a provider only: no password
        make("moqui.security.UserGroupMember", {userGroupId: "ADMIN", userId: users.linked, fromDate: 0})
        for (const userGroupId of [groups.staff, groups.sealed])
            make("moqui.security.CapabilityBundle", {userGroupId, appName: "Spec", description: "Spec provider bundle"})
        // a provider that is live, as if tested and enabled: its secret set and its test mark of another configuration
        make(FLOW, {authFlowId: liveFlowId, authFlowTypeEnumId: "AftOauth", description: "Spec live provider", disabled: "N"})
        make(OAUTH, {
            authFlowId: liveFlowId,
            clientTypeEnumId: "OctOauth20",
            clientId: "spec-client",
            secret,
            authorizeUrl: "https://idp.example.com/authorize",
            tokenUrl: "https://idp.example.com/token",
            userInfoUrl: "https://idp.example.com/userinfo"
        })
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.artifactExecution.disableAuthz()
        const flowIds = [flowId, liveFlowId, innerFlowId]
        for (const entityName of [ROLE_MAP, "moqui.security.sso.AuthFlowFieldMap", OAUTH, "moqui.security.sso.OidcFlow", FLOW])
            ec.entity.find(entityName).condition("authFlowId", "in", flowIds).deleteAll()
        const groupIds = Object.values(groups)
        const userIds = Object.values(users)
        ec.entity.find("moqui.security.CapabilityBundle").condition("userGroupId", "in", groupIds).deleteAll()
        ec.entity.find("moqui.security.UserGroupPermission").condition("userGroupId", "in", groupIds).deleteAll()
        ec.entity.find("moqui.security.UserGroupMember").condition("userId", "in", userIds).deleteAll()
        ec.entity.find("moqui.security.UserGroup").condition("userGroupId", "in", groupIds).deleteAll()
        ec.entity.find("moqui.security.UserAccount").condition("userId", "in", userIds).deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature("a role mapping gives only a registered bundle the actor may assign, never a privileged group or a sealed grant", {
    when() {
        const created = as(users.config, () =>
            invoke(C + "store#IdentityProvider", {
                authFlowId: flowId,
                authFlowTypeEnumId: "AftOauth",
                description: "Spec provider",
                clientTypeEnumId: "OctOauth20",
                clientId: "spec-client",
                secret,
                authorizeUrl: "https://idp.example.com/authorize",
                tokenUrl: "https://idp.example.com/token",
                fieldMapList: [{srcFieldName: "name", dstFieldName: "userFullName"}]
            })
        )
        const mapAs = (userId, roleMapList, extra = {}) =>
            as(userId, () =>
                invoke(
                    C + "store#IdentityProvider",
                    Object.assign({authFlowId: flowId, expectedRevision: provider(flowId).revision, roleMapList}, extra)
                )
            )
        const out = {
            created,
            saml: as(users.config, () =>
                invoke(C + "store#IdentityProvider", {authFlowTypeEnumId: "AftSaml", description: "Spec SAML"})
            ),
            expression: as(users.config, () =>
                invoke(C + "store#IdentityProvider", {
                    authFlowId: flowId,
                    expectedRevision: created.revision,
                    fieldMapList: [{srcFieldName: "ec.user.loginUser('x')", dstFieldName: "userFullName"}]
                })
            ),
            privileged: mapAs(users.access, [{roleName: "admins", userGroupId: "ADMIN"}]),
            // john.doe is an ADMIN member and holds every sealed permission: still never by a sign-in
            privilegedByMember: (() => {
                ec.artifactExecution.enableAuthz()
                try {
                    return invoke(C + "store#IdentityProvider", {
                        authFlowId: flowId,
                        expectedRevision: provider(flowId).revision,
                        roleMapList: [{roleName: "admins", userGroupId: "ADMIN"}]
                    })
                } finally {
                    ec.artifactExecution.disableAuthz()
                }
            })(),
            sealed: mapAs(users.access, [{roleName: "resetters", userGroupId: groups.sealed}]),
            unregistered: mapAs(users.access, [{roleName: "plain", userGroupId: groups.plain}]),
            defaultGroup: mapAs(users.access, undefined, {defaultUserGroupId: "ADMIN"}),
            byConfigure: mapAs(users.config, [{roleName: "staff", userGroupId: groups.staff}]),
            rowsAfterRefusals: find(ROLE_MAP, {authFlowId: flowId}).count()
        }
        out.mapped = mapAs(users.access, [{roleName: "staff", userGroupId: groups.staff}], {
            accountLinkEnumId: "AflVerifiedEmail"
        })
        out.detail = as(users.access, () => provider(flowId))
        out.removed = mapAs(users.access, [])
        out.auditRemoved = audit(ROLE_MAP, "userGroupId", flowId).some(
            row => row.oldValueText === groups.staff && !row.newValueText
        )
        return out
    },
    then(out) {
        out.created.applied == true
        out.created.enabled == false
        out.saml.reasonCode == "UNSUPPORTED_TYPE"
        out.expression.error.includes("A claim is a name")
        out.privileged.reasonCode == "PRIVILEGED_GROUP"
        out.privilegedByMember.reasonCode == "PRIVILEGED_GROUP"
        out.sealed.reasonCode == "SEALED_PERMISSION"
        out.unregistered.reasonCode == "UNREGISTERED_BUNDLE"
        out.defaultGroup.reasonCode == "PRIVILEGED_GROUP"
        out.byConfigure.error.includes("ADMIN_ACCESS_MANAGE is required")
        out.rowsAfterRefusals == 0
        out.mapped.applied == true
        out.detail.roleMapList.length == 1
        out.detail.roleMapList[0].allowed == true
        out.detail.accountLinkEnumId == "AflVerifiedEmail"
        out.detail.fieldMapList[0].dstFieldName == "userFullName"
        out.removed.applied == true
        out.auditRemoved == true
    }
})

feature(
    "a changed endpoint clears the stored secret and leaves the provider disabled until a test passes and it is enabled again",
    {
        when() {
            const store = (userId, parameters) =>
                as(userId, () =>
                    invoke(
                        C + "store#IdentityProvider",
                        Object.assign({authFlowId: liveFlowId, expectedRevision: provider(liveFlowId).revision}, parameters)
                    )
                )
            const out = {before: as(users.config, () => invoke(C + "find#IdentityProviders", {authFlowId: liveFlowId}))}
            out.stale = as(users.config, () =>
                invoke(C + "store#IdentityProvider", {authFlowId: liveFlowId, expectedRevision: "stale", scope: "openid"})
            )
            // a field no secret depends on leaves the provider as it is
            out.scope = store(users.config, {scope: "openid profile"})
            // the token endpoint moves and the secret is not entered again
            out.moved = store(users.config, {tokenUrl: "https://elsewhere.example.net/token"})
            out.flowAfter = one(FLOW, {authFlowId: liveFlowId})
            out.secretAfter = one(OAUTH, {authFlowId: liveFlowId}).secret
            out.secretAudit = audit(OAUTH, "secret", liveFlowId).map(row => row.newValueText)
            out.tokenAudit = audit(OAUTH, "tokenUrl", liveFlowId).some(
                row => row.newValueText === "https://elsewhere.example.net/token"
            )
            out.enableUntested = store(users.access, {enabled: true})
            out.enableByConfigure = store(users.config, {enabled: true})
            // the secret entered again with the endpoint: kept, and still disabled until tested
            out.reentered = store(users.config, {tokenUrl: "https://idp.example.com/token", secret})
            out.secretKept = one(OAUTH, {authFlowId: liveFlowId}).secret === secret
            out.plainHttp = store(users.config, {tokenUrl: "http://idp.example.com/token"})
            out.tested = as(users.config, () =>
                invoke(C + "test#IdentityProvider", {authFlowId: liveFlowId, testRoles: "staff, other"})
            )
            // a linked account in ADMIN signs in by a provider: only an instance administrator enables one
            out.enableBeyond = store(users.access, {enabled: true})
            // named on another provider it is not this one's to sign in (review: a provider's id is linked with its
            // provider), so the access administrator enables this one; the account names none again for what follows
            const linkTo = externalAuthFlowId => {
                const row = ec.entity.makeValue("moqui.security.UserAccount")
                row.setFields({userId: users.linked, externalAuthFlowId}, true, null, null)
                row.update()
            }
            linkTo(flowId)
            out.enableOthers = store(users.access, {enabled: true})
            out.pathOthers = invoke("org.moqui.admin.IdentityServices.check#AdministrationPath", {proposed: {}})
            linkTo(null)
            ec.artifactExecution.enableAuthz()
            try {
                out.enabled = invoke(C + "store#IdentityProvider", {
                    authFlowId: liveFlowId,
                    expectedRevision: provider(liveFlowId).revision,
                    enabled: true
                })
                // the linked administrator has no password: with every provider disabled it is a path no more
                out.path = invoke("org.moqui.admin.IdentityServices.check#AdministrationPath", {
                    proposed: {disabledFlowIds: [liveFlowId]}
                })
            } finally {
                ec.artifactExecution.disableAuthz()
            }
            // a provider that may not use the private network: its test sends nothing to an internal address
            ec.artifactExecution.disableAuthz()
            make(FLOW, {
                authFlowId: innerFlowId,
                authFlowTypeEnumId: "AftOidc",
                description: "Spec inner",
                disabled: "Y",
                allowPrivateNetwork: "N"
            })
            make("moqui.security.sso.OidcFlow", {
                authFlowId: innerFlowId,
                clientTypeEnumId: "OctOther",
                clientId: "spec-client",
                discoveryUri: "https://10.1.2.3/.well-known/openid-configuration"
            })
            out.inner = as(users.config, () => invoke(C + "test#IdentityProvider", {authFlowId: innerFlowId}))
            out.listed = as(users.config, () => invoke(C + "find#IdentityProviders", {}))
            out.none = as(users.none, () => invoke(C + "find#IdentityProviders", {}))
            out.answers = JSON.stringify([out.before, out.moved, out.reentered, out.tested, out.enabled, out.listed])
            return out
        },
        then(out) {
            out.before.provider.enabled == true
            out.before.provider.configuration.secretSet == true
            out.before.provider.configuration.secret === undefined
            out.stale.reasonCode == "STALE_REVISION"
            out.scope.applied == true
            out.scope.enabled == true
            out.moved.applied == true
            out.moved.secretCleared == true
            out.moved.enabled == false
            out.moved.testRequired == true
            out.flowAfter.disabled == "Y"
            !out.secretAfter
            out.secretAudit.some(text => text.includes("cleared"))
            out.tokenAudit == true
            out.enableUntested.reasonCode == "TEST_REQUIRED"
            out.enableByConfigure.error.includes("ADMIN_ACCESS_MANAGE is required")
            out.reentered.applied == true
            out.reentered.secretCleared == false
            out.reentered.enabled == false
            out.secretKept == true
            out.plainHttp.error.includes("must be https")
            out.tested.passed == true
            out.tested.callbackUrl.endsWith("/sso/callback")
            out.tested.grantPreview.length == 2
            out.tested.enabled == false
            out.enableBeyond.reasonCode == "HELD_BEYOND"
            out.enableOthers.applied == true
            // the administrator named on a disabled provider is no path, though another provider is enabled
            out.pathOthers.pathsBefore == out.path.pathsBefore - 1
            out.enabled.applied == true
            out.enabled.enabled == true
            out.path.pathsAfter == out.path.pathsBefore - 1
            out.inner.passed == false
            out.inner.checkList.some(row => row.check === "Provider" && row.text.includes("internal address"))
            out.listed.providerList.find(row => row.authFlowId === liveFlowId).readiness == "enabled"
            out.none.error.includes("ADMIN_CONNECTION_VIEW")
            !out.answers.includes(secret)
        }
    }
)

feature("the sign-in policy the server runs with is read only, and a group's sign-in rules are previewed before they are saved", {
    when() {
        const policy = userId => as(userId, () => invoke(C + "get#EffectiveSettings", {settingGroup: "Sign-in policy"}))
        const out = {
            config: policy(users.config),
            access: policy(users.access),
            accessAll: as(users.access, () => invoke(C + "get#EffectiveSettings", {})),
            none: policy(users.none),
            change: (() => {
                ec.artifactExecution.enableAuthz()
                try {
                    return invoke(C + "store#InstancePreference", {
                        settingKey: "signin.minLength",
                        value: "4",
                        expectedRevision: ""
                    })
                } finally {
                    ec.artifactExecution.disableAuthz()
                }
            })()
        }
        // the access administrators' own group: its one member has no factor
        out.preview = as(users.access, () =>
            invoke(A + "preview#GroupSignInRules", {userGroupId: groups.staff, requireAuthcFactor: "Y"})
        )
        ec.artifactExecution.disableAuthz()
        make("moqui.security.UserGroupMember", {userGroupId: groups.staff, userId: users.none, fromDate: 0})
        out.previewMember = as(users.access, () =>
            invoke(A + "preview#GroupSignInRules", {userGroupId: groups.staff, requireAuthcFactor: "Y", ipAllowed: "10.0.0.*"})
        )
        out.previewBad = as(users.access, () =>
            invoke(A + "preview#GroupSignInRules", {userGroupId: groups.staff, ipAllowed: "not an address"})
        )
        out.previewPlain = as(users.access, () =>
            invoke(A + "preview#GroupSignInRules", {userGroupId: groups.plain, requireAuthcFactor: "Y"})
        )
        out.previewNone = as(users.none, () => invoke(A + "preview#GroupSignInRules", {userGroupId: groups.staff}))
        out.stored = one("moqui.security.UserGroup", {userGroupId: groups.staff}).requireAuthcFactor
        return out
    },
    then(out) {
        out.config.settingList.find(row => row.settingKey === "signin.minLength").value == "8"
        out.config.settingList.find(row => row.settingKey === "signin.minLength").source == "default"
        out.config.settingList.find(row => row.settingKey === "signin.minLength").editable == false
        out.config.settingList.find(row => row.settingKey === "signin.sessionMinutes").value == "60"
        out.config.settingList.every(row => !row.masked)
        out.config.settingList.length == 13
        out.access.settingList.length == 13
        out.accessAll.settingList.every(row => ["Sign-in", "Sign-in policy"].includes(row.settingGroup))
        out.none.error.includes("ADMIN_SETTINGS_VIEW")
        out.change.reasonCode == "NOT_REGISTERED"
        out.preview.allowed == true
        out.preview.memberCount == 0
        out.previewMember.memberCount == 1
        out.previewMember.membersWithoutFactor == 1
        out.previewMember.changed == true
        out.previewBad.reasonCode == "IP_INVALID"
        out.previewPlain.reasonCode == "UNREGISTERED_BUNDLE"
        out.previewNone.error.includes("ADMIN_ACCESS_VIEW")
        out.stored != "Y"
    }
})
