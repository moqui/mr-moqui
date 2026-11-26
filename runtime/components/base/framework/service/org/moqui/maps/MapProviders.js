// The map providers of org.moqui.maps.MapServices (plans/Maps.md §4.2, MapProvider): the mapProvider connection kind and
// the Maps readiness checks.
//
// The kind is built on the engine of ConfigurationConnections.js (lib.engine), so org.moqui.admin.ConfigurationServices
// lists, reads, changes and tests a MapProvider row as every other connection: reading needs ADMIN_CONNECTION_VIEW, a
// change or a test ADMIN_CONNECTION_MANAGE; the key (apiKey) is write-only; a change of the endpoint or the kind clears it
// and disables the provider until a test of it as it is then passes; and a provider is enabled only by a passing test, so
// the seeded OpenFreeMap row, which has none, stays disabled until an administrator tests and then enables it. While
// maps@external-requests is false a provider is neither tested nor enabled (it may still be changed, and disabled).
//
// The test is one GET with no credentials and no body, never with the key, following no redirect (a 3xx fails, naming
// where it pointed), reading at most 2 MB, to no internal address unless an instance administrator allowed the provider
// the private network (allowPrivateNetwork, which is shown here and changed only in Tools, as for every kind): a hosted
// style must parse as a MapLibre style (version 8, sources, layers) and name its sources, tiles, sprite and glyphs on its
// own origin (scheme, host and port) only, so the map's allowed origins (the provider's) cover all it fetches; a raster
// template must answer an image for tile 0/0/0, a TileJSON name its tiles on its own origin; a geocoder must answer its
// service's fixed test search (MapGeocoders.js test: a public address or a city, never a user's text); an archive
// provider has no URL (store#Connection refuses one, ARCHIVE_ENDPOINT) and is sent nothing. Its texts are a fixed
// vocabulary (an origin only when it is a host name or an IP literal, a status, a kind of failure), never the
// response's own text. A map draws an outside URL only of an enabled provider that passed its test as it is configured
// now, on the provider's own origin (tested, for MapConfig.js).
//
// The readiness checks (moqui.server.ReadinessProvider maps.profile and maps.providers, data/AdminConfigurationData.xml): the
// installation's default profile is set and can be drawn, and the outline fallback in use is said; each enabled
// provider's last test passed for the configuration it has now, a provider with a key is said not to be drawn, a layer
// that asks an outside host with no provider is said not to be drawn, and the page's Content-Security-Policy (the conf's
// screen-render header and webapp@csp-append) lets the browser reach each provider's origins (each tile subdomain) in
// connect-src, img-src and font-src, with where to add what it lacks.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationLib.js")
const connections = ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationConnections.js").lib
const mapConfig = ec.resource.script("component://framework/service/org/moqui/maps/MapConfig.js")
const {find, one, rows, map, text, refused, plainText, urlText} = lib
const {hostOf, failureText, enumOptions, VIEW, MANAGE} = connections

const PROVIDER = "moqui.maps.MapProvider"
const LAYER = "moqui.maps.MapLayer"
const CHECK = "moqui.server.ConnectionCheck"
const ADAPTER_KEY = "mapProvider"
const OVERVIEW = "ADMIN_OVERVIEW_VIEW"
const yes = value => value === true || value === "true" || value === "Y"
const offline = () => text(ec.factory.getConfValue("maps@external-requests")) === "false"
const production = () => text(ec.factory.getConfValue("instance@purpose")) === "production"
const privateAllowed = row => row?.allowPrivateNetwork === "Y" || (row?.allowPrivateNetwork !== "N" && !production())
const L = value => ec.l10n.localize(value)
const say = (value, values) => ec.resource.expand(L(value), "", values || {})
const count = (entity, conditions) => Number(find(entity, conditions).count())

