// The export part of org.moqui.admin.DataTransferServices (mr-boss plan/design-admin.md §11 Exports and DataExport, §14
// "Import/export target", §16 "Business export"; impl-admin P16): the catalog of registered export targets, what of a
// target a caller may choose, a preview, the export itself, and the exports made, beside the deliveries of scheduled reports
// and business alerts as status only.
//
// What is exported. Only a registered target (moqui.server.TransferTarget, direction export, its owner's seed row): a request
// names the target's id, one of the scopes the target's scope service gives this caller, values of the selection options the
// registration names, and names of the fields the registration names. No entity, field, filter or service of a request
// reaches a call. The owner's service is called as the caller (DataTransferLib.js call#TargetService), so its own
// permission and scope checks decide, and the registration's exportPermission is asked here besides.
//
// Fields. Each registered field names the permission that reads it. A field the caller may not read is left out: it is not
// in the list the owner's service is given, and of a rows target's answer only the given fields are written, by own name, so
// an owner that answered more is not passed on. The answer names what was left out.
//
// Delivery. The file is the service's answer to its caller and nothing else: it is not stored, mailed or sent to an
// address, and no parameter names one. A record of the export is kept (TransferBatch, state exported: who, the target, the
// scope, the selection, the fields, the row count, the file's name and the hash of its content; never the content), with
// an audit log row. A file is bounded by the target's rows and bytes (at most 50000 rows and 16 MiB): a larger one is
// refused whole, never cut short. An
// operation key is one export's: its repeat makes the file again and keeps no second record; another request under it is
// refused. A rows target's file is written here, a cell a spreadsheet would read as a formula as text; a file target's
// owner writes its own format (one that writes delimited text must write such a cell as text itself).
//
// Status of deliveries. With ADMIN_TRANSFER_VIEW find#Transfers (direction export) also lists the occurrences of scheduled
// business reports and the last outcomes of business alert rules, as the report engine answers its administrative read:
// ids, states and counters, never a title, a recipient, a value or content, and no way to open one.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/DataTransferLib.js")
const batches = ec.resource.script("component://framework/service/org/moqui/admin/DataTransferBatch.js")
const {find, one, text, lineText, plain} = lib
const BATCH = "moqui.server.TransferBatch"
const REPORTS = "org.moqui.report.ReportServices."
const defaultDays = 90
const rangeDays = 366
const perSource = 200
// one export's bounds: the registration's, within these (an export is one answer, held whole while it is made)
const defaultRows = 5000
const hardRows = 50000
const defaultBytes = 2 * 1024 * 1024
const hardBytes = 16 * 1024 * 1024
const boundsOf = target => ({
    maxRows: Math.min(Math.max(Number(target.maxRows) || defaultRows, 1), hardRows),
    maxBytes: Math.min(Math.max(Number(target.maxBytes) || defaultBytes, 1024), hardBytes)
})
const zone = () => text(System.getProperty("default_time_zone")) || "UTC"
const zoned = value => (value == null ? "" : ec.l10n.format(Number(value), "yyyy-MM-dd HH:mm", null, zone()))
const refusal = (reasonCode, reason, extra = {}) => Object.assign({refused: true, reasonCode, reason}, extra)
const notAvailable = () => new Error("This export is not available")
const stateTexts = {
    exported: "Exported",
    running: "Running",
    complete: "Delivered",
    partial: "Partly delivered",
    failed: "Failed",
    uncertain: "Outcome unknown",
    skipped: "Skipped",
    paused: "Paused",
    other: "Other"
}
const contentTypes = {
    "text/csv": "csv",
    "text/vcard": "vcf",
    "text/calendar": "ics",
    "text/plain": "txt",
    "application/json": "json",
    "application/xml": "xml"
}

