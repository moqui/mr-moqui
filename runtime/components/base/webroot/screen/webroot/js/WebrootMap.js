/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The map widget (plans/archive/Maps.md §3.2), on the vendored OpenLayers bundle (libs/openlayers, an ES module build whose files
 * are named by their hash: map.loadOpenLayers reads ol-map.json for its entries, imports the main one and sets it as the
 * global MoquiOl, with ol-mapbox-style and the PMTiles sources). m-map of WebrootVue.qvt.js loads this
 * file, the bundle and ol.css when the first map mounts and draws m-map-view by its definition; the map-view XML element
 * writes m-map. A render function, not a template, so it needs no template compiler. The pure helpers are moqui.map, which
 * test/client/pages/WebrootMap.page.js runs in the page.
 *
 * The raster and projection part (the build's raster entry, P4c: OpenLayers' GeoTIFF source and WebGL tile layer,
 * geotiff.js, proj4) is imported only when a map needs it (map.needsRaster: a COG layer, or a view or a layer in a
 * projection other than EPSG:3857 and EPSG:4326), and shares the main bundle's OpenLayers. A projection is registered with
 * proj4 from the configuration's projections (the server's MapProjection rows), never fetched: the part refuses any lookup of
 * a code it was not given. geotiff decodes in module workers of this server (the build's decoder entry), so a page whose
 * Content-Security-Policy is stricter than the default needs worker-src to allow 'self'; the LERC and Zstandard decoders are
 * WebAssembly, instantiated in the worker.
 *
 * Coordinates are WGS84 degrees, longitude first wherever they are a pair or a list (GeoJSON's order): a point {lon, lat},
 * a center [lon, lat], an extent [west, south, east, north] (west greater than east crosses the antimeridian). Text shows
 * latitude, longitude, as people read them. The services' point is {longitude, latitude}: the same numbers.
 *
 * The configuration (a JSON prop, or the answer of config-url, which the map-view's own attributes then override):
 *   {
 *     profileId: "OUTLINE",
 *     profile: {                       the effective profile; without one, or without basemaps, the outline below
 *       profileId, profileName,
 *       basemaps: [{id, label, kind, url, styleUrl, attribution, attributionUrls, minZoom, maxZoom, raster}],
 *         kind outline: url a TopoJSON of countries (the Natural Earth file, /libs/naturalearth/countries-110m.json)
 *         kind style:   url a MapLibre/OpenMapTiles style document, drawn by ol-mapbox-style
 *         kind xyz:     url a raster tile template ({z}, {x}, {y})
 *         kind pmtiles: url a PMTiles archive of this server (same origin only: a registered archive's is the map-source
 *                       transition's, naming its revision, and a 409 from it loads the configuration again), styleUrl
 *                       its style (raster: true for a raster archive); a style's vector source is bound to the archive
 *                       (applyStyle, updateSource false)
 *         kind wms:     url a WMS 1.3.0 service (its GetMap address, or this server's proxy), ogc {params: {LAYERS, STYLES,
 *                       FORMAT, TRANSPARENT, VERSION, TIME}, projection, singleImage}: TileWMS, or ImageWMS for one image
 *         kind wmts:    url a WMTS service's KVP address or tile template, ogc {layer, style, format, matrixSet (as the
 *                       server recorded it from the capabilities: {Identifier, SupportedCRS, TileMatrix}), projection,
 *                       requestEncoding KVP or REST, dimensions}: the tile grid is made from matrixSet, so the browser never
 *                       reads the capabilities
 *         kind cog:     url a Cloud Optimized GeoTIFF (this server's map-source transition, its revision pinned, a 409
 *                       loading the configuration again; or a tested provider's URL, read with no credentials of the page),
 *                       raster {bands, sampleType, projection (its EPSG code), nodata, cors, style: {rgb: [r, g, b] or band
 *                       with colormap (grey, viridis, terrain, magma), min, max, nodata}}: drawn with WebGL (map.rasterStyle)
 *       basemap: the id of the one drawn first (else the first),
 *       allowedOrigins: ["https://tiles.example.org"]   origins its styles' sources, sprites and tiles may also come from
 *       notice: {text, href, linkText}  shown over the map: the effective configuration gives an administrator the way to
 *                                       Configuration when no provider is enabled
 *     },
 *     layers: ["PARKS"] (IDs asked for) or [{id, label, kind (geojson, gpx, kml, topojson, xyz, pmtiles, wms, wmts, wfs), url,
 *             attribution, visible, ogc}] (as the effective configuration resolved them); an ID left unresolved shows as not
 *             available. kind wfs: url a WFS 2.0.0 service, ogc {typeName, outputFormat (GeoJSON), srsName, version,
 *             maxFeatures}: read as GeoJSON for the view's box (the bbox strategy), at most maxFeatures at a time,
 *     center: [lon, lat], zoom: 0-22, extent: [west, south, east, north], projection: "EPSG:3857" | "EPSG:4326" | a code of
 *             projections (a vector-tile basemap or layer is drawn only in EPSG:3857: refused in another, with the reason),
 *     projections: [{code, label, proj4, extent (in its units), bounds (WGS84)}]  the registered projections the map needs,
 *     points: [{id, lon, lat, label, precisionEnumId, precision, source}]  (longitude/latitude, sourceText are read too),
 *     features: a GeoJSON FeatureCollection in WGS84 (each feature's label or name property its label),
 *     pick: true to let the user choose a point, pickForm: the name of a form of the page whose latitude and longitude
 *           fields a choice fills (the map-view's pick-form),
 *     adapter, recordKey, addressPurpose: a record shown through a registered adapter, sent to config-url as given,
 *     layerErrors: [{id, label, reasonCode, message}]  what the server did not resolve (id "record": the record's place),
 *                  each named under the map, with Retry (the configuration asked again) when it came from config-url,
 *     featuresTruncated: true when a record layer had more features than were sent, units: "metric" | "imperial" (the
 *                  scale line's),
 *     locationShares: [{shareId, role, label, ownerName, thruDate, approximate, keepTrail, latest {latitude, longitude,
 *                  accuracy, ageSeconds}, trail [[lon, lat]]}]  the live layer's shares (get#EffectiveConfig with
 *                  locationShares, as Maps › Browse asks for it; map.normalizeLiveShares)
 *   }
 * The live layer (plans/Location-Sharing.md §3.3): each share with a position is a marker labelled with its label and the
 * position's age (canvas text, never markup), its accuracy a ring and a kept trail a line; one with no report for
 * map.LIVE_STALE_SECONDS is drawn grey and dashed, and listed as stale under the map with the rest (label, age, latitude,
 * longitude, approximate). The map follows each share it shows on the topic locationShare:<shareId> of the shell's
 * notification socket (registerListener), from when it is listed until it is no longer listed, ends or is revoked for this
 * viewer, or the map unmounts: a position push moves its marker, ring and trail; changed takes a new end; ended (and a
 * viewer's revoked) takes it off for as long as the map is mounted. A configuration whose shares alone changed changes the
 * layer without drawing the map again; a position newer than the read's is kept (map.mergeLiveShares).
 * The network contracts of the bundle (build/libs/refresh.mjs, OPENLAYERS_HOSTS; plans/archive/Maps.md §3.2): a mapbox:// URL is
 * refused before it reaches the library; a style is loaded here, checked, its sources' attributions escaped, and its
 * metadata['ol:webfonts'] replaced with the local fonts' absolute template (map.webfonts), which is also the webfonts option;
 * a TileJSON is fetched here and handed on with its attribution escaped; KML is read with an iconUrlFunction answering a blank
 * data: icon (map.kmlOptions) and styled here; and nothing is asked of a host that is not the page's or the profile's own
 * (urlAllowed, and transformRequest for what ol-mapbox-style fetches). An OGC layer asks only its url's host (its provider's
 * origin or this server's proxy), with no credentials of the page for a provider that answers CORS (ogc.cors; one that does
 * not is asked as a plain image, which a fetched WFS cannot be). Locate me asks the browser for the position only when
 * pressed, and the position is neither kept nor sent.
 *
 * Editing (plans/archive/Maps.md §4.1, P4b; Browse's own overlays): editCommand(name, value) of the component (m-map passes it on)
 * edits a FeatureCollection in a layer of its own over the map while the overlay it came from is hidden: start {layerId,
 * collection (GeoJSON, WGS84), pointsOnly}, draw ("Point", "LineString", "Polygon" or "" for none: OpenLayers' Draw), with
 * the selected feature's vertices moved by Modify and snapped to the others' (Snap); from the keyboard Enter adds a vertex at
 * the map's center (a point at once), finish ends the shape and Enter with no shape being drawn selects the feature at the
 * center; delete (also the Delete key) removes the selected feature, details {label, description} sets its plain text,
 * collection answers the edited FeatureCollection in WGS84 (map.editedCollection), saved marks it unchanged and stop ends the
 * editing. It emits edit-change ({count, dirty, selected: {id, label, description} or null, sketch, drawType}). The map drawn
 * again (another configuration) keeps the editing, its changes and all.
 *
 * Package mode (plans/Maps-Offline.md §2.5): an offline map package on the device, which the desktop and mobile apps serve
 * to their shell at /__moqui-app/maps/<packageId>/<path> (moqui_client::packages::serve). The configuration is then
 *   {packageBase: "/__moqui-app/maps/<packageId>/", manifest: (the package's manifest.json), and the view or places of the
 *    page's own, which win}
 * and map.fromManifest makes it of the manifest's frozen configuration: each URL in it package-relative, resolved against
 * packageBase (one that is absolute, rooted or leaves the package is dropped, and its layer fails by name); the allowed
 * origin is the package path alone (urlAllowed takes packageBase: the page's own origin outside it is refused too); the
 * label fonts (ol:webfonts) and a style's sprite are the package's (packageFonts, packageStyle); a PMTiles archive is read
 * by range from the package; the outline is its outline.json; and a basemap's attribution is the manifest's, with the
 * source's own notice (an archive's metadata, often HTML) as plain text, never markup. No outside request is ever made.
 * Everything else (Locate me, places, overlays, selection) is as above; showPlace(place) marks a place a page found.
 *
 * Events: pick ({lon, lat}, also the DOM event m-map-pick with that detail, which bubbles), select (a place or a feature:
 * {id, lon, lat, label}, with layerId for a record layer's feature; an overlay layer's feature is kind "overlay" with its
 * layerId, layerLabel and description, label and description plain text (map.plainText), shown as text, never as markup)
 * and view-change ({center, zoom, extent}), and live-change ({shares, removed}: the live shares, as held, after each change, and the share an ended or a viewer's revoked
 * push took off; a live
 * share selected is a select of kind liveShare with its shareId). A feature is selected by a click, or from the keyboard by Enter on the map,
 * which selects the feature at its center (map.nearestFeature, 12 pixels; a map that asks for a choice takes the center
 * instead); the selection is announced. Each place listed has Copy, which puts its latitude, longitude on the clipboard.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui || (root.moqui = {});
    const map = moqui.map = moqui.map || {};

    const L = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);

    // ---- pure helpers ----

    // The label fonts of the packaged styles (refresh.mjs MAP_FONT_FACES), in ol-mapbox-style's webfonts template form. The
    // template is absolute (map.webfonts): ol-mapbox-style resolves a root-relative one against the style's URL, which for a
    // hosted style is the provider's host.
    const WEBFONTS = "/libs/mapfonts/{font-family}/{fontweight}{-fontstyle}.css";
    map.WEBFONTS = WEBFONTS;
    map.webfonts = (pageOrigin) => pageOrigin + WEBFONTS;
    // The icon of every KML IconStyle (iconUrlFunction): a transparent pixel, so reading a file asks no host for an icon.
    const BLANK_ICON = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
    map.BLANK_ICON = BLANK_ICON;
    // The options of OpenLayers' KML format: no styles of the file, no names drawn, and every icon href (and the default
    // pushpin an IconStyle without one gets) replaced, since KML loads an icon as the file is parsed, whatever extractStyles.
    map.kmlOptions = () => ({ extractStyles: false, showPointNames: false, iconUrlFunction: () => BLANK_ICON });
    // The profile drawn when none is given or the given one has no basemap: the country outline, from this server.
    const OUTLINE = { profileId: "OUTLINE", profileName: "Outline", basemaps: [{ id: "OUTLINE_NE110", label: "Country outlines", kind: "outline",
        url: "/libs/naturalearth/countries-110m.json", attribution: "Natural Earth", attributionUrls: ["https://www.naturalearthdata.com/"] }] };
    map.OUTLINE = OUTLINE;
    const KINDS = ["outline", "style", "xyz", "pmtiles", "wms", "wmts", "cog"];
    const LAYER_KINDS = ["geojson", "gpx", "kml", "topojson", "xyz", "pmtiles", "wms", "wmts", "wfs", "mvt", "cog"];
    const BUILT_IN_PROJECTIONS = ["EPSG:3857", "EPSG:4326"];
    const COLORMAPS = {
        grey: [[0, [0, 0, 0]], [1, [255, 255, 255]]],
        viridis: [[0, [68, 1, 84]], [0.25, [59, 82, 139]], [0.5, [33, 145, 140]], [0.75, [94, 201, 98]], [1, [253, 231, 37]]],
        terrain: [[0, [51, 51, 153]], [0.15, [0, 153, 255]], [0.25, [0, 204, 102]], [0.5, [255, 255, 153]], [0.75, [128, 92, 84]], [1, [255, 255, 255]]],
        magma: [[0, [0, 0, 4]], [0.25, [81, 18, 124]], [0.5, [183, 55, 121]], [0.75, [252, 137, 97]], [1, [252, 253, 191]]]
    };
    map.COLORMAPS = Object.keys(COLORMAPS);
    // the OpenLayers build (P4c): every file named by its hash, and this map of its entries ({main, raster, decoder}), the
    // one file of a fixed name, asked with cache: "no-cache" so an entry never pairs with another build's files
    const OL_DIR = "/libs/openlayers/";
    const OL_FILES = OL_DIR + "ol-map.json";
    const fileName = (name) => (typeof name === "string" && /^[A-Za-z0-9_.-]+\.js$/.test(name) ? name : null);
    // the vector files an overlay layer is read from by url
    const FILE_KINDS = ["geojson", "gpx", "kml", "topojson"];
    const PRECISION = { GPP_USER_CONFIRMED: "User-confirmed point", GPP_ADDRESS_MATCH: "Address match", GPP_STREET_RANGE: "Street-range interpolation",
        GPP_POSTAL_AREA: "Postal-area representative point", GPP_PLACE_ESTIMATE: "Place estimate" };

    const blank = (v) => v === null || v === undefined || (typeof v === "string" && (v.trim() === "" || v.trim() === "null"));
    const text = (v) => (blank(v) ? "" : String(v).trim());
    const num = (v) => {
        if (blank(v) || typeof v === "boolean") return null;
        const n = typeof v === "number" ? v : Number(String(v).trim());
        return Number.isFinite(n) ? n : null;
    };
    const lonOk = (lon) => lon !== null && lon >= -180 && lon <= 180;
    const latOk = (lat) => lat !== null && lat >= -90 && lat <= 90;
    const first = (object, ...names) => { for (const name of names) if (!blank(object[name])) return object[name]; return null; };
    // a list of numbers from an array or a comma text, or null when it is not count finite numbers
    const numbers = (v, count) => {
        if (blank(v)) return null;
        const parts = (Array.isArray(v) ? v : String(v).split(",")).map(num);
        return parts.length === count && parts.every((n) => n !== null) ? parts : null;
    };
    const idList = (v) => (Array.isArray(v) ? v : blank(v) ? [] : String(v).split(","));

    // Text for HTML: what the attribution control (which sets innerHTML) and anything else written as markup receives.
    map.escapeHtml = (value) => String(value === null || value === undefined ? "" : value)
        .replace(/[&<>"']/g, (ch) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[ch]);

    // A coordinate as people read it: latitude, longitude, five decimals (about a meter).
    map.formatCoordinates = (lon, lat) => Number(lat).toFixed(5) + ", " + Number(lon).toFixed(5);
    // A longitude brought back into -180 to 180 (a map that wraps gives 190 for -170).
    map.wrapLongitude = (lon) => ((((lon + 180) % 360) + 360) % 360) - 180;
    // A typed position, latitude first as people write it ("41.88, -87.63", "41.88 -87.63"), as {lon, lat}; null unless both
    // are numbers in range.
    map.parseLatLon = (value) => {
        const parts = String(value === null || value === undefined ? "" : value).trim().split(/[\s,;]+/).filter((part) => part);
        if (parts.length !== 2) return null;
        const lat = num(parts[0]), lon = num(parts[1]);
        return latOk(lat) && lonOk(lon) ? { lon, lat } : null;
    };
    // The words of a precision: a GeoPointPrecision enumId's description, or the text given.
    map.precisionText = (value) => (PRECISION[text(value)] ? L(PRECISION[text(value)]) : text(value));

    // A link of a notice: a path of this site or an http(s) address, else none (no javascript:, data: or //host).
    map.safeHref = (href) => {
        const value = text(href);
        if (!value) return "";
        // as a browser reads it (tabs and line breaks dropped, backslashes as slashes), against a stand-in origin
        let parsed;
        try { parsed = new URL(value, "https://page.invalid/"); } catch (err) { return ""; }
        if (parsed.protocol !== "https:" && parsed.protocol !== "http:") return "";
        return parsed.origin === "https://page.invalid" ? (/^[a-z][a-z0-9+.-]*:/i.test(value) ? "" : value) : (/^https?:\/\//i.test(value) ? value : "");
    };
    // A view's extent in WGS84 as view-change gives it: each edge within -180 to 180 (west greater than east across the
    // antimeridian), a view wider than the world the whole of it, latitudes within -90 to 90.
    map.wrapExtent = (extent) => {
        const [west, south, east, north] = extent;
        const clamp = (lat) => Math.max(-90, Math.min(90, lat));
        if (!(east - west < 360)) return [-180, clamp(south), 180, clamp(north)];
        return [map.wrapLongitude(west), clamp(south), map.wrapLongitude(east), clamp(north)];
    };

    // One place, checked: {id, lon, lat, label, precisionEnumId, precision, source} from lon/lat or longitude/latitude, or null
    // when they are not WGS84 degrees. A zero is a coordinate.
    map.normalizePoint = (point, index) => {
        if (!point || typeof point !== "object") return null;
        const lon = num(first(point, "lon", "longitude")), lat = num(first(point, "lat", "latitude"));
        if (!lonOk(lon) || !latOk(lat)) return null;
        const precision = text(first(point, "precisionEnumId", "precision"));
        const out = { id: text(point.id) || "p" + (index || 0), lon, lat, label: text(point.label) };
        if (PRECISION[precision]) out.precisionEnumId = precision;
        out.precision = map.precisionText(precision);
        out.source = text(first(point, "sourceText", "source"));
        return out;
    };
    map.normalizePoints = (points) => (Array.isArray(points) ? points : []).map(map.normalizePoint).filter((point) => point)
        .map((point, index, all) => (all.findIndex((other) => other.id === point.id) < index ? Object.assign({}, point, { id: point.id + "_" + index }) : point));

    const normalizeBasemap = (basemap, index) => {
        if (!basemap || typeof basemap !== "object") return null;
        const kind = text(basemap.kind).toLowerCase();
        const urls = basemap.attributionUrls;
        return { id: text(basemap.id) || "basemap" + index, label: text(basemap.label) || text(basemap.id) || kind, kind, url: text(basemap.url),
            styleUrl: text(basemap.styleUrl), attribution: text(basemap.attribution),
            attributionUrls: Array.isArray(urls) ? urls.map(text) : text(urls) ? text(urls).split("\n").map((url) => url.trim()) : [],
            minZoom: num(basemap.minZoom), maxZoom: num(basemap.maxZoom), raster: basemap.raster === true || basemap.raster === "true",
            ogc: map.normalizeOgc(basemap.ogc, kind), cog: kind === "cog" ? map.normalizeRaster(basemap.raster) : null };
    };
    const normalizeLayer = (layer, index) => {
        if (typeof layer === "string") return text(layer) ? { id: text(layer), requested: true } : null;
        if (!layer || typeof layer !== "object") return null;
        const kind = text(layer.kind).toLowerCase();
        return { id: text(layer.id) || "layer" + index, label: text(layer.label) || text(layer.id), kind, url: text(layer.url),
            styleUrl: text(layer.styleUrl), attribution: text(layer.attribution), visible: layer.visible !== false && layer.visible !== "false",
            ogc: map.normalizeOgc(layer.ogc, kind), maxZoom: num(layer.maxZoom), cog: kind === "cog" ? map.normalizeRaster(layer.raster) : null };
    };

    // ---- rasters and projections (P4c) ----

    const EPSG = /^EPSG:[0-9]{1,6}$/;
    // A COG layer's band settings checked against its band count: {rgb: [r, g, b]} or {band, colormap}, with min below max
    // and an optional nodata; null when they do not read (the layer is then drawn with its defaults, or not at all).
    map.normalizeRasterStyle = (style, bands) => {
        if (!style || typeof style !== "object") return null;
        const bandOk = (band) => Number.isInteger(band) && band >= 1 && band <= (bands || 64);
        const out = {};
        if (Array.isArray(style.rgb)) {
            const rgb = style.rgb.map(num);
            if (rgb.length !== 3 || !rgb.every(bandOk)) return null;
            out.rgb = rgb;
        } else {
            const band = num(style.band);
            if (!bandOk(band)) return null;
            out.band = band;
            out.colormap = COLORMAPS[text(style.colormap)] ? text(style.colormap) : "grey";
        }
        const min = num(style.min), max = num(style.max);
        if (min === null || max === null || !(min < max)) return null;
        out.min = min;
        out.max = max;
        const nodata = num(style.nodata);
        if (nodata !== null) out.nodata = nodata;
        return out;
    };
    // A COG layer's raster as the server resolved it, checked; null when it is not one (the layer then fails by name).
    map.normalizeRaster = (raster) => {
        if (!raster || typeof raster !== "object") return null;
        const bands = num(raster.bands);
        const projection = text(raster.projection).toUpperCase();
        if (!Number.isInteger(bands) || bands < 1 || bands > 64 || !EPSG.test(projection)) return null;
        const style = map.normalizeRasterStyle(raster.style, bands) || map.normalizeRasterStyle(raster.defaults, bands);
        if (!style) return null;
        return { bands, sampleType: shortText(raster.sampleType, 16), projection, nodata: num(raster.nodata), cors: raster.cors !== false,
            style, defaults: map.normalizeRasterStyle(raster.defaults, bands), own: raster.own === true };
    };
    // The registered projections a configuration gives: an EPSG code and a PROJ.4 definition of plain parameters each (no
    // URL, no grid file), with its extent in its own units and its bounds in WGS84 when given.
    map.normalizeProjections = (projections) => (Array.isArray(projections) ? projections : []).map((row) => {
        if (!row || typeof row !== "object") return null;
        const code = text(row.code).toUpperCase();
        const proj4 = text(row.proj4);
        if (!EPSG.test(code) || BUILT_IN_PROJECTIONS.includes(code) || proj4.length > 600 || !/^\+proj=[a-z]+(\s+\+[a-z_0-9]+(=[A-Za-z0-9_.,+@-]+)?)*$/.test(proj4) ||
            /\+(nadgrids|geoidgrids)=(?!@null)/.test(proj4)) return null;
        const extent = numbers(row.extent, 4);
        const bounds = numbers(row.bounds, 4);
        return { code, label: text(row.label) || code, proj4, extent: extent && extent[0] < extent[2] && extent[1] < extent[3] ? extent : null,
            bounds: bounds && lonOk(bounds[0]) && lonOk(bounds[2]) && latOk(bounds[1]) && latOk(bounds[3]) ? bounds : null };
    }).filter((row, index, all) => row && all.findIndex((other) => other && other.code === row.code) === index).slice(0, 50);
    // Whether a basemap or layer may be drawn in a projection: anything in EPSG:3857; vector tiles (a MapLibre style, a vector
    // PMTiles archive, an overlay simplified into tiles) only there, since they are not reprojected.
    map.drawableIn = (entry, projection) => projection === "EPSG:3857" || !(entry.kind === "style" || entry.kind === "mvt" || (entry.kind === "pmtiles" && !entry.raster));
    // The projections a configuration draws in: its view's, its COG layers', its OGC layers'.
    const projectionsUsed = (config) => {
        const out = new Set([config.projection]);
        config.profile.basemaps.concat(config.layers).forEach((entry) => {
            if (entry.cog) out.add(entry.cog.projection);
            if (entry.ogc && entry.ogc.projection) out.add(text(entry.ogc.projection).toUpperCase());
        });
        return out;
    };
    // Whether a map needs the raster and projection part: a COG layer, or a projection that is not EPSG:3857 or EPSG:4326
    // (a registered one, which proj4 draws). A map that needs neither never loads it.
    map.needsRaster = (config) => config.profile.basemaps.concat(config.layers).some((entry) => entry.kind === "cog") ||
        Array.from(projectionsUsed(config)).some((code) => EPSG.test(code) && !BUILT_IN_PROJECTIONS.includes(code));
    // The main bundle: the map of the build's entries read again (revalidated), then its main entry imported once and set
    // as the global MoquiOl. A failure is not kept, so a later map tries again.
    let olLoad = null;
    map.loadOpenLayers = () => {
        if (root.MoquiOl && root.MoquiOlFiles) return Promise.resolve(root.MoquiOl);
        return olLoad || (olLoad = root.fetch(OL_FILES, { cache: "no-cache", credentials: "same-origin" }).then((response) => {
            if (!response.ok) throw new Error("HTTP " + response.status + " for " + OL_FILES);
            return response.json();
        }).then((files) => {
            const main = fileName(files && files.main);
            if (!main || !fileName(files.raster) || !fileName(files.decoder)) throw new Error(OL_FILES + " does not name the build's entries");
            root.MoquiOlFiles = files;
            return import(OL_DIR + main);
        }).then((ol) => (root.MoquiOl = root.MoquiOl || ol), (err) => { olLoad = null; throw err; }));
    };
    // The raster part of the same build, imported once, its decoder workers set to the build's own (each a module worker,
    // whose failure to start fails the COG layers drawn: map.decoderFailed). A failure is not kept.
    let rasterLoad = null;
    const decoderListeners = new Set();
    map.decoderFailed = (event) => decoderListeners.forEach((listener) => { try { listener(event); } catch (err) { console.warn("m-map:", err); } });
    map.loadRaster = () => {
        if (root.MoquiOlRaster) return Promise.resolve(root.MoquiOlRaster);
        const files = root.MoquiOlFiles;
        if (!files || !fileName(files.raster) || !fileName(files.decoder)) return Promise.reject(new Error("The map's library is not loaded"));
        return rasterLoad || (rasterLoad = import(OL_DIR + files.raster).then((part) => {
            part.setWorkerFactory(() => {
                const worker = new root.Worker(OL_DIR + files.decoder, { type: "module" });
                // a worker the page's policy refuses, or that does not load, fails the layers rather than leave them waiting
                worker.addEventListener("error", (event) => map.decoderFailed(event));
                return worker;
            });
            return (root.MoquiOlRaster = part);
        }, (err) => { rasterLoad = null; throw err; }));
    };
    // Register a configuration's projections with proj4 and OpenLayers (each once): its definition (a UTM zone OpenLayers
    // makes itself is left to it), its extent and its area of use. Answers the codes registered with proj4.
    map.registerProjections = (O, R, projections) => {
        const added = [];
        projections.forEach((row) => {
            if (O.getProjection(row.code)) return;
            R.proj4.defs(row.code, row.proj4);
            added.push(row.code);
        });
        // proj4 is registered even with nothing added, so a code no row gave goes to the part's refusing lookup
        R.registerProj4(R.proj4);
        projections.forEach((row) => {
            const projection = O.getProjection(row.code);
            if (!projection) return;
            if (row.extent) projection.setExtent(row.extent);
            if (row.bounds) projection.setWorldExtent(row.bounds);
        });
        return added;
    };
    // The WebGL style of a COG layer's settings: the bands read (as the source's bands, so the style's are 1 to 3, or 1), each
    // stretched from min to max, through a colormap for one band; a nodata value transparent (the source's alpha band after
    // them).
    map.rasterStyle = (style) => {
        const scaled = (band) => ["clamp", ["/", ["-", ["band", band], ["var", "min"]], ["var", "range"]], 0, 1];
        const alphaBand = (style.rgb ? 3 : 1) + 1;
        let color;
        // a vector of 0 to 1 values (the 'array' operator's color); a colormap's stops are color literals
        if (style.rgb) color = ["array", scaled(1), scaled(2), scaled(3), 1];
        else {
            const stops = COLORMAPS[style.colormap] || COLORMAPS.grey;
            color = ["interpolate", ["linear"], scaled(1)].concat(...stops.map(([at, rgb]) => [at, rgb.concat([1])]));
        }
        if (style.nodata !== undefined) color = ["case", ["==", ["band", alphaBand], 0], [0, 0, 0, 0], color];
        return { color, variables: { min: style.min, range: style.max - style.min } };
    };
    // The bands a COG layer's source reads (1 from the first), in the style's order
    map.rasterBands = (style) => (style.rgb ? style.rgb.slice() : [style.band]);

    // ---- OGC layers ----

    // An OGC layer's options as the server resolved them, checked: only the names each kind's source takes, each a short
    // text or a number; null for another kind or nothing usable.
    const shortText = (v, max) => { const value = text(v); return value.length <= (max || 255) && !/[\u0000-\u001f\u007f]/.test(value) ? value : ""; };
    const WMS_PARAMS = ["LAYERS", "STYLES", "FORMAT", "TRANSPARENT", "VERSION", "TIME"];
    map.normalizeOgc = (ogc, kind) => {
        if (!ogc || typeof ogc !== "object" || !["wms", "wmts", "wfs"].includes(kind)) return null;
        const projection = shortText(ogc.projection, 128);
        if (kind === "wms") {
            const given = ogc.params && typeof ogc.params === "object" ? ogc.params : {};
            const params = {};
            WMS_PARAMS.forEach((name) => { const value = shortText(given[name], 1024); if (value || name === "STYLES") params[name] = value; });
            if (!params.LAYERS) return null;
            params.VERSION = "1.3.0";
            return { service: "wms", params, projection: projection || "EPSG:3857", singleImage: ogc.singleImage === true, cors: ogc.cors !== false };
        }
        if (kind === "wmts") {
            const set = ogc.matrixSet && typeof ogc.matrixSet === "object" ? ogc.matrixSet : null;
            const matrices = set && Array.isArray(set.TileMatrix) ? set.TileMatrix.slice(0, 30).map((matrix) => ({
                Identifier: shortText(matrix && matrix.Identifier, 64), ScaleDenominator: num(matrix && matrix.ScaleDenominator),
                TopLeftCorner: numbers(matrix && matrix.TopLeftCorner, 2), TileWidth: num(matrix && matrix.TileWidth), TileHeight: num(matrix && matrix.TileHeight),
                MatrixWidth: num(matrix && matrix.MatrixWidth), MatrixHeight: num(matrix && matrix.MatrixHeight) })) : [];
            const valid = matrices.length && matrices.every((matrix) => matrix.Identifier && matrix.ScaleDenominator > 0 && matrix.TopLeftCorner &&
                [matrix.TileWidth, matrix.TileHeight, matrix.MatrixWidth, matrix.MatrixHeight].every((n) => Number.isInteger(n) && n > 0));
            const layer = shortText(ogc.layer), crs = shortText(set && set.SupportedCRS, 128);
            if (!valid || !layer || !crs) return null;
            const dimensions = {};
            if (ogc.dimensions && typeof ogc.dimensions === "object" && shortText(ogc.dimensions.Time, 64)) dimensions.Time = shortText(ogc.dimensions.Time, 64);
            return { service: "wmts", layer, style: shortText(ogc.style) || "default", format: shortText(ogc.format, 64) || "image/png",
                matrixSet: { Identifier: shortText(set.Identifier, 128), SupportedCRS: crs, TileMatrix: matrices },
                projection: crs, requestEncoding: ogc.requestEncoding === "REST" ? "REST" : "KVP", dimensions, cors: ogc.cors !== false };
        }
        const typeName = shortText(ogc.typeName), maxFeatures = num(ogc.maxFeatures);
        if (!typeName) return null;
        return { service: "wfs", typeName, outputFormat: shortText(ogc.outputFormat, 128) || "application/json", srsName: projection || shortText(ogc.srsName, 128) || "EPSG:3857",
            version: "2.0.0", maxFeatures: Number.isInteger(maxFeatures) && maxFeatures > 0 ? Math.min(maxFeatures, 100000) : 1000, cors: ogc.cors !== false };
    };
    // The data projection of a WFS layer's GeoJSON: EPSG:3857 for a Web Mercator srsName, else EPSG:4326 (GeoJSON's
    // longitude, latitude)
    map.wfsDataProjection = (srsName) => (/(^|[:#\/])(3857|900913|102100|102113)$/.test(text(srsName)) ? "EPSG:3857" : "EPSG:4326");
    // A WFS GetFeature URL of a layer for a box in its srsName's units (west, south, east, north; longitude first, sent latitude
    // first for a srsName read so), at most ogc.maxFeatures features
    // Whether a srsName is EPSG:4326 in a form whose axis order is latitude first (a URN or an http URI of EPSG:4326; the
    // short EPSG:4326 and CRS84 are read longitude first, as GeoJSON is)
    map.wfsLatitudeFirst = (srsName) => /^(urn:(x-)?ogc:def:crs:EPSG:[0-9.]*:4326|http:\/\/www\.opengis\.net\/def\/crs\/EPSG\/0\/4326)$/.test(text(srsName));
    map.wfsUrl = (url, ogc, extent) => {
        const box = map.wfsLatitudeFirst(ogc.srsName) ? [extent[1], extent[0], extent[3], extent[2]] : extent;
        const params = { SERVICE: "WFS", VERSION: "2.0.0", REQUEST: "GetFeature", TYPENAMES: ogc.typeName, OUTPUTFORMAT: ogc.outputFormat,
            SRSNAME: ogc.srsName, BBOX: box.map((n) => Number(n.toFixed(map.wfsDataProjection(ogc.srsName) === "EPSG:3857" ? 2 : 7))).join(",") + "," + ogc.srsName,
            COUNT: String(ogc.maxFeatures) };
        const query = Object.keys(params).map((name) => name + "=" + encodeURIComponent(params[name])).join("&");
        return url + (url.includes("?") ? "&" : "?") + query;
    };
    // The crossOrigin of an OGC layer's images: none for this server (its proxy, with the session), anonymous (no credentials
    // of the page) for a provider whose test saw CORS, and none for one that did not, whose images then load as plain images
    map.ogcCrossOrigin = (url, ogc, pageOrigin) => (map.sameOrigin(url, pageOrigin) || !ogc || ogc.cors === false ? undefined : "anonymous");

    // ---- offline packages (plans/Maps-Offline.md §2.5) ----

    // The path a package's files are read at on the shell's origin, and its check: one package id, a plain name
    const PACKAGE_BASE = /^\/__moqui-app\/maps\/[A-Za-z0-9_.:-]{1,64}\/$/;
    map.isPackageBase = (base) => PACKAGE_BASE.test(text(base)) && !/\/\.+\/$/.test(text(base));
    map.packageBase = (packageId) => {
        const base = "/__moqui-app/maps/" + text(packageId) + "/";
        return map.isPackageBase(base) ? base : "";
    };
    // A package-relative path made the URL of its file (base + path), or "" for one that is not a plain path inside the
    // package: absolute, rooted, with a scheme, a query, a backslash, a percent sign, or a "." or ".." segment
    map.packageUrl = (path, base) => {
        const value = text(path);
        if (!value || !map.isPackageBase(base) || /^[a-z][a-z0-9+.-]*:/i.test(value) || /[\\?#%\u0000-\u001f]/.test(value) ||
            value.split("/").some((part) => !part || part === "." || part === "..")) return "";
        return base + value;
    };
    // The label fonts of a package (its fonts/ member, as OF1's MapPackages.js writes the style's ol:webfonts)
    const PACKAGE_FONTS = "fonts/{font-family}/{fontweight}{-fontstyle}.css";
    map.packageFonts = (pageOrigin, base) => pageOrigin + base + PACKAGE_FONTS;
    // A package's style with its sprite made the package's own absolute URL (the manifest writes it package-relative,
    // which a library would resolve against the style's folder): one that is not a plain package path is dropped
    map.packageStyle = (style, pageOrigin, base) => {
        if (!style || typeof style !== "object") return style;
        const out = Object.assign({}, style);
        const of = (url) => { const made = map.packageUrl(url, base); return made ? pageOrigin + made : ""; };
        if (typeof out.sprite === "string") {
            const sprite = of(out.sprite);
            if (sprite) out.sprite = sprite;
            else delete out.sprite;
        } else if (Array.isArray(out.sprite)) {
            out.sprite = out.sprite.map((entry) => Object.assign({}, entry, { url: of(entry && entry.url) })).filter((entry) => entry.url);
        }
        return out;
    };
    // The configuration a map draws of a package's manifest (its configuration, frozen when it was built): every URL the
    // package's, no allowed origin, no link of the notice, the view its region, and each basemap's attribution with the
    // notice of the data it was cut from (the manifest's sources: an archive's metadata, often HTML) read as plain text
    map.fromManifest = (manifest, base) => {
        const given = manifest && typeof manifest === "object" && manifest.configuration && typeof manifest.configuration === "object" ?
            JSON.parse(JSON.stringify(manifest.configuration)) : {};
        const sources = Array.isArray(manifest && manifest.sources) ? manifest.sources : [];
        const profile = given.profile && typeof given.profile === "object" ? given.profile : {};
        const notice = (member) => {
            const source = sources.find((one) => one && text(one.member) === member);
            return source ? map.attributionText(map.plainText(source.sourceAttribution, 300)) : "";
        };
        profile.basemaps = (Array.isArray(profile.basemaps) ? profile.basemaps : []).filter((basemap) => basemap && typeof basemap === "object").map((basemap) => {
            const own = notice(text(basemap.url));
            const lines = text(basemap.attribution).split("\n").map((line) => line.trim()).filter((line) => line);
            return Object.assign({}, basemap, { url: map.packageUrl(basemap.url, base), styleUrl: basemap.styleUrl ? map.packageUrl(basemap.styleUrl, base) : "",
                attribution: lines.concat(own && !lines.includes(own) ? [own] : []).join("\n") });
        });
        profile.allowedOrigins = [];
        if (profile.notice && typeof profile.notice === "object") profile.notice = { text: text(profile.notice.text) };
        given.profile = profile;
        given.layers = (Array.isArray(given.layers) ? given.layers : []).filter((layer) => layer && typeof layer === "object")
            .map((layer) => Object.assign({}, layer, { url: map.packageUrl(layer.url, base), styleUrl: layer.styleUrl ? map.packageUrl(layer.styleUrl, base) : "" }));
        // what a page of the server's would ask again or send: nothing of it offline
        ["recordLayers", "features", "points", "record", "adapter", "recordKey", "layerErrors", "projections"].forEach((key) => { delete given[key]; });
        given.projection = "EPSG:3857";
        given.packageBase = map.isPackageBase(base) ? base : "";
        return given;
    };
    // A package's file read whole: the apps' scheme answers a file past 16 MiB only by range (moqui_client::packages::serve),
    // so it is asked in slices of that, each with no HTTP cache (a package's URL carries no account), and joined
    const PACKAGE_SLICE = 16 * 1024 * 1024;
    map.readPackageFile = (url) => {
        const parts = [];
        const next = (at) => root.fetch(url, { cache: "no-store", credentials: "same-origin", headers: { Range: "bytes=" + at + "-" + (at + PACKAGE_SLICE - 1) } })
            .then((response) => {
                if (response.status === 200) return response.text().then((whole) => ({ whole }));
                if (response.status !== 206) throw new Error("HTTP " + response.status);
                const range = /\/(\d+)$/.exec(response.headers.get("Content-Range") || "");
                const total = range ? Number(range[1]) : 0;
                return response.arrayBuffer().then((buffer) => {
                    const bytes = new Uint8Array(buffer);
                    parts.push(bytes);
                    return bytes.length && at + bytes.length < total ? next(at + bytes.length) : {};
                });
            });
        return next(0).then((done) => {
            if (done && done.whole !== undefined) return done.whole;
            const joined = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
            let offset = 0;
            parts.forEach((part) => { joined.set(part, offset); offset += part.length; });
            return new root.TextDecoder().decode(joined);
        });
    };
    // What a package was built from, for the offline screen: each source's attribution and licenses as plain text (an
    // archive's own notice is often HTML, never shown as markup)
    map.packageSources = (manifest) => (Array.isArray(manifest && manifest.sources) ? manifest.sources : []).filter((source) => source && typeof source === "object").map((source) => {
        const plain = (value) => (value && typeof value === "object" ? "" : map.attributionText(map.plainText(value, 300)));
        const named = (license) => (license && typeof license === "object" ? license.name || license.license || license.licenseName || license.text || license.attribution : license);
        const licenses = []
            .concat(source.license ? [source.license] : [])
            .concat((Array.isArray(source.licenses) ? source.licenses : []).map(named))
            .concat((Array.isArray(source.imports) ? source.imports : []).flatMap((one) => (Array.isArray(one && one.licenses) ? one.licenses : []).map(named)))
            .map(plain).filter((line, index, all) => line && all.indexOf(line) === index);
        return { kind: text(source.kind), member: text(source.member), attribution: plain(source.attribution),
            sourceAttribution: plain(source.sourceAttribution), licenses };
    }).filter((source) => source.kind);

    // The configuration as m-map-view draws it (the shape at the top of this file), from a server's answer, a map-view's
    // attributes bound in a static template (texts) or a JSON text: numbers parsed, a point, center or extent that is not WGS84
    // degrees left out, layers as objects, and the outline profile when no basemap is given (outline true then).
    map.normalizeConfig = (input) => {
        let given = input;
        if (typeof given === "string") {
            try { given = JSON.parse(given); } catch (err) { given = {}; }
        }
        if (!given || typeof given !== "object") given = {};
        // a package's manifest (package mode): its configuration, the page's own view and places winning
        if (given.manifest && typeof given.manifest === "object") {
            const own = {};
            ["center", "zoom", "extent", "points", "pick", "pickForm", "units"].forEach((key) => { if (!blank(given[key])) own[key] = given[key]; });
            given = Object.assign(map.fromManifest(given.manifest, text(given.packageBase)), own);
        }
        const packageBase = map.isPackageBase(given.packageBase) ? text(given.packageBase) : "";
        const profileIn = given.profile && typeof given.profile === "object" ? given.profile : {};
        const basemaps = (Array.isArray(profileIn.basemaps) ? profileIn.basemaps : []).map(normalizeBasemap).filter((basemap) => basemap);
        const outline = !basemaps.length;
        // a package draws its own outline when it has no basemap
        const outlines = packageBase ? OUTLINE.basemaps.map((basemap) => Object.assign({}, basemap, { url: packageBase + "outline.json" })) : OUTLINE.basemaps;
        const profile = {
            profileId: text(profileIn.profileId) || (outline ? OUTLINE.profileId : text(given.profileId)),
            profileName: text(profileIn.profileName) || (outline ? OUTLINE.profileName : ""),
            basemaps: outline ? outlines.map(normalizeBasemap) : basemaps,
            basemap: text(profileIn.basemap),
            allowedOrigins: packageBase ? [] : (Array.isArray(profileIn.allowedOrigins) ? profileIn.allowedOrigins : []).map(text).filter((origin) => origin),
            notice: profileIn.notice && typeof profileIn.notice === "object" && text(profileIn.notice.text) ?
                { text: text(profileIn.notice.text), href: map.safeHref(profileIn.notice.href), linkText: text(profileIn.notice.linkText) } : null
        };
        const center = numbers(given.center, 2);
        const extent = numbers(given.extent, 4);
        const zoom = num(given.zoom);
        const projection = text(given.projection).toUpperCase();
        const projections = map.normalizeProjections(given.projections);
        return {
            profileId: text(given.profileId) || text(profileIn.profileId),
            profile, outline, packageBase,
            layers: idList(given.layers).map(normalizeLayer).filter((layer) => layer),
            center: center && lonOk(center[0]) && latOk(center[1]) ? center : null,
            zoom: zoom !== null && zoom >= 0 && zoom <= 22 ? zoom : null,
            extent: extent && lonOk(extent[0]) && lonOk(extent[2]) && latOk(extent[1]) && latOk(extent[3]) && extent[1] <= extent[3] ? extent : null,
            projection: BUILT_IN_PROJECTIONS.includes(projection) || projections.some((row) => row.code === projection) ? projection : "EPSG:3857",
            projections,
            points: map.normalizePoints(given.points),
            features: given.features && given.features.type === "FeatureCollection" && Array.isArray(given.features.features) ? given.features : null,
            pick: given.pick === true || given.pick === "true",
            pickForm: text(given.pickForm),
            adapter: text(given.adapter), recordKey: given.recordKey && typeof given.recordKey === "object" ? given.recordKey : null,
            addressPurpose: text(given.addressPurpose),
            layerErrors: map.normalizeLayerErrors(given.layerErrors),
            featuresTruncated: given.featuresTruncated === true || given.featuresTruncated === "true",
            units: text(given.units).toLowerCase() === "imperial" ? "imperial" : text(given.units).toLowerCase() === "metric" ? "metric" : "",
            // the live layer's shares (plans/Location-Sharing.md §3.3), held apart from the drawing: see map.normalizeLiveShares
            locationShares: map.normalizeLiveShares(given.locationShares)
        };
    };
    // What the server did not resolve, each {id, label, reasonCode, message} as text; one without an id is left out.
    map.normalizeLayerErrors = (errors) => (Array.isArray(errors) ? errors : []).filter((error) => error && typeof error === "object" && text(error.id))
        .map((error) => ({ id: text(error.id), label: text(error.label) || text(error.id), reasonCode: text(error.reasonCode), message: text(error.message) }));
    // The scale line's units of a configuration: imperial (feet and miles) or metric, the default.
    map.scaleUnits = (config) => (config && config.units === "imperial" ? "imperial" : "metric");

    // A server's effective configuration with what the map-view itself gave (both normalized): its view (center, zoom, extent,
    // projection) and pick win, its places follow the server's (a record's place first), and a layer it asked for that the
    // server did not resolve stays, as asked, to be shown as not available.
    map.mergeConfig = (server, local) => {
        const out = Object.assign({}, server);
        ["center", "zoom", "extent"].forEach((key) => { if (local[key] !== null) out[key] = local[key]; });
        if (local.projection !== "EPSG:3857") out.projection = local.projection;
        const known = new Set(server.projections.map((row) => row.code));
        out.projections = server.projections.concat(local.projections.filter((row) => !known.has(row.code)));
        out.pick = server.pick || local.pick;
        if (local.units) out.units = local.units;
        if (local.pickForm) out.pickForm = local.pickForm;
        // the map-view's own features (a record's area) beside the server's
        if (local.features) out.features = server.features ? { type: "FeatureCollection", features: server.features.features.concat(local.features.features) } : local.features;
        out.points = map.normalizePoints(server.points.concat(local.points));
        const knownLayers = new Set(server.layers.map((layer) => layer.id));
        out.layers = server.layers.concat(local.layers.filter((layer) => !knownLayers.has(layer.id)));
        if (!server.locationShares.length) out.locationShares = local.locationShares;
        return out;
    };

    // The parameters a config-url is asked with: the profile, the layer IDs, and a record's adapter, key and purpose.
    map.configParameters = (config) => {
        const out = {};
        if (config.profileId) out.profileId = config.profileId;
        const ids = config.layers.map((layer) => layer.id);
        if (ids.length) out.layerIds = ids.join(",");
        if (config.adapter) {
            out.adapterKey = config.adapter;
            if (config.recordKey) out.recordKey = JSON.stringify(config.recordKey);
            if (config.addressPurpose) out.addressPurpose = config.addressPurpose;
        }
        return out;
    };

    const originOf = (url, base) => {
        try { return new URL(url, base).origin; } catch (err) { return null; }
    };
    // The origins a map may ask: the page's, those of the URLs its profile and layers name, and the profile's allowedOrigins.
    map.allowedOrigins = (config, pageOrigin) => {
        const origins = new Set([pageOrigin]);
        // a package's map asks its package's path of the page's origin, nothing else (urlAllowed with its base)
        if (config.packageBase) return origins;
        const add = (url) => {
            if (!url || !/^https?:\/\//i.test(url)) return;
            const origin = originOf(url.replace(/\{[^}]*\}/g, "0"));
            if (origin) origins.add(origin);
        };
        config.profile.basemaps.forEach((basemap) => { if (basemap.kind !== "pmtiles") { add(basemap.url); add(basemap.styleUrl); } });
        config.layers.forEach((layer) => { if (layer.kind !== "pmtiles") add(layer.url); });
        config.profile.allowedOrigins.forEach((origin) => { const found = originOf(origin); if (found) origins.add(found); });
        return origins;
    };
    // Whether a URL may be asked: http or https (a relative URL is the page's), of an allowed origin. A mapbox:// URL, or a
    // data:, blob:, file: or javascript: one, is refused, and so is a host the profile does not name. With a package's
    // base (package mode), only a URL of the page's origin under that path, with no query.
    map.urlAllowed = (url, origins, pageOrigin, packageBase) => {
        const value = text(url);
        if (!value || /^mapbox:/i.test(value)) return false;
        let parsed;
        try { parsed = new URL(value.replace(/\{[^}]*\}/g, "0"), pageOrigin + "/"); } catch (err) { return false; }
        if (parsed.protocol !== "https:" && parsed.protocol !== "http:") return false;
        if (packageBase) return parsed.origin === pageOrigin && parsed.pathname.startsWith(packageBase) && !parsed.search;
        return origins.has(parsed.origin);
    };
    // A PMTiles source of this server's archive at url (pmtiles' FetchSource, the session's cookies), whose 409 calls
    // changed: the archive is no longer the revision the URL names.
    map.pinnedSource = (O, url, changed) => {
        const fetched = new O.FetchSource(url);
        return {
            getKey: () => url,
            getBytes: (offset, length, signal, etag) => fetched.getBytes(offset, length, signal, etag).catch((error) => {
                if (error && /\b409\b/.test(String(error.message))) changed();
                throw error;
            })
        };
    };
    // Whether a URL is the page's own (a PMTiles archive must be: its ranges are read with the session's access).
    map.sameOrigin = (url, pageOrigin) => map.urlAllowed(url, new Set([pageOrigin]), pageOrigin);

    // The URLs a style document names that may not be asked (its sources' url, tiles and GeoJSON data, and its sprite), each
    // resolved against the style's own URL. ol-mapbox-style does not read glyphs (it uses web fonts), so they are not asked.
    // bound: the IDs of sources m-map binds itself (a PMTiles archive's, whose url is pmtiles://), not asked by the library.
    // packageBase: a package's style, whose relative URLs are the package's (resolved against its root), each under it.
    map.refusedStyleUrls = (style, styleUrl, origins, pageOrigin, bound, packageBase) => {
        const refused = [];
        const check = (url) => {
            if (typeof url !== "string" || !url) return;
            let resolved = url;
            if (!/^[a-z][a-z0-9+.-]*:/i.test(url)) {
                const against = packageBase ? pageOrigin + packageBase : new URL(styleUrl, pageOrigin + "/");
                try { resolved = new URL(url.replace(/\{[^}]*\}/g, (m) => "__" + m.slice(1, -1) + "__"), against).href; } catch (err) { resolved = url; }
            }
            if (!map.urlAllowed(resolved, origins, pageOrigin, packageBase)) refused.push(url);
        };
        const sources = style && style.sources && typeof style.sources === "object" ? style.sources : {};
        Object.keys(sources).forEach((id) => {
            if (bound && bound.includes(id)) return;
            const source = sources[id] || {};
            check(source.url);
            (Array.isArray(source.tiles) ? source.tiles : []).forEach(check);
            if (typeof source.data === "string") check(source.data);
        });
        const sprite = style ? style.sprite : null;
        if (typeof sprite === "string") check(sprite);
        else if (Array.isArray(sprite)) sprite.forEach((entry) => check(entry && entry.url));
        return refused;
    };
    // A style document made safe to give ol-mapbox-style: its fonts local (metadata['ol:webfonts'], which wins over the webfonts
    // option, set to the absolute template webfonts), and each source's attribution, which the attribution control sets as
    // HTML, made escaped text.
    map.localStyle = (style, webfonts) => {
        const sources = {};
        const given = style && style.sources && typeof style.sources === "object" ? style.sources : {};
        Object.keys(given).forEach((id) => { sources[id] = map.sanitizeSource(given[id]); });
        return Object.assign({}, style, { sources, metadata: Object.assign({}, style && style.metadata, { "ol:webfonts": webfonts || WEBFONTS }) });
    };
    // A style source or TileJSON with its attribution (HTML from the provider) as escaped text.
    map.sanitizeSource = (source) => (source && typeof source === "object" && source.attribution !== undefined ?
        Object.assign({}, source, { attribution: map.escapeHtml(map.attributionText(source.attribution)) }) : source);

    // The attribution control's HTML for a text of lines and their links (attributionUrls, in order): each line escaped, a
    // link only for an http(s) URL, opening in a new tab. OpenLayers' attribution control sets what it is given as HTML, so
    // everything handed to it is made escaped text here, a package's own source notice too (plans/Maps-Offline.md §2.5:
    // an archive's attribution, often HTML, is shown as text, never as markup).
    map.attributionHtml = (attribution, urls) => text(attribution).split("\n").map((line) => line.trim()).filter((line) => line).map((line, index) => {
        const url = (urls || [])[index];
        return url && /^https?:\/\//i.test(url) ? '<a href="' + map.escapeHtml(url) + '" target="_blank" rel="noopener noreferrer">' + map.escapeHtml(line) + "</a>" :
            map.escapeHtml(line);
    }).join(" · ");
    // A polygon ring of lon/lat pairs made continuous across the antimeridian (a step of more than 180° is the other way
    // round: 179 then -179 is 179 then 181), so a planar map draws no band across the world; a ring around a pole, which would
    // not close so, is left as it is.
    map.unwrapRing = (ring) => {
        if (!Array.isArray(ring) || ring.length < 2) return ring;
        let shift = 0;
        const out = ring.map((point, index) => {
            if (index > 0) {
                const step = point[0] - ring[index - 1][0];
                if (step > 180) shift -= 360;
                else if (step < -180) shift += 360;
            }
            return shift ? [point[0] + shift].concat(point.slice(1)) : point;
        });
        return Math.abs(out[out.length - 1][0] - out[0][0]) > 180 ? ring : out;
    };
    // A source's attribution HTML as plain text (a style document's own, which may hold markup), for attributionHtml.
    map.attributionText = (html) => String(html || "").replace(/<[^>]*>/g, " ").replace(/&copy;/g, "©").replace(/&amp;/g, "&").replace(/\s+/g, " ").trim();

    // A feature's text as plain text, at most max characters: comments, a script's or a style's content and anything shaped
    // as a tag (a profile layer's KML or GPX description may hold HTML; a user's overlay is already plain text from the
    // server) removed, the named entities of HTML's basics and every numeric one read, control characters dropped, line
    // breaks kept. What the entities make is text too: it is shown as text, never as markup.
    const entity = (match, name) => {
        const named = { lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " ", amp: "&" }[name];
        if (named !== undefined) return named;
        const code = /^#x/i.test(name) ? parseInt(name.slice(2), 16) : /^#/.test(name) ? parseInt(name.slice(1), 10) : NaN;
        return Number.isInteger(code) && code > 0 && code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff) ? String.fromCodePoint(code) : match;
    };
    map.plainText = (value, max) => {
        if (value === null || value === undefined || typeof value === "object") return "";
        const limit = max || 2000;
        const out = String(value).replace(/<!--[\s\S]*?(-->|$)/g, " ").replace(/<(script|style)\b[\s\S]*?<\/\1\s*>/gi, " ").replace(/<\/?[A-Za-z][^<>]*>/g, " ")
            .replace(/&(lt|gt|quot|apos|nbsp|amp|#[0-9]{1,7}|#[xX][0-9A-Fa-f]{1,6});/g, entity)
            .replace(/\r\n?/g, "\n").replace(/[\u0000-\u0009\u000b-\u001f\u007f]/g, " ").replace(/[ \t]+/g, " ").replace(/ *\n */g, "\n").trim();
        return out.length > limit ? out.slice(0, limit - 1) + "…" : out;
    };
    // What a feature of an overlay or a record layer says of itself (its properties, as OpenLayers' getProperties gives
    // them): its label (label, name or title) and description (description or desc), as plain text.
    map.featureDetails = (properties) => {
        const given = properties && typeof properties === "object" ? properties : {};
        const pick = (...names) => { for (const name of names) { const value = map.plainText(given[name], name === "description" || name === "desc" ? 2000 : 200); if (value) return value; } return ""; };
        return { label: pick("label", "name", "title"), description: pick("description", "desc") };
    };
    // The feature nearest a coordinate of the map (its projection's units) among candidates [{source, layer}] of vector
    // sources, within tolerance (the same units: a resolution times the pixels): {feature, layer, distance}, or null. A
    // point or a line by its distance; a polygon that holds the coordinate counts as at the tolerance, so a point inside it
    // and near wins. The keyboard's selection of the feature at the map's center (Enter), with no pixel read.
    map.nearestFeature = (candidates, coordinate, tolerance) => {
        let best = null;
        const consider = (feature, layer, distance) => {
            if (distance <= tolerance && (!best || distance < best.distance)) best = { feature, layer, distance };
        };
        (candidates || []).forEach(({ source, layer }) => {
            if (!source || !source.getClosestFeatureToCoordinate) return;
            const closest = source.getClosestFeatureToCoordinate(coordinate);
            const geometry = closest && closest.getGeometry();
            if (geometry) {
                const at = geometry.getClosestPoint(coordinate);
                consider(closest, layer, Math.hypot(at[0] - coordinate[0], at[1] - coordinate[1]));
            }
            (source.getFeaturesAtCoordinate ? source.getFeaturesAtCoordinate(coordinate) : []).forEach((feature) => {
                const type = feature.getGeometry() && feature.getGeometry().getType();
                if (type === "Polygon" || type === "MultiPolygon") consider(feature, layer, tolerance);
            });
        });
        return best;
    };

    // ---- editing helpers ----

    // A feature collection as an edit saves it: each feature with a geometry of GeoJSON's simple types, its positions in
    // WGS84 (a longitude brought back into -180 to 180, seven decimals), its id (unique: one used already or missing is made
    // from its number) and only its label and description, as plain text. What the server reads again by the same rules.
    const GEOMETRY_TYPES = ["Point", "MultiPoint", "LineString", "MultiLineString", "Polygon", "MultiPolygon"];
    const round7 = (value) => Math.round(value * 1e7) / 1e7;
    const editedPositions = (value, depth) => {
        if (depth === 0) {
            if (!Array.isArray(value) || value.length < 2) return null;
            const lon = num(value[0]), lat = num(value[1]);
            return lon === null || lat === null ? null : [round7(map.wrapLongitude(lon)), round7(lat)];
        }
        if (!Array.isArray(value)) return null;
        const out = value.map((item) => editedPositions(item, depth - 1));
        return out.some((item) => item === null) ? null : out;
    };
    map.editedCollection = (collection) => {
        const given = collection && Array.isArray(collection.features) ? collection.features : [];
        const depth = { Point: 0, MultiPoint: 1, LineString: 1, MultiLineString: 2, Polygon: 2, MultiPolygon: 3 };
        const used = new Set();
        const features = [];
        given.forEach((feature, index) => {
            const geometry = feature && feature.geometry;
            if (!geometry || !GEOMETRY_TYPES.includes(geometry.type)) return;
            const coordinates = editedPositions(geometry.coordinates, depth[geometry.type]);
            if (!coordinates) return;
            let id = text(feature.id);
            if (!id || used.has(id) || !/^[A-Za-z0-9_.:-]{1,64}$/.test(id)) {
                let n = index + 1;
                while (used.has("f" + n)) n++;
                id = "f" + n;
            }
            used.add(id);
            const details = map.featureDetails(feature.properties);
            const properties = {};
            if (details.label) properties.label = details.label;
            if (details.description) properties.description = details.description;
            features.push({ type: "Feature", id, geometry: { type: geometry.type, coordinates }, properties });
        });
        return { type: "FeatureCollection", features };
    };
    // The geometry of a shape made from the keyboard, vertex by vertex ([lon, lat] each, in order): a point of its one vertex,
    // a line of two or more, an area of three or more (its ring closed); null when it has too few
    map.sketchGeometry = (type, vertices) => {
        const points = (vertices || []).filter((vertex) => Array.isArray(vertex) && vertex.length >= 2);
        if (type === "Point") return points.length ? { type: "Point", coordinates: points[points.length - 1] } : null;
        if (type === "LineString") return points.length >= 2 ? { type: "LineString", coordinates: points } : null;
        if (type === "Polygon") return points.length >= 3 ? { type: "Polygon", coordinates: [points.concat([points[0].slice()])] } : null;
        return null;
    };
    // Whether an overlay of Browse's layers is one its user may edit: their own, drawn whole from its GeoJSON (not
    // simplified into tiles), not another user's shared with them
    map.canEditOverlay = (layer) => !!(layer && layer.overlayId && layer.isOwn === true && !layer.tilesStatus && layer.kind === "geojson");

    // A choice put in the latitude and longitude fields of the page's form named pickForm (the shell's registered m-forms, by
    // their name or id), six decimals; false when there is no such form.
    map.fillPickForm = (pickForm, point) => {
        const name = text(pickForm);
        const forms = name && moqui.screenForms ? moqui.screenForms : [];
        const form = forms.find((vm) => vm && vm.$el && vm.$el.getAttribute && (vm.$el.getAttribute("name") === name || vm.$el.getAttribute("id") === name) && vm.fields);
        if (!form) return false;
        form.fields.latitude = Number(point.lat).toFixed(6);
        form.fields.longitude = Number(point.lon).toFixed(6);
        return true;
    };

    // ---- live location (plans/Location-Sharing.md §3.3) ----

    // The shares a map shows live: the effective configuration's locationShares (the viewer's own active shares and those
    // shared with them, which the server checked against each share's current audience), at most LIVE_MAX. Each, checked:
    // {shareId, role (owner or viewer), label (plain text, shown as text), ownerName, thruDate (epoch millis), approximate,
    // keepTrail, latest {lon, lat, accuracy, observedDate} or null, seenAt (this page's clock when that position was taken:
    // latest.seenAt as the parent stamped it at its read, else the read's ageSeconds before now, or a push's arrival), trail [[lon, lat]] (at most LIVE_TRAIL_MAX, the latest)}.
    const LIVE_MAX = 100;
    const LIVE_TRAIL_MAX = 500;
    // a share with no report for this long is drawn stale: its owner's page closed, or the device lost its fix
    map.LIVE_STALE_SECONDS = 300;
    const livePoint = (value) => {
        if (!value || typeof value !== "object") return null;
        const lon = num(first(value, "longitude", "lon")), lat = num(first(value, "latitude", "lat"));
        if (!lonOk(lon) || !latOk(lat)) return null;
        const accuracy = num(value.accuracy);
        return { lon, lat, accuracy: accuracy !== null && accuracy >= 0 && accuracy <= 1e7 ? accuracy : null, observedDate: num(value.observedDate) };
    };
    const liveTrail = (trail) => (Array.isArray(trail) ? trail : []).map((pair) => (Array.isArray(pair) ? [num(pair[0]), num(pair[1])] : [null, null]))
        .filter((pair) => lonOk(pair[0]) && latOk(pair[1])).slice(-LIVE_TRAIL_MAX);
    map.normalizeLiveShare = (row, now) => {
        if (!row || typeof row !== "object") return null;
        const shareId = shortText(row.shareId, 60);
        if (!shareId) return null;
        const latest = livePoint(row.latest);
        const age = latest ? num(row.latest.ageSeconds) : null;
        // the page's clock at its read, when the parent stamped it (Maps › Browse does, once a read): a configuration
        // normalized again later keeps it, so a share that stopped reporting goes stale
        const stamped = latest ? num(row.latest.seenAt) : null;
        return { shareId, role: row.role === "owner" ? "owner" : "viewer", label: map.plainText(row.label, 100) || map.plainText(row.ownerName, 100) || L("Shared location"),
            ownerName: map.plainText(row.ownerName, 100), thruDate: num(row.thruDate), approximate: row.approximate === true, keepTrail: row.keepTrail === true,
            latest, seenAt: !latest ? null : stamped !== null ? stamped : now - Math.max(0, age === null ? 0 : age) * 1000, trail: row.keepTrail === true ? liveTrail(row.trail) : [] };
    };
    map.normalizeLiveShares = (list, now) => {
        const at = now === undefined ? Date.now() : now;
        const seen = new Set();
        return (Array.isArray(list) ? list : []).map((row) => map.normalizeLiveShare(row, at)).filter((share) => share && !seen.has(share.shareId) && seen.add(share.shareId))
            .slice(0, LIVE_MAX);
    };
    // The seconds since a share's position was taken, null with none; stale past LIVE_STALE_SECONDS.
    map.liveAgeSeconds = (share, now) => (share && share.seenAt !== null && share.seenAt !== undefined ? Math.max(0, Math.round((now - share.seenAt) / 1000)) : null);
    map.liveStale = (share, now) => { const age = map.liveAgeSeconds(share, now); return age !== null && age > map.LIVE_STALE_SECONDS; };
    // The age as words ("just now", "4 min ago"); an approximate share's by the minute, as its time is kept.
    map.liveAgeText = (seconds) => (seconds === null || seconds === undefined ? L("No position yet") : seconds < 60 ? L("just now")
        : seconds < 3600 ? Math.floor(seconds / 60) + " " + L("min ago") : seconds < 86400 ? Math.floor(seconds / 3600) + " " + L("h ago")
        : Math.floor(seconds / 86400) + " " + L("d ago"));
    // The message of a push on a share's topic (locationShare:<shareId>: the NotificationMessage's message, an object or its
    // JSON), or null.
    map.liveMessage = (jsonObj) => {
        let message = jsonObj && typeof jsonObj === "object" ? jsonObj.message : null;
        if (typeof message === "string") { try { message = JSON.parse(message); } catch (err) { message = null; } }
        return message && typeof message === "object" && text(message.shareId) ? message : null;
    };
    // A push applied to the shares (LocationShares.js push): position moves the share's marker, its accuracy and its trail
    // (when kept) and makes it fresh; changed takes a new end; ended, and revoked for a viewer (who is out of its audience),
    // takes it off (a revoke the owner is told of ends an entry, not the share). {shares (a new list when changed),
    // changed, removed (the shareId taken off, or null)}.
    map.applyLiveMessage = (shares, message, now) => {
        const index = message ? shares.findIndex((share) => share.shareId === text(message.shareId)) : -1;
        if (index < 0) return { shares, changed: false, removed: null };
        const share = shares[index];
        const event = text(message.event);
        if (event === "ended" || (event === "revoked" && share.role === "viewer"))
            return { shares: shares.filter((one, at) => at !== index), changed: true, removed: share.shareId };
        let next = null;
        if (event === "position") {
            const latest = livePoint(message);
            if (!latest) return { shares, changed: false, removed: null };
            const trail = share.keepTrail ? share.trail.concat([[latest.lon, latest.lat]]).slice(-LIVE_TRAIL_MAX) : share.trail;
            next = Object.assign({}, share, { latest, seenAt: now, trail, approximate: message.approximate === true ? true : share.approximate });
        } else if (event === "changed" && num(message.thruDate) !== null) next = Object.assign({}, share, { thruDate: num(message.thruDate) });
        if (!next) return { shares, changed: false, removed: null };
        return { shares: shares.map((one, at) => (at === index ? next : one)), changed: true, removed: null };
    };
    // A configuration's shares over those held: a share the read no longer lists is gone; one taken off by a push (gone)
    // stays off; the held position is kept when the read's is the same one (its time when it was first seen stays, so a
    // configuration read or normalized again never makes an old position fresh) or older (a push came since).
    const samePosition = (a, b) => !!a && !!b && a.lon === b.lon && a.lat === b.lat && a.observedDate === b.observedDate;
    map.mergeLiveShares = (held, given, gone) => given.filter((share) => !(gone && gone.has(share.shareId))).map((share) => {
        const before = held.find((one) => one.shareId === share.shareId);
        const keep = before && before.seenAt !== null && (samePosition(before.latest, share.latest) || share.seenAt === null || before.seenAt > share.seenAt);
        return keep ? Object.assign({}, share, { latest: before.latest, seenAt: before.seenAt, trail: before.trail.length > share.trail.length ? before.trail : share.trail }) : share;
    });
    // The ring of a position's accuracy: count points metres around it, in WGS84 degrees (closed), for a polygon in any
    // view's projection.
    map.accuracyRing = (lon, lat, metres, count) => {
        const steps = count || 48;
        const north = metres / 111320;
        const east = metres / (111320 * Math.max(Math.cos((lat * Math.PI) / 180), 1e-6));
        const ring = [];
        for (let i = 0; i <= steps; i++) {
            const angle = (2 * Math.PI * (i % steps)) / steps;
            ring.push([lon + east * Math.cos(angle), Math.max(-90, Math.min(90, lat + north * Math.sin(angle)))]);
        }
        return ring;
    };

    // ---- the component ----

    const h = (...args) => root.Vue.h(...args);
    // A rejected promise the library catches, for transformRequest to refuse a URL: marked handled so a sprite image's
    // refusal is not an unhandled rejection.
    const refusal = (url) => {
        const promise = Promise.reject(new Error("The map does not ask " + url + ": it is not a host of the profile"));
        promise.catch(() => {});
        return promise;
    };

    const mapView = {
        name: "mMapView",
        props: { id: String, config: [Object, String], configUrl: String, height: String, pick: Boolean, libraryError: String },
        emits: ["pick", "select", "view-change", "edit-change", "live-change"],
        data() {
            return { effective: null, loadError: "", fallback: "", drawn: false, layerStatus: [], readout: "", picked: null, pickText: "", pickError: "",
                locating: false, located: "", basemapId: "", selectedId: "", announced: "", focused: false, editing: false, projectionNote: "",
                liveShares: [], liveNow: Date.now() };
        },
        computed: {
            local() {
                const config = map.normalizeConfig(this.config);
                if (this.pick) config.pick = true;
                return config;
            },
            shown() { return this.effective || this.local; },
            points() { return this.shown.points; },
            notice() { return this.shown.profile.notice; },
            basemaps() { return this.shown.profile.basemaps; },
            failedLayers() { return this.layerStatus.filter((status) => status.state === "failed"); }
        },
        watch: {
            // a parent's render makes its bound object again: only a configuration that changed draws the map again, and one
            // whose live shares alone changed (a parent's read of them again) changes the live layer without drawing it again
            config(value, before) {
                if (JSON.stringify(value) === JSON.stringify(before)) return;
                const others = (config) => {
                    let given = config;
                    if (typeof given === "string") { try { given = JSON.parse(given); } catch (err) { given = {}; } }
                    return JSON.stringify(Object.assign({}, given || {}, { locationShares: null }));
                };
                if (others(value) === others(before)) this.setLiveShares(this.local.locationShares);
                else this.load();
            }
        },
        mounted() { this.load(); },
        beforeUnmount() {
            this.editSnapshot = null;
            this.editor = null;
            if (this.decoderListener) decoderListeners.delete(this.decoderListener);
            this.stopLive();
            this.teardown();
        },
        methods: {
            pageOrigin() { return root.location.origin; },
            // a package's path in package mode (plans/Maps-Offline.md §2.5), else ""
            packageBase() { return this.shown.packageBase || ""; },
            webfonts() { return this.packageBase() ? map.packageFonts(this.pageOrigin(), this.packageBase()) : map.webfonts(this.pageOrigin()); },
            load() {
                const local = this.local;
                // an answer for an earlier configuration, come after a later one, is not drawn
                const seq = this.loadSeq = (this.loadSeq || 0) + 1;
                if (!this.configUrl) { this.start(local); return; }
                moqui.request(this.configUrl, { params: map.configParameters(local) }).then((result) => {
                    if (seq !== this.loadSeq) return;
                    this.loadError = "";
                    this.start(map.mergeConfig(map.normalizeConfig(result.json), local));
                }, (error) => {
                    if (seq !== this.loadSeq) return;
                    // the places given are still drawn, on the outline
                    this.loadError = L("The map settings did not load") + ": " + (error && error.status ? "HTTP " + error.status : (error && error.error && error.error.message) || "");
                    this.start(local);
                });
            },
            start(config) {
                this.teardown();
                this.effective = config;
                // the live shares are listed, and followed on their topics, whether or not the map can be drawn
                this.setLiveShares(config.locationShares);
                this.basemapId = config.profile.basemap && config.profile.basemaps.some((b) => b.id === config.profile.basemap) ? config.profile.basemap :
                    (config.profile.basemaps[0] || {}).id || "";
                const O = root.MoquiOl;
                if (this.libraryError || !O) { this.fallback = L("The map could not be drawn here") + (this.libraryError ? ": " + this.libraryError : ""); return; }
                this.fallback = "";
                this.projectionNote = "";
                // the raster and projection part only for a map that needs it (a COG layer, a registered projection)
                const seq = this.startSeq = (this.startSeq || 0) + 1;
                const ready = map.needsRaster(config) ? map.loadRaster().then((R) => { map.registerProjections(O, R, config.projections); return R; }, (err) => {
                    console.warn("m-map: the raster and projection part did not load", err);
                    this.rasterError = L("The raster and projection part did not load") + ": " + (err && err.message || err);
                    return null;
                }) : Promise.resolve(null);
                ready.then((R) => {
                    if (seq !== this.startSeq) return;
                    this.raster = R;
                    // a view in a projection that could not be registered is drawn in Web Mercator, and says so
                    let drawn = config;
                    if (!O.getProjection(config.projection)) {
                        drawn = Object.assign({}, config, { projection: "EPSG:3857" });
                        this.effective = drawn;
                        this.projectionNote = L("The map's projection could not be loaded, so it is drawn in Web Mercator") + ": " + config.projection;
                    }
                    this.$nextTick(() => {
                        try { this.build(O, drawn); } catch (err) {
                            console.error("m-map: the map could not be drawn", err);
                            this.teardown();
                            this.fallback = L("The map could not be drawn here") + ": " + (err && err.message || err);
                        }
                    });
                });
            },
            teardown() {
                // the map drawn again keeps what is being edited (build starts it again)
                if (this.editor) { this.editSnapshot = this.editSnapshotOf(); this.editor = null; }
                if (this.olMap) { this.olMap.setTarget(null); this.olMap.dispose(); this.olMap = null; }
                this.liveSource = null;
                this.drawn = false;
                this.layerStatus = [];
            },
            projection() { return this.shown.projection; },
            toMap(lon, lat) { return root.MoquiOl.fromLonLat([lon, lat], this.projection()); },
            toLonLat(coordinate) {
                const [lon, lat] = root.MoquiOl.toLonLat(coordinate, this.projection());
                return { lon: map.wrapLongitude(lon), lat };
            },
            status(id, label, state, error) {
                const others = this.layerStatus.filter((status) => status.id !== id);
                this.layerStatus = others.concat([{ id, label: label || id, state, error: error || "" }]);
            },
            build(O, config) {
                const canvas = this.$refs.canvas;
                if (!canvas) return;
                this.fallback = "";
                this.origins = map.allowedOrigins(config, this.pageOrigin());
                // a registered projection's view starts at the middle of its area of use and stays within its extent
                const registered = config.projections.find((row) => row.code === config.projection);
                const middle = registered && registered.bounds ? [(registered.bounds[0] + registered.bounds[2]) / 2, (registered.bounds[1] + registered.bounds[3]) / 2] : [0, 20];
                const viewOptions = { projection: config.projection, center: this.toMap(middle[0], middle[1]), zoom: 2, maxZoom: 22 };
                if (registered && registered.extent) Object.assign(viewOptions, { extent: registered.extent, showFullExtent: true });
                const view = new O.View(viewOptions);
                const olMap = new O.Map({
                    target: canvas, view, keyboardEventTarget: canvas,
                    controls: O.defaultControls({ rotate: false, attributionOptions: { collapsible: true } }).extend([new O.ScaleLine({ units: map.scaleUnits(config) })])
                });
                this.olMap = olMap;
                this.basemapGroup = new O.LayerGroup();
                this.overlayGroup = new O.LayerGroup();
                olMap.addLayer(this.basemapGroup);
                olMap.addLayer(this.overlayGroup);
                this.markerSource = new O.VectorSource();
                olMap.addLayer(new O.VectorLayer({ source: this.markerSource, style: (feature) => this.markerStyle(O, feature), zIndex: 10 }));
                // the live layer, over the places (plans/Location-Sharing.md §3.3)
                this.liveSource = new O.VectorSource();
                olMap.addLayer(new O.VectorLayer({ source: this.liveSource, style: (feature) => this.liveStyle(O, feature), zIndex: 11 }));
                this.drawLive(O);
                this.drawBasemap(O);
                config.layers.forEach((layer) => this.drawLayer(O, layer));
                this.drawPlaces(O, config);
                this.fitView(O, config);
                const kept = this.keptView;
                this.keptView = null;
                if (kept && kept.projection === config.projection) {
                    view.setCenter(kept.center);
                    if (kept.zoom !== undefined) view.setZoom(kept.zoom);
                }
                olMap.on("pointermove", (event) => {
                    if (event.dragging) return;
                    const at = this.toLonLat(event.coordinate);
                    this.readout = map.formatCoordinates(at.lon, at.lat);
                });
                if (this.editSnapshot) {
                    const snapshot = this.editSnapshot;
                    this.editSnapshot = null;
                    this.startEditor(O, snapshot);
                }
                olMap.on("singleclick", (event) => {
                    // while editing, a click is the editor's (its Select and Draw)
                    if (this.editor) return;
                    // a place, a record's or a record layer's feature, or a feature of an overlay layer (not the basemap's)
                    const hit = olMap.forEachFeatureAtPixel(event.pixel, (found, layer) => (found.get("place") || found.get("recordFeature") ||
                        found.get("liveMarker") || (layer && layer.get("overlayLayerId")) ? { feature: found, layer } : null), { hitTolerance: 4 });
                    if (hit && hit.feature.get("liveMarker")) this.selectLive(hit.feature.get("shareId"));
                    else if (hit && hit.feature.get("place")) this.select(hit.feature.get("place"));
                    else if (hit) this.selectFeature(hit.feature, hit.layer, this.toLonLat(event.coordinate));
                    else if (this.shown.pick) this.choose(this.toLonLat(event.coordinate));
                });
                olMap.on("moveend", () => {
                    const center = view.getCenter();
                    if (!center) return;
                    const at = this.toLonLat(center);
                    // a keyboard user hears where the map is after a move; the pointer's readout is not announced
                    if (root.document && root.document.activeElement === canvas) {
                        this.readout = map.formatCoordinates(at.lon, at.lat);
                        this.announced = L("Map center") + ": " + this.readout;
                    }
                    const extent = O.transformExtent(view.calculateExtent(olMap.getSize() || [1, 1]), config.projection, "EPSG:4326");
                    this.$emit("view-change", { center: [at.lon, at.lat], zoom: view.getZoom(), extent: map.wrapExtent(extent) });
                });
                this.drawn = true;
            },
            // ---- the basemap ----
            drawBasemap(O) {
                const group = this.basemapGroup;
                if (!group) return;
                group.getLayers().clear();
                const basemap = this.basemaps.find((candidate) => candidate.id === this.basemapId) || this.basemaps[0];
                if (!basemap) return;
                this.layerStatus = this.layerStatus.filter((status) => !this.basemaps.some((candidate) => candidate.id === status.id));
                const fail = (error) => {
                    const message = error && error.message || String(error || "");
                    console.warn("m-map: basemap " + basemap.id + " did not load", error);
                    this.status(basemap.id, basemap.label, "failed", message);
                };
                this.status(basemap.id, basemap.label, "loading");
                const attributions = map.attributionHtml(basemap.attribution, basemap.attributionUrls);
                try {
                    if (!map.drawableIn(basemap, this.projection())) throw new Error(L("It is drawn only in Web Mercator (EPSG:3857), not in") + " " + this.projection());
                    if (basemap.kind === "outline") this.outlineLayer(O, group, basemap, attributions);
                    else if (basemap.kind === "xyz") this.xyzLayer(O, group, basemap, attributions);
                    else if (basemap.kind === "style") this.styleLayer(O, group, basemap, attributions).catch(fail);
                    else if (basemap.kind === "pmtiles") this.pmtilesLayer(O, group, basemap, attributions).catch(fail);
                    else if (basemap.kind === "wms") this.wmsLayer(O, group, basemap, attributions);
                    else if (basemap.kind === "wmts") this.wmtsLayer(O, group, basemap, attributions);
                    else if (basemap.kind === "cog") this.cogLayer(O, group, basemap, attributions);
                    else throw new Error(L("Not a kind of basemap this map draws") + ": " + basemap.kind);
                } catch (err) { fail(err); }
            },
            checkUrl(url, what) {
                if (this.packageBase() && !map.urlAllowed(url, this.origins, this.pageOrigin(), this.packageBase()))
                    throw new Error(what + " " + (url || "(none)") + " " + L("is not a file of this offline map"));
                if (!map.urlAllowed(url, this.origins, this.pageOrigin())) throw new Error(what + " " + (url || "(none)") + " " + L("is not a host of the map's profile"));
            },
            watchSource(source, entry, kind) {
                let loaded = false;
                source.on(kind === "vector" ? "featuresloadend" : "tileloadend", () => {
                    if (!loaded) { loaded = true; this.status(entry.id, entry.label, "ready"); }
                });
                let errors = 0;
                source.on(kind === "vector" ? "featuresloaderror" : "tileloaderror", () => {
                    errors++;
                    if (kind === "vector" || (!loaded && errors >= 4)) this.status(entry.id, entry.label, "failed", L("Its data did not load"));
                });
            },
            outlineLayer(O, group, basemap, attributions) {
                this.checkUrl(basemap.url, L("The outline"));
                const dark = root.document && root.document.body && root.document.body.classList.contains("body--dark");
                // read here, not by url: each ring is made continuous across the antimeridian before it is projected
                const url = new URL(basemap.url, this.pageOrigin() + "/").href;
                // a package's by range, past 16 MiB as the app's scheme answers it
                const read = () => (this.packageBase() ? map.readPackageFile(url).then((body) => JSON.parse(body)) :
                    root.fetch(url, { credentials: "same-origin" }).then((response) => {
                        if (!response.ok) throw new Error("HTTP " + response.status);
                        return response.json();
                    }));
                const source = new O.VectorSource({ attributions, overlaps: false, loader: (extent, resolution, projection, success, failure) => {
                    read().then((topology) => {
                        const features = new O.TopoJSON({ layers: ["countries"] }).readFeatures(topology);
                        features.forEach((feature) => {
                            const geometry = feature.getGeometry();
                            if (!geometry || !geometry.getCoordinates) return;
                            const coordinates = geometry.getType() === "Polygon" ? geometry.getCoordinates().map(map.unwrapRing) :
                                geometry.getType() === "MultiPolygon" ? geometry.getCoordinates().map((polygon) => polygon.map(map.unwrapRing)) : null;
                            if (coordinates) geometry.setCoordinates(coordinates);
                            geometry.transform("EPSG:4326", projection);
                        });
                        source.addFeatures(features);
                        success(features);
                    }).catch((err) => { console.warn("m-map: the outline did not load", err); failure(); });
                } });
                this.watchSource(source, basemap, "vector");
                group.getLayers().push(new O.VectorLayer({ source, style: new O.Style({
                    fill: new O.Fill({ color: dark ? "#2f3b46" : "#e8ece6" }), stroke: new O.Stroke({ color: dark ? "#6b7a88" : "#9aa59a", width: 0.75 }) }) }));
            },
            xyzLayer(O, group, entry, attributions) {
                this.checkUrl(entry.url, L("The tiles"));
                const source = new O.XYZ({ url: entry.url, attributions, crossOrigin: "anonymous", maxZoom: entry.maxZoom === null ? 19 : entry.maxZoom });
                this.watchSource(source, entry, "tile");
                group.getLayers().push(new O.TileLayer({ source }));
            },
            // Vector tiles at a Z/X/Y template of this server (LocalMap's tile transition: a user's overlay simplified on the
            // server), drawn and selected as an overlay file's features are; past its maxZoom the last zoom's tiles are drawn larger
            mvtLayer(O, group, entry, attributions) {
                this.checkUrl(entry.url, L("The tiles"));
                if (!map.sameOrigin(entry.url, this.pageOrigin())) throw new Error(L("Vector tiles must be of this server") + ": " + entry.url);
                // full features (not render features), so a selected point has its coordinates and a highlight redraws it
                const source = new O.VectorTileSource({ format: new O.MVT({ featureClass: O.Feature }), url: entry.url, attributions,
                    maxZoom: entry.maxZoom === null ? 14 : entry.maxZoom });
                this.watchSource(source, entry, "tile");
                const layer = new O.VectorTileLayer({ source, declutter: true, style: (feature) => this.featureStyle(O, feature) });
                layer.set("overlayLayerId", entry.id);
                layer.set("overlayLabel", entry.label);
                group.getLayers().push(layer);
            },
            ogcCrossOrigin(url, ogc) { return map.ogcCrossOrigin(url, ogc, this.pageOrigin()); },
            ogcOf(entry, service) {
                if (!entry.ogc || entry.ogc.service !== service) throw new Error(L("The layer's service settings are missing") + ": " + entry.id);
                return entry.ogc;
            },
            // A WMS layer: tiles of the view (TileWMS), or one image of it (ImageWMS)
            wmsLayer(O, group, entry, attributions) {
                const ogc = this.ogcOf(entry, "wms");
                this.checkUrl(entry.url, L("The service"));
                const options = { url: entry.url, params: Object.assign({}, ogc.params), projection: ogc.projection, attributions, crossOrigin: this.ogcCrossOrigin(entry.url, ogc) };
                if (ogc.singleImage) {
                    const source = new O.ImageWMS(Object.assign(options, { ratio: 1 }));
                    let loaded = false;
                    source.on("imageloadend", () => { if (!loaded) { loaded = true; this.status(entry.id, entry.label, "ready"); } });
                    source.on("imageloaderror", () => this.status(entry.id, entry.label, "failed", L("Its data did not load")));
                    group.getLayers().push(new O.ImageLayer({ source }));
                    return;
                }
                const source = new O.TileWMS(options);
                this.watchSource(source, entry, "tile");
                group.getLayers().push(new O.TileLayer({ source }));
            },
            // A WMTS layer: its tile grid from the matrix set the server recorded, never the capabilities fetched here
            wmtsLayer(O, group, entry, attributions) {
                const ogc = this.ogcOf(entry, "wmts");
                this.checkUrl(entry.url, L("The tiles"));
                if (!O.getProjection(ogc.projection)) throw new Error(L("The layer's projection is not one this map draws") + ": " + ogc.projection);
                const matrixSet = JSON.parse(JSON.stringify(ogc.matrixSet));
                const source = new O.WMTS({ url: entry.url, layer: ogc.layer, style: ogc.style, format: ogc.format, matrixSet: matrixSet.Identifier,
                    tileGrid: O.createFromCapabilitiesMatrixSet(matrixSet), projection: ogc.projection, requestEncoding: ogc.requestEncoding,
                    dimensions: ogc.dimensions, attributions, crossOrigin: this.ogcCrossOrigin(entry.url, ogc), wrapX: true });
                this.watchSource(source, entry, "tile");
                group.getLayers().push(new O.TileLayer({ source }));
            },
            // A WFS feature type read as GeoJSON for the view's box, at most maxFeatures at a time: a load that would make more
            // starts the layer again with the view's features only, cut to maxFeatures
            wfsLayer(O, group, entry, attributions) {
                const ogc = this.ogcOf(entry, "wfs");
                this.checkUrl(entry.url, L("The service"));
                const dataProjection = map.wfsDataProjection(ogc.srsName);
                const format = new O.GeoJSON({ dataProjection });
                const credentials = map.sameOrigin(entry.url, this.pageOrigin()) ? "same-origin" : "omit";
                const source = new O.VectorSource({ format, attributions, strategy: O.bboxStrategy,
                    loader: (extent, resolution, projection, success, failure) => {
                        const box = O.transformExtent(extent, projection, dataProjection);
                        root.fetch(map.wfsUrl(new URL(entry.url, this.pageOrigin() + "/").href, ogc, box), { credentials, headers: { Accept: "application/json" } })
                            .then((response) => { if (!response.ok) throw new Error("HTTP " + response.status); return response.json(); })
                            .then((json) => {
                                const features = format.readFeatures(json, { dataProjection, featureProjection: projection }).slice(0, ogc.maxFeatures);
                                if (source.getFeatures().length + features.length > ogc.maxFeatures) source.clear(true);
                                source.addFeatures(features);
                                success(features);
                            }).catch((err) => { console.warn("m-map: the features of " + entry.id + " did not load", err); failure(); });
                    } });
                this.watchSource(source, entry, "vector");
                const vector = new O.VectorLayer({ source, style: (feature) => this.featureStyle(O, feature) });
                vector.set("overlayLayerId", entry.id);
                vector.set("overlayLabel", entry.label);
                group.getLayers().push(vector);
            },
            // A Cloud Optimized GeoTIFF (P4c), read by range with geotiff.js from the raster part and drawn with WebGL: its
            // bands as the settings choose them, stretched and colored by map.rasterStyle. This server's (the map-source
            // transition) is read with the session and a 409 loads the configuration again; a provider's with no credentials
            // of the page, and only when its test saw CORS.
            cogLayer(O, group, entry, attributions) {
                const R = this.raster;
                if (!entry.cog) throw new Error(L("The layer's raster settings are missing") + ": " + entry.id);
                if (!R) throw new Error(this.rasterError || L("The raster and projection part is not loaded"));
                if (!root.WebGLRenderingContext) throw new Error(L("A GeoTIFF is drawn with WebGL, which this browser does not have"));
                this.checkUrl(entry.url, L("The GeoTIFF"));
                const page = this.pageOrigin();
                const same = map.sameOrigin(entry.url, page);
                if (!same && entry.cog.cors === false) throw new Error(L("Its provider does not let a browser read it (CORS)"));
                const style = entry.cog.style;
                const olMap = this.olMap;
                const info = { url: new URL(entry.url, page + "/").href, bands: map.rasterBands(style),
                    loader: (asked, headers, signal) => root.fetch(asked, { headers, signal, credentials: same ? "same-origin" : "omit" }).then((response) => {
                        if (response.status === 409 && this.olMap === olMap) this.sourceChanged(entry);
                        return response;
                    }) };
                if (style.nodata !== undefined) info.nodata = style.nodata;
                // a decoder worker that does not start fails this layer (its tiles would wait for it)
                if (!this.decoderListener) {
                    this.decoderListener = () => {
                        if (!this.olMap) return;
                        this.shown.profile.basemaps.concat(this.shown.layers).filter((one) => one.kind === "cog").forEach((one) =>
                            this.status(one.id, one.label, "failed", L("Its decoder did not start: the page's policy may refuse its worker (worker-src 'self')")));
                    };
                    decoderListeners.add(this.decoderListener);
                }
                const source = new R.GeoTIFF({ sources: [info], normalize: false, projection: entry.cog.projection });
                if (attributions) source.setAttributions(attributions);
                source.on("change", () => {
                    if (source.getState() !== "error") return;
                    const error = source.getError();
                    this.status(entry.id, entry.label, "failed", (error && error.message) || L("Its data did not load"));
                });
                this.watchSource(source, entry, "tile");
                group.getLayers().push(new R.WebGLTileLayer({ source, style: map.rasterStyle(style) }));
            },
            // the request filter of what ol-mapbox-style fetches (TileJSON, tiles, sprites): only the profile's hosts
            // A TileJSON ("Source") is fetched here and answered with its attribution escaped, since the attribution control sets it
            // as HTML before m-map can replace it.
            transformRequest() {
                const origins = this.origins, page = this.pageOrigin(), base = this.packageBase();
                return (url, resourceType) => {
                    if (!map.urlAllowed(url, origins, page, base)) return refusal(url);
                    if (resourceType !== "Source") return undefined;
                    return root.fetch(url, { credentials: map.sameOrigin(url, page) ? "same-origin" : "omit", headers: { Accept: "application/json" } })
                        .then((response) => (!response.ok ? response : response.json().then((tileJson) =>
                            new root.Response(JSON.stringify(map.sanitizeSource(tileJson)), { status: 200, headers: { "Content-Type": "application/json" } }))));
                };
            },
            // A style, checked and made safe (map.localStyle). bindVector: its first vector source is a PMTiles archive m-map binds
            // itself, so its url (pmtiles://) is not checked; answers {style, sourceId}.
            loadStyle(url, bindVector) {
                this.checkUrl(url, L("The style"));
                const sameOrigin = map.sameOrigin(url, this.pageOrigin());
                return root.fetch(new URL(url, this.pageOrigin() + "/").href, { credentials: sameOrigin ? "same-origin" : "omit", headers: { Accept: "application/json" } })
                    .then((response) => {
                        if (!response.ok) throw new Error(L("The style did not load") + ": HTTP " + response.status);
                        return response.json();
                    }).then((given) => {
                        // a package's style: its sprite the package's own (its fonts are, through webfonts)
                        const base = this.packageBase();
                        const style = base ? map.packageStyle(given, this.pageOrigin(), base) : given;
                        const sources = style && style.sources && typeof style.sources === "object" ? style.sources : {};
                        const sourceId = bindVector ? Object.keys(sources).find((id) => (sources[id] || {}).type === "vector") : null;
                        const refused = map.refusedStyleUrls(style, url, this.origins, this.pageOrigin(), sourceId ? [sourceId] : null, base);
                        if (refused.length) throw new Error(L("The style names a host outside the map's profile") + ": " + refused[0]);
                        return { style: map.localStyle(style, this.webfonts()), sourceId };
                    });
            },
            styleLayer(O, group, basemap, attributions) {
                return this.loadStyle(basemap.url).then(({ style }) => O.apply(group, style, { styleUrl: new URL(basemap.url, this.pageOrigin() + "/").href,
                    webfonts: this.webfonts(), transformRequest: this.transformRequest() })).then(() => {
                    this.attribute(group, attributions);
                    // a source the library puts in later is given the same attribution
                    group.getLayersArray().forEach((layer) => layer.on("change:source", () => this.attribute(group, attributions)));
                    this.status(basemap.id, basemap.label, "ready");
                });
            },
            pmtilesLayer(O, group, entry, attributions) {
                if (!map.sameOrigin(entry.url, this.pageOrigin())) return Promise.reject(new Error(L("A PMTiles archive must be of this server") + ": " + entry.url));
                // a package's archive, read by range from the package (206 slices of the file; a package's file never
                // changes, so no 409)
                try { if (this.packageBase()) this.checkUrl(entry.url, L("The archive")); } catch (err) { return Promise.reject(err); }
                const url = new URL(entry.url, this.pageOrigin() + "/").href;
                // the archive's header first: one that cannot be read by range fails here, by name, not tile by tile in silence.
                // Its URL names a registered revision (the map-source transition): a 409 says the archive has another now,
                // and the map loads its configuration again rather than draw ranges of two revisions.
                // A read of a layer no longer drawn (another basemap chosen, the map drawn again) says nothing of this map.
                const olMap = this.olMap;
                let drawn = null;
                const live = () => this.olMap === olMap && (group === this.basemapGroup || this.overlayGroup.getLayers().getArray().includes(group)) &&
                    (!drawn || group.getLayers().getArray().includes(drawn));
                const archive = new O.PMTiles(map.pinnedSource(O, url, () => { if (live()) this.sourceChanged(entry); }));
                return archive.getHeader().then(() => {
                    if (entry.raster) {
                        const source = new O.PMTilesRasterSource({ url: archive.source, attributions });
                        this.watchSource(source, entry, "tile");
                        drawn = new O.TileLayer({ source });
                        group.getLayers().push(drawn);
                        return null;
                    }
                    const layer = drawn = new O.VectorTileLayer({ declutter: true, source: new O.PMTilesVectorSource({ url: archive.source, attributions }) });
                    this.watchSource(layer.getSource(), entry, "tile");
                    group.getLayers().push(layer);
                    return entry.styleUrl ? this.pmtilesStyle(O, layer, entry) : null;
                });
            },
            pmtilesStyle(O, layer, entry) {
                return this.loadStyle(entry.styleUrl, true).then(({ style, sourceId }) => {
                    // the style's vector source is the archive: ol-mapbox-style does not know pmtiles:// URLs
                    // styleUrl: the style's relative sprite resolves against it, on this server
                    return O.applyStyle(layer, style, { source: sourceId, updateSource: false, styleUrl: new URL(entry.styleUrl, this.pageOrigin() + "/").href,
                        webfonts: this.webfonts(), transformRequest: this.transformRequest() })
                        .then(() => O.applyBackground(layer, style));
                }).then(() => this.status(entry.id, entry.label, "ready"));
            },
            // A registered archive answered 409: it has another revision now (registered again) or its file changed. The
            // configuration is asked again, once in a while, so the map draws the new revision whole; with no config-url,
            // or a second change within a minute, the layer is named as failed.
            sourceChanged(entry) {
                const now = Date.now();
                if (this.configUrl && (!this.changedAt || now - this.changedAt > 60000)) {
                    this.changedAt = now;
                    console.info("m-map: the archive of " + entry.id + " changed; loading the map settings again");
                    // the map drawn again stays where the user had it
                    const view = this.olMap && this.olMap.getView();
                    this.keptView = view && view.getCenter() ? { center: view.getCenter(), zoom: view.getZoom(), projection: this.projection() } : null;
                    this.load();
                } else this.status(entry.id, entry.label, "failed", L("Its archive changed: reload the page"));
            },
            // the profile's attribution on every source ol-mapbox-style made, in place of the style's own markup
            attribute(group, attributions) {
                group.getLayersArray().forEach((layer) => {
                    const source = layer.getSource && layer.getSource();
                    if (!source || !source.setAttributions) return;
                    if (attributions) source.setAttributions(attributions);
                    else {
                        let parts = null;
                        try {
                            const own = source.getAttributions && source.getAttributions();
                            parts = own ? own({ viewState: {}, extent: [0, 0, 0, 0] }) : null;
                        } catch (err) { parts = null; }
                        source.setAttributions(map.escapeHtml(map.attributionText([].concat(parts || []).join(" "))));
                    }
                });
            },
            // ---- overlay layers ----
            drawLayer(O, layer) {
                const fail = (error) => {
                    console.warn("m-map: layer " + layer.id + " did not load", error);
                    this.status(layer.id, layer.label, "failed", error && error.message || String(error || ""));
                };
                if (layer.requested) { this.status(layer.id, layer.label, "failed", L("This layer is not available")); return; }
                this.status(layer.id, layer.label, "loading");
                const group = new O.LayerGroup({ visible: layer.visible });
                group.set("layerId", layer.id);
                this.overlayGroup.getLayers().push(group);
                const attributions = map.attributionHtml(layer.attribution);
                try {
                    if (!map.drawableIn(layer, this.projection())) throw new Error(L("It is drawn only in Web Mercator (EPSG:3857), not in") + " " + this.projection());
                    if (layer.kind === "xyz") this.xyzLayer(O, group, layer, attributions);
                    else if (layer.kind === "pmtiles") this.pmtilesLayer(O, group, layer, attributions).catch(fail);
                    else if (layer.kind === "wms") this.wmsLayer(O, group, layer, attributions);
                    else if (layer.kind === "wmts") this.wmtsLayer(O, group, layer, attributions);
                    else if (layer.kind === "wfs") this.wfsLayer(O, group, layer, attributions);
                    else if (layer.kind === "mvt") this.mvtLayer(O, group, layer, attributions);
                    else if (layer.kind === "cog") this.cogLayer(O, group, layer, attributions);
                    else if (FILE_KINDS.includes(layer.kind)) {
                        this.checkUrl(layer.url, L("The layer"));
                        const format = layer.kind === "gpx" ? new O.GPX() : layer.kind === "kml" ? new O.KML(map.kmlOptions()) :
                            layer.kind === "topojson" ? new O.TopoJSON() : new O.GeoJSON();
                        // a package's file read here, by range past 16 MiB as the app's scheme answers it; else by url
                        const url = new URL(layer.url, this.pageOrigin() + "/").href;
                        const source = this.packageBase() ? new O.VectorSource({ format, attributions, loader: (extent, resolution, projection, success, failure) => {
                            map.readPackageFile(url).then((body) => {
                                const features = format.readFeatures(body, { featureProjection: projection });
                                source.addFeatures(features);
                                success(features);
                            }).catch((err) => { console.warn("m-map: the layer " + layer.id + " did not load", err); failure(); });
                        } }) : new O.VectorSource({ url: layer.url, format, attributions });
                        this.watchSource(source, layer, "vector");
                        // its features are selected (a click, or Enter at the center): the layer says whose they are
                        const vector = new O.VectorLayer({ source, style: (feature) => this.featureStyle(O, feature) });
                        vector.set("overlayLayerId", layer.id);
                        vector.set("overlayLabel", layer.label);
                        group.getLayers().push(vector);
                    } else throw new Error(L("Not a kind of layer this map draws") + ": " + (layer.kind || "(none)"));
                } catch (err) { fail(err); }
            },
            retry(id) {
                const O = root.MoquiOl;
                if (!O || !this.olMap) return;
                if (this.basemaps.some((basemap) => basemap.id === id)) { this.drawBasemap(O); return; }
                const layer = this.shown.layers.find((candidate) => candidate.id === id);
                if (!layer) return;
                const groups = this.overlayGroup.getLayers();
                groups.getArray().filter((group) => group.get("layerId") === id).forEach((group) => groups.remove(group));
                this.drawLayer(O, layer);
            },
            chooseBasemap(id) {
                this.basemapId = id;
                if (root.MoquiOl && this.olMap) this.drawBasemap(root.MoquiOl);
            },
            // ---- places ----
            featureStyle(O, feature) {
                const label = map.plainText(feature.get("label") || feature.get("name"), 200);
                // the selected feature in the selected place's color
                const [solid, light] = feature === this.highlighted ? ["rgba(239,108,0,0.95)", "rgba(239,108,0,0.2)"] : ["rgba(25,118,210,0.85)", "rgba(25,118,210,0.15)"];
                return new O.Style({
                    image: new O.CircleStyle({ radius: feature === this.highlighted ? 7 : 5, fill: new O.Fill({ color: solid }), stroke: new O.Stroke({ color: "#fff", width: 1.5 }) }),
                    stroke: new O.Stroke({ color: solid, width: 2 }), fill: new O.Fill({ color: light }),
                    text: label ? this.labelText(O, label) : undefined
                });
            },
            // the feature drawn as selected (none: a place's marker is the selection), each changed so it is drawn again
            highlight(feature) {
                const before = this.highlighted;
                this.highlighted = feature || null;
                if (before && before !== feature) before.changed();
                if (feature) feature.changed();
            },
            labelText(O, label) {
                // drawn on the canvas, so no markup in a label is read; long labels are cut
                const shown = label.length > 40 ? label.slice(0, 39) + "…" : label;
                return new O.Text({ text: shown, offsetY: -14, font: "12px sans-serif", fill: new O.Fill({ color: "#1d1d1d" }), stroke: new O.Stroke({ color: "#fff", width: 3 }) });
            },
            markerStyle(O, feature) {
                const kind = feature.get("kind");
                const color = kind === "picked" ? "#c62828" : kind === "located" ? "#2e7d32" : kind === "found" ? "#6a1b9a" :
                    feature.get("place") && feature.get("place").id === this.selectedId ? "#ef6c00" : "#1976d2";
                const label = kind === "place" ? text(feature.get("place").label) : kind === "found" ? text(feature.get("label")) : "";
                return new O.Style({
                    image: new O.CircleStyle({ radius: kind === "place" ? 7 : 6, fill: new O.Fill({ color }), stroke: new O.Stroke({ color: "#fff", width: 2 }) }),
                    text: label ? this.labelText(O, label) : undefined
                });
            },
            drawPlaces(O, config) {
                const source = this.markerSource;
                source.clear();
                config.points.forEach((place) => {
                    const feature = new O.Feature({ geometry: new O.Point(this.toMap(place.lon, place.lat)), kind: "place" });
                    feature.set("place", place);
                    source.addFeature(feature);
                });
                if (config.features) {
                    try {
                        const features = new O.GeoJSON().readFeatures(config.features, { dataProjection: "EPSG:4326", featureProjection: config.projection });
                        features.forEach((feature) => feature.set("recordFeature", true));
                        const layer = new O.VectorLayer({ source: new O.VectorSource({ features }), style: (feature) => this.featureStyle(O, feature) });
                        layer.set("recordFeatures", true);
                        this.overlayGroup.getLayers().push(layer);
                    } catch (err) { this.status("features", L("Features"), "failed", err && err.message || String(err)); }
                }
            },
            fitView(O, config) {
                const view = this.olMap.getView();
                const size = this.olMap.getSize();
                if (config.extent) {
                    const [west, south, east, north] = config.extent;
                    // across the antimeridian the east edge goes past 180, which the wrapped world draws
                    const extent = O.transformExtent([west, south, east < west ? east + 360 : east, north], "EPSG:4326", config.projection);
                    view.fit(extent, { size, padding: [24, 24, 24, 24], maxZoom: 16 });
                } else if (config.center) {
                    view.setCenter(this.toMap(config.center[0], config.center[1]));
                    view.setZoom(config.zoom === null ? 10 : config.zoom);
                } else if (config.points.length > 1) {
                    const extent = O.boundingExtent(config.points.map((place) => this.toMap(place.lon, place.lat)));
                    view.fit(extent, { size, padding: [48, 48, 48, 48], maxZoom: config.zoom === null ? 14 : config.zoom });
                } else if (config.points.length === 1) {
                    view.setCenter(this.toMap(config.points[0].lon, config.points[0].lat));
                    view.setZoom(config.zoom === null ? 12 : config.zoom);
                } else if (config.zoom !== null) view.setZoom(config.zoom);
            },
            goTo(place) {
                this.selectedId = place.id;
                if (this.olMap) this.olMap.getView().animate({ center: this.toMap(place.lon, place.lat), zoom: Math.max(this.olMap.getView().getZoom() || 0, 10), duration: 250 });
                if (this.markerSource) this.markerSource.changed();
            },
            select(place) {
                this.selectedId = place.id;
                this.highlight(null);
                if (this.markerSource) this.markerSource.changed();
                this.$emit("select", { id: place.id, lon: place.lon, lat: place.lat, label: place.label });
            },
            // A feature chosen on the map: a record's or a record layer's (config.features: its id, label and layer), or an
            // overlay layer's (kind overlay: its label and description as plain text, and the layer's id and label). A point
            // is at its own coordinates, a line or an area at the point clicked (or the map's center, from the keyboard).
            selectFeature(feature, layer, at) {
                const overlayId = layer && layer.get ? text(layer.get("overlayLayerId")) : "";
                const geometry = feature.getGeometry && feature.getGeometry();
                const point = geometry && geometry.getType() === "Point" ? this.toLonLat(geometry.getCoordinates()) : at;
                const details = map.featureDetails(feature.getProperties ? feature.getProperties() : {});
                this.selectedId = "";
                this.highlight(feature);
                if (this.markerSource) this.markerSource.changed();
                this.announced = L("Selected") + ": " + (details.label || L("Feature")) + ", " + map.formatCoordinates(point.lon, point.lat);
                const selected = { id: String(feature.getId() === undefined ? "" : feature.getId()), lon: point.lon, lat: point.lat, label: details.label };
                if (overlayId) Object.assign(selected, { kind: "overlay", layerId: overlayId, layerLabel: text(layer.get("overlayLabel")), description: details.description });
                else selected.layerId = text(feature.get("layerId"));
                this.$emit("select", selected);
            },
            // the vector sources a keyboard selection reads: the shown overlay layers' and the record's features
            selectable() {
                const out = [];
                const visit = (layer, shown) => {
                    const visible = shown && layer.getVisible();
                    if (layer.getLayers) { layer.getLayers().getArray().forEach((child) => visit(child, visible)); return; }
                    if (visible && (layer.get("overlayLayerId") || layer.get("recordFeatures")) && layer.getSource()) out.push({ source: layer.getSource(), layer });
                };
                if (this.overlayGroup) visit(this.overlayGroup, true);
                return out;
            },
            // Enter on the map (with no choice to make): the feature at its center, within 12 pixels, is selected
            selectAtCenter() {
                const view = this.olMap && this.olMap.getView();
                const center = view && view.getCenter();
                if (!center) return;
                const hit = map.nearestFeature(this.selectable(), center, (view.getResolution() || 1) * 12);
                if (hit) this.selectFeature(hit.feature, hit.layer, this.toLonLat(center));
                else this.announced = L("No feature at the map's center");
            },
            copyPlace(place) {
                const coordinates = map.formatCoordinates(place.lon, place.lat);
                const clipboard = root.navigator && root.navigator.clipboard;
                const done = () => { this.announced = L("Copied") + ": " + coordinates; };
                if (!clipboard || !clipboard.writeText) { done(); return; }
                clipboard.writeText(coordinates).then(done, (err) => console.warn("m-map: the coordinates were not copied", err));
            },
            // ---- pick, locate ----
            choose(point) {
                this.picked = point;
                this.pickError = "";
                this.mark("picked", point);
                this.$emit("pick", { lon: point.lon, lat: point.lat });
                if (this.$el && this.$el.dispatchEvent && root.CustomEvent) {
                    this.$el.dispatchEvent(new root.CustomEvent("m-map-pick", { bubbles: true, detail: { lon: point.lon, lat: point.lat } }));
                }
                map.fillPickForm(this.shown.pickForm, point);
            },
            chooseCenter() {
                const center = this.olMap && this.olMap.getView().getCenter();
                if (center) this.choose(this.toLonLat(center));
            },
            chooseTyped() {
                const point = map.parseLatLon(this.pickText);
                if (!point) { this.pickError = L("Enter a latitude and a longitude, such as 41.8781, -87.6298"); return; }
                this.choose(point);
                if (this.olMap) this.olMap.getView().animate({ center: this.toMap(point.lon, point.lat), duration: 250 });
            },
            mark(kind, point, label) {
                const O = root.MoquiOl;
                if (!O || !this.markerSource) return;
                this.markerSource.getFeatures().filter((feature) => feature.get("kind") === kind).forEach((feature) => this.markerSource.removeFeature(feature));
                this.markerSource.addFeature(new O.Feature({ geometry: new O.Point(this.toMap(point.lon, point.lat)), kind, label: label || "" }));
            },
            // A place found by a page's own search (the offline screen's, plans/Maps-Offline.md §2.5): marked and shown,
            // its label drawn on the canvas as text; nothing is kept or sent
            showPlace(place) {
                const lon = num(place && place.lon), lat = num(place && place.lat);
                if (!lonOk(lon) || !latOk(lat)) return;
                const label = map.plainText(place.label, 200);
                this.mark("found", { lon, lat }, label);
                this.announced = (label || L("Place")) + ": " + map.formatCoordinates(lon, lat);
                const view = this.olMap && this.olMap.getView();
                if (view) view.animate({ center: this.toMap(lon, lat), zoom: Math.max(view.getZoom() || 0, num(place.zoom) || 15), duration: 250 });
            },
            // Locate me: the browser's position once, when asked; shown, never kept or sent
            locate() {
                const geo = root.navigator && root.navigator.geolocation;
                if (!geo) { this.located = L("This browser cannot give its location"); return; }
                this.locating = true;
                geo.getCurrentPosition((position) => {
                    this.locating = false;
                    const point = { lon: position.coords.longitude, lat: position.coords.latitude };
                    const accuracy = Math.round(position.coords.accuracy || 0);
                    this.located = L("Your location") + ": " + map.formatCoordinates(point.lon, point.lat) + (accuracy ? " (± " + accuracy + " m)" : "");
                    this.mark("located", point);
                    if (this.olMap) this.olMap.getView().animate({ center: this.toMap(point.lon, point.lat), zoom: Math.max(this.olMap.getView().getZoom() || 0, 14), duration: 250 });
                }, (error) => {
                    this.locating = false;
                    this.located = L("Your location is not available") + (error && error.message ? ": " + error.message : "");
                }, { enableHighAccuracy: false, timeout: 15000, maximumAge: 60000 });
            },
            // ---- live location (plans/Location-Sharing.md §3.3) ----
            // The page's one notification socket (the shell root's notificationClient), or null without one.
            liveClient() {
                const shell = this.$root && this.$root.notificationClient ? this.$root : moqui.webrootVue;
                return shell && shell.notificationClient ? shell.notificationClient : null;
            },
            // The shares of a configuration over those held (map.mergeLiveShares), each followed on its topic while shown.
            setLiveShares(given) {
                this.liveGone = this.liveGone || new Set();
                this.liveShares = map.mergeLiveShares(this.liveShares, Array.isArray(given) ? given : [], this.liveGone);
                this.liveChanged();
            },
            // After a change of the shares: the topics followed, the layer drawn, the ticking of ages, and live-change emitted.
            liveChanged(removed) {
                this.syncLiveTopics();
                if (root.MoquiOl && this.liveSource) this.drawLive(root.MoquiOl);
                if (this.liveShares.length && !this.liveTimer) {
                    // the ages and the stale style move on with the clock, twice a minute
                    this.liveTimer = setInterval(() => { this.liveNow = Date.now(); if (this.liveSource) this.liveSource.changed(); }, 30000);
                } else if (!this.liveShares.length && this.liveTimer) { clearInterval(this.liveTimer); this.liveTimer = null; }
                this.liveNow = Date.now();
                this.$emit("live-change", { shares: this.liveShares.map((share) => Object.assign({}, share)), removed: removed ? [removed] : [] });
            },
            // Subscribes to locationShare:<shareId> of each share shown, and unsubscribes from a share no longer shown.
            syncLiveTopics() {
                const client = this.liveClient();
                const held = this.liveTopics = this.liveTopics || {};
                const wanted = new Set(this.liveShares.map((share) => "locationShare:" + share.shareId));
                Object.keys(held).filter((topic) => !wanted.has(topic)).forEach((topic) => { held[topic](); delete held[topic]; });
                if (!client) return;
                wanted.forEach((topic) => {
                    if (held[topic]) return;
                    const listener = (jsonObj) => this.onLivePush(topic.slice("locationShare:".length), jsonObj);
                    const stop = client.registerListener(topic, listener);
                    held[topic] = typeof stop === "function" ? stop : () => client.unregisterListener(topic, listener);
                });
            },
            // A push on a share's topic: its message must name that share
            onLivePush(shareId, jsonObj) {
                const message = map.liveMessage(jsonObj);
                if (!message || text(message.shareId) !== shareId) return;
                const applied = map.applyLiveMessage(this.liveShares, message, Date.now());
                if (!applied.changed) return;
                if (applied.removed) this.liveGone = (this.liveGone || new Set()).add(applied.removed);
                this.liveShares = applied.shares;
                this.liveChanged(applied.removed);
            },
            stopLive() {
                if (this.liveTimer) { clearInterval(this.liveTimer); this.liveTimer = null; }
                const held = this.liveTopics || {};
                Object.keys(held).forEach((topic) => { held[topic](); delete held[topic]; });
            },
            // The layer's features: each share with a position as a marker (its label and age drawn as canvas text), its
            // accuracy as a ring and its kept trail as a line, all in the map's projection.
            drawLive(O) {
                const source = this.liveSource;
                if (!source) return;
                source.clear();
                const format = new O.GeoJSON();
                const geometry = (value) => format.readGeometry(value, { dataProjection: "EPSG:4326", featureProjection: this.projection() });
                this.liveShares.forEach((share) => {
                    if (!share.latest) return;
                    const { lon, lat, accuracy } = share.latest;
                    if (share.trail.length > 1) source.addFeature(new O.Feature({ geometry: geometry({ type: "LineString", coordinates: share.trail }), shareId: share.shareId, liveKind: "trail" }));
                    if (accuracy) source.addFeature(new O.Feature({ geometry: geometry({ type: "Polygon", coordinates: [map.accuracyRing(lon, lat, accuracy)] }),
                        shareId: share.shareId, liveKind: "accuracy" }));
                    const marker = new O.Feature({ geometry: new O.Point(this.toMap(lon, lat)), shareId: share.shareId, liveKind: "marker", liveMarker: true });
                    marker.setId("live:" + share.shareId);
                    source.addFeature(marker);
                });
            },
            liveShare(shareId) { return this.liveShares.find((share) => share.shareId === shareId) || null; },
            // A share's look: teal for one shared with the viewer, green for the viewer's own, grey and dashed once stale; the
            // label and the age are drawn on the canvas, so no markup in a label is read.
            liveStyle(O, feature) {
                const share = this.liveShare(feature.get("shareId"));
                if (!share) return null;
                const stale = map.liveStale(share, Date.now());
                const rgb = stale ? "117,117,117" : share.role === "owner" ? "46,125,50" : "0,121,107";
                const kind = feature.get("liveKind");
                if (kind === "trail") return new O.Style({ stroke: new O.Stroke({ color: "rgba(" + rgb + ",0.7)", width: 3, lineDash: stale ? [6, 6] : undefined }) });
                if (kind === "accuracy") return new O.Style({ fill: new O.Fill({ color: "rgba(" + rgb + ",0.12)" }), stroke: new O.Stroke({ color: "rgba(" + rgb + ",0.5)", width: 1 }) });
                const label = share.label + " · " + map.liveAgeText(map.liveAgeSeconds(share, Date.now()));
                return new O.Style({
                    image: new O.CircleStyle({ radius: 8, fill: new O.Fill({ color: "rgba(" + rgb + "," + (stale ? "0.6" : "1") + ")" }),
                        stroke: new O.Stroke({ color: "#fff", width: 2, lineDash: stale ? [3, 3] : undefined }) }),
                    text: this.labelText(O, label)
                });
            },
            goToLive(share) {
                if (!share.latest) return;
                this.selectedId = "";
                if (this.olMap) this.olMap.getView().animate({ center: this.toMap(share.latest.lon, share.latest.lat), zoom: Math.max(this.olMap.getView().getZoom() || 0, 14), duration: 250 });
            },
            // A share chosen on the map or in the list: emitted as select with kind liveShare and its shareId.
            selectLive(shareId) {
                const share = this.liveShare(shareId);
                if (!share || !share.latest) return;
                this.highlight(null);
                this.announced = L("Selected") + ": " + share.label + ", " + map.liveAgeText(map.liveAgeSeconds(share, Date.now()));
                this.$emit("select", { id: "live:" + share.shareId, kind: "liveShare", shareId: share.shareId, lon: share.latest.lon, lat: share.latest.lat, label: share.label });
            },
            onCanvasKey(event) {
                // only on the map itself: Enter on one of OpenLayers' controls inside it is that control's. Enter chooses the
                // center on a map that asks for a choice, and selects the feature there on any other; while editing, Enter adds
                // a vertex at the center when a shape is drawn, Delete removes the selected feature and Escape lets go of the
                // shape being drawn, or else of the selection
                if (event.target !== event.currentTarget) return;
                if (this.editor && (event.key === "Delete" || event.key === "Escape")) {
                    event.preventDefault();
                    this.editCommand(event.key === "Delete" ? "delete" : "escape");
                    return;
                }
                if (event.key !== "Enter") return;
                event.preventDefault();
                if (this.editor) this.editCommand("enter");
                else if (this.shown.pick) this.chooseCenter();
                else this.selectAtCenter();
            },
            onCanvasFocus(event) {
                // the crosshair shows for a keyboard's focus, not for a click's
                let visible = true;
                try { visible = !event || !event.target || !event.target.matches || event.target.matches(":focus-visible"); } catch (err) { visible = true; }
                this.focused = visible;
                const center = this.olMap && this.olMap.getView().getCenter();
                if (center) { const at = this.toLonLat(center); this.readout = map.formatCoordinates(at.lon, at.lat); }
            },
            // ---- editing ----
            // One command of the editing (the header above has them); answers what the command gives (collection: the
            // FeatureCollection; others nothing)
            editCommand(name, value) {
                const O = root.MoquiOl;
                if (name === "start") {
                    if (!O || !this.olMap) { this.editSnapshot = { layerId: text(value.layerId), collection: value.collection, pointsOnly: !!value.pointsOnly, dirty: false }; return null; }
                    this.startEditor(O, { layerId: text(value.layerId), collection: value.collection, pointsOnly: !!value.pointsOnly, dirty: false });
                    return null;
                }
                const editor = this.editor;
                if (name === "collection") return editor ? this.editedOf(editor) : this.editSnapshot ? map.editedCollection(this.editSnapshot.collection) : null;
                if (name === "stop") { this.stopEditor(); return null; }
                if (!editor) return null;
                if (name === "draw") this.setDrawType(value);
                else if (name === "enter") this.editEnter();
                else if (name === "finish") this.finishSketch();
                else if (name === "escape") {
                    if (editor.sketch.length) { editor.sketch = []; this.drawSketch(); }
                    else editor.select.getFeatures().clear();
                } else if (name === "delete") {
                    const selected = editor.select.getFeatures().getArray().slice();
                    editor.select.getFeatures().clear();
                    selected.forEach((feature) => { if (editor.source.hasFeature(feature)) editor.source.removeFeature(feature); });
                    this.announced = selected.length ? L("Feature deleted") : L("No feature is selected");
                } else if (name === "details") {
                    const feature = editor.select.getFeatures().item(0);
                    if (feature) {
                        feature.set("label", map.plainText(value && value.label, 200));
                        feature.set("description", map.plainText(value && value.description, 2000));
                    }
                } else if (name === "saved") editor.dirty = false;
                this.editChanged();
                return null;
            },
            editSnapshotOf() {
                const editor = this.editor;
                return { layerId: editor.layerId, collection: this.editedOf(editor), pointsOnly: editor.pointsOnly, dirty: editor.dirty, drawType: editor.drawType };
            },
            editedOf(editor) {
                const written = new root.MoquiOl.GeoJSON().writeFeaturesObject(editor.source.getFeatures(), { dataProjection: "EPSG:4326", featureProjection: this.projection() });
                return map.editedCollection(written);
            },
            startEditor(O, options) {
                this.stopEditor(true);
                const olMap = this.olMap;
                const format = new O.GeoJSON();
                const features = format.readFeatures(map.editedCollection(options.collection), { dataProjection: "EPSG:4326", featureProjection: this.projection() });
                const source = new O.VectorSource({ features });
                const sketchSource = new O.VectorSource();
                const layer = new O.VectorLayer({ source, style: (feature) => this.editStyle(O, feature), zIndex: 20 });
                const sketchLayer = new O.VectorLayer({ source: sketchSource, style: () => this.editStyle(O, null), zIndex: 21 });
                // the overlay drawn from its file is hidden while its features are edited here
                const hidden = this.overlayGroup.getLayers().getArray().filter((group) => group.get("layerId") === options.layerId && group.getVisible());
                hidden.forEach((group) => group.setVisible(false));
                const select = new O.Select({ layers: [layer], style: (feature) => this.editStyle(O, feature, true), hitTolerance: 4 });
                // only the selected feature's vertices move, so a drag of the map does not change a feature by mistake
                const modify = new O.Modify({ features: select.getFeatures() });
                olMap.addLayer(layer);
                olMap.addLayer(sketchLayer);
                olMap.addInteraction(select);
                olMap.addInteraction(modify);
                const editor = { layerId: options.layerId, pointsOnly: options.pointsOnly, source, sketchSource, layer, sketchLayer, select, modify, snap: null,
                    draw: null, drawType: "", sketch: [], hidden, dirty: !!options.dirty, made: 0 };
                this.editor = editor;
                this.editing = true;
                this.addSnap(O);
                const changed = () => { editor.dirty = true; this.editChanged(); };
                source.on("addfeature", changed);
                source.on("removefeature", changed);
                source.on("changefeature", changed);
                select.on("select", () => this.editChanged());
                if (options.drawType) this.setDrawType(options.drawType);
                this.editChanged();
            },
            // Snap last, so it sees what Draw and Modify do (OpenLayers' order); without the bundle's Snap, none
            addSnap(O) {
                const editor = this.editor;
                if (!editor || !O.Snap) return;
                if (editor.snap) this.olMap.removeInteraction(editor.snap);
                editor.snap = new O.Snap({ source: editor.source });
                this.olMap.addInteraction(editor.snap);
            },
            setDrawType(type) {
                const O = root.MoquiOl;
                const editor = this.editor;
                const allowed = editor.pointsOnly ? ["Point"] : ["Point", "LineString", "Polygon"];
                const wanted = allowed.includes(type) ? type : "";
                if (editor.draw) { this.olMap.removeInteraction(editor.draw); editor.draw = null; }
                editor.drawType = wanted;
                editor.sketch = [];
                this.drawSketch();
                // a click draws while a shape is drawn, and selects otherwise
                editor.select.setActive(!wanted);
                if (!wanted) return;
                editor.draw = new O.Draw({ source: editor.source, type: wanted });
                editor.draw.on("drawend", (event) => this.madeFeature(event.feature));
                this.olMap.addInteraction(editor.draw);
                this.addSnap(O);
            },
            // a feature drawn (by Draw or from the keyboard): its id, no text yet, and selected so its label can be given
            madeFeature(feature) {
                const editor = this.editor;
                editor.made += 1;
                feature.setId("n" + Date.now().toString(36) + editor.made);
                feature.set("label", "");
                const selected = editor.select.getFeatures();
                selected.clear();
                selected.push(feature);
                this.announced = L("Feature added: give it a name, or draw another");
                this.$nextTick(() => this.editChanged());
            },
            // Enter while editing: a vertex at the center for the shape being drawn (a point is made at once), else the feature
            // at the center selected
            editEnter() {
                const editor = this.editor;
                const view = this.olMap && this.olMap.getView();
                const center = view && view.getCenter();
                if (!center) return;
                if (!editor.drawType) {
                    const hit = map.nearestFeature([{ source: editor.source, layer: editor.layer }], center, (view.getResolution() || 1) * 12);
                    const selected = editor.select.getFeatures();
                    selected.clear();
                    if (hit) { selected.push(hit.feature); this.announced = L("Selected") + ": " + (map.plainText(hit.feature.get("label"), 200) || L("Feature")); }
                    else this.announced = L("No feature at the map's center");
                    return;
                }
                const at = this.toLonLat(center);
                editor.sketch.push([at.lon, at.lat]);
                if (editor.drawType === "Point") { this.finishSketch(); return; }
                this.announced = L("Vertex added") + " (" + editor.sketch.length + "): " + map.formatCoordinates(at.lon, at.lat);
                this.drawSketch();
            },
            finishSketch() {
                const O = root.MoquiOl;
                const editor = this.editor;
                const geometry = map.sketchGeometry(editor.drawType, editor.sketch);
                if (!geometry) { this.announced = L("A line needs two vertices and an area three"); return; }
                const feature = new O.GeoJSON().readFeature({ type: "Feature", geometry, properties: {} }, { dataProjection: "EPSG:4326", featureProjection: this.projection() });
                editor.sketch = [];
                this.drawSketch();
                editor.source.addFeature(feature);
                this.madeFeature(feature);
            },
            // the shape being drawn from the keyboard, shown as its vertices joined
            drawSketch() {
                const editor = this.editor;
                editor.sketchSource.clear();
                if (!editor.sketch.length) return;
                const geometry = editor.sketch.length === 1 ? { type: "Point", coordinates: editor.sketch[0] } : { type: "LineString", coordinates: editor.sketch };
                editor.sketchSource.addFeature(new root.MoquiOl.GeoJSON().readFeature({ type: "Feature", geometry, properties: {} },
                    { dataProjection: "EPSG:4326", featureProjection: this.projection() }));
            },
            // keep: the overlay stays hidden (another editing starts at once)
            stopEditor(keep) {
                const editor = this.editor;
                this.editSnapshot = null;
                if (!editor) return;
                this.editor = null;
                this.editing = false;
                if (this.olMap) {
                    [editor.select, editor.modify, editor.draw, editor.snap].forEach((interaction) => { if (interaction) this.olMap.removeInteraction(interaction); });
                    this.olMap.removeLayer(editor.layer);
                    this.olMap.removeLayer(editor.sketchLayer);
                }
                if (!keep) editor.hidden.forEach((group) => group.setVisible(true));
                this.$emit("edit-change", null);
            },
            editChanged() {
                const editor = this.editor;
                if (!editor) return;
                const feature = editor.select.getFeatures().item(0);
                const selected = feature ? Object.assign({ id: String(feature.getId() === undefined ? "" : feature.getId()) }, map.featureDetails(feature.getProperties())) : null;
                this.$emit("edit-change", { count: editor.source.getFeatures().length, dirty: editor.dirty, selected, sketch: editor.sketch.length, drawType: editor.drawType });
            },
            editStyle(O, feature, selected) {
                const [solid, light] = selected ? ["rgba(239,108,0,0.95)", "rgba(239,108,0,0.25)"] : ["rgba(46,125,50,0.9)", "rgba(46,125,50,0.15)"];
                const label = feature ? map.plainText(feature.get("label"), 200) : "";
                return new O.Style({
                    image: new O.CircleStyle({ radius: selected ? 7 : 5, fill: new O.Fill({ color: solid }), stroke: new O.Stroke({ color: "#fff", width: 1.5 }) }),
                    stroke: new O.Stroke({ color: solid, width: selected ? 3 : 2, lineDash: feature ? undefined : [6, 4] }), fill: new O.Fill({ color: light }),
                    text: label ? this.labelText(O, label) : undefined
                });
            },
            // ---- render ----
            placeItem(place) {
                const details = [place.precision, place.source].filter((part) => part).join(" · ");
                const body = [h("span", { class: "m-map-place-label" }, place.label || L("Place")), " ",
                    h("span", { class: "m-map-place-coordinates" }, map.formatCoordinates(place.lon, place.lat)),
                    details ? h("span", { class: "m-map-place-details" }, details) : null];
                const copy = h("button", { type: "button", class: "m-map-place-copy", "aria-label": L("Copy the coordinates of") + " " + (place.label || L("Place")),
                    onClick: () => this.copyPlace(place) }, L("Copy"));
                return h("li", { key: place.id, class: { "m-map-place": true, "m-map-place-selected": place.id === this.selectedId } },
                    (this.drawn ? [h("button", { type: "button", class: "m-map-place-button", title: L("Show on the map"),
                        onClick: () => { this.goTo(place); this.select(place); } }, body)] : body).concat([" ", copy]));
            },
            // A live share in the list under the map: its label, the age of its position (stale marked), where it is and whether
            // it is approximate, all as text; on a drawn map a button that goes to it
            liveItem(share) {
                const age = map.liveAgeSeconds(share, this.liveNow);
                const stale = map.liveStale(share, this.liveNow);
                const body = [h("span", { class: "m-map-live-label" }, share.label), " ",
                    h("span", { class: "m-map-live-age" }, map.liveAgeText(age) + (stale ? " (" + L("stale") + ")" : "")),
                    share.latest ? [" ", h("span", { class: "m-map-live-coordinates" }, map.formatCoordinates(share.latest.lon, share.latest.lat))] : null,
                    share.approximate ? [" ", h("span", { class: "m-map-live-details" }, L("approximate, about 1 km"))] : null];
                return h("li", { key: share.shareId, class: { "m-map-live-share": true, "m-map-live-stale": stale }, "data-share-id": share.shareId },
                    this.drawn && share.latest ? [h("button", { type: "button", class: "m-map-live-button", title: L("Show on the map"),
                        onClick: () => { this.goToLive(share); this.selectLive(share.shareId); } }, body)] : body);
            },
            toolbar() {
                const button = root.Vue.resolveComponent("q-btn");
                const items = [h(button, { dense: true, flat: true, noCaps: true, size: "sm", icon: "my_location", label: L("Locate me"), loading: this.locating,
                    class: "m-map-locate", onClick: () => this.locate() })];
                if (this.shown.pick) items.push(h(button, { dense: true, flat: true, noCaps: true, size: "sm", icon: "place", label: L("Choose the center"),
                    class: "m-map-choose-center", onClick: () => this.chooseCenter() }));
                if (this.basemaps.length > 1) {
                    items.push(h("select", { class: "m-map-basemap", "aria-label": L("Basemap"), value: this.basemapId,
                        onChange: (event) => this.chooseBasemap(event.target.value) }, this.basemaps.map((basemap) => h("option", { value: basemap.id }, basemap.label))));
                }
                return h("div", { class: "m-map-toolbar row items-center no-wrap" }, items);
            },
            pickForm() {
                return h("form", { class: "m-map-pick row items-center q-gutter-sm", onSubmit: (event) => { event.preventDefault(); this.chooseTyped(); } }, [
                    h("label", { class: "m-map-pick-label" }, [L("Latitude, longitude") + " ",
                        h("input", { class: "m-map-pick-input", value: this.pickText, inputmode: "decimal", autocomplete: "off", placeholder: "41.8781, -87.6298",
                            onInput: (event) => { this.pickText = event.target.value; } })]),
                    h("button", { type: "submit", class: "m-map-pick-use" }, L("Use")),
                    this.pickError ? h("span", { class: "m-map-pick-error text-negative", role: "alert" }, this.pickError) : null
                ]);
            }
        },
        render() {
            const shown = this.shown;
            const noticeSlot = this.$slots.notice ? this.$slots.notice() : null;
            const notice = noticeSlot || (this.notice ? [this.notice.text, this.notice.href ?
                [" ", h("a", { href: this.notice.href, class: "m-map-notice-link" }, this.notice.linkText || L("Configuration"))] : null] : null);
            const canvasLabel = L("Map") + ": " + L("arrow keys move it, + and - zoom") + ", " + (this.editing
                ? L("Enter adds a vertex at the center while drawing, else selects the feature there; Delete removes it")
                : shown.pick ? L("Enter chooses the center") : L("Enter selects the feature at the center"));
            return h("div", { class: "m-map", id: this.id }, [
                notice ? h("div", { class: "m-map-notice", role: "note" }, notice) : null,
                this.loadError ? h("div", { class: "m-map-status m-map-status-failed", role: "status" }, this.loadError) : null,
                this.projectionNote ? h("div", { class: "m-map-status m-map-projection-note", role: "note" }, this.projectionNote) : null,
                this.fallback ? h("div", { class: "m-map-fallback", role: "status" }, [this.fallback, ". ", L("The places are listed below.")]) :
                    h("div", { class: "m-map-frame", style: { height: this.height || "360px" } }, [
                        h("div", { ref: "canvas", class: "m-map-canvas", tabindex: 0, role: "application", "aria-label": canvasLabel,
                            onKeydown: this.onCanvasKey, onFocus: this.onCanvasFocus, onBlur: () => { this.focused = false; } }),
                        // the center a choice or a keyboard selection takes
                        shown.pick || this.focused || this.editing ? h("div", { class: "m-map-crosshair", "aria-hidden": "true" }) : null,
                        this.toolbar(),
                        this.readout ? h("div", { class: "m-map-readout", "aria-hidden": "true" }, this.readout) : null,
                        h("div", { class: "m-map-announce", "aria-live": "polite" }, this.announced)
                    ]),
                this.failedLayers.length ? h("ul", { class: "m-map-status m-map-layer-status" }, this.failedLayers.map((status) => h("li", { key: status.id }, [
                    h("span", { class: "text-negative" }, L("Not shown") + ": " + status.label + (status.error ? " (" + status.error + ")" : "")), " ",
                    h("button", { type: "button", class: "m-map-retry", onClick: () => this.retry(status.id) }, L("Retry"))]))) : null,
                shown.layerErrors.length ? h("ul", { class: "m-map-status m-map-layer-errors" }, shown.layerErrors.map((error) => h("li", {
                    key: error.id, class: error.id === "record" ? "m-map-record-error" : null }, [
                    h("span", { class: "text-negative" }, L("Not shown") + ": " + error.label + (error.message ? " (" + error.message + ")" : "")),
                    this.configUrl ? [" ", h("button", { type: "button", class: "m-map-retry", onClick: () => this.load() }, L("Retry"))] : null]))) : null,
                shown.featuresTruncated ? h("div", { class: "m-map-status m-map-truncated", role: "note" },
                    L("Only some of the records are shown: zoom in or search to narrow them")) : null,
                this.located ? h("div", { class: "m-map-located", "aria-live": "polite" }, this.located) : null,
                shown.pick ? this.pickForm() : null,
                this.picked ? h("div", { class: "m-map-picked", "aria-live": "polite" }, L("Chosen") + ": " + map.formatCoordinates(this.picked.lon, this.picked.lat)) : null,
                this.liveShares.length ? h("ul", { class: "m-map-live", "aria-label": L("Live location") }, this.liveShares.map((share) => this.liveItem(share))) : null,
                this.points.length ? h("ul", { class: "m-map-places", "aria-label": L("Places") }, this.points.map((place) => this.placeItem(place))) : null,
                this.fallback && !this.points.length && shown.center ? h("div", { class: "m-map-center" }, L("Center") + ": " + map.formatCoordinates(shown.center[0], shown.center[1])) : null
            ]);
        }
    };

    const components = { "m-map-view": mapView };
    map.components = components;

    // Before WebrootVue.qvt.js its registry takes them; after it they are registered late on the running application.
    const qvt = moqui.qvt;
    if (qvt && qvt.registerLate) Object.keys(components).forEach((name) => qvt.registerLate(name, components[name]));
    else {
        const registry = moqui.qvtComponents = moqui.qvtComponents || {};
        Object.keys(components).forEach((name) => { registry[name] = components[name]; });
    }

    if (typeof module !== "undefined" && module.exports) module.exports = map;
})(typeof window !== "undefined" ? window : globalThis);
