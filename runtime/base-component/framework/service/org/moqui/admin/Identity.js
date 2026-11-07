// The generic identity guards of org.moqui.admin.IdentityServices (mr-boss plan/design-admin.md §17.2; impl-admin P3, F41): the
// administration path. A change to a membership, a group's sign-in flags or (later) an account, a factor or a provider is refused
// when it would leave no usable instance administrator where there was one. Counted from the database with authorization off,
// as the login rules decide (crates/moqui-core/src/user/login.rs gate_account, check_ip_allowed; factor.rs
// second_factor_required): nothing here is cluster-specific, since a stale cluster member can only over-grant until the
// session refresh rechecks it.
//
// A usable path is an account that, after the change, at each moment the change names from now on:
// - is a date-valid member of ADMIN, the framework's instance administrators (the framework names no app's administrator group;
//   an app's administrator holds no Tools System and cannot repair ADMIN, so it is not a path);
// - is not permanently disabled (a tarpit lock, disabled with a disabledDateTime, expires) and not past its terminateDate;
// - can authenticate: a password, or an external identity with an enabled sign-in flow;
// - has a date-valid, validated factor when any of its date-valid groups requires one (a required factor with none in
//   date never completes a sign-in);
// - is admitted by the ipAllowed of the account and of every date-valid group, ALL_USERS included, from the acting
//   administrator's own address (no address, as for a job, checks nothing, as login does).
//
// The proposed change is a map: memberships (rows {userGroupId, userId, fromDate, thruDate} that replace the stored row of the
// same key or are added), groups ({userGroupId: {requireAuthcFactor, ipAllowed}}) and accounts ({userId: {disabled,
// disabledDateTime, terminateDate}}). Later phases add the factor and provider changes their services make.
const ADMIN_GROUP = "ADMIN"
const MEMBER = "moqui.security.UserGroupMember"
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
const open = (row, at) => (ms(row.fromDate) ?? -Infinity) <= at && (ms(row.thruDate) ?? Infinity) > at
const nowMs = () => ms(ec.user.nowTimestamp)
const memberKey = row => row.userGroupId + "\u0000" + row.userId + "\u0000" + ms(row.fromDate)

// The membership rows of the users the paths need, with the proposed rows over the stored ones.
function memberships(userIds, proposed) {
    const byKey = new Map()
    if (userIds.length)
        for (const row of Array.from(find(MEMBER).condition("userId", "in", userIds).list()).map(map))
            byKey.set(memberKey(row), row)
    for (const row of proposed.memberships || []) if (userIds.includes(row.userId)) byKey.set(memberKey(row), row)
    return Array.from(byKey.values())
}
// The accounts that may be a path: every user with a stored or proposed ADMIN membership.
function candidates(proposed) {
    const ids = new Set(rows(MEMBER, {userGroupId: ADMIN_GROUP}).map(row => row.userId))
    for (const row of proposed.memberships || []) if (row.userGroupId === ADMIN_GROUP) ids.add(row.userId)
    return Array.from(ids)
}
function groupFlags(groupId, proposed, cache) {
    if (!cache.has(groupId)) {
        const stored = map(one("moqui.security.UserGroup", {userGroupId: groupId}))
        cache.set(groupId, Object.assign(stored, (proposed.groups || {})[groupId] || {}))
    }
    return cache.get(groupId)
}
const enabledFlows = () => rows("moqui.security.sso.AuthFlow").some(row => row.disabled !== "Y")

