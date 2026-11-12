// The reads of org.moqui.report.ReportServices that the report screens need beside the engine's own (docs/tools/Monitoring.md,
// "Business datasets", "Report delivery" and "Mounting the report screens"; plans/Admin-Screens-Down.md, Track I): what a
// mountable report screen shows that the engine's operations do not answer, so that no screen reads an entity itself. Each
// operation checks what its caller may read: it asks the engine as the caller (the dataset's own artifact,
// org.moqui.report.dataset.<id>) or the caller's own grant (the group delivery artifact, a group's share artifact), or
// answers the caller's own rows,
// and reads with authorization off only what that opens. No dependency on Mantle or an app.
//
//     get#BusinessDataset         one business dataset the caller reads or stewards, and for its steward its document's
//                                 fields and conditions and the definitions that use it
//     get#BusinessDatasetOptions  what a dataset form may offer: the registered scope policies, each with its scope
//                                 parameter, the operators and functions a steward may use, and for a caller who may
//                                 create a dataset the documents of registered feeds
//     find#ReportRecipients       who a caller that may deliver to others may propose as a recipient of a report or an
//                                 alert, and which of the users and groups named may be one
//     find#ShareGroups            the groups the caller may share a definition with, by name: their own, and of
//                                 those named the ones whose share artifact their own grant holds
//     find#ScreenSchedules        the caller's own screen schedules (a ScreenScheduled of a screen path, no report
//                                 definition), which store#ScreenSchedule (ReportWrites.js) changes
const R = "org.moqui.report.ReportServices."
const POLICY_TYPE = "ReportScopePolicy"
// The dataset-save artifact (crates/moqui-report/src/definitions.rs), which create#BusinessDataset asks of its caller.
const DATASET_SAVE = "org.moqui.report.dataset"
// The telemetry indexes (crates/moqui-core/src/search/telemetry.rs): never a business source.
const TELEMETRY = ["otel_spans", "otel_logs", "otel_metrics", "otel_errors", "monitor_annotations", "browser_replays"]
// The condition operators and field functions a steward may use (crates/moqui-report/src/business.rs OPERATORS, FUNCTIONS).
const OPERATORS = [
    "equals",
    "not-equals",
    "less",
    "greater",
    "less-equals",
    "greater-equals",
    "in",
    "not-in",
    "between",
    "not-between",
    "like",
    "not-like",
    "is-null",
    "is-not-null"
]
const FUNCTIONS = ["min", "max", "sum", "avg", "count", "count-distinct", "upper", "lower"]
// The artifact a user needs to deliver to anyone but themselves (crates/moqui-report/src/delivery.rs GROUP_ARTIFACT, asked
// with the create action), and the permission of an account that is not an internal user (moqui_core::user EXTERNAL_USER).
const GROUP_DELIVERY = "org.moqui.report.delivery.group"
const EXTERNAL_USER = "EXTERNAL_USER"
// The artifact of sharing a definition with a group the sharer is not in, <this>.<userGroupId>, asked with the update
// action (crates/moqui-report/src/definitions.rs SHARE_ARTIFACT, may_share_with).
const SHARE_ARTIFACT = "org.moqui.report.share"
const ACCOUNT = "moqui.security.UserAccount"
const SCHEDULE = "moqui.screen.ScreenScheduled"
// a page of recipient choices, the most ids one call answers for, and the most screen schedules listed
const RECIPIENT_PAGE = 20
const RECIPIENT_PAGE_MAX = 100
const NAMED_MAX = 500
const SCREEN_SCHEDULE_MAX = 200

const text = value => (value == null ? "" : String(value))
const plainName = value => /^[A-Za-z0-9_]{1,64}$/.test(text(value))
function signedIn() {
    if (!ec.user.userId) throw new Error("The report reads need a signed-in user")
}
// An engine operation as the caller: its own checks decide, and its refusal is the caller's.
function asCaller(name, parameters) {
    const out = ec.service.sync().name(name).parameters(parameters).call() || {}
    if (ec.message.hasError()) throw new Error(ec.message.getErrorsString())
    return out
}
// Rows the engine's answer has opened: read with authorization off, for nothing but the caller's own dataset or the
// registrations every dataset form shows.
function rows(entityName, conditions) {
    return Array.from(ec.entity.find(entityName).condition(conditions).disableAuthz().list())
}

