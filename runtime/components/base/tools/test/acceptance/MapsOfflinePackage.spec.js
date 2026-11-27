// MapsOfflinePackage (plans/Maps-Offline.md OF4): the server half of the offline map walk on the Android emulator, whose
// device half is scripts/android-maps-offline.sh (it starts this spec's server with `moqui test ... --serve`, then
// drives the app). It makes maps.offline, a Maps user; a small PMTiles archive (zooms 0 to 4 everywhere and 5 to 12 over
// the region, every tile one vector tile of an earth and a water polygon, framework test/MapsArchiveLib.js) registered
// as its own basemap layer, drawn with the packaged Protomaps light style, on a profile of its own that is the user's;
// a saved place of the user's, a postal code with its place name, and an Overture place and two Overture addresses of
// active imports, each in a small region of Boston's harbor; and the user's package of that region (OF1), built by the
// MapPackageBuild job as the Offline screen's Build asks it. Its feature checks the package is ready and that its
// manifest lists the basemap's extract with its style, sprites and fonts, the outline, the places, the postal codes and
// the Overture rows. Each row is made when it is missing, and nothing is removed: the walk downloads the package from
// this database after the spec (a scratch database, gone with the server).
// Run alone: ./moq accept tools --spec MapsOfflinePackage; the walk: scripts/android-maps-offline.sh
const PASSWORD = "MapsOffline1!!"
const USER = "MAPS_OFFLINE_USER"
const USERNAME = "maps.offline"
const LAYER = "MAPS_OFFLINE_WALK"
const PROFILE = "MAPS_OFFLINE_WALK"
const LOCATION = "dbresource://maps-offline-walk/region.pmtiles"
const PACKAGE_NAME = "Offline walk harbor"
// where the manifest is read from the zip
const SCRATCH = "runtime://tmp/maps-offline-walk/manifest.json"
// Boston's harbor, west, south, east, north; and the package's zooms
const REGION = {west: -71.06, south: 42.35, east: -71.04, north: 42.365}
const MAX_ZOOM = 12
const S = "org.moqui.maps.MapSourceServices."
const lib = ec.resource.script("component://framework/test/MapsArchiveLib.js")
const one = (entityName, fields) => ec.entity.find(entityName).condition(fields).disableAuthz().useCache(false).one()
const ensure = (entityName, key, fields) => {
    if (!one(entityName, key))
        ec.entity
            .makeValue(entityName)
            .setAll(Object.assign({}, key, fields || {}))
            .create()
}
const packageOf = () =>
    ec.entity
        .find("moqui.maps.MapPackage")
        .condition({ownerUserId: USER, name: PACKAGE_NAME})
        .disableAuthz()
        .useCache(false)
        .one()
