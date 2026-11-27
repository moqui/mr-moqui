// Map archives (plans/archive/Maps.md §5; org.moqui.maps.MapSourceServices, MapSources.js, tools screen/Maps/LocalMap.xml source):
// registering a PMTiles archive records its revision (a DbResource archive in a drive, by its content blob; a file on disk,
// by its SHA-256 and time), and refuses an archive a map cannot draw, a raster layer over vector tiles, a git archive that
// is not a Git LFS object (NOT_LFS) and a caller who may not change Configuration, and registers several layers of one
// archive in one action (the light and dark styles of an extract); the map-source transition answers a
// holder of the drive's share a range (206) with the registered strong ETag and private caching, writing no ResourceRecent,
// and refuses a non-holder, a revoked share and a stale revision (409); the effective configuration hands the same-origin
// URL only to a user the transition lets read; the Z/X/Y tile transition answers a holder one tile with its strong ETag
// (and 304, and 204 for no tile) and refuses a non-holder and a stale revision. The archive is a tiny valid PMTiles
// version 3 file made here. A Cloud Optimized GeoTIFF (P4c) is registered by its header (its CRS and bands recorded, one in
// strips refused), read by range by a holder of its share and not by another user, and handed to the holder's map with its
// projection's definition. Readiness's room to replace archives (archiveSpace) is checked on given filesystem rows: room,
// too little on the runtime's or the database's filesystem or for the next backup alone, an external database not measured,
// no filesystems reported and a status map not read. A raster provider whose URL is a COG passes its test by one range of
// it, and one whose URL is no COG, or answers the whole file, fails naming why.
// Run: ./moq test framework --spec MapsSource
const S = "org.moqui.maps.MapSourceServices."
const HOLDER = "MAPS_SRC_HOLDER"
const OTHER = "MAPS_SRC_OTHER"
const DRIVE = "MAPS_SRC_DRIVE"
const SHARE = "MAPS_SRC_SHARE"
const REPO = "MAPS_SRC_REPO"
const ROOT = "dbresource://maps-source-spec"
const DRIVE_LOCATION = ROOT + "/archives/spec.pmtiles"
const FILE_LOCATION = "runtime://maps/maps-source-spec/plain.pmtiles"
// the same file as a file: location, and a file of the server outside the maps directory
const fileUri = () => "file:" + String(ec.factory.getRuntimePath()).replace(/\/+$/, "") + "/maps/maps-source-spec/plain.pmtiles"
const OUTSIDE_LOCATION = "runtime://tmp/maps-source-spec/outside.pmtiles"
const PROVIDER_OFF = "MAPS_SRC_OFF"
const BAD_LOCATION = ROOT + "/archives/broken.pmtiles"
const GIT_LOCATION = "git://" + REPO + "/plain.pmtiles"
const COG_LOCATION = ROOT + "/archives/spec.tif"
const COG_STRIPS = ROOT + "/archives/strips.tif"
const COG_SHARE = "MAPS_SRC_COG_SHARE"
// a provider's COGs (registerCogUrl), answered by the HTTP fixtures
const http = ec.factory.getTool("HttpFixtures")
const COG_HOST = "https://cog.spec.example"
const COG_PROVIDERS = {tested: "MAPS_SRC_COGP", untested: "MAPS_SRC_COGP_NEW", proxied: "MAPS_SRC_COGP_PROXY"}
const COG_URL_LAYERS = {
    good: COG_HOST + "/data/good.tif",
    whole: COG_HOST + "/data/whole.tif",
    noCors: COG_HOST + "/data/nocors.tif",
    otherOrigin: COG_HOST + "/data/other.tif",
    notProviders: "https://elsewhere.spec.example/data/a.tif",
    untested: COG_HOST + "/data/untested.tif",
    proxied: COG_HOST + "/data/proxied.tif"
}
// raster providers whose URL is a COG (a bucket of them): tested by one range of that COG
const COG_BUCKETS = {
    cog: "MAPS_SRC_COGB",
    notCog: "MAPS_SRC_COGB_NOT",
    whole: "MAPS_SRC_COGB_WHOLE",
    noCors: "MAPS_SRC_COGB_NOCORS",
    plain: "MAPS_SRC_COGB_HTTP",
    proxied: "MAPS_SRC_COGB_PROXY",
    keyed: "MAPS_SRC_COGB_KEY"
}
const cogUrlLayer = name => "MAPS_SRC_COGU_" + name.toUpperCase()
const layers = {
    drive: "MAPS_SRC_DRIVE",
    file: "MAPS_SRC_FILE",
    fileUri: "MAPS_SRC_FILE_URI",
    raster: "MAPS_SRC_RASTER",
    git: "MAPS_SRC_GIT",
    cog: "MAPS_SRC_COG"
}
const usernameOf = userId => userId.toLowerCase().replace(/_/g, ".")

