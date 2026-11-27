// The Geo data importer (plans/archive/Maps.md §6.4): org.moqui.maps.GeoDataServices import#GeoData, cancel#GeoDataImport and
// remove#GeoData, with HttpFixtures answering for the Census ZCTA zip and the GeoNames US zip (built here with
// writeZip). A load writes the four rows a code with its leading zeros kept (the ZCTA's land area on its point), a bad row goes to the error file while the
// rest load; a re-import changes nothing and sends the edition's validators, a 304 ends the run as current; retiring
// happens only after a whole load (an incomplete download, a file far smaller than the edition in use and a cancelled
// run retire nothing), and never retires a code whose row was passed over for another fault; GeoNames adds its point
// and place names beside the Census point, which stays active, is removed alone, and after a Remove is fetched whole
// and revives a code its owner retired; an import never changes a user-confirmed point or a code made by hand, and a
// Remove end-dates a code something refers to; refusals with maps@external-requests false, for a non-administrator
// (also with authorization off, by the importer's own MAPS_ADMIN check), for a license not acknowledged and for an
// operator's file with no country or outside the allowed locations; a ServiceJob run with no user runs as its
// runAsUserId only when that user added themselves to the job; and a queue of every country starts its next run when one
// ends and is cancelled as a group; a GeoNames first-level code finds its division by its ISO code (the crosswalk); and a
// removal under way refuses an import's start and another Remove of its scope.
// Run: ./moq test framework --spec GeoDataImport
const S = "org.moqui.maps.GeoDataServices."
const IMPORT = "moqui.maps.GeoDataImport"
const SETS = ["CENSUS_ZCTA", "GEONAMES_POSTAL", "CENSUS_COUNTY"]
const ZCTA_URL = "https://www2.census.gov/geo/docs/maps-data/data/gazetteer/2026_Gazetteer/2026_Gaz_zcta_national.zip"
const ZCTA_MEMBER = "2026_Gaz_zcta_national.txt"
const GN_URL = "https://download.geonames.org/export/zip/US.zip"
const ROOT = "dbresource://moqui/maps/spec-geo"
const USER = "GEO_SPEC_USER"
const CODES = ["00601", "02134", "02135", "02138", "02139", "02140", "00599"]
const JOBS = ["SPEC_GEO_RUNAS_OTHER", "SPEC_GEO_RUNAS_SELF"]
const QUEUE_SET = "SPEC_GEO_QUEUE"
const DIVISION_SETS = ["GEONAMES_ADMIN1", "GEONAMES_ADMIN2"]
// the divisions the crosswalk feature makes, and the Mantle-style province it may make when Mantle's data is not loaded
const DIVISIONS = ["DEU_A1_01", "DEU_A1_02", "CAN_A2_01_4806", "CAN_A1_01"]
let madeAlberta = false
let queueJobRunId = null
const http = ec.factory.getTool("HttpFixtures")
// the data sets' license acknowledgements before the spec, put back after
const acks = {}

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
// A call's result, or its error text with the messages cleared
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
const one = (entity, fields) => sys(() => ec.entity.find(entity).condition(fields).one())
const all = (entity, fields) =>
    sys(() =>
        Array.from(
            ec.entity
                .find(entity)
                .condition(fields || {})
                .list()
        )
    )
// The files the spec writes, deleted when it ends
const written = []
// A zip of one member, its bytes
function zipOf(name, member, text) {
    const source = `${ROOT}/${name}/${member}`
    const target = `${ROOT}/${name}.zip`
    written.push(source, target)
    return sys(() => {
        ec.resource.getLocationReference(source).putText(text)
        ec.resource.writeZip(target, [{path: member, location: source}])
        return ec.resource.getLocationReference(target).openStream().readAllBytes()
    })
}
const zcta = rows => ["GEOID|ALAND|AWATER|ALAND_SQMI|AWATER_SQMI|INTPTLAT|INTPTLONG"].concat(rows).join("\n") + "\n"
const Z00601 = "00601|166836392|799292|64.416|0.309|18.180555|-66.749961"
const Z02134 = "02134|2335958|158963|0.902|0.061|42.357579|-71.128389"
const Z02135 = "02135|1|1|0|0|42.348|-71.152"
const Z02138 = "02138|1|1|0|0|42.38|-71.135"
const BAD = "2136|1|1|0|0|42.3|-71.1"
const answer = (url, body, extra) => http.add(Object.assign({url, body, times: 1, contentType: "application/zip"}, extra || {}))
const importZcta = extra => invoke(S + "import#GeoData", Object.assign({dataSetId: "CENSUS_ZCTA"}, extra || {}))
const thruOf = code => (one("moqui.basic.GeoPostalCode", {geoId: "USA_Z_" + code}) || {}).thruDate

