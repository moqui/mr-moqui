// Maps › Configuration › Geo data (plans/archive/Maps.md §6.4; docs/tools/Maps.md "Geo data"), a plain XML screen rendered once (html):
// for an administrator its data sets with their licenses and Import, and refused to a Maps user (MAPS_ADMIN denies
// MAPS_USERS); its Import dialog (GeoData/ImportDialog.xml) offers GeoNames' countries only; Acknowledge license records who, and Import starts a run of the GeoDataImport ServiceJob with its pending run,
// never the import in the request (an operator's file here, so no download).
// Run: ./moq test tools --spec MapsGeoData
const USER = "MAPS_GEO_SCR_USER"
// under the signed-in administrator's own folder of the Maps import folder, the only one of it an import reads
let CSV = null
let operatorAck = null

const sys = run => {
    const was = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!was) ec.artifactExecution.enableAuthz()
    }
}
// Whether test() holds within millis, looked at every 50 ms
function waitFor(test, millis) {
    const deadline = Date.now() + millis
    while (Date.now() < deadline) {
        if (test()) return true
        sleep(50)
    }
    return test()
}
const runOf = id =>
    sys(() =>
        ec.entity
            .find("moqui.maps.GeoDataImport")
            .condition("geoDataImportId", id || "_NA_")
            .one()
    )
const ended = id => !["GdiPending", "GdiDownloading", "GdiValidating", "GdiLoading"].includes(runOf(id)?.statusId)
let startedRunId = null

function render(username, path, parameters) {
    ec.user.internalLoginUser(username)
    try {
        const res = ec.screen
            .makeTest()
            .rootScreen("component://webroot/screen/webroot.xml")
            .renderMode("html")
            .render(path, parameters || {}, null)
        return {status: res.status, errors: res.errorMessages.join(" | "), output: String(res.output || "")}
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
    }
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        CSV = `dbresource://moqui/maps/import/${ec.user.userId}/spec-screen/US.csv`
        sys(() => {
            ec.entity
                .makeValue("moqui.security.UserAccount")
                .setAll({userId: USER, username: "maps.geo.scr.user", userFullName: "Geo data screen spec"})
                .createOrUpdate()
            ec.entity
                .makeValue("moqui.security.UserGroupMember")
                .setAll({userGroupId: "MAPS_USERS", userId: USER, fromDate: ec.user.nowTimestamp})
                .createOrUpdate()
            const operator = ec.entity.find("moqui.maps.GeoDataSet").condition("dataSetId", "OPERATOR_CSV").one()
            operatorAck = {licenseAckUserId: operator.licenseAckUserId, licenseAckDate: operator.licenseAckDate}
            operator.setAll({licenseAckUserId: null, licenseAckDate: null}).update()
            ec.resource
                .getLocationReference(CSV)
                .putText(
                    "country,code,place,admin1,admin2,latitude,longitude,precision\nUS,00503,Spec Place,NY,,40.8,-73.0,postal\n"
                )
        })
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        // the job's run ends before its rows are removed
        if (startedRunId) waitFor(() => ended(startedRunId), 10000)
        ec.service
            .sync()
            .name("org.moqui.maps.GeoDataServices.remove#GeoData")
            .parameters({dataSetId: "OPERATOR_CSV", countryGeoId: "USA"})
            .call()
        ec.message.clearErrors()
        sys(() => {
            ec.entity.find("moqui.maps.GeoDataImport").condition("dataSetId", "OPERATOR_CSV").deleteAll()
            ec.entity.find("moqui.maps.GeoDataSet").condition("dataSetId", "OPERATOR_CSV").one().setAll(operatorAck).update()
            ec.entity.find("moqui.security.UserGroupMember").condition("userId", USER).deleteAll()
            ec.entity.find("moqui.security.UserAccount").condition("userId", USER).deleteAll()
            const file = ec.resource.getLocationReference(CSV)
            if (file.getExists()) file.delete()
        })
    }
})

feature("Geo data renders its data sets, licenses and Import for an administrator, and is refused to a Maps user", {
    when() {
        return {
            admin: render("john.doe", "/apps/maps/Configuration/GeoData"),
            user: render("maps.geo.scr.user", "/apps/maps/Configuration/GeoData"),
            // the Import dialog, loaded when opened: GeoNames' countries only, and every country with its estimate
            dialog: render("john.doe", "/apps/maps/Configuration/GeoData/ImportDialog", {
                dataSetId: "GEONAMES_POSTAL",
                lastStandalone: "true"
            })
        }
    },
    then({admin, user, dialog}) {
        admin.status == 200
        !/Error rendering|Exception in render/.test(admin.output)
        admin.output.includes("Data sets")
        admin.output.includes("US Census Bureau · CENSUS_ZCTA")
        admin.output.includes("CC BY 4.0")
        admin.output.includes("Acknowledge license")
        admin.output.includes("Import file")
        admin.output.includes("Refresh schedule")
        user.status == 403 || /not authorized/i.test(user.errors)
        !user.output.includes("CENSUS_ZCTA")
        dialog.status == 200
        !/Error rendering|Exception in render/.test(dialog.output)
        dialog.output.includes("Start import")
        dialog.output.includes("Morocco")
        !dialog.output.includes("Antarctica")
        // every country, as a queue, with its estimate
        dialog.output.includes("Import every country")
        /Imports 1\d\d countries one after another/.test(dialog.output)
    }
})

feature("Acknowledge license records who, and Import starts a GeoDataImport job run with its pending run", {
    when() {
        const call = (name, parameters) => {
            const out =
                ec.service
                    .sync()
                    .name("org.moqui.maps.GeoDataServices." + name)
                    .parameters(parameters)
                    .call() || {}
            const error = ec.message.getErrorsString() || ""
            ec.message.clearErrors()
            return Object.assign({}, out, {error})
        }
        const refused = call("start#GeoDataImport", {dataSetId: "OPERATOR_CSV", countryGeoId: "US", location: CSV})
        call("acknowledge#GeoDataLicense", {dataSetId: "OPERATOR_CSV"})
        const ack = sys(
            () => ec.entity.find("moqui.maps.GeoDataSet").condition("dataSetId", "OPERATOR_CSV").one().licenseAckUserId
        )
        const started = call("start#GeoDataImport", {dataSetId: "OPERATOR_CSV", countryGeoId: "US", location: CSV})
        startedRunId = started.geoDataImportId
        const run = runOf(started.geoDataImportId)
        const jobRun = sys(() =>
            ec.entity
                .find("moqui.service.job.ServiceJobRun")
                .condition("jobRunId", started.jobRunId || "_NA_")
                .one()
        )
        // the job, on a worker, takes the pending run and imports the file
        const finished = waitFor(() => ended(started.geoDataImportId), 10000) ? runOf(started.geoDataImportId) : {}
        return {refused, ack, started, run, jobRun, finished}
    },
    then({refused, ack, started, run, jobRun, finished}) {
        refused.error.includes("Acknowledge the license")
        refused.geoDataImportId == null
        ack == ec.user.userId
        started.error == ""
        started.jobRunId != null
        jobRun.jobName == "GeoDataImport"
        run.countryGeoId == "USA"
        run.userId == ec.user.userId
        run.jobRunId == started.jobRunId
        run.sourceLocation == CSV
        finished.statusId == "GdiCompleted"
        finished.rowsCreated == 1
    }
})
