// Delegated configuration administration, org.moqui.admin.ConfigurationServices' settings and localization parts (mr-boss
// plan/impl-admin.md P8; design-admin §7 InstanceSettings and Localization): an instance preference is a registered key only,
// typed and within its allowed values and limits, changed at the revision read and kept in the audit log; a secret-classified
// setting is never returned and never a preference; a translation keeps its placeholders, and a label is translated only for
// a registered label target. Each case with artifact authorization on, as a spec configuration administrator who holds
// ADMIN_SETTINGS_VIEW and ADMIN_SETTINGS_MANAGE through a group of its own (not ADMIN), and as a user who holds neither.
// Run: ./moq test framework --spec AdminConfiguration
const S = "org.moqui.admin.ConfigurationServices."
const adminUser = "spec.config.admin"
const adminId = "SPEC_CFG_ADMIN"
const otherUser = "spec.config.other"
const otherId = "SPEC_CFG_OTHER"
const groupId = "SPEC_CFG_ADMINS"
const choiceKey = "spec.config.choice"
const numberKey = "spec.config.number"
const secretKey = "spec.config.hidden"
const secretPreferenceKey = "spec.config.secretPreference"
const confKey = "spec.config.locale"
// a property the shipped conf marks is-secret under a name that does not read as one
const flaggedKey = "spec.config.account"
// an original whose value the expander ends at another } than its first: a translation may not add to it
const complexOriginal = "Spec config ${ok ? '}' : no} done"
const original = "Spec config ${count} items for ${name}"
const enumId = "SpecCfgLabel"
const firstPreferences = [
    "instance.displayName",
    "instance.defaultTimeZone",
    "signin.newDeviceWindowDays",
    "report.groupRecipientCeiling"
]

