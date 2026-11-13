// org.moqui.admin.AuditServices (mr-boss plan/design-admin.md §12 AuditChanges, §16, §17.2; impl-admin P17): the audit log
// as a delegated audit reader may be shown it.
//
// What is read. moqui.entity.EntityAuditLog holds the old and new value of every audit-logged field of every entity of
// every organization: business records, personal data and, where a field is not encrypted, whatever was typed into it. So
// nothing here reads it by a name a request gives. A projection (moqui.server.AuditProjection) registers one field of one
// entity, and only the changes of registered projections are found, listed or counted; a request narrows them by a
// projection's id or a category, never by an entity or a field. A registration whose field is * is every other field of
// its entity, for a configuration record whose every change is an administrative act (a connection, a job): such a change
// is named by its field and is always a marker, never a value. A registration that does not read (an unknown entity, an
// unknown permission, a source service that is not defined or lacks its contract) is logged by name and left out.
//
// What is shown of a change. Who and when, the record by its key values, the reason given and the session it was made in,
// always. Its values before and after only where the registration says show: anything else is a marker (set, changed,
// cleared), and a field the entity definition encrypts, or whose name reads as a secret, is a marker whatever its
// registration says (the entity audit log keeps an encrypted field's values encrypted; a service's own row for a secret
// holds "(set)", "(replaced)" or "(cleared...)", which are shown as they are). A shown value is plain text of at most
// valueLength characters with control characters removed; the screen that shows it escapes it.
//
// Whose changes. A projection with no sourceService is a record of the whole instance (a group's grants, a connection, a
// job, a setting): a holder of ADMIN_AUDIT_VIEW reads it, with the projection's viewPermission when it names one. A
// projection with a sourceService is a record that belongs to an organization (an account, a membership): its owner finds
// the audit rows within the caller's own scope and answers their ids only (find#OwnedChanges, called through
// call#ChangeSource under the caller's own authorization), and only those rows are read here, each checked to be of a
// projection of that source. A source that fails is named and the others are answered.
//
// Bounds. A find reads at most maxDepth rows from the newest across its sources (a deeper page is refused with a notice to
// narrow the dates); a page is 1 to 100 rows; an export is one page of exportRows rows, marked truncated when more match.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationLib.js")
const {find, one, rows, map, ms, text} = lib
const LOG = "moqui.entity.EntityAuditLog"
const PROJECTION = "moqui.server.AuditProjection"
const VIEW = "ADMIN_AUDIT_VIEW"
const CALL_SOURCE = "org.moqui.admin.AuditServices.call#ChangeSource"
const maxDepth = 2000
const exportRows = 1000
const valueLength = 500
const categories = {
    access: "Access and grants",
    identity: "Users and memberships",
    signIn: "Sign-in",
    configuration: "Configuration",
    connection: "Connections",
    operation: "Operations",
    transfer: "Imports and exports",
    backup: "Backups"
}
const keyKinds = ["user", "group", "plain", "hidden"]
// The registry as it was checked, kept for the rest of the request (a module constant lasts the outermost script run): a find
// reads it, and so does each source call it makes. Who may read a projection is decided at each use, never kept.
const registry = {}
const lineText = (value, maxLength = 255) =>
    text(value)
        .replace(/[\u0000-\u001f\u007f]+/g, " ")
        .slice(0, maxLength)
const names = value => text(value).split(",").map(text).filter(Boolean)
// a field name that reads as a secret: a marker whatever its registration says. A public key or a fingerprint is not one, nor
// is a flag, a date, a count or an event about one (requirePasswordChange, passwordReset, passwordSetDate).
const secretWords = name =>
    /pass|pw$|secret|token|credential|hash|verifycode/i.test(name) || (/key/i.test(name) && !/public|fingerprint/i.test(name))
const aboutOne = name => /^(require|is|has|allow)[A-Z]|(Reset|Date|Count|Change|Changed|Type|Required|Enabled|Set)$/.test(name)
const secretName = name => secretWords(name) && !aboutOne(name)
// the fixed texts the services write for a secret's change (ConfigurationConnections, ConfigurationProviders): shown as they are
const serviceMarker = value => /^\((set|replaced|cleared[^()]{0,60})\)$/.test(text(value))

