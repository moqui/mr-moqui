// What the alert screens of Dashboards show and ask (screen/Dashboards/Alerts.xml, AlertRule.xml and AlertShared.xml;
// docs/tools/Monitoring.md, "The Dashboards app" and "Business alerts"; plans/archive/Admin-Screens-Down.md, Track I): the alert
// rules the viewer reads, the alerts that told them and the shipped rules to copy; one rule with its reading and
// condition, whom it tells, the state of each of its series and each recipient's last outcome; and what changes one.
// Presentation only: every read and write is an operation of org.moqui.report.ReportServices called as the viewer, so
// the engine checks the operation's grant, the rule's owner, each dataset and each recipient's scope. A rule's state is
// read through get#AlertState, which answers it to the owner and to a recipient whose scope is the owner's now; no
// entity is read here.
//
// A rule is one of two kinds, and the engine says which (get#AlertState's business):
// - an operational rule tells contact points (a notification topic, an email address, a webhook), with silences and
//   mute timings, its own or shared ones: what Tools' rules over telemetry are;
// - a business rule, one that names an audience or reads a dataset whose rows depend on the viewer, tells an audience of
//   internal users and groups, each checked and told separately, and takes no contact point.
//
// Another app shows the screens under its own root (BOSS Insights › Business alerts). Its policy (template/ReportMount.js)
// is read here, in the transitions too, and never from a request:
// - features.alert: whether the viewer may set a rule here. Every transition that saves, copies, changes or deletes one
//   refuses without it (requireSet), and the screens offer none of them;
// - alertAudience: the mount tells every rule set there to an audience (audienceOnly). Each save then writes the
//   audience's fixed parts from the policy (its notification topic, its email template where email is a channel, the
//   link a notification opens) and drops any contact point; a rule there watches a panel's saved query, never one typed;
//   and only business rules are shown or changed there;
// - recipientPermissionId: whom a rule's owner may name, through find#ReportRecipients (ReportDelivery.js recipients),
//   which every save of an audience asks first;
// - recipientCap: the most recipients an audience is saved and checked with, which the app's alerts area adds, written on
//   every save.
// The rules of the components the mount hides (reportHiddenComponents) are not shown. With no mount each of these is the
// engine's own, and a rule is edited as Tools edits it: its query typed or built, its contact points, silences and mute
// timings.
const mountLib = ec.resource.script("component://tools/template/ReportMount.js")
const delivery = ec.resource.script("component://tools/template/ReportDelivery.js")
const {ask, offered, refuse, done, recipients, namesOf, groupOptions, dateText, deliveryStates} = delivery

const unavailable = "This alert is not available"
const receivedRules = 20
// most severe first; No data and Error say the health is not known
const stateWords = {
    Error: ["Error (not known)", "warning"],
    Firing: ["Firing", "negative"],
    NoData: ["No data (not known)", "warning"],
    Pending: ["Pending", "warning"],
    Recovering: ["Recovering", "warning"],
    Normal: ["Normal", "positive"]
}
const pauseReasons = {
    "owner-disabled": "Paused until a new owner takes it",
    "transfer-pending": "Paused until its new owner resumes it",
    "not-validated": "Not active yet"
}
const conditions = [
    {value: "above", label: "Above"},
    {value: "below", label: "Below"},
    {value: "within", label: "Inside a range"},
    {value: "outside", label: "Outside a range"}
]
const reductions = [
    {value: "last", label: "Latest value"},
    {value: "max", label: "Highest"},
    {value: "min", label: "Lowest"},
    {value: "mean", label: "Average"},
    {value: "sum", label: "Total"},
    {value: "count", label: "Count"},
    {value: "delta", label: "Change"},
    {value: "p95", label: "95th percentile"}
]
const windows = [
    {value: "now-1h", label: "The last hour"},
    {value: "now-1d", label: "The last day"},
    {value: "now-7d", label: "The last 7 days"},
    {value: "now-30d", label: "The last 30 days"}
]
const intervals = [
    {value: "300", label: "Every 5 minutes"},
    {value: "900", label: "Every 15 minutes"},
    {value: "3600", label: "Hourly"},
    {value: "86400", label: "Daily"}
]
const channelWords = {notification: "In the app", email: "Email"}
const sharedKinds = {contactPoint: "Contact point", silence: "Silence", muteTiming: "Mute timing"}
const sharedRefKeys = {contactPoint: "contactPointRefs", silence: "silenceRefs", muteTiming: "muteTimingRefs"}
const weekdayNames = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]
// a new rule with no panel behind it, as Tools starts one
const blankQuery = {datasetId: "otel_metrics", from: "now-5m", thru: "now", metricGranularity: "auto", aggregations: []}

const text = value => String(value ?? "").trim()
// a word by a key that came from a rule's body or the engine: own names only, so a key such as constructor finds nothing
const pick = (map, key, fallback) => (map && Object.hasOwn(map, String(key ?? "")) ? map[String(key)] : fallback)
const idList = value =>
    (value == null || value === "" ? [] : Array.from(typeof value === "string" ? value.split(",") : value))
        .map(text)
        .filter(Boolean)
const flag = value => value === true || value === "true" || value === "Y"
// a number as the engine answers it: a fractional one arrives as a decimal, which is read through its text
const plain = value => (value == null || value === "" ? null : Number(String(value)))
const valueText = value => (plain(value) == null ? "–" : String(Math.round(plain(value) * 100) / 100))
// a definition's body as plain JSON to change and save again, its numbers numbers
const clone = value =>
    JSON.parse(
        JSON.stringify(value ?? {}, (key, item) =>
            item != null && typeof item === "object" && !Array.isArray(item) && typeof item.toNumber === "function"
                ? plain(item)
                : item
        )
    )
const plural = (count, one, many) => count + " " + (count === 1 ? one : many)

// ===== refusals =====
// A transition's refusal, in its own words: thrown where it is found and shown as the transition's error (attempt).
function fail(words) {
    const error = new Error(words)
    error.refusal = true
    throw error
}
function attempt(action) {
    try {
        return action()
    } catch (error) {
        if (error?.refusal) return refuse(error.message)
        throw error
    }
}
function number(value, name) {
    const parsed = Number(value)
    if (value == null || text(value) === "" || !Number.isFinite(parsed)) fail(name + " must be a number")
    return parsed
}
function object(json, name) {
    let value
    try {
        value = JSON.parse(text(json) || "{}")
    } catch (error) {
        fail(name + " must be JSON")
    }
    if (!value || typeof value !== "object" || Array.isArray(value)) fail(name + " must be a JSON object")
    return value
}

