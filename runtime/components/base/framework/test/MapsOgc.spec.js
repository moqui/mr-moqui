// OGC map services (plans/Maps.md §4.1, P4a; MapOgc.js): a WMS, WMTS or WFS provider's test reads its GetCapabilities with
// the framework's XML reader and records what a map draws (capabilitiesJson) and, proxied, the rules of its requests; a
// DOCTYPE, an entity declaration or an unknown entity refuses the answer. The effective configuration draws an OGC layer
// from what was recorded, only for a user offered its profile; the proxy forwards an OGC request only for a registered layer
// the caller may be offered, with its own names, style, format and matrix set and only the parameters its kind carries; a
// URL with credentials (user@host) is never the provider's; Add OGC layer's services are Configuration's. The services are
// answered by the HTTP fixtures.
// Run: ./moq test framework --spec MapsOgc
const C = "org.moqui.admin.ConfigurationServices."
const http = ec.factory.getTool("HttpFixtures")
const WMS = "MAPS_OGC_WMS"
const WMTS = "MAPS_OGC_WMTS"
const WFS = "MAPS_OGC_WFS"
const WMTSP = "MAPS_OGC_WMTSP"
const PRIVATE = "MAPS_OGC_WFS_PRIVATE"
const PROFILE2 = "MAPS_OGC_PROFILE2"
const PROFILE = "MAPS_OGC_PROFILE"
const PERMISSION = "MAPS_OGC_VIEW"
const READERS = "MAPS_OGC_READERS"
const MANAGERS = "MAPS_OGC_MANAGERS"
const READER = "MAPS_OGC_READER"
const OTHER = "MAPS_OGC_OTHER"
const MANAGER = "MAPS_OGC_MANAGER"
const KEY = "spec-ogc-key-5521"
const WMS_URL = "https://wms.ogc.example/geoserver/ows?map=base"
const WMTS_URL = "https://wmts.ogc.example/wmts/1.0.0/WMTSCapabilities.xml"
const WFS_URL = "https://wfs.ogc.example/geoserver/wfs"
const WMTSP_URL = "https://wmtsp.ogc.example/wmts"
const URN_3857 = "urn:ogc:def:crs:EPSG::3857"
const PROXY = "/apps/maps/LocalMap/providerProxy/"

const wmsCapabilities = `<?xml version="1.0" encoding="UTF-8"?>
<WMS_Capabilities version="1.3.0" xmlns="http://www.opengis.net/wms" xmlns:xlink="http://www.w3.org/1999/xlink">
  <Service><Name>WMS</Name><Title>Spec WMS</Title></Service>
  <Capability>
    <Request><GetMap><Format>image/png</Format><Format>image/jpeg</Format><Format>text/html</Format>
      <DCPType><HTTP><Get><OnlineResource xlink:href="https://wms.ogc.example@evil.example/geoserver/ows?"/></Get></HTTP></DCPType></GetMap></Request>
    <Layer><Title>Root</Title><CRS>EPSG:2056</CRS>
      <Layer queryable="1"><Name>roads</Name><Title>Roads &amp; streets</Title><CRS>EPSG:4326</CRS><CRS>EPSG:3857</CRS>
        <EX_GeographicBoundingBox><westBoundLongitude>-10</westBoundLongitude><eastBoundLongitude>10</eastBoundLongitude>
          <southBoundLatitude>40</southBoundLatitude><northBoundLatitude>60</northBoundLatitude></EX_GeographicBoundingBox>
        <Style><Name>night</Name><Title>Night</Title></Style>
        <Dimension name="time" units="ISO8601" default="2026-11-01">2026-01-01/2026-11-01/P1D</Dimension>
      </Layer>
      <Layer><Name>swiss</Name><Title>Swiss only</Title><CRS>EPSG:2056</CRS></Layer>
    </Layer>
  </Capability>
</WMS_Capabilities>`
const matrices = [0, 1]
    .map(
        z => `<TileMatrix><ows:Identifier>${z}</ows:Identifier><ScaleDenominator>${559082264.0287178 / Math.pow(2, z)}</ScaleDenominator>
      <TopLeftCorner>-20037508.3427892 20037508.3427892</TopLeftCorner><TileWidth>256</TileWidth><TileHeight>256</TileHeight>
      <MatrixWidth>${Math.pow(2, z)}</MatrixWidth><MatrixHeight>${Math.pow(2, z)}</MatrixHeight></TileMatrix>`
    )
    .join("")