// ===== the kind's validation =====
// The attribution: plain text of at most 20 lines, each a line of at most 255 characters.
function attributionText(value, what) {
    const found = String(value ?? "")
        .replace(/\r\n?/g, "\n")
        .split("\n")
        .map(line => plainText(line, what, 255))
        .filter(line => line)
    if (found.length > 20) throw new Error(what + " has at most 20 lines")
    return found.join("\n")
}
// The attribution's links: an http(s) URL a line, at most 20.
function urlLines(value, what) {
    const found = String(value ?? "")
        .replace(/\r\n?/g, "\n")
        .split("\n")
        .map(line => line.trim())
        .filter(line => line)
        .map(line => urlText(line, what))
    if (found.length > 20) throw new Error(what + " has at most 20 lines")
    return found.join("\n")
}
// A provider's endpoint: an http(s) URL, a tile template's {z}, {x}, {y} and subdomain ({s}, {a-c}) kept; never a
// mapbox:// URL or a path.
function endpointText(value, what) {
    const found = text(value)
    if (!found) return ""
    urlText(templateUrls(found)[0] || found, what)
    return found
}
// The geocoders (MapGeocoders.js), loaded when asked: that module loads this one for tested
const geocoders = () => ec.resource.script("component://framework/service/org/moqui/maps/MapGeocoders.js")
// A whole number from 1 to max, or empty
function wholeText(value, what, max) {
    const found = text(value)
    if (!found) return ""
    if (!/^\d{1,9}$/.test(found) || Number(found) < 1 || Number(found) > max)
        throw new Error(say("${what} is a whole number from 1 to ${max}", {what: L(what), max}))
    return String(Number(found))
}
const OPERATIONS = ["style", "tiles", "geocode", "search", "reverse", "autocomplete"]
// Operations, comma separated, each of OPERATIONS
function operationsText(value) {
    const found = text(value)
        .toLowerCase()
        .split(/[\s,]+/)
        .filter(each => each)
    const unknown = found.filter(each => !OPERATIONS.includes(each))
    if (unknown.length) throw new Error(say("Allowed operations are of: ${list}", {list: OPERATIONS.join(", ")}))
    return Array.from(new Set(found)).join(",")
}
// The concrete URLs of a URL or a tile template (MapConfig.js templateUrls)
const templateUrls = url => mapConfig.templateUrls(url)

