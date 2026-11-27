// The local postal lookup (plans/archive/Maps.md §6.1, §6.3): org.moqui.maps.MapServices.get#PostalLocation over a few postal codes
// written here as an import writes them (Geo, GeoPostalCode, GeoPoint, GeoPointAssoc, GeoPostalCodePlace). A US ZIP keeps its
// leading zero and is found by the country's alpha-2, alpha-3 or Geo ID, and as a ZIP+4; the Census point is the active one
// before GeoNames', approximate, with its source and an extent sized from the ZCTA's land area; a Canadian code is found by its spaced or unspaced form,
// and a full code with no row falls back to its declared prefix at a lower precision, while a US code is never cut; a code
// with several places is ambiguous; Browse's search reads a country named in the text and lists a code's places as choices;
// an unknown code or country finds nothing; a user-confirmed point or a record's own
// location is never a postal representative; any signed-in user may look up (MAPS_DIALOG); a record whose adapter gives
// only an address is shown at its postal area (get#EffectiveConfig); the cache is cleared when an import (an operator's
// file) finishes; and the maps.geodata readiness checks (check#GeoDataReadiness) say the default country has postal data and
// name a scope whose last import failed, for a holder of ADMIN_OVERVIEW_VIEW only.
// Run: ./moq test framework --spec MapsPostalLookup
const LOOKUP = "org.moqui.maps.MapServices.get#PostalLocation"
const ADAPTER = "MAPS_POSTAL_SPEC"
const GRANT = "MAPS_POSTAL_SPEC_GRANT"
const USER = "MAPS_POSTAL_USER"
// under the signed-in administrator's own folder of the Maps import folder, the only one of it an import reads
let CSV = null
const GEO_IDS = ["USA_Z_00501", "USA_Z_05501", "USA_Z_00544", "USA_Z_00545", "CAN_Z_K1A0B1", "CAN_ZP_M5V", "USA_Z_00502"]
let operatorAck = null

