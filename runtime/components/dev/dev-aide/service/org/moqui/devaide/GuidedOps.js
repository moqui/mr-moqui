/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The operation builders of the terminal's guided editors (plans/design-dev-aide.md D14; the twins Entities/EditTui,
// Services/EditTui and Screens/DesignTui): the browser views' pure builders (moqui.devAide.entityOps, serviceOps and
// screenOps in screen/devstatic/DevAideEntity.js, DevAideService.js and DevAideScreen.js), ported as they are so a
// terminal edit makes the operations a browser edit does. test/GuidedOps.spec.js runs the browser page specs' cases
// (test/client/pages/DevAide*.page.js and test/vectors/entity-view-vectors.json) against them. Each builder takes an
// outline as get#XmlOutline answers it and answers a list of operations of the lossless XML layer
// (apply#EditBufferOps applies them on the buffered text).
//
// A screen calls it as ec.resource.script("component://dev-aide/service/org/moqui/devaide/GuidedOps.js").entity.addField(...).

// ---- shared: outlines and XML text ----

function attrOf(node, name) {
    const found = node && (node.attrs || []).find(pair => pair[0] === name)
    return found ? found[1] : undefined
}
function outlineAt(outline, path) {
    let node = outline || null
    for (const index of path || []) node = node && node.children ? node.children[index] || null : null
    return node
}
function walkOutline(outline, visit, depth) {
    if (!outline) return
    visit(outline, depth || 0)
    for (const child of outline.children || []) walkOutline(child, visit, (depth || 0) + 1)
}
function children(node, names) {
    return ((node && node.children) || []).filter(child => !names || names.indexOf(child.name) >= 0)
}
function escape(value) {
    return String(value == null ? "" : value)
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/"/g, "&quot;")
}
function escapeText(value) {
    return String(value == null ? "" : value)
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
}
// An element as text, as the service and screen views write it: attributes in the order given, those empty left out.
function plainXml(name, attrs, body) {
    let out = "<" + name
    for (const [key, value] of attrs || [])
        if (value !== undefined && value !== null && value !== "") out += " " + key + '="' + escape(value) + '"'
    return body ? out + ">" + body + "</" + name + ">" : out + "/>"
}
function last(list) {
    return list.length ? list[list.length - 1] : null
}
// An attribute set, or removed when empty.
function setAttributeOps(node, name, value) {
    if (!node) return []
    if (value === "" || value === null || value === undefined || value === false) {
        return attrOf(node, name) === undefined ? [] : [{op: "remove-attribute", at: node.path, name}]
    }
    return [{op: "set-attribute", at: node.path, name, value: value === true ? "true" : String(value)}]
}

// ---- the entity view (DevAideEntity.js) ----