const exportTarget = targetId => lib.targetOf(targetId, "export")
const dispatched = target => target.targetMode !== "wizard"
function requireExport(target) {
    lib.signedIn()
    for (const permission of lib.names(target.exportPermission))
        if (!ec.user.hasPermission(permission)) throw new Error(permission + " is required")
}
// A target as a reader sees it: never a service name, and of its fields only those of a target exported here.
function describedExport(target) {
    return Object.assign(lib.described(target), {
        exportHere: dispatched(target),
        runsKept: dispatched(target),
        exportAllowed: dispatched(target) && lib.holdsAll(target.exportPermission),
        viewAllowed: lib.holdsAll(target.viewPermission)
    })
}
// The names a caller gave as a list: a list, or one text with commas (a form's field).
const nameList = value =>
    lib
        .list(Array.isArray(value) || value?.toArray ? value : value == null || value === "" ? [] : [value])
        .flatMap(found => String(found).split(","))
        .map(text)
        .filter(Boolean)

// ===== targets and fields =====
// find#ExportTargets: the registered export targets, for a holder of ADMIN_TRANSFER_VIEW or ADMIN_TRANSFER_RUN.
function findExportTargets() {
    lib.requireView()
    return {canRun: ec.user.hasPermission(lib.RUN), targetList: lib.targetsOf("export").map(describedExport)}
}
// What the caller may choose of a target: {scopes, fieldList} with each field's allowed and selected.
function choicesOf(target) {
    const scopes = batches.scopesOf(target)
    const fieldList = target.fields.map(field => {
        const allowed = lib.holdsAll(field.permission)
        return {
            name: field.name,
            label: field.label,
            required: field.required,
            sensitive: field.sensitive,
            allowed,
            // what an export with no list of fields takes: the registration's default
            byDefault: field.required || field.selected,
            selected: allowed && (field.required || field.selected),
            // why it is not offered: never the permission's name
            note: allowed ? null : "Not included: you may not read it"
        }
    })
    return {scopes, fieldList}
}
// get#ExportFields: one target as its wizard needs it.
function getExportFields(p) {
    lib.signedIn()
    const target = exportTarget(p.targetId)
    const out = Object.assign(describedExport(target), {
        scoped: false,
        scopeList: [],
        optionList: [],
        fieldList: [],
        maxRows: null,
        maxBytes: null,
        problem: null
    })
    if (!dispatched(target) || !out.exportAllowed) return {target: out}
    const {scopes, fieldList} = choicesOf(target)
    const bounds = boundsOf(target)
    Object.assign(out, {
        scoped: !scopes.unscoped,
        scopeList: scopes.scopeList,
        optionList: target.options.map(option => ({
            name: option.name,
            label: option.label,
            type: option.type,
            min: option.min,
            max: option.max,
            maxLength: option.maxLength,
            options: option.options,
            defaultValue: option.defaultValue
        })),
        fieldList,
        maxRows: bounds.maxRows,
        maxBytes: bounds.maxBytes,
        problem: scopes.failure ? "Its scopes could not be read" : null
    })
    if (out.scoped && !out.scopeList.length) out.exportAllowed = false
    return {target: out}
}

