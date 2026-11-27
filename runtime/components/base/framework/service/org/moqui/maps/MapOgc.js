// OGC map services of the mapProvider connection kind (plans/archive/Maps.md §4.1 "WMS/WMTS service configuration", §9 P4): the
// kinds MPK_WMS (WMS 1.3.0), MPK_WMTS (WMTS 1.0.0) and MPK_WFS (WFS 2.0.0, read as GeoJSON), their test, the layers an
// administrator adds from what the test recorded, the source a map draws them from, and what the proxy forwards for a
// proxied one. MapProviders.js calls the test and the proxy's checks; MapConfig.js the layer source.
//
// The test is one GET of the service's GetCapabilities (the endpoint is the service's URL; a WMTS endpoint ending in .xml
// is its RESTful capabilities document, asked as it is), with no credentials but a proxied provider's key, following no
// redirect, at most 8 MiB, to no internal address unless the provider is allowed the private network. The answer is read
// in Rust by the framework's XML reader (MapServices.read#OgcCapabilities, moqui-lib geo/ogc.rs over its node parser), which
// refuses a DOCTYPE, an entity declaration and an entity it does not know, so no DTD or external entity is ever read; a WMS
// 1.1.1 answer (which has a DOCTYPE) is refused with it. In Rust because a large service's capabilities (NASA GIBS's WMS
// is 2.6 MB of some 1,600 layers) are more than a script's memory holds as a tree. It records, when it passes, what a map
// and Configuration need (capabilitiesJson, at most 2 MB): the service's version and its image formats (PNG, JPEG, WebP,
// GIF, AVIF: what the proxy passes), at most MAX_LAYERS layers (WMS named layers, WMTS layers, WFS feature types) with
// their titles, styles, formats, the CRSs of them a map draws (EPSG:3857 and EPSG:4326 under the names OpenLayers knows),
// their bounds and time dimension, a WMTS layer's matrix sets (each kept once, at most 30 matrices) and tile template, and
// for a provider browsers ask themselves whether it answers CORS (cors: one sample image, tile or feature asked with an
// Origin). Its texts are a fixed vocabulary and counts, never the answer's own text.
//
// What a map asks: always the provider's own origin. A URL with credentials (user@host) is never the provider's. An
// operation the capabilities advertise at another host (a server behind a proxy often names its inner name) is asked at the
// endpoint's own URL instead, and a WMTS tile template of another host is not used (KVP at the endpoint is, when the
// service has it). A proxied provider's requests go to LocalMap.xml providerProxy, whose rules the test records
// (proxyPaths): "wms|wmts|wfs path[?query]", the operation's path with the endpoint's own fixed query, and "wmtsrest
// template", a tile template's path (a template with a query, or a placeholder the proxy does not know, is not used:
// KVP is, when the service has it). The proxy forwards a request only for one of the provider's registered layers the
// caller may be offered (allowedRequests: enabled, its viewPermission held, the basemap or a layer of an enabled profile whose
// viewPermission they hold), with that layer's own names, style, format and matrix set, and with only the parameters its
// kind's requests carry, each name once, each other value checked by itself (a CRS a map draws, numbers for a box, a size
// and a tile, a time's characters, a WFS count of at most maps@max-features); the rule's own query is added.
const mapConfig = ec.resource.script("component://framework/service/org/moqui/maps/MapConfig.js")

const text = value => (value == null ? "" : String(value).trim())
const L = value => ec.l10n.localize(value)
const say = (value, values) => ec.resource.expand(L(value), "", values || {})
const own = (object, key) => !!object && typeof object === "object" && Object.hasOwn(object, key)

const LAYER = "moqui.maps.MapLayer"
const PROFILE = "moqui.maps.MapProfile"
const PROFILE_LAYER = "moqui.maps.MapProfileLayer"
const KINDS = {MPK_WMS: "wms", MPK_WMTS: "wmts", MPK_WFS: "wfs"}
const FORMATS = {MLF_WMS: "wms", MLF_WMTS: "wmts", MLF_WFS: "wfs"}
const VERSIONS = {wms: "1.3.0", wmts: "1.0.0", wfs: "2.0.0"}
const NAMES = {wms: "WMS", wmts: "WMTS", wfs: "WFS"}
const MAX_BYTES = 8 * 1024 * 1024
const MAX_LAYERS = 2000
// the features a WFS request asks for at most: maps@max-features, and no more than this, so a typical answer is within the
// proxy's 4 MiB
const WFS_COUNT = 1000
// The CRSs a map draws without a projection of its own: the codes OpenLayers knows for EPSG:3857 and EPSG:4326, a
// urn:ogc:def:crs:EPSG URN read as its EPSG code as OpenLayers reads it (so urn:ogc:def:crs:EPSG:6.18.3:3857 is EPSG:3857)
const WEB_MERCATOR = new Set([
    "EPSG:3857",
    "EPSG:102100",
    "EPSG:102113",
    "EPSG:900913",
    "http://www.opengis.net/def/crs/EPSG/0/3857",
    "http://www.opengis.net/gml/srs/epsg.xml#3857"
])
// longitude first
const CRS84 = [
    "CRS:84",
    "urn:ogc:def:crs:OGC:1.3:CRS84",
    "urn:ogc:def:crs:OGC:2:84",
    "http://www.opengis.net/def/crs/OGC/1.3/CRS84"
]
const GEOGRAPHIC = new Set([
    ...CRS84,
    "EPSG:4326",
    "http://www.opengis.net/gml/srs/epsg.xml#4326",
    "http://www.opengis.net/def/crs/EPSG/0/4326"
])
const olCode = code => text(code).replace(/^urn:(x-)?ogc:def:crs:EPSG:[0-9.]*:(\d+)$/, "EPSG:$2")
const mercator = code => WEB_MERCATOR.has(text(code)) || WEB_MERCATOR.has(olCode(code))
const drawnCrs = code => mercator(code) || GEOGRAPHIC.has(text(code)) || GEOGRAPHIC.has(olCode(code))
// The parameters of a service's own requests, taken out of an endpoint an administrator pasted with them
const RESERVED = ["service", "request", "version", "acceptversions", "sections", "updatesequence", "acceptformats"]

