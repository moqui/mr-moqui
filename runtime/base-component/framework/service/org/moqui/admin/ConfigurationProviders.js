// The identity provider part of org.moqui.admin.ConfigurationServices (mr-boss plan/design-admin.md §6 IdentityProviders,
// IdentityProviderDetail, §8.1; impl-admin P10): the sign-in providers of moqui.security.sso.AuthFlow with their OpenID
// Connect, OAuth and SAML rows, their identity mappings (AuthFlowFieldMap) and role mappings (AuthFlowRoleMap).
//
// Who controls a provider's configuration controls who signs in as the accounts linked to it, so the parts are held apart:
// - reading needs ADMIN_CONNECTION_VIEW or ADMIN_ACCESS_VIEW (or either manage permission);
// - the configuration (name, client, endpoints, secrets, identity mappings) needs ADMIN_CONNECTION_MANAGE;
// - the role mappings, the default group and the rule that links existing accounts need ADMIN_ACCESS_MANAGE, and each
//   group is one the actor may assign (AccessServices' grant proposal: a registered bundle, its delegable permission, never
//   another's privileged group) and is never privileged and never grants a sealed permission, whoever asks: a sign-in gives
//   neither (the sign-in itself passes such a row over too, crates/moqui-core/src/service/sso.rs);
// - enabling needs ADMIN_ACCESS_MANAGE, a passing test of the configuration as it is now (test#IdentityProvider), role
//   mappings that still hold, and no linked account holding access beyond the actor's (a privileged group the actor is
//   not in, a sealed permission the actor lacks): an instance administrator enables a provider such accounts sign in by;
// - the server reaches a provider started here at no internal address on a production instance (AuthFlow
//   allowPrivateNetwork N, enforced by the sign-in's own client for every request of the flow, with no redirect followed):
//   the discovery URL and the endpoints are the integration administrator's to choose, and a test answers what was found
//   there. An instance administrator allows a provider on the instance's own network (Tools); this service never does.
// - disabling keeps the administration path (IdentityServices: an administrator with no password signs in only through
//   an enabled provider).
//
// The endpoint-change rule (design §8.1): a change of what the stored secret is sent to or stands for (the client type,
// client id, issuer or discovery URL, realm, endpoints, Apple team or key, SAML metadata location or entity id) needs the
// secret entered again in the same save, else the stored one is cleared; either way the provider is saved disabled and its
// test mark is cleared, so nothing is sent to the new endpoint with the old secret and nobody signs in through it until a
// test passes and an access administrator enables it again. A change of what a sign-in is verified by (the token
// signature, the nonce) disables it the same way, the secret kept. A secret is write-only: never returned (secretSet only), never
// logged, and its replacement or clearing is an audit row with no value.
//
// No arbitrary expression: an identity mapping is a claim name onto one of the account fields listed here; a provider whose
// mappings carry an expression or a mapping service (an instance administrator's, in Tools) keeps them and they are read
// only here. Inbound flows (a remote system's, not a sign-in) are listed read only.
//
// A refusal a person can act on is answered (applied false, reasonCode, reason), never written; a missing permission or a
// malformed request is an error. No dependency on Mantle or an app.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationLib.js")
const access = ec.resource.script("component://framework/service/org/moqui/admin/Access.js").lib
const identity = ec.resource.script("component://framework/service/org/moqui/admin/Identity.js")
const {find, one, rows, map, iso, ms, stamp, text, refused} = lib

const FLOW = "moqui.security.sso.AuthFlow"
const FIELD_MAP = "moqui.security.sso.AuthFlowFieldMap"
const ROLE_MAP = "moqui.security.sso.AuthFlowRoleMap"
const ACCOUNT = "moqui.security.UserAccount"
const CONNECTION_VIEW = "ADMIN_CONNECTION_VIEW"
const CONNECTION_MANAGE = "ADMIN_CONNECTION_MANAGE"
const ACCESS_VIEW = "ADMIN_ACCESS_VIEW"
const ACCESS_MANAGE = "ADMIN_ACCESS_MANAGE"

// Each type's row: the fields a stored secret depends on (endpoint), the other fields changed here (those of retest are
// what a sign-in is verified by: a change of one needs a new test too, the secret kept), its write-only secrets, and which
// fields are URLs, flags or enumerations.
const types = {
    AftOidc: {
        entity: "moqui.security.sso.OidcFlow",
        label: "OpenID Connect",
        endpoint: ["clientTypeEnumId", "clientId", "discoveryUri", "realm", "baseUri", "appleTeamId", "appleKeyId"],
        other: ["preferredJwsAlgorithmEnumId", "useNonce"],
        retest: ["preferredJwsAlgorithmEnumId", "useNonce"],
        secrets: ["secret"],
        urls: ["discoveryUri", "baseUri"],
        flags: ["useNonce"],
        enums: {clientTypeEnumId: "OidcClientType", preferredJwsAlgorithmEnumId: "OidcJwsAlgorithm"}
    },
    AftOauth: {
        entity: "moqui.security.sso.OauthFlow",
        label: "OAuth",
        endpoint: ["clientTypeEnumId", "clientId", "authorizeUrl", "tokenUrl", "userInfoUrl"],
        other: ["scope"],
        retest: [],
        secrets: ["secret"],
        urls: ["authorizeUrl", "tokenUrl", "userInfoUrl"],
        flags: [],
        enums: {clientTypeEnumId: "OauthClientType"}
    },
    AftSaml: {
        entity: "moqui.security.sso.SamlFlow",
        label: "SAML",
        endpoint: ["serviceProviderEntityId", "identityProviderMetadataLocation"],
        other: ["forceAuth", "passive"],
        retest: ["forceAuth", "passive"],
        secrets: ["keystorePassword", "privateKeyPassword"],
        urls: ["identityProviderMetadataLocation"],
        flags: ["forceAuth", "passive"],
        enums: {}
    }
}
// The types a provider is started with here: the ones this server signs in by. A SAML row is kept and listed, and its
// test says that SAML is not available in this build.
const creatableTypes = ["AftOidc", "AftOauth"]
// The account fields a claim may be mapped onto.
const mappableFields = {
    userFullName: "Full name",
    emailAddress: "Email address",
    locale: "Locale",
    timeZone: "Time zone"
}
const secretFields = ["secret", "keystorePassword", "privateKeyPassword"]
const has = permission => ec.user.hasPermission(permission)
const given = value => value !== undefined && value !== null
const yes = value => value === true || value === "true" || value === "Y"
const no = value => value === false || value === "false" || value === "N"

