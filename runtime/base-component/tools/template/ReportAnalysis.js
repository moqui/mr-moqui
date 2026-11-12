// What the Analysis screen of Dashboards shows and asks (screen/Dashboards/Analysis.xml; docs/tools/Monitoring.md,
// "Analysis"; plans/Admin-Screens-Down.md, Track I): an aggregate over a dataset the viewer reads, built from typed
// choices (the dataset, a measure and how it is aggregated, up to two groups, a time grain, one filter), run through the
// report engine as the viewer and saved as an ordinary library panel, which the definition editor then refines. No query
// language is added: the choices become the engine's own query, each field checked against the dataset's fields, and
// the engine's semantic guards ("Semantic guards") refuse what does not mean anything: an amount across units until it
// is grouped by its unit or filtered to one, a ratio averaged, a balance summed across its dates, a total of a bounded
// page. A refusal is shown as its reason; nothing is computed around it. Presentation only: every read and write is an
// operation of org.moqui.report.ReportServices called as the viewer, so the dataset's own artifact and row filter decide
// the rows; no entity is read here.
//
// Under a mount (template/ReportMount.js) the range, when the request names none, is the mount's, a time grain's
// buckets are cut by the mount's calendar (reportPolicy.calendar: the zone, and a week's offset), the saved panel opens
// in the mount's panel editor and it is shared with a group as a dashboard is (ReportShare.js: the viewer's own groups,
// the engine's save checking the group). An app with business parts of its own (which datasets are offered, a scope
// choice with its own calendar, who may build one) shows the screen's sections in a screen of its own, with the page
// from its own service (BOSS Insights › Report builder › Analysis), and builds on what is exported here: the choices'
// lists, fieldsOf, choicesOf, queryOf and resultOf.
const mountLib = ec.resource.script("component://tools/template/ReportMount.js")
const shareLib = ec.resource.script("component://tools/template/ReportShare.js")
const {ask, offered, refuse, done} = ec.resource.script("component://tools/template/ReportDelivery.js")

const aggregates = [
    {value: "count", label: "Count of records"},
    {value: "sum", label: "Sum"},
    {value: "avg", label: "Average"},
    {value: "min", label: "Least"},
    {value: "max", label: "Greatest"},
    {value: "cardinality", label: "Distinct values"}
]
const grains = [
    {value: "day", label: "Day"},
    {value: "week", label: "Week"},
    {value: "month", label: "Month"},
    {value: "quarter", label: "Quarter"},
    {value: "year", label: "Year"}
]
const visualizations = ["table", "bar", "line", "stat"]
const visualizationOptions = visualizations.map(value => ({value, label: value.charAt(0).toUpperCase() + value.slice(1)}))
const guardCode = /\b(UNIT_MIXED|RATIO_AGGREGATE|NONADDITIVE|BOUNDED_RESULT|ROWS_RESTRICTED)\b/
const rowsMax = 500
const text = value => String(value ?? "").trim()

// ===== refusals =====
// A refusal in its own words, thrown where it is found and shown as the transition's error (attempt). A caller with
// reason codes of its own passes its own refusal (deny(code, words)) to queryOf.
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
const failCoded = (code, words) => fail(words)

