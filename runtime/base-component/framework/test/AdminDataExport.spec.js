// Business data export, org.moqui.admin.DataTransferServices (mr-boss plan/impl-admin.md P16; design-admin §11 Exports and
// DataExport, §14 "Import/export target", §16 "Business export"): an export is dispatched only by a registration (a request
// names a target's id, a scope the target gives the caller, the registered selection options and registered field names; no
// entity, field, filter or service of a request is read); a registered field the caller may not read is left out and named,
// whatever is asked; the file is the service's answer to its caller, never stored or sent, bounded (a larger one refused
// whole) and, written here, with a formula cell as text; an export keeps one record and one audit row (who, what, how many,
// never the content), its repeat under an operation key none; the records are read by who exported and those the target's
// view permission admits. The target is a copy of the framework's own, message translations, with one field restricted.
// Run: ./moq test framework --spec AdminDataExport
const T = "org.moqui.admin.DataTransferServices."
const TARGET = "specDxTranslations"
const MESSAGE = "moqui.basic.LocalizedMessage"
const BATCH = "moqui.server.TransferBatch"
const NOOP = "org.moqui.impl.BasicServices.noop"
const users = {run: "SPEC_DX_RUN", full: "SPEC_DX_FULL", view: "SPEC_DX_VIEW", settings: "SPEC_DX_SETTINGS", none: "SPEC_DX_NONE"}
const groups = {run: "SPEC_DX_RUNNERS", full: "SPEC_DX_FULLS", view: "SPEC_DX_VIEWERS", settings: "SPEC_DX_SETTERS"}
const SECRET = "SPEC_DX_TRANSLATION_READ"

const find = (entityName, conditions = {}) => ec.entity.find(entityName).condition(conditions).disableAuthz()
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
function as(userId, action) {
    ec.user.internalLoginUser(userId.toLowerCase().replace(/_/g, "."))
    ec.artifactExecution.enableAuthz()
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
}
function invoke(name, parameters = {}) {
    try {
        const result = ec.service.sync().name(name).parameters(parameters).call() || {}
        const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
        ec.message.clearErrors()
        return Object.assign({}, result, {error})
    } catch (e) {
        ec.message.clearErrors()
        return {error: String(e.message || e)}
    }
}
const has = (value, pattern) => pattern.test(String(value ?? ""))
const run = parameters => invoke(T + "run#Export", Object.assign({targetId: TARGET}, parameters))
const records = () => Array.from(find(BATCH, {targetId: TARGET}).list()).map(row => row.getMap())
const audits = () =>
    Array.from(
        find("moqui.entity.EntityAuditLog", {
            changedEntityName: BATCH,
            changedFieldName: "export",
            pkSecondaryValue: TARGET
        }).list()
    )

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const [key, userId] of Object.entries(users))
            make("moqui.security.UserAccount", {
                userId,
                username: userId.toLowerCase().replace(/_/g, "."),
                userFullName: "Spec export " + key
            })
        make("moqui.security.UserPermission", {userPermissionId: SECRET, description: "Spec export: read translations"})
        const grants = {
            run: ["ADMIN_TRANSFER_VIEW", "ADMIN_TRANSFER_RUN", "ADMIN_SETTINGS_VIEW"],
            full: ["ADMIN_TRANSFER_VIEW", "ADMIN_TRANSFER_RUN", "ADMIN_SETTINGS_VIEW", SECRET],
            view: ["ADMIN_TRANSFER_VIEW", "ADMIN_SETTINGS_VIEW"],
            settings: ["ADMIN_SETTINGS_VIEW", "ADMIN_SETTINGS_MANAGE"]
        }
        for (const [key, userGroupId] of Object.entries(groups)) {
            make("moqui.security.UserGroup", {userGroupId, description: "Spec export " + key})
            make("moqui.security.UserGroupMember", {userGroupId, userId: users[key], fromDate: 0})
            for (const userPermissionId of grants[key])
                make("moqui.security.UserGroupPermission", {userGroupId, userPermissionId, fromDate: 0})
        }
        // the framework's own export target, with its translation readable only with a permission and at most three rows
        const shipped = find("moqui.server.TransferTarget", {targetId: "messageTranslationsExport"}).one().getMap()
        make(
            "moqui.server.TransferTarget",
            Object.assign({}, shipped, {
                targetId: TARGET,
                maxRows: 3,
                columnsJson: JSON.stringify([
                    {name: "original", label: "Original", required: true},
                    {name: "locale", label: "Locale", required: true},
                    {name: "localized", label: "Translation", permission: SECRET},
                    {name: "ghost", label: "Ghost", permission: "SPEC_DX_NO_SUCH_PERMISSION"}
                ]),
                lastUpdatedStamp: null
            })
        )
        const message = (original, locale, localized) => make(MESSAGE, {original, locale, localized})
        message("Spec export hello", "zz_SP", "Spec export secret words")
        message("=1+1", "zz_SP", "@SUM(A1)")
        for (const index of [1, 2, 3, 4]) message("Spec export many " + index, "zz_SQ", "Many " + index)
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        find(BATCH, {targetId: TARGET}).deleteAll()
        find("moqui.server.TransferTarget", {targetId: TARGET}).deleteAll()
        for (const locale of ["zz_SP", "zz_SQ"]) find(MESSAGE, {locale}).deleteAll()
        for (const userGroupId of Object.values(groups)) {
            find("moqui.security.UserGroupPermission", {userGroupId}).deleteAll()
            find("moqui.security.UserGroupMember", {userGroupId}).deleteAll()
            find("moqui.security.UserGroup", {userGroupId}).deleteAll()
        }
        find("moqui.security.UserPermission", {userPermissionId: SECRET}).deleteAll()
        for (const userId of Object.values(users)) find("moqui.security.UserAccount", {userId}).deleteAll()
    }
})