const lib = ec.resource.script("component://framework/test/MapsArchiveLib.js")
// A PMTiles version 3 archive of one tile of tileBytes bytes (over the DbResource blob threshold, so it is a content blob read
// by range), zoom 0 to 5
function archive(tileBytes = 70000, seed = 1) {
    const tile = new Uint8Array(tileBytes)
    for (let i = 0; i < tileBytes; i++) tile[i] = (i * 7 + seed) % 251
    return lib.archive([{tileId: 0, bytes: tile}], {minZoom: 0, maxZoom: 5, center: [0, 0, 2]}).bytes
}
const put = lib.put
const make = (entity, fields) => ec.entity.makeValue(entity).setAll(fields).createOrUpdate()
const layerRow = layerId =>
    ec.entity.find("moqui.maps.MapLayer").condition("layerId", layerId).disableAuthz().useCache(false).one()
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
    ec.user.internalLoginUser(userId ? usernameOf(userId) : "john.doe")
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
const register = (layerId, resourceLocation) => as(null, () => invoke(S + "register#ArchiveLayer", {layerId, resourceLocation}))
// signed in as the user with authorization as the spec has it (off): the services' own checks of their caller only
function asSignedIn(userId, action) {
    ec.user.internalLoginUser(usernameOf(userId))
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
    }
}
// the map-source transition as the user, with a Range header when given: status, the headers it is checked by
function source(userId, layerId, revision, range) {
    return as(userId, () => {
        let test = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
        if (range) test = test.header("Range", range)
        const res = test.render("apps/maps/LocalMap/source", {layerId, revision}, "get")
        return {
            status: res.status,
            etag: res.header("ETag"),
            contentRange: res.header("Content-Range"),
            cacheControl: String(res.header("Cache-Control") || ""),
            length: String(res.output || "").length
        }
    })
}
// the tile transition as the user (tile/{layerId}/{revision}/{zxy}), with the request's headers: status and its headers
function tile(userId, layerId, revision, zxy, headers = {}) {
    return as(userId, () => {
        let test = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
        for (const [name, value] of Object.entries(headers)) test = test.header(name, value)
        const res = test.render("apps/maps/LocalMap/tile/" + layerId + "/" + revision + "/" + zxy, {}, "get")
        return {
            status: res.status,
            etag: res.header("ETag"),
            type: String(res.header("Content-Type") || ""),
            cacheControl: String(res.header("Cache-Control") || "")
        }
    })
}
const recentCount = () => ec.entity.find("moqui.resource.ResourceRecent").condition("userId", HOLDER).disableAuthz().count()

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const userId of [HOLDER, OTHER])
            make("moqui.security.UserAccount", {userId, username: usernameOf(userId), userFullName: "Maps source " + userId})
        make("moqui.resource.ResourceDrive", {
            driveId: DRIVE,
            driveName: "Maps source spec",
            driveKindEnumId: "RdkShared",
            rootLocation: ROOT,
            ownerUserId: "EX_JOHN_DOE",
            createdDate: ec.user.nowTimestamp
        })
        make("moqui.resource.ResourceShare", {
            shareId: SHARE,
            driveId: DRIVE,
            path: "archives/spec.pmtiles",
            sharedWithUserId: HOLDER,
            permissionEnumId: "RspView",
            createdByUserId: "EX_JOHN_DOE",
            createdDate: ec.user.nowTimestamp
        })
        put(DRIVE_LOCATION, archive())
        put(FILE_LOCATION, archive(3000, 5))
        const broken = archive(3000)
        broken[7] = 2
        put(BAD_LOCATION, broken)
        // a git repository with no LFS patterns: its .pmtiles file is a plain git object
        make("moqui.resource.GitRepository", {
            repositoryId: REPO,
            description: "Maps source spec",
            repositoryKind: "db",
            branch: "main"
        })
        ec.service.sync().name("org.moqui.git.GitServices.init#GitRepository").parameters({repositoryId: REPO}).call()
        put(GIT_LOCATION, archive(3000, 9))
        const layer = (layerId, formatEnumId) =>
            make("moqui.maps.MapLayer", {layerId, layerName: "Spec " + layerId, formatEnumId, isBasemap: "Y", disabled: "Y"})
        layer(layers.drive, "MLF_PMTILES_VECTOR")
        layer(layers.file, "MLF_PMTILES_VECTOR")
        layer(layers.fileUri, "MLF_PMTILES_VECTOR")
        put(OUTSIDE_LOCATION, archive(3000, 5))
        make("moqui.maps.MapProvider", {
            providerId: PROVIDER_OFF,
            providerName: "Spec archives off",
            providerKindEnumId: "MPK_ARCHIVE",
            disabled: "Y"
        })
        layer(layers.raster, "MLF_PMTILES_RASTER")
        layer(layers.git, "MLF_PMTILES_VECTOR")
        // a COG of the drive, shared with the holder; and one in strips
        put(COG_LOCATION, lib.cog())
        put(COG_STRIPS, lib.cog({strips: true}))
        make("moqui.resource.ResourceShare", {
            shareId: COG_SHARE,
            driveId: DRIVE,
            path: "archives/spec.tif",
            sharedWithUserId: HOLDER,
            permissionEnumId: "RspView",
            createdByUserId: "EX_JOHN_DOE",
            createdDate: ec.user.nowTimestamp
        })
        layer(layers.cog, "MLF_COG")
        // a provider's COGs: a raster provider tested through the fixtures, one never tested and one through the proxy
        http.enable()
        for (const [key, providerId] of Object.entries(COG_PROVIDERS))
            make("moqui.maps.MapProvider", {
                providerId,
                providerName: "Spec COG " + key,
                providerKindEnumId: "MPK_RASTER_TILES",
                endpointUrl: COG_HOST + "/tiles/{z}/{x}/{y}.png",
                proxyRequests: key === "proxied" ? "Y" : "N",
                disabled: "N"
            })
        for (const [name, sourceUrl] of Object.entries(COG_URL_LAYERS))
            make("moqui.maps.MapLayer", {
                layerId: cogUrlLayer(name),
                layerName: "Spec COG URL " + name,
                formatEnumId: "MLF_COG",
                sourceUrl,
                providerId:
                    name === "untested"
                        ? COG_PROVIDERS.untested
                        : name === "proxied"
                          ? COG_PROVIDERS.proxied
                          : COG_PROVIDERS.tested,
                disabled: "Y"
            })
        ec.message.clearErrors()
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const del = (entity, field, values) => ec.entity.find(entity).condition(field, "in", values).deleteAll()
        del("moqui.maps.MapLayer", "layerId", Object.values(layers))
        del("moqui.resource.ResourceShare", "shareId", [SHARE, COG_SHARE])
        del("moqui.resource.ResourceDrive", "driveId", [DRIVE])
        del("moqui.maps.MapProvider", "providerId", [PROVIDER_OFF])
        http.reset()
        del("moqui.maps.MapLayer", "layerId", Object.keys(COG_URL_LAYERS).map(cogUrlLayer))
        del("moqui.server.ConnectionCheck", "connectionKey", Object.values(COG_PROVIDERS))
        del("moqui.maps.MapProvider", "providerId", Object.values(COG_PROVIDERS))
        del("moqui.server.ConnectionCheck", "connectionKey", Object.values(COG_BUCKETS))
        del("moqui.maps.MapLayer", "layerId", ["MAPS_SRC_COGB_XYZ"])
        del("moqui.maps.MapProvider", "providerId", Object.values(COG_BUCKETS))
        for (const location of [DRIVE_LOCATION, BAD_LOCATION, FILE_LOCATION, OUTSIDE_LOCATION, COG_LOCATION, COG_STRIPS])
            try {
                ec.resource.getLocationReference(location).delete()
            } catch (e) {
                ec.logger.warn("MapsSource cleanup: " + location + " not deleted")
            }
        try {
            ec.service.sync().name("org.moqui.git.GitServices.delete#GitRepository").parameters({repositoryId: REPO}).call()
        } catch (e) {
            ec.logger.warn("MapsSource cleanup: repository " + REPO + " not deleted: " + (e.message || e))
        }
        ec.message.clearErrors()
        del("moqui.resource.ResourceRecent", "userId", [HOLDER, OTHER])
        del("moqui.security.UserAccount", "userId", [HOLDER, OTHER])
    }
})

