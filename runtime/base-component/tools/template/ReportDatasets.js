// What the dataset screens of Dashboards show (screen/Dashboards/Datasets.xml, DatasetDetail.xml, DatasetFields.xml and
// DatasetView.xml; docs/tools/Monitoring.md, "Business datasets"; plans/Admin-Screens-Down.md, Track I): the business
// datasets with their state, a dataset for its steward, its rows as the viewer reads them and their export, and Tools'
// other datasets. Presentation only: every read and write is an operation of org.moqui.report.ReportServices called as
// the viewer, so the engine checks the dataset's own artifact (view to read, update to steward) and its row filter scopes
// every row; no entity is read here. Another app shows the screens under its own root (BOSS Insights › Report builder):
// the mount's policy (template/ReportMount.js) gives what a dataset created there is named with, the words of the scope
// field and the default range, as data; the engine decides what is allowed.
const mountLib = ec.resource.script("component://tools/template/ReportMount.js")
// an engine operation as the viewer, {out, failure}: the report screens' one engine call (ReportDelivery.js)
const {ask} = ec.resource.script("component://tools/template/ReportDelivery.js")
const minute = 60 * 1000
const statusNames = {admitted: "Admitted", review: "Needs review", retired: "Retired", unavailable: "Unavailable"}
const statusColors = {admitted: "positive", review: "warning", retired: "grey-8", unavailable: "negative"}
const checkNames = {feed: "Registered feed", index: "Index built", scope: "Row filter bound", document: "Source document"}
// Tools' own datasets, which are not business datasets
const telemetry = ["otel_spans", "otel_logs", "otel_metrics", "otel_errors", "monitor_annotations"]
// the most rows one export holds, its default, and a page of the view
const exportCap = 10000
const exportDefault = 1000
const viewPageSize = 25

const text = value => (value == null ? "" : String(value))
// The same for a transition, whose refusal is its error.
function must(name, parameters) {
    const asked = ask(name, parameters)
    if (asked.failure) throw new Error(asked.failure)
    return asked.out
}

const timeText = ms => (ms == null || ms === "" ? "" : ec.l10n.format(new Date(Number(ms)), "yyyy-MM-dd HH:mm"))
function lagText(ms) {
    if (ms == null || ms === "") return ""
    const minutes = Math.max(0, Math.round((Date.now() - Number(ms)) / minute))
    if (minutes < 120) return minutes + (minutes === 1 ? " minute" : " minutes")
    if (minutes < 48 * 60) return Math.round(minutes / 60) + " hours"
    return Math.round(minutes / (24 * 60)) + " days"
}

// ===== the scope field =====
// A scope policy takes the field that carries a row's scope in one entry of rowFilterParameters, whose name is the
// policy's own, from its registration (scopeParameter, in the engine's answers); a policy that registers none is asked
// for no field. The words of the form's field are the mount's (datasetScopeLabel, datasetScopeHelp), Tools' own with no
// mount.
function scopeWords(policy) {
    return {label: policy.datasetScopeLabel, help: policy.datasetScopeHelp}
}
// The rowFilterParameters a form's scope field stands for under the policy chosen, or null when the field is empty. kept
// is what the dataset already holds under the same policy, which a form with one field does not drop.
function scopeParameters(scopePolicyId, scopeField, kept) {
    const field = text(scopeField).trim()
    if (!field) return null
    const chosen = Array.from(must("get#BusinessDatasetOptions", {}).policies || []).find(
        row => row.scopePolicyId === scopePolicyId
    )
    const parameter = text(chosen?.scopeParameter)
    if (!parameter)
        throw new Error(
            "The scope policy " + (chosen?.description || text(scopePolicyId)) + " takes no scope field: leave it empty"
        )
    return Object.assign({}, kept || {}, {[parameter]: field})
}

