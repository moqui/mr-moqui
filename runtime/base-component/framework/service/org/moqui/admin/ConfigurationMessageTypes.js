// The message type part of org.moqui.admin.ConfigurationServices (mr-boss plan/design-admin.md §8 MessageTypes and
// MessageTypeDetail, §14; impl-admin P13): the message contracts of the instance (moqui.service.message.SystemMessageType)
// for the integration administrator, who reads every type and changes only what a type's owner registered.
//
// What is changed. A type's send and consume services are executable configuration: here each is set only to a handler
// its owner registered for that one type and role (moqui.server.MessageHandler), chosen by the registration's key. No
// service name is taken from a request, a handler is never cleared, and a type with no registered handler is read only: its
// handlers are its owner's deployment or an instance administrator's in Tools. The service that receives a type's messages
// is never chosen here: it is how the instance takes what a remote posts (the remote's inbound sign-in is not changed in
// delegated administration either, ConfigurationConnections.js). For a type that is changed here, its name, content type
// and file transport options (where files are put and looked for on its remote, the file pattern, what is done with a
// received file) are too. A send path is expanded for each message, so it may hold only the names the sender gives
// (systemMessageId, remoteMessageId, systemMessageTypeId, systemMessageRemoteId, date, dateTime) and no other ${}; the other
// paths and the pattern hold none. A change names the revision it read and leaves an audit row for each field.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationLib.js")
const recovery = ec.resource.script("component://framework/service/org/moqui/admin/OperationRecovery.js")
const {find, one, rows, map, stamp, text, refused} = lib
const VIEW = "ADMIN_CONNECTION_VIEW"
const MANAGE = "ADMIN_CONNECTION_MANAGE"
const TYPE = "moqui.service.message.SystemMessageType"
const HANDLER = "moqui.server.MessageHandler"
const REMOTE = "moqui.service.message.SystemMessageRemote"
const MESSAGE = "moqui.service.message.SystemMessage"
const roles = {
    send: {field: "sendServiceName", text: "Send", parameter: "sendHandlerKey"},
    consume: {field: "consumeServiceName", text: "Consume", parameter: "consumeHandlerKey"}
}
const pathNames = ["systemMessageId", "remoteMessageId", "systemMessageTypeId", "systemMessageRemoteId", "date", "dateTime"]
// the counts of a type's messages read this far back
const countDays = 14

// The registered handlers in use: Map systemMessageTypeId -> {send: [], consume: []}. Each is checked when read: one with no
// type, another role or a service that is not defined is logged by name and left out.
function registeredHandlers() {
    const byType = new Map()
    for (const row of rows(HANDLER)) {
        if (row.disabled === "Y") continue
        const handler = map(row)
        const typeId = text(handler.systemMessageTypeId)
        const role = text(handler.handlerRole)
        if (
            !typeId ||
            !Object.hasOwn(roles, role) ||
            !text(handler.serviceName) ||
            !ec.service.getServiceDefinition(text(handler.serviceName))
        ) {
            ec.logger.error("Message handler " + handler.handlerKey + " names no type, role or defined service; left out")
            continue
        }
        if (!byType.has(typeId)) byType.set(typeId, {send: [], consume: []})
        byType.get(typeId)[role].push(handler)
    }
    for (const found of byType.values())
        for (const role of Object.keys(roles))
            found[role].sort(
                (a, b) =>
                    Number(a.sequenceNum ?? 9999) - Number(b.sequenceNum ?? 9999) || a.handlerKey.localeCompare(b.handlerKey)
            )
    return byType
}
const directionOf = type => {
    const out = !!(text(type.sendServiceName) || text(type.sendPath))
    const incoming = !!(text(type.consumeServiceName) || text(type.receiveServiceName) || text(type.receivePath))
    return out && incoming ? "both" : out ? "out" : incoming ? "in" : "none"
}
const directionTexts = {both: "Sent and received", out: "Sent", in: "Received", none: "Not routed"}
const modeText = (mode, owner) =>
    mode === "native" ? "By the message service" : mode === "owner" ? "By " + owner : "Not offered"
