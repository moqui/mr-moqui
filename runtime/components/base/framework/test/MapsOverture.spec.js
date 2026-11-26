// MapsOverture (plans/Maps-Overture.md §6): optional Overture data through org.moqui.maps.OvertureServices, with
// HttpFixtures answering for a small STAC catalog (maps@overture-catalog set to it) and serving the GeoParquet files by
// range with an ETag (the fixtures in test/fixtures/overture, goldens of the moqui-maps crate's tests). Refusals: a
// non-holder (also with authorization off, its run refused by the read's own MAPS_ADMIN check), outside requests off, a
// license not acknowledged, a region too large and a release of another schema. A holder's Places import is active and
// searched, reading only the item and row groups that meet the region, each by range and every range after the first
// pinned to the file's ETag; an update that fails part way, or whose file is replaced while it is read, leaves the release
// before it active and its staged rows gone; a release with a source under a license not acknowledged waits for review,
// and once acknowledged and activated keeps the one before for rollback, which makes it active again; a run cancelled while
// it reads deletes what it staged, and a run held for review is discarded; a Buildings import is the region's layer with
// the theme's attribution, removed with its archive; and Configuration › Overture rendered once (html) for an
// administrator and refused to a non-holder.
// Run: ./moq test framework --spec MapsOverture
const S = "org.moqui.maps.OvertureServices."
const IMPORT = "moqui.maps.OvertureImport"
const PLACE = "moqui.maps.OverturePlace"
const STAC = "https://stac.example.test"
const CATALOG = STAC + "/catalog.json"
const DATA = "https://data.example.test/release/"
const R1 = "2026-10-21.0"
const R2 = "2026-11-18.0"
const R3 = "2026-12-16.0"
const USER = "OVT_SPEC_USER"
const REGION = {regionName: "Spec Florence", west: 11.24, south: 43.76, east: 11.27, north: 43.78}
const http = ec.factory.getTool("HttpFixtures")
const themes = {}
let regionId = null

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
const count = (entity, fields) => sys(() => Number(ec.entity.find(entity).condition(fields).count()))
const fixture = name =>
    ec.resource
        .getLocationReference("component://framework/test/fixtures/overture/" + name)
        .openStream()
        .readAllBytes()
const search = (q, extent) =>
    ec.resource
        .script("component://framework/service/org/moqui/maps/OvertureData.js")
        .execute("searchPlaces", {q, extent: extent || null})

