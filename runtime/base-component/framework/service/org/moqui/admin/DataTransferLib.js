// What the parts of org.moqui.admin.DataTransferServices share (mr-boss plan/design-admin.md §11.1, §14 "Import/export
// target", §17.2; impl-admin P15 and later): each part is a module of its own beside this one (DataTransferImports.js the
// dispatch of an import to its registered target, DataTransferBatch.js the batch of a target that has none of its owner's;
// exports add theirs). A target is a moqui.server.TransferTarget its owner seeds: nothing is staged, applied or listed
// without one, and no service, entity or field name comes from a request. An owner's service is called through
// call#TargetService, by the step's name in the registration, as the caller: that service's ALL_USERS ALLOW is not
// inheritable, so the dispatching service's own grant does not carry into the owner's service, which is authorized for the
// caller as a direct call is (below an app screen its inheritable ALWAYS grant covers it, as everything there: an owner
// registers only services that check the caller's permission and scope themselves). The registration adds no right. ADMIN_TRANSFER_VIEW reads the catalog of targets and every
// target's runs; a target's own permissions and its owner's checks decide what is staged and applied. A refusal a person can
// act on is answered as a reason (refused true, reasonCode, reason), never written. No dependency on Mantle or an app.
//
// A target's direction is import or export (impl-admin P16). An export target's mode is file (its owner writes a file of its
// own format), rows (its owner answers rows and the file is written here, a formula cell as text) or wizard (its owner's own
// place, only listed); its fields are the registration's, each with the permission that reads it, and its owner's one
// service is the export step.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/OperationLib.js")
const {rows, one, map, text} = lib
const VIEW = "ADMIN_TRANSFER_VIEW"
const RUN = "ADMIN_TRANSFER_RUN"
const TARGET = "moqui.server.TransferTarget"
const directions = ["import", "export"]

const requireView = () =>
    ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationLib.js").requireView(VIEW, RUN)
const sha = value => ec.resource.digestText(String(value), "sha256")
// Text of an owner's or a file's that is shown or stored as one line: no control character, cut to a length.
const lineText = (value, maxLength = 255) =>
    text(value)
        .replace(/[\u0000-\u001f\u007f]+/g, " ")
        .replace(/^((Type|Reference)?Error:\s*)+/, "")
        .slice(0, maxLength)
const list = value =>
    value == null ? [] : Array.isArray(value) ? value : value.toArray ? Array.from(value.toArray()) : Array.from(value)
// A map a caller or an owner gave, as a plain object of its own names (never a prototype's).
function plain(value) {
    if (value == null || typeof value !== "object") return {}
    const source = value.getMap ? value.getMap() : value
    const out = {}
    for (const name of Object.keys(source)) out[name] = source[name]
    return out
}
const names = value => text(value).split(",").map(text).filter(Boolean)
// Whether the caller holds every permission of a comma separated list (an empty list holds).
const holdsAll = permissions => names(permissions).every(permission => ec.user.hasPermission(permission))
function json(value, fallback) {
    try {
        const found = JSON.parse(text(value) || "null")
        return found ?? fallback
    } catch (error) {
        return fallback
    }
}

