// GeoDataUsaImport (plans/archive/Maps.md §6.3, §9 acceptance): the real US Geo data through the importer, with outside requests: the
// Census Gazetteer Counties and ZCTAs, the Census 2020 ZCTA-to-county relationship file and GeoNames' US postal codes,
// downloaded from their registered hosts (moqui_core::outbound, opened to them alone with HttpFixtures.allowNetwork). It
// records each import's wall time and counts, the rows in use after it, the postal lookup's time cold and cached, and the
// process memory where the platform reports it (Process MemoryUsed and MemoryPeak of the status map, Linux), then a refresh:
// the same imports again, conditional (an unchanged source ends as current) and forced (downloaded, read and compared
// whole), neither changing a row. The numbers are logged as GeoDataUsaImport lines, for plans/archive/Maps.md §6.3.
// Run when asked: ./moq accept framework --spec GeoDataUsaImport (minutes; about 15 MB downloaded)
const S = "org.moqui.maps.GeoDataServices."
const SETS = [
    {dataSetId: "CENSUS_COUNTY"},
    {dataSetId: "CENSUS_ZCTA"},
    {dataSetId: "CENSUS_ZCTA_COUNTY"},
    {dataSetId: "GEONAMES_POSTAL", countryGeoId: "USA"}
]
const HOSTS = ["https://www2.census.gov/", "https://download.geonames.org/"]
const http = ec.factory.getTool("HttpFixtures")
const acks = {}
const results = {first: [], conditional: [], forced: []}

const sys = run => {
    const was = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!was) ec.artifactExecution.enableAuthz()
    }
}
const log = (label, value) => ec.logger.warn("GeoDataUsaImport: " + label + " " + JSON.stringify(value))
function memory() {
    try {
        const process = ec.factory.getStatusMap(false)?.Process || {}
        return {usedMiB: process.MemoryUsed ?? null, peakMiB: process.MemoryPeak ?? null}
    } catch (error) {
        return {usedMiB: null, peakMiB: null}
    }
}
const count = (entity, build) => sys(() => Number(build(ec.entity.find(entity)).count()))
function inUse() {
    return {
        postalCodes: count("moqui.basic.GeoPostalCode", find =>
            find.condition("countryGeoId", "USA").condition("thruDate", null)
        ),
        points: count("moqui.basic.GeoPointAssoc", find => find.condition("geoId", "like", "USA_%").condition("thruDate", null)),
        placeNames: count("moqui.basic.GeoPostalCodePlace", find =>
            find.condition("geoId", "like", "USA_Z_%").condition("thruDate", null)
        ),
        geos: count("moqui.basic.Geo", find => find.condition("geoId", "like", "USA_%")),
        countyLinks: count("moqui.basic.GeoAssoc", find =>
            find.condition("geoId", "like", "USA_C_%").condition("toGeoId", "like", "USA_Z_%")
        )
    }
}
// Each data set imported in turn: its wall time, outcome and counts, and the memory after it
function importAll(label, extra) {
    const out = []
    for (const set of SETS) {
        const started = Date.now()
        const result =
            ec.service
                .sync()
                .name(S + "import#GeoData")
                .parameters(Object.assign({}, set, extra || {}))
                .call() || {}
        const error = ec.message.getErrorsString() || ""
        ec.message.clearErrors()
        const row = {
            dataSetId: set.dataSetId,
            seconds: (Date.now() - started) / 1000,
            statusId: result.statusId || null,
            edition: result.edition || null,
            read: result.rowsRead,
            created: result.rowsCreated,
            updated: result.rowsUpdated,
            unchanged: result.rowsUnchanged,
            retired: result.rowsRetired,
            rejected: result.rowsRejected,
            failure: result.failureText || error || null,
            memory: memory()
        }
        log(label + " " + set.dataSetId, row)
        out.push(row)
    }
    return out
}
// The postal lookup of every nth code in use, cold (the cache cleared) and then cached: milliseconds a lookup
function lookupTimes() {
    const codes = sys(() =>
        Array.from(
            ec.entity
                .find("moqui.basic.GeoPostalCode")
                .condition("countryGeoId", "USA")
                .condition("thruDate", null)
                .selectField("normalizedCode")
                .orderBy("normalizedCode")
                .list()
        )
    )
        .filter((row, index) => index % 40 === 0)
        .map(row => row.normalizedCode)
    const time = () => {
        let found = 0
        const started = Date.now()
        for (const code of codes) {
            const out =
                ec.service
                    .sync()
                    .name("org.moqui.maps.MapServices.get#PostalLocation")
                    .parameters({countryGeoId: "US", postalCode: code})
                    .call() || {}
            if ((out.matches || []).length) found++
        }
        return {lookups: codes.length, found, msEach: codes.length ? (Date.now() - started) / codes.length : null}
    }
    ec.cache.getCache("maps.postal.lookup").clear()
    const cold = time()
    const cached = time()
    return {cold, cached}
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        http.allowNetwork(HOSTS)
        sys(() => {
            for (const {dataSetId} of SETS) {
                const set = ec.entity.find("moqui.maps.GeoDataSet").condition({dataSetId}).one()
                acks[dataSetId] = {licenseAckUserId: set.licenseAckUserId, licenseAckDate: set.licenseAckDate}
                set.setAll({licenseAckUserId: ec.user.userId, licenseAckDate: ec.user.nowTimestamp}).update()
            }
        })
        log("start", {at: ec.l10n.format(ec.user.nowTimestamp, "yyyy-MM-dd HH:mm:ss"), memory: memory()})
    },
    cleanupSpec() {
        ec.message.clearErrors()
        http.reset()
        sys(() => {
            for (const dataSetId of Object.keys(acks))
                ec.entity.find("moqui.maps.GeoDataSet").condition({dataSetId}).one().setAll(acks[dataSetId]).update()
        })
        log("summary", results)
    }
})

