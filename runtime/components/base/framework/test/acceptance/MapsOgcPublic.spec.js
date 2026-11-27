// MapsOgcPublic (plans/archive/Maps.md §4.1, P4a): real public OGC services through the OGC provider kinds (MapOgc.js), nothing
// proxied and no key: the USGS National Map's topographic WMTS (a small RESTful capabilities document, one layer, public
// domain) and NASA GIBS's WMS 1.3.0 (about 2.6 MB of capabilities, some 1,600 layers with time dimensions). Each is tested
// (its capabilities read with the framework's XML reader and recorded), a layer added from what was recorded
// (store#OgcLayer, as Configuration's Add OGC layer does), resolved by the effective configuration, and one tile or image
// asked the way m-map asks it (the WMTS template filled from the recorded matrix set, the WMS GetMap from the layer's
// params), which must answer an image. Times and sizes are logged; the rows are removed afterwards.
// Run when asked: ./moq accept framework --spec MapsOgcPublic
const http = ec.factory.getTool("HttpFixtures")
const C = "org.moqui.admin.ConfigurationServices."
const M = "org.moqui.maps.MapServices."
const USGS = "MAPS_OGC_PUBLIC_USGS"
const GIBS = "MAPS_OGC_PUBLIC_GIBS"
const log = (label, value) => ec.logger.warn("MapsOgcPublic: " + label + " " + JSON.stringify(value))
const providerRow = providerId =>
    ec.entity.find("moqui.maps.MapProvider").condition("providerId", providerId).useCache(false).one()
const created = []
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
function testProvider(providerId) {
    const started = Date.now()
    const out = admin(() =>
        ec.service
            .sync()
            .name(C + "test#Connection")
            .parameters({adapterKey: "mapProvider", connectionKey: providerId})
            .call()
    )
    const capabilities = JSON.parse(String(providerRow(providerId).capabilitiesJson || "null"))
    log(providerId + " test", {
        passed: out?.passed,
        ms: Date.now() - started,
        layers: capabilities?.layers?.length,
        bytes: String(providerRow(providerId).capabilitiesJson || "").length,
        checks: out?.checkList
    })
    return {out, capabilities}
}
function addLayer(parameters) {
    const out = admin(() =>
        ec.service
            .sync()
            .name(M + "store#OgcLayer")
            .parameters(parameters)
            .call()
    )
    if (out?.layerId) created.push(out.layerId)
    return out
}
function layerOf(layerId) {
    const started = Date.now()
    const config = admin(() =>
        ec.service
            .sync()
            .name(M + "get#EffectiveConfig")
            .parameters({layerIds: layerId})
            .call()
    )?.config
    log(layerId + " effective configuration", {ms: Date.now() - started})
    return {layer: (config?.layers || []).find(each => each.id == layerId), errors: config?.layerErrors || []}
}
function image(url) {
    const started = Date.now()
    const response = ec.service.rest().method("GET").uri(url).timeout(30).followRedirects(false).denyInternal().send()
    const answer = {
        status: Number(response.getStatusCode()),
        type: String(response.getContentType() || ""),
        bytes: response.bytes(4 * 1024 * 1024).length
    }
    log("asked " + url.replace(/\?.*$/, "?..."), Object.assign({ms: Date.now() - started}, answer))
    return answer
}