// A type as the list and the detail show it.
function described(type, handlers, descriptors, canManage) {
    const typeId = type.systemMessageTypeId
    const registered = handlers.get(typeId) || {send: [], consume: []}
    const own = descriptors.filter(descriptor => text(descriptor.targetKey) === typeId)
    const configurable = registered.send.length + registered.consume.length > 0
    const handlerText = role => {
        const current = text(type[roles[role].field])
        const handler = registered[role].find(candidate => text(candidate.serviceName) === current)
        return handler ? text(handler.title) || handler.handlerKey : current
    }
    const owners = Array.from(
        new Set(
            registered.send
                .concat(registered.consume)
                .map(handler => text(handler.ownerApp))
                .concat(own.map(descriptor => recovery.ownerName(descriptor)))
                .filter(Boolean)
        )
    )
    const direction = directionOf(type)
    return {
        systemMessageTypeId: typeId,
        description: text(type.description) || typeId,
        contentType: text(type.contentType) || null,
        direction,
        directionText: directionTexts[direction],
        sendHandlerText: handlerText("send") || null,
        consumeHandlerText: handlerText("consume") || null,
        receiveHandlerText: text(type.receiveServiceName) || null,
        ownerText: owners.join(", ") || null,
        recoveryText: own.length
            ? own
                  .map(descriptor =>
                      [
                          text(descriptor.retryMode) && "retry",
                          text(descriptor.reconcileMode) && "settle",
                          text(descriptor.cancelMode) && "cancel"
                      ]
                          .filter(Boolean)
                          .join(", ")
                  )
                  .filter(Boolean)
                  .join("; ") || "Registered, no action"
            : null,
        sendPath: text(type.sendPath) || null,
        receivePath: text(type.receivePath) || null,
        receiveFilePattern: text(type.receiveFilePattern) || null,
        receiveMovePath: text(type.receiveMovePath) || null,
        receiveResponseEnumId: text(type.receiveResponseEnumId) || null,
        configurable,
        canChange: canManage && configurable,
        readOnlyReason: configurable
            ? null
            : "Its owner registered no handler for delegated administration: it is changed by its owner's deployment, or by an instance administrator",
        revision: stamp(type)
    }
}

// find#MessageTypes: every message type with its direction, handlers, owner and whether it is changed here.
function findMessageTypes(p) {
    lib.requireView(VIEW, MANAGE)
    const canManage = ec.user.hasPermission(MANAGE)
    const handlers = registeredHandlers()
    const descriptors = recovery.descriptorsOf("SystemMessage")
    const term = text(p.term).toLowerCase()
    const all = rows(TYPE)
        .map(map)
        .map(type => described(type, handlers, descriptors, canManage))
        .filter(
            row =>
                (!term || row.systemMessageTypeId.toLowerCase().includes(term) || row.description.toLowerCase().includes(term)) &&
                (!text(p.direction) || row.direction === text(p.direction) || row.direction === "both")
        )
        .sort((a, b) => a.description.localeCompare(b.description) || a.systemMessageTypeId.localeCompare(b.systemMessageTypeId))
    const paged = lib.page(all, p)
    return {typeList: paged.rows, typeCount: paged.count, pageIndex: paged.pageIndex, pageSize: paged.pageSize, canManage}
}

