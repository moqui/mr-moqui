// Sharing a dashboard or a playlist with a group (screen/Dashboards/Dashboard.xml's share and Playlists.xml's
// sharePlaylist; docs/tools/Monitoring.md, "Sharing"; plans/Admin-Screens-Down.md, Track I). A definition has one group:
// its owner shares it with a group, moves it to another or makes it private again, and the group reads the definition
// only: each of its viewers still reads a dataset by their own grant. Presentation only: the groups are
// find#ShareGroups' and the save is the engine's save#Definition, each called as the viewer, so the engine checks the
// owner, the version read and the group; no entity is read here.
//
// Another app shows the screens under its own root (BOSS Insights › Dashboard and Wall display). Its policy
// (template/ReportMount.js) is read here, in the transitions too, and never from a request:
// - features.share: whether the viewer may share here. The share transitions refuse without it (requireShare) and the
//   screens offer no sharing. The engine alone would not hold it: it cannot tell an app's author from another of its
//   users who holds the engine's save;
// - shareGroups: groups offered beside the viewer's own, by the mount's words for them (an app's wall displays). Being
//   in the policy gives no right: find#ShareGroups answers such a group, and save#Definition accepts it, only for a
//   viewer whose own grant holds the group's share artifact (org.moqui.report.share.<userGroupId>).
// With no mount a viewer shares with a group of their own here, and Tools' definition editor has its group field too.
const mountLib = ec.resource.script("component://tools/template/ReportMount.js")
const {ask, offered, refuse, done} = ec.resource.script("component://tools/template/ReportDelivery.js")

const text = value => String(value ?? "").trim()
const kindWords = {dashboard: "dashboard", playlist: "playlist"}
// What a definition is for sharing here: a dashboard, a playlist (a report of kind playlist), or neither.
function kindOf(definition) {
    if (definition?.type === "dashboard") return "dashboard"
    if (definition?.type === "report" && definition.body?.kind === "playlist") return "playlist"
    return null
}
const owns = definition => !!definition && !definition.readOnly && definition.ownerId === ec.user.userId

// ===== refusals =====
function fail(words) {
    const error = new Error(words)
    error.refusal = true
    throw error
}
function attempt(action) {
    try {
        return action()
    } catch (error) {
        if (error?.refusal) return refuse(error.message)
        throw error
    }
}
// Whether the viewer may share here: the mount's policy says so (with no mount, Tools does). What a transition holds
// first; the engine then checks the owner and the group itself.
function requireShare(mount = mountLib.state()) {
    if (mount.policy.features.share !== true) fail("Sharing is not offered to you here")
    return mount
}
// Whether sharing is offered to the viewer at all: the policy, and their own grant of the two operations it asks.
const mayShare = mount => mount.policy.features.share === true && offered("save#Definition") && offered("find#ShareGroups")

// ===== groups =====
// The groups the viewer may share with here: their own by name, then the mount's by the mount's words, each of the
// mount's only when find#ShareGroups answers it (the viewer's own grant of its share artifact). present is a
// definition's group now, named when it can be.
function groupsFor(mount, present) {
    const offeredIds = mount.policy.shareGroups.map(group => group.value)
    const named = offeredIds.concat(present && !offeredIds.includes(present) ? [present] : [])
    const found = ask("find#ShareGroups", {groupIds: named})
    const groups = Array.from(found.out?.groups || [])
    const accepted = id => groups.some(group => group.userGroupId === id)
    const options = groups
        .filter(group => group.own && !offeredIds.includes(group.userGroupId))
        .map(group => ({value: group.userGroupId, label: group.description}))
        .concat(
            mount.policy.shareGroups
                .filter(group => accepted(group.value))
                .map(group => ({value: group.value, label: group.label}))
        )
    const names = Object.fromEntries(groups.map(group => [group.userGroupId, group.description]))
    for (const option of options) names[option.value] = option.label
    return {options, names, failure: found.failure}
}

// ===== what a screen shows =====
// The share dialog's state for one definition the viewer reads: whether it is offered (the viewer owns it, it is one of
// the kinds this screen shares, and sharing is theirs here), the groups to choose from, the group it is shared with now
// by name, and the form's values.
function state(definition, kinds) {
    const none = {shareable: false, groups: [], userGroupId: "", sharedWith: "", values: {}}
    const kind = kindOf(definition)
    if (!kind || !Array.from(kinds || []).includes(kind) || !owns(definition)) return none
    const mount = mountLib.state()
    if (!mayShare(mount)) return none
    const present = text(definition.userGroupId)
    const found = groupsFor(mount, present || null)
    if (found.failure) return none
    return {
        shareable: true,
        kind: kindWords[kind],
        groups: found.options,
        userGroupId: present,
        sharedWith: present ? found.names[present] || present : "",
        values: {definitionId: definition.id, expectedVersion: definition.version, userGroupId: present}
    }
}
// For a list of definitions (the playlists): whether sharing is offered here at all, the groups to choose from, and
// each group's name for the ones the list shows as shared.
function listState(groupIds) {
    const mount = mountLib.state()
    if (!mayShare(mount)) return {mayShare: false, groups: [], names: {}}
    const found = groupsFor(mount, null)
    const more = Array.from(new Set(Array.from(groupIds || []).filter(id => id && !Object.hasOwn(found.names, id))))
    if (more.length)
        for (const group of Array.from(ask("find#ShareGroups", {groupIds: more}).out?.groups || []))
            found.names[group.userGroupId] = group.description
    return {mayShare: !found.failure, groups: found.options, names: found.names}
}

// ===== the save =====
// share: the owner shares a definition of one of these kinds with a group offered here, or makes it private (no
// userGroupId). Refused without the mount's feature, for a definition that is not the viewer's own or not of these
// kinds, and for a group that is neither the viewer's own nor one of the mount's that they may name; the engine's save
// then checks the owner, the version read (expectedVersion) and the group again. Answers {definitionId, userGroupId},
// or null with the refusal as the transition's error.
function share(p, kinds) {
    return attempt(() => {
        const mount = requireShare()
        const allowed = Array.from(kinds || [])
        const what = allowed.map(kind => kindWords[kind]).join(" or ")
        const read = ask("get#Definition", {definitionId: text(p.definitionId)})
        const definition = read.out?.definition
        const kind = kindOf(definition)
        if (!definition || !kind || !allowed.includes(kind)) fail("Choose a " + what + " you read")
        if (!owns(definition)) fail("Only the owner shares a " + kindWords[kind] + "; save a copy first")
        const groupId = text(p.userGroupId) || null
        // a group the definition already has is kept as it is; any other is one offered here
        if (groupId && groupId !== text(definition.userGroupId)) {
            const found = groupsFor(mount, null)
            if (found.failure) fail(found.failure)
            if (!found.options.some(option => option.value === groupId)) fail("Share only with a group you belong to")
        }
        const expected = text(p.expectedVersion)
        const saved = ask("save#Definition", {
            definition: {
                id: definition.id,
                type: definition.type,
                title: definition.title,
                body: definition.body,
                version: expected && Number.isInteger(Number(expected)) ? Number(expected) : definition.version,
                userGroupId: groupId
            },
            note: groupId ? "Shared with " + groupId : "Made private"
        })
        if (saved.failure) fail(saved.failure)
        done(groupId ? "Shared" : "Made private")
        return {definitionId: saved.out.definition.id, userGroupId: saved.out.definition.userGroupId || null}
    })
}

;({kindOf, owns, requireShare, state, listState, share})