feature("an export is dispatched only by a registration, and a field the caller may not read is left out whatever is asked", {
    when() {
        const out = {}
        const selection = {locale: "zz_SP"}
        out.inNames = Array.from(ec.service.getServiceDefinition(T + "run#Export").inParameterNames).map(String)
        out.runner = as(users.run, () => ({
            targets: invoke(T + "find#ExportTargets"),
            fields: invoke(T + "get#ExportFields", {targetId: TARGET}),
            unknown: invoke(T + "run#Export", {targetId: "constructor"}),
            importTarget: invoke(T + "run#Export", {targetId: "messageTranslations"}),
            // an export target has no other step, and an import target no export step
            wrongStep: invoke(T + "call#TargetService", {targetId: TARGET, step: "applyRow", stepParameters: {}}),
            unregisteredField: run({selection, fieldList: ["original", "userId"]}),
            inheritedField: run({selection, fieldList: ["constructor"]}),
            ghostField: run({selection, fieldList: ["ghost"]}),
            unregisteredOption: run({selection: {locale: "zz_SP", entityName: "moqui.security.UserAccount"}}),
            badOption: run({selection: {locale: "${ec.user.userId}"}}),
            scoped: run({selection, scopeKey: "SPEC_DX_ORG"}),
            preview: invoke(T + "preview#Export", {targetId: TARGET, selection, fieldList: ["original", "locale", "localized"]}),
            // names a request may carry that are never read
            asked: run({
                selection,
                fieldList: ["original", "locale", "localized"],
                serviceName: NOOP,
                exportService: NOOP,
                entityName: "moqui.security.UserAccount",
                toAddress: "elsewhere@example.com"
            }),
            defaults: run({selection}),
            tooMany: run({selection: {locale: "zz_SQ"}})
        }))
        out.recordsAfterRunner = records().length
        out.full = as(users.full, () => ({
            fields: invoke(T + "get#ExportFields", {targetId: TARGET}),
            asked: run({selection, fieldList: "original,locale,localized"})
        }))
        // the owner's permission alone, the transfer permissions without the run, and neither
        out.settings = as(users.settings, () => run({selection}))
        out.view = as(users.view, () => ({run: run({selection}), fields: invoke(T + "get#ExportFields", {targetId: TARGET})}))
        out.none = as(users.none, () => ({run: run({selection}), targets: invoke(T + "find#ExportTargets")}))
        return out
    },
    then(out) {
        // no parameter names a recipient, an address, a path, an entity or a service
        assert(out.inNames.slice().sort().join() === "fieldList,operationKey,scopeKey,selection,targetId", out.inNames.join())
        const listed = out.runner.targets.targetList.find(target => target.targetId === TARGET)
        assert(listed && listed.exportHere && listed.exportAllowed && !JSON.stringify(out.runner.targets).includes("Services"))
        assert(out.runner.targets.targetList.some(target => target.targetId === "messageTranslationsExport"))
        assert(
            !out.runner.targets.targetList.some(target => target.targetId === "messageTranslations"),
            "an import target was listed"
        )
        // the restricted field is shown as not allowed, its permission never named; the one naming no permission is gone
        const fields = out.runner.fields.target.fieldList
        assert(fields.map(field => field.name).join() === "original,locale,localized", fields.map(field => field.name).join())
        assert(fields[2].allowed === false && fields[2].selected === false && fields[0].allowed === true)
        assert(!JSON.stringify(out.runner.fields).includes(SECRET) && !JSON.stringify(out.runner.fields).includes("Services"))
        assert(out.full.fields.target.fieldList[2].allowed === true)
        // only a registration is dispatched, by its own step, with its own fields and options
        assert(
            has(out.runner.unknown.error, /Unknown export target/) && has(out.runner.importTarget.error, /Unknown export target/)
        )
        assert(has(out.runner.wrongStep.error, /registers no applyRow step/), out.runner.wrongStep.error)
        for (const refused of [out.runner.unregisteredField, out.runner.inheritedField, out.runner.ghostField])
            assert(has(refused.error, /has no field/) && !refused.fileText, refused.error)
        assert(out.runner.unregisteredOption.refused === true && out.runner.unregisteredOption.reasonCode === "SELECTION_INVALID")
        assert(out.runner.badOption.refused === true && !out.runner.badOption.fileText, out.runner.badOption.error)
        assert(has(out.runner.scoped.error, /takes no scope/), out.runner.scoped.error)
        // the preview keeps nothing and names what would be left out
        assert(
            out.runner.preview.rowCount === 2 && out.runner.preview.omittedFieldList[0].name === "localized",
            out.runner.preview.error
        )
        assert(out.runner.preview.fieldList.map(field => field.name).join() === "original,locale")
        // the field asked for by name is left out, and named: its values are not in the file
        const asked = out.runner.asked
        assert(asked.exported === true && asked.rowCount === 2, asked.error || asked.reason)
        assert(asked.omittedFieldList.length === 1 && asked.omittedFieldList[0].name === "localized")
        assert(!asked.fileText.includes("Spec export secret words") && !asked.fileText.includes("Translation"), asked.fileText)
        assert(
            asked.fileText.includes('"Original","Locale"') &&
                asked.contentType === "text/csv" &&
                has(asked.filename, /^specDxTranslations-\d{8}-\d{4}\.csv$/)
        )
        // a cell a spreadsheet would read as a formula is written as text
        assert(asked.fileText.includes('"\'=1+1"'), asked.fileText)
        // the defaults leave it out the same way
        assert(out.runner.defaults.exported === true && !out.runner.defaults.fileText.includes("Spec export secret words"))
        // a holder of the field's permission gets it, a formula in it as text too
        assert(out.full.asked.exported === true && out.full.asked.omittedFieldList.length === 0, out.full.asked.error)
        assert(out.full.asked.fileText.includes("Spec export secret words") && out.full.asked.fileText.includes('"\'@SUM(A1)"'))
        // past the target's rows nothing is exported: refused whole, never cut short
        assert(
            out.runner.tooMany.refused === true &&
                out.runner.tooMany.reasonCode === "TOO_MANY_ROWS" &&
                !out.runner.tooMany.fileText
        )
        assert(out.recordsAfterRunner === 2, "a refused export was recorded: " + out.recordsAfterRunner)
        // each permission the registration names is asked, and the owner's own
        assert(has(out.settings.error, /ADMIN_TRANSFER_RUN is required/), out.settings.error)
        assert(has(out.view.run.error, /ADMIN_TRANSFER_RUN is required/) && out.view.fields.target.exportAllowed === false)
        assert(out.view.fields.target.fieldList.length === 0, "a non-exporter was told the fields")
        assert(has(out.none.run.error, /is required/) && has(out.none.targets.error, /ADMIN_TRANSFER_VIEW is required/))
    }
})

