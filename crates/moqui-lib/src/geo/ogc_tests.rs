use super::*;

fn read(service: OgcService, xml: &str) -> Value {
    match read_capabilities(service, xml, 100).expect("reads") {
        Capabilities::Read(found) => found,
        Capabilities::Refused(why) => panic!("refused: {why}"),
    }
}

#[test]
fn a_wms_layer_inherits_the_crs_styles_bounds_and_time_of_the_layers_above_it() {
    let found = read(
        OgcService::Wms,
        r#"<WMS_Capabilities version="1.3.0" xmlns="http://www.opengis.net/wms" xmlns:xlink="http://www.w3.org/1999/xlink">
        <Capability><Request><GetMap><Format>image/png</Format><Format>image/svg+xml</Format><Format>image/png; mode=8bit</Format>
        <DCPType><HTTP><Get><OnlineResource xlink:href="https://a.example/wms?"/></Get></HTTP></DCPType></GetMap></Request>
        <Layer><CRS>EPSG:3857</CRS><CRS>EPSG:2056</CRS><Style><Name>base</Name></Style>
          <EX_GeographicBoundingBox><westBoundLongitude>-1</westBoundLongitude><eastBoundLongitude>1</eastBoundLongitude>
            <southBoundLatitude>-2</southBoundLatitude><northBoundLatitude>2</northBoundLatitude></EX_GeographicBoundingBox>
          <Dimension name="time" default="2026-01-01">2025-01-01/2026-01-01/P1D</Dimension>
          <Layer><Name>roads</Name><CRS>urn:ogc:def:crs:EPSG::4326</CRS><Style><Name>night</Name></Style></Layer>
        </Layer></Capability></WMS_Capabilities>"#,
    );
    assert_eq!(found["formats"], json!(["image/png", "image/png; mode=8bit"]));
    assert_eq!(found["href"], "https://a.example/wms?");
    let layer = &found["layers"][0];
    assert_eq!(found["layers"].as_array().unwrap().len(), 1, "only named layers");
    assert_eq!(layer["title"], "roads");
    let list = |name: &str, field: &str| found[name][layer[field].as_u64().unwrap() as usize].clone();
    assert_eq!(
        list("crsLists", "crs"),
        json!(["EPSG:3857", "urn:ogc:def:crs:EPSG::4326"])
    );
    assert_eq!(list("styleLists", "styles")[1]["name"], "night");
    assert_eq!(layer["bounds"], json!([-1.0, -2.0, 1.0, 2.0]));
    assert_eq!(layer["time"]["default"], "2026-01-01");
}

#[test]
fn a_wmts_layer_keeps_its_matrix_set_in_a_crs_a_map_draws_and_its_template() {
    let found = read(
        OgcService::Wmts,
        r#"<Capabilities version="1.0.0" xmlns="http://www.opengis.net/wmts/1.0" xmlns:ows="http://www.opengis.net/ows/1.1">
        <Contents>
          <Layer><ows:Identifier>topo</ows:Identifier><Format>image/jpgpng</Format><Style isDefault="true"><ows:Identifier>default</ows:Identifier></Style>
            <TileMatrixSetLink><TileMatrixSet>g</TileMatrixSet></TileMatrixSetLink><TileMatrixSetLink><TileMatrixSet>swiss</TileMatrixSet></TileMatrixSetLink>
            <ResourceURL format="image/png" resourceType="tile" template="https://t.example/{TileMatrix}/{TileRow}/{TileCol}.png"/></Layer>
          <Layer><ows:Identifier>swissOnly</ows:Identifier><TileMatrixSetLink><TileMatrixSet>swiss</TileMatrixSet></TileMatrixSetLink></Layer>
          <TileMatrixSet><ows:Identifier>g</ows:Identifier><ows:SupportedCRS>urn:ogc:def:crs:EPSG:6.18.3:3857</ows:SupportedCRS>
            <TileMatrix><ows:Identifier>0</ows:Identifier><ScaleDenominator>559082264.0287178</ScaleDenominator>
              <TopLeftCorner>-20037508.34 20037508.34</TopLeftCorner><TileWidth>256</TileWidth><TileHeight>256</TileHeight>
              <MatrixWidth>1</MatrixWidth><MatrixHeight>1</MatrixHeight></TileMatrix></TileMatrixSet>
          <TileMatrixSet><ows:Identifier>swiss</ows:Identifier><ows:SupportedCRS>EPSG:2056</ows:SupportedCRS></TileMatrixSet>
        </Contents></Capabilities>"#,
    );
    let layers = found["layers"].as_array().unwrap();
    assert_eq!(
        layers.len(),
        1,
        "a layer with no matrix set in a CRS a map draws is left out"
    );
    assert_eq!(layers[0]["matrixSets"], json!(["g"]));
    assert_eq!(layers[0]["formats"], json!(["image/jpgpng"]));
    assert_eq!(
        layers[0]["template"],
        "https://t.example/{TileMatrix}/{TileRow}/{TileCol}.png"
    );
    assert_eq!(found["matrixSets"]["g"]["TileMatrix"][0]["MatrixWidth"], 1);
    assert!(found["matrixSets"].get("swiss").is_none());
    assert_eq!(found["kvp"], false, "no GetTile operation is advertised");
}

