/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// m-map's pure helpers in the page (moqui.map of /js/WebrootMap.js), and its list of places when the map cannot be drawn. The
// OpenLayers bundle (an ES module, imported as m-map imports it) is loaded for its formats and vector sources only, and its
// raster part for the projections and the lookup it refuses: drawing the map is a browser walk, not a regression test. The
// page is https://moqui.test.
"use strict";

const map = moqui.map;
const page = location.origin;
// the main bundle as m-map loads it: its entries' hashed names from ol-map.json, the main one imported, the global MoquiOl
const olLoad = map.loadOpenLayers();

test("a configuration is checked: numbers from texts, WGS84 degrees, longitude first, a zero kept, the outline by default", () => {
    const config = map.normalizeConfig(JSON.stringify({
        profileId: "OUTLINE", layers: "PARKS, TRAILS", center: "-87.63,41.88", zoom: "11", extent: [170, -20, -170, 20], projection: "epsg:4326",
        points: [{ longitude: "-87.6298", latitude: "41.8781", label: "Chicago <b>", precisionEnumId: "GPP_POSTAL_AREA", sourceText: "Census 2026" },
            { lon: 0, lat: 0, label: "Null Island" }, { lon: 200, lat: 10, label: "Nowhere" }, { lon: "", lat: "", label: "Blank" }, { lat: 91, lon: 0 }],
        pick: "true", adapter: "party", recordKey: { partyId: "P1" }
    }));
    assert.deepEqual(config.center, [-87.63, 41.88]);
    assert.equal(config.zoom, 11);
    assert.deepEqual(config.extent, [170, -20, -170, 20], "west greater than east crosses the antimeridian");
    assert.equal(config.projection, "EPSG:4326");
    assert.deepEqual(config.layers, [{ id: "PARKS", requested: true }, { id: "TRAILS", requested: true }]);
    assert.deepEqual(config.points.map((point) => [point.label, point.lon, point.lat]), [["Chicago <b>", -87.6298, 41.8781], ["Null Island", 0, 0]]);
    assert.equal(config.points[0].precision, "Postal-area representative point");
    assert.equal(config.points[0].source, "Census 2026");
    assert.equal(config.pick, true);
    assert.deepEqual(config.recordKey, { partyId: "P1" });
    // no basemap given: the outline of this server
    assert.equal(config.outline, true);
    assert.equal(config.profile.basemaps[0].kind, "outline");
    assert.equal(config.profile.basemaps[0].url, "/libs/naturalearth/countries-110m.json");
    // what is not WGS84 is left out, not guessed
    const bad = map.normalizeConfig({ center: [41.88, -187], extent: "0,10,5,5", zoom: 40, projection: "EPSG:2056" });
    assert.equal(bad.center, null);
    assert.equal(bad.extent, null, "south above north");
    assert.equal(bad.zoom, null);
    assert.equal(bad.projection, "EPSG:3857");
    assert.deepEqual(map.normalizeConfig("not json").points, []);
    // a server's profile with its basemaps
    const hosted = map.normalizeConfig({ profile: { profileId: "OPENFREEMAP", basemaps: [{ id: "OFM", kind: "STYLE", url: "https://tiles.openfreemap.org/styles/liberty",
        attribution: "OpenFreeMap\n© OpenMapTiles", attributionUrls: "https://openfreemap.org\nhttps://www.openmaptiles.org/" }],
        notice: { text: "No provider", href: "/qapps/maps/Configuration" } } });
    assert.equal(hosted.outline, false);
    assert.equal(hosted.profile.basemaps[0].kind, "style");
    assert.deepEqual(hosted.profile.basemaps[0].attributionUrls, ["https://openfreemap.org", "https://www.openmaptiles.org/"]);
    assert.equal(hosted.profile.notice.href, "/qapps/maps/Configuration");
});

test("a server's configuration takes the map-view's view, places and pick, and keeps the layers it did not resolve", () => {
    const server = map.normalizeConfig({ profileId: "LOCAL", center: [0, 20], zoom: 2, points: [{ id: "rec", lon: 10, lat: 50, label: "The record" }],
        layers: [{ id: "PARKS", kind: "geojson", url: "/maps/parks.json" }] });
    const local = map.normalizeConfig({ zoom: 9, points: [{ id: "rec", lon: 11, lat: 51, label: "Given" }], layers: ["PARKS", "GONE"], pick: true });
    const merged = map.mergeConfig(server, local);
    assert.deepEqual(merged.center, [0, 20]);
    assert.equal(merged.zoom, 9);
    assert.equal(merged.pick, true);
    assert.deepEqual(merged.points.map((point) => point.label), ["The record", "Given"]);
    assert.notEqual(merged.points[0].id, merged.points[1].id, "ids stay apart");
    assert.deepEqual(merged.layers.map((layer) => [layer.id, !!layer.requested]), [["PARKS", false], ["GONE", true]]);
    assert.deepEqual(map.configParameters(map.normalizeConfig({ profileId: "OUTLINE", layers: "A,B", adapter: "party", recordKey: { partyId: "P1" }, addressPurpose: "MAIL" })),
        { profileId: "OUTLINE", layerIds: "A,B", adapterKey: "party", recordKey: '{"partyId":"P1"}', addressPurpose: "MAIL" });
    assert.deepEqual(map.configParameters(map.normalizeConfig({ recordKey: { partyId: "P1" } })), {}, "a record key goes only with its adapter");
});