// Why the account is not a usable path at the moment, or null when it is.
function unusable(userId, at, memberRows, proposed, context) {
    const stored = one("moqui.security.UserAccount", {userId})
    if (!stored) return "NO_ACCOUNT"
    const account = Object.assign(map(stored), (proposed.accounts || {})[userId] || {})
    if (account.disabled === "Y" && !account.disabledDateTime) return "DISABLED"
    if (account.terminateDate && ms(account.terminateDate) < at) return "TERMINATED"
    if (!account.currentPassword && !(account.externalUserId && context.flows)) return "NO_CREDENTIAL"
    const groupIds = memberRows.filter(row => row.userId === userId && open(row, at)).map(row => row.userGroupId)
    if (!groupIds.includes(ADMIN_GROUP)) return "NOT_ADMINISTRATOR"
    groupIds.push("ALL_USERS")
    const groups = groupIds.map(groupId => groupFlags(groupId, proposed, context.groups))
    const factor = rows("moqui.security.UserAuthcFactor", {userId}).some(row => open(row, at) && row.needsValidation !== "Y")
    if (!factor && groups.some(group => group.requireAuthcFactor === "Y")) return "FACTOR_REQUIRED"
    if (context.clientIp) {
        const patterns = [account.ipAllowed, ...groups.map(group => group.ipAllowed)].filter(
            pattern => !!String(pattern || "").trim()
        )
        if (patterns.length && !patterns.some(pattern => WebUtilities.ipMatches(String(pattern), context.clientIp)))
            return "IP_NOT_ALLOWED"
    }
    return null
}
// The usable paths at a moment, with the change applied or not.
function countPaths(at, proposed, context) {
    const userIds = candidates(proposed)
    const memberRows = memberships(userIds, proposed)
    return userIds.filter(userId => unusable(userId, at, memberRows, proposed, context) === null).length
}
// The moments to check: now, and every later date the change names (an ending at a future date leaves the path until then).
function moments(proposed) {
    const now = nowMs()
    const dates = new Set([now])
    for (const row of proposed.memberships || [])
        for (const date of [ms(row.fromDate), ms(row.thruDate)]) if (date != null && date > now) dates.add(date)
    return Array.from(dates).sort((a, b) => a - b)
}

// Whether the change keeps a usable administration path: allowed, with the paths before and after (counts only, no names), or
// refused with ADMINISTRATION_PATH and the reason. A change on an instance that had no path is not refused for it.
function administrationPath(proposed = {}) {
    const context = {clientIp: String(ec.user.getClientIp() || ""), flows: enabledFlows(), groups: new Map()}
    const before = countPaths(nowMs(), {}, Object.assign({}, context, {groups: new Map()}))
    let after = before
    for (const at of moments(proposed)) {
        after = countPaths(at, proposed, context)
        if (before > 0 && after === 0)
            return {
                allowed: false,
                reasonCode: "ADMINISTRATION_PATH",
                reason:
                    "This change would leave no administrator who can sign in" +
                    (at > nowMs() ? " from " + new Date(at).toISOString() : "") +
                    ": keep another member of ADMIN who is enabled, can authenticate, meets the factor requirement and is admitted from your address",
                pathsBefore: before,
                pathsAfter: 0
            }
    }
    return {allowed: true, reasonCode: null, reason: null, pathsBefore: before, pathsAfter: after}
}

// check#AdministrationPath: for a caller who reads access or identities; the answer is a reason, never an error.
function checkPath(p) {
    if (!ec.user.userId) throw new Error("Sign in to check the administration path")
    if (!["ADMIN_ACCESS_VIEW", "ADMIN_IDENTITY_VIEW"].some(permission => ec.user.hasPermission(permission)))
        throw new Error("ADMIN_ACCESS_VIEW or ADMIN_IDENTITY_VIEW is required")
    const proposed = p.proposed && typeof p.proposed === "object" ? p.proposed : {}
    return administrationPath({
        memberships: Array.from(proposed.memberships || []).map(map),
        groups: Object.assign({}, proposed.groups || {}),
        accounts: Object.assign({}, proposed.accounts || {})
    })
}

;({
    administrationPath,
    execute: (op, parameters) => {
        if (op !== "checkPath") throw new Error("Unknown identity operation " + op)
        return checkPath(parameters)
    }
})
