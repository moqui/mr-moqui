// Capabilities, row rules and the access review, org.moqui.admin.AccessServices' read projections over the capability registry
// (mr-boss plan/design-admin.md §5 Capabilities and RowRules, §16 the access and grant review; impl-admin P9). Descriptive
// only: the registry (moqui.security.CapabilityBundle and its permissions and artifact bindings) describes the native
// permissions, artifact authorizations and filter sets, and the framework evaluator stays the one that decides. Nothing here
// writes; every change goes through preview#Grants and apply#Grants (Access.js), which recheck each one.
//
// - get#Capabilities: the registered capabilities of an app as rows (a launcher binding, or a permission by its kind: view,
//   change, export, sensitive values, each a row of its own, so a launcher never reads as a view or a change and a change never
//   as a view) against a bounded set of groups, each cell with what the group holds now and whether the actor may grant or end
//   it, and the same cells as a list (the matrix's twin for a phone or the terminal). What a group holds that no registered
//   bundle describes (another authorization type, an unregistered artifact group or permission) is listed apart, read only.
// - get#ScopeRules: the row rules in effect, each a filter set bound to a group's artifact binding with where it comes from (a
//   registered bundle, or deployed configuration), and the registered filter sets a registered binding lacks (one with no
//   filter set of its own), which apply#Grants may bind. A filter's expression and apply condition are not returned: they are Tools'.
// - get#GrantReview: the grants, app gates and members of the groups reviewed as of a date and time, for print and CSV. Sealed
//   permissions are left out and counted; allowed IPs, preference values and filter expressions are never in it.
//
// Reading needs ADMIN_ACCESS_VIEW (or ADMIN_ACCESS_MANAGE). The framework knows no organization: a caller that scopes members
// (an app) filters the review's members itself.
const access = ec.resource.script("component://framework/service/org/moqui/admin/Access.js")
const lib = access.lib
const GROUP = "moqui.security.UserGroup"
const MEMBER = "moqui.security.UserGroupMember"
const AUTHZ = "moqui.security.ArtifactAuthz"
const AUTHZ_FILTER = "moqui.security.ArtifactAuthzFilter"
const FILTER_SET = "moqui.security.EntityFilterSet"
const FILTER = "moqui.security.EntityFilter"
const BUNDLE_PERMISSION = "moqui.security.CapabilityBundlePermission"
const BUNDLE_ARTIFACT = "moqui.security.CapabilityBundleArtifact"
const MANAGE = "ADMIN_ACCESS_MANAGE"
// the groups a matrix compares at once, and those shown when only an app is chosen
const maxGroups = 8
const defaultGroups = 6
const maxReviewGroups = 60
const kindOrder = ["launcher", "view", "edit", "export", "sensitive"]
const kindLabels = {launcher: "Launcher", view: "View", edit: "Change", export: "Export", sensitive: "Sensitive values"}
const kindByEnum = {CpkView: "view", CpkEdit: "edit", CpkExport: "export", CpkSensitive: "sensitive"}
const typeText = enumId => ({AUTHZT_ALLOW: "Allow", AUTHZT_ALWAYS: "Always", AUTHZT_DENY: "Deny"})[enumId] || enumId || ""

const distinct = values => Array.from(new Set(values))
const idsOf = value =>
    distinct(
        Array.from(value || [])
            .map(lib.text)
            .filter(Boolean)
    )
function pushTo(found, key, value) {
    if (!found.has(key)) found.set(key, [])
    found.get(key).push(value)
}

