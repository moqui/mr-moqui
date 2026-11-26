// The map configuration of P1 (plans/Maps.md §3.2, §4.2; org.moqui.maps.MapServices, MapConfig.js and MapProviders.js): the
// effective configuration's precedence (the installation's default, then the user's preference, then the consumer's
// profile, then a record's place for the viewport), its fallbacks with an explanation (a profile whose provider is
// disabled, a disabled, removed or unpermitted layer, a stale layer of the preferences, a missing default with the
// administrator's notice), an outside URL only of a tested provider on its own origin, outside requests off (no outside
// URL, the outline with a notice), a disabled provider offered nowhere and refused as a preference, saved views changed by
// their owner only, a record adapter's refusal, failure and missing grant as layer errors, a hidden record layer not asked
// and a shown one cut to maps@max-features; the grants (an ALL_USERS-only user draws a map and changes nothing); the
// mapProvider connection kind (its style test through the HTTP fixtures, no redirect followed, an archive with no URL,
// enabled only after a passing test, an endpoint change clearing the key, refused with outside requests off and to a
// non-administrator); a record's link resolved by a MapRoute of the host app, else of no app, for a holder of its screen,
// and left out for a non-holder; and the Maps readiness checks, the page policy read from the header and webapp@csp-append.
// The adapters are test/service/org/moqui/mapstest's.
// Run: ./moq test framework --spec MapsConfig
const S = "org.moqui.maps.MapServices."
const C = "org.moqui.admin.ConfigurationServices."
const http = ec.factory.getTool("HttpFixtures")
const users = {user: "MAPS_CFG_USER", other: "MAPS_CFG_OTHER", manager: "MAPS_CFG_MANAGER", plain: "MAPS_CFG_PLAIN"}
const managerGroup = "MAPS_CFG_MANAGERS"
const managerPassword = "Maps-cfg-manage-5!"
const apiKey = "maps-cfg-key-8w3p"
const ADAPTER = "MAPS_CFG_ADAPTER"
const UNGRANTED = "MAPS_CFG_UNGRANTED"
const GRANT = "MAPS_CFG_GRANT"
const routes = ["MAPS_CFG_ROUTE", "MAPS_CFG_ROUTE_GAP", "MAPS_CFG_ROUTE_HOST", "MAPS_CFG_ROUTE_OTHER"]
const providers = ["MAPS_CFG_ON", "MAPS_CFG_OFF", "MAPS_CFG_KEYED", "MAPS_CFG_TEST", "MAPS_CFG_FRESH", "MAPS_CFG_ARCH"]
const layers = [
    "MAPS_CFG_BASE_ON",
    "MAPS_CFG_BASE_OFF",
    "MAPS_CFG_BASE_KEYED",
    "MAPS_CFG_LOCAL",
    "MAPS_CFG_REMOTE",
    "MAPS_CFG_DISABLED",
    "MAPS_CFG_PERM",
    "MAPS_CFG_RECORDS",
    "MAPS_CFG_RECORDS_HIDDEN",
    "MAPS_CFG_LOOSE",
    "MAPS_CFG_ELSEWHERE",
    "MAPS_CFG_UNTESTED"
]
const profiles = ["MAPS_CFG_PROFILE", "MAPS_CFG_PROFILE_OFF", "MAPS_CFG_PROFILE_LOCAL"]
const style = (sprite, host = "https://style.cfg.example") =>
    JSON.stringify({
        version: 8,
        sources: {openmaptiles: {type: "vector", url: host + "/planet"}},
        sprite,
        glyphs: host + "/fonts/{fontstack}/{range}.pbf",
        layers: [{id: "background", type: "background"}]
    })