// ===== a dataset as the screens show it =====
function present(row) {
    const checks = Array.from(row.checks || []).map(check => ({
        check: check.check,
        name: checkNames[check.check] || check.check,
        passed: check.passed === true,
        passedText: check.passed === true ? "Passed" : "Not passed",
        message: check.message || ""
    }))
    const scopeParameter = text(row.scopeParameter)
    const scopeField = scopeParameter ? text(row.rowFilterParameters?.[scopeParameter]) : ""
    return {
        datasetId: row.datasetId,
        title: row.title || row.datasetId,
        dataDocumentId: row.dataDocumentId || "",
        documentName: row.documentName || "",
        indexName: row.indexName || "",
        dataFeedId: row.dataFeedId || "",
        timeField: row.timeField || "",
        scopePolicyId: row.scopePolicyId || "",
        scopeText: row.scopePolicy ? row.scopePolicy + (scopeField ? " on " + scopeField : "") : "Not bound",
        scopeParameter,
        scopeField,
        component: row.component || "",
        readOnly: row.readOnly === true,
        status: row.status,
        statusName: statusNames[row.status] || row.status,
        statusColor: statusColors[row.status] || "grey-8",
        admissible: row.admissible === true,
        checks,
        failedText: checks
            .filter(check => !check.passed)
            .map(check => check.message)
            .join("; "),
        admittedText: timeText(row.admittedDate),
        lastIndexedText: timeText(row.lastIndexedDate),
        lastIndexedCount: row.lastIndexedCount ?? null,
        lagText: lagText(row.lastIndexedDate),
        readable: row.readable === true,
        stewarded: row.stewarded === true,
        version: row.version ?? 0
    }
}
const policyChoices = options =>
    Array.from(options?.policies || []).map(row => ({key: row.scopePolicyId, text: row.description || row.scopePolicyId}))
const named = values => Array.from(values || []).map(value => ({key: value, text: text(value).replace(/-/g, " ")}))

// ===== Datasets: the list =====
// The business datasets the viewer reads, and a steward's waiting for review, narrowed by state and by text in the
// title, the id or the source, ignoring case; what a new dataset may be made over, for one who may create one; and,
// with no mount, Tools' other datasets (the telemetry indexes, the DataDocument indexes and the saved datasets that are
// not business ones), with the fields of the one chosen.
function list(p) {
    const mount = mountLib.state()
    const policy = mount.policy
    const found = ask("find#BusinessDatasets", {})
    const all = Array.from(found.out?.datasets || []).map(row => present(row))
    const wanted = text(p.text).trim().toLowerCase()
    const datasets = all
        .filter(row => !p.status || row.status === p.status)
        .filter(
            row => !wanted || [row.title, row.datasetId, row.documentName].some(value => value.toLowerCase().includes(wanted))
        )
        .sort((a, b) => a.title.localeCompare(b.title))
    const options = ask("get#BusinessDatasetOptions", {}).out || {}
    return {
        mounted: mount.mounted,
        exploreUrl: mount.links.explore || "",
        datasets,
        failure: found.failure,
        filtered: !!(p.status || wanted),
        canCreate: options.canCreate === true,
        documentChoices: Array.from(options.documents || []).map(row => ({
            key: row.dataDocumentId,
            text: row.documentName + (row.indexName ? " (" + row.indexName + ")" : "")
        })),
        policyChoices: policyChoices(options),
        scope: scopeWords(policy),
        nameHelp: policy.datasetIdPrefix
            ? "Letters and digits; the dataset's id is " + policy.datasetIdPrefix + " and this"
            : "Letters and digits: the dataset's id",
        other: mount.mounted ? {datasets: [], fields: [], notice: "", timeField: ""} : others(p.otherDatasetId, all)
    }
}
function others(datasetId, business) {
    const businessIds = business.map(row => row.datasetId)
    const datasets = Array.from(ask("list#Datasets", {}).out?.datasets || [])
        .filter(dataset => !businessIds.includes(dataset.id))
        .map(dataset => {
            const saved = !telemetry.includes(dataset.id) && !text(dataset.id).startsWith("index:")
            return Object.assign({}, dataset, {
                title: dataset.title || dataset.id,
                kind: telemetry.includes(dataset.id) ? "Telemetry" : saved ? "Saved dataset" : "Document index",
                saved
            })
        })
    let fields = []
    let notice = ""
    let timeField = ""
    if (datasetId) {
        const got = ask("get#Dataset", {datasetId})
        if (got.failure) {
            ec.logger.error("Dataset " + datasetId + " fields unavailable: " + got.failure)
            notice = got.failure
        } else {
            fields = Array.from(got.out.dataset?.fields || []).map(field => ({
                name: field.name,
                type: field.type,
                kind: field.kind || "",
                searchable: field.searchable === false ? "N" : "Y",
                aggregatable: field.aggregatable ? "Y" : "N"
            }))
            timeField = got.out.dataset?.timeField || ""
        }
    }
    return {datasets, fields, notice, timeField}
}

