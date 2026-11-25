// The map configuration of org.moqui.maps.MapServices (plans/Maps.md §3.2, §4.2): the effective configuration m-map draws
// (webroot/screen/webroot/js/WebrootMap.js documents its shape), personal preferences and saved views.
//
// Precedence (§4.2): the installation's policy (maps@external-requests) constrains every choice; the consumer's profile
// (the map-view's profile) supplies the context; the user's allowed preferences override the profile's defaults (a
// profile's allowedOverrides says which: basemap, layers, extent, units); an explicit target (a record's place) overrides
// the initial viewport, and the map-view's own center, zoom and extent win over all of these in the browser
// (moqui.map.mergeConfig). The profile is the consumer's, else the user's own maps.profile preference, else their groups'
// (the ALL_USERS UserGroupPreference is the installation's default), else OUTLINE. A profile that is gone, disabled, not
// permitted (viewPermission) or asks an outside host while outside requests are off falls back to the next of those, with
// an explanation; an installation with no OUTLINE row or no maps.profile preference still draws the outline, and an
// administrator is told so, with the link to Configuration only for a user who may open it.
//
// What reaches a browser: no provider's key (a provider with one is not offered to a browser at all: its key would be
// in the tile and style URLs, and no provider kind of P1 has a key meant to be public; a proxied provider, which keeps the
// key on the server, is a later option), no service, entity or resource name, only paths of this server and outside URLs
// of an enabled provider that passed its test as it is configured now, on that provider's own origin (a layer with no
// provider, or a URL of another origin than its provider's, is a layer error), and with outside requests off only this
// server's. allowedOrigins are those origins, each tile subdomain ({s}, {a-c}) by itself. A profile or layer the caller
// may not see (viewPermission) is, to them, one that is not there, named by the ID asked for; a configurer is told. A record's place comes
// from its registered adapter, which checks the caller's access itself; a refusal and a failure are a named layer error,
// never a failed configuration, and a refusal does not say whether the record exists. A record layer's features are cut
// to the limit get#AdapterService gives (maps@max-features).
//
// Preferences are two UserPreference rows: maps.profile, the profile's ID (as the ALL_USERS default is, and as the
// map-view's print attribution reads it), and maps.preferences, one JSON value of the rest: {basemap, extent, units,
// restoreLastView, lastView, layers: {layerId: visible}}. A preference names only an enabled provider's layer and a
// profile the user is permitted, so it cannot turn on an unapproved provider. No location is collected: the last view
// is kept only when the user opted in, and turning that off clears it.
//
// Saved views (moqui.maps.MapSavedView, shaped as GridView): the owner's, and others' to read when shared; only the owner
// changes or deletes one. A layer a shared view names that the reader may not see is left out of what they read.
const PROFILE = "moqui.maps.MapProfile"
const PROFILE_LAYER = "moqui.maps.MapProfileLayer"
const LAYER = "moqui.maps.MapLayer"
const PROVIDER = "moqui.maps.MapProvider"
const SAVED_VIEW = "moqui.maps.MapSavedView"
const USER_PREFERENCE = "moqui.security.UserPreference"
const GROUP_PREFERENCE = "moqui.security.UserGroupPreference"
const OUTLINE_ID = "OUTLINE"
const PROFILE_KEY = "maps.profile"
const PREFERENCES_KEY = "maps.preferences"
const CONFIGURATION_SCREEN = "component://tools/screen/Maps/Configuration.xml"
const CONFIGURATION_URL = "/qapps/maps/Configuration"
const MAX_LAYERS = 50
const MAX_VIEWS = 500
const MAX_SHARED_VIEWS = 200
const WORLD = [-180, -85, 180, 85]
// the outline a map draws when no profile row can be used: the dashboard geomap's Natural Earth file, from this server
const BUILT_IN_OUTLINE = {
    id: "OUTLINE_NE110",
    label: "Country outlines",
    kind: "outline",
    url: "/libs/naturalearth/countries-110m.json",
    attribution: "Natural Earth",
    attributionUrls: ["https://www.naturalearthdata.com/"]
}
const BASEMAP_KINDS = {
    MLF_TOPOJSON: "outline",
    MLF_VECTOR_STYLE: "style",
    MLF_RASTER_XYZ: "xyz",
    MLF_PMTILES_VECTOR: "pmtiles",
    MLF_PMTILES_RASTER: "pmtiles"
}
const OVERLAY_KINDS = {
    MLF_GEOJSON: "geojson",
    MLF_TOPOJSON: "topojson",
    MLF_GPX: "gpx",
    MLF_KML: "kml",
    MLF_RASTER_XYZ: "xyz",
    MLF_PMTILES_VECTOR: "pmtiles",
    MLF_PMTILES_RASTER: "pmtiles",
    MLF_ADAPTER: "features"
}
const GEOMETRY_TYPES = ["Point", "MultiPoint", "LineString", "MultiLineString", "Polygon", "MultiPolygon"]
const OVERRIDES = ["basemap", "layers", "extent", "units"]

const L = value => ec.l10n.localize(value)
const say = (value, values) => ec.resource.expand(L(value), "", values || {})
const text = value => (value == null ? "" : String(value).trim())
const yes = value => value === true || value === "true" || value === "Y"
const find = (entity, conditions) =>
    ec.entity
        .find(entity)
        .condition(conditions || {})
        .useCache(true)
        .disableAuthz()
const one = (entity, conditions) => find(entity, conditions).one()
const rows = (entity, conditions) => Array.from(find(entity, conditions).list())
const offline = () => text(ec.factory.getConfValue("maps@external-requests")) === "false"
// Per call (a service loads this module once a call): whether the caller may open Configuration, and each provider's test
// state, read once.
let mayConfigure = null
const canConfigure = () => {
    if (mayConfigure === null)
        mayConfigure = !!ec.artifactExecution.isPermitted(CONFIGURATION_SCREEN, "AT_XML_SCREEN", "AUTHZA_VIEW")
    return mayConfigure
}
let providerKind = null
const testedProviders = new Map()
function providerTested(provider) {
    if (!testedProviders.has(provider.providerId)) {
        providerKind = providerKind || ec.resource.script("component://framework/service/org/moqui/maps/MapProviders.js")
        testedProviders.set(provider.providerId, !!providerKind.tested(provider))
    }
    return testedProviders.get(provider.providerId)
}

