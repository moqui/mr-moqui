// Shared devices, org.moqui.admin.DeviceServices (mr-boss plan/design-admin.md §6 Devices, §14 shared-device registration;
// design-crosscut CX-DEVICE; impl-admin P18; docs/security/Shared-Devices.md): a browser registered to open one route
// group as a display account. The credential itself (the cookie, its checks on every request, the route gate) is
// crates/moqui-core/src/user/device.rs; these services register, list and revoke, and redeem a registration code for the
// browser that enters it.
//
// - A route group is a moqui.security.DeviceRouteGroup row, seed data of the app that owns its screens. A row naming a
//   missing artifact group is logged by name when read and left out.
// - A display account is a UserAccount with displayAccount Y: no password, no external identity, nothing that signs it in,
//   in no privileged group and holding no sealed permission (UserSecurityChecks.displayAccountRefusal, the same check each
//   request of its devices makes). The registrar may name only one whose every current group is one the registrar could
//   assign (AccessServices' rules: a registered bundle or a custom group, its delegable permission, a privileged group only
//   by a member, no sealed grant carried to a non-holder), so registering a device gives nothing the registrar could not.
// - The registration code is made by the framework (about 99 bits), returned once by register#Device and kept only as its
//   hash; redeem#Device takes it once, within codeMinutes, and the device's secret then lives only in that browser's
//   cookie. Neither is logged, listed or put in an audit row.
// - The framework knows no organization: a caller that scopes identities (Mantle's IdentityServices, an app's screens)
//   checks the display account first and passes find#Devices the accounts in its scope.
// - What happens to a device is evidence on its display account in the audit log (deviceRegistered, deviceRedeemed,
//   deviceRevoked, deviceRefused, deviceStopped; the device the secondary key), read through the projections its
//   organization's owner registers.
const access = ec.resource.script("component://framework/service/org/moqui/admin/Access.js").lib
const configuration = ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationLib.js")
const identity = ec.resource.script("component://framework/service/org/moqui/admin/Identity.js")
const {find, one, rows, map, ms, text, open, nowMs} = access
const DEVICE = "moqui.security.DeviceCredential"
const ROUTE_GROUP = "moqui.security.DeviceRouteGroup"
const ACCOUNT = "moqui.security.UserAccount"
const VIEW = "ADMIN_ACCESS_VIEW"
const MANAGE = "ADMIN_ACCESS_MANAGE"
const MINUTE = 60000
const DAY = 86400000
const DEFAULT_IDLE_MINUTES = 10080
const ISSUE_MINUTES = 15
const iso = value => (value == null ? null : new Date(value).toISOString())
const refused = (reasonCode, reason, extra = {}) => Object.assign({applied: false, reasonCode, reason}, extra)
const controls = /[\u0000-\u001f\u007f]/

function requireView() {
    if (!ec.user.userId) throw new Error("Sign in to administer devices")
    if (!ec.user.hasPermission(VIEW) && !ec.user.hasPermission(MANAGE)) throw new Error(VIEW + " is required")
}
function requireManage() {
    if (!ec.user.userId) throw new Error("Sign in to administer devices")
    if (!ec.user.hasPermission(MANAGE)) throw new Error(MANAGE + " is required")
}
function plain(value, what, maxLength = 255) {
    const found = text(value)
    if (controls.test(found) || found.includes("${")) throw new Error(what + " may not hold control characters or ${")
    if (found.length > maxLength) throw new Error(what + " is longer than " + maxLength + " characters")
    return found
}
function bounded(value, fallback, low, high, what) {
    if (value == null || value === "") return fallback
    const number = Number(value)
    if (!Number.isInteger(number) || number < low || number > high)
        throw new Error(what + " is a whole number from " + low + " to " + high)
    return number
}