function requireView() {
    if (!ec.user.userId) throw new Error("Sign in to read the audit log")
    if (!ec.user.hasPermission(VIEW)) throw new Error(VIEW + " is required")
}

// ===== registrations =====
// Whether a registered source service is defined and carries the contract's parameters; read once for a call (sources: a Map
// the call keeps), since many projections name one source.
function sourceUsable(sourceService, sources) {
    if (!sources.has(sourceService)) {
        const definition = ec.service.getServiceDefinition(sourceService)
        sources.set(
            sourceService,
            !!definition &&
                Array.from(definition.inParameterNames || []).includes("projectionList") &&
                Object.hasOwn(definition.outParameterInfoMap || {}, "auditIdList")
        )
    }
    return sources.get(sourceService)
}
// A registration as it is used, or null: checked when read, a problem logged by the projection's id.
function checked(row, sources) {
    const projection = map(row)
    const id = text(projection.auditProjectionId)
    const entityName = text(projection.changedEntityName)
    const fieldName = text(projection.changedFieldName)
    const definition = entityName && ec.entity.isEntityDefined(entityName) ? ec.entity.getEntityDefinition(entityName) : null
    const permissions = names(projection.viewPermission)
    const sourceService = text(projection.sourceService)
    const problem = !/^[A-Za-z0-9_.-]{1,40}$/.test(id)
        ? "has no usable id"
        : !definition
          ? "names no entity"
          : definition.isViewEntity
            ? "names a view entity"
            : fieldName !== "*" && !/^[A-Za-z][A-Za-z0-9_]{0,59}$/.test(fieldName)
              ? "names no field"
              : !Object.hasOwn(categories, text(projection.category))
                ? "names no category"
                : permissions.some(permission => !one("moqui.security.UserPermission", {userPermissionId: permission}))
                  ? "names an unknown permission"
                  : sourceService && !sourceUsable(sourceService, sources)
                    ? "names a source service that is not defined or lacks its contract"
                    : null
    if (problem) {
        ec.logger.error("Audit projection " + (id || "(no id)") + " " + problem + "; left out")
        return null
    }
    // a registered event (an export, a backup request) need not be a field of the entity; a field that is one and is
    // encrypted, or any name that reads as a secret, never shows a value
    const wildcard = fieldName === "*"
    const field = wildcard ? null : definition.getFieldInfo(fieldName)
    const secret = wildcard || !!field?.encrypt || secretName(fieldName)
    const kind = value => (keyKinds.includes(text(value)) ? text(value) : "plain")
    return {
        auditProjectionId: id,
        changedEntityName: entityName,
        changedFieldName: fieldName,
        wildcard,
        category: text(projection.category),
        description: lineText(projection.description, 120) || entityName.split(".").pop() + " " + fieldName,
        show: text(projection.valueMode) === "show" && !secret,
        sourceService,
        permissions,
        primaryKeyKind: kind(projection.primaryKeyKind),
        primaryKeyTitle: lineText(projection.primaryKeyTitle, 40),
        secondaryKeyKind: kind(projection.secondaryKeyKind),
        secondaryKeyTitle: lineText(projection.secondaryKeyTitle, 40),
        linkKey: /^[A-Za-z0-9_.-]{1,40}$/.test(text(projection.linkKey)) ? text(projection.linkKey) : "",
        sequenceNum: Number(projection.sequenceNum ?? 9999)
    }
}
// The registered projections the caller may read: enabled, checked, the first of an entity's field, and every permission
// its registration names held. Each carries otherFields: the fields of its entity registered on their own, which a
// registration of every other field (*) never covers, whoever may read them and whether or not their registration reads
// (a disabled one, or one left out for a problem, hides its field: it does not fall to the wider registration).
function readable() {
    if (!registry.projections) registry.projections = registered()
    return registry.projections.filter(projection =>
        projection.permissions.every(permission => ec.user.hasPermission(permission))
    )
}
function registered() {
    const seen = new Set()
    const found = []
    const exact = new Map()
    const sources = new Map()
    for (const row of rows(PROJECTION).sort((a, b) => text(a.auditProjectionId).localeCompare(text(b.auditProjectionId)))) {
        const entityName = text(row.changedEntityName)
        if (text(row.changedFieldName) !== "*") {
            if (!exact.has(entityName)) exact.set(entityName, [])
            exact.get(entityName).push(text(row.changedFieldName))
        }
        if (row.disabled === "Y") continue
        const projection = checked(row, sources)
        if (!projection) continue
        const key = projection.changedEntityName + "\n" + projection.changedFieldName
        if (seen.has(key)) {
            ec.logger.error("Audit projection " + projection.auditProjectionId + " repeats a registered field; left out")
            continue
        }
        seen.add(key)
        found.push(projection)
    }
    for (const projection of found)
        projection.otherFields = projection.wildcard ? exact.get(projection.changedEntityName) || [] : []
    return found.sort((a, b) => a.sequenceNum - b.sequenceNum || a.auditProjectionId.localeCompare(b.auditProjectionId))
}
const keyOf = (entityName, fieldName) => entityName + "\n" + fieldName
// The projection of an audit row among those given (a Map by keyOf): its field's own, else its entity's every-other-field
// one unless the field is registered on its own; undefined for a row of nothing registered.
function projectionOf(byKey, row) {
    const own = byKey.get(keyOf(row.changedEntityName, row.changedFieldName))
    if (own) return own
    const other = byKey.get(keyOf(row.changedEntityName, "*"))
    return other && !other.otherFields.includes(text(row.changedFieldName)) ? other : undefined
}
const keyed = projections =>
    new Map(projections.map(projection => [keyOf(projection.changedEntityName, projection.changedFieldName), projection]))