function requireRead() {
    lib.signedIn()
    if (![CONNECTION_VIEW, CONNECTION_MANAGE, ACCESS_VIEW, ACCESS_MANAGE].some(has))
        throw new Error(CONNECTION_VIEW + " or " + ACCESS_VIEW + " is required")
}

// ===== reading =====
const subRow = flow =>
    types[flow.authFlowTypeEnumId] ? one(types[flow.authFlowTypeEnumId].entity, {authFlowId: flow.authFlowId}) : null
const fieldMaps = authFlowId =>
    rows(FIELD_MAP, {authFlowId}).sort((a, b) => String(a.ruleSeqId).localeCompare(String(b.ruleSeqId)))
const roleMaps = authFlowId => rows(ROLE_MAP, {authFlowId}).sort((a, b) => String(a.roleName).localeCompare(String(b.roleName)))
const advancedMap = row => !!text(row.dstFieldExpression) || !!text(row.mappingServiceRegisterId)
// What a passing test stands for: the type, every field a secret depends on or a sign-in is verified by, and whether each
// secret is set; never a secret's value.
function fingerprint(flow, sub) {
    const type = types[flow.authFlowTypeEnumId]
    if (!type) return ""
    const parts = [flow.authFlowTypeEnumId]
    for (const field of type.endpoint.concat(type.retest)) parts.push(field + "=" + text(sub?.[field]))
    for (const field of type.secrets) parts.push(field + (text(sub?.[field]) ? "+" : "-"))
    return ec.resource.digestText(parts.join("\n"), "sha256")
}
// A provider's revision: its row's, its type row's and its mappings', so a change of any is seen.
function revisionOf(flow, sub, fields, roles) {
    const stamps = [stamp(flow), stamp(sub)]
    for (const row of fields) stamps.push(row.ruleSeqId + ":" + stamp(row))
    for (const row of roles) stamps.push(row.roleName + ":" + stamp(row))
    return ec.resource.digestText(stamps.join("|"), "sha256").slice(0, 24)
}
// What a provider's row lacks before a test is worth running.
function incomplete(flow, sub) {
    const type = flow.authFlowTypeEnumId
    if (!types[type]) return "The provider has no supported type"
    if (!sub) return "The provider has no configuration yet"
    if (type === "AftSaml") return text(sub.identityProviderMetadataLocation) ? null : "Give the provider's metadata URL"
    if (!text(sub.clientId)) return "Give the client ID the provider issued"
    return null
}
const readinessTexts = {
    enabled: "Enabled",
    untested: "Enabled, not tested since its configuration changed",
    tested: "Tested, not enabled",
    needsTest: "Disabled until a test passes",
    incomplete: "Incomplete",
    unavailable: "Not available in this build",
    inbound: "Inbound flow, read only"
}
function readiness(flow, sub) {
    if (flow.inbound === "Y") return "inbound"
    const tested = !!flow.testedConfigHash && flow.testedConfigHash === fingerprint(flow, sub)
    if (flow.disabled !== "Y") return tested ? "enabled" : "untested"
    if (flow.authFlowTypeEnumId === "AftSaml") return "unavailable"
    if (incomplete(flow, sub)) return "incomplete"
    return tested ? "tested" : "needsTest"
}
const rootUrl = () => {
    try {
        return ec.web ? String(ec.web.getWebappRootUrl(true, null) || "").replace(/\/+$/, "") : ""
    } catch (error) {
        return ""
    }
}
const enumName = (enumId, names) => (enumId ? names.get(enumId) || enumId : "")
function enumNames() {
    const names = new Map()
    for (const enumTypeId of ["AuthFlowType", "OidcClientType", "OauthClientType", "OidcJwsAlgorithm", "AuthFlowAccountLink"])
        for (const row of rows("moqui.basic.Enumeration", {enumTypeId})) names.set(row.enumId, row.description || row.enumId)
    return names
}
function described(flow, context) {
    const sub = subRow(flow)
    const state = readiness(flow, sub)
    const type = types[flow.authFlowTypeEnumId]
    return {
        authFlowId: flow.authFlowId,
        description: flow.description || flow.authFlowId,
        authFlowTypeEnumId: flow.authFlowTypeEnumId || null,
        typeName: type?.label || enumName(flow.authFlowTypeEnumId, context.names) || "Unknown",
        clientTypeName: enumName(sub?.clientTypeEnumId, context.names),
        enabled: flow.disabled !== "Y" && flow.inbound !== "Y",
        inbound: flow.inbound === "Y",
        readiness: state,
        readinessText: readinessTexts[state],
        callbackUrl: flow.inbound === "Y" ? "" : context.root ? context.root + "/sso/callback" : "/sso/callback",
        // whether the server may reach the provider at an internal address (an instance administrator's choice)
        privateNetwork: flow.allowPrivateNetwork !== "N",
        sequenceNum: flow.sequenceNum ?? null,
        testedDate: iso(flow.testedDate),
        roleMapCount: context.roleCounts.get(flow.authFlowId) || 0
    }
}
function listContext() {
    const roleCounts = new Map()
    for (const row of rows(ROLE_MAP)) roleCounts.set(row.authFlowId, (roleCounts.get(row.authFlowId) || 0) + 1)
    return {names: enumNames(), root: rootUrl(), roleCounts}
}