// ===== the test =====
const MAX_BYTES = 2 * 1024 * 1024
// An origin as a test's text may show it: a host name or an IP literal of a sane length, else "an invalid host"; never the
// response's own text.
function shownOrigin(origin) {
    const value = text(origin)
    const parts =
        /^(https?):\/\/(\[[0-9a-f:.]{2,45}\]|[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)*)(:\d{1,5})?$/.exec(
            value
        )
    return parts && value.length <= 260 ? value : L("an invalid host")
}
// The origins a style or TileJSON names that are not the endpoint's own (scheme, host and port): an absolute URL by its
// concrete origins, a relative one is the endpoint's own, another scheme is named by its scheme.
function strangeOrigins(urls, endpoint) {
    const own = new Set(
        templateUrls(endpoint)
            .map(mapConfig.originOf)
            .filter(origin => origin)
    )
    const scheme = (/^(https?):/i.exec(endpoint)?.[1] || "https").toLowerCase()
    const found = new Set()
    for (const url of urls) {
        const value = typeof url === "string" ? url.trim() : ""
        if (!value) continue
        const absolute = value.startsWith("//") ? scheme + ":" + value : value
        if (!/^[a-z][a-z0-9+.-]*:/i.test(absolute)) continue
        if (!/^https?:/i.test(absolute)) {
            const other = absolute.split(":")[0].toLowerCase()
            found.add(/^[a-z][a-z0-9+.-]{0,15}$/.test(other) ? other + ":" : L("an invalid host"))
            continue
        }
        const origins = templateUrls(absolute).map(mapConfig.originOf)
        if (!origins.length) found.add(L("an invalid host"))
        for (const origin of origins)
            if (!origin || !own.has(origin)) found.add(origin ? shownOrigin(origin) : L("an invalid host"))
    }
    return Array.from(found)
}
// One GET, following no redirect (a 3xx fails, naming where it pointed), with no credentials: {status, redirect, tooLarge,
// contentType, body(): the text, at most MAX_BYTES}.
function fetchOnce(url, accept, denyInternal) {
    let client = ec.service.rest().method("GET").uri(url).acceptContentType(accept).timeout(10).followRedirects(false)
    client = denyInternal ? client.denyInternal() : client.allowInternal()
    const response = client.send()
    const status = Number(response.getStatusCode())
    if (status >= 300 && status < 400) {
        // where it pointed, absolute or against the URL asked
        const location = text(response.headerFirst("location") || response.headerFirst("Location"))
        const target = /^https?:\/\//i.test(location)
            ? mapConfig.originOf(location)
            : location.startsWith("//")
              ? mapConfig.originOf((/^(https?):/i.exec(url)?.[1] || "https") + ":" + location)
              : location.startsWith("/")
                ? mapConfig.originOf(url)
                : null
        return {status, redirect: target ? shownOrigin(target) : L("an invalid host")}
    }
    const length = Number(response.headerFirst("content-length"))
    if (Number.isFinite(length) && length > MAX_BYTES) return {status, tooLarge: true}
    return {
        status,
        contentType: text(response.getContentType()).toLowerCase(),
        body: () => {
            const found = response.text()
            if (found.length > MAX_BYTES) throw new Error("too large")
            return found
        }
    }
}
// A fetch's failure as a check row, or null when it answered 2xx.
function fetchFailure(answer, place, what) {
    if (answer.redirect)
        return {
            check: what,
            ok: false,
            text: say("${place} redirected to ${target}: a provider's URL is the one it answers at", {
                place,
                target: answer.redirect
            })
        }
    if (answer.tooLarge) return {check: what, ok: false, text: say("${place} answered more than 2 MB", {place})}
    if (answer.status < 200 || answer.status > 299)
        return {check: what, ok: false, text: say("${place} answered HTTP ${status}", {place, status: answer.status})}
    return null
}
function fetchJson(url, context, what) {
    const place = shownOrigin(mapConfig.originOf(url))
    const answer = fetchOnce(url, "application/json", context.denyInternal)
    const failed = fetchFailure(answer, place, what)
    if (failed) return {failed}
    try {
        return {json: JSON.parse(answer.body()), place}
    } catch (error) {
        return {
            failed: {check: what, ok: false, text: say("${place} answered something that is not JSON of 2 MB or less", {place})}
        }
    }
}
function hostsRow(others, endpoint, what) {
    const place = shownOrigin(mapConfig.originOf(templateUrls(endpoint)[0]))
    return others.length
        ? {
              check: "Hosts",
              ok: false,
              text: say("${what} names other hosts (${hosts}): a map is allowed only the provider's own, ${place}", {
                  what,
                  hosts: others.slice(0, 5).join(", "),
                  place
              })
          }
        : {check: "Hosts", ok: true, text: say("${what} names only ${place}", {what, place})}
}
function styleTest(url, context) {
    const answer = fetchJson(url, context, "Style")
    if (answer.failed) return [answer.failed]
    const style = answer.json
    if (
        !style ||
        typeof style !== "object" ||
        Number(style.version) !== 8 ||
        !style.sources ||
        typeof style.sources !== "object" ||
        !Array.isArray(style.layers)
    )
        return [
            {
                check: "Style",
                ok: false,
                text: say("${place} answered a document that is not a MapLibre style (version 8 with sources and layers)", {
                    place: answer.place
                })
            }
        ]
    const urls = []
    for (const source of Object.values(style.sources)) {
        if (!source || typeof source !== "object") continue
        urls.push(source.url, typeof source.data === "string" ? source.data : null)
        for (const tile of Array.isArray(source.tiles) ? source.tiles : []) urls.push(tile)
    }
    for (const sprite of Array.isArray(style.sprite) ? style.sprite.map(each => each?.url) : [style.sprite]) urls.push(sprite)
    urls.push(style.glyphs)
    return [
        {
            check: "Style",
            ok: true,
            text: say("${place} answered a style of ${sources} sources and ${layers} layers; no credentials were sent", {
                place: answer.place,
                sources: Object.keys(style.sources).length,
                layers: style.layers.length
            })
        },
        hostsRow(strangeOrigins(urls, url), url, L("The style"))
    ]
}
function rasterTest(url, context) {
    if (/\{[zxy]\}/.test(url)) {
        const tile = templateUrls(url)[0]
        const place = shownOrigin(mapConfig.originOf(tile))
        const answer = fetchOnce(tile, "image/*", context.denyInternal)
        const failed = fetchFailure(answer, place, "Tiles")
        if (failed) return [failed]
        return [
            answer.contentType.startsWith("image/")
                ? {
                      check: "Tiles",
                      ok: true,
                      text: say("${place} answered an image for tile 0/0/0; no credentials were sent", {place})
                  }
                : {check: "Tiles", ok: false, text: say("${place} did not answer an image for tile 0/0/0", {place})}
        ]
    }
    const answer = fetchJson(url, context, "Tiles")
    if (answer.failed) return [answer.failed]
    const tiles = answer.json && Array.isArray(answer.json.tiles) ? answer.json.tiles : null
    if (!tiles || !tiles.length)
        return [{check: "Tiles", ok: false, text: say("${place} answered no TileJSON with tiles", {place: answer.place})}]
    return [
        {
            check: "Tiles",
            ok: true,
            text: say("${place} answered a TileJSON of ${count} tile URLs; no credentials were sent", {
                place: answer.place,
                count: tiles.length
            })
        },
        hostsRow(strangeOrigins(tiles, url), url, L("The TileJSON"))
    ]
}
function providerTest(parts, context) {
    const provider = parts.main
    const kind = text(provider.providerKindEnumId)
    const url = text(provider.endpointUrl)
    if (kind === "MPK_ARCHIVE")
        return url
            ? [
                  {
                      check: "Archives",
                      ok: false,
                      text: L("A provider of self-hosted archives has no URL: its archives are this server's resources")
                  }
              ]
            : [{check: "Archives", ok: true, text: L("Archives are read from this server's resources; nothing was sent")}]
    try {
        if (kind === "MPK_VECTOR_STYLE") return styleTest(url, context)
        if (kind === "MPK_RASTER_TILES") return rasterTest(url, context)
        // a geocoder: its service's fixed test search (MapGeocoders.js), never a user's text
        if (geocoders().isGeocoderKind(kind)) return geocoders().test(provider, context.denyInternal)
        return [Object.assign({check: "Address"}, connections.probe(url, context.denyInternal))]
    } catch (error) {
        return [
            {
                check: "Connection",
                ok: false,
                text: failureText(shownOrigin(mapConfig.originOf(templateUrls(url)[0])), error?.message ?? error)
            }
        ]
    }
}

