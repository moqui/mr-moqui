// What the parts of org.moqui.admin.ConfigurationServices share (mr-boss plan/design-admin.md §17.2; impl-admin P8): each part
// is a module of its own beside this one (ConfigurationSettings.js the instance settings, ConfigurationLocalization.js the
// translations; identity providers, connections, protocols and message types add theirs), and each service checks its own
// permission with ec.user.hasPermission, never a group name. A refusal a person can act on (an unregistered key, a value out of
// range, a stale revision) is answered as a reason (applied false, reasonCode, reason), never written; a missing permission or
// a malformed request is an error. No dependency on Mantle or an app.
const find = (entity, conditions = {}) => ec.entity.find(entity).condition(conditions).disableAuthz()
const one = (entity, conditions) => find(entity, conditions).one()
const rows = (entity, conditions) => Array.from(find(entity, conditions).list())
const map = row => (row?.getMap ? row.getMap() : Object.assign({}, row || {}))
const ms = value =>
    value == null || value === ""
        ? null
        : value instanceof Date
          ? value.getTime()
          : /^-?\d+$/.test(String(value))
            ? Number(value)
            : Date.parse(String(value))
const iso = value => (value == null ? null : new Date(ms(value)).toISOString())
// A row's revision: its lastUpdatedStamp, as the caller read it; "" for no row.
const stamp = row => String(ms(row?.lastUpdatedStamp) ?? "")
const text = value => String(value ?? "").trim()

function signedIn() {
    if (!ec.user.userId) throw new Error("Sign in to administer the configuration")
}
// A read needs the view permission or its manage permission; a change the manage permission.
function requireView(view, manage) {
    signedIn()
    if (!ec.user.hasPermission(view) && !ec.user.hasPermission(manage)) throw new Error(view + " is required")
}
function requireManage(manage) {
    signedIn()
    if (!ec.user.hasPermission(manage)) throw new Error(manage + " is required")
}
const refused = (reasonCode, reason, extra = {}) => Object.assign({applied: false, reasonCode, reason}, extra)
function reasonText(p) {
    const reason = text(p.reason)
    if (reason.length > 255) throw new Error("Give a reason of at most 255 characters")
    return reason || null
}
// The reason the audit log records beside an audit-logged field the change writes ({field}_changeReason, as Java).
function auditReason(reason, fields) {
    if (!reason) return
    ec.context.putAll(Object.fromEntries(fields.map(field => [field + "_changeReason", reason])))
}
// An audit row the entity's own audit log does not write: a removal (the audit log keeps no delete), or a change of a field
// that is not audit-logged for every writer (a translation, which data files load by the thousand). It carries the session
// (visit) it was made in, as the entity's own audit rows do, so org.moqui.admin.AuditServices can show what one session did.
function auditRow(entityName, fieldName, primary, secondary, oldValue, newValue, reason, rest = null) {
    ec.service
        .sync()
        .name("create#moqui.entity.EntityAuditLog")
        .parameters({
            changedEntityName: entityName,
            changedFieldName: fieldName,
            pkPrimaryValue: primary,
            pkSecondaryValue: secondary,
            pkRestCombinedValue: rest,
            oldValueText: oldValue ?? null,
            newValueText: newValue ?? null,
            changedDate: ec.user.nowTimestamp,
            changedByUserId: ec.user.userId,
            changedInVisitId: ec.user.visitId || null,
            changeReason: reason
        })
        .disableAuthz()
        .call()
}
// A page of a list a service read whole: pageIndex from 0, pageSize 1 to 100 (50 when not given).
function page(list, p) {
    const pageSize = Math.min(Math.max(Number(p.pageSize) || 50, 1), 100)
    const pageIndex = Math.max(Number(p.pageIndex) || 0, 0)
    return {rows: list.slice(pageIndex * pageSize, (pageIndex + 1) * pageSize), count: list.length, pageIndex, pageSize}
}

// ----- what the parts that store connection settings share (identity providers, connections) -----
const controls = /[\u0000-\u001f\u007f]/
function plainText(value, what, maxLength = 255) {
    const found = text(value)
    if (controls.test(found) || found.includes("${")) throw new Error(what + " may not hold control characters or ${")
    if (found.length > maxLength) throw new Error(what + " is longer than " + maxLength + " characters")
    return found
}
// An https URL with a host and no credentials or fragment; http only for this machine on an instance that is not
// production (a provider run beside a development server), so a production server is never pointed at its own ports.
function urlText(value, what) {
    const found = plainText(value, what)
    if (!found) return ""
    const parts = /^(https?):\/\/([^\/?#@\s]+)(\/[^\s#]*)?(\?[^\s#]*)?$/.exec(found)
    if (!parts) throw new Error(what + " is not an http(s) URL with a host and no credentials or fragment")
    const host = parts[2].replace(/:\d+$/, "").toLowerCase()
    const local = ["localhost", "127.0.0.1", "[::1]"].includes(host)
    if (parts[1] === "http" && (!local || text(System.getProperty("instance_purpose")) === "production"))
        throw new Error(what + " must be https")
    return found
}
function enumValue(value, enumTypeId, what) {
    const found = text(value)
    if (found && !one("moqui.basic.Enumeration", {enumId: found, enumTypeId}))
        throw new Error(what + " is not one of its options")
    return found
}
// Create or update a row with the fields given (a null clears a field).
function write(entityName, fields, create) {
    const value = ec.entity.makeValue(entityName)
    value.setFields(fields, true, null, null)
    if (create) value.create()
    else value.update()
}

;({
    find,
    one,
    rows,
    map,
    ms,
    iso,
    stamp,
    text,
    signedIn,
    requireView,
    requireManage,
    refused,
    reasonText,
    auditReason,
    auditRow,
    page,
    plainText,
    urlText,
    enumValue,
    write
})