// as the user with authorization on (john.doe, an administrator, when none)
function as(username, action) {
    ec.user.internalLoginUser(username || "john.doe")
    ec.message.clearErrors()
    ec.artifactExecution.enableAuthz()
    try {
        const out = action() || {}
        const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
        return Object.assign({}, out, {error})
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
}
const call = (name, parameters) => ec.service.sync().name(name).parameters(parameters).call()
// manifest.json of a package's zip, whose members are all stored: found by the central directory, written to SCRATCH
function manifestOf(location) {
    const bytes = ec.resource.getLocationReference(location).openStream().readAllBytes()
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
    let end = bytes.length - 22
    while (end >= 0 && view.getUint32(end, true) !== 0x06054b50) end--
    if (end < 0) throw new Error(location + " is not a zip")
    let at = view.getUint32(end + 16, true)
    for (let index = 0; index < view.getUint16(end + 10, true); index++) {
        const nameLength = view.getUint16(at + 28, true)
        const name = String.fromCharCode(...bytes.subarray(at + 46, at + 46 + nameLength))
        if (name === "manifest.json") {
            const local = view.getUint32(at + 42, true)
            const dataAt = local + 30 + view.getUint16(local + 26, true) + view.getUint16(local + 28, true)
            lib.put(SCRATCH, bytes.slice(dataAt, dataAt + view.getUint32(at + 20, true)))
            return JSON.parse(ec.resource.getLocationText(SCRATCH))
        }
        at += 46 + nameLength + view.getUint16(at + 30, true) + view.getUint16(at + 32, true)
    }
    throw new Error(location + " has no manifest.json")
}
const results = {}

spec({
    timeout: 120,
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        ensure("moqui.security.UserAccount", {userId: USER}, {username: USERNAME, userFullName: "Maps offline walk"})
        const account = ec.entity.find("moqui.security.UserAccount").condition("userId", USER).one()
        account.setFields(
            {
                currentPassword: ec.user.hashPassword(PASSWORD),
                passwordSalt: "",
                passwordHashType: "argon2",
                passwordBase64: "N",
                requirePasswordChange: "N",
                disabled: "N",
                successiveFailedLogins: 0
            },
            true,
            null,
            null
        )
        account.update()
        if (!ec.entity.find("moqui.security.UserGroupMember").condition({userGroupId: "MAPS_USERS", userId: USER}).count())
            ec.entity
                .makeValue("moqui.security.UserGroupMember")
                .setAll({userGroupId: "MAPS_USERS", userId: USER, fromDate: ec.user.nowTimestamp})
                .create()
        // the archive: every tile of zooms 0 to 4, and the region's of 5 to 12, one vector tile the light style draws
        if (!ec.resource.getLocationReference(LOCATION).getExists()) {
            const tile = lib.mvtTile()
            const tiles = []
            for (let z = 0; z <= 4; z++)
                for (let x = 0; x < 1 << z; x++)
                    for (let y = 0; y < 1 << z; y++) tiles.push({tileId: lib.zxyToTileId(z, x, y), bytes: tile})
            for (let z = 5; z <= MAX_ZOOM; z++)
                for (const at of lib.viewportTiles(z, [REGION.west, REGION.south, REGION.east, REGION.north]))
                    tiles.push({tileId: at.tileId, bytes: tile})
            const metadata = {
                name: "Maps offline walk",
                attribution: "Walk shapes",
                vector_layers: [{id: "earth"}, {id: "water"}]
            }
            lib.put(LOCATION, lib.archive(tiles, {maxZoom: MAX_ZOOM, center: [-71.05, 42.357, 12], metadata}).bytes)
        }
        ensure(
            "moqui.maps.MapLayer",
            {layerId: LAYER},
            {
                layerName: "Offline walk map",
                formatEnumId: "MLF_PMTILES_VECTOR",
                styleLocation: "/libs/protomaps/light.json",
                assetsLocation: "/libs/protomaps",
                crs: "EPSG:3857",
                attribution: "Walk shapes",
                isBasemap: "Y",
                disabled: "Y"
            }
        )
        ensure(
            "moqui.maps.MapProfile",
            {profileId: PROFILE},
            {
                profileName: "Offline walk",
                basemapLayerId: LAYER,
                centerLongitude: -71.05,
                centerLatitude: 42.357,
                zoom: 13,
                externalAccess: "N",
                allowedOverrides: "basemap",
                disabled: "N"
            }
        )
        ensure("moqui.security.UserPreference", {userId: USER, preferenceKey: "maps.profile"}, {preferenceValue: PROFILE})
        // a postal code as the importer writes it, its point and its place name in the region
        ensure("moqui.basic.Geo", {geoId: "USA_Z_02110"}, {geoTypeEnumId: "GEOT_POSTAL_CODE", geoName: "02110"})
        ensure(
            "moqui.basic.GeoPostalCode",
            {geoId: "USA_Z_02110"},
            {
                countryGeoId: "USA",
                normalizedCode: "02110",
                displayCode: "02110",
                granularityEnumId: "GPG_FULL",
                sourceKey: "CENSUS_ZCTA"
            }
        )
        ensure(
            "moqui.basic.GeoPoint",
            {geoPointId: "USA_Z_02110_CENSUS_ZCTA"},
            {geoPointTypeEnumId: "GPT_GEO_REFERENCE", latitude: 42.357, longitude: -71.053}
        )
        ensure(
            "moqui.basic.GeoPointAssoc",
            {geoId: "USA_Z_02110", geoPointId: "USA_Z_02110_CENSUS_ZCTA"},
            {
                pointRoleEnumId: "GPR_REPRESENTATIVE",
                precisionEnumId: "GPP_POSTAL_AREA",
                dataSetId: "CENSUS_ZCTA",
                edition: "2026",
                priority: 10,
                fromDate: ec.user.nowTimestamp
            }
        )
        ensure(
            "moqui.basic.GeoPostalCodePlace",
            {geoId: "USA_Z_02110", sourceKey: "GN_POSTAL", placeName: "Boston"},
            {sequenceNum: 1, adminName1: "Massachusetts"}
        )
        // the active Overture imports of the region: a place, and two addresses of one street with the keys an import
        // writes (moqui-lib geo/address.rs street_key, number_key and postcode_key of these)
        ensure(
            "moqui.maps.OvertureRegion",
            {regionId: "MAPS_OFFLINE_WALK"},
            Object.assign({regionName: "Offline walk harbor"}, REGION)
        )
        const licensesJson = JSON.stringify([{dataset: "OpenStreetMap", license: "ODbL-1.0", rows: 3}])
        for (const themeId of ["PLACES", "ADDRESSES"]) {
            const importId = "MAPS_OFFLINE_WALK_" + themeId
            ensure(
                "moqui.maps.OvertureImport",
                {importId},
                {regionId: "MAPS_OFFLINE_WALK", themeId, statusId: "OviActive", releaseVersion: "2026-11-18.0", licensesJson}
            )
            ensure("moqui.maps.OvertureRegionTheme", {regionId: "MAPS_OFFLINE_WALK", themeId}, {activeImportId: importId})
        }
        ensure(
            "moqui.maps.OverturePlace",
            {placeId: "MAPS_OFFLINE_WALK_P1"},
            {
                importId: "MAPS_OFFLINE_WALK_PLACES",
                gersId: "walk-place-1",
                placeName: "Walk Harbor Bakery",
                nameSearch: "walk harbor bakery",
                basicCategory: "bakery",
                latitude: 42.358,
                longitude: -71.052,
                confidence: 0.9,
                addressText: "100 Harbor Street",
                locality: "Boston",
                region: "MA",
                postcode: "02110",
                countryCode: "US",
                sourceDatasets: "OpenStreetMap",
                sourceLicenses: "ODbL-1.0"
            }
        )
        for (const [index, number, latitude, longitude] of [
            [1, "100", 42.359, -71.051],
            [2, "102", 42.3592, -71.0512]
        ])
            ensure(
                "moqui.maps.OvertureAddress",
                {addressId: "MAPS_OFFLINE_WALK_A" + index},
                {
                    importId: "MAPS_OFFLINE_WALK_ADDRESSES",
                    gersId: "walk-address-" + index,
                    countryCode: "US",
                    streetName: "Harbor Street",
                    streetNorm: "harbor st",
                    houseNumber: number,
                    numberNorm: number,
                    unitNorm: "",
                    postcode: "02110",
                    postcodeNorm: "02110",
                    postalCity: "Boston",
                    locality: "Boston",
                    region: "MA",
                    latitude,
                    longitude,
                    sourceDatasets: "OpenStreetMap",
                    sourceLicenses: "ODbL-1.0"
                }
            )
        // registered as an administrator does it, through the service
        results.registered = as(null, () => call(S + "register#ArchiveLayer", {layerId: LAYER, resourceLocation: LOCATION}))
        // the user's saved place, and their package of the region, as Browse and the Offline screen make them
        results.place = as(USERNAME, () =>
            call("org.moqui.maps.MapServices.store#Place", {label: "Walk Harbor Cafe", longitude: -71.05, latitude: 42.36})
        )
        if (!packageOf())
            results.created = as(USERNAME, () =>
                call(
                    S + "create#Package",
                    Object.assign(
                        {
                            name: PACKAGE_NAME,
                            minZoom: 0,
                            maxZoom: MAX_ZOOM,
                            profileId: PROFILE,
                            basemapLayerIds: [LAYER],
                            includePlaces: "Y",
                            includeOverlays: "Y",
                            includePostal: "Y",
                            includeOverture: "Y"
                        },
                        REGION
                    )
                )
            )
        // the MapPackageBuild job runs after the create commits
        const deadline = Date.now() + 60000
        while (Date.now() < deadline && ["MapPkgPending", "MapPkgBuilding"].includes(String(packageOf()?.statusId))) sleep(200)
        ec.artifactExecution.disableAuthz()
    }
})