feature("registering records the revision of a drive's archive and a file's, and refuses what a map cannot read", {
    when() {
        return {
            drive: register(layers.drive, DRIVE_LOCATION),
            file: register(layers.file, FILE_LOCATION),
            fileUri: register(layers.fileUri, fileUri()),
            outside: register(layers.raster, OUTSIDE_LOCATION),
            driveRow: layerRow(layers.drive),
            fileRow: layerRow(layers.file),
            broken: register(layers.raster, BAD_LOCATION),
            mismatch: register(layers.raster, DRIVE_LOCATION),
            git: register(layers.git, GIT_LOCATION),
            // the light and dark styles of one archive: one action registers both; a layer of another format refuses it
            together: as(null, () =>
                invoke(S + "register#ArchiveLayer", {
                    layerId: layers.file,
                    resourceLocation: FILE_LOCATION,
                    alsoLayerIds: [layers.fileUri]
                })
            ),
            mixed: as(null, () =>
                invoke(S + "register#ArchiveLayer", {
                    layerId: layers.file,
                    resourceLocation: FILE_LOCATION,
                    alsoLayerIds: [layers.raster]
                })
            ),
            refused: as(HOLDER, () => invoke(S + "register#ArchiveLayer", {layerId: layers.file})),
            readiness: as(null, () => invoke(S + "check#ArchiveReadiness", {providerKey: "maps.archives"}))
        }
    },
    then({drive, file, fileUri, outside, driveRow, fileRow, broken, mismatch, git, together, mixed, refused, readiness}) {
        const checks = Array.from(readiness.checks || [])
        drive.applied && file.applied && fileUri.applied
        layerRow(layers.fileUri).sourceRevision == fileRow.sourceRevision && layerRow(layers.fileUri).sourceAddressKind == "file"
        // outside every drive: registering says every user who may see the layer reads it
        JSON.stringify(file.notes).includes("every user who may see the layer")
        !JSON.stringify(drive.notes || []).includes("every user who may see the layer")
        !outside.applied && outside.reasonCode == "DISK_ROOT_ONLY"
        driveRow.sourceAddressKind == "content-blob" && driveRow.sourceTag == "sha256-" + driveRow.sourceRevision
        !!driveRow.sourceVersionName && driveRow.driveId == DRIVE && driveRow.disabled == "N"
        driveRow.maxZoom == 5 && driveRow.attribution == "Spec data"
        fileRow.sourceAddressKind == "file" && !fileRow.sourceVersionName && !!fileRow.sourceModifiedDate && !fileRow.driveId
        fileRow.sourceTag == "sha256-" + fileRow.sourceRevision && fileRow.sourceRevision.length == 64
        !broken.applied && broken.reasonCode == "ARCHIVE_INVALID" && /version 2/.test(broken.reason)
        !mismatch.applied && mismatch.reasonCode == "TILE_TYPE_MISMATCH"
        !git.applied && git.reasonCode == "NOT_LFS" && /Git LFS/.test(git.reason)
        together.applied && together.layerIds.join() == [layers.file, layers.fileUri].join()
        layerRow(layers.fileUri).resourceLocation == FILE_LOCATION && layerRow(layers.fileUri).disabled == "N"
        !mixed.applied && mixed.reasonCode == "FORMAT_MISMATCH"
        layerRow(layers.raster).disabled == "Y" && !layerRow(layers.git).sourceRevision
        refused.applied !== true && (refused.reasonCode == "PERMISSION_DENIED" || /authoriz/i.test(refused.error))
        checks.find(check => check.checkKey == "maps.archive." + layers.drive)?.state == "ready"
        // this server's own filesystems: measured on Unix
        checks.some(check => check.checkKey == "maps.archive.space" && check.state != "failed")
    }
})

