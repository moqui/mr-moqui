// The batch part of org.moqui.admin.DataTransferServices (mr-boss plan/design-admin.md §11.1, DataImport and ImportDetail;
// impl-admin P15): the import of a target that has no batch of its owner's (moqui.server.TransferTarget, targetMode rows),
// kept in TransferBatch and TransferItem and answering the same contract an owner's five steps answer (operationKey,
// mappingRevision, inputFingerprint, importRows and the counts), so one wizard serves both.
//
// What a file may hold. Delimited UTF-8 text within the target's bounds (bytes, rows; 50 columns, 2000 characters a cell):
// anything else is refused before it is kept. A file with a column named like a secret (a password, a token, a factor, a
// hash) or like one the target refuses is refused whole, with the column named: such a column is never read, mapped or
// stored. A row carries only the target's registered fields, by a mapping from the file's headers; nothing else of a row is
// given to the owner's services.
//
// Who reads a batch. The user who staged it, while they hold the target's import permission (or its view permission) and
// its scope; and a holder of the target's view permission whom the target's scope service admits to the batch's scope. Only
// the user who staged it changes or applies it, while they still hold the target's import permission and its scope: every row
// is applied under that user's own authority. Anyone else is told it is not available.
//
// Revisions. A mapping change names the mapping revision it read. A validation changes nothing of the owner's: each row is
// given to the target's validateRowService, and its answer (the values an apply would be given, the source's revision) is
// kept. The validation's fingerprint covers the content's hash, the mapping, the options, the registration's revision and
// every row's preview: an apply names it and is refused when anything of it changed (STALE_PREVIEW).
//
// Applying. Each valid row is on its own: locked, validated again (a source that changed since the preview is not written:
// STALE_SOURCE), then given to the target's applyRowService apart from the others, under an operation key made of the
// batch, the row and its values, so an owner that honors the key answers a repeat with what it made. An applied row is never
// applied again: a repeat of the apply, or a resumed one after a stop, takes only the rows that are still valid, and a
// retry only the rows that failed.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/DataTransferLib.js")
const {find, one, rows, map, text, refused, lineText, plain, sha} = lib
const BATCH = "moqui.server.TransferBatch"
const ITEM = "moqui.server.TransferItem"
const S = "org.moqui.admin.DataTransferServices."
const pageSize = 50
const hardBytes = 2 * 1024 * 1024
const hardRows = 5000
const maxRetry = 200
const secretParts = ["password", "passwd", "passphrase", "secret", "token", "apikey", "privatekey", "credential", "totp"]
const secretWords = ["pwd", "otp", "pin", "hash", "salt", "factor", "mfa"]
const notAvailable = () => new Error("This import is not available")
const refusal = (reasonCode, reason, extra = {}) => Object.assign({refused: true, reasonCode, reason}, extra)

const boundsOf = target => ({
    maxBytes: Math.min(Number(target.maxBytes) || 1024 * 1024, hardBytes),
    maxRows: Math.min(Number(target.maxRows) || 2000, hardRows),
    maxColumns: 50,
    maxCell: 2000
})
const itemsOf = transferId => rows(ITEM, {transferId}).sort((a, b) => Number(a.itemSeqId) - Number(b.itemSeqId))
const ms = lib.ms

// ===== scope and access =====
// The scopes the target's scope service gives the caller: {scopeList} or {failure}; a target with no scope service has none.
function scopesOf(target) {
    if (!lib.hasStep(target, "scopes")) return {scopeList: [], unscoped: true}
    const asked = lib.stepApart(target, "scopes", {targetId: target.targetId})
    if (asked.failure) return {failure: asked.failure, scopeList: []}
    const scopeList = []
    for (const found of lib.list(asked.out.scopeList)) {
        const scope = plain(found)
        const scopeKey = text(scope.scopeKey)
        if (!/^[A-Za-z0-9_.:-]{1,40}$/.test(scopeKey) || scopeList.some(existing => existing.scopeKey === scopeKey)) continue
        scopeList.push({scopeKey, title: lineText(scope.title, 100) || scopeKey})
    }
    return {scopeList}
}
// A batch the caller may read (need read) or change and apply (need edit); anything else is not available, whatever the
// reason. With lock the row is locked for the transaction.
function batchOf(target, operationKey, need, lock = false) {
    const transferId = text(operationKey)
    if (!transferId) throw notAvailable()
    const row = lock ? find(BATCH, {transferId}).forUpdate(true).one() : one(BATCH, {transferId})
    if (!row || row.targetId !== target.targetId) throw notAvailable()
    const own = row.userId === ec.user.userId
    const scopeKey = text(row.scopeKey)
    let admitted = null
    const scoped = () => {
        if (admitted == null) {
            const found = scopesOf(target)
            admitted = scopeKey ? found.scopeList.some(scope => scope.scopeKey === scopeKey) : !!found.unscoped
        }
        return admitted
    }
    const canEdit = own && lib.holdsAll(target.importPermission) && scoped()
    if (need === "edit" && !canEdit) throw notAvailable()
    // who staged it reads it as anyone else does once they no longer hold the target's import permission: with its view
    // permission and its scope (the rows of a file are people's names and addresses, and results of the owner's)
    if (need === "read" && !canEdit && !((own || text(target.viewPermission)) && lib.holdsAll(target.viewPermission) && scoped()))
        throw notAvailable()
    return {row: map(row), request: lib.json(row.requestJson, {}), canEdit, own}
}