// ===== the mount =====
function mode() {
    const mount = mountLib.state()
    const fixed = mount.policy.alertAudience
    return {
        mounted: mount.mounted,
        links: mount.links,
        policy: mount.policy,
        // the mount tells every rule set there to an audience, with these fixed parts
        audienceOnly: !!fixed,
        fixed: fixed || {topic: null, emailTemplateId: null, detailLink: null},
        maySet: mount.policy.features.alert === true,
        hidden: mount.mounted ? Array.from(ec.context.get("reportHiddenComponents") || []) : []
    }
}
// Whether the viewer may set a rule here: the mount's policy says so (with no mount, Tools does) and their own grant
// opens the engine's save. The first is what a transition holds (requireSet); the second the engine checks itself.
const canSet = m => m.maySet && offered("save#Definition")
// Whether an audience may be told by email here: with no mount of audiences always (a rule naming no template is sent the
// engine's own, REPORT_ALERT), under one only where it names its template (fixedParts writes it).
const emailOffered = m => !m.audienceOnly || !!m.fixed.emailTemplateId
function requireSet(m = mode()) {
    if (!m.maySet) fail("Setting an alert is not offered to you here")
    return m
}

// ===== rules =====
const owns = definition => definition.ownerId === ec.user.userId && !definition.readOnly
function leafQuery(query) {
    return query && Array.isArray(query.queries) ? leafQuery(query.queries[0]) : query || {}
}
// The engine's answer for one rule: its kind, its owner by name, and the state of each series for the owner or an
// equivalent recipient, or why not.
function stateOf(definitionId) {
    const none = {available: false, business: null, ownerName: null, instances: [], lastEvaluated: null}
    if (!offered("get#AlertState")) return Object.assign(none, {reason: "Its state is not shown to you"})
    const got = ask("get#AlertState", {definitionId})
    if (got.failure) {
        ec.logger.error("Report alerts: state of " + definitionId + ": " + got.failure)
        return Object.assign(none, {reason: "Its state could not be read just now"})
    }
    return got.out
}
// Whether a rule is one this mount shows: not a hidden component's and, where the mount tells every rule to an audience,
// a business rule.
const shown = (m, definition, state) => !m.hidden.includes(definition.component) && (!m.audienceOnly || state?.business === true)
// One rule the viewer reads, among those shown here; with its state where the mount or the caller needs the kind.
function readRule(m, definitionId, withState) {
    const definition = ask("get#Definition", {definitionId: text(definitionId) || "_NA_"}).out?.definition
    if (!definition || definition.type !== "alert") fail(unavailable)
    const state = m.audienceOnly || withState ? stateOf(definition.id) : null
    if (!shown(m, definition, state)) fail(unavailable)
    return {definition, state}
}
function ownRule(m, definitionId, withState) {
    const read = readRule(m, definitionId, withState)
    if (!owns(read.definition)) fail("Only its owner changes an alert; copy it to set your own")
    return read
}
// Whether the viewer reads the dataset a rule's query names now (its artifact, its admission and its documents): the
// engine's own answer, asked once a dataset a call. A query that names none reads what the engine gives it.
function datasetReader() {
    const known = new Map()
    return query => {
        const id = text(leafQuery(query).datasetId)
        if (!id) return true
        if (!known.has(id)) known.set(id, !!ask("get#Dataset", {datasetId: id}).out?.dataset)
        return known.get(id)
    }
}
function conditionText(body) {
    const condition = body.condition || {}
    const kind = condition.type || "above"
    const field = body.field ? body.field + " " : ""
    if (kind === "within" || kind === "outside")
        return field + (kind === "within" ? "inside " : "outside ") + valueText(condition.min) + " to " + valueText(condition.max)
    return field + kind + " " + valueText(condition.threshold ?? 0)
}
// Who a rule tells: an operational rule's contact points by count; a business rule's audience by name for its owner,
// while anyone else learns only that it has one.
function tellsText(definition, business, names) {
    const body = definition.body || {}
    if (!business) {
        const count = Array.from(body.contactPoints || []).length + Array.from(body.contactPointRefs || []).length
        return count ? plural(count, "contact point", "contact points") : "No contact point"
    }
    const audience = body.audience || {}
    if (audience.deliveryMode !== "group") return "Its owner"
    if (!owns(definition)) return "Its audience"
    const users = Array.from(audience.recipientUserIds || [])
    const groups = Array.from(audience.recipientGroupIds || [])
    const parts = []
    if (users.length) parts.push(users.length === 1 ? pick(names.users, users[0], "1 person") : users.length + " people")
    if (groups.length) parts.push(groups.map(groupId => pick(names.groups, groupId, groupId)).join(", "))
    return parts.join(" and ") || "Its owner"
}
// The names of the people and groups the viewer's own rules tell, as find#ReportRecipients answers them under the
// mount's permission; nothing is asked when none names anyone.
function audienceNames(m, definitions) {
    const all = key =>
        Array.from(new Set(definitions.filter(owns).flatMap(definition => Array.from(definition.body?.audience?.[key] || []))))
    const userIds = all("recipientUserIds")
    const groupIds = all("recipientGroupIds")
    if (!userIds.length && !groupIds.length) return {users: {}, groups: {}}
    return namesOf(recipients(m.policy, {userIds, groupIds}))
}
// The rule's state in words: a template, paused, not the viewer's to read, or the most severe state of its series.
function summary(definition, state) {
    const body = definition.body || {}
    if (definition.readOnly) return ["Template", "grey-7"]
    if (body.paused) return [pick(pauseReasons, body.pausedReason, "Paused"), body.pausedReason ? "warning" : "grey-7"]
    if (!state.available) return ["Not shown to you", "grey-7"]
    const found = Array.from(state.instances || []).map(instance => instance.state)
    for (const name of Object.keys(stateWords)) if (found.includes(name)) return stateWords[name]
    if (found.length) return ["Not known", "warning"]
    return ["Not evaluated yet", "grey-7"]
}
function ruleRow(definition, state, names, sets) {
    const body = definition.body || {}
    const words = summary(definition, state)
    const business = state.business === true
    const own = owns(definition)
    return {
        definitionId: definition.id,
        title: definition.title || definition.id,
        own,
        template: !!definition.readOnly,
        business,
        kindText: business ? "Business" : "Operational",
        ownerText: definition.readOnly ? "Shipped" : own ? "You" : state.ownerName || definition.ownerId || "",
        conditionText: conditionText(body),
        audienceText: tellsText(definition, business, names),
        paused: !!body.paused,
        // what the row's actions are offered by: the viewer's own rule, where they may set one
        changeable: own && sets,
        stateText: words[0],
        stateColor: words[1],
        lastText: dateText(state.lastEvaluated),
        version: definition.version
    }
}
function labelsText(instance, body) {
    const labels = Object.entries(instance.labels || {})
        .filter(([name]) => name !== "field" && name !== "rule" && !Object.hasOwn(body?.labels || {}, name))
        .map(([name, value]) => name + " " + value)
    return labels.join(", ") || (instance.labels?.field ? String(instance.labels.field) : "The rule")
}
function stateRow(instance, body) {
    const word = pick(stateWords, instance.state, [text(instance.state), "grey-7"])
    return {
        seriesId: instance.seriesId,
        seriesText: labelsText(instance, body),
        stateText: word[0],
        stateColor: word[1],
        // a state that is not a reading shows no value
        valueText: instance.state === "NoData" || instance.state === "Error" ? "–" : valueText(instance.value),
        sinceText: dateText(instance.stateSince),
        evaluatedText: dateText(instance.lastEvaluated)
    }
}
function deliveryRow(row) {
    const state = pick(deliveryStates, row.statusId, [text(row.statusId), "grey-7"])
    return {
        deliveryId: row.deliveryId,
        definitionId: row.definitionId,
        recipientName: row.userFullName || row.username || "You",
        channelText: pick(channelWords, row.channel, text(row.channel)),
        // one skipped was told nothing, and the engine keeps no state for them
        toldText: text(row.alertState)
            ? text(row.alertState)
                  .split(", ")
                  .map(name => pick(stateWords, name, [name])[0])
                  .join(", ")
            : "Nothing",
        stateText: state[0],
        stateColor: state[1],
        reasonText: row.reason || "",
        attemptText: dateText(row.attemptDate)
    }
}