feature("the user's package of the region is built with its basemap, style, places, postal codes and Overture rows", {
    when() {
        const row = packageOf() || {}
        const manifest = row.location && row.statusId === "MapPkgReady" ? manifestOf(row.location) : {}
        const paths = Array.from(manifest.members || []).map(member => member.path)
        ec.logger.warn(
            "MapsOfflinePackage: package " +
                row.packageId +
                " " +
                row.statusId +
                " revision " +
                row.revision +
                ", " +
                row.size +
                " bytes, " +
                paths.length +
                " members"
        )
        return {
            row: {packageId: row.packageId, statusId: row.statusId, statusNote: row.statusNote, size: row.size},
            manifest,
            paths
        }
    },
    then({row, manifest, paths}) {
        results.registered.applied == true
        results.place.applied == true
        row.statusId == "MapPkgReady" && row.size > 0
        manifest.package.packageId == row.packageId && manifest.package.maxZoom == MAX_ZOOM
        paths.includes("basemap/" + LAYER + ".pmtiles")
        paths.some(path => path.startsWith("style/")) &&
            paths.some(path => path.startsWith("sprites/")) &&
            paths.some(path => path.startsWith("fonts/"))
        paths.includes("outline.json") && paths.includes("places.geojson") && paths.includes("postal.json")
        paths.includes("overture-places.json") && paths.includes("overture-addresses.json")
        Array.from(manifest.failures || []).length == 0
    }
})
