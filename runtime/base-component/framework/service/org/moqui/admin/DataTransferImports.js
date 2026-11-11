// The import part of org.moqui.admin.DataTransferServices (mr-boss plan/design-admin.md §11.1, Imports, DataImport and
// ImportDetail, §14 "Import/export target"; impl-admin P15): the catalog of registered import targets, the dispatch of an
// import's steps to its target, the runs of every target and the retry of failed rows.
//
// Dispatch. A request names a target by its id; the registration (moqui.server.TransferTarget, its owner's seed row) names
// what is called. A target with its owner's batch (targetMode steps) has its five steps called with the import contract's
// parameters and nothing else; one without (rows) is kept by DataTransferBatch.js, which calls the target's row services; an
// owner's wizard (wizard) is only listed, with its owner's place. Every owner's service is called as the caller
// (DataTransferLib.js call#TargetService), so its own permission and scope checks decide, and a registration's
// importPermission and viewPermission are checked here besides. No service, entity or field name of a request reaches a call.
//
// Runs. find#Transfers needs ADMIN_TRANSFER_VIEW and lists, for each target the caller may view, the batches kept here and
// what the owner's find service answers for this caller: summaries with the owner's place, never a row's content. An owner
// that does not answer is named and the others are listed.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/DataTransferLib.js")
const batches = ec.resource.script("component://framework/service/org/moqui/admin/DataTransferBatch.js")
const {text, lineText, plain, refused} = lib
const defaultDays = 90
const rangeDays = 366
const perSource = 200
const zone = () => text(System.getProperty("default_time_zone")) || "UTC"
const zoned = value => (value == null ? "" : ec.l10n.format(Number(value), "yyyy-MM-dd HH:mm", null, zone()))
const stateTexts = {
    staged: "Staged",
    mapped: "Mapped",
    validated: "Validated",
    applying: "Applying",
    complete: "Complete",
    partial: "Partly applied",
    failed: "Failed",
    open: "In review",
    running: "Running"
}

const importTarget = targetId => lib.targetOf(targetId, "import")
function requireImport(target) {
    lib.signedIn()
    for (const permission of lib.names(target.importPermission))
        if (!ec.user.hasPermission(permission)) throw new Error(permission + " is required")
}
// The contract's parameters of a request, by name: nothing else of it is passed on.
function stepParameters(p, extra = {}) {
    const out = {}
    for (const name of lib.stepInputs) if (p[name] != null && p[name] !== "") out[name] = p[name]
    return Object.assign(out, extra)
}
// What a target offers in its wizard.
function featuresOf(target) {
    if (target.targetMode === "rows")
        return {
            kinds: false,
            updateMode: false,
            formats: false,
            clearFields: false,
            forceNew: true,
            sample: true,
            valueMappings: true
        }
    const named = lib.names(target.stepFeatures)
    return {
        kinds: named.includes("kinds") && lib.hasStep(target, "kinds"),
        updateMode: named.includes("updateMode"),
        formats: named.includes("formats"),
        clearFields: named.includes("clearFields"),
        forceNew: named.includes("forceNew"),
        sample: named.includes("sample") && lib.hasStep(target, "sample"),
        valueMappings: true
    }
}