// ===== DatasetDetail and DatasetFields: one dataset for its steward =====
// The dataset with its admission checks, the definitions that use it, its document's fields and conditions, and what
// its forms offer. A dataset the viewer does not steward is not shown here (found false, with why).
function detail(datasetId) {
    const mount = mountLib.state()
    const policy = mount.policy
    const none = {
        found: false,
        mounted: mount.mounted,
        dataset: {},
        checks: [],
        uses: [],
        usesAvailable: true,
        fields: [],
        conditions: [],
        fieldNames: [],
        policyChoices: [],
        operatorOptions: [],
        functionOptions: [],
        scope: scopeWords(policy),
        exploreUrl: ""
    }
    const got = ask("get#BusinessDataset", {datasetId: datasetId || "_NA_"})
    if (got.failure || !got.out?.dataset) return Object.assign(none, {failure: got.failure})
    const dataset = present(got.out.dataset)
    if (!dataset.stewarded) return Object.assign(none, {failure: "Dataset " + dataset.title + " is not one you steward"})
    const options = ask("get#BusinessDatasetOptions", {}).out || {}
    const stamped = row => Object.assign({datasetId: dataset.datasetId, expectedVersion: dataset.version}, row)
    const fields = Array.from(got.out.fields || [])
    return Object.assign(none, {
        found: true,
        failure: null,
        dataset,
        checks: dataset.checks,
        uses: Array.from(got.out.uses || []).map(use => ({
            id: use.id,
            title: use.title || use.id,
            type: use.type,
            shipped: use.readOnly === true
        })),
        usesAvailable: got.out.usesAvailable !== false,
        fields: fields.map(stamped),
        conditions: Array.from(got.out.conditions || []).map(stamped),
        fieldNames: fields.map(field => ({key: field.fieldName, text: field.fieldName})),
        policyChoices: policyChoices(options),
        operatorOptions: named(options.operators),
        functionOptions: named(options.functions),
        exploreUrl: dataset.readable ? mount.links.explore || "" : ""
    })
}

// ===== stewardship: what a form's values stand for =====
function slug(value) {
    return text(value)
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-+|-+$/g, "")
        .slice(0, 50)
}
// CreateDataset's values as create#BusinessDataset's request: the id is the mount's prefix and the short name, which
// the engine takes only where the creator's own grant names that dataset.
function createRequest(p) {
    const policy = mountLib.state().policy
    const name = slug(p.name || p.title)
    if (!name) throw new Error("Name the dataset")
    const request = {
        datasetId: policy.datasetIdPrefix + name,
        title: p.title || p.name,
        dataDocumentId: p.dataDocumentId,
        timeField: p.timeField || "_timestamp",
        scopePolicyId: p.scopePolicyId
    }
    const parameters = scopeParameters(p.scopePolicyId, p.scopeField)
    if (parameters) request.rowFilterParameters = parameters
    return request
}
// EditDataDocument's values as store#BusinessDataset's request. The scope field goes under the parameter of the policy
// chosen (the dataset's own when the form names none), beside what the dataset already holds under that policy.
function storeRequest(p) {
    const request = {
        datasetId: p.datasetId,
        title: p.title || null,
        timeField: p.timeField || null,
        scopePolicyId: p.scopePolicyId || null,
        expectedVersion: p.expectedVersion
    }
    if (text(p.scopeField).trim()) {
        const current = must("get#BusinessDataset", {datasetId: p.datasetId}).dataset || {}
        const scopePolicyId = p.scopePolicyId || current.scopePolicyId
        const kept = scopePolicyId === current.scopePolicyId ? current.rowFilterParameters : null
        request.rowFilterParameters = scopeParameters(scopePolicyId, p.scopeField, kept)
    }
    return request
}