// ===== a request =====
// The filters of a request as they are used: each a whole value, none an expression.
function filtersOf(p) {
    const plain = (value, what, maxLength = 255) => {
        const found = text(value)
        if (found.length > maxLength || /[\u0000-\u001f\u007f]/.test(found)) throw new Error(what + " is not a value to find by")
        return found || null
    }
    const day = (value, what) => {
        if (value == null || value === "") return null
        const found = ms(value)
        if (!Number.isFinite(found)) throw new Error(what + " is not a date")
        return found
    }
    const filters = {
        changedByUserId: plain(p.changedByUserId, "The user", 40),
        recordKey: plain(p.recordKey, "The record's key"),
        visitId: plain(p.correlation ?? p.visitId, "The session", 40),
        fromDate: day(p.fromDate, "From"),
        thruDate: day(p.thruDate, "Before"),
        // only what a signed-in user changed: a data load's own rows (no user) left out
        actorOnly: p.actorOnly === true || p.actorOnly === "true"
    }
    if (filters.fromDate != null && filters.thruDate != null && filters.fromDate >= filters.thruDate)
        throw new Error("The dates must end after they start")
    return filters
}
// The audit log narrowed by a request's filters (not by what is registered: each caller adds that).
function filtered(found, filters) {
    const factory = ec.entity.conditionFactory
    if (filters.changedByUserId) found.condition("changedByUserId", filters.changedByUserId)
    else if (filters.actorOnly) found.condition("changedByUserId", "not-equals", null)
    if (filters.visitId) found.condition("changedInVisitId", filters.visitId)
    if (filters.recordKey)
        found.condition(
            factory.makeCondition(
                [
                    factory.makeCondition("pkPrimaryValue", "equals", filters.recordKey),
                    factory.makeCondition("pkSecondaryValue", "equals", filters.recordKey)
                ],
                "or"
            )
        )
    if (filters.fromDate != null) found.condition("changedDate", "greater-equals", new Date(filters.fromDate))
    if (filters.thruDate != null) found.condition("changedDate", "less", new Date(filters.thruDate))
    return found
}
// The condition of a set of projections: one entity's registered fields, or another's. An entity with a registration of
// every other field is every field of it but those registered on their own and not among the projections given.
function registeredCondition(projections) {
    const factory = ec.entity.conditionFactory
    const byEntity = new Map()
    for (const projection of projections) {
        if (!byEntity.has(projection.changedEntityName)) byEntity.set(projection.changedEntityName, {fields: [], other: null})
        const entry = byEntity.get(projection.changedEntityName)
        if (projection.wildcard) entry.other = projection
        else entry.fields.push(projection.changedFieldName)
    }
    return factory.makeCondition(
        Array.from(byEntity.entries()).map(([entityName, entry]) => {
            const parts = [factory.makeCondition("changedEntityName", "equals", entityName)]
            const left = entry.other ? entry.other.otherFields.filter(name => !entry.fields.includes(name)) : []
            if (!entry.other) parts.push(factory.makeCondition("changedFieldName", "in", entry.fields))
            else if (left.length) parts.push(factory.makeCondition("changedFieldName", "not-in", left))
            return factory.makeCondition(parts)
        }),
        "or"
    )
}