// The catalog: three releases (the third of schema 3), each with places and buildings, each type with two items: one over
// Florence, one over Rome (never read for the region)
function serve(placesR2, replacedR1) {
    http.clear()
    const json = (url, doc) => http.add({url, body: JSON.stringify(doc), contentType: "application/json"})
    json(CATALOG, {
        type: "Catalog",
        latest: R2,
        links: [R1, R2, R3].map(version => ({
            rel: "child",
            href: `${STAC}/${version}/catalog.json`,
            title: version + " Overture Release"
        }))
    })
    const files = {
        [R1]: {places: fixture("places-r1.parquet"), buildings: fixture("buildings-r1.parquet")},
        [R2]: {places: placesR2 || fixture("places-r2.parquet"), buildings: fixture("buildings-r1.parquet")},
        [R3]: {places: fixture("places-r1.parquet"), buildings: fixture("buildings-r1.parquet")}
    }
    for (const version of [R1, R2, R3]) {
        const base = `${STAC}/${version}`
        json(base + "/catalog.json", {
            "release:version": version,
            "schema:version": version === R3 ? "3.0.0" : "2.0.0",
            links: ["places", "buildings"].map(theme => ({rel: "child", href: `./${theme}/catalog.json`, title: theme}))
        })
        for (const [theme, kind] of [
            ["places", "place"],
            ["buildings", "building"]
        ]) {
            json(`${base}/${theme}/catalog.json`, {links: [{rel: "child", href: `./${kind}/collection.json`, title: kind}]})
            const boxes = [
                [11.0, 43.0, 12.0, 44.0],
                [12.0, 41.0, 13.0, 42.5]
            ]
            json(`${base}/${theme}/${kind}/collection.json`, {
                "schema:version": "2.0.0",
                extent: {spatial: {bbox: [[11.0, 41.0, 13.0, 44.0]].concat(boxes)}},
                links: boxes.map((box, index) => ({rel: "item", href: `./0000${index}/0000${index}.json`}))
            })
            boxes.forEach((bbox, index) =>
                json(`${base}/${theme}/${kind}/0000${index}/0000${index}.json`, {
                    bbox,
                    assets: {
                        aws: {
                            href: `${DATA}${version}/theme=${theme}/part-0000${index}.parquet`,
                            type: "application/vnd.apache.parquet"
                        }
                    }
                })
            )
            // an ETag, which the reader pins its later ranges to (If-Match); replacedR1: release 1's places answer their first
            // request as one file and the rest as another, as a file replaced while it is read
            if (replacedR1 && version === R1 && theme === "places")
                http.add({
                    url: `${DATA}${version}/theme=${theme}/part-00000.parquet`,
                    body: files[version][theme],
                    ranges: true,
                    times: 1,
                    headers: {ETag: '"replaced-before"'}
                })
            http.add({
                url: `${DATA}${version}/theme=${theme}/part-00000.parquet`,
                body: files[version][theme],
                ranges: true,
                headers: {ETag: `"${version}-${theme}"`}
            })
        }
    }
}
// Start an import and wait (at most 15 s) for its run on a job worker to end
function importRun(themeId, releaseVersion) {
    const started = invoke(S + "start#OvertureImport", {regionId, themeId, releaseVersion})
    if (!started.importId) return {started, run: null}
    const deadline = Date.now() + 15000
    const going = () => ["OviPending", "OviRunning"].includes(one(IMPORT, {importId: started.importId})?.statusId)
    while (Date.now() < deadline && going()) sleep(50)
    return {started, run: one(IMPORT, {importId: started.importId})}
}
const activeOf = themeId => one("moqui.maps.OvertureRegionTheme", {regionId, themeId})?.activeImportId || null

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        http.enable()
        setConf("maps@overture-catalog", CATALOG)
        setConf("maps@external-requests", "true")
        sys(() => {
            for (const themeId of ["PLACES", "BUILDINGS"]) {
                const theme = ec.entity.find("moqui.maps.OvertureTheme").condition({themeId}).one()
                themes[themeId] = {
                    licenses: theme.licenses,
                    licenseAckUserId: theme.licenseAckUserId,
                    licenseAckDate: theme.licenseAckDate
                }
                theme.setAll({licenseAckUserId: "EX_JOHN_DOE", licenseAckDate: ec.user.nowTimestamp}).update()
            }
            ec.entity
                .makeValue("moqui.security.UserAccount")
                .setAll({userId: USER, username: "ovt.spec.user", userFullName: "Overture spec user"})
                .create()
        })
        serve()
        regionId = invoke(S + "store#OvertureRegion", Object.assign({themeIds: ["PLACES", "BUILDINGS"]}, REGION)).regionId
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        if (regionId) invoke(S + "remove#OvertureTheme", {regionId})
        sys(() => {
            if (regionId) {
                const importIds = Array.from(ec.entity.find(IMPORT).condition({regionId}).list()).map(run => run.importId)
                if (importIds.length) ec.entity.find(PLACE).condition("importId", "in", importIds).deleteAll()
                ec.entity.find(IMPORT).condition({regionId}).deleteAll()
            }
            for (const themeId of Object.keys(themes))
                ec.entity.find("moqui.maps.OvertureTheme").condition({themeId}).one().setAll(themes[themeId]).update()
            ec.entity.find("moqui.security.UserAccount").condition("userId", USER).deleteAll()
        })
        http.reset()
        ec.message.clearErrors()
    }
})

