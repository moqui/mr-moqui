// The lockout preview of a group's sign-in rules, org.moqui.admin.AccessServices.preview#GroupSignInRules (mr-boss
// plan/design-admin.md §6 GroupSignInRules; impl-admin P10): what requiring a factor of a group's members, or admitting
// them from some addresses only, would do, before store#Group makes the change. Counts only: no member is named, so the
// preview needs no organization scope. Nothing is written. Access.js gives its helpers (lib) and Identity.js the
// administration path, which store#Group checks again when it saves.
const access = ec.resource.script("component://framework/service/org/moqui/admin/Access.js").lib
const {find, one, rows, text, open, stamp, identity} = access
const GROUP = "moqui.security.UserGroup"

function previewGroupSignInRules(p) {
    access.requireView()
    const group = one(GROUP, {userGroupId: text(p.userGroupId)})
    const refusal = access.groupRefusal(group)
    const revision = stamp(group)
    if (refusal) return {allowed: false, reasonCode: refusal.reasonCode, reason: refusal.reason, changed: false, revision}
    const factorNow = group.requireAuthcFactor === "Y"
    const factorAfter = p.requireAuthcFactor == null ? factorNow : p.requireAuthcFactor === true || p.requireAuthcFactor === "Y"
    const ipNow = text(group.ipAllowed)
    const ipAfter = p.ipAllowed == null ? ipNow : text(p.ipAllowed)
    if (!access.validIpAllowed(ipAfter))
        return {
            allowed: false,
            reasonCode: "IP_INVALID",
            reason: "Allowed IPs are comma separated IPv4 addresses (each part a number, * or a range like 0-31) or IPv6 addresses",
            changed: false,
            revision
        }
    const memberIds = Array.from(
        new Set(
            rows("moqui.security.UserGroupMember", {userGroupId: group.userGroupId})
                .filter(row => open(row))
                .map(row => row.userId)
        )
    )
    // the members with no confirmed factor in date: once a factor is required their sign-in never completes
    const withFactor = new Set()
    if (memberIds.length)
        for (const row of Array.from(find("moqui.security.UserAuthcFactor").condition("userId", "in", memberIds).list()))
            if (open(row) && row.needsValidation !== "Y") withFactor.add(row.userId)
    const membersWithoutFactor = memberIds.filter(userId => !withFactor.has(userId)).length
    const clientIp = String(ec.user.getClientIp() || "")
    const callerAdmitted =
        !ipAfter || !clientIp
            ? true
            : ipAfter
                  .split(",")
                  .map(pattern => pattern.trim())
                  .some(pattern => WebUtilities.ipMatches(pattern, clientIp))
    const path = identity.administrationPath({
        groups: {[group.userGroupId]: {requireAuthcFactor: factorAfter ? "Y" : "N", ipAllowed: ipAfter || null}}
    })
    const notes = []
    if (factorAfter && membersWithoutFactor)
        notes.push(
            membersWithoutFactor +
                " of " +
                memberIds.length +
                (memberIds.length === 1 ? " member has" : " members have") +
                " no confirmed factor and could not sign in until one is set up"
        )
    else if (factorAfter) notes.push("Every member has a confirmed factor")
    if (ipAfter) notes.push(callerAdmitted ? "Members sign in only from " + ipAfter : "Your own address is not among " + ipAfter)
    if (!factorAfter && !ipAfter) notes.push("Members sign in with no factor or address requirement of this group")
    return {
        allowed: path.allowed,
        reasonCode: path.reasonCode,
        reason: path.reason,
        changed: factorAfter !== factorNow || ipAfter !== ipNow,
        memberCount: memberIds.length,
        membersWithoutFactor: factorAfter ? membersWithoutFactor : 0,
        callerAdmitted,
        pathsBefore: path.pathsBefore,
        pathsAfter: path.pathsAfter,
        summary: notes.join(". "),
        revision
    }
}

const operations = {previewGroupSignInRules}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown sign-in rule operation " + op)
        return operations[op](parameters)
    }
})
