// GeoDataImportCancel (plans/archive/Maps.md §6.4, steps 4 and 5): a real import, a run of the GeoDataImport ServiceJob started as
// Maps › Configuration › Geo data starts one (start#GeoDataImport), of an operator's file of 20,000 New Zealand codes (ten
// batches of 2,000), cancelled while it loads. The batches it committed stay, whole batches only; nothing is retired (a code
// of the edition in use that the new file lacks stays in date); the run ends GdiCancelled; the postal lookup cache is
// cleared, the committed batches being in use. A rerun of the scope then completes: the codes loaded before are unchanged,
// the rest created, and only then the code the new edition lacks is retired. No outside request.
// Run when asked: ./moq accept framework --spec GeoDataImportCancel
const S = "org.moqui.maps.GeoDataServices."
const IMPORT = "moqui.maps.GeoDataImport"
const CACHE = "maps.postal.lookup"
const ROWS = 20000
const BATCH = 2000
const OLD = ["OLD001", "OLD002", "OLD003"]
let folder = null
let ack = null
let previousUser = null

const sys = run => {
    const was = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!was) ec.artifactExecution.enableAuthz()
    }
}
function invoke(name, params) {
    const result = ec.service.sync().name(name).parameters(params).call() || {}
    const error = ec.message.getErrorsString() || ""
    ec.message.clearErrors()
    return Object.assign({}, result, {error})
}
const run = geoDataImportId => sys(() => ec.entity.find(IMPORT).condition({geoDataImportId}).one())
const inProgress = status => ["GdiPending", "GdiDownloading", "GdiValidating", "GdiLoading"].includes(status)
// The run's row once `done` holds of it, or after `seconds`, whichever comes first. The run is a ServiceJob's, on a job
// worker, so the loop looks every 100 ms with sleep between looks: ec.factory.waitWorkerPoolEmpty waits for the service
// worker pool only, and returns at once while a job runs.
function waitFor(geoDataImportId, seconds, done) {
    const deadline = Date.now() + seconds * 1000
    let row = run(geoDataImportId)
    while (Date.now() < deadline && !done(row)) {
        sleep(100)
        row = run(geoDataImportId)
    }
    return row
}
const codesInDate = () =>
    sys(() =>
        Number(
            ec.entity
                .find("moqui.basic.GeoPostalCode")
                .condition({countryGeoId: "NZL", sourceKey: "OPERATOR"})
                .condition("thruDate", null)
                .count()
        )
    )
const thruOf = code => sys(() => ec.entity.find("moqui.basic.GeoPostalCode").condition({geoId: "NZL_Z_" + code}).one())?.thruDate
const header = "country,code,place,admin1,admin2,latitude,longitude,precision\n"
function writeFile(name, lines) {
    const location = `${folder}/${name}`
    sys(() => ec.resource.getLocationReference(location).putText(header + lines.join("\n") + "\n"))
    return location
}
function clean() {
    ec.message.clearErrors()
    // a run still going is stopped first: a Remove is refused while an import of its scope runs
    const running = () =>
        sys(() =>
            Array.from(ec.entity.find(IMPORT).condition({dataSetId: "OPERATOR_CSV", countryGeoId: "NZL"}).list()).filter(row =>
                inProgress(row.statusId)
            )
        )
    for (const row of running()) invoke(S + "cancel#GeoDataImport", {geoDataImportId: row.geoDataImportId})
    const deadline = Date.now() + 120000
    while (Date.now() < deadline && running().length) sleep(250)
    invoke(S + "remove#GeoData", {dataSetId: "OPERATOR_CSV", countryGeoId: "NZL"})
    sys(() => {
        ec.entity.find(IMPORT).condition({dataSetId: "OPERATOR_CSV", countryGeoId: "NZL"}).deleteAll()
        for (const name of ["old.csv", "NZ.csv"]) {
            const file = ec.resource.getLocationReference(`${folder}/${name}`)
            if (file.getExists()) file.delete()
        }
    })
}

