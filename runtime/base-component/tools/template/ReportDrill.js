// What the Drill screen of Dashboards shows (screen/Dashboards/Drill.xml; docs/tools/Monitoring.md, "Drill";
// plans/archive/Admin-Screens-Down.md, Track I): from a value of a saved panel to a breakdown and the records behind it. The
// request names the definition, the panel, one of its queries, the range and the buckets chosen, as the panel showed
// them; the engine's get#Drill reads the saved panel again as the viewer and computes every predicate itself, so
// nothing here or in the request is a query, an entity or a service. Presentation only: get#Drill and get#Definition
// are called as the viewer, and no entity is read. A record's columns are the query's own time, groups and measured
// fields, never the whole document; a viewer who reads the totals and not the records is told so and gets no record.
//
// view(request, options) answers the page. Under a mount (template/ReportMount.js) the range, when the request names
// none, is the mount's, and the mount's fixed variables (reportFixedVariables, as Dashboard reads them) replace
// whatever the request carries. An app with a drill screen of its own around these sections (BOSS Insights › Drill)
// calls view with its own parts as options, each decided by the app's service, never by a request:
//     definition      the definition as the viewer reads it, when the app has read it already
//     fixedVariables  the variables fixed over the request's (the app's scope), in place of reportFixedVariables
//     range           {from, thru}: the range when the request names none, in place of the mount's
//     permitted       name => whether a field may be a column or a breakdown dimension for the viewer (a cost)
//     row             (source, shown, carry) => shown: what the app adds to a record's row (recordUrl and
//                     recordParameters, where it has a screen for the record; Tools has none), carry being what a link
//                     back to this drill carries
//     carry           further request parameters its links carry (its scope choice)
const mountLib = ec.resource.script("component://tools/template/ReportMount.js")
const {ask} = ec.resource.script("component://tools/template/ReportDelivery.js")
const pageSize = 20
const bucketRows = 200
const refusalText = /not authorized|may not read|is not admitted|does not exist|not available/i
const carried = ["panelId", "queryReference", "from", "thru", "variablesJson", "breakdownDimension"]

function text(value) {
    if (value == null) return ""
    if (Array.isArray(value)) return value.map(text).join(", ")
    if (typeof value === "object") return JSON.stringify(value)
    return String(value)
}
// A value of a frame as the panel shows it: a time as a date and time in the user's zone, else its text.
function shown(field, value) {
    if (value == null) return ""
    if (field.type === "time" || field.kind === "time") {
        const millis = /^-?\d+$/.test(String(value)) ? Number(value) : Date.parse(String(value))
        if (!Number.isNaN(millis)) return ec.l10n.format(new Date(millis), "yyyy-MM-dd HH:mm")
    }
    return text(value)
}
// The request's buckets: a list of {field, value}, also as the JSON text a link carries (bucketsJson); null when the
// text is not readable.
function buckets(p) {
    let given = Array.from(p.selectedBuckets || [])
    if (p.bucketsJson) {
        try {
            const parsed = JSON.parse(String(p.bucketsJson))
            if (Array.isArray(parsed)) given = given.concat(parsed)
        } catch (error) {
            return null
        }
    }
    return given
        .filter(item => item && typeof item === "object")
        .map(item => ({field: String(item.field || ""), value: item.value == null ? null : item.value}))
}
function variablesOf(p) {
    if (p.variables && typeof p.variables === "object") return Object.assign({}, p.variables)
    if (!p.variablesJson) return {}
    try {
        const parsed = JSON.parse(String(p.variablesJson))
        return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? parsed : {}
    } catch (error) {
        return {}
    }
}
// The rows of a breakdown or of the panel itself as a list: one row a bucket, its group values, its measures and, for a
// group, the bucket to choose.
function frameRows(frames, groupOnly) {
    const out = []
    for (const frame of Array.from(frames || [])) {
        const fields = Array.from(frame.fields || [])
        const groups = fields.filter(field => field.config?.bucket)
        if (groupOnly && !groups.length) continue
        const length = Math.max(0, ...fields.map(field => Array.from(field.values || []).length))
        for (let index = 0; index < length && out.length < bucketRows; index++) {
            const group = groups[0]
            const label = group ? text(group.config.bucket.labels?.[index] ?? group.values[index]) : ""
            out.push({
                groupField: group?.name || "",
                groupValue: group ? text(group.values[index] ?? label) : "",
                label: groups.map(field => shown(field, field.config.bucket.labels?.[index] ?? field.values[index])).join(" · "),
                valuesText: fields
                    .filter(field => !field.config?.bucket && !["other", "missing"].includes(field.name))
                    .map(field => field.name + " " + shown(field, field.values[index]) + (field.unit ? " " + field.unit : ""))
                    .join(", ")
            })
        }
    }
    return out
}
const unavailable = (p, reason, more) =>
    Object.assign(
        {
            available: false,
            reason,
            selected: [],
            selectedText: "",
            dimensions: [],
            breakdown: [],
            panelBuckets: [],
            rows: [],
            columns: [],
            carry: {definitionId: p.definitionId}
        },
        more || {}
    )

