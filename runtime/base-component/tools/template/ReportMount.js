// The mount of the report screens (docs/tools/Monitoring.md, "Mounting the report screens"; plans/Admin-Screens-Down.md,
// Track I). The Dashboards screens are also shown by another app under its own root (IronStone Insights): mounted by
// location, or included in a screen of the app's own with the transitions it takes in. Such a root sets two maps in its
// always-actions, so that its transitions read them too; Tools' roots (Dashboards, System's Monitor) set both to null, so
// a request parameter never supplies one there. The maps are data only, never authority: a screen shown under a mount
// still asks its service, and the service decides.
//
//     reportLinks   {key: screen URL}: where each report link goes under the mount. A key the map leaves out or sets to
//                   null is not offered there. With no mount the links are Tools' own (toolsLinks below).
//     reportPolicy  what the mounting app's policy is for the report screens it shows (toolsPolicy below has each field
//                   with what Tools does itself). A field the map leaves out has its default.
//
// state() answers {mounted, links, policy} for a screen's actions, every key and every field present. url(key, own) is a
// transition's response: the mount's link, else the screen's own default, else Tools'. This module is the one reader of
// both maps; reportFixedVariables, reportHiddenComponents and reportDefinitionTypes are read by the screens they are for
// (Dashboard, Home, Definition).

// Tools' own screen for each link key. share has none (Tools shares in the definition editor's group field), nor has
// drill (Tools has no drill screen yet); a key with no screen here is offered only under a mount that names one.
const toolsLinks = {
    dashboards: "//apps/dashboards/Home",
    dashboard: "//apps/dashboards/Dashboard",
    definition: "//apps/dashboards/Definition",
    panel: "//apps/dashboards/Definition",
    explore: "//apps/dashboards/Explore",
    split: "//apps/dashboards/Split",
    playlist: "//apps/dashboards/Playlist",
    alerts: "//apps/dashboards/Alerts",
    alertRule: "//apps/dashboards/AlertRule",
    alertShared: "//apps/dashboards/AlertShared",
    logContext: "//apps/dashboards/LogContext",
    import: "//apps/dashboards/Home",
    share: null,
    drill: null
}

// The policy with no mount: what Tools' own screens do. Under a mount a field the app leaves out has the same default,
// but for the features, which a mount offers only when it says so (a mount that forgets the map offers neither).
//
//     features.alert         whether the viewer may set an alert rule here (new, copy, change)
//     features.share         whether the viewer may share a dashboard of their own here
//     range                  {from, thru}: the range a screen opens on when the request names none
//     datasetIdPrefix        what the id of a dataset created here starts with
//     datasetScopeParameter  the name of the scope policy's parameter a dataset form asks for (the field that carries a
//                            row's scope), sent in rowFilterParameters; null asks for none
//     recipientPermissionId  the permission a person holds to be proposed as a recipient of a report or an alert; null
//                            proposes by the engine's own rule
//     reportEmailTemplateId  the EmailTemplate of a delivered report; null is the engine's
//     alertAudience          {topic, emailTemplateId, detailLink}: the fixed parts of an alert rule told to an audience
//                            of users and groups; null is an alert rule with contact points, as Tools' are
//     shareGroups            [{value, label}]: groups a definition may be shared with beside the viewer's own
//     recipientCap           the most recipients a group delivery is saved and checked with; null is the engine's
//     calendar               {timeZone, weekOffset}: the zone calendar buckets are cut in and the offset of a week
//                            bucket (the engine's time_zone and offset); null is the viewer's zone and the instance's week
const toolsPolicy = () => ({
    features: {alert: true, share: true},
    range: {from: "now-7d", thru: "now"},
    datasetIdPrefix: "",
    datasetScopeParameter: null,
    recipientPermissionId: null,
    reportEmailTemplateId: null,
    alertAudience: null,
    shareGroups: [],
    recipientCap: null,
    calendar: null
})

// A mount's map, or null. A value that is not a map (a request parameter under a root that set none) is a map with
// nothing in it: no link and no policy is taken from it.
function mapOf(name) {
    const value = ec.context.get(name)
    if (value === null || value === undefined || value === "") return null
    return typeof value === "object" ? value : {}
}
const text = value => (typeof value === "string" && value !== "" ? value : null)
const mapIn = value => (value !== null && typeof value === "object" ? value : {})

function linksOf(given, mounted) {
    const links = {}
    for (const key of Object.keys(toolsLinks)) links[key] = mounted ? text(mapIn(given)[key]) : toolsLinks[key]
    return links
}
function policyOf(given, mounted) {
    const policy = toolsPolicy()
    if (!mounted) return policy
    const from = mapIn(given)
    const features = mapIn(from.features)
    policy.features = {alert: features.alert === true, share: features.share === true}
    const range = mapIn(from.range)
    policy.range = {from: text(range.from) || policy.range.from, thru: text(range.thru) || policy.range.thru}
    for (const name of ["datasetIdPrefix", "datasetScopeParameter", "recipientPermissionId", "reportEmailTemplateId"])
        policy[name] = text(from[name]) ?? policy[name]
    if (from.alertAudience !== null && typeof from.alertAudience === "object") {
        const audience = from.alertAudience
        policy.alertAudience = {
            topic: text(audience.topic),
            emailTemplateId: text(audience.emailTemplateId),
            detailLink: text(audience.detailLink)
        }
    }
    policy.shareGroups = Array.from(from.shareGroups || [])
        .filter(group => group !== null && typeof group === "object" && text(group.value))
        .map(group => ({value: group.value, label: text(group.label) || group.value}))
    const cap = Number(from.recipientCap)
    if (Number.isInteger(cap) && cap > 0) policy.recipientCap = cap
    if (from.calendar !== null && typeof from.calendar === "object")
        policy.calendar = {timeZone: text(from.calendar.timeZone), weekOffset: text(from.calendar.weekOffset)}
    return policy
}

function state() {
    const links = mapOf("reportLinks")
    const policy = mapOf("reportPolicy")
    const mounted = !!(links || policy)
    return {mounted, links: linksOf(links, mounted), policy: policyOf(policy, mounted)}
}
// Where a transition that follows a link answers: the mount's screen for the key; with no mount the screen's own default
// (a sibling, where the screen is included by others) or Tools'. A link the mount does not offer is not shown, so its
// transition is reached only by hand, and answers Tools' screen, which opens to its own users only.
function url(key, own) {
    return text(mapIn(mapOf("reportLinks"))[key]) || own || toolsLinks[key] || "."
}

;({state, url})