function sweep() {
    sys(() => {
        const geoIds = CODES.map(code => "USA_Z_" + code)
        ec.entity.find("moqui.basic.GeoPostalCodePlace").condition("geoId", "in", geoIds).deleteAll()
        ec.entity.find("moqui.basic.GeoPointAssoc").condition("geoId", "in", geoIds).deleteAll()
        for (const geoId of geoIds) {
            ec.entity
                .find("moqui.basic.GeoPoint")
                .condition("geoPointId", "like", geoId + "_%")
                .deleteAll()
        }
        ec.entity.find("moqui.basic.GeoAssoc").condition("toGeoId", "in", geoIds).deleteAll()
        ec.entity.find("moqui.basic.GeoPostalCode").condition("geoId", "in", geoIds).deleteAll()
        ec.entity.find("moqui.basic.Geo").condition("geoId", "in", geoIds).deleteAll()
        for (const run of Array.from(ec.entity.find(IMPORT).condition("dataSetId", "in", SETS).list())) {
            if (run.errorFileLocation) written.push(run.errorFileLocation)
        }
        ec.entity
            .find(IMPORT)
            .condition("dataSetId", "in", SETS.concat(["OPERATOR_CSV", QUEUE_SET], DIVISION_SETS))
            .deleteAll()
        ec.entity.find("moqui.basic.GeoAssoc").condition("toGeoId", "in", DIVISIONS).deleteAll()
        ec.entity.find("moqui.basic.Geo").condition("geoId", "in", DIVISIONS).deleteAll()
        if (madeAlberta) {
            ec.entity.find("moqui.basic.GeoAssoc").condition({geoId: "CAN", toGeoId: "CAN_AB"}).deleteAll()
            ec.entity.find("moqui.basic.Geo").condition("geoId", "CAN_AB").deleteAll()
            madeAlberta = false
        }
        if (queueJobRunId) ec.entity.find("moqui.service.job.ServiceJobRun").condition("jobRunId", queueJobRunId).deleteAll()
        ec.entity.find("moqui.maps.GeoDataSet").condition("dataSetId", QUEUE_SET).deleteAll()
        for (const entity of ["ServiceJobUser", "ServiceJobParameter", "ServiceJobRun", "ServiceJobRunLock", "ServiceJob"])
            ec.entity
                .find("moqui.service.job." + entity)
                .condition("jobName", "in", JOBS)
                .deleteAll()
    })
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        http.enable()
        sweep()
        sys(() => {
            for (const dataSetId of ["CENSUS_ZCTA", "GEONAMES_POSTAL", "OPERATOR_CSV"].concat(DIVISION_SETS)) {
                const set = ec.entity.find("moqui.maps.GeoDataSet").condition({dataSetId}).one()
                acks[dataSetId] = {licenseAckUserId: set.licenseAckUserId, licenseAckDate: set.licenseAckDate}
                set.setAll({licenseAckUserId: ec.user.userId, licenseAckDate: ec.user.nowTimestamp}).update()
            }
            ec.entity
                .makeValue("moqui.security.UserAccount")
                .setAll({userId: USER, username: "geo.spec.user", userFullName: "Geo spec user"})
                .create()
        })
    },
    cleanupSpec() {
        ec.message.clearErrors()
        http.reset()
        ec.user.internalLoginUser("john.doe")
        invoke(S + "remove#GeoData", {dataSetId: "GEONAMES_POSTAL", countryGeoId: "USA"})
        invoke(S + "remove#GeoData", {dataSetId: "CENSUS_ZCTA"})
        invoke(S + "remove#GeoData", {dataSetId: "OPERATOR_CSV", countryGeoId: "USA"})
        sweep()
        sys(() => {
            for (const dataSetId of Object.keys(acks)) {
                ec.entity.find("moqui.maps.GeoDataSet").condition({dataSetId}).one().setAll(acks[dataSetId]).update()
            }
            ec.entity.find("moqui.security.UserAccount").condition("userId", USER).deleteAll()
        })
        sys(() => {
            for (const location of written) {
                const file = ec.resource.getLocationReference(location)
                if (file.getExists()) file.delete()
            }
        })
        ec.message.clearErrors()
    }
})