// ===== route groups =====
// The registered route groups in use: enabled rows whose artifact group exists.
function routeGroups() {
    const found = []
    for (const row of rows(ROUTE_GROUP).sort((a, b) => (a.sequenceNum ?? 0) - (b.sequenceNum ?? 0))) {
        if (row.disabled === "Y") continue
        if (!row.artifactGroupId || !one("moqui.security.ArtifactGroup", {artifactGroupId: row.artifactGroupId})) {
            ec.logger.error("Device route group " + row.routeGroupId + " names an artifact group that does not exist; left out")
            continue
        }
        found.push(map(row))
    }
    return found
}
const routeGroupSummary = row => ({
    routeGroupId: row.routeGroupId,
    description: row.description || row.routeGroupId,
    ownerName: row.ownerName || "",
    parameterName: row.parameterName || "",
    parameterTitle: row.parameterTitle || "",
    parameterRequired: row.parameterRequired === "Y",
    defaultIdleMinutes: Number(row.defaultIdleMinutes || DEFAULT_IDLE_MINUTES),
    defaultLifetimeDays: Math.min(Number(row.defaultLifetimeDays || 365), 730)
})
function findRouteGroups() {
    requireView()
    return {routeGroupList: routeGroups().map(routeGroupSummary)}
}

// ===== the display account =====
// Why the actor may not register a device for the account, or null: it must be a display account fit for a device, and
// every group it is in now or later one the actor could assign.
function displayRefusal(account) {
    const unfit = UserSecurityChecks.displayAccountRefusal(account.userId)
    if (unfit) return {reasonCode: "DISPLAY_ACCOUNT", reason: "A device cannot run as " + account.username + ": " + unfit}
    const at = nowMs()
    const registry = access.bundles()
    const privileged = access.privilegedIds()
    const sealed = access.sealedIds()
    const groupIds = Array.from(
        new Set(
            rows("moqui.security.UserGroupMember", {userId: account.userId})
                .filter(row => (ms(row.thruDate) ?? Infinity) > at)
                .map(row => row.userGroupId)
        )
    )
    for (const groupId of groupIds) {
        const group = one("moqui.security.UserGroup", {userGroupId: groupId})
        const refusal = access.groupRefusal(group, registry, privileged)
        if (refusal)
            return {
                reasonCode: "DISPLAY_GROUP",
                reason: account.username + " is in " + groupId + ", which you could not assign: " + refusal.reason
            }
        const carried = rows("moqui.security.UserGroupPermission", {userGroupId: groupId}).find(
            row => (ms(row.thruDate) ?? Infinity) > at && sealed.has(row.userPermissionId)
        )
        if (carried)
            return {
                reasonCode: "SEALED_PERMISSION",
                reason: account.username + " is in " + groupId + ", which grants the sealed permission " + carried.userPermissionId
            }
    }
    return null
}

