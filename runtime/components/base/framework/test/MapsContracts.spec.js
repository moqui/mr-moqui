// The Maps contracts of P0 (plans/archive/Maps.md §3.2, §4.2): the maps conf element's defaults and an override of
// maps@max-features bounding a layer query; the adapter registry (org.moqui.maps.MapServices find#MapAdapters,
// get#AdapterService), which logs and leaves out a row naming a missing service or one that does not implement its
// interface rather than failing; the dialog's grant (MAPS_DIALOG), which a user with no group but ALL_USERS holds while
// the Maps app (MAPS_APP) stays closed to them; and the seeded configuration of a fresh install, which makes no outside
// request: the OpenFreeMap provider and profile disabled, the default profile drawn from this server only.
// Run: ./moq test framework --spec MapsContracts
const S = "org.moqui.maps.MapServices."
const ADAPTER = "moqui.maps.MapAdapter"
const OK = "MAPS_SPEC_OK"
const MISSING = "MAPS_SPEC_MISSING"
const READ_ONLY = "MAPS_SPEC_READ_ONLY"
const NOT_IMPLEMENTED = "MAPS_SPEC_NOT_IMPLEMENTED"
const USER = "MAPS_SPEC_USER"
const USERNAME = "maps.spec.user"

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
const outside = text => /^\s*(https?:)?\/\//i.test(String(text || ""))

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        sys(() => {
            // the interface definitions stand in for an adapter's services: they exist, which is all the registry checks
            ec.entity
                .makeValue(ADAPTER)
                .setAll({
                    adapterKey: OK,
                    description: "Spec adapter",
                    contextServiceName: S + "get#RecordMapContext",
                    storeServiceName: S + "store#RecordLocation",
                    featuresServiceName: S + "find#LayerFeatures",
                    sequenceNum: 1
                })
                .create()
            ec.entity
                .makeValue(ADAPTER)
                .setAll({adapterKey: MISSING, contextServiceName: "spec.MapsMissing.get#Nothing", sequenceNum: 2})
                .create()
            ec.entity
                .makeValue(ADAPTER)
                .setAll({adapterKey: READ_ONLY, contextServiceName: S + "get#RecordMapContext", sequenceNum: 3})
                .create()
            // a service that exists but takes none of get#RecordMapContext's parameters
            ec.entity
                .makeValue(ADAPTER)
                .setAll({adapterKey: NOT_IMPLEMENTED, contextServiceName: S + "find#MapAdapters", sequenceNum: 4})
                .create()
            ec.entity
                .makeValue("moqui.security.UserAccount")
                .setAll({userId: USER, username: USERNAME, userFullName: "Maps spec user"})
                .create()
        })
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        sys(() => {
            ec.entity.find(ADAPTER).condition("adapterKey", "in", [OK, MISSING, READ_ONLY, NOT_IMPLEMENTED]).deleteAll()
            ec.entity.find("moqui.security.UserAccount").condition("userId", USER).deleteAll()
        })
    }
})

feature("the maps settings default as the conf writes them, and maps@max-features bounds a layer query", {
    when() {
        const conf = name => ec.factory.getConfValue("maps@" + name)
        const limit = asked => invoke(S + "get#AdapterService", {adapterKey: OK, operation: "features", limit: asked}).limit
        return {
            externalRequests: conf("external-requests"),
            importMaxBytes: conf("import-max-bytes"),
            maxFeatures: conf("max-features"),
            unasked: limit(null),
            tooMany: limit(9000),
            fewer: limit(20),
            lowered: withConf("maps@max-features", "3", () => ({
                value: conf("max-features"),
                tooMany: limit(20),
                none: limit(null)
            })),
            off: withConf("maps@max-features", "0", () => limit(20)),
            malformed: withConf("maps@max-features", "5000abc", () =>
                invoke(S + "get#AdapterService", {adapterKey: OK, operation: "features", limit: 20})
            )
        }
    },
    then({externalRequests, importMaxBytes, maxFeatures, unasked, tooMany, fewer, lowered, off, malformed}) {
        externalRequests == "true"
        importMaxBytes == "536870912"
        maxFeatures == "5000"
        unasked == 5000
        tooMany == 5000
        fewer == 20
        lowered.value == "3"
        lowered.tooMany == 3
        lowered.none == 3
        off == 0
        malformed.error.includes("maps@max-features is not a whole number")
        malformed.limit == null
    }
})