const wmtsCapabilities = `<?xml version="1.0" encoding="UTF-8"?>
<Capabilities version="1.0.0" xmlns="http://www.opengis.net/wmts/1.0" xmlns:ows="http://www.opengis.net/ows/1.1">
  <Contents>
    <Layer><ows:Title>Topo</ows:Title><ows:Identifier>topo</ows:Identifier>
      <ows:WGS84BoundingBox><ows:LowerCorner>-180 -85</ows:LowerCorner><ows:UpperCorner>180 85</ows:UpperCorner></ows:WGS84BoundingBox>
      <Style isDefault="true"><ows:Identifier>default</ows:Identifier></Style><Format>image/png</Format>
      <TileMatrixSetLink><TileMatrixSet>GoogleMapsCompatible</TileMatrixSet></TileMatrixSetLink>
      <ResourceURL format="image/png" resourceType="tile" template="https://wmts.ogc.example/wmts/topo/{Style}/{TileMatrixSet}/{TileMatrix}/{TileRow}/{TileCol}.png"/>
    </Layer>
    <Layer><ows:Identifier>elsewhere</ows:Identifier><Format>image/png</Format>
      <TileMatrixSetLink><TileMatrixSet>GoogleMapsCompatible</TileMatrixSet></TileMatrixSetLink>
      <ResourceURL format="image/png" resourceType="tile" template="https://wmts.ogc.example@cdn.other.example/{TileMatrix}/{TileRow}/{TileCol}.png"/>
    </Layer>
    <TileMatrixSet><ows:Identifier>GoogleMapsCompatible</ows:Identifier><ows:SupportedCRS>urn:ogc:def:crs:EPSG::3857</ows:SupportedCRS>${matrices}</TileMatrixSet>
  </Contents>
</Capabilities>`
const wfsCapabilities = `<?xml version="1.0" encoding="UTF-8"?>
<wfs:WFS_Capabilities version="2.0.0" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:ows="http://www.opengis.net/ows/1.1" xmlns:xlink="http://www.w3.org/1999/xlink">
  <ows:OperationsMetadata>
    <ows:Operation name="GetFeature"><ows:DCP><ows:HTTP><ows:Get xlink:href="${WFS_URL}"/></ows:HTTP></ows:DCP>
      <ows:Parameter name="outputFormat"><ows:AllowedValues><ows:Value>application/gml+xml; version=3.2</ows:Value>
        <ows:Value>application/json</ows:Value></ows:AllowedValues></ows:Parameter></ows:Operation>
  </ows:OperationsMetadata>
  <wfs:FeatureTypeList><wfs:FeatureType><wfs:Name>topp:parks</wfs:Name><wfs:Title>Parks</wfs:Title>
    <wfs:DefaultCRS>urn:ogc:def:crs:EPSG::4326</wfs:DefaultCRS><wfs:OtherCRS>urn:ogc:def:crs:EPSG::3857</wfs:OtherCRS>
  </wfs:FeatureType>
  <wfs:FeatureType><wfs:Name>topp:secret</wfs:Name><wfs:DefaultCRS>urn:ogc:def:crs:EPSG::3857</wfs:DefaultCRS></wfs:FeatureType>
  <wfs:FeatureType><wfs:Name>topp:private</wfs:Name><wfs:DefaultCRS>urn:ogc:def:crs:EPSG::3857</wfs:DefaultCRS></wfs:FeatureType>
  </wfs:FeatureTypeList>
</wfs:WFS_Capabilities>`
// a proxied KVP WMTS: a layer whose name has a colon, by its template too, and one whose template has a query (KVP only)
const wmtspCapabilities = `<?xml version="1.0" encoding="UTF-8"?>
<Capabilities version="1.0.0" xmlns="http://www.opengis.net/wmts/1.0" xmlns:ows="http://www.opengis.net/ows/1.1" xmlns:xlink="http://www.w3.org/1999/xlink">
  <ows:OperationsMetadata><ows:Operation name="GetTile"><ows:DCP><ows:HTTP><ows:Get xlink:href="${WMTSP_URL}?">
    <ows:Constraint name="GetEncoding"><ows:AllowedValues><ows:Value>KVP</ows:Value></ows:AllowedValues></ows:Constraint>
  </ows:Get></ows:HTTP></ows:DCP></ows:Operation></ows:OperationsMetadata>
  <Contents>
    <Layer><ows:Identifier>ns:topo</ows:Identifier><Style isDefault="true"><ows:Identifier>default</ows:Identifier></Style>
      <Style><ows:Identifier>dark</ows:Identifier></Style><Format>image/png</Format>
      <TileMatrixSetLink><TileMatrixSet>g</TileMatrixSet></TileMatrixSetLink>
      <ResourceURL format="image/png" resourceType="tile" template="https://wmtsp.ogc.example/rest/{Layer}/{Style}/{TileMatrixSet}/{TileMatrix}/{TileRow}/{TileCol}.png"/>
    </Layer>
    <Layer><ows:Identifier>q</ows:Identifier><Format>image/png</Format><TileMatrixSetLink><TileMatrixSet>g</TileMatrixSet></TileMatrixSetLink>
      <ResourceURL format="image/png" resourceType="tile" template="https://wmtsp.ogc.example/rest/q/{TileMatrix}/{TileRow}/{TileCol}.png?token=1"/>
    </Layer>
    <TileMatrixSet><ows:Identifier>g</ows:Identifier><ows:SupportedCRS>EPSG:3857</ows:SupportedCRS>${matrices}</TileMatrixSet>
  </Contents>
</Capabilities>`
const png = Uint8Array.from([0x89, 0x50, 0x4e, 0x47])
const geojson = JSON.stringify({
    type: "FeatureCollection",
    features: [{type: "Feature", properties: {name: "Park"}, geometry: {type: "Point", coordinates: [1, 2]}}]
})