// ===== the source =====
// A dataset's fields as choices, each with what the dataset's definition declares of it (fieldDefaults: its unit
// field, a ratio, the dimensions it is not added across): fields is get#Dataset's, defaults the definition body's.
function fieldsOf(fields, defaults) {
    const declaredOf = defaults && typeof defaults === "object" ? defaults : {}
    return Array.from(fields || [])
        .filter(field => !String(field.name).startsWith("_"))
        .map(field => {
            const declared = (Object.hasOwn(declaredOf, field.name) && declaredOf[field.name]) || {}
            const notes = []
            if (declared.unitField) notes.push("in its " + declared.unitField)
            if (declared.ratio) notes.push("a ratio")
            if (declared.nonAdditive) notes.push("not added across " + Array.from(declared.nonAdditive).join(", "))
            return {
                value: field.name,
                label: (declared.displayName || field.name) + (notes.length ? " (" + notes.join("; ") + ")" : ""),
                type: field.type,
                aggregatable: field.aggregatable !== false,
                unitField: declared.unitField || null
            }
        })
        .sort((a, b) => a.value.localeCompare(b.value))
}
// The datasets the viewer reads (list#Datasets: each dataset's own artifact) and, for the one chosen, its fields:
// {datasets, dataset, fields}; a dataset named that is not among them is refused.
function sources(datasetId) {
    const listed = ask("list#Datasets", {})
    if (listed.failure) fail(listed.failure)
    const datasets = Array.from(listed.out.datasets || [])
        .map(row => ({value: row.id, label: row.title || row.id, timeField: row.timeField}))
        .sort((a, b) => a.label.localeCompare(b.label))
    const chosen = datasetId ? datasets.find(row => row.value === datasetId) : null
    if (datasetId && !chosen) fail("Choose a dataset you may read")
    if (!chosen) return {datasets, dataset: null, fields: []}
    const found = ask("get#Dataset", {datasetId})
    if (found.failure) fail(found.failure)
    // a saved dataset's definition declares its fields' units and ratios; a telemetry index has none
    const body = ask("get#Definition", {definitionId: datasetId}).out?.definition?.body || {}
    return {
        datasets,
        dataset: Object.assign({}, chosen, {timeField: found.out.dataset?.timeField || chosen.timeField}),
        fields: fieldsOf(found.out.dataset?.fields, body.fieldDefaults)
    }
}
// The form's choices of a source.
function choicesOf(found) {
    return {
        datasets: found.datasets,
        measureFields: found.fields.filter(field => field.type === "number"),
        groupFields: found.fields.filter(field => field.aggregatable && field.type !== "number"),
        filterFields: found.fields.filter(field => field.aggregatable),
        aggregates,
        grains,
        visualizations: visualizationOptions
    }
}

// ===== the query =====
// The engine's calendar bucket parameters of a calendar {timeZone, weekOffset} for a grain: the zone, and a week's
// offset; nothing for a part left empty, where the viewer's zone and the instance's week apply.
function calendarParameters(calendar, grain) {
    const parameters = {}
    if (calendar?.timeZone) parameters.time_zone = calendar.timeZone
    if (grain === "week" && calendar?.weekOffset) parameters.offset = calendar.weekOffset
    return parameters
}
// The engine's query of the typed choices: every field one of the dataset's own. bucketParameters are a time grain's
// calendar parameters (calendarParameters, or an app's own); deny(code, words) is the caller's refusal.
function queryOf(p, found, bucketParameters, deny = failCoded) {
    const named = (value, list, what) => {
        if (!value) return null
        const field = list.find(row => row.value === value)
        if (!field) deny("FIELD_UNAVAILABLE", what + " " + value + " is not a field of this dataset")
        return field
    }
    const aggregate = p.aggregate || "count"
    if (!aggregates.some(row => row.value === aggregate)) deny("VALUE_INVALID", "Choose how the measure is aggregated")
    const measure = named(p.measureField, found.fields, "Measure")
    if (aggregate !== "count" && !measure) deny("VALUE_INVALID", "Choose the field to " + aggregate)
    if (measure && !["count", "cardinality"].includes(aggregate) && measure.type !== "number")
        deny("VALUE_INVALID", measure.value + " is not a number")
    const aggregations = []
    if (p.timeGrain) {
        if (!grains.some(row => row.value === p.timeGrain)) deny("VALUE_INVALID", "Choose a time grain")
        aggregations.push({
            id: "period",
            type: "date_histogram",
            role: "x-axis",
            params: Object.assign({field: found.dataset.timeField, calendar_interval: p.timeGrain}, bucketParameters || {})
        })
    }
    for (const value of [p.groupBy, p.groupBy2].filter(Boolean)) {
        const group = named(value, found.fields, "Group")
        if (!group.aggregatable) deny("FIELD_UNAVAILABLE", value + " cannot be grouped by")
        if (aggregations.some(aggregation => aggregation.id === value)) continue
        aggregations.push({
            id: value,
            type: "terms",
            role: aggregations.length ? "split-rows" : "x-axis",
            params: {field: value, size: 20, other_bucket: true, other_bucket_key: "Other"}
        })
    }
    if (aggregate !== "count") aggregations.push({id: "value", type: aggregate, role: "metric", params: {field: measure.value}})
    const query = {datasetId: found.dataset.value, aggregations}
    const filter = named(p.filterField, found.fields, "Filter")
    if (filter && p.filterValue != null && String(p.filterValue) !== "")
        query.filters = [
            {
                query: {field: {name: filter.value, op: "eq", value: String(p.filterValue)}},
                label: filter.value + ": " + p.filterValue
            }
        ]
    return query
}