// ===== DatasetView: the rows and their export =====
// The fields a document row shows, by name, leaving out the index's own (_id, _index, ...).
function summary(body) {
    return Object.entries(body || {})
        .filter(([name, value]) => !name.startsWith("_") && value != null && typeof value !== "object")
        .slice(0, 10)
        .map(([name, value]) => name + ": " + value)
        .join(" · ")
}
const rangeOf = (p, policy) => ({from: p.from || policy.range.from, thru: p.thru || policy.range.thru})
// The dataset among those the viewer reads, the range (the mount's when the request names none) and a page of the
// dataset's rows as the viewer reads them, scoped by its row filter in the engine.
function view(p) {
    const mount = mountLib.state()
    const policy = mount.policy
    const range = rangeOf(p, policy)
    const pageIndex = Math.max(0, Number(p.pageIndex) || 0)
    const out = {
        from: range.from,
        thru: range.thru,
        choices: Array.from(ask("find#BusinessDatasets", {status: "admitted"}).out?.datasets || [])
            .filter(row => row.readable === true)
            .map(row => ({key: row.datasetId, text: row.title || row.datasetId}))
            .sort((a, b) => a.text.localeCompare(b.text)),
        dataset: null,
        failure: null,
        documents: [],
        count: 0,
        pageIndex,
        pageSize: viewPageSize,
        pageMaxIndex: 0,
        pageRangeLow: 0,
        pageRangeHigh: 0,
        totalText: "0",
        exploreUrl: mount.links.explore || "",
        exportCapText: String(exportCap).replace(/\B(?=(\d{3})+(?!\d))/g, ","),
        // as text, so that a form shows it as typed rather than as a formatted number
        exportDefault: String(exportDefault)
    }
    if (!p.datasetId) return out
    const got = ask("get#BusinessDataset", {datasetId: p.datasetId})
    if (got.failure || !got.out?.dataset) return Object.assign(out, {failure: got.failure})
    const dataset = present(got.out.dataset)
    if (!dataset.readable) return Object.assign(out, {failure: "Dataset " + dataset.title + " is not admitted"})
    const queried = ask(
        "query#Documents",
        Object.assign(
            {datasetId: dataset.datasetId, queryString: p.queryString || null, pageSize: viewPageSize, pageIndex},
            range
        )
    )
    if (queried.failure) return Object.assign(out, {failure: queried.failure})
    const total = Number(queried.out.total || 0)
    return Object.assign(out, {
        dataset,
        documents: Array.from(queried.out.hits || []).map(hit => ({
            documentId: hit.id,
            title: hit.documentTitle || hit.id,
            summary: summary(hit.body)
        })),
        count: total,
        pageMaxIndex: Math.max(0, Math.ceil(total / viewPageSize) - 1),
        pageRangeLow: total ? pageIndex * viewPageSize + 1 : 0,
        pageRangeHigh: Math.min(total, (pageIndex + 1) * viewPageSize),
        totalText: queried.out.totalRelation === "gte" ? "at least " + total : String(total)
    })
}
function csvCell(value) {
    const cell = value == null ? "" : typeof value === "object" ? JSON.stringify(value) : String(value)
    return /[",\n\r]/.test(cell) ? '"' + cell.replace(/"/g, '""') + '"' : cell
}
// The export of the rows the viewer reads: through query#Documents with the dataset's row filter, to the cap, as CSV or
// JSON, recorded through audit#Export. A refusal is the transition's error.
function exportRows(p) {
    const policy = mountLib.state().policy
    const dataset = present(must("get#BusinessDataset", {datasetId: p.datasetId}).dataset || {})
    if (!dataset.readable) throw new Error("Dataset " + dataset.title + " is not admitted")
    const format = p.format === "json" ? "json" : "csv"
    const limit = Math.min(exportCap, Math.max(1, Number(p.requestedRowLimit) || exportDefault))
    const range = rangeOf(p, policy)
    const bodies = []
    let total = 0
    for (let pageIndex = 0; bodies.length < limit; pageIndex++) {
        const out = must(
            "query#Documents",
            Object.assign({datasetId: dataset.datasetId, queryString: p.queryString || null, pageSize: 500, pageIndex}, range)
        )
        const hits = Array.from(out.hits || [])
        total = Number(out.total || 0)
        for (const hit of hits) if (bodies.length < limit) bodies.push(Object.assign({_id: hit.id}, hit.body || {}))
        if (hits.length < 500) break
    }
    let content
    if (format === "json") {
        content = p.prettyPrint === "true" || p.prettyPrint === true ? JSON.stringify(bodies, null, 2) : JSON.stringify(bodies)
    } else {
        const columns = Array.from(
            new Set(bodies.flatMap(body => Object.keys(body).filter(name => !name.startsWith("_") || name === "_id")))
        )
        content = [columns.map(csvCell).join(",")]
            .concat(bodies.map(body => columns.map(name => csvCell(body[name])).join(",")))
            .join("\r\n")
    }
    must("audit#Export", {kind: "documents", format, definitionId: dataset.datasetId, rowCount: bodies.length})
    return {
        content,
        contentType: format === "json" ? "application/json" : "text/csv",
        filename: dataset.datasetId + "." + format,
        rowCount: bodies.length,
        capped: total > bodies.length
    }
}

;({list, detail, view, exportRows, createRequest, storeRequest})