#[test]
fn a_wfs_feature_type_needs_a_geojson_output() {
    let found = read(
        OgcService::Wfs,
        r#"<wfs:WFS_Capabilities version="2.0.0" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:ows="http://www.opengis.net/ows/1.1" xmlns:xlink="http://www.w3.org/1999/xlink">
        <ows:OperationsMetadata><ows:Operation name="GetFeature"><ows:DCP><ows:HTTP><ows:Get xlink:href="https://f.example/wfs"/></ows:HTTP></ows:DCP>
          <ows:Parameter name="outputFormat"><ows:AllowedValues><ows:Value>text/xml; subtype=gml/3.2</ows:Value><ows:Value>application/json</ows:Value></ows:AllowedValues></ows:Parameter>
        </ows:Operation></ows:OperationsMetadata>
        <wfs:FeatureTypeList><wfs:FeatureType><wfs:Name>parks</wfs:Name><wfs:DefaultCRS>urn:ogc:def:crs:EPSG::4326</wfs:DefaultCRS></wfs:FeatureType></wfs:FeatureTypeList>
        </wfs:WFS_Capabilities>"#,
    );
    assert_eq!(found["href"], "https://f.example/wfs");
    assert_eq!(found["layers"][0]["formats"], json!(["application/json"]));
    assert_eq!(found["layers"][0]["crs"], json!(["urn:ogc:def:crs:EPSG::4326"]));
}

#[test]
fn a_doctype_an_entity_declaration_and_an_unknown_entity_are_refused() {
    let base = r#"<WMS_Capabilities version="1.3.0"><Capability/></WMS_Capabilities>"#;
    for xml in [
        format!("<!DOCTYPE WMT_MS_Capabilities SYSTEM \"http://evil.example/x.dtd\">{base}"),
        format!("<!DOCTYPE x [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">]>{base}"),
        r#"<WMS_Capabilities version="1.3.0"><Capability><Layer><Name>&xxe;</Name></Layer></Capability></WMS_Capabilities>"#.to_string(),
    ] {
        assert!(read_capabilities(OgcService::Wms, &xml, 10).is_err(), "{xml}");
    }
    assert!(matches!(
        read_capabilities(OgcService::Wms, r#"<WMT_MS_Capabilities version="1.1.1"/>"#, 10).unwrap(),
        Capabilities::Refused(_)
    ));
    assert!(matches!(
        read_capabilities(
            OgcService::Wfs,
            r#"<ows:ExceptionReport xmlns:ows="http://www.opengis.net/ows/1.1"/>"#,
            10
        )
        .unwrap(),
        Capabilities::Refused(_)
    ));
}

#[test]
fn the_codes_a_map_draws_are_read_as_openlayers_reads_them() {
    for code in [
        "EPSG:3857",
        "urn:ogc:def:crs:EPSG::3857",
        "urn:ogc:def:crs:EPSG:6.18.3:3857",
        "CRS:84",
        "urn:ogc:def:crs:OGC:1.3:CRS84",
        "EPSG:4326",
    ] {
        assert!(drawn_crs(code), "{code}");
    }
    for code in [
        "EPSG:2056",
        "epsg:3857",
        "urn:ogc:def:crs:EPSG::2056",
        "URN:OGC:DEF:CRS:EPSG::3857",
        "urn:ogc:def:crs:EPSG:x:3857",
    ] {
        assert!(!drawn_crs(code), "{code}");
    }
}

#[test]
fn inherited_lists_are_kept_once_and_a_document_past_2_mb_of_json_is_refused() {
    let layer = |index: usize| format!("<Layer><Name>{index:0>250}</Name><Title>{index:0>250}</Title></Layer>");
    let many = |count: usize| {
        format!(
            r#"<WMS_Capabilities version="1.3.0"><Capability><Layer><CRS>EPSG:3857</CRS>{}</Layer></Capability></WMS_Capabilities>"#,
            (0..count).map(layer).collect::<String>()
        )
    };
    let found = read(OgcService::Wms, &many(50));
    assert_eq!(
        found["crsLists"].as_array().unwrap().len(),
        1,
        "the root's CRS list once for its 50 children"
    );
    assert!(matches!(
        read_capabilities(OgcService::Wms, &many(5000), 5000).unwrap(),
        Capabilities::Refused(_)
    ));
}
