// MapsCogPublic (plans/Maps.md §4.1, P4c): a real public Cloud Optimized GeoTIFF at a provider's URL, nothing proxied and no
// key: the true-colour image (TCI.tif, about 224 MiB, 10980 pixels square, three bands of 8 bits) of a Sentinel-2 L2A scene
// in the Sentinel-2 COGs bucket on AWS Open Data, the one OpenLayers' own COG example draws (examples/cog.js), which answers
// ranges and CORS for any origin. A raster-tiles provider for the bucket's host is tested, the layer registered from the URL
// (register#ArchiveLayer: one range of its first 64 KiB, its header read in Rust), and the recorded header checked (tiled,
// with overviews, in its UTM zone's EPSG code, which the projection registry has, CORS allowed); the effective configuration
// hands the layer with the provider's URL and its projection's definition; then the full image's first tile is read by
// range through the outbound HTTP client, its offset and length taken from the file's first image directory. Times and sizes
// are logged; the rows are removed afterwards.
//
// The provider's test asks a raster-tiles provider for tile 0/0/0 as an image, and the bucket serves COGs, not tiles: the
// provider here names the scene's thumbnail (a JPEG on the same host) as its tile template, so that its test passes.
// Run when asked: ./moq accept framework --spec MapsCogPublic
const http = ec.factory.getTool("HttpFixtures")
const C = "org.moqui.admin.ConfigurationServices."
const M = "org.moqui.maps.MapServices."
const S = "org.moqui.maps.MapSourceServices."
const HOST = "https://sentinel-cogs.s3.us-west-2.amazonaws.com/"
// Sentinel-2A, 1 July 2020, MGRS tile 36QWD (UTM zone 36N, the Red Sea coast of Sudan)
const SCENE = HOST + "sentinel-s2-l2a-cogs/36/Q/WD/2020/7/S2A_36QWD_20200701_0_L2A/"
const COG_URL = SCENE + "TCI.tif"
const PROVIDER = "MAPS_COG_PUBLIC"
const LAYER = "MAPS_COG_PUBLIC_TCI"
const log = (label, value) => ec.logger.warn("MapsCogPublic: " + label + " " + JSON.stringify(value))
const providerRow = () => ec.entity.find("moqui.maps.MapProvider").condition("providerId", PROVIDER).useCache(false).one()
const layerRow = () => ec.entity.find("moqui.maps.MapLayer").condition("layerId", LAYER).useCache(false).one()
function admin(action) {
    ec.user.internalLoginUser("john.doe")
    ec.message.clearErrors()
    ec.artifactExecution.enableAuthz()
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.artifactExecution.disableAuthz()
    }
}
const call = (name, parameters) =>
    admin(() => {
        const out = ec.service.sync().name(name).parameters(parameters).call() || {}
        return Object.assign({}, out, {error: ec.message.hasError() ? ec.message.getErrorsString() : ""})
    })
// A range of the COG through the outbound HTTP client, as the browser's geotiff.js asks it
function range(from, to) {
    const started = Date.now()
    const response = ec.service
        .rest()
        .method("GET")
        .uri(COG_URL)
        .timeout(30)
        .followRedirects(false)
        .denyInternal()
        .addHeader("Range", "bytes=" + from + "-" + to)
        .send()
    const status = Number(response.getStatusCode())
    const bytes = status === 206 ? response.bytes(to - from + 1024) : new Uint8Array(0)
    const answer = {status, contentRange: String(response.headerFirst("content-range") || ""), length: bytes.length}
    log("range " + from + "-" + to, Object.assign({ms: Date.now() - started}, answer))
    return Object.assign(answer, {bytes})
}
// The first tile of the file's first image (the full resolution) from its first directory, read from the file's first bytes:
// {tiles, offset, length}, or {} when the directory or its arrays are not within them
function firstTile(bytes) {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
    const within = (at, size) => at >= 0 && at + size <= bytes.length
    if (!within(0, 16)) return {}
    const little = view.getUint16(0) === 0x4949
    const big = view.getUint16(2, little) === 43
    const u16 = at => view.getUint16(at, little)
    const u32 = at => view.getUint32(at, little)
    const u64 = at => Number(view.getBigUint64(at, little))
    const directory = big ? u64(8) : u32(4)
    if (!within(directory, 8)) return {}
    const count = big ? u64(directory) : u16(directory)
    const found = {}
    for (let i = 0; i < count; i++) {
        const entry = directory + (big ? 8 + i * 20 : 2 + i * 12)
        if (!within(entry, big ? 20 : 12)) return {}
        const tag = u16(entry)
        const size = {3: 2, 4: 4, 16: 8}[u16(entry + 2)]
        if ((tag !== 324 && tag !== 325) || !size) continue
        const values = big ? u64(entry + 4) : u32(entry + 4)
        const inline = values * size <= (big ? 8 : 4)
        const field = entry + (big ? 12 : 8)
        const at = inline ? field : big ? u64(field) : u32(field)
        if (!within(at, size)) return {}
        found[tag] = {values, first: size === 2 ? u16(at) : size === 4 ? u32(at) : u64(at)}
    }
    return found[324] && found[325] ? {tiles: found[324].values, offset: found[324].first, length: found[325].first} : {}
}