// ===== Alerts: the list =====
// The shared contact points, silences and mute timings the viewer reads, each with what it is in a line.
function sharedItems() {
    const of = type => Array.from(ask("list#Definitions", {definitionType: type}).out?.definitions || [])
    const row = (item, detailText) => ({
        id: item.id,
        title: item.title || item.id,
        type: item.type,
        detail: detailText,
        ownerId: item.ownerId || "",
        deletable: owns(item)
    })
    return {
        contacts: of("contactPoint").map(item => {
            const body = item.body || {}
            return row(
                item,
                [body.type, body.topic || body.toAddresses || body.urlProperty || body.url].filter(Boolean).join(": ")
            )
        }),
        silences: of("silence").map(item => row(item, (item.body?.from || "") + " to " + (item.body?.thru || ""))),
        mutes: of("muteTiming").map(item => row(item, (item.body?.start || "00:00") + " to " + (item.body?.end || "24:00")))
    }
}
// All (every rule the viewer reads, where the screen is Tools' own or a mount keeps contact points), Mine (the rules
// they own), Received (the rules that told them, with their own delivery) and Templates (the shipped rules to copy,
// each over a dataset the viewer reads or not); with no mount also the state of each series and the shared contact
// points, silences and mute timings. A viewer who may set no rule under a mount of audiences has Received alone.
function list(p) {
    const m = mode()
    const sets = canSet(m)
    const names = {all: "All", mine: "Mine", received: "Received", templates: "Templates"}
    const views = m.audienceOnly ? (sets ? ["mine", "received", "templates"] : ["received"]) : Object.keys(names)
    const view = views.includes(p.view) ? p.view : views[0]
    const out = {
        mounted: m.mounted,
        operational: !m.audienceOnly,
        view,
        viewOptions: views.map(value => ({value, label: names[value]})),
        stateOptions: [
            {value: "active", label: "Active"},
            {value: "paused", label: "Paused"}
        ],
        // whether the viewer may set a rule here (not named canSet: the screen lint reads a name that ends in Set as a
        // collection)
        settable: sets,
        canEvaluate: !m.audienceOnly && offered("evaluate#Alerts"),
        ruleUrl: m.links.alertRule,
        sharedUrl: m.audienceOnly ? null : m.links.alertShared,
        alerts: [],
        received: [],
        templates: [],
        instances: [],
        shared: {contacts: [], silences: [], mutes: []},
        failure: null
    }
    if (out.sharedUrl) out.shared = sharedItems()
    const found = offered("list#Definitions") ? ask("list#Definitions", {definitionType: "alert"}) : {out: {}, failure: null}
    if (found.failure) return Object.assign(out, {failure: found.failure})
    const all = Array.from(found.out.definitions || [])
    if (view === "received") {
        if (!offered("find#AlertDeliveries")) return out
        const told = ask("find#AlertDeliveries", {received: true, pageSize: 100})
        if (told.failure) return Object.assign(out, {failure: told.failure})
        const seen = new Set()
        for (const row of Array.from(told.out.deliveries || [])) {
            if (seen.has(row.definitionId) || seen.size >= receivedRules) continue
            seen.add(row.definitionId)
            const definition = all.find(candidate => candidate.id === row.definitionId)
            const words = definition ? summary(definition, stateOf(definition.id)) : ["Not shown to you", "grey-7"]
            out.received.push(
                Object.assign(deliveryRow(row), {
                    title: definition?.title || row.title || "An alert you no longer read",
                    readable: !!definition,
                    ruleStateText: words[0],
                    ruleStateColor: words[1]
                })
            )
        }
        return out
    }
    const wanted = all.filter(definition =>
        view === "templates" ? definition.readOnly : view === "mine" ? owns(definition) : true
    )
    const states = {}
    for (const definition of wanted) states[definition.id] = stateOf(definition.id)
    const rules = wanted.filter(definition => shown(m, definition, states[definition.id]))
    if (view === "templates") {
        const reads = datasetReader()
        out.templates = rules.map(definition => {
            const available = reads(definition.body?.query)
            return {
                definitionId: definition.id,
                title: definition.title || definition.id,
                description: definition.body?.description || "",
                kindText: states[definition.id].business === true ? "Business" : "Operational",
                conditionText: conditionText(definition.body || {}),
                available: available && sets,
                availableText: available ? "" : "Its dataset is not yours to read"
            }
        })
        return out
    }
    const audienceOf = audienceNames(m, rules)
    out.alerts = rules.map(definition => ruleRow(definition, states[definition.id], audienceOf, sets))
    if (p.state === "active" || p.state === "paused") out.alerts = out.alerts.filter(row => row.paused === (p.state === "paused"))
    if (!m.audienceOnly)
        for (const definition of rules)
            for (const instance of Array.from(states[definition.id].instances || []))
                out.instances.push(
                    Object.assign(stateRow(instance, definition.body), {
                        definitionId: definition.id,
                        title: definition.title || definition.id
                    })
                )
    return out
}