// ===== the request =====
// A request as the registration admits it: the target, a scope the caller has, the registered selection and the fields the
// caller chose and may read. {refusal} for what a person can correct; an Error for a request no screen makes.
function requestOf(p) {
    const target = exportTarget(p.targetId)
    if (!dispatched(target)) throw new Error(lineText(target.title) + " is exported in its owner's own place")
    requireExport(target)
    const {scopes, fieldList} = choicesOf(target)
    if (scopes.failure) throw new Error("The scopes of this export could not be read. Try again later.")
    const scopeKey = text(p.scopeKey)
    const scopeWord = (lineText(target.scopeLabel, 40) || "a scope").toLowerCase()
    if (scopes.unscoped) {
        if (scopeKey) throw new Error("This export takes no scope")
    } else if (!scopeKey) return {refusal: refusal("SCOPE_REQUIRED", "Choose " + scopeWord + " to export")}
    // a scope the caller does not have reads as one that does not exist
    else if (!scopes.scopeList.some(scope => scope.scopeKey === scopeKey)) throw notAvailable()
    let selection
    try {
        const given = plain(p.selection)
        for (const name of Object.keys(given))
            if (!target.options.some(option => option.name === name))
                throw new Error("This export has no option " + lineText(name, 40))
        selection = batches.optionsOf(target, given)
    } catch (error) {
        return {refusal: refusal("SELECTION_INVALID", lineText(error?.message ?? error, 250))}
    }
    const asked = nameList(p.fieldList)
    // a field is one of the registration's, by its own name: nothing else is a field
    const unknown = asked.find(name => !fieldList.some(field => field.name === name))
    if (unknown) throw new Error("This export has no field " + lineText(unknown, 40))
    // with no list given the registration's defaults are taken; a wanted field the caller may not read is named as left out
    const wanted = fieldList.filter(field => field.required || (asked.length ? asked.includes(field.name) : field.byDefault))
    const included = wanted.filter(field => field.allowed)
    const omitted = wanted.filter(field => !field.allowed)
    if (!included.length) return {refusal: refusal("NO_FIELDS", "None of the chosen fields may be read")}
    return {
        target,
        scopeKey,
        scopeTitle: scopes.scopeList.find(scope => scope.scopeKey === scopeKey)?.title || null,
        selection,
        included,
        omitted,
        bounds: boundsOf(target)
    }
}
const fieldAnswer = request => ({
    fieldList: request.included.map(field => ({name: field.name, label: field.label})),
    omittedFieldList: request.omitted.map(field => ({name: field.name, label: field.label, note: field.note}))
})
// The owner's answer to the export step: {out} or {refusal}; the owner's own errors stay on the request.
function ask(request, countOnly) {
    const out = lib.step(request.target, "export", {
        targetId: request.target.targetId,
        scopeKey: request.scopeKey || null,
        selection: request.selection,
        fieldList: request.included.map(field => field.name),
        countOnly,
        maxRows: request.bounds.maxRows
    })
    if (ec.message.hasError()) return {failed: true}
    if (out.refused === true)
        return {
            refusal: refusal(
                lineText(out.reasonCode, 40).replace(/[^A-Z0-9_]/g, "") || "OWNER_REFUSED",
                lineText(out.reason, 250) || "Its owner refused the export"
            )
        }
    const rowCount = Number(out.rowCount)
    if (!Number.isInteger(rowCount) || rowCount < 0) throw new Error(lineText(request.target.title) + " did not answer a count")
    if (rowCount > request.bounds.maxRows)
        return {
            refusal: refusal(
                "TOO_MANY_ROWS",
                "This selection has " +
                    rowCount +
                    " rows and at most " +
                    request.bounds.maxRows +
                    " are exported at a time: narrow it. Nothing was exported."
            )
        }
    return {out, rowCount}
}
const stampText = () => ec.l10n.format(lib.ms(ec.user.nowTimestamp), "yyyyMMdd-HHmm", null, zone())
// The file of an answer: written here for a rows target (the given fields only, by own name), the owner's for a file target.
function fileOf(request, out, rowCount) {
    const target = request.target
    let fileText
    let contentType
    let filename
    if (target.targetMode === "rows") {
        const records = lib
            .list(out.rowList)
            .slice(0, request.bounds.maxRows)
            .map(plain)
            .map(row =>
                Object.fromEntries(
                    request.included.map(field => [field.label, Object.hasOwn(row, field.name) ? (row[field.name] ?? "") : ""])
                )
            )
        if (records.length !== rowCount) throw new Error(lineText(target.title) + " answered a count that is not its rows")
        fileText = lib.writeDelimited(
            request.included.map(field => field.label),
            records
        )
        contentType = "text/csv"
        filename = target.targetId + "-" + stampText() + ".csv"
    } else {
        fileText = out.fileText == null ? "" : String(out.fileText)
        const given = text(out.contentType).toLowerCase().split(";")[0].trim()
        contentType = Object.hasOwn(contentTypes, given) ? given : "application/octet-stream"
        const extension = contentTypes[contentType] || "dat"
        const base =
            lineText(out.fileName, 80)
                .replace(/\.[A-Za-z0-9]{1,8}$/, "")
                .replace(/[^A-Za-z0-9_-]+/g, "-")
                .replace(/^-+|-+$/g, "") || target.targetId
        // the name says when; its extension is its type's, whatever the owner named
        filename = base.slice(0, 60) + "-" + stampText() + "." + extension
    }
    if (fileText.length > request.bounds.maxBytes)
        return {
            refusal: refusal(
                "FILE_TOO_LARGE",
                "The file would be larger than " +
                    Math.floor(request.bounds.maxBytes / 1024) +
                    " KiB: narrow the selection. Nothing was exported."
            )
        }
    return {fileText, contentType, filename}
}
const requestHash = request =>
    lib
        .sha(
            JSON.stringify({
                targetId: request.target.targetId,
                scopeKey: request.scopeKey,
                selection: request.selection,
                fields: request.included.map(field => field.name),
                targetRevision: request.target.revision
            })
        )
        .slice(0, 40)