const usernameOf = userId => userId.toLowerCase().replace(/_/g, ".")
const make = (entity, fields) => ec.entity.makeValue(entity).setAll(fields).createOrUpdate()
const providerRow = providerId =>
    ec.entity.find("moqui.maps.MapProvider").condition("providerId", providerId).useCache(false).one()
// the recorded capabilities, each layer's shared CRS and style lists in place
const capabilities = providerId => {
    const found = JSON.parse(String(providerRow(providerId).capabilitiesJson || "null"))
    for (const layer of found?.layers || []) {
        if (typeof layer.crs === "number") layer.crs = found.crsLists[layer.crs]
        if (typeof layer.styles === "number") layer.styles = found.styleLists[layer.styles]
    }
    return found
}
function as(userId, action) {
    ec.user.internalLoginUser(usernameOf(userId))
    ec.message.clearErrors()
    ec.artifactExecution.enableAuthz()
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
}
const test = connectionKey =>
    as(MANAGER, () =>
        ec.service
            .sync()
            .name(C + "test#Connection")
            .parameters({adapterKey: "mapProvider", connectionKey})
            .call()
    )
// the proxy's service as the providerProxy transition calls it (a screen test sends no raw query string): status and text
const proxy = (userId, providerId, path, query) =>
    as(userId, () => {
        const out = ec.service
            .sync()
            .name("org.moqui.maps.MapSourceServices.get#ProviderAsset")
            .parameters({providerId, pathSegments: path.split("/").filter(part => part), queryString: query || ""})
            .call()
        return {status: out?.ok ? 200 : Number(out?.status) || 0, output: String(out?.text || "")}
    })
const fixtures = (wms = wmsCapabilities) => {
    http.clear()
    http.add({
        method: "GET",
        url: "https://wms.ogc.example/geoserver/ows?map=base&SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap*",
        contentType: "image/png",
        body: png
    })
    http.add({method: "GET", url: "https://wms.ogc.example/geoserver/ows*", contentType: "text/xml", body: wms})
    http.add({method: "GET", url: WMTS_URL, contentType: "application/xml", body: wmtsCapabilities})
    // the direct WMTS's sample tile, asked with an Origin: it answers CORS
    http.add({
        method: "GET",
        url: "https://wmts.ogc.example/wmts/topo/*",
        contentType: "image/png",
        body: png,
        headers: {"Access-Control-Allow-Origin": "*"}
    })
    http.add({
        method: "GET",
        url: WMTSP_URL + "?SERVICE=WMTS&REQUEST=GetCapabilities*",
        contentType: "text/xml",
        body: wmtspCapabilities
    })
    http.add({method: "GET", url: WMTSP_URL + "?*", contentType: "image/png", body: png})
    http.add({method: "GET", url: "https://wmtsp.ogc.example/rest/*", contentType: "image/png", body: png})
    http.add({
        method: "GET",
        url: WFS_URL + "?SERVICE=WFS&REQUEST=GetCapabilities*",
        contentType: "text/xml",
        body: wfsCapabilities
    })
    http.add({method: "GET", url: WFS_URL + "?*", contentType: "application/json", body: geojson})
}
const wfsQuery = extra =>
    "SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=topp:parks&OUTPUTFORMAT=application%2Fjson&SRSNAME=" +
    URN_3857 +
    "&BBOX=-100,-100,100,100," +
    URN_3857 +
    "&COUNT=100" +
    (extra || "")

