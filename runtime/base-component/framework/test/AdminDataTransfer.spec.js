// Business data transfer, org.moqui.admin.DataTransferServices (mr-boss plan/impl-admin.md P15; design-admin §11.1, §14
// "Import/export target", §15.3 "Import preview/retry" and "Independent failure"): an import is dispatched only by a
// registration (a request names a target's id, and no service, entity or field the registration does not name; a
// registration without its contract is left out; an owner's service runs as the caller); a file that is not text, or has a
// column named like a secret, is refused whole; a batch is read by who staged it and those its target admits; a stale
// preview is refused; an identical retry answers the first result and changed content under its key is refused; each row is
// on its own, and only failed rows are retried: an applied row is never applied again. The target is the framework's own,
// message translations, whose rows are saved by store#LocalizedMessage's rules.
// Run: ./moq test framework --spec AdminDataTransfer
const T = "org.moqui.admin.DataTransferServices."
const TARGET = "messageTranslations"
const MESSAGE = "moqui.basic.LocalizedMessage"
const NOOP = "org.moqui.impl.BasicServices.noop"
const users = {
    run: "SPEC_DT_RUN",
    other: "SPEC_DT_OTHER",
    view: "SPEC_DT_VIEW",
    settings: "SPEC_DT_SETTINGS",
    none: "SPEC_DT_NONE"
}
const groups = {run: "SPEC_DT_RUNNERS", view: "SPEC_DT_VIEWERS", settings: "SPEC_DT_SETTERS"}

const find = (entityName, conditions = {}) => ec.entity.find(entityName).condition(conditions).disableAuthz()
const one = (entityName, conditions) => find(entityName, conditions).one()
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
const step = (verb, parameters) => invoke(T + verb, Object.assign({targetId: TARGET}, parameters))
const translation = (original, locale) => one(MESSAGE, {original, locale})?.localized ?? null
const audits = original =>
    Number(
        find("moqui.entity.EntityAuditLog", {
            changedEntityName: MESSAGE,
            changedFieldName: "localized",
            pkPrimaryValue: original
        }).count()
    )
const rowOf = (out, itemSeqId) => Array.from(out.importRows || []).find(row => Number(row.itemSeqId) === itemSeqId)
const resultOf = (out, itemSeqId) => Array.from(out.resultList || []).find(row => Number(row.itemSeqId) === itemSeqId)

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const [key, userId] of Object.entries(users))
            make("moqui.security.UserAccount", {
                userId,
                username: userId.toLowerCase().replace(/_/g, "."),
                userFullName: "Spec transfer " + key
            })
        for (const [key, userGroupId] of Object.entries(groups))
            make("moqui.security.UserGroup", {userGroupId, description: "Spec transfer " + key})
        const grant = (userGroupId, permissions) =>
            permissions.forEach(userPermissionId =>
                make("moqui.security.UserGroupPermission", {userGroupId, userPermissionId, fromDate: 0})
            )
        grant(groups.run, ["ADMIN_TRANSFER_VIEW", "ADMIN_TRANSFER_RUN", "ADMIN_SETTINGS_VIEW", "ADMIN_SETTINGS_MANAGE"])
        grant(groups.view, ["ADMIN_TRANSFER_VIEW", "ADMIN_SETTINGS_VIEW"])
        // the owner's permission without the transfer permission the registration asks for besides
        grant(groups.settings, ["ADMIN_SETTINGS_VIEW", "ADMIN_SETTINGS_MANAGE"])
        for (const key of Object.keys(groups))
            make("moqui.security.UserGroupMember", {userGroupId: groups[key], userId: users[key], fromDate: 0})
        make("moqui.security.UserGroupMember", {userGroupId: groups.run, userId: users.other, fromDate: 0})
        // a registration naming services without the row contract is left out: nothing is ever called for it
        make("moqui.server.TransferTarget", {
            targetId: "specDtNoContract",
            direction: "import",
            targetMode: "rows",
            title: "Spec target without a contract",
            columnsJson: '[{"name":"original","label":"Original"}]',
            validateRowService: NOOP,
            applyRowService: NOOP
        })
        make(MESSAGE, {original: "Spec transfer greeting", locale: "es", localized: "Saludo antiguo"})
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.artifactExecution.disableAuthz()
    }
})