const username = userId => userId.toLowerCase().replace(/_/g, ".")
const make = (entity, fields) => ec.entity.makeValue(entity).setAll(fields).create()
const one = (entity, fields) => ec.entity.find(entity).condition(fields).disableAuthz().one()
function as(userId, action, recent = false) {
    ec.user.internalLoginUser(userId ? username(userId) : "john.doe")
    if (recent)
        ec.service
            .sync()
            .name("org.moqui.sso.RecentAuthServices.reauthenticate#User")
            .parameters({password: managerPassword})
            .disableAuthz()
            .call()
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
// a call's result, or its error text with the messages cleared
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
const effective = (params = {}) => invoke(S + "get#EffectiveConfig", params).config || {}
const prefs = params => invoke(S + "store#Preferences", params)
const ids = list => (list || []).map(row => row.id)
const codes = list => (list || []).map(row => row.reasonCode)
const clearPreferences = userId =>
    ec.entity
        .find("moqui.security.UserPreference")
        .condition("userId", userId)
        .condition("preferenceKey", "like", "maps.%")
        .disableAuthz()
        .deleteAll()
const connection = key => invoke(C + "get#Connection", {adapterKey: "mapProvider", connectionKey: key})
const storeConnection = (key, values, extra = {}) =>
    invoke(
        C + "store#Connection",
        Object.assign(
            {adapterKey: "mapProvider", connectionKey: key, values, expectedRevision: connection(key).connection?.revision},
            extra
        )
    )

spec({
    setupSpec() {
        http.enable()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const userId of Object.values(users))
            make("moqui.security.UserAccount", {userId, username: username(userId), userFullName: "Maps config " + userId})
        for (const userId of [users.user, users.other])
            make("moqui.security.UserGroupMember", {userGroupId: "MAPS_USERS", userId, fromDate: 0})
        make("moqui.security.UserGroup", {userGroupId: managerGroup, description: "Maps config managers"})
        for (const userPermissionId of ["ADMIN_CONNECTION_VIEW", "ADMIN_CONNECTION_MANAGE"])
            make("moqui.security.UserGroupPermission", {userGroupId: managerGroup, userPermissionId, fromDate: 0})
        make("moqui.security.UserGroupMember", {userGroupId: managerGroup, userId: users.manager, fromDate: 0})
        const account = ec.entity.find("moqui.security.UserAccount").condition("userId", users.manager).one()
        account.setFields(
            {
                currentPassword: ec.user.hashPassword(managerPassword),
                passwordHashType: "argon2",
                passwordSalt: "",
                passwordBase64: "N",
                disabled: "N",
                requirePasswordChange: "N"
            },
            true,
            null,
            null
        )
        account.update()
        make("moqui.security.UserPermission", {userPermissionId: "MAPS_CFG_PERMISSION", description: "Maps config spec"})

        const provider = (providerId, fields) =>
            make(
                "moqui.maps.MapProvider",
                Object.assign(
                    {
                        providerId,
                        providerName: providerId,
                        providerKindEnumId: "MPK_VECTOR_STYLE",
                        attribution: "Spec tiles",
                        allowPrivateNetwork: "N"
                    },
                    fields
                )
            )
        provider("MAPS_CFG_ON", {endpointUrl: "https://tiles.cfg.example/styles/base", disabled: "N"})
        provider("MAPS_CFG_OFF", {endpointUrl: "https://off.cfg.example/styles/base", disabled: "Y"})
        provider("MAPS_CFG_KEYED", {endpointUrl: "https://keyed.cfg.example/styles/base", disabled: "N", apiKey})
        provider("MAPS_CFG_TEST", {endpointUrl: "https://style.cfg.example/styles/liberty", disabled: "Y", apiKey})
        provider("MAPS_CFG_FRESH", {endpointUrl: "https://fresh.cfg.example/styles/base", disabled: "N"})
        make("moqui.maps.MapAdapter", {
            adapterKey: UNGRANTED,
            description: "Spec ungranted",
            sequenceNum: 2,
            contextServiceName: "org.moqui.mapstest.MapsTestServices.get#UngrantedContext"
        })
        make("moqui.maps.MapAdapter", {
            adapterKey: ADAPTER,
            description: "Spec places",
            sequenceNum: 1,
            contextServiceName: "org.moqui.mapstest.MapsTestServices.get#RecordMapContext",
            featuresServiceName: "org.moqui.mapstest.MapsTestServices.find#LayerFeatures"
        })
        // where the spec adapter's link (route specRecord) goes, each screen the Maps app's own grant: of no app, a row naming a
        // parameter the link has not (passed over) and one that resolves it; and two apps' own rows, the other app's first by
        // priority, which a map of the host app or of none never takes
        const route = (mapRouteId, priority, urlTemplate, routeApp) =>
            make("moqui.maps.MapRoute", {
                mapRouteId,
                routeKey: "specRecord",
                priority,
                urlTemplate,
                routeApp,
                screenLocation: "component://tools/screen/Maps.xml"
            })
        route(routes[1], 1, "/apps/maps/Gap?x={missing}", null)
        route(routes[0], 2, "/apps/maps/Browse?recordId={recordId}", null)
        route(routes[2], 5, "/apps/spechost/Record?recordId={recordId}", "specHost")
        route(routes[3], 0, "/apps/specother/Record?recordId={recordId}", "specOther")
        const layer = (layerId, fields) => make("moqui.maps.MapLayer", Object.assign({layerId, layerName: layerId}, fields))
        layer("MAPS_CFG_BASE_ON", {providerId: "MAPS_CFG_ON", formatEnumId: "MLF_VECTOR_STYLE", isBasemap: "Y"})
        layer("MAPS_CFG_BASE_OFF", {providerId: "MAPS_CFG_OFF", formatEnumId: "MLF_VECTOR_STYLE", isBasemap: "Y"})
        layer("MAPS_CFG_BASE_KEYED", {providerId: "MAPS_CFG_KEYED", formatEnumId: "MLF_VECTOR_STYLE", isBasemap: "Y"})
        layer("MAPS_CFG_LOCAL", {formatEnumId: "MLF_GEOJSON", sourceUrl: "/spec/maps/parks.json"})
        layer("MAPS_CFG_REMOTE", {
            providerId: "MAPS_CFG_ON",
            formatEnumId: "MLF_GEOJSON",
            sourceUrl: "https://tiles.cfg.example/data/trails.json"
        })
        // an outside host with no provider, another origin than its provider's, and an untested provider's
        layer("MAPS_CFG_LOOSE", {formatEnumId: "MLF_GEOJSON", sourceUrl: "https://data.cfg.example/trails.json"})
        layer("MAPS_CFG_ELSEWHERE", {
            providerId: "MAPS_CFG_ON",
            formatEnumId: "MLF_GEOJSON",
            sourceUrl: "https://other.cfg.example/x.json"
        })
        layer("MAPS_CFG_UNTESTED", {
            providerId: "MAPS_CFG_FRESH",
            formatEnumId: "MLF_GEOJSON",
            sourceUrl: "https://fresh.cfg.example/x.json"
        })
        layer("MAPS_CFG_DISABLED", {formatEnumId: "MLF_GEOJSON", sourceUrl: "/spec/maps/old.json", disabled: "Y"})
        layer("MAPS_CFG_PERM", {
            formatEnumId: "MLF_GEOJSON",
            sourceUrl: "/spec/maps/private.json",
            viewPermission: "MAPS_CFG_PERMISSION"
        })
        layer("MAPS_CFG_RECORDS", {formatEnumId: "MLF_ADAPTER", adapterKey: ADAPTER})
        layer("MAPS_CFG_RECORDS_HIDDEN", {formatEnumId: "MLF_ADAPTER", adapterKey: ADAPTER})
        make("moqui.maps.MapProfile", {
            profileId: "MAPS_CFG_PROFILE",
            profileName: "Spec hosted",
            basemapLayerId: "MAPS_CFG_BASE_ON",
            centerLongitude: 10,
            centerLatitude: 50,
            zoom: 5,
            externalAccess: "Y",
            allowedOverrides: "basemap,layers,extent,units"
        })
        make("moqui.maps.MapProfile", {
            profileId: "MAPS_CFG_PROFILE_OFF",
            profileName: "Spec off",
            basemapLayerId: "MAPS_CFG_BASE_OFF",
            externalAccess: "Y"
        })
        make("moqui.maps.MapProfile", {
            profileId: "MAPS_CFG_PROFILE_LOCAL",
            profileName: "Spec local",
            basemapLayerId: "OUTLINE_NE110",
            centerLongitude: 0,
            centerLatitude: 20,
            zoom: 2,
            externalAccess: "N",
            allowedOverrides: "layers,extent"
        })
        const profileLayer = (profileId, layerId, sequenceNum, defaultVisible = "Y") =>
            make("moqui.maps.MapProfileLayer", {profileId, layerId, sequenceNum, defaultVisible})
        profileLayer("MAPS_CFG_PROFILE", "MAPS_CFG_LOCAL", 1)
        profileLayer("MAPS_CFG_PROFILE", "MAPS_CFG_DISABLED", 2)
        profileLayer("MAPS_CFG_PROFILE", "MAPS_CFG_REMOTE", 3, "N")
        profileLayer("MAPS_CFG_PROFILE_LOCAL", "MAPS_CFG_RECORDS", 1)
        profileLayer("MAPS_CFG_PROFILE_LOCAL", "MAPS_CFG_RECORDS_HIDDEN", 2, "N")
        // the adapter's own grant, from its component, as a real adapter's
        make("moqui.security.ArtifactGroup", {artifactGroupId: GRANT, description: "Maps config spec adapter"})
        make("moqui.security.ArtifactGroupMember", {
            artifactGroupId: GRANT,
            artifactName: "org\\.moqui\\.mapstest\\.MapsTestServices\\.(get#RecordMapContext|find#LayerFeatures)",
            artifactTypeEnumId: "AT_SERVICE",
            nameIsPattern: "Y",
            inheritAuthz: "N"
        })
        make("moqui.security.ArtifactAuthz", {
            artifactAuthzId: GRANT,
            userGroupId: "ALL_USERS",
            artifactGroupId: GRANT,
            authzTypeEnumId: "AUTHZT_ALLOW",
            authzActionEnumId: "AUTHZA_ALL"
        })
        // the hosted provider passes its test, as an administrator's would: a map draws an outside URL only of a tested one
        http.add({
            method: "GET",
            url: "https://tiles.cfg.example/styles/base",
            status: 200,
            contentType: "application/json",
            body: style("https://tiles.cfg.example/sprites/base", "https://tiles.cfg.example"),
            times: 1
        })
        const tested = as(users.manager, () =>
            invoke(C + "test#Connection", {adapterKey: "mapProvider", connectionKey: "MAPS_CFG_ON"})
        )
        if (!tested.passed) throw new Error("The spec provider's test did not pass: " + JSON.stringify(tested))
        http.clear()
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        http.reset()
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const del = (entity, field, values) => ec.entity.find(entity).condition(field, "in", values).deleteAll()
        del("moqui.maps.MapSavedView", "ownerUserId", Object.values(users))
        del("moqui.security.UserPreference", "userId", Object.values(users))
        del("moqui.server.ConnectionCheck", "connectionKey", providers)
        del("moqui.security.ArtifactAuthz", "artifactAuthzId", [GRANT])
        del("moqui.security.ArtifactGroupMember", "artifactGroupId", [GRANT])
        del("moqui.security.ArtifactGroup", "artifactGroupId", [GRANT])
        del("moqui.maps.MapProfileLayer", "profileId", profiles)
        del("moqui.maps.MapProfile", "profileId", profiles)
        del("moqui.maps.MapLayer", "layerId", layers)
        del("moqui.maps.MapAdapter", "adapterKey", [ADAPTER, UNGRANTED])
        del("moqui.maps.MapRoute", "mapRouteId", routes)
        del("moqui.maps.MapProvider", "providerId", providers)
        del("moqui.security.UserGroupMember", "userId", Object.values(users))
        del("moqui.security.UserGroupPermission", "userGroupId", [managerGroup])
        del("moqui.security.UserGroup", "userGroupId", [managerGroup])
        del("moqui.security.UserPermission", "userPermissionId", ["MAPS_CFG_PERMISSION"])
        del("moqui.security.UserAccount", "userId", Object.values(users))
    }
})