const isOgcKind = kind => own(KINDS, text(kind))
const unique = list => Array.from(new Set(list.filter(value => value)))
const kindOfFormat = format => (own(FORMATS, text(format)) ? FORMATS[text(format)] : null)
const serviceOf = provider => (own(KINDS, text(provider?.providerKindEnumId)) ? KINDS[text(provider.providerKindEnumId)] : null)

// ===== URLs =====
// {origin, path, query} of an http(s) URL (the query without the reserved parameters), or null; null too for a URL with
// credentials (user@host), whose host is not the part a reader sees first
function splitUrl(url) {
    const found = /^(https?:\/\/[^\/?#@\s\\]+)((?:\/[^?#]*)?)(?:\?([^#]*))?(?:#.*)?$/i.exec(text(url))
    if (!found) return null
    const query = text(found[3])
        .split("&")
        .filter(pair => pair && !RESERVED.includes(decodeName(pair.split("=")[0]).toLowerCase()))
        .join("&")
    return {origin: found[1], path: found[2] || "/", query, rawQuery: text(found[3])}
}
function decodeName(value) {
    try {
        return decodeURIComponent(String(value).replace(/\+/g, " "))
    } catch (error) {
        return String(value)
    }
}
// The URL a service's requests are sent to: the endpoint without the reserved parameters
function serviceBase(endpoint) {
    const parts = splitUrl(endpoint)
    return parts ? parts.origin + parts.path + (parts.query ? "?" + parts.query : "") : ""
}
const restfulCapabilities = (kind, endpoint) => kind === "wmts" && /\.xml$/i.test(splitUrl(endpoint)?.path || "")
// The GetCapabilities URL of a service
function capabilitiesUrl(kind, endpoint) {
    if (restfulCapabilities(kind, endpoint)) return text(endpoint).split("#")[0]
    const base = serviceBase(endpoint)
    return (
        base + (base.includes("?") ? "&" : "?") + "SERVICE=" + NAMES[kind] + "&REQUEST=GetCapabilities&VERSION=" + VERSIONS[kind]
    )
}
const sameOrigin = (url, endpoint) => {
    if (!splitUrl(url)) return false
    const one = mapConfig.originOf(url)
    return !!one && one === mapConfig.originOf(endpoint)
}
const withQuery = (url, query) => url + (url.includes("?") ? "&" : "?") + query

// ===== the test =====
// One GET of the capabilities: {status, redirect, tooLarge, body()}. headers: a proxied provider's key header.
function fetchCapabilities(url, denyInternal, headers) {
    let client = ec.service
        .rest()
        .method("GET")
        .uri(url)
        .acceptContentType("application/xml, text/xml")
        .timeout(10)
        .followRedirects(false)
    for (const [name, value] of Object.entries(headers || {})) client = client.addHeader(name, value)
    client = denyInternal ? client.denyInternal() : client.allowInternal()
    const response = client.send()
    const status = Number(response.getStatusCode())
    if (status >= 300 && status < 400) return {status, redirect: true}
    const length = Number(response.headerFirst("content-length"))
    if (Number.isFinite(length) && length > MAX_BYTES) return {status, tooLarge: true}
    return {
        status,
        body: () => {
            const found = response.text(MAX_BYTES + 1)
            if (found.length > MAX_BYTES) throw new Error("too large")
            return found
        }
    }
}
// Whether a provider browsers ask themselves answers CORS: one sample request (a small GetMap, a tile, one feature) with an
// Origin, its Access-Control-Allow-Origin read; false when it cannot be asked
function corsAnswered(capabilities, denyInternal) {
    const layer = capabilities.layers[0]
    let url = ""
    if (capabilities.service === "wms") {
        const crs = layer.crs.find(mercator) || layer.crs[0]
        const box = mercator(crs)
            ? "-20037508,-20037508,20037508,20037508"
            : CRS84.includes(crs)
              ? "-180,-90,180,90"
              : "-90,-180,90,180"
        url = withQuery(
            capabilities.url,
            "SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=" +
                encodeURIComponent(layer.name) +
                "&STYLES=&FORMAT=" +
                encodeURIComponent(capabilities.formats[0] || "image/png") +
                "&CRS=" +
                encodeURIComponent(crs) +
                "&BBOX=" +
                box +
                "&WIDTH=64&HEIGHT=64"
        )
    } else if (capabilities.service === "wmts") {
        const set = capabilities.matrixSets[layer.matrixSets[0]]
        const matrix = set?.TileMatrix?.[0]?.Identifier || "0"
        const style = layer.styles.find(each => each.isDefault)?.name || layer.styles[0]?.name || "default"
        url = layer.template
            ? layer.template.replace(/\{(\w+)\}/g, (match, name) =>
                  encodeURIComponent(
                      {
                          layer: layer.name,
                          style,
                          tilematrixset: set?.Identifier || "",
                          tilematrix: matrix,
                          tilerow: "0",
                          tilecol: "0"
                      }[name.toLowerCase()] ??
                          (layer.time?.default || "default")
                  )
              )
            : withQuery(
                  capabilities.url,
                  "SERVICE=WMTS&VERSION=1.0.0&REQUEST=GetTile&LAYER=" +
                      encodeURIComponent(layer.name) +
                      "&STYLE=" +
                      encodeURIComponent(style) +
                      "&FORMAT=" +
                      encodeURIComponent(layer.formats[0] || "image/png") +
                      "&TILEMATRIXSET=" +
                      encodeURIComponent(set?.Identifier || "") +
                      "&TILEMATRIX=" +
                      encodeURIComponent(matrix) +
                      "&TILEROW=0&TILECOL=0"
              )
    } else
        url = withQuery(
            capabilities.url,
            "SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=" +
                encodeURIComponent(layer.name) +
                "&OUTPUTFORMAT=" +
                encodeURIComponent(layer.formats[0]) +
                "&COUNT=1"
        )
    try {
        let client = ec.service
            .rest()
            .method("GET")
            .uri(url)
            .timeout(10)
            .followRedirects(false)
            .addHeader("Origin", "https://moqui.invalid")
        client = denyInternal ? client.denyInternal() : client.allowInternal()
        const response = client.send()
        return !!text(response.headerFirst("access-control-allow-origin"))
    } catch (error) {
        return false
    }
}
// The test of an OGC provider: [check rows], with {capabilities, rules} for MapProviders.js to record when every row passed.
// helpers: {withKey(url, provider), queryWithoutKey(query, provider), shownOrigin(origin), unsafePath(path)}; context:
// {denyInternal, proxied}.
function test(provider, context, helpers) {
    const kind = serviceOf(provider)
    const endpoint = text(provider.endpointUrl)
    const place = helpers.shownOrigin(mapConfig.originOf(endpoint))
    const name = NAMES[kind]
    const row = (check, ok, message, values) => ({check, ok, text: say(message, Object.assign({place, name}, values || {}))})
    if (!kind || !splitUrl(endpoint))
        return {rows: [row("Capabilities", false, "The service's URL is not an http(s) URL with a host and no credentials")]}
    const asked = capabilitiesUrl(kind, endpoint)
    const keyed = context.proxied ? helpers.withKey(asked, provider) : {url: asked, headers: {}}
    const answer = fetchCapabilities(keyed.url, context.denyInternal, keyed.headers)
    if (answer.redirect)
        return {
            rows: [
                row(
                    "Capabilities",
                    false,
                    "${place} redirected its ${name} capabilities: a provider's URL is the one it answers at"
                )
            ]
        }
    if (answer.tooLarge) return {rows: [row("Capabilities", false, "${place} answered ${name} capabilities of more than 8 MiB")]}
    if (answer.status < 200 || answer.status > 299)
        return {rows: [row("Capabilities", false, "${place} answered HTTP ${status}", {status: answer.status})]}
    let body
    try {
        body = answer.body()
    } catch (error) {
        return {rows: [row("Capabilities", false, "${place} answered ${name} capabilities of more than 8 MiB")]}
    }
    // read in Rust (read#OgcCapabilities, moqui-lib geo/ogc.rs): a DOCTYPE, an entity declaration or an unknown entity
    // does not parse, so no DTD or external entity is read; a large document is no tree in this script's memory
    const was = ec.artifactExecution.disableAuthz()
    let answered
    try {
        answered =
            ec.service
                .sync()
                .name("org.moqui.maps.MapServices.read#OgcCapabilities")
                .parameters({service: kind, text: body, maxLayers: MAX_LAYERS})
                .call() || {}
    } finally {
        if (!was) ec.artifactExecution.enableAuthz()
    }
    body = null
    if (!answered.read)
        return {
            rows: [
                row("Capabilities", false, "${place} answered ${what}", {what: L(text(answered.refused) || "nothing it reads")})
            ]
        }
    const read = expanded(JSON.parse(text(answered.capabilitiesJson) || "null"))
    if (!read) return {rows: [row("Capabilities", false, "${place} answered ${what}", {what: L("nothing it reads")})]}
    const drawn = read.layers.filter(layer => layer.crs.length && (kind !== "wfs" || layer.formats.length))
    if (!drawn.length)
        return {
            rows: [
                row(
                    "Layers",
                    false,
                    kind === "wfs"
                        ? "${place} names no feature type a map reads (GeoJSON output, in EPSG:3857 or EPSG:4326)"
                        : "${place} names no layer a map draws (in EPSG:3857 or EPSG:4326)"
                )
            ]
        }
    if (kind !== "wmts" && !read.formats.length && !drawn.some(layer => (layer.formats || []).length))
        return {
            rows: [
                row("Formats", false, kind === "wfs" ? "${place} offers no GeoJSON output" : "${place} offers no image format")
            ]
        }
    // where a map asks: the operation's own address on the provider's origin, else the endpoint's
    const base = read.href && sameOrigin(read.href, endpoint) ? serviceBase(read.href) : serviceBase(endpoint)
    const rows = [
        row(
            "Capabilities",
            true,
            "${place} answered ${name} ${version} capabilities of ${count} layers; no credentials were sent",
            {
                version: read.version || VERSIONS[kind],
                count: drawn.length
            }
        )
    ]
    if (read.href && !sameOrigin(read.href, endpoint))
        rows.push(
            row("Hosts", true, "Its capabilities name another host for its requests: maps ask ${place}, the service's own URL")
        )
    const kvp = kind === "wmts" ? read.kvp && !restfulCapabilities(kind, endpoint) : true
    if (kind === "wmts") {
        // a tile template of another host is not used, nor through this server one with a query or a placeholder the proxy
        // does not know; KVP at the provider's own address is, when the service has it
        for (const layer of drawn) {
            const template = layer.template ? splitUrl(layer.template) : null
            if (
                !template ||
                !sameOrigin(layer.template, endpoint) ||
                (context.proxied && (template.rawQuery || !templatePattern(template.path)))
            )
                layer.template = ""
        }
        const usable = drawn.filter(layer => layer.template || kvp)
        if (!usable.length)
            return {
                rows: rows.concat([
                    row(
                        "Tiles",
                        false,
                        "${place} names no tile address on its own host this server can reach (KVP GetTile or a tile template)"
                    )
                ])
            }
        read.layers = usable
    } else read.layers = drawn
    const capabilities = {
        service: kind,
        version: read.version || VERSIONS[kind],
        url: base,
        kvp,
        formats: read.formats,
        layers: read.layers,
        matrixSets: read.matrixSets || {}
    }
    // a provider browsers ask themselves: whether its images, tiles or features may be read across origins
    if (!context.proxied) {
        capabilities.cors = corsAnswered(capabilities, context.denyInternal)
        if (!capabilities.cors) {
            if (kind === "wfs")
                return {
                    rows: rows.concat([
                        row(
                            "CORS",
                            false,
                            "${place} does not answer CORS: a browser cannot read its features; reach it through this server"
                        )
                    ])
                }
            rows.push(
                row(
                    "CORS",
                    true,
                    "${place} does not answer CORS: its images load as plain images, which send the browser's cookies of its site; reach it through this server to avoid that"
                )
            )
        }
    }
    // a proxied provider: the paths its requests are sent to, the rules of the proxy
    const rules = []
    if (context.proxied) {
        const parts = splitUrl(base)
        if (!parts || parts.path === "/" || helpers.unsafePath(parts.path))
            return {rows: rows.concat([row("Proxy", false, "The service's path is not one this server reaches for a map")])}
        const fixed = helpers.queryWithoutKey(parts.query, provider)
        if (kvp) rules.push(kind + " " + parts.path + (fixed ? "?" + fixed : ""))
        if (kind === "wmts")
            for (const template of unique(read.layers.map(layer => layer.template))) {
                const parts = splitUrl(template)
                if (parts && !helpers.unsafePath(parts.path)) rules.push("wmtsrest " + parts.path)
            }
    }
    // shared lists are written once again (a WMS's inherited CRSs and styles)
    return {rows, capabilities: compacted(capabilities), rules: unique(rules).slice(0, 200)}
}

// ===== what was recorded =====
// A WMS's layers name their CRS and style lists by index (crsLists, styleLists: what children inherit, kept once); read
// with each list in place, the same array for every layer that names it
function expanded(found) {
    if (!found || typeof found !== "object" || !Array.isArray(found.layers)) return null
    const listAt = (lists, at) =>
        typeof at === "number" ? (Array.isArray(lists) && Array.isArray(lists[at]) ? lists[at] : []) : at
    for (const layer of found.layers) {
        layer.crs = listAt(found.crsLists, layer.crs) || []
        layer.styles = listAt(found.styleLists, layer.styles) || []
        layer.formats = Array.isArray(layer.formats) ? layer.formats : []
        layer.matrixSets = Array.isArray(layer.matrixSets) ? layer.matrixSets : []
    }
    delete found.crsLists
    delete found.styleLists
    if (!found.matrixSets || typeof found.matrixSets !== "object") found.matrixSets = {}
    return found
}
// The capabilities as kept: each CRS and style list a layer has written once, layers naming it by index
function compacted(capabilities) {
    const lists = {crs: [], styles: []}
    const keys = {crs: new Map(), styles: new Map()}
    const at = (field, list) => {
        const key = JSON.stringify(list)
        if (!keys[field].has(key)) {
            keys[field].set(key, lists[field].length)
            lists[field].push(list)
        }
        return keys[field].get(key)
    }
    const layers = capabilities.layers.map(layer =>
        Object.assign({}, layer, {crs: at("crs", layer.crs), styles: at("styles", layer.styles)})
    )
    return Object.assign({}, capabilities, {layers, crsLists: lists.crs, styleLists: lists.styles})
}
// read once a call for a provider's revision
const parsed = new Map()
function capabilitiesOf(provider) {
    const key = text(provider?.providerId) + "|" + text(provider?.lastUpdatedStamp)
    if (parsed.has(key)) return parsed.get(key)
    let found = null
    try {
        found = expanded(JSON.parse(text(provider?.capabilitiesJson) || "null"))
    } catch (error) {
        found = null
    }
    if (parsed.size > 20) parsed.clear()
    parsed.set(key, found)
    return found
}

// ===== a layer a map draws =====
const listOf = value =>
    text(value)
        .split(",")
        .map(each => each.trim())
        .filter(each => each)
const maxFeatures = () => {
    const found = Number(ec.factory.getConfValue("maps@max-features"))
    return Number.isInteger(found) && found > 0 ? found : 5000
}
const refusal = (reasonCode, reason) => ({ok: false, reasonCode, reason})
// The CRS a layer is drawn in: the one configured when the layer has it, else EPSG:3857 when it has it, else (a WFS layer,
// read longitude first) CRS84 or the short EPSG:4326, else its first
function chosenCrs(configured, offered, kind) {
    if (configured) return offered.find(code => code === configured) || null
    return (
        offered.find(mercator) ||
        (kind === "wfs" ? offered.find(code => CRS84.includes(code)) || offered.find(code => code === "EPSG:4326") : null) ||
        offered[0] ||
        null
    )
}
// The source of an OGC layer from what its provider's last test recorded: {ok, url, ogc} (url the address a map asks: the
// service's, a WMTS tile template, or the proxy's path) or a refusal. A layer, style, format, matrix set or CRS the
// capabilities do not name is refused (the provider is tested again, or the layer changed).
function layerSource(layer, provider, proxyPath) {
    const kind = kindOfFormat(layer.formatEnumId)
    if (!kind || serviceOf(provider) !== kind)
        return refusal("KIND_UNSUPPORTED", say("its provider is not a ${name} service", {name: (kind && NAMES[kind]) || "OGC"}))
    const capabilities = capabilitiesOf(provider)
    const untested = refusal("PROVIDER_UNTESTED", L("its provider's last test did not record it: test the provider again"))
    if (!capabilities || capabilities.service !== kind) return untested
    const names = listOf(layer.ogcLayers)
    if (!names.length) return refusal("SOURCE_MISSING", L("it names no layer of its service"))
    const found = names.map(name => capabilities.layers.find(each => each.name === name))
    if (found.some(each => !each) || (kind !== "wms" && names.length > 1)) return untested
    const first = found[0]
    // the CRSs every named layer has
    const offered = first.crs.filter(code => found.every(each => each.crs.includes(code)))
    const crs = chosenCrs(text(layer.crs), offered, kind)
    if (!crs) return refusal("CRS_UNSUPPORTED", L("its CRS is not one its service offers for it that a map draws"))
    const time = text(layer.ogcTime)
    if (time && (!found.every(each => each.time) || !/^[0-9A-Za-z:.\/,+-]{1,64}$/.test(time)))
        return refusal("TIME_UNSUPPORTED", L("it names a time its layer has no dimension for"))
    const proxied = !!proxyPath
    const url = proxied ? proxyPath + (splitUrl(capabilities.url)?.path || "") : capabilities.url
    // a provider a browser asks itself is asked without the page's credentials only when it answers CORS
    const cors = proxied || capabilities.cors !== false
    if (kind === "wms") {
        const styles = listOf(layer.ogcStyles)
        if (styles.length > names.length || styles.some((style, index) => !found[index].styles.some(each => each.name === style)))
            return refusal("STYLE_UNSUPPORTED", L("it names a style its layer does not have"))
        const format =
            text(layer.ogcFormat) || (capabilities.formats.includes("image/png") ? "image/png" : capabilities.formats[0])
        if (!capabilities.formats.includes(format))
            return refusal("FORMAT_UNSUPPORTED", L("its format is not one its service offers"))
        const params = {
            LAYERS: names.join(","),
            STYLES: names.map((name, index) => styles[index] || "").join(","),
            FORMAT: format,
            TRANSPARENT: layer.isBasemap === "Y" ? "FALSE" : "TRUE",
            VERSION: "1.3.0"
        }
        if (time) params.TIME = time
        return {ok: true, url, ogc: {service: "wms", params, projection: crs, singleImage: layer.ogcSingleImage === "Y", cors}}
    }
    if (kind === "wmts") {
        const sets = first.matrixSets.filter(id => own(capabilities.matrixSets, id))
        const matrixSet =
            text(layer.ogcMatrixSet) || sets.find(id => capabilities.matrixSets[id].SupportedCRS === crs) || sets[0] || ""
        const set = sets.includes(matrixSet) ? capabilities.matrixSets[matrixSet] : null
        if (!set || !Array.isArray(set.TileMatrix))
            return refusal("MATRIX_SET_UNSUPPORTED", L("its matrix set is not one its layer is linked to"))
        const style =
            text(layer.ogcStyles) || first.styles.find(each => each.isDefault)?.name || first.styles[0]?.name || "default"
        if (first.styles.length && !first.styles.some(each => each.name === style))
            return refusal("STYLE_UNSUPPORTED", L("it names a style its layer does not have"))
        const format =
            text(layer.ogcFormat) ||
            first.templateFormat ||
            (first.formats.includes("image/png") ? "image/png" : first.formats[0]) ||
            "image/png"
        if (first.formats.length && !first.formats.includes(format))
            return refusal("FORMAT_UNSUPPORTED", L("its format is not one its layer offers"))
        const rest = !!first.template && (!capabilities.kvp || (!!first.templateFormat && first.templateFormat === format))
        if (!rest && !capabilities.kvp) return untested
        const ogc = {
            service: "wmts",
            layer: first.name,
            style,
            format,
            matrixSet: set,
            projection: set.SupportedCRS,
            requestEncoding: rest ? "REST" : "KVP",
            cors
        }
        if (first.time) ogc.dimensions = {Time: time || first.time.default || "default"}
        if (!rest) return {ok: true, url, ogc}
        const template = splitUrl(first.template)
        if (!template) return untested
        return {ok: true, url: proxied ? proxyPath + template.path : first.template, ogc, template: template.path}
    }
    const format = text(layer.ogcFormat) || first.formats[0]
    if (!first.formats.includes(format))
        return refusal("FORMAT_UNSUPPORTED", L("its format is not a GeoJSON output its service offers"))
    return {
        ok: true,
        url,
        ogc: {
            service: "wfs",
            typeName: first.name,
            outputFormat: format,
            srsName: crs,
            version: "2.0.0",
            maxFeatures: Math.min(maxFeatures(), WFS_COUNT),
            cors
        }
    }
}

// ===== the proxy =====
const permittedRow = row => row.disabled !== "Y" && (!text(row.viewPermission) || ec.user.hasPermission(row.viewPermission))
// The provider's layers the caller may be offered: enabled, its viewPermission held, and the basemap or a layer of an enabled
// profile whose viewPermission they hold, or any basemap layer when such a profile lets its user choose another basemap
function offeredLayers(provider) {
    const cached = entity => ec.entity.find(entity).useCache(true).disableAuthz()
    const layers = Array.from(cached(LAYER).condition("providerId", provider.providerId).limit(200).list()).filter(permittedRow)
    if (!layers.length) return []
    const ids = layers.map(row => String(row.layerId))
    const profileLayers = Array.from(cached(PROFILE_LAYER).condition("layerId", "in", ids).limit(2000).list())
    const cf = ec.entity.conditionFactory
    const either = [cf.makeCondition("basemapLayerId", "in", ids), cf.makeCondition("allowedOverrides", "like", "%basemap%")]
    const profileIds = unique(profileLayers.map(row => String(row.profileId)))
    if (profileIds.length) either.push(cf.makeCondition("profileId", "in", profileIds))
    const profiles = Array.from(cached(PROFILE).condition(cf.makeCondition(either, "or")).limit(1000).list()).filter(permittedRow)
    const shownIn = new Set(profiles.map(row => String(row.profileId)))
    const offered = new Set(profiles.map(row => String(row.basemapLayerId)))
    for (const row of profileLayers) if (shownIn.has(String(row.profileId))) offered.add(String(row.layerId))
    const anyBasemap = profiles.some(row => listOf(row.allowedOverrides).includes("basemap"))
    return layers.filter(row => offered.has(String(row.layerId)) || (anyBasemap && row.isBasemap === "Y"))
}
const escape = value => String(value).replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
// a path's segment as the proxy writes a request's (MapProviders.js proxyPathOf): encoded, but @, , and : as they are
const segment = value => encodeURIComponent(value).replace(/%40/g, "@").replace(/%2C/g, ",").replace(/%3A/gi, ":")
// a literal part of a template's path written as the proxy writes a request's path
const canonical = part =>
    part
        .split("/")
        .map(piece => segment(decodeName(piece)))
        .join("/")
// The pattern of a WMTS tile template's path for one layer: its layer, style and matrix set as recorded for that layer, a
// matrix of the set, whole numbers for the row and column, a time's characters; null for another placeholder
function restPattern(templatePath, ogc) {
    let bad = false
    const source = String(templatePath)
        .split(/(\{[A-Za-z]+\})/)
        .map(part => {
            const name = /^\{([A-Za-z]+)\}$/.exec(part)?.[1]?.toLowerCase()
            if (!name) return escape(canonical(part))
            if (name === "tilerow" || name === "tilecol") return "\\d{1,9}"
            if (name === "layer") return escape(segment(ogc.layer))
            if (name === "style") return escape(segment(ogc.style))
            if (name === "tilematrixset") return escape(segment(ogc.matrixSet.Identifier))
            if (name === "tilematrix")
                return "(?:" + ogc.matrixSet.TileMatrix.map(matrix => escape(segment(matrix.Identifier))).join("|") + ")"
            if (name === "time") return "[0-9A-Za-z:.,+-]{1,64}"
            bad = true
            return ""
        })
        .join("")
    return bad ? null : "^" + source + "$"
}
// Whether a WMTS tile template's path is one the proxy reaches (the placeholders it knows)
const templatePattern = templatePath =>
    !/\{(?!(?:TileMatrix|TileMatrixSet|TileRow|TileCol|Style|Layer|Time)\})[^}]*\}/i.test(templatePath)