feature("the map-source transition answers a holder a range with the strong ETag, and refuses a non-holder", {
    when() {
        const row = layerRow(layers.drive)
        const before = recentCount()
        return {
            row,
            holder: source(HOLDER, layers.drive, row.sourceRevision, "bytes=0-126"),
            other: source(OTHER, layers.drive, row.sourceRevision, "bytes=0-126"),
            recent: recentCount() - before
        }
    },
    then({row, holder, other, recent}) {
        holder.status == 206
        holder.etag == '"' + row.sourceTag + '"'
        holder.contentRange == "bytes 0-126/" + row.sourceSize
        holder.cacheControl.includes("private") && holder.cacheControl.includes("max-age")
        other.status == 403 && other.etag == null
        recent == 0
    }
})

feature("a stale revision is 409, and a revoked share is refused at the next request", {
    when() {
        const row = layerRow(layers.drive)
        const stale = source(HOLDER, layers.drive, "0".repeat(64), "bytes=0-126")
        const fileRow = layerRow(layers.file)
        const plain = source(OTHER, layers.file, fileRow.sourceRevision, "bytes=127-200")
        ec.entity.makeValue("moqui.resource.ResourceShare").setAll({shareId: SHARE, revokedDate: ec.user.nowTimestamp}).update()
        const revoked = source(HOLDER, layers.drive, row.sourceRevision, "bytes=127-200")
        ec.entity.makeValue("moqui.resource.ResourceShare").setAll({shareId: SHARE, revokedDate: null}).update()
        // the file on disk changed in place (another size): 409 until it is registered again
        put(FILE_LOCATION, archive(3100, 5))
        const changed = source(OTHER, layers.file, fileRow.sourceRevision, "bytes=127-200")
        return {stale, plain, revoked, changed}
    },
    then({stale, plain, revoked, changed}) {
        stale.status == 409
        plain.status == 206 && plain.contentRange.startsWith("bytes 127-200/")
        revoked.status == 403
        changed.status == 409
    }
})