const sys = run => {
    const was = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!was) ec.artifactExecution.enableAuthz()
    }
}
const make = (entityName, fields) => ec.entity.makeValue(entityName).setAll(fields).createOrUpdate()
const lookup = (countryGeoId, postalCode) => {
    const out = ec.service.sync().name(LOOKUP).parameters({countryGeoId, postalCode}).call() || {}
    const error = ec.message.getErrorsString() || ""
    ec.message.clearErrors()
    return Object.assign({}, out, {error})
}
// A postal code as an import writes it: its points (each {key, lat, lon, set, priority, precision, type, landArea}) and places
function code(geoId, countryGeoId, normalizedCode, displayCode, granularityEnumId, points, places) {
    make("moqui.basic.Geo", {geoId, geoTypeEnumId: "GEOT_POSTAL_CODE", geoName: displayCode})
    make("moqui.basic.GeoPostalCode", {
        geoId,
        countryGeoId,
        normalizedCode,
        displayCode,
        granularityEnumId,
        sourceKey: "CENSUS_ZCTA"
    })
    for (const point of points) {
        const geoPointId = geoId + "_" + point.key
        make("moqui.basic.GeoPoint", {
            geoPointId,
            geoPointTypeEnumId: point.type || "GPT_GEO_REFERENCE",
            latitude: point.lat,
            longitude: point.lon
        })
        make("moqui.basic.GeoPointAssoc", {
            geoId,
            geoPointId,
            pointRoleEnumId: "GPR_REPRESENTATIVE",
            precisionEnumId: point.precision || "GPP_POSTAL_AREA",
            dataSetId: point.set,
            edition: "2026",
            priority: point.priority,
            landArea: point.landArea || null,
            fromDate: ec.user.nowTimestamp
        })
    }
    for (const [index, place] of (places || []).entries()) {
        const geoPointId = place.lat == null ? null : geoId + "_GN_POSTAL_" + (index + 1)
        if (geoPointId)
            make("moqui.basic.GeoPoint", {
                geoPointId,
                geoPointTypeEnumId: "GPT_GEO_REFERENCE",
                latitude: place.lat,
                longitude: place.lon
            })
        make("moqui.basic.GeoPostalCodePlace", {
            geoId,
            sourceKey: "GN_POSTAL",
            placeName: place.name,
            sequenceNum: index + 1,
            adminName1: "MA",
            geoPointId
        })
    }
}
function sweep() {
    sys(() => {
        ec.entity.find("moqui.basic.GeoPostalCodePlace").condition("geoId", "in", GEO_IDS).deleteAll()
        ec.entity.find("moqui.basic.GeoPointAssoc").condition("geoId", "in", GEO_IDS).deleteAll()
        for (const geoId of GEO_IDS)
            ec.entity
                .find("moqui.basic.GeoPoint")
                .condition("geoPointId", "like", geoId + "_%")
                .deleteAll()
        ec.entity.find("moqui.basic.GeoPostalCode").condition("geoId", "in", GEO_IDS).deleteAll()
        ec.entity.find("moqui.basic.Geo").condition("geoId", "in", GEO_IDS).deleteAll()
        ec.entity.find("moqui.maps.GeoDataImport").condition("dataSetId", "OPERATOR_CSV").deleteAll()
        ec.entity.find("moqui.maps.GeoDataImport").condition("failureText", "Spec failure").deleteAll()
        ec.cache.getCache("maps.postal.lookup").clear()
    })
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        CSV = `dbresource://moqui/maps/import/${ec.user.userId}/spec-postal/US.csv`
        sweep()
        sys(() => {
            // a ZCTA of 1,000 km² of land: its extent is a square of that area
            const census = [{key: "CENSUS_ZCTA", lat: 40.81, lon: -73.04, set: "CENSUS_ZCTA", priority: 10, landArea: 1000000000}]
            code(
                "USA_Z_00501",
                "USA",
                "00501",
                "00501",
                "GPG_FULL",
                census.concat([{key: "GN_POSTAL", lat: 40.92, lon: -72.63, set: "GEONAMES_POSTAL", priority: 20}])
            )
            code(
                "USA_Z_05501",
                "USA",
                "05501",
                "05501",
                "GPG_FULL",
                [{key: "GN_POSTAL", lat: 42.65, lon: -71.14, set: "GEONAMES_POSTAL", priority: 20}],
                [
                    {name: "Andover", lat: 42.65, lon: -71.14},
                    {name: "Lawrence", lat: 42.7, lon: -71.16}
                ]
            )
            // a user-confirmed point and a record's own location, ahead of the reference point by priority
            code("USA_Z_00544", "USA", "00544", "00544", "GPG_FULL", [
                {key: "CONFIRMED", lat: 1, lon: 1, set: "CENSUS_ZCTA", priority: 1, precision: "GPP_USER_CONFIRMED"},
                {key: "RECORD", lat: 2, lon: 2, set: "CENSUS_ZCTA", priority: 2, type: "GPT_RECORD_LOCATION"},
                {key: "GN_POSTAL", lat: 40.82, lon: -73.05, set: "GEONAMES_POSTAL", priority: 20}
            ])
            code("USA_Z_00545", "USA", "00545", "00545", "GPG_FULL", [
                {key: "CONFIRMED", lat: 1, lon: 1, set: "CENSUS_ZCTA", priority: 1, precision: "GPP_USER_CONFIRMED"}
            ])
            code("CAN_Z_K1A0B1", "CAN", "K1A0B1", "K1A 0B1", "GPG_FULL", [
                {key: "OPERATOR", lat: 45.42, lon: -75.7, set: "OPERATOR_CSV", priority: 5}
            ])
            code("CAN_ZP_M5V", "CAN", "M5V", "M5V", "GPG_PREFIX", [
                {key: "GN_POSTAL", lat: 43.64, lon: -79.39, set: "GEONAMES_POSTAL", priority: 20, precision: "GPP_PLACE_ESTIMATE"}
            ])
            make("moqui.security.UserAccount", {
                userId: USER,
                username: "maps.postal.user",
                userFullName: "Postal lookup spec user"
            })
            // the framework's test adapter, granted here as a component grants its own
            make("moqui.maps.MapAdapter", {
                adapterKey: ADAPTER,
                description: "Spec addresses",
                contextServiceName: "org.moqui.mapstest.MapsTestServices.get#RecordMapContext"
            })
            make("moqui.security.ArtifactGroup", {artifactGroupId: GRANT, description: "Postal lookup spec adapter"})
            make("moqui.security.ArtifactGroupMember", {
                artifactGroupId: GRANT,
                artifactName: "org.moqui.mapstest.MapsTestServices.get#RecordMapContext",
                artifactTypeEnumId: "AT_SERVICE",
                inheritAuthz: "N"
            })
            make("moqui.security.ArtifactAuthz", {
                artifactAuthzId: GRANT,
                userGroupId: "ALL_USERS",
                artifactGroupId: GRANT,
                authzTypeEnumId: "AUTHZT_ALLOW",
                authzActionEnumId: "AUTHZA_ALL"
            })
            const operator = ec.entity.find("moqui.maps.GeoDataSet").condition("dataSetId", "OPERATOR_CSV").one()
            operatorAck = {licenseAckUserId: operator.licenseAckUserId, licenseAckDate: operator.licenseAckDate}
            operator.setAll({licenseAckUserId: ec.user.userId, licenseAckDate: ec.user.nowTimestamp}).update()
        })
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.service
            .sync()
            .name("org.moqui.maps.GeoDataServices.remove#GeoData")
            .parameters({dataSetId: "OPERATOR_CSV", countryGeoId: "USA"})
            .call()
        ec.message.clearErrors()
        sweep()
        sys(() => {
            ec.entity.find("moqui.maps.GeoDataSet").condition("dataSetId", "OPERATOR_CSV").one().setAll(operatorAck).update()
            ec.entity.find("moqui.security.ArtifactAuthz").condition("artifactAuthzId", GRANT).deleteAll()
            ec.entity.find("moqui.security.ArtifactGroupMember").condition("artifactGroupId", GRANT).deleteAll()
            ec.entity.find("moqui.security.ArtifactGroup").condition("artifactGroupId", GRANT).deleteAll()
            ec.entity.find("moqui.maps.MapAdapter").condition("adapterKey", ADAPTER).deleteAll()
            ec.entity.find("moqui.security.UserAccount").condition("userId", USER).deleteAll()
            const file = ec.resource.getLocationReference(CSV)
            if (file.getExists()) file.delete()
        })
    }
})

