// Native group, membership and grant governance, org.moqui.admin.AccessServices (mr-boss plan/design-admin.md §5 Groups and
// GroupDetail, §17.2; impl-admin P3). The framework evaluator stays authoritative: these services only write the native rows
// (UserGroup, UserGroupMember, UserGroupPermission, ArtifactAuthz with its filters, UserGroupPreference), each through the entity
// facade with authorization on, so the sealed-permission and privileged-group EECAs (Security.eecas.xml) decide every row as they
// do for Tools, imports and direct entity calls, and each write moves the authorization caches here and on every live cluster
// member (the entity-changed notice and the cluster invalidation).
//
// Bounded delegation (design §1.1): reading needs ADMIN_ACCESS_VIEW and every change ADMIN_ACCESS_MANAGE (sealed, so only a
// holder grants it), checked with ec.user.hasPermission, never a group name. A membership is given only in a registered bundle
// (moqui.security.CapabilityBundle) or a custom group an access administrator created (a registered purpose); a grant only from
// a registered bundle's permissions and artifact bindings; a bundle naming a delegable permission needs it too, and a privileged
// group's bundle membership of that group (so its powers are not copied to another group). A sealed permission is granted or
// ended only by a holder, a membership in a group that grants one given only by a holder (so no sealed grant is smuggled
// through a group; the framework's UserGroupMemberSealed EECA refuses it too), and a privileged group changed only by a member,
// refused here with the reason before the EECA would refuse the write. Every change names the revision it read (a row's or group's lastUpdatedStamp; for
// grants the group's grant revision from preview#Grants) and is refused naming the current one when stale; every membership,
// group-flag and grant change keeps the administration path (IdentityServices.check#AdministrationPath). A refusal is answered
// as a reason (applied false, reasonCode, reason), never written; a malformed request is an error. The audit log keeps before
// and after: the entities' audit-logged fields (UserGroupMember's and UserGroupPermission's dates, ArtifactAuthz's type,
// UserGroup's factor and IP flags, UserGroupPreference's value) with the reason given, and an explicit row for a removed
// artifact binding.
const identity = ec.resource.script("component://framework/service/org/moqui/admin/Identity.js")
const GROUP = "moqui.security.UserGroup"
const MEMBER = "moqui.security.UserGroupMember"
const GRANT = "moqui.security.UserGroupPermission"
const AUTHZ = "moqui.security.ArtifactAuthz"
const AUTHZ_FILTER = "moqui.security.ArtifactAuthzFilter"
const PREFERENCE = "moqui.security.UserGroupPreference"
const BUNDLE = "moqui.security.CapabilityBundle"
const BUNDLE_PERMISSION = "moqui.security.CapabilityBundlePermission"
const BUNDLE_ARTIFACT = "moqui.security.CapabilityBundleArtifact"
const VIEW = "ADMIN_ACCESS_VIEW"
const MANAGE = "ADMIN_ACCESS_MANAGE"
// a group a custom group may not be named: the framework's own and the automatic group
const reservedGroups = ["ALL_USERS", "ADMIN", "ADMIN_ADV"]

const find = (entity, conditions = {}) => ec.entity.find(entity).condition(conditions).disableAuthz()
const one = (entity, conditions) => find(entity, conditions).one()
const rows = (entity, conditions) => Array.from(find(entity, conditions).list())
const map = row => (row?.getMap ? row.getMap() : Object.assign({}, row || {}))
const ms = value =>
    value == null || value === ""
        ? null
        : value instanceof Date
          ? value.getTime()
          : /^-?\d+$/.test(String(value))
            ? Number(value)
            : Date.parse(String(value))
const iso = value => (value == null ? null : new Date(value).toISOString())
const nowMs = () => ms(ec.user.nowTimestamp)
const open = (row, at = nowMs()) => (ms(row.fromDate) ?? -Infinity) <= at && (ms(row.thruDate) ?? Infinity) > at
const current = (row, at = nowMs()) => (ms(row.thruDate) ?? Infinity) > at
const stamp = row => String(ms(row?.lastUpdatedStamp) ?? "")
const text = value => String(value ?? "").trim()

// ===== permission, refusal and reason =====
function signedIn() {
    if (!ec.user.userId) throw new Error("Sign in to administer access")
}
function requireView() {
    signedIn()
    if (!ec.user.hasPermission(VIEW) && !ec.user.hasPermission(MANAGE)) throw new Error(VIEW + " is required")
}
function requireManage() {
    signedIn()
    if (!ec.user.hasPermission(MANAGE)) throw new Error(MANAGE + " is required")
}
const refused = (reasonCode, reason, extra = {}) => Object.assign({applied: false, reasonCode, reason}, extra)
function reasonText(p) {
    const reason = text(p.reason)
    if (reason.length > 255) throw new Error("Give a reason of at most 255 characters")
    return reason || null
}
// The reason the audit log records beside each audit-logged field the change writes ({field}_changeReason, as Java).
function auditReason(reason, fields) {
    if (!reason) return
    ec.context.putAll(Object.fromEntries(fields.map(field => [field + "_changeReason", reason])))
}

// ===== the effective seals and the registry =====
const privilegedIds = () => new Set(Array.from(UserSecurityChecks.privilegedGroups()))
const sealedIds = () => new Set(Array.from(UserSecurityChecks.sealedPermissions()))
// The registered bundles in use: enabled rows whose group exists; a row naming a missing group is logged by name and left out.
function bundles() {
    const groupIds = new Set(rows(GROUP).map(row => row.userGroupId))
    const found = new Map()
    for (const row of rows(BUNDLE)) {
        if (row.disabled === "Y") continue
        if (!groupIds.has(row.userGroupId)) {
            ec.logger.error("Capability bundle " + row.userGroupId + " names a group that does not exist; left out")
            continue
        }
        found.set(row.userGroupId, map(row))
    }
    return found
}
// A bundle the actor may assign or grant from: one naming a delegable permission only by its holders, and a privileged
// group's bundle only by a member of it, so a privileged group's permissions and bindings are not granted to another group by
// a non-member (a custom group given BOSS_ADMIN_BACKUP's permissions would be BOSS_ADMIN_BACKUP without its membership rule).
const delegable = (bundle, privileged = privilegedIds()) =>
    (!bundle.delegablePermissionId || ec.user.hasPermission(bundle.delegablePermissionId)) &&
    (!privileged.has(bundle.userGroupId) || ec.user.isInGroup(bundle.userGroupId))