test("only the page's and the profile's hosts are asked: no mapbox URL, no other scheme, no other host", () => {
    const config = map.normalizeConfig({ profile: { basemaps: [{ kind: "style", url: "https://tiles.openfreemap.org/styles/liberty" },
        { kind: "xyz", url: "https://tile.example.org/{z}/{x}/{y}.png" }, { kind: "pmtiles", url: "https://elsewhere.example/a.pmtiles" }],
        allowedOrigins: ["https://fonts.example.net/ignored/path"] }, layers: [{ id: "P", kind: "geojson", url: "https://data.example.com/parks.json" }] });
    const origins = map.allowedOrigins(config, page);
    assert.deepEqual([...origins].sort(), [page, "https://data.example.com", "https://fonts.example.net", "https://tile.example.org", "https://tiles.openfreemap.org"].sort());
    assert.ok(!origins.has("https://elsewhere.example"), "a PMTiles archive names no host to allow");
    const allowed = (url) => map.urlAllowed(url, origins, page);
    assert.ok(allowed("/libs/naturalearth/countries-110m.json"));
    assert.ok(allowed("https://tiles.openfreemap.org/planet/20261120/{z}/{x}/{y}.pbf"));
    assert.ok(allowed(page + "/maps/source?rev=1"));
    assert.ok(!allowed("mapbox://styles/mapbox/streets-v12"));
    assert.ok(!allowed("MAPBOX://mapbox.mapbox-streets-v8"));
    assert.ok(!allowed("https://api.mapbox.com/styles/v1/x"));
    assert.ok(!allowed("https://cdn.jsdelivr.net/fontsource/fonts/noto-sans@latest/latin-400-normal.woff2"));
    assert.ok(!allowed("//evil.example/tiles.json"));
    assert.ok(!allowed("data:application/json,{}"));
    assert.ok(!allowed("javascript:alert(1)"));
    assert.ok(!allowed(""));
    assert.ok(map.sameOrigin("/maps/archive.pmtiles", page));
    assert.ok(!map.sameOrigin("https://tiles.openfreemap.org/a.pmtiles", page));
});

test("a style's sources, tiles and sprite must be of those hosts, and its fonts are the local ones", () => {
    const origins = new Set([page, "https://tiles.openfreemap.org"]);
    const style = { version: 8, metadata: { "ol:webfonts": "https://cdn.jsdelivr.net/fontsource/{font-family}.css", other: 1 },
        sources: { openmaptiles: { type: "vector", url: "https://tiles.openfreemap.org/planet" }, local: { type: "vector", tiles: ["tiles/{z}/{x}/{y}.pbf"] },
            mb: { type: "raster", url: "mapbox://mapbox.satellite" }, geo: { type: "geojson", data: "https://data.example.com/x.geojson" } },
        sprite: "https://tiles.openfreemap.org/sprites/ofm_f384/ofm", glyphs: "https://fonts.example.net/{fontstack}/{range}.pbf", layers: [] };
    assert.deepEqual(map.refusedStyleUrls(style, "https://tiles.openfreemap.org/styles/liberty", origins, page),
        ["mapbox://mapbox.satellite", "https://data.example.com/x.geojson"]);
    assert.deepEqual(map.refusedStyleUrls({ sources: {}, sprite: [{ id: "x", url: "https://sprites.example.org/s" }] }, "/maps/style.json", origins, page),
        ["https://sprites.example.org/s"]);
    assert.deepEqual(map.refusedStyleUrls({ sources: { a: { tiles: ["/maps/tiles/{z}/{x}/{y}.pbf"] } } }, "/maps/style.json", origins, page), []);
    // a PMTiles archive's source, which m-map binds itself, names pmtiles:// and is not asked
    const protomaps = { sources: { protomaps: { type: "vector", url: "pmtiles://https://example.org/x.pmtiles" } } };
    assert.deepEqual(map.refusedStyleUrls(protomaps, "/maps/style.json", origins, page, ["protomaps"]), []);
    assert.deepEqual(map.refusedStyleUrls(protomaps, "/maps/style.json", origins, page), ["pmtiles://https://example.org/x.pmtiles"]);
    // the fonts: an absolute template of this page (ol-mapbox-style resolves a root-relative one against the style's host)
    assert.equal(map.webfonts(page), page + "/libs/mapfonts/{font-family}/{fontweight}{-fontstyle}.css");
    const local = map.localStyle(style, map.webfonts(page));
    assert.equal(local.metadata["ol:webfonts"], page + "/libs/mapfonts/{font-family}/{fontweight}{-fontstyle}.css");
    assert.equal(local.metadata.other, 1);
    assert.equal(style.metadata["ol:webfonts"], "https://cdn.jsdelivr.net/fontsource/{font-family}.css", "the style given is not changed");
    // a source's attribution is set as HTML by the attribution control: it is made escaped text, in the style and a TileJSON
    const marked = map.localStyle({ sources: { a: { type: "vector", attribution: '<a href="https://x.org" onclick="alert(1)">&copy; X</a><img src=y>' } } }, map.webfonts(page));
    assert.equal(marked.sources.a.attribution, "© X");
    assert.equal(map.sanitizeSource({ tiles: ["/t/{z}/{x}/{y}.pbf"], attribution: "<script>alert(1)</script>A & B" }).attribution, "alert(1) A &amp; B");
    assert.deepEqual(map.sanitizeSource({ tiles: [] }), { tiles: [] });
});

test("a KML file is read with no request for its icons, whatever its styles name", async () => {
    const kml = '<kml xmlns="http://www.opengis.net/kml/2.2"><Document>' +
        '<Style id="s"><IconStyle><Icon><href>https://icons.example.com/pin.png</href></Icon></IconStyle></Style>' +
        '<Style id="d"><IconStyle><scale>1.2</scale></IconStyle></Style>' +
        '<Placemark><name>&lt;b&gt;One&lt;/b&gt;</name><styleUrl>#s</styleUrl><Point><coordinates>-87.6,41.9</coordinates></Point></Placemark>' +
        '<Placemark><name>Two</name><styleUrl>#d</styleUrl><Style><IconStyle><Icon><href>http://maps.google.com/mapfiles/kml/pushpin/ylw-pushpin.png</href></Icon></IconStyle></Style>' +
        '<Point><coordinates>2.35,48.85</coordinates></Point></Placemark></Document></kml>';
    const descriptor = Object.getOwnPropertyDescriptor(HTMLImageElement.prototype, "src");
    const setAttribute = Element.prototype.setAttribute;
    const asked = [];
    Object.defineProperty(HTMLImageElement.prototype, "src", { configurable: true, get: descriptor.get,
        set(value) { asked.push(String(value)); descriptor.set.call(this, value); } });
    Element.prototype.setAttribute = function (name, value) {
        if (this instanceof HTMLImageElement && String(name).toLowerCase() === "src") asked.push(String(value));
        return setAttribute.call(this, name, value);
    };
    await olLoad;
    const before = performance.getEntriesByType("resource").length;
    try {
        const features = new MoquiOl.KML(map.kmlOptions()).readFeatures(kml, { featureProjection: "EPSG:3857" });
        assert.equal(features.length, 2);
        assert.equal(features[0].get("name"), "<b>One</b>", "a name is text");
        assert.equal(features[0].getStyle(), null, "the file's styles are not used");
        await new Promise((resolve) => setTimeout(resolve, 200));
    } finally {
        Object.defineProperty(HTMLImageElement.prototype, "src", descriptor);
        Element.prototype.setAttribute = setAttribute;
    }
    assert.ok(asked.every((src) => src === map.BLANK_ICON), "only the blank icon: " + asked.join(", "));
    const later = performance.getEntriesByType("resource").slice(before).map((entry) => entry.name);
    assert.ok(!later.some((name) => /icons\.example\.com|maps\.google\.com/.test(name)), later.join(", "));
});