spec({
    setupSpec() {
        previousUser = ec.user.username || null
        ec.user.internalLoginUser("john.doe")
        folder = `dbresource://moqui/maps/import/${ec.user.userId}/spec-cancel`
        sys(() => {
            const set = ec.entity.find("moqui.maps.GeoDataSet").condition({dataSetId: "OPERATOR_CSV"}).one()
            ack = {licenseAckUserId: set.licenseAckUserId, licenseAckDate: set.licenseAckDate}
            set.setAll({licenseAckUserId: ec.user.userId, licenseAckDate: ec.user.nowTimestamp}).update()
        })
        clean()
    },
    cleanupSpec() {
        clean()
        sys(() => ec.entity.find("moqui.maps.GeoDataSet").condition({dataSetId: "OPERATOR_CSV"}).one().setAll(ack).update())
        if (previousUser) ec.user.internalLoginUser(previousUser)
        else ec.user.logoutUser()
    }
})

let cancelled = null
let newFile = null

feature("a running import cancelled between batches keeps the batches it committed, retires nothing and clears the lookup cache", {
    when() {
        // the edition in use: three codes the new file does not have
        const old = writeFile(
            "old.csv",
            OLD.map((code, index) => `NZ,${code},Old place ${index},,,-41.${index},174.7,postal`)
        )
        const first = invoke(S + "import#GeoData", {dataSetId: "OPERATOR_CSV", countryGeoId: "NZL", location: old})
        const lines = []
        for (let index = 0; index < ROWS; index++)
            lines.push(`NZ,T${String(index).padStart(5, "0")},Place ${index},,,${(-46 + index / 4000).toFixed(5)},${(168 + index / 3000).toFixed(5)},postal`)
        newFile = writeFile("NZ.csv", lines)
        ec.cache.getCache(CACHE).put("SPEC|CANCEL", "[]")
        const started = invoke(S + "start#GeoDataImport", {dataSetId: "OPERATOR_CSV", countryGeoId: "NZL", location: newFile})
        // cancelled once its first batch is committed
        const loading = waitFor(started.geoDataImportId, 120, row => !inProgress(row?.statusId) || Number(row?.rowsCreated) >= BATCH)
        const cancel = invoke(S + "cancel#GeoDataImport", {geoDataImportId: started.geoDataImportId})
        cancelled = waitFor(started.geoDataImportId, 120, row => !inProgress(row?.statusId))
        ec.logger.warn("GeoDataImportCancel: cancelled " + JSON.stringify({
            createdWhenAsked: Number(loading.rowsCreated), created: Number(cancelled.rowsCreated), status: cancelled.statusId
        }))
        return {
            first,
            started,
            loading,
            cancel,
            ended: cancelled,
            inDate: codesInDate(),
            oldThru: OLD.map(thruOf),
            cached: ec.cache.getCache(CACHE).get("SPEC|CANCEL")
        }
    },
    then({first, started, loading, cancel, ended, inDate, oldThru, cached}) {
        first.statusId == "GdiCompleted"
        started.error == ""
        started.jobRunId != null
        loading.statusId == "GdiLoading"
        cancel.error == ""
        ended.statusId == "GdiCancelled"
        ended.cancelRequested == "Y"
        ended.rowsRetired == 0
        ended.rowsRejected == 0
        // whole batches, some and not all
        Number(ended.rowsCreated) >= BATCH
        Number(ended.rowsCreated) < ROWS
        Number(ended.rowsCreated) % BATCH == 0
        inDate == OLD.length + Number(ended.rowsCreated)
        oldThru.every(thru => thru == null)
        cached == null
    }
})

feature("a rerun of the cancelled scope completes, and only then retires what the new edition lacks", {
    when() {
        const started = invoke(S + "start#GeoDataImport", {dataSetId: "OPERATOR_CSV", countryGeoId: "NZL", location: newFile})
        const ended = waitFor(started.geoDataImportId, 300, row => !inProgress(row?.statusId))
        return {started, ended, before: Number(cancelled.rowsCreated), inDate: codesInDate(), oldThru: OLD.map(thruOf)}
    },
    then({started, ended, before, inDate, oldThru}) {
        started.error == ""
        ended.statusId == "GdiCompleted"
        Number(ended.rowsUnchanged) == before
        Number(ended.rowsCreated) == ROWS - before
        Number(ended.rowsRetired) == OLD.length
        inDate == ROWS
        oldThru.every(thru => thru != null)
    }
})