// ===== devices =====
function evidence(device, fieldName, summary, reason) {
    configuration.auditRow(ACCOUNT, fieldName, device.displayUserId, device.deviceId, null, summary, reason)
}
function deviceState(row, at) {
    if (row.revokedDate) return {state: "revoked", stateReason: "Revoked"}
    if (row.stoppedDate) {
        const reasons = {
            copied: "Stopped: a copy of its cookie was used",
            idle: "Stopped: not seen for longer than its idle timeout",
            expired: "Stopped: its registration ended"
        }
        return {state: "stopped", stateReason: reasons[row.stopReason] || "Stopped"}
    }
    if (!row.redeemedDate && !row.registrationCodeHash)
        return at - (ms(row.registeredDate) ?? 0) <= ISSUE_MINUTES * MINUTE
            ? {state: "pending", stateReason: "Waiting for its code to be shown"}
            : {state: "codeExpired", stateReason: "Its code was never shown; register it again"}
    if (!row.redeemedDate)
        return (ms(row.codeExpireDate) ?? 0) > at
            ? {state: "pending", stateReason: "Waiting for its code to be entered on the device"}
            : {state: "codeExpired", stateReason: "The code was not entered in time; register it again"}
    if ((ms(row.expireDate) ?? Infinity) <= at) return {state: "stopped", stateReason: "Its registration ended"}
    const idle = Number(row.idleTimeoutMinutes || DEFAULT_IDLE_MINUTES)
    const seen = ms(row.lastSeenDate) ?? ms(row.redeemedDate) ?? at
    if (at - seen > idle * MINUTE) return {state: "stopped", stateReason: "Not seen for longer than its idle timeout"}
    return {state: "active", stateReason: at - seen > 15 * MINUTE ? "Registered; not seen lately" : "Showing"}
}
const names = new Map()
function userLabel(userId) {
    if (!userId) return {username: "", name: ""}
    if (!names.has(userId)) {
        const account = one(ACCOUNT, {userId})
        names.set(userId, {username: account?.username || "", name: account?.userFullName || account?.username || userId})
    }
    return names.get(userId)
}
// One device as it is listed: never a hash, a code or a secret.
function deviceSummary(row, groups, at) {
    const group = groups.get(row.routeGroupId)
    const display = userLabel(row.displayUserId)
    return Object.assign(
        {
            deviceId: row.deviceId,
            deviceName: row.deviceName || row.deviceId,
            routeGroupId: row.routeGroupId,
            routeGroupDescription: group?.description || row.routeGroupId,
            parameterTitle: group?.parameterTitle || "",
            parameterValue: row.parameterValue || "",
            displayUserId: row.displayUserId,
            displayUsername: display.username,
            displayName: display.name,
            facilityId: row.facilityId || "",
            ipAllowed: row.ipAllowed || "",
            idleTimeoutMinutes: Number(row.idleTimeoutMinutes || DEFAULT_IDLE_MINUTES),
            registeredByUserId: row.registeredByUserId || "",
            registeredByName: userLabel(row.registeredByUserId).name,
            registeredDate: ms(row.registeredDate),
            redeemedDate: ms(row.redeemedDate),
            expireDate: ms(row.expireDate),
            lastSeenDate: ms(row.lastSeenDate),
            lastSeenAddress: row.lastSeenAddress || "",
            clientSummary: row.lastSeenAgent ? identity.clientSummary(row.lastSeenAgent) : "",
            revokedDate: ms(row.revokedDate),
            revokedByName: userLabel(row.revokedByUserId).name
        },
        deviceState(row, at)
    )
}
// find#Devices: the registered devices, newest first, narrowed by route group, state, site, text and last seen. With
// displayUserIdList (a caller's identity scope) only the devices of those display accounts are listed and the others counted.
function findDevices(p) {
    requireView()
    const at = nowMs()
    const groups = new Map(rows(ROUTE_GROUP).map(row => [row.routeGroupId, map(row)]))
    const scoped = p.displayUserIdList == null ? null : new Set(Array.from(p.displayUserIdList).map(text).filter(Boolean))
    let found = find(DEVICE)
    if (text(p.routeGroupId)) found = found.condition("routeGroupId", text(p.routeGroupId))
    if (text(p.facilityId)) found = found.condition("facilityId", text(p.facilityId))
    if (text(p.displayUserId)) found = found.condition("displayUserId", text(p.displayUserId))
    if (text(p.parameterValue)) found = found.condition("parameterValue", text(p.parameterValue))
    if (text(p.deviceId)) found = found.condition("deviceId", text(p.deviceId))
    let all = Array.from(found.orderBy("-registeredDate").limit(2000).list()).map(map)
    const otherCount = scoped ? all.filter(row => !scoped.has(row.displayUserId)).length : 0
    if (scoped) all = all.filter(row => scoped.has(row.displayUserId))
    let list = all.map(row => deviceSummary(row, groups, at))
    const state = text(p.state)
    if (state) list = list.filter(row => row.state === state)
    else if (p.includeEnded !== true && p.includeEnded !== "true")
        list = list.filter(row => !["revoked", "stopped", "codeExpired"].includes(row.state))
    const search = text(p.text).toLowerCase()
    if (search)
        list = list.filter(row =>
            [row.deviceName, row.displayName, row.displayUsername, row.parameterValue, row.lastSeenAddress].some(value =>
                String(value || "")
                    .toLowerCase()
                    .includes(search)
            )
        )
    const sinceDays = Number(p.seenSinceDays || 0)
    if (sinceDays > 0) list = list.filter(row => (row.lastSeenDate ?? 0) >= at - sinceDays * DAY)
    const paged = configuration.page(list, p)
    return {
        deviceList: paged.rows,
        deviceCount: paged.count,
        pageIndex: paged.pageIndex,
        pageSize: paged.pageSize,
        activeCount: list.filter(row => row.state === "active").length,
        otherCount
    }
}