spec({
    setupSpec() {
        http.enable()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const now = ec.user.nowTimestamp
        for (const userId of [READER, OTHER, MANAGER])
            make("moqui.security.UserAccount", {userId, username: usernameOf(userId), userFullName: "Maps OGC " + userId})
        make("moqui.security.UserPermission", {userPermissionId: PERMISSION, description: "Maps OGC spec: the profile"})
        make("moqui.security.UserGroup", {userGroupId: READERS, description: "Maps OGC readers"})
        make("moqui.security.UserGroupPermission", {userGroupId: READERS, userPermissionId: PERMISSION, fromDate: now})
        make("moqui.security.UserGroupMember", {userGroupId: READERS, userId: READER, fromDate: now})
        make("moqui.security.UserGroup", {userGroupId: MANAGERS, description: "Maps OGC managers"})
        for (const userPermissionId of ["ADMIN_CONNECTION_VIEW", "ADMIN_CONNECTION_MANAGE"])
            make("moqui.security.UserGroupPermission", {userGroupId: MANAGERS, userPermissionId, fromDate: now})
        make("moqui.security.UserGroupMember", {userGroupId: MANAGERS, userId: MANAGER, fromDate: now})
        const provider = (providerId, kind, endpointUrl, extra) =>
            make(
                "moqui.maps.MapProvider",
                Object.assign(
                    {
                        providerId,
                        providerName: "Spec " + providerId,
                        providerKindEnumId: kind,
                        endpointUrl,
                        attribution: "Spec <b>OGC</b>",
                        allowPrivateNetwork: "N",
                        disabled: "N"
                    },
                    extra || {}
                )
            )
        provider(WMS, "MPK_WMS", WMS_URL, {proxyRequests: "Y"})
        provider(WMTS, "MPK_WMTS", WMTS_URL)
        provider(WFS, "MPK_WFS", WFS_URL, {proxyRequests: "Y", apiKey: KEY})
        provider(WMTSP, "MPK_WMTS", WMTSP_URL, {proxyRequests: "Y"})
        make("moqui.maps.MapLayer", {
            layerId: WMTSP,
            layerName: "Spec proxied topo",
            providerId: WMTSP,
            formatEnumId: "MLF_WMTS",
            ogcLayers: "ns:topo"
        })
        // a registered feature type the reader may not see
        make("moqui.maps.MapLayer", {
            layerId: PRIVATE,
            layerName: "Spec private",
            providerId: WFS,
            formatEnumId: "MLF_WFS",
            ogcLayers: "topp:private",
            viewPermission: "MAPS_OGC_NOT_HELD"
        })
        make("moqui.maps.MapLayer", {
            layerId: WMTS,
            layerName: "Spec topo",
            providerId: WMTS,
            formatEnumId: "MLF_WMTS",
            ogcLayers: "topo",
            isBasemap: "Y"
        })
        make("moqui.maps.MapLayer", {
            layerId: WMS,
            layerName: "Spec roads",
            providerId: WMS,
            formatEnumId: "MLF_WMS",
            ogcLayers: "roads",
            ogcStyles: "night",
            ogcTime: "2026-10-01"
        })
        make("moqui.maps.MapLayer", {
            layerId: WFS,
            layerName: "Spec parks",
            providerId: WFS,
            formatEnumId: "MLF_WFS",
            ogcLayers: "topp:parks"
        })
        make("moqui.maps.MapProfile", {
            profileId: PROFILE,
            profileName: "Spec OGC",
            basemapLayerId: WMTS,
            viewPermission: PERMISSION,
            externalAccess: "Y",
            centerLongitude: 0,
            centerLatitude: 0,
            zoom: 2
        })
        make("moqui.maps.MapProfileLayer", {profileId: PROFILE, layerId: WMS, sequenceNum: 1})
        make("moqui.maps.MapProfileLayer", {profileId: PROFILE, layerId: WFS, sequenceNum: 2})
        make("moqui.maps.MapProfileLayer", {profileId: PROFILE, layerId: WMTSP, sequenceNum: 3})
        make("moqui.maps.MapProfileLayer", {profileId: PROFILE, layerId: PRIVATE, sequenceNum: 4})
        make("moqui.maps.MapProfile", {
            profileId: PROFILE2,
            profileName: "Spec OGC 2",
            basemapLayerId: WMTS,
            externalAccess: "Y",
            disabled: "Y"
        })
        ec.message.clearErrors()
    },
    cleanupSpec() {
        http.reset()
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const del = (entity, field, values) => ec.entity.find(entity).condition(field, "in", values).deleteAll()
        del("moqui.maps.MapProfileLayer", "profileId", [PROFILE, PROFILE2])
        del("moqui.maps.MapProfile", "profileId", [PROFILE, PROFILE2])
        del("moqui.maps.MapLayer", "providerId", [WMS, WMTS, WFS, WMTSP])
        del("moqui.server.ConnectionCheck", "connectionKey", [WMS, WMTS, WFS, WMTSP])
        del("moqui.maps.MapProvider", "providerId", [WMS, WMTS, WFS, WMTSP])
        del("moqui.security.UserGroupMember", "userId", [READER, MANAGER])
        del("moqui.security.UserGroupPermission", "userGroupId", [READERS, MANAGERS])
        del("moqui.security.UserGroup", "userGroupId", [READERS, MANAGERS])
        del("moqui.security.UserPermission", "userPermissionId", [PERMISSION])
        del("moqui.security.UserAccount", "userId", [READER, OTHER, MANAGER])
    }
})