feature("the tile transition answers a holder a tile with its ETag, and refuses a non-holder and a stale revision", {
    when() {
        const row = layerRow(layers.drive)
        const before = recentCount()
        const holder = tile(HOLDER, layers.drive, row.sourceRevision, "0/0/0.mvt")
        return {
            row,
            holder,
            again: tile(HOLDER, layers.drive, row.sourceRevision, "0/0/0", {"If-None-Match": holder.etag || "none"}),
            none: tile(HOLDER, layers.drive, row.sourceRevision, "1/0/0"),
            other: tile(OTHER, layers.drive, row.sourceRevision, "0/0/0"),
            stale: tile(HOLDER, layers.drive, "0".repeat(64), "0/0/0"),
            recent: recentCount() - before
        }
    },
    then({row, holder, again, none, other, stale, recent}) {
        holder.status == 200 && holder.type.startsWith("application/vnd.mapbox-vector-tile")
        holder.etag == '"' + row.sourceTag + '.0"'
        holder.cacheControl.includes("private") && holder.cacheControl.includes("immutable")
        again.status == 304
        none.status == 204
        other.status == 403 && other.etag == null
        stale.status == 409
        recent == 0
    }
})

feature("the effective configuration hands the archive's same-origin URL only to a user the transition lets read", {
    when() {
        const config = userId =>
            as(userId, () => invoke("org.moqui.maps.MapServices.get#EffectiveConfig", {layerIds: layers.drive})).config || {}
        return {holder: config(HOLDER), other: config(OTHER), revision: layerRow(layers.drive).sourceRevision}
    },
    then({holder, other, revision}) {
        holder.layers.length == 1 && holder.layers[0].kind == "pmtiles"
        holder.layers[0].url == "/apps/maps/LocalMap/source?layerId=" + layers.drive + "&revision=" + revision
        other.layers.length == 0
        other.layerErrors.some(error => error.id == layers.drive && error.reasonCode == "LAYER_MISSING")
        !JSON.stringify(other).includes("/LocalMap/source")
    }
})

feature("a layer's viewPermission, a disabled provider and no signed-in user each refuse the archive", {
    when() {
        const row = layerRow(layers.drive)
        const read = () => source(HOLDER, layers.drive, row.sourceRevision, "bytes=0-126").status
        const update = fields =>
            ec.entity
                .makeValue("moqui.maps.MapLayer")
                .setAll(Object.assign({layerId: layers.drive}, fields))
                .update()
        const shared = read()
        update({viewPermission: "MAPS_SRC_NOT_HELD"})
        const unpermitted = read()
        update({viewPermission: null, providerId: PROVIDER_OFF})
        const providerOff = read()
        update({providerId: null})
        ec.user.logoutUser()
        let anonymous = null
        try {
            anonymous = ec.service
                .sync()
                .name(S + "get#SourceAccess")
                .parameters({layerId: layers.drive, revision: row.sourceRevision})
                .disableAuthz()
                .call()
        } finally {
            ec.message.clearErrors()
            ec.user.internalLoginUser("john.doe")
        }
        return {shared, unpermitted, providerOff, anonymous}
    },
    then({shared, unpermitted, providerOff, anonymous}) {
        shared == 206
        unpermitted == 404
        providerOff == 404
        anonymous.ok == false && anonymous.status == 403 && !anonymous.location
    }
})