// ===== the result =====
function shown(field, value) {
    if (value == null) return ""
    if (field.type === "time") {
        const millis = /^-?\d+$/.test(String(value)) ? Number(value) : Date.parse(String(value))
        if (!Number.isNaN(millis)) return ec.l10n.format(new Date(millis), "yyyy-MM-dd")
    }
    return String(value)
}
// A guard's or the engine's refusal of a query as the answer: its code where it has one, and its reason.
function refusedOf(message, query) {
    const words = String(message ?? "").replace(/^.*failed: /, "")
    const code = words.match(guardCode)
    return {
        available: false,
        refusedCode: code ? code[1] : "QUERY_FAILED",
        reason: words.replace(/^[A-Z_]+: /, ""),
        query,
        rows: [],
        columns: [],
        notices: []
    }
}
// run#Query's answer as the page's rows: one row a bucket, its groups as its label, its measures with their units.
function resultOf(answer, query, range) {
    const frame = Array.from(answer.frames || [])[0] || {fields: []}
    const fields = Array.from(frame.fields || [])
    const length = Math.max(0, ...fields.map(field => Array.from(field.values || []).length))
    // the engine's flags of a bucket (the rest, a missing value) are not measures
    const flags = ["other", "missing"]
    const flagged = (name, index) => fields.find(field => field.name === name)?.values?.[index] === true
    const count = fields.find(field => field.name === "count")
    const rows = []
    for (let index = 0; index < length && rows.length < rowsMax; index++) {
        // an empty rest is not a row
        if (flagged("other", index) && count && Number(count.values[index] || 0) === 0) continue
        rows.push({
            label: fields
                .filter(field => field.config?.bucket)
                .map(field => shown(field, field.config.bucket.labels?.[index] ?? field.values[index]))
                .join(" · "),
            valuesText: fields
                .filter(field => !field.config?.bucket && !flags.includes(field.name))
                .map(field => {
                    // an amount's unit is its row's (grouped by its unit field) or the one unit of the result
                    const unit = flagged("other", index) ? "" : field.config?.units?.[index] || field.unit || ""
                    return field.name + " " + shown(field, field.values[index]) + (unit ? " " + unit : "")
                })
                .join(", ")
        })
    }
    const answered = answer.range || range
    const day = value => {
        const millis = Date.parse(String(value ?? ""))
        return Number.isNaN(millis) ? String(value ?? "") : ec.l10n.format(new Date(millis), "yyyy-MM-dd")
    }
    return {
        available: true,
        query,
        range: answered,
        rangeText: day(answered?.from) + " to " + day(answered?.thru),
        columns: fields.map(field => field.name),
        rows,
        // the records the result is computed from, the engine's own count: never the number of rows shown
        sourceCount: answer.sourceCount == null ? null : Number(answer.sourceCount),
        notices: Array.from(answer.notices || [])
            .map(notice => notice.message)
            .filter(Boolean)
    }
}
// The choices run as the viewer over a source: the result, or the engine's refusal as the answer.
function run(p, found, mount) {
    const query = queryOf(p, found, calendarParameters(mount.policy.calendar, p.timeGrain))
    const range = {from: text(p.from) || mount.policy.range.from, thru: text(p.thru) || mount.policy.range.thru}
    const asked = ask("run#Query", {query: Object.assign({}, query, range)})
    return asked.failure ? refusedOf(asked.failure, query) : resultOf(asked.out, query, range)
}

// ===== what the screen shows =====
const choiceNames = ["datasetId", "aggregate", "measureField", "groupBy", "groupBy2", "timeGrain", "filterField", "filterValue"]
// The page: the source's choices, the form's values, the result when run is asked, and for a result what saving
// offers (the viewer's own groups and dashboards, where their own grant opens the engine's save).
function view(p) {
    const mount = mountLib.state()
    const values = Object.fromEntries(choiceNames.map(name => [name, text(p[name]) || null]))
    values.aggregate = values.aggregate || "count"
    values.from = text(p.from) || mount.policy.range.from
    values.thru = text(p.thru) || mount.policy.range.thru
    const none = {
        datasets: [],
        measureFields: [],
        groupFields: [],
        filterFields: [],
        aggregates,
        grains,
        visualizations: visualizationOptions
    }
    let found = null
    let optionsError = null
    let analysis = null
    try {
        found = sources(values.datasetId)
        if (values.datasetId && p.run) analysis = run(values, found, mount)
    } catch (error) {
        if (!error?.refusal) throw error
        // a refused source leaves the form with no choices; a refused choice is the result's reason
        if (found) analysis = Object.assign(refusedOf(error.message, null), {refusedCode: "CHOICE_REFUSED"})
        else optionsError = error.message
    }
    const canSave = offered("save#Definition") && !!mount.links.panel
    const sharing = analysis?.available && canSave ? shareLib.listState([]) : {mayShare: false, groups: []}
    const dashboards =
        analysis?.available && canSave
            ? Array.from(ask("list#Definitions", {definitionType: "dashboard"}).out?.definitions || [])
                  .filter(row => shareLib.owns(row))
                  .map(row => ({value: row.id, label: row.title || row.id}))
            : []
    return {
        options: found ? choicesOf(found) : none,
        optionsError,
        noDataset: !optionsError && !!found && !found.datasets.length,
        choices: values,
        analysis,
        canSave,
        shareGroups: sharing.mayShare ? sharing.groups : [],
        dashboards,
        saveValues: Object.assign({}, values, {visualization: "table"})
    }
}