// get#MessageTypeUse: one type's contract and where it is used: its handlers with the registered choices, the remotes of
// this type, what is registered for the recovery of its messages, and how many messages it has by status.
function getMessageTypeUse(p) {
    lib.requireView(VIEW, MANAGE)
    const canManage = ec.user.hasPermission(MANAGE)
    const typeId = text(p.systemMessageTypeId)
    const type = typeId ? one(TYPE, {systemMessageTypeId: typeId}) : null
    if (!type) throw new Error("No message type " + typeId)
    const handlers = registeredHandlers()
    const descriptors = recovery.descriptorsOf("SystemMessage")
    const registered = handlers.get(typeId) || {send: [], consume: []}
    const current = map(type)
    const since = new Date(lib.ms(ec.user.nowTimestamp) - countDays * 86400000).toISOString()
    const statuses = Array.from(find("moqui.basic.StatusItem", {statusTypeId: "SystemMessage"}).orderBy("sequenceNum").list())
    return {
        messageType: described(current, handlers, descriptors, canManage),
        handlerList: Object.entries(roles)
            .map(([role, spec]) => {
                const service = text(current[spec.field])
                return {
                    handlerRole: role,
                    roleText: spec.text,
                    currentService: service || null,
                    currentHandlerKey:
                        registered[role].find(handler => text(handler.serviceName) === service)?.handlerKey || null,
                    changeable: registered[role].length > 0,
                    note: registered[role].length ? null : "No handler is registered for this: it stays as its owner deployed it",
                    choiceList: registered[role].map(handler => ({
                        handlerKey: handler.handlerKey,
                        title: text(handler.title) || handler.handlerKey,
                        description: text(handler.description) || null,
                        ownerApp: text(handler.ownerApp) || null,
                        current: text(handler.serviceName) === service
                    }))
                }
            })
            .concat([
                {
                    handlerRole: "receive",
                    roleText: "Receive",
                    currentService: text(current.receiveServiceName) || null,
                    currentHandlerKey: null,
                    changeable: false,
                    note: "How the instance takes what a remote posts is not chosen here",
                    choiceList: []
                }
            ]),
        producerText: text(current.produceServiceName) || null,
        remoteList: rows(REMOTE, {systemMessageTypeId: typeId}).map(remote => ({
            systemMessageRemoteId: remote.systemMessageRemoteId,
            description: text(remote.description) || remote.systemMessageRemoteId,
            disabled: remote.disabled === "Y"
        })),
        recoveryList: descriptors
            .filter(descriptor => text(descriptor.targetKey) === typeId)
            .map(descriptor => {
                const owner = recovery.ownerName(descriptor)
                return {
                    descriptorKey: descriptor.descriptorKey,
                    ownerApp: owner,
                    title: text(descriptor.title) || descriptor.descriptorKey,
                    directionText:
                        text(descriptor.direction) === "in" ? "Received" : text(descriptor.direction) === "out" ? "Sent" : "Both",
                    retryText: modeText(text(descriptor.retryMode), owner),
                    reconcileText: modeText(text(descriptor.reconcileMode), owner),
                    cancelText: modeText(text(descriptor.cancelMode), owner),
                    ownerPermission: text(descriptor.managePermission) || null,
                    dataPermission: text(descriptor.dataPermission) || null,
                    idempotencyNote: text(descriptor.idempotencyNote) || null,
                    ownerRoute: text(descriptor.ownerRoute) || null
                }
            }),
        countDays,
        countList: statuses
            .map(status => ({
                statusId: status.statusId,
                statusText: text(status.description) || status.statusId,
                messageCount: Number(
                    find(MESSAGE, {systemMessageTypeId: typeId, statusId: status.statusId})
                        .condition("initDate", "greater-equals", since)
                        .count()
                )
            }))
            .filter(row => row.messageCount > 0),
        receiveResponseList: rows("moqui.basic.Enumeration", {enumTypeId: "MessageReceiveResponse"}).map(row => ({
            enumId: row.enumId,
            description: text(row.description) || row.enumId
        })),
        canManage
    }
}

