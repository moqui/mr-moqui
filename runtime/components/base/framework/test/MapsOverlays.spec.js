// Users' overlays and saved places (plans/Maps.md §4.1, §4.2, §8; org.moqui.maps.MapServices store#Overlay, get#OverlayContent,
// find#Overlays, store#Place; MapOverlays.js; tools screen/Maps/Browse.xml overlay): a file is read before it is kept (one
// that cannot be drawn is refused with nothing written, a format that is not GeoJSON, GPX or KML, more than
// maps@max-features features), an upload is kept in Maps/overlays of the personal drive with its bad feature counted and
// its KML network link and script not passed on; the overlay transition answers the owner and a holder of the file's
// share, and refuses a non-holder and the holder once the share is revoked, writing no ResourceRecent; saved places are a
// GeoJSON file of the personal drive, found by Browse's search; a file past maps@max-features is offered simplifying on the
// server, and with generalize is kept and cut into tiles beside it (the MapOverlayTiles job) that the tile transition
// answers its owner and refuses a non-holder; a delimited text and a binary WKB upload are read by their own readers; and
// an overlay edited on the map is saved by its owner only, beside a KML file as name.edited.geojson, refused for a
// geometry that is not valid and for a file changed since the edit began; a file in UTM or the British National Grid is
// converted to WGS84 with its CRS and transform recorded (P4c). The converter's own rules (geometry, CRS, caps,
// sanitizing, each format's reader) are moqui-lib geo/overlay_tests.rs and geo/overlay/formats_tests.rs, the tiler's
// geo/tiles_tests.rs.
// Run: ./moq test framework --spec MapsOverlays
const S = "org.moqui.maps.MapServices."
const OWNER = "MAPS_OVL_OWNER"
const READER = "MAPS_OVL_READER"
const OTHER = "MAPS_OVL_OTHER"
const SHARE = "MAPS_OVL_SHARE"
const FILE_SHARE = "MAPS_OVL_FILE_SHARE"
const FAKE_RUN = "MAPS_OVL_RUN"
// a pause of the thread, for the job a feature waits on
const pause = ms => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms)
const users = [OWNER, READER, OTHER]
const usernameOf = userId => userId.toLowerCase().replace(/_/g, ".")
const make = (entity, fields) => ec.entity.makeValue(entity).setAll(fields).createOrUpdate()
const KML = `<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2"><Document>
  <NetworkLink><Link><href>https://tracker.spec.example/more.kml</href></Link></NetworkLink>
  <Placemark><name>Pond</name><description><![CDATA[Ducks<script>alert('spec')</script>]]></description>
    <Point><coordinates>-71.06,42.36</coordinates></Point></Placemark>
  <Placemark><name>Trail</name><LineString><coordinates>-71.1,42.3 -71.0,42.4</coordinates></LineString></Placemark>
  <Placemark><name>Nowhere</name><Point><coordinates>-71.06,142.36</coordinates></Point></Placemark>
</Document></kml>`
const geojson = count =>
    JSON.stringify({
        type: "FeatureCollection",
        features: Array.from({length: count}, (each, index) => ({
            type: "Feature",
            properties: {name: "Point " + index},
            geometry: {type: "Point", coordinates: [index, index]}
        }))
    })

function invoke(name, params = {}) {
    try {
        const result = ec.service.sync().name(name).parameters(params).call() || {}
        const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
        ec.message.clearErrors()
        return Object.assign({}, result, {error})
    } catch (e) {
        ec.message.clearErrors()
        return {error: String(e.message || e)}
    }
}
function as(userId, action) {
    ec.user.internalLoginUser(usernameOf(userId))
    ec.message.clearErrors()
    ec.artifactExecution.enableAuthz()
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
}
// Browse's overlay transition as the user: its status and body
function overlay(userId, overlayId) {
    return as(userId, () => {
        const res = ec.screen
            .makeTest()
            .rootScreen("component://webroot/screen/webroot.xml")
            .render("apps/maps/Browse/overlay", {overlayId}, "get")
        return {status: res.status, output: String(res.output || "")}
    })
}
const ownerDrive = () =>
    ec.entity
        .find("moqui.resource.ResourceDrive")
        .condition({ownerUserId: OWNER, driveKindEnumId: "RdkPersonal"})
        .disableAuthz()
        .one()