// An element as text: attributes in the order given, those empty or false left out; `body` is its content as XML.
function entityXml(name, attrs, body) {
    let out = "<" + name
    for (const [key, value] of attrs || []) {
        if (value === undefined || value === null || value === "" || value === false) continue
        out += " " + key + '="' + escape(value === true ? "true" : value) + '"'
    }
    return body ? out + ">" + body + "</" + name + ">" : out + "/>"
}
function shortName(name) {
    return String(name || "")
        .split(".")
        .pop()
}
function entityElements(outline) {
    return children(outline, ["entity", "view-entity", "extend-entity"]).map(node => ({
        node,
        kind: node.name,
        entityName: attrOf(node, "entity-name") || "",
        package: attrOf(node, "package") || ""
    }))
}
// The element of `fullName` in an entities file: its definition, or an extend-entity of it.
function findEntityElement(outline, fullName, kinds) {
    const short = shortName(fullName)
    const pkg = String(fullName || "").indexOf(".") > 0 ? String(fullName).slice(0, String(fullName).lastIndexOf(".")) : ""
    return (
        entityElements(outline).find(
            entry =>
                (!kinds || kinds.indexOf(entry.kind) >= 0) &&
                (entry.entityName === fullName ||
                    (entry.entityName === short && (!entry.package || !pkg || entry.package === pkg)))
        ) || null
    )
}
function childNamed(node, name, key, value) {
    return children(node, [name]).find(child => attrOf(child, key || "name") === value) || null
}
// The operations that make the extend-entity of `fullName` in a site entities file when it has none: answers the
// path the element has after them.
function ensureExtend(outline, fullName) {
    const found = findEntityElement(outline, fullName, ["extend-entity"])
    if (found) return {path: found.node.path, ops: []}
    const short = shortName(fullName)
    const pkg = String(fullName).indexOf(".") > 0 ? String(fullName).slice(0, String(fullName).lastIndexOf(".")) : ""
    const count = children(outline).length
    return {
        path: [count],
        ops: [
            {
                op: "insert",
                at: outline.path || [],
                position: "last",
                xml: entityXml("extend-entity", [
                    ["entity-name", short],
                    ["package", pkg]
                ])
            }
        ]
    }
}
function fieldXml(field) {
    return entityXml(
        "field",
        [
            ["name", field.name],
            ["type", field.type],
            ["is-pk", !!field.isPk],
            ["not-null", !!field.notNull],
            ["default", field.defaultValue],
            ["encrypt", !!field.encrypt],
            ["enable-audit-log", field.enableAuditLog],
            ["enable-localization", !!field.enableLocalization],
            ["create-only", !!field.createOnly]
        ],
        field.description ? "<description>" + escapeText(field.description) + "</description>" : ""
    )
}
// Insert `xml` among an element's children of `names`: after the child `afterName` (by its name attribute), else
// after the last of them, else before the first other child, else as the element's last child.
function insertAmong(element, names, xml, afterName) {
    const among = children(element, names)
    const after = afterName ? among.find(child => attrOf(child, "name") === afterName) : null
    const anchor = after || last(among)
    if (anchor) return [{op: "insert", at: anchor.path, position: "after", xml}]
    const first = children(element)[0]
    if (first) return [{op: "insert", at: first.path, position: "before", xml}]
    return [{op: "insert", at: element.path, position: "last", xml}]
}
function addField(element, field, afterName) {
    return insertAmong(element, ["field"], fieldXml(field), afterName)
}
// An attribute of a field: set on the field element when the element has it, else an override field of that name
// with the attribute (an extend-entity's field merges into the entity's by name).
function setFieldAttribute(element, fieldName, name, value) {
    const field = childNamed(element, "field", "name", fieldName)
    if (field) {
        if (value === "" || value === false || value === null || value === undefined) {
            return attrOf(field, name) === undefined ? [] : [{op: "remove-attribute", at: field.path, name}]
        }
        return [{op: "set-attribute", at: field.path, name, value: value === true ? "true" : String(value)}]
    }
    if (value === "" || value === false || value == null) return []
    return insertAmong(
        element,
        ["field"],
        entityXml("field", [
            ["name", fieldName],
            [name, value === true ? "true" : String(value)]
        ])
    )
}
function setFieldDescription(element, fieldName, text) {
    const field = childNamed(element, "field", "name", fieldName)
    if (!field)
        return insertAmong(
            element,
            ["field"],
            entityXml("field", [["name", fieldName]], "<description>" + escapeText(text) + "</description>")
        )
    const description = children(field, ["description"])[0]
    if (description) return text ? [{op: "set-text", at: description.path, text}] : [{op: "remove", at: description.path}]
    return text
        ? [{op: "insert", at: field.path, position: "last", xml: "<description>" + escapeText(text) + "</description>"}]
        : []
}
function removeField(element, fieldName) {
    const field = childNamed(element, "field", "name", fieldName)
    return field ? [{op: "remove", at: field.path}] : []
}
function moveField(element, fieldName, otherName, position) {
    const field = childNamed(element, "field", "name", fieldName)
    const other = childNamed(element, "field", "name", otherName)
    if (!field || !other || field === other) return []
    return [{op: "move", at: field.path, to: other.path, position: position === "before" ? "before" : "after"}]
}
// Move up or down: before the field above, or after the field below (the terminal's row actions for a drag).
function stepField(element, fieldName, step) {
    const fields = children(element, ["field"])
    const at = fields.findIndex(node => attrOf(node, "name") === fieldName)
    const other = at < 0 ? null : fields[at + (step < 0 ? -1 : 1)]
    return other ? moveField(element, fieldName, attrOf(other, "name"), step < 0 ? "before" : "after") : []
}
function relationshipXml(rel) {
    return entityXml(
        "relationship",
        [
            ["type", rel.type || "one"],
            ["title", rel.title],
            ["related", rel.related],
            ["short-alias", rel.shortAlias],
            ["fk-name", rel.fkName]
        ],
        (rel.keyMaps || [])
            .filter(key => key.fieldName)
            .map(key =>
                entityXml("key-map", [
                    ["field-name", key.fieldName],
                    ["related", key.related]
                ])
            )
            .join("")
    )
}
function insertRelationship(element, xml) {
    const rels = children(element, ["relationship"])
    return rels.length
        ? [{op: "insert", at: rels[rels.length - 1].path, position: "after", xml}]
        : [{op: "insert", at: element.path, position: "last", xml}]
}
function addRelationship(element, rel) {
    return insertRelationship(element, relationshipXml(rel))
}
function removeRelationship(element, related, title) {
    const found = children(element, ["relationship"]).find(
        child => attrOf(child, "related") === related && (attrOf(child, "title") || "") === (title || "")
    )
    return found ? [{op: "remove", at: found.path}] : []
}
function indexXml(index) {
    return entityXml(
        "index",
        [
            ["name", index.name],
            ["unique", !!index.unique]
        ],
        (index.fields || [])
            .filter(name => name)
            .map(name => entityXml("index-field", [["name", name]]))
            .join("")
    )
}
function addIndex(element, index) {
    return insertAmong(element, ["index"], indexXml(index))
}
function removeIndex(element, name) {
    const found = childNamed(element, "index", "name", name)
    return found ? [{op: "remove", at: found.path}] : []
}
// A Picklist field (8.1): the ...EnumId field and its relationship to Enumeration titled by the type, and the seed
// records: the EnumerationType and an Enumeration per value.
function picklist(spec) {
    const typeId = spec.enumTypeId
    const fieldName = spec.fieldName
    const field = fieldXml({name: fieldName, type: "id", description: spec.description})
    const relationship = relationshipXml({
        type: "one",
        title: spec.title || typeId,
        related: "moqui.basic.Enumeration",
        keyMaps: [{fieldName, related: "enumId"}]
    })
    const seed = []
    if (!spec.existingType)
        seed.push(
            entityXml("moqui.basic.EnumerationType", [
                ["enumTypeId", typeId],
                ["description", spec.typeDescription || spec.title || typeId]
            ])
        )
    ;(spec.values || []).forEach((value, index) =>
        seed.push(
            entityXml("moqui.basic.Enumeration", [
                ["enumId", value.id],
                ["enumTypeId", typeId],
                ["sequenceNum", String((index + 1) * 10)],
                ["description", value.description || value.id]
            ])
        )
    )
    return {field, relationship, seed}
}
// A Status field (8.1): statusId and its relationship to StatusItem, and the seed records: the StatusType, a
// StatusItem per status, the StatusFlow of the type and a transition from each status to the next.
function status(spec) {
    const typeId = spec.statusTypeId
    const fieldName = spec.fieldName || "statusId"
    const field = fieldXml({name: fieldName, type: "id", description: spec.description})
    const relationship = relationshipXml({
        type: "one",
        title: spec.title,
        related: "moqui.basic.StatusItem",
        keyMaps: fieldName === "statusId" ? [] : [{fieldName, related: "statusId"}]
    })
    const items = spec.items || []
    const seed = [
        entityXml("moqui.basic.StatusType", [
            ["statusTypeId", typeId],
            ["description", spec.typeDescription || typeId]
        ])
    ]
    items.forEach((item, index) =>
        seed.push(
            entityXml("moqui.basic.StatusItem", [
                ["statusId", item.id],
                ["statusTypeId", typeId],
                ["sequenceNum", String((index + 1) * 10)],
                ["description", item.description || item.id]
            ])
        )
    )
    if (spec.flow !== false && items.length > 1) {
        seed.push(
            entityXml("moqui.basic.StatusFlow", [
                ["statusFlowId", typeId],
                ["statusTypeId", typeId],
                ["description", (spec.typeDescription || typeId) + " flow"]
            ])
        )
        for (let index = 0; index + 1 < items.length; index++) {
            seed.push(
                entityXml("moqui.basic.StatusFlowTransition", [
                    ["statusFlowId", typeId],
                    ["statusId", items[index].id],
                    ["toStatusId", items[index + 1].id],
                    ["transitionName", items[index + 1].description || items[index + 1].id]
                ])
            )
        }
    }
    return {field, relationship, seed}
}
function insertRecords(outline, records) {
    return (records || []).map(xml => ({op: "insert", at: (outline && outline.path) || [], position: "last", xml}))
}
function recordTables(outline) {
    const tables = []
    const byName = {}
    for (const node of children(outline)) {
        let table = byName[node.name]
        if (!table) {
            table = byName[node.name] = {entity: node.name, columns: [], rows: []}
            tables.push(table)
        }
        for (const [key] of node.attrs || []) if (table.columns.indexOf(key) < 0) table.columns.push(key)
        table.rows.push({path: node.path, values: Object.fromEntries(node.attrs || []), nested: (node.children || []).length})
    }
    return tables
}
function setRecordValue(row, name, value) {
    return value === "" || value == null
        ? row.values[name] === undefined
            ? []
            : [{op: "remove-attribute", at: row.path, name}]
        : [{op: "set-attribute", at: row.path, name, value: String(value)}]
}
function addRecord(outline, table, values) {
    const rows = table.rows
    const xml = entityXml(
        table.entity,
        table.columns.map(name => [name, values ? values[name] : ""])
    )
    return rows.length
        ? [{op: "insert", at: rows[rows.length - 1].path, position: "after", xml}]
        : [{op: "insert", at: (outline && outline.path) || [], position: "last", xml}]
}
// The terminal's Edit of a record (its dialog posts every column): each value that differs from the record's, set, or
// removed when empty; a value not given stays.
function setRecordValues(row, values) {
    let list = []
    for (const [name, value] of Object.entries(values || {})) {
        if (value == null || String(value) === (row.values[name] == null ? "" : String(row.values[name]))) continue
        list = list.concat(setRecordValue(row, name, value))
    }
    return list
}
// A record of a data file's tables by its outline key ("3"), with its table; null when no record is there.
function recordAt(tables, key) {
    for (const table of tables || []) {
        const row = table.rows.find(item => item.path.join(".") === String(key == null ? "" : key))
        if (row) return {table, row}
    }
    return null
}
function flagAttr(node, name) {
    const value = attrOf(node, name)
    return value === undefined ? undefined : value === "true"
}
function fieldOf(node) {
    return {
        name: attrOf(node, "name"),
        type: attrOf(node, "type"),
        isPk: flagAttr(node, "is-pk"),
        notNull: flagAttr(node, "not-null"),
        defaultStr: attrOf(node, "default"),
        encrypt: flagAttr(node, "encrypt"),
        enableAuditLog: attrOf(node, "enable-audit-log"),
        enableLocalization: flagAttr(node, "enable-localization"),
        createOnly: flagAttr(node, "create-only")
    }
}
// The fields the view shows: the server's merged definition with the buffer's element over it, so an edit shows
// before Save.
function mergeFields(serverFields, element, inPlace) {
    const own = children(element, ["field"])
        .map(fieldOf)
        .filter(field => field.name)
    const server = serverFields || []
    const byName = {}
    for (const field of server) byName[field.name] = field
    const merged = field => {
        const base = byName[field.name]
        if (!base) return Object.assign({}, field, {type: field.type || "", unsaved: true})
        const out = Object.assign({}, base)
        for (const key of Object.keys(field)) if (field[key] !== undefined && key !== "name") out[key] = field[key]
        if (field.type === undefined) out.type = base.type
        return out
    }
    if (inPlace && element && element.name !== "extend-entity") {
        const listed = own.map(merged)
        const names = new Set(own.map(field => field.name))
        return listed.concat(server.filter(field => !names.has(field.name)))
    }
    const overrides = {}
    for (const field of own) overrides[field.name] = field
    const out = server.map(field => (overrides[field.name] ? merged(overrides[field.name]) : field))
    for (const field of own) if (!byName[field.name]) out.push(merged(field))
    return out
}
function kindOf(field, relationships) {
    const type = String(field.type || "")
    if (
        field.name === "statusId" ||
        (relationships || []).some(
            rel =>
                rel.relatedEntityName === "moqui.basic.StatusItem" &&
                (rel.keyMap ? Object.keys(rel.keyMap).indexOf(field.name) >= 0 : false)
        )
    )
        return "Status"
    if (/EnumId$/.test(field.name)) return "Picklist"
    if (/^date/.test(type)) return "Date"
    if (/^number|^currency/.test(type)) return "Number"
    if (type === "text-indicator") return "Yes/No"
    if (
        /^id/.test(type) &&
        (relationships || []).some(rel => rel.type === "one" && rel.keyMap && Object.keys(rel.keyMap).indexOf(field.name) >= 0)
    )
        return "Reference"
    if (/^text/.test(type)) return "Text"
    return type
}
function typeOfKind(kind) {
    return (
        {
            Text: "text-medium",
            Number: "number-decimal",
            Date: "date",
            "Date-time": "date-time",
            "Yes/No": "text-indicator",
            Picklist: "id",
            Reference: "id",
            Status: "id"
        }[kind] || "text-medium"
    )
}
function camel(words) {
    return String(words || "")
        .trim()
        .split(/[^A-Za-z0-9]+/)
        .filter(part => part)
        .map((part, index) =>
            index === 0 ? part.charAt(0).toLowerCase() + part.slice(1) : part.charAt(0).toUpperCase() + part.slice(1)
        )
        .join("")
}
function titleOf(name) {
    return String(name || "")
        .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
        .replace(/^./, ch => ch.toUpperCase())
}
// A Picklist's or Status's type description: its words as a sentence (contact channel: Contact channel), or its id's
// words less the entity's name when it has none.
function typeDescriptionOf(words, typeId, entityName) {
    const said = String(words || "").trim().replace(/\s+/g, " ")
    if (said) return said.charAt(0).toUpperCase() + said.slice(1)
    return titleOf(String(typeId || "").replace(shortName(entityName), "") || typeId)
}
function localizedXml(original, locale, localized) {
    return entityXml("moqui.basic.LocalizedMessage", [
        ["original", original],
        ["locale", locale],
        ["localized", localized]
    ])
}
const KINDS = ["Text", "Number", "Date", "Date-time", "Yes/No", "Picklist", "Reference", "Status", "Other"]
// Add field's name from its words, as the browser's dialog makes it (nameFromWords): a Picklist's ends EnumId, a
// Reference's Id, a Status with no words is statusId; with the type ids the seed records take.
function fieldNaming(entityName, kind, words) {
    let name = camel(words)
    if (kind === "Picklist" && name && !/EnumId$/.test(name)) name += "EnumId"
    if (kind === "Reference" && name && !/Id$/.test(name)) name += "Id"
    if (kind === "Status" && !name) name = "statusId"
    const pascal = name ? name.charAt(0).toUpperCase() + name.slice(1) : ""
    return {
        name,
        type: kind === "Other" ? "" : typeOfKind(kind),
        enumTypeId: kind === "Picklist" ? shortName(entityName) + pascal.replace(/EnumId$/, "") : "",
        statusTypeId:
            kind === "Status" ? shortName(entityName) + (name && name !== "statusId" ? pascal.replace(/Id$/, "") : "") : ""
    }
}
// The values of a Picklist or the statuses of a Status, one a line or comma separated, each with an id from words.
function valuesOf(text, prefix) {
    return String(text || "")
        .split(/[\n,]/)
        .map(value => value.trim())
        .filter(value => value)
        .map(description => ({
            id: prefix + camel(description).replace(/^./, ch => ch.toUpperCase()),
            description
        }))
}
// Add field as the browser's addField does it, for the element edits go to: the field (and a Reference's
// relationship, or a Picklist's or Status's relationship), and the seed records a Picklist or Status writes. On an
// `extension` (an upstream entity's extend-entity) a primary key is refused: it would change the upstream table's key;
// so is a related entity or values given for a kind that takes none. A refusal answers `refused`.
function addFieldSpec(element, entityName, add) {
    const refused = (why) => ({name: "", ops: [], seed: [], refused: why})
    if (add.isPk && add.extension)
        return refused("A primary key cannot be added to an upstream entity's extension: it would change the upstream table's key")
    if (add.related && add.kind !== "Reference") return refused("A related entity is for a Reference field: choose the Reference kind or leave it empty")
    if (String(add.values || "").trim() && add.kind !== "Picklist" && add.kind !== "Status")
        return refused("Values are for a Picklist or Status field: choose that kind or leave them empty")
    const naming = fieldNaming(entityName, add.kind, add.words)
    const name = add.name || naming.name
    const field = {
        name,
        type: add.type || naming.type || "text-medium",
        isPk: !!add.isPk,
        notNull: !!add.notNull,
        description: add.description
    }
    if (add.kind === "Picklist" || add.kind === "Status") {
        const enumTypeId = add.enumTypeId || naming.enumTypeId
        const statusTypeId = add.statusTypeId || naming.statusTypeId
        const spec =
            add.kind === "Picklist"
                ? picklist({
                      fieldName: name,
                      enumTypeId,
                      title: add.title || enumTypeId.replace(shortName(entityName), "") || enumTypeId,
                      typeDescription: typeDescriptionOf(add.words, enumTypeId, entityName),
                      description: add.description,
                      values: valuesOf(add.values, enumTypeId.replace(/Type$/, ""))
                  })
                : status({
                      fieldName: name,
                      statusTypeId,
                      typeDescription: typeDescriptionOf(add.words, statusTypeId, entityName),
                      description: add.description,
                      items: valuesOf(add.values, statusTypeId)
                  })
        return {
            name,
            ops: addField(element, field, add.after).concat(insertRelationship(element, spec.relationship)),
            seed: spec.seed
        }
    }
    const reference =
        add.kind === "Reference" && add.related
            ? {type: "one", related: add.related, keyMaps: [{fieldName: name, related: ""}]}
            : null
    return {name, ops: addField(element, field, add.after).concat(reference ? addRelationship(element, reference) : []), seed: []}
}
const ENTITIES_TEXT =
    '<?xml version="1.0" encoding="UTF-8"?>\n<entities xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" ' +
    'xsi:noNamespaceSchemaLocation="http://moqui.org/xsd/mr-entity-definition.xsd">\n</entities>\n'
