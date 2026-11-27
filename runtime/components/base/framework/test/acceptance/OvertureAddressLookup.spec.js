// OvertureAddressLookup (plans/archive/Maps-Overture.md §8): the latest real Overture release's Addresses for two small boxes,
// central Florence (1.6 by 1.7 km, the Italian countrywide source, CC BY 4.0, no postal codes) and part of Boston's Back
// Bay (1.2 by 1.1 km, Massachusetts' statewide points, a point per unit, which Overture marks LicenseRef-Proprietary),
// through org.moqui.maps.OvertureServices with the catalog and the files opened to their hosts alone
// (HttpFixtures.allowNetwork). Each region's first run waits for review of its sources' licenses, which the test
// acknowledges as an administrator would; it records the wall time, files, row groups, bytes, rows kept and rejected,
// licenses and the process memory where the platform reports it (Linux), then tests the local geocoder (a fixed query of
// its own data) and runs sample lookups through find#Places with outside requests off, logging each one's time, precision,
// match kind, candidates and ambiguity as OvertureAddressLookup lines. The regions are removed afterwards.
// Run when asked: ./moq accept framework --spec OvertureAddressLookup (a minute or so; about 10 MB downloaded)
const S = "org.moqui.maps.OvertureServices."
const IMPORT = "moqui.maps.OvertureImport"
const PROVIDER = "OVT_ACC_ADDRESSES"
const HOSTS = ["https://stac.overturemaps.org/", "https://overturemaps-us-west-2.s3.us-west-2.amazonaws.com/"]
const REGIONS = {
    florence: {regionName: "Acceptance Florence addresses", west: 11.245, south: 43.765, east: 11.265, north: 43.78},
    boston: {regionName: "Acceptance Back Bay addresses", west: -71.09, south: 42.345, east: -71.075, north: 42.355}
}
const SAMPLES = {
    florence: [
        "Via Maggio 15, Firenze",
        "Via dei Fossi 51, Firenze",
        "Via Por Santa Maria 53r, Firenze",
        "Via dei Fossi 999, Firenze",
        "Via Inesistente 1, 50123 Firenze"
    ],
    boston: [
        "52 Hemenway St, Boston, MA 02115",
        "373 Commonwealth Ave Apt 503, Boston, MA 02115",
        "250 Newbury Street #3F, Boston",
        "999 Hemenway Street, Boston MA",
        "1 Nowhere Street, Boston, MA 02116"
    ]
}
const http = ec.factory.getTool("HttpFixtures")
const results = {}
const regionIds = {}
let theme = null