// A custom group: one an access administrator created, of a registered purpose (a UserGroupType with optionIndicator Y).
function purposes() {
    return rows("moqui.basic.Enumeration", {enumTypeId: "UserGroupType", optionIndicator: "Y"}).sort(
        (a, b) => (a.sequenceNum ?? 0) - (b.sequenceNum ?? 0)
    )
}
function isCustom(group, purposeIds = new Set(purposes().map(row => row.enumId))) {
    return !!group?.groupTypeEnumId && purposeIds.has(group.groupTypeEnumId)
}
// What the actor may change of a group: a registered bundle or a custom group, and for a privileged group only a member. The
// reason code when not.
function groupRefusal(group, registry = bundles(), privileged = privilegedIds()) {
    if (!group) return {reasonCode: "GROUP_UNAVAILABLE", reason: "The group is not available"}
    const groupId = group.userGroupId
    if (privileged.has(groupId) && !ec.user.isInGroup(groupId))
        return {
            reasonCode: "PRIVILEGED_GROUP",
            reason: groupId + " is a privileged group: only a member of it may change it or its members"
        }
    const bundle = registry.get(groupId)
    if (bundle && !delegable(bundle, privileged))
        return {reasonCode: "NOT_DELEGABLE", reason: groupId + " is assigned only by holders of " + bundle.delegablePermissionId}
    if (!bundle && !isCustom(group) && !(privileged.has(groupId) && ec.user.isInGroup(groupId)))
        return {
            reasonCode: "UNREGISTERED_BUNDLE",
            reason: groupId + " is not a registered bundle or a custom group; an instance administrator changes it in Tools"
        }
    return null
}

// ===== groups =====
function memberCounts() {
    const counts = new Map()
    const at = nowMs()
    for (const row of rows(MEMBER)) if (open(row, at)) counts.set(row.userGroupId, (counts.get(row.userGroupId) || 0) + 1)
    return counts
}
function groupSummary(group, context) {
    const groupId = group.userGroupId
    const bundle = context.registry.get(groupId) || null
    const grants = (context.grants.get(groupId) || []).filter(row => current(row, context.at))
    const sealed = Array.from(new Set(grants.map(row => row.userPermissionId).filter(id => context.sealed.has(id)))).sort()
    const custom = isCustom(group, context.purposeIds)
    return {
        userGroupId: groupId,
        description: group.description || "",
        groupTypeEnumId: group.groupTypeEnumId || null,
        purposeName: context.purposeNames.get(group.groupTypeEnumId) || "",
        appName: bundle?.appName || (custom ? "Custom" : ""),
        registered: !!bundle,
        custom,
        kind: bundle ? "registered" : custom ? "custom" : "other",
        privileged: context.privileged.has(groupId),
        sealedPermissionIds: sealed,
        hasSealed: sealed.length > 0,
        requireAuthcFactor: group.requireAuthcFactor === "Y",
        ipLimited: !!text(group.ipAllowed),
        memberCount: context.counts.get(groupId) || 0,
        permissionCount: grants.length,
        appGates: (context.authz.get(groupId) || []).map(row => row.artifactGroupId),
        revision: stamp(group)
    }
}
function listContext() {
    const grouped = (entity, key = "userGroupId") => {
        const byGroup = new Map()
        for (const row of rows(entity).map(map)) {
            if (!byGroup.has(row[key])) byGroup.set(row[key], [])
            byGroup.get(row[key]).push(row)
        }
        return byGroup
    }
    const purposeRows = rows("moqui.basic.Enumeration", {enumTypeId: "UserGroupType"})
    return {
        at: nowMs(),
        registry: bundles(),
        privileged: privilegedIds(),
        sealed: sealedIds(),
        counts: memberCounts(),
        grants: grouped(GRANT),
        authz: grouped(AUTHZ),
        purposeIds: new Set(purposes().map(row => row.enumId)),
        purposeNames: new Map(purposeRows.map(row => [row.enumId, row.description || row.enumId]))
    }
}

// find#Groups: the groups the access administrator sees, with the markers that explain a refusal before it happens.
function findGroups(p) {
    requireView()
    const context = listContext()
    const term = text(p.text).toLowerCase()
    const groups = rows(GROUP)
        .map(group => groupSummary(group, context))
        .filter(group => !term || [group.userGroupId, group.description].some(value => value.toLowerCase().includes(term)))
        .filter(group => !p.appName || group.appName === p.appName)
        .filter(group => !p.kind || group.kind === p.kind || (p.kind === "supported" && group.kind !== "other"))
        .filter(group => !p.privilegedOnly || group.privileged)
        .filter(
            group =>
                !p.userPermissionId ||
                (context.grants.get(group.userGroupId) || []).some(
                    row => row.userPermissionId === p.userPermissionId && open(row, context.at)
                )
        )
        .sort(
            (a, b) =>
                ["registered", "custom", "other"].indexOf(a.kind) - ["registered", "custom", "other"].indexOf(b.kind) ||
                a.appName.localeCompare(b.appName) ||
                a.userGroupId.localeCompare(b.userGroupId)
        )
    const appNames = Array.from(
        new Set(
            Array.from(context.registry.values())
                .map(row => row.appName)
                .filter(Boolean)
        )
    ).sort()
    return {
        groupList: groups,
        appNames,
        purposeOptions: purposes().map(row => ({value: row.enumId, label: row.description || row.enumId})),
        canManage: ec.user.hasPermission(MANAGE)
    }
}

// The revision of a group's grants: its permission and artifact rows as they are, so a preview and its apply see the same set.
function grantRevision(groupId) {
    const grants = rows(GRANT, {userGroupId: groupId})
        .map(row => [row.userPermissionId, ms(row.fromDate), ms(row.thruDate), stamp(row)].join("|"))
        .sort()
    const authz = rows(AUTHZ, {userGroupId: groupId})
        .map(row => [row.artifactAuthzId, row.artifactGroupId, row.authzTypeEnumId, stamp(row)].join("|"))
        .sort()
    return ec.user.singleUseCodeHash(JSON.stringify([grants, authz]))
}
function userLabel(userId) {
    const account = one("moqui.security.UserAccount", {userId})
    return {username: account?.username || "", userFullName: account?.userFullName || account?.username || userId}
}