// The registry by what it describes: each permission and each artifact group with the bundles in use that carry it, and whether
// the actor may grant from each (Access.js delegable).
function registryIndex(context) {
    const delegable = new Map(
        Array.from(context.registry.values()).map(bundle => [bundle.userGroupId, lib.delegable(bundle, context.privileged)])
    )
    const permissions = new Map()
    const artifacts = new Map()
    for (const row of lib.rows(BUNDLE_PERMISSION).map(lib.map)) {
        const bundle = context.registry.get(row.userGroupId)
        if (bundle) pushTo(permissions, row.userPermissionId, {bundle, row, delegable: delegable.get(bundle.userGroupId)})
    }
    for (const row of lib.rows(BUNDLE_ARTIFACT).map(lib.map)) {
        const bundle = context.registry.get(row.userGroupId)
        if (bundle) pushTo(artifacts, row.artifactGroupId, {bundle, row, delegable: delegable.get(bundle.userGroupId)})
    }
    return {
        permissions,
        artifacts,
        permissionDescriptions: new Map(
            lib.rows("moqui.security.UserPermission").map(row => [row.userPermissionId, row.description || ""])
        ),
        artifactDescriptions: new Map(
            lib.rows("moqui.security.ArtifactGroup").map(row => [row.artifactGroupId, row.description || ""])
        ),
        groups: new Map(lib.rows(GROUP).map(row => [row.userGroupId, row]))
    }
}
// A group as a column or a review heading: its summary and whether the actor may change its grants, with the reason when not.
function groupColumn(group, context, withRevision) {
    const refusal = lib.groupRefusal(group, context.registry, context.privileged)
    const mayManage = ec.user.hasPermission(MANAGE)
    return Object.assign(lib.groupSummary(group, context), {
        canManage: mayManage && !refusal,
        manageReasonCode: refusal?.reasonCode || (mayManage ? null : "PERMISSION_DENIED"),
        manageReason: refusal?.reason || (mayManage ? null : MANAGE + " is required to change grants"),
        grantRevision: withRevision ? lib.grantRevision(group.userGroupId) : null
    })
}

