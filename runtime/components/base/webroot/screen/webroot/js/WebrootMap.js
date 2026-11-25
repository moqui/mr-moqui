/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The map widget (plans/Maps.md §3.2), on the vendored OpenLayers bundle (libs/openlayers/ol-map.min.js, the global MoquiOl,
 * with ol-mapbox-style and the PMTiles sources). m-map of WebrootVue.qvt.js loads this file, the bundle and ol.css when the
 * first map mounts and draws m-map-view by its definition; the map-view XML element writes m-map. A render function, not a
 * template, so it needs no template compiler. The pure helpers are moqui.map, which test/client/pages/WebrootMap.page.js runs
 * in the page.
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
 *         kind pmtiles: url a PMTiles archive of this server (same origin only), styleUrl its style (raster: true for a
 *                       raster archive); a style's vector source is bound to the archive (applyStyle, updateSource false)
 *       basemap: the id of the one drawn first (else the first),
 *       allowedOrigins: ["https://tiles.example.org"]   origins its styles' sources, sprites and tiles may also come from
 *       notice: {text, href, linkText}  shown over the map: the effective configuration gives an administrator the way to
 *                                       Configuration when no provider is enabled
 *     },
 *     layers: ["PARKS"] (IDs asked for) or [{id, label, kind (geojson, gpx, kml, topojson, xyz, pmtiles), url, attribution,
 *             visible}] (as the effective configuration resolved them); an ID left unresolved shows as not available,
 *     center: [lon, lat], zoom: 0-22, extent: [west, south, east, north], projection: "EPSG:3857" | "EPSG:4326",
 *     points: [{id, lon, lat, label, precisionEnumId, precision, source}]  (longitude/latitude, sourceText are read too),
 *     features: a GeoJSON FeatureCollection in WGS84 (each feature's label or name property its label),
 *     pick: true to let the user choose a point, pickForm: the name of a form of the page whose latitude and longitude
 *           fields a choice fills (the map-view's pick-form),
 *     adapter, recordKey, addressPurpose: a record shown through a registered adapter, sent to config-url as given,
 *     layerErrors: [{id, label, reasonCode, message}]  what the server did not resolve (id "record": the record's place),
 *                  each named under the map, with Retry (the configuration asked again) when it came from config-url,
 *     featuresTruncated: true when a record layer had more features than were sent, units: "metric" | "imperial" (the
 *                  scale line's)
 *   }
 * The network contracts of the bundle (build/libs/refresh.mjs, OPENLAYERS_HOSTS; plans/Maps.md §3.2): a mapbox:// URL is
 * refused before it reaches the library; a style is loaded here, checked, its sources' attributions escaped, and its
 * metadata['ol:webfonts'] replaced with the local fonts' absolute template (map.webfonts), which is also the webfonts option;
 * a TileJSON is fetched here and handed on with its attribution escaped; KML is read with an iconUrlFunction answering a blank
 * data: icon (map.kmlOptions) and styled here; and nothing is asked of a host that is not the page's or the profile's own
 * (urlAllowed, and transformRequest for what ol-mapbox-style fetches). Locate me asks the browser for the position only when
 * pressed, and the position is neither kept nor sent.
 *
 * Events: pick ({lon, lat}, also the DOM event m-map-pick with that detail, which bubbles), select (a place or a feature:
 * {id, lon, lat, label}, with layerId for a record layer's feature) and view-change ({center, zoom, extent}). Each place
 * listed has Copy, which puts its latitude, longitude on the clipboard.
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
    const KINDS = ["outline", "style", "xyz", "pmtiles"];
    const LAYER_KINDS = ["geojson", "gpx", "kml", "topojson", "xyz", "pmtiles"];
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
            minZoom: num(basemap.minZoom), maxZoom: num(basemap.maxZoom), raster: basemap.raster === true || basemap.raster === "true" };
    };
    const normalizeLayer = (layer, index) => {
        if (typeof layer === "string") return text(layer) ? { id: text(layer), requested: true } : null;
        if (!layer || typeof layer !== "object") return null;
        return { id: text(layer.id) || "layer" + index, label: text(layer.label) || text(layer.id), kind: text(layer.kind).toLowerCase(), url: text(layer.url),
            styleUrl: text(layer.styleUrl), attribution: text(layer.attribution), visible: layer.visible !== false && layer.visible !== "false" };
    };

    // The configuration as m-map-view draws it (the shape at the top of this file), from a server's answer, a map-view's
    // attributes bound in a static template (texts) or a JSON text: numbers parsed, a point, center or extent that is not WGS84
    // degrees left out, layers as objects, and the outline profile when no basemap is given (outline true then).
    map.normalizeConfig = (input) => {
        let given = input;
        if (typeof given === "string") {
            try { given = JSON.parse(given); } catch (err) { given = {}; }
        }
        if (!given || typeof given !== "object") given = {};
        const profileIn = given.profile && typeof given.profile === "object" ? given.profile : {};
        const basemaps = (Array.isArray(profileIn.basemaps) ? profileIn.basemaps : []).map(normalizeBasemap).filter((basemap) => basemap);
        const outline = !basemaps.length;
        const profile = {
            profileId: text(profileIn.profileId) || (outline ? OUTLINE.profileId : text(given.profileId)),
            profileName: text(profileIn.profileName) || (outline ? OUTLINE.profileName : ""),
            basemaps: outline ? OUTLINE.basemaps.map(normalizeBasemap) : basemaps,
            basemap: text(profileIn.basemap),
            allowedOrigins: (Array.isArray(profileIn.allowedOrigins) ? profileIn.allowedOrigins : []).map(text).filter((origin) => origin),
            notice: profileIn.notice && typeof profileIn.notice === "object" && text(profileIn.notice.text) ?
                { text: text(profileIn.notice.text), href: map.safeHref(profileIn.notice.href), linkText: text(profileIn.notice.linkText) } : null
        };
        const center = numbers(given.center, 2);
        const extent = numbers(given.extent, 4);
        const zoom = num(given.zoom);
        const projection = text(given.projection).toUpperCase();
        return {
            profileId: text(given.profileId) || text(profileIn.profileId),
            profile, outline,
            layers: idList(given.layers).map(normalizeLayer).filter((layer) => layer),
            center: center && lonOk(center[0]) && latOk(center[1]) ? center : null,
            zoom: zoom !== null && zoom >= 0 && zoom <= 22 ? zoom : null,
            extent: extent && lonOk(extent[0]) && lonOk(extent[2]) && latOk(extent[1]) && latOk(extent[3]) && extent[1] <= extent[3] ? extent : null,
            projection: projection === "EPSG:4326" ? "EPSG:4326" : "EPSG:3857",
            points: map.normalizePoints(given.points),
            features: given.features && given.features.type === "FeatureCollection" && Array.isArray(given.features.features) ? given.features : null,
            pick: given.pick === true || given.pick === "true",
            pickForm: text(given.pickForm),
            adapter: text(given.adapter), recordKey: given.recordKey && typeof given.recordKey === "object" ? given.recordKey : null,
            addressPurpose: text(given.addressPurpose),
            layerErrors: map.normalizeLayerErrors(given.layerErrors),
            featuresTruncated: given.featuresTruncated === true || given.featuresTruncated === "true",
            units: text(given.units).toLowerCase() === "imperial" ? "imperial" : text(given.units).toLowerCase() === "metric" ? "metric" : ""
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
        out.pick = server.pick || local.pick;
        if (local.units) out.units = local.units;
        if (local.pickForm) out.pickForm = local.pickForm;
        // the map-view's own features (a record's area) beside the server's
        if (local.features) out.features = server.features ? { type: "FeatureCollection", features: server.features.features.concat(local.features.features) } : local.features;
        out.points = map.normalizePoints(server.points.concat(local.points));
        const known = new Set(server.layers.map((layer) => layer.id));
        out.layers = server.layers.concat(local.layers.filter((layer) => !known.has(layer.id)));
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
    // data:, blob:, file: or javascript: one, is refused, and so is a host the profile does not name.
    map.urlAllowed = (url, origins, pageOrigin) => {
        const value = text(url);
        if (!value || /^mapbox:/i.test(value)) return false;
        let parsed;
        try { parsed = new URL(value.replace(/\{[^}]*\}/g, "0"), pageOrigin + "/"); } catch (err) { return false; }
        if (parsed.protocol !== "https:" && parsed.protocol !== "http:") return false;
        return origins.has(parsed.origin);
    };
    // Whether a URL is the page's own (a PMTiles archive must be: its ranges are read with the session's access).
    map.sameOrigin = (url, pageOrigin) => map.urlAllowed(url, new Set([pageOrigin]), pageOrigin);

    // The URLs a style document names that may not be asked (its sources' url, tiles and GeoJSON data, and its sprite), each
    // resolved against the style's own URL. ol-mapbox-style does not read glyphs (it uses web fonts), so they are not asked.
    // bound: the IDs of sources m-map binds itself (a PMTiles archive's, whose url is pmtiles://), not asked by the library.
    map.refusedStyleUrls = (style, styleUrl, origins, pageOrigin, bound) => {
        const refused = [];
        const check = (url) => {
            if (typeof url !== "string" || !url) return;
            let resolved = url;
            if (!/^[a-z][a-z0-9+.-]*:/i.test(url)) {
                try { resolved = new URL(url.replace(/\{[^}]*\}/g, (m) => "__" + m.slice(1, -1) + "__"), new URL(styleUrl, pageOrigin + "/")).href; } catch (err) { resolved = url; }
            }
            if (!map.urlAllowed(resolved, origins, pageOrigin)) refused.push(url);
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
    // link only for an http(s) URL, opening in a new tab.
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
        emits: ["pick", "select", "view-change"],
        data() {
            return { effective: null, loadError: "", fallback: "", drawn: false, layerStatus: [], readout: "", picked: null, pickText: "", pickError: "",
                locating: false, located: "", basemapId: "", selectedId: "", announced: "" };
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
            // a parent's render makes its bound object again: only a configuration that changed draws the map again
            config(value, before) { if (JSON.stringify(value) !== JSON.stringify(before)) this.load(); }
        },
        mounted() { this.load(); },
        beforeUnmount() { this.teardown(); },
        methods: {
            pageOrigin() { return root.location.origin; },
            webfonts() { return map.webfonts(this.pageOrigin()); },
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
                this.basemapId = config.profile.basemap && config.profile.basemaps.some((b) => b.id === config.profile.basemap) ? config.profile.basemap :
                    (config.profile.basemaps[0] || {}).id || "";
                const O = root.MoquiOl;
                if (this.libraryError || !O) { this.fallback = L("The map could not be drawn here") + (this.libraryError ? ": " + this.libraryError : ""); return; }
                this.fallback = "";
                this.$nextTick(() => {
                    try { this.build(O, config); } catch (err) {
                        console.error("m-map: the map could not be drawn", err);
                        this.teardown();
                        this.fallback = L("The map could not be drawn here") + ": " + (err && err.message || err);
                    }
                });
            },
            teardown() {
                if (this.olMap) { this.olMap.setTarget(null); this.olMap.dispose(); this.olMap = null; }
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
                const view = new O.View({ projection: config.projection, center: this.toMap(0, 20), zoom: 2, maxZoom: 22 });
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
                this.drawBasemap(O);
                config.layers.forEach((layer) => this.drawLayer(O, layer));
                this.drawPlaces(O, config);
                this.fitView(O, config);
                olMap.on("pointermove", (event) => {
                    if (event.dragging) return;
                    const at = this.toLonLat(event.coordinate);
                    this.readout = map.formatCoordinates(at.lon, at.lat);
                });
                olMap.on("singleclick", (event) => {
                    const feature = olMap.forEachFeatureAtPixel(event.pixel, (found) => (found.get("place") || found.get("recordFeature") ? found : null));
                    if (feature && feature.get("place")) this.select(feature.get("place"));
                    else if (feature) this.selectFeature(feature, this.toLonLat(event.coordinate));
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
                    if (basemap.kind === "outline") this.outlineLayer(O, group, basemap, attributions);
                    else if (basemap.kind === "xyz") this.xyzLayer(O, group, basemap, attributions);
                    else if (basemap.kind === "style") this.styleLayer(O, group, basemap, attributions).catch(fail);
                    else if (basemap.kind === "pmtiles") this.pmtilesLayer(O, group, basemap, attributions).catch(fail);
                    else throw new Error(L("Not a kind of basemap this map draws") + ": " + basemap.kind);
                } catch (err) { fail(err); }
            },
            checkUrl(url, what) {
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
                const source = new O.VectorSource({ attributions, overlaps: false, loader: (extent, resolution, projection, success, failure) => {
                    root.fetch(url, { credentials: "same-origin" }).then((response) => {
                        if (!response.ok) throw new Error("HTTP " + response.status);
                        return response.json();
                    }).then((topology) => {
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
            // the request filter of what ol-mapbox-style fetches (TileJSON, tiles, sprites): only the profile's hosts
            // A TileJSON ("Source") is fetched here and answered with its attribution escaped, since the attribution control sets it
            // as HTML before m-map can replace it.
            transformRequest() {
                const origins = this.origins, page = this.pageOrigin();
                return (url, resourceType) => {
                    if (!map.urlAllowed(url, origins, page)) return refusal(url);
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
                    }).then((style) => {
                        const sources = style && style.sources && typeof style.sources === "object" ? style.sources : {};
                        const sourceId = bindVector ? Object.keys(sources).find((id) => (sources[id] || {}).type === "vector") : null;
                        const refused = map.refusedStyleUrls(style, url, this.origins, this.pageOrigin(), sourceId ? [sourceId] : null);
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
                const url = new URL(entry.url, this.pageOrigin() + "/").href;
                // the archive's header first: one that cannot be read by range fails here, by name, not tile by tile in silence
                const archive = new O.PMTiles(url);
                return archive.getHeader().then(() => {
                    if (entry.raster) {
                        const source = new O.PMTilesRasterSource({ url: archive.source, attributions });
                        this.watchSource(source, entry, "tile");
                        group.getLayers().push(new O.TileLayer({ source }));
                        return null;
                    }
                    const layer = new O.VectorTileLayer({ declutter: true, source: new O.PMTilesVectorSource({ url: archive.source, attributions }) });
                    this.watchSource(layer.getSource(), entry, "tile");
                    group.getLayers().push(layer);
                    return entry.styleUrl ? this.pmtilesStyle(O, layer, entry) : null;
                });
            },
            pmtilesStyle(O, layer, entry) {
                return this.loadStyle(entry.styleUrl, true).then(({ style, sourceId }) => {
                    // the style's vector source is the archive: ol-mapbox-style does not know pmtiles:// URLs
                    return O.applyStyle(layer, style, { source: sourceId, updateSource: false, webfonts: this.webfonts(), transformRequest: this.transformRequest() })
                        .then(() => O.applyBackground(layer, style));
                }).then(() => this.status(entry.id, entry.label, "ready"));
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
                    if (layer.kind === "xyz") this.xyzLayer(O, group, layer, attributions);
                    else if (layer.kind === "pmtiles") this.pmtilesLayer(O, group, layer, attributions).catch(fail);
                    else if (LAYER_KINDS.includes(layer.kind)) {
                        this.checkUrl(layer.url, L("The layer"));
                        const format = layer.kind === "gpx" ? new O.GPX() : layer.kind === "kml" ? new O.KML(map.kmlOptions()) :
                            layer.kind === "topojson" ? new O.TopoJSON() : new O.GeoJSON();
                        const source = new O.VectorSource({ url: layer.url, format, attributions });
                        this.watchSource(source, layer, "vector");
                        group.getLayers().push(new O.VectorLayer({ source, style: (feature) => this.featureStyle(O, feature) }));
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
                const label = text(feature.get("label") || feature.get("name"));
                return new O.Style({
                    image: new O.CircleStyle({ radius: 5, fill: new O.Fill({ color: "rgba(25,118,210,0.85)" }), stroke: new O.Stroke({ color: "#fff", width: 1.5 }) }),
                    stroke: new O.Stroke({ color: "rgba(25,118,210,0.9)", width: 2 }), fill: new O.Fill({ color: "rgba(25,118,210,0.15)" }),
                    text: label ? this.labelText(O, label) : undefined
                });
            },
            labelText(O, label) {
                // drawn on the canvas, so no markup in a label is read; long labels are cut
                const shown = label.length > 40 ? label.slice(0, 39) + "…" : label;
                return new O.Text({ text: shown, offsetY: -14, font: "12px sans-serif", fill: new O.Fill({ color: "#1d1d1d" }), stroke: new O.Stroke({ color: "#fff", width: 3 }) });
            },
            markerStyle(O, feature) {
                const kind = feature.get("kind");
                const color = kind === "picked" ? "#c62828" : kind === "located" ? "#2e7d32" : feature.get("place") && feature.get("place").id === this.selectedId ? "#ef6c00" : "#1976d2";
                const label = kind === "place" ? text(feature.get("place").label) : "";
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
                if (this.markerSource) this.markerSource.changed();
                this.$emit("select", { id: place.id, lon: place.lon, lat: place.lat, label: place.label });
            },
            // a record layer's feature (config.features): its id, label and layer, at the point clicked
            selectFeature(feature, at) {
                this.selectedId = "";
                if (this.markerSource) this.markerSource.changed();
                this.$emit("select", { id: String(feature.getId() === undefined ? "" : feature.getId()), lon: at.lon, lat: at.lat,
                    label: text(feature.get("label") || feature.get("name")), layerId: text(feature.get("layerId")) });
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
            mark(kind, point) {
                const O = root.MoquiOl;
                if (!O || !this.markerSource) return;
                this.markerSource.getFeatures().filter((feature) => feature.get("kind") === kind).forEach((feature) => this.markerSource.removeFeature(feature));
                this.markerSource.addFeature(new O.Feature({ geometry: new O.Point(this.toMap(point.lon, point.lat)), kind }));
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
            onCanvasKey(event) {
                // only on the map itself: Enter on one of OpenLayers' controls inside it is that control's
                if (event.key === "Enter" && this.shown.pick && event.target === event.currentTarget) { event.preventDefault(); this.chooseCenter(); }
            },
            onCanvasFocus() {
                const center = this.olMap && this.olMap.getView().getCenter();
                if (center) { const at = this.toLonLat(center); this.readout = map.formatCoordinates(at.lon, at.lat); }
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
            const canvasLabel = L("Map") + ": " + L("arrow keys move it, + and - zoom") + (shown.pick ? ", " + L("Enter chooses the center") : "");
            return h("div", { class: "m-map", id: this.id }, [
                notice ? h("div", { class: "m-map-notice", role: "note" }, notice) : null,
                this.loadError ? h("div", { class: "m-map-status m-map-status-failed", role: "status" }, this.loadError) : null,
                this.fallback ? h("div", { class: "m-map-fallback", role: "status" }, [this.fallback, ". ", L("The places are listed below.")]) :
                    h("div", { class: "m-map-frame", style: { height: this.height || "360px" } }, [
                        h("div", { ref: "canvas", class: "m-map-canvas", tabindex: 0, role: "application", "aria-label": canvasLabel,
                            onKeydown: this.onCanvasKey, onFocus: this.onCanvasFocus }),
                        shown.pick ? h("div", { class: "m-map-crosshair", "aria-hidden": "true" }) : null,
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