// ===== AlertRule: one rule =====
// The queries of a dashboard's panel that have a measure, for a rule set from a panel's menu: each the saved query.
function panelSource(dashboardId, panelId) {
    const dashboard = ask("get#Definition", {definitionId: text(dashboardId) || "_NA_"}).out?.definition
    let panel = null
    if (dashboard) {
        const placed = dashboard.type === "panel" ? dashboard.body : pick(dashboard.body?.panels, text(panelId), null)
        if (placed?.panelRef) {
            const library = ask("get#Definition", {definitionId: placed.panelRef}).out?.definition
            panel = library ? Object.assign({}, library.body, {title: placed.title || library.title}) : null
        } else panel = placed
    }
    if (!panel) fail("This panel is not available")
    const queries = Array.from(panel.queries || [])
        .map((query, index) => ({query, index}))
        .filter(entry => Array.from(entry.query.aggregations || []).some(aggregation => aggregation.role === "metric"))
    return {panel, dashboardTitle: dashboard.title, queries}
}
const measures = query =>
    Array.from(query?.aggregations || [])
        .filter(aggregation => aggregation.role === "metric")
        .map(aggregation => ({value: aggregation.id, label: aggregation.id}))
// The numbers of a rule's basis as a form shows them: canonical text, so a text-line shows neither grouping nor a
// rounded value and the save reads back what was stored.
function basisValues(body) {
    const condition = body.condition || {}
    return {
        threshold: String(condition.threshold ?? 0),
        minimum: String(condition.min ?? 0),
        maximum: String(condition.max ?? 0),
        recoveryThreshold: String(condition.recoveryThreshold ?? ""),
        pendingSeconds: String(body.pendingSeconds || 0),
        keepFiringSeconds: String(body.keepFiringSeconds || 0),
        intervalSeconds: String(body.intervalSeconds || 60)
    }
}
// The form values every rule has, and the ones of a rule whose query is typed (labels, the query itself).
function basisOf(definition, creating) {
    const body = definition.body || {}
    const query = leafQuery(body.query)
    return Object.assign(basisValues(body), {
        definitionId: creating ? "" : definition.id,
        title: definition.title || "",
        valueField: body.field || "",
        reduce: body.reduce || "last",
        conditionType: body.condition?.type || "above",
        window: windows.some(option => option.value === query.from) ? query.from : "",
        windowText: (query.from || "") + " to " + (query.thru || "now"),
        paused: body.paused ? "Y" : "N",
        queryJson: JSON.stringify(body.query || {}, null, 2),
        labelFields: Array.from(body.labelFields || []).join(", "),
        labelsJson: JSON.stringify(body.labels || {}, null, 2),
        expectedVersion: definition.version ?? ""
    })
}
const emptyOptions = () => ({conditions, reductions, windows, intervals, groups: [], measures: [], queries: [], datasets: []})
const notFound = () => ({
    found: false,
    creating: false,
    audienceOnly: false,
    operational: false,
    business: false,
    canEdit: false,
    canCopy: false,
    canDistribute: false,
    canEvaluate: false,
    alert: {},
    options: emptyOptions(),
    audience: [],
    audienceNote: "",
    states: [],
    stateNote: "",
    deliveries: [],
    contacts: [],
    silences: [],
    mutes: [],
    sharedChoices: [],
    refs: [],
    listUrl: null,
    sharedUrl: null
})
// A rule not saved yet. Under a mount of audiences: over the saved query of a panel the viewer reads, saved inactive.
// Otherwise as Tools starts one: a query to type or build, or a panel's queries.
function draft(m, p, out) {
    const options = out.options
    if (m.audienceOnly) {
        const source = panelSource(p.dashboardId, p.panelId)
        if (!source.queries.length) fail("This panel has no measure an alert could watch")
        const chosen = source.queries.find(entry => String(entry.index) === text(p.queryIndex)) || source.queries[0]
        options.measures = measures(chosen.query)
        options.queries = source.queries.map(entry => ({
            value: String(entry.index),
            label: "Query " + (entry.index + 1) + " (" + (entry.query.datasetId || "") + ")"
        }))
        return Object.assign(basisOf({title: (source.panel.title || "Panel") + " alert", body: {intervalSeconds: 3600}}, true), {
            sourceText: source.dashboardTitle + " › " + (source.panel.title || text(p.panelId)),
            queryIndex: String(chosen.index),
            valueField: options.measures[0]?.value || "",
            window: "now-1d",
            audienceKind: "personal",
            emailChannel: "N"
        })
    }
    const definition = {title: "", body: {query: clone(blankQuery), condition: {type: "above", threshold: 0}}}
    if (text(p.dashboardId) && text(p.panelId)) {
        const source = panelSource(p.dashboardId, p.panelId)
        definition.title = source.panel.title || "Panel alert"
        definition.body.query = {
            queries: source.panel.queries || [],
            transformations: source.panel.transformations || [],
            from: "now-5m",
            thru: "now"
        }
    }
    return Object.assign(basisOf(definition, true), {sourceText: "A new rule", ruleId: ""})
}
// One rule, for anyone who reads it (another's unshared rule, one this mount does not show and none are not found): its
// reading and condition, whom it tells, for its owner each recipient's result of the engine's check, the state of each
// series as the viewer may read it and the deliveries (each recipient's for the owner, the viewer's own otherwise). With
// no rule named: the draft of a new one.
function detail(p) {
    const m = mode()
    const sets = canSet(m)
    const out = Object.assign(notFound(), {
        audienceOnly: m.audienceOnly,
        operational: !m.audienceOnly,
        listUrl: m.links.alerts,
        sharedUrl: m.audienceOnly ? null : m.links.alertShared
    })
    if (!text(p.definitionId)) {
        if (!sets) return out
        let values
        try {
            values = draft(m, p, out)
        } catch (error) {
            if (!error?.refusal) throw error
            return out
        }
        if (!m.audienceOnly) out.options.datasets = Array.from(ask("list#Datasets", {}).out?.datasets || [])
        return Object.assign(out, {
            found: true,
            creating: true,
            business: m.audienceOnly,
            canEdit: true,
            alert: Object.assign(values, {
                dashboardId: text(p.dashboardId),
                panelId: text(p.panelId),
                paused: m.audienceOnly ? "Y" : "N",
                stateText: "New",
                stateColor: "grey-7"
            })
        })
    }
    let read
    try {
        read = readRule(m, p.definitionId, true)
    } catch (error) {
        if (!error?.refusal) throw error
        return out
    }
    const {definition, state} = read
    const body = definition.body || {}
    const own = owns(definition)
    const business = state.business === true
    const audience = body.audience || {}
    const named = {
        userIds: own ? Array.from(audience.recipientUserIds || []) : [],
        groupIds: own ? Array.from(audience.recipientGroupIds || []) : []
    }
    // who the owner may name here, and the names of those the rule names
    const proposed = own && business && sets ? recipients(m.policy, named) : null
    const names = proposed ? namesOf(proposed) : {users: {}, groups: {}}
    const canEdit = own && sets
    const query = leafQuery(body.query)
    const options = out.options
    options.measures = measures(query)
    options.groups = proposed ? groupOptions(proposed) : []
    if (canEdit && !m.audienceOnly) options.datasets = Array.from(ask("list#Datasets", {}).out?.datasets || [])
    const email = Array.from(audience.channels || []).includes("email")
    const alert = Object.assign(ruleRow(definition, state, names, sets), basisOf(definition, false), {
        description: body.description || "",
        sourceText: (query.datasetId || "") + (body.panelId ? " · from the panel " + body.panelId : ""),
        ownerName: definition.readOnly || own ? "" : state.ownerName || "",
        dashboardId: body.dashboardId || "",
        panelId: body.panelId || "",
        audienceKind: audience.deliveryMode === "group" ? "group" : "personal",
        recipientUserIds: named.userIds,
        recipientGroupIds: named.groupIds,
        selectedRecipients: named.userIds.join(","),
        emailChannel: email ? "Y" : "N",
        // email is offered with no mount of audiences (the rule's own template, else the engine's REPORT_ALERT), and
        // under one where it names a template
        emailOffered: emailOffered(m),
        userGroupId: definition.userGroupId || "",
        pausedReason: body.pausedReason || ""
    })
    // each recipient's result, for the owner who may tell others: eligibility in words, never the other person's grants
    let audienceRows = []
    let audienceNote = ""
    if (own && audience.deliveryMode === "group" && proposed?.canDeliver && offered("check#ReportAudience")) {
        const checked = ask("check#ReportAudience", {
            definitionId: definition.id,
            recipientUserIds: named.userIds,
            recipientGroupIds: named.groupIds,
            recipientCap: audience.recipientCap || m.policy.recipientCap || null
        })
        if (checked.failure) {
            ec.logger.error("Report alerts: audience of " + definition.id + ": " + checked.failure)
            audienceNote = "The recipients could not be checked just now."
        } else {
            audienceRows = Array.from(checked.out.recipients || []).map(recipient => {
                // the notification in the app needs no address
                const eligible = recipient.eligible === true || (recipient.reasonCode === "no-address" && !email)
                return {
                    userId: recipient.userId,
                    recipientName: recipient.userFullName || recipient.username || recipient.userId,
                    viaText: Array.from(recipient.viaGroupIds || []).length ? "In a group" : "Named",
                    eligible,
                    resultText: eligible ? "May be told" : recipient.reason,
                    resultColor: eligible ? "positive" : "negative",
                    reasonCode: recipient.reasonCode || ""
                }
            })
            audienceNote =
                audienceRows.filter(row => row.eligible).length +
                " of " +
                checked.out.recipientCount +
                " may be told; at most " +
                checked.out.recipientCap +
                " recipients."
        }
    }
    const states = Array.from(state.instances || []).map(instance => stateRow(instance, body))
    const told =
        business && offered("find#AlertDeliveries")
            ? ask("find#AlertDeliveries", own ? {definitionId: definition.id} : {definitionId: definition.id, received: true})
            : {out: {}}
    if (told.failure) ec.logger.error("Report alerts: deliveries of " + definition.id + ": " + told.failure)
    Object.assign(out, {
        found: true,
        business,
        canEdit,
        canCopy: sets && !own && (m.audienceOnly || offered("copy#Definition")),
        canDistribute: !!proposed?.canDeliver,
        canEvaluate: !m.audienceOnly && offered("evaluate#Alerts"),
        alert,
        audience: audienceRows,
        audienceNote,
        states,
        stateNote: definition.readOnly
            ? "A shipped alert is a template: copy it to set your own."
            : state.available
              ? states.length
                  ? ""
                  : "Not evaluated yet."
              : state.reason || "Its state is not shown to you.",
        deliveries: Array.from(told.out?.deliveries || []).map(deliveryRow)
    })
    if (m.audienceOnly) return out
    // what an operational rule tells and what quiets any rule here: its own contact points, silences and mute timings,
    // and the shared ones it names
    const numbered = key => Array.from(body[key] || []).map((item, index) => Object.assign({itemIndex: index}, item))
    const shared = Object.keys(sharedKinds).flatMap(type =>
        Array.from(ask("list#Definitions", {definitionType: type}).out?.definitions || [])
    )
    return Object.assign(out, {
        contacts: numbered("contactPoints"),
        silences: numbered("silences"),
        mutes: numbered("muteTimings").map(item =>
            Object.assign(item, {
                weekdayNames: Array.from(item.weekdays || [])
                    .map(day => ec.l10n.localize(weekdayNames[Number(day)] || String(day)))
                    .join(", ")
            })
        ),
        sharedChoices: shared
            .filter(item => business === false || item.type !== "contactPoint")
            .map(item => ({id: item.id, label: sharedKinds[item.type] + ": " + (item.title || item.id)})),
        refs: Object.entries(sharedRefKeys).flatMap(([type, refKey]) =>
            Array.from(body[refKey] || []).map(refId => {
                const item = shared.find(candidate => candidate.id === refId)
                return {refKey, refId, kind: sharedKinds[type], title: item ? item.title || refId : refId + " (not available)"}
            })
        )
    })
}

