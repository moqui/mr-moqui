// Map archives (plans/Maps.md §5; org.moqui.maps.MapSourceServices, MapSources.js, tools screen/Maps/LocalMap.xml source):
// registering a PMTiles archive records its revision (a DbResource archive in a drive, by its content blob; a file on disk,
// by its SHA-256 and time), and refuses an archive a map cannot draw, a raster layer over vector tiles, a git archive that
// is not a Git LFS object (NOT_LFS) and a caller who may not change Configuration, and registers several layers of one
// archive in one action (the light and dark styles of an extract); the map-source transition answers a
// holder of the drive's share a range (206) with the registered strong ETag and private caching, writing no ResourceRecent,
// and refuses a non-holder, a revoked share and a stale revision (409); the effective configuration hands the same-origin
// URL only to a user the transition lets read. The archive is a tiny valid PMTiles version 3 file made here.
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
const layers = {
    drive: "MAPS_SRC_DRIVE",
    file: "MAPS_SRC_FILE",
    fileUri: "MAPS_SRC_FILE_URI",
    raster: "MAPS_SRC_RASTER",
    git: "MAPS_SRC_GIT"
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
        ec.message.clearErrors()
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const del = (entity, field, values) => ec.entity.find(entity).condition(field, "in", values).deleteAll()
        del("moqui.maps.MapLayer", "layerId", Object.values(layers))
        del("moqui.resource.ResourceShare", "shareId", [SHARE])
        del("moqui.resource.ResourceDrive", "driveId", [DRIVE])
        del("moqui.maps.MapProvider", "providerId", [PROVIDER_OFF])
        for (const location of [DRIVE_LOCATION, BAD_LOCATION, FILE_LOCATION, OUTSIDE_LOCATION])
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
        Array.from(readiness.checks || []).find(check => check.checkKey == "maps.archive." + layers.drive)?.state == "ready"
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