// ===== the groups a sign-in may give =====
// Each group a role mapping or the default group names, as the actor may map it: the grant proposal's rules for the actor,
// and never a privileged group or one that grants a sealed permission now, whoever asks.
function groupChecks(groupIds) {
    const ids = Array.from(new Set(groupIds.map(text).filter(Boolean)))
    const found = new Map()
    if (!ids.length) return found
    const privileged = access.privilegedIds()
    const sealed = access.sealedIds()
    for (const row of access.proposalBundles({bundleGroupIdList: ids})) {
        const check = {
            userGroupId: row.bundleGroupId,
            description: row.description || row.bundleGroupId,
            appName: row.appName || "",
            allowed: row.allowed,
            reasonCode: row.reasonCode || null,
            reason: row.reason || null
        }
        const sealedGrants = rows("moqui.security.UserGroupPermission", {userGroupId: row.bundleGroupId})
            .filter(grant => access.current(grant) && sealed.has(grant.userPermissionId))
            .map(grant => grant.userPermissionId)
        if (privileged.has(row.bundleGroupId))
            Object.assign(check, {
                allowed: false,
                reasonCode: "PRIVILEGED_GROUP",
                reason: row.bundleGroupId + " is a privileged group: a sign-in never gives its membership, a member does"
            })
        else if (sealedGrants.length)
            Object.assign(check, {
                allowed: false,
                reasonCode: "SEALED_PERMISSION",
                reason:
                    row.bundleGroupId +
                    " grants " +
                    Array.from(new Set(sealedGrants)).join(", ") +
                    ", sealed: a sign-in never gives it, a holder does"
            })
        found.set(row.bundleGroupId, check)
    }
    return found
}
// The registered bundles a role mapping may name, for the picker: those the actor may map.
function groupOptions() {
    const ids = Array.from(access.bundles().keys())
    return Array.from(groupChecks(ids).values())
        .filter(check => check.allowed)
        .map(check => ({
            value: check.userGroupId,
            label: (check.appName ? check.appName + ": " : "") + check.description
        }))
        .sort((a, b) => a.label.localeCompare(b.label))
}
// The accounts the provider signs in (an external identity it is named on, UserAccount.externalAuthFlowId, or one that
// names no provider yet, which the first provider to sign in with its id takes) that hold access beyond the actor's: in a
// privileged group the actor is not in, or in a group granting a sealed permission the actor lacks. Whoever enables a
// provider decides who signs in as them. An account another provider is named on is not this provider's: the sign-in
// never finds it (crates/moqui-core/src/service/sso.rs).
function linkedBeyond(authFlowId) {
    const linked = new Set(
        Array.from(
            find(ACCOUNT)
                .condition("externalUserId", "is-not-null", null)
                .selectField("userId,externalUserId,externalAuthFlowId")
                .list()
        )
            .filter(row => !!text(row.externalUserId) && (!text(row.externalAuthFlowId) || row.externalAuthFlowId === authFlowId))
            .map(row => row.userId)
    )
    if (!linked.size) return 0
    const groupIds = new Set(Array.from(access.privilegedIds()).filter(groupId => !ec.user.isInGroup(groupId)))
    const lacking = Array.from(access.sealedIds()).filter(permission => !has(permission))
    if (lacking.length)
        for (const grant of Array.from(
            find("moqui.security.UserGroupPermission").condition("userPermissionId", "in", lacking).list()
        ))
            if (access.current(grant)) groupIds.add(grant.userGroupId)
    if (!groupIds.size) return 0
    const beyond = new Set()
    for (const member of Array.from(
        find("moqui.security.UserGroupMember").condition("userGroupId", "in", Array.from(groupIds)).list()
    ))
        if (access.open(member) && linked.has(member.userId)) beyond.add(member.userId)
    return beyond.size
}