feature("the installation's default, then the user's preference, then the consumer's profile; a record's place sets the view", {
    when() {
        clearPreferences(users.user)
        return as(users.user, () => {
            const fresh = effective()
            const preferred = prefs({profileId: "MAPS_CFG_PROFILE"})
            const hosted = effective()
            prefs({extent: [-10, 40, 0, 50], layers: {MAPS_CFG_REMOTE: true}})
            const personal = effective()
            const consumer = effective({profileId: "OUTLINE"})
            const record = effective({adapterKey: ADAPTER, recordKey: JSON.stringify({recordId: "LOCATED"})})
            const hostLinks = effective({
                adapterKey: ADAPTER,
                recordKey: JSON.stringify({recordId: "LOCATED"}),
                routeApp: "specHost"
            }).record
            const read = invoke(S + "get#Preferences")
            // a route's URL template, render-free: a path of this site only, each value one encoded segment or value
            const fill = ec.resource.script("component://framework/service/org/moqui/maps/MapRoutes.js").fillTemplate
            const refusedTemplates = [
                "//evil.com/x",
                "/\\evil.com",
                "javascript:alert(1)",
                "https://x",
                "/a\t/b",
                '/a"b',
                "{p}"
            ].map(template => fill(template, {p: "1"}))
            const filled = [fill("/x?id={id}", {id: "a/b?c#d&e"}), fill("/x/{id}", {id: "a/b"}), fill("/x/{id}", {id: ".."})]
            return {fresh, preferred, hosted, personal, consumer, record, hostLinks, read, refusedTemplates, filled}
        })
    },
    then({fresh, preferred, hosted, personal, consumer, record, hostLinks, read, refusedTemplates, filled}) {
        fresh.profileId == "OUTLINE"
        fresh.profile.basemaps[0].kind == "outline"
        fresh.profile.basemaps[0].url == "/libs/naturalearth/countries-110m.json"
        fresh.profile.allowedOrigins.length == 0
        // an ordinary user is told nothing of the configuration
        fresh.profile.notice == null
        fresh.fallbacks.length == 0
        preferred.applied == true
        hosted.profileId == "MAPS_CFG_PROFILE"
        hosted.profile.basemaps[0].id == "MAPS_CFG_BASE_ON"
        hosted.profile.basemaps[0].kind == "style"
        hosted.profile.basemaps[0].url == "https://tiles.cfg.example/styles/base"
        hosted.profile.allowedOrigins.includes("https://tiles.cfg.example")
        hosted.center.join(",") == "10,50"
        hosted.zoom == 5
        ids(hosted.layers).join(",") == "MAPS_CFG_LOCAL,MAPS_CFG_REMOTE"
        hosted.layers.find(layer => layer.id === "MAPS_CFG_REMOTE").visible == false
        // a provider with a key is offered as no basemap, and the key is in no answer
        !ids(hosted.profile.basemaps).includes("MAPS_CFG_BASE_KEYED")
        !ids(hosted.profile.basemaps).includes("MAPS_CFG_BASE_OFF")
        !JSON.stringify(hosted).includes(apiKey)
        personal.extent.join(",") == "-10,40,0,50"
        personal.center == null
        personal.layers.find(layer => layer.id === "MAPS_CFG_REMOTE").visible == true
        consumer.profileId == "OUTLINE"
        record.profileId == "MAPS_CFG_PROFILE"
        record.points.length == 1
        record.points[0].lon == -87.6298
        record.points[0].precisionEnumId == "GPP_POSTAL_AREA"
        record.points[0].source == "Spec data set"
        record.center.join(",") == "-87.6298,41.8781"
        record.extent == null
        record.record.recordVersion == "1"
        record.record.links[0].route == "specRecord"
        // a Maps user holds the routes' screen: with no host app the link resolves to the rows of no app, past the one it
        // cannot fill, never to an app's; in the host app's map to that app's row, whatever another app registered. The
        // link to the record shown is left out of both
        record.record.links.length == 2
        record.record.resolvedLinks.length == 1
        record.record.resolvedLinks[0].url == "/apps/maps/Browse?recordId=RELATED"
        record.record.resolvedLinks[0].label == "Spec related"
        hostLinks.resolvedLinks.map(link => link.url).join(",") == "/apps/spechost/Record?recordId=RELATED"
        refusedTemplates.every(url => url === null)
        filled.map(String).join(" ") == "/x?id=a%2Fb%3Fc%23d%26e /x/a%2Fb null"
        !record.layerErrors.some(row => row.id === "record")
        read.preferences.profileId == "MAPS_CFG_PROFILE"
        read.preferences.extent.join(",") == "-10,40,0,50"
        read.installationProfileId == "OUTLINE"
    }
})

