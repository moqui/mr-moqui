// The fixture of client/MapsLocalOnly (plans/archive/Maps.md §5, §9), run on the walks' server by client/suite.js, whose server has
// maps@external-requests false: maps.local, a Maps user whose map profile is the local Protomaps profile; a PMTiles archive
// (zoom 0 to 4, every tile one vector tile of an earth and a water polygon, framework test/MapsArchiveLib.js) in a
// dbresource:// file outside any drive, registered as LOCAL_PROTOMAPS_LIGHT and LOCAL_PROTOMAPS_DARK
// (MapSourceServices.register#ArchiveLayer, as an administrator); and the LOCAL_PROTOMAPS profile enabled. Each is made
// when it is missing.
// Run: ./moq accept tools --spec MapsLocalOnly, which brings this spec in
const PASSWORD = "MapsLocal1!!"
const USER = "MAPS_LOCAL_USER"
const LOCATION = "dbresource://maps-local-spec/region.pmtiles"
const lib = ec.resource.script("component://framework/test/MapsArchiveLib.js")
const one = (entityName, fields) => ec.entity.find(entityName).condition(fields).disableAuthz().useCache(false).one()
const ensure = (entityName, key, fields) => {
    if (!one(entityName, key))
        ec.entity
            .makeValue(entityName)
            .setAll(Object.assign({}, key, fields || {}))
            .create()
}
const results = {}

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        ensure("moqui.security.UserAccount", {userId: USER}, {username: "maps.local", userFullName: "Maps local walk"})
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
        ensure(
            "moqui.security.UserPreference",
            {userId: USER, preferenceKey: "maps.profile"},
            {preferenceValue: "LOCAL_PROTOMAPS"}
        )
        if (!ec.resource.getLocationReference(LOCATION).getExists()) {
            const tile = lib.mvtTile()
            const tiles = []
            for (let z = 0; z <= 4; z++)
                for (let x = 0; x < 1 << z; x++)
                    for (let y = 0; y < 1 << z; y++) tiles.push({tileId: lib.zxyToTileId(z, x, y), bytes: tile})
            const metadata = {name: "Maps local walk", attribution: "Spec shapes", vector_layers: [{id: "earth"}, {id: "water"}]}
            lib.put(LOCATION, lib.archive(tiles, {center: [11.25, 43.77, 3], metadata}).bytes)
        }
        // registered and enabled as an administrator does it, through the services
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.enableAuthz()
        try {
            for (const layerId of ["LOCAL_PROTOMAPS_LIGHT", "LOCAL_PROTOMAPS_DARK"])
                results[layerId] = ec.service
                    .sync()
                    .name("org.moqui.maps.MapSourceServices.register#ArchiveLayer")
                    .parameters({layerId, resourceLocation: LOCATION})
                    .call()
            results.profile = ec.service
                .sync()
                .name("org.moqui.maps.MapServices.store#ProfileSettings")
                .parameters({profileId: "LOCAL_PROTOMAPS", enabled: "Y"})
                .call()
        } finally {
            ec.artifactExecution.disableAuthz()
        }
    }
})

feature("maps.local, the registered archive and the enabled local profile are there", {
    expect() {
        results.LOCAL_PROTOMAPS_LIGHT.applied == true
        results.LOCAL_PROTOMAPS_DARK.applied == true
        results.profile.applied == true
        !!one("moqui.maps.MapLayer", {layerId: "LOCAL_PROTOMAPS_LIGHT"}).sourceRevision
        one("moqui.maps.MapProfile", {profileId: "LOCAL_PROTOMAPS"}).disabled == "N"
    }
})
