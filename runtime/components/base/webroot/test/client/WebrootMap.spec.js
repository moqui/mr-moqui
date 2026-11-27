/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// m-map's pure helpers (js/WebrootMap.js) in the page, with Vue's runtime for its list of places; the page imports the
// vendored OpenLayers bundle (an ES module) for reading a KML file, and its raster part for projections. One map is drawn: an
// offline package's (plans/Maps-Offline.md §2.5, package mode), from its files alone, with no outside request and its
// attribution as text; drawing a server's map is a browser walk.
spec({ server: false, timeout: 60 })
browser.runInPage({
    scripts: ["/libs/vue/vue.runtime.global.prod.js", "/js/MoquiLib.js", "/js/WebrootMap.js"],
    test: "/files/component/webroot/test/client/pages/WebrootMap.page.js",
})