feature("a Cloud Optimized GeoTIFF is registered by its header and read by range by a holder of its share only", {
    when() {
        const registered = register(layers.cog, COG_LOCATION)
        const row = layerRow(layers.cog)
        const effective = as(HOLDER, () => invoke("org.moqui.maps.MapServices.get#EffectiveConfig", {layerIds: layers.cog}))
        const config = effective.config || {}
        return {
            registered,
            row,
            raster: JSON.parse(row.rasterJson || "{}"),
            strips: register(layers.cog, COG_STRIPS),
            holder: source(HOLDER, layers.cog, row.sourceRevision, "bytes=0-1023"),
            other: source(OTHER, layers.cog, row.sourceRevision, "bytes=0-1023"),
            drawn: Array.from(config.layers || []).find(layer => layer.id == layers.cog) || {},
            projection: Array.from(config.projections || []).find(projection => projection.code == "EPSG:32633") || {},
            // the holder's own band settings: a band the COG lacks refused, one it has kept and drawn
            ...asSignedIn(HOLDER, () => {
                const refusedBands = invoke("org.moqui.maps.MapServices.store#Preferences", {
                    rasters: {[layers.cog]: {band: 2, min: 0, max: 100}}
                })
                const kept = invoke("org.moqui.maps.MapServices.store#Preferences", {
                    rasters: {[layers.cog]: {band: 1, min: 5, max: 200, colormap: "terrain"}}
                })
                const after = invoke("org.moqui.maps.MapServices.get#EffectiveConfig", {layerIds: layers.cog}).config || {}
                invoke("org.moqui.maps.MapServices.store#Preferences", {rasters: {[layers.cog]: null}})
                return {refusedBands, kept, styled: Array.from(after.layers || []).find(layer => layer.id == layers.cog) || {}}
            })
        }
    },
    then({registered, row, raster, strips, holder, other, drawn, projection, refusedBands, kept, styled}) {
        registered.applied && row.crs == "EPSG:32633" && row.disabled == "N"
        raster.bands == 1 && raster.sampleType == "uint8" && raster.nodata == "0" && raster.defaults.band == 1
        // UTM zone 33N from 500 km east, 6000 km north: about 15°E, 54°N
        row.minLongitude > 14.9 && row.maxLongitude < 15.2 && row.minLatitude > 54 && row.maxLatitude < 54.2
        !strips.applied && strips.reasonCode == "COG_INVALID" && /strips/.test(strips.reason)
        // a refusal changes nothing: the layer keeps its registered revision
        layerRow(layers.cog).sourceRevision == row.sourceRevision
        holder.status == 206 && holder.etag == '"' + row.sourceTag + '"'
        other.status == 403
        drawn.kind == "cog" && drawn.raster.projection == "EPSG:32633" && drawn.raster.style.band == 1
        drawn.url.includes("layerId=" + layers.cog)
        projection.proj4.includes("+zone=33")
        refusedBands.applied === false && refusedBands.reasonCode == "RASTER_INVALID"
        kept.applied === true &&
            styled.raster.own === true &&
            styled.raster.style.colormap == "terrain" &&
            styled.raster.style.min == 5
    }
})

feature("a provider's COG is read by one range from its URL: a 206 this site may read by CORS, on a tested provider's host", {
    when() {
        const image = lib.cog()
        http.clear()
        http.add({
            method: "GET",
            url: COG_HOST + "/tiles/0/0/0.png",
            contentType: "image/png",
            body: Uint8Array.from([137, 80, 78, 71])
        })
        http.add({
            method: "GET",
            url: COG_URL_LAYERS.good,
            contentType: "image/tiff",
            body: image,
            ranges: true,
            headers: {"Access-Control-Allow-Origin": "*"}
        })
        http.add({
            method: "GET",
            url: COG_URL_LAYERS.whole,
            contentType: "image/tiff",
            body: image,
            headers: {"Access-Control-Allow-Origin": "*"}
        })
        http.add({method: "GET", url: COG_URL_LAYERS.noCors, contentType: "image/tiff", body: image, ranges: true})
        http.add({
            method: "GET",
            url: COG_URL_LAYERS.otherOrigin,
            contentType: "image/tiff",
            body: image,
            ranges: true,
            headers: {"Access-Control-Allow-Origin": "https://elsewhere.spec.example"}
        })
        const tested = as(null, () =>
            invoke("org.moqui.admin.ConfigurationServices.test#Connection", {
                adapterKey: "mapProvider",
                connectionKey: COG_PROVIDERS.tested
            })
        )
        const out = {tested}
        for (const name of Object.keys(COG_URL_LAYERS)) out[name] = register(cogUrlLayer(name))
        out.row = layerRow(cogUrlLayer("good"))
        out.ranged = http.requests().filter(request => request.url == COG_URL_LAYERS.good)
        return out
    },
    then(out) {
        out.tested.error == "" || !out.tested.error
        out.good.applied === true && out.row.crs == "EPSG:32633" && out.row.disabled == "N" && !out.row.sourceRevision
        JSON.parse(out.row.rasterJson).cors === true && JSON.parse(out.row.rasterJson).bands == 1
        out.ranged.length == 1 && out.ranged[0].headers.Range == "bytes=0-65535" && !out.ranged[0].headers.Cookie
        out.whole.applied === false && out.whole.reasonCode == "NO_RANGES"
        out.noCors.applied === false && out.noCors.reasonCode == "NO_CORS"
        out.otherOrigin.applied === false && out.otherOrigin.reasonCode == "NO_CORS"
        out.notProviders.applied === false && out.notProviders.reasonCode == "URL_NOT_PROVIDERS"
        out.untested.applied === false && out.untested.reasonCode == "PROVIDER_UNTESTED"
        out.proxied.applied === false && out.proxied.reasonCode == "PROVIDER_PROXIED"
    }
})