const SEED_TEXT = '<?xml version="1.0" encoding="UTF-8"?>\n<entity-facade-xml type="seed">\n</entity-facade-xml>\n'

// ---- the service view (DevAideService.js) ----

function servicesOf(outline) {
    return children(outline, ["service"]).map(node => ({
        node,
        name: (attrOf(node, "verb") || "") + (attrOf(node, "noun") ? "#" + attrOf(node, "noun") : "")
    }))
}
function parameterXml(parameter) {
    return plainXml(
        "parameter",
        [
            ["name", parameter.name],
            ["type", parameter.type && parameter.type !== "String" ? parameter.type : ""],
            ["required", parameter.required ? "true" : ""],
            ["default-value", parameter.defaultValue]
        ],
        parameter.description ? "<description>" + escape(parameter.description) + "</description>" : ""
    )
}
// A parameter added last in in-parameters or out-parameters (made when missing), or with `parent` (a name path, as
// parameterNode takes it) last inside that parameter, a Map's or a List's: after its parameters, before its
// validations, as the schema orders them. None when the parent is not there.
function addParameter(service, which, parameter, parent) {
    const holder = children(service, [which])[0]
    const text = parameterXml(parameter)
    if (parent && (!Array.isArray(parent) || parent.length)) {
        const node = parameterNode(service, which, parent)
        if (!node) return []
        const nested = children(node, ["parameter", "auto-parameters"])
        if (nested.length) return [{op: "insert", at: last(nested).path, position: "after", xml: text}]
        const validation = children(node).find(child => child.name !== "description")
        return validation
            ? [{op: "insert", at: validation.path, position: "before", xml: text}]
            : [{op: "insert", at: node.path, position: "last", xml: text}]
    }
    if (holder) return [{op: "insert", at: holder.path, position: "last", xml: text}]
    const before = children(service, which === "in-parameters" ? ["out-parameters", "actions"] : ["actions"])[0]
    const made = plainXml(which, [], text)
    return before
        ? [{op: "insert", at: before.path, position: "before", xml: made}]
        : [{op: "insert", at: service.path, position: "last", xml: made}]
}
// A parameter of in-parameters or out-parameters by its name, or by its path of names (["order", "items"] or
// "order.items"), each a parameter of the one before.
function parameterNode(service, which, name) {
    const names = (Array.isArray(name) ? name : String(name == null ? "" : name).split(".")).filter(part => part !== "")
    let node = names.length ? children(service, [which])[0] || null : null
    for (const part of names)
        node = node ? children(node, ["parameter"]).find(child => attrOf(child, "name") === part) || null : null
    return node
}
function removeParameter(service, which, name) {
    const node = parameterNode(service, which, name)
    return node ? [{op: "remove", at: node.path}] : []
}
// Edit a parameter: its type, required and default as attributes, its description as its child.
function editParameter(service, which, name, parameter) {
    const node = parameterNode(service, which, name)
    if (!node) return []
    const list = setAttributeOps(node, "type", parameter.type && parameter.type !== "String" ? parameter.type : "")
        .concat(setAttributeOps(node, "required", parameter.required ? "true" : ""))
        .concat(setAttributeOps(node, "default-value", parameter.defaultValue))
    const description = children(node, ["description"])[0]
    if (description)
        list.push(
            parameter.description
                ? {op: "set-text", at: description.path, text: parameter.description}
                : {op: "remove", at: description.path}
        )
    else if (parameter.description)
        list.push({
            op: "insert",
            at: node.path,
            // first, before a Map's or List's parameters, as the schema orders them
            position: children(node).length ? "first" : "last",
            xml: "<description>" + escape(parameter.description) + "</description>"
        })
    return list
}
function secaXml(service, when, condition, call) {
    const body =
        (condition ? "<condition><expression>" + escapeText(condition) + "</expression></condition>" : "") +
        "<actions>" +
        (call
            ? plainXml("service-call", [
                  ["name", call],
                  ["in-map", "context"]
              ])
            : "") +
        "</actions>"
    return plainXml(
        "seca",
        [
            ["service", service],
            ["when", when || "post-service"]
        ],
        body
    )
}
function addRule(outline, ruleXml) {
    return [{op: "insert", at: (outline && outline.path) || [], position: "last", xml: ruleXml}]
}
function tryParameters(values, parameters) {
    const out = {}
    for (const parameter of parameters || []) {
        const value = values[parameter.name]
        if (value === undefined || value === "") continue
        out[parameter.name] =
            /Integer|Long|BigDecimal|Double|Number/.test(parameter.type || "") && value !== "" && !isNaN(Number(value))
                ? Number(value)
                : parameter.type === "Boolean"
                  ? value === true || value === "true"
                  : value
    }
    return out
}
const SECAS_TEXT =
    '<?xml version="1.0" encoding="UTF-8"?>\n<secas xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" ' +
    'xsi:noNamespaceSchemaLocation="http://moqui.org/xsd/mr-service-eca.xsd">\n</secas>\n'