const fileText = (drive, path) => {
    const ref = ec.resource.getLocationReference(drive.rootLocation + "/" + path)
    return ref.getExists() ? String(ref.getText()) : null
}
const recentCount = userId => ec.entity.find("moqui.resource.ResourceRecent").condition("userId", userId).disableAuthz().count()
const state = {}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const userId of users) {
            make("moqui.security.UserAccount", {userId, username: usernameOf(userId), userFullName: "Maps overlays " + userId})
            make("moqui.security.UserGroupMember", {userGroupId: "MAPS_USERS", userId, fromDate: ec.user.nowTimestamp})
        }
        ec.message.clearErrors()
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const del = (entity, field, values) => ec.entity.find(entity).condition(field, "in", values).deleteAll()
        del("moqui.maps.MapOverlay", "ownerUserId", users)
        del("moqui.resource.ResourceShare", "shareId", [SHARE, FILE_SHARE])
        del("moqui.service.job.ServiceJobRun", "jobRunId", [FAKE_RUN])
        del("moqui.resource.ResourceRecent", "userId", users)
        for (const drive of Array.from(
            ec.entity.find("moqui.resource.ResourceDrive").condition("ownerUserId", "in", users).list()
        )) {
            // the drive's whole tree, as one change set of its parent folder (a reference's delete does not take a folder
            // that has files)
            const root = String(drive.rootLocation).replace(/\/+$/, "")
            const slash = root.lastIndexOf("/")
            const removed = ec.resource
                .makeChangeSet(root.slice(0, slash))
                .delete(root.slice(slash + 1))
                .commit(null)
            if (removed.failures.length || ec.resource.getLocationReference(root).getExists())
                ec.logger.warn("MapsOverlays cleanup: " + root + " not deleted")
            ec.entity.find("moqui.resource.ResourceDrive").condition("driveId", drive.driveId).deleteAll()
        }
        del("moqui.security.UserGroupMember", "userId", users)
        del("moqui.security.UserAccount", "userId", users)
        ec.message.clearErrors()
    }
})

feature("a file is read before it is kept: refused with nothing written, or kept in Maps/overlays with its bad feature counted", {
    when() {
        const out = as(OWNER, () => {
            const out = {}
            out.invalid = invoke(S + "store#Overlay", {file: uploadedFile("bad.geojson", geojson(1).replace("[0,0]", "[0,95]"))})
            out.notGeo = invoke(S + "store#Overlay", {file: uploadedFile("notes.txt", "plain")})
            out.tooMany = withConf("maps@max-features", "2", () =>
                invoke(S + "store#Overlay", {file: uploadedFile("many.geojson", geojson(3))})
            )
            out.rowsBefore = ec.entity.find("moqui.maps.MapOverlay").condition("ownerUserId", OWNER).disableAuthz().count()
            out.kml = invoke(S + "store#Overlay", {file: uploadedFile("trip.kml", KML, "application/vnd.google-earth.kml+xml")})
            out.listed = invoke(S + "find#Overlays").overlayList || []
            return out
        })
        // the drive's files, read as the framework
        const drive = ownerDrive()
        out.badKept = !!drive && fileText(drive, "Maps/overlays/bad.geojson") !== null
        out.kept = drive ? fileText(drive, "Maps/overlays/trip.kml") : null
        return out
    },
    then(out) {
        state.overlayId = out.kml.overlayId
        out.invalid.applied == false
        out.invalid.reasonCode == "NO_FEATURES"
        out.notGeo.reasonCode == "FORMAT_UNKNOWN"
        out.tooMany.reasonCode == "TOO_MANY_FEATURES"
        out.rowsBefore == 0
        out.badKept == false
        out.kml.applied == true
        out.kml.featureCount == 2
        out.kml.rejectedCount == 1
        out.kml.notes.some(note => note.includes("network links were not followed"))
        // the original kept as it was sent
        out.kept == KML
        out.listed.length == 1
        out.listed[0].label == "trip"
        out.listed[0].isOwn == true
        out.listed[0].path == "Maps/overlays/trip.kml"
    }
})