// ===== sources =====
// The instance's own records: the registered fields of the audit log, newest first.
function instanceChanges(projections, filters, depth) {
    if (!projections.length) return {rows: [], count: 0}
    const base = () => filtered(find(LOG), filters).condition(registeredCondition(projections))
    return {
        rows: Array.from(base().orderBy("-changedDate,-auditHistorySeqId").limit(depth).list()).map(map),
        count: Number(base().count())
    }
}
// An owner's records: the ids its source answers within the caller's scope, read here and kept only when they are of a
// projection of that source. {rows, count}, or {failure}.
function sourceChanges(sourceService, projections, filters, depth) {
    ec.message.pushErrors()
    let out = null
    let failure = ""
    try {
        out =
            ec.service
                .sync()
                .name(CALL_SOURCE)
                .parameters(
                    Object.assign(
                        {
                            sourceService,
                            auditProjectionIdList: projections.map(projection => projection.auditProjectionId),
                            limit: depth
                        },
                        filters
                    )
                )
                .requireNewTransaction(true)
                .call() || {}
        if (ec.message.hasError()) failure = ec.message.getErrorsString()
    } catch (error) {
        failure = String(error?.message ?? error)
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
    if (failure) return {failure}
    const result = out.result || {}
    const ids = Array.from(result.auditIdList || [])
        .map(text)
        .filter(Boolean)
        .slice(0, depth)
    const allowed = keyed(projections)
    const found = ids.length
        ? Array.from(find(LOG).condition("auditHistorySeqId", "in", ids).list())
              .map(map)
              .filter(row => projectionOf(allowed, row))
        : []
    return {rows: found, count: Math.max(Number(result.changeCount) || 0, found.length)}
}
// call#ChangeSource: the owner's service of a registered source, with the registered projections of it the caller may read.
function callChangeSource(p) {
    requireView()
    const sourceService = text(p.sourceService)
    const wanted = new Set(Array.from(p.auditProjectionIdList || []).map(text))
    const projections = readable().filter(
        projection => projection.sourceService === sourceService && (!wanted.size || wanted.has(projection.auditProjectionId))
    )
    if (!sourceService || !projections.length) throw new Error("Unknown audit source " + sourceService)
    // a registration of every other field (*) does not cover a field registered on its own: the owner is told which, unless
    // that field's own projection is among these
    const given = keyed(projections)
    const parameters = {
        projectionList: projections.map(projection =>
            Object.assign(
                {
                    auditProjectionId: projection.auditProjectionId,
                    changedEntityName: projection.changedEntityName,
                    changedFieldName: projection.changedFieldName
                },
                projection.wildcard
                    ? {exceptFieldNames: projection.otherFields.filter(name => !given.has(keyOf(projection.changedEntityName, name)))}
                    : {}
            )
        ),
        limit: Math.min(Math.max(Number(p.limit) || 50, 1), maxDepth)
    }
    for (const name of ["changedByUserId", "recordKey", "visitId", "fromDate", "thruDate"])
        if (p[name] != null && p[name] !== "") parameters[name] = p[name]
    if (p.actorOnly === true) parameters.actorOnly = true
    return {result: ec.service.sync().name(sourceService).parameters(parameters).call() || {}}
}

// ===== a change as it is shown =====
function namesOf(changes, projections) {
    const userIds = new Set()
    const groupIds = new Set()
    for (const row of changes) {
        if (row.changedByUserId) userIds.add(text(row.changedByUserId))
        const projection = projectionOf(projections, row)
        for (const [kind, value] of [
            [projection.primaryKeyKind, row.pkPrimaryValue],
            [projection.secondaryKeyKind, row.pkSecondaryValue]
        ]) {
            if (kind === "user" && value) userIds.add(text(value))
            if (kind === "group" && value) groupIds.add(text(value))
        }
    }
    const users = new Map()
    if (userIds.size)
        for (const row of Array.from(
            find("moqui.security.UserAccount")
                .condition("userId", "in", Array.from(userIds))
                .selectField("userId")
                .selectField("username")
                .selectField("userFullName")
                .list()
        ))
            users.set(
                row.userId,
                lineText([row.username, row.userFullName && "(" + row.userFullName + ")"].filter(Boolean).join(" "), 120)
            )
    const groups = new Map()
    if (groupIds.size)
        for (const row of Array.from(
            find("moqui.security.UserGroup")
                .condition("userGroupId", "in", Array.from(groupIds))
                .selectField("userGroupId")
                .selectField("description")
                .list()
        ))
            groups.set(row.userGroupId, lineText(row.description, 120))
    return {users, groups}
}
function shown(projection, row, found) {
    const keyText = (kind, value) =>
        value == null || value === "" || kind === "hidden"
            ? ""
            : kind === "user"
              ? found.users.get(text(value)) || text(value)
              : kind === "group"
                ? found.groups.get(text(value)) || text(value)
                : lineText(value, 255)
    const primaryKey = keyText(projection.primaryKeyKind, row.pkPrimaryValue)
    const secondaryKey = keyText(projection.secondaryKeyKind, row.pkSecondaryValue)
    const before = row.oldValueText == null || row.oldValueText === "" ? null : String(row.oldValueText)
    const after = row.newValueText == null || row.newValueText === "" ? null : String(row.newValueText)
    // the audit log keeps a date as its milliseconds: shown as a time in the user's zone
    const dated = /Date$/.test(text(row.changedFieldName))
    const value = found => {
        if (found == null) return null
        if (dated && /^-?\d{1,15}$/.test(found))
            return Number(found) === 0 ? "the start" : ec.l10n.format(new Date(Number(found)), "yyyy-MM-dd HH:mm")
        const line = lineText(found, valueLength + 1)
        return line.length > valueLength ? line.slice(0, valueLength) + "…" : line
    }
    const marker = before != null && after != null ? "changed" : after != null ? "set" : before != null ? "cleared" : "recorded"
    const changedDate = ms(row.changedDate)
    return {
        auditId: text(row.auditHistorySeqId),
        changedDate,
        changedDateText: changedDate == null ? "" : ec.l10n.format(new Date(changedDate), "yyyy-MM-dd HH:mm:ss"),
        changedByUserId: text(row.changedByUserId) || null,
        actorName: row.changedByUserId ? found.users.get(text(row.changedByUserId)) || text(row.changedByUserId) : "",
        auditProjectionId: projection.auditProjectionId,
        category: projection.category,
        categoryLabel: categories[projection.category],
        description: projection.description + (projection.wildcard ? ": " + lineText(row.changedFieldName, 60) : ""),
        changedEntityName: projection.changedEntityName,
        changedFieldName: lineText(row.changedFieldName, 60),
        primaryKey,
        primaryKeyTitle: projection.primaryKeyTitle,
        secondaryKey,
        secondaryKeyTitle: projection.secondaryKeyTitle,
        recordText: [
            primaryKey && (projection.primaryKeyTitle ? projection.primaryKeyTitle + ": " : "") + primaryKey,
            secondaryKey && (projection.secondaryKeyTitle ? projection.secondaryKeyTitle + ": " : "") + secondaryKey
        ]
            .filter(Boolean)
            .join(" · "),
        // the values only where the registration shows them; a service's own marker of a secret's change as it is
        masked: !projection.show,
        marker,
        before: projection.show ? value(before) : serviceMarker(before) ? text(before) : null,
        after: projection.show ? value(after) : serviceMarker(after) ? text(after) : null,
        reason: lineText(row.changeReason, 255),
        correlation: text(row.changedInVisitId) || null,
        linkKey: projection.linkKey,
        linkId: projection.linkKey && projection.primaryKeyKind !== "hidden" ? text(row.pkPrimaryValue) : ""
    }
}

// ===== find#Changes =====
function findChanges(p) {
    requireView()
    const filters = filtersOf(p)
    const all = readable()
    const options = {
        projectionList: all.map(projection => ({
            auditProjectionId: projection.auditProjectionId,
            category: projection.category,
            description: categories[projection.category] + ": " + projection.description
        })),
        categoryList: Object.keys(categories)
            .filter(category => all.some(projection => projection.category === category))
            .map(category => ({category, description: categories[category]})),
        maxDepth,
        exportRows
    }
    // a request names a registered projection or a category, never an entity or a field
    const projectionId = text(p.auditProjectionId)
    const category = text(p.category)
    if (projectionId && !all.some(projection => projection.auditProjectionId === projectionId))
        throw new Error("Unknown audit projection " + lineText(projectionId, 40))
    if (category && !Object.hasOwn(categories, category)) throw new Error("Unknown audit category " + lineText(category, 40))
    const chosen = all.filter(
        projection =>
            (!projectionId || projection.auditProjectionId === projectionId) && (!category || projection.category === category)
    )
    const forExport = p.forExport === true
    const pageSize = forExport ? exportRows : Math.min(Math.max(Number(p.pageSize) || 50, 1), 100)
    const pageIndex = forExport ? 0 : Math.max(Number(p.pageIndex) || 0, 0)
    const paging = (count, shownCount) => ({
        changeListCount: count,
        changeListPageIndex: pageIndex,
        changeListPageSize: pageSize,
        changeListPageMaxIndex: Math.max(Math.ceil(count / pageSize) - 1, 0),
        changeListPageRangeLow: shownCount ? pageIndex * pageSize + 1 : 0,
        changeListPageRangeHigh: pageIndex * pageSize + shownCount
    })
    const depth = (pageIndex + 1) * pageSize
    if (depth > maxDepth)
        return Object.assign(options, paging(0, 0), {
            changeList: [],
            truncated: false,
            sourceProblems: [],
            notice:
                "Only the newest " + maxDepth + " changes of a find are read: narrow the dates or the filters to read older ones"
        })
    const bySource = new Map()
    for (const projection of chosen) {
        if (!bySource.has(projection.sourceService)) bySource.set(projection.sourceService, [])
        bySource.get(projection.sourceService).push(projection)
    }
    let merged = []
    let count = 0
    const sourceProblems = []
    for (const [sourceService, projections] of bySource) {
        const found = sourceService
            ? sourceChanges(sourceService, projections, filters, depth)
            : instanceChanges(projections, filters, depth)
        if (found.failure) {
            // one owner's source that fails is named; the others' changes are still answered
            ec.logger.error("Audit source " + sourceService + " failed: " + lineText(found.failure, 500))
            sourceProblems.push(Array.from(new Set(projections.map(projection => categories[projection.category]))).join(", "))
            continue
        }
        merged = merged.concat(found.rows)
        count += found.count
    }
    merged.sort(
        (a, b) =>
            (ms(b.changedDate) ?? 0) - (ms(a.changedDate) ?? 0) ||
            text(b.auditHistorySeqId).localeCompare(text(a.auditHistorySeqId), undefined, {numeric: true})
    )
    const byKey = keyed(chosen)
    // only a row of a projection asked for is ever shown, whatever a find or a source answered
    const page = merged.filter(row => projectionOf(byKey, row)).slice(pageIndex * pageSize, (pageIndex + 1) * pageSize)
    const found = namesOf(page, byKey)
    return Object.assign(options, paging(count, page.length), {
        changeList: page.map(row => shown(projectionOf(byKey, row), row, found)),
        truncated: forExport && count > page.length,
        sourceProblems: Array.from(new Set(sourceProblems)),
        notice: null
    })
}

const operations = {findChanges, callChangeSource}
;({
    categories,
    maxDepth,
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown audit operation " + op)
        return operations[op](parameters)
    }
})