test("a notice links only to this site or an http address; a view's extent is within the world", () => {
    assert.equal(map.safeHref("/qapps/maps/Configuration"), "/qapps/maps/Configuration");
    assert.equal(map.safeHref("Configuration?tab=providers"), "Configuration?tab=providers");
    assert.equal(map.safeHref("https://openfreemap.org/tos/"), "https://openfreemap.org/tos/");
    for (const bad of ["javascript:alert(1)", "java\nscript:alert(1)", " JAVASCRIPT:alert(1)", "data:text/html,x", "//evil.example/x", "\\\\evil.example/x",
        "/\\evil.example/x", "http:evil.example", "mailto:a@b.c"]) {
        assert.equal(map.safeHref(bad), "", bad);
    }
    assert.equal(map.normalizeConfig({ profile: { notice: { text: "Set up maps", href: "javascript:alert(1)" } } }).profile.notice.href, "");
    assert.deepEqual(map.wrapExtent([-200, -10, -170, 10]), [160, -10, -170, 10], "across the antimeridian: west greater than east");
    assert.deepEqual(map.wrapExtent([-540, -100, 540, 100]), [-180, -90, 180, 90], "wider than the world");
    assert.deepEqual(map.wrapExtent([-10, 40, 10, 50]), [-10, 40, 10, 50]);
});

test("labels and attributions are text: escaped, and a link only to an http address", () => {
    assert.equal(map.escapeHtml('<img src=x onerror="alert(1)"> & \'q\''), "&lt;img src=x onerror=&quot;alert(1)&quot;&gt; &amp; &#39;q&#39;");
    assert.equal(map.attributionHtml("OpenFreeMap\n© <b>OpenMapTiles</b>\nData", ["https://openfreemap.org", "javascript:alert(1)"]),
        '<a href="https://openfreemap.org" target="_blank" rel="noopener noreferrer">OpenFreeMap</a> · © &lt;b&gt;OpenMapTiles&lt;/b&gt; · Data');
    assert.equal(map.attributionHtml(""), "");
    assert.equal(map.attributionText('<a href="https://openmaptiles.org/">&copy; OpenMapTiles</a> <a href="x">OSM</a>'), "© OpenMapTiles OSM");
});

test("a coordinate reads latitude first; a typed one is two numbers in range", () => {
    assert.equal(map.formatCoordinates(-87.6298, 41.8781), "41.87810, -87.62980");
    assert.equal(map.formatCoordinates(0, 0), "0.00000, 0.00000");
    assert.deepEqual(map.parseLatLon("41.8781, -87.6298"), { lon: -87.6298, lat: 41.8781 });
    assert.deepEqual(map.parseLatLon(" 0 0 "), { lon: 0, lat: 0 });
    assert.equal(map.parseLatLon("-187.6298, 41.8781"), null, "a longitude where the latitude goes is out of range");
    assert.equal(map.parseLatLon("41.8"), null);
    assert.equal(map.parseLatLon("north, west"), null);
    assert.equal(map.wrapLongitude(190), -170);
    assert.equal(map.wrapLongitude(-190), 170);
    assert.equal(map.wrapLongitude(45), 45);
    // a ring across the antimeridian is made continuous, so a planar map draws no band across the world; a polar ring is kept
    assert.deepEqual(map.unwrapRing([[170, 60], [-170, 60], [-170, 70], [170, 70], [170, 60]]), [[170, 60], [190, 60], [190, 70], [170, 70], [170, 60]]);
    const polar = [[-180, -80], [0, -70], [180, -80], [-180, -80]];
    assert.deepEqual(map.unwrapRing(polar), polar);
    assert.equal(map.precisionText("GPP_USER_CONFIRMED"), "User-confirmed point");
    assert.equal(map.precisionText("Rooftop, per the county"), "Rooftop, per the county");
});

test("when the map cannot be drawn its places are listed, as text, with the notice", async () => {
    const holder = document.createElement("div");
    document.body.appendChild(holder);
    const app = Vue.createApp({ render: () => Vue.h(map.components["m-map-view"], { id: "Fallback", libraryError: "fixture bundle unavailable",
        config: { profile: { notice: { text: "No map provider is enabled.", href: "/qapps/maps/Configuration", linkText: "Configure maps" } },
            points: [{ lon: -87.6298, lat: 41.8781, label: "<img src=x onerror=alert(1)>", precision: "GPP_POSTAL_AREA" }] } }) });
    app.mount(holder);
    await Vue.nextTick();
    await Vue.nextTick();
    const root = holder.querySelector("#Fallback");
    assert.ok(root.querySelector(".m-map-fallback").textContent.includes("fixture bundle unavailable"));
    assert.equal(root.querySelector(".m-map-canvas"), null, "no map frame");
    assert.equal(root.querySelector(".m-map-notice-link").getAttribute("href"), "/qapps/maps/Configuration");
    const place = root.querySelector(".m-map-place");
    assert.equal(place.querySelector(".m-map-place-label").textContent, "<img src=x onerror=alert(1)>");
    assert.equal(place.querySelector("img"), null, "a label is never markup");
    assert.equal(place.querySelector(".m-map-place-coordinates").textContent, "41.87810, -87.62980");
    assert.equal(place.querySelector(".m-map-place-details").textContent, "Postal-area representative point");
    app.unmount();
    holder.remove();
});

