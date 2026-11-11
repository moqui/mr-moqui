// The mount of the administration screens (the tools README, "Mounting the administration screens";
// plans/Admin-Screens-Down.md). The System screens of the areas Access, Security, Settings, Operations, Data, Audit,
// Integrations and Mail and protocols are also shown by another app under its own root (IronStone Admin): mounted by
// location, or included in a screen of the app's own with the transitions it takes in. Such a root sets `adminMount` in
// its always-actions, so that its transitions read it too; System's root sets it to null, so a request parameter never
// supplies one there. The map is data only, never authority:
//
//     routes   {key: {url, idParameter, parameters}}: where a link to a record goes under this mount. The keys are the
//              registrations' own (a recovery descriptor's or transfer target's ownerRoute, a provider's linkKey: "a key
//              the application showing it resolves", framework/entity/ServerEntities.xml) and the framework's below; a
//              mount adds its apps' and may replace the framework's (the user's page). A key with no route is no link.
//     hide     the part keys a mount leaves out of the screens it shows
//
// state() answers {mounted, raw, routes, hidden} for a screen's actions. raw is whether the instance administrator's
// parts are shown: what only the System screens do (any field of a record, records with no registration, artifact
// authorization), never under a mount, and only to a user who opens System by their own grant. The question is asked of
// the user's own authorization records (ec.artifactExecution.isPermittedOwn): a mount's inheritable always-allow answers
// isPermitted for everyone below it, and passes the transitions and entity operations below it too, so every transition of
// a raw part carries raw() as its condition and a screen with one is never mounted by location under another app.
const systemLocation = "component://tools/screen/System.xml"

// Where the framework's own records open, relative to a screen of an area (System/<Area>/<Screen> and a mount's
// <Root>/<Area>/<Screen> alike); each phase of the move adds the keys its screens link.
const frameworkRoutes = {
    user: {url: "../../Security/UserAccount/UserAccountDetail", idParameter: "userId"}
}

// The mount's map, or null. A value that is not a map (a request parameter under a root that set none) is a mount with
// nothing in it: no route is taken from it and nothing raw is shown.
function mountOf() {
    const mount = ec.context.get("adminMount")
    if (mount === null || mount === undefined || mount === "") return null
    return typeof mount === "object" ? mount : {}
}
function routesOf(mount) {
    const routes = Object.assign({}, frameworkRoutes)
    const given = mount && typeof mount.routes === "object" && mount.routes ? mount.routes : {}
    for (const key of Object.keys(given)) {
        const route = given[key]
        if (route && typeof route === "object" && route.url) routes[key] = route
        else if (route === null) delete routes[key]
    }
    return routes
}
const ownsSystem = () => !!ec.user.userId && ec.artifactExecution.isPermittedOwn(systemLocation, "AT_XML_SCREEN", "view")

function raw() {
    return !mountOf() && ownsSystem()
}
function state() {
    const mount = mountOf()
    const hidden = Array.from((mount && mount.hide) || []).map(String)
    return {mounted: !!mount, raw: !mount && ownsSystem(), routes: routesOf(mount), hidden}
}
// A route's link for one record: {url, parameters} with the record's id under the route's parameter, or null when the
// mount has no place for the key.
function link(routes, key, id) {
    const route = key ? (routes || {})[key] : null
    if (!route) return null
    const parameters = Object.assign({}, route.parameters || {})
    if (route.idParameter && id !== null && id !== undefined) parameters[route.idParameter] = id
    return {url: route.url, parameters}
}

;({state, raw, link})