feature("an import is dispatched only by a registration, as the caller, and a file that may not be read is refused whole", {
    when() {
        const out = (context.out = {})
        const good = "original,locale,localized\nSpec transfer hello,es,Hola\n"
        out.run = as(users.run, () => ({
            targets: invoke(T + "find#ImportTargets"),
            target: invoke(T + "get#ImportTarget", {targetId: TARGET}),
            unknown: invoke(T + "stage#Import", {targetId: "constructor", sourceText: good}),
            noContract: invoke(T + "stage#Import", {targetId: "specDtNoContract", sourceText: good}),
            direct: invoke(T + "call#TargetService", {
                targetId: TARGET,
                step: "applyRow",
                serviceName: NOOP,
                stepParameters: {
                    targetId: TARGET,
                    rowValues: {original: "Spec transfer direct", locale: "es"},
                    operationKey: "spec-dt-direct"
                }
            }),
            unknownStep: invoke(T + "call#TargetService", {targetId: TARGET, step: "constructor", stepParameters: {}}),
            secret: step("stage#Import", {sourceText: "original,locale,localized,User Password\nA,es,B,hunter2\n"}),
            token: step("stage#Import", {sourceText: "original,locale,localized,apiToken\nA,es,B,t-1\n"}),
            binary: step("stage#Import", {sourceText: "original,locale\n\u0000PK,es\n"}),
            headerOnly: step("stage#Import", {sourceText: "original,locale,localized\n"}),
            scoped: step("stage#Import", {sourceText: good, organizationPartyId: "SPEC_DT_ORG"}),
            staged: step("stage#Import", {
                sourceText: "Original,Locale,Translation,Entity\nSpec transfer hello,es,Hola,moqui.security.UserAccount\n",
                // names a request may carry that are never read: the registration alone says what is called
                serviceName: NOOP,
                applyRowService: NOOP,
                entityName: "moqui.security.UserAccount"
            })
        }))
        out.mapUnregistered = as(users.run, () =>
            step("configure#Import", {
                operationKey: out.run.staged.operationKey,
                mappingRevision: out.run.staged.mappingRevision,
                mapping: {Entity: "userId"}
            })
        )
        out.mapInherited = as(users.run, () =>
            step("configure#Import", {
                operationKey: out.run.staged.operationKey,
                mappingRevision: out.run.staged.mappingRevision,
                defaults: {constructor: "x"}
            })
        )
        // the owner's permission alone, and the transfer permission alone, stage nothing
        out.settings = as(users.settings, () => step("stage#Import", {sourceText: good}))
        out.view = as(users.view, () => ({
            stage: step("stage#Import", {sourceText: good}),
            read: step("get#Transfer", {operationKey: out.run.staged.operationKey}),
            report: step("get#ImportReport", {operationKey: out.run.staged.operationKey}),
            change: step("configure#Import", {
                operationKey: out.run.staged.operationKey,
                mappingRevision: out.run.staged.mappingRevision,
                mapping: {}
            }),
            list: invoke(T + "find#Transfers", {targetId: TARGET})
        }))
        // another holder of every permission reads it, and applies nothing of it: each row is its importer's own
        out.other = as(users.other, () => ({
            read: step("get#Transfer", {operationKey: out.run.staged.operationKey}),
            validate: step("validate#Import", {
                operationKey: out.run.staged.operationKey,
                mappingRevision: out.run.staged.mappingRevision
            })
        }))
        out.none = as(users.none, () => ({
            targets: invoke(T + "find#ImportTargets"),
            list: invoke(T + "find#Transfers"),
            read: step("get#Transfer", {operationKey: out.run.staged.operationKey}),
            report: step("get#ImportReport", {operationKey: out.run.staged.operationKey}),
            item: invoke(T + "apply#TransferItem", {
                targetId: TARGET,
                operationKey: out.run.staged.operationKey,
                itemSeqId: 1,
                retry: true
            })
        }))
    },
    then() {
        const {run, view, other, none, settings, mapUnregistered, mapInherited} = context.out
        // the catalog: the framework's own target, never a service name; the malformed registration is left out
        const listed = Array.from(run.targets.targetList).find(target => target.targetId === TARGET)
        listed.targetMode == "rows"
        listed.importAllowed == true
        !Array.from(run.targets.targetList).some(target => target.targetId === "specDtNoContract")
        !JSON.stringify(run.targets.targetList).includes("Services.")
        !JSON.stringify(run.target.target).includes("Services.")
        Array.from(run.target.target.columnList)
            .map(column => column.name)
            .join(",") == "original,locale,localized"
        has(run.unknown.error, /Unknown import target/)
        has(run.noContract.error, /Unknown import target/)
        // a caller names a step of a registration, never a service: the row service ran as itself and refused its empty row
        !run.direct.error
        run.direct.result.applied == false
        has(run.unknownStep.error, /Unknown transfer step/)
        // refused whole, the column named, nothing kept
        run.secret.refused == true
        run.secret.reasonCode == "COLUMN_REFUSED"
        run.secret.reason.includes("User Password")
        !run.secret.operationKey
        run.token.reasonCode == "COLUMN_REFUSED"
        run.binary.reasonCode == "FILE_UNREADABLE"
        run.headerOnly.reasonCode == "FILE_UNREADABLE"
        has(run.scoped.error, /takes no scope/)
        !find("moqui.server.TransferItem")
            .list()
            .some(item => String(item.sourceJson).includes("hunter2"))
        // staged: the headers mapped by name or label, the unregistered column to nothing
        run.staged.state == "staged"
        run.staged.rowCount == 1
        run.staged.mapping.Original == "original"
        run.staged.mapping.Translation == "localized"
        run.staged.mapping.Entity == null
        has(mapUnregistered.error, /field of this import/)
        has(mapInherited.error, /field of this import/)
        has(settings.error, /ADMIN_TRANSFER_RUN is required/)
        has(view.stage.error, /ADMIN_TRANSFER_RUN is required/)
        // a holder of the target's view permissions reads the batch and changes nothing; anyone else learns nothing
        view.read.rowCount == 1
        view.read.canEdit == false
        view.report.reportCsv.includes("Spec transfer hello")
        has(view.change.error, /ADMIN_TRANSFER_RUN is required/)
        other.read.canEdit == false
        has(other.validate.error, /not available/)
        Array.from(view.list.transferList).some(found => found.runId === run.staged.operationKey && found.detail === true)
        has(none.targets.error, /ADMIN_TRANSFER_VIEW is required/)
        has(none.list.error, /ADMIN_TRANSFER_VIEW is required/)
        has(none.read.error, /not available/)
        has(none.report.error, /not available/)
        none.item.error.length > 0
        translation("Spec transfer hello", "es") == null
    }
})

