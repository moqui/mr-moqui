// MapsOvertureAddresses (plans/Maps-Overture.md §O2): Overture's Addresses theme imported for a region through the O1 run
// (HttpFixtures answer for a small STAC catalog and serve test/fixtures/overture/addresses-r*.parquet by range, goldens of
// the moqui-maps crate's tests), and the local geocoder (MPK_GEOCODER_OVERTURE) over its index. A non-holder's import is
// refused; a holder's first run waits for review of the source licenses its acknowledgement does not cover and is active
// once they are acknowledged. The geocoder passes its test (a fixed query of its own data) and is enabled with outside
// requests off, and a search sends nothing: an address with its postal code is an address match, a number written twice
// 100 m apart two choices, a number and street in another city and postal code, a number found without its suffix and a
// street without the number estimates, and an address it does not have the postal area of its code (approximate), else
// nothing. A record's address with no point is matched by it (for a user with no Maps app too), its choices offered, never a
// match in another city, else its postal area, nothing saved. A refresh is activated as a whole: a moved address at its new
// point, a removed one gone, what changed counted by address, the run before kept for rollback, and a user-confirmed point
// at a removed address untouched.
// Run: ./moq test framework --spec MapsOvertureAddresses
const S = "org.moqui.maps.OvertureServices."
const C = "org.moqui.admin.ConfigurationServices."
const IMPORT = "moqui.maps.OvertureImport"
const ADDRESS = "moqui.maps.OvertureAddress"
const STAC = "https://stac.example.test"
const CATALOG = STAC + "/catalog.json"
const DATA = "https://data.example.test/release/"
const R1 = "2026-10-21.0"
const R2 = "2026-11-18.0"
const USER = "OVT_ADDR_USER"
// a user of no group but ALL_USERS: no Maps app, so no find#Places; the local-map dialog is everyone's
const PLAIN = "OVT_ADDR_PLAIN"
const PROVIDER = "OVT_ADDR_SPEC"
const ADAPTER = "MAPS_OVT_ADDR_SPEC"
const GRANT = "MAPS_OVT_ADDR_GRANT"
const POSTAL = "ITA_Z_50123"
const CONFIRMED = "OVT_ADDR_SPEC_CONFIRMED"
const REGION = {regionName: "Spec Florence addresses", west: 11.24, south: 43.76, east: 11.27, north: 43.78}
const http = ec.factory.getTool("HttpFixtures")
let theme = null
let regionId = null

function sys(run) {
    const was = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!was) ec.artifactExecution.enableAuthz()
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
const make = (entity, fields) => ec.entity.makeValue(entity).setAll(fields).createOrUpdate()
const one = (entity, fields) => sys(() => ec.entity.find(entity).condition(fields).one())
const count = (entity, fields) => sys(() => Number(ec.entity.find(entity).condition(fields).count()))
const fixture = name =>
    ec.resource
        .getLocationReference("component://framework/test/fixtures/overture/" + name)
        .openStream()
        .readAllBytes()
const activeOf = () => one("moqui.maps.OvertureRegionTheme", {regionId, themeId: "ADDRESSES"})?.activeImportId || null
// find#Places as a Maps user who is no administrator
function search(query, extra) {
    ec.user.internalLoginUser("ovt.addr.user")
    ec.artifactExecution.enableAuthz()
    try {
        return invoke("org.moqui.maps.MapServices.find#Places", Object.assign({query, providerId: PROVIDER}, extra || {}))
    } finally {
        ec.user.internalLoginUser("john.doe")
    }
}
const recordContext = (recordId, username) => {
    if (username) {
        ec.user.internalLoginUser(username)
        ec.artifactExecution.enableAuthz()
    }
    try {
        return invoke("org.moqui.maps.MapServices.get#EffectiveConfig", {
            adapterKey: ADAPTER,
            recordKey: {recordId},
            recordLayers: false
        }).config
    } finally {
        if (username) ec.user.internalLoginUser("john.doe")
    }
}