spec({
    timeout: 300,
    setupSpec() {
        // outbound requests opened to these services alone (moqui_core::outbound)
        http.allowNetwork(["https://basemap.nationalmap.gov/", "https://gibs.earthdata.nasa.gov/"])
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const provider = (providerId, providerKindEnumId, endpointUrl, attribution) =>
            ec.entity
                .makeValue("moqui.maps.MapProvider")
                .setAll({
                    providerId,
                    providerName: "Spec " + providerId,
                    providerKindEnumId,
                    endpointUrl,
                    attribution,
                    allowPrivateNetwork: "N",
                    disabled: "N"
                })
                .createOrUpdate()
        provider(
            USGS,
            "MPK_WMTS",
            "https://basemap.nationalmap.gov/arcgis/rest/services/USGSTopo/MapServer/WMTS/1.0.0/WMTSCapabilities.xml",
            "USGS The National Map"
        )
        provider(GIBS, "MPK_WMS", "https://gibs.earthdata.nasa.gov/wms/epsg3857/best/wms.cgi", "NASA EOSDIS GIBS")
    },
    cleanupSpec() {
        http.reset()
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const del = (entity, field, values) => values.length && ec.entity.find(entity).condition(field, "in", values).deleteAll()
        del("moqui.maps.MapLayer", "layerId", created)
        del("moqui.server.ConnectionCheck", "connectionKey", [USGS, GIBS])
        del("moqui.maps.MapProvider", "providerId", [USGS, GIBS])
    }
})

feature("the USGS topographic WMTS: its capabilities recorded, a basemap added from them, a tile asked by its template", {
    when() {
        const tested = testProvider(USGS)
        const layerName = tested.capabilities?.layers?.[0]?.name || ""
        const added = addLayer({providerId: USGS, ogcLayers: layerName, isBasemap: "N"})
        const {layer, errors} = layerOf(added?.layerId)
        let tile = null
        if (layer?.ogc?.requestEncoding == "REST") {
            const set = layer.ogc.matrixSet
            const matrix = set.TileMatrix.find(each => each.MatrixWidth >= 4) || set.TileMatrix[set.TileMatrix.length - 1]
            const url = layer.url.replace(
                /\{(\w+)\}/g,
                (m, name) =>
                    ({
                        style: layer.ogc.style,
                        tilematrixset: set.Identifier,
                        tilematrix: matrix.Identifier,
                        tilerow: "1",
                        tilecol: "1"
                    })[name.toLowerCase()] ?? m
            )
            tile = image(url)
        }
        return {tested, added, layer, errors, tile}
    },
    then({tested, added, layer, errors, tile}) {
        tested.out.passed == true
        tested.capabilities.layers.length >= 1
        added.applied == true
        errors.length == 0
        layer.kind == "wmts"
        layer.url.startsWith("https://basemap.nationalmap.gov/")
        layer.attribution.includes("USGS")
        tile.status == 200
        tile.type.startsWith("image/")
        tile.bytes > 100
    }
})

feature("NASA GIBS's WMS: its capabilities of some 1,600 layers recorded, a layer with a time added, an image of it asked", {
    when() {
        const tested = testProvider(GIBS)
        const wanted = "MODIS_Terra_CorrectedReflectance_TrueColor"
        const recorded = tested.capabilities?.layers?.find(each => each.name == wanted)
        const added = addLayer({providerId: GIBS, ogcLayers: wanted, ogcFormat: "image/jpeg", ogcTime: "2026-06-01"})
        const {layer, errors} = layerOf(added?.layerId)
        let map = null
        if (layer?.ogc?.params) {
            const params = Object.assign(
                {
                    SERVICE: "WMS",
                    REQUEST: "GetMap",
                    CRS: layer.ogc.projection,
                    WIDTH: "256",
                    HEIGHT: "256",
                    BBOX: "-20037508.34,-20037508.34,20037508.34,20037508.34"
                },
                layer.ogc.params
            )
            map = image(
                layer.url +
                    (layer.url.includes("?") ? "&" : "?") +
                    Object.entries(params)
                        .map(([k, v]) => k + "=" + encodeURIComponent(v))
                        .join("&")
            )
        }
        return {tested, recorded, added, layer, errors, map}
    },
    then({tested, recorded, added, layer, errors, map}) {
        tested.out.passed == true
        tested.capabilities.layers.length > 1000
        recorded.time.default != ""
        added.applied == true
        errors.length == 0
        layer.kind == "wms"
        layer.ogc.params.TIME == "2026-06-01"
        layer.ogc.params.FORMAT == "image/jpeg"
        map.status == 200
        map.type.startsWith("image/")
    }
})