feature("a Census ZCTA load writes four rows a code with leading zeros kept, and a bad row goes to the error file", {
    when() {
        answer(ZCTA_URL, zipOf("zcta1", ZCTA_MEMBER, zcta([Z00601, Z02134, Z02135, BAD, Z02138])), {
            headers: {ETag: '"zcta-1"', "Last-Modified": "Wed, 01 Apr 2026 00:00:00 GMT"}
        })
        const result = importZcta()
        return {
            result,
            geo: one("moqui.basic.Geo", {geoId: "USA_Z_00601"}),
            code: one("moqui.basic.GeoPostalCode", {geoId: "USA_Z_00601"}),
            point: one("moqui.basic.GeoPoint", {geoPointId: "USA_Z_00601_CENSUS_ZCTA"}),
            assoc: one("moqui.basic.GeoPointAssoc", {geoId: "USA_Z_00601", geoPointId: "USA_Z_00601_CENSUS_ZCTA"}),
            run: one(IMPORT, {geoDataImportId: result.geoDataImportId}),
            errorText: result.errorFileLocation ? sys(() => ec.resource.getLocationText(result.errorFileLocation)) : ""
        }
    },
    then({result, geo, code, point, assoc, run, errorText}) {
        result.error == ""
        result.statusId == "GdiCompleted"
        result.rowsRead == 5
        result.rowsCreated == 4
        result.rowsRejected == 1
        result.edition == "2026"
        result.errors[0].line == 5
        result.errors[0].field == "GEOID"
        geo.geoTypeEnumId == "GEOT_POSTAL_CODE"
        code.countryGeoId == "USA"
        code.normalizedCode == "00601"
        code.displayCode == "00601"
        code.granularityEnumId == "GPG_FULL"
        code.sourceKey == "CENSUS_ZCTA"
        point.latitude == 18.180555
        point.geoPointTypeEnumId == "GPT_GEO_REFERENCE"
        point.dataSourceId == "CENSUS_ZCTA"
        assoc.precisionEnumId == "GPP_POSTAL_AREA"
        assoc.pointRoleEnumId == "GPR_REPRESENTATIVE"
        assoc.priority == 10
        assoc.edition == "2026"
        // the ZCTA's land area, for the lookup's extent
        assoc.landArea == 166836392
        run.statusId == "GdiCompleted"
        run.sourceUrl == ZCTA_URL
        run.sourceEtag == '"zcta-1"'
        run.checksum.length == 64
        run.rowsCreated == 4
        errorText.includes("2136|1|1")
        errorText.includes("five-digit")
    }
})

feature("a re-import changes nothing and asks conditionally, and an unchanged source ends the run as current", {
    when() {
        answer(ZCTA_URL, zipOf("zcta1", ZCTA_MEMBER, zcta([Z00601, Z02134, Z02135, BAD, Z02138])))
        const again = importZcta()
        const asked = http.last().headers
        answer(ZCTA_URL, "", {status: 304})
        const current = importZcta()
        return {again, asked, current}
    },
    then({again, asked, current}) {
        again.statusId == "GdiCompleted"
        again.rowsUnchanged == 4
        again.rowsCreated == 0
        again.rowsUpdated == 0
        again.rowsRetired == 0
        asked["If-None-Match"] == '"zcta-1"'
        asked["If-Modified-Since"] == "Wed, 01 Apr 2026 00:00:00 GMT"
        current.statusId == "GdiUnchanged"
        current.rowsCreated == 0
    }
})