feature("an unusable profile, a disabled, removed or unpermitted layer and a stale preference fall back with an explanation", {
    when() {
        clearPreferences(users.user)
        ec.artifactExecution.disableAuthz()
        make("moqui.security.UserPreference", {
            userId: users.user,
            preferenceKey: "maps.preferences",
            preferenceValue: JSON.stringify({layers: {MAPS_CFG_GONE: true}})
        })
        ec.artifactExecution.enableAuthz()
        const out = as(users.user, () => ({
            off: effective({profileId: "MAPS_CFG_PROFILE_OFF"}),
            hosted: effective({profileId: "MAPS_CFG_PROFILE"}),
            asked: effective({
                profileId: "MAPS_CFG_PROFILE",
                layerIds: "MAPS_CFG_LOCAL,MAPS_CFG_GONE,MAPS_CFG_PERM,MAPS_CFG_LOOSE,MAPS_CFG_ELSEWHERE,MAPS_CFG_UNTESTED"
            })
        }))
        // an administrator, who may open Configuration, is told why
        out.adminAsked = as(null, () => effective({profileId: "MAPS_CFG_PROFILE", layerIds: "MAPS_CFG_PERM"}))
        // an installation whose default profile preference is gone draws the outline, and says so to an administrator
        ec.artifactExecution.disableAuthz()
        const preference = one("moqui.security.UserGroupPreference", {userGroupId: "ALL_USERS", preferenceKey: "maps.profile"})
        preference.delete()
        clearPreferences(users.user)
        try {
            out.adminMissing = as(null, () => effective())
            out.userMissing = as(users.user, () => effective())
        } finally {
            ec.artifactExecution.disableAuthz()
            make("moqui.security.UserGroupPreference", {
                userGroupId: "ALL_USERS",
                preferenceKey: "maps.profile",
                preferenceValue: "OUTLINE"
            })
            ec.artifactExecution.enableAuthz()
        }
        return out
    },
    then({off, hosted, asked, adminAsked, adminMissing, userMissing}) {
        off.profileId == "OUTLINE"
        off.fallbacks[0].kind == "profile"
        off.fallbacks[0].id == "MAPS_CFG_PROFILE_OFF"
        off.fallbacks[0].reasonCode == "BASEMAP_UNAVAILABLE"
        off.fallbacks[0].message.includes("Spec off")
        off.profile.notice.text.includes("Spec off")
        off.profile.notice.href == ""
        hosted.profileId == "MAPS_CFG_PROFILE"
        !ids(hosted.layers).includes("MAPS_CFG_DISABLED")
        hosted.layerErrors.find(row => row.id === "MAPS_CFG_DISABLED").reasonCode == "LAYER_DISABLED"
        hosted.fallbacks.some(row => row.kind === "layer" && row.id === "MAPS_CFG_DISABLED")
        // the preferences name a layer that is gone: said, and passed over
        hosted.fallbacks.some(row => row.kind === "layer" && row.id === "MAPS_CFG_GONE" && row.reasonCode === "LAYER_MISSING")
        ids(asked.layers).join(",") == "MAPS_CFG_LOCAL"
        // one the user may not see is as one that is not there, named by its ID; an outside host is only a tested
        // provider's, on its own origin
        codes(asked.layerErrors).join(",") == "LAYER_MISSING,LAYER_MISSING,SOURCE_UNTESTED,URL_NOT_PROVIDERS,PROVIDER_UNTESTED"
        asked.layerErrors[1].label == "MAPS_CFG_PERM"
        asked.layerErrors[1].message.includes("is not registered")
        !JSON.stringify(asked.profile.allowedOrigins).includes("data.cfg.example")
        codes(adminAsked.layerErrors).join(",") == "LAYER_NOT_PERMITTED"
        adminMissing.profileId == "OUTLINE"
        adminMissing.profile.notice.text.includes("maps.profile")
        adminMissing.profile.notice.href == "/qapps/maps/Configuration"
        userMissing.profileId == "OUTLINE"
        userMissing.profile.notice == null
    }
})

