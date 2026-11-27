// A map provider reached through this server (plans/archive/Maps.md §4.2, "a proxy is an explicit provider option"; MapProviders.js
// providerAsset, tools screen/Maps/LocalMap.xml providerProxy): a keyed provider with proxyRequests Y is tested as the
// proxy asks it, the key added and the paths its style names recorded; the effective configuration hands its layer the
// proxy's same-origin path; the style and its TileJSON come back rewritten to the proxy's paths with the key out of them;
// a tile is asked of the provider with the key added on the server, Cache-Control private; a path the style did not name,
// outside requests turned off and a user who may not be offered the provider's profile are refused with nothing sent.
// The provider is answered by the HTTP fixtures. Run: ./moq test framework --spec MapsProviderProxy
const C = "org.moqui.admin.ConfigurationServices."
const http = ec.factory.getTool("HttpFixtures")
const PROVIDER = "MAPS_PX_PROVIDER"
const LAYER = "MAPS_PX_LAYER"
const PROFILE = "MAPS_PX_PROFILE"
const PERMISSION = "MAPS_PX_VIEW"
const READERS = "MAPS_PX_READERS"
const MANAGERS = "MAPS_PX_MANAGERS"
const READER = "MAPS_PX_READER"
const OTHER = "MAPS_PX_OTHER"
const MANAGER = "MAPS_PX_MANAGER"
const KEY = "spec-proxy-key-7781"
const ORIGIN = "https://tiles.proxy.example"
const STYLE = ORIGIN + "/styles/basic/style.json"
const PROXY = "/apps/maps/LocalMap/providerProxy/" + PROVIDER
const style = JSON.stringify({
    version: 8,
    sources: {base: {type: "vector", url: ORIGIN + "/tiles/v3/tiles.json?key=" + KEY}},
    sprite: ORIGIN + "/sprites/basic",
    glyphs: ORIGIN + "/fonts/{fontstack}/{range}.pbf",
    layers: [{id: "earth", type: "fill", source: "base", "source-layer": "earth"}]
})
const tileJson = JSON.stringify({tilejson: "3.0.0", tiles: [ORIGIN + "/tiles/v3/{z}/{x}/{y}.pbf?key=" + KEY]})

const usernameOf = userId => userId.toLowerCase().replace(/_/g, ".")
const make = (entity, fields) => ec.entity.makeValue(entity).setAll(fields).createOrUpdate()
const providerRow = () => ec.entity.find("moqui.maps.MapProvider").condition("providerId", PROVIDER).useCache(false).one()
// a change committed by itself, so the entity cache the proxy reads its rows through has it
const change = (entity, fields) =>
    ec.transaction.runRequireNew(10, "Changing a proxy spec row", () => ec.entity.makeValue(entity).setAll(fields).update())
// a change of a layer, a profile or a permission reaches the proxy within its 30-second decision cache: cleared here
const fresh = () => ec.cache.getCache("maps.provider.proxy").clear()
const retest = () =>
    as(MANAGER, () =>
        ec.service
            .sync()
            .name(C + "test#Connection")
            .parameters({adapterKey: "mapProvider", connectionKey: PROVIDER})
            .call()
    )
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
// the proxy transition as a user: status, output and Cache-Control
const proxy = (userId, path, parameters) =>
    as(userId, () => {
        const res = ec.screen
            .makeTest()
            .rootScreen("component://webroot/screen/webroot.xml")
            .render("apps/maps/LocalMap/providerProxy/" + PROVIDER + path, parameters || {}, "get")
        return {
            status: res.status,
            output: String(res.output || ""),
            cache: res.header("Cache-Control"),
            type: res.header("Content-Type")
        }
    })
const json = text => {
    try {
        return JSON.parse(text)
    } catch (e) {
        return {}
    }
}
// the provider's answers; first: answers matched before them (a fixture answers in the order added)
const fixtures = (first = []) => {
    http.clear()
    for (const fixture of first) http.add(Object.assign({method: "GET", times: 1}, fixture))
    http.add({method: "GET", url: ORIGIN + "/sprites/basic.json*", contentType: "application/json", body: "{}"})
    http.add({method: "GET", url: STYLE + "*", contentType: "application/json", body: style})
    http.add({method: "GET", url: ORIGIN + "/styles/dark/style.json*", contentType: "application/json", body: style})
    http.add({method: "GET", url: ORIGIN + "/tiles/v3/tiles.json*", contentType: "application/json", body: tileJson})
    http.add({
        method: "GET",
        url: ORIGIN + "/tiles/v3/1/0/0.pbf*",
        contentType: "application/x-protobuf",
        body: Uint8Array.from([0x1a, 0x03, 0x0a, 0x01, 0x65])
    })
}

