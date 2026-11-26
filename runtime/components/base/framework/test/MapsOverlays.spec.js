// Users' overlays and saved places (plans/Maps.md §4.1, §4.2, §8; org.moqui.maps.MapServices store#Overlay, get#OverlayContent,
// find#Overlays, store#Place; MapOverlays.js; tools screen/Maps/Browse.xml overlay): a file is read before it is kept (one
// that cannot be drawn is refused with nothing written, a format that is not GeoJSON, GPX or KML, more than
// maps@max-features features), an upload is kept in Maps/overlays of the personal drive with its bad feature counted and
// its KML network link and script not passed on; the overlay transition answers the owner and a holder of the file's
// share, and refuses a non-holder and the holder once the share is revoked, writing no ResourceRecent; saved places are a
// GeoJSON file of the personal drive, found by Browse's search. The converter's own rules (geometry, CRS, caps,
// sanitizing) are moqui-lib geo/overlay_tests.rs.
// Run: ./moq test framework --spec MapsOverlays
const S = "org.moqui.maps.MapServices."
const OWNER = "MAPS_OVL_OWNER"
const READER = "MAPS_OVL_READER"
const OTHER = "MAPS_OVL_OTHER"
const SHARE = "MAPS_OVL_SHARE"
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
        del("moqui.resource.ResourceShare", "shareId", [SHARE])
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