feature(
    "a US ZIP keeps its leading zero, is found by alpha-2, alpha-3, Geo ID and as a ZIP+4, and gives the Census point, approximate",
    {
        when() {
            // any signed-in user, authorization on: the lookup is MAPS_DIALOG's
            ec.user.internalLoginUser("maps.postal.user")
            try {
                return {
                    alpha2: lookup("US", "00501"),
                    alpha3: lookup("usa", " 00501 "),
                    geoId: lookup("USA", "00501"),
                    zip4: lookup("US", "00501-1234"),
                    territory: lookup("PR", "00501"),
                    cut: lookup("US", "501"),
                    // the cache holds data with no words of a language: the precision is put in words when read
                    cached: JSON.parse(String(ec.cache.getCache("maps.postal.lookup").get("USA|00501|GPG_FULL") || "[]"))
                }
            } finally {
                ec.user.internalLoginUser("john.doe")
            }
        },
        then({alpha2, alpha3, geoId, zip4, territory, cut, cached}) {
            alpha2.error == ""
            alpha2.matches.length == 1
            alpha2.matches[0].geoId == "USA_Z_00501"
            alpha2.matches[0].displayCode == "00501"
            alpha2.matches[0].lat == 40.81
            alpha2.matches[0].lon == -73.04
            alpha2.matches[0].dataSetId == "CENSUS_ZCTA"
            alpha2.matches[0].precisionEnumId == "GPP_POSTAL_AREA"
            alpha2.matches[0].precisionText == "Approximate location — postal area"
            alpha2.matches[0].sourceText == "US Census Gazetteer: ZCTAs, 2026"
            alpha2.matches[0].attribution.includes("Census")
            alpha2.matches[0].extent[0] < -73.1
            alpha2.matches[0].extent[2] > -73.0
            alpha2.matches[0].extent[1] < 40.75
            alpha2.matches[0].extent[3] > 40.85
            // sized from the land area (a square of 31.6 km), not the fixed box of 0.16°
            Math.abs(alpha2.matches[0].extent[3] - alpha2.matches[0].extent[1] - 0.284) < 0.002
            alpha2.ambiguous == false
            alpha3.matches[0].geoId == "USA_Z_00501"
            geoId.matches[0].geoId == "USA_Z_00501"
            zip4.zipPlus4 == true
            zip4.matches[0].geoId == "USA_Z_00501"
            territory.matches[0].geoId == "USA_Z_00501"
            cached.length == 1
            cached[0].precisionText == null
            cut.matches.length == 0
            cut.reasonCode == "CODE_INVALID"
        }
    }
)