// What the caller's requests through the proxy may ask of the provider: for each layer they may be offered, its own names,
// style, format and matrix set (wms, wmts, wfs) and its tile template's pattern (rest). Decided once a short while
// (maps.provider.proxy, 30 seconds) for the user and the provider's revision, so a change of a layer, a profile or a
// permission reaches it within that.
function allowedRequests(provider) {
    const key =
        "ogc|" +
        ec.resource.digestText([text(ec.user.userId), provider.providerId, text(provider.lastUpdatedStamp)].join("\n"), "sha256")
    const cache = ec.cache.getCache("maps.provider.proxy")
    const kept = cache.get(key)
    if (kept) {
        try {
            return JSON.parse(String(kept))
        } catch (error) {
            // decided again
        }
    }
    const allowed = {wms: [], wmts: [], wfs: [], rest: []}
    const capabilities = capabilitiesOf(provider)
    for (const row of capabilities ? offeredLayers(provider) : []) {
        const built = layerSource(row, provider, "/proxy")
        if (!built.ok) continue
        const ogc = built.ogc
        if (ogc.service === "wms")
            allowed.wms.push({LAYERS: ogc.params.LAYERS, STYLES: ogc.params.STYLES, FORMAT: ogc.params.FORMAT})
        else if (ogc.service === "wfs")
            allowed.wfs.push({TYPENAMES: ogc.typeName, OUTPUTFORMAT: ogc.outputFormat, SRSNAME: ogc.srsName})
        else {
            if (capabilities.kvp)
                allowed.wmts.push({
                    LAYER: ogc.layer,
                    STYLE: ogc.style,
                    FORMAT: ogc.format,
                    TILEMATRIXSET: ogc.matrixSet.Identifier,
                    TILEMATRIX: ogc.matrixSet.TileMatrix.map(matrix => matrix.Identifier)
                })
            const template = capabilities.layers.find(each => each.name === ogc.layer)?.template
            const pattern = template ? restPattern(splitUrl(template)?.path || "", ogc) : null
            if (pattern) allowed.rest.push(pattern)
        }
    }
    cache.put(key, JSON.stringify(allowed))
    return allowed
}
// Whether a request's path matches a tile template of a layer the caller may be offered (a wmtsrest rule's)
const restAllowed = (provider, path) => allowedRequests(provider).rest.some(pattern => new RegExp(pattern).test(path))

