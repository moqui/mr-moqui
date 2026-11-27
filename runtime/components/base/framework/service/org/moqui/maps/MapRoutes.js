/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// A record's links on a map (plans/archive/Maps.md §3.2; docs/tools/Maps.md "Record links"): an adapter's get#RecordMapContext names
// where its record's links go by route key ({label, route, parameters}), never a URL, and the components register where a key
// goes as moqui.maps.MapRoute seed rows. resolveLinks gives the framework's LocalMap and Browse (get#EffectiveConfig,
// record.resolvedLinks) a URL for each link it can, in the host app's context (routeApp, which an app's mount of the dialog
// passes): of a key's enabled rows, those of the host app by priority and then those of no app (never another app's: a HOME
// dialog does not link into BOSS for a user of both; a host that passes no routeApp, as an app's wrapper that sets none,
// gets the rows of no app), the first whose urlTemplate's every
// placeholder is in the link's parameters and whose screenLocation the user's own grants open (isPermittedOwn, view: an app
// root's grant of the screen showing the map does not count). A key with no row, or none the user may open, is left out
// silently; a row that fails to check is logged and passed over. A link is data, never authority: the screen it opens checks
// the user again. An application with its own resolution (BOSS's MapLinks.js) reads record.links as before.
const ROUTE = "moqui.maps.MapRoute"
// at most this many of an answer's links are resolved
const MAX_LINKS = 20
const text = value => (value == null ? "" : String(value).trim())

// A route's URL for the parameters, or null: a path of this site (one "/", not "//" or "/\", no space, quote, angle bracket
// or backslash), each {name} the parameter's value URL-encoded (so a value stays one path segment or one query value); a
// placeholder with no value, a value of "." or "..", or a template that is not such a path, gives none.
function fillTemplate(template, parameters) {
    const given = text(template)
    if (!/^\/(?![/\\])/.test(given) || /[\s<>"'`\\]/.test(given)) return null
    let missing = false
    const url = given.replace(/\{([A-Za-z0-9_]{1,60})\}/g, (match, name) => {
        const value = parameters && typeof parameters === "object" ? parameters[name] : null
        if (value == null || typeof value === "object" || ["", ".", ".."].includes(text(value))) {
            missing = true
            return ""
        }
        return encodeURIComponent(text(value))
    })
    return missing || /[{}]/.test(url) ? null : url
}

// The rows considered for a key in an app's context: the app's own by priority, then those of no app; another app's never.
function candidates(rows, routeApp) {
    const app = text(routeApp)
    const live = Array.from(rows).filter(row => row.disabled !== "Y")
    return (app ? live.filter(row => text(row.routeApp) === app) : []).concat(live.filter(row => !text(row.routeApp)))
}

// Whether a link goes to the record the map shows: each of its parameters is a key field of the record's key, with the same
// value (a map opened beside a record has no use for a link back to it)
function linksToShown(link, shownKey) {
    const parameters = link?.parameters && typeof link.parameters === "object" ? link.parameters : null
    const names = parameters ? Object.keys(parameters) : []
    if (!names.length || !shownKey || typeof shownKey !== "object") return false
    const shownNames = Object.keys(shownKey)
    return names.every(name => shownNames.includes(name) && text(shownKey[name]) === text(parameters[name]))
}

// resolveLinks(links, routeApp, shownKey): [{label, route, url}] of an adapter's links ({label, route, parameters}) a registered
// route resolves, in the host app's context, to a screen the user may open, in the answer's order; a link to the record the
// map shows (shownKey, its key fields) is left out.
function resolveLinks(links, routeApp, shownKey) {
    const out = []
    const byKey = new Map()
    for (const link of Array.from(links || []).slice(0, MAX_LINKS)) {
        const routeKey = text(link?.route)
        if (!/^[A-Za-z0-9_.-]{1,60}$/.test(routeKey) || linksToShown(link, shownKey)) continue
        if (!byKey.has(routeKey))
            byKey.set(
                routeKey,
                candidates(
                    ec.entity
                        .find(ROUTE)
                        .condition("routeKey", routeKey)
                        .orderBy("priority")
                        .orderBy("mapRouteId")
                        .useCache(true)
                        .disableAuthz()
                        .list(),
                    routeApp
                )
            )
        for (const row of byKey.get(routeKey)) {
            const url = fillTemplate(row.urlTemplate, link.parameters)
            if (!url || !text(row.screenLocation)) continue
            let permitted = false
            try {
                permitted = !!ec.artifactExecution.isPermittedOwn(text(row.screenLocation), "AT_XML_SCREEN", "view")
            } catch (err) {
                ec.logger.warn("Map route " + row.mapRouteId + " not checked: " + (err?.message ?? err))
            }
            if (!permitted) continue
            out.push({label: text(link.label) || routeKey, route: routeKey, url})
            break
        }
    }
    return out
}

;({resolveLinks, fillTemplate})
