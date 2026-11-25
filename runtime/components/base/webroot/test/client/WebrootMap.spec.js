/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// m-map's pure helpers (js/WebrootMap.js) in the page, with Vue's runtime for its list of places and the vendored OpenLayers
// bundle for reading a KML file; drawing a map is not tested here.
spec({ server: false, timeout: 60 })
browser.runInPage({
    scripts: ["/libs/vue/vue.runtime.global.prod.js", "/js/MoquiLib.js", "/libs/openlayers/ol-map.min.js", "/js/WebrootMap.js"],
    test: "/files/component/webroot/test/client/pages/WebrootMap.page.js",
})
