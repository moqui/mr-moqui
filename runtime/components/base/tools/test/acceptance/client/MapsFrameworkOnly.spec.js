/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// MapsFrameworkOnly (plans/Maps.md §1, §9 acceptance): an installation of the framework and tools components alone (this
// suite's server: no Mantle, BOSS, HOME or PASS), where the Maps app, its Configuration with Archives, a registered local
// archive and the local-map dialog with the framework's test adapter (MapsWalkFixtures, MapsLocalFixtures) all work.
// Run: ./moq accept tools --spec MapsFrameworkOnly
spec({timeout: 120})

const record = recordId => ({adapterKey: "MAPS_WALK_ADAPTER", recordKey: JSON.stringify({recordId})})

feature("no Mantle: its entities and apps are not there", {
    expect() {
        login("john.doe")
        const party = http.get("/rest/e1/mantle.party.Party", {params: {pageSize: 1}, accept: "json"})
        party.status >= 400
        http.get("/apps/boss").status != 200
        logout()
    }
})

feature("the Maps app, Configuration's Archives and a registered archive answer an administrator", {
    expect() {
        login("john.doe")
        const browse = http.get("/qapps/maps/Browse")
        browse.status == 200
        const archives = http.get("/apps/maps/Configuration/Archives")
        archives.status == 200
        archives.text.includes("Map archives")
        archives.text.includes("Local map (light)")
        const config = http.get("/apps/maps/LocalMap/getConfig", {params: {profileId: "LOCAL_PROTOMAPS"}, accept: "json"}).json()
        const url = config.profile.basemaps[0].url
        url.startsWith("/apps/maps/LocalMap/source?layerId=LOCAL_PROTOMAPS_LIGHT&revision=")
        const header = http.get(url, {headers: {Range: "bytes=0-126"}})
        header.status == 206 && header.magic == "PMTi"
        logout()
    }
})

feature("the local-map dialog shows a record of the framework's test adapter, in a browser", {
    expect() {
        const page = browser.newPage({user: "maps.walk", password: "MapsWalk1!!"})
        page.allowProblems(/40[34] [A-Za-z ]+ .*\/(userPhoto\?userId=\w+|activeCalls)$/)
        const params = record("LOCATED")
        page.goto("/qapps/maps/LocalMap?adapterKey=" + params.adapterKey + "&recordKey=" + encodeURIComponent(params.recordKey))
            .status == 200
        page.waitFor(() => !!document.querySelector(".ol-viewport, .m-map-fallback"), {timeout: 60})
        !/not available/i.test(page.text())
        !/Error rendering|Exception in render/.test(page.text())
    }
})