// ===== targets =====
// get#ImportTarget: one target as its wizard needs it: its words, what it offers, its bounds, fields and options, and what
// the caller may choose (the kinds its owner gives the caller, the scopes its scope service gives). Never a service name.
function getImportTarget(p) {
    lib.signedIn()
    const target = importTarget(p.targetId)
    const out = Object.assign(lib.described(target), {
        features: featuresOf(target),
        clearableFields: lib.names(target.clearableFields),
        columnList: [],
        optionList: [],
        kindOptions: [],
        scopeList: [],
        scoped: false,
        importAllowed: target.targetMode !== "wizard" && lib.holdsAll(target.importPermission),
        problem: null
    })
    if (target.targetMode === "rows") {
        const bounds = batches.boundsOf(target)
        const scopes = batches.scopesOf(target)
        Object.assign(out, {
            columnList: target.columns,
            optionList: target.options.map(option => ({
                name: option.name,
                label: option.label,
                type: option.type,
                min: option.min,
                max: option.max,
                maxLength: option.maxLength,
                multiline: option.multiline,
                options: option.options,
                defaultValue: option.defaultValue
            })),
            maxBytes: bounds.maxBytes,
            maxRows: bounds.maxRows,
            scoped: !scopes.unscoped,
            scopeList: out.importAllowed ? scopes.scopeList : [],
            problem: scopes.failure ? "Its scopes could not be read" : null
        })
        if (out.scoped && !out.scopeList.length) out.importAllowed = false
    } else if (target.targetMode === "steps" && out.features.kinds) {
        // the owner says which kinds this caller may import: none means the caller may not import here
        const asked = lib.stepApart(target, "kinds", {organizationPartyId: text(p.organizationPartyId) || null})
        if (asked.failure) out.importAllowed = false
        else
            out.kindOptions = lib
                .list(asked.out.kindOptions)
                .map(plain)
                .map(kind => ({value: text(kind.value), label: lineText(kind.label, 100) || text(kind.value)}))
                .filter(kind => kind.value)
        if (!out.kindOptions.length) out.importAllowed = false
    }
    return {target: out}
}
// find#ImportTargets: the registered import targets, for a holder of ADMIN_TRANSFER_VIEW: what each is, whose it is, whether
// it keeps runs and whether the caller holds the permissions its registration names (the owner's own checks still decide).
function findImportTargets() {
    lib.requireView()
    return {
        canRun: ec.user.hasPermission(lib.RUN),
        targetList: lib.targetsOf("import").map(target =>
            Object.assign(lib.described(target), {
                viewAllowed: lib.holdsAll(target.viewPermission),
                importAllowed: target.targetMode !== "wizard" && lib.holdsAll(target.importPermission),
                wizardHere: target.targetMode !== "wizard"
            })
        )
    }
}

// ===== the steps =====
// One of the five steps of an import: the owner's for a steps target, the batch's for a rows target.
function dispatch(stepName, p) {
    const target = importTarget(p.targetId)
    if (target.targetMode === "wizard") throw new Error(lineText(target.title) + " is imported in its owner's own wizard")
    if (stepName !== "get") requireImport(target)
    else lib.signedIn()
    if (target.targetMode === "rows") return Object.assign({targetId: target.targetId}, batches[stepName](target, p))
    return Object.assign(lib.step(target, stepName, stepParameters(p)), {targetId: target.targetId})
}
const stageImport = p => dispatch("stage", p)
const configureImport = p => dispatch("configure", p)
const validateImport = p => dispatch("validate", p)
const applyImport = p => dispatch("apply", p)
// get#Transfer: one import as its target answers it, with a page of its rows.
const getTransfer = p => dispatch("get", Object.assign({}, p, {operationKey: text(p.operationKey) || text(p.transferId)}))

// get#ImportReport and get#ImportSample: a file of an import's rows with their results, and an empty file of a target's
// fields.
function getImportReport(p) {
    lib.signedIn()
    const target = importTarget(p.targetId)
    if (target.targetMode === "rows") return batches.report(target, p)
    if (target.targetMode !== "steps" || !lib.hasStep(target, "report"))
        throw new Error(lineText(target.title) + " has no result file")
    const out = lib.step(target, "report", {operationKey: p.operationKey, errorsOnly: p.errorsOnly === true})
    return {
        reportCsv: out.reportCsv == null ? null : String(out.reportCsv),
        filename: lineText(out.filename, 80).replace(/[^A-Za-z0-9._-]/g, "_") || "import.csv"
    }
}
function getImportSample(p) {
    lib.signedIn()
    const target = importTarget(p.targetId)
    requireImport(target)
    if (target.targetMode === "rows") return batches.sample(target)
    if (target.targetMode !== "steps" || !lib.hasStep(target, "sample"))
        throw new Error(lineText(target.title) + " has no sample file")
    const out = lib.step(target, "sample", {kindId: p.kindId, organizationPartyId: p.organizationPartyId})
    return {
        reportCsv: out.reportCsv == null ? null : String(out.reportCsv),
        filename: lineText(out.filename, 80).replace(/[^A-Za-z0-9._-]/g, "_") || "sample.csv"
    }
}

