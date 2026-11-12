// Readiness, refresh, the settings index and the support summary, org.moqui.admin.ReadinessServices
// (plans/archive/Admin-Screens-Down.md A6; first IronStone Admin's registries, mr-boss plan/design-admin.md §3, §7, §10, §14, §16):
// each is read from the owners' registrations (moqui.server ReadinessProvider, RefreshOperation, SettingsEntry), and a
// registered service is called only because a row names it, as the caller. A provider that fails is named while the
// others answer; one the caller may not read, one whose service refuses the caller and one naming no service are left
// out; a scope reaches only a provider whose registration names it. A refresh needs its registration's permission and
// answers a safe result. A settings entry is listed only to a caller whose own grants open its screen and who holds its
// view permission. The support summary is a reader of the overview's; redacted, it names no one and nothing of the
// server's inside. The registrations, permissions, users and grants are the spec's own, removed after.
// Run: ./moq test framework --spec AdminReadiness
const S = "org.moqui.admin.ReadinessServices."
const PROVIDER = "moqui.server.ReadinessProvider"
const REFRESH = "moqui.server.RefreshOperation"
const SETTINGS = "moqui.server.SettingsEntry"
const CHECK = S + "check#InstanceSetup"
const users = {reader: "SPEC_RD_READER", operator: "SPEC_RD_OPERATOR", none: "SPEC_RD_NONE"}
const groups = {reader: "SPEC_RD_READERS", operator: "SPEC_RD_OPERATORS"}
const RUN = "SPEC_RD_REFRESH"
const OTHER = "SPEC_RD_OTHER"
const OWNER = "Spec readiness"
const key = name => "spec.readiness." + name
const screen = path => "component://framework/screen/SpecReadiness/" + path + ".xml"

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
const readiness = (user, parameters) => as(user, () => invoke(S + "get#Readiness", parameters))
const keysOf = found => Array.from(found.checks || []).map(row => row.providerKey)
const own = found => keysOf(found).filter(providerKey => providerKey.startsWith("spec.readiness."))
const settings = (user, parameters) =>
    Array.from(as(user, () => invoke(S + "find#Settings", Object.assign({ownerApp: OWNER}, parameters))).entries || []).map(
        row => row.settingsKey
    )

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const [name, userId] of Object.entries(users))
            make("moqui.security.UserAccount", {
                userId,
                username: userId.toLowerCase().replace(/_/g, "."),
                userFullName: "Spec readiness " + name
            })
        for (const userPermissionId of [RUN, OTHER])
            make("moqui.security.UserPermission", {userPermissionId, description: "Spec readiness"})
        const grants = {
            reader: ["ADMIN_OVERVIEW_VIEW"],
            operator: ["ADMIN_OVERVIEW_VIEW", "ADMIN_SETTINGS_VIEW", "ADMIN_CONNECTION_VIEW", RUN]
        }
        for (const [name, userGroupId] of Object.entries(groups)) {
            make("moqui.security.UserGroup", {userGroupId, description: "Spec readiness " + name})
            make("moqui.security.UserGroupMember", {userGroupId, userId: users[name], fromDate: 0})
            for (const userPermissionId of grants[name])
                make("moqui.security.UserGroupPermission", {userGroupId, userPermissionId, fromDate: 0})
        }
        // providers: each names the framework's own check service, which knows none of these keys, so it fails for a
        // reader of the overview and refuses anyone else
        const provider = (name, fields) =>
            make(
                PROVIDER,
                Object.assign(
                    {
                        providerKey: key(name),
                        version: 1,
                        ownerApp: OWNER,
                        title: "Spec " + name + " check",
                        healthGroupEnumId: "AdmHgObservability",
                        scopeEnumId: "AdmScInstance",
                        serviceName: CHECK,
                        requiredPermission: "ADMIN_OVERVIEW_VIEW",
                        fixRoute: "specFix",
                        fixId: "SPEC",
                        sequenceNum: 900
                    },
                    fields
                )
            )
        provider("failing", {})
        provider("missing", {serviceName: "org.moqui.admin.ReadinessSpec.missing#Service"})
        provider("hidden", {requiredPermission: RUN})
        provider("refusing", {requiredPermission: null})
        provider("scoped", {scopeParameters: "specScope"})
        provider("disabled", {disabled: "Y"})
        // refreshes: a harmless read of the framework's stands for an owner's service; one whose service fails
        const refresh = (name, fields) =>
            make(
                REFRESH,
                Object.assign(
                    {
                        refreshKey: key(name),
                        version: 1,
                        ownerApp: OWNER,
                        title: "Spec " + name,
                        serviceName: S + "find#Settings",
                        sourceKindEnumId: "AdmRsProjection",
                        requiredPermission: RUN,
                        scopeParameters: "text, reason",
                        providerKey: key("failing")
                    },
                    fields
                )
            )
        refresh("refresh", {})
        // check#InstanceSetup needs a providerKey, which a refresh never sends: the owner's service fails
        refresh("broken", {serviceName: CHECK, providerKey: null})
        refresh("unnamed", {serviceName: "org.moqui.admin.ReadinessSpec.missing#Service", providerKey: null})
        // settings entries over screens only the operators are granted: one by an allow of its own, one below an
        // inheritable allow, one below a deny, one below an allow that is not inheritable
        const artifact = (location, inheritAuthz, authzTypeEnumId) => {
            const artifactGroupId = "SPEC_RD_" + location.replace(/\W/g, "_").slice(-40)
            make("moqui.security.ArtifactGroup", {artifactGroupId, description: "Spec readiness"})
            make("moqui.security.ArtifactGroupMember", {
                artifactGroupId,
                artifactName: screen(location),
                artifactTypeEnumId: "AT_XML_SCREEN",
                inheritAuthz
            })
            make("moqui.security.ArtifactAuthz", {
                artifactAuthzId: artifactGroupId,
                artifactGroupId,
                userGroupId: groups.operator,
                authzTypeEnumId,
                authzActionEnumId: "AUTHZA_ALL"
            })
        }
        artifact("Own/Open", "N", "AUTHZT_ALLOW")
        artifact("Tree", "Y", "AUTHZT_ALLOW")
        artifact("Tree/Closed", "Y", "AUTHZT_DENY")
        artifact("Flat", "N", "AUTHZT_ALLOW")
        const entry = (name, location, fields) =>
            make(
                SETTINGS,
                Object.assign(
                    {
                        settingsKey: key(name),
                        version: 1,
                        ownerApp: OWNER,
                        title: "Spec " + name + " settings",
                        description: name === "below" ? "Zebra stripes" : "Spec",
                        settingsGroupEnumId: "AdmSgInstance",
                        scopeEnumId: "AdmScInstance",
                        destinationRoute: "specSettings",
                        screenLocation: location ? screen(location) : null,
                        sequenceNum: 900
                    },
                    fields
                )
            )
        entry("own", "Own/Open", {providerKey: key("failing")})
        entry("below", "Tree/Deep/Leaf", {})
        entry("denied", "Tree/Closed/Leaf", {})
        entry("flat", "Flat/Leaf", {})
        entry("ungranted", "Nowhere/Leaf", {})
        entry("permission", "Own/Open", {viewPermission: OTHER})
        entry("placeless", "Own/Open", {destinationRoute: null})
        entry("screenless", null, {})
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const like = (entityName, field, prefix) =>
            find(entityName)
                .condition(field, "like", prefix + "%")
                .disableAuthz()
                .deleteAll()
        like(SETTINGS, "settingsKey", "spec.readiness.")
        like(REFRESH, "refreshKey", "spec.readiness.")
        like(PROVIDER, "providerKey", "spec.readiness.")
        like("moqui.security.ArtifactAuthz", "artifactAuthzId", "SPEC_RD_")
        like("moqui.security.ArtifactGroupMember", "artifactGroupId", "SPEC_RD_")
        like("moqui.security.ArtifactGroup", "artifactGroupId", "SPEC_RD_")
        like("moqui.security.UserGroupPermission", "userGroupId", "SPEC_RD_")
        like("moqui.security.UserGroupMember", "userGroupId", "SPEC_RD_")
        like("moqui.security.UserGroup", "userGroupId", "SPEC_RD_")
        like("moqui.security.UserPermission", "userPermissionId", "SPEC_RD_")
        like("moqui.security.UserAccount", "userId", "SPEC_RD_")
    }
})