// ===== get#BusinessDataset =====
// A field's name in the document: its alias, or the last part of its path (business.rs field_alias).
function fieldName(field) {
    const path = text(field.fieldPath)
    return text(field.fieldNameAlias) || (path.includes(":") ? path.split(":").pop() : path)
}
function documentFields(dataDocumentId) {
    return rows("moqui.entity.document.DataDocumentField", {dataDocumentId})
        .map(field => ({
            fieldSeqId: field.fieldSeqId,
            fieldPath: field.fieldPath,
            fieldNameAlias: text(field.fieldNameAlias),
            fieldName: fieldName(field),
            functionName: text(field.functionName),
            sequenceNum: field.sequenceNum ?? null,
            // a path that is not relationship and field names is an expression, which only its component writes
            expression: !/^[A-Za-z0-9_:]+$/.test(text(field.fieldPath))
        }))
        .sort((a, b) => (a.sequenceNum ?? 999) - (b.sequenceNum ?? 999) || text(a.fieldSeqId).localeCompare(text(b.fieldSeqId)))
}
function documentConditions(dataDocumentId) {
    return rows("moqui.entity.document.DataDocumentCondition", {dataDocumentId})
        .map(condition => ({
            conditionSeqId: condition.conditionSeqId,
            fieldNameAlias: text(condition.fieldNameAlias),
            operator: text(condition.operator) || "equals",
            fieldValue: text(condition.fieldValue),
            toFieldNameAlias: text(condition.toFieldNameAlias),
            postQuery: condition.postQuery === "Y"
        }))
        .sort((a, b) => text(a.conditionSeqId).localeCompare(text(b.conditionSeqId)))
}
// The definitions over the dataset that the caller reads. One that cannot be listed is logged and the dataset is answered
// without it (usesAvailable false), so the screen says so rather than "none".
function definitionUses(datasetId) {
    ec.message.pushErrors()
    try {
        const uses = Array.from(asCaller(R + "find#DefinitionUses", {definitionId: datasetId}).usedIn || []).map(use => ({
            id: use.id,
            title: use.title || use.id,
            type: use.type,
            readOnly: use.readOnly === true
        }))
        return {uses, usesAvailable: true}
    } catch (error) {
        ec.logger.error("Business dataset " + datasetId + ": its uses were not read: " + (error?.message ?? error))
        ec.message.clearErrors()
        return {uses: [], usesAvailable: false}
    } finally {
        ec.message.popErrors()
    }
}
// get#BusinessDataset: the dataset as find#BusinessDatasets answers it to this caller (so one they neither read nor
// steward is refused as one that is not there), and for its steward (stewarded: the dataset's update artifact, asked by
// the engine) the fields and conditions of its document and the definitions that use it. A reader gets the dataset alone.
function getBusinessDataset(p) {
    signedIn()
    const datasetId = text(p.datasetId)
    const dataset = datasetId ? Array.from(asCaller(R + "find#BusinessDatasets", {datasetId}).datasets || [])[0] : null
    if (!dataset) throw new Error("Dataset " + datasetId + " is not available")
    if (dataset.stewarded !== true || !dataset.dataDocumentId)
        return {dataset, fields: [], conditions: [], uses: [], usesAvailable: false}
    return Object.assign(
        {dataset, fields: documentFields(dataset.dataDocumentId), conditions: documentConditions(dataset.dataDocumentId)},
        definitionUses(datasetId)
    )
}