// ===== capabilities =====
// The capability rows of the apps asked for: one per app and launcher binding or permission, with the bundles that carry it.
function capabilityRows(p, context, index) {
    const appName = lib.text(p.appName)
    const byKey = new Map()
    const add = (entry, capabilityKey, fields) => {
        const bundle = entry.bundle
        if (appName && bundle.appName !== appName) return
        const rowKey = (bundle.appName || "") + "|" + capabilityKey
        if (!byKey.has(rowKey))
            byKey.set(
                rowKey,
                Object.assign({appName: bundle.appName || "", capabilityKey, bundleGroupIds: [], scopeDependencies: []}, fields)
            )
        const row = byKey.get(rowKey)
        row.bundleGroupIds.push(bundle.userGroupId)
        if (bundle.scopeDependency && !row.scopeDependencies.includes(bundle.scopeDependency))
            row.scopeDependencies.push(bundle.scopeDependency)
    }
    for (const [artifactGroupId, entries] of index.artifacts)
        for (const entry of entries)
            add(entry, "a:" + artifactGroupId, {
                kind: "launcher",
                id: artifactGroupId,
                label: entry.row.description || index.artifactDescriptions.get(artifactGroupId) || artifactGroupId,
                sealed: false,
                authzTypeEnumId: entry.row.authzTypeEnumId || "AUTHZT_ALLOW",
                registeredFilterSetIds: lib.idList(entry.row.entityFilterSetIds),
                // any registered bundle the actor may assign offers it, in whatever app (as preview#Grants decides)
                offered: entries.some(other => other.delegable)
            })
    for (const [userPermissionId, entries] of index.permissions)
        for (const entry of entries)
            add(entry, "p:" + userPermissionId, {
                kind: kindByEnum[entry.row.capabilityKindEnumId] || "edit",
                id: userPermissionId,
                label: index.permissionDescriptions.get(userPermissionId) || userPermissionId,
                sealed: context.sealed.has(userPermissionId),
                authzTypeEnumId: null,
                registeredFilterSetIds: [],
                offered: entries.some(other => other.delegable)
            })
    const term = lib.text(p.text).toLowerCase()
    const kind = lib.text(p.kind)
    return Array.from(byKey.values())
        .filter(row => !kind || row.kind === kind)
        .filter(row => !term || [row.id, row.label].some(value => String(value).toLowerCase().includes(term)))
        .map(row =>
            Object.assign(row, {
                kindLabel: kindLabels[row.kind],
                scopeDependency: row.scopeDependencies.join("; "),
                bundleGroupIds: distinct(row.bundleGroupIds)
            })
        )
        .sort(
            (a, b) =>
                a.appName.localeCompare(b.appName) ||
                kindOrder.indexOf(a.kind) - kindOrder.indexOf(b.kind) ||
                a.id.localeCompare(b.id)
        )
}
// What a group holds of a capability now, and whether the actor may grant or end it (preview#Grants decides again): a
// launcher binding only of the registered type, anything else on the artifact group shown as an advanced rule, never changed.
function capabilityCell(row, group, context, proposed) {
    const groupId = group.userGroupId
    const cell = {
        userGroupId: groupId,
        state: "none",
        fromDate: null,
        thruDate: null,
        filterSetIds: [],
        unscoped: false,
        advancedType: null,
        canAdd: false,
        canEnd: false,
        reasonCode: null,
        reason: null,
        proposed: proposed.get(groupId + "|" + row.capabilityKey) || null
    }
    if (row.kind === "launcher") {
        const bindings = (context.authz.get(groupId) || []).filter(authz => authz.artifactGroupId === row.id)
        const registered = bindings.filter(authz => authz.authzTypeEnumId === row.authzTypeEnumId)
        if (registered.length) {
            const bound = registered.map(authz => lib.filterSetIds(authz.artifactAuthzId))
            cell.state = "granted"
            cell.filterSetIds = distinct(bound.flat()).sort()
            // lacking a registered row rule, unless the binding carries one of its own (scoped on purpose, Tools')
            cell.unscoped = bound.some(
                ids =>
                    ids.every(id => row.registeredFilterSetIds.includes(id)) &&
                    row.registeredFilterSetIds.some(id => !ids.includes(id))
            )
        } else if (bindings.length) {
            cell.state = "advanced"
            cell.advancedType = distinct(bindings.map(authz => typeText(authz.authzTypeEnumId))).join(", ")
        }
    } else {
        const held = (context.grants.get(groupId) || []).filter(
            grant => grant.userPermissionId === row.id && lib.current(grant, context.at)
        )
        const now = held.find(grant => lib.open(grant, context.at))
        if (held.length) {
            cell.state = now ? "granted" : "scheduled"
            cell.fromDate = lib.ms((now || held[0]).fromDate)
            cell.thruDate = lib.ms((now || held[0]).thruDate)
        }
    }
    const refuse = (reasonCode, reason) => Object.assign(cell, {reasonCode, reason})
    if (!group.canManage) return refuse(group.manageReasonCode, group.manageReason)
    if (cell.state === "advanced")
        return refuse(
            "UNSUPPORTED_RULE",
            "An unregistered rule (" + cell.advancedType + "); an instance administrator changes it in Tools"
        )
    if (row.sealed && !ec.user.hasPermission(row.id))
        return refuse("SEALED_PERMISSION", row.id + " is sealed: only a holder of it may grant or end it")
    if (cell.state === "none") {
        if (!row.offered) return refuse("NOT_DELEGABLE", "No registered bundle you may assign offers " + row.id)
        cell.canAdd = true
    } else cell.canEnd = row.kind !== "launcher" || row.offered
    return cell
}
// What the groups hold that no registered bundle describes: read only, never a checkbox.
function unsupportedRules(groups, context, index) {
    const found = []
    for (const group of groups) {
        const base = {userGroupId: group.userGroupId, groupDescription: group.description || group.userGroupId}
        for (const authz of context.authz.get(group.userGroupId) || []) {
            const entries = index.artifacts.get(authz.artifactGroupId) || []
            if (entries.some(entry => entry.row.authzTypeEnumId === authz.authzTypeEnumId)) continue
            found.push(
                Object.assign({}, base, {
                    ruleType: "artifact",
                    id: authz.artifactGroupId,
                    label: index.artifactDescriptions.get(authz.artifactGroupId) || authz.artifactGroupId,
                    typeText: typeText(authz.authzTypeEnumId) + (authz.authzServiceName ? ", decided by a service" : ""),
                    filterSetIds: lib.filterSetIds(authz.artifactAuthzId),
                    reason: entries.length ? "Registered with another type" : "Not a registered app gate"
                })
            )
        }
        for (const grant of context.grants.get(group.userGroupId) || []) {
            if (!lib.current(grant, context.at) || index.permissions.has(grant.userPermissionId)) continue
            found.push(
                Object.assign({}, base, {
                    ruleType: "permission",
                    id: grant.userPermissionId,
                    label: index.permissionDescriptions.get(grant.userPermissionId) || grant.userPermissionId,
                    typeText: context.sealed.has(grant.userPermissionId) ? "Sealed permission" : "Permission",
                    filterSetIds: [],
                    reason: "Not in a registered bundle"
                })
            )
        }
    }
    return found.sort(
        (a, b) => a.userGroupId.localeCompare(b.userGroupId) || a.ruleType.localeCompare(b.ruleType) || a.id.localeCompare(b.id)
    )
}
// get#Capabilities: the matrix and its list.
function getCapabilities(p) {
    lib.requireView()
    const context = lib.listContext()
    const index = registryIndex(context)
    const appName = lib.text(p.appName)
    let groupIds = idsOf(p.userGroupIdList)
    if (groupIds.length > maxGroups) throw new Error("Compare at most " + maxGroups + " groups at a time")
    let moreGroupCount = 0
    if (!groupIds.length && appName) {
        const appBundles = Array.from(context.registry.values())
            .filter(bundle => bundle.appName === appName)
            .sort((a, b) => (a.sequenceNum ?? 0) - (b.sequenceNum ?? 0) || a.userGroupId.localeCompare(b.userGroupId))
        groupIds = appBundles.slice(0, defaultGroups).map(bundle => bundle.userGroupId)
        moreGroupCount = Math.max(0, appBundles.length - defaultGroups)
    }
    const groups = groupIds.filter(id => index.groups.has(id)).map(id => groupColumn(index.groups.get(id), context, true))
    const proposed = new Map()
    for (const item of Array.from(p.proposedList || []).map(lib.map)) {
        const key = item.userPermissionId
            ? "p:" + item.userPermissionId
            : item.artifactGroupId
              ? "a:" + item.artifactGroupId
              : null
        if (key && item.userGroupId) proposed.set(item.userGroupId + "|" + key, item.action === "end" ? "end" : "add")
    }
    const capabilityList = capabilityRows(p, context, index).map(row =>
        Object.assign(row, {cells: groups.map(group => capabilityCell(row, group, context, proposed))})
    )
    // the list twin: the same cells, one row each
    const cellList = capabilityList.flatMap(row =>
        row.cells.map((cell, position) => {
            const item = Object.assign({}, row, cell, {groupDescription: groups[position].description || cell.userGroupId})
            delete item.cells
            return item
        })
    )
    return {
        appNames: distinct(
            Array.from(context.registry.values())
                .map(bundle => bundle.appName)
                .filter(Boolean)
        ).sort(),
        kindOptions: kindOrder.map(kind => ({value: kind, label: kindLabels[kind]})),
        groupList: groups,
        moreGroupCount,
        maxGroups,
        capabilityList,
        cellList,
        unsupportedList: unsupportedRules(groups, context, index),
        canManage: ec.user.hasPermission(MANAGE)
    }
}