feature("retiring happens only after a whole load", {
    when() {
        const smaller = zipOf("zcta2", ZCTA_MEMBER, zcta([Z00601, Z02134, Z02135]))
        // an incomplete download: the zip cut short
        answer(ZCTA_URL, smaller.slice(0, smaller.length - 40))
        const cut = importZcta({force: true})
        // far fewer rows than the edition in use: refused unless forced
        answer(ZCTA_URL, smaller)
        const fewer = importZcta()
        // a cancelled run: a pending run cancelled before it starts never runs
        const pendingId = sys(() => {
            const pending = ec.entity
                .makeValue(IMPORT)
                .setAll({dataSetId: "CENSUS_ZCTA", countryGeoId: "USA", statusId: "GdiPending"})
                .setSequencedIdPrimary()
            pending.create()
            return pending.geoDataImportId
        })
        const cancel = invoke(S + "cancel#GeoDataImport", {geoDataImportId: pendingId})
        const cancelledRun = importZcta({geoDataImportId: pendingId})
        const before = {cut: thruOf("02138"), fewer: thruOf("02138")}
        // a whole load, forced: 02138 is no longer in the edition; 02135's row is passed over for its latitude, but its
        // code is still in the edition
        answer(ZCTA_URL, zipOf("zcta3", ZCTA_MEMBER, zcta([Z00601, Z02134, "02135|1|1|0|0|95|-71.152"])))
        const forced = importZcta({force: true})
        return {
            cut,
            fewer,
            cancel,
            cancelledRun,
            before,
            forced,
            retired: thruOf("02138"),
            kept: thruOf("02134"),
            passedOver: thruOf("02135")
        }
    },
    then({cut, fewer, cancel, cancelledRun, before, forced, retired, kept, passedOver}) {
        cut.statusId == "GdiFailed"
        cut.failureText.includes("does not read")
        fewer.statusId == "GdiFailed"
        fewer.failureText.includes("far fewer")
        cancel.statusId == "GdiCancelled"
        cancelledRun.error.includes("not pending")
        before.cut == null
        before.fewer == null
        forced.statusId == "GdiCompleted"
        forced.rowsRetired == 1
        forced.rowsUnchanged == 2
        forced.rowsRejected == 1
        forced.errors[0].field == "latitude"
        retired != null
        kept == null
        passedOver == null
    }
})

feature("GeoNames adds its point and place names beside the Census point, which stays active, and is removed alone", {
    when() {
        const rows = [
            "US\t02134\tAllston\tMassachusetts\tMA\tSuffolk\t025\t\t\t42.3539\t-71.1337\t4",
            "US\t02134\tBoston\tMassachusetts\tMA\tSuffolk\t025\t\t\t42.358\t-71.06\t6",
            "US\t02139\tCambridge\tMassachusetts\tMA\tMiddlesex\t017\t\t\t42.3647\t-71.1042\t4",
            "US\t0213X\tNowhere\tMassachusetts\tMA\t\t\t\t\t42.3\t-71.1\t1"
        ].join("\n")
        answer(GN_URL, zipOf("gn", "US.txt", rows + "\n"), {headers: {ETag: '"gn-1"'}})
        const result = invoke(S + "import#GeoData", {dataSetId: "GEONAMES_POSTAL", countryGeoId: "USA"})
        const points = all("moqui.basic.GeoPointAssoc", {geoId: "USA_Z_02134"})
            .sort((a, b) => a.priority - b.priority)
            .map(row => ({point: row.geoPointId, priority: row.priority, precision: row.precisionEnumId}))
        const places = all("moqui.basic.GeoPostalCodePlace", {geoId: "USA_Z_02134"})
            .sort((a, b) => a.sequenceNum - b.sequenceNum)
            .map(row => ({name: row.placeName, sequenceNum: row.sequenceNum, point: row.geoPointId, accuracy: row.accuracy}))
        const owner = one("moqui.basic.GeoPostalCode", {geoId: "USA_Z_02134"}).sourceKey
        const added = one("moqui.basic.GeoPostalCode", {geoId: "USA_Z_02139"})
        const removed = invoke(S + "remove#GeoData", {dataSetId: "GEONAMES_POSTAL", countryGeoId: "USA"})
        const after = {
            gone: one("moqui.basic.Geo", {geoId: "USA_Z_02139"}),
            kept: one("moqui.basic.Geo", {geoId: "USA_Z_02134"}),
            points: all("moqui.basic.GeoPointAssoc", {geoId: "USA_Z_02134"}).map(row => row.geoPointId),
            places: all("moqui.basic.GeoPostalCodePlace", {geoId: "USA_Z_02134"}).length
        }
        // after a Remove the source has no rows: fetched whole, not answered "unchanged"; and 02138, which Census
        // retired, is revived by GeoNames having it, Census still its owner
        const revived = rows + "\nUS\t02138\tCambridge\tMassachusetts\tMA\tMiddlesex\t017\t\t\t42.38\t-71.135\t4\n"
        answer(GN_URL, zipOf("gn2", "US.txt", revived))
        const again = invoke(S + "import#GeoData", {dataSetId: "GEONAMES_POSTAL", countryGeoId: "USA"})
        return {
            again,
            asked: http.last().headers,
            revived: thruOf("02138"),
            reviveOwner: one("moqui.basic.GeoPostalCode", {geoId: "USA_Z_02138"}).sourceKey,
            result,
            points,
            places,
            owner,
            added,
            removed,
            after
        }
    },
    then({again, asked, revived, reviveOwner, result, points, places, owner, added, removed, after}) {
        result.statusId == "GdiCompleted"
        result.rowsRead == 4
        result.rowsRejected == 1
        result.errors[0].field == "postalCode"
        points.length == 2
        points[0].point == "USA_Z_02134_CENSUS_ZCTA"
        points[1].point == "USA_Z_02134_GN_POSTAL"
        points[1].priority == 20
        points[1].precision == "GPP_POSTAL_AREA"
        places.length == 2
        places[0].name == "Allston"
        places[0].point == "USA_Z_02134_GN_POSTAL_1"
        places[0].accuracy == 4
        places[1].name == "Boston"
        places[1].point == "USA_Z_02134_GN_POSTAL"
        owner == "CENSUS_ZCTA"
        added.sourceKey == "GN_POSTAL"
        removed.error == ""
        after.gone == null
        after.kept != null
        after.points.length == 1
        after.points[0] == "USA_Z_02134_CENSUS_ZCTA"
        after.places == 0
        again.statusId == "GdiCompleted"
        again.rowsCreated == 3
        asked["If-None-Match"] == null
        revived == null
        reviveOwner == "CENSUS_ZCTA"
    }
})