feature(
    "with outside requests off no outside URL is handed out, the outline is drawn with a notice, and a disabled provider is offered nowhere",
    {
        when() {
            clearPreferences(users.user)
            return as(users.user, () => {
                const out = {options: invoke(S + "get#Preferences")}
                prefs({profileId: "MAPS_CFG_PROFILE"})
                withConf("maps@external-requests", "false", () => {
                    out.offline = effective()
                    out.local = effective({profileId: "MAPS_CFG_PROFILE_LOCAL", layerIds: "MAPS_CFG_REMOTE,MAPS_CFG_LOCAL"})
                    out.chosen = prefs({profileId: "MAPS_CFG_PROFILE"})
                })
                return out
            })
        },
        then({options, offline, local, chosen}) {
            offline.profileId == "OUTLINE"
            offline.externalRequests == false
            offline.fallbacks[0].reasonCode == "EXTERNAL_REQUESTS_OFF"
            offline.profile.notice.text.includes("Spec hosted")
            offline.profile.allowedOrigins.length == 0
            !JSON.stringify(offline).includes("cfg.example")
            local.profileId == "MAPS_CFG_PROFILE_LOCAL"
            ids(local.layers).join(",") == "MAPS_CFG_LOCAL"
            local.layerErrors[0].id == "MAPS_CFG_REMOTE"
            local.layerErrors[0].reasonCode == "EXTERNAL_REQUESTS_OFF"
            chosen.applied == false
            chosen.cause == "EXTERNAL_REQUESTS_OFF"
            options.basemapOptions.some(row => row.layerId === "MAPS_CFG_BASE_ON")
            !options.basemapOptions.some(row =>
                ["MAPS_CFG_BASE_OFF", "MAPS_CFG_BASE_KEYED", "OPENFREEMAP_LIBERTY"].includes(row.layerId)
            )
            options.profileOptions.some(row => row.profileId === "MAPS_CFG_PROFILE")
            !options.profileOptions.some(row => ["MAPS_CFG_PROFILE_OFF", "OPENFREEMAP"].includes(row.profileId))
        }
    }
)