feature(
    "readiness names a failing provider while the others answer and leaves out one the caller cannot read, one that refuses and one naming no service",
    {
        when() {
            const reader = readiness(users.reader, {})
            const checks = Array.from(reader.checks || [])
            const failing = checks.find(row => row.providerKey === key("failing")) || {}
            const reportMail = checks.find(row => row.checkKey === "admin.reportMail") || {}
            return {
                reader,
                failing,
                reportMail,
                readerOwn: own(reader).sort(),
                // the framework's own checks answer beside it
                mail: checks.filter(row => row.providerKey === "admin.mailServer").length,
                unknownStates: checks.filter(row => !["ready", "warning", "failed", "unavailable"].includes(row.state)).length,
                operatorOwn: own(readiness(users.operator, {})).sort(),
                // no permission: the provider with none asked refuses this caller, and is left out, not named
                none: readiness(users.none, {}),
                // a scope reaches only the provider whose registration names it; one that takes none of it is left out
                scoped: keysOf(readiness(users.reader, {scopeParameters: {specScope: "x", other: "y"}})),
                // the instance's own: a provider whose registration names a scope is left out
                instance: own(readiness(users.reader, {instanceOnly: true})).sort(),
                narrowed: keysOf(readiness(users.reader, {providerKey: key("failing"), state: "unavailable"})),
                ready: keysOf(readiness(users.reader, {providerKey: key("failing"), state: "ready"}))
            }
        },
        then({
            reader,
            failing,
            reportMail,
            readerOwn,
            mail,
            unknownStates,
            operatorOwn,
            none,
            scoped,
            instance,
            narrowed,
            ready
        }) {
            reader.error == ""
            failing.state == "unavailable"
            failing.reasonCode == "PROVIDER_FAILED"
            failing.message == "Spec failing check could not be checked"
            failing.subject == "Instance"
            // the fix place is a key with its record, never a URL
            failing.fixRoute == "specFix"
            failing.fixId == "SPEC"
            // a check's own fix place replaces its provider's: report mail with no mail server is fixed at the mail
            // relays, not at the email templates its registration names
            reportMail.state == "ready" || reportMail.fixRoute == "adminMailRelay"
            reader.failedProviders.includes("Spec failing check")
            // hidden needs a permission the reader lacks, missing names no service, disabled is off: none is shown or named
            readerOwn == [key("failing"), key("refusing"), key("scoped")]
            !reader.failedProviders.some(title => /hidden|missing|disabled/.test(title))
            reader.ownerOptions.includes(OWNER)
            Array.from(reader.groupOptions).some(row => row.key == "AdmHgObservability")
            mail == 1
            unknownStates == 0
            operatorOwn == [key("failing"), key("hidden"), key("refusing"), key("scoped")]
            none.error == ""
            Array.from(none.checks).length == 0
            Array.from(none.failedProviders).length == 0
            !Array.from(none.ownerOptions).includes("System")
            // the owner's only provider this caller may read refuses them: the owner is not offered
            !Array.from(none.ownerOptions).includes(OWNER)
            scoped == [key("scoped")]
            instance == [key("failing"), key("refusing")]
            narrowed == [key("failing")]
            ready.length == 0
        }
    }
)