// register#Device: a new registration and its code, shown once. Nothing of the route group, the home path or the fixed
// parameter's name comes from the request: only the registered row's.
function registerDevice(p) {
    requireManage()
    const deviceName = plain(p.deviceName, "The device's name", 100)
    if (!deviceName) throw new Error("Name the device")
    const operationKey = plain(p.operationKey, "The operation key", 100)
    if (operationKey) {
        // a retry answers the first registration; its code was shown once and is not kept
        const first = one(DEVICE, {operationKey})
        if (first)
            return {
                applied: true,
                repeated: true,
                reasonCode: null,
                reason: "This device is registered; its code was shown once. Revoke it and register again for a new code.",
                deviceId: first.deviceId,
                registrationCode: null,
                codeExpireDate: ms(first.codeExpireDate)
            }
    }
    const group = routeGroups().find(row => row.routeGroupId === text(p.routeGroupId))
    if (!group) return refused("ROUTE_GROUP_UNAVAILABLE", "Route group " + text(p.routeGroupId) + " is not available")
    const parameterValue = group.parameterName ? plain(p.parameterValue, group.parameterTitle || group.parameterName, 100) : ""
    if (group.parameterName && group.parameterRequired === "Y" && !parameterValue)
        return refused("PARAMETER_REQUIRED", "Choose the " + String(group.parameterTitle || group.parameterName).toLowerCase())
    const account = text(p.displayUserId) ? one(ACCOUNT, {userId: text(p.displayUserId)}) : null
    if (!account) return refused("DISPLAY_ACCOUNT", "Display account " + text(p.displayUserId) + " is not available")
    const refusal = displayRefusal(map(account))
    if (refusal) return refused(refusal.reasonCode, refusal.reason)
    const ipAllowed = plain(p.ipAllowed, "The allowed addresses")
    if (ipAllowed && !access.validIpAllowed(ipAllowed))
        throw new Error("Allowed addresses are comma separated IPv4 patterns (numbers, * or ranges) or IPv6 addresses")
    const summary = routeGroupSummary(group)
    const idleTimeoutMinutes = bounded(p.idleTimeoutMinutes, summary.defaultIdleMinutes, 5, 43200, "The idle timeout in minutes")
    const lifetimeDays = bounded(p.lifetimeDays, summary.defaultLifetimeDays, 1, 730, "The registration's days")
    const codeMinutes = bounded(p.codeMinutes, 15, 5, 60, "The code's minutes")
    const reason = plain(p.reason, "The reason")
    const now = nowMs()
    // issueCode false: the code is made when it is shown (issue#DeviceCode), so a screen carries none from its write to its render
    const withCode = !(p.issueCode === false || p.issueCode === "false")
    const made = withCode ? UserSecurityChecks.newDeviceCode() : {code: null, codeHash: null}
    const value = ec.entity.makeValue(DEVICE)
    value.setFields(
        {
            deviceName,
            routeGroupId: group.routeGroupId,
            parameterValue: parameterValue || null,
            displayUserId: account.userId,
            facilityId: plain(p.facilityId, "The site", 40) || null,
            ipAllowed: ipAllowed || null,
            idleTimeoutMinutes,
            registrationCodeHash: made.codeHash,
            codeExpireDate: withCode ? iso(now + codeMinutes * MINUTE) : null,
            registeredByUserId: ec.user.userId,
            registeredDate: iso(now),
            expireDate: iso(now + lifetimeDays * DAY),
            operationKey: operationKey || null
        },
        true,
        null,
        null
    )
    value.setSequencedIdPrimary()
    value.create()
    evidence(
        value,
        "deviceRegistered",
        "Device " + deviceName + " registered for " + group.routeGroupId + (parameterValue ? " (" + parameterValue + ")" : ""),
        reason || null
    )
    ec.logger.info("Device " + value.deviceId + " registered by " + ec.user.username + " for " + group.routeGroupId)
    return {
        applied: true,
        repeated: false,
        reasonCode: null,
        reason: null,
        deviceId: value.deviceId,
        registrationCode: made.code,
        codeExpireDate: withCode ? now + codeMinutes * MINUTE : null
    }
}