feature("a preference cannot choose a disabled provider's basemap or profile, nor a provider's that needs a key", {
    when() {
        clearPreferences(users.user)
        return as(users.user, () => ({
            disabledBasemap: prefs({basemap: "MAPS_CFG_BASE_OFF"}),
            seededProfile: prefs({profileId: "OPENFREEMAP"}),
            offProfile: prefs({profileId: "MAPS_CFG_PROFILE_OFF"}),
            keyed: prefs({basemap: "MAPS_CFG_BASE_KEYED"}),
            unknownLayer: prefs({layers: {MAPS_CFG_GONE: true}}),
            badExtent: prefs({extent: [10, 50, 20, 40]}),
            fine: prefs({
                basemap: "MAPS_CFG_BASE_ON",
                units: "imperial",
                restoreLastView: true,
                lastView: {center: [2.35, 48.85], zoom: 11}
            }),
            read: invoke(S + "get#Preferences").preferences,
            cleared: prefs({restoreLastView: false}),
            readAfter: invoke(S + "get#Preferences").preferences
        }))
    },
    then({disabledBasemap, seededProfile, offProfile, keyed, unknownLayer, badExtent, fine, read, cleared, readAfter}) {
        disabledBasemap.applied == false
        disabledBasemap.reasonCode == "BASEMAP_UNAVAILABLE"
        disabledBasemap.cause == "PROVIDER_DISABLED"
        seededProfile.reasonCode == "PROFILE_UNAVAILABLE"
        seededProfile.cause == "PROFILE_DISABLED"
        offProfile.reasonCode == "PROFILE_UNAVAILABLE"
        offProfile.cause == "BASEMAP_UNAVAILABLE"
        keyed.cause == "PROVIDER_KEYED"
        unknownLayer.reasonCode == "LAYER_UNAVAILABLE"
        badExtent.error.includes("initial extent")
        fine.applied == true
        read.basemap == "MAPS_CFG_BASE_ON"
        read.units == "imperial"
        read.lastView.center.join(",") == "2.35,48.85"
        cleared.applied == true
        readAfter.restoreLastView == false
        readAfter.lastView == null
        readAfter.basemap == "MAPS_CFG_BASE_ON"
    }
})

feature("a saved view is its owner's: another user reads it only when shared and changes or deletes none", {
    when() {
        const view = {center: [10, 50], zoom: 6, layers: ["MAPS_CFG_LOCAL"]}
        const made = as(users.user, () =>
            invoke(S + "store#SavedView", {description: "Spec parks", profileId: "MAPS_CFG_PROFILE", view})
        )
        const privateRead = as(users.other, () =>
            invoke(S + "find#SavedViews").savedViewList.filter(row => row.savedViewId === made.savedViewId)
        )
        const shared = as(users.user, () => invoke(S + "store#SavedView", {savedViewId: made.savedViewId, isShared: "Y"}))
        const sharedRead = as(users.other, () =>
            invoke(S + "find#SavedViews").savedViewList.filter(row => row.savedViewId === made.savedViewId)
        )
        const otherChange = as(users.other, () =>
            invoke(S + "store#SavedView", {savedViewId: made.savedViewId, description: "Taken"})
        )
        const otherDelete = as(users.other, () => invoke(S + "delete#SavedView", {savedViewId: made.savedViewId}))
        const own = as(users.user, () => ({
            privateView: invoke(S + "store#SavedView", {description: "Spec private", view: {extent: "-10,40,0,50"}}),
            unpermitted: invoke(S + "store#SavedView", {
                description: "Spec hidden",
                view: {center: [0, 0], layers: ["MAPS_CFG_PERM"]}
            }),
            malformed: invoke(S + "store#SavedView", {description: "Spec bad", view: {extent: [0, 60, 10, 50]}})
        }))
        const otherPrivate = as(users.other, () => invoke(S + "delete#SavedView", {savedViewId: own.privateView.savedViewId}))
        const deleted = as(users.user, () => invoke(S + "delete#SavedView", {savedViewId: made.savedViewId}))
        return {
            made,
            privateRead,
            shared,
            sharedRead,
            otherChange,
            otherDelete,
            own,
            otherPrivate,
            deleted,
            row: one("moqui.maps.MapSavedView", {savedViewId: made.savedViewId})
        }
    },
    then({made, privateRead, shared, sharedRead, otherChange, otherDelete, own, otherPrivate, deleted, row}) {
        made.applied == true
        privateRead.length == 0
        shared.applied == true
        sharedRead.length == 1
        sharedRead[0].isOwn == false
        sharedRead[0].ownerName == "Maps config MAPS_CFG_USER"
        sharedRead[0].view.layers.join(",") == "MAPS_CFG_LOCAL"
        otherChange.applied == false
        otherChange.reasonCode == "PERMISSION_DENIED"
        otherDelete.reasonCode == "PERMISSION_DENIED"
        own.privateView.applied == true
        own.unpermitted.reasonCode == "LAYER_UNAVAILABLE"
        own.malformed.error.includes("extent")
        otherPrivate.reasonCode == "RECORD_UNAVAILABLE"
        deleted.applied == true
        row == null
    }
})