feature("a refresh needs its registration's permission, runs only a registered service and answers a safe result", {
    when() {
        const request = {
            refreshKey: key("refresh"),
            reason: "Spec",
            operationKey: "spec-refresh-1",
            scopeParameters: {text: "x", ownerApp: "y"}
        }
        const offered = user =>
            (Array.from(readiness(user, {providerKey: key("failing")}).checks || [])[0] || {}).refreshKey || null
        return {
            reader: as(users.reader, () => invoke(S + "request#Refresh", request)),
            operator: as(users.operator, () => invoke(S + "request#Refresh", request)),
            broken: as(users.operator, () => invoke(S + "request#Refresh", {refreshKey: key("broken")})),
            unnamed: as(users.operator, () => invoke(S + "request#Refresh", {refreshKey: key("unnamed")})),
            unknown: as(users.operator, () => invoke(S + "request#Refresh", {refreshKey: key("none")})),
            // a service name in place of a key is no registration
            forged: as(users.operator, () => invoke(S + "request#Refresh", {refreshKey: S + "find#Settings"})),
            offeredOperator: offered(users.operator),
            offeredReader: offered(users.reader)
        }
    },
    then({reader, operator, broken, unnamed, unknown, forged, offeredOperator, offeredReader}) {
        reader.error.includes("REFRESH_DENIED")
        operator.error == ""
        operator.state == "requested"
        operator.title == "Spec refresh"
        // an owner's failure is the operation's safe result, without its text
        broken.error == ""
        broken.state == "failed"
        broken.message == "Spec broken could not be requested"
        unnamed.error.includes("REFRESH_UNAVAILABLE")
        unknown.error.includes("REFRESH_UNAVAILABLE")
        forged.error.includes("REFRESH_UNAVAILABLE")
        // the refresh registered for a provider is offered with its check only to one who may run it
        offeredOperator == key("refresh")
        offeredReader == null
    }
})