// ===== the contracts =====
// The parameters an owner's service must carry to be called for a step (an implementing service gets them from the
// interface), and the only parameters it is given. The five steps are Products' (mantle.product.ProductImportServices), so a
// target that follows them registers as it is.
const stepInputs = [
    "operationKey",
    "kindId",
    "organizationPartyId",
    "sourceText",
    "filename",
    "importFile",
    "delimiter",
    "forceNew",
    "updateMode",
    "mappingRevision",
    "mapping",
    "defaults",
    "valueMappings",
    "clearFields",
    "numberFormat",
    "dateFormat",
    "batchOptions",
    "inputFingerprint",
    "selectedRowIds",
    "pageIndex",
    "errorsOnly"
]
const stepContract = {requiredIn: ["operationKey"], requiredOut: ["operationKey", "state"], allowed: stepInputs}
const contracts = {
    stage: Object.assign({field: "stageService"}, stepContract),
    configure: Object.assign({field: "configureService"}, stepContract),
    validate: Object.assign({field: "validateService"}, stepContract),
    apply: Object.assign({field: "applyService"}, stepContract),
    get: Object.assign({field: "getService"}, stepContract),
    report: {
        field: "reportService",
        requiredIn: ["operationKey"],
        requiredOut: ["reportCsv"],
        allowed: ["operationKey", "errorsOnly"]
    },
    sample: {
        field: "sampleService",
        requiredIn: ["kindId"],
        requiredOut: ["reportCsv"],
        allowed: ["kindId", "organizationPartyId"]
    },
    kinds: {field: "kindsService", requiredIn: [], requiredOut: ["kindOptions"], allowed: ["organizationPartyId"]},
    validateRow: {
        field: "validateRowService",
        requiredIn: ["targetId", "rowValues"],
        requiredOut: ["valid", "normalizedValues"],
        allowed: ["targetId", "scopeKey", "rowValues", "batchOptions", "fixedValues", "operationKey"]
    },
    applyRow: {
        field: "applyRowService",
        requiredIn: ["targetId", "rowValues", "operationKey"],
        requiredOut: ["applied", "resultRef"],
        allowed: ["targetId", "scopeKey", "rowValues", "batchOptions", "fixedValues", "operationKey", "sourceRevision"]
    },
    scopes: {field: "scopeService", requiredIn: ["targetId"], requiredOut: ["scopeList"], allowed: ["targetId"]},
    runs: {
        field: "findService",
        requiredIn: ["targetId"],
        requiredOut: ["runList"],
        allowed: ["targetId", "scopeKey", "fromDate", "thruDate", "maxRows"]
    },
    // an export target's one service (DataTransferServices.run#TargetExport): the scope, the registered selection and the
    // fields the caller may read and chose; nothing else of a request reaches it
    export: {
        field: "exportService",
        requiredIn: ["targetId", "fieldList"],
        requiredOut: ["rowCount"],
        allowed: ["targetId", "scopeKey", "selection", "fieldList", "countOnly", "maxRows", "fixedValues"]
    }
}
const modeSteps = {
    steps: {required: ["stage", "configure", "validate", "apply", "get"], optional: ["report", "sample", "kinds", "runs"]},
    rows: {required: ["validateRow", "applyRow"], optional: ["scopes"]},
    wizard: {required: [], optional: ["runs"]}
}
// What an export target registers, by its mode.
const exportModeSteps = {
    file: {required: ["export"], optional: ["scopes"]},
    rows: {required: ["export"], optional: ["scopes"]},
    wizard: {required: [], optional: []}
}
// The steps of a registration's direction, by mode: an import target has no export step and an export target no other.
const stepsFor = target => (text(target.direction) === "export" ? exportModeSteps : modeSteps)
// Why a registered service cannot be called for a step, or "".
function contractProblem(serviceName, contract) {
    const definition = ec.service.getServiceDefinition(serviceName)
    if (!definition) return "names a service that is not defined (" + serviceName + ")"
    const inNames = Array.from(definition.inParameterNames || [])
    if (
        !contract.requiredIn.every(name => inNames.includes(name)) ||
        !contract.requiredOut.every(name => Object.hasOwn(definition.outParameterInfoMap || {}, name))
    )
        return "names a service without its step's parameters (" + serviceName + ")"
    return ""
}