// ===== get#BusinessDatasetOptions =====
// The registered scope policies (business.rs policies): a row with no service is not one. scopeParameter is the
// registration's own (optionValue): the name of the rowFilterParameters entry in which a dataset names the field that
// carries a row's scope, null for a policy that registers none.
function scopePolicies() {
    return rows("moqui.basic.Enumeration", {enumTypeId: POLICY_TYPE})
        .filter(row => text(row.enumCode))
        .map(row => ({
            scopePolicyId: row.enumId,
            description: text(row.description) || row.enumId,
            scopeParameter: plainName(row.optionValue) ? text(row.optionValue) : null
        }))
        .sort((a, b) => a.description.localeCompare(b.description))
}
// The documents a business dataset may be made over: those a registered feed indexes, never a telemetry index.
function fedDocuments() {
    const fed = Array.from(new Set(rows("moqui.entity.feed.DataFeedDocument", {}).map(row => row.dataDocumentId)))
    if (!fed.length) return []
    return Array.from(
        ec.entity
            .find("moqui.entity.document.DataDocument")
            .condition("dataDocumentId", "in", fed)
            .orderBy("documentName")
            .disableAuthz()
            .list()
    )
        .filter(row => !TELEMETRY.includes(text(row.dataDocumentId)) && !TELEMETRY.includes(text(row.indexName)))
        .map(row => ({
            dataDocumentId: row.dataDocumentId,
            documentName: text(row.documentName) || row.dataDocumentId,
            indexName: text(row.indexName)
        }))
}
// get#BusinessDatasetOptions: the policies, operators and functions for any signed-in caller (registrations, no data);
// the documents only for one who may create a dataset (canCreate: the dataset-save artifact by the caller's own grant,
// as create#BusinessDataset asks it). Nothing of a request is trusted: the answer has no input.
function getBusinessDatasetOptions() {
    signedIn()
    const canCreate = ec.artifactExecution.isPermittedOwn(DATASET_SAVE, "AT_SERVICE", "update") === true
    return {
        canCreate,
        documents: canCreate ? fedDocuments() : [],
        policies: scopePolicies(),
        operators: OPERATORS.slice(),
        functions: FUNCTIONS.slice()
    }
}

// ===== find#ReportRecipients =====
const idList = value =>
    Array.from(
        new Set(
            (value == null || value === "" ? [] : Array.from(typeof value === "string" ? value.split(",") : value))
                .map(id => text(id).trim())
                .filter(Boolean)
        )
    )