// ===== row rules =====
// Where a bound filter set comes from: the registry (the group's own bundle, or the bundle a grant copied it from), else the
// deployed configuration (seed data or Tools).
function ruleSource(authz, filterSetId, entries) {
    const naming = entries.filter(
        entry =>
            entry.row.authzTypeEnumId === authz.authzTypeEnumId && lib.idList(entry.row.entityFilterSetIds).includes(filterSetId)
    )
    if (naming.some(entry => entry.bundle.userGroupId === authz.userGroupId))
        return {
            source: "registered",
            sourceText: "Registered bundle " + authz.userGroupId,
            sourceBundleGroupId: authz.userGroupId
        }
    if (naming.length)
        return {
            source: "bundle",
            sourceText: "As bundle " + naming[0].bundle.userGroupId + " registers it",
            sourceBundleGroupId: naming[0].bundle.userGroupId
        }
    return {source: "configuration", sourceText: "Deployed configuration or Tools", sourceBundleGroupId: null}
}
// get#ScopeRules: the row rules in effect and the registered ones a registered binding lacks.
function getScopeRules(p) {
    lib.requireView()
    const context = lib.listContext()
    const index = registryIndex(context)
    const sets = new Map(lib.rows(FILTER_SET).map(row => [row.entityFilterSetId, lib.map(row)]))
    const entityNames = new Map()
    for (const row of lib.rows(FILTER)) if (row.entityName) pushTo(entityNames, row.entityFilterSetId, row.entityName)
    const boundByAuthz = new Map()
    for (const row of lib.rows(AUTHZ_FILTER).map(lib.map)) pushTo(boundByAuthz, row.artifactAuthzId, row)
    const columns = new Map()
    const column = groupId => {
        if (!columns.has(groupId)) {
            const group = index.groups.get(groupId)
            columns.set(groupId, group ? groupColumn(group, context, true) : {canManage: false, grantRevision: null})
        }
        return columns.get(groupId)
    }
    const rules = []
    const registeredSetIds = new Set()
    for (const entries of index.artifacts.values())
        for (const entry of entries) for (const id of lib.idList(entry.row.entityFilterSetIds)) registeredSetIds.add(id)
    for (const authz of lib.rows(AUTHZ).map(lib.map)) {
        const entries = index.artifacts.get(authz.artifactGroupId) || []
        const bound = boundByAuthz.get(authz.artifactAuthzId) || []
        const base = {
            artifactAuthzId: authz.artifactAuthzId,
            userGroupId: authz.userGroupId,
            groupDescription: index.groups.get(authz.userGroupId)?.description || authz.userGroupId,
            artifactGroupId: authz.artifactGroupId,
            artifactDescription: index.artifactDescriptions.get(authz.artifactGroupId) || authz.artifactGroupId,
            authzTypeEnumId: authz.authzTypeEnumId,
            typeText: typeText(authz.authzTypeEnumId),
            registeredBinding: entries.some(entry => entry.row.authzTypeEnumId === authz.authzTypeEnumId)
        }
        const rule = (filterSetId, fields) =>
            Object.assign({}, base, fields, {
                entityFilterSetId: filterSetId,
                filterDescription: sets.get(filterSetId)?.description || filterSetId,
                entityNames: distinct(entityNames.get(filterSetId) || []).sort()
            })
        for (const filter of bound)
            rules.push(
                rule(
                    filter.entityFilterSetId,
                    Object.assign(
                        {
                            state: "bound",
                            conditional: !!(filter.applyCond || sets.get(filter.entityFilterSetId)?.applyCond),
                            canBind: false,
                            grantRevision: null
                        },
                        ruleSource(authz, filter.entityFilterSetId, entries)
                    )
                )
            )
        // a registered binding without a filter set its bundle registers: what apply#Grants may bind. One that carries a
        // filter set the registry does not name for this artifact group was scoped on purpose and is left as it is.
        const typed = entries.filter(entry => entry.row.authzTypeEnumId === authz.authzTypeEnumId)
        const registered = distinct(typed.flatMap(entry => lib.idList(entry.row.entityFilterSetIds)))
        const missing = bound.some(filter => !registered.includes(filter.entityFilterSetId))
            ? []
            : registered.filter(id => sets.has(id) && !bound.some(filter => filter.entityFilterSetId === id))
        for (const filterSetId of missing) {
            const offering = typed.find(
                entry => entry.delegable && lib.idList(entry.row.entityFilterSetIds).includes(filterSetId)
            )
            rules.push(
                rule(filterSetId, {
                    state: "missing",
                    conditional: !!sets.get(filterSetId)?.applyCond,
                    canBind: !!offering && column(authz.userGroupId).canManage,
                    grantRevision: column(authz.userGroupId).grantRevision,
                    source: "registry",
                    sourceText: "Registered for this app gate, not bound",
                    sourceBundleGroupId: (offering || typed[0]).bundle.userGroupId
                })
            )
        }
    }
    const term = lib.text(p.text).toLowerCase()
    const matching = rules
        .filter(row => !p.userGroupId || row.userGroupId === p.userGroupId)
        .filter(row => !p.artifactGroupId || row.artifactGroupId === p.artifactGroupId)
        .filter(row => !p.entityFilterSetId || row.entityFilterSetId === p.entityFilterSetId)
        .filter(row => !p.state || row.state === p.state)
        .filter(row => !p.source || row.source === p.source)
        .filter(
            row =>
                !term ||
                [row.userGroupId, row.groupDescription, row.artifactGroupId, row.artifactDescription].some(value =>
                    String(value).toLowerCase().includes(term)
                )
        )
        .sort(
            (a, b) =>
                Number(b.state === "missing") - Number(a.state === "missing") ||
                a.userGroupId.localeCompare(b.userGroupId) ||
                a.artifactGroupId.localeCompare(b.artifactGroupId) ||
                a.entityFilterSetId.localeCompare(b.entityFilterSetId)
        )
    const pageSize = Math.min(Math.max(Number(p.pageSize) || 50, 1), 200)
    const pageIndex = Math.max(Number(p.pageIndex) || 0, 0)
    const counts = new Map()
    for (const row of rules)
        if (row.state === "bound") counts.set(row.entityFilterSetId, (counts.get(row.entityFilterSetId) || 0) + 1)
    return {
        ruleList: matching.slice(pageIndex * pageSize, (pageIndex + 1) * pageSize),
        ruleCount: matching.length,
        missingCount: matching.filter(row => row.state === "missing").length,
        pageIndex,
        pageSize,
        filterSetList: Array.from(sets.values())
            .map(set => ({
                entityFilterSetId: set.entityFilterSetId,
                description: set.description || set.entityFilterSetId,
                conditional: !!set.applyCond,
                entityNames: distinct(entityNames.get(set.entityFilterSetId) || []).sort(),
                bindingCount: counts.get(set.entityFilterSetId) || 0,
                registered: registeredSetIds.has(set.entityFilterSetId)
            }))
            .sort((a, b) => a.entityFilterSetId.localeCompare(b.entityFilterSetId)),
        groupOptions: distinct(rules.map(row => row.userGroupId))
            .sort()
            .map(id => ({value: id, label: index.groups.get(id)?.description || id})),
        artifactGroupOptions: distinct(rules.map(row => row.artifactGroupId))
            .sort()
            .map(id => ({value: id, label: index.artifactDescriptions.get(id) || id})),
        canManage: ec.user.hasPermission(MANAGE)
    }
}

