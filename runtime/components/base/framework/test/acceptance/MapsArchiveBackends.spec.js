// MapsArchiveBackends (plans/archive/Maps.md §5, §9 acceptance): one PMTiles archive (zoom 0 to 5, every tile ASCII text naming it,
// made by MapsArchiveLib) registered from each of the four backends (a plain file, a DbResource file in a drive, a git
// repository on Postgres and one on the filesystem, each as a Git LFS object), and the same viewport's tile ranges read
// through the map-source transition from each, compared with the tiles, with each backend's time logged; then a replacement:
// a new commit is not what a map reads until it is registered again (the registered version still answers, readiness says a
// newer revision is stored), after which the old revision is 409 and the new one answers; a plain file changed in place is
// 409 until registered again; and a revoked share of the drive is refused at the next request.
// Run when asked: ./moq accept framework --spec MapsArchiveBackends
const S = "org.moqui.maps.MapSourceServices."
const lib = ec.resource.script("component://framework/test/MapsArchiveLib.js")
const USER = "MAPS_BK_USER"
const DRIVE = "MAPS_BK_DRIVE"
const SHARE = "MAPS_BK_SHARE"
// the git repository on Postgres is a drive, shared with the user as the dbresource:// one is
const GIT_DRIVE = "MAPS_BK_GITDRIVE"
const GIT_SHARE = "MAPS_BK_GITSHARE"
const REPO_DB = "MAPS_BK_GITDB"
const REPO_LOCAL = "MAPS_BK_GITFS"
const ROOT = "dbresource://maps-backends-spec"
const backends = [
    {key: "file", layerId: "MAPS_BK_FILE", location: "runtime://maps/maps-backends-spec/region.pmtiles", kind: "file"},
    {key: "dbresource", layerId: "MAPS_BK_DB", location: ROOT + "/maps/region.pmtiles", kind: "content-blob"},
    {key: "git-db", layerId: "MAPS_BK_GITDB", location: "git://" + REPO_DB + "/maps/region.pmtiles", kind: "git-lfs"},
    // outside every drive: its repository's view decides, which the spec's user lacks and an administrator has
    {
        key: "git-fs",
        layerId: "MAPS_BK_GITFS",
        location: "git://" + REPO_LOCAL + "/maps/region.pmtiles",
        kind: "git-lfs",
        reader: "john.doe"
    }
]
// a viewport over Florence at zooms 4 and 5, and the whole world at zoom 2
const VIEWPORT = [10.5, 43.0, 12.5, 44.5]
const MAX_ZOOM = 5
const usernameOf = userId => userId.toLowerCase().replace(/_/g, ".")
const log = (label, value) => ec.logger.warn("MapsArchiveBackends: " + label + " " + JSON.stringify(value))
const sys = run => {
    const was = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!was) ec.artifactExecution.enableAuthz()
    }
}
function made(revision) {
    const tiles = lib.asciiTiles(MAX_ZOOM, revision)
    const built = lib.archive(tiles, {bounds: [-180, -85, 180, 85], center: [11.25, 43.77, 4]})
    const byId = new Map(tiles.map(tile => [tile.tileId, tile]))
    return {bytes: built.bytes, entries: built.entries, byId}
}
const archives = {}
const layerRow = layerId =>
    ec.entity.find("moqui.maps.MapLayer").condition("layerId", layerId).disableAuthz().useCache(false).one()