feature("a raster provider whose URL is a COG is tested by one range of that COG, and a URL that is no COG fails by name", {
    when() {
        const urls = {
            cog: COG_HOST + "/bucket/scene.tif",
            notCog: COG_HOST + "/bucket/picture.tif",
            whole: COG_HOST + "/bucket/whole.tiff",
            noCors: COG_HOST + "/bucket/nocors.tif",
            plain: COG_HOST.replace("https:", "http:") + "/bucket/scene.tif",
            proxied: COG_HOST + "/bucket/scene.tif",
            keyed: COG_HOST + "/bucket/scene.tif"
        }
        http.clear()
        const cors = {"Access-Control-Allow-Origin": "*"}
        http.add({method: "GET", url: urls.cog, contentType: "image/tiff", body: lib.cog(), ranges: true, headers: cors})
        http.add({
            method: "GET",
            url: urls.notCog,
            contentType: "image/tiff",
            body: Uint8Array.from([137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13]),
            ranges: true,
            headers: cors
        })
        http.add({method: "GET", url: urls.whole, contentType: "image/tiff", body: lib.cog(), headers: cors})
        http.add({method: "GET", url: urls.noCors, contentType: "image/tiff", body: lib.cog(), ranges: true})
        const out = {}
        for (const [key, providerId] of Object.entries(COG_BUCKETS)) {
            make("moqui.maps.MapProvider", {
                providerId,
                providerName: "Spec COG bucket " + key,
                providerKindEnumId: "MPK_RASTER_TILES",
                endpointUrl: urls[key],
                proxyRequests: key === "proxied" ? "Y" : "N",
                apiKey: key === "keyed" ? "spec-key" : null,
                disabled: "N"
            })
            const tested = as(null, () =>
                invoke("org.moqui.admin.ConfigurationServices.test#Connection", {
                    adapterKey: "mapProvider",
                    connectionKey: providerId
                })
            )
            out[key] = {
                passed: tested.passed,
                texts: Array.from(tested.checkList || [])
                    .map(row => String(row.text))
                    .join(" | ")
            }
        }
        out.asked = http.requests().filter(request => request.url == urls.cog)
        out.plainAsked = http.requests().filter(request => request.url == urls.plain).length
        // a tile layer of the COG provider with no URL of its own is not handed the GeoTIFF as its template
        make("moqui.maps.MapLayer", {
            layerId: "MAPS_SRC_COGB_XYZ",
            layerName: "Spec COG bucket tiles",
            formatEnumId: "MLF_RASTER_XYZ",
            providerId: COG_BUCKETS.cog,
            disabled: "N"
        })
        const tiles = as(null, () => invoke("org.moqui.maps.MapServices.get#EffectiveConfig", {layerIds: "MAPS_SRC_COGB_XYZ"}))
        out.tileErrors = Array.from(tiles.config?.layerErrors || []).map(error => error.reasonCode)
        out.tileLayers = Array.from(tiles.config?.layers || []).length
        // an error the caller already has is not the header reader's
        const sources = ec.resource.script("component://framework/service/org/moqui/maps/MapSources.js")
        ec.message.addError("an earlier error")
        out.withError = sources.readCogHead(urls.cog, false)
        out.errorKept = ec.message.getErrorsString().includes("an earlier error")
        ec.message.clearErrors()
        out.withCredentials = sources.readCogHead("https://user:pass@cog.spec.example/bucket/scene.tif", false)
        // what a COG endpoint is: a .tif or .tiff path, any case, with no placeholder
        const isCog = ec.resource.script("component://framework/service/org/moqui/maps/MapConfig.js").cogEndpoint
        out.detected = [
            "https://h/x.png?.tif",
            "https://h/x.tif?{z}",
            "https://h/X.TIFF",
            "https://h/a.tif?v=2",
            "https://h/{z}.tif"
        ]
            .map(url => isCog(url))
            .join()
        return out
    },
    then({
        cog,
        notCog,
        whole,
        noCors,
        plain,
        proxied,
        keyed,
        asked,
        plainAsked,
        withError,
        errorKept,
        withCredentials,
        detected,
        tileErrors,
        tileLayers
    }) {
        cog.passed === true && cog.texts.includes("Cloud Optimized GeoTIFF") && cog.texts.includes("EPSG:32633")
        asked.length == 1 && asked[0].headers.Range == "bytes=0-65535" && !asked[0].headers.Cookie
        notCog.passed === false && notCog.texts.includes("(COG_INVALID)")
        whole.passed === false && whole.texts.includes("(NO_RANGES)")
        noCors.passed === false && noCors.texts.includes("(NO_CORS)")
        // registering refuses an http URL, so its test does too, asking nothing
        plain.passed === false && plain.texts.includes("(LOCATION_INVALID)") && plainAsked == 0
        proxied.passed === false && proxied.texts.includes("not reached through this server")
        keyed.passed === false && keyed.texts.includes("has no key")
        withError.ok === true && errorKept === true
        withCredentials.ok === false && withCredentials.reasonCode == "LOCATION_INVALID"
        detected == "false,false,true,true,false"
        tileLayers == 0 && tileErrors.join() == "SOURCE_NOT_TILES"
    }
})