// ===== retry =====
// retry#ImportItems: only rows that failed, each on its own. A batch here retries its rows itself. For an owner's batch the
// rows chosen must read as failed there now; the owner then validates the import again and applies only those rows, so a
// row that was applied is never applied again (the owner's apply takes only valid rows of the ones named).
function retryImportItems(p) {
    const target = importTarget(p.targetId)
    if (target.targetMode === "wizard") throw new Error(lineText(target.title) + " is retried in its owner's own wizard")
    requireImport(target)
    if (target.targetMode === "rows") return batches.retry(target, p)
    const operationKey = text(p.operationKey)
    const first = lib.step(target, "get", {operationKey, pageIndex: 0})
    if (ec.message.hasError()) return {}
    const size = Math.max(Number(first.pageSize) || 50, 1)
    const pages = Math.ceil((Number(first.rowCount) || 0) / size)
    if (pages > 100)
        throw new Error(
            "This import is too large to retry here: retry it in " + (lineText(target.ownerApp, 60) || "its owner's app")
        )
    const results = new Map()
    for (let pageIndex = 0; pageIndex < pages; pageIndex++) {
        const page = pageIndex === 0 ? first : lib.step(target, "get", {operationKey, pageIndex})
        if (ec.message.hasError()) return {}
        for (const row of lib.list(page.importRows).map(plain)) results.set(Number(row.itemSeqId), text(row.result))
    }
    const given = lib
        .list(
            Array.isArray(p.itemSeqIdList) || p.itemSeqIdList?.toArray
                ? p.itemSeqIdList
                : p.itemSeqIdList == null || p.itemSeqIdList === ""
                  ? []
                  : [p.itemSeqIdList]
        )
        .flatMap(found => String(found).split(/[\s,]+/))
        .filter(Boolean)
        .map(Number)
    if (given.some(id => !Number.isInteger(id) || id < 1)) throw new Error("Choose rows by their numbers")
    const failedStates = ["failed", "stale"]
    const chosen = given.length
        ? Array.from(new Set(given))
        : Array.from(results.entries())
              .filter(([, result]) => failedStates.includes(result))
              .map(([id]) => id)
    if (!chosen.length) return refused("NONE_FAILED", "No failed row to retry")
    if (chosen.length > 200) throw new Error("At most 200 rows at a time")
    const retried = chosen.filter(id => failedStates.includes(results.get(id)))
    const resultOf = (id, after) =>
        !results.has(id)
            ? {itemSeqId: id, applied: false, reasonCode: "NOT_FOUND", reason: "No such row"}
            : !failedStates.includes(results.get(id))
              ? {
                    itemSeqId: id,
                    applied: false,
                    reasonCode: "NOT_FAILED",
                    reason:
                        results.get(id) === "applied"
                            ? "Already applied: it is not applied again"
                            : "Only a failed row is retried"
                }
              : after.get(id)?.result === "applied"
                ? {itemSeqId: id, applied: true, reasonCode: null, reason: null}
                : {
                      itemSeqId: id,
                      applied: false,
                      reasonCode: "FAILED",
                      reason: lineText(after.get(id)?.errorMessage, 250) || "The row was not applied"
                  }
    const after = new Map()
    let state = text(first.state)
    if (retried.length) {
        const validated = lib.step(target, "validate", {operationKey, mappingRevision: first.mappingRevision})
        if (ec.message.hasError()) return {}
        const applied = lib.step(target, "apply", {
            operationKey,
            inputFingerprint: validated.inputFingerprint,
            selectedRowIds: retried
        })
        if (ec.message.hasError()) return {}
        state = text(applied.state) || state
        for (const pageIndex of new Set(retried.map(id => Math.floor((id - 1) / size)))) {
            const page = lib.step(target, "get", {operationKey, pageIndex})
            ec.message.clearErrors()
            for (const row of lib.list(page.importRows).map(plain)) after.set(Number(row.itemSeqId), row)
        }
    }
    const resultList = chosen.map(id => resultOf(id, after))
    const appliedCount = resultList.filter(found => found.applied).length
    return {
        applied: appliedCount > 0,
        reasonCode: appliedCount ? null : resultList.length === 1 ? resultList[0].reasonCode : "NONE_APPLIED",
        reason: appliedCount ? null : resultList.length === 1 ? resultList[0].reason : "None of the rows could be applied",
        operationKey,
        state,
        appliedCount,
        refusedCount: resultList.length - appliedCount,
        resultList
    }
}