feature(
    "a Canadian code is found spaced or not, and a full code with no row falls back to its declared prefix at a lower precision",
    {
        when() {
            return {
                full: lookup("CA", "k1a0b1"),
                spaced: lookup("CAN", "K1A 0B1"),
                prefix: lookup("CA", "M5V 3L9"),
                alone: lookup("CA", "M5V")
            }
        },
        then({full, spaced, prefix, alone}) {
            full.matches[0].displayCode == "K1A 0B1"
            full.matches[0].matchKind == "code"
            full.matches[0].dataSetId == "OPERATOR_CSV"
            spaced.matches[0].geoId == "CAN_Z_K1A0B1"
            prefix.matches.length == 1
            prefix.matches[0].geoId == "CAN_ZP_M5V"
            prefix.matches[0].matchKind == "prefix"
            prefix.matches[0].granularityEnumId == "GPG_PREFIX"
            prefix.matches[0].precisionEnumId == "GPP_PLACE_ESTIMATE"
            prefix.matches[0].precisionText.includes("only the first part of the code")
            prefix.matches[0].extent[3] - prefix.matches[0].extent[1] > full.matches[0].extent[3] - full.matches[0].extent[1]
            alone.matches[0].matchKind == "code"
            alone.matches[0].precisionEnumId == "GPP_PLACE_ESTIMATE"
            alone.matches[0].precisionText.includes("only the first part of the code")
        }
    }
)

feature("a code with several places is ambiguous, and an unknown code or country finds nothing", {
    when() {
        return {several: lookup("US", "05501"), unknown: lookup("US", "99999"), country: lookup("ZZ", "00501")}
    },
    then({several, unknown, country}) {
        several.ambiguous == true
        several.matches[0].placeNames.length == 2
        several.matches[0].placeNames[0].name == "Andover"
        several.matches[0].placeNames[1].lat == 42.7
        several.matches[0].extent[3] > 42.7
        unknown.matches.length == 0
        unknown.reasonCode == "NO_MATCH"
        country.reasonCode == "COUNTRY_UNKNOWN"
    }
})

feature(
    "Browse's search: a named country with postal data that reads the code, else the code inferred; a code's places as choices",
    {
        when() {
            const search = (q, country) =>
                ec.resource
                    .script("component://framework/service/org/moqui/maps/MapLookup.js")
                    .execute("searchPostal", {q, country})
            return {
                leading: search("CA K1A 0B1"),
                trailing: search("05501, US"),
                chosen: search("k1a0b1", "CAN"),
                // Morocco has no postal data here, and Canada's rules do not read 00501: the US code, inferred
                morocco: search("MA 00501"),
                canada: search("CA 00501"),
                inferred: search("00501"),
                words: search("Main Street")
            }
        },
        then({leading, trailing, chosen, morocco, canada, inferred, words}) {
            leading.countryGeoId == "CAN"
            leading.results[0].kind == "postal"
            leading.results[0].label == "K1A 0B1"
            trailing.results.length == 3
            trailing.results[0].label == "05501 — Andover, Lawrence"
            trailing.results[1].kind == "postalPlace"
            trailing.results[1].precisionText == "Approximate location — postal area"
            chosen.results[0].id == "postal:CAN_Z_K1A0B1"
            morocco.countryGeoId == "USA"
            morocco.results[0].id == "postal:USA_Z_00501"
            canada.results[0].id == "postal:USA_Z_00501"
            inferred.countryGeoId == "USA"
            inferred.ambiguous == false
            inferred.countries.length == 2
            words.results.length == 0
        }
    }
)

feature("a user-confirmed point or a record's own location is never a postal representative", {
    when() {
        return {passed: lookup("US", "00544"), none: lookup("US", "00545")}
    },
    then({passed, none}) {
        passed.matches.length == 1
        passed.matches[0].lat == 40.82
        passed.matches[0].dataSetId == "GEONAMES_POSTAL"
        none.matches.length == 0
        none.reasonCode == "NO_MATCH"
    }
})