feature("a stale preview is refused, an identical retry answers the first result and changed content under its key is refused", {
    when() {
        const out = (context.keyed = {})
        const content =
            "original,locale,localized\nSpec transfer one,es,Uno\nSpec transfer two,es,Dos\n=Spec transfer formula,es,Formula\n"
        as(users.run, () => {
            out.staged = step("stage#Import", {
                sourceText: content,
                operationKey: "spec-dt-key-1",
                filename: "../../spec ${evil}.csv"
            })
            out.again = step("stage#Import", {sourceText: content, operationKey: "spec-dt-key-1"})
            out.changed = step("stage#Import", {
                sourceText: content + "Spec transfer three,es,Tres\n",
                operationKey: "spec-dt-key-1"
            })
            out.derived = step("stage#Import", {sourceText: content})
            out.derivedAgain = step("stage#Import", {sourceText: content})
            out.forced = step("stage#Import", {sourceText: content, forceNew: true})
            out.staleMap = step("validate#Import", {operationKey: "spec-dt-key-1", mappingRevision: "old"})
            out.validated = step("validate#Import", {operationKey: "spec-dt-key-1", mappingRevision: out.staged.mappingRevision})
            out.before = translation("Spec transfer one", "es")
            out.noPreview = step("apply#Import", {operationKey: "spec-dt-key-1"})
            // the mapping changes after the review: the preview its caller holds is stale
            out.remapped = step("configure#Import", {
                operationKey: "spec-dt-key-1",
                mappingRevision: out.validated.mappingRevision,
                valueMappings: {localized: {Dos: "Dos!"}}
            })
            out.stale = step("apply#Import", {operationKey: "spec-dt-key-1", inputFingerprint: out.validated.inputFingerprint})
            out.afterStale = translation("Spec transfer one", "es")
            out.revalidated = step("validate#Import", {
                operationKey: "spec-dt-key-1",
                mappingRevision: out.remapped.mappingRevision
            })
            out.applied = step("apply#Import", {
                operationKey: "spec-dt-key-1",
                inputFingerprint: out.revalidated.inputFingerprint
            })
            out.auditsAfter = audits("Spec transfer one")
            out.repeat = step("apply#Import", {operationKey: "spec-dt-key-1", inputFingerprint: out.applied.inputFingerprint})
            out.oldPreview = step("apply#Import", {
                operationKey: "spec-dt-key-1",
                inputFingerprint: out.revalidated.inputFingerprint
            })
            out.report = step("get#ImportReport", {operationKey: "spec-dt-key-1"})
        })
    },
    then() {
        const k = context.keyed
        k.staged.operationKey == "spec-dt-key-1"
        // a file's name is its last part, with nothing a template would expand
        k.staged.filename == "spec $_{evil}.csv"
        k.again.repeated == true
        k.again.operationKey == "spec-dt-key-1"
        k.changed.refused == true
        k.changed.reasonCode == "OPERATION_KEY_CONFLICT"
        k.derived.operationKey == k.derivedAgain.operationKey
        k.derivedAgain.repeated == true
        k.forced.operationKey != k.derived.operationKey
        k.staleMap.reasonCode == "STALE_MAPPING"
        k.validated.validCount == 3
        !!k.validated.inputFingerprint
        // a validation writes nothing of the owner's
        k.before == null
        k.noPreview.reasonCode == "STALE_PREVIEW"
        k.stale.refused == true
        k.stale.reasonCode == "STALE_PREVIEW"
        k.afterStale == null
        k.applied.appliedCount == 3
        k.applied.changedRows == 3
        k.applied.state == "complete"
        translation("Spec transfer one", "es") == "Uno"
        translation("Spec transfer two", "es") == "Dos!"
        k.auditsAfter == 1
        // the same apply again answers the import as it stands and writes nothing
        k.repeat.repeated == true
        k.repeat.changedRows == 0
        k.repeat.appliedCount == 3
        k.oldPreview.changedRows == 0
        audits("Spec transfer one") == 1
        // a cell a spreadsheet would read as a formula is written as text
        k.report.reportCsv.includes('"\'=Spec transfer formula"')
        k.report.filename == "messageTranslations-import-results.csv"
    }
})

