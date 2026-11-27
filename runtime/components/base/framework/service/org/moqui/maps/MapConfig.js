// The map configuration of org.moqui.maps.MapServices (plans/archive/Maps.md §3.2, §4.2): the effective configuration m-map draws
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
// What reaches a browser: no provider's key (a provider with one is offered only through this server's proxy, its
// proxyRequests Y: its key would be in the tile and style URLs, and no provider kind has a key meant to be public; a
// proxied provider's layers are handed the proxy's same-origin paths, LocalMap.xml providerProxy, only to a caller the
// proxy lets reach it, MapProviders.js proxyOffered), no service, entity or resource name, only paths of this server and outside URLs
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
// Projections (P4c, moqui.maps.MapProjection): a profile's view is drawn in EPSG:3857 unless it names another
// (viewProjection: EPSG:4326 or a registered row), and a basemap of vector tiles (a MapLibre style, a vector PMTiles archive),
// which a map cannot reproject, is drawn only in EPSG:3857: a profile with one in another view is refused with that reason,
// and the other basemaps offered are those its view can draw. The configuration hands the browser the rows of the
// projections it needs (its view's, its COG layers'), whose PROJ.4 text m-map registers with proj4js: no definition is
// fetched from anywhere else. A Cloud Optimized GeoTIFF layer (MLF_COG) carries its header's bands and its band settings:
// the user's own (maps.preferences rasters, by layer), else the layer's defaults (rasterJson).
//
// Saved views (moqui.maps.MapSavedView, shaped as GridView): the owner's, and others' to read when shared; only the owner
// changes or deletes one. A layer a shared view names that the reader may not see is left out of what they read; a view
// may name a user's overlay (overlay:ID), which a reader sees only while its file is readable to them.
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
    MLF_PMTILES_RASTER: "pmtiles",
    MLF_WMS: "wms",
    MLF_WMTS: "wmts",
    MLF_COG: "cog"
}
const OVERLAY_KINDS = {
    MLF_GEOJSON: "geojson",
    MLF_TOPOJSON: "topojson",
    MLF_GPX: "gpx",
    MLF_KML: "kml",
    MLF_RASTER_XYZ: "xyz",
    MLF_PMTILES_VECTOR: "pmtiles",
    MLF_PMTILES_RASTER: "pmtiles",
    MLF_ADAPTER: "features",
    MLF_WMS: "wms",
    MLF_WMTS: "wmts",
    MLF_WFS: "wfs",
    MLF_COG: "cog"
}
const PROJECTION = "moqui.maps.MapProjection"
const WEB_MERCATOR = "EPSG:3857"
const BUILT_IN_PROJECTIONS = ["EPSG:3857", "EPSG:4326"]
const COLORMAPS = ["grey", "viridis", "terrain", "magma"]
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
// state, read once. The caller's own grants (isPermittedOwn): an app root's inheritable grant over the screen a map is shown
// under (an app's LocalMap mount) never makes its user a Maps configurer.
let mayConfigure = null
const canConfigure = () => {
    if (mayConfigure === null)
        mayConfigure = !!ec.artifactExecution.isPermittedOwn(CONFIGURATION_SCREEN, "AT_XML_SCREEN", "AUTHZA_VIEW")
    return mayConfigure
}
// Whether the caller may change Configuration (store#ProfileSettings): its update, not only its view
const canChangeConfiguration = () => !!ec.artifactExecution.isPermittedOwn(CONFIGURATION_SCREEN, "AT_XML_SCREEN", "AUTHZA_UPDATE")
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
// scheme://host[:port], lower case, without the scheme's default port; null for a URL with credentials (user@host): its
// host is the part after the @, which a check of the part before it would miss
function originOf(url) {
    const parts = /^(https?):\/\/([^\/?#@\s\\]+)(?=[\/?#]|$)/i.exec(text(url))
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
    // a key is never sent to a browser, and no provider kind has one meant to be public: one with a key is drawn only
    // through this server's proxy, which adds it
    if (text(provider.apiKey) && provider.proxyRequests !== "Y")
        return refusal("PROVIDER_KEYED", say("its provider ${name} needs a key, which is never sent to a browser", {name}))
    return null
}
// The map archives (MapSources.js), loaded when a layer has one
let sources = null
// The OGC services (MapOgc.js), loaded when a layer is of one
let ogcModule = null
const ogc = () => (ogcModule = ogcModule || ec.resource.script("component://framework/service/org/moqui/maps/MapOgc.js"))
const OGC_FORMATS = ["MLF_WMS", "MLF_WMTS", "MLF_WFS"]
const PROXY_PATH = "/apps/maps/LocalMap/providerProxy"
// The URLs an OGC layer's browser asks for (MapOgc.js layerSource, from what its provider's last test recorded): {url, styleUrl,
// ogc}, or a refusal. Always of an enabled, tested provider of its service, on that provider's origin or through this
// server's proxy.
function ogcUrls(layer, provider) {
    if (!provider)
        return refusal("SOURCE_UNTESTED", L("it asks a host outside this server that is not an enabled, tested map provider's"))
    if (offline())
        return refusal(
            "EXTERNAL_REQUESTS_OFF",
            L("it is drawn from a host outside this server, and outside requests are turned off (maps@external-requests)")
        )
    if (!providerTested(provider))
        return refusal(
            "PROVIDER_UNTESTED",
            say("its provider ${name} has not passed a test as it is configured now", {
                name: text(provider.providerName) || provider.providerId
            })
        )
    const proxiedProvider = provider.proxyRequests === "Y"
    if (proxiedProvider) {
        providerKind = providerKind || ec.resource.script("component://framework/service/org/moqui/maps/MapProviders.js")
        if (!providerKind.proxyOffered(provider))
            return refusal("LAYER_MISSING", say("the layer ${layerId} is not registered", {layerId: layer.layerId}))
    }
    const built = ogc().layerSource(
        layer,
        provider,
        proxiedProvider ? PROXY_PATH + "/" + encodeURIComponent(provider.providerId) : null
    )
    if (!built.ok) return built
    // a direct one: on its provider's own origin only (each a provider's recorded address or the endpoint's)
    if (!proxiedProvider) {
        const own = new Set(originsOf(provider.endpointUrl))
        const asked = originsOf(built.url)
        if (!asked.length || !asked.every(origin => own.has(origin)))
            return refusal("URL_NOT_PROVIDERS", L("its address is not on its provider's host"))
    }
    return {url: built.url, styleUrl: "", ogc: built.ogc}
}
// A raster provider's endpoint that is a Cloud Optimized GeoTIFF rather than a tile template or a TileJSON: a path ending
// in .tif or .tiff, with no placeholder (MapProviders.js tests such a provider by a range of that COG)
const cogEndpoint = url => !/[{}]/.test(text(url)) && /\.tiff?$/i.test(text(url).split(/[?#]/)[0])
// The URLs a layer's browser asks for: {url, styleUrl}, or a refusal.
function layerUrls(layer, provider, kind) {
    const format = text(layer.formatEnumId)
    let url = ""
    let styleUrl = ""
    if (format === "MLF_VECTOR_STYLE") url = text(layer.styleLocation) || text(provider?.endpointUrl)
    else if (format === "MLF_RASTER_XYZ") {
        // a COG provider's URL is a GeoTIFF, never a tile template
        if (!text(layer.sourceUrl) && cogEndpoint(provider?.endpointUrl))
            return refusal(
                "SOURCE_NOT_TILES",
                L("its provider's URL is a GeoTIFF, not a tile template: give the layer its own tile URL")
            )
        url = text(layer.sourceUrl) || text(provider?.endpointUrl)
    } else if (format === "MLF_ADAPTER") return {url: "", styleUrl: ""}
    else if (OGC_FORMATS.includes(format)) return ogcUrls(layer, provider)
    else {
        url = text(layer.sourceUrl)
        if (kind === "pmtiles") styleUrl = text(layer.styleLocation)
    }
    if (!url && text(layer.resourceLocation)) {
        if (kind !== "pmtiles" && kind !== "cog")
            return refusal(
                "SOURCE_NOT_SERVED",
                L("its file is a registered resource, which this server does not hand to a map here")
            )
        // a registered archive: the map-source transition's same-origin URL with its revision, for a caller it lets read the
        // archive now (MapSources.js); one it does not is, to a caller who may not configure, one that is not there
        sources = sources || ec.resource.script("component://framework/service/org/moqui/maps/MapSources.js")
        const served = sources.servedUrl(layer)
        if (!served.ok)
            return served.reasonCode === "SOURCE_MISSING" || (served.reasonCode === "SOURCE_NOT_PERMITTED" && !canConfigure())
                ? refusal("LAYER_MISSING", say("the layer ${layerId} is not registered", {layerId: layer.layerId}))
                : refusal(served.reasonCode, served.reason)
        url = served.url
    }
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
    // a provider's COG is read by the browser itself, by range: never through the proxy
    if (kind === "cog" && provider?.proxyRequests === "Y" && isWeb(url))
        return refusal("PROVIDER_PROXIED", L("a GeoTIFF is read by the browser itself, not through this server"))
    // a proxied provider: the browser asks this server's proxy, at the same paths of the provider's origin, the key added
    // there; only a caller the proxy lets reach the provider is handed them
    if (provider?.proxyRequests === "Y" && isWeb(url)) {
        providerKind = providerKind || ec.resource.script("component://framework/service/org/moqui/maps/MapProviders.js")
        if (!providerKind.proxyOffered(provider))
            return refusal("LAYER_MISSING", say("the layer ${layerId} is not registered", {layerId: layer.layerId}))
        const proxiedUrl = providerKind.proxyUrl(provider, url)
        if (!proxiedUrl) return refusal("URL_NOT_PROVIDERS", L("its address is not on its provider's host"))
        // the proxy forwards only what the provider's last test reached: a layer added since needs a new test
        if (!providerKind.proxyRecorded(provider, url))
            return refusal(
                "PROVIDER_UNTESTED",
                say(
                    "its address is not one a test of its provider ${name} reached through this server: test the provider again",
                    {
                        name: text(provider.providerName) || provider.providerId
                    }
                )
            )
        return {url: proxiedUrl, styleUrl}
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
    // an OGC layer: what m-map builds its source from (MapOgc.js layerSource), its URL the service's or the proxy's
    if (urls.ogc) entry.ogc = urls.ogc
    if (format === "MLF_PMTILES_RASTER") entry.raster = true
    if (kind === "cog") {
        const raster = cogRaster(layer)
        if (!raster.ok) return raster
        entry.raster = raster.raster
    }
    if (num(layer.minZoom) !== null) entry.minZoom = num(layer.minZoom)
    if (num(layer.maxZoom) !== null) entry.maxZoom = num(layer.maxZoom)
    if (kind === "features") entry.adapterKey = text(layer.adapterKey)
    const origins = [urls.url, urls.styleUrl].flatMap(originsOf)
    return {ok: true, entry, origins}
}
// ===== projections and rasters (P4c) =====
// A projection a map may be drawn in or read a COG in: EPSG:3857 and EPSG:4326, which every map has, or an enabled row of
// the registry with a definition; null otherwise. The rows a configuration uses are collected for its projections list.
const usedProjections = new Map()
function projectionOf(code) {
    const value = text(code).toUpperCase()
    if (!value) return null
    if (BUILT_IN_PROJECTIONS.includes(value)) return {code: value, builtIn: true}
    const row = one(PROJECTION, {projectionId: value})
    if (!row || row.disabled === "Y" || !text(row.proj4Definition)) return null
    const projected = text(row.projectedExtent)
        .split(",")
        .map(num)
        .filter(value => value !== null)
    const out = {
        code: value,
        label: text(row.label) || value,
        proj4: text(row.proj4Definition),
        bounds: extentOf([row.minLongitude, row.minLatitude, row.maxLongitude, row.maxLatitude]),
        extent: projected.length === 4 ? projected : null
    }
    usedProjections.set(value, out)
    return out
}
// The projection a profile's map is drawn in: its viewProjection when that is one a map draws, else EPSG:3857
const viewProjectionOf = profile => projectionOf(profile?.viewProjection)?.code || WEB_MERCATOR
// Whether a basemap or layer of a kind may be drawn in a projection: everything in EPSG:3857; vector tiles (a MapLibre
// style, a vector PMTiles archive, a user's overlay simplified into tiles) only there, since a map cannot reproject them;
// rasters (XYZ, WMS, WMTS, a raster archive, a COG) are reprojected, and vector files read in any.
const drawableIn = (entry, projection) =>
    projection === WEB_MERCATOR ||
    !(entry.kind === "style" || entry.kind === "mvt" || (entry.kind === "pmtiles" && !entry.raster))
const projectionRefusal = (entry, projection) =>
    refusal(
        "PROJECTION_INCOMPATIBLE",
        say("${label} is drawn only in Web Mercator (EPSG:3857), not in ${projection}: its vector tiles are not reprojected", {
            label: entry.label,
            projection
        })
    )
// A COG's band settings checked against its band count: {rgb: [r, g, b]} or {band, colormap}, with min and max (min below
// max) and an optional nodata; null when they do not read
function rasterStyleOf(value, bands) {
    if (!value || typeof value !== "object" || Array.isArray(value)) return null
    const bandOk = band => Number.isInteger(band) && band >= 1 && band <= (bands || 64)
    const out = {}
    if (Array.isArray(value.rgb)) {
        const rgb = value.rgb.map(num)
        if (rgb.length !== 3 || !rgb.every(bandOk)) return null
        out.rgb = rgb
    } else {
        const band = num(value.band)
        if (!bandOk(band)) return null
        out.band = band
        const colormap = text(value.colormap) || COLORMAPS[0]
        if (!COLORMAPS.includes(colormap)) return null
        out.colormap = colormap
    }
    const min = num(value.min)
    const max = num(value.max)
    if (min === null || max === null || !(min < max)) return null
    out.min = min
    out.max = max
    if (value.nodata !== undefined && value.nodata !== null && value.nodata !== "") {
        const nodata = num(value.nodata)
        if (nodata === null) return null
        out.nodata = nodata
    }
    return out
}
// What m-map draws a COG layer with: its header's bands and type, its CRS (registered), its band settings (the user's
// own, else the layer's defaults) and whether its provider answers CORS; a refusal when its header was never read or its CRS
// is not a projection a map draws
let rasterPreferences = null
function cogRaster(layer) {
    let raster = null
    try {
        raster = JSON.parse(text(layer.rasterJson) || "null")
    } catch (error) {
        raster = null
    }
    if (!raster || typeof raster !== "object")
        return refusal(
            "SOURCE_NOT_REGISTERED",
            L("its GeoTIFF's header has not been read: register it under Maps › Configuration › Archives")
        )
    const projection = projectionOf(raster.epsg || layer.crs)
    if (!projection) {
        const crs = text(raster.epsg || layer.crs).toUpperCase() || "?"
        const row = one(PROJECTION, {projectionId: crs})
        return refusal(
            "PROJECTION_UNREGISTERED",
            !row
                ? say("its CRS ${crs} is not a registered map projection", {crs})
                : row.disabled === "Y"
                  ? say("its CRS ${crs} is a disabled map projection", {crs})
                  : say("its CRS ${crs} is registered with no definition", {crs})
        )
    }
    const bands = num(raster.bands) || 1
    rasterPreferences = rasterPreferences || storedPreferences().rasters || {}
    const own = rasterStyleOf(rasterPreferences[layer.layerId], bands)
    const style = own || rasterStyleOf(raster.defaults, bands) || {band: 1, min: 0, max: 255, colormap: COLORMAPS[0]}
    return {
        ok: true,
        raster: {
            bands,
            sampleType: text(raster.sampleType),
            projection: projection.code,
            nodata: num(raster.nodata),
            statistics: Array.isArray(raster.statistics) ? raster.statistics.slice(0, 64) : [],
            cors: raster.cors !== false,
            style,
            defaults: rasterStyleOf(raster.defaults, bands),
            own: !!own
        }
    }
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
    const projection = viewProjectionOf(profile)
    if (!drawableIn(basemap.entry, projection)) {
        const refused = projectionRefusal(basemap.entry, projection)
        return refusal("BASEMAP_UNAVAILABLE", say("its basemap is not available: ${reason}", {reason: refused.reason}))
    }
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

// A postal code's one local match (get#PostalLocation, MapLookup.js), for a record whose adapter gave an address with no
// point; null when there is none or more than one
function postalPlace(postalCode, countryGeoId) {
    if (!text(countryGeoId)) return null
    const found = probe("org.moqui.maps.MapServices.get#PostalLocation", {
        postalCode: text(postalCode),
        countryGeoId: text(countryGeoId)
    })
    const matches = found.failed ? [] : Array.from(found.out.matches || [])
    return matches.length === 1 ? matches[0] : null
}

// An address with no point, matched by the installation's local geocoder (MPK_GEOCODER_OVERTURE: this server's own index of
// Overture address points, no outside request) when one is enabled, tested and its data active: {candidates, ambiguous} of
// the address matches only, none whose postal code or locality differs (an estimate is no better than the postal area), or
// null when there is no such geocoder. Searched as this read's own step (the adapter let the caller read the record), so a
// user with no Maps app grant has it too.
const UNIT_LINE = /^\s*(#|apt\b|apartment\b|unit\b|suite\b|ste\b|fl\b|floor\b|rm\b|room\b|int\b|interno\b|app\b)/i
function addressMatch(out) {
    const geocoders = ec.resource.script("component://framework/service/org/moqui/maps/MapGeocoders.js")
    const line = [text(out.address1), UNIT_LINE.test(text(out.address2)) ? text(out.address2) : ""].filter(Boolean).join(" ")
    const place = [text(out.postalCode), text(out.city)].filter(Boolean).join(" ")
    let found = null
    ec.message.pushErrors()
    try {
        found = geocoders.localSearch([line, place].filter(Boolean).join(", ").slice(0, 200), text(out.countryGeoId) || null, 5)
    } catch (error) {
        ec.logger.warn("The local address match of a record failed: " + (error?.message ?? error))
        found = null
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
    if (!found) return null
    const candidates = Array.from(found.candidates || []).filter(
        candidate =>
            candidate.precisionEnumId === "GPP_ADDRESS_MATCH" &&
            candidate.matchKind === "address" &&
            candidate.postcodeAgrees !== false &&
            candidate.localityAgrees !== false
    )
    return {candidates, ambiguous: candidates.length > 1, providerName: text(found.providerName)}
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
    const viewProjection = profile ? viewProjectionOf(profile) : WEB_MERCATOR
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
                // one the profile's view cannot draw is not offered
                if (!resolved.ok || !drawableIn(resolved.entry, viewProjection)) continue
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
            const incompatible = resolved.ok && !drawableIn(resolved.entry, viewProjection)
            fallbacks.push({
                kind: "basemap",
                id: preferredBasemap,
                reasonCode: incompatible ? "PROJECTION_INCOMPATIBLE" : resolved.ok ? "BASEMAP_UNAVAILABLE" : resolved.reasonCode,
                message: say("Your basemap ${id} is not available (${reason}): the profile's is shown.", {
                    id: preferredBasemap,
                    reason: incompatible
                        ? projectionRefusal(resolved.entry, viewProjection).reason
                        : resolved.ok
                          ? L("the profile does not offer it")
                          : resolved.reason
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
                // a user's overlay (overlay:ID) is not a MapLayer: Browse lists it itself (find#Overlays)
                .filter(id => id && !id.startsWith("overlay:"))
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
        const found = resolveLayer(layer, row.layerId, false)
        // a layer of vector tiles over a view in another projection is named, not drawn
        const resolved =
            found.ok && !drawableIn(found.entry, viewProjection) ? projectionRefusal(found.entry, viewProjection) : found
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
    if (viewProjection !== WEB_MERCATOR) config.projection = viewProjection
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
                    // the links a registered route (moqui.maps.MapRoute) of the host app (routeApp), else of no app, resolves
                    // to a screen the user's own grants open (MapRoutes.js); record.links stay as the adapter named them
                    try {
                        record.resolvedLinks = record.links.length
                            ? ec.resource
                                  .script("component://framework/service/org/moqui/maps/MapRoutes.js")
                                  .resolveLinks(record.links, text(p.routeApp) || null, recordKey)
                            : []
                    } catch (err) {
                        ec.logger.warn(
                            "Map adapter " + adapterKey + ": the record's links were not resolved: " + (err?.message ?? err)
                        )
                        record.resolvedLinks = []
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
                    } else if (text(out.postalCode) || text(out.address1)) {
                        // an address with no point (§6.1): the address point the local geocoder matches, when the
                        // installation has one (this server's index); else its local postal-area position, approximate,
                        // with a broad extent. No outside service is asked and nothing is saved to the record: Choose
                        // location saves through the adapter
                        const matched = text(out.address1) ? addressMatch(out) : null
                        const postal =
                            matched?.candidates.length === 1 || !text(out.postalCode)
                                ? null
                                : postalPlace(out.postalCode, out.countryGeoId)
                        if (matched?.candidates.length === 1) {
                            const found = matched.candidates[0]
                            Object.assign(record, {
                                precisionEnumId: found.precisionEnumId,
                                precisionText: found.precisionText,
                                sourceText: found.sourceText,
                                approximate: true,
                                addressMatched: true,
                                addressNote: say(
                                    "Found from the record's address by ${name}, not saved to the record: Choose location saves a point",
                                    {name: matched.providerName}
                                )
                            })
                            points.push({
                                id: "record",
                                lon: found.lon,
                                lat: found.lat,
                                label: record.label || found.label,
                                precision: found.precisionText,
                                source: found.sourceText
                            })
                            setView(found.extent ? {extent: found.extent} : {center: [found.lon, found.lat], zoom: 17})
                        } else if (matched?.candidates.length > 1) {
                            // several address points: choices on the map, none chosen; the postal area is the view
                            record.addressNote = L(
                                "Several address points match the record's address: they are shown, none is chosen; Choose location saves one"
                            )
                            record.addressChoices = matched.candidates.map((found, index) => ({
                                id: "address:" + index,
                                label: found.label,
                                lon: found.lon,
                                lat: found.lat,
                                precision: found.precisionText,
                                source: found.sourceText
                            }))
                            points.push(...record.addressChoices)
                        }
                        if (postal) {
                            Object.assign(record, {
                                precisionEnumId: postal.precisionEnumId,
                                precisionText: postal.precisionText,
                                sourceText: postal.sourceText,
                                approximate: true
                            })
                            points.push({
                                id: "record",
                                lon: postal.lon,
                                lat: postal.lat,
                                label: record.label || postal.displayCode,
                                precision: postal.precisionText,
                                source: postal.sourceText
                            })
                            setView({extent: postal.extent})
                        } else if (record.addressChoices) {
                            const lons = record.addressChoices.map(choice => choice.lon)
                            const lats = record.addressChoices.map(choice => choice.lat)
                            const pad = 0.002
                            setView({
                                extent: [
                                    Math.min(...lons) - pad,
                                    Math.min(...lats) - pad,
                                    Math.max(...lons) + pad,
                                    Math.max(...lats) + pad
                                ]
                            })
                        } else if (!record.addressMatched && extent) setView({extent})
                    } else if (extent) setView({extent})
                }
            }
        }
    }

    // ----- record layers: each adapter's features for the initial view, cut to its limit -----
    let truncated = false
    const recordLayers = []
    // the view the features are read for: the one asked for (Browse's search, the view on screen), else the initial one
    const featuresExtent = extentOf(p.featuresExtent) || config.extent || WORLD
    // recordLayers false (the local-map dialog's own record read): the layers are listed, and no adapter is asked
    const readFeatures = !(p.recordLayers === false || p.recordLayers === "false" || p.recordLayers === "N")
    for (const layer of adapterLayers) {
        recordLayers.push({id: layer.id, label: layer.label, visible: layer.visible, adapterKey: layer.adapterKey})
        // a hidden layer's adapter is not asked until it is shown
        if (!layer.visible || !readFeatures) continue
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
            extent: (([west, south, east, north]) => ({west, south, east, north}))(featuresExtent),
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
    // the definitions of the registered projections the map needs (its view's, its COG layers'): m-map registers each with
    // proj4js, and fetches none
    const projections = Array.from(usedProjections.values()).filter(row => !BUILT_IN_PROJECTIONS.includes(row.code))
    if (projections.length) config.projections = projections.slice(0, 50)
    // the record layers, drawn as features: each with whether it is shown (a hidden one's adapter was not asked)
    if (recordLayers.length) config.recordLayers = recordLayers
    config.points = points
    if (features.length) config.features = {type: "FeatureCollection", features}
    if (truncated) config.featuresTruncated = true
    if (record) config.record = record
    config.fallbacks = fallbacks
    config.layerErrors = layerErrors
    config.externalRequests = !offline()
    if (yes(p.locationShares)) {
        const live = liveShares()
        config.locationShares = live.locationShares
        if (live.failed) config.locationSharesFailed = true
    }
    return {config}
}

// ===== live location (plans/Location-Sharing.md §3.3) =====
// The viewer's own active shares and those shared with them, for m-map's live layer, Browse's panels and the terminal. Read
// only through org.moqui.maps.LocationServices, whose reads check the caller against each share's current audience
// (find#MyShares, find#SharedWithMe, get#Share for a kept trail): no entity of a share is read here. Nothing of it is
// logged or cached, and a failed read leaves that part out.
const LIVE_SERVICES = "org.moqui.maps.LocationServices."
// the most points of a trail handed to a map, its latest ones, and the most shares whose trail is read
const LIVE_TRAIL_POINTS = 500
const LIVE_TRAIL_SHARES = 10
const wholeSetting = (name, fallback) => {
    const number = Number(text(ec.factory.getConfValue("location@" + name)))
    return Number.isInteger(number) && number >= 0 ? number : fallback
}
function livePosition(row) {
    if (!row || row.latitude == null || row.longitude == null) return null
    return {
        latitude: Number(row.latitude),
        longitude: Number(row.longitude),
        accuracy: row.accuracy == null ? null : Number(row.accuracy),
        heading: row.heading == null ? null : Number(row.heading),
        speed: row.speed == null ? null : Number(row.speed),
        observedDate: row.observedDate == null ? null : Number(row.observedDate),
        ageSeconds: row.ageSeconds == null ? null : Number(row.ageSeconds)
    }
}
// a share's kept trail as [longitude, latitude] pairs, its latest LIVE_TRAIL_POINTS in time order (get#Share's pages)
function liveTrail(shareId, knownCount) {
    const page = (index, size) => {
        const found = probe(LIVE_SERVICES + "get#Share", {shareId, trailPageIndex: index, trailPageSize: size})
        return found.failed || found.out.reasonCode ? null : found.out
    }
    let count = knownCount
    if (count == null) {
        const head = page(0, 1)
        if (!head) return []
        count = Number(head.trailCount) || 0
    }
    if (!count) return []
    let rows = []
    if (count <= LIVE_TRAIL_POINTS) rows = Array.from(page(0, LIVE_TRAIL_POINTS)?.trailList || [])
    else {
        const last = Math.floor((count - 1) / LIVE_TRAIL_POINTS)
        for (const index of [last - 1, last]) rows = rows.concat(Array.from(page(index, LIVE_TRAIL_POINTS)?.trailList || []))
        rows = rows.slice(-LIVE_TRAIL_POINTS)
    }
    return rows
        .map(livePosition)
        .filter(point => point)
        .map(point => [point.longitude, point.latitude])
}
// who an own share is with, as names: its entries that are not revoked
function audienceNames(list) {
    return Array.from(list || [])
        .filter(row => row && !row.revoked)
        .map(row => ({
            audienceSeqId: row.audienceSeqId,
            audienceTypeEnumId: row.audienceTypeEnumId,
            userGroupId: row.userGroupId || null,
            name:
                row.audienceTypeEnumId === "LsaLink"
                    ? L("A guest link")
                    : text(row.userName || row.groupName || row.jid || row.userId || row.userGroupId)
        }))
}
function liveShares() {
    if (!ec.user.userId) return {locationShares: []}
    const minimum = wholeSetting("min-interval-seconds", 5)
    const approximate = Math.max(minimum, wholeSetting("approximate-interval-seconds", 60))
    const shares = []
    const mine = probe(LIVE_SERVICES + "find#MyShares", {})
    for (const share of mine.failed ? [] : Array.from(mine.out.shareList || []))
        shares.push({
            shareId: share.shareId,
            role: "owner",
            label: text(share.label),
            purposeText: share.purposeText || null,
            fromDate: share.fromDate,
            thruDate: share.thruDate,
            approximate: share.approximate === true,
            keepTrail: share.keepTrail === true,
            trailKeepDays: share.trailKeepDays ?? null,
            trailCount: Number(share.trailCount) || 0,
            audience: audienceNames(share.audienceList),
            reportIntervalSeconds: share.approximate === true ? approximate : minimum,
            latest: livePosition(share.latest)
        })
    const theirs = probe(LIVE_SERVICES + "find#SharedWithMe", {})
    // a read that failed is said, so a page does not take its empty list for the user's having no share
    const failed = mine.failed || theirs.failed
    for (const share of theirs.failed ? [] : Array.from(theirs.out.shareList || []))
        shares.push({
            shareId: share.shareId,
            role: "viewer",
            label: text(share.label) || text(share.ownerName),
            ownerName: text(share.ownerName),
            purposeText: share.purposeText || null,
            fromDate: share.fromDate,
            thruDate: share.thruDate,
            approximate: share.approximate === true,
            keepTrail: share.keepTrail === true,
            latest: livePosition(share.latest)
        })
    for (const share of shares.filter(one => one.keepTrail).slice(0, LIVE_TRAIL_SHARES))
        share.trail = liveTrail(share.shareId, share.role === "owner" ? share.trailCount : null)
    return {locationShares: shares, failed}
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
            layers: Object.fromEntries(Object.keys(layers).map(layerId => [layerId, yes(layers[layerId])])),
            rasters: stored.rasters && typeof stored.rasters === "object" ? stored.rasters : {}
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
    // a COG layer's band settings, each checked against the layer's bands; a layer's null clears its own
    if (given(p.rasters)) {
        const rasters = p.rasters && typeof p.rasters === "object" && !Array.isArray(p.rasters) ? p.rasters : null
        if (!rasters) throw new Error(L("The band settings are each COG layer's ID with its settings"))
        const kept = Object.assign({}, next.rasters || {})
        for (const layerId of Object.keys(rasters).slice(0, MAX_LAYERS)) {
            if (rasters[layerId] === null || rasters[layerId] === "") {
                delete kept[layerId]
                continue
            }
            const layer = one(LAYER, {layerId})
            const resolved = resolveLayer(layer, layerId, false)
            if (!resolved.ok || resolved.entry.kind !== "cog")
                return refused("LAYER_UNAVAILABLE", say("The layer ${layerId} is not a COG layer you may be offered", {layerId}))
            const style = rasterStyleOf(rasters[layerId], resolved.entry.raster.bands)
            if (!style)
                return refused(
                    "RASTER_INVALID",
                    say(
                        "The band settings of ${layerId} are bands of the layer, a minimum below a maximum and a known colormap",
                        {
                            layerId
                        }
                    )
                )
            kept[layerId] = style
        }
        if (Object.keys(kept).length > MAX_LAYERS) throw new Error(say("At most ${count} layers", {count: MAX_LAYERS}))
        if (Object.keys(kept).length) next.rasters = kept
        else delete next.rasters
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
        if (!projectionOf(projection))
            throw new Error(L("The view's projection is EPSG:3857, EPSG:4326 or a registered map projection"))
        out.projection = projection
    }
    // the band settings of its COG layers, as a user's preferences hold them (each layer's bands checked when it is drawn)
    if (view.rasters && typeof view.rasters === "object" && !Array.isArray(view.rasters)) {
        const rasters = {}
        for (const layerId of Object.keys(view.rasters).slice(0, MAX_LAYERS)) {
            const style = rasterStyleOf(view.rasters[layerId], 64)
            if (!style) throw new Error(say("The view's band settings of ${layerId} do not read", {layerId}))
            rasters[layerId] = style
        }
        if (Object.keys(rasters).length) out.rasters = rasters
    }
    const layers = view.layers == null ? [] : Array.isArray(view.layers) ? view.layers : text(view.layers).split(",")
    out.layers = Array.from(new Set(layers.map(text).filter(id => id)))
    if (out.layers.length > MAX_LAYERS) throw new Error(say("A saved view names at most ${count} layers", {count: MAX_LAYERS}))
    if (text(view.basemap)) out.basemap = text(view.basemap)
    return out
}
// A layer a saved view names: a MapLayer resolved for the reader, or a user's overlay (overlay:ID, MapOverlays.js) the
// reader may read in its drive now
let overlays = null
function viewLayer(layerId) {
    if (!text(layerId).startsWith("overlay:")) return resolveLayer(one(LAYER, {layerId}), layerId, false)
    overlays = overlays || ec.resource.script("component://framework/service/org/moqui/maps/MapOverlays.js")
    return overlays.overlayLayerReadable(layerId)
        ? {ok: true}
        : refusal("LAYER_MISSING", say("the layer ${layerId} is not registered", {layerId}))
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
            const resolved = viewLayer(layerId)
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
            const shown = layers.filter(layerId => viewLayer(layerId).ok)
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

// ===== find#Configuration and store#ProfileSettings (Maps › Configuration) =====
// For a caller who may open Configuration (canConfigure) only: what is configured, each profile and layer with whether it
// can be drawn now and why not, and the installation's outside-request policy and limits as the conf sets them (read
// here, changed only in the conf).
function findConfiguration() {
    if (!canConfigure()) return refused("PERMISSION_DENIED", L("Maps configuration is not available to you"))
    const installed = installationProfileId()
    const layerNames = new Map()
    const layerList = Array.from(find(LAYER, {}).orderBy("layerName").limit(200).list()).map(layer => {
        const resolved = resolveLayer(layer, layer.layerId, layer.isBasemap === "Y")
        const provider = text(layer.providerId) ? one(PROVIDER, {providerId: layer.providerId}) : null
        const layerName = text(layer.layerName) ? L(layer.layerName) : layer.layerId
        layerNames.set(layer.layerId, layerName)
        return {
            layerId: layer.layerId,
            layerName,
            formatEnumId: text(layer.formatEnumId) || null,
            providerName: provider ? text(provider.providerName) || provider.providerId : "",
            isBasemap: layer.isBasemap === "Y",
            adapterKey: text(layer.adapterKey) || null,
            enabled: layer.disabled !== "Y",
            drawable: resolved.ok,
            reason: resolved.ok ? "" : resolved.reason
        }
    })
    const profileList = Array.from(find(PROFILE, {}).orderBy("profileName").limit(200).list()).map(profile => {
        const resolved = resolveProfile(profile, profile.profileId)
        return {
            profileId: profile.profileId,
            profileName: profileName(profile, profile.profileId),
            description: text(profile.description) ? L(profile.description) : "",
            basemapName: layerNames.get(text(profile.basemapLayerId)) || text(profile.basemapLayerId),
            externalAccess: profile.externalAccess === "Y",
            enabled: profile.disabled !== "Y",
            installationDefault: profile.profileId === installed,
            viewProjection: viewProjectionOf(profile),
            drawable: resolved.ok,
            reason: resolved.ok ? "" : resolved.reason
        }
    })
    const setting = name => text(ec.factory.getConfValue("maps@" + name))
    return {
        applied: true,
        installationProfileId: installed || null,
        externalRequests: !offline(),
        maxFeatures: setting("max-features") || null,
        importMaxBytes: setting("import-max-bytes") || null,
        // the projections a profile's view may be drawn in: the two every map has, and the registry's (P4c)
        projectionList: BUILT_IN_PROJECTIONS.map(code => ({
            code,
            label: code === WEB_MERCATOR ? L("Web Mercator") : L("WGS 84 (degrees)")
        })).concat(
            Array.from(find(PROJECTION, {}).orderBy("projectionId").limit(500).list())
                .filter(row => row.disabled !== "Y" && !BUILT_IN_PROJECTIONS.includes(row.projectionId))
                .map(row => ({code: row.projectionId, label: text(row.label) || row.projectionId}))
        ),
        profileList,
        layerList
    }
}
// Enable or disable a profile, and make one the installation's default (the ALL_USERS UserGroupPreference maps.profile).
// The default must be enabled and drawable now (PROFILE_UNAVAILABLE, with the cause); the default is not disabled
// (DEFAULT_PROFILE: choose another first). A profile enabled whose basemap's provider is not enabled yet is enabled, and
// the answer says it is not drawn until the provider is.
// A write the caller was checked for above, of configuration no group is granted to write: authorization off, the caller
// kept, so the audit log (MapProfile.disabled, UserGroupPreference.preferenceValue) names who changed it
function asCaller(write) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return write()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function storeProfileSettings(p) {
    if (!canConfigure() || !canChangeConfiguration())
        return refused("PERMISSION_DENIED", L("Changing the maps configuration is not available to you"))
    const profileId = text(p.profileId)
    const profile = profileId ? ec.entity.find(PROFILE).condition({profileId}).disableAuthz().one() : null
    if (!profile) return refused("RECORD_UNAVAILABLE", say("The map profile ${profileId} is not registered", {profileId}))
    const installed = installationProfileId()
    const enable = given(p.enabled) && text(p.enabled) !== "" ? yes(p.enabled) : null
    const makeDefault = yes(p.makeDefault)
    if (enable === false && (profileId === installed || makeDefault))
        return refused(
            "DEFAULT_PROFILE",
            L("This profile is the installation's default map profile: choose another default before disabling it")
        )
    // the projection its view is drawn in: one a map draws, in which its basemap can be drawn
    if (given(p.viewProjection)) {
        const code = text(p.viewProjection).toUpperCase() || WEB_MERCATOR
        if (!projectionOf(code))
            return refused("PROJECTION_UNKNOWN", say("${code} is not a projection a map draws: register it first", {code}))
        const basemap = resolveLayer(
            one(LAYER, {layerId: profile.basemapLayerId}),
            text(profile.basemapLayerId) || "(none)",
            true
        )
        if (basemap.ok && !drawableIn(basemap.entry, code))
            return refused("PROJECTION_INCOMPATIBLE", projectionRefusal(basemap.entry, code).reason)
        const stored = code === WEB_MERCATOR ? null : code
        if (text(profile.viewProjection) !== text(stored)) {
            profile.viewProjection = stored
            asCaller(() => ec.entity.makeValue(PROFILE).setAll({profileId, viewProjection: stored}).update())
        }
    }
    if (enable !== null && (profile.disabled !== "Y") !== enable) {
        profile.disabled = enable ? "N" : "Y"
        asCaller(() =>
            ec.entity
                .makeValue(PROFILE)
                .setAll({profileId, disabled: enable ? "N" : "Y"})
                .update()
        )
    }
    const name = profileName(profile, profileId)
    const resolved = resolveProfile(profile, profileId)
    if (makeDefault) {
        if (!resolved.ok)
            return refused(
                "PROFILE_UNAVAILABLE",
                say("${name} cannot be the default: ${reason}", {name, reason: resolved.reason}),
                {cause: resolved.reasonCode}
            )
        const row = ec.entity.makeValue(GROUP_PREFERENCE)
        row.setFields({userGroupId: "ALL_USERS", preferenceKey: PROFILE_KEY, preferenceValue: profileId}, true, null, null)
        asCaller(() => row.createOrUpdate())
        return {applied: true, reason: say("${name} is the installation's default map profile", {name})}
    }
    if (enable && !resolved.ok)
        return {
            applied: true,
            reason: say("${name} is enabled, but it is not drawn yet: ${reason}", {name, reason: resolved.reason})
        }
    return {applied: true, reason: enable === false ? say("${name} is disabled", {name}) : say("${name} is saved", {name})}
}

// ===== the projection registry (P4c; Configuration › Projections) =====
// A PROJ.4 definition a browser registers: plain parameters only, no URL, no grid but Web Mercator's @null (as m-map's
// moqui.map.normalizeProjections takes them)
const BROWSER_DEFINITION = /^\+proj=[a-z]+(\s+\+[a-z_0-9]+(=[A-Za-z0-9_.,+@-]+)?)*$/
const definitionRefusal = definition =>
    definition.length > 600 || !BROWSER_DEFINITION.test(definition) || /\+(nadgrids|geoidgrids)=(?!@null)/.test(definition)
// Whether the server reads a definition (moqui-lib geo/proj.rs): {readable, reason, transform}
function serverReads(definition) {
    const out = ec.service
        .sync()
        .name("org.moqui.maps.MapSourceServices.check#ProjectionDefinition")
        .parameters({definition})
        .disableAuthz()
        .call()
    return {readable: out?.readable === true, reason: text(out?.reason), transform: text(out?.transform)}
}
const browserOnlyNote = reason =>
    say(
        "A map draws it, but the server does not read its definition (${reason}): an overlay in it is refused, and a COG in it has no recorded bounds",
        {
            reason
        }
    )
// find#Projections
function findProjections() {
    if (!canConfigure()) return refused("PERMISSION_DENIED", L("Maps configuration is not available to you"))
    const projectionList = Array.from(ec.entity.find(PROJECTION).orderBy("projectionId").limit(2000).disableAuthz().list()).map(
        row => {
            const builtIn = BUILT_IN_PROJECTIONS.includes(row.projectionId)
            const server = builtIn ? {readable: true, reason: "", transform: ""} : serverReads(text(row.proj4Definition))
            return {
                projectionId: row.projectionId,
                label: text(row.label) || row.projectionId,
                proj4Definition: text(row.proj4Definition),
                axisOrder: text(row.axisOrder) || null,
                bounds: extentOf([row.minLongitude, row.minLatitude, row.maxLongitude, row.maxLatitude]),
                projectedExtent: text(row.projectedExtent) || null,
                enabled: row.disabled !== "Y",
                builtIn,
                serverReadable: server.readable,
                serverNote: builtIn ? L("Every map's own") : server.readable ? server.transform : browserOnlyNote(server.reason),
                transformNote: text(row.transformNote) || null,
                sourceNote: text(row.sourceNote) || null
            }
        }
    )
    return {applied: true, projectionList}
}
// store#Projection
function storeProjection(p) {
    if (!canConfigure() || !canChangeConfiguration())
        return refused("PERMISSION_DENIED", L("Changing the maps configuration is not available to you"))
    const code = text(p.projectionId).toUpperCase()
    if (!/^EPSG:[0-9]{1,6}$/.test(code)) return refused("CODE_INVALID", L("A projection's code is EPSG: and its number"))
    if (BUILT_IN_PROJECTIONS.includes(code)) return refused("BUILT_IN", L("EPSG:4326 and EPSG:3857 are every map's own"))
    const existing = ec.entity.find(PROJECTION).condition({projectionId: code}).disableAuthz().one()
    const disabled = text(p.disabled).toUpperCase()
    if (disabled && !["Y", "N"].includes(disabled)) return refused("VALUE_INVALID", L("Disabled is Y or N"))
    // enable or disable alone
    if (existing && !given(p.proj4Definition) && !given(p.label) && disabled) {
        asCaller(() => ec.entity.makeValue(PROJECTION).setAll({projectionId: code, disabled}).update())
        return {applied: true, reason: say(disabled === "Y" ? "${code} is disabled" : "${code} is enabled", {code})}
    }
    const definition = text(p.proj4Definition).replace(/\s+/g, " ")
    if (!definition) return refused("DEFINITION_INVALID", L("Give its PROJ.4 definition"))
    if (definitionRefusal(definition))
        return refused(
            "DEFINITION_INVALID",
            L("The definition is PROJ.4 parameters (+proj=... +name=value), with no URL and no grid file but +nadgrids=@null")
        )
    const axisOrder = text(p.axisOrder).toLowerCase() || null
    if (axisOrder && !["enu", "neu"].includes(axisOrder))
        return refused("VALUE_INVALID", L("The axis order is enu (easting first) or neu (northing first)"))
    const bounds = text(p.bounds) ? extentOf(p.bounds) : null
    if (text(p.bounds) && !bounds) return refused("VALUE_INVALID", L("The area of use is west, south, east and north in degrees"))
    const extent = text(p.projectedExtent)
        .split(",")
        .map(num)
        .filter(value => value !== null)
    if (text(p.projectedExtent) && (extent.length !== 4 || !(extent[0] < extent[2] && extent[1] < extent[3])))
        return refused("VALUE_INVALID", L("The extent is the least and greatest x and y in its units"))
    const server = serverReads(definition)
    const fields = {
        projectionId: code,
        label: text(p.label) || text(existing?.label) || code,
        proj4Definition: definition,
        axisOrder,
        minLongitude: bounds ? bounds[0] : null,
        minLatitude: bounds ? bounds[1] : null,
        maxLongitude: bounds ? bounds[2] : null,
        maxLatitude: bounds ? bounds[3] : null,
        projectedExtent: extent.length === 4 ? extent.join(",") : null,
        transformNote: server.readable ? server.transform.slice(0, 250) : null,
        sourceNote: existing ? text(existing.sourceNote) || null : L("Added under Maps › Configuration")
    }
    if (disabled) fields.disabled = disabled
    asCaller(() => ec.entity.makeValue(PROJECTION).setAll(fields).createOrUpdate())
    return {
        applied: true,
        serverReadable: server.readable,
        serverReason: server.readable ? null : server.reason,
        reason: server.readable
            ? say("${code} is saved; the server reads it too: ${transform}", {code, transform: server.transform})
            : say("${code} is saved. ${note}", {code, note: browserOnlyNote(server.reason)})
    }
}

// ===== Add OGC layer (Configuration) =====
const OGC_LAYER_FORMATS = {MPK_WMS: "MLF_WMS", MPK_WMTS: "MLF_WMTS", MPK_WFS: "MLF_WFS"}
// An OGC provider the caller may add a layer from: a tested one of an OGC kind
function ogcProvider(providerId) {
    const provider = text(providerId) ? one(PROVIDER, {providerId: text(providerId)}) : null
    return provider && OGC_LAYER_FORMATS[text(provider.providerKindEnumId)] && providerTested(provider) ? provider : null
}
// get#OgcLayerOptions: the tested OGC providers, and what one's recorded capabilities offer for a layer (MapOgc.js layerOptions)
function ogcLayerOptions(p) {
    if (!canConfigure()) return refused("PERMISSION_DENIED", L("Maps configuration is not available to you"))
    // the fields the test state reads, not each provider's recorded capabilities
    const providers = Array.from(
        find(PROVIDER, {})
            .selectFields([
                "providerId",
                "providerName",
                "providerKindEnumId",
                "endpointUrl",
                "apiKey",
                "apiKeyParameter",
                "proxyRequests",
                "lastUpdatedStamp"
            ])
            .orderBy("providerName")
            .limit(200)
            .list()
    )
        .filter(row => OGC_LAYER_FORMATS[text(row.providerKindEnumId)] && providerTested(row))
        .map(row => ({value: row.providerId, label: text(row.providerName) || row.providerId}))
    const provider = ogcProvider(p.providerId)
    return Object.assign({applied: true, providers}, provider ? ogc().layerOptions(provider, p.ogcLayers) : {})
}
// store#OgcLayer: a layer of a tested OGC provider, from what its last test recorded, checked as a map would draw it before it
// is kept; with a profileId, also one of that profile's layers (an overlay) or that profile's basemap (a basemap layer)
function storeOgcLayer(p) {
    if (!canConfigure() || !canChangeConfiguration())
        return refused("PERMISSION_DENIED", L("Changing the maps configuration is not available to you"))
    const provider = ogcProvider(p.providerId)
    if (!provider)
        return refused("PROVIDER_UNAVAILABLE", L("Choose an OGC provider (WMS, WMTS or WFS) that passed its test as it is now"))
    const format = OGC_LAYER_FORMATS[text(provider.providerKindEnumId)]
    const isBasemap = yes(p.isBasemap) && format !== "MLF_WFS"
    const fields = {
        providerId: provider.providerId,
        formatEnumId: format,
        ogcLayers: text(p.ogcLayers),
        ogcStyles: text(p.ogcStyles) || null,
        ogcFormat: text(p.ogcFormat) || null,
        crs: text(p.crs) || null,
        ogcMatrixSet: format === "MLF_WMTS" ? text(p.ogcMatrixSet) || null : null,
        ogcTime: text(p.ogcTime) || null,
        ogcSingleImage: format === "MLF_WMS" && yes(p.ogcSingleImage) ? "Y" : "N",
        isBasemap: isBasemap ? "Y" : "N",
        disabled: "N"
    }
    const built = ogc().layerSource(Object.assign({layerId: "new"}, fields), provider, null)
    if (!built.ok)
        return refused("LAYER_UNAVAILABLE", say("This layer cannot be drawn: ${reason}", {reason: built.reason}), {
            cause: built.reasonCode
        })
    const title = ogc().layerOptions(provider, fields.ogcLayers.split(",")[0]).title
    let layerName
    try {
        layerName = lib().plainText(text(p.layerName) || title || fields.ogcLayers, "The name", 255)
    } catch (error) {
        return refused("NAME_INVALID", String(error?.message ?? error))
    }
    let layerId = text(p.layerId)
    if (layerId && !/^[A-Za-z0-9][A-Za-z0-9_]{0,39}$/.test(layerId))
        return refused("LAYER_ID_INVALID", L("A layer's id is up to 40 letters, digits or underscores"))
    if (layerId && one(LAYER, {layerId}))
        return refused("LAYER_EXISTS", say("The layer ${layerId} is already registered", {layerId}))
    const profileId = text(p.profileId)
    const profile = profileId ? one(PROFILE, {profileId}) : null
    if (profileId && !profile)
        return refused("RECORD_UNAVAILABLE", say("The map profile ${profileId} is not registered", {profileId}))
    const value = ec.entity.makeValue(LAYER).setAll(Object.assign(fields, {layerName}))
    asCaller(() => {
        if (layerId) value.set("layerId", layerId)
        else value.setSequencedIdPrimary()
        value.create()
        layerId = String(value.layerId)
        if (profile && isBasemap) ec.entity.makeValue(PROFILE).setAll({profileId, basemapLayerId: layerId}).update()
        else if (profile) {
            const count = Number(ec.entity.find(PROFILE_LAYER).condition({profileId}).disableAuthz().count())
            ec.entity
                .makeValue(PROFILE_LAYER)
                .setAll({profileId, layerId, sequenceNum: (count + 1) * 10, defaultVisible: "Y"})
                .createOrUpdate()
        }
    })
    return {
        applied: true,
        layerId,
        reason:
            profile && isBasemap
                ? say("${name} is added, and is the basemap of the profile ${profile}", {
                      name: layerName,
                      profile: profileName(profile, profileId)
                  })
                : profile
                  ? say("${name} is added, and shown in the profile ${profile}", {
                        name: layerName,
                        profile: profileName(profile, profileId)
                    })
                  : say("${name} is added", {name: layerName})
    }
}
let libModule = null
const lib = () =>
    (libModule = libModule || ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationLib.js"))

const operations = {
    ogcLayerOptions,
    storeOgcLayer,
    effectiveConfig,
    liveShares,
    getPreferences,
    storePreferences,
    storeSavedView,
    deleteSavedView,
    findSavedViews,
    findConfiguration,
    storeProfileSettings,
    findProjections,
    storeProjection
}
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
    cogEndpoint,
    OUTLINE_ID
})