feature("an import never changes a user-confirmed point or a code made by hand, and a Remove end-dates a code in use", {
    when() {
        sys(() => {
            ec.entity
                .find("moqui.basic.GeoPointAssoc")
                .condition({geoId: "USA_Z_00601", geoPointId: "USA_Z_00601_CENSUS_ZCTA"})
                .one()
                .setAll({precisionEnumId: "GPP_USER_CONFIRMED"})
                .update()
            ec.entity
                .makeValue("moqui.basic.Geo")
                .setAll({geoId: "USA_Z_02140", geoTypeEnumId: "GEOT_POSTAL_CODE", geoName: "By hand"})
                .create()
            ec.entity
                .makeValue("moqui.basic.GeoPostalCode")
                .setAll({
                    geoId: "USA_Z_02140",
                    countryGeoId: "USA",
                    normalizedCode: "02140",
                    displayCode: "02140 (hand)",
                    granularityEnumId: "GPG_FULL"
                })
                .create()
        })
        const moved = "00601|166836392|799292|64.416|0.309|10.5|-66.749961"
        answer(ZCTA_URL, zipOf("zcta4", ZCTA_MEMBER, zcta([moved, Z02134, Z02135, "02140|1|1|0|0|42.39|-71.13"])))
        const result = importZcta({force: true})
        const confirmed = {
            point: one("moqui.basic.GeoPoint", {geoPointId: "USA_Z_00601_CENSUS_ZCTA"}),
            assoc: one("moqui.basic.GeoPointAssoc", {geoId: "USA_Z_00601", geoPointId: "USA_Z_00601_CENSUS_ZCTA"})
        }
        const hand = {
            geo: one("moqui.basic.Geo", {geoId: "USA_Z_02140"}),
            code: one("moqui.basic.GeoPostalCode", {geoId: "USA_Z_02140"}),
            point: one("moqui.basic.GeoPoint", {geoPointId: "USA_Z_02140_CENSUS_ZCTA"})
        }
        // something refers to 02135: a group membership
        sys(() =>
            ec.entity
                .makeValue("moqui.basic.GeoAssoc")
                .setAll({geoId: "USA", toGeoId: "USA_Z_02135", geoAssocTypeEnumId: "GAT_GROUP_MEMBER"})
                .create()
        )
        const removed = invoke(S + "remove#GeoData", {dataSetId: "CENSUS_ZCTA"})
        return {
            result,
            confirmed,
            hand,
            removed,
            inUse: {
                geo: one("moqui.basic.Geo", {geoId: "USA_Z_02135"}),
                thru: thruOf("02135")
            },
            handAfter: one("moqui.basic.GeoPostalCode", {geoId: "USA_Z_02140"}),
            confirmedAfter: one("moqui.basic.GeoPointAssoc", {geoId: "USA_Z_00601", geoPointId: "USA_Z_00601_CENSUS_ZCTA"})
        }
    },
    then({result, confirmed, hand, removed, inUse, handAfter, confirmedAfter}) {
        result.statusId == "GdiCompleted"
        confirmed.point.latitude == 18.180555
        confirmed.assoc.precisionEnumId == "GPP_USER_CONFIRMED"
        hand.geo.geoName == "By hand"
        hand.code.sourceKey == null
        hand.code.displayCode == "02140 (hand)"
        hand.point.latitude == 42.39
        removed.error == ""
        removed.rowsRetired >= 1
        inUse.geo != null
        inUse.thru != null
        handAfter.sourceKey == null
        handAfter.thruDate == null
        confirmedAfter.precisionEnumId == "GPP_USER_CONFIRMED"
    }
})