const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const find = (entityName, conditions) => ec.entity.find(entityName).condition(conditions).disableAuthz()
const rows = (entityName, conditions) => Array.from(find(entityName, conditions).list())
// Run action signed in as username with artifact authorization on, then as john.doe again.
function as(username, action) {
    ec.user.internalLoginUser(username)
    ec.artifactExecution.enableAuthz()
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
    }
}
// A service's result, or {error} with its message.
function invoke(name, parameters = {}) {
    try {
        const result =
            ec.service
                .sync()
                .name(S + name)
                .parameters(parameters)
                .call() || {}
        const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
        ec.message.clearErrors()
        return Object.assign({}, result, {error})
    } catch (e) {
        ec.message.clearErrors()
        return {error: String(e.message || e)}
    }
}
const setting = key => Array.from(invoke("get#EffectiveSettings", {settingKey: key}).settingList || [])[0]
const audits = (entityName, pk) => rows("moqui.entity.EntityAuditLog", {changedEntityName: entityName, pkPrimaryValue: pk})

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        make("moqui.security.UserAccount", {
            userId: adminId,
            username: adminUser,
            userFullName: "Spec configuration administrator"
        })
        make("moqui.security.UserAccount", {userId: otherId, username: otherUser, userFullName: "Spec configuration other"})
        make("moqui.security.UserGroup", {userGroupId: groupId, description: "Spec configuration administrators"})
        for (const permission of ["ADMIN_SETTINGS_VIEW", "ADMIN_SETTINGS_MANAGE"])
            make("moqui.security.UserGroupPermission", {userGroupId: groupId, userPermissionId: permission, fromDate: 0})
        make("moqui.security.UserGroupMember", {userGroupId: groupId, userId: adminId, fromDate: 0})
        const settings = [
            {
                settingKey: choiceKey,
                title: "Spec choice",
                valueTypeEnumId: "IsvChoice",
                allowedValues: "compact, comfortable",
                defaultValue: "comfortable",
                editable: "Y"
            },
            {
                settingKey: numberKey,
                title: "Spec number",
                valueTypeEnumId: "IsvInteger",
                minValue: 1,
                maxValue: 500,
                defaultValue: "200",
                editable: "Y"
            },
            // a secret-classified configuration value, and a registration that wrongly makes a secret editable
            {
                settingKey: secretKey,
                title: "Spec secret",
                valueTypeEnumId: "IsvText",
                propertyName: "default_locale",
                isSecret: "Y"
            },
            {
                settingKey: secretPreferenceKey,
                title: "Spec secret preference",
                valueTypeEnumId: "IsvText",
                isSecret: "Y",
                editable: "Y"
            },
            {
                settingKey: flaggedKey,
                title: "Spec flagged",
                valueTypeEnumId: "IsvText",
                propertyName: "fcm_service_account"
            },
            {
                settingKey: confKey,
                title: "Spec locale",
                valueTypeEnumId: "IsvText",
                propertyName: "default_locale",
                restartRequired: "Y",
                operatorNote: "Set default_locale and restart"
            }
        ]
        for (const fields of settings) make("moqui.server.InstanceSetting", Object.assign({settingGroup: "Spec"}, fields))
        make("moqui.basic.EnumerationType", {enumTypeId: "SpecCfgType", description: "Spec configuration type"})
        make("moqui.basic.Enumeration", {enumId, enumTypeId: "SpecCfgType", description: "Spec label for ${name}"})
        // a registered target the definitions do not bear out: a field that is not localized
        make("moqui.basic.LocalizedLabelTarget", {
            entityName: "moqui.basic.Enumeration",
            fieldName: "enumCode",
            title: "Spec codes"
        })
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        find("moqui.server.InstancePreference", {}).condition("settingKey", "like", "spec.config.%").deleteAll()
        find("moqui.server.InstanceSetting", {}).condition("settingKey", "like", "spec.config.%").deleteAll()
        find("moqui.basic.LocalizedMessage", {original}).deleteAll()
        find("moqui.basic.LocalizedEntityField", {pkValue: enumId}).deleteAll()
        find("moqui.basic.LocalizedLabelTarget", {entityName: "moqui.basic.Enumeration", fieldName: "enumCode"}).deleteAll()
        find("moqui.basic.Enumeration", {enumId}).deleteAll()
        find("moqui.basic.EnumerationType", {enumTypeId: "SpecCfgType"}).deleteAll()
        find("moqui.security.UserGroupMember", {userGroupId: groupId}).deleteAll()
        find("moqui.security.UserGroupPermission", {userGroupId: groupId}).deleteAll()
        find("moqui.security.UserGroup", {userGroupId: groupId}).deleteAll()
        for (const userId of [adminId, otherId]) find("moqui.security.UserAccount", {userId}).deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature("a preference is a registered key, typed and within its allowed values, at the revision read", {
    when() {
        return as(adminUser, () => {
            const unregistered = invoke("store#InstancePreference", {settingKey: "spec.config.unknown", value: "x"})
            const configuration = invoke("store#InstancePreference", {settingKey: confKey, value: "es"})
            const outside = invoke("store#InstancePreference", {settingKey: choiceKey, value: "huge"})
            const overLimit = invoke("store#InstancePreference", {settingKey: numberKey, value: "501"})
            const notNumber = invoke("store#InstancePreference", {settingKey: numberKey, value: "many"})
            const expression = invoke("store#InstancePreference", {
                settingKey: "instance.displayName",
                value: "${ec.user.userId}",
                expectedRevision: ""
            })
            const before = setting(numberKey)
            const stored = invoke("store#InstancePreference", {
                settingKey: numberKey,
                value: "120",
                expectedRevision: "",
                reason: "Spec ceiling"
            })
            const after = setting(numberKey)
            const stale = invoke("store#InstancePreference", {settingKey: numberKey, value: "130", expectedRevision: ""})
            const read = invoke("get#InstancePreference", {settingKey: numberKey})
            const removed = invoke("store#InstancePreference", {
                settingKey: numberKey,
                value: "",
                expectedRevision: stored.revision
            })
            return {
                unregistered,
                configuration,
                outside,
                overLimit,
                notNumber,
                expression,
                before,
                stored,
                after,
                stale,
                read,
                removed,
                restored: invoke("get#InstancePreference", {settingKey: numberKey}),
                stray: rows("moqui.server.InstancePreference", {settingKey: choiceKey}).length,
                audit: audits("moqui.server.InstancePreference", numberKey)
            }
        })
    },
    then({
        unregistered,
        configuration,
        outside,
        overLimit,
        notNumber,
        expression,
        before,
        stored,
        after,
        stale,
        read,
        removed,
        restored,
        stray,
        audit
    }) {
        unregistered.applied === false && unregistered.reasonCode == "NOT_REGISTERED"
        configuration.applied === false && configuration.reasonCode == "NOT_REGISTERED"
        outside.applied === false && outside.reasonCode == "VALUE_INVALID" && outside.reason.includes("comfortable")
        overLimit.reasonCode == "VALUE_INVALID" && notNumber.reasonCode == "VALUE_INVALID"
        // a text preference holds no expression
        expression.applied === false && expression.reasonCode == "VALUE_INVALID"
        stray == 0
        before.value == "200" && before.source == "default" && before.canEdit === true
        stored.applied === true && stored.value == "120" && !!stored.revision
        after.value == "120" &&
            after.source == "preference" &&
            after.changedByUserId == adminId &&
            after.changeReason == "Spec ceiling"
        stale.applied === false && stale.reasonCode == "STALE_REVISION" && stale.revision == stored.revision
        read.value == "120" && read.numberValue == 120 && read.source == "preference"
        removed.applied === true && restored.numberValue == 200 && restored.source == "default"
        // each value and the removal are in the audit log, the change with its reason
        audit.some(row => row.newValueText == "120" && row.changeReason == "Spec ceiling")
        audit.some(row => row.oldValueText == "120" && !row.newValueText)
    }
})