// The catalog: two releases of schema 2, each with an addresses item over Florence
function serve() {
    http.clear()
    const json = (url, doc) => http.add({url, body: JSON.stringify(doc), contentType: "application/json"})
    json(CATALOG, {
        type: "Catalog",
        latest: R2,
        links: [R1, R2].map(version => ({rel: "child", href: `${STAC}/${version}/catalog.json`, title: version}))
    })
    for (const [version, file] of [
        [R1, "addresses-r1.parquet"],
        [R2, "addresses-r2.parquet"]
    ]) {
        const base = `${STAC}/${version}`
        json(base + "/catalog.json", {
            "release:version": version,
            "schema:version": "2.0.0",
            links: [{rel: "child", href: "./addresses/catalog.json", title: "addresses"}]
        })
        json(base + "/addresses/catalog.json", {links: [{rel: "child", href: "./address/collection.json", title: "address"}]})
        json(base + "/addresses/address/collection.json", {
            "schema:version": "2.0.0",
            extent: {
                spatial: {
                    bbox: [
                        [11.0, 41.0, 13.0, 44.0],
                        [11.0, 41.0, 13.0, 44.0]
                    ]
                }
            },
            links: [{rel: "item", href: "./00000/00000.json"}]
        })
        json(base + "/addresses/address/00000/00000.json", {
            bbox: [11.0, 41.0, 13.0, 44.0],
            assets: {aws: {href: `${DATA}${version}/theme=addresses/part-00000.parquet`, type: "application/vnd.apache.parquet"}}
        })
        http.add({
            url: `${DATA}${version}/theme=addresses/part-00000.parquet`,
            body: fixture(file),
            ranges: true,
            headers: {ETag: `"${version}-addresses"`}
        })
    }
}
function importRun(releaseVersion) {
    const started = invoke(S + "start#OvertureImport", {regionId, themeId: "ADDRESSES", releaseVersion})
    if (!started.importId) return {started, run: null}
    const deadline = Date.now() + 15000
    const going = () => ["OviPending", "OviRunning"].includes(one(IMPORT, {importId: started.importId})?.statusId)
    while (Date.now() < deadline && going()) sleep(50)
    return {started, run: one(IMPORT, {importId: started.importId})}
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        http.enable()
        setConf("maps@overture-catalog", CATALOG)
        setConf("maps@external-requests", "true")
        sys(() => {
            const row = ec.entity.find("moqui.maps.OvertureTheme").condition({themeId: "ADDRESSES"}).one()
            theme = {licenses: row.licenses, licenseAckUserId: row.licenseAckUserId, licenseAckDate: row.licenseAckDate}
            // acknowledged for CC0 sources only: the Italian source's CC BY is reviewed
            row.setAll({licenses: "CC0-1.0", licenseAckUserId: "EX_JOHN_DOE", licenseAckDate: ec.user.nowTimestamp}).update()
            const now = ec.user.nowTimestamp
            make("moqui.security.UserAccount", {userId: USER, username: "ovt.addr.user", userFullName: "Overture address user"})
            make("moqui.security.UserAccount", {userId: PLAIN, username: "ovt.addr.plain", userFullName: "Overture plain user"})
            make("moqui.security.UserGroupMember", {userGroupId: "MAPS_USERS", userId: USER, fromDate: now})
            // the local geocoder, enabled but not yet tested
            make("moqui.maps.MapProvider", {
                providerId: PROVIDER,
                providerName: "Spec Overture addresses",
                providerKindEnumId: "MPK_GEOCODER_OVERTURE",
                attribution: "Overture Maps Foundation",
                allowPrivateNetwork: "N",
                disabled: "N",
                sequenceNum: 1
            })
            // the postal code of the address the index lacks, as an import writes it
            make("moqui.basic.Geo", {geoId: POSTAL, geoTypeEnumId: "GEOT_POSTAL_CODE", geoName: "50123"})
            make("moqui.basic.GeoPostalCode", {
                geoId: POSTAL,
                countryGeoId: "ITA",
                normalizedCode: "50123",
                displayCode: "50123",
                granularityEnumId: "GPG_FULL",
                sourceKey: "GN_POSTAL"
            })
            make("moqui.basic.GeoPoint", {
                geoPointId: POSTAL + "_GN",
                geoPointTypeEnumId: "GPT_GEO_REFERENCE",
                latitude: 43.775,
                longitude: 11.25
            })
            make("moqui.basic.GeoPointAssoc", {
                geoId: POSTAL,
                geoPointId: POSTAL + "_GN",
                pointRoleEnumId: "GPR_REPRESENTATIVE",
                precisionEnumId: "GPP_POSTAL_AREA",
                dataSetId: "GEONAMES_POSTAL",
                edition: "2026",
                priority: 20,
                fromDate: now
            })
            // a user-confirmed point at the address release 2 removes (Via Maggio 15)
            make("moqui.basic.GeoPoint", {
                geoPointId: CONFIRMED,
                geoPointTypeEnumId: "GPT_GEO_REFERENCE",
                latitude: 43.7677,
                longitude: 11.2495
            })
            // the framework's test adapter, granted as a component grants its own
            make("moqui.maps.MapAdapter", {
                adapterKey: ADAPTER,
                description: "Spec Florence addresses",
                contextServiceName: "org.moqui.mapstest.MapsTestServices.get#RecordMapContext"
            })
            make("moqui.security.ArtifactGroup", {artifactGroupId: GRANT, description: "Overture address spec adapter"})
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
            ec.cache.getCache("maps.postal.lookup").clear()
            ec.cache.getCache("moqui.test.maps.stored").clear()
        })
        serve()
        regionId = invoke(S + "store#OvertureRegion", Object.assign({themeIds: ["ADDRESSES"]}, REGION)).regionId
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        if (regionId) invoke(S + "remove#OvertureTheme", {regionId})
        sys(() => {
            if (regionId) {
                const importIds = Array.from(ec.entity.find(IMPORT).condition({regionId}).list()).map(run => run.importId)
                if (importIds.length) ec.entity.find(ADDRESS).condition("importId", "in", importIds).deleteAll()
                ec.entity.find(IMPORT).condition({regionId}).deleteAll()
            }
            ec.entity.find("moqui.maps.OvertureTheme").condition({themeId: "ADDRESSES"}).one().setAll(theme).update()
            ec.entity.find("moqui.server.ConnectionCheck").condition("connectionKey", PROVIDER).deleteAll()
            ec.entity.find("moqui.maps.MapProvider").condition("providerId", PROVIDER).deleteAll()
            ec.entity.find("moqui.basic.GeoPointAssoc").condition("geoId", POSTAL).deleteAll()
            ec.entity
                .find("moqui.basic.GeoPoint")
                .condition("geoPointId", "in", [POSTAL + "_GN", CONFIRMED])
                .deleteAll()
            ec.entity.find("moqui.basic.GeoPostalCode").condition("geoId", POSTAL).deleteAll()
            ec.entity.find("moqui.basic.Geo").condition("geoId", POSTAL).deleteAll()
            ec.entity.find("moqui.security.ArtifactAuthz").condition("artifactAuthzId", GRANT).deleteAll()
            ec.entity.find("moqui.security.ArtifactGroupMember").condition("artifactGroupId", GRANT).deleteAll()
            ec.entity.find("moqui.security.ArtifactGroup").condition("artifactGroupId", GRANT).deleteAll()
            ec.entity.find("moqui.maps.MapAdapter").condition("adapterKey", ADAPTER).deleteAll()
            ec.entity.find("moqui.security.UserGroupMember").condition("userId", USER).deleteAll()
            ec.entity.find("moqui.security.UserAccount").condition("userId", "in", [USER, PLAIN]).deleteAll()
            ec.cache.getCache("maps.postal.lookup").clear()
        })
        http.reset()
        ec.message.clearErrors()
    }
})