feature("an import is refused with outside requests off, for a non-administrator, and before its license is acknowledged", {
    when() {
        const runs = () => all(IMPORT, {dataSetId: "CENSUS_ZCTA"}).length
        const before = runs()
        const off = withConf("maps@external-requests", "false", () => importZcta())
        const license = invoke(S + "import#GeoData", {dataSetId: "CENSUS_COUNTY"})
        const noCountry = invoke(S + "import#GeoData", {dataSetId: "OPERATOR_CSV", location: "runtime://import/z.csv"})
        const outside = invoke(S + "import#GeoData", {
            dataSetId: "OPERATOR_CSV",
            countryGeoId: "USA",
            location: "component://framework/data/z.csv"
        })
        ec.user.internalLoginUser("geo.spec.user")
        const user = importZcta()
        // with authorization off the service runs, and the importer's own check refuses
        const userNoAuthz = sys(() => importZcta())
        ec.user.internalLoginUser("john.doe")
        return {off, license, noCountry, outside, user, userNoAuthz, made: runs() - before}
    },
    then({off, license, noCountry, outside, user, userNoAuthz, made}) {
        off.error.includes("maps@external-requests")
        off.geoDataImportId == null
        license.error.includes("has not been acknowledged")
        noCountry.error.includes("name the countryGeoId")
        outside.error.includes("Not available")
        user.error != ""
        user.statusId == null
        userNoAuthz.error.includes("MAPS_ADMIN")
        userNoAuthz.statusId == null
        made == 0
    }
})

feature("a ServiceJob run with no user runs as its runAsUserId only when that user added themselves to the job", {
    when() {
        const me = ec.user.userId
        const location = `dbresource://moqui/maps/import/${me}/spec-runas/US.csv`
        written.push(location)
        sys(() => {
            ec.resource
                .getLocationReference(location)
                .putText("country,code,place,admin1,admin2,latitude,longitude,precision\nUS,00599,Spec,NY,,40.8,-73.0,postal\n")
            for (const jobName of JOBS) {
                ec.entity
                    .makeValue("moqui.service.job.ServiceJob")
                    .setAll({jobName, serviceName: S + "import#GeoData", transactionTimeout: 600})
                    .create()
                const parameters = {dataSetId: "OPERATOR_CSV", countryGeoId: "USA", location, runAsUserId: me}
                for (const [parameterName, parameterValue] of Object.entries(parameters))
                    ec.entity
                        .makeValue("moqui.service.job.ServiceJobParameter")
                        .setAll({jobName, parameterName, parameterValue})
                        .create()
            }
        })
        // the job user row is added by another user to one job, and by the run-as user to the other
        ec.user.internalLoginUser("geo.spec.user")
        sys(() => ec.entity.makeValue("moqui.service.job.ServiceJobUser").setAll({jobName: JOBS[0], userId: me}).create())
        ec.user.internalLoginUser("john.doe")
        sys(() => ec.entity.makeValue("moqui.service.job.ServiceJobUser").setAll({jobName: JOBS[1], userId: me}).create())
        ec.user.logoutUser()
        // as a scheduler starts a job: no user, authorization off
        const runIds = sys(() => JOBS.map(jobName => ec.service.job(jobName).run()))
        ec.user.internalLoginUser("john.doe")
        const ended = id =>
            sys(() =>
                ec.entity
                    .find("moqui.service.job.ServiceJobRun")
                    .condition("jobRunId", id || "_NA_")
                    .one()
            )
        const deadline = Date.now() + 10000
        // each run on a job worker: look every 50 ms
        while (Date.now() < deadline && !runIds.every(id => ended(id)?.endTime)) sleep(50)
        const [other, self] = runIds.map(ended)
        return {other, self, imported: all(IMPORT, {dataSetId: "OPERATOR_CSV"}), me}
    },
    then({other, self, imported, me}) {
        other.hasError == "Y"
        other.errors.includes("who added themselves")
        self.hasError != "Y"
        imported.length == 1
        imported[0].statusId == "GdiCompleted"
        imported[0].userId == me
        imported[0].jobRunId == self.jobRunId
    }
})

