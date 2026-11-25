/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// m-map's pure helpers in the page (moqui.map of /js/WebrootMap.js), and its list of places when the map cannot be drawn. The
// OpenLayers bundle is not loaded: drawing the map is a browser walk, not a regression test. The page is https://moqui.test.
"use strict";

const map = moqui.map;
const page = location.origin;

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