feature("a registered adapter resolves by its key, and a row naming a missing service is left out without failing", {
    when() {
        const listed = invoke(S + "find#MapAdapters", {})
        const keys = (listed.adapterList || []).map(adapter => adapter.adapterKey)
        return {
            listed,
            keys,
            readOnly: (listed.adapterList || []).find(adapter => adapter.adapterKey === READ_ONLY),
            context: invoke(S + "get#AdapterService", {adapterKey: OK, operation: "context"}),
            missing: invoke(S + "get#AdapterService", {adapterKey: MISSING, operation: "context"}),
            noStore: invoke(S + "get#AdapterService", {adapterKey: READ_ONLY, operation: "store"})
        }
    },
    then({listed, keys, readOnly, context, missing, noStore}) {
        listed.error == ""
        keys.includes(OK) && keys.includes(READ_ONLY)
        !keys.includes(MISSING)
        !keys.includes(NOT_IMPLEMENTED)
        readOnly.operations.join(",") == "context"
        context.error == ""
        context.serviceName == S + "get#RecordMapContext"
        context.adapter.adapterKey == OK
        context.adapter.operations.join(",") == "context,store,features"
        context.limit == null
        missing.error.includes("No map adapter is registered as " + MISSING)
        noStore.error.includes("does not offer store")
    }
})

feature("a user with no group but ALL_USERS opens the dialog and resolves an adapter (MAPS_DIALOG), not the Maps app", {
    when() {
        ec.user.internalLoginUser(USERNAME)
        try {
            const permitted = location => ec.artifactExecution.isPermitted(location, "AT_XML_SCREEN", "AUTHZA_VIEW")
            return {
                resolved: invoke(S + "get#AdapterService", {adapterKey: OK, operation: "features", limit: 10}),
                dialog: permitted("component://tools/screen/Maps/LocalMap.xml"),
                app: permitted("component://tools/screen/Maps.xml")
            }
        } finally {
            ec.user.internalLoginUser("john.doe")
        }
    },
    then({resolved, dialog, app}) {
        resolved.error == ""
        resolved.serviceName == S + "find#LayerFeatures"
        resolved.limit == 10
        dialog == true
        app == false
    }
})

feature("a fresh install makes no outside request: OpenFreeMap is disabled and the default profile is drawn from this server", {
    when() {
        const preference = one("moqui.security.UserGroupPreference", {userGroupId: "ALL_USERS", preferenceKey: "maps.profile"})
        const profile = one("moqui.maps.MapProfile", {profileId: preference.preferenceValue})
        const layerIds = [profile.basemapLayerId].concat(
            all("moqui.maps.MapProfileLayer", {profileId: profile.profileId}).map(row => row.layerId)
        )
        const layers = layerIds.map(layerId => one("moqui.maps.MapLayer", {layerId}))
        return {
            profile,
            layers,
            freeMap: one("moqui.maps.MapProvider", {providerId: "OPENFREEMAP"}),
            freeMapProfile: one("moqui.maps.MapProfile", {profileId: "OPENFREEMAP"})
        }
    },
    then({profile, layers, freeMap, freeMapProfile}) {
        profile.profileId == "OUTLINE"
        profile.externalAccess == "N"
        profile.disabled != "Y"
        layers.length == 1
        layers[0].sourceUrl == "/libs/naturalearth/countries-110m.json"
        layers.every(layer => !layer.providerId && ![layer.sourceUrl, layer.styleLocation, layer.assetsLocation].some(outside))
        freeMap.disabled == "Y"
        freeMap.endpointUrl == "https://tiles.openfreemap.org/styles/liberty"
        freeMap.attribution.includes("OpenStreetMap")
        freeMapProfile.disabled == "Y"
    }
})