// ===== the kind =====
const definition = {
    adapterKey: ADAPTER_KEY,
    parts: {main: {entity: PROVIDER, keyField: "providerId"}},
    statePart: "main",
    fields: [
        {name: "providerName", label: "Name", kind: "text"},
        {name: "description", label: "Description", kind: "text"},
        {
            name: "providerKindEnumId",
            label: "Kind",
            kind: "choice",
            // what the test asks, and what the endpoint means, depend on it
            endpoint: true,
            options: () => enumOptions("MapProviderKind")
        },
        {
            name: "endpointUrl",
            label: "Style or tile URL",
            kind: "url",
            endpoint: true,
            validate: value => endpointText(value, "The style or tile URL"),
            help: "A hosted style document's URL, a raster tile template with {z}, {x} and {y}, or a geocoder's base URL (https://geocoding.geo.census.gov/geocoder, a Nominatim or Photon server's root); empty for self-hosted archives"
        },
        {
            name: "attribution",
            label: "Attribution",
            kind: "text",
            validate: value => attributionText(value, "The attribution"),
            help: "Plain text, a part a line, shown with the provider's layers"
        },
        {
            name: "attributionUrls",
            label: "Attribution links",
            kind: "text",
            validate: value => urlLines(value, "An attribution link"),
            help: "The link of each part of the attribution, a line each"
        },
        {name: "termsUrl", label: "Terms of use", kind: "url"},
        // the provider's policy for this installation: what it is used for, what may keep its answers and its rate. None
        // is an endpoint field, so a change of one needs no new test (the fingerprint is the endpoint's)
        {
            name: "allowedOperations",
            label: "Allowed operations",
            kind: "text",
            validate: value => operationsText(value),
            help: "What it is used for, comma separated: style, tiles, geocode, search, reverse, autocomplete. A geocoder is asked only when this is empty or names geocode or search"
        },
        {
            name: "cachePolicy",
            label: "Cache policy",
            kind: "choice",
            options: () => [
                {value: "none", label: L("None: nothing of its answers is kept")},
                {value: "private", label: L("Private: only the user's browser may keep them; not this server")},
                {value: "shared", label: L("Shared: this server keeps them for every user")}
            ],
            help: "What its terms permit keeping; empty is none"
        },
        {
            name: "cacheSeconds",
            label: "Cache seconds",
            kind: "number",
            validate: value => wholeText(value, "Cache seconds", 31536000),
            help: "How long a shared answer is kept; empty for what the provider's Cache-Control max-age says (nothing kept without one)"
        },
        {
            name: "requestsPerSecond",
            label: "Requests per second",
            kind: "number",
            validate: value => wholeText(value, "Requests per second", 1000),
            help: "The most searches a second this server sends it (on each server of a cluster by itself); empty is 1 for Nominatim and Photon, none for the others"
        },
        {
            name: "requestsPerDay",
            label: "Requests per day",
            kind: "number",
            validate: value => wholeText(value, "Requests per day", 100000000),
            help: "Recorded for the provider's terms; not enforced"
        },
        {
            name: "privateNetwork",
            label: "Private network",
            kind: "display",
            read: parts =>
                privateAllowed(parts.main)
                    ? L("Allowed: the server may reach it on its own network")
                    : L("Not allowed: an instance administrator allows it in Tools")
        }
    ],
    // the key stays on the server: never returned, never in map JSON, never sent by the test
    secrets: [{name: "apiKey", label: "API key", help: "Kept on the server; a provider with a key is not drawn in a browser"}],
    name: parts => text(parts.main.providerName) || parts.main.providerId,
    providerName: () => "Map provider",
    summary: parts => hostOf(parts.main.endpointUrl) || (text(parts.main.resourceRoot) ? L("Archives on this server") : ""),
    listKeys: () =>
        Array.from(find(PROVIDER).selectField("providerId").orderBy("providerId").limit(500).list()).map(row =>
            String(row.providerId)
        ),
    incomplete: parts => {
        const kind = text(parts.main.providerKindEnumId)
        if (!kind) return L("Choose the provider's kind")
        if (kind === "MPK_GEOCODER") return L("Choose the geocoder's service: US Census, Nominatim or Photon")
        if (kind !== "MPK_ARCHIVE" && !text(parts.main.endpointUrl)) return L("Give the provider's style or tile URL")
        return null
    },
    // an archive provider's layers are this server's resources: a URL would be an outside host no test asks
    check: after =>
        text(after.main?.providerKindEnumId) === "MPK_ARCHIVE" && text(after.main?.endpointUrl)
            ? refused(
                  "ARCHIVE_ENDPOINT",
                  L("A provider of self-hosted archives has no URL: its archives are this server's resources")
              )
            : null,
    test: providerTest,
    newParts: values => {
        if (!plainText(values.providerName, "The name")) throw new Error(L("Give the provider a name"))
        let providerId = text(values.connectionId)
        if (!providerId) {
            const value = ec.entity.makeValue(PROVIDER)
            value.setSequencedIdPrimary()
            providerId = String(value.providerId)
        }
        if (!/^[A-Za-z0-9][A-Za-z0-9_]{0,39}$/.test(providerId))
            throw new Error(L("A provider's id is up to 40 letters, digits or underscores"))
        return {connectionKey: providerId, parts: {main: {providerId}}}
    },
    consumers: (parts, key) => {
        const layers = count(LAYER, {providerId: key})
        return [
            {
                ownerName: "Maps",
                recordType: "MapLayer",
                recordId: null,
                recordName: layers + (layers === 1 ? " layer" : " layers"),
                use: L("Drawn from this provider"),
                state: "ok"
            }
        ]
    }
}
const kind = connections.engine(definition)