feature(
    "the overlay transition answers the owner and a holder of the file's share, and refuses a non-holder and a revoked share",
    {
        when() {
            const out = {}
            out.owner = overlay(OWNER, state.overlayId)
            out.before = overlay(READER, state.overlayId)
            ec.artifactExecution.disableAuthz()
            make("moqui.resource.ResourceShare", {
                shareId: SHARE,
                driveId: ownerDrive().driveId,
                path: "Maps/overlays",
                sharedWithUserId: READER,
                permissionEnumId: "RspView",
                createdByUserId: OWNER,
                createdDate: ec.user.nowTimestamp
            })
            out.recentBefore = recentCount(READER)
            out.holder = overlay(READER, state.overlayId)
            out.readerList = as(READER, () => invoke(S + "find#Overlays").overlayList || [])
            out.other = overlay(OTHER, state.overlayId)
            out.recentAfter = recentCount(READER)
            ec.artifactExecution.disableAuthz()
            const share = ec.entity.find("moqui.resource.ResourceShare").condition("shareId", SHARE).one()
            share.set("revokedDate", ec.user.nowTimestamp)
            share.update()
            out.revoked = overlay(READER, state.overlayId)
            return out
        },
        then(out) {
            out.owner.status == 200
            JSON.parse(out.owner.output).features.length == 2
            out.owner.output.includes('"label":"Pond"')
            !out.owner.output.includes("<script") && !out.owner.output.includes("tracker.spec.example")
            out.before.status == 404
            out.holder.status == 200
            out.holder.output == out.owner.output
            out.readerList.length == 1
            out.readerList[0].isOwn == false
            out.readerList[0].path == null
            out.other.status == 404
            out.recentAfter == out.recentBefore
            out.revoked.status == 404
        }
    }
)

feature("saved places are a GeoJSON file of the personal drive, refused out of range, found by Browse's search", {
    when() {
        const out = as(OWNER, () => {
            const out = {}
            out.saved = invoke(S + "store#Place", {label: "Boathouse", longitude: "-71.07", latitude: "42.355"})
            out.outOfRange = invoke(S + "store#Place", {label: "Off", longitude: "-71", latitude: "91"})
            out.found = ec.resource
                .script("component://framework/service/org/moqui/maps/MapOverlays.js")
                .execute("searchPlaces", {q: "boat"}).results
            out.places = (invoke(S + "find#Overlays", {placesOnly: true}).overlayList || [])[0] || {}
            return out
        })
        out.file = JSON.parse(fileText(ownerDrive(), "Maps/places.geojson") || "{}")
        return out
    },
    then(out) {
        out.saved.applied == true
        out.outOfRange.reasonCode == "POINT_INVALID"
        out.file.features.length == 1
        out.file.features[0].properties.name == "Boathouse"
        out.places.isPlaces == true
        out.places.featureCount == 1
        out.found.length == 1
        out.found[0].placeId == out.saved.placeId
        out.found[0].precisionEnumId == "GPP_USER_CONFIRMED"
    }
})

// A little-endian WKB point as the characters of an uploadedFile's binary bytes
const wkbPoint = (lon, lat) => {
    const view = new DataView(new ArrayBuffer(21))
    view.setUint8(0, 1)
    view.setUint32(1, 1, true)
    view.setFloat64(5, lon, true)
    view.setFloat64(13, lat, true)
    return String.fromCharCode(...new Uint8Array(view.buffer))
}