// ===== registrations =====
// A registration as it is used: checked when read, and one with an unknown mode, a missing step, a service that is not
// defined or lacks its contract, or columns that do not read, is logged by name and left out.
function checked(row) {
    const target = map(row)
    const mode = text(target.targetMode)
    const exporting = text(target.direction) === "export"
    const table = stepsFor(target)
    const steps = table[mode]
    let problem = !directions.includes(text(target.direction))
        ? "names a direction that is not " + directions.join(" or ")
        : !Object.hasOwn(table, mode)
          ? "names a mode that is not " + Object.keys(table).join(", ")
          : ""
    if (!problem)
        for (const step of steps.required.concat(steps.optional)) {
            const serviceName = text(target[contracts[step].field])
            if (!serviceName) {
                if (steps.required.includes(step)) problem = "names no " + step + " service"
            } else problem = contractProblem(serviceName, contracts[step])
            if (problem) break
        }
    let columns = []
    let options = []
    let fields = []
    if (!problem && (mode === "rows" || (exporting && mode === "file"))) {
        columns = list(json(target.columnsJson, [])).filter(column => /^[A-Za-z][A-Za-z0-9]{0,39}$/.test(text(column?.name)))
        options = list(json(target.optionsJson, [])).filter(option => /^[A-Za-z][A-Za-z0-9]{0,39}$/.test(text(option?.name)))
        if (exporting) {
            fields = exportFields(target, columns)
            columns = []
            if (!fields.length) problem = "names no fields"
        } else if (!columns.length) problem = "names no columns"
    }
    for (const permission of exporting && !problem ? names(target.exportPermission) : [])
        if (!one("moqui.security.UserPermission", {userPermissionId: permission}))
            problem = "names the unknown permission " + permission
    if (problem) {
        ec.logger.error("Transfer target " + target.targetId + " " + problem + "; left out")
        return null
    }
    return Object.assign(target, {
        targetMode: mode,
        fields,
        // an import column naming a permission (comma separated, every one needed) is shown only to a holder and cleared from
        // what was staged of a row once it is applied (DataTransferBatch.js, restricted columns); an unknown permission is held
        // by nobody
        columns: columns.map(column => ({
            name: text(column.name),
            label: lineText(column.label, 60) || text(column.name),
            required: column.required === true,
            permission: names(column.permission)
                .filter(permission => /^[A-Za-z0-9_.-]{1,60}$/.test(permission))
                .join(",")
        })),
        options: options.map(option => ({
            name: text(option.name),
            label: lineText(option.label, 60) || text(option.name),
            type: ["integer", "option"].includes(option.type) ? option.type : "text",
            min: Number.isFinite(Number(option.min)) ? Number(option.min) : null,
            max: Number.isFinite(Number(option.max)) ? Number(option.max) : null,
            maxLength: Math.min(Number(option.maxLength) || 255, 2000),
            multiline: option.multiline === true,
            options: list(option.options).map(choice => ({value: text(choice?.value), label: lineText(choice?.label, 60)})),
            defaultValue: option.default == null ? "" : text(option.default)
        })),
        fixedValues: plain(json(target.fixedValuesJson, {})),
        // the registration's revision: a preview made under another is stale
        revision: sha(
            JSON.stringify(
                Object.entries(target)
                    .filter(([name]) => name !== "lastUpdatedStamp")
                    .map(([name, value]) => [name, value == null ? null : String(value)])
                    .sort((a, b) => a[0].localeCompare(b[0]))
            )
        ).slice(0, 24)
    })
}
// The fields of an export target as its registration names them (columnsJson: name, label, permission, required,
// selected): permission is what a caller must hold to have the field written, comma separated, none for a field every
// exporter of the target reads; required is written whenever it may be read; selected false is left out unless asked for. A
// field naming a permission that does not exist is logged and left out (so nobody is given it).
function exportFields(target, columns) {
    const found = []
    for (const column of columns) {
        const name = text(column.name)
        if (found.some(field => field.name === name)) continue
        const permissions = names(column.permission)
        const unknown = permissions.find(
            permission =>
                !/^[A-Za-z0-9_.-]{1,60}$/.test(permission) ||
                !one("moqui.security.UserPermission", {userPermissionId: permission})
        )
        if (unknown) {
            ec.logger.error(
                "Transfer target " +
                    target.targetId +
                    " field " +
                    name +
                    " names the unknown permission " +
                    unknown +
                    "; left out"
            )
            continue
        }
        found.push({
            name,
            label: lineText(column.label, 60) || name,
            permission: permissions.join(","),
            required: column.required === true,
            selected: column.selected !== false,
            sensitive: permissions.length > 0
        })
    }
    return found
}
function targetsOf(direction) {
    return rows(TARGET, {direction})
        .filter(row => row.disabled !== "Y")
        .map(checked)
        .filter(Boolean)
        .sort((a, b) => Number(a.sequenceNum ?? 9999) - Number(b.sequenceNum ?? 9999) || a.targetId.localeCompare(b.targetId))
}
// The registered target of an id a caller gave, or an error: an unknown id, a disabled or a malformed registration alike.
function targetOf(targetId, direction) {
    const id = text(targetId)
    const row = /^[A-Za-z0-9_.-]{1,40}$/.test(id) ? one(TARGET, {targetId: id}) : null
    const target = row && row.disabled !== "Y" && text(row.direction) === direction ? checked(row) : null
    if (!target) throw new Error("Unknown " + direction + " target " + id)
    return target
}