// get#Group: one group's profile, members, grants, artifact bindings and preferences, with what the actor may change; with
// includeOptions the registered bundles a grant may come from.
function getGroup(p) {
    requireView()
    const group = one(GROUP, {userGroupId: p.userGroupId})
    if (!group) throw new Error("Group " + p.userGroupId + " is not available")
    const context = listContext()
    const summary = groupSummary(group, context)
    const at = context.at
    const permissionDescriptions = new Map(
        rows("moqui.security.UserPermission").map(row => [row.userPermissionId, row.description])
    )
    const registeredPermissions = new Map()
    for (const row of rows(BUNDLE_PERMISSION))
        if (context.registry.has(row.userGroupId) && !registeredPermissions.has(row.userPermissionId))
            registeredPermissions.set(row.userPermissionId, row.capabilityKindEnumId)
    const registeredArtifacts = new Map()
    for (const row of rows(BUNDLE_ARTIFACT))
        if (context.registry.has(row.userGroupId) && !registeredArtifacts.has(row.artifactGroupId))
            registeredArtifacts.set(row.artifactGroupId, map(row))
    const members = rows(MEMBER, {userGroupId: group.userGroupId})
        .map(row =>
            Object.assign(
                {
                    userId: row.userId,
                    fromDate: ms(row.fromDate),
                    thruDate: ms(row.thruDate),
                    active: open(row, at),
                    ended: !current(row, at),
                    revision: stamp(row)
                },
                userLabel(row.userId)
            )
        )
        .sort((a, b) => Number(b.active) - Number(a.active) || (b.fromDate ?? 0) - (a.fromDate ?? 0))
    const grants = rows(GRANT, {userGroupId: group.userGroupId})
        .map(row => ({
            userPermissionId: row.userPermissionId,
            description: permissionDescriptions.get(row.userPermissionId) || "",
            fromDate: ms(row.fromDate),
            thruDate: ms(row.thruDate),
            active: open(row, at),
            ended: !current(row, at),
            sealed: context.sealed.has(row.userPermissionId),
            registered: registeredPermissions.has(row.userPermissionId),
            capabilityKindEnumId: registeredPermissions.get(row.userPermissionId) || null
        }))
        .sort((a, b) => Number(b.active) - Number(a.active) || a.userPermissionId.localeCompare(b.userPermissionId))
    const artifacts = rows(AUTHZ, {userGroupId: group.userGroupId}).map(row => ({
        artifactAuthzId: row.artifactAuthzId,
        artifactGroupId: row.artifactGroupId,
        description: one("moqui.security.ArtifactGroup", {artifactGroupId: row.artifactGroupId})?.description || "",
        authzTypeEnumId: row.authzTypeEnumId,
        authzActionEnumId: row.authzActionEnumId,
        filterSetIds: rows(AUTHZ_FILTER, {artifactAuthzId: row.artifactAuthzId}).map(filter => filter.entityFilterSetId),
        registered: registeredArtifacts.get(row.artifactGroupId)?.authzTypeEnumId === row.authzTypeEnumId
    }))
    const keys = new Map(rows("moqui.basic.Enumeration", {enumTypeId: "UserPreferenceKey"}).map(row => [row.enumId, row]))
    const preferences = rows(PREFERENCE, {userGroupId: group.userGroupId}).map(row => ({
        preferenceKey: row.preferenceKey,
        description: keys.get(row.preferenceKey)?.description || "",
        valueType: preferenceType(keys.get(row.preferenceKey)),
        preferenceValue: row.preferenceValue,
        groupPriority: row.groupPriority,
        registered: keys.has(row.preferenceKey),
        revision: stamp(row)
    }))
    const refusal = groupRefusal(group, context.registry, context.privileged)
    const result = {
        group: Object.assign(summary, {ipAllowed: group.ipAllowed || ""}),
        bundle: context.registry.get(group.userGroupId) || null,
        members,
        grants,
        artifacts,
        preferences,
        preferenceKeys: Array.from(keys.values()).map(row => ({
            value: row.enumId,
            label: row.description || row.enumId,
            valueType: preferenceType(row)
        })),
        grantRevision: grantRevision(group.userGroupId),
        canManage: ec.user.hasPermission(MANAGE) && !refusal,
        manageReasonCode: refusal?.reasonCode || (ec.user.hasPermission(MANAGE) ? null : "PERMISSION_DENIED"),
        manageReason: refusal?.reason || (ec.user.hasPermission(MANAGE) ? null : MANAGE + " is required to change groups"),
        purposeOptions: purposes().map(row => ({value: row.enumId, label: row.description || row.enumId}))
    }
    if (p.includeOptions) result.bundleOptions = bundleOptions(context)
    return result
}
// The registered bundles a grant may come from, each with its permissions and artifact bindings and whether the actor may grant
// each (sealed ones only by a holder; a bundle naming a delegable permission only by its holders).
function bundleOptions(context = listContext()) {
    const permissionDescriptions = new Map(
        rows("moqui.security.UserPermission").map(row => [row.userPermissionId, row.description])
    )
    const byGroup = (entity, conditions = {}) => {
        const found = new Map()
        for (const row of rows(entity, conditions).map(map)) {
            if (!found.has(row.userGroupId)) found.set(row.userGroupId, [])
            found.get(row.userGroupId).push(row)
        }
        return found
    }
    const permissions = byGroup(BUNDLE_PERMISSION)
    const artifacts = byGroup(BUNDLE_ARTIFACT)
    return Array.from(context.registry.values())
        .sort((a, b) => (a.appName || "").localeCompare(b.appName || "") || (a.sequenceNum ?? 0) - (b.sequenceNum ?? 0))
        .map(bundle => ({
            bundleGroupId: bundle.userGroupId,
            appName: bundle.appName || "",
            description: bundle.description || one(GROUP, {userGroupId: bundle.userGroupId})?.description || "",
            delegable: delegable(bundle, context.privileged),
            privileged: context.privileged.has(bundle.userGroupId),
            permissions: (permissions.get(bundle.userGroupId) || []).map(row => ({
                userPermissionId: row.userPermissionId,
                description: permissionDescriptions.get(row.userPermissionId) || "",
                capabilityKindEnumId: row.capabilityKindEnumId || null,
                sealed: context.sealed.has(row.userPermissionId),
                grantable:
                    delegable(bundle, context.privileged) &&
                    (!context.sealed.has(row.userPermissionId) || ec.user.hasPermission(row.userPermissionId))
            })),
            artifacts: (artifacts.get(bundle.userGroupId) || []).map(row => ({
                artifactGroupId: row.artifactGroupId,
                description: row.description || "",
                authzTypeEnumId: row.authzTypeEnumId
            }))
        }))
}

