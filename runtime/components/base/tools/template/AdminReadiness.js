// What the Health, settings index and support summary screens show (System › Operations › Health and SupportSummary,
// System › Settings › SettingsHome; plans/archive/Admin-Screens-Down.md A6), over org.moqui.admin.ReadinessServices, which
// check the caller themselves. A refusal is shown as its reason (AdminCalls.read). A place a registration names is a
// key: it is linked by the mount's routes (AdminMount.js), a screen path by url and parameters, which the link widget
// disables for a viewer the screen does not open to, another app's page by href; a key with no route here is no link.
// An app that shows a screen of its own around these screens' sections calls the same functions (health with the
// scope its own find form adds).
const calls = ec.resource.script("component://framework/service/org/moqui/admin/AdminCalls.js")
const mounts = ec.resource.script("component://tools/template/AdminMount.js")
const SERVICES = "org.moqui.admin.ReadinessServices."

// A link's fields under a prefix, for the two link widgets of a row: <prefix>Href for another app's page,
// <prefix>Url with <prefix>Parameters for a screen path; each empty when there is no place. A link to a screen carries
// what the target declares of the linking screen's context, a row's fields among them, so the names in clear (the
// list's own find values and row fields) are sent empty unless the link gives them a value.
function linkFields(prefix, link, clear) {
    const screen = !!link && !link.plain
    return {
        [prefix + "Href"]: link && link.plain ? link.href : "",
        [prefix + "Url"]: screen ? link.url : "",
        [prefix + "Parameters"]: screen
            ? Object.assign(Object.fromEntries((clear || []).map(name => [name, ""])), link.parameters)
            : {}
    }
}
const healthNames = ["healthGroupEnumId", "state", "ownerApp", "providerKey", "title", "text"]
const settingsNames = ["text", "ownerApp", "settingsGroupEnumId", "state", "healthGroupEnumId", "title"]
// A check as a list names it: "Provider: Check" when the provider's title and the check's differ (People setup:
// Departments), else the one title.
const checkTextOf = row =>
    row.providerTitle && row.title && row.providerTitle !== row.title
        ? row.providerTitle + ": " + row.title
        : row.title || row.providerTitle || ""
const freshnessOf = row =>
    (row.watermarkText ? "Last " + row.watermarkText + (row.lagText ? ", " + row.lagText + " ago" : "") : "") +
    (row.expectedFrequencyText ? (row.watermarkText ? "; " : "") + "every " + row.expectedFrequencyText : "")

// Health: the checks the viewer may read, narrowed by the find's values and, for an app, a scope; each row with its
// fix link, what "Check again" sends (its provider and its scope) and the scope a refresh of it is asked with.
function health(findValues, scopeParameters) {
    const state = mounts.state()
    const find = findValues || {}
    const called = calls.read(
        SERVICES + "get#Readiness",
        {
            healthGroupEnumId: find.healthGroupEnumId || null,
            state: find.state || null,
            ownerApp: find.ownerApp || null,
            providerKey: find.providerKey || null,
            scopeParameters: scopeParameters || null
        },
        "The checks are not available to you."
    )
    const found = called.out || {}
    const checks = Array.from(found.checks || []).map((row, index) =>
        Object.assign(
            {},
            row,
            linkFields(
                "fix",
                row.fixRoute ? mounts.link(state.routes, row.fixRoute, row.fixId, row.fixParameters) : null,
                healthNames
            ),
            {
                checkText: checkTextOf(row),
                freshnessText: freshnessOf(row),
                // the row's own state, group and owner are not the find's: sent empty, so the link leaves them out
                againParameters: Object.assign({}, row.scope || {}, {
                    providerKey: row.providerKey,
                    state: "",
                    healthGroupEnumId: "",
                    ownerApp: ""
                }),
                scopeJson: JSON.stringify(row.scope || {}),
                dialogKey: "RequestRefresh_" + index
            }
        )
    )
    return {
        checks,
        failure: called.failure,
        failedText: Array.from(found.failedProviders || []).join(", "),
        checkedText: found.checkedText || "",
        ownerOptions: Array.from(found.ownerOptions || []).map(owner => ({key: owner, text: owner})),
        groupOptions: Array.from(found.groupOptions || [])
    }
}
// One of the health cards (get#HealthCards: no provider is run) as a number card shows it: {title, value, icon, link},
// link the URL with its query string of the place the mount gives its key ("" for none), or null for a viewer who may not
// read the cards.
function healthCard(key) {
    const called = calls.read(SERVICES + "get#HealthCards", {}, "The health cards are not available to you.")
    const card = Array.from(called.out?.cards || []).find(row => row.key === key)
    if (!card) return null
    const link = card.route ? mounts.link(mounts.state().routes, card.route, card.routeId, card.parameters) : null
    return {title: card.title, value: card.value, icon: card.icon, link: link ? link.href : ""}
}
// A refresh's scope as the row's dialog sent it: a plain map, or none for anything else.
function scopeOf(scopeJson) {
    try {
        const scope = JSON.parse(String(scopeJson || "{}"))
        return scope && typeof scope === "object" && !Array.isArray(scope) ? scope : {}
    } catch (error) {
        return {}
    }
}