// A request's parameters checked by themselves, by their upper-case names; the names of what is drawn (IDENTITY) are compared
// with a layer the caller may be offered
const plain = /^[^\u0000-\u001f\u007f]{0,1024}$/
const decimal = /^[-+]?(\d{1,15}(\.\d{0,15})?|\.\d{1,15})([eE][-+]?\d{1,3})?$/
const decimals = (value, count) => {
    const parts = value.split(",")
    return parts.length === count && parts.every(part => decimal.test(part) && Number.isFinite(Number(part)))
}
const whole = (value, max) => /^\d{1,9}$/.test(value) && Number(value) >= 1 && Number(value) <= max
const nonNegative = (value, max) => /^\d{1,9}$/.test(value) && Number(value) <= max
const timeValue = value => /^[0-9A-Za-z:.\/,+-]{1,64}$/.test(value)
const IDENTITY = {
    wms: ["LAYERS", "STYLES", "FORMAT"],
    wmts: ["LAYER", "STYLE", "FORMAT", "TILEMATRIXSET", "TILEMATRIX"],
    wfs: ["TYPENAMES", "OUTPUTFORMAT", "SRSNAME"]
}
const PLAIN_CHECKS = {
    wms: {
        required: ["REQUEST", "LAYERS", "CRS", "BBOX", "WIDTH", "HEIGHT", "FORMAT"],
        SERVICE: v => v.toUpperCase() === "WMS",
        REQUEST: v => v === "GetMap",
        VERSION: v => v === "1.3.0",
        TRANSPARENT: v => /^(true|false)$/i.test(v),
        CRS: drawnCrs,
        BBOX: v => decimals(v, 4),
        WIDTH: v => whole(v, 4096),
        HEIGHT: v => whole(v, 4096),
        TIME: timeValue
    },
    wmts: {
        required: ["REQUEST", "LAYER", "STYLE", "FORMAT", "TILEMATRIXSET", "TILEMATRIX", "TILEROW", "TILECOL"],
        SERVICE: v => v.toUpperCase() === "WMTS",
        REQUEST: v => v === "GetTile",
        VERSION: v => v === "1.0.0",
        TILEROW: v => nonNegative(v, 1e8),
        TILECOL: v => nonNegative(v, 1e8),
        TIME: timeValue
    },
    wfs: {
        required: ["REQUEST", "TYPENAMES", "OUTPUTFORMAT", "SRSNAME", "COUNT"],
        SERVICE: v => v.toUpperCase() === "WFS",
        REQUEST: v => v === "GetFeature",
        VERSION: v => v === "2.0.0",
        // four numbers, and the CRS they are in, when given, the request's
        BBOX: (v, all) => {
            const parts = v.split(",")
            return (
                decimals(parts.slice(0, 4).join(","), 4) &&
                (parts.length === 4 || (parts.length === 5 && parts[4] === all.SRSNAME))
            )
        },
        COUNT: v => whole(v, maxFeatures())
    }
}
// The same number of styles as layers, a missing one empty
const stylesOf = (styles, layers) => {
    const given = String(styles ?? "").split(",")
    return String(layers)
        .split(",")
        .map((name, index) => given[index] || "")
        .join(",")
}
function sameIdentity(kind, given, allowed) {
    if (kind === "wms")
        return (
            given.LAYERS === allowed.LAYERS &&
            stylesOf(given.STYLES, given.LAYERS) === allowed.STYLES &&
            String(given.STYLES ?? "").split(",").length <= given.LAYERS.split(",").length &&
            given.FORMAT === allowed.FORMAT
        )
    if (kind === "wmts")
        return (
            given.LAYER === allowed.LAYER &&
            given.STYLE === allowed.STYLE &&
            given.FORMAT === allowed.FORMAT &&
            given.TILEMATRIXSET === allowed.TILEMATRIXSET &&
            Array.isArray(allowed.TILEMATRIX) &&
            allowed.TILEMATRIX.includes(given.TILEMATRIX)
        )
    return (
        given.TYPENAMES === allowed.TYPENAMES && given.OUTPUTFORMAT === allowed.OUTPUTFORMAT && given.SRSNAME === allowed.SRSNAME
    )
}
// The query the proxy sends for a request of an OGC rule (wms, wmts, wfs), or null when it is refused: the browser's
// parameters, each a name its kind's requests carry, given once (whatever its case), what is drawn one of a layer the caller
// may be offered and the others checked by themselves; then the rule's own fixed query. Names are sent upper case, values
// as given (re-encoded).
function proxyQuery(provider, kind, queryString, fixedQuery) {
    if (!own(PLAIN_CHECKS, kind)) return null
    const rules = PLAIN_CHECKS[kind]
    const names = IDENTITY[kind]
    const given = {}
    for (const pair of text(queryString).split("&")) {
        if (!pair) continue
        const at = pair.indexOf("=")
        const name = decodeName(at < 0 ? pair : pair.slice(0, at)).toUpperCase()
        const value = decodeName(at < 0 ? "" : pair.slice(at + 1))
        const known = name !== "REQUIRED" && (own(rules, name) || names.includes(name))
        if (!known || own(given, name) || !plain.test(value)) return null
        given[name] = value
    }
    for (const name of rules.required) if (!own(given, name)) return null
    for (const [name, value] of Object.entries(given)) if (!names.includes(name) && !rules[name](value, given)) return null
    if (!allowedRequests(provider)[kind].some(allowed => sameIdentity(kind, given, allowed))) return null
    const query = Object.entries(given).map(([name, value]) => name + "=" + encodeURIComponent(value))
    return (text(fixedQuery) ? [text(fixedQuery)] : []).concat(query).join("&")
}