const groupIdPattern = /^[A-Z][A-Z0-9_]{2,59}$/
// create#Group: a custom group, never privileged, of a registered purpose, with no members or grants yet.
function createGroup(p) {
    requireManage()
    const groupId = text(p.userGroupId).toUpperCase()
    if (!groupIdPattern.test(groupId))
        throw new Error("Give a group ID of 3 to 60 capital letters, digits or underscores, starting with a letter")
    const description = text(p.description)
    if (!description || description.length > 255) throw new Error("Give the group a description of 1–255 characters")
    if (!purposes().some(row => row.enumId === p.groupTypeEnumId)) throw new Error("Choose the group's purpose")
    if (privilegedIds().has(groupId) || reservedGroups.includes(groupId))
        return refused("PRIVILEGED_GROUP", groupId + " is reserved: a custom group is never privileged")
    if (one(GROUP, {userGroupId: groupId})) return refused("GROUP_EXISTS", "A group " + groupId + " already exists")
    const value = ec.entity.makeValue(GROUP)
    value.setFields({userGroupId: groupId, description, groupTypeEnumId: p.groupTypeEnumId}, true, null, null)
    value.create()
    return {applied: true, userGroupId: groupId, revision: stamp(one(GROUP, {userGroupId: groupId}))}
}

// The allowed IP patterns as login reads them: comma separated, each an IPv4 address whose parts are a number, * or a range, or
// an IPv6 address.
function validIpAllowed(value) {
    if (!value) return true
    return value.split(",").every(part => {
        const pattern = part.trim()
        if (!pattern) return false
        if (pattern.includes(":")) return /^[0-9A-Fa-f:.]+$/.test(pattern)
        const parts = pattern.split(".")
        return parts.length === 4 && parts.every(item => item === "*" || /^\d{1,3}$/.test(item) || /^\d{1,3}-\d{1,3}$/.test(item))
    })
}
// store#Group: the safe profile of a group (description, purpose of a custom group) and its sign-in policy (factor required,
// allowed IPs), which keep the administration path.
function storeGroup(p) {
    requireManage()
    const reason = reasonText(p)
    const group = find(GROUP, {userGroupId: p.userGroupId}).forUpdate(true).one()
    const refusal = groupRefusal(group)
    if (refusal) return refused(refusal.reasonCode, refusal.reason)
    if (!p.expectedRevision || p.expectedRevision !== stamp(group))
        return refused("REVISION_CHANGED", "The group changed since you read it; reload it (revision " + stamp(group) + ")", {
            revision: stamp(group)
        })
    const changes = {}
    if (p.description != null) {
        const description = text(p.description)
        if (!description || description.length > 255) throw new Error("Give the group a description of 1–255 characters")
        changes.description = description
    }
    if (p.groupTypeEnumId != null && p.groupTypeEnumId !== group.groupTypeEnumId) {
        if (!isCustom(group) || !purposes().some(row => row.enumId === p.groupTypeEnumId))
            throw new Error("Only a custom group's purpose changes, to another registered purpose")
        changes.groupTypeEnumId = p.groupTypeEnumId
    }
    if (p.requireAuthcFactor != null) {
        const flag = p.requireAuthcFactor === true || p.requireAuthcFactor === "Y" ? "Y" : "N"
        if (flag !== (group.requireAuthcFactor === "Y" ? "Y" : "N")) changes.requireAuthcFactor = flag
    }
    if (p.ipAllowed != null) {
        const ipAllowed = text(p.ipAllowed)
        if (!validIpAllowed(ipAllowed))
            throw new Error(
                "Allowed IPs are comma separated IPv4 addresses (each part a number, * or a range like 0-31) or IPv6 addresses"
            )
        if (ipAllowed !== text(group.ipAllowed)) changes.ipAllowed = ipAllowed || null
    }
    if (!Object.keys(changes).length) return {applied: true, userGroupId: group.userGroupId, revision: stamp(group)}
    let path = null
    if ("requireAuthcFactor" in changes || "ipAllowed" in changes) {
        path = identity.administrationPath({
            groups: {
                [group.userGroupId]: {
                    requireAuthcFactor: changes.requireAuthcFactor ?? group.requireAuthcFactor,
                    ipAllowed: "ipAllowed" in changes ? changes.ipAllowed : group.ipAllowed
                }
            }
        })
        if (!path.allowed) return refused(path.reasonCode, path.reason)
    }
    auditReason(reason, ["requireAuthcFactor", "ipAllowed"])
    const value = ec.entity.makeValue(GROUP)
    value.setFields(Object.assign({userGroupId: group.userGroupId}, changes), true, null, null)
    value.update()
    return {
        applied: true,
        userGroupId: group.userGroupId,
        revision: stamp(one(GROUP, {userGroupId: group.userGroupId})),
        pathsAfter: path?.pathsAfter ?? null
    }
}