// issue#DeviceCode: the code of a registration made without one, made as it is shown: once, to the registrar who made the
// registration, within fifteen minutes of it. The write holds only while the registration has no code, so two asks show one.
function issueDeviceCode(p) {
    requireManage()
    const device = text(p.deviceId) ? find(DEVICE).condition("deviceId", text(p.deviceId)).forUpdate(true).one() : null
    if (!device) return refused("DEVICE_UNAVAILABLE", "Device " + text(p.deviceId) + " is not available")
    if (device.registeredByUserId !== ec.user.userId)
        return refused("NOT_REGISTRAR", "The code is shown to the administrator who registered the device")
    if (device.revokedDate || device.stoppedDate || device.redeemedDate)
        return refused("CODE_UNAVAILABLE", "The device is already in use or ended; register it again for a new code")
    if (device.registrationCodeHash)
        return refused("CODE_SHOWN", "The code is shown only once: revoke the device and register it again for a new code")
    const now = nowMs()
    if (now - (ms(device.registeredDate) ?? 0) > ISSUE_MINUTES * MINUTE)
        return refused("CODE_UNAVAILABLE", "The registration was not finished in time; revoke the device and register it again")
    const codeMinutes = bounded(p.codeMinutes, 15, 5, 60, "The code's minutes")
    const made = UserSecurityChecks.newDeviceCode()
    device.setFields({registrationCodeHash: made.codeHash, codeExpireDate: iso(now + codeMinutes * MINUTE)}, true, null, null)
    device.update()
    evidence(device, "deviceRegistered", "The registration code of " + (device.deviceName || device.deviceId) + " was shown to its registrar", null)
    return {
        applied: true,
        reasonCode: null,
        reason: null,
        deviceId: device.deviceId,
        deviceName: device.deviceName,
        registrationCode: made.code,
        codeExpireDate: now + codeMinutes * MINUTE
    }
}

// revoke#Device: the device ends at once, on every server: its next request reads the row and is refused, and no session
// holds its identity. A revoked device stays revoked; revoking it again answers the first revocation.
function revokeDevice(p) {
    requireManage()
    const device = text(p.deviceId) ? one(DEVICE, {deviceId: text(p.deviceId)}) : null
    if (!device) return refused("DEVICE_UNAVAILABLE", "Device " + text(p.deviceId) + " is not available")
    if (device.revokedDate) return {applied: true, repeated: true, reasonCode: null, reason: null, deviceId: device.deviceId}
    const reason = plain(p.reason, "The reason")
    const now = iso(nowMs())
    device.setFields({revokedDate: now, revokedByUserId: ec.user.userId, stopReason: "revoked"}, true, null, null)
    device.update()
    evidence(device, "deviceRevoked", "Device " + (device.deviceName || device.deviceId) + " revoked", reason || null)
    ec.logger.info("Device " + device.deviceId + " revoked by " + ec.user.username)
    return {applied: true, repeated: false, reasonCode: null, reason: null, deviceId: device.deviceId}
}

// redeem#Device: the browser of this request becomes the device of the code, once. One answer for every refusal, so the
// page says nothing of why; the reason is in the audit log (deviceRefused) and the web protection counts the attempt.
function redeemDevice(p) {
    const out = UserSecurityChecks.redeemDeviceCode(String(p.code ?? ""))
    if (!out.redeemed) return {redeemed: false, reason: "That code is not valid, was already used or has expired", homePath: null}
    const group = one(ROUTE_GROUP, {routeGroupId: out.routeGroupId})
    return {redeemed: true, reason: null, homePath: group?.homePath || "/Device"}
}
// get#DeviceStatus: what the browser of this request is registered as, for the device page: its name and where it opens.
function deviceStatus() {
    const current = UserSecurityChecks.currentDevice()
    if (!current) return {registered: false, deviceName: null, homePath: null}
    const device = one(DEVICE, {deviceId: current.deviceId})
    const group = one(ROUTE_GROUP, {routeGroupId: current.routeGroupId})
    return {registered: true, deviceName: device?.deviceName || "", homePath: group?.homePath || null}
}

const operations = {findRouteGroups, findDevices, registerDevice, issueDeviceCode, revokeDevice, redeemDevice, deviceStatus}
;({
    displayRefusal,
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown device operation " + op)
        return operations[op](parameters)
    }
})