feature("a delimited text and a binary WKB upload are read by their own readers", {
    when() {
        return as(OWNER, () => ({
            csv: invoke(S + "store#Overlay", {file: uploadedFile("stops.csv", "name,lat,lon,route\nNorth,42.37,-71.05,1\nSouth,42.33,-71.06,1\n")}),
            wkb: invoke(S + "store#Overlay", {file: uploadedFile("spot.wkb", wkbPoint(-71.06, 42.36), "application/octet-stream", true)}),
            gpkg: invoke(S + "store#Overlay", {file: uploadedFile("layers.gpkg", "SQLite format 3")})
        }))
    },
    then(out) {
        out.csv.applied == true && out.csv.featureCount == 2
        out.csv.notes.some(note => note.includes("other properties"))
        out.wkb.applied == true && out.wkb.featureCount == 1
        out.gpkg.reasonCode == "FORMAT_UNKNOWN" && out.gpkg.reason.includes("GeoPackage")
    }
})

feature("an overlay in UTM or the British National Grid is converted to WGS84, its CRS and transform recorded", {
    when() {
        const crsOf = name => ({type: "name", properties: {name}})
        const utm = {
            type: "FeatureCollection",
            crs: crsOf("EPSG:32619"),
            features: [{type: "Feature", properties: {name: "Boston"}, geometry: {type: "Point", coordinates: [330000, 4691000]}}]
        }
        const bng = {
            type: "FeatureCollection",
            crs: crsOf("urn:ogc:def:crs:EPSG::27700"),
            features: [
                {type: "Feature", properties: {name: "Greenwich"}, geometry: {type: "Point", coordinates: [538874, 177344]}}
            ]
        }
        const stored = as(OWNER, () =>
            invoke(S + "store#Overlay", {file: uploadedFile("boston-utm.geojson", JSON.stringify(utm))})
        )
        const convert = (text, format) =>
            invoke("org.moqui.maps.MapSourceServices.convert#Overlay", {text, format, summaryOnly: true})
        return {
            stored,
            row: stored.overlayId
                ? ec.entity.find("moqui.maps.MapOverlay").condition("overlayId", stored.overlayId).disableAuthz().one()
                : null,
            bng: convert(JSON.stringify(bng), "MLF_GEOJSON"),
            unknown: convert("SRID=2056;POINT(2600000 1200000)", "MLF_WKT")
        }
    },
    then({stored, row, bng, unknown}) {
        stored.applied == true && row.sourceCrs == "EPSG:32619" && /transverse Mercator/.test(row.crsTransform)
        row.minLongitude > -71.1 && row.maxLongitude < -71.0 && row.minLatitude > 42.3 && row.maxLatitude < 42.4
        bng.valid == true && bng.crs == "EPSG:27700" && /OSGB36/.test(bng.transform)
        bng.bounds[1] > 51.47 && bng.bounds[1] < 51.49 && Math.abs(bng.bounds[0]) < 0.01
        unknown.valid == false && unknown.reasonCode == "CRS_UNSUPPORTED" && /2056/.test(unknown.reason)
    }
})