// ---- the screen view (DevAideScreen.js) ----

const ANCHORABLE = [
    "container",
    "container-box",
    "container-row",
    "row-col",
    "container-panel",
    "container-split",
    "container-dialog",
    "section",
    "section-iterate",
    "form-single",
    "form-list",
    "tree",
    "dynamic-container",
    "button-menu",
    "label",
    "link",
    "image"
]
const SCREEN_CONTAINERS = [
    "container",
    "container-box",
    "row-col",
    "section",
    "section-iterate",
    "container-panel",
    "dynamic-container"
]
function anchorName(node) {
    return attrOf(node, "name") || attrOf(node, "id") || ""
}
function widgetsOf(outline) {
    return children(outline, ["widgets"])[0] || null
}
function nearestAnchor(outline, path) {
    for (let depth = path.length; depth > 0; depth--) {
        const parentPath = path.slice(0, depth - 1)
        const parent = outlineAt(outline, parentPath)
        const index = path[depth - 1]
        const siblings = (parent && parent.children) || []
        for (let at = index - 1; at >= 0; at--) {
            if (ANCHORABLE.indexOf(siblings[at].name) >= 0 && anchorName(siblings[at]))
                return {name: anchorName(siblings[at]), where: "after", path: siblings[at].path}
        }
        for (let at = index + 1; at < siblings.length; at++) {
            if (ANCHORABLE.indexOf(siblings[at].name) >= 0 && anchorName(siblings[at]))
                return {name: anchorName(siblings[at]), where: "before", path: siblings[at].path}
        }
        if (parent && SCREEN_CONTAINERS.indexOf(parent.name) >= 0 && anchorName(parent))
            return {name: anchorName(parent), where: "append", path: parent.path}
    }
    return null
}
function extensionPoints(outline) {
    const points = []
    const widgets = widgetsOf(outline)
    if (widgets) {
        walkOutline(widgets, node => {
            if (node === widgets || ANCHORABLE.indexOf(node.name) < 0) return
            const name = anchorName(node)
            const container = SCREEN_CONTAINERS.indexOf(node.name) >= 0 || node.name === "container-row"
            if (!name && !container && !/^form-|^section/.test(node.name)) return
            points.push({
                kind: node.name,
                name,
                path: node.path,
                line: node.line,
                named: !!name,
                nearest: name ? null : nearestAnchor(outline, node.path)
            })
        })
    }
    for (const transition of children(outline, ["transition"])) {
        points.push({
            kind: "transition",
            name: attrOf(transition, "name") || "",
            path: transition.path,
            line: transition.line,
            named: true,
            nearest: null
        })
    }
    return points
}
function nodeAtLine(outline, line, column) {
    let found = null
    walkOutline(outline, node => {
        if (node.line === line && (!column || node.column === column || !found)) found = node
    })
    return found
}
function formOf(outline, path) {
    for (let depth = path.length; depth >= 0; depth--) {
        const node = outlineAt(outline, path.slice(0, depth))
        if (node && (node.name === "form-single" || node.name === "form-list")) return node
    }
    return null
}
function screenFieldOf(outline, path) {
    for (let depth = path.length; depth >= 0; depth--) {
        const node = outlineAt(outline, path.slice(0, depth))
        if (node && node.name === "field") return node
        if (node && (node.name === "form-single" || node.name === "form-list")) return null
    }
    return null
}
const EXTEND_TEXT =
    '<?xml version="1.0" encoding="UTF-8"?>\n<screen-extend xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" ' +
    'xsi:noNamespaceSchemaLocation="http://moqui.org/xsd/mr-xml-screen.xsd">\n</screen-extend>\n'