spec({
    setupSpec() {
        http.enable()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const now = ec.user.nowTimestamp
        for (const userId of [READER, OTHER, MANAGER])
            make("moqui.security.UserAccount", {userId, username: usernameOf(userId), userFullName: "Maps proxy " + userId})
        make("moqui.security.UserPermission", {userPermissionId: PERMISSION, description: "Maps proxy spec: the profile"})
        make("moqui.security.UserGroup", {userGroupId: READERS, description: "Maps proxy readers"})
        make("moqui.security.UserGroupPermission", {userGroupId: READERS, userPermissionId: PERMISSION, fromDate: now})
        make("moqui.security.UserGroupMember", {userGroupId: READERS, userId: READER, fromDate: now})
        make("moqui.security.UserGroup", {userGroupId: MANAGERS, description: "Maps proxy managers"})
        for (const userPermissionId of ["ADMIN_CONNECTION_VIEW", "ADMIN_CONNECTION_MANAGE"])
            make("moqui.security.UserGroupPermission", {userGroupId: MANAGERS, userPermissionId, fromDate: now})
        make("moqui.security.UserGroupMember", {userGroupId: MANAGERS, userId: MANAGER, fromDate: now})
        make("moqui.maps.MapProvider", {
            providerId: PROVIDER,
            providerName: "Spec keyed tiles",
            providerKindEnumId: "MPK_VECTOR_STYLE",
            endpointUrl: STYLE,
            attribution: "Spec tiles",
            apiKey: KEY,
            proxyRequests: "Y",
            cachePolicy: "private",
            cacheSeconds: 600,
            allowPrivateNetwork: "N",
            disabled: "N"
        })
        make("moqui.maps.MapLayer", {
            layerId: LAYER,
            layerName: "Spec keyed basemap",
            providerId: PROVIDER,
            formatEnumId: "MLF_VECTOR_STYLE",
            styleLocation: STYLE,
            isBasemap: "Y"
        })
        // a second layer of the provider, its dark style, in no profile: the provider's test records its paths too
        make("moqui.maps.MapLayer", {
            layerId: LAYER + "_DARK",
            layerName: "Spec keyed dark",
            providerId: PROVIDER,
            formatEnumId: "MLF_VECTOR_STYLE",
            styleLocation: ORIGIN + "/styles/dark/style.json",
            isBasemap: "Y"
        })
        make("moqui.maps.MapProfile", {
            profileId: PROFILE,
            profileName: "Spec keyed",
            basemapLayerId: LAYER,
            viewPermission: PERMISSION,
            externalAccess: "Y",
            centerLongitude: 0,
            centerLatitude: 0,
            zoom: 2
        })
        ec.message.clearErrors()
    },
    cleanupSpec() {
        http.reset()
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const del = (entity, field, values) => ec.entity.find(entity).condition(field, "in", values).deleteAll()
        del("moqui.maps.MapProfile", "profileId", [PROFILE])
        del("moqui.maps.MapLayer", "layerId", [LAYER, LAYER + "_DARK"])
        del("moqui.server.ConnectionCheck", "connectionKey", [PROVIDER])
        del("moqui.maps.MapProvider", "providerId", [PROVIDER])
        del("moqui.security.UserGroupMember", "userId", [READER, MANAGER])
        del("moqui.security.UserGroupPermission", "userGroupId", [READERS, MANAGERS])
        del("moqui.security.UserGroup", "userGroupId", [READERS, MANAGERS])
        del("moqui.security.UserPermission", "userPermissionId", [PERMISSION])
        del("moqui.security.UserAccount", "userId", [READER, OTHER, MANAGER])
    }
})

