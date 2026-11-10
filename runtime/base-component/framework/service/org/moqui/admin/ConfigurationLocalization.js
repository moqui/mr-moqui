// The localization part of org.moqui.admin.ConfigurationServices (mr-boss plan/design-admin.md §7 Localization, §17.2; impl-admin
// P8): delegated translation of messages (moqui.basic.LocalizedMessage) and of registered labels
// (moqui.basic.LocalizedEntityField), for a holder of ADMIN_SETTINGS_VIEW (reads) and ADMIN_SETTINGS_MANAGE (changes) who is not
// an instance administrator; ADMIN keeps Tools System > Localization, which writes the rows directly.
//
// - A translation keeps its placeholders: the ${...} expressions of the translation must be exactly those of the original,
//   so a translator neither loses a value nor adds an expression the server would evaluate. Markup is refused by the
//   services' parameters (no HTML).
// - A label is translated only for a registered target (moqui.basic.LocalizedLabelTarget, each component's seed data) that
//   the entity definitions bear out: the field is enable-localization and not encrypted, on a configuration entity with one
//   primary key field; another registration is logged by name and left out. So translation access reads and writes labels
//   (status, enumeration, unit and place names), never a business record or a sensitive value; the labels' own values are
//   read to show what is being translated.
// - Each change names the revision it read (the row's lastUpdatedStamp, empty for a new translation) and is refused naming
//   the current one when stale; an empty translation removes the row. Each change leaves an audit row with the text before
//   and after and the reason (the entities are not audit-logged for every writer: data files load translations by the
//   thousand).
const lib = ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationLib.js")
const {find, one, rows, map, stamp, text, refused} = lib
const MESSAGE = "moqui.basic.LocalizedMessage"
const FIELD = "moqui.basic.LocalizedEntityField"
const TARGET = "moqui.basic.LocalizedLabelTarget"
const VIEW = "ADMIN_SETTINGS_VIEW"
const MANAGE = "ADMIN_SETTINGS_MANAGE"
// the most originals or labels a "missing" search reads before it says it stopped
const scanLimit = 2000
const maxTranslation = 4000

const likeText = value => "%" + String(value).replace(/[\\%_]/g, character => "\\" + character) + "%"
function anyLike(fields, value) {
    const factory = ec.entity.conditionFactory
    return factory.makeCondition(
        fields.map(field => factory.makeCondition(field, "like", likeText(value)).ignoreCase()),
        "or"
    )
}
const placeholders = value => Array.from(new Set(String(value ?? "").match(/\$\{[^}]*\}/g) || [])).sort()
// What is wrong with a translation of original, or null: its placeholders are the original's, exactly.
function translationProblem(original, localized) {
    if (localized.length > maxTranslation) return "The translation is longer than " + maxTranslation + " characters"
    if (/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(localized)) return "The translation has a control character"
    const expected = placeholders(original)
    const given = placeholders(localized)
    const missing = expected.filter(item => !given.includes(item))
    const added = given.filter(item => !expected.includes(item))
    if (missing.length) return "The translation must keep " + missing.join(", ")
    if (added.length) return "The translation may not add " + added.join(", ")
    // an unclosed ${ would swallow what follows it
    if ((localized.match(/\$\{/g) || []).length !== (localized.match(/\$\{[^}]*\}/g) || []).length)
        return "The translation has an unclosed ${"
    return null
}
// A locale as the framework looks translations up: a language (es), a language and country (pt_BR), or for a label the
// fallback every locale reads (default).
const localeOk = (locale, allowDefault) => /^[a-z]{2,3}(_[A-Z]{2})?$/.test(locale) || (allowDefault && locale === "default")
function localeOptions() {
    const found = new Set()
    for (const entity of [MESSAGE, FIELD])
        for (const row of Array.from(find(entity).selectField("locale").distinct(true).list()))
            if (row.locale) found.add(row.locale)
    return Array.from(found).sort()
}

// ===== label targets =====
// A registration the entity definitions bear out, with its entity's one primary key field; another is logged and null.
function checkedTarget(row) {
    const key = row.entityName + "." + row.fieldName
    const definition = ec.entity.isEntityDefined(row.entityName) ? ec.entity.getEntityDefinition(row.entityName) : null
    const field = definition?.getFieldInfo(row.fieldName)
    const pkNames = definition ? Array.from(definition.getPkFieldNames()) : []
    const problem = !definition
        ? "names no entity"
        : definition.isViewEntity
          ? "names a view entity"
          : String(definition.getEntityNode().attribute("use") || "") !== "configuration"
            ? "is not on a configuration entity"
            : !field
              ? "names no field"
              : !field.enableLocalization
                ? "is not a localized field"
                : field.encrypt
                  ? "is an encrypted field"
                  : pkNames.length !== 1
                    ? "is on an entity without a single key"
                    : null
    if (problem) {
        ec.logger.error("Localized label target " + key + " " + problem + "; left out")
        return null
    }
    return {
        targetKey: key,
        entityName: row.entityName,
        fieldName: row.fieldName,
        pkField: pkNames[0],
        title: row.title || key,
        sequenceNum: Number(row.sequenceNum ?? 9999)
    }
}
// The registered targets in use.
function targets() {
    return rows(TARGET)
        .filter(row => row.disabled !== "Y")
        .map(checkedTarget)
        .filter(Boolean)
        .sort((a, b) => a.sequenceNum - b.sequenceNum || a.title.localeCompare(b.title))
}
// One registered target in use, by its entity (full name) and field.
function targetNamed(entityName, fieldName) {
    const row = entityName && fieldName ? one(TARGET, {entityName, fieldName}) : null
    return row && row.disabled !== "Y" ? checkedTarget(row) : null
}
const targetOf = (list, entityName, fieldName) =>
    list.find(target => target.entityName === entityName && target.fieldName === fieldName) || null
