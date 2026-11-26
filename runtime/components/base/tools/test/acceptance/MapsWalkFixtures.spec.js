// The Maps walks' fixture (client/MapsBrowseWalk, client/MapsDialogWalk; plans/Maps.md §9), run on the walks' server by
// client/suite.js: maps.walk, a Maps user the framework's test adapter lets write (MAPS_SPEC_STORE), and maps.walk.plain,
// with ALL_USERS alone; the test adapter (framework test/service/org/moqui/mapstest) registered as MAPS_WALK_ADAPTER and
// granted to every user, as a component would; a record layer of it on the outline profile (its features one more than a
// layer may send, so the cut is said); a saved view of maps.walk's; and a US postal code (USA_Z_60601, as an import writes
// it, with a Census point and two GeoNames places, each with its own point) for the postal search. Each row is made when it is missing.
// Run: ./moq accept tools --spec MapsBrowseWalk (or MapsDialogWalk), which brings this spec in
const PASSWORD = "MapsWalk1!!"
const one = (entityName, fields) => ec.entity.find(entityName).condition(fields).disableAuthz().one()
const ensure = (entityName, key, fields) => {
    if (!one(entityName, key))
        ec.entity
            .makeValue(entityName)
            .setAll(Object.assign({}, key, fields || {}))
            .create()
}
function ensureUser(userId, username) {
    ensure("moqui.security.UserAccount", {userId}, {username, userFullName: "Maps walk " + username})
    const account = ec.entity.find("moqui.security.UserAccount").condition("userId", userId).one()
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
}
const member = (userGroupId, userId) => {
    if (!ec.entity.find("moqui.security.UserGroupMember").condition({userGroupId, userId}).disableAuthz().count())
        ec.entity
            .makeValue("moqui.security.UserGroupMember")
            .setAll({userGroupId, userId, fromDate: ec.user.nowTimestamp})
            .create()
}

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        ensureUser("MAPS_WALK_USER", "maps.walk")
        ensureUser("MAPS_WALK_PLAIN", "maps.walk.plain")
        member("MAPS_USERS", "MAPS_WALK_USER")
        ensure(
            "moqui.security.UserPermission",
            {userPermissionId: "MAPS_SPEC_STORE"},
            {description: "Maps spec: store a record's location"}
        )
        ensure("moqui.security.UserGroup", {userGroupId: "MAPS_WALK_WRITERS"}, {description: "Maps walk writers"})
        if (!ec.entity.find("moqui.security.UserGroupPermission").condition({userGroupId: "MAPS_WALK_WRITERS"}).count())
            ec.entity
                .makeValue("moqui.security.UserGroupPermission")
                .setAll({userGroupId: "MAPS_WALK_WRITERS", userPermissionId: "MAPS_SPEC_STORE", fromDate: ec.user.nowTimestamp})
                .create()
        member("MAPS_WALK_WRITERS", "MAPS_WALK_USER")
        ensure(
            "moqui.maps.MapAdapter",
            {adapterKey: "MAPS_WALK_ADAPTER"},
            {
                description: "Walk places",
                sequenceNum: 1,
                contextServiceName: "org.moqui.mapstest.MapsTestServices.get#RecordMapContext",
                storeServiceName: "org.moqui.mapstest.MapsTestServices.store#RecordLocation",
                featuresServiceName: "org.moqui.mapstest.MapsTestServices.find#LayerFeatures"
            }
        )
        ensure("moqui.security.ArtifactGroup", {artifactGroupId: "MAPS_WALK_GRANT"}, {description: "Maps walk adapter"})
        ensure(
            "moqui.security.ArtifactGroupMember",
            {
                artifactGroupId: "MAPS_WALK_GRANT",
                artifactName:
                    "org\\.moqui\\.mapstest\\.MapsTestServices\\.(get#RecordMapContext|store#RecordLocation|find#LayerFeatures)",
                artifactTypeEnumId: "AT_SERVICE"
            },
            {nameIsPattern: "Y", inheritAuthz: "N"}
        )
        ensure(
            "moqui.security.ArtifactAuthz",
            {artifactAuthzId: "MAPS_WALK_GRANT"},
            {
                userGroupId: "ALL_USERS",
                artifactGroupId: "MAPS_WALK_GRANT",
                authzTypeEnumId: "AUTHZT_ALLOW",
                authzActionEnumId: "AUTHZA_ALL"
            }
        )
        ensure(
            "moqui.maps.MapLayer",
            {layerId: "MAPS_WALK_RECORDS"},
            {layerName: "Walk records", formatEnumId: "MLF_ADAPTER", adapterKey: "MAPS_WALK_ADAPTER"}
        )
        ensure(
            "moqui.maps.MapProfileLayer",
            {profileId: "OUTLINE", layerId: "MAPS_WALK_RECORDS"},
            {sequenceNum: 1, defaultVisible: "Y"}
        )
        ensure(
            "moqui.maps.MapSavedView",
            {savedViewId: "MAPS_WALK_HARBOR"},
            {
                description: "Walk harbor",
                ownerUserId: "MAPS_WALK_USER",
                isShared: "N",
                profileId: "OUTLINE",
                viewJson: JSON.stringify({center: [-87.6, 41.89], zoom: 11, layers: []})
            }
        )
        // a postal code as the importer writes it: its Geo, code, Census point and two place names
        ensure("moqui.basic.Geo", {geoId: "USA_Z_60601"}, {geoTypeEnumId: "GEOT_POSTAL_CODE", geoName: "60601"})
        ensure(
            "moqui.basic.GeoPostalCode",
            {geoId: "USA_Z_60601"},
            {
                countryGeoId: "USA",
                normalizedCode: "60601",
                displayCode: "60601",
                granularityEnumId: "GPG_FULL",
                sourceKey: "CENSUS_ZCTA"
            }
        )
        ensure(
            "moqui.basic.GeoPoint",
            {geoPointId: "USA_Z_60601_CENSUS_ZCTA"},
            {geoPointTypeEnumId: "GPT_GEO_REFERENCE", latitude: 41.8858, longitude: -87.6181}
        )
        ensure(
            "moqui.basic.GeoPointAssoc",
            {geoId: "USA_Z_60601", geoPointId: "USA_Z_60601_CENSUS_ZCTA"},
            {
                pointRoleEnumId: "GPR_REPRESENTATIVE",
                precisionEnumId: "GPP_POSTAL_AREA",
                dataSetId: "CENSUS_ZCTA",
                edition: "2026",
                priority: 10,
                fromDate: ec.user.nowTimestamp
            }
        )
        // each place with a point of its own, as a GeoNames row has
        for (const [index, [placeName, latitude, longitude]] of [
            ["Chicago", 41.8857, -87.6229],
            ["Loop", 41.8837, -87.6289]
        ].entries()) {
            const geoPointId = "USA_Z_60601_GN_POSTAL_" + (index + 1)
            ensure("moqui.basic.GeoPoint", {geoPointId}, {geoPointTypeEnumId: "GPT_GEO_REFERENCE", latitude, longitude})
            ensure(
                "moqui.basic.GeoPostalCodePlace",
                {geoId: "USA_Z_60601", sourceKey: "GN_POSTAL", placeName},
                {sequenceNum: index + 1, adminName1: "Illinois", geoPointId}
            )
        }
        ec.cache.getCache("maps.postal.lookup").clear()
        ec.artifactExecution.enableAuthz()
    }
})

feature("the Maps walks' users, adapter, record layer and saved view are there", {
    expect() {
        one("moqui.maps.MapAdapter", {adapterKey: "MAPS_WALK_ADAPTER"}) != null
        one("moqui.maps.MapSavedView", {savedViewId: "MAPS_WALK_HARBOR"}) != null
    }
})