// ===== runs =====
// An owner's run as data: its texts cut to a line, its numbers numbers, its parameters for the owner's place names and short
// values only.
function ownerRun(found) {
    const run = plain(found)
    const runId = lineText(run.runId, 120)
    if (!runId) return null
    const count = value => (value == null || !Number.isFinite(Number(value)) ? null : Math.max(Math.trunc(Number(value)), 0))
    const parameters = {}
    for (const [name, value] of Object.entries(plain(run.ownerParameters)))
        if (/^[A-Za-z][A-Za-z0-9]{0,39}$/.test(name) && value != null && Object.keys(parameters).length < 6)
            parameters[name] = lineText(value, 120)
    const state = Object.hasOwn(stateTexts, text(run.state)) ? text(run.state) : "other"
    return {
        runId,
        fileLabel: lineText(run.fileLabel, 120) || null,
        scopeKey: lineText(run.scopeKey, 40) || null,
        scopeTitle: lineText(run.scopeTitle, 100) || null,
        state,
        // shown as a chip: words only
        stateText: lineText(run.stateText, 60).replace(/[^\p{L}\p{N} .,:()-]/gu, "") || stateTexts[state] || "Other",
        startedDate: lib.ms(run.startedDate),
        userName: lineText(run.userName, 100) || null,
        rowCount: count(run.rowCount),
        appliedCount: count(run.appliedCount),
        errorCount: count(run.errorCount),
        skippedCount: count(run.skippedCount),
        detail: false,
        ownerParameters: parameters
    }
}
// find#Transfers: the import runs of every target the caller may view, newest first, a page at a time.
function findTransfers(p) {
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
    const scopeKey = text(p.scopeKey)
    const targets = lib.targetsOf("import")
    const wanted = text(p.targetId)
    if (wanted && !targets.some(target => target.targetId === wanted)) throw new Error("Unknown import target " + wanted)
    const all = []
    const unanswered = []
    for (const target of targets) {
        if ((wanted && target.targetId !== wanted) || (text(p.ownerApp) && text(target.ownerApp) !== text(p.ownerApp))) continue
        // what its registration asks of a reader; its owner's own checks narrow further
        if (!lib.holdsAll(target.viewPermission) && target.targetMode !== "rows") continue
        const described = lib.described(target)
        let found = []
        if (target.targetMode === "rows")
            found = batches
                .runsOf(target, {fromDate: since, thruDate: until, scopeKey, maxRows: perSource})
                .map(run => Object.assign(run, {stateText: stateTexts[run.state] || run.state}))
        else if (lib.hasStep(target, "runs")) {
            const asked = lib.stepApart(target, "runs", {
                targetId: target.targetId,
                scopeKey: scopeKey || null,
                fromDate: since,
                thruDate: until,
                maxRows: perSource
            })
            if (asked.failure) {
                ec.logger.error("Transfer target " + target.targetId + ": its owner did not answer its runs: " + asked.failure)
                unanswered.push({targetId: target.targetId, title: described.title, ownerApp: described.ownerApp})
                continue
            }
            found = lib
                .list(asked.out.runList)
                .slice(0, perSource)
                .map(ownerRun)
                .filter(Boolean)
                .filter(run => run.startedDate == null || (run.startedDate >= since && run.startedDate < until))
                .filter(run => !scopeKey || run.scopeKey === scopeKey)
                // an owner that keeps the import contract's batch answers its detail here
                .map(run => Object.assign(run, {detail: target.targetMode === "steps"}))
        }
        for (const run of found)
            all.push(
                Object.assign(run, {
                    targetId: target.targetId,
                    targetTitle: described.title,
                    ownerApp: described.ownerApp,
                    ownerRoute: described.ownerRoute,
                    startedText: zoned(run.startedDate)
                })
            )
    }
    const matching = all
        .filter(run => !state || run.state === state)
        .sort((a, b) => (b.startedDate ?? 0) - (a.startedDate ?? 0) || String(a.runId).localeCompare(String(b.runId)))
    return {
        transferList: matching.slice(pageIndex * pageSize, (pageIndex + 1) * pageSize),
        transferCount: matching.length,
        pageIndex,
        pageSize,
        unansweredTargetList: unanswered,
        stateList: Object.entries(stateTexts).map(([value, label]) => ({state: value, text: label})),
        defaultDays,
        rangeDays,
        timeZone: zone()
    }
}

const operations = {
    findImportTargets,
    getImportTarget,
    stageImport,
    configureImport,
    validateImport,
    applyImport,
    getTransfer,
    getImportReport,
    getImportSample,
    retryImportItems,
    findTransfers,
    callTargetService: lib.callTargetService,
    applyTransferItem: batches.applyItem,
    failTransferItem: batches.failItem,
    cleanTransferBatches: batches.clean
}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown data transfer operation " + op)
        return operations[op](parameters)
    }
})