feature("a settings entry is listed only to a caller whose own grants open its screen and who holds its view permission", {
    when() {
        const all = as(users.operator, () => invoke(S + "find#Settings", {ownerApp: OWNER}))
        return {
            all,
            entry: Array.from(all.entries || []).find(row => row.settingsKey === key("own")) || {},
            operator: settings(users.operator, {}).sort(),
            reader: settings(users.reader, {}),
            readerApps: Array.from(as(users.reader, () => invoke(S + "find#Settings", {})).appOptions || []),
            searched: settings(users.operator, {text: "ZEBRA"}),
            otherGroup: settings(users.operator, {settingsGroupEnumId: "AdmSgApplications"}),
            // the instance administrator opens the framework's own
            administrator: Array.from(invoke(S + "find#Settings", {settingsGroupEnumId: "AdmSgInstance"}).entries || []).map(
                row => row.settingsKey
            )
        }
    },
    then({all, entry, operator, reader, readerApps, searched, otherGroup, administrator}) {
        all.error == ""
        // own: an allow of its own; below: under an inheritable allow. Not: under a deny, under an allow that is not
        // inheritable, with no grant, with a permission not held, with no place or no screen
        operator == [key("below"), key("own")]
        entry.destinationRoute == "specSettings"
        entry.providerKey == key("failing")
        entry.groupName == "Instance"
        entry.url == null
        reader.length == 0
        !readerApps.includes(OWNER)
        searched == [key("below")]
        otherGroup.length == 0
        administrator.includes("admin.instanceSettings")
    }
})

feature("the overview and the support summary are a reader's; the summary's JSON is redacted", {
    when() {
        const address = /[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/
        const overview = as(users.reader, () => invoke(S + "get#Overview", {}))
        // the cards alone, with no provider run
        const healthCards = as(users.reader, () => invoke(S + "get#HealthCards", {}))
        const redacted = as(users.reader, () => invoke(S + "get#SupportSummary", {redacted: true})).summary
        // the operator reads the settings and the connections too
        const plain = as(users.operator, () => invoke(S + "get#SupportSummary", {})).summary
        const administrator = as(users.operator, () => invoke(S + "get#SupportSummary", {redacted: true})).summary
        const inside = row => /\.(host|listenHost|mediaHost|allowedAddresses)$|File$/.test(row.settingKey)
        return {
            overview,
            cards: Array.from(overview.cards || []),
            healthCards: Array.from(healthCards.cards || []),
            healthCardsError: healthCards.error,
            refusedCards: as(users.none, () => invoke(S + "get#HealthCards", {})),
            attention: Array.from(overview.attention || []),
            refusedOverview: as(users.none, () => invoke(S + "get#Overview", {})),
            redacted,
            redactedText: JSON.stringify(redacted),
            plain,
            insideRows: administrator.settings.settings.filter(inside),
            address,
            refused: as(users.none, () => invoke(S + "get#SupportSummary", {})),
            check: as(users.none, () => invoke(CHECK, {providerKey: "admin.mailServer"}))
        }
    },
    then({
        overview,
        cards,
        healthCards,
        healthCardsError,
        refusedCards,
        attention,
        refusedOverview,
        redacted,
        redactedText,
        plain,
        insideRows,
        address,
        refused,
        check
    }) {
        overview.error == ""
        cards.map(card => card.key) == ["failedJobs", "transports", "backup", "signIn"]
        // a card's place is a key, with what the list is narrowed by
        cards.map(card => card.route) == ["adminJobRuns", "adminSystemMessages", "adminJob", "adminVisits"]
        cards[0].parameters.state == "failed"
        healthCardsError == ""
        healthCards.map(card => card.key + " " + card.route) == cards.map(card => card.key + " " + card.route)
        // the backup coverage from the backup check alone is the overview's
        healthCards[2].value == cards[2].value
        healthCards[3].value != "Unavailable"
        refusedCards.error.includes("ADMIN_OVERVIEW_VIEW")
        attention.length > 0 && attention.length <= 10
        attention.every(row => row.state != "ready")
        overview.failedProviders.includes("Spec failing check")
        refusedOverview.error.includes("ADMIN_OVERVIEW_VIEW")
        redacted.redacted == true
        redacted.instance.preparedBy == null
        redacted.instance.version.length > 0
        redacted.readiness.available == true
        redacted.readiness.checks.length > 0
        redacted.readiness.failedProviders.includes("Spec failing check")
        // no fix place, refresh or scope, and no address
        !redactedText.includes("fixRoute") && !redactedText.includes("refreshKey") && !redactedText.includes('"scope"')
        !address.test(redactedText)
        // the reader holds neither the connections' nor the settings' permission: each section says so
        redacted.protocols.available == false
        redacted.settings.available == false
        redacted.settings.reason.length > 0
        // for one who reads the settings: a setting that names the server's inside is only set or not in the redacted JSON
        insideRows.length > 0
        insideRows.every(row => row.internal && row.value == null)
        plain.redacted == false
        plain.instance.preparedBy != null
        plain.settings.available == true
        plain.settings.settings.every(row => !row.internal)
        plain.settings.settings.filter(row => row.masked).every(row => row.value == null && row.shownValue.startsWith("Hidden"))
        plain.protocols.available == true
        refused.error.includes("ADMIN_OVERVIEW_VIEW")
        check.error.includes("ADMIN_OVERVIEW_VIEW")
    }
})