function fieldWidget(widget) {
    return (
        {
            "text-line": "<text-line/>",
            "text-area": "<text-area/>",
            "date-time": '<date-time type="date"/>',
            check: '<check><option key="Y"/></check>',
            "drop-down": '<drop-down allow-empty="true"/>',
            link: '<link url="." text="${' + '}"/>'
        }[widget] || "<display/>"
    )
}
function formFieldXml(field) {
    return plainXml(
        "field",
        [
            ["name", field.name],
            ["after", field.after]
        ],
        plainXml("default-field", [["title", field.title]], field.widgetXml || fieldWidget(field.widget))
    )
}
function extendFormField(extOutline, formKind, formName, field) {
    const form = children(extOutline, [formKind]).find(node => attrOf(node, "name") === formName)
    const xml = formFieldXml(field)
    if (form) return [{op: "insert", at: form.path, position: "last", xml}]
    return [{op: "insert", at: extOutline.path || [], position: "last", xml: plainXml(formKind, [["name", formName]], xml)}]
}
function extendWidgets(extOutline, anchor, where, widgetXml) {
    const existing = children(extOutline, ["widgets-extend"]).find(
        node => attrOf(node, "name") === anchor && (attrOf(node, "where") || "") === where
    )
    if (existing) return [{op: "insert", at: existing.path, position: "last", xml: widgetXml}]
    return [
        {
            op: "insert",
            at: extOutline.path || [],
            position: "last",
            xml: plainXml(
                "widgets-extend",
                [
                    ["name", anchor],
                    ["where", where]
                ],
                widgetXml
            )
        }
    ]
}
function insertFormField(form, field, afterName) {
    const fields = children(form, ["field"])
    const anchor = (afterName && fields.find(node => attrOf(node, "name") === afterName)) || fields[fields.length - 1]
    const xml = formFieldXml({name: field.name, title: field.title, widget: field.widget, widgetXml: field.widgetXml})
    const list = [
        anchor ? {op: "insert", at: anchor.path, position: "after", xml} : {op: "insert", at: form.path, position: "first", xml}
    ]
    const layout = children(form, ["field-layout"])[0]
    const findRef = (node, name) => {
        let found = null
        walkOutline(node, child => {
            if (!found && child.name === "field-ref" && attrOf(child, "name") === name) found = child
        })
        return found
    }
    const refXml = plainXml("field-ref", [["name", field.name]])
    if (layout) {
        const ref = afterName ? findRef(layout, afterName) : null
        list.push(
            ref
                ? {op: "insert", at: ref.path, position: "after", xml: refXml}
                : {op: "insert", at: layout.path, position: "last", xml: refXml}
        )
    }
    const columns = children(form, ["form-list-column"])
    if (columns.length) {
        const column = afterName ? columns.find(node => findRef(node, afterName)) : null
        list.push({
            op: "insert",
            at: (column || columns[columns.length - 1]).path,
            position: "after",
            xml: plainXml("form-list-column", [], refXml)
        })
    }
    return list
}
function copyInto(extOutline, text, node) {
    const copied = String(text || "").slice(node.from16, node.to16)
    return copied ? [{op: "insert", at: extOutline.path || [], position: "last", xml: copied}] : []
}
function actionsExtend(extOutline, when) {
    return [
        {
            op: "insert",
            at: extOutline.path || [],
            position: "last",
            xml: plainXml(
                "actions-extend",
                [
                    ["type", "actions"],
                    ["when", when]
                ],
                '<log level="info" message="' + when + " the screen's actions\"/>"
            )
        }
    ]
}
function listFindOf(outline, listName) {
    let found = null
    if (!listName) return null
    walkOutline(outline, node => {
        if (!found && (node.name === "entity-find" || node.name === "entity-find-count") && attrOf(node, "list") === listName)
            found = node
    })
    return found
}
function hasSearchInputs(find) {
    return !!(find && children(find, ["search-form-inputs"]).length)
}
function filterXml(fieldName) {
    return plainXml("field", [["name", fieldName]], '<header-field show-order-by="true"><text-find/></header-field>')
}
function filterOps(inPlace, outline, form, fieldName) {
    if (!inPlace) {
        const merge = children(outline, ["form-list"]).find(node => attrOf(node, "name") === attrOf(form, "name"))
        return merge
            ? [{op: "insert", at: merge.path, position: "last", xml: filterXml(fieldName)}]
            : [
                  {
                      op: "insert",
                      at: outline.path || [],
                      position: "last",
                      xml: plainXml("form-list", [["name", attrOf(form, "name")]], filterXml(fieldName))
                  }
              ]
    }
    const field = children(form, ["field"]).find(node => attrOf(node, "name") === fieldName)
    if (!field)
        return [
            {
                op: "insert",
                at: form.path,
                position: "last",
                xml: plainXml(
                    "field",
                    [["name", fieldName]],
                    '<header-field show-order-by="true"><text-find/></header-field><default-field><display/></default-field>'
                )
            }
        ]
    if (children(field, ["header-field"]).length) return []
    return [
        {op: "insert", at: field.path, position: "first", xml: '<header-field show-order-by="true"><text-find/></header-field>'}
    ]
}
function sectionXml(spec) {
    const name = spec.name || "NewSection"
    if (spec.kind === "related") {
        const list = (spec.entity.split(".").pop() || "related").replace(/^./, ch => ch.toLowerCase()) + "List"
        const find = plainXml(
            "entity-find",
            [
                ["entity-name", spec.entity],
                ["list", list]
            ],
            plainXml("econdition", [
                ["field-name", spec.keyField],
                ["from", spec.keyFrom || spec.keyField]
            ])
        )
        const formList = plainXml(
            "form-list",
            [
                ["name", name + "List"],
                ["list", list],
                ["skip-form", "true"]
            ],
            plainXml("auto-fields-entity", [
                ["entity-name", spec.entity],
                ["field-type", "display"]
            ])
        )
        return plainXml(
            "section",
            [["name", name]],
            "<actions>" +
                find +
                "</actions><widgets>" +
                plainXml(
                    "container-box",
                    [["name", name + "Box"]],
                    plainXml("box-header", [["title", spec.title || name]]) + "<box-body>" + formList + "</box-body>"
                ) +
                "</widgets>"
        )
    }
    return plainXml(
        "container-box",
        [["name", name]],
        plainXml("box-header", [["title", spec.title || name]]) + "<box-body></box-body>"
    )
}
function transitionXml(spec) {
    return plainXml(
        "transition",
        [["name", spec.transition]],
        plainXml("service-call", [
            ["name", spec.service],
            ["in-map", "true"]
        ]) + '<default-response url="."/>'
    )
}
function linkXml(spec) {
    return plainXml("link", [
        ["url", spec.transition],
        ["text", spec.text],
        ["btn-type", "primary"]
    ])
}
function transitionOp(outline, xml) {
    const next = children(outline, ["subscreens", "pre-actions", "always-actions", "actions", "widgets"])[0]
    return next
        ? {op: "insert", at: next.path, position: "before", xml}
        : {op: "insert", at: outline.path || [], position: "last", xml}
}
function buttonOps(extension, outline, spec, insertAt, node) {
    const transition = transitionXml(spec)
    if (extension)
        return [{op: "insert", at: outline.path || [], position: "last", xml: transition}].concat(insertAt(linkXml(spec)))
    const first = transitionOp(outline, transition)
    const path = node.path.slice()
    if (first.position === "before" && first.at.length === 1 && path[0] >= first.at[0]) path[0]++
    return [first].concat(insertAt(linkXml(spec)).map(op => Object.assign({}, op, {at: path})))
}
function rowActionXml(spec) {
    return plainXml(
        "field",
        [["name", spec.name || "rowAction"]],
        plainXml(
            "default-field",
            [["title", ""]],
            plainXml("link", [
                ["url", spec.transition],
                ["text", spec.text],
                ["link-type", "anchor"],
                ["parameter-map", spec.keyField ? "{" + spec.keyField + ": " + spec.keyField + "}" : ""]
            ])
        )
    )
}
function textAttribute(node) {
    return (
        (node &&
            {label: "text", link: "text", "box-header": "title", "default-field": "title", "container-box": null}[node.name]) ||
        null
    )
}
const SNIPPETS = [
    {key: "label", label: "Label", xml: '<label text="New label" type="p"/>'},
    {key: "link", label: "Link", xml: '<link url="." text="New link" btn-type="default"/>'},
    {
        key: "box",
        label: ec.l10n.localize("Box with header and body"),
        xml: '<container-box name="NewBox"><box-header title="New box"/><box-body></box-body></container-box>'
    },
    {key: "container", label: "Container", xml: '<container name="NewContainer"></container>'},
    {
        key: "row",
        label: ec.l10n.localize("Row of two columns"),
        xml: '<container-row name="NewRow"><row-col md="6"></row-col><row-col md="6"></row-col></container-row>'
    },
    {
        key: "section",
        label: ec.l10n.localize("Section with a condition"),
        xml: '<section name="NewSection" condition="true"><widgets></widgets></section>'
    },
    {key: "dialog", label: ec.l10n.localize("Dynamic dialog"), xml: '<dynamic-dialog id="NewDialog" button-text="Open" transition="."/>'},
    {key: "menu", label: ec.l10n.localize("Button menu"), xml: '<button-menu text="More"></button-menu>'},
    {key: "status", label: ec.l10n.localize("Status bar"), xml: "<status-bar/>"}
]
function labelOf(node) {
    const chips = []
    for (const key of ["name", "id", "text", "title", "location", "url", "list", "where"]) {
        const value = attrOf(node, key)
        if (value) chips.push({text: key === "name" || key === "id" ? value : key + "=" + value, title: key})
        if (chips.length >= 2) break
    }
    return {text: node.name, chips}
}
// A block's move up or down among its siblings (the browser's Alt+Up and Alt+Down, blocks.stepOp).
function stepOp(outline, path, step) {
    const parent = outlineAt(outline, path.slice(0, -1))
    const siblings = (parent && parent.children) || []
    const at = path[path.length - 1]
    const other = siblings[at + (step < 0 ? -1 : 1)]
    return other ? [{op: "move", at: path, to: other.path, position: step < 0 ? "before" : "after"}] : []
}
// A field's XML into a form: in the site's screen-extend, the form merge of that name (made when missing); in place,
// after the form's last field (the terminal's Add row action).
function formFieldOps(extension, outline, form, xml) {
    if (extension) {
        const merge = children(outline, [form.name]).find(node => attrOf(node, "name") === attrOf(form, "name"))
        return merge
            ? [{op: "insert", at: merge.path, position: "last", xml}]
            : [
                  {
                      op: "insert",
                      at: outline.path || [],
                      position: "last",
                      xml: plainXml(form.name, [["name", attrOf(form, "name")]], xml)
                  }
              ]
    }
    const fields = children(form, ["field"])
    return fields.length
        ? [{op: "insert", at: fields[fields.length - 1].path, position: "after", xml}]
        : [{op: "insert", at: form.path, position: "first", xml}]
}
// Widgets placed before, after or inside an element: in place at the element (last in the widgets with none); in the
// site's screen-extend at the element's own anchor, else the nearest one (question 14: anchors are never generated).
function placeOps(extension, base, extOutline, node, where, xml) {
    if (!extension) {
        if (!node || !node.path.length) {
            const widgets = widgetsOf(base)
            return [{op: "insert", at: widgets ? widgets.path : [], position: "last", xml}]
        }
        return [
            {op: "insert", at: node.path, position: where === "inside" ? "last" : where === "before" ? "before" : "after", xml}
        ]
    }
    const own = node && ANCHORABLE.indexOf(node.name) >= 0 ? anchorName(node) : ""
    if (own) return extendWidgets(extOutline, own, where === "inside" ? "append" : where === "before" ? "before" : "after", xml)
    const nearest = node && node.path.length ? nearestAnchor(base, node.path) : null
    return nearest ? extendWidgets(extOutline, nearest.name, nearest.where, xml) : []
}

