// The settings part of org.moqui.admin.ConfigurationServices (mr-boss plan/design-admin.md §7 InstanceSettings, §17.2; impl-admin
// P8): the effective instance settings and the instance preferences.
//
// A setting is a row of moqui.server.InstanceSetting, seed data of the component that reads it; nothing else is read or
// written, so there is no unregistered key, JSON or code value. Two kinds:
// - a preference (editable Y): its value is the moqui.server.InstancePreference row when one is set (source preference), else
//   the conf property it names when that has a value (source configuration), else its registered default (source default). It
//   is changed at runtime by store#InstancePreference, typed and within its allowed values or limits, at the revision read,
//   with who, when and why; the audit log keeps each value.
// - deployment configuration (editable N): a default-property of the conf (or the environment variable of that name), shown
//   read only with whether it is the shipped default, whether a change waits for a restart and how an operator changes it.
//   The file and line a value comes from are System > Conf's, for an instance administrator. A setting that names a
//   confAttribute instead (impl-admin P10, F110: the sign-in policy, literal attributes of user-facade and of the web
//   session that no property holds) takes the value the server runs with from get#SignInPolicyValues, which answers a
//   fixed list of nonsecret attributes and nothing else.
// A secret is never a preference and never returned: a setting marked secret, or whose property name reads as one (pass, pw,
// key, secret, token, credential, as the conf masks them), answers only whether it is set, and a registration that makes a
// secret editable is logged by name and left out.
//
// Reading the list needs ADMIN_SETTINGS_VIEW and a change ADMIN_SETTINGS_MANAGE (and the setting's own managePermission, if
// any). get#InstancePreference, the read of one registered preference for the code that uses it (value(key) for a script),
// needs no permission: a preference is nonsecret by registration.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationLib.js")
const {find, one, rows, map, iso, stamp, text, refused} = lib
const SETTING = "moqui.server.InstanceSetting"
const PREFERENCE = "moqui.server.InstancePreference"
const VIEW = "ADMIN_SETTINGS_VIEW"
const MANAGE = "ADMIN_SETTINGS_MANAGE"
const signInGroups = ["Sign-in", "Sign-in policy"]
const signInGroupPermissions = ["ADMIN_ACCESS_VIEW", "ADMIN_ACCESS_MANAGE"]
const valueTypes = {
    IsvText: "text",
    IsvInteger: "integer",
    IsvBoolean: "boolean",
    IsvChoice: "choice",
    IsvLocale: "locale",
    IsvTimeZone: "timeZone"
}
const sourceTexts = {
    preference: "Instance preference",
    configuration: "Deployment configuration",
    default: "Default"
}
const secretName = name => /pass|pw|key|secret|token|credential/i.test(String(name || ""))
// also a property the shipped conf marks is-secret, whatever its name (shippedConf below)
const isSecret = setting =>
    setting.isSecret === "Y" ||
    secretName(setting.propertyName) ||
    secretName(setting.settingKey) ||
    (!!setting.propertyName && shippedConf().secrets.has(setting.propertyName))
const allowedList = setting =>
    text(setting.allowedValues)
        .split(",")
        .map(value => value.trim())
        .filter(Boolean)
const property = name => (name ? text(System.getProperty(name)) : "")

// ===== the registry =====
// A registration in use, or why it is not: each is checked when read, logged by name and left out.
function registrationProblem(setting) {
    if (!valueTypes[setting.valueTypeEnumId]) return "has no known value type"
    if (setting.editable === "Y" && isSecret(setting)) return "is secret and may not be an instance preference"
    if (setting.editable !== "Y" && !setting.propertyName && !setting.confAttribute)
        return "is configuration and names no property or conf attribute"
    if (setting.editable === "Y" && setting.confAttribute) return "is a preference and names a conf attribute"
    if (setting.valueTypeEnumId === "IsvChoice" && !allowedList(setting).length) return "is a choice with no allowed values"
    return null
}
function registered() {
    const found = []
    for (const row of rows(SETTING)) {
        if (row.disabled === "Y") continue
        const setting = map(row)
        const problem = registrationProblem(setting)
        if (problem) {
            ec.logger.error("Instance setting " + setting.settingKey + " " + problem + "; left out")
            continue
        }
        found.push(setting)
    }
    return found.sort(
        (a, b) => Number(a.sequenceNum ?? 9999) - Number(b.sequenceNum ?? 9999) || a.settingKey.localeCompare(b.settingKey)
    )
}
function registeredOne(settingKey) {
    const row = settingKey ? one(SETTING, {settingKey}) : null
    if (!row || row.disabled === "Y") return null
    const setting = map(row)
    const problem = registrationProblem(setting)
    if (problem) {
        ec.logger.error("Instance setting " + setting.settingKey + " " + problem + "; left out")
        return null
    }
    return setting
}