// ===== memberships =====
function memberRow(groupId, userId, fromDate) {
    const at = ms(fromDate)
    return (
        Array.from(find(MEMBER, {userGroupId: groupId, userId}).forUpdate(true).list()).find(row => ms(row.fromDate) === at) ||
        null
    )
}
// store#Membership: a dated membership in a registered bundle or a custom group, from a date (now when none) and until one, or a
// change of an existing row's end date; never a second overlapping membership.
function storeMembership(p) {
    requireManage()
    const reason = reasonText(p)
    const group = one(GROUP, {userGroupId: p.userGroupId})
    const registry = bundles()
    if (group && !registry.has(group.userGroupId) && !isCustom(group))
        return refused(
            "UNREGISTERED_BUNDLE",
            p.userGroupId + " is not a registered bundle or a custom group; only registered bundles are assigned here"
        )
    const refusal = groupRefusal(group, registry)
    if (refusal) return refused(refusal.reasonCode, refusal.reason)
    // a membership must not carry a sealed permission the actor does not hold (no sealed grant smuggled through a group)
    const sealed = sealedIds()
    const carried = Array.from(
        new Set(
            rows(GRANT, {userGroupId: group.userGroupId})
                .filter(row => current(row) && sealed.has(row.userPermissionId) && !ec.user.hasPermission(row.userPermissionId))
                .map(row => row.userPermissionId)
        )
    )
    if (carried.length)
        return refused(
            "SEALED_PERMISSION",
            group.userGroupId + " grants " + carried.join(", ") + ", sealed: only a holder may give its membership"
        )
    const account = p.userId ? one("moqui.security.UserAccount", {userId: p.userId}) : null
    if (!account) return refused("USER_UNAVAILABLE", "User " + (p.userId || "") + " is not available")
    const now = nowMs()
    const existing = p.fromDate != null && p.fromDate !== "" ? memberRow(group.userGroupId, account.userId, p.fromDate) : null
    const fromDate = existing ? ms(existing.fromDate) : p.fromDate != null && p.fromDate !== "" ? ms(p.fromDate) : now
    const thruDate = p.thruDate != null && p.thruDate !== "" ? ms(p.thruDate) : null
    if (!Number.isFinite(fromDate) || (thruDate != null && (!Number.isFinite(thruDate) || thruDate <= fromDate)))
        throw new Error("The membership must end after it starts")
    if (existing) {
        if (!p.expectedRevision || p.expectedRevision !== stamp(existing))
            return refused("REVISION_CHANGED", "The membership changed since you read it; reload it", {revision: stamp(existing)})
        if (existing.thruDate && ms(existing.thruDate) <= now)
            return refused("MEMBERSHIP_ENDED", "An ended membership is kept as it was; add a new one")
    } else {
        if (fromDate < now - 60000) throw new Error("A new membership starts now or later")
        const overlapping = rows(MEMBER, {userGroupId: group.userGroupId, userId: account.userId}).some(
            row => (ms(row.fromDate) ?? -Infinity) < (thruDate ?? Infinity) && (ms(row.thruDate) ?? Infinity) > fromDate
        )
        if (overlapping)
            return refused(
                "MEMBERSHIP_EXISTS",
                account.username + " is already a member of " + group.userGroupId + " for those dates"
            )
    }
    const row = {userGroupId: group.userGroupId, userId: account.userId, fromDate, thruDate}
    const path = identity.administrationPath({memberships: [row]})
    if (!path.allowed) return refused(path.reasonCode, path.reason)
    auditReason(reason, ["fromDate", "thruDate"])
    const value = ec.entity.makeValue(MEMBER)
    value.setFields(
        {userGroupId: row.userGroupId, userId: row.userId, fromDate: iso(fromDate), thruDate: iso(thruDate)},
        true,
        null,
        null
    )
    if (existing) value.update()
    else value.create()
    const stored = memberRow(row.userGroupId, row.userId, fromDate)
    return {applied: true, userGroupId: row.userGroupId, userId: row.userId, fromDate, thruDate, revision: stamp(stored)}
}

// end#Membership: ends a membership at a date (now when none), keeping the row; any group, so a member of a privileged group or
// ADMIN can end another's, as long as the administration path stays.
function endMembership(p) {
    requireManage()
    const reason = reasonText(p)
    const group = one(GROUP, {userGroupId: p.userGroupId})
    if (!group) return refused("GROUP_UNAVAILABLE", "The group is not available")
    if (privilegedIds().has(group.userGroupId) && !ec.user.isInGroup(group.userGroupId))
        return refused(
            "PRIVILEGED_GROUP",
            group.userGroupId + " is a privileged group: only a member of it may change its members"
        )
    const existing = memberRow(group.userGroupId, p.userId, p.fromDate)
    if (!existing) return refused("MEMBERSHIP_UNAVAILABLE", "The membership is not available")
    if (!p.expectedRevision || p.expectedRevision !== stamp(existing))
        return refused("REVISION_CHANGED", "The membership changed since you read it; reload it", {revision: stamp(existing)})
    const now = nowMs()
    const thruDate = p.thruDate != null && p.thruDate !== "" ? ms(p.thruDate) : now
    if (!Number.isFinite(thruDate) || thruDate < now - 60000) throw new Error("A membership ends now or later")
    if (existing.thruDate && ms(existing.thruDate) <= now) return refused("MEMBERSHIP_ENDED", "The membership has already ended")
    const fromDate = ms(existing.fromDate)
    const end = Math.max(thruDate, fromDate ?? thruDate)
    const row = {userGroupId: group.userGroupId, userId: existing.userId, fromDate, thruDate: end}
    const path = identity.administrationPath({memberships: [row]})
    if (!path.allowed) return refused(path.reasonCode, path.reason)
    auditReason(reason, ["thruDate"])
    const value = ec.entity.makeValue(MEMBER)
    value.setFields(
        {userGroupId: row.userGroupId, userId: row.userId, fromDate: iso(fromDate), thruDate: iso(end)},
        true,
        null,
        null
    )
    value.update()
    return {
        applied: true,
        userGroupId: row.userGroupId,
        userId: row.userId,
        fromDate,
        thruDate: end,
        revision: stamp(memberRow(row.userGroupId, row.userId, fromDate))
    }
}

