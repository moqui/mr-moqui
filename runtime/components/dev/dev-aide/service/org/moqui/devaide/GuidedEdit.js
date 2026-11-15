/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The terminal guided editors' server side (plans/design-dev-aide.md D14; Entities/EditTui, Services/EditTui and
// Screens/DesignTui): what each twin reads (the view service, its edit target and companions as staged in the user's
// edit buffer, their outlines), and the steps of an edit (build the operations with GuidedOps.js, apply them on the
// buffered text with apply#EditBufferOps), of Save (save#EditBuffer in kind order, its conflict kept for the page that
// shows it) and of Discard. Nothing is written until Save.

const OPS = "component://dev-aide/service/org/moqui/devaide/GuidedOps.js"
const BUFFER = "org.moqui.devaide.WorkspaceServices."

function guidedOps() {
    return ec.resource.script(OPS)
}
function call(name, parameters) {
    return ec.service.sync().name(name).parameters(parameters || {}).call() || {}
}
// A call whose refusal the page shows rather than fails on: its errors as text, cleared.
function probe(name, parameters) {
    ec.message.pushErrors()
    let out = {}
    try {
        out = ec.service.sync().name(name).parameters(parameters || {}).requireNewTransaction(true).call() || {}
    } finally {
        const errors = ec.message.hasError() ? ec.message.errorsString : ""
        ec.message.clearErrors()
        ec.message.popErrors()
        out = Object.assign({}, out, {refused: errors})
    }
    return out
}
function fileKey(file) {
    return file ? file.rootId + ":" + file.path : ""
}

// The staged texts of `files` ([{rootId, path, initialText}]) with their outlines and diffs, and the buffer's pending
// summary and expiry.
function open(target, files) {
    const asked = (files || []).filter((file) => file && file.rootId && file.path)
    const out = call(BUFFER + "get#EditBuffer", {target, files: JSON.stringify(asked), outline: true, diff: true})
    const byKey = {}
    for (const file of out.files || []) byKey[fileKey(file)] = file
    return {bufferId: out.bufferId, files: out.files || [], byKey, pending: out.pending || {count: 0, files: [], names: []}, expired: !!out.expired}
}
// Apply operations to one file of the buffer (its initial text when it is not there yet).
function apply(target, file, ops, load) {
    if (!file) {
        ec.message.addError(ec.l10n.localize("There is no file to make this edit in: create a site component on Home first"))
        return null
    }
    if (!ops || !ops.length) {
        ec.message.addMessage(ec.l10n.localize("Nothing to change"), "info")
        return null
    }
    const out = call(BUFFER + "apply#EditBufferOps", {target, rootId: file.rootId, path: file.path, initialText: file.initialText || "",
        ops: JSON.stringify(ops), load: !!load})
    if (!ec.message.hasError()) ec.message.addMessage(ec.l10n.localize("Staged: Save writes it (Ctrl+S on the header)"), "info")
    return out
}

// The result a transition leaves for the page it returns to, once.
function stashKey(target) {
    return "DevAideGuided." + target
}
function stash(target, result) {
    ec.web.sessionAttributes[stashKey(target)] = JSON.stringify(result)
}
function taken(target) {
    let result = null
    try { result = JSON.parse(ec.web.sessionAttributes[stashKey(target)] || "null") } catch (err) { result = null }
    if (result) ec.web.sessionAttributes.remove(stashKey(target))
    return result || {}
}