test("what the server did not resolve is named under the map, a cut feature list said, the scale in the configuration's units", async () => {
    const config = map.normalizeConfig({ units: "IMPERIAL", featuresTruncated: true, layerErrors: [
        { id: "record", label: "Spec places", reasonCode: "RECORD_UNAVAILABLE", message: "This record's place is not available" },
        { id: "PARKS", label: "Parks <b>", reasonCode: "LAYER_DISABLED", message: "the layer is disabled" }, { label: "No id" }, "junk"] });
    assert.deepEqual(config.layerErrors.map((error) => error.id), ["record", "PARKS"]);
    assert.equal(config.featuresTruncated, true);
    assert.equal(map.scaleUnits(config), "imperial");
    assert.equal(map.scaleUnits(map.normalizeConfig({ units: "furlongs" })), "metric");
    assert.equal(map.mergeConfig(config, map.normalizeConfig({ units: "metric" })).units, "metric", "the map-view's units win");
    const holder = document.createElement("div");
    document.body.appendChild(holder);
    const app = Vue.createApp({ render: () => Vue.h(map.components["m-map-view"], { id: "Errors", libraryError: "fixture bundle unavailable",
        config: { featuresTruncated: true, layerErrors: config.layerErrors, points: [{ lon: -87.6298, lat: 41.8781, label: "Chicago" }] } }) });
    app.mount(holder);
    await Vue.nextTick();
    await Vue.nextTick();
    const root = holder.querySelector("#Errors");
    assert.ok(root.querySelector(".m-map-record-error").textContent.includes("This record's place is not available"));
    const errors = Array.from(root.querySelectorAll(".m-map-layer-errors li")).map((item) => item.textContent);
    assert.equal(errors.length, 2);
    assert.ok(errors[1].includes("Parks <b>"), "a label is text");
    assert.equal(root.querySelector(".m-map-layer-errors b"), null);
    assert.equal(root.querySelector(".m-map-layer-errors .m-map-retry"), null, "no config-url: nothing to ask again");
    assert.ok(root.querySelector(".m-map-truncated").textContent.includes("Only some of the records are shown"));
    assert.ok(root.querySelector(".m-map-place .m-map-place-copy"), "each place can be copied");
    app.unmount();
    holder.remove();
    // a choice fills the latitude and longitude of the form named by pickForm (LocalMap's Choose location), and no other
    const named = document.createElement("form");
    named.setAttribute("name", "ChooseLocation");
    const other = document.createElement("form");
    other.setAttribute("name", "Other");
    const before = moqui.screenForms;
    const chosen = { $el: named, fields: {} }, untouched = { $el: other, fields: {} };
    moqui.screenForms = [untouched, chosen];
    try {
        assert.equal(map.fillPickForm("ChooseLocation", { lon: -87.62980123, lat: 41.8781 }), true);
        assert.deepEqual(chosen.fields, { latitude: "41.878100", longitude: "-87.629801" });
        assert.deepEqual(untouched.fields, {});
        assert.equal(map.fillPickForm("Missing", { lon: 0, lat: 0 }), false);
        assert.equal(map.fillPickForm("", { lon: 0, lat: 0 }), false);
        assert.equal(map.normalizeConfig({ pick: true, pickForm: "ChooseLocation" }).pickForm, "ChooseLocation");
    } finally {
        moqui.screenForms = before;
    }
});

test("an overlay's feature is selected by its text, never markup; Enter takes the feature nearest the map's center", async () => {
    await olLoad;
    // a profile layer's KML description may hold HTML: shown as plain text, scripts and tags gone, a lone "<" kept
    assert.equal(map.plainText('<p>Trail <b>head</b></p><script>alert(1)</script><img src=x onerror="alert(2)">&lt;b&gt; a<b'), "Trail head <b> a<b");
    assert.equal(map.plainText("Line one\r\nLine two\u0007"), "Line one\nLine two");
    assert.equal(map.plainText({ html: "<b>x</b>" }), "", "an object is no text");
    // comments go; numeric entities are read, and what they make is still text (a tag made so is not removed, nor markup)
    assert.equal(map.plainText("Spring<!-- <b>hidden</b> --> water<!-- unclosed"), "Spring water");
    assert.equal(map.plainText("Caf&#233; &#x2014; &#60;b&#62;bold&#60;/b&#62; &#7;&#0;"), "Café — <b>bold</b> &#0;");
    assert.equal(map.plainText("&#xD800; &#1114112; &bogus;"), "&#xD800; &#1114112; &bogus;", "not a character: left as it was");
    assert.equal(map.plainText("x".repeat(30), 10), "xxxxxxxxx…");
    assert.deepEqual(map.featureDetails({ name: "<i>Spring</i>", desc: "Fresh <a href='javascript:alert(1)'>water</a>", geometry: {} }),
        { label: "Spring", description: "Fresh water" });
    assert.deepEqual(map.featureDetails({ label: "Camp", name: "ignored", description: "" }), { label: "Camp", description: "" });
    assert.deepEqual(map.featureDetails(null), { label: "", description: "" });
    // the sources of a map's shown overlays, in degrees here (the map's projection in m-map)
    const features = new MoquiOl.GeoJSON().readFeatures({ type: "FeatureCollection", features: [
        { type: "Feature", id: "spring", geometry: { type: "Point", coordinates: [10, 10] }, properties: { label: "Spring" } },
        { type: "Feature", id: "trail", geometry: { type: "LineString", coordinates: [[0, 20], [20, 20]] }, properties: { label: "Trail" } },
        { type: "Feature", id: "park", geometry: { type: "Polygon", coordinates: [[[-5, -5], [30, -5], [30, 30], [-5, 30], [-5, -5]]] }, properties: { label: "Park" } }] });
    const overlay = { get: (name) => ({ overlayLayerId: "overlay:7", overlayLabel: "Hikes" })[name] };
    const candidates = [{ source: new MoquiOl.VectorSource({ features }), layer: overlay }];
    const near = (lon, lat) => { const hit = map.nearestFeature(candidates, [lon, lat], 1); return hit ? hit.feature.getId() : null; };
    assert.equal(near(10.5, 10.5), "spring", "a point within the tolerance, inside the park, wins");
    assert.equal(near(5, 20.4), "trail");
    assert.equal(near(25, 0), "park", "inside an area with nothing nearer");
    assert.equal(near(60, 60), null, "nothing within the tolerance");
    assert.equal(map.nearestFeature(candidates, [10, 10], 1).layer, overlay, "the layer says whose feature it is");
    assert.equal(map.nearestFeature([], [0, 0], 1), null);
});