feature("every country is a queue: a run that ends starts the queue's next pending run, and a cancel of the queue stops the rest", {
    when() {
        const run = (geoDataImportId, fields) =>
            ec.entity
                .makeValue(IMPORT)
                .setAll(Object.assign({geoDataImportId, fromDate: ec.user.nowTimestamp, userId: ec.user.userId}, fields))
                .create()
        sys(() => {
            // a disabled set: the chained run is refused at its preflight, downloading nothing
            ec.entity
                .makeValue("moqui.maps.GeoDataSet")
                .setAll({dataSetId: QUEUE_SET, sourceKey: "SPEC_GQ", parserEnumId: "GDP_GEONAMES", yieldsEnumId: "GDY_POSTAL_POINTS",
                    perCountry: "Y", disabled: "Y", urlTemplate: "https://download.geonames.org/export/zip/{country}.zip"})
                .create()
            const queued = {dataSetId: QUEUE_SET, statusId: "GdiPending", queueId: "SPEC_GQ_1"}
            run("SPEC_GQ_1", Object.assign({countryGeoId: "USA", cancelRequested: "Y"}, queued))
            run("SPEC_GQ_2", Object.assign({countryGeoId: "CAN"}, queued))
            // a queue with one running and one waiting
            const other = {dataSetId: QUEUE_SET, queueId: "SPEC_GQ_3"}
            run("SPEC_GQ_3", Object.assign({countryGeoId: "MEX", statusId: "GdiLoading"}, other))
            run("SPEC_GQ_4", Object.assign({countryGeoId: "DEU", statusId: "GdiPending"}, other))
        })
        // the first was cancelled before it started: it ends, and starts the next
        const first = invoke(S + "import#GeoData", {geoDataImportId: "SPEC_GQ_1"})
        // the next run is the GeoDataImport job's, on a job worker: wait (at most 10 s) for it to end
        const going = () =>
            ["GdiPending", "GdiDownloading", "GdiValidating", "GdiLoading"].includes(one(IMPORT, {geoDataImportId: "SPEC_GQ_2"})?.statusId)
        const deadline = Date.now() + 10000
        while (Date.now() < deadline && going()) sleep(50)
        const cancel = invoke(S + "cancel#GeoDataImport", {queueId: "SPEC_GQ_3"})
        const again = invoke(S + "cancel#GeoDataImport", {queueId: "SPEC_GQ_1"})
        const row = geoDataImportId => one(IMPORT, {geoDataImportId})
        const q2 = row("SPEC_GQ_2")
        queueJobRunId = q2?.jobRunId || null
        return {first, cancel, again, q1: row("SPEC_GQ_1"), q2, q3: row("SPEC_GQ_3"), q4: row("SPEC_GQ_4")}
    },
    then({first, cancel, again, q1, q2, q3, q4}) {
        first.error.includes("cancelled before it started")
        q1.statusId == "GdiCancelled"
        // the next ran as a run of the GeoDataImport job, and was refused there: the set is disabled
        q2.jobRunId != null
        q2.statusId == "GdiFailed"
        q2.failureText.includes("disabled")
        // the queue cancelled as a group: the waiting run at once, the running one asked to stop
        cancel.error == ""
        cancel.cancelledCount == 2
        q4.statusId == "GdiCancelled"
        q3.cancelRequested == "Y"
        q3.statusId == "GdiLoading"
        again.error.includes("no run in progress")
    }
})