// ===== find#IdentityProviders =====
function detail(flow, context) {
    const sub = subRow(flow)
    const type = types[flow.authFlowTypeEnumId]
    const fields = fieldMaps(flow.authFlowId)
    const roles = roleMaps(flow.authFlowId)
    const checks = groupChecks(roles.map(row => row.userGroupId).concat([flow.defaultUserGroupId]))
    const configuration = {}
    if (type && sub) {
        for (const field of type.endpoint.concat(type.other))
            configuration[field] = type.flags.includes(field) ? sub[field] === "Y" : text(sub[field])
        // a secret is never returned: only whether one is set
        for (const field of type.secrets) configuration[field + "Set"] = !!text(sub[field])
        if (flow.authFlowTypeEnumId === "AftSaml") configuration.keystoreLocationSet = !!text(sub.keystoreLocation)
    }
    const tester = flow.testedByUserId ? one(ACCOUNT, {userId: flow.testedByUserId}) : null
    return Object.assign(described(flow, context), {
        iconName: flow.iconName || "",
        accountLinkEnumId: flow.accountLinkEnumId || "AflNever",
        accountLinkName: enumName(flow.accountLinkEnumId || "AflNever", context.names),
        defaultUserGroupId: flow.defaultUserGroupId || "",
        defaultGroup: flow.defaultUserGroupId ? checks.get(flow.defaultUserGroupId) || null : null,
        configuration,
        problem: flow.inbound === "Y" ? null : incomplete(flow, sub),
        tested: !!flow.testedConfigHash && flow.testedConfigHash === fingerprint(flow, sub),
        testedByName: tester ? tester.userFullName || tester.username : null,
        fieldMapList: fields.map(row => ({
            ruleSeqId: row.ruleSeqId,
            srcFieldName: row.srcFieldName || "",
            dstFieldName: row.dstFieldName || "",
            dstFieldLabel: mappableFields[row.dstFieldName] || row.dstFieldName || "",
            // an expression or a mapping service is an instance administrator's: shown as such, never its text
            advanced: advancedMap(row)
        })),
        fieldMapsAdvanced: fields.some(advancedMap),
        roleMapList: roles.map(row => {
            const check = checks.get(row.userGroupId)
            return {
                roleName: row.roleName,
                userGroupId: row.userGroupId || "",
                groupName: check?.description || row.userGroupId || "",
                appName: check?.appName || "",
                allowed: !!check?.allowed,
                reasonCode: check?.reasonCode || null,
                reason: check?.reason || null
            }
        }),
        revision: revisionOf(flow, sub, fields, roles)
    })
}
// find#IdentityProviders: the sign-in providers, each with its type, state and callback, narrowed by text, type and state;
// with authFlowId, that provider's configuration (secrets as secretSet only), mappings and the options its forms offer.
function findIdentityProviders(p) {
    requireRead()
    const context = listContext()
    const canConfigure = has(CONNECTION_MANAGE)
    const canMap = has(ACCESS_MANAGE)
    const out = {canConfigure, canMap, canEnable: canMap}
    out.typeOptions = creatableTypes.map(value => ({value, label: types[value].label}))
    const authFlowId = text(p.authFlowId)
    if (authFlowId) {
        const flow = one(FLOW, {authFlowId})
        if (!flow) return Object.assign(out, {providerList: [], provider: null})
        const provider = detail(map(flow), context)
        const enumOptions = enumTypeId =>
            rows("moqui.basic.Enumeration", {enumTypeId})
                .sort(
                    (a, b) =>
                        (a.sequenceNum ?? 999) - (b.sequenceNum ?? 999) || String(a.description).localeCompare(b.description)
                )
                .map(row => ({value: row.enumId, label: row.description || row.enumId}))
        const type = types[provider.authFlowTypeEnumId]
        return Object.assign(out, {
            providerList: [provider],
            provider,
            clientTypeOptions: type?.enums.clientTypeEnumId ? enumOptions(type.enums.clientTypeEnumId) : [],
            algorithmOptions: type?.enums.preferredJwsAlgorithmEnumId ? enumOptions("OidcJwsAlgorithm") : [],
            accountLinkOptions: enumOptions("AuthFlowAccountLink"),
            localFieldOptions: Object.entries(mappableFields).map(([value, label]) => ({value, label})),
            groupOptions: canMap ? groupOptions() : []
        })
    }
    const term = text(p.text).toLowerCase()
    const state = text(p.state)
    const providerList = rows(FLOW)
        .map(map)
        .map(flow => described(flow, context))
        .filter(row => !term || [row.authFlowId, row.description].some(value => String(value).toLowerCase().includes(term)))
        .filter(row => !p.authFlowTypeEnumId || row.authFlowTypeEnumId === p.authFlowTypeEnumId)
        .filter(row => !state || (state === "enabled") === row.enabled)
        .sort((a, b) => (a.sequenceNum ?? 999) - (b.sequenceNum ?? 999) || a.description.localeCompare(b.description))
        .slice(0, 200)
    return Object.assign(out, {providerList, provider: null})
}