function register(layerId, resourceLocation) {
    ec.message.clearErrors()
    const out =
        ec.service
            .sync()
            .name(S + "register#ArchiveLayer")
            .parameters({layerId, resourceLocation})
            .call() || {}
    const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
    ec.message.clearErrors()
    return Object.assign({}, out, {error})
}
// the transition as the user: status and output of one range
function read(layerId, revision, range, reader) {
    ec.user.internalLoginUser(reader || usernameOf(USER))
    ec.artifactExecution.enableAuthz()
    try {
        const res = ec.screen
            .makeTest()
            .rootScreen("component://webroot/screen/webroot.xml")
            .header("Range", range)
            .render("apps/maps/LocalMap/source", {layerId, revision}, "get")
        return {
            status: res.status,
            output: String(res.output || ""),
            etag: res.header("ETag"),
            range: res.header("Content-Range")
        }
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
}
const viewport = () => [2, 4, 5].flatMap(z => lib.viewportTiles(z, z === 2 ? [-179, -84, 179, 84] : VIEWPORT))
// each viewport tile's range from a layer: how many matched the archive's tile, the problems, the time
function readViewport(layerId, revision, archive, reader) {
    const started = Date.now()
    const problems = []
    let matched = 0
    for (const tile of viewport()) {
        const entry = archive.entries.find(row => row.tileId === tile.tileId)
        const res = read(layerId, revision, "bytes=" + entry.offset + "-" + (entry.offset + entry.length - 1), reader)
        if (res.status == 206 && res.output === archive.byId.get(tile.tileId).text) matched++
        else problems.push(tile.z + "/" + tile.x + "/" + tile.y + ": " + res.status)
    }
    return {tiles: viewport().length, matched, problems, ms: Date.now() - started}
}

spec({
    timeout: 300,
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        archives.first = made(1)
        archives.second = made(2)
        const make = (entity, fields) => ec.entity.makeValue(entity).setAll(fields).createOrUpdate()
        make("moqui.security.UserAccount", {userId: USER, username: usernameOf(USER), userFullName: "Maps backends spec"})
        make("moqui.resource.ResourceDrive", {
            driveId: DRIVE,
            driveName: "Maps backends spec",
            driveKindEnumId: "RdkShared",
            rootLocation: ROOT,
            ownerUserId: "EX_JOHN_DOE",
            createdDate: ec.user.nowTimestamp
        })
        make("moqui.resource.ResourceShare", {
            shareId: SHARE,
            driveId: DRIVE,
            path: "maps",
            isDirectory: "Y",
            sharedWithUserId: USER,
            permissionEnumId: "RspView",
            createdByUserId: "EX_JOHN_DOE",
            createdDate: ec.user.nowTimestamp
        })
        for (const [repositoryId, repositoryKind] of [
            [REPO_DB, "db"],
            [REPO_LOCAL, "local"]
        ]) {
            make("moqui.resource.GitRepository", {
                repositoryId,
                description: "Maps backends " + repositoryKind,
                repositoryKind,
                branch: "main"
            })
            ec.service
                .sync()
                .name("org.moqui.git.GitServices.init#GitRepository")
                .parameters({repositoryId, contentRepository: true})
                .call()
        }
        make("moqui.resource.ResourceDrive", {
            driveId: GIT_DRIVE,
            driveName: "Maps backends git",
            driveKindEnumId: "RdkShared",
            rootLocation: "git://" + REPO_DB + "/",
            repositoryId: REPO_DB,
            ownerUserId: "EX_JOHN_DOE",
            createdDate: ec.user.nowTimestamp
        })
        make("moqui.resource.ResourceShare", {
            shareId: GIT_SHARE,
            driveId: GIT_DRIVE,
            path: "maps",
            isDirectory: "Y",
            sharedWithUserId: USER,
            permissionEnumId: "RspView",
            createdByUserId: "EX_JOHN_DOE",
            createdDate: ec.user.nowTimestamp
        })
        for (const backend of backends) {
            const started = Date.now()
            lib.put(backend.location, archives.first.bytes)
            log("write " + backend.key, {bytes: archives.first.bytes.length, ms: Date.now() - started})
            make("moqui.maps.MapLayer", {
                layerId: backend.layerId,
                layerName: "Backends " + backend.key,
                formatEnumId: "MLF_PMTILES_VECTOR",
                isBasemap: "Y",
                disabled: "Y"
            })
        }
        ec.message.clearErrors()
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const del = (entity, field, values) => ec.entity.find(entity).condition(field, "in", values).deleteAll()
        del(
            "moqui.maps.MapLayer",
            "layerId",
            backends.map(backend => backend.layerId)
        )
        del("moqui.resource.ResourceShare", "shareId", [SHARE, GIT_SHARE])
        del("moqui.resource.ResourceDrive", "driveId", [DRIVE, GIT_DRIVE])
        for (const backend of backends.slice(0, 2))
            try {
                ec.resource.getLocationReference(backend.location).delete()
            } catch (e) {
                ec.logger.warn("MapsArchiveBackends cleanup: " + backend.location + " not deleted")
            }
        for (const repositoryId of [REPO_DB, REPO_LOCAL])
            try {
                ec.service.sync().name("org.moqui.git.GitServices.delete#GitRepository").parameters({repositoryId}).call()
            } catch (e) {
                ec.logger.warn("MapsArchiveBackends cleanup: repository " + repositoryId + " not deleted")
            }
        ec.message.clearErrors()
        del("moqui.resource.ResourceRecent", "userId", [USER])
        del("moqui.security.UserAccount", "userId", [USER])
    }
})

