// GeoDataEveryCountry (plans/archive/Maps.md §6.3 "Every country", §6.4): an import of every country of a set read a country at a
// time, as Geo data's Import starts one (start#GeoDataImport with everyCountry), with outside requests (opened to
// download.geonames.org alone): a set as an administrator adds one for a mirror, sharing GeoNames postal's source key, whose
// countryCodes are two small countries and the largest GeoNames country file between them (Brazil, Portugal's 206,942
// rows, Canada's first parts). The queue runs the three one after another, each its own run and edition of the
// GeoDataImport ServiceJob, the one after Portugal showing the queue goes on past a large country. It records each run's
// wall time and counts and the rows in use after it, logged as GeoDataEveryCountry lines for plans/archive/Maps.md §6.3 (the
// server's memory is sampled beside the run). The wait looks at the runs every half second, pausing with sleep between
// looks (ec.factory.waitWorkerPoolEmpty waits for the service worker pool only, not a job's run). Cleanup cancels what is
// still running and removes what it loaded: the set shares GeoNames postal's source key, so that is the GEONAMES_POSTAL
// data of these countries, which a test database has none of before it.
// Run when asked: ./moq accept framework --spec GeoDataEveryCountry (about four minutes, most of it Portugal and its removal; a few MB downloaded)
const S = "org.moqui.maps.GeoDataServices."
const IMPORT = "moqui.maps.GeoDataImport"
const SET = "SPEC_GN_FEW"
const CODES = ["BR", "PT", "CA"]
// queued, in the order of the set's countryCodes
const QUEUED = ["BRA", "PRT", "CAN"]
const http = ec.factory.getTool("HttpFixtures")
const results = {}
let queueId = null
let previousUser = null

const sys = run => {
    const was = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!was) ec.artifactExecution.enableAuthz()
    }
}
const log = (label, value) => ec.logger.warn("GeoDataEveryCountry: " + label + " " + JSON.stringify(value))
function invoke(name, params) {
    const result = ec.service.sync().name(name).parameters(params).call() || {}
    const error = ec.message.getErrorsString() || ""
    ec.message.clearErrors()
    return Object.assign({}, result, {error})
}
const inProgress = status => ["GdiPending", "GdiDownloading", "GdiValidating", "GdiLoading"].includes(status)
const queueRuns = queueId =>
    sys(() => Array.from(ec.entity.find(IMPORT).condition({queueId}).list())).sort(
        (a, b) => a.geoDataImportId.length - b.geoDataImportId.length || a.geoDataImportId.localeCompare(b.geoDataImportId)
    )
const count = (entity, build) => sys(() => Number(build(ec.entity.find(entity)).count()))
const countries = () =>
    sys(() =>
        Array.from(
            ec.entity.find("moqui.basic.Geo").condition("geoTypeEnumId", "GEOT_COUNTRY").condition("geoCodeAlpha2", "in", CODES).list()
        )
    )

spec({
    setupSpec() {
        previousUser = ec.user.username || null
        ec.user.internalLoginUser("john.doe")
        http.allowNetwork(["https://download.geonames.org/"])
        sys(() => {
            const original = ec.entity.find("moqui.maps.GeoDataSet").condition({dataSetId: "GEONAMES_POSTAL"}).one()
            ec.entity
                .makeValue("moqui.maps.GeoDataSet")
                .setAll({
                    dataSetId: SET,
                    sourceKey: original.sourceKey,
                    publisher: original.publisher,
                    parserEnumId: original.parserEnumId,
                    yieldsEnumId: original.yieldsEnumId,
                    perCountry: "Y",
                    countryCodes: CODES.join(","),
                    urlTemplate: original.urlTemplate,
                    archiveMember: original.archiveMember,
                    priority: original.priority,
                    dataSourceId: original.dataSourceId,
                    licenseName: original.licenseName,
                    attribution: original.attribution,
                    licenseAckUserId: ec.user.userId,
                    licenseAckDate: ec.user.nowTimestamp
                })
                .createOrUpdate()
        })
    },
    cleanupSpec() {
        ec.message.clearErrors()
        // what is still running is stopped first: a Remove is refused while an import of its scope runs
        if (queueId) {
            invoke(S + "cancel#GeoDataImport", {queueId})
            const deadline = Date.now() + 120000
            while (Date.now() < deadline && queueRuns(queueId).some(run => inProgress(run.statusId))) sleep(500)
        }
        const started = Date.now()
        for (const country of countries()) invoke(S + "remove#GeoData", {dataSetId: SET, countryGeoId: country.geoId})
        log("removed", {seconds: (Date.now() - started) / 1000})
        http.reset()
        sys(() => {
            ec.entity.find(IMPORT).condition({dataSetId: SET}).deleteAll()
            ec.entity.find("moqui.maps.GeoDataSet").condition({dataSetId: SET}).deleteAll()
        })
        log("summary", results)
        if (previousUser) ec.user.internalLoginUser(previousUser)
        else ec.user.logoutUser()
    }
})

feature("every country runs as a queue, one after another, each its own completed run", {
    when() {
        const estimate = ec.resource
            .script("component://framework/service/org/moqui/maps/GeoData.js")
            .execute("everyCountryEstimate", {dataSetId: SET})
        const startedAt = Date.now()
        const started = invoke(S + "start#GeoDataImport", {dataSetId: SET, everyCountry: true})
        queueId = started.queueId || null
        const deadline = Date.now() + 15 * 60 * 1000
        let runs = queueRuns(started.queueId || "_NA_")
        let lastLog = 0
        while (Date.now() < deadline && runs.some(run => inProgress(run.statusId))) {
            sleep(500)
            runs = queueRuns(started.queueId || "_NA_")
            if (Date.now() - lastLog > 30000) {
                lastLog = Date.now()
                log("progress", runs.map(run => run.countryGeoId + " " + run.statusId + " " + (run.rowsCreated || 0)))
            }
        }
        results.seconds = (Date.now() - startedAt) / 1000
        results.runs = runs.map(run => {
            const prefix = run.countryGeoId + "_"
            return {
                country: run.countryGeoId,
                status: run.statusId,
                seconds: (Number(run.thruDate) - Number(run.fromDate)) / 1000,
                read: Number(run.rowsRead),
                created: Number(run.rowsCreated),
                rejected: Number(run.rowsRejected),
                failure: run.failureText || null,
                postalCodes: count("moqui.basic.GeoPostalCode", find =>
                    find.condition("countryGeoId", run.countryGeoId).condition("thruDate", null)
                ),
                points: count("moqui.basic.GeoPointAssoc", find => find.condition("geoId", "like", prefix + "%").condition("thruDate", null)),
                placeNames: count("moqui.basic.GeoPostalCodePlace", find =>
                    find.condition("geoId", "like", prefix + "%").condition("thruDate", null)
                )
            }
        })
        results.estimate = estimate
        for (const run of results.runs) log("run " + run.country, run)
        return {estimate, started, runs}
    },
    then({estimate, started, runs}) {
        estimate.countryCount == QUEUED.length
        estimate.estimateText.includes("3 countries")
        started.error == ""
        started.countryCount == QUEUED.length
        runs.length == QUEUED.length
        runs.map(run => run.countryGeoId).join(",") == QUEUED.join(",")
        runs.every(run => run.statusId === "GdiCompleted")
        runs.every(run => run.queueId === started.queueId && run.jobRunId)
        new Set(runs.map(run => run.jobRunId)).size == QUEUED.length
        // one at a time: each started after the one before it ended
        runs.every((run, index) => index === 0 || Number(run.fromDate) >= Number(runs[index - 1].thruDate))
    }
})