feature("a secret-classified setting is never returned or made a preference; configuration shows its source and restart effect", {
    when() {
        return as(adminUser, () => ({
            secret: setting(secretKey),
            flagged: setting(flaggedKey),
            secretPreference: setting(secretPreferenceKey),
            storeSecret: invoke("store#InstancePreference", {settingKey: secretPreferenceKey, value: "hunter2"}),
            readSecret: invoke("get#InstancePreference", {settingKey: secretKey}),
            conf: setting(confKey),
            seeded: Array.from(invoke("get#EffectiveSettings", {kind: "preference"}).settingList || []).map(
                row => row.settingKey
            ),
            all: JSON.stringify(invoke("get#EffectiveSettings").settingList || [])
        }))
    },
    then({secret, flagged, secretPreference, storeSecret, readSecret, conf, seeded, all}) {
        secret.masked === true && secret.value === null && secret.valueSet === true && secret.defaultValue === null
        // the conf's own is-secret mark masks a property whatever it is called
        flagged.masked === true && flagged.value === null
        // a registration that makes a secret editable is left out, so it is neither listed nor stored
        secretPreference === undefined
        storeSecret.applied === false && storeSecret.reasonCode == "NOT_REGISTERED"
        readSecret.error.includes("not a registered instance preference")
        conf.value == System.getProperty("default_locale") && conf.editable === false && conf.canEdit === false
        conf.restartRequired === true && conf.propertyName == "default_locale" && conf.operatorNote.includes("restart")
        conf.source == "default" || conf.source == "configuration"
        // the first registered preferences: presentation, the new-device window and the group recipient ceiling
        firstPreferences.every(key => seeded.includes(key))
        !all.includes("hunter2")
    }
})

feature("a user without the settings permissions reads and changes nothing, but code reads a preference", {
    when() {
        return as(otherUser, () => ({
            list: invoke("get#EffectiveSettings"),
            store: invoke("store#InstancePreference", {settingKey: numberKey, value: "5", expectedRevision: ""}),
            messages: invoke("find#LocalizedMessages"),
            translate: invoke("store#LocalizedMessage", {original, locale: "es", localized: "x"}),
            ceiling: invoke("get#InstancePreference", {settingKey: "report.groupRecipientCeiling"})
        }))
    },
    then({list, store, messages, translate, ceiling}) {
        list.error.includes("ADMIN_SETTINGS_VIEW") && !list.settingList
        store.error.includes("ADMIN_SETTINGS_MANAGE")
        messages.error.includes("ADMIN_SETTINGS_VIEW")
        translate.error.includes("ADMIN_SETTINGS_MANAGE")
        ceiling.error == "" && ceiling.numberValue == 200
    }
})