// ===== values =====
// A value as its type reads it: {value} the canonical text, or {problem} what is wrong with it.
function typed(setting, given) {
    const raw = text(given)
    const type = valueTypes[setting.valueTypeEnumId]
    if (/[\u0000-\u001f\u007f]/.test(raw)) return {problem: "has a control character"}
    if (type === "integer") {
        if (!/^-?\d{1,9}$/.test(raw)) return {problem: "is not a whole number"}
        const number = Number(raw)
        if (setting.minValue != null && number < Number(setting.minValue)) return {problem: "is less than " + setting.minValue}
        if (setting.maxValue != null && number > Number(setting.maxValue)) return {problem: "is more than " + setting.maxValue}
        return {value: String(number)}
    }
    if (type === "boolean") {
        const value = {true: "true", y: "true", yes: "true", false: "false", n: "false", no: "false"}[raw.toLowerCase()]
        return value ? {value} : {problem: "is not true or false"}
    }
    if (type === "choice")
        return allowedList(setting).includes(raw) ? {value: raw} : {problem: "is not one of " + allowedList(setting).join(", ")}
    if (type === "locale") {
        const locales = Array.from(ec.l10n.getAvailableLocales() || []).map(row => String(row.locale))
        const allowed = allowedList(setting)
        if (!/^[a-z]{2,3}(_[A-Z]{2})?$/.test(raw) || !locales.includes(raw)) return {problem: "is not a locale of this instance"}
        if (allowed.length && !allowed.includes(raw)) return {problem: "is not one of " + allowed.join(", ")}
        return {value: raw}
    }
    if (type === "timeZone") {
        const zones = Array.from(ec.l10n.getAvailableTimeZones() || []).map(String)
        return zones.includes(raw) ? {value: raw} : {problem: "is not a time zone"}
    }
    // text is shown, never run: no expression for a screen that shows it to expand
    if (raw.includes("${")) return {problem: "may not hold ${"}
    const maxLength = Math.min(Number(setting.maxLength) || 255, 255)
    if (raw.length > maxLength) return {problem: "is longer than " + maxLength + " characters"}
    return {value: raw}
}
// The framework default conf's default-properties: their shipped values, to tell a shipped default from a deployment's own
// value, and the names it marks is-secret, which are never returned whatever they are called.
let shipped = null
function shippedConf() {
    if (shipped) return shipped
    shipped = {values: new Map(), secrets: new Set(), text: ""}
    try {
        const conf = String(ec.resource.getLocationText("component://framework/conf/MoquiDefaultConf.xml", false) || "")
        shipped.text = conf
        const unescape = value =>
            value
                .replace(/&lt;/g, "<")
                .replace(/&gt;/g, ">")
                .replace(/&quot;/g, '"')
                .replace(/&apos;/g, "'")
                .replace(/&amp;/g, "&")
        for (const found of conf.matchAll(/<default-property\s([^>]*)>/g)) {
            const attribute = name => (found[1].match(new RegExp("(?:^|\\s)" + name + '="([^"]*)"')) || [])[1]
            const name = attribute("name")
            if (!name) continue
            shipped.values.set(name, unescape(attribute("value") ?? ""))
            if (attribute("is-secret") === "true") shipped.secrets.add(name)
        }
    } catch (error) {
        ec.logger.error("The default conf could not be read for the instance settings: " + error.message)
    }
    return shipped
}
const shippedDefaults = () => shippedConf().values
// The sign-in policy the server runs with, by conf attribute (the Rust service's fixed list), read once a call; empty when
// the caller may not read it or the build has no such service, and each such setting is then left out by name.
let policyValues = null
function confAttributeValues() {
    if (policyValues) return policyValues
    policyValues = {}
    try {
        ec.message.pushErrors()
        const out = ec.service.sync().name("org.moqui.admin.ConfigurationServices.get#SignInPolicyValues").call() || {}
        if (!ec.message.hasError()) policyValues = lib.map(out.valueMap)
        else ec.logger.error("The sign-in policy could not be read: " + ec.message.getErrorsString())
    } catch (error) {
        ec.logger.error("The sign-in policy could not be read: " + error.message)
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
    return policyValues
}
// The shipped conf's value of a conf attribute (parent/element/@name): the attribute of the first such element after its
// parent's opening tag, or null when the shipped conf leaves it out.
function shippedAttribute(confAttribute) {
    const found = /^([\w-]+)\/([\w-]+)\/@([\w-]+)$/.exec(String(confAttribute || ""))
    if (!found) return null
    const conf = shippedConf().text
    const parent = conf.search(new RegExp("<" + found[1] + "[\\s>]"))
    if (parent < 0) return null
    const element = new RegExp("<" + found[2] + "\\s([^>]*)>").exec(conf.slice(parent))
    const attribute = element ? new RegExp("(?:^|\\s)" + found[3] + '="([^"]*)"').exec(element[1]) : null
    return attribute ? attribute[1].trim() : null
}
// A setting's effective value and where it comes from. A stored preference its registration no longer accepts is logged and
// passed over, so a narrowed registration never serves a value outside it.
function effective(setting, preference) {
    if (setting.editable === "Y") {
        if (preference) {
            const stored = typed(setting, preference.preferenceValue)
            if (stored.value != null && text(preference.preferenceValue) !== "")
                return {value: stored.value, source: "preference"}
            ec.logger.error("Instance preference " + setting.settingKey + " " + (stored.problem || "is empty") + "; passed over")
        }
        const configured = property(setting.propertyName)
        if (configured) {
            const read = typed(setting, configured)
            if (read.value != null) return {value: read.value, source: "configuration"}
        }
        return {value: text(setting.defaultValue), source: "default"}
    }
    if (setting.confAttribute) {
        const running = text(confAttributeValues()[setting.confAttribute])
        const shippedValue = shippedAttribute(setting.confAttribute)
        return {value: running, source: running === (shippedValue ?? text(setting.defaultValue)) ? "default" : "configuration"}
    }
    const configured = property(setting.propertyName)
    const defaults = shippedDefaults()
    const isDefault = defaults.has(setting.propertyName) ? defaults.get(setting.propertyName).trim() === configured : !configured
    return {value: configured || text(setting.defaultValue), source: isDefault ? "default" : "configuration"}
}
function described(setting, preference, canManage) {
    const secret = isSecret(setting)
    const found = effective(setting, preference)
    const editable = setting.editable === "Y"
    const type = valueTypes[setting.valueTypeEnumId]
    const changedBy = preference?.changedByUserId ? one("moqui.security.UserAccount", {userId: preference.changedByUserId}) : null
    return {
        settingKey: setting.settingKey,
        title: setting.title || setting.settingKey,
        description: setting.description || "",
        settingGroup: setting.settingGroup || "",
        valueType: type,
        allowedValues:
            type === "choice" || type === "locale" ? allowedList(setting) : type === "boolean" ? ["true", "false"] : [],
        minValue: setting.minValue ?? null,
        maxValue: setting.maxValue ?? null,
        defaultValue: secret ? null : text(setting.defaultValue),
        // a secret is never returned: only whether it is set to something other than the placeholder
        value: secret ? null : found.value,
        masked: secret,
        valueSet: secret ? !!found.value && found.value !== "CHANGEME" : found.value !== "",
        source: found.source,
        sourceText: sourceTexts[found.source],
        propertyName: setting.propertyName || null,
        confAttribute: setting.confAttribute || null,
        editable,
        canEdit: editable && canManage && (!setting.managePermission || ec.user.hasPermission(setting.managePermission)),
        managePermission: setting.managePermission || null,
        // a preference applies at once; a conf value marked so is read at the next start
        restartRequired: !editable && setting.restartRequired === "Y",
        operatorNote: editable ? "" : setting.operatorNote || "",
        revision: editable ? stamp(preference) : "",
        changedByUserId: preference?.changedByUserId || null,
        changedByName: changedBy ? changedBy.userFullName || changedBy.username : null,
        changedDate: iso(preference?.changedDate),
        changeReason: preference?.changeReason || null
    }
}

// ===== services =====
// get#EffectiveSettings: the registered settings with their effective values, masked, each with its key, source and restart
// effect; narrowed by group, by kind (preference or configuration) and to one key.
function getEffectiveSettings(p) {
    lib.signedIn()
    // the access administrator reads the sign-in settings beside the groups' sign-in rules (impl-admin P10), nothing else
    const all = ec.user.hasPermission(VIEW) || ec.user.hasPermission(MANAGE)
    if (!all && !signInGroupPermissions.some(permission => ec.user.hasPermission(permission)))
        throw new Error(VIEW + " is required")
    const canManage = ec.user.hasPermission(MANAGE)
    const preferences = new Map(rows(PREFERENCE).map(row => [row.settingKey, map(row)]))
    const settingList = registered()
        .filter(setting => all || signInGroups.includes(setting.settingGroup))
        .filter(setting => !p.settingKey || setting.settingKey === p.settingKey)
        .filter(setting => !p.settingGroup || setting.settingGroup === p.settingGroup)
        .filter(setting => !p.kind || (p.kind === "preference") === (setting.editable === "Y"))
        // a conf attribute the server does not answer (not in its list) is logged by name and left out
        .filter(setting => {
            if (!setting.confAttribute || Object.hasOwn(confAttributeValues(), setting.confAttribute)) return true
            ec.logger.error("Instance setting " + setting.settingKey + " names a conf attribute that is not answered; left out")
            return false
        })
        .map(setting => described(setting, preferences.get(setting.settingKey), canManage))
    return {settingList, canManage, groupList: Array.from(new Set(settingList.map(row => row.settingGroup).filter(Boolean)))}
}
// The effective value of one registered preference, as text: the stored one, else the conf property, else the default.
function value(settingKey) {
    const setting = registeredOne(settingKey)
    if (!setting || setting.editable !== "Y") throw new Error(settingKey + " is not a registered instance preference")
    const row = one(PREFERENCE, {settingKey})
    return Object.assign(effective(setting, row ? map(row) : null), {valueType: valueTypes[setting.valueTypeEnumId]})
}
function getInstancePreference(p) {
    const found = value(text(p.settingKey))
    return {
        value: found.value,
        numberValue: found.valueType === "integer" && found.value !== "" ? Number(found.value) : null,
        source: found.source
    }
}
// store#InstancePreference: one registered preference, typed and within its registration, at the revision read; an empty
// value removes the preference, so the conf property or the default applies again.
function storeInstancePreference(p) {
    lib.requireManage(MANAGE)
    const settingKey = text(p.settingKey)
    const reason = lib.reasonText(p)
    const setting = registeredOne(settingKey)
    if (!setting || setting.editable !== "Y")
        return refused("NOT_REGISTERED", (settingKey || "The key") + " is not a registered instance preference")
    if (setting.managePermission && !ec.user.hasPermission(setting.managePermission))
        throw new Error(setting.managePermission + " is required")
    const existing = one(PREFERENCE, {settingKey})
    const revision = stamp(existing)
    if (text(p.expectedRevision) !== revision)
        return refused(
            "STALE_REVISION",
            (setting.title || settingKey) + " was changed since it was read; read it again and repeat the change",
            {revision}
        )
    const given = text(p.value)
    if (given === "") {
        if (existing) {
            find(PREFERENCE, {settingKey}).deleteAll()
            // the audit log keeps no delete: the value before, none after
            lib.auditRow(PREFERENCE, "preferenceValue", settingKey, null, existing.preferenceValue, null, reason)
        }
        return {applied: true, revision: "", value: effective(setting, null).value}
    }
    const read = typed(setting, given)
    if (read.value == null)
        return refused("VALUE_INVALID", (setting.title || settingKey) + ": the value " + read.problem, {revision})
    if (existing && existing.preferenceValue === read.value) return {applied: true, revision, value: read.value}
    lib.auditReason(reason, ["preferenceValue"])
    const row = ec.entity.makeValue(PREFERENCE)
    row.setFields(
        {
            settingKey,
            preferenceValue: read.value,
            changeReason: reason,
            changedByUserId: ec.user.userId,
            changedDate: ec.user.nowTimestamp
        },
        true,
        null,
        null
    )
    if (existing) row.update()
    else row.create()
    return {applied: true, revision: stamp(one(PREFERENCE, {settingKey})), value: read.value}
}

const operations = {getEffectiveSettings, getInstancePreference, storeInstancePreference}
;({
    value: settingKey => value(settingKey).value,
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown configuration settings operation " + op)
        return operations[op](parameters)
    }
})