test("a registered archive's 409 says its revision changed; another failure does not, and both still fail the read", async () => {
    const answers = { "/a.pmtiles": "Bad response code: 409", "/b.pmtiles": "Bad response code: 404" };
    const O = { FetchSource: class { constructor(url) { this.url = url; }
        getBytes() { return Promise.reject(new Error(answers[this.url])); } } };
    const changed = [];
    for (const url of ["/a.pmtiles", "/b.pmtiles"]) {
        const source = map.pinnedSource(O, url, () => changed.push(url));
        assert.equal(source.getKey(), url);
        let failed = null;
        try { await source.getBytes(0, 16384); } catch (err) { failed = err.message; }
        assert.equal(failed, answers[url]);
    }
    assert.deepEqual(changed, ["/a.pmtiles"]);
});

test("an OGC layer's options are checked, and its WMS, WMTS and WFS sources ask only the URL the server gave, with no network", async () => {
    const O = await olLoad;
    // WMS: only the parameters a GetMap takes, VERSION 1.3.0 whatever is given
    const wms = map.normalizeConfig({ layers: [{ id: "W", kind: "WMS", url: "https://maps.example.org/wms?map=a",
        ogc: { params: { LAYERS: "roads,rivers", STYLES: ",", FORMAT: "image/png", TRANSPARENT: "TRUE", VERSION: "1.1.1", REQUEST: "GetFeatureInfo", EVIL: "x" }, projection: "EPSG:3857" } }] }).layers[0];
    assert.equal(wms.kind, "wms");
    assert.deepEqual(Object.keys(wms.ogc.params).sort(), ["FORMAT", "LAYERS", "STYLES", "TRANSPARENT", "VERSION"]);
    assert.equal(wms.ogc.params.VERSION, "1.3.0");
    const tile = new O.TileWMS({ url: wms.url, params: wms.ogc.params, projection: wms.ogc.projection });
    const url = new URL(tile.getTileUrlFunction()([2, 1, 1], 1, O.getProjection("EPSG:3857")));
    assert.equal(url.origin + url.pathname, "https://maps.example.org/wms");
    assert.equal(url.searchParams.get("map"), "a");
    assert.equal(url.searchParams.get("LAYERS"), "roads,rivers");
    assert.equal(url.searchParams.get("CRS"), "EPSG:3857");
    assert.equal(url.searchParams.get("REQUEST"), "GetMap");
    assert.equal(url.searchParams.get("EVIL"), null);
    assert.equal(map.normalizeOgc({ params: { FORMAT: "image/png" } }, "wms"), null, "no layer");
    assert.equal(map.normalizeOgc({ params: { LAYERS: "a" } }, "xyz"), null, "another kind has none");

    // WMTS: the tile grid from the recorded matrix set (EPSG:3857, two matrices), KVP and a REST template
    const matrixSet = { Identifier: "GoogleMapsCompatible", SupportedCRS: "urn:ogc:def:crs:EPSG::3857", TileMatrix: [0, 1].map((z) => ({
        Identifier: String(z), ScaleDenominator: 559082264.0287178 / Math.pow(2, z), TopLeftCorner: [-20037508.3427892, 20037508.3427892],
        TileWidth: 256, TileHeight: 256, MatrixWidth: Math.pow(2, z), MatrixHeight: Math.pow(2, z) })) };
    const wmts = map.normalizeConfig({ profile: { basemaps: [{ id: "T", kind: "wmts", url: "https://tiles.example.org/wmts",
        ogc: { layer: "Topo", style: "default", format: "image/png", matrixSet, projection: matrixSet.SupportedCRS, requestEncoding: "KVP", dimensions: { Time: "2026-11-01", Other: "x" } } }] } }).profile.basemaps[0];
    assert.equal(wmts.ogc.requestEncoding, "KVP");
    assert.deepEqual(wmts.ogc.dimensions, { Time: "2026-11-01" });
    const kvp = new O.WMTS({ url: wmts.url, layer: wmts.ogc.layer, style: wmts.ogc.style, format: wmts.ogc.format, matrixSet: matrixSet.Identifier,
        tileGrid: O.createFromCapabilitiesMatrixSet(JSON.parse(JSON.stringify(wmts.ogc.matrixSet))), projection: wmts.ogc.projection,
        requestEncoding: "KVP", dimensions: wmts.ogc.dimensions });
    assert.deepEqual(kvp.getTileGrid().getMatrixIds(), ["0", "1"]);
    const kvpUrl = new URL(kvp.getTileUrlFunction()([1, 1, 0], 1, O.getProjection("EPSG:3857")));
    assert.equal(kvpUrl.origin + kvpUrl.pathname, "https://tiles.example.org/wmts");
    assert.deepEqual(["layer", "TileMatrix", "TileCol", "TileRow", "Time"].map((name) => kvpUrl.searchParams.get(name)), ["Topo", "1", "1", "0", "2026-11-01"]);
    const rest = new O.WMTS({ url: "https://tiles.example.org/{Layer}/{Style}/{TileMatrixSet}/{TileMatrix}/{TileRow}/{TileCol}.png", layer: "Topo",
        style: "default", format: "image/png", matrixSet: matrixSet.Identifier, tileGrid: O.createFromCapabilitiesMatrixSet(JSON.parse(JSON.stringify(matrixSet))),
        projection: matrixSet.SupportedCRS, requestEncoding: "REST" });
    assert.equal(rest.getTileUrlFunction()([1, 0, 1], 1, O.getProjection("EPSG:3857")), "https://tiles.example.org/Topo/default/GoogleMapsCompatible/1/1/0.png");
    const broken = JSON.parse(JSON.stringify(matrixSet));
    broken.TileMatrix[1].TileWidth = 0;
    assert.equal(map.normalizeOgc({ layer: "Topo", matrixSet: broken }, "wmts"), null, "a matrix that is not whole numbers");
    const origins = map.allowedOrigins(map.normalizeConfig({ profile: { basemaps: [wmts] }, layers: [{ id: "R", kind: "wmts", url: "https://rest.example.net/{TileMatrix}/{TileRow}/{TileCol}.png", ogc: { layer: "Topo", matrixSet } }] }), page);
    assert.ok(origins.has("https://tiles.example.org") && origins.has("https://rest.example.net"));

    // WFS: GeoJSON for the view's box, in the srsName's units, at most maxFeatures
    const wfs = map.normalizeConfig({ layers: [{ id: "F", kind: "wfs", url: "/apps/maps/LocalMap/providerProxy/P/wfs",
        ogc: { typeName: "parks", outputFormat: "application/json", projection: "EPSG:4326", maxFeatures: "250" } }] }).layers[0];
    assert.equal(wfs.ogc.maxFeatures, 250);
    assert.equal(map.normalizeOgc({ typeName: "t" }, "wfs").maxFeatures, 1000, "the default count a typical answer fits in");
    assert.equal(map.wfsDataProjection(wfs.ogc.srsName), "EPSG:4326");
    assert.equal(map.wfsDataProjection("urn:ogc:def:crs:EPSG::3857"), "EPSG:3857");
    const wfsUrl = new URL(map.wfsUrl(page + wfs.url, wfs.ogc, [-87.7, 41.8, -87.6, 41.9]));
    assert.equal(wfsUrl.origin, page);
    assert.deepEqual(["REQUEST", "TYPENAMES", "OUTPUTFORMAT", "SRSNAME", "BBOX", "COUNT"].map((name) => wfsUrl.searchParams.get(name)),
        ["GetFeature", "parks", "application/json", "EPSG:4326", "-87.7,41.8,-87.6,41.9,EPSG:4326", "250"]);

    // a srsName read latitude first gets its box latitude first; CRS84 and the short EPSG:4326 longitude first
    const latFirst = new URL(map.wfsUrl(page + "/wfs", Object.assign({}, wfs.ogc, { srsName: "urn:ogc:def:crs:EPSG::4326" }), [-87.7, 41.8, -87.6, 41.9]));
    assert.equal(latFirst.searchParams.get("BBOX"), "41.8,-87.7,41.9,-87.6,urn:ogc:def:crs:EPSG::4326");
    assert.ok(!map.wfsLatitudeFirst("CRS:84") && !map.wfsLatitudeFirst("EPSG:4326") && map.wfsLatitudeFirst("http://www.opengis.net/def/crs/EPSG/0/4326"));
});