// ===== the transitions that set a rule =====
function newId(title) {
    const slug = title
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-+|-+$/g, "")
        .slice(0, 40)
    const user = text(ec.user.userId)
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "")
    return "alert-" + (slug || "rule") + "-" + user + "-" + Date.now().toString(36)
}
const versionOf = (given, stored) => (given == null || given === "" ? stored : Number(given))
// What every save of a rule carries under a mount of audiences, beside the form's values: the audience's notification
// topic, its email template where email is a channel, the recipient cap and the link a notification opens, each from
// the mount's policy and never from the request; and no contact point. Under any mount the cap is the policy's. With no
// mount the rule keeps what it has.
function fixedParts(m, body) {
    const cap = m.policy.recipientCap
    if (!m.audienceOnly) {
        if (cap && body.audience) body.audience = Object.assign({}, body.audience, {recipientCap: cap})
        return body
    }
    const audience = Object.assign(
        {recipientUserIds: [], recipientGroupIds: [], channels: ["notification"], groupWaitSeconds: 0},
        body.audience
    )
    delete audience.deliveryMode
    if (m.fixed.topic) audience.topic = m.fixed.topic
    const email = Array.from(audience.channels || []).includes("email") && !!m.fixed.emailTemplateId
    audience.channels = email ? ["notification", "email"] : ["notification"]
    if (email) audience.emailTemplateId = m.fixed.emailTemplateId
    else delete audience.emailTemplateId
    if (cap) audience.recipientCap = cap
    body.audience = audience
    if (m.fixed.detailLink) body.detailLink = m.fixed.detailLink
    delete body.contactPoints
    delete body.contactPointRefs
    return body
}
// The rule's reading run now as the viewer, never sent: whether it can be evaluated, and how many series it has.
function reading(body) {
    const result = ask("run#Query", {query: body.query})
    if (result.failure) {
        ec.logger.error("Report alerts: reading: " + result.failure)
        return {ok: false, reason: "You may not read its source, or it did not answer"}
    }
    if (Array.from(result.out.notices || []).some(notice => notice.complete === false))
        return {ok: false, reason: "Its source did not answer completely just now"}
    const rows = Array.from(result.out.frames || []).reduce(
        (count, frame) => Math.max(count, Array.from(frame.fields?.[0]?.values || []).length),
        0
    )
    return {ok: true, seriesCount: rows}
}
// The form's condition, reduction, measure and timing into a rule's body.
function basis(m, p, body) {
    const kind = conditions.some(option => option.value === p.conditionType) ? p.conditionType : "above"
    const condition = {type: kind}
    if (kind === "within" || kind === "outside") {
        condition.min = number(p.minimum, "The range's lowest value")
        condition.max = number(p.maximum, "The range's highest value")
        if (condition.min > condition.max) fail("The range's highest value must be at least its lowest")
    } else {
        condition.threshold = number(p.threshold, "The threshold")
        if (text(p.recoveryThreshold)) condition.recoveryThreshold = number(p.recoveryThreshold, "The recovery threshold")
    }
    body.condition = condition
    body.reduce = reductions.some(option => option.value === p.reduce) ? p.reduce : "last"
    const field = text(p.valueField)
    if (m.audienceOnly && field && !measures(leafQuery(body.query)).some(option => option.value === field))
        fail("Choose one of the query's measures")
    body.field = field || null
    if (m.audienceOnly) {
        // one of the intervals offered, else the one the rule has
        const interval = intervals.find(option => option.value === text(p.intervalSeconds))
        body.intervalSeconds = Number(interval?.value || plain(body.intervalSeconds) || 3600)
    } else body.intervalSeconds = Math.max(60, Math.round(number(text(p.intervalSeconds) || 60, "The interval")))
    for (const name of ["pendingSeconds", "keepFiringSeconds"]) {
        const seconds = text(p[name]) ? number(p[name], "A duration") : 0
        if (seconds < 0 || seconds > 604800) fail("A duration is 0 to 604800 seconds")
        body[name] = Math.round(seconds)
    }
    if (m.audienceOnly) {
        // the range read at each evaluation; a compound query keeps its own
        const window = windows.find(option => option.value === text(p.window))
        if (window && body.query && !Array.isArray(body.query.queries))
            Object.assign(body.query, {from: window.value, thru: "now"})
    }
    return body
}
// What only a rule whose query is typed has in its form: the query, whether it is evaluated, its series labels and
// its routing labels, and the panel it came from.
function typedParts(p, body) {
    body.query = object(p.queryJson, "The query")
    body.paused = p.paused === "Y"
    if (!body.paused) delete body.pausedReason
    const labelFields = idList(p.labelFields)
    if (labelFields.length) body.labelFields = labelFields
    else delete body.labelFields
    body.labels = object(p.labelsJson, "The routing labels")
    if (text(p.dashboardId)) body.dashboardId = text(p.dashboardId)
    if (text(p.panelId)) body.panelId = text(p.panelId)
    return body
}
// A new rule under a mount of audiences: over the saved query of a panel the viewer reads, for the viewer alone and
// inactive until they activate it.
function panelRule(p, title) {
    const source = panelSource(p.dashboardId, p.panelId)
    if (!source.queries.length) fail("This panel has no measure an alert could watch")
    const chosen = source.queries.find(entry => String(entry.index) === text(p.queryIndex)) || source.queries[0]
    const query = clone(chosen.query)
    delete query.ignoreVariables
    Object.assign(query, {from: source.panel.time?.from || "now-1d", thru: "now"})
    const body = {
        query,
        dashboardId: text(p.dashboardId),
        panelId: text(p.panelId),
        paused: true,
        pausedReason: "not-validated"
    }
    if (source.panel.business) body.business = clone(source.panel.business)
    return {id: newId(title), type: "alert", title, userGroupId: null, body}
}
// storeAlert: a rule of the viewer's own, new or changed in its reading, condition and timing; whom it tells is kept
// (storeAudience changes it). Answers the rule's id, or null with the refusal.
function store(p) {
    return attempt(() => {
        const m = requireSet()
        const creating = !text(p.definitionId)
        const title = text(p.title)
        if (!title || title.length > 120) fail("Name the alert in at most 120 characters")
        let definition
        if (!creating) {
            const existing = ownRule(m, p.definitionId).definition
            definition = {
                id: existing.id,
                type: "alert",
                title,
                userGroupId: existing.userGroupId || null,
                body: clone(existing.body),
                version: versionOf(p.expectedVersion, existing.version)
            }
        } else if (m.audienceOnly) definition = panelRule(p, title)
        else definition = {id: text(p.ruleId) || newId(title), type: "alert", title, userGroupId: null, body: {}}
        if (!m.audienceOnly) typedParts(p, definition.body)
        fixedParts(m, basis(m, p, definition.body))
        const saved = ask("save#Definition", {definition, note: text(p.note) || (creating ? "Set" : "Changed")})
        if (saved.failure) fail(saved.failure)
        done(saved.out.definition?.body?.paused ? "Alert saved. It is inactive until you activate it" : "Alert saved")
        return saved.out.definition.id
    })
}
// Why the people and groups an audience names, and the group a rule is shared with, may not be so here, or null when
// they may: the viewer may tell others, each group is one of theirs and each person is among those
// find#ReportRecipients accepts under the mount's permission. It says nothing of an account it refuses. An audience of
// oneself alone, shared with nobody, asks nothing.
function unaccepted(policy, users, groups, shareGroupId) {
    const others = users.filter(userId => userId !== ec.user.userId)
    if (!groups.length && !others.length && !shareGroupId) return null
    const found = recipients(policy, {userIds: users, groupIds: shareGroupId ? groups.concat([shareGroupId]) : groups})
    if (found.failure) return "The recipients could not be checked just now"
    if (!found.canDeliver)
        return groups.length || others.length
            ? "Telling other people of an alert needs the group delivery grant"
            : "Sharing an alert with a group needs the group delivery grant"
    if (shareGroupId && found.refusedGroupIds.includes(shareGroupId)) return "Share only with a group you belong to"
    if (found.refusedGroupIds.length) return "Tell only a group you belong to"
    if (found.refusedUserIds.length) return "Choose recipients among the people who may be told here"
    return null
}
// storeAudience: whom a business rule of the viewer's tells (themselves alone, or people and groups, each accepted
// first), how (in the app, and at each recipient's own account address where email is offered: the mount's template, else
// the rule's own, else the engine's), and the group of the viewer's own it is shared with, so its recipients read it. The engine then checks every recipient of an active rule.
function storeAudience(p) {
    return attempt(() => {
        const m = requireSet()
        const {definition: existing, state} = ownRule(m, p.definitionId, true)
        if (state.business !== true) fail("This rule tells its contact points: it has no audience")
        const others = p.audienceKind === "group"
        const users = others ? idList(p.recipientUserIds) : []
        const groups = others ? idList(p.recipientGroupIds) : []
        const shareGroupId = text(p.userGroupId) || null
        const refusal = unaccepted(m.policy, users, groups, shareGroupId)
        if (refusal) fail(refusal)
        const body = clone(existing.body)
        const before = body.audience || {}
        const template = m.fixed.emailTemplateId || before.emailTemplateId || null
        const email = flag(p.emailChannel) && emailOffered(m)
        // each series is its own notification, so nothing is gained by waiting to group; a state still firing is told
        // again after the engine's repeat interval
        const audience = {
            recipientUserIds: users,
            recipientGroupIds: groups,
            channels: email ? ["notification", "email"] : ["notification"],
            groupWaitSeconds: before.groupWaitSeconds ?? 0
        }
        for (const name of ["topic", "recipientCap", "repeatSeconds", "groupBy"])
            if (before[name] != null) audience[name] = before[name]
        if (email && template) audience.emailTemplateId = template
        body.audience = audience
        const saved = ask("save#Definition", {
            definition: {
                id: existing.id,
                type: "alert",
                title: existing.title,
                userGroupId: shareGroupId,
                body: fixedParts(m, body),
                version: versionOf(p.expectedVersion, existing.version)
            },
            note: "Changed whom it tells"
        })
        if (saved.failure) fail(saved.failure)
        return done(
            saved.out.definition?.body?.audience?.deliveryMode === "group"
                ? "Audience saved: each recipient is checked before every notification"
                : "The alert now tells you alone"
        )
    })
}
// pauseAlerts: one rule or several of the viewer's own paused or activated, each with its own result. An activation
// first runs the rule's reading as the viewer, then saves it active, which is the engine's check of the owner's sources
// and of every recipient; one it refuses says why and the others go on.
function pause(p) {
    return attempt(() => {
        const m = requireSet()
        const paused = !(p.paused === false || p.paused === "false" || p.paused === "N")
        const asked = idList(p.definitionIds)
        let applied = 0
        for (const definitionId of asked) {
            let failure = null
            try {
                const existing = ownRule(m, definitionId).definition
                const body = clone(existing.body)
                if (!paused) {
                    const read = reading(body)
                    if (!read.ok) fail(read.reason)
                }
                body.paused = paused
                delete body.pausedReason
                failure = ask("save#Definition", {
                    definition: {
                        id: existing.id,
                        type: "alert",
                        title: existing.title,
                        userGroupId: existing.userGroupId || null,
                        body: fixedParts(m, body),
                        version: existing.version
                    },
                    note: paused ? "Paused" : "Activated"
                }).failure
            } catch (error) {
                if (!error?.refusal) throw error
                failure = error.message
            }
            // one rule's refusal is the transition's; of several, each is told and the others go on
            if (failure && asked.length === 1) fail(failure)
            if (failure) ec.message.addMessage(failure, "danger")
            else applied++
        }
        if (applied) done(plural(applied, "alert", "alerts") + (paused ? " paused" : " activated"))
        return applied
    })
}
// testAlert: the rule's reading as the viewer now; it changes no state and tells nobody.
function test(p) {
    return attempt(() => {
        const m = requireSet()
        const read = reading(readRule(m, p.definitionId).definition.body || {})
        if (!read.ok) return ec.message.addMessage(read.reason, "warning")
        return done("The reading ran as you: " + read.seriesCount + " series. Nothing was sent.")
    })
}
// copyAlert: the viewer's own copy of a shipped rule, or of one shared with them, over a dataset they read. Under a
// mount of audiences it tells the viewer alone, has the mount's fixed parts and no contact point, and waits to be
// activated; otherwise it is the engine's copy (copy#Definition, which holds a business rule inactive the same way).
// Answers the copy's id, or null with the refusal.
function copy(p) {
    return attempt(() => {
        const m = requireSet()
        const source = readRule(m, p.definitionId).definition
        if (!datasetReader()(source.body?.query)) fail("Its dataset is not yours to read")
        const title = text(p.title) || source.title
        if (title.length > 120) fail("Name the alert in at most 120 characters")
        let copied
        if (m.audienceOnly) {
            const body = clone(source.body)
            Object.assign(body, {
                paused: true,
                pausedReason: "not-validated",
                copiedFrom: source.id,
                copiedVersion: source.version ?? 0,
                audience: {recipientUserIds: [], recipientGroupIds: [], channels: ["notification"], groupWaitSeconds: 0}
            })
            copied = ask("save#Definition", {
                definition: {id: newId(title), type: "alert", title, userGroupId: null, body: fixedParts(m, body)},
                note: "Copied from " + source.id
            })
        } else copied = ask("copy#Definition", {definitionId: source.id, newDefinitionId: newId(title), title})
        if (copied.failure) fail(copied.failure)
        done(
            copied.out.definition?.body?.paused
                ? "Copied: set its threshold and whom it tells, then activate it"
                : "Copied: it is yours to change"
        )
        return copied.out.definition.id
    })
}
// deleteAlert: a rule of the viewer's own, with its state, its deliveries and its versions. True when it is gone.
function remove(p) {
    return attempt(() => {
        const m = requireSet()
        const existing = ownRule(m, p.definitionId).definition
        const deleted = ask("delete#Definition", {definitionId: existing.id})
        if (deleted.failure) fail(deleted.failure)
        done("Alert deleted")
        return true
    })
}
// recipientChoices: the people an audience's drop-down offers, as the schedule screens' do, under the mount's permission.
const recipientChoices = p => delivery.recipientChoices(p)