// ===== changing =====
// A path on a remote file server: plain text with no parent segment, and only the names the sender gives when it is
// expanded (a send path), none otherwise.
function pathText(value, what, expanded) {
    const found = text(value)
    if (/[\u0000-\u001f\u007f]/.test(found)) throw new Error(what + " may not hold control characters")
    if (found.length > 255) throw new Error(what + " is longer than 255 characters")
    if (/(^|[\/\\])\.\.([\/\\]|$)/.test(found)) throw new Error(what + " may not hold a .. segment")
    const rest = expanded ? pathNames.reduce((left, name) => left.split("${" + name + "}").join(""), found) : found
    if (rest.includes("${"))
        throw new Error(
            what +
                (expanded
                    ? " may hold only the names " + pathNames.map(name => "${" + name + "}").join(", ")
                    : " may not hold ${")
        )
    return found
}
function contentTypeText(value) {
    const found = lib.plainText(value, "The content type", 120)
    if (found && !/^[\w.+-]+\/[\w.+-]+(\s*;\s*[\w-]+=[\w.-]+)*$/.test(found))
        throw new Error("The content type is not a media type")
    return found
}
function patternText(value) {
    const found = lib.plainText(value, "The file pattern", 120)
    if (!found) return ""
    try {
        new RegExp(found)
    } catch (error) {
        throw new Error("The file pattern is not a regular expression")
    }
    return found
}
// The fields of a type that are changed here, each with how its value is read. A parameter that is not given (null) is
// left as it is; an empty text clears an optional one.
const fields = {
    description: value => {
        const found = lib.plainText(value, "The name")
        if (!found) throw new Error("A message type has a name")
        return found
    },
    contentType: contentTypeText,
    sendPath: value => pathText(value, "The send path", true),
    receivePath: value => pathText(value, "The receive path", false),
    receiveFilePattern: patternText,
    receiveMovePath: value => pathText(value, "The path received files are moved to", false),
    receiveResponseEnumId: value => lib.enumValue(value, "MessageReceiveResponse", "What is done with a received file")
}

// store#MessageType: change a registered type's transport options and choose its send or consume handler among those its
// owner registered for it.
function storeMessageType(p) {
    lib.requireManage(MANAGE)
    const typeId = text(p.systemMessageTypeId)
    const row = typeId ? find(TYPE, {systemMessageTypeId: typeId}).forUpdate(true).one() : null
    if (!row) throw new Error("No message type " + typeId)
    const current = map(row)
    const revision = stamp(current)
    const registered = registeredHandlers().get(typeId)
    if (!registered)
        return refused(
            "NOT_CONFIGURABLE",
            "This message type is not changed here: its owner registered no handler for delegated administration",
            {revision}
        )
    if (text(p.expectedRevision) !== revision)
        return refused("STALE_REVISION", "The message type changed since it was read; read it again and repeat", {revision})
    const reason = lib.reasonText(p)
    const changes = {}
    for (const [name, read] of Object.entries(fields)) {
        if (p[name] == null) continue
        // a value given as it was read is no change, whatever today's validation says of it
        if (text(p[name]) === text(current[name])) continue
        changes[name] = read(p[name]) || null
    }
    for (const [role, spec] of Object.entries(roles)) {
        const handlerKey = text(p[spec.parameter])
        if (!handlerKey) continue
        const handler = registered[role].find(candidate => candidate.handlerKey === handlerKey)
        if (!handler)
            return refused(
                "HANDLER_NOT_REGISTERED",
                "That is not a handler registered to " + spec.text.toLowerCase() + " this message type",
                {revision}
            )
        if (text(handler.serviceName) !== text(current[spec.field])) changes[spec.field] = text(handler.serviceName)
    }
    const changed = Object.keys(changes).filter(name => (changes[name] ?? "") !== text(current[name]))
    if (!changed.length) return {applied: true, changed: false, systemMessageTypeId: typeId, revision}
    row.setFields(Object.fromEntries(changed.map(name => [name, changes[name]])), true, null, null)
    row.update()
    for (const name of changed) lib.auditRow(TYPE, name, typeId, null, text(current[name]) || null, changes[name], reason)
    return {
        applied: true,
        changed: true,
        changedFieldList: changed,
        systemMessageTypeId: typeId,
        revision: stamp(one(TYPE, {systemMessageTypeId: typeId}))
    }
}

const operations = {findMessageTypes, getMessageTypeUse, storeMessageType}
;({
    registeredHandlers,
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown message type operation " + op)
        return operations[op](parameters)
    }
})
