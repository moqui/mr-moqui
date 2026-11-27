// OvertureRegionImport (plans/archive/Maps-Overture.md §6): the latest real Overture release for a box of about 3 km² (central
// Florence: 1.6 by 1.7 km), Places and then Buildings, through org.moqui.maps.OvertureServices with outside requests, the
// catalog and the files opened to their hosts alone (HttpFixtures.allowNetwork). It records each import's wall time, the
// files, row groups and bytes read, the rows kept and rejected, the licenses found, the archive's size and tiles, and the
// process memory where the platform reports it (Process MemoryUsed and MemoryPeak of the status map, Linux), logged as
// OvertureRegionImport lines; then a search of the places, and the region removed.
// Run when asked: ./moq accept framework --spec OvertureRegionImport (a minute or two; a few MB downloaded)
const S = "org.moqui.maps.OvertureServices."
const IMPORT = "moqui.maps.OvertureImport"
const HOSTS = ["https://stac.overturemaps.org/", "https://overturemaps-us-west-2.s3.us-west-2.amazonaws.com/"]
const REGION = {regionName: "Acceptance Florence", west: 11.245, south: 43.765, east: 11.265, north: 43.78}
const http = ec.factory.getTool("HttpFixtures")
const themes = {}
const results = {}
let regionId = null

const sys = run => {
    const was = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!was) ec.artifactExecution.enableAuthz()
    }
}
const log = (label, value) => ec.logger.warn("OvertureRegionImport: " + label + " " + JSON.stringify(value))
function memory() {
    try {
        const process = ec.factory.getStatusMap(false)?.Process || {}
        return {usedMiB: process.MemoryUsed ?? null, peakMiB: process.MemoryPeak ?? null}
    } catch (error) {
        return {usedMiB: null, peakMiB: null}
    }
}
function invoke(name, params) {
    try {
        const result = call(name, params) || {}
        const error = ec.message.getErrorsString() || ""
        ec.message.clearErrors()
        return Object.assign({}, result, {error})
    } catch (e) {
        ec.message.clearErrors()
        return {error: String(e.message || e)}
    }
}
// An import of the latest release, its run waited for (at most ten minutes)
function importTheme(themeId) {
    const startedAt = Date.now()
    const started = invoke(S + "start#OvertureImport", {regionId, themeId})
    if (!started.importId) return {started}
    const deadline = Date.now() + 600000
    const going = () =>
        ["OviPending", "OviRunning"].includes(
            sys(() => ec.entity.find(IMPORT).condition({importId: started.importId}).one())?.statusId
        )
    while (Date.now() < deadline && going()) sleep(250)
    const run = sys(() => ec.entity.find(IMPORT).condition({importId: started.importId}).one())
    const outcome = {
        themeId,
        seconds: Math.round((Date.now() - startedAt) / 100) / 10,
        statusId: run.statusId,
        reason: run.failureText || null,
        release: run.releaseVersion,
        schema: run.schemaVersion,
        files: run.filesRead,
        rowGroupsRead: run.rowGroupsRead,
        rowGroupsSkipped: run.rowGroupsSkipped,
        bytesRead: run.bytesRead,
        rowsRead: run.rowsRead,
        rowsKept: run.rowsKept,
        rowsRejected: run.rowsRejected,
        newLicenses: run.newLicenses || null,
        licenses: JSON.parse(run.licensesJson || "[]"),
        archiveSize: run.archiveSize || null,
        tileCount: run.tileCount || null,
        maxZoom: run.maxZoom ?? null,
        memory: memory()
    }
    log(themeId, outcome)
    return {started, run, outcome}
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        http.reset()
        http.allowNetwork(HOSTS)
        setConf("maps@external-requests", "true")
        setConf("maps@overture-catalog", "https://stac.overturemaps.org/catalog.json")
        sys(() => {
            for (const themeId of ["PLACES", "BUILDINGS"]) {
                const theme = ec.entity.find("moqui.maps.OvertureTheme").condition({themeId}).one()
                themes[themeId] = {
                    licenses: theme.licenses,
                    licenseAckUserId: theme.licenseAckUserId,
                    licenseAckDate: theme.licenseAckDate
                }
            }
        })
        for (const themeId of ["PLACES", "BUILDINGS"]) invoke(S + "acknowledge#OvertureLicense", {themeId})
        regionId = invoke(S + "store#OvertureRegion", Object.assign({themeIds: ["PLACES", "BUILDINGS"]}, REGION)).regionId
        log("start", {at: ec.l10n.format(ec.user.nowTimestamp, "yyyy-MM-dd HH:mm:ss"), memory: memory()})
    },
    cleanupSpec() {
        ec.message.clearErrors()
        if (regionId) invoke(S + "remove#OvertureTheme", {regionId})
        sys(() => {
            if (regionId) {
                const importIds = Array.from(ec.entity.find(IMPORT).condition({regionId}).list()).map(run => run.importId)
                if (importIds.length)
                    ec.entity.find("moqui.maps.OverturePlace").condition("importId", "in", importIds).deleteAll()
                ec.entity.find(IMPORT).condition({regionId}).deleteAll()
            }
            for (const themeId of Object.keys(themes))
                ec.entity.find("moqui.maps.OvertureTheme").condition({themeId}).one().setAll(themes[themeId]).update()
        })
        http.reset()
        log("results", results)
    }
})