feature("a DOCTYPE, an entity declaration and an unknown entity refuse a service's capabilities; nothing is recorded", {
    when() {
        const refusedBy = body => {
            fixtures(body)
            return test(WMS)
        }
        const doctype = refusedBy(
            wmsCapabilities.replace(
                "<WMS_Capabilities",
                '<!DOCTYPE WMT_MS_Capabilities SYSTEM "http://evil.example/x.dtd">\n<WMS_Capabilities'
            )
        )
        const entity = refusedBy(
            wmsCapabilities
                .replace("<WMS_Capabilities", '<!DOCTYPE x [<!ENTITY xxe SYSTEM "file:///etc/passwd">]>\n<WMS_Capabilities')
                .replace("Roads &amp;", "&xxe;")
        )
        const unknown = refusedBy(wmsCapabilities.replace("Roads &amp;", "Roads &xxe;"))
        return {
            doctype,
            entity,
            unknown,
            sent: http.requests().map(row => String(row.url)),
            recorded: providerRow(WMS).capabilitiesJson
        }
    },
    then({doctype, entity, unknown, sent, recorded}) {
        doctype.passed == false
        JSON.stringify(doctype.checkList).includes("DOCTYPE")
        entity.passed == false
        unknown.passed == false
        // only the capabilities were asked: no DTD or entity was fetched
        sent.every(url =>
            url.startsWith("https://wms.ogc.example/geoserver/ows?map=base&SERVICE=WMS&REQUEST=GetCapabilities&VERSION=1.3.0")
        )
        recorded == null
    }
})

