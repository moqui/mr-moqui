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
// same key or are added), groups ({userGroupId: {requireAuthcFactor, ipAllowed}}), accounts ({userId: {disabled,
// disabledDateTime, terminateDate, ipAllowed}}), endedFactorIds (factors a reset ends, P4) and disabledFlowIds (the sign-in
// providers a change disables, P10: an administrator with no password signs in only through an enabled one).
//
// P4 adds the sessions and credentials of an account (design-admin §4 Sessions, UserDetail AccessCredentials; impl-admin F3,
// F4, F42): find#Sessions and revoke#Sessions over Visit and HttpSession, for the caller's own sessions (Commons Account) or,
// with ADMIN_IDENTITY_VIEW and ADMIN_IDENTITY_MANAGE, another user's; get#AccountCredentials (metadata only, never a secret,
// hash or session token); and revoke#AccountAccess, which ends another user's sessions on every server, the login keys
// (access tokens, device keys and session keys, so a notification socket on a key closes at its next check), app passwords
// (XMPP's sign-in), SSH keys and SIP phones (each live SIP engine drops the phone), pauses the agent triggers that run as the
// user and asks the user's open agent runs to cancel. The framework knows no organization: a caller that scopes identities
// (Mantle's IdentityServices, an app's screens) checks the target first and asks for ADMIN_IDENTITY_GLOBAL where the identity
// is shared beyond the caller's scope (design §4.1).
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
// The sign-in providers enabled once the change has disabled those it names (their ids).
function enabledFlows(proposed = {}) {
    const disabled = new Set(Array.from(proposed.disabledFlowIds || []).map(String))
    return new Set(
        rows("moqui.security.sso.AuthFlow")
            .filter(row => row.disabled !== "Y" && row.inbound !== "Y" && !disabled.has(String(row.authFlowId)))
            .map(row => String(row.authFlowId))
    )
}
// Whether an administrator signs in through a provider: the account carries a provider's id, and the provider it names is
// enabled; one that names none is taken by a provider only while that is the one enabled (with several, a sign-in never
// takes an account in a privileged group, crates/moqui-core/src/service/sso.rs).
const providerSignIn = (account, flows) =>
    !!account.externalUserId && (account.externalAuthFlowId ? flows.has(String(account.externalAuthFlowId)) : flows.size === 1)