// ===== preview and run =====
// preview#Export: what an export of this request would hold, changing and keeping nothing: the fields that would be written
// and those left out, and the owner's count of rows. Under the caller's authority only.
function previewExport(p) {
    const request = requestOf(p)
    if (request.refusal) return request.refusal
    const asked = ask(request, true)
    if (asked.failed) return {}
    if (asked.refusal) return Object.assign(asked.refusal, fieldAnswer(request))
    return Object.assign(fieldAnswer(request), {
        targetId: request.target.targetId,
        scopeKey: request.scopeKey || null,
        scopeTitle: request.scopeTitle,
        selection: request.selection,
        rowCount: asked.rowCount,
        maxRows: request.bounds.maxRows,
        targetVersion: lineText(request.target.targetVersion, 20) || null
    })
}
// run#Export: the file, to its caller, with the record and the audit row of the export.
function runExport(p) {
    const request = requestOf(p)
    if (request.refusal) return request.refusal
    const target = request.target
    const key = lib.operationKey(p)
    const hash = requestHash(request)
    // the key is one export's: the same user and request answer it again; anything else under it is refused
    const earlier = key ? find(BATCH, {transferId: key}).forUpdate(true).one() : null
    if (
        earlier &&
        (earlier.userId !== ec.user.userId ||
            earlier.targetId !== target.targetId ||
            earlier.state !== "exported" ||
            earlier.inputFingerprint !== hash)
    )
        return refusal("OPERATION_KEY_CONFLICT", "That operation key was used for another export or import")
    const asked = ask(request, false)
    if (asked.failed) return {}
    if (asked.refusal) return Object.assign(asked.refusal, fieldAnswer(request))
    const file = fileOf(request, asked.out, asked.rowCount)
    if (file.refusal) return Object.assign(file.refusal, fieldAnswer(request))
    const transferId =
        key || "export-" + lib.sha([ec.user.userId, target.targetId, hash, ec.user.randomString(20)].join("|")).slice(0, 32)
    if (!earlier) {
        const now = ec.user.nowTimestamp
        lib.write(
            BATCH,
            {
                transferId,
                targetId: target.targetId,
                scopeKey: request.scopeKey || null,
                userId: ec.user.userId,
                state: "exported",
                filename: file.filename,
                // evidence of what was made, never the content
                fileHash: lib.sha(file.fileText),
                requestJson: JSON.stringify({
                    direction: "export",
                    selection: request.selection,
                    fields: request.included.map(field => field.name),
                    omittedFields: request.omitted.map(field => field.name),
                    contentType: file.contentType
                }),
                inputFingerprint: hash,
                rowCount: asked.rowCount,
                stagedDate: now,
                completedDate: now
            },
            true
        )
        lib.auditRow(
            BATCH,
            "export",
            transferId,
            target.targetId,
            null,
            (
                asked.rowCount +
                " rows of " +
                (lineText(target.title, 60) || target.targetId) +
                (request.scopeKey ? " (" + request.scopeKey + ")" : "") +
                "; fields: " +
                request.included.map(field => field.name).join(", ")
            ).slice(0, 255),
            null
        )
    }
    return Object.assign(fieldAnswer(request), {
        exported: true,
        repeated: !!earlier,
        transferId,
        targetId: target.targetId,
        scopeKey: request.scopeKey || null,
        rowCount: asked.rowCount,
        fileText: file.fileText,
        filename: file.filename,
        contentType: file.contentType
    })
}