const sys = run => {
    const was = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!was) ec.artifactExecution.enableAuthz()
    }
}
const log = (label, value) => ec.logger.warn("OvertureAddressLookup: " + label + " " + JSON.stringify(value))
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
const runOf = importId => sys(() => ec.entity.find(IMPORT).condition({importId}).one())
// The latest release's addresses of a region, its run waited for (at most ten minutes) and activated once the licenses its
// sources name are acknowledged
function importRegion(key) {
    const regionId = regionIds[key]
    const startedAt = Date.now()
    const started = invoke(S + "start#OvertureImport", {regionId, themeId: "ADDRESSES"})
    if (!started.importId) return {started}
    const deadline = Date.now() + 600000
    while (Date.now() < deadline && ["OviPending", "OviRunning"].includes(runOf(started.importId)?.statusId)) sleep(250)
    let run = runOf(started.importId)
    const seconds = Math.round((Date.now() - startedAt) / 100) / 10
    let activated = null
    if (run.statusId === "OviReview") {
        invoke(S + "acknowledge#OvertureLicense", {themeId: "ADDRESSES", licenses: String(run.newLicenses || "").split(",")})
        activated = invoke(S + "activate#OvertureImport", {importId: run.importId})
    }
    const reviewed = run.newLicenses || null
    run = runOf(started.importId)
    const outcome = {
        seconds,
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
        countries: run.countryCodes,
        reviewedLicenses: reviewed,
        licenses: JSON.parse(run.licensesJson || "[]"),
        memory: memory()
    }
    log(key + " import", outcome)
    return {started, run, activated, outcome}
}
// The sample lookups of a region through find#Places, with outside requests off: each one's time and what it found
function lookups(key) {
    return withConf("maps@external-requests", "false", () =>
        SAMPLES[key].map(query => {
            const startedAt = Date.now()
            const found = invoke("org.moqui.maps.MapServices.find#Places", {query, providerId: PROVIDER, limit: 5})
            const out = {
                query,
                ms: Date.now() - startedAt,
                reasonCode: found.reasonCode || null,
                ambiguous: !!found.ambiguous,
                candidates: Array.from(found.candidates || []).map(candidate => ({
                    label: candidate.label,
                    matchKind: candidate.matchKind,
                    precision: candidate.precisionEnumId,
                    lon: candidate.lon,
                    lat: candidate.lat
                }))
            }
            log(key + " lookup", out)
            return out
        })
    )
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        http.reset()
        http.allowNetwork(HOSTS)
        setConf("maps@external-requests", "true")
        setConf("maps@overture-catalog", "https://stac.overturemaps.org/catalog.json")
        sys(() => {
            const row = ec.entity.find("moqui.maps.OvertureTheme").condition({themeId: "ADDRESSES"}).one()
            theme = {licenses: row.licenses, licenseAckUserId: row.licenseAckUserId, licenseAckDate: row.licenseAckDate}
            ec.entity
                .makeValue("moqui.maps.MapProvider")
                .setAll({
                    providerId: PROVIDER,
                    providerName: "Acceptance Overture addresses",
                    providerKindEnumId: "MPK_GEOCODER_OVERTURE",
                    allowPrivateNetwork: "N",
                    disabled: "N",
                    sequenceNum: 1
                })
                .createOrUpdate()
        })
        invoke(S + "acknowledge#OvertureLicense", {themeId: "ADDRESSES"})
        for (const [key, region] of Object.entries(REGIONS))
            regionIds[key] = invoke(S + "store#OvertureRegion", Object.assign({themeIds: ["ADDRESSES"]}, region)).regionId
        log("start", {at: ec.l10n.format(ec.user.nowTimestamp, "yyyy-MM-dd HH:mm:ss"), memory: memory()})
    },
    cleanupSpec() {
        ec.message.clearErrors()
        for (const regionId of Object.values(regionIds)) if (regionId) invoke(S + "remove#OvertureTheme", {regionId})
        sys(() => {
            for (const regionId of Object.values(regionIds).filter(Boolean)) {
                const importIds = Array.from(ec.entity.find(IMPORT).condition({regionId}).list()).map(run => run.importId)
                if (importIds.length)
                    ec.entity.find("moqui.maps.OvertureAddress").condition("importId", "in", importIds).deleteAll()
                ec.entity.find(IMPORT).condition({regionId}).deleteAll()
            }
            ec.entity.find("moqui.maps.OvertureTheme").condition({themeId: "ADDRESSES"}).one().setAll(theme).update()
            ec.entity.find("moqui.server.ConnectionCheck").condition("connectionKey", PROVIDER).deleteAll()
            ec.entity.find("moqui.maps.MapProvider").condition("providerId", PROVIDER).deleteAll()
        })
        http.reset()
        log("results", results)
    }
})

feature("central Florence's addresses import, the local geocoder passes its test, and its lookups say their precision", {
    when() {
        const imported = importRegion("florence")
        const tested = invoke("org.moqui.admin.ConfigurationServices.test#Connection", {
            adapterKey: "mapProvider",
            connectionKey: PROVIDER
        })
        const found = lookups("florence")
        results.florence = {import: imported.outcome, tested: !!tested.passed, lookups: found}
        return {imported, tested, found}
    },
    then({imported, tested, found}) {
        imported.started.error == ""
        imported.run.statusId == "OviActive"
        imported.run.rowsKept > 1000
        imported.run.rowGroupsSkipped > imported.run.rowGroupsRead
        imported.run.countryCodes == "IT"
        tested.passed == true
        found[0].candidates[0].precision == "GPP_ADDRESS_MATCH"
        found[3].candidates[0].matchKind == "street"
        found.every(lookup =>
            lookup.candidates.every(c => ["GPP_ADDRESS_MATCH", "GPP_PLACE_ESTIMATE", "GPP_POSTAL_AREA"].includes(c.precision))
        )
    }
})

feature("part of Boston's Back Bay imports with its units, and its lookups match the ZIP code and the unit", {
    when() {
        const imported = importRegion("boston")
        const found = lookups("boston")
        results.boston = {import: imported.outcome, lookups: found}
        return {imported, found}
    },
    then({imported, found}) {
        imported.started.error == ""
        imported.run.statusId == "OviActive"
        imported.run.rowsKept > 1000
        imported.run.countryCodes == "US"
        found[0].candidates[0].precision == "GPP_ADDRESS_MATCH"
        found[0].candidates[0].label.includes("02115")
        found[1].candidates[0].precision == "GPP_ADDRESS_MATCH"
        found[3].candidates[0].matchKind == "street"
        found[4].candidates.every(c => c.precision != "GPP_ADDRESS_MATCH")
    }
})