feature(
    "an import is refused to a non-holder, with outside requests off, before its license is acknowledged, for a region too large and for a release of another schema",
    {
        when() {
            ec.user.internalLoginUser("ovt.spec.user")
            const user = invoke(S + "start#OvertureImport", {regionId, themeId: "PLACES"})
            // with authorization off the service starts a run, and the run's read refuses the user by its own check
            const before = http.count()
            const userNoAuthz = sys(() => invoke(S + "start#OvertureImport", {regionId, themeId: "PLACES"}))
            const deadline = Date.now() + 15000
            const going = () =>
                ["OviPending", "OviRunning"].includes(one(IMPORT, {importId: userNoAuthz.importId || "_NA_"})?.statusId)
            while (Date.now() < deadline && going()) sleep(50)
            const userRun = one(IMPORT, {importId: userNoAuthz.importId || "_NA_"})
            ec.user.internalLoginUser("john.doe")
            const off = withConf("maps@external-requests", "false", () =>
                invoke(S + "start#OvertureImport", {regionId, themeId: "PLACES"})
            )
            const setAck = licenseAckUserId =>
                sys(() =>
                    ec.entity
                        .find("moqui.maps.OvertureTheme")
                        .condition({themeId: "BUILDINGS"})
                        .one()
                        .setAll({licenseAckUserId})
                        .update()
                )
            setAck(null)
            const license = invoke(S + "start#OvertureImport", {regionId, themeId: "BUILDINGS"})
            setAck("EX_JOHN_DOE")
            const large = invoke(S + "store#OvertureRegion", {regionName: "Too large", west: 0, south: 40, east: 5, north: 45})
            const schema = importRun("PLACES", R3)
            const dataRequests = http
                .requests()
                .slice(before)
                .filter(request => request.url.startsWith(DATA)).length
            return {user, userRun, off, license, large, schema, dataRequests, runs: count(IMPORT, {regionId})}
        },
        then({user, userRun, off, license, large, schema, dataRequests, runs}) {
            user.error != ""
            user.importId == null
            userRun.statusId == "OviFailed"
            userRun.reasonCode == "PERMISSION_DENIED"
            off.error.includes("EXTERNAL_REQUESTS_OFF")
            license.error.includes("LICENSE_NOT_ACKNOWLEDGED")
            large.error.includes("REGION_TOO_LARGE")
            schema.run.statusId == "OviFailed"
            schema.run.reasonCode == "SCHEMA_UNSUPPORTED"
            schema.run.releaseVersion == R3
            dataRequests == 0
            runs == 2
        }
    }
)

feature("a holder's Places import is active and searched, reading only the item and row groups that meet the region, by range", {
    when() {
        const before = http.count()
        const {started, run} = importRun("PLACES", R1)
        const requests = http.requests().slice(before)
        const data = requests.filter(request => request.url.startsWith(DATA))
        return {
            started,
            run,
            active: activeOf("PLACES"),
            byName: search("cattedrale"),
            byCategory: search("museum"),
            none: search("colosseo"),
            dataUrls: Array.from(new Set(data.map(request => request.url))),
            unranged: data.filter(request => !String(request.headers.Range || request.headers.range || "").startsWith("bytes="))
                .length,
            // every range after the first is pinned to the file's ETag
            unpinned: data.slice(1).filter(request => request.headers["If-Match"] !== `"${R1}-places"`).length,
            ranges: data.length
        }
    },
    then({started, run, active, byName, byCategory, none, dataUrls, unranged, unpinned, ranges}) {
        started.error == ""
        run.statusId == "OviActive"
        run.releaseVersion == R1
        run.schemaVersion == "2.0.0"
        run.rowsKept == 3
        run.rowsRejected == 1
        run.rowGroupsRead == 1
        run.rowGroupsSkipped == 1
        active == run.importId
        byName.results.length == 1
        byName.results[0].label == "Cattedrale di Santa Maria del Fiore"
        byName.results[0].sourceText.includes("religious organization")
        byName.results[0].sourceText.includes("Apache-2.0")
        byName.results[0].gersId == "08f1e2a0-0000-4000-8000-000000000001"
        byName.results[0].release == R1
        byName.attribution.includes("Overture Maps Foundation")
        byCategory.results[0].label == "Galleria degli Uffizi"
        none.results.length == 0
        dataUrls.length == 1
        dataUrls[0].endsWith("part-00000.parquet")
        unranged == 0
        ranges > 1
        unpinned == 0
    }
})