// The outline's elements as rows of the tree component: key the path joined, the parent's key, a label of its name
// and naming attributes, extension points marked.
function outlineRows(outline, openUrl) {
    const rows = []
    const anchors = new Set(
        extensionPoints(outline)
            .filter(point => point.named)
            .map(point => point.path.join("."))
    )
    walkOutline(outline, node => {
        if (!node.path.length) return
        const key = node.path.join(".")
        const label = labelOf(node)
        rows.push({
            key,
            parent: node.path.slice(0, -1).join(".") || "",
            label:
                (anchors.has(key) ? "◆ " : "") +
                label.text +
                (label.chips.length ? " " + label.chips.map(chip => chip.text).join(" ") : ""),
            _open: openUrl ? openUrl(key) : undefined
        })
    })
    return rows
}

;({
    pure: {attrOf, outlineAt, walkOutline, children, setAttributeOps},
    entity: {
        xml: entityXml,
        escape,
        escapeText,
        shortName,
        entityElements,
        findEntityElement,
        childNamed,
        ensureExtend,
        fieldXml,
        addField,
        setFieldAttribute,
        setFieldDescription,
        removeField,
        moveField,
        stepField,
        relationshipXml,
        insertRelationship,
        addRelationship,
        removeRelationship,
        indexXml,
        addIndex,
        removeIndex,
        picklist,
        status,
        insertRecords,
        recordTables,
        setRecordValue,
        setRecordValues,
        recordAt,
        addRecord,
        fieldOf,
        mergeFields,
        kindOf,
        typeOfKind,
        camel,
        titleOf,
        localizedXml,
        fieldNaming,
        valuesOf,
        addFieldSpec,
        KINDS,
        ENTITIES_TEXT,
        SEED_TEXT
    },
    service: {
        xml: plainXml,
        servicesOf,
        parameterXml,
        addParameter,
        parameterNode,
        removeParameter,
        editParameter,
        secaXml,
        addRule,
        tryParameters,
        SECAS_TEXT
    },
    screen: {
        xml: plainXml,
        ANCHORABLE,
        CONTAINERS: SCREEN_CONTAINERS,
        anchorName,
        widgetsOf,
        nearestAnchor,
        extensionPoints,
        nodeAtLine,
        formOf,
        fieldOf: screenFieldOf,
        EXTEND_TEXT,
        fieldWidget,
        fieldXml: formFieldXml,
        extendFormField,
        extendWidgets,
        insertFormField,
        copyInto,
        actionsExtend,
        listFindOf,
        hasSearchInputs,
        filterXml,
        filterOps,
        sectionXml,
        transitionXml,
        linkXml,
        transitionOp,
        buttonOps,
        rowActionXml,
        textAttribute,
        SNIPPETS,
        labelOf,
        stepOp,
        formFieldOps,
        placeOps,
        outlineRows
    }
})