feature("the US Geo data imports from the Census and GeoNames downloads, and the postal lookup reads it", {
    when() {
        const started = Date.now()
        results.first = importAll("first")
        results.firstSeconds = (Date.now() - started) / 1000
        results.inUse = inUse()
        results.lookup = lookupTimes()
        log("in use", results.inUse)
        log("lookup", results.lookup)
        return {runs: results.first, rows: results.inUse, lookup: results.lookup}
    },
    then({runs, rows, lookup}) {
        runs.every(run => run.statusId === "GdiCompleted")
        runs.every(run => run.rejected < run.read / 100)
        rows.postalCodes > 33000
        rows.points > rows.postalCodes
        rows.placeNames > 30000
        rows.countyLinks > 30000
        lookup.cold.found == lookup.cold.lookups
        lookup.cached.found == lookup.cold.lookups
    }
})

feature("a refresh changes nothing: conditional, an unchanged source ends as current; forced, every row is read and left alone", {
    when() {
        let started = Date.now()
        results.conditional = importAll("conditional")
        results.conditionalSeconds = (Date.now() - started) / 1000
        started = Date.now()
        results.forced = importAll("forced", {force: true})
        results.forcedSeconds = (Date.now() - started) / 1000
        results.inUseAfter = inUse()
        return {conditional: results.conditional, forced: results.forced, before: results.inUse, after: results.inUseAfter}
    },
    then({conditional, forced, before, after}) {
        conditional.every(run => ["GdiUnchanged", "GdiCompleted"].includes(run.statusId))
        conditional.every(run => !run.created && !run.updated && !run.retired)
        forced.every(run => run.statusId === "GdiCompleted")
        forced.every(run => run.created === 0 && run.updated === 0 && run.retired === 0)
        after.postalCodes == before.postalCodes
        after.points == before.points
        after.placeNames == before.placeNames
    }
})