feature(
    "a record adapter's refusal and failure are layer errors, not a failed configuration, and its layer is cut to maps@max-features",
    {
        when() {
            return as(users.user, () => {
                const record = recordId =>
                    effective({
                        profileId: "MAPS_CFG_PROFILE_LOCAL",
                        layerIds: "MAPS_CFG_LOCAL",
                        adapterKey: ADAPTER,
                        recordKey: {recordId}
                    })
                return {
                    denied: record("DENIED"),
                    broken: record("BROKEN"),
                    unknown: effective({adapterKey: "MAPS_CFG_NOT_REGISTERED", recordKey: {recordId: "LOCATED"}}),
                    malformed: effective({adapterKey: ADAPTER, recordKey: "[1, 2]"}),
                    badKey: effective({adapterKey: "no such key!", recordKey: {recordId: "LOCATED"}}),
                    // an adapter its component granted to no one: as a record the user may not read, and never run
                    ungranted: effective({adapterKey: UNGRANTED, recordKey: {recordId: "LOCATED"}}),
                    features: withConf("maps@max-features", "3", () => {
                        ec.cache.getCache("moqui.test.maps.asked").clear()
                        return effective({profileId: "MAPS_CFG_PROFILE_LOCAL"})
                    }),
                    asked: ["MAPS_CFG_RECORDS", "MAPS_CFG_RECORDS_HIDDEN", "UNGRANTED"].map(
                        key => ec.cache.getCache("moqui.test.maps.asked").get(key) || ""
                    )
                }
            })
        },
        then({denied, broken, unknown, malformed, badKey, ungranted, features, asked}) {
            denied.profileId == "MAPS_CFG_PROFILE_LOCAL"
            ids(denied.layers).join(",") == "MAPS_CFG_LOCAL"
            denied.points.length == 0
            denied.record == null
            denied.layerErrors[0].id == "record"
            denied.layerErrors[0].reasonCode == "RECORD_UNAVAILABLE"
            denied.layerErrors[0].label == "Spec places"
            broken.layerErrors[0].reasonCode == "ADAPTER_FAILED"
            !JSON.stringify(broken).includes("cannot be read")
            unknown.layerErrors[0].reasonCode == "ADAPTER_UNAVAILABLE"
            malformed.layerErrors[0].reasonCode == "RECORD_KEY_INVALID"
            features.features.features.length == 3
            features.featuresTruncated == true
            features.features.features[0].properties.layerId == "MAPS_CFG_RECORDS"
            badKey.layerErrors[0].reasonCode == "ADAPTER_UNAVAILABLE"
            ungranted.layerErrors[0].reasonCode == "RECORD_UNAVAILABLE"
            ungranted.points.length == 0
            !JSON.stringify(ungranted).includes("UNGRANTED-SECRET")
            // the hidden record layer is listed, and its adapter not asked; the ungranted adapter never ran
            ids(features.recordLayers).join(",") == "MAPS_CFG_RECORDS,MAPS_CFG_RECORDS_HIDDEN"
            features.recordLayers[1].visible == false
            asked.join(",") == "Y,,"
        }
    }
)

feature(
    "the mapProvider kind: tested through the fixtures, enabled only after a passing test, an endpoint change clears the key",
    {
        when() {
            http.clear()
            const key = "MAPS_CFG_TEST"
            const out = {}
            out.listed = as(users.manager, () => invoke(C + "find#Connections", {adapterKey: "mapProvider"}).connectionList || [])
            as(
                users.manager,
                () => {
                    out.read = connection(key).connection
                    out.early = storeConnection(key, {}, {enabled: true})
                    out.offline = withConf("maps@external-requests", "false", () =>
                        invoke(C + "test#Connection", {adapterKey: "mapProvider", connectionKey: key})
                    )
                    out.offlineRequests = http.count()
                    // its sprite on another host: the map would not be allowed to reach it
                    http.add({
                        method: "GET",
                        url: "https://style.cfg.example/styles/liberty",
                        status: 200,
                        contentType: "application/json",
                        body: style("https://elsewhere.cfg.example/sprites/liberty"),
                        times: 1
                    })
                    out.strange = invoke(C + "test#Connection", {adapterKey: "mapProvider", connectionKey: key})
                    // a redirect is not followed: the test fails, naming where it pointed
                    http.add({
                        method: "GET",
                        url: "https://style.cfg.example/styles/liberty",
                        status: 302,
                        headers: {Location: "https://moved.cfg.example/styles/liberty"},
                        times: 1
                    })
                    out.redirected = invoke(C + "test#Connection", {adapterKey: "mapProvider", connectionKey: key})
                    http.add({
                        method: "GET",
                        url: "https://style.cfg.example/styles/liberty",
                        status: 200,
                        contentType: "application/json",
                        body: style("https://style.cfg.example/sprites/liberty"),
                        times: 1
                    })
                    out.passed = invoke(C + "test#Connection", {adapterKey: "mapProvider", connectionKey: key})
                    out.sent = http.requests().map(request => request.url + " " + JSON.stringify(request.headers || {}))
                    out.offlineEnable = withConf("maps@external-requests", "false", () =>
                        storeConnection(key, {}, {enabled: true})
                    )
                    out.enabled = storeConnection(key, {}, {enabled: true})
                    // an archive provider has no URL: its archives are this server's
                    out.archive = invoke(C + "store#Connection", {
                        adapterKey: "mapProvider",
                        values: {
                            connectionId: "MAPS_CFG_ARCH",
                            providerName: "Spec archives",
                            providerKindEnumId: "MPK_ARCHIVE",
                            endpointUrl: "https://archive.cfg.example/tiles"
                        }
                    })
                },
                false
            )
            out.enabledRow = one("moqui.maps.MapProvider", {providerId: key}).getMap()
            out.moved = as(
                users.manager,
                () => storeConnection(key, {endpointUrl: "https://style.cfg.example/styles/bright"}),
                true
            )
            out.movedRow = one("moqui.maps.MapProvider", {providerId: key}).getMap()
            out.outsider = as(users.user, () => connection(key))
            return out
        },
        then({
            listed,
            read,
            early,
            offline,
            offlineRequests,
            strange,
            redirected,
            archive,
            passed,
            sent,
            offlineEnable,
            enabled,
            enabledRow,
            moved,
            movedRow,
            outsider
        }) {
            // providers list with every other connection, the seeded OpenFreeMap row disabled
            listed.find(row => row.connectionKey === "OPENFREEMAP").enabled == false
            listed.some(row => row.connectionKey === "MAPS_CFG_TEST")
            read.secretList[0].name == "apiKey"
            read.secretList[0].secretSet == true
            !JSON.stringify(read).includes(apiKey)
            read.fieldList.find(field => field.name === "privateNetwork").kind == "display"
            early.applied == false
            early.reasonCode == "TEST_REQUIRED"
            offline.reasonCode == "EXTERNAL_REQUESTS_OFF"
            offline.passed == false
            offlineRequests == 0
            strange.passed == false
            strange.checkList.find(row => row.check === "Hosts").text.includes("elsewhere.cfg.example")
            redirected.passed == false
            redirected.checkList.find(row => row.check === "Style").text.includes("https://moved.cfg.example")
            archive.applied == false
            archive.reasonCode == "ARCHIVE_ENDPOINT"
            passed.passed == true
            passed.enabled == false
            // the test sends no key and no credentials
            sent.length == 3
            sent.every(line => !line.includes(apiKey) && !line.toLowerCase().includes("authorization"))
            offlineEnable.reasonCode == "EXTERNAL_REQUESTS_OFF"
            enabled.applied == true
            enabled.enabled == true
            enabledRow.disabled == "N"
            moved.applied == true
            moved.secretCleared == true
            moved.enabled == false
            movedRow.disabled == "Y"
            movedRow.apiKey == null
            outsider.error.includes("ADMIN_CONNECTION_VIEW is required")
        }
    }
)