feature("the latest release's places of central Florence import, read by range, and are searched", {
    when() {
        const releases = invoke(S + "find#OvertureReleases", {})
        const places = importTheme("PLACES")
        // a release whose places name a license not yet acknowledged waits for review: acknowledged, it is activated
        let activated = null
        if (places.run?.statusId === "OviReview") {
            invoke(S + "acknowledge#OvertureLicense", {
                themeId: "PLACES",
                licenses: String(places.run.newLicenses || "").split(",")
            })
            activated = invoke(S + "activate#OvertureImport", {importId: places.run.importId})
        }
        const searchStarted = Date.now()
        const found = ec.resource
            .script("component://framework/service/org/moqui/maps/OvertureData.js")
            .execute("searchPlaces", {q: "caff"})
        results.places = Object.assign({}, places.outcome, {
            searchMs: Date.now() - searchStarted,
            searchResults: found.results.length
        })
        return {releases, places, activated, found}
    },
    then({releases, places, activated, found}) {
        releases.error == ""
        releases.latest != null
        places.started.error == ""
        places.run.statusId == "OviActive" || activated?.error == ""
        places.run.releaseVersion == releases.latest
        places.run.rowsKept > 100
        places.run.rowGroupsSkipped > places.run.rowGroupsRead
        found.results.length > 0
    }
})

feature("the latest release's buildings of central Florence become the region's layer", {
    when() {
        const buildings = importTheme("BUILDINGS")
        let activated = null
        if (buildings.run?.statusId === "OviReview") {
            invoke(S + "acknowledge#OvertureLicense", {
                themeId: "BUILDINGS",
                licenses: String(buildings.run.newLicenses || "").split(",")
            })
            activated = invoke(S + "activate#OvertureImport", {importId: buildings.run.importId})
        }
        results.buildings = buildings.outcome
        const regionTheme = sys(() =>
            ec.entity.find("moqui.maps.OvertureRegionTheme").condition({regionId, themeId: "BUILDINGS"}).one()
        )
        const layer = sys(() =>
            ec.entity
                .find("moqui.maps.MapLayer")
                .condition({layerId: regionTheme?.layerId || "_NA_"})
                .one()
        )
        return {buildings, activated, layer}
    },
    then({buildings, activated, layer}) {
        buildings.started.error == ""
        buildings.run.statusId == "OviActive" || activated?.error == ""
        buildings.run.rowsKept > 500
        buildings.run.tileCount > 0
        layer.sourceRevision != null
        layer.attribution.includes("OpenStreetMap")
    }
})