feature("readiness says whether the filesystem an archive is on has room to replace it, from given rows", {
    when() {
        const sources = ec.resource.script("component://framework/service/org/moqui/maps/MapSources.js")
        const space = sources.archiveSpace
        const MIB = 1048576
        const archive = (layerId, resourceLocation, mib) => ({
            layerId,
            resourceLocation,
            sourceRevision: "r" + layerId,
            sourceSize: mib * MIB
        })
        // a disk archive of 100 MiB, a drive's of 50 MiB drawn by two layers (counted once), and one never registered
        const rows = [
            archive("DISK", "runtime://maps/region.pmtiles", 100),
            archive("LIGHT", "dbresource://drive/region.pmtiles", 50),
            archive("DARK", "dbresource://drive/region.pmtiles", 50),
            {layerId: "NEW", resourceLocation: "runtime://maps/new.pmtiles", sourceSize: 900 * MIB}
        ]
        const runtimeRow = availableBytes => ({mountPoint: "/", holds: ["runtime", "system temp"], availableBytes})
        const backup = {embedded: true, holds: "database"}
        return {
            // one filesystem: the largest (100 MiB) once more and the drive's 50 MiB for the next local backup
            fits: space(rows, [runtimeRow(10240 * MIB)], backup),
            tight: space(rows, [runtimeRow(149 * MIB)], backup),
            // runtime/db on a filesystem of its own: the drive's archive and its backup are measured there
            dbShort: space(
                rows,
                [runtimeRow(10240 * MIB), {mountPoint: "/srv/db", holds: ["database"], availableBytes: 60 * MIB}],
                backup
            ),
            external: space([rows[1]], [runtimeRow(10240 * MIB)], {embedded: false, holds: null}),
            // the backups written under the runtime, apart from runtime/db: that filesystem holds only the next backup
            backupOnly: space(
                [rows[1]],
                [runtimeRow(30 * MIB), {mountPoint: "/srv/db", holds: ["database"], availableBytes: 10240 * MIB}],
                {embedded: true, holds: "runtime"}
            ),
            unread: space(rows, [], backup),
            statusUnread: space(rows, null, backup),
            none: space([rows[3]], [runtimeRow(MIB)], backup),
            // a read of the server that throws is the row's, and with no registered archive there is none
            thrown: sources.spaceCheck(rows, () => {
                throw new Error("spec: the status map is gone")
            }),
            thrownNone: sources.spaceCheck([rows[3]], () => {
                throw new Error("spec: the status map is gone")
            }),
            // where the content blobs are: transactional and content only
            embedded: [
                [{group: "transactional", embedded: true}],
                [
                    {group: "transactional", embedded: true},
                    {group: "content", embedded: true}
                ],
                [
                    {group: "transactional", embedded: true},
                    {group: "content", embedded: false}
                ],
                [
                    {group: "transactional", embedded: false},
                    {group: "content", embedded: true}
                ],
                [
                    {group: "transactional", embedded: true},
                    {group: "transactional#clone1", embedded: false}
                ],
                [
                    {group: "transactional", embedded: true},
                    {group: "logging", embedded: null}
                ],
                []
            ]
                .map(sources.embeddedOf)
                .join()
        }
    },
    then({fits, tight, dbShort, external, backupOnly, unread, statusUnread, none, thrown, thrownNone, embedded}) {
        fits.checkKey == "maps.archive.space" && fits.state == "ready" && fits.fixRoute == "mapsConfiguration"
        fits.reasonCode === null
        fits.message == "/ has 10 GiB free, room for 150 MiB"
        tight.state == "warning" && tight.reasonCode == "ARCHIVE_SPACE_LOW"
        tight.message.startsWith("/ has 149 MiB free and needs 150 MiB")
        dbShort.state == "warning" && dbShort.message.startsWith("/srv/db has 60 MiB free and needs 100 MiB")
        !dbShort.message.includes("/ has")
        external.state == "unavailable" && external.reasonCode == "ARCHIVE_SPACE_EXTERNAL_DB"
        external.message.includes("external database")
        backupOnly.state == "warning" && backupOnly.reasonCode == "ARCHIVE_SPACE_LOW"
        backupOnly.message.startsWith("/ has 30 MiB free and needs 50 MiB: the database's archives once more in the next local")
        !backupOnly.message.includes("largest")
        unread.state == "unavailable" && unread.reasonCode == "ARCHIVE_SPACE_UNREAD" && unread.message.includes("only on Unix")
        statusUnread.reasonCode == "ARCHIVE_SPACE_UNREAD" && !statusUnread.message.includes("Unix")
        none === null
        thrown.state == "unavailable" && thrown.reasonCode == "ARCHIVE_SPACE_UNREAD"
        thrownNone === null
        embedded == "true,true,false,false,true,true,false"
    }
})