// ===== grants =====
// The changes of a grant request, each a permission or an artifact binding of the group, added or ended; a bundle named alone
// stands for each of its permissions and bindings.
function requested(p) {
    const list = Array.isArray(p.grantList) ? p.grantList : Array.from(p.grantList || [])
    const changes = []
    const registry = bundles()
    for (const item of list.map(map)) {
        const action = item.action === "end" ? "end" : "add"
        const base = {action, bundleGroupId: item.bundleGroupId || null, fromDate: item.fromDate, thruDate: item.thruDate}
        if (item.userPermissionId) changes.push(Object.assign({kind: "permission", id: item.userPermissionId}, base))
        else if (item.artifactGroupId) changes.push(Object.assign({kind: "artifact", id: item.artifactGroupId}, base))
        else if (item.bundleGroupId && registry.has(item.bundleGroupId)) {
            for (const row of rows(BUNDLE_PERMISSION, {userGroupId: item.bundleGroupId}))
                changes.push(Object.assign({kind: "permission", id: row.userPermissionId}, base))
            for (const row of rows(BUNDLE_ARTIFACT, {userGroupId: item.bundleGroupId}))
                changes.push(Object.assign({kind: "artifact", id: row.artifactGroupId}, base))
        } else if (item.bundleGroupId) changes.push(Object.assign({kind: "bundle", id: item.bundleGroupId}, base))
        else throw new Error("Each grant names a permission, an artifact group or a bundle")
    }
    const seen = new Set()
    return changes.filter(change => {
        const key = change.kind + "|" + change.id + "|" + change.action
        if (seen.has(key)) return false
        seen.add(key)
        return true
    })
}
// Each change's effect and whether the actor may make it, against the group's rows as they are.
function evaluate(group, changes) {
    const registry = bundles()
    const sealed = sealedIds()
    const at = nowMs()
    const privileged = privilegedIds()
    const delegableBundles = Array.from(registry.values()).filter(bundle => delegable(bundle, privileged))
    const registeredPermission = (permissionId, bundleGroupId) =>
        rows(BUNDLE_PERMISSION, {userPermissionId: permissionId}).find(
            row =>
                registry.has(row.userGroupId) &&
                (!bundleGroupId || row.userGroupId === bundleGroupId) &&
                delegableBundles.some(bundle => bundle.userGroupId === row.userGroupId)
        ) || null
    const registeredArtifact = (artifactGroupId, bundleGroupId) =>
        rows(BUNDLE_ARTIFACT, {artifactGroupId}).find(
            row =>
                registry.has(row.userGroupId) &&
                (!bundleGroupId || row.userGroupId === bundleGroupId) &&
                delegableBundles.some(bundle => bundle.userGroupId === row.userGroupId)
        ) || null
    const grants = rows(GRANT, {userGroupId: group.userGroupId})
    const authz = rows(AUTHZ, {userGroupId: group.userGroupId})
    return changes.map(change => {
        const result = Object.assign({}, change, {allowed: true, noChange: false, reasonCode: null, reason: null, sealed: false})
        const refuse = (reasonCode, reason) => Object.assign(result, {allowed: false, reasonCode, reason})
        if (change.kind === "bundle") return refuse("UNREGISTERED_BUNDLE", change.id + " is not a registered bundle")
        if (change.kind === "permission") {
            result.sealed = sealed.has(change.id)
            result.label = one("moqui.security.UserPermission", {userPermissionId: change.id})?.description || change.id
            const openRows = grants.filter(row => row.userPermissionId === change.id && current(row, at))
            result.before = openRows.length ? "granted" : "not granted"
            if (change.action === "add") {
                const definition = registeredPermission(change.id, change.bundleGroupId)
                if (!definition)
                    return refuse("UNREGISTERED_GRANT", change.id + " is not a permission of a registered bundle you may assign")
                result.capabilityKindEnumId = definition.capabilityKindEnumId || null
                result.bundleGroupId = definition.userGroupId
                if (openRows.length) Object.assign(result, {noChange: true, after: "granted"})
                else result.after = "granted"
            } else {
                if (!openRows.length) Object.assign(result, {noChange: true, after: "not granted"})
                else result.after = "ended"
            }
            if (!result.noChange && result.sealed && !ec.user.hasPermission(change.id))
                return refuse("SEALED_PERMISSION", change.id + " is sealed: only a holder of it may grant or end it")
            return result
        }
        // an artifact binding: only a registered one, added or ended with the type the bundle registers, never a deny
        const definition = registeredArtifact(change.id, change.bundleGroupId)
        result.label = one("moqui.security.ArtifactGroup", {artifactGroupId: change.id})?.description || change.id
        if (!definition)
            return refuse("UNREGISTERED_GRANT", change.id + " is not an artifact binding of a registered bundle you may assign")
        result.bundleGroupId = definition.userGroupId
        result.authzTypeEnumId = definition.authzTypeEnumId
        const matching = authz.filter(
            row => row.artifactGroupId === change.id && row.authzTypeEnumId === definition.authzTypeEnumId
        )
        result.before = matching.length ? "granted" : "not granted"
        if (change.action === "add")
            Object.assign(result, matching.length ? {noChange: true, after: "granted"} : {after: "granted"})
        else Object.assign(result, matching.length ? {after: "ended"} : {noChange: true, after: "not granted"})
        return result
    })
}
function grantPreview(p) {
    const group = one(GROUP, {userGroupId: p.userGroupId})
    if (!group) throw new Error("Group " + p.userGroupId + " is not available")
    const refusal = groupRefusal(group)
    const changes = evaluate(group, requested(p))
    const memberCount = memberCounts().get(group.userGroupId) || 0
    const refusedChanges = changes.filter(change => !change.allowed)
    return {
        group,
        changes,
        memberCount,
        revision: grantRevision(group.userGroupId),
        allowed: !refusal && refusedChanges.length === 0,
        reasonCode: refusal?.reasonCode || refusedChanges[0]?.reasonCode || null,
        reason: refusal?.reason || refusedChanges[0]?.reason || null
    }
}
// preview#Grants: what the change would grant and end, for how many members, and what is refused and why; nothing is written.
function previewGrants(p) {
    requireView()
    const preview = grantPreview(p)
    const effective = preview.changes.filter(change => change.allowed && !change.noChange)
    return {
        changeList: preview.changes,
        memberCount: preview.memberCount,
        allowed: preview.allowed,
        reasonCode: preview.reasonCode,
        reason: preview.reason,
        revision: preview.revision,
        summary:
            effective.length === 0
                ? "Nothing changes"
                : effective.filter(change => change.action === "add").length +
                  " grant(s) added and " +
                  effective.filter(change => change.action === "end").length +
                  " ended, for " +
                  preview.memberCount +
                  " current member(s)"
    }
}
// apply#Grants: the previewed change, refused as a whole when any part is refused or the grants changed since the preview (a
// group's grants are one change, previewed as one).
function applyGrants(p) {
    requireManage()
    const reason = reasonText(p)
    find(GROUP, {userGroupId: p.userGroupId}).forUpdate(true).one()
    const preview = grantPreview(p)
    if (!preview.allowed) return refused(preview.reasonCode, preview.reason, {changeList: preview.changes})
    if (!p.expectedRevision || p.expectedRevision !== preview.revision)
        return refused("REVISION_CHANGED", "The group's grants changed since the preview; preview again", {
            revision: preview.revision
        })
    const path = identity.administrationPath({})
    if (!path.allowed) return refused(path.reasonCode, path.reason)
    const groupId = preview.group.userGroupId
    const now = nowMs()
    auditReason(reason, ["fromDate", "thruDate", "authzTypeEnumId"])
    let added = 0
    let ended = 0
    for (const change of preview.changes.filter(change => !change.noChange)) {
        if (change.kind === "permission" && change.action === "add") {
            const fromDate = change.fromDate ? Math.max(ms(change.fromDate), now) : now
            const value = ec.entity.makeValue(GRANT)
            value.setFields(
                {userGroupId: groupId, userPermissionId: change.id, fromDate: iso(fromDate), thruDate: iso(ms(change.thruDate))},
                true,
                null,
                null
            )
            value.create()
            added++
        } else if (change.kind === "permission") {
            const thruDate = change.thruDate ? Math.max(ms(change.thruDate), now) : now
            for (const row of rows(GRANT, {userGroupId: groupId, userPermissionId: change.id}).filter(row => current(row, now))) {
                const value = ec.entity.makeValue(GRANT)
                value.setFields(
                    {
                        userGroupId: groupId,
                        userPermissionId: change.id,
                        fromDate: iso(ms(row.fromDate)),
                        thruDate: iso(Math.max(thruDate, ms(row.fromDate) ?? thruDate))
                    },
                    true,
                    null,
                    null
                )
                value.update()
            }
            ended++
        } else if (change.action === "add") {
            const definition = one(BUNDLE_ARTIFACT, {userGroupId: change.bundleGroupId, artifactGroupId: change.id})
            const value = ec.entity.makeValue(AUTHZ)
            value.setFields(
                {
                    userGroupId: groupId,
                    artifactGroupId: change.id,
                    authzTypeEnumId: definition.authzTypeEnumId || "AUTHZT_ALLOW",
                    authzActionEnumId: definition.authzActionEnumId || "AUTHZA_ALL"
                },
                true,
                null,
                null
            )
            value.setSequencedIdPrimary()
            value.create()
            for (const filterSetId of text(definition.entityFilterSetIds)
                .split(",")
                .map(id => id.trim())
                .filter(Boolean)) {
                const filter = ec.entity.makeValue(AUTHZ_FILTER)
                filter.setFields({artifactAuthzId: value.artifactAuthzId, entityFilterSetId: filterSetId}, true, null, null)
                filter.create()
            }
            added++
        } else {
            for (const row of rows(AUTHZ, {
                userGroupId: groupId,
                artifactGroupId: change.id,
                authzTypeEnumId: change.authzTypeEnumId
            })) {
                find(AUTHZ_FILTER, {artifactAuthzId: row.artifactAuthzId}).deleteAll()
                find(AUTHZ, {artifactAuthzId: row.artifactAuthzId}).deleteAll()
                // the audit log keeps no delete, so the removed binding is recorded here: its type before, none after
                ec.service
                    .sync()
                    .name("create#moqui.entity.EntityAuditLog")
                    .parameters({
                        changedEntityName: AUTHZ,
                        changedFieldName: "authzTypeEnumId",
                        pkPrimaryValue: row.artifactAuthzId,
                        oldValueText: row.authzTypeEnumId,
                        newValueText: null,
                        changedDate: ec.user.nowTimestamp,
                        changedByUserId: ec.user.userId,
                        changeReason: reason || "Artifact binding " + change.id + " of " + groupId + " ended"
                    })
                    .disableAuthz()
                    .call()
            }
            ended++
        }
    }
    return {applied: true, added, ended, revision: grantRevision(groupId), changeList: preview.changes}
}