feature("the same viewport's tile ranges from one archive on each of the four backends", {
    when() {
        const out = {}
        for (const backend of backends) {
            const started = Date.now()
            const registered = register(backend.layerId, backend.location)
            const row = layerRow(backend.layerId)
            const header = read(backend.layerId, row.sourceRevision, "bytes=0-16383", backend.reader)
            const tiles = readViewport(backend.layerId, row.sourceRevision, archives.first, backend.reader)
            // the spec's user, who may view the drives but not the repository outside them
            const user = read(backend.layerId, row.sourceRevision, "bytes=0-126").status
            out[backend.key] = {
                applied: registered.applied,
                reason: registered.reason || registered.error,
                kind: row.sourceAddressKind,
                tag: row.sourceTag,
                headerStatus: header.status,
                headerTag: header.etag,
                user,
                tiles,
                registerMs: Date.now() - started - tiles.ms
            }
            log(backend.key, {
                reason: out[backend.key].reason,
                kind: row.sourceAddressKind,
                registerMs: out[backend.key].registerMs,
                tiles: tiles.tiles,
                viewportMs: tiles.ms
            })
        }
        return out
    },
    then(out) {
        backends.every(backend => out[backend.key].applied)
        out.file.user == 206 && out.dbresource.user == 206 && out["git-db"].user == 206
        out["git-fs"].user == 403
        backends.every(backend => out[backend.key].kind == backend.kind)
        backends.every(
            backend => out[backend.key].headerStatus == 206 && out[backend.key].headerTag == '"' + out[backend.key].tag + '"'
        )
        backends.every(
            backend => out[backend.key].tiles.matched == out[backend.key].tiles.tiles && out[backend.key].tiles.tiles > 10
        )
        // the three content-addressed backends name the same content
        out["git-db"].tag == out["git-fs"].tag && out["git-db"].tag == out.dbresource.tag
    }
})

feature("a replacement is a new revision: the old one still answers until it is registered, then it is 409", {
    when() {
        const gitDb = backends.find(backend => backend.key == "git-db")
        const file = backends.find(backend => backend.key == "file")
        const before = layerRow(gitDb.layerId).sourceRevision
        const fileBefore = layerRow(file.layerId).sourceRevision
        lib.put(gitDb.location, archives.second.bytes)
        lib.put(file.location, archives.second.bytes)
        const entry = archives.first.entries[0]
        const range = "bytes=" + entry.offset + "-" + (entry.offset + entry.length - 1)
        const pinned = read(gitDb.layerId, before, range)
        const changedFile = read(file.layerId, fileBefore, range)
        const readiness = ec.service
            .sync()
            .name(S + "check#ArchiveReadiness")
            .parameters({providerKey: "maps.archives"})
            .call()
        const newer = Array.from(readiness.checks || []).find(check => check.checkKey == "maps.archive." + gitDb.layerId)
        const again = register(gitDb.layerId)
        const fileAgain = register(file.layerId)
        const after = layerRow(gitDb.layerId).sourceRevision
        return {
            before,
            after,
            pinned,
            expected: archives.first.byId.get(entry.tileId).text,
            changedFile,
            newer,
            again,
            fileAgain,
            stale: read(gitDb.layerId, before, range).status,
            tiles: readViewport(gitDb.layerId, after, archives.second),
            fileTiles: readViewport(file.layerId, layerRow(file.layerId).sourceRevision, archives.second)
        }
    },
    then({before, after, pinned, expected, changedFile, newer, again, fileAgain, stale, tiles, fileTiles}) {
        pinned.status == 206 && pinned.output === expected
        changedFile.status == 409
        newer.state == "warning" && newer.reasonCode == "NEWER_REVISION"
        again.applied && again.replaced && after != before
        fileAgain.applied && fileAgain.replaced
        stale == 409
        tiles.matched == tiles.tiles
        fileTiles.matched == fileTiles.tiles
    }
})

feature("a revoked share of the drive is refused at the next range", {
    when() {
        const drive = backends.find(backend => backend.key == "dbresource")
        const revision = layerRow(drive.layerId).sourceRevision
        const entry = archives.first.entries[3]
        const range = "bytes=" + entry.offset + "-" + (entry.offset + entry.length - 1)
        const shared = read(drive.layerId, revision, range).status
        sys(() =>
            ec.entity
                .makeValue("moqui.resource.ResourceShare")
                .setAll({shareId: SHARE, revokedDate: ec.user.nowTimestamp})
                .update()
        )
        const revoked = read(drive.layerId, revision, range).status
        return {shared, revoked}
    },
    then({shared, revoked}) {
        shared == 206
        revoked == 403
    }
})