// The users who hold a permission now: the members in date of the groups that hold it in date.
function holderIds(userPermissionId) {
    const groupIds = Array.from(
        ec.entity
            .find("moqui.security.UserGroupPermission")
            .condition("userPermissionId", userPermissionId)
            .conditionDate("fromDate", "thruDate", null)
            .selectField("userGroupId")
            .disableAuthz()
            .list()
    ).map(row => row.userGroupId)
    if (!groupIds.length) return []
    return Array.from(
        new Set(
            Array.from(
                ec.entity
                    .find("moqui.security.UserGroupMember")
                    .condition("userGroupId", "in", groupIds)
                    .conditionDate("fromDate", "thruDate", null)
                    .selectField("userId")
                    .disableAuthz()
                    .list()
            ).map(row => row.userId)
        )
    )
}
// The accounts that may be proposed: active (not disabled, not past their end date), internal, and with a permission
// named its holders only; narrowed further by the find given. The engine's own rule for a recipient is the same but for
// the permission (delivery.rs judge: inactive, external), so nobody it would refuse on sight is offered.
function proposable(permissionId, narrow) {
    const cf = ec.entity.conditionFactory
    let find = ec.entity
        .find(ACCOUNT)
        .condition(cf.makeCondition("disabled", "not-equals", "Y", true))
        .condition(cf.makeCondition("terminateDate", "greater", ec.user.nowTimestamp, true))
        .selectField("userId,username,userFullName")
        .disableAuthz()
    const external = holderIds(EXTERNAL_USER)
    if (external.length) find = find.condition("userId", "not-in", external)
    if (permissionId) {
        const holders = holderIds(permissionId)
        // nobody holds it: nobody is proposed
        find = find.condition("userId", "in", holders.length ? holders : ["_NA_"])
    }
    return narrow(find, cf)
}
const person = row => ({userId: row.userId, username: text(row.username), name: text(row.userFullName) || text(row.username)})
// find#ReportRecipients: for a caller who may deliver to others (canDeliver: the group delivery artifact by the caller's
// own grant, as the engine asks it), a page of the people who may be proposed (users: found by term in the name or the
// username, ignoring case, in the query), the caller's own groups (groups: a delivery goes to a group its owner belongs
// to, never ALL_USERS), and of the users and groups named which may be recipients (selectedUsers, with their names) and
// which may not (refusedUserIds, refusedGroupIds). With permissionId only its holders are proposed or accepted. The caller
// is always accepted (a delivery to oneself). A caller who may not deliver to others gets no account and no group, and
// every other user and every group named refused. Being accepted here grants nothing: the engine checks each recipient's
// scope when a schedule or a rule is saved and again before each message.
function findReportRecipients(p) {
    signedIn()
    const viewer = text(ec.user.userId)
    const userIds = idList(p.userIds).slice(0, NAMED_MAX)
    const groupIds = idList(p.groupIds).slice(0, NAMED_MAX)
    const canDeliver = ec.artifactExecution.isPermittedOwn(GROUP_DELIVERY, "AT_SERVICE", "create") === true
    const own = userIds.includes(viewer) ? rows(ACCOUNT, {userId: viewer}).slice(0, 1).map(person) : []
    if (!canDeliver)
        return {
            canDeliver,
            users: [],
            groups: [],
            selectedUsers: own,
            refusedUserIds: userIds.filter(userId => userId !== viewer),
            refusedGroupIds: groupIds
        }
    const permissionId = text(p.permissionId).trim()
    const term = text(p.term).trim()
    const size = Math.min(RECIPIENT_PAGE_MAX, Math.max(1, Number(p.pageSize) || RECIPIENT_PAGE))
    const users = Array.from(
        proposable(permissionId, (find, cf) =>
            term
                ? find.condition(
                      cf.makeCondition(
                          [
                              cf.makeCondition("userFullName", "like", "%" + term + "%").ignoreCase(),
                              cf.makeCondition("username", "like", "%" + term + "%").ignoreCase()
                          ],
                          "or"
                      )
                  )
                : find
        )
            .orderBy("userFullName,username")
            .limit(size)
            .list()
    ).map(person)
    const others = userIds.filter(userId => userId !== viewer)
    const accepted = others.length
        ? Array.from(proposable(permissionId, find => find.condition("userId", "in", others)).list()).map(person)
        : []
    const selectedUsers = own.concat(accepted)
    const ownGroupIds = Array.from(ec.user.getUserGroupIdSet() || []).filter(groupId => groupId !== "ALL_USERS")
    const groups = ownGroupIds.length
        ? Array.from(ec.entity.find("moqui.security.UserGroup").condition("userGroupId", "in", ownGroupIds).disableAuthz().list())
              .map(row => ({userGroupId: row.userGroupId, description: text(row.description) || row.userGroupId}))
              .sort((a, b) => a.description.localeCompare(b.description))
        : []
    return {
        canDeliver,
        users,
        groups,
        selectedUsers,
        refusedUserIds: userIds.filter(userId => !selectedUsers.some(user => user.userId === userId)),
        refusedGroupIds: groupIds.filter(groupId => !ownGroupIds.includes(groupId))
    }
}