// The settings index: the entries the viewer may open that have a place under this mount, each with its link and,
// where it names a provider, Health narrowed to it.
function settingsIndex(findValues) {
    const state = mounts.state()
    const find = findValues || {}
    const called = calls.read(
        SERVICES + "find#Settings",
        {text: find.text || null, ownerApp: find.ownerApp || null, settingsGroupEnumId: find.settingsGroupEnumId || null},
        "The settings index is not available to you."
    )
    const found = called.out || {}
    const entries = []
    for (const entry of Array.from(found.entries || [])) {
        const link = mounts.link(state.routes, entry.destinationRoute)
        if (!link) continue
        entries.push(
            Object.assign(
                {},
                entry,
                linkFields("destination", link, settingsNames),
                linkFields(
                    "readiness",
                    entry.providerKey ? mounts.link(state.routes, "adminHealth", entry.providerKey) : null,
                    settingsNames
                )
            )
        )
    }
    const apps = new Set(entries.map(entry => entry.ownerApp))
    return {
        entries,
        failure: called.failure,
        // the apps of the entries that have a place here, whatever the find narrowed to
        appOptions: Array.from(found.appOptions || [])
            .filter(app => find.text || find.ownerApp || find.settingsGroupEnumId || apps.has(app))
            .map(app => ({key: app, text: app})),
        groupOptions: Array.from(found.groupOptions || [])
    }
}

// The support summary as the screen and its print show it; the redacted JSON is the service's own answer.
function supportSummary() {
    const called = calls.read(SERVICES + "get#SupportSummary", {redacted: false}, "The support summary is not available to you.")
    const summary = called.out?.summary || null
    const instance = summary?.instance || {}
    const readinessSection = summary?.readiness || {available: false, checks: []}
    const counts = readinessSection.counts || {}
    const protocolSection = summary?.protocols || {available: false, protocols: []}
    const settingSection = summary?.settings || {available: false, settings: []}
    return {
        failure: called.failure,
        summary,
        identityText: summary
            ? [instance.displayName, "MR Moqui " + instance.version, instance.packageName].filter(Boolean).join(" · ")
            : "",
        preparedText: summary
            ? "Prepared " + instance.preparedText + (instance.preparedBy ? " by " + instance.preparedBy : "")
            : "",
        readinessSection,
        countsText: readinessSection.available
            ? ["failed", "unavailable", "warning", "ready"].map(state => (counts[state] || 0) + " " + state).join(", ")
            : "",
        failedText: Array.from(readinessSection.failedProviders || []).join(", "),
        checks: Array.from(readinessSection.checks || []).map(row => Object.assign({}, row, {checkText: checkTextOf(row)})),
        protocolSection,
        protocols: Array.from(protocolSection.protocols || []).map(row =>
            Object.assign({}, row, {
                checksText: Array.from(row.checks || [])
                    .map(
                        check =>
                            check.title +
                            ": " +
                            (check.lastCheckPassed == null ? "not run" : check.lastCheckPassed ? "passed" : "did not pass")
                    )
                    .join("; ")
            })
        ),
        settingSection,
        settings: Array.from(settingSection.settings || []).map(row =>
            Object.assign({}, row, {restartText: row.restartRequired ? "After a restart" : ""})
        )
    }
}

;({health, healthCard, scopeOf, settingsIndex, supportSummary, linkFields})