// ===== preferences =====
// A registered preference key's value type (its optionValue): indicator, number or text.
function preferenceType(key) {
    const type = String(key?.optionValue || "").toLowerCase()
    return ["indicator", "number", "boolean"].includes(type) ? type : "text"
}
// store#GroupPreference: a group's default for a registered preference key (a UserPreferenceKey), typed by the key; an empty
// value removes the default.
function storeGroupPreference(p) {
    requireManage()
    const reason = reasonText(p)
    const group = one(GROUP, {userGroupId: p.userGroupId})
    const refusal = groupRefusal(group)
    if (refusal) return refused(refusal.reasonCode, refusal.reason)
    const key = one("moqui.basic.Enumeration", {enumId: p.preferenceKey, enumTypeId: "UserPreferenceKey"})
    if (!key) return refused("UNREGISTERED_PREFERENCE", (p.preferenceKey || "The key") + " is not a registered preference key")
    const existing = find(PREFERENCE, {userGroupId: group.userGroupId, preferenceKey: key.enumId}).forUpdate(true).one()
    if (existing && (!p.expectedRevision || p.expectedRevision !== stamp(existing)))
        return refused("REVISION_CHANGED", "The preference changed since you read it; reload it", {revision: stamp(existing)})
    const valueText = p.preferenceValue == null ? "" : String(p.preferenceValue).trim()
    const type = preferenceType(key)
    if (valueText) {
        if (type === "indicator" && !["Y", "N"].includes(valueText)) throw new Error(key.enumId + " takes Y or N")
        if (type === "boolean" && !["true", "false"].includes(valueText)) throw new Error(key.enumId + " takes true or false")
        if (type === "number" && !Number.isFinite(Number(valueText))) throw new Error(key.enumId + " takes a number")
        if (valueText.length > 4000) throw new Error("A preference value is at most 4000 characters")
    }
    const priority =
        p.groupPriority != null && p.groupPriority !== "" ? Number(p.groupPriority) : (existing?.groupPriority ?? null)
    if (priority != null && !Number.isInteger(priority)) throw new Error("The priority is a whole number")
    auditReason(reason, ["preferenceValue"])
    const fields = {userGroupId: group.userGroupId, preferenceKey: key.enumId}
    if (!valueText) {
        if (existing) {
            find(PREFERENCE, fields).deleteAll()
            ec.service
                .sync()
                .name("create#moqui.entity.EntityAuditLog")
                .parameters({
                    changedEntityName: PREFERENCE,
                    changedFieldName: "preferenceValue",
                    pkPrimaryValue: group.userGroupId,
                    pkSecondaryValue: key.enumId,
                    oldValueText: existing.preferenceValue,
                    newValueText: null,
                    changedDate: ec.user.nowTimestamp,
                    changedByUserId: ec.user.userId,
                    changeReason: reason
                })
                .disableAuthz()
                .call()
        }
        return {applied: true, revision: null}
    }
    const value = ec.entity.makeValue(PREFERENCE)
    value.setFields(Object.assign({}, fields, {preferenceValue: valueText, groupPriority: priority}), true, null, null)
    if (existing) value.update()
    else value.create()
    return {applied: true, revision: stamp(one(PREFERENCE, fields))}
}