// ===== the access and grant review =====
// get#GrantReview: what each reviewed group grants and to whom, as of a date and time. Sealed permissions are left out and
// counted; an app gate carries no dates, so it is shown as it is now.
function getGrantReview(p) {
    lib.requireView()
    const context = lib.listContext()
    const index = registryIndex(context)
    const at = p.asOfDate != null && p.asOfDate !== "" ? lib.ms(p.asOfDate) : context.at
    if (!Number.isFinite(at)) throw new Error("Give the review a date and time")
    const appName = lib.text(p.appName)
    let groupIds = idsOf(p.userGroupIdList)
    if (!groupIds.length && appName)
        groupIds = Array.from(context.registry.values())
            .filter(bundle => bundle.appName === appName)
            .sort((a, b) => (a.sequenceNum ?? 0) - (b.sequenceNum ?? 0) || a.userGroupId.localeCompare(b.userGroupId))
            .map(bundle => bundle.userGroupId)
    if (!groupIds.length) throw new Error("Name the groups or the app to review")
    if (groupIds.length > maxReviewGroups) throw new Error("Review at most " + maxReviewGroups + " groups at a time")
    const groupList = []
    const entryList = []
    const memberList = []
    let sealedOmittedCount = 0
    for (const groupId of groupIds) {
        const group = index.groups.get(groupId)
        if (!group) continue
        const bundle = context.registry.get(groupId)
        const base = {userGroupId: groupId, groupDescription: group.description || groupId}
        let sealedOmitted = 0
        for (const grant of (context.grants.get(groupId) || []).filter(row => lib.open(row, at))) {
            if (context.sealed.has(grant.userPermissionId)) {
                sealedOmitted++
                continue
            }
            const entries = index.permissions.get(grant.userPermissionId) || []
            const kind = entries.length ? kindByEnum[entries[0].row.capabilityKindEnumId] || "edit" : null
            entryList.push(
                Object.assign({}, base, {
                    entryType: "permission",
                    id: grant.userPermissionId,
                    label: index.permissionDescriptions.get(grant.userPermissionId) || grant.userPermissionId,
                    kind,
                    kindLabel: kind ? kindLabels[kind] : "Not registered",
                    fromDate: lib.ms(grant.fromDate),
                    thruDate: lib.ms(grant.thruDate),
                    scope:
                        bundle?.scopeDependency ||
                        distinct(entries.map(entry => entry.bundle.scopeDependency).filter(Boolean)).join("; "),
                    registered: entries.length > 0
                })
            )
        }
        for (const authz of context.authz.get(groupId) || []) {
            const entries = index.artifacts.get(authz.artifactGroupId) || []
            const registered = entries.some(entry => entry.row.authzTypeEnumId === authz.authzTypeEnumId)
            entryList.push(
                Object.assign({}, base, {
                    entryType: "appGate",
                    id: authz.artifactGroupId,
                    label: index.artifactDescriptions.get(authz.artifactGroupId) || authz.artifactGroupId,
                    kind: registered ? "launcher" : null,
                    kindLabel: (registered ? kindLabels.launcher : "Advanced rule") + " · " + typeText(authz.authzTypeEnumId),
                    fromDate: null,
                    thruDate: null,
                    scope: lib.filterSetIds(authz.artifactAuthzId).join(", "),
                    registered
                })
            )
        }
        const members = lib.rows(MEMBER, {userGroupId: groupId}).filter(row => lib.open(row, at))
        for (const member of members)
            memberList.push(
                Object.assign({}, base, lib.userLabel(member.userId), {
                    userId: member.userId,
                    fromDate: lib.ms(member.fromDate),
                    thruDate: lib.ms(member.thruDate)
                })
            )
        sealedOmittedCount += sealedOmitted
        groupList.push(
            Object.assign({}, base, {
                appName: bundle?.appName || "",
                kind: bundle ? "registered" : lib.isCustom(group, context.purposeIds) ? "custom" : "other",
                privileged: context.privileged.has(groupId),
                memberCount: members.length,
                sealedOmitted
            })
        )
    }
    const order = (a, b) => groupIds.indexOf(a.userGroupId) - groupIds.indexOf(b.userGroupId)
    return {
        asOfDate: at,
        groupList,
        entryList: entryList.sort((a, b) => order(a, b) || b.entryType.localeCompare(a.entryType) || a.id.localeCompare(b.id)),
        memberList: memberList.sort((a, b) => order(a, b) || a.userFullName.localeCompare(b.userFullName)),
        sealedOmittedCount
    }
}

const operations = {getCapabilities, getScopeRules, getGrantReview}
;({
    execute: (op, parameters) => {
        if (!operations[op]) throw new Error("Unknown access projection " + op)
        return operations[op](parameters)
    }
})