function labelValue(target, pkValue) {
    const row = one(target.entityName, {[target.pkField]: pkValue})
    return row ? {value: String(map(row)[target.fieldName] ?? "")} : null
}

// ===== messages =====
// find#LocalizedMessages: the translations, by locale and text (original or translation, ignoring case); with missing and a
// locale, the originals that have a translation in another locale and none in this one.
function findLocalizedMessages(p) {
    lib.requireView(VIEW, MANAGE)
    const locale = text(p.locale)
    const search = text(p.text)
    const out = {canManage: ec.user.hasPermission(MANAGE), localeOptions: localeOptions(), truncated: false}
    if (p.missing && locale) {
        let originals = find(MESSAGE).selectField("original").distinct(true).orderBy("original")
        if (search) originals = originals.condition(anyLike(["original"], search))
        const all = Array.from(originals.limit(scanLimit + 1).list()).map(row => row.original)
        out.truncated = all.length > scanLimit
        const translated = new Set(Array.from(find(MESSAGE, {locale}).selectField("original").list()).map(row => row.original))
        const paged = lib.page(
            all.slice(0, scanLimit).filter(original => !translated.has(original)),
            p
        )
        return Object.assign(out, {
            messageList: paged.rows.map(original => ({original, locale, localized: "", revision: "", missing: true})),
            messageListCount: paged.count
        })
    }
    const query = () => {
        let found = find(MESSAGE, locale ? {locale} : {})
        if (search) found = found.condition(anyLike(["original", "localized"], search))
        return found
    }
    const pageSize = Math.min(Math.max(Number(p.pageSize) || 50, 1), 100)
    const pageIndex = Math.max(Number(p.pageIndex) || 0, 0)
    const list = Array.from(
        query()
            .orderBy("original")
            .orderBy("locale")
            .offset(pageIndex * pageSize)
            .limit(pageSize)
            .list()
    )
    return Object.assign(out, {
        messageList: list.map(row => ({
            original: row.original,
            locale: row.locale,
            localized: row.localized || "",
            revision: stamp(row),
            missing: false
        })),
        messageListCount: Number(query().count())
    })
}
// store#LocalizedMessage: the translation of a message in a locale, its placeholders kept; empty removes it.
function storeLocalizedMessage(p) {
    lib.requireManage(MANAGE)
    const original = String(p.original ?? "")
    const locale = text(p.locale)
    const localized = String(p.localized ?? "").trim()
    const reason = lib.reasonText(p)
    if (!original.trim() || original.length > 255) throw new Error("Give the original message, at most 255 characters")
    if (!localeOk(locale, false)) return refused("LOCALE_INVALID", "Give a locale such as es or pt_BR")
    const key = {original, locale}
    const existing = one(MESSAGE, key)
    const revision = stamp(existing)
    if (text(p.expectedRevision) !== revision)
        return refused("STALE_REVISION", "The translation was changed since it was read; read it again and repeat the change", {
            revision
        })
    if (!localized) {
        if (existing) {
            find(MESSAGE, key).deleteAll()
            lib.auditRow(MESSAGE, "localized", original, locale, existing.localized, null, reason)
        }
        return {applied: true, revision: ""}
    }
    const problem = translationProblem(original, localized)
    if (problem) return refused("PLACEHOLDER_MISMATCH", problem, {revision})
    if (existing && existing.localized === localized) return {applied: true, revision}
    const row = ec.entity.makeValue(MESSAGE)
    row.setFields(Object.assign({localized}, key), true, null, null)
    if (existing) row.update()
    else row.create()
    lib.auditRow(MESSAGE, "localized", original, locale, existing?.localized ?? null, localized, reason)
    return {applied: true, revision: stamp(one(MESSAGE, key))}
}