// ===== Configuration: the choices of Add OGC layer =====
// What a tested OGC provider's capabilities offer: {layers: [{value, label}]} or, for one layer, its styles, formats, CRSs,
// matrix sets and whether it has a time dimension
function layerOptions(provider, layerName) {
    const capabilities = capabilitiesOf(provider)
    if (!capabilities) return {layers: [], styles: [], formats: [], crs: [], matrixSets: [], time: null}
    const layer = capabilities.layers.find(each => each.name === text(layerName)) || null
    return {
        service: capabilities.service,
        layers: capabilities.layers.map(each => ({
            value: each.name,
            label: each.title && each.title !== each.name ? each.title + " (" + each.name + ")" : each.name
        })),
        styles: layer ? layer.styles.map(each => ({value: each.name, label: each.title || each.name})) : [],
        formats: (layer && layer.formats.length ? layer.formats : capabilities.formats).map(each => ({value: each, label: each})),
        crs: layer ? layer.crs.map(each => ({value: each, label: each})) : [],
        matrixSets: layer
            ? layer.matrixSets
                  .filter(each => own(capabilities.matrixSets, each))
                  .map(each => ({value: each, label: each + " (" + capabilities.matrixSets[each].SupportedCRS + ")"}))
            : [],
        time: layer?.time || null,
        title: layer?.title || ""
    }
}

;({
    isOgcKind,
    kindOfFormat,
    serviceOf,
    test,
    capabilitiesOf,
    layerSource,
    proxyQuery,
    restAllowed,
    layerOptions,
    drawnCrs
})