feature("each row is on its own and only failed rows are retried: an applied row is never applied again", {
    when() {
        const out = (context.rows = {})
        const content = [
            "original,locale,localized",
            "Spec transfer greeting,es,Saludo nuevo",
            "Spec transfer bye,es,Adios",
            "Spec transfer bye,es,Adios otra vez",
            "Spec transfer ${name},es,Hola sin nombre",
            "Spec transfer link,es,https://example.test/x",
            "Spec transfer bad locale,spanish,Hola"
        ].join("\n")
        as(users.run, () => {
            out.staged = step("stage#Import", {sourceText: content})
            out.validated = step("validate#Import", {
                operationKey: out.staged.operationKey,
                mappingRevision: out.staged.mappingRevision
            })
        })
        // the translation one row would replace changes after the preview
        const changed = ec.entity.makeValue(MESSAGE)
        changed.setFields({original: "Spec transfer greeting", locale: "es", localized: "Saludo de otro"}, true, null, null)
        changed.update()
        as(users.run, () => {
            out.applied = step("apply#Import", {
                operationKey: out.staged.operationKey,
                inputFingerprint: out.validated.inputFingerprint
            })
            out.byeAudits = audits("Spec transfer bye")
            out.afterApply = translation("Spec transfer greeting", "es")
            out.failedOnly = step("get#Transfer", {operationKey: out.staged.operationKey, errorsOnly: true})
            out.retry = step("retry#ImportItems", {
                operationKey: out.staged.operationKey,
                itemSeqIdList: [1, 2, 6, 99],
                retryKey: "spec-dt-retry-1"
            })
            out.retryAgain = step("retry#ImportItems", {
                operationKey: out.staged.operationKey,
                itemSeqIdList: [1, 2, 6, 99],
                retryKey: "spec-dt-retry-1"
            })
            out.none = step("retry#ImportItems", {operationKey: out.staged.operationKey})
            out.after = step("get#Transfer", {operationKey: out.staged.operationKey})
        })
        out.clean = invoke(T + "clean#TransferBatches")
        out.view = as(users.view, () => step("retry#ImportItems", {operationKey: out.staged.operationKey, itemSeqIdList: [1]}))
    },
    then() {
        const r = context.rows
        // the preview: each row with its own verdict, by the owner's rules
        r.validated.validCount == 2
        rowOf(r.validated, 1).result == "valid"
        rowOf(r.validated, 1).warnings[0] == "Replaces the translation there"
        rowOf(r.validated, 3).result == "invalid"
        has(rowOf(r.validated, 3).errorMessage, /Repeats row 3/)
        rowOf(r.validated, 4).result == "invalid"
        has(rowOf(r.validated, 4).errorMessage, /must keep/)
        has(rowOf(r.validated, 5).errorMessage, /link or a path/)
        has(rowOf(r.validated, 6).errorMessage, /locale/)
        // one row's source changed since the preview: it is not written and is named; the other is applied
        r.applied.state == "partial"
        r.applied.appliedCount == 1
        r.applied.attemptedCount == 2
        r.applied.changedRows == 1
        rowOf(r.applied, 1).result == "failed"
        rowOf(r.applied, 1).reasonCode == "STALE_SOURCE"
        rowOf(r.applied, 2).result == "applied"
        r.afterApply == "Saludo de otro"
        translation("Spec transfer bye", "es") == "Adios"
        r.failedOnly.shownCount == 5
        // the retry: the failed row is validated again and applied; an applied, an invalid and an unknown row are named
        r.retry.applied == true
        r.retry.appliedCount == 1
        resultOf(r.retry, 1).applied == true
        resultOf(r.retry, 2).reasonCode == "NOT_FAILED"
        has(resultOf(r.retry, 2).reason, /not applied again/)
        resultOf(r.retry, 6).reasonCode == "NOT_FAILED"
        resultOf(r.retry, 99).reasonCode == "NOT_FOUND"
        translation("Spec transfer greeting", "es") == "Saludo nuevo"
        audits("Spec transfer bye") == r.byeAudits
        r.retryAgain.repeated == true
        r.retryAgain.appliedCount == 1
        r.none.reasonCode == "NONE_FAILED"
        r.after.appliedCount == 2
        r.after.state == "partial"
        has(r.view.error, /ADMIN_TRANSFER_RUN is required/)
        // retention leaves a batch within its days as it is
        r.clean.removedCount == 0
        r.clean.purgedCount == 0
    }
})