// ===== calling an owner =====
// call#TargetService: the service a registration names for a step, with the step's parameters only, under the caller's own
// authorization. The step is one of the contracts above and the service is read from the registration here, never given.
function callTargetService(p) {
    lib.signedIn()
    const step = text(p.step)
    if (!Object.hasOwn(contracts, step)) throw new Error("Unknown transfer step " + step)
    const id = text(p.targetId)
    const row = one(TARGET, {targetId: id})
    const target = row && row.disabled !== "Y" ? checked(row) : null
    if (!target) throw new Error("Unknown transfer target " + id)
    const contract = contracts[step]
    const steps = stepsFor(target)[target.targetMode]
    const serviceName = text(target[contract.field])
    if (!serviceName || !steps.required.concat(steps.optional).includes(step))
        throw new Error(lineText(target.title) + " registers no " + step + " step")
    const declared = Array.from(ec.service.getServiceDefinition(serviceName).inParameterNames || [])
    const given = plain(p.stepParameters)
    const parameters = {}
    for (const name of contract.allowed)
        if (Object.hasOwn(given, name) && given[name] != null && declared.includes(name)) parameters[name] = given[name]
    // what the registration fixes for its row services is the registration's, never the request's
    if (contract.allowed.includes("fixedValues") && declared.includes("fixedValues")) parameters.fixedValues = target.fixedValues
    return {result: ec.service.sync().name(serviceName).parameters(parameters).call() || {}}
}
// A step of a target for a dispatching service: the owner's answer, its errors left on the request for the caller.
function step(target, stepName, stepParameters) {
    const out = ec.service
        .sync()
        .name("org.moqui.admin.DataTransferServices.call#TargetService")
        .parameters({targetId: target.targetId, step: stepName, stepParameters})
        .call()
    return plain(out?.result)
}
// A step apart from the caller's transaction and errors: {out} its answer, or {failure} why it has none (never thrown, so
// one row or one owner that fails does not stop the others).
function stepApart(target, stepName, stepParameters) {
    const asked = lib.callRegistered("org.moqui.admin.DataTransferServices.call#TargetService", {
        targetId: target.targetId,
        step: stepName,
        stepParameters
    })
    return asked.failure ? {failure: lineText(asked.failure, 500)} : {out: plain(asked.out?.result)}
}
const hasStep = (target, stepName) => !!text(target[contracts[stepName].field])