// Whether a provider passed its last test as it is configured now (its ConnectionCheck's fingerprint is its configuration's),
// which an outside URL of a map needs (MapConfig.js).
function tested(provider) {
    if (!provider?.providerId) return false
    const check = one(CHECK, {adapterKey: ADAPTER_KEY, connectionKey: provider.providerId})
    return (
        !!check?.testedConfigHash &&
        check.lastCheckPassed === "Y" &&
        check.testedConfigHash === kind.fingerprint({main: map(provider)})
    )
}

// run#MapProviderAdapter: one operation of the kind, for org.moqui.admin.ConfigurationServices. With outside requests off a
// provider is neither tested nor enabled.
function runMapProviderAdapter(p) {
    lib.requireView(VIEW, MANAGE)
    if (text(p.adapterKey) !== ADAPTER_KEY) throw new Error("No map provider kind " + text(p.adapterKey))
    const operation = text(p.operation)
    if (offline() && (operation === "test" || (operation === "store" && yes(p.enabled)))) {
        lib.requireManage(MANAGE)
        const parts = kind.load(text(p.connectionKey))
        if (!parts || text(parts.main.providerKindEnumId) !== "MPK_ARCHIVE")
            return {
                result: refused(
                    "EXTERNAL_REQUESTS_OFF",
                    L(
                        "Outside map requests are turned off (maps@external-requests): a map provider is neither tested nor enabled"
                    ),
                    {connectionKey: text(p.connectionKey) || null, passed: false}
                )
            }
    }
    return {result: kind.run(operation, p)}
}