// Save: the staged files in kind order; a conflict is kept for the page, with its diff and the two ways out.
function save(target, overwrite) {
    // a conflict or problems an earlier Save left are this one's to answer now
    ec.web.sessionAttributes.remove(stashKey(target))
    const out = call(BUFFER + "save#EditBuffer", {target, overwrite: JSON.stringify(overwrite || [])})
    if (ec.message.hasError()) return out
    const saved = out.saved || []
    const errors = (out.problems || []).filter((problem) => problem.severity === "error")
    if (out.conflict) {
        stash(target, {conflict: out.conflict, problems: out.problems || []})
        ec.message.addMessage(ec.l10n.localize("A file changed on disk since the edits started: compare them, then choose"), "warning")
        return out
    }
    const loaded = (out.loaded || []).map((entry) => entry.error ? entry.path + ": " + entry.error : entry.records + " records from " + entry.path)
    const summary = saved.map((file) => file.path.split("/").pop() + (file.summary ? " (" + file.summary + ")" : "")).join(", ")
    ec.message.addMessage("Saved " + saved.length + (saved.length === 1 ? " file" : " files") + (summary ? ": " + summary : "") +
        (loaded.length ? "; seed data loaded: " + loaded.join(", ") : "") + (out.restart ? "; a restart is needed" : ""),
    errors.length ? "warning" : "success")
    if (errors.length || (out.problems || []).length) stash(target, {problems: out.problems || []})
    return out
}
function discard(target, file) {
    ec.web.sessionAttributes.remove(stashKey(target))
    return call(BUFFER + "clear#EditBuffer", file ? {target, rootId: file.rootId, path: file.path} : {target})
}

// The pending line of a twin's header: how many files are staged, which, and when they expire.
function pendingText(buffer) {
    const pending = buffer.pending || {}
    if (!pending.count) return ""
    return pending.count + (pending.count === 1 ? " unsaved change" : " unsaved changes") + " in " + (pending.names || []).join(", ") +
        (pending.expiresInMinutes != null ? " · kept " + pending.expiresInMinutes + " min more" : "")
}

// ---- the service writer ----

// verb#noun of a full service name, as its element names it in the file.
function serviceShortName(name) {
    return name.indexOf(".") >= 0 && name.indexOf("#") > 0 ? name.slice(name.lastIndexOf(".", name.indexOf("#")) + 1) : name
}
function serviceContext(artifact) {
    const ops = guidedOps()
    const serviceName = String(artifact || "").replace(/^service:/, "")
    const target = "service:" + serviceName
    const answer = serviceName ? probe("org.moqui.devaide.ServiceWriterServices.get#ServiceView", {serviceName}) : {refused: "Name a service"}
    const view = answer.view || null
    const row = (view && view.artifact) || {}
    const file = row.rootId && row.relativePath ? {rootId: row.rootId, path: row.relativePath} : null
    const secas = view && view.secasFile ? {rootId: view.secasFile.rootId, path: view.secasFile.path, initialText: ops.service.SECAS_TEXT} : null
    const buffer = view ? open(target, [file, secas]) : {files: [], byKey: {}, pending: {count: 0}}
    const serviceFile = file ? buffer.byKey[fileKey(file)] || null : null
    const shortName = serviceShortName(serviceName)
    const entry = serviceFile && serviceFile.outline ? ops.service.servicesOf(serviceFile.outline).find((item) => item.name === shortName) : null
    return {ops, serviceName, target, view, refused: answer.refused, file, secas, buffer, serviceFile, secasFile: secas ? buffer.byKey[fileKey(secas)] : null,
        node: entry ? entry.node : null, shortName}
}
// A service's parameters as rows: the staged element's own (children flattened and indented) when the file is
// staged, else the definition's; the implied ones (auto) from the definition after them.
function parameterRows(service, which) {
    const {ops, view, node, serviceFile} = service
    const defined = (view && view.service && view.service[which === "in-parameters" ? "inParameters" : "outParameters"]) || []
    const rows = []
    const push = (parameter, depth, implied) => {
        rows.push({which, name: parameter.name, shown: (depth ? "  ".repeat(depth - 1) + "  └ " : "") + parameter.name + (implied ? " (auto)" : ""),
            type: parameter.type || "String", required: parameter.required === "true" || parameter.required === true ? "yes" : "",
            defaultValue: parameter.defaultValue || parameter.default || "", description: parameter.description || "", top: !depth && !implied,
            requiredFlag: parameter.required === "true" || parameter.required === true ? "true" : ""})
    }
    if (serviceFile && serviceFile.staged && node) {
        const holder = ops.pure.children(node, [which])[0]
        const walk = (parent, depth) => {
            for (const child of ops.pure.children(parent, ["parameter"])) {
                const description = ops.pure.children(child, ["description"])[0]
                push({name: ops.pure.attrOf(child, "name"), type: ops.pure.attrOf(child, "type"), required: ops.pure.attrOf(child, "required"),
                    defaultValue: ops.pure.attrOf(child, "default-value"), description: description ? description.text : ""}, depth, false)
                walk(child, depth + 1)
            }
        }
        if (holder) walk(holder, 0)
        const own = new Set(rows.map((row) => row.name))
        for (const parameter of defined) if (parameter.implied && !own.has(parameter.name)) push(parameter, 0, true)
        return rows
    }
    for (const parameter of defined) {
        push(parameter, 0, !!parameter.implied)
        for (const child of parameter.children || []) push(child, 1, false)
    }
    return rows
}