spec({
    timeout: 300,
    setupSpec() {
        // outbound requests opened to this host alone (moqui_core::outbound)
        http.allowNetwork([HOST])
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        ec.entity
            .makeValue("moqui.maps.MapProvider")
            .setAll({
                providerId: PROVIDER,
                providerName: "Spec Sentinel-2 COGs on AWS",
                providerKindEnumId: "MPK_RASTER_TILES",
                endpointUrl: SCENE + "thumbnail.jpg?tile={z}/{x}/{y}",
                attribution: "Contains modified Copernicus Sentinel data 2020",
                allowPrivateNetwork: "N",
                proxyRequests: "N",
                disabled: "N"
            })
            .createOrUpdate()
        ec.entity
            .makeValue("moqui.maps.MapLayer")
            .setAll({
                layerId: LAYER,
                layerName: "Spec Sentinel-2 true colour 36QWD",
                formatEnumId: "MLF_COG",
                sourceUrl: COG_URL,
                providerId: PROVIDER,
                isBasemap: "N",
                disabled: "Y"
            })
            .createOrUpdate()
    },
    cleanupSpec() {
        http.reset()
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const del = (entity, field, values) => ec.entity.find(entity).condition(field, "in", values).deleteAll()
        del("moqui.maps.MapLayer", "layerId", [LAYER])
        del("moqui.server.ConnectionCheck", "connectionKey", [PROVIDER])
        del("moqui.maps.MapProvider", "providerId", [PROVIDER])
    }
})

feature("a Sentinel-2 COG: its provider tested, the layer registered from its URL with its header recorded", {
    when() {
        let started = Date.now()
        const tested = call(C + "test#Connection", {adapterKey: "mapProvider", connectionKey: PROVIDER})
        log("provider test", {passed: tested.passed, ms: Date.now() - started, checks: tested.checkList})
        started = Date.now()
        const registered = call(S + "register#ArchiveLayer", {layerId: LAYER})
        const row = layerRow()
        const raster = JSON.parse(String(row.rasterJson || "{}"))
        log("registered", {
            applied: registered.applied,
            reason: registered.reason || registered.error,
            ms: Date.now() - started,
            fileBytes: registered.size,
            crs: row.crs,
            bounds: [row.minLongitude, row.minLatitude, row.maxLongitude, row.maxLatitude].map(Number),
            raster: Object.assign({}, raster, {statistics: undefined, defaults: undefined})
        })
        const projection = row.crs
            ? ec.entity.find("moqui.maps.MapProjection").condition("projectionId", row.crs).useCache(false).one()
            : null
        return {tested, registered, row, raster, projection}
    },
    then({tested, registered, row, raster, projection}) {
        tested.passed == true
        registered.applied === true && row.disabled == "N"
        // nothing of a provider's COG is pinned: no revision, no location
        !row.sourceRevision && !row.resourceLocation
        registered.size > 100 * 1024 * 1024
        // Sentinel-2 tiles are in their UTM zone: 36QWD is zone 36 north
        raster.epsg == row.crs && /^EPSG:32[67]\d\d$/.test(row.crs) && row.crs == "EPSG:32636"
        projection != null && projection.disabled != "Y" && String(projection.proj4Definition).includes("+zone=36")
        raster.cors === true
        raster.bands == 3 && raster.sampleType == "uint8" && raster.width == 10980 && raster.height == 10980
        raster.tileWidth >= 256 && raster.tileWidth % 16 == 0
        raster.overviews >= 3
        raster.defaults.rgb.join() == "1,2,3"
        // the scene's bounds in WGS84, its UTM edges converted: about 33.0 to 34.0°E, 16.2 to 17.2°N
        row.minLongitude > 32.9 && row.maxLongitude < 34.1 && row.minLatitude > 16.1 && row.maxLatitude < 17.3
    }
})

feature("the effective configuration offers the layer at the provider's URL, and its first tile is read by range", {
    when() {
        const started = Date.now()
        const config = call(M + "get#EffectiveConfig", {layerIds: LAYER}).config || {}
        log("effective configuration", {ms: Date.now() - started, layerErrors: config.layerErrors})
        const drawn = Array.from(config.layers || []).find(layer => layer.id == LAYER) || {}
        const projection = Array.from(config.projections || []).find(each => each.code == "EPSG:32636") || {}
        const head = range(0, 65535)
        const tile = firstTile(head.bytes)
        log("first tile of the full image", tile)
        const read = tile.length ? range(tile.offset, tile.offset + tile.length - 1) : {}
        return {drawn, projection, errors: Array.from(config.layerErrors || []), head, tile, read}
    },
    then({drawn, projection, errors, head, tile, read}) {
        errors.length == 0
        drawn.kind == "cog" && drawn.url == COG_URL
        drawn.raster.projection == "EPSG:32636" && drawn.raster.bands == 3
        drawn.attribution.includes("Copernicus")
        projection.proj4.includes("+zone=36")
        head.status == 206 && head.length == 65536 && head.contentRange.startsWith("bytes 0-65535/")
        tile.tiles > 100 && tile.offset > 0 && tile.length > 0
        read.status == 206 && read.length == tile.length
        read.contentRange.startsWith("bytes " + tile.offset + "-" + (tile.offset + tile.length - 1) + "/")
    }
})