feature(
    "an update that fails part way, or whose file is replaced while it is read, leaves the release before it active, its staged rows gone",
    {
        when() {
            const broken = fixture("places-r2.parquet")
            // the first column chunks overwritten: the footer reads, a page does not
            for (let index = 4; index < 200 && index < broken.length - 100; index++) broken[index] = 0xff
            serve(broken)
            const {run} = importRun("PLACES", R2)
            serve(null, true)
            const replaced = importRun("PLACES", R1).run
            serve()
            return {
                run,
                replaced,
                active: activeOf("PLACES"),
                staged: count(PLACE, {importId: run?.importId || "_NA_"}),
                found: search("cattedrale")
            }
        },
        then({run, replaced, active, staged, found}) {
            run.statusId == "OviFailed"
            run.reasonCode == "READ_FAILED"
            replaced.statusId == "OviFailed"
            replaced.failureText.includes("changed while it was read")
            active != run.importId
            one(IMPORT, {importId: active}).releaseVersion == R1
            staged == 0
            found.results.length == 1
        }
    }
)

feature(
    "a release with a source under a license not acknowledged waits for review; activated it keeps the one before for rollback, which makes it active again",
    {
        when() {
            const first = activeOf("PLACES")
            const {run} = importRun("PLACES", R2)
            const waiting = {active: activeOf("PLACES"), market: search("mercato")}
            const early = invoke(S + "activate#OvertureImport", {importId: run.importId})
            const acknowledged = invoke(S + "acknowledge#OvertureLicense", {themeId: "PLACES", licenses: ["CC-BY-4.0"]})
            const activated = invoke(S + "activate#OvertureImport", {importId: run.importId})
            const after = {
                active: activeOf("PLACES"),
                previous: one(IMPORT, {importId: first}),
                market: search("mercato"),
                renamed: search("cattedrale"),
                regionTheme: one("moqui.maps.OvertureRegionTheme", {regionId, themeId: "PLACES"})
            }
            const rolledBack = invoke(S + "rollback#OvertureTheme", {regionId, themeId: "PLACES"})
            const again = {
                active: activeOf("PLACES"),
                run: one(IMPORT, {importId: run.importId}),
                staged: count(PLACE, {importId: run.importId}),
                found: search("cattedrale"),
                nothing: invoke(S + "rollback#OvertureTheme", {regionId, themeId: "PLACES"})
            }
            return {first, run, waiting, early, acknowledged, activated, after, rolledBack, again}
        },
        then({first, run, waiting, early, acknowledged, activated, after, rolledBack, again}) {
            run.statusId == "OviReview"
            run.newLicenses == "CC-BY-4.0"
            waiting.active == first
            waiting.market.results.length == 0
            early.error.includes("LICENSE_NOT_ACKNOWLEDGED")
            acknowledged.error == ""
            activated.error == ""
            after.active == run.importId
            after.previous.statusId == "OviPrevious"
            after.regionTheme.previousImportId == first
            after.market.results[0].label == "Mercato Centrale"
            after.market.results[0].sourceText.includes("CC-BY-4.0")
            after.renamed.results.length == 0
            rolledBack.error == ""
            again.active == first
            again.run.statusId == "OviRolledBack"
            again.staged == 0
            again.found.results.length == 1
            again.nothing.error.includes("NOTHING_TO_ROLL_BACK")
        }
    }
)