// ===== store#IdentityProvider =====
const controls = /[\u0000-\u001f\u007f]/
function plainText(value, what, maxLength = 255) {
    const found = text(value)
    if (controls.test(found) || found.includes("${")) throw new Error(what + " may not hold control characters or ${")
    if (found.length > maxLength) throw new Error(what + " is longer than " + maxLength + " characters")
    return found
}
// An https URL with a host and no credentials or fragment; http only for this machine on an instance that is not
// production (a provider run beside a development server), so a production server is never pointed at its own ports.
function urlText(value, what) {
    const found = plainText(value, what)
    if (!found) return ""
    const parts = /^(https?):\/\/([^\/?#@\s]+)(\/[^\s#]*)?(\?[^\s#]*)?$/.exec(found)
    if (!parts) throw new Error(what + " is not an http(s) URL with a host and no credentials or fragment")
    const host = parts[2].replace(/:\d+$/, "").toLowerCase()
    const local = ["localhost", "127.0.0.1", "[::1]"].includes(host)
    if (parts[1] === "http" && (!local || text(System.getProperty("instance_purpose")) === "production"))
        throw new Error(what + " must be https")
    return found
}
function enumValue(value, enumTypeId, what) {
    const found = text(value)
    if (found && !one("moqui.basic.Enumeration", {enumId: found, enumTypeId}))
        throw new Error(what + " is not one of its options")
    return found
}
// The type row's fields a save gives, validated: {field: value}, only those that differ from the stored row.
function configurationChanges(p, type, sub) {
    const changes = {}
    for (const field of type.endpoint.concat(type.other)) {
        if (!given(p[field])) continue
        let value
        if (type.flags.includes(field)) value = yes(p[field]) ? "Y" : "N"
        else if (type.urls.includes(field)) value = urlText(p[field], field)
        else if (type.enums[field]) value = enumValue(p[field], type.enums[field], field)
        else if (field === "scope") {
            value = plainText(p[field], field)
            if (!/^[\w.:\/ -]*$/.test(value)) throw new Error("scope is space separated names")
        } else value = plainText(p[field], field)
        const stored = type.flags.includes(field) ? (sub?.[field] === "Y" ? "Y" : "N") : text(sub?.[field])
        if (value !== stored) changes[field] = value || null
    }
    return changes
}
// The identity mappings a save gives, normalized: [{srcFieldName, dstFieldName}], a claim name onto an account field.
function fieldMapRows(list) {
    const found = Array.from(list || []).map(row => ({
        srcFieldName: text(row?.srcFieldName),
        dstFieldName: text(row?.dstFieldName)
    }))
    const seen = new Set()
    for (const row of found) {
        if (!/^[A-Za-z_][A-Za-z0-9_]{0,99}$/.test(row.srcFieldName))
            throw new Error("A claim is a name of letters, digits and underscores: " + (row.srcFieldName || "(empty)"))
        if (!Object.hasOwn(mappableFields, row.dstFieldName))
            throw new Error("A claim is mapped onto one of: " + Object.values(mappableFields).join(", "))
        if (seen.has(row.dstFieldName)) throw new Error(mappableFields[row.dstFieldName] + " is mapped twice")
        seen.add(row.dstFieldName)
    }
    return found
}
function roleMapRows(list) {
    const found = Array.from(list || []).map(row => ({roleName: text(row?.roleName), userGroupId: text(row?.userGroupId)}))
    const seen = new Set()
    for (const row of found) {
        if (!row.roleName || !row.userGroupId) throw new Error("A role mapping names a remote role and a group")
        plainText(row.roleName, "A remote role")
        if (seen.has(row.roleName)) throw new Error("The remote role " + row.roleName + " is mapped twice")
        seen.add(row.roleName)
    }
    return found.sort((a, b) => a.roleName.localeCompare(b.roleName))
}
const sameRows = (a, b, keys) =>
    a.length === b.length && a.every((row, index) => keys.every(key => text(row[key]) === text(b[index][key])))
function write(entityName, fields, create) {
    const value = ec.entity.makeValue(entityName)
    value.setFields(fields, true, null, null)
    if (create) value.create()
    else value.update()
}
const auditedFields = [
    "disabled",
    "accountLinkEnumId",
    "defaultUserGroupId",
    "clientId",
    "discoveryUri",
    "realm",
    "baseUri",
    "authorizeUrl",
    "tokenUrl",
    "userInfoUrl",
    "serviceProviderEntityId",
    "identityProviderMetadataLocation",
    "userGroupId"
]

// store#IdentityProvider: a new provider (disabled), or changes of one at the revision read; see the head of this file for
// who may change which part and the endpoint-change rule.
function storeIdentityProvider(p) {
    lib.signedIn()
    const canConfigure = has(CONNECTION_MANAGE)
    const canMap = has(ACCESS_MANAGE)
    if (!canConfigure && !canMap) throw new Error(CONNECTION_MANAGE + " or " + ACCESS_MANAGE + " is required")
    const needConfigure = () => {
        if (!canConfigure) throw new Error(CONNECTION_MANAGE + " is required to change a provider's configuration")
    }
    const needMap = what => {
        if (!canMap) throw new Error(ACCESS_MANAGE + " is required to " + what)
    }
    const reason = lib.reasonText(p)
    let authFlowId = text(p.authFlowId)
    const stored = authFlowId ? find(FLOW, {authFlowId}).forUpdate(true).one() : null
    const creating = !stored
    let flow
    if (creating) {
        needConfigure()
        const typeEnumId = text(p.authFlowTypeEnumId)
        if (!creatableTypes.includes(typeEnumId))
            return refused(
                "UNSUPPORTED_TYPE",
                "A provider is started as " +
                    creatableTypes.map(id => types[id].label).join(" or ") +
                    ": the types this server signs in by"
            )
        if (authFlowId && !/^[A-Za-z][A-Za-z0-9_]{2,39}$/.test(authFlowId))
            throw new Error("A provider id is 3 to 40 letters, digits or underscores, starting with a letter")
        if (!plainText(p.description, "The name")) throw new Error("Give the provider a name")
        // a provider started here reaches no internal address on a production instance (the server would fetch a URL
        // its integration administrator chose from inside the network); an instance administrator allows one (Tools)
        flow = {authFlowId, authFlowTypeEnumId: typeEnumId, disabled: "Y"}
        if (text(System.getProperty("instance_purpose")) === "production") flow.allowPrivateNetwork = "N"
    } else {
        flow = map(stored)
        if (flow.inbound === "Y")
            return refused("UNSUPPORTED_PROVIDER", "An inbound flow is a remote system's, not a sign-in: it is changed in Tools")
        if (!types[flow.authFlowTypeEnumId])
            return refused("UNSUPPORTED_TYPE", "The provider has no supported type; it is changed in Tools")
    }
    const type = types[flow.authFlowTypeEnumId]
    const sub = creating ? null : map(subRow(flow))
    const subExists = !creating && !!subRow(flow)
    const storedFields = creating ? [] : fieldMaps(authFlowId)
    const storedRoles = creating ? [] : roleMaps(authFlowId)
    const revision = creating ? "" : revisionOf(flow, subExists ? sub : null, storedFields, storedRoles)
    if (!creating && text(p.expectedRevision) !== revision)
        return refused("STALE_REVISION", "The provider was changed since it was read; read it again and repeat the change", {
            authFlowId,
            revision
        })

    // ----- what the save changes, part by part -----
    const flowChanges = {}
    if (given(p.description)) {
        const description = plainText(p.description, "The name")
        if (!description) throw new Error("Give the provider a name")
        if (description !== text(flow.description)) flowChanges.description = description
    }
    if (given(p.iconName)) {
        const iconName = plainText(p.iconName, "The icon", 63)
        if (!/^[\w:-]*$/.test(iconName)) throw new Error("An icon is a name of letters, digits, _ : and -")
        if (iconName !== text(flow.iconName)) flowChanges.iconName = iconName || null
    }
    if (given(p.sequenceNum) && text(p.sequenceNum) !== "") {
        if (!/^\d{1,6}$/.test(text(p.sequenceNum))) throw new Error("The order is a whole number")
        if (Number(p.sequenceNum) !== Number(flow.sequenceNum ?? NaN)) flowChanges.sequenceNum = Number(p.sequenceNum)
    }
    const subChanges = configurationChanges(p, type, sub)
    const newSecrets = {}
    for (const field of type.secrets) {
        if (!given(p[field]) || String(p[field]) === "") continue
        const secret = String(p[field])
        if (secret.length > 4000 || /[\u0000-\u0008\u000b\u000c\u000e-\u001f]/.test(secret))
            throw new Error("The secret is too long or holds control characters")
        newSecrets[field] = secret
    }
    let fieldRows = null
    if (given(p.fieldMapList)) {
        const wanted = fieldMapRows(p.fieldMapList)
        const simple = storedFields.filter(row => !advancedMap(row))
        if (!sameRows(wanted, simple, ["srcFieldName", "dstFieldName"])) {
            if (storedFields.some(advancedMap))
                return refused(
                    "ADVANCED_MAPPING",
                    "This provider's identity mappings carry an expression or a mapping service: an instance administrator changes them in Tools",
                    {authFlowId, revision}
                )
            fieldRows = wanted
        }
    }
    if (Object.keys(flowChanges).length || Object.keys(subChanges).length || Object.keys(newSecrets).length || fieldRows)
        needConfigure()

    // the access parts: the link rule, the default group and the role mappings
    if (given(p.accountLinkEnumId)) {
        const link = enumValue(p.accountLinkEnumId, "AuthFlowAccountLink", "The link rule") || "AflNever"
        if (link !== (flow.accountLinkEnumId || "AflNever")) {
            needMap("change how existing accounts are linked")
            flowChanges.accountLinkEnumId = link
        }
    }
    let roleRows = null
    if (given(p.roleMapList)) {
        const wanted = roleMapRows(p.roleMapList)
        if (!sameRows(wanted, storedRoles, ["roleName", "userGroupId"])) {
            needMap("change role mappings")
            roleRows = wanted
        }
    }
    const defaultChanged = given(p.defaultUserGroupId) && text(p.defaultUserGroupId) !== text(flow.defaultUserGroupId)
    if (defaultChanged) {
        needMap("change the default group")
        flowChanges.defaultUserGroupId = text(p.defaultUserGroupId) || null
    }
    const rolesAfter = roleRows || storedRoles.map(row => ({roleName: row.roleName, userGroupId: row.userGroupId}))
    const defaultAfter = "defaultUserGroupId" in flowChanges ? flowChanges.defaultUserGroupId : flow.defaultUserGroupId
    const checkGroups = () => {
        const checks = groupChecks(rolesAfter.map(row => row.userGroupId).concat([defaultAfter]))
        const roleMapList = rolesAfter.map(row => Object.assign({}, row, checks.get(row.userGroupId) || {allowed: false}))
        const first =
            roleMapList.find(row => !row.allowed) ||
            (defaultAfter && !checks.get(defaultAfter)?.allowed ? checks.get(defaultAfter) : null)
        return first
            ? refused(first.reasonCode || "UNREGISTERED_BUNDLE", first.reason || "A mapped group is not available", {
                  authFlowId,
                  revision,
                  roleMapList
              })
            : null
    }
    if (roleRows || defaultChanged) {
        const refusal = checkGroups()
        if (refusal) return refusal
    }

    // ----- the endpoint-change rule -----
    const endpointChanged = !creating && type.endpoint.some(field => field in subChanges)
    let secretCleared = false
    const secretWrites = Object.assign({}, newSecrets)
    if (endpointChanged)
        for (const field of type.secrets)
            if (!(field in newSecrets) && text(sub?.[field])) {
                secretWrites[field] = null
                secretCleared = true
            }
    const subAfter = Object.assign({}, sub || {}, subChanges, secretWrites)
    const wasEnabled = !creating && flow.disabled !== "Y"
    let enabledAfter = wasEnabled
    // what a sign-in is verified by changed too (the token signature, the nonce): a new test, the secret kept
    const retest = endpointChanged || (!creating && type.retest.some(field => field in subChanges))
    if (retest) {
        if (yes(p.enabled))
            return refused("TEST_REQUIRED", "The configuration changed: test the provider as it is now, then enable it", {
                authFlowId,
                revision
            })
        enabledAfter = false
        Object.assign(flowChanges, {testedConfigHash: null, testedDate: null, testedByUserId: null})
    } else if (yes(p.enabled) && !wasEnabled) {
        needMap("enable a sign-in provider")
        if (flow.authFlowTypeEnumId === "AftSaml")
            return refused("UNSUPPORTED_TYPE", "SAML sign-in is not available in this build", {authFlowId, revision})
        if (creating || !flow.testedConfigHash || flow.testedConfigHash !== fingerprint(flow, subAfter))
            return refused("TEST_REQUIRED", "Test the provider as it is configured now, then enable it", {authFlowId, revision})
        const refusal = checkGroups()
        if (refusal) return refusal
        const beyond = linkedBeyond(authFlowId)
        if (beyond)
            return refused(
                "HELD_BEYOND",
                beyond +
                    (beyond === 1 ? " account that signs" : " accounts that sign") +
                    " in through this provider hold access you do not (a privileged group you are not in, or a sealed permission): an instance administrator enables this provider",
                {authFlowId, revision}
            )
        enabledAfter = true
    } else if (no(p.enabled) && wasEnabled) enabledAfter = false
    if (wasEnabled && !enabledAfter) {
        // an administrator with no password signs in only through an enabled provider
        const path = identity.administrationPath({disabledFlowIds: [authFlowId]})
        if (!path.allowed) return refused(path.reasonCode, path.reason, {authFlowId, revision})
    }
    if (enabledAfter !== wasEnabled || creating) flowChanges.disabled = enabledAfter ? "N" : "Y"

    // ----- write -----
    const nothing =
        !creating &&
        !Object.keys(flowChanges).length &&
        !Object.keys(subChanges).length &&
        !Object.keys(secretWrites).length &&
        !fieldRows &&
        !roleRows
    if (nothing) return {applied: true, authFlowId, revision, enabled: wasEnabled, secretCleared: false, testRequired: false}
    lib.auditReason(reason, auditedFields)
    if (creating) {
        const value = ec.entity.makeValue(FLOW)
        value.setFields(Object.assign({}, flow, flowChanges), true, null, null)
        if (!authFlowId) value.setSequencedIdPrimary()
        else if (one(FLOW, {authFlowId})) throw new Error("A provider with the id " + authFlowId + " exists")
        value.create()
        authFlowId = String(value.authFlowId)
    } else if (Object.keys(flowChanges).length) write(FLOW, Object.assign({authFlowId}, flowChanges), false)
    const subFields = Object.assign({}, subChanges, secretWrites)
    if (creating || !subExists) write(type.entity, Object.assign({authFlowId}, subFields), true)
    else if (Object.keys(subFields).length) write(type.entity, Object.assign({authFlowId}, subFields), false)
    // a secret's replacement or clearing is recorded with no value
    for (const field of Object.keys(secretWrites))
        lib.auditRow(
            type.entity,
            field,
            authFlowId,
            null,
            text(sub?.[field]) ? "(set)" : null,
            secretWrites[field] == null ? "(cleared: the endpoint changed)" : "(replaced)",
            reason
        )
    if (fieldRows) {
        const before = storedFields.map(row => row.srcFieldName + " > " + row.dstFieldName).join(", ")
        find(FIELD_MAP, {authFlowId}).deleteAll()
        fieldRows.forEach((row, index) =>
            write(FIELD_MAP, Object.assign({authFlowId, ruleSeqId: String(index + 1).padStart(2, "0")}, row), true)
        )
        lib.auditRow(
            FIELD_MAP,
            "dstFieldName",
            authFlowId,
            null,
            before || null,
            fieldRows.map(row => row.srcFieldName + " > " + row.dstFieldName).join(", ") || null,
            reason
        )
    }
    if (roleRows) {
        const wanted = new Map(roleRows.map(row => [row.roleName, row.userGroupId]))
        for (const row of storedRoles) {
            if (wanted.has(row.roleName)) continue
            find(ROLE_MAP, {authFlowId, roleName: row.roleName}).deleteAll()
            // the audit log keeps no delete: the group before, none after
            lib.auditRow(ROLE_MAP, "userGroupId", authFlowId, row.roleName, row.userGroupId, null, reason)
        }
        const before = new Map(storedRoles.map(row => [row.roleName, row.userGroupId]))
        for (const row of roleRows) {
            if (!before.has(row.roleName)) write(ROLE_MAP, Object.assign({authFlowId}, row), true)
            else if (before.get(row.roleName) !== row.userGroupId) write(ROLE_MAP, Object.assign({authFlowId}, row), false)
        }
    }
    const after = map(one(FLOW, {authFlowId}))
    const subNow = subRow(after)
    return {
        applied: true,
        reasonCode: null,
        reason: retest
            ? (endpointChanged ? "The endpoint changed: " : "What a sign-in is verified by changed: ") +
              (secretCleared ? "the stored secret was cleared, and " : "") +
              "the provider is disabled until a test of it passes and it is enabled again"
            : null,
        authFlowId,
        revision: revisionOf(after, subNow, fieldMaps(authFlowId), roleMaps(authFlowId)),
        enabled: after.disabled !== "Y",
        secretCleared,
        testRequired: after.disabled === "Y" && after.testedConfigHash !== fingerprint(after, subNow)
    }
}

// ===== test#IdentityProvider =====
// The roles of a test identity: a list, or text separated by commas or spaces.
const roleNames = value =>
    (Array.isArray(value) ? value : String(value ?? "").split(/[,\s]+/)).map(text).filter(Boolean).slice(0, 50)
// A message the server made, safe to show: never a stored secret, and bounded.
function safeMessage(message, sub, type) {
    let found = String(message || "")
    for (const field of type?.secrets || []) {
        const secret = String(sub?.[field] || "")
        if (secret.length >= 4) found = found.split(secret).join("(secret)")
    }
    return found.replace(/[\u0000-\u001f]+/g, " ").slice(0, 500)
}
// test#IdentityProvider: the explicit check of a provider as it is configured now, enabled or not: its row is complete, its
// role mappings and default group still hold, and a sign-in could start (the server reads an OpenID provider's discovery
// document and builds its client, exactly as a sign-in does). No account is made, nobody is signed in and no token is
// asked for. A pass is marked on the provider for the configuration it ran against; a failure clears the mark. With a test
// identity's roles, the groups its sign-in would give.
function testIdentityProvider(p) {
    lib.signedIn()
    if (!has(CONNECTION_MANAGE) && !has(ACCESS_MANAGE))
        throw new Error(CONNECTION_MANAGE + " or " + ACCESS_MANAGE + " is required")
    const authFlowId = text(p.authFlowId)
    const stored = authFlowId ? find(FLOW, {authFlowId}).forUpdate(true).one() : null
    if (!stored) return refused("PROVIDER_UNAVAILABLE", "The provider is not available", {passed: false})
    const flow = map(stored)
    if (flow.inbound === "Y")
        return refused("UNSUPPORTED_PROVIDER", "An inbound flow is a remote system's, not a sign-in", {passed: false})
    const type = types[flow.authFlowTypeEnumId]
    const sub = map(subRow(flow))
    const roles = roleMaps(authFlowId)
    const checkList = []
    const check = (name, ok, textOk, textFailed) => {
        checkList.push({check: name, state: ok ? "ok" : "failed", text: ok ? textOk : textFailed})
        return ok
    }
    const problem = incomplete(flow, subRow(flow) ? sub : null)
    let passed = check("Configuration", !problem, "The configuration is complete", problem)
    const groups = groupChecks(roles.map(row => row.userGroupId).concat([flow.defaultUserGroupId]))
    const refusedGroups = Array.from(groups.values()).filter(row => !row.allowed)
    passed =
        check(
            "Role mappings",
            !refusedGroups.length,
            roles.length ? "Every mapped group may be given by a sign-in" : "No role is mapped",
            refusedGroups.map(row => row.reason).join("; ")
        ) && passed
    let callbackUrl = ""
    let authorizeHost = ""
    if (!problem) {
        let result = {}
        let failure = ""
        ec.message.pushErrors()
        try {
            result = ec.service.sync().name("org.moqui.sso.AuthServices.check#Flow").parameters({authFlowId}).call() || {}
            if (ec.message.hasError()) failure = ec.message.getErrorsString()
        } catch (error) {
            failure = String(error.message || error)
        } finally {
            ec.message.clearErrors()
            ec.message.popErrors()
        }
        callbackUrl = text(result.callbackUrl)
        authorizeHost = text(result.authorizeHost)
        const started = !failure && result.passed === true
        passed =
            check(
                "Provider",
                started,
                "A sign-in can start" + (authorizeHost ? " at " + authorizeHost : ""),
                safeMessage(failure || result.message || "The provider could not be reached", sub, type) +
                    (/internal address/.test(failure || result.message || "")
                        ? ": an instance administrator allows a provider on the instance's own network"
                        : "")
            ) && passed
    }
    const tested = fingerprint(flow, sub)
    write(
        FLOW,
        passed
            ? {authFlowId, testedConfigHash: tested, testedDate: ec.user.nowTimestamp, testedByUserId: ec.user.userId}
            : {authFlowId, testedConfigHash: null, testedDate: null, testedByUserId: null},
        false
    )
    // the groups a test identity's sign-in would give: each of its roles through the mappings, else the default group
    const testRoles = roleNames(p.testRoles)
    const mapped = new Map(roles.map(row => [row.roleName, row.userGroupId]))
    const grantPreview = testRoles.map(roleName => {
        const group = groups.get(mapped.get(roleName))
        return {
            roleName,
            userGroupId: mapped.get(roleName) || "",
            groupName: group?.description || "",
            given: !!group?.allowed,
            text: !mapped.has(roleName)
                ? "Not mapped: gives nothing"
                : group?.allowed
                  ? "Gives " + group.description
                  : group?.reason || ""
        }
    })
    const defaultGroup = flow.defaultUserGroupId ? groups.get(flow.defaultUserGroupId) : null
    if (defaultGroup && !grantPreview.some(row => row.given))
        grantPreview.push({
            roleName: "(no mapped role)",
            userGroupId: flow.defaultUserGroupId,
            groupName: defaultGroup.description,
            given: defaultGroup.allowed,
            text: defaultGroup.allowed ? "Gives the default group " + defaultGroup.description : defaultGroup.reason
        })
    const after = map(one(FLOW, {authFlowId}))
    return {
        applied: true,
        passed,
        checkList,
        callbackUrl,
        authorizeHost,
        grantPreview,
        enabled: after.disabled !== "Y",
        testedDate: iso(after.testedDate),
        revision: revisionOf(after, subRow(after), fieldMaps(authFlowId), roles)
    }
}

const operations = {findIdentityProviders, storeIdentityProvider, testIdentityProvider}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown identity provider operation " + op)
        return operations[op](parameters)
    }
})
