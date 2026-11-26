/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// MapsBrowseWalk (plans/Maps.md §3.1, §9): Maps › Browse in a browser on the qvt shell, at desktop and phone width, as a Maps
// user (MapsWalkFixtures: maps.walk, a record layer of the framework's test adapter on the outline profile, a saved view):
// the map draws with its scale line and attribution; Layers names the record layer and says its features were cut;
// Search finds the saved place, a record, and a postal code (MapsWalkFixtures' 60601) with its places as choices, going to
// it showing its approximate precision; going to the record shows its details; Save view saves where the map is
// and lists it; on a phone the panel is a bottom sheet over the map.
// Run: ./moq accept tools --spec MapsBrowseWalk
spec({timeout: 120})

const user = {user: "maps.walk", password: "MapsWalk1!!"}
function mapsPage(extra) {
    const page = browser.newPage(Object.assign({}, user, extra || {}))
    // the shell asks for the user's photo and their calls, which a user without Comms is refused
    page.allowProblems(/40[34] [A-Za-z ]+ .*\/(userPhoto\?userId=\w+|activeCalls)$/)
    return page
}
// the map drawn: OpenLayers' canvas, or the place list when the bundle or WebGL is missing
const drawn = () => !!document.querySelector("#MapsBrowseMap .ol-viewport, #MapsBrowseMap .m-map-fallback")

feature("desktop: the map draws; Layers, Search, a record's details and Save view", {
    expect() {
        const page = mapsPage()
        page.goto("/qapps/maps/Browse").status == 200
        page.waitFor(drawn, {timeout: 60})
        page.evaluate(() => !!document.querySelector("#MapsBrowseMap .ol-scale-line"))
        page.evaluate(() => !!document.querySelector("#MapsBrowseMap .ol-attribution"))
        // Layers: the record layer, and the cut said there (the badge on the button)
        page.getByRole("button", {name: "Layers"}).click()
        page.waitFor(() => (document.querySelector(".m-maps-panel") || {}).textContent.includes("Walk records"))
        page.text().includes("Only some of the records are shown")
        // Search: the saved place
        page.getByLabel("Search places, records and postal codes").fill("harbor")
        page.click("#MapsSearchButton")
        page.waitFor(() => !!document.querySelector('[data-maps-result="view:MAPS_WALK_HARBOR"]'))
        page.locator('[data-maps-result="view:MAPS_WALK_HARBOR"]').click()
        // a postal code of the user's country: the code and its two places as choices, approximate, the country shown
        page.getByLabel("Search places, records and postal codes").fill("60601")
        page.click("#MapsSearchButton")
        page.waitFor(() => !!document.querySelector('[data-maps-result="postal:USA_Z_60601"]'))
        page.evaluate(() => document.querySelectorAll('[data-maps-result^="postal:USA_Z_60601:"]').length) == 2
        page.locator('[data-maps-result="postal:USA_Z_60601"]').click()
        page.waitFor(() =>
            (document.querySelector(".m-maps-panel") || {}).textContent.includes("Approximate location — postal area")
        )
        page.text().includes("60601 — Chicago, Loop")
        // and a record of the layer, whose details show where it is
        page.getByLabel("Search places, records and postal codes").fill("Feature 1")
        page.click("#MapsSearchButton")
        page.waitFor(() => !!document.querySelector('[data-maps-result^="record:"]'))
        page.locator('[data-maps-result^="record:"]').first().click()
        page.waitFor(() => (document.querySelector(".m-maps-panel") || {}).textContent.includes("Copy coordinates"))
        page.text().includes("Feature 1")
        page.text().includes("Walk records")
        // Save view: where the map is, listed under Saved views
        page.getByRole("button", {name: "Save view"}).click()
        page.getByLabel("Name").fill("Walk saved " + Date.now())
        page.evaluate(() => {
            const dialog = document.querySelector(".q-dialog")
            Array.from(dialog.querySelectorAll("button"))
                .find(button => button.textContent.trim() === "Save view")
                .click()
            return true
        })
        page.waitFor(() => !document.querySelector(".q-dialog"))
        page.getByRole("button", {name: "Saved views"}).click()
        page.waitFor(() => (document.querySelector(".m-maps-panel") || {}).textContent.includes("Walk saved"))
        !/Error rendering|Exception in render/.test(page.text())
    }
})

feature("phone width: the map fills the page and a panel is a bottom sheet over it", {
    expect() {
        const page = mapsPage({viewport: {width: 390, height: 844}})
        page.goto("/qapps/maps/Browse").status == 200
        page.waitFor(drawn, {timeout: 60})
        const frame = page.locator("#MapsBrowseMap").boundingBox()
        frame.width >= page.evaluate(() => document.documentElement.clientWidth) - 20
        page.getByRole("button", {name: "Layers"}).click()
        page.waitFor(() => !!document.querySelector(".m-maps-sheet"))
        const sheet = page.locator(".m-maps-sheet").boundingBox()
        sheet.width >= page.evaluate(() => document.documentElement.clientWidth) - 2
        Math.abs(sheet.y + sheet.height - page.evaluate(() => window.innerHeight)) <= 2
        page.text().includes("Walk records")
        page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)
    }
})