function view(p, options = {}) {
    const mount = mountLib.state()
    const permitted = typeof options.permitted === "function" ? options.permitted : () => true
    const selected = buckets(p)
    if (!selected) return unavailable(p, "The chosen buckets are not readable")
    if (p.breakdownDimension && !permitted(p.breakdownDimension))
        return unavailable(p, "A breakdown by " + p.breakdownDimension + " is not available to you")
    const definition = options.definition || ask("get#Definition", {definitionId: String(p.definitionId || "")}).out?.definition
    // a definition the viewer may not read answers alike, whether or not it exists
    if (!definition) return unavailable(p, "This value is not available to you")
    const defaults = options.range || mount.policy.range
    // the fixed variables over whatever the request carries, as the dashboard fixes them
    const fixed = options.fixedVariables || (mount.mounted ? ec.context.get("reportFixedVariables") : null) || {}
    const asked = ask("get#Drill", {
        definitionId: p.definitionId,
        panelId: p.panelId || null,
        queryReference: p.queryReference == null || p.queryReference === "" ? null : String(p.queryReference),
        range: {from: p.from || defaults.from, thru: p.thru || defaults.thru},
        variables: Object.assign(variablesOf(p), fixed),
        selectedBuckets: selected,
        breakdownDimension: p.breakdownDimension || null,
        pageIndex: p.pageIndex ? Number(p.pageIndex) : 0,
        pageSize
    })
    if (asked.failure)
        return unavailable(p, refusalText.test(asked.failure) ? "This value is not available to you" : asked.failure)
    const answer = asked.out
    if (!answer.available) return unavailable(p, answer.reason || "This value is not available", {title: answer.title})
    const columns = Array.from(answer.columns || []).filter(permitted)
    // what a link back to this drill carries: the request as it was, which the drill checks again when it is opened
    const carry = {definitionId: p.definitionId}
    for (const name of carried.concat(Array.from(options.carry || [])))
        if (p[name] != null && String(p[name]) !== "") carry[name] = String(p[name])
    if (selected.length) carry.bucketsJson = JSON.stringify(selected)
    const withBucket = (field, value) =>
        Object.assign({}, carry, {bucketsJson: JSON.stringify(selected.concat([{field, value}])), breakdownDimension: ""})
    const rowOf = typeof options.row === "function" ? options.row : (source, row) => row
    const rows = Array.from(answer.rows || []).map(source => {
        const body = source.body || {}
        return rowOf(
            source,
            {
                id: source.id,
                title: source.title || source.id,
                valuesText: columns
                    .map(column => column + " " + shown({type: /date|time/i.test(column) ? "time" : ""}, body[column]))
                    .join(", "),
                recordUrl: null,
                recordParameters: null
            },
            carry
        )
    })
    const size = Number(answer.pageSize || pageSize)
    const total = Number(answer.total || 0)
    const pageIndex = Number(answer.pageIndex || 0)
    const chosen = Array.from(answer.selected || [])
    return {
        available: true,
        title: answer.title || definition.title,
        definitionType: definition.type,
        datasetId: answer.datasetId,
        range: answer.range,
        selected: chosen,
        selectedText: chosen.map(item => (item.sourceField || item.field) + ": " + item.label).join(" › "),
        dimensions: Array.from(answer.dimensions || [])
            .filter(permitted)
            .map(name => ({value: name, label: name})),
        // the panel's own buckets to choose from (no bucket chosen yet), and the breakdown's
        panelBuckets: selected.length
            ? []
            : frameRows(answer.panelFrames, true).map(row =>
                  Object.assign(row, {chooseParameters: withBucket(row.groupField, row.groupValue)})
              ),
        breakdownDimension: answer.breakdown?.dimension || null,
        breakdownRefused: answer.breakdown?.refused || null,
        breakdown: frameRows(answer.breakdown?.frames, false),
        // the panel's own result as the engine answered it, for a caller that presents it itself
        panelFrames: selected.length ? [] : Array.from(answer.panelFrames || []),
        rowsAvailable: !!answer.rowsAvailable,
        rowsReason: answer.rowsAvailable ? null : "You may read this total, not the records behind it.",
        columns,
        rows,
        total,
        bounded: !!answer.bounded,
        pageIndex,
        pageMaxIndex: Math.max(0, Math.ceil(total / size) - 1),
        carry,
        breakdownValues: Object.assign({}, carry, {breakdownDimension: answer.breakdown?.dimension || ""}),
        clearParameters: Object.assign({}, carry, {bucketsJson: "", breakdownDimension: ""}),
        previousParameters: Object.assign({}, carry, {pageIndex: Math.max(0, pageIndex - 1)}),
        nextParameters: Object.assign({}, carry, {pageIndex: pageIndex + 1})
    }
}

;({view})