feature("a record whose adapter gives only an address is shown at its postal area, approximate, with a broad extent", {
    when() {
        ec.user.internalLoginUser("maps.postal.user")
        try {
            const out = ec.service
                .sync()
                .name("org.moqui.maps.MapServices.get#EffectiveConfig")
                .parameters({adapterKey: ADAPTER, recordKey: {recordId: "ADDRESS_ONLY"}, recordLayers: false})
                .call()
            const error = ec.message.getErrorsString() || ""
            ec.message.clearErrors()
            return {config: out.config || {}, error}
        } finally {
            ec.user.internalLoginUser("john.doe")
        }
    },
    then({config, error}) {
        error == ""
        config.points.length == 1
        config.points[0].id == "record"
        config.points[0].lat == 40.81
        config.points[0].precision == "Approximate location — postal area"
        config.record.approximate == true
        config.record.precisionEnumId == "GPP_POSTAL_AREA"
        config.record.sourceText.includes("2026")
        config.extent[0] < -73.1
        config.extent[2] > -73.0
    }
})

feature("the lookup cache is cleared when an import finishes: a code an operator's file adds is found at once", {
    when() {
        const before = lookup("US", "00502")
        sys(() =>
            ec.resource
                .getLocationReference(CSV)
                .putText(
                    "country,code,place,admin1,admin2,latitude,longitude,precision\nUS,00502,Holtsville,NY,,40.8152,-73.0455,postal\n"
                )
        )
        const imported =
            ec.service
                .sync()
                .name("org.moqui.maps.GeoDataServices.import#GeoData")
                .parameters({dataSetId: "OPERATOR_CSV", countryGeoId: "USA", location: CSV})
                .call() || {}
        const importError = ec.message.getErrorsString() || ""
        ec.message.clearErrors()
        return {before, imported, importError, after: lookup("US", "00502")}
    },
    then({before, imported, importError, after}) {
        before.reasonCode == "NO_MATCH"
        importError == ""
        imported.statusId == "GdiCompleted"
        after.matches.length == 1
        after.matches[0].lat == 40.8152
        after.matches[0].dataSetId == "OPERATOR_CSV"
        after.matches[0].placeNames[0].name == "Holtsville"
    }
})

feature(
    "readiness: postal data for the default country, a scope whose last import failed, and a reader without the permission refused",
    {
        when() {
            // Canada (in use: a completed run, rows) whose last run failed; Germany, which never completed, is not named
            const run = (countryGeoId, statusId, daysAgo) =>
                sys(() =>
                    ec.entity
                        .makeValue("moqui.maps.GeoDataImport")
                        .setAll({
                            dataSetId: "GEONAMES_POSTAL",
                            countryGeoId,
                            statusId,
                            failureText: "Spec failure",
                            fromDate: new Date(Date.now() - daysAgo * 86400000)
                        })
                        .setSequencedIdPrimary()
                        .create()
                )
            run("CAN", "GdiCompleted", 2)
            run("CAN", "GdiFailed", 1)
            run("DEU", "GdiFailed", 1)
            const check = () => {
                const out =
                    ec.service
                        .sync()
                        .name("org.moqui.maps.MapServices.check#GeoDataReadiness")
                        .parameters({providerKey: "maps.geodata"})
                        .call() || {}
                const error = ec.message.getErrorsString() || ""
                ec.message.clearErrors()
                return {checks: Array.from(out.checks || []), error}
            }
            const admin = check()
            ec.user.internalLoginUser("maps.postal.user")
            try {
                return {admin, plain: check()}
            } finally {
                ec.user.internalLoginUser("john.doe")
            }
        },
        then({admin, plain}) {
            admin.error == ""
            admin.checks.find(check => check.checkKey === "maps.geodata.default").state == "ready"
            admin.checks.find(check => check.checkKey === "maps.geodata.imports").reasonCode == "IMPORT_FAILED"
            admin.checks.find(check => check.checkKey === "maps.geodata.imports").message.includes("GEONAMES_POSTAL (Canada)")
            !admin.checks.find(check => check.checkKey === "maps.geodata.imports").message.includes("Germany")
            plain.error.includes("PERMISSION_DENIED")
            plain.checks.length == 0
        }
    }
)