feature("a GeoNames first-level code finds its division by its ISO 3166-2 code through the crosswalk", {
    when() {
        sys(() => {
            // Mantle's Alberta, as its install data has it, unless it is loaded already
            if (!ec.entity.find("moqui.basic.Geo").condition("geoId", "CAN_AB").one()) {
                madeAlberta = true
                ec.entity
                    .makeValue("moqui.basic.Geo")
                    .setAll({geoId: "CAN_AB", geoTypeEnumId: "GEOT_PROVINCE", geoName: "Alberta", geoCodeAlpha2: "AB"})
                    .create()
            }
            ec.entity
                .makeValue("moqui.basic.GeoAssoc")
                .setAll({geoId: "CAN", toGeoId: "CAN_AB", geoAssocTypeEnumId: "GAT_REGIONS"})
                .createOrUpdate()
            // a division an earlier import made, with no ISO code then
            ec.entity.makeValue("moqui.basic.Geo").setAll({geoId: "DEU_A1_01", geoTypeEnumId: "GEOT_STATE", geoName: "Baden-Wuerttemberg"}).create()
            ec.entity
                .makeValue("moqui.basic.GeoAssoc")
                .setAll({geoId: "DEU", toGeoId: "DEU_A1_01", geoAssocTypeEnumId: "GAT_REGIONS"})
                .create()
        })
        const text = {contentType: "text/plain"}
        answer(
            "https://download.geonames.org/export/dump/admin1CodesASCII.txt",
            "CA.01\tAlberta\tAlberta\t5883102\nDE.01\tBaden-Württemberg\tBaden-Wurttemberg\t2953481\nDE.02\tBavaria\tBavaria\t2951839\n",
            text
        )
        answer("https://download.geonames.org/export/dump/admin2Codes.txt", "CA.01.4806\tDivision No. 6\tDivision No. 6\t5964347\n", text)
        const admin1 = invoke(S + "import#GeoData", {dataSetId: "GEONAMES_ADMIN1"})
        const admin2 = invoke(S + "import#GeoData", {dataSetId: "GEONAMES_ADMIN2"})
        return {
            admin1,
            admin2,
            alberta: one("moqui.basic.GeoAssoc", {geoId: "CAN_AB", toGeoId: "CAN_A2_01_4806"}),
            fipsMade: one("moqui.basic.Geo", {geoId: "CAN_A1_01"}),
            bavaria: one("moqui.basic.Geo", {geoId: "DEU_A1_02"}),
            backfilled: one("moqui.basic.Geo", {geoId: "DEU_A1_01"})
        }
    },
    then({admin1, admin2, alberta, fipsMade, bavaria, backfilled}) {
        admin1.statusId == "GdiCompleted"
        admin2.statusId == "GdiCompleted"
        // CA.01 is ISO CA-AB: Mantle's Alberta, not a second one
        alberta != null
        fipsMade == null
        // a division made gets its ISO code, never the FIPS code
        bavaria.geoCodeAlpha2 == "BY"
        bavaria.geoName == "Bavaria"
        // one made before gets it now
        backfilled.geoCodeAlpha2 == "BW"
    }
})

feature("a removal under way refuses an import's start and another Remove of its scope, and not another country's", {
    when() {
        const mark = fields => sys(() => ec.entity.find("moqui.maps.GeoDataSet").condition({dataSetId: "OPERATOR_CSV"}).one().setAll(fields).update())
        // as remove#GeoData marks the set while it removes the US rows, a batch at a time
        mark({removingStamp: ec.user.nowTimestamp, removingCountryGeoId: "USA"})
        try {
            return {
                start: invoke(S + "start#GeoDataImport", {dataSetId: "OPERATOR_CSV", countryGeoId: "USA", location: ROOT + "/none.csv"}),
                remove: invoke(S + "remove#GeoData", {dataSetId: "OPERATOR_CSV", countryGeoId: "USA"}),
                otherCountry: invoke(S + "remove#GeoData", {dataSetId: "OPERATOR_CSV", countryGeoId: "CAN"})
            }
        } finally {
            mark({removingStamp: null, removingCountryGeoId: null})
        }
    },
    then({start, remove, otherCountry}) {
        start.error.includes("being removed")
        start.geoDataImportId == null
        remove.error.includes("being removed")
        otherCountry.error == ""
    }
})