// ===== coordinates =====
function num(value) {
    if (value == null || value === "" || typeof value === "boolean") return null
    const found = typeof value === "number" ? value : Number(String(value).trim())
    return Number.isFinite(found) ? found : null
}
const lonOk = lon => lon !== null && lon >= -180 && lon <= 180
const latOk = lat => lat !== null && lat >= -90 && lat <= 90
// [west, south, east, north] from a list, a "w,s,e,n" text or {west, south, east, north}; null when it is not one in WGS84
// degrees (west greater than east crosses the antimeridian)
function extentOf(value) {
    if (value == null || value === "") return null
    const parts = Array.isArray(value)
        ? value
        : typeof value === "string"
          ? value.split(",")
          : typeof value === "object"
            ? [value.west, value.south, value.east, value.north]
            : null
    if (!parts || parts.length !== 4) return null
    const [west, south, east, north] = parts.map(num)
    return lonOk(west) && lonOk(east) && latOk(south) && latOk(north) && south <= north ? [west, south, east, north] : null
}
// [lon, lat] from a list, a "lon,lat" text or {lon, lat} / {longitude, latitude}
function centerOf(value) {
    if (value == null || value === "") return null
    const parts = Array.isArray(value)
        ? value
        : typeof value === "string"
          ? value.split(",")
          : typeof value === "object"
            ? [value.lon ?? value.longitude, value.lat ?? value.latitude]
            : null
    if (!parts || parts.length !== 2) return null
    const [lon, lat] = parts.map(num)
    return lonOk(lon) && latOk(lat) ? [lon, lat] : null
}
const zoomOf = value => {
    const zoom = num(value)
    return zoom !== null && zoom >= 0 && zoom <= 22 ? zoom : null
}
// A GeoJSON geometry of the types a map draws, its positions WGS84 [longitude, latitude]; null otherwise.
function geometryOf(value) {
    if (!value || typeof value !== "object" || !GEOMETRY_TYPES.includes(value.type)) return null
    const positionOk = position =>
        Array.isArray(position) && position.length >= 2 && lonOk(num(position[0])) && latOk(num(position[1]))
    const depth = {Point: 0, MultiPoint: 1, LineString: 1, MultiLineString: 2, Polygon: 2, MultiPolygon: 3}[value.type]
    const valid = (coordinates, level) =>
        level === 0
            ? positionOk(coordinates)
            : Array.isArray(coordinates) && coordinates.length > 0 && coordinates.every(item => valid(item, level - 1))
    return valid(value.coordinates, depth) ? {type: value.type, coordinates: value.coordinates} : null
}