feature(
    "an Addresses import is refused to a non-holder; a holder's first run waits for review of its sources' licenses and is active once they are acknowledged",
    {
        when() {
            ec.user.internalLoginUser("ovt.addr.user")
            const user = invoke(S + "start#OvertureImport", {regionId, themeId: "ADDRESSES"})
            ec.user.internalLoginUser("john.doe")
            const {run} = importRun(R1)
            const waiting = {active: activeOf(), staged: count(ADDRESS, {importId: run.importId})}
            const acknowledged = invoke(S + "acknowledge#OvertureLicense", {themeId: "ADDRESSES", licenses: ["CC-BY-4.0"]})
            const activated = invoke(S + "activate#OvertureImport", {importId: run.importId})
            return {user, run, waiting, acknowledged, activated, active: activeOf(), after: one(IMPORT, {importId: run.importId})}
        },
        then({user, run, waiting, acknowledged, activated, active, after}) {
            user.error.includes("PERMISSION_DENIED") || /not authorized/i.test(user.error)
            user.importId == null
            run.statusId == "OviReview"
            run.newLicenses == "CC-BY-4.0"
            run.rowsKept == 7
            run.rowsRejected == 1
            run.rowGroupsSkipped == 1
            run.countryCodes == "IT"
            waiting.active == null
            waiting.staged == 7
            acknowledged.error == ""
            activated.error == ""
            active == run.importId
            after.statusId == "OviActive"
        }
    }
)