// ===== the transitions of an operational rule =====
// What an evaluation request answered, in words: the service's count of evaluated rules includes those skipped inside
// their interval or already being evaluated, so it is not a count of new results.
function evaluationMessages(evaluation) {
    if (!evaluation) return []
    const messages = []
    if (Number(evaluation.failed || 0) > 0)
        messages.push({
            text:
                "Evaluation request reported " +
                evaluation.failed +
                " failed rule(s). Review the instance state and server logs.",
            type: "warning"
        })
    else if (Number(evaluation.evaluated || 0) === 0)
        messages.push({
            text: "No active alert rule was processed. If the rule is paused, resume it before evaluating.",
            type: "info"
        })
    else
        messages.push({
            text: "Evaluation requested. Rules within their evaluation interval or already being evaluated are skipped. Check the Evaluated time and state below.",
            type: "info"
        })
    for (const notice of evaluation.notices || [])
        if (notice && notice.message) messages.push({text: String(notice.message), type: "warning"})
    return messages
}
// evaluate: one rule evaluated now, where the viewer's grant opens the engine's evaluation (an administrator's).
function evaluate(p) {
    return attempt(() => {
        requireSet()
        const asked = ask("evaluate#Alerts", {definitionId: p.definitionId})
        if (asked.failure) fail(asked.failure)
        for (const message of evaluationMessages(asked.out)) ec.message.addMessage(message.text, message.type)
        return true
    })
}
// A change of a rule of the viewer's own that has contact points, silences and mute timings: read, changed and saved at
// the version read. A mount of audiences has none of these.
function change(p, note, apply) {
    return attempt(() => {
        const m = requireSet()
        if (m.audienceOnly) fail("An alert here tells an audience: it takes no contact point, silence or mute timing")
        const definition = ownRule(m, p.definitionId).definition
        const body = clone(definition.body)
        const said = apply(body) || note
        const saved = ask("save#Definition", {
            definition: {
                id: definition.id,
                type: "alert",
                title: definition.title,
                userGroupId: definition.userGroupId || null,
                body,
                version: definition.version
            },
            note: said
        })
        if (saved.failure) fail(saved.failure)
        return true
    })
}
const isoTime = /(Z|[+-]\d\d:\d\d)$/
function addContact(p) {
    return change(p, "Added contact point", body => {
        const contact = {
            id: text(p.contactId) || "contact-" + Date.now(),
            type: text(p.contactType) || "notification",
            topic: text(p.topic) || "ReportAlert",
            groupBy: idList(p.groupBy),
            groupWaitSeconds: Math.max(0, number(text(p.groupWaitSeconds) || 30, "The group wait")),
            repeatSeconds: Math.max(60, number(text(p.repeatSeconds) || 14400, "The repeat interval")),
            matchers: object(p.matchersJson, "The label matchers")
        }
        for (const [name, value] of [
            ["userId", p.contactUserId],
            ["userGroupId", p.contactGroupId],
            ["emailTemplateId", p.emailTemplateId],
            ["toAddresses", p.toAddresses],
            ["url", p.webhookUrl]
        ])
            if (text(value)) contact[name] = text(value)
        body.contactPoints = Array.from(body.contactPoints || []).concat([contact])
    })
}
function addSilence(p) {
    return change(p, "Added silence", body => {
        if (!isoTime.test(text(p.silenceFrom)) || !isoTime.test(text(p.silenceThru)))
            fail("Use ISO times including Z or an explicit UTC offset")
        const from = new Date(text(p.silenceFrom))
        const thru = new Date(text(p.silenceThru))
        if (!Number.isFinite(from.getTime()) || !(thru.getTime() > from.getTime())) fail("Silence must end after it starts")
        body.silences = Array.from(body.silences || []).concat([
            {
                from: from.toISOString(),
                thru: thru.toISOString(),
                comment: text(p.silenceComment),
                matchers: object(p.silenceMatchers, "The label matchers")
            }
        ])
    })
}
function addMute(p) {
    return change(p, "Added mute timing", body => {
        if (!/^([01]\d|2[0-3]):[0-5]\d$/.test(text(p.muteStart)) || !/^(([01]\d|2[0-3]):[0-5]\d|24:00)$/.test(text(p.muteEnd)))
            fail("Use HH:mm times")
        const days = idList(p.weekdays).map(Number)
        if (!days.length || days.some(day => !Number.isInteger(day) || day < 0 || day > 6)) fail("Choose weekdays")
        body.muteTimings = Array.from(body.muteTimings || []).concat([
            {name: text(p.muteName), weekdays: days, start: text(p.muteStart), end: text(p.muteEnd)}
        ])
    })
}
function removeItem(p) {
    return change(p, "Removed an item", body => {
        if (!["contactPoints", "silences", "muteTimings"].includes(p.collection)) fail("Unknown rule collection")
        const items = Array.from(body[p.collection] || [])
        const index = Number(p.itemIndex)
        if (!Number.isInteger(index) || index < 0 || index >= items.length) fail("This item no longer exists")
        items.splice(index, 1)
        body[p.collection] = items
        return "Removed " + p.collection + " item"
    })
}
function addRef(p) {
    return change(p, "Added a shared item", body => {
        const shared = ask("get#Definition", {definitionId: text(p.sharedRef) || "_NA_"}).out?.definition
        const key = pick(sharedRefKeys, shared?.type, null)
        if (!key) fail("Choose a shared contact point, silence or mute timing")
        const refs = Array.from(body[key] || [])
        if (!refs.includes(shared.id)) body[key] = refs.concat([shared.id])
        return "Added shared " + shared.type + " " + shared.id
    })
}
function removeRef(p) {
    return change(p, "Removed a shared item", body => {
        if (!Object.values(sharedRefKeys).includes(p.refKey)) fail("Unknown shared reference list")
        body[p.refKey] = Array.from(body[p.refKey] || []).filter(id => id !== p.refId)
        return "Removed shared reference " + text(p.refId)
    })
}
// deleteShared: a shared contact point, silence or mute timing of the viewer's own, from the list.
function removeShared(p) {
    return attempt(() => {
        requireSet()
        const item = ask("get#Definition", {definitionId: text(p.definitionId) || "_NA_"}).out?.definition
        if (!item || !Object.hasOwn(sharedKinds, item.type)) fail("This shared item is not available")
        const deleted = ask("delete#Definition", {definitionId: item.id})
        if (deleted.failure) fail(deleted.failure)
        done("Shared item deleted")
        return true
    })
}

;({
    list,
    detail,
    basisValues,
    evaluationMessages,
    requireSet,
    recipientChoices,
    store,
    storeAudience,
    pause,
    test,
    copy,
    remove,
    evaluate,
    addContact,
    addSilence,
    addMute,
    removeItem,
    addRef,
    removeRef,
    removeShared
})
