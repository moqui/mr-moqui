// The reads of org.moqui.report.ReportServices that the report screens need beside the engine's own (docs/tools/Monitoring.md,
// "Business datasets" and "Mounting the report screens"; plans/Admin-Screens-Down.md, Track I): what a mountable report
// screen shows that the engine's operations do not answer, so that no screen reads an entity itself. Each operation checks
// what its caller may read: it asks the engine as the caller (the dataset's own artifact, org.moqui.report.dataset.<id>) and
// reads with authorization off only what that answer opens. No dependency on Mantle or an app.
//
//     get#BusinessDataset         one business dataset the caller reads or stewards, and for its steward its document's
//                                 fields and conditions and the definitions that use it
//     get#BusinessDatasetOptions  what a dataset form may offer: the registered scope policies, each with its scope
//                                 parameter, the operators and functions a steward may use, and for a caller who may
//                                 create a dataset the documents of registered feeds
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

const operations = {getBusinessDataset, getBusinessDatasetOptions}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown report read " + op)
        return operations[op](parameters)
    }
})