feature("each service's test records the layers a map draws, on the provider's own host, and a proxied one's request rules", {
    when() {
        fixtures()
        const tested = [WMS, WMTS, WFS, WMTSP].map(test)
        return {
            tested,
            wms: capabilities(WMS),
            wmts: capabilities(WMTS),
            wfs: capabilities(WFS),
            wmtsp: capabilities(WMTSP),
            wmtspPaths: String(providerRow(WMTSP).proxyPaths || ""),
            wmsPaths: String(providerRow(WMS).proxyPaths || ""),
            wfsPaths: String(providerRow(WFS).proxyPaths || ""),
            wfsAsked: http
                .requests()
                .map(row => String(row.url))
                .filter(url => url.startsWith(WFS_URL))
        }
    },
    then({tested, wms, wmts, wfs, wmtsp, wmtspPaths, wmsPaths, wfsPaths, wfsAsked}) {
        tested.every(out => out.passed == true)
        // WMS: the named layers with the CRSs a map draws, inherited; an image format only; asked at its own host, not the one
        // after a user@ its capabilities name
        wms.layers.map(layer => layer.name).join(",") == "roads"
        wms.layers[0].title == "Roads & streets"
        wms.layers[0].crs.join(",") == "EPSG:4326,EPSG:3857"
        wms.layers[0].styles[0].name == "night"
        wms.layers[0].time.default == "2026-11-01"
        wms.formats.join(",") == "image/png,image/jpeg"
        wms.url == WMS_URL
        JSON.stringify(tested[0].checkList).includes("another host")
        wmsPaths == "wms /geoserver/ows?map=base"
        // WMTS: the matrix set recorded once; a template of another host (after a user@) is not used, so that layer is left
        // out; the provider answers CORS
        wmts.layers.map(layer => layer.name).join(",") == "topo"
        wmts.cors == true
        wmts.matrixSets.GoogleMapsCompatible.TileMatrix.length == 2
        wmts.matrixSets.GoogleMapsCompatible.SupportedCRS == "urn:ogc:def:crs:EPSG::3857"
        // WFS: GeoJSON output only, asked with the key, which no rule keeps
        wfs.layers[0].formats.join(",") == "application/json"
        wfs.layers[0].crs.join(",") == "urn:ogc:def:crs:EPSG::4326,urn:ogc:def:crs:EPSG::3857"
        wfsAsked.length == 1 && wfsAsked[0].includes("key=" + KEY)
        wfsPaths == "wfs /geoserver/wfs"
        // a proxied WMTS: KVP, and the template of the layer whose template has no query; that layer by KVP only
        wmtsp.layers.find(layer => layer.name == "q").template == ""
        wmtspPaths.split("\n").sort().join("|") ==
            "wmts /wmts|wmtsrest /rest/{Layer}/{Style}/{TileMatrixSet}/{TileMatrix}/{TileRow}/{TileCol}.png"
    }
})

feature("the effective configuration draws the OGC layers from what was recorded, only for a user offered the profile", {
    when() {
        const config = userId =>
            as(userId, () =>
                ec.service.sync().name("org.moqui.maps.MapServices.get#EffectiveConfig").parameters({profileId: PROFILE}).call()
            )?.config
        return {reader: config(READER), other: config(OTHER)}
    },
    then({reader, other}) {
        reader.profileId == PROFILE
        reader.profile.basemaps[0].kind == "wmts"
        reader.profile.basemaps[0].url ==
            "https://wmts.ogc.example/wmts/topo/{Style}/{TileMatrixSet}/{TileMatrix}/{TileRow}/{TileCol}.png"
        reader.profile.basemaps[0].ogc.requestEncoding == "REST"
        reader.profile.basemaps[0].ogc.matrixSet.TileMatrix.length == 2
        reader.profile.allowedOrigins.includes("https://wmts.ogc.example")
        const wms = reader.layers.find(layer => layer.id == WMS)
        wms.kind == "wms"
        wms.url == PROXY + WMS + "/geoserver/ows"
        wms.ogc.params.LAYERS == "roads" && wms.ogc.params.STYLES == "night" && wms.ogc.params.TIME == "2026-10-01"
        wms.ogc.projection == "EPSG:3857"
        const wfs = reader.layers.find(layer => layer.id == WFS)
        wfs.url == PROXY + WFS + "/geoserver/wfs"
        wfs.ogc.srsName == "urn:ogc:def:crs:EPSG::3857"
        wfs.ogc.maxFeatures == 1000
        reader.layerErrors.some(error => error.id == PRIVATE && error.reasonCode == "LAYER_MISSING")
        !JSON.stringify(reader).includes(KEY)
        !reader.profile.allowedOrigins.includes("https://wfs.ogc.example")
        // a user not offered the profile is drawn the outline, and nothing of its layers
        other.profileId != PROFILE
        !JSON.stringify(other.layers).includes("ogc.example")
    }
})