feature("the test asks with the key and records the exact rules its style and layers name; the map gets a rewritten style", {
    when() {
        fixtures()
        const tested = retest()
        const asked = http.requests().map(row => String(row.url))
        const paths = String(providerRow().proxyPaths || "")
        const config = as(READER, () =>
            ec.service.sync().name("org.moqui.maps.MapServices.get#EffectiveConfig").parameters({profileId: PROFILE}).call()
        )?.config
        fixtures()
        return {
            tested,
            asked,
            paths,
            basemap: config?.profile?.basemaps?.[0] || {},
            origins: config?.profile?.allowedOrigins || [],
            style: proxy(READER, "/styles/basic/style.json"),
            tileJson: proxy(READER, "/tiles/v3/tiles.json"),
            sprite: proxy(READER, "/sprites/basic.json")
        }
    },
    then({tested, asked, paths, basemap, origins, style, tileJson, sprite}) {
        tested.passed == true
        asked.length >= 3
        asked.every(url => url.includes("key=" + KEY))
        // exact rules: documents and the sprite by their paths, tiles by their template, never a directory or "/"
        paths.split("\n").sort().join("|") ==
            [
                "doc /styles/basic/style.json",
                "doc /styles/dark/style.json",
                "doc /tiles/v3/tiles.json",
                "sprite /sprites/basic",
                "tile /tiles/v3/{z}/{x}/{y}.pbf"
            ].join("|")
        basemap.url == PROXY + "/styles/basic/style.json"
        !JSON.stringify(basemap).includes(KEY)
        !origins.includes(ORIGIN)
        style.status == 200
        style.cache == "private, max-age=600"
        json(style.output).sources.base.url == PROXY + "/tiles/v3/tiles.json"
        json(style.output).sprite == PROXY + "/sprites/basic"
        json(style.output).glyphs === undefined
        !style.output.includes(KEY)
        json(tileJson.output).tiles[0] == PROXY + "/tiles/v3/{z}/{x}/{y}.pbf"
        !tileJson.output.includes(KEY)
        sprite.status == 200
    }
})

feature("a tile is asked with the key added on the server, the browser's own key parameter dropped; or as a header", {
    when() {
        fixtures()
        const tile = proxy(READER, "/tiles/v3/1/0/0.pbf", {key: "browser-key", extra: "1"})
        const url = http.count() ? String(http.last().url) : ""
        // the key as a header: a new test, then the tile asked with the header and no key in its URL
        change("moqui.maps.MapProvider", {providerId: PROVIDER, apiKeyParameter: "header:X-Api-Key"})
        fixtures()
        const headerTested = retest()
        // the documents still name the key as key=: never kept in the rules nor handed to the browser
        const headerPaths = String(providerRow().proxyPaths || "")
        fresh()
        fixtures()
        const headerTile = proxy(READER, "/tiles/v3/1/0/0.pbf")
        const headerRequest = http.count() ? http.last() : {}
        change("moqui.maps.MapProvider", {providerId: PROVIDER, apiKeyParameter: null})
        fixtures()
        retest()
        fresh()
        return {
            headerPaths,
            tile,
            url,
            headerTested,
            headerTile,
            headerUrl: String(headerRequest.url || ""),
            headers: headerRequest.headers || {}
        }
    },
    then({headerPaths, tile, url, headerTested, headerTile, headerUrl, headers}) {
        tile.status == 200
        tile.type.startsWith("application/x-protobuf")
        tile.cache == "private, max-age=600"
        url == ORIGIN + "/tiles/v3/1/0/0.pbf?key=" + KEY
        headerTested.passed == true
        headerPaths.includes("tile /tiles/v3/{z}/{x}/{y}.pbf") && !headerPaths.includes(KEY)
        headerTile.status == 200
        headerUrl == ORIGIN + "/tiles/v3/1/0/0.pbf"
        Object.keys(headers).some(name => name.toLowerCase() == "x-api-key" && headers[name] == "[redacted]")
    }
})