feature("a translation keeps its placeholders; a label is translated only for a registered target", {
    when() {
        return as(adminUser, () => {
            const dropped = invoke("store#LocalizedMessage", {original, locale: "es", localized: "Spec ${count} elementos"})
            const added = invoke("store#LocalizedMessage", {
                original,
                locale: "es",
                localized: "Spec ${count} elementos de ${name} ${ec.user.userId}"
            })
            // the original's value kept as text and an expression after it, which the expander would run
            const complex = invoke("store#LocalizedMessage", {
                original: complexOriginal,
                locale: "es",
                localized: "Spec ${ok ? '}' + ec.user.userId : no} hecho"
            })
            // a value that is more than a name or a simple call stays Tools'
            const unsupported = invoke("store#LocalizedMessage", {
                original: "Spec config ${ {a: ok}.a } done",
                locale: "es",
                localized: "Spec ${ {a: ok}.a } hecho"
            })
            const link = invoke("store#LocalizedMessage", {
                original,
                locale: "es",
                localized: "https://example.invalid/${count}/${name}"
            })
            const stored = invoke("store#LocalizedMessage", {
                original,
                locale: "es",
                localized: "Spec: ${name} tiene ${count} elementos",
                reason: "Spec translation"
            })
            const stale = invoke("store#LocalizedMessage", {
                original,
                locale: "es",
                localized: "Otra ${count} ${name}",
                expectedRevision: ""
            })
            const found = invoke("find#LocalizedMessages", {locale: "es", text: "spec config"})
            const missing = invoke("find#LocalizedMessages", {locale: "fr", text: "spec config", missing: true})
            const label = {entityName: "moqui.basic.Enumeration", fieldName: "description", pkValue: enumId, locale: "es"}
            const unregistered = invoke(
                "store#LocalizedEntityField",
                Object.assign({}, label, {
                    entityName: "moqui.security.UserAccount",
                    fieldName: "userFullName",
                    pkValue: adminId,
                    localized: "x"
                })
            )
            const notLocalized = invoke(
                "store#LocalizedEntityField",
                Object.assign({}, label, {fieldName: "enumCode", localized: "x"})
            )
            const noLabel = invoke(
                "store#LocalizedEntityField",
                Object.assign({}, label, {pkValue: "SpecCfgNone", localized: "x"})
            )
            const labelDropped = invoke("store#LocalizedEntityField", Object.assign({}, label, {localized: "Etiqueta"}))
            const labelStored = invoke("store#LocalizedEntityField", Object.assign({}, label, {localized: "Etiqueta de ${name}"}))
            const fields = invoke("find#LocalizedEntityFields", {
                targetKey: "moqui.basic.Enumeration.description",
                locale: "es",
                text: enumId
            })
            const labelMissing = invoke("find#LocalizedEntityFields", {
                targetKey: "moqui.basic.Enumeration.description",
                locale: "fr",
                text: enumId,
                missing: true
            })
            return {
                dropped,
                added,
                complex,
                unsupported,
                link,
                complexRows: rows("moqui.basic.LocalizedMessage", {original: complexOriginal}).length,
                stored,
                stale,
                found,
                missing,
                unregistered,
                notLocalized,
                noLabel,
                labelDropped,
                labelStored,
                fields,
                labelMissing,
                audit: audits("moqui.basic.LocalizedMessage", original)
            }
        })
    },
    then({
        dropped,
        added,
        complex,
        unsupported,
        link,
        complexRows,
        stored,
        stale,
        found,
        missing,
        unregistered,
        notLocalized,
        noLabel,
        labelDropped,
        labelStored,
        fields,
        labelMissing,
        audit
    }) {
        dropped.applied === false && dropped.reasonCode == "PLACEHOLDER_MISMATCH" && dropped.reason.includes("${name}")
        added.applied === false && added.reasonCode == "PLACEHOLDER_MISMATCH" && added.reason.includes("may not add")
        complex.applied === false && complex.reasonCode == "PLACEHOLDER_MISMATCH" && complexRows == 0
        unsupported.applied === false && unsupported.reasonCode == "PLACEHOLDER_UNSUPPORTED"
        link.applied === false && link.reasonCode == "PLACEHOLDER_MISMATCH" && link.reason.includes("link")
        stored.applied === true && !!stored.revision
        stale.applied === false && stale.reasonCode == "STALE_REVISION"
        found.messageList.length == 1 &&
            found.messageList[0].localized == "Spec: ${name} tiene ${count} elementos" &&
            found.messageList[0].revision == stored.revision
        missing.messageList.length == 1 && missing.messageList[0].original == original && missing.messageList[0].missing === true
        audit.length == 1 && audit[0].pkSecondaryValue == "es" && audit[0].changeReason == "Spec translation"
        unregistered.applied === false && unregistered.reasonCode == "NOT_REGISTERED"
        notLocalized.applied === false && notLocalized.reasonCode == "NOT_REGISTERED"
        noLabel.reasonCode == "LABEL_UNAVAILABLE"
        labelDropped.reasonCode == "PLACEHOLDER_MISMATCH"
        labelStored.applied === true
        fields.fieldList.length == 1 &&
            fields.fieldList[0].original == "Spec label for ${name}" &&
            fields.fieldList[0].localized == "Etiqueta de ${name}"
        // the target the definitions do not bear out is not offered
        !fields.targetList.some(target => target.targetKey.endsWith(".enumCode"))
        labelMissing.fieldList.length == 1 &&
            labelMissing.fieldList[0].pkValue == enumId &&
            labelMissing.fieldList[0].missing === true
    }
})
