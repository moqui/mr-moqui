/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// MapsLocalOnly (plans/Maps.md §5, §9 acceptance), on a server with maps@external-requests false (client/suite.js): a user
// whose profile is the local Protomaps profile (MapsLocalFixtures: a registered archive) browses Maps › Browse, which draws
// the archive with the packaged style, sprite and fonts, every request to this server (the browser's own resource list);
// a pan and a zoom ask a measured number of archive ranges, logged against the per-client all rate limit (50 a second,
// a burst of 500); and a locally resolvable place in the local-map dialog asks nothing outside either. The postal lookup with
// outside requests off is the Geo data's own (GeoDataUsaImport, framework MapsPostalLookup), not walked here.
// Run: ./moq accept tools --spec MapsLocalOnly
spec({timeout: 120})

const user = {user: "maps.local", password: "MapsLocal1!!"}
function mapsPage() {
    const page = browser.newPage(Object.assign({}, user))
    // the shell asks for the user's photo and their calls, which a user without Comms is refused
    page.allowProblems(/40[34] [A-Za-z ]+ .*\/(userPhoto\?userId=\w+|activeCalls)$/)
    return page
}
// what the page asked: each resource's URL, as the browser lists them
const asked = page => page.evaluate(() => performance.getEntriesByType("resource").map(entry => entry.name))
const outside = page =>
    page.evaluate(() =>
        performance
            .getEntriesByType("resource")
            .map(entry => entry.name)
            .filter(url => new URL(url).origin !== location.origin)
    )
const ranges = page => asked(page).filter(url => url.includes("/apps/maps/LocalMap/source?layerId=LOCAL_PROTOMAPS_LIGHT")).length

feature("Browse draws the local archive with the packaged style, sprite and fonts, asking only this server", {
    expect() {
        const page = mapsPage()
        page.goto("/qapps/maps/Browse").status == 200
        page.waitFor(() => !!document.querySelector("#MapsBrowseMap .ol-viewport"), {timeout: 60})
        // the archive's header, its tiles and the style
        page.waitFor(
            () => performance.getEntriesByType("resource").filter(entry => entry.name.includes("/LocalMap/source?")).length >= 2,
            {timeout: 60}
        )
        page.waitFor(
            () => performance.getEntriesByType("resource").some(entry => entry.name.includes("/libs/protomaps/light.json")),
            {timeout: 30}
        )
        page.evaluate(() => !document.querySelector("#MapsBrowseMap .m-map-retry"))
        asked(page).some(url => url.includes("/libs/protomaps/sprites/v4/light"))
        outside(page).length == 0
        // the attribution (collapsed behind its button): the layer's Protomaps and OpenStreetMap
        page.evaluate(() => (document.querySelector("#MapsBrowseMap .ol-attribution") || {}).textContent || "").includes(
            "OpenStreetMap"
        )
        !/not available|did not load/.test(page.text())
        // a pan and a zoom: the ranges asked, against the all rate limit
        const before = ranges(page)
        const started = Date.now()
        page.locator("#MapsBrowseMap .m-map-canvas").focus()
        for (let i = 0; i < 6; i++) page.keyboard.press("ArrowRight")
        page.keyboard.press("+")
        page.keyboard.press("+")
        sleep(2500)
        const asked2 = ranges(page) - before
        const seconds = (Date.now() - started) / 1000
        log.warn(
            "MapsLocalOnly: a pan of 6 steps and 2 zooms asked " +
                asked2 +
                " archive ranges in " +
                seconds.toFixed(1) +
                " s (the all rate limit: 50 a second, a burst of 500)"
        )
        asked2 < 500
        outside(page).length == 0
    }
})

feature("the local-map dialog of a locally resolvable place asks nothing outside", {
    expect() {
        const page = mapsPage()
        page.goto("/qapps/maps/LocalMap?longitude=11.25&latitude=43.77&label=Spot").status == 200
        page.waitFor(() => !!document.querySelector(".ol-viewport, .m-map-fallback"), {timeout: 60})
        page.waitFor(() => performance.getEntriesByType("resource").some(entry => entry.name.includes("/LocalMap/source?")), {
            timeout: 60
        })
        page.text().includes("43.77")
        outside(page).length == 0
    }
})