feature("the proxy forwards an OGC request only with its kind's parameters, each checked against the capabilities", {
    when() {
        fixtures()
        const ok = proxy(READER, WFS, "/geoserver/wfs", wfsQuery())
        const forwarded = http.count() ? String(http.last().url) : ""
        const wmsOk = proxy(
            READER,
            WMS,
            "/geoserver/ows",
            "SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=roads&STYLES=night&FORMAT=image%2Fpng" +
                "&TRANSPARENT=TRUE&TIME=2026-10-01&CRS=EPSG:3857&BBOX=0,0,1,1&WIDTH=256&HEIGHT=256"
        )
        const wmsForwarded = http.count() ? String(http.last().url) : ""
        http.clear()
        const out = {
            ok,
            forwarded,
            wmsOk,
            wmsForwarded,
            extra: proxy(READER, WFS, "/geoserver/wfs", wfsQuery("&CQL_FILTER=1%3D1")),
            twice: proxy(READER, WFS, "/geoserver/wfs", wfsQuery("&COUNT=5")),
            unknownType: proxy(READER, WFS, "/geoserver/wfs", wfsQuery().replace("topp:parks", "topp:secret")),
            transaction: proxy(READER, WFS, "/geoserver/wfs", wfsQuery().replace("GetFeature", "Transaction")),
            tooMany: proxy(READER, WFS, "/geoserver/wfs", wfsQuery().replace("COUNT=100", "COUNT=5001")),
            noCount: proxy(READER, WFS, "/geoserver/wfs", wfsQuery().replace("&COUNT=100", "")),
            wmsStyle: proxy(
                READER,
                WMS,
                "/geoserver/ows",
                "SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=roads&STYLES=day&FORMAT=image%2Fpng" +
                    "&CRS=EPSG:3857&BBOX=0,0,1,1&WIDTH=256&HEIGHT=256"
            ),
            wmsFormat: proxy(
                READER,
                WMS,
                "/geoserver/ows",
                "SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=roads&FORMAT=text%2Fhtml" +
                    "&CRS=EPSG:3857&BBOX=0,0,1,1&WIDTH=256&HEIGHT=256"
            ),
            other: proxy(OTHER, WFS, "/geoserver/wfs", wfsQuery()),
            // a type its capabilities have that is not a registered layer, and a registered one the reader may not see
            secret: proxy(READER, WFS, "/geoserver/wfs", wfsQuery().replace("topp:parks", "topp:secret")),
            private: proxy(READER, WFS, "/geoserver/wfs", wfsQuery().replace("topp:parks", "topp:private")),
            // a name twice in another case, another output format than the layer's, a box that is not four plain numbers
            caseTwice: proxy(READER, WFS, "/geoserver/wfs", wfsQuery("&typenames=topp:parks")),
            hexBox: proxy(READER, WFS, "/geoserver/wfs", wfsQuery().replace("-100,-100", "0x10,-100")),
            wmsFive: proxy(
                READER,
                WMS,
                "/geoserver/ows",
                "SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=roads&STYLES=night" +
                    "&FORMAT=image%2Fpng&CRS=EPSG:3857&BBOX=0,0,1,1,EPSG:3857&WIDTH=256&HEIGHT=256"
            ),
            sent: 0
        }
        out.sent = http.count()
        return out
    },
    then(out) {
        out.ok.status == 200
        JSON.parse(out.ok.output).features[0].properties.name == "Park"
        out.forwarded.startsWith(WFS_URL + "?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=topp%3Aparks")
        out.forwarded.includes("COUNT=100") && out.forwarded.includes("key=" + KEY)
        // the rule's own query first (the endpoint's map=base), then the checked parameters
        out.wmsOk.status == 200
        out.wmsForwarded.startsWith(
            "https://wms.ogc.example/geoserver/ows?map=base&SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=roads&STYLES=night"
        )
        out.extra.status == 400
        out.twice.status == 400
        out.unknownType.status == 400
        out.transaction.status == 400
        out.tooMany.status == 400
        out.noCount.status == 400
        out.wmsStyle.status == 400
        out.wmsFormat.status == 400
        out.other.status == 404
        out.secret.status == 400
        out.private.status == 400
        out.caseTwice.status == 400
        out.hexBox.status == 400
        out.wmsFive.status == 400
        out.sent == 0
    }
})