// Why the account is not a usable path at the moment, or null when it is.
function unusable(userId, at, memberRows, proposed, context) {
    const stored = one("moqui.security.UserAccount", {userId})
    if (!stored) return "NO_ACCOUNT"
    const account = Object.assign(map(stored), (proposed.accounts || {})[userId] || {})
    if (account.disabled === "Y" && !account.disabledDateTime) return "DISABLED"
    if (account.terminateDate && ms(account.terminateDate) < at) return "TERMINATED"
    if (!account.currentPassword && !providerSignIn(account, context.flows)) return "NO_CREDENTIAL"
    const groupIds = memberRows.filter(row => row.userId === userId && open(row, at)).map(row => row.userGroupId)
    if (!groupIds.includes(ADMIN_GROUP)) return "NOT_ADMINISTRATOR"
    groupIds.push("ALL_USERS")
    const groups = groupIds.map(groupId => groupFlags(groupId, proposed, context.groups))
    const endedFactors = new Set(Array.from(proposed.endedFactorIds || []).map(String))
    const factor = rows("moqui.security.UserAuthcFactor", {userId}).some(
        row => open(row, at) && row.needsValidation !== "Y" && !endedFactors.has(String(row.factorId))
    )
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
    const context = {clientIp: String(ec.user.getClientIp() || ""), flows: enabledFlows(proposed), groups: new Map()}
    const before = countPaths(nowMs(), {}, Object.assign({}, context, {flows: enabledFlows(), groups: new Map()}))
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

// ===== sessions and credentials (P4) =====
const IDENTITY_VIEW = "ADMIN_IDENTITY_VIEW"
const IDENTITY_MANAGE = "ADMIN_IDENTITY_MANAGE"
const VISIT = "moqui.server.Visit"
const SESSION = "moqui.server.HttpSession"
const LOGIN_KEY = "moqui.security.UserLoginKey"
const APP_PASSWORD = "moqui.security.UserAppPassword"
const SSH_KEY = "moqui.security.UserSshKey"
const SIP_DEVICE = "moqui.call.SipDevice"
const FACTOR = "moqui.security.UserAuthcFactor"
const ACCOUNT = "moqui.security.UserAccount"
const DAY = 86400000
const accessKinds = ["sessions", "loginKeys", "appPasswords", "sshKeys", "sipDevices", "agents"]
const text = value => String(value ?? "").trim()
const signedInUser = () => {
    if (!ec.user.userId) throw new Error("Sign in to see sessions")
    return ec.user.userId
}
// The target of a session or credential service: the caller's own account, or another one when the caller holds the
// permission (the caller's own scope check, for an app that scopes identities, comes first).
function target(p, permission) {
    const me = signedInUser()
    const userId = text(p.userId) || me
    if (userId !== me && !ec.user.hasPermission(permission))
        throw new Error(permission + " is required for another user's account")
    return {userId, self: userId === me}
}
// A browser and system from a user agent, as Commons Account names a device; the agent itself is not returned.
function clientSummary(agent) {
    const value = String(agent || "")
    if (!value) return "Unknown device"
    const browser = /Edg\//.test(value)
        ? "Edge"
        : /Firefox\//.test(value)
          ? "Firefox"
          : /Chrome\//.test(value)
            ? "Chrome"
            : /Safari\//.test(value)
              ? "Safari"
              : /moqui|MR Moqui/i.test(value)
                ? "MR Moqui app"
                : "Browser"
    const system = /iPhone|iPad/.test(value)
        ? "iOS"
        : /Android/.test(value)
          ? "Android"
          : /Mac OS X/.test(value)
            ? "macOS"
            : /Windows/.test(value)
              ? "Windows"
              : /Linux/.test(value)
                ? "Linux"
                : ""
    return system ? browser + " on " + system : browser
}
const userNames = new Map()
function userName(userId) {
    if (!userNames.has(userId)) {
        const account = one(ACCOUNT, {userId})
        userNames.set(userId, {
            username: account?.username || "",
            userFullName: account?.userFullName || account?.username || userId
        })
    }
    return userNames.get(userId)
}

// Bounded delegation for an account-wide action on another user (review R1): a user in a privileged group the caller is not
// in, or holding through a current group a sealed permission the caller lacks, holds access the caller could not be given,
// so the caller may not change, sign out or take over that account (its sign-in fields, factors, locks, sessions,
// credentials, Login as). The refusal's reason, or null when the caller may act; an instance administrator holds every one.
function heldBeyond(userId) {
    const at = nowMs()
    const groupIds = rows(MEMBER, {userId})
        .filter(row => open(row, at))
        .map(row => row.userGroupId)
    if (!groupIds.length) return null
    const privileged = new Set(Array.from(UserSecurityChecks.privilegedGroups()))
    const sealed = new Set(Array.from(UserSecurityChecks.sealedPermissions()))
    const group = groupIds.find(groupId => privileged.has(groupId) && !ec.user.isInGroup(groupId))
    if (group) return "The account is in the privileged group " + group + ", which you are not in"
    const grants = Array.from(find("moqui.security.UserGroupPermission").condition("userGroupId", "in", groupIds).list()).filter(
        row => open(row, at) && sealed.has(row.userPermissionId) && !ec.user.hasPermission(row.userPermissionId)
    )
    if (grants.length) return "The account holds the sealed permission " + grants[0].userPermissionId + ", which you do not"
    return null
}

// find#Sessions: the sessions of an account (its visits, the live ones those with a web session), newest first; for the
// caller's own account any signed-in user, for another or all accounts ADMIN_IDENTITY_VIEW. A session is named by its visit
// (sessionKey), never by its cookie or token, and its client by a summary and address.
function findSessions(p) {
    const me = signedInUser()
    const listed = Array.from(p.userIdList || [])
        .map(text)
        .filter(Boolean)
    const allUsers = !!p.allUsers
    let userIds
    if (allUsers || listed.length) {
        if (!ec.user.hasPermission(IDENTITY_VIEW)) throw new Error(IDENTITY_VIEW + " is required for other users' sessions")
        userIds = allUsers ? null : listed
    } else userIds = [target(p, IDENTITY_VIEW).userId]
    const activeOnly = p.activeOnly == null ? false : p.activeOnly === true || p.activeOnly === "true" || p.activeOnly === "Y"
    const days = Math.max(1, Math.min(365, Number(p.sinceDays || 30)))
    const limit = Math.max(1, Math.min(500, Number(p.limit || 100)))
    const now = nowMs()
    const current = String(ec.user.visitId || "")
    // the live web sessions, by visit: the last time each was written stands for when it was last seen
    let sessions = find(SESSION).condition("visitId", "is-not-null", null)
    if (userIds) sessions = sessions.condition("userId", "in", userIds.length ? userIds : ["_NA_"])
    else sessions = sessions.condition("userId", "is-not-null", null)
    const live = new Map()
    for (const row of Array.from(sessions.list())) {
        const seen = ms(row.lastUpdatedStamp) ?? 0
        if (!live.has(row.visitId) || live.get(row.visitId) < seen) live.set(row.visitId, seen)
    }
    let visits = find(VISIT)
    if (userIds) visits = visits.condition("userId", "in", userIds.length ? userIds : ["_NA_"])
    else visits = visits.condition("userId", "is-not-null", null)
    if (activeOnly) visits = visits.condition("visitId", "in", live.size ? Array.from(live.keys()) : ["_NA_"])
    else visits = visits.condition("fromDate", ">=", new Date(now - days * DAY).toISOString())
    const rowsFound = Array.from(visits.orderBy("-fromDate").limit(limit).list()).map(visit => {
        const active = live.has(visit.visitId) && !visit.thruDate
        const names = userName(visit.userId)
        return {
            sessionKey: visit.visitId,
            userId: visit.userId,
            username: names.username,
            userFullName: names.userFullName,
            startedDate: ms(visit.fromDate),
            lastSeenDate: live.get(visit.visitId) ?? null,
            endedDate: ms(visit.thruDate),
            clientSummary: clientSummary(visit.initialUserAgent),
            clientAddress: visit.clientIpAddress || "",
            clientPlace: [visit.clientIpCity, visit.clientIpCountryGeoId].filter(Boolean).join(", "),
            state: active ? "active" : visit.thruDate ? "ended" : "expired",
            active,
            current: !!current && visit.visitId === current && visit.userId === me
        }
    })
    return {sessionList: rowsFound, activeCount: rowsFound.filter(row => row.active).length}
}

// revoke#Sessions: ends the named sessions (by sessionKey, a visit) on every server, the caller's own but the current one,
// or with ADMIN_IDENTITY_MANAGE another user's; with allOthers every other session of the account. Each visit is checked as
// the target's; the rest are refused, not ended.
function revokeSessions(p) {
    const me = signedInUser()
    const keys = Array.from(p.sessionKeyList || [])
        .map(text)
        .filter(Boolean)
    if (!keys.length && !p.allOthers) throw new Error("Name the sessions to end")
    const byUser = new Map()
    let refusedCount = 0
    if (p.allOthers) {
        const {userId, self} = target(p, IDENTITY_MANAGE)
        const held = self ? null : heldBeyond(userId)
        if (held) throw new Error(held + ": its sessions are not yours to end")
        byUser.set(userId, null)
    } else {
        // a visit of another account than the one named, or another user's without the permission or of an account holding
        // access the caller lacks, is refused
        const held = new Map()
        for (const visit of Array.from(find(VISIT).condition("visitId", "in", keys).list())) {
            const owner = visit.userId
            if (
                !owner ||
                (owner !== me && !ec.user.hasPermission(IDENTITY_MANAGE)) ||
                (text(p.userId) && owner !== text(p.userId))
            )
                continue
            if (owner !== me) {
                if (!held.has(owner)) held.set(owner, heldBeyond(owner))
                if (held.get(owner)) continue
            }
            if (!byUser.has(owner)) byUser.set(owner, [])
            byUser.get(owner).push(visit.visitId)
        }
        refusedCount = keys.length - Array.from(byUser.values()).reduce((sum, list) => sum + list.length, 0)
    }
    let sessionCount = 0
    let visitCount = 0
    const ended = new Set()
    for (const [userId, visitIds] of byUser) {
        try {
            sessionCount += Number(UserSecurityChecks.endUserSessions(userId, visitIds))
            visitCount += visitIds ? visitIds.length : 0
            ended.add(userId)
        } catch (error) {
            // one account's failure is named; the others' sessions still end
            ec.logger.error("Sessions of " + userId + " not ended: " + error.message)
            refusedCount += visitIds ? visitIds.length : 1
        }
    }
    if (byUser.size)
        ec.logger.info("Sessions ended by " + me + " for " + Array.from(byUser.keys()).join(", ") + ": " + sessionCount)
    // another account's sessions ended by an administrator are evidence (impl-admin P17): who, whose, and how many were named;
    // none for an account whose sessions could not be ended
    for (const [userId, visitIds] of byUser)
        if (userId !== me && ended.has(userId))
            accessEvidence(userId, "sessions", visitIds ? visitIds.length + " sessions ended" : "Every session ended", null)
    return {sessionCount, visitCount, refusedCount}
}

// An administrative action on an account that changes no audit-logged field (sessions ended, credentials revoked) is a row
// of the audit log on the account, with who did it, the session it was done in and the reason given; never a credential.
function accessEvidence(userId, fieldName, summary, reason) {
    ec.resource
        .script("component://framework/service/org/moqui/admin/ConfigurationLib.js")
        .auditRow(ACCOUNT, fieldName, userId, null, null, summary, reason)
}

// get#AccountCredentials: an account's credentials, metadata only (never a secret, hash, verifier or factor option): the
// sign-in factors, access tokens, app and device keys, app passwords, SSH keys and phones, current ones (with includeEnded
// the ended too). The caller's own, or another's with ADMIN_IDENTITY_VIEW.
function accountCredentials(p) {
    const {userId} = target(p, IDENTITY_VIEW)
    const all = !!p.includeEnded
    const now = nowMs()
    const live = row => all || (ms(row.thruDate) ?? Infinity) > now
    const factorNames = new Map(
        rows("moqui.basic.Enumeration", {enumTypeId: "UserAuthcFactorType"}).map(row => [
            row.enumId,
            row.description || row.enumId
        ])
    )
    const factors = rows(FACTOR, {userId})
        .filter(row => live(row) && row.factorTypeEnumId !== "UafSingleUse")
        .map(row => ({
            factorId: row.factorId,
            kind: factorNames.get(row.factorTypeEnumId) || row.factorTypeEnumId || "",
            factorTypeEnumId: row.factorTypeEnumId || "",
            verified: row.needsValidation !== "Y",
            fromDate: ms(row.fromDate),
            thruDate: ms(row.thruDate)
        }))
    const singleUse = rows(FACTOR, {userId, factorTypeEnumId: "UafSingleUse"}).filter(
        row => (ms(row.thruDate) ?? Infinity) > now
    ).length
    const credentials = []
    for (const row of rows(LOGIN_KEY, {userId}).filter(live)) {
        const kind = row.deviceName ? "deviceKey" : row.description ? "accessToken" : "loginKey"
        credentials.push({
            kind,
            credentialId: row.loginKey,
            description: row.deviceName || row.description || "Sign-in key",
            fromDate: ms(row.fromDate),
            lastUsedDate: ms(row.lastUsedDate),
            thruDate: ms(row.thruDate)
        })
    }
    for (const row of rows(APP_PASSWORD, {userId}).filter(row => all || !row.revokedDate))
        credentials.push({
            kind: "appPassword",
            credentialId: row.appPasswordId,
            description: row.name || "App password",
            fromDate: ms(row.createdDate),
            lastUsedDate: ms(row.lastUsedDate),
            thruDate: ms(row.revokedDate)
        })
    for (const row of rows(SSH_KEY, {userId}).filter(live))
        credentials.push({
            kind: "sshKey",
            credentialId: row.sshKeyId,
            description: row.description || "SSH key",
            fingerprint: row.fingerprint || "",
            fromDate: ms(row.fromDate),
            thruDate: ms(row.thruDate)
        })
    for (const row of rows(SIP_DEVICE, {userId}).filter(row => all || row.statusId === "SdsActive"))
        credentials.push({
            kind: "sipDevice",
            credentialId: row.sipDeviceId,
            description: row.name || "Phone",
            fromDate: ms(row.createdDate),
            lastUsedDate: ms(row.lastSeenDate),
            thruDate: ms(row.revokedDate)
        })
    return {factorList: factors, singleUseCodeCount: singleUse, credentialList: credentials}
}

// End one credential of the account, or every one of its kind: true when one was current.
function endCredential(userId, kind, credentialId) {
    const now = ec.user.nowTimestamp
    const byId = (entity, idField, conditions = {}) => {
        let found = find(entity, Object.assign({userId}, conditions))
        if (credentialId) found = found.condition(idField, credentialId)
        return Array.from(found.list())
    }
    let ended = 0
    if (kind === "loginKeys" || kind === "accessToken" || kind === "deviceKey" || kind === "loginKey") {
        for (const row of byId(LOGIN_KEY, "loginKey").filter(row => (ms(row.thruDate) ?? Infinity) > nowMs())) {
            row.set("thruDate", now)
            row.update()
            ended++
        }
    } else if (kind === "appPasswords" || kind === "appPassword") {
        for (const row of byId(APP_PASSWORD, "appPasswordId").filter(row => !row.revokedDate)) {
            row.set("revokedDate", now)
            row.update()
            ended++
        }
    } else if (kind === "sshKeys" || kind === "sshKey") {
        for (const row of byId(SSH_KEY, "sshKeyId").filter(row => (ms(row.thruDate) ?? Infinity) > nowMs())) {
            row.set("thruDate", now)
            row.update()
            ended++
        }
    } else if (kind === "sipDevices" || kind === "sipDevice") {
        const out = ec.resource
            .script("component://framework/service/org/moqui/call/SipDevices.js")
            .execute("revokeUserDevices", {userId, sipDeviceId: credentialId || null})
        ended += Number(out.revokedCount || 0)
    } else if (kind === "agents") {
        // the triggers that run as the user (theirs, or their agent's run-as user) pause with their cron jobs
        const agentIds = new Set(rows("moqui.agent.AgentDefinition", {runAsUserId: userId}).map(row => row.agentId))
        for (const trigger of rows("moqui.agent.AgentTrigger")) {
            const runsAs = trigger.runAsUserId ? trigger.runAsUserId === userId : agentIds.has(trigger.agentId)
            if (!runsAs || trigger.paused === "Y") continue
            trigger.set("paused", "Y")
            trigger.update()
            const job = one("moqui.service.job.ServiceJob", {jobName: "agent_trigger_" + trigger.triggerId})
            if (job && job.paused !== "Y") {
                job.set("paused", "Y")
                job.update()
            }
            ended++
        }
        // the user's open runs are asked to cancel; the node running each reads it at its next renewal
        for (const run of Array.from(
            find("moqui.agent.AgentRun")
                .condition("statusId", "in", ["AgrSubmitted", "AgrWorking", "AgrInputRequired", "AgrAuthRequired"])
                .list()
        )) {
            if ((run.userId !== userId && run.runAsUserId !== userId) || run.cancelRequested === "Y") continue
            run.set("cancelRequested", "Y")
            run.update()
            ended++
        }
    } else throw new Error("Unknown credential kind " + kind)
    return ended
}

// revoke#AccountAccess: another user's access ended at once (impl-admin F3, F42): their sessions on every server, login keys,
// app passwords, SSH keys and phones, the agent triggers that run as them paused and their open agent runs asked to cancel;
// or one credential (credentialKind and credentialId). The account itself is left as it is: disabling it is the lifecycle's
// (P6), and its owner signs in again unless the account is disabled. Needs ADMIN_IDENTITY_MANAGE (the caller scopes the
// target and asks for ADMIN_IDENTITY_GLOBAL where the identity is shared); the caller's own credentials are Commons
// Account's self-service. Each kind on its own: one that fails is named and the others still end.
function revokeAccountAccess(p) {
    const me = signedInUser()
    const userId = text(p.userId)
    if (!userId) throw new Error("userId is required")
    if (!ec.user.hasPermission(IDENTITY_MANAGE)) throw new Error(IDENTITY_MANAGE + " is required")
    if (userId === me) throw new Error("Your own sign-ins and credentials are ended in your account settings")
    const account = one(ACCOUNT, {userId})
    if (!account) throw new Error("User " + userId + " is not available")
    const held = heldBeyond(userId)
    if (held) throw new Error(held + ": revoking its access is refused")
    const reason = text(p.reason).slice(0, 255)
    const single = text(p.credentialKind)
    const kinds = single
        ? [single]
        : Array.from(p.kindList || accessKinds)
              .map(text)
              .filter(kind => accessKinds.includes(kind))
    const counts = {}
    const failed = []
    for (const kind of kinds) {
        try {
            if (kind === "sessions") {
                counts.sessions = Number(UserSecurityChecks.endUserSessions(userId, null))
                // a session another server holds that no row names yet signs out at its next request (the session refresh)
                if (!single) {
                    const value = ec.entity.makeValue(ACCOUNT)
                    value.setFields({userId, hasLoggedOut: "Y"}, true, null, null)
                    value.update()
                }
            } else counts[kind] = endCredential(userId, kind, single ? text(p.credentialId) || null : null)
        } catch (error) {
            ec.logger.error("Access of " + userId + ": " + kind + " not revoked: " + error.message)
            failed.push(kind)
        }
    }
    // the evidence of it (impl-admin P17): the kinds and how many of each, never a credential
    accessEvidence(
        userId,
        "access",
        (
            Object.entries(counts)
                .map(([kind, count]) => kind + ": " + count)
                .join(", ") + (failed.length ? "; not revoked: " + failed.join(", ") : "")
        ).slice(0, 255) || "Nothing to revoke",
        reason || null
    )
    ec.logger.warn(
        "Account access of " +
            account.username +
            " revoked by " +
            ec.user.username +
            " (" +
            JSON.stringify(counts) +
            ")" +
            (reason ? ": " + reason : "")
    )
    return {revokedCounts: counts, failedKinds: failed}
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
        accounts: Object.assign({}, proposed.accounts || {}),
        endedFactorIds: Array.from(proposed.endedFactorIds || []),
        disabledFlowIds: Array.from(proposed.disabledFlowIds || [])
    })
}

const operations = {checkPath, findSessions, revokeSessions, accountCredentials, revokeAccountAccess}
;({
    administrationPath,
    heldBeyond,
    clientSummary,
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown identity operation " + op)
        return operations[op](parameters)
    }
})