// ===== URLs =====
// a path of this server: /..., never //host
const isLocal = url => /^\/(?!\/)/.test(text(url))
const isWeb = url => /^https?:\/\/[^\/?#@\s]+/i.test(text(url))
// scheme://host[:port], lower case, without the scheme's default port
function originOf(url) {
    const parts = /^(https?):\/\/([^\/?#@\s]+)/i.exec(text(url))
    if (!parts) return null
    const scheme = parts[1].toLowerCase()
    const host = parts[2].toLowerCase().replace(scheme === "https" ? /:443$/ : /:80$/, "")
    return scheme + "://" + host
}
// The concrete URLs of a URL or a tile template: {s} as a, b and c, {a-c} as each letter of the range, any other
// placeholder of the path ({z}, {fontstack}) as 0.
function templateUrls(url) {
    const value = text(url).replace(/\{[zxy]\}/g, "0")
    let list = [value]
    list = list.flatMap(each => (each.includes("{s}") ? ["a", "b", "c"].map(letter => each.split("{s}").join(letter)) : [each]))
    list = list.flatMap(each => {
        const range = /\{([a-z])-([a-z])\}/.exec(each)
        if (!range) return [each]
        const out = []
        for (let code = range[1].charCodeAt(0); code <= range[2].charCodeAt(0) && out.length < 26; code++)
            out.push(each.replace(range[0], String.fromCharCode(code)))
        return out
    })
    // a placeholder of the path or query ({fontstack}, {range}) is any value; one left in the host is no URL
    return list
        .map(each => {
            const authority = /^([a-z][a-z0-9+.-]*:\/\/[^\/?#]*)(.*)$/i.exec(each)
            return authority ? authority[1] + authority[2].replace(/\{[^{}\/?#]*\}/g, "0") : each.replace(/\{[^{}\/?#]*\}/g, "0")
        })
        .filter(each => each && !/[{}]/.test(each))
}
// the concrete origins of a URL or a tile template (each tile subdomain by itself)
const originsOf = url =>
    Array.from(
        new Set(
            templateUrls(url)
                .map(originOf)
                .filter(origin => origin)
        )
    )
const lines = value =>
    text(value)
        .split("\n")
        .map(line => line.trim())
        .filter(line => line)

// ===== providers, layers and profiles: what a user may be offered =====
const refusal = (reasonCode, reason) => ({ok: false, reasonCode, reason})
// Whether a provider's layers may be handed to this user's browser.
function providerRefusal(provider, providerId) {
    if (!provider) return refusal("PROVIDER_MISSING", say("its provider ${providerId} is not registered", {providerId}))
    const name = text(provider.providerName) || provider.providerId
    if (provider.disabled === "Y") return refusal("PROVIDER_DISABLED", say("its provider ${name} is not enabled", {name}))
    // a key is never sent to a browser, and no provider kind has one meant to be public
    if (text(provider.apiKey))
        return refusal("PROVIDER_KEYED", say("its provider ${name} needs a key, which is never sent to a browser", {name}))
    return null
}
// The URLs a layer's browser asks for: {url, styleUrl}, or a refusal.
function layerUrls(layer, provider, kind) {
    const format = text(layer.formatEnumId)
    let url = ""
    let styleUrl = ""
    if (format === "MLF_VECTOR_STYLE") url = text(layer.styleLocation) || text(provider?.endpointUrl)
    else if (format === "MLF_RASTER_XYZ") url = text(layer.sourceUrl) || text(provider?.endpointUrl)
    else if (format === "MLF_ADAPTER") return {url: "", styleUrl: ""}
    else {
        url = text(layer.sourceUrl)
        if (kind === "pmtiles") styleUrl = text(layer.styleLocation)
    }
    if (!url && text(layer.resourceLocation))
        return refusal("SOURCE_NOT_SERVED", L("its file is a registered resource, which this server does not hand to a map here"))
    if (!url) return refusal("SOURCE_MISSING", L("it names no source"))
    for (const each of [url, styleUrl].filter(value => value)) {
        if (!isLocal(each) && !isWeb(each))
            return refusal("URL_REFUSED", L("its address is not a path of this server or an http(s) URL"))
        // an archive is read by range from this server only
        if (kind === "pmtiles" && !isLocal(each)) return refusal("URL_REFUSED", L("its archive is not on this server"))
        if (isLocal(each)) continue
        if (offline())
            return refusal(
                "EXTERNAL_REQUESTS_OFF",
                L("it is drawn from a host outside this server, and outside requests are turned off (maps@external-requests)")
            )
        // an outside host only of an enabled provider, on its own origin, as its passing test saw it
        if (!provider)
            return refusal(
                "SOURCE_UNTESTED",
                L("it asks a host outside this server that is not an enabled, tested map provider's")
            )
        const own = new Set(originsOf(provider.endpointUrl))
        const asked = originsOf(each)
        if (!asked.length || !asked.every(origin => own.has(origin)))
            return refusal("URL_NOT_PROVIDERS", L("its address is not on its provider's host"))
        if (!providerTested(provider))
            return refusal(
                "PROVIDER_UNTESTED",
                say("its provider ${name} has not passed a test as it is configured now", {
                    name: text(provider.providerName) || provider.providerId
                })
            )
    }
    return {url, styleUrl}
}
// A layer as m-map draws it, or a refusal: {ok, entry, origins} | {ok: false, reasonCode, reason}.
function resolveLayer(layer, layerId, asBasemap) {
    if (!layer) return refusal("LAYER_MISSING", say("the layer ${layerId} is not registered", {layerId}))
    if (layer.disabled === "Y") return refusal("LAYER_DISABLED", L("the layer is disabled"))
    // one the caller may not see is, to them, one that is not there; a configurer is told
    if (text(layer.viewPermission) && !ec.user.hasPermission(layer.viewPermission))
        return canConfigure()
            ? refusal("LAYER_NOT_PERMITTED", L("the layer is not available to you"))
            : refusal("LAYER_MISSING", say("the layer ${layerId} is not registered", {layerId}))
    if (asBasemap && layer.isBasemap !== "Y") return refusal("KIND_UNSUPPORTED", L("it is not a basemap"))
    const format = text(layer.formatEnumId)
    const kind = (asBasemap ? BASEMAP_KINDS : OVERLAY_KINDS)[format]
    if (!kind)
        return refusal(
            "KIND_UNSUPPORTED",
            asBasemap
                ? L("it is not a kind of layer a map draws as its basemap")
                : L("it is not a kind of layer a map draws over its basemap")
        )
    if (kind === "features" && !text(layer.adapterKey)) return refusal("SOURCE_MISSING", L("it names no adapter"))
    const provider = text(layer.providerId) ? one(PROVIDER, {providerId: layer.providerId}) : null
    if (text(layer.providerId)) {
        const refused = providerRefusal(provider, layer.providerId)
        if (refused) return refused
    }
    const urls = layerUrls(layer, provider, kind)
    if (urls.ok === false) return urls
    const attribution = [provider?.attribution, layer.attribution].map(lines).flat()
    const entry = {
        id: layer.layerId,
        label: text(layer.layerName) ? L(layer.layerName) : layer.layerId,
        kind,
        url: urls.url,
        attribution: attribution.join("\n"),
        attributionUrls: lines(provider?.attributionUrls)
    }
    if (urls.styleUrl) entry.styleUrl = urls.styleUrl
    if (format === "MLF_PMTILES_RASTER") entry.raster = true
    if (num(layer.minZoom) !== null) entry.minZoom = num(layer.minZoom)
    if (num(layer.maxZoom) !== null) entry.maxZoom = num(layer.maxZoom)
    if (kind === "features") entry.adapterKey = text(layer.adapterKey)
    const origins = [urls.url, urls.styleUrl].flatMap(originsOf)
    return {ok: true, entry, origins}
}
const overridesOf = profile =>
    text(profile?.allowedOverrides)
        .split(",")
        .map(value => value.trim())
        .filter(value => OVERRIDES.includes(value))
// Whether a profile may be drawn for this user: {ok, basemap (resolved)} or a refusal.
function resolveProfile(profile, profileId) {
    if (!profile) return refusal("PROFILE_MISSING", say("the map profile ${profileId} is not registered", {profileId}))
    if (profile.disabled === "Y") return refusal("PROFILE_DISABLED", L("the map profile is disabled"))
    if (text(profile.viewPermission) && !ec.user.hasPermission(profile.viewPermission))
        return canConfigure()
            ? refusal("PROFILE_NOT_PERMITTED", L("the map profile is not available to you"))
            : refusal("PROFILE_MISSING", say("the map profile ${profileId} is not registered", {profileId}))
    if (profile.externalAccess === "Y" && offline())
        return refusal(
            "EXTERNAL_REQUESTS_OFF",
            L(
                "the map profile is drawn from a host outside this server, and outside requests are turned off (maps@external-requests)"
            )
        )
    const basemap = resolveLayer(one(LAYER, {layerId: profile.basemapLayerId}), text(profile.basemapLayerId) || "(none)", true)
    if (!basemap.ok)
        return refusal("BASEMAP_UNAVAILABLE", say("its basemap is not available: ${reason}", {reason: basemap.reason}))
    return {ok: true, basemap}
}
const profileName = (profile, profileId) => (profile && text(profile.profileName) ? L(profile.profileName) : profileId)

// ===== preferences =====
function ownPreference(key) {
    const userId = ec.user.userId
    if (!userId) return ""
    return text(ec.entity.find(USER_PREFERENCE).condition({userId, preferenceKey: key}).disableAuthz().one()?.preferenceValue)
}
const installationProfileId = () =>
    text(
        ec.entity.find(GROUP_PREFERENCE).condition({userGroupId: "ALL_USERS", preferenceKey: PROFILE_KEY}).disableAuthz().one()
            ?.preferenceValue
    )
// maps.preferences, the user's or else their groups' (an installation may seed an ALL_USERS default), as an object
function storedPreferences() {
    const raw = text(ec.user.getPreference(PREFERENCES_KEY))
    if (!raw) return {}
    try {
        const parsed = JSON.parse(raw)
        return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? parsed : {}
    } catch (error) {
        ec.logger.warn("The map preferences of user " + ec.user.userId + " are not JSON: they are passed over")
        return {}
    }
}

// maps.preferences, the user's own row only: what store#Preferences changes, never their groups' default copied in
function ownPreferences() {
    const raw = ownPreference(PREFERENCES_KEY)
    if (!raw) return {}
    try {
        const parsed = JSON.parse(raw)
        return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? parsed : {}
    } catch (error) {
        return {}
    }
}

// ===== get#EffectiveConfig =====
function probe(serviceName, parameters) {
    ec.message.pushErrors()
    try {
        const out = ec.service.sync().name(serviceName).parameters(parameters).requireNewTransaction(true).call() || {}
        if (ec.message.hasError()) return {failed: true, error: ec.message.getErrorsString()}
        return {failed: false, out}
    } catch (error) {
        return {failed: true, error: String(error?.message ?? error)}
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
}
function recordKeyOf(value) {
    let found = value
    if (typeof found === "string") {
        if (!found.trim()) return null
        try {
            found = JSON.parse(found)
        } catch (error) {
            return undefined
        }
    }
    if (!found || typeof found !== "object" || Array.isArray(found)) return undefined
    const keys = Object.keys(found)
    if (!keys.length || keys.length > 10) return undefined
    for (const key of keys)
        if (!/^[A-Za-z][A-Za-z0-9_]{0,63}$/.test(key) || !["string", "number"].includes(typeof found[key])) return undefined
    return found
}

function effectiveConfig(p) {
    const configurer = canConfigure()
    const fallbacks = []
    const adminNotes = []
    const layerErrors = []
    const origins = new Set()
    const preferences = storedPreferences()

    // ----- the profile: the consumer's, the user's own, their groups', the installation's, OUTLINE -----
    const candidates = []
    const add = (profileId, source) => {
        if (profileId && !candidates.some(row => row.profileId === profileId)) candidates.push({profileId, source})
    }
    add(text(p.profileId), "consumer")
    add(ownPreference(PROFILE_KEY), "preference")
    add(text(ec.user.getPreference(PROFILE_KEY)), "group")
    const installed = installationProfileId()
    if (!installed)
        adminNotes.push(
            L("No installation default map profile is set (the ALL_USERS preference maps.profile): the outline is drawn.")
        )
    add(installed, "installation")
    add(OUTLINE_ID, "outline")
    let chosen = null
    const passed = []
    for (const candidate of candidates) {
        const profile = one(PROFILE, {profileId: candidate.profileId})
        const resolved = resolveProfile(profile, candidate.profileId)
        if (resolved.ok) {
            chosen = Object.assign({profile, resolved}, candidate)
            break
        }
        passed.push(Object.assign({profile, refused: resolved}, candidate))
    }
    const chosenName = chosen ? profileName(chosen.profile, chosen.profileId) : L("Outline")
    for (const row of passed) {
        // one the caller may not see is named by the ID asked for, as one that is not there
        const name = row.refused.reasonCode === "PROFILE_MISSING" ? row.profileId : profileName(row.profile, row.profileId)
        if (row.source === "outline") {
            adminNotes.push(
                say("The outline profile (OUTLINE) is not usable (${reason}): the built-in outline is drawn.", {
                    reason: row.refused.reason
                })
            )
            continue
        }
        const message =
            row.source === "preference"
                ? say("Your map profile ${name} is not available (${reason}): ${shown} is shown.", {
                      name,
                      reason: row.refused.reason,
                      shown: chosenName
                  })
                : row.source === "consumer"
                  ? say("The map profile ${name} is not available (${reason}): ${shown} is shown.", {
                        name,
                        reason: row.refused.reason,
                        shown: chosenName
                    })
                  : say("The default map profile ${name} is not available (${reason}): ${shown} is shown.", {
                        name,
                        reason: row.refused.reason,
                        shown: chosenName
                    })
        fallbacks.push({kind: "profile", id: row.profileId, reasonCode: row.refused.reasonCode, message})
    }
    const profile = chosen?.profile || null
    const overrides = profile ? overridesOf(profile) : []
    const allowed = name => overrides.includes(name)

    // ----- the basemaps: the profile's, and with the basemap override every other one this user may be offered -----
    const basemaps = []
    if (chosen) {
        basemaps.push(chosen.resolved.basemap.entry)
        chosen.resolved.basemap.origins.forEach(origin => origins.add(origin))
        if (allowed("basemap")) {
            const others = Array.from(find(LAYER, {isBasemap: "Y"}).orderBy("layerName").limit(100).list())
            for (const layer of others) {
                if (layer.layerId === profile.basemapLayerId || layer.disabled === "Y") continue
                const resolved = resolveLayer(layer, layer.layerId, true)
                if (!resolved.ok) continue
                basemaps.push(resolved.entry)
                resolved.origins.forEach(origin => origins.add(origin))
                if (basemaps.length >= 20) break
            }
        }
    } else basemaps.push(BUILT_IN_OUTLINE)
    let basemap = basemaps[0].id
    const preferredBasemap = text(preferences.basemap)
    if (preferredBasemap && allowed("basemap")) {
        if (basemaps.some(entry => entry.id === preferredBasemap)) basemap = preferredBasemap
        else {
            const resolved = resolveLayer(one(LAYER, {layerId: preferredBasemap}), preferredBasemap, true)
            fallbacks.push({
                kind: "basemap",
                id: preferredBasemap,
                reasonCode: resolved.ok ? "BASEMAP_UNAVAILABLE" : resolved.reasonCode,
                message: say("Your basemap ${id} is not available (${reason}): the profile's is shown.", {
                    id: preferredBasemap,
                    reason: resolved.ok ? L("the profile does not offer it") : resolved.reason
                })
            })
        }
    }

    // ----- the overlay layers: those asked for, else the profile's, with the user's visibility where allowed -----
    const shownLayers = []
    const adapterLayers = []
    const visibility = preferences.layers && typeof preferences.layers === "object" ? preferences.layers : {}
    const asked = Array.from(
        new Set(
            text(p.layerIds)
                .split(",")
                .map(id => id.trim())
                .filter(id => id)
        )
    ).slice(0, MAX_LAYERS)
    const wanted = asked.length
        ? asked.map(layerId => ({layerId, visible: true, asked: true}))
        : profile
          ? rows(PROFILE_LAYER, {profileId: profile.profileId})
                .sort((a, b) => (num(a.sequenceNum) ?? 999) - (num(b.sequenceNum) ?? 999))
                .slice(0, MAX_LAYERS)
                .map(row => ({layerId: row.layerId, visible: row.defaultVisible !== "N", asked: false}))
          : []
    if (allowed("layers"))
        for (const row of wanted) if (Object.hasOwn(visibility, row.layerId)) row.visible = yes(visibility[row.layerId])
    for (const row of wanted) {
        const layer = one(LAYER, {layerId: row.layerId})
        const resolved = resolveLayer(layer, row.layerId, false)
        if (!resolved.ok) {
            const label =
                layer && resolved.reasonCode !== "LAYER_MISSING" && text(layer.layerName) ? L(layer.layerName) : row.layerId
            layerErrors.push({id: row.layerId, label, reasonCode: resolved.reasonCode, message: resolved.reason})
            if (!row.asked)
                fallbacks.push({
                    kind: "layer",
                    id: row.layerId,
                    reasonCode: resolved.reasonCode,
                    message: say("The layer ${label} is not shown: ${reason}.", {label, reason: resolved.reason})
                })
            continue
        }
        resolved.origins.forEach(origin => origins.add(origin))
        if (resolved.entry.kind === "features") adapterLayers.push(Object.assign({visible: row.visible}, resolved.entry))
        else shownLayers.push(Object.assign({visible: row.visible}, resolved.entry))
    }
    // a layer the user's preferences name that is gone or not permitted: said once, and left out
    if (allowed("layers"))
        for (const layerId of Object.keys(visibility).slice(0, MAX_LAYERS)) {
            if (wanted.some(row => row.layerId === layerId)) continue
            const layer = one(LAYER, {layerId})
            const resolved = layer ? resolveLayer(layer, layerId, false) : refusal("LAYER_MISSING", L("it is not registered"))
            if (!resolved.ok && resolved.reasonCode !== "KIND_UNSUPPORTED")
                fallbacks.push({
                    kind: "layer",
                    id: layerId,
                    reasonCode: resolved.reasonCode,
                    message: say("A layer of your preferences, ${layerId}, is not available: ${reason}.", {
                        layerId,
                        reason: resolved.reason
                    })
                })
        }

    // ----- the viewport: the profile's, the user's extent, the last view (when kept), then the record's place -----
    const config = {profileId: chosen ? chosen.profileId : OUTLINE_ID}
    if (profile) {
        const center = centerOf([profile.centerLongitude, profile.centerLatitude])
        if (center) config.center = center
        if (zoomOf(profile.zoom) !== null) config.zoom = zoomOf(profile.zoom)
        const extent = extentOf([profile.minLongitude, profile.minLatitude, profile.maxLongitude, profile.maxLatitude])
        if (extent && !center) config.extent = extent
    }
    const setView = view => {
        delete config.center
        delete config.zoom
        delete config.extent
        Object.assign(config, view)
    }
    if (allowed("extent")) {
        const extent = extentOf(preferences.extent)
        if (extent) setView({extent})
        const last = preferences.lastView
        if (yes(preferences.restoreLastView) && last && typeof last === "object") {
            const center = centerOf(last.center)
            if (center) setView(Object.assign({center}, zoomOf(last.zoom) !== null ? {zoom: zoomOf(last.zoom)} : {}))
        }
    }
    if (allowed("units") && ["metric", "imperial"].includes(text(preferences.units))) config.units = text(preferences.units)
    else if (profile && ["metric", "imperial"].includes(text(profile.displayUnits))) config.units = text(profile.displayUnits)

    // ----- a record's place, through its registered adapter (its own access check) -----
    const points = []
    const features = []
    let record = null
    const adapterKey = text(p.adapterKey)
    if (adapterKey) {
        const recordKey = recordKeyOf(p.recordKey)
        const recordError = (reasonCode, message, label) =>
            layerErrors.push({id: "record", label: label || L("The record"), reasonCode, message})
        // a key that is no registered adapter's shape is not looked up, nor written to the log
        if (!/^[A-Za-z0-9_.-]{1,60}$/.test(adapterKey))
            recordError("ADAPTER_UNAVAILABLE", L("This record's place cannot be shown here"))
        else if (!recordKey) recordError("RECORD_KEY_INVALID", L("The record's identity is not a set of key fields"))
        else {
            const resolved = probe("org.moqui.maps.MapServices.get#AdapterService", {adapterKey, operation: "context"})
            if (resolved.failed) {
                ec.logger.warn("Map adapter " + adapterKey + " is not available for a record's place: " + resolved.error)
                recordError("ADAPTER_UNAVAILABLE", L("This record's place cannot be shown here"))
            } else {
                const label = text(resolved.out.adapter?.description) || adapterKey
                const answered = probe(resolved.out.serviceName, {recordKey, addressPurpose: text(p.addressPurpose) || null})
                const out = answered.out || {}
                if (answered.failed) {
                    // an adapter the user may not call is as a record they may not read: nothing says which. Its own
                    // error is its service's, logged there; this names the adapter only, never what it said
                    const authz = /not authorized|authoriz/i.test(answered.error)
                    if (!authz) ec.logger.error("Map adapter " + adapterKey + " failed reading a record's place")
                    if (authz) recordError("RECORD_UNAVAILABLE", L("This record's place is not available"), label)
                    else recordError("ADAPTER_FAILED", L("This record's place could not be read"), label)
                } else if (text(out.reasonCode)) {
                    // PERMISSION_DENIED and RECORD_UNAVAILABLE alike: the adapter alone tells them apart
                    recordError("RECORD_UNAVAILABLE", L("This record's place is not available"), label)
                } else {
                    const geometry = out.geometry == null ? null : geometryOf(out.geometry)
                    const extent = extentOf(out.extent)
                    if (out.geometry != null && !geometry)
                        ec.logger.warn("Map adapter " + adapterKey + " answered a geometry that is not WGS84 GeoJSON: left out")
                    record = {
                        adapterKey,
                        label: text(out.label),
                        precisionEnumId: text(out.precisionEnumId) || null,
                        sourceText: text(out.sourceText) || null,
                        recordVersion: text(out.recordVersion) || null,
                        permittedActions: Array.from(out.permittedActions || [])
                            .map(text)
                            .filter(action => ["view", "chooseLocation"].includes(action)),
                        links: Array.from(out.links || [])
                            .filter(link => link && typeof link === "object" && text(link.route))
                            .slice(0, 20)
                            .map(link => ({label: text(link.label), route: text(link.route), parameters: link.parameters || {}})),
                        located: !!geometry
                    }
                    if (geometry && geometry.type === "Point") {
                        const [lon, lat] = geometry.coordinates.map(num)
                        const point = {id: "record", lon, lat}
                        if (record.label) point.label = record.label
                        if (record.precisionEnumId) point.precisionEnumId = record.precisionEnumId
                        if (record.sourceText) point.source = record.sourceText
                        points.push(point)
                        // a known point is zoomed to; an approximate one (a postal area) shows its extent
                        setView(extent ? {extent} : {center: [lon, lat], zoom: 15})
                    } else if (geometry) {
                        features.push({
                            type: "Feature",
                            id: "record",
                            geometry,
                            properties: {label: record.label, precisionEnumId: record.precisionEnumId}
                        })
                        if (extent) setView({extent})
                    } else if (extent) setView({extent})
                }
            }
        }
    }

    // ----- record layers: each adapter's features for the initial view, cut to its limit -----
    let truncated = false
    const recordLayers = []
    for (const layer of adapterLayers) {
        recordLayers.push({id: layer.id, label: layer.label, visible: layer.visible, adapterKey: layer.adapterKey})
        // a hidden layer's adapter is not asked until it is shown
        if (!layer.visible) continue
        const resolved = probe("org.moqui.maps.MapServices.get#AdapterService", {
            adapterKey: layer.adapterKey,
            operation: "features"
        })
        if (resolved.failed) {
            ec.logger.warn("Map layer " + layer.id + ": its adapter " + layer.adapterKey + " is not available: " + resolved.error)
            layerErrors.push({
                id: layer.id,
                label: layer.label,
                reasonCode: "ADAPTER_UNAVAILABLE",
                message: L("This layer cannot be shown here")
            })
            continue
        }
        const limit = Number(resolved.out.limit) || 0
        const answered = probe(resolved.out.serviceName, {
            layerId: layer.id,
            extent: (([west, south, east, north]) => ({west, south, east, north}))(config.extent || WORLD),
            zoom: config.zoom ?? null,
            limit
        })
        if (answered.failed) {
            ec.logger.error("Map layer " + layer.id + ": its adapter " + layer.adapterKey + " failed reading its features")
            layerErrors.push({
                id: layer.id,
                label: layer.label,
                reasonCode: "ADAPTER_FAILED",
                message: L("This layer's features could not be read")
            })
            continue
        }
        const given = Array.from(answered.out.features || [])
        if (given.length > limit || answered.out.truncated === true) truncated = true
        for (const feature of given.slice(0, limit)) {
            const geometry = feature && typeof feature === "object" ? geometryOf(feature.geometry) : null
            if (!geometry) continue
            features.push({
                type: "Feature",
                id: layer.id + ":" + text(feature.featureId),
                geometry,
                properties: {
                    label: text(feature.label),
                    layerId: layer.id,
                    precisionEnumId: text(feature.precisionEnumId) || null
                }
            })
        }
    }

    // ----- the notice: the fallbacks for everyone, an administrator's notes and the way to Configuration -----
    if (configurer && config.profileId === OUTLINE_ID && !adminNotes.length)
        adminNotes.push(L("Only country outlines are drawn: no basemap is configured for this map."))
    const noticeParts = fallbacks.map(row => row.message).concat(configurer ? adminNotes : [])
    const notice = noticeParts.length
        ? {
              text: noticeParts.join(" "),
              href: configurer ? CONFIGURATION_URL : "",
              linkText: configurer ? L("Maps configuration") : ""
          }
        : null

    config.profile = {
        profileId: config.profileId,
        profileName: chosenName,
        basemaps,
        basemap,
        allowedOrigins: Array.from(origins).sort(),
        notice
    }
    config.layers = shownLayers
    // the record layers, drawn as features: each with whether it is shown (a hidden one's adapter was not asked)
    if (recordLayers.length) config.recordLayers = recordLayers
    config.points = points
    if (features.length) config.features = {type: "FeatureCollection", features}
    if (truncated) config.featuresTruncated = true
    if (record) config.record = record
    config.fallbacks = fallbacks
    config.layerErrors = layerErrors
    config.externalRequests = !offline()
    return {config}
}

// ===== get#Preferences and store#Preferences =====
function profileOptions() {
    return Array.from(find(PROFILE, {}).orderBy("profileName").limit(200).list())
        .filter(profile => resolveProfile(profile, profile.profileId).ok)
        .map(profile => ({
            profileId: profile.profileId,
            profileName: profileName(profile, profile.profileId),
            description: text(profile.description) ? L(profile.description) : "",
            externalAccess: profile.externalAccess === "Y",
            allowedOverrides: overridesOf(profile)
        }))
}
function basemapOptions() {
    return Array.from(find(LAYER, {isBasemap: "Y"}).orderBy("layerName").limit(200).list())
        .map(layer => resolveLayer(layer, layer.layerId, true))
        .filter(resolved => resolved.ok)
        .map(resolved => ({layerId: resolved.entry.id, label: resolved.entry.label, kind: resolved.entry.kind}))
}
function getPreferences() {
    if (!ec.user.userId) throw new Error(L("Sign in to read map preferences"))
    const stored = storedPreferences()
    const layers = stored.layers && typeof stored.layers === "object" ? stored.layers : {}
    return {
        preferences: {
            profileId: ownPreference(PROFILE_KEY) || null,
            basemap: text(stored.basemap) || null,
            extent: extentOf(stored.extent),
            units: ["metric", "imperial"].includes(text(stored.units)) ? text(stored.units) : null,
            restoreLastView: yes(stored.restoreLastView),
            lastView: yes(stored.restoreLastView) && stored.lastView && centerOf(stored.lastView.center) ? stored.lastView : null,
            layers: Object.fromEntries(Object.keys(layers).map(layerId => [layerId, yes(layers[layerId])]))
        },
        installationProfileId: installationProfileId() || null,
        profileOptions: profileOptions(),
        basemapOptions: basemapOptions()
    }
}
const refused = (reasonCode, reason, extra) => Object.assign({applied: false, reasonCode, reason}, extra || {})
const given = value => value !== undefined && value !== null
function storePreferences(p) {
    if (!ec.user.userId) throw new Error(L("Sign in to change map preferences"))
    const stored = ownPreferences()
    const next = Object.assign({}, stored)
    // the profile: one the user may be offered, whose basemap's provider is enabled
    let profileId = null
    if (given(p.profileId)) {
        profileId = text(p.profileId)
        if (profileId) {
            const resolved = resolveProfile(one(PROFILE, {profileId}), profileId)
            if (!resolved.ok)
                return refused(
                    "PROFILE_UNAVAILABLE",
                    say("The map profile ${profileId} cannot be chosen: ${reason}", {profileId, reason: resolved.reason}),
                    {cause: resolved.reasonCode}
                )
        }
    }
    if (given(p.basemap)) {
        const layerId = text(p.basemap)
        if (layerId) {
            const resolved = resolveLayer(one(LAYER, {layerId}), layerId, true)
            if (!resolved.ok)
                return refused(
                    "BASEMAP_UNAVAILABLE",
                    say("The basemap ${layerId} cannot be chosen: ${reason}", {layerId, reason: resolved.reason}),
                    {cause: resolved.reasonCode}
                )
            next.basemap = layerId
        } else delete next.basemap
    }
    if (given(p.extent)) {
        if (p.extent === "" || (Array.isArray(p.extent) && !p.extent.length)) delete next.extent
        else {
            const extent = extentOf(p.extent)
            if (!extent) throw new Error(L("The initial extent is west, south, east and north in degrees"))
            next.extent = extent
        }
    }
    if (given(p.units)) {
        const units = text(p.units)
        if (units && !["metric", "imperial"].includes(units)) throw new Error(L("The units are metric or imperial"))
        if (units) next.units = units
        else delete next.units
    }
    if (given(p.layers)) {
        const layers = p.layers && typeof p.layers === "object" && !Array.isArray(p.layers) ? p.layers : null
        if (!layers) throw new Error(L("The layers are each layer's ID with whether it is shown"))
        const ids = Object.keys(layers)
        if (ids.length > MAX_LAYERS) throw new Error(say("At most ${count} layers", {count: MAX_LAYERS}))
        for (const layerId of ids) {
            const resolved = resolveLayer(one(LAYER, {layerId}), layerId, false)
            if (!resolved.ok)
                return refused(
                    "LAYER_UNAVAILABLE",
                    say("The layer ${layerId} cannot be chosen: ${reason}", {layerId, reason: resolved.reason}),
                    {cause: resolved.reasonCode}
                )
        }
        next.layers = Object.fromEntries(ids.map(layerId => [layerId, yes(layers[layerId])]))
    }
    // the last view is kept only while the user wants it restored; turning that off clears it
    if (given(p.restoreLastView)) next.restoreLastView = yes(p.restoreLastView)
    if (!yes(next.restoreLastView)) {
        delete next.lastView
        delete next.restoreLastView
    } else if (given(p.lastView)) {
        const last = p.lastView && typeof p.lastView === "object" ? p.lastView : null
        const center = last ? centerOf(last.center) : null
        if (!center) throw new Error(L("The last view is a center in degrees and a zoom"))
        next.lastView = Object.assign({center}, zoomOf(last.zoom) !== null ? {zoom: zoomOf(last.zoom)} : {})
    }

    if (given(p.profileId)) {
        if (profileId) ec.user.setPreference(PROFILE_KEY, profileId)
        else
            ec.entity
                .find(USER_PREFERENCE)
                .condition({userId: ec.user.userId, preferenceKey: PROFILE_KEY})
                .disableAuthz()
                .deleteAll()
    }
    if (JSON.stringify(next) !== JSON.stringify(stored)) ec.user.setPreference(PREFERENCES_KEY, JSON.stringify(next))
    return {applied: true, reasonCode: null, reason: null}
}

// ===== saved views =====
// A view's JSON as stored: its center and zoom or its extent, projection, basemap and layers, each checked for the owner.
function normalizedView(value) {
    let view = value
    if (typeof view === "string") {
        try {
            view = JSON.parse(view)
        } catch (error) {
            throw new Error(L("The view is not JSON"))
        }
    }
    if (!view || typeof view !== "object" || Array.isArray(view))
        throw new Error(L("The view is a map of its center, zoom, extent and layers"))
    const out = {}
    if (given(view.extent) && view.extent !== "") {
        out.extent = extentOf(view.extent)
        if (!out.extent) throw new Error(L("The view's extent is west, south, east and north in degrees"))
    }
    if (given(view.center) && view.center !== "") {
        out.center = centerOf(view.center)
        if (!out.center) throw new Error(L("The view's center is a longitude and a latitude in degrees"))
    }
    if (given(view.zoom) && view.zoom !== "") {
        out.zoom = zoomOf(view.zoom)
        if (out.zoom === null) throw new Error(L("The view's zoom is from 0 to 22"))
    }
    if (!out.extent && !out.center) throw new Error(L("A saved view has a center or an extent"))
    const projection = text(view.projection).toUpperCase()
    if (projection) {
        if (!["EPSG:3857", "EPSG:4326"].includes(projection))
            throw new Error(L("The view's projection is EPSG:3857 or EPSG:4326"))
        out.projection = projection
    }
    const layers = view.layers == null ? [] : Array.isArray(view.layers) ? view.layers : text(view.layers).split(",")
    out.layers = Array.from(new Set(layers.map(text).filter(id => id)))
    if (out.layers.length > MAX_LAYERS) throw new Error(say("A saved view names at most ${count} layers", {count: MAX_LAYERS}))
    if (text(view.basemap)) out.basemap = text(view.basemap)
    return out
}
function savedViewRefusal(row) {
    if (!row || (row.ownerUserId !== ec.user.userId && row.isShared !== "Y"))
        return refused("RECORD_UNAVAILABLE", L("The saved view is not available"))
    if (row.ownerUserId !== ec.user.userId) return refused("PERMISSION_DENIED", L("Only its owner changes a saved view"))
    return null
}
const savedViewRow = savedViewId =>
    text(savedViewId)
        ? ec.entity
              .find(SAVED_VIEW)
              .condition({savedViewId: text(savedViewId)})
              .disableAuthz()
              .forUpdate(true)
              .one()
        : null
function storeSavedView(p) {
    if (!ec.user.userId) throw new Error(L("Sign in to save a map view"))
    const savedViewId = text(p.savedViewId)
    const row = savedViewId ? savedViewRow(savedViewId) : null
    if (savedViewId) {
        const refusal = savedViewRefusal(row)
        if (refusal) return Object.assign(refusal, {savedViewId})
    }
    const description = text(p.description) || text(row?.description)
    if (!description) throw new Error(L("Give the saved view a name"))
    if (description.length > 255 || /[\u0000-\u001f]/.test(description))
        throw new Error(L("The name is one line of at most 255 characters"))
    const profileId = given(p.profileId) ? text(p.profileId) : text(row?.profileId)
    if (profileId) {
        const resolved = resolveProfile(one(PROFILE, {profileId}), profileId)
        if (!resolved.ok)
            return refused(
                "PROFILE_UNAVAILABLE",
                say("The map profile ${profileId} cannot be saved with a view: ${reason}", {profileId, reason: resolved.reason}),
                {
                    cause: resolved.reasonCode,
                    savedViewId: savedViewId || null
                }
            )
    }
    let viewJson = row?.viewJson || null
    if (given(p.view) || !row) {
        const view = normalizedView(p.view)
        for (const layerId of view.layers) {
            const resolved = resolveLayer(one(LAYER, {layerId}), layerId, false)
            if (!resolved.ok)
                return refused(
                    "LAYER_UNAVAILABLE",
                    say("The layer ${layerId} cannot be saved with a view: ${reason}", {layerId, reason: resolved.reason}),
                    {
                        cause: resolved.reasonCode,
                        savedViewId: savedViewId || null
                    }
                )
        }
        if (view.basemap) {
            const resolved = resolveLayer(one(LAYER, {layerId: view.basemap}), view.basemap, true)
            if (!resolved.ok)
                return refused(
                    "BASEMAP_UNAVAILABLE",
                    say("The basemap ${layerId} cannot be saved with a view: ${reason}", {
                        layerId: view.basemap,
                        reason: resolved.reason
                    }),
                    {
                        cause: resolved.reasonCode,
                        savedViewId: savedViewId || null
                    }
                )
        }
        viewJson = JSON.stringify(view)
    }
    const isShared = given(p.isShared) ? (yes(p.isShared) ? "Y" : "N") : row?.isShared === "Y" ? "Y" : "N"
    const value = row || ec.entity.makeValue(SAVED_VIEW)
    if (!row) {
        const count = Number(ec.entity.find(SAVED_VIEW).condition({ownerUserId: ec.user.userId}).disableAuthz().count())
        if (count >= MAX_VIEWS) return refused("TOO_MANY_VIEWS", say("You have ${count} saved views: delete one first", {count}))
        value.setSequencedIdPrimary()
        value.set("ownerUserId", ec.user.userId)
    }
    value.set("description", description)
    value.set("isShared", isShared)
    value.set("profileId", profileId || null)
    value.set("viewJson", viewJson)
    const write = ec.artifactExecution.disableAuthz()
    try {
        if (row) value.update()
        else value.create()
    } finally {
        if (!write) ec.artifactExecution.enableAuthz()
    }
    return {applied: true, reasonCode: null, reason: null, savedViewId: String(value.savedViewId)}
}
function deleteSavedView(p) {
    if (!ec.user.userId) throw new Error(L("Sign in to delete a map view"))
    const savedViewId = text(p.savedViewId)
    const row = savedViewRow(savedViewId)
    const refusal = savedViewRefusal(row)
    if (refusal) return Object.assign(refusal, {savedViewId})
    ec.entity.find(SAVED_VIEW).condition({savedViewId}).disableAuthz().deleteAll()
    return {applied: true, reasonCode: null, reason: null, savedViewId}
}
function findSavedViews(p) {
    if (!ec.user.userId) throw new Error(L("Sign in to read saved map views"))
    const userId = ec.user.userId
    // the user's own first, each list with its own cap, so others' shared views never crowd out the user's
    const listOf = (conditions, cap) => {
        let found = ec.entity.find(SAVED_VIEW).condition(conditions).disableAuthz()
        if (text(p.profileId)) found = found.condition({profileId: text(p.profileId)})
        return Array.from(found.orderBy("description").limit(cap).list())
    }
    const cf = ec.entity.conditionFactory
    const views = listOf({ownerUserId: userId}, MAX_VIEWS).concat(
        listOf(
            cf.makeCondition(
                [cf.makeCondition("isShared", "equals", "Y"), cf.makeCondition("ownerUserId", "not-equals", userId)],
                "and"
            ),
            MAX_SHARED_VIEWS
        )
    )
    const owners = new Map()
    const ownerName = ownerUserId => {
        if (!owners.has(ownerUserId)) {
            const account = ec.entity
                .find("moqui.security.UserAccount")
                .condition({userId: ownerUserId})
                .useCache(true)
                .disableAuthz()
                .one()
            owners.set(ownerUserId, account ? text(account.userFullName) || text(account.username) : ownerUserId)
        }
        return owners.get(ownerUserId)
    }
    const savedViewList = []
    for (const row of views) {
        // one view that does not read never takes the list with it
        try {
            const view = row.viewJson ? JSON.parse(row.viewJson) : {}
            const layers = Array.isArray(view.layers) ? view.layers.map(text) : []
            const shown = layers.filter(layerId => resolveLayer(one(LAYER, {layerId}), layerId, false).ok)
            view.layers = shown
            if (view.basemap && !resolveLayer(one(LAYER, {layerId: view.basemap}), view.basemap, true).ok) delete view.basemap
            const left = layers.length - shown.length
            savedViewList.push({
                savedViewId: row.savedViewId,
                description: text(row.description),
                isShared: row.isShared === "Y",
                isOwn: row.ownerUserId === userId,
                ownerName: ownerName(row.ownerUserId),
                profileId: row.profileId || null,
                view,
                explanation: left
                    ? say(
                          left === 1
                              ? "${count} layer of this view is not available and is left out"
                              : "${count} layers of this view are not available and are left out",
                          {
                              count: left
                          }
                      )
                    : null
            })
        } catch (error) {
            ec.logger.error("Saved map view " + row.savedViewId + " does not read: " + (error?.message ?? error))
        }
    }
    return {savedViewList}
}

const operations = {effectiveConfig, getPreferences, storePreferences, storeSavedView, deleteSavedView, findSavedViews}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown map configuration operation " + op)
        return operations[op](parameters || {})
    },
    // for the readiness checks and the provider kind (MapProviders.js)
    resolveProfile,
    resolveLayer,
    installationProfileId,
    originOf,
    templateUrls,
    OUTLINE_ID
})