// ===== find#ShareGroups =====
// find#ShareGroups: the groups the caller may share a definition with, by name, as save#Definition accepts them: the
// caller's own groups (own true; never the implicit ALL_USERS), and of the groups named those the caller may share with
// though no member of them (the group's share artifact by the caller's own grant: a group no person is in, such as an
// app's wall displays). A group named that is neither, or does not exist, is in refusedGroupIds. Nothing is granted
// here: the save asks the same of the group it is given.
function findShareGroups(p) {
    signedIn()
    const named = idList(p.groupIds).slice(0, NAMED_MAX)
    const ownIds = Array.from(ec.user.getUserGroupIdSet() || []).filter(groupId => groupId !== "ALL_USERS")
    const granted = named.filter(
        groupId =>
            !ownIds.includes(groupId) &&
            /^[A-Za-z0-9_.:-]+$/.test(groupId) &&
            ec.artifactExecution.isPermittedOwn(SHARE_ARTIFACT + "." + groupId, "AT_SERVICE", "update") === true
    )
    const ids = ownIds.concat(granted)
    const groups = ids.length
        ? Array.from(ec.entity.find("moqui.security.UserGroup").condition("userGroupId", "in", ids).disableAuthz().list())
              .map(row => ({
                  userGroupId: row.userGroupId,
                  description: text(row.description) || row.userGroupId,
                  own: ownIds.includes(row.userGroupId)
              }))
              .sort((a, b) => a.description.localeCompare(b.description))
        : []
    return {groups, refusedGroupIds: named.filter(groupId => !groups.some(group => group.userGroupId === groupId))}
}

// ===== find#ScreenSchedules =====
// a timestamp is text of epoch milliseconds in a script (docs/dev/JS-Globals.md)
const millis = value =>
    value == null || value === "" ? null : /^-?\d+$/.test(String(value)) ? Number(value) : Date.parse(String(value))
// find#ScreenSchedules: the screen schedules the caller set up for themselves (userId the caller, a screen path and no
// report definition; a report schedule is list#Schedules'), one of them with screenScheduledId, each with its saved
// find's description and whether the group it names is sent to (only through a registered ReportLegacyAdapter of its
// screen path, org.moqui.impl.ScreenServices). Another user's schedule is not answered, whatever id is asked.
function findScreenSchedules(p) {
    signedIn()
    let find = ec.entity
        .find(SCHEDULE)
        .condition("userId", ec.user.userId)
        .condition("reportDefinitionId", "is-null", null)
        .orderBy("screenPath,screenScheduledId")
        .limit(SCREEN_SCHEDULE_MAX)
        .disableAuthz()
    if (text(p.screenScheduledId)) find = find.condition("screenScheduledId", text(p.screenScheduledId))
    const schedules = Array.from(find.list())
    const findIds = Array.from(new Set(schedules.map(row => row.formListFindId).filter(Boolean)))
    const descriptions = {}
    if (findIds.length)
        for (const saved of ec.entity
            .find("moqui.screen.form.FormListFind")
            .condition("formListFindId", "in", findIds)
            .disableAuthz()
            .list())
            descriptions[saved.formListFindId] = text(saved.description)
    const grouped = Array.from(new Set(schedules.filter(row => row.userGroupId).map(row => text(row.screenPath))))
    const admitted = grouped.length
        ? Array.from(
              ec.entity
                  .find("moqui.basic.Enumeration")
                  .condition("enumTypeId", "ReportLegacyAdapter")
                  .condition("enumCode", "in", grouped)
                  .disableAuthz()
                  .list()
          ).map(row => text(row.enumCode))
        : []
    return {
        schedules: schedules.map(row => ({
            screenScheduledId: row.screenScheduledId,
            screenPath: text(row.screenPath),
            formListFindId: row.formListFindId || null,
            description: descriptions[row.formListFindId] || "",
            cronExpression: text(row.cronExpression),
            renderMode: text(row.renderMode) || "csv",
            noResultsAbort: row.noResultsAbort === "Y",
            paused: row.paused === "Y",
            fromDate: millis(row.fromDate),
            thruDate: millis(row.thruDate),
            emailSubject: text(row.emailSubject),
            userGroupId: row.userGroupId || null,
            groupAdmitted: !!row.userGroupId && admitted.includes(text(row.screenPath))
        }))
    }
}

const operations = {
    getBusinessDataset,
    getBusinessDatasetOptions,
    findReportRecipients,
    findShareGroups,
    findScreenSchedules
}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown report read " + op)
        return operations[op](parameters)
    }
})