feature("only a recorded rule's path is forwarded: another path, a near one, a climbing one and a wrong tile are refused", {
    when() {
        fixtures()
        return {
            outside: proxy(READER, "/admin/settings.json"),
            nearSprite: proxy(READER, "/sprites/basicXYZ.json"),
            spritePath: proxy(READER, "/sprites/basic"),
            climbing: proxy(READER, "/tiles/v3/../../admin.json"),
            letters: proxy(READER, "/tiles/v3/a/0/0.pbf"),
            extension: proxy(READER, "/tiles/v3/1/0/0.png"),
            deeper: proxy(READER, "/tiles/v3/1/0/0/0.pbf"),
            sent: http.count()
        }
    },
    then(out) {
        out.outside.status == 404
        out.nearSprite.status == 404
        out.spritePath.status == 404
        out.climbing.status != 200
        out.letters.status == 404
        out.extension.status == 404
        out.deeper.status == 404
        out.sent == 0
    }
})

feature(
    "the provider's answer is refused: another origin or a climbing path in a document, a page or SVG, a redirect, too large",
    {
        when() {
            const foreign = JSON.stringify({
                version: 8,
                sources: {x: {type: "vector", tiles: ["https://evil.example/{z}/{x}/{y}.pbf"]}},
                layers: []
            })
            const climbing = JSON.stringify({version: 8, sources: {}, sprite: "../../admin/sprite", layers: []})
            const tile = ORIGIN + "/tiles/v3/1/0/0.pbf*"
            const answer = first => {
                fixtures(first)
                return first[0].url === tile
                    ? proxy(READER, "/tiles/v3/1/0/0.pbf").status
                    : proxy(READER, "/styles/basic/style.json").status
            }
            return {
                foreign: answer([{url: STYLE + "*", contentType: "application/json", body: foreign}]),
                climbing: answer([{url: STYLE + "*", contentType: "application/json", body: climbing}]),
                html: answer([{url: tile, contentType: "text/html", body: "<script>alert(1)</script>"}]),
                svg: answer([{url: tile, contentType: "image/svg+xml", body: "<svg/>"}]),
                redirect: answer([{url: tile, status: 302, headers: {Location: "https://evil.example/"}, body: ""}]),
                large: answer([{url: tile, contentType: "application/x-protobuf", body: new Uint8Array(4 * 1024 * 1024 + 1)}])
            }
        },
        then(out) {
            out.foreign == 502
            out.climbing == 502
            out.html == 502
            out.svg == 502
            out.redirect == 502
            out.large == 502
        }
    }
)

feature("outside requests off, a user not offered the profile, a disabled profile or layer and its viewPermission refuse it", {
    when() {
        fixtures()
        const out = {
            offline: withConf("maps@external-requests", "false", () => proxy(READER, "/tiles/v3/1/0/0.pbf")),
            other: proxy(OTHER, "/tiles/v3/1/0/0.pbf")
        }
        change("moqui.maps.MapProfile", {profileId: PROFILE, disabled: "Y"})
        fresh()
        out.profileOff = proxy(READER, "/tiles/v3/1/0/0.pbf")
        change("moqui.maps.MapProfile", {profileId: PROFILE, disabled: "N"})
        change("moqui.maps.MapLayer", {layerId: LAYER, disabled: "Y"})
        fresh()
        out.layerOff = proxy(READER, "/tiles/v3/1/0/0.pbf")
        change("moqui.maps.MapLayer", {layerId: LAYER, disabled: "N", viewPermission: "MAPS_PX_NOT_HELD"})
        fresh()
        out.layerPermission = proxy(READER, "/tiles/v3/1/0/0.pbf")
        change("moqui.maps.MapLayer", {layerId: LAYER, viewPermission: null})
        fresh()
        out.sent = http.count()
        out.again = proxy(READER, "/tiles/v3/1/0/0.pbf")
        return out
    },
    then(out) {
        out.offline.status == 403
        out.other.status == 404
        out.profileOff.status == 404
        out.layerOff.status == 404
        out.layerPermission.status == 404
        out.sent == 0
        out.again.status == 200
    }
})

feature("a key is reached through this server only over https", {
    when() {
        change("moqui.maps.MapProvider", {
            providerId: PROVIDER,
            endpointUrl: "http://tiles.proxy.example/styles/basic/style.json"
        })
        fixtures()
        const tested = retest()
        const sent = http.count()
        change("moqui.maps.MapProvider", {providerId: PROVIDER, endpointUrl: STYLE})
        return {tested, sent}
    },
    then({tested, sent}) {
        tested.passed == false
        JSON.stringify(tested.checkList).includes("https")
        sent == 0
    }
})