feature("an overlay is edited on the map by its owner only: saved beside its KML file, refused when invalid or stale", {
    when() {
        const out = {}
        const edit = (userId, params) => as(userId, () => invoke(S + "store#OverlayEdit", params))
        out.opened = as(OWNER, () => invoke(S + "get#OverlayEdit", {overlayId: state.overlayId}))
        out.otherOpened = as(OTHER, () => invoke(S + "get#OverlayEdit", {overlayId: state.overlayId}))
        const collection = JSON.parse(out.opened.geojson || '{"features":[]}')
        collection.features.push({
            type: "Feature",
            id: "nwalk",
            geometry: {type: "LineString", coordinates: [[-71.07, 42.35], [-71.06, 42.36]]},
            properties: {label: "<b>New</b> walk", other: "dropped"}
        })
        const given = {overlayId: state.overlayId, geojson: JSON.stringify(collection), baseVersion: out.opened.version}
        out.other = edit(READER, given)
        const broken = JSON.parse(given.geojson)
        broken.features[0].geometry = {type: "Point", coordinates: [-71, 95]}
        out.invalid = edit(OWNER, Object.assign({}, given, {geojson: JSON.stringify(broken)}))
        out.saved = edit(OWNER, given)
        out.stale = edit(OWNER, given)
        out.again = edit(OWNER, Object.assign({}, given, {baseVersion: out.saved.version}))
        ec.artifactExecution.disableAuthz()
        const drive = ownerDrive()
        out.original = fileText(drive, "Maps/overlays/trip.kml")
        out.edited = JSON.parse(fileText(drive, "Maps/overlays/trip.edited.geojson") || "{}")
        out.row = ec.entity.find("moqui.maps.MapOverlay").condition("overlayId", state.overlayId).one() || {}
        // a GeoJSON file is saved over only when it is what the map makes of it; one with a name property is not
        const one = props => JSON.stringify({type: "FeatureCollection", features: [
            {type: "Feature", id: "a", geometry: {type: "Point", coordinates: [1, 2]}, properties: props}]})
        out.exact = as(OWNER, () => {
            const added = invoke(S + "store#Overlay", {file: uploadedFile("exact.geojson", one({label: "A"}))})
            return invoke(S + "get#OverlayEdit", {overlayId: added.overlayId})
        })
        out.named = as(OWNER, () => {
            const added = invoke(S + "store#Overlay", {file: uploadedFile("named.geojson", one({name: "A"}))})
            return invoke(S + "get#OverlayEdit", {overlayId: added.overlayId})
        })
        return out
    },
    then(out) {
        out.opened.applied == true && out.opened.inPlace == false
        out.opened.editPath == "Maps/overlays/trip.edited.geojson"
        out.otherOpened.reasonCode == "RECORD_UNAVAILABLE"
        out.other.reasonCode == "RECORD_UNAVAILABLE"
        out.invalid.reasonCode == "INVALID_GEOMETRY"
        out.saved.applied == true && out.saved.featureCount == 3
        out.saved.path == "Maps/overlays/trip.edited.geojson"
        out.stale.reasonCode == "STALE_VERSION"
        out.again.applied == true
        // the original kept as it was, the edit beside it in plain text, and the overlay drawn from the edit
        out.original == KML
        out.edited.features.length == 3
        out.edited.features[2].properties.label == "New walk" && out.edited.features[2].properties.other === undefined
        out.row.path == "Maps/overlays/trip.edited.geojson" && out.row.sourcePath == "Maps/overlays/trip.kml"
        out.row.formatEnumId == "MLF_GEOJSON" && out.row.featureCount == 3
        // the original's provenance kept
        String(out.row.importNotes).includes("network links") && out.row.sourceCrs == "EPSG:4326"
        out.exact.inPlace == true && out.exact.editPath == "Maps/overlays/exact.geojson"
        out.named.inPlace == false && out.named.editPath == "Maps/overlays/named.edited.geojson"
    }
})