feature(
    "the local geocoder passes its test by its own data with outside requests off; it answers an address match, choices, estimates (a postal code and city that differ among them) or the postal area, and nothing is sent",
    {
        when() {
            return withConf("maps@external-requests", "false", () => {
                const before = http.count()
                const tested = invoke(C + "test#Connection", {adapterKey: "mapProvider", connectionKey: PROVIDER})
                const out = {
                    tested,
                    exact: search("Piazza della Signoria 5, 50122 Firenze"),
                    elsewhere: search("Piazza della Signoria 5, 50199 Prato"),
                    both: search("Via de' Fossi 51, Firenze", {countryGeoId: "IT"}),
                    letter: search("Via dei Fossi 12/A, Firenze"),
                    red: search("Via dei Fossi 51r, Firenze"),
                    street: search("Via dei Fossi 999, Firenze"),
                    postal: search("Via Roma 1, 50123 Firenze"),
                    none: search("Via Roma 1, Firenze")
                }
                out.requests = http.count() - before
                return out
            })
        },
        then({tested, exact, elsewhere, both, letter, red, street, postal, none, requests}) {
            tested.passed == true
            exact.error == ""
            exact.candidates.length == 1
            exact.ambiguous == false
            exact.candidates[0].precisionEnumId == "GPP_ADDRESS_MATCH"
            exact.candidates[0].matchKind == "address"
            exact.candidates[0].label == "Piazza Della Signoria 5, 50122 Firenze"
            exact.candidates[0].precisionText.includes("does not say")
            exact.candidates[0].sourceText.includes("CC-BY-4.0")
            exact.candidates[0].release == R1
            Math.abs(exact.candidates[0].lon - 11.2556) < 1e-6
            elsewhere.candidates[0].precisionEnumId == "GPP_PLACE_ESTIMATE"
            elsewhere.candidates[0].localityAgrees == false
            elsewhere.candidates[0].precisionText.includes("locality")
            elsewhere.ambiguous == true
            both.ambiguous == true
            both.candidates.length == 2
            both.candidates.every(candidate => candidate.precisionEnumId == "GPP_ADDRESS_MATCH")
            letter.candidates[0].precisionEnumId == "GPP_ADDRESS_MATCH"
            red.candidates[0].precisionEnumId == "GPP_PLACE_ESTIMATE"
            red.ambiguous == true
            street.candidates[0].matchKind == "street"
            street.candidates[0].precisionEnumId == "GPP_PLACE_ESTIMATE"
            street.candidates[0].extent != null
            postal.reasonCode == "NO_ADDRESS_MATCH"
            postal.candidates[0].matchKind == "postal"
            postal.candidates[0].precisionEnumId == "GPP_POSTAL_AREA"
            none.reasonCode == "NO_MATCH"
            none.candidates.length == 0
            requests == 0
        }
    }
)