feature("a proxied WMTS forwards a registered layer's own tiles, by KVP and by its template, and refuses another", {
    when() {
        fixtures()
        const kvp = extra =>
            "SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0&LAYER=ns:topo&STYLE=default&FORMAT=image%2Fpng&TILEMATRIXSET=g" +
            "&TILEMATRIX=1&TILEROW=0&TILECOL=1" +
            (extra || "")
        const out = {
            kvp: proxy(READER, WMTSP, "/wmts", kvp()),
            kvpSent: http.count() ? String(http.last().url) : "",
            rest: proxy(READER, WMTSP, "/rest/ns:topo/default/g/1/0/1.png"),
            restSent: http.count() ? String(http.last().url) : ""
        }
        http.clear()
        Object.assign(out, {
            otherStyle: proxy(READER, WMTSP, "/wmts", kvp().replace("STYLE=default", "STYLE=dark")),
            otherLayer: proxy(READER, WMTSP, "/wmts", kvp().replace("LAYER=ns:topo", "LAYER=q")),
            otherMatrix: proxy(READER, WMTSP, "/wmts", kvp().replace("TILEMATRIX=1", "TILEMATRIX=9")),
            restStyle: proxy(READER, WMTSP, "/rest/ns:topo/dark/g/1/0/1.png"),
            restLayer: proxy(READER, WMTSP, "/rest/q/default/g/1/0/1.png"),
            other: proxy(OTHER, WMTSP, "/rest/ns:topo/default/g/1/0/1.png"),
            sent: http.count()
        })
        return out
    },
    then(out) {
        out.kvp.status == 200
        out.kvpSent.startsWith(WMTSP_URL + "?SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0&LAYER=ns%3Atopo&STYLE=default")
        out.rest.status == 200
        out.restSent == "https://wmtsp.ogc.example/rest/ns:topo/default/g/1/0/1.png"
        out.otherStyle.status == 400
        out.otherLayer.status == 400
        out.otherMatrix.status == 400
        out.restStyle.status == 404
        out.restLayer.status == 404
        out.other.status == 404
        out.sent == 0
    }
})

feature("Add OGC layer: Configuration's alone; a layer checked as a map draws it; an overlay or a basemap of a profile", {
    when() {
        const call = (userId, name, parameters) =>
            userId
                ? as(userId, () => {
                      try {
                          return (
                              ec.service
                                  .sync()
                                  .name("org.moqui.maps.MapServices." + name)
                                  .parameters(parameters)
                                  .call() || {}
                          )
                      } catch (error) {
                          return {refused: String(error?.message ?? error)}
                      }
                  })
                : ec.service
                      .sync()
                      .name("org.moqui.maps.MapServices." + name)
                      .parameters(parameters)
                      .call() || {}
        const admin = (name, parameters) => {
            ec.user.internalLoginUser("john.doe")
            ec.artifactExecution.enableAuthz()
            try {
                return (
                    ec.service
                        .sync()
                        .name("org.moqui.maps.MapServices." + name)
                        .parameters(parameters)
                        .call() || {}
                )
            } finally {
                ec.message.clearErrors()
                ec.artifactExecution.disableAuthz()
            }
        }
        const readerOptions = call(READER, "get#OgcLayerOptions", {providerId: WMS})
        const readerStore = call(READER, "store#OgcLayer", {providerId: WMS, ogcLayers: "roads"})
        const options = admin("get#OgcLayerOptions", {providerId: WMS, ogcLayers: "roads"})
        const badStyle = admin("store#OgcLayer", {providerId: WMS, ogcLayers: "roads", ogcStyles: "day"})
        const overlay = admin("store#OgcLayer", {providerId: WFS, ogcLayers: "topp:parks", profileId: PROFILE2})
        const basemap = admin("store#OgcLayer", {providerId: WMTS, ogcLayers: "topo", isBasemap: "Y", profileId: PROFILE2})
        const find = (entity, cond) => ec.entity.find(entity).condition(cond).useCache(false).disableAuthz().one()
        return {
            readerOptions,
            readerStore,
            options,
            badStyle,
            overlay,
            basemap,
            shown: !!find("moqui.maps.MapProfileLayer", {profileId: PROFILE2, layerId: overlay.layerId}),
            profileBasemap: String(find("moqui.maps.MapProfile", {profileId: PROFILE2})?.basemapLayerId || "")
        }
    },
    then(out) {
        out.readerOptions.applied != true
        out.readerStore.applied != true
        out.options.providers.some(row => row.value == WMS)
        out.options.layers.map(row => row.value).join(",") == "roads"
        out.options.styles.map(row => row.value).join(",") == "night"
        out.badStyle.applied == false &&
            out.badStyle.reasonCode == "LAYER_UNAVAILABLE" &&
            out.badStyle.cause == "STYLE_UNSUPPORTED"
        out.overlay.applied == true && out.shown == true
        out.basemap.applied == true && out.profileBasemap == out.basemap.layerId
    }
})