// ===== readiness =====
function denied(permission) {
    const error = new Error("PERMISSION_DENIED: " + permission + " is required")
    error.reason = "PERMISSION_DENIED"
    throw error
}
const check = (checkKey, title, state, reasonCode, message, extra) =>
    Object.assign({checkKey, title, state, reasonCode, message, subject: "Instance"}, extra || {})

function profileChecks() {
    if (!ec.user.hasPermission(OVERVIEW)) denied(OVERVIEW)
    const out = []
    const defaultId = mapConfig.installationProfileId()
    const outlineRow = one("moqui.maps.MapProfile", {profileId: mapConfig.OUTLINE_ID})
    if (!defaultId)
        out.push(
            check(
                "maps.profile.default",
                "Default map profile",
                "warning",
                "DEFAULT_PROFILE_UNSET",
                L("No default map profile is set (the ALL_USERS preference maps.profile): maps draw the outline"),
                {fixRoute: "mapsConfiguration"}
            )
        )
    else {
        const profile = one("moqui.maps.MapProfile", {profileId: defaultId})
        const resolved = mapConfig.resolveProfile(profile, defaultId)
        out.push(
            resolved.ok
                ? check(
                      "maps.profile.default",
                      "Default map profile",
                      "ready",
                      null,
                      say("${name} is drawn by default", {name: text(profile.profileName) || defaultId})
                  )
                : check(
                      "maps.profile.default",
                      "Default map profile",
                      "failed",
                      resolved.reasonCode,
                      say("The default map profile ${id} cannot be drawn: ${reason}; maps fall back to the outline", {
                          id: defaultId,
                          reason: resolved.reason
                      }),
                      {fixRoute: "mapsConfiguration"}
                  )
        )
    }
    // informational: the outline, which makes no outside request, is what is drawn or what a map falls back to
    if (!outlineRow)
        out.push(
            check(
                "maps.profile.outline",
                "Outline fallback",
                "warning",
                "OUTLINE_MISSING",
                L("The outline profile (OUTLINE) is missing: a map that falls back draws the built-in outline"),
                {fixRoute: "mapsConfiguration"}
            )
        )
    else if (!defaultId || defaultId === mapConfig.OUTLINE_ID)
        out.push(
            check(
                "maps.profile.outline",
                "Outline fallback",
                "ready",
                "OUTLINE_IN_USE",
                L("Maps draw country outlines only, from this server: no basemap is configured")
            )
        )
    else
        out.push(
            check(
                "maps.profile.outline",
                "Outline fallback",
                "ready",
                null,
                L("A map whose profile cannot be drawn falls back to the country outlines")
            )
        )
    return out
}