// the tile transition as the user, at a tile URL of the overlay (its template filled): its status
function tile(userId, url, zxy) {
    return as(userId, () => {
        const path = String(url || "/apps/maps/LocalMap/tile/none/none/{z}/{x}/{y}").replace(/^\//, "").replace("{z}/{x}/{y}", zxy)
        const res = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml").render(path, {}, "get")
        return {status: res.status, type: String(res.header("Content-Type") || "")}
    })
}

feature("a file too large to draw whole is offered simplifying, and its tiles are made beside it for its readers", {
    when() {
        const out = {}
        withConf("maps@max-features", "2", () => {
            out.offered = as(OWNER, () => invoke(S + "store#Overlay", {file: uploadedFile("many.geojson", geojson(3))}))
            out.added = as(OWNER, () =>
                invoke(S + "store#Overlay", {file: uploadedFile("many.geojson", geojson(3)), generalize: true})
            )
        })
        // the MapOverlayTiles job runs after the add commits
        const row = () =>
            ec.entity
                .find("moqui.maps.MapOverlay")
                .condition("overlayId", out.added.overlayId || "_NA_")
                .disableAuthz()
                .one()
        const deadline = Date.now() + 10000
        while (Date.now() < deadline && !["ready", "failed", "cancelled"].includes(String(row()?.tilesStatus))) pause(50)
        out.row = row() || {}
        out.listed =
            (as(OWNER, () => invoke(S + "find#Overlays").overlayList) || []).find(entry => entry.overlayId == out.added.overlayId) || {}
        out.owner = tile(OWNER, out.listed.tileUrl, "0/0/0")
        out.other = tile(OTHER, out.listed.tileUrl, "0/0/0")
        out.cancel = as(OWNER, () => invoke("org.moqui.maps.MapSourceServices.cancel#OverlayTiles", {overlayId: out.added.overlayId}))
        ec.artifactExecution.disableAuthz()
        const drive = ownerDrive()
        out.archive = !!out.row.tilesPath && ec.resource.getLocationReference(drive.rootLocation + "/" + out.row.tilesPath).getExists()
        // a holder of the file's share reads its tiles, and may not simplify it (no file of theirs can go beside it)
        make("moqui.resource.ResourceShare", {
            shareId: FILE_SHARE,
            driveId: drive.driveId,
            path: "Maps/overlays/many.geojson",
            sharedWithUserId: READER,
            permissionEnumId: "RspView",
            createdByUserId: OWNER,
            createdDate: ec.user.nowTimestamp
        })
        out.holder = tile(READER, out.listed.tileUrl, "0/0/0")
        const pick = {driveId: drive.driveId, path: "Maps/overlays/many.geojson", generalize: true}
        out.notWritable = withConf("maps@max-features", "2", () => as(READER, () => invoke(S + "store#Overlay", pick)))
        ec.artifactExecution.disableAuthz()
        const share = ec.entity.find("moqui.resource.ResourceShare").condition("shareId", FILE_SHARE).one()
        share.set("revokedDate", ec.user.nowTimestamp)
        share.update()
        out.revoked = tile(READER, out.listed.tileUrl, "0/0/0")
        // a run in progress (its job run not ended) refuses the file again; once its run is gone a cancel ends it
        ec.artifactExecution.disableAuthz()
        make("moqui.service.job.ServiceJobRun", {jobRunId: FAKE_RUN, jobName: "MapOverlayTiles", startTime: ec.user.nowTimestamp})
        ec.entity
            .makeValue("moqui.maps.MapOverlay")
            .setAll({overlayId: out.added.overlayId, tilesStatus: "running", tilesJobRunId: FAKE_RUN})
            .update()
        out.running = withConf("maps@max-features", "2", () => as(OWNER, () => invoke(S + "store#Overlay", pick)))
        ec.artifactExecution.disableAuthz()
        ec.entity.find("moqui.service.job.ServiceJobRun").condition("jobRunId", FAKE_RUN).deleteAll()
        out.ended = as(OWNER, () => invoke("org.moqui.maps.MapSourceServices.cancel#OverlayTiles", {overlayId: out.added.overlayId}))
        out.endedRow = row() || {}
        return out
    },
    then(out) {
        out.offered.applied == false && out.offered.reasonCode == "TOO_MANY_FEATURES" && out.offered.canGeneralize == true
        out.added.applied == true && out.added.generalizing == true && out.added.featureCount == 3
        out.row.tilesStatus == "ready"
        out.row.tilesPath == "Maps/overlays/many.tiles.pmtiles" && out.archive
        out.listed.tileUrl.startsWith("/apps/maps/LocalMap/tile/overlay:" + out.added.overlayId + "/")
        out.owner.status == 200 && out.owner.type.startsWith("application/vnd.mapbox-vector-tile")
        out.other.status == 404
        // a run that has ended is not cancelled
        out.cancel.reasonCode == "RECORD_UNAVAILABLE"
        out.holder.status == 200
        out.notWritable.reasonCode == "TILES_NOT_WRITABLE"
        out.revoked.status == 404
        out.running.reasonCode == "TILES_RUNNING"
        out.ended.applied == true && out.endedRow.tilesStatus == "cancelled"
    }
})