// ===== exports made and delivery status =====
// The records of a target's exports the caller may read: their own while they hold its export or view permission, and with
// its view permission the others'; each within the scopes its scope service admits. Summaries only.
function exportRunsOf(target, filter) {
    const scopes = batches.scopesOf(target)
    const viewer = !!text(target.viewPermission) && lib.holdsAll(target.viewPermission) && !scopes.failure
    const exporter = lib.holdsAll(target.exportPermission) || viewer
    const titles = new Map(scopes.scopeList.map(scope => [scope.scopeKey, scope.title]))
    const names = new Map()
    return Array.from(
        find(BATCH, {targetId: target.targetId, state: "exported"})
            .condition("stagedDate", "greater-equals", lib.iso(filter.fromDate))
            .condition("stagedDate", "less", lib.iso(filter.thruDate))
            .orderBy("-stagedDate")
            .list()
    )
        .filter(row => {
            const scopeKey = text(row.scopeKey)
            if (filter.scopeKey && scopeKey !== filter.scopeKey) return false
            return (
                ((row.userId === ec.user.userId && exporter) || viewer) && (scopeKey ? titles.has(scopeKey) : !!scopes.unscoped)
            )
        })
        .slice(0, perSource)
        .map(row => {
            if (!names.has(row.userId)) {
                const account = one("moqui.security.UserAccount", {userId: row.userId})
                names.set(row.userId, text(account?.userFullName) || text(account?.username) || row.userId)
            }
            return {
                kind: "export",
                runId: row.transferId,
                fileLabel: lineText(row.filename, 120) || null,
                scopeKey: text(row.scopeKey) || null,
                scopeTitle: titles.get(text(row.scopeKey)) || null,
                state: "exported",
                stateText: stateTexts.exported,
                startedDate: lib.ms(row.stagedDate),
                userName: names.get(row.userId),
                rowCount: row.rowCount == null ? null : Number(row.rowCount),
                fieldCount: lib.list(lib.json(row.requestJson, {}).fields).length,
                // the file was its exporter's answer and is not kept: there is nothing to open
                download: false
            }
        })
}
// A report engine's administrative read, apart: its rows, or null when it did not answer (named by the caller).
function engineRows(serviceName, listName) {
    const found = []
    for (let pageIndex = 0; pageIndex * 100 < perSource; pageIndex++) {
        const asked = lib.callRegistered(REPORTS + serviceName, {administrative: true, pageIndex, pageSize: 100})
        if (asked.failure) {
            ec.logger.error("Exports: " + serviceName + " did not answer: " + lineText(asked.failure, 300))
            return null
        }
        const rows = lib.list(asked.out[listName]).map(plain)
        found.push(...rows)
        if (rows.length < 100) break
    }
    return found
}
const count = value => (value == null || !Number.isFinite(Number(value)) ? 0 : Math.max(Math.trunc(Number(value)), 0))
const accountName = (userId, names) => {
    if (!userId) return null
    if (!names.has(userId)) {
        const account = one("moqui.security.UserAccount", {userId})
        names.set(userId, text(account?.userFullName) || text(account?.username) || userId)
    }
    return names.get(userId)
}
// An occurrence of a scheduled report as status: its id, when, whose, its state and its counters. No title, no recipient.
function occurrenceRun(row, names) {
    const counts = {
        recipientCount: count(row.recipientCount),
        sentCount: count(row.sentCount),
        skippedCount: count(row.skippedCount),
        failedCount: count(row.failedCount),
        uncertainCount: count(row.uncertainCount)
    }
    // the engine's own state of the run, and by its counters besides: a run with an uncertain or a failed delivery is never
    // read as delivered, whatever it is called
    const engine = {
        RptOccRunning: "running",
        RptOccComplete: "complete",
        RptOccIncomplete: "partial",
        RptOccSkipped: "skipped",
        RptOccFailed: "failed"
    }
    const byCounters =
        counts.uncertainCount > 0
            ? "uncertain"
            : counts.failedCount > 0
              ? counts.sentCount > 0
                  ? "partial"
                  : "failed"
              : counts.sentCount > 0
                ? "complete"
                : counts.recipientCount > 0 || counts.skippedCount > 0
                  ? "skipped"
                  : "other"
    const named = Object.hasOwn(engine, text(row.statusId)) ? engine[text(row.statusId)] : null
    const state =
        named === "running" || named === "failed" || named === "skipped"
            ? named
            : counts.uncertainCount > 0
              ? "uncertain"
              : named === "complete" && counts.failedCount > 0
                ? byCounters
                : named || (row.completedDate == null ? "running" : byCounters)
    return Object.assign(counts, {
        kind: "reportDelivery",
        runId: lineText(row.occurrenceId, 60),
        fileLabel: null,
        scopeKey: null,
        scopeTitle: null,
        state,
        stateText: stateTexts[state],
        engineStatus: lineText(row.statusId, 40).replace(/[^A-Za-z0-9_-]/g, "") || null,
        reasonCode: lineText(row.reasonCode, 40).replace(/[^A-Za-z0-9_-]/g, "") || null,
        startedDate: lib.ms(row.occurrenceDate),
        userName: accountName(text(row.ownerUserId), names),
        rowCount: null,
        download: false
    })
}
// A business alert rule's deliveries as counters: no title, no series, no value, no recipient.
function alertRun(row, names) {
    const counts = {
        recipientCount: null,
        sentCount: count(row.sentCount ?? row.sent),
        skippedCount: count(row.skippedCount ?? row.skipped),
        failedCount: count(row.failedCount ?? row.failed),
        uncertainCount: 0
    }
    const paused = row.paused === true || row.paused === "Y"
    const state = paused
        ? "paused"
        : counts.failedCount > 0
          ? counts.sentCount > 0
              ? "partial"
              : "failed"
          : counts.sentCount > 0
            ? "complete"
            : counts.skippedCount > 0
              ? "skipped"
              : "other"
    return Object.assign(counts, {
        kind: "alertDelivery",
        // a rule's id carries its title's words: only a digest of it is shown
        runId: text(row.definitionId) ? "rule-" + lib.sha(text(row.definitionId)).slice(0, 10) : "",
        fileLabel: null,
        scopeKey: null,
        scopeTitle: null,
        state,
        stateText: stateTexts[state],
        engineStatus: null,
        reasonCode: null,
        startedDate: lib.ms(row.lastEvaluated ?? row.lastEvaluatedDate ?? row.evaluatedDate),
        userName: accountName(text(row.ownerUserId), names),
        rowCount: null,
        download: false
    })
}
const kindList = [
    {kind: "export", text: "Export"},
    {kind: "reportDelivery", text: "Scheduled report"},
    {kind: "alertDelivery", text: "Business alert"}
]
// find#Transfers with direction export: the exports of every target the caller may read and, for a holder of
// ADMIN_TRANSFER_VIEW, the deliveries of scheduled reports and business alerts as status. Newest first, a page at a time.
function findExportTransfers(p) {
    lib.requireView()
    const {pageIndex, pageSize} = lib.pageOf(p)
    const now = lib.ms(ec.user.nowTimestamp)
    const from = lib.ms(p.fromDate)
    const thru = lib.ms(p.thruDate)
    if ((p.fromDate && (from == null || Number.isNaN(from))) || (p.thruDate && (thru == null || Number.isNaN(thru))))
        throw new Error("A date is not a date")
    const until = thru ?? now + 60000
    const since = Math.max(from ?? until - defaultDays * 86400000, until - rangeDays * 86400000)
    const state = text(p.state)
    const kind = text(p.kind)
    if (kind && !kindList.some(option => option.kind === kind)) throw new Error("Unknown kind " + lineText(kind, 40))
    const scopeKey = text(p.scopeKey)
    const targets = lib.targetsOf("export")
    const wanted = text(p.targetId)
    if (wanted && !targets.some(target => target.targetId === wanted)) throw new Error("Unknown export target " + wanted)
    const all = []
    const unanswered = []
    if (!kind || kind === "export")
        for (const target of targets) {
            if (!dispatched(target)) continue
            if ((wanted && target.targetId !== wanted) || (text(p.ownerApp) && text(target.ownerApp) !== text(p.ownerApp)))
                continue
            const described = lib.described(target)
            for (const run of exportRunsOf(target, {fromDate: since, thruDate: until, scopeKey}))
                all.push(
                    Object.assign(run, {
                        targetId: target.targetId,
                        targetTitle: described.title,
                        ownerApp: described.ownerApp,
                        ownerRoute: described.ownerRoute
                    })
                )
        }
    // the report engine's own reads ask ADMIN_TRANSFER_VIEW; a holder of the run permission alone is not shown them
    if (ec.user.hasPermission(lib.VIEW) && !wanted && !scopeKey && !text(p.ownerApp)) {
        const names = new Map()
        const sources = [
            {
                kind: "reportDelivery",
                service: "find#ReportOccurrences",
                list: "occurrences",
                run: occurrenceRun,
                title: "Scheduled report"
            },
            {kind: "alertDelivery", service: "find#AlertDeliveries", list: "deliveries", run: alertRun, title: "Business alert"}
        ]
        for (const source of sources) {
            if (kind && kind !== source.kind) continue
            const rows = engineRows(source.service, source.list)
            if (rows == null) {
                unanswered.push({targetId: source.kind, title: source.title + " deliveries", ownerApp: "Reports"})
                continue
            }
            for (const row of rows) {
                const run = source.run(row, names)
                if (!run.runId) continue
                // a rule never evaluated has no date: it is listed, last
                if (run.startedDate != null && (run.startedDate < since || run.startedDate >= until)) continue
                all.push(
                    Object.assign(run, {
                        targetId: null,
                        targetTitle: source.title,
                        ownerApp: "Reports",
                        ownerRoute: source.kind
                    })
                )
            }
        }
    }
    const matching = all
        .filter(run => !state || run.state === state)
        .sort((a, b) => (b.startedDate ?? 0) - (a.startedDate ?? 0) || String(a.runId).localeCompare(String(b.runId)))
    return {
        transferList: matching
            .slice(pageIndex * pageSize, (pageIndex + 1) * pageSize)
            .map(run => Object.assign(run, {startedText: zoned(run.startedDate), detail: false, ownerParameters: {}})),
        transferCount: matching.length,
        pageIndex,
        pageSize,
        unansweredTargetList: unanswered,
        stateList: Object.entries(stateTexts)
            .filter(([value]) => value !== "other")
            .map(([value, label]) => ({state: value, text: label})),
        kindList,
        defaultDays,
        rangeDays,
        timeZone: zone()
    }
}

const operations = {findExportTargets, getExportFields, previewExport, runExport}
;({
    findExportTransfers,
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown data export operation " + op)
        return operations[op](parameters)
    }
})