// ===== delimited text =====
// A bounded delimited file: quotes may hold separators, doubled quotes and line ends. {headers, rows: [{rowNumber,
// sourceValues, sourceError}]}; a problem with the whole file is an Error whose message a person can act on.
function parseDelimited(source, delimiter, bounds) {
    const content = String(source || "").replace(/^﻿/, "")
    if (!content.trim()) throw new Error("The file is empty")
    if (content.includes("\0")) throw new Error("The file is not text: save it as UTF-8 CSV")
    const records = []
    let row = []
    let value = ""
    let quoted = false
    let closed = false
    const cell = () => {
        row.push(value)
        value = ""
        closed = false
    }
    const line = () => {
        cell()
        if (row.some(found => found !== "")) records.push(row)
        row = []
        if (records.length > bounds.maxRows + 1) throw new Error("Import at most " + bounds.maxRows + " rows at a time")
    }
    for (let index = 0; index < content.length; index++) {
        const character = content[index]
        if (quoted) {
            if (character === '"') {
                if (content[index + 1] === '"') {
                    value += '"'
                    index++
                } else {
                    quoted = false
                    closed = true
                }
            } else value += character
            continue
        }
        if (character === '"') {
            if (value || closed) throw new Error("A quote must begin a cell (line " + (records.length + 1) + ")")
            quoted = true
        } else if (character === delimiter) cell()
        else if (character === "\r" || character === "\n") {
            if (character === "\r" && content[index + 1] === "\n") index++
            line()
        } else {
            if (closed && character !== " " && character !== "\t")
                throw new Error("Text follows a quoted cell (line " + (records.length + 1) + ")")
            if (!closed) value += character
        }
    }
    if (quoted) throw new Error("A quoted cell is not closed")
    if (value || row.length || closed) line()
    if (records.length < 2) throw new Error("Include a header line and at least one row")
    const headers = records.shift().map(text)
    if (headers.length > bounds.maxColumns) throw new Error("Use at most " + bounds.maxColumns + " columns")
    if (
        headers.some(header => !header || header.length > 100 || /[\u0000-\u001f\u007f]/.test(header)) ||
        new Set(headers).size !== headers.length
    )
        throw new Error("Give each column a distinct header of 1 to 100 characters")
    return {
        headers,
        rows: records.map((cells, index) => ({
            rowNumber: index + 2,
            sourceValues: Object.fromEntries(headers.map((header, column) => [header, cells[column] ?? ""])),
            sourceError:
                cells.length > headers.length
                    ? "This row has more cells than the header"
                    : cells.some(found => found.length > bounds.maxCell)
                      ? "A cell is longer than " + bounds.maxCell + " characters"
                      : null
        }))
    }
}
// Whether bytes are UTF-8 text.
function validUtf8(bytes) {
    for (let index = 0; index < bytes.length; index++) {
        const byte = bytes[index] & 255
        if (byte === 0) return false
        if (byte < 128) continue
        let count
        let low = 128
        let high = 191
        if (byte >= 194 && byte <= 223) count = 1
        else if (byte >= 224 && byte <= 239) {
            count = 2
            if (byte === 224) low = 160
            if (byte === 237) high = 159
        } else if (byte >= 240 && byte <= 244) {
            count = 3
            if (byte === 240) low = 144
            if (byte === 244) high = 143
        } else return false
        if (index + count >= bytes.length) return false
        const second = bytes[index + 1] & 255
        if (second < low || second > high) return false
        for (let next = 2; next <= count; next++) {
            const more = bytes[index + next] & 255
            if (more < 128 || more > 191) return false
        }
        index += count
    }
    return true
}
// A cell of a file people open in a spreadsheet: one that would be read as a formula (=, +, -, @, a tab or a line start)
// is written as text; a plain number (-10.00, +1, -1,234.50) is written as it is, as the screen CSV writer does
// (template/screen/ScreenCsv.js).
function delimitedCell(value) {
    let found = String(value ?? "")
    if (/^[=+\-@\t\r]/.test(found) && !/^[+-]?[\d,]*\.?\d+$/.test(found)) found = "'" + found
    return '"' + found.replace(/"/g, '""') + '"'
}
function writeDelimited(headers, records) {
    return (
        "﻿" +
        headers.map(delimitedCell).join(",") +
        "\r\n" +
        records.map(record => headers.map(header => delimitedCell(record[header])).join(",")).join("\r\n") +
        "\r\n"
    )
}

// ===== what a target is, for a reader =====
// A registration as data: never a service name.
function described(target) {
    return {
        targetId: target.targetId,
        title: lineText(target.title) || target.targetId,
        description: lineText(target.description) || null,
        effectsNote: lineText(target.effectsNote) || null,
        ownerApp: lineText(target.ownerApp, 60) || null,
        targetVersion: lineText(target.targetVersion, 20) || null,
        targetMode: target.targetMode,
        fieldLabel: lineText(target.fieldLabel, 40) || "Target field",
        scopeLabel: lineText(target.scopeLabel, 40) || null,
        ownerRoute: /^[A-Za-z][A-Za-z0-9]*$/.test(text(target.ownerRoute)) ? text(target.ownerRoute) : null,
        runsKept: target.targetMode === "rows" || hasStep(target, "runs"),
        importPermission: text(target.importPermission) || null,
        exportPermission: text(target.exportPermission) || null,
        viewPermission: text(target.viewPermission) || null
    }
}

;({
    ...lib,
    VIEW,
    RUN,
    TARGET,
    requireView,
    sha,
    lineText,
    list,
    plain,
    names,
    holdsAll,
    json,
    contracts,
    stepInputs,
    targetsOf,
    targetOf,
    callTargetService,
    step,
    stepApart,
    hasStep,
    parseDelimited,
    validUtf8,
    delimitedCell,
    writeDelimited,
    described
})