// ===== the request =====
function refusedColumn(target, headers) {
    const refusedParts = secretParts.concat(
        lib
            .names(target.refusedColumns)
            .map(word => word.toLowerCase().replace(/[^a-z]/g, ""))
            .filter(Boolean)
    )
    for (const header of headers) {
        const words = header
            .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
            .toLowerCase()
            .split(/[^a-z]+/)
            .filter(Boolean)
        const joined = words.join("")
        if (refusedParts.some(part => joined.includes(part)) || words.some(word => secretWords.includes(word))) return header
    }
    return null
}
const fieldKey = value =>
    text(value)
        .toLowerCase()
        .replace(/[^a-z0-9]/g, "")
// The batch's options as the target registers them: a value of a registered option only, of its type and within its
// bounds; an option not given keeps its current value or its default. An Error names what is wrong.
function optionsOf(target, given, current = {}) {
    const out = {}
    for (const option of target.options) {
        let value = Object.hasOwn(given, option.name)
            ? given[option.name]
            : Object.hasOwn(current, option.name)
              ? current[option.name]
              : option.defaultValue
        value = value == null ? "" : String(value)
        if (option.type === "integer") {
            if (value.trim() === "") value = option.defaultValue
            if (
                value !== "" &&
                (!/^-?\d{1,9}$/.test(value.trim()) ||
                    (option.min != null && Number(value) < option.min) ||
                    (option.max != null && Number(value) > option.max))
            )
                throw new Error(
                    option.label +
                        " is a whole number" +
                        (option.min != null && option.max != null ? " from " + option.min + " to " + option.max : "")
                )
            value = value === "" ? "" : String(Number(value))
        } else if (option.type === "option") {
            if (value.trim() === "") value = option.defaultValue
            if (value !== "" && !option.options.some(choice => choice.value === value))
                throw new Error(option.label + " is not one of its options")
        } else {
            value = option.multiline ? value.replace(/\r\n?/g, "\n").trim() : value.trim()
            if (
                (option.multiline ? /[\u0000-\u0008\u000b-\u001f\u007f]/ : /[\u0000-\u001f\u007f]/).test(value) ||
                value.includes("${")
            )
                throw new Error(option.label + " may not hold control characters or ${")
            if (value.length > option.maxLength)
                throw new Error(option.label + " is longer than " + option.maxLength + " characters")
        }
        out[option.name] = value
    }
    return out
}
const mappingRevision = (row, request, target) =>
    sha(
        JSON.stringify({
            targetId: target.targetId,
            fileHash: row.fileHash,
            mapping: request.mapping,
            defaults: request.defaults,
            valueMappings: request.valueMappings,
            batchOptions: request.batchOptions,
            targetRevision: target.revision
        })
    ).slice(0, 32)
function fingerprintOf(revision, items) {
    return sha(
        JSON.stringify({
            revision,
            items: items.map(item => [Number(item.itemSeqId), item.result, item.previewJson || null])
        })
    ).slice(0, 40)
}
function write(entityName, fields) {
    lib.write(entityName, fields, false)
}