// ---- the entity modeler ----

function entityContext(artifact) {
    const ops = guidedOps()
    const entityName = String(artifact || "").replace(/^(entity|view-entity):/, "")
    const target = "entity:" + entityName
    const answer = entityName ? probe("org.moqui.devaide.EntityServices.get#EntityView", {entityName}) : {refused: "Name an entity"}
    const view = answer.view || null
    const entity = view && view.entity
    const edit = view && view.target ? {rootId: view.target.rootId, path: view.target.path, inPlace: !!view.target.inPlace,
        initialText: view.target.inPlace ? "" : ops.entity.ENTITIES_TEXT, component: view.target.component} : null
    const seed = view && view.seedFile ? {rootId: view.seedFile.rootId, path: view.seedFile.path, initialText: ops.entity.SEED_TEXT} : null
    const buffer = view ? open(target, [edit, seed]) : {files: [], byKey: {}, pending: {count: 0}}
    const editFile = edit ? buffer.byKey[fileKey(edit)] || null : null
    const seedFile = seed ? buffer.byKey[fileKey(seed)] || null : null
    const found = editFile && editFile.outline && entity
        ? ops.entity.findEntityElement(editFile.outline, entity.fullEntityName, edit.inPlace ? null : ["extend-entity"]) : null
    return {ops, entityName: entity ? entity.fullEntityName : entityName, target, view, entity, refused: answer.refused, edit, seed, buffer,
        editFile, seedFile, element: found ? found.node : null}
}
// The element an entity edit goes to, made first when the site has no extend-entity of it yet: `build` gets it and
// answers the edit's operations.
function entityEdit(entity, build) {
    const {ops, editFile, element} = entity
    if (!entity.edit || !editFile) return {ops: [], refused: "This entity is upstream and there is no site component to extend it in"}
    let first = []
    let path = element ? element.path : null
    if (!path) {
        const made = ops.entity.ensureExtend(editFile.outline || {path: [], children: []}, entity.entity.fullEntityName)
        path = made.path
        first = made.ops
    }
    return {ops: first.concat(build(element || {path, children: [], attrs: []}))}
}

// ---- the screen builder ----

function screenContext(rootId, path, page) {
    const ops = guidedOps()
    const answer = probe("org.moqui.devaide.ScreenServices.get#ScreenView", {rootId, path, page})
    const view = answer.view || null
    const target = "screen:" + (view ? view.location : (rootId + ":" + path + ":" + page))
    // where the edits go: the base file in place, or the site's screen-extend of it
    const base = view && view.base ? {rootId: view.base.rootId, path: view.base.path} : null
    const edit = view && view.inPlace ? base
        : view && view.extension ? {rootId: view.extension.rootId, path: view.extension.path, initialText: ops.screen.EXTEND_TEXT} : null
    const buffer = view ? open(target, [base, edit && fileKey(edit) !== fileKey(base) ? edit : null]) : {files: [], byKey: {}, pending: {count: 0}}
    return {ops, target, view, refused: answer.refused, base, edit, buffer, baseFile: base ? buffer.byKey[fileKey(base)] || null : null,
        editFile: edit ? buffer.byKey[fileKey(edit)] || null : null, extension: !!(view && !view.inPlace)}
}

;({open, apply, save, discard, stash, taken, pendingText, probe, fileKey, serviceContext, parameterRows, serviceShortName, entityContext, entityEdit,
    screenContext})