// ===== the save =====
// A new library panel's id from its name: unique to its owner and the moment.
function panelId(title) {
    const slug = title
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-+|-+$/g, "")
        .slice(0, 40)
    const owner = String(ec.user.userId || "")
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "")
    return "analysis-" + (slug || "panel") + "-" + owner + "-" + Date.now().toString(36)
}
// A dashboard of the viewer's own to place a saved panel on, read before the panel is saved; a refusal is deny's.
function ownDashboard(dashboardId, deny = failCoded) {
    const dashboard = ask("get#Definition", {definitionId: dashboardId}).out?.definition
    if (!dashboard || dashboard.type !== "dashboard" || !shareLib.owns(dashboard))
        deny("DASHBOARD_NOT_OWNED", "Place the analysis on a dashboard of your own; save a copy of a shared one first")
    return dashboard
}
// The saved panel placed on it by reference, as its last panel.
function placeOn(dashboard, panel, title) {
    const body = dashboard.body || {}
    body.panels = body.panels || {}
    body.panels[panel.id] = {panelRef: panel.id}
    const layout = body.layout && Array.isArray(body.layout.items) ? body.layout : {type: "autoGrid", items: []}
    layout.items.push({panelId: panel.id})
    body.layout = layout
    return ask("save#Definition", {
        definition: {
            id: dashboard.id,
            type: "dashboard",
            title: dashboard.title,
            body,
            version: dashboard.version,
            userGroupId: dashboard.userGroupId || null
        },
        note: "Added analysis " + title
    })
}
// saveAnalysis: the choices saved as a library panel the viewer owns, private or shared with a group offered here
// (ReportShare.js; the engine's save checks the group), and optionally placed by reference on a dashboard of their
// own. An analysis the engine's guards refuse is not saved. Answers the panel's id, or null with the refusal.
function save(p) {
    return attempt(() => {
        const mount = mountLib.state()
        const title = text(p.title)
        if (!title || title.length > 120) fail("Name the analysis in at most 120 characters")
        const visualization = text(p.visualization) || "table"
        if (!visualizations.includes(visualization)) fail("Choose how the analysis is shown")
        const groupId = text(p.userGroupId) || null
        if (groupId) {
            // sharing is the mount's feature, and the group one offered to the viewer here
            const sharing = shareLib.listState([])
            if (!sharing.mayShare || !sharing.groups.some(group => group.value === groupId))
                fail("Share only with a group you belong to")
        }
        const found = sources(text(p.datasetId))
        if (!found.dataset) fail("Choose a dataset you may read")
        // refused now, as it would be on every render (an amount across units, a ratio averaged)
        const checked = run(p, found, mount)
        if (!checked.available) fail(checked.reason)
        const dashboard = text(p.dashboardId) ? ownDashboard(text(p.dashboardId)) : null
        const saved = ask("save#Definition", {
            definition: {
                id: panelId(title),
                type: "panel",
                title,
                userGroupId: groupId,
                body: {title, visualization, queries: [checked.query]}
            },
            note: "Saved from Analysis"
        })
        if (saved.failure) fail(saved.failure)
        const placed = dashboard ? placeOn(dashboard, saved.out.definition, title) : null
        if (placed?.failure)
            ec.message.addMessage("It is saved, and was not added to the dashboard: " + placed.failure, "warning")
        done("Saved as a library panel")
        return saved.out.definition.id
    })
}

;({
    aggregates,
    grains,
    visualizations,
    visualizationOptions,
    fieldsOf,
    choicesOf,
    calendarParameters,
    queryOf,
    refusedOf,
    resultOf,
    panelId,
    view,
    save
})