// ===== grant proposals (P4) =====
// A grant proposal (impl-admin §4, F11): the registered bundles an app supplies for an account it provisions (a staff account,
// later an accepted invitation or an SSO role map), checked here so the layer that provisions (Mantle) names no app group. Each
// bundle by the delegation rules of store#Membership, for the actor: registered (never a custom or unregistered group), its
// delegable permission held, a privileged group only by a member, no sealed grant carried to a non-holder. An identity
// operator (ADMIN_IDENTITY_MANAGE) proposes as an access administrator does; neither widens what the actor could assign.
function requireProposalView() {
    signedIn()
    if (![VIEW, MANAGE, "ADMIN_IDENTITY_VIEW", "ADMIN_IDENTITY_MANAGE"].some(permission => ec.user.hasPermission(permission)))
        throw new Error(VIEW + " or ADMIN_IDENTITY_VIEW is required")
}
function proposalBundles(p) {
    const ids = Array.from(
        new Set(
            Array.from(p.bundleGroupIdList || [])
                .map(text)
                .filter(Boolean)
        )
    )
    if (!ids.length) throw new Error("Name the bundles to propose")
    const registry = bundles()
    const privileged = privilegedIds()
    const sealed = sealedIds()
    return ids.map(groupId => {
        const group = one(GROUP, {userGroupId: groupId})
        const bundle = registry.get(groupId)
        const out = {
            bundleGroupId: groupId,
            appName: bundle?.appName || "",
            description: bundle?.description || group?.description || ""
        }
        if (!group || !bundle)
            return Object.assign(out, {
                allowed: false,
                reasonCode: "UNREGISTERED_BUNDLE",
                reason: groupId + " is not a registered bundle; a proposal names registered bundles only"
            })
        const refusal = groupRefusal(group, registry, privileged)
        if (refusal) return Object.assign(out, {allowed: false}, refusal)
        const carried = Array.from(
            new Set(
                rows(GRANT, {userGroupId: groupId})
                    .filter(
                        row => current(row) && sealed.has(row.userPermissionId) && !ec.user.hasPermission(row.userPermissionId)
                    )
                    .map(row => row.userPermissionId)
            )
        )
        if (carried.length)
            return Object.assign(out, {
                allowed: false,
                reasonCode: "SEALED_PERMISSION",
                reason: groupId + " grants " + carried.join(", ") + ", sealed: only a holder may give its membership"
            })
        return Object.assign(out, {allowed: true, reasonCode: null, reason: null})
    })
}
// check#GrantProposal: each bundle of a proposal, allowed or refused with its reason; nothing written.
function checkGrantProposal(p) {
    requireProposalView()
    const bundleList = proposalBundles(p)
    const first = bundleList.find(row => !row.allowed)
    return {allowed: !first, reasonCode: first?.reasonCode || null, reason: first?.reason || null, bundleList}
}
// apply#GrantProposal: the memberships of a checked proposal for one account, from fromDate (now when empty), each a dated
// row written with authorization on (the EECAs decide each); refused whole when any bundle is refused, and a bundle the
// account is already a member of now is left as it is.
function applyGrantProposal(p) {
    signedIn()
    if (!ec.user.hasPermission(MANAGE) && !ec.user.hasPermission("ADMIN_IDENTITY_MANAGE"))
        throw new Error(MANAGE + " or ADMIN_IDENTITY_MANAGE is required")
    const reason = reasonText(p)
    const account = text(p.userId) ? one("moqui.security.UserAccount", {userId: text(p.userId)}) : null
    if (!account) return refused("USER_UNAVAILABLE", "User " + text(p.userId) + " is not available")
    const bundleList = proposalBundles(p)
    const first = bundleList.find(row => !row.allowed)
    if (first) return refused(first.reasonCode, first.reason, {bundleList})
    const now = nowMs()
    const fromDate = p.fromDate != null && p.fromDate !== "" ? ms(p.fromDate) : now
    if (!Number.isFinite(fromDate) || fromDate < now - 60000) throw new Error("A proposal's memberships start now or later")
    const added = []
    auditReason(reason, ["fromDate", "thruDate"])
    for (const row of bundleList) {
        const existing = rows(MEMBER, {userGroupId: row.bundleGroupId, userId: account.userId}).some(
            member => (ms(member.thruDate) ?? Infinity) > fromDate
        )
        if (existing) continue
        const value = ec.entity.makeValue(MEMBER)
        value.setFields({userGroupId: row.bundleGroupId, userId: account.userId, fromDate: iso(fromDate)}, true, null, null)
        value.create()
        added.push(row.bundleGroupId)
    }
    return {applied: true, reasonCode: null, reason: null, addedGroupIds: added, bundleList}
}

const operations = {
    checkGrantProposal,
    applyGrantProposal,
    findGroups,
    getGroup,
    createGroup,
    storeGroup,
    storeMembership,
    endMembership,
    previewGrants,
    applyGrants,
    storeGroupPreference
}
;({
    execute: (op, parameters) => {
        if (!operations[op]) throw new Error("Unknown access operation " + op)
        return operations[op](parameters)
    }
})