feature(
    "a run cancelled while it reads deletes what it staged, and a run held for review is discarded, the active release kept",
    {
        when() {
            const active = activeOf("PLACES")
            const cancelled = sys(() => {
                const value = ec.entity.makeValue(IMPORT).setAll({
                    regionId,
                    themeId: "PLACES",
                    statusId: "OviPending",
                    releaseVersion: R2,
                    cancelRequested: "Y",
                    userId: ec.user.userId,
                    startedDate: ec.user.nowTimestamp
                })
                value.setSequencedIdPrimary()
                value.create()
                return String(value.importId)
            })
            const ran = invoke(S + "run#OvertureImport", {importId: cancelled})
            // the acknowledgement as it was before the review feature added CC-BY-4.0: release 2 waits for review again
            sys(() =>
                ec.entity
                    .find("moqui.maps.OvertureTheme")
                    .condition({themeId: "PLACES"})
                    .one()
                    .setAll({licenses: themes.PLACES.licenses})
                    .update()
            )
            const {run} = importRun("PLACES", R2)
            const discarded = invoke(S + "discard#OvertureImport", {importId: run.importId})
            return {
                active,
                ran,
                cancelledRun: one(IMPORT, {importId: cancelled}),
                cancelledStaged: count(PLACE, {importId: cancelled}),
                review: run,
                discarded,
                discardedRun: one(IMPORT, {importId: run.importId}),
                discardedStaged: count(PLACE, {importId: run.importId}),
                after: activeOf("PLACES")
            }
        },
        then({active, ran, cancelledRun, cancelledStaged, review, discarded, discardedRun, discardedStaged, after}) {
            ran.statusId == "OviCancelled"
            cancelledRun.statusId == "OviCancelled"
            cancelledStaged == 0
            review.statusId == "OviReview"
            discarded.error == ""
            discardedRun.statusId == "OviDiscarded"
            discardedStaged == 0
            after == active
        }
    }
)

feature("a Buildings import is the region's layer with the theme's attribution, and is removed with its archive", {
    when() {
        const {run} = importRun("BUILDINGS", R1)
        const regionTheme = one("moqui.maps.OvertureRegionTheme", {regionId, themeId: "BUILDINGS"})
        const layer = one("moqui.maps.MapLayer", {layerId: regionTheme?.layerId || "_NA_"})
        const archiveThere = sys(() => ec.resource.getLocationReference(run.archiveLocation).exists)
        const removed = invoke(S + "remove#OvertureTheme", {regionId, themeId: "BUILDINGS"})
        return {
            run,
            layer,
            archiveThere,
            removed,
            layerAfter: layer ? one("moqui.maps.MapLayer", {layerId: layer.layerId}) : "none",
            archiveAfter: sys(() => ec.resource.getLocationReference(run.archiveLocation).exists),
            runAfter: one(IMPORT, {importId: run.importId}),
            places: activeOf("PLACES")
        }
    },
    then({run, layer, archiveThere, removed, layerAfter, archiveAfter, runAfter, places}) {
        run.statusId == "OviActive"
        run.rowsKept == 3
        run.tileCount > 0
        run.archiveLocation.startsWith("dbresource://moqui/maps/overture/")
        archiveThere == true
        layer.formatEnumId == "MLF_PMTILES_VECTOR"
        layer.resourceLocation == run.archiveLocation
        layer.attribution.includes("OpenStreetMap contributors")
        layer.sourceEdition == R1
        layer.sourceRevision != null
        layer.disabled == "N"
        removed.error == ""
        layerAfter == null
        archiveAfter == false
        runAfter.statusId == "OviRemoved"
        places != null
    }
})

feature(
    "Configuration › Overture renders the themes, the region's active release and the runs for an administrator, and is refused to a non-holder",
    {
        when() {
            const render = username => {
                ec.user.internalLoginUser(username)
                try {
                    const res = ec.screen
                        .makeTest()
                        .rootScreen("component://webroot/screen/webroot.xml")
                        .renderMode("html")
                        .render("/apps/maps/Configuration/Overture", {}, null)
                    return {status: res.status, errors: res.errorMessages.join(" | "), output: String(res.output || "")}
                } finally {
                    ec.message.clearErrors()
                    ec.user.internalLoginUser("john.doe")
                }
            }
            return {admin: render("john.doe"), user: render("ovt.spec.user")}
        },
        then({admin, user}) {
            admin.status == 200
            !/Error rendering|Exception in render/.test(admin.output)
            admin.output.includes("Overture places")
            admin.output.includes("ODbL")
            admin.output.includes("Spec Florence")
            admin.output.includes(R1)
            admin.output.includes("Acknowledge license")
            user.status == 403 || /not authorized/i.test(user.errors)
            !user.output.includes("Spec Florence")
        }
    }
)