// ===== stage =====
function stage(target, p) {
    lib.signedIn()
    for (const permission of lib.names(target.importPermission))
        if (!ec.user.hasPermission(permission)) throw new Error(permission + " is required")
    const bounds = boundsOf(target)
    const file = p.importFile
    let source
    if (file && Number(file.size) > 0) {
        if (Number(file.size) > bounds.maxBytes)
            return refusal("FILE_TOO_LARGE", "The file is larger than " + Math.floor(bounds.maxBytes / 1024) + " KiB")
        const stream = file.getInputStream()
        let ok
        try {
            ok = lib.validUtf8(stream.readAllBytes())
        } finally {
            stream.close()
        }
        // by its content, whatever its name or declared type says
        if (!ok) return refusal("FILE_NOT_TEXT", "The file is not UTF-8 text: save it as CSV (UTF-8)")
        source = file.getString("UTF-8")
    } else source = String(p.sourceText || "")
    if (source.length > bounds.maxBytes)
        return refusal("FILE_TOO_LARGE", "The file is larger than " + Math.floor(bounds.maxBytes / 1024) + " KiB")
    const delimiter = p.delimiter === "tab" ? "\t" : text(p.delimiter) || ","
    if (![",", ";", "\t"].includes(delimiter)) throw new Error("Choose comma, semicolon or tab")
    let parsed
    try {
        parsed = lib.parseDelimited(source, delimiter, bounds)
    } catch (error) {
        return refusal("FILE_UNREADABLE", String(error?.message ?? error))
    }
    const secret = refusedColumn(target, parsed.headers)
    if (secret)
        return refusal(
            "COLUMN_REFUSED",
            'The column "' +
                lineText(secret, 60) +
                '" is not imported here: passwords, factors, tokens and what this import refuses are never read from a file. Remove the column and stage the file again.'
        )
    // the scope the import is for: one the target's scope service gives the caller, or none for a target without one
    const scopeKey = text(p.organizationPartyId)
    const scopes = scopesOf(target)
    if (scopes.failure) throw new Error("The scopes of this import could not be read. Try again later.")
    if (scopes.unscoped ? !!scopeKey : !scopes.scopeList.some(scope => scope.scopeKey === scopeKey))
        throw new Error(
            scopes.unscoped
                ? "This import takes no scope"
                : "Choose " + (lineText(target.scopeLabel, 40) || "a scope").toLowerCase() + " you may import into"
        )
    const batchOptions = optionsOf(target, plain(p.batchOptions))
    const fileHash = sha(delimiter + "\n" + source)
    const givenKey = lib.operationKey(p)
    const transferId =
        givenKey ||
        "transfer-" +
            sha(
                [ec.user.userId, target.targetId, scopeKey, fileHash, p.forceNew === true ? ec.user.randomString(20) : ""].join(
                    "|"
                )
            ).slice(0, 32)
    const earlier = one(BATCH, {transferId})
    if (earlier) {
        // the key is one import's: the same user, target, scope and content answer it; anything else under it is refused
        if (
            earlier.userId !== ec.user.userId ||
            earlier.targetId !== target.targetId ||
            text(earlier.scopeKey) !== scopeKey ||
            earlier.fileHash !== fileHash
        )
            return refusal("OPERATION_KEY_CONFLICT", "The operation key was used for another import or other content")
        return Object.assign(get(target, {operationKey: transferId}), {repeated: true})
    }
    const byKey = new Map(
        target.columns.flatMap(column => [
            [fieldKey(column.name), column.name],
            [fieldKey(column.label), column.name]
        ])
    )
    const taken = new Set()
    const mapping = {}
    for (const header of parsed.headers) {
        const name = byKey.get(fieldKey(header)) || null
        mapping[header] = name && !taken.has(name) ? name : null
        if (name) taken.add(name)
    }
    const request = {headers: parsed.headers, delimiter, mapping, defaults: {}, valueMappings: {}, batchOptions}
    lib.write(
        BATCH,
        {
            transferId,
            targetId: target.targetId,
            scopeKey: scopeKey || null,
            userId: ec.user.userId,
            state: "staged",
            filename:
                String(file && Number(file.size) > 0 ? file.getName() : p.filename || "import.csv")
                    .split(/[\\/]/)
                    .at(-1)
                    .replace(/[\u0000-\u001f\u007f]/g, "_")
                    .replace(/\$\{/g, "$_{")
                    .slice(0, 120) || "import.csv",
            fileHash,
            requestJson: JSON.stringify(request),
            rowCount: parsed.rows.length,
            stagedDate: ec.user.nowTimestamp
        },
        true
    )
    parsed.rows.forEach((row, index) =>
        lib.write(
            ITEM,
            {
                transferId,
                itemSeqId: index + 1,
                rowNumber: row.rowNumber,
                sourceJson: JSON.stringify({sourceValues: row.sourceValues, sourceError: row.sourceError}),
                result: "staged",
                attemptCount: 0
            },
            true
        )
    )
    return get(target, {operationKey: transferId})
}

// ===== configure =====
function configure(target, p) {
    const batch = batchOf(target, p.operationKey, "edit", true)
    if (batch.row.purgedDate) return refusal("PURGED", "The staged rows of this import were removed: stage the file again")
    if (!text(p.mappingRevision) || text(p.mappingRevision) !== mappingRevision(batch.row, batch.request, target))
        return refusal("STALE_MAPPING", "The mapping changed since it was read; read it again and repeat the change")
    const fields = new Set(target.columns.map(column => column.name))
    const next = Object.assign({}, batch.request)
    if (p.mapping != null) {
        const given = plain(p.mapping)
        const mapping = {}
        for (const header of batch.request.headers) {
            const field = Object.hasOwn(given, header) ? text(given[header]) : text(batch.request.mapping[header])
            if (field && !fields.has(field)) throw new Error("Map a column to a field of this import")
            mapping[header] = field || null
        }
        if (Object.keys(given).some(header => !batch.request.headers.includes(header)))
            throw new Error("Map the columns of this file")
        const targets = Object.values(mapping).filter(Boolean)
        if (new Set(targets).size !== targets.length) throw new Error("Each field takes one column")
        next.mapping = mapping
    }
    if (p.defaults != null) {
        const defaults = {}
        for (const [field, value] of Object.entries(plain(p.defaults))) {
            if (!fields.has(field)) throw new Error("A default is for a field of this import")
            if (value != null && String(value) !== "") defaults[field] = String(value).slice(0, 2000)
        }
        next.defaults = defaults
    }
    if (p.valueMappings != null) {
        const valueMappings = {}
        for (const [field, values] of Object.entries(plain(p.valueMappings))) {
            if (!fields.has(field)) throw new Error("A value mapping is for a field of this import")
            const found = {}
            for (const [from, to] of Object.entries(plain(values)))
                found[String(from).slice(0, 2000)] = String(to ?? "").slice(0, 2000)
            if (Object.keys(found).length) valueMappings[field] = found
        }
        next.valueMappings = valueMappings
    }
    if (p.batchOptions != null) next.batchOptions = optionsOf(target, plain(p.batchOptions), plain(batch.request.batchOptions))
    write(BATCH, {
        transferId: batch.row.transferId,
        requestJson: JSON.stringify(next),
        inputFingerprint: null,
        state: "mapped",
        lastError: null
    })
    for (const item of itemsOf(batch.row.transferId).filter(found => found.result !== "applied"))
        write(ITEM, {
            transferId: batch.row.transferId,
            itemSeqId: item.itemSeqId,
            previewJson: null,
            result: "staged",
            reasonCode: null,
            errorMessage: null
        })
    return get(target, p)
}

// ===== validate =====
// A row's values by the mapping: the registered fields only, a default where the file has none, a mapped value in place of
// the file's.
function rowValues(target, request, source) {
    if (source.sourceError) throw new Error(source.sourceError)
    const values = {}
    for (const [field, value] of Object.entries(plain(request.defaults)))
        if (target.columns.some(column => column.name === field)) values[field] = String(value)
    for (const header of request.headers) {
        const field = text(request.mapping[header])
        if (!field) continue
        const raw = Object.hasOwn(source.sourceValues, header) ? String(source.sourceValues[header] ?? "") : ""
        if (raw.trim() !== "") values[field] = raw.trim()
    }
    for (const field of Object.keys(values)) {
        const mapped = plain(plain(request.valueMappings)[field])
        if (Object.hasOwn(mapped, values[field])) values[field] = String(mapped[values[field]])
        if (values[field] === "") delete values[field]
    }
    for (const column of target.columns)
        if (column.required && !Object.hasOwn(values, column.name)) throw new Error(column.label + " is required")
    return values
}
// What the target's validator says of a row, as data: {valid, skip, errors, values, warnings, sourceRevision, displayText}.
function previewOf(target, batch, source, operationKey = null) {
    const parameters = {
        targetId: target.targetId,
        scopeKey: text(batch.row.scopeKey) || null,
        rowValues: rowValues(target, batch.request, source),
        batchOptions: plain(batch.request.batchOptions),
        // at an apply, the key the row's apply carries: an owner that already made the row under it still says valid
        operationKey
    }
    const asked = lib.stepApart(target, "validateRow", parameters)
    if (asked.failure) throw new Error(asked.failure)
    const out = asked.out
    const values = {}
    const given = plain(out.normalizedValues)
    for (const column of target.columns)
        if (Object.hasOwn(given, column.name) && given[column.name] != null) values[column.name] = String(given[column.name])
    return {
        valid: out.valid === true,
        skip: out.skip === true,
        errors: lib
            .list(out.errors)
            .map(error => lineText(error, 240))
            .filter(Boolean)
            .slice(0, 5),
        warnings: lib
            .list(out.warnings)
            .map(warning => lineText(warning, 240))
            .filter(Boolean)
            .slice(0, 5),
        values,
        sourceRevision: lineText(out.sourceRevision, 80),
        displayText: lineText(out.displayText, 240)
    }
}
function validate(target, p) {
    const batch = batchOf(target, p.operationKey, "edit", true)
    if (batch.row.purgedDate) return refusal("PURGED", "The staged rows of this import were removed: stage the file again")
    const revision = mappingRevision(batch.row, batch.request, target)
    if (text(p.mappingRevision) !== revision)
        return refusal("STALE_MAPPING", "The mapping changed since it was read; read it again and validate")
    const keyFields = lib.names(target.keyFields)
    const seen = new Map()
    const items = itemsOf(batch.row.transferId)
    // a row applied before keeps its key: a row that repeats it stays a repeat when the rest is validated again
    for (const item of items) {
        const done = item.result === "applied" && keyFields.length ? lib.json(item.previewJson, null)?.values : null
        if (done) seen.set(JSON.stringify(keyFields.map(field => done[field] ?? "")), item.rowNumber)
    }
    for (const item of items) {
        if (item.result === "applied") continue
        const source = lib.json(item.sourceJson, {sourceValues: {}})
        let result = "invalid"
        let preview = null
        let reason = null
        try {
            preview = previewOf(target, batch, source)
            if (preview.skip) {
                result = "skipped"
                reason = preview.warnings[0] || preview.errors[0] || "Nothing to change"
            } else if (!preview.valid) reason = preview.errors.join("; ") || "The row is not valid"
            else {
                const key = keyFields.length ? JSON.stringify(keyFields.map(field => preview.values[field] ?? "")) : null
                if (key && seen.has(key)) reason = "Repeats row " + seen.get(key) + " of this file"
                else {
                    if (key) seen.set(key, item.rowNumber)
                    result = "valid"
                }
            }
        } catch (error) {
            ec.message.clearErrors()
            reason = lineText(String(error?.message ?? error), 250) || "The row could not be validated"
        }
        write(ITEM, {
            transferId: batch.row.transferId,
            itemSeqId: item.itemSeqId,
            previewJson: preview ? JSON.stringify(preview) : null,
            result,
            reasonCode: result === "invalid" ? "INVALID" : null,
            errorMessage: result === "valid" ? null : lineText(reason, 250)
        })
    }
    const next = Object.assign({}, batch.request, {validatedRevision: revision})
    write(BATCH, {
        transferId: batch.row.transferId,
        requestJson: JSON.stringify(next),
        inputFingerprint: fingerprintOf(revision, itemsOf(batch.row.transferId)),
        state: "validated",
        lastError: null
    })
    return get(target, p)
}

// ===== apply =====
const itemKey = (transferId, item, values) =>
    "tr." + sha([transferId, item.itemSeqId, JSON.stringify(values)].join("|")).slice(0, 40)
// apply#TransferItem: one row in a transaction of its own, the row locked. A valid row (or, for a retry, a failed one) is
// validated again and given to the target's apply service only if the source is as the preview read it, at a retry as at
// the apply; a row with nothing left to do is skipped.
function applyItem(p) {
    const target = lib.targetOf(p.targetId, "import")
    const row = one(BATCH, {transferId: text(p.operationKey)})
    if (!row || row.targetId !== target.targetId || row.userId !== ec.user.userId || !lib.holdsAll(target.importPermission))
        throw notAvailable()
    const batch = {row: map(row), request: lib.json(row.requestJson, {})}
    const retry = p.retry === true
    if (!retry && (!text(p.inputFingerprint) || text(p.inputFingerprint) !== text(row.inputFingerprint)))
        throw new Error("The validation changed; review the import again")
    const item = find(ITEM, {transferId: row.transferId, itemSeqId: Number(p.itemSeqId)})
        .forUpdate(true)
        .one()
    if (!item) throw new Error("No such row")
    if (item.result === "applied") return {alreadyApplied: true}
    if (item.result !== (retry ? "failed" : "valid"))
        return {
            applied: false,
            reasonCode: retry ? "NOT_FAILED" : "NOT_VALID",
            reason: retry ? "Only a failed row is retried" : "Only a validated row is applied"
        }
    const mark = fields =>
        write(
            ITEM,
            Object.assign(
                {transferId: row.transferId, itemSeqId: item.itemSeqId, attemptCount: Number(item.attemptCount || 0) + 1},
                fields
            )
        )
    const failed = (reasonCode, reason) => {
        mark({
            result: "failed",
            reasonCode: lineText(reasonCode, 40) || "FAILED",
            errorMessage: lineText(reason, 250) || "The row was not applied"
        })
        return {
            applied: false,
            reasonCode: lineText(reasonCode, 40) || "FAILED",
            reason: lineText(reason, 250) || "The row was not applied"
        }
    }
    const before = lib.json(item.previewJson, null)
    let now
    try {
        now = previewOf(
            target,
            batch,
            lib.json(item.sourceJson, {sourceValues: {}}),
            before?.values ? itemKey(row.transferId, item, before.values) : null
        )
    } catch (error) {
        ec.message.clearErrors()
        return failed("INVALID", String(error?.message ?? error))
    }
    if (!now.valid && now.skip) {
        // what the row would change is already so (another's change, or this row's own apply whose result was not kept)
        const reason = lineText(now.warnings[0] || now.errors[0], 250) || "Nothing to change"
        mark({result: "skipped", reasonCode: null, errorMessage: reason})
        return {applied: false, reasonCode: "NOTHING_TO_DO", reason}
    }
    if (!now.valid) return failed("INVALID", now.errors.join("; ") || now.warnings[0] || "The row is no longer valid")
    // a retry too writes only what its preview showed: a row whose source changed is validated and reviewed again
    if (!before || now.sourceRevision !== before.sourceRevision || JSON.stringify(now.values) !== JSON.stringify(before.values))
        return failed("STALE_SOURCE", "What this row would change was changed since the preview; validate and review it again")
    const operationKey = itemKey(row.transferId, item, now.values)
    const asked = lib.stepApart(target, "applyRow", {
        targetId: target.targetId,
        scopeKey: text(row.scopeKey) || null,
        rowValues: now.values,
        batchOptions: plain(batch.request.batchOptions),
        operationKey,
        sourceRevision: now.sourceRevision || null
    })
    if (asked.failure) return Object.assign(failed("FAILED", asked.failure), {operationKey})
    if (asked.out.applied !== true)
        return failed(text(asked.out.reasonCode) || "REFUSED", asked.out.reason || "Its owner did not take the row")
    mark({
        result: "applied",
        previewJson: JSON.stringify(now),
        reasonCode: null,
        errorMessage: null,
        resultRef: lineText(asked.out.resultRef, 120) || null,
        resultText: lineText(asked.out.resultText, 250) || null,
        operationKey,
        completedDate: ec.user.nowTimestamp
    })
    return {applied: true, resultRef: lineText(asked.out.resultRef, 120) || null}
}
// fail#TransferItem: a row whose own transaction did not end is marked failed, unless it was applied.
function failItem(p) {
    const row = one(BATCH, {transferId: text(p.operationKey)})
    if (!row || row.userId !== ec.user.userId) throw notAvailable()
    const item = find(ITEM, {transferId: row.transferId, itemSeqId: Number(p.itemSeqId)})
        .forUpdate(true)
        .one()
    if (item && item.result !== "applied")
        write(ITEM, {
            transferId: row.transferId,
            itemSeqId: item.itemSeqId,
            result: "failed",
            reasonCode: "FAILED",
            errorMessage: lineText(p.errorMessage, 250) || "The row was not applied",
            attemptCount: Number(item.attemptCount || 0) + 1
        })
    return {}
}
// One row through apply#TransferItem, apart from the others; its result, never thrown.
function runItem(target, transferId, itemSeqId, extra) {
    const asked = lib.callRegistered(
        S + "apply#TransferItem",
        Object.assign({targetId: target.targetId, operationKey: transferId, itemSeqId}, extra)
    )
    if (!asked.failure) return Object.assign({itemSeqId}, asked.out)
    const reason = lineText(asked.failure, 250)
    ec.logger.error("Transfer " + transferId + " row " + itemSeqId + " was not applied: " + reason)
    const marked = lib.callRegistered(S + "fail#TransferItem", {operationKey: transferId, itemSeqId, errorMessage: reason})
    if (marked.failure)
        ec.logger.error("Transfer " + transferId + " row " + itemSeqId + " result was not kept: " + marked.failure)
    return {itemSeqId, applied: false, reasonCode: "FAILED", reason}
}
function selectedIds(value) {
    if (value == null || value === "") return null
    const ids = lib
        .list(Array.isArray(value) || value?.toArray ? value : [value])
        .flatMap(found => String(found).split(/[\s,]+/))
        .filter(Boolean)
        .map(Number)
    if (ids.some(id => !Number.isInteger(id) || id < 1)) throw new Error("Choose rows by their numbers")
    return ids.length ? new Set(ids) : null
}
function finish(target, batch) {
    const items = itemsOf(batch.row.transferId)
    const done = items.filter(item => ["applied", "skipped"].includes(item.result)).length
    const failedCount = items.filter(item => ["invalid", "failed"].includes(item.result)).length
    const state = done === items.length ? "complete" : "partial"
    write(BATCH, {
        transferId: batch.row.transferId,
        state,
        // the previews an apply or a retry rewrote are part of the fingerprint
        inputFingerprint: fingerprintOf(batch.request.validatedRevision, items),
        lastError: failedCount ? failedCount + " rows need attention" : null,
        appliedDate: batch.row.appliedDate || ec.user.nowTimestamp,
        completedDate: state === "complete" ? ec.user.nowTimestamp : null
    })
    return state
}
function apply(target, p) {
    const batch = batchOf(target, p.operationKey, "edit")
    if (batch.row.purgedDate) return refusal("PURGED", "The staged rows of this import were removed: stage the file again")
    const current = Object.assign(get(target, p), {changedRows: 0, attemptedCount: 0})
    // the preview its caller reviewed: the same validation, mapping, options and registration
    if (
        !text(p.inputFingerprint) ||
        text(p.inputFingerprint) !== text(batch.row.inputFingerprint) ||
        batch.request.validatedRevision !== mappingRevision(batch.row, batch.request, target)
    )
        return Object.assign(
            current,
            refusal("STALE_PREVIEW", "The import changed since this preview: validate and review it again")
        )
    const selected = selectedIds(p.selectedRowIds)
    const work = itemsOf(batch.row.transferId).filter(
        item => item.result === "valid" && (!selected || selected.has(Number(item.itemSeqId)))
    )
    if (work.length) {
        const marked = lib.callRegistered("update#" + BATCH, {transferId: batch.row.transferId, state: "applying"})
        if (marked.failure) ec.logger.error("Transfer " + batch.row.transferId + " state was not kept: " + marked.failure)
    }
    let changedRows = 0
    for (const item of work)
        if (
            runItem(target, batch.row.transferId, Number(item.itemSeqId), {inputFingerprint: p.inputFingerprint}).applied === true
        )
            changedRows++
    // nothing to do: the answer of the first apply, as it stands
    if (!work.length && ["complete", "partial"].includes(batch.row.state)) return Object.assign(current, {repeated: true})
    if (!work.length) return current
    const state = finish(target, batch)
    return Object.assign(get(target, p), {state, changedRows, attemptedCount: work.length})
}
// retry#ImportItems for a batch: the failed rows chosen (or every failed row, at most maxRetry), each validated again and
// applied on its own; a row that is not failed is named and left as it is.
function retry(target, p) {
    const batch = batchOf(target, p.operationKey, "edit")
    if (batch.row.purgedDate) return refused("PURGED", "The staged rows of this import were removed: stage the file again")
    const selected = selectedIds(p.itemSeqIdList)
    const items = itemsOf(batch.row.transferId)
    const chosen = selected
        ? Array.from(selected).sort((a, b) => a - b)
        : items.filter(item => item.result === "failed").map(item => Number(item.itemSeqId))
    if (!chosen.length) return refused("NONE_FAILED", "No failed row to retry")
    if (chosen.length > maxRetry) throw new Error("At most " + maxRetry + " rows at a time")
    const key = lib.operationKey({operationKey: p.retryKey})
    const run = () => {
        const resultList = chosen.map(itemSeqId => {
            const item = items.find(found => Number(found.itemSeqId) === itemSeqId)
            if (!item) return {itemSeqId, applied: false, reasonCode: "NOT_FOUND", reason: "No such row"}
            if (item.result !== "failed")
                return {
                    itemSeqId,
                    applied: false,
                    reasonCode: "NOT_FAILED",
                    reason:
                        item.result === "applied" ? "Already applied: it is not applied again" : "Only a failed row is retried"
                }
            const done = runItem(target, batch.row.transferId, itemSeqId, {retry: true})
            return {
                itemSeqId,
                applied: done.applied === true,
                reasonCode: done.applied ? null : done.reasonCode || "FAILED",
                reason: done.applied ? null : done.reason || null,
                resultRef: done.resultRef || null
            }
        })
        const appliedCount = resultList.filter(found => found.applied).length
        const state = finish(target, batch)
        return {
            applied: appliedCount > 0,
            reasonCode: appliedCount ? null : resultList.length === 1 ? resultList[0].reasonCode : "NONE_APPLIED",
            reason: appliedCount ? null : resultList.length === 1 ? resultList[0].reason : "None of the rows could be applied",
            operationKey: batch.row.transferId,
            state,
            appliedCount,
            refusedCount: resultList.length - appliedCount,
            resultList
        }
    }
    const kept = result =>
        [80, 0]
            .map(length =>
                Object.assign({}, result, {
                    resultList: result.resultList.map(found => ({
                        itemSeqId: found.itemSeqId,
                        applied: found.applied,
                        reasonCode: found.reasonCode,
                        reason: length ? text(found.reason).slice(0, length) || null : null
                    }))
                })
            )
            .find(short => JSON.stringify(short).length < 3900) || Object.assign({}, result, {resultList: []})
    return lib.once(key, "importRetry", sha(batch.row.transferId + ":" + chosen.join(",")).slice(0, 40), run, kept)
}

// ===== get =====
const countsOf = items => ({
    rowCount: items.length,
    validCount: items.filter(item => item.result === "valid").length,
    appliedCount: items.filter(item => item.result === "applied").length,
    errorCount: items.filter(item => ["invalid", "failed"].includes(item.result)).length,
    failedCount: items.filter(item => item.result === "failed").length,
    skippedCount: items.filter(item => item.result === "skipped").length
})
function get(target, p) {
    const batch = batchOf(target, p.operationKey, "read")
    const all = itemsOf(batch.row.transferId)
    const shown = p.errorsOnly === true ? all.filter(item => ["invalid", "failed"].includes(item.result)) : all
    const pageIndex = Math.max(0, Math.min(Math.max(0, Math.ceil(shown.length / pageSize) - 1), Number(p.pageIndex) || 0))
    const account = one("moqui.security.UserAccount", {userId: batch.row.userId})
    return Object.assign(
        {
            operationKey: batch.row.transferId,
            targetId: target.targetId,
            kindId: target.targetId,
            kindDescription: lineText(target.title),
            organizationPartyId: text(batch.row.scopeKey) || null,
            state: batch.row.state,
            lastError: batch.row.lastError || null,
            filename: batch.row.filename,
            fileHash: batch.row.fileHash,
            headers: batch.request.headers || [],
            mapping: plain(batch.request.mapping),
            defaults: plain(batch.request.defaults),
            valueMappings: plain(batch.request.valueMappings),
            batchOptions: plain(batch.request.batchOptions),
            clearFields: [],
            updateMode: "create",
            numberFormat: {decimal: ".", group: ""},
            dateFormat: "iso",
            columns: target.columns.map(column => ({
                name: column.name,
                label: column.label,
                type: "text",
                required: column.required
            })),
            mappingRevision: mappingRevision(batch.row, batch.request, target),
            inputFingerprint: batch.row.inputFingerprint || null,
            pageIndex,
            pageSize,
            shownCount: shown.length,
            canEdit: batch.canEdit && !batch.row.purgedDate,
            purged: !!batch.row.purgedDate,
            stagedDate: ms(batch.row.stagedDate),
            appliedDate: ms(batch.row.appliedDate),
            completedDate: ms(batch.row.completedDate),
            userId: batch.row.userId,
            userName: text(account?.userFullName) || text(account?.username) || batch.row.userId,
            importRows: shown.slice(pageIndex * pageSize, (pageIndex + 1) * pageSize).map(item => {
                const preview = lib.json(item.previewJson, null)
                return {
                    itemSeqId: Number(item.itemSeqId),
                    rowNumber: Number(item.rowNumber),
                    sourceValues: plain(lib.json(item.sourceJson, {})?.sourceValues),
                    values: preview?.values || null,
                    changes: [],
                    warnings: preview?.warnings || [],
                    displayText: preview?.displayText || null,
                    result: item.result,
                    reasonCode: item.reasonCode || null,
                    errorMessage: item.errorMessage || null,
                    resultRecordId: item.resultRef || null,
                    resultText: item.resultText || null,
                    attemptCount: Number(item.attemptCount || 0)
                }
            })
        },
        countsOf(all)
    )
}
// The rows as a file: what was staged, each row's result, reason and reference. For those who may read the batch; a cell a
// spreadsheet would read as a formula is written as text.
function report(target, p) {
    const batch = batchOf(target, p.operationKey, "read")
    const headers = (batch.request.headers || []).concat(["Import result", "Import reason", "Reference"])
    const failedOnly = p.errorsOnly === true
    return {
        reportCsv: lib.writeDelimited(
            headers,
            itemsOf(batch.row.transferId)
                .filter(item => !failedOnly || ["invalid", "failed"].includes(item.result))
                .map(item =>
                    Object.assign({}, plain(lib.json(item.sourceJson, {})?.sourceValues), {
                        "Import result": item.result,
                        "Import reason": item.errorMessage || "",
                        Reference: item.resultRef || ""
                    })
                )
        ),
        filename: target.targetId + (failedOnly ? "-import-errors.csv" : "-import-results.csv")
    }
}
function sample(target) {
    return {
        reportCsv: lib.writeDelimited(
            target.columns.map(column => column.name),
            []
        ),
        filename: target.targetId + "-sample.csv"
    }
}

// ===== runs =====
// The batches of a target the caller may read, newest first, of the scopes its scope service admits: their own while they
// hold its import or view permission, and with its view permission the others'. Summaries only.
function runsOf(target, filter) {
    const scopes = scopesOf(target)
    const viewer = !!text(target.viewPermission) && lib.holdsAll(target.viewPermission) && !scopes.failure
    const importer = lib.holdsAll(target.importPermission) || lib.holdsAll(target.viewPermission)
    const scopeKeys = scopes.scopeList.map(scope => scope.scopeKey)
    const titles = new Map(scopes.scopeList.map(scope => [scope.scopeKey, scope.title]))
    let query = find(BATCH, {targetId: target.targetId})
    if (filter.fromDate != null) query = query.condition("stagedDate", "greater-equals", lib.iso(filter.fromDate))
    if (filter.thruDate != null) query = query.condition("stagedDate", "less", lib.iso(filter.thruDate))
    const found = Array.from(query.orderBy("-stagedDate").list()).filter(row => {
        const scopeKey = text(row.scopeKey)
        if (filter.scopeKey && scopeKey !== filter.scopeKey) return false
        return (
            ((row.userId === ec.user.userId && importer) || viewer) &&
            (scopeKey ? scopeKeys.includes(scopeKey) : !!scopes.unscoped)
        )
    })
    const names = new Map()
    return found.slice(0, filter.maxRows).map(row => {
        if (!names.has(row.userId)) {
            const account = one("moqui.security.UserAccount", {userId: row.userId})
            names.set(row.userId, text(account?.userFullName) || text(account?.username) || row.userId)
        }
        const counts = countsOf(itemsOf(row.transferId))
        return Object.assign(
            {
                runId: row.transferId,
                fileLabel: row.filename,
                scopeKey: text(row.scopeKey) || null,
                scopeTitle: titles.get(text(row.scopeKey)) || null,
                state: row.state,
                startedDate: ms(row.stagedDate),
                userName: names.get(row.userId),
                detail: true
            },
            counts
        )
    })
}

// ===== retention =====
// clean#TransferBatches: a batch never applied is removed after its target's retention; an applied one keeps its outcomes
// and loses what was staged (the file's values and the previews). Each batch on its own.
function clean() {
    const targets = new Map(rows(lib.TARGET, {}).map(row => [row.targetId, row]))
    const now = ms(ec.user.nowTimestamp)
    let removedCount = 0
    let purgedCount = 0
    for (const row of Array.from(find(BATCH).condition("purgedDate", "equals", null).list())) {
        const days = Math.max(Number(targets.get(row.targetId)?.retentionDays) || 30, 1)
        const last = ms(row.completedDate) ?? ms(row.appliedDate) ?? ms(row.stagedDate) ?? 0
        // an apply under way is left alone; one that stopped (nothing kept of it for a day) is cleaned as any other
        if (now - last < days * 86400000 || (row.state === "applying" && now - (ms(row.lastUpdatedStamp) ?? 0) < 86400000))
            continue
        try {
            const items = itemsOf(row.transferId)
            if (!items.some(item => item.result === "applied")) {
                find(ITEM, {transferId: row.transferId}).deleteAll()
                find(BATCH, {transferId: row.transferId}).deleteAll()
                removedCount++
            } else {
                for (const item of items)
                    write(ITEM, {transferId: row.transferId, itemSeqId: item.itemSeqId, sourceJson: null, previewJson: null})
                write(BATCH, {
                    transferId: row.transferId,
                    requestJson: JSON.stringify({headers: [], mapping: {}, defaults: {}, valueMappings: {}, batchOptions: {}}),
                    purgedDate: ec.user.nowTimestamp
                })
                purgedCount++
            }
        } catch (error) {
            ec.logger.error("Transfer " + row.transferId + " was not cleaned: " + String(error?.message ?? error))
        }
    }
    return {removedCount, purgedCount}
}

;({stage, configure, validate, apply, retry, get, report, sample, runsOf, scopesOf, applyItem, failItem, clean, boundsOf})