// ===== labels =====
// find#LocalizedEntityFields: the translations of the registered label targets, by target, locale and text, each with the
// label it translates; with missing, a target and a locale, that target's labels with no translation in the locale.
function findLocalizedEntityFields(p) {
    lib.requireView(VIEW, MANAGE)
    const targetList = targets()
    const locale = text(p.locale)
    const search = text(p.text)
    const out = {
        canManage: ec.user.hasPermission(MANAGE),
        localeOptions: localeOptions(),
        targetList: targetList.map(target => ({targetKey: target.targetKey, title: target.title})),
        truncated: false,
        fieldList: [],
        fieldListCount: 0
    }
    const chosen = p.targetKey ? targetList.find(target => target.targetKey === p.targetKey) : null
    if (p.targetKey && !chosen) return out
    const shown = (target, pkValue, original, row) => ({
        targetKey: target.targetKey,
        targetTitle: target.title,
        entityName: target.entityName,
        fieldName: target.fieldName,
        pkValue,
        original,
        locale: row ? row.locale : locale,
        localized: row?.localized || "",
        revision: stamp(row),
        missing: !row
    })
    if (p.missing && chosen && locale) {
        let labels = find(chosen.entityName).selectField(chosen.pkField).selectField(chosen.fieldName).orderBy(chosen.pkField)
        if (search) labels = labels.condition(anyLike([chosen.fieldName, chosen.pkField], search))
        const all = Array.from(labels.limit(scanLimit + 1).list()).map(row => map(row))
        out.truncated = all.length > scanLimit
        const translated = new Set(
            rows(FIELD, {entityName: chosen.entityName, fieldName: chosen.fieldName, locale}).map(row => row.pkValue)
        )
        const paged = lib.page(
            all
                .slice(0, scanLimit)
                .filter(
                    row =>
                        row[chosen.fieldName] != null &&
                        row[chosen.fieldName] !== "" &&
                        !translated.has(String(row[chosen.pkField]))
                ),
            p
        )
        return Object.assign(out, {
            fieldList: paged.rows.map(row => shown(chosen, String(row[chosen.pkField]), String(row[chosen.fieldName]), null)),
            fieldListCount: paged.count
        })
    }
    if (!targetList.length) return out
    let found = find(FIELD)
    if (chosen) found = found.condition({entityName: chosen.entityName, fieldName: chosen.fieldName})
    else found = found.condition("entityName", "in", Array.from(new Set(targetList.map(target => target.entityName))))
    if (locale) found = found.condition("locale", locale)
    if (search) found = found.condition(anyLike(["localized", "pkValue"], search))
    // a field of a registered entity that is not itself a target is left out after the read
    const list = Array.from(
        found.orderBy("entityName").orderBy("fieldName").orderBy("pkValue").orderBy("locale").limit(scanLimit).list()
    )
        .map(row => ({row, target: targetOf(targetList, row.entityName, row.fieldName)}))
        .filter(item => item.target)
    const paged = lib.page(list, p)
    return Object.assign(out, {
        fieldList: paged.rows.map(({row, target}) =>
            shown(target, row.pkValue, labelValue(target, row.pkValue)?.value ?? "", row)
        ),
        fieldListCount: paged.count
    })
}
// store#LocalizedEntityField: the translation of one label of a registered target in a locale, its placeholders kept; empty
// removes it.
function storeLocalizedEntityField(p) {
    lib.requireManage(MANAGE)
    const locale = text(p.locale)
    const pkValue = text(p.pkValue)
    const localized = String(p.localized ?? "").trim()
    const reason = lib.reasonText(p)
    const target = targetNamed(text(p.entityName), text(p.fieldName))
    if (!target)
        return refused("NOT_REGISTERED", text(p.entityName) + "." + text(p.fieldName) + " is not a registered label to translate")
    if (!localeOk(locale, true)) return refused("LOCALE_INVALID", "Give a locale such as es or pt_BR")
    const label = pkValue ? labelValue(target, pkValue) : null
    if (!label) return refused("LABEL_UNAVAILABLE", target.title + " has no " + (pkValue || "such label"))
    const key = {entityName: target.entityName, fieldName: target.fieldName, pkValue, locale}
    const existing = one(FIELD, key)
    const revision = stamp(existing)
    if (text(p.expectedRevision) !== revision)
        return refused("STALE_REVISION", "The translation was changed since it was read; read it again and repeat the change", {
            revision
        })
    if (!localized) {
        if (existing) {
            find(FIELD, key).deleteAll()
            lib.auditRow(FIELD, "localized", target.targetKey, pkValue, existing.localized, null, reason, locale)
        }
        return {applied: true, revision: ""}
    }
    const problem = translationProblem(label.value, localized)
    if (problem) return refused("PLACEHOLDER_MISMATCH", problem, {revision})
    if (existing && existing.localized === localized) return {applied: true, revision}
    const row = ec.entity.makeValue(FIELD)
    row.setFields(Object.assign({localized}, key), true, null, null)
    if (existing) row.update()
    else row.create()
    lib.auditRow(FIELD, "localized", target.targetKey, pkValue, existing?.localized ?? null, localized, reason, locale)
    return {applied: true, revision: stamp(one(FIELD, key))}
}

const operations = {findLocalizedMessages, storeLocalizedMessage, findLocalizedEntityFields, storeLocalizedEntityField}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown configuration localization operation " + op)
        return operations[op](parameters)
    }
})