// The page's Content-Security-Policy as directives: {name: {sources, from: header or append}} (the first of a name counts,
// as a browser reads it).
function cspDirectives() {
    const header = text(
        ec.factory.getConfValue("webapp-list/webapp[webroot]/response-header[screen-render,Content-Security-Policy]@value")
    )
    const append = text(ec.factory.getConfValue("webapp-list/webapp[webroot]@csp-append"))
    const found = {}
    for (const [from, policy] of [
        ["header", header],
        ["append", append]
    ])
        for (const part of policy.split(";")) {
            const words = part
                .trim()
                .split(/\s+/)
                .filter(word => word)
            if (!words.length) continue
            const name = words[0].toLowerCase()
            if (!Object.hasOwn(found, name)) found[name] = {sources: words.slice(1), from}
        }
    return found
}
const defaultPort = scheme => (scheme === "https" ? "443" : "80")
// Whether a source list lets a page of pageScheme reach a URL (CSP3 source matching): * and a scheme (http: allows https
// too), a host with an optional scheme (a source without one has the page's, and an http page's also allows https), a
// wildcard subdomain, a port (the scheme's default when none is given) and a path (a prefix when it ends with /).
function sourceAllows(sources, url, pageScheme) {
    const parts = /^(https?):\/\/([^\/?#:]+)(?::(\d+))?([^?#]*)/i.exec(url)
    if (!parts) return false
    const scheme = parts[1].toLowerCase()
    const host = parts[2].toLowerCase()
    const port = parts[3] || defaultPort(scheme)
    const path = parts[4] || "/"
    const schemeOk = given => given === scheme || (given === "http" && scheme === "https")
    for (const raw of sources) {
        const source = raw.toLowerCase()
        if (source === "*") return true
        if (/^[a-z][a-z0-9+.-]*:$/.test(source)) {
            if (schemeOk(source.slice(0, -1))) return true
            continue
        }
        if (source.startsWith("'")) continue
        const found = /^(?:([a-z][a-z0-9+.-]*):\/\/)?(\*\.)?([a-z0-9.-]+|\*)(?::(\d+|\*))?(\/[^?#]*)?$/.exec(source)
        if (!found) continue
        const [, sourceScheme, wildcard, sourceHost, sourcePort, sourcePath] = found
        if (!schemeOk(sourceScheme || pageScheme)) continue
        if (!(sourceHost === "*" || (wildcard ? host.endsWith("." + sourceHost) : sourceHost === host))) continue
        const portOk =
            sourcePort === "*" ||
            (sourcePort
                ? sourcePort === port || (sourcePort === "80" && scheme === "https" && port === "443")
                : port === defaultPort(scheme) ||
                  (scheme === "https" && (sourceScheme || pageScheme) === "http" && port === "443"))
        if (!portOk) continue
        if (sourcePath && !(sourcePath.endsWith("/") ? path.startsWith(sourcePath) : path === sourcePath)) continue
        return true
    }
    return false
}
// What the page's policy blocks of these URLs: each directive with the origins to add and where to add them.
function cspBlocked(urls, directives) {
    const pageScheme = text(ec.factory.getConfValue("webapp-list/webapp[webroot]@https-enabled")) === "true" ? "https" : "http"
    const missing = []
    for (const directive of ["connect-src", "img-src", "font-src"]) {
        const governing = directives[directive] || directives["default-src"]
        if (!governing) continue
        const origins = new Set()
        for (const url of urls) if (!sourceAllows(governing.sources, url, pageScheme)) origins.add(mapConfig.originOf(url))
        origins.delete(null)
        if (!origins.size) continue
        // a directive the header sets is changed there: a second one appended is ignored
        const where = directives[directive]?.from === "header" ? "header" : "append"
        missing.push({directive, origins: Array.from(origins).sort(), where})
    }
    return missing
}
const outsideUrl = url => /^https?:\/\//i.test(text(url))
function providerChecks() {
    if (!ec.user.hasPermission(VIEW) && !ec.user.hasPermission(MANAGE)) denied(VIEW)
    const enabled = rows(PROVIDER)
        .filter(row => row.disabled !== "Y")
        .slice(0, 100)
    // a layer that asks an outside host with no provider of its own is never drawn: no test vouches for its host
    const loose = rows(LAYER)
        .filter(row => row.disabled !== "Y" && !text(row.providerId) && [row.sourceUrl, row.styleLocation].some(outsideUrl))
        .slice(0, 50)
    if (!enabled.length && !loose.length)
        return [
            check("maps.providers", "Map providers", "ready", null, L("No map provider is enabled: maps make no outside request"))
        ]
    const out = []
    for (const layer of loose)
        out.push(
            check(
                "maps.layer." + layer.layerId + ".source",
                "Map layer source",
                "warning",
                "SOURCE_UNTESTED",
                say(
                    "The layer ${name} asks a host outside this server with no map provider, so it is not drawn: give it a tested provider",
                    {
                        name: text(layer.layerName) || layer.layerId
                    }
                ),
                {subject: text(layer.layerName) || layer.layerId, fixRoute: "mapsConfiguration"}
            )
        )
    const directives = cspDirectives()
    const off = offline()
    for (const provider of enabled) {
        const providerId = provider.providerId
        const name = text(provider.providerName) || providerId
        const fix = {
            fixRoute: "adminConnectionDetail",
            fixParameters: {adapterKey: ADAPTER_KEY, connectionKey: providerId},
            subject: name
        }
        const last = one(CHECK, {adapterKey: ADAPTER_KEY, connectionKey: providerId})
        const key = "maps.provider." + providerId
        out.push(
            tested(provider)
                ? check(key + ".test", "Map provider test", "ready", null, say("${name} passed its last test", {name}), fix)
                : last?.lastCheckDate && last.lastCheckPassed !== "Y"
                  ? check(
                        key + ".test",
                        "Map provider test",
                        "failed",
                        "TEST_FAILED",
                        say("${name} failed its last test", {name}),
                        fix
                    )
                  : check(
                        key + ".test",
                        "Map provider test",
                        "warning",
                        "UNTESTED",
                        say("${name} is enabled but not tested as it is configured now: maps do not draw it", {name}),
                        fix
                    )
        )
        // a geocoder is asked by this server, never by a browser: no key rule and no page policy apply
        const geocoder = geocoders().isGeocoderKind(provider.providerKindEnumId)
        // its key would be in the URLs a browser asks: it is not offered to a browser, so the page policy does not matter
        if (text(provider.apiKey) && !geocoder) {
            out.push(
                check(
                    key + ".keyed",
                    "Map provider key",
                    "warning",
                    "PROVIDER_KEYED",
                    say("${name} needs a key, which is never sent to a browser: maps do not draw it", {name}),
                    fix
                )
            )
            continue
        }
        if (off) {
            out.push(
                check(
                    key + ".external",
                    "Map provider requests",
                    "warning",
                    "EXTERNAL_REQUESTS_OFF",
                    say("Outside map requests are turned off (maps@external-requests): ${name} is not offered to a map", {name}),
                    fix
                )
            )
            continue
        }
        if (geocoder) {
            // a public demo server: its policy allows an application very little (plans/Maps.md §6.2)
            const host = geocoders().publicHost(provider)
            if (host)
                out.push(
                    check(key + ".public", "Geocoder policy", "warning", "PUBLIC_GEOCODER", geocoders().publicHostText(host), fix)
                )
            continue
        }
        // the URLs a browser asks for this provider's layers, each tile subdomain by itself
        const urls = new Set(templateUrls(provider.endpointUrl).filter(outsideUrl))
        for (const layer of rows(LAYER, {providerId})
            .filter(row => row.disabled !== "Y")
            .slice(0, 100))
            for (const url of [layer.sourceUrl, layer.styleLocation].filter(outsideUrl))
                templateUrls(url).forEach(each => urls.add(each))
        const missing = cspBlocked(Array.from(urls), directives)
        const advice = missing
            .map(row =>
                say(
                    row.where === "header"
                        ? "${origins} to ${directive} of the screen-render Content-Security-Policy header"
                        : "${origins} to ${directive} with webapp@csp-append",
                    {
                        origins: row.origins.join(" "),
                        directive: row.directive
                    }
                )
            )
            .join("; ")
        out.push(
            missing.length
                ? check(
                      key + ".csp",
                      "Map provider and the page policy",
                      "failed",
                      "CSP_BLOCKS",
                      say("The page's Content-Security-Policy blocks ${name}: add ${advice}", {name, advice}),
                      fix
                  )
                : check(
                      key + ".csp",
                      "Map provider and the page policy",
                      "ready",
                      null,
                      say("The page's Content-Security-Policy lets a browser reach ${name}", {name}),
                      fix
                  )
        )
    }
    return out
}
function readiness(p) {
    const key = text(p.providerKey)
    if (key === "maps.profile") return {checks: profileChecks()}
    if (key === "maps.providers") return {checks: providerChecks()}
    throw new Error("Unknown Maps readiness check " + key)
}

const operations = {runMapProviderAdapter, readiness}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown map provider operation " + op)
        return operations[op](parameters || {})
    },
    // for the effective configuration (MapConfig.js)
    tested
})