feature("readiness: the default profile and the outline in use, an untested provider, and a page policy that blocks a provider", {
    when() {
        const check = providerKey => invoke(S + "check#Readiness", {providerKey}).checks || []
        return {
            profile: as(null, () => check("maps.profile")),
            providers: as(null, () => check("maps.providers")),
            blocked: as(null, () =>
                withConf("webapp-list/webapp[webroot]@csp-append", "connect-src 'self'", () => check("maps.providers"))
            ),
            // the header's own policy: a directive it sets is changed there, one it leaves to default-src is appended
            header: as(null, () =>
                withConf(
                    "webapp-list/webapp[webroot]/response-header[screen-render,Content-Security-Policy]@value",
                    "default-src 'self'; connect-src 'self' https://*.cfg.example:8443; frame-ancestors 'none'",
                    () => check("maps.providers")
                )
            ),
            // a wildcard host on the default port allows it
            allowed: as(null, () =>
                withConf(
                    "webapp-list/webapp[webroot]/response-header[screen-render,Content-Security-Policy]@value",
                    "default-src 'self' https://*.cfg.example",
                    () => check("maps.providers")
                )
            ),
            outsider: as(users.user, () => invoke(S + "check#Readiness", {providerKey: "maps.profile"}))
        }
    },
    then({profile, providers, blocked, header, allowed, outsider}) {
        profile.find(row => row.checkKey === "maps.profile.default").state == "ready"
        profile.find(row => row.checkKey === "maps.profile.outline").reasonCode == "OUTLINE_IN_USE"
        providers.find(row => row.checkKey === "maps.provider.MAPS_CFG_ON.test").state == "ready"
        providers.find(row => row.checkKey === "maps.provider.MAPS_CFG_FRESH.test").reasonCode == "UNTESTED"
        providers.find(row => row.checkKey === "maps.provider.MAPS_CFG_ON.csp").state == "ready"
        // a provider with a key is said not to be drawn, and its page policy is not checked
        providers.find(row => row.checkKey === "maps.provider.MAPS_CFG_KEYED.keyed").reasonCode == "PROVIDER_KEYED"
        !providers.some(row => row.checkKey === "maps.provider.MAPS_CFG_KEYED.csp")
        providers.find(row => row.checkKey === "maps.layer.MAPS_CFG_LOOSE.source").reasonCode == "SOURCE_UNTESTED"
        header
            .find(row => row.checkKey === "maps.provider.MAPS_CFG_ON.csp")
            .message.includes("https://tiles.cfg.example to connect-src of the screen-render Content-Security-Policy header")
        header
            .find(row => row.checkKey === "maps.provider.MAPS_CFG_ON.csp")
            .message.includes("https://tiles.cfg.example to img-src with webapp@csp-append")
        allowed.find(row => row.checkKey === "maps.provider.MAPS_CFG_ON.csp").state == "ready"
        blocked.find(row => row.checkKey === "maps.provider.MAPS_CFG_ON.csp").reasonCode == "CSP_BLOCKS"
        blocked
            .find(row => row.checkKey === "maps.provider.MAPS_CFG_ON.csp")
            .message.includes("https://tiles.cfg.example to connect-src with webapp@csp-append")
        !blocked.find(row => row.checkKey === "maps.provider.MAPS_CFG_ON.csp").message.includes("img-src")
        outsider.error.includes("PERMISSION_DENIED")
    }
})

feature("a user with no group but ALL_USERS draws a map and reads their preferences, and changes none of Maps' records", {
    when() {
        return as(users.plain, () => ({
            config: invoke(S + "get#EffectiveConfig", {}),
            record: effective({adapterKey: ADAPTER, recordKey: JSON.stringify({recordId: "LOCATED"})}).record,
            read: invoke(S + "get#Preferences"),
            store: invoke(S + "store#Preferences", {units: "metric"}),
            saved: invoke(S + "store#SavedView", {description: "Spec plain", view: {center: [0, 0]}})
        }))
    },
    then({config, record, read, store, saved}) {
        config.error == ""
        // the record is theirs to read, its link's screen is not: the link is left out
        record.links[0].route == "specRecord"
        record.resolvedLinks.length == 0
        config.config.profileId == "OUTLINE"
        read.error == ""
        read.installationProfileId == "OUTLINE"
        store.error.includes("not authorized") || store.error.includes("authoriz")
        store.applied == null
        saved.error.includes("authoriz")
        saved.savedViewId == null
    }
})