test("m-map builds its WMS, WMTS and WFS layers only for the URL it was given, with CORS as the provider answers", async () => {
    const O = await olLoad;
    const methods = map.components["m-map-view"].methods;
    const matrixSet = { Identifier: "g", SupportedCRS: "EPSG:3857", TileMatrix: [{ Identifier: "0", ScaleDenominator: 559082264.0287178,
        TopLeftCorner: [-20037508.3427892, 20037508.3427892], TileWidth: 256, TileHeight: 256, MatrixWidth: 1, MatrixHeight: 1 }] };
    const config = map.normalizeConfig({ layers: [
        { id: "W", kind: "wms", url: "https://maps.example.org/wms", ogc: { params: { LAYERS: "roads", FORMAT: "image/png" }, projection: "EPSG:3857", cors: false } },
        { id: "I", kind: "wms", url: "/apps/maps/LocalMap/providerProxy/P/wms", ogc: { params: { LAYERS: "roads" }, projection: "EPSG:3857", singleImage: true } },
        { id: "T", kind: "wmts", url: "https://tiles.example.org/wmts", ogc: { layer: "topo", matrixSet, projection: "EPSG:3857" } },
        { id: "F", kind: "wfs", url: "https://features.example.org/wfs", ogc: { typeName: "parks", projection: "EPSG:3857" } },
        { id: "X", kind: "wms", url: "https://elsewhere.example/wms", ogc: { params: { LAYERS: "roads" } } }] });
    const statuses = [];
    // a stand-in for the component: its own methods, the profile's origins, no map drawn
    const self = { origins: map.allowedOrigins(config, page), pageOrigin: () => page, status: (...args) => statuses.push(args), watchSource: () => {},
        featureStyle: () => null };
    for (const name of ["checkUrl", "ogcOf", "ogcCrossOrigin", "wmsLayer", "wmtsLayer", "wfsLayer"]) self[name] = methods[name].bind(self);
    self.origins.delete("https://elsewhere.example");
    const built = {};
    for (const layer of config.layers) {
        const group = new O.LayerGroup();
        try { self[layer.kind + "Layer"](O, group, layer, ""); built[layer.id] = group.getLayers().item(0); } catch (err) { built[layer.id] = err.message; }
    }
    const source = (id) => built[id].getSource();
    assert.ok(source("W") instanceof O.TileWMS);
    assert.ok(source("W").crossOrigin === null, "a provider that answers no CORS is asked as a plain image");
    assert.ok(source("I") instanceof O.ImageWMS, "one image of the view");
    assert.ok(source("T") instanceof O.WMTS);
    assert.equal(source("T").crossOrigin, "anonymous", "no page credentials for a provider that answers CORS");
    assert.equal(source("T").getTileUrlFunction()([0, 0, 0], 1, O.getProjection("EPSG:3857")).split("?")[0], "https://tiles.example.org/wmts");
    assert.ok(source("F") instanceof O.VectorSource);
    assert.equal(built.F.get("overlayLayerId"), "F", "its features are selected as an overlay's");
    assert.ok(/is not a host of the map's profile/.test(built.X), "a layer of a host the profile does not name is refused");
    assert.equal(map.ogcCrossOrigin(page + "/apps/x", { cors: true }, page), undefined, "this server's proxy is asked with the session");
});

test("an edit is saved as plain GeoJSON: WGS84 to seven decimals, plain text, unique ids; a keyboard shape needs its vertices", async () => {
    await olLoad;
    const edited = map.editedCollection({ type: "FeatureCollection", features: [
        { type: "Feature", id: "a", geometry: { type: "Point", coordinates: [190.123456789, 10] },
            properties: { label: "<b>Spring</b>", other: "dropped", description: "Fresh <script>alert(1)</script>water" } },
        { type: "Feature", id: "a", geometry: { type: "LineString", coordinates: [[0, 0], [1, 1]] }, properties: { name: "Trail" } },
        { type: "Feature", geometry: { type: "GeometryCollection", geometries: [] } },
        { type: "Feature", id: "<x>", geometry: { type: "Polygon", coordinates: [[[0, 0], [1, 0], [1, 1], [0, 0]]] }, properties: {} },
        { type: "Feature", id: "bad", geometry: { type: "Point", coordinates: ["a", 1] } }] });
    assert.deepEqual(edited.features.map((feature) => feature.id), ["a", "f2", "f4"], "a used or unplain id is made from its number");
    assert.deepEqual(edited.features[0].geometry.coordinates, [-169.8765432, 10], "a wrapped longitude back in range");
    assert.deepEqual(edited.features[0].properties, { label: "Spring", description: "Fresh water" });
    assert.deepEqual(edited.features[1].properties, { label: "Trail" });
    assert.deepEqual(map.editedCollection(null), { type: "FeatureCollection", features: [] });
    // from the keyboard: a point of one vertex, a line of two, an area of three, closed
    assert.deepEqual(map.sketchGeometry("Point", [[1, 2]]), { type: "Point", coordinates: [1, 2] });
    assert.equal(map.sketchGeometry("LineString", [[1, 2]]), null);
    assert.deepEqual(map.sketchGeometry("Polygon", [[0, 0], [1, 0], [1, 1]]).coordinates, [[[0, 0], [1, 0], [1, 1], [0, 0]]]);
    assert.equal(map.sketchGeometry("Polygon", [[0, 0], [1, 0]]), null);
    assert.equal(map.sketchGeometry("Circle", [[0, 0]]), null);
    // only one's own overlay drawn whole is edited
    const own = { overlayId: "7", isOwn: true, kind: "geojson" };
    assert.equal(map.canEditOverlay(own), true);
    assert.equal(map.canEditOverlay(Object.assign({}, own, { isOwn: false })), false, "shared with the user");
    assert.equal(map.canEditOverlay(Object.assign({}, own, { tilesStatus: "ready", kind: "mvt" })), false, "simplified into tiles");
    assert.equal(map.canEditOverlay({ id: "PARKS", kind: "geojson" }), false, "a profile's layer");
    assert.ok(typeof MoquiOl.Snap === "function" && typeof MoquiOl.Draw === "function" && typeof MoquiOl.Modify === "function", "the bundle has the editing interactions");
});

test("the raster and projection part loads only for a map that needs it: a COG layer or a registered projection", () => {
    const utm = { code: "EPSG:32633", label: "UTM 33N", proj4: "+proj=utm +zone=33 +datum=WGS84 +units=m +no_defs", bounds: [12, 0, 18, 84] };
    const cog = { id: "DEM", kind: "cog", url: "/apps/maps/LocalMap/source?layerId=DEM&revision=r1",
        raster: { bands: 1, sampleType: "float32", projection: "EPSG:32633", style: { band: 1, min: 0, max: 3000, colormap: "terrain" } } };
    assert.equal(map.needsRaster(map.normalizeConfig({})), false, "the outline");
    assert.equal(map.needsRaster(map.normalizeConfig({ projection: "EPSG:4326", layers: [{ id: "X", kind: "xyz", url: "/t/{z}/{x}/{y}.png" }] })), false,
        "EPSG:4326 and a raster of tiles");
    assert.equal(map.needsRaster(map.normalizeConfig({ layers: [cog], projections: [utm] })), true, "a COG layer");
    assert.equal(map.needsRaster(map.normalizeConfig({ projection: "EPSG:32633", projections: [utm] })), true, "a registered view");
    // a view in a projection it was not given is Web Mercator, and needs nothing
    const unknown = map.normalizeConfig({ projection: "EPSG:32633" });
    assert.equal(unknown.projection, "EPSG:3857");
    assert.equal(map.needsRaster(unknown), false);
    const wms = map.normalizeConfig({ projections: [utm], layers: [{ id: "W", kind: "wms", url: "https://maps.example.org/wms",
        ogc: { params: { LAYERS: "a" }, projection: "EPSG:32633" } }] });
    assert.equal(map.needsRaster(wms), true, "a WMS layer in a registered projection");
    assert.equal(window.MoquiOlRaster, undefined, "deciding loads nothing");
    // a definition that names a grid or anything but plain parameters is not registered
    const given = map.normalizeProjections([utm, { code: "EPSG:4267", proj4: "+proj=longlat +datum=NAD27 +nadgrids=conus" },
        { code: "EPSG:9999", proj4: "+proj=utm +zone=1 +x=https://example.org/x" }, { code: "EPSG:3857", proj4: "+proj=merc" }, utm]);
    assert.deepEqual(given.map((row) => row.code), ["EPSG:32633"]);
    // vector tiles are drawn only in Web Mercator; rasters and vector files in any
    assert.equal(map.drawableIn({ kind: "style" }, "EPSG:3413"), false);
    assert.equal(map.drawableIn({ kind: "pmtiles", raster: false }, "EPSG:3413"), false);
    assert.equal(map.drawableIn({ kind: "pmtiles", raster: true }, "EPSG:3413"), true);
    assert.equal(map.drawableIn({ kind: "wmts" }, "EPSG:3413"), true);
    assert.equal(map.drawableIn({ kind: "style" }, "EPSG:3857"), true);
});

test("a COG layer's band settings are checked and drawn as a WebGL style: bands, stretch, colormap, nodata", () => {
    const layer = (style) => map.normalizeConfig({ layers: [{ id: "C", kind: "cog", url: "/c.tif", raster: { bands: 4, projection: "EPSG:3857", style,
        defaults: { rgb: [1, 2, 3], min: 0, max: 255 } } }] }).layers[0].cog;
    assert.deepEqual(layer({ rgb: [4, 3, 2], min: 0, max: 3000 }).style, { rgb: [4, 3, 2], min: 0, max: 3000 });
    assert.deepEqual(layer({ rgb: [5, 3, 2], min: 0, max: 3000 }).style, { rgb: [1, 2, 3], min: 0, max: 255 }, "a band it does not have: its defaults");
    assert.deepEqual(layer({ band: 2, min: 10, max: 10 }).style, { rgb: [1, 2, 3], min: 0, max: 255 }, "min not below max");
    assert.equal(layer({ band: 2, min: 0, max: 1, colormap: "<b>" }).style.colormap, "grey", "an unknown colormap is grey");
    assert.equal(map.normalizeRaster({ bands: 3, projection: "not a code", style: { band: 1, min: 0, max: 1 } }), null);
    const rgb = map.rasterStyle({ rgb: [4, 3, 2], min: 100, max: 3100 });
    assert.deepEqual(rgb.variables, { min: 100, range: 3000 });
    assert.equal(rgb.color[0], "array");
    assert.deepEqual(map.rasterBands({ rgb: [4, 3, 2] }), [4, 3, 2]);
    const single = map.rasterStyle({ band: 2, min: 0, max: 50, colormap: "viridis", nodata: -9999 });
    assert.equal(single.color[0], "case", "nodata transparent");
    assert.deepEqual(single.color[1], ["==", ["band", 2], 0], "its alpha is the band after the one read");
    assert.equal(single.color[3][0], "interpolate");
    assert.deepEqual(map.rasterBands({ band: 2 }), [2]);
});

test("the raster part registers the projections it is given and looks up none", async () => {
    const O = await olLoad;
    const R = await map.loadRaster();
    assert.equal(R, window.MoquiOlRaster);
    assert.ok(typeof R.GeoTIFF === "function" && typeof R.WebGLTileLayer === "function", "the GeoTIFF source and the WebGL tile layer");
    assert.notEqual(O.getProjection("EPSG:3857"), null, "one OpenLayers: the main bundle's projections");
    const laea = { code: "EPSG:3035", proj4: "+proj=laea +lat_0=52 +lon_0=10 +x_0=4321000 +y_0=3210000 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs",
        extent: [1896628.62, 1507846.05, 4662111.45, 6829874.45], bounds: [-35.58, 24.6, 44.83, 84.73] };
    const utm = { code: "EPSG:32633", proj4: "+proj=utm +zone=33 +datum=WGS84 +units=m +no_defs", extent: [166021.44, 0, 833978.56, 9329005.18],
        bounds: [12, 0, 18, 84] };
    // a UTM zone OpenLayers makes itself; LAEA Europe from its definition, as the server gave it
    const added = map.registerProjections(O, R, map.normalizeProjections([laea, utm]));
    assert.deepEqual(added, ["EPSG:3035"]);
    const [east, north] = O.fromLonLat([5, 50], "EPSG:3035");
    assert.ok(Math.abs(east - 3962799.45) < 0.01 && Math.abs(north - 2999718.85) < 0.01, "the EPSG guidance example: " + east + " " + north);
    const before = performance.getEntriesByType("resource").filter((entry) => !entry.name.startsWith(page)).length;
    // a code it was not given is refused, never asked of spatialreference.org
    await assert.rejects(R.fromProjectionCode("EPSG:2056"), /not registered/);
    assert.equal(performance.getEntriesByType("resource").filter((entry) => !entry.name.startsWith(page)).length, before, "no outside request");
    assert.deepEqual(map.registerProjections(O, R, map.normalizeProjections([laea])), [], "once");
    const [x, y] = O.fromLonLat([15, 0], "EPSG:32633");
    assert.ok(Math.abs(x - 500000) < 1e-6 && Math.abs(y) < 1e-6, x + " " + y);
    assert.deepEqual(O.getProjection("EPSG:32633").getExtent(), [166021.44, 0, 833978.56, 9329005.18]);
});

test("the build's files are named by their hash, and its decoder worker inflates a Deflate tile", async () => {
    await olLoad;
    const files = window.MoquiOlFiles;
    for (const role of ["main", "raster", "decoder"]) assert.ok(/^ol-[a-z-]+-[A-Z0-9]+\.min\.js$/.test(files[role]), role + ": " + files[role]);
    // a tile of 4 by 4 bytes, compressed as a TIFF's Deflate is (zlib), decoded by the build's module worker
    const pixels = Uint8Array.from({ length: 16 }, (unused, index) => (index * 17) % 256);
    const compressed = new Uint8Array(await new Response(new Blob([pixels]).stream().pipeThrough(new CompressionStream("deflate"))).arrayBuffer());
    const worker = new Worker("/libs/openlayers/" + files.decoder, { type: "module" });
    try {
        const answer = await new Promise((resolve, reject) => {
            const timer = setTimeout(() => reject(new Error("the decoder did not answer")), 10000);
            worker.addEventListener("message", (event) => { clearTimeout(timer); resolve(event.data); });
            worker.addEventListener("error", (event) => { clearTimeout(timer); reject(new Error("the decoder worker failed: " + (event.message || "to load"))); });
            worker.postMessage({ compression: 8, decoderParameters: { tileWidth: 4, tileHeight: 4, planarConfiguration: 1, bitsPerSample: [8], predictor: 1 },
                buffer: compressed.buffer, jobId: 1 }, [compressed.buffer]);
        });
        assert.equal(answer.error, undefined);
        assert.equal(answer.jobId, 1);
        assert.deepEqual(Array.from(new Uint8Array(answer.decoded)), Array.from(pixels));
    } finally {
        worker.terminate();
    }
});