feature(
    "a record's address with no point is matched by the local geocoder, for a user with no Maps app too, several points offered as choices, never a match in another city, else its postal area, and nothing is saved",
    {
        when() {
            return {
                signoria: recordContext("ADDRESS_SIGNORIA"),
                plain: recordContext("ADDRESS_SIGNORIA", "ovt.addr.plain"),
                prato: recordContext("ADDRESS_PRATO"),
                fossi: recordContext("ADDRESS_FOSSI"),
                roma: recordContext("ADDRESS_ROMA"),
                stored: ["ADDRESS_SIGNORIA", "ADDRESS_FOSSI", "ADDRESS_ROMA"].filter(id =>
                    ec.cache.getCache("moqui.test.maps.stored").get(id)
                )
            }
        },
        then({signoria, plain, prato, fossi, roma, stored}) {
            signoria.record.precisionEnumId == "GPP_ADDRESS_MATCH"
            signoria.record.addressMatched == true
            signoria.record.located == false
            signoria.record.addressNote.includes("not saved")
            Math.abs(signoria.points.find(point => point.id == "record").lon - 11.2556) < 1e-6
            plain.record.precisionEnumId == "GPP_ADDRESS_MATCH"
            plain.record.addressMatched == true
            prato.record.addressMatched == null
            prato.record.precisionEnumId != "GPP_ADDRESS_MATCH"
            prato.points.length == 0
            fossi.record.addressChoices.length == 2
            fossi.points.filter(point => String(point.id).startsWith("address:")).length == 2
            roma.record.approximate == true
            roma.record.precisionEnumId == "GPP_POSTAL_AREA"
            roma.record.addressChoices == null
            stored.length == 0
        }
    }
)

feature(
    "a refresh is activated as a whole: a moved address at its new point, a removed one gone, the changes counted by address, the run before kept for rollback, a confirmed point untouched",
    {
        when() {
            const first = activeOf()
            const {run} = importRun(R2)
            const after = {
                active: activeOf(),
                previous: one(IMPORT, {importId: first}),
                parione: search("Via Parione 48, Firenze"),
                maggio: search("Via Maggio 15, Firenze"),
                porta: search("Via Porta Rossa 10, Firenze"),
                confirmed: one("moqui.basic.GeoPoint", {geoPointId: CONFIRMED})
            }
            const rolledBack = invoke(S + "rollback#OvertureTheme", {regionId, themeId: "ADDRESSES"})
            const again = {
                active: activeOf(),
                maggio: search("Via Maggio 15, Firenze"),
                staged: count(ADDRESS, {importId: run.importId})
            }
            return {first, run, after, rolledBack, again}
        },
        then({first, run, after, rolledBack, again}) {
            run.statusId == "OviActive"
            JSON.parse(run.changesJson).comparedWith == first
            JSON.parse(run.changesJson).added == 1
            JSON.parse(run.changesJson).removed == 1
            JSON.parse(run.changesJson).moved == 1
            JSON.parse(run.changesJson).unchanged == 4
            after.active == run.importId
            after.previous.statusId == "OviPrevious"
            Math.abs(after.parione.candidates[0].lon - 11.2524) < 1e-6
            after.parione.candidates[0].release == R2
            after.maggio.reasonCode == "NO_MATCH"
            after.porta.candidates[0].precisionEnumId == "GPP_ADDRESS_MATCH"
            Number(after.confirmed.latitude) == 43.7677
            Number(after.confirmed.longitude) == 11.2495
            rolledBack.error == ""
            again.active == first
            again.maggio.candidates[0].precisionEnumId == "GPP_ADDRESS_MATCH"
            again.staged == 0
        }
    }
)