feature(
    "an export is its caller's answer and one record: who, what and how many, never the content; a repeat under its key adds none",
    {
        when() {
            const out = {}
            find(BATCH, {targetId: TARGET}).deleteAll()
            const before = audits().length
            const request = {
                selection: {locale: "zz_SP"},
                fieldList: ["original", "locale", "localized"],
                operationKey: "SPEC_DX_K1"
            }
            as(users.full, () => {
                out.first = run(request)
                out.repeat = run(request)
                out.otherRequest = run(Object.assign({}, request, {selection: {locale: "zz_SQ"}, fieldList: ["original"]}))
                out.own = invoke(T + "find#Transfers", {direction: "export", targetId: TARGET})
                out.imports = invoke(T + "find#Transfers", {targetId: "messageTranslations"})
            })
            out.otherUser = as(users.run, () => ({
                sameKey: run(request),
                list: invoke(T + "find#Transfers", {direction: "export", kind: "export"}),
                deliveries: invoke(T + "find#Transfers", {direction: "export"})
            }))
            out.viewer = as(users.view, () => invoke(T + "find#Transfers", {direction: "export", targetId: TARGET}))
            out.settings = as(users.settings, () => invoke(T + "find#Transfers", {direction: "export"}))
            out.records = records()
            out.auditsAdded = audits().length - before
            out.audit = audits()
                .find(row => row.pkPrimaryValue === "SPEC_DX_K1")
                ?.getMap()
            return out
        },
        then(out) {
            assert(out.first.exported === true && out.first.transferId === "SPEC_DX_K1" && !out.first.repeated, out.first.error)
            // the repeat makes the file again and keeps nothing more; another request under the key is refused
            assert(out.repeat.exported === true && out.repeat.repeated === true && out.repeat.fileText === out.first.fileText)
            assert(
                out.otherRequest.refused === true &&
                    out.otherRequest.reasonCode === "OPERATION_KEY_CONFLICT" &&
                    !out.otherRequest.fileText
            )
            assert(out.otherUser.sameKey.refused === true && out.otherUser.sameKey.reasonCode === "OPERATION_KEY_CONFLICT")
            assert(
                out.records.length === 1 && out.auditsAdded === 1,
                out.records.length + " records, " + out.auditsAdded + " audit rows"
            )
            // the record: who, the target, the fields, the rows and a hash; not the content
            const record = out.records[0]
            assert(
                record.userId === users.full &&
                    record.state === "exported" &&
                    Number(record.rowCount) === 2 &&
                    record.fileHash.length === 64
            )
            assert(JSON.parse(record.requestJson).fields.join() === "original,locale,localized")
            assert(!JSON.stringify(record).includes("Spec export secret words"), "the record holds content")
            assert(
                out.audit.changedByUserId === users.full &&
                    has(out.audit.newValueText, /^2 rows of Message translations; fields: original, locale, localized$/)
            )
            // its exporter and a holder of the target's view permissions read the record: nothing to open; the imports' list is as it was
            const own = out.own.transferList
            assert(
                own.length === 1 &&
                    own[0].runId === "SPEC_DX_K1" &&
                    own[0].rowCount === 2 &&
                    own[0].download === false &&
                    own[0].detail === false
            )
            assert(own[0].userName === "Spec export full" && own[0].kind === "export" && !("fileText" in own[0]))
            assert(out.viewer.transferList.length === 1 && out.viewer.transferList[0].runId === "SPEC_DX_K1", out.viewer.error)
            assert(out.imports.error === "" && !out.imports.transferList.some(row => row.runId === "SPEC_DX_K1"))
            assert(
                out.otherUser.list.transferList.some(row => row.runId === "SPEC_DX_K1"),
                out.otherUser.list.error
            )
            // the report engine's deliveries are asked for apart and answer none here, without failing the list
            assert(
                out.otherUser.deliveries.error === "" && out.otherUser.deliveries.unansweredTargetList.length === 0,
                out.otherUser.deliveries.error
            )
            assert(out.otherUser.deliveries.kindList.length === 3)
            assert(has(out.settings.error, /ADMIN_TRANSFER_VIEW is required/), out.settings.error)
        }
    }
)
